//! 文件状态快照与变化分类（大文件视图的轮询、Tail 模式）。
//!
//! 每次检查都"打开 → 取信息 → 关闭"，不长期持有句柄。判断依据：
//! - 文件标识（Windows 的文件索引号、Unix 的 inode）变了：文件被替换（轮转）
//! - 长度变小：被截断
//! - 长度变大：增长；再比较旧末尾前 4 KiB 的哈希，不同说明是整体改写
//! - 长度不变但修改时间变了：可能被原地改写，同样比较末尾哈希
//!
//! Windows 的"文件系统隧道"会让 15 秒内同名重建的文件继承旧的创建时间，
//! 因此不用创建时间判断替换。

use std::fs::File;
use std::io;
use std::path::Path;
use std::time::SystemTime;

/// 用于比较的末尾片段长度。
const PROBE: u64 = 4096;

/// 某一时刻的文件状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileState {
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// 文件标识（无法取得时为 None）
    pub id: Option<u64>,
    /// `[len - 4096, len)` 的哈希
    pub probe: u64,
}

/// 变化类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    /// 末尾追加了内容
    Grown {
        from: u64,
        to: u64,
    },
    /// 长度变小
    Truncated,
    /// 换成了另一个文件（轮转）
    Replaced,
    /// 内容被改写
    Rewritten,
    /// 文件不存在了
    Missing,
}

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(windows)]
fn file_id(f: &File) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[allow(non_snake_case)]
    struct ByHandleFileInformation {
        dwFileAttributes: u32,
        ftCreationTime: [u32; 2],
        ftLastAccessTime: [u32; 2],
        ftLastWriteTime: [u32; 2],
        dwVolumeSerialNumber: u32,
        nFileSizeHigh: u32,
        nFileSizeLow: u32,
        nNumberOfLinks: u32,
        nFileIndexHigh: u32,
        nFileIndexLow: u32,
    }
    extern "system" {
        fn GetFileInformationByHandle(
            h: *mut core::ffi::c_void,
            info: *mut ByHandleFileInformation,
        ) -> i32;
    }
    // 安全：句柄有效，结构体按 BY_HANDLE_FILE_INFORMATION 布局声明
    let mut info: ByHandleFileInformation = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle() as *mut _, &mut info) };
    if ok == 0 {
        return None;
    }
    let idx = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    Some(idx ^ ((info.dwVolumeSerialNumber as u64) << 48))
}

#[cfg(unix)]
fn file_id(f: &File) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    f.metadata().ok().map(|m| m.ino() ^ (m.dev() << 48))
}

#[cfg(not(any(windows, unix)))]
fn file_id(_f: &File) -> Option<u64> {
    None
}

fn probe_at(f: &File, end: u64) -> u64 {
    let start = end.saturating_sub(PROBE);
    let mut buf = vec![0u8; (end - start) as usize];
    let mut done = 0;
    while done < buf.len() {
        #[cfg(windows)]
        let r = std::os::windows::fs::FileExt::seek_read(f, &mut buf[done..], start + done as u64);
        #[cfg(unix)]
        let r = std::os::unix::fs::FileExt::read_at(f, &mut buf[done..], start + done as u64);
        match r {
            Ok(0) | Err(_) => break,
            Ok(n) => done += n,
        }
    }
    fnv(&buf[..done])
}

impl FileState {
    /// 读取当前状态。
    pub fn read(path: &Path) -> io::Result<FileState> {
        let f = File::open(path)?;
        let m = f.metadata()?;
        let len = m.len();
        Ok(FileState {
            len,
            modified: m.modified().ok(),
            id: file_id(&f),
            probe: probe_at(&f, len),
        })
    }

    /// 与之前的状态 `self` 比较，得出变化类型。
    pub fn check(&self, path: &Path) -> (Change, Option<FileState>) {
        let f = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return (Change::Missing, None),
            Err(_) => return (Change::None, None), // 暂时打不开（被独占）：下次再试
        };
        let Ok(m) = f.metadata() else {
            return (Change::None, None);
        };
        let id = file_id(&f);
        let len = m.len();
        let modified = m.modified().ok();
        let new = FileState {
            len,
            modified,
            id,
            probe: probe_at(&f, len),
        };
        if id.is_some() && self.id.is_some() && id != self.id {
            return (Change::Replaced, Some(new));
        }
        if len < self.len {
            return (Change::Truncated, Some(new));
        }
        if len == self.len && modified == self.modified {
            return (Change::None, Some(new));
        }
        // 旧末尾是否还在
        if probe_at(&f, self.len) != self.probe {
            return (Change::Rewritten, Some(new));
        }
        if len > self.len {
            (
                Change::Grown {
                    from: self.len,
                    to: len,
                },
                Some(new),
            )
        } else {
            (Change::None, Some(new))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hn-large-tail-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn grow_truncate_replace() {
        let d = dir("a");
        let p = d.join("app.log");
        std::fs::write(&p, b"line 1\n").unwrap();
        let s0 = FileState::read(&p).unwrap();
        assert_eq!(s0.check(&p).0, Change::None);

        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"line 2\n").unwrap();
        drop(f);
        let (c, s1) = s0.check(&p);
        assert_eq!(c, Change::Grown { from: 7, to: 14 });
        let s1 = s1.unwrap();

        // 轮转：改名后新建同名文件
        std::fs::rename(&p, d.join("app.log.1")).unwrap();
        std::fs::write(&p, b"new file content that is longer\n").unwrap();
        let (c, s2) = s1.check(&p);
        assert_eq!(c, Change::Replaced);
        let s2 = s2.unwrap();

        // 截断
        std::fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(3)
            .unwrap();
        assert_eq!(s2.check(&p).0, Change::Truncated);

        std::fs::remove_file(&p).unwrap();
        assert_eq!(s2.check(&p).0, Change::Missing);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rewrite_detected_by_probe() {
        let d = dir("b");
        let p = d.join("x.txt");
        std::fs::write(&p, b"aaaaaaaa").unwrap();
        let s0 = FileState::read(&p).unwrap();
        // 原地改写为更长的不同内容（文件标识不变）
        let mut f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.write_all(b"bbbbbbbbbbbb").unwrap();
        drop(f);
        assert_eq!(s0.check(&p).0, Change::Rewritten);
        let _ = std::fs::remove_dir_all(&d);
    }
}
