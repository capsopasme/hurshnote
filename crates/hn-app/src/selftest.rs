//! `--selftest <输出文件>`：在真实窗口中做基本检查，结果写入文件后退出。
//!
//! CI 用它验证 exe 能启动、Scintilla 与 Lexilla 静态链接正常、主要编辑功能可用，并测量启动耗时。

use std::fmt::Write as _;

use hn_sci::theme::MARK_BOOKMARK;
use hn_sci::*;
use hn_search::{Mode, Options};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use crate::app::App;
use crate::commands::*;
use crate::finddlg::{FindAction, FindState};

fn ft(t: &FILETIME) -> u64 {
    ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
}

/// 从进程创建到现在的毫秒数（包含加载器时间）。
pub fn process_age_ms() -> f64 {
    unsafe {
        let mut created: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exit,
            &mut kernel,
            &mut user,
        ) == 0
        {
            return -1.0;
        }
        let mut now: FILETIME = std::mem::zeroed();
        GetSystemTimePreciseAsFileTime(&mut now);
        ft(&now).saturating_sub(ft(&created)) as f64 / 10_000.0
    }
}

fn set_text(sci: &Sci, text: &str) {
    sci.load_document(text.as_bytes());
    sci.send(SCI_SETEOLMODE, SC_EOL_CRLF as usize, 0);
}

fn text(sci: &Sci) -> String {
    String::from_utf8_lossy(&sci.bytes()).into_owned()
}

/// 执行检查，返回 `key=value` 格式的报告。最后一行 `result=pass|fail`。
pub fn run(app: &App, startup_ms: f64) -> String {
    let mut out = String::new();
    writeln!(out, "startup_ms={startup_ms:.1}").unwrap();
    let Some(sci) = app.st_sci() else {
        out.push_str("sci_ok=0\nresult=fail\n");
        return out;
    };
    let mut ok = true;
    let mut check = |name: &str, pass: bool, detail: &str, out: &mut String| {
        if pass {
            writeln!(out, "{name}=1").unwrap();
        } else {
            writeln!(out, "{name}=0  # {detail}").unwrap();
        }
        ok &= pass;
    };

    // 1. 中文文本往返与修改状态
    let sample = "测试 HurshNote 😀\r\n第二行";
    set_text(&sci, sample);
    check("sci_roundtrip", text(&sci) == sample, "", &mut out);
    check(
        "sci_not_modified_after_load",
        !sci.is_modified(),
        "",
        &mut out,
    );
    sci.send(SCI_APPENDTEXT, 1, b"x".as_ptr() as isize);
    check("sci_modified_after_edit", sci.is_modified(), "", &mut out);
    check(
        "sci_can_undo",
        sci.send(SCI_CANUNDO, 0, 0) != 0,
        "",
        &mut out,
    );
    check(
        "sci_directwrite",
        sci.send(SCI_GETTECHNOLOGY, 0, 0) == SC_TECHNOLOGY_DIRECTWRITE,
        "",
        &mut out,
    );
    check(
        "sci_utf8",
        sci.send(SCI_GETCODEPAGE, 0, 0) == SC_CP_UTF8,
        "",
        &mut out,
    );

    // 2. Lexilla 与语言切换
    writeln!(out, "lexer_count={}", hn_sci::lexer_count()).unwrap();
    app.st_set_language("python");
    check(
        "lexer_python",
        sci.send(SCI_GETLEXER, 0, 0) == SCLEX_PYTHON && app.st_active_lang() == "python",
        "",
        &mut out,
    );
    app.st_set_language("json");
    check(
        "lexer_json",
        sci.send(SCI_GETLEXER, 0, 0) == SCLEX_JSON,
        "",
        &mut out,
    );
    check(
        "lexer_unknown_rejected",
        !sci.set_lexer("no-such-lexer"),
        "",
        &mut out,
    );
    app.st_set_language("text");

    // 3. 行操作
    set_text(&sci, "b\r\na\r\nc");
    app.st_command(IDM_LINE_SORT_ASC);
    let t = text(&sci);
    check("line_sort", t == "a\r\nb\r\nc", &format!("{t:?}"), &mut out);
    set_text(&sci, "x\r\ny\r\nx\r\n");
    app.st_command(IDM_LINE_UNIQUE);
    let t = text(&sci);
    check(
        "line_unique",
        t == "x\r\ny\r\n",
        &format!("{t:?}"),
        &mut out,
    );

    // 4. 列表续写与自动缩进
    set_text(&sci, "- item");
    let end = sci.len();
    sci.set_sel(end, end);
    sci.send_str(SCI_REPLACESEL, 0, "\r\n");
    crate::editops::on_newline(&sci, true);
    let t = text(&sci);
    check(
        "list_continue",
        t == "- item\r\n- ",
        &format!("{t:?}"),
        &mut out,
    );
    sci.send_str(SCI_REPLACESEL, 0, "\r\n");
    crate::editops::on_newline(&sci, true);
    let t = text(&sci);
    check("list_end", t == "- item\r\n", &format!("{t:?}"), &mut out);
    set_text(&sci, "    indented");
    let end = sci.len();
    sci.set_sel(end, end);
    sci.send_str(SCI_REPLACESEL, 0, "\r\n");
    crate::editops::on_newline(&sci, false);
    let t = text(&sci);
    check(
        "auto_indent",
        t == "    indented\r\n    ",
        &format!("{t:?}"),
        &mut out,
    );

    // 5. 书签
    set_text(&sci, "a\r\nb\r\nc");
    crate::editops::toggle_bookmark(&sci, 1);
    let has = sci.send(SCI_MARKERGET, 1, 0) & (1 << MARK_BOOKMARK) != 0;
    sci.set_sel(0, 0);
    let jumped =
        crate::editops::goto_bookmark(&sci, true) && sci.line_from_pos(sci.current_pos()) == 1;
    check("bookmark", has && jumped, "", &mut out);
    crate::editops::delete_lines_where_bookmarked(&sci, true);
    let t = text(&sci);
    check(
        "bookmark_delete",
        t == "a\r\nc",
        &format!("{t:?}"),
        &mut out,
    );

    // 6. 查找与替换
    set_text(&sci, "foo bar foo");
    sci.set_sel(0, 0);
    let st = FindState {
        pattern: "bar".into(),
        wrap: true,
        ..Default::default()
    };
    app.find_action(FindAction::Next, st);
    check(
        "find_next",
        (sci.selection_start(), sci.selection_end()) == (4, 7),
        "",
        &mut out,
    );
    set_text(&sci, "x1 x2");
    let st = FindState {
        pattern: r"x(\d)".into(),
        replace: "y$1".into(),
        opts: Options {
            mode: Mode::Regex,
            case_sensitive: true,
            ..Default::default()
        },
        wrap: true,
        in_selection: false,
    };
    app.find_action(FindAction::ReplaceAll, st);
    let t = text(&sci);
    check(
        "replace_all_regex",
        t == "y1 y2",
        &format!("{t:?}"),
        &mut out,
    );
    check(
        "replace_all_undo_group",
        {
            sci.send(SCI_UNDO, 0, 0);
            text(&sci) == "x1 x2"
        },
        "",
        &mut out,
    );

    // 7. 编码
    let gbk = hn_text::encoding::encoding_rs::GBK
        .encode("中文测试")
        .0
        .into_owned();
    let d =
        hn_text::encoding::decode_auto(&gbk, &hn_text::encoding::DetectHints::for_codepage(936));
    check(
        "decode_gbk",
        d.text == "中文测试",
        &format!("{:?}", d.info),
        &mut out,
    );

    // 8. 多文档
    sci.set_save_point();
    let before = app.st_doc_count();
    app.new_document();
    let two = app.st_doc_count() == before + 1;
    app.st_close_active();
    check(
        "multi_doc",
        two && app.st_doc_count() == before,
        "",
        &mut out,
    );

    // 9. 缩放
    let z0 = app.st_zoom();
    app.st_command(IDM_VIEW_ZOOM_IN);
    let zoomed = app.st_zoom() > z0;
    app.st_command(IDM_VIEW_ZOOM_RESET);
    check("zoom", zoomed && app.st_zoom() == 100, "", &mut out);

    sci.set_save_point();
    writeln!(out, "result={}", if ok { "pass" } else { "fail" }).unwrap();
    out
}
