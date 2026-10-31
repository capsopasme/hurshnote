//! 文档模型与文件加载。
//!
//! 加载在调用线程完成（小文件）或后台线程完成（大文件）：读盘 → 嗅探编码 → 解码 →
//! 填入 Scintilla 的 ILoader。之后回到 UI 线程把 ILoader 转为文档挂到视图上。

use std::path::{Path, PathBuf};

use std::os::windows::fs::MetadataExt;

use hn_core::session::SessionDoc;
use hn_core::stamp::DiskStamp;
use hn_sci::Loader;
use hn_text::encoding::{self, DetectHints, EncodingInfo, TextEncoding};
use hn_text::eol::{self, Eol};

/// 超过此大小的文件暂不打开（大文件视图在 M4 实现）。
pub const MAX_OPEN_SIZE: u64 = 256 * 1024 * 1024;
/// 超过此大小在后台线程加载。
pub const ASYNC_LOAD_SIZE: u64 = 4 * 1024 * 1024;
/// 最长行超过此字节数时进入"长行模式"：关闭高亮、自动换行、缩进线。
pub const LONG_LINE_LIMIT: usize = 1024 * 1024;

/// 切换文档时保存的视图状态。
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewState {
    pub anchor: usize,
    pub caret: usize,
    pub first_line: usize,
    pub x_offset: isize,
}

#[derive(Debug)]
pub struct Doc {
    pub id: u32,
    /// Scintilla 文档指针（本结构持有一个引用）。
    pub ptr: isize,
    pub path: Option<PathBuf>,
    /// 未命名文档的序号（"新建 3"）。
    pub untitled: u32,
    pub enc: EncodingInfo,
    /// 解码时有无法识别的字节。
    pub lossy: bool,
    pub binary: bool,
    /// 编码等元数据变化但文本未变时也需要保存。
    pub meta_dirty: bool,
    /// 文本已修改（非当前文档时的缓存；当前文档以 Scintilla 为准）。
    pub modified: bool,
    /// 在 `hn_text::lang::LANGS` 中的下标。
    pub lang: usize,
    pub long_lines: bool,
    pub view: ViewState,
    /// 当前快照文件名（有未保存内容时）
    pub snapshot: Option<String>,
    /// 自上次快照以来内容有变化
    pub snap_stale: bool,
    /// 载入/保存时的磁盘状态
    pub disk: Option<DiskStamp>,
    /// 加密文档的密钥（内存中，释放时清零）
    pub key: Option<hn_crypto::Key>,
    /// 磁盘上已被删除
    pub missing: bool,
    /// 会话恢复时尚未载入（懒加载）的文档状态；载入后为 None
    pub pending: Option<SessionDoc>,
    /// 刚载入、挂到视图后需要完成的初始化
    pub init: Option<DocInit>,
    /// 非当前文档的书签行与折叠行（切换时记录）
    pub bookmarks: Vec<usize>,
    pub folds: Vec<usize>,
    /// 非当前文档的换行符与只读状态（切换时记录）
    pub eol: Eol,
    pub read_only: bool,
}

/// 文档挂到视图后需要设置的属性（这些属性只能对当前文档设置）。
#[derive(Debug, Clone, Default)]
pub struct DocInit {
    pub eol: Option<Eol>,
    pub read_only: bool,
    pub bookmarks: Vec<usize>,
    pub folds: Vec<usize>,
    pub view: Option<ViewState>,
}

impl Doc {
    /// 新的空文档条目。
    pub fn new(id: u32, ptr: isize) -> Doc {
        Doc {
            id,
            ptr,
            path: None,
            untitled: 0,
            enc: EncodingInfo::UTF8,
            lossy: false,
            binary: false,
            meta_dirty: false,
            modified: false,
            lang: 0,
            long_lines: false,
            view: ViewState::default(),
            snapshot: None,
            snap_stale: false,
            disk: None,
            key: None,
            missing: false,
            pending: None,
            init: None,
            bookmarks: Vec::new(),
            folds: Vec::new(),
            eol: Eol::Crlf,
            read_only: false,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        self.key.is_some()
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
            None => format!("新建 {}", self.untitled),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.modified || self.meta_dirty
    }
}

/// 一次加载请求。
#[derive(Debug, Clone)]
pub struct LoadJob {
    pub path: PathBuf,
    /// 指定编码（"重新打开为"），否则自动嗅探。
    pub forced: Option<TextEncoding>,
    /// 重新载入到已有文档（文档 id）。
    pub reuse: Option<u32>,
    /// 打开后跳到 (行, 列)，均为 1 起始。
    pub goto: Option<(usize, Option<usize>)>,
    pub read_only: bool,
}

pub struct LoadedText {
    pub loader: Loader,
    pub info: EncodingInfo,
    pub lossy: bool,
    pub binary: bool,
    pub eol: Eol,
    pub mixed_eol: bool,
    pub long_lines: bool,
    /// 文件开头一段文本，用于识别语言。
    pub head: String,
    pub disk: DiskStamp,
}

pub struct Loaded {
    pub job: LoadJob,
    pub result: Result<LoadedText, String>,
}

/// 执行加载（可在后台线程调用）。
pub fn load(job: LoadJob, mut loader: Loader, hints: DetectHints) -> Loaded {
    let result = (|| {
        let bytes = std::fs::read(&job.path).map_err(|e| e.to_string())?;
        let mtime = std::fs::metadata(&job.path)
            .map(|m| m.last_write_time())
            .unwrap_or(0);
        let disk = DiskStamp::new(mtime, &bytes);
        let dec = match job.forced {
            Some(enc) => encoding::decode_as(&bytes, enc),
            None => encoding::decode_auto(&bytes, &hints),
        };
        drop(bytes);
        let text = dec.text.as_bytes();
        let stats = eol::EolStats::scan(text);
        let long_lines = encoding::max_line_len(text) > LONG_LINE_LIMIT;
        let head_end = floor_char_boundary(&dec.text, 1024);
        let head = dec.text[..head_end].to_string();
        if !loader.add(text) {
            return Err("内存不足，无法载入文件".to_string());
        }
        Ok(LoadedText {
            loader,
            info: dec.info,
            lossy: dec.lossy,
            binary: dec.binary,
            eol: stats.dominant().unwrap_or(Eol::Crlf),
            mixed_eol: stats.is_mixed(),
            long_lines,
            head,
            disk,
        })
    })();
    Loaded { job, result }
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 文件大小检查。返回错误说明。
pub fn check_size(path: &Path) -> Result<u64, String> {
    let m = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if m.is_dir() {
        return Err("这是一个文件夹".into());
    }
    if m.len() > MAX_OPEN_SIZE {
        return Err("文件超过 256 MB，大文件视图尚未实现".into());
    }
    Ok(m.len())
}

/// 文件是否为 HurshNote 加密文件（只读前 8 字节）。
pub fn is_encrypted_file(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 8];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .map(|_| hn_crypto::is_encrypted(&buf))
        .unwrap_or(false)
}

/// 当前磁盘状态：读取全部内容计算哈希。
pub fn disk_stamp(path: &Path) -> Option<DiskStamp> {
    let bytes = std::fs::read(path).ok()?;
    let mtime = std::fs::metadata(path).ok()?.last_write_time();
    Some(DiskStamp::new(mtime, &bytes))
}

pub fn eol_from_str(s: &str) -> Option<Eol> {
    match s {
        "crlf" => Some(Eol::Crlf),
        "lf" => Some(Eol::Lf),
        "cr" => Some(Eol::Cr),
        _ => None,
    }
}

pub fn eol_to_str(e: Eol) -> &'static str {
    match e {
        Eol::Crlf => "crlf",
        Eol::Lf => "lf",
        Eol::Cr => "cr",
    }
}
