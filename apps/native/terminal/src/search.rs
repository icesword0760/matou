use std::{cell::OnceCell, collections::HashMap, ops::Range};

use anyhow::{Context, Result, ensure};
use regress::{Flags, Regex};
use serde::{Deserialize, Serialize};

use crate::{GridSelection, HistoryRow, Terminal};

const NON_WORD_CHARACTERS: &[u8] = b" ~!@#$%^&*()+`-=[]{}|\\;:\"',./<>?";
const HIGHLIGHT_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchMatch {
    /// A non-Unicode regular expression can match part of a surrogate pair.
    pub term: Vec<u16>,
    pub row: usize,
    pub col: isize,
    pub size: isize,
}

#[derive(Clone, Copy)]
struct Position {
    row: isize,
    col: isize,
}

struct LogicalLine {
    text: String,
    offsets: Vec<usize>,
    units: OnceCell<Vec<u16>>,
    lowercase: OnceCell<Vec<u16>>,
}

impl LogicalLine {
    fn units(&self, lowercase: bool) -> &[u16] {
        if lowercase {
            self.lowercase
                .get_or_init(|| self.text.to_lowercase().encode_utf16().collect())
        } else {
            self.units
                .get_or_init(|| self.text.encode_utf16().collect())
        }
    }
}

enum Pattern {
    Literal(Vec<u16>),
    Regex(Regex),
}

struct Query {
    term: Vec<u16>,
    pattern: Pattern,
    options: SearchOptions,
}

impl Query {
    fn new(term: &str, options: SearchOptions) -> Result<Option<Self>> {
        if term.is_empty() {
            return Ok(None);
        }
        let original: Vec<u16> = term.encode_utf16().collect();
        let pattern = if options.regex {
            let flags = Flags {
                icase: !options.case_sensitive,
                ..Flags::default()
            };
            Pattern::Regex(
                Regex::from_unicode(original.iter().copied().map(u32::from), flags)
                    .context("invalid terminal search regular expression")?,
            )
        } else if options.case_sensitive {
            Pattern::Literal(original.clone())
        } else {
            Pattern::Literal(term.to_lowercase().encode_utf16().collect())
        };
        Ok(Some(Self {
            term: original,
            pattern,
            options,
        }))
    }

    fn locate(&self, line: &[u16], offset: usize, reverse: bool) -> Option<(usize, Vec<u16>)> {
        let (index, term) = match &self.pattern {
            Pattern::Literal(needle) => {
                let index = if reverse {
                    offset.checked_sub(needle.len())?;
                    line[..offset.min(line.len())]
                        .windows(needle.len())
                        .rposition(|window| window == needle)?
                } else {
                    let offset = offset.min(line.len());
                    offset
                        + line[offset..]
                            .windows(needle.len())
                            .position(|window| window == needle)?
                };
                (index, self.term.as_slice())
            }
            Pattern::Regex(regex) => {
                let found = if reverse {
                    last_regex_match(regex, &line[..offset.min(line.len())])?
                } else {
                    let offset = offset.min(line.len());
                    let found = regex.find_from_ucs2(&line[offset..], 0).next()?.range;
                    if found.is_empty() {
                        return None;
                    }
                    (offset + found.start)..(offset + found.end)
                };
                (found.start, &line[found])
            }
        };
        if self.options.whole_word && !whole_word(line, index, term.len()) {
            return None;
        }
        Some((index, term.to_vec()))
    }
}

fn last_regex_match(regex: &Regex, text: &[u16]) -> Option<Range<usize>> {
    let mut index = 0;
    let mut last = None;
    while let Some(found) = regex.find_from_ucs2(text, index).next() {
        let start = found.range.start;
        last = Some(found.range);
        if start == text.len() {
            break;
        }
        // The reference searches overlapping and zero-length matches.
        index = start + 1;
    }
    last
}

fn whole_word(line: &[u16], start: usize, len: usize) -> bool {
    let boundary =
        |unit: &u16| u8::try_from(*unit).is_ok_and(|byte| NON_WORD_CHARACTERS.contains(&byte));
    let end = start + len;
    (start == 0 || boundary(&line[start - 1]))
        && (end == line.len() || line.get(end).is_some_and(boundary))
}

/// Searches the current buffer, retaining rows and wrapped strings for this borrow.
pub struct SearchEngine<'a> {
    terminal: &'a mut Terminal,
    cols: isize,
    total: isize,
    rows: HashMap<isize, HistoryRow>,
    lines: HashMap<isize, LogicalLine>,
}

impl<'a> SearchEngine<'a> {
    pub fn new(terminal: &'a mut Terminal) -> Result<Self> {
        let screen = terminal.screen()?;
        Ok(Self {
            terminal,
            cols: isize::try_from(screen.cols)?,
            total: isize::try_from(screen.scrollbar.total)?,
            rows: HashMap::new(),
            lines: HashMap::new(),
        })
    }

    pub fn find(
        &mut self,
        term: &str,
        row: usize,
        col: isize,
        options: SearchOptions,
    ) -> Result<Option<SearchMatch>> {
        let Some(query) = Query::new(term, options)? else {
            return Ok(None);
        };
        ensure!(
            col <= self.cols,
            "invalid column {col} to search in terminal of {} columns",
            self.cols
        );
        self.find_forward(
            &query,
            Position {
                row: row.try_into()?,
                col,
            },
        )
    }

    pub fn find_next(
        &mut self,
        term: &str,
        options: SearchOptions,
        previous: Option<GridSelection>,
        cached_term: Option<&str>,
    ) -> Result<Option<SearchMatch>> {
        let Some(query) = Query::new(term, options)? else {
            return Ok(None);
        };
        let start = if let Some(selection) = previous {
            let point = if cached_term == Some(term) {
                selection.anchor.max(selection.end)
            } else {
                selection.anchor.min(selection.end)
            };
            Position {
                row: point.row.try_into()?,
                col: point.col.try_into()?,
            }
        } else {
            Position { row: 0, col: 0 }
        };
        if let Some(found) = self.find_forward(&query, start)? {
            return Ok(Some(found));
        }
        for row in 0..start.row.min(self.total) {
            if let Some(found) = self.find_in_line(&query, &mut Position { row, col: 0 }, false)? {
                return Ok(Some(found));
            }
        }
        if let Some(selection) = previous {
            let row = selection.anchor.min(selection.end).row.try_into()?;
            return self.find_in_line(&query, &mut Position { row, col: 0 }, false);
        }
        Ok(None)
    }

    pub fn find_previous(
        &mut self,
        term: &str,
        options: SearchOptions,
        previous: Option<GridSelection>,
        cached_term: Option<&str>,
    ) -> Result<Option<SearchMatch>> {
        let Some(query) = Query::new(term, options)? else {
            return Ok(None);
        };
        let last_row = self.total - 1;
        let mut position = Position {
            row: last_row,
            col: self.cols,
        };
        if let Some(selection) = previous {
            let point = selection.anchor.min(selection.end);
            position = Position {
                row: point.row.try_into()?,
                col: point.col.try_into()?,
            };
            if cached_term != Some(term) {
                if let Some(found) = self.find_in_line(&query, &mut position, false)? {
                    return Ok(Some(found));
                }
                let point = selection.anchor.max(selection.end);
                position = Position {
                    row: point.row.try_into()?,
                    col: point.col.try_into()?,
                };
            }
        }
        let start_row = position.row;
        if let Some(found) = self.find_in_line(&query, &mut position, true)? {
            return Ok(Some(found));
        }
        position.col = position.col.max(self.cols);
        for row in (0..start_row.min(self.total)).rev() {
            position.row = row;
            if let Some(found) = self.find_in_line(&query, &mut position, true)? {
                return Ok(Some(found));
            }
        }
        if start_row != last_row {
            for row in (start_row..self.total).rev() {
                position.row = row;
                if let Some(found) = self.find_in_line(&query, &mut position, true)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }

    pub fn all_matches(&mut self, term: &str, options: SearchOptions) -> Result<Vec<SearchMatch>> {
        let Some(query) = Query::new(term, options)? else {
            return Ok(Vec::new());
        };
        let mut matches: Vec<SearchMatch> = Vec::new();
        let mut position = Position { row: 0, col: 0 };
        while matches.len() < HIGHLIGHT_LIMIT {
            let Some(found) = self.find_forward(&query, position)? else {
                break;
            };
            if matches
                .last()
                .is_some_and(|last| (last.row, last.col) == (found.row, found.col))
            {
                break;
            }
            let row = isize::try_from(found.row)?;
            position = if found.col + isize::try_from(found.term.len())? >= self.cols {
                Position {
                    row: row + 1,
                    col: 0,
                }
            } else {
                Position {
                    row,
                    col: found.col + 1,
                }
            };
            matches.push(found);
        }
        Ok(matches)
    }

    fn find_forward(&mut self, query: &Query, start: Position) -> Result<Option<SearchMatch>> {
        let mut position = start;
        if let Some(found) = self.find_in_line(query, &mut position, false)? {
            return Ok(Some(found));
        }
        if start.row >= self.total - 1 {
            return Ok(None);
        }
        for row in (start.row + 1)..self.total {
            if let Some(found) = self.find_in_line(query, &mut Position { row, col: 0 }, false)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    fn find_in_line(
        &mut self,
        query: &Query,
        position: &mut Position,
        reverse: bool,
    ) -> Result<Option<SearchMatch>> {
        while self
            .row(position.row)?
            .is_some_and(|row| row.metadata.wrap_continuation)
        {
            position.col = position
                .col
                .checked_add(self.cols)
                .context("search column overflow")?;
            if reverse {
                return Ok(None);
            }
            position.row -= 1;
        }
        let row = position.row;
        let offset = self.buffer_cols_to_string_offset(row, position.col)?;
        let line = self.logical_line(row)?;
        let lowercase = !query.options.regex && !query.options.case_sensitive;
        let Some((start, term)) = query.locate(line.units(lowercase), offset, reverse) else {
            return Ok(None);
        };
        let end = start + term.len();
        let start_offset = line.offsets.partition_point(|&offset| offset <= start) - 1;
        let end_offset = line.offsets.partition_point(|&offset| offset <= end) - 1;
        let start_column = start - line.offsets[start_offset];
        let end_column = end - line.offsets[end_offset];
        let start_row = row + isize::try_from(start_offset)?;
        let end_row = row + isize::try_from(end_offset)?;
        let col = self.string_length_to_buffer_size(start_row, start_column)?;
        let end_col = self.string_length_to_buffer_size(end_row, end_column)?;
        Ok(Some(SearchMatch {
            term,
            row: start_row.try_into()?,
            col,
            size: end_col - col + self.cols * (end_row - start_row),
        }))
    }

    fn row(&mut self, row: isize) -> Result<Option<&HistoryRow>> {
        if row < 0 || row >= self.total {
            return Ok(None);
        }
        let cached = match self.rows.entry(row) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(self.terminal.history_row(row as usize)?)
            }
        };
        Ok(Some(cached))
    }

    fn logical_line(&mut self, row: isize) -> Result<&LogicalLine> {
        if !self.lines.contains_key(&row) {
            let line = self.build_line(row)?;
            self.lines.insert(row, line);
        }
        Ok(&self.lines[&row])
    }

    fn build_line(&mut self, mut row: isize) -> Result<LogicalLine> {
        let mut text = String::new();
        let mut offsets = vec![0];
        let mut units = 0;
        while self.row(row)?.is_some() {
            let next = self.row(row + 1)?;
            let wrap = next.is_some_and(|line| line.metadata.wrap_continuation);
            let wide_next = next
                .and_then(|line| line.cells.first())
                .is_some_and(|cell| cell.width == 2);
            let line = self.row(row)?.context("missing cached terminal line")?;
            let cells = &line.cells;
            let last = if wrap {
                let blank_pad = cells
                    .last()
                    .is_some_and(|cell| cell.width == 1 && !cell.has_content);
                cells.len() - usize::from(blank_pad && wide_next)
            } else {
                cells
                    .iter()
                    .rposition(|cell| cell.has_content)
                    .map_or(0, |col| {
                        (col + usize::from(cells[col].width)).min(cells.len())
                    })
            };
            for cell in &cells[..last] {
                if cell.width != 0 {
                    let chars = if cell.has_content { &cell.text } else { " " };
                    text.push_str(chars);
                    units += chars.encode_utf16().count();
                }
            }
            if !wrap {
                break;
            }
            offsets.push(units);
            row += 1;
        }
        Ok(LogicalLine {
            text,
            offsets,
            units: OnceCell::new(),
            lowercase: OnceCell::new(),
        })
    }

    fn buffer_cols_to_string_offset(&mut self, mut row: isize, mut cols: isize) -> Result<usize> {
        let mut offset = 0;
        let width = self.cols;
        while cols > 0 {
            let Some(line) = self.row(row)? else {
                break;
            };
            for cell in line.cells.iter().take(cols.min(width) as usize) {
                if cell.width != 0 {
                    offset += if cell.has_content {
                        cell.text.encode_utf16().count()
                    } else {
                        1
                    };
                }
            }
            row += 1;
            cols -= width;
            if cols > 0
                && self
                    .row(row)?
                    .is_some_and(|line| !line.metadata.wrap_continuation)
            {
                break;
            }
        }
        Ok(offset)
    }

    fn string_length_to_buffer_size(&mut self, row: isize, offset: usize) -> Result<isize> {
        let Some(line) = self.row(row)? else {
            return Ok(0);
        };
        let mut offset = isize::try_from(offset)?;
        for (col, cell) in line.cells.iter().enumerate() {
            if col as isize >= offset {
                break;
            }
            let units = isize::try_from(cell.text.encode_utf16().count())?;
            if units > 1 {
                offset -= units - 1;
            }
            if line.cells.get(col + 1).is_some_and(|next| next.width == 0) {
                offset += 1;
            }
        }
        Ok(offset)
    }
}
