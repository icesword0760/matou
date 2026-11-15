use std::ops::Range;

use matou_native_terminal::{Cell, InputModes, Screen};

const WORD_SEPARATORS: &str = " ()[]{}',\"`";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GridPoint {
    pub row: usize,
    pub col: usize,
}

/// Half-open selection over the visible grid, in reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GridSelection {
    pub anchor: GridPoint,
    pub end: GridPoint,
}

impl GridSelection {
    pub fn range(self, row: usize, cols: usize) -> Range<usize> {
        let first = self.anchor.min(self.end);
        let last = self.anchor.max(self.end);
        if row < first.row || row > last.row {
            return 0..0;
        }
        let left = if row == first.row { first.col } else { 0 };
        let right = if row == last.row { last.col } else { cols };
        left.min(cols)..right.min(cols)
    }

    pub fn text(self, screen: &Screen, cols: usize) -> String {
        if self.anchor == self.end || screen.cols == 0 {
            return String::new();
        }
        let first = self.anchor.min(self.end).row;
        let last = self.anchor.max(self.end).row;
        let mut result = String::new();
        for (row, cells) in screen
            .cells
            .chunks(screen.cols)
            .enumerate()
            .take(last + 1)
            .skip(first)
        {
            let range = self.range(row, cols.min(cells.len()));
            let content_end = cells
                .iter()
                .enumerate()
                .rev()
                .find(|(_, cell)| cell.width != 0 && cell.has_content)
                .map_or(0, |(col, cell)| col + usize::from(cell.width));
            let end = range.end.min(content_end);
            if range.start < end {
                for (col, cell) in cells.iter().enumerate().take(end) {
                    if cell.width != 0 && col + usize::from(cell.width) > range.start {
                        result.push_str(&cell.text);
                    }
                }
            }
            if row < last && !wrap_continues(screen, row + 1) {
                result.push('\n');
            }
        }
        result
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionMode {
    Character,
    Word,
    Line,
}

#[derive(Clone, Copy)]
pub(crate) struct SelectionDrag {
    pub mode: SelectionMode,
    pub initial: GridSelection,
}

impl SelectionDrag {
    pub fn begin(
        mode: SelectionMode,
        screen: &Screen,
        row: usize,
        x: f32,
        cols: usize,
        rows: usize,
    ) -> Self {
        let point = selection_cell(row, x, cols);
        let mode = if mode == SelectionMode::Word && point.col == cols {
            SelectionMode::Character
        } else {
            mode
        };
        let initial = match mode {
            SelectionMode::Character => {
                let anchor = grid_boundary(screen, row, x, cols);
                GridSelection {
                    anchor,
                    end: anchor,
                }
            }
            SelectionMode::Word => word_at(screen, point, cols, rows),
            SelectionMode::Line => logical_line_at(screen, row, cols, rows),
        };
        Self { mode, initial }
    }

    pub fn extend(
        self,
        screen: &Screen,
        row: usize,
        x: f32,
        cols: usize,
        rows: usize,
    ) -> GridSelection {
        let anchor = self.initial.anchor;
        match self.mode {
            SelectionMode::Character => GridSelection {
                anchor,
                end: grid_boundary(screen, row, x, cols),
            },
            SelectionMode::Word => {
                let point = selection_cell(row, x, cols);
                let candidate = if point.col == cols {
                    point
                } else {
                    let word = word_at(screen, point, cols, rows);
                    if point < anchor {
                        word.anchor
                    } else {
                        word.end
                    }
                };
                self.extend_to(candidate, cols)
            }
            // The initial line follows soft wraps. Dragging snaps to the pointer's
            // physical row, including when it lands on a continuation.
            SelectionMode::Line => {
                let col = if row < anchor.row { 0 } else { cols };
                self.extend_to(GridPoint { row, col }, cols)
            }
        }
    }

    fn extend_to(self, candidate: GridPoint, cols: usize) -> GridSelection {
        let GridSelection { anchor, end } = self.initial;
        if candidate < anchor {
            return GridSelection {
                anchor: candidate,
                end,
            };
        }
        let end = if candidate.row != anchor.row {
            candidate
        } else if end.row > anchor.row {
            // The anchored length wraps. An exact column multiple ends at
            // column zero of the following row during a drag on the first row.
            if end.col == cols {
                GridPoint {
                    row: end.row + 1,
                    col: 0,
                }
            } else {
                end
            }
        } else {
            end.max(candidate)
        };
        GridSelection { anchor, end }
    }
}

fn selection_cell(row: usize, x: f32, cols: usize) -> GridPoint {
    // Selection hits use the nearest cell boundary, including one past the last cell.
    let col = ((x + 0.5).ceil() - 1.).clamp(0., cols as f32) as usize;
    GridPoint { row, col }
}

fn wrap_continues(screen: &Screen, row: usize) -> bool {
    screen
        .row_metadata
        .get(row)
        .is_some_and(|row| row.wrap_continuation)
}

fn logical_line_at(screen: &Screen, row: usize, cols: usize, rows: usize) -> GridSelection {
    let mut first = row;
    while first > 0 && wrap_continues(screen, first) {
        first -= 1;
    }
    let mut last = row;
    while last + 1 < rows && wrap_continues(screen, last + 1) {
        last += 1;
    }
    GridSelection {
        anchor: GridPoint { row: first, col: 0 },
        end: GridPoint {
            row: last,
            col: cols,
        },
    }
}

fn row_cells(screen: &Screen, row: usize, cols: usize) -> &[Cell] {
    let start = row * screen.cols;
    &screen.cells[start..start + cols]
}

fn cell_head(cells: &[Cell], mut col: usize) -> usize {
    while col > 0 && cells[col].width == 0 {
        col -= 1;
    }
    col
}

fn word_in_row(cells: &[Cell], col: usize) -> Range<usize> {
    let mut start = cell_head(cells, col);
    let mut end = (start + usize::from(cells[start].width).max(1)).min(cells.len());
    let space = cells[start].text == " ";
    let includes = |cell: &Cell| {
        if space {
            cell.text == " "
        } else {
            cell.has_content && !WORD_SEPARATORS.contains(cell.text.as_str())
        }
    };
    while start > 0 {
        let previous = cell_head(cells, start - 1);
        if !includes(&cells[previous]) {
            break;
        }
        start = previous;
    }
    while end < cells.len() && includes(&cells[end]) {
        end = (end + usize::from(cells[end].width).max(1)).min(cells.len());
    }
    start..end
}

fn written_space(cell: &Cell) -> bool {
    cell.has_content && cell.text == " "
}

fn wrap_word(cells: &[Cell], col: usize) -> Option<Range<usize>> {
    let span = word_in_row(cells, col);
    cells[span.clone()]
        .iter()
        .any(|cell| !cell.text.trim().is_empty())
        .then_some(span)
}

fn word_at(screen: &Screen, point: GridPoint, cols: usize, rows: usize) -> GridSelection {
    let span = word_in_row(row_cells(screen, point.row, cols), point.col);
    let mut anchor = GridPoint {
        row: point.row,
        col: span.start,
    };
    let mut end = GridPoint {
        row: point.row,
        col: span.end,
    };
    while anchor.col == 0
        && anchor.row > 0
        && wrap_continues(screen, anchor.row)
        && !written_space(&row_cells(screen, anchor.row, cols)[0])
    {
        let row = anchor.row - 1;
        let Some(span) = wrap_word(row_cells(screen, row, cols), cols - 1) else {
            break;
        };
        anchor = GridPoint {
            row,
            col: span.start,
        };
    }
    while end.col == cols
        && end.row + 1 < rows
        && wrap_continues(screen, end.row + 1)
        && !written_space(&row_cells(screen, end.row, cols)[cols - 1])
    {
        let row = end.row + 1;
        let Some(span) = wrap_word(row_cells(screen, row, cols), 0) else {
            break;
        };
        end = GridPoint { row, col: span.end };
    }
    GridSelection { anchor, end }
}

/// Treat both cells of a wide grapheme as one hit target.
pub(crate) fn grid_boundary(
    screen: &Screen,
    row: usize,
    x_in_cells: f32,
    cols: usize,
) -> GridPoint {
    let line = row * screen.cols;
    let cols = cols
        .min(screen.cols)
        .min(screen.cells.len().saturating_sub(line));
    let x = x_in_cells.clamp(0., cols as f32);
    if x >= cols as f32 {
        return GridPoint { row, col: cols };
    }
    let col = cell_head(row_cells(screen, row, cols), x.floor() as usize);
    let width = usize::from(screen.cells[line + col].width).max(1);
    GridPoint {
        row,
        col: if x < col as f32 + width as f32 / 2. {
            col
        } else {
            (col + width).min(cols)
        },
    }
}

pub(crate) fn encode_paste(text: &str, modes: InputModes) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !modes.bracketed_paste {
        return text.into_bytes();
    }
    let text = text
        .replace("\x1b[200~", "")
        .replace("\x1b[201~", "")
        .replace(['\x1b', '\u{009b}'], "");
    if text.is_empty() {
        return Vec::new();
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

/// Printable characters are committed by the platform text handler.
pub(crate) fn terminal_key(
    key: &str,
    shift: bool,
    control: bool,
    alt: bool,
    modes: InputModes,
) -> Option<Vec<u8>> {
    let application = modes.application_cursor_keys;
    let special: &[u8] = match key {
        "enter" => b"\r",
        "backspace" => b"\x7f",
        "tab" if shift => b"\x1b[Z",
        "tab" => b"\t",
        "escape" => b"\x1b",
        "up" if application => b"\x1bOA",
        "down" if application => b"\x1bOB",
        "right" if application => b"\x1bOC",
        "left" if application => b"\x1bOD",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "delete" => b"\x1b[3~",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        _ => {
            if control
                && !alt
                && let [letter @ b'a'..=b'z'] = key.as_bytes()
            {
                return Some(vec![letter - b'a' + 1]);
            }
            return None;
        }
    };
    Some(special.to_vec())
}

pub(crate) fn byte_from_utf16(text: &str, requested: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > requested {
            return byte;
        }
    }
    text.len()
}

pub(crate) fn utf16_from_byte(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

fn byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    byte_from_utf16(text, range.start)..byte_from_utf16(text, range.end.max(range.start))
}

#[derive(Default)]
pub(crate) struct Composition {
    pub text: String,
    pub selection: Range<usize>,
}

impl Composition {
    pub fn is_active(&self) -> bool {
        !self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.selection = 0..0;
    }

    pub fn text_for_range(&self, range: Range<usize>) -> (String, Range<usize>) {
        let bytes = byte_range(&self.text, range);
        let adjusted =
            utf16_from_byte(&self.text, bytes.start)..utf16_from_byte(&self.text, bytes.end);
        (self.text[bytes].to_owned(), adjusted)
    }

    pub fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let replaced = range
            .map(|range| byte_range(&self.text, range))
            .unwrap_or(0..self.text.len());
        let start = utf16_from_byte(&self.text, replaced.start);
        self.text.replace_range(replaced, text);
        let marked_len = text.encode_utf16().count();
        let selected = byte_range(text, selected.unwrap_or(marked_len..marked_len));
        self.selection = start + utf16_from_byte(text, selected.start)
            ..start + utf16_from_byte(text, selected.end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matou_native_terminal::{
        Color, CursorStyle, RowMetadata, Scrollbar, Terminal, TerminalOptions,
    };
    use serde_json::Value;

    fn sample() -> Screen {
        let cells = [
            ("x", 1),
            ("中", 2),
            ("", 0),
            ("👩🏽‍💻", 2),
            ("", 0),
            (" ", 1),
            ("后", 2),
            ("", 0),
            ("Z", 1),
            (" ", 1),
            (" ", 1),
            (" ", 1),
        ]
        .into_iter()
        .map(|(text, width)| Cell {
            text: text.into(),
            has_content: width != 0 && text != " ",
            width,
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            italic: false,
            dim: false,
            underline: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
            blink: false,
            underline_style: 0,
            underline_color: Color::Default,
        })
        .collect();
        Screen {
            cols: 6,
            rows: 2,
            cells,
            row_metadata: vec![
                RowMetadata {
                    wrap: false,
                    wrap_continuation: false
                };
                2
            ],
            cursor_col: 0,
            cursor_row: 0,
            cursor_has_position: true,
            cursor_visible: true,
            cursor_style: CursorStyle::Bar,
            cursor_blinking: false,
            foreground: 0,
            background: 0,
            cursor_color: None,
            palette: [0; 256],
            history_lines: 0,
            scrollbar: Scrollbar {
                offset: 0,
                total: 2,
                len: 2,
            },
            alternate: false,
        }
    }

    #[test]
    fn drag_copies_wide_graphemes_once_and_joins_soft_rows() {
        let mut screen = sample();
        let start = grid_boundary(&screen, 0, 1.2, 6);
        let end = grid_boundary(&screen, 1, 2.7, 6);
        let selection = GridSelection {
            anchor: end,
            end: start,
        };
        assert_eq!(selection.text(&screen, 6), "中👩🏽‍💻\n后Z");
        assert_eq!(
            grid_boundary(&screen, 0, 2.4, 6),
            GridPoint { row: 0, col: 3 }
        );
        screen.cells[5].has_content = true;
        screen.row_metadata[0].wrap = true;
        screen.row_metadata[1].wrap_continuation = true;
        assert_eq!(selection.text(&screen, 6), "中👩🏽‍💻 后Z");
    }

    #[test]
    fn drag_preserves_written_spaces_and_trims_empty_cells_in_real_output() -> anyhow::Result<()> {
        let mut terminal = Terminal::new(TerminalOptions::new(6, 4, 7, 13))?;
        terminal.feed("中 \r\nABCD  Z中  ".as_bytes());
        let screen = terminal.screen()?;
        assert!(screen.cells[2].has_content);
        assert!(!screen.cells[3].has_content);
        assert!(!screen.cells[1].has_content);
        assert_eq!(screen.cells[2].text, screen.cells[3].text);
        assert!(screen.row_metadata[2].wrap_continuation);
        let selection = GridSelection {
            anchor: GridPoint { row: 0, col: 0 },
            end: GridPoint { row: 2, col: 6 },
        };
        assert_eq!(selection.text(&screen, 6), "中 \nABCD  Z中  ");
        Ok(())
    }

    #[test]
    fn word_and_line_selection_match_the_reference() -> anyhow::Result<()> {
        let data: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/terminal-selection-reference.json"
        ))?;
        let point = |value: &Value| GridPoint {
            col: value[0].as_u64().unwrap() as usize,
            row: value[1].as_u64().unwrap() as usize,
        };
        for case in data["cases"].as_array().unwrap() {
            let mut screen = sample();
            let plain = screen.cells[0].clone();
            screen.cols = case["cols"].as_u64().unwrap() as usize;
            screen.rows = case["rows"].as_u64().unwrap() as usize;
            screen.cells.clear();
            screen.row_metadata.clear();
            for row in case["grid"].as_array().unwrap() {
                screen.row_metadata.push(RowMetadata {
                    wrap: false,
                    wrap_continuation: row["continuation"].as_bool().unwrap(),
                });
                screen
                    .cells
                    .extend(row["cells"].as_array().unwrap().iter().map(|cell| Cell {
                        text: cell["text"].as_str().unwrap().to_owned(),
                        width: cell["width"].as_u64().unwrap() as u8,
                        has_content: cell["hasContent"].as_bool().unwrap(),
                        ..plain.clone()
                    }));
            }
            for probe in case["probes"].as_array().unwrap() {
                let mode = match probe["mode"].as_str().unwrap() {
                    "word" => SelectionMode::Word,
                    "line" => SelectionMode::Line,
                    mode => panic!("Unexpected selection mode: {mode}"),
                };
                let start = point(&probe["point"]);
                let drag = SelectionDrag::begin(
                    mode,
                    &screen,
                    start.row,
                    start.col as f32,
                    screen.cols,
                    screen.rows,
                );
                let selection = if let Some(target) = probe.get("target").map(point) {
                    drag.extend(
                        &screen,
                        target.row,
                        target.col as f32,
                        screen.cols,
                        screen.rows,
                    )
                } else {
                    drag.initial
                };
                let expected = &probe["expected"];
                let name = &case["name"];
                assert_eq!(
                    selection,
                    GridSelection {
                        anchor: point(&expected["start"]),
                        end: point(&expected["end"]),
                    },
                    "{name}: {probe}"
                );
                assert_eq!(
                    selection.text(&screen, screen.cols),
                    expected["text"].as_str().unwrap(),
                    "{name}: {probe}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn ime_uses_utf16_offsets_without_splitting_surrogates() {
        let mut mark = Composition::default();
        mark.replace(None, "中🙂a", Some(1..3));
        assert_eq!(mark.selection, 1..3);
        assert_eq!(mark.text_for_range(2..3), ("🙂".into(), 1..3));
        mark.replace(Some(2..3), "字", Some(1..1));
        assert_eq!(mark.text, "中字a");
        assert_eq!(mark.selection, 2..2);
        mark.clear();
        assert!(!mark.is_active());
        assert_eq!(mark.selection, 0..0);
    }
}
