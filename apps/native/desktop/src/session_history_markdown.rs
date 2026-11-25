use std::{collections::HashMap, iter::Peekable, ops::Range};

use gpui::SharedString;
use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, OffsetIter, Options, Parser, Tag, TagEnd,
};

const MAX_PARAGRAPH_CHARS: usize = 8_000;
const MAX_LIST_DEPTH: usize = 24;

#[derive(Debug)]
pub(super) enum Block {
    Paragraph(Paragraph),
    Heading {
        level: HeadingLevel,
        paragraph: Paragraph,
    },
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Table {
        alignments: Vec<Alignment>,
        rows: Vec<Vec<Paragraph>>,
    },
    Code {
        text: SharedString,
        untyped: bool,
        tight_before: bool,
    },
    Literal(SharedString),
}

#[derive(Debug)]
pub(super) struct Paragraph {
    pub(super) text: SharedString,
    pub(super) spans: Vec<Span>,
    pub(super) links: Vec<(Range<usize>, SharedString)>,
}

#[derive(Debug)]
pub(super) struct Span {
    pub(super) range: Range<usize>,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) code: bool,
    pub(super) link: bool,
}

#[derive(Default)]
pub(super) struct Cache {
    bodies: HashMap<(i64, bool), Body>,
    #[cfg(any(debug_assertions, test))]
    parses: usize,
}

pub(super) struct Body {
    pub(super) blocks: Vec<Block>,
    pub(super) clipped: bool,
}

impl Cache {
    pub(super) fn insert(&mut self, index: i64, expanded: bool, text: &str, clipped: bool) {
        self.bodies.entry((index, expanded)).or_insert_with(|| {
            #[cfg(any(debug_assertions, test))]
            {
                self.parses += 1;
            }
            Body {
                blocks: parse(text, clipped),
                clipped,
            }
        });
    }

    pub(super) fn get(&self, index: i64, expanded: bool) -> Option<&Body> {
        self.bodies.get(&(index, expanded))
    }

    #[cfg(any(debug_assertions, test))]
    pub(super) fn parses(&self) -> usize {
        self.parses
    }
}

type Events<'a> = Peekable<OffsetIter<'a>>;

fn parse(source: &str, clipped: bool) -> Vec<Block> {
    let mut events = Parser::new_ext(source, Options::ENABLE_TABLES)
        .into_offset_iter()
        .peekable();
    let mut blocks = Vec::new();
    while let Some((_, range)) = events.peek() {
        let start = range.start;
        match block(source, &mut events, 0) {
            Ok(block) => blocks.push(block),
            Err(()) => {
                let line_start = source[..start]
                    .rfind(['\n', '\r'])
                    .map_or(0, |index| index + 1);
                let start = if source[line_start..start].trim().is_empty() {
                    line_start
                } else {
                    start
                };
                blocks.push(literal(&source[start..]));
                break;
            }
        }
    }
    if blocks.is_empty() {
        blocks.push(literal(source));
    }
    if clipped {
        match blocks.last_mut() {
            Some(Block::Paragraph(paragraph) | Block::Heading { paragraph, .. }) => {
                paragraph.text = format!("{}…", paragraph.text).into();
            }
            Some(Block::Literal(text)) => *text = format!("{text}…").into(),
            _ => blocks.push(literal("…")),
        }
    }
    blocks
}

fn literal(text: &str) -> Block {
    Block::Literal(text.to_owned().into())
}

fn block(source: &str, events: &mut Events<'_>, depth: usize) -> Result<Block, ()> {
    if events.peek().is_some_and(|(event, _)| is_inline(event)) {
        return inline(source, events, None).map(Block::Paragraph);
    }
    let (event, range) = events.next().ok_or(())?;
    match event {
        Event::Start(Tag::Paragraph) => {
            if source[range.clone()]
                .chars()
                .take(MAX_PARAGRAPH_CHARS + 1)
                .count()
                > MAX_PARAGRAPH_CHARS
            {
                skip(events, TagEnd::Paragraph)?;
                return Ok(literal(&source[range]));
            }
            let paragraph = inline(source, events, Some(TagEnd::Paragraph))?;
            Ok(if paragraph.text.is_empty() {
                literal(&source[range])
            } else {
                Block::Paragraph(paragraph)
            })
        }
        Event::Start(Tag::Heading {
            level: level @ (HeadingLevel::H1 | HeadingLevel::H2 | HeadingLevel::H3),
            ..
        }) => {
            let paragraph = inline(source, events, Some(TagEnd::Heading(level)))?;
            Ok(Block::Heading { level, paragraph })
        }
        Event::Start(Tag::List(start)) if depth < MAX_LIST_DEPTH => {
            let mut items = Vec::new();
            loop {
                match events.next().map(|(event, _)| event) {
                    Some(Event::End(TagEnd::List(_))) => break,
                    Some(Event::Start(Tag::Item)) => {
                        let mut item = Vec::new();
                        while !at_end(events, TagEnd::Item) {
                            item.push(block(source, events, depth + 1)?);
                        }
                        events.next();
                        items.push(item);
                    }
                    _ => return Err(()),
                }
            }
            Ok(Block::List { start, items })
        }
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(language))) => {
            fenced(source, events, range, &language)
        }
        Event::Start(Tag::Table(alignments)) => table(source, events, alignments),
        Event::Start(tag) => {
            skip(events, tag.to_end())?;
            Ok(literal(&source[range]))
        }
        Event::Rule => Ok(literal(source[range].trim_end_matches(['\r', '\n']))),
        _ => Err(()),
    }
}

fn table(source: &str, events: &mut Events<'_>, alignments: Vec<Alignment>) -> Result<Block, ()> {
    let mut rows = Vec::new();
    loop {
        let end = match events.next().map(|(event, _)| event) {
            Some(Event::Start(Tag::TableHead)) => TagEnd::TableHead,
            Some(Event::Start(Tag::TableRow)) => TagEnd::TableRow,
            Some(Event::End(TagEnd::Table)) => return Ok(Block::Table { alignments, rows }),
            _ => return Err(()),
        };
        let mut cells = Vec::with_capacity(alignments.len());
        while !at_end(events, end) {
            if !matches!(events.next(), Some((Event::Start(Tag::TableCell), _))) {
                return Err(());
            }
            cells.push(inline(source, events, Some(TagEnd::TableCell))?);
        }
        events.next();
        rows.push(cells);
    }
}

fn at_end(events: &mut Events<'_>, tag: TagEnd) -> bool {
    matches!(events.peek(), Some((Event::End(end), _)) if *end == tag)
}

fn skip(events: &mut Events<'_>, end: TagEnd) -> Result<(), ()> {
    let mut depth = 0;
    for (event, _) in events.by_ref() {
        match event {
            Event::Start(_) => depth += 1,
            Event::End(tag) if depth == 0 => return (tag == end).then_some(()).ok_or(()),
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    Err(())
}

fn fenced(
    source: &str,
    events: &mut Events<'_>,
    range: Range<usize>,
    language: &str,
) -> Result<Block, ()> {
    let raw = &source[range.clone()];
    let opening = raw.lines().next().ok_or(())?;
    let marker = *opening.as_bytes().first().ok_or(())?;
    let width = opening.bytes().take_while(|byte| *byte == marker).count();
    let mut code = String::new();
    let mut content_end = range.start + opening.len();
    for (event, offset) in events.by_ref() {
        match event {
            Event::Text(text) => {
                code.push_str(&text);
                content_end = content_end.max(offset.end);
            }
            Event::End(TagEnd::CodeBlock) => {
                // CommonMark also emits an End event at EOF for unclosed fences.
                // A real closing line lies outside the emitted code text.
                let tail = source.get(content_end..range.end).ok_or(())?;
                let close = tail.lines().next_back().unwrap_or("").trim();
                if close.len() < width || !close.bytes().all(|byte| byte == marker) {
                    return Err(());
                }
                let code = code.strip_suffix('\n').unwrap_or(&code);
                let untyped = matches!(
                    language.split_whitespace().next(),
                    None | Some("text" | "plaintext")
                );
                let tight_before = source[..range.start]
                    .trim_end_matches([' ', '\t'])
                    .lines()
                    .next_back()
                    .is_some_and(|line| !line.trim().is_empty());
                return Ok(Block::Code {
                    text: code.to_owned().into(),
                    untyped,
                    tight_before,
                });
            }
            _ => return Err(()),
        }
    }
    Err(())
}

fn is_inline(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Text(_)
            | Event::Code(_)
            | Event::InlineHtml(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::Start(Tag::Strong | Tag::Emphasis | Tag::Link { .. } | Tag::Image { .. })
            | Event::End(TagEnd::Strong | TagEnd::Emphasis | TagEnd::Link)
    )
}

fn inline(source: &str, events: &mut Events<'_>, end: Option<TagEnd>) -> Result<Paragraph, ()> {
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut links = Vec::new();
    let mut bold = 0usize;
    let mut italic = 0usize;
    let mut link = None;

    while events.peek().is_some_and(|(event, _)| is_inline(event)) {
        let (event, range) = events.next().ok_or(())?;
        let code = matches!(event, Event::Code(_));
        let start = text.len();
        match event {
            Event::Text(value) | Event::Code(value) | Event::InlineHtml(value) => {
                text.push_str(&value);
            }
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::Start(Tag::Strong) => bold += 1,
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Strong) => bold = bold.checked_sub(1).ok_or(())?,
            Event::End(TagEnd::Emphasis) => italic = italic.checked_sub(1).ok_or(())?,
            Event::Start(Tag::Link { dest_url, .. }) => {
                link = Some((start, dest_url));
            }
            Event::End(TagEnd::Link) => {
                let (link_start, target) = link.take().ok_or(())?;
                if !target.is_empty() && &text[link_start..start] != target.as_ref() {
                    if link_start == start {
                        text.push_str(&target);
                    } else {
                        text.push_str(" (");
                        text.push_str(&target);
                        text.push(')');
                    }
                }
                if let Some(url) = web_url(&target).filter(|_| link_start < text.len()) {
                    links.push((link_start..text.len(), url));
                }
            }
            Event::Start(Tag::Image { .. }) => {
                skip(events, TagEnd::Image)?;
                text.push_str(&source[range]);
            }
            _ => return Err(()),
        }
        if start == text.len() {
            continue;
        }
        let bold = bold > 0;
        let italic = italic > 0;
        let link = link.is_some();
        if !(bold || italic || code || link) {
            continue;
        }
        if let Some(last) = spans.last_mut()
            && last.range.end == start
            && (last.bold, last.italic, last.code, last.link) == (bold, italic, code, link)
        {
            last.range.end = text.len();
        } else {
            spans.push(Span {
                range: start..text.len(),
                bold,
                italic,
                code,
                link,
            });
        }
    }
    if bold != 0 || italic != 0 || link.is_some() {
        return Err(());
    }
    if let Some(end) = end {
        if !at_end(events, end) {
            return Err(());
        }
        events.next();
    }
    Ok(Paragraph {
        text: text.into(),
        spans,
        links,
    })
}

fn web_url(url: &str) -> Option<SharedString> {
    let (scheme, host) = url.split_once("://")?;
    (!host.is_empty()
        && (scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http"))
        && !url
            .chars()
            .any(|character| character.is_control() || character.is_whitespace()))
    .then(|| url.to_owned().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_heading_and_styles_utf8_ranges() {
        let blocks = parse(
            "### 灯光\n\n普通 **粗体与 *斜体***、*斜体*、`路径`、[**手册**](https://example.com/help)。",
            false,
        );
        let [
            Block::Heading {
                level: HeadingLevel::H3,
                paragraph: heading,
            },
            Block::Paragraph(body),
        ] = blocks.as_slice()
        else {
            panic!("unexpected blocks: {blocks:?}");
        };
        assert_eq!(heading.text.as_ref(), "灯光");
        assert_eq!(
            body.text.as_ref(),
            "普通 粗体与 斜体、斜体、路径、手册 (https://example.com/help)。"
        );
        let spans: Vec<_> = body
            .spans
            .iter()
            .map(|span| {
                (
                    &body.text[span.range.clone()],
                    span.bold,
                    span.italic,
                    span.code,
                    span.link,
                )
            })
            .collect();
        assert_eq!(
            spans,
            [
                ("粗体与 ", true, false, false, false),
                ("斜体", true, true, false, false),
                ("斜体", false, true, false, false),
                ("路径", false, false, true, false),
                ("手册", true, false, false, true),
            ]
        );
        let [(range, target)] = body.links.as_slice() else {
            panic!("expected one clickable link");
        };
        assert_eq!(&body.text[range.clone()], "手册 (https://example.com/help)");
        assert_eq!(target.as_ref(), "https://example.com/help");
    }

    #[test]
    fn renders_ordered_and_unordered_nested_lists() {
        let blocks = parse("3. **主项**\n   - 内项\n     1. 内层有序\n4. 尾项", false);
        let [
            Block::List {
                start: Some(3),
                items,
            },
        ] = blocks.as_slice()
        else {
            panic!("unexpected blocks: {blocks:?}");
        };
        let [
            Block::Paragraph(first),
            Block::List {
                start: None,
                items: nested,
            },
        ] = items[0].as_slice()
        else {
            panic!("unexpected ordered item: {:?}", items[0]);
        };
        assert_eq!(first.text.as_ref(), "主项");
        assert!(first.spans[0].bold);
        let [
            Block::Paragraph(item),
            Block::List {
                start: Some(1),
                items: ordered,
            },
        ] = nested[0].as_slice()
        else {
            panic!("unexpected unordered item: {:?}", nested[0]);
        };
        assert_eq!(item.text.as_ref(), "内项");
        assert!(
            matches!(&ordered[0][0], Block::Paragraph(item) if item.text.as_ref() == "内层有序")
        );
        assert!(matches!(&items[1][0], Block::Paragraph(item) if item.text.as_ref() == "尾项"));
    }

    #[test]
    fn fences_preserve_indent_and_support_nested_and_empty_code() {
        let blocks = parse(
            "````rust\r\n  fn main() {\r\n    ```\r\n  }\r\n````\r\n\r\n- 项目\n  ~~~\n    code\n  ~~~~\n\n```\n```",
            false,
        );
        let [
            Block::Code {
                text: first,
                untyped: false,
                ..
            },
            Block::List { items, .. },
            Block::Code {
                text: empty,
                untyped: true,
                ..
            },
        ] = blocks.as_slice()
        else {
            panic!("unexpected blocks: {blocks:?}");
        };
        assert_eq!(first.as_ref(), "  fn main() {\n    ```\n  }");
        assert!(matches!(&items[0][1], Block::Code {text, ..} if text.as_ref() == "  code"));
        assert!(empty.is_empty());
    }

    #[test]
    fn unclosed_fence_falls_back_from_its_start_and_keeps_remaining_source() {
        for tail in [
            "```rust\n  unfinished\n### literal\n",
            "````rust\n```\n",
            "```\n    ```",
            "```",
            "   ```\n    unfinished",
            "- list\n  ```\n  unfinished\n\nNext paragraph",
        ] {
            let blocks = parse(&format!("**Earlier**\n\n{tail}"), false);
            assert!(
                matches!(blocks.as_slice(), [Block::Paragraph(_), Block::Literal(text)] if text.as_ref() == tail),
                "unexpected fallback: {blocks:?}"
            );
        }
    }

    #[test]
    fn unsupported_syntax_stays_literal_and_unmatched_markers_keep_their_text() {
        let blocks = parse(
            "<div>**raw**</div>\n\nKeep <i>inline</i>, ![**alt**](https://example.com/i), and **unfinished *syntax.",
            false,
        );
        let [Block::Literal(html), Block::Paragraph(body)] = blocks.as_slice() else {
            panic!("unexpected blocks: {blocks:?}");
        };
        assert_eq!(html.trim_end(), "<div>**raw**</div>");
        assert_eq!(
            body.text.as_ref(),
            "Keep <i>inline</i>, ![**alt**](https://example.com/i), and **unfinished *syntax."
        );
        assert!(body.spans.is_empty());
        assert!(body.links.is_empty());
    }

    #[test]
    fn table_keeps_cells_and_column_alignment_without_markdown_delimiters() {
        let blocks = parse(
            "| **项目** | Count | Mode |\n| :--- | ---: | :---: |\n| 灯\\|光 | 120 | warm |",
            false,
        );
        let [Block::Table { alignments, rows }] = blocks.as_slice() else {
            panic!("unexpected blocks: {blocks:?}");
        };
        assert_eq!(
            alignments,
            &[Alignment::Left, Alignment::Right, Alignment::Center]
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].text.as_ref(), "项目");
        assert!(rows[0][0].spans[0].bold);
        assert_eq!(rows[1][0].text.as_ref(), "灯|光");
        assert_eq!(rows[1][1].text.as_ref(), "120");
    }

    #[test]
    fn unsupported_link_targets_keep_style_without_click_actions() {
        let blocks = parse(
            "[local](file:///tmp/file) [action](javascript:alert) [web](HTTP://example.com)",
            false,
        );
        let [Block::Paragraph(body)] = blocks.as_slice() else {
            panic!("unexpected blocks: {blocks:?}");
        };
        assert_eq!(
            body.text.as_ref(),
            "local (file:///tmp/file) action (javascript:alert) web (HTTP://example.com)"
        );
        assert_eq!(body.spans.iter().filter(|span| span.link).count(), 3);
        assert_eq!(body.links.len(), 1);
        assert_eq!(body.links[0].1.as_ref(), "HTTP://example.com");
    }

    #[test]
    fn long_paragraph_keeps_the_bounded_source_and_ellipsis() {
        let source = format!("**{}**", "灯".repeat(MAX_PARAGRAPH_CHARS));
        let blocks = parse(&source, true);
        assert!(
            matches!(blocks.as_slice(), [Block::Literal(text)] if text.strip_suffix('…') == Some(source.as_str()))
        );
    }
}
