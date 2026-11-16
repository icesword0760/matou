use std::ops::Range;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{HistoryCell, Terminal};

const WORD_SEPARATORS: &str = " ()[]{}',\"`";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GridPoint {
    pub row: usize,
    pub col: isize,
}

/// Selection over absolute buffer rows, either linear or rectangular.
/// Columns are half-open, including on each row of a rectangular selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridSelection {
    pub anchor: GridPoint,
    pub end: GridPoint,
    pub column: bool,
}

impl GridSelection {
    pub fn range(self, row: usize, cols: usize) -> Range<usize> {
        let cols = cols as isize;
        let range = self.columns(row, cols);
        range.start.clamp(0, cols) as usize..range.end.clamp(0, cols) as usize
    }

    fn columns(self, row: usize, cols: isize) -> Range<isize> {
        let first = self.anchor;
        let last = self.end;
        if row < first.row || row > last.row {
            return 0..0;
        }
        let (left, right) = if self.column {
            (
                self.anchor.col.min(self.end.col),
                self.anchor.col.max(self.end.col),
            )
        } else {
            (
                if row == first.row { first.col } else { 0 },
                if row == last.row { last.col } else { cols },
            )
        };
        left..right
    }

    pub fn text(self, terminal: &mut Terminal) -> Result<String> {
        if self.anchor == self.end || (self.column && self.anchor.col == self.end.col) {
            return Ok(String::new());
        }
        let (cols, total) = grid_size(terminal)?;
        let first = self.anchor.row;
        let last = self.end.row;
        let mut result = String::new();
        for row in first..=last.min(total - 1) {
            let history = terminal.history_row(row)?;
            if row > first && (self.column || !history.metadata.wrap_continuation) {
                result.push('\n');
            }
            let cells = &history.cells;
            let range = self.columns(row, cols.min(cells.len()) as isize);
            let content_end = cells
                .iter()
                .enumerate()
                .rev()
                .find(|(_, cell)| cell.width != 0 && cell.has_content)
                .map_or(0, |(col, cell)| col + usize::from(cell.width));
            let end = range.end.min(content_end as isize);
            let mut col = range.start;
            if col < 0 && col < end {
                // The renderer buffer translates missing cells before column zero as spaces.
                let stop = end.min(0);
                let spaces = usize::try_from(i128::from(stop as i64) - i128::from(col as i64))?;
                result.try_reserve(spaces)?;
                result.extend(std::iter::repeat_n(' ', spaces));
                col = stop;
            }
            while col < end {
                let cell = &cells[col as usize];
                if cell.width == 0 || cell.text.is_empty() {
                    result.push(' ');
                } else {
                    result.push_str(&cell.text);
                }
                col += isize::from(cell.width).max(1);
            }
        }
        if first < total && last >= total {
            result.push('\n');
        }
        Ok(result.replace('\u{a0}', " "))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionMode {
    Character,
    Word,
    Line,
    Column,
}

/// Pointer operations use absolute rows of the active terminal buffer.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum SelectionAction {
    Select {
        row: usize,
        col: isize,
        length: isize,
    },
    Begin {
        mode: SelectionMode,
        row: usize,
        x: f32,
    },
    IncrementalClick {
        row: usize,
        x: f32,
    },
    Extend {
        row: usize,
        x: f32,
    },
    ExtendOutside {
        row: usize,
        x: f32,
        below: bool,
    },
    Scroll {
        lines: i32,
    },
    SetEnd(GridPoint),
    Clear,
}

#[derive(Clone, Copy)]
pub struct SelectionDrag {
    pub mode: SelectionMode,
    pub initial: GridSelection,
}

impl SelectionDrag {
    pub fn begin(terminal: &mut Terminal, mode: SelectionMode, row: usize, x: f32) -> Result<Self> {
        let (cols, total) = grid_size(terminal)?;
        Self::begin_on_grid(terminal, mode, row, x, cols, total)
    }

    fn begin_on_grid(
        terminal: &Terminal,
        mode: SelectionMode,
        row: usize,
        x: f32,
        cols: usize,
        total: usize,
    ) -> Result<Self> {
        let row = row.min(total - 1);
        let point = selection_cell(row, x, cols);
        let mode = if mode == SelectionMode::Word && point.col == cols as isize {
            SelectionMode::Character
        } else {
            mode
        };
        let initial = match mode {
            SelectionMode::Character | SelectionMode::Column => {
                let anchor = grid_boundary(terminal, row, x, cols)?;
                GridSelection {
                    anchor,
                    end: anchor,
                    column: mode == SelectionMode::Column,
                }
            }
            SelectionMode::Word => word_at(terminal, point, cols, total)?,
            SelectionMode::Line => logical_line_at(terminal, row, cols, total)?,
        };
        Ok(Self { mode, initial })
    }

    pub fn extend(self, terminal: &mut Terminal, row: usize, x: f32) -> Result<GridSelection> {
        let (cols, total) = grid_size(terminal)?;
        let anchor = self.initial.anchor;
        let candidate = selection_candidate(terminal, self.mode, anchor, row, x, cols, total)?;
        if matches!(self.mode, SelectionMode::Character | SelectionMode::Column) {
            return Ok(GridSelection {
                anchor: anchor.min(candidate),
                end: anchor.max(candidate),
                column: self.mode == SelectionMode::Column,
            });
        }
        Ok(self.extend_to(candidate, cols))
    }

    fn extend_to(self, candidate: GridPoint, cols: usize) -> GridSelection {
        let GridSelection { anchor, end, .. } = self.initial;
        if candidate < anchor {
            return GridSelection {
                anchor: candidate,
                end,
                column: false,
            };
        }
        let end = if candidate.row != anchor.row {
            candidate
        } else if end.row > anchor.row {
            // The anchored length wraps. An exact column multiple ends at
            // column zero of the following row during a drag on the first row.
            if end.col == cols as isize {
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
        GridSelection {
            anchor,
            end,
            column: false,
        }
    }
}

/// A selection in the active buffer, including the length established by its initial click.
pub struct SelectionState {
    mode: SelectionMode,
    start: Option<GridPoint>,
    start_length: isize,
    end: Option<GridPoint>,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            mode: SelectionMode::Character,
            start: None,
            start_length: 0,
            end: None,
        }
    }
}

impl SelectionState {
    pub fn select(&mut self, row: usize, col: isize, length: isize) -> Result<()> {
        col.checked_add(length)
            .context("programmatic selection column overflow")?;
        self.start = Some(GridPoint { row, col });
        self.start_length = length;
        self.end = None;
        Ok(())
    }

    pub fn begin(
        &mut self,
        terminal: &mut Terminal,
        mode: SelectionMode,
        row: usize,
        x: f32,
    ) -> Result<()> {
        let (cols, total) = grid_size(terminal)?;
        let drag = SelectionDrag::begin_on_grid(terminal, mode, row, x, cols, total)?;
        let GridSelection { anchor, end, .. } = drag.initial;
        *self = Self {
            mode: drag.mode,
            start: Some(anchor),
            start_length: ((end.row - anchor.row) * cols) as isize + end.col - anchor.col,
            end: None,
        };
        Ok(())
    }

    pub fn extend(&mut self, terminal: &mut Terminal, row: usize, x: f32) -> Result<()> {
        let Some(start) = self.start else {
            return Ok(());
        };
        let (cols, total) = grid_size(terminal)?;
        self.set_end(selection_candidate(
            terminal, self.mode, start, row, x, cols, total,
        )?);
        Ok(())
    }

    pub fn incremental_click(&mut self, terminal: &mut Terminal, row: usize, x: f32) -> Result<()> {
        if self.start.is_some() {
            let (cols, total) = grid_size(terminal)?;
            self.set_end(selection_cell(row.min(total - 1), x, cols));
        }
        Ok(())
    }

    pub fn extend_outside(
        &mut self,
        terminal: &mut Terminal,
        row: usize,
        x: f32,
        below: bool,
    ) -> Result<()> {
        self.extend(terminal, row, x)?;
        if self.mode != SelectionMode::Column
            && let Some(end) = &mut self.end
        {
            end.col = if below {
                terminal.screen()?.cols as isize
            } else {
                0
            };
        }
        Ok(())
    }

    pub fn scroll(&mut self, terminal: &mut Terminal, lines: i32) -> Result<()> {
        if self.start.is_none() || lines == 0 {
            return Ok(());
        }
        let Some(end) = &mut self.end else {
            return Ok(());
        };
        terminal.scroll(lines)?;
        let screen = terminal.screen()?;
        let offset = usize::try_from(screen.scrollbar.offset)?;
        end.row = if lines > 0 {
            offset
        } else {
            let last_row = usize::try_from(screen.scrollbar.total)? - 1;
            (offset + screen.rows).min(last_row)
        };
        if self.mode != SelectionMode::Column {
            end.col = if lines > 0 { 0 } else { screen.cols as isize };
        }
        Ok(())
    }

    /// Complete a drag step at a raw cell boundary, without expanding by word or line.
    pub fn set_end(&mut self, end: GridPoint) {
        self.end = Some(end);
    }

    pub fn range(&self, cols: usize) -> Option<GridSelection> {
        let cols = cols as isize;
        let start = self.start?;
        let (anchor, forward_end) = match self.end {
            Some(end) if end < start => (end, None),
            end => (start, end),
        };
        let initial_end_col = start.col + self.start_length;
        let end = match forward_end {
            Some(end) if self.start_length == 0 || end.row != start.row => end,
            Some(end) if initial_end_col <= cols => GridPoint {
                row: end.row,
                col: initial_end_col.max(end.col),
            },
            None if initial_end_col <= cols => GridPoint {
                row: start.row,
                col: initial_end_col,
            },
            None if initial_end_col % cols == 0 => GridPoint {
                row: start
                    .row
                    .checked_add((initial_end_col / cols - 1) as usize)?,
                col: cols,
            },
            _ => GridPoint {
                row: start.row.checked_add((initial_end_col / cols) as usize)?,
                col: initial_end_col % cols,
            },
        };
        Some(GridSelection {
            anchor,
            end,
            column: self.mode == SelectionMode::Column,
        })
    }

    pub fn trim(&mut self, amount: usize) {
        if self.end.is_some_and(|end| end.row < amount) {
            self.clear();
            return;
        }
        if let Some(start) = &mut self.start {
            start.row = start.row.saturating_sub(amount);
        }
        if let Some(end) = &mut self.end {
            end.row -= amount;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

fn selection_candidate(
    terminal: &Terminal,
    mode: SelectionMode,
    start: GridPoint,
    row: usize,
    x: f32,
    cols: usize,
    total: usize,
) -> Result<GridPoint> {
    let row = row.min(total - 1);
    match mode {
        SelectionMode::Character | SelectionMode::Column => grid_boundary(terminal, row, x, cols),
        SelectionMode::Word => {
            let point = selection_cell(row, x, cols);
            if point.col == cols as isize {
                return Ok(point);
            }
            let word = word_at(terminal, point, cols, total)?;
            Ok(if point < start { word.anchor } else { word.end })
        }
        // The initial line follows soft wraps. Dragging snaps to the pointer's
        // physical row, including when it lands on a continuation.
        SelectionMode::Line => Ok(GridPoint {
            row,
            col: if row < start.row { 0 } else { cols as isize },
        }),
    }
}

fn selection_cell(row: usize, x: f32, cols: usize) -> GridPoint {
    // Selection hits use the nearest cell boundary, including one past the last cell.
    let col = ((x + 0.5).ceil() - 1.).clamp(0., cols as f32) as isize;
    GridPoint { row, col }
}

fn grid_size(terminal: &mut Terminal) -> Result<(usize, usize)> {
    let screen = terminal.screen()?;
    Ok((screen.cols, screen.scrollbar.total.try_into()?))
}

fn logical_line_at(
    terminal: &Terminal,
    row: usize,
    cols: usize,
    total: usize,
) -> Result<GridSelection> {
    let mut first = row;
    while first > 0 && terminal.history_row(first)?.metadata.wrap_continuation {
        first -= 1;
    }
    let mut last = row;
    while last + 1 < total && terminal.history_row(last + 1)?.metadata.wrap_continuation {
        last += 1;
    }
    Ok(GridSelection {
        anchor: GridPoint { row: first, col: 0 },
        end: GridPoint {
            row: last,
            col: cols as isize,
        },
        column: false,
    })
}

fn cell_head(cells: &[HistoryCell], mut col: usize) -> usize {
    while col > 0 && cells[col].width == 0 {
        col -= 1;
    }
    col
}

fn word_in_row(cells: &[HistoryCell], col: usize) -> Range<usize> {
    let mut start = cell_head(cells, col);
    let mut end = (start + usize::from(cells[start].width).max(1)).min(cells.len());
    let space = cells[start].text == " ";
    let includes = |cell: &HistoryCell| {
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

fn written_space(cell: &HistoryCell) -> bool {
    cell.has_content && cell.text == " "
}

fn wrap_word(cells: &[HistoryCell], col: usize) -> Option<Range<usize>> {
    let span = word_in_row(cells, col);
    cells[span.clone()]
        .iter()
        .any(|cell| !cell.text.trim().is_empty())
        .then_some(span)
}

fn word_at(
    terminal: &Terminal,
    point: GridPoint,
    cols: usize,
    total: usize,
) -> Result<GridSelection> {
    let hit = terminal.history_row(point.row)?;
    let span = word_in_row(&hit.cells, point.col as usize);
    let mut anchor = GridPoint {
        row: point.row,
        col: span.start as isize,
    };
    let mut end = GridPoint {
        row: point.row,
        col: span.end as isize,
    };
    let mut follows_previous = hit.metadata.wrap_continuation && !written_space(&hit.cells[0]);
    while anchor.col == 0 && anchor.row > 0 && follows_previous {
        let row = anchor.row - 1;
        let previous = terminal.history_row(row)?;
        let Some(span) = wrap_word(&previous.cells, cols - 1) else {
            break;
        };
        follows_previous =
            previous.metadata.wrap_continuation && !written_space(&previous.cells[0]);
        anchor = GridPoint {
            row,
            col: span.start as isize,
        };
    }
    let mut no_trailing_space = !written_space(&hit.cells[cols - 1]);
    while end.col == cols as isize && end.row + 1 < total && no_trailing_space {
        let row = end.row + 1;
        let next = terminal.history_row(row)?;
        if !next.metadata.wrap_continuation {
            break;
        }
        let Some(span) = wrap_word(&next.cells, 0) else {
            break;
        };
        no_trailing_space = !written_space(&next.cells[cols - 1]);
        end = GridPoint {
            row,
            col: span.end as isize,
        };
    }
    Ok(GridSelection {
        anchor,
        end,
        column: false,
    })
}

/// Treat both cells of a wide grapheme as one hit target.
fn grid_boundary(
    terminal: &Terminal,
    row: usize,
    x_in_cells: f32,
    cols: usize,
) -> Result<GridPoint> {
    let x = x_in_cells.clamp(0., cols as f32);
    if x >= cols as f32 {
        return Ok(GridPoint {
            row,
            col: cols as isize,
        });
    }
    let history = terminal.history_row(row)?;
    let col = cell_head(&history.cells, x.floor() as usize);
    let width = usize::from(history.cells[col].width).max(1);
    Ok(GridPoint {
        row,
        col: if x < col as f32 + width as f32 / 2. {
            col as isize
        } else {
            (col + width).min(cols) as isize
        },
    })
}
