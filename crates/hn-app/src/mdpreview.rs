//! Markdown 分栏预览（`app` 的子模块）：何时显示、布局、把源码交给预览窗口、滚动同步、打开链接。
//!
//! 预览窗口本身（排版、绘制、选择）在 `crate::mdview`。
//!
//! - 显示条件：设置中打开了预览，焦点视图（分屏时也看另一视图）的文档语言是 Markdown，且不是大文件视图 / Hex 视图
//! - 预览优先跟随焦点视图中的文档；切换文档或重新载入（文档指针变化）时立即同步解析，编辑时防抖 150 ms 后台解析
//! - 滚动同步：编辑器滚动 → 预览按源码行定位；用户滚动预览 → 编辑器跟随。用户滚动预览后的 200 ms 内
//!   忽略编辑器发来的滚动通知，避免两边来回拉扯

use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::Shell::ShellExecuteW;

use super::*;
use crate::mdview::{self, StyleSpec};

pub(super) const TIMER_MD: usize = 6;
/// 编辑停止多久后重新解析
const DEBOUNCE_MS: u32 = 150;
/// 用户滚动预览后，多久内不让编辑器的滚动反过来带动预览
const GUARD_MS: u64 = 200;

impl App {
    /// 文档是否可以预览：已载入的 Markdown 文档，不是大文件/Hex 视图，不是长行模式。
    fn md_previewable(&self, id: u32) -> bool {
        if id == 0 || self.overlay_of(id).is_some() {
            return false;
        }
        self.docs
            .try_borrow()
            .ok()
            .and_then(|docs| {
                docs.iter()
                    .find(|d| d.id == id)
                    .map(|d| d.ptr != 0 && !d.long_lines && LANGS[d.lang].id == "markdown")
            })
            .unwrap_or(false)
    }

    /// 应当预览的文档：优先焦点视图中的文档；分屏时焦点视图不是 Markdown 而另一视图是，就预览另一视图的
    /// （在两个视图之间点来点去时预览不会忽隐忽现）。
    fn md_wanted(&self) -> Option<u32> {
        if !self
            .settings
            .try_borrow()
            .map(|s| s.md_preview)
            .unwrap_or(false)
        {
            return None;
        }
        let focus = self.focus.get();
        let mut views = vec![focus];
        if self.split.get() != 0 {
            views.push(1 - focus);
        }
        views
            .into_iter()
            .map(|i| self.view_doc[i].get())
            .find(|&id| self.md_previewable(id))
    }

    /// 显示文档 `id` 的编辑视图（优先焦点视图）。
    fn md_view_of(&self, id: u32) -> Option<Sci> {
        let focus = self.focus.get();
        let mut order = vec![focus];
        if self.split.get() != 0 {
            order.push(1 - focus);
        }
        order
            .into_iter()
            .find(|&i| self.view_doc[i].get() == id)
            .and_then(|i| self.views[i].get())
    }

    /// 按当前文档显示或隐藏预览；文档变化时立即同步内容。由 `refresh` 调用。
    pub(super) fn sync_preview(&self) {
        let Some(v) = mdview::view() else { return };
        let want = self.md_wanted();
        let visible = want.is_some();
        if visible != self.md_visible.get() {
            self.md_visible.set(visible);
            if !visible {
                let had = unsafe { GetFocus() } == v.hwnd();
                v.show(false);
                v.clear();
                self.md_ptr.set(0);
                unsafe { KillTimer(self.hwnd(), TIMER_MD) };
                if had {
                    if let Some(s) = self.sci() {
                        unsafe { SetFocus(s.hwnd()) };
                    }
                }
            }
            self.layout();
            if visible {
                v.show(true);
            }
        }
        let Some(id) = want else { return };
        let ptr = self.with_doc(id, |d| d.ptr).unwrap_or(0);
        if v.doc_id() != id || self.md_ptr.get() != ptr {
            self.md_ptr.set(ptr);
            self.md_apply_style();
            self.md_push_source(id, true, true);
            self.md_editor_scrolled();
        }
    }

    /// 预览区的布局：返回留给编辑视图的宽度。
    pub(super) fn layout_preview(
        &self,
        left: i32,
        width: i32,
        height: i32,
        splitters: &mut Vec<(Drag, RECT)>,
    ) -> i32 {
        let Some(v) = mdview::view() else {
            return width;
        };
        if !self.md_visible.get() {
            return width;
        }
        let dpi = self.dpi.get() as i32;
        let gap = 5 * dpi / 96;
        let min = 120 * dpi / 96;
        let ratio = self.settings.borrow().md_preview_ratio;
        let pw = (((width - gap) as f32 * ratio) as i32).clamp(
            min.min(width / 2),
            (width - gap - min).max(min.min(width / 2)),
        );
        let x = left + width - pw;
        unsafe { MoveWindow(v.hwnd(), x, 0, pw.max(0), height.max(0), 1) };
        self.md_span.set((left, left + width));
        splitters.push((
            Drag::Preview,
            RECT {
                left: x - gap,
                top: 0,
                right: x,
                bottom: height,
            },
        ));
        (width - pw - gap).max(0)
    }

    /// 拖动预览分隔条。
    pub(super) fn drag_preview(&self, x: i32) {
        let (a, b) = self.md_span.get();
        if b > a {
            let r = ((b - x) as f32 / (b - a) as f32).clamp(0.15, 0.85);
            self.settings.borrow_mut().md_preview_ratio = r;
        }
    }

    /// 打开/关闭预览。
    pub(super) fn toggle_preview(&self) {
        self.toggle_setting(|s| s.md_preview = !s.md_preview);
        let on = self.settings.borrow().md_preview;
        if on && self.md_wanted().is_none() {
            self.show_message("Markdown 预览已打开；当前文档不是 Markdown（可在“语言”菜单中切换）");
        }
    }

    /// 预览字体随编辑器字体、缩放与 DPI 变化。
    pub(super) fn md_apply_style(&self) {
        let Some(v) = mdview::view() else { return };
        let Ok(s) = self.settings.try_borrow() else {
            return;
        };
        let dpi = self.dpi.get() as f32;
        let zoom = s.zoom as f32 / 100.0;
        let pt = s.font_size_x100 as f32 / 100.0;
        v.set_style(StyleSpec {
            body_px: pt.max(10.5) * 1.1 * zoom * dpi / 72.0,
            code_face: s.font_face.clone(),
            code_px: pt * 0.95 * zoom * dpi / 72.0,
            px_scale: dpi / 96.0 * zoom,
        });
    }

    /// 把文档 `id` 的文本交给预览。`sync` 为真时立即解析（切换文档时避免闪烁）。
    pub(super) fn md_push_source(&self, id: u32, sync: bool, reset_scroll: bool) {
        let (Some(v), Some(sci)) = (mdview::view(), self.md_view_of(id)) else {
            return;
        };
        let base = self
            .with_doc(id, |d| {
                d.path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(Path::to_path_buf)
            })
            .flatten();
        if sci.len() > mdview::MAX_SOURCE {
            v.set_message(id, "文档太大（超过 16 MB），不显示预览");
            return;
        }
        let text = String::from_utf8_lossy(&sci.bytes()).into_owned();
        v.set_source(id, text, base, reset_scroll, sync);
    }

    /// 文档内容变化（SCN_MODIFIED 的插入/删除）：防抖后重新解析。
    pub(super) fn md_on_modified(&self, doc: u32) {
        if self.md_visible.get() && mdview::view().is_some_and(|v| v.doc_id() == doc) {
            unsafe { SetTimer(self.hwnd(), TIMER_MD, DEBOUNCE_MS, None) };
        }
    }

    pub(super) fn md_on_timer(&self) {
        unsafe { KillTimer(self.hwnd(), TIMER_MD) };
        let id = mdview::view().map(|v| v.doc_id()).unwrap_or(0);
        if self.md_visible.get() && self.md_view_of(id).is_some() {
            self.md_push_source(id, false, false);
        }
    }

    /// 显示预览文档的编辑视图滚动后：预览跟随。
    pub(super) fn md_editor_scrolled(&self) {
        if !self.md_visible.get() || unsafe { GetTickCount64() } < self.md_guard.get() {
            return;
        }
        let Some(v) = mdview::view() else { return };
        let Some(sci) = self.md_view_of(v.doc_id()) else {
            return;
        };
        let first = sci.send(SCI_GETFIRSTVISIBLELINE, 0, 0).max(0) as usize;
        let doc_line = sci.send(SCI_DOCLINEFROMVISIBLE, first, 0).max(0) as usize;
        let vis_start = sci.send(SCI_VISIBLEFROMDOCLINE, doc_line, 0).max(0) as usize;
        let wraps = sci.send(SCI_WRAPCOUNT, doc_line, 0).max(1) as f64;
        let frac = (first.saturating_sub(vis_start) as f64 / wraps).min(1.0);
        let on_screen = sci.send(SCI_LINESONSCREEN, 0, 0).max(1) as usize;
        let total = sci.send(SCI_VISIBLEFROMDOCLINE, sci.line_count(), 0).max(0) as usize;
        let at_end = first > 0 && first + on_screen >= total;
        v.scroll_to_line(doc_line as f64 + frac, at_end);
    }

    // ---- 预览窗口的回调 ----

    /// 后台解析的结果已显示：保持与编辑器对齐。
    pub(crate) fn md_doc_ready(&self) {
        self.md_editor_scrolled();
    }

    /// 用户滚动了预览：编辑器跟随。
    pub(crate) fn md_user_scrolled(&self) {
        let Some(v) = mdview::view() else { return };
        let Some(sci) = self.md_view_of(v.doc_id()) else {
            return;
        };
        self.md_guard.set(unsafe { GetTickCount64() } + GUARD_MS);
        let (line, at_end) = v.top_line();
        let target = if at_end {
            sci.send(SCI_VISIBLEFROMDOCLINE, sci.line_count(), 0).max(0) as usize
        } else {
            let doc_line = (line.floor().max(0.0) as usize).min(sci.line_count().saturating_sub(1));
            let vis = sci.send(SCI_VISIBLEFROMDOCLINE, doc_line, 0).max(0) as usize;
            let wraps = sci.send(SCI_WRAPCOUNT, doc_line, 0).max(1) as f64;
            vis + ((line - line.floor()) * wraps) as usize
        };
        sci.send(SCI_SETFIRSTVISIBLELINE, target, 0);
    }

    /// 打开预览中的链接：网页用默认浏览器；本地文件在编辑器中打开（不执行任何程序）。
    pub(crate) fn md_open_link(&self, target: hn_md::LinkTarget, base: Option<PathBuf>) {
        match target {
            hn_md::LinkTarget::Web(url) => {
                let (op, file) = (wide("open"), wide(&url));
                unsafe {
                    ShellExecuteW(self.hwnd(), op.as_ptr(), file.as_ptr(), null(), null(), 1)
                };
            }
            hn_md::LinkTarget::File(p) => {
                let p = PathBuf::from(p.replace('/', "\\"));
                let path = if p.is_absolute() {
                    p
                } else {
                    match base {
                        Some(b) => b.join(p),
                        None => {
                            self.show_message("未保存的文档不能打开相对路径的链接");
                            return;
                        }
                    }
                };
                if path.is_file() {
                    self.open_path(&path, None, false);
                } else if path.is_dir() {
                    self.reveal_in_explorer(&path);
                } else {
                    self.show_message(&format!("找不到文件：{}", path.display()));
                }
            }
            hn_md::LinkTarget::Anchor(_) => {}
            hn_md::LinkTarget::Unsupported => self.show_message(
                "出于安全考虑，不打开这种链接（只支持 http、https、mailto 与本地文件）",
            ),
        }
    }

    pub(crate) fn md_copy(&self, text: &str) {
        self.copy_text(text);
    }

    pub(crate) fn md_escape(&self) {
        if let Some(s) = self.sci() {
            unsafe { SetFocus(s.hwnd()) };
        }
    }

    pub(crate) fn md_ctrl_wheel(&self, delta: i32) {
        self.on_ctrl_wheel(delta);
    }

    pub(crate) fn md_close(&self) {
        if self.settings.borrow().md_preview {
            self.toggle_preview();
        }
    }

    pub(crate) fn md_message(&self, text: &str) {
        self.show_message(text);
    }

    // ---- 自测 ----

    pub(crate) fn st_md_visible(&self) -> bool {
        self.md_visible.get()
    }

    /// 自测：立即把编辑中的文本交给预览（同步解析）。
    pub(crate) fn st_md_push(&self) {
        let id = mdview::view().map(|v| v.doc_id()).unwrap_or(0);
        self.md_push_source(id, true, false);
    }

    pub(crate) fn st_md_editor_scrolled(&self) {
        self.md_guard.set(0);
        self.md_editor_scrolled();
    }
}
