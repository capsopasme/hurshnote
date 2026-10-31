//! panic 处理：记录日志，阻止 panic 穿过 FFI 边界。

use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static PANIC_REPORTED: AtomicBool = AtomicBool::new(false);

/// panic 日志位置：%LOCALAPPDATA%\HurshNote\logs\panic.log
pub fn log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("HurshNote").join("logs").join("panic.log")
}

pub fn install_panic_hook() {
    panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let path = log_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(
                f,
                "==== HurshNote {} panic ====\n{info}\n{bt}\n",
                env!("CARGO_PKG_VERSION")
            );
        }
    }));
}

/// 在 FFI 回调（窗口过程等）中执行 `f`，捕获 panic。
///
/// 发生 panic 时返回 `None`，并且只在第一次时提示用户。
pub fn guard<R>(f: impl FnOnce() -> R) -> Option<R> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => Some(r),
        Err(_) => {
            if !PANIC_REPORTED.swap(true, Ordering::SeqCst) {
                crate::dialogs::error_box(
                    std::ptr::null_mut(),
                    &format!(
                        "HurshNote 遇到内部错误，已记录到：\n{}\n\n建议保存工作后重启程序。",
                        log_path().display()
                    ),
                );
            }
            None
        }
    }
}
