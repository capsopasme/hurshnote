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
use crate::lview::{self, Mode as ViewMode};

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

    // 10. 安全保存
    let dir = std::env::temp_dir().join(format!("hn-selftest-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let plain = dir.join("plain.txt");
    app.new_document();
    set_text(&sci, "保存测试\r\n第二行");
    sci.send(SCI_APPENDTEXT, 1, b"!".as_ptr() as isize);
    let saved = app.st_write_active(&plain);
    let on_disk = std::fs::read(&plain).unwrap_or_default();
    check(
        "safe_save",
        saved && on_disk == "保存测试\r\n第二行!".as_bytes() && !sci.is_modified(),
        &format!("saved={saved} {:?}", String::from_utf8_lossy(&on_disk)),
        &mut out,
    );

    // 11. 加密文件：打开 → 编辑 → 保存 → 用密码重新解密
    let enc_path = dir.join("secret.hne");
    let params = hn_crypto::KdfParams {
        m_cost_kib: 1024,
        t_cost: 1,
        p_cost: 1,
    };
    let enc_ok = (|| -> Option<bool> {
        let key = hn_crypto::Key::generate("测试密码", params).ok()?;
        let payload = hn_core::payload::Payload::new("机密第一行\r\n".as_bytes().to_vec())
            .with("eol", "crlf");
        let file = hn_crypto::seal(&key, &payload.encode()).ok()?;
        std::fs::write(&enc_path, &file).ok()?;
        let (k2, pt) = hn_crypto::open_with_password("测试密码", &file).ok()?;
        let p2 = hn_core::payload::Payload::decode(&pt).ok()?;
        let stamp = hn_core::stamp::DiskStamp::new(0, &file);
        if !app.st_install_encrypted(&enc_path, k2, p2, stamp) || !app.st_active_encrypted() {
            return Some(false);
        }
        if text(&sci) != "机密第一行\r\n" {
            return Some(false);
        }
        let end = sci.len();
        sci.set_sel(end, end);
        sci.send_str(SCI_REPLACESEL, 0, "追加");
        if !app.st_write_active(&enc_path) {
            return Some(false);
        }
        let written = std::fs::read(&enc_path).ok()?;
        if written.windows(6).any(|w| w == "机密".as_bytes()) {
            return Some(false); // 明文泄漏
        }
        let (_, pt3) = hn_crypto::open_with_password("测试密码", &written).ok()?;
        let p3 = hn_core::payload::Payload::decode(&pt3).ok()?;
        Some(p3.text == "机密第一行\r\n追加".as_bytes())
    })()
    .unwrap_or(false);
    check("encrypted_roundtrip", enc_ok, "", &mut out);
    app.st_close_active();
    app.st_close_active();

    // 12. 工作区目录树（遵守 .gitignore、目录在前）与快速打开
    let ws = dir.join("ws");
    let _ = std::fs::create_dir_all(ws.join("sub"));
    let _ = std::fs::write(ws.join("alpha.txt"), "a");
    let _ = std::fs::write(ws.join("sub").join("beta_gamma.rs"), "b");
    let _ = std::fs::write(ws.join(".gitignore"), "ignored.log\n");
    let _ = std::fs::write(ws.join("ignored.log"), "x");
    let names = app.st_workspace_tree(&ws);
    check(
        "workspace_tree",
        names.first().map(String::as_str) == Some("sub")
            && names.iter().any(|n| n == "alpha.txt")
            && !names.iter().any(|n| n == "ignored.log"),
        &format!("{names:?}"),
        &mut out,
    );
    let _ = std::fs::write(ws.join("delta.txt"), "d");
    let _ = std::fs::remove_file(ws.join("alpha.txt"));
    let synced = app.st_workspace_sync();
    check(
        "workspace_sync",
        synced.iter().any(|n| n == "delta.txt")
            && !synced.iter().any(|n| n == "alpha.txt")
            && synced.first().map(String::as_str) == Some("sub"),
        &format!("{synced:?}"),
        &mut out,
    );
    let hits = app.st_quick("bgam");
    check(
        "quick_open",
        hits.first()
            .and_then(|p| p.file_name())
            .is_some_and(|n| n == "beta_gamma.rs")
            && !app
                .st_quick("ignored")
                .iter()
                .any(|p| p.ends_with("ignored.log")),
        &format!("{:?}", hits.iter().take(3).collect::<Vec<_>>()),
        &mut out,
    );
    app.st_clear_workspace();
    check(
        "sidebar_list",
        app.st_list_count() == app.st_doc_count(),
        &format!("list={} docs={}", app.st_list_count(), app.st_doc_count()),
        &mut out,
    );

    // 13. 分屏：克隆当前文档；两个视图显示不同文档；F6 切换焦点；取消分屏保留焦点文档
    let a = app.st_active_id();
    app.st_split(1);
    let (_, v0, v1, p0, p1) = app.st_views();
    let cloned = v0 == a && v1 == a && p0 == p1 && p0 != 0;
    app.new_document();
    let b = app.st_active_id();
    let (_, v0b, v1b, _, _) = app.st_views();
    let separate = b != a && v0b == b && v1b == a;
    let mru_ok = app.st_switcher_target() == Some(a);
    app.st_toggle_focus();
    let (f, _, _, _, _) = app.st_views();
    let switched = f == 1 && app.st_active_id() == a;
    app.st_split(0);
    let (f2, v0c, v1c, _, _) = app.st_views();
    let merged = f2 == 0 && v0c == a && v1c == 0 && app.st_active_id() == a;
    check(
        "split_view",
        cloned && separate && switched && merged,
        &format!("cloned={cloned} separate={separate} switched={switched} merged={merged}"),
        &mut out,
    );
    check("mru", mru_ok, "", &mut out);
    // 关闭分屏测试中新建的文档
    app.activate(b);
    app.st_close_active();

    // 14. 大文件视图：超过阈值改用大文件视图、行索引、跳转、绘制、搜索、选中、Hex 切换
    large_checks(app, &dir, &mut |n, p, d| check(n, p, d, &mut out));

    // 15. 开发者工具：目录搜索/替换、运行 Python、终端
    dev_checks(app, &dir, &mut |n, p, d| check(n, p, d, &mut out));

    // 16. 内容：Markdown 预览、宏、打印、加密界面
    let metrics = m6_checks(app, &dir, &mut |n, p, d| check(n, p, d, &mut out));
    for m in metrics {
        writeln!(out, "{m}").unwrap();
    }

    let _ = std::fs::remove_dir_all(&dir);

    sci.set_save_point();
    writeln!(out, "result={}", if ok { "pass" } else { "fail" }).unwrap();
    out
}

/// 大文件视图、Hex 视图、Tail 模式的检查。
fn large_checks(app: &App, dir: &std::path::Path, check: &mut dyn FnMut(&str, bool, &str)) {
    app.st_set_large_threshold(64 * 1024);

    // 20000 行日志（约 1 MB），每 1000 行中第 7 行含 NEEDLE
    let mut content = String::new();
    let mut starts = Vec::new();
    for i in 0..20000 {
        starts.push(content.len() as u64);
        let mark = if i % 1000 == 7 { " NEEDLE" } else { "" };
        content.push_str(&format!("{i:06} INFO 第{i}行 message{mark}\r\n"));
    }
    let big = dir.join("big.log");
    let _ = std::fs::write(&big, &content);
    app.st_open(&big);
    let rc = app.st_active_large();
    let indexed = app.st_wait_index(15_000);
    let lines = rc
        .as_ref()
        .map(|r| r.borrow().index.line_count())
        .unwrap_or(0);
    check(
        "large_open",
        app.st_active_is_large() && indexed && lines == 20001,
        &format!(
            "large={} indexed={indexed} lines={lines}",
            app.st_active_is_large()
        ),
    );
    let Some(rc) = rc else { return };

    let off = app.st_large_goto_line(12344);
    check(
        "large_goto",
        off == Some(starts[12344]),
        &format!("{off:?} != {}", starts[12344]),
    );

    let lv = lview::view(app.st_focus_view_index());
    let rows = lv.as_ref().map(|v| v.st_paint()).unwrap_or(0);
    let surface = lv.as_ref().is_some_and(|v| v.st_has_surface());
    check(
        "large_paint",
        rows > 0 && surface,
        &format!("rows={rows} surface={surface}"),
    );

    // 键盘：下移一行、行尾
    if let Some(v) = &lv {
        v.st_key(0x28); // VK_DOWN
        v.st_key(0x23); // VK_END
    }
    let caret = rc.borrow().caret;
    let line_end = starts[12346] - 2;
    check(
        "large_keys",
        caret == line_end,
        &format!("caret={caret} expected={line_end}"),
    );

    let st = FindState {
        pattern: "NEEDLE".into(),
        wrap: true,
        ..Default::default()
    };
    app.find_action(FindAction::MarkAll, st.clone());
    let res = app.st_results();
    check(
        "large_find_all",
        res.len() == 20 && res[0].0 == Some(7) && res[19].0 == Some(19007),
        &format!(
            "n={} first={:?} last={:?}",
            res.len(),
            res.first().map(|r| r.0),
            res.last().map(|r| r.0)
        ),
    );

    {
        let mut d = rc.borrow_mut();
        d.caret = starts[8];
        d.anchor = starts[8];
    }
    app.find_action(FindAction::Next, st.clone());
    let sel = rc.borrow().sel();
    let needle = content.find(" NEEDLE").unwrap() as u64; // 第 7 行
    let second = needle + content[needle as usize + 1..].find(" NEEDLE").unwrap() as u64 + 1;
    check(
        "large_find_next",
        sel == (second + 1, second + 7),
        &format!("{sel:?} expected ({}, {})", second + 1, second + 7),
    );
    app.find_action(FindAction::Prev, st);
    let sel = rc.borrow().sel();
    check(
        "large_find_prev",
        sel == (needle + 1, needle + 7),
        &format!("{sel:?}"),
    );
    let text = rc.borrow().selected_text().unwrap_or_default();
    check("large_copy", text == "NEEDLE", &format!("{text:?}"));

    let caret = rc.borrow().caret;
    app.st_toggle_hex();
    let (mode, hex_top, vis) = {
        let d = rc.borrow();
        (
            d.mode,
            d.hex_top,
            lv.as_ref().map(|v| v.visible_rows()).unwrap_or(1),
        )
    };
    let row = caret / 16;
    let hex_rows = lv.as_ref().map(|v| v.st_paint()).unwrap_or(0);
    app.st_toggle_hex();
    let back = rc.borrow().mode;
    check(
        "large_hex_toggle",
        mode == ViewMode::Hex
            && row >= hex_top
            && row < hex_top + vis
            && back == ViewMode::Text
            && rc.borrow().caret == caret
            && hex_rows == 0,
        &format!("mode={mode:?} row={row} top={hex_top} vis={vis} back={back:?}"),
    );
    drop(rc);
    app.st_close_active();

    // 普通文档的临时 Hex 视图：光标位置换算
    app.new_document();
    if let Some(sci) = app.st_sci() {
        set_text(&sci, "AB中文");
        sci.set_sel(2, 2);
        app.st_toggle_hex();
        let ov = app.st_active_large();
        let ok1 = ov.as_ref().is_some_and(|r| {
            let d = r.borrow();
            d.embedded && d.len() == 8 && d.caret == 2
        });
        if let Some(r) = &ov {
            let mut d = r.borrow_mut();
            d.caret = 5;
            d.anchor = 5;
        }
        drop(ov);
        app.st_toggle_hex();
        let ok2 = app.st_active_large().is_none() && sci.current_pos() == 5;
        check(
            "hex_embedded",
            ok1 && ok2,
            &format!("ok1={ok1} ok2={ok2} pos={}", sci.current_pos()),
        );
    }
    app.st_close_active();

    // Tail：普通文档切换为 Tail、增长、轮转（改名后新建）
    let log = dir.join("tail.log");
    let _ = std::fs::write(&log, "a\r\n".repeat(10));
    app.st_open(&log);
    let was_text = !app.st_active_is_large();
    app.st_toggle_tail();
    let rc = app.st_active_large();
    let t1 = app.st_wait_index(5000)
        && rc.as_ref().is_some_and(|r| {
            let d = r.borrow();
            d.tail && d.index.line_count() == 11
        });
    let appended = std::fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all("b\r\n".repeat(5).as_bytes())
        })
        .is_ok();
    app.st_poll_large();
    let t2 = appended
        && app.st_wait_index(5000)
        && rc.as_ref().is_some_and(|r| {
            let d = r.borrow();
            d.index.line_count() == 16 && d.caret == d.len() && d.len() == 45
        });
    let rotated = std::fs::rename(&log, dir.join("tail.log.1")).is_ok()
        && std::fs::write(&log, "new\r\n").is_ok();
    app.st_poll_large();
    let t3 = rotated
        && app.st_wait_index(5000)
        && rc.as_ref().is_some_and(|r| {
            let d = r.borrow();
            d.index.line_count() == 2 && d.len() == 5
        });
    drop(rc);
    app.st_toggle_tail();
    let reverted = !app.st_active_is_large() && app.st_active_large().is_none();
    check(
        "tail",
        was_text && t1 && t2 && t3 && reverted,
        &format!("text={was_text} t1={t1} t2={t2} rotated={rotated} t3={t3} reverted={reverted}"),
    );
    app.st_close_active();

    // 二进制检测
    let bin = dir.join("x.bin");
    let _ = std::fs::write(&bin, (0u8..=255).collect::<Vec<u8>>());
    check(
        "binary_sniff",
        crate::docs::sniff_binary(&bin) && !crate::docs::sniff_binary(&log),
        "",
    );
}

/// 等待条件成立（不处理消息；用于后台线程推进的状态）。
fn wait_until(timeout_ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    loop {
        if f() {
            return true;
        }
        if start.elapsed().as_millis() as u64 > timeout_ms {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// M5：目录搜索与替换、输出面板与运行 Python、终端。
fn dev_checks(app: &App, dir: &std::path::Path, check: &mut dyn FnMut(&str, bool, &str)) {
    use crate::dirdlg::DirForm;
    use hn_text::encoding::encoding_rs::GBK;

    // ---- 目录搜索 ----
    let root = dir.join("dirsearch");
    let _ = std::fs::create_dir_all(root.join("src"));
    let _ = std::fs::create_dir_all(root.join("node_modules"));
    let _ = std::fs::write(
        root.join("src/a.rs"),
        "fn needle() {}\r\n// needle again\r\n",
    );
    let _ = std::fs::write(root.join("src/b.txt"), "nothing here\r\n");
    let _ = std::fs::write(root.join("node_modules/x.js"), "needle");
    let gbk_text = "这是一段用于测试简体中文编码识别的文字，需要足够长。\r\n中文 needle 测试\r\n";
    let _ = std::fs::write(root.join("gbk.txt"), GBK.encode(gbk_text).0);
    let mut form = DirForm {
        pattern: "needle".into(),
        dir: root.display().to_string(),
        exclude: "node_modules".into(),
        case_sensitive: true,
        gitignore: true,
        ..Default::default()
    };
    let status = app.st_dir_search(form.clone(), false);
    let res = app.st_dir_results();
    let mut files: Vec<String> = res
        .iter()
        .filter(|r| r.1 == 0)
        .map(|r| r.0.clone())
        .collect();
    files.sort();
    let gbk_hit = res
        .iter()
        .position(|r| r.0 == "gbk.txt" && r.1 == 2 && r.2 == "中文 needle 测试");
    check(
        "dir_search",
        files == ["a.rs", "gbk.txt"] && res.len() == 5 && gbk_hit.is_some(),
        &format!("files={files:?} rows={} status={status}", res.len()),
    );

    // 跳转：打开文件并选中匹配
    let jumped = gbk_hit.is_some_and(|i| {
        app.st_jump_result(i);
        let sel = app.st_sci().map(|s| {
            String::from_utf8_lossy(&s.text_range(s.selection_start(), s.selection_end()))
                .into_owned()
        });
        app.st_active_path().is_some_and(|p| p.ends_with("gbk.txt"))
            && sel.as_deref() == Some("needle")
    });
    check("dir_jump", jumped, "");
    let drawer = app.st_drawer();
    check(
        "drawer_results",
        drawer == (true, Some("results")),
        &format!("{drawer:?}"),
    );
    app.st_close_active();

    // 替换：a.rs 已在编辑器中打开 → 在缓冲区替换；gbk.txt 未打开 → 按 GBK 写回磁盘
    let a_rs = root.join("src/a.rs");
    app.st_open(&a_rs);
    form.replace = "pin".into();
    let status = app.st_dir_search(form.clone(), true);
    let gbk_now = std::fs::read(root.join("gbk.txt")).unwrap_or_default();
    let gbk_ok = gbk_now
        == GBK
            .encode(&gbk_text.replace("needle", "pin"))
            .0
            .into_owned();
    let disk_a = std::fs::read_to_string(&a_rs).unwrap_or_default();
    let buf_a = app.st_sci().map(|s| text(&s)).unwrap_or_default();
    let in_buffer = app.st_active_path().is_some_and(|p| p.ends_with("a.rs"))
        && buf_a.contains("fn pin() {}")
        && disk_a.contains("fn needle() {}");
    check(
        "dir_replace",
        gbk_ok && in_buffer,
        &format!("gbk={gbk_ok} buffer={in_buffer} status={status}"),
    );
    app.st_close_active();

    // ---- 运行 Python ----
    let script = dir.join("err.py");
    let _ = std::fs::write(
        &script,
        "print('hello 中文')\nimport sys\nprint('to-stderr', file=sys.stderr)\nraise ValueError('boom')\n",
    );
    if app.st_python_available(&script) {
        let code = app.st_run_python(&script, 30_000);
        let out = app.st_output_text();
        let ran = code == Some(Some(1))
            && out.contains("hello 中文")
            && out.contains("to-stderr")
            && out.contains("ValueError: boom")
            && out.contains("代码 1");
        check(
            "run_python",
            ran,
            &format!(
                "code={code:?} out={:?}",
                out.chars().take(300).collect::<String>()
            ),
        );
        let jumped = app.st_output_jump("line 4")
            && app.st_active_path().is_some_and(|p| p.ends_with("err.py"))
            && app
                .st_sci()
                .is_some_and(|s| s.line_from_pos(s.current_pos()) == 3);
        check("run_traceback_jump", jumped, "");
        app.st_close_active();
    } else {
        check("run_python", true, "skipped=no-python");
    }

    // ---- 终端 ----
    match app.st_new_terminal("cmd") {
        Some(id) => {
            let v = crate::term::view(id);
            let started = v
                .as_ref()
                .is_some_and(|v| wait_until(15_000, || v.text().contains('>')));
            crate::term::write(id, b"echo hn-term-%OS%\r".to_vec());
            let echoed = v
                .as_ref()
                .is_some_and(|v| wait_until(10_000, || v.text().contains("hn-term-Windows_NT")));
            let cols = v.as_ref().map(|v| v.terminal().size().cols).unwrap_or(0);
            crate::term::write(id, b"exit\r".to_vec());
            let exited = v
                .as_ref()
                .is_some_and(|v| wait_until(10_000, || v.exited()));
            let text = v.as_ref().map(|v| v.text()).unwrap_or_default();
            check(
                "terminal",
                started && echoed && exited && cols > 20,
                &format!(
                    "started={started} echoed={echoed} exited={exited} cols={cols} text={:?}",
                    text.chars().take(200).collect::<String>()
                ),
            );
            drop(v);
            app.st_close_terminal(id);
        }
        None => check("terminal", false, "spawn failed"),
    }

    // PowerShell：PSReadLine 补全、彩色输出、中文输入输出
    match app.st_new_terminal("powershell") {
        Some(id) => {
            let v = crate::term::view(id);
            let started = v
                .as_ref()
                .is_some_and(|v| wait_until(30_000, || v.text().contains("PS ")));
            crate::term::write(id, b"Write-Host -ForegroundColor Red hn-red\r".to_vec());
            let red = v.as_ref().is_some_and(|v| {
                wait_until(10_000, || {
                    let snap = v.terminal().snapshot();
                    (0..snap.size.rows).any(|r| {
                        snap.row_text(r) == "hn-red"
                            && snap.cell(r, 0).fg != hn_term::palette::Palette::default().fg
                    })
                })
            });
            crate::term::write(id, b"Get-ChildIt\t".to_vec());
            let completed = v
                .as_ref()
                .is_some_and(|v| wait_until(10_000, || v.text().contains("Get-ChildItem")));
            // 执行补全出的命令，再输出中文（ESC 紧跟字符会被当成 Alt 组合键，不能用来清行）
            crate::term::write(id, b"\r".to_vec());
            crate::term::write(id, "echo 中文hn\r".as_bytes().to_vec());
            let chinese = v
                .as_ref()
                .is_some_and(|v| wait_until(10_000, || v.text().matches("中文hn").count() >= 2));
            let text = v.as_ref().map(|v| v.text()).unwrap_or_default();
            crate::term::write(id, b"exit\r".to_vec());
            let exited = v
                .as_ref()
                .is_some_and(|v| wait_until(10_000, || v.exited()));
            check(
                "terminal_powershell",
                started && red && completed && chinese && exited,
                &format!(
                    "started={started} red={red} completed={completed} chinese={chinese} exited={exited} text={:?}",
                    text.lines().rev().take(6).collect::<Vec<_>>()
                ),
            );
            drop(v);
            app.st_close_terminal(id);
        }
        None => check("terminal_powershell", false, "spawn failed"),
    }
}

/// `--selftest-large <文件> <输出>`：大文件视图性能测试（CI 用 5 GB 日志）。
pub fn run_large(app: &App, file: &std::path::Path) -> String {
    use std::time::Instant;
    let mut out = String::new();
    let len = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    writeln!(out, "file_mb={:.0}", len as f64 / 1048576.0).unwrap();

    // 不预热：按真实的首次打开测量
    let t0 = Instant::now();
    let id = app.st_open_large(file, false, false);
    let open = t0.elapsed().as_secs_f64() * 1000.0;
    let lv = lview::view(app.st_focus_view_index());
    let rows = lv.as_ref().map(|v| v.st_paint()).unwrap_or(0);
    let first = t0.elapsed().as_secs_f64() * 1000.0;
    let t1 = Instant::now();
    if let Some(v) = &lv {
        v.st_paint();
    }
    let repaint = t1.elapsed().as_secs_f64() * 1000.0;
    writeln!(out, "first_screen_ms={first:.1}").unwrap();
    writeln!(out, "open_ms={open:.1}").unwrap();
    writeln!(out, "first_paint_ms={:.1}", first - open).unwrap();
    writeln!(out, "repaint_ms={repaint:.1}").unwrap();
    for (k, v) in lview::st_timings() {
        writeln!(out, "{k}={v:.1}").unwrap();
    }
    writeln!(out, "rows={rows}").unwrap();
    let (Some(_), Some(rc), Some(lv)) = (id, app.st_active_large(), lv) else {
        out.push_str("result=fail\n");
        return out;
    };
    let vis = lv.visible_rows();

    let time = |f: &dyn Fn()| -> f64 {
        let t = Instant::now();
        f();
        lv.st_paint();
        t.elapsed().as_secs_f64() * 1000.0
    };
    let mid = time(&|| rc.borrow_mut().scroll_to_fraction(0.5, vis));
    let end = time(&|| rc.borrow_mut().goto_end(vis));
    let mut worst = 0f64;
    let mut total = 0f64;
    rc.borrow_mut().scroll_to_fraction(0.3, vis);
    for _ in 0..60 {
        let ms = time(&|| rc.borrow_mut().scroll(vis as i64, vis));
        worst = worst.max(ms);
        total += ms;
    }
    writeln!(out, "jump_middle_ms={mid:.1}").unwrap();
    writeln!(out, "jump_end_ms={end:.1}").unwrap();
    writeln!(out, "page_down_avg_ms={:.2}", total / 60.0).unwrap();
    writeln!(out, "page_down_worst_ms={worst:.2}").unwrap();

    let t = Instant::now();
    let indexed = app.st_wait_index(180_000);
    let lines = rc.borrow().index.line_count();
    writeln!(out, "index_ms={:.0}", t.elapsed().as_secs_f64() * 1000.0).unwrap();
    writeln!(out, "lines={lines}").unwrap();
    let goto = time(&|| {
        app.st_large_goto_line(lines / 2);
    });
    writeln!(out, "goto_middle_ms={goto:.1}").unwrap();

    let ok = rows > 0 && indexed && first < 300.0 && worst < 100.0;
    writeln!(out, "result={}", if ok { "pass" } else { "fail" }).unwrap();
    drop(rc);
    app.st_close_active();
    out
}

/// `--bench-dirsearch <目录> <查找内容> <输出>`：目录搜索基准（不创建窗口）。
/// 先预热一次（让文件进入系统缓存），再测 3 次取中位数，与 ripgrep 对比用。
pub fn bench_dir(dir: &std::path::Path, pattern: &str, out: &std::path::Path) -> i32 {
    use hn_search::dir::{search_dir, DirQuery};
    use std::sync::atomic::AtomicBool;
    let mut q = DirQuery::new(
        vec![dir.to_path_buf()],
        pattern,
        Options {
            mode: Mode::Normal,
            case_sensitive: true,
            ..Default::default()
        },
    );
    q.exclude.clear();
    let cancel = AtomicBool::new(false);
    let mut report = String::new();
    let mut times = Vec::new();
    let mut last = None;
    for i in 0..4 {
        let t = std::time::Instant::now();
        let r = search_dir(
            &q,
            &cancel,
            std::time::Duration::from_millis(200),
            &|_, _| {},
        );
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        match r {
            Ok(p) => {
                if i > 0 {
                    times.push(ms);
                }
                last = Some(p);
            }
            Err(e) => {
                let _ = writeln!(report, "error={e}");
                break;
            }
        }
    }
    times.sort_by(|a, b| a.total_cmp(b));
    if let (Some(p), Some(median)) = (last, times.get(times.len() / 2)) {
        let _ = writeln!(report, "dirsearch_ms={median:.1}");
        let _ = writeln!(
            report,
            "dirsearch_runs={}",
            times
                .iter()
                .map(|t| format!("{t:.1}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let _ = writeln!(report, "files_scanned={}", p.files_scanned);
        let _ = writeln!(report, "files_matched={}", p.files_matched);
        let _ = writeln!(report, "matches={}", p.matches);
        let _ = writeln!(report, "skipped_binary={}", p.skipped_binary);
        report.push_str("result=pass\n");
    } else {
        report.push_str("result=fail\n");
    }
    match std::fs::write(out, report) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// 2×2 的 24 位 BMP（预览图片解码用）。
fn bmp_2x2() -> Vec<u8> {
    let (off, data) = (54u32, 16u32);
    let mut v = Vec::new();
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&(off + data).to_le_bytes());
    v.extend_from_slice(&[0u8; 4]);
    v.extend_from_slice(&off.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&2i32.to_le_bytes());
    v.extend_from_slice(&2i32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&data.to_le_bytes());
    v.extend_from_slice(&2835i32.to_le_bytes());
    v.extend_from_slice(&2835i32.to_le_bytes());
    v.extend_from_slice(&[0u8; 8]);
    for _ in 0..2 {
        v.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
    }
    v
}

/// M6：Markdown 预览、宏、打印、加密界面。返回性能数据（`key=value` 行）。
fn m6_checks(
    app: &App,
    dir: &std::path::Path,
    check: &mut dyn FnMut(&str, bool, &str),
) -> Vec<String> {
    let mut metrics = Vec::new();
    let Some(sci) = app.st_sci() else {
        check("m6_sci", false, "");
        return metrics;
    };
    let Some(v) = crate::mdview::view() else {
        check("md_view", false, "no preview window");
        return metrics;
    };

    // ---- Markdown 预览 ----
    let _ = std::fs::write(dir.join("img.bmp"), bmp_2x2());
    let md_path = dir.join("doc.md");
    let src = "# 标题\r\n\r\n段落 **粗体** 与 [链接](https://example.com/x)。\r\n\r\n- 一\r\n- 二\r\n  1. 嵌套\r\n\r\n| a | b |\r\n|---|--:|\r\n| 1 | 2 |\r\n\r\n```rust\r\nfn main() {}\r\n```\r\n\r\n![图](img.bmp)\r\n\r\n> 引用\r\n";
    let _ = std::fs::write(&md_path, src);
    app.st_open(&md_path);
    check(
        "md_visible",
        app.st_md_visible() && v.is_shown() && app.st_active_lang() == "markdown",
        &format!(
            "visible={} lang={}",
            app.st_md_visible(),
            app.st_active_lang()
        ),
    );
    let (blocks, laid, total, _) = v.st_stats();
    check(
        "md_blocks",
        blocks == 9 && laid == 9 && total > 0.0,
        &format!("blocks={blocks} laid={laid} total={total}"),
    );
    check("md_paint", v.st_paint(), "");
    let link = v.st_link_point(1).and_then(|(x, y)| v.st_link_at(x, y));
    check(
        "md_link_hit",
        link.as_deref() == Some("https://example.com/x"),
        &format!("{link:?} at {:?}", v.st_link_point(1)),
    );
    let img_ok = wait_until(8000, || {
        v.st_pump();
        v.st_images().0 == 1
    });
    check("md_image", img_ok, &format!("{:?}", v.st_images()));
    let all = v.st_select_all_text();
    check(
        "md_copy",
        all.contains("标题")
            && all.contains("a\tb")
            && all.contains("fn main() {}")
            && all.contains("• 一"),
        &format!("{all:?}"),
    );
    let ys: Vec<f32> = (0..22).map(|l| v.st_y_for_line(l as f64)).collect();
    let mono = ys.windows(2).all(|w| w[1] >= w[0]);
    let back = v.st_line_for_y(v.st_y_for_line(10.0));
    check(
        "md_line_map",
        mono && ys[21] > ys[0] && (back - 10.0).abs() < 1.0,
        &format!("mono={mono} back={back:.2} ys={ys:?}"),
    );
    // 只改一个段落：其余块复用排版结果
    let pos = sci.with_text(|t| {
        std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.find("段落"))
            .unwrap_or(0)
    });
    sci.set_sel(pos, pos);
    sci.send_str(SCI_REPLACESEL, 0, "新");
    app.st_md_push();
    let (b2, _, _, rebuilt) = v.st_stats();
    check(
        "md_incremental",
        b2 == 9 && rebuilt <= 2,
        &format!("blocks={b2} rebuilt={rebuilt}"),
    );
    app.st_set_language("text");
    check(
        "md_hidden_for_text",
        !app.st_md_visible() && !v.is_shown(),
        "",
    );
    app.st_set_language("markdown");
    check("md_shown_again", app.st_md_visible() && v.is_shown(), "");
    app.st_close_active();

    // ---- 10 万字文档：首次排版、编辑后更新、滚动同步 ----
    app.new_document();
    let mut big = String::new();
    let mut i = 0;
    let mut chars = 0;
    while chars < 100_000 {
        let before = big.len();
        big.push_str(&format!(
            "## 第 {i} 节\r\n\r\n这是一段用于测试预览性能的**中文**文字，包含 `代码` 与 [链接](https://example.com/{i})。\r\n\r\n- 项目甲\r\n- 项目乙\r\n\r\n"
        ));
        chars += big[before..].chars().count();
        i += 1;
    }
    set_text(&sci, &big);
    // 切换为 Markdown 时预览立即解析，在时间预算内排版一部分（其余用估计高度），空闲时补齐
    let t = std::time::Instant::now();
    app.st_set_language("markdown");
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;
    let (bb, built_first, _, _) = v.st_stats();
    let t = std::time::Instant::now();
    v.st_finish_layout();
    let rest_ms = t.elapsed().as_secs_f64() * 1000.0;
    let (_, built_all, total_big, _) = v.st_stats();
    metrics.push(format!("md_first_ms={first_ms:.1}"));
    metrics.push(format!("md_full_ms={:.1}", first_ms + rest_ms));
    check(
        "md_progressive",
        built_first < bb && built_all == bb && first_ms < 200.0,
        &format!(
            "blocks={bb} first_built={built_first} all_built={built_all} first={first_ms:.0}ms"
        ),
    );
    let mid = sci.line_start(sci.line_count() / 2);
    sci.set_sel(mid, mid);
    sci.send_str(SCI_REPLACESEL, 0, "插入");
    let t = std::time::Instant::now();
    app.st_md_push();
    let edit_ms = t.elapsed().as_secs_f64() * 1000.0;
    let (_, _, _, rebuilt_big) = v.st_stats();
    metrics.push(format!("md_blocks_100k={bb}"));
    metrics.push(format!("md_edit_ms={edit_ms:.1}"));
    check(
        "md_large",
        bb > 1000 && rebuilt_big <= 3,
        &format!("blocks={bb} rebuilt={rebuilt_big} edit={edit_ms:.0}ms"),
    );
    sci.send(SCI_SETFIRSTVISIBLELINE, sci.line_count() / 2, 0);
    app.st_md_editor_scrolled();
    let s1 = v.st_scroll();
    check(
        "md_sync_editor_to_preview",
        s1 > total_big * 0.3 && s1 < total_big * 0.7,
        &format!("scroll={s1} total={total_big}"),
    );
    v.st_set_scroll(0.0);
    let first = sci.send(SCI_GETFIRSTVISIBLELINE, 0, 0);
    check(
        "md_sync_preview_to_editor",
        first < 3,
        &format!("first={first}"),
    );
    app.st_close_active();

    // ---- 宏：录制（含回车触发的列表续写与菜单命令）→ 回放 → 撤销 ----
    app.new_document();
    set_text(&sci, "");
    app.st_toggle_record();
    sci.send_str(SCI_REPLACESEL, 0, "- a");
    sci.send(SCI_NEWLINE, 0, 0);
    sci.send_str(SCI_REPLACESEL, 0, "b");
    app.st_command(IDM_EDIT_SELECT_ALL);
    app.st_command(IDM_EDIT_UPPER);
    sci.send(SCI_DOCUMENTEND, 0, 0);
    app.st_toggle_record();
    let recorded = text(&sci);
    let steps = app.st_macro_steps();
    check(
        "macro_record",
        recorded == "- A\r\n- B" && steps == 6,
        &format!("{recorded:?} steps={steps}"),
    );
    set_text(&sci, "");
    app.st_play_macro(1, false);
    let played = text(&sci);
    check("macro_play", played == "- A\r\n- B", &format!("{played:?}"));
    sci.send(SCI_UNDO, 0, 0);
    check(
        "macro_single_undo",
        text(&sci).is_empty(),
        &format!("{:?}", text(&sci)),
    );
    // 运行到文件末尾
    set_text(&sci, "x\r\nx\r\nx\r\n");
    app.st_toggle_record();
    sci.send_str(SCI_REPLACESEL, 0, "* ");
    sci.send(SCI_LINEDOWN, 0, 0);
    sci.send(SCI_HOME, 0, 0);
    app.st_toggle_record();
    set_text(&sci, "x\r\nx\r\nx\r\n");
    sci.send(SCI_GOTOPOS, 0, 0);
    app.st_play_macro(1, true);
    let to_end = text(&sci);
    check(
        "macro_until_end",
        to_end == "* x\r\n* x\r\n* x\r\n",
        &format!("{to_end:?}"),
    );
    app.st_close_active();

    // ---- 打印：分页（内存 DC），打印后恢复缩放样式 ----
    app.new_document();
    let lines: String = (0..400)
        .map(|i| format!("第 {i} 行 print test line\r\n"))
        .collect();
    set_text(&sci, &lines);
    app.st_command(IDM_VIEW_ZOOM_IN);
    let size_before = sci.send(SCI_STYLEGETSIZEFRACTIONAL, STYLE_DEFAULT as usize, 0);
    let pages = app.st_print_pages();
    let size_after = sci.send(SCI_STYLEGETSIZEFRACTIONAL, STYLE_DEFAULT as usize, 0);
    app.st_command(IDM_VIEW_ZOOM_RESET);
    check(
        "print_paginate",
        matches!(pages, Ok(n) if n >= 2) && size_before == size_after,
        &format!("{pages:?} size {size_before} -> {size_after}"),
    );
    app.st_close_active();

    // ---- 加密：修改密码 ----
    let pw_path = dir.join("pw.hne");
    let pw_res = (|| -> Result<(), String> {
        let e = |x: &dyn std::fmt::Display| x.to_string();
        let fast = hn_crypto::KdfParams {
            m_cost_kib: 1024,
            t_cost: 1,
            p_cost: 1,
        };
        let key = hn_crypto::Key::generate("旧密码", fast).map_err(|x| e(&x))?;
        let payload = hn_core::payload::Payload::new("改密测试".as_bytes().to_vec());
        let file = hn_crypto::seal(&key, &payload.encode()).map_err(|x| e(&x))?;
        std::fs::write(&pw_path, &file).map_err(|x| e(&x))?;
        let stamp = hn_core::stamp::DiskStamp::new(0, &file);
        if !app.st_install_encrypted(&pw_path, key, payload, stamp) {
            return Err("install".into());
        }
        if app.st_change_password("错误", "新密码").is_ok() {
            return Err("wrong old password accepted".into());
        }
        app.st_change_password("旧密码", "新密码")?;
        let written = std::fs::read(&pw_path).map_err(|x| e(&x))?;
        if hn_crypto::open_with_password("旧密码", &written).is_ok() {
            return Err("old password still opens".into());
        }
        let (_, pt) = hn_crypto::open_with_password("新密码", &written).map_err(|x| e(&x))?;
        let p = hn_core::payload::Payload::decode(&pt).map_err(|x| e(&x))?;
        if p.text != "改密测试".as_bytes() {
            return Err("content changed".into());
        }
        Ok(())
    })();
    check(
        "encrypt_change_password",
        pw_res.is_ok(),
        &format!("{pw_res:?}"),
    );
    if app.st_active_encrypted() {
        app.st_close_active();
    }
    metrics
}
