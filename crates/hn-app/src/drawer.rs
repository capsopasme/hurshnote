//! 底部抽屉（`app` 的子模块）：页签条 + 页面（搜索结果 / 输出 / 终端……），可拖动高度。
//!
//! - 页签条是 `SysTabControl32`（按钮样式），只占顶部一条；页面窗口放在它下面，互不重叠
//! - 搜索结果是虚拟 ListView，同时承载大文件搜索与目录搜索的结果
//! - 输出面板是只读的 Scintilla（由程序着色），运行 Python 的输出写在这里，双击跳转到源码位置

use std::path::PathBuf;

use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

use super::*;
use crate::ctl::*;
use crate::lview;

pub(super) const ID_RESULTS: usize = 120;
pub(super) const ID_DRAWER_TAB: usize = 121;
pub(super) const ID_DRAWER_INFO: usize = 122;
pub(super) const ID_OUTPUT: usize = 123;
/// 输出面板最多保留的字节数。
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

const TCS_FLATBUTTONS: u32 = 0x0008;
const TCS_BUTTONS: u32 = 0x0100;
const TCS_FOCUSNEVER: u32 = 0x8000;
const TCM_DELETEALLITEMS: u32 = 0x1309;
const TCM_GETCURSEL: u32 = 0x130B;
const TCM_SETCURSEL: u32 = 0x130C;
const TCM_INSERTITEMW: u32 = 0x133E;
const TCN_SELCHANGE: u32 = (-551i32) as u32;
const TCIF_TEXT: u32 = 0x1;
const SS_RIGHT: u32 = 0x2;
const SS_CENTERIMAGE: u32 = 0x200;
const SS_ENDELLIPSIS: u32 = 0x4000;

#[repr(C)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
struct TCITEMW {
    mask: u32,
    dwState: u32,
    dwStateMask: u32,
    pszText: *mut u16,
    cchTextMax: i32,
    iImage: i32,
    lParam: isize,
}

/// 抽屉中的页面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    Results,
    Output,
    Term(u32),
}

/// 搜索结果列表中的一行。
#[derive(Debug, Clone)]
pub(super) enum ResultItem {
    /// 大文件搜索的一处匹配
    Large {
        doc: u32,
        line: Option<u64>,
        start: u64,
        end: u64,
        preview: String,
    },
    /// 目录搜索：文件标题行
    File {
        path: Rc<PathBuf>,
        count: usize,
        enc: String,
    },
    /// 目录搜索：一处匹配（行号 0 起始；列为行内 UTF-8 字节）
    Hit {
        path: Rc<PathBuf>,
        line: u64,
        col: usize,
        col_end: usize,
        text: String,
    },
}

fn create_child(
    parent: HWND,
    hinstance: *mut core::ffi::c_void,
    class: &str,
    style: u32,
    id: usize,
) -> HWND {
    let c = wide(class);
    let t = wide("");
    unsafe {
        CreateWindowExW(
            0,
            c.as_ptr(),
            t.as_ptr(),
            WS_CHILD | style,
            0,
            0,
            0,
            0,
            parent,
            id as *mut core::ffi::c_void,
            hinstance,
            null(),
        )
    }
}

impl App {
    pub(super) fn create_drawer(&self, hwnd: HWND) {
        let tab = create_child(
            hwnd,
            self.hinstance,
            "SysTabControl32",
            TCS_BUTTONS | TCS_FLATBUTTONS | TCS_FOCUSNEVER,
            ID_DRAWER_TAB,
        );
        let info = create_child(
            hwnd,
            self.hinstance,
            "STATIC",
            SS_RIGHT | SS_CENTERIMAGE | SS_ENDELLIPSIS | 0x80, // SS_NOPREFIX
            ID_DRAWER_INFO,
        );
        self.drawer_tab.set(tab);
        self.drawer_info.set(info);

        let list = create_child(
            hwnd,
            self.hinstance,
            "SysListView32",
            WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_OWNERDATA,
            ID_RESULTS,
        );
        self.results.set(list);
        let p = &GRUVBOX_LIGHT;
        let explorer = wide("Explorer");
        unsafe {
            windows_sys::Win32::UI::Controls::SetWindowTheme(list, explorer.as_ptr(), null());
            let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
            SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex, ex as isize);
            for (i, (title, w)) in [("位置", 200), ("内容", 4000)].iter().enumerate() {
                let t = wide(title);
                let mut col: LVCOLUMNW = std::mem::zeroed();
                col.mask = LVCF_WIDTH | 0x4; // LVCF_TEXT
                col.cx = *w * self.dpi.get() as i32 / 96;
                col.pszText = t.as_ptr() as *mut u16;
                SendMessageW(list, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
            }
            SendMessageW(list, LVM_SETBKCOLOR, 0, theme::bgr(p.bg0_s));
            SendMessageW(list, LVM_SETTEXTBKCOLOR, 0, CLR_NONE);
            SendMessageW(list, LVM_SETTEXTCOLOR, 0, theme::bgr(p.fg1));
        }

        if let Some(out) = unsafe { Sci::create(hwnd, self.hinstance, ID_OUTPUT) } {
            theme::init_view(&out, &GRUVBOX_LIGHT, self.dpi.get());
            self.apply_output_style(&out);
            unsafe { ShowWindow(out.hwnd(), SW_HIDE) };
            self.output.set(Some(out));
        }
        *self.drawer_pages.borrow_mut() = vec![Page::Results, Page::Output];
        self.rebuild_tabs();
        self.update_drawer_font();
    }

    fn apply_output_style(&self, out: &Sci) {
        let s = self.settings.borrow().clone();
        let font = theme::FontSpec {
            face: s.font_face.clone(),
            size_x100: s.font_size_x100,
        };
        theme::init_output(out, &GRUVBOX_LIGHT, &font, s.zoom);
    }

    /// DPI 或字体变化后更新页签条字体与输出面板样式。
    pub(super) fn update_drawer_font(&self) {
        let font = self.side_font.get();
        for h in [
            self.drawer_tab.get(),
            self.drawer_info.get(),
            self.results.get(),
        ] {
            if !h.is_null() {
                unsafe { SendMessageW(h, 0x0030, font as usize, 1) }; // WM_SETFONT
            }
        }
        if let Some(out) = self.output.get() {
            theme::init_view(&out, &GRUVBOX_LIGHT, self.dpi.get());
            self.apply_output_style(&out);
        }
        let (face, px) = self.term_font();
        crate::term::set_font_all(&face, px);
    }

    fn page_title(&self, p: Page) -> String {
        match p {
            Page::Results => "搜索结果".into(),
            Page::Output => "输出".into(),
            Page::Term(id) => crate::term::title(id)
                .map(|t| format!("{t} {id}"))
                .unwrap_or_else(|| "终端".into()),
        }
    }

    pub(super) fn rebuild_tabs(&self) {
        let tab = self.drawer_tab.get();
        if tab.is_null() {
            return;
        }
        let pages = self.drawer_pages.borrow().clone();
        unsafe { SendMessageW(tab, TCM_DELETEALLITEMS, 0, 0) };
        for (i, p) in pages.iter().enumerate() {
            let mut t = wide(&format!("  {}  ", self.page_title(*p)));
            let item = TCITEMW {
                mask: TCIF_TEXT,
                dwState: 0,
                dwStateMask: 0,
                pszText: t.as_mut_ptr(),
                cchTextMax: 0,
                iImage: 0,
                lParam: 0,
            };
            unsafe { SendMessageW(tab, TCM_INSERTITEMW, i, &item as *const _ as isize) };
        }
        let cur = self.drawer_page.get().min(pages.len().saturating_sub(1));
        self.drawer_page.set(cur);
        unsafe { SendMessageW(tab, TCM_SETCURSEL, cur, 0) };
    }

    fn page_hwnd(&self, p: Page) -> HWND {
        match p {
            Page::Results => self.results.get(),
            Page::Output => self.output.get().map(|s| s.hwnd()).unwrap_or(null_mut()),
            Page::Term(id) => crate::term::hwnd(id).unwrap_or(null_mut()),
        }
    }

    pub(super) fn current_page(&self) -> Option<Page> {
        self.drawer_pages
            .borrow()
            .get(self.drawer_page.get())
            .copied()
    }

    /// 显示抽屉并切到页面 `p`（不存在时加到末尾）；`focus` 为真时把焦点移过去。
    pub(super) fn show_page(&self, p: Page, focus: bool) {
        let idx = {
            let mut pages = self.drawer_pages.borrow_mut();
            match pages.iter().position(|x| *x == p) {
                Some(i) => i,
                None => {
                    pages.push(p);
                    pages.len() - 1
                }
            }
        };
        let rebuilt = self.drawer_pages.borrow().len()
            != unsafe { SendMessageW(self.drawer_tab.get(), 0x1304, 0, 0) } as usize; // TCM_GETITEMCOUNT
        self.drawer_page.set(idx);
        if rebuilt {
            self.rebuild_tabs();
        } else {
            unsafe { SendMessageW(self.drawer_tab.get(), TCM_SETCURSEL, idx, 0) };
        }
        self.drawer_shown.set(true);
        self.layout();
        if focus {
            let h = self.page_hwnd(p);
            if !h.is_null() {
                unsafe { SetFocus(h) };
            }
        }
    }

    /// 从抽屉中移除页面（关闭终端时）。
    pub(super) fn remove_page(&self, p: Page) {
        let had_focus = {
            let h = self.page_hwnd(p);
            !h.is_null()
                && unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() } == h
        };
        {
            let mut pages = self.drawer_pages.borrow_mut();
            pages.retain(|x| *x != p);
        }
        let n = self.drawer_pages.borrow().len();
        if self.drawer_page.get() >= n {
            self.drawer_page.set(n.saturating_sub(1));
        }
        self.rebuild_tabs();
        self.layout();
        if had_focus {
            self.focus_editor();
        }
    }

    pub(super) fn hide_drawer(&self) {
        if !self.drawer_shown.replace(false) {
            return;
        }
        self.layout();
        self.focus_editor();
    }

    pub(super) fn focus_editor(&self) {
        if let Some(s) = self.sci() {
            unsafe { SetFocus(s.hwnd()) };
        }
    }

    /// F7：显示/隐藏搜索结果。
    pub(super) fn toggle_results(&self) {
        if self.drawer_shown.get() && self.current_page() == Some(Page::Results) {
            self.hide_drawer();
        } else {
            self.show_page(Page::Results, true);
        }
    }

    /// Ctrl+`：显示/隐藏抽屉；打开时如果没有终端就新建一个。
    pub(super) fn toggle_drawer(&self) {
        if self.drawer_shown.get() {
            self.hide_drawer();
            return;
        }
        let term = self
            .drawer_pages
            .borrow()
            .iter()
            .find(|p| matches!(p, Page::Term(_)))
            .copied();
        match term {
            Some(t) => self.show_page(t, true),
            None => {
                self.new_terminal(None);
            }
        }
    }

    pub(super) fn on_tab_notify(&self, lp: LPARAM) -> Option<LRESULT> {
        let hdr = unsafe { &*(lp as *const NMHDR) };
        if hdr.code == TCN_SELCHANGE {
            let i = unsafe { SendMessageW(self.drawer_tab.get(), TCM_GETCURSEL, 0, 0) };
            if let Some(p) = usize::try_from(i)
                .ok()
                .and_then(|i| self.drawer_pages.borrow().get(i).copied())
            {
                self.show_page(p, true);
            }
            return Some(0);
        }
        None
    }

    /// 抽屉右侧的状态文字（搜索进度、运行状态）。
    pub(super) fn set_drawer_info(&self, text: &str) {
        let h = self.drawer_info.get();
        if !h.is_null() {
            let w = wide(text);
            unsafe { SetWindowTextW(h, w.as_ptr()) };
        }
    }

    /// 摆放抽屉，返回编辑区剩下的高度。
    pub(super) fn layout_drawer(
        &self,
        left: i32,
        width: i32,
        height: i32,
        splitters: &mut Vec<(Drag, RECT)>,
    ) -> i32 {
        let pages = self.drawer_pages.borrow().clone();
        let tab = self.drawer_tab.get();
        let info = self.drawer_info.get();
        let cur = self.current_page();
        let shown = self.drawer_shown.get();
        for p in &pages {
            let h = self.page_hwnd(*p);
            if !h.is_null() && (!shown || Some(*p) != cur) {
                unsafe { ShowWindow(h, SW_HIDE) };
            }
        }
        if !shown || tab.is_null() {
            unsafe {
                ShowWindow(tab, SW_HIDE);
                ShowWindow(info, SW_HIDE);
            }
            return height;
        }
        let dpi = self.dpi.get() as i32;
        let px = |v: i32| v * dpi / 96;
        let gap = px(5);
        let tab_h = px(26);
        let want = px(self.settings.borrow().drawer_height);
        let dh = want.clamp(tab_h + px(40), (height - px(80)).max(tab_h + px(40)));
        let top = (height - dh).max(0);
        let info_w = (width / 3).min(px(420));
        unsafe {
            MoveWindow(tab, left, top, width - info_w, tab_h, 1);
            MoveWindow(info, left + width - info_w, top, info_w - px(6), tab_h, 1);
            ShowWindow(tab, SW_SHOW);
            ShowWindow(info, SW_SHOW);
        }
        if let Some(p) = cur {
            let h = self.page_hwnd(p);
            if !h.is_null() {
                unsafe {
                    MoveWindow(h, left, top + tab_h, width, (dh - tab_h).max(0), 1);
                    ShowWindow(h, SW_SHOW);
                }
            }
        }
        splitters.push((
            Drag::Drawer,
            RECT {
                left,
                top: top - gap,
                right: left + width,
                bottom: top,
            },
        ));
        (top - gap).max(0)
    }

    // ---- 搜索结果 ----

    /// 开始一批新结果。`doc` 为大文件搜索的文档，目录搜索为 0。
    pub(super) fn results_begin(&self, doc: u32, gen: u32) {
        self.results_items.borrow_mut().clear();
        self.results_doc.set(doc);
        self.results_gen.set(gen);
        self.results_count_changed();
    }

    pub(super) fn results_push(&self, items: Vec<ResultItem>) {
        self.results_items.borrow_mut().extend(items);
        self.results_count_changed();
    }

    pub(super) fn results_count_changed(&self) {
        let n = self.results_items.borrow().len();
        unsafe {
            SendMessageW(self.results.get(), LVM_SETITEMCOUNT, n, 2); // LVSICF_NOSCROLL
        }
    }

    fn result_texts(item: &ResultItem) -> (String, String) {
        match item {
            ResultItem::Large { line, preview, .. } => (
                line.map(|l| format!("行 {}", l + 1))
                    .unwrap_or_else(|| "—".into()),
                preview.clone(),
            ),
            ResultItem::File { path, count, enc } => (
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                format!(
                    "{}    {count} 处{}",
                    path.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    if enc.is_empty() {
                        String::new()
                    } else {
                        format!("    {enc}")
                    }
                ),
            ),
            ResultItem::Hit { line, text, .. } => (format!("    行 {}", line + 1), text.clone()),
        }
    }

    /// 结果列表的通知。
    pub(super) fn on_results_notify(&self, lp: LPARAM) -> Option<LRESULT> {
        let hdr = unsafe { &*(lp as *const NMHDR) };
        match hdr.code {
            LVN_GETDISPINFOW => {
                let di = unsafe { &mut *(lp as *mut NMLVDISPINFOW) };
                if di.item.mask & LVIF_TEXT == 0 {
                    return Some(0);
                }
                let text = {
                    let items = self.results_items.borrow();
                    let Some(it) = items.get(di.item.iItem as usize) else {
                        return Some(0);
                    };
                    let (a, b) = Self::result_texts(it);
                    if di.item.iSubItem == 0 {
                        a
                    } else {
                        b
                    }
                };
                Self::copy_to_wide_buf(&text, di.item.pszText, di.item.cchTextMax);
                Some(0)
            }
            NM_DBLCLK => {
                let nm = unsafe { &*(lp as *const NMITEMACTIVATE) };
                if nm.iItem >= 0 {
                    self.jump_to_result(nm.iItem as usize);
                }
                Some(0)
            }
            NM_RETURN => {
                let i = unsafe {
                    SendMessageW(
                        self.results.get(),
                        LVM_GETNEXTITEM,
                        usize::MAX,
                        LVNI_SELECTED as isize,
                    )
                };
                if i >= 0 {
                    self.jump_to_result(i as usize);
                }
                Some(0)
            }
            LVN_KEYDOWN => {
                let nm = unsafe { &*(lp as *const NMKEYDOWN) };
                let vk = nm.wVKey;
                if vk == 0x1B {
                    self.hide_drawer();
                }
                Some(0)
            }
            _ => None,
        }
    }

    /// 跳到第 `i` 条结果。
    pub(super) fn jump_to_result(&self, i: usize) {
        let Some(item) = self.results_items.borrow().get(i).cloned() else {
            return;
        };
        match item {
            ResultItem::Large {
                doc, start, end, ..
            } => {
                if self.with_doc(doc, |_| ()).is_none() {
                    self.show_message("搜索结果所属的文档已关闭");
                    return;
                }
                self.activate(doc);
                let Some(rc) = self.overlay_of(doc) else {
                    return;
                };
                let vis = lview::views_showing(&rc)
                    .first()
                    .map(|v| v.visible_rows())
                    .unwrap_or(30);
                rc.borrow_mut().select(start, end, vis);
                lview::reveal(&rc);
            }
            ResultItem::File { path, .. } => self.open_path(&path, None, false),
            ResultItem::Hit {
                path,
                line,
                col,
                col_end,
                ..
            } => {
                self.open_path(&path, None, false);
                if self.active_overlay().is_some() {
                    return;
                }
                if let Some(s) = self.sci() {
                    let line = (line as usize).min(s.line_count().saturating_sub(1));
                    let ls = s.line_start(line);
                    let le = s.line_end(line);
                    let a = (ls + col).min(le);
                    let b = (ls + col_end).min(le);
                    s.select_and_show(a, b);
                }
            }
        }
        self.focus_editor();
        self.update_status();
    }

    /// 关闭文档时清理属于它的大文件搜索结果。
    pub(super) fn drop_results_of(&self, id: u32) {
        if self.results_doc.get() == id && id != 0 {
            self.results_items.borrow_mut().clear();
            self.results_count_changed();
            self.results_doc.set(0);
        }
    }

    // ---- 输出面板 ----

    pub(super) fn output_clear(&self) {
        if let Some(o) = self.output.get() {
            o.clear_readonly();
        }
    }

    pub(super) fn output_append(&self, text: &str, style: u8) {
        if let Some(o) = self.output.get() {
            o.append_styled(text.as_bytes(), style, OUTPUT_LIMIT);
        }
    }

    pub(super) fn on_output_notify(&self, n: &Notification) {
        if n.code == SCN_DOUBLECLICK {
            let Some(o) = self.output.get() else { return };
            let line = o.line_from_pos(n.position.max(0) as usize);
            let text = o.line_text(line);
            let base = self.run_dir.borrow().clone();
            if let Some(loc) = hn_run::parse::location(&text, base.as_deref()) {
                if loc.path.is_file() {
                    self.open_path(&loc.path, Some((loc.line, None)), false);
                    self.focus_editor();
                } else {
                    self.show_message(&format!("找不到文件 {}", loc.path.display()));
                }
            }
        }
    }

    // ---- 终端 ----

    fn term_font(&self) -> (String, f32) {
        let s = self.settings.borrow();
        let px = s.font_size_x100 as f32 * self.dpi.get() as f32 / 7200.0;
        (s.font_face.clone(), px.round().max(6.0))
    }

    /// 解析 shell 设置：(程序, 参数, 名称, 种类)。
    fn shell_spec(name: &str) -> (String, Vec<String>, String, &'static str) {
        let n = name.trim();
        let stem = Path::new(n)
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let is_path = n.contains(['\\', '/']);
        let program = |default: &str| {
            if is_path {
                n.to_string()
            } else {
                default.to_string()
            }
        };
        match stem.as_str() {
            "cmd" => (program("cmd.exe"), Vec::new(), "cmd".into(), "cmd"),
            "pwsh" => (
                program("pwsh.exe"),
                vec!["-NoLogo".into()],
                "pwsh".into(),
                "pwsh",
            ),
            "powershell" | "" => (
                program("powershell.exe"),
                vec!["-NoLogo".into()],
                "PowerShell".into(),
                "powershell",
            ),
            _ => (n.to_string(), Vec::new(), stem.clone(), "other"),
        }
    }

    /// 新终端的工作目录：当前文档所在目录 → 第一个工作区目录 → 用户目录。
    fn term_cwd(&self) -> Option<PathBuf> {
        self.active_doc(|d| d.path.clone())
            .flatten()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .filter(|d| d.is_dir())
            .or_else(|| self.settings.borrow().workspace.first().cloned())
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
    }

    /// 新建终端（`shell` 为 None 时用设置中的 shell）。
    pub(super) fn new_terminal(&self, shell: Option<&str>) -> Option<u32> {
        self.new_terminal_in(shell, self.term_cwd())
    }

    pub(super) fn new_terminal_in(&self, shell: Option<&str>, cwd: Option<PathBuf>) -> Option<u32> {
        let name = shell
            .map(String::from)
            .unwrap_or_else(|| self.settings.borrow().terminal_shell.clone());
        let (program, args, label, kind) = Self::shell_spec(&name);
        let (face, px) = self.term_font();
        // 估计页面尺寸，让 shell 一启动就有接近正确的行列数
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(self.hwnd(), &mut rc) };
        let dpi = self.dpi.get() as i32;
        let width = (rc.right - rc.left - self.settings.borrow().sidebar_width * dpi / 96).max(200);
        let height = ((self.settings.borrow().drawer_height - 26) * dpi / 96).max(60);
        let sp = crate::term::Spawn {
            program,
            args,
            cwd,
            label,
            kind,
            face,
            px,
            width,
            height,
        };
        match crate::term::create(self.hwnd(), self.hinstance, sp) {
            Ok(id) => {
                self.show_page(Page::Term(id), true);
                Some(id)
            }
            Err(e) => {
                self.show_message(&e);
                None
            }
        }
    }

    pub(super) fn close_terminal(&self, id: u32) {
        self.remove_page(Page::Term(id));
        crate::term::close(id);
    }

    pub(super) fn close_current_terminal(&self) {
        if let Some(Page::Term(id)) = self.current_page() {
            self.close_terminal(id);
        }
    }

    /// 终端请求写剪贴板（选区复制、OSC 52）。
    pub(crate) fn term_copy(&self, text: &str) {
        self.copy_text(text);
    }

    /// shell 退出：关闭终端页。
    pub(crate) fn term_exited(&self, id: u32) {
        self.close_terminal(id);
    }

    // ---- 自测 ----

    pub(crate) fn st_new_terminal(&self, shell: &str) -> Option<u32> {
        self.new_terminal(Some(shell))
    }

    pub(crate) fn st_close_terminal(&self, id: u32) {
        self.close_terminal(id);
    }

    pub(crate) fn st_output_text(&self) -> String {
        self.output
            .get()
            .map(|o| String::from_utf8_lossy(&o.bytes()).into_owned())
            .unwrap_or_default()
    }

    pub(crate) fn st_drawer(&self) -> (bool, Option<&'static str>) {
        let p = self.current_page().map(|p| match p {
            Page::Results => "results",
            Page::Output => "output",
            Page::Term(_) => "term",
        });
        (self.drawer_shown.get(), p)
    }
}
