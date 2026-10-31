//! Win32 常量与小工具。
//!
//! 常量直接写数值，避免依赖 windows-sys 中常量所在模块的变化。

#![allow(dead_code)]

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

// ---- 消息 ----
pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_SETFOCUS: u32 = 0x0007;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_NOTIFY: u32 = 0x004E;
pub const WM_CONTEXTMENU: u32 = 0x007B;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_DPICHANGED: u32 = 0x02E0;
pub const WM_APP: u32 = 0x8000;

// ---- 窗口样式 ----
pub const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
pub const SW_SHOWDEFAULT: i32 = 10;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const IDC_ARROW: u16 = 32512;

// ---- 菜单 ----
pub const MF_STRING: u32 = 0x0000;
pub const MF_POPUP: u32 = 0x0010;
pub const MF_SEPARATOR: u32 = 0x0800;
pub const TPM_RIGHTBUTTON: u32 = 0x0002;
pub const FVIRTKEY: u8 = 0x01;
pub const FSHIFT: u8 = 0x04;
pub const FCONTROL: u8 = 0x08;

// ---- 消息框 ----
pub const MB_OK: u32 = 0x0000;
pub const MB_YESNOCANCEL: u32 = 0x0003;
pub const MB_ICONERROR: u32 = 0x0010;
pub const MB_ICONWARNING: u32 = 0x0030;
pub const MB_ICONINFORMATION: u32 = 0x0040;
pub const IDCANCEL: i32 = 2;
pub const IDYES: i32 = 6;
pub const IDNO: i32 = 7;

// ---- 通用对话框 ----
pub const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;
pub const OFN_HIDEREADONLY: u32 = 0x0000_0004;
pub const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
pub const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
pub const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
pub const OFN_EXPLORER: u32 = 0x0008_0000;
pub const OFN_ENABLESIZING: u32 = 0x0080_0000;

/// 以 NUL 结尾的 UTF-16 字符串。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 路径转为以 NUL 结尾的 UTF-16。
pub fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 读取以 NUL 结尾（或到缓冲区末尾）的 UTF-16 字符串。
pub fn from_wide(buf: &[u16]) -> OsString {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..end])
}

#[inline]
pub fn loword(v: usize) -> u16 {
    (v & 0xffff) as u16
}

#[inline]
pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xffff) as u16
}

/// 从 LPARAM 取有符号坐标（GET_X_LPARAM / GET_Y_LPARAM）。
#[inline]
pub fn point_from_lparam(lp: isize) -> (i32, i32) {
    let x = (lp & 0xffff) as u16 as i16 as i32;
    let y = ((lp >> 16) & 0xffff) as u16 as i16 as i32;
    (x, y)
}
