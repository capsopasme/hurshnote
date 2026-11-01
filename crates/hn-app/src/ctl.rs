//! 通用控件（ListView / TreeView / ListBox）的常量与结构。
//!
//! 直接按 commctrl.h 定义，避免依赖 windows-sys 中随版本变化的类型与特性开关。

#![allow(dead_code, non_snake_case)]

use windows_sys::Win32::Foundation::{HWND, POINT};

const fn neg(n: i32) -> u32 {
    n as u32
}

// ---- 通知 ----
pub const NM_CLICK: u32 = neg(-2);
pub const NM_DBLCLK: u32 = neg(-3);
pub const NM_RETURN: u32 = neg(-4);
pub const NM_RCLICK: u32 = neg(-5);
pub const LVN_ITEMCHANGED: u32 = neg(-101);
pub const LVN_BEGINDRAG: u32 = neg(-109);
pub const LVN_KEYDOWN: u32 = neg(-155);
pub const LVN_GETINFOTIPW: u32 = neg(-158);
pub const LVN_GETDISPINFOW: u32 = neg(-177);
pub const TVN_SELCHANGEDW: u32 = neg(-451);
pub const TVN_ITEMEXPANDINGW: u32 = neg(-454);
pub const TVN_BEGINLABELEDITW: u32 = neg(-459);
pub const TVN_ENDLABELEDITW: u32 = neg(-460);
pub const TVN_KEYDOWN: u32 = neg(-412);

// ---- ListView ----
pub const LVM_SETBKCOLOR: u32 = 0x1001;
pub const LVM_SETIMAGELIST: u32 = 0x1003;
pub const LVM_GETITEMCOUNT: u32 = 0x1004;
pub const LVM_GETNEXTITEM: u32 = 0x100C;
pub const LVM_HITTEST: u32 = 0x1012;
pub const LVM_ENSUREVISIBLE: u32 = 0x1013;
pub const LVM_SETCOLUMNWIDTH: u32 = 0x101E;
pub const LVM_SETTEXTCOLOR: u32 = 0x1024;
pub const LVM_SETTEXTBKCOLOR: u32 = 0x1026;
pub const LVM_SETITEMSTATE: u32 = 0x102B;
pub const LVM_SETITEMCOUNT: u32 = 0x102F;
pub const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = 0x1036;
pub const LVM_INSERTCOLUMNW: u32 = 0x1061;

pub const LVS_REPORT: u32 = 0x1;
pub const LVS_SINGLESEL: u32 = 0x4;
pub const LVS_SHOWSELALWAYS: u32 = 0x8;
pub const LVS_SHAREIMAGELISTS: u32 = 0x40;
pub const LVS_OWNERDATA: u32 = 0x1000;
pub const LVS_NOCOLUMNHEADER: u32 = 0x4000;
pub const LVS_EX_FULLROWSELECT: usize = 0x20;
pub const LVS_EX_INFOTIP: usize = 0x400;
pub const LVS_EX_DOUBLEBUFFER: usize = 0x10000;
pub const LVIF_TEXT: u32 = 0x1;
pub const LVIF_IMAGE: u32 = 0x2;
pub const LVIF_STATE: u32 = 0x8;
pub const LVIS_FOCUSED: u32 = 0x1;
pub const LVIS_SELECTED: u32 = 0x2;
pub const LVNI_SELECTED: usize = 0x2;
pub const LVSIL_SMALL: usize = 1;
pub const LVCF_WIDTH: u32 = 0x2;
pub const LVSCW_AUTOSIZE_USEHEADER: isize = -2;
pub const CLR_NONE: isize = 0xFFFF_FFFF;

// ---- TreeView ----
pub const TVM_DELETEITEM: u32 = 0x1101;
pub const TVM_EXPAND: u32 = 0x1102;
pub const TVM_SETIMAGELIST: u32 = 0x1109;
pub const TVM_GETNEXTITEM: u32 = 0x110A;
pub const TVM_SELECTITEM: u32 = 0x110B;
pub const TVM_HITTEST: u32 = 0x1111;
pub const TVM_ENDEDITLABELNOW: u32 = 0x1116;
pub const TVM_SETBKCOLOR: u32 = 0x111D;
pub const TVM_SETTEXTCOLOR: u32 = 0x111E;
pub const TVM_SETEXTENDEDSTYLE: u32 = 0x112C;
pub const TVM_INSERTITEMW: u32 = 0x1132;
pub const TVM_GETITEMW: u32 = 0x113E;
pub const TVM_SETITEMW: u32 = 0x113F;
pub const TVM_EDITLABELW: u32 = 0x1141;

pub const TVS_HASBUTTONS: u32 = 0x1;
pub const TVS_HASLINES: u32 = 0x2;
pub const TVS_LINESATROOT: u32 = 0x4;
pub const TVS_EDITLABELS: u32 = 0x8;
pub const TVS_SHOWSELALWAYS: u32 = 0x20;
pub const TVS_TRACKSELECT: u32 = 0x200;
pub const TVS_FULLROWSELECT: u32 = 0x1000;
pub const TVS_EX_DOUBLEBUFFER: isize = 0x4;
pub const TVIF_TEXT: u32 = 0x1;
pub const TVIF_IMAGE: u32 = 0x2;
pub const TVIF_PARAM: u32 = 0x4;
pub const TVIF_STATE: u32 = 0x8;
pub const TVIF_HANDLE: u32 = 0x10;
pub const TVIF_SELECTEDIMAGE: u32 = 0x20;
pub const TVIF_CHILDREN: u32 = 0x40;
pub const TVE_COLLAPSE: usize = 0x1;
pub const TVE_EXPAND: usize = 0x2;
pub const TVE_COLLAPSERESET: usize = 0x8000;
pub const TVGN_ROOT: usize = 0x0;
pub const TVGN_NEXT: usize = 0x1;
pub const TVGN_PARENT: usize = 0x3;
pub const TVGN_CHILD: usize = 0x4;
pub const TVGN_CARET: usize = 0x9;
pub const TVSIL_NORMAL: usize = 0;
pub const TVI_ROOT: isize = -0x10000;
pub const TVI_LAST: isize = -0xFFFE;
pub const TVHT_ONITEM: u32 = 0x46;

// ---- ListBox ----
pub const LB_ADDSTRING: u32 = 0x0180;
pub const LB_RESETCONTENT: u32 = 0x0184;
pub const LB_SETCURSEL: u32 = 0x0186;
pub const LB_GETCURSEL: u32 = 0x0188;
pub const LB_GETCOUNT: u32 = 0x018B;
pub const LBS_NOTIFY: u32 = 0x1;
pub const LBS_NOINTEGRALHEIGHT: u32 = 0x100;
pub const WS_VSCROLL: u32 = 0x0020_0000;
pub const LBN_SELCHANGE: u16 = 1;
pub const LBN_DBLCLK: u16 = 2;
pub const EN_CHANGE: u16 = 0x0300;
pub const CLS_LISTBOX: u16 = 0x0083;

#[repr(C)]
pub struct NMHDR {
    pub hwndFrom: HWND,
    pub idFrom: usize,
    pub code: u32,
}

#[repr(C)]
pub struct LVITEMW {
    pub mask: u32,
    pub iItem: i32,
    pub iSubItem: i32,
    pub state: u32,
    pub stateMask: u32,
    pub pszText: *mut u16,
    pub cchTextMax: i32,
    pub iImage: i32,
    pub lParam: isize,
    pub iIndent: i32,
    pub iGroupId: i32,
    pub cColumns: u32,
    pub puColumns: *mut u32,
    pub piColFmt: *mut i32,
    pub iGroup: i32,
}

#[repr(C)]
pub struct NMLVDISPINFOW {
    pub hdr: NMHDR,
    pub item: LVITEMW,
}

#[repr(C)]
pub struct NMLISTVIEW {
    pub hdr: NMHDR,
    pub iItem: i32,
    pub iSubItem: i32,
    pub uNewState: u32,
    pub uOldState: u32,
    pub uChanged: u32,
    pub ptAction: POINT,
    pub lParam: isize,
}

#[repr(C)]
pub struct NMITEMACTIVATE {
    pub hdr: NMHDR,
    pub iItem: i32,
    pub iSubItem: i32,
    pub uNewState: u32,
    pub uOldState: u32,
    pub uChanged: u32,
    pub ptAction: POINT,
    pub lParam: isize,
    pub uKeyFlags: u32,
}

#[repr(C)]
pub struct NMLVGETINFOTIPW {
    pub hdr: NMHDR,
    pub dwFlags: u32,
    pub pszText: *mut u16,
    pub cchTextMax: i32,
    pub iItem: i32,
    pub iSubItem: i32,
    pub lParam: isize,
}

#[repr(C)]
pub struct LVHITTESTINFO {
    pub pt: POINT,
    pub flags: u32,
    pub iItem: i32,
    pub iSubItem: i32,
    pub iGroup: i32,
}

#[repr(C)]
pub struct LVCOLUMNW {
    pub mask: u32,
    pub fmt: i32,
    pub cx: i32,
    pub pszText: *mut u16,
    pub cchTextMax: i32,
    pub iSubItem: i32,
    pub iImage: i32,
    pub iOrder: i32,
    pub cxMin: i32,
    pub cxDefault: i32,
    pub cxIdeal: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TVITEMW {
    pub mask: u32,
    pub hItem: isize,
    pub state: u32,
    pub stateMask: u32,
    pub pszText: *mut u16,
    pub cchTextMax: i32,
    pub iImage: i32,
    pub iSelectedImage: i32,
    pub cChildren: i32,
    pub lParam: isize,
}

impl Default for TVITEMW {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// commctrl.h 中 item 是 TVITEMEXW / TVITEMW 的联合，按较大的 TVITEMEXW 补齐。
#[repr(C)]
pub struct TVINSERTSTRUCTW {
    pub hParent: isize,
    pub hInsertAfter: isize,
    pub item: TVITEMW,
    pub ext: [usize; 4],
}

impl TVINSERTSTRUCTW {
    pub fn new(parent: isize, after: isize, item: TVITEMW) -> Self {
        TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: after,
            item,
            ext: [0; 4],
        }
    }
}

#[repr(C)]
pub struct NMTREEVIEWW {
    pub hdr: NMHDR,
    pub action: u32,
    pub itemOld: TVITEMW,
    pub itemNew: TVITEMW,
    pub ptDrag: POINT,
}

#[repr(C)]
pub struct NMTVDISPINFOW {
    pub hdr: NMHDR,
    pub item: TVITEMW,
}

/// NMLVKEYDOWN / NMTVKEYDOWN（commctrl.h 中按 1 字节对齐）。
#[repr(C, packed(1))]
pub struct NMKEYDOWN {
    pub hdr: NMHDR,
    pub wVKey: u16,
    pub flags: u32,
}

#[repr(C)]
pub struct TVHITTESTINFO {
    pub pt: POINT,
    pub flags: u32,
    pub hItem: isize,
}
