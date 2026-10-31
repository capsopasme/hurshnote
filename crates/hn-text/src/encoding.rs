//! 编码识别与转换。
//!
//! 编辑器内部统一使用 UTF-8；本模块负责磁盘字节与 UTF-8 之间的转换。
//!
//! 嗅探顺序：BOM → UTF-16 无 BOM 启发式 → 二进制（含 NUL）→ 纯 ASCII → 严格 UTF-8 → chardetng 统计推断。
//! 二进制文件按 Windows-1252 解码：encoding_rs 中该编码对 256 个字节值一一映射，往返无损。

use encoding_rs::Encoding;

pub use encoding_rs;

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
const UTF16LE_BOM: &[u8] = &[0xFF, 0xFE];
const UTF16BE_BOM: &[u8] = &[0xFE, 0xFF];

/// 嗅探只看文件开头这么多字节（UTF-8 校验除外，校验全文）。
const SNIFF_LEN: usize = 64 * 1024;
/// 喂给 chardetng 的最大字节数。
const CHARDET_LEN: usize = 1024 * 1024;

/// 文本编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    /// 其他单/多字节编码（GBK、Big5、Shift_JIS、Windows-125x……）。
    Legacy(&'static Encoding),
}

impl TextEncoding {
    /// 显示名称（不含 BOM 信息）。
    pub fn name(self) -> String {
        match self {
            TextEncoding::Utf8 => "UTF-8".into(),
            TextEncoding::Utf16Le => "UTF-16 LE".into(),
            TextEncoding::Utf16Be => "UTF-16 BE".into(),
            TextEncoding::Legacy(e) => pretty_name(e),
        }
    }

    /// 持久化用的稳定标识（会话文件中保存）。
    pub fn id(self) -> String {
        match self {
            TextEncoding::Utf8 => "utf-8".into(),
            TextEncoding::Utf16Le => "utf-16le".into(),
            TextEncoding::Utf16Be => "utf-16be".into(),
            TextEncoding::Legacy(e) => e.name().to_string(),
        }
    }

    /// [`TextEncoding::id`] 的逆操作。
    pub fn from_id(id: &str) -> Option<TextEncoding> {
        match id {
            "utf-8" => Some(TextEncoding::Utf8),
            "utf-16le" => Some(TextEncoding::Utf16Le),
            "utf-16be" => Some(TextEncoding::Utf16Be),
            other => Encoding::for_label(other.as_bytes()).map(|e| {
                if e == encoding_rs::UTF_8 {
                    TextEncoding::Utf8
                } else {
                    TextEncoding::Legacy(e)
                }
            }),
        }
    }

    pub fn supports_bom(self) -> bool {
        !matches!(self, TextEncoding::Legacy(_))
    }

    fn bom(self) -> &'static [u8] {
        match self {
            TextEncoding::Utf8 => UTF8_BOM,
            TextEncoding::Utf16Le => UTF16LE_BOM,
            TextEncoding::Utf16Be => UTF16BE_BOM,
            TextEncoding::Legacy(_) => &[],
        }
    }
}

fn pretty_name(e: &'static Encoding) -> String {
    let n = e.name();
    if let Some(rest) = n.strip_prefix("windows-") {
        format!("Windows-{rest}")
    } else if n == "gb18030" {
        "GB18030".into()
    } else {
        n.to_string()
    }
}

/// 编码 + 是否带 BOM。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingInfo {
    pub encoding: TextEncoding,
    pub bom: bool,
}

impl EncodingInfo {
    pub const UTF8: EncodingInfo = EncodingInfo {
        encoding: TextEncoding::Utf8,
        bom: false,
    };

    pub fn new(encoding: TextEncoding, bom: bool) -> Self {
        EncodingInfo {
            encoding,
            bom: bom && encoding.supports_bom(),
        }
    }

    /// 状态栏显示用，例如 "UTF-8"、"UTF-8 BOM"、"GBK"。
    pub fn label(&self) -> String {
        if self.bom {
            format!("{} BOM", self.encoding.name())
        } else {
            self.encoding.name()
        }
    }
}

impl Default for EncodingInfo {
    fn default() -> Self {
        EncodingInfo::UTF8
    }
}

/// 嗅探时参考的系统信息。
#[derive(Debug, Clone, Copy)]
pub struct DetectHints {
    /// 系统 ANSI 代码页对应的编码（统计推断失败时的兜底）。
    pub ansi: &'static Encoding,
    /// 给 chardetng 的顶级域名提示，例如 `b"cn"`。
    pub tld: Option<&'static [u8]>,
}

impl Default for DetectHints {
    fn default() -> Self {
        DetectHints {
            ansi: encoding_rs::WINDOWS_1252,
            tld: None,
        }
    }
}

impl DetectHints {
    /// 根据 Windows ANSI 代码页构造提示。
    pub fn for_codepage(cp: u32) -> Self {
        DetectHints {
            ansi: encoding_for_codepage(cp).unwrap_or(encoding_rs::WINDOWS_1252),
            tld: tld_for_codepage(cp),
        }
    }
}

/// 嗅探依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detection {
    Bom,
    Utf16Heuristic,
    Binary,
    Ascii,
    Utf8,
    Statistical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub info: EncodingInfo,
    pub how: Detection,
}

/// 嗅探字节序列的编码。
pub fn detect(bytes: &[u8], hints: &DetectHints) -> Detected {
    let bom = |encoding| Detected {
        info: EncodingInfo {
            encoding,
            bom: true,
        },
        how: Detection::Bom,
    };
    if bytes.starts_with(UTF8_BOM) {
        return bom(TextEncoding::Utf8);
    }
    if bytes.starts_with(UTF16LE_BOM) {
        return bom(TextEncoding::Utf16Le);
    }
    if bytes.starts_with(UTF16BE_BOM) {
        return bom(TextEncoding::Utf16Be);
    }

    let sample = &bytes[..bytes.len().min(SNIFF_LEN)];
    let plain = |encoding, how| Detected {
        info: EncodingInfo {
            encoding,
            bom: false,
        },
        how,
    };

    if let Some(e) = sniff_utf16(sample) {
        return plain(e, Detection::Utf16Heuristic);
    }
    if sample.contains(&0) {
        return plain(
            TextEncoding::Legacy(encoding_rs::WINDOWS_1252),
            Detection::Binary,
        );
    }
    if bytes.is_ascii() {
        return plain(TextEncoding::Utf8, Detection::Ascii);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return plain(TextEncoding::Utf8, Detection::Utf8);
    }

    let mut det = chardetng::EncodingDetector::new();
    det.feed(&bytes[..bytes.len().min(CHARDET_LEN)], true);
    let guess = det.guess(hints.tld, false);
    let enc = if guess == encoding_rs::UTF_8 {
        hints.ansi
    } else {
        guess
    };
    plain(TextEncoding::Legacy(enc), Detection::Statistical)
}

/// UTF-16 无 BOM 启发式：ASCII 字符的高字节为 0，统计偶数/奇数位置上 NUL 的比例。
fn sniff_utf16(sample: &[u8]) -> Option<TextEncoding> {
    let n = sample.len() & !1;
    if n < 4 {
        return None;
    }
    let (mut even0, mut odd0) = (0usize, 0usize);
    for pair in sample[..n].chunks_exact(2) {
        if pair[0] == 0 {
            even0 += 1;
        }
        if pair[1] == 0 {
            odd0 += 1;
        }
    }
    let pairs = n / 2;
    // NUL 明显偏向一侧（至少 5% 的码元，且另一侧不到其四分之一），再用解码校验排除二进制。
    let (cand, enc) = if odd0 * 20 >= pairs && even0 * 4 <= odd0 {
        (TextEncoding::Utf16Le, encoding_rs::UTF_16LE)
    } else if even0 * 20 >= pairs && odd0 * 4 <= even0 {
        (TextEncoding::Utf16Be, encoding_rs::UTF_16BE)
    } else {
        return None;
    };
    let valid = |len: usize| !enc.decode_without_bom_handling(&sample[..len]).1;
    // 采样末尾可能截断代理对，允许去掉最后一个码元再试
    if valid(n) || (n >= 4 && valid(n - 2)) {
        Some(cand)
    } else {
        None
    }
}

/// 解码结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    pub info: EncodingInfo,
    /// 存在无法解码的字节（已替换为 U+FFFD）。按原编码保存会损坏这些字节。
    pub lossy: bool,
    /// 按二进制文件处理。
    pub binary: bool,
}

/// 自动嗅探并解码。
pub fn decode_auto(bytes: &[u8], hints: &DetectHints) -> Decoded {
    let d = detect(bytes, hints);
    let mut out = decode_as(bytes, d.info.encoding);
    out.binary = d.how == Detection::Binary;
    out
}

/// 按指定编码解码；BOM 存在则去掉并记录。
pub fn decode_as(bytes: &[u8], encoding: TextEncoding) -> Decoded {
    let bom = encoding.bom();
    let has_bom = !bom.is_empty() && bytes.starts_with(bom);
    let body = if has_bom { &bytes[bom.len()..] } else { bytes };

    let (text, lossy) = match encoding {
        TextEncoding::Utf8 => match std::str::from_utf8(body) {
            Ok(s) => (s.to_owned(), false),
            Err(_) => {
                let (cow, err) = encoding_rs::UTF_8.decode_without_bom_handling(body);
                (cow.into_owned(), err)
            }
        },
        TextEncoding::Utf16Le => {
            let (cow, err) = encoding_rs::UTF_16LE.decode_without_bom_handling(body);
            (cow.into_owned(), err)
        }
        TextEncoding::Utf16Be => {
            let (cow, err) = encoding_rs::UTF_16BE.decode_without_bom_handling(body);
            (cow.into_owned(), err)
        }
        TextEncoding::Legacy(e) => {
            let (cow, err) = e.decode_without_bom_handling(body);
            (cow.into_owned(), err)
        }
    };
    Decoded {
        text,
        info: EncodingInfo {
            encoding,
            bom: has_bom,
        },
        lossy,
        binary: false,
    }
}

/// 编码结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    /// 目标编码无法表示、已替换为 `?` 的字符数。
    pub unmappable: usize,
}

/// 把 UTF-8 文本编码为磁盘字节。
pub fn encode(text: &str, info: EncodingInfo) -> Encoded {
    let mut bytes = Vec::with_capacity(text.len() + 4);
    if info.bom {
        bytes.extend_from_slice(info.encoding.bom());
    }
    let mut unmappable = 0;
    match info.encoding {
        TextEncoding::Utf8 => bytes.extend_from_slice(text.as_bytes()),
        TextEncoding::Utf16Le => {
            for u in text.encode_utf16() {
                bytes.extend_from_slice(&u.to_le_bytes());
            }
        }
        TextEncoding::Utf16Be => {
            for u in text.encode_utf16() {
                bytes.extend_from_slice(&u.to_be_bytes());
            }
        }
        TextEncoding::Legacy(e) => {
            let mut encoder = e.new_encoder();
            let mut src = text;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let (res, read, written) =
                    encoder.encode_from_utf8_without_replacement(src, &mut buf, true);
                bytes.extend_from_slice(&buf[..written]);
                src = &src[read..];
                match res {
                    encoding_rs::EncoderResult::InputEmpty => break,
                    encoding_rs::EncoderResult::OutputFull => {}
                    encoding_rs::EncoderResult::Unmappable(_) => {
                        bytes.push(b'?');
                        unmappable += 1;
                    }
                }
            }
        }
    }
    Encoded { bytes, unmappable }
}

/// 菜单中列出的编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingChoice {
    pub label: &'static str,
    pub info: EncodingInfo,
}

/// "重新打开为"/"转换为"菜单的编码列表。
pub fn catalog() -> Vec<EncodingChoice> {
    use encoding_rs as e;
    let legacy = |label, enc| EncodingChoice {
        label,
        info: EncodingInfo::new(TextEncoding::Legacy(enc), false),
    };
    vec![
        EncodingChoice {
            label: "UTF-8",
            info: EncodingInfo::new(TextEncoding::Utf8, false),
        },
        EncodingChoice {
            label: "UTF-8 BOM",
            info: EncodingInfo::new(TextEncoding::Utf8, true),
        },
        EncodingChoice {
            label: "UTF-16 LE BOM",
            info: EncodingInfo::new(TextEncoding::Utf16Le, true),
        },
        EncodingChoice {
            label: "UTF-16 BE BOM",
            info: EncodingInfo::new(TextEncoding::Utf16Be, true),
        },
        EncodingChoice {
            label: "UTF-16 LE（无 BOM）",
            info: EncodingInfo::new(TextEncoding::Utf16Le, false),
        },
        legacy("GB18030（简体中文）", e::GB18030),
        legacy("GBK（简体中文）", e::GBK),
        legacy("Big5（繁体中文）", e::BIG5),
        legacy("Shift_JIS（日文）", e::SHIFT_JIS),
        legacy("EUC-JP（日文）", e::EUC_JP),
        legacy("EUC-KR（韩文）", e::EUC_KR),
        legacy("Windows-1252（西欧）", e::WINDOWS_1252),
        legacy("Windows-1250（中欧）", e::WINDOWS_1250),
        legacy("Windows-1251（西里尔）", e::WINDOWS_1251),
        legacy("KOI8-R（俄文）", e::KOI8_R),
        legacy("Windows-1253（希腊）", e::WINDOWS_1253),
        legacy("Windows-1254（土耳其）", e::WINDOWS_1254),
        legacy("Windows-1255（希伯来）", e::WINDOWS_1255),
        legacy("Windows-1256（阿拉伯）", e::WINDOWS_1256),
        legacy("Windows-874（泰文）", e::WINDOWS_874),
        legacy("Windows-1258（越南）", e::WINDOWS_1258),
    ]
}

/// Windows 代码页 → encoding_rs 编码。
pub fn encoding_for_codepage(cp: u32) -> Option<&'static Encoding> {
    use encoding_rs as e;
    Some(match cp {
        65001 => e::UTF_8,
        936 => e::GBK,
        54936 => e::GB18030,
        950 => e::BIG5,
        932 => e::SHIFT_JIS,
        20932 | 51932 => e::EUC_JP,
        949 | 51949 => e::EUC_KR,
        874 => e::WINDOWS_874,
        1250 => e::WINDOWS_1250,
        1251 => e::WINDOWS_1251,
        1252 | 28591 => e::WINDOWS_1252,
        1253 => e::WINDOWS_1253,
        1254 => e::WINDOWS_1254,
        1255 => e::WINDOWS_1255,
        1256 => e::WINDOWS_1256,
        1257 => e::WINDOWS_1257,
        1258 => e::WINDOWS_1258,
        866 => e::IBM866,
        20866 => e::KOI8_R,
        21866 => e::KOI8_U,
        _ => return None,
    })
}

/// 代码页 → chardetng 的顶级域名提示。
pub fn tld_for_codepage(cp: u32) -> Option<&'static [u8]> {
    Some(match cp {
        936 | 54936 => b"cn",
        950 => b"tw",
        932 | 20932 | 51932 => b"jp",
        949 | 51949 => b"kr",
        1251 | 866 | 20866 => b"ru",
        _ => return None,
    })
}

/// 最长一行的字节数（用于超长行降级判断）。
pub fn max_line_len(text: &[u8]) -> usize {
    let mut max = 0;
    let mut start = 0;
    for (i, &b) in text.iter().enumerate() {
        if b == b'\n' || b == b'\r' {
            max = max.max(i - start);
            start = i + 1;
        }
    }
    max.max(text.len() - start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(name: &str) -> Vec<u8> {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus/");
        std::fs::read(format!("{p}{name}")).unwrap_or_else(|e| panic!("读取语料 {name} 失败: {e}"))
    }

    fn cn() -> DetectHints {
        DetectHints::for_codepage(936)
    }

    #[test]
    fn detects_boms() {
        assert_eq!(
            detect(&corpus("utf8-bom.txt"), &cn()).info,
            EncodingInfo::new(TextEncoding::Utf8, true)
        );
        assert_eq!(
            detect(&corpus("utf16le-bom.txt"), &cn()).info,
            EncodingInfo::new(TextEncoding::Utf16Le, true)
        );
        assert_eq!(
            detect(&corpus("utf16be-bom.txt"), &cn()).info,
            EncodingInfo::new(TextEncoding::Utf16Be, true)
        );
    }

    #[test]
    fn detects_utf16_without_bom() {
        let d = detect(&corpus("utf16le-nobom.txt"), &cn());
        assert_eq!(d.info, EncodingInfo::new(TextEncoding::Utf16Le, false));
        assert_eq!(d.how, Detection::Utf16Heuristic);
    }

    #[test]
    fn detects_utf8_and_ascii() {
        assert_eq!(detect(&corpus("ascii.txt"), &cn()).how, Detection::Ascii);
        let d = detect(&corpus("utf8.txt"), &cn());
        assert_eq!(d.info, EncodingInfo::UTF8);
        assert_eq!(d.how, Detection::Utf8);
        assert_eq!(detect(b"", &cn()).info, EncodingInfo::UTF8);
    }

    #[test]
    fn detects_gbk() {
        let d = detect(&corpus("gbk.txt"), &cn());
        assert_eq!(d.how, Detection::Statistical);
        assert!(
            matches!(d.info.encoding, TextEncoding::Legacy(e) if e == encoding_rs::GBK || e == encoding_rs::GB18030),
            "{:?}",
            d.info
        );
    }

    #[test]
    fn short_gbk_with_chinese_hint() {
        let d = detect(&corpus("gbk-short.txt"), &cn());
        assert!(
            matches!(d.info.encoding, TextEncoding::Legacy(e) if e == encoding_rs::GBK || e == encoding_rs::GB18030),
            "{:?}",
            d.info
        );
    }

    #[test]
    fn binary_is_lossless() {
        let bytes = corpus("binary.bin");
        let d = decode_auto(&bytes, &cn());
        assert!(d.binary);
        assert!(!d.lossy);
        assert_eq!(encode(&d.text, d.info).bytes, bytes);
    }

    #[test]
    fn roundtrip_all_text_corpus() {
        for name in [
            "ascii.txt",
            "utf8.txt",
            "utf8-bom.txt",
            "crlf.txt",
            "mixed-eol.txt",
            "gbk.txt",
            "utf16le-bom.txt",
            "utf16be-bom.txt",
            "utf16le-nobom.txt",
            "empty.txt",
            "long-line.txt",
        ] {
            let bytes = corpus(name);
            let d = decode_auto(&bytes, &cn());
            assert!(!d.lossy, "{name} lossy");
            let e = encode(&d.text, d.info);
            assert_eq!(e.unmappable, 0, "{name}");
            assert_eq!(e.bytes, bytes, "{name} 往返不一致（{:?}）", d.info);
        }
    }

    #[test]
    fn gb18030_four_byte_roundtrip() {
        let bytes = corpus("gb18030-4byte.txt");
        let d = decode_as(&bytes, TextEncoding::Legacy(encoding_rs::GB18030));
        assert!(!d.lossy);
        assert!(d.text.contains('\u{20000}'));
        assert_eq!(encode(&d.text, d.info).bytes, bytes);
    }

    #[test]
    fn unmappable_counted() {
        let info = EncodingInfo::new(TextEncoding::Legacy(encoding_rs::GBK), false);
        let e = encode("中文😀a", info);
        assert_eq!(e.unmappable, 1);
        assert!(e.bytes.ends_with(b"?a"));
    }

    #[test]
    fn lossy_utf8_flagged() {
        let d = decode_as(&[b'a', 0xFF, b'b'], TextEncoding::Utf8);
        assert!(d.lossy);
        assert_eq!(d.text, "a\u{FFFD}b");
    }

    #[test]
    fn reopen_keeps_bom_flag() {
        let d = decode_as(&corpus("utf8-bom.txt"), TextEncoding::Utf8);
        assert!(d.info.bom);
        assert!(!d.text.starts_with('\u{FEFF}'));
    }

    #[test]
    fn labels() {
        assert_eq!(
            EncodingInfo::new(TextEncoding::Utf8, true).label(),
            "UTF-8 BOM"
        );
        assert_eq!(
            EncodingInfo::new(TextEncoding::Legacy(encoding_rs::WINDOWS_1252), true).label(),
            "Windows-1252"
        );
        assert_eq!(
            EncodingInfo::new(TextEncoding::Legacy(encoding_rs::GB18030), false).label(),
            "GB18030"
        );
    }

    #[test]
    fn encoding_ids_roundtrip() {
        for c in catalog() {
            let id = c.info.encoding.id();
            assert_eq!(TextEncoding::from_id(&id), Some(c.info.encoding), "{id}");
        }
        assert_eq!(TextEncoding::from_id("no-such"), None);
    }

    #[test]
    fn max_line() {
        assert_eq!(max_line_len(b""), 0);
        assert_eq!(max_line_len(b"ab\r\nabcd\nx"), 4);
        assert_eq!(max_line_len(&corpus("long-line.txt")), 200_000);
    }
}
