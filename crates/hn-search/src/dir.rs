//! 目录搜索与目录替换（跨平台）。
//!
//! - 遍历用 `ignore`（ripgrep 的遍历组件）：多线程、遵守 `.gitignore`、按通配符过滤
//! - 跳过二进制文件（含 NUL 且不像 UTF-16）、HurshNote 加密文件（魔数）、超过大小上限的文件
//! - 每个文件按 `hn-text` 嗅探编码后解码为 UTF-8 再用 PCRE2 匹配，因此 GBK、UTF-16 文件也能搜
//! - 结果按文件合批，每隔一段时间通过回调交给调用方（UI 线程负责把它们放进列表）

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hn_text::encoding::{self, DetectHints, Detection, EncodingInfo, TextEncoding};

use crate::{Error, Options, Searcher};

/// 每个文件最多记录的匹配数（计数不受影响）。
pub const MAX_HITS_PER_FILE: usize = 10_000;
/// 预览文字的最大字符数。
const PREVIEW_CHARS: usize = 400;
const ENCRYPTED_MAGIC: &[u8] = b"HURSHENC";

/// 一次目录搜索的条件。
#[derive(Debug, Clone)]
pub struct DirQuery {
    pub roots: Vec<PathBuf>,
    pub pattern: String,
    pub opts: Options,
    /// 只搜这些通配符匹配的文件（如 `*.rs`），空表示全部
    pub include: Vec<String>,
    /// 跳过的目录或文件（如 `node_modules`、`*.min.js`）
    pub exclude: Vec<String>,
    pub gitignore: bool,
    /// 包含隐藏文件与目录
    pub hidden: bool,
    /// 超过此大小的文件跳过
    pub max_file_size: u64,
    pub hints: DetectHints,
    /// 工作线程数（0 表示自动）
    pub threads: usize,
}

impl DirQuery {
    pub fn new(roots: Vec<PathBuf>, pattern: &str, opts: Options) -> DirQuery {
        DirQuery {
            roots,
            pattern: pattern.to_string(),
            opts,
            include: Vec::new(),
            exclude: vec!["node_modules".into()],
            gitignore: true,
            hidden: false,
            max_file_size: 64 * 1024 * 1024,
            hints: DetectHints::default(),
            threads: 0,
        }
    }
}

/// 把 `"*.rs; *.py"` 这样的列表拆成通配符。
pub fn split_globs(s: &str) -> Vec<String> {
    s.split([';', ',', ' '])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

/// 一处匹配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineHit {
    /// 行号（0 起始）
    pub line: u64,
    /// 匹配在该行中的起止（UTF-8 字节，相对行首；跨行匹配截到行尾）
    pub col: usize,
    pub col_end: usize,
    /// 行的文字（截断到约 400 个字符，不含换行符）
    pub text: String,
}

/// 一个文件中的匹配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHits {
    pub path: PathBuf,
    pub encoding: EncodingInfo,
    /// 匹配总数（可能多于 `hits` 的条数）
    pub count: usize,
    pub hits: Vec<LineHit>,
}

/// 进度。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub files_scanned: u64,
    pub files_matched: u64,
    pub matches: u64,
    pub skipped_binary: u64,
    pub skipped_large: u64,
    pub skipped_encrypted: u64,
}

/// 单个文件的处理结果。
#[derive(Debug)]
pub enum FileOutcome {
    Hits(FileHits),
    NoMatch,
    Binary,
    TooLarge,
    Encrypted,
    Unreadable,
}

/// 文件内容解码后的文本（UTF-8 文件直接借用，避免复制）。
pub struct Decoded<'a> {
    pub text: std::borrow::Cow<'a, str>,
    pub info: EncodingInfo,
    pub lossy: bool,
}

/// 按嗅探的编码解码文件内容。二进制返回 None。
pub fn decode_file<'a>(bytes: &'a [u8], hints: &DetectHints) -> Option<Decoded<'a>> {
    let d = encoding::detect(bytes, hints);
    if d.how == Detection::Binary {
        return None;
    }
    if d.info.encoding == TextEncoding::Utf8 {
        let body = if d.info.bom { &bytes[3..] } else { bytes };
        if let Ok(s) = std::str::from_utf8(body) {
            return Some(Decoded {
                text: std::borrow::Cow::Borrowed(s),
                info: d.info,
                lossy: false,
            });
        }
    }
    let dec = encoding::decode_as(bytes, d.info.encoding);
    Some(Decoded {
        text: std::borrow::Cow::Owned(dec.text),
        info: dec.info,
        lossy: dec.lossy,
    })
}

fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 在文本中查找，返回 (匹配总数, 匹配列表)。
pub fn find_in_text(searcher: &Searcher, text: &str) -> Result<(usize, Vec<LineHit>), Error> {
    let spans = searcher.find_all(text, 0..text.len())?;
    let count = spans.len();
    let bytes = text.as_bytes();
    let mut hits = Vec::with_capacity(count.min(MAX_HITS_PER_FILE));
    let mut line = 0u64;
    let mut pos = 0usize; // 已统计换行到这个位置
    let mut line_start = 0usize;
    for &(s, e) in spans.iter().take(MAX_HITS_PER_FILE) {
        // 推进到 s 所在的行
        for (i, &b) in bytes[pos..s].iter().enumerate() {
            if b == b'\n' {
                line += 1;
                line_start = pos + i + 1;
            }
        }
        pos = s;
        let line_end = bytes[s..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| s + i)
            .unwrap_or(bytes.len());
        let content_end = if line_end > line_start && bytes[line_end - 1] == b'\r' {
            line_end - 1
        } else {
            line_end
        };
        let shown_end = floor_char(text, (line_start + PREVIEW_CHARS * 4).min(content_end));
        let mut shown = &text[line_start..shown_end];
        if shown.chars().count() > PREVIEW_CHARS {
            let cut = shown
                .char_indices()
                .nth(PREVIEW_CHARS)
                .map(|(i, _)| i)
                .unwrap_or(shown.len());
            shown = &shown[..cut];
        }
        hits.push(LineHit {
            line,
            col: s - line_start,
            col_end: e.min(content_end).max(s) - line_start,
            text: shown.replace('\t', "    "),
        });
    }
    Ok((count, hits))
}

/// 读取并搜索一个文件。
pub fn search_file(path: &Path, searcher: &Searcher, q: &DirQuery) -> FileOutcome {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > q.max_file_size => return FileOutcome::TooLarge,
        Ok(_) => {}
        Err(_) => return FileOutcome::Unreadable,
    }
    let Ok(bytes) = std::fs::read(path) else {
        return FileOutcome::Unreadable;
    };
    if bytes.starts_with(ENCRYPTED_MAGIC) {
        return FileOutcome::Encrypted;
    }
    let Some(dec) = decode_file(&bytes, &q.hints) else {
        return FileOutcome::Binary;
    };
    match find_in_text(searcher, &dec.text) {
        Ok((0, _)) | Err(_) => FileOutcome::NoMatch,
        Ok((count, hits)) => FileOutcome::Hits(FileHits {
            path: path.to_path_buf(),
            encoding: dec.info,
            count,
            hits,
        }),
    }
}

fn build_walker(q: &DirQuery) -> Result<ignore::WalkParallel, String> {
    let first = q.roots.first().ok_or("没有指定目录")?;
    let mut b = ignore::WalkBuilder::new(first);
    for r in &q.roots[1..] {
        b.add(r);
    }
    b.hidden(!q.hidden)
        .git_ignore(q.gitignore)
        .git_global(q.gitignore)
        .git_exclude(q.gitignore)
        .ignore(q.gitignore)
        .parents(q.gitignore)
        .require_git(false)
        .follow_links(false)
        .threads(q.threads);
    let mut ob = ignore::overrides::OverrideBuilder::new(first);
    for g in &q.include {
        ob.add(g).map_err(|e| format!("文件类型“{g}”无效：{e}"))?;
    }
    for g in &q.exclude {
        let pat = format!("!{}", g.trim_end_matches(['/', '\\']));
        ob.add(&pat).map_err(|e| format!("排除项“{g}”无效：{e}"))?;
    }
    let ov = ob.build().map_err(|e| e.to_string())?;
    b.overrides(ov);
    Ok(b.build_parallel())
}

/// 搜索目录。每隔约 `interval` 调用一次 `emit(新发现的文件, 当前进度)`，结束时再调用一次。
/// 返回最终进度；`cancel` 置位后尽快返回。
pub fn search_dir(
    q: &DirQuery,
    cancel: &AtomicBool,
    interval: Duration,
    emit: &(dyn Fn(Vec<FileHits>, Progress) + Sync),
) -> Result<Progress, String> {
    // 先编译一次，报告表达式错误
    Searcher::new(&q.pattern, &q.opts).map_err(|e| e.to_string())?;
    let walker = build_walker(q)?;
    let counters: [AtomicU64; 6] = Default::default();
    let pending: Mutex<(Vec<FileHits>, Instant)> = Mutex::new((Vec::new(), Instant::now()));
    let progress = |c: &[AtomicU64; 6]| Progress {
        files_scanned: c[0].load(Ordering::Relaxed),
        files_matched: c[1].load(Ordering::Relaxed),
        matches: c[2].load(Ordering::Relaxed),
        skipped_binary: c[3].load(Ordering::Relaxed),
        skipped_large: c[4].load(Ordering::Relaxed),
        skipped_encrypted: c[5].load(Ordering::Relaxed),
    };
    walker.run(|| {
        let searcher = Searcher::new(&q.pattern, &q.opts).ok();
        let counters = &counters;
        let pending = &pending;
        let progress = &progress;
        Box::new(move |entry| {
            if cancel.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }
            let Some(searcher) = &searcher else {
                return ignore::WalkState::Quit;
            };
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return ignore::WalkState::Continue;
            }
            counters[0].fetch_add(1, Ordering::Relaxed);
            let hit = match search_file(entry.path(), searcher, q) {
                FileOutcome::Hits(h) => Some(h),
                FileOutcome::Binary => {
                    counters[3].fetch_add(1, Ordering::Relaxed);
                    None
                }
                FileOutcome::TooLarge => {
                    counters[4].fetch_add(1, Ordering::Relaxed);
                    None
                }
                FileOutcome::Encrypted => {
                    counters[5].fetch_add(1, Ordering::Relaxed);
                    None
                }
                FileOutcome::NoMatch | FileOutcome::Unreadable => None,
            };
            let mut p = pending.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(h) = hit {
                counters[1].fetch_add(1, Ordering::Relaxed);
                counters[2].fetch_add(h.count as u64, Ordering::Relaxed);
                p.0.push(h);
            }
            if p.1.elapsed() >= interval {
                p.1 = Instant::now();
                let batch = std::mem::take(&mut p.0);
                drop(p);
                emit(batch, progress(counters));
            }
            ignore::WalkState::Continue
        })
    });
    let rest = std::mem::take(&mut pending.lock().unwrap_or_else(|e| e.into_inner()).0);
    let fin = progress(&counters);
    emit(rest, fin);
    Ok(fin)
}

/// 替换文本中的所有匹配，返回 (新文本, 替换数)。
pub fn replace_in_text(
    searcher: &Searcher,
    text: &str,
    template: &str,
) -> Result<(String, usize), Error> {
    let spans = searcher.find_all(text, 0..text.len())?;
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for &(s, e) in &spans {
        out.push_str(&text[last..s]);
        out.push_str(&searcher.replacement(text, (s, e), template)?);
        last = e;
    }
    out.push_str(&text[last..]);
    Ok((out, spans.len()))
}

/// 对文件内容执行替换，返回按原编码（保留 BOM）编码后的新内容与替换数。
/// 解码有损、或新内容无法用原编码表示时返回错误说明（不应写回）。
pub fn replace_in_bytes(
    bytes: &[u8],
    searcher: &Searcher,
    template: &str,
    hints: &DetectHints,
) -> Result<Option<(Vec<u8>, usize)>, String> {
    if bytes.starts_with(ENCRYPTED_MAGIC) {
        return Err("加密文件".into());
    }
    let dec = decode_file(bytes, hints).ok_or("二进制文件")?;
    if dec.lossy {
        return Err(format!("按 {} 解码有损，跳过", dec.info.label()));
    }
    let (text, n) = replace_in_text(searcher, &dec.text, template).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    let enc = encoding::encode(&text, dec.info);
    if enc.unmappable > 0 {
        return Err(format!(
            "有 {} 个字符无法用 {} 表示，跳过",
            enc.unmappable,
            dec.info.label()
        ));
    }
    Ok(Some((enc.bytes, n)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hn-dir-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn opts(mode: Mode) -> Options {
        Options {
            mode,
            case_sensitive: true,
            ..Default::default()
        }
    }

    #[test]
    fn find_in_text_lines_and_columns() {
        let s = Searcher::new("foo", &opts(Mode::Normal)).unwrap();
        let (n, hits) = find_in_text(&s, "a\r\nxx foo foo\r\n\r\n中foo").unwrap();
        assert_eq!(n, 3);
        assert_eq!(hits[0].line, 1);
        assert_eq!((hits[0].col, hits[0].col_end), (3, 6));
        assert_eq!(hits[0].text, "xx foo foo");
        assert_eq!(hits[1].col, 7);
        assert_eq!(hits[2].line, 3);
        assert_eq!(hits[2].col, 3);
        assert_eq!(hits[2].text, "中foo");
    }

    #[test]
    fn walks_filters_and_skips() {
        let d = tmp("walk");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join("src/a.rs"), "fn needle() {}\n// needle\n").unwrap();
        std::fs::write(d.join("src/b.txt"), "needle here\n").unwrap();
        std::fs::write(d.join("node_modules/x/c.rs"), "needle").unwrap();
        std::fs::write(d.join("target/d.rs"), "needle").unwrap();
        std::fs::write(d.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(d.join("bin.dat"), b"needle\0\x01\x02").unwrap();
        std::fs::write(d.join("secret.hne"), b"HURSHENC needle").unwrap();
        let gbk_text = "这是一段用于测试简体中文编码识别的文字，需要足够长。\n中文 needle 测试\n";
        let (gbk, _, _) = encoding::encoding_rs::GBK.encode(gbk_text);
        std::fs::write(d.join("gbk.txt"), &gbk).unwrap();

        let mut q = DirQuery::new(vec![d.clone()], "needle", opts(Mode::Normal));
        q.hints = DetectHints::for_codepage(936);
        let found = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);
        let p = search_dir(&q, &cancel, Duration::from_millis(10), &|batch, _| {
            found.lock().unwrap().extend(batch);
        })
        .unwrap();
        let mut names: Vec<String> = found
            .lock()
            .unwrap()
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.rs", "b.txt", "gbk.txt"]);
        assert_eq!(p.files_matched, 3);
        assert_eq!(p.matches, 4);
        assert_eq!(p.skipped_binary, 1);
        assert_eq!(p.skipped_encrypted, 1);
        let g = found
            .lock()
            .unwrap()
            .iter()
            .find(|f| f.path.ends_with("gbk.txt"))
            .cloned()
            .unwrap();
        assert_eq!(g.hits[0].text, "中文 needle 测试");
        assert_eq!(g.hits[0].line, 1);
        assert_eq!(g.hits[0].col, "中文 ".len());

        // 只搜 *.rs
        q.include = split_globs("*.rs");
        let found = Mutex::new(Vec::new());
        search_dir(&q, &cancel, Duration::from_millis(10), &|b, _| {
            found.lock().unwrap().extend(b)
        })
        .unwrap();
        assert_eq!(found.lock().unwrap().len(), 1);

        // 不遵守 .gitignore 时 target 也会被搜到
        q.include.clear();
        q.gitignore = false;
        let found = Mutex::new(Vec::new());
        search_dir(&q, &cancel, Duration::from_millis(10), &|b, _| {
            found.lock().unwrap().extend(b)
        })
        .unwrap();
        assert!(found
            .lock()
            .unwrap()
            .iter()
            .any(|f| f.path.ends_with("d.rs")));
        assert!(!found
            .lock()
            .unwrap()
            .iter()
            .any(|f| f.path.ends_with("c.rs")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_pattern_and_cancel() {
        let d = tmp("bad");
        let q = DirQuery::new(vec![d.clone()], "(", opts(Mode::Regex));
        let cancel = AtomicBool::new(false);
        assert!(search_dir(&q, &cancel, Duration::from_millis(10), &|_, _| {}).is_err());
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let q = DirQuery::new(vec![d.clone()], "x", opts(Mode::Normal));
        let cancel = AtomicBool::new(true);
        let p = search_dir(&q, &cancel, Duration::from_millis(10), &|_, _| {}).unwrap();
        assert_eq!(p.files_matched, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn replace_keeps_encoding() {
        let s = Searcher::new(r"(\d+)", &opts(Mode::Regex)).unwrap();
        let src = "\u{feff}a1 b22\r\n".replace('\u{feff}', "");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(src.as_bytes());
        let (out, n) = replace_in_bytes(&bytes, &s, "<$1>", &DetectHints::default())
            .unwrap()
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(&out[..3], &[0xEF, 0xBB, 0xBF]);
        assert_eq!(&out[3..], "a<1> b<22>\r\n".as_bytes());

        let (gbk, _, _) = encoding::encoding_rs::GBK.encode("简体中文编码识别测试文字中文1");
        let (out, _) = replace_in_bytes(&gbk, &s, "二", &DetectHints::for_codepage(936))
            .unwrap()
            .unwrap();
        assert_eq!(
            encoding::encoding_rs::GBK.decode(&out).0,
            "简体中文编码识别测试文字中文二"
        );
        // 无法用 GBK 表示的字符：拒绝写回
        assert!(replace_in_bytes(&gbk, &s, "😀", &DetectHints::for_codepage(936)).is_err());
        assert_eq!(
            replace_in_bytes(b"abc", &s, "x", &DetectHints::default()).unwrap(),
            None
        );
    }
}
