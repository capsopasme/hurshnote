//! 编译 third_party 中的 Scintilla 与 Lexilla（静态库），并从头文件生成 Rust 常量。

use std::collections::HashSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// 编进程序的 Lexilla lexer 源文件（与 native/hn_lexers.cxx 中的列表保持一致）。
const LEXERS: &[&str] = &[
    "LexBash",
    "LexBatch",
    "LexCPP",
    "LexCSS",
    "LexDiff",
    "LexHTML",
    "LexJSON",
    "LexMarkdown",
    "LexNull",
    "LexPowerShell",
    "LexProps",
    "LexPython",
    "LexRust",
    "LexSQL",
    "LexTOML",
    "LexYAML",
];

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let third = manifest.join("../../third_party");
    let sci = third.join("scintilla");
    let lex = third.join("lexilla");
    let native = manifest.join("native");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", native.display());
    println!(
        "cargo:rerun-if-changed={}",
        third.join("VERSIONS.txt").display()
    );
    println!("cargo:rerun-if-env-changed=HN_SKIP_NATIVE");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    if !sci.join("include/Scintilla.h").exists() || !lex.join("include/SciLexer.h").exists() {
        panic!(
            "缺少 third_party/scintilla 或 third_party/lexilla 源码。\n\
             运行 `python scripts/vendor_third_party.py` 下载，或拉取最新的仓库内容。"
        );
    }

    generate_consts(&sci, &lex);

    if env::var_os("HN_SKIP_NATIVE").is_some() {
        println!("cargo:warning=HN_SKIP_NATIVE 已设置，跳过 Scintilla/Lexilla 的 C++ 编译（只能用于 cargo check）");
        return;
    }

    let release = env::var("PROFILE").as_deref() == Ok("release");

    // ---- Scintilla ----
    let mut b = base_build(release);
    b.include(sci.join("include")).include(sci.join("src"));
    for f in cxx_files(&sci.join("src")) {
        b.file(f);
    }
    for f in cxx_files(&sci.join("win32")) {
        if f.file_name().is_some_and(|n| n == "ScintillaDLL.cxx") {
            continue;
        }
        b.file(f);
    }
    b.file(native.join("hn_loader.cxx"));
    b.file(native.join("hn_drop.cxx"));
    b.compile("scintilla");

    // ---- Lexilla（只编译需要的 lexer） ----
    let mut b = base_build(release);
    b.include(lex.join("include"))
        .include(lex.join("lexlib"))
        .include(sci.join("include"));
    for f in cxx_files(&lex.join("lexlib")) {
        b.file(f);
    }
    for name in LEXERS {
        let f = lex.join("lexers").join(format!("{name}.cxx"));
        assert!(f.exists(), "Lexilla 中找不到 {}", f.display());
        b.file(f);
    }
    b.file(native.join("hn_lexers.cxx"));
    b.compile("lexilla");

    for lib in [
        "user32", "gdi32", "imm32", "ole32", "oleaut32", "advapi32", "uuid", "msimg32", "shell32",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
}

fn base_build(release: bool) -> cc::Build {
    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++17")
        .warnings(false)
        .flag_if_supported("/utf-8")
        .flag_if_supported("/EHsc")
        .define("_CRT_SECURE_NO_DEPRECATE", None);
    if release {
        b.define("NDEBUG", None);
    }
    b
}

fn cxx_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("读取 {} 失败: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "cxx"))
        .collect();
    v.sort();
    v
}

/// 从 Scintilla.h 与 SciLexer.h 中提取数值宏，生成 `sci_consts.rs`。
///
/// 类型约定：`SCI_*`（消息）、`SCN_*`、`SCEN_*`（通知码）为 `u32`，其余为 `isize`。
fn generate_consts(sci: &Path, lex: &Path) {
    let mut out = String::new();
    let mut seen = HashSet::new();
    for header in [
        sci.join("include/Scintilla.h"),
        lex.join("include/SciLexer.h"),
    ] {
        let text = fs::read_to_string(&header).unwrap();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            if it.next() != Some("#define") {
                continue;
            }
            let (Some(name), Some(value), None) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if !name
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
            {
                continue;
            }
            let Some(v) = parse_int(value) else { continue };
            if !seen.insert(name.to_string()) {
                continue;
            }
            let ty = if name.starts_with("SCI_")
                || name.starts_with("SCN_")
                || name.starts_with("SCEN_")
            {
                if v < 0 || v > u32::MAX as i64 {
                    continue;
                }
                "u32"
            } else {
                "isize"
            };
            writeln!(out, "pub const {name}: {ty} = {v};").unwrap();
        }
    }
    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("sci_consts.rs");
    fs::write(dest, out).unwrap();
}

fn parse_int(s: &str) -> Option<i64> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let v = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).ok()?
    } else if body.bytes().all(|c| c.is_ascii_digit()) && !body.is_empty() {
        body.parse::<i64>().ok()?
    } else {
        return None;
    };
    Some(if neg { -v } else { v })
}
