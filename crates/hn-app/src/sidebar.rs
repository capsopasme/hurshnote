//! 侧边栏（`app` 的子模块）：打开的文件（虚拟 ListView）与工作区目录树（懒加载 TreeView），
//! 以及主窗口中的分隔条拖动。

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, InvalidateRect, ScreenToClient,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Controls::SetWindowTheme;
use windows_sys::Win32::UI::HiDpi::SystemParametersInfoForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::Shell::{
    SHBrowseForFolderW, SHFileOperationW, SHGetFileInfoW, SHGetPathFromIDListW, ShellExecuteW,
    BROWSEINFOW, SHFILEINFOW, SHFILEOPSTRUCTW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    LoadCursorW, SendMessageW, SetCursor, NONCLIENTMETRICSW,
};

use super::*;
use crate::ctl::*;

pub(super) const ID_LIST: usize = 110;
pub(super) const ID_TREE: usize = 111;
pub(super) const ID_LBL_OPEN: usize = 112;
pub(super) const ID_LBL_WS: usize = 113;

/// 树节点 lParam 中表示"还有更多未显示"的占位。
const NODE_MORE: isize = -1;
/// 每个目录最多显示的条目数。
const MAX_DIR_ENTRIES: usize = 10_000;

const SHGFI_SMALLICON: u32 = 0x1;
const SHGFI_OPENICON: u32 = 0x2;
const SHGFI_USEFILEATTRIBUTES: u32 = 0x10;
const SHGFI_SYSICONINDEX: u32 = 0x4000;
const FO_DELETE: u32 = 3;
const FOF_ALLOWUNDO: u32 = 0x40;
const FOF_WANTNUKEWARNING: u32 = 0x4000;
const BIF_RETURNONLYFSDIRS: u32 = 0x1;
const BIF_EDITBOX: u32 = 0x10;
const BIF_NEWDIALOGSTYLE: u32 = 0x40;
const IDC_SIZEWE: u16 = 32644;
const IDC_SIZENS: u16 = 32645;
const SPI_GETNONCLIENTMETRICS: u32 = 0x29;
const WM_SETFONT: u32 = 0x0030;
/// SS_NOPREFIX | SS_CENTERIMAGE
const SS_LABEL: u32 = 0x0280;

/// 主窗口中可拖动的分隔条。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Drag {
    /// 侧边栏与编辑区之间
    Sidebar,
    /// 打开的文件与工作区之间
    SidebarSplit,
    /// 两个编辑视图之间
    EditorSplit,
    /// 在"打开的文件"中拖动排序（起始下标）
    DocReorder(usize),
}

fn px(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

fn create_child(
    parent: HWND,
    hinstance: *mut core::ffi::c_void,
    class: &str,
    style: u32,
    id: usize,
    text: &str,
) -> HWND {
    let c = wide(class);
    let t = wide(text);
    unsafe {
        CreateWindowExW(
            0,
            c.as_ptr(),
            t.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
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
    pub(super) fn create_sidebar(&self, hwnd: HWND) {
        let p = &GRUVBOX_LIGHT;

        let lbl_open = create_child(
            hwnd,
            self.hinstance,
            "STATIC",
            SS_LABEL,
            ID_LBL_OPEN,
            " 打开的文件",
        );
        let lbl_ws = create_child(
            hwnd,
            self.hinstance,
            "STATIC",
            SS_LABEL,
            ID_LBL_WS,
            " 工作区",
        );
        let list = create_child(
            hwnd,
            self.hinstance,
            "SysListView32",
            WS_TABSTOP
                | LVS_REPORT
                | LVS_NOCOLUMNHEADER
                | LVS_SINGLESEL
                | LVS_SHOWSELALWAYS
                | LVS_SHAREIMAGELISTS
                | LVS_OWNERDATA,
            ID_LIST,
            "",
        );
        let tree = create_child(
            hwnd,
            self.hinstance,
            "SysTreeView32",
            WS_TABSTOP
                | TVS_HASBUTTONS
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS
                | TVS_EDITLABELS
                | TVS_TRACKSELECT
                | TVS_FULLROWSELECT,
            ID_TREE,
            "",
        );
        self.side_list.set(list);
        self.side_tree.set(tree);
        self.side_lbl_open.set(lbl_open);
        self.side_lbl_ws.set(lbl_ws);

        let explorer = wide("Explorer");
        let himl = self.system_image_list();
        unsafe {
            SetWindowTheme(list, explorer.as_ptr(), null());
            SetWindowTheme(tree, explorer.as_ptr(), null());
            let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_INFOTIP;
            SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex, ex as isize);
            let mut col: LVCOLUMNW = std::mem::zeroed();
            col.mask = LVCF_WIDTH;
            col.cx = 200;
            SendMessageW(list, LVM_INSERTCOLUMNW, 0, &col as *const _ as isize);
            SendMessageW(list, LVM_SETIMAGELIST, LVSIL_SMALL, himl);
            SendMessageW(list, LVM_SETBKCOLOR, 0, theme::bgr(p.bg0_s));
            SendMessageW(list, LVM_SETTEXTBKCOLOR, 0, CLR_NONE);
            SendMessageW(list, LVM_SETTEXTCOLOR, 0, theme::bgr(p.fg1));
            SendMessageW(
                tree,
                TVM_SETEXTENDEDSTYLE,
                TVS_EX_DOUBLEBUFFER as usize,
                TVS_EX_DOUBLEBUFFER,
            );
            SendMessageW(tree, TVM_SETIMAGELIST, TVSIL_NORMAL, himl);
            SendMessageW(tree, TVM_SETBKCOLOR, 0, theme::bgr(p.bg0_s));
            SendMessageW(tree, TVM_SETTEXTCOLOR, 0, theme::bgr(p.fg1));
        }
        self.update_sidebar_font();
        self.populate_workspace();
    }

    /// 按当前 DPI 设置侧边栏字体（系统消息字体）。
    pub(super) fn update_sidebar_font(&self) {
        let dpi = self.dpi.get();
        let mut ncm: NONCLIENTMETRICSW = unsafe { std::mem::zeroed() };
        ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
        let ok = unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS,
                ncm.cbSize,
                &mut ncm as *mut _ as *mut core::ffi::c_void,
                0,
                dpi,
            )
        };
        if ok == 0 {
            return;
        }
        let font = unsafe { CreateFontIndirectW(&ncm.lfMessageFont) };
        let old = self.side_font.replace(font as isize);
        for h in [
            self.side_list.get(),
            self.side_tree.get(),
            self.side_lbl_open.get(),
            self.side_lbl_ws.get(),
        ] {
            unsafe { SendMessageW(h, WM_SETFONT, font as usize, 1) };
        }
        if old != 0 {
            unsafe { DeleteObject(old as _) };
        }
    }

    fn system_image_list(&self) -> isize {
        let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let name = wide("x.txt");
        unsafe {
            SHGetFileInfoW(
                name.as_ptr(),
                FILE_ATTRIBUTE_NORMAL,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
            ) as isize
        }
    }

    /// 系统图标在系统图像列表中的下标（按扩展名缓存，不访问磁盘）。
    pub(super) fn icon_index(&self, name: &str, dir: bool, open: bool) -> i32 {
        let key = if dir {
            if open {
                "<dir-open>".to_string()
            } else {
                "<dir>".to_string()
            }
        } else {
            Path::new(name)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        if let Some(&i) = self.icon_cache.borrow().get(&key) {
            return i;
        }
        let probe = if dir {
            "folder".to_string()
        } else {
            format!("x.{key}")
        };
        let w = wide(&probe);
        let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let mut flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES;
        if open {
            flags |= SHGFI_OPENICON;
        }
        let attr = if dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        unsafe {
            SHGetFileInfoW(
                w.as_ptr(),
                attr,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags,
            );
        }
        self.icon_cache.borrow_mut().insert(key, info.iIcon);
        info.iIcon
    }

    // ---- 布局 ----

    /// 侧边栏布局；返回编辑区左边界。同时记录分隔条位置供鼠标命中测试。
    pub(super) fn layout_sidebar(&self, height: i32, splitters: &mut Vec<(Drag, RECT)>) -> i32 {
        let dpi = self.dpi.get();
        let s = self.settings.borrow().clone();
        let hs = [
            self.side_list.get(),
            self.side_tree.get(),
            self.side_lbl_open.get(),
            self.side_lbl_ws.get(),
        ];
        if !s.sidebar_visible {
            for h in hs {
                unsafe { ShowWindow(h, SW_HIDE) };
            }
            return 0;
        }
        for h in hs {
            unsafe { ShowWindow(h, SW_SHOW) };
        }
        let w = px(s.sidebar_width, dpi);
        let gap = px(5, dpi);
        let lh = px(22, dpi);
        let avail = (height - 2 * lh - gap).max(0);
        let list_h = (avail as f32 * s.sidebar_split) as i32;
        unsafe {
            MoveWindow(self.side_lbl_open.get(), 0, 0, w, lh, 1);
            MoveWindow(self.side_list.get(), 0, lh, w, list_h, 1);
            MoveWindow(self.side_lbl_ws.get(), 0, lh + list_h + gap, w, lh, 1);
            MoveWindow(
                self.side_tree.get(),
                0,
                2 * lh + list_h + gap,
                w,
                (height - 2 * lh - list_h - gap).max(0),
                1,
            );
            SendMessageW(
                self.side_list.get(),
                LVM_SETCOLUMNWIDTH,
                0,
                LVSCW_AUTOSIZE_USEHEADER,
            );
        }
        splitters.push((
            Drag::SidebarSplit,
            RECT {
                left: 0,
                top: lh + list_h,
                right: w,
                bottom: lh + list_h + gap,
            },
        ));
        splitters.push((
            Drag::Sidebar,
            RECT {
                left: w,
                top: 0,
                right: w + gap,
                bottom: height,
            },
        ));
        w + gap
    }

    fn hit_splitter(&self, x: i32, y: i32) -> Option<Drag> {
        self.splitters
            .borrow()
            .iter()
            .find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            .map(|(d, _)| *d)
    }

    fn cursor_for(&self, d: Drag) -> u16 {
        match d {
            Drag::Sidebar => IDC_SIZEWE,
            Drag::SidebarSplit => IDC_SIZENS,
            Drag::EditorSplit => {
                if self.split.get() == 2 {
                    IDC_SIZENS
                } else {
                    IDC_SIZEWE
                }
            }
            Drag::DocReorder(_) => 32649, // IDC_HAND
        }
    }

    /// WM_SETCURSOR：分隔条上显示调整大小的光标。返回是否已处理。
    pub(super) fn on_set_cursor(&self) -> bool {
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut pt);
            ScreenToClient(self.hwnd(), &mut pt);
        }
        let d = self.drag.get().or_else(|| self.hit_splitter(pt.x, pt.y));
        match d {
            Some(d) => {
                unsafe { SetCursor(LoadCursorW(null_mut(), self.cursor_for(d) as usize as _)) };
                true
            }
            None => false,
        }
    }

    pub(super) fn on_lbutton_down(&self, x: i32, y: i32) -> bool {
        if let Some(d) = self.hit_splitter(x, y) {
            self.drag.set(Some(d));
            unsafe { SetCapture(self.hwnd()) };
            return true;
        }
        false
    }

    pub(super) fn on_mouse_move(&self, x: i32, y: i32) {
        let Some(d) = self.drag.get() else { return };
        let dpi = self.dpi.get();
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(self.hwnd(), &mut rc) };
        match d {
            Drag::Sidebar => {
                let w = (x * 96 / dpi as i32).clamp(120, 800);
                self.settings.borrow_mut().sidebar_width = w;
            }
            Drag::SidebarSplit => {
                let lh = px(22, dpi);
                let h = (rc.bottom - self.status_height.get() - 2 * lh).max(1);
                let r = ((y - lh) as f32 / h as f32).clamp(0.1, 0.9);
                self.settings.borrow_mut().sidebar_split = r;
            }
            Drag::EditorSplit => {
                let (a, b) = self.editor_span.get();
                let pos = if self.split.get() == 2 { y } else { x };
                if b > a {
                    self.split_ratio
                        .set(((pos - a) as f32 / (b - a) as f32).clamp(0.1, 0.9));
                }
            }
            Drag::DocReorder(_) => return,
        }
        self.layout();
    }

    pub(super) fn on_lbutton_up(&self) {
        let Some(d) = self.drag.take() else { return };
        unsafe { ReleaseCapture() };
        match d {
            Drag::DocReorder(from) => self.finish_doc_drag(from),
            Drag::EditorSplit => self.arm_session_timer(),
            _ => config::save_settings(&self.settings.borrow()),
        }
    }

    // ---- 打开的文件 ----

    /// 刷新"打开的文件"列表并选中当前文档。
    pub(super) fn refresh_open_list(&self) {
        let list = self.side_list.get();
        if list.is_null() {
            return;
        }
        let ids = self.doc_ids();
        let active = self.active.get();
        let idx = ids.iter().position(|&x| x == active);
        self.side_suppress.set(true);
        unsafe {
            SendMessageW(list, LVM_SETITEMCOUNT, ids.len(), 2); // LVSICF_NOSCROLL
            let mut it: LVITEMW = std::mem::zeroed();
            it.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
            it.state = 0;
            SendMessageW(list, LVM_SETITEMSTATE, usize::MAX, &it as *const _ as isize);
            if let Some(i) = idx {
                it.state = LVIS_SELECTED | LVIS_FOCUSED;
                SendMessageW(list, LVM_SETITEMSTATE, i, &it as *const _ as isize);
                SendMessageW(list, LVM_ENSUREVISIBLE, i, 0);
            }
            InvalidateRect(list, null(), 0);
        }
        self.side_suppress.set(false);
    }

    fn doc_label(&self, d: &Doc) -> String {
        let dirty = self.is_dirty(d.id);
        let mut s = String::new();
        if dirty {
            s.push('*');
        }
        s.push_str(&d.title());
        if d.key.is_some() {
            s.push_str("  [加密]");
        }
        if d.missing {
            s.push_str("  [已删除]");
        }
        s
    }

    fn copy_to_wide_buf(text: &str, buf: *mut u16, cap: i32) {
        if buf.is_null() || cap <= 0 {
            return;
        }
        let w: Vec<u16> = text.encode_utf16().collect();
        let n = w.len().min(cap as usize - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(w.as_ptr(), buf, n);
            *buf.add(n) = 0;
        }
    }

    pub(super) fn on_list_notify(&self, lp: LPARAM) -> Option<LRESULT> {
        let hdr = unsafe { &*(lp as *const NMHDR) };
        match hdr.code {
            LVN_GETDISPINFOW => {
                let di = unsafe { &mut *(lp as *mut NMLVDISPINFOW) };
                let Ok(docs) = self.docs.try_borrow() else {
                    return Some(0);
                };
                let Some(d) = docs.get(di.item.iItem as usize) else {
                    return Some(0);
                };
                let label = self.doc_label(d);
                let name = d.title();
                drop(docs);
                if di.item.mask & LVIF_TEXT != 0 {
                    Self::copy_to_wide_buf(&label, di.item.pszText, di.item.cchTextMax);
                }
                if di.item.mask & LVIF_IMAGE != 0 {
                    di.item.iImage = self.icon_index(&name, false, false);
                }
                Some(0)
            }
            LVN_GETINFOTIPW => {
                let tip = unsafe { &mut *(lp as *mut NMLVGETINFOTIPW) };
                let text = self
                    .docs
                    .try_borrow()
                    .ok()
                    .and_then(|docs| {
                        docs.get(tip.iItem as usize).map(|d| match &d.path {
                            Some(p) => p.display().to_string(),
                            None => d.title(),
                        })
                    })
                    .unwrap_or_default();
                Self::copy_to_wide_buf(&text, tip.pszText, tip.cchTextMax);
                Some(0)
            }
            LVN_ITEMCHANGED => {
                let nm = unsafe { &*(lp as *const NMLISTVIEW) };
                if !self.side_suppress.get()
                    && nm.iItem >= 0
                    && nm.uNewState & LVIS_SELECTED != 0
                    && nm.uOldState & LVIS_SELECTED == 0
                {
                    if let Some(&id) = self.doc_ids().get(nm.iItem as usize) {
                        let list = self.side_list.get();
                        unsafe {
                            PostMessageW(
                                self.hwnd(),
                                WM_APP_ACTIVATE_DOC,
                                id as usize,
                                list as isize,
                            )
                        };
                    }
                }
                Some(0)
            }
            NM_CLICK => {
                let nm = unsafe { &*(lp as *const NMITEMACTIVATE) };
                if let Some(&id) = self
                    .doc_ids()
                    .get(nm.iItem.max(0) as usize)
                    .filter(|_| nm.iItem >= 0)
                {
                    unsafe { PostMessageW(self.hwnd(), WM_APP_ACTIVATE_DOC, id as usize, 0) };
                }
                Some(0)
            }
            LVN_BEGINDRAG => {
                let nm = unsafe { &*(lp as *const NMLISTVIEW) };
                if nm.iItem >= 0 {
                    self.drag.set(Some(Drag::DocReorder(nm.iItem as usize)));
                    unsafe { SetCapture(self.hwnd()) };
                }
                Some(0)
            }
            _ => None,
        }
    }

    fn finish_doc_drag(&self, from: usize) {
        let list = self.side_list.get();
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut pt);
            ScreenToClient(list, &mut pt);
        }
        let mut ht = LVHITTESTINFO {
            pt,
            flags: 0,
            iItem: -1,
            iSubItem: 0,
            iGroup: 0,
        };
        let hit = unsafe { SendMessageW(list, LVM_HITTEST, 0, &mut ht as *mut _ as isize) };
        let n = self.docs.borrow().len();
        let to = if hit >= 0 {
            hit as usize
        } else if pt.y > 0 {
            n.saturating_sub(1)
        } else {
            return;
        };
        if from != to && from < n && to < n {
            let mut docs = self.docs.borrow_mut();
            let d = docs.remove(from);
            docs.insert(to, d);
        }
        self.refresh_open_list();
        self.arm_session_timer();
    }

    /// "打开的文件"的右键菜单（WM_CONTEXTMENU）：鼠标下的项，键盘触发时用选中项。
    pub(super) fn list_context_menu_at_cursor(&self, from_keyboard: bool) {
        let list = self.side_list.get();
        let idx = if from_keyboard {
            unsafe { SendMessageW(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize) }
        } else {
            let mut pt = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut pt);
                ScreenToClient(list, &mut pt);
            }
            let mut ht = LVHITTESTINFO {
                pt,
                flags: 0,
                iItem: -1,
                iSubItem: 0,
                iGroup: 0,
            };
            unsafe { SendMessageW(list, LVM_HITTEST, 0, &mut ht as *mut _ as isize) }
        };
        let ids = self.doc_ids();
        let Some(&id) = usize::try_from(idx).ok().and_then(|i| ids.get(i)) else {
            return;
        };
        self.ctx_doc.set(id);
        self.list_context_menu();
    }

    fn list_context_menu(&self) {
        let items: [(u16, &str); 6] = [
            (IDM_CTX_CLOSE, "关闭"),
            (IDM_CTX_CLOSE_OTHERS, "关闭其他"),
            (0, ""),
            (IDM_CTX_COPY_PATH, "复制完整路径"),
            (IDM_CTX_COPY_NAME, "复制文件名"),
            (IDM_CTX_REVEAL, "在资源管理器中显示"),
        ];
        self.popup_items(&items);
    }

    fn popup_items(&self, items: &[(u16, &str)]) {
        let m = unsafe { CreatePopupMenu() };
        for (id, text) in items {
            let w = wide(text);
            if *id == 0 {
                unsafe { AppendMenuW(m, MF_SEPARATOR, 0, null()) };
            } else {
                unsafe { AppendMenuW(m, MF_STRING, *id as usize, w.as_ptr()) };
            }
        }
        self.popup_at_cursor(m);
    }

    pub(super) fn copy_text(&self, text: &str) {
        if let Some(sci) = self.sci() {
            sci.send(SCI_COPYTEXT, text.len(), text.as_ptr() as isize);
        }
    }

    pub(super) fn reveal_in_explorer(&self, path: &Path) {
        let (op, file) = (wide("open"), wide("explorer.exe"));
        let params = wide(&format!("/select,\"{}\"", path.display()));
        unsafe {
            ShellExecuteW(
                self.hwnd(),
                op.as_ptr(),
                file.as_ptr(),
                params.as_ptr(),
                null(),
                1,
            )
        };
    }

    pub(super) fn on_list_context_command(&self, id: u16) {
        let doc = self.ctx_doc.get();
        let path = self.with_doc(doc, |d| d.path.clone()).flatten();
        match id {
            IDM_CTX_CLOSE => {
                self.close_doc(doc);
            }
            IDM_CTX_CLOSE_OTHERS => {
                for other in self.doc_ids() {
                    if other != doc && !self.close_doc(other) {
                        break;
                    }
                }
            }
            IDM_CTX_COPY_PATH => {
                if let Some(p) = &path {
                    self.copy_text(&p.display().to_string());
                }
            }
            IDM_CTX_COPY_NAME => {
                let name = self.with_doc(doc, |d| d.title()).unwrap_or_default();
                self.copy_text(&name);
            }
            IDM_CTX_REVEAL => {
                if let Some(p) = &path {
                    self.reveal_in_explorer(p);
                }
            }
            _ => {}
        }
    }

    // ---- 工作区目录树 ----

    fn tree_insert(&self, parent: isize, path: &Path, label: &str, is_dir: bool) -> isize {
        let idx = {
            let mut nodes = self.tree_nodes.borrow_mut();
            nodes.push(path.to_path_buf());
            nodes.len() - 1
        };
        let w = wide(label);
        let icon = self.icon_index(label, is_dir, false);
        let open_icon = if is_dir {
            self.icon_index(label, true, true)
        } else {
            icon
        };
        let ins = TVINSERTSTRUCTW::new(
            parent,
            TVI_LAST,
            TVITEMW {
                mask: TVIF_TEXT | TVIF_PARAM | TVIF_IMAGE | TVIF_SELECTEDIMAGE | TVIF_CHILDREN,
                pszText: w.as_ptr() as *mut u16,
                iImage: icon,
                iSelectedImage: open_icon,
                cChildren: is_dir as i32,
                lParam: idx as isize,
                ..Default::default()
            },
        );
        unsafe {
            SendMessageW(
                self.side_tree.get(),
                TVM_INSERTITEMW,
                0,
                &ins as *const _ as isize,
            )
        }
    }

    /// 重建工作区根节点。
    pub(super) fn populate_workspace(&self) {
        let tree = self.side_tree.get();
        if tree.is_null() {
            return;
        }
        unsafe { SendMessageW(tree, TVM_DELETEITEM, 0, TVI_ROOT) };
        self.tree_nodes.borrow_mut().clear();
        let roots = self.settings.borrow().workspace.clone();
        for r in roots {
            let label = r
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| r.display().to_string());
            self.tree_insert(TVI_ROOT, &r, &label, true);
        }
    }

    fn item_param(&self, item: isize) -> Option<isize> {
        let mut it = TVITEMW {
            mask: TVIF_PARAM | TVIF_HANDLE,
            hItem: item,
            ..Default::default()
        };
        let ok = unsafe {
            SendMessageW(
                self.side_tree.get(),
                TVM_GETITEMW,
                0,
                &mut it as *mut _ as isize,
            )
        };
        (ok != 0).then_some(it.lParam)
    }

    fn item_path(&self, item: isize) -> Option<PathBuf> {
        let p = self.item_param(item)?;
        if p < 0 {
            return None;
        }
        self.tree_nodes.borrow().get(p as usize).cloned()
    }

    fn is_root_item(&self, item: isize) -> bool {
        unsafe { SendMessageW(self.side_tree.get(), TVM_GETNEXTITEM, TVGN_PARENT, item) == 0 }
    }

    /// 列出目录内容（可选遵守 .gitignore），目录在前，按名称排序。
    fn list_dir(&self, dir: &Path) -> (Vec<(PathBuf, bool)>, usize) {
        let gitignore = self.settings.borrow().respect_gitignore;
        let mut out: Vec<(PathBuf, bool)> = Vec::new();
        if gitignore {
            let walker = ignore::WalkBuilder::new(dir)
                .max_depth(Some(1))
                .hidden(false)
                .parents(true)
                .git_ignore(true)
                .git_exclude(true)
                .git_global(true)
                .require_git(false)
                .build();
            for e in walker.flatten() {
                if e.depth() == 0 {
                    continue;
                }
                let is_dir = e.file_type().is_some_and(|t| t.is_dir());
                out.push((e.into_path(), is_dir));
            }
        } else if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                out.push((e.path(), is_dir));
            }
        }
        out.retain(|(p, _)| p.file_name().is_some_and(|n| n != ".git"));
        out.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| {
                let an =
                    a.0.file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                let bn =
                    b.0.file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                an.cmp(&bn)
            })
        });
        let total = out.len();
        out.truncate(MAX_DIR_ENTRIES);
        (out, total)
    }

    fn populate_dir(&self, item: isize, dir: &Path) {
        let (entries, total) = self.list_dir(dir);
        let tree = self.side_tree.get();
        if entries.is_empty() {
            let it = TVITEMW {
                mask: TVIF_CHILDREN | TVIF_HANDLE,
                hItem: item,
                cChildren: 0,
                ..Default::default()
            };
            unsafe { SendMessageW(tree, TVM_SETITEMW, 0, &it as *const _ as isize) };
            return;
        }
        for (p, is_dir) in &entries {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.tree_insert(item, p, &name, *is_dir);
        }
        if total > entries.len() {
            let label = wide(&format!("（还有 {} 项未显示）", total - entries.len()));
            let ins = TVINSERTSTRUCTW::new(
                item,
                TVI_LAST,
                TVITEMW {
                    mask: TVIF_TEXT | TVIF_PARAM,
                    pszText: label.as_ptr() as *mut u16,
                    lParam: NODE_MORE,
                    ..Default::default()
                },
            );
            unsafe { SendMessageW(tree, TVM_INSERTITEMW, 0, &ins as *const _ as isize) };
        }
    }

    /// 重新读取目录节点的内容。
    fn refresh_tree_item(&self, item: isize) {
        let tree = self.side_tree.get();
        unsafe {
            SendMessageW(tree, TVM_EXPAND, TVE_COLLAPSE | TVE_COLLAPSERESET, item);
            let it = TVITEMW {
                mask: TVIF_CHILDREN | TVIF_HANDLE,
                hItem: item,
                cChildren: 1,
                ..Default::default()
            };
            SendMessageW(tree, TVM_SETITEMW, 0, &it as *const _ as isize);
            SendMessageW(tree, TVM_EXPAND, TVE_EXPAND, item);
        }
    }

    pub(super) fn on_tree_notify(&self, lp: LPARAM) -> Option<LRESULT> {
        let hdr = unsafe { &*(lp as *const NMHDR) };
        let tree = self.side_tree.get();
        match hdr.code {
            TVN_ITEMEXPANDINGW => {
                let nm = unsafe { &*(lp as *const NMTREEVIEWW) };
                let item = nm.itemNew.hItem;
                if nm.action as usize & TVE_EXPAND != 0
                    && unsafe { SendMessageW(tree, TVM_GETNEXTITEM, TVGN_CHILD, item) } == 0
                {
                    if let Some(p) = self.item_path(item) {
                        self.populate_dir(item, &p);
                    }
                }
                Some(0)
            }
            NM_DBLCLK | NM_RETURN => {
                self.tree_open_selected();
                Some(0)
            }
            TVN_KEYDOWN => {
                let k = unsafe { &*(lp as *const NMKEYDOWN) };
                let vk = k.wVKey;
                match vk {
                    0x71 => self.on_tree_command(IDM_TREE_RENAME),  // F2
                    0x2E => self.on_tree_command(IDM_TREE_DELETE),  // Delete
                    0x74 => self.on_tree_command(IDM_TREE_REFRESH), // F5
                    _ => {}
                }
                Some(0)
            }
            NM_RCLICK => {
                self.tree_context_menu();
                Some(1)
            }
            TVN_BEGINLABELEDITW => {
                let di = unsafe { &*(lp as *const NMTVDISPINFOW) };
                // 根节点（工作区文件夹）不允许改名
                Some(self.is_root_item(di.item.hItem) as isize)
            }
            TVN_ENDLABELEDITW => {
                let di = unsafe { &*(lp as *const NMTVDISPINFOW) };
                if di.item.pszText.is_null() {
                    return Some(0);
                }
                let mut len = 0usize;
                while unsafe { *di.item.pszText.add(len) } != 0 {
                    len += 1;
                }
                let new_name = String::from_utf16_lossy(unsafe {
                    std::slice::from_raw_parts(di.item.pszText, len)
                });
                Some(self.rename_tree_item(di.item.hItem, new_name.trim()) as isize)
            }
            _ => None,
        }
    }

    fn selected_tree_item(&self) -> isize {
        unsafe { SendMessageW(self.side_tree.get(), TVM_GETNEXTITEM, TVGN_CARET, 0) }
    }

    fn tree_open_selected(&self) {
        let item = self.selected_tree_item();
        if let Some(p) = self.item_path(item) {
            if p.is_file() {
                self.open_path(&p, None, false);
            }
        }
    }

    pub(super) fn tree_context_menu(&self) {
        let tree = self.side_tree.get();
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut pt);
            ScreenToClient(tree, &mut pt);
        }
        let mut ht = TVHITTESTINFO {
            pt,
            flags: 0,
            hItem: 0,
        };
        let item = unsafe { SendMessageW(tree, TVM_HITTEST, 0, &mut ht as *mut _ as isize) };
        if item != 0 {
            unsafe { SendMessageW(tree, TVM_SELECTITEM, TVGN_CARET, item) };
        }
        self.ctx_tree.set(item);
        let path = self.item_path(item);
        let is_dir = path.as_ref().is_some_and(|p| p.is_dir());
        let is_root = item != 0 && self.is_root_item(item);
        let mut items: Vec<(u16, &str)> = Vec::new();
        if let Some(p) = &path {
            if p.is_file() {
                items.push((IDM_TREE_OPEN, "打开"));
                items.push((0, ""));
            }
            if is_dir {
                items.push((IDM_TREE_NEW_FILE, "新建文件"));
                items.push((IDM_TREE_NEW_DIR, "新建文件夹"));
                items.push((0, ""));
            }
            if !is_root {
                items.push((IDM_TREE_RENAME, "重命名\tF2"));
                items.push((IDM_TREE_DELETE, "删除（到回收站）\tDel"));
                items.push((0, ""));
            }
            items.push((IDM_TREE_COPY_PATH, "复制完整路径"));
            items.push((IDM_TREE_REVEAL, "在资源管理器中显示"));
            items.push((0, ""));
            if is_dir {
                items.push((IDM_TREE_REFRESH, "刷新\tF5"));
            }
            if is_root {
                items.push((IDM_TREE_REMOVE_ROOT, "从工作区移除"));
            }
        }
        items.push((IDM_WS_ADD, "添加文件夹到工作区..."));
        self.popup_items(&items);
    }

    pub(super) fn on_tree_command(&self, id: u16) {
        let tree = self.side_tree.get();
        let item = if id == IDM_TREE_RENAME || id == IDM_TREE_DELETE || id == IDM_TREE_REFRESH {
            let s = self.selected_tree_item();
            if s != 0 {
                s
            } else {
                self.ctx_tree.get()
            }
        } else {
            self.ctx_tree.get()
        };
        let path = self.item_path(item);
        match id {
            IDM_TREE_OPEN => {
                if let Some(p) = path {
                    self.open_path(&p, None, false);
                }
            }
            IDM_TREE_NEW_FILE | IDM_TREE_NEW_DIR => {
                let Some(dir) = path.filter(|p| p.is_dir()) else {
                    return;
                };
                let is_dir = id == IDM_TREE_NEW_DIR;
                let base = if is_dir {
                    "新建文件夹"
                } else {
                    "新建文件.txt"
                };
                let mut target = dir.join(base);
                let mut n = 2;
                while target.exists() {
                    target = dir.join(if is_dir {
                        format!("新建文件夹 ({n})")
                    } else {
                        format!("新建文件 ({n}).txt")
                    });
                    n += 1;
                }
                let r = if is_dir {
                    std::fs::create_dir(&target)
                } else {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&target)
                        .map(|_| ())
                };
                if let Err(e) = r {
                    dialogs::error_box(
                        self.hwnd(),
                        &format!("无法创建 {}\n\n{e}", target.display()),
                    );
                    return;
                }
                self.refresh_tree_item(item);
                // 找到新节点并进入改名
                let mut child = unsafe { SendMessageW(tree, TVM_GETNEXTITEM, TVGN_CHILD, item) };
                while child != 0 {
                    if self.item_path(child).is_some_and(|p| p == target) {
                        unsafe {
                            SendMessageW(tree, TVM_SELECTITEM, TVGN_CARET, child);
                            SetFocus(tree);
                            SendMessageW(tree, TVM_EDITLABELW, 0, child);
                        }
                        break;
                    }
                    child = unsafe { SendMessageW(tree, TVM_GETNEXTITEM, TVGN_NEXT, child) };
                }
                if !is_dir {
                    self.open_path(&target, None, false);
                }
            }
            IDM_TREE_RENAME => {
                if item != 0 && !self.is_root_item(item) {
                    unsafe {
                        SetFocus(tree);
                        SendMessageW(tree, TVM_EDITLABELW, 0, item);
                    }
                }
            }
            IDM_TREE_DELETE => {
                let Some(p) = path else { return };
                if item == 0 || self.is_root_item(item) {
                    return;
                }
                let mut from: Vec<u16> = p.as_os_str().encode_wide_vec();
                from.push(0);
                from.push(0);
                let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
                op.hwnd = self.hwnd();
                op.wFunc = FO_DELETE as _;
                op.pFrom = from.as_ptr();
                op.fFlags = (FOF_ALLOWUNDO | FOF_WANTNUKEWARNING) as _;
                let r = unsafe { SHFileOperationW(&mut op) };
                if r == 0 && op.fAnyOperationsAborted == 0 {
                    unsafe { SendMessageW(tree, TVM_DELETEITEM, 0, item) };
                }
            }
            IDM_TREE_COPY_PATH => {
                if let Some(p) = path {
                    self.copy_text(&p.display().to_string());
                }
            }
            IDM_TREE_REVEAL => {
                if let Some(p) = path {
                    self.reveal_in_explorer(&p);
                }
            }
            IDM_TREE_REFRESH => {
                if item != 0 && path.is_some_and(|p| p.is_dir()) {
                    self.refresh_tree_item(item);
                } else {
                    self.populate_workspace();
                }
            }
            IDM_TREE_REMOVE_ROOT => {
                if let Some(p) = path {
                    {
                        let mut s = self.settings.borrow_mut();
                        s.workspace.retain(|r| !config::same_path(r, &p));
                        config::save_settings(&s);
                    }
                    self.populate_workspace();
                    self.file_index.borrow_mut().take();
                }
            }
            _ => {}
        }
    }

    /// 树中改名（TVN_ENDLABELEDIT）。返回是否接受新名字。
    fn rename_tree_item(&self, item: isize, new_name: &str) -> bool {
        let Some(old) = self.item_path(item) else {
            return false;
        };
        if new_name.is_empty() || new_name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        {
            return false;
        }
        let Some(parent) = old.parent() else {
            return false;
        };
        let new = parent.join(new_name);
        if new == old {
            return false;
        }
        if let Err(e) = std::fs::rename(&old, &new) {
            dialogs::error_box(self.hwnd(), &format!("无法重命名\n\n{e}"));
            return false;
        }
        if let Some(p) = self.item_param(item) {
            if let Some(slot) = self.tree_nodes.borrow_mut().get_mut(p as usize) {
                *slot = new.clone();
            }
        }
        // 已打开的文档跟随改名
        let renamed: Vec<u32> = self
            .docs
            .borrow_mut()
            .iter_mut()
            .filter_map(|d| {
                let p = d.path.as_ref()?;
                let rel = p.strip_prefix(&old).ok()?.to_path_buf();
                d.path = Some(if rel.as_os_str().is_empty() {
                    new.clone()
                } else {
                    new.join(rel)
                });
                Some(d.id)
            })
            .collect();
        for id in renamed {
            if let Some(p) = self.with_doc(id, |d| d.path.clone()).flatten() {
                self.watch(id, &p);
            }
        }
        self.refresh();
        true
    }

    /// 选择文件夹加入工作区。
    pub(super) fn add_workspace_folder(&self) {
        let title = wide("选择要加入工作区的文件夹");
        let mut bi: BROWSEINFOW = unsafe { std::mem::zeroed() };
        bi.hwndOwner = self.hwnd();
        bi.lpszTitle = title.as_ptr();
        bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX;
        let pidl = unsafe { SHBrowseForFolderW(&bi) };
        if pidl.is_null() {
            return;
        }
        let mut buf = vec![0u16; 1024];
        let ok = unsafe { SHGetPathFromIDListW(pidl, buf.as_mut_ptr()) };
        unsafe { CoTaskMemFree(pidl as *const core::ffi::c_void) };
        if ok == 0 {
            return;
        }
        self.add_workspace_path(&PathBuf::from(from_wide(&buf)));
    }

    pub(super) fn add_workspace_path(&self, dir: &Path) {
        {
            let mut s = self.settings.borrow_mut();
            if s.workspace.iter().any(|r| config::same_path(r, dir)) {
                return;
            }
            s.workspace.push(dir.to_path_buf());
            s.sidebar_visible = true;
            config::save_settings(&s);
        }
        self.populate_workspace();
        self.file_index.borrow_mut().take();
        self.layout();
    }

    /// 释放侧边栏的 GDI 资源。
    pub(super) fn destroy_sidebar(&self) {
        let h = self.side_font.replace(0);
        if h != 0 {
            unsafe { DeleteObject(h as _) };
        }
    }

    /// 自测：展开第一个工作区根节点，返回子项名称。
    pub(crate) fn st_expand_first_root(&self) -> Vec<String> {
        let tree = self.side_tree.get();
        let root = unsafe { SendMessageW(tree, TVM_GETNEXTITEM, TVGN_ROOT, 0) };
        if root == 0 {
            return Vec::new();
        }
        unsafe { SendMessageW(tree, TVM_EXPAND, TVE_EXPAND, root) };
        let mut out = Vec::new();
        let mut c = unsafe { SendMessageW(tree, TVM_GETNEXTITEM, TVGN_CHILD, root) };
        while c != 0 {
            if let Some(p) = self.item_path(c) {
                out.push(
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
            }
            c = unsafe { SendMessageW(tree, TVM_GETNEXTITEM, TVGN_NEXT, c) };
        }
        out
    }

    /// 自测："打开的文件"列表的项数。
    pub(crate) fn st_list_count(&self) -> usize {
        unsafe { SendMessageW(self.side_list.get(), LVM_GETITEMCOUNT, 0, 0) }.max(0) as usize
    }
}

trait EncodeWideVec {
    fn encode_wide_vec(&self) -> Vec<u16>;
}

impl EncodeWideVec for std::ffi::OsStr {
    fn encode_wide_vec(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().collect()
    }
}
