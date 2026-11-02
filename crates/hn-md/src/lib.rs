//! Markdown 预览的解析部分：把源码解析为**与平台无关的扁平块列表**。
//!
//! 预览窗口（`hn-app`）只需要把块从上到下排开：每个块带有列表嵌套层数（`indent`）、
//! 引用嵌套层数（`quote`）和列表标记（`marker`），嵌套结构已经展开，绘制时不必再递归。
//!
//! - 解析用 `pulldown-cmark`（CommonMark + GFM 表格、任务列表、删除线、脚注）
//! - 每个块记录源码行号，用于编辑器与预览的滚动同步
//! - 行内样式是互不重叠的样式段（[`Span`]），链接地址集中在 [`Document::links`]
//! - 本地图片单独成块；远程图片、表格里和链接里的图片显示为替代文字
//! - 内嵌 HTML 按原文显示（`<br>` 换行，HTML 注释隐藏）；YAML 头信息不显示

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// 行内样式（位标志）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style(pub u8);

impl Style {
    pub const BOLD: Style = Style(1);
    pub const ITALIC: Style = Style(2);
    pub const STRIKE: Style = Style(4);
    pub const CODE: Style = Style(8);
    pub const LINK: Style = Style(16);
    /// 按原文显示的内嵌 HTML
    pub const HTML: Style = Style(32);
    /// 替代文字（显示不了的图片）
    pub const ALT: Style = Style(64);

    pub fn has(self, f: Style) -> bool {
        self.0 & f.0 != 0
    }

    pub fn with(self, f: Style) -> Style {
        Style(self.0 | f.0)
    }

    pub fn is_plain(self) -> bool {
        self.0 == 0
    }
}

/// 一段样式：`[start, end)` 为 [`Inline::text`] 中的字节范围。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
    /// [`Document::links`] 中的下标
    pub link: Option<u32>,
}

/// 一段带样式的文字。`spans` 按位置排序、互不重叠，只覆盖有样式或链接的部分。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Inline {
    pub text: String,
    pub spans: Vec<Span>,
}

/// UTF-16 下标表示的样式段（DirectWrite 按 UTF-16 码元计位置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf16Span {
    pub start: u32,
    pub end: u32,
    pub style: Style,
    pub link: Option<u32>,
}

impl Inline {
    /// 转为 UTF-16 文本，样式段的位置换算为 UTF-16 下标。
    pub fn to_utf16(&self) -> (Vec<u16>, Vec<Utf16Span>) {
        let mut wide = Vec::with_capacity(self.text.len());
        // 样式段的边界（字节偏移）→ UTF-16 下标
        let mut bounds: Vec<usize> = self.spans.iter().flat_map(|s| [s.start, s.end]).collect();
        bounds.sort_unstable();
        bounds.dedup();
        let mut map = HashMap::with_capacity(bounds.len());
        let mut bi = 0;
        for (i, ch) in self.text.char_indices() {
            while bi < bounds.len() && bounds[bi] <= i {
                map.insert(bounds[bi], wide.len() as u32);
                bi += 1;
            }
            let mut buf = [0u16; 2];
            wide.extend_from_slice(ch.encode_utf16(&mut buf));
        }
        while bi < bounds.len() {
            map.insert(bounds[bi], wide.len() as u32);
            bi += 1;
        }
        let spans = self
            .spans
            .iter()
            .map(|s| Utf16Span {
                start: map[&s.start],
                end: map[&s.end],
                style: s.style,
                link: s.link,
            })
            .collect();
        (wide, spans)
    }

    /// 字节偏移处的样式段。
    pub fn span_at(&self, byte: usize) -> Option<&Span> {
        self.spans.iter().find(|s| s.start <= byte && byte < s.end)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Table {
    pub aligns: Vec<Align>,
    pub head: Vec<Inline>,
    pub rows: Vec<Vec<Inline>>,
}

impl Table {
    /// 列数（表头与各行中最多的单元格数）。
    pub fn columns(&self) -> usize {
        self.rows
            .iter()
            .map(Vec::len)
            .chain([self.head.len(), self.aligns.len()])
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    Para(Inline),
    /// 标题级别 1–6
    Heading(u8, Inline),
    Code {
        lang: String,
        text: String,
    },
    /// 按原文显示的 HTML 块
    Html(String),
    Rule,
    Table(Table),
    /// 本地图片（`src` 为 Markdown 中的原始地址，由调用方按文件目录解析）
    Image {
        src: String,
        alt: String,
    },
}

/// 列表项第一个块前的标记。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Marker {
    /// 无序列表，参数为嵌套深度（0 起），决定符号样式
    Bullet(u8),
    Number(u64),
    Task(bool),
    /// 脚注定义
    Footnote(String),
}

impl Marker {
    /// 显示与复制用的文字。
    pub fn label(&self) -> String {
        match self {
            Marker::Bullet(d) => ["•", "◦", "▪"][*d as usize % 3].to_string(),
            Marker::Number(n) => format!("{n}."),
            Marker::Task(true) => "☑".to_string(),
            Marker::Task(false) => "☐".to_string(),
            Marker::Footnote(l) => format!("[{l}]"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Block {
    pub kind: Kind,
    /// 源码中的首行（0 起）
    pub line: usize,
    /// 源码中的末行（含）
    pub last_line: usize,
    /// 列表（与脚注）嵌套层数；标记画在最后一层缩进里
    pub indent: u16,
    /// 引用嵌套层数
    pub quote: u8,
    pub marker: Option<Marker>,
    /// 紧凑列表中的段落（与上一项之间不留段间距）
    pub compact: bool,
}

impl Block {
    /// 与源码位置无关的内容哈希（用于复用排版结果：在上方插入一行不应让下面的块重新排版）。
    pub fn content_hash(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.kind.hash(&mut h);
        self.indent.hash(&mut h);
        self.quote.hash(&mut h);
        self.marker.hash(&mut h);
        self.compact.hash(&mut h);
        h.finish()
    }

    /// 复制用的纯文本（含列表标记）。
    pub fn plain_text(&self) -> String {
        let body = match &self.kind {
            Kind::Para(i) | Kind::Heading(_, i) => i.text.clone(),
            Kind::Code { text, .. } | Kind::Html(text) => text.clone(),
            Kind::Rule => "---".to_string(),
            Kind::Image { src, alt } => {
                if alt.is_empty() {
                    src.clone()
                } else {
                    alt.clone()
                }
            }
            Kind::Table(t) => {
                let mut s = String::new();
                let row = |cells: &[Inline], s: &mut String| {
                    let v: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
                    s.push_str(&v.join("\t"));
                    s.push('\n');
                };
                row(&t.head, &mut s);
                for r in &t.rows {
                    row(r, &mut s);
                }
                s.pop();
                s
            }
        };
        match &self.marker {
            Some(m) => format!("{} {body}", m.label()),
            None => body,
        }
    }
}

/// 解析结果。
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// 链接地址（[`Span::link`] 指向这里）
    pub links: Vec<String>,
    /// 锚点（标题的 GitHub 风格 slug、脚注 `^标签`）→ 块下标
    pub anchors: Vec<(String, usize)>,
}

impl Document {
    /// 源码第 `line` 行（0 起）所在或之前最近的块。
    pub fn block_at_line(&self, line: usize) -> Option<usize> {
        let n = self.blocks.partition_point(|b| b.line <= line);
        n.checked_sub(1)
    }

    /// 页内链接 `#xxx` 指向的块。
    pub fn find_anchor(&self, name: &str) -> Option<usize> {
        let name = name.strip_prefix('#').unwrap_or(name);
        let want = slug(&percent_decode(name));
        self.anchors
            .iter()
            .find(|(a, _)| *a == want || a.as_str() == name)
            .map(|(_, i)| *i)
    }
}

/// 链接地址的种类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// http / https / mailto
    Web(String),
    /// 页内锚点（不含 #）
    Anchor(String),
    /// 本地文件（相对或绝对路径，已去掉 `#片段` 并解码 %xx），由调用方按文件目录解析
    File(String),
    /// 其他协议（javascript:、file: 之外的未知协议等），不打开
    Unsupported,
}

/// 判断链接地址的种类。只放行 http(s)、mailto、页内锚点与本地路径。
pub fn classify_link(url: &str) -> LinkTarget {
    let u = url.trim();
    if let Some(a) = u.strip_prefix('#') {
        return LinkTarget::Anchor(a.to_string());
    }
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
    {
        return LinkTarget::Web(u.to_string());
    }
    if let Some(rest) = lower.strip_prefix("file:") {
        // file:///C:/a/b.md → C:/a/b.md
        let rest = &u[u.len() - rest.len()..];
        let path = rest.trim_start_matches('/');
        let path = if path.as_bytes().get(1) == Some(&b':') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        return LinkTarget::File(percent_decode(strip_fragment(&path)));
    }
    // 形如 "scheme:" 的其他协议（单个字母视为盘符）
    if let Some(colon) = u.find(':') {
        let scheme = &u[..colon];
        if scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return LinkTarget::Unsupported;
        }
    }
    if u.is_empty() {
        return LinkTarget::Unsupported;
    }
    LinkTarget::File(percent_decode(strip_fragment(u)))
}

fn strip_fragment(s: &str) -> &str {
    match s.find(['#', '?']) {
        Some(i) => &s[..i],
        None => s,
    }
}

/// 图片地址是否为远程地址（v1 不加载远程图片）。
pub fn is_remote(src: &str) -> bool {
    let l = src.trim().to_ascii_lowercase();
    l.starts_with("http://")
        || l.starts_with("https://")
        || l.starts_with("//")
        || l.starts_with("data:")
}

/// 解码 `%xx`（无效序列原样保留）。
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// GitHub 风格的标题锚点：小写，保留字母数字（含中文）、`-`、`_`，空格换成 `-`，其余去掉。
pub fn slug(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            s.extend(c.to_lowercase());
        } else if c == ' ' {
            s.push('-');
        }
    }
    s
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF
        | 0x20000..=0x2FA1F)
}

// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Para,
    Heading(u8),
    Cell,
}

struct InlineCtx {
    target: Target,
    line: usize,
    last_line: usize,
    text: String,
    spans: Vec<Span>,
    styles: Vec<Style>,
    links: Vec<u32>,
    /// 等待决定是否插入空格的软换行
    soft: bool,
    /// 没有 Paragraph 标签的段落（紧凑列表项）
    implicit: bool,
}

impl InlineCtx {
    fn new(target: Target, line: usize, implicit: bool) -> InlineCtx {
        InlineCtx {
            target,
            line,
            last_line: line,
            text: String::new(),
            spans: Vec::new(),
            styles: Vec::new(),
            links: Vec::new(),
            soft: false,
            implicit,
        }
    }

    fn style(&self) -> Style {
        self.styles.iter().fold(Style::default(), |a, s| a.with(*s))
    }

    fn push(&mut self, s: &str, extra: Style, link: Option<u32>) {
        if s.is_empty() {
            return;
        }
        if self.soft {
            self.soft = false;
            let prev = self.text.chars().next_back();
            let next = s.chars().next();
            let cjk = prev.is_some_and(is_cjk) && next.is_some_and(is_cjk);
            if !cjk && !self.text.is_empty() {
                self.push_raw(" ", self.style(), self.links.last().copied());
            }
        }
        let style = self.style().with(extra);
        let link = link.or_else(|| self.links.last().copied());
        self.push_raw(s, style, link);
    }

    fn push_raw(&mut self, s: &str, style: Style, link: Option<u32>) {
        let start = self.text.len();
        self.text.push_str(s);
        let end = self.text.len();
        if style.is_plain() && link.is_none() {
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            if last.end == start && last.style == style && last.link == link {
                last.end = end;
                return;
            }
        }
        self.spans.push(Span {
            start,
            end,
            style,
            link,
        });
    }

    fn finish(self) -> Inline {
        Inline {
            text: self.text,
            spans: self.spans,
        }
    }
}

struct TableCtx {
    aligns: Vec<Align>,
    head: Vec<Inline>,
    rows: Vec<Vec<Inline>>,
    row: Vec<Inline>,
    in_head: bool,
    line: usize,
}

struct ImageCtx {
    src: String,
    alt: String,
}

struct Builder<'s> {
    src: &'s str,
    line_starts: Vec<usize>,
    doc: Document,
    quote: u8,
    /// 各层列表的下一个编号（无序列表为 None）
    lists: Vec<Option<u64>>,
    /// 脚注定义的额外缩进
    footnote: u16,
    /// 等待放到下一个块上的列表标记
    marker: Option<Marker>,
    /// 等待指向下一个块的锚点
    pending_anchor: Option<String>,
    inline: Option<InlineCtx>,
    code: Option<(String, String, usize)>,
    html: Option<(String, usize)>,
    table: Option<TableCtx>,
    image: Option<ImageCtx>,
    metadata: bool,
    slugs: HashMap<String, usize>,
}

/// 解析 Markdown 源码。
pub fn parse(src: &str) -> Document {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    let mut line_starts = vec![0];
    line_starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let mut b = Builder {
        src,
        line_starts,
        doc: Document::default(),
        quote: 0,
        lists: Vec::new(),
        footnote: 0,
        marker: None,
        pending_anchor: None,
        inline: None,
        code: None,
        html: None,
        table: None,
        image: None,
        metadata: false,
        slugs: HashMap::new(),
    };
    for (ev, range) in Parser::new_ext(src, opts).into_offset_iter() {
        b.event(ev, range);
    }
    b.close_inline();
    b.doc
}

impl Builder<'_> {
    fn line_of(&self, off: usize) -> usize {
        let off = off.min(self.src.len());
        self.line_starts
            .partition_point(|&s| s <= off)
            .saturating_sub(1)
    }

    /// 范围的末行（含）。
    fn last_line_of(&self, r: &Range<usize>) -> usize {
        let end = if r.end > r.start { r.end - 1 } else { r.start };
        // 范围通常包含结尾的换行符，换行符属于它前面那一行
        self.line_of(end)
    }

    fn indent(&self) -> u16 {
        self.lists.len() as u16 + self.footnote
    }

    fn push_block(&mut self, kind: Kind, line: usize, last_line: usize, compact: bool) {
        let idx = self.doc.blocks.len();
        if let Some(a) = self.pending_anchor.take() {
            self.doc.anchors.push((a, idx));
        }
        let block = Block {
            kind,
            line,
            last_line: last_line.max(line),
            indent: self.indent(),
            quote: self.quote,
            marker: self.marker.take(),
            compact,
        };
        self.doc.blocks.push(block);
    }

    /// 结束当前段落（或标题），生成块。空段落不生成块（列表标记留给下一个块）。
    fn close_inline(&mut self) {
        let Some(ctx) = self.inline.take() else {
            return;
        };
        let (target, line, last, implicit) = (ctx.target, ctx.line, ctx.last_line, ctx.implicit);
        let inline = ctx.finish();
        match target {
            Target::Heading(level) => {
                let base = slug(&inline.text);
                let n = self.slugs.entry(base.clone()).or_insert(0);
                let name = if *n == 0 { base } else { format!("{base}-{n}") };
                *n += 1;
                self.pending_anchor = Some(name);
                self.push_block(Kind::Heading(level, inline), line, last, false);
            }
            Target::Para => {
                if inline.text.trim().is_empty() {
                    return;
                }
                let compact = implicit && !self.lists.is_empty();
                self.push_block(Kind::Para(inline), line, last, compact);
            }
            Target::Cell => {
                if let Some(t) = self.table.as_mut() {
                    if t.in_head {
                        t.head.push(inline);
                    } else {
                        t.row.push(inline);
                    }
                }
            }
        }
    }

    /// 取得当前的行内上下文；没有时开始一个隐式段落（紧凑列表项中的文字没有 Paragraph 标签）。
    fn inline_mut(&mut self, range: &Range<usize>) -> &mut InlineCtx {
        if self.inline.is_none() {
            let line = self.line_of(range.start);
            self.inline = Some(InlineCtx::new(Target::Para, line, true));
        }
        let last = self.last_line_of(range);
        let ctx = self.inline.as_mut().unwrap();
        if ctx.target != Target::Cell {
            ctx.last_line = ctx.last_line.max(last);
        }
        ctx
    }

    fn event(&mut self, ev: Event<'_>, range: Range<usize>) {
        if self.metadata {
            if matches!(ev, Event::End(TagEnd::MetadataBlock(_))) {
                self.metadata = false;
            }
            return;
        }
        if let Some(img) = self.image.as_mut() {
            match ev {
                Event::Text(t) | Event::Code(t) => {
                    img.alt.push_str(&t);
                    return;
                }
                Event::End(TagEnd::Image) => {
                    let img = self.image.take().unwrap();
                    self.finish_image(img, &range);
                    return;
                }
                Event::SoftBreak | Event::HardBreak => {
                    img.alt.push(' ');
                    return;
                }
                // 替代文字中的强调等标签忽略
                Event::Start(_) | Event::End(_) => return,
                _ => return,
            }
        }
        if let Some((_, text, _)) = self.code.as_mut() {
            match ev {
                Event::Text(t) => {
                    text.push_str(&t);
                    return;
                }
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, mut text, line) = self.code.take().unwrap();
                    if text.ends_with('\n') {
                        text.pop();
                        if text.ends_with('\r') {
                            text.pop();
                        }
                    }
                    let last = self.last_line_of(&range);
                    self.push_block(Kind::Code { lang, text }, line, last, false);
                    return;
                }
                _ => return,
            }
        }
        if let Some((text, _)) = self.html.as_mut() {
            match ev {
                Event::Html(t) | Event::Text(t) | Event::InlineHtml(t) => {
                    text.push_str(&t);
                    return;
                }
                Event::End(TagEnd::HtmlBlock) => {
                    let (mut text, line) = self.html.take().unwrap();
                    let stripped = strip_html_comments(&text);
                    if stripped.trim().is_empty() {
                        return;
                    }
                    while text.ends_with(['\n', '\r']) {
                        text.pop();
                    }
                    let last = self.last_line_of(&range);
                    self.push_block(Kind::Html(text), line, last, false);
                    return;
                }
                _ => return,
            }
        }

        match ev {
            Event::Start(tag) => self.start(tag, range),
            Event::End(tag) => self.end(tag, range),
            Event::Text(t) => {
                self.inline_mut(&range).push(&t, Style::default(), None);
            }
            Event::Code(t) => {
                self.inline_mut(&range).push(&t, Style::CODE, None);
            }
            Event::InlineHtml(t) | Event::Html(t) => {
                let tag = t.trim().to_ascii_lowercase().replace(' ', "");
                let ctx = self.inline_mut(&range);
                if matches!(tag.as_str(), "<br>" | "<br/>") {
                    ctx.soft = false;
                    ctx.push_raw("\n", Style::default(), None);
                } else if !tag.starts_with("<!--") {
                    ctx.push(&t, Style::HTML, None);
                }
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                self.inline_mut(&range).push(&t, Style::CODE, None);
            }
            Event::FootnoteReference(label) => {
                let idx = self.doc.links.len() as u32;
                self.doc.links.push(format!("#^{label}"));
                self.inline_mut(&range)
                    .push(&format!("[{label}]"), Style::LINK, Some(idx));
            }
            Event::SoftBreak => {
                if let Some(ctx) = self.inline.as_mut() {
                    ctx.soft = true;
                }
            }
            Event::HardBreak => {
                let ctx = self.inline_mut(&range);
                ctx.soft = false;
                ctx.push_raw("\n", Style::default(), None);
            }
            Event::Rule => {
                self.close_inline();
                let line = self.line_of(range.start);
                self.push_block(Kind::Rule, line, line, false);
            }
            Event::TaskListMarker(done) => {
                self.marker = Some(Marker::Task(done));
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        let line = self.line_of(range.start);
        match tag {
            Tag::Paragraph => {
                self.close_inline();
                self.inline = Some(InlineCtx::new(Target::Para, line, false));
            }
            Tag::Heading { level, .. } => {
                self.close_inline();
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                let mut ctx = InlineCtx::new(Target::Heading(n), line, false);
                ctx.last_line = self.last_line_of(&range);
                self.inline = Some(ctx);
            }
            Tag::BlockQuote(_) => {
                self.close_inline();
                self.quote = self.quote.saturating_add(1);
            }
            Tag::CodeBlock(kind) => {
                self.close_inline();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new(), line));
            }
            Tag::HtmlBlock => {
                self.close_inline();
                self.html = Some((String::new(), line));
            }
            Tag::List(start) => {
                self.close_inline();
                self.lists.push(start);
            }
            Tag::Item => {
                self.close_inline();
                let depth = self.lists.len().saturating_sub(1);
                let m = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = Marker::Number(*n);
                        *n += 1;
                        m
                    }
                    _ => Marker::Bullet(depth.min(255) as u8),
                };
                self.marker = Some(m);
            }
            Tag::FootnoteDefinition(label) => {
                self.close_inline();
                self.footnote += 1;
                self.marker = Some(Marker::Footnote(label.to_string()));
                self.pending_anchor = Some(format!("^{label}"));
            }
            Tag::Table(aligns) => {
                self.close_inline();
                let aligns = aligns
                    .iter()
                    .map(|a| match a {
                        Alignment::None => Align::None,
                        Alignment::Left => Align::Left,
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                    })
                    .collect();
                self.table = Some(TableCtx {
                    aligns,
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    in_head: false,
                    line,
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.row.clear();
                }
            }
            Tag::TableCell => {
                self.inline = Some(InlineCtx::new(Target::Cell, line, false));
            }
            Tag::Emphasis => self.inline_mut(&range).styles.push(Style::ITALIC),
            Tag::Strong => self.inline_mut(&range).styles.push(Style::BOLD),
            Tag::Strikethrough => self.inline_mut(&range).styles.push(Style::STRIKE),
            Tag::Link { dest_url, .. } => {
                let idx = self.doc.links.len() as u32;
                self.doc.links.push(dest_url.to_string());
                let ctx = self.inline_mut(&range);
                ctx.styles.push(Style::LINK);
                ctx.links.push(idx);
            }
            Tag::Image { dest_url, .. } => {
                self.image = Some(ImageCtx {
                    src: dest_url.to_string(),
                    alt: String::new(),
                });
            }
            Tag::MetadataBlock(_) => self.metadata = true,
            Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => {}
        }
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>) {
        match tag {
            TagEnd::Paragraph => {
                let last = self.last_line_of(&range);
                if let Some(ctx) = self.inline.as_mut() {
                    ctx.last_line = ctx.last_line.max(last);
                }
                self.close_inline();
            }
            TagEnd::Heading(_) => self.close_inline(),
            TagEnd::BlockQuote(_) => {
                self.close_inline();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.close_inline();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.close_inline();
                if self.marker.is_some() {
                    // 空列表项：仍然显示标记
                    let line = self.line_of(range.start);
                    self.push_block(Kind::Para(Inline::default()), line, line, true);
                }
            }
            TagEnd::FootnoteDefinition => {
                self.close_inline();
                self.marker = None;
                self.footnote = self.footnote.saturating_sub(1);
            }
            TagEnd::TableCell => self.close_inline(),
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    let last = self.last_line_of(&range);
                    let table = Table {
                        aligns: t.aligns,
                        head: t.head,
                        rows: t.rows,
                    };
                    self.push_block(Kind::Table(table), t.line, last, false);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                if let Some(ctx) = self.inline.as_mut() {
                    ctx.styles.pop();
                }
            }
            TagEnd::Link => {
                if let Some(ctx) = self.inline.as_mut() {
                    ctx.styles.pop();
                    ctx.links.pop();
                }
            }
            _ => {}
        }
    }

    /// 图片结束：本地图片单独成块，其余显示为替代文字。
    fn finish_image(&mut self, img: ImageCtx, range: &Range<usize>) {
        let in_cell = self
            .inline
            .as_ref()
            .is_some_and(|c| c.target == Target::Cell);
        let in_link = self.inline.as_ref().is_some_and(|c| !c.links.is_empty());
        let in_heading = self
            .inline
            .as_ref()
            .is_some_and(|c| matches!(c.target, Target::Heading(_)));
        if is_remote(&img.src) || img.src.trim().is_empty() || in_cell || in_link || in_heading {
            let alt = if img.alt.trim().is_empty() {
                "图片".to_string()
            } else {
                img.alt
            };
            let link = if in_link {
                None
            } else {
                let idx = self.doc.links.len() as u32;
                self.doc.links.push(img.src);
                Some(idx)
            };
            self.inline_mut(range)
                .push(&format!("[{alt}]"), Style::ALT, link);
            return;
        }
        // 本地图片：结束前面的文字，图片单独成块，之后的文字另起一块
        let line = self.line_of(range.start);
        let (implicit, had_inline) = match self.inline.as_ref() {
            Some(c) => (c.implicit, true),
            None => (true, false),
        };
        self.close_inline();
        self.push_block(
            Kind::Image {
                src: img.src,
                alt: img.alt,
            },
            line,
            line,
            false,
        );
        if had_inline {
            self.inline = Some(InlineCtx::new(Target::Para, line, implicit));
        }
    }
}

/// 去掉 `<!-- ... -->` 注释。
fn strip_html_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i + 4..].find("-->") {
            Some(j) => rest = &rest[i + 4 + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
