//! Read Claude Code's project transcripts without modifying the provider's catalog.
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap, VecDeque},
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;
use rusqlite::{Connection, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const LIST_LIMIT: usize = 200;
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_EVENT_PAGE_LIMIT: i64 = 200;
const MAX_EVENT_PAGE_LIMIT: i64 = 500;
const SESSION_NOT_FOUND: &str = "Provider session was not found in this working directory";
const MAX_RECENT_REPLIES: usize = 4;
const MAX_REPLY_UNITS: usize = 240;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogEntry {
    pub provider_session_id: String,
    pub title: String,
    pub updated_at: i64,
    pub model: Option<String>,
    pub permission_mode: String,
    pub message_count: usize,
    pub loaded_by: Option<Value>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionDetail {
    pub events: Vec<PreviewEvent>,
    pub page: EventPage,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewEvent {
    index: usize,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<Value>,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    is_compact_summary: bool,
    matched: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentReply {
    pub kind: String,
    pub role: Option<String>,
    pub timestamp: Option<Value>,
    pub text: String,
    pub tool_name: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentReplies {
    pub title: Option<String>,
    pub cwd: String,
    pub updated_label: Option<String>,
    pub events: Vec<RecentReply>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventPage {
    start_event_index: usize,
    end_event_index: usize,
    total: usize,
    has_earlier: bool,
    has_later: bool,
}

#[derive(Default)]
struct Skipped {
    outside_root: usize,
    malformed_lines: usize,
    oversized_lines: usize,
    unreadable_files: usize,
}

impl Skipped {
    fn report(&self) {
        if self.outside_root + self.malformed_lines + self.oversized_lines + self.unreadable_files
            > 0
        {
            eprintln!(
                "[provider-catalog] skipped: outside_root={}, malformed_lines={}, oversized_lines={}, unreadable_files={}",
                self.outside_root,
                self.malformed_lines,
                self.oversized_lines,
                self.unreadable_files
            );
        }
    }
}

pub(crate) fn projects_root() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".claude")))
        .map(|config| config.join("projects"))
}

pub(crate) fn read(
    root: Option<&Path>,
    cwd: &str,
    selected: Option<&str>,
) -> Result<Vec<CatalogEntry>, &'static str> {
    if selected.is_some_and(|id| !valid_id(id)) {
        return Err("Invalid provider session ID");
    }
    let Some((root, cwd, workspace)) = catalog_paths(root, cwd)? else {
        return Ok(Vec::new());
    };
    let mut skipped = Skipped::default();
    let result = scan(&root, &cwd, &workspace, selected, &mut skipped);
    skipped.report();
    result
}

fn catalog_paths(
    root: Option<&Path>,
    cwd: &str,
) -> Result<Option<(PathBuf, PathBuf, PathBuf)>, &'static str> {
    let Some(root) = root else {
        return Err("Claude Code configuration directory is unavailable");
    };
    let cwd =
        absolute(Path::new(cwd)).map_err(|_| "Could not resolve the catalog working directory")?;
    let workspace = match fs::canonicalize(&cwd) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => return Ok(None),
        Err(error) if missing(&error) => return Ok(None),
        Err(_) => return Err("Could not resolve the catalog working directory"),
    };
    let root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) if missing(&error) => return Ok(None),
        Err(_) => return Err("Could not resolve the Claude Code projects directory"),
    };
    Ok(Some((root, cwd, workspace)))
}

const TIMESTAMP_SQL: &str = "SELECT ROUND(unixepoch(?1, 'subsec') * 1000)";

fn scan(
    root: &Path,
    cwd: &Path,
    workspace: &Path,
    selected: Option<&str>,
    skipped: &mut Skipped,
) -> Result<Vec<CatalogEntry>, &'static str> {
    let names = project_names(cwd, workspace);
    let mut visited = BTreeSet::new();
    let mut entries = HashMap::new();
    // SQLite is already linked. Use its ISO-8601 parser for the CLI's timestamp
    // strings, including offsets, while preserving numeric millisecond values.
    let dates = Connection::open_in_memory()
        .map_err(|_| "Could not initialize catalog timestamp parsing")?;
    let mut timestamp = dates
        .prepare(TIMESTAMP_SQL)
        .map_err(|_| "Could not initialize catalog timestamp parsing")?;
    for name in names {
        let directory = match contained(root, &root.join(name), skipped) {
            Ok(Some(directory)) if directory.is_dir() && visited.insert(directory.clone()) => {
                directory
            }
            Ok(_) => continue,
            Err(_) => return Err("Could not inspect a Claude Code project directory"),
        };
        let candidates = if let Some(id) = selected {
            vec![directory.join(format!("{id}.jsonl"))]
        } else {
            fs::read_dir(&directory)
                .and_then(|files| {
                    files
                        .map(|file| file.map(|file| file.path()))
                        .collect::<io::Result<Vec<_>>>()
                })
                .map_err(|_| "Could not list a Claude Code project directory")?
        };
        for candidate in candidates {
            let Some(id) = candidate
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".jsonl"))
                .filter(|id| valid_id(id))
            else {
                continue;
            };
            let entry = (|| {
                let Some((file, _)) = open_transcript(root, &candidate, skipped)? else {
                    return Ok(None);
                };
                index(file, id, workspace, &mut timestamp, skipped)
            })();
            match entry {
                Ok(Some(entry)) => {
                    entries.insert(entry.provider_session_id.clone(), entry);
                }
                Ok(None) => {}
                Err(error) if missing(&error) => {}
                Err(_) => skipped.unreadable_files += 1,
            }
        }
    }
    let mut entries: Vec<_> = entries.into_values().collect();
    entries.sort_unstable_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.provider_session_id.cmp(&right.provider_session_id))
    });
    entries.truncate(LIST_LIMIT);
    Ok(entries)
}

fn project_names(cwd: &Path, workspace: &Path) -> [String; 2] {
    [encode(cwd), encode(&display_path(workspace))]
}

pub(crate) fn resume_transcript(root: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let canonical_cwd = cwd.canonicalize().ok()?;
    project_names(cwd, &canonical_cwd)
        .into_iter()
        .rev()
        .find_map(|project| {
            let candidate = root.join(project).join(format!("{id}.jsonl"));
            open_transcript(&root, &candidate, &mut Skipped::default())
                .ok()
                .flatten()?;
            Some(candidate)
        })
}

pub(crate) fn detail(
    root: Option<&Path>,
    cwd: &str,
    id: &str,
    start_event_index: Option<i64>,
    limit: Option<i64>,
) -> Result<SessionDetail, &'static str> {
    if !valid_id(id) {
        return Err("Invalid provider session ID");
    }
    let (root, cwd, workspace) = catalog_paths(root, cwd)?.ok_or(SESSION_NOT_FOUND)?;
    let mut skipped = Skipped::default();
    let start = start_event_index.map(|start| usize::try_from(start.max(1)).unwrap_or(usize::MAX));
    let limit = limit
        .unwrap_or(DEFAULT_EVENT_PAGE_LIMIT)
        .clamp(1, MAX_EVENT_PAGE_LIMIT) as usize;
    let result = transcript_in_projects(
        &root,
        &cwd,
        &workspace,
        id,
        &mut skipped,
        |reader, dates, skipped| {
            preview_page(reader, &workspace, start, limit, dates, skipped).map(|page| {
                page.map(|page| {
                    let has_events = !page.events.is_empty();
                    (page, has_events)
                })
            })
        },
    );
    skipped.report();
    result
}

fn transcript_in_projects<T>(
    root: &Path,
    cwd: &Path,
    workspace: &Path,
    id: &str,
    skipped: &mut Skipped,
    mut read: impl FnMut(
        BufReader<io::Take<&File>>,
        &mut Statement<'_>,
        &mut Skipped,
    ) -> io::Result<Option<(T, bool)>>,
) -> Result<T, &'static str> {
    const READ_ERROR: &str = "Could not read the provider session transcript";
    const CHANGED_ERROR: &str = "Provider session transcript changed while it was being read";
    let names = project_names(cwd, workspace);
    let dates = Connection::open_in_memory()
        .map_err(|_| "Could not initialize catalog timestamp parsing")?;
    let mut timestamp = dates
        .prepare(TIMESTAMP_SQL)
        .map_err(|_| "Could not initialize catalog timestamp parsing")?;
    let mut visited = BTreeSet::new();
    let mut found = None;
    // Prefer the canonical bucket, which replaces the alias in the list.
    for name in names.into_iter().rev() {
        let project = root.join(name);
        let Some(directory) = contained(root, &project, skipped).map_err(|_| READ_ERROR)? else {
            continue;
        };
        if !directory.is_dir() || !visited.insert(directory.clone()) {
            continue;
        }
        let candidate = project.join(format!("{id}.jsonl"));
        let Some((file, path)) =
            open_transcript(root, &candidate, skipped).map_err(|_| READ_ERROR)?
        else {
            continue;
        };
        #[cfg(windows)]
        if !matches_windows_file(&path, &file).map_err(|_| READ_ERROR)? {
            return Err(CHANGED_ERROR);
        }
        let before = file.metadata().map_err(|_| READ_ERROR)?;
        let reader = BufReader::new((&file).take(before.len()));
        let page = read(reader, &mut timestamp, skipped).map_err(|_| READ_ERROR)?;
        let after = file.metadata().map_err(|_| READ_ERROR)?;
        if before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
            || !recheck_transcript(root, &candidate, &path, &file, skipped)
                .map_err(|_| READ_ERROR)?
        {
            return Err(CHANGED_ERROR);
        }
        #[cfg(windows)]
        if !matches_windows_file(&path, &file).map_err(|_| READ_ERROR)? {
            return Err(CHANGED_ERROR);
        }
        if let Some((detail, has_events)) = page {
            if has_events {
                return Ok(detail);
            }
            found = Some(detail);
        }
    }
    found.ok_or(SESSION_NOT_FOUND)
}

pub(crate) fn recent_replies(
    root: Option<&Path>,
    cwd: &str,
    id: &str,
) -> Result<RecentReplies, &'static str> {
    if !valid_id(id) {
        return Err("Invalid provider session ID");
    }
    let (root, cwd, workspace) = catalog_paths(root, cwd)?.ok_or(SESSION_NOT_FOUND)?;
    let mut skipped = Skipped::default();
    let dates = Connection::open_in_memory()
        .map_err(|_| "Could not initialize catalog timestamp parsing")?;
    let mut updated_label = dates
        .prepare(
            "SELECT printf('%d/%d %s', strftime('%m', ?1 / 1000., 'unixepoch', 'localtime'),
            strftime('%d', ?1 / 1000., 'unixepoch', 'localtime'),
            strftime('%H:%M', ?1 / 1000., 'unixepoch', 'localtime'))",
        )
        .map_err(|_| "Could not initialize catalog timestamp parsing")?;
    let result = transcript_in_projects(
        &root,
        &cwd,
        &workspace,
        id,
        &mut skipped,
        |reader, dates, skipped| {
            recent_replies_from_transcript(reader, &workspace, dates, &mut updated_label, skipped)
        },
    );
    skipped.report();
    result
}

fn recent_replies_from_transcript(
    mut reader: impl BufRead,
    workspace: &Path,
    dates: &mut Statement<'_>,
    updated_label: &mut Statement<'_>,
    skipped: &mut Skipped,
) -> io::Result<Option<(RecentReplies, bool)>> {
    let mut effective_origin = None;
    let mut updated_at = 0;
    let mut title = TranscriptTitle::default();
    let mut line = Vec::new();
    let mut total = 0;
    let mut dialogue = VecDeque::with_capacity(MAX_RECENT_REPLIES);
    let mut fallback = VecDeque::with_capacity(MAX_RECENT_REPLIES);
    while let Some(row) = read_row(&mut reader, &mut line, skipped, false)? {
        observe_workspace_origin(&mut effective_origin, &row);
        let event = preview_event(&row["message"]);
        title.observe(
            &row,
            event.as_ref().is_some_and(|event| event.kind == "user"),
        );
        updated_at = updated_at.max(update_millis(&row["timestamp"], dates).unwrap_or(0));
        let Some(event) = event else { continue };
        total += 1;
        let date = row.get("timestamp").cloned();
        if matches!(event.kind, "user" | "assistant") {
            push_recent(&mut dialogue, total, &event, date.clone(), event.kind);
        }
        push_recent(&mut fallback, total, &event, date, "other");
    }
    let Some(cwd) = effective_origin else {
        return Ok(None);
    };
    if !matches_workspace(&cwd, workspace)? {
        return Ok(None);
    }
    let first = total.saturating_sub(DEFAULT_EVENT_PAGE_LIMIT as usize);
    dialogue.retain(|(index, _)| *index > first);
    fallback.retain(|(index, _)| *index > first);
    let rows = if dialogue.is_empty() {
        fallback
    } else {
        dialogue
    };
    let events = rows
        .into_iter()
        .map(|(_, mut reply)| {
            reply.timestamp = reply
                .timestamp
                .take()
                .and_then(|date| preview_timestamp(date, dates));
            reply
        })
        .collect();
    let updated_label = (updated_at > 0)
        .then(|| {
            updated_label
                .query_row([updated_at], |row| row.get::<_, String>(0))
                .ok()
        })
        .flatten();
    let title = title
        .into_title()
        .map(|title| sanitize_reply("user", &title))
        .filter(|title| !title.is_empty());
    Ok(Some((
        RecentReplies {
            title,
            cwd,
            updated_label,
            events,
        },
        total > 0,
    )))
}

fn push_recent(
    recent: &mut VecDeque<(usize, RecentReply)>,
    index: usize,
    event: &PreviewEvent,
    timestamp: Option<Value>,
    filter_kind: &str,
) {
    let text = format_reply(&sanitize_reply(filter_kind, &event.text));
    if text.is_empty() {
        return;
    }
    if recent.len() == MAX_RECENT_REPLIES {
        recent.pop_front();
    }
    recent.push_back((
        index,
        RecentReply {
            kind: event.kind.into(),
            role: event.role.map(str::to_owned),
            timestamp,
            text,
            tool_name: event.tool_name.clone(),
        },
    ));
}

pub fn deferred_summary_text(text: &str) -> String {
    format_reply(&sanitize_reply("other", text))
}

fn preview_page(
    mut reader: impl BufRead,
    workspace: &Path,
    start: Option<usize>,
    limit: usize,
    timestamp: &mut Statement<'_>,
    skipped: &mut Skipped,
) -> io::Result<Option<SessionDetail>> {
    let mut effective_origin = None;
    let mut line = Vec::new();
    let mut total = 0;
    let mut events = VecDeque::with_capacity(limit);
    let mut last_before_start = None;
    let mut compact_boundary = None;
    while let Some(row) = read_row(&mut reader, &mut line, skipped, false)? {
        observe_workspace_origin(&mut effective_origin, &row);
        if start.is_none() && row["type"] == "system" && row["subtype"] == "compact_boundary" {
            compact_boundary = row
                .get("timestamp")
                .cloned()
                .map(|timestamp| (total, timestamp));
        }
        let Some(mut event) = preview_event(&row["message"]) else {
            continue;
        };
        total += 1;
        event.index = total;
        event.timestamp = row.get("timestamp").cloned();
        event.is_compact_summary = row["isCompactSummary"] == true;
        if let Some(start) = start {
            if total < start {
                last_before_start = Some(event);
                continue;
            }
            if events.len() == limit {
                continue;
            }
            last_before_start = None;
        } else if events.len() == limit {
            events.pop_front();
        }
        events.push_back(event);
    }
    // A file without an origin can only return an empty page.
    match effective_origin {
        Some(origin) if matches_workspace(&origin, workspace)? => {}
        None if total == 0 => {}
        _ => return Ok(None),
    }
    if let Some(last) = last_before_start {
        events.push_back(last);
    }
    for event in &mut events {
        event.timestamp = event
            .timestamp
            .take()
            .and_then(|date| preview_timestamp(date, timestamp));
    }
    let start_event_index = events.front().map_or(0, |event| event.index);
    let end_event_index = events.back().map_or(0, |event| event.index + 1);
    // The latest window carries its compaction marker alongside the page.
    // It does not consume a transcript index or a slot in the message window.
    if total > 0
        && let Some((index, timestamp_value)) = compact_boundary
        && let Some(time) = preview_timestamp(timestamp_value, timestamp)
    {
        events.push_back(PreviewEvent {
            index: index.max(1),
            kind: "system",
            role: None,
            timestamp: Some(time),
            text: "compact_boundary".into(),
            tool_name: None,
            is_compact_summary: false,
            matched: false,
        });
    }
    Ok(Some(SessionDetail {
        events: events.into(),
        page: EventPage {
            start_event_index,
            end_event_index,
            total,
            has_earlier: start_event_index > 1,
            has_later: end_event_index > 0 && end_event_index <= total,
        },
    }))
}

fn preview_event(message: &Value) -> Option<PreviewEvent> {
    let role = match message["role"].as_str() {
        Some("user") => Some("user"),
        Some("assistant") => Some("assistant"),
        _ => None,
    };
    let content = &message["content"];
    let mut text = String::new();
    let mut tool_name = None;
    if let Some(content) = content.as_str() {
        text.push_str(content);
    }
    for block in content.as_array().into_iter().flatten() {
        let part: Cow<'_, str> = match block["type"].as_str() {
            Some("text") => block["text"].as_str().unwrap_or_default().into(),
            Some("tool_use") if let Some(name) = block["name"].as_str() => {
                tool_name.get_or_insert(name);
                let input = block
                    .get("input")
                    .map_or_else(|| "undefined".into(), Value::to_string);
                let name = name.trim_start_matches(is_preview_space);
                if name.is_empty() {
                    input.into()
                } else {
                    format!("{name} {input}").into()
                }
            }
            Some("tool_result") => {
                tool_name.get_or_insert("tool_result");
                match block.get("content") {
                    Some(Value::String(content)) => content.as_str().into(),
                    Some(content) => content.to_string().into(),
                    None => continue,
                }
            }
            _ => continue,
        };
        if !part.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&part);
        }
    }
    let trimmed = trim_preview(&text);
    if trimmed.is_empty() {
        return None;
    }
    let kind = if tool_name.is_some() {
        "tool"
    } else {
        role.unwrap_or("system")
    };
    Some(PreviewEvent {
        index: 0,
        kind,
        role,
        timestamp: None,
        text: if trimmed.len() == text.len() {
            text
        } else {
            trimmed.into()
        },
        tool_name: tool_name.filter(|name| !name.is_empty()).map(str::to_owned),
        is_compact_summary: false,
        matched: false,
    })
}

fn trim_preview(text: &str) -> &str {
    text.trim_matches(is_preview_space)
}

fn is_preview_space(c: char) -> bool {
    // JavaScript String.trim also removes BOM and leaves NEL intact.
    c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}')
}

/// Read appended rows in bounded slices. Retry a partial final row after it is
/// committed, and reset the projections if the provider replaces the file.
#[derive(Default)]
pub(crate) struct LiveTranscript {
    file: Option<File>,
    offset: u64,
    partial: Vec<u8>,
    oversized: bool,
    title: TranscriptTitle,
}

impl LiveTranscript {
    pub(crate) fn title(&self) -> Option<&str> {
        self.title.title()
    }

    pub(crate) fn poll(
        &mut self,
        root: &Path,
        candidate: &Path,
        id: &str,
        mut ingest: impl FnMut(Option<&Value>),
    ) -> io::Result<bool> {
        let root = root.canonicalize()?;
        let Some((mut file, path)) = open_transcript(&root, candidate, &mut Skipped::default())?
        else {
            return Ok(false);
        };
        if path.file_name() != candidate.file_name() {
            return Ok(false);
        }
        let metadata = file.metadata()?;
        let mut replaced = metadata.len() < self.offset;
        if let Some(previous) = &self.file {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let previous = previous.metadata()?;
                replaced |= (previous.dev(), previous.ino()) != (metadata.dev(), metadata.ino());
            }
            #[cfg(windows)]
            {
                replaced |= !matches_windows_file(&path, previous)?;
            }
        }
        if replaced {
            *self = Self::default();
            ingest(None);
        }
        const READ_BUDGET: u64 = 256 * 1024;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        (&mut file).take(READ_BUDGET).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        self.file = Some(file);
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            self.oversized |= self.partial.len() as u64 + part.len() as u64 > MAX_LINE_BYTES;
            if !self.oversized {
                self.partial.extend_from_slice(part);
            }
            if !part.ends_with(b"\n") {
                continue;
            }
            self.ingest_line(id, &mut ingest);
            self.oversized = false;
            self.partial.clear();
        }
        let backlog = self.offset < metadata.len();
        // A transcript may end in a complete JSON value without a final newline.
        // Keep an incomplete value buffered until Claude appends the rest.
        if !backlog && !self.partial.is_empty() && self.ingest_line(id, &mut ingest) {
            self.partial.clear();
        }
        Ok(backlog)
    }

    fn ingest_line(&mut self, id: &str, ingest: &mut impl FnMut(Option<&Value>)) -> bool {
        if self.oversized {
            return false;
        }
        let Ok(row) = serde_json::from_slice::<Value>(&self.partial) else {
            return false;
        };
        if row.is_object()
            && row["sessionId"]
                .as_str()
                .is_none_or(|session| session == id)
        {
            let message = &row["message"];
            let (has_text, tool) = message_content(&message["content"]);
            let user = row["type"] == "user" && message["role"] == "user" && has_text && !tool;
            self.title.observe(&row, user);
            ingest(Some(&row));
        }
        true
    }
}

fn open_transcript(
    root: &Path,
    candidate: &Path,
    skipped: &mut Skipped,
) -> io::Result<Option<(File, PathBuf)>> {
    let Some(path) = contained(root, candidate, skipped)? else {
        return Ok(None);
    };
    if !fs::metadata(&path)?.is_file() {
        return Ok(None);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path)?;
    Ok(recheck_transcript(root, candidate, &path, &file, skipped)?.then_some((file, path)))
}

fn recheck_transcript(
    root: &Path,
    candidate: &Path,
    path: &Path,
    file: &File,
    skipped: &mut Skipped,
) -> io::Result<bool> {
    // Do not read after the candidate has been replaced or redirected.
    if contained(root, candidate, skipped)?.as_deref() != Some(path) || !file.metadata()?.is_file()
    {
        skipped.unreadable_files += 1;
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        let checked = fs::metadata(path)?;
        if (opened.dev(), opened.ino()) != (checked.dev(), checked.ino()) {
            skipped.outside_root += 1;
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(windows)]
fn matches_windows_file(path: &Path, opened: &File) -> io::Result<bool> {
    use std::{ffi::c_void, mem::size_of, os::windows::io::AsRawHandle};

    #[derive(Default, PartialEq, Eq)]
    #[repr(C)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut c_void,
            class: i32,
            info: *mut FileIdInfo,
            size: u32,
        ) -> i32;
    }

    fn file_id(file: &File) -> io::Result<FileIdInfo> {
        const FILE_ID_INFO: i32 = 18;
        let mut info = FileIdInfo::default();
        // The handle is live and info has the layout and size of FILE_ID_INFO.
        let status = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FILE_ID_INFO,
                &mut info,
                size_of::<FileIdInfo>() as u32,
            )
        };
        if status == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(info)
        }
    }

    Ok(file_id(opened)? == file_id(&File::open(path)?)?)
}

fn contained(root: &Path, path: &Path, skipped: &mut Skipped) -> io::Result<Option<PathBuf>> {
    match fs::canonicalize(path) {
        Ok(path) if path.starts_with(root) => Ok(Some(path)),
        Ok(_) => {
            skipped.outside_root += 1;
            Ok(None)
        }
        Err(error) if missing(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    let mut resolved = PathBuf::new();
    // Normalize dot components before realpath, as path.resolve does.
    for component in std::path::absolute(path)?.components() {
        if component == Component::ParentDir {
            resolved.pop();
        } else {
            resolved.push(component);
        }
    }
    Ok(resolved)
}

#[cfg(windows)]
fn display_path(path: &Path) -> PathBuf {
    // realpath emits ordinary drive and UNC paths for the project encoder.
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(&path))
    }
}

#[cfg(not(windows))]
fn display_path(path: &Path) -> PathBuf {
    path.to_owned()
}

fn encode(absolute: &Path) -> String {
    absolute
        .to_string_lossy()
        .encode_utf16()
        .map(|unit| match unit {
            48..=57 | 65..=90 | 97..=122 => char::from(unit as u8),
            _ => '-',
        })
        .collect()
}

fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=160).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(byte))
}

fn read_row(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    skipped: &mut Skipped,
    count_line_ending: bool,
) -> io::Result<Option<Value>> {
    loop {
        line.clear();
        if reader
            .by_ref()
            .take(MAX_LINE_BYTES + if count_line_ending { 1 } else { 3 })
            .read_until(b'\n', line)?
            == 0
        {
            return Ok(None);
        }
        let mut size = line.len();
        if !count_line_ending && line.ends_with(b"\n") {
            size -= if line.ends_with(b"\r\n") { 2 } else { 1 };
        }
        if size as u64 > MAX_LINE_BYTES {
            skipped.oversized_lines += 1;
            if !line.ends_with(b"\n") {
                reader.skip_until(b'\n')?;
            }
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(row) if !count_line_ending && !row.is_object() => skipped.malformed_lines += 1,
            Ok(row) => return Ok(Some(row)),
            Err(_) => skipped.malformed_lines += 1,
        }
    }
}

fn observe_workspace_origin(effective_origin: &mut Option<String>, row: &Value) {
    // The first cwd establishes the origin. Only session-level migrations can update it.
    let migrated = match row["type"].as_str() {
        Some("relocated") => nonempty(&row["relocatedCwd"]),
        Some("worktree-state") => nonempty(&row["worktreeSession"]["worktreePath"]),
        _ => None,
    };
    if let Some(path) = migrated {
        *effective_origin = Some(path.to_owned());
    } else if effective_origin.is_none() {
        *effective_origin = nonempty(&row["cwd"]).map(str::to_owned);
    }
}

fn matches_workspace(origin: &str, workspace: &Path) -> io::Result<bool> {
    let origin = absolute(Path::new(origin))?;
    Ok(fs::canonicalize(&origin).unwrap_or(origin) == workspace)
}

fn parse_date(date: &Value, timestamp: &mut Statement<'_>) -> Option<i64> {
    timestamp
        .query_row([date.as_str()?], |row| row.get::<_, Option<f64>>(0))
        .ok()
        .flatten()
        .map(|millis| millis as i64)
}

fn preview_timestamp(date: Value, timestamp: &mut Statement<'_>) -> Option<Value> {
    if date.is_number() {
        Some(date)
    } else {
        parse_date(&date, timestamp).map(Value::from)
    }
}

fn index(
    file: File,
    id: &str,
    workspace: &Path,
    timestamp: &mut Statement<'_>,
    skipped: &mut Skipped,
) -> io::Result<Option<CatalogEntry>> {
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut effective_origin = None;
    let mut title = TranscriptTitle::default();
    let mut entry = CatalogEntry {
        provider_session_id: id.to_owned(),
        title: String::new(),
        updated_at: 0,
        model: None,
        permission_mode: "default".into(),
        message_count: 0,
        loaded_by: None,
    };
    while let Some(row) = read_row(&mut reader, &mut line, skipped, true)? {
        observe_workspace_origin(&mut effective_origin, &row);
        if let Some(millis) = update_millis(&row["timestamp"], timestamp) {
            entry.updated_at = entry.updated_at.max(millis);
        }
        let message = &row["message"];
        for mode in [&row["permissionMode"], &message["permissionMode"]] {
            if let Some(
                mode @ ("default" | "auto" | "acceptEdits" | "plan" | "bypassPermissions"),
            ) = mode.as_str()
            {
                entry.permission_mode = mode.to_owned();
            }
        }
        if let Some(model) = message["model"]
            .as_str()
            .filter(|model| !model.trim().is_empty())
        {
            entry.model = Some(model.to_owned());
        }
        let (has_text, tool) = message_content(&message["content"]);
        if has_text {
            entry.message_count += 1;
        }
        title.observe(&row, has_text && !tool && message["role"] == "user");
    }
    let Some(origin) = effective_origin else {
        return Ok(None);
    };
    if !matches_workspace(&origin, workspace)? || entry.message_count == 0 {
        return Ok(None);
    }
    entry.title = title
        .into_title()
        .unwrap_or_else(|| "未命名 Claude 会话".into());
    Ok(Some(entry))
}

fn nonempty(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn update_millis(date: &Value, timestamp: &mut Statement<'_>) -> Option<i64> {
    date.as_f64()
        .map(|millis| millis as i64)
        .or_else(|| parse_date(date, timestamp))
}

fn message_content(content: &Value) -> (bool, bool) {
    let mut has_text = content
        .as_str()
        .is_some_and(|text| !trim_preview(text).is_empty());
    let mut tool = false;
    for block in content.as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => {
                has_text |= block["text"]
                    .as_str()
                    .is_some_and(|text| !trim_preview(text).is_empty());
            }
            Some("tool_use") if block["name"].is_string() => {
                tool = true;
                has_text = true;
            }
            Some("tool_result") => {
                tool = true;
                has_text |= block.get("content").is_some_and(|value| {
                    value
                        .as_str()
                        .is_none_or(|text| !trim_preview(text).is_empty())
                });
            }
            _ => {}
        }
    }
    (has_text, tool)
}

#[derive(Default)]
struct TranscriptTitle {
    custom: Option<String>,
    auto: Option<String>,
    summary: Option<String>,
    prompt: Option<String>,
}

impl TranscriptTitle {
    fn observe(&mut self, row: &Value, user_prompt: bool) {
        if let Some(custom) = nonempty(&row["customTitle"]) {
            self.custom = Some(custom.into());
        }
        if row["type"] == "ai-title"
            && let Some(auto) = nonempty(&row["aiTitle"])
        {
            self.auto = Some(auto.into());
        }
        if row["type"] == "summary"
            && let Some(summary) = nonempty(&row["summary"])
        {
            self.summary = Some(summary.into());
        }
        if self.prompt.is_none() && user_prompt {
            let title = compact_title(&row["message"]["content"]);
            if !title.is_empty() {
                self.prompt = Some(title);
            }
        }
    }

    fn into_title(self) -> Option<String> {
        self.custom.or(self.auto).or(self.prompt)
    }

    fn title(&self) -> Option<&str> {
        self.custom
            .as_deref()
            .or(self.auto.as_deref())
            .or(self.summary.as_deref())
            .or(self.prompt.as_deref())
    }
}

const SYNTHETIC_TAGS: &[&str] = &[
    "local-command-caveat",
    "local-command-stdout",
    "system-reminder",
    "task-notification",
    "teammate-message",
    "agent-message",
];

pub enum HistoryText<'a> {
    Plain(Cow<'a, str>),
    Command { line: String, output: String },
    CommandOutput(String),
}

pub enum HistoryTextSource {
    Assistant,
    User,
    Envelope,
}

static HISTORY_ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?:\x1b\[|\x{9b})[0-?]*[ -/]*[@-~]",
        r"|(?:\x1b\]|\x{9d})[^\x07\x1b\x{9c}]*(?:\x07|\x1b\\|\x{9c}|\z)",
        r"|(?:\x1b[PX^_]|[\x{90}\x{98}\x{9e}\x{9f}])[^\x1b\x{9c}]*(?:\x1b\\|\x{9c}|\z)",
        r"|\x1b[ -/]*[0-~]|\x1b",
    ))
    .unwrap()
});

static HISTORY_WRAPPERS: LazyLock<Regex> = LazyLock::new(|| {
    let patterns: Vec<_> = SYNTHETIC_TAGS
        .iter()
        .chain(&["command-name", "command-args", "command-message"])
        .map(|tag| {
            format!(
                r"<{tag}(?:\s+[^>]*)?>(?:.*?</{tag}\s*>|.*\z)|</{tag}\s*>|</?{tag}(?:\s+[^>]*)?\z"
            )
        })
        .collect();
    Regex::new(&format!("(?is){}", patterns.join("|"))).unwrap()
});

/// Interpret transcript envelopes for the in-card history view.
pub fn history_text(text: &str, source: HistoryTextSource) -> HistoryText<'_> {
    let text = HISTORY_ANSI.replace_all(text, "");
    let is_user = match source {
        HistoryTextSource::Assistant => return HistoryText::Plain(text),
        HistoryTextSource::User => true,
        HistoryTextSource::Envelope => false,
    };
    if is_user
        && starts_with_any(
            strip_image_markers(trim_preview(&text)),
            SYNTHETIC_USER_PREFIXES,
        )
    {
        return HistoryText::Plain(Cow::Borrowed(""));
    }
    let text = if text.contains("tool_use_error>") {
        Cow::Owned(
            text.replace("<tool_use_error>", "")
                .replace("</tool_use_error>", ""),
        )
    } else {
        text
    };
    // The provider journals the injected notification preamble separately from
    // the conversation, including when the task payload is an inner envelope.
    if text
        .trim_start()
        .starts_with("[SYSTEM NOTIFICATION - NOT USER INPUT]")
    {
        return HistoryText::Plain(Cow::Borrowed(""));
    }
    if !text.contains('<') {
        return HistoryText::Plain(text);
    }
    let mut command = String::new();
    let mut args = String::new();
    let mut output = String::new();
    let plain = HISTORY_WRAPPERS.replace_all(&text, |capture: &regex::Captures<'_>| {
        if let Some((tag, rest)) = capture[0].split_once('>')
            && let Some(name) = tag
                .strip_prefix('<')
                .and_then(|tag| tag.split_whitespace().next())
            && let Some((body, closing)) = rest.rsplit_once("</")
            && closing
                .trim_end_matches('>')
                .trim()
                .eq_ignore_ascii_case(name)
        {
            match name.to_ascii_lowercase().as_str() {
                "command-name" if command.is_empty() => command.push_str(body.trim()),
                "command-args" if args.is_empty() => args.push_str(body.trim()),
                "local-command-stdout" if !body.trim().is_empty() => {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(body.trim());
                }
                _ => {}
            }
        }
        ""
    });
    if matches!(plain, Cow::Borrowed(_)) {
        return HistoryText::Plain(text);
    }
    let plain = trim_preview(&plain);
    if is_user && starts_with_any(strip_image_markers(plain), SYNTHETIC_USER_PREFIXES) {
        return HistoryText::Plain(Cow::Borrowed(""));
    }
    if !plain.is_empty() {
        return HistoryText::Plain(Cow::Owned(plain.into()));
    }
    if !command.is_empty() {
        if !args.is_empty() {
            command.push(' ');
            command.push_str(&args);
        }
        return HistoryText::Command {
            line: command,
            output,
        };
    }
    if !output.is_empty() {
        return HistoryText::CommandOutput(output);
    }
    HistoryText::Plain(Cow::Borrowed(""))
}

const SYNTHETIC_USER_PREFIXES: &[&str] = &[
    "Base directory for this skill:",
    "Launching skill:",
    "Stop hook feedback:",
    "A session-scoped Stop hook",
    "Goal check-in:",
    "[Request interrupted by user]",
    "[Image: source:",
];

const SYNTHETIC_ASSISTANT_PREFIXES: &[&str] = &[
    "API Error:",
    "Prompt is too long",
    "automatic compaction failed:",
    "All credentials for model ",
    "No response requested.",
];

// Separate alternatives pair each closing tag with its own opening tag.
static SYNTHETIC_BLOCKS: LazyLock<Regex> = LazyLock::new(|| {
    let patterns: Vec<_> = SYNTHETIC_TAGS
        .iter()
        .map(|tag| format!(r"<{tag}(?-u:\b)[^>]*>.*?</{tag}>"))
        .collect();
    Regex::new(&format!("(?is){}", patterns.join("|"))).unwrap()
});
static UNCLOSED_SYNTHETIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?is)<(?:{})(?-u:\b).*",
        SYNTHETIC_TAGS.join("|")
    ))
    .unwrap()
});
static COMMAND_ARGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<command-args>(.*?)</command-args>").unwrap());
static IMAGE_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\[Image #[0-9]+\]").unwrap());

fn starts_with_any(text: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| text.starts_with(prefix))
}

fn strip_image_markers(mut text: &str) -> &str {
    while let Some(marker) = IMAGE_MARKER.find(text) {
        text = text[marker.end()..].trim_start_matches(is_preview_space);
    }
    trim_preview(text)
}

fn user_text(text: &str, title: bool) -> String {
    // The directory title and the card deliberately handle unclosed tags differently.
    if title
        && !text.starts_with(['<', '['])
        && text.len() > 32_768
        && text.encode_utf16().count() > 32_768
    {
        return text.into();
    }
    let mut text = trim_preview(text);
    while text.contains("<command-name>") && !starts_with_any(text, SYNTHETIC_USER_PREFIXES) {
        let Some(args) = COMMAND_ARGS.captures(text).and_then(|args| args.get(1)) else {
            return String::new();
        };
        text = trim_preview(args.as_str());
        if title
            && !text.starts_with(['<', '['])
            && text.len() > 32_768
            && text.encode_utf16().count() > 32_768
        {
            return text.into();
        }
    }
    if starts_with_any(text, SYNTHETIC_USER_PREFIXES) {
        return String::new();
    }
    let cleaned = SYNTHETIC_BLOCKS.replace_all(text, " ");
    if title && starts_with_any(&cleaned, &["<local-command-", "<task-notification"]) {
        return String::new();
    }
    let cleaned = if title {
        cleaned
    } else {
        Cow::Owned(UNCLOSED_SYNTHETIC.replace(&cleaned, " ").into_owned())
    };
    let cleaned = strip_image_markers(&cleaned);
    if starts_with_any(cleaned, SYNTHETIC_USER_PREFIXES) {
        return String::new();
    }
    cleaned.into()
}

fn sanitize_reply(kind: &str, text: &str) -> String {
    let text = trim_preview(text);
    if kind == "user" {
        return user_text(text, false);
    }
    if starts_with_any(text, SYNTHETIC_ASSISTANT_PREFIXES) {
        return String::new();
    }
    if kind == "assistant" {
        return text.into();
    }
    let stripped = SYNTHETIC_BLOCKS.replace_all(text, " ");
    trim_preview(&UNCLOSED_SYNTHETIC.replace(&stripped, " ")).into()
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_preview_space).filter(|word| !word.is_empty())
}

fn format_reply(text: &str) -> String {
    let units: Vec<_> = words(text)
        .enumerate()
        .flat_map(|(index, word)| {
            (index != 0)
                .then_some(u16::from(b' '))
                .into_iter()
                .chain(word.encode_utf16())
        })
        .take(MAX_REPLY_UNITS + 1)
        .collect();
    if units.len() > MAX_REPLY_UNITS {
        format!("{}…", String::from_utf16_lossy(&units[..MAX_REPLY_UNITS]))
    } else {
        String::from_utf16_lossy(&units)
    }
}

fn compact_title(content: &Value) -> String {
    let blocks = content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .chain(content.as_str());
    let mut units = Vec::with_capacity(73);
    for block in blocks {
        let cleaned = user_text(block, true);
        for word in words(&cleaned) {
            if !units.is_empty() {
                units.push(u16::from(b' '));
            }
            units.extend(word.encode_utf16().take(73 - units.len()));
            if units.len() == 73 {
                return format!("{}…", String::from_utf16_lossy(&units[..71]));
            }
        }
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn transcript(cwd: &Path, timestamp: Value) -> Value {
        json!({
            "cwd": cwd, "timestamp": timestamp, "permissionMode": "plan",
            "message": {"role": "user", "content": "First   question"}
        })
    }

    fn write_rows(path: &Path, rows: &[Value]) -> io::Result<()> {
        let body = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, body)
    }

    #[test]
    fn encoding_matches_utf16_path_resolve() -> io::Result<()> {
        let path = Path::new(if cfg!(windows) {
            r"C:\tmp\drop\..\中 文🚀"
        } else {
            "/tmp/drop/../中 文🚀"
        });
        assert_eq!(
            encode(&absolute(path)?),
            if cfg!(windows) {
                "C--tmp------"
            } else {
                "-tmp------"
            }
        );
        assert!(valid_id("Claude_01.a-b"));
        assert!(valid_id(&"a".repeat(160)));
        for id in [
            "",
            "../valid",
            "-leading",
            "with space",
            "历史",
            &"a".repeat(161),
        ] {
            assert!(!valid_id(id));
        }
        Ok(())
    }

    #[test]
    fn list_filters_by_origin_and_names_then_sorts_and_caps() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("project");
        let other = root.path().join("other");
        fs::create_dir(&cwd)?;
        fs::create_dir(&other)?;
        let projects = root.path().join("projects");
        let bucket = projects.join(encode(&cwd));
        fs::create_dir_all(&bucket)?;
        for i in 0..202 {
            write_rows(
                &bucket.join(format!("valid-{i}.jsonl")),
                &[transcript(&cwd, json!(i + 1))],
            )?;
        }
        write_rows(
            &bucket.join("space invalid.jsonl"),
            &[transcript(&cwd, json!(1000))],
        )?;
        write_rows(
            &bucket.join("wrong-origin.jsonl"),
            &[
                transcript(&other, json!(2000)),
                transcript(&cwd, json!(3000)),
            ],
        )?;
        write_rows(
            &bucket.join("missing-origin.jsonl"),
            &[json!({"message": {"role": "user", "content": "missing cwd"}})],
        )?;
        write_rows(&bucket.join("no-events.jsonl"), &[json!({"cwd": cwd})])?;
        write_rows(
            &bucket.join("not-jsonl.txt"),
            &[transcript(&cwd, json!(4000))],
        )?;
        let entries =
            read(Some(&projects), cwd.to_str().unwrap(), None).map_err(anyhow::Error::msg)?;
        assert_eq!(entries.len(), 200);
        assert_eq!(entries[0].provider_session_id, "valid-201");
        assert_eq!(entries[199].provider_session_id, "valid-2");
        assert!(
            entries
                .windows(2)
                .all(|window| window[0].updated_at > window[1].updated_at)
        );
        assert_eq!(entries[0].title, "First question");
        assert_eq!(entries[0].permission_mode, "plan");
        let selected = |id| read(Some(&projects), cwd.to_str().unwrap(), Some(id));
        assert_eq!(
            selected("valid-0").unwrap()[0].provider_session_id,
            "valid-0"
        );
        assert!(selected("wrong-origin").unwrap().is_empty());
        assert!(selected("missing").unwrap().is_empty());
        assert!(selected("../valid-0").is_err());
        assert!(
            read(Some(&projects), other.to_str().unwrap(), None)
                .unwrap()
                .is_empty()
        );
        assert!(
            read(
                Some(&projects),
                root.path().join("missing").to_str().unwrap(),
                None
            )
            .unwrap()
            .is_empty()
        );
        Ok(())
    }

    #[test]
    fn relocated_worktree_keeps_history_before_compaction() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let parent = root.path().join("project");
        let worktree = parent.join(".claude/worktrees/active");
        fs::create_dir_all(&worktree)?;
        let projects = root.path().join("projects");
        let bucket = projects.join(encode(&worktree));
        fs::create_dir_all(&bucket)?;
        let mut rows = vec![transcript(&parent, json!(1))];
        rows.resize(
            35_340,
            json!({"cwd": parent, "timestamp": 2, "message": {
                "role": "assistant", "content": "Inherited reply"
            }}),
        );
        rows.extend([
            json!({"type": "system", "subtype": "compact_boundary"}),
            json!({"type": "relocated", "relocatedCwd": worktree, "sessionId": "forked"}),
            json!({"cwd": worktree, "timestamp": 3, "message": {
                "role": "assistant", "content": "Worktree reply"
            }}),
        ]);
        write_rows(&bucket.join("forked.jsonl"), &rows)?;

        let cwd = worktree.to_str().unwrap();
        let entries = read(Some(&projects), cwd, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].provider_session_id, "forked");
        assert_eq!(entries[0].title, "First question");
        assert_eq!(entries[0].message_count, 35_341);

        let first = detail(Some(&projects), cwd, "forked", Some(1), Some(2)).unwrap();
        assert_eq!(first.page.total, 35_341);
        assert!(first.page.has_later);
        assert_eq!(first.events[0].text, "First   question");
        assert_eq!(first.events[1].text, "Inherited reply");
        let latest = detail(Some(&projects), cwd, "forked", None, Some(2)).unwrap();
        assert!(latest.page.has_earlier);
        assert_eq!(latest.events[0].text, "Inherited reply");
        assert_eq!(latest.events[1].text, "Worktree reply");

        let replies = recent_replies(Some(&projects), cwd, "forked").unwrap();
        assert_eq!(replies.cwd, cwd);
        assert!(replies.events.iter().map(|event| event.text.as_str()).eq([
            "Inherited reply",
            "Inherited reply",
            "Inherited reply",
            "Worktree reply"
        ]));
        Ok(())
    }

    #[test]
    fn final_worktree_state_moves_history_before_any_new_message() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let parent = root.path().join("project");
        let previous = parent.join(".claude/worktrees/previous");
        let worktree = parent.join(".claude/worktrees/active");
        fs::create_dir_all(&previous)?;
        fs::create_dir_all(&worktree)?;
        let projects = root.path().join("projects");
        let current_bucket = projects.join(encode(&worktree));
        let previous_bucket = projects.join(encode(&previous));
        fs::create_dir_all(&current_bucket)?;
        fs::create_dir_all(&previous_bucket)?;
        let file = "moved.jsonl";
        write_rows(
            &current_bucket.join(file),
            &[
                transcript(&parent, json!(1)),
                json!({"type": "relocated", "relocatedCwd": previous, "sessionId": "moved"}),
                json!({"cwd": previous, "timestamp": 2, "message": {
                    "role": "assistant", "content": "Previous worktree reply"
                }}),
                json!({"type": "worktree-state", "worktreeSession": {"worktreePath": worktree}}),
            ],
        )?;
        fs::copy(current_bucket.join(file), previous_bucket.join(file))?;

        let cwd = worktree.to_str().unwrap();
        let entries = read(Some(&projects), cwd, Some("moved")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_count, 2);
        let history = detail(Some(&projects), cwd, "moved", None, None).unwrap();
        assert_eq!(history.page.total, 2);
        assert_eq!(history.events[0].text, "First   question");
        assert_eq!(history.events[1].text, "Previous worktree reply");
        let replies = recent_replies(Some(&projects), cwd, "moved").unwrap();
        assert_eq!(replies.cwd, cwd);
        assert_eq!(replies.events.len(), 2);
        assert_eq!(replies.events[0].text, "First question");
        assert_eq!(replies.events[1].text, "Previous worktree reply");

        let previous = previous.to_str().unwrap();
        assert!(
            read(Some(&projects), previous, Some("moved"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            detail(Some(&projects), previous, "moved", None, None).unwrap_err(),
            SESSION_NOT_FOUND
        );
        assert_eq!(
            recent_replies(Some(&projects), previous, "moved").unwrap_err(),
            SESSION_NOT_FOUND
        );
        Ok(())
    }

    #[test]
    fn metadata_prefers_custom_then_auto_and_skips_bad_lines() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path();
        let projects = cwd.join("projects");
        let bucket = projects.join(encode(cwd));
        fs::create_dir_all(&bucket)?;
        let path = bucket.join("valid.jsonl");
        let prompt = "中".repeat(73);
        let rows = [
            json!({"cwd": cwd, "timestamp": "2026-10-30T17:01:11.012+01:00",
                "message": {"role": "user", "content": prompt}}),
            json!({"cwd": "/different/later", "timestamp": "2026-10-30T16:02:00.013Z",
                "message": {"role": "assistant", "model": "test-model", "permissionMode": "acceptEdits",
                    "content": [{"type": "tool_use", "name": "Read", "input": {}}]}}),
            json!({"message": {"role": "assistant", "content": [{"type": "thinking", "thinking": "ignored"}]}}),
        ];
        write_rows(&path, &rows)?;
        let list = || read(Some(&projects), cwd.to_str().unwrap(), None).unwrap();
        assert_eq!(list()[0].title, format!("{}…", "中".repeat(71)));
        let mut rows = rows.to_vec();
        rows.push(json!({"type": "ai-title", "aiTitle": "  Generated  "}));
        write_rows(&path, &rows)?;
        assert_eq!(list()[0].title, "Generated");
        use std::io::Write;
        let mut append = fs::OpenOptions::new().append(true).open(&path)?;
        append.write_all(b"\n{bad\n")?;
        io::copy(&mut io::repeat(b'x').take(MAX_LINE_BYTES + 9), &mut append)?;
        writeln!(
            append,
            "\n{}",
            json!({"type": "custom-title", "customTitle": "  Chosen  "})
        )?;
        let mut skipped = Skipped::default();
        let dates = Connection::open_in_memory()?;
        let mut date = dates.prepare("SELECT ROUND(unixepoch(?1, 'subsec') * 1000)")?;
        let entry = index(
            File::open(&path)?,
            "valid",
            &fs::canonicalize(cwd)?,
            &mut date,
            &mut skipped,
        )?
        .unwrap();
        assert_eq!(entry.title, "Chosen");
        assert_eq!(entry.model.as_deref(), Some("test-model"));
        assert_eq!(entry.permission_mode, "acceptEdits");
        assert_eq!(entry.updated_at, 1_793_376_120_013);
        assert_eq!(entry.message_count, 2);
        assert_eq!(skipped.malformed_lines, 1);
        assert_eq!(skipped.oversized_lines, 1);
        Ok(())
    }

    #[test]
    fn deferred_card_filters_dialogue_and_keeps_full_history() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let workspace = root.path();
        let cwd = workspace.to_str().unwrap();
        let projects = workspace.join("projects");
        let bucket = projects.join(encode(workspace));
        fs::create_dir_all(&bucket)?;
        let path = bucket.join("saved.jsonl");
        let long_reply = format!("{}终", "🦀".repeat(120));
        let rows = [
            json!({"cwd": cwd, "message": {"role": "user", "content": "  Launching skill: example"}}),
            json!({"message": {"role": "user", "content": "<command-name>/clear</command-name><command-args></command-args>"}}),
            json!({"message": {"role": "user", "content": [
                {"type": "text", "text": "Goal check-in: synthetic"},
                {"type": "text", "text": "<task-notification>background</task-notification>"},
                {"type": "text", "text": "<command-name>/review</command-name><command-args>[Image #1] 核对\n  这个结果</command-args>"}
            ]}}),
            json!({"message": {"role": "assistant", "content": "No response requested."}}),
            json!({"message": {"role": "user", "content": "请继续 <system-reminder>内部数据</system-reminder> 查看"}}),
            json!({"message": {"role": "assistant", "content": "All credentials for model isolated-probe are unavailable"}}),
            json!({"message": {"role": "assistant", "content": "这是回复"}}),
            json!({"message": {"role": "user", "content": "接着看<agent-message>未闭合包装"}}),
            json!({"timestamp": 1770000000000_i64, "message": {"role": "assistant", "content": long_reply}}),
            json!({"message": {"role": "assistant", "content": [{"type": "tool_use", "name": "Read", "input": {}}]}}),
        ];
        write_rows(&path, &rows)?;
        let before = fs::read(&path)?;
        let list = read(Some(&projects), cwd, None).unwrap();
        assert_eq!(list[0].title, "核对 这个结果");
        let preview = recent_replies(Some(&projects), cwd, "saved").unwrap();
        assert_eq!(preview.title.as_deref(), Some("核对 这个结果"));
        assert_eq!(preview.cwd, cwd);
        assert!(
            preview
                .updated_label
                .as_deref()
                .is_some_and(|label| label.contains('/'))
        );
        let messages: Vec<_> = preview
            .events
            .iter()
            .map(|event| event.text.as_str())
            .collect();
        assert_eq!(&messages[..3], ["请继续 查看", "这是回复", "接着看"]);
        assert_eq!(messages[3], format!("{}…", "🦀".repeat(120)));
        assert_eq!(preview.events[3].timestamp, Some(json!(1770000000000_i64)));
        assert_eq!(preview.events[3].role.as_deref(), Some("assistant"));
        assert_eq!(preview.events[3].kind, "assistant");
        let full = detail(Some(&projects), cwd, "saved", None, None).unwrap();
        assert_eq!(full.events.len(), rows.len());
        assert!(
            full.events
                .iter()
                .any(|event| event.text == "No response requested.")
        );
        assert_eq!(fs::read(&path)?, before);
        Ok(())
    }

    #[test]
    fn recent_card_falls_back_to_readable_events_only_within_the_last_200() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path().to_str().unwrap();
        let bucket = root.path().join(encode(root.path()));
        fs::create_dir_all(&bucket)?;
        let mut rows =
            vec![json!({"cwd": cwd, "message": {"role": "user", "content": "old prompt"}})];
        rows.extend(
            (1..=200)
                .map(|n| json!({"message": {"role": "system", "content": format!("event {n}")}})),
        );
        rows.push(json!({"message": {"role": "assistant", "content": "API Error: ignore"}}));
        write_rows(&bucket.join("events.jsonl"), &rows)?;
        let preview = recent_replies(Some(root.path()), cwd, "events").unwrap();
        assert_eq!(preview.title.as_deref(), Some("old prompt"));
        let messages: Vec<_> = preview
            .events
            .iter()
            .map(|event| event.text.as_str())
            .collect();
        assert_eq!(
            messages,
            ["event 197", "event 198", "event 199", "event 200"]
        );
        write_rows(&bucket.join("empty.jsonl"), &[json!({"cwd": cwd})])?;
        assert!(
            recent_replies(Some(root.path()), cwd, "empty")
                .unwrap()
                .events
                .is_empty()
        );
        assert!(recent_replies(Some(root.path()), cwd, "missing").is_err());
        assert!(recent_replies(Some(root.path()), cwd, "../events").is_err());
        assert_eq!(
            deferred_summary_text(" <task-notification>noise</task-notification> summary "),
            "summary"
        );
        assert_eq!(
            deferred_summary_text("automatic compaction failed: noise"),
            ""
        );
        Ok(())
    }

    #[test]
    fn title_cleanup_applies_to_each_user_text_block() {
        for noise in SYNTHETIC_USER_PREFIXES {
            assert!(compact_title(&json!(format!("{noise} x"))).is_empty());
        }
        for tag in SYNTHETIC_TAGS {
            assert_eq!(
                compact_title(&json!(format!(
                    "<{tag} attr=\"test\">hidden</{tag}> actual"
                ))),
                "actual"
            );
            assert_eq!(
                sanitize_reply("user", &format!("actual <{tag}>hidden")),
                "actual"
            );
        }
        assert_eq!(
            compact_title(&json!("<local-command-stdout>unterminated")),
            ""
        );
        assert_eq!(compact_title(&json!("<task-notification>unterminated")), "");
        assert_eq!(
            compact_title(&json!("<system-reminder>retained by catalog")),
            "<system-reminder>retained by catalog"
        );
        assert_eq!(
            compact_title(&json!("[image #1] [Image #14] first prompt")),
            "first prompt"
        );
        assert_eq!(
            compact_title(
                &json!([{"type":"text","text":"   one "}, {"type":"text","text":"  two"}])
            ),
            "one two"
        );
        for prefix in SYNTHETIC_ASSISTANT_PREFIXES {
            for kind in ["assistant", "other"] {
                assert!(sanitize_reply(kind, &format!("{prefix} test")).is_empty());
            }
        }
    }

    #[test]
    fn preview_uses_the_frozen_message_shapes_and_keeps_long_text() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path();
        let projects = cwd.join("projects");
        let bucket = projects.join(encode(cwd));
        fs::create_dir_all(&bucket)?;
        let body = format!("{}\n  尾行 🚀", "原文".repeat(9000));
        write_rows(
            &bucket.join("detail.jsonl"),
            &[
                json!({"type": "summary", "summary": "metadata", "cwd": cwd}),
                transcript(cwd, json!("2026-10-30T17:01:11.012+01:00")),
                json!({"message": {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "ignored"},
                    {"type": "text", "text": body}
                ]}, "timestamp": 1234.5}),
                json!({"cwd": "/different/later", "message": {"role": "assistant", "content": [
                    {"type": "text", "text": "Checking"},
                    {"type": "tool_use", "name": "Read", "input": {"z": 1, "a": "中"}},
                    {"type": "tool_use", "name": "Second", "input": {}}
                ]}}),
                json!({"message": {"role": "user", "content": [
                    {"type": "tool_result", "content": [{"type": "text", "text": "file contents"}]}
                ]}}),
                json!({"type": "system", "message": {"role": "system", "content": "  Ready  "}}),
                json!({"type": "system", "content": "metadata without a message"}),
                json!({"message": {"role": "assistant", "content": [{"type": "thinking"}]}}),
                json!({"message": {"role": "user", "content": " \n "}}),
            ],
        )?;
        let actual = detail(Some(&projects), cwd.to_str().unwrap(), "detail", None, None).unwrap();
        assert_eq!(
            serde_json::to_value(actual.events)?,
            json!([
                {"index": 1, "kind": "user", "role": "user", "timestamp": 1793376071012_i64,
                    "text": "First   question", "matched": false},
                {"index": 2, "kind": "assistant", "role": "assistant", "timestamp": 1234.5,
                    "text": body, "matched": false},
                {"index": 3, "kind": "tool", "role": "assistant", "toolName": "Read",
                    "text": "Checking\nRead {\"z\":1,\"a\":\"中\"}\nSecond {}", "matched": false},
                {"index": 4, "kind": "tool", "role": "user", "toolName": "tool_result",
                    "text": "[{\"type\":\"text\",\"text\":\"file contents\"}]", "matched": false},
                {"index": 5, "kind": "system", "text": "Ready", "matched": false}
            ])
        );
        assert_eq!(actual.page.total, 5);
        assert_eq!(
            read(Some(&projects), cwd.to_str().unwrap(), None).unwrap()[0].message_count,
            5
        );
        Ok(())
    }

    #[test]
    fn preview_pages_are_bounded_stable_and_read_only() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path();
        let projects = cwd.join("projects");
        let bucket = projects.join(encode(cwd));
        fs::create_dir_all(&bucket)?;
        let path = bucket.join("detail.jsonl");
        write_rows(
            &path,
            &(1..=510)
                .map(|index| transcript(cwd, json!(index)))
                .collect::<Vec<_>>(),
        )?;
        fs::write(bucket.join("empty.jsonl"), "")?;
        write_rows(&bucket.join("metadata.jsonl"), &[json!({"cwd": cwd})])?;
        let before = (fs::read(&path)?, fs::metadata(&path)?.modified()?);
        let read_page = |start, limit| {
            detail(
                Some(&projects),
                cwd.to_str().unwrap(),
                "detail",
                start,
                limit,
            )
            .unwrap()
        };
        for (start, limit, first, end) in [
            (Some(-1), Some(0), 1, 2),
            (Some(1), Some(25), 1, 26),
            (Some(250), Some(25), 250, 275),
            (Some(500), Some(25), 500, 511),
            (Some(i64::MAX), Some(25), 510, 511),
            (None, None, 311, 511),
            (None, Some(800), 11, 511),
        ] {
            let page = read_page(start, limit);
            assert_eq!(
                page.page,
                EventPage {
                    start_event_index: first,
                    end_event_index: end,
                    total: 510,
                    has_earlier: first > 1,
                    has_later: end <= 510,
                }
            );
            assert!(page.events.iter().map(|event| event.index).eq(first..end));
        }
        std::thread::scope(|scope| {
            let other = scope.spawn(|| read_page(Some(240), None));
            assert_eq!(read_page(Some(240), None), other.join().unwrap());
        });
        for id in ["empty", "metadata"] {
            assert_eq!(
                detail(Some(&projects), cwd.to_str().unwrap(), id, Some(900), None).unwrap(),
                SessionDetail {
                    events: Vec::new(),
                    page: EventPage {
                        start_event_index: 0,
                        end_event_index: 0,
                        total: 0,
                        has_earlier: false,
                        has_later: false
                    },
                }
            );
        }
        assert_eq!(before, (fs::read(&path)?, fs::metadata(&path)?.modified()?));
        Ok(())
    }

    #[test]
    fn latest_preview_carries_compaction_without_changing_the_event_window() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path();
        let projects = cwd.join("projects");
        let bucket = projects.join(encode(cwd));
        fs::create_dir_all(&bucket)?;
        write_rows(
            &bucket.join("compacted.jsonl"),
            &[
                transcript(cwd, json!("older")),
                json!({"type": "system", "subtype": "compact_boundary", "timestamp": 1000}),
                json!({"cwd": cwd, "isCompactSummary": true,
                    "message": {"role": "user", "content": "Summary text"}}),
            ],
        )?;
        let read = |start| {
            detail(
                Some(&projects),
                cwd.to_str().unwrap(),
                "compacted",
                start,
                Some(1),
            )
        };
        let latest = read(None).map_err(anyhow::Error::msg)?;
        assert_eq!(latest.page.start_event_index, 2);
        assert_eq!(latest.page.total, 2);
        assert_eq!(latest.events[0].text, "Summary text");
        assert_eq!(
            serde_json::to_value(&latest.events[0])?["isCompactSummary"],
            true
        );
        assert_eq!(latest.events[1].text, "compact_boundary");
        assert!(!latest.events[1].is_compact_summary);
        assert_eq!(latest.events[1].timestamp, Some(json!(1000)));
        let earlier = read(Some(1)).map_err(anyhow::Error::msg)?;
        assert_eq!(earlier.events.len(), 1);
        assert!(earlier.page.has_later);
        Ok(())
    }

    #[test]
    fn preview_skips_invalid_rows_and_accepts_the_exact_line_limit() -> anyhow::Result<()> {
        use std::io::Write;

        let root = tempfile::tempdir()?;
        let path = root.path().join("detail.jsonl");
        let mut file = File::create(&path)?;
        let first = transcript(root.path(), json!(1)).to_string();
        writeln!(file, "{first}")?;
        writeln!(file, "not-json\n42")?;
        // A record exactly 8 MiB excluding CRLF remains readable.
        io::copy(
            &mut io::repeat(b' ').take(MAX_LINE_BYTES - first.len() as u64),
            &mut file,
        )?;
        write!(file, "{first}\r\n")?;
        io::copy(&mut io::repeat(b' ').take(MAX_LINE_BYTES), &mut file)?;
        writeln!(file, "{first}")?;
        write!(file, "{}", transcript(root.path(), json!(3)))?;
        let workspace = fs::canonicalize(root.path())?;
        let dates = Connection::open_in_memory()?;
        let mut timestamp = dates.prepare(TIMESTAMP_SQL)?;
        let mut skipped = Skipped::default();
        let page = preview_page(
            BufReader::new(File::open(&path)?),
            &workspace,
            None,
            2,
            &mut timestamp,
            &mut skipped,
        )?
        .unwrap();
        assert_eq!(page.page.total, 3);
        assert_eq!(page.page.start_event_index, 2);
        assert_eq!(page.page.end_event_index, 4);
        assert_eq!(page.events[1].timestamp, Some(json!(3)));
        assert_eq!(skipped.malformed_lines, 2);
        assert_eq!(skipped.oversized_lines, 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scans_alias_and_canonical_buckets_without_following_escapes() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let workspace = root.path().join("actual 工作");
        let alias = root.path().join("alias here");
        fs::create_dir(&workspace)?;
        symlink(&workspace, &alias)?;
        let projects = root.path().join("projects");
        let alias_bucket = projects.join(encode(&alias));
        let canonical_bucket = projects.join(encode(&fs::canonicalize(&workspace)?));
        fs::create_dir_all(&alias_bucket)?;
        fs::create_dir_all(&canonical_bucket)?;
        write_rows(
            &alias_bucket.join("via-alias.jsonl"),
            &[transcript(&workspace, json!(1))],
        )?;
        write_rows(
            &canonical_bucket.join("via-real.jsonl"),
            &[transcript(&alias, json!(2))],
        )?;
        let outside = root.path().join("outside");
        fs::create_dir(&outside)?;
        write_rows(
            &outside.join("secret.jsonl"),
            &[transcript(&alias, json!(3))],
        )?;
        symlink(
            outside.join("secret.jsonl"),
            alias_bucket.join("escaping-file.jsonl"),
        )?;
        let mut skipped = Skipped::default();
        let projects = fs::canonicalize(projects)?;
        let real = fs::canonicalize(&workspace)?;
        let entries =
            scan(&projects, &alias, &real, None, &mut skipped).map_err(anyhow::Error::msg)?;
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.provider_session_id.as_str())
                .collect::<Vec<_>>(),
            ["via-real", "via-alias"]
        );
        assert_eq!(skipped.outside_root, 1);
        let detail_by_alias = |id| detail(Some(&projects), alias.to_str().unwrap(), id, None, None);
        assert_eq!(detail_by_alias("via-alias").unwrap().page.total, 1);
        assert_eq!(detail_by_alias("via-real").unwrap().page.total, 1);
        assert_eq!(
            detail_by_alias("escaping-file").unwrap_err(),
            SESSION_NOT_FOUND
        );
        assert!(detail_by_alias("../secret").is_err());
        write_rows(
            &alias_bucket.join("wrong-origin.jsonl"),
            &[transcript(&outside, json!(4))],
        )?;
        assert_eq!(
            detail_by_alias("wrong-origin").unwrap_err(),
            SESSION_NOT_FOUND
        );
        let candidate = alias_bucket.join("via-alias.jsonl");
        let (opened, path) = open_transcript(&projects, &candidate, &mut skipped)?.unwrap();
        fs::rename(&candidate, alias_bucket.join("old.jsonl"))?;
        write_rows(&candidate, &[transcript(&workspace, json!(5))])?;
        assert!(!recheck_transcript(
            &projects,
            &candidate,
            &path,
            &opened,
            &mut skipped
        )?);
        assert_eq!(skipped.outside_root, 2);
        fs::rename(&alias_bucket, projects.join("hidden-alias"))?;
        symlink(&outside, &alias_bucket)?;
        let entries =
            scan(&projects, &alias, &real, None, &mut skipped).map_err(anyhow::Error::msg)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].provider_session_id, "via-real");
        assert_eq!(skipped.outside_root, 3);
        Ok(())
    }
}
