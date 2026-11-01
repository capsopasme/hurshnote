//! Hex 视图：固定 16 字节一行。
//!
//! 行格式（等宽字体）：`偏移  00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F  文本`
//! 文本列只显示可打印 ASCII，其他字节显示为 `.`，保证列对齐。

use std::fmt::Write as _;

/// 每行字节数。
pub const ROW: u64 = 16;

/// 列布局（以字符为单位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// 偏移列的位数
    pub addr: usize,
}

impl Layout {
    /// 按文件长度确定偏移列宽度（至少 8 位十六进制）。
    pub fn for_len(len: u64) -> Layout {
        let digits = if len == 0 {
            1
        } else {
            (64 - (len - 1).leading_zeros() as usize).div_ceil(4)
        };
        Layout {
            addr: digits.max(8),
        }
    }

    /// 第 i 个字节（0..16）在十六进制区的起始列。
    pub fn hex_col(&self, i: usize) -> usize {
        self.addr + 2 + i * 3 + usize::from(i >= 8)
    }

    /// 文本区起始列。
    pub fn text_col(&self) -> usize {
        // 最后一个字节的两位 + 一个空格，再空两格
        self.hex_col(15) + 3 + 2
    }

    /// 一行的总列数。
    pub fn width(&self) -> usize {
        self.text_col() + ROW as usize
    }

    /// 列号 → (字节下标, 是否在文本区)。不在任何字节上时取最近的字节。
    pub fn byte_at_col(&self, col: f32) -> (usize, bool) {
        let tc = self.text_col() as f32;
        if col >= tc - 1.0 {
            let i = (col - tc).floor().clamp(0.0, 15.0) as usize;
            return (i, true);
        }
        let mut best = 0;
        for i in 0..ROW as usize {
            if col >= self.hex_col(i) as f32 - 0.5 {
                best = i;
            }
        }
        (best, false)
    }

    /// 格式化一行。`bytes` 不足 16 字节时后面留空。
    pub fn format(&self, offset: u64, bytes: &[u8]) -> String {
        let mut s = String::with_capacity(self.width());
        let _ = write!(s, "{:0width$X}  ", offset, width = self.addr);
        for i in 0..ROW as usize {
            if i == 8 {
                s.push(' ');
            }
            match bytes.get(i) {
                Some(b) => {
                    let _ = write!(s, "{b:02X} ");
                }
                None => s.push_str("   "),
            }
        }
        s.push_str("  ");
        for &b in bytes.iter().take(ROW as usize) {
            s.push(if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '.'
            });
        }
        s
    }
}

/// 总行数（空文件也显示一行）。
pub fn row_count(len: u64) -> u64 {
    len.div_ceil(ROW).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_columns() {
        let l = Layout::for_len(100);
        assert_eq!(l.addr, 8);
        let s = l.format(0x10, b"Hello, World!\n\x00\xff");
        assert_eq!(
            s,
            "00000010  48 65 6C 6C 6F 2C 20 57  6F 72 6C 64 21 0A 00 FF   Hello, World!..."
        );
        assert_eq!(&s[l.hex_col(0)..l.hex_col(0) + 2], "48");
        assert_eq!(&s[l.hex_col(8)..l.hex_col(8) + 2], "6F");
        assert_eq!(&s[l.hex_col(15)..l.hex_col(15) + 2], "FF");
        assert_eq!(&s[l.text_col()..l.text_col() + 5], "Hello");
        assert_eq!(s.len(), l.width());
        assert_eq!(l.byte_at_col(l.hex_col(3) as f32 + 1.0), (3, false));
        assert_eq!(l.byte_at_col(l.text_col() as f32 + 4.5), (4, true));
        assert_eq!(l.byte_at_col(0.0), (0, false));
    }

    #[test]
    fn short_row_keeps_alignment() {
        let l = Layout::for_len(3);
        let s = l.format(0, b"ab");
        assert_eq!(&s[l.text_col()..], "ab");
    }

    #[test]
    fn wide_addresses() {
        assert_eq!(Layout::for_len(0x1_0000_0000).addr, 8);
        assert_eq!(Layout::for_len(0x1_0000_0001).addr, 9);
        assert_eq!(Layout::for_len(5 << 30).addr, 9);
        assert_eq!(row_count(0), 1);
        assert_eq!(row_count(16), 1);
        assert_eq!(row_count(17), 2);
    }
}
