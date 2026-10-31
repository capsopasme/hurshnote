//! 崩溃处理：未处理的结构化异常写 minidump；用标记文件检测上次是否异常退出。
//!
//! minidump 只用 MiniDumpNormal（不含堆内存），避免加密文档的明文进入转储文件。
//! dbghelp.dll 动态加载，不在启动时引入依赖。

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
};

use crate::win::*;

const EXCEPTION_EXECUTE_HANDLER: i32 = 1;
const MINI_DUMP_NORMAL: u32 = 0;

#[repr(C, packed(4))]
struct MinidumpExceptionInformation {
    thread_id: u32,
    exception_pointers: *mut c_void,
    client_pointers: i32,
}

type MiniDumpWriteDumpFn = unsafe extern "system" fn(
    process: *mut c_void,
    pid: u32,
    file: *mut c_void,
    dump_type: u32,
    exception: *const MinidumpExceptionInformation,
    user_stream: *const c_void,
    callback: *const c_void,
) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn SetUnhandledExceptionFilter(
        filter: Option<unsafe extern "system" fn(*mut c_void) -> i32>,
    ) -> *mut c_void;
}

static DUMP_DIR: OnceLock<PathBuf> = OnceLock::new();

fn crash_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("HurshNote")
        .join("crash")
}

unsafe extern "system" fn filter(info: *mut c_void) -> i32 {
    if let Some(dir) = DUMP_DIR.get() {
        let _ = std::fs::create_dir_all(dir);
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("hurshnote-{secs}.dmp"));
        unsafe { write_dump(&path, info) };
    }
    EXCEPTION_EXECUTE_HANDLER
}

unsafe fn write_dump(path: &std::path::Path, info: *mut c_void) {
    let lib = wide("dbghelp.dll");
    let h = unsafe { LoadLibraryW(lib.as_ptr()) };
    if h.is_null() {
        return;
    }
    let Some(proc_addr) = (unsafe { GetProcAddress(h, c"MiniDumpWriteDump".as_ptr().cast()) })
    else {
        return;
    };
    let f = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, MiniDumpWriteDumpFn>(proc_addr)
    };
    let wp = wide_path(path);
    let file = unsafe {
        CreateFileW(
            wp.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if file == INVALID_HANDLE_VALUE {
        return;
    }
    let mei = MinidumpExceptionInformation {
        thread_id: unsafe { GetCurrentThreadId() },
        exception_pointers: info,
        client_pointers: 0,
    };
    unsafe {
        f(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            file,
            MINI_DUMP_NORMAL,
            &mei,
            std::ptr::null(),
            std::ptr::null(),
        );
        CloseHandle(file);
    }
}

/// 安装未处理异常过滤器。
pub fn install() {
    let _ = DUMP_DIR.set(crash_dir());
    unsafe {
        SetUnhandledExceptionFilter(Some(filter));
    }
}

/// 运行标记：启动时创建，正常退出时删除。启动时已存在说明上次异常退出。
pub struct RunMarker {
    path: PathBuf,
}

impl RunMarker {
    /// 创建标记，返回 (标记, 上次是否异常退出)。
    pub fn acquire(data_dir: &std::path::Path) -> (RunMarker, bool) {
        let path = data_dir.join("running.marker");
        let crashed = path.exists();
        let _ = std::fs::create_dir_all(data_dir);
        let _ = std::fs::write(&path, std::process::id().to_string());
        (RunMarker { path }, crashed)
    }

    pub fn release(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
