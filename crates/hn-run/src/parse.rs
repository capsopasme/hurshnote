//! 从输出行中找出源码位置（双击跳转）。
//!
//! 支持 Python Traceback 的 `File "x.py", line 12`，以及常见的 `x.py:12:` / `x.py(12)` 形式。

use std::path::{Path, PathBuf};

/// 一个源码位置：文件与行号（1 起始）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: usize,
}

fn digits(s: &str) -> Option<(usize, usize)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    s[..n].parse().ok().map(|v| (v, n))
}

/// 解析一行输出。相对路径以 `base`（脚本所在目录）为基准。
pub fn location(line: &str, base: Option<&Path>) -> Option<Location> {
    let resolve = |p: &str| -> Option<PathBuf> {
        let p = p.trim();
        if p.is_empty() || p.starts_with('<') {
            return None; // <stdin>、<frozen importlib._bootstrap> 之类
        }
        let pb = PathBuf::from(p);
        Some(match base {
            Some(b) if pb.is_relative() => b.join(pb),
            _ => pb,
        })
    };
    // 1. Python：File "path", line N
    if let Some(i) = line.find("File \"") {
        let rest = &line[i + 6..];
        let q = rest.find('"')?;
        let path = &rest[..q];
        let after = &rest[q + 1..];
        let li = after.find("line ")?;
        let (n, _) = digits(&after[li + 5..])?;
        return Some(Location {
            path: resolve(path)?,
            line: n,
        });
    }
    // 2. path:N: 或 path(N)：从文件扩展名之后找
    let t = line.trim_start();
    for (open, close) in [(':', ':'), ('(', ')')] {
        // 跳过盘符中的冒号（C:\...）
        let start = if t.len() > 2 && t.as_bytes()[1] == b':' {
            2
        } else {
            0
        };
        let mut search = start;
        while let Some(k) = t[search..].find(open) {
            let k = search + k;
            if let Some((n, len)) = digits(&t[k + 1..]) {
                let end = k + 1 + len;
                let rest = &t[end..];
                let ok = rest.starts_with(close)
                    || (open == ':' && rest.is_empty())
                    || (open == '(' && rest.starts_with(','));
                let path = &t[..k];
                if ok && Path::new(path).extension().is_some() && !path.contains(' ') {
                    return Some(Location {
                        path: resolve(path)?,
                        line: n,
                    });
                }
            }
            search = k + 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_traceback() {
        let l = location(r#"  File "C:\work\demo.py", line 12, in <module>"#, None).unwrap();
        assert_eq!(l.path, PathBuf::from(r"C:\work\demo.py"));
        assert_eq!(l.line, 12);
        let l = location(r#"  File "pkg/mod.py", line 3"#, Some(Path::new("/base"))).unwrap();
        assert_eq!(l.path, Path::new("/base").join("pkg/mod.py"));
        assert!(location(r#"  File "<stdin>", line 1, in <module>"#, None).is_none());
    }

    #[test]
    fn compiler_style() {
        let l = location(r"C:\src\main.rs:42:7: error", None).unwrap();
        assert_eq!(l.path, PathBuf::from(r"C:\src\main.rs"));
        assert_eq!(l.line, 42);
        let l = location("test_x.py:5: AssertionError", Some(Path::new("/b"))).unwrap();
        assert_eq!(l.line, 5);
        let l = location(r"D:\a\b.cs(17,3): warning", None).unwrap();
        assert_eq!(l.line, 17);
        assert!(location("Traceback (most recent call last):", None).is_none());
        assert!(location("time: 12:30:00", None).is_none());
    }
}
