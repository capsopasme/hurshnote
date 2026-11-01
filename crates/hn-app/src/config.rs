//! 设置与最近文件的持久化。
//!
//! 位置：`%APPDATA%\HurshNote\`；exe 同目录存在 `portable.flag` 时改为 exe 同目录的 `data\`（便携模式）；
//! 测试时可用环境变量 `HURSHNOTE_DATA_DIR` 指定。
//! 设置保存为 `settings.toml`；首次运行新版本时自动迁移旧的 `settings.ini`。

use std::path::{Path, PathBuf};

use hn_core::atomic::write_atomic;
use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 15;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub wrap: bool,
    pub show_whitespace: bool,
    pub show_eol: bool,
    pub indent_guides: bool,
    pub line_numbers: bool,
    pub list_continue: bool,
    /// 会话快照：关闭时不询问保存，重启后恢复未保存的修改
    pub snapshots: bool,
    /// 未修改的文档被外部修改时自动重新载入
    pub auto_reload: bool,
    /// 缩放百分比
    pub zoom: u32,
    pub font_face: String,
    /// 1/100 磅
    pub font_size_x100: i32,
    /// 窗口正常位置 (left, top, right, bottom)
    pub window: Option<(i32, i32, i32, i32)>,
    pub maximized: bool,
    /// 侧边栏
    pub sidebar_visible: bool,
    /// 侧边栏宽度（DIP）
    pub sidebar_width: i32,
    /// "打开的文件"占侧边栏高度的比例
    pub sidebar_split: f32,
    /// 工作区根目录
    pub workspace: Vec<PathBuf>,
    /// 工作区与快速打开遵守 .gitignore
    pub respect_gitignore: bool,
    /// 底部抽屉高度（DIP，含页签条）
    pub drawer_height: i32,
    /// Python 解释器路径；空为自动探测
    pub python: String,
    /// 新建终端使用的 shell：powershell / pwsh / cmd，或可执行文件的完整路径
    pub terminal_shell: String,
    /// 目录搜索：文件类型与排除项（逗号或分号分隔的通配符）
    pub dir_include: String,
    pub dir_exclude: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            wrap: false,
            show_whitespace: false,
            show_eol: false,
            indent_guides: true,
            line_numbers: true,
            list_continue: true,
            snapshots: true,
            auto_reload: true,
            zoom: 100,
            font_face: "Consolas".into(),
            font_size_x100: 1100,
            window: None,
            maximized: false,
            sidebar_visible: true,
            sidebar_width: 240,
            sidebar_split: 0.35,
            workspace: Vec::new(),
            respect_gitignore: true,
            drawer_height: 220,
            python: String::new(),
            terminal_shell: "powershell".into(),
            dir_include: String::new(),
            dir_exclude: "node_modules".into(),
        }
    }
}

pub const ZOOM_MIN: u32 = 30;
pub const ZOOM_MAX: u32 = 500;

pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("HURSHNOTE_DATA_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("portable.flag").exists() {
                return dir.join("data");
            }
        }
    }
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("HurshNote")
}

/// 是否为便携模式。
pub fn is_portable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("portable.flag").exists()))
        .unwrap_or(false)
}

fn sanitize(mut s: Settings) -> Settings {
    s.zoom = s.zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    s.font_size_x100 = s.font_size_x100.clamp(600, 7200);
    s.sidebar_width = s.sidebar_width.clamp(120, 800);
    if !(0.1..=0.9).contains(&s.sidebar_split) {
        s.sidebar_split = 0.35;
    }
    s.drawer_height = s.drawer_height.clamp(80, 2000);
    if s.terminal_shell.trim().is_empty() {
        s.terminal_shell = "powershell".into();
    }
    if s.font_face.trim().is_empty() {
        s.font_face = "Consolas".into();
    }
    if let Some((l, t, r, b)) = s.window {
        if r < l + 100 || b < t + 100 {
            s.window = None;
        }
    }
    s
}

pub fn load_settings() -> Settings {
    let dir = data_dir();
    if let Ok(text) = std::fs::read_to_string(dir.join("settings.toml")) {
        if let Ok(s) = toml::from_str::<Settings>(&text) {
            return sanitize(s);
        }
    }
    // 迁移旧格式
    if let Ok(text) = std::fs::read_to_string(dir.join("settings.ini")) {
        let s = sanitize(parse_ini(&text));
        save_settings(&s);
        let _ = std::fs::remove_file(dir.join("settings.ini"));
        return s;
    }
    Settings::default()
}

pub fn save_settings(s: &Settings) {
    if let Ok(text) = toml::to_string_pretty(s) {
        let body = format!("# HurshNote 设置（程序会自动改写此文件）\n\n{text}");
        let _ = write_atomic(&data_dir().join("settings.toml"), body.as_bytes());
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// M1/M2 使用的 `键=值` 格式。
fn parse_ini(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        let b = parse_bool(v);
        match k {
            "wrap" => s.wrap = b.unwrap_or(s.wrap),
            "show_whitespace" => s.show_whitespace = b.unwrap_or(s.show_whitespace),
            "show_eol" => s.show_eol = b.unwrap_or(s.show_eol),
            "indent_guides" => s.indent_guides = b.unwrap_or(s.indent_guides),
            "line_numbers" => s.line_numbers = b.unwrap_or(s.line_numbers),
            "list_continue" => s.list_continue = b.unwrap_or(s.list_continue),
            "snapshots" => s.snapshots = b.unwrap_or(s.snapshots),
            "auto_reload" => s.auto_reload = b.unwrap_or(s.auto_reload),
            "maximized" => s.maximized = b.unwrap_or(false),
            "zoom" => s.zoom = v.parse().unwrap_or(s.zoom),
            "font_face" if !v.is_empty() => s.font_face = v.to_string(),
            "font_size_x100" => s.font_size_x100 = v.parse().unwrap_or(s.font_size_x100),
            "window" => {
                let n: Vec<i32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if n.len() == 4 {
                    s.window = Some((n[0], n[1], n[2], n[3]));
                }
            }
            _ => {}
        }
    }
    s
}

pub fn load_recent() -> Vec<PathBuf> {
    std::fs::read_to_string(data_dir().join("recent.txt"))
        .map(|t| {
            t.lines()
                .filter(|l| !l.trim().is_empty())
                .map(PathBuf::from)
                .take(MAX_RECENT)
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_recent(list: &[PathBuf]) {
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = write_atomic(&data_dir().join("recent.txt"), text.as_bytes());
}

/// 把路径放到最近列表最前面（去重，限制长度）。
pub fn push_recent(list: &mut Vec<PathBuf>, path: &Path) {
    list.retain(|p| !same_path(p, path));
    list.insert(0, path.to_path_buf());
    list.truncate(MAX_RECENT);
}

/// Windows 路径比较（不区分大小写）。
pub fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip() {
        let s = Settings {
            window: Some((1, 2, 300, 400)),
            workspace: vec![PathBuf::from(r"C:\项目")],
            ..Default::default()
        };
        let t = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&t).unwrap();
        assert_eq!(back.window, s.window);
        assert_eq!(back.workspace, s.workspace);
    }

    #[test]
    fn missing_fields_default() {
        let s: Settings = toml::from_str("zoom = 150\n").unwrap();
        assert_eq!(s.zoom, 150);
        assert!(s.sidebar_visible);
    }

    #[test]
    fn ini_migration() {
        let s = parse_ini("wrap=1\nzoom=120\nwindow=10,20,500,600\n");
        assert!(s.wrap);
        assert_eq!(s.zoom, 120);
        assert_eq!(s.window, Some((10, 20, 500, 600)));
    }
}
