//! GPUI styling for parsed session-history Markdown.
//!
//! The parser in the sibling `markdown` module turns cleaned history text into
//! blocks and byte ranges. This module only paints those blocks: inline runs
//! stay inside a single `StyledText` (or `InteractiveText` when the paragraph
//! has links), while block structure is laid out with `div`/flex.
//!
//! The visual baseline is the real terminal rendering of the same message:
//! body text inherits the terminal font, size and line height; headings are the
//! same size as body text (H1 bold+italic+underlined, H2/H3 bold); inline code
//! is #B1B9F9 with no background or extra spaces; links keep the body color and
//! show `label (URL)`; code blocks have no chrome and hard-wrap at the remaining
//! terminal columns, with untyped fences using the inline-code color and a
//! directly-following fence consuming the inter-block blank row; tables use
//! Unicode boxes when they fit and otherwise fall back to the terminal's
//! `header: value` record layout.

use std::{cell::Cell, ops::Range, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, Div, ElementId, FontStyle, FontWeight, HighlightStyle,
    InteractiveText, Pixels, Rgba, SharedString, Styled, StyledText, TextLayout, canvas,
    combine_highlights, div, prelude::*,
};
use pulldown_cmark::{Alignment, HeadingLevel};
use unicode_segmentation::UnicodeSegmentation;

use super::HistoryStyle;
use super::markdown::{Block, Paragraph, Span};

/// Assistant history rows reserve two terminal columns for the `⏺ ` prefix.
/// The outer row owns that alignment; this module only needs it to know how
/// many columns remain for code blocks and tables.
const BODY_PREFIX_COLUMNS: usize = 2;
const TAB_COLUMNS: usize = 4;
/// The terminal prints a 40-column rule between stacked table records.
const RECORD_SEPARATOR_COLUMNS: usize = 40;

fn inline_code_foreground() -> Rgba {
    gpui::rgba(0xB1B9F9ff)
}

pub(super) fn render(
    blocks: &[Block],
    event_index: i64,
    style: HistoryStyle,
    scroll_pending: Option<Rc<Cell<bool>>>,
) -> Div {
    let mut state = RenderState {
        style,
        event_index,
        next_link_id: 0,
        scroll_pending,
    };
    let children = state.top_level_blocks(blocks);
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .children(children)
}

struct RenderState {
    style: HistoryStyle,
    event_index: i64,
    next_link_id: usize,
    scroll_pending: Option<Rc<Cell<bool>>>,
}

impl RenderState {
    /// One blank terminal row between top-level blocks. A fence that directly
    /// follows its preceding paragraph (`tight_before`) gets no extra row, so
    /// the code starts on the very next line as it does in the terminal.
    fn top_level_blocks(&mut self, blocks: &[Block]) -> Vec<Div> {
        let mut children = Vec::with_capacity(blocks.len());
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 && !is_tight_code(block) {
                children.push(div().h(self.style.line_height));
            }
            children.push(self.block(block));
        }
        children
    }

    /// Nested block lists (list items) keep zero spacing: the terminal lays
    /// list bodies and their nested content out without blank rows.
    fn blocks(&mut self, blocks: &[Block]) -> Vec<Div> {
        blocks.iter().map(|block| self.block(block)).collect()
    }

    fn block(&mut self, block: &Block) -> Div {
        match block {
            Block::Paragraph(paragraph) => self.paragraph(paragraph, None),
            Block::Heading { level, paragraph } => self.paragraph(paragraph, Some(*level)),
            Block::List { start, items } => self.list(*start, items),
            Block::Table { alignments, rows } => self.table(alignments, rows),
            Block::Code {
                text,
                untyped,
                mark,
                ..
            } => self.code_block(text, *untyped, *mark),
            Block::Literal(text) => literal_block(text),
            Block::MarkedLiteral(text, mark) => {
                let styled = StyledText::new(text.clone())
                    .with_highlights([(mark.clone(), search_highlight(self.style))]);
                let layout = styled.layout().clone();
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .child(styled)
                    .children(self.scroll_marker(Some((layout, mark.start))))
            }
        }
    }

    fn paragraph(&mut self, paragraph: &Paragraph, heading: Option<HeadingLevel>) -> Div {
        let text = self.inline_text(paragraph);
        let container = div().w_full().min_w_0().whitespace_normal().child(text);
        let Some(level) = heading else {
            return container;
        };

        let paint = heading_paint(level);
        let mut container = container;
        if paint.bold {
            container = container.font_weight(FontWeight::BOLD);
        }
        if paint.italic {
            container = container.italic();
        }
        if paint.underline {
            container = container.underline();
        }
        container
    }

    /// Builds one styled text element for the paragraph. Inline text is never
    /// split across flex children, so wrapping, selection metrics and
    /// cursor/hit-testing stay with the framework's text layout.
    fn inline_text(&mut self, paragraph: &Paragraph) -> AnyElement {
        let highlights = paragraph_highlights(paragraph);
        let mut text = StyledText::new(paragraph.text.clone());
        let marker = paragraph
            .mark
            .as_ref()
            .and_then(|mark| self.scroll_marker(Some((text.layout().clone(), mark.start))));
        if let Some(mark) = &paragraph.mark {
            text = text.with_highlights(combine_highlights(
                highlights,
                [(mark.clone(), search_highlight(self.style))],
            ));
        } else {
            text = text.with_highlights(highlights);
        }
        let links = clickable_links(paragraph);
        let text = if links.is_empty() {
            text.into_any_element()
        } else {
            let link_id = self.next_link_id;
            self.next_link_id += 1;
            let element_id = ElementId::from(SharedString::from(format!(
                "history-markdown-link-{}-{link_id}",
                self.event_index
            )));
            let (ranges, targets): (Vec<_>, Vec<_>) = links.into_iter().unzip();
            InteractiveText::new(element_id, text)
                .on_click(ranges, move |index, _window, cx: &mut App| {
                    if let Some(target) = targets.get(index) {
                        cx.open_url(target.as_str());
                    }
                })
                .into_any_element()
        };
        if let Some(marker) = marker {
            div().child(text).child(marker).into_any_element()
        } else {
            text
        }
    }

    fn scroll_marker(&mut self, text: Option<(TextLayout, usize)>) -> Option<AnyElement> {
        let pending = self.scroll_pending.take().filter(|pending| pending.get())?;
        let after_paint = pending.clone();
        let line_height = self.style.line_height;
        Some(
            canvas(
                move |bounds, window, _| {
                    if !pending.get() {
                        return;
                    }
                    let mut bounds: Bounds<Pixels> = bounds;
                    if let Some((layout, index)) = &text
                        && let Some(position) = layout.position_for_index(*index)
                    {
                        bounds.origin.y = position.y;
                    }
                    bounds.size.height = line_height;
                    window.request_autoscroll(bounds);
                },
                move |_, (), _, _| after_paint.set(false),
            )
            .absolute()
            .top_0()
            .left_0()
            .into_any_element(),
        )
    }

    fn list(&mut self, start: Option<u64>, items: &[Vec<Block>]) -> Div {
        // The marker column is as wide as the widest marker plus one space.
        // The item body then starts exactly at the previous content column,
        // which is how the terminal nests lists without a fixed indent.
        let marker_width = items
            .iter()
            .enumerate()
            .map(|(index, _)| list_marker(start, index).chars().count())
            .max()
            .unwrap_or(1);
        let marker_columns = marker_width + 1;

        let mut rows = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            let marker = format!("{} ", list_marker(start, index));
            let body = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .children(self.blocks(item));
            rows.push(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .child(
                        div()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .w(self.style.cell_width * marker_columns as f32)
                            .child(marker),
                    )
                    .child(body),
            );
        }
        div().w_full().min_w_0().flex().flex_col().children(rows)
    }

    fn code_block(&mut self, code: &SharedString, untyped: bool, mark: Option<usize>) -> Div {
        let (text, marked_row) = content_columns(self.style).map_or_else(
            || {
                (
                    code.to_string(),
                    mark.map(|byte| code[..byte].matches('\n').count()),
                )
            },
            |columns| wrap_code_with_mark(code, columns, mark),
        );
        div()
            .w_full()
            .min_w_0()
            .whitespace_normal()
            .when_some(code_foreground(untyped), |block, color| {
                block.text_color(color)
            })
            .child(self.terminal_rows(&text, marked_row))
    }

    fn table(&mut self, alignments: &[Alignment], rows: &[Vec<Paragraph>]) -> Div {
        let columns = table_columns(alignments, rows);
        if columns == 0 {
            return div();
        }
        let cells = table_cells(rows, columns);
        let widths = natural_widths(&cells);
        if content_columns(self.style).is_some_and(|available| table_width(&widths) > available) {
            return self.record_table(&cells, rows, columns);
        }
        let mut line = 1;
        let marked_row = rows.iter().zip(&cells).find_map(|(row, cells)| {
            let start = line;
            line += cells
                .iter()
                .map(|cell| cell.split('\n').count())
                .max()
                .unwrap_or(1)
                + 1;
            row.iter().find_map(|cell| {
                cell.mark
                    .as_ref()
                    .map(|mark| start + cell.text[..mark.start].matches('\n').count())
            })
        });
        div()
            .w_full()
            .min_w_0()
            .whitespace_nowrap()
            .child(self.terminal_rows(&format_box_table(alignments, &cells, &widths), marked_row))
    }

    /// The terminal does not squeeze wide tables into narrow columns. It turns
    /// every data row into `header: value` lines, bolds the header label and
    /// keeps the parsed value styling, then prints a 40-column rule between
    /// records. Empty columns are still labelled so records stay self-describing.
    fn record_table(
        &mut self,
        cells: &[Vec<String>],
        rows: &[Vec<Paragraph>],
        columns: usize,
    ) -> Div {
        let headers = cells.first().cloned().unwrap_or_default();
        let separator_width = content_columns(self.style)
            .map_or(RECORD_SEPARATOR_COLUMNS, |available| {
                RECORD_SEPARATOR_COLUMNS.min(available)
            });
        let separator = "─".repeat(separator_width);
        let mut container = div().w_full().min_w_0().flex().flex_col();
        for (row_index, row) in rows.iter().skip(1).enumerate() {
            if row_index > 0 {
                container = container.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_nowrap()
                        .child(separator.clone()),
                );
            }
            for column in 0..columns {
                let label = headers.get(column).map_or("", String::as_str);
                let mut paragraph = record_paragraph(label, row.get(column));
                if row_index == 0
                    && let Some(mark) = rows[0].get(column).and_then(|cell| cell.mark.clone())
                {
                    paragraph.mark = Some(mark);
                }
                container = container.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .child(self.inline_text(&paragraph)),
                );
            }
        }
        container
    }

    fn terminal_rows(&mut self, text: &str, mark: Option<usize>) -> Div {
        terminal_rows(
            text,
            self.style,
            mark,
            mark.and_then(|_| self.scroll_marker(None)),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeadingPaint {
    bold: bool,
    italic: bool,
    underline: bool,
}

/// H1 is bold, italic and underlined in the terminal; H2/H3 only turn bold.
/// No level keeps the `#` markers or grows the font.
fn heading_paint(level: HeadingLevel) -> HeadingPaint {
    match level {
        HeadingLevel::H1 => HeadingPaint {
            bold: true,
            italic: true,
            underline: true,
        },
        _ => HeadingPaint {
            bold: true,
            italic: false,
            underline: false,
        },
    }
}

/// Untyped fences (no info string, `text` or `plaintext`) are drawn in the
/// same #B1B9F9 as inline code; a fence with an explicit language stays in the
/// ordinary body color, single-colored, because syntax colors are not
/// reimplemented here.
fn code_foreground(untyped: bool) -> Option<Rgba> {
    untyped.then(inline_code_foreground)
}

/// Paints terminal-shaped text at fixed terminal columns. Continuous GPUI
/// shaping gives a CJK glyph less than two terminal cells, so the box bars that
/// follow it drift left. ASCII runs stay in one child (their shaping is
/// unchanged), while every non-ASCII grapheme gets its own non-shrinking child
/// whose width is the terminal cell count. Blank lines keep a full line height.
fn search_highlight(style: HistoryStyle) -> HighlightStyle {
    HighlightStyle {
        background_color: Some(search_background(style).into()),
        ..Default::default()
    }
}

fn search_background(style: HistoryStyle) -> Rgba {
    Rgba {
        a: 0.35,
        ..style.amber
    }
}

fn terminal_rows(
    text: &str,
    style: HistoryStyle,
    mark: Option<usize>,
    mut marker: Option<AnyElement>,
) -> Div {
    let cell = |text: &str, columns: usize| {
        div()
            .flex_shrink_0()
            .whitespace_nowrap()
            .w(style.cell_width * columns as f32)
            .child(text.to_owned())
    };
    let mut rows = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let mut cells = Vec::new();
        let mut ascii_start = 0;
        for (index, grapheme) in line.grapheme_indices(true) {
            if grapheme.is_ascii() {
                continue;
            }
            if ascii_start < index {
                let run = &line[ascii_start..index];
                cells.push(cell(run, run.len()));
            }
            cells.push(cell(grapheme, grapheme_columns(grapheme)));
            ascii_start = index + grapheme.len();
        }
        if ascii_start < line.len() {
            let run = &line[ascii_start..];
            cells.push(cell(run, run.len()));
        }
        rows.push(
            div()
                .flex()
                .h(style.line_height)
                .whitespace_nowrap()
                .when(mark == Some(index), |row| {
                    row.bg(search_background(style)).children(marker.take())
                })
                .children(cells),
        );
    }
    div().w_full().min_w_0().flex().flex_col().children(rows)
}

fn is_tight_code(block: &Block) -> bool {
    matches!(
        block,
        Block::Code {
            tight_before: true,
            ..
        }
    )
}

fn literal_block(text: &SharedString) -> Div {
    div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .child(text.clone())
}

fn list_marker(start: Option<u64>, index: usize) -> String {
    match start {
        Some(start) => format!("{}.", start.saturating_add(index as u64)),
        None => "-".to_owned(),
    }
}

/// Builds a sorted, non-overlapping highlight list from the parser's spans.
/// The parser already guarantees this shape; the bounds checks keep a future
/// parser bug from turning a malformed range into a paint-time panic.
fn paragraph_highlights(paragraph: &Paragraph) -> Vec<(Range<usize>, HighlightStyle)> {
    let text = paragraph.text.as_ref();
    let mut cursor = 0;
    let mut highlights = Vec::new();
    for span in &paragraph.spans {
        // Links keep the ordinary body color and no underline; only their
        // `label (URL)` text is rendered, so no highlight is needed for them.
        if !(span.bold || span.italic || span.code) {
            continue;
        }
        let start = span.range.start.max(cursor);
        let end = span.range.end.min(text.len());
        if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        cursor = end;

        let mut highlight = HighlightStyle::default();
        if span.bold {
            highlight.font_weight = Some(FontWeight::BOLD);
        }
        if span.italic {
            highlight.font_style = Some(FontStyle::Italic);
        }
        if span.code {
            // Inline code is foreground-only in the terminal: no fill and no
            // padding spaces. Pin the color so inverted user rows stay legible.
            highlight.color = Some(inline_code_foreground().into());
        }
        highlights.push((start..end, highlight));
    }
    highlights
}

/// Clickable ranges must be the exact UTF-8 byte ranges the parser produced,
/// and the target list is kept in the same order so `InteractiveText`'s click
/// index maps straight back to the URL. Only links supplied by the parser
/// (already restricted to http/https) ever reach this list.
fn clickable_links(paragraph: &Paragraph) -> Vec<(Range<usize>, SharedString)> {
    let text = paragraph.text.as_ref();
    paragraph
        .links
        .iter()
        .filter_map(|(range, target)| {
            let end = range.end.min(text.len());
            (range.start < end && text.is_char_boundary(range.start) && text.is_char_boundary(end))
                .then(|| (range.start..end, target.clone()))
        })
        .collect()
}

/// Remaining terminal columns for an assistant body. The painted grid supplies
/// the terminal column count; the assistant row's two prefix columns are the
/// only inset this module applies, so code and tables use the same body width.
fn content_columns(style: HistoryStyle) -> Option<usize> {
    style.grid_width.map(|width| {
        let columns = (f32::from(width) / f32::from(style.cell_width))
            .round()
            .max(1.) as usize;
        columns.saturating_sub(BODY_PREFIX_COLUMNS).max(1)
    })
}

fn record_paragraph(header: &str, value: Option<&Paragraph>) -> Paragraph {
    // Header text is flattened to plain text and only `header:` is bold; the
    // separating space and the parsed value keep their own styles.
    let mut text = format!("{header}: ");
    let bold_end = text.len() - 1;
    let offset = text.len();
    let mut spans = vec![Span {
        range: 0..bold_end,
        bold: true,
        italic: false,
        code: false,
        link: false,
    }];
    let mut links = Vec::new();
    let mut mark = None;
    if let Some(value) = value {
        text.push_str(&value.text);
        spans.extend(value.spans.iter().map(|span| Span {
            range: span.range.start + offset..span.range.end + offset,
            bold: span.bold,
            italic: span.italic,
            code: span.code,
            link: span.link,
        }));
        links.extend(
            value
                .links
                .iter()
                .map(|(range, target)| (range.start + offset..range.end + offset, target.clone())),
        );
        mark = value
            .mark
            .as_ref()
            .map(|mark| mark.start + offset..mark.end + offset);
    }
    Paragraph {
        text: text.into(),
        spans,
        links,
        mark,
    }
}

fn table_columns(alignments: &[Alignment], rows: &[Vec<Paragraph>]) -> usize {
    let widest = rows.iter().map(Vec::len).max().unwrap_or(0);
    alignments.len().max(widest)
}

fn table_cells(rows: &[Vec<Paragraph>], columns: usize) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            (0..columns)
                .map(|column| {
                    row.get(column)
                        .map_or_else(String::new, |cell| expand_tabs(cell.text.trim()))
                })
                .collect()
        })
        .collect()
}

fn natural_widths(cells: &[Vec<String>]) -> Vec<usize> {
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; columns];
    for row in cells {
        for (column, cell) in row.iter().enumerate() {
            let width = cell.lines().map(display_width).max().unwrap_or(0);
            widths[column] = widths[column].max(width);
        }
    }
    widths
}

/// Total box width: content, one space of padding on both sides of every
/// column, and one box character per column edge.
fn table_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + 3 * widths.len() + 1
}

fn format_box_table(alignments: &[Alignment], cells: &[Vec<String>], widths: &[usize]) -> String {
    let columns = widths.len();
    let mut out = String::new();
    push_border(&mut out, '┌', '┬', '┐', widths);
    for (row_index, row) in cells.iter().enumerate() {
        let lines: Vec<Vec<&str>> = (0..columns)
            .map(|column| {
                row.get(column)
                    .map_or_else(|| vec![""], |cell| cell.split('\n').collect())
            })
            .collect();
        let height = lines.iter().map(Vec::len).max().unwrap_or(1);
        for line_index in 0..height {
            out.push('│');
            for column in 0..columns {
                let content = lines[column].get(line_index).copied().unwrap_or("");
                let alignment = alignments.get(column).copied().unwrap_or(Alignment::None);
                out.push(' ');
                out.push_str(&pad_cell(content, widths[column], alignment));
                out.push(' ');
                out.push('│');
            }
            out.push('\n');
        }
        if row_index + 1 < cells.len() {
            push_border(&mut out, '├', '┼', '┤', widths);
        }
    }
    push_border(&mut out, '└', '┴', '┘', widths);
    out
}

fn push_border(out: &mut String, left: char, join: char, right: char, widths: &[usize]) {
    out.push(left);
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            out.push(join);
        }
        for _ in 0..width + 2 {
            out.push('─');
        }
    }
    out.push(right);
    out.push('\n');
}

fn pad_cell(content: &str, width: usize, alignment: Alignment) -> String {
    let length = display_width(content);
    if length >= width {
        return content.to_owned();
    }
    let padding = width - length;
    let (left, right) = match alignment {
        Alignment::Right => (padding, 0),
        Alignment::Center => (padding / 2, padding - padding / 2),
        Alignment::None | Alignment::Left => (0, padding),
    };
    let mut padded = String::with_capacity(content.len() + padding);
    for _ in 0..left {
        padded.push(' ');
    }
    padded.push_str(content);
    for _ in 0..right {
        padded.push(' ');
    }
    padded
}

fn wrap_code_with_mark(code: &str, columns: usize, mark: Option<usize>) -> (String, Option<usize>) {
    let columns = columns.max(1);
    let expanded = expand_tabs(code);
    let mark = mark.map(|byte| expand_tabs(&code[..byte]).len());
    let mut out = String::with_capacity(expanded.len());
    let mut source_start = 0;
    let mut row = 0;
    let mut marked_row = None;
    for line in expanded.split_inclusive('\n') {
        let mut column = 0;
        for (index, grapheme) in line.grapheme_indices(true) {
            if grapheme == "\n" {
                out.push('\n');
                row += 1;
                continue;
            }
            let width = grapheme_columns(grapheme);
            if column > 0 && column + width > columns {
                out.push('\n');
                row += 1;
                column = 0;
            }
            let start = source_start + index;
            if mark.is_some_and(|byte| (start..start + grapheme.len()).contains(&byte)) {
                marked_row = Some(row);
            }
            out.push_str(grapheme);
            column += width;
        }
        source_start += line.len();
    }
    (out, marked_row)
}

#[cfg(test)]
fn wrap_code(code: &str, columns: usize) -> String {
    wrap_code_with_mark(code, columns, None).0
}

fn expand_tabs(text: &str) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut column = 0;
    for grapheme in text.graphemes(true) {
        if grapheme == "\n" {
            expanded.push('\n');
            column = 0;
        } else if grapheme == "\t" {
            let spaces = TAB_COLUMNS - column % TAB_COLUMNS;
            for _ in 0..spaces {
                expanded.push(' ');
            }
            column += spaces;
        } else {
            expanded.push_str(grapheme);
            column += grapheme_columns(grapheme);
        }
    }
    expanded
}

fn display_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_columns).sum()
}

fn grapheme_columns(grapheme: &str) -> usize {
    grapheme.chars().map(char_columns).sum()
}

/// Terminal cell width for one scalar. Combining marks take no cell; CJK,
/// fullwidth forms and emoji take two. Tables and code wrapping share this so
/// their padding and hard breaks agree.
fn char_columns(character: char) -> usize {
    let code = character as u32;
    if code < 0x20 || (0x7F..=0x9F).contains(&code) {
        return 0;
    }
    if character == '\t' {
        return TAB_COLUMNS;
    }
    if is_zero_width(character) {
        return 0;
    }
    if is_wide(character) { 2 } else { 1 }
}

fn is_zero_width(character: char) -> bool {
    matches!(
        character as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
            | 0xE0100..=0xE01EF
    )
}

fn is_wide(character: char) -> bool {
    matches!(
        character as u32,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F000..=0x1FAFF
            | 0x20000..=0x3FFFD
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::rgba;

    fn paragraph(text: &'static str, spans: Vec<super::super::markdown::Span>) -> Paragraph {
        Paragraph {
            text: text.into(),
            spans,
            links: Vec::new(),
            mark: None,
        }
    }

    fn span(
        range: Range<usize>,
        bold: bool,
        italic: bool,
        code: bool,
        link: bool,
    ) -> super::super::markdown::Span {
        super::super::markdown::Span {
            range,
            bold,
            italic,
            code,
            link,
        }
    }

    #[test]
    fn highlights_keep_each_inline_flag_and_stay_non_overlapping() {
        let paragraph = paragraph(
            "abcdefgh",
            vec![
                span(0..2, true, false, false, false),
                span(2..4, false, true, false, false),
                span(4..6, false, false, true, false),
                span(6..8, true, false, false, true),
                span(8..12, false, false, false, false),
            ],
        );
        let highlights = paragraph_highlights(&paragraph);
        assert_eq!(highlights.len(), 4);
        assert_eq!(highlights[0].0, 0..2);
        assert_eq!(highlights[0].1.font_weight, Some(FontWeight::BOLD));
        assert_eq!(highlights[1].0, 2..4);
        assert_eq!(highlights[1].1.font_style, Some(FontStyle::Italic));
        assert_eq!(highlights[2].0, 4..6);
        assert_eq!(highlights[2].1.color, Some(inline_code_foreground().into()));
        assert_eq!(highlights[2].1.background_color, None);
        assert_eq!(highlights[3].0, 6..8);
        assert_eq!(highlights[3].1.font_weight, Some(FontWeight::BOLD));
        // Links are painted with the ordinary body color and no underline.
        assert_eq!(highlights[3].1.color, None);
        assert_eq!(highlights[3].1.underline, None);
    }

    #[test]
    fn headings_match_terminal_emphasis_without_markers() {
        assert_eq!(
            heading_paint(HeadingLevel::H1),
            HeadingPaint {
                bold: true,
                italic: true,
                underline: true,
            }
        );
        assert_eq!(
            heading_paint(HeadingLevel::H2),
            HeadingPaint {
                bold: true,
                italic: false,
                underline: false,
            }
        );
        assert_eq!(
            heading_paint(HeadingLevel::H3),
            HeadingPaint {
                bold: true,
                italic: false,
                underline: false,
            }
        );
    }

    #[test]
    fn untyped_fences_use_inline_code_color_and_tight_fences_skip_the_gap() {
        assert_eq!(code_foreground(true), Some(inline_code_foreground()));
        assert_eq!(code_foreground(false), None);

        let tight = Block::Code {
            text: "code".into(),
            untyped: true,
            tight_before: true,
            mark: None,
        };
        let loose = Block::Code {
            text: "code".into(),
            untyped: false,
            tight_before: false,
            mark: None,
        };
        assert!(is_tight_code(&tight));
        assert!(!is_tight_code(&loose));
        assert!(!is_tight_code(&Block::Paragraph(paragraph("text", vec![]))));
    }

    #[test]
    fn clickable_links_stay_exact_and_drop_out_of_bounds_ranges() {
        let mut paragraph = paragraph("see docs here", vec![span(4..8, false, false, false, true)]);
        paragraph.links = vec![
            (4..8, "https://example.com/docs".into()),
            (14..99, "https://example.com/out".into()),
        ];
        let links = clickable_links(&paragraph);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0, 4..8);
        assert_eq!(links[0].1.as_ref(), "https://example.com/docs");
    }

    #[test]
    fn list_markers_use_terminal_dashes_and_ordered_numbers() {
        assert_eq!(list_marker(None, 0), "-");
        assert_eq!(list_marker(Some(9), 0), "9.");
        assert_eq!(list_marker(Some(9), 1), "10.");
    }

    #[test]
    fn code_hard_wraps_at_terminal_columns_and_expands_tabs() {
        assert_eq!(wrap_code("abcdef", 4), "abcd\nef");
        assert_eq!(wrap_code("灯光灯", 4), "灯光\n灯");
        assert_eq!(wrap_code("a\tb", 8), "a   b");
        assert_eq!(wrap_code("  keep\n    indent", 40), "  keep\n    indent");

        let wrapped = wrap_code("      println!(\"ABCDEFGHIJKLMNOPQRSTUVWXYZ\");", 20);
        assert_eq!(
            wrapped,
            "      println!(\"ABCD\nEFGHIJKLMNOPQRSTUVWX\nYZ\");"
        );
        for line in wrapped.lines() {
            assert!(
                display_width(line) <= 20,
                "line exceeded terminal columns: {line:?}"
            );
        }
    }

    #[test]
    fn narrow_tables_match_unicode_boxes_and_cjk_widths() {
        let rows = vec![
            vec![
                paragraph("Item 项目", vec![]),
                paragraph("Count", vec![]),
                paragraph("Mode", vec![]),
            ],
            vec![
                paragraph("Alpha", vec![]),
                paragraph("2", vec![]),
                paragraph("cold", vec![]),
            ],
            vec![
                paragraph("灯光", vec![]),
                paragraph("120", vec![]),
                paragraph("warm", vec![]),
            ],
        ];
        let alignments = [Alignment::Left, Alignment::Right, Alignment::Left];
        let cells = table_cells(&rows, 3);
        let widths = natural_widths(&cells);
        assert_eq!(widths, [9, 5, 4]);
        assert_eq!(table_width(&widths), 28);

        let table = format_box_table(&alignments, &cells, &widths);
        assert_eq!(
            table,
            "┌───────────┬───────┬──────┐\n\
             │ Item 项目 │ Count │ Mode │\n\
             ├───────────┼───────┼──────┤\n\
             │ Alpha     │     2 │ cold │\n\
             ├───────────┼───────┼──────┤\n\
             │ 灯光      │   120 │ warm │\n\
             └───────────┴───────┴──────┘\n"
        );
        assert!(!table.contains('|'));
        assert!(!table.contains("---"));
        for line in table.lines() {
            assert_eq!(display_width(line), 28, "misaligned line: {line:?}");
        }
    }

    #[test]
    fn wide_tables_stack_header_value_records_with_bold_labels() {
        // `table_width` is the decision input: a six-column table naturally
        // exceeds a 58-column body and must not be squeezed into boxes.
        let widths = [18, 12, 10, 8, 15, 9];
        assert!(table_width(&widths) > 58);
        assert_eq!(RECORD_SEPARATOR_COLUMNS.min(58), 40);

        let value = paragraph(
            "稳定源片段分母",
            vec![span(0..21, true, false, false, false)],
        );
        let record = record_paragraph("统计口径", Some(&value));
        assert_eq!(record.text.as_ref(), "统计口径: 稳定源片段分母");
        let label_end = "统计口径:".len();
        assert_eq!(record.spans[0].range, 0..label_end);
        assert!(record.spans[0].bold);
        let value_start = "统计口径: ".len();
        assert_eq!(record.spans[1].range, value_start..value_start + 21);
        assert!(record.spans[1].bold);
    }
}
