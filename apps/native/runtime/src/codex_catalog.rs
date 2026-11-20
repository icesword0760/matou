//! Read the Codex catalog on the shared provider-catalog worker.
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, Metadata},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};

const MAX_LINE: u64 = 8 * 1024 * 1024;
const HEADER_CACHE_LIMIT: usize = 256;
const NOT_FOUND: &str = "Provider session was not found in this working directory";
const CORRUPT: &str = "Codex 会话文件已损坏";
const UNREADABLE: &str = "无法读取 Codex 会话文件";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryBase {
    thread_id: String,
    end_ordinal_exclusive: Option<u64>,
    end_byte_offset: Option<u64>,
}

#[derive(Clone)]
struct Header {
    root: PathBuf,
    path: PathBuf,
    id: String,
    cwd: PathBuf,
    updated_at: i64,
    modified: SystemTime,
    size: u64,
    forked_from_id: Option<String>,
    history_base: Option<HistoryBase>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogEntry {
    pub provider_session_id: String,
    pub title: String,
    pub model: Option<String>,
    pub permission_mode: &'static str,
    pub has_completed_turn: bool,
    cwd: PathBuf,
    updated_at: i64,
    message_count: usize,
    match_count: usize,
    hits: Vec<Hit>,
    forked_from_id: Option<String>,
    history_base: Option<HistoryBase>,
    pub loaded_by: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page<T> {
    #[serde(flatten)]
    pub window: Window,
    pub items: Vec<T>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Window {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub next_offset: usize,
    pub has_more: bool,
}

impl Window {
    fn new(total: usize, offset: usize, limit: usize) -> Self {
        let offset = offset.min(total);
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Hit {
    event_index: usize,
    kind: &'static str,
    excerpt: String,
}

#[derive(Default)]
pub(crate) struct ListOptions {
    pub query: String,
    pub full_text: bool,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Default)]
struct Turn {
    user: bool,
    final_answer: bool,
    failed: bool,
}

struct EventRef {
    offset: u64,
    length: usize,
    kind: &'static str,
}

struct Candidate {
    event: EventRef,
    id: Option<String>,
    turn: String,
    structured: bool,
}

#[derive(Default)]
struct Index {
    events: Vec<EventRef>,
    model: Option<String>,
    has_completed_turn: bool,
}

pub(crate) struct Catalog {
    root: Option<PathBuf>,
    headers: HashMap<PathBuf, Header>,
    index: Option<(Header, Index)>,
}

impl Catalog {
    pub(crate) fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            headers: HashMap::new(),
            index: None,
        }
    }

    pub(crate) fn list(
        &mut self,
        cwd: &str,
        options: &ListOptions,
    ) -> Result<Page<CatalogEntry>, &'static str> {
        let db = dates()?;
        let headers = self.headers(cwd, &db)?;
        let titles = self.titles(cwd, &db)?;
        let query = normalized(&options.query);
        let mut entries = Vec::new();
        for header in headers {
            let mut entry = entry(&header, &titles, &db);
            let metadata_match = !query.is_empty()
                && normalized(&format!("{} {}", entry.title, entry.provider_session_id))
                    .contains(&query);
            if !query.is_empty() && options.full_text {
                let (window, hits) = self.search_header(&header, &query, 0, 4)?;
                entry.match_count = window.total;
                entry.hits = hits;
            }
            entry.match_count += usize::from(metadata_match);
            if query.is_empty() || entry.match_count > 0 {
                entries.push(entry);
            }
        }
        entries.sort_unstable_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.provider_session_id.cmp(&b.provider_session_id))
        });
        let window = Window::new(
            entries.len(),
            offset(options.offset),
            limit(options.limit, 50, 200),
        );
        let items = entries.drain(window.offset..window.next_offset).collect();
        Ok(Page { window, items })
    }

    pub(crate) fn load(&mut self, cwd: &str, id: &str) -> Result<CatalogEntry, &'static str> {
        let db = dates()?;
        let header = self.selected(cwd, id, &db)?;
        let mut entry = entry(&header, &self.titles(cwd, &db)?, &db);
        let (_, index) = self.open_index(&header)?;
        entry.model.clone_from(&index.model);
        entry.has_completed_turn = index.has_completed_turn;
        Ok(entry)
    }

    pub(crate) fn detail(
        &mut self,
        cwd: &str,
        id: &str,
        start: Option<i64>,
        around: Option<i64>,
        max: Option<i64>,
    ) -> Result<Value, &'static str> {
        let db = dates()?;
        let header = self.selected(cwd, id, &db)?;
        let (mut file, index) = self.open_index(&header)?;
        let total = index.events.len();
        let limit = limit(max, 200, 500);
        let newest = total.saturating_sub(limit);
        let start = if let Some(around) = around {
            offset(Some(around.saturating_sub(1)))
                .saturating_sub(limit / 2)
                .min(newest)
        } else {
            start.map_or(newest, |start| {
                offset(Some(start.saturating_sub(1))).min(total)
            })
        };
        let end = start.saturating_add(limit).min(total);
        let mut events = Vec::with_capacity(end - start);
        for (position, event_ref) in index.events[start..end].iter().enumerate() {
            let row = read_event(&mut file, event_ref)?;
            let (kind, text) = visible(&row).ok_or(CORRUPT)?;
            events.push(json!({
                "index": start + position + 1, "kind": kind, "role": (kind != "tool").then_some(kind),
                "text": truncate(&text, 32_768), "timestamp": timestamp(&db, &row["timestamp"]),
                "toolName": (kind == "tool").then_some("Bash"), "matched": false,
            }));
        }
        let first_prompt = index
            .events
            .iter()
            .find(|event| event.kind == "user")
            .map(|event| read_event(&mut file, event))
            .transpose()?
            .and_then(|row| visible(&row))
            .map(|(_, text)| truncate(&text, 32_768).to_owned());
        Ok(json!({
            "model": index.model, "permissionMode": "default", "events": events,
            "firstPrompt": first_prompt,
            "page": {
                "startEventIndex": if start < end { start + 1 } else { 0 },
                "endEventIndex": if start < end { end } else { 0 },
                "total": total, "hasEarlier": start > 0, "hasLater": end < total,
            },
        }))
    }

    pub(crate) fn search(
        &mut self,
        cwd: &str,
        id: &str,
        query: &str,
        from: Option<i64>,
        max: Option<i64>,
    ) -> Result<Page<Hit>, &'static str> {
        let header = self.selected(cwd, id, &dates()?)?;
        let (window, items) = self.search_header(
            &header,
            &normalized(query),
            offset(from),
            limit(max, 100, 200),
        )?;
        Ok(Page { window, items })
    }

    fn search_header(
        &mut self,
        header: &Header,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Window, Vec<Hit>), &'static str> {
        let mut hits = Vec::new();
        let mut total = 0;
        if !query.is_empty() {
            let (mut file, index) = self.open_index(header)?;
            for (position, event_ref) in index.events.iter().enumerate() {
                let row = read_event(&mut file, event_ref)?;
                let (kind, text) = visible(&row).ok_or(CORRUPT)?;
                let tool = if kind == "tool" { "Bash " } else { "" };
                if normalized(&format!("{tool}{text}")).contains(query) {
                    if total >= offset && hits.len() < limit {
                        hits.push(Hit {
                            event_index: position + 1,
                            kind,
                            excerpt: excerpt(&text, query),
                        });
                    }
                    total += 1;
                }
            }
        }
        Ok((Window::new(total, offset, limit), hits))
    }

    fn selected(&mut self, cwd: &str, id: &str, db: &Connection) -> Result<Header, &'static str> {
        if uuid(id).is_none() {
            return Err("Invalid provider session ID");
        }
        self.headers(cwd, db)?
            .into_iter()
            .find(|header| header.id.eq_ignore_ascii_case(id))
            .ok_or(NOT_FOUND)
    }

    fn headers(&mut self, cwd: &str, dates: &Connection) -> Result<Vec<Header>, &'static str> {
        let root = sessions_root(self.effective_root(cwd).as_deref())?;
        let Some(root) = root else {
            return Ok(Vec::new());
        };
        let cwd = match fs::canonicalize(cwd) {
            Ok(path) if path.is_dir() => path,
            Ok(_) => return Ok(Vec::new()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(UNREADABLE),
        };
        let mut entries: HashMap<String, Header> = HashMap::new();
        scan_rollouts(&root, &root, &mut |path, file, metadata| {
            let modified = metadata.modified().map_err(|_| UNREADABLE)?;
            let size = metadata.len();
            let header = match self.headers.get(&path) {
                Some(cached) if cached.modified == modified && cached.size == size => {
                    cached.clone()
                }
                _ => {
                    let mut reader = BufReader::new(file.take(size));
                    let mut line = Vec::new();
                    loop {
                        if !read_line(&mut reader, &mut line)? {
                            return Err(CORRUPT);
                        }
                        if !line.iter().all(u8::is_ascii_whitespace) {
                            break;
                        }
                    }
                    let row: Value = serde_json::from_slice(&line).map_err(|_| CORRUPT)?;
                    if row["type"] != "session_meta" {
                        return Err(CORRUPT);
                    }
                    let payload = &row["payload"];
                    let id = payload["id"].as_str().and_then(uuid).ok_or(CORRUPT)?;
                    let raw_cwd = payload["cwd"]
                        .as_str()
                        .filter(|cwd| !cwd.is_empty())
                        .ok_or(CORRUPT)?;
                    let base = &payload["history_base"];
                    let history_base =
                        base["thread_id"]
                            .as_str()
                            .and_then(uuid)
                            .map(|thread_id| HistoryBase {
                                thread_id,
                                end_ordinal_exclusive: base["end_ordinal_exclusive"].as_u64(),
                                end_byte_offset: base["end_byte_offset"].as_u64(),
                            });
                    let header = Header {
                        root: root.clone(),
                        path: path.clone(),
                        id,
                        cwd: fs::canonicalize(raw_cwd).unwrap_or_else(|_| PathBuf::from(raw_cwd)),
                        updated_at: millis(modified)
                            .or_else(|| timestamp(dates, &payload["timestamp"]))
                            .unwrap_or_default(),
                        modified,
                        size,
                        forked_from_id: payload["forked_from_id"].as_str().and_then(uuid),
                        history_base,
                    };
                    if self.headers.len() == HEADER_CACHE_LIMIT
                        && !self.headers.contains_key(&path)
                        && let Some(key) = self.headers.keys().next().cloned()
                    {
                        self.headers.remove(&key);
                    }
                    self.headers.insert(path, header.clone());
                    header
                }
            };
            if header.cwd == cwd
                && entries.get(&header.id).is_none_or(|current| {
                    (header.modified, &header.path) > (current.modified, &current.path)
                })
            {
                entries.insert(header.id.clone(), header);
            }
            Ok(())
        })?;
        Ok(entries.into_values().collect())
    }

    fn effective_root(&self, cwd: &str) -> Option<PathBuf> {
        self.root.as_ref().map(|root| Path::new(cwd).join(root))
    }

    fn titles(
        &self,
        cwd: &str,
        dates: &Connection,
    ) -> Result<HashMap<String, (i64, String)>, &'static str> {
        let mut titles = HashMap::new();
        let Some(root) = self.effective_root(cwd) else {
            return Ok(titles);
        };
        let root = match fs::canonicalize(root) {
            Ok(root) => root,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(titles),
            Err(_) => return Err(UNREADABLE),
        };
        let Some((file, metadata)) = open_file(&root, &root.join("session_index.jsonl"))? else {
            return Ok(titles);
        };
        let mut reader = BufReader::new(file.take(metadata.len()));
        let mut line = Vec::new();
        loop {
            match read_line(&mut reader, &mut line) {
                Ok(false) => break,
                Err(CORRUPT) => {
                    reader.skip_until(b'\n').map_err(|_| UNREADABLE)?;
                    continue;
                }
                Err(error) => return Err(error),
                Ok(true) => {}
            }
            let Ok(row) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if let (Some(id), Some(title), Some(at)) = (
                row["id"].as_str().and_then(uuid),
                row["thread_name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty()),
                timestamp(dates, &row["updated_at"]),
            ) && titles.get(&id).is_none_or(|(previous, _)| at >= *previous)
            {
                titles.insert(id, (at, title.to_owned()));
            }
        }
        Ok(titles)
    }

    fn open_index(&mut self, header: &Header) -> Result<(File, &Index), &'static str> {
        let (mut file, meta) = open_file(&header.root, &header.path)?.ok_or(NOT_FOUND)?;
        let modified = meta.modified().map_err(|_| UNREADABLE)?;
        if self.index.as_ref().is_none_or(|(cached, _)| {
            cached.path != header.path || cached.modified != modified || cached.size != meta.len()
        }) {
            let index = index(&mut file, meta.len(), header)?;
            let mut current = header.clone();
            current.modified = modified;
            current.size = meta.len();
            self.index = Some((current, index));
        }
        Ok((file, &self.index.as_ref().expect("index initialized").1))
    }
}

pub(crate) fn root() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".codex")))
}

fn sessions_root(root: Option<&Path>) -> Result<Option<PathBuf>, &'static str> {
    let Some(root) = root else {
        return Ok(None);
    };
    let root = match fs::canonicalize(root) {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(UNREADABLE),
    };
    let sessions = root.join("sessions");
    match fs::canonicalize(&sessions) {
        Ok(path) if path.starts_with(&root) && path.is_dir() => Ok(Some(path)),
        Ok(_) => Err(UNREADABLE),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(UNREADABLE),
    }
}

fn scan_rollouts(
    root: &Path,
    directory: &Path,
    read: &mut impl FnMut(PathBuf, File, Metadata) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let mut directories = vec![directory.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for item in fs::read_dir(directory).map_err(|_| UNREADABLE)? {
            let item = item.map_err(|_| UNREADABLE)?;
            let kind = item.file_type().map_err(|_| UNREADABLE)?;
            let path = item.path();
            if kind.is_dir() {
                if fs::canonicalize(&path).is_ok_and(|path| path.starts_with(root)) {
                    directories.push(path);
                }
            } else if kind.is_file()
                && item
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
                && let Some((file, metadata)) = open_file(root, &path)?
            {
                read(path, file, metadata)?;
            }
        }
    }
    Ok(())
}

fn open_file(root: &Path, path: &Path) -> Result<Option<(File, Metadata)>, &'static str> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(UNREADABLE),
    };
    if !metadata.is_file() || !fs::canonicalize(path).is_ok_and(|path| path.starts_with(root)) {
        return Ok(None);
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(UNREADABLE),
    };
    let opened = file.metadata().map_err(|_| UNREADABLE)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (metadata.dev(), metadata.ino()) != (opened.dev(), opened.ino()) {
            return Err(UNREADABLE);
        }
    }
    if !opened.is_file() || !fs::canonicalize(path).is_ok_and(|path| path.starts_with(root)) {
        return Err(UNREADABLE);
    }
    Ok(Some((file, opened)))
}

fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> Result<bool, &'static str> {
    line.clear();
    let length = reader
        .take(MAX_LINE + 1)
        .read_until(b'\n', line)
        .map_err(|_| UNREADABLE)?;
    if length as u64 > MAX_LINE {
        return Err(CORRUPT);
    }
    Ok(length != 0)
}

fn dates() -> Result<Connection, &'static str> {
    Connection::open_in_memory().map_err(|_| UNREADABLE)
}

fn timestamp(dates: &Connection, value: &Value) -> Option<i64> {
    if let Some(at) = value.as_i64() {
        return Some(at);
    }
    dates
        .query_row(
            "SELECT CAST(ROUND(unixepoch(?1, 'subsec') * 1000) AS INTEGER)",
            [value.as_str()?],
            |row| row.get(0),
        )
        .ok()
        .flatten()
}

fn millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

fn uuid(value: &str) -> Option<String> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|_| value.len() == 36)
        .map(|id| id.to_string())
}

fn normalized(query: &str) -> String {
    query.trim().to_lowercase()
}

fn offset(value: Option<i64>) -> usize {
    value.unwrap_or(0).max(0).try_into().unwrap_or(usize::MAX)
}

fn limit(value: Option<i64>, default: i64, max: i64) -> usize {
    value.unwrap_or(default).clamp(1, max) as usize
}

fn entry(
    header: &Header,
    titles: &HashMap<String, (i64, String)>,
    dates: &Connection,
) -> CatalogEntry {
    let named = titles.get(&header.id);
    let title = named.map(|(_, title)| title.clone()).unwrap_or_else(|| {
        let at: String = dates
            .query_row(
                "SELECT strftime('%Y-%m-%d %H:%M', ?1 / 1000.0, 'unixepoch')",
                [header.updated_at],
                |row| row.get(0),
            )
            .unwrap_or_default();
        format!("{at} · {}", header.id)
    });
    CatalogEntry {
        provider_session_id: header.id.clone(),
        title,
        cwd: header.cwd.clone(),
        updated_at: named.map_or(header.updated_at, |(at, _)| header.updated_at.max(*at)),
        model: None,
        permission_mode: "default",
        has_completed_turn: false,
        message_count: 0,
        match_count: 0,
        hits: Vec::new(),
        forked_from_id: header.forked_from_id.clone(),
        history_base: header.history_base.clone(),
        loaded_by: None,
    }
}

fn index(file: &mut File, size: u64, header: &Header) -> Result<Index, &'static str> {
    let mut reader = BufReader::new(file.take(size));
    let mut line = Vec::new();
    let mut result = Index::default();
    let mut offset = 0;
    let mut current_turn = String::new();
    let mut turns = HashMap::<String, Turn>::new();
    let mut structured_ids = HashSet::new();
    let mut structured_users = HashSet::new();
    let mut checked_header = false;
    let mut candidates = Vec::new();
    while read_line(&mut reader, &mut line)? {
        let start = offset;
        offset += line.len() as u64;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row: Value = match serde_json::from_slice(&line) {
            Ok(row @ Value::Object(_)) => row,
            _ if !line.ends_with(b"\n") && offset == size => break,
            _ => return Err(CORRUPT),
        };
        let payload = &row["payload"];
        if !checked_header {
            if row["type"] != "session_meta"
                || payload["id"].as_str().and_then(uuid).as_ref() != Some(&header.id)
                || payload["cwd"].as_str().is_none_or(|cwd| {
                    fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd)) != header.cwd
                })
            {
                return Err(CORRUPT);
            }
            checked_header = true;
        }
        if row["type"] == "turn_context"
            && let Some(model) = payload["model"].as_str()
        {
            result.model = Some(model.to_owned());
        }
        if let Some(turn) = payload["turn_id"].as_str()
            && turn != current_turn
        {
            current_turn = turn.to_owned();
        }
        let structured = row["type"] == "event_msg" && payload["type"] == "item_completed";
        let item = if structured {
            &payload["item"]
        } else {
            payload
        };
        let event_kind = visible(&row).map(|(kind, _)| kind);
        if row["type"] == "event_msg" {
            let turn = turns.entry(current_turn.clone()).or_default();
            match payload["type"].as_str() {
                Some("turn_aborted" | "task_failed" | "error") => turn.failed = true,
                Some("item_completed") => {
                    turn.user |= event_kind == Some("user");
                    turn.final_answer |=
                        event_kind == Some("assistant") && item["phase"] == "final_answer";
                }
                Some("task_complete") => {
                    result.has_completed_turn |= turn.user
                        && turn.final_answer
                        && !turn.failed
                        && payload["error"].is_null();
                }
                _ => {}
            }
        }
        let Some(kind) = event_kind else {
            continue;
        };
        let id = item["id"]
            .as_str()
            .or_else(|| payload["id"].as_str())
            .map(str::to_owned);
        if structured {
            if let Some(id) = &id
                && !structured_ids.insert(id.clone())
            {
                continue;
            }
            if kind == "user" {
                structured_users.insert(current_turn.clone());
            }
        }
        candidates.push(Candidate {
            event: EventRef {
                offset: start,
                length: line.len(),
                kind,
            },
            id,
            turn: current_turn.clone(),
            structured,
        });
    }
    if !checked_header {
        return Err(CORRUPT);
    }
    let mut seen = HashSet::new();
    result.events = candidates
        .into_iter()
        .filter_map(|candidate| {
            if !candidate.structured
                && (candidate
                    .id
                    .as_ref()
                    .is_some_and(|id| structured_ids.contains(id))
                    || (candidate.event.kind == "user"
                        && structured_users.contains(&candidate.turn)))
            {
                return None;
            }
            candidate
                .id
                .is_none_or(|id| seen.insert(id))
                .then_some(candidate.event)
        })
        .collect();
    Ok(result)
}

fn read_event(file: &mut File, event: &EventRef) -> Result<Value, &'static str> {
    let mut line = vec![0; event.length];
    file.seek(SeekFrom::Start(event.offset))
        .and_then(|_| file.read_exact(&mut line))
        .map_err(|_| UNREADABLE)?;
    serde_json::from_slice(&line).map_err(|_| CORRUPT)
}

fn visible(row: &Value) -> Option<(&'static str, String)> {
    let payload = &row["payload"];
    let structured = row["type"] == "event_msg" && payload["type"] == "item_completed";
    let (kind, item) = if structured {
        let item = &payload["item"];
        let kind = match item["type"].as_str()? {
            "UserMessage" => "user",
            "AgentMessage" => "assistant",
            "CommandExecution" => "tool",
            _ => return None,
        };
        (kind, item)
    } else if row["type"] == "response_item" && payload["type"] == "message" {
        let kind = match payload["role"].as_str()? {
            "user" => "user",
            "assistant" => "assistant",
            _ => return None,
        };
        (kind, payload)
    } else {
        return None;
    };
    let text = if kind == "tool" {
        let cmd = item["command"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                item["command"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        let out = item["aggregated_output"].as_str().unwrap_or_default();
        [cmd.as_str(), out]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        item["content"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                item["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    };
    if text.is_empty()
        || (kind == "user"
            && [
                "# AGENTS.md instructions",
                "<environment_context>",
                "<permissions instructions>",
            ]
            .iter()
            .any(|prefix| text.trim_start().starts_with(prefix)))
    {
        return None;
    }
    Some((kind, text))
}

fn truncate(text: &str, limit: usize) -> &str {
    &text[..text.floor_char_boundary(limit.min(text.len()))]
}

fn excerpt(text: &str, query: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    let Some(position) = lower.find(query) else {
        return truncate(&flat, 120).to_owned();
    };
    let start = flat.floor_char_boundary(position.saturating_sub(36).min(flat.len()));
    let end = flat.ceil_char_boundary((position + query.len() + 70).min(flat.len()));
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        &flat[start..end],
        if end < flat.len() { "…" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = anyhow::Result<()>;

    trait CatalogResult<T> {
        fn checked(self) -> anyhow::Result<T>;
    }

    impl<T> CatalogResult<T> for Result<T, &'static str> {
        fn checked(self) -> anyhow::Result<T> {
            self.map_err(anyhow::Error::msg)
        }
    }

    fn fixture(
        session_root: &Path,
        cwd: &Path,
        id: &str,
        name: &str,
        extra: Value,
        rows: &[Value],
        mtime: u64,
    ) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(session_root)?;
        let path = session_root.join(format!("rollout-{name}-{id}.jsonl"));
        let mut payload = json!({"id": id, "cwd": cwd, "timestamp": "2026-01-01T00:00:00Z"});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut bodies = vec![json!({"type": "session_meta", "payload": payload}).to_string()];
        bodies.extend(rows.iter().map(Value::to_string));
        let file = File::create(&path)?;
        (&file).write_all(format!("\n{}\n", bodies.join("\n")).as_bytes())?;
        file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(mtime))?;
        Ok(path)
    }

    use std::io::Write;

    #[test]
    fn codex_catalog_pages_metadata_and_workspace_filter() -> TestResult {
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("工作区");
        let other = root.path().join("另一工作区");
        fs::create_dir_all(&cwd)?;
        fs::create_dir_all(&other)?;
        let home = root.path().join("Codex home");
        let dated = home.join("sessions/2026/10/01");
        let mut catalog = Catalog::new(Some(home.clone()));
        let missing = catalog
            .list(cwd.to_str().unwrap(), &ListOptions::default())
            .checked()?;
        assert!(missing.items.is_empty());
        for n in 1..=203 {
            let id = format!("00100000-0000-4000-8000-{n:012}");
            fixture(&dated, &cwd, &id, "original", json!({}), &[], 10)?;
        }
        let first = "00100000-0000-4000-8000-000000000001";
        let second = "00100000-0000-4000-8000-000000000002";
        let third = "00100000-0000-4000-8000-000000000003";
        fixture(
            &dated,
            &cwd,
            second,
            "newer duplicate",
            json!({ "forked_from_id": first }),
            &[],
            20,
        )?;
        fixture(&dated, &other, third, "wrong workspace", json!({}), &[], 30)?;
        fs::write(
            home.join("session_index.jsonl"),
            format!(
                "bad row\n{}\n{}\n{}\n{{\"id\":",
                json!({"id": first, "thread_name": "登录流程", "updated_at": "2026-01-12T15:23:20.155Z"}),
                json!({"id": first, "thread_name": "older name", "updated_at": "2026-01-11T15:23:20Z"}),
                json!({"id": first, "thread_name": "invalid date", "updated_at": "broken"}),
            ),
        )?;
        let list = |catalog: &mut Catalog, options| -> anyhow::Result<_> {
            Ok(catalog.list(cwd.to_str().unwrap(), &options).checked()?)
        };
        let page = list(&mut catalog, ListOptions::default())?;
        assert_eq!(
            (
                page.window.offset,
                page.window.next_offset,
                page.window.total
            ),
            (0, 50, 203)
        );
        assert!(page.window.has_more);
        assert_eq!(page.items[0].title, "登录流程");
        assert_eq!(page.items[1].provider_session_id, second);
        assert_eq!(page.items[1].forked_from_id.as_deref(), Some(first));
        assert_eq!(page.items[2].provider_session_id, third);
        println!(
            "initial page 50, total 203; newest valid index title, duplicate rollout and UUID tie order"
        );
        let page = list(
            &mut catalog,
            ListOptions {
                offset: Some(-10),
                limit: Some(300),
                ..Default::default()
            },
        )?;
        assert_eq!(page.items.len(), 200);
        let page = list(
            &mut catalog,
            ListOptions {
                offset: Some(200),
                limit: Some(300),
                ..Default::default()
            },
        )?;
        assert_eq!(page.items.len(), 3);
        assert!(!page.window.has_more);
        let page = list(
            &mut catalog,
            ListOptions {
                offset: Some(i64::MAX),
                limit: Some(0),
                ..Default::default()
            },
        )?;
        assert_eq!(
            (
                page.window.offset,
                page.window.limit,
                page.window.next_offset
            ),
            (203, 1, 203)
        );
        assert!(page.items.is_empty());
        println!("negative, last and out-of-range pages clamped; list cap 200");
        let page = list(
            &mut catalog,
            ListOptions {
                query: "  登录 ".into(),
                ..Default::default()
            },
        )?;
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].provider_session_id, first);
        let page = list(
            &mut catalog,
            ListOptions {
                query: "absent".into(),
                ..Default::default()
            },
        )?;
        assert!(page.items.is_empty());
        assert_eq!(page.window.total, 0);
        println!("exact cwd, malformed/missing title data, metadata hit and empty result");
        Ok(())
    }

    #[test]
    fn codex_catalog_rollout_preview_search_and_ancestry_fields() -> TestResult {
        let root = tempfile::tempdir()?;
        let home = root.path().join("codex");
        let dir = home.join("sessions");
        let cwd = root.path().to_str().unwrap();
        let parent = "10000000-0000-4000-8000-000000000001";
        let child = "10000000-0000-4000-8000-000000000002";
        let rows = [
            json!({"type":"turn_context","payload":{"turn_id":"t1","model":"fixture-model"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"<environment_context> hidden needle"}]}}),
            json!({"type":"response_item","payload":{"id":"prompt","type":"message","role":"user","content":[{"text":"older needle mirror"}]}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"t1","item":{"id":"prompt","type":"UserMessage","content":[{"text":"First NEEDLE 提示"}]}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"t1","item":{"id":"answer","type":"AgentMessage","phase":"final_answer","content":"Final answer needle"}}}),
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}),
        ];
        fixture(&dir, root.path(), parent, "parent", json!({}), &[], 10)?;
        let path = fixture(
            &dir,
            root.path(),
            child,
            "child",
            json!({"forked_from_id":parent,"history_base":{"thread_id":parent,"end_ordinal_exclusive":4,"end_byte_offset":521}}),
            &rows,
            11,
        )?;
        fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"{\"type\":")?;
        let mut catalog = Catalog::new(Some(home));
        let index = catalog
            .list(
                cwd,
                &ListOptions {
                    query: " needle ".into(),
                    full_text: true,
                    ..Default::default()
                },
            )
            .checked()?;
        assert_eq!(index.items.len(), 1);
        assert_eq!(index.items[0].match_count, 2);
        assert_eq!(index.items[0].forked_from_id.as_deref(), Some(parent));
        let base = index.items[0].history_base.as_ref().unwrap();
        assert_eq!(
            (
                &*base.thread_id,
                base.end_ordinal_exclusive,
                base.end_byte_offset
            ),
            (parent, Some(4), Some(521))
        );
        assert_eq!(index.items[0].provider_session_id, child);
        assert!(index.items[0].title.ends_with(child));
        let page = catalog.detail(cwd, child, None, None, Some(1)).checked()?;
        assert_eq!(page["page"]["startEventIndex"], 2);
        assert_eq!(page["page"]["hasEarlier"], true);
        assert_eq!(page["model"], "fixture-model");
        assert_eq!(page["firstPrompt"], "First NEEDLE 提示");
        let page = catalog
            .detail(cwd, child, None, Some(1), Some(1))
            .checked()?;
        assert_eq!(page["events"][0]["text"], "First NEEDLE 提示");
        let page = catalog
            .detail(cwd, child, Some(i64::MAX), None, Some(i64::MAX))
            .checked()?;
        assert!(page["events"].as_array().unwrap().is_empty());
        let page = catalog
            .search(cwd, child, "needLE", Some(1), Some(1))
            .checked()?;
        assert_eq!((page.window.total, page.items[0].event_index), (2, 2));
        assert_eq!(
            catalog
                .search(cwd, child, "mirror", None, None)
                .checked()?
                .window
                .total,
            0
        );
        assert_eq!(
            catalog
                .search(cwd, child, "", None, None)
                .checked()?
                .window
                .total,
            0
        );
        assert!(catalog.load(cwd, child).checked()?.has_completed_turn);
        println!(
            "case-insensitive full-text, paging and jump; injection and mirror excluded; partial tail accepted"
        );
        println!(
            "Fork parent and explicit ordinal/byte boundaries retained; selected child UUID unchanged"
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"broken\n{}\n")?;
        assert_eq!(
            catalog.detail(cwd, child, None, None, None).unwrap_err(),
            CORRUPT
        );
        assert_eq!(
            catalog
                .list(cwd, &ListOptions::default())
                .checked()?
                .items
                .len(),
            2
        );
        fs::remove_file(path)?;
        assert_eq!(
            catalog.detail(cwd, child, None, None, None).unwrap_err(),
            NOT_FOUND
        );
        assert_eq!(
            catalog
                .detail(cwd, "../other", None, None, None)
                .unwrap_err(),
            "Invalid provider session ID"
        );
        println!(
            "middle corruption localized, header list usable, removed and invalid UUID rejected"
        );
        Ok(())
    }

    #[test]
    fn codex_catalog_large_body_does_not_expand_list_reads() -> TestResult {
        let root = tempfile::tempdir()?;
        let id = "00100000-0000-4000-8000-000000000001";
        let dir = root.path().join("sessions");
        let path = fixture(&dir, root.path(), id, "large", json!({}), &[], 10)?;
        let mut file = fs::OpenOptions::new().append(true).open(path)?;
        file.write_all(&vec![b'x'; MAX_LINE as usize + 1])?;
        file.write_all(b"\n")?;
        let mut catalog = Catalog::new(Some(root.path().to_owned()));
        let cwd = root.path().to_str().unwrap();
        assert_eq!(
            catalog
                .list(cwd, &ListOptions::default())
                .checked()?
                .items
                .len(),
            1
        );
        assert_eq!(
            catalog.detail(cwd, id, None, None, None).unwrap_err(),
            CORRUPT
        );
        println!(
            "rollout with an oversized body record lists by header and detail reports bounded read"
        );
        Ok(())
    }
}
