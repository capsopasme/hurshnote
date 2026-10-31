//! 密码对话框（模态）。

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DialogBoxIndirectParamW, EndDialog, DLGTEMPLATE,
};
use zeroize::Zeroizing;

use crate::dlg::{self, Template};
use crate::win::*;

const IDC_PW1: u16 = 301;
const IDC_PW2: u16 = 302;
const ES_PASSWORD: u32 = 0x20;

thread_local! {
    static CONFIRM: RefCell<bool> = const { RefCell::new(false) };
    static RESULT: RefCell<Option<Zeroizing<String>>> = const { RefCell::new(None) };
}

/// 询问密码。`new_password` 为真时要求输入两次并提示无法找回。取消返回 None。
pub fn ask(
    owner: HWND,
    hinstance: *mut core::ffi::c_void,
    title: &str,
    prompt: &str,
    new_password: bool,
) -> Option<Zeroizing<String>> {
    CONFIRM.with(|c| *c.borrow_mut() = new_password);
    RESULT.with(|r| *r.borrow_mut() = None);
    let h: i16 = if new_password { 112 } else { 66 };
    let mut t = Template::new(
        title,
        WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME | DS_CENTER,
        220,
        h,
    )
    .label(prompt, 0xFFFF, 7, 7, 206, 18)
    .edit(IDC_PW1, 7, 27, 206, 13, ES_PASSWORD);
    let mut y = 45;
    if new_password {
        t = t
            .label("再次输入：", 0xFFFF, 7, 47, 206, 9)
            .edit(IDC_PW2, 7, 58, 206, 13, ES_PASSWORD)
            .label("注意：忘记密码将无法恢复文件内容。", 0xFFFF, 7, 76, 206, 9);
        y = 91;
    }
    let t = t
        .button("确定", IDOK as u16, 106, y, 50, 14, true)
        .button("取消", IDCANCEL as u16, 163, y, 50, 14, false)
        .build();
    let r = unsafe {
        DialogBoxIndirectParamW(
            hinstance,
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            0,
        )
    };
    let pw = RESULT.with(|r| r.borrow_mut().take());
    if r == IDOK as isize {
        pw
    } else {
        None
    }
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> isize {
    let r = crate::crash::guard(|| match msg {
        WM_INITDIALOG => 1,
        WM_COMMAND => match loword(wp) as i32 {
            IDOK => {
                let p1 = Zeroizing::new(dlg::get_text(h, IDC_PW1));
                if p1.is_empty() {
                    crate::dialogs::error_box(h, "密码不能为空。");
                    return 1;
                }
                if CONFIRM.with(|c| *c.borrow()) {
                    let p2 = Zeroizing::new(dlg::get_text(h, IDC_PW2));
                    if *p1 != *p2 {
                        crate::dialogs::error_box(h, "两次输入的密码不一致。");
                        dlg::set_text(h, IDC_PW2, "");
                        return 1;
                    }
                }
                RESULT.with(|r| *r.borrow_mut() = Some(p1));
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
