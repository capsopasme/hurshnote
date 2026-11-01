//! 大文件视图与 Hex 视图：自绘子窗口（Direct2D / DirectWrite），只读。
//!
//! 状态（[`LargeDoc`]）属于文档，用 `Rc<RefCell<..>>` 共享；窗口（[`LargeView`]）属于编辑视图：
//! 每个编辑视图一个，叠放在 Scintilla 视图的位置上，显示该视图当前文档的大文件状态。
//! 同一个 `LargeDoc` 也用来在普通文档上临时显示 Hex 视图（`embedded`，数据是编码后的内存快照）。
//!
//! 借用规则：持有 `LargeDoc` 的借用期间不调用可能同步重入窗口过程的 Win32 函数
//! （SetScrollInfo、SetFocus、ShowWindow……），先计算、释放借用再调用。

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use hn_large::decode::{self, RowText};
use hn_large::hex;
use hn_large::rows::Nav;
use hn_large::tail::FileState;
use hn_large::{ByteSource, Charset, FileSource, LineIndex, MemSource, Reader};
use hn_render::{Font, Surface, TextLayout};
use hn_sci::theme::GRUVBOX_LIGHT;
use hn_text::encoding::{self, encoding_rs, DetectHints, Detection, EncodingInfo, TextEncoding};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows_sys::Win32::UI::Controls::SetScrollInfo;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetScrollInfo, GetWindowLongPtrW,
    IsWindowVisible, LoadCursorW, RegisterClassExW, SetWindowLongPtrW, ShowWindow, SCROLLINFO,
    WNDCLASSEXW,
};

use crate::win::*;

/// 打开时用于嗅探编码的字节数。
pub const SNIFF: usize = 256 * 1024;
/// 一次最多复制的字节数。
pub const COPY_LIMIT: u64 = 64 << 20;
const SCROLL_MAX: i32 = 65535;
const CLASS: &str = "HurshNoteLargeView";
const GWLP_USERDATA: i32 = -21;
const CS_DBLCLKS: u32 = 0x0008;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_HSCROLL: u32 = 0x0010_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const IDC_IBEAM: u16 = 32513;

const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_HSCROLL: u32 = 0x0114;
const WM_VSCROLL: u32 = 0x0115;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_CAPTURECHANGED: u32 = 0x0215;

const SB_HORZ: i32 = 0;
const SB_VERT: i32 = 1;
const SB_LINEUP: u16 = 0;
const SB_LINEDOWN: u16 = 1;
const SB_PAGEUP: u16 = 2;
const SB_PAGEDOWN: u16 = 3;
const SB_THUMBPOSITION: u16 = 4;
const SB_THUMBTRACK: u16 = 5;
const SB_TOP: u16 = 6;
const SB_BOTTOM: u16 = 7;
const SIF_RANGE: u32 = 0x1;
const SIF_PAGE: u32 = 0x2;
const SIF_POS: u32 = 0x4;
const SIF_DISABLENOSCROLL: u32 = 0x8;
const SIF_TRACKPOS: u32 = 0x10;

const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_ESCAPE: usize = 0x1B;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_END: usize = 0x23;
const VK_HOME: usize = 0x24;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const VK_INSERT: usize = 0x2D;

fn key_down(vk: i32) -> bool {
    let s = unsafe { GetKeyState(vk) };
    s < 0
}

/// 视图模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Text,
    Hex,
}

/// 编码 → 编码族。
pub fn charset_for(enc: TextEncoding) -> Charset {
    match enc {
        TextEncoding::Utf8 => Charset::Utf8,
        TextEncoding::Utf16Le => Charset::Utf16Le,
        TextEncoding::Utf16Be => Charset::Utf16Be,
        TextEncoding::Legacy(e) => {
            if e == encoding_rs::GB18030 || e == encoding_rs::GBK {
                Charset::Gb18030
            } else if e == encoding_rs::BIG5 || e == encoding_rs::EUC_KR {
                Charset::Dbcs
            } else if e == encoding_rs::SHIFT_JIS {
                Charset::ShiftJis
            } else if e == encoding_rs::EUC_JP {
                Charset::EucJp
            } else {
                Charset::SingleByte
            }
        }
    }
}

/// 文件开头的 BOM 长度（必须与编码一致）。
fn bom_len(head: &[u8], enc: TextEncoding) -> u64 {
    match enc {
        TextEncoding::Utf8 if head.starts_with(&[0xEF, 0xBB, 0xBF]) => 3,
        TextEncoding::Utf16Le if head.starts_with(&[0xFF, 0xFE]) => 2,
        TextEncoding::Utf16Be if head.starts_with(&[0xFE, 0xFF]) => 2,
        _ => 0,
    }
}

/// 嗅探编码：只看开头一段（截在最后一个换行之后，避免切断多字节字符）。
pub fn sniff(head: &[u8], hints: &DetectHints) -> (TextEncoding, bool) {
    if head.is_empty() {
        return (TextEncoding::Utf8, false);
    }
    let cut = head
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .filter(|&i| i > head.len() / 2)
        .unwrap_or(head.len());
    let d = encoding::detect(&head[..cut], hints);
    (d.info.encoding, d.how == Detection::Binary)
}

/// 开头一段中占多数的换行符（状态栏显示）。
fn eol_label(head: &[u8], cs: Charset) -> &'static str {
    let unit = cs.unit() as usize;
    let (mut crlf, mut lf) = (0usize, 0usize);
    let mut i = 0;
    while i + unit <= head.len() {
        if cs.is_lf(&head[i..]) {
            if i >= unit && cs.is_cr(&head[i - unit..]) {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
        i += unit;
    }
    match (crlf, lf) {
        (0, 0) => "—",
        (c, l) if c > 0 && l > 0 => {
            if c >= l {
                "Windows (CRLF)*"
            } else {
                "Unix (LF)*"
            }
        }
        (c, _) if c > 0 => "Windows (CRLF)",
        _ => "Unix (LF)",
    }
}

/// 大文件（或临时 Hex 视图）的状态。
pub struct LargeDoc {
    pub path: Option<PathBuf>,
    pub reader: Reader,
    pub file: Option<Arc<FileSource>>,
    pub enc: EncodingInfo,
    pub cs: Charset,
    pub data_start: u64,
    pub binary: bool,
    pub eol: &'static str,
    pub index: LineIndex,
    /// 每次重置（文件被替换、改编码）加一，用于丢弃过期的后台结果
    pub gen: u32,
    /// 当前行索引扫描的取消标志
    pub cancel: Arc<AtomicBool>,
    pub scanning: bool,
    pub mode: Mode,
    /// 文本模式：顶部显示行的起点
    pub top: u64,
    /// Hex 模式：顶部行号
    pub hex_top: u64,
    /// 横向滚动（像素）
    pub x_off: f32,
    pub caret: u64,
    pub anchor: u64,
    /// 上下移动时保持的横坐标
    pub want_x: Option<f32>,
    /// Tail：跟随文件末尾
    pub tail: bool,
    pub state: Option<FileState>,
    /// 普通文档上的临时 Hex 视图（内存快照）
    pub embedded: bool,
    pub missing: bool,
    /// 已提示过"文件正在增长"
    pub grown_hint: bool,
    pub search_gen: u32,
    pub search_cancel: Option<Arc<AtomicBool>>,
    /// 搜索进度（0..1）
    pub search_progress: Option<f32>,
}

impl std::fmt::Debug for LargeDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LargeDoc")
            .field("path", &self.path)
            .field("len", &self.len())
            .field("mode", &self.mode)
            .field("tail", &self.tail)
            .finish()
    }
}

impl Drop for LargeDoc {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(c) = &self.search_cancel {
            c.store(true, Ordering::Relaxed);
        }
    }
}

impl LargeDoc {
    /// 打开磁盘文件。`forced` 指定编码，否则嗅探。
    pub fn open(
        path: &Path,
        forced: Option<TextEncoding>,
        hints: &DetectHints,
    ) -> std::io::Result<LargeDoc> {
        let file = Arc::new(FileSource::open(path)?);
        let mut head = vec![0u8; SNIFF];
        let n = file.read_full(0, &mut head)?;
        head.truncate(n);
        let (enc, binary) = match forced {
            Some(e) => (e, false),
            None => sniff(&head, hints),
        };
        let src: Arc<dyn ByteSource> = file.clone();
        let mut d = LargeDoc::build(src, Some(file), Some(path.to_path_buf()), &head, enc);
        d.binary = binary;
        d.state = FileState::read(path).ok();
        if binary {
            d.mode = Mode::Hex;
        }
        Ok(d)
    }

    /// 内存中的字节（普通文档按当前编码编码后的快照），以 Hex 模式显示。
    pub fn from_bytes(bytes: Vec<u8>, enc: EncodingInfo) -> LargeDoc {
        let head = bytes[..bytes.len().min(SNIFF)].to_vec();
        let src: Arc<dyn ByteSource> = Arc::new(MemSource::new(bytes));
        let mut d = LargeDoc::build(src, None, None, &head, enc.encoding);
        d.embedded = true;
        d.mode = Mode::Hex;
        d.caret = 0;
        d.anchor = 0;
        d
    }

    fn build(
        src: Arc<dyn ByteSource>,
        file: Option<Arc<FileSource>>,
        path: Option<PathBuf>,
        head: &[u8],
        enc: TextEncoding,
    ) -> LargeDoc {
        let cs = charset_for(enc);
        let ds = bom_len(head, enc);
        LargeDoc {
            path,
            reader: Reader::new(src),
            file,
            enc: EncodingInfo::new(enc, ds > 0),
            cs,
            data_start: ds,
            binary: false,
            eol: eol_label(head, cs),
            index: LineIndex::new(cs, ds),
            gen: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            scanning: false,
            mode: Mode::Text,
            top: ds,
            hex_top: 0,
            x_off: 0.0,
            caret: ds,
            anchor: ds,
            want_x: None,
            tail: false,
            state: None,
            embedded: false,
            missing: false,
            grown_hint: false,
            search_gen: 0,
            search_cancel: None,
            search_progress: None,
        }
    }

    pub fn len(&self) -> u64 {
        self.reader.len()
    }

    pub fn nav(&self) -> Nav<'_> {
        Nav::new(&self.reader, self.cs, self.data_start)
    }

    pub fn source(&self) -> Arc<dyn ByteSource> {
        self.reader.source()
    }

    /// 遗留编码（非 UTF-8/16）对应的 encoding_rs 编码。
    pub fn legacy(&self) -> Option<&'static encoding_rs::Encoding> {
        match self.enc.encoding {
            TextEncoding::Legacy(e) => Some(e),
            _ => None,
        }
    }

    /// 选区（起点, 终点），起点不大于终点。
    pub fn sel(&self) -> (u64, u64) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// 行索引进度（0..1），已完成时为 None。
    pub fn index_progress(&self) -> Option<f32> {
        if self.index.is_done() && !self.scanning {
            return None;
        }
        let len = self.len().max(1);
        Some((self.index.scanned() as f64 / len as f64).min(1.0) as f32)
    }

    /// 光标所在行号（0 起始）；索引尚未扫描到时为 None。
    pub fn caret_line(&self) -> Option<u64> {
        self.index.line_of(self.caret, &self.reader)
    }

    /// 把偏移对齐到字符边界（文本模式）。
    pub fn snap(&self, off: u64) -> u64 {
        if self.mode == Mode::Hex {
            return off.min(self.len());
        }
        let nav = self.nav();
        let off = nav.clamp(off);
        let rs = nav.row_start(off);
        let bytes = self.reader.read(rs, (off - rs) as usize);
        let mut i = 0usize;
        while i < bytes.len() {
            let n = self.cs.char_len(&bytes[i..]).max(1);
            if i + n > bytes.len() {
                break;
            }
            i += n;
        }
        rs + i as u64
    }

    /// 文件被替换、截断、改写或改了编码：清空缓存与索引，保持位置（限制在新长度内）。
    pub fn reset(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.scanning = false;
        self.gen = self.gen.wrapping_add(1);
        self.reader.clear();
        let mut head = vec![0u8; 4096.min(self.len() as usize)];
        let n = self.source().read_full(0, &mut head).unwrap_or(0);
        head.truncate(n);
        self.data_start = bom_len(&head, self.enc.encoding);
        self.enc = EncodingInfo::new(self.enc.encoding, self.data_start > 0);
        self.index = LineIndex::new(self.cs, self.data_start);
        let len = self.len();
        self.caret = self.snap(self.caret.min(len));
        self.anchor = self.snap(self.anchor.min(len));
        self.top = self.nav().row_start(self.top.min(len));
        self.hex_top = self.hex_top.min(hex::row_count(len) - 1);
    }

    /// 改用另一种编码解读（"重新打开为"）。
    pub fn set_encoding(&mut self, enc: TextEncoding) {
        self.enc = EncodingInfo::new(enc, false);
        self.cs = charset_for(enc);
        let mut head = vec![0u8; SNIFF.min(self.len() as usize)];
        let n = self.source().read_full(0, &mut head).unwrap_or(0);
        head.truncate(n);
        self.eol = eol_label(&head, self.cs);
        self.binary = false;
        self.reset();
    }

    /// 让光标所在行可见（`vis` 为窗口能完整显示的行数）。
    pub fn ensure_visible(&mut self, vis: u64) {
        let vis = vis.max(1);
        match self.mode {
            Mode::Hex => {
                let rows = hex::row_count(self.len());
                let cr = (self.caret / hex::ROW).min(rows - 1);
                if cr < self.hex_top {
                    self.hex_top = cr;
                } else if cr >= self.hex_top + vis {
                    self.hex_top = cr + 1 - vis;
                }
            }
            Mode::Text => {
                let new_top = {
                    let nav = self.nav();
                    let cr = nav.row_start(self.caret);
                    if cr < self.top {
                        Some(cr)
                    } else {
                        let mut r = self.top;
                        let mut found = r == cr;
                        for _ in 1..vis {
                            if found {
                                break;
                            }
                            match nav.next_row(r) {
                                Some(n) if n <= cr => {
                                    r = n;
                                    found = r == cr;
                                }
                                _ => break,
                            }
                        }
                        if found {
                            None
                        } else {
                            Some(nav.step(cr, -(vis as i64 - 1)))
                        }
                    }
                };
                if let Some(t) = new_top {
                    self.top = t;
                }
            }
        }
    }

    /// 限制顶部位置：最后一行最多滚到窗口底部。
    pub fn clamp_top(&mut self, vis: u64) {
        let vis = vis.max(1);
        match self.mode {
            Mode::Hex => {
                let rows = hex::row_count(self.len());
                self.hex_top = self.hex_top.min(rows.saturating_sub(vis));
            }
            Mode::Text => {
                let max_top = {
                    let nav = self.nav();
                    nav.step(nav.last_row(), -(vis as i64 - 1))
                };
                if self.top > max_top {
                    self.top = max_top;
                }
                if self.top < self.data_start {
                    self.top = self.data_start;
                }
            }
        }
    }

    /// 滚动 `n` 个显示行。
    pub fn scroll(&mut self, n: i64, vis: u64) {
        match self.mode {
            Mode::Hex => {
                self.hex_top = (self.hex_top as i64 + n).max(0) as u64;
            }
            Mode::Text => {
                self.top = self.nav().step(self.top, n);
            }
        }
        self.clamp_top(vis);
    }

    /// 当前滚动位置（0..1）。
    pub fn fraction(&self, vis: u64) -> f64 {
        match self.mode {
            Mode::Hex => {
                let range = hex::row_count(self.len()).saturating_sub(vis.max(1));
                if range == 0 {
                    0.0
                } else {
                    (self.hex_top as f64 / range as f64).min(1.0)
                }
            }
            Mode::Text => {
                let total = self.len().saturating_sub(self.data_start);
                if total == 0 {
                    0.0
                } else {
                    ((self.top - self.data_start) as f64 / total as f64).min(1.0)
                }
            }
        }
    }

    /// 滚动到比例位置（0..1）。
    pub fn scroll_to_fraction(&mut self, f: f64, vis: u64) {
        let f = f.clamp(0.0, 1.0);
        match self.mode {
            Mode::Hex => {
                let range = hex::row_count(self.len()).saturating_sub(vis.max(1));
                self.hex_top = (range as f64 * f).round() as u64;
            }
            Mode::Text => {
                if f >= 0.9999 {
                    self.top = u64::MAX;
                } else {
                    let total = self.len().saturating_sub(self.data_start);
                    let target = self.data_start + (total as f64 * f) as u64;
                    self.top = self.nav().row_start(target);
                }
            }
        }
        self.clamp_top(vis);
    }

    /// 选中 `[a, b)` 并滚动到可见。
    pub fn select(&mut self, a: u64, b: u64, vis: u64) {
        self.anchor = a.min(self.len());
        self.caret = b.min(self.len());
        self.want_x = None;
        self.ensure_visible(vis);
    }

    /// 光标移到末尾并滚动（Tail）。
    pub fn goto_end(&mut self, vis: u64) {
        let len = self.len();
        self.caret = len;
        self.anchor = len;
        match self.mode {
            Mode::Hex => self.hex_top = u64::MAX,
            Mode::Text => self.top = u64::MAX,
        }
        self.clamp_top(vis);
    }

    /// 选中内容：文本模式按当前编码解码，Hex 模式为十六进制字节。
    pub fn selected_text(&self) -> Result<String, String> {
        let (a, b) = self.sel();
        if a == b {
            return Err("没有选中内容".into());
        }
        if b - a > COPY_LIMIT {
            return Err(format!(
                "选中了 {:.1} MB，超过复制上限（{} MB）",
                (b - a) as f64 / 1048576.0,
                COPY_LIMIT >> 20
            ));
        }
        let mut buf = vec![0u8; (b - a) as usize];
        let n = self
            .source()
            .read_full(a, &mut buf)
            .map_err(|e| format!("读取失败：{e}"))?;
        buf.truncate(n);
        Ok(match self.mode {
            Mode::Hex => buf
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" "),
            Mode::Text => encoding::decode_as(&buf, self.enc.encoding).text,
        })
    }
}

// ---- 视图窗口 ----

struct Painted {
    start: u64,
    content_end: u64,
    next: Option<u64>,
    text: RowText,
    layout: Option<TextLayout>,
}

/// 一个编辑视图上的大文件视图窗口。
pub struct LargeView {
    pub idx: usize,
    hwnd: Cell<HWND>,
    doc: RefCell<Option<(u32, Rc<RefCell<LargeDoc>>)>>,
    surface: RefCell<Option<Surface>>,
    font: RefCell<Option<Rc<Font>>>,
    font_key: RefCell<(String, u32)>,
    painted: RefCell<Vec<Painted>>,
    vis: Cell<u64>,
    selecting: Cell<bool>,
    max_w: Cell<f32>,
    text_x: Cell<f32>,
    pad: Cell<f32>,
    show_numbers: Cell<bool>,
}

thread_local! {
    static VIEWS: RefCell<Vec<Rc<LargeView>>> = const { RefCell::new(Vec::new()) };
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
    /// 首次创建字体、渲染目标的耗时（毫秒），供性能测试输出
    static TIMINGS: RefCell<Vec<(&'static str, f64)>> = const { RefCell::new(Vec::new()) };
}

pub fn record(name: &'static str, start: std::time::Instant) {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    TIMINGS.with(|t| {
        let mut t = t.borrow_mut();
        if !t.iter().any(|(k, _)| *k == name) {
            t.push((name, ms));
        }
    });
}

/// 自测：各项首次初始化耗时。
pub fn st_timings() -> Vec<(&'static str, f64)> {
    TIMINGS.with(|t| t.borrow().clone())
}

/// 第 `idx` 个编辑视图上的大文件视图。
pub fn view(idx: usize) -> Option<Rc<LargeView>> {
    VIEWS.with(|v| v.borrow().get(idx).cloned())
}

/// 正在显示 `doc` 的视图窗口。
pub fn views_showing(doc: &Rc<RefCell<LargeDoc>>) -> Vec<Rc<LargeView>> {
    VIEWS.with(|v| {
        v.borrow()
            .iter()
            .filter(|lv| lv.is_shown() && lv.doc().is_some_and(|d| Rc::ptr_eq(&d, doc)))
            .cloned()
            .collect()
    })
}

/// 文档状态变化后：滚动到光标、重绘所有显示它的视图。
pub fn reveal(doc: &Rc<RefCell<LargeDoc>>) {
    let views = views_showing(doc);
    let vis = views.first().map(|v| v.vis.get()).unwrap_or(30);
    doc.borrow_mut().ensure_visible(vis);
    for v in views {
        v.ensure_caret_x();
        v.refresh();
    }
}

/// 重绘所有显示 `doc` 的视图（不滚动）。
pub fn repaint(doc: &Rc<RefCell<LargeDoc>>) {
    for v in views_showing(doc) {
        v.refresh();
    }
}

/// 创建第 `idx` 个视图窗口（隐藏）。
pub fn create(parent: HWND, hinstance: *mut core::ffi::c_void, idx: usize) -> HWND {
    let class = wide(CLASS);
    if !REGISTERED.with(|r| r.replace(true)) {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: null_mut(),
            hCursor: unsafe { LoadCursorW(null_mut(), IDC_IBEAM as usize as _) },
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: null_mut(),
        };
        unsafe { RegisterClassExW(&wc) };
    }
    let lv = Rc::new(LargeView {
        idx,
        hwnd: Cell::new(null_mut()),
        doc: RefCell::new(None),
        surface: RefCell::new(None),
        font: RefCell::new(None),
        font_key: RefCell::new((String::new(), 0)),
        painted: RefCell::new(Vec::new()),
        vis: Cell::new(30),
        selecting: Cell::new(false),
        max_w: Cell::new(0.0),
        text_x: Cell::new(0.0),
        pad: Cell::new(4.0),
        show_numbers: Cell::new(true),
    });
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        if v.len() <= idx {
            v.resize(idx + 1, lv.clone());
        }
        v[idx] = lv.clone();
    });
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            null(),
            WS_CHILD | WS_VSCROLL | WS_HSCROLL | WS_CLIPSIBLINGS | WS_TABSTOP,
            0,
            0,
            0,
            0,
            parent,
            (200 + idx) as *mut core::ffi::c_void,
            hinstance,
            null(),
        )
    };
    lv.hwnd.set(hwnd);
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, idx as isize + 1) };
    hwnd
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let idx = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if idx > 0 {
        let r = crate::crash::guard(|| view(idx as usize - 1).and_then(|v| v.handle(msg, wp, lp)));
        if let Some(Some(r)) = r {
            return r;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn client(hwnd: HWND) -> (f32, f32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    ((rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32)
}

fn digits(n: u64) -> usize {
    n.max(1).ilog10() as usize + 1
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// 遗留编码的单字符解码函数。
fn legacy_fn(e: Option<&'static encoding_rs::Encoding>) -> impl Fn(&[u8]) -> String {
    move |b: &[u8]| match e {
        Some(e) => e.decode_without_bom_handling(b).0.into_owned(),
        None => "\u{FFFD}".to_string(),
    }
}

impl LargeView {
    pub fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    pub fn is_shown(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd.get()) != 0 }
    }

    pub fn doc(&self) -> Option<Rc<RefCell<LargeDoc>>> {
        self.doc.borrow().as_ref().map(|(_, d)| d.clone())
    }

    pub fn doc_id(&self) -> Option<u32> {
        self.doc.borrow().as_ref().map(|(id, _)| *id)
    }

    /// 能完整显示的行数。
    pub fn visible_rows(&self) -> u64 {
        self.vis.get()
    }

    /// 显示文档 `id` 的状态。
    pub fn show(&self, id: u32, d: Rc<RefCell<LargeDoc>>) {
        let same = self
            .doc
            .borrow()
            .as_ref()
            .is_some_and(|(i, x)| *i == id && Rc::ptr_eq(x, &d));
        if !same {
            *self.doc.borrow_mut() = Some((id, d));
            self.painted.borrow_mut().clear();
        }
        if !self.is_shown() {
            unsafe { ShowWindow(self.hwnd.get(), SW_SHOW) };
        }
        self.update_vis();
        self.refresh();
    }

    pub fn hide(&self) {
        if self.is_shown() {
            unsafe { ShowWindow(self.hwnd.get(), SW_HIDE) };
        }
        self.doc.borrow_mut().take();
        self.painted.borrow_mut().clear();
        self.selecting.set(false);
    }

    /// 设置字体（像素字号）与是否显示行号。
    pub fn set_font(&self, face: &str, px: f32, numbers: bool) {
        let key = (face.to_string(), (px * 100.0) as u32);
        let changed = *self.font_key.borrow() != key || self.show_numbers.get() != numbers;
        if changed {
            *self.font_key.borrow_mut() = key;
            self.font.borrow_mut().take();
            self.show_numbers.set(numbers);
            self.update_vis();
            if self.is_shown() {
                self.refresh();
            }
        }
    }

    fn font(&self) -> Option<Rc<Font>> {
        if let Some(f) = self.font.borrow().as_ref() {
            return Some(f.clone());
        }
        let (face, px100) = self.font_key.borrow().clone();
        let px = if px100 == 0 {
            15.0
        } else {
            px100 as f32 / 100.0
        };
        let face = if face.is_empty() {
            "Consolas".to_string()
        } else {
            face
        };
        let t = std::time::Instant::now();
        let f = Rc::new(Font::new(&face, px, 4).ok()?);
        record("font_init_ms", t);
        *self.font.borrow_mut() = Some(f.clone());
        Some(f)
    }

    fn update_vis(&self) {
        let (_, h) = client(self.hwnd.get());
        if let Some(f) = self.font() {
            self.vis.set(((h / f.line_h).floor() as u64).max(1));
        }
    }

    /// 重绘并更新滚动条。
    pub fn refresh(&self) {
        unsafe { InvalidateRect(self.hwnd.get(), null(), 0) };
        self.update_scrollbars();
    }

    fn handle(&self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_PAINT => {
                self.paint();
                Some(0)
            }
            WM_ERASEBKGND => Some(1),
            WM_SIZE => {
                let (w, h) = (loword(lp as usize) as u32, hiword(lp as usize) as u32);
                if let Some(s) = self.surface.borrow().as_ref() {
                    s.resize(w, h);
                }
                self.update_vis();
                if let Some(d) = self.doc() {
                    let vis = self.vis.get();
                    d.borrow_mut().clamp_top(vis);
                }
                self.refresh();
                Some(0)
            }
            WM_VSCROLL => {
                self.on_vscroll(loword(wp));
                Some(0)
            }
            WM_HSCROLL => {
                self.on_hscroll(loword(wp));
                Some(0)
            }
            WM_MOUSEWHEEL => {
                let delta = hiword(wp) as i16 as i32;
                if key_down(VK_CONTROL) {
                    if let Some(app) = crate::app::app() {
                        app.lv_ctrl_wheel(delta);
                    }
                } else if let Some(d) = self.doc() {
                    let vis = self.vis.get();
                    d.borrow_mut()
                        .scroll(-(delta as i64) * 3 / WHEEL_DELTA as i64, vis);
                    self.refresh();
                }
                Some(0)
            }
            WM_MOUSEHWHEEL => {
                let delta = hiword(wp) as i16 as i32;
                self.hscroll_by(delta as f32);
                Some(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_from_lparam(lp);
                unsafe {
                    SetFocus(self.hwnd.get());
                    SetCapture(self.hwnd.get());
                }
                self.selecting.set(true);
                self.click(x as f32, y as f32, key_down(VK_SHIFT));
                Some(0)
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = point_from_lparam(lp);
                self.select_word(x as f32, y as f32);
                Some(0)
            }
            WM_MOUSEMOVE => {
                if self.selecting.get() {
                    let (x, y) = point_from_lparam(lp);
                    self.drag(x as f32, y as f32);
                }
                Some(0)
            }
            WM_LBUTTONUP => {
                if self.selecting.replace(false) {
                    unsafe { ReleaseCapture() };
                }
                Some(0)
            }
            WM_CAPTURECHANGED => {
                self.selecting.set(false);
                Some(0)
            }
            WM_KEYDOWN => self.on_key(wp).then_some(0),
            WM_CHAR => Some(0),
            WM_SETFOCUS => {
                if let Some(app) = crate::app::app() {
                    app.lv_focus(self.idx);
                }
                self.refresh();
                Some(0)
            }
            WM_KILLFOCUS => {
                self.refresh();
                Some(0)
            }
            _ => None,
        }
    }

    // ---- 绘制 ----

    fn paint(&self) {
        let hwnd = self.hwnd.get();
        let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
        unsafe { BeginPaint(hwnd, &mut ps) };
        self.render();
        unsafe { EndPaint(hwnd, &ps) };
    }

    fn render(&self) -> Option<()> {
        let hwnd = self.hwnd.get();
        let (w, h) = client(hwnd);
        let doc = self.doc()?;
        let font = self.font()?;
        if self.surface.borrow().is_none() {
            let t = std::time::Instant::now();
            let s = Surface::new(hwnd, w as u32, h as u32).ok()?;
            record("surface_init_ms", t);
            *self.surface.borrow_mut() = Some(s);
        }
        let surf = self.surface.borrow();
        let s = surf.as_ref()?;
        s.begin();
        {
            let d = doc.borrow();
            let focused = unsafe { GetFocus() } == hwnd;
            match d.mode {
                Mode::Text => self.render_text(s, &d, &font, w, h, focused),
                Mode::Hex => self.render_hex(s, &d, &font, w, h, focused),
            }
        }
        let t = std::time::Instant::now();
        let res = s.end();
        record("first_end_draw_ms", t);
        drop(surf);
        if let Err(e) = res {
            if hn_render::needs_recreate(&e) {
                self.surface.borrow_mut().take();
                unsafe { InvalidateRect(hwnd, null(), 0) };
            }
        }
        Some(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_text(&self, s: &Surface, d: &LargeDoc, font: &Font, w: f32, h: f32, focused: bool) {
        let p = &GRUVBOX_LIGHT;
        s.clear(p.bg0);
        let lh = font.line_h;
        let cw = font.char_w;
        let pad = (cw * 0.8).round().max(2.0);
        self.pad.set(pad);
        let nav = d.nav();
        let rows = (h / lh).ceil() as usize + 1;
        let numbers = self.show_numbers.get();
        let gutter = if numbers {
            (digits(d.index.line_count()).max(4) as f32 * cw + 2.0 * pad).round()
        } else {
            0.0
        };
        let tx = gutter + pad;
        self.text_x.set(tx);
        if gutter > 0.0 {
            s.fill(0.0, 0.0, gutter, h, p.bg0_s);
        }
        let caret_w = (lh / 12.0).round().max(1.0);
        let (sa, sb) = d.sel();
        let caret_row = nav.row_start(d.caret);
        let mut line = d.index.line_of(d.top, &d.reader);
        let dec = legacy_fn(d.legacy());
        let legacy: Option<decode::LegacyDecode> = if d.legacy().is_some() {
            Some(&dec)
        } else {
            None
        };
        let mut painted = Vec::with_capacity(rows);
        let mut max_w = 0f32;
        let x = tx - d.x_off;
        let mut r = d.top;
        for i in 0..rows {
            let row = nav.row(r);
            if i > 0 && row.line_start {
                line = line.map(|l| l + 1);
            }
            let y = i as f32 * lh;
            let bytes = d
                .reader
                .read(row.start, (row.content_end - row.start) as usize);
            let text = decode::decode_row(&bytes, d.cs, legacy);
            let layout = TextLayout::new(&text.text, font).ok();
            let tw = layout.as_ref().map_or(0.0, |l| l.width);
            max_w = max_w.max(tw);
            let xa = |pos: u64| -> f32 {
                let u = text.unit_of((pos - row.start) as u32);
                layout.as_ref().map_or(0.0, |l| l.x_at(u))
            };

            s.clip(gutter, 0.0, w, h);
            if row.start == caret_row {
                s.fill(gutter, y, w, y + lh, p.bg1);
            }
            let row_end = row.next.unwrap_or(row.content_end);
            if sa < sb && sb > row.start && sa < row_end.max(row.start + 1) {
                let a = sa.max(row.start).min(row.content_end);
                let b = sb.min(row.content_end);
                let mut x1 = x + xa(b);
                if sb > row.content_end && row.next.is_some() {
                    x1 += cw * 0.6;
                }
                s.fill(x + xa(a), y, x1, y + lh, p.bg2);
            }
            if let Some(l) = &layout {
                s.layout(l, x, y, p.fg1);
            }
            if focused
                && row.start == caret_row
                && d.caret >= row.start
                && d.caret <= row.content_end
            {
                let cx = (x + xa(d.caret)).round();
                s.fill(cx, y, cx + caret_w, y + lh, p.fg1);
            }
            s.unclip();

            if numbers && row.line_start {
                if let Some(l) = line {
                    let t = utf16(&(l + 1).to_string());
                    let tw = t.len() as f32 * cw;
                    s.text(&t, font, gutter - pad - tw, y, gutter, y + lh, p.gray);
                }
            }
            painted.push(Painted {
                start: row.start,
                content_end: row.content_end,
                next: row.next,
                text,
                layout,
            });
            match row.next {
                Some(n) => r = n,
                None => break,
            }
        }
        *self.painted.borrow_mut() = painted;
        self.max_w.set(max_w + tx + cw * 4.0);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_hex(&self, s: &Surface, d: &LargeDoc, font: &Font, w: f32, h: f32, focused: bool) {
        let p = &GRUVBOX_LIGHT;
        s.clear(p.bg0);
        let lh = font.line_h;
        let cw = font.char_w;
        let pad = (cw * 0.8).round().max(2.0);
        self.pad.set(pad);
        self.painted.borrow_mut().clear();
        let len = d.len();
        let lay = hex::Layout::for_len(len);
        let total = hex::row_count(len);
        let rows = (h / lh).ceil() as u64 + 1;
        let x0 = pad - d.x_off;
        let (sa, sb) = d.sel();
        let caret_row = (d.caret / hex::ROW).min(total - 1);
        let tc = lay.text_col() as f32;
        for i in 0..rows {
            let row = d.hex_top + i;
            if row >= total {
                break;
            }
            let off = row * hex::ROW;
            let bytes = d.reader.read(off, hex::ROW as usize);
            let y = i as f32 * lh;
            if row == caret_row {
                s.fill(0.0, y, w, y + lh, p.bg1);
            }
            for j in 0..bytes.len() {
                let b = off + j as u64;
                let hx = x0 + lay.hex_col(j) as f32 * cw;
                let txx = x0 + (tc + j as f32) * cw;
                if b >= sa && b < sb {
                    s.fill(hx, y, hx + 2.0 * cw, y + lh, p.bg2);
                    s.fill(txx, y, txx + cw, y + lh, p.bg2);
                }
            }
            if focused && row == caret_row {
                let j = (d.caret - off).min(hex::ROW - 1) as usize;
                let hx = x0 + lay.hex_col(j) as f32 * cw;
                let txx = x0 + (tc + j as f32) * cw;
                let t = (lh / 10.0).round().max(2.0);
                s.fill(hx, y + lh - t, hx + 2.0 * cw, y + lh, p.fg1);
                s.fill(txx, y + lh - t, txx + cw, y + lh, p.fg1);
            }
            let line = utf16(&lay.format(off, &bytes));
            let a = lay.addr.min(line.len());
            let t = lay.text_col().min(line.len());
            s.text(
                &line[..a],
                font,
                x0,
                y,
                x0 + (a as f32 + 1.0) * cw,
                y + lh,
                p.blue,
            );
            s.text(
                &line[a..t],
                font,
                x0 + a as f32 * cw,
                y,
                w.max(x0 + t as f32 * cw),
                y + lh,
                p.fg1,
            );
            s.text(
                &line[t..],
                font,
                x0 + tc * cw,
                y,
                w.max(x0 + (tc + 17.0) * cw),
                y + lh,
                p.aqua,
            );
        }
        self.max_w.set((lay.width() as f32 + 2.0) * cw + pad);
    }

    // ---- 滚动条 ----

    fn update_scrollbars(&self) {
        let hwnd = self.hwnd.get();
        let Some(doc) = self.doc() else { return };
        let vis = self.vis.get();
        let (page, pos, hmax, hpos) = {
            let d = doc.borrow();
            let page = match d.mode {
                Mode::Hex => {
                    let total = hex::row_count(d.len());
                    (vis as f64 / total as f64).min(1.0)
                }
                Mode::Text => {
                    let total = d.len().saturating_sub(d.data_start).max(1);
                    let span = self
                        .painted
                        .borrow()
                        .last()
                        .map(|p| p.next.unwrap_or(d.len()))
                        .unwrap_or(d.len())
                        .saturating_sub(d.top);
                    (span as f64 / total as f64).min(1.0)
                }
            };
            let page = ((page * (SCROLL_MAX as f64 + 1.0)) as u32).max(1);
            let range = (SCROLL_MAX as f64 + 1.0 - page as f64).max(0.0);
            let pos = (d.fraction(vis) * range).round() as i32;
            (page, pos, self.max_w.get() as i32, d.x_off as i32)
        };
        let (cw, _) = client(hwnd);
        let v = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: SCROLL_MAX,
            nPage: page,
            nPos: pos,
            nTrackPos: 0,
        };
        let hsi = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: hmax.max(0),
            nPage: cw.max(1.0) as u32,
            nPos: hpos,
            nTrackPos: 0,
        };
        unsafe {
            SetScrollInfo(hwnd, SB_VERT, &v, 1);
            SetScrollInfo(hwnd, SB_HORZ, &hsi, 1);
        }
    }

    fn on_vscroll(&self, code: u16) {
        let Some(doc) = self.doc() else { return };
        let vis = self.vis.get();
        let page = vis.saturating_sub(1).max(1) as i64;
        let track = if code == SB_THUMBTRACK || code == SB_THUMBPOSITION {
            let mut si: SCROLLINFO = unsafe { std::mem::zeroed() };
            si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
            si.fMask = SIF_TRACKPOS | SIF_PAGE;
            unsafe { GetScrollInfo(self.hwnd.get(), SB_VERT, &mut si) };
            let range = (SCROLL_MAX as f64 + 1.0 - si.nPage as f64).max(1.0);
            Some(si.nTrackPos as f64 / range)
        } else {
            None
        };
        {
            let mut d = doc.borrow_mut();
            match code {
                SB_LINEUP => d.scroll(-1, vis),
                SB_LINEDOWN => d.scroll(1, vis),
                SB_PAGEUP => d.scroll(-page, vis),
                SB_PAGEDOWN => d.scroll(page, vis),
                SB_TOP => d.scroll_to_fraction(0.0, vis),
                SB_BOTTOM => d.scroll_to_fraction(1.0, vis),
                _ => {
                    if let Some(f) = track {
                        d.scroll_to_fraction(f, vis);
                    }
                }
            }
        }
        self.refresh();
    }

    fn on_hscroll(&self, code: u16) {
        let (w, _) = client(self.hwnd.get());
        let cw = self.font().map_or(8.0, |f| f.char_w);
        match code {
            SB_LINEUP => self.hscroll_by(-cw * 4.0),
            SB_LINEDOWN => self.hscroll_by(cw * 4.0),
            SB_PAGEUP => self.hscroll_by(-w * 0.8),
            SB_PAGEDOWN => self.hscroll_by(w * 0.8),
            SB_TOP => self.hscroll_to(0.0),
            SB_BOTTOM => self.hscroll_to(f32::MAX),
            SB_THUMBTRACK | SB_THUMBPOSITION => {
                let mut si: SCROLLINFO = unsafe { std::mem::zeroed() };
                si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
                si.fMask = SIF_TRACKPOS;
                unsafe { GetScrollInfo(self.hwnd.get(), SB_HORZ, &mut si) };
                self.hscroll_to(si.nTrackPos as f32);
            }
            _ => {}
        }
    }

    fn hscroll_by(&self, dx: f32) {
        let cur = self.doc().map_or(0.0, |d| d.borrow().x_off);
        self.hscroll_to(cur + dx);
    }

    fn hscroll_to(&self, x: f32) {
        let Some(doc) = self.doc() else { return };
        let (w, _) = client(self.hwnd.get());
        let max = (self.max_w.get() - w).max(0.0);
        doc.borrow_mut().x_off = x.clamp(0.0, max).round();
        self.refresh();
    }

    // ---- 鼠标 ----

    /// 窗口坐标 → 文件偏移。
    fn hit(&self, x: f32, y: f32) -> Option<u64> {
        let doc = self.doc()?;
        let font = self.font()?;
        let d = doc.borrow();
        match d.mode {
            Mode::Text => {
                let painted = self.painted.borrow();
                let last = painted.len().checked_sub(1)?;
                let i = ((y / font.line_h).floor().max(0.0) as usize).min(last);
                let pr = &painted[i];
                let rel = x - (self.text_x.get() - d.x_off);
                let u = pr.layout.as_ref().map_or(0, |l| l.hit(rel));
                Some((pr.start + pr.text.byte_of(u) as u64).min(pr.content_end))
            }
            Mode::Hex => {
                let len = d.len();
                let total = hex::row_count(len);
                let row = (d.hex_top + (y / font.line_h).floor().max(0.0) as u64).min(total - 1);
                let col = (x + d.x_off - self.pad.get()) / font.char_w;
                let (j, _) = hex::Layout::for_len(len).byte_at_col(col);
                Some((row * hex::ROW + j as u64).min(len))
            }
        }
    }

    fn click(&self, x: f32, y: f32, shift: bool) {
        let Some(doc) = self.doc() else { return };
        let Some(pos) = self.hit(x, y) else { return };
        {
            let mut d = doc.borrow_mut();
            d.caret = pos;
            if !shift {
                d.anchor = pos;
            }
            d.want_x = None;
        }
        self.changed();
    }

    fn drag(&self, x: f32, y: f32) {
        let Some(doc) = self.doc() else { return };
        let (_, h) = client(self.hwnd.get());
        let vis = self.vis.get();
        if y < 0.0 {
            doc.borrow_mut().scroll(-1, vis);
        } else if y > h {
            doc.borrow_mut().scroll(1, vis);
        }
        if y < 0.0 || y > h {
            // 先按新的滚动位置重绘，命中测试才能用上新的行
            self.refresh();
            unsafe { windows_sys::Win32::Graphics::Gdi::UpdateWindow(self.hwnd.get()) };
        }
        let Some(pos) = self.hit(x, y.clamp(0.0, (h - 1.0).max(0.0))) else {
            return;
        };
        {
            let mut d = doc.borrow_mut();
            d.caret = if d.mode == Mode::Hex && pos >= d.anchor {
                (pos + 1).min(d.len())
            } else {
                pos
            };
        }
        self.changed();
    }

    /// 双击：选中光标处的词（文本模式）。
    fn select_word(&self, x: f32, y: f32) {
        let Some(doc) = self.doc() else { return };
        if doc.borrow().mode != Mode::Text {
            return;
        }
        let Some(font) = self.font() else { return };
        let range = {
            let painted = self.painted.borrow();
            let Some(last) = painted.len().checked_sub(1) else {
                return;
            };
            let i = ((y / font.line_h).floor().max(0.0) as usize).min(last);
            let pr = &painted[i];
            let d = doc.borrow();
            let rel = x - (self.text_x.get() - d.x_off);
            let u = pr.layout.as_ref().map_or(0, |l| l.hit(rel));
            let t = &pr.text.text;
            let is_word = |c: u16| {
                char::from_u32(c as u32).is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
                    || (0xD800..=0xDFFF).contains(&c)
            };
            let mut a = u.min(t.len());
            let mut b = a;
            while a > 0 && is_word(t[a - 1]) {
                a -= 1;
            }
            while b < t.len() && is_word(t[b]) {
                b += 1;
            }
            (
                pr.start + pr.text.byte_of(a) as u64,
                pr.start + pr.text.byte_of(b) as u64,
            )
        };
        {
            let mut d = doc.borrow_mut();
            d.anchor = range.0;
            d.caret = range.1;
        }
        self.changed();
    }

    /// 选区或光标变化后：重绘显示同一文档的视图、更新状态栏。
    fn changed(&self) {
        if let Some(doc) = self.doc() {
            repaint(&doc);
        }
        if let Some(app) = crate::app::app() {
            app.lv_changed();
        }
    }

    // ---- 键盘 ----

    fn on_key(&self, vk: usize) -> bool {
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        let Some(doc) = self.doc() else {
            return false;
        };
        if ctrl && (vk == b'C' as usize || vk == VK_INSERT) {
            let text = doc.borrow().selected_text();
            if let Some(app) = crate::app::app() {
                match text {
                    Ok(t) => app.lv_copy(&t),
                    Err(e) => app.lv_message(&e),
                }
            }
            return true;
        }
        if ctrl && vk == b'A' as usize {
            {
                let mut d = doc.borrow_mut();
                d.anchor = if d.mode == Mode::Hex { 0 } else { d.data_start };
                d.caret = d.len();
            }
            self.changed();
            return true;
        }
        if vk == VK_ESCAPE {
            if let Some(app) = crate::app::app() {
                app.lv_escape();
            }
            return true;
        }
        if !matches!(
            vk,
            VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT | VK_HOME | VK_END
        ) {
            return false;
        }
        let vis = self.vis.get();
        let mode = doc.borrow().mode;
        match mode {
            Mode::Hex => self.move_hex(&doc, vk, ctrl, shift, vis),
            Mode::Text => {
                let Some(font) = self.font() else { return true };
                self.move_text(&doc, &font, vk, ctrl, shift, vis);
            }
        }
        doc.borrow_mut().ensure_visible(vis);
        self.ensure_caret_x();
        self.changed();
        self.update_scrollbars();
        true
    }

    fn move_hex(&self, doc: &Rc<RefCell<LargeDoc>>, vk: usize, ctrl: bool, shift: bool, vis: u64) {
        let mut d = doc.borrow_mut();
        let len = d.len();
        let c = d.caret as i64;
        let page = (vis.saturating_sub(1).max(1) * hex::ROW) as i64;
        let row_start = (d.caret / hex::ROW) * hex::ROW;
        let new = match vk {
            VK_LEFT => c - 1,
            VK_RIGHT => c + 1,
            VK_UP => c - hex::ROW as i64,
            VK_DOWN => c + hex::ROW as i64,
            VK_PRIOR => c - page,
            VK_NEXT => c + page,
            VK_HOME if ctrl => 0,
            VK_END if ctrl => len as i64,
            VK_HOME => row_start as i64,
            VK_END => (row_start + hex::ROW - 1).min(len) as i64,
            _ => c,
        };
        d.caret = new.clamp(0, len as i64) as u64;
        if !shift {
            d.anchor = d.caret;
        }
        if vk == VK_PRIOR || vk == VK_NEXT {
            let n = if vk == VK_PRIOR {
                -(vis.saturating_sub(1).max(1) as i64)
            } else {
                vis.saturating_sub(1).max(1) as i64
            };
            d.scroll(n, vis);
        }
    }

    /// 某显示行中光标处的横坐标（相对文字左端）。
    fn x_in_row(d: &LargeDoc, font: &Font, start: u64, pos: u64) -> f32 {
        let nav = d.nav();
        let row = nav.row(start);
        let bytes = d
            .reader
            .read(row.start, (row.content_end - row.start) as usize);
        let dec = legacy_fn(d.legacy());
        let legacy: Option<decode::LegacyDecode> = if d.legacy().is_some() {
            Some(&dec)
        } else {
            None
        };
        let text = decode::decode_row(&bytes, d.cs, legacy);
        let pos = pos.clamp(row.start, row.content_end);
        TextLayout::new(&text.text, font)
            .map(|l| l.x_at(text.unit_of((pos - row.start) as u32)))
            .unwrap_or(0.0)
    }

    /// 显示行 `start` 中最接近横坐标 `x` 的偏移。
    fn pos_at_x(d: &LargeDoc, font: &Font, start: u64, x: f32) -> u64 {
        let nav = d.nav();
        let row = nav.row(start);
        let bytes = d
            .reader
            .read(row.start, (row.content_end - row.start) as usize);
        let dec = legacy_fn(d.legacy());
        let legacy: Option<decode::LegacyDecode> = if d.legacy().is_some() {
            Some(&dec)
        } else {
            None
        };
        let text = decode::decode_row(&bytes, d.cs, legacy);
        let u = TextLayout::new(&text.text, font)
            .map(|l| l.hit(x))
            .unwrap_or(0);
        (row.start + text.byte_of(u) as u64).min(row.content_end)
    }

    fn move_text(
        &self,
        doc: &Rc<RefCell<LargeDoc>>,
        font: &Font,
        vk: usize,
        ctrl: bool,
        shift: bool,
        vis: u64,
    ) {
        let mut d = doc.borrow_mut();
        let (sa, sb) = d.sel();
        if !shift && sa != sb && (vk == VK_LEFT || vk == VK_RIGHT) {
            d.caret = if vk == VK_LEFT { sa } else { sb };
            d.anchor = d.caret;
            d.want_x = None;
            return;
        }
        let page = vis.saturating_sub(1).max(1) as i64;
        let (caret, want_x, top) = {
            let nav = d.nav();
            let cur = nav.row_start(d.caret);
            let row = nav.row(cur);
            match vk {
                VK_LEFT => {
                    let c = if d.caret == row.start && row.line_start && cur > d.data_start {
                        match nav.prev_row(cur) {
                            Some(p) => nav.row(p).content_end,
                            None => d.caret,
                        }
                    } else {
                        nav.prev_char(d.caret)
                    };
                    (c, None, None)
                }
                VK_RIGHT => {
                    let c = if d.caret >= row.content_end {
                        row.next.unwrap_or(d.caret)
                    } else {
                        nav.next_char(d.caret).min(row.content_end)
                    };
                    (c, None, None)
                }
                VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT => {
                    let x = d
                        .want_x
                        .unwrap_or_else(|| Self::x_in_row(&d, font, cur, d.caret));
                    let n = match vk {
                        VK_UP => -1,
                        VK_DOWN => 1,
                        VK_PRIOR => -page,
                        _ => page,
                    };
                    let target = nav.step(cur, n);
                    let c = if target == cur {
                        // 已到头：上移到开头、下移到末尾
                        if n < 0 {
                            d.data_start
                        } else {
                            d.len()
                        }
                    } else {
                        Self::pos_at_x(&d, font, target, x)
                    };
                    let top = if vk == VK_PRIOR || vk == VK_NEXT {
                        Some(nav.step(d.top, n))
                    } else {
                        None
                    };
                    (c, Some(x), top)
                }
                VK_HOME => (if ctrl { d.data_start } else { row.start }, None, None),
                VK_END => (if ctrl { d.len() } else { row.content_end }, None, None),
                _ => (d.caret, d.want_x, None),
            }
        };
        d.caret = caret;
        d.want_x = want_x;
        if let Some(t) = top {
            d.top = t;
            d.clamp_top(vis);
        }
        if !shift {
            d.anchor = d.caret;
        }
    }

    /// 横向滚动使光标可见（文本模式）。
    pub fn ensure_caret_x(&self) {
        let Some(doc) = self.doc() else { return };
        let Some(font) = self.font() else { return };
        let (w, _) = client(self.hwnd.get());
        let mut d = doc.borrow_mut();
        let area = (w - self.text_x.get()).max(font.char_w * 8.0);
        let cx = match d.mode {
            Mode::Text => {
                let cur = d.nav().row_start(d.caret);
                Self::x_in_row(&d, &font, cur, d.caret)
            }
            Mode::Hex => {
                let lay = hex::Layout::for_len(d.len());
                let j = (d.caret % hex::ROW) as usize;
                lay.hex_col(j) as f32 * font.char_w
            }
        };
        let margin = font.char_w * 4.0;
        if cx < d.x_off {
            d.x_off = (cx - margin).max(0.0).round();
        } else if cx > d.x_off + area - font.char_w * 2.0 {
            d.x_off = (cx - area + margin).max(0.0).round();
        }
    }

    // ---- 自测 ----

    /// 立即绘制，返回绘制的行数（文本模式）或 0。
    pub fn st_paint(&self) -> usize {
        unsafe {
            InvalidateRect(self.hwnd.get(), null(), 0);
            windows_sys::Win32::Graphics::Gdi::UpdateWindow(self.hwnd.get());
        }
        self.painted.borrow().len()
    }

    pub fn st_has_surface(&self) -> bool {
        self.surface.borrow().is_some()
    }

    pub fn st_key(&self, vk: usize) {
        self.on_key(vk);
    }
}
