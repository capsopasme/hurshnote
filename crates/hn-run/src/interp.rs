//! Python 解释器探测。
//!
//! 顺序：设置中指定 → 脚本所在目录及工作区根目录下的 `.venv` / `venv` → `py -3` 启动器 →
//! PATH 中的 `python`（排除 `WindowsApps` 下的商店占位程序，运行它会弹出商店）。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// 找到的解释器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpreter {
    pub program: PathBuf,
    /// 放在脚本参数之前的参数（如 `py` 的 `-3`）
    pub args: Vec<String>,
    /// 显示用说明
    pub label: String,
}

#[cfg(windows)]
const VENV_PYTHON: &[&str] = &["Scripts", "python.exe"];
#[cfg(not(windows))]
const VENV_PYTHON: &[&str] = &["bin", "python"];
#[cfg(windows)]
const PATH_NAMES: &[&str] = &["python.exe"];
#[cfg(not(windows))]
const PATH_NAMES: &[&str] = &["python3", "python"];

/// 是否为 Windows 商店的占位程序（`...\Microsoft\WindowsApps\python.exe`）。
pub fn is_store_stub(p: &Path) -> bool {
    p.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("WindowsApps")
    })
}

/// 探测时需要的环境信息（便于测试时替换）。
pub struct Env<'a> {
    /// 设置中指定的解释器
    pub configured: Option<&'a Path>,
    /// 依次检查虚拟环境的目录（脚本所在目录、工作区根目录……）
    pub dirs: &'a [PathBuf],
    /// PATH 环境变量
    pub path_var: Option<&'a OsStr>,
    /// `py.exe` 所在目录（通常是 `C:\Windows`）
    pub windir: Option<&'a Path>,
    /// 文件是否存在
    pub exists: &'a dyn Fn(&Path) -> bool,
}

/// 按优先级列出所有找到的解释器。
pub fn candidates(env: &Env) -> Vec<Interpreter> {
    let mut out = Vec::new();
    if let Some(p) = env.configured {
        if (env.exists)(p) {
            out.push(Interpreter {
                program: p.to_path_buf(),
                args: Vec::new(),
                label: format!("设置中指定的 {}", p.display()),
            });
        }
    }
    for d in env.dirs {
        for venv in [".venv", "venv"] {
            let mut p = d.join(venv);
            for part in VENV_PYTHON {
                p.push(part);
            }
            if (env.exists)(&p) && !out.iter().any(|i: &Interpreter| i.program == p) {
                out.push(Interpreter {
                    label: format!("虚拟环境 {}", d.join(venv).display()),
                    program: p,
                    args: Vec::new(),
                });
            }
        }
    }
    if cfg!(windows) {
        if let Some(w) = env.windir {
            let py = w.join("py.exe");
            if (env.exists)(&py) {
                out.push(Interpreter {
                    program: py,
                    args: vec!["-3".into()],
                    label: "Python 启动器（py -3）".into(),
                });
            }
        }
    }
    if let Some(path) = env.path_var {
        for dir in std::env::split_paths(path) {
            for name in PATH_NAMES {
                let p = dir.join(name);
                if !is_store_stub(&p) && (env.exists)(&p) && !out.iter().any(|i| i.program == p) {
                    out.push(Interpreter {
                        label: format!("PATH 中的 {}", p.display()),
                        program: p,
                        args: Vec::new(),
                    });
                }
            }
        }
    }
    out
}

/// 用真实环境探测，返回第一个可用的解释器。
pub fn find(configured: Option<&Path>, dirs: &[PathBuf]) -> Option<Interpreter> {
    let path = std::env::var_os("PATH");
    let windir = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir"));
    let windir = windir.map(PathBuf::from);
    let exists = |p: &Path| p.is_file();
    candidates(&Env {
        configured,
        dirs,
        path_var: path.as_deref(),
        windir: windir.as_deref(),
        exists: &exists,
    })
    .into_iter()
    .next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn priority_and_store_stub() {
        let mut venv = p("/proj/.venv");
        for part in VENV_PYTHON {
            venv.push(part);
        }
        let stub = p("/Users/x/AppData/Local/Microsoft/WindowsApps").join(PATH_NAMES[0]);
        let real = p("/opt/py").join(PATH_NAMES[0]);
        let files: HashSet<PathBuf> = [venv.clone(), stub.clone(), real.clone(), p("/cfg/python")]
            .into_iter()
            .collect();
        let exists = |x: &Path| files.contains(x);
        let path = std::env::join_paths([stub.parent().unwrap(), real.parent().unwrap()]).unwrap();
        let dirs = vec![p("/proj")];
        let env = Env {
            configured: Some(Path::new("/cfg/python")),
            dirs: &dirs,
            path_var: Some(path.as_os_str()),
            windir: None,
            exists: &exists,
        };
        let c = candidates(&env);
        assert_eq!(c[0].program, p("/cfg/python"));
        assert_eq!(c[1].program, venv);
        assert_eq!(c[2].program, real);
        assert_eq!(c.len(), 3, "商店占位程序必须排除：{c:?}");
        assert!(is_store_stub(&stub));
    }

    #[test]
    fn missing_configured_is_skipped() {
        let exists = |_: &Path| false;
        let env = Env {
            configured: Some(Path::new("/nope")),
            dirs: &[],
            path_var: None,
            windir: None,
            exists: &exists,
        };
        assert!(candidates(&env).is_empty());
    }
}
