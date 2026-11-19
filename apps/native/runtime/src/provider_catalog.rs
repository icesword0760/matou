//! Read Claude Code's project transcripts without modifying the provider's catalog.
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap, VecDeque},
    fs::{self, File},
    io::{self, BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
};

use rusqlite::{Connection, Statement};
use serde::Serialize;
use serde_json::Value;

const LIST_LIMIT: usize = 200;
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_EVENT_PAGE_LIMIT: i64 = 200;
const MAX_EVENT_PAGE_LIMIT: i64 = 500;
const SESSION_NOT_FOUND: &str = "Provider session was not found in this working directory";

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
    matched: bool,
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
    let result = detail_from_projects(
        &root,
        &cwd,
        &workspace,
        id,
        start_event_index.map(|start| usize::try_from(start.max(1)).unwrap_or(usize::MAX)),
        limit
            .unwrap_or(DEFAULT_EVENT_PAGE_LIMIT)
            .clamp(1, MAX_EVENT_PAGE_LIMIT) as usize,
        &mut skipped,
    );
    skipped.report();
    result
}

fn detail_from_projects(
    root: &Path,
    cwd: &Path,
    workspace: &Path,
    id: &str,
    start: Option<usize>,
    limit: usize,
    skipped: &mut Skipped,
) -> Result<SessionDetail, &'static str> {
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
        let page = preview_page(reader, workspace, start, limit, &mut timestamp, skipped)
            .map_err(|_| READ_ERROR)?;
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
        if let Some(detail) = page {
            if !detail.events.is_empty() {
                return Ok(detail);
            }
            found = Some(detail);
        }
    }
    found.ok_or(SESSION_NOT_FOUND)
}

fn preview_page(
    mut reader: impl BufRead,
    workspace: &Path,
    start: Option<usize>,
    limit: usize,
    timestamp: &mut Statement<'_>,
    skipped: &mut Skipped,
) -> io::Result<Option<SessionDetail>> {
    let mut origin_found = false;
    let mut line = Vec::new();
    let mut total = 0;
    let mut events = VecDeque::with_capacity(limit);
    let mut last_before_start = None;
    while let Some(row) = read_row(&mut reader, &mut line, skipped, false)? {
        if !origin_found && let Some(origin) = nonempty(&row["cwd"]) {
            if !matches_workspace(origin, workspace)? {
                return Ok(None);
            }
            origin_found = true;
        }
        let Some(mut event) = preview_event(&row["message"]) else {
            continue;
        };
        total += 1;
        event.index = total;
        event.timestamp = row.get("timestamp").cloned();
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
    if !origin_found && total > 0 {
        return Ok(None);
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
    let file = File::open(&path)?;
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
    let mut origin_found = false;
    let mut title = None;
    let mut auto_title = None;
    let mut first_prompt = None;
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
        // The first cwd is the conversation's origin even after later cd commands.
        if !origin_found && let Some(origin) = nonempty(&row["cwd"]) {
            if !matches_workspace(origin, workspace)? {
                return Ok(None);
            }
            origin_found = true;
        }
        if let Some(custom) = nonempty(&row["customTitle"]) {
            title = Some(custom.to_owned());
        }
        if row["type"] == "ai-title"
            && let Some(auto) = nonempty(&row["aiTitle"])
        {
            auto_title = Some(auto.to_owned());
        }
        let date = &row["timestamp"];
        let millis = date
            .as_f64()
            .map(|millis| millis as i64)
            .or_else(|| parse_date(date, timestamp));
        if let Some(millis) = millis {
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
        let (has_text, tool, text) = message_content(
            &message["content"],
            first_prompt.is_none() && message["role"] == "user",
        );
        if has_text {
            entry.message_count += 1;
            if !tool && first_prompt.is_none() && !text.is_empty() {
                first_prompt = Some(compact_title(&text));
            }
        }
    }
    entry.title = title
        .or(auto_title)
        .or(first_prompt)
        .unwrap_or_else(|| "未命名 Claude 会话".into());
    Ok((origin_found && entry.message_count > 0).then_some(entry))
}

fn nonempty(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn message_content(content: &Value, collect_title: bool) -> (bool, bool, String) {
    if let Some(text) = nonempty(content) {
        return (
            true,
            false,
            if collect_title {
                text.to_owned()
            } else {
                String::new()
            },
        );
    }
    let mut has_text = false;
    let mut tool = false;
    let mut title = String::new();
    for block in content.as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") if let Some(text) = nonempty(&block["text"]) => {
                has_text = true;
                if collect_title {
                    if !title.is_empty() {
                        title.push(' ');
                    }
                    title.push_str(text);
                }
            }
            Some("tool_use") if block["name"].is_string() => {
                tool = true;
                has_text |= nonempty(&block["name"]).is_some() || block.get("input").is_some();
            }
            Some("tool_result") => {
                tool = true;
                has_text |= nonempty(&block["content"]).is_some()
                    || block
                        .get("content")
                        .is_some_and(|content| !content.is_string());
            }
            _ => {}
        }
    }
    (has_text, tool, title)
}

fn compact_title(text: &str) -> String {
    let mut title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.encode_utf16().count() > 72 {
        let mut units = 0;
        let end = title
            .char_indices()
            .find(|(_, c)| {
                units += c.len_utf16();
                units > 71
            })
            .map_or(title.len(), |(index, _)| index);
        title.truncate(end);
        title.push('…');
    }
    title
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
