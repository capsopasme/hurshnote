//! 文件对话框与消息框。

use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;

use crate::win::*;

const APP_NAME: &str = "HurshNote";

/// 文件类型过滤器：成对的（说明, 通配符），以双 NUL 结尾。
fn filter() -> Vec<u16> {
    let pairs = [
        ("所有文件 (*.*)", "*.*"),
        ("文本文件 (*.txt;*.md;*.log)", "*.txt;*.md;*.log"),
    ];
    let mut v = Vec::new();
    for (desc, pat) in pairs {
        v.extend(desc.encode_utf16());
        v.push(0);
        v.extend(pat.encode_utf16());
        v.push(0);
    }
    v.push(0);
    v
}

pub fn open_file(owner: HWND) -> Option<PathBuf> {
    let filter = filter();
    let mut buf = vec![0u16; 32 * 1024];
    let mut ofn: OPENFILENAMEW = unsafe { zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.Flags = OFN_EXPLORER
        | OFN_FILEMUSTEXIST
        | OFN_PATHMUSTEXIST
        | OFN_HIDEREADONLY
        | OFN_NOCHANGEDIR
        | OFN_ENABLESIZING;
    if unsafe { GetOpenFileNameW(&mut ofn) } != 0 {
        Some(PathBuf::from(from_wide(&buf)))
    } else {
        None
    }
}

pub fn save_file(owner: HWND, current: Option<&Path>) -> Option<PathBuf> {
    let filter = filter();
    let mut buf = vec![0u16; 32 * 1024];
    if let Some(p) = current {
        let w = wide_path(p);
        if w.len() < buf.len() {
            buf[..w.len()].copy_from_slice(&w);
        }
    }
    let mut ofn: OPENFILENAMEW = unsafe { zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.Flags = OFN_EXPLORER
        | OFN_OVERWRITEPROMPT
        | OFN_PATHMUSTEXIST
        | OFN_HIDEREADONLY
        | OFN_NOCHANGEDIR
        | OFN_ENABLESIZING;
    if unsafe { GetSaveFileNameW(&mut ofn) } != 0 {
        Some(PathBuf::from(from_wide(&buf)))
    } else {
        None
    }
}

fn message_box(owner: HWND, text: &str, flags: u32) -> i32 {
    let t = wide(text);
    let c = wide(APP_NAME);
    unsafe { MessageBoxW(owner, t.as_ptr(), c.as_ptr(), flags) }
}

pub fn error_box(owner: HWND, text: &str) {
    message_box(owner, text, MB_OK | MB_ICONERROR);
}

pub fn info_box(owner: HWND, text: &str) {
    message_box(owner, text, MB_OK | MB_ICONINFORMATION);
}

/// 是 / 否 / 取消。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
}

pub fn ask_yes_no_cancel(owner: HWND, text: &str) -> Answer {
    match message_box(owner, text, MB_YESNOCANCEL | MB_ICONWARNING) {
        IDYES => Answer::Yes,
        IDNO => Answer::No,
        _ => Answer::Cancel,
    }
}
