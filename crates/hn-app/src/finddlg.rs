//! 查找/替换对话框（非模态）与跳转到行对话框（模态）。

use std::cell::Cell;

use hn_search::{Mode, Options};
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateDialogIndirectParamW, DialogBoxIndirectParamW, EndDialog, GetWindowRect, SetWindowPos,
    ShowWindow, DLGTEMPLATE,
};

use crate::dlg::{self, Template};
use crate::win::*;

pub const IDC_FIND: u16 = 101;
pub const IDC_REPLACE: u16 = 102;
pub const IDC_CASE: u16 = 103;
pub const IDC_WORD: u16 = 104;
pub const IDC_INSEL: u16 = 105;
pub const IDC_WRAP: u16 = 106;
pub const IDC_NORMAL: u16 = 107;
pub const IDC_EXTENDED: u16 = 108;
pub const IDC_REGEX: u16 = 109;
pub const IDC_DOTALL: u16 = 110;
pub const IDC_PREV: u16 = 111;
pub const IDC_COUNT: u16 = 112;
pub const IDC_MARK: u16 = 113;
pub const IDC_CLEAR: u16 = 114;
pub const IDC_REPLACE_ONE: u16 = 115;
pub const IDC_REPLACE_ALL: u16 = 116;
pub const IDC_STATUS: u16 = 117;

/// 查找对话框触发的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindAction {
    Next,
    Prev,
    Count,
    MarkAll,
    ClearMarks,
    Replace,
    ReplaceAll,
}

/// 当前查找条件（对话框关闭后 F3 仍使用）。
#[derive(Debug, Clone, Default)]
pub struct FindState {
    pub pattern: String,
    pub replace: String,
    pub opts: Options,
    pub wrap: bool,
    pub in_selection: bool,
}

fn template() -> Vec<u32> {
    let bx = 232; // 按钮列 x
    let bw = 70;
    Template::new(
        "查找与替换",
        WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME,
        310,
        134,
    )
    .label("查找内容(&N)：", 0xFFFF, 7, 9, 52, 9)
    .edit(IDC_FIND, 62, 7, 162, 13, 0)
    .label("替换为(&P)：", 0xFFFF, 7, 27, 52, 9)
    .edit(IDC_REPLACE, 62, 25, 162, 13, 0)
    .checkbox("区分大小写(&C)", IDC_CASE, 7, 46, 100, 10)
    .checkbox("全词匹配(&W)", IDC_WORD, 7, 59, 100, 10)
    .checkbox("仅在选区内(&I)", IDC_INSEL, 7, 72, 100, 10)
    .checkbox("循环查找(&D)", IDC_WRAP, 7, 85, 100, 10)
    .group("查找模式", 112, 43, 112, 64)
    .radio("普通(&L)", IDC_NORMAL, 118, 54, 100, 10, true)
    .radio(
        "扩展（\\n \\r \\t \\x）(&E)",
        IDC_EXTENDED,
        118,
        67,
        104,
        10,
        false,
    )
    .radio("正则表达式(&G)", IDC_REGEX, 118, 80, 100, 10, false)
    .checkbox(". 匹配换行符", IDC_DOTALL, 128, 93, 90, 10)
    .button("查找下一个", IDOK as u16, bx, 7, bw, 14, true)
    .button("查找上一个", IDC_PREV, bx, 24, bw, 14, false)
    .button("计数(&T)", IDC_COUNT, bx, 41, bw, 14, false)
    .button("标记全部(&M)", IDC_MARK, bx, 58, bw, 14, false)
    .button("清除标记", IDC_CLEAR, bx, 75, bw, 14, false)
    .button("替换(&R)", IDC_REPLACE_ONE, bx, 92, bw, 14, false)
    .button("全部替换(&A)", IDC_REPLACE_ALL, bx, 109, bw, 14, false)
    .label("", IDC_STATUS, 7, 115, 217, 10)
    .build()
}

/// 创建（隐藏的）查找对话框。
pub fn create(owner: HWND, hinstance: *mut core::ffi::c_void) -> HWND {
    let t = template();
    unsafe {
        CreateDialogIndirectParamW(
            hinstance,
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(find_proc),
            0,
        )
    }
}

/// 显示对话框并聚焦到查找框（`replace` 为真时聚焦替换框）。
pub fn show(dlg_hwnd: HWND, owner: HWND, state: &FindState, initial: Option<&str>, replace: bool) {
    load_state(dlg_hwnd, state);
    if let Some(t) = initial {
        dlg::set_text(dlg_hwnd, IDC_FIND, t);
    }
    dlg::set_text(dlg_hwnd, IDC_STATUS, "");
    unsafe {
        // 首次显示时放在主窗口右上角
        if FIRST_SHOW.with(|f| f.replace(false)) {
            let mut o: RECT = std::mem::zeroed();
            let mut d: RECT = std::mem::zeroed();
            GetWindowRect(owner, &mut o);
            GetWindowRect(dlg_hwnd, &mut d);
            let w = d.right - d.left;
            SetWindowPos(
                dlg_hwnd,
                std::ptr::null_mut(),
                o.right - w - 40,
                o.top + 90,
                0,
                0,
                0x0001 | 0x0004,
            );
        }
        ShowWindow(dlg_hwnd, SW_SHOW);
    }
    let focus = if replace { IDC_REPLACE } else { IDC_FIND };
    unsafe { SetFocus(dlg::item(dlg_hwnd, focus)) };
    dlg::select_all(dlg_hwnd, focus);
}

thread_local! {
    static FIRST_SHOW: Cell<bool> = const { Cell::new(true) };
}

fn load_state(h: HWND, s: &FindState) {
    dlg::set_text(h, IDC_FIND, &s.pattern);
    dlg::set_text(h, IDC_REPLACE, &s.replace);
    dlg::set_checked(h, IDC_CASE, s.opts.case_sensitive);
    dlg::set_checked(h, IDC_WORD, s.opts.whole_word);
    dlg::set_checked(h, IDC_INSEL, s.in_selection);
    dlg::set_checked(h, IDC_WRAP, s.wrap);
    dlg::set_checked(h, IDC_NORMAL, s.opts.mode == Mode::Normal);
    dlg::set_checked(h, IDC_EXTENDED, s.opts.mode == Mode::Extended);
    dlg::set_checked(h, IDC_REGEX, s.opts.mode == Mode::Regex);
    dlg::set_checked(h, IDC_DOTALL, s.opts.dot_all);
}

/// 从对话框读取当前条件。
pub fn read_state(h: HWND) -> FindState {
    let mode = if dlg::is_checked(h, IDC_REGEX) {
        Mode::Regex
    } else if dlg::is_checked(h, IDC_EXTENDED) {
        Mode::Extended
    } else {
        Mode::Normal
    };
    FindState {
        pattern: dlg::get_text(h, IDC_FIND),
        replace: dlg::get_text(h, IDC_REPLACE),
        opts: Options {
            mode,
            case_sensitive: dlg::is_checked(h, IDC_CASE),
            whole_word: dlg::is_checked(h, IDC_WORD),
            dot_all: dlg::is_checked(h, IDC_DOTALL),
        },
        wrap: dlg::is_checked(h, IDC_WRAP),
        in_selection: dlg::is_checked(h, IDC_INSEL),
    }
}

pub fn set_status(h: HWND, text: &str) {
    if !h.is_null() {
        dlg::set_text(h, IDC_STATUS, text);
    }
}

unsafe extern "system" fn find_proc(h: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> isize {
    let r = crate::crash::guard(|| match msg {
        WM_INITDIALOG => 1,
        WM_COMMAND => {
            let id = loword(wp);
            let action = match id as i32 {
                IDOK => Some(FindAction::Next),
                IDCANCEL => {
                    unsafe { ShowWindow(h, SW_HIDE) };
                    return 1;
                }
                _ => match id {
                    IDC_PREV => Some(FindAction::Prev),
                    IDC_COUNT => Some(FindAction::Count),
                    IDC_MARK => Some(FindAction::MarkAll),
                    IDC_CLEAR => Some(FindAction::ClearMarks),
                    IDC_REPLACE_ONE => Some(FindAction::Replace),
                    IDC_REPLACE_ALL => Some(FindAction::ReplaceAll),
                    _ => None,
                },
            };
            match action {
                Some(a) => {
                    let st = read_state(h);
                    if let Some(app) = crate::app::app() {
                        app.find_action(a, st);
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

// ---- 跳转到行 ----

const IDC_LINE: u16 = 201;

thread_local! {
    static GOTO_MAX: Cell<usize> = const { Cell::new(1) };
    static GOTO_CUR: Cell<usize> = const { Cell::new(1) };
    static GOTO_RESULT: Cell<usize> = const { Cell::new(0) };
}

/// 模态询问行号（1 起始）。取消时返回 None。
pub fn ask_line(
    owner: HWND,
    hinstance: *mut core::ffi::c_void,
    current: usize,
    max: usize,
) -> Option<usize> {
    GOTO_MAX.with(|c| c.set(max));
    GOTO_CUR.with(|c| c.set(current));
    GOTO_RESULT.with(|c| c.set(0));
    let t = Template::new(
        "跳转到行",
        WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME | DS_CENTER,
        180,
        62,
    )
    .label(&format!("行号（1 - {max}）："), 0xFFFF, 7, 9, 166, 9)
    .edit(IDC_LINE, 7, 21, 166, 13, ES_NUMBER)
    .button("确定", IDOK as u16, 66, 41, 50, 14, true)
    .button("取消", IDCANCEL as u16, 123, 41, 50, 14, false)
    .build();
    let r = unsafe {
        DialogBoxIndirectParamW(
            hinstance,
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(goto_proc),
            0,
        )
    };
    let line = GOTO_RESULT.with(|c| c.get());
    (r == IDOK as isize && line > 0).then_some(line)
}

unsafe extern "system" fn goto_proc(h: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> isize {
    let r = crate::crash::guard(|| match msg {
        WM_INITDIALOG => {
            dlg::set_text(h, IDC_LINE, &GOTO_CUR.with(|c| c.get()).to_string());
            dlg::select_all(h, IDC_LINE);
            1
        }
        WM_COMMAND => match loword(wp) as i32 {
            IDOK => {
                let max = GOTO_MAX.with(|c| c.get());
                let v = dlg::get_text(h, IDC_LINE)
                    .trim()
                    .parse::<usize>()
                    .unwrap_or(0);
                GOTO_RESULT.with(|c| c.set(v.clamp(1, max.max(1))));
                unsafe { EndDialog(h, IDOK as isize) };
                1
            }
            IDCANCEL => {
                unsafe { EndDialog(h, IDCANCEL as isize) };
                1
            }
            _ => 0,
        },
        _ => 0,
    });
    r.unwrap_or(0)
}
