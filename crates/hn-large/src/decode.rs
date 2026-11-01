//! 显示行解码。
//!
//! 把一段源字节解码成 UTF-16（交给 DirectWrite 排版），同时记录每个 UTF-16 码元对应的
//! 源字节偏移，用于鼠标命中测试、选区高亮。也可以解码成 UTF-8（供搜索使用），同样带偏移表。
//!
//! UTF-8 / UTF-16 / ASCII 字节在本模块内解码；其他编码的非 ASCII 字符交给调用方的回调
//! （通常是 encoding_rs），回调对单个字符的字节返回解码结果。
//!
//! 显示时把控制字符换成 Unicode 控制图形（U+2400 起），制表符保留。

use crate::charset::Charset;

/// 遗留编码的单字符解码回调：输入一个字符的字节，返回解码后的字符串（通常 1 个字符）。
pub type LegacyDecode<'a> = &'a dyn Fn(&[u8]) -> String;

/// 解码后的显示行。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowText {
    /// UTF-16 文本
    pub text: Vec<u16>,
    /// `map[i]` 为第 i 个码元所属字符的源字节偏移（相对于行首）；`map[text.len()]` 为总字节数
    pub map: Vec<u32>,
}

impl RowText {
    /// 源字节偏移（相对行首）→ 码元位置：落在字符中间时取该字符的起点。
    pub fn unit_of(&self, byte: u32) -> usize {
        // map 单调不减，找第一个 >= byte 的位置
        let i = self.map.partition_point(|&b| b < byte);
        i.min(self.text.len())
    }

    /// 码元位置 → 源字节偏移（相对行首）。
    pub fn byte_of(&self, unit: usize) -> u32 {
        self.map[unit.min(self.map.len() - 1)]
    }
}

/// 逐字符遍历：回调参数为 (字符起始偏移, 字符字节数, 解码结果)。
fn for_each_char(
    bytes: &[u8],
    cs: Charset,
    legacy: Option<LegacyDecode>,
    mut f: impl FnMut(usize, usize, &mut dyn Iterator<Item = char>),
) {
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        let n = cs.char_len(rest).max(1).min(rest.len());
        let s = &rest[..n];
        match cs {
            Charset::Utf8 => {
                let c = std::str::from_utf8(s)
                    .ok()
                    .and_then(|t| t.chars().next())
                    .unwrap_or('\u{FFFD}');
                f(i, n, &mut std::iter::once(c));
            }
            Charset::Utf16Le | Charset::Utf16Be => {
                let units: Vec<u16> = s
                    .chunks_exact(2)
                    .map(|p| {
                        if cs == Charset::Utf16Le {
                            u16::from_le_bytes([p[0], p[1]])
                        } else {
                            u16::from_be_bytes([p[0], p[1]])
                        }
                    })
                    .collect();
                let mut it = char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}'));
                if n < 2 {
                    f(i, n, &mut std::iter::once('\u{FFFD}'));
                } else {
                    f(i, n, &mut it);
                }
            }
            _ => {
                if s[0] < 0x80 {
                    f(i, n, &mut std::iter::once(s[0] as char));
                } else {
                    match legacy {
                        Some(dec) => {
                            let t = dec(s);
                            f(i, n, &mut t.chars());
                        }
                        None => f(i, n, &mut std::iter::once('\u{FFFD}')),
                    }
                }
            }
        }
        i += n;
    }
}

/// 显示用的字符替换：控制字符换成控制图形，制表符保留。
#[inline]
fn visible(c: char) -> char {
    match c {
        '\t' => '\t',
        '\u{0}'..='\u{1F}' => char::from_u32(0x2400 + c as u32).unwrap_or('?'),
        '\u{7F}' => '\u{2421}',
        _ => c,
    }
}

/// 解码一个显示行（不含行尾换行符）。
pub fn decode_row(bytes: &[u8], cs: Charset, legacy: Option<LegacyDecode>) -> RowText {
    let mut out = RowText {
        text: Vec::with_capacity(bytes.len()),
        map: Vec::with_capacity(bytes.len() + 1),
    };
    let mut buf = [0u16; 2];
    for_each_char(bytes, cs, legacy, |off, _n, chars| {
        for c in chars {
            for u in visible(c).encode_utf16(&mut buf) {
                out.text.push(*u);
                out.map.push(off as u32);
            }
        }
    });
    out.map.push(bytes.len() as u32);
    out
}

/// 解码为 UTF-8（不替换控制字符），返回文本与偏移表：`map` 中每项为
/// (文本中的字节位置, 源字节偏移)，按字符记录，末尾追加 (文本长度, 源长度)。
pub fn decode_mapped(
    bytes: &[u8],
    cs: Charset,
    legacy: Option<LegacyDecode>,
) -> (String, Vec<(u32, u32)>) {
    let mut text = String::with_capacity(bytes.len());
    let mut map = Vec::with_capacity(bytes.len() / 2 + 1);
    for_each_char(bytes, cs, legacy, |off, _n, chars| {
        map.push((text.len() as u32, off as u32));
        for c in chars {
            text.push(c);
        }
    });
    map.push((text.len() as u32, bytes.len() as u32));
    (text, map)
}

/// 在 [`decode_mapped`] 的偏移表中，把文本字节位置换算成源字节偏移。
pub fn source_offset(map: &[(u32, u32)], text_pos: usize) -> u32 {
    let i = map.partition_point(|&(t, _)| (t as usize) <= text_pos);
    if i == 0 {
        0
    } else {
        map[i - 1].1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_row_with_map() {
        let s = "a中😀\tb\u{1}";
        let r = decode_row(s.as_bytes(), Charset::Utf8, None);
        let expect: Vec<u16> = "a中😀\tb\u{2401}".encode_utf16().collect();
        assert_eq!(r.text, expect);
        // 😀 占两个码元，都映射到同一个字节偏移
        assert_eq!(r.map, vec![0, 1, 4, 4, 8, 9, 10, 11]);
        assert_eq!(r.unit_of(4), 2);
        assert_eq!(r.unit_of(5), 4); // 字符中间：取下一个字符（命中测试不会产生这种偏移）
        assert_eq!(r.byte_of(4), 8);
        assert_eq!(r.byte_of(100), 11);
    }

    #[test]
    fn invalid_utf8_becomes_replacement() {
        let r = decode_row(&[0x41, 0xFF, 0x42], Charset::Utf8, None);
        assert_eq!(String::from_utf16(&r.text).unwrap(), "A\u{FFFD}B");
        assert_eq!(r.map, vec![0, 1, 2, 3]);
    }

    #[test]
    fn utf16_be() {
        let bytes: Vec<u8> = "x😀".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let r = decode_row(&bytes, Charset::Utf16Be, None);
        assert_eq!(String::from_utf16(&r.text).unwrap(), "x😀");
        assert_eq!(r.map, vec![0, 2, 2, 6]);
    }

    #[test]
    fn legacy_callback() {
        // 假的双字节编码：每个非 ASCII 双字节解码为 '汉'
        let dec = |b: &[u8]| -> String {
            if b.len() == 2 {
                "汉".into()
            } else {
                "\u{FFFD}".into()
            }
        };
        let r = decode_row(&[0x41, 0xB0, 0xA1, 0x42], Charset::Dbcs, Some(&dec));
        assert_eq!(String::from_utf16(&r.text).unwrap(), "A汉B");
        assert_eq!(r.map, vec![0, 1, 3, 4]);
        let (t, map) = decode_mapped(&[0x41, 0xB0, 0xA1, 0x42], Charset::Dbcs, Some(&dec));
        assert_eq!(t, "A汉B");
        assert_eq!(source_offset(&map, 1), 1);
        assert_eq!(source_offset(&map, 4), 3);
        assert_eq!(source_offset(&map, 5), 4);
    }

    #[test]
    fn mapped_utf8() {
        let (t, map) = decode_mapped(b"ab\xFFc", Charset::Utf8, None);
        assert_eq!(t, "ab\u{FFFD}c");
        // U+FFFD 在文本中占 3 字节，源中占 1 字节
        assert_eq!(source_offset(&map, 2), 2);
        assert_eq!(source_offset(&map, 5), 3);
        assert_eq!(source_offset(&map, t.len()), 4);
    }
}
