//! The local v1 control channel. Socket threads authenticate and frame requests.
//! Only the domain thread can dispatch them against the live topology.

use std::{ffi::OsStr, path::Path};

#[cfg(unix)]
use std::{
    borrow::Cow,
    collections::HashMap,
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
    rusqlite::Connection,
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
        // This gate never creates confirmation references.
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
];
#[cfg(unix)]
const OTHER_METHODS: &[&str] = &[
    "terminal.send-text",
    "terminal.send-key",
    "task.status.write",
    "task.progress.write",
    "task.log.append",
    "task.move-to-window",
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
    "navigation.focus.session",
    "navigation.switch.workspace",
    "navigation.switch.task",
    "navigation.switch.canvas",
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
#[derive(Serialize)]
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
}

#[cfg(unix)]
impl Action {
    fn parse(method: &str, params: &Value) -> WireResult<Self> {
        if !MANAGED_SCOPES.contains(&method) {
            return Err(unsupported_method());
        }
        let params = object(params)?;
        match method {
            "host.identify" => Ok(Self::Identify),
            "host.list" => match params.get("scope") {
                None => Ok(Self::List(Scope::CurrentLevel)),
                Some(scope) => match scope.as_str() {
                    Some("current-level") => Ok(Self::List(Scope::CurrentLevel)),
                    Some("all") => Ok(Self::List(Scope::All)),
                    _ => Err(Fault::new(INVALID, "scope is unsupported")),
                },
            },
            _ => Ok(Self::Terminal(
                Selector::parse(params.get("target").unwrap_or(&Value::Null))?,
                ReadKind::parse(method, params)?,
            )),
        }
    }
}

#[cfg(unix)]
struct Pending {
    digest: [u8; 32],
    method: String,
    id: String,
    deadline: i64,
    action: Action,
    reply: SyncSender<WireResult<Reply>>,
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

    pub(crate) fn poll(&self, owner: &crate::owner::ShellOwner) {
        let Some(pending) = &self.pending else { return };
        for _ in 0..OWNER_BATCH {
            let Ok(request) = pending.try_recv() else {
                break;
            };
            let result = self.dispatch(&request, owner);
            let _ = request.reply.send(result);
        }
    }

    fn dispatch(&self, request: &Pending, owner: &crate::owner::ShellOwner) -> WireResult<Reply> {
        let caller = self
            .state
            .authorize(&request.digest, &request.method, request.deadline)?;
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
                let result = enqueue(&request, state, sender);
                write_frame(socket, state, request.id, result)?;
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
        if (state.clock)() > request.deadline {
            return Err(deadline_elapsed());
        }
        match response.recv_timeout(IO_POLL) {
            Ok(result) => return result?.complete(request, state),
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
    fn parse(target: &Value) -> WireResult<Self> {
        let target = object(target)?;
        let kind = target.get("kind").and_then(Value::as_str);
        let is_legacy_ref = !target.contains_key("kind") && target.contains_key("ref");
        let explicit_session = target.get("sessionId").is_some_and(Value::is_string);
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
            integer(field("ordinal"), 10_000)
                .map(|value| value as usize)
                .ok_or(Fault::new(
                    INVALID,
                    "target.ordinal must be between 1 and 10000",
                ))
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
        let check_revision = |expected: &str, visible: &[&Target]| {
            if revision(visible) != expected {
                return Err(Fault::new(
                    "STALE_PROJECTION",
                    "target ordinal projection is stale; list targets again",
                ));
            }
            Ok(())
        };
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

    pub(crate) fn poll(&self, _: &crate::owner::ShellOwner) {}
}

#[cfg(all(unix, test))]
mod socket_tests {
    include!("../tests/host_control/socket.rs");

    mod mt_discovery {
        include!("../tests/host_control/mt_discovery.rs");
    }
}
