use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, ClickEvent, FocusHandle, HighlightStyle, InteractiveElement, ListAlignment,
    ListState, Pixels, Rgba, SharedString, StatefulInteractiveElement, Styled, StyledText,
    WeakEntity, Window, div, prelude::*, px,
};
use matou_native_core::{pty::LaunchProfile, sessions::InterfaceLocale};
use matou_native_runtime::{HistoryText, HistoryTextSource, history_text};
use matou_native_terminal::{Color, Screen, Theme};
use serde_json::Value;

use super::TerminalSurface;

// The terminal probe includes this module without the workspace.
#[path = "local_time.rs"]
pub(crate) mod local_time;
use local_time::local_date_time;
#[path = "session_history_markdown.rs"]
mod markdown;
#[path = "session_history_markdown_render.rs"]
mod markdown_render;

pub(super) const WHEEL_SETTLE: Duration = Duration::from_millis(120);
pub(super) const FALSE_START: Duration = Duration::from_millis(200);
pub(super) const READ_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const PAGE_SIZE: i64 = 200;
const COLLAPSED_CHARS: usize = 4_000;
const EXPANDED_CHARS: usize = 20_000;
const COMPACT_BOUNDARY: &str = "compact_boundary";

pub(crate) fn is_compaction(value: &Value) -> bool {
    value["kind"] == "system" && value["text"] == COMPACT_BOUNDARY
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub profile: LaunchProfile,
    pub cwd: String,
    pub provider_session_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Latest,
    Earlier(i64),
    Updates(i64),
}

impl Page {
    pub(crate) fn start(self) -> Option<i64> {
        match self {
            Self::Latest => None,
            Self::Earlier(first) => Some((first - PAGE_SIZE).max(1)),
            Self::Updates(start) => Some(start),
        }
    }

    pub(crate) fn limit(self) -> i64 {
        match self {
            Self::Earlier(first) => PAGE_SIZE.min(first - 1),
            _ => PAGE_SIZE,
        }
    }

    pub(super) fn matches(self, result: &Value) -> bool {
        if result["error"].is_string() {
            return true;
        }
        let page = &result["page"];
        let Some(total) = page["total"].as_i64() else {
            return false;
        };
        let expected = match self {
            Self::Latest => (total - PAGE_SIZE + 1).max(1),
            _ => self.start().unwrap_or(1).min(total),
        };
        page["startEventIndex"].as_i64() == Some(if total == 0 { 0 } else { expected })
    }
}

#[cfg(debug_assertions)]
pub(super) fn tracing() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MATOU_HISTORY_SEAM_TRACE").is_some())
}

#[cfg(debug_assertions)]
pub(super) fn trace(phase: &str, since: Instant) {
    if tracing() {
        eprintln!(
            "[history-seam] {phase} ms={:.3}",
            since.elapsed().as_secs_f64() * 1000.
        );
    }
}

#[derive(Default)]
pub(super) struct WheelProbe {
    serial: u64,
    waiting: Option<(u64, u64)>,
    previous_quiet: Option<u64>,
}

impl WheelProbe {
    pub fn reset(&mut self) {
        self.waiting = None;
        self.previous_quiet = None;
    }

    pub fn start(&mut self, fingerprint: u64) -> Option<u64> {
        if self.waiting.is_some() {
            return None;
        }
        self.serial += 1;
        self.waiting = Some((self.serial, fingerprint));
        Some(self.serial)
    }

    pub fn observe(&mut self, fingerprint: u64) {
        if self
            .waiting
            .is_some_and(|(_, before)| before != fingerprint)
            || self
                .previous_quiet
                .is_some_and(|before| before != fingerprint)
        {
            self.reset();
        }
    }

    pub fn settled(&mut self, serial: u64, fingerprint: u64) -> bool {
        if self.waiting != Some((serial, fingerprint)) {
            return false;
        }
        self.waiting = None;
        self.previous_quiet.replace(fingerprint) == Some(fingerprint)
    }

    pub fn needs_observation(&self) -> bool {
        self.waiting.is_some() || self.previous_quiet.is_some()
    }
}

fn screen_lines(screen: &Screen, limit: usize) -> impl Iterator<Item = String> + '_ {
    screen
        .cells
        .chunks(screen.cols.max(1))
        .take(limit)
        .map(|cells| {
            let mut line = String::new();
            for cell in cells.iter().filter(|cell| cell.width != 0) {
                if cell.invisible || cell.text.is_empty() {
                    line.push(' ');
                } else {
                    line.push_str(&cell.text);
                }
            }
            line.trim_end().to_owned()
        })
}

fn user_prefix(profile: LaunchProfile) -> &'static str {
    match profile {
        LaunchProfile::Codex => "›",
        _ => "❯",
    }
}

#[derive(Clone, Copy)]
pub(super) struct UserRowPaint {
    prefix: &'static str,
    foreground: u32,
    prefix_color: u32,
    background: u32,
    bold_prefix: bool,
    left_columns: usize,
    prefix_columns: usize,
    right_columns: usize,
    top_padding: bool,
    bottom_padding: bool,
}

impl UserRowPaint {
    pub fn from_screen(profile: LaunchProfile, screen: &Screen) -> Option<Self> {
        let prefix = user_prefix(profile);
        let cols = screen.cols.max(1);
        let body_rows = screen.rows.saturating_sub(input_rows(profile, screen));
        let row_cells = |row: usize| screen.cells.get(row * cols..(row + 1) * cols);
        let has_text = |cell: &matou_native_terminal::Cell| {
            cell.width != 0 && !cell.invisible && !cell.text.trim().is_empty()
        };
        for (row, cells) in screen.cells.chunks(cols).take(body_rows).enumerate() {
            let Some(left) = cells.iter().take(3).position(has_text) else {
                continue;
            };
            let marker = &cells[left];
            let (prefix_color, background) = super::cell_colors(marker, screen);
            if marker.text != prefix || background == screen.background {
                continue;
            }
            let same_band = |cell: &matou_native_terminal::Cell| {
                super::cell_colors(cell, screen).1 == background
            };
            let width = cells[left..]
                .iter()
                .take_while(|cell| same_band(cell))
                .count();
            if width < cols / 2 {
                continue;
            }
            let Some(text) = cells[left + 1..left + width].iter().position(has_text) else {
                continue;
            };
            let prefix_columns = text + 1;
            let (foreground, _) = super::cell_colors(&cells[left + prefix_columns], screen);
            let padded = |row| {
                row_cells(row).is_some_and(|cells| {
                    cells[left..left + width]
                        .iter()
                        .all(|cell| same_band(cell) && !has_text(cell))
                })
            };
            return Some(Self {
                prefix,
                foreground,
                prefix_color: if marker.dim {
                    super::mix(prefix_color, background)
                } else {
                    prefix_color
                },
                background,
                bold_prefix: marker.bold,
                left_columns: left,
                prefix_columns,
                right_columns: cols - left - width,
                top_padding: row.checked_sub(1).is_some_and(padded),
                bottom_padding: row + 1 < body_rows && padded(row + 1),
            });
        }
        None
    }

    pub fn fallback(profile: LaunchProfile, theme: Theme, screen: Option<&Screen>) -> Self {
        let (foreground, background) = screen
            .map_or((theme.foreground, theme.background), |screen| {
                (screen.foreground, screen.background)
            });
        let codex = profile == LaunchProfile::Codex;
        let (foreground, background) = if codex {
            let gray = if background & 0xff > 127 { 255 } else { 236 };
            let shade = screen.map_or_else(|| theme.palette()[gray], |screen| screen.palette[gray]);
            (foreground, shade)
        } else {
            (background, foreground)
        };
        Self {
            prefix: user_prefix(profile),
            foreground,
            prefix_color: foreground,
            background,
            bold_prefix: codex,
            left_columns: 0,
            prefix_columns: 2,
            right_columns: 0,
            top_padding: codex,
            bottom_padding: codex,
        }
    }
}

#[derive(Clone)]
pub(super) struct JumpHint {
    pub row: usize,
    pub columns: Range<usize>,
}

pub(super) fn jump_hint(profile: LaunchProfile, screen: &Screen) -> Option<JumpHint> {
    if !screen.alternate || !screen.cursor_has_position {
        return None;
    }
    let label = match profile {
        LaunchProfile::ClaudeCode => "Jump to bottom (click) ↓",
        LaunchProfile::Codex => "↓ Back to bottom · esc",
        _ => return None,
    };
    let width = label.chars().count();
    let cols = screen.cols.max(1);
    let caret_index = screen.cursor_row * cols + screen.cursor_col;
    let composer = screen
        .cells
        .get(caret_index)
        .and_then(|caret| {
            let mut background = super::cell_colors(caret, screen).1;
            if background == screen.background && screen.cursor_col > 0 {
                // A cursor directly after typed text may land in an unpainted cell.
                let previous = &screen.cells[caret_index - 1];
                if previous.has_content {
                    background = super::cell_colors(previous, screen).1;
                }
            }
            screen
                .cells
                .chunks(cols)
                .take(screen.cursor_row + 1)
                .rposition(|cells| {
                    cells
                        .iter()
                        .find(|cell| !cell.invisible && !cell.text.trim().is_empty())
                        .is_some_and(|cell| {
                            cell.text == user_prefix(profile)
                                && super::cell_colors(cell, screen).1 == background
                        })
                })
        })
        .unwrap_or(screen.cursor_row);
    let first_row = composer.saturating_sub(input_rows(profile, screen) + 2);
    let rows = screen
        .cells
        .chunks(cols)
        .take(composer)
        .enumerate()
        .skip(first_row);
    for (row, cells) in rows.rev() {
        for (start, candidate) in cells.windows(width).enumerate() {
            if !candidate.iter().zip(label.chars()).all(|(cell, ch)| {
                cell.width == 1
                    && !cell.invisible
                    && (cell.text.starts_with(ch) || (ch == ' ' && cell.text.is_empty()))
            }) {
                continue;
            }
            let background = super::cell_colors(&candidate[0], screen).1;
            if background == screen.background
                || candidate
                    .iter()
                    .any(|cell| super::cell_colors(cell, screen).1 != background)
            {
                continue;
            }
            // The Codex badge occupies its own centered row above the composer.
            if profile == LaunchProfile::Codex
                && ((start * 2 + width).abs_diff(cells.len()) > 4
                    || cells[..start]
                        .iter()
                        .chain(&cells[start + width..])
                        .any(|cell| !cell.invisible && !cell.text.trim().is_empty()))
            {
                continue;
            }
            let padding = |cell: &matou_native_terminal::Cell| {
                cell.text == " " && super::cell_colors(cell, screen).1 == background
            };
            let end = start + width;
            return Some(JumpHint {
                row,
                columns: start - usize::from(start > 0 && padding(&cells[start - 1]))
                    ..end + usize::from(cells.get(end).is_some_and(padding)),
            });
        }
    }
    None
}

pub(super) fn input_rows(profile: LaunchProfile, screen: &Screen) -> usize {
    let Some(profile_config) = profile.history_seam() else {
        return 0;
    };
    let floor = profile_config.input_rows;
    if !screen.cursor_has_position || screen.cursor_row >= screen.rows {
        return (floor + 2).min(screen.rows);
    }
    let caret = screen.cursor_row;
    let prompt = match profile {
        LaunchProfile::ClaudeCode => "❯",
        LaunchProfile::Codex => "›",
        _ => "",
    };
    let prompt_row = screen
        .cells
        .chunks(screen.cols.max(1))
        .take(caret + 1)
        .enumerate()
        .rev()
        .find_map(|(row, cells)| {
            cells
                .iter()
                .find(|cell| cell.width != 0 && !cell.invisible && !cell.text.trim().is_empty())
                .filter(|cell| cell.text == prompt && cell.bg == Color::Default)
                .map(|_| row)
        });
    let first_input_row = prompt_row.map_or_else(
        || caret.saturating_sub(floor.saturating_sub(1)),
        |row| row.saturating_sub(1),
    );
    screen
        .rows
        .saturating_sub(first_input_row)
        .max(floor)
        .min(screen.rows)
}

pub(super) fn fingerprint(screen: &Screen, input_rows: usize) -> u64 {
    let mut hash = DefaultHasher::new();
    screen.cols.hash(&mut hash);
    screen.rows.hash(&mut hash);
    for line in screen_lines(screen, screen.rows.saturating_sub(input_rows)) {
        normalized(&line).hash(&mut hash);
    }
    hash.finish()
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|ch| !ignored_anchor_char(*ch))
        .collect()
}

fn ignored_anchor_char(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '❯' | '⏺' | '●' | '›' | '•')
}

#[derive(Clone, Debug)]
struct Message {
    index: i64,
    kind: MessageKind,
    timestamp: Option<u64>,
    text: SharedString,
    longer: bool,
    tool_preview: Option<SharedString>,
    command_output: Option<SharedString>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MessageKind {
    User,
    Assistant,
    Tool,
    Result,
    System,
    Command,
    CommandOutput,
    CompactSummary,
}

impl Message {
    fn parse(value: &Value) -> Option<Self> {
        if is_compaction(value) {
            return None;
        }
        let tool_name = value["toolName"].as_str();
        let mut kind = match value["kind"].as_str()? {
            "user" => MessageKind::User,
            "assistant" => MessageKind::Assistant,
            "tool" if tool_name == Some("tool_result") => MessageKind::Result,
            "tool" => MessageKind::Tool,
            _ => MessageKind::System,
        };
        let is_summary = value["isCompactSummary"] == true;
        let source = match (kind, is_summary) {
            (MessageKind::Assistant, false) => HistoryTextSource::Assistant,
            (MessageKind::User, false) => HistoryTextSource::User,
            _ => HistoryTextSource::Envelope,
        };
        let cleaned = history_text(value["text"].as_str()?, source);
        let (text, command_output) = match &cleaned {
            HistoryText::Plain(text) => (text.as_ref(), None),
            HistoryText::Command { line, output } => {
                kind = MessageKind::Command;
                (
                    line.as_str(),
                    (!output.is_empty()).then_some(output.as_str()),
                )
            }
            HistoryText::CommandOutput(output) => {
                kind = MessageKind::CommandOutput;
                (output.as_str(), None)
            }
        };
        if text.trim().is_empty() {
            return None;
        }
        if is_summary {
            kind = MessageKind::CompactSummary;
        }
        let (cap, longer) = truncate(text, EXPANDED_CHARS);
        Some(Self {
            index: value["index"].as_i64()?,
            kind,
            timestamp: timestamp(value),
            text: cap.to_owned().into(),
            longer,
            command_output: command_output
                .map(|output| truncate(output, EXPANDED_CHARS + 1).0.to_owned().into()),
            tool_preview: match kind {
                MessageKind::Tool => tool_name
                    .filter(|name| !name.trim().is_empty())
                    .map(|name| tool_summary(name, text).into()),
                MessageKind::Result => result_preview(cap).map(Into::into),
                _ => None,
            },
        })
    }

    fn body(&self, expanded: bool) -> Option<(&str, bool)> {
        if expanded {
            Some((self.text.as_ref(), self.longer))
        } else if self.kind == MessageKind::CompactSummary {
            None
        } else {
            Some(truncate(&self.text, COLLAPSED_CHARS))
        }
    }

    fn cache_markdown(&self, cache: &mut markdown::Cache, expanded: bool) {
        if matches!(
            self.kind,
            MessageKind::User | MessageKind::Assistant | MessageKind::CompactSummary
        ) && cache.get(self.index, expanded).is_none()
            && let Some((text, clipped)) = self.body(expanded)
        {
            cache.insert(self.index, expanded, text, clipped);
        }
    }
}

fn timestamp(value: &Value) -> Option<u64> {
    value["timestamp"]
        .as_f64()
        .filter(|time| time.is_finite() && *time >= 0.)
        .map(|time| time.round() as u64)
}

fn truncate(text: &str, limit: usize) -> (&str, bool) {
    text.char_indices()
        .nth(limit)
        .map_or((text, false), |(byte, _)| (&text[..byte], true))
}

fn tool_summary(name: &str, text: &str) -> String {
    let name = name.trim();
    let named_parameters = text.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(name)
            .filter(|suffix| suffix.starts_with(char::is_whitespace))
            .map(str::trim)
    });
    let parameters = named_parameters.unwrap_or_else(|| text.trim());
    if name == "Bash" && named_parameters.is_none() && !parameters.starts_with('{') {
        let mut lines = parameters.lines().filter(|line| !line.trim().is_empty());
        if let Some(command) = lines.next().filter(|command| *command != name)
            && let Some(result) = lines.next()
        {
            return format!(
                "{name} {} · {}",
                bounded_summary(command),
                bounded_summary(result)
            );
        }
    }
    let field = match name {
        "Read" | "Edit" | "Write" => Some("file_path"),
        "Bash" => Some("command"),
        _ => None,
    };
    let readable = if let Some(field) = field {
        serde_json::from_str::<Value>(parameters)
            .ok()
            .and_then(|input| input[field].as_str().map(bounded_summary))
    } else {
        other_tool_argument(parameters).map(|value| bounded_summary(&value))
    };
    let argument = readable
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| bounded_summary(parameters));
    if argument.is_empty() || argument == name {
        return name.to_owned();
    }
    format!("{name} {argument}")
}

fn other_tool_argument(parameters: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(truncate(parameters, EXPANDED_CHARS).0).ok()?;
    let fields = parsed.as_object()?;
    let scalar = |(key, value): (&str, &Value)| {
        let value = match value {
            Value::String(value) if !value.trim().is_empty() => value.to_owned(),
            Value::Number(_) | Value::Bool(_) => value.to_string(),
            _ => return None,
        };
        Some(format!("{key}: {value}"))
    };
    ["query", "pattern", "path", "file_path", "url", "command"]
        .into_iter()
        .filter_map(|key| scalar((key, fields.get(key)?)))
        .next()
        .or_else(|| fields.iter().find_map(|(key, value)| scalar((key, value))))
}

fn result_preview(text: &str) -> Option<String> {
    let text = text.trim();
    let parsed;
    let visible = if text.starts_with(['[', '{']) {
        parsed = serde_json::from_str::<Value>(text).ok()?;
        if let Some(blocks) = parsed.as_array() {
            blocks.iter().find_map(|block| block["text"].as_str())?
        } else {
            ["text", "message", "output"]
                .into_iter()
                .find_map(|key| parsed[key].as_str())?
        }
    } else {
        text
    };
    visible
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(bounded_summary)
}

fn bounded_summary(text: &str) -> String {
    let (text, clipped) = truncate(text.trim(), 120);
    let mut text = text.replace(['\r', '\n', '\t'], " ");
    if clipped {
        text.push('…');
    }
    text
}

#[derive(Clone, Debug)]
enum Row {
    Message(Message),
    Tool {
        call: Option<Message>,
        result: Option<Message>,
    },
}

impl Row {
    fn index(&self) -> i64 {
        match self {
            Self::Message(message) => message.index,
            Self::Tool { call, result } => call.as_ref().or(result.as_ref()).unwrap().index,
        }
    }

    fn timestamp(&self) -> Option<u64> {
        match self {
            Self::Message(message) => message.timestamp,
            Self::Tool { call, result } => call
                .as_ref()
                .or(result.as_ref())
                .and_then(|message| message.timestamp),
        }
    }
}

fn group(messages: impl IntoIterator<Item = Message>) -> Vec<Row> {
    let mut rows = Vec::new();
    for message in messages {
        match message.kind {
            MessageKind::Tool => rows.push(Row::Tool {
                call: Some(message),
                result: None,
            }),
            MessageKind::Result => match rows.last_mut() {
                Some(Row::Tool {
                    call: Some(_),
                    result: slot @ None,
                }) => *slot = Some(message),
                _ => rows.push(Row::Tool {
                    call: None,
                    result: Some(message),
                }),
            },
            _ => rows.push(Row::Message(message)),
        }
    }
    rows
}

fn cache_page_markdown(cache: &mut markdown::Cache, rows: &[Row], _page: Page) {
    #[cfg(debug_assertions)]
    let measurement = tracing().then(|| (Instant::now(), cache.parses()));
    for row in rows {
        if let Row::Message(message) = row {
            message.cache_markdown(cache, false);
        }
    }
    #[cfg(debug_assertions)]
    if let Some((started, before)) = measurement {
        eprintln!(
            "[history-seam] markdown-page page={_page:?} parsed={} ms={:.3}",
            cache.parses() - before,
            started.elapsed().as_secs_f64() * 1000.
        );
    }
}

/// Match against the same bounded text range that the history view can show.
fn anchor(screen: &Screen, input_rows: usize, messages: &[Message]) -> Option<(i64, usize)> {
    let visible: Vec<_> = screen_lines(screen, screen.rows.saturating_sub(input_rows)).collect();
    // At the top of Claude's conversation, its startup decoration may occupy
    // several viewport rows before the first transcript line.
    let first_message = visible
        .iter()
        .position(|line| line.trim_start().starts_with(['❯', '⏺', '●', '›', '•']))
        .unwrap_or(0);
    let lines: Vec<_> = visible[first_message..]
        .iter()
        .map(|line| normalized(line))
        .filter(|line| line.chars().count() >= 6)
        .take(5)
        .collect();
    let joined = lines.concat();
    let needles: Vec<_> = std::iter::once(&joined)
        .chain(lines.iter())
        .filter(|line| !line.is_empty())
        .collect();
    if needles.is_empty() {
        return None;
    }
    let mut found: Option<(usize, i64, &str, usize)> = None;
    for message in messages
        .iter()
        .rev()
        .filter(|message| message.kind != MessageKind::CompactSummary)
    {
        let text = message.text.as_ref();
        let searchable = normalized(text);
        for (priority, needle) in needles.iter().enumerate() {
            if found.is_some_and(|(previous, ..)| previous <= priority) {
                break;
            }
            if let Some(offset) = searchable.find(needle.as_str()) {
                found = Some((priority, message.index, text, offset));
                break;
            }
        }
        if found.is_some_and(|(priority, ..)| priority == 0) {
            break;
        }
    }
    let (_, index, text, mut remaining) = found?;
    let byte = text.char_indices().find_map(|(byte, ch)| {
        if ignored_anchor_char(ch) {
            return None;
        }
        if remaining == 0 {
            return Some(byte);
        }
        remaining -= ch.len_utf8();
        None
    });
    Some((index, byte.unwrap_or(0)))
}

pub(super) struct HistoryView {
    pub list: ListState,
    pub focus: FocusHandle,
    pub opened_at: Instant,
    pub pending: Option<(Page, u64)>,
    pub request_id: Option<u64>,
    pub retry: Option<Page>,
    pub ready: bool,
    pub closing: bool,
    pub latest_index: i64,
    pub new_messages: usize,
    #[cfg(debug_assertions)]
    pub input_sent_at: Option<Instant>,
    #[cfg(debug_assertions)]
    pub request_sent_at: Instant,
    has_earlier: bool,
    first_index: i64,
    start_time: Option<u64>,
    compact_time: Option<u64>,
    rows: Vec<Row>,
    markdown: markdown::Cache,
    expanded: HashSet<i64>,
    full_text: HashSet<i64>,
    snapshot: Option<Rc<Screen>>,
}

impl HistoryView {
    pub fn new(screen: Option<&Screen>, focus: FocusHandle) -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Bottom, px(720.)),
            focus,
            opened_at: Instant::now(),
            pending: None,
            request_id: None,
            retry: None,
            ready: false,
            closing: false,
            latest_index: 0,
            new_messages: 0,
            #[cfg(debug_assertions)]
            input_sent_at: None,
            #[cfg(debug_assertions)]
            request_sent_at: Instant::now(),
            has_earlier: false,
            first_index: 0,
            start_time: None,
            compact_time: None,
            rows: Vec::new(),
            markdown: markdown::Cache::default(),
            expanded: HashSet::new(),
            full_text: HashSet::new(),
            snapshot: screen.cloned().map(Rc::new),
        }
    }

    pub fn live(&self) -> bool {
        self.snapshot.is_some()
    }

    pub fn visible(&self) -> bool {
        self.ready || self.retry == Some(Page::Latest)
    }

    pub fn at_bottom(&self) -> bool {
        self.list.logical_scroll_top().item_ix >= self.list.item_count()
    }

    pub fn earlier_page(&self) -> Option<Page> {
        (self.ready && self.has_earlier && self.pending.is_none() && self.retry.is_none())
            .then_some(Page::Earlier(self.first_index))
    }

    pub fn fail(&mut self, page: Page) {
        self.pending = None;
        self.request_id = None;
        if matches!(page, Page::Updates(_)) {
            return;
        }
        self.retry = Some(page);
        if self.ready {
            self.list.splice(0..1, 1);
        }
    }

    pub fn receive(&mut self, request: Page, result: &Value, profile: LaunchProfile) -> bool {
        let Some(events) = result["events"].as_array() else {
            self.fail(request);
            return false;
        };
        let page = &result["page"];
        let first = page["startEventIndex"].as_i64().unwrap_or_default();
        let last = page["endEventIndex"].as_i64().unwrap_or_default()
            - i64::from(profile == LaunchProfile::ClaudeCode && first > 0);
        if first == 0 && request == Page::Latest {
            self.fail(request);
            return false;
        }
        self.pending = None;
        self.request_id = None;
        if first == 1 && !matches!(request, Page::Updates(_)) {
            self.start_time = events
                .iter()
                .filter(|event| !is_compaction(event))
                .find_map(timestamp);
        }
        match request {
            Page::Latest => {
                self.retry = None;
                self.latest_index = last;
                if profile == LaunchProfile::ClaudeCode {
                    self.compact_time = events
                        .iter()
                        .find(|event| is_compaction(event))
                        .and_then(timestamp);
                }
                let messages: Vec<_> = events.iter().filter_map(Message::parse).collect();
                let matched = self
                    .snapshot
                    .as_ref()
                    .and_then(|screen| anchor(screen, input_rows(profile, screen), &messages));
                let messages = messages.into_iter().filter_map(|mut message| {
                    if let Some((index, offset)) = matched {
                        if message.index > index || (message.index == index && offset == 0) {
                            return None;
                        }
                        if message.index == index {
                            let prefix = message.text[..offset].trim_end();
                            if prefix.is_empty() {
                                return None;
                            }
                            message.text = prefix.to_owned().into();
                            message.longer = false;
                            message.command_output = None;
                        }
                    }
                    Some(message)
                });
                self.rows = group(messages);
                self.markdown = markdown::Cache::default();
                cache_page_markdown(&mut self.markdown, &self.rows, request);
                self.first_index = first;
                self.has_earlier = page["hasEarlier"] == true;
                self.ready = true;
                self.opened_at = Instant::now();
                let seam_index = self.rows.len() + 1;
                self.list.reset(
                    seam_index
                        + 1
                        + usize::from(self.compact_time.is_some())
                        + usize::from(self.snapshot.is_some()),
                );
                if self.snapshot.is_some() {
                    // The same terminal rows sit directly below the divider. The
                    // additional upward wheel movement reveals the divider first.
                    self.list.scroll_to(gpui::ListOffset {
                        item_ix: seam_index,
                        offset_in_item: px(0.),
                    });
                }
                true
            }
            Page::Earlier(_) => {
                self.retry = None;
                let mut earlier = group(
                    events
                        .iter()
                        .filter_map(Message::parse)
                        .filter(|event| event.index < self.first_index),
                );
                let mut replaced = 0;
                if let (
                    Some(Row::Tool {
                        call: Some(_),
                        result: slot @ None,
                    }),
                    Some(Row::Tool {
                        call: None,
                        result: Some(result),
                    }),
                ) = (earlier.last_mut(), self.rows.first())
                {
                    *slot = Some(result.clone());
                    self.rows.remove(0);
                    replaced = 1;
                }
                let added = earlier.len();
                cache_page_markdown(&mut self.markdown, &earlier, request);
                let offset = self.list.logical_scroll_top();
                earlier.append(&mut self.rows);
                self.rows = earlier;
                self.first_index = first;
                self.has_earlier = page["hasEarlier"] == true;
                self.list.splice(1..1 + replaced, added);
                if replaced == 1 && offset.item_ix == 1 {
                    self.list.scroll_to(gpui::ListOffset {
                        item_ix: added,
                        offset_in_item: offset.offset_in_item,
                    });
                }
                self.list.splice(0..1, 1);
                false
            }
            Page::Updates(_) => {
                self.new_messages += events
                    .iter()
                    .filter(|event| {
                        event["index"]
                            .as_i64()
                            .is_some_and(|index| index > self.latest_index)
                            && matches!(event["kind"].as_str(), Some("user" | "assistant"))
                            && Message::parse(event).is_some_and(|message| {
                                matches!(message.kind, MessageKind::User | MessageKind::Assistant)
                            })
                    })
                    .count();
                self.latest_index = self.latest_index.max(last);
                false
            }
        }
    }

    pub fn near_top(&self, line_height: Pixels) -> bool {
        let view = self.list.viewport_bounds();
        let offset = self.list.logical_scroll_top();
        let minimum = line_height * offset.item_ix.saturating_sub(1) as f32 + offset.offset_in_item;
        minimum <= view.size.height * 2.
            && self.list.scroll_px_offset_for_scrollbar().y.abs() <= view.size.height * 2.
    }

    pub fn remeasure_rows(&self) {
        let count = self.list.item_count();
        let offset = self.list.logical_scroll_top();
        self.list.splice(0..count, count);
        self.list.scroll_to(offset);
    }

    pub fn expand_tool(&mut self, index: i64) {
        if !self.expanded.remove(&index) {
            self.expanded.insert(index);
        }
        self.remeasure(index);
    }

    pub fn expand_text(&mut self, index: i64) {
        if !self.full_text.remove(&index) {
            self.full_text.insert(index);
            if let Some(Row::Message(message)) = self.rows.iter().find(|row| row.index() == index)
            {
                #[cfg(debug_assertions)]
                let measurement = tracing().then(|| (Instant::now(), self.markdown.parses()));
                message.cache_markdown(&mut self.markdown, true);
                #[cfg(debug_assertions)]
                if let Some((started, before)) = measurement
                    && self.markdown.parses() > before
                {
                    trace("markdown-expand parsed=1", started);
                }
            }
        }
        self.remeasure(index);
    }

    fn remeasure(&self, index: i64) {
        if let Some(position) = self.rows.iter().position(|row| {
            row.index() == index
                || matches!(row, Row::Tool { result: Some(result), .. } if result.index == index)
        }) {
            let offset = self.list.logical_scroll_top();
            self.list.splice(position + 1..position + 2, 1);
            self.list.scroll_to(offset);
        }
    }

    pub fn snapshot_at(&self, index: usize) -> Option<Rc<Screen>> {
        (index == self.rows.len() + 2 + usize::from(self.compact_time.is_some()))
            .then(|| self.snapshot.clone())
            .flatten()
    }

    pub fn item(
        &self,
        index: usize,
        style: HistoryStyle,
        surface: &WeakEntity<TerminalSurface>,
    ) -> AnyElement {
        let HistoryStyle { locale, dim, .. } = style;
        if index == 0 {
            if let Some(Page::Earlier(_)) = self.retry {
                return error_row(locale, surface).into_any_element();
            }
            if self.has_earlier {
                return div().into_any_element();
            }
            let beginning = locale.text("已到会话开头", "Start of conversation");
            let label = self
                .start_time
                .or_else(|| self.rows.first().and_then(Row::timestamp))
                .map_or_else(
                    || beginning.to_owned(),
                    |time| format!("{beginning} · {}", start_date(time, locale)),
                );
            return div()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .gap(style.spacing(8.))
                .pt(style.spacing(4.))
                .pb(style.spacing(18.))
                .text_size(style.label_font_size)
                .text_color(dim)
                .child(div().size(px(6.)).rounded_full().bg(style.start_dot))
                .child(div().min_w_0().text_ellipsis().child(label))
                .into_any_element();
        }
        if let Some(row) = self.rows.get(index - 1) {
            return match row {
                Row::Message(message) => self.message(message, style, true, surface),
                Row::Tool { call, result } => {
                    self.tool(row.index(), call.as_ref(), result.as_ref(), style, surface)
                }
            };
        }
        if index == self.rows.len() + 1 {
            return div()
                .w_full()
                .flex()
                .items_center()
                .gap(style.spacing(10.))
                .pt(style.spacing(18.))
                .pb(style.spacing(16.))
                .text_size(style.label_font_size)
                .text_color(dim)
                .child(div().h(px(1.)).flex_1().bg(style.rule))
                .child(locale.text("更早的会话记录", "Earlier conversation"))
                .child(div().h(px(1.)).flex_1().bg(style.rule))
                .into_any_element();
        }
        if let Some(time) = self.compact_time {
            let (_, time) = local_date_time(time);
            return div()
                .pt(style.spacing(4.))
                .pb(style.spacing(14.))
                .text_color(style.amber)
                .text_size(style.label_font_size)
                .text_ellipsis()
                .child(format!(
                    "── {} · {time} ──",
                    locale.text("会话已压缩", "Conversation compacted")
                ))
                .into_any_element();
        }
        div().into_any_element()
    }

    fn message(
        &self,
        message: &Message,
        style: HistoryStyle,
        prefix_visible: bool,
        surface: &WeakEntity<TerminalSurface>,
    ) -> AnyElement {
        let HistoryStyle {
            locale,
            foreground,
            dim,
            ..
        } = style;
        let open = self.full_text.contains(&message.index);
        let summary = message.kind == MessageKind::CompactSummary;
        let markdown = self.markdown.get(message.index, open);
        let content = markdown.is_none().then(|| message.body(open)).flatten();
        let output = message.command_output.as_ref().map(|output| {
            truncate(
                output,
                if open {
                    EXPANDED_CHARS
                } else {
                    COLLAPSED_CHARS
                },
            )
        });
        let clipped = markdown.is_some_and(|body| body.clipped)
            || content.is_some_and(|(_, clipped)| clipped)
            || output.is_some_and(|(_, clipped)| clipped);
        let prefix = match (prefix_visible, message.kind) {
            (true, MessageKind::Assistant) => "⏺",
            (true, MessageKind::Command) => style.user_row.prefix,
            _ => "",
        };
        let user = (prefix_visible && message.kind == MessageKind::User).then_some(style.user_row);
        let body = div()
            .min_w_0()
            .flex_1()
            .overflow_x_hidden()
            .when(summary, |body| {
                body.child(
                    div()
                        .id(("history-summary", message.index as usize))
                        .cursor_pointer()
                        .on_click(click(surface, Action::Text(message.index)))
                        .text_ellipsis()
                        .child(match (locale, open) {
                            (InterfaceLocale::Chinese, false) => "会话已压缩的摘要（显示全部）",
                            (InterfaceLocale::Chinese, true) => "会话已压缩的摘要（收起）",
                            (InterfaceLocale::English, false) => "Compaction summary (Show all)",
                            (InterfaceLocale::English, true) => "Compaction summary (Collapse)",
                        }),
                )
            })
            .when_some(markdown, |body, markdown| {
                body.child(markdown_render::render(&markdown.blocks, message.index, style))
            })
            .when_some(content, |body, (text, clipped)| {
                body.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .child(if clipped {
                            format!("{text}…")
                        } else {
                            text.to_owned()
                        }),
                )
            })
            .when_some(output, |body, (text, clipped)| {
                body.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .text_color(dim)
                        .whitespace_normal()
                        .child(if clipped {
                            format!("{text}…")
                        } else {
                            text.to_owned()
                        }),
                )
            })
            .when(!summary && (clipped || open), |body| {
                body.child(
                    div()
                        .id(("history-text", message.index as usize))
                        .cursor_pointer()
                        .on_click(click(surface, Action::Text(message.index)))
                        .text_color(user.map_or(dim, |user| gpui::rgb(user.foreground)))
                        .child(if open {
                            locale.text("收起", "Collapse")
                        } else {
                            locale.text("显示全部", "Show all")
                        }),
                )
            });
        if let Some(user) = user {
            return div()
                .w_full()
                .when_some(style.grid_width, |div, width| div.max_w(width))
                .pb(style.spacing(10.))
                .pl(style.cell_width * user.left_columns as f32)
                .pr(style.cell_width * user.right_columns as f32)
                .child(
                    div()
                        .w_full()
                        .flex()
                        .bg(gpui::rgb(user.background))
                        .text_color(gpui::rgb(user.foreground))
                        .when(user.top_padding, |div| div.pt(style.line_height))
                        .when(user.bottom_padding, |div| div.pb(style.line_height))
                        .child(
                            div_prefix(user.prefix)
                                .w(style.cell_width * user.prefix_columns as f32)
                                .text_color(gpui::rgb(user.prefix_color))
                                .when(user.bold_prefix, |div| {
                                    div.font_weight(gpui::FontWeight::BOLD)
                                }),
                        )
                        .child(body),
                )
                .into_any_element();
        }
        let muted = matches!(
            message.kind,
            MessageKind::CommandOutput | MessageKind::CompactSummary
        );
        div()
            .w_full()
            .min_w_0()
            .when_some(style.grid_width, |div, width| div.max_w(width))
            .when(
                prefix_visible && message.kind != MessageKind::Command,
                |div| div.pb(style.spacing(10.)),
            )
            .when(message.kind == MessageKind::CommandOutput, |div| {
                div.px(style.cell_width * 2.)
            })
            .flex()
            .when(message.kind != MessageKind::Assistant, |div| div.gap(px(4.)))
            .text_color(if muted { dim } else { foreground })
            .when(!prefix.is_empty(), |div| {
                div.child(
                    div_prefix(prefix)
                        .text_color(style.accent)
                        .when(message.kind == MessageKind::Assistant, |marker| {
                            marker.w(style.cell_width * 2.)
                        }),
                )
            })
            .child(body)
            .into_any_element()
    }

    fn tool(
        &self,
        index: i64,
        call: Option<&Message>,
        result: Option<&Message>,
        style: HistoryStyle,
        surface: &WeakEntity<TerminalSurface>,
    ) -> AnyElement {
        let expanded = self.expanded.contains(&index);
        let call_summary = call
            .and_then(|message| {
                message
                    .tool_preview
                    .as_ref()
                    .map(|summary| summary.as_ref())
            })
            .unwrap_or_else(|| style.locale.text("工具结果", "Tool result"));
        let first_result = result.and_then(|message| message.tool_preview.as_ref());
        let summary = first_result.map_or_else(
            || call_summary.to_owned(),
            |result| format!("{call_summary} · {result}"),
        );
        let name_len = if call.is_some() {
            call_summary.find(' ').unwrap_or(call_summary.len())
        } else {
            call_summary.len()
        };
        let summary = StyledText::new(summary).with_highlights([(
            0..name_len,
            HighlightStyle {
                color: Some(style.foreground.into()),
                ..Default::default()
            },
        )]);
        div()
            .w_full()
            .min_w_0()
            .when_some(style.grid_width, |div, width| div.max_w(width))
            .pb(style.spacing(10.))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .relative()
                    .px(style.spacing(10.) + px(2.))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(2.))
                            .bg(style.tool_rule),
                    )
                    .child(
                        div()
                            .id(("history-tool", index as usize))
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(style.spacing(6.))
                            .text_color(style.dim)
                            .cursor_pointer()
                            .on_click(click(surface, Action::Tool(index)))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(style.label_font_size)
                                    .child(if expanded { "▾" } else { "▸" }),
                            )
                            .child(div().flex_1().min_w_0().text_ellipsis().child(summary)),
                    )
                    .when(expanded, |div| {
                        let body_style = HistoryStyle {
                            foreground: style.tool_foreground,
                            ..style
                        };
                        div.child(
                            gpui::div()
                                .w_full()
                                .mt(style.spacing(6.))
                                .mb(style.spacing(2.))
                                .min_w_0()
                                .overflow_x_hidden()
                                .px(style.spacing(10.))
                                .py(style.spacing(8.))
                                .rounded(px(6.))
                                .bg(style.tool_background)
                                .text_size(style.label_font_size)
                                .line_height(style.line_height * 0.92)
                                .children(call.into_iter().chain(result).map(|message| {
                                    self.message(message, body_style, false, surface)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
pub(super) struct HistoryStyle {
    pub locale: InterfaceLocale,
    pub foreground: Rgba,
    pub dim: Rgba,
    pub line_height: Pixels,
    pub cell_width: Pixels,
    pub grid_width: Option<Pixels>,
    pub user_row: UserRowPaint,
    pub accent: Rgba,
    pub amber: Rgba,
    pub rule: Rgba,
    pub start_dot: Rgba,
    pub tool_rule: Rgba,
    pub tool_background: Rgba,
    pub tool_foreground: Rgba,
    pub label_font_size: Pixels,
}

impl HistoryStyle {
    pub fn spacing(self, reference_px: f32) -> Pixels {
        self.line_height * (reference_px / 20.25)
    }
}

fn start_date(timestamp: u64, locale: InterfaceLocale) -> String {
    let (date, time) = local_date_time(timestamp);
    if locale == InterfaceLocale::Chinese
        && let Some((month, day)) = date.split_once('/')
    {
        return format!(
            "{} 月 {} 日 {time}",
            month.trim_start_matches('0'),
            day.trim_start_matches('0')
        );
    }
    format!("{date} {time}")
}

fn div_prefix(prefix: &'static str) -> gpui::Div {
    div().flex_shrink_0().child(prefix)
}

#[derive(Clone, Copy)]
pub(super) enum Action {
    Retry,
    Tool(i64),
    Text(i64),
}

fn click(
    surface: &WeakEntity<TerminalSurface>,
    action: Action,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let surface = surface.clone();
    move |_, _, cx| {
        let _ = surface.update(cx, |surface, cx| surface.history_action(action, cx));
        cx.stop_propagation();
    }
}

pub(super) fn error_row(
    locale: InterfaceLocale,
    surface: &WeakEntity<TerminalSurface>,
) -> gpui::Div {
    div()
        .flex()
        .gap(px(10.))
        .child(locale.text("读取不到更早的记录", "Couldn't read earlier history"))
        .child(
            div()
                .id("history-retry")
                .cursor_pointer()
                .on_click(click(surface, Action::Retry))
                .child(locale.text("重试", "Retry")),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markdown_page_merge_and_body_lookups_parse_each_variant_once() {
        let message = |index, text: &str| {
            Message::parse(&json!({"index": index, "kind": "assistant", "text": text})).unwrap()
        };
        let latest = message(
            200,
            &format!("\x1b[1m**bold**\x1b[0m\n\n{}", "灯".repeat(21_000)),
        );
        let earlier = message(199, "### Earlier");
        let mut cache = markdown::Cache::default();
        let rows = group([latest.clone()]);
        cache_page_markdown(&mut cache, &rows, Page::Latest);
        assert_eq!(cache.parses(), 1);
        for _ in 0..90 {
            let body = cache.get(200, false).unwrap();
            assert!(body.clipped);
            assert!(matches!(&body.blocks[0], markdown::Block::Paragraph(body)
                if body.text.as_ref() == "bold" && body.spans[0].bold));
        }
        assert_eq!(cache.parses(), 1);

        let earlier_rows = group([earlier, latest.clone()]);
        cache_page_markdown(&mut cache, &earlier_rows, Page::Earlier(200));
        cache_page_markdown(&mut cache, &earlier_rows, Page::Earlier(200));
        assert_eq!(cache.parses(), 2);

        for _ in 0..3 {
            latest.cache_markdown(&mut cache, true);
            assert!(cache.get(200, true).unwrap().clipped);
            assert!(cache.get(200, false).unwrap().clipped);
        }
        assert_eq!(cache.parses(), 3);
        let expanded = cache.get(200, true).unwrap();
        assert!(matches!(&expanded.blocks[1], markdown::Block::Literal(text)
            if text.chars().count() == EXPANDED_CHARS - "**bold**\n\n".len() + 1));
    }

    #[test]
    fn probe_needs_two_unchanged_forwarded_wheels_and_resets_on_content() {
        let mut probe = WheelProbe::default();
        let first = probe.start(5).unwrap();
        assert!(probe.start(5).is_none());
        assert!(!probe.settled(first, 5));
        let moved = probe.start(5).unwrap();
        probe.observe(7);
        assert!(!probe.settled(moved, 7));
        let first_again = probe.start(7).unwrap();
        assert!(!probe.settled(first_again, 7));
        let second = probe.start(7).unwrap();
        assert!(probe.settled(second, 7));
    }

    #[test]
    fn history_user_rows_reuse_cli_cells_and_terminal_columns() -> anyhow::Result<()> {
        use matou_native_terminal::{Terminal, TerminalOptions};

        let mut terminal = Terminal::new(TerminalOptions::new(50, 14, 8, 14))?;
        terminal.set_theme(Theme::LIGHT)?;
        terminal.feed(
            format!(
                "\x1b[?1049h\x1b[3;1H\x1b[7m❯ Claude message{}\x1b[0m\x1b[12;1H❯ \x1b[12;3H",
                " ".repeat(34)
            )
            .as_bytes(),
        );
        let screen = terminal.screen()?;
        let claude = UserRowPaint::from_screen(LaunchProfile::ClaudeCode, &screen).unwrap();
        assert_eq!(claude.prefix, "❯");
        assert_eq!(
            (claude.foreground, claude.background),
            (screen.background, screen.foreground)
        );
        assert_eq!(
            (
                claude.left_columns,
                claude.prefix_columns,
                claude.right_columns
            ),
            (0, 2, 0)
        );
        assert!(!claude.top_padding && !claude.bottom_padding);

        terminal.feed(
            "\x1b[2J\x1b[48;5;255m\x1b[3;1H\x1b[K\x1b[4;1H› Codex message\x1b[K\x1b[5;1H\x1b[K\x1b[0m\x1b[12;3H"
                .as_bytes(),
        );
        let screen = terminal.screen()?;
        let codex = UserRowPaint::from_screen(LaunchProfile::Codex, &screen).unwrap();
        assert_eq!(codex.prefix, "›");
        assert_eq!(
            (codex.foreground, codex.background),
            (screen.foreground, screen.palette[255])
        );
        assert_eq!(
            (
                codex.left_columns,
                codex.prefix_columns,
                codex.right_columns
            ),
            (0, 2, 0)
        );
        assert!(codex.top_padding && codex.bottom_padding);
        Ok(())
    }

    #[test]
    fn jump_hints_hide_the_styled_badge_without_the_composer_or_plain_text() -> anyhow::Result<()> {
        use matou_native_terminal::{Terminal, TerminalOptions};
        let mut terminal = Terminal::new(TerminalOptions::new(50, 14, 8, 14))?;
        terminal.set_theme(Theme::LIGHT)?;
        terminal.feed(
            "\x1b[?1049h\x1b[3;1HJump to bottom (click) ↓\x1b[9;1Hreply \x1b[48;5;236m Jump to bottom (click) ↓ \x1b[0m\x1b[12;3H"
                .as_bytes(),
        );
        let hint = jump_hint(LaunchProfile::ClaudeCode, &terminal.screen()?).unwrap();
        assert_eq!(hint.row, 8);
        assert_eq!(hint.columns, 6..32);
        terminal.feed(
            "\x1b[2J\x1b[9;14H\x1b[48;5;255m ↓ Back to bottom · esc \x1b[0m\x1b[12;3H".as_bytes(),
        );
        let hint = jump_hint(LaunchProfile::Codex, &terminal.screen()?).unwrap();
        assert_eq!(hint.row, 8);
        assert_eq!(hint.columns, 13..37);
        terminal.feed(b"\x1b[2J\x1b[9;14H");
        terminal.feed("↓ Back to bottom · esc\x1b[12;3H".as_bytes());
        assert!(jump_hint(LaunchProfile::Codex, &terminal.screen()?).is_none());
        terminal.feed(
            "\x1b[2J\x1b[48;5;255m\x1b[8;1H› Draft\x1b[9;14H ↓ Back to bottom · esc \x1b[10;3Hcontinuation"
                .as_bytes(),
        );
        assert!(jump_hint(LaunchProfile::Codex, &terminal.screen()?).is_none());
        Ok(())
    }

    #[test]
    fn live_history_keeps_the_provider_floor_caret_and_full_composer_clear() -> anyhow::Result<()> {
        use matou_native_terminal::{Terminal, TerminalOptions};

        let mut terminal = Terminal::new(TerminalOptions::new(85, 32, 8, 14))?;
        let profile = LaunchProfile::Codex;
        let floor = profile.history_seam().expect("Codex history").input_rows;
        let mut short_reserve = 0;
        for (name, cols, rows, font_pixels, prompt, caret, draft, status) in [
            ("short footer", 85, 32, 14, 30, 30, false, "Ready"),
            (
                "wrapped path",
                85,
                32,
                14,
                28,
                28,
                false,
                "Long workspace path\r\ncontinues here",
            ),
            (
                "narrow window",
                42,
                26,
                14,
                21,
                21,
                false,
                "Long path\r\ncontinues here",
            ),
            (
                "larger font",
                38,
                22,
                21,
                17,
                17,
                false,
                "Long path\r\ncontinues here",
            ),
            (
                "multiline draft",
                38,
                22,
                21,
                12,
                17,
                true,
                "Long path\r\ncontinues here",
            ),
        ] {
            terminal.resize(cols, rows, 8, font_pixels)?;
            terminal.feed(
                format!(
                    "\x1b[?1049h\x1b[2J\x1b[4;1H\x1b[48;2;68;68;68m› Prior turn\x1b[0m\x1b[{prompt};1H› first draft line\x1b[{caret};3H{}\x1b[{};1H{status}\x1b[{caret};3H",
                    if draft { "last draft line" } else { "" },
                    rows - u16::from(status.contains('\n')),
                )
                .as_bytes(),
            );
            let screen = terminal.screen()?;
            let reserved = input_rows(profile, &screen);
            let boundary = screen.rows - reserved;
            assert!(
                reserved >= floor,
                "{name}: reserve falls below the profile minimum"
            );
            assert!(
                boundary <= screen.cursor_row,
                "{name}: history covers the caret"
            );
            assert!(
                boundary < prompt as usize,
                "{name}: history covers part of the composer"
            );
            if name == "short footer" {
                short_reserve = reserved;
            } else {
                assert!(reserved > short_reserve, "{name}: reserve did not grow");
            }
        }
        let mut screen = terminal.screen()?;
        screen.cursor_has_position = false;
        assert!(input_rows(profile, &screen) > floor);
        Ok(())
    }

    #[test]
    fn adjacent_tool_results_are_grouped_and_unicode_truncation_preserves_boundaries() {
        let rows = group(
            [
                json!({"index":1,"kind":"tool","toolName":"Read","text":"Read path"}),
                json!({"index":2,"kind":"tool","toolName":"tool_result","text":"output"}),
                json!({"index":3,"kind":"assistant","text":"Done"}),
            ]
            .iter()
            .filter_map(Message::parse),
        );
        assert_eq!(rows.len(), 2);
        assert!(
            matches!(&rows[0], Row::Tool { call: Some(call), result: Some(result) } if call.index == 1 && result.index == 2)
        );
        assert_eq!(truncate("早𠮷🌕", 2), ("早𠮷", true));
    }

    #[test]
    fn tool_summaries_use_readable_arguments_and_fall_back_to_text() {
        for (name, text, expected) in [
            (
                "Read",
                "Checking a file\nRead {\"file_path\":\"src/你好 a.rs\"}",
                "Read src/你好 a.rs",
            ),
            (
                "Bash",
                "Bash {\"command\":\"echo hi\\nprintf done\",\"timeout\":50}",
                "Bash echo hi printf done",
            ),
            (
                "Edit",
                "Edit {\"file_path\":\"src/main.rs\",\"new_string\":\"unused\"}",
                "Edit src/main.rs",
            ),
            (
                "Write",
                "Write {\"file_path\":\"notes.md\",\"content\":\"body\"}",
                "Write notes.md",
            ),
            (
                "Grep",
                "Grep {\"path\":\"src\",\"pattern\":\"history\"}",
                "Grep pattern: history",
            ),
            ("Read", "Read unclear path", "Read unclear path"),
            ("Read", "Read", "Read"),
            (
                "Bash",
                "cat notes.md\nresult first line\nresult second line",
                "Bash cat notes.md · result first line",
            ),
        ] {
            assert_eq!(tool_summary(name, text), expected);
        }
        let input = format!("Bash {{\"command\":\"{}\"}}", "中".repeat(150));
        assert_eq!(
            tool_summary("Bash", &input),
            format!("Bash {}…", "中".repeat(120))
        );
        let input = format!(
            "Write {{\"content\":\"{}\",\"file_path\":\"notes.md\"}}",
            "x".repeat(EXPANDED_CHARS + 100)
        );
        let message = Message::parse(&json!({
            "index": 1, "kind": "tool", "toolName": "Write", "text": input
        }))
        .expect("tool message");
        assert_eq!(
            message
                .tool_preview
                .as_ref()
                .map(|preview| preview.as_ref()),
            Some("Write notes.md")
        );
        assert_eq!(message.text.chars().count(), EXPANDED_CHARS);
        assert!(message.longer);
        assert_eq!(
            result_preview(r#"[{"type":"text","text":"文件已读取\n第二行"}]"#).as_deref(),
            Some("文件已读取")
        );
        assert_eq!(result_preview(r#"{"opaque":{"value":1}}"#), None);
    }

    #[test]
    fn matches_first_conversation_row_below_claude_startup_decoration() -> anyhow::Result<()> {
        use matou_native_terminal::{Terminal, TerminalOptions};

        let mut terminal = Terminal::new(TerminalOptions::new(80, 20, 8, 14))?;
        terminal.feed(
            "Header line one\r\nHeader line two\r\nHeader line three\r\nHeader line four\r\nHeader line five\r\nConversation compacted\r\n❯ 核对入口卷起后的位置\r\n⏺ 保留刚才的画面\r\n".as_bytes(),
        );
        let messages: Vec<_> = [
            json!({"index":29,"kind":"user","text":"核对入口卷起后的位置"}),
            json!({"index":30,"kind":"assistant","text":"保留刚才的画面"}),
        ]
        .iter()
        .filter_map(Message::parse)
        .collect();
        let matches = anchor(&terminal.screen()?, 5, &messages);
        assert_eq!(matches, Some((29, 0)));
        Ok(())
    }

    #[test]
    fn strips_terminal_escape_sequences_from_transcript_text() {
        let message = Message::parse(&json!({
            "index": 1, "kind": "assistant",
            "text": "\u{1b}[2m已完成\u{1b}[22m。\u{1b}]8;;https://example.com\u{1b}\\结果\u{1b}]8;;\u{7}",
        }))
        .unwrap();
        assert_eq!(message.text.as_ref(), "已完成。结果");
        let result = Message::parse(&json!({
            "index": 2, "kind": "tool", "toolName": "tool_result",
            "text": "<tool_use_error>Error: could not open file</tool_use_error>",
        }))
        .unwrap();
        assert_eq!(result.kind, MessageKind::Result);
        assert_eq!(
            result.tool_preview.as_ref().map(|preview| preview.as_ref()),
            Some("Error: could not open file")
        );
    }

    #[test]
    fn assistant_history_preserves_literal_tags_and_strips_ansi() {
        let replies = [
            "这是 `<command-name>` 的例子。\n\n最后一段也应显示。",
            "<command-name> 是一个未闭合的语法示例。\n\n最后一段也应显示。",
            "<system-reminder>示例</system-reminder>\n\n最后一段也应显示。",
            "围栏内的例子：\n\n```xml\n<system-reminder>示例</system-reminder>\n```\n\n最后一段也应显示。",
            "<command-name>/example</command-name>\n\n这是字面 `[2m`，最后一段也应显示。",
        ];
        for expected in replies {
            let message = Message::parse(&json!({
                "index": 42, "kind": "assistant", "text": format!("\x1b[2m{expected}\x1b[22m"),
            }))
            .expect("assistant reply must remain visible");
            assert_eq!(message.body(false), Some((expected, false)));
            assert_eq!(message.kind, MessageKind::Assistant);
            assert_eq!(message.index, 42);
        }
    }

    #[test]
    fn displays_slash_command_with_its_arguments() {
        let message = Message::parse(&json!({
            "index": 1, "kind": "user",
            "text": "<command-name>/compact</command-name>\n<command-message>compact</command-message>\n<command-args>keep decisions</command-args>",
        }))
        .unwrap();
        assert_eq!(message.kind, MessageKind::Command);
        assert_eq!(message.text.as_ref(), "/compact keep decisions");
    }

    #[test]
    fn omits_command_caveats_and_hidden_transcript_envelopes() {
        let wrapped = [
            "<local-command-caveat>Caveat</local-command-caveat>",
            "<system-reminder>Reminder</system-reminder>",
            "<task-notification><summary>Finished</summary></task-notification>",
            r#"<teammate-message teammate_id="local">Agent activity</teammate-message>"#,
            "<system-reminder>Reminder with no closing tag",
            "<task-notification><summary>Truncated notification",
            r#"<teammate-message teammate_id="local""#,
            "[SYSTEM NOTIFICATION - NOT USER INPUT]\nAutomated preamble\n<task-notification>Finished</task-notification>",
        ];
        for text in wrapped {
            assert!(Message::parse(&json!({"index": 1, "kind": "user", "text": text})).is_none());
        }
        let message = Message::parse(&json!({
            "index": 2, "kind": "user",
            "text": "保留这条消息\n<system-reminder>Reminder</system-reminder>",
        }))
        .unwrap();
        assert_eq!(message.kind, MessageKind::User);
        assert_eq!(message.text.as_ref(), "保留这条消息");
    }

    #[test]
    fn synthetic_user_feedback_does_not_become_a_history_row() {
        let events = [
            "Stop hook feedback:\n[ok 完成该计划]: Transcript shows a task was still running.",
            "\u{1b}[2mStop hook feedback:\u{1b}[22m Hidden feedback",
            "<system-reminder>Internal context</system-reminder>\n[Image #1] Stop hook feedback: hidden",
            "A session-scoped Stop hook was set",
            "Base directory for this skill: /example",
            "Launching skill: example",
            "Goal check-in: keep working",
            "[Request interrupted by user]",
            "[Image: source: /example.png]",
        ]
        .map(|text| json!({"index": 1, "kind": "user", "text": text}));
        assert!(group(events.iter().filter_map(Message::parse)).is_empty());

        let quoted = Message::parse(&json!({
            "index": 2, "kind": "assistant", "text": "Stop hook feedback: quoted from the log",
        }))
        .unwrap();
        assert_eq!(quoted.kind, MessageKind::Assistant);
    }

    #[test]
    fn displays_local_command_stdout_as_output() {
        let stdout = "<local-command-stdout>\u{1b}[2mCompacted (ctrl+o to see full summary)\u{1b}[22m</local-command-stdout>";
        let message = Message::parse(&json!({"index": 1, "kind": "user", "text": stdout})).unwrap();
        assert_eq!(message.kind, MessageKind::CommandOutput);
        assert_eq!(
            message.text.as_ref(),
            "Compacted (ctrl+o to see full summary)"
        );
        let combined = Message::parse(&json!({
            "index": 2, "kind": "user",
            "text": format!("<command-name>/compact</command-name>{stdout}"),
        }))
        .unwrap();
        assert_eq!(combined.kind, MessageKind::Command);
        assert_eq!(combined.command_output.as_ref(), Some(&message.text));
    }

    #[test]
    fn compaction_summary_hides_its_body_until_expanded() {
        let message = Message::parse(&json!({
            "index": 1, "kind": "user", "isCompactSummary": true,
            "text": "摘要".repeat(EXPANDED_CHARS),
        }))
        .unwrap();
        assert_eq!(message.kind, MessageKind::CompactSummary);
        assert_eq!(message.body(false), None);
        let (body, clipped) = message.body(true).unwrap();
        assert_eq!(body.chars().count(), EXPANDED_CHARS);
        assert!(clipped);
    }
}
