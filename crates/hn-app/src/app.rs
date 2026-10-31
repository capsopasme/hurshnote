//! 主窗口与应用状态。
//!
//! 线程规则：本模块的一切都只在 UI 线程运行。
//!
//! 状态借用规则：`RefCell` 的借用不得跨越任何可能重入窗口过程的调用
//! （Scintilla 消息会同步发出 WM_NOTIFY、消息框和对话框会运行消息循环）。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::rc::Rc;

use hn_sci::*;
use hn_text::eol::{self, Eol};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, UpdateWindow};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyAcceleratorTable, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetMessageW, LoadCursorW, LoadIconW, MoveWindow, PostMessageW,
    PostQuitMessage, RegisterClassExW, SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu,
    TranslateAcceleratorW, TranslateMessage, MSG, WNDCLASSEXW,
};

use crate::dialogs::{self, Answer};
use crate::menu::*;
use crate::selftest;
use crate::win::*;

const CLASS_NAME: &str = "HurshNoteMain";
const ID_EDITOR: usize = 100;
pub const WM_APP_SELFTEST: u32 = WM_APP + 1;

/// 大于此大小的文件暂不打开（大文件视图在 M4 实现）。
const MAX_OPEN_SIZE: u64 = 256 * 1024 * 1024;

/// 当前文档的元数据。
#[derive(Debug, Default)]
struct DocState {
    path: Option<PathBuf>,
    /// 原文件带 UTF-8 BOM，保存时保持。
    bom: bool,
}

pub struct App {
    hinstance: *mut core::ffi::c_void,
    hwnd: Cell<HWND>,
    sci: Cell<Option<Sci>>,
    doc: RefCell<DocState>,
    selftest_out: Option<PathBuf>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn app() -> Option<Rc<App>> {
    APP.with(|a| a.borrow().clone())
}

struct Args {
    file: Option<PathBuf>,
    selftest_out: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args {
        file: None,
        selftest_out: None,
    };
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        if a == "--selftest" {
            args.selftest_out = it.next().map(PathBuf::from);
        } else if a.to_string_lossy().starts_with('-') {
            // 其他命令行参数在 M1 实现。
        } else if args.file.is_none() {
            args.file = Some(PathBuf::from(a));
        }
    }
    args
}

/// 程序主流程，返回进程退出码。
pub fn run() -> i32 {
    crate::crash::install_panic_hook();
    let args = parse_args();

    unsafe {
        let hinstance = GetModuleHandleW(null());
        if !hn_sci::register_classes(hinstance) {
            dialogs::error_box(null_mut(), "无法初始化编辑器组件（Scintilla）。");
            return 1;
        }

        let class_name = wide(CLASS_NAME);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: LoadIconW(hinstance, 1 as _),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW as usize as _),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            dialogs::error_box(null_mut(), "无法注册主窗口类。");
            return 1;
        }

        let app = Rc::new(App {
            hinstance,
            hwnd: Cell::new(null_mut()),
            sci: Cell::new(None),
            doc: RefCell::new(DocState::default()),
            selftest_out: args.selftest_out.clone(),
        });
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));

        let title = wide("HurshNote");
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            null_mut(),
            build_main_menu(),
            hinstance,
            null(),
        );
        if hwnd.is_null() {
            dialogs::error_box(null_mut(), "无法创建主窗口。");
            return 1;
        }

        if let Some(path) = &args.file {
            app.open_path(path);
        }
        app.update_title();

        ShowWindow(hwnd, SW_SHOWDEFAULT);
        UpdateWindow(hwnd);

        if app.selftest_out.is_some() {
            PostMessageW(hwnd, WM_APP_SELFTEST, 0, 0);
        }

        let accel = build_accelerators();
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if TranslateAcceleratorW(app.hwnd.get(), accel, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        DestroyAcceleratorTable(accel);

        APP.with(|a| *a.borrow_mut() = None);
        hn_sci::release_resources();
        msg.wParam as i32
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let handled = crate::crash::guard(|| match app() {
        Some(app) => app.handle(hwnd, msg, wp, lp),
        None => None,
    });
    match handled {
        Some(Some(r)) => r,
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Scintilla 通知的公共头部（NMHDR）。
#[repr(C)]
#[allow(dead_code)]
struct NotifyHeader {
    hwnd_from: HWND,
    id_from: usize,
    code: u32,
}

impl App {
    fn sci(&self) -> Option<Sci> {
        self.sci.get()
    }

    /// 处理窗口消息；返回 `None` 表示交给 DefWindowProc。
    fn handle(&self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_CREATE => {
                self.hwnd.set(hwnd);
                if self.create_editor(hwnd) {
                    Some(0)
                } else {
                    Some(-1)
                }
            }
            WM_SIZE => {
                if let Some(sci) = self.sci() {
                    let w = loword(lp as usize) as i32;
                    let h = hiword(lp as usize) as i32;
                    unsafe { MoveWindow(sci.hwnd(), 0, 0, w, h, 1) };
                }
                Some(0)
            }
            WM_SETFOCUS => {
                if let Some(sci) = self.sci() {
                    unsafe { SetFocus(sci.hwnd()) };
                }
                Some(0)
            }
            WM_COMMAND => {
                // lParam 非 0 是子控件通知（如 SCEN_CHANGE），不是菜单命令。
                if lp != 0 {
                    return None;
                }
                self.command(loword(wp));
                Some(0)
            }
            WM_NOTIFY => {
                let hdr = unsafe { &*(lp as *const NotifyHeader) };
                if let Some(sci) = self.sci() {
                    if hdr.hwnd_from == sci.hwnd() {
                        self.on_editor_notify(hdr.code);
                    }
                }
                Some(0)
            }
            WM_CONTEXTMENU => {
                self.show_context_menu(lp);
                Some(0)
            }
            WM_DPICHANGED => {
                let r = unsafe { &*(lp as *const RECT) };
                unsafe {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
                Some(0)
            }
            WM_CLOSE => {
                if self.confirm_discard() {
                    unsafe { DestroyWindow(hwnd) };
                }
                Some(0)
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                Some(0)
            }
            WM_APP_SELFTEST => {
                self.run_selftest();
                Some(0)
            }
            _ => None,
        }
    }

    fn create_editor(&self, hwnd: HWND) -> bool {
        let Some(sci) = (unsafe { Sci::create(hwnd, self.hinstance, ID_EDITOR) }) else {
            return false;
        };
        theme::init_view(&sci);
        theme::apply_base(&sci, &theme::GRUVBOX_LIGHT, &theme::FontSpec::default());
        sci.send(SCI_SETEOLMODE, SC_EOL_CRLF as usize, 0);
        self.sci.set(Some(sci));
        true
    }

    fn on_editor_notify(&self, code: u32) {
        if matches!(code, SCN_SAVEPOINTREACHED | SCN_SAVEPOINTLEFT) {
            self.update_title();
        }
    }

    fn command(&self, id: u16) {
        let Some(sci) = self.sci() else { return };
        match id {
            IDM_FILE_NEW => self.new_document(),
            IDM_FILE_OPEN => self.open(),
            IDM_FILE_SAVE => {
                self.save();
            }
            IDM_FILE_SAVE_AS => {
                self.save_as();
            }
            IDM_FILE_EXIT => unsafe {
                PostMessageW(self.hwnd.get(), WM_CLOSE, 0, 0);
            },
            IDM_EDIT_UNDO => {
                sci.send(SCI_UNDO, 0, 0);
            }
            IDM_EDIT_REDO => {
                sci.send(SCI_REDO, 0, 0);
            }
            IDM_EDIT_CUT => {
                sci.send(SCI_CUT, 0, 0);
            }
            IDM_EDIT_COPY => {
                sci.send(SCI_COPY, 0, 0);
            }
            IDM_EDIT_PASTE => {
                sci.send(SCI_PASTE, 0, 0);
            }
            IDM_EDIT_DELETE => {
                sci.send(SCI_CLEAR, 0, 0);
            }
            IDM_EDIT_SELECT_ALL => {
                sci.send(SCI_SELECTALL, 0, 0);
            }
            IDM_HELP_ABOUT => dialogs::info_box(
                self.hwnd.get(),
                &format!(
                    "HurshNote {}\n\n原生 Windows 文本编辑器\n编辑内核：Scintilla / Lexilla\n\nMIT License",
                    env!("CARGO_PKG_VERSION")
                ),
            ),
            _ => {}
        }
    }

    fn show_context_menu(&self, lp: LPARAM) {
        let Some(sci) = self.sci() else { return };
        let (mut x, mut y) = point_from_lparam(lp);
        if lp == -1 {
            // 键盘触发（Shift+F10 / 菜单键）：在光标处弹出。
            let pos = sci.send(SCI_GETCURRENTPOS, 0, 0);
            let mut pt = POINT {
                x: sci.send(SCI_POINTXFROMPOSITION, 0, pos) as i32,
                y: sci.send(SCI_POINTYFROMPOSITION, 0, pos) as i32,
            };
            unsafe { ClientToScreen(sci.hwnd(), &mut pt) };
            x = pt.x;
            y = pt.y;
        }
        let menu = build_context_menu();
        unsafe {
            TrackPopupMenu(menu, TPM_RIGHTBUTTON, x, y, 0, self.hwnd.get(), null());
            DestroyMenu(menu);
        }
    }

    // ---- 文档操作 ----

    fn display_name(&self) -> String {
        self.doc
            .borrow()
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "无标题".to_string())
    }

    pub fn update_title(&self) {
        let modified = self.sci().is_some_and(|s| s.is_modified());
        let title = format!(
            "{}{} - HurshNote",
            if modified { "*" } else { "" },
            self.display_name()
        );
        let w = wide(&title);
        unsafe { SetWindowTextW(self.hwnd.get(), w.as_ptr()) };
    }

    /// 有未保存修改时询问用户。返回 `true` 表示可以继续（已保存或放弃修改）。
    fn confirm_discard(&self) -> bool {
        let Some(sci) = self.sci() else { return true };
        if !sci.is_modified() {
            return true;
        }
        let text = format!("是否保存对“{}”的修改？", self.display_name());
        match dialogs::ask_yes_no_cancel(self.hwnd.get(), &text) {
            Answer::Yes => self.save(),
            Answer::No => true,
            Answer::Cancel => false,
        }
    }

    fn new_document(&self) {
        if !self.confirm_discard() {
            return;
        }
        let Some(sci) = self.sci() else { return };
        sci.load_document(b"");
        sci.send(SCI_SETEOLMODE, SC_EOL_CRLF as usize, 0);
        *self.doc.borrow_mut() = DocState::default();
        theme::update_line_number_width(&sci);
        self.update_title();
    }

    fn open(&self) {
        if !self.confirm_discard() {
            return;
        }
        if let Some(path) = dialogs::open_file(self.hwnd.get()) {
            self.open_path(&path);
        }
    }

    pub fn open_path(&self, path: &Path) {
        let Some(sci) = self.sci() else { return };
        let hwnd = self.hwnd.get();

        match std::fs::metadata(path) {
            Ok(m) if m.len() > MAX_OPEN_SIZE => {
                dialogs::error_box(
                    hwnd,
                    &format!(
                        "{}\n\n文件超过 256 MB，大文件视图尚未实现。",
                        path.display()
                    ),
                );
                return;
            }
            Ok(_) => {}
            Err(e) => {
                dialogs::error_box(hwnd, &format!("无法打开 {}\n\n{e}", path.display()));
                return;
            }
        }

        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                dialogs::error_box(hwnd, &format!("无法读取 {}\n\n{e}", path.display()));
                return;
            }
        };
        let decoded = match hn_text::encoding::decode_utf8(&bytes) {
            Ok(d) => d,
            Err(e) => {
                dialogs::error_box(
                    hwnd,
                    &format!(
                        "{}\n\n{e}。\n当前版本只支持 UTF-8，其他编码即将支持。",
                        path.display()
                    ),
                );
                return;
            }
        };

        let text = decoded.text.as_bytes();
        let eol_mode = match eol::detect(text, Eol::Crlf) {
            Eol::Crlf => SC_EOL_CRLF,
            Eol::Lf => SC_EOL_LF,
            Eol::Cr => SC_EOL_CR,
        };
        sci.load_document(text);
        sci.send(SCI_SETEOLMODE, eol_mode as usize, 0);
        theme::update_line_number_width(&sci);

        *self.doc.borrow_mut() = DocState {
            path: Some(path.to_path_buf()),
            bom: decoded.bom,
        };
        self.update_title();
    }

    /// 保存。返回是否成功（用户取消也算失败）。
    fn save(&self) -> bool {
        let path = self.doc.borrow().path.clone();
        match path {
            Some(p) => self.write_to(&p),
            None => self.save_as(),
        }
    }

    fn save_as(&self) -> bool {
        let current = self.doc.borrow().path.clone();
        let Some(path) = dialogs::save_file(self.hwnd.get(), current.as_deref()) else {
            return false;
        };
        if !self.write_to(&path) {
            return false;
        }
        self.doc.borrow_mut().path = Some(path);
        self.update_title();
        true
    }

    /// 写入磁盘。M0 为直接写入，M2 替换为安全保存流程（临时文件 + ReplaceFileW）。
    fn write_to(&self, path: &Path) -> bool {
        let Some(sci) = self.sci() else { return false };
        let bom = self.doc.borrow().bom;
        let data = hn_text::encoding::encode_utf8(&sci.bytes(), bom);
        match std::fs::write(path, data) {
            Ok(()) => {
                sci.set_save_point();
                true
            }
            Err(e) => {
                dialogs::error_box(
                    self.hwnd.get(),
                    &format!("无法保存 {}\n\n{e}", path.display()),
                );
                false
            }
        }
    }

    // ---- 自测 ----

    fn run_selftest(&self) {
        let startup_ms = selftest::process_age_ms();
        let report = match self.sci() {
            Some(sci) => selftest::run(&sci, startup_ms),
            None => format!("startup_ms={startup_ms:.1}\nsci_ok=0\n"),
        };
        if let Some(out) = &self.selftest_out {
            let _ = std::fs::write(out, report);
        }
        if let Some(sci) = self.sci() {
            sci.load_document(b"");
        }
        unsafe { DestroyWindow(self.hwnd.get()) };
    }
}
