//! Read the Codex catalog on the shared provider-catalog worker.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, Metadata},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};

const MAX_LINE: u64 = 8 * 1024 * 1024;
const HEADER_CACHE_LIMIT: usize = 256;
const ANCESTOR_LIMIT: usize = 5;
const NOT_FOUND: &str = "Codex 会话不存在或不属于当前工作目录";
const INVALID_ID: &str = "Codex 会话标识必须是完整 UUID";
const CORRUPT: &str = "Codex 会话文件已损坏";
const UNREADABLE: &str = "无法读取 Codex 会话文件";
const MISSING_ANCESTOR: &str = "祖先历史不完整：祖先文件缺失或不可读，仅显示当前会话。";
const CYCLE: &str = "祖先历史存在循环，已停止回溯并显示可读取的历史。";
const DEPTH_LIMIT: &str = "祖先历史超过 5 层，仅显示已读取的历史。";

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
    created_at: Option<i64>,
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
    id: Option<String>,
    ordinal: Option<u64>,
    timestamp: Option<i64>,
}

struct Candidate {
    event: EventRef,
    turn: String,
    structured: bool,
}

#[derive(Default)]
struct Index {
    events: Vec<EventRef>,
    sort_default: i64,
    model: Option<String>,
    has_completed_turn: bool,
}

struct History {
    sources: Vec<(File, Rc<Index>)>,
    events: Vec<(usize, usize)>,
    warning: Option<&'static str>,
}

impl History {
    fn total(&self) -> usize {
        self.events.len() + usize::from(self.warning.is_some())
    }

    fn event(&mut self, position: usize) -> Result<(&EventRef, String), &'static str> {
        let (source, event) = self.events[position];
        let (file, index) = &mut self.sources[source];
        let event = &index.events[event];
        let (_, text) = visible(&read_event(file, event)?).ok_or(CORRUPT)?;
        Ok((event, text))
    }
}

pub(crate) struct Catalog {
    root: Option<PathBuf>,
    headers: HashMap<PathBuf, Header>,
    indexes: VecDeque<(Header, Rc<Index>)>,
}

impl Catalog {
    pub(crate) fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            headers: HashMap::new(),
            indexes: VecDeque::new(),
        }
    }

    pub(crate) fn list(
        &mut self,
        cwd: &str,
        options: &ListOptions,
    ) -> Result<Page<CatalogEntry>, &'static str> {
        let db = dates()?;
        let query = normalized(&options.query);
        let full_text = !query.is_empty() && options.full_text;
        let headers = self.headers(
            cwd,
            &db,
            |_, error| {
                if full_text { Ok(()) } else { Err(error) }
            },
        )?;
        let in_workspace = workspace_headers(cwd, &headers)?;
        let archive = full_text.then(|| newest_headers(headers.iter()));
        let titles = self.titles(cwd, &db)?;
        let mut entries = Vec::new();
        for header in in_workspace.into_values() {
            let mut entry = entry(header, &titles, &db);
            let metadata_match = !query.is_empty()
                && normalized(&format!("{} {}", entry.title, entry.provider_session_id))
                    .contains(&query);
            if let Some(archive) = &archive {
                let (window, hits) = self.search_header(&db, archive, header, &query, 0, 4)?;
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
        self.load_entry(cwd, id).map_err(|error| match error {
            NOT_FOUND => "Provider session was not found in this working directory",
            INVALID_ID => "Invalid provider session ID",
            _ => error,
        })
    }

    fn load_entry(&mut self, cwd: &str, id: &str) -> Result<CatalogEntry, &'static str> {
        let db = dates()?;
        let headers = self.selection_headers(cwd, id, &db)?;
        let header = selected(cwd, id, &headers)?;
        let mut entry = entry(header, &self.titles(cwd, &db)?, &db);
        let (_, index) = self.open_index(&db, header)?;
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
        let headers = self.selection_headers(cwd, id, &db)?;
        let header = selected(cwd, id, &headers)?;
        let mut history = self.open_history(&db, &newest_headers(headers.iter()), header)?;
        let total = history.total();
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
        for position in start..end.min(history.events.len()) {
            let (event, text) = history.event(position)?;
            let kind = event.kind;
            events.push(json!({
                "index": position + 1, "kind": kind, "role": (kind != "tool").then_some(kind),
                "text": truncate(&text, 32_768), "timestamp": event.timestamp,
                "toolName": (kind == "tool").then_some("Bash"), "matched": false,
            }));
        }
        if let Some(warning) = history.warning.filter(|_| start < total && end == total) {
            events
                .push(json!({"index": total, "kind": "system", "text": warning, "matched": false}));
        }
        let first_prompt = history
            .events
            .iter()
            .position(|&(source, event)| history.sources[source].1.events[event].kind == "user")
            .map(|position| history.event(position).map(|(_, text)| text))
            .transpose()?
            .map(|text| truncate(&text, 32_768).to_owned());
        Ok(json!({
            "model": history.sources[0].1.model, "permissionMode": "default", "events": events,
            "firstPrompt": first_prompt,
            "page": {
                "startEventIndex": if start < end { start + 1 } else { 0 },
                "endEventIndex": if start < end { end } else { 0 },
                "total": total, "hasEarlier": start < end && start > 0, "hasLater": end < total,
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
        let db = dates()?;
        let headers = self.selection_headers(cwd, id, &db)?;
        let header = selected(cwd, id, &headers)?;
        let (window, items) = self.search_header(
            &db,
            &newest_headers(headers.iter()),
            header,
            &normalized(query),
            offset(from),
            limit(max, 100, 200),
        )?;
        Ok(Page { window, items })
    }

    fn search_header(
        &mut self,
        db: &Connection,
        archive: &HashMap<&str, &Header>,
        header: &Header,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Window, Vec<Hit>), &'static str> {
        let mut hits = Vec::new();
        let mut total = 0;
        if !query.is_empty() {
            let mut history = self.open_history(db, archive, header)?;
            for position in 0..history.events.len() {
                let (event, text) = history.event(position)?;
                let kind = event.kind;
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

    fn selection_headers(
        &mut self,
        cwd: &str,
        id: &str,
        db: &Connection,
    ) -> Result<Vec<Header>, &'static str> {
        let id = uuid(id).ok_or(INVALID_ID)?;
        self.headers(cwd, db, |path, error| {
            if path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(&id))
            {
                Err(error)
            } else {
                Ok(())
            }
        })
    }

    fn headers(
        &mut self,
        cwd: &str,
        dates: &Connection,
        on_error: impl FnMut(&Path, &'static str) -> Result<(), &'static str>,
    ) -> Result<Vec<Header>, &'static str> {
        let root = sessions_root(self.effective_root(cwd).as_deref())?;
        let Some(root) = root else {
            return Ok(Vec::new());
        };
        let mut entries = Vec::new();
        scan_rollouts(&root, on_error, |path, file, metadata| {
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
                    let created_at = timestamp(dates, &payload["timestamp"]);
                    let header = Header {
                        root: root.clone(),
                        path: path.clone(),
                        id,
                        cwd: fs::canonicalize(raw_cwd).unwrap_or_else(|_| PathBuf::from(raw_cwd)),
                        created_at,
                        updated_at: millis(modified).or(created_at).unwrap_or_default(),
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
            entries.push(header);
            Ok(())
        })?;
        Ok(entries)
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

    fn open_index(
        &mut self,
        dates: &Connection,
        header: &Header,
    ) -> Result<(File, Rc<Index>), &'static str> {
        let (mut file, meta) = open_file(&header.root, &header.path)?.ok_or(NOT_FOUND)?;
        let modified = meta.modified().map_err(|_| UNREADABLE)?;
        let cached = self.indexes.iter().position(|(cached, _)| {
            cached.path == header.path && cached.modified == modified && cached.size == meta.len()
        });
        let (current, index) =
            if let Some(entry) = cached.and_then(|position| self.indexes.remove(position)) {
                entry
            } else {
                let index = index(&mut file, meta.len(), header, dates)?;
                let mut current = header.clone();
                current.modified = modified;
                current.size = meta.len();
                (current, Rc::new(index))
            };
        if self.indexes.len() > ANCESTOR_LIMIT {
            self.indexes.pop_front();
        }
        self.indexes.push_back((current, index.clone()));
        Ok((file, index))
    }

    fn open_history(
        &mut self,
        dates: &Connection,
        archive: &HashMap<&str, &Header>,
        child: &Header,
    ) -> Result<History, &'static str> {
        let mut sources = vec![self.open_index(dates, child)?];
        let mut bounds = vec![None];
        let mut base = child.history_base.as_ref();
        let mut visited = HashSet::from([child.id.as_str()]);
        let mut warning = None;
        while let Some(link) = base {
            if sources.len() > ANCESTOR_LIMIT {
                warning = Some(DEPTH_LIMIT);
                break;
            }
            if !visited.insert(&link.thread_id) {
                warning = Some(CYCLE);
                break;
            }
            let parent = archive.get(link.thread_id.as_str());
            let readable = parent.and_then(|parent| self.open_index(dates, parent).ok());
            let Some((parent, source)) = parent.zip(readable) else {
                sources.truncate(1);
                bounds.truncate(1);
                warning = Some(MISSING_ANCESTOR);
                break;
            };
            sources.push(source);
            bounds.push(Some(link));
            base = parent.history_base.as_ref();
        }

        let mut events = Vec::new();
        let mut seen = HashSet::new();
        for (source, (_, index)) in sources.iter().enumerate().rev() {
            for (position, event) in index.events.iter().enumerate() {
                let inherited = bounds[source].is_none_or(|link| {
                    link.end_byte_offset
                        .is_none_or(|end| event.offset + event.length as u64 <= end)
                        && link
                            .end_ordinal_exclusive
                            .is_none_or(|end| event.ordinal.is_none_or(|ordinal| ordinal < end))
                });
                if inherited && event.id.as_deref().is_none_or(|id| seen.insert(id)) {
                    events.push((source, position));
                }
            }
        }
        if child.history_base.is_some() {
            events.sort_by_key(|&(source, event)| {
                let index = &sources[source].1;
                index.events[event].timestamp.unwrap_or(index.sort_default)
            });
        }
        Ok(History {
            sources,
            events,
            warning,
        })
    }
}

fn newest_headers<'a>(headers: impl Iterator<Item = &'a Header>) -> HashMap<&'a str, &'a Header> {
    let mut result = HashMap::<&str, &Header>::new();
    for header in headers {
        if result.get(header.id.as_str()).is_none_or(|current| {
            (header.modified, &header.path) > (current.modified, &current.path)
        }) {
            result.insert(&header.id, header);
        }
    }
    result
}

fn workspace_headers<'a>(
    cwd: &str,
    headers: &'a [Header],
) -> Result<HashMap<&'a str, &'a Header>, &'static str> {
    let cwd = match fs::canonicalize(cwd) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => return Ok(HashMap::new()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(_) => return Err(UNREADABLE),
    };
    Ok(newest_headers(
        headers.iter().filter(|header| header.cwd == cwd),
    ))
}

fn selected<'a>(cwd: &str, id: &str, headers: &'a [Header]) -> Result<&'a Header, &'static str> {
    workspace_headers(cwd, headers)?
        .get(id.to_ascii_lowercase().as_str())
        .copied()
        .ok_or(NOT_FOUND)
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
    mut on_error: impl FnMut(&Path, &'static str) -> Result<(), &'static str>,
    mut read: impl FnMut(PathBuf, File, Metadata) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let items = match fs::read_dir(&directory) {
            Ok(items) => items,
            Err(_) if directory == root => return Err(UNREADABLE),
            Err(_) => {
                on_error(&directory, UNREADABLE)?;
                continue;
            }
        };
        for item in items {
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
            {
                let result = match open_file(root, &path) {
                    Ok(Some((file, metadata))) => read(path.clone(), file, metadata),
                    Ok(None) => Ok(()),
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    on_error(&path, error)?;
                }
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
        .prepare_cached("SELECT CAST(ROUND(unixepoch(?1, 'subsec') * 1000) AS INTEGER)")
        .ok()?
        .query_row([value.as_str()?], |row| row.get(0))
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

fn index(
    file: &mut File,
    size: u64,
    header: &Header,
    dates: &Connection,
) -> Result<Index, &'static str> {
    let mut reader = BufReader::new(file.take(size));
    let mut line = Vec::new();
    let mut result = Index {
        sort_default: header.created_at.unwrap_or(header.updated_at),
        ..Index::default()
    };
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
        let record = line.strip_suffix(b"\n").unwrap_or(&line);
        let record = record.strip_suffix(b"\r").unwrap_or(record);
        candidates.push(Candidate {
            event: EventRef {
                offset: start,
                length: record.len(),
                kind,
                id,
                ordinal: row["ordinal"].as_u64(),
                timestamp: timestamp(dates, &row["timestamp"]),
            },
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
                    .event
                    .id
                    .as_ref()
                    .is_some_and(|id| structured_ids.contains(id))
                    || (candidate.event.kind == "user"
                        && structured_users.contains(&candidate.turn)))
            {
                return None;
            }
            candidate
                .event
                .id
                .as_ref()
                .is_none_or(|id| seen.insert(id.clone()))
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
    fn codex_history_replays_frozen_cases() -> TestResult {
        let frozen: Value = serde_json::from_str(include_str!(
            "../../../../tooling/native-parity/fixtures/provider-history-reference.json"
        ))?;
        for case in frozen["cases"].as_array().unwrap() {
            let name = case["id"].as_str().unwrap();
            if !name.starts_with("codex-") {
                continue;
            }
            let root = tempfile::tempdir()?;
            let home = root.path().join("codex");
            let sessions = home.join("sessions");
            let workspace = root.path().join("workspace");
            fs::create_dir_all(&sessions)?;
            fs::create_dir(&workspace)?;
            let cwd = workspace.to_str().unwrap();
            let mut paths = Vec::new();
            for dataset in case["input"]["datasets"].as_array().unwrap() {
                for input in frozen["datasets"][dataset.as_str().unwrap()]
                    .as_array()
                    .unwrap()
                {
                    let id = input["id"].as_str().unwrap();
                    let path = input["path"].as_str().map_or_else(
                        || sessions.join(format!("rollout-{id}.jsonl")),
                        |path| home.join(path),
                    );
                    let mut rows = input["rows"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|row| {
                            if row["type"] == "session_meta" && row["payload"]["cwd"] == "workspace"
                            {
                                let mut row = row.clone();
                                row["payload"]["cwd"] = json!(cwd);
                                row.to_string()
                            } else {
                                row.as_str().map_or_else(|| row.to_string(), str::to_owned)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if input["noFinalNewline"] != true {
                        rows.push('\n');
                    }
                    fs::write(&path, rows.as_bytes())?;
                    if let Some(at) = input["mtimeMs"].as_u64() {
                        File::open(&path)?
                            .set_modified(UNIX_EPOCH + std::time::Duration::from_millis(at))?;
                    }
                    paths.push((path, rows));
                }
            }
            let mut catalog = Catalog::new(Some(home));
            let steps = case["input"]["steps"].as_array().unwrap();
            let expected = case["expected"]["steps"].as_array().unwrap();
            for (step, expected) in steps.iter().zip(expected) {
                let args = &step["args"];
                let id = args["providerSessionId"].as_str().unwrap_or_default();
                let query = args["query"].as_str().unwrap_or_default();
                match step["operation"].as_str().unwrap() {
                    "list" => {
                        let got = catalog
                            .list(
                                cwd,
                                &ListOptions {
                                    query: query.to_owned(),
                                    full_text: args["searchScope"] == "all",
                                    ..Default::default()
                                },
                            )
                            .checked()?;
                        assert_eq!(json!(got.window.total), expected["total"], "{name}");
                        for (entry, expected) in got
                            .items
                            .iter()
                            .zip(expected["sessions"].as_array().unwrap())
                        {
                            assert_eq!(entry.provider_session_id, expected["providerSessionId"]);
                            assert_eq!(json!(entry.hits), expected["hits"]);
                        }
                    }
                    "detail" => {
                        let result = catalog.detail(
                            cwd,
                            id,
                            None,
                            args["aroundEventIndex"].as_i64(),
                            args["limit"].as_i64(),
                        );
                        if expected["error"].is_object() {
                            assert_eq!(result.unwrap_err(), CORRUPT, "{name}");
                        } else {
                            let got = result.checked()?;
                            assert_eq!(got["page"], expected["page"], "{name}");
                            let project = |events: &Value| {
                                events
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|event| {
                                        let mut event = event.as_object().unwrap().clone();
                                        event.retain(|_, field| !field.is_null());
                                        event
                                    })
                                    .collect::<Vec<_>>()
                            };
                            assert_eq!(
                                project(&got["events"]),
                                project(&expected["events"]),
                                "{name}"
                            );
                        }
                    }
                    "search" => {
                        let got = catalog
                            .search(cwd, id, query, None, args["limit"].as_i64())
                            .checked()?;
                        assert_eq!(json!(got.window.total), expected["total"], "{name}");
                        assert_eq!(json!(got.window.has_more), expected["hasMore"], "{name}");
                        assert_eq!(json!(got.items), expected["hits"], "{name}");
                    }
                    "load" => assert_eq!(catalog.load(cwd, id).checked()?.provider_session_id, id),
                    operation => panic!("unexpected {operation} in {name}"),
                }
            }
            for (path, bytes) in paths {
                assert_eq!(fs::read(path)?, bytes.as_bytes(), "{name}: read-only");
            }
            println!("frozen history matched: {name}");
        }
        Ok(())
    }

    #[test]
    fn codex_history_clamps_bounds_and_reselects_duplicate_parent() -> TestResult {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("sessions");
        let other = root.path().join("worktree");
        fs::create_dir(&other)?;
        let cwd = root.path().to_str().unwrap();
        let parent = "10000000-0000-4000-8000-000000000001";
        let child = "10000000-0000-4000-8000-000000000002";
        let row = |text: &str, timestamp: i64, ordinal: Option<u64>| {
            json!({"type":"response_item","ordinal":ordinal,"timestamp":timestamp,
                   "payload":{"type":"message","role":"assistant","content":text}})
        };
        let latest = fixture(&dir, &other, parent, "new", json!({}), &[], 30)?;
        let first = row("祖先 needle 🧩", 101, None);
        let byte_end = fs::metadata(&latest)?.len() + first.to_string().len() as u64;
        let mut parent_file = fs::OpenOptions::new().append(true).open(&latest)?;
        writeln!(parent_file, "{first}\r")?;
        writeln!(parent_file, "{}", row("祖先后续 needle", 102, Some(5)))?;
        parent_file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(30))?;
        fixture(
            &dir,
            root.path(),
            parent,
            "stale",
            json!({}),
            &[row("较旧重复 UUID", 100, None)],
            20,
        )?;
        let mut catalog = Catalog::new(Some(root.path().to_owned()));
        let set_bounds = |byte, ordinal, mtime| {
            fixture(
                &dir,
                root.path(),
                child,
                "child",
                json!({"history_base":{"thread_id":parent, "end_byte_offset":byte, "end_ordinal_exclusive":ordinal}}),
                &[row("本会话 needle", 103, None)],
                mtime,
            )
        };
        for (byte, ordinal, mtime, total) in [
            (byte_end - 1, u64::MAX, 40, 1),
            (byte_end, u64::MAX, 41, 2),
            (u64::MAX, 5, 42, 2),
            (u64::MAX, u64::MAX, 43, 3),
            (0, 0, 44, 1),
        ] {
            set_bounds(byte, ordinal, mtime)?;
            let page = catalog.detail(cwd, child, None, None, None).checked()?;
            assert_eq!(page["page"]["total"], total);
            if total > 1 {
                assert_eq!(page["events"][0]["text"], "祖先 needle 🧩");
            }
            assert_eq!(page["events"][total - 1]["text"], "本会话 needle");
        }
        println!(
            "exact CRLF UTF-8 byte boundary, split record, missing ordinal, exclusive ordinal, zero and beyond EOF"
        );
        set_bounds(u64::MAX, u64::MAX, 45)?;
        let hits = catalog
            .search(cwd, child, " needle ", Some(1), Some(1))
            .checked()?;
        assert_eq!(
            (
                hits.window.total,
                hits.window.next_offset,
                hits.items[0].event_index
            ),
            (3, 2, 2)
        );
        let around = catalog
            .detail(
                cwd,
                child,
                None,
                Some(hits.items[0].event_index as i64),
                Some(1),
            )
            .checked()?;
        assert_eq!(around["events"][0]["text"], "祖先后续 needle");
        assert_eq!(
            around["page"],
            json!({"startEventIndex":2,"endEventIndex":2,"total":3,"hasEarlier":true,"hasLater":true})
        );
        let before = catalog
            .detail(cwd, child, Some(1), None, Some(1))
            .checked()?;
        assert_eq!(before["events"][0]["index"], 1);
        let beyond = catalog
            .detail(cwd, child, Some(999), None, Some(1))
            .checked()?;
        assert_eq!(beyond["events"], json!([]));
        assert_eq!(beyond["page"]["hasEarlier"], false);
        println!("ancestor search offset and hit-index jump share detail order and earlier page");
        fs::remove_file(latest)?;
        let page = catalog.detail(cwd, child, None, None, None).checked()?;
        assert_eq!(page["events"][0]["text"], "较旧重复 UUID");
        println!("archive selects the newest UUID across worktrees and refreshes after removal");
        Ok(())
    }

    #[test]
    fn codex_history_bad_ancestor_header_and_oversized_body_fall_back() -> TestResult {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("sessions");
        let cwd = root.path().to_str().unwrap();
        let parent = "20000000-0000-4000-8000-000000000001";
        let child = "20000000-0000-4000-8000-000000000002";
        fixture(
            &dir,
            root.path(),
            child,
            "child",
            json!({"history_base":{"thread_id":parent}}),
            &[
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":"当前正文"}}),
            ],
            10,
        )?;
        let path = fixture(&dir, root.path(), parent, "parent", json!({}), &[], 11)?;
        let mut catalog = Catalog::new(Some(root.path().to_owned()));
        let assert_fallback = |catalog: &mut Catalog| -> TestResult {
            let page = catalog.detail(cwd, child, None, None, None).checked()?;
            assert_eq!(page["events"][0]["text"], "当前正文");
            assert_eq!(page["events"][1]["text"], MISSING_ANCESTOR);
            assert_eq!(page["page"]["total"], 2);
            assert_eq!(
                catalog
                    .search(cwd, child, "当前正文", None, None)
                    .checked()?
                    .items[0]
                    .event_index,
                1
            );
            Ok(())
        };
        assert_eq!(
            catalog.detail(cwd, child, None, None, None).checked()?["page"]["total"],
            1
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(&vec![b'x'; MAX_LINE as usize + 1])?;
        assert_eq!(
            catalog.search(cwd, parent, "anything", None, None).err(),
            Some(CORRUPT)
        );
        assert_fallback(&mut catalog)?;
        println!("oversized ancestor row falls back while direct search reports corruption");
        fs::write(&path, b"bad header\n")?;
        assert_eq!(
            catalog.detail(cwd, parent, None, None, None).unwrap_err(),
            CORRUPT
        );
        assert_fallback(&mut catalog)?;
        println!("malformed ancestor header falls back while direct detail reports corruption");
        Ok(())
    }

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
            INVALID_ID
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
