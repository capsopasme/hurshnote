//! 分屏视图与 Ctrl+Tab 最近使用切换（`app` 的子模块）。
//!
//! 两个 Scintilla 视图共用文档（引用计数）。`sci` 始终是焦点视图，`active` 是焦点视图中的文档；
//! 另一视图只显示，不参与命令。离开焦点视图前把文档状态（位置、书签、折叠、只读等）记到 `Doc` 中。

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsChild, SendMessageW};

use super::*;
use crate::ctl::*;

const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const VK_TAB: usize = 0x09;
const VK_RETURN: usize = 0x0D;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_ESCAPE: usize = 0x1B;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_UP: usize = 0x26;
const VK_DOWN: usize = 0x28;
const LB_GETITEMHEIGHT: u32 = 0x01A1;
const WS_EX_TOOLWINDOW: u32 = 0x80;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const SWP_SHOWWINDOW: u32 = 0x0040;

fn key_down(vk: i32) -> bool {
    let state = unsafe { GetKeyState(vk) };
    state < 0
}

impl App {
    /// 分屏时的另一视图（下标与视图）。
    pub(super) fn other_view(&self) -> Option<(usize, Sci)> {
        if self.split.get() == 0 {
            return None;
        }
        let o = 1 - self.focus.get();
        self.views[o].get().map(|s| (o, s))
    }

    pub(super) fn view_index(&self, hwnd: HWND) -> Option<usize> {
        (0..2).find(|&i| self.views[i].get().is_some_and(|s| s.hwnd() == hwnd))
    }

    /// 离开焦点视图中的文档前记录其状态（这些只能从视图读取）。
    pub(super) fn leave_active(&self) {
        let Some(sci) = self.sci() else { return };
        let cur = self.active.get();
        self.snapshot_active();
        let state = Self::capture_view(&sci);
        let modified = sci.is_modified();
        let (bookmarks, folds) = persist::capture_marks(&sci);
        let eol = editops::eol_of(&sci);
        let ro = sci.is_read_only();
        self.with_doc_mut(cur, |d| {
            d.view = state;
            d.modified = modified;
            d.bookmarks = bookmarks;
            d.folds = folds;
            d.eol = eol;
            d.read_only = ro;
        });
    }

    /// 把焦点移到视图 `i`（不改变各视图显示的文档）。
    pub(super) fn focus_view(&self, i: usize) {
        if i > 1 || i == self.focus.get() || (i == 1 && self.split.get() == 0) {
            return;
        }
        let Some(target) = self.views[i].get() else {
            return;
        };
        let old = self.focus.get();
        let had_focus = self.views[old]
            .get()
            .is_some_and(|s| unsafe { GetFocus() } == s.hwnd());
        self.leave_active();
        self.focus.set(i);
        self.sci.set(Some(target));
        let id = self.view_doc[i].get();
        self.active.set(id);
        self.touch_mru(id);
        if had_focus {
            unsafe { SetFocus(target.hwnd()) };
        }
        self.refresh();
        self.arm_session_timer();
    }

    /// 视图 `i` 改为显示文档 `id`（另一视图用；焦点视图请用 activate）。
    fn show_in_view(&self, i: usize, id: u32) {
        let Some(v) = self.views[i].get() else { return };
        let Some(ptr) = self.with_doc(id, |d| d.ptr).filter(|&p| p != 0) else {
            return;
        };
        v.set_doc_pointer(ptr);
        v.send(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0);
        self.view_doc[i].set(id);
        self.apply_view(i);
    }

    /// 关闭文档前：另一视图若显示它，改为显示焦点视图的文档。
    pub(super) fn retarget_other_view(&self, closing: u32) {
        if let Some((o, _)) = self.other_view() {
            if self.view_doc[o].get() == closing {
                self.show_in_view(o, self.active.get());
            }
        }
    }

    /// 设置分屏模式：0 不分屏，1 左右，2 上下。
    pub(super) fn set_split(&self, mode: u8) {
        let mode = mode.min(2);
        let old = self.split.get();
        if old == mode {
            return;
        }
        let Some(v1) = self.views[1].get() else {
            return;
        };
        if mode == 0 {
            let id = self.active.get();
            self.split.set(0);
            if self.focus.get() == 1 {
                self.focus_view(0);
                self.activate_here(id);
            }
            // 释放视图 1 对文档的引用
            v1.set_doc_pointer(0);
            self.view_doc[1].set(0);
            unsafe { ShowWindow(v1.hwnd(), SW_HIDE) };
            if unsafe { GetFocus() } == v1.hwnd() {
                if let Some(s) = self.sci() {
                    unsafe { SetFocus(s.hwnd()) };
                }
            }
        } else if old == 0 {
            // 克隆当前文档到视图 1
            let id = self.active.get();
            self.split.set(mode);
            self.show_in_view(1, id);
            if let Some(sci) = self.sci() {
                let first = sci.send(SCI_GETFIRSTVISIBLELINE, 0, 0) as usize;
                v1.send(SCI_SETFIRSTVISIBLELINE, first, 0);
            }
            unsafe { ShowWindow(v1.hwnd(), SW_SHOW) };
        } else {
            self.split.set(mode);
        }
        self.layout();
        self.arm_session_timer();
        self.refresh();
    }

    /// 在另一视图显示当前文档（未分屏时先左右分屏）。
    pub(super) fn show_active_in_other(&self) {
        if self.split.get() == 0 {
            self.set_split(1);
            return;
        }
        let id = self.active.get();
        let o = 1 - self.focus.get();
        self.focus_view(o);
        self.activate_here(id);
        if let Some(s) = self.sci() {
            unsafe { SetFocus(s.hwnd()) };
        }
    }

    /// F6：在两个视图间切换焦点。
    pub(super) fn toggle_view_focus(&self) {
        if let Some((o, v)) = self.other_view() {
            self.focus_view(o);
            unsafe { SetFocus(v.hwnd()) };
        }
    }

    /// 编辑区布局：返回各视图的矩形，记录分隔条位置。
    pub(super) fn layout_editors(
        &self,
        left: i32,
        width: i32,
        height: i32,
        splitters: &mut Vec<(Drag, RECT)>,
    ) {
        let v0 = self.views[0].get();
        let v1 = self.views[1].get();
        let gap = 5 * self.dpi.get() as i32 / 96;
        let mode = self.split.get();
        let ratio = self.split_ratio.get();
        let mv = |s: Option<Sci>, x: i32, y: i32, w: i32, h: i32| {
            if let Some(s) = s {
                unsafe { MoveWindow(s.hwnd(), x, y, w.max(0), h.max(0), 1) };
            }
        };
        match mode {
            1 => {
                let w0 = ((width - gap) as f32 * ratio) as i32;
                mv(v0, left, 0, w0, height);
                mv(v1, left + w0 + gap, 0, width - w0 - gap, height);
                self.editor_span.set((left, left + width));
                splitters.push((
                    Drag::EditorSplit,
                    RECT {
                        left: left + w0,
                        top: 0,
                        right: left + w0 + gap,
                        bottom: height,
                    },
                ));
            }
            2 => {
                let h0 = ((height - gap) as f32 * ratio) as i32;
                mv(v0, left, 0, width, h0);
                mv(v1, left, h0 + gap, width, height - h0 - gap);
                self.editor_span.set((0, height));
                splitters.push((
                    Drag::EditorSplit,
                    RECT {
                        left,
                        top: h0,
                        right: left + width,
                        bottom: h0 + gap,
                    },
                ));
            }
            _ => mv(v0, left, 0, width, height),
        }
    }

    // ---- 最近使用顺序 ----

    pub(super) fn touch_mru(&self, id: u32) {
        let mut m = self.mru.borrow_mut();
        m.retain(|&x| x != id);
        m.insert(0, id);
    }

    /// 按最近使用排序的文档（未使用过的按列表顺序排在后面）。
    pub(super) fn mru_order(&self) -> Vec<u32> {
        let ids = self.doc_ids();
        let mut out: Vec<u32> = self
            .mru
            .borrow()
            .iter()
            .filter(|x| ids.contains(x))
            .copied()
            .collect();
        for id in ids {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }

    // ---- Ctrl+Tab 切换器 ----

    /// 消息循环中预处理键盘消息。返回 true 表示已处理。
    pub(super) fn pretranslate(&self, msg: &MSG) -> bool {
        if msg.message != WM_KEYDOWN && msg.message != WM_KEYUP {
            return false;
        }
        if self.quick_pretranslate(msg) {
            return true;
        }
        let main = self.hwnd();
        let ours = msg.hwnd == main || unsafe { IsChild(main, msg.hwnd) } != 0;
        let visible = !self.switcher_ids.borrow().is_empty();
        if !ours && !visible {
            return false;
        }
        let vk = msg.wParam;
        if msg.message == WM_KEYDOWN {
            if vk == VK_TAB && key_down(VK_CONTROL as i32) {
                let back = key_down(VK_SHIFT);
                if visible {
                    self.switcher_move(if back { -1 } else { 1 });
                } else {
                    self.switcher_open(back);
                }
                return true;
            }
            if visible {
                match vk {
                    VK_ESCAPE => self.switcher_close(false),
                    VK_RETURN => self.switcher_close(true),
                    VK_UP | VK_PRIOR => self.switcher_move(-1),
                    VK_DOWN | VK_NEXT => self.switcher_move(1),
                    _ => return false,
                }
                return true;
            }
        } else if vk == VK_CONTROL && visible {
            self.switcher_close(true);
            return true;
        }
        false
    }

    /// 侧边栏与快速打开中不带 Ctrl 的按键（F2、Del、F5……）留给控件自己处理，不走加速键。
    pub(super) fn accel_allowed(&self, msg: &MSG) -> bool {
        if msg.message != WM_KEYDOWN || key_down(VK_CONTROL as i32) {
            return true;
        }
        let h = msg.hwnd;
        let tree = self.side_tree.get();
        let quick = self.quick.get();
        let in_side = h == self.side_list.get() || h == tree || unsafe { IsChild(tree, h) } != 0;
        let in_quick = !quick.is_null() && (h == quick || unsafe { IsChild(quick, h) } != 0);
        !(in_side || in_quick)
    }

    fn ensure_switcher(&self) -> HWND {
        let h = self.switcher.get();
        if !h.is_null() {
            return h;
        }
        let class = wide("LISTBOX");
        let h = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                null(),
                WS_POPUP | WS_BORDER | WS_VSCROLL | LBS_NOINTEGRALHEIGHT,
                0,
                0,
                0,
                0,
                self.hwnd(),
                null_mut(),
                self.hinstance,
                null(),
            )
        };
        self.switcher.set(h);
        h
    }

    fn switcher_open(&self, back: bool) {
        let ids = self.mru_order();
        if ids.len() < 2 {
            return;
        }
        let lb = self.ensure_switcher();
        let font = self.side_font.get();
        let active = self.active.get();
        unsafe {
            SendMessageW(lb, 0x0030, font as usize, 0); // WM_SETFONT
            SendMessageW(lb, LB_RESETCONTENT, 0, 0);
        }
        for &id in &ids {
            let label = self
                .with_doc(id, |d| {
                    let dirty = if id == active { false } else { d.is_dirty() };
                    let loc = d
                        .path
                        .as_ref()
                        .and_then(|p| p.parent())
                        .map(|p| format!("    {}", p.display()))
                        .unwrap_or_default();
                    format!("{}{}{}", if dirty { "*" } else { "" }, d.title(), loc)
                })
                .unwrap_or_default();
            let label = if id == active && self.is_dirty(id) {
                format!("*{label}")
            } else {
                label
            };
            let w = wide(&label);
            unsafe { SendMessageW(lb, LB_ADDSTRING, 0, w.as_ptr() as isize) };
        }
        let dpi = self.dpi.get() as i32;
        let ih = unsafe { SendMessageW(lb, LB_GETITEMHEIGHT, 0, 0) }.max(1) as i32;
        let rows = ids.len().min(16) as i32;
        let (w, h) = (560 * dpi / 96, rows * ih + 4);
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(self.hwnd(), &mut rc) };
        let x = rc.left + ((rc.right - rc.left) - w) / 2;
        let y = rc.top + ((rc.bottom - rc.top) - h) / 3;
        let sel = if back { ids.len() - 1 } else { 1 };
        unsafe {
            SendMessageW(lb, LB_SETCURSEL, sel, 0);
            SetWindowPos(lb, null_mut(), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        *self.switcher_ids.borrow_mut() = ids;
    }

    fn switcher_move(&self, delta: isize) {
        let lb = self.switcher.get();
        let n = self.switcher_ids.borrow().len() as isize;
        if n == 0 {
            return;
        }
        let cur = unsafe { SendMessageW(lb, LB_GETCURSEL, 0, 0) }.max(0);
        let next = (cur + delta).rem_euclid(n);
        unsafe { SendMessageW(lb, LB_SETCURSEL, next as usize, 0) };
    }

    /// 关闭切换器；`commit` 为真时切换到选中的文档。
    pub(super) fn switcher_close(&self, commit: bool) {
        let ids = std::mem::take(&mut *self.switcher_ids.borrow_mut());
        let lb = self.switcher.get();
        if lb.is_null() || ids.is_empty() {
            return;
        }
        let sel = unsafe { SendMessageW(lb, LB_GETCURSEL, 0, 0) };
        unsafe { ShowWindow(lb, SW_HIDE) };
        if commit {
            if let Some(&id) = usize::try_from(sel).ok().and_then(|i| ids.get(i)) {
                self.activate(id);
                if let Some(s) = self.sci() {
                    unsafe { SetFocus(s.hwnd()) };
                }
            }
        }
    }

    pub(crate) fn st_switcher_target(&self) -> Option<u32> {
        self.mru_order().get(1).copied()
    }
}
