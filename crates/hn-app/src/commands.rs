//! 命令 ID、快捷键表、菜单结构。
//!
//! 快捷键是数据：[`KEYS`] 同时生成加速键表和菜单上显示的快捷键文字。

use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu, ACCEL, HACCEL, HMENU,
};

use crate::win::*;

// ---- 文件 ----
pub const IDM_FILE_NEW: u16 = 1001;
pub const IDM_FILE_OPEN: u16 = 1002;
pub const IDM_FILE_SAVE: u16 = 1003;
pub const IDM_FILE_SAVE_AS: u16 = 1004;
pub const IDM_FILE_SAVE_ALL: u16 = 1005;
pub const IDM_FILE_CLOSE: u16 = 1006;
pub const IDM_FILE_CLOSE_ALL: u16 = 1007;
pub const IDM_FILE_EXIT: u16 = 1008;
pub const IDM_RECENT_CLEAR: u16 = 1009;

// ---- 编辑 ----
pub const IDM_EDIT_UNDO: u16 = 1101;
pub const IDM_EDIT_REDO: u16 = 1102;
pub const IDM_EDIT_CUT: u16 = 1103;
pub const IDM_EDIT_COPY: u16 = 1104;
pub const IDM_EDIT_PASTE: u16 = 1105;
pub const IDM_EDIT_DELETE: u16 = 1106;
pub const IDM_EDIT_SELECT_ALL: u16 = 1107;
pub const IDM_EDIT_READ_ONLY: u16 = 1108;
pub const IDM_EDIT_LIST_CONTINUE: u16 = 1109;
pub const IDM_EDIT_UPPER: u16 = 1110;
pub const IDM_EDIT_LOWER: u16 = 1111;

pub const IDM_LINE_DUPLICATE: u16 = 1201;
pub const IDM_LINE_UP: u16 = 1202;
pub const IDM_LINE_DOWN: u16 = 1203;
pub const IDM_LINE_JOIN: u16 = 1204;
pub const IDM_LINE_SORT_ASC: u16 = 1205;
pub const IDM_LINE_SORT_DESC: u16 = 1206;
pub const IDM_LINE_SORT_ASC_CI: u16 = 1207;
pub const IDM_LINE_SORT_DESC_CI: u16 = 1208;
pub const IDM_LINE_SORT_NUM_ASC: u16 = 1209;
pub const IDM_LINE_SORT_NUM_DESC: u16 = 1210;
pub const IDM_LINE_UNIQUE: u16 = 1211;
pub const IDM_LINE_UNIQUE_ADJ: u16 = 1212;
pub const IDM_LINE_REMOVE_EMPTY: u16 = 1213;
pub const IDM_LINE_REMOVE_BLANK: u16 = 1214;
pub const IDM_LINE_TRIM_TRAILING: u16 = 1215;
pub const IDM_LINE_TRIM_LEADING: u16 = 1216;

pub const IDM_EOL_CRLF: u16 = 1301;
pub const IDM_EOL_LF: u16 = 1302;
pub const IDM_EOL_CR: u16 = 1303;

// ---- 搜索 ----
pub const IDM_SEARCH_FIND: u16 = 1401;
pub const IDM_SEARCH_REPLACE: u16 = 1402;
pub const IDM_SEARCH_NEXT: u16 = 1403;
pub const IDM_SEARCH_PREV: u16 = 1404;
pub const IDM_SEARCH_SEL_NEXT: u16 = 1405;
pub const IDM_SEARCH_SEL_PREV: u16 = 1406;
pub const IDM_SEARCH_CLEAR_MARKS: u16 = 1407;
pub const IDM_SEARCH_GOTO: u16 = 1408;

pub const IDM_BM_TOGGLE: u16 = 1501;
pub const IDM_BM_NEXT: u16 = 1502;
pub const IDM_BM_PREV: u16 = 1503;
pub const IDM_BM_CLEAR: u16 = 1504;
pub const IDM_BM_INVERT: u16 = 1505;
pub const IDM_BM_COPY: u16 = 1506;
pub const IDM_BM_CUT: u16 = 1507;
pub const IDM_BM_DELETE: u16 = 1508;
pub const IDM_BM_DELETE_UNMARKED: u16 = 1509;

// ---- 视图 ----
pub const IDM_VIEW_WRAP: u16 = 1601;
pub const IDM_VIEW_WHITESPACE: u16 = 1602;
pub const IDM_VIEW_EOL: u16 = 1603;
pub const IDM_VIEW_INDENT_GUIDES: u16 = 1604;
pub const IDM_VIEW_LINE_NUMBERS: u16 = 1605;
pub const IDM_VIEW_ZOOM_IN: u16 = 1606;
pub const IDM_VIEW_ZOOM_OUT: u16 = 1607;
pub const IDM_VIEW_ZOOM_RESET: u16 = 1608;
pub const IDM_VIEW_FOLD_ALL: u16 = 1609;
pub const IDM_VIEW_UNFOLD_ALL: u16 = 1610;

// ---- 窗口 / 帮助 ----
pub const IDM_WIN_NEXT: u16 = 1701;
pub const IDM_WIN_PREV: u16 = 1702;
pub const IDM_HELP_ABOUT: u16 = 1901;

// ---- 动态范围 ----
pub const IDM_ENC_REOPEN_BASE: u16 = 2000;
pub const IDM_ENC_CONVERT_BASE: u16 = 2100;
pub const IDM_LANG_BASE: u16 = 2200;
pub const IDM_ZOOM_BASE: u16 = 2300;
pub const IDM_WINDOW_BASE: u16 = 3000;
pub const IDM_WINDOW_MAX: u16 = 400;
pub const IDM_RECENT_BASE: u16 = 3500;
pub const IDM_RECENT_MAX: u16 = 30;

/// 状态栏缩放菜单中的预设比例。
pub const ZOOM_PRESETS: &[u32] = &[50, 75, 90, 100, 110, 125, 150, 175, 200, 250, 300];

// ---- 快捷键 ----

const VK_TAB: u16 = 0x09;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;
const VK_ADD: u16 = 0x6B;
const VK_SUBTRACT: u16 = 0x6D;
const VK_DIVIDE: u16 = 0x6F;
const VK_F2: u16 = 0x71;
const VK_F3: u16 = 0x72;
const VK_OEM_PLUS: u16 = 0xBB;
const VK_OEM_MINUS: u16 = 0xBD;

const C: u8 = 1;
const S: u8 = 2;
const A: u8 = 4;

/// 一条快捷键。`native` 表示由 Scintilla 键位表处理，只用于菜单显示。
pub struct Key {
    pub cmd: u16,
    pub vk: u16,
    pub mods: u8,
    pub native: bool,
}

const fn k(cmd: u16, mods: u8, vk: u16) -> Key {
    Key {
        cmd,
        vk,
        mods,
        native: false,
    }
}

const fn n(cmd: u16, mods: u8, vk: u16) -> Key {
    Key {
        cmd,
        vk,
        mods,
        native: true,
    }
}

const fn ch(c: u8) -> u16 {
    c as u16
}

/// 快捷键表（尽量沿用 Notepad++ 的默认键位）。同一命令可有多个键，菜单显示第一个。
pub static KEYS: &[Key] = &[
    k(IDM_FILE_NEW, C, ch(b'N')),
    k(IDM_FILE_OPEN, C, ch(b'O')),
    k(IDM_FILE_SAVE, C, ch(b'S')),
    k(IDM_FILE_SAVE_AS, C | A, ch(b'S')),
    k(IDM_FILE_SAVE_ALL, C | S, ch(b'S')),
    k(IDM_FILE_CLOSE, C, ch(b'W')),
    k(IDM_FILE_CLOSE_ALL, C | S, ch(b'W')),
    n(IDM_EDIT_UNDO, C, ch(b'Z')),
    n(IDM_EDIT_REDO, C, ch(b'Y')),
    n(IDM_EDIT_CUT, C, ch(b'X')),
    n(IDM_EDIT_COPY, C, ch(b'C')),
    n(IDM_EDIT_PASTE, C, ch(b'V')),
    n(IDM_EDIT_SELECT_ALL, C, ch(b'A')),
    n(IDM_EDIT_UPPER, C | S, ch(b'U')),
    n(IDM_EDIT_LOWER, C, ch(b'U')),
    n(IDM_LINE_DUPLICATE, C, ch(b'D')),
    k(IDM_LINE_UP, C | S, VK_UP),
    k(IDM_LINE_DOWN, C | S, VK_DOWN),
    k(IDM_LINE_JOIN, C, ch(b'J')),
    k(IDM_SEARCH_FIND, C, ch(b'F')),
    k(IDM_SEARCH_REPLACE, C, ch(b'H')),
    k(IDM_SEARCH_NEXT, 0, VK_F3),
    k(IDM_SEARCH_PREV, S, VK_F3),
    k(IDM_SEARCH_SEL_NEXT, C, VK_F3),
    k(IDM_SEARCH_SEL_PREV, C | S, VK_F3),
    k(IDM_SEARCH_GOTO, C, ch(b'G')),
    k(IDM_BM_TOGGLE, C, VK_F2),
    k(IDM_BM_NEXT, 0, VK_F2),
    k(IDM_BM_PREV, S, VK_F2),
    k(IDM_VIEW_ZOOM_IN, C, VK_OEM_PLUS),
    k(IDM_VIEW_ZOOM_IN, C, VK_ADD),
    k(IDM_VIEW_ZOOM_OUT, C, VK_OEM_MINUS),
    k(IDM_VIEW_ZOOM_OUT, C, VK_SUBTRACT),
    k(IDM_VIEW_ZOOM_RESET, C, ch(b'0')),
    k(IDM_VIEW_ZOOM_RESET, C, VK_DIVIDE),
    k(IDM_WIN_NEXT, C, VK_TAB),
    k(IDM_WIN_PREV, C | S, VK_TAB),
];

fn key_name(vk: u16) -> String {
    match vk {
        VK_TAB => "Tab".into(),
        VK_UP => "Up".into(),
        VK_DOWN => "Down".into(),
        VK_ADD => "Num +".into(),
        VK_SUBTRACT => "Num -".into(),
        VK_DIVIDE => "Num /".into(),
        VK_OEM_PLUS => "=".into(),
        VK_OEM_MINUS => "-".into(),
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        _ => char::from_u32(vk as u32)
            .map(String::from)
            .unwrap_or_default(),
    }
}

/// 命令的快捷键文字，例如 "Ctrl+Shift+S"。
pub fn shortcut_text(cmd: u16) -> Option<String> {
    let key = KEYS.iter().find(|x| x.cmd == cmd)?;
    let mut s = String::new();
    if key.mods & C != 0 {
        s.push_str("Ctrl+");
    }
    if key.mods & A != 0 {
        s.push_str("Alt+");
    }
    if key.mods & S != 0 {
        s.push_str("Shift+");
    }
    s.push_str(&key_name(key.vk));
    Some(s)
}

pub fn build_accelerators() -> HACCEL {
    let table: Vec<ACCEL> = KEYS
        .iter()
        .filter(|x| !x.native)
        .map(|x| {
            let mut f = FVIRTKEY;
            if x.mods & C != 0 {
                f |= FCONTROL;
            }
            if x.mods & S != 0 {
                f |= FSHIFT;
            }
            if x.mods & A != 0 {
                f |= FALT;
            }
            ACCEL {
                fVirt: f,
                key: x.vk,
                cmd: x.cmd,
            }
        })
        .collect();
    unsafe { CreateAcceleratorTableW(table.as_ptr(), table.len() as i32) }
}

// ---- 菜单 ----

/// 运行时需要填充或更新的子菜单。
#[derive(Clone, Copy, Debug)]
pub struct Menus {
    pub bar: HMENU,
    pub recent: HMENU,
    pub window: HMENU,
}

/// 窗口菜单中固定项的数量（其后是文档列表）。
pub const WINDOW_FIXED_ITEMS: i32 = 3;

pub enum Item {
    Cmd(u16, &'static str),
    Sep,
    Sub(&'static str, &'static [Item]),
    /// 动态子菜单，构建时为空。
    Recent(&'static str),
    EncReopen(&'static str),
    EncConvert(&'static str),
}
use Item::*;

const FILE: &[Item] = &[
    Cmd(IDM_FILE_NEW, "新建(&N)"),
    Cmd(IDM_FILE_OPEN, "打开(&O)..."),
    Recent("最近打开的文件(&R)"),
    Sep,
    Cmd(IDM_FILE_SAVE, "保存(&S)"),
    Cmd(IDM_FILE_SAVE_AS, "另存为(&A)..."),
    Cmd(IDM_FILE_SAVE_ALL, "全部保存(&L)"),
    Sep,
    Cmd(IDM_FILE_CLOSE, "关闭(&C)"),
    Cmd(IDM_FILE_CLOSE_ALL, "全部关闭"),
    Sep,
    Cmd(IDM_FILE_EXIT, "退出(&X)\tAlt+F4"),
];

const LINE_OPS: &[Item] = &[
    Cmd(IDM_LINE_DUPLICATE, "复制当前行(&D)"),
    Cmd(IDM_LINE_UP, "上移行(&U)"),
    Cmd(IDM_LINE_DOWN, "下移行(&W)"),
    Cmd(IDM_LINE_JOIN, "合并行(&J)"),
    Sep,
    Cmd(IDM_LINE_SORT_ASC, "升序排列"),
    Cmd(IDM_LINE_SORT_DESC, "降序排列"),
    Cmd(IDM_LINE_SORT_ASC_CI, "升序排列（忽略大小写）"),
    Cmd(IDM_LINE_SORT_DESC_CI, "降序排列（忽略大小写）"),
    Cmd(IDM_LINE_SORT_NUM_ASC, "按数值升序"),
    Cmd(IDM_LINE_SORT_NUM_DESC, "按数值降序"),
    Sep,
    Cmd(IDM_LINE_UNIQUE, "删除重复行"),
    Cmd(IDM_LINE_UNIQUE_ADJ, "删除相邻的重复行"),
    Cmd(IDM_LINE_REMOVE_EMPTY, "删除空行"),
    Cmd(IDM_LINE_REMOVE_BLANK, "删除空行（含只有空白的行）"),
    Sep,
    Cmd(IDM_LINE_TRIM_TRAILING, "删除行尾空白"),
    Cmd(IDM_LINE_TRIM_LEADING, "删除行首空白"),
];

const CASE: &[Item] = &[
    Cmd(IDM_EDIT_UPPER, "转为大写(&U)"),
    Cmd(IDM_EDIT_LOWER, "转为小写(&L)"),
];

const EOL: &[Item] = &[
    Cmd(IDM_EOL_CRLF, "Windows (CRLF)"),
    Cmd(IDM_EOL_LF, "Unix (LF)"),
    Cmd(IDM_EOL_CR, "Macintosh (CR)"),
];

const EDIT: &[Item] = &[
    Cmd(IDM_EDIT_UNDO, "撤销(&U)"),
    Cmd(IDM_EDIT_REDO, "重做(&R)"),
    Sep,
    Cmd(IDM_EDIT_CUT, "剪切(&T)"),
    Cmd(IDM_EDIT_COPY, "复制(&C)"),
    Cmd(IDM_EDIT_PASTE, "粘贴(&P)"),
    Cmd(IDM_EDIT_DELETE, "删除(&D)\tDel"),
    Cmd(IDM_EDIT_SELECT_ALL, "全选(&A)"),
    Sep,
    Sub("行操作(&L)", LINE_OPS),
    Sub("大小写(&V)", CASE),
    Sub("换行符转换(&E)", EOL),
    Sep,
    Cmd(IDM_EDIT_READ_ONLY, "只读模式(&O)"),
    Cmd(IDM_EDIT_LIST_CONTINUE, "智能列表续写"),
];

const BOOKMARKS: &[Item] = &[
    Cmd(IDM_BM_TOGGLE, "切换书签(&T)"),
    Cmd(IDM_BM_NEXT, "下一个书签(&N)"),
    Cmd(IDM_BM_PREV, "上一个书签(&P)"),
    Cmd(IDM_BM_CLEAR, "清除所有书签(&C)"),
    Cmd(IDM_BM_INVERT, "反转书签(&I)"),
    Sep,
    Cmd(IDM_BM_COPY, "复制书签行"),
    Cmd(IDM_BM_CUT, "剪切书签行"),
    Cmd(IDM_BM_DELETE, "删除书签行"),
    Cmd(IDM_BM_DELETE_UNMARKED, "删除未加书签的行"),
];

const SEARCH: &[Item] = &[
    Cmd(IDM_SEARCH_FIND, "查找(&F)..."),
    Cmd(IDM_SEARCH_REPLACE, "替换(&R)..."),
    Cmd(IDM_SEARCH_NEXT, "查找下一个(&N)"),
    Cmd(IDM_SEARCH_PREV, "查找上一个(&V)"),
    Cmd(IDM_SEARCH_SEL_NEXT, "查找选中内容"),
    Cmd(IDM_SEARCH_SEL_PREV, "向上查找选中内容"),
    Cmd(IDM_SEARCH_CLEAR_MARKS, "清除标记(&C)"),
    Sep,
    Cmd(IDM_SEARCH_GOTO, "跳转到行(&G)..."),
    Sep,
    Sub("书签(&B)", BOOKMARKS),
];

const VIEW: &[Item] = &[
    Cmd(IDM_VIEW_WRAP, "自动换行(&W)"),
    Cmd(IDM_VIEW_WHITESPACE, "显示空白字符(&S)"),
    Cmd(IDM_VIEW_EOL, "显示换行符(&E)"),
    Cmd(IDM_VIEW_INDENT_GUIDES, "显示缩进参考线(&I)"),
    Cmd(IDM_VIEW_LINE_NUMBERS, "显示行号(&L)"),
    Sep,
    Cmd(IDM_VIEW_ZOOM_IN, "放大(&+)"),
    Cmd(IDM_VIEW_ZOOM_OUT, "缩小(&-)"),
    Cmd(IDM_VIEW_ZOOM_RESET, "恢复默认缩放(&0)"),
    Sep,
    Cmd(IDM_VIEW_FOLD_ALL, "折叠全部"),
    Cmd(IDM_VIEW_UNFOLD_ALL, "展开全部"),
];

const ENCODING: &[Item] = &[EncReopen("重新打开为(&R)"), EncConvert("转换为(&C)")];

const WINDOW: &[Item] = &[
    Cmd(IDM_WIN_NEXT, "下一个文档(&N)"),
    Cmd(IDM_WIN_PREV, "上一个文档(&P)"),
    Sep,
];

const HELP: &[Item] = &[Cmd(IDM_HELP_ABOUT, "关于 HurshNote(&A)")];

fn append(menu: HMENU, flags: u32, id: usize, text: &str) {
    let w = wide(text);
    unsafe { AppendMenuW(menu, flags, id, w.as_ptr()) };
}

/// 菜单项文字：标签 + 快捷键。
fn label(id: u16, text: &str) -> String {
    match shortcut_text(id) {
        Some(sc) if !text.contains('\t') => format!("{text}\t{sc}"),
        _ => text.to_string(),
    }
}

/// 编码列表子菜单（重新打开 / 转换）。
pub fn encoding_menu(base: u16) -> HMENU {
    let m = unsafe { CreatePopupMenu() };
    for (i, c) in hn_text::encoding::catalog().iter().enumerate() {
        append(m, MF_STRING, (base + i as u16) as usize, c.label);
        if i == 4 {
            append(m, MF_SEPARATOR, 0, "");
        }
    }
    m
}

/// 语言列表菜单。
pub fn language_menu() -> HMENU {
    let m = unsafe { CreatePopupMenu() };
    for (i, l) in hn_text::lang::LANGS.iter().enumerate() {
        append(m, MF_STRING, (IDM_LANG_BASE + i as u16) as usize, l.name);
        if i == 0 {
            append(m, MF_SEPARATOR, 0, "");
        }
    }
    m
}

/// 缩放预设菜单。
pub fn zoom_menu() -> HMENU {
    let m = unsafe { CreatePopupMenu() };
    for (i, z) in ZOOM_PRESETS.iter().enumerate() {
        append(
            m,
            MF_STRING,
            (IDM_ZOOM_BASE + i as u16) as usize,
            &format!("{z}%"),
        );
    }
    m
}

/// 换行符菜单。
pub fn eol_menu() -> HMENU {
    popup(EOL, &mut None)
}

fn popup(items: &[Item], recent: &mut Option<HMENU>) -> HMENU {
    let m = unsafe { CreatePopupMenu() };
    for it in items {
        match it {
            Cmd(id, text) => append(m, MF_STRING, *id as usize, &label(*id, text)),
            Sep => append(m, MF_SEPARATOR, 0, ""),
            Sub(text, sub) => {
                let s = popup(sub, recent);
                append(m, MF_POPUP, s as usize, text);
            }
            Recent(text) => {
                let s = unsafe { CreatePopupMenu() };
                *recent = Some(s);
                append(m, MF_POPUP, s as usize, text);
            }
            EncReopen(text) => append(
                m,
                MF_POPUP,
                encoding_menu(IDM_ENC_REOPEN_BASE) as usize,
                text,
            ),
            EncConvert(text) => append(
                m,
                MF_POPUP,
                encoding_menu(IDM_ENC_CONVERT_BASE) as usize,
                text,
            ),
        }
    }
    m
}

/// 构建主菜单栏。
pub fn build_main_menu() -> Menus {
    let bar = unsafe { CreateMenu() };
    let mut recent = None;
    let mut window = std::ptr::null_mut();
    let tops: [(&str, &[Item]); 8] = [
        ("文件(&F)", FILE),
        ("编辑(&E)", EDIT),
        ("搜索(&S)", SEARCH),
        ("视图(&V)", VIEW),
        ("编码(&N)", ENCODING),
        ("语言(&L)", &[]),
        ("窗口(&W)", WINDOW),
        ("帮助(&H)", HELP),
    ];
    for (title, items) in tops {
        let sub = if title.starts_with("语言") {
            language_menu()
        } else {
            popup(items, &mut recent)
        };
        if title.starts_with("窗口") {
            window = sub;
        }
        append(bar, MF_POPUP, sub as usize, title);
    }
    Menus {
        bar,
        recent: recent.unwrap_or(std::ptr::null_mut()),
        window,
    }
}

/// 编辑区右键菜单（调用方负责 DestroyMenu）。
pub fn build_context_menu() -> HMENU {
    const CTX: &[Item] = &[
        Cmd(IDM_EDIT_UNDO, "撤销(&U)"),
        Cmd(IDM_EDIT_REDO, "重做(&R)"),
        Sep,
        Cmd(IDM_EDIT_CUT, "剪切(&T)"),
        Cmd(IDM_EDIT_COPY, "复制(&C)"),
        Cmd(IDM_EDIT_PASTE, "粘贴(&P)"),
        Cmd(IDM_EDIT_DELETE, "删除(&D)\tDel"),
        Sep,
        Cmd(IDM_EDIT_SELECT_ALL, "全选(&A)"),
        Sep,
        Cmd(IDM_BM_TOGGLE, "切换书签"),
        Sub("大小写", CASE),
    ];
    popup(CTX, &mut None)
}
