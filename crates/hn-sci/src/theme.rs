//! 主题：调色板、语义样式、视图外观。
//!
//! Lexilla 各 lexer 的样式编号互不相同。主题只为"语义样式"（[`Sem`]）定义外观，
//! 再由 [`crate::langs`] 把每个 lexer 的样式号映射到语义样式。
//!
//! Scintilla 中：lexer、关键字、属性、EOL、只读属于**文档**；样式与边栏属于**视图**。
//! 因此切换文档后要对视图重新调用 [`apply_styles`]。

use crate::langs;
use crate::*;

/// 书签标记号（避开修改历史 21–24 与折叠 25–31）。
pub const MARK_BOOKMARK: usize = 18;
/// "标记全部"使用的指示器。
pub const INDIC_FIND: usize = 8;

pub const MARGIN_NUMBERS: usize = 0;
pub const MARGIN_BOOKMARKS: usize = 1;
pub const MARGIN_FOLD: usize = 2;

/// 调色板（RGB，0xRRGGBB）。
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg0: u32,
    pub bg0_s: u32,
    pub bg1: u32,
    pub bg2: u32,
    pub bg3: u32,
    pub fg1: u32,
    pub fg3: u32,
    pub gray: u32,
    pub red: u32,
    pub green: u32,
    pub yellow: u32,
    pub blue: u32,
    pub purple: u32,
    pub aqua: u32,
    pub orange: u32,
    /// 高亮背景（标记全部）
    pub highlight: u32,
}

/// Gruvbox Light（medium）。
pub const GRUVBOX_LIGHT: Palette = Palette {
    bg0: 0xfbf1c7,
    bg0_s: 0xf2e5bc,
    bg1: 0xebdbb2,
    bg2: 0xd5c4a1,
    bg3: 0xbdae93,
    fg1: 0x3c3836,
    fg3: 0x665c54,
    gray: 0x928374,
    red: 0x9d0006,
    green: 0x79740e,
    yellow: 0xb57614,
    blue: 0x076678,
    purple: 0x8f3f71,
    aqua: 0x427b58,
    orange: 0xaf3a03,
    highlight: 0xfabd2f,
};

/// 0xRRGGBB → Scintilla 的 0x00BBGGRR。
pub const fn bgr(rgb: u32) -> isize {
    (((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)) as isize
}

/// 0xRRGGBB → 不透明的 0xFFBBGGRR（元素颜色 API 使用）。
pub const fn bgra(rgb: u32) -> isize {
    bgr(rgb) | 0xff00_0000
}

/// 0xRRGGBB + alpha → 0xAABBGGRR。
pub const fn bgr_alpha(rgb: u32, alpha: u8) -> isize {
    bgr(rgb) | ((alpha as isize) << 24)
}

/// 语义样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sem {
    Default,
    Comment,
    CommentDoc,
    Keyword,
    Keyword2,
    Type,
    String,
    Char,
    Escape,
    Number,
    Constant,
    Operator,
    Function,
    Variable,
    Preprocessor,
    Tag,
    Attribute,
    Entity,
    Key,
    Section,
    Heading,
    Emphasis,
    Strong,
    Strike,
    Code,
    Link,
    Quote,
    ListMarker,
    Label,
    Regex,
    Error,
    Added,
    Deleted,
    Changed,
    DiffHeader,
    TaskMarker,
}

#[derive(Debug, Clone, Copy, Default)]
struct StyleSpec {
    fore: Option<u32>,
    back: Option<u32>,
    bold: bool,
    italic: bool,
    underline: bool,
}

fn spec(p: &Palette, s: Sem) -> StyleSpec {
    let fg = |c| StyleSpec {
        fore: Some(c),
        ..Default::default()
    };
    let bold = |c| StyleSpec {
        fore: Some(c),
        bold: true,
        ..Default::default()
    };
    let italic = |c| StyleSpec {
        fore: Some(c),
        italic: true,
        ..Default::default()
    };
    match s {
        Sem::Default => fg(p.fg1),
        Sem::Comment | Sem::CommentDoc => italic(p.gray),
        Sem::Keyword => fg(p.red),
        Sem::Keyword2 => fg(p.orange),
        Sem::Type => fg(p.yellow),
        Sem::String | Sem::Char => fg(p.green),
        Sem::Escape => fg(p.orange),
        Sem::Number | Sem::Constant => fg(p.purple),
        Sem::Operator => fg(p.fg3),
        Sem::Function => fg(p.blue),
        Sem::Variable => fg(p.blue),
        Sem::Preprocessor => fg(p.aqua),
        Sem::Tag => fg(p.blue),
        Sem::Attribute => fg(p.aqua),
        Sem::Entity => fg(p.orange),
        Sem::Key => fg(p.blue),
        Sem::Section => bold(p.yellow),
        Sem::Heading => bold(p.blue),
        Sem::Emphasis => StyleSpec {
            italic: true,
            ..Default::default()
        },
        Sem::Strong => StyleSpec {
            bold: true,
            ..Default::default()
        },
        Sem::Strike => fg(p.gray),
        Sem::Code => StyleSpec {
            fore: Some(p.aqua),
            back: Some(p.bg0_s),
            ..Default::default()
        },
        Sem::Link => StyleSpec {
            fore: Some(p.blue),
            underline: true,
            ..Default::default()
        },
        Sem::Quote => italic(p.fg3),
        Sem::ListMarker => fg(p.orange),
        Sem::Label => fg(p.purple),
        Sem::Regex => fg(p.aqua),
        Sem::Error => StyleSpec {
            fore: Some(p.red),
            underline: true,
            ..Default::default()
        },
        Sem::Added => fg(p.green),
        Sem::Deleted => fg(p.red),
        Sem::Changed => fg(p.yellow),
        Sem::DiffHeader => bold(p.blue),
        Sem::TaskMarker => bold(p.orange),
    }
}

/// 字体设置。
#[derive(Debug, Clone)]
pub struct FontSpec {
    pub face: String,
    /// 字号，单位 1/100 磅（1100 = 11pt）。
    pub size_x100: i32,
}

impl Default for FontSpec {
    fn default() -> Self {
        FontSpec {
            face: "Consolas".to_string(),
            size_x100: 1100,
        }
    }
}

/// 按 DPI 缩放像素值（Scintilla 的边栏宽度等以像素为单位，不会自动缩放）。
fn px(v: i32, dpi: u32) -> isize {
    (v as i64 * dpi as i64 / 96) as isize
}

/// 新建视图时调用一次（DPI 变化时再调用）：渲染方式、行为选项、边栏、标记与指示器。
pub fn init_view(sci: &Sci, p: &Palette, dpi: u32) {
    sci.send(SCI_SETTECHNOLOGY, SC_TECHNOLOGY_DIRECTWRITE as usize, 0);
    // DirectWrite 自带缓冲，关闭 Scintilla 的双缓冲。
    sci.send(SCI_SETBUFFEREDDRAW, 0, 0);
    sci.send(SCI_SETLAYOUTCACHE, SC_CACHE_PAGE as usize, 0);
    sci.send(SCI_SETSCROLLWIDTH, 1, 0);
    sci.send(SCI_SETSCROLLWIDTHTRACKING, 1, 0);
    sci.send(SCI_SETTABWIDTH, 4, 0);
    // 右键菜单由程序自己提供（中文）。
    sci.send(SCI_USEPOPUP, SC_POPUP_NEVER as usize, 0);
    // 不需要 SCN_MODIFIED，减少通知开销（保存点通知不受影响）。
    sci.send(SCI_SETMODEVENTMASK, SC_MOD_NONE as usize, 0);

    // 多光标、列选
    sci.send(SCI_SETMULTIPLESELECTION, 1, 0);
    sci.send(SCI_SETADDITIONALSELECTIONTYPING, 1, 0);
    sci.send(SCI_SETMULTIPASTE, SC_MULTIPASTE_EACH as usize, 0);
    sci.send(
        SCI_SETVIRTUALSPACEOPTIONS,
        SCVS_RECTANGULARSELECTION as usize,
        0,
    );
    sci.send(SCI_SETMOUSESELECTIONRECTANGULARSWITCH, 1, 0);

    // 缩放交给程序处理（小数字号），去掉 Scintilla 自带的 Ctrl+小键盘缩放
    for key in [SCK_ADD, SCK_SUBTRACT, SCK_DIVIDE] {
        sci.send(SCI_CLEARCMDKEY, (key | (SCMOD_CTRL << 16)) as usize, 0);
    }

    // 边栏
    sci.send(SCI_SETMARGINTYPEN, MARGIN_NUMBERS, SC_MARGIN_NUMBER);
    sci.send(SCI_SETMARGINTYPEN, MARGIN_BOOKMARKS, SC_MARGIN_SYMBOL);
    sci.send(SCI_SETMARGINMASKN, MARGIN_BOOKMARKS, 1 << MARK_BOOKMARK);
    sci.send(SCI_SETMARGINWIDTHN, MARGIN_BOOKMARKS, px(14, dpi));
    sci.send(SCI_SETMARGINSENSITIVEN, MARGIN_BOOKMARKS, 1);
    sci.send(SCI_SETMARGINTYPEN, MARGIN_FOLD, SC_MARGIN_SYMBOL);
    sci.send(SCI_SETMARGINMASKN, MARGIN_FOLD, SC_MASK_FOLDERS);
    sci.send(SCI_SETMARGINWIDTHN, MARGIN_FOLD, px(14, dpi));
    sci.send(SCI_SETMARGINSENSITIVEN, MARGIN_FOLD, 1);
    sci.send(SCI_SETMARGINLEFT, 0, px(2, dpi));

    // 书签
    sci.send(SCI_MARKERDEFINE, MARK_BOOKMARK, SC_MARK_BOOKMARK);
    sci.send(SCI_MARKERSETFORE, MARK_BOOKMARK, bgr(p.blue));
    sci.send(SCI_MARKERSETBACK, MARK_BOOKMARK, bgr(p.blue));

    // 折叠：方框树形
    let folds = [
        (SC_MARKNUM_FOLDEROPEN, SC_MARK_BOXMINUS),
        (SC_MARKNUM_FOLDER, SC_MARK_BOXPLUS),
        (SC_MARKNUM_FOLDERSUB, SC_MARK_VLINE),
        (SC_MARKNUM_FOLDERTAIL, SC_MARK_LCORNER),
        (SC_MARKNUM_FOLDEREND, SC_MARK_BOXPLUSCONNECTED),
        (SC_MARKNUM_FOLDEROPENMID, SC_MARK_BOXMINUSCONNECTED),
        (SC_MARKNUM_FOLDERMIDTAIL, SC_MARK_TCORNER),
    ];
    for (num, sym) in folds {
        sci.send(SCI_MARKERDEFINE, num as usize, sym);
        sci.send(SCI_MARKERSETFORE, num as usize, bgr(p.bg0));
        sci.send(SCI_MARKERSETBACK, num as usize, bgr(p.gray));
    }
    sci.send(SCI_SETFOLDMARGINCOLOUR, 1, bgr(p.bg0));
    sci.send(SCI_SETFOLDMARGINHICOLOUR, 1, bgr(p.bg0));
    sci.send(
        SCI_SETAUTOMATICFOLD,
        (SC_AUTOMATICFOLD_SHOW | SC_AUTOMATICFOLD_CLICK | SC_AUTOMATICFOLD_CHANGE) as usize,
        0,
    );
    sci.send(
        SCI_SETFOLDFLAGS,
        SC_FOLDFLAG_LINEAFTER_CONTRACTED as usize,
        0,
    );

    // 标记全部
    sci.send(SCI_INDICSETSTYLE, INDIC_FIND, INDIC_ROUNDBOX);
    sci.send(SCI_INDICSETFORE, INDIC_FIND, bgr(p.highlight));
    sci.send(SCI_INDICSETALPHA, INDIC_FIND, 110);
    sci.send(SCI_INDICSETOUTLINEALPHA, INDIC_FIND, 200);
    sci.send(SCI_INDICSETUNDER, INDIC_FIND, 1);

    // 光标、当前行、选区、空白字符
    sci.send(SCI_SETELEMENTCOLOUR, SC_ELEMENT_CARET as usize, bgra(p.fg1));
    sci.send(SCI_SETCARETWIDTH, px(2, dpi) as usize, 0);
    sci.send(
        SCI_SETELEMENTCOLOUR,
        SC_ELEMENT_CARET_LINE_BACK as usize,
        bgra(p.bg0_s),
    );
    sci.send(SCI_SETCARETLINEVISIBLEALWAYS, 1, 0);
    sci.send(
        SCI_SETELEMENTCOLOUR,
        SC_ELEMENT_SELECTION_BACK as usize,
        bgra(p.bg2),
    );
    sci.send(
        SCI_SETELEMENTCOLOUR,
        SC_ELEMENT_SELECTION_ADDITIONAL_BACK as usize,
        bgra(p.bg2),
    );
    sci.send(
        SCI_SETELEMENTCOLOUR,
        SC_ELEMENT_SELECTION_INACTIVE_BACK as usize,
        bgra(p.bg1),
    );
    sci.send(
        SCI_SETELEMENTCOLOUR,
        SC_ELEMENT_WHITE_SPACE as usize,
        bgr_alpha(p.gray, 160),
    );
    sci.send(SCI_SETWHITESPACESIZE, px(2, dpi) as usize, 0);
}

/// 设置当前文档的语言：lexer、关键字、属性，并重新着色。
pub fn set_language(sci: &Sci, lang_id: &str, lexer: &str) {
    let conf = langs::conf(lang_id);
    if !sci.set_lexer(lexer) {
        sci.set_lexer("null");
    }
    for (i, words) in conf.keywords.iter().enumerate() {
        sci.send_str(SCI_SETKEYWORDS, i, words);
    }
    for (k, v) in langs::COMMON_PROPS.iter().chain(conf.props.iter()) {
        let key = std::ffi::CString::new(*k).unwrap();
        let val = std::ffi::CString::new(*v).unwrap();
        sci.send(
            SCI_SETPROPERTY,
            key.as_ptr() as usize,
            val.as_ptr() as isize,
        );
    }
    sci.send(SCI_COLOURISE, 0, -1);
}

/// 应用样式（视图级）。切换文档、换语言、缩放后调用。
pub fn apply_styles(sci: &Sci, p: &Palette, font: &FontSpec, zoom_pct: u32, lang_id: &str) {
    let def = STYLE_DEFAULT as usize;
    let size = (font.size_x100 as i64 * zoom_pct as i64 / 100).max(300) as isize;
    sci.send_str(SCI_STYLESETFONT, def, &font.face);
    sci.send(SCI_STYLESETSIZEFRACTIONAL, def, size);
    sci.send(SCI_STYLESETFORE, def, bgr(p.fg1));
    sci.send(SCI_STYLESETBACK, def, bgr(p.bg0));
    sci.send(SCI_STYLECLEARALL, 0, 0);

    let ln = STYLE_LINENUMBER as usize;
    sci.send(SCI_STYLESETFORE, ln, bgr(p.gray));
    sci.send(SCI_STYLESETBACK, ln, bgr(p.bg0_s));

    sci.send(SCI_STYLESETFORE, STYLE_BRACELIGHT as usize, bgr(p.red));
    sci.send(SCI_STYLESETBACK, STYLE_BRACELIGHT as usize, bgr(p.bg2));
    sci.send(SCI_STYLESETBOLD, STYLE_BRACELIGHT as usize, 1);
    sci.send(SCI_STYLESETFORE, STYLE_BRACEBAD as usize, bgr(p.bg0));
    sci.send(SCI_STYLESETBACK, STYLE_BRACEBAD as usize, bgr(p.red));
    sci.send(SCI_STYLESETFORE, STYLE_INDENTGUIDE as usize, bgr(p.bg3));
    sci.send(SCI_STYLESETFORE, STYLE_CONTROLCHAR as usize, bgr(p.orange));

    for &(style, sem) in langs::conf(lang_id).styles {
        let s = spec(p, sem);
        let st = style as usize;
        if let Some(c) = s.fore {
            sci.send(SCI_STYLESETFORE, st, bgr(c));
        }
        if let Some(c) = s.back {
            sci.send(SCI_STYLESETBACK, st, bgr(c));
            sci.send(SCI_STYLESETEOLFILLED, st, 1);
        }
        if s.bold {
            sci.send(SCI_STYLESETBOLD, st, 1);
        }
        if s.italic {
            sci.send(SCI_STYLESETITALIC, st, 1);
        }
        if s.underline {
            sci.send(SCI_STYLESETUNDERLINE, st, 1);
        }
    }
}

/// 按当前行数调整行号栏宽度（至少能放下 4 位数字）。`visible` 为假时隐藏。
pub fn update_line_number_width(sci: &Sci, visible: bool) {
    if !visible {
        sci.send(SCI_SETMARGINWIDTHN, MARGIN_NUMBERS, 0);
        return;
    }
    let lines = sci.send(SCI_GETLINECOUNT, 0, 0).max(1);
    let digits = (lines.ilog10() + 1).max(4) as usize;
    let sample = format!("_{}", "9".repeat(digits));
    let w = sci.send_str(SCI_TEXTWIDTH, STYLE_LINENUMBER as usize, &sample);
    sci.send(SCI_SETMARGINWIDTHN, MARGIN_NUMBERS, w);
}
