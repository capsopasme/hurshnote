//! 显示行导航。
//!
//! 大文件视图以"显示行"为单位滚动。不超过 [`SEG`] 字节的行占一个显示行；更长的行在
//! 按绝对偏移划分的网格点（`data_start + k * SEG`，再对齐到字符边界）处折开。
//! 这样任意偏移所在显示行的起点都只需向前、向后各扫描不超过 `SEG` 字节就能确定，
//! 与行索引是否建好无关，也不需要扫描整条超长行。

use crate::charset::Charset;
use crate::source::Reader;

/// 显示行的最大字节数（超长行按此折行）。
pub const SEG: u64 = 2048;

/// 一个显示行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub start: u64,
    /// 显示内容的结束位置（不含行尾的 CR/LF）
    pub content_end: u64,
    /// 下一显示行的起点；None 表示这是最后一个显示行
    pub next: Option<u64>,
    /// 本显示行是否是一行（逻辑行）的开头
    pub line_start: bool,
}

/// 在 [`Reader`] 上做显示行导航所需的上下文。
#[derive(Clone, Copy)]
pub struct Nav<'a> {
    pub r: &'a Reader,
    pub cs: Charset,
    pub data_start: u64,
}

impl<'a> Nav<'a> {
    pub fn new(r: &'a Reader, cs: Charset, data_start: u64) -> Self {
        Nav { r, cs, data_start }
    }

    fn len(&self) -> u64 {
        self.r.len()
    }

    /// 把偏移对齐到码元（UTF-16 为相对 data_start 的偶数位置）并限制在数据范围内。
    pub fn clamp(&self, off: u64) -> u64 {
        let off = off.clamp(self.data_start, self.len().max(self.data_start));
        let unit = self.cs.unit();
        off - (off - self.data_start) % unit
    }

    /// 网格点 `g` 对齐到字符边界后的位置。
    fn align(&self, g: u64) -> u64 {
        let skip = self.cs.align_skip(&self.r.peek(g, 4));
        g + skip as u64
    }

    /// 不大于 `x` 的、已对齐到字符边界的网格点。
    fn grid_floor(&self, x: u64) -> u64 {
        let rel = x - self.data_start;
        let g = self.data_start + rel / SEG * SEG;
        let a = self.align(g);
        if a <= x {
            return a;
        }
        if g == self.data_start {
            return self.data_start;
        }
        // x 落在跨网格点的字符内部：属于上一个网格点开始的显示行
        self.align(g - SEG)
    }

    /// `off` 处（码元对齐）之前紧挨着的是否为 LF，即 `off` 是否为逻辑行的开头。
    pub fn is_line_start(&self, off: u64) -> bool {
        let unit = self.cs.unit();
        off <= self.data_start
            || (off >= unit && self.cs.is_lf(&self.r.peek(off - unit, unit as usize)))
    }

    /// 从行首 `s` 开始的行是否"短"（不超过 SEG 字节就遇到 LF 或文件末尾）。
    fn line_is_short(&self, s: u64) -> bool {
        match self.r.find_lf(self.cs, s, s + SEG) {
            Some(_) => true,
            None => self.len() <= s + SEG,
        }
    }

    /// 偏移 `x` 所在显示行的起点。
    pub fn row_start(&self, x: u64) -> u64 {
        let x = self.clamp(x);
        if x <= self.data_start {
            return self.data_start;
        }
        let unit = self.cs.unit();
        let lo = self.data_start.max(x.saturating_sub(SEG));
        let s = match self.r.rfind_lf(self.cs, lo, x) {
            Some(p) => p + unit,
            None if lo == self.data_start => self.data_start,
            // SEG 字节内没有换行：一定在超长行中
            None => {
                let gf = self.grid_floor(x);
                if gf < lo {
                    // 网格点退回到了更早的位置，中间可能有行首
                    if let Some(p) = self.r.rfind_lf(self.cs, gf, lo) {
                        return (p + unit).max(gf);
                    }
                }
                return gf;
            }
        };
        if self.line_is_short(s) {
            s
        } else {
            s.max(self.grid_floor(x))
        }
    }

    /// 以 `start` 为起点（必须是某个显示行的起点）的显示行。
    pub fn row(&self, start: u64) -> Row {
        let len = self.len();
        let unit = self.cs.unit();
        let line_start = self.is_line_start(start);
        let limit = if line_start && self.line_is_short(start) {
            start + SEG
        } else {
            let rel = start - self.data_start;
            let g = self.data_start + (rel / SEG + 1) * SEG;
            let mut a = self.align(g);
            if a <= start {
                a = self.align(g + SEG);
            }
            a
        };
        match self.r.find_lf(self.cs, start, limit.min(len)) {
            Some(q) => {
                let mut end = q;
                if q >= start + unit && self.cs.is_cr(&self.r.peek(q - unit, unit as usize)) {
                    end = q - unit;
                }
                Row {
                    start,
                    content_end: end,
                    next: Some(q + unit),
                    line_start,
                }
            }
            None if limit >= len => Row {
                start,
                content_end: len,
                next: None,
                line_start,
            },
            None => Row {
                start,
                content_end: limit,
                next: Some(limit),
                line_start,
            },
        }
    }

    /// 下一显示行的起点。
    pub fn next_row(&self, start: u64) -> Option<u64> {
        self.row(start).next.filter(|&n| n > start)
    }

    /// 上一显示行的起点。
    pub fn prev_row(&self, start: u64) -> Option<u64> {
        if start <= self.data_start {
            return None;
        }
        let p = self.row_start(start - self.cs.unit());
        if p < start {
            Some(p)
        } else {
            Some(self.data_start.max(start.saturating_sub(SEG)))
        }
    }

    /// 从 `start` 向后（`n > 0`）或向前（`n < 0`）移动 `n` 个显示行，到头为止。
    pub fn step(&self, start: u64, n: i64) -> u64 {
        let mut cur = start;
        if n >= 0 {
            for _ in 0..n {
                match self.next_row(cur) {
                    Some(x) => cur = x,
                    None => break,
                }
            }
        } else {
            for _ in 0..(-n) {
                match self.prev_row(cur) {
                    Some(x) => cur = x,
                    None => break,
                }
            }
        }
        cur
    }

    /// 最后一个显示行的起点。
    pub fn last_row(&self) -> u64 {
        self.row_start(self.len())
    }

    /// `off` 之后的下一个字符边界（不超过文件末尾）。
    pub fn next_char(&self, off: u64) -> u64 {
        let bytes = self.r.peek(off, 4);
        if bytes.is_empty() {
            return off;
        }
        off + self.cs.char_len(&bytes).max(1) as u64
    }

    /// `off` 之前的上一个字符边界：从所在显示行的起点逐字符前进。
    pub fn prev_char(&self, off: u64) -> u64 {
        if off <= self.data_start {
            return self.data_start;
        }
        let rs = self.row_start(off - self.cs.unit());
        let bytes = self.r.read(rs, (off - rs) as usize);
        let mut i = 0usize;
        let mut last = 0usize;
        while i < bytes.len() {
            last = i;
            i += self.cs.char_len(&bytes[i..]).max(1);
        }
        rs + last as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemSource;
    use std::sync::Arc;

    fn reader(bytes: &[u8]) -> Reader {
        Reader::new(Arc::new(MemSource::new(bytes.to_vec())))
    }

    /// 从头遍历所有显示行，检查与 row_start / prev_row 的一致性，返回所有显示行。
    fn walk(nav: &Nav) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut cur = nav.data_start;
        loop {
            let row = nav.row(cur);
            assert!(row.content_end >= row.start);
            assert!(
                row.content_end - row.start <= SEG + 4,
                "row too long at {cur}"
            );
            // 行内任意位置的 row_start 都是这一行
            let probe_end = row.next.unwrap_or(nav.r.len()).min(row.start + SEG + 4);
            let mut p = row.start;
            while p < probe_end {
                assert_eq!(nav.row_start(p), row.start, "row_start({p})");
                p += 97.max(nav.cs.unit());
                p = nav.clamp(p);
                if p >= probe_end {
                    break;
                }
            }
            rows.push(row);
            match row.next {
                Some(n) => {
                    assert!(n > cur);
                    assert_eq!(nav.prev_row(n), Some(cur), "prev_row({n})");
                    cur = n;
                }
                None => break,
            }
        }
        assert_eq!(nav.last_row(), rows.last().unwrap().start);
        rows
    }

    #[test]
    fn short_lines() {
        let data = b"one\r\ntwo\n\nthree";
        let r = reader(data);
        let nav = Nav::new(&r, Charset::Utf8, 0);
        let rows = walk(&nav);
        let texts: Vec<&[u8]> = rows
            .iter()
            .map(|x| &data[x.start as usize..x.content_end as usize])
            .collect();
        assert_eq!(texts, vec![&b"one"[..], b"two", b"", b"three"]);
        assert!(rows.iter().all(|x| x.line_start));
    }

    #[test]
    fn trailing_newline_gives_empty_last_row() {
        let data = b"a\nb\n";
        let r = reader(data);
        let nav = Nav::new(&r, Charset::Utf8, 0);
        let rows = walk(&nav);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].start, 4);
        assert_eq!(rows[2].content_end, 4);
    }

    #[test]
    fn long_line_wraps_on_grid() {
        let mut data = b"short\n".to_vec();
        data.extend(std::iter::repeat_n(b'x', 10_000));
        data.extend_from_slice(b"\nend\n");
        let r = reader(&data);
        let nav = Nav::new(&r, Charset::Utf8, 0);
        let rows = walk(&nav);
        // 第二行折成多个显示行，只有第一个显示行是行首
        let long: Vec<&Row> = rows
            .iter()
            .filter(|x| x.start >= 6 && x.start < 10_006)
            .collect();
        assert!(long.len() >= 5);
        assert!(long[0].line_start);
        assert!(long[1..].iter().all(|x| !x.line_start));
        assert_eq!(long[1].start % SEG, 0);
    }

    #[test]
    fn huge_line_without_newline_and_utf8_alignment() {
        // 每个字符 3 字节，网格点会落在字符中间，需要对齐
        let data = "中".repeat(5000).into_bytes();
        let r = reader(&data);
        let nav = Nav::new(&r, Charset::Utf8, 0);
        let rows = walk(&nav);
        for row in &rows {
            assert!(
                row.start % 3 == 0,
                "row start {} not on char boundary",
                row.start
            );
        }
        assert_eq!(rows.last().unwrap().content_end, data.len() as u64);
    }

    #[test]
    fn utf16_rows_and_bom() {
        let text = format!("第一行\r\n{}\r\n末行", "长".repeat(3000));
        let mut data = vec![0xFF, 0xFE];
        data.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let r = reader(&data);
        let nav = Nav::new(&r, Charset::Utf16Le, 2);
        let rows = walk(&nav);
        assert_eq!(rows[0].start, 2);
        assert_eq!(rows[0].content_end, 2 + 6);
        assert!(rows.iter().all(|x| x.start % 2 == 0));
        assert!(rows.len() > 3);
    }

    #[test]
    fn line_exactly_seg_long() {
        for extra in [-1i64, 0, 1] {
            let n = (SEG as i64 + extra) as usize;
            let mut data = vec![b'a'; n];
            data.extend_from_slice(b"\nnext\n");
            let r = reader(&data);
            let nav = Nav::new(&r, Charset::Utf8, 0);
            walk(&nav);
        }
    }

    #[test]
    fn step_and_chars() {
        let data = "a中\nb😀\nc".as_bytes();
        let r = reader(data);
        let nav = Nav::new(&r, Charset::Utf8, 0);
        assert_eq!(nav.step(0, 2), 11);
        assert_eq!(nav.step(11, -5), 0);
        assert_eq!(nav.next_char(1), 4);
        assert_eq!(nav.prev_char(4), 1);
        assert_eq!(nav.prev_char(10), 6);
        assert_eq!(nav.prev_char(5), 4);
        assert_eq!(nav.row_start(data.len() as u64), 11);
    }

    #[test]
    fn empty() {
        let r = reader(b"");
        let nav = Nav::new(&r, Charset::Utf8, 0);
        let row = nav.row(0);
        assert_eq!(row.next, None);
        assert_eq!(nav.prev_row(0), None);
        assert_eq!(nav.last_row(), 0);
    }

    #[test]
    fn random_files_are_consistent() {
        // 简单的确定性伪随机：混合短行、空行、超长行与多字节字符
        let mut seed = 12345u64;
        let mut rnd = |m: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        for _ in 0..20 {
            let mut s = String::new();
            for _ in 0..rnd(40) {
                let kind = rnd(10);
                let n = match kind {
                    0 => rnd(6000) as usize,
                    1 => 0,
                    _ => rnd(120) as usize,
                };
                for _ in 0..n {
                    s.push(match rnd(5) {
                        0 => '中',
                        1 => 'é',
                        _ => 'x',
                    });
                }
                s.push_str(if rnd(2) == 0 { "\n" } else { "\r\n" });
            }
            let r = reader(s.as_bytes());
            walk(&Nav::new(&r, Charset::Utf8, 0));
        }
    }
}
