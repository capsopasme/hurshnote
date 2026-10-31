//! 语言定义与识别（按扩展名、文件名、首行）。
//!
//! 这里只描述"是什么语言、用哪个 lexer"；关键字与配色在 hn-sci 中按 `id` 配置。

use std::path::Path;

#[derive(Debug)]
pub struct Lang {
    /// 稳定标识，用于配置与 hn-sci 中的样式表。
    pub id: &'static str,
    /// 菜单与状态栏显示名。
    pub name: &'static str,
    /// Lexilla lexer 名称。
    pub lexer: &'static str,
    /// 扩展名（小写，不含点）。
    pub exts: &'static [&'static str],
    /// 完整文件名（小写）。
    pub names: &'static [&'static str],
}

macro_rules! lang {
    ($id:expr, $name:expr, $lexer:expr, [$($e:expr),*], [$($n:expr),*]) => {
        Lang { id: $id, name: $name, lexer: $lexer, exts: &[$($e),*], names: &[$($n),*] }
    };
}

/// 所有语言。第一项是纯文本（默认）。
pub static LANGS: &[Lang] = &[
    lang!("text", "纯文本", "null", ["txt", "log", "text"], []),
    lang!(
        "markdown",
        "Markdown",
        "markdown",
        ["md", "markdown", "mdown", "mkd", "mkdn"],
        []
    ),
    lang!(
        "json",
        "JSON",
        "json",
        [
            "json",
            "jsonc",
            "json5",
            "geojson",
            "har",
            "webmanifest",
            "ipynb"
        ],
        [".babelrc", ".eslintrc", "composer.lock"]
    ),
    lang!(
        "xml",
        "XML",
        "xml",
        [
            "xml", "xsd", "xsl", "xslt", "svg", "csproj", "vcxproj", "vbproj", "fsproj", "props",
            "targets", "config", "manifest", "plist", "resx", "xaml", "nuspec", "rss", "atom",
            "kml", "gpx", "wsdl", "xul"
        ],
        []
    ),
    lang!(
        "html",
        "HTML",
        "hypertext",
        ["html", "htm", "xhtml", "shtml", "hta"],
        []
    ),
    lang!(
        "python",
        "Python",
        "python",
        ["py", "pyw", "pyi", "pyx"],
        ["sconstruct", "sconscript"]
    ),
    lang!(
        "c",
        "C / C++",
        "cpp",
        [
            "c", "h", "cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++", "inl", "ino", "cu",
            "cuh", "m", "mm"
        ],
        []
    ),
    lang!("csharp", "C#", "cpp", ["cs", "csx"], []),
    lang!("java", "Java", "cpp", ["java", "kt", "kts", "gradle"], []),
    lang!(
        "javascript",
        "JavaScript",
        "cpp",
        ["js", "mjs", "cjs", "jsx"],
        []
    ),
    lang!(
        "typescript",
        "TypeScript",
        "cpp",
        ["ts", "tsx", "mts", "cts"],
        []
    ),
    lang!("css", "CSS", "css", ["css", "scss", "less"], []),
    lang!("sql", "SQL", "sql", ["sql", "ddl", "dml"], []),
    lang!(
        "yaml",
        "YAML",
        "yaml",
        ["yml", "yaml"],
        [".clang-format", ".clang-tidy"]
    ),
    lang!(
        "toml",
        "TOML",
        "toml",
        ["toml"],
        ["cargo.lock", "pipfile", "poetry.lock"]
    ),
    lang!(
        "ini",
        "INI / Properties",
        "props",
        [
            "ini",
            "inf",
            "cfg",
            "conf",
            "properties",
            "reg",
            "editorconfig",
            "url",
            "desktop"
        ],
        [".editorconfig", ".gitconfig", ".npmrc"]
    ),
    lang!("batch", "批处理", "batch", ["bat", "cmd", "nt"], []),
    lang!(
        "powershell",
        "PowerShell",
        "powershell",
        ["ps1", "psm1", "psd1"],
        []
    ),
    lang!(
        "bash",
        "Shell",
        "bash",
        ["sh", "bash", "zsh", "ksh"],
        [".bashrc", ".bash_profile", ".zshrc", ".profile"]
    ),
    lang!("rust", "Rust", "rust", ["rs"], []),
    lang!("diff", "Diff", "diff", ["diff", "patch"], []),
];

pub fn text() -> &'static Lang {
    &LANGS[0]
}

pub fn by_id(id: &str) -> Option<&'static Lang> {
    LANGS.iter().find(|l| l.id == id)
}

/// 在 [`LANGS`] 中的下标。
pub fn index_of(lang: &Lang) -> usize {
    LANGS
        .iter()
        .position(|l| std::ptr::eq(l, lang))
        .unwrap_or(0)
}

/// 识别语言：文件名 → 扩展名 → 首行内容。`head` 为文件开头的一段文本。
pub fn detect(path: Option<&Path>, head: &str) -> &'static Lang {
    if let Some(p) = path {
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            let lower = name.to_ascii_lowercase();
            if let Some(l) = LANGS.iter().find(|l| l.names.contains(&lower.as_str())) {
                return l;
            }
            if let Some(ext) = Path::new(&lower).extension().and_then(|e| e.to_str()) {
                if let Some(l) = LANGS.iter().find(|l| l.exts.contains(&ext)) {
                    return l;
                }
            }
        }
    }
    detect_from_content(head).unwrap_or_else(text)
}

fn detect_from_content(head: &str) -> Option<&'static Lang> {
    let first = head
        .trim_start_matches('\u{FEFF}')
        .lines()
        .next()
        .unwrap_or("")
        .trim();
    if let Some(shebang) = first.strip_prefix("#!") {
        let id = if shebang.contains("python") {
            "python"
        } else if shebang.contains("pwsh") || shebang.contains("powershell") {
            "powershell"
        } else if shebang.contains("node") || shebang.contains("deno") {
            "javascript"
        } else if shebang.contains("sh") {
            "bash"
        } else {
            return None;
        };
        return by_id(id);
    }
    let lower = first.to_ascii_lowercase();
    if lower.starts_with("<?xml") {
        return by_id("xml");
    }
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
        return by_id("html");
    }
    if lower.starts_with("diff --git") || lower.starts_with("--- ") || lower.starts_with("index: ")
    {
        return by_id("diff");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(p: &str, head: &str) -> &'static str {
        detect(Some(Path::new(p)), head).id
    }

    #[test]
    fn by_extension() {
        assert_eq!(id("a.PY", ""), "python");
        assert_eq!(id(r"C:\x\main.rs", ""), "rust");
        assert_eq!(id("x.json", ""), "json");
        assert_eq!(id("y.yml", ""), "yaml");
        assert_eq!(id("notes.md", ""), "markdown");
        assert_eq!(id("a.hpp", ""), "c");
        assert_eq!(id("unknown.zzz", ""), "text");
    }

    #[test]
    fn by_name() {
        assert_eq!(id("Cargo.lock", ""), "toml");
        assert_eq!(id(".editorconfig", ""), "ini");
    }

    #[test]
    fn by_content() {
        assert_eq!(id("script", "#!/usr/bin/env python3\nprint(1)"), "python");
        assert_eq!(id("run", "#!/bin/bash\n"), "bash");
        assert_eq!(id("data", "<?xml version=\"1.0\"?>"), "xml");
        assert_eq!(detect(None, "<!DOCTYPE html>").id, "html");
        assert_eq!(detect(None, "hello").id, "text");
    }

    #[test]
    fn ids_unique_and_index() {
        for (i, l) in LANGS.iter().enumerate() {
            assert_eq!(index_of(l), i);
            assert_eq!(LANGS.iter().filter(|o| o.id == l.id).count(), 1);
        }
    }
}
