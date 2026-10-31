//! 文档模型与文件加载。
//!
//! 加载在调用线程完成（小文件）或后台线程完成（大文件）：读盘 → 嗅探编码 → 解码 →
//! 填入 Scintilla 的 ILoader。之后回到 UI 线程把 ILoader 转为文档挂到视图上。

use std::path::{Path, PathBuf};

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
}

impl Doc {
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
}

pub struct Loaded {
    pub job: LoadJob,
    pub result: Result<LoadedText, String>,
}

/// 执行加载（可在后台线程调用）。
pub fn load(job: LoadJob, mut loader: Loader, hints: DetectHints) -> Loaded {
    let result = (|| {
        let bytes = std::fs::read(&job.path).map_err(|e| e.to_string())?;
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
