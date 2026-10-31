//! `--selftest <输出文件>`：在真实窗口中做基本检查，结果写入文件后退出。
//!
//! CI 用它验证 exe 能启动、Scintilla 与 Lexilla 静态链接正常，并测量启动耗时。

use std::fmt::Write as _;

use hn_sci::*;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

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

/// 执行检查，返回 `key=value` 格式的报告。最后一行 `result=pass|fail`。
pub fn run(sci: &Sci, startup_ms: f64) -> String {
    let mut out = String::new();
    let mut ok = true;
    let mut check = |name: &str, pass: bool, out: &mut String| {
        writeln!(out, "{name}={}", if pass { 1 } else { 0 }).unwrap();
        ok &= pass;
    };

    writeln!(out, "startup_ms={startup_ms:.1}").unwrap();

    // 1. 中文文本往返
    let sample = "测试 HurshNote 😀\r\n第二行";
    sci.load_document(sample.as_bytes());
    let back = sci.bytes();
    check("sci_roundtrip", back == sample.as_bytes(), &mut out);
    check("sci_not_modified_after_load", !sci.is_modified(), &mut out);

    // 2. 编辑后变为已修改，撤销可用
    sci.send(SCI_APPENDTEXT, 1, b"x".as_ptr() as isize);
    check("sci_modified_after_edit", sci.is_modified(), &mut out);
    check("sci_can_undo", sci.send(SCI_CANUNDO, 0, 0) != 0, &mut out);

    // 3. 渲染技术与编码
    check(
        "sci_directwrite",
        sci.send(SCI_GETTECHNOLOGY, 0, 0) == SC_TECHNOLOGY_DIRECTWRITE,
        &mut out,
    );
    check(
        "sci_utf8",
        sci.send(SCI_GETCODEPAGE, 0, 0) == SC_CP_UTF8,
        &mut out,
    );

    // 4. Lexilla
    writeln!(out, "lexer_count={}", hn_sci::lexer_count()).unwrap();
    check(
        "lexer_python",
        sci.set_lexer("python") && sci.send(SCI_GETLEXER, 0, 0) == SCLEX_PYTHON,
        &mut out,
    );
    check(
        "lexer_json",
        sci.set_lexer("json") && sci.send(SCI_GETLEXER, 0, 0) == SCLEX_JSON,
        &mut out,
    );
    check(
        "lexer_unknown_rejected",
        !sci.set_lexer("no-such-lexer"),
        &mut out,
    );
    sci.set_lexer("null");

    writeln!(out, "result={}", if ok { "pass" } else { "fail" }).unwrap();
    out
}
