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
pub const IDM_FILE_SAVE_ENCRYPTED: u16 = 1010;
pub const IDM_SET_SNAPSHOTS: u16 = 1011;
pub const IDM_SET_AUTO_RELOAD: u16 = 1012;
pub const IDM_FILE_QUICK_OPEN: u16 = 1013;
pub const IDM_WS_ADD: u16 = 1014;
pub const IDM_SET_GITIGNORE: u16 = 1015;
pub const IDM_FILE_PRINT: u16 = 1016;
pub const IDM_FILE_CHANGE_PASSWORD: u16 = 1017;
pub const IDM_FILE_SAVE_DECRYPTED: u16 = 1018;

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
pub const IDM_SEARCH_RESULTS: u16 = 1409;
pub const IDM_SEARCH_IN_FILES: u16 = 1410;

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
pub const IDM_VIEW_SIDEBAR: u16 = 1611;
pub const IDM_VIEW_SPLIT_NONE: u16 = 1612;
pub const IDM_VIEW_SPLIT_H: u16 = 1613;
pub const IDM_VIEW_SPLIT_V: u16 = 1614;
pub const IDM_VIEW_SWITCH_FOCUS: u16 = 1615;
pub const IDM_VIEW_MOVE_OTHER: u16 = 1616;
pub const IDM_VIEW_HEX: u16 = 1617;
pub const IDM_VIEW_TAIL: u16 = 1618;
pub const IDM_VIEW_MD_PREVIEW: u16 = 1619;

// ---- 运行 / 终端 ----
pub const IDM_RUN_PYTHON: u16 = 1801;
pub const IDM_RUN_STOP: u16 = 1802;
pub const IDM_RUN_IN_TERMINAL: u16 = 1803;
pub const IDM_RUN_CLEAR_OUTPUT: u16 = 1804;
pub const IDM_VIEW_OUTPUT: u16 = 1805;
pub const IDM_TERM_TOGGLE: u16 = 1810;
pub const IDM_TERM_NEW: u16 = 1811;
pub const IDM_TERM_NEW_POWERSHELL: u16 = 1812;
pub const IDM_TERM_NEW_CMD: u16 = 1813;
pub const IDM_TERM_NEW_PWSH: u16 = 1814;
pub const IDM_TERM_CLOSE: u16 = 1815;
pub const IDM_DRAWER_HIDE: u16 = 1816;

// ---- 宏 ----
pub const IDM_MACRO_RECORD: u16 = 1950;
pub const IDM_MACRO_PLAY: u16 = 1951;
pub const IDM_MACRO_PLAY_MULTI: u16 = 1952;

// ---- 窗口 / 帮助 ----
pub const IDM_WIN_NEXT: u16 = 1701;
pub const IDM_WIN_PREV: u16 = 1702;
pub const IDM_HELP_ABOUT: u16 = 1901;

// ---- 侧边栏右键菜单 ----
pub const IDM_CTX_CLOSE: u16 = 4001;
pub const IDM_CTX_CLOSE_OTHERS: u16 = 4002;
pub const IDM_CTX_COPY_PATH: u16 = 4003;
pub const IDM_CTX_COPY_NAME: u16 = 4004;
pub const IDM_CTX_REVEAL: u16 = 4005;
pub const IDM_TREE_OPEN: u16 = 4101;
pub const IDM_TREE_NEW_FILE: u16 = 4102;
pub const IDM_TREE_NEW_DIR: u16 = 4103;
pub const IDM_TREE_RENAME: u16 = 4104;
pub const IDM_TREE_DELETE: u16 = 4105;
pub const IDM_TREE_COPY_PATH: u16 = 4106;
pub const IDM_TREE_REVEAL: u16 = 4107;
pub const IDM_TREE_REFRESH: u16 = 4108;
pub const IDM_TREE_REMOVE_ROOT: u16 = 4109;

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
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_F5: u16 = 0x74;
const VK_F6: u16 = 0x75;
const VK_F7: u16 = 0x76;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;
const VK_ADD: u16 = 0x6B;
const VK_SUBTRACT: u16 = 0x6D;
const VK_DIVIDE: u16 = 0x6F;
const VK_F2: u16 = 0x71;
const VK_F3: u16 = 0x72;
const VK_OEM_PLUS: u16 = 0xBB;
const VK_OEM_MINUS: u16 = 0xBD;
/// 美式键盘上的 ` ~ 键
pub const VK_OEM_3: u16 = 0xC0;

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
    k(IDM_FILE_QUICK_OPEN, C, ch(b'P')),
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
    k(IDM_WIN_NEXT, C, VK_NEXT),
    k(IDM_WIN_PREV, C, VK_PRIOR),
    k(IDM_VIEW_SWITCH_FOCUS, 0, VK_F6),
    k(IDM_VIEW_SIDEBAR, C | S, ch(b'B')),
    k(IDM_VIEW_HEX, C | S, ch(b'H')),
    k(IDM_SEARCH_RESULTS, 0, VK_F7),
    k(IDM_SEARCH_IN_FILES, C | S, ch(b'F')),
    k(IDM_RUN_PYTHON, 0, VK_F5),
    k(IDM_RUN_STOP, S, VK_F5),
    k(IDM_RUN_IN_TERMINAL, C, VK_F5),
    k(IDM_TERM_TOGGLE, C, VK_OEM_3),
    k(IDM_TERM_NEW, C | S, VK_OEM_3),
    k(IDM_VIEW_MD_PREVIEW, C | S, ch(b'M')),
    k(IDM_MACRO_RECORD, C | S, ch(b'R')),
    k(IDM_MACRO_PLAY, C | S, ch(b'P')),
];

fn key_name(vk: u16) -> String {
    match vk {
        VK_TAB => "Tab".into(),
        VK_PRIOR => "PgUp".into(),
        VK_NEXT => "PgDn".into(),
        VK_UP => "Up".into(),
        VK_DOWN => "Down".into(),
        VK_ADD => "Num +".into(),
        VK_SUBTRACT => "Num -".into(),
        VK_DIVIDE => "Num /".into(),
        VK_OEM_PLUS => "=".into(),
        VK_OEM_MINUS => "-".into(),
        VK_OEM_3 => "`".into(),
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
    Cmd(IDM_FILE_QUICK_OPEN, "快速打开(&Q)..."),
    Cmd(IDM_WS_ADD, "添加文件夹到工作区(&K)..."),
    Recent("最近打开的文件(&R)"),
    Sep,
    Cmd(IDM_FILE_SAVE, "保存(&S)"),
    Cmd(IDM_FILE_SAVE_AS, "另存为(&A)..."),
    Cmd(IDM_FILE_SAVE_ALL, "全部保存(&L)"),
    Sub("加密(&E)", ENCRYPTION),
    Sep,
    Cmd(IDM_FILE_PRINT, "打印(&P)..."),
    Sep,
    Cmd(IDM_FILE_CLOSE, "关闭(&C)"),
    Cmd(IDM_FILE_CLOSE_ALL, "全部关闭"),
    Sep,
    Sub("设置(&T)", SETTINGS),
    Sep,
    Cmd(IDM_FILE_EXIT, "退出(&X)\tAlt+F4"),
];

const ENCRYPTION: &[Item] = &[
    Cmd(IDM_FILE_SAVE_ENCRYPTED, "另存为加密文件(&E)..."),
    Cmd(IDM_FILE_CHANGE_PASSWORD, "修改密码(&W)..."),
    Cmd(IDM_FILE_SAVE_DECRYPTED, "另存为不加密的文件(&D)..."),
];

const SETTINGS: &[Item] = &[
    Cmd(
        IDM_SET_SNAPSHOTS,
        "会话快照：关闭时不询问，下次启动恢复未保存内容",
    ),
    Cmd(
        IDM_SET_AUTO_RELOAD,
        "文件被外部修改时自动重新载入（未修改的文档）",
    ),
    Cmd(IDM_SET_GITIGNORE, "工作区与快速打开遵守 .gitignore"),
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
    Cmd(IDM_SEARCH_IN_FILES, "在文件中查找(&I)..."),
    Cmd(IDM_SEARCH_RESULTS, "搜索结果面板(&S)"),
    Sep,
    Sub("书签(&B)", BOOKMARKS),
];

const SPLIT: &[Item] = &[
    Cmd(IDM_VIEW_SPLIT_NONE, "不分屏(&N)"),
    Cmd(IDM_VIEW_SPLIT_H, "左右分屏(&H)"),
    Cmd(IDM_VIEW_SPLIT_V, "上下分屏(&V)"),
    Sep,
    Cmd(IDM_VIEW_SWITCH_FOCUS, "切换到另一视图(&S)"),
    Cmd(IDM_VIEW_MOVE_OTHER, "在另一视图显示当前文档(&O)"),
];

const VIEW: &[Item] = &[
    Cmd(IDM_VIEW_SIDEBAR, "侧边栏(&B)"),
    Sub("分屏(&P)", SPLIT),
    Sep,
    Cmd(IDM_VIEW_MD_PREVIEW, "Markdown 预览(&M)"),
    Cmd(IDM_VIEW_HEX, "Hex 视图(&X)"),
    Cmd(IDM_VIEW_TAIL, "Tail 模式：跟随文件末尾(&T)"),
    Sep,
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

const TERMINALS: &[Item] = &[
    Cmd(IDM_TERM_NEW_POWERSHELL, "PowerShell"),
    Cmd(IDM_TERM_NEW_PWSH, "PowerShell 7（pwsh）"),
    Cmd(IDM_TERM_NEW_CMD, "命令提示符（cmd）"),
];

const RUN: &[Item] = &[
    Cmd(IDM_RUN_PYTHON, "运行 Python(&R)"),
    Cmd(IDM_RUN_IN_TERMINAL, "在终端中运行(&T)"),
    Cmd(IDM_RUN_STOP, "停止(&S)"),
    Cmd(IDM_RUN_CLEAR_OUTPUT, "清空输出(&C)"),
    Sep,
    Cmd(IDM_TERM_TOGGLE, "显示/隐藏终端(&O)"),
    Cmd(IDM_TERM_NEW, "新建终端(&N)"),
    Sub("新建指定终端(&W)", TERMINALS),
    Cmd(IDM_TERM_CLOSE, "关闭当前终端(&L)"),
    Sep,
    Cmd(IDM_VIEW_OUTPUT, "输出面板(&P)"),
    Cmd(IDM_DRAWER_HIDE, "隐藏底部面板(&H)"),
];

const MACRO: &[Item] = &[
    Cmd(IDM_MACRO_RECORD, "录制宏(&R)"),
    Cmd(IDM_MACRO_PLAY, "回放(&P)"),
    Cmd(IDM_MACRO_PLAY_MULTI, "多次回放(&M)..."),
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
    let tops: [(&str, &[Item]); 10] = [
        ("文件(&F)", FILE),
        ("编辑(&E)", EDIT),
        ("搜索(&S)", SEARCH),
        ("视图(&V)", VIEW),
        ("运行(&R)", RUN),
        ("宏(&M)", MACRO),
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

/// 大文件视图 / Hex 视图的右键菜单（调用方负责 DestroyMenu）。
pub fn build_large_context_menu() -> HMENU {
    const CTX: &[Item] = &[
        Cmd(IDM_EDIT_COPY, "复制(&C)\tCtrl+C"),
        Cmd(IDM_EDIT_SELECT_ALL, "全选(&A)\tCtrl+A"),
        Sep,
        Cmd(IDM_SEARCH_FIND, "查找(&F)..."),
        Cmd(IDM_SEARCH_GOTO, "跳转到行(&G)..."),
        Sep,
        Cmd(IDM_VIEW_HEX, "Hex 视图(&X)"),
        Cmd(IDM_VIEW_TAIL, "Tail 模式(&T)"),
    ];
    popup(CTX, &mut None)
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
