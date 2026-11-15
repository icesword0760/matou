use std::ops::Range;

use anyhow::Result;

use crate::{HistoryCell, Terminal};

const WORD_SEPARATORS: &str = " ()[]{}',\"`";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    pub row: usize,
    pub col: usize,
}

/// Half-open selection in reading order over absolute rows of the active buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSelection {
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

    pub fn text(self, terminal: &mut Terminal) -> Result<String> {
        if self.anchor == self.end {
            return Ok(String::new());
        }
        let (cols, total) = grid_size(terminal)?;
        let first = self.anchor.min(self.end).row;
        let last = self.anchor.max(self.end).row;
        let mut result = String::new();
        for row in first..=last.min(total - 1) {
            let history = terminal.history_row(row)?;
            if row > first && !history.metadata.wrap_continuation {
                result.push('\n');
            }
            let cells = &history.cells;
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
        }
        if first < total && last >= total {
            result.push('\n');
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    Character,
    Word,
    Line,
}

#[derive(Clone, Copy)]
pub struct SelectionDrag {
    pub mode: SelectionMode,
    pub initial: GridSelection,
}

impl SelectionDrag {
    pub fn begin(terminal: &mut Terminal, mode: SelectionMode, row: usize, x: f32) -> Result<Self> {
        let (cols, total) = grid_size(terminal)?;
        let row = row.min(total - 1);
        let point = selection_cell(row, x, cols);
        let mode = if mode == SelectionMode::Word && point.col == cols {
            SelectionMode::Character
        } else {
            mode
        };
        let initial = match mode {
            SelectionMode::Character => {
                let anchor = grid_boundary(terminal, row, x, cols)?;
                GridSelection {
                    anchor,
                    end: anchor,
                }
            }
            SelectionMode::Word => word_at(terminal, point, cols, total)?,
            SelectionMode::Line => logical_line_at(terminal, row, cols, total)?,
        };
        Ok(Self { mode, initial })
    }

    pub fn extend(self, terminal: &mut Terminal, row: usize, x: f32) -> Result<GridSelection> {
        let (cols, total) = grid_size(terminal)?;
        let row = row.min(total - 1);
        let anchor = self.initial.anchor;
        Ok(match self.mode {
            SelectionMode::Character => GridSelection {
                anchor,
                end: grid_boundary(terminal, row, x, cols)?,
            },
            SelectionMode::Word => {
                let point = selection_cell(row, x, cols);
                let candidate = if point.col == cols {
                    point
                } else {
                    let word = word_at(terminal, point, cols, total)?;
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
        })
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
            col: cols,
        },
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
    let span = word_in_row(&hit.cells, point.col);
    let mut anchor = GridPoint {
        row: point.row,
        col: span.start,
    };
    let mut end = GridPoint {
        row: point.row,
        col: span.end,
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
            col: span.start,
        };
    }
    let mut no_trailing_space = !written_space(&hit.cells[cols - 1]);
    while end.col == cols && end.row + 1 < total && no_trailing_space {
        let row = end.row + 1;
        let next = terminal.history_row(row)?;
        if !next.metadata.wrap_continuation {
            break;
        }
        let Some(span) = wrap_word(&next.cells, 0) else {
            break;
        };
        no_trailing_space = !written_space(&next.cells[cols - 1]);
        end = GridPoint { row, col: span.end };
    }
    Ok(GridSelection { anchor, end })
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
        return Ok(GridPoint { row, col: cols });
    }
    let history = terminal.history_row(row)?;
    let col = cell_head(&history.cells, x.floor() as usize);
    let width = usize::from(history.cells[col].width).max(1);
    Ok(GridPoint {
        row,
        col: if x < col as f32 + width as f32 / 2. {
            col
        } else {
            (col + width).min(cols)
        },
    })
}
