//! 字节源与块缓存。
//!
//! [`FileSource`] 每次读取都重新打开文件（Windows 上 std 默认以读/写/删除共享模式打开），
//! 读完立即关闭：不长期持有句柄，写日志的程序可以随时轮转或截断文件。
//! UI 线程通过 [`Reader`] 访问，按 64 KiB 块缓存最近使用的数据。

use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::charset::Charset;

/// 可随机读取的字节序列。
pub trait ByteSource: Send + Sync {
    /// 当前已知长度。
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 从 `off` 读取最多 `buf.len()` 字节（不超过已知长度）。返回实际读取的字节数。
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// 尽量读满 `buf`；遇到末尾时返回较少的字节数。
    fn read_full(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            let n = self.read_at(off + done as u64, &mut buf[done..])?;
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }
}

/// 磁盘文件。长度在打开时取得，之后由调用方（Tail 轮询）更新。
#[derive(Debug)]
pub struct FileSource {
    path: PathBuf,
    len: AtomicU64,
}

impl FileSource {
    pub fn open(path: &Path) -> io::Result<FileSource> {
        let m = std::fs::metadata(path)?;
        if m.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "这是一个文件夹",
            ));
        }
        Ok(FileSource {
            path: path.to_path_buf(),
            len: AtomicU64::new(m.len()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_len(&self, len: u64) {
        self.len.store(len, Ordering::Relaxed);
    }
}

#[cfg(windows)]
fn pread(f: &std::fs::File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, off)
}

#[cfg(unix)]
fn pread(f: &std::fs::File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, off)
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len.load(Ordering::Relaxed)
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.len();
        if off >= len || buf.is_empty() {
            return Ok(0);
        }
        let n = (len - off).min(buf.len() as u64) as usize;
        let f = std::fs::File::open(&self.path)?;
        let mut done = 0;
        while done < n {
            let k = pread(&f, &mut buf[done..n], off + done as u64)?;
            if k == 0 {
                break; // 文件被截断
            }
            done += k;
        }
        Ok(done)
    }

    // 一次打开读满，避免默认实现每次循环都重新打开文件
    fn read_full(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.read_at(off, buf)
    }
}

/// 内存中的字节（例如编辑中文档按当前编码编码后的快照，用于 Hex 视图）。
#[derive(Debug, Clone)]
pub struct MemSource(pub Arc<Vec<u8>>);

impl MemSource {
    pub fn new(bytes: Vec<u8>) -> MemSource {
        MemSource(Arc::new(bytes))
    }
}

impl ByteSource for MemSource {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.0.len() as u64;
        if off >= len {
            return Ok(0);
        }
        let n = (len - off).min(buf.len() as u64) as usize;
        buf[..n].copy_from_slice(&self.0[off as usize..off as usize + n]);
        Ok(n)
    }
}

/// 缓存块大小。
pub const CACHE_BLOCK: u64 = 64 * 1024;
/// 最多缓存的块数（8 MiB）。
const CACHE_BLOCKS: usize = 128;

struct Slot {
    idx: u64,
    data: Rc<[u8]>,
    used: u64,
}

/// 带块缓存的读取器（只在 UI 线程使用）。
pub struct Reader {
    src: Arc<dyn ByteSource>,
    slots: RefCell<Vec<Slot>>,
    tick: Cell<u64>,
    error: RefCell<Option<String>>,
}

impl Reader {
    pub fn new(src: Arc<dyn ByteSource>) -> Reader {
        Reader {
            src,
            slots: RefCell::new(Vec::new()),
            tick: Cell::new(0),
            error: RefCell::new(None),
        }
    }

    pub fn source(&self) -> Arc<dyn ByteSource> {
        self.src.clone()
    }

    pub fn len(&self) -> u64 {
        self.src.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 最近一次读取错误（读取失败时数据按空处理）。
    pub fn take_error(&self) -> Option<String> {
        self.error.borrow_mut().take()
    }

    /// 丢弃全部缓存（文件被替换、改写后）。
    pub fn clear(&self) {
        self.slots.borrow_mut().clear();
    }

    /// 丢弃包含 `off` 及其之后的缓存块（文件增长后，原来的最后一块不完整）。
    pub fn invalidate_from(&self, off: u64) {
        let first = off / CACHE_BLOCK;
        self.slots.borrow_mut().retain(|s| s.idx < first);
    }

    /// 第 `idx` 块的数据（最后一块可能不足 64 KiB）。
    pub fn block(&self, idx: u64) -> Rc<[u8]> {
        let t = self.tick.get() + 1;
        self.tick.set(t);
        {
            let mut slots = self.slots.borrow_mut();
            if let Some(s) = slots.iter_mut().find(|s| s.idx == idx) {
                s.used = t;
                return s.data.clone();
            }
        }
        let start = idx * CACHE_BLOCK;
        let len = self.src.len();
        let want = len.saturating_sub(start).min(CACHE_BLOCK) as usize;
        let mut buf = vec![0u8; want];
        let n = match self.src.read_full(start, &mut buf) {
            Ok(n) => n,
            Err(e) => {
                *self.error.borrow_mut() = Some(e.to_string());
                0
            }
        };
        buf.truncate(n);
        let data: Rc<[u8]> = buf.into();
        let mut slots = self.slots.borrow_mut();
        if slots.len() >= CACHE_BLOCKS {
            if let Some(i) = slots
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.used)
                .map(|(i, _)| i)
            {
                slots.swap_remove(i);
            }
        }
        slots.push(Slot {
            idx,
            data: data.clone(),
            used: t,
        });
        data
    }

    /// 读取 `[off, off + len)`（超出末尾的部分忽略）。
    pub fn read(&self, off: u64, len: usize) -> Vec<u8> {
        let end = (off + len as u64).min(self.len());
        let mut out = Vec::with_capacity(end.saturating_sub(off) as usize);
        let mut pos = off;
        while pos < end {
            let b = self.block(pos / CACHE_BLOCK);
            let in_block = (pos % CACHE_BLOCK) as usize;
            if in_block >= b.len() {
                break; // 读取失败或文件被截断
            }
            let take = ((end - pos) as usize).min(b.len() - in_block);
            out.extend_from_slice(&b[in_block..in_block + take]);
            pos += take as u64;
        }
        out
    }

    /// 读取从 `off` 开始的最多 `n` 字节到定长数组（用于判断字符、换行符）。
    pub fn peek(&self, off: u64, n: usize) -> Vec<u8> {
        self.read(off, n)
    }

    /// 在 `[from, to)` 中找第一个 LF（码元对齐，`from` 必须对齐）。
    pub fn find_lf(&self, cs: Charset, from: u64, to: u64) -> Option<u64> {
        let to = to.min(self.len());
        let unit = cs.unit();
        let mut pos = from;
        while pos + unit <= to {
            let b = self.block(pos / CACHE_BLOCK);
            let start = (pos % CACHE_BLOCK) as usize;
            if start >= b.len() {
                return None;
            }
            let end = ((to - (pos - start as u64)) as usize).min(b.len());
            if let Some(i) = cs.find_lf(&b[start..end]) {
                return Some(pos + i as u64);
            }
            // 块大小是偶数，UTF-16 码元不会跨块
            pos += (end - start) as u64;
            if end < b.len() {
                break;
            }
        }
        None
    }

    /// 在 `[lo, before)` 中找最后一个 LF（码元对齐）。
    pub fn rfind_lf(&self, cs: Charset, lo: u64, before: u64) -> Option<u64> {
        let before = before.min(self.len());
        let unit = cs.unit();
        let mut end = before;
        while end >= lo + unit {
            let idx = (end - 1) / CACHE_BLOCK;
            let base = idx * CACHE_BLOCK;
            let b = self.block(idx);
            let s = lo.max(base);
            let e = end.min(base + b.len() as u64);
            if e <= s {
                return None;
            }
            // UTF-16：从对齐的位置开始
            let s_aligned = if unit == 2 && s % 2 == 1 { s + 1 } else { s };
            if e > s_aligned {
                let slice = &b[(s_aligned - base) as usize..(e - base) as usize];
                if let Some(i) = cs.rfind_lf(slice) {
                    return Some(s_aligned + i as u64);
                }
            }
            end = s;
        }
        None
    }

    /// 统计 `[from, to)` 中的 LF 个数。
    pub fn count_lf(&self, cs: Charset, from: u64, to: u64) -> u64 {
        let to = to.min(self.len());
        let mut pos = from;
        let mut n = 0;
        while pos < to {
            let b = self.block(pos / CACHE_BLOCK);
            let start = (pos % CACHE_BLOCK) as usize;
            if start >= b.len() {
                break;
            }
            let end = ((to - (pos - start as u64)) as usize).min(b.len());
            n += cs.count_lf(&b[start..end]);
            pos += (end - start) as u64;
            if end < b.len() {
                break;
            }
        }
        n
    }

    /// 在 `[from, to)` 中找第 `n` 个（1 起始）LF。
    pub fn nth_lf(&self, cs: Charset, from: u64, to: u64, n: u64) -> Option<u64> {
        let to = to.min(self.len());
        let mut pos = from;
        let mut left = n;
        while pos < to && left > 0 {
            let b = self.block(pos / CACHE_BLOCK);
            let start = (pos % CACHE_BLOCK) as usize;
            if start >= b.len() {
                break;
            }
            let end = ((to - (pos - start as u64)) as usize).min(b.len());
            let slice = &b[start..end];
            let c = cs.count_lf(slice);
            if c >= left {
                return cs.nth_lf(slice, left).map(|i| pos + i as u64);
            }
            left -= c;
            pos += (end - start) as u64;
            if end < b.len() {
                break;
            }
        }
        None
    }
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("len", &self.len())
            .field("cached_blocks", &self.slots.borrow().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(bytes: &[u8]) -> Reader {
        Reader::new(Arc::new(MemSource::new(bytes.to_vec())))
    }

    #[test]
    fn read_across_blocks() {
        let data: Vec<u8> = (0..(CACHE_BLOCK as usize * 3 + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        let r = reader(&data);
        let off = CACHE_BLOCK - 5;
        assert_eq!(
            r.read(off, 20),
            data[off as usize..off as usize + 20].to_vec()
        );
        assert_eq!(r.read(data.len() as u64 - 3, 10).len(), 3);
        assert!(r.read(data.len() as u64 + 1, 10).is_empty());
    }

    #[test]
    fn lf_search_across_blocks() {
        let mut data = vec![b'x'; CACHE_BLOCK as usize * 2 + 100];
        let p1 = CACHE_BLOCK as usize - 1;
        let p2 = CACHE_BLOCK as usize + 10;
        data[p1] = b'\n';
        data[p2] = b'\n';
        let r = reader(&data);
        let cs = Charset::Utf8;
        assert_eq!(r.find_lf(cs, 0, data.len() as u64), Some(p1 as u64));
        assert_eq!(
            r.find_lf(cs, p1 as u64 + 1, data.len() as u64),
            Some(p2 as u64)
        );
        assert_eq!(r.rfind_lf(cs, 0, data.len() as u64), Some(p2 as u64));
        assert_eq!(r.rfind_lf(cs, 0, p2 as u64), Some(p1 as u64));
        assert_eq!(r.rfind_lf(cs, p1 as u64 + 1, p2 as u64), None);
        assert_eq!(r.count_lf(cs, 0, data.len() as u64), 2);
        assert_eq!(r.nth_lf(cs, 0, data.len() as u64, 2), Some(p2 as u64));
        assert_eq!(r.nth_lf(cs, 0, data.len() as u64, 3), None);
    }

    #[test]
    fn utf16_rfind_alignment() {
        let le: Vec<u8> = "ab\ncd"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let r = reader(&le);
        let cs = Charset::Utf16Le;
        assert_eq!(r.rfind_lf(cs, 0, le.len() as u64), Some(4));
        // 从奇数下界开始也要按偶数对齐
        assert_eq!(r.rfind_lf(cs, 1, le.len() as u64), Some(4));
        assert_eq!(r.find_lf(cs, 0, le.len() as u64), Some(4));
    }

    #[test]
    fn file_source_reads_and_tolerates_truncation() {
        let dir = std::env::temp_dir().join(format!("hn-large-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        std::fs::write(&p, b"hello world").unwrap();
        let src = FileSource::open(&p).unwrap();
        let mut buf = [0u8; 5];
        assert_eq!(src.read_at(6, &mut buf).unwrap(), 5);
        assert_eq!(&buf, b"world");
        // 不持有句柄：可以删除、重建
        std::fs::remove_file(&p).unwrap();
        std::fs::write(&p, b"hi").unwrap();
        let n = src.read_at(0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hi");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cache_eviction_keeps_working() {
        let data = vec![7u8; CACHE_BLOCK as usize * (CACHE_BLOCKS + 10)];
        let r = reader(&data);
        for i in 0..(CACHE_BLOCKS as u64 + 10) {
            assert_eq!(r.block(i).len(), CACHE_BLOCK as usize);
        }
        assert!(r.slots.borrow().len() <= CACHE_BLOCKS);
        r.invalidate_from(CACHE_BLOCK * 5);
        assert!(r.slots.borrow().iter().all(|s| s.idx < 5));
    }
}
