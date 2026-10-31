//! 编辑操作：行操作、书签、列表续写与自动缩进、括号匹配。
//!
//! 文本处理交给 hn-text 的纯函数，这里只负责从 Scintilla 取范围、写回、组成一个撤销步骤。

use hn_sci::theme::MARK_BOOKMARK;
use hn_sci::*;
use hn_text::eol::Eol;
use hn_text::lines::{self, SortOptions};
use hn_text::listcont::{self, ListAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOp {
    Sort(SortOptions),
    Unique { adjacent_only: bool },
    RemoveEmpty { blank_counts: bool },
    TrimTrailing,
    TrimLeading,
}

pub fn eol_of(sci: &Sci) -> Eol {
    match sci.send(SCI_GETEOLMODE, 0, 0) {
        x if x == SC_EOL_LF => Eol::Lf,
        x if x == SC_EOL_CR => Eol::Cr,
        _ => Eol::Crlf,
    }
}

/// 操作范围：有选区时为选区覆盖的整行，否则为全文。返回 (起始行, 结束行)。
fn line_range(sci: &Sci) -> (usize, usize) {
    let (s, e) = (sci.selection_start(), sci.selection_end());
    if s == e {
        return (0, sci.line_count().saturating_sub(1));
    }
    let first = sci.line_from_pos(s);
    let mut last = sci.line_from_pos(e);
    // 选区结束于行首时不包含该行
    if last > first && sci.line_start(last) == e {
        last -= 1;
    }
    (first, last)
}

/// 对选中的行（或全文）执行行操作。
pub fn apply_line_op(sci: &Sci, op: LineOp) {
    if sci.is_read_only() {
        return;
    }
    let (first, last) = line_range(sci);
    let start = sci.line_start(first);
    let end = sci.line_end(last);
    let text = String::from_utf8_lossy(&sci.text_range(start, end)).into_owned();
    let eol = eol_of(sci);
    let out = match op {
        LineOp::Sort(o) => lines::sort(&text, eol, o),
        LineOp::Unique { adjacent_only } => lines::unique(&text, eol, adjacent_only),
        LineOp::RemoveEmpty { blank_counts } => lines::remove_empty(&text, eol, blank_counts),
        LineOp::TrimTrailing => lines::trim_trailing(&text, eol),
        LineOp::TrimLeading => lines::trim_leading(&text, eol),
    };
    if out == text {
        return;
    }
    sci.undo_group(|| sci.replace_range(start, end, out.as_bytes()));
    sci.set_sel(start, start + out.len());
}

// ---- 书签 ----

fn has_bookmark(sci: &Sci, line: usize) -> bool {
    sci.send(SCI_MARKERGET, line, 0) & (1 << MARK_BOOKMARK) != 0
}

pub fn toggle_bookmark(sci: &Sci, line: usize) {
    if has_bookmark(sci, line) {
        sci.send(SCI_MARKERDELETE, line, MARK_BOOKMARK as isize);
    } else {
        sci.send(SCI_MARKERADD, line, MARK_BOOKMARK as isize);
    }
}

pub fn toggle_bookmark_at_caret(sci: &Sci) {
    toggle_bookmark(sci, sci.line_from_pos(sci.current_pos()));
}

/// 跳到下一个（或上一个）书签，到头后循环。
pub fn goto_bookmark(sci: &Sci, forward: bool) -> bool {
    let mask = 1isize << MARK_BOOKMARK;
    let cur = sci.line_from_pos(sci.current_pos());
    let found = if forward {
        let n = sci.send(SCI_MARKERNEXT, cur + 1, mask);
        if n >= 0 {
            n
        } else {
            sci.send(SCI_MARKERNEXT, 0, mask)
        }
    } else {
        let p = if cur > 0 {
            sci.send(SCI_MARKERPREVIOUS, cur - 1, mask)
        } else {
            -1
        };
        if p >= 0 {
            p
        } else {
            sci.send(SCI_MARKERPREVIOUS, sci.line_count().saturating_sub(1), mask)
        }
    };
    if found < 0 {
        return false;
    }
    let line = found as usize;
    sci.send(SCI_ENSUREVISIBLEENFORCEPOLICY, line, 0);
    sci.send(SCI_GOTOLINE, line, 0);
    true
}

pub fn clear_bookmarks(sci: &Sci) {
    sci.send(SCI_MARKERDELETEALL, MARK_BOOKMARK, 0);
}

pub fn invert_bookmarks(sci: &Sci) {
    for line in 0..sci.line_count() {
        toggle_bookmark(sci, line);
    }
}

fn bookmarked_lines(sci: &Sci, marked: bool) -> Vec<usize> {
    (0..sci.line_count())
        .filter(|&l| has_bookmark(sci, l) == marked)
        .collect()
}

/// 复制带书签（或不带书签）的行到剪贴板，返回行数。
pub fn copy_bookmarked(sci: &Sci) -> usize {
    let lines = bookmarked_lines(sci, true);
    if lines.is_empty() {
        return 0;
    }
    let eol = std::str::from_utf8(eol_of(sci).as_bytes()).unwrap();
    let mut text = String::new();
    for l in &lines {
        text.push_str(&sci.line_text(*l));
        text.push_str(eol);
    }
    sci.send(SCI_COPYTEXT, text.len(), text.as_ptr() as isize);
    lines.len()
}

/// 删除带书签（`marked`）或不带书签的行，返回删除的行数。
pub fn delete_lines_where_bookmarked(sci: &Sci, marked: bool) -> usize {
    if sci.is_read_only() {
        return 0;
    }
    let lines = bookmarked_lines(sci, marked);
    if lines.is_empty() {
        return 0;
    }
    sci.undo_group(|| {
        for &l in lines.iter().rev() {
            let (start, end) = if l + 1 < sci.line_count() {
                (sci.line_start(l), sci.line_start(l + 1))
            } else if l > 0 {
                // 最后一行：连同上一行的换行符一起删除
                (sci.line_end(l - 1), sci.line_end(l))
            } else {
                (0, sci.line_end(l))
            };
            sci.send(SCI_DELETERANGE, start, (end - start) as isize);
        }
    });
    lines.len()
}

// ---- 列表续写 / 自动缩进 ----

/// 换行字符已插入后调用：续写列表、结束空列表项，或复制上一行的缩进。
pub fn on_newline(sci: &Sci, list_enabled: bool) {
    if sci.send(SCI_GETSELECTIONS, 0, 0) > 1 {
        return;
    }
    let pos = sci.current_pos();
    let line = sci.line_from_pos(pos);
    if line == 0 || sci.line_start(line) != pos {
        return;
    }
    let prev = sci.line_text(line - 1);
    let action = if list_enabled {
        listcont::on_enter(&prev)
    } else {
        ListAction::None
    };
    match action {
        ListAction::Continue(prefix) => {
            sci.send_str(SCI_REPLACESEL, 0, &prefix);
        }
        ListAction::End => {
            // 删除空列表项及刚插入的换行，光标回到该行开头
            let start = sci.line_start(line - 1);
            sci.replace_range(start, pos, b"");
            sci.set_sel(start, start);
        }
        ListAction::None => {
            let indent = listcont::indentation(&prev);
            if !indent.is_empty() {
                sci.send_str(SCI_REPLACESEL, 0, indent);
            }
        }
    }
}

/// SCN_CHARADDED 的字符是否表示"换行完成"（CRLF 模式下只在 '\n' 时处理一次）。
pub fn is_newline_char(sci: &Sci, ch: i32) -> bool {
    match eol_of(sci) {
        Eol::Cr => ch == '\r' as i32,
        _ => ch == '\n' as i32,
    }
}

// ---- 括号匹配 ----

fn is_brace(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'[' | b']' | b'{' | b'}')
}

pub fn update_brace_match(sci: &Sci) {
    let pos = sci.current_pos();
    let len = sci.len();
    let at = |p: usize| sci.send(SCI_GETCHARAT, p, 0) as u8;
    let brace = if pos > 0 && is_brace(at(pos - 1)) {
        Some(pos - 1)
    } else if pos < len && is_brace(at(pos)) {
        Some(pos)
    } else {
        None
    };
    match brace {
        Some(b) => {
            let m = sci.send(SCI_BRACEMATCH, b, 0);
            if m >= 0 {
                sci.send(SCI_BRACEHIGHLIGHT, b, m);
                let col = sci
                    .send(SCI_GETCOLUMN, b, 0)
                    .min(sci.send(SCI_GETCOLUMN, m as usize, 0));
                sci.send(SCI_SETHIGHLIGHTGUIDE, col as usize, 0);
            } else {
                sci.send(SCI_BRACEBADLIGHT, b, 0);
                sci.send(SCI_SETHIGHLIGHTGUIDE, 0, 0);
            }
        }
        None => {
            sci.send(SCI_BRACEHIGHLIGHT, usize::MAX, -1);
            sci.send(SCI_SETHIGHLIGHTGUIDE, 0, 0);
        }
    }
}

/// 跳到第 `line` 行第 `col` 列（均为 1 起始；列按字符计）。
pub fn goto_line_col(sci: &Sci, line: usize, col: Option<usize>) {
    let l = line
        .saturating_sub(1)
        .min(sci.line_count().saturating_sub(1));
    sci.send(SCI_ENSUREVISIBLEENFORCEPOLICY, l, 0);
    let mut pos = sci.line_start(l);
    if let Some(c) = col {
        let end = sci.line_end(l);
        for _ in 1..c {
            if pos >= end {
                break;
            }
            pos = sci.send(SCI_POSITIONAFTER, pos, 0) as usize;
        }
    }
    sci.set_sel(pos, pos);
    sci.send(SCI_SCROLLCARET, 0, 0);
    sci.send(SCI_VERTICALCENTRECARET, 0, 0);
}
