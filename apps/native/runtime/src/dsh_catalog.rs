//! Read-only DSH v4 transcript catalog and per-card title observation.

use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{HashSet, VecDeque},
    fs::{self, File, Metadata},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::Value;

use crate::dsh_events::{DshUpdate, js_whitespace, valid_session_id};
use crate::provider_catalog::project_scope::ProjectScope;

const MIB: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 16 * MIB;
const MAX_FRAME_BYTES: usize = 32 * MIB;
const MAX_LOG_BYTES: u64 = 512 * MIB as u64;
const MAX_EVENTS: usize = 100_000;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Debug)]
pub enum CatalogError {
    InvalidId,
    Missing,
    Corrupt,
    UnsupportedEvent,
    ReadLimit,
    Io(io::Error),
}

impl CatalogError {
    pub fn message(&self, english: bool) -> Cow<'static, str> {
        let (zh, en) = match self {
            Self::InvalidId => (
                "DSH 会话标识格式错误",
                "The DSH session id has an invalid format",
            ),
            Self::Missing => (
                "DSH 会话不存在或不属于当前工作目录",
                "That DSH session does not exist or does not belong to this working directory",
            ),
            Self::Corrupt => (
                "DSH 会话文件已损坏或包含不支持的记录，无法预览",
                "The DSH session file is damaged or contains unsupported records and cannot be previewed",
            ),
            Self::UnsupportedEvent => (
                "DSH 会话包含暂不支持的必要事件，已停止预览",
                "The DSH session contains an unsupported required event; preview was stopped",
            ),
            Self::ReadLimit => (
                "DSH 会话超过预览读取上限，原文件未被修改",
                "The DSH session exceeds the preview read limit; the original file was not changed",
            ),
            Self::Io(error) => return Cow::Owned(error.to_string()),
        };
        Cow::Borrowed(if english { en } else { zh })
    }
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message(english()))
    }
}

impl std::error::Error for CatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for CatalogError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::NotFound {
            Self::Missing
        } else {
            Self::Io(error)
        }
    }
}

type Result<T> = std::result::Result<T, CatalogError>;

fn english() -> bool {
    std::env::var("MATOU_LOCALE").as_deref() == Ok("en")
}

pub fn default_session_root() -> Option<PathBuf> {
    let setting = |name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    setting("DSH_TUI_SESSION_ROOT").or_else(|| {
        setting("DSH_HOME")
            .or_else(|| std::env::home_dir().map(|home| home.join(".dsh")))
            .map(|home| home.join("sessions"))
    })
}

pub(crate) fn is_session_id(id: &str) -> bool {
    id.len() <= 160 && valid_session_id(id)
}

/// The provider derives its directory key from UTF-16 code units.
pub fn encode_project_path(cwd: &str) -> String {
    use std::fmt::Write as _;

    let mut key = String::new();
    let mut separator = false;
    for unit in cwd.encode_utf16() {
        match unit {
            47 | 58 | 92 => {
                if !separator && !key.is_empty() {
                    key.push('-');
                }
                separator = true;
            }
            45 if key.is_empty() => separator = false,
            45 | 46 | 48..=57 | 65..=90 | 95 | 97..=122 => {
                key.push(unit as u8 as char);
                separator = false;
            }
            _ => {
                write!(key, "~{unit:04X}").expect("writing to a String");
                separator = false;
            }
        }
        if key.len() >= 251 {
            key.truncate(251);
            break;
        }
    }
    format!("--{}--", if key.is_empty() { "root" } else { &key })
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchScope {
    Metadata,
    #[default]
    All,
}

#[derive(Default)]
pub struct ListOptions<'a> {
    pub query: &'a str,
    pub scope: SearchScope,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub title_for_session: Option<&'a dyn Fn(&str) -> Option<String>>,
}

#[derive(Default)]
pub struct DetailOptions<'a> {
    pub query: &'a str,
    pub before_event_index: Option<i64>,
    pub around_event_index: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub next_offset: usize,
    pub has_more: bool,
}

impl Pagination {
    fn new(total: usize, offset: Option<i64>, limit: Option<i64>, default_limit: i64) -> Self {
        let offset = offset.unwrap_or(0).clamp(0, total as i64) as usize;
        let limit = limit.unwrap_or(default_limit).clamp(1, 200) as usize;
        let next_offset = (offset + limit).min(total);
        Self {
            total,
            offset,
            limit,
            next_offset,
            has_more: next_offset < total,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub provider: &'static str,
    pub provider_session_id: String,
    pub cwd: String,
    pub title: String,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub permission_mode: &'static str,
    pub event_count: usize,
    pub match_count: usize,
    pub hits: Vec<SearchHit>,
    pub availability: &'static str,
}

#[derive(Debug, Serialize)]
pub struct SessionList {
    pub provider: &'static str,
    pub sessions: Vec<SessionSummary>,
    #[serde(flatten)]
    pub pagination: Pagination,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    #[serde(flatten)]
    pub summary: SessionSummary,
    pub transcript_title: String,
    pub events: Vec<PreviewEvent>,
    pub page: EventPage,
}

#[derive(Debug, Serialize)]
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
pub struct PreviewEvent {
    pub index: usize,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub text: String,
    pub matched: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub event_index: usize,
    pub kind: &'static str,
    pub excerpt: String,
}

#[derive(Debug, Serialize)]
pub struct SessionSearch {
    pub query: String,
    pub hits: Vec<SearchHit>,
    #[serde(flatten)]
    pub pagination: Pagination,
}

/// In-memory file references only. The provider directory is opened read-only.
pub struct DshSessionCatalog {
    root: PathBuf,
    indexes: VecDeque<Arc<Index>>,
    searches: VecDeque<SearchEntry>,
}

impl DshSessionCatalog {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            indexes: VecDeque::new(),
            searches: VecDeque::new(),
        }
    }

    pub fn list(&mut self, cwd: &Path, options: ListOptions<'_>) -> Result<SessionList> {
        let workspace = Workspace::new(cwd)?;
        let query = normalize(options.query);
        let mut sessions = Vec::new();
        for path in self.paths(&workspace, None)? {
            if let Some(index) = self.index(&path, &workspace)?
                && let Some(summary) = self.summarize(&index, &query, &options, false)?
            {
                sessions.push(summary);
            }
        }
        Ok(Self::finish_list(sessions, options))
    }

    pub(crate) fn list_in_project(
        &mut self,
        cwd: &Path,
        scope: &ProjectScope,
        options: ListOptions<'_>,
    ) -> Result<SessionList> {
        let query = normalize(options.query);
        let mut candidates = Vec::new();
        for path in self.project_paths(cwd, scope, None)? {
            if let Some(index) = self.index_matching(&path, |origin| scope.contains(origin))? {
                let summary = self.summarize(&index, &query, &options, true)?;
                candidates.push((
                    index.stamp.millis(),
                    index.path.clone(),
                    index.id.clone(),
                    summary,
                ));
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut seen = HashSet::new();
        let sessions = candidates
            .into_iter()
            .filter(|(_, _, id, _)| seen.insert(id.clone()))
            .filter_map(|(_, _, _, summary)| summary)
            .collect();
        Ok(Self::finish_list(sessions, options))
    }

    fn summarize(
        &mut self,
        index: &Arc<Index>,
        query: &str,
        options: &ListOptions<'_>,
        project: bool,
    ) -> Result<Option<SessionSummary>> {
        let title = options
            .title_for_session
            .and_then(|title| title(&index.id))
            .unwrap_or_else(|| index.title());
        let metadata_text = if project {
            format!(
                "{title} {} {} {}",
                index.id,
                index.cwd.display(),
                index.model.as_deref().unwrap_or_default()
            )
        } else {
            format!("{title} {}", index.id)
        };
        let metadata = !query.is_empty() && normalize(&metadata_text).contains(query);
        let hits = if query.is_empty() || options.scope == SearchScope::Metadata {
            Arc::from([])
        } else {
            self.hits(index, query)?
        };
        if query.is_empty() || metadata || !hits.is_empty() {
            Ok(Some(index.summary(title, &hits, metadata)))
        } else {
            Ok(None)
        }
    }

    fn finish_list(mut sessions: Vec<SessionSummary>, options: ListOptions<'_>) -> SessionList {
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| compare_ids(&a.provider_session_id, &b.provider_session_id))
        });
        let pagination = Pagination::new(sessions.len(), options.offset, options.limit, 50);
        let sessions = sessions
            .drain(pagination.offset..pagination.next_offset)
            .collect();
        SessionList {
            provider: "dsh",
            sessions,
            pagination,
        }
    }

    pub fn detail(
        &mut self,
        cwd: &Path,
        id: &str,
        options: DetailOptions<'_>,
    ) -> Result<SessionDetail> {
        let index = self.selected(cwd, id)?;
        self.detail_index(index, options)
    }

    pub(crate) fn detail_in_project(
        &mut self,
        cwd: &Path,
        scope: &ProjectScope,
        id: &str,
        options: DetailOptions<'_>,
    ) -> Result<SessionDetail> {
        let index = self.selected_in_project(cwd, scope, id)?;
        self.detail_index(index, options)
    }

    fn detail_index(
        &mut self,
        index: Arc<Index>,
        options: DetailOptions<'_>,
    ) -> Result<SessionDetail> {
        let query = normalize(options.query);
        let total = index.events.len();
        let mut limit = options.limit.unwrap_or(200).clamp(1, 500) as usize;
        let (start, end) = loop {
            let (start, end) = event_window(total, &options, limit);
            let selected = &index.events[start..end];
            if selected.len() <= 1
                || selected.iter().map(|event| event.line.len).sum::<usize>() <= 8 * MIB
            {
                break (start, end);
            }
            limit = (limit / 2).max(1);
        };
        let mut events = Vec::with_capacity(end - start);
        if start < end {
            let mut reader = EventReader::open(&index.path)?;
            for (at, event) in index.events[start..end].iter().enumerate() {
                let row = reader.read(event.line)?;
                let content = event_text(&row);
                let matched = matches_event(event, &content, &query);
                let preview = prefix(&content, 100_000);
                let text = if preview.len() == content.len() {
                    content
                } else {
                    let shortened = if english() {
                        "Preview shortened"
                    } else {
                        "预览内容已截短"
                    };
                    format!("{preview}\n[{shortened}]")
                };
                events.push(PreviewEvent {
                    index: start + at + 1,
                    kind: event.kind,
                    role: matches!(event.kind, "user" | "assistant").then_some(event.kind),
                    timestamp: event.timestamp.clone(),
                    tool_name: event.tool_name.clone(),
                    text,
                    matched,
                });
            }
        }
        let hits = if query.is_empty() {
            Arc::from([])
        } else {
            self.hits(&index, &query)?
        };
        let title = index.title();
        let start_event_index = if events.is_empty() { 0 } else { start + 1 };
        let end_event_index = if events.is_empty() { 0 } else { end };
        Ok(SessionDetail {
            summary: index.summary(title.clone(), &hits, false),
            transcript_title: title,
            events,
            page: EventPage {
                start_event_index,
                end_event_index,
                total,
                has_earlier: start_event_index > 1,
                has_later: end_event_index > 0 && end_event_index < total,
            },
        })
    }

    pub fn search(
        &mut self,
        cwd: &Path,
        id: &str,
        query: &str,
        offset: Option<i64>,
        limit: Option<i64>,
    ) -> Result<SessionSearch> {
        let index = self.selected(cwd, id)?;
        self.search_index(index, query, offset, limit)
    }

    pub(crate) fn search_in_project(
        &mut self,
        cwd: &Path,
        scope: &ProjectScope,
        id: &str,
        query: &str,
        offset: Option<i64>,
        limit: Option<i64>,
    ) -> Result<SessionSearch> {
        let index = self.selected_in_project(cwd, scope, id)?;
        self.search_index(index, query, offset, limit)
    }

    fn search_index(
        &mut self,
        index: Arc<Index>,
        query: &str,
        offset: Option<i64>,
        limit: Option<i64>,
    ) -> Result<SessionSearch> {
        let query = normalize(query);
        let hits = if query.is_empty() {
            Arc::from([])
        } else {
            self.hits(&index, &query)?
        };
        let pagination = Pagination::new(hits.len(), offset, limit, 100);
        let hits = hits[pagination.offset..pagination.next_offset].to_vec();
        Ok(SessionSearch {
            query,
            hits,
            pagination,
        })
    }

    pub fn validate_history(&mut self, cwd: &Path, id: &str) -> Result<()> {
        self.selected(cwd, id).map(|_| ())
    }

    fn selected(&mut self, cwd: &Path, id: &str) -> Result<Arc<Index>> {
        if !is_session_id(id) {
            return Err(CatalogError::InvalidId);
        }
        let workspace = Workspace::new(cwd)?;
        for path in self.paths(&workspace, Some(id))? {
            if let Some(index) = self.index(&path, &workspace)? {
                return Ok(index);
            }
        }
        Err(CatalogError::Missing)
    }

    fn selected_in_project(
        &mut self,
        cwd: &Path,
        scope: &ProjectScope,
        id: &str,
    ) -> Result<Arc<Index>> {
        if !is_session_id(id) {
            return Err(CatalogError::InvalidId);
        }
        let mut selected = None::<Arc<Index>>;
        for path in self.project_paths(cwd, scope, Some(id))? {
            if let Some(index) = self.index_matching(&path, |origin| scope.contains(origin))? {
                if selected.as_ref().is_none_or(|previous| {
                    index.stamp.millis() > previous.stamp.millis()
                        || (index.stamp.millis() == previous.stamp.millis()
                            && index.path < previous.path)
                }) {
                    selected = Some(index);
                }
            }
        }
        selected.ok_or(CatalogError::Missing)
    }

    fn project_paths(
        &self,
        cwd: &Path,
        scope: &ProjectScope,
        id: Option<&str>,
    ) -> Result<Vec<PathBuf>> {
        let workspace = Workspace::new(cwd)?;
        let mut roots = scope.roots().to_vec();
        roots.extend(
            scope
                .roots()
                .iter()
                .filter_map(|root| crate::provider_catalog::system_path_alias(root)),
        );
        // Preserve a caller's symlink spelling for the buckets written before canonicalization.
        for root in scope.roots() {
            if let Ok(relative) = workspace.canonical.strip_prefix(root) {
                if let Some(alias) = workspace
                    .original
                    .ancestors()
                    .nth(relative.components().count())
                {
                    roots.push(alias.to_owned());
                }
            }
        }
        let keys: Vec<_> = roots
            .iter()
            .map(|root| encode_project_path(&root.to_string_lossy()))
            .collect();
        let all_directories = roots.iter().any(|root| root.parent().is_none());
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let matching_bucket = all_directories
                || keys.iter().any(|key| {
                    name == key
                        || name
                            .strip_prefix(key.strip_suffix("--").unwrap_or(key))
                            .is_some_and(|suffix| suffix.starts_with('-'))
                });
            if !matching_bucket {
                continue;
            }
            if let Some(id) = id {
                find_transcript(&entry.path().join(id), &mut paths)?;
            } else {
                for session in fs::read_dir(entry.path())? {
                    let session = session?;
                    if session.file_type()?.is_dir()
                        && session.file_name().to_str().is_some_and(is_session_id)
                    {
                        find_transcript(&session.path(), &mut paths)?;
                    }
                }
            }
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    fn paths(&self, workspace: &Workspace, id: Option<&str>) -> Result<Vec<PathBuf>> {
        let canonical = encode_project_path(&workspace.canonical.to_string_lossy());
        let original = encode_project_path(&workspace.original.to_string_lossy());
        let alias = crate::provider_catalog::system_path_alias(&workspace.canonical)
            .map(|path| encode_project_path(&path.to_string_lossy()))
            .filter(|key| key != &original && key != &canonical);
        let mut paths = Vec::new();
        for project in std::iter::once(&original)
            .chain((canonical != original).then_some(&canonical))
            .chain(alias.as_ref())
        {
            let base = self.root.join(project);
            if let Some(id) = id {
                find_transcript(&base.join(id), &mut paths)?;
                continue;
            }
            let entries = match fs::read_dir(&base) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_dir()
                    && entry.file_name().to_str().is_some_and(is_session_id)
                {
                    find_transcript(&entry.path(), &mut paths)?;
                }
            }
        }
        Ok(paths)
    }

    fn index(&mut self, path: &Path, workspace: &Workspace) -> Result<Option<Arc<Index>>> {
        self.index_matching(path, |origin| origin == workspace.canonical)
    }

    fn index_matching(
        &mut self,
        path: &Path,
        accepts: impl Fn(&Path) -> bool,
    ) -> Result<Option<Arc<Index>>> {
        let file = File::open(path)?;
        let stamp = Stamp::new(&file.metadata()?)?;
        if let Some(at) = self
            .indexes
            .iter()
            .position(|entry| entry.path == path && entry.stamp == stamp)
        {
            let index = self.indexes.remove(at).expect("cached index exists");
            self.indexes.push_back(Arc::clone(&index));
            return Ok(accepts(&index.cwd).then_some(index));
        }
        let Some(index) = read_index(path, file, stamp, accepts)? else {
            return Ok(None);
        };
        let index = Arc::new(index);
        self.indexes.retain(|entry| entry.path != path);
        self.indexes.push_back(Arc::clone(&index));
        while self.indexes.len() > 16
            || self
                .indexes
                .iter()
                .map(|entry| entry.events.len())
                .sum::<usize>()
                > 2 * MAX_EVENTS
        {
            self.indexes.pop_front();
        }
        Ok(Some(index))
    }

    fn hits(&mut self, index: &Index, query: &str) -> Result<Arc<[SearchHit]>> {
        if let Some(entry) = self.searches.iter().find(|entry| {
            entry.path == index.path && entry.stamp == index.stamp && entry.query == query
        }) {
            return Ok(Arc::clone(&entry.hits));
        }
        let mut hits = Vec::new();
        if !index.events.is_empty() {
            let mut reader = EventReader::open(&index.path)?;
            for (at, event) in index.events.iter().enumerate() {
                let content = event_text(&reader.read(event.line)?);
                if matches_event(event, &content, query) {
                    hits.push(SearchHit {
                        event_index: at + 1,
                        kind: event.kind,
                        excerpt: excerpt(&content, query),
                    });
                }
            }
        }
        let hits = Arc::<[SearchHit]>::from(hits);
        self.searches.retain(|entry| entry.path != index.path);
        self.searches.push_back(SearchEntry {
            path: index.path.clone(),
            stamp: index.stamp,
            query: query.to_owned(),
            hits: Arc::clone(&hits),
        });
        while self.searches.len() > 16
            || self
                .searches
                .iter()
                .map(|entry| entry.hits.len())
                .sum::<usize>()
                > 2 * MAX_EVENTS
        {
            self.searches.pop_front();
        }
        Ok(hits)
    }
}

struct Workspace {
    original: PathBuf,
    canonical: PathBuf,
}

impl Workspace {
    fn new(cwd: &Path) -> io::Result<Self> {
        let mut original = PathBuf::new();
        for part in std::path::absolute(cwd)?.components() {
            if part == Component::ParentDir {
                original.pop();
            } else {
                original.push(part);
            }
        }
        let canonical = original
            .canonicalize()
            .map(|path| ordinary_path(path))
            .unwrap_or_else(|_| original.clone());
        Ok(Self {
            original,
            canonical,
        })
    }
}

#[cfg(windows)]
fn ordinary_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(drive) = text.strip_prefix(r"\\?\") {
        PathBuf::from(drive)
    } else {
        path
    }
}

#[cfg(not(windows))]
fn ordinary_path(path: PathBuf) -> PathBuf {
    path
}

fn find_transcript(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for name in ["session.v4.jsonl.zstd", "session.v4.jsonl"] {
        let path = directory.join(name);
        match path.metadata() {
            Ok(file) if file.is_file() => {
                paths.push(path);
                break;
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
}

impl Stamp {
    fn new(metadata: &Metadata) -> Result<Self> {
        let len = metadata.len();
        if len > MAX_LOG_BYTES {
            return Err(CatalogError::ReadLimit);
        }
        Ok(Self {
            len,
            modified: metadata.modified()?,
        })
    }

    fn millis(self) -> i64 {
        match self.modified.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as i64,
            Err(error) => -(error.duration().as_millis() as i64),
        }
    }
}

struct SearchEntry {
    path: PathBuf,
    stamp: Stamp,
    query: String,
    hits: Arc<[SearchHit]>,
}

struct Index {
    path: PathBuf,
    stamp: Stamp,
    id: String,
    cwd: PathBuf,
    created_at: i64,
    title: Option<String>,
    model: Option<String>,
    events: Vec<EventRef>,
}

impl Index {
    fn title(&self) -> String {
        self.title.clone().unwrap_or_else(|| {
            format!(
                "{} · {}",
                iso_timestamp(self.created_at),
                &self.id[..self.id.len().min(8)]
            )
        })
    }

    fn summary(&self, title: String, hits: &[SearchHit], metadata: bool) -> SessionSummary {
        SessionSummary {
            provider: "dsh",
            provider_session_id: self.id.clone(),
            cwd: self.cwd.to_string_lossy().into_owned(),
            title,
            updated_at: self.stamp.millis(),
            model: self.model.clone(),
            permission_mode: "default",
            event_count: self.events.len(),
            match_count: hits.len() + usize::from(metadata),
            hits: hits[..hits.len().min(4)].to_vec(),
            availability: "available",
        }
    }

    fn header(row: &Value, path: &Path, stamp: Stamp) -> Result<Self> {
        let id = row["id"].as_str().filter(|id| {
            is_session_id(id)
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == *id)
        });
        let created_at = row["createdAt"]
            .as_f64()
            .filter(|value| (0.0..=8.64e15).contains(value));
        let (Some(id), Some(created_at), Some(cwd)) = (
            id,
            created_at,
            row["cwd"]
                .as_str()
                .filter(|cwd| !cwd.trim_matches(js_whitespace).is_empty()),
        ) else {
            return Err(CatalogError::Corrupt);
        };
        if row["type"] != "session" || row["version"].as_f64() != Some(4.0) {
            return Err(CatalogError::Corrupt);
        }
        Ok(Self {
            path: path.to_owned(),
            stamp,
            id: id.to_owned(),
            cwd: Workspace::new(Path::new(cwd))?.canonical,
            created_at: created_at as i64,
            title: None,
            model: None,
            events: Vec::new(),
        })
    }

    fn push(&mut self, row: &Value, line: LineRef, previous_seq: &mut i64) -> Result<()> {
        let seq = integer(&row["seq"])
            .filter(|seq| seq > previous_seq)
            .ok_or(CatalogError::Corrupt)?;
        let record = row["type"].as_str().ok_or(CatalogError::Corrupt)?;
        *previous_seq = seq;

        let data = &row["data"];
        let kind = match record {
            "system/message" | "developer/message" => "system",
            "user/message" => "user",
            "assistant/message" => "assistant",
            "tool/result" => "tool",
            "session/title" => {
                if let Some(title) = nonempty(&data["title"]) {
                    self.title = Some(prefix(title, 4096).to_owned());
                }
                return Ok(());
            }
            "model/selection" | "request/header" => {
                let model = if record == "model/selection" {
                    &data["model"]
                } else {
                    &data["header"]["config"]["model"]
                };
                if let Some(model) = nonempty(model) {
                    self.model = Some(model.to_owned());
                }
                return Ok(());
            }
            _ if known_event(record) || row["ignorable"] == true => return Ok(()),
            _ => return Err(CatalogError::UnsupportedEvent),
        };
        let event = EventRef {
            seq,
            kind,
            timestamp: row.get("time").filter(|time| time.is_number()).cloned(),
            tool_name: nonempty(&data["message"]["toolName"]).map(str::to_owned),
            line,
        };
        let operation = &row["surfaceOp"];
        if operation == "append" {
            self.events.push(event);
        } else {
            self.replace(row, operation, event)?;
        }
        if self.events.len() > MAX_EVENTS {
            return Err(CatalogError::ReadLimit);
        }
        Ok(())
    }

    fn replace(&mut self, row: &Value, operation: &Value, event: EventRef) -> Result<()> {
        let position = |key| {
            let seq = integer(&operation[key])?;
            self.events.iter().position(|item| item.seq == seq)
        };
        let (Some(start), Some(end), Some(sources)) = (
            position("startSeq"),
            position("endSeq"),
            row["sourceEventSeqs"].as_array(),
        ) else {
            return Err(CatalogError::Corrupt);
        };
        if operation["op"] != "replace" || end < start {
            return Err(CatalogError::Corrupt);
        }
        let sources: std::collections::HashSet<_> = sources.iter().filter_map(integer).collect();
        if self.events[start..=end]
            .iter()
            .any(|item| !sources.contains(&item.seq))
        {
            return Err(CatalogError::Corrupt);
        }
        self.events.splice(start..=end, [event]);
        Ok(())
    }
}

fn known_event(kind: &str) -> bool {
    matches!(
        kind,
        "turn/start"
            | "turn/end"
            | "step/start"
            | "step/end"
            | "assistant/attempt"
            | "tool/call"
            | "request/context"
            | "session/end-seed"
            | "agent-preset/selected"
            | "agent/inbox/spliced"
            | "approval/asked"
            | "approval/decided"
            | "approval/policy"
            | "command/done"
            | "command/run"
            | "compaction/end"
            | "compaction/prune"
            | "compaction/start"
            | "compaction/summary"
            | "deliverables/presented"
            | "feedback/message-delete"
            | "feedback/message-put"
            | "feedback/record"
            | "goal/change"
            | "image/offload"
            | "job/registered"
            | "job/settled"
            | "llm/retry"
            | "llm/retry-started"
            | "permission/preset"
            | "plan/mode"
            | "sandbox/mode"
            | "schedule/change"
            | "session-log-deepseek/delivery-accepted"
            | "session/title-llm-request"
            | "subagent/catalog"
            | "subagent/descriptor"
            | "subagent/model-selection-policy"
            | "team/member"
            | "team/message/delivered"
            | "team/message/queued"
            | "team/task"
            | "todo/write"
            | "tool-workflow/agent-end"
            | "tool-workflow/agent-start"
            | "tool-workflow/run-end"
            | "tool-workflow/run-start"
            | "tool/ptc-dispatch"
            | "tool/ptc-dispatch-start"
            | "web/deepseek-search-llm-request"
            | "working-directory/change"
            | "workspace/changes"
    )
}

struct EventRef {
    seq: i64,
    kind: &'static str,
    timestamp: Option<Value>,
    tool_name: Option<String>,
    line: LineRef,
}

#[derive(Clone, Copy)]
struct LineRef {
    offset: u64,
    len: usize,
    frame: Option<Frame>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Frame {
    start: u64,
    end: u64,
}

fn read_index(
    path: &Path,
    file: File,
    stamp: Stamp,
    accepts: impl Fn(&Path) -> bool,
) -> Result<Option<Index>> {
    let mut index = None::<Index>;
    let mut previous_seq = -1;
    let mut accept = |row: Value, line| {
        if let Some(index) = &mut index {
            index.push(&row, line, &mut previous_seq)?;
        } else {
            let header = Index::header(&row, path, stamp)?;
            if !accepts(&header.cwd) {
                return Ok(false);
            }
            index = Some(header);
        }
        Ok(true)
    };
    let completed = if path.extension().is_some_and(|ext| ext == "zstd") {
        scan_frames(file, stamp.len, &mut accept)?
    } else {
        scan_plain(file, stamp.len, &mut accept)?
    };
    if completed {
        index.map(Some).ok_or(CatalogError::Corrupt)
    } else {
        Ok(None)
    }
}

fn parse_line(source: &[u8]) -> Result<Value> {
    if source.len() > MAX_LINE_BYTES {
        return Err(CatalogError::ReadLimit);
    }
    let row: Value = serde_json::from_str(&String::from_utf8_lossy(source))
        .map_err(|_| CatalogError::Corrupt)?;
    if !row.is_object() {
        return Err(CatalogError::Corrupt);
    }
    Ok(row)
}

fn scan_plain(
    file: File,
    size: u64,
    accept: &mut impl FnMut(Value, LineRef) -> Result<bool>,
) -> Result<bool> {
    let mut reader = BufReader::with_capacity(64 * 1024, file.take(size));
    let mut source = Vec::new();
    let mut offset = 0;
    loop {
        source.clear();
        let count = (&mut reader)
            .take((MAX_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut source)?;
        if count == 0 {
            return Ok(true);
        }
        let complete = source.last() == Some(&b'\n');
        if complete {
            source.pop();
        } else if count > MAX_LINE_BYTES {
            return Err(CatalogError::ReadLimit);
        }
        if !source.is_empty() {
            let row = match parse_line(&source) {
                Err(CatalogError::Corrupt) if !complete => return Ok(true),
                result => result?,
            };
            if !accept(
                row,
                LineRef {
                    offset,
                    len: source.len(),
                    frame: None,
                },
            )? {
                return Ok(false);
            }
        }
        offset += count as u64;
    }
}

struct EventReader {
    file: File,
    frame: Option<(Frame, Vec<u8>)>,
}

impl EventReader {
    fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            frame: None,
        })
    }

    fn read(&mut self, line: LineRef) -> Result<Value> {
        if let Some(frame) = line.frame {
            if self.frame.as_ref().map(|(key, _)| *key) != Some(frame) {
                self.frame = Some((frame, decode_frame(&mut self.file, frame)?));
            }
            let offset = line.offset as usize;
            let bytes = &self.frame.as_ref().expect("loaded frame").1;
            return parse_line(
                bytes
                    .get(offset..offset + line.len)
                    .ok_or(CatalogError::Corrupt)?,
            );
        }
        let mut source = vec![0; line.len];
        read_at(&mut self.file, line.offset, &mut source)?;
        parse_line(&source)
    }
}

fn read_at(file: &mut File, offset: u64, bytes: &mut [u8]) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            CatalogError::Corrupt
        } else {
            error.into()
        }
    })
}

/// Each complete frame is independent. A frame still being appended is left
/// unpublished, while a malformed record inside a complete frame is an error.
fn scan_frames(
    mut file: File,
    size: u64,
    accept: &mut impl FnMut(Value, LineRef) -> Result<bool>,
) -> Result<bool> {
    let mut start = 0;
    let mut decoded = 0;
    while let Some(end) = frame_end(&mut file, start, size)? {
        let frame = Frame { start, end };
        let content = decode_frame(&mut file, frame)?;
        decoded += content.len() as u64;
        if decoded > MAX_LOG_BYTES {
            return Err(CatalogError::ReadLimit);
        }
        let mut offset = 0;
        for row in content.split_inclusive(|byte| *byte == b'\n') {
            let source = row.strip_suffix(b"\n").unwrap_or(row);
            if !source.is_empty()
                && !accept(
                    parse_line(source)?,
                    LineRef {
                        offset,
                        len: source.len(),
                        frame: Some(frame),
                    },
                )?
            {
                return Ok(false);
            }
            offset += row.len() as u64;
        }
        start = end;
    }
    Ok(true)
}

fn frame_end(file: &mut File, start: u64, size: u64) -> Result<Option<u64>> {
    if start + 5 > size {
        return Ok(None);
    }
    let mut header = [0; 5];
    read_at(file, start, &mut header)?;
    let descriptor = header[4];
    if header[..4] != [0x28, 0xb5, 0x2f, 0xfd] || descriptor & 0x18 != 0 {
        return Err(CatalogError::Corrupt);
    }
    let single = descriptor & 0x20 != 0;
    let flag = descriptor >> 6;
    let content_size = if flag == 0 {
        u64::from(single)
    } else {
        1 << flag
    };
    let dictionary_size = [0, 1, 2, 4][usize::from(descriptor & 3)];
    let mut cursor = start + 5 + u64::from(!single) + dictionary_size + content_size;
    loop {
        if cursor + 3 > size {
            return Ok(None);
        }
        let mut header = [0; 4];
        read_at(file, cursor, &mut header[..3])?;
        let block = u32::from_le_bytes(header);
        let kind = (block >> 1) & 3;
        let block_bytes = match kind {
            1 => 1,
            0 | 2 => u64::from(block >> 3),
            _ => return Err(CatalogError::Corrupt),
        };
        cursor += 3 + block_bytes;
        let last = block & 1 != 0;
        if last && descriptor & 4 != 0 {
            cursor += 4;
        }
        if cursor - start > MAX_FRAME_BYTES as u64 {
            return Err(CatalogError::ReadLimit);
        }
        if last || cursor > size {
            return Ok((cursor <= size).then_some(cursor));
        }
    }
}

fn decode_frame(file: &mut File, frame: Frame) -> Result<Vec<u8>> {
    use ruzstd::decoding::{StreamingDecoder, errors::FrameDecoderError};

    let len = frame.end - frame.start;
    file.seek(SeekFrom::Start(frame.start))?;
    let source = BufReader::new(file.take(len));
    let mut decoder = StreamingDecoder::new_with_max_window_size(source, MAX_FRAME_BYTES as u64)
        .map_err(|error| match error {
            FrameDecoderError::WindowSizeTooBig { .. } => CatalogError::ReadLimit,
            _ => CatalogError::Corrupt,
        })?;
    if decoder.decoder.content_size() > MAX_FRAME_BYTES as u64 {
        return Err(CatalogError::ReadLimit);
    }
    let mut decoded = Vec::new();
    (&mut decoder)
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|_| CatalogError::Corrupt)?;
    if decoded.len() > MAX_FRAME_BYTES {
        return Err(CatalogError::ReadLimit);
    }
    let size = decoder.decoder.content_size();
    let checksum = decoder.decoder.get_checksum_from_data();
    if (size != 0 && size != decoded.len() as u64)
        || (checksum.is_some() && checksum != decoder.decoder.get_calculated_checksum())
        || decoder.decoder.bytes_read_from_source() != len
    {
        return Err(CatalogError::Corrupt);
    }
    Ok(decoded)
}

fn event_window(total: usize, options: &DetailOptions<'_>, limit: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let end = if let Some(anchor) = options.around_event_index {
        let anchor = anchor.clamp(1, total as i64) as usize;
        ((anchor - 1).saturating_sub(limit / 2) + limit).min(total)
    } else {
        options.before_event_index.map_or(total, |before| {
            before.saturating_sub(1).clamp(0, total as i64) as usize
        })
    };
    (end.saturating_sub(limit), end)
}

fn integer(value: &Value) -> Option<i64> {
    let number = value.as_f64()?;
    (number.abs() <= MAX_SAFE_INTEGER && number.fract() == 0.0).then_some(number as i64)
}

fn nonempty(value: &Value) -> Option<&str> {
    let value = value.as_str()?.trim_matches(js_whitespace);
    (!value.is_empty()).then_some(value)
}

fn normalize(value: &str) -> String {
    value.trim_matches(js_whitespace).to_lowercase()
}

fn event_text(row: &Value) -> String {
    let data = &row["data"];
    let content = [&data["message"]["content"], &data["content"], &data["text"]]
        .into_iter()
        .find(|value| !value.is_null());
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn matches_event(event: &EventRef, content: &str, query: &str) -> bool {
    !query.is_empty()
        && normalize(&format!(
            "{} {content}",
            event.tool_name.as_deref().unwrap_or_default()
        ))
        .contains(query)
}

fn utf16_boundary(value: &str, limit: usize) -> usize {
    let mut units = 0;
    for (offset, ch) in value.char_indices() {
        units += ch.len_utf16();
        if units > limit {
            return offset;
        }
    }
    value.len()
}

fn prefix(value: &str, limit: usize) -> &str {
    &value[..utf16_boundary(value, limit)]
}

fn excerpt(text: &str, query: &str) -> String {
    let flat = text
        .split(js_whitespace)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let lower = flat.to_lowercase();
    let Some(at) = lower.find(query) else {
        return prefix(&flat, 120).to_owned();
    };
    let position = lower[..at].encode_utf16().count();
    let start = utf16_boundary(&flat, position.saturating_sub(36));
    let end = utf16_boundary(&flat, position + query.encode_utf16().count() + 70);
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        &flat[start..end],
        if end < flat.len() { "…" } else { "" }
    )
}

fn compare_ids(a: &str, b: &str) -> Ordering {
    // Session IDs are ASCII. Match the reference collation's punctuation order
    // and lowercase-first tie breaking over this restricted alphabet.
    let primary = |byte: u8| match byte {
        b'_' => 0,
        b'-' => 1,
        b'.' => 2,
        _ => byte.to_ascii_lowercase(),
    };
    a.bytes()
        .map(primary)
        .cmp(b.bytes().map(primary))
        .then_with(|| {
            a.bytes()
                .map(|byte| byte.is_ascii_uppercase())
                .cmp(b.bytes().map(|byte| byte.is_ascii_uppercase()))
        })
}

fn iso_timestamp(millis: i64) -> String {
    // Gregorian era conversion also covers ISO expanded years accepted by the
    // provider (up to +275760). A day has no timezone offset in this format.
    let days = millis / 86_400_000 + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let year = era * 400 + year_of_era + i64::from(month <= 2);
    let year = if year > 9999 {
        format!("+{year:06}")
    } else {
        format!("{year:04}")
    };
    let hour = millis / 3_600_000 % 24;
    let minute = millis / 60_000 % 60;
    let second = millis / 1000 % 60;
    let fraction = millis % 1000;
    format!("{year}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{fraction:03}Z")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleSource {
    Default,
    Auto,
    Manual,
}

/// The card owns the visible title. Always remember the latest accepted
/// provider title, so an explicit return to automatic naming can use it.
pub struct DshTitleState {
    title: String,
    source: TitleSource,
    provider_title: Option<String>,
    run_id: Option<String>,
}

impl DshTitleState {
    pub fn new(title: String, source: TitleSource, provider_title: Option<String>) -> Self {
        Self {
            title,
            source,
            provider_title,
            run_id: None,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn source(&self) -> TitleSource {
        self.source
    }

    pub fn provider_title(&self) -> Option<&str> {
        self.provider_title.as_deref()
    }

    pub fn start_run(&mut self, run_id: impl Into<String>) {
        self.run_id = Some(run_id.into());
    }

    pub fn end_run(&mut self, run_id: &str) {
        if self.run_id.as_deref() == Some(run_id) {
            self.run_id = None;
        }
    }

    /// Consume the title from an authenticated, accepted DshEvents update.
    /// Returns whether any persisted title field would change.
    pub fn observe(&mut self, run_id: &str, update: &DshUpdate<'_>) -> bool {
        if self.run_id.as_deref() != Some(run_id) {
            return false;
        }
        let Some(title) = update
            .title
            .map(|title| title.trim_matches(js_whitespace))
            .filter(|title| !title.is_empty())
        else {
            return false;
        };
        let visible_changed = self.source != TitleSource::Manual
            && (self.title != title || self.source != TitleSource::Auto);
        let provider_changed = self.provider_title.as_deref() != Some(title);
        if provider_changed {
            self.provider_title = Some(title.to_owned());
        }
        if visible_changed {
            self.title = title.to_owned();
            self.source = TitleSource::Auto;
        }
        visible_changed || provider_changed
    }

    pub fn rename(&mut self, title: &str) -> bool {
        let title = title.trim_matches(js_whitespace);
        if title.is_empty() || (self.title == title && self.source == TitleSource::Manual) {
            return false;
        }
        self.title = title.to_owned();
        self.source = TitleSource::Manual;
        true
    }

    pub fn restore_auto_title(&mut self) -> bool {
        let Some(title) = &self.provider_title else {
            return false;
        };
        if self.source == TitleSource::Auto && self.title == *title {
            return false;
        }
        self.title.clone_from(title);
        self.source = TitleSource::Auto;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(path: &Path, args: &[&str]) {
        let output = matou_native_core::git_repo::git_command(path)
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository(path: &Path) {
        fs::create_dir_all(path).unwrap();
        git(path, &["init", "--quiet"]);
        git(
            path,
            &[
                "-c",
                "user.name=Catalog Test",
                "-c",
                "user.email=catalog@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
    }

    fn project_transcript(root: &Path, directory: &Path, origin: &Path, id: &str) {
        let path = root
            .join(encode_project_path(&directory.to_string_lossy()))
            .join(id);
        fs::create_dir_all(&path).unwrap();
        let rows = [
            serde_json::json!({"type":"session", "version":4, "id":id, "cwd":origin, "createdAt":1000}),
            serde_json::json!({"type":"request/header", "seq":0, "data":{"header":{"config":{"model":"fixture-model"}}}}),
            serde_json::json!({"type":"user/message", "seq":1, "surfaceOp":"append", "data":{"content":[{"text":format!("needle conversation {id}")}]}}),
        ];
        let text = rows
            .into_iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>();
        fs::write(path.join("session.v4.jsonl"), text).unwrap();
    }

    #[test]
    fn project_catalog_lists_worktrees_and_subdirectories_and_preserves_exact_history() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("project");
        repository(&main);
        let main = main.canonicalize().unwrap();
        let linked = main.parent().unwrap().join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                linked.to_str().unwrap(),
            ],
        );
        let subdirectory = main.join("src");
        fs::create_dir(&subdirectory).unwrap();
        let sibling = main.parent().unwrap().join("project-other");
        repository(&sibling);
        let nested = main.join("vendor/independent");
        repository(&nested);
        let root = temp.path().join("histories");
        for (id, cwd) in [
            ("main", &main),
            ("linked", &linked),
            ("child", &subdirectory),
            ("sibling", &sibling),
            ("nested", &nested),
        ] {
            project_transcript(&root, cwd, cwd, id);
        }
        // A misleading bucket name must never override the cwd in the provider's header.
        project_transcript(&root, &main, &sibling, "misplaced");
        let scope = ProjectScope::discover(&linked, Some(&main)).unwrap();
        let mut catalog = DshSessionCatalog::new(root);
        let exact = catalog.list(&linked, ListOptions::default()).unwrap();
        assert_eq!(exact.sessions.len(), 1);
        assert_eq!(exact.sessions[0].provider_session_id, "linked");
        let project = catalog
            .list_in_project(&linked, &scope, ListOptions::default())
            .unwrap();
        let ids: HashSet<_> = project
            .sessions
            .iter()
            .map(|entry| entry.provider_session_id.as_str())
            .collect();
        assert_eq!(ids, HashSet::from(["main", "linked", "child"]));
        let detail = catalog
            .detail_in_project(&linked, &scope, "main", DetailOptions::default())
            .unwrap();
        assert_eq!(detail.summary.cwd, main.to_string_lossy());
        assert!(detail.events[0].text.contains("needle conversation main"));
        assert_eq!(
            catalog
                .search_in_project(&linked, &scope, "main", "needle", None, None)
                .unwrap()
                .pagination
                .total,
            1
        );
        assert!(matches!(
            catalog.validate_history(&linked, "main"),
            Err(CatalogError::Missing)
        ));
        for excluded in ["sibling", "nested", "misplaced"] {
            assert!(matches!(
                catalog.detail_in_project(&linked, &scope, excluded, DetailOptions::default()),
                Err(CatalogError::Missing)
            ));
        }
        let by_path = catalog
            .list_in_project(
                &linked,
                &scope,
                ListOptions {
                    query: subdirectory.to_str().unwrap(),
                    scope: SearchScope::Metadata,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(by_path.sessions.len(), 1);
        assert_eq!(by_path.sessions[0].provider_session_id, "child");
        let by_model = catalog
            .list_in_project(
                &linked,
                &scope,
                ListOptions {
                    query: "fixture-model",
                    scope: SearchScope::Metadata,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(by_model.sessions.len(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn project_catalog_recovers_alias_buckets_and_deduplicates_session_ids() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("project");
        repository(&main);
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&main, &alias).unwrap();
        let child = main.join("src");
        fs::create_dir(&child).unwrap();
        let root = temp.path().join("histories");
        project_transcript(&root, &main, &main, "shared");
        project_transcript(&root, &alias, &alias, "shared");
        project_transcript(&root, &alias, &alias, "alias-only");
        let scope = ProjectScope::discover(&alias.join("src"), None).unwrap();
        let mut catalog = DshSessionCatalog::new(root);
        let list = catalog
            .list_in_project(&alias.join("src"), &scope, ListOptions::default())
            .unwrap();
        assert_eq!(list.sessions.len(), 2);
        let detail = catalog
            .detail_in_project(
                &alias.join("src"),
                &scope,
                "alias-only",
                DetailOptions::default(),
            )
            .unwrap();
        assert_eq!(
            detail.summary.cwd,
            main.canonicalize().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn project_encoding_and_iso_fallback_match_provider_boundaries() {
        assert_eq!(encode_project_path("/"), "--root--");
        assert_eq!(
            encode_project_path(r"C:\目录\🧪\demo"),
            "--C-~76EE~5F55-~D83E~DDEA-demo--"
        );
        assert_eq!(encode_project_path(&"界".repeat(100)).len(), 255);
        assert_eq!(iso_timestamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_timestamp(951_782_400_001), "2000-02-29T00:00:00.001Z");
        assert_eq!(
            iso_timestamp(8_640_000_000_000_000),
            "+275760-09-13T00:00:00.000Z"
        );
    }

    #[test]
    fn detail_window_anchors_and_rebalances_at_the_end() {
        let options = DetailOptions {
            around_event_index: Some(100),
            before_event_index: Some(1),
            ..Default::default()
        };
        assert_eq!(event_window(12, &options, 5), (7, 12));
        let options = DetailOptions {
            before_event_index: Some(1),
            ..Default::default()
        };
        assert_eq!(event_window(12, &options, 5), (0, 0));
    }
}
