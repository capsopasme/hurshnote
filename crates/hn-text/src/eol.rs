//! 换行符检测。

/// 换行符类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Eol {
    Crlf,
    Lf,
    Cr,
}

impl Eol {
    /// 状态栏显示用的名称。
    pub fn label(self) -> &'static str {
        match self {
            Eol::Crlf => "CRLF",
            Eol::Lf => "LF",
            Eol::Cr => "CR",
        }
    }

    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Eol::Crlf => b"\r\n",
            Eol::Lf => b"\n",
            Eol::Cr => b"\r",
        }
    }
}

/// 各类换行符的出现次数。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EolStats {
    pub crlf: usize,
    pub lf: usize,
    pub cr: usize,
}

impl EolStats {
    /// 统计字节序列中的换行符。
    ///
    /// 适用于 UTF-8、GBK/GB18030 等 ASCII 兼容编码（这些编码的多字节序列中不会出现 0x0A/0x0D）。
    /// UTF-16 文本需先解码。
    pub fn scan(bytes: &[u8]) -> Self {
        let mut s = EolStats::default();
        let mut i = 0;
        let n = bytes.len();
        while i < n {
            match bytes[i] {
                b'\r' => {
                    if i + 1 < n && bytes[i + 1] == b'\n' {
                        s.crlf += 1;
                        i += 2;
                        continue;
                    }
                    s.cr += 1;
                }
                b'\n' => s.lf += 1,
                _ => {}
            }
            i += 1;
        }
        s
    }

    pub fn total(&self) -> usize {
        self.crlf + self.lf + self.cr
    }

    /// 是否混用了多种换行符。
    pub fn is_mixed(&self) -> bool {
        [self.crlf, self.lf, self.cr]
            .iter()
            .filter(|&&c| c > 0)
            .count()
            > 1
    }

    /// 出现最多的换行符；没有换行符时返回 `None`。数量相同时优先级为 CRLF > LF > CR。
    pub fn dominant(&self) -> Option<Eol> {
        if self.total() == 0 {
            return None;
        }
        let mut best = (Eol::Crlf, self.crlf);
        for cand in [(Eol::Lf, self.lf), (Eol::Cr, self.cr)] {
            if cand.1 > best.1 {
                best = cand;
            }
        }
        Some(best.0)
    }
}

/// 检测文本的主要换行符，没有换行符时返回 `default`。
pub fn detect(bytes: &[u8], default: Eol) -> Eol {
    EolStats::scan(bytes).dominant().unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_each_kind() {
        let s = EolStats::scan(b"a\r\nb\nc\rd\r\n");
        assert_eq!(
            s,
            EolStats {
                crlf: 2,
                lf: 1,
                cr: 1
            }
        );
        assert!(s.is_mixed());
        assert_eq!(s.dominant(), Some(Eol::Crlf));
    }

    #[test]
    fn trailing_cr_is_cr() {
        let s = EolStats::scan(b"abc\r");
        assert_eq!(s.cr, 1);
        assert_eq!(s.crlf, 0);
    }

    #[test]
    fn empty_uses_default() {
        assert_eq!(detect(b"", Eol::Lf), Eol::Lf);
        assert_eq!(detect(b"no newline", Eol::Crlf), Eol::Crlf);
    }

    #[test]
    fn tie_prefers_crlf_then_lf() {
        assert_eq!(detect(b"a\r\nb\n", Eol::Cr), Eol::Crlf);
        assert_eq!(detect(b"a\nb\r", Eol::Crlf), Eol::Lf);
    }

    #[test]
    fn single_kind_not_mixed() {
        let s = EolStats::scan(b"1\n2\n3\n");
        assert!(!s.is_mixed());
        assert_eq!(s.dominant(), Some(Eol::Lf));
    }
}
