//! Ctrl+P 快速打开（`app` 的子模块）。
//!
//! 候选：已打开的文档、最近文件、工作区中的文件（后台线程用 `ignore` 遍历建立索引）。
//! 输入框内容变化时用 `hn_text::fuzzy` 排序；上下键在输入框中直接移动列表选择。

use std::time::{Duration, Instant};

use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateDialogIndirectParamW, GetWindowRect, IsWindowVisible, SendMessageW, DLGTEMPLATE,
};

use super::*;
use crate::ctl::*;
use crate::dlg::{self, Template};

pub(super) const WM_APP_INDEX: u32 = WM_APP + 7;
const IDC_Q_EDIT: u16 = 201;
const IDC_Q_LIST: u16 = 202;
const IDC_Q_INFO: u16 = 203;
const MAX_INDEX: usize = 200_000;
const MAX_RESULTS: usize = 200;
const INDEX_TTL: Duration = Duration::from_secs(30);
const WM_KEYDOWN: u32 = 0x0100;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_UP: usize = 0x26;
const VK_DOWN: usize = 0x28;

/// 工作区文件索引。
pub(super) struct FileIndex {
    pub roots: Vec<PathBuf>,
    /// 匹配用的键（"根目录名\相对路径"）与完整路径
    pub files: Vec<(String, PathBuf)>,
    pub truncated: bool,
    pub built: Instant,
}

/// 遍历工作区（后台线程）。
pub(super) fn build_index(roots: Vec<PathBuf>, gitignore: bool) -> FileIndex {
    let mut files = Vec::new();
    let mut truncated = false;
    'outer: for root in &roots {
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .parents(gitignore)
            .git_ignore(gitignore)
            .git_exclude(gitignore)
            .git_global(gitignore)
            .ignore(gitignore)
            .require_git(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build();
        for e in walker.flatten() {
            if !e.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = e.into_path();
            let rel = path.strip_prefix(root).unwrap_or(&path);
            files.push((format!("{root_name}\\{}", rel.display()), path));
            if files.len() >= MAX_INDEX {
                truncated = true;
                break 'outer;
            }
        }
    }
    FileIndex {
        roots,
        files,
        truncated,
        built: Instant::now(),
    }
}

fn template() -> Vec<u32> {
    Template::new(
        "快速打开",
        WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME,
        340,
        214,
    )
    .edit(IDC_Q_EDIT, 7, 7, 326, 13, 0)
    .item(
        CLS_LISTBOX,
        "",
        IDC_Q_LIST,
        7,
        24,
        326,
        170,
        WS_BORDER | WS_VSCROLL | WS_TABSTOP | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT,
    )
    .label("", IDC_Q_INFO, 7, 199, 326, 9)
    .build()
}

unsafe extern "system" fn quick_proc(hwnd: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> isize {
    let r = crate::crash::guard(|| {
        let app = app()?;
        match msg {
            WM_INITDIALOG => Some(1),
            WM_COMMAND => {
                let id = loword(wp);
                let code = hiword(wp);
                match (id, code) {
                    (IDC_Q_EDIT, EN_CHANGE) => app.quick_filter(),
                    (IDC_Q_LIST, LBN_DBLCLK) => app.quick_commit(),
                    (x, _) if x == IDOK as u16 => app.quick_commit(),
                    (x, _) if x == IDCANCEL as u16 => app.quick_hide(),
                    _ => return Some(0),
                }
                Some(1)
            }
            WM_CLOSE => {
                app.quick_hide();
                Some(1)
            }
            _ => None,
        }
    });
    let _ = hwnd;
    match r {
        Some(Some(v)) => v,
        _ => 0,
    }
}

impl App {
    pub(super) fn quick_show(&self) {
        let mut dlg_h = self.quick.get();
        if dlg_h.is_null() {
            let t = template();
            dlg_h = unsafe {
                CreateDialogIndirectParamW(
                    self.hinstance,
                    t.as_ptr() as *const DLGTEMPLATE,
                    self.hwnd(),
                    Some(quick_proc),
                    0,
                )
            };
            if dlg_h.is_null() {
                return;
            }
            self.quick.set(dlg_h);
        }
        // 放在主窗口上部居中
        let mut o: RECT = unsafe { std::mem::zeroed() };
        let mut d: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetWindowRect(self.hwnd(), &mut o);
            GetWindowRect(dlg_h, &mut d);
        }
        let w = d.right - d.left;
        unsafe {
            SetWindowPos(
                dlg_h,
                null_mut(),
                o.left + ((o.right - o.left) - w) / 2,
                o.top + 80,
                0,
                0,
                0x0001 | 0x0004,
            );
        }
        self.ensure_index();
        dlg::set_text(dlg_h, IDC_Q_EDIT, "");
        self.quick_filter();
        unsafe {
            ShowWindow(dlg_h, SW_SHOW);
            SetFocus(dlg::item(dlg_h, IDC_Q_EDIT));
        }
    }

    pub(super) fn quick_hide(&self) {
        let h = self.quick.get();
        if !h.is_null() {
            unsafe { ShowWindow(h, SW_HIDE) };
        }
        if let Some(s) = self.sci() {
            unsafe { SetFocus(s.hwnd()) };
        }
    }

    /// 需要时在后台重建索引（过期或工作区变化）。
    pub(super) fn ensure_index(&self) {
        let (roots, gitignore) = {
            let s = self.settings.borrow();
            (s.workspace.clone(), s.respect_gitignore)
        };
        let fresh = self
            .file_index
            .borrow()
            .as_ref()
            .is_some_and(|ix| ix.roots == roots && ix.built.elapsed() < INDEX_TTL);
        if fresh || self.index_building.get() || roots.is_empty() {
            if roots.is_empty() {
                self.file_index.borrow_mut().take();
            }
            return;
        }
        self.index_building.set(true);
        let tx = self.index_tx.clone();
        let hwnd = self.hwnd() as isize;
        std::thread::Builder::new()
            .name("file-index".into())
            .spawn(move || {
                let ix = build_index(roots, gitignore);
                let _ = tx.send(ix);
                unsafe { PostMessageW(hwnd as HWND, WM_APP_INDEX, 0, 0) };
            })
            .ok();
    }

    pub(super) fn on_index_ready(&self) {
        let mut got = false;
        while let Ok(ix) = self.index_rx.try_recv() {
            *self.file_index.borrow_mut() = Some(ix);
            got = true;
        }
        self.index_building.set(false);
        if got {
            let h = self.quick.get();
            if !h.is_null() && unsafe { IsWindowVisible(h) } != 0 {
                self.quick_filter();
            }
        }
    }

    /// 候选列表：已打开文档、最近文件、索引中的文件（按路径去重）。
    fn quick_candidates(&self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |key: String, p: PathBuf, out: &mut Vec<(String, PathBuf)>| {
            if seen.insert(p.to_string_lossy().to_lowercase()) {
                out.push((key, p));
            }
        };
        let open: Vec<PathBuf> = self
            .mru_order()
            .into_iter()
            .filter_map(|id| self.with_doc(id, |d| d.path.clone()).flatten())
            .collect();
        for p in open {
            push(p.display().to_string(), p, &mut out);
        }
        for p in self.recent.borrow().iter() {
            push(p.display().to_string(), p.clone(), &mut out);
        }
        if let Some(ix) = self.file_index.borrow().as_ref() {
            for (k, p) in &ix.files {
                push(k.clone(), p.clone(), &mut out);
            }
        }
        out
    }

    pub(super) fn quick_filter(&self) {
        let h = self.quick.get();
        if h.is_null() {
            return;
        }
        let query = dlg::get_text(h, IDC_Q_EDIT);
        let cands = self.quick_candidates();
        let keys: Vec<&str> = cands.iter().map(|c| c.0.as_str()).collect();
        let ranked: Vec<usize> = if query.trim().is_empty() {
            (0..cands.len().min(MAX_RESULTS)).collect()
        } else {
            hn_text::fuzzy::rank(&query, &keys, MAX_RESULTS)
                .into_iter()
                .map(|(i, _)| i)
                .collect()
        };
        let list = dlg::item(h, IDC_Q_LIST);
        unsafe {
            SendMessageW(list, 0x000B, 0, 0); // WM_SETREDRAW false
            SendMessageW(list, LB_RESETCONTENT, 0, 0);
        }
        let mut paths = Vec::with_capacity(ranked.len());
        for i in ranked {
            let (key, path) = &cands[i];
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let w = wide(&format!("{name}    —  {key}"));
            unsafe { SendMessageW(list, LB_ADDSTRING, 0, w.as_ptr() as isize) };
            paths.push(path.clone());
        }
        unsafe {
            if !paths.is_empty() {
                SendMessageW(list, LB_SETCURSEL, 0, 0);
            }
            SendMessageW(list, 0x000B, 1, 0);
            InvalidateRect(list, null(), 1);
        }
        let info = {
            let ix = self.file_index.borrow();
            match ix.as_ref() {
                _ if self.index_building.get() => "正在建立工作区文件索引…".to_string(),
                Some(ix) if ix.truncated => {
                    format!("工作区文件过多，只索引了前 {} 个", ix.files.len())
                }
                Some(ix) => format!(
                    "{} 个匹配 · 工作区共 {} 个文件",
                    paths.len(),
                    ix.files.len()
                ),
                None if self.settings.borrow().workspace.is_empty() => {
                    "提示：在侧边栏“工作区”中添加文件夹后可搜索其中的文件".to_string()
                }
                None => String::new(),
            }
        };
        dlg::set_text(h, IDC_Q_INFO, &info);
        *self.quick_paths.borrow_mut() = paths;
    }

    pub(super) fn quick_commit(&self) {
        let h = self.quick.get();
        if h.is_null() {
            return;
        }
        let list = dlg::item(h, IDC_Q_LIST);
        let sel = unsafe { SendMessageW(list, LB_GETCURSEL, 0, 0) };
        let path = usize::try_from(sel)
            .ok()
            .and_then(|i| self.quick_paths.borrow().get(i).cloned());
        self.quick_hide();
        if let Some(p) = path {
            if p.exists() {
                self.open_path(&p, None, false);
            } else {
                self.show_message(&format!("文件不存在：{}", p.display()));
            }
        }
    }

    /// 输入框中的上下键、翻页键移动列表选择。
    pub(super) fn quick_pretranslate(&self, msg: &MSG) -> bool {
        let h = self.quick.get();
        if h.is_null() || msg.message != WM_KEYDOWN || msg.hwnd != dlg::item(h, IDC_Q_EDIT) {
            return false;
        }
        let delta: isize = match msg.wParam {
            VK_UP => -1,
            VK_DOWN => 1,
            VK_PRIOR => -10,
            VK_NEXT => 10,
            _ => return false,
        };
        let list = dlg::item(h, IDC_Q_LIST);
        let n = unsafe { SendMessageW(list, LB_GETCOUNT, 0, 0) };
        if n > 0 {
            let cur = unsafe { SendMessageW(list, LB_GETCURSEL, 0, 0) }.max(0);
            let next = (cur + delta).clamp(0, n - 1);
            unsafe { SendMessageW(list, LB_SETCURSEL, next as usize, 0) };
        }
        true
    }

    pub(super) fn quick_dialog(&self) -> HWND {
        self.quick.get()
    }

    // 自测
    pub(crate) fn st_quick_query(&self, query: &str) -> Vec<PathBuf> {
        self.quick_show();
        let h = self.quick.get();
        dlg::set_text(h, IDC_Q_EDIT, query);
        self.quick_filter();
        let r = self.quick_paths.borrow().clone();
        self.quick_hide();
        r
    }
}
