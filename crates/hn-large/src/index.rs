//! 稀疏行索引。
//!
//! 按文件绝对偏移把文件分成 64 KiB 的块，记录"到第 k 块末尾为止共有多少个 LF"。
//! 5 GB 文件约 8 万项、640 KB 内存。查询都只需在索引之外再扫描不超过一块：
//! - 偏移 → 行号：前一块的累计数 + 本块内到该偏移的 LF 数
//! - 行号 → 偏移：二分找到第 n 个 LF 所在的块，再在块内找
//!
//! 后台线程用 [`scan`] 顺序读文件、分批发出 [`IndexUpdate`]，UI 线程用 [`LineIndex::apply`] 合并。
//! 文件增长（Tail）时从最后一个完整块继续扫描即可。
//!
//! 行的定义：以 LF 结尾（CRLF 视为一个换行）；单独的 CR 不算换行。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::charset::Charset;
use crate::source::{ByteSource, Reader};

/// 索引块大小（与 [`crate::source::CACHE_BLOCK`] 相同，因此块内扫描只读一个缓存块）。
pub const BLOCK: u64 = 64 * 1024;
/// 后台扫描一次读取的块数（4 MiB）。
const READ_BLOCKS: usize = 64;
/// 两次进度回报之间的最短间隔。
const EMIT_INTERVAL: Duration = Duration::from_millis(100);

/// 后台扫描发出的一批结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexUpdate {
    /// `cum` 中第一项对应的块号
    pub first_block: usize,
    /// 各完整块末尾的累计 LF 数
    pub cum: Vec<u64>,
    /// 已扫描到的偏移
    pub scanned: u64,
    /// 最后一个完整块之后、`scanned` 之前的 LF 数
    pub tail_lfs: u64,
    /// 是否扫描到了（扫描开始时的）文件末尾
    pub done: bool,
}

#[derive(Debug, Clone)]
pub struct LineIndex {
    cs: Charset,
    data_start: u64,
    cum: Vec<u64>,
    scanned: u64,
    tail_lfs: u64,
    done: bool,
}

impl LineIndex {
    pub fn new(cs: Charset, data_start: u64) -> LineIndex {
        LineIndex {
            cs,
            data_start,
            cum: Vec::new(),
            scanned: 0,
            tail_lfs: 0,
            done: false,
        }
    }

    pub fn charset(&self) -> Charset {
        self.cs
    }

    pub fn data_start(&self) -> u64 {
        self.data_start
    }

    /// 已扫描到的偏移。
    pub fn scanned(&self) -> u64 {
        self.scanned
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// 已知的 LF 总数。
    pub fn lf_count(&self) -> u64 {
        self.cum.last().copied().unwrap_or(0) + self.tail_lfs
    }

    /// 已知的行数（扫描完成时即总行数）。
    pub fn line_count(&self) -> u64 {
        self.lf_count() + 1
    }

    /// 继续扫描的起点：(块号, 该块之前的累计 LF 数)。
    pub fn resume_point(&self) -> (usize, u64) {
        (self.cum.len(), self.cum.last().copied().unwrap_or(0))
    }

    /// 文件增长后标记为未完成（之后从 [`resume_point`](Self::resume_point) 继续扫描）。
    pub fn mark_incomplete(&mut self) {
        self.done = false;
    }

    /// 合并一批扫描结果。批次不连续时（重新扫描最后一块）先截断。
    pub fn apply(&mut self, u: &IndexUpdate) {
        if u.first_block > self.cum.len() {
            return; // 不连续：丢弃（不应发生）
        }
        self.cum.truncate(u.first_block);
        self.cum.extend_from_slice(&u.cum);
        self.scanned = u.scanned;
        self.tail_lfs = u.tail_lfs;
        self.done = u.done;
    }

    /// `[0, off)` 中的 LF 个数，即 `off` 所在行的行号（0 起始）。`off` 尚未扫描到时返回 None。
    pub fn line_of(&self, off: u64, r: &Reader) -> Option<u64> {
        if off > self.scanned {
            return None;
        }
        let k = (off / BLOCK) as usize;
        let (base_k, base) = if k <= self.cum.len() {
            (k, if k == 0 { 0 } else { self.cum[k - 1] })
        } else {
            (self.cum.len(), self.cum.last().copied().unwrap_or(0))
        };
        let from = base_k as u64 * BLOCK;
        Some(base + r.count_lf(self.cs, from, off))
    }

    /// 第 `line` 行（0 起始）的起始偏移。尚未扫描到时返回 None。
    pub fn line_start(&self, line: u64, r: &Reader) -> Option<u64> {
        if line == 0 {
            return Some(self.data_start);
        }
        // 找第 line 个 LF 所在的块：cum[k-1] < line <= cum[k]
        let k = self.cum.partition_point(|&c| c < line);
        let unit = self.cs.unit();
        if k < self.cum.len() {
            let base = if k == 0 { 0 } else { self.cum[k - 1] };
            let from = k as u64 * BLOCK;
            return r
                .nth_lf(self.cs, from, from + BLOCK, line - base)
                .map(|p| p + unit);
        }
        let base = self.cum.last().copied().unwrap_or(0);
        if line > base + self.tail_lfs {
            return None;
        }
        let from = self.cum.len() as u64 * BLOCK;
        r.nth_lf(self.cs, from, self.scanned, line - base)
            .map(|p| p + unit)
    }
}

/// 后台扫描：从第 `start_block` 块（之前累计 `start_cum` 个 LF）扫描到 `len`。
///
/// 每隔约 100 ms 通过 `emit` 发出一批结果；`cancel` 置位后尽快返回。
/// 返回 false 表示被取消或读取失败。
pub fn scan(
    src: &dyn ByteSource,
    cs: Charset,
    start_block: usize,
    start_cum: u64,
    len: u64,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(IndexUpdate),
) -> bool {
    let mut buf = vec![0u8; BLOCK as usize * READ_BLOCKS];
    let mut block = start_block;
    let mut cum = start_cum;
    let mut pending: Vec<u64> = Vec::new();
    let mut pending_first = start_block;
    let mut last_emit = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let off = block as u64 * BLOCK;
        if off >= len {
            emit(IndexUpdate {
                first_block: pending_first,
                cum: std::mem::take(&mut pending),
                scanned: len.max(off.min(len)),
                tail_lfs: 0,
                done: true,
            });
            return true;
        }
        let want = ((len - off) as usize).min(buf.len());
        let n = match src.read_full(off, &mut buf[..want]) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let data = &buf[..n];
        let mut tail_lfs = 0;
        let mut finished = n < want; // 文件在扫描中被截断
        for chunk in data.chunks(BLOCK as usize) {
            let c = cs.count_lf(chunk);
            if chunk.len() as u64 == BLOCK {
                cum += c;
                pending.push(cum);
                block += 1;
            } else {
                tail_lfs = c;
                finished = true;
            }
        }
        let scanned = off + n as u64;
        if scanned >= len {
            finished = true;
        }
        if finished || last_emit.elapsed() >= EMIT_INTERVAL {
            emit(IndexUpdate {
                first_block: pending_first,
                cum: std::mem::take(&mut pending),
                scanned,
                tail_lfs,
                done: finished,
            });
            pending_first = block;
            last_emit = Instant::now();
        }
        if finished {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemSource;
    use std::sync::Arc;

    fn build(data: &[u8], cs: Charset, data_start: u64) -> (LineIndex, Reader) {
        let src = Arc::new(MemSource::new(data.to_vec()));
        let mut idx = LineIndex::new(cs, data_start);
        let cancel = AtomicBool::new(false);
        let mut ups = Vec::new();
        assert!(scan(
            &*src,
            cs,
            0,
            0,
            data.len() as u64,
            &cancel,
            &mut |u| ups.push(u)
        ));
        for u in &ups {
            idx.apply(u);
        }
        assert!(idx.is_done());
        (idx, Reader::new(src))
    }

    fn sample(lines: usize) -> (Vec<u8>, Vec<u64>) {
        let mut data = Vec::new();
        let mut starts = Vec::new();
        for i in 0..lines {
            starts.push(data.len() as u64);
            let len = (i * 37) % 300;
            data.extend(std::iter::repeat_n(b'a' + (i % 26) as u8, len));
            if i % 3 == 0 {
                data.push(b'\r');
            }
            data.push(b'\n');
        }
        (data, starts)
    }

    #[test]
    fn line_numbers_and_starts() {
        let (mut data, starts) = sample(5000);
        data.extend_from_slice(b"last line without newline");
        let (idx, r) = build(&data, Charset::Utf8, 0);
        assert_eq!(idx.line_count(), 5001);
        for (i, &s) in starts.iter().enumerate().step_by(7) {
            assert_eq!(idx.line_start(i as u64, &r), Some(s), "line {i}");
            assert_eq!(idx.line_of(s, &r), Some(i as u64));
            assert_eq!(idx.line_of(s + 1, &r).unwrap(), i as u64);
        }
        let last = *starts.last().unwrap();
        let tail_start = data.len() as u64 - 25;
        assert!(last < tail_start);
        assert_eq!(idx.line_start(5000, &r), Some(tail_start));
        assert_eq!(idx.line_start(5001, &r), None);
        assert_eq!(idx.line_of(data.len() as u64, &r), Some(5000));
    }

    #[test]
    fn utf16_with_bom() {
        let text: String = (0..3000).map(|i| format!("第{i}行 line\r\n")).collect();
        let mut data = vec![0xFF, 0xFE];
        data.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let (idx, r) = build(&data, Charset::Utf16Le, 2);
        assert_eq!(idx.line_count(), 3001);
        let s10 = idx.line_start(10, &r).unwrap();
        let expect: usize = (0..10)
            .map(|i| format!("第{i}行 line\r\n").encode_utf16().count())
            .sum();
        assert_eq!(s10, 2 + expect as u64 * 2);
        assert_eq!(idx.line_of(s10, &r), Some(10));
        assert_eq!(idx.line_start(0, &r), Some(2));
    }

    #[test]
    fn incremental_growth() {
        let (data, starts) = sample(3000);
        let src = Arc::new(MemSource::new(data.clone()));
        let cancel = AtomicBool::new(false);
        let mut idx = LineIndex::new(Charset::Utf8, 0);
        // 先扫描前一部分（不完整的最后一块），再从 resume_point 继续
        let first = 200_000u64;
        let mut ups = Vec::new();
        scan(&*src, Charset::Utf8, 0, 0, first, &cancel, &mut |u| {
            ups.push(u)
        });
        for u in &ups {
            idx.apply(u);
        }
        assert!(idx.is_done());
        let partial_lines = idx.line_count();
        idx.mark_incomplete();
        let (b, c) = idx.resume_point();
        let mut ups = Vec::new();
        scan(
            &*src,
            Charset::Utf8,
            b,
            c,
            data.len() as u64,
            &cancel,
            &mut |u| ups.push(u),
        );
        for u in &ups {
            idx.apply(u);
        }
        assert!(idx.is_done());
        assert!(idx.line_count() > partial_lines);
        assert_eq!(idx.line_count(), 3001);
        let r = Reader::new(src);
        assert_eq!(idx.line_start(2999, &r), Some(starts[2999]));
    }

    #[test]
    fn line_of_beyond_scanned_is_none() {
        let (data, _) = sample(100);
        let src = Arc::new(MemSource::new(data.clone()));
        let r = Reader::new(src);
        let idx = LineIndex::new(Charset::Utf8, 0);
        assert_eq!(idx.line_of(10, &r), None);
        assert_eq!(idx.line_start(0, &r), Some(0));
        assert_eq!(idx.line_start(1, &r), None);
    }

    #[test]
    fn cancel_stops_scan() {
        let data = vec![b'\n'; BLOCK as usize * 200];
        let src = MemSource::new(data);
        let cancel = AtomicBool::new(true);
        assert!(!scan(
            &src,
            Charset::Utf8,
            0,
            0,
            src.len(),
            &cancel,
            &mut |_| {}
        ));
    }

    #[test]
    fn empty_file() {
        let (idx, r) = build(b"", Charset::Utf8, 0);
        assert_eq!(idx.line_count(), 1);
        assert_eq!(idx.line_start(0, &r), Some(0));
        assert_eq!(idx.line_of(0, &r), Some(0));
    }
}
