//! 设置与最近文件的持久化。
//!
//! 位置：`%APPDATA%\HurshNote\`；exe 同目录存在 `portable.flag` 时改为 exe 同目录的 `data\`。
//! 格式为简单的 `键=值` 文本（M3 换成 TOML）。

use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 15;

#[derive(Debug, Clone)]
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
    /// 窗口正常位置 (left, top, right, bottom) 与是否最大化
    pub window: Option<(i32, i32, i32, i32)>,
    pub maximized: bool,
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
        }
    }
}

pub const ZOOM_MIN: u32 = 30;
pub const ZOOM_MAX: u32 = 500;

pub fn data_dir() -> PathBuf {
    // 测试用：指定独立的数据目录
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

use hn_core::atomic::write_atomic;

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

pub fn load_settings() -> Settings {
    let mut s = Settings::default();
    let Ok(text) = std::fs::read_to_string(data_dir().join("settings.ini")) else {
        return s;
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "wrap" => s.wrap = parse_bool(v).unwrap_or(s.wrap),
            "show_whitespace" => s.show_whitespace = parse_bool(v).unwrap_or(s.show_whitespace),
            "show_eol" => s.show_eol = parse_bool(v).unwrap_or(s.show_eol),
            "indent_guides" => s.indent_guides = parse_bool(v).unwrap_or(s.indent_guides),
            "line_numbers" => s.line_numbers = parse_bool(v).unwrap_or(s.line_numbers),
            "list_continue" => s.list_continue = parse_bool(v).unwrap_or(s.list_continue),
            "snapshots" => s.snapshots = parse_bool(v).unwrap_or(s.snapshots),
            "auto_reload" => s.auto_reload = parse_bool(v).unwrap_or(s.auto_reload),
            "zoom" => {
                if let Ok(z) = v.parse::<u32>() {
                    s.zoom = z.clamp(ZOOM_MIN, ZOOM_MAX);
                }
            }
            "font_face" if !v.is_empty() => s.font_face = v.to_string(),
            "font_size_x100" => {
                if let Ok(z) = v.parse::<i32>() {
                    s.font_size_x100 = z.clamp(600, 7200);
                }
            }
            "window" => {
                let n: Vec<i32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if n.len() == 4 && n[2] > n[0] + 100 && n[3] > n[1] + 100 {
                    s.window = Some((n[0], n[1], n[2], n[3]));
                }
            }
            "maximized" => s.maximized = parse_bool(v).unwrap_or(false),
            _ => {}
        }
    }
    s
}

pub fn save_settings(s: &Settings) {
    let b = |v: bool| if v { "1" } else { "0" };
    let mut out = format!(
        "wrap={}\nshow_whitespace={}\nshow_eol={}\nindent_guides={}\nline_numbers={}\nlist_continue={}\nzoom={}\nfont_face={}\nfont_size_x100={}\nmaximized={}\n",
        b(s.wrap),
        b(s.show_whitespace),
        b(s.show_eol),
        b(s.indent_guides),
        b(s.line_numbers),
        b(s.list_continue),
        s.zoom,
        s.font_face,
        s.font_size_x100,
        b(s.maximized),
    );
    out.push_str(&format!(
        "snapshots={}\nauto_reload={}\n",
        b(s.snapshots),
        b(s.auto_reload)
    ));
    if let Some((l, t, r, bm)) = s.window {
        out.push_str(&format!("window={l},{t},{r},{bm}\n"));
    }
    let _ = write_atomic(&data_dir().join("settings.ini"), out.as_bytes());
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
