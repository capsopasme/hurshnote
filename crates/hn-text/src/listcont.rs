//! 智能列表续写：在列表项末尾按 Enter 时生成下一项的前缀；空列表项再按 Enter 则结束列表。
//!
//! 支持 `- ` `* ` `+ `、任务列表 `- [ ] `、有序列表 `1. ` `1) `、引用 `> `，保留缩进。

/// 按 Enter 后应执行的动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListAction {
    /// 上一行是非空列表项：在新行插入这个前缀。
    Continue(String),
    /// 上一行是空列表项：删除上一行的前缀，结束列表。
    End,
    /// 不是列表项。
    None,
}

/// 列表项前缀解析结果：(完整前缀长度, 下一项前缀)。
fn parse(line: &str) -> Option<(usize, String)> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    let b = rest.as_bytes();

    // 无序列表 / 任务列表
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        let bullet = b[0] as char;
        let after = &rest[2..];
        for task in ["[ ] ", "[x] ", "[X] "] {
            if after.starts_with(task) {
                return Some((indent_len + 2 + 4, format!("{indent}{bullet} [ ] ")));
            }
        }
        return Some((indent_len + 2, format!("{indent}{bullet} ")));
    }

    // 有序列表：最多 9 位数字
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&digits)
        && b.len() >= digits + 2
        && matches!(b[digits], b'.' | b')')
        && b[digits + 1] == b' '
    {
        let n: u64 = rest[..digits].parse().ok()?;
        let delim = b[digits] as char;
        return Some((
            indent_len + digits + 2,
            format!("{indent}{}{delim} ", n + 1),
        ));
    }

    // 引用
    if b.len() >= 2 && b[0] == b'>' && b[1] == b' ' {
        return Some((indent_len + 2, format!("{indent}> ")));
    }
    None
}

/// 根据上一行（按 Enter 之前光标所在行，不含换行符）决定动作。
pub fn on_enter(prev_line: &str) -> ListAction {
    match parse(prev_line) {
        Some((prefix_len, next)) => {
            if prev_line[prefix_len..].trim().is_empty() {
                ListAction::End
            } else {
                ListAction::Continue(next)
            }
        }
        None => ListAction::None,
    }
}

/// 行首缩进（空格与制表符）。
pub fn indentation(line: &str) -> &str {
    let n = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[..n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cont(s: &str) -> ListAction {
        ListAction::Continue(s.to_string())
    }

    #[test]
    fn bullets() {
        assert_eq!(on_enter("- item"), cont("- "));
        assert_eq!(on_enter("* item"), cont("* "));
        assert_eq!(on_enter("  + item"), cont("  + "));
        assert_eq!(on_enter("\t- x"), cont("\t- "));
    }

    #[test]
    fn tasks_reset_checkbox() {
        assert_eq!(on_enter("- [x] done"), cont("- [ ] "));
        assert_eq!(on_enter("- [ ] todo"), cont("- [ ] "));
        assert_eq!(on_enter("- [ ] "), ListAction::End);
    }

    #[test]
    fn ordered() {
        assert_eq!(on_enter("1. one"), cont("2. "));
        assert_eq!(on_enter("  9) nine"), cont("  10) "));
        assert_eq!(on_enter("1. "), ListAction::End);
    }

    #[test]
    fn empty_item_ends() {
        assert_eq!(on_enter("- "), ListAction::End);
        assert_eq!(on_enter("  -   "), ListAction::End);
        assert_eq!(on_enter("> "), ListAction::End);
    }

    #[test]
    fn not_lists() {
        assert_eq!(on_enter("plain"), ListAction::None);
        assert_eq!(on_enter("-no space"), ListAction::None);
        assert_eq!(on_enter("**bold**"), ListAction::None);
        assert_eq!(on_enter("---"), ListAction::None);
        assert_eq!(on_enter("1.5 is a number"), ListAction::None);
        assert_eq!(on_enter(""), ListAction::None);
        assert_eq!(on_enter("1234567890. too long"), ListAction::None);
    }

    #[test]
    fn quotes() {
        assert_eq!(on_enter("> quoted"), cont("> "));
    }

    #[test]
    fn indent() {
        assert_eq!(indentation("  \tabc"), "  \t");
        assert_eq!(indentation("abc"), "");
    }
}
