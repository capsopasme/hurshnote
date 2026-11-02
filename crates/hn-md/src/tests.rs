use super::*;

fn para(b: &Block) -> &Inline {
    match &b.kind {
        Kind::Para(i) | Kind::Heading(_, i) => i,
        k => panic!("不是段落：{k:?}"),
    }
}

#[test]
fn headings_and_paragraphs_with_lines() {
    let d = parse("# 标题\n\n第一段\n第二行\n\n## Sub *x*\n");
    assert_eq!(d.blocks.len(), 3);
    assert!(matches!(d.blocks[0].kind, Kind::Heading(1, _)));
    assert_eq!(para(&d.blocks[0]).text, "标题");
    assert_eq!((d.blocks[0].line, d.blocks[0].last_line), (0, 0));
    // 中文之间的软换行不加空格
    assert_eq!(para(&d.blocks[1]).text, "第一段第二行");
    assert_eq!((d.blocks[1].line, d.blocks[1].last_line), (2, 3));
    assert_eq!(d.blocks[2].line, 5);
    assert_eq!(d.find_anchor("#标题"), Some(0));
    assert_eq!(d.find_anchor("#sub-x"), Some(2));
}

#[test]
fn soft_break_between_latin_words_is_space() {
    let d = parse("hello\nworld\n");
    assert_eq!(para(&d.blocks[0]).text, "hello world");
    let d = parse("a  \nb\n");
    assert_eq!(para(&d.blocks[0]).text, "a\nb");
}

#[test]
fn inline_styles_are_merged_runs() {
    let d = parse("a **b *c*** `d` ~~e~~ [f](http://x.com)\n");
    let i = para(&d.blocks[0]);
    assert_eq!(i.text, "a b c d e f");
    let styled: Vec<(&str, Style, Option<u32>)> = i
        .spans
        .iter()
        .map(|s| (&i.text[s.start..s.end], s.style, s.link))
        .collect();
    assert_eq!(
        styled,
        vec![
            ("b ", Style::BOLD, None),
            ("c", Style::BOLD.with(Style::ITALIC), None),
            ("d", Style::CODE, None),
            ("e", Style::STRIKE, None),
            ("f", Style::LINK, Some(0)),
        ]
    );
    assert_eq!(d.links, vec!["http://x.com".to_string()]);
}

#[test]
fn nested_lists_flatten_with_markers() {
    let src = "- a\n- b\n  1. x\n  2. y\n- [x] done\n- [ ] todo\n";
    let d = parse(src);
    let got: Vec<(String, u16, Option<Marker>, bool)> = d
        .blocks
        .iter()
        .map(|b| (para(b).text.clone(), b.indent, b.marker.clone(), b.compact))
        .collect();
    assert_eq!(
        got,
        vec![
            ("a".into(), 1, Some(Marker::Bullet(0)), true),
            ("b".into(), 1, Some(Marker::Bullet(0)), true),
            ("x".into(), 2, Some(Marker::Number(1)), true),
            ("y".into(), 2, Some(Marker::Number(2)), true),
            ("done".into(), 1, Some(Marker::Task(true)), true),
            ("todo".into(), 1, Some(Marker::Task(false)), true),
        ]
    );
    assert_eq!(d.blocks[3].line, 3);
}

#[test]
fn loose_list_item_with_two_paragraphs() {
    let d = parse("3. one\n\n   more\n\n4. two\n");
    assert_eq!(d.blocks.len(), 3);
    assert_eq!(d.blocks[0].marker, Some(Marker::Number(3)));
    assert!(!d.blocks[0].compact);
    assert_eq!(d.blocks[1].marker, None);
    assert_eq!(d.blocks[1].indent, 1);
    assert_eq!(d.blocks[2].marker, Some(Marker::Number(4)));
}

#[test]
fn empty_list_item_keeps_marker() {
    let d = parse("- \n- b\n");
    assert_eq!(d.blocks.len(), 2);
    assert_eq!(d.blocks[0].marker, Some(Marker::Bullet(0)));
    assert_eq!(para(&d.blocks[0]).text, "");
}

#[test]
fn quotes_and_code() {
    let d = parse("> q1\n>\n> > q2\n\n```rust title\nfn main() {}\n\n```\n\n    indented\n");
    assert_eq!(d.blocks[0].quote, 1);
    assert_eq!(d.blocks[1].quote, 2);
    match &d.blocks[2].kind {
        Kind::Code { lang, text } => {
            assert_eq!(lang, "rust");
            assert_eq!(text, "fn main() {}\n");
        }
        k => panic!("{k:?}"),
    }
    assert_eq!((d.blocks[2].line, d.blocks[2].last_line), (4, 7));
    match &d.blocks[3].kind {
        Kind::Code { lang, text } => {
            assert_eq!(lang, "");
            assert_eq!(text, "indented");
        }
        k => panic!("{k:?}"),
    }
}

#[test]
fn table() {
    let d = parse("| a | b |\n|:--|--:|\n| 1 | **2** |\n| 3 |\n");
    let Kind::Table(t) = &d.blocks[0].kind else {
        panic!()
    };
    assert_eq!(t.aligns, vec![Align::Left, Align::Right]);
    assert_eq!(t.head.len(), 2);
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0][1].text, "2");
    assert_eq!(t.rows[0][1].spans[0].style, Style::BOLD);
    assert_eq!(t.columns(), 2);
    assert_eq!((d.blocks[0].line, d.blocks[0].last_line), (0, 3));
    // 缺少的单元格由解析器补成空单元格
    assert_eq!(t.rows[1].len(), 2);
    assert_eq!(d.blocks[0].plain_text(), "a\tb\n1\t2\n3\t");
}

#[test]
fn images_local_block_remote_inline() {
    let d = parse("前 ![图](img/a.png) 后\n\n![b](https://x/b.png)\n\n[![c](c.png)](http://l)\n");
    let kinds: Vec<String> = d
        .blocks
        .iter()
        .map(|b| match &b.kind {
            Kind::Para(i) => format!("P:{}", i.text),
            Kind::Image { src, alt } => format!("I:{src}:{alt}"),
            k => format!("{k:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["P:前 ", "I:img/a.png:图", "P: 后", "P:[b]", "P:[c]"]
    );
    // 远程图片的替代文字链接到图片地址；链接中的图片沿用外层链接
    let remote = para(&d.blocks[3]);
    assert_eq!(remote.spans[0].style, Style::ALT);
    assert_eq!(
        d.links[remote.spans[0].link.unwrap() as usize],
        "https://x/b.png"
    );
    let linked = para(&d.blocks[4]);
    assert!(linked.spans[0].style.has(Style::LINK));
    assert_eq!(d.links[linked.spans[0].link.unwrap() as usize], "http://l");
}

#[test]
fn image_as_only_content_of_list_item() {
    let d = parse("- ![x](a.png)\n- b\n");
    assert!(matches!(d.blocks[0].kind, Kind::Image { .. }));
    assert_eq!(d.blocks[0].marker, Some(Marker::Bullet(0)));
    assert_eq!(d.blocks[1].marker, Some(Marker::Bullet(0)));
}

#[test]
fn html_raw_comments_hidden_br_breaks() {
    let d = parse("<!-- hidden -->\n\n<div>\nraw\n</div>\n\na<br>b <span>c</span>\n");
    assert_eq!(d.blocks.len(), 2);
    match &d.blocks[0].kind {
        Kind::Html(t) => assert_eq!(t, "<div>\nraw\n</div>"),
        k => panic!("{k:?}"),
    }
    let i = para(&d.blocks[1]);
    assert_eq!(i.text, "a\nb <span>c</span>");
    assert!(i.spans.iter().all(|s| s.style == Style::HTML));
}

#[test]
fn front_matter_and_rule() {
    let d = parse("---\ntitle: x\n---\n\n正文\n\n***\n");
    assert_eq!(d.blocks.len(), 2);
    assert_eq!(para(&d.blocks[0]).text, "正文");
    assert_eq!(d.blocks[0].line, 4);
    assert_eq!(d.blocks[1].kind, Kind::Rule);
}

#[test]
fn footnotes() {
    let d = parse("文字[^n]\n\n[^n]: 注释\n");
    let i = para(&d.blocks[0]);
    assert_eq!(i.text, "文字[n]");
    let link = &d.links[i.spans[0].link.unwrap() as usize];
    assert_eq!(link, "#^n");
    let target = d.find_anchor(link).unwrap();
    assert_eq!(d.blocks[target].marker, Some(Marker::Footnote("n".into())));
    assert_eq!(para(&d.blocks[target]).text, "注释");
}

#[test]
fn duplicate_heading_slugs() {
    let d = parse("# A\n# A\n");
    assert_eq!(d.find_anchor("a"), Some(0));
    assert_eq!(d.find_anchor("a-1"), Some(1));
}

#[test]
fn block_at_line_for_scroll_sync() {
    let d = parse("# t\n\npara\nline\n\n- a\n- b\n");
    assert_eq!(d.block_at_line(0), Some(0));
    assert_eq!(d.block_at_line(1), Some(0));
    assert_eq!(d.block_at_line(3), Some(1));
    assert_eq!(d.block_at_line(6), Some(3));
    assert_eq!(d.block_at_line(100), Some(3));
    assert_eq!(parse("").block_at_line(0), None);
}

#[test]
fn crlf_lines() {
    let d = parse("a\r\n\r\nb\r\nc\r\n");
    assert_eq!((d.blocks[1].line, d.blocks[1].last_line), (2, 3));
    assert_eq!(para(&d.blocks[1]).text, "b c");
}

#[test]
fn utf16_spans() {
    let d = parse("😀**粗**x\n");
    let (w, spans) = para(&d.blocks[0]).to_utf16();
    assert_eq!(String::from_utf16(&w).unwrap(), "😀粗x");
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].start, spans[0].end), (2, 3));
}

#[test]
fn content_hash_ignores_lines() {
    let a = parse("x\n\npara\n");
    let b = parse("x\n\n\n\npara\n");
    assert_ne!(a.blocks[1].line, b.blocks[1].line);
    assert_eq!(a.blocks[1].content_hash(), b.blocks[1].content_hash());
}

#[test]
fn plain_text_with_marker() {
    let d = parse("1. a\n- [x] b\n");
    assert_eq!(d.blocks[0].plain_text(), "1. a");
    assert_eq!(d.blocks[1].plain_text(), "☑ b");
}

#[test]
fn link_classification() {
    assert_eq!(
        classify_link("https://a.b/c"),
        LinkTarget::Web("https://a.b/c".into())
    );
    assert_eq!(
        classify_link("MAILTO:x@y"),
        LinkTarget::Web("MAILTO:x@y".into())
    );
    assert_eq!(classify_link("#sec"), LinkTarget::Anchor("sec".into()));
    assert_eq!(
        classify_link("docs/a%20b.md#x"),
        LinkTarget::File("docs/a b.md".into())
    );
    assert_eq!(
        classify_link("file:///C:/x/y.txt"),
        LinkTarget::File("C:/x/y.txt".into())
    );
    assert_eq!(
        classify_link("C:\\x\\y.md"),
        LinkTarget::File("C:\\x\\y.md".into())
    );
    assert_eq!(
        classify_link("javascript:alert(1)"),
        LinkTarget::Unsupported
    );
    assert_eq!(
        classify_link("ms-settings:display"),
        LinkTarget::Unsupported
    );
    assert_eq!(classify_link(""), LinkTarget::Unsupported);
}

#[test]
fn percent_decoding() {
    assert_eq!(percent_decode("a%20b%e4%b8%ad"), "a b中");
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%zz"), "%zz");
}

#[test]
fn large_document_parses_quickly() {
    let mut s = String::new();
    for i in 0..5000 {
        s.push_str(&format!("## 第 {i} 节\n\n这是一段**中文**文字，包含 `code` 与 [链接](http://x/{i})。\n\n- 项目 {i}\n- 项目\n\n"));
    }
    let t = std::time::Instant::now();
    let d = parse(&s);
    let ms = t.elapsed().as_millis();
    assert_eq!(d.blocks.len(), 5000 * 4);
    // 调试构建下也应远小于 1 秒
    assert!(ms < 3000, "{ms} ms");
}

#[test]
fn malformed_input_does_not_panic() {
    for s in [
        "|",
        "| a |\n|",
        "*",
        "**",
        "[",
        "![",
        "![a](",
        "```",
        "> ",
        "- [",
        "<",
        "<!--",
        "\u{0}",
        "#",
        "1.",
        "* * *",
        "[^",
        "[^x]:",
        "|a|\n|-|\n|![i](x.png)|",
    ] {
        let _ = parse(s);
    }
}
