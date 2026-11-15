use std::time::{Duration, Instant};

use anyhow::Result;
use matou_native_terminal::{SearchEngine, SearchMatch, SearchOptions, SelectionAction, Terminal};

const REFRESH_DELAY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub sequence: u64,
    pub query: String,
    pub options: SearchOptions,
    pub previous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub sequence: u64,
    pub matches: Vec<SearchMatch>,
    pub active: Option<SearchMatch>,
    pub result_index: isize,
}

#[derive(Default)]
pub(crate) struct SessionSearch {
    cached_query: Option<String>,
    last_options: SearchOptions,
    sequence: u64,
    matches: Vec<SearchMatch>,
    deadline: Option<Instant>,
}

impl SessionSearch {
    pub(crate) fn search(
        &mut self,
        terminal: &mut Terminal,
        request: SearchRequest,
    ) -> Result<SearchResult> {
        let result = self.find(terminal, request, true);
        if result.is_err() {
            // Approved D02 recovery: an invalid query must not be retried by
            // live output or publish results from an earlier valid request.
            self.clear();
        }
        result
    }

    pub(crate) fn clear(&mut self) {
        self.cached_query = None;
        self.matches.clear();
        self.deadline = None;
    }

    pub(crate) fn buffer_changed(&mut self) {
        self.deadline = self
            .cached_query
            .as_ref()
            .filter(|query| !query.is_empty())
            .map(|_| Instant::now() + REFRESH_DELAY);
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn refresh_due(&mut self, terminal: &mut Terminal) -> Option<Result<SearchResult>> {
        if Instant::now() < self.deadline? {
            return None;
        }
        self.deadline = None;
        let query = self.cached_query.take()?;
        Some(self.find(
            terminal,
            SearchRequest {
                sequence: self.sequence,
                query,
                options: self.last_options,
                previous: true,
            },
            false,
        ))
    }

    fn find(
        &mut self,
        terminal: &mut Terminal,
        request: SearchRequest,
        scroll: bool,
    ) -> Result<SearchResult> {
        let SearchRequest {
            sequence,
            query,
            options,
            previous,
        } = request;
        // The source stores the new options before checking whether to rebuild
        // decorations. Toggling options for the same query keeps cached highlights.
        self.last_options = options;
        let active = if query.is_empty() {
            self.clear();
            None
        } else {
            let selection = terminal
                .selection_range()?
                .filter(|range| range.anchor != range.end);
            let mut engine = SearchEngine::new(terminal)?;
            let cached = self.cached_query.as_deref();
            if cached != Some(query.as_str()) {
                self.matches.clear();
                self.matches = engine.all_matches(&query, options)?;
            }
            if previous {
                engine.find_previous(&query, options, selection, cached)?
            } else {
                engine.find_next(&query, options, selection, cached)?
            }
        };

        if let Some(found) = &active {
            terminal.selection_action(SelectionAction::Select {
                row: found.row,
                col: found.col,
                length: found.size,
            })?;
            if scroll {
                center_if_outside_viewport(terminal, found.row)?;
            }
        } else {
            terminal.selection_action(SelectionAction::Clear)?;
        }

        self.cached_query = Some(query);
        self.sequence = sequence;
        let result_index = active
            .as_ref()
            .filter(|found| found.size > 0)
            .and_then(|found| {
                self.matches.iter().position(|entry| {
                    (entry.row, entry.col, entry.size) == (found.row, found.col, found.size)
                })
            })
            .map_or(-1, |index| index as isize);
        Ok(SearchResult {
            sequence,
            matches: self.matches.clone(),
            active,
            result_index,
        })
    }
}

fn center_if_outside_viewport(terminal: &mut Terminal, row: usize) -> Result<()> {
    let screen = terminal.screen()?;
    let offset = usize::try_from(screen.scrollbar.offset)?;
    if row < offset || row >= offset + screen.rows {
        let target = row.saturating_sub(screen.rows / 2);
        // The terminal's native scroll uses positive lines toward older rows.
        let delta = i64::try_from(offset)? - i64::try_from(target)?;
        terminal.scroll(i32::try_from(delta)?)?;
    }
    Ok(())
}
