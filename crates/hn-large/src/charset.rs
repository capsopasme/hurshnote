//! 编码族：字符边界与换行符。
//!
//! 大文件视图只需要知道"一个字符占几个字节"和"换行符长什么样"，真正的解码交给调用方
//! （`hn-text` / encoding_rs）。所有 ASCII 兼容编码中 0x0A 都不会出现在多字节字符内部
//! （GB18030/GBK/Big5/Shift_JIS/EUC 的尾字节都不小于 0x30），因此按字节找 LF 是安全的。

/// 编码族。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    Utf16Le,
    Utf16Be,
    /// 单字节编码（Windows-125x、KOI8……）
    SingleByte,
    /// GB18030 / GBK：首字节 0x81–0xFE，第二字节为 0x30–0x39 时是四字节字符
    Gb18030,
    /// 首字节 0x81–0xFE 的双字节编码（Big5、EUC-KR）
    Dbcs,
    ShiftJis,
    EucJp,
}

impl Charset {
    /// 码元字节数（UTF-16 为 2，其他为 1）。
    pub fn unit(self) -> u64 {
        match self {
            Charset::Utf16Le | Charset::Utf16Be => 2,
            _ => 1,
        }
    }

    pub fn is_utf16(self) -> bool {
        self.unit() == 2
    }

    /// `bytes` 开头（已对齐到码元）是否为 LF。
    #[inline]
    pub fn is_lf(self, bytes: &[u8]) -> bool {
        match self {
            Charset::Utf16Le => bytes.len() >= 2 && bytes[0] == 0x0A && bytes[1] == 0,
            Charset::Utf16Be => bytes.len() >= 2 && bytes[0] == 0 && bytes[1] == 0x0A,
            _ => bytes.first() == Some(&b'\n'),
        }
    }

    /// `bytes` 开头是否为 CR。
    #[inline]
    pub fn is_cr(self, bytes: &[u8]) -> bool {
        match self {
            Charset::Utf16Le => bytes.len() >= 2 && bytes[0] == 0x0D && bytes[1] == 0,
            Charset::Utf16Be => bytes.len() >= 2 && bytes[0] == 0 && bytes[1] == 0x0D,
            _ => bytes.first() == Some(&b'\r'),
        }
    }

    /// 统计 `bytes` 中的 LF 个数。`bytes` 必须从码元边界开始。
    pub fn count_lf(self, bytes: &[u8]) -> u64 {
        match self {
            Charset::Utf16Le => bytes
                .chunks_exact(2)
                .filter(|p| p[0] == 0x0A && p[1] == 0)
                .count() as u64,
            Charset::Utf16Be => bytes
                .chunks_exact(2)
                .filter(|p| p[0] == 0 && p[1] == 0x0A)
                .count() as u64,
            // 这个写法会被自动向量化，速度可达每秒数 GB
            _ => bytes.iter().filter(|&&b| b == b'\n').count() as u64,
        }
    }

    /// 在 `bytes` 中找第一个 LF 的位置（码元对齐）。
    pub fn find_lf(self, bytes: &[u8]) -> Option<usize> {
        match self {
            Charset::Utf16Le | Charset::Utf16Be => bytes
                .chunks_exact(2)
                .position(|p| self.is_lf(p))
                .map(|i| i * 2),
            _ => bytes.iter().position(|&b| b == b'\n'),
        }
    }

    /// 在 `bytes` 中找最后一个 LF 的位置（码元对齐）。
    pub fn rfind_lf(self, bytes: &[u8]) -> Option<usize> {
        match self {
            Charset::Utf16Le | Charset::Utf16Be => {
                let n = bytes.len() / 2;
                (0..n)
                    .rev()
                    .find(|&i| self.is_lf(&bytes[i * 2..]))
                    .map(|i| i * 2)
            }
            _ => bytes.iter().rposition(|&b| b == b'\n'),
        }
    }

    /// 在 `bytes` 中找第 `n` 个（1 起始）LF 的位置。
    pub fn nth_lf(self, bytes: &[u8], n: u64) -> Option<usize> {
        if n == 0 {
            return None;
        }
        let mut seen = 0u64;
        match self {
            Charset::Utf16Le | Charset::Utf16Be => {
                for (i, p) in bytes.chunks_exact(2).enumerate() {
                    if self.is_lf(p) {
                        seen += 1;
                        if seen == n {
                            return Some(i * 2);
                        }
                    }
                }
                None
            }
            _ => {
                for (i, &b) in bytes.iter().enumerate() {
                    if b == b'\n' {
                        seen += 1;
                        if seen == n {
                            return Some(i);
                        }
                    }
                }
                None
            }
        }
    }

    /// 从 `bytes` 开头的字符占几个字节（至少 1 个码元；数据不完整或无效时按 1 个码元算）。
    pub fn char_len(self, bytes: &[u8]) -> usize {
        let Some(&b) = bytes.first() else {
            return 0;
        };
        let avail = bytes.len();
        let need = |n: usize, ok: &dyn Fn(&[u8]) -> bool| -> usize {
            if avail >= n && ok(&bytes[..n]) {
                n
            } else {
                1
            }
        };
        match self {
            Charset::Utf8 => {
                let cont = |s: &[u8]| s[1..].iter().all(|&c| (0x80..=0xBF).contains(&c));
                match b {
                    0x00..=0x7F => 1,
                    0xC2..=0xDF => need(2, &cont),
                    0xE0 => need(3, &|s| cont(s) && s[1] >= 0xA0),
                    0xED => need(3, &|s| cont(s) && s[1] <= 0x9F),
                    0xE1..=0xEF => need(3, &cont),
                    0xF0 => need(4, &|s| cont(s) && s[1] >= 0x90),
                    0xF4 => need(4, &|s| cont(s) && s[1] <= 0x8F),
                    0xF1..=0xF3 => need(4, &cont),
                    _ => 1,
                }
            }
            Charset::Utf16Le | Charset::Utf16Be => {
                if avail < 2 {
                    return avail;
                }
                let u = |i: usize| -> u16 {
                    if self == Charset::Utf16Le {
                        u16::from_le_bytes([bytes[i], bytes[i + 1]])
                    } else {
                        u16::from_be_bytes([bytes[i], bytes[i + 1]])
                    }
                };
                let hi = u(0);
                if (0xD800..=0xDBFF).contains(&hi)
                    && avail >= 4
                    && (0xDC00..=0xDFFF).contains(&u(2))
                {
                    4
                } else {
                    2
                }
            }
            Charset::SingleByte => 1,
            Charset::Gb18030 => match b {
                0x81..=0xFE => {
                    if avail >= 2 && (0x30..=0x39).contains(&bytes[1]) {
                        need(4, &|s| {
                            (0x81..=0xFE).contains(&s[2]) && (0x30..=0x39).contains(&s[3])
                        })
                    } else {
                        need(2, &|s| (0x40..=0xFE).contains(&s[1]) && s[1] != 0x7F)
                    }
                }
                _ => 1,
            },
            Charset::Dbcs => match b {
                0x81..=0xFE => need(2, &|s| s[1] >= 0x40),
                _ => 1,
            },
            Charset::ShiftJis => match b {
                0x81..=0x9F | 0xE0..=0xFC => need(2, &|s| s[1] >= 0x40 && s[1] != 0x7F),
                _ => 1,
            },
            Charset::EucJp => match b {
                0x8E => need(2, &|s| s[1] >= 0xA1),
                0x8F => need(3, &|s| s[1] >= 0xA1 && s[2] >= 0xA1),
                0xA1..=0xFE => need(2, &|s| s[1] >= 0xA1),
                _ => 1,
            },
        }
    }

    /// 从任意位置出发，需要向后跳过多少字节才能到达一个可能的字符起点。
    ///
    /// UTF-8 跳过续字节，UTF-16 跳过低代理项；其他多字节编码无法从任意位置判断边界，返回 0。
    pub fn align_skip(self, bytes: &[u8]) -> usize {
        match self {
            Charset::Utf8 => bytes
                .iter()
                .take(3)
                .take_while(|&&b| (0x80..=0xBF).contains(&b))
                .count(),
            Charset::Utf16Le | Charset::Utf16Be => {
                if bytes.len() < 2 {
                    return 0;
                }
                let u = if self == Charset::Utf16Le {
                    u16::from_le_bytes([bytes[0], bytes[1]])
                } else {
                    u16::from_be_bytes([bytes[0], bytes[1]])
                };
                if (0xDC00..=0xDFFF).contains(&u) {
                    2
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    /// `bytes` 中最后一个完整字符之后的位置（用于把块尾截断在字符边界上）。
    pub fn floor_boundary(self, bytes: &[u8]) -> usize {
        let mut i = 0;
        while i < bytes.len() {
            let n = self.char_len(&bytes[i..]);
            if i + n > bytes.len() {
                break;
            }
            // char_len 对不完整的多字节序列返回 1；末尾 4 字节内的不完整序列留给下一块
            if n == 1 && bytes.len() - i < 4 && self.maybe_truncated(&bytes[i..]) {
                break;
            }
            i += n.max(1);
        }
        i
    }

    /// `bytes`（位于缓冲区末尾）是否可能是被截断的多字节字符的开头。
    fn maybe_truncated(self, bytes: &[u8]) -> bool {
        let b = bytes[0];
        match self {
            Charset::Utf8 => {
                let want = match b {
                    0xC2..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    0xF0..=0xF4 => 4,
                    _ => return false,
                };
                bytes.len() < want && bytes[1..].iter().all(|&c| (0x80..=0xBF).contains(&c))
            }
            Charset::Gb18030 => (0x81..=0xFE).contains(&b) && bytes.len() < 4,
            Charset::Dbcs | Charset::ShiftJis | Charset::EucJp => b >= 0x81 && bytes.len() < 3,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_char_len() {
        let s = "a中😀é".as_bytes();
        let cs = Charset::Utf8;
        assert_eq!(cs.char_len(s), 1);
        assert_eq!(cs.char_len(&s[1..]), 3);
        assert_eq!(cs.char_len(&s[4..]), 4);
        assert_eq!(cs.char_len(&s[8..]), 2);
        // 无效字节、截断序列按 1 字节
        assert_eq!(cs.char_len(&[0xFF, 0x41]), 1);
        assert_eq!(cs.char_len(&[0xE4, 0xB8]), 1);
        assert_eq!(cs.char_len(&[0xC0, 0x80]), 1);
        assert_eq!(cs.align_skip(&s[2..]), 2);
        assert_eq!(cs.align_skip(&s[4..]), 0);
    }

    #[test]
    fn utf16_lf_and_surrogates() {
        let le: Vec<u8> = "a\n😀\r\n"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let cs = Charset::Utf16Le;
        assert_eq!(cs.count_lf(&le), 2);
        assert_eq!(cs.find_lf(&le), Some(2));
        assert_eq!(cs.rfind_lf(&le), Some(le.len() - 2));
        assert_eq!(cs.nth_lf(&le, 2), Some(le.len() - 2));
        assert_eq!(cs.char_len(&le[4..]), 4);
        assert_eq!(cs.align_skip(&le[6..]), 2);
        assert!(cs.is_cr(&le[8..]));
        // 0x0A 0x00 出现在奇数位置时不是 LF
        let tricky = [0x00, 0x0A, 0x00, 0x41, 0x00];
        assert_eq!(Charset::Utf16Le.count_lf(&tricky[..4]), 0);
        let be: Vec<u8> = "x\ny"
            .encode_utf16()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        assert_eq!(Charset::Utf16Be.find_lf(&be), Some(2));
    }

    #[test]
    fn gb18030_char_len() {
        // "中" = D6 D0；U+20000 = 95 32 82 36
        let cs = Charset::Gb18030;
        assert_eq!(cs.char_len(&[0xD6, 0xD0, 0x41]), 2);
        assert_eq!(cs.char_len(&[0x95, 0x32, 0x82, 0x36]), 4);
        assert_eq!(cs.char_len(&[0x41]), 1);
        assert_eq!(cs.char_len(&[0xD6]), 1);
    }

    #[test]
    fn floor_boundary_keeps_partial_char_for_next_chunk() {
        let s = "ab中".as_bytes();
        assert_eq!(Charset::Utf8.floor_boundary(&s[..4]), 2);
        assert_eq!(Charset::Utf8.floor_boundary(s), 5);
        assert_eq!(Charset::Gb18030.floor_boundary(&[0x41, 0xD6]), 1);
        assert_eq!(Charset::SingleByte.floor_boundary(&[0x41, 0xD6]), 2);
    }

    #[test]
    fn count_lf_single_byte() {
        assert_eq!(Charset::Utf8.count_lf(b"a\nb\r\nc\n"), 3);
        assert_eq!(Charset::Utf8.nth_lf(b"a\nb\r\nc\n", 2), Some(4));
        assert_eq!(Charset::Utf8.nth_lf(b"a\nb", 2), None);
    }
}
