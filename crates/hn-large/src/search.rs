//! 按行对齐的分块遍历，供大文件流式搜索使用。
//!
//! 每块约 4 MiB，切在换行符之后，因此匹配不会被块边界切断（跨行匹配除外）。
//! 一行超过块大小的上限（[`MAX_LINE`]）时在字符边界处强行切开，这一处可能漏掉匹配。

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::charset::Charset;
use crate::source::ByteSource;

/// 默认块大小。
pub const CHUNK: usize = 4 * 1024 * 1024;
/// 为了凑齐整行，单块最多扩展到的大小。
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// 遍历到的一块。
#[derive(Debug)]
pub struct Chunk<'a> {
    /// 块的起始偏移
    pub offset: u64,
    /// 块首所在的行号（0 起始）；起点不是从文件开头时为 None
    pub first_line: Option<u64>,
    pub data: &'a [u8],
}

/// 遍历结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    /// 遍历到了范围末尾
    Completed,
    /// 回调要求停止
    Stopped,
    /// 被取消
    Cancelled,
}

/// 从 `from`（行首）向后遍历到 `end`。回调返回 false 时停止。
#[allow(clippy::too_many_arguments)]
pub fn forward(
    src: &dyn ByteSource,
    cs: Charset,
    from: u64,
    first_line: Option<u64>,
    end: u64,
    chunk: usize,
    cancel: &AtomicBool,
    f: &mut dyn FnMut(&Chunk) -> bool,
) -> io::Result<Walk> {
    let end = end.min(src.len());
    let mut pos = from;
    let mut line = first_line;
    let mut buf: Vec<u8> = Vec::new();
    while pos < end {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Walk::Cancelled);
        }
        let mut want = chunk;
        let data_len = loop {
            let n = ((end - pos) as usize).min(want);
            buf.resize(n, 0);
            let got = src.read_full(pos, &mut buf[..n])?;
            buf.truncate(got);
            if got == 0 {
                return Ok(Walk::Completed); // 文件被截断
            }
            if pos + got as u64 >= end {
                break got;
            }
            match cs.rfind_lf(&buf) {
                Some(i) => break i + cs.unit() as usize,
                None if want < MAX_LINE => want = (want * 2).min(MAX_LINE),
                None => {
                    let b = cs.floor_boundary(&buf);
                    break if b == 0 { got } else { b };
                }
            }
        };
        let data = &buf[..data_len];
        let c = Chunk {
            offset: pos,
            first_line: line,
            data,
        };
        if !f(&c) {
            return Ok(Walk::Stopped);
        }
        line = line.map(|l| l + cs.count_lf(data));
        pos += data_len as u64;
    }
    Ok(Walk::Completed)
}

/// 从 `before` 向前遍历到 `start`（块按从后往前的顺序给出，每块从行首开始）。
/// 回调返回 false 时停止。块的 `first_line` 总是 None。
pub fn backward(
    src: &dyn ByteSource,
    cs: Charset,
    start: u64,
    before: u64,
    chunk: usize,
    cancel: &AtomicBool,
    f: &mut dyn FnMut(&Chunk) -> bool,
) -> io::Result<Walk> {
    let unit = cs.unit();
    let mut end = before.min(src.len());
    let mut buf: Vec<u8> = Vec::new();
    while end > start {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Walk::Cancelled);
        }
        let mut want = chunk as u64;
        let (from, skip) = loop {
            let from = end.saturating_sub(want).max(start);
            // UTF-16 保持与 start 相同的奇偶
            let from = from + (from - start) % unit;
            let n = (end - from) as usize;
            buf.resize(n, 0);
            let got = src.read_full(from, &mut buf[..n])?;
            buf.truncate(got);
            if from == start {
                break (from, 0usize);
            }
            match cs.find_lf(&buf) {
                // 从第一个换行之后开始，保证块首是行首
                Some(i) if i + (unit as usize) < got => break (from, i + unit as usize),
                _ if want < MAX_LINE as u64 => want = (want * 2).min(MAX_LINE as u64),
                _ => {
                    // 超长行：在字符边界处切开
                    let mut k = 0usize;
                    while k < buf.len() && cs.align_skip(&buf[k..]) > 0 {
                        k += 1;
                    }
                    break (from, k);
                }
            }
        };
        let data = &buf[skip..];
        let c = Chunk {
            offset: from + skip as u64,
            first_line: None,
            data,
        };
        if !data.is_empty() && !f(&c) {
            return Ok(Walk::Stopped);
        }
        let new_end = from + skip as u64;
        if new_end >= end {
            break; // 防御：没有进展
        }
        end = new_end;
    }
    Ok(Walk::Completed)
}

/// 把块切成行：返回 (行首, 内容结束（不含 CR/LF）, 下一行首)，偏移相对于块首。
pub fn lines(data: &[u8], cs: Charset) -> Vec<(usize, usize, usize)> {
    let unit = cs.unit() as usize;
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        match cs.find_lf(&data[pos..]) {
            Some(i) => {
                let q = pos + i;
                let mut end = q;
                if q >= pos + unit && cs.is_cr(&data[q - unit..]) {
                    end = q - unit;
                }
                out.push((pos, end, q + unit));
                pos = q + unit;
            }
            None => {
                out.push((pos, data.len(), data.len()));
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemSource;

    fn src(n: usize) -> (MemSource, Vec<u64>) {
        let mut data = Vec::new();
        let mut starts = Vec::new();
        for i in 0..n {
            starts.push(data.len() as u64);
            data.extend_from_slice(format!("line {i} {}\r\n", "x".repeat(i % 50)).as_bytes());
        }
        (MemSource::new(data), starts)
    }

    #[test]
    fn forward_chunks_are_line_aligned_and_count_lines() {
        let (s, starts) = src(10_000);
        let cancel = AtomicBool::new(false);
        let mut seen_lines = 0u64;
        let mut total = 0u64;
        let w = forward(
            &s,
            Charset::Utf8,
            0,
            Some(0),
            s.len(),
            4096,
            &cancel,
            &mut |c| {
                assert_eq!(c.first_line, Some(seen_lines));
                assert!(
                    starts.binary_search(&c.offset).is_ok(),
                    "chunk not at line start"
                );
                assert!(c.data.ends_with(b"\n") || c.offset + c.data.len() as u64 == s.len());
                seen_lines += Charset::Utf8.count_lf(c.data);
                total += c.data.len() as u64;
                true
            },
        )
        .unwrap();
        assert_eq!(w, Walk::Completed);
        assert_eq!(total, s.len());
        assert_eq!(seen_lines, 10_000);
    }

    #[test]
    fn backward_covers_everything_in_reverse() {
        let (s, starts) = src(5000);
        let cancel = AtomicBool::new(false);
        let mut chunks = Vec::new();
        backward(&s, Charset::Utf8, 0, s.len(), 4096, &cancel, &mut |c| {
            assert!(
                starts.binary_search(&c.offset).is_ok(),
                "chunk not at line start"
            );
            chunks.push((c.offset, c.data.len() as u64));
            true
        })
        .unwrap();
        // 连续、无重叠、从后往前
        let mut end = s.len();
        for (off, len) in &chunks {
            assert_eq!(off + len, end);
            end = *off;
        }
        assert_eq!(end, 0);
    }

    #[test]
    fn stop_and_cancel() {
        let (s, _) = src(5000);
        let cancel = AtomicBool::new(false);
        let mut n = 0;
        let w = forward(
            &s,
            Charset::Utf8,
            0,
            None,
            s.len(),
            1024,
            &cancel,
            &mut |_| {
                n += 1;
                n < 3
            },
        )
        .unwrap();
        assert_eq!(w, Walk::Stopped);
        let cancel = AtomicBool::new(true);
        let w = backward(&s, Charset::Utf8, 0, s.len(), 1024, &cancel, &mut |_| true).unwrap();
        assert_eq!(w, Walk::Cancelled);
    }

    #[test]
    fn very_long_line_is_split() {
        let mut data = b"a\n".to_vec();
        data.extend(std::iter::repeat_n(b'z', 100_000));
        data.extend_from_slice(b"\nb\n");
        let s = MemSource::new(data.clone());
        let cancel = AtomicBool::new(false);
        let mut total = 0;
        forward(
            &s,
            Charset::Utf8,
            0,
            Some(0),
            s.len(),
            1024,
            &cancel,
            &mut |c| {
                total += c.data.len();
                true
            },
        )
        .unwrap();
        assert_eq!(total, data.len());
        let mut total = 0;
        backward(&s, Charset::Utf8, 0, s.len(), 1024, &cancel, &mut |c| {
            total += c.data.len();
            true
        })
        .unwrap();
        assert_eq!(total, data.len());
    }

    #[test]
    fn utf16_lines() {
        let data: Vec<u8> = "ab\r\ncd\nef"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let ls = lines(&data, Charset::Utf16Le);
        assert_eq!(ls, vec![(0, 4, 8), (8, 12, 14), (14, 18, 18)]);
        let ls = lines(b"x\r\n\ny", Charset::Utf8);
        assert_eq!(ls, vec![(0, 1, 3), (3, 3, 4), (4, 5, 5)]);
    }
}
