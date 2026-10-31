//! 主题与基础外观。
//!
//! M0 只设置默认样式、行号栏、光标、当前行、选区。语义样式与各 lexer 的映射在 M1 加入。

use crate::*;

/// 调色板（RGB，0xRRGGBB）。
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// 编辑区背景
    pub bg0: u32,
    /// 行号栏背景
    pub bg0_s: u32,
    /// 当前行
    pub bg1: u32,
    /// 选区
    pub bg2: u32,
    /// 正文
    pub fg1: u32,
    /// 注释、行号
    pub gray: u32,
    pub red: u32,
    pub green: u32,
    pub yellow: u32,
    pub blue: u32,
    pub purple: u32,
    pub aqua: u32,
    pub orange: u32,
}

/// Gruvbox Light（medium）。
pub const GRUVBOX_LIGHT: Palette = Palette {
    bg0: 0xfbf1c7,
    bg0_s: 0xf2e5bc,
    bg1: 0xebdbb2,
    bg2: 0xd5c4a1,
    fg1: 0x3c3836,
    gray: 0x928374,
    red: 0x9d0006,
    green: 0x79740e,
    yellow: 0xb57614,
    blue: 0x076678,
    purple: 0x8f3f71,
    aqua: 0x427b58,
    orange: 0xaf3a03,
};

/// 0xRRGGBB → Scintilla 的 0x00BBGGRR。
pub const fn bgr(rgb: u32) -> isize {
    (((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)) as isize
}

/// 0xRRGGBB → 不透明的 0xFFBBGGRR（元素颜色 API 使用）。
pub const fn bgra(rgb: u32) -> isize {
    bgr(rgb) | 0xff00_0000
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

/// 每个新建的 Scintilla 视图都要调用一次：渲染方式、编码、性能相关选项。
pub fn init_view(sci: &Sci) {
    sci.send(SCI_SETTECHNOLOGY, SC_TECHNOLOGY_DIRECTWRITE as usize, 0);
    sci.send(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0);
    // DirectWrite 自带缓冲，关闭 Scintilla 的双缓冲。
    sci.send(SCI_SETBUFFEREDDRAW, 0, 0);
    sci.send(SCI_SETLAYOUTCACHE, SC_CACHE_PAGE as usize, 0);
    sci.send(SCI_SETSCROLLWIDTH, 1, 0);
    sci.send(SCI_SETSCROLLWIDTHTRACKING, 1, 0);
    sci.send(SCI_SETTABWIDTH, 4, 0);
    // 右键菜单由程序自己提供（中文）。
    sci.send(SCI_USEPOPUP, SC_POPUP_NEVER as usize, 0);
}

/// 应用基础外观。换主题或换 lexer 后需重新调用。
pub fn apply_base(sci: &Sci, p: &Palette, font: &FontSpec) {
    let def = STYLE_DEFAULT as usize;
    sci.send_str(SCI_STYLESETFONT, def, &font.face);
    sci.send(SCI_STYLESETSIZEFRACTIONAL, def, font.size_x100 as isize);
    sci.send(SCI_STYLESETFORE, def, bgr(p.fg1));
    sci.send(SCI_STYLESETBACK, def, bgr(p.bg0));
    sci.send(SCI_STYLECLEARALL, 0, 0);

    // 行号栏
    let ln = STYLE_LINENUMBER as usize;
    sci.send(SCI_STYLESETFORE, ln, bgr(p.gray));
    sci.send(SCI_STYLESETBACK, ln, bgr(p.bg0_s));

    // 括号匹配（M1 接入逻辑，这里先定样式）
    sci.send(SCI_STYLESETFORE, STYLE_BRACELIGHT as usize, bgr(p.blue));
    sci.send(SCI_STYLESETBACK, STYLE_BRACELIGHT as usize, bgr(p.bg2));
    sci.send(SCI_STYLESETBOLD, STYLE_BRACELIGHT as usize, 1);
    sci.send(SCI_STYLESETFORE, STYLE_BRACEBAD as usize, bgr(p.red));
    sci.send(SCI_STYLESETFORE, STYLE_INDENTGUIDE as usize, bgr(p.bg2));

    // 边栏：0 行号，1 留白，2 折叠（M1）
    sci.send(SCI_SETMARGINTYPEN, 0, SC_MARGIN_NUMBER);
    update_line_number_width(sci);
    sci.send(SCI_SETMARGINTYPEN, 1, SC_MARGIN_COLOUR);
    sci.send(SCI_SETMARGINBACKN, 1, bgr(p.bg0));
    sci.send(SCI_SETMARGINWIDTHN, 1, 6);
    sci.send(SCI_SETMARGINWIDTHN, 2, 0);

    // 光标与当前行
    sci.send(SCI_SETELEMENTCOLOUR, SC_ELEMENT_CARET as usize, bgra(p.fg1));
    sci.send(SCI_SETCARETWIDTH, 2, 0);
    sci.send(
        SCI_SETELEMENTCOLOUR,
        SC_ELEMENT_CARET_LINE_BACK as usize,
        bgra(p.bg1),
    );
    sci.send(SCI_SETCARETLINEVISIBLEALWAYS, 1, 0);

    // 选区
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
        bgra(0xe2d3b0),
    );
}

/// 按当前行数调整行号栏宽度（至少能放下 4 位数字）。
pub fn update_line_number_width(sci: &Sci) {
    let lines = sci.send(SCI_GETLINECOUNT, 0, 0).max(1);
    let digits = (lines.ilog10() + 1).max(4) as usize;
    let sample = format!("_{}", "9".repeat(digits));
    let w = sci.send_str(SCI_TEXTWIDTH, STYLE_LINENUMBER as usize, &sample);
    sci.send(SCI_SETMARGINWIDTHN, 0, w);
}
