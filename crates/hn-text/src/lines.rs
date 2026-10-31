//! 行操作：排序、去重、清除空白、删除空行。
//!
//! 都是纯函数：输入一段文本（若干完整行），输出处理后的文本。换行符统一为调用方给定的 `eol`。

use crate::eol::Eol;

/// 按任意换行符切分。文本以换行符结尾时不产生末尾的空行。
pub fn split(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(['\r', '\n']) {
        lines.push(&rest[..i]);
        let skip = if rest[i..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[i + skip..];
    }
    if !rest.is_empty() {
        lines.push(rest);
    }
    lines
}

/// 用 `eol` 连接各行；`trailing` 为真时末尾也加换行符。
pub fn join<S: AsRef<str>>(lines: &[S], eol: Eol, trailing: bool) -> String {
    let sep = std::str::from_utf8(eol.as_bytes()).unwrap();
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str(sep);
        }
        out.push_str(l.as_ref());
    }
    if trailing && !lines.is_empty() {
        out.push_str(sep);
    }
    out
}

fn ends_with_eol(text: &str) -> bool {
    text.ends_with('\n') || text.ends_with('\r')
}

/// 对一段文本按行处理，保持"末尾是否有换行符"不变。
fn map_lines(text: &str, eol: Eol, f: impl FnOnce(Vec<&str>) -> Vec<String>) -> String {
    let out = f(split(text));
    join(&out, eol, ends_with_eol(text))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SortOptions {
    pub descending: bool,
    pub case_insensitive: bool,
    /// 按行首数值排序；没有数值的行排在最后并保持原顺序。
    pub numeric: bool,
}

/// 行首的数值（允许前导空白、正负号、小数、科学计数法）。
fn leading_number(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    if end < b.len() && (b[end] == b'+' || b[end] == b'-') {
        end += 1;
    }
    let digits_start = end;
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
    }
    if end < b.len() && b[end] == b'.' {
        end += 1;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
    }
    if end == digits_start || (end == digits_start + 1 && b[digits_start] == b'.') {
        return None;
    }
    // 科学计数法
    if end < b.len() && (b[end] == b'e' || b[end] == b'E') {
        let mut e = end + 1;
        if e < b.len() && (b[e] == b'+' || b[e] == b'-') {
            e += 1;
        }
        let ds = e;
        while e < b.len() && b[e].is_ascii_digit() {
            e += 1;
        }
        if e > ds {
            end = e;
        }
    }
    t[..end].parse().ok()
}

/// 排序（稳定）。
pub fn sort(text: &str, eol: Eol, opt: SortOptions) -> String {
    map_lines(text, eol, |lines| {
        let mut v: Vec<&str> = lines;
        if opt.numeric {
            let (mut num, rest): (Vec<(f64, &str)>, Vec<&str>) = {
                let mut num = Vec::new();
                let mut rest = Vec::new();
                for l in v {
                    match leading_number(l) {
                        Some(n) if n.is_finite() => num.push((n, l)),
                        _ => rest.push(l),
                    }
                }
                (num, rest)
            };
            num.sort_by(|a, b| {
                let o = a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal);
                if opt.descending {
                    o.reverse()
                } else {
                    o
                }
            });
            num.into_iter()
                .map(|(_, l)| l.to_string())
                .chain(rest.into_iter().map(str::to_string))
                .collect()
        } else {
            if opt.case_insensitive {
                v.sort_by_cached_key(|l| l.to_lowercase());
            } else {
                v.sort();
            }
            if opt.descending {
                v.reverse();
            }
            v.into_iter().map(str::to_string).collect()
        }
    })
}

/// 去重，保留第一次出现的行。`adjacent_only` 为真时只去掉相邻的重复行。
pub fn unique(text: &str, eol: Eol, adjacent_only: bool) -> String {
    map_lines(text, eol, |lines| {
        let mut out: Vec<String> = Vec::with_capacity(lines.len());
        if adjacent_only {
            for l in lines {
                if out.last().map(String::as_str) != Some(l) {
                    out.push(l.to_string());
                }
            }
        } else {
            let mut seen = std::collections::HashSet::new();
            for l in lines {
                if seen.insert(l) {
                    out.push(l.to_string());
                }
            }
        }
        out
    })
}

/// 删除行尾空白。
pub fn trim_trailing(text: &str, eol: Eol) -> String {
    map_lines(text, eol, |lines| {
        lines
            .into_iter()
            .map(|l| l.trim_end().to_string())
            .collect()
    })
}

/// 删除行首空白。
pub fn trim_leading(text: &str, eol: Eol) -> String {
    map_lines(text, eol, |lines| {
        lines
            .into_iter()
            .map(|l| l.trim_start().to_string())
            .collect()
    })
}

/// 删除空行。`blank_counts` 为真时只含空白字符的行也算空行。
pub fn remove_empty(text: &str, eol: Eol, blank_counts: bool) -> String {
    map_lines(text, eol, |lines| {
        lines
            .into_iter()
            .filter(|l| {
                if blank_counts {
                    !l.trim().is_empty()
                } else {
                    !l.is_empty()
                }
            })
            .map(str::to_string)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LF: Eol = Eol::Lf;

    #[test]
    fn split_and_join() {
        assert_eq!(split("a\r\nb\nc"), vec!["a", "b", "c"]);
        assert_eq!(split("a\n"), vec!["a"]);
        assert_eq!(split("a\n\nb"), vec!["a", "", "b"]);
        assert!(split("").is_empty());
        assert_eq!(join(&["a", "b"], Eol::Crlf, true), "a\r\nb\r\n");
    }

    #[test]
    fn sort_basic() {
        assert_eq!(sort("b\na\nC\n", LF, SortOptions::default()), "C\na\nb\n");
        let ci = SortOptions {
            case_insensitive: true,
            ..Default::default()
        };
        assert_eq!(sort("b\na\nC", LF, ci), "a\nb\nC");
        let desc = SortOptions {
            descending: true,
            ..Default::default()
        };
        assert_eq!(sort("a\nc\nb", LF, desc), "c\nb\na");
    }

    #[test]
    fn sort_numeric() {
        let n = SortOptions {
            numeric: true,
            ..Default::default()
        };
        assert_eq!(sort("10\n9\nx\n-1.5\n2e1", LF, n), "-1.5\n9\n10\n2e1\nx");
        let nd = SortOptions {
            numeric: true,
            descending: true,
            ..Default::default()
        };
        assert_eq!(sort("1\nfoo\n3\n2", LF, nd), "3\n2\n1\nfoo");
    }

    #[test]
    fn sort_is_stable_for_equal_numbers() {
        let n = SortOptions {
            numeric: true,
            ..Default::default()
        };
        assert_eq!(sort("2 b\n1\n2 a", LF, n), "1\n2 b\n2 a");
    }

    #[test]
    fn leading_numbers() {
        assert_eq!(leading_number("  42abc"), Some(42.0));
        assert_eq!(leading_number("-3.5"), Some(-3.5));
        assert_eq!(leading_number("1e3x"), Some(1000.0));
        assert_eq!(leading_number("1e"), Some(1.0));
        assert_eq!(leading_number("."), None);
        assert_eq!(leading_number("abc"), None);
        assert_eq!(leading_number("-"), None);
    }

    #[test]
    fn unique_lines() {
        assert_eq!(unique("a\nb\na\nb\nc\n", LF, false), "a\nb\nc\n");
        assert_eq!(unique("a\na\nb\na", LF, true), "a\nb\na");
    }

    #[test]
    fn trim_and_remove() {
        assert_eq!(trim_trailing("a  \n b\t\n", LF), "a\n b\n");
        assert_eq!(trim_leading("  a\n\tb", LF), "a\nb");
        assert_eq!(remove_empty("a\n\n  \nb\n", LF, false), "a\n  \nb\n");
        assert_eq!(remove_empty("a\n\n  \nb\n", LF, true), "a\nb\n");
    }

    #[test]
    fn eol_normalized_to_target() {
        assert_eq!(
            sort("b\r\na\r\n", Eol::Crlf, SortOptions::default()),
            "a\r\nb\r\n"
        );
    }
}
