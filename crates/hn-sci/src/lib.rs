//! Scintilla / Lexilla 的 Rust 封装。
//!
//! 约定：整个程序中只有本 crate 直接给 Scintilla 发消息，其他模块通过 [`Sci`] 访问。
//! [`Sci`] 只能在 UI 线程使用。

#![cfg(windows)]

use std::ffi::{c_char, c_int, c_void, CString};
use std::ptr;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, SendMessageW};

pub mod theme;

/// 从 Scintilla.h / SciLexer.h 自动生成的常量。
#[allow(dead_code, clippy::unreadable_literal)]
pub mod consts {
    include!(concat!(env!("OUT_DIR"), "/sci_consts.rs"));
}
pub use consts::*;

type SciFnDirect = unsafe extern "C" fn(isize, u32, usize, isize) -> isize;

extern "C" {
    fn Scintilla_RegisterClasses(hinstance: *mut c_void) -> c_int;
    fn Scintilla_ReleaseResources() -> c_int;
    fn hn_create_lexer(name: *const c_char) -> *mut c_void;
    fn hn_lexer_count() -> c_int;
}

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;

/// 注册 Scintilla 窗口类。程序启动时调用一次。
pub fn register_classes(hinstance: *mut c_void) -> bool {
    unsafe { Scintilla_RegisterClasses(hinstance) != 0 }
}

/// 程序退出前释放 Scintilla 的全局资源。
pub fn release_resources() {
    unsafe {
        Scintilla_ReleaseResources();
    }
}

/// 编进程序的 lexer 数量。
pub fn lexer_count() -> usize {
    unsafe { hn_lexer_count() as usize }
}

/// 一个 Scintilla 编辑器窗口的句柄与直接调用函数。
///
/// 通过 `SCI_GETDIRECTFUNCTION` 绕过消息队列直接调用，比 `SendMessage` 快得多。
#[derive(Clone, Copy, Debug)]
pub struct Sci {
    hwnd: HWND,
    func: SciFnDirect,
    ptr: isize,
}

impl Sci {
    /// 创建一个 Scintilla 子窗口。
    pub unsafe fn create(parent: HWND, hinstance: *mut c_void, id: usize) -> Option<Sci> {
        let class: Vec<u16> = "Scintilla\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_CLIPCHILDREN,
                0,
                0,
                0,
                0,
                parent,
                id as *mut c_void,
                hinstance,
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return None;
        }
        let f = unsafe { SendMessageW(hwnd, SCI_GETDIRECTFUNCTION, 0, 0) };
        let p = unsafe { SendMessageW(hwnd, SCI_GETDIRECTPOINTER, 0, 0) };
        if f == 0 || p == 0 {
            return None;
        }
        let func = unsafe { std::mem::transmute::<isize, SciFnDirect>(f) };
        Some(Sci { hwnd, func, ptr: p })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 发送一条 Scintilla 消息。
    #[inline]
    pub fn send(&self, msg: u32, wparam: usize, lparam: isize) -> isize {
        unsafe { (self.func)(self.ptr, msg, wparam, lparam) }
    }

    /// 发送带 C 字符串参数（lParam）的消息。字符串含 NUL 时截断到 NUL 之前。
    pub fn send_str(&self, msg: u32, wparam: usize, s: &str) -> isize {
        let end = s.find('\0').unwrap_or(s.len());
        let c = CString::new(&s[..end]).unwrap_or_default();
        self.send(msg, wparam, c.as_ptr() as isize)
    }

    /// 用给定的 UTF-8 字节替换全部文本（可含 NUL）。不清空撤销历史。
    pub fn set_bytes(&self, text: &[u8]) {
        self.send(SCI_CLEARALL, 0, 0);
        if !text.is_empty() {
            self.send(SCI_APPENDTEXT, text.len(), text.as_ptr() as isize);
        }
    }

    /// 取出全部文本的 UTF-8 字节副本。
    pub fn bytes(&self) -> Vec<u8> {
        let len = self.len();
        if len == 0 {
            return Vec::new();
        }
        // SCI_GETCHARACTERPOINTER 返回连续缓冲区，下一次修改前有效，因此立即复制。
        let p = self.send(SCI_GETCHARACTERPOINTER, 0, 0) as *const u8;
        unsafe { std::slice::from_raw_parts(p, len) }.to_vec()
    }

    /// 文本字节数。
    pub fn len(&self) -> usize {
        self.send(SCI_GETLENGTH, 0, 0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 自上次保存点以来是否有修改。
    pub fn is_modified(&self) -> bool {
        self.send(SCI_GETMODIFY, 0, 0) != 0
    }

    pub fn set_save_point(&self) {
        self.send(SCI_SETSAVEPOINT, 0, 0);
    }

    pub fn empty_undo_buffer(&self) {
        self.send(SCI_EMPTYUNDOBUFFER, 0, 0);
    }

    /// 载入新文档内容：替换文本、清空撤销历史、设保存点、光标回到开头。
    pub fn load_document(&self, text: &[u8]) {
        let ro = self.send(SCI_GETREADONLY, 0, 0);
        self.send(SCI_SETREADONLY, 0, 0);
        self.send(SCI_SETUNDOCOLLECTION, 0, 0);
        self.set_bytes(text);
        self.send(SCI_SETUNDOCOLLECTION, 1, 0);
        self.empty_undo_buffer();
        self.set_save_point();
        self.send(SCI_GOTOPOS, 0, 0);
        self.send(SCI_SETREADONLY, ro as usize, 0);
    }

    /// 按名称创建并设置 lexer。返回是否成功。
    pub fn set_lexer(&self, name: &str) -> bool {
        let Ok(c) = CString::new(name) else {
            return false;
        };
        let lexer = unsafe { hn_create_lexer(c.as_ptr()) };
        if lexer.is_null() {
            return false;
        }
        // Scintilla 接管 lexer 对象的生命周期。
        self.send(SCI_SETILEXER, 0, lexer as isize);
        true
    }
}
