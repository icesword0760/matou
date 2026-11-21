//! The local v1 control channel. Socket threads authenticate and frame requests.
//! Only the domain thread can dispatch them against the live topology.

use std::{ffi::OsStr, path::Path};

#[cfg(unix)]
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    fs::{self, DirBuilder, Metadata, Permissions},
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
        net::{SocketAddr, UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
#[cfg(unix)]
use {
    anyhow::{Context, bail, ensure},
    base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
    rusqlite::{Connection, OptionalExtension, params},
    serde::Serialize,
    serde_json::{Map, Value, json},
    sha2::{Digest, Sha256},
    subtle::ConstantTimeEq,
};

use matou_native_core::pty::ShellLaunch;

#[cfg(unix)]
#[path = "host_control/terminal.rs"]
mod terminal;
#[cfg(unix)]
use crate::owner::ShellOwner;
#[cfg(unix)]
use matou_native_core::{
    hierarchy::{self, canvas, cards, tabs, tasks, workspaces},
    storage::{CommandMetadata, CommandReplayConflictError},
};
#[cfg(unix)]
use terminal::{ReadKind, Reply};

const CONTROL_ENV: [&str; 8] = [
    "MATOU_CONTROL_ENDPOINT",
    "MATOU_CONTROL_TOKEN",
    "MATOU_CONTROL_PROTOCOL",
    "MATOU_CONTROL_CALLER_SESSION",
    "MATOU_CONTROL_CALLER_RUN",
    "MATOU_CONTROL_NODE_EXECUTABLE",
    "MATOU_CONTROL_ASSET_ROOT",
    "DSH_MATOU_CONTROL_TOKEN",
];

pub(crate) fn clear_control_environment(launch: &mut ShellLaunch) {
    let control_key = |key: &OsStr| {
        CONTROL_ENV.iter().any(|name| {
            if cfg!(windows) {
                key.to_str()
                    .is_some_and(|key| key.eq_ignore_ascii_case(name))
            } else {
                key == OsStr::new(name)
            }
        })
    };
    launch.inherited_env.retain(|key, _| !control_key(key));
    launch.env.retain(|key, _| !control_key(key));
}

pub(crate) struct RunControl {
    #[cfg(unix)]
    endpoint: PathBuf,
    #[cfg(unix)]
    tokens: Arc<Tokens>,
}

impl RunControl {
    #[cfg(unix)]
    pub(crate) fn inject(
        &self,
        launch: &mut ShellLaunch,
        session_id: &str,
        run_id: &str,
    ) -> Result<()> {
        let token = self
            .tokens
            .issue(run_id, session_id, MANAGED_SCOPES, i64::MAX)?;
        launch.env.insert(
            CONTROL_ENV[0].into(),
            self.endpoint.clone().into_os_string(),
        );
        for (key, value) in [
            (CONTROL_ENV[1], token.as_str()),
            (CONTROL_ENV[2], "1"),
            (CONTROL_ENV[3], session_id),
            (CONTROL_ENV[4], run_id),
        ] {
            launch.env.insert(key.into(), value.into());
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub(crate) fn inject(&self, _: &mut ShellLaunch, _: &str, _: &str) -> Result<()> {
        // No endpoint or capability is published until named pipe access is implemented.
        Ok(())
    }

    pub(crate) fn revoke(&self, run_id: &str) {
        #[cfg(unix)]
        self.tokens
            .records
            .lock()
            .unwrap()
            .retain(|record| record.run_id != run_id);
        #[cfg(not(unix))]
        let _ = run_id;
    }
}

#[cfg(unix)]
pub(crate) const VERSION: u8 = 1;
#[cfg(unix)]
pub(crate) const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
#[cfg(unix)]
const MAX_CONNECTIONS: usize = 16;
#[cfg(unix)]
const OWNER_QUEUE_CAPACITY: usize = 8;
#[cfg(unix)]
const OWNER_BATCH: usize = 4;
#[cfg(unix)]
const IO_POLL: Duration = Duration::from_millis(20);
#[cfg(unix)]
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(unix)]
const MANAGED_SCOPES: &[&str] = &[
    "host.identify",
    "host.list",
    "terminal.read-current",
    "terminal.read-history",
    "terminal.read-commands",
    "navigation.focus.session",
    "navigation.switch.workspace",
    "navigation.switch.task",
    "navigation.switch.canvas",
    "terminal.send-text",
    "terminal.send-key",
    "structure.create.workspace",
    "structure.create.task",
    "structure.create.canvas",
    "structure.create.session",
    "structure.fork.child",
    "structure.fork.sibling",
    "structure.fork.children",
    "structure.remove.preview",
    "structure.remove.commit",
    "structure.canvas-close.preview",
    "structure.canvas-close.commit",
];
#[cfg(unix)]
const OTHER_METHODS: &[&str] = &[
    "task.status.write",
    "task.progress.write",
    "task.log.append",
    "task.move-to-window",
];

#[cfg(unix)]
struct Capability {
    digest: [u8; 32],
    run_id: String,
    session_id: String,
    scopes: &'static [&'static str],
    expires_at: i64,
    runtime_generation: String,
}

#[cfg(unix)]
struct Tokens {
    generation: String,
    records: Mutex<Vec<Capability>>,
}

#[cfg(unix)]
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Caller {
    run_id: String,
    session_id: String,
}

#[cfg(unix)]
impl Tokens {
    fn issue(
        &self,
        run_id: &str,
        session_id: &str,
        scopes: &'static [&'static str],
        expires_at: i64,
    ) -> Result<String> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("generate control capability"))?;
        let token = URL_SAFE_NO_PAD.encode(bytes);
        self.records.lock().unwrap().push(Capability {
            digest: Sha256::digest(token.as_bytes()).into(),
            run_id: run_id.into(),
            session_id: session_id.into(),
            scopes,
            expires_at,
            runtime_generation: self.generation.clone(),
        });
        Ok(token)
    }

    fn has_caller(&self, caller: &Caller, now: i64) -> bool {
        self.records.lock().unwrap().iter().any(|record| {
            record.run_id == caller.run_id
                && record.session_id == caller.session_id
                && record.expires_at >= now
        })
    }

    fn authorize(&self, digest: &[u8; 32], method: &str, now: i64) -> WireResult<Caller> {
        let records = self.records.lock().unwrap();
        // Scan all records so the location of a matching digest doesn't affect the scan.
        let mut matched = None;
        for record in records.iter() {
            if bool::from(record.digest.ct_eq(digest)) {
                matched = Some(record);
            }
        }
        matched
            .filter(|record| {
                record.expires_at >= now
                    && record.runtime_generation == self.generation
                    && record.scopes.contains(&method)
            })
            .map(|record| Caller {
                run_id: record.run_id.clone(),
                session_id: record.session_id.clone(),
            })
            .ok_or(Fault::new(
                "CAPABILITY_DENIED",
                "capability token is missing, expired, or out of scope",
            ))
    }
}

#[cfg(unix)]
#[derive(Serialize)]
struct Fault {
    code: &'static str,
    message: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[cfg(unix)]
impl Fault {
    fn new(code: &'static str, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }
}

#[cfg(unix)]
type WireResult<T> = std::result::Result<T, Fault>;
#[cfg(unix)]
const INVALID: &str = "INVALID_REQUEST";
#[cfg(unix)]
const TIMEOUT: &str = "TIMEOUT";
#[cfg(unix)]
const UNSUPPORTED: &str = "UNSUPPORTED";

#[cfg(unix)]
fn missing_target(message: impl Into<Cow<'static, str>>) -> Fault {
    Fault::new("TARGET_NOT_FOUND", message)
}

#[cfg(unix)]
fn missing_caller() -> Fault {
    missing_target("当前调用会话已不在 Matou 会话图中")
}

#[cfg(unix)]
fn deadline_elapsed() -> Fault {
    Fault::new(TIMEOUT, "request deadline elapsed")
}

#[cfg(unix)]
fn unsupported_method() -> Fault {
    Fault::new(UNSUPPORTED, "unsupported control method")
}

#[cfg(unix)]
fn object(value: &Value) -> WireResult<&Map<String, Value>> {
    value
        .as_object()
        .ok_or(Fault::new(INVALID, "params must be an object"))
}

#[cfg(unix)]
fn bounded_text<'a>(
    value: &'a Value,
    max_bytes: usize,
    message: &'static str,
) -> WireResult<&'a str> {
    value
        .as_str()
        .filter(|text| {
            text.len() <= max_bytes
                && !text
                    .trim_matches(|c: char| c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace()))
                    .is_empty()
        })
        .ok_or(Fault::new(INVALID, message))
}

#[cfg(unix)]
fn integer(value: &Value, max: u64) -> Option<u64> {
    // JSON represents both 1 and 1.0 as the same number in the frozen client.
    let number = value.as_f64()?;
    (number >= 1.0 && number <= max as f64 && number.fract() == 0.0).then_some(number as u64)
}

#[cfg(unix)]
struct Envelope<'a> {
    id: &'a str,
    method: &'a str,
    digest: [u8; 32],
    params: &'a Value,
    deadline: i64,
}

#[cfg(unix)]
impl<'a> Envelope<'a> {
    fn parse(value: &'a Value) -> WireResult<Self> {
        object(value)?;
        if integer(&value["version"], VERSION.into()) != Some(VERSION.into()) {
            return Err(Fault::new(INVALID, "unsupported control protocol version"));
        }
        let method = value["method"]
            .as_str()
            .filter(|method| MANAGED_SCOPES.contains(method) || OTHER_METHODS.contains(method))
            .ok_or_else(unsupported_method)?;
        let id = bounded_text(
            &value["requestId"],
            160,
            "requestId must be a non-empty bounded string",
        )?;
        let token = bounded_text(
            &value["token"],
            512,
            "token must be a non-empty bounded string",
        )?;
        let deadline = integer(&value["deadlineAt"], 9_007_199_254_740_991).ok_or(Fault::new(
            INVALID,
            "deadlineAt must be between 1 and 9007199254740991",
        ))?;
        Ok(Self {
            id,
            method,
            digest: Sha256::digest(token.as_bytes()).into(),
            params: &value["params"],
            deadline: deadline as i64,
        })
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum Scope {
    CurrentLevel,
    All,
}

#[cfg(unix)]
impl Scope {
    fn name(self) -> &'static str {
        match self {
            Self::CurrentLevel => "current-level",
            Self::All => "all",
        }
    }
}

#[cfg(unix)]
enum Action {
    Identify,
    List(Scope),
    Terminal(Selector, ReadKind),
    Navigation(EntityLevel, NavigationSelector),
    Input(Selector, Value),
    Write(Value),
}

#[cfg(unix)]
impl Action {
    fn parse(method: &str, params: &Value) -> WireResult<Self> {
        let fields = object(params)?;
        match method {
            "host.identify" => Ok(Self::Identify),
            "host.list" => match fields.get("scope") {
                None => Ok(Self::List(Scope::CurrentLevel)),
                Some(scope) => match scope.as_str() {
                    Some("current-level") => Ok(Self::List(Scope::CurrentLevel)),
                    Some("all") => Ok(Self::List(Scope::All)),
                    _ => Err(Fault::new(INVALID, "scope is unsupported")),
                },
            },
            "navigation.focus.session"
            | "navigation.switch.workspace"
            | "navigation.switch.task"
            | "navigation.switch.canvas" => {
                if fields.keys().any(|key| key != "target") {
                    return Err(Fault::new(INVALID, "params contains an unsupported field"));
                }
                let level = match method {
                    "navigation.focus.session" => EntityLevel::Session,
                    "navigation.switch.workspace" => EntityLevel::Workspace,
                    "navigation.switch.task" => EntityLevel::Task,
                    _ => EntityLevel::Canvas,
                };
                let target = fields.get("target").unwrap_or(&Value::Null);
                Ok(Self::Navigation(level, NavigationSelector::parse(target)?))
            }
            "terminal.send-text" | "terminal.send-key" => {
                let selector = terminal_input_selector(&params["target"])?;
                Ok(Self::Input(selector, params.clone()))
            }
            method if method.starts_with("structure.") || method.starts_with("task.") => {
                Ok(Self::Write(params.clone()))
            }
            _ => Ok(Self::Terminal(
                Selector::parse(fields.get("target").unwrap_or(&Value::Null), false)?,
                ReadKind::parse(method, fields)?,
            )),
        }
    }
}

#[cfg(unix)]
fn wire_text<'a>(params: &'a Value, field: &str, max: usize) -> WireResult<&'a str> {
    let message = || {
        Fault::new(
            INVALID,
            format!("{field} must be a non-empty bounded string"),
        )
    };
    bounded_text(&params[field], max, "invalid text").map_err(|_| message())
}

#[cfg(unix)]
fn optional_bool(params: &Value, field: &str) -> WireResult<bool> {
    params.get(field).map_or(Ok(false), |value| {
        value
            .as_bool()
            .ok_or_else(|| Fault::new(INVALID, format!("{field} must be a boolean")))
    })
}

#[cfg(unix)]
fn terminal_input_selector(target: &Value) -> WireResult<Selector> {
    let target = object(target)?;
    if let Some(session) = target.get("sessionId").filter(|value| value.is_string()) {
        return Selector::parse(&json!({"sessionId": session}), false);
    }
    let fields: &[&str] = match target.get("kind").and_then(Value::as_str) {
        Some("self") => &["kind"],
        Some("relative") => &["kind", "direction"],
        Some("relation") => &["kind", "relation", "ordinal"],
        Some("sibling") => &["kind", "ordinal", "projectionRevision"],
        Some("ref") => &["kind", "ref", "projectionRevision"],
        Some("session") => &["kind", "sessionId"],
        None if !target.contains_key("kind") => &["ref", "projectionRevision"],
        _ => return Err(Fault::new(INVALID, "target.kind is unsupported")),
    };
    let selector = target
        .iter()
        .filter(|(key, _)| fields.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Selector::parse(&Value::Object(selector), false)
}

#[cfg(unix)]
fn control_key(key: &str) -> Option<&'static [u8]> {
    Some(match key {
        "Enter" => b"\r",
        "Tab" => b"\t",
        "Escape" => b"\x1b",
        "Backspace" => b"\x7f",
        "Delete" => b"\x1b[3~",
        "ArrowUp" => b"\x1b[A",
        "ArrowDown" => b"\x1b[B",
        "ArrowLeft" => b"\x1b[D",
        "ArrowRight" => b"\x1b[C",
        "Home" => b"\x1b[H",
        "End" => b"\x1b[F",
        "PageUp" => b"\x1b[5~",
        "PageDown" => b"\x1b[6~",
        "CtrlC" => b"\x03",
        "CtrlD" => b"\x04",
        "CtrlL" => b"\x0c",
        "CtrlU" => b"\x15",
        "CtrlZ" => b"\x1a",
        _ => return None,
    })
}

#[cfg(unix)]
pub(crate) enum ControlUpdate {
    Navigation(PendingNavigation),
    Changed(Vec<String>, Vec<String>),
}

#[cfg(unix)]
enum InputStep {
    Body(Vec<u8>),
    Acknowledge(Receiver<Result<()>>, bool),
    Echo(Instant),
}

#[cfg(unix)]
struct TerminalWrite {
    run: String,
    submit: bool,
    step: InputStep,
}

#[cfg(unix)]
fn read_only_fault() -> Fault {
    Fault::new("STORAGE_READ_ONLY", "Matou storage is read-only")
}

#[cfg(unix)]
fn input_not_ready() -> Fault {
    Fault::new("TARGET_NOT_READY", "目标会话没有可写的运行中终端")
}

#[cfg(unix)]
fn terminal_write_error(error: anyhow::Error) -> Fault {
    match error.to_string().as_str() {
        "session storage is paused" => read_only_fault(),
        "session output is frozen" => Fault::new(
            "TARGET_NOT_READY",
            "session is paused because the network exit failed a safety check",
        ),
        _ => input_not_ready(),
    }
}

#[cfg(unix)]
impl TerminalWrite {
    fn advance(
        &mut self,
        request: &Pending,
        state: &ServerState,
        owner: &crate::owner::ShellOwner,
        session: &str,
    ) -> WireResult<Option<Reply>> {
        if let InputStep::Acknowledge(result, false) = &self.step {
            return match result.try_recv() {
                Ok(Ok(())) => serialize_ok(&request.id, json!({"sent": true}))
                    .map(Reply::Ready)
                    .map(Some),
                Ok(Err(error)) => Err(terminal_write_error(error)),
                Err(mpsc::TryRecvError::Disconnected) => Err(input_not_ready()),
                Err(mpsc::TryRecvError::Empty)
                    if (state.clock)() > request.deadline.saturating_add(1_000) =>
                {
                    Err(deadline_elapsed())
                }
                Err(mpsc::TryRecvError::Empty) => Ok(None),
            };
        }
        if matches!(self.step, InputStep::Body(_)) {
            state.authorize(&request.digest, &request.method, request.deadline)?;
        } else {
            // Once the body reaches the PTY, give Enter the entire quiet window.
            // Capability expiry and Run replacement still stop the second write.
            state
                .tokens
                .authorize(&request.digest, &request.method, (state.clock)())?;
        }
        if owner.control_is_read_only().map_err(write_error)? {
            return Err(read_only_fault());
        }
        let (_, last_output) = owner
            .control_terminal(session)
            .filter(|(run, _)| *run == self.run)
            .ok_or_else(input_not_ready)?;
        let write = |bytes, submit_afterward| {
            let (reply, result) = mpsc::channel();
            owner
                .send(
                    session,
                    crate::worker::WorkerCommand::Write { bytes, reply },
                )
                .map_err(|_| input_not_ready())?;
            Ok(InputStep::Acknowledge(result, submit_afterward))
        };
        self.step = match &mut self.step {
            InputStep::Body(bytes) => write(std::mem::take(bytes), self.submit)?,
            InputStep::Acknowledge(result, submit) => match result.try_recv() {
                Ok(Ok(())) if *submit => InputStep::Echo(Instant::now()),
                Ok(Ok(())) => {
                    return serialize_ok(&request.id, json!({"sent": true}))
                        .map(Reply::Ready)
                        .map(Some);
                }
                Ok(Err(error)) => return Err(terminal_write_error(error)),
                Err(mpsc::TryRecvError::Disconnected) => return Err(input_not_ready()),
                Err(mpsc::TryRecvError::Empty) => return Ok(None),
            },
            InputStep::Echo(started) => {
                let elapsed = started.elapsed();
                let quiet =
                    last_output.is_none_or(|output| output.elapsed() >= Duration::from_millis(250));
                if elapsed < Duration::from_millis(120)
                    || (!quiet && elapsed < Duration::from_secs(5))
                {
                    return Ok(None);
                }
                write(vec![b'\r'], false)?
            }
        };
        Ok(None)
    }
}

#[cfg(unix)]
struct ControlInput {
    request: Pending,
    write: TerminalWrite,
}

#[cfg(unix)]
struct Pending {
    digest: [u8; 32],
    method: String,
    id: String,
    deadline: i64,
    action: Action,
    reply: SyncSender<WireResult<Reply>>,
    response_written: Arc<AtomicBool>,
}

#[cfg(unix)]
pub(crate) struct PendingNavigation {
    request: Pending,
    path: NavigationPath,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NavigationPath {
    route_window_id: String,
    target_window_id: String,
    workspace_id: String,
    task_id: String,
    pub(crate) scene_id: String,
    pub(crate) session_id: String,
}

#[cfg(unix)]
impl PendingNavigation {
    pub(crate) fn complete(
        self,
        window_id: &str,
        apply: impl FnOnce(&NavigationPath) -> Result<()>,
    ) {
        let result = if self.path.route_window_id != window_id {
            Err(navigation_not_ready())
        } else {
            apply(&self.path)
                .map_err(|error| {
                    if error
                        .downcast_ref::<rusqlite::Error>()
                        .is_some_and(|error| {
                            error.sqlite_error_code() == Some(rusqlite::ErrorCode::ReadOnly)
                        })
                    {
                        Fault::new("STORAGE_READ_ONLY", "Matou storage is read-only")
                    } else {
                        eprintln!("[host-control] navigation failed: {error:#}");
                        Fault::new("INTERNAL_ERROR", "could not commit host navigation")
                    }
                })
                .and_then(|()| {
                    serialize_ok(
                        &self.request.id,
                        json!({"kind": "navigated", "finalPath": self.path}),
                    )
                    .map(Reply::Ready)
                })
        };
        let _ = self.request.reply.send(result);
    }
}

#[cfg(unix)]
type Clock = fn() -> i64;

#[cfg(unix)]
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(unix)]
struct ServerState {
    stopping: AtomicBool,
    tokens: Arc<Tokens>,
    clock: Clock,
    idle: Duration,
}

#[cfg(unix)]
impl ServerState {
    fn authorize(&self, digest: &[u8; 32], method: &str, deadline: i64) -> WireResult<Caller> {
        let now = (self.clock)();
        if now > deadline {
            return Err(deadline_elapsed());
        }
        self.tokens.authorize(digest, method, now)
    }

    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }
}

#[cfg(unix)]
pub(crate) struct ControlServer {
    endpoint: Endpoint,
    listener: Option<JoinHandle<()>>,
    pending: Option<Receiver<Pending>>,
    state: Arc<ServerState>,
    reader: Connection,
    root: PathBuf,
    inputs: HashMap<String, VecDeque<ControlInput>>,
    confirmations: HashMap<String, Confirmation>,
    forks: Vec<ForkWait>,
    disposals: VecDeque<(Arc<AtomicBool>, Vec<String>)>,
    #[cfg(test)]
    projection_aliases: Option<fn(&mut [Target])>,
    #[cfg(test)]
    topology_reads: std::cell::Cell<usize>,
    #[cfg(test)]
    resolved_reads: std::cell::Cell<usize>,
}

#[cfg(unix)]
impl ControlServer {
    pub(crate) fn start(root: &Path, generation: &str) -> Result<Self> {
        let path = root.join("matou.sqlite");
        let reader = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("open host control topology at {}", path.display()))?;
        reader.busy_timeout(Duration::ZERO)?;
        Self::listen(root, generation, reader, now_ms, IDLE_TIMEOUT)
    }

    fn listen(
        root: &Path,
        generation: &str,
        reader: Connection,
        clock: Clock,
        idle: Duration,
    ) -> Result<Self> {
        let path = root.join("control/runtime.sock");
        let (endpoint, listener) = Endpoint::bind(&path)
            .with_context(|| format!("start host control at {}", path.display()))?;
        let (send, pending) = mpsc::sync_channel(OWNER_QUEUE_CAPACITY);
        let state = Arc::new(ServerState {
            stopping: AtomicBool::new(false),
            tokens: Arc::new(Tokens {
                generation: generation.into(),
                records: Mutex::new(Vec::new()),
            }),
            clock,
            idle,
        });
        let shared = state.clone();
        let listener = thread::Builder::new()
            .name("host-control-listener".into())
            .spawn(move || accept_connections(listener, &shared, &send))
            .with_context(|| format!("start host control listener at {}", path.display()))?;
        Ok(Self {
            endpoint,
            listener: Some(listener),
            pending: Some(pending),
            state,
            reader,
            root: root.to_owned(),
            inputs: HashMap::new(),
            confirmations: HashMap::new(),
            forks: Vec::new(),
            disposals: VecDeque::new(),
            #[cfg(test)]
            projection_aliases: None,
            #[cfg(test)]
            topology_reads: std::cell::Cell::new(0),
            #[cfg(test)]
            resolved_reads: std::cell::Cell::new(0),
        })
    }

    pub(crate) fn runs(&self) -> RunControl {
        RunControl {
            endpoint: self.endpoint.path.clone(),
            tokens: self.state.tokens.clone(),
        }
    }

    pub(crate) fn poll(
        &mut self,
        owner: &mut crate::owner::ShellOwner,
        window_id: &str,
    ) -> Option<ControlUpdate> {
        if let Some(index) = self
            .disposals
            .iter()
            .position(|(written, _)| written.load(Ordering::Acquire))
        {
            let (_, disposed) = self.disposals.remove(index).expect("ready disposals");
            owner.retire_sessions(&disposed);
            return Some(ControlUpdate::Changed(disposed, Vec::new()));
        }
        self.poll_inputs(owner);
        self.poll_forks(owner);
        for _ in 0..OWNER_BATCH {
            let Ok(request) = self.pending.as_ref()?.try_recv() else {
                break;
            };
            let result = match &request.action {
                Action::Navigation(level, selector) => {
                    match self.prepare_navigation(&request, *level, selector) {
                        Ok(path) => {
                            return Some(ControlUpdate::Navigation(PendingNavigation {
                                request,
                                path,
                            }));
                        }
                        Err(error) => Err(error),
                    }
                }
                Action::Input(..) => {
                    self.queue_input(request, owner);
                    continue;
                }
                Action::Write(params) => {
                    let result = self.dispatch_write(&request, params, owner, window_id);
                    match result {
                        Ok((value, changed)) => {
                            if let Some((disposed, _)) = &changed
                                && !disposed.is_empty()
                            {
                                self.disposals.push_back((
                                    request.response_written.clone(),
                                    disposed.clone(),
                                ));
                                let _ = request
                                    .reply
                                    .send(serialize_ok(&request.id, value).map(Reply::Ready));
                                continue;
                            }
                            let wait = value["kind"] == "fork-batch"
                                || params["start"] == true && value["kind"] == "forked";
                            let changed = changed.map(|(disposed, jobs)| {
                                let launches = jobs
                                    .iter()
                                    .filter(|job| job.spawn)
                                    .map(|job| job.session.clone())
                                    .collect::<Vec<_>>();
                                (disposed, launches, jobs)
                            });
                            let jobs = changed
                                .as_ref()
                                .map(|(_, _, jobs)| jobs.as_slice())
                                .unwrap_or_default();
                            if wait && !jobs.is_empty() {
                                self.forks.push(ForkWait::new(request, value, jobs));
                            } else {
                                let _ = request
                                    .reply
                                    .send(serialize_ok(&request.id, value).map(Reply::Ready));
                            }
                            if let Some((disposed, launches, _)) = changed
                                && (!disposed.is_empty() || !launches.is_empty() || !wait)
                            {
                                return Some(ControlUpdate::Changed(disposed, launches));
                            }
                            continue;
                        }
                        Err(error) => Err(error),
                    }
                }
                _ => self.dispatch(&request, owner),
            };
            let _ = request.reply.send(result);
        }
        None
    }

    fn queue_input(&mut self, mut request: Pending, owner: &crate::owner::ShellOwner) {
        let prepared = (|| {
            let caller =
                self.state
                    .authorize(&request.digest, &request.method, request.deadline)?;
            if owner.control_is_read_only().map_err(write_error)? {
                return Err(read_only_fault());
            }
            let Action::Input(selector, params) = &mut request.action else {
                unreachable!()
            };
            let targets = self.targets()?;
            let target = selector.resolve(&targets, &caller)?;
            let (bytes, submit) = if request.method == "terminal.send-key" {
                let bytes = control_key(params["key"].as_str().unwrap_or_default())
                    .ok_or_else(|| Fault::new(UNSUPPORTED, "control key is not allowlisted"))?;
                (bytes.to_vec(), false)
            } else {
                (
                    wire_text(params, "text", 64 * 1024)?.as_bytes().to_vec(),
                    optional_bool(params, "submit")?,
                )
            };
            // The pending write only needs its authority and small byte payload.
            *params = Value::Null;
            let (run, _) = owner
                .control_terminal(&target.session_id)
                .ok_or_else(input_not_ready)?;
            if self.inputs.values().map(VecDeque::len).sum::<usize>() >= MAX_CONNECTIONS {
                return Err(Fault::new(TIMEOUT, "host control input queue is full"));
            }
            Ok((target.session_id.clone(), run.to_owned(), bytes, submit))
        })();
        match prepared {
            Ok((session, run, bytes, submit)) => {
                self.inputs
                    .entry(session)
                    .or_default()
                    .push_back(ControlInput {
                        request,
                        write: TerminalWrite {
                            run,
                            submit,
                            step: InputStep::Body(bytes),
                        },
                    });
            }
            Err(error) => {
                let _ = request.reply.send(Err(error));
            }
        }
    }

    fn poll_inputs(&mut self, owner: &crate::owner::ShellOwner) {
        self.inputs.retain(|session, queue| {
            if self.forks.iter().any(|fork| {
                fork.items
                    .iter()
                    .any(|item| !item.done && item.session == *session && item.write.is_some())
            }) {
                return true;
            }
            if let Some(input) = queue.front_mut() {
                let result = match input
                    .write
                    .advance(&input.request, &self.state, owner, session)
                {
                    Ok(None) => return true,
                    Ok(Some(result)) => Ok(result),
                    Err(error) => Err(error),
                };
                let _ = queue
                    .pop_front()
                    .expect("input at front")
                    .request
                    .reply
                    .send(result);
            }
            !queue.is_empty()
        });
    }

    fn poll_forks(&mut self, owner: &mut ShellOwner) {
        self.forks.retain_mut(|fork| {
            let result = match fork.advance(&self.state, owner, &self.inputs) {
                Ok(false) => return true,
                Ok(true) => serialize_ok(&fork.request.id, &fork.result).map(Reply::Ready),
                Err(error) => Err(error),
            };
            let _ = fork.request.reply.send(result);
            false
        });
    }

    fn dispatch_write(
        &mut self,
        request: &Pending,
        params: &Value,
        owner: &mut ShellOwner,
        window_id: &str,
    ) -> WireResult<(Value, Option<(Vec<String>, Vec<ForkLaunch>)>)> {
        let caller = self
            .state
            .authorize(&request.digest, &request.method, request.deadline)?;
        let method = request.method.as_str();
        if method.starts_with("structure.fork.") && self.forks.len() >= MAX_CONNECTIONS {
            return Err(Fault::new(TIMEOUT, "host control fork queue is full"));
        }
        if owner.control_is_read_only().map_err(write_error)? && !method.ends_with(".preview") {
            return Err(read_only_fault());
        }
        let now = (self.state.clock)();
        if method.starts_with("task.") {
            return task_write(owner.control_storage(), method, params, now)
                .map(|value| (value, None));
        }
        let targets = self.targets()?;
        let result = match method {
            "structure.remove.preview" | "structure.canvas-close.preview" => {
                let close = method == "structure.canvas-close.preview";
                strict_fields(
                    params,
                    if close {
                        &["target"]
                    } else {
                        &["target", "scope"]
                    },
                )?;
                let scope = if close {
                    "subtree"
                } else {
                    choice(params, "scope", &["node", "subtree"])?
                };
                let target = structure_target(&targets, &caller, &params["target"])?;
                if close {
                    require_level(&target, EntityLevel::Canvas)?;
                } else {
                    target.validate_removal(&self.reader)?;
                }
                let impact = target.impact(&self.reader, scope, close)?;
                self.confirmations.retain(|_, item| {
                    item.expires_at > now && self.state.tokens.has_caller(&item.caller, now)
                });
                let reference = uuid::Uuid::new_v4().to_string();
                self.confirmations.insert(
                    reference.clone(),
                    Confirmation {
                        caller,
                        close,
                        level: target.level,
                        id: target.id().to_owned(),
                        revision: revision(&targets.iter().collect::<Vec<_>>()),
                        impact: impact.clone(),
                        expires_at: now + 120_000,
                    },
                );
                let kind = if close {
                    "canvas-close-preview"
                } else {
                    "removal-preview"
                };
                (
                    json!({"kind": kind, "impact": impact, "confirmationRef": reference}),
                    None,
                )
            }
            "structure.remove.commit" | "structure.canvas-close.commit" => {
                strict_fields(params, &["confirmationRef"])?;
                let reference = action_text(params, "confirmationRef", 160)?;
                let record = self.confirmations.get(reference).cloned();
                self.confirmations.retain(|_, item| {
                    item.expires_at > now && self.state.tokens.has_caller(&item.caller, now)
                });
                let record = record.ok_or_else(|| confirmation_required(false))?;
                if record.caller != caller {
                    return Err(confirmation_required(true));
                }
                if now >= record.expires_at {
                    return Err(Fault::new("CONFIRMATION_EXPIRED", "确认已过期，请重新预览"));
                }
                let close = method == "structure.canvas-close.commit";
                if record.close != close {
                    return Err(Fault::new(
                        "CONFIRMATION_STALE",
                        "确认对应的操作已变化，请重新预览",
                    ));
                }
                let target = targets
                    .iter()
                    .find(|target| record.level.id(target) == record.id)
                    .map(|target| StructureTarget {
                        level: record.level,
                        target,
                    })
                    .ok_or_else(confirmation_stale)?;
                let scope = record.impact["scope"].as_str().expect("preview scope");
                let impact = target.impact(&self.reader, scope, close)?;
                if record.revision != revision(&targets.iter().collect::<Vec<_>>())
                    || record.impact != impact
                {
                    return Err(confirmation_stale());
                }
                self.state
                    .authorize(&request.digest, method, request.deadline)?;
                let _ = self.confirmations.remove(reference);
                let command = metadata(
                    &caller,
                    method,
                    format!("host-confirmation:{reference}"),
                    &impact,
                );
                let result = target.commit(owner, &command, &impact, close, now)?;
                (result.0, Some((result.1, Vec::new())))
            }
            "structure.create.workspace"
            | "structure.create.task"
            | "structure.create.canvas"
            | "structure.create.session" => {
                let (value, changed) =
                    create_structure(owner, &targets, &caller, method, params, window_id, now)?;
                (value, changed.then(|| (Vec::new(), Vec::new())))
            }
            _ => {
                let (value, launches) =
                    fork_structure(owner, &targets, &caller, method, params, now)?;
                (
                    value,
                    (!launches.is_empty()).then(|| (Vec::new(), launches)),
                )
            }
        };
        Ok(result)
    }

    fn prepare_navigation(
        &self,
        request: &Pending,
        level: EntityLevel,
        selector: &NavigationSelector,
    ) -> WireResult<NavigationPath> {
        let caller = self
            .state
            .authorize(&request.digest, &request.method, request.deadline)?;
        let targets = self.targets()?;
        let path = selector.resolve(&self.reader, &targets, &caller, level)?;
        // Resolution can read multiple tables. Recheck before touching window state.
        self.state
            .authorize(&request.digest, &request.method, request.deadline)?;
        Ok(path)
    }

    fn targets(&self) -> WireResult<Vec<Target>> {
        let targets = read_topology(&self.reader)
            .map_err(|_| Fault::new("INTERNAL_ERROR", "could not read host control topology"))?;
        #[cfg(test)]
        let targets = {
            self.topology_reads.set(self.topology_reads.get() + 1);
            let mut targets = targets;
            if let Some(project) = self.projection_aliases {
                project(&mut targets);
            }
            targets
        };
        Ok(targets)
    }

    fn dispatch(&self, request: &Pending, owner: &crate::owner::ShellOwner) -> WireResult<Reply> {
        let caller = self
            .state
            .authorize(&request.digest, &request.method, request.deadline)?;
        let targets = self.targets()?;
        let result = match &request.action {
            Action::Identify => {
                #[derive(Serialize)]
                struct Identified<'a> {
                    caller: Caller,
                    target: &'a Target,
                }
                let target = Selector::SelfTarget.resolve(&targets, &caller)?;
                serialize_ok(&request.id, Identified { caller, target }).map(Reply::Ready)
            }
            Action::List(scope) => {
                #[derive(Serialize)]
                #[serde(rename_all = "camelCase")]
                struct Listed<'a> {
                    projection_revision: String,
                    scope: &'static str,
                    targets: Vec<&'a Target>,
                }
                let visible = projection(&targets, &caller, *scope)?;
                serialize_ok(
                    &request.id,
                    Listed {
                        projection_revision: revision(&visible),
                        scope: scope.name(),
                        targets: visible,
                    },
                )
                .map(Reply::Ready)
            }
            Action::Terminal(selector, kind) => {
                let target = selector.resolve(&targets, &caller)?;
                let kind = kind.validate()?;
                #[cfg(test)]
                self.resolved_reads.set(self.resolved_reads.get() + 1);
                terminal::prepare(
                    &request.id,
                    &self.root,
                    &self.reader,
                    owner,
                    &target.session_id,
                    kind,
                )
            }
            Action::Navigation(..) | Action::Input(..) | Action::Write(..) => {
                unreachable!("mutations are dispatched separately")
            }
        };
        if result.is_ok() && (self.state.clock)() > request.deadline {
            return Err(deadline_elapsed());
        }
        result
    }
}

#[cfg(unix)]
impl Drop for ControlServer {
    fn drop(&mut self) {
        self.state.stopping.store(true, Ordering::Release);
        self.pending.take();
        if let Some(listener) = self.listener.take() {
            listener.thread().unpark();
            let _ = listener.join();
        }
        self.state.tokens.records.lock().unwrap().clear();
        // Endpoint drops after the listener and all client sockets close.
    }
}

#[cfg(unix)]
struct Endpoint {
    path: PathBuf,
    identity: (u64, u64),
}

#[cfg(unix)]
impl Endpoint {
    fn bind(path: &Path) -> Result<(Self, UnixListener)> {
        SocketAddr::from_pathname(path).context("invalid or overlong Unix socket path")?;
        let directory = path.parent().context("control directory is missing")?;
        match DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata(directory)?;
        // SAFETY: geteuid reads the current process credential and takes no arguments.
        let uid = unsafe { libc::geteuid() };
        ensure!(
            metadata.is_dir() && metadata.uid() == uid,
            "control directory is not an owned directory"
        );
        fs::set_permissions(directory, Permissions::from_mode(0o700))?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                ensure!(
                    metadata.file_type().is_socket() && metadata.uid() == uid,
                    "existing control path is not an owned socket"
                );
                // Probe without a blocking connect against a full listener backlog.
                let probe = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let result = probe.block_on(async {
                    tokio::time::timeout(
                        Duration::from_millis(250),
                        tokio::net::UnixStream::connect(path),
                    )
                    .await
                });
                match result {
                    Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                        ensure!(
                            socket_identity(&fs::symlink_metadata(path)?)
                                == socket_identity(&metadata),
                            "existing control socket changed during probe"
                        );
                        fs::remove_file(path)?;
                    }
                    Ok(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                        bail!("control socket is already in use")
                    }
                    Ok(Err(error)) => return Err(error).context("probe existing control socket"),
                    _ => bail!("control socket is already in use"),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(path).context("bind Unix socket")?;
        let endpoint = Self {
            path: path.into(),
            identity: socket_identity(&fs::symlink_metadata(path)?)
                .context("bound path is not a socket")?,
        };
        fs::set_permissions(path, Permissions::from_mode(0o600))
            .context("set control socket permissions")?;
        listener.set_nonblocking(true)?;
        Ok((endpoint, listener))
    }
}

#[cfg(unix)]
fn socket_identity(metadata: &Metadata) -> Option<(u64, u64)> {
    metadata
        .file_type()
        .is_socket()
        .then(|| (metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
impl Drop for Endpoint {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .ok()
            .as_ref()
            .and_then(socket_identity)
            == Some(self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn accept_connections(
    listener: UnixListener,
    state: &Arc<ServerState>,
    sender: &SyncSender<Pending>,
) {
    let mut clients: Vec<(UnixStream, JoinHandle<()>)> = Vec::new();
    while !state.stopping() {
        clients.retain_mut(|(_, worker)| !worker.is_finished());
        match listener.accept() {
            Ok((stream, _)) if clients.len() < MAX_CONNECTIONS => {
                let Ok(shutdown) = stream.try_clone() else {
                    continue;
                };
                let state = state.clone();
                let sender = sender.clone();
                if let Ok(worker) = thread::Builder::new()
                    .name("host-control-client".into())
                    .spawn(move || serve(stream, &state, &sender))
                {
                    clients.push((shutdown, worker));
                }
            }
            Ok(_) => {} // No request was read, so there is no response ID.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::park_timeout(IO_POLL)
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    drop(listener);
    for (socket, _) in &clients {
        let _ = socket.shutdown(Shutdown::Both);
    }
    for (_, worker) in clients {
        let _ = worker.join();
    }
}

#[cfg(unix)]
fn serve(mut socket: UnixStream, state: &ServerState, sender: &SyncSender<Pending>) {
    let _ = serve_frames(&mut socket, state, sender);
}

#[cfg(unix)]
fn serve_frames(
    socket: &mut UnixStream,
    state: &ServerState,
    sender: &SyncSender<Pending>,
) -> io::Result<()> {
    socket.set_read_timeout(Some(IO_POLL))?;
    socket.set_write_timeout(Some(IO_POLL))?;
    while !state.stopping() {
        let until = Instant::now() + state.idle;
        let mut prefix = [0; 4];
        transfer(&mut prefix, until, state, |part| socket.read(part))?;
        let size = u32::from_be_bytes(prefix) as usize;
        if size > MAX_FRAME_BYTES {
            return write_frame(
                socket,
                state,
                "unknown",
                Err(Fault::new(INVALID, "control frame exceeds size limit")),
            );
        }
        let mut body = vec![0; size];
        transfer(&mut body, until, state, |part| socket.read(part))?;
        let value = match serde_json::from_slice::<Value>(&body) {
            Ok(value) => value,
            Err(_) => {
                write_frame(
                    socket,
                    state,
                    "unknown",
                    Err(Fault::new(INVALID, "control frame is not valid JSON")),
                )?;
                continue;
            }
        };
        match Envelope::parse(&value) {
            Ok(request) => {
                let written = Arc::new(AtomicBool::new(false));
                let result = enqueue(&request, state, sender, written.clone());
                let result = write_frame(socket, state, request.id, result);
                written.store(true, Ordering::Release);
                result?;
            }
            Err(error) => write_frame(socket, state, "unknown", Err(error))?,
        }
    }
    Ok(())
}

#[cfg(unix)]
fn transfer(
    mut bytes: &mut [u8],
    until: Instant,
    state: &ServerState,
    mut operation: impl FnMut(&mut [u8]) -> io::Result<usize>,
) -> io::Result<()> {
    while !bytes.is_empty() {
        if state.stopping() || Instant::now() >= until {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match operation(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn enqueue(
    request: &Envelope<'_>,
    state: &ServerState,
    sender: &SyncSender<Pending>,
    response_written: Arc<AtomicBool>,
) -> WireResult<Vec<u8>> {
    state.authorize(&request.digest, request.method, request.deadline)?;
    let action = Action::parse(request.method, request.params)?;
    let (reply, response) = mpsc::sync_channel(1);
    let pending = Pending {
        digest: request.digest,
        method: request.method.into(),
        id: request.id.into(),
        deadline: request.deadline,
        action,
        reply,
        response_written,
    };
    match sender.try_send(pending) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            return Err(Fault::new(TIMEOUT, "host control queue is full"));
        }
        Err(TrySendError::Disconnected(_)) => {
            return Err(Fault::new(TIMEOUT, "host control is stopping"));
        }
    }
    while !state.stopping() {
        match response.recv_timeout(IO_POLL) {
            Ok(Ok(Reply::Ready(body)))
                if request.method.starts_with("structure.")
                    && !request.method.ends_with(".preview") =>
            {
                return Ok(body);
            }
            Ok(result) => return result?.complete(request, state),
            Err(mpsc::RecvTimeoutError::Timeout) if (state.clock)() > request.deadline => {
                return Err(deadline_elapsed());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Err(Fault::new(TIMEOUT, "host control is stopping"))
}

#[cfg(unix)]
fn serialize_ok(id: &str, result: impl Serialize) -> WireResult<Vec<u8>> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Response<'a, T> {
        version: u8,
        request_id: &'a str,
        ok: bool,
        result: T,
    }
    struct LimitedFrame(Vec<u8>);
    impl Write for LimitedFrame {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.len() + bytes.len() > MAX_FRAME_BYTES {
                return Err(io::ErrorKind::InvalidData.into());
            }
            self.0.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut body = LimitedFrame(Vec::new());
    serde_json::to_writer(
        &mut body,
        &Response {
            version: VERSION,
            request_id: id,
            ok: true,
            result,
        },
    )
    .map_err(|_| Fault::new("INTERNAL_ERROR", "control response exceeds size limit"))?;
    Ok(body.0)
}

#[cfg(unix)]
fn write_frame(
    socket: &mut UnixStream,
    state: &ServerState,
    id: &str,
    result: WireResult<Vec<u8>>,
) -> io::Result<()> {
    let mut body = match result {
        Ok(body) => body,
        Err(error) => serde_json::to_vec(&json!({
            "version": VERSION, "requestId": id, "ok": false, "error": error
        }))
        .map_err(io::Error::other)?,
    };
    let until = Instant::now() + state.idle;
    transfer(
        &mut (body.len() as u32).to_be_bytes(),
        until,
        state,
        |part| socket.write(part),
    )?;
    transfer(&mut body, until, state, |part| socket.write(part))
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    #[serde(rename = "ref")]
    reference: String,
    workspace_id: String,
    task_id: String,
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mount_id: Option<String>,
    title: String,
    profile: String,
    cwd: String,
    work_status: String,
    environment: Map<String, Value>,
    window: Value,
    workspace: Value,
    task: Value,
    canvas: Value,
    session: Value,
    dag: Dag,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Dag {
    depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_ref: Option<String>,
    child_refs: Vec<String>,
    #[serde(serialize_with = "serialize_siblings")]
    sibling_refs: Arc<Vec<String>>,
}

#[cfg(unix)]
fn serialize_siblings<S: serde::Serializer>(
    siblings: &Arc<Vec<String>>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    siblings.as_slice().serialize(serializer)
}

#[cfg(unix)]
fn session_ref(id: &str) -> String {
    format!("session:{id}")
}

#[cfg(unix)]
fn read_topology(connection: &Connection) -> rusqlite::Result<Vec<Target>> {
    // One SELECT establishes the read snapshot for both ranks and all selector inputs.
    let mut statement = connection.prepare_cached(
        "WITH active AS (
            SELECT s.id AS session_id, s.title, s.kind, s.cwd, s.work_status,
                   w.id AS workspace_id, w.name AS workspace_name, w.is_pinned,
                   w.pin_sort_key, w.last_opened_at, w.created_at AS workspace_created_at,
                   t.id AS task_id, t.title AS task_name, p.ordinal AS task_placement,
                   t.sort_key AS task_sort, t.created_at AS task_created_at,
                   c.id AS canvas_id, c.name AS canvas_name, c.sort_key AS canvas_sort,
                   c.created_at AS canvas_created_at, mount.id AS mount_id,
                   coalesce(d.native_window_key, p.window_id, 'unplaced:' || t.id) AS window_id,
                   d.native_window_key IS NOT NULL AS detached,
                   relation.to_session_id AS parent_id, m.last_user_interaction_seq,
                   m.sibling_created_seq, e.id AS context_id, e.kind AS context_kind,
                   git.state AS git_state, coalesce(git.branch, tree.branch_name) AS branch,
                   tree.id AS worktree_id
            FROM session_canvas_memberships AS m
            JOIN sessions AS s ON s.id=m.session_id AND s.archived_at IS NULL
            JOIN scenes AS c ON c.id=m.scene_id AND c.archived_at IS NULL
            JOIN tasks AS t ON t.id=c.task_id AND t.archived_at IS NULL
            JOIN workspaces AS w ON w.id=t.workspace_id AND w.archived_at IS NULL
            JOIN execution_contexts AS e ON e.id=s.execution_context_id AND e.archived_at IS NULL
            LEFT JOIN execution_context_git_states AS git ON git.execution_context_id=e.id
            LEFT JOIN worktrees AS tree ON tree.execution_context_id=e.id AND tree.state<>'removed'
            LEFT JOIN session_relations_current AS relation ON relation.from_session_id=s.id
                AND relation.relation_kind IN ('forked-from','derived-from')
            LEFT JOIN session_mounts AS mount ON mount.id=(
                SELECT id FROM session_mounts WHERE scene_id=c.id AND session_id=s.id
                ORDER BY created_at,id LIMIT 1
            )
            LEFT JOIN scene_windows AS d ON d.id=mount.scene_window_id AND d.state='detached'
            LEFT JOIN window_task_placements AS p ON p.task_id=t.id
        ), windows AS (
            SELECT id, row_number() OVER (ORDER BY created_at,id) AS ordinal
            FROM app_windows WHERE state<>'closed'
        )
        SELECT active.*,
            coalesce(windows.ordinal, (SELECT count(*)+1 FROM windows)) AS window_rank,
            dense_rank() OVER (ORDER BY is_pinned DESC,
                CASE WHEN is_pinned<>0 THEN pin_sort_key END,
                CASE WHEN is_pinned=0 THEN last_opened_at END DESC,
                workspace_created_at DESC,workspace_id) AS workspace_rank,
            coalesce(task_placement+1, dense_rank() OVER (PARTITION BY workspace_id
                ORDER BY task_placement NULLS LAST,task_sort,task_created_at,task_id)) AS task_rank,
            dense_rank() OVER (PARTITION BY task_id
                ORDER BY canvas_sort,canvas_created_at,canvas_id) AS canvas_rank,
            row_number() OVER (PARTITION BY canvas_id,parent_id
                ORDER BY last_user_interaction_seq DESC,sibling_created_seq,session_id) AS session_rank
        FROM active LEFT JOIN windows ON windows.id=active.window_id
        ORDER BY canvas_id,parent_id,session_rank",
    )?;
    let mut targets = statement
        .query_map([], |row| {
            let session_id = row.get::<_, String>("session_id")?;
            let workspace_id = row.get::<_, String>("workspace_id")?;
            let task_id = row.get::<_, String>("task_id")?;
            let canvas_id = row.get::<_, String>("canvas_id")?;
            let detached = row.get::<_, bool>("detached")?;
            let parent_id = row.get::<_, Option<String>>("parent_id")?;
            let profile = row.get::<_, String>("kind")?;
            let mode = match row.get::<_, String>("context_kind")?.as_str() {
                "git-worktree" => "git-worktree",
                _ if row.get::<_, Option<String>>("git_state")?.as_deref() == Some("ready") => "git-checkout",
                _ => "directory",
            };
            let mut environment = Map::from_iter([
                ("executionContextRef".into(), json!(format!("context:{}", row.get::<_, String>("context_id")?))),
                ("mode".into(), json!(mode)),
            ]);
            if let Some(branch) = row.get::<_, Option<String>>("branch")? {
                environment.insert("branch".into(), json!(branch));
            }
            if let Some(worktree) = row.get::<_, Option<String>>("worktree_id")? {
                environment.insert("worktreeRef".into(), json!(format!("worktree:{worktree}")));
            }
            Ok(Target {
                reference: session_ref(&session_id),
                session: json!({ "id": session_id, "ordinal": row.get::<_, i64>("session_rank")?, "detached": detached }),
                session_id,
                workspace: json!({ "id": workspace_id, "name": row.get::<_, String>("workspace_name")?, "ordinal": row.get::<_, i64>("workspace_rank")? }),
                workspace_id,
                task: json!({ "id": task_id, "name": row.get::<_, String>("task_name")?, "ordinal": row.get::<_, i64>("task_rank")? }),
                task_id,
                canvas: json!({ "id": canvas_id, "name": row.get::<_, String>("canvas_name")?, "ordinal": row.get::<_, i64>("canvas_rank")? }),
                window: json!({
                    "id": row.get::<_, String>("window_id")?,
                    "kind": if detached { "detached-terminal" } else { "main" },
                    "ordinal": row.get::<_, i64>("window_rank")?
                }),
                mount_id: row.get("mount_id")?,
                title: row.get("title")?,
                profile: if profile == "agent-team-member" { "claude-code".into() } else { profile },
                cwd: row.get("cwd")?,
                work_status: row.get("work_status")?,
                environment,
                dag: Dag {
                    depth: 0,
                    parent_ref: parent_id.as_deref().map(session_ref),
                    child_refs: Vec::new(),
                    sibling_refs: Arc::default(),
                },
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut children: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    for group in targets.chunk_by_mut(|left, right| {
        left.canvas["id"] == right.canvas["id"] && left.dag.parent_ref == right.dag.parent_ref
    }) {
        let siblings = Arc::new(
            group
                .iter()
                .map(|target| target.reference.clone())
                .collect(),
        );
        for target in group {
            target.dag.sibling_refs = Arc::clone(&siblings);
            if let Some(parent) = &target.dag.parent_ref {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push((ordinal(&target.session), target.reference.clone()));
            }
        }
    }
    let parents: HashMap<_, _> = targets
        .iter()
        .map(|target| (target.reference.clone(), target.dag.parent_ref.clone()))
        .collect();
    let mut depths = HashMap::new();
    for target in &mut targets {
        target.dag.depth = depth(&target.reference, &parents, &mut depths);
        if let Some(mut refs) = children.remove(&target.reference) {
            refs.sort_unstable();
            target.dag.child_refs = refs.into_iter().map(|(_, reference)| reference).collect();
        }
    }
    targets.sort_unstable_by(|a, b| {
        let order = |target: &Target| {
            (
                ordinal(&target.window),
                ordinal(&target.workspace),
                ordinal(&target.task),
                ordinal(&target.canvas),
                target.dag.depth,
                ordinal(&target.session),
            )
        };
        order(a)
            .cmp(&order(b))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    Ok(targets)
}

#[cfg(unix)]
fn ordinal(value: &Value) -> u64 {
    value["ordinal"].as_u64().unwrap_or_default()
}

#[cfg(unix)]
fn depth<'a>(
    reference: &'a str,
    parents: &'a HashMap<String, Option<String>>,
    depths: &mut HashMap<String, usize>,
) -> usize {
    let mut trail = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cursor = reference;
    let mut base = loop {
        if let Some(previous) = depths.get(cursor) {
            break previous + 1;
        }
        if !seen.insert(cursor) {
            // The domain normally prevents cycles. Bound the walk for imported data.
            break 0;
        }
        trail.push(cursor);
        match parents.get(cursor).and_then(Option::as_deref) {
            Some(parent) if parents.contains_key(parent) => cursor = parent,
            _ => break 0,
        }
    };
    for visited in trail.into_iter().rev() {
        depths.insert(visited.to_owned(), base);
        base += 1;
    }
    depths[reference]
}

#[cfg(unix)]
fn projection<'a>(
    targets: &'a [Target],
    caller: &Caller,
    scope: Scope,
) -> WireResult<Vec<&'a Target>> {
    match scope {
        Scope::All => Ok(targets.iter().collect()),
        Scope::CurrentLevel => {
            let current = targets
                .iter()
                .find(|target| target.session_id == caller.session_id)
                .ok_or_else(missing_caller)?;
            Ok(targets
                .iter()
                .filter(|target| Arc::ptr_eq(&target.dag.sibling_refs, &current.dag.sibling_refs))
                .collect())
        }
    }
}

#[cfg(unix)]
fn revision(targets: &[&Target]) -> String {
    // Struct order is part of the frozen hash contract.
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RevisionTarget<'a> {
        #[serde(rename = "ref")]
        reference: &'a str,
        workspace_id: &'a str,
        task_id: &'a str,
        session_id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        mount_id: Option<&'a str>,
        parent_ref: Option<&'a str>,
        child_refs: &'a [String],
    }
    let fields: Vec<_> = targets
        .iter()
        .map(|target| RevisionTarget {
            reference: &target.reference,
            workspace_id: &target.workspace_id,
            task_id: &target.task_id,
            session_id: &target.session_id,
            mount_id: target.mount_id.as_deref(),
            parent_ref: target.dag.parent_ref.as_deref(),
            child_refs: &target.dag.child_refs,
        })
        .collect();
    let bytes = serde_json::to_vec(&fields).expect("revision contains only strings and arrays");
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
enum Selector {
    SelfTarget,
    Session(String),
    Reference {
        reference: String,
        revision: String,
        scope: Scope,
    },
    Sibling {
        ordinal: usize,
        revision: String,
    },
    Left,
    Right,
    Parent,
    Child(usize),
}

#[cfg(unix)]
impl Selector {
    fn parse(target: &Value, navigation: bool) -> WireResult<Self> {
        let target = object(target)?;
        let kind = target.get("kind").and_then(Value::as_str);
        let is_legacy_ref =
            !navigation && !target.contains_key("kind") && target.contains_key("ref");
        let explicit_session = !navigation && target.get("sessionId").is_some_and(Value::is_string);
        let permitted: &[&str] = match kind {
            _ if explicit_session => &["kind", "sessionId"],
            None if is_legacy_ref => &["ref", "projectionRevision"],
            Some("session") => &["kind", "sessionId"],
            Some("ref") => &["kind", "ref", "projectionRevision"],
            Some("sibling") => &["kind", "ordinal", "projectionRevision"],
            Some("relative") => &["kind", "direction"],
            Some("relation") => &["kind", "relation", "ordinal"],
            Some("self") => &["kind"],
            _ => return Err(Fault::new(INVALID, "target.kind is unsupported")),
        };
        if target
            .keys()
            .any(|field| !permitted.contains(&field.as_str()))
        {
            return Err(Fault::new(INVALID, "target contains an unsupported field"));
        }
        let field = |name| target.get(name).unwrap_or(&Value::Null);
        let index = || {
            let max = if navigation {
                9_007_199_254_740_991
            } else {
                10_000
            };
            integer(field("ordinal"), max)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    Fault::new(
                        INVALID,
                        format!("target.ordinal must be between 1 and {max}"),
                    )
                })
        };
        let revision = || {
            bounded_text(
                field("projectionRevision"),
                160,
                "target.projectionRevision must be a non-empty bounded string",
            )
            .map(str::to_owned)
        };
        match kind {
            _ if explicit_session || kind == Some("session") => bounded_text(
                field("sessionId"),
                160,
                "target.sessionId must be a non-empty bounded string",
            )
            .map(|id| Self::Session(id.into())),
            _ if is_legacy_ref || kind == Some("ref") => Ok(Self::Reference {
                reference: bounded_text(
                    field("ref"),
                    160,
                    "target.ref must be a non-empty bounded string",
                )?
                .into(),
                revision: revision()?,
                scope: if is_legacy_ref {
                    Scope::CurrentLevel
                } else {
                    Scope::All
                },
            }),
            Some("sibling") => Ok(Self::Sibling {
                ordinal: index()?,
                revision: revision()?,
            }),
            Some("relative") => match field("direction").as_str() {
                Some("left") => Ok(Self::Left),
                Some("right") => Ok(Self::Right),
                _ => Err(Fault::new(INVALID, "target.direction is unsupported")),
            },
            Some("relation") => {
                let ordinal = if target.contains_key("ordinal") {
                    index()?
                } else {
                    1
                };
                match field("relation").as_str() {
                    Some("parent") => Ok(Self::Parent),
                    Some("child") => Ok(Self::Child(ordinal)),
                    _ => Err(Fault::new(INVALID, "target.relation is unsupported")),
                }
            }
            Some("self") => Ok(Self::SelfTarget),
            _ => unreachable!("validated selector kind"),
        }
    }

    fn resolve<'a>(&self, targets: &'a [Target], caller: &Caller) -> WireResult<&'a Target> {
        let scoped = match self {
            Self::Reference {
                revision, scope, ..
            } => {
                let visible = projection(targets, caller, *scope)?;
                check_revision(revision, &visible)?;
                Some(visible)
            }
            Self::Sibling { revision, .. } => {
                let visible = projection(targets, caller, Scope::CurrentLevel)?;
                check_revision(revision, &visible)?;
                None
            }
            _ => None,
        };
        let current = targets
            .iter()
            .find(|target| target.session_id == caller.session_id)
            .ok_or_else(missing_caller)?;
        let find_ref =
            |reference: &str| targets.iter().find(|target| target.reference == reference);
        match self {
            Self::SelfTarget => Ok(current),
            Self::Session(id) => targets
                .iter()
                .find(|target| target.session_id == *id)
                .ok_or_else(|| missing_target("指定会话已不在 Matou 会话图中")),
            Self::Reference { reference, .. } => {
                let mut matches = scoped
                    .into_iter()
                    .flatten()
                    .filter(|target| target.reference == *reference);
                let first = matches.next().ok_or_else(|| {
                    missing_target(format!("目标 {reference} 已不在 Matou 会话图中"))
                })?;
                if let Some(second) = matches.next() {
                    let candidates: Vec<_> = [first, second]
                        .into_iter()
                        .chain(matches)
                        .take(16)
                        .map(|target| {
                            let path = [&target.workspace, &target.task, &target.canvas]
                                .map(|part| part["name"].as_str().unwrap_or_default());
                            let path = path
                                .into_iter()
                                .chain([target.title.as_str()])
                                .collect::<Vec<_>>()
                                .join(" / ");
                            json!({ "humanPath": path.chars().take(256).collect::<String>() })
                        })
                        .collect();
                    return Err(Fault {
                        code: "AMBIGUOUS_TARGET",
                        message: format!("目标 {reference} 匹配多个层级位置").into(),
                        details: Some(json!({ "candidates": candidates })),
                    });
                }
                Ok(first)
            }
            Self::Sibling { ordinal, .. } => current
                .dag
                .sibling_refs
                .get(ordinal - 1)
                .and_then(|reference| find_ref(reference))
                .ok_or_else(|| missing_target(format!("当前层级没有第 {ordinal} 个会话"))),
            Self::Left | Self::Right => {
                let index = ordinal(&current.session) as usize - 1;
                let neighbor = if matches!(self, Self::Left) {
                    index.checked_sub(1)
                } else {
                    index.checked_add(1)
                };
                neighbor
                    .and_then(|index| current.dag.sibling_refs.get(index))
                    .and_then(|reference| find_ref(reference))
                    .ok_or_else(|| {
                        missing_target(if matches!(self, Self::Left) {
                            "当前会话左侧没有同层会话"
                        } else {
                            "当前会话右侧没有同层会话"
                        })
                    })
            }
            Self::Parent => {
                let reference = current
                    .dag
                    .parent_ref
                    .as_deref()
                    .ok_or_else(|| missing_target("当前会话没有父会话"))?;
                find_ref(reference).ok_or_else(|| missing_target("父会话已不在 Matou 会话图中"))
            }
            Self::Child(ordinal) => current
                .dag
                .child_refs
                .get(ordinal - 1)
                .and_then(|reference| find_ref(reference))
                .ok_or_else(|| missing_target(format!("当前会话没有第 {ordinal} 个子会话"))),
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntityLevel {
    Workspace,
    Task,
    Canvas,
    Session,
}

#[cfg(unix)]
impl EntityLevel {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "workspace" => Some(Self::Workspace),
            "task" => Some(Self::Task),
            "canvas" => Some(Self::Canvas),
            "session" => Some(Self::Session),
            _ => None,
        }
    }

    fn id(self, target: &Target) -> &str {
        match self {
            Self::Workspace => &target.workspace_id,
            Self::Task => &target.task_id,
            Self::Canvas => target.canvas["id"].as_str().expect("topology canvas ID"),
            Self::Session => &target.session_id,
        }
    }
}

#[cfg(unix)]
enum NavigationSelector {
    Current(EntityLevel),
    Target(Selector, Option<String>),
}

#[cfg(unix)]
impl NavigationSelector {
    fn parse(target: &Value) -> WireResult<Self> {
        let fields = object(target)?;
        match fields.get("kind").and_then(Value::as_str) {
            Some("current") => {
                if fields.len() != 2 {
                    return Err(Fault::new(
                        INVALID,
                        "current target must contain kind and entity",
                    ));
                }
                let level = target["entity"]
                    .as_str()
                    .and_then(EntityLevel::parse)
                    .ok_or(Fault::new(INVALID, "target.entity is unsupported"))?;
                Ok(Self::Current(level))
            }
            Some("relative" | "relation") => {
                let mut fields = fields.clone();
                let revision = fields.remove("projectionRevision").unwrap_or_default();
                let revision = bounded_text(
                    &revision,
                    160,
                    "target.projectionRevision must be a non-empty bounded string",
                )?;
                let selector = Selector::parse(&Value::Object(fields), true)?;
                Ok(Self::Target(selector, Some(revision.into())))
            }
            _ => Ok(Self::Target(Selector::parse(target, true)?, None)),
        }
    }

    fn resolve(
        &self,
        connection: &Connection,
        targets: &[Target],
        caller: &Caller,
        level: EntityLevel,
    ) -> WireResult<NavigationPath> {
        // Keep both the entity kind and its representative path. A switch can
        // use the enclosing level of a session selector, while focus needs a session.
        let (workspace_id, task_id, scene_id, session) = match self {
            Self::Current(entity) if *entity != EntityLevel::Session => {
                let current = Selector::SelfTarget.resolve(targets, caller)?;
                let (workspace, task, scene) =
                    entity_path(connection, *entity, entity.id(current))?;
                (workspace, task, scene, None)
            }
            Self::Target(
                Selector::Reference {
                    reference,
                    revision: expected,
                    ..
                },
                _,
            ) if let Some((prefix, id)) = reference.split_once(':')
                && !id.is_empty()
                && let Some(entity) =
                    EntityLevel::parse(if prefix == "scene" { "canvas" } else { prefix })
                && entity != EntityLevel::Session =>
            {
                check_revision(expected, &projection(targets, caller, Scope::All)?)?;
                let (workspace, task, scene) = entity_path(connection, entity, id)?;
                (workspace, task, scene, None)
            }
            _ => {
                let target = match self {
                    Self::Current(_) => Selector::SelfTarget.resolve(targets, caller)?,
                    Self::Target(selector, expected) => {
                        if let Some(expected) = expected {
                            check_revision(
                                expected,
                                &projection(targets, caller, Scope::CurrentLevel)?,
                            )?;
                        }
                        selector.resolve(targets, caller)?
                    }
                };
                (
                    target.workspace_id.clone(),
                    target.task_id.clone(),
                    EntityLevel::Canvas.id(target).to_owned(),
                    Some(target),
                )
            }
        };
        let route_window_id: String = connection
            .query_row(
                "SELECT placement.window_id FROM window_task_placements AS placement
                 JOIN app_windows AS window ON window.id = placement.window_id
                 WHERE placement.task_id = ? AND window.kind = 'main' AND window.state <> 'closed'",
                [&task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(navigation_query_error)?
            .ok_or_else(navigation_not_ready)?;
        let target = if level == EntityLevel::Session {
            session.ok_or_else(|| missing_target("导航目标必须是会话"))?
        } else {
            let session_id = switch_anchor(
                connection,
                &route_window_id,
                &workspace_id,
                (level != EntityLevel::Workspace).then_some(task_id.as_str()),
                (level == EntityLevel::Canvas).then_some(scene_id.as_str()),
            )?;
            targets
                .iter()
                .find(|target| target.session_id == session_id)
                .ok_or_else(navigation_not_ready)?
        };
        if target.mount_id.is_none() {
            return Err(navigation_not_ready());
        }
        Ok(NavigationPath {
            route_window_id,
            target_window_id: target.window["id"]
                .as_str()
                .expect("topology window ID")
                .into(),
            workspace_id: target.workspace_id.clone(),
            task_id: target.task_id.clone(),
            scene_id: EntityLevel::Canvas.id(target).into(),
            session_id: target.session_id.clone(),
        })
    }
}

#[cfg(unix)]
fn check_revision(expected: &str, visible: &[&Target]) -> WireResult<()> {
    if revision(visible) != expected {
        return Err(Fault::new(
            "STALE_PROJECTION",
            "target ordinal projection is stale; list targets again",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn entity_path(
    connection: &Connection,
    level: EntityLevel,
    id: &str,
) -> WireResult<(String, String, String)> {
    connection
        .query_row(
            "WITH target_task AS (
                SELECT task.id, task.workspace_id FROM tasks AS task
                JOIN workspaces AS workspace ON workspace.id = task.workspace_id
                LEFT JOIN window_task_placements AS placement ON placement.task_id = task.id
                WHERE task.archived_at IS NULL AND workspace.archived_at IS NULL
                  AND (?1 IS NULL OR workspace.id = ?1) AND (?2 IS NULL OR task.id = ?2)
                  AND (?3 IS NULL OR task.id = (SELECT task_id FROM scenes WHERE id = ?3))
                ORDER BY placement.ordinal, task.sort_key, task.created_at, task.id LIMIT 1
            )
            SELECT task.workspace_id, task.id, scene.id FROM target_task AS task
            JOIN scenes AS scene ON scene.task_id = task.id AND scene.archived_at IS NULL
            WHERE ?3 IS NULL OR scene.id = ?3
            ORDER BY scene.sort_key, scene.created_at, scene.id LIMIT 1",
            params![
                (level == EntityLevel::Workspace).then_some(id),
                (level == EntityLevel::Task).then_some(id),
                (level == EntityLevel::Canvas).then_some(id),
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(navigation_query_error)?
        .ok_or_else(|| missing_target("导航目标已不在 Matou 会话图中"))
}

#[cfg(unix)]
fn switch_anchor(
    connection: &Connection,
    window_id: &str,
    workspace_id: &str,
    task_id: Option<&str>,
    scene_id: Option<&str>,
) -> WireResult<String> {
    connection
        .query_row(
            "WITH active_task AS (
                SELECT task.id FROM tasks AS task
                JOIN window_task_placements AS placement ON placement.task_id = task.id
                LEFT JOIN window_workspace_focus AS focus
                  ON focus.window_id = ?1 AND focus.workspace_id = ?2
                WHERE task.workspace_id = ?2 AND task.archived_at IS NULL
                  AND placement.window_id = ?1 AND (?3 IS NULL OR task.id = ?3)
                ORDER BY (task.id = focus.active_task_id) DESC,
                         placement.ordinal, task.sort_key, task.created_at, task.id LIMIT 1
            ), active_scene AS (
                SELECT scene.id FROM scenes AS scene
                JOIN active_task AS task ON task.id = scene.task_id
                LEFT JOIN window_task_focus AS focus
                  ON focus.window_id = ?1 AND focus.task_id = task.id
                WHERE scene.archived_at IS NULL AND (?4 IS NULL OR scene.id = ?4)
                ORDER BY (scene.id = focus.active_scene_id) DESC,
                         scene.sort_key, scene.created_at, scene.id LIMIT 1
            )
            SELECT session.id FROM active_scene AS scene
            JOIN session_canvas_memberships AS member ON member.scene_id = scene.id
            JOIN sessions AS session ON session.id = member.session_id AND session.archived_at IS NULL
            LEFT JOIN window_scene_focus AS focus ON focus.window_id = ?1 AND focus.scene_id = scene.id
            ORDER BY (session.id = focus.active_session_id) DESC,
                     member.last_user_interaction_seq DESC, member.sibling_created_seq, session.id
            LIMIT 1",
            params![window_id, workspace_id, task_id, scene_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(navigation_query_error)?
        .ok_or_else(navigation_not_ready)
}

#[cfg(unix)]
fn navigation_query_error(_: rusqlite::Error) -> Fault {
    Fault::new("INTERNAL_ERROR", "could not read host navigation state")
}

#[cfg(unix)]
fn navigation_not_ready() -> Fault {
    Fault::new("TARGET_NOT_READY", "导航目标暂不可用")
}

#[cfg(not(unix))]
pub(crate) struct ControlServer;

#[cfg(not(unix))]
impl ControlServer {
    pub(crate) fn start(_: &Path, _: &str) -> Result<Self> {
        // Windows named pipe transport and ACLs are a separate acceptance gate.
        Ok(Self)
    }

    pub(crate) fn runs(&self) -> RunControl {
        RunControl {}
    }
}

#[cfg(all(unix, test))]
mod socket_tests {
    include!("../tests/host_control/socket.rs");

    mod mt_discovery {
        include!("../tests/host_control/mt_discovery.rs");
    }

    mod navigation {
        include!("../tests/host_control/navigation.rs");
    }
}

#[cfg(unix)]
fn strict_fields(params: &Value, allowed: &[&str]) -> WireResult<()> {
    if let Some(field) = object(params)?
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        let field: String = field
            .chars()
            .take(80)
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || "_.[]-".contains(ch) {
                    ch
                } else {
                    '?'
                }
            })
            .collect();
        return Err(Fault::new(
            INVALID,
            format!("请求参数包含不支持的字段: {field}"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn action_text<'a>(params: &'a Value, field: &str, max: usize) -> WireResult<&'a str> {
    let value = params[field]
        .as_str()
        .ok_or_else(|| Fault::new(INVALID, format!("请求参数 {field} 缺失或类型不正确")))?;
    if value.is_empty() || value.encode_utf16().count() > max {
        return Err(Fault::new(INVALID, format!("请求参数 {field} 不符合约束")));
    }
    Ok(value)
}

#[cfg(unix)]
fn choice<'a>(params: &'a Value, field: &str, choices: &[&str]) -> WireResult<&'a str> {
    params[field]
        .as_str()
        .filter(|value| choices.contains(value))
        .ok_or_else(|| Fault::new(INVALID, format!("{field} is unsupported")))
}

#[cfg(unix)]
fn write_error(error: anyhow::Error) -> Fault {
    if error
        .downcast_ref::<rusqlite::Error>()
        .is_some_and(|error| {
            matches!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::ReadOnly | rusqlite::ErrorCode::PermissionDenied)
            )
        })
    {
        return read_only_fault();
    }
    let text = error.to_string();
    let canvas_code = error
        .downcast_ref::<canvas::CanvasError>()
        .map(|error| error.code);
    let code = if let Some(code @ ("BRANCH_CONFLICT" | "WORKTREE_CONFLICT")) = canvas_code {
        code
    } else if error.is::<CommandReplayConflictError>()
        || text.contains("already exists")
        || matches!(canvas_code, Some("DUPLICATE_NAME" | "DUPLICATE_SCENE_NAME"))
        || error
            .downcast_ref::<tasks::TaskError>()
            .is_some_and(|error| error.code == "WORKSPACE_PATH_INVALID")
    {
        "PATH_CONFLICT"
    } else if text.contains("does not exist") {
        "TARGET_NOT_FOUND"
    } else if error.is::<canvas::CanvasError>()
        || text.contains("Scene must keep one Session")
        || text.contains("intent is stale")
    {
        "TARGET_NOT_READY"
    } else {
        eprintln!("[host-control] write failed: {error:#}");
        return Fault::new("INTERNAL_ERROR", "could not commit host action");
    };
    Fault::new(code, text)
}

#[cfg(unix)]
fn sql_error(error: rusqlite::Error) -> Fault {
    write_error(error.into())
}

#[cfg(unix)]
fn task_write(
    connection: &mut Connection,
    method: &str,
    params: &Value,
    now: i64,
) -> WireResult<Value> {
    if method == "task.move-to-window" {
        let move_task = || {
            let _ = wire_text(params, "migrationId", 160)?;
            let task = wire_text(params, "taskId", 160)?;
            let source = wire_text(params, "sourceWindowId", 160)?;
            let target = wire_text(params, "targetWindowId", 160)?;
            if source == target {
                return Err(Fault::new(
                    "CONFLICT",
                    "Task migration target must differ from source",
                ));
            }
            let placement: Option<String> = connection
                .query_row(
                    "SELECT placement.window_id FROM window_task_placements AS placement
                 JOIN tasks ON tasks.id = placement.task_id AND tasks.archived_at IS NULL
                 WHERE task_id = ?",
                    [task],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql_error)?;
            if placement.as_deref() != Some(source) {
                return Err(Fault::new("CONFLICT", "Task source placement is stale"));
            }
            let registered = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM app_windows WHERE id = ? AND kind = 'main' AND state <> 'closed')",
                [target], |row| row.get::<_, bool>(0),
            ).map_err(sql_error)?;
            if !registered {
                return Err(Fault::new("CONFLICT", "Target main window does not exist"));
            }
            Err(Fault::new(
                "CONFLICT",
                "Target main window is not connected to this runtime",
            ))
        };
        return move_task().map_err(|error| {
            if error.code == "STORAGE_READ_ONLY" {
                error
            } else {
                Fault::new("CONFLICT", error.message)
            }
        });
    }
    let task = wire_text(params, "taskId", 160)?;
    let active: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ? AND archived_at IS NULL)",
            [task],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if !active {
        return Err(Fault::new(
            "INTERNAL_ERROR",
            format!("Task {task} does not exist"),
        ));
    }
    let generation: String = connection
        .query_row(
            "SELECT value FROM _runtime_meta WHERE key = 'runtime_generation'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    match method {
        "task.status.write" => {
            let key = wire_text(params, "key", 160)?;
            if params.get("value").is_some_and(Value::is_null) {
                connection
                    .execute(
                        "DELETE FROM task_status_entries WHERE task_id = ? AND key = ?",
                        params![task, key],
                    )
                    .map_err(sql_error)?;
            } else {
                let value = wire_text(params, "value", 4096)?;
                connection.execute(
                    "INSERT INTO task_status_entries (task_id, key, value, runtime_generation, updated_at)
                     VALUES (?, ?, ?, ?, ?) ON CONFLICT(task_id, key) DO UPDATE SET
                     value = excluded.value, runtime_generation = excluded.runtime_generation, updated_at = excluded.updated_at",
                    params![task, key, value, generation, now],
                ).map_err(sql_error)?;
            }
        }
        "task.progress.write" => {
            let progress = params["progress"]
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| Fault::new(INVALID, "progress must be a finite number"))?
                .clamp(0.0, 100.0);
            let label = params
                .get("label")
                .map(|_| wire_text(params, "label", 1024))
                .transpose()?;
            connection.execute(
                "INSERT INTO task_progress (task_id, progress, label, runtime_generation, updated_at)
                 VALUES (?, ?, ?, ?, ?) ON CONFLICT(task_id) DO UPDATE SET progress = excluded.progress,
                 label = excluded.label, runtime_generation = excluded.runtime_generation, updated_at = excluded.updated_at",
                params![task, progress, label, generation, now],
            ).map_err(sql_error)?;
        }
        "task.log.append" => {
            let level = choice(params, "level", &["debug", "info", "warn", "error"])?;
            let source = wire_text(params, "source", 160)?;
            let message = wire_text(params, "message", 16 * 1024)?;
            let tx = connection.transaction().map_err(sql_error)?;
            tx.execute(
                "INSERT INTO task_logs (task_id, level, source, message, runtime_generation, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)", params![task, level, source, message, generation, now],
            ).map_err(sql_error)?;
            tx.execute(
                "DELETE FROM task_logs WHERE task_id = ?1 AND id IN
                 (SELECT id FROM task_logs WHERE task_id = ?1 ORDER BY id DESC LIMIT -1 OFFSET 10000)", [task],
            ).map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            return Ok(json!({"appended": true}));
        }
        _ => unreachable!("validated task method"),
    }
    Ok(json!({"written": true}))
}

#[cfg(unix)]
#[derive(Clone)]
struct Confirmation {
    caller: Caller,
    close: bool,
    level: EntityLevel,
    id: String,
    revision: String,
    impact: Value,
    expires_at: i64,
}

#[cfg(unix)]
fn confirmation_stale() -> Fault {
    Fault::new(
        "CONFIRMATION_STALE",
        "确认对应的目标或影响已变化，请重新预览",
    )
}

#[cfg(unix)]
fn confirmation_required(other_run: bool) -> Fault {
    Fault::new(
        "CONFIRMATION_REQUIRED",
        if other_run {
            "当前运行没有可用的确认"
        } else {
            "确认已失效，请先重新预览"
        },
    )
}

#[cfg(unix)]
struct StructureTarget<'a> {
    level: EntityLevel,
    target: &'a Target,
}

#[cfg(unix)]
fn require_level(target: &StructureTarget<'_>, level: EntityLevel) -> WireResult<()> {
    if target.level != level {
        return Err(missing_target(format!("目标必须是 {}", level.kind())));
    }
    Ok(())
}

#[cfg(unix)]
impl EntityLevel {
    fn kind(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Task => "task",
            Self::Canvas => "canvas",
            Self::Session => "session",
        }
    }

    fn prefix(self) -> &'static str {
        if self == Self::Canvas {
            "scene"
        } else {
            self.kind()
        }
    }
}

#[cfg(unix)]
fn structure_target<'a>(
    targets: &'a [Target],
    caller: &Caller,
    value: &Value,
) -> WireResult<StructureTarget<'a>> {
    let selector = NavigationSelector::parse(value)?;
    let find = |level: EntityLevel, id: &str| {
        targets
            .iter()
            .find(|target| level.id(target) == id)
            .map(|target| StructureTarget { level, target })
            .ok_or_else(|| missing_target("指定目标已不在 Matou 会话图中"))
    };
    match selector {
        NavigationSelector::Current(level) => {
            let current = Selector::SelfTarget.resolve(targets, caller)?;
            Ok(StructureTarget {
                level,
                target: current,
            })
        }
        NavigationSelector::Target(
            Selector::Reference {
                reference,
                revision: expected,
                ..
            },
            _,
        ) => {
            let visible = projection(targets, caller, Scope::All)?;
            structural_revision(&expected, &visible)?;
            let (prefix, id) = reference
                .split_once(':')
                .ok_or_else(|| missing_target("目标引用不存在"))?;
            let level = EntityLevel::parse(if prefix == "scene" { "canvas" } else { prefix })
                .ok_or_else(|| missing_target("目标引用不存在"))?;
            if level == EntityLevel::Session {
                let selector = Selector::Reference {
                    reference,
                    revision: expected,
                    scope: Scope::All,
                };
                return Ok(StructureTarget {
                    level,
                    target: selector.resolve(targets, caller)?,
                });
            }
            find(level, id)
        }
        NavigationSelector::Target(selector, expected) => {
            if let Some(expected) = expected {
                structural_revision(
                    &expected,
                    &projection(targets, caller, Scope::CurrentLevel)?,
                )?;
            } else if let Selector::Sibling { revision, .. } = &selector {
                structural_revision(revision, &projection(targets, caller, Scope::CurrentLevel)?)?;
            }
            Ok(StructureTarget {
                level: EntityLevel::Session,
                target: selector.resolve(targets, caller)?,
            })
        }
    }
}

#[cfg(unix)]
fn structural_revision(expected: &str, targets: &[&Target]) -> WireResult<()> {
    if expected != revision(targets) {
        return Err(Fault::new(
            "STALE_PROJECTION",
            "目标列表已更新，请重新列举后再执行",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn metadata(caller: &Caller, method: &str, command_id: String, input: &Value) -> CommandMetadata {
    let mut request = input.clone();
    if request.is_object() {
        request["method"] = json!(method);
    } else {
        request = json!({"method": method, "input": input});
    }
    CommandMetadata {
        request_hash: fingerprint(&json!({
            "callerSessionId": caller.session_id, "request": request,
        })),
        command_type: method.into(),
        causation_id: Some(caller.run_id.clone()),
        correlation_id: Some(command_id.clone()),
        command_id,
    }
}

#[cfg(unix)]
fn replay(connection: &Connection, command: &CommandMetadata) -> WireResult<Option<Value>> {
    let recorded: Option<(String, String)> = connection
        .query_row(
            "SELECT request_hash, response_json FROM command_deduplication WHERE command_id = ?",
            [&command.command_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    recorded
        .map(|(hash, response)| {
            if hash != command.request_hash {
                return Err(Fault::new("PATH_CONFLICT", "操作标识已用于不同请求"));
            }
            serde_json::from_str(&response).map_err(|error| write_error(error.into()))
        })
        .transpose()
}

#[cfg(unix)]
fn public_path(hierarchy: &Value, window: &str, level: EntityLevel, detached: bool) -> Value {
    let mut result = json!({
        "window": { "ref": format!("window:{window}"), "title": if detached { "独立终端窗口" } else { "主窗口" } }
    });
    for (field, key, label, entity) in [
        ("workspace", "workspace", "name", EntityLevel::Workspace),
        ("task", "task", "title", EntityLevel::Task),
        ("scene", "canvas", "name", EntityLevel::Canvas),
        ("session", "session", "title", EntityLevel::Session),
    ] {
        let item = &hierarchy[field];
        if let Some(id) = item["id"].as_str() {
            result[key] =
                json!({ "ref": format!("{}:{id}", entity.prefix()), "title": item[label] });
            if entity == EntityLevel::Workspace {
                result[key]["path"] = item["rootDirectory"].clone();
            }
        }
        if level == entity {
            break;
        }
    }
    result
}

#[cfg(unix)]
fn active_path(owner: &ShellOwner, window: &str) -> WireResult<Value> {
    let hierarchy = owner.hierarchy(window).map_err(write_error)?;
    let hierarchy = serde_json::to_value(hierarchy).map_err(|error| write_error(error.into()))?;
    Ok(public_path(&hierarchy, window, EntityLevel::Session, false))
}

#[cfg(unix)]
impl StructureTarget<'_> {
    fn id(&self) -> &str {
        self.level.id(self.target)
    }

    fn window(&self, connection: &Connection) -> WireResult<String> {
        connection
            .query_row(
                "SELECT placement.window_id FROM window_task_placements AS placement
             JOIN app_windows AS window ON window.id = placement.window_id
             WHERE placement.task_id = ? AND window.kind = 'main' AND window.state <> 'closed'",
                [&self.target.task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(navigation_not_ready)
    }

    fn validate_removal(&self, connection: &Connection) -> WireResult<()> {
        if self.level == EntityLevel::Canvas {
            return Err(Fault::new("TARGET_NOT_READY", "画布使用关闭画布操作"));
        }
        if self.level == EntityLevel::Workspace
            && connection
                .query_row(
                    "SELECT is_default FROM workspaces WHERE id = ?",
                    [self.id()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sql_error)?
        {
            return Err(Fault::new("TARGET_NOT_READY", "默认工作空间会保留在侧栏中"));
        }
        Ok(())
    }

    fn impact(&self, connection: &Connection, scope: &str, close: bool) -> WireResult<Value> {
        let count = |sql, id| {
            connection
                .query_row(sql, [id], |row| row.get::<_, i64>(0))
                .map_err(sql_error)
        };
        let workspace_tasks = || {
            count(
                "SELECT COUNT(*) FROM tasks WHERE workspace_id = ? AND archived_at IS NULL",
                &self.target.workspace_id,
            )
        };
        let task_canvases = || {
            count(
                "SELECT COUNT(*) FROM scenes WHERE task_id = ? AND archived_at IS NULL",
                &self.target.task_id,
            )
        };
        let tasks = match self.level {
            EntityLevel::Workspace => workspace_tasks()?,
            EntityLevel::Task => 1,
            _ => 0,
        };
        let last_canvas = close && task_canvases()? == 1 && workspace_tasks()? == 1;
        let canvases = match self.level {
            EntityLevel::Workspace => count(
                "SELECT COUNT(*) FROM scenes JOIN tasks ON tasks.id = scenes.task_id
                 WHERE tasks.workspace_id = ? AND tasks.archived_at IS NULL AND scenes.archived_at IS NULL", &self.target.workspace_id)?,
            EntityLevel::Task => task_canvases()?,
            EntityLevel::Canvas if !last_canvas => 1,
            _ => 0,
        };
        let scene = EntityLevel::Canvas.id(self.target);
        let session = &self.target.session_id;
        let (sessions, live_runs, processes) = if last_canvas {
            (0, 0, 0)
        } else {
            connection.query_row(
                "WITH RECURSIVE branch(id) AS (
                   SELECT ?1
                   UNION
                   SELECT relation.from_session_id FROM session_relations_current AS relation
                   JOIN branch ON relation.to_session_id = branch.id
                   JOIN session_canvas_memberships AS member ON member.session_id = relation.from_session_id
                   JOIN sessions ON sessions.id = member.session_id
                   WHERE ?2 AND member.scene_id = ?3 AND sessions.archived_at IS NULL
                     AND relation.relation_kind IN ('derived-from','forked-from')
                 ), affected(id) AS (
                   SELECT sessions.id FROM sessions JOIN tasks ON tasks.id = sessions.task_id
                   WHERE sessions.archived_at IS NULL AND tasks.archived_at IS NULL AND
                     CASE ?4
                       WHEN 'session' THEN sessions.id IN (SELECT id FROM branch)
                       WHEN 'canvas' THEN sessions.id IN (SELECT session_id FROM session_canvas_memberships WHERE scene_id = ?3)
                       WHEN 'task' THEN tasks.id = ?5
                       ELSE tasks.workspace_id = ?6
                     END
                 )
                 SELECT (SELECT COUNT(*) FROM affected), COUNT(runs.id), COUNT(runs.pid)
                 FROM session_runs AS runs JOIN affected ON affected.id = runs.session_id
                 WHERE runs.status IN ('starting', 'running')",
                params![session, scope == "subtree", scene, self.level.kind(), self.target.task_id, self.target.workspace_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
            ).map_err(sql_error)?
        };
        let descendants = if self.level == EntityLevel::Session {
            sessions.saturating_sub(1)
        } else if self.level == EntityLevel::Canvas {
            sessions
        } else {
            sessions
                + canvases
                + if self.level == EntityLevel::Workspace {
                    tasks
                } else {
                    0
                }
        };
        let scope = if self.level != EntityLevel::Session {
            "subtree"
        } else if descendants == 0 {
            "node"
        } else {
            scope
        };
        let path = hierarchy::read_created_path(connection, session).map_err(write_error)?;
        let detached =
            self.level == EntityLevel::Session && self.target.session["detached"] == true;
        let window = if detached {
            self.target.window["id"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        } else {
            self.window(connection)?
        };
        let path_level = if self.level == EntityLevel::Session {
            EntityLevel::Session
        } else {
            EntityLevel::Canvas
        };
        Ok(json!({
            "target": public_path(&path, &window, path_level, detached), "scope": scope, "tasks": tasks,
            "canvases": canvases, "sessions": sessions, "descendants": descendants,
            "liveRuns": live_runs, "terminalProcesses": processes, "preservesProjectFiles": true,
            "preservesBranches": true, "preservesWorktrees": true,
        }))
    }

    fn commit(
        &self,
        owner: &mut ShellOwner,
        command: &CommandMetadata,
        impact: &Value,
        close: bool,
        now: i64,
    ) -> WireResult<(Value, Vec<String>)> {
        let window = self.window(owner.control_storage())?;
        let id = self.id().to_owned();
        let db = owner.control_storage();
        let result = match self.level {
            EntityLevel::Workspace => workspaces::remove_workspace(
                db,
                command,
                &workspaces::RemoveWorkspaceInput {
                    window_id: window.clone(),
                    workspace_id: id.clone(),
                    confirmed_intent: format!("remove-workspace:{id}"),
                    now,
                },
                "默认工作空间会保留在侧栏中",
                now,
            ),
            EntityLevel::Task => tasks::delete_task(
                db,
                command,
                &tasks::DeleteTaskInput {
                    window_id: window.clone(),
                    task_id: id.clone(),
                    confirmed_intent: Some(format!("delete-task:{id}")),
                    now,
                },
                &crate::host::hierarchy_ids(),
                "默认",
                now,
            ),
            EntityLevel::Canvas => tabs::close_scene(
                db,
                command,
                &tabs::CloseSceneInput {
                    window_id: window.clone(),
                    scene_id: id.clone(),
                    confirmed_intent: Some(format!("close-scene:{id}")),
                    now,
                },
                now,
            ),
            EntityLevel::Session => cards::remove_branch(
                db,
                command,
                &cards::RemoveSessionBranchInput {
                    window_id: window.clone(),
                    scene_id: EntityLevel::Canvas.id(self.target).into(),
                    session_id: id.clone(),
                    scope: if impact["scope"] == "subtree" {
                        cards::RemoveScope::NodeAndDescendants
                    } else {
                        cards::RemoveScope::NodeOnly
                    },
                    now,
                },
                now,
            ),
        }
        .map_err(write_error)?;
        let disposed: Vec<String> = serde_json::from_value(result["disposedSessionIds"].clone())
            .map_err(|error| write_error(error.into()))?;
        let mut value = json!({
            "kind": if close { "canvas-closed" } else { "removed" },
            "targetRef": format!("{}:{id}", self.level.prefix()),
            "removedSessions": if close || self.level == EntityLevel::Session {
                json!(disposed.len())
            } else { impact["sessions"].clone() },
            "activePath": active_path(owner, &window)?,
        });
        if !close {
            value["removedTasks"] = impact["tasks"].clone();
            value["removedCanvases"] = impact["canvases"].clone();
        }
        Ok((value, disposed))
    }
}

#[cfg(unix)]
fn create_structure(
    owner: &mut ShellOwner,
    targets: &[Target],
    caller: &Caller,
    method: &str,
    params: &Value,
    window: &str,
    now: i64,
) -> WireResult<(Value, bool)> {
    use matou_native_core::pty::LaunchProfile;
    let (level, parent, field, fields): (_, _, _, &[_]) = match method {
        "structure.create.workspace" => (EntityLevel::Workspace, None, "path", &["path"]),
        "structure.create.task" => (
            EntityLevel::Task,
            Some(EntityLevel::Workspace),
            "workspace",
            &["workspace"],
        ),
        "structure.create.canvas" => (
            EntityLevel::Canvas,
            Some(EntityLevel::Task),
            "task",
            &["task"],
        ),
        _ => (
            EntityLevel::Session,
            Some(EntityLevel::Canvas),
            "canvas",
            &["canvas", "profile", "cwd", "permissionMode"],
        ),
    };
    let mut allowed = vec!["title", "submissionKey", "enter"];
    allowed.extend_from_slice(fields);
    strict_fields(params, &allowed)?;
    let title = params
        .get("title")
        .map(|value| action_title(value, "title"))
        .transpose()?
        .map(str::to_owned);
    let key = action_text(params, "submissionKey", 160)?;
    let enter = optional_bool(params, "enter")?;
    let mode = params
        .get("permissionMode")
        .map(|_| {
            choice(
                params,
                "permissionMode",
                &["default", "acceptEdits", "plan", "bypassPermissions"],
            )
        })
        .transpose()?;
    let cwd = params
        .get("cwd")
        .map(|_| action_text(params, "cwd", 4096))
        .transpose()?;
    let profile = if level == EntityLevel::Session {
        match choice(
            params,
            "profile",
            &["shell", "claude-code", "codex", "dsh", "antigravity"],
        )? {
            "shell" => LaunchProfile::Shell,
            "claude-code" => LaunchProfile::ClaudeCode,
            "codex" => LaunchProfile::Codex,
            "dsh" => LaunchProfile::Dsh,
            _ => LaunchProfile::Antigravity,
        }
    } else {
        LaunchProfile::Shell
    };
    let command = metadata(caller, method, format!("host-action:{key}"), params);
    let (record, changed) = if let Some(result) = replay(owner.control_storage(), &command)? {
        (result, false)
    } else {
        let target = parent
            .map(|parent| {
                let target = structure_target(targets, caller, &params[field])?;
                require_level(&target, parent)?;
                Ok(target)
            })
            .transpose()?;
        let route = match &target {
            Some(target) => target.window(owner.control_storage())?,
            None => window.to_owned(),
        };
        let target_id = || {
            target
                .as_ref()
                .expect("created entity has a parent")
                .id()
                .to_owned()
        };
        let navigation = Some(if enter { "activate" } else { "preserve" }.into());
        let result = match level {
            EntityLevel::Workspace => {
                let path = action_text(params, field, 4096)?;
                let directory = crate::host::resolve_workspace_directory(path)
                    .ok()
                    .filter(|path| path.is_dir() && readable_directory(path))
                    .ok_or_else(|| {
                        Fault::new("PATH_CONFLICT", format!("工作空间目录不可用：{path}"))
                    })?;
                let name = title.unwrap_or_else(|| {
                    directory
                        .file_name()
                        .unwrap_or(directory.as_os_str())
                        .to_string_lossy()
                        .into_owned()
                });
                owner.create_workspace(
                    &command,
                    &workspaces::CreateWorkspaceInput {
                        window_id: route,
                        name,
                        root_directory: directory.to_string_lossy().into_owned(),
                        navigation,
                        now,
                    },
                    &crate::host::hierarchy_ids(),
                    now,
                )
            }
            EntityLevel::Task => owner.create_task(
                &command,
                &tasks::CreateTaskInput {
                    window_id: route,
                    workspace_id: target_id(),
                    title,
                    navigation,
                    now,
                },
                &crate::host::hierarchy_ids(),
                now,
            ),
            EntityLevel::Canvas => owner.create_canvas(
                &command,
                &canvas::CreateCanvasInput {
                    window_id: route,
                    task_id: target_id(),
                    title,
                    navigation,
                    now,
                },
                &canvas_ids(),
                &canvas_messages(),
                now,
            ),
            EntityLevel::Session => {
                let target = target.as_ref().expect("session has a canvas").target;
                let scene = target_id();
                let anchor = switch_anchor(
                    owner.control_storage(),
                    &route,
                    &target.workspace_id,
                    Some(&target.task_id),
                    Some(&scene),
                )?;
                canvas::create_session_sibling(
                    owner.control_storage(),
                    &command,
                    &canvas::CreateShellSiblingInput {
                        window_id: route,
                        scene_id: scene,
                        source_session_id: anchor,
                        parent_session_id: None,
                        execution_context_id: None,
                        title,
                        navigation,
                        now,
                    },
                    &canvas_ids(),
                    &canvas_messages(),
                    now,
                    &canvas::SessionSiblingProfile {
                        kind: profile,
                        cwd,
                        permission_mode: mode,
                    },
                )
            }
        }
        .map_err(write_error)?;
        (result, true)
    };
    let window = record["navigation"]["windowId"].as_str().unwrap_or(window);
    let path = public_path(&record["created"], window, EntityLevel::Session, false);
    let field = if level == EntityLevel::Canvas {
        "canvas"
    } else {
        level.kind()
    };
    Ok((
        json!({
            "kind": "created", "entity": level.kind(), "createdRef": path[field]["ref"],
            "path": path, "focusedPath": public_path(&record, window, EntityLevel::Session, false)
        }),
        changed,
    ))
}

#[cfg(unix)]
fn readable_directory(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).is_ok_and(|path| {
        // SAFETY: the path is NUL terminated and lives through the call.
        unsafe { libc::access(path.as_ptr(), libc::R_OK | libc::X_OK) == 0 }
    })
}

#[cfg(unix)]
fn action_title<'a>(value: &'a Value, field: &str) -> WireResult<&'a str> {
    value
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 160)
        .ok_or_else(|| Fault::new(INVALID, format!("params.{field} is invalid")))
}

#[cfg(unix)]
fn canvas_ids() -> canvas::CanvasIds {
    let id = || uuid::Uuid::new_v4().to_string();
    canvas::CanvasIds {
        scene_id: id(),
        root_node_id: id(),
        secondary_node_id: id(),
        session_id: id(),
        mount_id: id(),
        relation_id: id(),
    }
}

#[cfg(unix)]
fn canvas_messages() -> canvas::CanvasMessages<'static> {
    canvas::CanvasMessages {
        new_canvas: "新画布",
        duplicate_scene_name: "当前事项下已存在同名页签",
        workspace_path_invalid: "工作区目录不可用，请先在本地恢复原路径，或移出该工作区",
        detached_session_must_return: "独立窗口中的会话需要先回到原会话列表",
    }
}

#[cfg(unix)]
fn fingerprint(value: &Value) -> String {
    let mut canonical = value.clone();
    canonical.sort_all_objects();
    format!("{:x}", Sha256::digest(canonical.to_string()))
}

#[cfg(unix)]
struct ForkItem<'a> {
    key: &'a str,
    title: &'a str,
    environment: &'a Value,
    start: bool,
    prompt: Option<&'a str>,
}

#[cfg(unix)]
impl<'a> ForkItem<'a> {
    fn parse(key: &'a str, input: &'a Value) -> WireResult<Self> {
        let title = action_title(&input["title"], "title")?;
        let environment = &input["environment"];
        let fields: &[_] = match choice(
            environment,
            "mode",
            &["current", "new-worktree", "existing-worktree"],
        )? {
            "current" => &["mode"],
            "new-worktree" => &["mode", "branch"],
            _ => &["mode", "branch", "worktreeRef"],
        };
        strict_fields(environment, fields)?;
        if fields.contains(&"branch") {
            action_text(environment, "branch", 160)?;
        }
        if fields.contains(&"worktreeRef") {
            action_text(environment, "worktreeRef", 160)?;
        }
        let prompt = input
            .get("prompt")
            .map(|value| {
                value
                    .as_str()
                    .filter(|text| text.len() <= 64 * 1024)
                    .ok_or_else(|| Fault::new(INVALID, "params.prompt is invalid"))
            })
            .transpose()?;
        Ok(Self {
            key,
            title,
            environment,
            start: optional_bool(input, "start")?,
            prompt,
        })
    }

    fn environment(&self) -> WireResult<canvas::ForkEnvironment<'a>> {
        let branch = || {
            self.environment["branch"]
                .as_str()
                .expect("validated branch")
        };
        Ok(
            match self.environment["mode"].as_str().expect("validated mode") {
                "current" => canvas::ForkEnvironment::Current,
                "new-worktree" => canvas::ForkEnvironment::New { branch: branch() },
                _ => {
                    let reference = self.environment["worktreeRef"]
                        .as_str()
                        .expect("validated reference");
                    let id = reference
                        .strip_prefix("worktree:")
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| Fault::new("WORKTREE_CONFLICT", "指定 Worktree 引用无效"))?;
                    canvas::ForkEnvironment::Existing {
                        id,
                        branch: branch(),
                    }
                }
            },
        )
    }
}

#[cfg(unix)]
fn fork_source<'a>(
    targets: &'a [Target],
    caller: &Caller,
    source: &Value,
) -> WireResult<StructureTarget<'a>> {
    if source["kind"] == "current" {
        return Err(Fault::new(INVALID, "params.source is invalid"));
    }
    let source = structure_target(targets, caller, source)?;
    require_level(&source, EntityLevel::Session)?;
    Ok(source)
}

#[cfg(unix)]
fn preflight_fork_environment(
    owner: &mut ShellOwner,
    source: &StructureTarget<'_>,
    item: &ForkItem<'_>,
) -> WireResult<()> {
    use matou_native_core::git_repo::{ForkBranchError, Repository, WorktreeScope};
    let environment = item.environment()?;
    if matches!(environment, canvas::ForkEnvironment::Current) {
        return Ok(());
    }
    let unavailable = || Fault::new("WORKTREE_CONFLICT", "当前分叉缺少可用 Git 工作树");
    let repository = Repository::discover(Path::new(&source.target.cwd))
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    match environment {
        canvas::ForkEnvironment::New { branch } => {
            repository.check_fork_branch(branch).map_err(|error| {
                let code = if matches!(error, ForkBranchError::Git(_)) {
                    "WORKTREE_CONFLICT"
                } else {
                    "BRANCH_CONFLICT"
                };
                Fault::new(code, error.to_string())
            })?;
            let root = owner
                .fork_worktree_root
                .as_deref()
                .ok_or_else(|| Fault::new("WORKTREE_CONFLICT", "需要配置受信任的隔离工作树目录"))?;
            WorktreeScope::new(root, &repository).map_err(|_| unavailable())?;
        }
        canvas::ForkEnvironment::Existing { id, branch } => {
            let row: Option<(String, String, String, Option<String>, Option<String>)> = owner.control_storage().query_row(
                "SELECT worktree.worktree_path, worktree.repository_root, worktree.branch_name, git.state, git.branch
                 FROM worktrees AS worktree JOIN execution_contexts AS context ON context.id = worktree.execution_context_id
                 LEFT JOIN execution_context_git_states AS git ON git.execution_context_id = context.id
                 WHERE worktree.id = ? AND context.workspace_id = ? AND context.archived_at IS NULL
                   AND worktree.state IN ('ready', 'dirty', 'retained')",
                params![id, source.target.workspace_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional().map_err(sql_error)?;
            let (path, root, saved_branch, state, observed) = row.ok_or_else(unavailable)?;
            if saved_branch != branch
                || state.as_deref() != Some("ready")
                || observed.as_deref() != Some(branch)
            {
                return Err(Fault::new(
                    "BRANCH_CONFLICT",
                    format!("Worktree 当前分支与 {branch} 不一致"),
                ));
            }
            let ready = crate::environment_runtime::probe_environment(
                crate::environment_runtime::EnvironmentTarget::Worktree(
                    crate::environment_runtime::WorktreeExpectation {
                        repository_root: Path::new(&root),
                        path: Path::new(&path),
                        head: &matou_native_core::git_repo::Head::Branch(branch.into()),
                    },
                ),
            )
            .map_err(|_| unavailable())?;
            if !ready
                .git
                .is_some_and(|git| git.repository.same_repository(&repository))
            {
                return Err(unavailable());
            }
        }
        canvas::ForkEnvironment::Current => unreachable!(),
    }
    Ok(())
}

#[cfg(unix)]
fn fork_structure(
    owner: &mut ShellOwner,
    targets: &[Target],
    caller: &Caller,
    method: &str,
    params: &Value,
    now: i64,
) -> WireResult<(Value, Vec<ForkLaunch>)> {
    if method == "structure.fork.children" {
        return fork_batch(owner, targets, caller, params, now);
    }
    strict_fields(
        params,
        &[
            "source",
            "title",
            "environment",
            "prompt",
            "start",
            "submissionKey",
        ],
    )?;
    let key = action_text(params, "submissionKey", 160)?;
    let item = ForkItem::parse(key, params)?;
    let command = metadata(caller, method, format!("host-action:{key}"), params);
    let mut launches = Vec::new();
    let stored = replay(owner.control_storage(), &command)?;
    let (session, window) = if let Some(record) = stored {
        let session = record["sessionId"]
            .as_str()
            .or_else(|| record["session"]["id"].as_str())
            .ok_or_else(|| missing_target("已提交的分支目标已不存在"))?
            .to_owned();
        let window = read_fork_window(owner.control_storage(), &session)?;
        if item.start {
            launches.push(ForkLaunch::new(&session, &item, false));
        }
        (session, window)
    } else {
        let source = fork_source(targets, caller, &params["source"])?;
        preflight_fork_environment(owner, &source, &item)?;
        let placement = if method.ends_with(".sibling") {
            canvas::ForkPlacement::Sibling
        } else {
            canvas::ForkPlacement::Child
        };
        let (session, window) = create_fork(owner, &source, placement, &item, &command, now)?;
        launches.push(ForkLaunch::new(&session, &item, true));
        (session, window)
    };
    let (state, error) = fork_state(owner.control_storage(), &session)?;
    if state == "failed" {
        return Err(Fault::new(
            "TARGET_NOT_READY",
            error.unwrap_or_else(|| "分支尚未就绪".into()),
        ));
    }
    let path =
        hierarchy::read_created_path(owner.control_storage(), &session).map_err(write_error)?;
    Ok((
        json!({
            "kind":"forked", "state": state, "sessionRef":session_ref(&session),
            "path":public_path(&path, &window, EntityLevel::Session, false), "environment":item.environment,
        }),
        launches,
    ))
}

#[cfg(unix)]
fn create_fork(
    owner: &mut ShellOwner,
    source: &StructureTarget<'_>,
    placement: canvas::ForkPlacement,
    item: &ForkItem<'_>,
    command: &CommandMetadata,
    now: i64,
) -> WireResult<(String, String)> {
    let window = source.window(owner.control_storage())?;
    let environment = item.environment()?;
    let result = canvas::create_fork_in_environment(
        owner.control_storage(),
        command,
        &canvas::ForkSessionInput {
            window_id: window.clone(),
            scene_id: EntityLevel::Canvas.id(source.target).into(),
            source_session_id: source.target.session_id.clone(),
            placement,
            name: item.title.into(),
            worktree_mode: if matches!(environment, canvas::ForkEnvironment::New { .. }) {
                canvas::ForkWorktreeMode::New
            } else {
                canvas::ForkWorktreeMode::Current
            },
            submission_key: command.command_id.clone(),
            now,
        },
        &canvas_ids(),
        &canvas_messages(),
        now,
        &environment,
        true,
    )
    .map_err(write_error)?;
    let id = result["sessionId"].as_str().expect("fork receipt session");
    Ok((id.into(), window))
}

#[cfg(unix)]
fn read_fork_window(connection: &Connection, session: &str) -> WireResult<String> {
    connection
        .query_row(
            "SELECT placement.window_id FROM window_task_placements AS placement
         JOIN sessions ON sessions.task_id = placement.task_id AND sessions.archived_at IS NULL
         WHERE sessions.id = ?",
            [session],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| missing_target("已提交的分支目标已不存在"))
}

#[cfg(unix)]
fn fork_state(
    connection: &Connection,
    session: &str,
) -> WireResult<(&'static str, Option<String>)> {
    connection
        .query_row(
            "SELECT stage, error_message FROM session_fork_intents WHERE session_id = ?",
            [session],
            |row| {
                let stage: String = row.get(0)?;
                Ok((
                    match stage.as_str() {
                        "succeeded" => "ready",
                        "failed" => "failed",
                        _ => "created",
                    },
                    row.get(1)?,
                ))
            },
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| missing_target("已提交的分支目标已不存在"))
}

#[cfg(unix)]
pub(crate) struct ForkLaunch {
    session: String,
    spawn: bool,
    start: bool,
    prompt: Option<String>,
}

#[cfg(unix)]
impl ForkLaunch {
    fn new(session: &str, item: &ForkItem<'_>, spawn: bool) -> Self {
        Self {
            session: session.into(),
            spawn,
            start: item.start,
            prompt: item.prompt.map(str::to_owned),
        }
    }
}

#[cfg(unix)]
struct BatchRow {
    session: Option<String>,
    state: String,
    start_state: String,
    error: Option<String>,
    failure: i64,
}

#[cfg(unix)]
fn fork_batch(
    owner: &mut ShellOwner,
    targets: &[Target],
    caller: &Caller,
    params: &Value,
    now: i64,
) -> WireResult<(Value, Vec<ForkLaunch>)> {
    strict_fields(params, &["source", "batchKey", "items", "retryItemKeys"])?;
    let batch = action_text(params, "batchKey", 160)?;
    let raw_items = params["items"]
        .as_array()
        .filter(|items| items.len() <= 50)
        .ok_or_else(|| Fault::new(INVALID, "params.items is invalid"))?;
    let mut keys = std::collections::HashSet::new();
    let items = raw_items
        .iter()
        .map(|item| {
            strict_fields(
                item,
                &["itemKey", "title", "environment", "prompt", "start"],
            )?;
            let key = action_text(item, "itemKey", 160)?;
            if !keys.insert(key) {
                return Err(Fault::new(INVALID, "itemKey must be unique"));
            }
            ForkItem::parse(key, item)
        })
        .collect::<WireResult<Vec<_>>>()?;
    let retry_present = params.get("retryItemKeys").is_some();
    let mut retries = std::collections::HashSet::new();
    if retry_present {
        let raw = params["retryItemKeys"]
            .as_array()
            .filter(|keys| keys.len() <= 50)
            .ok_or_else(|| Fault::new(INVALID, "params.retryItemKeys is invalid"))?;
        for key in raw {
            let key = key
                .as_str()
                .filter(|key| keys.contains(key))
                .ok_or_else(|| Fault::new(INVALID, "retry itemKey must belong to items"))?;
            if !retries.insert(key) {
                return Err(Fault::new(INVALID, "retry itemKey must be unique"));
            }
        }
    }
    // The public selector is part of the durable key so a retry cannot switch
    // targets when a relative position is subsequently occupied by another card.
    let request = fingerprint(
        &json!({"callerSessionId": caller.session_id, "batchKey": batch, "source": params["source"], "items": raw_items}),
    );
    let accepted: Option<(String, String)> = owner
        .control_storage()
        .query_row(
            "SELECT COALESCE(public_request_fingerprint, request_fingerprint), source_session_id
         FROM fork_batch_ledger WHERE batch_key = ?",
            [batch],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let source_id = if let Some((hash, source)) = accepted {
        if hash != request {
            return Err(Fault::new(
                "PATH_CONFLICT",
                format!("批次 {batch} 与已提交输入不一致"),
            ));
        }
        source
    } else {
        if retry_present {
            return Err(missing_target(format!(
                "批次 {batch} 没有可重试的上一轮结果"
            )));
        }
        let source = fork_source(targets, caller, &params["source"])?;
        if owner
            .workspace_path_state(&source.target.workspace_id)
            .map_err(write_error)?
            .is_some_and(|state| state.status == "invalid")
        {
            return Err(Fault::new("PATH_CONFLICT", "工作区目录不可用"));
        }
        for item in &items {
            preflight_fork_environment(owner, &source, item)?;
        }
        let tx = owner.control_storage().transaction().map_err(sql_error)?;
        tx.execute(
            "INSERT INTO fork_batch_ledger (batch_key, request_fingerprint, caller_session_id, source_session_id,
               source_scene_id, item_count, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![batch, request, caller.session_id, source.target.session_id, EntityLevel::Canvas.id(source.target), items.len() as i64, now, now],
        ).map_err(sql_error)?;
        for (ordinal, (item, raw)) in items.iter().zip(raw_items).enumerate() {
            let key = fingerprint(&json!([batch, item.key]));
            tx.execute(
                "INSERT INTO fork_batch_items (batch_key, item_key, ordinal, item_fingerprint, submission_key,
                   title, environment_json, start_requested, prompt_fingerprint, state, start_state, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'unsubmitted', ?, ?, ?)",
                params![batch, item.key, ordinal as i64, fingerprint(raw), key, item.title, item.environment.to_string(),
                    item.start, item.prompt.map(|prompt| fingerprint(&json!(prompt))),
                    if item.start { "pending" } else { "not-requested" }, now, now],
            ).map_err(sql_error)?;
        }
        tx.commit().map_err(sql_error)?;
        source.target.session_id.clone()
    };

    let mut launches = Vec::new();
    for item in &items {
        let mut row = batch_row(owner.control_storage(), batch, item.key)?;
        refresh_batch_row(owner.control_storage(), batch, item.key, &mut row, now)?;
        let retry = retries.contains(item.key) && row.state == "failed";
        if row.state != "unsubmitted" && !retry {
            if let Some(session) = &row.session
                && (row.state == "created" || (item.start && row.state == "ready"))
            {
                launches.push(ForkLaunch::new(session, item, false));
            }
            continue;
        }
        let result = if let Some(session) = &row.session {
            let window = read_fork_window(owner.control_storage(), session)?;
            let retry_key = format!(
                "mt-fork-retry:{}",
                fingerprint(&json!([batch, item.key, row.failure]))
            );
            let command = metadata(
                caller,
                "structure.fork.children",
                retry_key,
                &json!([batch, item.key, row.failure]),
            );
            owner
                .retry_fork(&command, &window, session, now)
                .map_err(write_error)
                .map(|_| session.clone())
        } else {
            let source = targets
                .iter()
                .find(|target| target.session_id == source_id)
                .map(|target| StructureTarget {
                    level: EntityLevel::Session,
                    target,
                });
            match source {
                Some(source) => {
                    let key = format!(
                        "mt-fork-item:{}",
                        fingerprint(&json!([batch, item.key, row.failure]))
                    );
                    let command = metadata(
                        caller,
                        "structure.fork.children",
                        key,
                        &json!([batch, item.key, request]),
                    );
                    create_fork(
                        owner,
                        &source,
                        canvas::ForkPlacement::Child,
                        item,
                        &command,
                        now,
                    )
                    .map(|(id, _)| id)
                }
                None => Err(missing_target("分支来源已不存在")),
            }
        };
        let connection = owner.control_storage();
        match result {
            Ok(session) => {
                connection.execute(
                    "UPDATE fork_batch_items SET state = 'created', session_id = ?, error_message = NULL,
                       start_state = CASE WHEN start_requested THEN 'pending' ELSE 'not-requested' END, updated_at = ?
                     WHERE batch_key = ? AND item_key = ?",
                    params![session, now, batch, item.key],
                ).map_err(sql_error)?;
                launches.push(ForkLaunch::new(&session, item, true));
            }
            Err(error) if error.code == "STORAGE_READ_ONLY" => return Err(error),
            Err(error) => {
                connection.execute(
                    "UPDATE fork_batch_items SET state = 'failed', failure_generation = failure_generation + 1,
                       error_message = ?, updated_at = ? WHERE batch_key = ? AND item_key = ?",
                    params![error.message.as_ref(), now, batch, item.key],
                ).map_err(sql_error)?;
            }
        }
    }
    let mut results = Vec::new();
    let mut failed = Vec::new();
    for item in &items {
        let row = batch_row(owner.control_storage(), batch, item.key)?;
        let mut result = json!({
            "itemKey": item.key, "title": item.title, "state": row.state, "environment": item.environment
        });
        if let Some(session) = row.session {
            result["sessionRef"] = json!(session_ref(&session));
        }
        if row.state == "failed" {
            failed.push(item.key);
            if let Some(error) = row.error {
                result["error"] = json!(error);
            }
        }
        results.push(result);
    }
    let mut response = json!({
        "kind":"fork-batch", "batchKey":batch, "succeeded":results.len() - failed.len(),
        "failed":failed.len(), "items":results,
    });
    if !failed.is_empty() {
        response["retry"] = json!({"batchKey":batch, "itemKeys":failed});
    }
    Ok((response, launches))
}

#[cfg(unix)]
fn batch_row(connection: &Connection, batch: &str, item: &str) -> WireResult<BatchRow> {
    connection.query_row(
        "SELECT session_id, state, start_state, error_message, failure_generation FROM fork_batch_items
         WHERE batch_key = ? AND item_key = ?",
        params![batch, item], |row| Ok(BatchRow {
            session: row.get(0)?, state: row.get(1)?, start_state: row.get(2)?, error: row.get(3)?, failure: row.get(4)?,
        }),
    ).map_err(sql_error)
}

#[cfg(unix)]
fn refresh_batch_row(
    connection: &Connection,
    batch: &str,
    key: &str,
    row: &mut BatchRow,
    now: i64,
) -> WireResult<()> {
    if row.state != "created" {
        return Ok(());
    }
    let Some(session) = &row.session else {
        return Ok(());
    };
    let (mut state, error) = fork_state(connection, session)?;
    if state == "ready" && row.start_state == "completed" {
        state = "started";
    }
    if state == "created" {
        return Ok(());
    }
    row.state = state.into();
    row.error = error;
    row.failure += i64::from(state == "failed");
    connection.execute(
        "UPDATE fork_batch_items SET state = ?, error_message = ?, failure_generation = ?, updated_at = ?
         WHERE batch_key = ? AND item_key = ?",
        params![state, row.error, row.failure, now, batch, key],
    ).map_err(sql_error)?;
    Ok(())
}

#[cfg(unix)]
struct ForkWait {
    request: Pending,
    result: Value,
    items: Vec<ForkWaitItem>,
}

#[cfg(unix)]
struct ForkWaitItem {
    session: String,
    prompt: Option<String>,
    start: bool,
    write: Option<TerminalWrite>,
    done: bool,
    expires: Instant,
}

#[cfg(unix)]
impl ForkWait {
    fn new(request: Pending, result: Value, jobs: &[ForkLaunch]) -> Self {
        let items = jobs
            .iter()
            .map(|job| ForkWaitItem {
                session: job.session.clone(),
                prompt: job.prompt.clone(),
                start: job.start,
                expires: Instant::now() + Duration::from_secs(if job.start { 60 } else { 70 }),
                write: None,
                done: false,
            })
            .collect();
        Self {
            request,
            result,
            items,
        }
    }

    fn advance(
        &mut self,
        state: &ServerState,
        owner: &mut ShellOwner,
        inputs: &HashMap<String, VecDeque<ControlInput>>,
    ) -> WireResult<bool> {
        state.authorize(
            &self.request.digest,
            &self.request.method,
            self.request.deadline,
        )?;
        if owner.control_is_read_only().map_err(write_error)? {
            return Err(read_only_fault());
        }
        let single = self.result["kind"] == "forked";
        let batch = self.result["batchKey"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let now = (state.clock)();
        for item in self.items.iter_mut().filter(|item| !item.done) {
            let Some((settled, error, start_state)) =
                item.advance(&self.request, state, owner, inputs)?
            else {
                continue;
            };
            item.done = true;
            if single {
                // A successfully accepted branch still exists if startup or prompt
                // delivery fails. A replay reads the authoritative launch state.
                self.result["state"] = json!(if settled == "failed" {
                    "created"
                } else {
                    settled
                });
                continue;
            }
            let reference = session_ref(&item.session);
            let items = self.result["items"]
                .as_array_mut()
                .expect("fork batch items");
            if let Some(result) = items
                .iter_mut()
                .find(|value| value["sessionRef"] == reference)
            {
                result["state"] = json!(settled);
                if let Some(error) = &error {
                    result["error"] = json!(error);
                }
                let key = result["itemKey"].as_str().expect("fork item key");
                owner.control_storage().execute(
                    "UPDATE fork_batch_items SET state = ?, start_state = ?, error_message = ?,
                       failure_generation = failure_generation + (state <> 'failed' AND ? = 'failed'), updated_at = ?
                     WHERE batch_key = ? AND item_key = ?",
                    params![settled, start_state, error, settled, now, batch, key],
                ).map_err(sql_error)?;
            }
        }
        if self.items.iter().any(|item| !item.done) {
            return Ok(false);
        }
        if !single {
            let items = self.result["items"].as_array().expect("fork batch items");
            let failed = items
                .iter()
                .filter(|item| item["state"] == "failed")
                .map(|item| item["itemKey"].clone())
                .collect::<Vec<_>>();
            let succeeded = items.len() - failed.len();
            self.result["succeeded"] = json!(succeeded);
            self.result["failed"] = json!(failed.len());
            if failed.is_empty() {
                self.result.as_object_mut().unwrap().remove("retry");
            } else {
                self.result["retry"] = json!({"batchKey": batch, "itemKeys": failed});
            }
        }
        Ok(true)
    }
}

#[cfg(unix)]
impl ForkWaitItem {
    fn advance(
        &mut self,
        request: &Pending,
        state: &ServerState,
        owner: &mut ShellOwner,
        inputs: &HashMap<String, VecDeque<ControlInput>>,
    ) -> WireResult<Option<(&'static str, Option<String>, &'static str)>> {
        let (stage, error) = fork_state(owner.control_storage(), &self.session)?;
        let now = (state.clock)();
        if stage == "failed" {
            return Ok(Some((
                if self.start { "created" } else { "failed" },
                error,
                if self.start {
                    "failed"
                } else {
                    "not-requested"
                },
            )));
        }
        if let Some(write) = &mut self.write {
            return match write.advance(request, state, owner, &self.session) {
                Ok(None) => Ok(None),
                Ok(Some(_)) => {
                    mark_prompt_complete(owner.control_storage(), &self.session, now)?;
                    Ok(Some(("started", None, "completed")))
                }
                Err(error) => Ok(Some((
                    "created",
                    Some(format!("首条提示词送达状态不确定：{}", error.message)),
                    "uncertain",
                ))),
            };
        }
        if stage == "created" || self.start && owner.control_terminal(&self.session).is_none() {
            return if Instant::now() >= self.expires {
                Ok(Some((
                    if self.start { "created" } else { "failed" },
                    Some("等待分支会话就绪超时".into()),
                    if self.start {
                        "failed"
                    } else {
                        "not-requested"
                    },
                )))
            } else {
                Ok(None)
            };
        }
        if !self.start || self.prompt.is_none() {
            return Ok(Some((
                "ready",
                None,
                if self.start {
                    "completed"
                } else {
                    "not-requested"
                },
            )));
        }
        match prompt_receipt(owner.control_storage(), &self.session)?.as_deref() {
            Some("completed") => return Ok(Some(("started", None, "completed"))),
            Some("delivering") => {
                if Instant::now() < self.expires {
                    return Ok(None);
                }
                return Ok(Some((
                    "created",
                    Some("首条提示词送达状态不确定".into()),
                    "uncertain",
                )));
            }
            _ => {}
        }
        if inputs.contains_key(&self.session) {
            return Ok(None);
        }
        let run = owner
            .control_terminal(&self.session)
            .ok_or_else(input_not_ready)?
            .0
            .to_owned();
        let prompt = self.prompt.take().expect("start prompt");
        let submit = !prompt.is_empty();
        let id = format!("host-fork-prompt:{}", self.session);
        let command = CommandMetadata {
            request_hash: fingerprint(&json!(prompt)),
            command_type: "fork.prompt.delivery".into(),
            causation_id: None,
            correlation_id: None,
            command_id: id,
        };
        matou_native_core::storage::execute(owner.control_storage(), &command, now, |_, _| {
            Ok(json!("delivering"))
        })
        .map_err(write_error)?;
        self.write = Some(TerminalWrite {
            run,
            submit,
            step: if !submit {
                InputStep::Body(vec![b'\r'])
            } else {
                InputStep::Body(prompt.into_bytes())
            },
        });
        Ok(None)
    }
}

#[cfg(unix)]
fn prompt_receipt(connection: &Connection, session: &str) -> WireResult<Option<String>> {
    connection.query_row(
        "SELECT json_extract(response_json, '$') FROM command_deduplication WHERE command_id = ?",
        [format!("host-fork-prompt:{session}")], |row| row.get(0),
    ).optional().map_err(sql_error)
}

#[cfg(unix)]
fn mark_prompt_complete(connection: &Connection, session: &str, now: i64) -> WireResult<()> {
    connection.execute(
        "UPDATE command_deduplication SET response_json = ?, committed_at = ? WHERE command_id = ?",
        params!["\"completed\"", now, format!("host-fork-prompt:{session}")],
    ).map_err(sql_error)?;
    Ok(())
}
