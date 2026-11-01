//! 终端窗口：`hn_term::Terminal`（ConPTY + VT 状态机）+ Direct2D 网格绘制。
//!
//! - 每个终端一个子窗口，放在底部抽屉的页面位置；窗口与终端状态登记在线程局部表中
//! - 后台线程（PTY 读取）只通过回调把事件放进队列，再 `PostMessage(WM_APP_TERM)` 给本窗口；
//!   `pending` 标志合并重复的唤醒，避免输出很多时消息队列被灌满
//! - 绘制时在锁内复制可见区域（[`hn_term::Snapshot`]），释放锁后再画
//! - 键盘：方向键、功能键等在 `WM_KEYDOWN` 中按 xterm 约定编码；其余字符走 `WM_CHAR`
//!   （Ctrl+字母已由系统转为控制字符；Backspace 发 DEL，Ctrl+Backspace 发 BS）；Alt+字符发 ESC 前缀
//! - 输入法：组字窗口跟随光标；上屏的文字以 `WM_CHAR` 到达
//! - 复制粘贴：有选区时 Ctrl+C 复制，否则发给程序（中断）；Ctrl+V / Shift+Insert / 右键粘贴

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hn_render::{Font, Surface};
use hn_term::alacritty_terminal::term::TermMode;
use hn_term::keys::{self, Key, Mods};
use hn_term::palette::Palette;
use hn_term::{Cell as TCell, Cursor, Size, TermEvent, Terminal};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetMessageTime,
    GetWindowLongPtrW, LoadCursorW, PostMessageW, RegisterClassExW, SetWindowLongPtrW, WNDCLASSEXW,
};

use crate::win::*;

const CLASS: &str = "HurshNoteTerminal";
const GWLP_USERDATA: i32 = -21;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const IDC_IBEAM: u16 = 32513;
/// 后台线程唤醒终端窗口
pub const WM_APP_TERM: u32 = WM_APP + 20;

const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSCHAR: u32 = 0x0106;
const WM_IME_STARTCOMPOSITION: u32 = 0x010D;
const WM_IME_COMPOSITION: u32 = 0x010F;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_CAPTURECHANGED: u32 = 0x0215;
const WM_GETDLGCODE: u32 = 0x0087;
const DLGC_WANTALLKEYS: isize = 0x0004;

const VK_BACK: usize = 0x08;
const VK_TAB: usize = 0x09;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_SPACE: usize = 0x20;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_END: usize = 0x23;
const VK_HOME: usize = 0x24;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const VK_INSERT: usize = 0x2D;
const VK_DELETE: usize = 0x2E;
const VK_F1: usize = 0x70;
const VK_F4: usize = 0x73;
const VK_F12: usize = 0x7B;
const CF_UNICODETEXT: u32 = 13;
const CFS_POINT: u32 = 0x0002;

#[repr(C)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
struct COMPOSITIONFORM {
    dwStyle: u32,
    ptCurrentPos: POINT,
    rcArea: RECT,
}

#[link(name = "imm32")]
extern "system" {
    fn ImmGetContext(hwnd: HWND) -> isize;
    fn ImmReleaseContext(hwnd: HWND, himc: isize) -> i32;
    fn ImmSetCompositionWindow(himc: isize, form: *const COMPOSITIONFORM) -> i32;
}

extern "system" {
    fn GlobalLock(h: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    fn GlobalUnlock(h: *mut core::ffi::c_void) -> i32;
}

fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) < 0 }
}

/// 后台线程与 UI 线程共享的状态。
struct Shared {
    /// 已投递唤醒消息、尚未处理
    pending: AtomicBool,
    events: Mutex<Vec<TermEvent>>,
    exited: AtomicBool,
}

struct Fonts {
    normal: Font,
    bold: Font,
    italic: Font,
    /// 格子宽高（像素）
    cw: f32,
    ch: f32,
}

/// 一个终端窗口。
pub struct TermView {
    id: u32,
    hwnd: HWND,
    term: Terminal,
    shared: Arc<Shared>,
    label: String,
    /// shell 种类（拼命令行用）：powershell / pwsh / cmd / other
    pub kind: &'static str,
    title: RefCell<String>,
    surface: RefCell<Option<Surface>>,
    fonts: RefCell<Option<Rc<Fonts>>>,
    font_key: RefCell<(String, u32)>,
    pad: Cell<f32>,
    selecting: Cell<bool>,
    sel_moved: Cell<bool>,
    /// (连击次数, 时间, x, y)
    clicks: Cell<(u32, u32, i32, i32)>,
    high_surrogate: Cell<u16>,
    /// 丢弃紧随其后的一个 WM_CHAR（已在 WM_KEYDOWN 中处理的按键）
    skip_char: Cell<bool>,
    wheel_accum: Cell<i32>,
}

thread_local! {
    static TERMS: RefCell<Vec<Rc<TermView>>> = const { RefCell::new(Vec::new()) };
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };
}

/// 启动参数。
pub struct Spawn {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// 页签上显示的名称
    pub label: String,
    pub kind: &'static str,
    pub face: String,
    pub px: f32,
    /// 预计的客户区像素尺寸（用来算初始行列数）
    pub width: i32,
    pub height: i32,
}

pub fn view(id: u32) -> Option<Rc<TermView>> {
    TERMS.with(|t| t.borrow().iter().find(|v| v.id == id).cloned())
}

pub fn ids() -> Vec<u32> {
    TERMS.with(|t| t.borrow().iter().map(|v| v.id).collect())
}

pub fn hwnd(id: u32) -> Option<HWND> {
    view(id).map(|v| v.hwnd)
}

pub fn title(id: u32) -> Option<String> {
    view(id).map(|v| v.label.clone())
}

/// 窗口是否为终端（或其子窗口）。
pub fn id_of(h: HWND) -> Option<u32> {
    TERMS.with(|t| t.borrow().iter().find(|v| v.hwnd == h).map(|v| v.id))
}

/// 关闭终端：结束 shell、销毁窗口。
pub fn close(id: u32) {
    let v = TERMS.with(|t| {
        let mut t = t.borrow_mut();
        let i = t.iter().position(|v| v.id == id)?;
        Some(t.remove(i))
    });
    if let Some(v) = v {
        v.term.shutdown();
        unsafe { DestroyWindow(v.hwnd) };
    }
}

/// 所有终端改用新字体（DPI 或字体设置变化）。
pub fn set_font_all(face: &str, px: f32) {
    for id in ids() {
        if let Some(v) = view(id) {
            v.set_font(face, px);
        }
    }
}

pub fn write(id: u32, bytes: Vec<u8>) {
    if let Some(v) = view(id) {
        v.term.scroll_to_bottom();
        v.term.write(bytes);
    }
}

fn client(hwnd: HWND) -> (f32, f32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    ((rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32)
}

fn make_fonts(face: &str, px: f32) -> Option<Fonts> {
    let normal = Font::new(face, px, 8).ok()?;
    let bold = Font::styled(face, px, 8, true, false).ok()?;
    let italic = Font::styled(face, px, 8, false, true).ok()?;
    let (cw, ch) = (normal.char_w, normal.line_h);
    Some(Fonts {
        normal,
        bold,
        italic,
        cw,
        ch,
    })
}

fn grid_size(w: f32, h: f32, pad: f32, f: &Fonts) -> Size {
    Size {
        cols: (((w - 2.0 * pad) / f.cw).floor() as usize).max(2),
        rows: (((h - 2.0 * pad) / f.ch).floor() as usize).max(1),
    }
}

/// 创建终端窗口（隐藏）并启动 shell，返回终端 id。
pub fn create(parent: HWND, hinstance: *mut core::ffi::c_void, sp: Spawn) -> Result<u32, String> {
    let class = wide(CLASS);
    if !REGISTERED.with(|r| r.replace(true)) {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
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
    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            null(),
            WS_CHILD | WS_CLIPSIBLINGS | WS_TABSTOP,
            0,
            0,
            0,
            0,
            parent,
            (300 + id as usize) as *mut core::ffi::c_void,
            hinstance,
            null(),
        )
    };
    if hwnd.is_null() {
        return Err("无法创建终端窗口".into());
    }
    let fonts = make_fonts(&sp.face, sp.px).ok_or_else(|| {
        unsafe { DestroyWindow(hwnd) };
        "无法创建终端字体".to_string()
    })?;
    let pad = (sp.px / 3.0).round().max(2.0);
    let size = grid_size(sp.width as f32, sp.height as f32, pad, &fonts);
    let shared = Arc::new(Shared {
        pending: AtomicBool::new(false),
        events: Mutex::new(Vec::new()),
        exited: AtomicBool::new(false),
    });
    let sh = shared.clone();
    let target = hwnd as isize;
    let notify = Arc::new(move |e: TermEvent| {
        match e {
            TermEvent::Wakeup => {}
            TermEvent::Exit(_) => {
                sh.exited.store(true, Ordering::SeqCst);
                sh.events.lock().unwrap_or_else(|e| e.into_inner()).push(e);
            }
            other => sh
                .events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(other),
        }
        if !sh.pending.swap(true, Ordering::SeqCst) {
            unsafe { PostMessageW(target as HWND, WM_APP_TERM, 0, 0) };
        }
    });
    let opts = hn_term::Options {
        program: sp.program.clone(),
        args: sp.args.clone(),
        cwd: sp.cwd.clone(),
        env: Default::default(),
        size: Some(size),
        cell: (fonts.cw.round() as u16, fonts.ch.round() as u16),
    };
    let term = match Terminal::spawn(&opts, notify) {
        Ok(t) => t,
        Err(e) => {
            unsafe { DestroyWindow(hwnd) };
            return Err(format!("无法启动 {}：{e}", sp.program));
        }
    };
    let v = Rc::new(TermView {
        id,
        hwnd,
        term,
        shared,
        label: sp.label,
        kind: sp.kind,
        title: RefCell::new(String::new()),
        surface: RefCell::new(None),
        fonts: RefCell::new(Some(Rc::new(fonts))),
        font_key: RefCell::new((sp.face, (sp.px * 100.0) as u32)),
        pad: Cell::new(pad),
        selecting: Cell::new(false),
        sel_moved: Cell::new(false),
        clicks: Cell::new((0, 0, 0, 0)),
        high_surrogate: Cell::new(0),
        skip_char: Cell::new(false),
        wheel_accum: Cell::new(0),
    });
    TERMS.with(|t| t.borrow_mut().push(v));
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, id as isize) };
    Ok(id)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let id = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if id > 0 {
        let r = crate::crash::guard(|| view(id as u32).and_then(|v| v.handle(msg, wp, lp)));
        if let Some(Some(r)) = r {
            return r;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn utf8_of(units: &[u16]) -> Vec<u8> {
    String::from_utf16_lossy(units).into_bytes()
}

impl TermView {
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn terminal(&self) -> &Terminal {
        &self.term
    }

    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::SeqCst)
    }

    /// 可见区域的全部文字（自测用）。
    pub fn text(&self) -> String {
        let s = self.term.snapshot();
        (0..s.size.rows)
            .map(|r| s.row_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn fonts(&self) -> Option<Rc<Fonts>> {
        if let Some(f) = self.fonts.borrow().as_ref() {
            return Some(f.clone());
        }
        let (face, px100) = self.font_key.borrow().clone();
        let f = Rc::new(make_fonts(&face, px100 as f32 / 100.0)?);
        *self.fonts.borrow_mut() = Some(f.clone());
        Some(f)
    }

    pub fn set_font(&self, face: &str, px: f32) {
        let key = (face.to_string(), (px * 100.0) as u32);
        if *self.font_key.borrow() == key {
            return;
        }
        *self.font_key.borrow_mut() = key;
        self.fonts.borrow_mut().take();
        self.pad.set((px / 3.0).round().max(2.0));
        self.update_size();
        self.invalidate();
    }

    fn invalidate(&self) {
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }

    fn update_size(&self) {
        let (w, h) = client(self.hwnd);
        if w < 1.0 || h < 1.0 {
            return;
        }
        if let Some(f) = self.fonts() {
            let size = grid_size(w, h, self.pad.get(), &f);
            self.term
                .resize(size, f.cw.round() as u16, f.ch.round() as u16);
        }
    }

    fn handle(&self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
                unsafe { BeginPaint(self.hwnd, &mut ps) };
                self.render();
                unsafe { EndPaint(self.hwnd, &ps) };
                Some(0)
            }
            WM_ERASEBKGND => Some(1),
            WM_SIZE => {
                let (w, h) = (loword(lp as usize) as u32, hiword(lp as usize) as u32);
                if let Some(s) = self.surface.borrow().as_ref() {
                    s.resize(w, h);
                }
                self.update_size();
                self.invalidate();
                Some(0)
            }
            WM_APP_TERM => {
                self.on_wakeup();
                Some(0)
            }
            WM_GETDLGCODE => Some(DLGC_WANTALLKEYS),
            WM_KEYDOWN => self.on_keydown(wp, false).then_some(0),
            WM_SYSKEYDOWN => {
                if wp == VK_F4 || wp == VK_MENU as usize {
                    return None;
                }
                self.on_keydown(wp, true).then_some(0)
            }
            WM_CHAR => {
                self.on_char(wp as u16, false);
                Some(0)
            }
            WM_SYSCHAR => {
                if wp == VK_SPACE {
                    return None; // Alt+空格：系统菜单
                }
                self.on_char(wp as u16, true);
                Some(0)
            }
            WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION => {
                self.place_ime();
                None
            }
            WM_MOUSEWHEEL => {
                self.on_wheel(hiword(wp) as i16 as i32);
                Some(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_from_lparam(lp);
                self.on_ldown(x, y);
                Some(0)
            }
            WM_MOUSEMOVE => {
                if self.selecting.get() {
                    let (x, y) = point_from_lparam(lp);
                    let (r, c, right) = self.hit(x, y);
                    self.term.select_update(r, c, right);
                    self.sel_moved.set(true);
                    self.invalidate();
                }
                Some(0)
            }
            WM_LBUTTONUP => {
                if self.selecting.replace(false) {
                    unsafe { ReleaseCapture() };
                    if !self.sel_moved.get() {
                        self.term.clear_selection();
                        self.invalidate();
                    }
                }
                Some(0)
            }
            WM_CAPTURECHANGED => {
                self.selecting.set(false);
                Some(0)
            }
            WM_RBUTTONUP => {
                unsafe { SetFocus(self.hwnd) };
                if !self.copy_selection() {
                    self.paste();
                }
                Some(0)
            }
            WM_MBUTTONUP => {
                self.paste();
                Some(0)
            }
            WM_SETFOCUS | WM_KILLFOCUS => {
                self.invalidate();
                Some(0)
            }
            _ => None,
        }
    }

    fn on_wakeup(&self) {
        self.shared.pending.store(false, Ordering::SeqCst);
        let events: Vec<TermEvent> =
            std::mem::take(&mut *self.shared.events.lock().unwrap_or_else(|e| e.into_inner()));
        let mut exited = false;
        for e in events {
            match e {
                TermEvent::Title(t) => *self.title.borrow_mut() = t,
                TermEvent::Clipboard(s) => {
                    if let Some(app) = crate::app::app() {
                        app.term_copy(&s);
                    }
                }
                TermEvent::Exit(_) => exited = true,
                TermEvent::Bell | TermEvent::Wakeup => {}
            }
        }
        self.invalidate();
        if exited {
            if let Some(app) = crate::app::app() {
                app.term_exited(self.id);
            }
        }
    }

    // ---- 键盘 ----

    fn send_input(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.term.scroll_to_bottom();
        self.term.clear_selection();
        self.term.write(bytes);
        self.invalidate();
    }

    /// 返回是否已处理（未处理的按键交给 DefWindowProc，随后的 WM_CHAR 再处理）。
    fn on_keydown(&self, vk: WPARAM, alt: bool) -> bool {
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        let mods = Mods { shift, alt, ctrl };
        let key = match vk {
            VK_UP => Some(Key::Up),
            VK_DOWN => Some(Key::Down),
            VK_LEFT => Some(Key::Left),
            VK_RIGHT => Some(Key::Right),
            VK_HOME => Some(Key::Home),
            VK_END => Some(Key::End),
            VK_DELETE => Some(Key::Delete),
            VK_F1..=VK_F12 => Some(Key::F((vk - VK_F1 + 1) as u8)),
            _ => None,
        };
        // 回看与复制粘贴
        match vk {
            VK_PRIOR | VK_NEXT if shift && !ctrl => {
                let rows = self.term.size().rows as i32;
                let page = (rows - 1).max(1);
                self.term.scroll(if vk == VK_PRIOR { page } else { -page });
                self.invalidate();
                return true;
            }
            VK_INSERT if shift && !ctrl => {
                self.paste();
                return true;
            }
            VK_INSERT if ctrl && !shift => {
                self.copy_selection();
                return true;
            }
            0x43 if ctrl && !alt && (shift || self.term.selection_text().is_some()) => {
                // Ctrl+C（有选区）/ Ctrl+Shift+C：复制
                self.copy_selection();
                self.skip_char.set(true);
                return true;
            }
            0x56 if ctrl && !alt => {
                // Ctrl+V / Ctrl+Shift+V：粘贴
                self.paste();
                self.skip_char.set(true);
                return true;
            }
            VK_TAB if shift && !ctrl => {
                self.send_input(b"\x1b[Z".to_vec());
                self.skip_char.set(true);
                return true;
            }
            VK_SPACE if ctrl => {
                self.send_input(vec![0]);
                self.skip_char.set(true);
                return true;
            }
            VK_BACK if alt => {
                self.send_input(b"\x1b\x7f".to_vec());
                self.skip_char.set(true);
                return true;
            }
            _ => {}
        }
        let key = key.or(match vk {
            VK_PRIOR => Some(Key::PageUp),
            VK_NEXT => Some(Key::PageDown),
            VK_INSERT => Some(Key::Insert),
            _ => None,
        });
        let Some(key) = key else { return false };
        let app_cursor = self.term.mode().contains(TermMode::APP_CURSOR);
        if let Some(b) = keys::bytes(key, mods, app_cursor) {
            self.send_input(b);
        }
        true
    }

    fn on_char(&self, unit: u16, alt: bool) {
        if self.skip_char.replace(false) {
            return;
        }
        let mut units: Vec<u16> = Vec::with_capacity(2);
        match unit {
            0xD800..=0xDBFF => {
                self.high_surrogate.set(unit);
                return;
            }
            0xDC00..=0xDFFF => {
                let hi = self.high_surrogate.replace(0);
                if hi == 0 {
                    return;
                }
                units.push(hi);
                units.push(unit);
            }
            _ => units.push(unit),
        }
        let mut bytes = Vec::with_capacity(8);
        if alt {
            bytes.push(0x1b);
        }
        match unit {
            0x08 => bytes.push(0x7f),
            0x7f => bytes.push(0x08),
            _ => bytes.extend(utf8_of(&units)),
        }
        self.send_input(bytes);
    }

    // ---- 剪贴板 ----

    /// 复制选区；没有选区时返回 false。
    fn copy_selection(&self) -> bool {
        let Some(text) = self.term.selection_text() else {
            return false;
        };
        if let Some(app) = crate::app::app() {
            app.term_copy(&text.replace('\n', "\r\n"));
        }
        self.term.clear_selection();
        self.invalidate();
        true
    }

    fn paste(&self) {
        let Some(text) = clipboard_text(self.hwnd) else {
            return;
        };
        let bracketed = self.term.mode().contains(TermMode::BRACKETED_PASTE);
        self.send_input(keys::paste(&text, bracketed));
    }

    // ---- 鼠标 ----

    /// 坐标 → (行, 列, 是否在格子右半边)。
    fn hit(&self, x: i32, y: i32) -> (usize, usize, bool) {
        let Some(f) = self.fonts() else {
            return (0, 0, false);
        };
        let size = self.term.size();
        let pad = self.pad.get();
        let fx = ((x as f32 - pad) / f.cw).max(0.0);
        let fy = ((y as f32 - pad) / f.ch).max(0.0);
        let col = (fx.floor() as usize).min(size.cols.saturating_sub(1));
        let row = (fy.floor() as usize).min(size.rows.saturating_sub(1));
        (row, col, fx.fract() > 0.5)
    }

    fn on_ldown(&self, x: i32, y: i32) {
        unsafe {
            SetFocus(self.hwnd);
            SetCapture(self.hwnd);
        }
        let now = unsafe { GetMessageTime() } as u32;
        let (n, t, px, py) = self.clicks.get();
        let near = (x - px).abs() <= 4 && (y - py).abs() <= 4;
        let n = if near && now.wrapping_sub(t) <= unsafe { GetDoubleClickTime() } {
            n % 3 + 1
        } else {
            1
        };
        self.clicks.set((n, now, x, y));
        let (r, c, right) = self.hit(x, y);
        self.term.select_start(r, c, right, (n - 1) as u8);
        self.selecting.set(true);
        self.sel_moved.set(n > 1);
        self.invalidate();
    }

    fn on_wheel(&self, delta: i32) {
        let acc = self.wheel_accum.get() + delta;
        let lines = acc * 3 / WHEEL_DELTA;
        self.wheel_accum.set(acc - lines * WHEEL_DELTA / 3);
        if lines == 0 {
            return;
        }
        let mode = self.term.mode();
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // 全屏程序（vim、less）：滚轮换成方向键
            let key = if lines > 0 { Key::Up } else { Key::Down };
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            let mut b = Vec::new();
            for _ in 0..lines.unsigned_abs() {
                b.extend(keys::bytes(key, Mods::default(), app_cursor).unwrap_or_default());
            }
            self.term.write(b);
        } else {
            self.term.scroll(lines);
            self.invalidate();
        }
    }

    /// 把输入法组字窗口放到光标处。
    fn place_ime(&self) {
        let Some(f) = self.fonts() else { return };
        let snap = self.term.snapshot();
        let (r, c) = snap.cursor.unwrap_or((0, 0));
        let pad = self.pad.get();
        let form = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT {
                x: (pad + c as f32 * f.cw) as i32,
                y: (pad + r as f32 * f.ch) as i32,
            },
            rcArea: unsafe { std::mem::zeroed() },
        };
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc != 0 {
                ImmSetCompositionWindow(himc, &form);
                ImmReleaseContext(self.hwnd, himc);
            }
        }
    }

    // ---- 绘制 ----

    fn render(&self) -> Option<()> {
        let hwnd = self.hwnd;
        let (w, h) = client(hwnd);
        let f = self.fonts()?;
        if self.surface.borrow().is_none() {
            let s = Surface::new(hwnd, w as u32, h as u32).ok()?;
            *self.surface.borrow_mut() = Some(s);
        }
        let snap = self.term.snapshot();
        let pal = Palette::default();
        let focused = unsafe { GetFocus() } == hwnd;
        let surf = self.surface.borrow();
        let s = surf.as_ref()?;
        let pad = self.pad.get();
        let (cw, ch) = (f.cw, f.ch);
        let cols = snap.size.cols;
        let x_of = |c: usize| pad + c as f32 * cw;
        let bg_of = |c: &TCell| {
            if c.flags & TCell::SELECTED != 0 {
                pal.selection
            } else {
                c.bg
            }
        };
        let font_of = |flags: u8| {
            if flags & TCell::BOLD != 0 {
                &f.bold
            } else if flags & TCell::ITALIC != 0 {
                &f.italic
            } else {
                &f.normal
            }
        };
        s.begin();
        s.clear(pal.bg);
        let mut buf: Vec<u16> = Vec::with_capacity(cols);
        for row in 0..snap.size.rows {
            let y = pad + row as f32 * ch;
            // 背景：同色的连续格子合并成一块
            let mut col = 0;
            while col < cols {
                let bg = bg_of(snap.cell(row, col));
                let start = col;
                while col < cols && bg_of(snap.cell(row, col)) == bg {
                    col += 1;
                }
                if bg != pal.bg {
                    let right = if col == cols { w } else { x_of(col) };
                    s.fill(x_of(start), y, right, y + ch, bg);
                }
            }
            // 文字：ASCII 按同样式连续绘制（等宽字体，前进宽度与格子一致）；
            // 其他字符逐个画在自己的格子里，避免回退字体的宽度把后面的字符挤歪
            let mut col = 0;
            while col < cols {
                let c = *snap.cell(row, col);
                if c.flags & TCell::SPACER != 0 {
                    col += 1;
                    continue;
                }
                let deco = c.flags & (TCell::UNDERLINE | TCell::STRIKE);
                let style = c.flags & (TCell::BOLD | TCell::ITALIC);
                let start = col;
                if c.c.is_ascii() && c.flags & TCell::WIDE == 0 {
                    buf.clear();
                    while col < cols {
                        let d = snap.cell(row, col);
                        if !d.c.is_ascii()
                            || d.flags & (TCell::WIDE | TCell::SPACER) != 0
                            || d.fg != c.fg
                            || d.flags & (TCell::BOLD | TCell::ITALIC) != style
                            || d.flags & (TCell::UNDERLINE | TCell::STRIKE) != deco
                        {
                            break;
                        }
                        buf.push(if d.c == '\0' { b' ' as u16 } else { d.c as u16 });
                        col += 1;
                    }
                    while buf.last() == Some(&(b' ' as u16)) && deco == 0 {
                        buf.pop();
                    }
                    if !buf.is_empty() {
                        s.text(
                            &buf,
                            font_of(style),
                            x_of(start),
                            y,
                            x_of(col) + cw,
                            y + ch,
                            c.fg,
                        );
                    }
                } else {
                    let n = if c.flags & TCell::WIDE != 0 { 2 } else { 1 };
                    let mut tmp = [0u16; 2];
                    let units = c.c.encode_utf16(&mut tmp);
                    s.text(
                        units,
                        font_of(style),
                        x_of(start),
                        y,
                        x_of(start + n) + cw * 0.5,
                        y + ch,
                        c.fg,
                    );
                    col += n;
                }
                if deco & TCell::UNDERLINE != 0 {
                    let uy = (y + ch - 1.5).floor() + 0.5;
                    s.line(x_of(start), uy, x_of(col), uy, 1.0, c.fg);
                }
                if deco & TCell::STRIKE != 0 {
                    let sy = (y + ch / 2.0).floor() + 0.5;
                    s.line(x_of(start), sy, x_of(col), sy, 1.0, c.fg);
                }
            }
        }
        // 光标
        if let Some((r, c)) = snap.cursor.filter(|_| snap.cursor_shape != Cursor::Hidden) {
            let cell = *snap.cell(r, c);
            let n = if cell.flags & TCell::WIDE != 0 { 2 } else { 1 };
            let (l, t) = (x_of(c), pad + r as f32 * ch);
            let (rr, b) = (x_of(c + n), t + ch);
            let thick = (ch / 10.0).round().max(2.0);
            if !focused {
                s.line(l + 0.5, t + 0.5, rr - 0.5, t + 0.5, 1.0, pal.cursor);
                s.line(l + 0.5, b - 0.5, rr - 0.5, b - 0.5, 1.0, pal.cursor);
                s.line(l + 0.5, t + 0.5, l + 0.5, b - 0.5, 1.0, pal.cursor);
                s.line(rr - 0.5, t + 0.5, rr - 0.5, b - 0.5, 1.0, pal.cursor);
            } else {
                match snap.cursor_shape {
                    Cursor::Beam => s.fill(l, t, l + thick, b, pal.cursor),
                    Cursor::Underline => s.fill(l, b - thick, rr, b, pal.cursor),
                    _ => {
                        s.fill(l, t, rr, b, pal.cursor);
                        if cell.c != ' ' && cell.c != '\0' {
                            let mut tmp = [0u16; 2];
                            let units = cell.c.encode_utf16(&mut tmp);
                            let style = cell.flags & (TCell::BOLD | TCell::ITALIC);
                            s.text(units, font_of(style), l, t, rr + cw, b, pal.bg);
                        }
                    }
                }
            }
        }
        let res = s.end();
        drop(surf);
        if let Err(e) = res {
            if hn_render::needs_recreate(&e) {
                self.surface.borrow_mut().take();
                self.invalidate();
            }
        }
        Some(())
    }
}

fn clipboard_text(owner: HWND) -> Option<String> {
    unsafe {
        if OpenClipboard(owner) == 0 {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let mut out = None;
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let mut n = 0usize;
                while *p.add(n) != 0 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}
