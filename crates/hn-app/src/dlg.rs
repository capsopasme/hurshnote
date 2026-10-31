//! 内存中的对话框模板（DLGTEMPLATE）与控件小工具。
//!
//! 用对话框单位布局，系统按字体与 DPI 自动缩放；Tab / Enter / Esc 由对话框管理器处理。

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetDlgItem, GetWindowTextLengthW, GetWindowTextW, SendMessageW, SetWindowTextW,
};

use crate::win::*;

pub const CLS_BUTTON: u16 = 0x0080;
pub const CLS_EDIT: u16 = 0x0081;
pub const CLS_STATIC: u16 = 0x0082;

/// 对话框模板构建器。
pub struct Template {
    words: Vec<u16>,
    count_pos: usize,
    count: u16,
}

fn push_str(v: &mut Vec<u16>, s: &str) {
    v.extend(s.encode_utf16());
    v.push(0);
}

fn align_dword(v: &mut Vec<u16>) {
    if v.len() % 2 == 1 {
        v.push(0);
    }
}

impl Template {
    /// `style` 为对话框窗口样式（会自动加上 DS_SETFONT）；坐标为对话框单位。
    pub fn new(title: &str, style: u32, w: i16, h: i16) -> Template {
        let mut v: Vec<u16> = Vec::with_capacity(512);
        let style = style | DS_SETFONT;
        v.push((style & 0xffff) as u16);
        v.push((style >> 16) as u16);
        v.push(0); // dwExtendedStyle
        v.push(0);
        let count_pos = v.len();
        v.push(0); // cdit
        v.push(0); // x
        v.push(0); // y
        v.push(w as u16);
        v.push(h as u16);
        v.push(0); // 菜单
        v.push(0); // 窗口类
        push_str(&mut v, title);
        v.push(9); // 字号
        push_str(&mut v, "Microsoft YaHei UI");
        Template {
            words: v,
            count_pos,
            count: 0,
        }
    }

    /// 添加一个控件。`class` 为预定义类原子（CLS_*）。
    #[allow(clippy::too_many_arguments)]
    pub fn item(
        mut self,
        class: u16,
        text: &str,
        id: u16,
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        style: u32,
    ) -> Template {
        let v = &mut self.words;
        align_dword(v);
        let style = style | WS_CHILD | WS_VISIBLE;
        v.push((style & 0xffff) as u16);
        v.push((style >> 16) as u16);
        v.push(0);
        v.push(0);
        v.push(x as u16);
        v.push(y as u16);
        v.push(w as u16);
        v.push(h as u16);
        v.push(id);
        v.push(0xFFFF);
        v.push(class);
        push_str(v, text);
        v.push(0); // 创建数据长度
        self.count += 1;
        self
    }

    pub fn label(self, text: &str, id: u16, x: i16, y: i16, w: i16, h: i16) -> Template {
        self.item(CLS_STATIC, text, id, x, y, w, h, 0)
    }

    pub fn edit(self, id: u16, x: i16, y: i16, w: i16, h: i16, extra: u32) -> Template {
        self.item(
            CLS_EDIT,
            "",
            id,
            x,
            y,
            w,
            h,
            WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL | extra,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn button(
        self,
        text: &str,
        id: u16,
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        default: bool,
    ) -> Template {
        let s = if default {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        self.item(CLS_BUTTON, text, id, x, y, w, h, s | WS_TABSTOP)
    }

    pub fn checkbox(self, text: &str, id: u16, x: i16, y: i16, w: i16, h: i16) -> Template {
        self.item(
            CLS_BUTTON,
            text,
            id,
            x,
            y,
            w,
            h,
            BS_AUTOCHECKBOX | WS_TABSTOP,
        )
    }

    /// 单选按钮；`first` 为一组中的第一个（带 WS_GROUP）。
    #[allow(clippy::too_many_arguments)]
    pub fn radio(
        self,
        text: &str,
        id: u16,
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        first: bool,
    ) -> Template {
        let g = if first { WS_GROUP | WS_TABSTOP } else { 0 };
        self.item(CLS_BUTTON, text, id, x, y, w, h, BS_AUTORADIOBUTTON | g)
    }

    pub fn group(self, text: &str, x: i16, y: i16, w: i16, h: i16) -> Template {
        self.item(CLS_BUTTON, text, 0xFFFF, x, y, w, h, BS_GROUPBOX)
    }

    /// 生成 DWORD 对齐的模板内存。
    pub fn build(mut self) -> Vec<u32> {
        self.words[self.count_pos] = self.count;
        align_dword(&mut self.words);
        self.words
            .chunks_exact(2)
            .map(|c| c[0] as u32 | ((c[1] as u32) << 16))
            .collect()
    }
}

// ---- 控件读写 ----

pub fn item(dlg: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(dlg, id as i32) }
}

pub fn get_text(dlg: HWND, id: u16) -> String {
    let h = item(dlg, id);
    let len = unsafe { GetWindowTextLengthW(h) }.max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

pub fn set_text(dlg: HWND, id: u16, text: &str) {
    let w = wide(text);
    unsafe { SetWindowTextW(item(dlg, id), w.as_ptr()) };
}

pub fn is_checked(dlg: HWND, id: u16) -> bool {
    unsafe { SendMessageW(item(dlg, id), BM_GETCHECK, 0, 0) as usize == BST_CHECKED }
}

pub fn set_checked(dlg: HWND, id: u16, on: bool) {
    unsafe { SendMessageW(item(dlg, id), BM_SETCHECK, on as usize, 0) };
}

/// 选中编辑框全部文字。
pub fn select_all(dlg: HWND, id: u16) {
    unsafe { SendMessageW(item(dlg, id), EM_SETSEL, 0, -1) };
}
