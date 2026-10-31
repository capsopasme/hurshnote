//! 编码识别与转换。
//!
//! 目前只处理 UTF-8（含 BOM）。完整的编码嗅探（UTF-16、GBK/GB18030、chardetng）在 M1 实现。

use std::fmt;

pub const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const UTF16LE_BOM: [u8; 2] = [0xFF, 0xFE];
const UTF16BE_BOM: [u8; 2] = [0xFE, 0xFF];

/// 解码得到的 UTF-8 文本（借用原缓冲区，不复制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf8Text<'a> {
    /// 去掉 BOM 后的文本。
    pub text: &'a str,
    /// 原文件是否带 UTF-8 BOM。保存时需保持一致。
    pub bom: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// 不是合法的 UTF-8；`valid_up_to` 为第一个非法字节的偏移（相对去掉 BOM 后的内容）。
    NotUtf8 { valid_up_to: usize },
    /// 带 UTF-16 BOM 的文件。
    Utf16,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::NotUtf8 { valid_up_to } => {
                write!(
                    f,
                    "不是有效的 UTF-8 文本（第 {valid_up_to} 字节处出现非法序列）"
                )
            }
            DecodeError::Utf16 => write!(f, "文件是 UTF-16 编码"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// 把字节按 UTF-8 解码。带 UTF-8 BOM 时去掉 BOM 并记录。
pub fn decode_utf8(bytes: &[u8]) -> Result<Utf8Text<'_>, DecodeError> {
    if bytes.starts_with(&UTF16LE_BOM) || bytes.starts_with(&UTF16BE_BOM) {
        return Err(DecodeError::Utf16);
    }
    let (bom, body) = match bytes.strip_prefix(&UTF8_BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    match std::str::from_utf8(body) {
        Ok(text) => Ok(Utf8Text { text, bom }),
        Err(e) => Err(DecodeError::NotUtf8 {
            valid_up_to: e.valid_up_to(),
        }),
    }
}

/// 把 UTF-8 文本编码为要写入磁盘的字节。
pub fn encode_utf8(text: &[u8], bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + if bom { 3 } else { 0 });
    if bom {
        out.extend_from_slice(&UTF8_BOM);
    }
    out.extend_from_slice(text);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(name: &str) -> Vec<u8> {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus/");
        std::fs::read(format!("{p}{name}")).unwrap_or_else(|e| panic!("读取语料 {name} 失败: {e}"))
    }

    #[test]
    fn plain_utf8() {
        let t = decode_utf8("你好\nworld".as_bytes()).unwrap();
        assert_eq!(t.text, "你好\nworld");
        assert!(!t.bom);
    }

    #[test]
    fn utf8_with_bom() {
        let mut b = UTF8_BOM.to_vec();
        b.extend_from_slice("中文".as_bytes());
        let t = decode_utf8(&b).unwrap();
        assert_eq!(t.text, "中文");
        assert!(t.bom);
    }

    #[test]
    fn roundtrip_keeps_bom() {
        for bom in [false, true] {
            let bytes = encode_utf8("行1\r\n行2".as_bytes(), bom);
            let t = decode_utf8(&bytes).unwrap();
            assert_eq!(t.bom, bom);
            assert_eq!(encode_utf8(t.text.as_bytes(), t.bom), bytes);
        }
    }

    #[test]
    fn empty_file() {
        let t = decode_utf8(b"").unwrap();
        assert_eq!(t.text, "");
        assert!(!t.bom);
    }

    #[test]
    fn utf16_rejected() {
        assert_eq!(decode_utf8(&[0xFF, 0xFE, b'a', 0]), Err(DecodeError::Utf16));
        assert_eq!(decode_utf8(&[0xFE, 0xFF, 0, b'a']), Err(DecodeError::Utf16));
    }

    #[test]
    fn corpus_utf8_files_decode() {
        for name in [
            "utf8.txt",
            "utf8-bom.txt",
            "crlf.txt",
            "mixed-eol.txt",
            "ascii.txt",
        ] {
            assert!(decode_utf8(&corpus(name)).is_ok(), "{name}");
        }
        assert!(decode_utf8(&corpus("utf8-bom.txt")).unwrap().bom);
    }

    #[test]
    fn corpus_gbk_is_not_utf8() {
        assert!(matches!(
            decode_utf8(&corpus("gbk.txt")),
            Err(DecodeError::NotUtf8 { .. })
        ));
    }

    #[test]
    fn corpus_utf16_detected() {
        assert_eq!(
            decode_utf8(&corpus("utf16le-bom.txt")),
            Err(DecodeError::Utf16)
        );
        assert_eq!(
            decode_utf8(&corpus("utf16be-bom.txt")),
            Err(DecodeError::Utf16)
        );
    }
}
