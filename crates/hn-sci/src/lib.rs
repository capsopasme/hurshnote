//! Scintilla / Lexilla 的 Rust 封装。
//!
//! 约定：整个程序中只有本 crate 直接给 Scintilla 发消息，其他模块通过 [`Sci`] 访问。
//! [`Sci`] 只能在 UI 线程使用。

#![cfg(windows)]

use std::ffi::{c_char, c_int, c_void, CString};
use std::ptr;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, SendMessageW};

pub mod langs;
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

/// 注册 Scintilla 窗口类。程序启动时调用一次，`hinstance` 为本模块句柄。
pub unsafe fn register_classes(hinstance: *mut c_void) -> bool {
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

    /// 以只读切片访问全部文本。闭包内不得再调用会修改文档的消息。
    pub fn with_text<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        let len = self.len();
        if len == 0 {
            return f(&[]);
        }
        let p = self.send(SCI_GETCHARACTERPOINTER, 0, 0) as *const u8;
        f(unsafe { std::slice::from_raw_parts(p, len) })
    }

    /// 取 `[start, end)` 范围文本的副本。
    pub fn text_range(&self, start: usize, end: usize) -> Vec<u8> {
        self.with_text(|t| {
            let end = end.min(t.len());
            t[start.min(end)..end].to_vec()
        })
    }

    /// 用 `text` 替换 `[start, end)`。
    pub fn replace_range(&self, start: usize, end: usize, text: &[u8]) {
        self.send(SCI_SETTARGETRANGE, start, end as isize);
        self.send(SCI_REPLACETARGET, text.len(), text.as_ptr() as isize);
    }

    /// 在一个撤销步骤内执行 `f`。
    pub fn undo_group<R>(&self, f: impl FnOnce() -> R) -> R {
        self.send(SCI_BEGINUNDOACTION, 0, 0);
        let r = f();
        self.send(SCI_ENDUNDOACTION, 0, 0);
        r
    }

    pub fn current_pos(&self) -> usize {
        self.send(SCI_GETCURRENTPOS, 0, 0) as usize
    }

    pub fn anchor(&self) -> usize {
        self.send(SCI_GETANCHOR, 0, 0) as usize
    }

    pub fn selection_start(&self) -> usize {
        self.send(SCI_GETSELECTIONSTART, 0, 0) as usize
    }

    pub fn selection_end(&self) -> usize {
        self.send(SCI_GETSELECTIONEND, 0, 0) as usize
    }

    pub fn set_sel(&self, anchor: usize, caret: usize) {
        self.send(SCI_SETSEL, anchor, caret as isize);
    }

    /// 选中并滚动到可见（考虑折叠）。
    pub fn select_and_show(&self, start: usize, end: usize) {
        let line = self.line_from_pos(start);
        self.send(SCI_ENSUREVISIBLEENFORCEPOLICY, line, 0);
        self.set_sel(start, end);
        self.send(SCI_SCROLLCARET, 0, 0);
    }

    pub fn line_count(&self) -> usize {
        self.send(SCI_GETLINECOUNT, 0, 0) as usize
    }

    pub fn line_from_pos(&self, pos: usize) -> usize {
        self.send(SCI_LINEFROMPOSITION, pos, 0) as usize
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.send(SCI_POSITIONFROMLINE, line, 0) as usize
    }

    /// 行尾位置（不含换行符）。
    pub fn line_end(&self, line: usize) -> usize {
        self.send(SCI_GETLINEENDPOSITION, line, 0) as usize
    }

    /// 某行文本（不含换行符）。
    pub fn line_text(&self, line: usize) -> String {
        let bytes = self.text_range(self.line_start(line), self.line_end(line));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn is_read_only(&self) -> bool {
        self.send(SCI_GETREADONLY, 0, 0) != 0
    }

    // ---- 多文档 ----

    /// 当前视图挂着的文档指针。
    pub fn doc_pointer(&self) -> isize {
        self.send(SCI_GETDOCPOINTER, 0, 0)
    }

    /// 把视图切换到另一个文档（视图会持有它的一个引用）。
    pub fn set_doc_pointer(&self, doc: isize) {
        self.send(SCI_SETDOCPOINTER, 0, doc);
    }

    /// 新建空文档，返回的指针引用计数为 1，由调用方负责释放。
    pub fn create_document(&self) -> isize {
        self.send(SCI_CREATEDOCUMENT, 0, SC_DOCUMENTOPTION_DEFAULT)
    }

    pub fn add_ref_document(&self, doc: isize) {
        self.send(SCI_ADDREFDOCUMENT, 0, doc);
    }

    pub fn release_document(&self, doc: isize) {
        self.send(SCI_RELEASEDOCUMENT, 0, doc);
    }

    /// 创建文档加载器（只能在 UI 线程调用）；可交给后台线程填充。
    pub fn create_loader(&self, size_hint: usize) -> Option<Loader> {
        let p = self.send(SCI_CREATELOADER, size_hint, SC_DOCUMENTOPTION_DEFAULT);
        (p != 0).then_some(Loader(p as *mut c_void))
    }
}

extern "C" {
    fn hn_loader_add(loader: *mut c_void, data: *const c_char, length: isize) -> c_int;
    fn hn_loader_convert(loader: *mut c_void) -> *mut c_void;
    fn hn_loader_release(loader: *mut c_void) -> c_int;
}

/// Scintilla 文档加载器（ILoader）。可以在后台线程调用 [`Loader::add`]。
pub struct Loader(*mut c_void);

// 安全：ILoader 的设计就是在后台线程填充，之后交回 UI 线程转换。
unsafe impl Send for Loader {}

impl Loader {
    /// 追加 UTF-8 数据。返回 false 表示内存不足等错误。
    pub fn add(&mut self, data: &[u8]) -> bool {
        for chunk in data.chunks(1 << 20) {
            let st = unsafe {
                hn_loader_add(
                    self.0,
                    chunk.as_ptr() as *const c_char,
                    chunk.len() as isize,
                )
            };
            if st as isize != SC_STATUS_OK {
                return false;
            }
        }
        true
    }

    /// 转换为文档指针（引用计数 1）。只能在 UI 线程调用。
    pub fn into_document(self) -> isize {
        let p = self.0;
        std::mem::forget(self);
        unsafe { hn_loader_convert(p) as isize }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        unsafe {
            hn_loader_release(self.0);
        }
    }
}

/// Scintilla 通知结构（与 Scintilla.h 中的 `SCNotification` 布局一致）。
#[repr(C)]
#[derive(Debug)]
pub struct Notification {
    pub hwnd_from: HWND,
    pub id_from: usize,
    pub code: u32,
    pub position: isize,
    pub ch: i32,
    pub modifiers: i32,
    pub modification_type: i32,
    pub text: *const c_char,
    pub length: isize,
    pub lines_added: isize,
    pub message: i32,
    pub wparam: usize,
    pub lparam: isize,
    pub line: isize,
    pub fold_level_now: i32,
    pub fold_level_prev: i32,
    pub margin: i32,
    pub list_type: i32,
    pub x: i32,
    pub y: i32,
    pub token: i32,
    pub annotation_lines_added: isize,
    pub updated: i32,
    pub list_completion_method: i32,
    pub character_source: i32,
}
