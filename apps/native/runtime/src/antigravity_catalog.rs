//! Read Antigravity CLI conversation metadata and transcript previews.

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    fs::{self, File, Metadata},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    ops::Range,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use crate::{
    antigravity_events::compact,
    antigravity_launch::{LaunchMode, env_value, valid_conversation_id},
    dsh_events::js_whitespace,
};

const MAX_LINE: u64 = 8 * 1024 * 1024;
const METADATA: &str = "cache/conversation_metadata.json";
const SUMMARIES: &str = "conversation_summaries.db";
const NOT_FOUND: &str = "Antigravity 会话不存在或不属于当前工作空间";
const INVALID_ID: &str = "Antigravity 会话标识格式错误";
const INDEX_CACHE_LIMIT: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchScope {
    Metadata,
    #[default]
    All,
}

#[derive(Default)]
pub struct ListOptions<'a> {
    pub query: &'a str,
    pub offset: i64,
    pub limit: Option<i64>,
    pub scope: SearchScope,
    pub title_for_session: Option<&'a dyn Fn(&str) -> Option<String>>,
}

#[derive(Default)]
pub struct DetailOptions<'a> {
    pub query: &'a str,
    pub limit: Option<i64>,
    pub before_event_index: Option<i64>,
    pub around_event_index: Option<i64>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub provider: &'static str,
    pub provider_session_id: String,
    pub title: String,
    pub cwd: PathBuf,
    pub updated_at: serde_json::Number,
    pub permission_mode: &'static str,
    pub event_count: usize,
    pub match_count: usize,
    pub hits: Vec<Hit>,
    pub availability: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct List {
    pub provider: &'static str,
    pub sessions: Vec<Summary>,
    #[serde(flatten)]
    pub page: Page,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub next_offset: usize,
    pub has_more: bool,
}

impl Page {
    fn new(total: usize, offset: i64, limit: usize) -> Self {
        let offset = position(offset, total);
        let next_offset = offset.saturating_add(limit).min(total);
        Self {
            total,
            offset,
            limit,
            next_offset,
            has_more: next_offset < total,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub event_index: usize,
    pub kind: EventKind,
    pub excerpt: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub index: usize,
    pub kind: EventKind,
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<serde_json::Number>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub matched: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    #[serde(flatten)]
    pub summary: Summary,
    pub events: Vec<Preview>,
    pub page: EventPage,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub start_event_index: usize,
    pub end_event_index: usize,
    pub total: usize,
    pub has_earlier: bool,
    pub has_later: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Search {
    pub query: String,
    pub hits: Vec<Hit>,
    #[serde(flatten)]
    pub page: Page,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryCandidate {
    Unavailable,
    AwaitingIdentity(LaunchMode),
}

#[derive(Clone)]
struct Conversation {
    id: String,
    title: String,
    auto_title: Option<String>,
    updated_at: serde_json::Number,
    step_count: usize,
    workspace_uris: Vec<String>,
}

#[derive(PartialEq, Eq)]
struct FileVersion {
    size: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    file_id: (u64, u64, i64, i64),
}

impl FileVersion {
    fn read(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            file_id: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        }
    }
}

struct EventRef {
    offset: u64,
    length: usize,
}

struct Index {
    version: FileVersion,
    events: Vec<EventRef>,
}

pub struct Catalog {
    root: PathBuf,
    metadata: Option<(FileVersion, Vec<Conversation>)>,
    indexes: HashMap<String, Index>,
    english: bool,
}

pub fn catalog_root(home: &Path, environment: &BTreeMap<OsString, OsString>) -> PathBuf {
    env_value(environment, "MATOU_ANTIGRAVITY_HOME")
        .and_then(|path| path.to_str())
        .map(|path| path.trim_matches(js_whitespace))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".gemini/antigravity-cli"))
}

impl Catalog {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            root: std::path::absolute(&root).unwrap_or(root),
            metadata: None,
            indexes: HashMap::new(),
            english: false,
        }
    }

    pub fn with_english_titles(mut self) -> Self {
        self.english = true;
        self.metadata = None;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn list(&mut self, cwd: &Path, options: ListOptions<'_>) -> Result<List> {
        let (cwd, conversations) = self.workspace(cwd)?;
        let query = normalize(options.query);
        let mut sessions = Vec::new();
        for mut entry in conversations {
            if let Some(title) = options
                .title_for_session
                .and_then(|title| title(&entry.id))
                .filter(|title| !title.is_empty())
            {
                entry.title = title;
            }
            let (hits, count) = if !query.is_empty() && options.scope == SearchScope::All {
                self.search_transcript(&entry.id, &query, 0, 4)?
            } else {
                (Vec::new(), 0)
            };
            let metadata_matches = !query.is_empty()
                && normalize(&format!(
                    "{} {} {} antigravity",
                    entry.title,
                    entry.id,
                    cwd.display()
                ))
                .contains(&query);
            let count = count + usize::from(metadata_matches);
            if query.is_empty() || count != 0 {
                sessions.push(summary(entry, cwd.clone(), count, hits));
            }
        }
        sessions.sort_by(|left, right| {
            right
                .updated_at
                .as_f64()
                .partial_cmp(&left.updated_at.as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
                .then_with(|| right.title.cmp(&left.title))
        });
        let page = Page::new(
            sessions.len(),
            options.offset,
            options.limit.unwrap_or(50).clamp(1, 200) as usize,
        );
        let sessions = sessions.drain(page.offset..page.next_offset).collect();
        Ok(List {
            provider: "antigravity",
            sessions,
            page,
        })
    }

    pub fn detail(&mut self, cwd: &Path, id: &str, options: DetailOptions<'_>) -> Result<Detail> {
        let (cwd, entry) = self.selected(cwd, id)?;
        let query = normalize(options.query);
        let mut events = Vec::new();
        let mut hits = Vec::new();
        let mut count = 0;
        let mut total = 0;
        let mut window = 0..0;
        if let Some((mut file, index)) = self.transcript(id)? {
            total = index.events.len();
            window = event_window(total, &options);
            let dates = Connection::open_in_memory()?;
            if !query.is_empty() {
                (hits, count) = search_events(&mut file, &index.events, &dates, &query, 0, 4)?;
            }
            let mut line = Vec::new();
            for (at, reference) in index.events[window.clone()].iter().enumerate() {
                if let Some(mut event) = read_event(
                    &mut file,
                    &mut line,
                    reference,
                    window.start + at + 1,
                    &dates,
                )? {
                    event.matched = matches_query(&event, &query);
                    events.push(event);
                }
            }
        }
        let start = if window.is_empty() {
            0
        } else {
            window.start + 1
        };
        let end = if window.is_empty() { 0 } else { window.end };
        Ok(Detail {
            summary: summary(entry, cwd, count, hits),
            events,
            page: EventPage {
                start_event_index: start,
                end_event_index: end,
                total,
                has_earlier: start > 1,
                has_later: end > 0 && end < total,
            },
        })
    }

    pub fn search(
        &mut self,
        cwd: &Path,
        id: &str,
        query: &str,
        offset: i64,
        limit: Option<i64>,
    ) -> Result<Search> {
        self.selected(cwd, id)?;
        let query = normalize(query);
        let limit = limit.unwrap_or(100).clamp(1, 200) as usize;
        let (hits, total) =
            self.search_transcript(id, &query, position(offset, usize::MAX), limit)?;
        Ok(Search {
            query,
            hits,
            page: Page::new(total, offset, limit),
        })
    }

    pub fn auto_title(&mut self, cwd: &Path, id: &str) -> Result<Option<String>> {
        ensure!(valid_conversation_id(id), INVALID_ID);
        Ok(self
            .workspace(cwd)?
            .1
            .into_iter()
            .find(|entry| entry.id == id)
            .and_then(|entry| entry.auto_title))
    }

    /// A metadata match permits a targeted launch. The transcript can be
    /// absent or partial, and only the live run can confirm recovery.
    pub fn recovery_candidate(&mut self, cwd: &Path, id: &str) -> Result<RecoveryCandidate> {
        ensure!(valid_conversation_id(id), INVALID_ID);
        let belongs = self
            .workspace(cwd)?
            .1
            .into_iter()
            .any(|entry| entry.id == id);
        Ok(if belongs {
            RecoveryCandidate::AwaitingIdentity(LaunchMode::Resume(id.to_owned()))
        } else {
            RecoveryCandidate::Unavailable
        })
    }

    /// Observe a live title without requiring the workspace index to exist yet.
    pub fn latest_auto_title(&self, id: &str) -> Result<Option<String>> {
        if !valid_conversation_id(id) {
            return Ok(None);
        }
        let metadata = if let Some((file, _)) = open_contained(&self.root, Path::new(METADATA))? {
            serde_json::from_reader(BufReader::new(file)).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let entry = &metadata["conversations"][id];
        if is_internal(entry) {
            return Ok(None);
        }
        let database = safe_file_path(&self.root, Path::new(SUMMARIES))?;
        Ok(database
            .as_deref()
            .and_then(|path| database_title(path, id))
            .or_else(|| string(&entry["summary"]["Title"]).map(str::to_owned)))
    }

    fn workspace(&mut self, cwd: &Path) -> Result<(PathBuf, Vec<Conversation>)> {
        let cwd = canonical_path(cwd)?;
        let entries = self
            .metadata()?
            .iter()
            .filter(|entry| {
                entry
                    .workspace_uris
                    .iter()
                    .filter_map(|uri| workspace_path(uri))
                    .any(|path| canonical_path(&path).is_ok_and(|path| path == cwd))
            })
            .cloned()
            .collect();
        Ok((cwd, entries))
    }

    fn selected(&mut self, cwd: &Path, id: &str) -> Result<(PathBuf, Conversation)> {
        ensure!(valid_conversation_id(id), INVALID_ID);
        let (cwd, entries) = self.workspace(cwd)?;
        let entry = entries
            .into_iter()
            .find(|entry| entry.id == id)
            .context(NOT_FOUND)?;
        Ok((cwd, entry))
    }

    fn metadata(&mut self) -> Result<&[Conversation]> {
        let Some((file, version)) = open_contained(&self.root, Path::new(METADATA))? else {
            self.metadata = None;
            return Ok(&[]);
        };
        if !self
            .metadata
            .as_ref()
            .is_some_and(|(cached, _)| cached == &version)
        {
            let Ok(json) = serde_json::from_reader::<_, Value>(BufReader::new(file)) else {
                return Ok(&[]);
            };
            let dates = Connection::open_in_memory()?;
            let fallback = if self.english {
                "Untitled Antigravity session"
            } else {
                "未命名 Antigravity 会话"
            };
            let entries = json["conversations"]
                .as_object()
                .into_iter()
                .flat_map(|conversations| conversations.iter())
                .filter_map(|(id, entry)| conversation(&dates, id, entry, fallback))
                .collect();
            self.metadata = Some((version, entries));
        }
        Ok(self
            .metadata
            .as_ref()
            .map_or(&[], |(_, entries)| entries.as_slice()))
    }

    fn transcript(&mut self, id: &str) -> Result<Option<(File, &Index)>> {
        let path = Path::new("brain")
            .join(id)
            .join(".system_generated/logs/transcript.jsonl");
        let Some((mut file, version)) = open_contained(&self.root, &path)? else {
            self.indexes.remove(id);
            return Ok(None);
        };
        if !self
            .indexes
            .get(id)
            .is_some_and(|index| index.version == version)
        {
            let events = index_events(&mut file, version.size)?;
            if self.indexes.len() >= INDEX_CACHE_LIMIT {
                self.indexes.clear();
            }
            self.indexes
                .insert(id.to_owned(), Index { version, events });
        }
        Ok(Some((file, &self.indexes[id])))
    }

    fn search_transcript(
        &mut self,
        id: &str,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Hit>, usize)> {
        if !query.is_empty()
            && let Some((mut file, index)) = self.transcript(id)?
        {
            let dates = Connection::open_in_memory()?;
            return Ok(search_events(
                &mut file,
                &index.events,
                &dates,
                query,
                offset,
                limit,
            )?);
        }
        Ok((Vec::new(), 0))
    }
}

fn conversation(
    dates: &Connection,
    id: &str,
    entry: &Value,
    fallback: &str,
) -> Option<Conversation> {
    let data = &entry["summary"];
    if !valid_conversation_id(id) || is_internal(entry) || data["AppDataDir"] != "antigravity-cli" {
        return None;
    }
    let raw_title = string(&data["Title"]);
    Some(Conversation {
        id: id.into(),
        title: compact(
            raw_title
                .or_else(|| string(&data["Preview"]))
                .unwrap_or(fallback),
            72,
        ),
        auto_title: raw_title.map(str::to_owned),
        updated_at: timestamp(dates, &data["UpdatedAt"])
            .or_else(|| timestamp(dates, &entry["last_modified_time"]))
            .unwrap_or(0.into()),
        step_count: data["NumSteps"].as_f64().unwrap_or_default().max(0.0) as usize,
        workspace_uris: data["WorkspaceURIs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|uri| uri.as_str().map(str::to_owned))
            .collect(),
    })
}

fn is_internal(entry: &Value) -> bool {
    entry["is_internal"] == true || entry["summary"]["Internal"] == true
}

fn summary(entry: Conversation, cwd: PathBuf, match_count: usize, hits: Vec<Hit>) -> Summary {
    Summary {
        provider: "antigravity",
        provider_session_id: entry.id,
        title: entry.title,
        cwd,
        updated_at: entry.updated_at,
        permission_mode: "default",
        event_count: entry.step_count,
        match_count,
        hits,
        availability: "available",
    }
}

fn workspace_path(input: &str) -> Option<PathBuf> {
    let input = input.trim_matches(js_whitespace);
    if input.is_empty() {
        return None;
    }
    if input.starts_with("file://") {
        let url = Url::parse(input).ok()?;
        let lower = url.path().to_ascii_lowercase();
        if lower.contains("%2f") || cfg!(windows) && lower.contains("%5c") {
            return None;
        }
        let path = url.to_file_path().ok()?;
        return (!path.as_os_str().as_encoded_bytes().contains(&0)).then_some(path);
    }
    Some(input.into())
}

fn canonical_path(path: &Path) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(part),
        }
    }
    Ok(fs::canonicalize(&normalized).unwrap_or(normalized))
}

fn missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn safe_file_path(root: &Path, relative: &Path) -> io::Result<Option<PathBuf>> {
    let root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) if missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let path = root.join(relative);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let resolved = match fs::canonicalize(&path) {
        Ok(path) => path,
        Err(error) if missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(resolved.starts_with(root).then_some(resolved))
}

fn open_contained(root: &Path, relative: &Path) -> io::Result<Option<(File, FileVersion)>> {
    let Some(path) = safe_file_path(root, relative)? else {
        return Ok(None);
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || safe_file_path(root, relative)?.as_deref() != Some(&path) {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if missing(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        if (current.dev(), current.ino()) != (metadata.dev(), metadata.ino()) {
            return Ok(None);
        }
    }
    Ok(Some((file, FileVersion::read(&metadata))))
}

fn database_title(path: &Path, id: &str) -> Option<String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let connection = Connection::open_with_flags(path, flags).ok()?;
    connection.execute_batch("PRAGMA query_only = ON").ok()?;
    let title: String = connection
        .query_row(
            "SELECT title FROM conversation_summaries WHERE conversation_id = ?1
         AND (app_data_dir = '' OR app_data_dir = 'antigravity-cli') LIMIT 1",
            [id],
            |row| row.get(0),
        )
        .ok()?;
    let title = title.trim_matches(js_whitespace);
    (!title.is_empty()).then(|| title.to_owned())
}

fn index_events(file: &mut File, size: u64) -> io::Result<Vec<EventRef>> {
    let mut reader = BufReader::new(file.take(size));
    let mut events = Vec::new();
    let mut line = Vec::new();
    let mut offset = 0;
    loop {
        line.clear();
        let length = (&mut reader)
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut line)?;
        if length == 0 {
            return Ok(events);
        }
        if length as u64 > MAX_LINE {
            offset += length as u64;
            if !line.ends_with(b"\n") {
                offset += reader.skip_until(b'\n')? as u64;
            }
            continue;
        }
        if serde_json::from_slice::<Value>(&line).is_ok_and(|row| event_text(&row).is_some()) {
            events.push(EventRef { offset, length });
        }
        offset += length as u64;
    }
}

fn read_event(
    file: &mut File,
    line: &mut Vec<u8>,
    event: &EventRef,
    index: usize,
    dates: &Connection,
) -> io::Result<Option<Preview>> {
    file.seek(SeekFrom::Start(event.offset))?;
    line.resize(event.length, 0);
    match file.read_exact(line) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    Ok(serde_json::from_slice::<Value>(line).ok().and_then(|row| {
        let (kind, role, text, tool_name) = event_text(&row)?;
        Some(Preview {
            index,
            kind,
            role,
            timestamp: timestamp(dates, &row["created_at"])
                .or_else(|| timestamp(dates, &row["timestamp"])),
            text,
            tool_name,
            matched: false,
        })
    }))
}

fn event_text(row: &Value) -> Option<(EventKind, &'static str, String, Option<String>)> {
    if row["type"] == "USER_INPUT" || row["source"] == "USER_EXPLICIT" {
        return Some((
            EventKind::User,
            "user",
            string(&row["content"])?.into(),
            None,
        ));
    }
    if row["type"] != "PLANNER_RESPONSE" && row["source"] != "MODEL" {
        return None;
    }
    let mut parts: Vec<String> = string(&row["content"])
        .map(str::to_owned)
        .into_iter()
        .collect();
    let mut first_tool = None;
    for tool in row["tool_calls"].as_array().into_iter().flatten() {
        if string(&tool["name"]).is_none() {
            continue;
        }
        let name = tool["name"].as_str().expect("validated tool name");
        first_tool.get_or_insert_with(|| name.to_owned());
        let arguments = match tool.get("arguments") {
            Some(Value::String(text)) => text.trim_matches(js_whitespace).to_owned(),
            Some(value) => value.to_string(),
            None => String::new(),
        };
        parts.push(
            format!("{name} {arguments}")
                .trim_matches(js_whitespace)
                .to_owned(),
        );
    }
    if parts.is_empty() {
        return None;
    }
    let kind = if first_tool.is_some() {
        EventKind::Tool
    } else {
        EventKind::Assistant
    };
    Some((kind, "assistant", parts.join("\n"), first_tool))
}

fn timestamp(dates: &Connection, value: &Value) -> Option<serde_json::Number> {
    if let Some(number) = value.as_number() {
        return Some(number.clone());
    }
    let milliseconds: i64 = dates
        .prepare_cached("SELECT CAST(ROUND(unixepoch(?1, 'subsec') * 1000) AS INTEGER)")
        .ok()?
        .query_row([value.as_str()?], |row| row.get(0))
        .ok()?;
    Some(milliseconds.into())
}

fn string(value: &Value) -> Option<&str> {
    let value = value.as_str()?.trim_matches(js_whitespace);
    (!value.is_empty()).then_some(value)
}

fn normalize(text: &str) -> String {
    text.trim_matches(js_whitespace).to_lowercase()
}

fn position(value: i64, maximum: usize) -> usize {
    usize::try_from(value.max(0))
        .unwrap_or(usize::MAX)
        .min(maximum)
}

fn event_window(total: usize, options: &DetailOptions<'_>) -> Range<usize> {
    let limit = options.limit.unwrap_or(200).clamp(1, 500) as usize;
    if let Some(anchor) = options.around_event_index {
        let start = position(anchor, total).saturating_sub(1 + limit / 2);
        let end = (start + limit).min(total);
        return end.saturating_sub(limit)..end;
    }
    let end = options
        .before_event_index
        .map_or(total, |index| position(index.saturating_sub(1), total));
    end.saturating_sub(limit)..end
}

fn matches_query(event: &Preview, query: &str) -> bool {
    !query.is_empty()
        && normalize(&format!(
            "{} {}",
            event.tool_name.as_deref().unwrap_or_default(),
            event.text
        ))
        .contains(query)
}

fn search_events(
    file: &mut File,
    refs: &[EventRef],
    dates: &Connection,
    query: &str,
    offset: usize,
    limit: usize,
) -> io::Result<(Vec<Hit>, usize)> {
    let mut hits = Vec::new();
    let mut count = 0;
    let mut line = Vec::new();
    for (index, event) in refs.iter().enumerate() {
        if let Some(event) = read_event(file, &mut line, event, index + 1, dates)?
            && matches_query(&event, query)
        {
            if count >= offset && hits.len() < limit {
                hits.push(Hit {
                    event_index: event.index,
                    kind: event.kind,
                    excerpt: excerpt(&event.text, query),
                });
            }
            count += 1;
        }
    }
    Ok((hits, count))
}

fn excerpt(text: &str, query: &str) -> String {
    let flat = text
        .split(js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let units: Vec<_> = flat.encode_utf16().collect();
    let lower = flat.to_lowercase();
    let Some(at) = lower.find(query) else {
        return String::from_utf16_lossy(&units[..units.len().min(120)]);
    };
    let start = lower[..at]
        .encode_utf16()
        .count()
        .saturating_sub(36)
        .min(units.len());
    let end = (lower[..at + query.len()].encode_utf16().count() + 70).min(units.len());
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        String::from_utf16_lossy(&units[start..end]),
        if end < units.len() { "…" } else { "" }
    )
}
