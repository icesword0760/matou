//! Private HTTP ingress for a Claude run. SQLite remains owned by the host thread.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};
use uuid::Uuid;

pub(crate) const IDENTITY_TIMEOUT: Duration = Duration::from_secs(10);
const RETIRE_GRACE: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_HOOK_BYTES: usize = 1024 * 1024;
const MAX_HEADERS: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 8;
const EVENTS: [(&str, u8); 9] = [
    ("SessionStart", 5),
    ("SessionEnd", 3),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 10),
    ("PostToolUse", 10),
    ("PostToolUseFailure", 5),
    ("PermissionRequest", 10),
    ("Stop", 5),
    ("Notification", 5),
];

#[derive(Debug)]
pub(crate) struct HookSetup;

impl std::fmt::Display for HookSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("prepare provider hook")
    }
}

#[derive(Clone)]
pub(crate) struct Scope {
    pub session_id: String,
    pub run_id: String,
    pub generation: String,
    pub expected_id: Option<String>,
    pub fork_source_id: Option<String>,
}

#[derive(PartialEq, Eq)]
pub(crate) struct Observation {
    pub event: &'static str,
    pub identity: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub cwd: Option<String>,
    pub notification: Option<(Notification, Instant)>,
}

impl Observation {
    pub(super) fn parse(payload: &Map<String, Value>) -> Self {
        let text = |names: &[&str]| {
            names
                .iter()
                .find_map(|&name| payload.get(name).and_then(nonempty))
        };
        let event = text(&["hook_event_name", "hookEventName"])
            .and_then(|name| EVENTS.iter().find(|(event, _)| *event == name))
            .map_or("unknown", |&(event, _)| event);
        let identity = [
            "session_id",
            "sessionId",
            "conversationId",
            "conversation_id",
        ]
        .iter()
        .find_map(|name| {
            let value = payload.get(*name)?;
            nonempty(value)?;
            value.as_str()
        })
        .or_else(|| {
            let path = text(&["transcript_path", "transcriptPath", "agent_transcript_path"])?;
            transcript_uuid(path)
        });
        Self {
            event,
            identity: identity.map(str::to_owned),
            model: text(&["model"])
                .or_else(|| payload.get("model")?.get("id").and_then(nonempty))
                .map(str::to_owned),
            permission_mode: text(&["permission_mode", "permissionMode"]).map(str::to_owned),
            cwd: text(&["cwd"]).map(str::to_owned),
            notification: Notification::parse(event, payload).map(|event| (event, Instant::now())),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotificationKind {
    Completed,
    Permission,
    Waiting,
    Attention,
}

#[derive(PartialEq, Eq)]
pub(crate) struct Notification {
    pub kind: NotificationKind,
    subtitle: String,
    body: String,
    cooldown_key: &'static str,
}

impl Notification {
    fn parse(event: &str, payload: &Map<String, Value>) -> Option<Self> {
        let text = |key: &str| payload.get(key).and_then(nonempty);
        let nested = |key: &str, field: &str| payload.get(key)?.get(field).and_then(nonempty);
        let (kind, subtitle, body, limit, cooldown_key) = match event {
            "Stop" => {
                let project = text("cwd")
                    .and_then(|cwd| cwd.rsplit(['/', '\\']).find(|part| !part.is_empty()));
                let subtitle = project.map_or_else(
                    || "Completed".into(),
                    |project| format!("Completed in {project}"),
                );
                let body = text("last_assistant_message")
                    .or_else(|| text("lastAssistantMessage"))
                    .or_else(|| text("result"))
                    .map_or_else(
                        || {
                            project.map_or_else(
                                || "Claude session completed".into(),
                                |project| format!("Claude session completed in {project}"),
                            )
                        },
                        str::to_owned,
                    );
                (NotificationKind::Completed, subtitle, body, 200, "Stop")
            }
            "PermissionRequest" => {
                let tool = text("tool_name")
                    .or_else(|| text("toolName"))
                    .unwrap_or("operation");
                let detail = [
                    text("message"),
                    text("description"),
                    nested("tool_input", "command"),
                    nested("tool_input", "file_path"),
                    nested("toolInput", "command"),
                    nested("toolInput", "filePath"),
                ]
                .into_iter()
                .flatten()
                .next();
                let body = detail.map_or_else(
                    || format!("{tool} requires approval"),
                    |detail| format!("{tool}: {detail}"),
                );
                (
                    NotificationKind::Permission,
                    "Permission".into(),
                    body,
                    180,
                    "Notification",
                )
            }
            "Notification" => {
                let raw = [
                    text("message"),
                    text("body"),
                    text("text"),
                    text("prompt"),
                    text("error"),
                    text("description"),
                    nested("notification", "message"),
                    nested("data", "message"),
                ]
                .into_iter()
                .flatten()
                .next();
                let kind = NotificationKind::classify(raw.unwrap_or_default());
                (
                    kind,
                    kind.labels().1.into(),
                    raw.unwrap_or("Claude needs your attention").into(),
                    180,
                    "Notification",
                )
            }
            _ => return None,
        };
        Some(Self {
            kind,
            subtitle,
            body: compact(&body, limit),
            cooldown_key,
        })
    }

    pub fn payload(&self) -> Value {
        json!({
            "eventType": self.kind.labels().0,
            "title": "Claude Code",
            "subtitle": self.subtitle,
            "body": self.body,
            "sound": true,
            "cooldownKey": self.cooldown_key,
        })
    }
}

impl NotificationKind {
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Completed => ("completed", "Completed"),
            Self::Permission => ("permission", "Permission"),
            Self::Waiting => ("waiting", "Waiting"),
            Self::Attention => ("attention", "Attention"),
        }
    }

    fn classify(raw: &str) -> Self {
        let lower = raw.to_lowercase();
        if ["permission", "approve", "approval"]
            .iter()
            .any(|word| lower.contains(word))
        {
            Self::Permission
        } else if ["complet", "finish", "done", "success"]
            .iter()
            .any(|word| lower.contains(word))
        {
            Self::Completed
        } else if ["idle", "wait", "input"]
            .iter()
            .any(|word| lower.contains(word))
        {
            Self::Waiting
        } else {
            Self::Attention
        }
    }

    pub fn work_status(self, current: &str) -> &'static str {
        match (self, current) {
            (_, "error") => "error",
            (Self::Completed, _) => "idle",
            _ => "needs-input",
        }
    }
}

fn nonempty(value: &Value) -> Option<&str> {
    let value = value.as_str()?.trim_matches(js_whitespace);
    (!value.is_empty()).then_some(value)
}

fn js_whitespace(c: char) -> bool {
    c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace())
}

fn compact(value: &str, limit: usize) -> String {
    let line = value
        .split(js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut units: Vec<_> = line.encode_utf16().take(limit + 1).collect();
    if units.len() <= limit {
        return line;
    }
    units.truncate(limit - 1);
    // A JavaScript slice can leave half of a surrogate pair. JSON text in Rust
    // must be valid UTF-8, so render the surviving half as a replacement character.
    let mut shortened = String::from_utf16_lossy(&units);
    shortened.push('…');
    shortened
}

fn transcript_uuid(path: &str) -> Option<&str> {
    path.as_bytes()
        .windows(6)
        .enumerate()
        .filter(|(_, ext)| ext.eq_ignore_ascii_case(b".jsonl"))
        .find_map(|(end, _)| {
            let id = path.get(end.checked_sub(36)?..end)?;
            Uuid::parse_str(id).ok().map(|_| id)
        })
}

pub(crate) enum HookMessage {
    Observed(Scope, Observation, Instant),
    Retired(Scope),
}

type Registry = Arc<Mutex<HashMap<String, Record>>>;

struct Record {
    scope: Scope,
    _files: HookFiles,
    retired: Option<Instant>,
    reported_retirement: bool,
}

struct HookFiles {
    settings: PathBuf,
    script: PathBuf,
    temporary: PathBuf,
}

impl Drop for HookFiles {
    fn drop(&mut self) {
        for path in [&self.temporary, &self.settings, &self.script] {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) struct Registration {
    token: String,
    registry: Registry,
    pub settings_path: PathBuf,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(record) = self.registry.lock().unwrap().get_mut(&self.token) {
            record
                .retired
                .get_or_insert_with(|| Instant::now() + RETIRE_GRACE);
        }
    }
}

pub(crate) struct ProviderHookServer {
    port: u16,
    registry: Registry,
    messages: Receiver<HookMessage>,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
}

impl ProviderHookServer {
    pub fn start() -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .context("bind provider hook to local loopback")?;
        listener
            .set_nonblocking(true)
            .context("configure provider hook listener")?;
        let port = listener.local_addr()?.port();
        let registry = Registry::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (send, messages) = mpsc::sync_channel(128);
        let handle = {
            let registry = Arc::clone(&registry);
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("provider-hook".into())
                .spawn(move || listen(listener, &registry, &send, &stop))
                .context("start provider hook listener")?
        };
        Ok(Self {
            port,
            registry,
            messages,
            stop,
            listener: Some(handle),
        })
    }

    pub fn register(
        &self,
        data_root: &Path,
        scope: Scope,
        permission_respawn: bool,
    ) -> Result<Registration> {
        let token = new_token();
        let url = format!("http://127.0.0.1:{}/hooks/{token}", self.port);
        let directory = data_root.join("provider-hooks");
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .context("create provider hook directory")?;
        ensure!(
            fs::symlink_metadata(&directory)?.is_dir(),
            "provider hook directory must be a real directory"
        );
        set_mode(&directory, 0o700).context("make provider hook directory private")?;
        let files = HookFiles {
            settings: directory.join(format!("{token}.settings.json")),
            script: directory.join(format!("{token}.statusline.sh")),
            temporary: directory.join(format!("{token}.settings.json.tmp")),
        };
        let settings = settings(&url, &files.script, permission_respawn)?;
        write_private(&files.script, status_script(&url).as_bytes(), 0o700)
            .context("write provider statusline script")?;
        write_private(
            &files.temporary,
            &serde_json::to_vec_pretty(&settings)?,
            0o600,
        )
        .context("write provider settings")?;
        fs::rename(&files.temporary, &files.settings).context("publish provider settings")?;
        File::open(&files.settings).context("read provider settings")?;
        let settings_path = files.settings.clone();
        self.registry.lock().unwrap().insert(
            token.clone(),
            Record {
                scope,
                _files: files,
                retired: None,
                reported_retirement: false,
            },
        );
        Ok(Registration {
            token,
            registry: Arc::clone(&self.registry),
            settings_path,
        })
    }

    pub fn messages(&self) -> impl Iterator<Item = HookMessage> + '_ {
        self.messages.try_iter()
    }
}

impl Drop for ProviderHookServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        self.registry.lock().unwrap().clear();
    }
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn write_private(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    set_mode(path, mode)
}

fn settings(url: &str, script: &Path, permission_respawn: bool) -> Result<Value> {
    let script = script
        .to_str()
        .context("provider statusline path is not UTF-8")?;
    #[cfg(unix)]
    let command = if script
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
    {
        script.to_owned()
    } else {
        format!("'{}'", script.replace('\'', "'\\''"))
    };
    #[cfg(not(unix))]
    let command = script;
    let mut hooks: Map<String, Value> = EVENTS
        .into_iter()
        .map(|(event, timeout)| {
            (
                event.to_owned(),
                json!([{ "hooks": [{ "type": "http", "url": url, "timeout": timeout }] }]),
            )
        })
        .collect();
    if permission_respawn {
        // Some CLI versions skip HTTP SessionStart hooks. This switch holds
        // input until identity is known, so the startup event must reach the
        // same run-scoped listener without a user prompt.
        #[cfg(unix)]
        let (curl, sink) = ("/usr/bin/curl", "/dev/null");
        #[cfg(not(unix))]
        let (curl, sink) = ("curl.exe", "NUL");
        let command = format!(
            "{curl} --silent --noproxy '*' --max-time 2 --output {sink} --header 'content-type: application/json' --data-binary @- '{url}'"
        );
        hooks.insert(
            "SessionStart".into(),
            json!([{
                "hooks": [{"type": "command", "command": command, "timeout": 5}]
            }]),
        );
    }
    Ok(json!({
        "hooks": hooks,
        "statusLine": { "type": "command", "command": command, "padding": 0 },
    }))
}

fn status_script(url: &str) -> String {
    format!(
        "#!/bin/sh\n/usr/bin/curl --silent --show-error --max-time 2 --request POST --header 'content-type: application/json' --data-binary @- '{url}?kind=statusline' >/dev/null 2>&1 || true\n"
    )
}

fn new_token() -> String {
    // UUID v4 uses the platform CSPRNG. Copy only bytes without version or
    // variant bits, giving the token 32 independent random bytes.
    let mut random = [0; 32];
    for chunk in random.chunks_mut(14) {
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        for (output, input) in chunk
            .iter_mut()
            .zip(bytes[..6].iter().chain(&bytes[7..8]).chain(&bytes[9..]))
        {
            *output = *input;
        }
    }
    const BASE64URL: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut token = String::with_capacity(43);
    for chunk in random.chunks(3) {
        let a = chunk[0];
        let b = chunk[1];
        token.push(BASE64URL[(a >> 2) as usize] as char);
        token.push(BASE64URL[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        token.push(
            BASE64URL[(((b & 15) << 2) | (chunk.get(2).copied().unwrap_or(0) >> 6)) as usize]
                as char,
        );
        if let Some(c) = chunk.get(2) {
            token.push(BASE64URL[(c & 63) as usize] as char);
        }
    }
    token
}

fn listen(
    listener: TcpListener,
    registry: &Registry,
    send: &SyncSender<HookMessage>,
    stop: &AtomicBool,
) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        registry
            .lock()
            .unwrap()
            .retain(|_, record| record.retired.is_none_or(|deadline| now < deadline));
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        if workers.len() < MAX_CONNECTIONS
            && let Ok((socket, _)) = listener.accept()
        {
            let registry = Arc::clone(registry);
            let send = send.clone();
            if let Ok(worker) = thread::Builder::new()
                .name("provider-hook-request".into())
                .spawn(move || handle_request(socket, &registry, &send))
            {
                workers.push(worker);
            }
        } else {
            thread::sleep(Duration::from_millis(10));
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
}

fn handle_request(mut socket: TcpStream, registry: &Registry, send: &SyncSender<HookMessage>) {
    // BSD can inherit non-blocking mode from the listening socket.
    if socket.set_nonblocking(false).is_err() {
        return;
    }
    let result = read_request(&mut socket, registry, send);
    let (status, body) = match result {
        Ok(()) => ("200 OK", "{}"),
        Err(status) => (status, ""),
    };
    let _ = socket.set_write_timeout(Some(Duration::from_millis(100)));
    let _ = write!(
        socket,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

type HttpResult<T> = std::result::Result<T, &'static str>;
const BAD_REQUEST: &str = "400 Bad Request";
const TOO_LARGE: &str = "413 Payload Too Large";
const NOT_FOUND: &str = "404 Not Found";

fn read_request(
    socket: &mut TcpStream,
    registry: &Registry,
    send: &SyncSender<HookMessage>,
) -> HttpResult<()> {
    let mut reader = BufReader::new(DeadlineRead {
        socket,
        deadline: Instant::now() + REQUEST_TIMEOUT,
    });
    let mut header_budget = MAX_HEADERS;
    let first = read_line(&mut reader, &mut header_budget)?;
    let mut fields = first.split_ascii_whitespace();
    let method = fields.next().ok_or(BAD_REQUEST)?;
    if method != "POST" {
        return Err("405 Method Not Allowed");
    }
    let path = fields
        .next()
        .and_then(|path| path.strip_prefix("/hooks/"))
        .ok_or(NOT_FOUND)?;
    let (token, statusline) = path
        .strip_suffix("?kind=statusline")
        .map_or((path, false), |token| (token, true));
    if !matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1")) || fields.next().is_some() {
        return Err(BAD_REQUEST);
    }
    if !registry.lock().unwrap().contains_key(token) {
        return Err(NOT_FOUND);
    }

    let mut length = None;
    let mut chunked = false;
    loop {
        let header = read_line(&mut reader, &mut header_budget)?;
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').ok_or(BAD_REQUEST)?;
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(BAD_REQUEST);
            }
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(BAD_REQUEST);
            }
            length = Some(value.parse::<usize>().map_err(|_| TOO_LARGE)?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked || !value.trim().eq_ignore_ascii_case("chunked") {
                return Err(BAD_REQUEST);
            }
            chunked = true;
        }
    }
    let mut body = Vec::new();
    if chunked {
        if length.is_some() {
            return Err(BAD_REQUEST);
        }
        loop {
            let line = read_line(&mut reader, &mut header_budget)?;
            let length = usize::from_str_radix(line.split(';').next().unwrap_or(""), 16)
                .map_err(|_| BAD_REQUEST)?;
            if length == 0 {
                while !read_line(&mut reader, &mut header_budget)?.is_empty() {}
                break;
            }
            append_body(&mut reader, &mut body, length)?;
            if !read_line(&mut reader, &mut 2)?.is_empty() {
                return Err(BAD_REQUEST);
            }
        }
    } else {
        append_body(&mut reader, &mut body, length.ok_or(BAD_REQUEST)?)?;
    }
    let payload: Map<String, Value> = serde_json::from_slice(&body).map_err(|_| BAD_REQUEST)?;
    let mut registry = registry.lock().unwrap();
    let record = registry.get_mut(token).ok_or(NOT_FOUND)?;
    if record.retired.is_some() {
        if !record.reported_retirement
            && send
                .try_send(HookMessage::Retired(record.scope.clone()))
                .is_ok()
        {
            record.reported_retirement = true;
        }
        return Ok(());
    }
    let mut observation = Observation::parse(&payload);
    if statusline {
        observation.event = "unknown";
        observation.notification = None;
    }
    match send.try_send(HookMessage::Observed(
        record.scope.clone(),
        observation,
        Instant::now(),
    )) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
            Err("503 Service Unavailable")
        }
    }
}

struct DeadlineRead<'a> {
    socket: &'a mut TcpStream,
    deadline: Instant,
}

impl Read for DeadlineRead<'_> {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.socket.set_read_timeout(Some(remaining))?;
        self.socket.read(data)
    }
}

fn read_error(error: io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "408 Request Timeout",
        _ => BAD_REQUEST,
    }
}

fn read_line(reader: &mut impl BufRead, budget: &mut usize) -> HttpResult<String> {
    let mut line = Vec::new();
    let length = reader
        .take(*budget as u64)
        .read_until(b'\n', &mut line)
        .map_err(read_error)?;
    *budget -= length;
    let line = line.strip_suffix(b"\r\n").ok_or(BAD_REQUEST)?;
    String::from_utf8(line.to_vec()).map_err(|_| BAD_REQUEST)
}

fn append_body(reader: &mut impl Read, body: &mut Vec<u8>, length: usize) -> HttpResult<()> {
    if length > MAX_HOOK_BYTES - body.len() {
        return Err(TOO_LARGE);
    }
    let start = body.len();
    body.resize(start + length, 0);
    reader.read_exact(&mut body[start..]).map_err(read_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(run: &str) -> Scope {
        Scope {
            session_id: "session".into(),
            run_id: run.into(),
            generation: "generation".into(),
            expected_id: None,
            fork_source_id: None,
        }
    }

    fn request(server: &ProviderHookServer, token: &str, method: &str, body: &[u8]) -> Result<u16> {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port))?;
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        write!(
            socket,
            "{method} /hooks/{token} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )?;
        if body.len() <= MAX_HOOK_BYTES {
            socket.write_all(body)?;
        }
        let _ = socket.shutdown(std::net::Shutdown::Write);
        let mut response = String::new();
        BufReader::new(socket).read_line(&mut response)?;
        Ok(response
            .split_ascii_whitespace()
            .nth(1)
            .context("HTTP status")?
            .parse()?)
    }

    #[test]
    fn files_and_ingress_match_contract() -> Result<()> {
        let root = tempfile::tempdir()?;
        let server = ProviderHookServer::start()?;
        let run = server.register(root.path(), scope("current"), false)?;
        let config: Value = serde_json::from_reader(File::open(&run.settings_path)?)?;
        let script = run
            .settings_path
            .with_file_name(format!("{}.statusline.sh", run.token));
        let url = format!("http://127.0.0.1:{}/hooks/{}", server.port, run.token);
        assert_eq!(run.token.len(), 43);
        assert!(
            run.token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        );
        assert!(b"AEIMQUYcgkosw048".contains(&run.token.as_bytes()[42]));
        assert_eq!(config["hooks"].as_object().unwrap().len(), 9);
        for (event, timeout) in [
            ("SessionStart", 5),
            ("SessionEnd", 3),
            ("UserPromptSubmit", 5),
            ("PreToolUse", 10),
            ("PostToolUse", 10),
            ("PostToolUseFailure", 5),
            ("PermissionRequest", 10),
            ("Stop", 5),
            ("Notification", 5),
        ] {
            assert_eq!(
                config["hooks"][event],
                json!([{"hooks": [{"type": "http", "url": url, "timeout": timeout}]}])
            );
        }
        assert_eq!(
            config["statusLine"],
            json!({"type": "command", "command": script, "padding": 0})
        );
        assert_eq!(
            fs::read_to_string(&script)?,
            format!(
                "#!/bin/sh\n/usr/bin/curl --silent --show-error --max-time 2 --request POST --header 'content-type: application/json' --data-binary @- '{url}?kind=statusline' >/dev/null 2>&1 || true\n"
            )
        );
        let directory = script.parent().unwrap();
        assert_eq!(fs::read_dir(directory)?.count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for (path, expected) in [
                (directory, 0o700),
                (&run.settings_path, 0o600),
                (&script, 0o700),
            ] {
                assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, expected);
            }
        }

        assert_eq!(request(&server, &run.token, "GET", b"")?, 405);
        assert_eq!(request(&server, "incorrect", "POST", b"")?, 404);
        for body in [b"null".as_slice(), b"[]", b"true", b"\"x\"", b"{"] {
            assert_eq!(request(&server, &run.token, "POST", body)?, 400);
        }
        assert!(server.messages().next().is_none());
        let mut exact = br#"{"hook_event_name":"Stop","session_id":"external"}"#.to_vec();
        exact.resize(MAX_HOOK_BYTES, b' ');
        assert_eq!(request(&server, &run.token, "POST", &exact)?, 200);
        exact.push(b' ');
        assert_eq!(request(&server, &run.token, "POST", &exact)?, 413);
        let Some(HookMessage::Observed(scope, observation, _)) = server.messages().next() else {
            panic!("hook body was not delivered");
        };
        assert_eq!(scope.run_id, "current");
        assert_eq!(observation.identity.as_deref(), Some("external"));
        assert_eq!(
            observation.notification.unwrap().0.kind,
            NotificationKind::Completed
        );
        assert!(server.messages().next().is_none());
        let statusline = format!("{}?kind=statusline", run.token);
        assert_eq!(
            request(
                &server,
                &statusline,
                "POST",
                br#"{"hook_event_name":"Stop","session_id":" exact-ID "}"#,
            )?,
            200
        );
        let Some(HookMessage::Observed(_, statusline, _)) = server.messages().next() else {
            panic!("statusline body was not delivered");
        };
        assert_eq!(statusline.event, "unknown");
        assert_eq!(statusline.identity.as_deref(), Some(" exact-ID "));
        assert!(statusline.notification.is_none());
        Ok(())
    }

    #[test]
    fn notifications_follow_frozen_text_precedence() {
        use NotificationKind::*;

        for (payload, expected) in [
            (
                json!({"hook_event_name": "Stop", "message": "approval"}),
                Some(Completed),
            ),
            (
                json!({"hook_event_name": "PermissionRequest", "message": "done"}),
                Some(Permission),
            ),
            (
                json!({"hook_event_name": "PreToolUse", "message": "done"}),
                None,
            ),
            (
                json!({"hook_event_name": "unknown", "message": "done"}),
                None,
            ),
        ] {
            assert_eq!(
                Observation::parse(payload.as_object().unwrap())
                    .notification
                    .map(|(event, _)| event.kind),
                expected
            );
        }
        for (mut payload, expected) in [
            (
                json!({"message": "DONE but waiting for APPROVAL"}),
                Permission,
            ),
            (json!({"body": "Approve", "text": "Done"}), Permission),
            (
                json!({"message": " \u{feff} ", "body": 123, "text": "Finished, awaiting input"}),
                Completed,
            ),
            (
                json!({"message": "Review this", "body": "success"}),
                Attention,
            ),
            (
                json!({"prompt": "Successful", "error": "permission"}),
                Completed,
            ),
            (
                json!({"error": "COMPLETED", "description": "needs your input"}),
                Completed,
            ),
            (
                json!({"description": "IDLE", "notification": {"message": "done"}}),
                Waiting,
            ),
            (
                json!({"notification": {"message": "Awaiting"}, "data": {"message": "done"}}),
                Waiting,
            ),
            (
                json!({"notification": [{"message": "done"}], "data": {"message": "INPUT"}}),
                Waiting,
            ),
            (
                json!({"message": "", "notification_type": "permission_prompt"}),
                Attention,
            ),
        ] {
            payload["hookEventName"] = json!("Notification");
            let parsed = Observation::parse(payload.as_object().unwrap());
            let event = parsed.notification.unwrap().0;
            assert_eq!(event.kind, expected, "{payload}");
            assert_eq!(event.payload()["subtitle"], expected.labels().1);
            assert_eq!(event.payload()["cooldownKey"], "Notification");
        }
    }

    fn notification(payload: Value) -> Value {
        Observation::parse(payload.as_object().unwrap())
            .notification
            .unwrap()
            .0
            .payload()
    }

    #[test]
    fn stop_notification_uses_the_last_reply_and_cwd_tail() {
        let mut payload = json!({
            "hook_event_name": "Stop", "cwd": "  C:\\workspace/工程\\\\  ",
            "last_assistant_message": " first\u{feff}\nreply ",
            "lastAssistantMessage": "second", "result": "third",
        });
        assert_eq!(
            notification(payload.clone()),
            json!({
                "eventType": "completed", "title": "Claude Code",
                "subtitle": "Completed in 工程", "body": "first reply",
                "sound": true, "cooldownKey": "Stop",
            })
        );
        payload["last_assistant_message"] = json!(" \n ");
        assert_eq!(notification(payload.clone())["body"], "second");
        payload["lastAssistantMessage"] = json!({"text": "ignored"});
        assert_eq!(notification(payload.clone())["body"], "third");
        payload["result"] = Value::Null;
        assert_eq!(
            notification(payload.clone())["body"],
            "Claude session completed in 工程"
        );
        payload["cwd"] = json!("/");
        let event = notification(payload);
        assert_eq!(event["subtitle"], "Completed");
        assert_eq!(event["body"], "Claude session completed");
    }

    #[test]
    fn permission_notification_uses_the_frozen_tool_and_detail_order() {
        let mut payload = json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": " Shell ", "toolName": "Edit",
            "message": " confirm this ", "description": "described",
            "tool_input": {"command": "snake command", "file_path": "snake file"},
            "toolInput": {"command": "camel command", "filePath": "camel file"},
        });
        assert_eq!(
            notification(payload.clone()),
            json!({
                "eventType": "permission", "title": "Claude Code",
                "subtitle": "Permission", "body": "Shell: confirm this",
                "sound": true, "cooldownKey": "Notification",
            })
        );
        for (field, previous, next) in [
            ("/message", "confirm this", "described"),
            ("/description", "described", "snake command"),
            ("/tool_input/command", "snake command", "snake file"),
            ("/tool_input/file_path", "snake file", "camel command"),
            ("/toolInput/command", "camel command", "camel file"),
        ] {
            assert_eq!(
                notification(payload.clone())["body"],
                format!("Shell: {previous}")
            );
            *payload.pointer_mut(field).unwrap() = json!(" ");
            assert_eq!(
                notification(payload.clone())["body"],
                format!("Shell: {next}")
            );
        }
        payload["tool_name"] = json!(" ");
        assert_eq!(notification(payload.clone())["body"], "Edit: camel file");
        payload["toolInput"] = json!([{"filePath": "ignored"}]);
        assert_eq!(
            notification(payload.clone())["body"],
            "Edit requires approval"
        );
        payload["toolName"] = Value::Null;
        assert_eq!(notification(payload)["body"], "operation requires approval");
    }

    #[test]
    fn notifications_compact_and_truncate_at_frozen_utf16_limits() {
        for (hook, field, prefix, limit) in [
            ("Stop", "result", "", 200),
            ("PermissionRequest", "description", "File: ", 180),
            ("Notification", "body", "", 180),
        ] {
            let mut payload = json!({"hook_event_name": hook, "tool_name": "File"});
            let base = format!("a {}", "界".repeat(limit - prefix.len() - 4));
            payload[field] = json!(format!(" a\n\t{}🙂 ", &base[2..]));
            assert_eq!(
                notification(payload.clone())["body"],
                format!("{prefix}{base}🙂")
            );
            payload[field] = json!(format!("{base}🙂 extra"));
            assert_eq!(notification(payload)["body"], format!("{prefix}{base}�…"));
        }
        assert_eq!(
            notification(json!({"hook_event_name": "Notification"})),
            json!({
                "eventType": "attention", "title": "Claude Code",
                "subtitle": "Attention", "body": "Claude needs your attention",
                "sound": true, "cooldownKey": "Notification",
            })
        );
    }

    #[test]
    fn retired_token_loses_authority_before_the_files_expire() -> Result<()> {
        let root = tempfile::tempdir()?;
        let server = ProviderHookServer::start()?;
        let first = server.register(root.path(), scope("first"), false)?;
        let old = first.token.clone();
        let old_file = first.settings_path.clone();
        let second = server.register(root.path(), scope("second"), false)?;
        assert_ne!(old, second.token);
        drop(first);
        let body = br#"{"hook_event_name":"UserPromptSubmit","sessionId":"external"}"#;
        assert!(old_file.exists());
        assert_eq!(request(&server, &old, "POST", body)?, 200);
        assert_eq!(request(&server, &second.token, "POST", body)?, 200);
        let mut messages = server.messages();
        assert!(
            matches!(messages.next(), Some(HookMessage::Retired(scope)) if scope.run_id == "first")
        );
        assert!(
            matches!(messages.next(), Some(HookMessage::Observed(scope, _, _)) if scope.run_id == "second")
        );
        assert!(messages.next().is_none());
        drop(second);
        let deadline = Instant::now() + Duration::from_secs(4);
        let directory = old_file.parent().unwrap();
        while fs::read_dir(directory)?.next().is_some() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        assert!(fs::read_dir(directory)?.next().is_none());
        assert_eq!(request(&server, &old, "POST", body)?, 404);
        Ok(())
    }

    #[test]
    fn identity_comes_only_from_hook_fields() {
        let id = "baf44be9-1de8-439a-9fe9-1c45a4022069";
        for field in [
            "session_id",
            "sessionId",
            "conversationId",
            "conversation_id",
        ] {
            let body = json!({field: " observed ", "transcript_path": format!("/path/{id}.jsonl")});
            let observation = Observation::parse(body.as_object().unwrap());
            assert_eq!(observation.identity.as_deref(), Some(" observed "));
        }
        let body = json!({"transcriptPath": format!("/path/{id}.JSONL"), "hookEventName": "Stop"});
        let observation = Observation::parse(body.as_object().unwrap());
        assert_eq!(observation.identity.as_deref(), Some(id));
        assert_eq!(observation.event, "Stop");
        let body = json!({"session_id": " ", "untrusted": id});
        assert!(
            Observation::parse(body.as_object().unwrap())
                .identity
                .is_none()
        );
    }
}
