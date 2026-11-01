//! 主窗口与应用状态。
//!
//! 线程规则：本模块的一切都只在 UI 线程运行；后台加载线程只通过 channel + PostMessage 回传。
//!
//! 借用规则：`RefCell` 的借用不得跨越任何可能重入窗口过程的调用
//! （Scintilla 消息会同步发出 WM_NOTIFY，消息框和对话框会运行消息循环）。
//! 先把需要的数据取出来、释放借用，再调用 Scintilla。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};

use hn_sci::theme::{self, GRUVBOX_LIGHT};
use hn_sci::*;
use hn_search::Searcher;
use hn_text::encoding::{self, DetectHints, EncodingInfo};
use hn_text::eol::Eol;
use hn_text::lang::{self, LANGS};
use hn_text::lines::SortOptions;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Globalization::GetACP;
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CreateSolidBrush, SetBkColor, SetTextColor, UpdateWindow, HDC,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows_sys::Win32::UI::Controls::{InitCommonControlsEx, INITCOMMONCONTROLSEX};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, DragAcceptFiles, DragFinish, DragQueryFileW, SetWindowSubclass, HDROP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuItem, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DeleteMenu,
    DestroyAcceleratorTable, DestroyMenu, DestroyWindow, DispatchMessageW, EnableMenuItem,
    GetCursorPos, GetMenuItemCount, GetMessageW, GetWindowPlacement, IsDialogMessageW, KillTimer,
    LoadCursorW, LoadIconW, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassExW, SetTimer,
    SetWindowPlacement, SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu,
    TranslateAcceleratorW, TranslateMessage, HMENU, MSG, WINDOWPLACEMENT, WNDCLASSEXW,
};

use crate::commands::*;
use crate::config::{self, Settings};
use crate::dialogs::{self, Answer};
use crate::docs::{self, Doc, LoadJob, Loaded, ViewState};
use crate::editops::{self, LineOp};
use crate::finddlg::{self, FindAction, FindState};
use crate::instance::{self, Args};
use crate::statusbar::{self, *};
use crate::win::*;

#[path = "files.rs"]
mod files;
#[path = "persist.rs"]
mod persist;
#[path = "quick.rs"]
mod quick;
#[path = "sidebar.rs"]
mod sidebar;
#[path = "views.rs"]
mod views;

use crate::crashdump::{self, RunMarker};
use crate::watcher::{Event as FsEvent, Watcher};
use hn_core::snapshot::Store;
use persist::{SnapJob, TIMER_SESSION, TIMER_SESSIONTEST, TIMER_SNAPSHOT};
use sidebar::Drag;

pub const CLASS_NAME: &str = "HurshNoteMain";
const ID_EDITOR: usize = 100;
const ID_STATUS: usize = 101;
const ID_EDITOR2: usize = 102;
const WM_APP_SELFTEST: u32 = WM_APP + 1;
const WM_APP_LOADED: u32 = WM_APP + 2;
const WM_APP_REFRESH: u32 = WM_APP + 3;
const WM_APP_FILES: u32 = WM_APP + 4;
const WM_APP_SESSIONTEST: u32 = WM_APP + 5;
/// wParam = 文档 id；lParam = 0 时激活后把焦点移到编辑区
const WM_APP_ACTIVATE_DOC: u32 = WM_APP + 6;
const WM_APP_FOCUS_VIEW: u32 = WM_APP + 8;
const WM_SETCURSOR: u32 = 0x0020;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_CAPTURECHANGED: u32 = 0x0215;
const WM_CTLCOLORLISTBOX: u32 = 0x0134;
const WM_CTLCOLORSTATIC: u32 = 0x0138;
const ICC_LISTVIEW_CLASSES: u32 = 0x1;
const ICC_TREEVIEW_CLASSES: u32 = 0x2;
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;
const TIMER_MESSAGE: usize = 1;
const MF_BYPOSITION: u32 = 0x0400;
const MF_CHECKED: u32 = 0x0008;
const MF_GRAYED: u32 = 0x0001;
const SW_SHOWMAXIMIZED: u32 = 3;

pub struct App {
    hinstance: *mut core::ffi::c_void,
    hwnd: Cell<HWND>,
    sci: Cell<Option<Sci>>,
    status: Cell<HWND>,
    find_dlg: Cell<HWND>,
    menus: Cell<Option<Menus>>,
    docs: RefCell<Vec<Doc>>,
    active: Cell<u32>,
    next_id: Cell<u32>,
    untitled_seq: Cell<u32>,
    settings: RefCell<Settings>,
    recent: RefCell<Vec<PathBuf>>,
    find: RefCell<FindState>,
    hints: DetectHints,
    loads_tx: Sender<Loaded>,
    loads_rx: Receiver<Loaded>,
    wheel_accum: Cell<i32>,
    message: RefCell<String>,
    dpi: Cell<u32>,
    /// 其他实例转发来、等待处理的命令行参数
    pending_args: RefCell<Vec<Args>>,
    selftest_out: Option<PathBuf>,
    session_test: Option<(String, String, PathBuf)>,
    // ---- 快照与会话 ----
    store: Store,
    snap_tx: Sender<SnapJob>,
    snap_armed: Cell<bool>,
    snap_first: Cell<u64>,
    /// 等会话清单写出后再删除的快照
    to_remove: RefCell<Vec<String>>,
    // ---- 文件监控 ----
    watcher: RefCell<Option<Watcher>>,
    fs_tx: Sender<FsEvent>,
    fs_rx: Receiver<FsEvent>,
    pending_fs: RefCell<Vec<FsEvent>>,
    marker: RefCell<Option<RunMarker>>,
    // ---- 分屏 ----
    /// 两个编辑视图；`sci` 是其中的焦点视图
    views: [Cell<Option<Sci>>; 2],
    /// 各视图显示的文档
    view_doc: [Cell<u32>; 2],
    focus: Cell<usize>,
    /// 0 不分屏，1 左右，2 上下
    split: Cell<u8>,
    split_ratio: Cell<f32>,
    /// 分隔条拖动时的可用范围（左右分屏为 x，上下分屏为 y）
    editor_span: Cell<(i32, i32)>,
    status_height: Cell<i32>,
    mru: RefCell<Vec<u32>>,
    switcher: Cell<HWND>,
    switcher_ids: RefCell<Vec<u32>>,
    // ---- 侧边栏 ----
    side_list: Cell<HWND>,
    side_tree: Cell<HWND>,
    side_lbl_open: Cell<HWND>,
    side_lbl_ws: Cell<HWND>,
    side_font: Cell<isize>,
    side_brush: Cell<isize>,
    side_suppress: Cell<bool>,
    icon_cache: RefCell<HashMap<String, i32>>,
    tree_nodes: RefCell<Vec<PathBuf>>,
    splitters: RefCell<Vec<(Drag, RECT)>>,
    drag: Cell<Option<Drag>>,
    ctx_doc: Cell<u32>,
    ctx_tree: Cell<isize>,
    /// 拖放到编辑区、等待打开的文件
    pending_drop: RefCell<Vec<PathBuf>>,
    // ---- 快速打开 ----
    quick: Cell<HWND>,
    quick_paths: RefCell<Vec<PathBuf>>,
    file_index: RefCell<Option<quick::FileIndex>>,
    index_building: Cell<bool>,
    index_tx: Sender<quick::FileIndex>,
    index_rx: Receiver<quick::FileIndex>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

pub fn app() -> Option<Rc<App>> {
    APP.with(|a| a.borrow().clone())
}

/// 程序主流程，返回进程退出码。
pub fn run() -> i32 {
    crate::crash::install_panic_hook();
    let raw_args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    let args = instance::parse(raw_args.clone(), &cwd);

    if let Some((tmp, target)) = &args.elevated_save {
        return crate::fsio::run_elevated_helper(tmp, target);
    }
    crashdump::install();

    if !args.multi_inst && instance::forward_to_running(CLASS_NAME, &raw_args) {
        return 0;
    }

    unsafe {
        // 拖放目标（RegisterDragDrop）需要 OLE
        OleInitialize(null());
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES
                | ICC_STANDARD_CLASSES
                | ICC_LISTVIEW_CLASSES
                | ICC_TREEVIEW_CLASSES,
        };
        InitCommonControlsEx(&icc);

        let hinstance = GetModuleHandleW(null());
        if !hn_sci::register_classes(hinstance) {
            dialogs::error_box(null_mut(), "无法初始化编辑器组件（Scintilla）。");
            return 1;
        }

        let class_name = wide(CLASS_NAME);
        let bg_brush = CreateSolidBrush(theme::bgr(GRUVBOX_LIGHT.bg0_s) as u32);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: LoadIconW(hinstance, 1 as _),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW as usize as _),
            hbrBackground: bg_brush,
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            dialogs::error_box(null_mut(), "无法注册主窗口类。");
            return 1;
        }

        let (tx, rx) = channel();
        let (fs_tx, fs_rx) = channel();
        let (index_tx, index_rx) = channel();
        let settings = config::load_settings();
        let store = Store::new(config::data_dir().join("snapshots"));
        let snap_tx = persist::start_writer(store.clone());
        let (marker, crashed) = if args.selftest.is_none() {
            let (m, c) = RunMarker::acquire(&config::data_dir());
            (Some(m), c)
        } else {
            (None, false)
        };
        let app = Rc::new(App {
            hinstance,
            hwnd: Cell::new(null_mut()),
            sci: Cell::new(None),
            status: Cell::new(null_mut()),
            find_dlg: Cell::new(null_mut()),
            menus: Cell::new(None),
            docs: RefCell::new(Vec::new()),
            active: Cell::new(0),
            next_id: Cell::new(1),
            untitled_seq: Cell::new(0),
            settings: RefCell::new(settings.clone()),
            recent: RefCell::new(config::load_recent()),
            find: RefCell::new(FindState {
                wrap: true,
                ..Default::default()
            }),
            hints: DetectHints::for_codepage(GetACP()),
            loads_tx: tx,
            loads_rx: rx,
            wheel_accum: Cell::new(0),
            message: RefCell::new(String::new()),
            dpi: Cell::new(96),
            pending_args: RefCell::new(Vec::new()),
            selftest_out: args.selftest.clone(),
            session_test: args.session_test.clone(),
            store,
            snap_tx,
            snap_armed: Cell::new(false),
            snap_first: Cell::new(0),
            to_remove: RefCell::new(Vec::new()),
            watcher: RefCell::new(None),
            fs_tx,
            fs_rx,
            pending_fs: RefCell::new(Vec::new()),
            marker: RefCell::new(marker),
            views: [Cell::new(None), Cell::new(None)],
            view_doc: [Cell::new(0), Cell::new(0)],
            focus: Cell::new(0),
            split: Cell::new(0),
            split_ratio: Cell::new(0.5),
            editor_span: Cell::new((0, 0)),
            status_height: Cell::new(0),
            mru: RefCell::new(Vec::new()),
            switcher: Cell::new(null_mut()),
            switcher_ids: RefCell::new(Vec::new()),
            side_list: Cell::new(null_mut()),
            side_tree: Cell::new(null_mut()),
            side_lbl_open: Cell::new(null_mut()),
            side_lbl_ws: Cell::new(null_mut()),
            side_font: Cell::new(0),
            side_brush: Cell::new(bg_brush as isize),
            side_suppress: Cell::new(false),
            icon_cache: RefCell::new(HashMap::new()),
            tree_nodes: RefCell::new(Vec::new()),
            splitters: RefCell::new(Vec::new()),
            drag: Cell::new(None),
            ctx_doc: Cell::new(0),
            ctx_tree: Cell::new(0),
            pending_drop: RefCell::new(Vec::new()),
            quick: Cell::new(null_mut()),
            quick_paths: RefCell::new(Vec::new()),
            file_index: RefCell::new(None),
            index_building: Cell::new(false),
            index_tx,
            index_rx,
        });
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));

        let menus = build_main_menu();
        app.menus.set(Some(menus));
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
            menus.bar,
            hinstance,
            null(),
        );
        if hwnd.is_null() {
            dialogs::error_box(null_mut(), "无法创建主窗口。");
            return 1;
        }

        if app.snapshots_enabled() && !args.no_session {
            app.restore_session();
        }
        app.open_from_args(&args);
        app.refresh();
        if crashed && app.session_test.is_none() {
            app.show_message("上次 HurshNote 没有正常退出，已从快照恢复未保存的内容");
        }

        let mut show = SW_SHOWDEFAULT;
        if app.selftest_out.is_none() {
            if let Some((l, t, r, b)) = settings.window {
                let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
                wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                wp.rcNormalPosition = RECT {
                    left: l,
                    top: t,
                    right: r,
                    bottom: b,
                };
                wp.showCmd = if settings.maximized {
                    SW_SHOWMAXIMIZED
                } else {
                    1
                };
                SetWindowPlacement(hwnd, &wp);
                show = wp.showCmd as i32;
            }
        }
        ShowWindow(hwnd, show);
        UpdateWindow(hwnd);

        if app.selftest_out.is_some() {
            PostMessageW(hwnd, WM_APP_SELFTEST, 0, 0);
        }
        if app.session_test.is_some() {
            PostMessageW(hwnd, WM_APP_SESSIONTEST, 0, 0);
        }

        let accel = build_accelerators();
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if crate::crash::guard(|| app.pretranslate(&msg)) == Some(true) {
                continue;
            }
            if app.accel_allowed(&msg) && TranslateAcceleratorW(app.hwnd.get(), accel, &msg) != 0 {
                continue;
            }
            let fd = app.find_dlg.get();
            if !fd.is_null() && IsDialogMessageW(fd, &msg) != 0 {
                continue;
            }
            let qd = app.quick_dialog();
            if !qd.is_null() && IsDialogMessageW(qd, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        DestroyAcceleratorTable(accel);
        if let Some(m) = app.marker.borrow().as_ref() {
            m.release();
        }

        APP.with(|a| *a.borrow_mut() = None);
        hn_sci::release_resources();
        OleUninitialize();
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

/// 编辑器子类化：Ctrl+滚轮无级缩放、拖放文件。
unsafe extern "system" fn editor_subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_MOUSEWHEEL && (loword(wp) as usize & MK_CONTROL) != 0 {
        let delta = hiword(wp) as i16 as i32;
        if let Some(Some(app)) = crate::crash::guard(app) {
            crate::crash::guard(|| app.on_ctrl_wheel(delta));
        }
        return 0;
    }
    if msg == WM_DROPFILES {
        if let Some(Some(app)) = crate::crash::guard(app) {
            crate::crash::guard(|| app.on_drop(wp as HDROP));
        }
        return 0;
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

/// 编辑区 OLE 拖放目标的文件回调：记下路径，稍后在消息循环中打开（拖放过程中不弹对话框）。
unsafe extern "C" fn on_drop_files(_ctx: *mut core::ffi::c_void, paths: *const u16) {
    let _ = crate::crash::guard(|| {
        let files = unsafe { hn_sci::parse_drop_paths(paths) };
        if let Some(app) = app() {
            app.pending_drop.borrow_mut().extend(files);
            unsafe { PostMessageW(app.hwnd(), WM_APP_REFRESH, 2, 0) };
        }
    });
}

impl App {
    fn sci(&self) -> Option<Sci> {
        self.sci.get()
    }

    fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    /// 处理窗口消息；返回 `None` 表示交给 DefWindowProc。
    fn handle(&self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_CREATE => {
                self.hwnd.set(hwnd);
                Some(if self.on_create(hwnd) { 0 } else { -1 })
            }
            WM_SIZE => {
                self.layout();
                Some(0)
            }
            WM_SETFOCUS => {
                if let Some(sci) = self.sci() {
                    unsafe { SetFocus(sci.hwnd()) };
                }
                Some(0)
            }
            WM_COMMAND => {
                if lp != 0 {
                    return None; // 子控件通知（如 SCEN_CHANGE）
                }
                self.command(loword(wp));
                Some(0)
            }
            WM_NOTIFY => Some(self.on_notify(lp)),
            WM_CONTEXTMENU => {
                let target = wp as HWND;
                if target == self.side_list.get() {
                    self.list_context_menu_at_cursor(lp == -1);
                } else if target == self.side_tree.get() {
                    self.tree_context_menu();
                } else if self.view_index(target).is_some() {
                    self.show_context_menu(lp);
                } else {
                    return None;
                }
                Some(0)
            }
            WM_SETCURSOR => {
                if wp as HWND == hwnd && self.on_set_cursor() {
                    Some(1)
                } else {
                    None
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_from_lparam(lp);
                self.on_lbutton_down(x, y).then_some(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_from_lparam(lp);
                self.on_mouse_move(x, y);
                Some(0)
            }
            WM_LBUTTONUP => {
                self.on_lbutton_up();
                Some(0)
            }
            WM_CAPTURECHANGED => {
                if lp as HWND != hwnd {
                    self.drag.set(None);
                }
                Some(0)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX => {
                let hdc = wp as HDC;
                let p = &GRUVBOX_LIGHT;
                let fg = if msg == WM_CTLCOLORSTATIC {
                    p.gray
                } else {
                    p.fg1
                };
                unsafe {
                    SetBkColor(hdc, theme::bgr(p.bg0_s) as u32);
                    SetTextColor(hdc, theme::bgr(fg) as u32);
                }
                Some(self.side_brush.get())
            }
            WM_APP_ACTIVATE_DOC => {
                self.activate(wp as u32);
                if lp == 0 {
                    if let Some(s) = self.sci() {
                        unsafe { SetFocus(s.hwnd()) };
                    }
                }
                Some(0)
            }
            WM_APP_FOCUS_VIEW => {
                self.focus_view(wp);
                Some(0)
            }
            quick::WM_APP_INDEX => {
                self.on_index_ready();
                Some(0)
            }
            WM_INITMENUPOPUP => {
                self.on_init_menu(wp as HMENU);
                Some(0)
            }
            WM_DROPFILES => {
                self.on_drop(wp as HDROP);
                Some(0)
            }
            WM_COPYDATA => {
                let cds = unsafe { &*(lp as *const COPYDATASTRUCT) };
                if cds.dwData == instance::COPYDATA_ARGS && !cds.lpData.is_null() {
                    let data = unsafe {
                        std::slice::from_raw_parts(
                            cds.lpData as *const u16,
                            cds.cbData as usize / 2,
                        )
                    };
                    let (cwd, raw) = instance::decode_payload(data);
                    // 消息处理中不能弹出模态框（发送方在等待），延后执行
                    let args = instance::parse(raw, &cwd);
                    self.pending_args.borrow_mut().push(args);
                    unsafe { PostMessageW(hwnd, WM_APP_REFRESH, 1, 0) };
                }
                Some(1)
            }
            WM_TIMER => {
                match wp {
                    TIMER_MESSAGE => {
                        unsafe { KillTimer(hwnd, TIMER_MESSAGE) };
                        self.message.borrow_mut().clear();
                        self.update_status();
                    }
                    TIMER_SNAPSHOT | TIMER_SESSION => self.on_persist_timer(wp),
                    TIMER_SESSIONTEST => self.session_test_ready(),
                    _ => {}
                }
                Some(0)
            }
            WM_ACTIVATE => {
                if loword(wp) == 0 {
                    self.switcher_close(false);
                }
                // 回到前台时处理期间积累的外部修改
                if loword(wp) != 0 && !self.pending_fs.borrow().is_empty() {
                    unsafe { PostMessageW(hwnd, WM_APP_FILES, 0, 0) };
                }
                None
            }
            WM_APP_FILES => {
                self.on_fs_events();
                Some(0)
            }
            WM_QUERYENDSESSION => {
                // 注销/关机：立即保存快照
                self.flush_persist();
                Some(1)
            }
            WM_ENDSESSION => {
                if wp != 0 {
                    self.flush_persist();
                    if let Some(m) = self.marker.borrow().as_ref() {
                        m.release();
                    }
                }
                Some(0)
            }
            WM_APP_SESSIONTEST => {
                self.session_test_start();
                Some(0)
            }
            WM_DPICHANGED => {
                self.dpi.set(hiword(wp) as u32);
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
                for v in &self.views {
                    if let Some(s) = v.get() {
                        theme::init_view(&s, &GRUVBOX_LIGHT, self.dpi.get());
                    }
                }
                self.apply_all_views();
                self.update_sidebar_font();
                self.layout();
                Some(0)
            }
            WM_CLOSE => {
                // 快照模式：不询问，保留未保存内容，下次启动恢复
                let ok = if self.snapshots_enabled() {
                    self.flush_persist();
                    true
                } else {
                    self.confirm_close_all()
                };
                if ok {
                    self.save_window_state();
                    unsafe { DestroyWindow(hwnd) };
                }
                Some(0)
            }
            WM_DESTROY => {
                self.destroy_sidebar();
                unsafe { PostQuitMessage(0) };
                Some(0)
            }
            WM_APP_SELFTEST => {
                self.run_selftest();
                Some(0)
            }
            WM_APP_LOADED => {
                while let Ok(l) = self.loads_rx.try_recv() {
                    self.finish_load(l);
                }
                Some(0)
            }
            WM_APP_REFRESH => {
                if wp == 1 {
                    let pending: Vec<Args> = self.pending_args.borrow_mut().drain(..).collect();
                    for a in pending {
                        self.open_from_args(&a);
                    }
                }
                if wp == 2 {
                    let files: Vec<PathBuf> = self.pending_drop.borrow_mut().drain(..).collect();
                    self.open_dropped(files);
                }
                self.refresh();
                Some(0)
            }
            _ => None,
        }
    }

    fn on_create(&self, hwnd: HWND) -> bool {
        self.dpi.set(unsafe { GetDpiForWindow(hwnd) }.max(96));
        let Some(sci) = (unsafe { Sci::create(hwnd, self.hinstance, ID_EDITOR) }) else {
            return false;
        };
        let Some(sci2) = (unsafe { Sci::create(hwnd, self.hinstance, ID_EDITOR2) }) else {
            return false;
        };
        self.sci.set(Some(sci));
        self.views[0].set(Some(sci));
        self.views[1].set(Some(sci2));
        unsafe { DragAcceptFiles(hwnd, 1) };
        for v in [sci, sci2] {
            theme::init_view(&v, &GRUVBOX_LIGHT, self.dpi.get());
            unsafe {
                SetWindowSubclass(v.hwnd(), Some(editor_subclass), 1, 0);
                DragAcceptFiles(v.hwnd(), 1);
                // 替换 Scintilla 自带的拖放目标：同时接受文件与文字
                hn_sci::register_drop_target(&v, on_drop_files, null_mut());
            }
        }
        unsafe { ShowWindow(sci2.hwnd(), SW_HIDE) };

        // 接管视图自带的初始文档
        let ptr = sci.doc_pointer();
        sci.add_ref_document(ptr);
        let id = self.alloc_id();
        let mut d = Doc::new(id, ptr);
        d.untitled = self.next_untitled();
        self.docs.borrow_mut().push(d);
        self.active.set(id);
        self.view_doc[0].set(id);
        self.touch_mru(id);
        *self.watcher.borrow_mut() = Some(Watcher::start(hwnd, WM_APP_FILES, self.fs_tx.clone()));
        sci.send(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0);
        sci.send(SCI_SETEOLMODE, SC_EOL_CRLF as usize, 0);
        theme::set_language(&sci, LANGS[0].id, LANGS[0].lexer);
        self.apply_doc_view();

        self.create_sidebar(hwnd);
        let sb = statusbar::create(hwnd, self.hinstance, ID_STATUS);
        self.status.set(sb);
        self.find_dlg.set(finddlg::create(hwnd, self.hinstance));
        true
    }

    fn layout(&self) {
        let hwnd = self.hwnd();
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc) };
        let sb = self.status.get();
        let sh = if sb.is_null() {
            0
        } else {
            statusbar::layout(sb, self.dpi.get())
        };
        self.status_height.set(sh);
        let h = (rc.bottom - sh).max(0);
        let mut splitters = Vec::new();
        let left = self.layout_sidebar(h, &mut splitters);
        self.layout_editors(left, (rc.right - left).max(0), h, &mut splitters);
        *self.splitters.borrow_mut() = splitters;
    }

    fn alloc_id(&self) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    fn next_untitled(&self) -> u32 {
        let n = self.untitled_seq.get() + 1;
        self.untitled_seq.set(n);
        n
    }

    // ---- 文档查询 ----

    fn with_doc<R>(&self, id: u32, f: impl FnOnce(&Doc) -> R) -> Option<R> {
        self.docs.borrow().iter().find(|d| d.id == id).map(f)
    }

    fn with_doc_mut<R>(&self, id: u32, f: impl FnOnce(&mut Doc) -> R) -> Option<R> {
        self.docs
            .borrow_mut()
            .iter_mut()
            .find(|d| d.id == id)
            .map(f)
    }

    fn active_doc<R>(&self, f: impl FnOnce(&Doc) -> R) -> Option<R> {
        self.with_doc(self.active.get(), f)
    }

    fn active_doc_mut<R>(&self, f: impl FnOnce(&mut Doc) -> R) -> Option<R> {
        self.with_doc_mut(self.active.get(), f)
    }

    fn doc_ids(&self) -> Vec<u32> {
        self.docs.borrow().iter().map(|d| d.id).collect()
    }

    /// 文档是否有未保存修改（显示中的文档以 Scintilla 为准）。
    fn is_dirty(&self, id: u32) -> bool {
        let shown = if id == self.active.get() {
            self.sci()
        } else {
            self.other_view()
                .filter(|(o, _)| self.view_doc[*o].get() == id)
                .map(|(_, v)| v)
        };
        match shown {
            Some(v) => v.is_modified() || self.with_doc(id, |d| d.meta_dirty).unwrap_or(false),
            None => self.with_doc(id, |d| d.is_dirty()).unwrap_or(false),
        }
    }

    // ---- 切换文档 ----

    fn capture_view(sci: &Sci) -> ViewState {
        ViewState {
            anchor: sci.anchor(),
            caret: sci.current_pos(),
            first_line: sci.send(SCI_GETFIRSTVISIBLELINE, 0, 0) as usize,
            x_offset: sci.send(SCI_GETXOFFSET, 0, 0),
        }
    }

    /// 切换到文档：已在另一视图显示时把焦点移过去，否则在焦点视图中显示。
    pub fn activate(&self, id: u32) {
        if self.active.get() == id {
            return;
        }
        if let Some((o, _)) = self.other_view() {
            if self.view_doc[o].get() == id {
                self.focus_view(o);
                return;
            }
        }
        self.activate_here(id);
    }

    /// 在焦点视图中显示文档。
    fn activate_here(&self, id: u32) {
        let Some(sci) = self.sci() else { return };
        let cur = self.active.get();
        if cur == id {
            return;
        }
        // 会话中尚未载入的文档：现在载入
        if self.with_doc(id, |d| d.ptr == 0) == Some(true) && !self.load_pending(id) {
            return;
        }
        let Some((ptr, view)) = self.with_doc(id, |d| (d.ptr, d.view)) else {
            return;
        };
        // 离开当前文档：写快照、记录状态（这些只能对当前文档读取）
        self.leave_active();

        sci.set_doc_pointer(ptr);
        self.active.set(id);
        self.view_doc[self.focus.get()].set(id);
        self.touch_mru(id);
        sci.send(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0);
        match self.with_doc_mut(id, |d| d.init.take()).flatten() {
            Some(init) => self.apply_init(&sci, id, init),
            None => {
                self.apply_doc_view();
                sci.set_sel(view.anchor, view.caret);
                sci.send(SCI_SETFIRSTVISIBLELINE, view.first_line, 0);
                sci.send(SCI_SETXOFFSET, view.x_offset as usize, 0);
            }
        }
        self.refresh();
        self.arm_session_timer();
    }

    /// 按当前文档与设置应用焦点视图：样式、换行、空白显示、行号等。
    fn apply_doc_view(&self) {
        self.apply_view(self.focus.get());
    }

    /// 设置变化后应用到所有可见视图。
    fn apply_all_views(&self) {
        self.apply_view(self.focus.get());
        if let Some((o, _)) = self.other_view() {
            self.apply_view(o);
        }
    }

    /// 按视图中的文档与设置应用视图属性。
    fn apply_view(&self, i: usize) {
        let Some(sci) = self.views[i].get() else {
            return;
        };
        let (lang_idx, long_lines) = self
            .with_doc(self.view_doc[i].get(), |d| (d.lang, d.long_lines))
            .unwrap_or((0, false));
        let s = self.settings.borrow().clone();
        let font = theme::FontSpec {
            face: s.font_face.clone(),
            size_x100: s.font_size_x100,
        };
        let lang = &LANGS[lang_idx];
        theme::apply_styles(
            &sci,
            &GRUVBOX_LIGHT,
            &font,
            s.zoom,
            if long_lines { "text" } else { lang.id },
        );
        let wrap = if s.wrap && !long_lines {
            SC_WRAP_WORD
        } else {
            SC_WRAP_NONE
        };
        sci.send(SCI_SETWRAPMODE, wrap as usize, 0);
        let ws = if s.show_whitespace {
            SCWS_VISIBLEALWAYS
        } else {
            SCWS_INVISIBLE
        };
        sci.send(SCI_SETVIEWWS, ws as usize, 0);
        sci.send(SCI_SETVIEWEOL, s.show_eol as usize, 0);
        let guides = if s.indent_guides && !long_lines {
            SC_IV_LOOKBOTH
        } else {
            SC_IV_NONE
        };
        sci.send(SCI_SETINDENTATIONGUIDES, guides as usize, 0);
        theme::update_line_number_width(&sci, s.line_numbers);
    }

    // ---- 新建 / 打开 / 关闭 ----

    pub fn new_document(&self) -> u32 {
        let Some(sci) = self.sci() else { return 0 };
        let ptr = sci.create_document();
        let id = self.alloc_id();
        let mut d = Doc::new(id, ptr);
        d.untitled = self.next_untitled();
        self.docs.borrow_mut().push(d);
        self.activate(id);
        sci.send(SCI_SETEOLMODE, SC_EOL_CRLF as usize, 0);
        theme::set_language(&sci, LANGS[0].id, LANGS[0].lexer);
        self.apply_doc_view();
        self.refresh();
        id
    }

    fn open_from_args(&self, args: &Args) {
        let goto = args.line.map(|l| (l, args.col));
        for f in &args.files {
            self.open_path(f, goto, args.read_only);
        }
    }

    fn open_dialog(&self) {
        let dir = self
            .active_doc(|d| {
                d.path
                    .as_ref()
                    .and_then(|p| p.parent().map(Path::to_path_buf))
            })
            .flatten();
        for path in dialogs::open_files(self.hwnd(), dir.as_deref()) {
            self.open_path(&path, None, false);
        }
    }

    pub fn open_path(&self, path: &Path, goto: Option<(usize, Option<usize>)>, read_only: bool) {
        let Some(sci) = self.sci() else { return };
        let path = std::fs::canonicalize(path)
            .map(|p| strip_verbatim(&p))
            .unwrap_or_else(|_| path.to_path_buf());

        // 已打开则切换过去
        let existing = self
            .docs
            .borrow()
            .iter()
            .find(|d| {
                d.path
                    .as_deref()
                    .is_some_and(|p| config::same_path(p, &path))
            })
            .map(|d| d.id);
        if let Some(id) = existing {
            self.activate(id);
            if let Some((l, c)) = goto {
                editops::goto_line_col(&sci, l, c);
            }
            return;
        }

        let size = match docs::check_size(&path) {
            Ok(s) => s,
            Err(e) => {
                dialogs::error_box(self.hwnd(), &format!("无法打开 {}\n\n{e}", path.display()));
                return;
            }
        };
        if docs::is_encrypted_file(&path) {
            self.open_encrypted(&path, goto, read_only);
            return;
        }
        let Some(loader) = sci.create_loader(size as usize) else {
            dialogs::error_box(self.hwnd(), "内存不足，无法打开文件。");
            return;
        };
        let job = LoadJob {
            path: path.clone(),
            forced: None,
            reuse: None,
            goto,
            read_only,
        };
        self.start_load(job, loader, size);
    }

    fn start_load(&self, job: LoadJob, loader: Loader, size: u64) {
        let hints = self.hints;
        if size <= docs::ASYNC_LOAD_SIZE || self.selftest_out.is_some() {
            let loaded = docs::load(job, loader, hints);
            self.finish_load(loaded);
        } else {
            self.show_message(&format!("正在打开 {} …", job.path.display()));
            let tx = self.loads_tx.clone();
            let hwnd = self.hwnd() as isize;
            std::thread::spawn(move || {
                let loaded = docs::load(job, loader, hints);
                let _ = tx.send(loaded);
                unsafe { PostMessageW(hwnd as HWND, WM_APP_LOADED, 0, 0) };
            });
        }
    }

    fn finish_load(&self, loaded: Loaded) {
        let Some(sci) = self.sci() else { return };
        let job = loaded.job;
        let t = match loaded.result {
            Ok(t) => t,
            Err(e) => {
                self.message.borrow_mut().clear();
                dialogs::error_box(
                    self.hwnd(),
                    &format!("无法打开 {}\n\n{e}", job.path.display()),
                );
                return;
            }
        };
        let lang_idx = if t.long_lines {
            0
        } else {
            lang::index_of(lang::detect(Some(&job.path), &t.head))
        };
        let new_ptr = t.loader.into_document();

        let id = match job.reuse {
            Some(id) => {
                // 重新载入：替换文档指针，保留位置
                let Some(old) = self.with_doc(id, |d| d.ptr) else {
                    sci.release_document(new_ptr);
                    return;
                };
                if self.active.get() != id {
                    self.activate(id);
                }
                // 激活后焦点视图可能变了
                let Some(sci) = self.sci() else { return };
                let caret_line = sci.line_from_pos(sci.current_pos());
                self.with_doc_mut(id, |d| d.ptr = new_ptr);
                sci.set_doc_pointer(new_ptr);
                sci.send(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0);
                if let Some((o, v)) = self.other_view() {
                    if self.view_doc[o].get() == id {
                        v.set_doc_pointer(new_ptr);
                        v.send(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0);
                    }
                }
                sci.release_document(old);
                sci.send(SCI_GOTOLINE, caret_line, 0);
                id
            }
            None => {
                let pristine = self.pristine_untitled();
                let id = self.alloc_id();
                let mut d = Doc::new(id, new_ptr);
                d.path = Some(job.path.clone());
                d.enc = t.info;
                d.lang = lang_idx;
                d.long_lines = t.long_lines;
                self.docs.borrow_mut().push(d);
                self.activate(id);
                if let Some(p) = pristine {
                    self.close_doc_unchecked(p);
                }
                id
            }
        };

        // 以下操作焦点视图（可能已切换）
        let Some(sci) = self.sci() else { return };
        self.with_doc_mut(id, |d| {
            d.enc = t.info;
            d.lossy = t.lossy;
            d.binary = t.binary;
            d.long_lines = t.long_lines;
            d.lang = lang_idx;
            d.meta_dirty = false;
            d.modified = false;
            d.missing = false;
            d.disk = Some(t.disk);
        });
        self.watch(id, &job.path);
        let eol_mode = match t.eol {
            Eol::Crlf => SC_EOL_CRLF,
            Eol::Lf => SC_EOL_LF,
            Eol::Cr => SC_EOL_CR,
        };
        sci.send(SCI_SETEOLMODE, eol_mode as usize, 0);
        let lang = &LANGS[lang_idx];
        theme::set_language(
            &sci,
            if t.long_lines { "text" } else { lang.id },
            if t.long_lines { "null" } else { lang.lexer },
        );
        sci.empty_undo_buffer();
        sci.set_save_point();
        sci.send(SCI_SETREADONLY, job.read_only as usize, 0);
        self.apply_all_views();
        if let Some((l, c)) = job.goto {
            editops::goto_line_col(&sci, l, c);
        }

        self.add_recent(&job.path);
        // 重新载入后文档与磁盘一致：清理快照
        self.snapshot_active();
        self.arm_session_timer();

        let mut notes = Vec::new();
        if t.binary {
            notes.push("按二进制文件打开（Windows-1252，往返无损）".to_string());
        }
        if t.lossy {
            notes.push(format!("按 {} 解码时有无法识别的字节", t.info.label()));
        }
        if t.long_lines {
            notes.push("包含超长行，已关闭高亮与自动换行".to_string());
        }
        if t.mixed_eol {
            notes.push("文件混用了多种换行符".to_string());
        }
        self.message.borrow_mut().clear();
        if !notes.is_empty() {
            self.show_message(&notes.join("；"));
        }
        self.refresh();
    }

    /// 唯一的、空的、未修改的未命名文档（打开文件时用它的位置替换它）。
    fn pristine_untitled(&self) -> Option<u32> {
        let docs = self.docs.borrow();
        if docs.len() != 1 {
            return None;
        }
        let d = &docs[0];
        let id = d.id;
        let untitled = d.path.is_none() && !d.meta_dirty;
        drop(docs);
        let empty = self.sci().is_some_and(|s| s.is_empty() && !s.is_modified());
        (untitled && empty && self.active.get() == id).then_some(id)
    }

    /// 关闭文档（询问保存）。返回是否已关闭。
    fn close_doc(&self, id: u32) -> bool {
        if self.is_dirty(id) {
            self.activate(id);
            let name = self.with_doc(id, |d| d.title()).unwrap_or_default();
            match dialogs::ask_yes_no_cancel(self.hwnd(), &format!("是否保存对“{name}”的修改？"))
            {
                Answer::Yes => {
                    if !self.save_active() {
                        return false;
                    }
                }
                Answer::No => {}
                Answer::Cancel => return false,
            }
        }
        self.close_doc_unchecked(id);
        true
    }

    /// 不询问直接关闭。最后一个文档关闭后自动新建空文档。
    fn close_doc_unchecked(&self, id: u32) {
        let Some(sci) = self.sci() else { return };
        let ids = self.doc_ids();
        let Some(idx) = ids.iter().position(|&x| x == id) else {
            return;
        };
        if self.active.get() == id {
            if ids.len() == 1 {
                self.new_document();
            } else {
                let next = if idx + 1 < ids.len() {
                    ids[idx + 1]
                } else {
                    ids[idx - 1]
                };
                self.activate(next);
            }
        }
        self.retarget_other_view(id);
        let (ptr, snap) = self
            .with_doc(id, |d| (d.ptr, d.snapshot.clone()))
            .unwrap_or((0, None));
        self.docs.borrow_mut().retain(|d| d.id != id);
        self.mru.borrow_mut().retain(|&x| x != id);
        self.unwatch(id);
        if ptr != 0 {
            sci.release_document(ptr);
        }
        if let Some(s) = snap {
            self.to_remove.borrow_mut().push(s);
        }
        self.arm_session_timer();
        self.refresh();
    }

    fn close_all(&self) -> bool {
        for id in self.doc_ids() {
            if !self.close_doc(id) {
                return false;
            }
        }
        true
    }

    /// 退出前逐个确认未保存的文档。
    fn confirm_close_all(&self) -> bool {
        for id in self.doc_ids() {
            if self.is_dirty(id) {
                self.activate(id);
                let name = self.with_doc(id, |d| d.title()).unwrap_or_default();
                match dialogs::ask_yes_no_cancel(
                    self.hwnd(),
                    &format!("是否保存对“{name}”的修改？"),
                ) {
                    Answer::Yes => {
                        if !self.save_active() {
                            return false;
                        }
                    }
                    Answer::No => {}
                    Answer::Cancel => return false,
                }
            }
        }
        true
    }

    // ---- 保存 ----

    fn save_active(&self) -> bool {
        match self.active_doc(|d| d.path.clone()).flatten() {
            Some(p) => self.write_active(&p),
            None => self.save_active_as(),
        }
    }

    fn save_active_as(&self) -> bool {
        let current = self.active_doc(|d| d.path.clone()).flatten();
        let suggested = current
            .clone()
            .or_else(|| self.active_doc(|d| PathBuf::from(format!("{}.txt", d.title()))));
        let Some(path) = dialogs::save_file(self.hwnd(), suggested.as_deref()) else {
            return false;
        };
        if !self.write_active(&path) {
            return false;
        }
        let new_lang = lang::index_of(lang::detect(Some(&path), ""));
        let lang_changed = self.active_doc_mut(|d| {
            d.path = Some(path.clone());
            let changed = d.lang == 0 && new_lang != 0;
            if changed {
                d.lang = new_lang;
            }
            changed
        });
        if lang_changed == Some(true) {
            self.set_active_language(new_lang);
        }
        self.refresh();
        true
    }

    fn save_all(&self) {
        let cur = self.active.get();
        for id in self.doc_ids() {
            if self.is_dirty(id) {
                self.activate(id);
                if !self.save_active() {
                    break;
                }
            }
        }
        self.activate(cur);
    }

    // ---- 编码 / 语言 / 换行符 ----

    fn reopen_with_encoding(&self, idx: usize) {
        let Some(choice) = encoding::catalog().get(idx).copied() else {
            return;
        };
        let id = self.active.get();
        if self.active_doc(|d| d.is_encrypted()) == Some(true) {
            self.show_message("加密文档始终使用 UTF-8，不能更改编码");
            return;
        }
        let Some(path) = self.active_doc(|d| d.path.clone()).flatten() else {
            self.show_message("未保存的文档不能重新打开，可以使用“转换为”");
            return;
        };
        if self.is_dirty(id)
            && dialogs::ask_yes_no_cancel(self.hwnd(), "重新载入会丢失未保存的修改，继续吗？")
                != Answer::Yes
        {
            return;
        }
        let Some(sci) = self.sci() else { return };
        let size = match docs::check_size(&path) {
            Ok(s) => s,
            Err(e) => {
                dialogs::error_box(self.hwnd(), &e);
                return;
            }
        };
        let Some(loader) = sci.create_loader(size as usize) else {
            return;
        };
        let job = LoadJob {
            path,
            forced: Some(choice.info.encoding),
            reuse: Some(id),
            goto: None,
            read_only: sci.is_read_only(),
        };
        self.start_load(job, loader, size);
    }

    fn convert_encoding(&self, idx: usize) {
        let Some(choice) = encoding::catalog().get(idx).copied() else {
            return;
        };
        self.active_doc_mut(|d| {
            if d.enc != choice.info {
                d.enc = choice.info;
                d.meta_dirty = true;
            }
        });
        self.refresh();
    }

    fn set_active_language(&self, idx: usize) {
        let Some(sci) = self.sci() else { return };
        let idx = idx.min(LANGS.len() - 1);
        let long = self.active_doc_mut(|d| {
            d.lang = idx;
            d.long_lines
        });
        if long == Some(true) {
            self.show_message("长行模式下不启用语法高亮");
            return;
        }
        theme::set_language(&sci, LANGS[idx].id, LANGS[idx].lexer);
        self.apply_all_views();
        self.refresh();
    }

    fn convert_eol(&self, eol: Eol) {
        let Some(sci) = self.sci() else { return };
        let mode = match eol {
            Eol::Crlf => SC_EOL_CRLF,
            Eol::Lf => SC_EOL_LF,
            Eol::Cr => SC_EOL_CR,
        };
        if editops::eol_of(&sci) != eol {
            self.active_doc_mut(|d| d.meta_dirty = true);
        }
        sci.send(SCI_CONVERTEOLS, mode as usize, 0);
        sci.send(SCI_SETEOLMODE, mode as usize, 0);
        self.refresh();
    }

    // ---- 视图 ----

    fn toggle_setting(&self, f: impl FnOnce(&mut Settings)) {
        {
            let mut s = self.settings.borrow_mut();
            f(&mut s);
            config::save_settings(&s);
        }
        self.apply_all_views();
        self.refresh();
    }

    fn set_zoom(&self, z: u32) {
        let z = z.clamp(config::ZOOM_MIN, config::ZOOM_MAX);
        if self.settings.borrow().zoom == z {
            return;
        }
        self.toggle_setting(|s| s.zoom = z);
    }

    fn zoom_step(&self, inc: bool) {
        let z = self.settings.borrow().zoom as f64;
        let nz = if inc {
            (z * 1.1).round()
        } else {
            (z / 1.1).round()
        };
        let nz = if (nz - z).abs() < 1.0 {
            if inc {
                z + 1.0
            } else {
                z - 1.0
            }
        } else {
            nz
        };
        self.set_zoom(nz as u32);
    }

    fn on_ctrl_wheel(&self, delta: i32) {
        let acc = self.wheel_accum.get() + delta;
        let steps = acc / WHEEL_DELTA;
        self.wheel_accum.set(acc % WHEEL_DELTA);
        for _ in 0..steps.abs() {
            self.zoom_step(steps > 0);
        }
    }

    fn on_drop(&self, hdrop: HDROP) {
        let mut files = Vec::new();
        unsafe {
            let n = DragQueryFileW(hdrop, u32::MAX, null_mut(), 0);
            for i in 0..n {
                let len = DragQueryFileW(hdrop, i, null_mut(), 0) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, buf.as_mut_ptr(), buf.len() as u32);
                files.push(PathBuf::from(from_wide(&buf)));
            }
            DragFinish(hdrop);
        }
        self.open_dropped(files);
    }

    /// 打开拖入的文件；文件夹加入工作区。
    fn open_dropped(&self, files: Vec<PathBuf>) {
        for f in files {
            if f.is_dir() {
                self.add_workspace_path(&f);
            } else if f.is_file() {
                self.open_path(&f, None, false);
            }
        }
    }

    // ---- 命令 ----

    fn command(&self, id: u16) {
        let Some(sci) = self.sci() else { return };
        let simple = |m: u32| {
            sci.send(m, 0, 0);
        };
        match id {
            IDM_FILE_NEW => {
                self.new_document();
            }
            IDM_FILE_OPEN => self.open_dialog(),
            IDM_FILE_SAVE => {
                self.save_active();
            }
            IDM_FILE_SAVE_AS => {
                self.save_active_as();
            }
            IDM_FILE_SAVE_ALL => self.save_all(),
            IDM_FILE_CLOSE => {
                self.close_doc(self.active.get());
            }
            IDM_FILE_CLOSE_ALL => {
                self.close_all();
            }
            IDM_FILE_EXIT => unsafe {
                PostMessageW(self.hwnd(), WM_CLOSE, 0, 0);
            },
            IDM_FILE_SAVE_ENCRYPTED => {
                self.save_encrypted_as();
            }
            IDM_SET_SNAPSHOTS => {
                self.toggle_setting(|s| s.snapshots = !s.snapshots);
                if self.snapshots_enabled() {
                    self.mark_changed();
                } else {
                    // 关闭快照：删除已有快照与会话
                    let names: Vec<String> = self
                        .docs
                        .borrow_mut()
                        .iter_mut()
                        .filter_map(|d| d.snapshot.take())
                        .collect();
                    for n in names {
                        self.store.remove(&n);
                    }
                    let _ = std::fs::remove_file(persist::session_path());
                }
            }
            IDM_SET_AUTO_RELOAD => self.toggle_setting(|s| s.auto_reload = !s.auto_reload),
            IDM_SET_GITIGNORE => {
                self.toggle_setting(|s| s.respect_gitignore = !s.respect_gitignore);
                self.file_index.borrow_mut().take();
                self.populate_workspace();
            }
            IDM_FILE_QUICK_OPEN => self.quick_show(),
            IDM_WS_ADD => self.add_workspace_folder(),
            IDM_VIEW_SIDEBAR => {
                self.toggle_setting(|s| s.sidebar_visible = !s.sidebar_visible);
                self.layout();
            }
            IDM_VIEW_SPLIT_NONE => self.set_split(0),
            IDM_VIEW_SPLIT_H => self.set_split(1),
            IDM_VIEW_SPLIT_V => self.set_split(2),
            IDM_VIEW_SWITCH_FOCUS => self.toggle_view_focus(),
            IDM_VIEW_MOVE_OTHER => self.show_active_in_other(),
            IDM_CTX_CLOSE..=IDM_CTX_REVEAL => self.on_list_context_command(id),
            IDM_TREE_OPEN..=IDM_TREE_REMOVE_ROOT => self.on_tree_command(id),
            IDM_RECENT_CLEAR => {
                self.recent.borrow_mut().clear();
                config::save_recent(&[]);
            }

            IDM_EDIT_UNDO => simple(SCI_UNDO),
            IDM_EDIT_REDO => simple(SCI_REDO),
            IDM_EDIT_CUT => simple(SCI_CUT),
            IDM_EDIT_COPY => simple(SCI_COPY),
            IDM_EDIT_PASTE => simple(SCI_PASTE),
            IDM_EDIT_DELETE => simple(SCI_CLEAR),
            IDM_EDIT_SELECT_ALL => simple(SCI_SELECTALL),
            IDM_EDIT_UPPER => simple(SCI_UPPERCASE),
            IDM_EDIT_LOWER => simple(SCI_LOWERCASE),
            IDM_EDIT_READ_ONLY => {
                sci.send(SCI_SETREADONLY, !sci.is_read_only() as usize, 0);
                self.refresh();
            }
            IDM_EDIT_LIST_CONTINUE => self.toggle_setting(|s| s.list_continue = !s.list_continue),

            IDM_LINE_DUPLICATE => simple(SCI_SELECTIONDUPLICATE),
            IDM_LINE_UP => simple(SCI_MOVESELECTEDLINESUP),
            IDM_LINE_DOWN => simple(SCI_MOVESELECTEDLINESDOWN),
            IDM_LINE_JOIN => {
                if sci.selection_start() == sci.selection_end() {
                    // 无选区：合并当前行与下一行
                    let line = sci.line_from_pos(sci.current_pos());
                    if line + 1 < sci.line_count() {
                        sci.send(SCI_SETTARGETRANGE, sci.line_start(line), sci.line_end(line + 1) as isize);
                        sci.send(SCI_LINESJOIN, 0, 0);
                    }
                } else {
                    sci.send(SCI_TARGETFROMSELECTION, 0, 0);
                    sci.send(SCI_LINESJOIN, 0, 0);
                }
            }
            IDM_LINE_SORT_ASC..=IDM_LINE_TRIM_LEADING => self.line_op(id),

            IDM_EOL_CRLF => self.convert_eol(Eol::Crlf),
            IDM_EOL_LF => self.convert_eol(Eol::Lf),
            IDM_EOL_CR => self.convert_eol(Eol::Cr),

            IDM_SEARCH_FIND | IDM_SEARCH_REPLACE => self.show_find(id == IDM_SEARCH_REPLACE),
            IDM_SEARCH_NEXT | IDM_SEARCH_PREV => {
                if self.find.borrow().pattern.is_empty() {
                    self.show_find(false);
                } else {
                    let st = self.find.borrow().clone();
                    let a = if id == IDM_SEARCH_NEXT { FindAction::Next } else { FindAction::Prev };
                    self.find_action(a, st);
                }
            }
            IDM_SEARCH_SEL_NEXT | IDM_SEARCH_SEL_PREV => self.find_selection(id == IDM_SEARCH_SEL_NEXT),
            IDM_SEARCH_CLEAR_MARKS => self.clear_marks(),
            IDM_SEARCH_GOTO => {
                let cur = sci.line_from_pos(sci.current_pos()) + 1;
                if let Some(l) = finddlg::ask_line(self.hwnd(), self.hinstance, cur, sci.line_count()) {
                    editops::goto_line_col(&sci, l, None);
                }
            }

            IDM_BM_TOGGLE => editops::toggle_bookmark_at_caret(&sci),
            IDM_BM_NEXT => {
                if !editops::goto_bookmark(&sci, true) {
                    self.show_message("没有书签");
                }
            }
            IDM_BM_PREV => {
                if !editops::goto_bookmark(&sci, false) {
                    self.show_message("没有书签");
                }
            }
            IDM_BM_CLEAR => editops::clear_bookmarks(&sci),
            IDM_BM_INVERT => editops::invert_bookmarks(&sci),
            IDM_BM_COPY => {
                let n = editops::copy_bookmarked(&sci);
                self.show_message(&format!("已复制 {n} 行"));
            }
            IDM_BM_CUT => {
                let n = editops::copy_bookmarked(&sci);
                editops::delete_lines_where_bookmarked(&sci, true);
                self.show_message(&format!("已剪切 {n} 行"));
            }
            IDM_BM_DELETE => {
                let n = editops::delete_lines_where_bookmarked(&sci, true);
                self.show_message(&format!("已删除 {n} 行"));
            }
            IDM_BM_DELETE_UNMARKED => {
                let n = editops::delete_lines_where_bookmarked(&sci, false);
                self.show_message(&format!("已删除 {n} 行"));
            }

            IDM_VIEW_WRAP => self.toggle_setting(|s| s.wrap = !s.wrap),
            IDM_VIEW_WHITESPACE => self.toggle_setting(|s| s.show_whitespace = !s.show_whitespace),
            IDM_VIEW_EOL => self.toggle_setting(|s| s.show_eol = !s.show_eol),
            IDM_VIEW_INDENT_GUIDES => self.toggle_setting(|s| s.indent_guides = !s.indent_guides),
            IDM_VIEW_LINE_NUMBERS => self.toggle_setting(|s| s.line_numbers = !s.line_numbers),
            IDM_VIEW_ZOOM_IN => self.zoom_step(true),
            IDM_VIEW_ZOOM_OUT => self.zoom_step(false),
            IDM_VIEW_ZOOM_RESET => self.set_zoom(100),
            IDM_VIEW_FOLD_ALL => {
                sci.send(SCI_FOLDALL, SC_FOLDACTION_CONTRACT as usize, 0);
            }
            IDM_VIEW_UNFOLD_ALL => {
                sci.send(SCI_FOLDALL, SC_FOLDACTION_EXPAND as usize, 0);
            }

            IDM_WIN_NEXT | IDM_WIN_PREV => {
                let ids = self.doc_ids();
                if ids.len() > 1 {
                    let i = ids.iter().position(|&x| x == self.active.get()).unwrap_or(0);
                    let n = ids.len();
                    let j = if id == IDM_WIN_NEXT { (i + 1) % n } else { (i + n - 1) % n };
                    self.activate(ids[j]);
                }
            }
            IDM_HELP_ABOUT => dialogs::info_box(
                self.hwnd(),
                &format!(
                    "HurshNote {}\n\n原生 Windows 文本编辑器\n编辑内核：Scintilla / Lexilla\n设置目录：{}{}\n\nMIT License",
                    env!("CARGO_PKG_VERSION"),
                    config::data_dir().display(),
                    if config::is_portable() { "（便携模式）" } else { "" }
                ),
            ),

            x if (IDM_ENC_REOPEN_BASE..IDM_ENC_REOPEN_BASE + 100).contains(&x) => {
                self.reopen_with_encoding((x - IDM_ENC_REOPEN_BASE) as usize)
            }
            x if (IDM_ENC_CONVERT_BASE..IDM_ENC_CONVERT_BASE + 100).contains(&x) => {
                self.convert_encoding((x - IDM_ENC_CONVERT_BASE) as usize)
            }
            x if (IDM_LANG_BASE..IDM_LANG_BASE + 100).contains(&x) => {
                self.set_active_language((x - IDM_LANG_BASE) as usize)
            }
            x if (IDM_ZOOM_BASE..IDM_ZOOM_BASE + 100).contains(&x) => {
                if let Some(z) = ZOOM_PRESETS.get((x - IDM_ZOOM_BASE) as usize) {
                    self.set_zoom(*z);
                }
            }
            x if (IDM_WINDOW_BASE..IDM_WINDOW_BASE + IDM_WINDOW_MAX).contains(&x) => {
                if let Some(&doc) = self.doc_ids().get((x - IDM_WINDOW_BASE) as usize) {
                    self.activate(doc);
                }
            }
            x if (IDM_RECENT_BASE..IDM_RECENT_BASE + IDM_RECENT_MAX).contains(&x) => {
                let p = self.recent.borrow().get((x - IDM_RECENT_BASE) as usize).cloned();
                if let Some(p) = p {
                    if p.exists() {
                        self.open_path(&p, None, false);
                    } else {
                        dialogs::error_box(self.hwnd(), &format!("文件不存在：\n{}", p.display()));
                        self.recent.borrow_mut().retain(|x| x != &p);
                        config::save_recent(&self.recent.borrow());
                    }
                }
            }
            _ => {}
        }
    }

    fn line_op(&self, id: u16) {
        let Some(sci) = self.sci() else { return };
        let sort = |descending, case_insensitive, numeric| {
            LineOp::Sort(SortOptions {
                descending,
                case_insensitive,
                numeric,
            })
        };
        let op = match id {
            IDM_LINE_SORT_ASC => sort(false, false, false),
            IDM_LINE_SORT_DESC => sort(true, false, false),
            IDM_LINE_SORT_ASC_CI => sort(false, true, false),
            IDM_LINE_SORT_DESC_CI => sort(true, true, false),
            IDM_LINE_SORT_NUM_ASC => sort(false, false, true),
            IDM_LINE_SORT_NUM_DESC => sort(true, false, true),
            IDM_LINE_UNIQUE => LineOp::Unique {
                adjacent_only: false,
            },
            IDM_LINE_UNIQUE_ADJ => LineOp::Unique {
                adjacent_only: true,
            },
            IDM_LINE_REMOVE_EMPTY => LineOp::RemoveEmpty {
                blank_counts: false,
            },
            IDM_LINE_REMOVE_BLANK => LineOp::RemoveEmpty { blank_counts: true },
            IDM_LINE_TRIM_TRAILING => LineOp::TrimTrailing,
            IDM_LINE_TRIM_LEADING => LineOp::TrimLeading,
            _ => return,
        };
        editops::apply_line_op(&sci, op);
    }

    // ---- 通知 ----

    fn on_notify(&self, lp: LPARAM) -> LRESULT {
        let hdr = unsafe { &*(lp as *const Notification) };
        if let Some(i) = self.view_index(hdr.hwnd_from) {
            if let Some(sci) = self.views[i].get() {
                self.on_editor_notify(i, &sci, hdr);
            }
            return 0;
        }
        let from = hdr.hwnd_from;
        if from == self.side_list.get() {
            return self.on_list_notify(lp).unwrap_or(0);
        }
        if from == self.side_tree.get() {
            return self.on_tree_notify(lp).unwrap_or(0);
        }
        if from == self.status.get() && hdr.code == NM_CLICK {
            let nm = unsafe { &*(lp as *const statusbar::NmMouse) };
            self.on_status_click(nm.item_spec);
        }
        0
    }

    fn on_editor_notify(&self, view: usize, sci: &Sci, n: &Notification) {
        let focused = view == self.focus.get();
        if n.code == SCN_FOCUSIN && !focused {
            // 用户点进了另一视图
            if self.docs.try_borrow_mut().is_ok() {
                self.focus_view(view);
            } else {
                unsafe { PostMessageW(self.hwnd(), WM_APP_FOCUS_VIEW, view, 0) };
            }
            return;
        }
        if !focused {
            // 非焦点视图：只维护显示相关的状态
            match n.code {
                SCN_UPDATEUI => {
                    editops::update_brace_match(sci);
                    if n.updated & SC_UPDATE_CONTENT as i32 != 0 {
                        let show = self
                            .settings
                            .try_borrow()
                            .map(|s| s.line_numbers)
                            .unwrap_or(true);
                        theme::update_line_number_width(sci, show);
                    }
                }
                SCN_SAVEPOINTREACHED | SCN_SAVEPOINTLEFT => {
                    let id = self.view_doc[view].get();
                    if let Ok(mut docs) = self.docs.try_borrow_mut() {
                        if let Some(d) = docs.iter_mut().find(|d| d.id == id) {
                            d.modified = n.code == SCN_SAVEPOINTLEFT;
                        }
                    }
                    self.request_refresh();
                }
                _ => {}
            }
            return;
        }
        match n.code {
            SCN_SAVEPOINTREACHED | SCN_SAVEPOINTLEFT => {
                let modified = n.code == SCN_SAVEPOINTLEFT;
                if let Ok(mut docs) = self.docs.try_borrow_mut() {
                    if let Some(d) = docs.iter_mut().find(|d| d.id == self.active.get()) {
                        d.modified = modified;
                    }
                }
                self.arm_snapshot_timer();
                self.request_refresh();
            }
            SCN_UPDATEUI => {
                editops::update_brace_match(sci);
                if n.updated & SC_UPDATE_CONTENT as i32 != 0 {
                    let show = self
                        .settings
                        .try_borrow()
                        .map(|s| s.line_numbers)
                        .unwrap_or(true);
                    theme::update_line_number_width(sci, show);
                    self.mark_changed();
                }
                self.update_status();
            }
            SCN_CHARADDED => {
                if editops::is_newline_char(sci, n.ch) {
                    let lang = self.active_doc(|d| d.lang).unwrap_or(0);
                    let list_lang = matches!(LANGS[lang].id, "text" | "markdown");
                    let enabled = self.settings.borrow().list_continue && list_lang;
                    editops::on_newline(sci, enabled);
                }
            }
            SCN_MARGINCLICK if n.margin as usize == theme::MARGIN_BOOKMARKS => {
                editops::toggle_bookmark(sci, sci.line_from_pos(n.position as usize));
            }
            _ => {}
        }
    }

    // ---- 菜单状态 ----

    fn on_init_menu(&self, menu: HMENU) {
        let Some(menus) = self.menus.get() else {
            return;
        };
        if menu == menus.recent {
            self.rebuild_recent_menu(menu);
        } else if menu == menus.window {
            self.rebuild_window_menu(menu);
        }
        self.update_checks(menus.bar);
    }

    fn update_checks(&self, menu: HMENU) {
        let Some(sci) = self.sci() else { return };
        let s = self.settings.borrow().clone();
        let check = |id: u16, on: bool| unsafe {
            CheckMenuItem(menu, id as u32, if on { MF_CHECKED } else { 0 });
        };
        check(IDM_VIEW_WRAP, s.wrap);
        check(IDM_VIEW_WHITESPACE, s.show_whitespace);
        check(IDM_VIEW_EOL, s.show_eol);
        check(IDM_VIEW_INDENT_GUIDES, s.indent_guides);
        check(IDM_VIEW_LINE_NUMBERS, s.line_numbers);
        check(IDM_EDIT_LIST_CONTINUE, s.list_continue);
        check(IDM_SET_SNAPSHOTS, s.snapshots);
        check(IDM_SET_AUTO_RELOAD, s.auto_reload);
        check(IDM_SET_GITIGNORE, s.respect_gitignore);
        check(IDM_VIEW_SIDEBAR, s.sidebar_visible);
        let split = self.split.get();
        check(IDM_VIEW_SPLIT_NONE, split == 0);
        check(IDM_VIEW_SPLIT_H, split == 1);
        check(IDM_VIEW_SPLIT_V, split == 2);
        check(IDM_EDIT_READ_ONLY, sci.is_read_only());
        let eol = editops::eol_of(&sci);
        check(IDM_EOL_CRLF, eol == Eol::Crlf);
        check(IDM_EOL_LF, eol == Eol::Lf);
        check(IDM_EOL_CR, eol == Eol::Cr);
        let (lang_idx, enc) = self
            .active_doc(|d| (d.lang, d.enc))
            .unwrap_or((0, EncodingInfo::UTF8));
        for i in 0..LANGS.len() {
            check(IDM_LANG_BASE + i as u16, i == lang_idx);
        }
        for (i, c) in encoding::catalog().iter().enumerate() {
            check(IDM_ENC_REOPEN_BASE + i as u16, c.info == enc);
            check(IDM_ENC_CONVERT_BASE + i as u16, c.info == enc);
        }
        for (i, z) in ZOOM_PRESETS.iter().enumerate() {
            check(IDM_ZOOM_BASE + i as u16, *z == s.zoom);
        }
    }

    fn rebuild_recent_menu(&self, menu: HMENU) {
        unsafe {
            for i in (0..GetMenuItemCount(menu)).rev() {
                DeleteMenu(menu, i as u32, MF_BYPOSITION);
            }
        }
        let recent = self.recent.borrow().clone();
        let add = |flags: u32, id: usize, text: &str| {
            let w = wide(text);
            unsafe { AppendMenuW(menu, flags, id, w.as_ptr()) };
        };
        if recent.is_empty() {
            add(MF_STRING | MF_GRAYED, 0, "（空）");
            return;
        }
        for (i, p) in recent.iter().take(IDM_RECENT_MAX as usize).enumerate() {
            let label = if i < 9 {
                format!("&{} {}", i + 1, p.display())
            } else {
                p.display().to_string()
            };
            add(
                MF_STRING,
                (IDM_RECENT_BASE + i as u16) as usize,
                &label.replace('&', "&&").replacen("&&", "&", 1),
            );
        }
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING, IDM_RECENT_CLEAR as usize, "清除列表");
    }

    fn rebuild_window_menu(&self, menu: HMENU) {
        unsafe {
            for i in (WINDOW_FIXED_ITEMS..GetMenuItemCount(menu)).rev() {
                DeleteMenu(menu, i as u32, MF_BYPOSITION);
            }
        }
        let active = self.active.get();
        let items: Vec<(u32, String, bool)> = self
            .docs
            .borrow()
            .iter()
            .map(|d| {
                let dirty = if d.id == active { false } else { d.is_dirty() };
                (d.id, d.title(), dirty)
            })
            .collect();
        for (i, (id, title, dirty)) in items.iter().take(IDM_WINDOW_MAX as usize).enumerate() {
            let dirty = *dirty || (*id == active && self.is_dirty(active));
            let label = format!(
                "{}{}{}",
                if i < 9 {
                    format!("&{} ", i + 1)
                } else {
                    String::new()
                },
                if dirty { "*" } else { "" },
                title.replace('&', "&&")
            );
            let w = wide(&label);
            let flags = MF_STRING | if *id == active { MF_CHECKED } else { 0 };
            unsafe {
                AppendMenuW(
                    menu,
                    flags,
                    (IDM_WINDOW_BASE + i as u16) as usize,
                    w.as_ptr(),
                )
            };
        }
    }

    fn popup_at_cursor(&self, menu: HMENU) {
        self.update_checks(menu);
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut pt);
            TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, self.hwnd(), null());
            DestroyMenu(menu);
        }
    }

    fn on_status_click(&self, part: usize) {
        let menu = match part {
            PART_LANG => language_menu(),
            PART_EOL => eol_menu(),
            PART_ENC => {
                let m = unsafe { CreatePopupMenu() };
                let r = encoding_menu(IDM_ENC_REOPEN_BASE);
                let c = encoding_menu(IDM_ENC_CONVERT_BASE);
                let (wr, wc) = (wide("重新打开为"), wide("转换为"));
                unsafe {
                    AppendMenuW(m, MF_POPUP, r as usize, wr.as_ptr());
                    AppendMenuW(m, MF_POPUP, c as usize, wc.as_ptr());
                }
                m
            }
            PART_ZOOM => zoom_menu(),
            PART_STATE => {
                self.command(IDM_EDIT_READ_ONLY);
                return;
            }
            _ => return,
        };
        self.popup_at_cursor(menu);
    }

    fn show_context_menu(&self, lp: LPARAM) {
        let Some(sci) = self.sci() else { return };
        let (mut x, mut y) = point_from_lparam(lp);
        if lp == -1 {
            let pos = sci.current_pos();
            let mut pt = POINT {
                x: sci.send(SCI_POINTXFROMPOSITION, 0, pos as isize) as i32,
                y: sci.send(SCI_POINTYFROMPOSITION, 0, pos as isize) as i32,
            };
            unsafe { ClientToScreen(sci.hwnd(), &mut pt) };
            x = pt.x;
            y = pt.y;
        }
        let menu = build_context_menu();
        self.update_checks(menu);
        unsafe {
            if sci.is_read_only() {
                for id in [IDM_EDIT_CUT, IDM_EDIT_PASTE, IDM_EDIT_DELETE] {
                    EnableMenuItem(menu, id as u32, MF_GRAYED);
                }
            }
            TrackPopupMenu(menu, TPM_RIGHTBUTTON, x, y, 0, self.hwnd(), null());
            DestroyMenu(menu);
        }
    }

    // ---- 查找 ----

    fn show_find(&self, replace: bool) {
        let Some(sci) = self.sci() else { return };
        let (s, e) = (sci.selection_start(), sci.selection_end());
        let sel = if s != e && e - s < 500 {
            let t = String::from_utf8_lossy(&sci.text_range(s, e)).into_owned();
            (!t.contains(['\r', '\n'])).then_some(t)
        } else {
            None
        };
        let st = self.find.borrow().clone();
        finddlg::show(
            self.find_dlg.get(),
            self.hwnd(),
            &st,
            sel.as_deref(),
            replace,
        );
    }

    fn find_selection(&self, forward: bool) {
        let Some(sci) = self.sci() else { return };
        let (mut s, mut e) = (sci.selection_start(), sci.selection_end());
        if s == e {
            s = sci.send(SCI_WORDSTARTPOSITION, s, 1) as usize;
            e = sci.send(SCI_WORDENDPOSITION, e, 1) as usize;
        }
        if s == e {
            return;
        }
        let text = String::from_utf8_lossy(&sci.text_range(s, e)).into_owned();
        let mut st = self.find.borrow().clone();
        st.pattern = text;
        st.opts.mode = hn_search::Mode::Normal;
        sci.set_sel(s, e);
        self.find_action(
            if forward {
                FindAction::Next
            } else {
                FindAction::Prev
            },
            st,
        );
    }

    fn report(&self, text: &str) {
        finddlg::set_status(self.find_dlg.get(), text);
        self.show_message(text);
    }

    /// 执行查找对话框或快捷键触发的动作。
    pub fn find_action(&self, action: FindAction, st: FindState) {
        let Some(sci) = self.sci() else { return };
        *self.find.borrow_mut() = st.clone();
        if action == FindAction::ClearMarks {
            self.clear_marks();
            self.report("已清除标记");
            return;
        }
        let searcher = match Searcher::new(&st.pattern, &st.opts) {
            Ok(s) => s,
            Err(e) => {
                self.report(&e.to_string());
                return;
            }
        };
        let (sel_s, sel_e) = (sci.selection_start(), sci.selection_end());
        let scope = if st.in_selection && sel_s != sel_e {
            sel_s..sel_e
        } else {
            0..sci.len()
        };

        enum Outcome {
            Found(usize, usize, bool),
            NotFound,
            All(Vec<(usize, usize)>),
            Replacements(Vec<(usize, usize, String)>),
            Error(String),
        }

        let outcome = sci.with_text(|bytes| {
            let Ok(text) = std::str::from_utf8(bytes) else {
                return Outcome::Error("文档包含无效的 UTF-8 字节，无法搜索".into());
            };
            let len = text.len();
            let r = (|| -> Result<Outcome, hn_search::Error> {
                Ok(match action {
                    FindAction::Next | FindAction::Replace => {
                        if action == FindAction::Replace && sel_s != sel_e {
                            if let Some((a, b)) = searcher.find_forward(text, sel_s, sel_e)? {
                                if a == sel_s && b == sel_e {
                                    let rep = searcher.replacement(text, (a, b), &st.replace)?;
                                    return Ok(Outcome::Replacements(vec![(a, b, rep)]));
                                }
                            }
                        }
                        match searcher.find_forward(text, sel_e, len)? {
                            Some((a, b)) => Outcome::Found(a, b, false),
                            None if st.wrap => match searcher.find_forward(text, 0, len)? {
                                Some((a, b)) => Outcome::Found(a, b, true),
                                None => Outcome::NotFound,
                            },
                            None => Outcome::NotFound,
                        }
                    }
                    FindAction::Prev => match searcher.find_backward(text, 0..len, sel_s)? {
                        Some((a, b)) => Outcome::Found(a, b, false),
                        None if st.wrap => match searcher.find_backward(text, 0..len, len)? {
                            Some((a, b)) => Outcome::Found(a, b, true),
                            None => Outcome::NotFound,
                        },
                        None => Outcome::NotFound,
                    },
                    FindAction::Count | FindAction::MarkAll => {
                        Outcome::All(searcher.find_all(text, scope.clone())?)
                    }
                    FindAction::ReplaceAll => {
                        let spans = searcher.find_all(text, scope.clone())?;
                        let mut reps = Vec::with_capacity(spans.len());
                        for sp in spans {
                            reps.push((sp.0, sp.1, searcher.replacement(text, sp, &st.replace)?));
                        }
                        Outcome::Replacements(reps)
                    }
                    FindAction::ClearMarks => Outcome::NotFound,
                })
            })();
            r.unwrap_or_else(|e| Outcome::Error(e.to_string()))
        });

        match outcome {
            Outcome::Error(e) => self.report(&e),
            Outcome::NotFound => {
                self.report(&format!("找不到“{}”", st.pattern));
                unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0) };
            }
            Outcome::Found(a, b, wrapped) => {
                sci.select_and_show(a, b);
                self.report(if wrapped {
                    "已从另一端继续查找"
                } else {
                    ""
                });
            }
            Outcome::All(spans) => {
                if action == FindAction::MarkAll {
                    self.clear_marks();
                    sci.send(SCI_SETINDICATORCURRENT, theme::INDIC_FIND, 0);
                    for (a, b) in &spans {
                        sci.send(SCI_INDICATORFILLRANGE, *a, (*b - *a) as isize);
                    }
                    self.report(&format!("已标记 {} 处", spans.len()));
                } else {
                    self.report(&format!("共 {} 处匹配", spans.len()));
                }
            }
            Outcome::Replacements(reps) => {
                if sci.is_read_only() {
                    self.report("文档为只读");
                    return;
                }
                let n = reps.len();
                if n == 0 {
                    self.report(&format!("找不到“{}”", st.pattern));
                    return;
                }
                let single = action == FindAction::Replace;
                sci.undo_group(|| {
                    for (a, b, rep) in reps.iter().rev() {
                        sci.replace_range(*a, *b, rep.as_bytes());
                    }
                });
                if single {
                    let (a, _, rep) = &reps[0];
                    sci.set_sel(a + rep.len(), a + rep.len());
                    self.find_action(FindAction::Next, st);
                    self.report("已替换 1 处");
                } else {
                    self.report(&format!("已替换 {n} 处"));
                }
            }
        }
    }

    fn clear_marks(&self) {
        if let Some(sci) = self.sci() {
            sci.send(SCI_SETINDICATORCURRENT, theme::INDIC_FIND, 0);
            sci.send(SCI_INDICATORCLEARRANGE, 0, sci.len() as isize);
        }
    }

    // ---- 标题与状态栏 ----

    fn show_message(&self, text: &str) {
        *self.message.borrow_mut() = text.to_string();
        unsafe { KillTimer(self.hwnd(), TIMER_MESSAGE) };
        if !text.is_empty() {
            unsafe { SetTimer(self.hwnd(), TIMER_MESSAGE, 6000, None) };
        }
        self.update_status();
    }

    /// 在通知处理中途可能处于借用状态，此时改为投递消息稍后刷新。
    fn request_refresh(&self) {
        if self.docs.try_borrow_mut().is_ok() {
            self.refresh();
        } else {
            unsafe { PostMessageW(self.hwnd(), WM_APP_REFRESH, 0, 0) };
        }
    }

    pub fn refresh(&self) {
        self.update_title();
        self.update_status();
        self.refresh_open_list();
    }

    fn update_title(&self) {
        let id = self.active.get();
        let Some((title, path, missing)) =
            self.active_doc(|d| (d.title(), d.path.clone(), d.missing))
        else {
            return;
        };
        let dirty = self.is_dirty(id);
        let ro = self.sci().is_some_and(|s| s.is_read_only());
        let text = format!(
            "{}{}{}{} - HurshNote",
            if dirty { "*" } else { "" },
            path.map(|p| p.display().to_string()).unwrap_or(title),
            if missing { " [磁盘上已删除]" } else { "" },
            if ro { " [只读]" } else { "" }
        );
        let w = wide(&text);
        unsafe { SetWindowTextW(self.hwnd(), w.as_ptr()) };
    }

    fn update_status(&self) {
        let sb = self.status.get();
        let Some(sci) = self.sci() else { return };
        if sb.is_null() {
            return;
        }
        let Ok(docs) = self.docs.try_borrow() else {
            return;
        };
        let Some(doc) = docs.iter().find(|d| d.id == self.active.get()) else {
            return;
        };
        let (lang, enc, lossy, binary, long_lines, meta_dirty, encrypted) = (
            doc.lang,
            doc.enc,
            doc.lossy,
            doc.binary,
            doc.long_lines,
            doc.meta_dirty,
            doc.key.is_some(),
        );
        drop(docs);

        let pos = sci.current_pos();
        let line = sci.line_from_pos(pos) + 1;
        let col = sci.send(SCI_GETCOLUMN, pos, 0) + 1;
        let nsel = sci.send(SCI_GETSELECTIONS, 0, 0).max(1) as usize;
        let mut chars = 0isize;
        for i in 0..nsel {
            let a = sci.send(SCI_GETSELECTIONNSTART, i, 0);
            let b = sci.send(SCI_GETSELECTIONNEND, i, 0);
            if b > a {
                chars += sci.send(SCI_COUNTCHARACTERS, a as usize, b);
            }
        }
        let sel = if chars == 0 {
            String::new()
        } else if nsel > 1 {
            format!("{nsel} 处选区，共 {chars} 字符")
        } else {
            let l1 = sci.line_from_pos(sci.selection_start());
            let l2 = sci.line_from_pos(sci.selection_end());
            if l2 > l1 {
                format!("选中 {chars} 字符（{} 行）", l2 - l1 + 1)
            } else {
                format!("选中 {chars} 字符")
            }
        };

        let msg = self.message.borrow().clone();
        let mut flags = Vec::new();
        if encrypted {
            flags.push("已加密");
        }
        if binary {
            flags.push("二进制");
        }
        if long_lines {
            flags.push("长行模式");
        }
        if lossy {
            flags.push("解码有损");
        }
        let part0 = if msg.is_empty() {
            flags.join(" · ")
        } else {
            msg
        };

        let dirty = sci.is_modified() || meta_dirty;
        let state = match (dirty, sci.is_read_only()) {
            (true, true) => "已修改 · 只读",
            (true, false) => "已修改",
            (false, true) => "只读",
            (false, false) => "",
        };

        set(sb, PART_MESSAGE, &part0);
        set(sb, PART_POS, &format!("行 {line}，列 {col}"));
        set(sb, PART_SEL, &sel);
        set(sb, PART_LANG, LANGS[lang].name);
        set(sb, PART_EOL, editops::eol_of(&sci).label());
        set(sb, PART_ENC, &enc.label());
        set(
            sb,
            PART_ZOOM,
            &format!(
                "{}%",
                self.settings.try_borrow().map(|s| s.zoom).unwrap_or(100)
            ),
        );
        set(sb, PART_STATE, state);
    }

    fn save_window_state(&self) {
        let mut wp: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if unsafe { GetWindowPlacement(self.hwnd(), &mut wp) } != 0 && self.selftest_out.is_none() {
            let r = wp.rcNormalPosition;
            let mut s = self.settings.borrow_mut();
            s.window = Some((r.left, r.top, r.right, r.bottom));
            s.maximized = wp.showCmd == SW_SHOWMAXIMIZED;
            config::save_settings(&s);
        }
    }

    // ---- 自测 ----

    fn run_selftest(&self) {
        let startup_ms = crate::selftest::process_age_ms();
        let report = crate::selftest::run(self, startup_ms);
        if let Some(out) = &self.selftest_out {
            let _ = std::fs::write(out, report);
        }
        // 清除修改状态，避免退出时弹出保存提示
        for id in self.doc_ids() {
            self.with_doc_mut(id, |d| {
                d.modified = false;
                d.meta_dirty = false;
            });
        }
        if let Some(sci) = self.sci() {
            sci.set_save_point();
        }
        unsafe { DestroyWindow(self.hwnd()) };
    }

    // 供自测使用的内部接口
    pub(crate) fn st_sci(&self) -> Option<Sci> {
        self.sci()
    }
    pub(crate) fn st_doc_count(&self) -> usize {
        self.docs.borrow().len()
    }
    pub(crate) fn st_close_active(&self) {
        if let Some(sci) = self.sci() {
            sci.set_save_point();
        }
        self.active_doc_mut(|d| d.meta_dirty = false);
        self.close_doc_unchecked(self.active.get());
    }
    pub(crate) fn st_active_lang(&self) -> &'static str {
        LANGS[self.active_doc(|d| d.lang).unwrap_or(0)].id
    }
    pub(crate) fn st_set_language(&self, id: &str) {
        if let Some(l) = lang::by_id(id) {
            self.set_active_language(lang::index_of(l));
        }
    }
    pub(crate) fn st_zoom(&self) -> u32 {
        self.settings.borrow().zoom
    }
    pub(crate) fn st_command(&self, id: u16) {
        self.command(id);
    }
    pub(crate) fn st_write_active(&self, path: &Path) -> bool {
        let ok = self.write_active(path);
        if ok {
            self.active_doc_mut(|d| d.path = Some(path.to_path_buf()));
        }
        ok
    }
    pub(crate) fn st_install_encrypted(
        &self,
        path: &Path,
        key: hn_crypto::Key,
        payload: hn_core::payload::Payload,
        stamp: hn_core::stamp::DiskStamp,
    ) -> bool {
        self.install_encrypted(path, key, payload, stamp, None, false)
            .is_some()
    }
    pub(crate) fn st_active_encrypted(&self) -> bool {
        self.active_doc(|d| d.is_encrypted()).unwrap_or(false)
    }
    /// 自测：把工作区设为 `dir`（不写设置文件），返回根节点展开后的子项。
    pub(crate) fn st_workspace_tree(&self, dir: &Path) -> Vec<String> {
        {
            let mut s = self.settings.borrow_mut();
            s.workspace = vec![dir.to_path_buf()];
            s.respect_gitignore = true;
        }
        self.populate_workspace();
        self.st_expand_first_root()
    }
    /// 自测：同步建立索引后执行快速打开查询。
    pub(crate) fn st_quick(&self, query: &str) -> Vec<PathBuf> {
        let roots = self.settings.borrow().workspace.clone();
        *self.file_index.borrow_mut() = Some(quick::build_index(roots, true));
        self.st_quick_query(query)
    }
    pub(crate) fn st_clear_workspace(&self) {
        self.settings.borrow_mut().workspace.clear();
        self.file_index.borrow_mut().take();
        self.populate_workspace();
    }
    pub(crate) fn st_split(&self, mode: u8) {
        self.set_split(mode);
    }
    pub(crate) fn st_toggle_focus(&self) {
        self.toggle_view_focus();
    }
    pub(crate) fn st_active_id(&self) -> u32 {
        self.active.get()
    }
    /// 自测：(焦点视图, 视图 0 文档, 视图 1 文档, 视图 0 指针, 视图 1 指针)
    pub(crate) fn st_views(&self) -> (usize, u32, u32, isize, isize) {
        let ptr = |i: usize| self.views[i].get().map(|s| s.doc_pointer()).unwrap_or(0);
        (
            self.focus.get(),
            self.view_doc[0].get(),
            self.view_doc[1].get(),
            ptr(0),
            ptr(1),
        )
    }
}

/// 去掉 canonicalize 产生的 `\\?\` 前缀（仅限普通盘符路径）。
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if rest.as_bytes().get(1) == Some(&b':') {
            return PathBuf::from(rest);
        }
    }
    p.to_path_buf()
}
