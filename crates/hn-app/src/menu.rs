//! 菜单、右键菜单与快捷键表。

use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu, ACCEL, HACCEL, HMENU,
};

use crate::win::*;

// 命令 ID
pub const IDM_FILE_NEW: u16 = 1001;
pub const IDM_FILE_OPEN: u16 = 1002;
pub const IDM_FILE_SAVE: u16 = 1003;
pub const IDM_FILE_SAVE_AS: u16 = 1004;
pub const IDM_FILE_EXIT: u16 = 1005;

pub const IDM_EDIT_UNDO: u16 = 1101;
pub const IDM_EDIT_REDO: u16 = 1102;
pub const IDM_EDIT_CUT: u16 = 1103;
pub const IDM_EDIT_COPY: u16 = 1104;
pub const IDM_EDIT_PASTE: u16 = 1105;
pub const IDM_EDIT_DELETE: u16 = 1106;
pub const IDM_EDIT_SELECT_ALL: u16 = 1107;

pub const IDM_HELP_ABOUT: u16 = 1901;

enum Item {
    Cmd(u16, &'static str),
    Sep,
}
use Item::*;

const FILE_ITEMS: &[Item] = &[
    Cmd(IDM_FILE_NEW, "新建(&N)\tCtrl+N"),
    Cmd(IDM_FILE_OPEN, "打开(&O)...\tCtrl+O"),
    Sep,
    Cmd(IDM_FILE_SAVE, "保存(&S)\tCtrl+S"),
    Cmd(IDM_FILE_SAVE_AS, "另存为(&A)...\tCtrl+Shift+S"),
    Sep,
    Cmd(IDM_FILE_EXIT, "退出(&X)\tAlt+F4"),
];

// 编辑快捷键由 Scintilla 自带的键位表处理，这里只显示。
const EDIT_ITEMS: &[Item] = &[
    Cmd(IDM_EDIT_UNDO, "撤销(&U)\tCtrl+Z"),
    Cmd(IDM_EDIT_REDO, "重做(&R)\tCtrl+Y"),
    Sep,
    Cmd(IDM_EDIT_CUT, "剪切(&T)\tCtrl+X"),
    Cmd(IDM_EDIT_COPY, "复制(&C)\tCtrl+C"),
    Cmd(IDM_EDIT_PASTE, "粘贴(&P)\tCtrl+V"),
    Cmd(IDM_EDIT_DELETE, "删除(&D)\tDel"),
    Sep,
    Cmd(IDM_EDIT_SELECT_ALL, "全选(&A)\tCtrl+A"),
];

const HELP_ITEMS: &[Item] = &[Cmd(IDM_HELP_ABOUT, "关于 HurshNote(&A)")];

unsafe fn popup(items: &[Item]) -> HMENU {
    let m = unsafe { CreatePopupMenu() };
    for it in items {
        match it {
            Cmd(id, text) => {
                let w = wide(text);
                unsafe { AppendMenuW(m, MF_STRING, *id as usize, w.as_ptr()) };
            }
            Sep => unsafe {
                AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            },
        }
    }
    m
}

/// 主菜单栏。
pub fn build_main_menu() -> HMENU {
    unsafe {
        let bar = CreateMenu();
        for (title, items) in [
            ("文件(&F)", FILE_ITEMS),
            ("编辑(&E)", EDIT_ITEMS),
            ("帮助(&H)", HELP_ITEMS),
        ] {
            let sub = popup(items);
            let w = wide(title);
            AppendMenuW(bar, MF_POPUP, sub as usize, w.as_ptr());
        }
        bar
    }
}

/// 编辑区右键菜单（调用方负责 DestroyMenu）。
pub fn build_context_menu() -> HMENU {
    unsafe { popup(EDIT_ITEMS) }
}

/// 主窗口快捷键。
pub fn build_accelerators() -> HACCEL {
    let table = [
        ACCEL {
            fVirt: FVIRTKEY | FCONTROL,
            key: b'N' as u16,
            cmd: IDM_FILE_NEW,
        },
        ACCEL {
            fVirt: FVIRTKEY | FCONTROL,
            key: b'O' as u16,
            cmd: IDM_FILE_OPEN,
        },
        ACCEL {
            fVirt: FVIRTKEY | FCONTROL,
            key: b'S' as u16,
            cmd: IDM_FILE_SAVE,
        },
        ACCEL {
            fVirt: FVIRTKEY | FCONTROL | FSHIFT,
            key: b'S' as u16,
            cmd: IDM_FILE_SAVE_AS,
        },
    ];
    unsafe { CreateAcceleratorTableW(table.as_ptr(), table.len() as i32) }
}
