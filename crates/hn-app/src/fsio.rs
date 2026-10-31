//! 安全保存用户文件。
//!
//! 流程：同目录写临时文件 → FlushFileBuffers → ReplaceFileW（保留 ACL、属性、创建时间）。
//! - 符号链接：保存到链接指向的最终文件，链接本身不变
//! - 硬链接（链接数 > 1）：替换会断开链接，改为"备份 → 原地覆盖写"
//! - ReplaceFileW 失败（跨卷、部分网络盘、文件被占用但允许写）：同样退化为原地写
//! - 无权限：返回 [`SaveError::AccessDenied`]，由界面决定是否提权（`--elevated-save`）

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HWND};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, MoveFileExW, ReplaceFileW, BY_HANDLE_FILE_INFORMATION,
};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};

use crate::win::*;

const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_CANCELLED: u32 = 1223;
const REPLACEFILE_IGNORE_MERGE_ERRORS: u32 = 0x2;
const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;
const SEE_MASK_NOASYNC: u32 = 0x100;
const INFINITE: u32 = 0xFFFF_FFFF;

#[derive(Debug)]
pub enum SaveError {
    /// 文件带只读属性
    ReadOnlyAttr,
    /// 没有写权限（可尝试以管理员身份保存）
    AccessDenied,
    Io(io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::ReadOnlyAttr => write!(f, "文件带有只读属性"),
            SaveError::AccessDenied => write!(f, "没有写入权限"),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

fn is_access_denied(e: &io::Error) -> bool {
    e.raw_os_error() == Some(ERROR_ACCESS_DENIED) || e.kind() == io::ErrorKind::PermissionDenied
}

fn map_err(e: io::Error) -> SaveError {
    if is_access_denied(&e) {
        SaveError::AccessDenied
    } else {
        SaveError::Io(e)
    }
}

fn wide_os(p: &Path) -> Vec<u16> {
    wide_path(p)
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// 同目录下的临时文件名（以 `.~hn` 开头，便于识别残留）。
fn temp_name(target: &Path) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!(".~hn{}-{}.tmp", std::process::id(), n);
    target.with_file_name(name)
}

/// 文件的硬链接数。
fn link_count(path: &Path) -> io::Result<u32> {
    let f = File::open(path)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info.nNumberOfLinks)
}

/// 去掉 canonicalize 产生的 `\\?\` 前缀（仅限普通盘符路径）。
pub fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if rest.as_bytes().get(1) == Some(&b':') {
            return PathBuf::from(rest);
        }
    }
    p.to_path_buf()
}

/// 实际要写入的文件：符号链接解析到最终目标。
pub fn resolve_target(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path)
            .map(|p| strip_verbatim(&p))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

fn write_new(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// 原地覆盖写：先在数据目录备份原文件，写成功后删除备份。
fn write_in_place(target: &Path, data: &[u8]) -> io::Result<()> {
    let backup_dir = crate::config::data_dir().join("backup");
    let _ = fs::create_dir_all(&backup_dir);
    let backup = backup_dir.join(format!(
        "{}.{}.bak",
        target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    let have_backup = target.exists() && fs::copy(target, &backup).is_ok();
    let mut f = OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(target)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    if have_backup {
        let _ = fs::remove_file(&backup);
    }
    Ok(())
}

fn replace(target: &Path, tmp: &Path) -> io::Result<()> {
    let (t, r) = (wide_os(target), wide_os(tmp));
    let ok = unsafe {
        ReplaceFileW(
            t.as_ptr(),
            r.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn move_into_place(tmp: &Path, target: &Path) -> io::Result<()> {
    let (a, b) = (wide_os(tmp), wide_os(target));
    let ok = unsafe {
        MoveFileExW(
            a.as_ptr(),
            b.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// 文件修改时间（FILETIME）。
pub fn mtime(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|m| m.last_write_time())
}

/// 安全保存。返回保存后的修改时间。
pub fn save_file(path: &Path, data: &[u8]) -> Result<u64, SaveError> {
    let target = resolve_target(path);
    let meta = fs::metadata(&target).ok();
    if let Some(m) = &meta {
        if m.is_dir() {
            return Err(SaveError::Io(io::Error::other("目标是一个文件夹")));
        }
        if m.permissions().readonly() {
            return Err(SaveError::ReadOnlyAttr);
        }
    }
    let exists = meta.is_some();
    let hardlinked = exists && link_count(&target).unwrap_or(1) > 1;

    if hardlinked {
        write_in_place(&target, data).map_err(map_err)?;
    } else {
        let tmp = temp_name(&target);
        match write_new(&tmp, data) {
            Ok(()) => {
                let r = if exists {
                    replace(&target, &tmp)
                } else {
                    move_into_place(&tmp, &target)
                };
                if let Err(e) = r {
                    let _ = fs::remove_file(&tmp);
                    if !exists {
                        return Err(map_err(e));
                    }
                    // 替换失败：退化为原地写
                    write_in_place(&target, data).map_err(map_err)?;
                }
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                // 目录不可写但文件本身可写时，仍可原地写
                if exists && is_access_denied(&e) {
                    write_in_place(&target, data).map_err(map_err)?;
                } else {
                    return Err(map_err(e));
                }
            }
        }
    }
    Ok(mtime(&target).unwrap_or(0))
}

/// 去掉只读属性。
pub fn clear_readonly(path: &Path) -> io::Result<()> {
    let target = resolve_target(path);
    let mut perm = fs::metadata(&target)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    fs::set_permissions(&target, perm)
}

/// 以管理员身份启动自身完成保存（会弹出 UAC 提示）。
pub fn save_elevated(owner: HWND, target: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!(
        "hurshnote-elevated-{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&tmp, data).map_err(|e| e.to_string())?;
    let result = (|| {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let params = format!(
            "--elevated-save \"{}\" \"{}\"",
            tmp.display(),
            target.display()
        );
        let (verb, file, par) = (wide("runas"), wide_path(&exe), wide(&params));
        let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        sei.hwnd = owner;
        sei.lpVerb = verb.as_ptr();
        sei.lpFile = file.as_ptr();
        sei.lpParameters = par.as_ptr();
        sei.nShow = SW_HIDE;
        if unsafe { ShellExecuteExW(&mut sei) } == 0 {
            return Err(if unsafe { GetLastError() } == ERROR_CANCELLED {
                "已取消".to_string()
            } else {
                io::Error::last_os_error().to_string()
            });
        }
        if sei.hProcess.is_null() {
            return Err("无法启动提权进程".into());
        }
        let mut code: u32 = 1;
        unsafe {
            WaitForSingleObject(sei.hProcess, INFINITE);
            GetExitCodeProcess(sei.hProcess, &mut code);
            CloseHandle(sei.hProcess);
        }
        match code {
            0 => Ok(()),
            3 => Err("文件带有只读属性".into()),
            _ => Err(format!("提权保存失败（代码 {code}）")),
        }
    })();
    let _ = fs::remove_file(&tmp);
    result
}

/// `--elevated-save <临时文件> <目标>` 的处理（在提权进程中运行）。返回退出码。
pub fn run_elevated_helper(tmp: &Path, target: &Path) -> i32 {
    let Ok(data) = fs::read(tmp) else {
        return 2;
    };
    match save_file(target, &data) {
        Ok(_) => 0,
        Err(SaveError::ReadOnlyAttr) => 3,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hn-fsio-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn create_and_overwrite() {
        let d = tmpdir("basic");
        let p = d.join("a.txt");
        save_file(&p, b"one").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one");
        save_file(&p, b"two two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two two");
        // 不留下临时文件
        let names: Vec<String> = fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.txt".to_string()]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn hardlink_preserved() {
        let d = tmpdir("hardlink");
        let a = d.join("a.txt");
        let b = d.join("b.txt");
        fs::write(&a, b"old").unwrap();
        fs::hard_link(&a, &b).unwrap();
        save_file(&a, b"new").unwrap();
        assert_eq!(
            fs::read(&b).unwrap(),
            b"new",
            "硬链接的另一个名字应看到新内容"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn readonly_reported() {
        let d = tmpdir("ro");
        let p = d.join("r.txt");
        fs::write(&p, b"x").unwrap();
        let mut perm = fs::metadata(&p).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&p, perm).unwrap();
        assert!(matches!(save_file(&p, b"y"), Err(SaveError::ReadOnlyAttr)));
        clear_readonly(&p).unwrap();
        save_file(&p, b"y").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"y");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn mtime_changes() {
        let d = tmpdir("mtime");
        let p = d.join("m.txt");
        let t1 = save_file(&p, b"1").unwrap();
        assert!(t1 > 0);
        assert_eq!(mtime(&p), Some(t1));
        let _ = fs::remove_dir_all(&d);
    }
}
