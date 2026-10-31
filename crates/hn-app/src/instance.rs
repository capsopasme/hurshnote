//! 命令行参数与单实例。
//!
//! 第二次启动时，把工作目录与参数通过 WM_COPYDATA 发给已运行的窗口，然后退出。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SendMessageW, SetForegroundWindow, ShowWindow,
};

use crate::win::*;

const ERROR_ALREADY_EXISTS: u32 = 183;
/// WM_COPYDATA 的 dwData 标识。
pub const COPYDATA_ARGS: usize = 0x4855_524E; // "HURN"

#[derive(Debug, Default, Clone)]
pub struct Args {
    pub files: Vec<PathBuf>,
    /// 1 起始的行号
    pub line: Option<usize>,
    /// 1 起始的列号
    pub col: Option<usize>,
    pub read_only: bool,
    pub multi_inst: bool,
    pub selftest: Option<PathBuf>,
}

/// 解析参数。支持：`文件...`、`-n<行>`、`-c<列>`、`-ro`、`-multiInst`、`--selftest <输出>`。
pub fn parse(args: Vec<OsString>, cwd: &Path) -> Args {
    let mut a = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let s = arg.to_string_lossy().to_string();
        if s == "--selftest" {
            a.selftest = it.next().map(PathBuf::from);
            a.multi_inst = true;
        } else if let Some(v) = s
            .strip_prefix("-n")
            .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        {
            a.line = v.parse().ok();
        } else if let Some(v) = s
            .strip_prefix("-c")
            .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        {
            a.col = v.parse().ok();
        } else if s.eq_ignore_ascii_case("-ro") {
            a.read_only = true;
        } else if s.eq_ignore_ascii_case("-multiInst") {
            a.multi_inst = true;
        } else if s.starts_with('-') && !Path::new(&s).exists() {
            // 未知选项忽略
        } else {
            let p = PathBuf::from(&arg);
            a.files.push(if p.is_absolute() { p } else { cwd.join(p) });
        }
    }
    a
}

/// 已有实例在运行时把参数转发过去，返回 true（调用方应退出）。
pub fn forward_to_running(class_name: &str, args: &[OsString]) -> bool {
    let name = wide("Local\\HurshNote.SingleInstance");
    // 互斥量在进程生命周期内保持打开，不关闭句柄。
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if h.is_null() || unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return false;
    }
    let class = wide(class_name);
    let mut hwnd: HWND = std::ptr::null_mut();
    // 已有实例可能还在启动，稍等窗口出现
    for _ in 0..40 {
        hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        if !hwnd.is_null() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if hwnd.is_null() {
        return false;
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let mut payload: Vec<u16> = Vec::new();
    for part in std::iter::once(cwd.into_os_string()).chain(args.iter().cloned()) {
        use std::os::windows::ffi::OsStrExt;
        payload.extend(part.encode_wide());
        payload.push(0);
    }
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_ARGS,
        cbData: (payload.len() * 2) as u32,
        lpData: payload.as_mut_ptr().cast(),
    };
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
        SendMessageW(hwnd, WM_COPYDATA, 0, &cds as *const _ as isize);
    }
    true
}

/// 解析 WM_COPYDATA 收到的数据：返回 (工作目录, 参数)。
pub fn decode_payload(data: &[u16]) -> (PathBuf, Vec<OsString>) {
    use std::os::windows::ffi::OsStringExt;
    let mut parts: Vec<OsString> = data.split(|&c| c == 0).map(OsString::from_wide).collect();
    // 末尾 NUL 产生的空串
    if parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    if parts.is_empty() {
        return (PathBuf::new(), Vec::new());
    }
    let cwd = PathBuf::from(parts.remove(0));
    (cwd, parts)
}
