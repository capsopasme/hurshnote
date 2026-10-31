//! 查找替换引擎。
//!
//! 三种模式都编译为 PCRE2 正则（普通/转义模式先转义），因此行为一致：
//! - 普通：按字面匹配
//! - 转义：支持 `\n \r \t \0 \\ \xHH \uHHHH`
//! - 正则：PCRE2 语法；`^ $` 按行匹配，换行符可以是 CRLF / LF / CR
//!
//! 输入文本必须是合法 UTF-8（接口使用 `&str` 保证这一点）。

use std::fmt;
use std::ops::Range;

use pcre2::bytes::{CaptureLocations, Regex, RegexBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Extended,
    Regex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub mode: Mode,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// 正则模式下 `.` 匹配换行符。
    pub dot_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    EmptyPattern,
    Regex(String),
    Search(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::EmptyPattern => write!(f, "查找内容为空"),
            Error::Regex(e) => write!(f, "正则表达式错误：{e}"),
            Error::Search(e) => write!(f, "搜索失败：{e}"),
        }
    }
}

impl std::error::Error for Error {}

/// 一处匹配的字节范围。
pub type Span = (usize, usize);

/// 编译好的查找器。
pub struct Searcher {
    re: Regex,
    mode: Mode,
}

/// 转义正则元字符。
pub fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if "\\^$.|?*+()[]{}#-/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 解析转义序列：`\n \r \t \0 \\ \xHH \uHHHH`；无法识别的序列原样保留。
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.peek().copied() {
            Some('n') => {
                it.next();
                out.push('\n');
            }
            Some('r') => {
                it.next();
                out.push('\r');
            }
            Some('t') => {
                it.next();
                out.push('\t');
            }
            Some('0') => {
                it.next();
                out.push('\0');
            }
            Some('\\') => {
                it.next();
                out.push('\\');
            }
            Some(k @ ('x' | 'u')) => {
                let width = if k == 'x' { 2 } else { 4 };
                let hex: String = it.clone().skip(1).take(width).collect();
                let code = (hex.len() == width)
                    .then(|| u32::from_str_radix(&hex, 16).ok())
                    .flatten()
                    .and_then(char::from_u32);
                match code {
                    Some(ch) => {
                        for _ in 0..=width {
                            it.next();
                        }
                        out.push(ch);
                    }
                    None => out.push('\\'),
                }
            }
            _ => out.push('\\'),
        }
    }
    out
}

impl Searcher {
    pub fn new(pattern: &str, opt: &Options) -> Result<Searcher, Error> {
        if pattern.is_empty() {
            return Err(Error::EmptyPattern);
        }
        let mut body = match opt.mode {
            Mode::Normal => escape_regex(pattern),
            Mode::Extended => escape_regex(&unescape(pattern)),
            Mode::Regex => pattern.to_string(),
        };
        if opt.whole_word {
            body = format!(r"(?<![\w])(?:{body})(?![\w])");
        }
        let mut b = RegexBuilder::new();
        b.caseless(!opt.case_sensitive)
            .multi_line(true)
            .crlf(true)
            .dotall(opt.dot_all && opt.mode == Mode::Regex)
            .utf(true)
            .ucp(true)
            .jit_if_available(true);
        // 安全：接口只接受 &str，主题文本保证是合法 UTF-8，无需 PCRE2 每次重复校验。
        unsafe {
            b.disable_utf_check();
        }
        let re = b.build(&body).map_err(|e| Error::Regex(e.to_string()))?;
        Ok(Searcher { re, mode: opt.mode })
    }

    /// 从 `start` 开始（含）向后找第一处匹配，限定在 `hay[..end]` 内。
    pub fn find_forward(&self, hay: &str, start: usize, end: usize) -> Result<Option<Span>, Error> {
        let end = end.min(hay.len());
        if start > end {
            return Ok(None);
        }
        let sub = &hay.as_bytes()[..end];
        match self.re.find_at(sub, start) {
            Ok(Some(m)) => Ok(Some((m.start(), m.end()))),
            Ok(None) => Ok(None),
            Err(e) => Err(Error::Search(e.to_string())),
        }
    }

    /// 在 `range` 内找最后一处起点小于 `before` 的匹配（向上查找）。
    pub fn find_backward(
        &self,
        hay: &str,
        range: Range<usize>,
        before: usize,
    ) -> Result<Option<Span>, Error> {
        let mut last = None;
        let limit = before.min(range.end);
        for m in self.iter(hay, range.start..limit)? {
            last = Some(m);
        }
        Ok(last)
    }

    /// `range` 内的所有匹配（不重叠）。
    pub fn find_all(&self, hay: &str, range: Range<usize>) -> Result<Vec<Span>, Error> {
        self.iter(hay, range)
    }

    fn iter(&self, hay: &str, range: Range<usize>) -> Result<Vec<Span>, Error> {
        let mut out = Vec::new();
        let end = range.end.min(hay.len());
        let mut pos = range.start;
        while pos <= end {
            let Some((s, e)) = self.find_forward(hay, pos, end)? else {
                break;
            };
            out.push((s, e));
            pos = if e > s {
                e
            } else {
                // 空匹配：前进一个字符
                match hay[e..].chars().next() {
                    Some(c) => e + c.len_utf8(),
                    None => break,
                }
            };
        }
        Ok(out)
    }

    /// 计算替换文本。`template` 的含义随模式而定：
    /// - 普通：字面文本
    /// - 转义：解析转义序列
    /// - 正则：`$0 $1 ${12} ${name} $& \1` 引用分组，`$$` 表示 `$`，并支持 `\n \r \t \\`
    pub fn replacement(&self, hay: &str, span: Span, template: &str) -> Result<String, Error> {
        match self.mode {
            Mode::Normal => Ok(template.to_string()),
            Mode::Extended => Ok(unescape(template)),
            Mode::Regex => {
                let mut locs = self.re.capture_locations();
                let sub = &hay.as_bytes()[..span.1.max(span.0)];
                let found = self
                    .re
                    .captures_read_at(&mut locs, sub, span.0)
                    .map_err(|e| Error::Search(e.to_string()))?;
                if found.is_none() {
                    // 子串上重新匹配失败（例如依赖后文的断言）：退回整段文本
                    self.re
                        .captures_read_at(&mut locs, hay.as_bytes(), span.0)
                        .map_err(|e| Error::Search(e.to_string()))?;
                }
                Ok(expand(template, hay, &locs, self.re.capture_names()))
            }
        }
    }
}

fn group_text<'a>(hay: &'a str, locs: &CaptureLocations, i: usize) -> &'a str {
    locs.get(i).map(|(s, e)| &hay[s..e]).unwrap_or("")
}

fn expand(template: &str, hay: &str, locs: &CaptureLocations, names: &[Option<String>]) -> String {
    let mut out = String::new();
    let t = template.as_bytes();
    let mut i = 0;
    while i < t.len() {
        let c = t[i];
        if c == b'$' && i + 1 < t.len() {
            let n = t[i + 1];
            if n == b'$' {
                out.push('$');
                i += 2;
                continue;
            }
            if n == b'&' {
                out.push_str(group_text(hay, locs, 0));
                i += 2;
                continue;
            }
            if n.is_ascii_digit() {
                // $1 … $99（贪婪取最多两位，超出分组数时只取一位）
                let mut j = i + 1;
                while j < t.len() && j < i + 3 && t[j].is_ascii_digit() {
                    j += 1;
                }
                let mut num: usize = template[i + 1..j].parse().unwrap_or(0);
                if num >= locs.len() && j - i - 1 == 2 {
                    j -= 1;
                    num = template[i + 1..j].parse().unwrap_or(0);
                }
                out.push_str(group_text(hay, locs, num));
                i = j;
                continue;
            }
            if n == b'{' {
                if let Some(close) = template[i + 2..].find('}') {
                    let key = &template[i + 2..i + 2 + close];
                    let idx = key
                        .parse::<usize>()
                        .ok()
                        .or_else(|| names.iter().position(|nm| nm.as_deref() == Some(key)));
                    if let Some(idx) = idx {
                        out.push_str(group_text(hay, locs, idx));
                    }
                    i += 3 + close;
                    continue;
                }
            }
        }
        if c == b'\\' && i + 1 < t.len() {
            let n = t[i + 1];
            match n {
                b'0'..=b'9' => out.push_str(group_text(hay, locs, (n - b'0') as usize)),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'\\' => out.push('\\'),
                b'$' => out.push('$'),
                _ => {
                    out.push('\\');
                    i += 1;
                    continue;
                }
            }
            i += 2;
            continue;
        }
        // 普通字符（按 UTF-8 字符整体复制）
        let ch = template[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(mode: Mode) -> Options {
        Options {
            mode,
            case_sensitive: true,
            ..Default::default()
        }
    }

    fn all(pat: &str, o: Options, hay: &str) -> Vec<Span> {
        Searcher::new(pat, &o)
            .unwrap()
            .find_all(hay, 0..hay.len())
            .unwrap()
    }

    #[test]
    fn literal_with_metachars() {
        assert_eq!(all("a.b(", opt(Mode::Normal), "a.b( axb("), vec![(0, 4)]);
    }

    #[test]
    fn case_insensitive_unicode() {
        let o = Options::default();
        assert_eq!(all("straße", o, "STRASSE Straße").len(), 1);
        assert_eq!(all("abc", o, "ABC abc").len(), 2);
    }

    #[test]
    fn whole_word() {
        let o = Options {
            whole_word: true,
            case_sensitive: true,
            ..Default::default()
        };
        assert_eq!(
            all("cat", o, "cat concat cat_ cat."),
            vec![(0, 3), (16, 19)]
        );
    }

    #[test]
    fn extended_escapes() {
        assert_eq!(all(r"a\r\nb", opt(Mode::Extended), "a\r\nb"), vec![(0, 4)]);
        assert_eq!(unescape(r"\x41\u4e2d\t\\\q"), "A中\t\\\\q");
    }

    #[test]
    fn regex_line_anchors_with_crlf() {
        let hay = "foo\r\nbar\r\nfoo";
        assert_eq!(all("^foo$", opt(Mode::Regex), hay), vec![(0, 3), (10, 13)]);
    }

    #[test]
    fn zero_length_matches_advance() {
        let hay = "a\nb\n中";
        assert_eq!(all("^", opt(Mode::Regex), hay).len(), 3);
        assert_eq!(all("x*", opt(Mode::Regex), "中文").len(), 3);
    }

    #[test]
    fn backward() {
        let s = Searcher::new("ab", &opt(Mode::Normal)).unwrap();
        let hay = "ab ab ab";
        assert_eq!(s.find_backward(hay, 0..hay.len(), 6).unwrap(), Some((3, 5)));
        assert_eq!(s.find_backward(hay, 0..hay.len(), 0).unwrap(), None);
    }

    #[test]
    fn forward_respects_end() {
        let s = Searcher::new("ab", &opt(Mode::Normal)).unwrap();
        assert_eq!(s.find_forward("xxab", 0, 3).unwrap(), None);
        assert_eq!(s.find_forward("xxab", 0, 4).unwrap(), Some((2, 4)));
    }

    #[test]
    fn regex_replacement() {
        let s = Searcher::new(r"(\w+)@(?<host>\w+)", &opt(Mode::Regex)).unwrap();
        let hay = "mail: bob@example";
        let m = s.find_forward(hay, 0, hay.len()).unwrap().unwrap();
        assert_eq!(
            s.replacement(hay, m, "${host}:$1 [$&] $$ \\1\\n").unwrap(),
            "example:bob [bob@example] $ bob\n"
        );
    }

    #[test]
    fn replacement_modes() {
        let n = Searcher::new("a", &opt(Mode::Normal)).unwrap();
        assert_eq!(n.replacement("a", (0, 1), r"$1\n").unwrap(), r"$1\n");
        let e = Searcher::new("a", &opt(Mode::Extended)).unwrap();
        assert_eq!(e.replacement("a", (0, 1), r"x\ty").unwrap(), "x\ty");
    }

    #[test]
    fn errors() {
        assert_eq!(
            Searcher::new("", &Options::default()).err(),
            Some(Error::EmptyPattern)
        );
        assert!(matches!(
            Searcher::new("(", &opt(Mode::Regex)),
            Err(Error::Regex(_))
        ));
    }

    #[test]
    fn chinese_offsets_are_bytes() {
        assert_eq!(all("文", opt(Mode::Normal), "中文"), vec![(3, 6)]);
    }
}
