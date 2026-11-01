//! "在文件中查找"对话框（非模态）。只负责界面，搜索与替换由 `app` 执行。

use std::cell::Cell;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateDialogIndirectParamW, GetWindowRect, SetWindowPos, ShowWindow, DLGTEMPLATE,
};

use crate::dlg::{self, Template};
use crate::win::*;

pub const IDC_FIND: u16 = 301;
pub const IDC_REPLACE: u16 = 302;
pub const IDC_DIR: u16 = 303;
pub const IDC_BROWSE: u16 = 304;
pub const IDC_INCLUDE: u16 = 305;
pub const IDC_EXCLUDE: u16 = 306;
pub const IDC_CASE: u16 = 307;
pub const IDC_WORD: u16 = 308;
pub const IDC_REGEX: u16 = 309;
pub const IDC_GITIGNORE: u16 = 310;
pub const IDC_HIDDEN: u16 = 311;
pub const IDC_REPLACE_ALL: u16 = 312;
pub const IDC_STOP: u16 = 313;
pub const IDC_STATUS: u16 = 314;

/// 对话框触发的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirAction {
    Find,
    Replace,
    Stop,
    Browse,
}

/// 对话框中的条件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirForm {
    pub pattern: String,
    pub replace: String,
    /// 一个或多个目录（分号分隔）
    pub dir: String,
    pub include: String,
    pub exclude: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub gitignore: bool,
    pub hidden: bool,
}

impl DirForm {
    pub fn roots(&self) -> Vec<PathBuf> {
        self.dir
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    pub fn options(&self) -> hn_search::Options {
        hn_search::Options {
            mode: if self.regex {
                hn_search::Mode::Regex
            } else {
                hn_search::Mode::Normal
            },
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            dot_all: false,
        }
    }
}

fn template() -> Vec<u32> {
    let bx = 252;
    let bw = 66;
    let ew = 180;
    Template::new(
        "在文件中查找",
        WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME,
        326,
        150,
    )
    .label("查找内容(&N)：", 0xFFFF, 7, 9, 58, 9)
    .edit(IDC_FIND, 66, 7, ew, 13, 0)
    .label("替换为(&P)：", 0xFFFF, 7, 27, 58, 9)
    .edit(IDC_REPLACE, 66, 25, ew, 13, 0)
    .label("目录(&D)：", 0xFFFF, 7, 45, 58, 9)
    .edit(IDC_DIR, 66, 43, ew - 22, 13, 0)
    .button("…", IDC_BROWSE, 66 + ew - 20, 43, 20, 13, false)
    .label("文件类型(&T)：", 0xFFFF, 7, 63, 58, 9)
    .edit(IDC_INCLUDE, 66, 61, ew, 13, 0)
    .label("排除(&X)：", 0xFFFF, 7, 81, 58, 9)
    .edit(IDC_EXCLUDE, 66, 79, ew, 13, 0)
    .checkbox("区分大小写(&C)", IDC_CASE, 7, 98, 80, 10)
    .checkbox("全词匹配(&W)", IDC_WORD, 90, 98, 76, 10)
    .checkbox("正则表达式(&G)", IDC_REGEX, 170, 98, 80, 10)
    .checkbox("遵守 .gitignore(&I)", IDC_GITIGNORE, 7, 111, 80, 10)
    .checkbox("包含隐藏文件(&H)", IDC_HIDDEN, 90, 111, 80, 10)
    .button("查找全部", IDOK as u16, bx, 7, bw, 14, true)
    .button("全部替换(&A)…", IDC_REPLACE_ALL, bx, 24, bw, 14, false)
    .button("停止(&S)", IDC_STOP, bx, 41, bw, 14, false)
    .button("关闭", IDCANCEL as u16, bx, 58, bw, 14, false)
    .label("", IDC_STATUS, 7, 128, 312, 18)
    .build()
}

/// 创建（隐藏的）对话框。
pub fn create(owner: HWND, hinstance: *mut core::ffi::c_void) -> HWND {
    let t = template();
    unsafe {
        CreateDialogIndirectParamW(
            hinstance,
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(dir_proc),
            0,
        )
    }
}

thread_local! {
    static FIRST_SHOW: Cell<bool> = const { Cell::new(true) };
}

pub fn load(h: HWND, f: &DirForm) {
    dlg::set_text(h, IDC_FIND, &f.pattern);
    dlg::set_text(h, IDC_REPLACE, &f.replace);
    dlg::set_text(h, IDC_DIR, &f.dir);
    dlg::set_text(h, IDC_INCLUDE, &f.include);
    dlg::set_text(h, IDC_EXCLUDE, &f.exclude);
    dlg::set_checked(h, IDC_CASE, f.case_sensitive);
    dlg::set_checked(h, IDC_WORD, f.whole_word);
    dlg::set_checked(h, IDC_REGEX, f.regex);
    dlg::set_checked(h, IDC_GITIGNORE, f.gitignore);
    dlg::set_checked(h, IDC_HIDDEN, f.hidden);
}

pub fn read(h: HWND) -> DirForm {
    DirForm {
        pattern: dlg::get_text(h, IDC_FIND),
        replace: dlg::get_text(h, IDC_REPLACE),
        dir: dlg::get_text(h, IDC_DIR).trim().to_string(),
        include: dlg::get_text(h, IDC_INCLUDE),
        exclude: dlg::get_text(h, IDC_EXCLUDE),
        case_sensitive: dlg::is_checked(h, IDC_CASE),
        whole_word: dlg::is_checked(h, IDC_WORD),
        regex: dlg::is_checked(h, IDC_REGEX),
        gitignore: dlg::is_checked(h, IDC_GITIGNORE),
        hidden: dlg::is_checked(h, IDC_HIDDEN),
    }
}

/// 显示对话框（首次显示放在主窗口中上方），聚焦到查找框。
pub fn show(h: HWND, owner: HWND, f: &DirForm) {
    load(h, f);
    unsafe {
        if FIRST_SHOW.with(|f| f.replace(false)) {
            let mut o: RECT = std::mem::zeroed();
            let mut d: RECT = std::mem::zeroed();
            GetWindowRect(owner, &mut o);
            GetWindowRect(h, &mut d);
            let w = d.right - d.left;
            SetWindowPos(
                h,
                std::ptr::null_mut(),
                o.left + ((o.right - o.left) - w) / 2,
                o.top + 90,
                0,
                0,
                0x0001 | 0x0004,
            );
        }
        ShowWindow(h, SW_SHOW);
        SetFocus(dlg::item(h, IDC_FIND));
    }
    dlg::select_all(h, IDC_FIND);
}

pub fn set_status(h: HWND, text: &str) {
    if !h.is_null() {
        dlg::set_text(h, IDC_STATUS, text);
    }
}

unsafe extern "system" fn dir_proc(h: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> isize {
    let r = crate::crash::guard(|| match msg {
        WM_INITDIALOG => 1,
        WM_COMMAND => {
            let id = loword(wp);
            let action = match id as i32 {
                IDOK => Some(DirAction::Find),
                IDCANCEL => {
                    unsafe { ShowWindow(h, SW_HIDE) };
                    return 1;
                }
                _ => match id {
                    IDC_REPLACE_ALL => Some(DirAction::Replace),
                    IDC_STOP => Some(DirAction::Stop),
                    IDC_BROWSE => Some(DirAction::Browse),
                    _ => None,
                },
            };
            match action {
                Some(a) => {
                    let f = read(h);
                    if let Some(app) = crate::app::app() {
                        app.dir_action(a, f);
                    }
                    1
                }
                None => 0,
            }
        }
        WM_CLOSE => {
            unsafe { ShowWindow(h, SW_HIDE) };
            1
        }
        _ => 0,
    });
    r.unwrap_or(0)
}
