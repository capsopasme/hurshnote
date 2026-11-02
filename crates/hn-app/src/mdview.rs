//! Markdown 预览窗口：自绘子窗口（Direct2D / DirectWrite），显示 `hn_md` 解析出的扁平块列表。
//!
//! - 解析在后台线程（`hn_md::parse`），图片解码在另一个后台线程（WIC），结果经 channel +
//!   `PostMessage(WM_APP_MD)` 回到本窗口
//! - 每个块排版一次，按"内容哈希 + 宽度 + 字体代号"缓存；编辑时只有改动的块重新排版
//! - 按像素虚拟滚动，只绘制可见的块；源码行 ↔ 纵坐标的映射用于与编辑器滚动同步
//! - 选择：按字符（文字块）或整块（表格、图片、分隔线），Ctrl+C 复制纯文本
//! - 链接单击打开（由 `app` 判断种类：网页用默认浏览器，本地文件在编辑器中打开，页内锚点滚动）
//!
//! 借用规则：持有 `state` 的借用期间不调用可能重入窗口过程的函数（SetScrollInfo、SetFocus、
//! 回调 `app`……），先计算、释放借用再调用。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};

use hn_md::{Block, Document, Inline, Kind, Marker, Style};
use hn_render::{Align, Bitmap, RichText, Surface, TextFormat};
use hn_sci::theme::GRUVBOX_LIGHT;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, ScreenToClient, PAINTSTRUCT,
};
use windows_sys::Win32::UI::Controls::SetScrollInfo;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, GetClientRect,
    GetCursorPos, GetScrollInfo, GetWindowLongPtrW, IsWindowVisible, KillTimer, LoadCursorW,
    PostMessageW, RegisterClassExW, SetCursor, SetTimer, SetWindowLongPtrW, ShowWindow,
    TrackPopupMenu, SCROLLINFO, WNDCLASSEXW,
};

use crate::win::*;

const CLASS: &str = "HurshNoteMdPreview";
/// 后台解析或图片解码完成（wParam：0 解析，1 图片）
pub const WM_APP_MD: u32 = WM_APP + 30;
/// 超过此大小的文档不预览
pub const MAX_SOURCE: usize = 16 << 20;
const TIMER_AUTOSCROLL: usize = 1;
const IDC_HAND: u16 = 32649;
const IDC_ARROW_: u16 = 32512;

const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETCURSOR: u32 = 0x0020;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_VSCROLL: u32 = 0x0115;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_CAPTURECHANGED: u32 = 0x0215;
const WM_GETDLGCODE: u32 = 0x0087;
const DLGC_WANTARROWS: isize = 0x0001;
const DLGC_WANTCHARS: isize = 0x0080;
const HTCLIENT: u16 = 1;
const CS_DBLCLKS: u32 = 0x0008;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const GWLP_USERDATA: i32 = -21;
const TPM_RETURNCMD: u32 = 0x0100;
const TPM_NONOTIFY: u32 = 0x0080;
const MF_GRAYED_: u32 = 0x0001;

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
const VK_SPACE: usize = 0x20;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_END: usize = 0x23;
const VK_HOME: usize = 0x24;
const VK_UP: usize = 0x26;
const VK_DOWN: usize = 0x28;

const CMD_COPY: usize = 1;
const CMD_SELECT_ALL: usize = 2;
const CMD_COPY_LINK: usize = 3;
const CMD_CLOSE: usize = 4;

/// 正文字体（DirectWrite 自动为中文回退到微软雅黑）。
const BODY_FACE: &str = "Segoe UI";
/// 标题字号倍数（h1–h6）。
const HEADING_SCALE: [f32; 6] = [1.9, 1.5, 1.25, 1.1, 1.0, 0.95];

fn key_down(vk: i32) -> bool {
    let s = unsafe { GetKeyState(vk) };
    s < 0
}

fn client(hwnd: HWND) -> (f32, f32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    ((rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32)
}

// ---------------------------------------------------------------------------
// 字体与排版结果

/// 预览字体参数（像素）。
#[derive(Debug, Clone, PartialEq)]
pub struct StyleSpec {
    pub body_px: f32,
    pub code_face: String,
    pub code_px: f32,
    /// 图片按 DPI 放大的倍数（dpi / 96）
    pub px_scale: f32,
}

struct Fonts {
    body: TextFormat,
    heads: Vec<TextFormat>,
    code: TextFormat,
    cell: TextFormat,
    spec: StyleSpec,
}

impl Fonts {
    fn new(spec: &StyleSpec) -> Option<Fonts> {
        let b = spec.body_px.max(6.0);
        let heads = HEADING_SCALE
            .iter()
            .map(|k| TextFormat::new(BODY_FACE, b * k, true, false, 1.25, true).ok())
            .collect::<Option<Vec<_>>>()?;
        Some(Fonts {
            body: TextFormat::new(BODY_FACE, b, false, false, 1.5, true).ok()?,
            heads,
            code: TextFormat::new(
                &spec.code_face,
                spec.code_px.max(6.0),
                false,
                false,
                1.3,
                true,
            )
            .ok()?,
            cell: TextFormat::new(BODY_FACE, b, false, false, 1.3, true).ok()?,
            spec: spec.clone(),
        })
    }

    fn em(&self) -> f32 {
        self.body.size
    }
}

/// 一段排好版的文字。
struct TextBox {
    rt: RichText,
    wide: Vec<u16>,
    /// (起点, 终点, 链接下标)，UTF-16 下标
    links: Vec<(u32, u32, u32)>,
    /// 行内代码的范围（画底色）
    code: Vec<(u32, u32)>,
}

impl TextBox {
    fn link_at(&self, pos: usize) -> Option<u32> {
        let p = pos as u32;
        self.links
            .iter()
            .find(|(a, b, _)| *a <= p && p < *b)
            .map(|x| x.2)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Para,
    Heading(u8),
    Code,
    Html,
}

struct TableBox {
    cols: Vec<f32>,
    rows: Vec<f32>,
    /// 第 0 行是表头
    cells: Vec<Vec<TextBox>>,
    pad_x: f32,
    pad_y: f32,
}

enum Content {
    Text(TextKind, TextBox),
    Rule,
    Table(TableBox),
    Image { src: String, alt: String },
}

struct Laid {
    content: Rc<Content>,
    marker: Option<Rc<RichText>>,
    x: f32,
    w: f32,
    y: f32,
    h: f32,
    /// 与上一块的间距（引用竖线跨越用）
    gap: f32,
}

/// 选择位置：块下标 + 块内位置（文字块为 UTF-16 下标；其他块 0 = 开头、1 = 结尾）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub block: usize,
    pub off: u32,
}

enum ImgState {
    Loading,
    Ready {
        w: u32,
        h: u32,
        pixels: Vec<u8>,
        orig: (u32, u32),
        bmp: RefCell<Option<Bitmap>>,
    },
    Failed(String),
}

struct ImgEntry {
    state: ImgState,
    mtime: Option<std::time::SystemTime>,
}

struct ParseJob {
    doc_id: u32,
    gen: u64,
    text: String,
}

struct ParseOut {
    doc_id: u32,
    gen: u64,
    doc: Document,
}

struct ImgJob {
    path: PathBuf,
}

struct ImgOut {
    path: PathBuf,
    mtime: Option<std::time::SystemTime>,
    result: Result<hn_render::Image, String>,
}

#[derive(Default)]
struct State {
    doc_id: u32,
    gen: u64,
    /// 已安装的解析结果对应的代号
    shown_gen: u64,
    doc: Rc<Document>,
    base: Option<PathBuf>,
    message: Option<String>,
    laid: Vec<Laid>,
    /// 排版时的客户区宽度；为 0 表示需要重新排版
    layout_w: f32,
    total_h: f32,
    cache: HashMap<(u64, u32, u64), Rc<Content>>,
    /// 列表标记的排版（按标记文字缓存，字体变化时清空）
    markers: HashMap<Marker, Rc<RichText>>,
    fonts: Option<Rc<Fonts>>,
    spec: Option<StyleSpec>,
    font_gen: u64,
    scroll: f32,
    sel: Option<(Pos, Pos)>,
    selecting: bool,
    /// 按下鼠标的位置（判断单击与拖动）
    press: Option<(i32, i32)>,
    images: HashMap<PathBuf, ImgEntry>,
    /// 新文档第一次排版后滚到顶部
    reset_scroll: bool,
    /// 左右页边距（排版时计算）
    pad_x: f32,
}

pub struct MdView {
    hwnd: Cell<HWND>,
    st: RefCell<State>,
    surface: RefCell<Option<Surface>>,
    parse_tx: RefCell<Option<Sender<ParseJob>>>,
    parse_rx: RefCell<Option<Receiver<ParseOut>>>,
    img_tx: RefCell<Option<Sender<ImgJob>>>,
    img_rx: RefCell<Option<Receiver<ImgOut>>>,
    /// 调试与自测：最近一次排版重建（未命中缓存）的块数
    rebuilt: Cell<usize>,
}

thread_local! {
    static VIEW: RefCell<Option<Rc<MdView>>> = const { RefCell::new(None) };
}

/// 预览窗口（创建后才有）。
pub fn view() -> Option<Rc<MdView>> {
    VIEW.with(|v| v.borrow().clone())
}

/// 创建预览窗口（隐藏）与后台线程。
pub fn create(parent: HWND, hinstance: *mut core::ffi::c_void) -> HWND {
    let class = wide(CLASS);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: null_mut(),
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW_ as usize as _) },
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class.as_ptr(),
        hIconSm: null_mut(),
    };
    unsafe { RegisterClassExW(&wc) };
    let v = Rc::new(MdView {
        hwnd: Cell::new(null_mut()),
        st: RefCell::new(State::default()),
        surface: RefCell::new(None),
        parse_tx: RefCell::new(None),
        parse_rx: RefCell::new(None),
        img_tx: RefCell::new(None),
        img_rx: RefCell::new(None),
        rebuilt: Cell::new(0),
    });
    VIEW.with(|x| *x.borrow_mut() = Some(v.clone()));
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            null(),
            WS_CHILD | WS_VSCROLL | WS_CLIPSIBLINGS | WS_TABSTOP,
            0,
            0,
            0,
            0,
            parent,
            210 as *mut core::ffi::c_void,
            hinstance,
            null(),
        )
    };
    v.hwnd.set(hwnd);
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1) };
    v.start_workers();
    hwnd
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } != 0 {
        let r = crate::crash::guard(|| view().and_then(|v| v.handle(msg, wp, lp)));
        if let Some(Some(r)) = r {
            return r;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// 解析线程：只处理最新的一份文本（中间的版本直接丢弃）。
fn parse_worker(rx: Receiver<ParseJob>, tx: Sender<ParseOut>, hwnd: isize) {
    while let Ok(mut job) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            job = newer;
        }
        let doc = hn_md::parse(&job.text);
        drop(job.text);
        if tx
            .send(ParseOut {
                doc_id: job.doc_id,
                gen: job.gen,
                doc,
            })
            .is_err()
        {
            return;
        }
        unsafe { PostMessageW(hwnd as HWND, WM_APP_MD, 0, 0) };
    }
}

/// 图片线程：WIC 解码并缩小到不超过 2560 像素宽，拒绝超过 6400 万像素的原图。
fn image_worker(rx: Receiver<ImgJob>, tx: Sender<ImgOut>, hwnd: isize) {
    while let Ok(job) = rx.recv() {
        let meta = std::fs::metadata(&job.path);
        let mtime = meta.as_ref().ok().and_then(|m| m.modified().ok());
        let result = match meta {
            Ok(m) if m.is_file() && m.len() > 200 << 20 => Err("文件太大".to_string()),
            Ok(m) if m.is_file() => hn_render::decode_image(&job.path, 2560, 64_000_000),
            Ok(_) => Err("不是文件".to_string()),
            Err(e) => Err(e.to_string()),
        };
        if tx
            .send(ImgOut {
                path: job.path,
                mtime,
                result,
            })
            .is_err()
        {
            return;
        }
        unsafe { PostMessageW(hwnd as HWND, WM_APP_MD, 1, 0) };
    }
}

// ---------------------------------------------------------------------------
// 排版

fn inline_box(inline: &Inline, fmt: &TextFormat, w: f32, fonts: &Fonts) -> Option<TextBox> {
    let p = &GRUVBOX_LIGHT;
    let (wide, spans) = inline.to_utf16();
    let mut rt = RichText::new(&wide, fmt, w).ok()?;
    let mut links = Vec::new();
    let mut code = Vec::new();
    for s in &spans {
        let (a, n) = (s.start as usize, (s.end - s.start) as usize);
        if n == 0 {
            continue;
        }
        if s.style.has(Style::BOLD) {
            rt.bold(a, n);
        }
        if s.style.has(Style::ITALIC) || s.style.has(Style::ALT) {
            rt.italic(a, n);
        }
        if s.style.has(Style::STRIKE) {
            rt.strike(a, n);
        }
        if s.style.has(Style::CODE) {
            rt.family(&fonts.spec.code_face, a, n);
            rt.font_size(fmt.size * 0.9, a, n);
            code.push((s.start, s.end));
        }
        let color = if s.style.has(Style::LINK) || (s.link.is_some() && !s.style.has(Style::ALT)) {
            rt.underline(a, n);
            Some(p.blue)
        } else if s.style.has(Style::CODE) {
            Some(p.aqua)
        } else if s.style.has(Style::HTML) || s.style.has(Style::ALT) {
            Some(p.gray)
        } else {
            None
        };
        if let Some(c) = color {
            rt.color(c, a, n);
        }
        if let Some(l) = s.link {
            links.push((s.start, s.end, l));
        }
    }
    rt.finish();
    Some(TextBox {
        rt,
        wide,
        links,
        code,
    })
}

fn plain_box(text: &str, fmt: &TextFormat, w: f32) -> Option<TextBox> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let rt = RichText::new(&wide, fmt, w).ok()?;
    Some(TextBox {
        rt,
        wide,
        links: Vec::new(),
        code: Vec::new(),
    })
}

fn build_table(t: &hn_md::Table, w: f32, fonts: &Fonts) -> Option<TableBox> {
    let em = fonts.em();
    let (pad_x, pad_y) = (em * 0.55, em * 0.3);
    let ncol = t.columns().max(1);
    let mut cells: Vec<Vec<TextBox>> = Vec::with_capacity(t.rows.len() + 1);
    let empty = Inline::default();
    for (ri, row) in std::iter::once(&t.head).chain(t.rows.iter()).enumerate() {
        let mut r = Vec::with_capacity(ncol);
        for c in 0..ncol {
            let inline = row.get(c).unwrap_or(&empty);
            let tb = inline_box(inline, &fonts.cell, 1.0e6, fonts)?;
            if ri == 0 {
                tb.rt.bold(0, tb.wide.len());
            }
            r.push(tb);
        }
        cells.push(r);
    }
    // 列宽：放得下就用自然宽度；放不下时每列至少最小宽度，剩余按"自然 - 最小"比例分配
    let mut natural = vec![0f32; ncol];
    let mut minimum = vec![0f32; ncol];
    for r in &mut cells {
        for (c, tb) in r.iter_mut().enumerate() {
            tb.rt.finish();
            natural[c] = natural[c].max(tb.rt.width.ceil() + 2.0 * pad_x);
            minimum[c] = minimum[c].max(tb.rt.min_width().ceil() + 2.0 * pad_x);
        }
    }
    let avail = (w - 1.0).max(ncol as f32 * 2.0 * pad_x);
    let nat_sum: f32 = natural.iter().sum();
    let min_sum: f32 = minimum.iter().sum();
    let cols: Vec<f32> = if nat_sum <= avail {
        natural
    } else if min_sum >= avail {
        minimum
    } else {
        let extra = avail - min_sum;
        let flex: f32 = natural
            .iter()
            .zip(&minimum)
            .map(|(n, m)| (n - m).max(0.0))
            .sum::<f32>()
            .max(1.0);
        natural
            .iter()
            .zip(&minimum)
            .map(|(n, m)| m + extra * (n - m).max(0.0) / flex)
            .collect()
    };
    let mut rows = Vec::with_capacity(cells.len());
    for r in &mut cells {
        let mut h = 0f32;
        for (c, tb) in r.iter_mut().enumerate() {
            tb.rt.set_max_width((cols[c] - 2.0 * pad_x).max(1.0));
            let a = match t.aligns.get(c) {
                Some(hn_md::Align::Center) => Align::Center,
                Some(hn_md::Align::Right) => Align::Right,
                _ => Align::Left,
            };
            if a != Align::Left {
                tb.rt.align(a);
                tb.rt.finish();
            }
            h = h.max(tb.rt.height);
        }
        rows.push(h.ceil() + 2.0 * pad_y);
    }
    Some(TableBox {
        cols,
        rows,
        cells,
        pad_x,
        pad_y,
    })
}

fn build_content(b: &Block, w: f32, fonts: &Fonts) -> Option<Content> {
    let em = fonts.em();
    Some(match &b.kind {
        Kind::Para(i) => Content::Text(TextKind::Para, inline_box(i, &fonts.body, w, fonts)?),
        Kind::Heading(level, i) => {
            let lv = (*level).clamp(1, 6);
            let fmt = &fonts.heads[lv as usize - 1];
            Content::Text(TextKind::Heading(lv), inline_box(i, fmt, w, fonts)?)
        }
        Kind::Code { text, .. } => {
            let inner = (w - 2.0 * em * 0.8).max(10.0);
            Content::Text(TextKind::Code, plain_box(text, &fonts.code, inner)?)
        }
        Kind::Html(text) => {
            let inner = (w - 2.0 * em * 0.8).max(10.0);
            let mut tb = plain_box(text, &fonts.code, inner)?;
            tb.rt.color(GRUVBOX_LIGHT.gray, 0, tb.wide.len());
            Content::Text(TextKind::Html, tb)
        }
        Kind::Rule => Content::Rule,
        Kind::Table(t) => Content::Table(build_table(t, w, fonts)?),
        Kind::Image { src, alt } => Content::Image {
            src: src.clone(),
            alt: alt.clone(),
        },
    })
}

fn build_marker(m: &Marker, fonts: &Fonts) -> Option<RichText> {
    let label = m.label();
    let wide: Vec<u16> = label.encode_utf16().collect();
    let mut rt = RichText::new(&wide, &fonts.body, 1.0e5).ok()?;
    let c = match m {
        Marker::Task(true) => GRUVBOX_LIGHT.green,
        Marker::Footnote(_) => GRUVBOX_LIGHT.blue,
        _ => GRUVBOX_LIGHT.fg3,
    };
    rt.color(c, 0, wide.len());
    rt.finish();
    Some(rt)
}

/// 块的上、下外边距。
fn margins(b: &Block, first: bool, em: f32) -> (f32, f32) {
    match &b.kind {
        Kind::Heading(l, _) => (
            if first {
                0.0
            } else if *l <= 2 {
                em * 1.4
            } else {
                em * 1.1
            },
            em * 0.5,
        ),
        Kind::Para(_) if b.compact => (0.0, em * 0.15),
        Kind::Para(_) => (0.0, em * 0.7),
        _ => (0.0, em * 0.8),
    }
}

impl MdView {
    pub fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    pub fn is_shown(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd.get()) != 0 }
    }

    pub fn show(&self, on: bool) {
        if on != self.is_shown() {
            unsafe { ShowWindow(self.hwnd.get(), if on { SW_SHOW } else { SW_HIDE }) };
            if on {
                self.refresh();
            }
        }
    }

    /// 当前显示的文档 id（0 为没有）。
    pub fn doc_id(&self) -> u32 {
        // 可能在本窗口的处理过程中被编辑器通知间接调用，不能因借用冲突而 panic
        self.st.try_borrow().map(|s| s.doc_id).unwrap_or(0)
    }

    fn start_workers(&self) {
        let hwnd = self.hwnd.get() as isize;
        let (ptx, prx) = channel::<ParseJob>();
        let (otx, orx) = channel::<ParseOut>();
        std::thread::Builder::new()
            .name("md-parse".into())
            .spawn(move || parse_worker(prx, otx, hwnd))
            .ok();
        let (itx, irx) = channel::<ImgJob>();
        let (iotx, iorx) = channel::<ImgOut>();
        std::thread::Builder::new()
            .name("md-image".into())
            .spawn(move || image_worker(irx, iotx, hwnd))
            .ok();
        *self.parse_tx.borrow_mut() = Some(ptx);
        *self.parse_rx.borrow_mut() = Some(orx);
        *self.img_tx.borrow_mut() = Some(itx);
        *self.img_rx.borrow_mut() = Some(iorx);
    }

    /// 设置字体（DPI、缩放或编辑器字体变化时）。
    pub fn set_style(&self, spec: StyleSpec) {
        let mut st = self.st.borrow_mut();
        if st.spec.as_ref() == Some(&spec) {
            return;
        }
        st.spec = Some(spec);
        st.fonts = None;
        st.font_gen += 1;
        st.cache.clear();
        st.markers.clear();
        st.layout_w = 0.0;
        drop(st);
        self.refresh();
    }

    /// 显示文档 `doc_id` 的源码。`sync` 为真时在当前线程立即解析（自测、切换文档时避免闪烁）。
    pub fn set_source(
        &self,
        doc_id: u32,
        text: String,
        base: Option<PathBuf>,
        reset_scroll: bool,
        sync: bool,
    ) {
        let mut st = self.st.borrow_mut();
        let switched = st.doc_id != doc_id;
        st.doc_id = doc_id;
        st.base = base;
        st.gen += 1;
        let gen = st.gen;
        if reset_scroll || switched {
            st.reset_scroll = true;
            st.sel = None;
        }
        st.message = None;
        drop(st);
        if sync {
            let doc = hn_md::parse(&text);
            self.install(doc_id, gen, doc);
        } else if let Some(tx) = self.parse_tx.borrow().as_ref() {
            let _ = tx.send(ParseJob { doc_id, gen, text });
        }
    }

    /// 文档太大等情况：只显示一句说明。
    pub fn set_message(&self, doc_id: u32, msg: &str) {
        {
            let mut st = self.st.borrow_mut();
            st.doc_id = doc_id;
            st.gen += 1;
            st.shown_gen = st.gen;
            st.doc = Rc::new(Document::default());
            st.laid.clear();
            st.sel = None;
            st.message = Some(msg.to_string());
            st.layout_w = 0.0;
        }
        self.refresh();
    }

    /// 不显示任何文档。
    pub fn clear(&self) {
        let mut st = self.st.borrow_mut();
        st.doc_id = 0;
        st.gen += 1;
        st.doc = Rc::new(Document::default());
        st.laid.clear();
        st.sel = None;
        st.layout_w = 0.0;
    }

    fn install(&self, doc_id: u32, gen: u64, doc: Document) {
        {
            let mut st = self.st.borrow_mut();
            if st.doc_id != doc_id || gen < st.shown_gen || gen > st.gen {
                return;
            }
            st.shown_gen = gen;
            st.doc = Rc::new(doc);
            st.layout_w = 0.0;
            // 选区可能已不合法
            let n = st.doc.blocks.len();
            if let Some((a, b)) = st.sel {
                if a.block >= n || b.block >= n {
                    st.sel = None;
                }
            }
        }
        self.request_images();
        self.ensure_layout();
        self.refresh();
        if let Some(app) = crate::app::app() {
            app.md_doc_ready();
        }
    }

    fn on_app_msg(&self, kind: usize) {
        if kind == 0 {
            let mut latest = None;
            if let Some(rx) = self.parse_rx.borrow().as_ref() {
                while let Ok(o) = rx.try_recv() {
                    latest = Some(o);
                }
            }
            if let Some(o) = latest {
                self.install(o.doc_id, o.gen, o.doc);
            }
        } else {
            let mut got = Vec::new();
            if let Some(rx) = self.img_rx.borrow().as_ref() {
                while let Ok(o) = rx.try_recv() {
                    got.push(o);
                }
            }
            if got.is_empty() {
                return;
            }
            {
                let mut st = self.st.borrow_mut();
                for o in got {
                    let state = match o.result {
                        Ok(img) => ImgState::Ready {
                            w: img.width,
                            h: img.height,
                            orig: (img.orig_width, img.orig_height),
                            pixels: img.pixels,
                            bmp: RefCell::new(None),
                        },
                        Err(e) => ImgState::Failed(e),
                    };
                    st.images.insert(
                        o.path,
                        ImgEntry {
                            state,
                            mtime: o.mtime,
                        },
                    );
                }
                // 图片尺寸变了，重新计算位置
                st.layout_w = 0.0;
            }
            self.ensure_layout();
            self.refresh();
        }
    }

    /// 解析图片地址为本地路径（相对路径以 Markdown 文件所在目录为基准）。
    fn image_path(base: Option<&Path>, src: &str) -> Option<PathBuf> {
        let hn_md::LinkTarget::File(p) = hn_md::classify_link(src) else {
            return None;
        };
        let p = PathBuf::from(p.replace('/', "\\"));
        if p.is_absolute() {
            Some(p)
        } else {
            base.map(|b| b.join(p))
        }
    }

    /// 为文档中的本地图片发出解码请求；文件修改过的重新解码；不再引用的释放。
    fn request_images(&self) {
        let mut jobs = Vec::new();
        {
            let mut st = self.st.borrow_mut();
            let doc = st.doc.clone();
            let base = st.base.clone();
            let mut wanted = Vec::new();
            for b in &doc.blocks {
                if let Kind::Image { src, .. } = &b.kind {
                    if let Some(p) = Self::image_path(base.as_deref(), src) {
                        wanted.push(p);
                    }
                }
            }
            st.images.retain(|k, _| wanted.contains(k));
            for p in wanted {
                let stale = match st.images.get(&p) {
                    None => true,
                    Some(e) => {
                        !matches!(e.state, ImgState::Loading)
                            && std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()) != e.mtime
                    }
                };
                if stale {
                    st.images.insert(
                        p.clone(),
                        ImgEntry {
                            state: ImgState::Loading,
                            mtime: None,
                        },
                    );
                    jobs.push(p);
                }
            }
        }
        if let Some(tx) = self.img_tx.borrow().as_ref() {
            for path in jobs {
                let _ = tx.send(ImgJob { path });
            }
        }
    }

    /// 按需（宽度、字体、文档变化后）重新排版。
    fn ensure_layout(&self) {
        let (w, h) = client(self.hwnd.get());
        if w < 1.0 {
            return;
        }
        let mut st = self.st.borrow_mut();
        if st.layout_w == w && !st.laid.is_empty() {
            return;
        }
        if st.layout_w == w && st.doc.blocks.is_empty() {
            return;
        }
        let Some(spec) = st.spec.clone() else { return };
        if st.fonts.is_none() {
            st.fonts = Fonts::new(&spec).map(Rc::new);
        }
        let Some(fonts) = st.fonts.clone() else {
            return;
        };
        let em = fonts.em();
        let pad_x = (em * 1.2).min(w * 0.08).max(4.0);
        let pad_top = em * 0.8;
        let indent_w = em * 1.8;
        let quote_w = em * 1.1;
        let doc = st.doc.clone();
        let font_gen = st.font_gen;
        let old_cache = std::mem::take(&mut st.cache);
        let mut cache = HashMap::with_capacity(doc.blocks.len());
        let mut laid = Vec::with_capacity(doc.blocks.len());
        let mut rebuilt = 0;
        let mut y = pad_top;
        let mut prev_mb = 0f32;
        let base = st.base.clone();
        for (i, b) in doc.blocks.iter().enumerate() {
            let x = pad_x + b.quote as f32 * quote_w + b.indent as f32 * indent_w;
            let bw = (w - x - pad_x).max(em * 3.0);
            let key = (b.content_hash(), bw.to_bits(), font_gen);
            let content = match old_cache.get(&key).or_else(|| cache.get(&key)) {
                Some(c) => Rc::clone(c),
                None => {
                    rebuilt += 1;
                    match build_content(b, bw, &fonts) {
                        Some(c) => Rc::new(c),
                        None => continue,
                    }
                }
            };
            cache.insert(key, content.clone());
            let marker = match &b.marker {
                Some(m) => match st.markers.get(m).cloned() {
                    Some(r) => Some(r),
                    None => {
                        let r = build_marker(m, &fonts).map(Rc::new);
                        if let Some(r) = &r {
                            st.markers.insert(m.clone(), r.clone());
                        }
                        r
                    }
                },
                None => None,
            };
            let (mt, mb) = margins(b, i == 0, em);
            let gap = if i == 0 { 0.0 } else { prev_mb.max(mt) };
            y += gap;
            let bh = match &*content {
                Content::Text(TextKind::Para, tb) => tb.rt.height,
                Content::Text(TextKind::Heading(l), tb) => {
                    tb.rt.height + if *l <= 2 { em * 0.35 + 1.0 } else { 0.0 }
                }
                Content::Text(_, tb) => tb.rt.height + 2.0 * em * 0.6,
                Content::Rule => em,
                Content::Table(t) => t.rows.iter().sum::<f32>() + 1.0,
                Content::Image { src, .. } => {
                    let path = Self::image_path(base.as_deref(), src);
                    match path
                        .as_ref()
                        .and_then(|p| st.images.get(p))
                        .map(|e| &e.state)
                    {
                        Some(ImgState::Ready { orig, .. }) => {
                            let (iw, ih) =
                                (orig.0 as f32 * spec.px_scale, orig.1 as f32 * spec.px_scale);
                            let k = (bw / iw).min(1.0);
                            (ih * k).max(1.0)
                        }
                        _ => fonts.body.line_h * 1.6,
                    }
                }
            };
            laid.push(Laid {
                content,
                marker,
                x,
                w: bw,
                y,
                h: bh.ceil(),
                gap,
            });
            y += bh.ceil();
            prev_mb = mb;
        }
        st.cache = cache;
        st.laid = laid;
        st.pad_x = pad_x;
        st.total_h = y + prev_mb + em * 2.0;
        st.layout_w = w;
        let max = (st.total_h - h).max(0.0);
        if st.reset_scroll {
            st.reset_scroll = false;
            st.scroll = 0.0;
        }
        st.scroll = st.scroll.clamp(0.0, max);
        self.rebuilt.set(rebuilt);
    }

    /// 重绘并更新滚动条。
    pub fn refresh(&self) {
        unsafe { InvalidateRect(self.hwnd.get(), null(), 0) };
        self.update_scrollbar();
    }

    fn update_scrollbar(&self) {
        let hwnd = self.hwnd.get();
        let (_, h) = client(hwnd);
        let (total, pos) = {
            let st = self.st.borrow();
            (st.total_h, st.scroll)
        };
        let si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: total.max(1.0) as i32,
            nPage: h.max(1.0) as u32,
            nPos: pos as i32,
            nTrackPos: 0,
        };
        unsafe { SetScrollInfo(hwnd, SB_VERT, &si, 1) };
    }

    fn max_scroll(&self) -> f32 {
        let (_, h) = client(self.hwnd.get());
        (self.st.borrow().total_h - h).max(0.0)
    }

    /// 滚动到纵坐标（不通知编辑器）。
    fn set_scroll(&self, y: f32) {
        let max = self.max_scroll();
        let y = y.clamp(0.0, max).round();
        let changed = {
            let mut st = self.st.borrow_mut();
            let c = st.scroll != y;
            st.scroll = y;
            c
        };
        if changed {
            self.refresh();
        }
    }

    /// 用户滚动：滚动后通知 `app` 同步编辑器。
    fn user_scroll(&self, y: f32) {
        self.set_scroll(y);
        if let Some(app) = crate::app::app() {
            app.md_user_scrolled();
        }
    }

    fn line_px(&self) -> f32 {
        self.st
            .borrow()
            .fonts
            .as_ref()
            .map(|f| f.body.line_h)
            .unwrap_or(20.0)
    }

    // ---- 源码行 ↔ 纵坐标 ----

    /// 源码行（可带小数）对应的纵坐标。
    fn y_for_line(st: &State, line: f64) -> f32 {
        let blocks = &st.doc.blocks;
        if st.laid.len() != blocks.len() || blocks.is_empty() {
            return 0.0;
        }
        let l = line.max(0.0);
        let li = l.floor() as usize;
        let Some(i) = st.doc.block_at_line(li) else {
            // 第一个块之前
            let b0 = &blocks[0];
            let t = (l / (b0.line.max(1) as f64)).min(1.0) as f32;
            return st.laid[0].y * t;
        };
        let (b, lb) = (&blocks[i], &st.laid[i]);
        let span = (b.last_line - b.line + 1) as f64;
        if li <= b.last_line {
            let f = ((l - b.line as f64) / span).clamp(0.0, 1.0) as f32;
            return lb.y + f * lb.h;
        }
        // 两块之间的空行
        let y0 = lb.y + lb.h;
        match (blocks.get(i + 1), st.laid.get(i + 1)) {
            (Some(nb), Some(nl)) => {
                let from = (b.last_line + 1) as f64;
                let to = nb.line.max(b.last_line + 1) as f64;
                let t = if to > from {
                    ((l - from) / (to - from)).clamp(0.0, 1.0) as f32
                } else {
                    1.0
                };
                y0 + t * (nl.y - y0)
            }
            _ => y0,
        }
    }

    /// 纵坐标对应的源码行（带小数）。
    fn line_for_y(st: &State, y: f32) -> f64 {
        let blocks = &st.doc.blocks;
        if st.laid.len() != blocks.len() || blocks.is_empty() {
            return 0.0;
        }
        let n = st.laid.partition_point(|l| l.y <= y);
        if n == 0 {
            let l0 = &st.laid[0];
            let t = if l0.y > 0.0 {
                (y / l0.y).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return t as f64 * blocks[0].line as f64;
        }
        let i = n - 1;
        let (b, lb) = (&blocks[i], &st.laid[i]);
        let span = (b.last_line - b.line + 1) as f64;
        if y <= lb.y + lb.h {
            let f = if lb.h > 0.0 {
                ((y - lb.y) / lb.h).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return b.line as f64 + f as f64 * span;
        }
        let y0 = lb.y + lb.h;
        let from = (b.last_line + 1) as f64;
        match (blocks.get(i + 1), st.laid.get(i + 1)) {
            (Some(nb), Some(nl)) if nl.y > y0 => {
                let t = ((y - y0) / (nl.y - y0)).clamp(0.0, 1.0) as f64;
                from + t * (nb.line as f64 - from).max(0.0)
            }
            _ => from,
        }
    }

    /// 编辑器滚动后：让源码行 `line` 位于顶部；`at_end` 时滚到底。
    pub fn scroll_to_line(&self, line: f64, at_end: bool) {
        self.ensure_layout();
        if at_end {
            let max = self.max_scroll();
            self.set_scroll(max);
            return;
        }
        let y = {
            let st = self.st.borrow();
            if line <= 0.0 {
                0.0
            } else {
                let pad = st.fonts.as_ref().map(|f| f.em() * 0.8).unwrap_or(0.0);
                (Self::y_for_line(&st, line) - pad).max(0.0)
            }
        };
        self.set_scroll(y);
    }

    /// 预览顶部对应的源码行，以及是否已滚到底。
    pub fn top_line(&self) -> (f64, bool) {
        let max = self.max_scroll();
        let st = self.st.borrow();
        let pad = st.fonts.as_ref().map(|f| f.em() * 0.8).unwrap_or(0.0);
        let line = if st.scroll <= 0.0 {
            0.0
        } else {
            Self::line_for_y(&st, st.scroll + pad)
        };
        (line, max > 0.0 && st.scroll >= max - 1.0)
    }

    /// 滚动到块（页内链接）。
    fn scroll_to_block(&self, i: usize) {
        let y = {
            let st = self.st.borrow();
            let pad = st.fonts.as_ref().map(|f| f.em() * 0.8).unwrap_or(0.0);
            st.laid.get(i).map(|l| (l.y - pad).max(0.0))
        };
        if let Some(y) = y {
            self.user_scroll(y);
        }
    }

    // ---- 消息 ----

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
                self.ensure_layout();
                let max = self.max_scroll();
                {
                    let mut st = self.st.borrow_mut();
                    st.scroll = st.scroll.clamp(0.0, max);
                }
                self.refresh();
                Some(0)
            }
            WM_APP_MD => {
                self.on_app_msg(wp);
                Some(0)
            }
            WM_VSCROLL => {
                self.on_vscroll(loword(wp));
                Some(0)
            }
            WM_MOUSEWHEEL => {
                let delta = hiword(wp) as i16 as i32;
                if key_down(VK_CONTROL) {
                    if let Some(app) = crate::app::app() {
                        app.md_ctrl_wheel(delta);
                    }
                } else {
                    let cur = self.st.borrow().scroll;
                    let step = self.line_px() * 3.0;
                    self.user_scroll(cur - delta as f32 / WHEEL_DELTA as f32 * step);
                }
                Some(0)
            }
            WM_GETDLGCODE => Some(DLGC_WANTARROWS | DLGC_WANTCHARS),
            WM_KEYDOWN => self.on_key(wp).then_some(0),
            WM_CHAR => Some(0),
            WM_SETCURSOR => {
                if loword(lp as usize) == HTCLIENT {
                    let mut pt = POINT { x: 0, y: 0 };
                    unsafe {
                        GetCursorPos(&mut pt);
                        ScreenToClient(self.hwnd.get(), &mut pt);
                    }
                    let over = self.link_at(pt.x as f32, pt.y as f32).is_some();
                    let id = if over { IDC_HAND } else { IDC_ARROW_ };
                    unsafe { SetCursor(LoadCursorW(null_mut(), id as usize as _)) };
                    Some(1)
                } else {
                    None
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_from_lparam(lp);
                unsafe {
                    SetFocus(self.hwnd.get());
                    SetCapture(self.hwnd.get());
                }
                let p = self.pos_at(x as f32, y as f32);
                {
                    let mut st = self.st.borrow_mut();
                    st.selecting = true;
                    st.press = Some((x, y));
                    st.sel = match (p, st.sel) {
                        (Some(p), Some((a, _))) if key_down(VK_SHIFT) => Some((a, p)),
                        (Some(p), _) => Some((p, p)),
                        (None, _) => None,
                    };
                }
                self.refresh();
                Some(0)
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = point_from_lparam(lp);
                self.select_block_at(x as f32, y as f32);
                Some(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_from_lparam(lp);
                self.drag_to(x, y);
                Some(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = point_from_lparam(lp);
                let (was, press) = {
                    let mut st = self.st.borrow_mut();
                    let w = st.selecting;
                    st.selecting = false;
                    (w, st.press.take())
                };
                unsafe { KillTimer(self.hwnd.get(), TIMER_AUTOSCROLL) };
                if was {
                    unsafe { ReleaseCapture() };
                }
                // 没有拖动的单击：打开链接
                if let Some((px, py)) = press {
                    if (px - x).abs() <= 3 && (py - y).abs() <= 3 && !key_down(VK_SHIFT) {
                        if let Some(url) = self.link_at(x as f32, y as f32) {
                            self.st.borrow_mut().sel = None;
                            self.refresh();
                            self.open_link(&url);
                        }
                    }
                }
                Some(0)
            }
            WM_CAPTURECHANGED => {
                self.st.borrow_mut().selecting = false;
                unsafe { KillTimer(self.hwnd.get(), TIMER_AUTOSCROLL) };
                Some(0)
            }
            WM_TIMER if wp == TIMER_AUTOSCROLL => {
                let mut pt = POINT { x: 0, y: 0 };
                unsafe {
                    GetCursorPos(&mut pt);
                    ScreenToClient(self.hwnd.get(), &mut pt);
                }
                self.drag_to(pt.x, pt.y);
                Some(0)
            }
            WM_CONTEXTMENU => {
                self.context_menu(lp);
                Some(0)
            }
            _ => None,
        }
    }

    fn on_vscroll(&self, code: u16) {
        let (_, h) = client(self.hwnd.get());
        let cur = self.st.borrow().scroll;
        let line = self.line_px();
        let y = match code {
            SB_LINEUP => cur - line,
            SB_LINEDOWN => cur + line,
            SB_PAGEUP => cur - (h - line).max(line),
            SB_PAGEDOWN => cur + (h - line).max(line),
            SB_TOP => 0.0,
            SB_BOTTOM => f32::MAX,
            SB_THUMBTRACK | SB_THUMBPOSITION => {
                let mut si: SCROLLINFO = unsafe { std::mem::zeroed() };
                si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
                si.fMask = SIF_TRACKPOS;
                unsafe { GetScrollInfo(self.hwnd.get(), SB_VERT, &mut si) };
                si.nTrackPos as f32
            }
            _ => return,
        };
        self.user_scroll(y);
    }

    fn on_key(&self, vk: usize) -> bool {
        let (_, h) = client(self.hwnd.get());
        let cur = self.st.borrow().scroll;
        let line = self.line_px();
        let ctrl = key_down(VK_CONTROL);
        let target = match vk {
            VK_UP => Some(cur - line),
            VK_DOWN => Some(cur + line),
            VK_PRIOR => Some(cur - (h - line).max(line)),
            VK_NEXT | VK_SPACE => Some(cur + (h - line).max(line)),
            VK_HOME => Some(0.0),
            VK_END => Some(f32::MAX),
            _ => None,
        };
        if let Some(y) = target {
            self.user_scroll(y);
            return true;
        }
        match vk {
            0x43 if ctrl => {
                self.copy();
                true
            }
            0x41 if ctrl => {
                self.select_all();
                true
            }
            VK_ESCAPE => {
                if let Some(app) = crate::app::app() {
                    app.md_escape();
                }
                true
            }
            _ => false,
        }
    }

    fn drag_to(&self, x: i32, y: i32) {
        let selecting = self.st.borrow().selecting;
        if !selecting {
            return;
        }
        let (_, h) = client(self.hwnd.get());
        if y < 0 || y as f32 > h {
            // 拖出窗口：自动滚动
            let cur = self.st.borrow().scroll;
            let d = if y < 0 { y as f32 } else { y as f32 - h };
            self.user_scroll(cur + d.clamp(-200.0, 200.0));
            unsafe { SetTimer(self.hwnd.get(), TIMER_AUTOSCROLL, 50, None) };
        } else {
            unsafe { KillTimer(self.hwnd.get(), TIMER_AUTOSCROLL) };
        }
        let yc = (y as f32).clamp(0.0, h - 1.0);
        if let Some(p) = self.pos_at(x as f32, yc) {
            let mut st = self.st.borrow_mut();
            if let Some((a, b)) = st.sel {
                if b != p {
                    st.sel = Some((a, p));
                    drop(st);
                    self.refresh();
                }
            }
        }
    }

    // ---- 命中测试 ----

    /// 文字块中文字的左上角（文档坐标）。
    fn text_origin(l: &Laid, kind: TextKind, em: f32) -> (f32, f32) {
        match kind {
            TextKind::Code | TextKind::Html => (l.x + em * 0.8, l.y + em * 0.6),
            _ => (l.x, l.y),
        }
    }

    /// 窗口坐标处的选择位置。
    fn pos_at(&self, x: f32, y: f32) -> Option<Pos> {
        self.ensure_layout();
        let st = self.st.borrow();
        let em = st.fonts.as_ref()?.em();
        if st.laid.is_empty() {
            return None;
        }
        let dy = y + st.scroll;
        let n = st.laid.partition_point(|l| l.y <= dy);
        let i = n.saturating_sub(1);
        let l = &st.laid[i];
        // 在块下方的空隙里：算作块尾
        if dy > l.y + l.h {
            let off = match &*l.content {
                Content::Text(_, tb) => tb.wide.len() as u32,
                _ => 1,
            };
            return Some(Pos { block: i, off });
        }
        if n == 0 {
            return Some(Pos { block: 0, off: 0 });
        }
        Some(match &*l.content {
            Content::Text(kind, tb) => {
                let (ox, oy) = Self::text_origin(l, *kind, em);
                let (p, _) = tb.rt.hit(x - ox, dy - oy);
                Pos {
                    block: i,
                    off: p as u32,
                }
            }
            _ => Pos {
                block: i,
                off: (x > l.x + l.w / 2.0) as u32,
            },
        })
    }

    /// 窗口坐标下的链接地址。
    fn link_at(&self, x: f32, y: f32) -> Option<String> {
        let st = self.st.borrow();
        let em = st.fonts.as_ref()?.em();
        let dy = y + st.scroll;
        let n = st.laid.partition_point(|l| l.y <= dy);
        let l = st.laid.get(n.checked_sub(1)?)?;
        if dy > l.y + l.h || x < l.x || x > l.x + l.w {
            return None;
        }
        let idx = match &*l.content {
            Content::Text(kind, tb) => {
                let (ox, oy) = Self::text_origin(l, *kind, em);
                tb.link_at(tb.rt.char_at(x - ox, dy - oy)?)?
            }
            Content::Table(t) => {
                let mut ry = l.y;
                let mut found = None;
                for (r, rh) in t.rows.iter().enumerate() {
                    if dy >= ry && dy < ry + rh {
                        let mut cx = l.x;
                        for (c, cw) in t.cols.iter().enumerate() {
                            if x >= cx && x < cx + cw {
                                let tb = &t.cells[r][c];
                                let p = tb.rt.char_at(x - cx - t.pad_x, dy - ry - t.pad_y)?;
                                found = tb.link_at(p);
                            }
                            cx += cw;
                        }
                    }
                    ry += rh;
                }
                found?
            }
            _ => return None,
        };
        st.doc.links.get(idx as usize).cloned()
    }

    fn open_link(&self, url: &str) {
        let target = hn_md::classify_link(url);
        if let hn_md::LinkTarget::Anchor(a) = &target {
            let i = self.st.borrow().doc.find_anchor(a);
            match i {
                Some(i) => self.scroll_to_block(i),
                None => {
                    if let Some(app) = crate::app::app() {
                        app.md_message(&format!("找不到页内位置“#{a}”"));
                    }
                }
            }
            return;
        }
        let base = self.st.borrow().base.clone();
        if let Some(app) = crate::app::app() {
            app.md_open_link(target, base);
        }
    }

    // ---- 选择与复制 ----

    fn block_len(l: &Laid) -> u32 {
        match &*l.content {
            Content::Text(_, tb) => tb.wide.len() as u32,
            _ => 1,
        }
    }

    pub fn select_all(&self) {
        let mut st = self.st.borrow_mut();
        if let Some(last) = st.laid.last() {
            let end = Pos {
                block: st.laid.len() - 1,
                off: Self::block_len(last),
            };
            st.sel = Some((Pos::default(), end));
        }
        drop(st);
        self.refresh();
    }

    fn select_block_at(&self, x: f32, y: f32) {
        if let Some(p) = self.pos_at(x, y) {
            let mut st = self.st.borrow_mut();
            if let Some(l) = st.laid.get(p.block) {
                let len = Self::block_len(l);
                st.sel = Some((
                    Pos {
                        block: p.block,
                        off: 0,
                    },
                    Pos {
                        block: p.block,
                        off: len,
                    },
                ));
            }
            drop(st);
            self.refresh();
        }
    }

    /// 选中内容的纯文本（块之间换行）。
    pub fn selected_text(&self) -> String {
        let st = self.st.borrow();
        let Some((a, b)) = st.sel else {
            return String::new();
        };
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut parts: Vec<String> = Vec::new();
        for i in a.block..=b.block.min(st.laid.len().saturating_sub(1)) {
            let (Some(l), Some(block)) = (st.laid.get(i), st.doc.blocks.get(i)) else {
                continue;
            };
            let len = Self::block_len(l);
            let s = if i == a.block { a.off.min(len) } else { 0 };
            let e = if i == b.block { b.off.min(len) } else { len };
            if s >= e && !(s == 0 && e == 0 && len == 0) {
                continue;
            }
            let text = match &*l.content {
                Content::Text(_, tb) if !(s == 0 && e == len) => {
                    String::from_utf16_lossy(&tb.wide[s as usize..e as usize])
                }
                _ => block.plain_text(),
            };
            parts.push(text);
        }
        parts.join("\n").replace("\r\n", "\n").replace('\n', "\r\n")
    }

    fn copy(&self) {
        let text = self.selected_text();
        if text.is_empty() {
            return;
        }
        if let Some(app) = crate::app::app() {
            app.md_copy(&text);
        }
    }

    fn context_menu(&self, lp: LPARAM) {
        let (mut x, mut y) = point_from_lparam(lp);
        if lp == -1 {
            let mut pt = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut pt) };
            x = pt.x;
            y = pt.y;
        }
        let mut pt = POINT { x, y };
        unsafe { ScreenToClient(self.hwnd.get(), &mut pt) };
        let link = self.link_at(pt.x as f32, pt.y as f32);
        let has_sel = self.st.borrow().sel.is_some_and(|(a, b)| a != b);
        let m = unsafe { CreatePopupMenu() };
        let add = |flags: u32, id: usize, text: &str| {
            let w = wide(text);
            unsafe { AppendMenuW(m, flags, id, w.as_ptr()) };
        };
        add(
            if has_sel {
                MF_STRING
            } else {
                MF_STRING | MF_GRAYED_
            },
            CMD_COPY,
            "复制(&C)\tCtrl+C",
        );
        add(MF_STRING, CMD_SELECT_ALL, "全选(&A)\tCtrl+A");
        if link.is_some() {
            add(MF_STRING, CMD_COPY_LINK, "复制链接地址(&L)");
        }
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING, CMD_CLOSE, "关闭预览(&X)\tCtrl+Shift+M");
        let cmd = unsafe {
            TrackPopupMenu(
                m,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                x,
                y,
                0,
                self.hwnd.get(),
                null(),
            )
        } as usize;
        unsafe { DestroyMenu(m) };
        match cmd {
            CMD_COPY => self.copy(),
            CMD_SELECT_ALL => self.select_all(),
            CMD_COPY_LINK => {
                if let (Some(url), Some(app)) = (link, crate::app::app()) {
                    app.md_copy(&url);
                }
            }
            CMD_CLOSE => {
                if let Some(app) = crate::app::app() {
                    app.md_close();
                }
            }
            _ => {}
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
        self.ensure_layout();
        if self.surface.borrow().is_none() {
            let s = Surface::new(hwnd, w as u32, h as u32).ok()?;
            *self.surface.borrow_mut() = Some(s);
        }
        let surf = self.surface.borrow();
        let s = surf.as_ref()?;
        s.begin();
        self.draw(s, w, h);
        let res = s.end();
        drop(surf);
        if let Err(e) = res {
            if hn_render::needs_recreate(&e) {
                self.surface.borrow_mut().take();
                unsafe { InvalidateRect(hwnd, null(), 0) };
            }
        }
        Some(())
    }

    fn draw(&self, s: &Surface, w: f32, h: f32) {
        let p = &GRUVBOX_LIGHT;
        s.clear(p.bg0);
        let st = self.st.borrow();
        let Some(fonts) = st.fonts.clone() else {
            return;
        };
        let em = fonts.em();
        if let Some(msg) = &st.message {
            let wide: Vec<u16> = msg.encode_utf16().collect();
            if let Ok(rt) = RichText::new(&wide, &fonts.body, (w - 2.0 * em).max(10.0)) {
                s.rich(&rt, em, em, p.gray);
            }
            return;
        }
        if st.laid.is_empty() {
            let msg: Vec<u16> = "（空文档）".encode_utf16().collect();
            if let Ok(rt) = RichText::new(&msg, &fonts.body, (w - 2.0 * em).max(10.0)) {
                s.rich(&rt, em, em, p.gray);
            }
            return;
        }
        let top = st.scroll;
        let sel = st.sel.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let first = st.laid.partition_point(|l| l.y + l.h < top - em * 2.0);
        let blocks = &st.doc.blocks;
        for i in first..st.laid.len() {
            let l = &st.laid[i];
            if l.y - top > h + em * 2.0 {
                break;
            }
            let Some(b) = blocks.get(i) else { break };
            let y = l.y - top;
            // 引用竖线（与上一块同属引用时连起来）
            for q in 0..b.quote {
                let joined = i > 0 && blocks[i - 1].quote > q;
                let bx = st.pad_x + q as f32 * em * 1.1;
                let t = if joined { y - l.gap } else { y };
                s.fill(bx, t, bx + 3.0, y + l.h, p.bg2);
            }
            // 选区（整块）
            let block_sel = sel.and_then(|(a, z)| {
                if i < a.block || i > z.block {
                    return None;
                }
                let len = Self::block_len(l);
                let s0 = if i == a.block { a.off.min(len) } else { 0 };
                let e0 = if i == z.block { z.off.min(len) } else { len };
                (e0 > s0).then_some((s0, e0))
            });
            let text_color = if b.quote > 0 { p.fg3 } else { p.fg1 };
            match &*l.content {
                Content::Text(kind, tb) => {
                    let (ox, oy) = Self::text_origin(l, *kind, em);
                    let oy = oy - top;
                    if matches!(kind, TextKind::Code | TextKind::Html) {
                        s.fill_round(l.x, y, l.x + l.w, y + l.h, 4.0, p.bg0_s);
                        s.outline(
                            l.x + 0.5,
                            y + 0.5,
                            l.x + l.w - 0.5,
                            y + l.h - 0.5,
                            1.0,
                            p.bg1,
                        );
                    }
                    for &(a, z) in &tb.code {
                        for (rl, rt, rr, rb) in tb.rt.range_rects(a as usize, (z - a) as usize) {
                            s.fill_round(
                                ox + rl - 1.0,
                                oy + rt + 1.0,
                                ox + rr + 1.0,
                                oy + rb - 1.0,
                                3.0,
                                p.bg0_s,
                            );
                        }
                    }
                    if let Some((a, z)) = block_sel {
                        for (rl, rt, rr, rb) in tb.rt.range_rects(a as usize, (z - a) as usize) {
                            s.fill(ox + rl, oy + rt, ox + rr, oy + rb, p.bg2);
                        }
                    }
                    let c = match kind {
                        TextKind::Heading(_) => p.blue,
                        _ => text_color,
                    };
                    s.rich(&tb.rt, ox, oy, c);
                    if let TextKind::Heading(lv) = kind {
                        if *lv <= 2 {
                            let ly = y + l.h - 1.0;
                            s.fill(l.x, ly, l.x + l.w, ly + 1.0, p.bg2);
                        }
                    }
                }
                Content::Rule => {
                    let ly = (y + l.h / 2.0).round();
                    s.fill(l.x, ly - 1.0, l.x + l.w, ly + 1.0, p.bg2);
                }
                Content::Table(t) => {
                    if block_sel.is_some() {
                        let tw: f32 = t.cols.iter().sum();
                        s.fill(l.x, y, l.x + tw, y + l.h, p.bg2);
                    }
                    self.draw_table(s, t, l.x, y, text_color);
                }
                Content::Image { src, alt } => {
                    if block_sel.is_some() {
                        s.fill(l.x, y, l.x + l.w, y + l.h, p.bg2);
                    }
                    self.draw_image(s, &st, &fonts, src, alt, l, y);
                }
            }
            if let Some(m) = &l.marker {
                let my = match &*l.content {
                    Content::Text(kind, _) => Self::text_origin(l, *kind, em).1 - top,
                    _ => y,
                };
                let mx = l.x - m.width - em * 0.45;
                s.rich(m, mx, my, p.fg3);
            }
        }
    }

    fn draw_table(&self, s: &Surface, t: &TableBox, x: f32, y: f32, color: u32) {
        let p = &GRUVBOX_LIGHT;
        let tw: f32 = t.cols.iter().sum();
        let th: f32 = t.rows.iter().sum();
        if let Some(h0) = t.rows.first() {
            s.fill(x, y, x + tw, y + h0, p.bg0_s);
        }
        let mut ry = y;
        for (r, rh) in t.rows.iter().enumerate() {
            let mut cx = x;
            for (c, cw) in t.cols.iter().enumerate() {
                if let Some(tb) = t.cells.get(r).and_then(|row| row.get(c)) {
                    s.clip(cx, ry, cx + cw, ry + rh);
                    s.rich(&tb.rt, cx + t.pad_x, ry + t.pad_y, color);
                    s.unclip();
                }
                cx += cw;
            }
            ry += rh;
            s.fill(x, ry.round(), x + tw, ry.round() + 1.0, p.bg2);
        }
        // 外框与竖线
        s.fill(x, y, x + tw, y + 1.0, p.bg2);
        let mut cx = x;
        s.fill(cx, y, cx + 1.0, y + th + 1.0, p.bg2);
        for cw in &t.cols {
            cx += cw;
            s.fill(cx.round() - 1.0, y, cx.round(), y + th + 1.0, p.bg2);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_image(
        &self,
        s: &Surface,
        st: &State,
        fonts: &Fonts,
        src: &str,
        alt: &str,
        l: &Laid,
        y: f32,
    ) {
        let p = &GRUVBOX_LIGHT;
        let path = Self::image_path(st.base.as_deref(), src);
        let entry = path.as_ref().and_then(|x| st.images.get(x));
        let placeholder = |text: String| {
            s.fill_round(l.x, y, l.x + l.w, y + l.h, 4.0, p.bg0_s);
            let wide: Vec<u16> = text.encode_utf16().collect();
            if let Ok(rt) = RichText::new(&wide, &fonts.body, (l.w - 16.0).max(10.0)) {
                s.clip(l.x, y, l.x + l.w, y + l.h);
                s.rich(&rt, l.x + 8.0, y + (l.h - rt.height) / 2.0, p.gray);
                s.unclip();
            }
        };
        let name = if alt.is_empty() { src } else { alt };
        match entry.map(|e| &e.state) {
            Some(ImgState::Ready {
                w,
                h,
                pixels,
                orig,
                bmp,
            }) => {
                let scale = st.spec.as_ref().map(|x| x.px_scale).unwrap_or(1.0);
                let (iw, ih) = (orig.0 as f32 * scale, orig.1 as f32 * scale);
                let k = (l.w / iw).min(1.0);
                let (dw, dh) = (iw * k, ih * k);
                let mut b = bmp.borrow_mut();
                if !b.as_ref().is_some_and(|x| x.belongs_to(s)) {
                    *b = s.create_bitmap(*w, *h, pixels).ok();
                }
                match b.as_ref() {
                    Some(bm) => s.bitmap(bm, l.x, y, l.x + dw, y + dh),
                    None => placeholder(format!("[图片无法显示：{name}]")),
                }
            }
            Some(ImgState::Failed(e)) => placeholder(format!("[图片无法显示：{name}（{e}）]")),
            Some(ImgState::Loading) => placeholder(format!("[正在载入图片：{name}]")),
            None if path.is_none() => placeholder(format!("[无法定位图片：{name}]")),
            None => placeholder(format!("[图片：{name}]")),
        }
    }

    // ---- 自测接口 ----

    /// (块数, 排版块数, 内容总高, 最近一次重建块数)
    pub fn st_stats(&self) -> (usize, usize, f32, usize) {
        self.ensure_layout();
        let st = self.st.borrow();
        (
            st.doc.blocks.len(),
            st.laid.len(),
            st.total_h,
            self.rebuilt.get(),
        )
    }

    pub fn st_scroll(&self) -> f32 {
        self.st.borrow().scroll
    }

    pub fn st_set_scroll(&self, y: f32) {
        self.user_scroll(y);
    }

    pub fn st_select_all_text(&self) -> String {
        self.select_all();
        self.selected_text()
    }

    /// 第 `i` 个块中第一个链接所在点的窗口坐标。
    pub fn st_link_point(&self, i: usize) -> Option<(f32, f32)> {
        self.ensure_layout();
        let st = self.st.borrow();
        let em = st.fonts.as_ref()?.em();
        let l = st.laid.get(i)?;
        let Content::Text(kind, tb) = &*l.content else {
            return None;
        };
        let (a, z, _) = *tb.links.first()?;
        let (ox, oy) = Self::text_origin(l, *kind, em);
        let (rl, rt, rr, rb) = *tb.rt.range_rects(a as usize, (z - a) as usize).first()?;
        Some((ox + (rl + rr) / 2.0, oy + (rt + rb) / 2.0 - st.scroll))
    }

    pub fn st_link_at(&self, x: f32, y: f32) -> Option<String> {
        self.link_at(x, y)
    }

    /// 图片状态：(就绪数, 失败数, 载入中数)
    pub fn st_images(&self) -> (usize, usize, usize) {
        let st = self.st.borrow();
        let mut r = (0, 0, 0);
        for e in st.images.values() {
            match e.state {
                ImgState::Ready { .. } => r.0 += 1,
                ImgState::Failed(_) => r.1 += 1,
                ImgState::Loading => r.2 += 1,
            }
        }
        r
    }

    /// 处理已到达的后台结果（自测等待用）。
    pub fn st_pump(&self) {
        self.on_app_msg(0);
        self.on_app_msg(1);
    }

    pub fn st_y_for_line(&self, line: f64) -> f32 {
        self.ensure_layout();
        Self::y_for_line(&self.st.borrow(), line)
    }

    pub fn st_line_for_y(&self, y: f32) -> f64 {
        self.ensure_layout();
        Self::line_for_y(&self.st.borrow(), y)
    }

    pub fn st_paint(&self) -> bool {
        self.render().is_some()
    }

    pub fn st_has_focus(&self) -> bool {
        unsafe { GetFocus() == self.hwnd.get() }
    }
}
