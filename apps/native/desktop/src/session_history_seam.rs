use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, ClickEvent, FocusHandle, InteractiveElement, ListAlignment, ListState, Pixels,
    Rgba, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, prelude::*,
    px,
};
use matou_native_core::sessions::InterfaceLocale;
use matou_native_terminal::Screen;
use serde_json::Value;

use super::TerminalSurface;

// Claude reserves both composer borders, the prompt, a spacer and its status row.
pub(super) const INPUT_ROWS: usize = 5;
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

pub(super) fn fingerprint(screen: &Screen) -> u64 {
    let mut hash = DefaultHasher::new();
    screen.cols.hash(&mut hash);
    screen.rows.hash(&mut hash);
    for line in screen_lines(screen, screen.rows.saturating_sub(INPUT_ROWS)) {
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
    ch.is_whitespace() || matches!(ch, '❯' | '⏺' | '●')
}

#[derive(Clone, Debug)]
struct Message {
    index: i64,
    kind: MessageKind,
    timestamp: Option<u64>,
    text: SharedString,
    longer: bool,
    tool_name: Option<SharedString>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MessageKind {
    User,
    Assistant,
    Tool,
    Result,
    System,
}

impl Message {
    fn parse(value: &Value) -> Option<Self> {
        if is_compaction(value) {
            return None;
        }
        let tool_name = value["toolName"].as_str();
        let kind = match value["kind"].as_str()? {
            "user" => MessageKind::User,
            "assistant" => MessageKind::Assistant,
            "tool" if tool_name == Some("tool_result") => MessageKind::Result,
            "tool" => MessageKind::Tool,
            _ => MessageKind::System,
        };
        let text = value["text"].as_str()?;
        let (cap, longer) = truncate(text, EXPANDED_CHARS);
        Some(Self {
            index: value["index"].as_i64()?,
            kind,
            timestamp: timestamp(value),
            text: cap.to_owned().into(),
            longer,
            tool_name: tool_name.map(|name| name.to_owned().into()),
        })
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

/// Match against the same bounded text range that the history view can show.
fn anchor(screen: &Screen, rows: &[Value]) -> Option<(i64, usize)> {
    let visible: Vec<_> = screen_lines(screen, screen.rows.saturating_sub(INPUT_ROWS)).collect();
    // At the top of Claude's conversation, its startup decoration may occupy
    // several viewport rows before the first transcript line.
    let first_message = visible
        .iter()
        .position(|line| line.trim_start().starts_with(['❯', '⏺', '●']))
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
    for row in rows.iter().rev().filter(|row| !is_compaction(row)) {
        let (Some(index), Some(text)) = (row["index"].as_i64(), row["text"].as_str()) else {
            continue;
        };
        let searchable = normalized(truncate(text, EXPANDED_CHARS).0);
        for (priority, needle) in needles.iter().enumerate() {
            if found.is_some_and(|(previous, ..)| previous <= priority) {
                break;
            }
            if let Some(offset) = searchable.find(needle.as_str()) {
                found = Some((priority, index, text, offset));
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
    compact_time: Option<u64>,
    rows: Vec<Row>,
    expanded: HashSet<i64>,
    full_text: HashSet<i64>,
    snapshot: Option<Rc<Screen>>,
}

impl HistoryView {
    pub fn new(screen: &Screen, focus: FocusHandle) -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Bottom, px(720.)),
            focus,
            opened_at: Instant::now(),
            pending: None,
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
            compact_time: None,
            rows: Vec::new(),
            expanded: HashSet::new(),
            full_text: HashSet::new(),
            snapshot: Some(Rc::new(screen.clone())),
        }
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
        if matches!(page, Page::Updates(_)) {
            return;
        }
        self.retry = Some(page);
        if self.ready {
            self.list.splice(0..1, 1);
        }
    }

    pub fn receive(&mut self, request: Page, result: &Value) -> bool {
        let Some(events) = result["events"].as_array() else {
            self.fail(request);
            return false;
        };
        let page = &result["page"];
        let first = page["startEventIndex"].as_i64().unwrap_or_default();
        let last = page["endEventIndex"].as_i64().unwrap_or_default() - 1;
        if first == 0 && request == Page::Latest {
            self.fail(request);
            return false;
        }
        self.pending = None;
        match request {
            Page::Latest => {
                self.retry = None;
                self.latest_index = last;
                self.compact_time = events
                    .iter()
                    .find(|event| is_compaction(event))
                    .and_then(timestamp);
                let matched = self
                    .snapshot
                    .as_ref()
                    .and_then(|screen| anchor(screen, events));
                let mut messages = events.iter().filter_map(|value| {
                    let mut message = Message::parse(value)?;
                    if let Some((index, offset)) = matched {
                        if message.index > index || (message.index == index && offset == 0) {
                            return None;
                        }
                        if message.index == index {
                            let text = value["text"].as_str()?;
                            let (prefix, longer) =
                                truncate(text[..offset].trim_end(), EXPANDED_CHARS);
                            if prefix.is_empty() {
                                return None;
                            }
                            message.text = prefix.to_owned().into();
                            message.longer = longer;
                        }
                    }
                    Some(message)
                });
                self.rows = group(&mut messages);
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
            let label = self.rows.first().and_then(Row::timestamp).map_or_else(
                || beginning.to_owned(),
                |time| format!("{beginning} · {}", start_date(time, locale)),
            );
            return label_row(label, dim, style.line_height);
        }
        if let Some(row) = self.rows.get(index - 1) {
            return match row {
                Row::Message(message) => self.message(message, style, true, surface),
                Row::Tool { call, result } => {
                    self.tool(row.index(), call.as_ref(), result.as_ref(), style, surface)
                }
            };
        }
        let label = if index == self.rows.len() + 1 {
            locale
                .text("更早的会话记录", "Earlier conversation")
                .to_owned()
        } else if let Some(time) = self.compact_time {
            let (_, time) = crate::workspace::local_date_time(time);
            format!(
                "── {} · {time} ──",
                locale.text("会话已压缩", "Conversation compacted")
            )
        } else {
            String::new()
        };
        label_row(label, dim, style.line_height)
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
            line_height,
        } = style;
        let open = self.full_text.contains(&message.index);
        let (text, clipped) = if open {
            (message.text.as_ref(), message.longer)
        } else {
            truncate(&message.text, COLLAPSED_CHARS)
        };
        let prefix = match (prefix_visible, message.kind) {
            (true, MessageKind::User) => "❯ ",
            (true, MessageKind::Assistant) => "⏺ ",
            _ => "",
        };
        div()
            .py(line_height * 0.25)
            .flex()
            .gap(px(4.))
            .text_color(foreground)
            .when(!prefix.is_empty(), |div| div.child(div_prefix(prefix)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(div().whitespace_normal().child(if clipped {
                        format!("{text}…")
                    } else {
                        text.to_owned()
                    }))
                    .when(clipped && !open || open, |body| {
                        body.child(
                            div()
                                .id(("history-text", message.index as usize))
                                .cursor_pointer()
                                .on_click(click(surface, Action::Text(message.index)))
                                .text_color(dim)
                                .child(if open {
                                    locale.text("收起", "Collapse")
                                } else {
                                    locale.text("显示全部", "Show all")
                                }),
                        )
                    }),
            )
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
        let name = call
            .and_then(|message| message.tool_name.as_ref().map(|name| name.as_ref()))
            .unwrap_or_else(|| style.locale.text("工具结果", "Tool result"));
        let parameters = call.map_or("", |message| {
            message
                .text
                .lines()
                .find(|line| line.starts_with(name))
                .unwrap_or(&message.text)
                .strip_prefix(name)
                .unwrap_or(&message.text)
                .trim()
        });
        let first_result =
            result.and_then(|message| message.text.lines().find(|line| !line.trim().is_empty()));
        let summary = [
            Some(name),
            (!parameters.is_empty()).then_some(parameters),
            first_result,
        ]
        .into_iter()
        .flatten()
        .map(|part| truncate(part, 120).0)
        .collect::<Vec<_>>()
        .join(" · ");
        div()
            .py(style.line_height * 0.2)
            .child(
                div()
                    .id(("history-tool", index as usize))
                    .text_ellipsis()
                    .cursor_pointer()
                    .on_click(click(surface, Action::Tool(index)))
                    .text_color(style.dim)
                    .child(format!("{} {summary}", if expanded { "▾" } else { "▸" })),
            )
            .when(expanded, |div| {
                div.children(
                    call.into_iter()
                        .chain(result)
                        .map(|message| self.message(message, style, false, surface)),
                )
            })
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
pub(super) struct HistoryStyle {
    pub locale: InterfaceLocale,
    pub foreground: Rgba,
    pub dim: Rgba,
    pub line_height: Pixels,
}

fn start_date(timestamp: u64, locale: InterfaceLocale) -> String {
    let (date, time) = crate::workspace::local_date_time(timestamp);
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

fn label_row(label: String, color: Rgba, line_height: Pixels) -> AnyElement {
    div()
        .h(line_height)
        .text_color(color)
        .text_ellipsis()
        .child(label)
        .into_any_element()
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
    fn matches_first_conversation_row_below_claude_startup_decoration() -> anyhow::Result<()> {
        use matou_native_terminal::{Terminal, TerminalOptions};

        let mut terminal = Terminal::new(TerminalOptions::new(80, 20, 8, 14))?;
        terminal.feed(
            "Header line one\r\nHeader line two\r\nHeader line three\r\nHeader line four\r\nHeader line five\r\nConversation compacted\r\n❯ 核对入口卷起后的位置\r\n⏺ 保留刚才的画面\r\n".as_bytes(),
        );
        let matches = anchor(
            &terminal.screen()?,
            &[
                json!({"index":29,"kind":"user","text":"核对入口卷起后的位置"}),
                json!({"index":30,"kind":"assistant","text":"保留刚才的画面"}),
            ],
        );
        assert_eq!(matches, Some((29, 0)));
        Ok(())
    }
}
