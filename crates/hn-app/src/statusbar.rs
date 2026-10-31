//! 状态栏（msctls_statusbar32）。
//!
//! 分栏：消息 | 行列 | 选区 | 语言 | 换行符 | 编码 | 缩放 | 状态。
//! 点击语言 / 换行符 / 编码 / 缩放 / 状态栏位会弹出切换菜单。

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, GetWindowRect, SendMessageW,
};

use crate::win::*;

pub const PART_MESSAGE: usize = 0;
pub const PART_POS: usize = 1;
pub const PART_SEL: usize = 2;
pub const PART_LANG: usize = 3;
pub const PART_EOL: usize = 4;
pub const PART_ENC: usize = 5;
pub const PART_ZOOM: usize = 6;
pub const PART_STATE: usize = 7;

/// 除消息栏外各栏宽度（DIP）。
const WIDTHS: [i32; 7] = [130, 150, 120, 60, 130, 55, 100];

pub fn create(parent: HWND, hinstance: *mut core::ffi::c_void, id: usize) -> HWND {
    let class = wide("msctls_statusbar32");
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | SBARS_SIZEGRIP,
            0,
            0,
            0,
            0,
            parent,
            id as *mut core::ffi::c_void,
            hinstance,
            std::ptr::null(),
        )
    }
}

/// 父窗口尺寸变化后调用；返回状态栏高度。
pub fn layout(sb: HWND, dpi: u32) -> i32 {
    unsafe {
        SendMessageW(sb, WM_SIZE, 0, 0);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(sb, &mut rc);
        let width = rc.right;
        let fixed: i32 = WIDTHS.iter().map(|w| w * dpi as i32 / 96).sum();
        let mut edges = [0i32; 8];
        let mut x = (width - fixed).max(60);
        edges[0] = x;
        for (i, w) in WIDTHS.iter().enumerate() {
            x += w * dpi as i32 / 96;
            edges[i + 1] = x;
        }
        edges[7] = -1;
        SendMessageW(sb, SB_SETPARTS, edges.len(), edges.as_ptr() as isize);
        let mut wr: RECT = std::mem::zeroed();
        GetWindowRect(sb, &mut wr);
        wr.bottom - wr.top
    }
}

pub fn set(sb: HWND, part: usize, text: &str) {
    let w = wide(text);
    unsafe { SendMessageW(sb, SB_SETTEXTW, part, w.as_ptr() as isize) };
}

/// NM_CLICK 的通知结构（NMMOUSE）。
#[repr(C)]
pub struct NmMouse {
    pub hwnd_from: HWND,
    pub id_from: usize,
    pub code: u32,
    pub item_spec: usize,
    pub item_data: usize,
    pub pt_x: i32,
    pub pt_y: i32,
    pub hit_info: isize,
}
