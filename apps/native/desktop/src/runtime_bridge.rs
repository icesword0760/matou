// Desktop facade over a supervised private runtime child. No in-process fallback.
use anyhow::{Context, Result, anyhow, bail, ensure};
use matou_native_core::database_recovery::RECOVERY_ID_CHANGED;
use matou_native_core::sessions::{current_interface_locale, read_locale_preference};
pub use matou_native_runtime::host::{
    Command, DatabaseRecoveryAction, DatabaseRecoveryDetails, DatabaseRecoveryResult, Event,
    LaunchOptions, ReadOnlyNavigation, ReadOnlyQuery, RecoveryState, StartupBlock,
    StartupBlockKind,
};
use matou_native_runtime::host::{
    DATABASE_RECOVERY_BUSY, DATABASE_RECOVERY_INTERRUPTED, STORAGE_READ_ONLY,
    STORAGE_READ_ONLY_TITLE, STORAGE_RECOVERY_WAITING, StoragePhase, storage_command_rejected,
};
use matou_native_runtime::protocol::{
    ClientMessage, ServerMessage, VERSION, check_version, read_messages, write_message,
};
use std::{
    path::Path,
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionPhase {
    Starting,
    Ready,
    ReadOnly {
        pending: Option<String>,
    },
    DatabaseRecovery {
        details: Arc<DatabaseRecoveryDetails>,
        pending: Option<String>,
    },
    Reconnecting,
    Blocked(StartupBlock),
    Stopped,
}
pub enum BridgeEvent {
    TransportDiagnostic {
        generation: u64,
        message: String,
    },
    Connection {
        generation: u64,
        phase: ConnectionPhase,
    },
    Runtime {
        generation: u64,
        event: Event,
    },
}
impl BridgeEvent {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Connection { generation, .. }
            | Self::Runtime { generation, .. }
            | Self::TransportDiagnostic { generation, .. } => *generation,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    Stopped,
    WrongGeneration { expected: u64, actual: u64 },
    NotReady,
    LifecycleCommand,
    StorageReadOnly,
    RecoveryWaiting,
    RecoveryIdChanged,
    RecoveryBusy,
}
impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StorageReadOnly => return f.write_str(STORAGE_READ_ONLY_TITLE),
            Self::RecoveryWaiting => return f.write_str(STORAGE_RECOVERY_WAITING),
            Self::RecoveryIdChanged => return f.write_str(RECOVERY_ID_CHANGED),
            Self::RecoveryBusy => return f.write_str(DATABASE_RECOVERY_BUSY),
            _ => {}
        }
        write!(f, "native runtime command rejected: {self:?}")
    }
}
impl std::error::Error for SendError {}
enum Outgoing {
    Business(Command),
    Stop,
}
struct State {
    generation: u64,
    phase: ConnectionPhase,
    sender: Option<Sender<Outgoing>>,
    stopping: bool,
    retry: bool,
    pid: u32,
}

fn qualify_command(phase: &ConnectionPhase, command: &Command) -> Result<(), SendError> {
    if matches!(command, Command::Shutdown) {
        return Err(SendError::LifecycleCommand);
    }
    let (storage, rejection) = match phase {
        ConnectionPhase::Ready => (StoragePhase::Writable, SendError::NotReady),
        ConnectionPhase::ReadOnly { .. } => (StoragePhase::ReadOnly, SendError::StorageReadOnly),
        ConnectionPhase::DatabaseRecovery { .. } => {
            (StoragePhase::Recovery, SendError::RecoveryWaiting)
        }
        _ => return Err(SendError::NotReady),
    };
    if !storage.accepts(command) {
        return Err(rejection);
    }
    if let (
        ConnectionPhase::DatabaseRecovery { details, .. },
        Command::DatabaseRecovery { action, .. },
    ) = (phase, command)
        && action
            .expected_recovery_id()
            .is_some_and(|id| id != details.recovery_id)
    {
        return Err(SendError::RecoveryIdChanged);
    }
    Ok(())
}
#[cfg(test)]
#[derive(Default)]
struct TestHooks {
    spawn_error_once: Mutex<Option<std::io::ErrorKind>>,
    before_hello: Mutex<Option<(Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    before_business: Mutex<Option<(Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    after_hello: Mutex<Option<(Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    transitions: Mutex<Vec<(u64, ConnectionPhase, std::time::Instant)>>,
}
#[cfg(test)]
fn pause_once(hook: &Mutex<Option<(Sender<()>, std::sync::mpsc::Receiver<()>)>>) {
    let pause = hook.lock().unwrap().take();
    if let Some((reached, resume)) = pause {
        let _ = reached.send(());
        let _ = resume.recv();
    }
}
struct Shared {
    #[cfg(test)]
    hooks: TestHooks,
    state: Mutex<State>,
    changed: Condvar,
}
#[derive(Clone)]
pub struct RuntimeCommands {
    shared: Arc<Shared>,
}
impl RuntimeCommands {
    /// The producer supplies its original connection, never the connection at completion time.
    pub fn send_at(
        &self,
        expected_generation: u64,
        command: Command,
    ) -> std::result::Result<(), SendError> {
        let mut state = self.shared.state.lock().unwrap();
        if state.stopping {
            return Err(SendError::Stopped);
        }
        if state.generation != expected_generation {
            return Err(SendError::WrongGeneration {
                expected: expected_generation,
                actual: state.generation,
            });
        }
        qualify_command(&state.phase, &command)?;
        let mut recovery_request = None;
        if let Command::DatabaseRecovery { request_id, .. } = &command {
            if matches!(
                &state.phase,
                ConnectionPhase::DatabaseRecovery {
                    pending: Some(_),
                    ..
                } | ConnectionPhase::ReadOnly { pending: Some(_) }
            ) {
                return Err(SendError::RecoveryBusy);
            }
            recovery_request = Some(request_id.clone());
        }
        state
            .sender
            .as_ref()
            .ok_or(SendError::NotReady)?
            .send(Outgoing::Business(command))
            .map_err(|_| SendError::NotReady)?;
        if let ConnectionPhase::DatabaseRecovery { pending, .. }
        | ConnectionPhase::ReadOnly { pending } = &mut state.phase
            && recovery_request.is_some()
        {
            *pending = recovery_request;
        }
        Ok(())
    }
    pub fn connection(&self) -> (u64, ConnectionPhase) {
        let state = self.shared.state.lock().unwrap();
        (state.generation, state.phase.clone())
    }
    /// Shared by Workspace and contract tests before applying any event side effects.
    pub fn accepts(&self, event: &BridgeEvent) -> bool {
        let state = self.shared.state.lock().unwrap();
        state.generation == event.generation()
    }
    fn stop(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.stopping = true;
        if let Some(sender) = &state.sender {
            let _ = sender.send(Outgoing::Stop);
        }
        self.shared.changed.notify_all();
    }
}
pub struct RuntimeBridge {
    pub commands: RuntimeCommands,
    pub events: async_channel::Receiver<BridgeEvent>,
    thread: Option<JoinHandle<Result<()>>>,
}
impl RuntimeBridge {
    pub fn start(options: LaunchOptions) -> Result<Self> {
        let executable = std::env::current_exe().context("locate native desktop executable")?;
        let binary = executable
            .parent()
            .context("native desktop executable has no parent")?
            .join(if cfg!(windows) {
                "matou-native-runtime.exe"
            } else {
                "matou-native-runtime"
            });
        Self::start_with_binary(options, &binary)
    }
    pub fn start_with_binary(options: LaunchOptions, binary: &Path) -> Result<Self> {
        Self::start_supervised(
            options,
            binary,
            #[cfg(test)]
            TestHooks::default(),
        )
    }
    fn start_supervised(
        options: LaunchOptions,
        binary: &Path,
        #[cfg(test)] hooks: TestHooks,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            #[cfg(test)]
            hooks,
            state: Mutex::new(State {
                generation: 0,
                phase: ConnectionPhase::Starting,
                sender: None,
                stopping: false,
                retry: false,
                pid: 0,
            }),
            changed: Condvar::new(),
        });
        let commands = RuntimeCommands {
            shared: shared.clone(),
        };
        let (send, events) = async_channel::unbounded();
        let (started, startup) = mpsc::channel();
        let binary = binary.to_owned();
        let thread = thread::Builder::new()
            .name("matou-runtime-supervisor".into())
            .spawn(move || supervise(options, &binary, shared, send, started))
            .context("start native runtime bridge manager")?;
        match startup.recv() {
            Ok(Ok(())) => Ok(Self {
                commands,
                events,
                thread: Some(thread),
            }),
            outcome => {
                commands.stop();
                let _ = thread.join();
                match outcome {
                    Ok(Err(error)) => bail!(error),
                    _ => bail!("native runtime manager ended before spawning child"),
                }
            }
        }
    }
    pub fn runtime_pid(&self) -> u32 {
        self.commands.shared.state.lock().unwrap().pid
    }
    pub fn retry_startup(&self) -> bool {
        let mut state = self.commands.shared.state.lock().unwrap();
        if state.stopping || !matches!(state.phase, ConnectionPhase::Blocked(_)) {
            return false;
        }
        state.retry = true;
        self.commands.shared.changed.notify_all();
        true
    }
    pub fn shutdown(&mut self) -> Result<()> {
        self.commands.stop();
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread
            .join()
            .map_err(|_| anyhow!("native runtime bridge manager panicked"))?
    }

    /// Stop accepting commands and hand off the supervisor so the caller can wait off-thread.
    #[cfg(target_os = "macos")]
    pub fn shutdown_async(&mut self) -> Option<JoinHandle<Result<()>>> {
        self.commands.stop();
        self.thread.take()
    }
}
impl Drop for RuntimeBridge {
    fn drop(&mut self) {
        self.commands.stop();
    }
}
const BACKOFF_MS: [u64; 5] = [100, 500, 1000, 2000, 5000];
fn backoff(attempt: usize) -> Duration {
    Duration::from_millis(BACKOFF_MS[attempt.min(4)])
}
fn publish(
    shared: &Shared,
    events: &async_channel::Sender<BridgeEvent>,
    generation: u64,
    mut phase: ConnectionPhase,
) {
    {
        let mut state = shared.state.lock().unwrap();
        if let (
            ConnectionPhase::DatabaseRecovery {
                details: old,
                pending: old_pending,
            },
            ConnectionPhase::DatabaseRecovery { details, pending },
        ) = (&state.phase, &mut phase)
            && old.recovery_id == details.recovery_id
        {
            *pending = old_pending.clone();
        }
        state.phase = phase.clone();
    }
    #[cfg(test)]
    shared.hooks.transitions.lock().unwrap().push((
        generation,
        phase.clone(),
        std::time::Instant::now(),
    ));
    if events
        .send_blocking(BridgeEvent::Connection { generation, phase })
        .is_err()
    {
        let mut state = shared.state.lock().unwrap();
        state.stopping = true;
        if let Some(sender) = &state.sender {
            let _ = sender.send(Outgoing::Stop);
        }
        shared.changed.notify_all();
    }
}
fn supervise(
    options: LaunchOptions,
    binary: &Path,
    shared: Arc<Shared>,
    events: async_channel::Sender<BridgeEvent>,
    started: Sender<std::result::Result<(), String>>,
) -> Result<()> {
    let mut attempt = 0;
    let mut first = true;
    let mut fatal = None;
    loop {
        let generation = {
            let mut state = shared.state.lock().unwrap();
            if state.stopping {
                break;
            }
            state.generation += 1;
            state.generation
        };
        publish(&shared, &events, generation, ConnectionPhase::Starting);
        let result = run_child(
            &options,
            binary,
            generation,
            &shared,
            &events,
            if first { Some(&started) } else { None },
        );
        first = false;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                // Without a successful wait no successor owner is permitted,
                // including through explicit startup retry.
                shared.state.lock().unwrap().stopping = true;
                fatal = Some(error);
                break;
            }
        };
        if outcome.ready {
            attempt = 0;
        }
        let mut state = shared.state.lock().unwrap();
        state.sender = None;
        state.pid = 0;
        if state.stopping {
            fatal = outcome.error;
            break;
        }
        drop(state);
        if let Some(reason) = outcome.blocked {
            publish(
                &shared,
                &events,
                generation,
                ConnectionPhase::Blocked(reason),
            );
            let state = shared.state.lock().unwrap();
            let mut state = shared
                .changed
                .wait_while(state, |s| !s.stopping && !s.retry)
                .unwrap();
            if state.stopping {
                break;
            }
            state.retry = false;
            attempt = 0;
        } else {
            publish(&shared, &events, generation, ConnectionPhase::Reconnecting);
            let state = shared.state.lock().unwrap();
            let (state, _) = shared
                .changed
                .wait_timeout_while(state, backoff(attempt), |s| !s.stopping)
                .unwrap();
            if state.stopping {
                break;
            }
            attempt = attempt.saturating_add(1);
        }
    }
    let generation = shared.state.lock().unwrap().generation;
    publish(&shared, &events, generation, ConnectionPhase::Stopped);
    let _ = events.send_blocking(BridgeEvent::Runtime {
        generation,
        event: Event::Closed,
    });
    match fatal {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
struct AttemptOutcome {
    ready: bool,
    blocked: Option<StartupBlock>,
    error: Option<anyhow::Error>,
}
fn spawn_is_configuration(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::InvalidInput
    )
}
fn run_child(
    options: &LaunchOptions,
    binary: &Path,
    generation: u64,
    shared: &Arc<Shared>,
    events: &async_channel::Sender<BridgeEvent>,
    started: Option<&Sender<std::result::Result<(), String>>>,
) -> Result<AttemptOutcome> {
    let (sender, incoming) = mpsc::channel();
    // Spawn and stop share one linearization point. A stop observed here cannot
    // be followed by an OS spawn, even at the end of a backoff timeout.
    let mut state = shared.state.lock().unwrap();
    if state.stopping {
        return Ok(AttemptOutcome {
            ready: false,
            blocked: None,
            error: None,
        });
    }
    let mut process = ProcessCommand::new(binary);
    process
        .arg("--data-root")
        .arg(&options.data_root)
        .arg("--workspace")
        .arg(&options.workspace)
        .arg("--shell-home")
        .arg(&options.shell_home)
        .arg("--shell")
        .arg(&options.shell)
        .env(
            "MATOU_LOCALE",
            current_interface_locale(read_locale_preference(&options.data_root)).value(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(test)]
    let spawn = match shared.hooks.spawn_error_once.lock().unwrap().take() {
        Some(kind) => Err(std::io::Error::from(kind)),
        None => process.spawn(),
    };
    #[cfg(not(test))]
    let spawn = process.spawn();
    let spawn =
        spawn.with_context(|| format!("start required native runtime child {}", binary.display()));
    let mut child = match spawn {
        Ok(child) => child,
        Err(error) => {
            drop(state);
            let configuration = error
                .downcast_ref::<std::io::Error>()
                .is_some_and(spawn_is_configuration);
            let message = format!("{error:#}");
            if let Some(started) = started {
                // A transient initial failure still has a live supervisor;
                // neither an OS child nor domain Ready is implied by this Ok.
                let _ = started.send(if configuration {
                    Err(message.clone())
                } else {
                    Ok(())
                });
            }
            let _ = events.send_blocking(BridgeEvent::TransportDiagnostic {
                generation,
                message: message.clone(),
            });
            return Ok(AttemptOutcome {
                ready: false,
                blocked: configuration.then_some(StartupBlock {
                    kind: StartupBlockKind::Configuration,
                    message,
                }),
                error: None, // no child existed to fail an active shutdown
            });
        }
    };
    state.pid = child.id();
    state.sender = Some(sender.clone());
    drop(state);
    let mut ready = false;
    let mut blocked = None;
    let mut writer = None;
    let mut missing_hello = false;
    let result = (|| -> Result<()> {
        let mut stdin = child.stdin.take().context("native runtime stdin")?;
        let stdout = child.stdout.take().context("native runtime stdout")?;
        let writer_shared = shared.clone();
        let writer_events = events.clone();
        writer = Some(
            thread::Builder::new()
                .name("matou-runtime-writer".into())
                .spawn(move || -> Result<()> {
                    #[cfg(test)]
                    pause_once(&writer_shared.hooks.before_hello);
                    write_message(&mut stdin, &ClientMessage::Hello { version: VERSION })?;
                    while let Ok(message) = incoming.recv() {
                        match message {
                            Outgoing::Stop => {
                                // Lifecycle is independent of Ready and never becomes a business command.
                                write_message(
                                    &mut stdin,
                                    &ClientMessage::Command(Command::Shutdown),
                                )
                                .context("write native runtime shutdown")?;
                                return Ok(());
                            }
                            Outgoing::Business(command) => {
                                #[cfg(test)]
                                pause_once(&writer_shared.hooks.before_business);
                                let state = writer_shared.state.lock().unwrap();
                                // Normal stop closes admission, then drains accepted FIFO
                                // before Stop. Failure retirement changes phase instead.
                                let qualification = if state.generation != generation {
                                    Err(SendError::NotReady)
                                } else {
                                    qualify_command(&state.phase, &command).and_then(|()| {
                                        match (&state.phase, &command) {
                                            (
                                                ConnectionPhase::DatabaseRecovery {
                                                    pending, ..
                                                }
                                                | ConnectionPhase::ReadOnly { pending },
                                                Command::DatabaseRecovery { request_id, .. },
                                            ) if pending.as_ref() != Some(request_id) => {
                                                Err(SendError::NotReady)
                                            }
                                            _ => Ok(()),
                                        }
                                    })
                                };
                                let report =
                                    state.sender.is_some() && state.generation == generation;
                                drop(state);
                                if let Err(error) = qualification {
                                    // Retirement owns the pending export completion once the pipe is gone.
                                    if report {
                                        let (code, message) = match error {
                                            SendError::StorageReadOnly => {
                                                (STORAGE_READ_ONLY, STORAGE_READ_ONLY_TITLE)
                                            }
                                            SendError::RecoveryWaiting => {
                                                (STORAGE_READ_ONLY, STORAGE_RECOVERY_WAITING)
                                            }
                                            _ => (
                                                "RECOVERY_INTERRUPTED",
                                                DATABASE_RECOVERY_INTERRUPTED,
                                            ),
                                        };
                                        let _ = writer_events.send_blocking(BridgeEvent::Runtime {
                                            generation,
                                            event: storage_command_rejected(command, code, message),
                                        });
                                    }
                                    continue;
                                }
                                // Once written to the old pipe bytes cannot be retracted;
                                // that pipe and queue are never reused by a new attempt.
                                write_message(&mut stdin, &ClientMessage::Command(command))?;
                            }
                        }
                    }
                    Ok(())
                })
                .context("start native runtime writer")?,
        );
        if let Some(started) = started {
            let _ = started.send(Ok(()));
        }
        let mut messages = read_messages::<ServerMessage>(stdout);
        let first = messages.next();
        missing_hello = first.is_none();
        match first.context("native runtime EOF before Hello")?? {
            ServerMessage::Hello { version } => {
                if let Err(error) = check_version(version) {
                    blocked = Some(StartupBlock {
                        kind: StartupBlockKind::Protocol,
                        message: format!("{error:#}"),
                    });
                    return Err(error);
                }
            }
            ServerMessage::Event(_) => bail!("native runtime event before Hello"),
        }
        #[cfg(test)]
        pause_once(&shared.hooks.after_hello);
        let mut closed = false;
        for message in messages {
            ensure!(!closed, "native runtime data after Closed");
            let event = match message? {
                ServerMessage::Hello { .. } => bail!("native runtime repeated Hello"),
                ServerMessage::Event(event) => event,
            };
            if matches!(event, Event::Closed) {
                closed = true;
                continue;
            }
            if let Event::StartupBlocked(reason) = &event {
                blocked = Some(reason.clone());
            }
            if let Event::DatabaseRecoveryRequired(details) = &event
                && !shared.state.lock().unwrap().stopping
            {
                publish(
                    shared,
                    events,
                    generation,
                    ConnectionPhase::DatabaseRecovery {
                        details: Arc::new(details.clone()),
                        pending: None,
                    },
                );
            }
            if let Event::DatabaseRecoveryResult(result) = &event {
                let mut state = shared.state.lock().unwrap();
                if let ConnectionPhase::DatabaseRecovery { pending, .. }
                | ConnectionPhase::ReadOnly { pending } = &mut state.phase
                    && pending.as_ref() == Some(&result.request_id)
                {
                    *pending = None;
                }
            }
            if matches!(event, Event::Ready { .. } | Event::ReadOnlyReady { .. }) {
                ready = true;
                if !shared.state.lock().unwrap().stopping {
                    let phase = if matches!(event, Event::ReadOnlyReady { .. }) {
                        ConnectionPhase::ReadOnly { pending: None }
                    } else {
                        ConnectionPhase::Ready
                    };
                    publish(shared, events, generation, phase);
                }
            }
            if events
                .send_blocking(BridgeEvent::Runtime { generation, event })
                .is_err()
            {
                let mut state = shared.state.lock().unwrap();
                state.stopping = true;
                let _ = sender.send(Outgoing::Stop);
            }
        }
        ensure!(closed, "native runtime EOF without Closed");
        Ok(())
    })();
    if writer.is_none() {
        if let (Some(started), Err(error)) = (started, &result) {
            let _ = started.send(Err(format!("{error:#}")));
        }
    }
    // Retire this attempt before waking its writer; its entire queue dies with it.
    let pending_recovery = {
        let mut state = shared.state.lock().unwrap();
        state.sender = None;
        match std::mem::replace(&mut state.phase, ConnectionPhase::Reconnecting) {
            ConnectionPhase::DatabaseRecovery { pending, .. }
            | ConnectionPhase::ReadOnly { pending } => pending,
            _ => None,
        }
    };
    if let Some(request_id) = pending_recovery {
        let _ = events.send_blocking(BridgeEvent::Runtime {
            generation,
            event: Event::DatabaseRecoveryResult(DatabaseRecoveryResult::failed(
                request_id,
                "RECOVERY_INTERRUPTED",
                DATABASE_RECOVERY_INTERRUPTED,
            )),
        });
    }
    let _ = sender.send(Outgoing::Stop);
    drop(child.stdin.take());
    drop(child.stdout.take());
    let written = writer
        .map(|writer| {
            writer
                .join()
                .map_err(|_| anyhow!("native runtime writer panicked"))
                .and_then(|result| result)
        })
        .unwrap_or(Ok(()));
    // A protocol failure closes stdin and the child exits. Never start a second owner before wait.
    let waited = child.wait();
    // A mismatched legacy child rejects our Hello before sending its own.
    // Signal deaths before Hello remain transient supervisor failures.
    if missing_hello && waited.as_ref().is_ok_and(|status| status.code() == Some(1)) {
        blocked = Some(StartupBlock {
            kind: StartupBlockKind::Protocol,
            message: format!("native runtime exited before accepting protocol version {VERSION}"),
        });
    }
    let mut failures = Vec::new();
    if let Err(error) = &result {
        failures.push(format!("reader: {error:#}"));
    }
    if let Err(error) = &written {
        failures.push(format!("writer: {error:#}"));
    }
    match &waited {
        Ok(status) if !status.success() => failures.push(format!("child exit: {status}")),
        Err(error) => failures.push(format!("wait: {error}")),
        _ => {}
    }
    let error = (!failures.is_empty()).then(|| {
        anyhow!(
            "native runtime {} ended: {}",
            child.id(),
            failures.join("; ")
        )
    });
    if let Some(error) = &error {
        let _ = events.send_blocking(BridgeEvent::TransportDiagnostic {
            generation,
            message: format!("{error:#}"),
        });
    }
    if waited.is_err() {
        return Err(error
            .expect("wait failure retained above")
            .context("cannot confirm native runtime exit; supervisor stopped without successor"));
    }
    Ok(AttemptOutcome {
        ready,
        blocked,
        error,
    })
}

#[cfg(all(test, target_os = "macos"))]
mod supervisor_tests {
    use super::*;
    use std::{fs, path::PathBuf, time::Instant};
    use uuid::Uuid;
    const LIMIT: Duration = Duration::from_secs(12);
    fn setup() -> Result<(PathBuf, RuntimeBridge)> {
        setup_with_hooks(TestHooks::default())
    }
    fn setup_with_hooks(hooks: TestHooks) -> Result<(PathBuf, RuntimeBridge)> {
        let root = std::env::temp_dir().join(format!("matou-supervisor-race-{}", Uuid::new_v4()));
        let home = root.join("home");
        fs::create_dir_all(&home)?;
        fs::write(home.join(".zshrc"), "PROMPT='RACE_READY> '\n")?;
        let exe = std::env::current_exe()?;
        let binary = exe
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("matou-native-runtime");
        let options = LaunchOptions {
            data_root: root.join("data"),
            workspace: root.join("workspace"),
            shell_home: home,
            shell: "/bin/zsh".into(),
        };
        Ok((
            root,
            RuntimeBridge::start_supervised(options, &binary, hooks)?,
        ))
    }
    fn next(bridge: &RuntimeBridge, mut accepts: impl FnMut(BridgeEvent) -> bool) -> Result<()> {
        let deadline = Instant::now() + LIMIT;
        loop {
            match bridge.events.try_recv() {
                Ok(event) => {
                    if accepts(event) {
                        return Ok(());
                    }
                }
                Err(async_channel::TryRecvError::Empty) => {}
                Err(e) => bail!(e),
            }
            ensure!(Instant::now() < deadline, "supervisor event deadline");
            thread::sleep(Duration::from_millis(3));
        }
    }
    fn ready(bridge: &RuntimeBridge) -> Result<(u64, String)> {
        let mut result = None;
        next(bridge, |event| {
            if let BridgeEvent::Runtime {
                generation,
                event: Event::Ready { hierarchy, .. },
            } = event
            {
                result = Some((generation, hierarchy.session.unwrap().id));
                true
            } else {
                false
            }
        })?;
        Ok(result.unwrap())
    }
    fn wait_for_shell_prompt(
        bridge: &RuntimeBridge,
        generation: u64,
        session: &str,
        prompt: &str,
    ) -> Result<()> {
        let mut applied = Ok(());
        next(bridge, |event| {
            let BridgeEvent::Runtime {
                generation: current,
                event:
                    Event::Frame {
                        session_id,
                        sequence,
                        screen,
                        ..
                    },
            } = event
            else {
                return false;
            };
            if current != generation || session_id != session {
                return false;
            }
            let has_prompt = screen.cells.chunks(screen.cols).any(|row| {
                let text: String = row.iter().map(|cell| cell.text.as_str()).collect();
                text.trim_end() == prompt
            });
            if has_prompt {
                // Leave the writer queue empty for the input-pause hook.
                return true;
            }
            applied = bridge.commands.send_at(
                generation,
                Command::Applied {
                    session_id,
                    sequence,
                },
            );
            applied.is_err()
        })
        .with_context(|| format!("wait for the Shell prompt in generation {generation}"))?;
        applied?;
        Ok(())
    }
    fn kill_owned(bridge: &RuntimeBridge, root: &Path) -> Result<u32> {
        kill_pid_owned(bridge.runtime_pid(), root)
    }
    fn kill_pid_owned(pid: u32, root: &Path) -> Result<u32> {
        ensure!(pid > 0 && pid != std::process::id());
        let output = ProcessCommand::new("ps")
            .args(["-p", &pid.to_string(), "-o", "ppid=,args="])
            .output()?;
        let actual = String::from_utf8(output.stdout)?;
        ensure!(
            actual.contains(root.to_str().unwrap())
                && actual.contains("matou-native-runtime")
                && actual
                    .split_whitespace()
                    .next()
                    .and_then(|pid| pid.parse::<u32>().ok())
                    == Some(std::process::id()),
            "not our child: {actual}"
        );
        println!("race test verified runtime child={pid} before kill; actual PPID/args: {actual}");
        ensure!(
            ProcessCommand::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status()?
                .success()
        );
        Ok(pid)
    }
    fn install(
        hook: &Mutex<Option<(Sender<()>, mpsc::Receiver<()>)>>,
    ) -> (mpsc::Receiver<()>, Sender<()>) {
        let (reached, wait) = mpsc::channel();
        let (resume, go) = mpsc::channel();
        *hook.lock().unwrap() = Some((reached, go));
        (wait, resume)
    }
    fn scheduled_delay(bridge: &RuntimeBridge, generation: u64) -> Duration {
        let transitions = bridge.commands.shared.hooks.transitions.lock().unwrap();
        let start = transitions
            .iter()
            .find(|(g, p, _)| *g == generation && *p == ConnectionPhase::Reconnecting)
            .unwrap()
            .2;
        let end = transitions
            .iter()
            .find(|(g, p, _)| *g == generation + 1 && *p == ConnectionPhase::Starting)
            .unwrap()
            .2;
        end.duration_since(start)
    }
    #[test]
    fn queued_old_input_paused_before_pipe_is_never_replayed() -> Result<()> {
        const NEW_INPUT_LIMIT: Duration = Duration::from_secs(30);
        let (root, mut bridge) = setup()?;
        let (g1, session) = ready(&bridge)?;
        // Ready opens the bridge before the Shell and its journal are initialized.
        wait_for_shell_prompt(&bridge, g1, &session, "RACE_READY>")?;
        let (paused, resume) = install(&bridge.commands.shared.hooks.before_business);
        let old = root.join("OLD_QUEUE_SIDE_EFFECT");
        let fresh = root.join("NEW_QUEUE_SIDE_EFFECT");
        bridge.commands.send_at(
            g1,
            Command::Input {
                session_id: session.clone(),
                bytes: format!("touch {}\r", old.display()).into_bytes(),
                defer_ordering: false,
            },
        )?;
        paused.recv_timeout(LIMIT)?;
        // A distinct prompt proves the replacement Shell itself has started.
        fs::write(root.join("home/.zshrc"), "PROMPT='RACE_RESTORED> '\n")?;
        kill_owned(&bridge, &root)?;
        // The reader must retire this exact attempt before its blocked writer can continue.
        let deadline = Instant::now() + LIMIT;
        while bridge.commands.connection().1 == ConnectionPhase::Ready {
            ensure!(
                Instant::now() < deadline,
                "reader did not retire dead attempt"
            );
            thread::sleep(Duration::from_millis(2));
        }
        resume.send(())?;
        let (g2, session2) = ready(&bridge)?;
        ensure!(g2 == g1 + 1 && session == session2);
        wait_for_shell_prompt(&bridge, g2, &session, "RACE_RESTORED>")?;
        bridge.commands.send_at(
            g2,
            Command::Input {
                session_id: session,
                bytes: format!("touch {}\r", fresh.display()).into_bytes(),
                defer_ordering: false,
            },
        )?;
        // Let the replacement shell execute its input under parallel test load.
        let deadline = Instant::now() + NEW_INPUT_LIMIT;
        while !fresh.exists() {
            ensure!(Instant::now() < deadline, "new input did not execute");
            thread::sleep(Duration::from_millis(4));
        }
        ensure!(!old.exists(), "old queued input executed on new child");
        bridge.shutdown()?;
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn queued_read_only_export_ends_once_and_does_not_replay_after_disconnect() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("matou-supervisor-read-only-{}", Uuid::new_v4()));
        let data_root = root.join("data");
        drop(matou_native_core::startup::open(
            &data_root,
            "read-only-fixture",
        )?);
        sql(
            &root,
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
                    VALUES (999, 'newer', 'newer', 1)",
        )?;
        let executable = std::env::current_exe()?;
        let binary = executable
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("matou-native-runtime");
        let mut bridge = RuntimeBridge::start_supervised(
            LaunchOptions {
                data_root,
                workspace: root.join("workspace"),
                shell_home: root.join("shell-home"),
                shell: "/unused/shell".into(),
            },
            &binary,
            TestHooks::default(),
        )?;
        let ready = |bridge: &RuntimeBridge, after| -> Result<u64> {
            let mut generation = None;
            next(bridge, |event| {
                if let BridgeEvent::Runtime {
                    generation: current,
                    event:
                        Event::ReadOnlyReady {
                            projection: Ok(_), ..
                        },
                } = event
                    && current > after
                {
                    generation = Some(current);
                    return true;
                }
                false
            })?;
            generation.context("read-only generation")
        };
        let generation = ready(&bridge, 0)?;
        assert_eq!(
            bridge.commands.send_at(generation, Command::Refresh),
            Err(SendError::StorageReadOnly)
        );
        let (paused, resume) = install(&bridge.commands.shared.hooks.before_business);
        // An invalid UUID still tests queue admission, and makes writing any
        // export impossible even if an older writer accidentally sends it.
        let request_id = "read-only-export-interrupted".to_owned();
        bridge.commands.send_at(
            generation,
            Command::DatabaseRecovery {
                request_id: request_id.clone(),
                action: DatabaseRecoveryAction::ExportRecoveryBundle,
            },
        )?;
        paused.recv_timeout(LIMIT)?;
        assert_eq!(
            bridge.commands.send_at(
                generation,
                Command::DatabaseRecovery {
                    request_id: "second-export".into(),
                    action: DatabaseRecoveryAction::ExportRecoveryBundle,
                }
            ),
            Err(SendError::RecoveryBusy),
        );
        kill_owned(&bridge, &root)?;
        next(&bridge, |event| {
            matches!(event, BridgeEvent::Runtime {
            generation: retired,
            event: Event::DatabaseRecoveryResult(result),
        } if retired == generation && result.request_id == request_id
            && result.error.as_ref().is_some_and(|error| error.message == DATABASE_RECOVERY_INTERRUPTED))
        })?;
        resume.send(())?;
        let next_generation = ready(&bridge, generation)?;
        assert_eq!(
            bridge.commands.connection(),
            (next_generation, ConnectionPhase::ReadOnly { pending: None })
        );
        assert!(matches!(
            bridge.commands.send_at(
                generation,
                Command::History(matou_native_runtime::history_worker::HistoryRequest::Page {
                    session_id: "unused".into(),
                    sequence: 1,
                    before: None,
                })
            ),
            Err(SendError::WrongGeneration { .. })
        ));
        bridge.commands.send_at(
            next_generation,
            Command::ReadOnlyQuery {
                request_id: 2,
                query: ReadOnlyQuery::Workspace {
                    workspace_id: "missing".into(),
                },
            },
        )?;
        next(&bridge, |event| {
            assert!(!matches!(&event, BridgeEvent::Runtime {
                event: Event::DatabaseRecoveryResult(result), ..
            } if result.request_id == request_id));
            matches!(event, BridgeEvent::Runtime {
                generation,
                event: Event::ReadOnlyQueryResult { request_id: 2, result: Err(_) },
            } if generation == next_generation)
        })?;
        bridge.shutdown()?;
        fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn actual_pre_ready_failures_follow_backoff_ready_resets_and_stop_interrupts_five_seconds()
    -> Result<()> {
        let (root, mut bridge) = setup()?;
        let (g1, _) = ready(&bridge)?;
        let (mut paused, mut resume) = install(&bridge.commands.shared.hooks.before_hello);
        kill_owned(&bridge, &root)?;
        let mut previous = g1;
        // First post-ready crash is100ms; four real pre-Hello deaths then exercise
        //500,1000,2000,5000ms. Hello never grants Ready/business eligibility.
        for expected in [100, 500, 1000, 2000, 5000] {
            next(
                &bridge,
                |e| matches!(e,BridgeEvent::Connection {generation,phase:ConnectionPhase::Reconnecting} if generation==previous),
            )?;
            paused.recv_timeout(LIMIT)?;
            let elapsed = scheduled_delay(&bridge, previous);
            ensure!(
                elapsed >= Duration::from_millis(expected),
                "backoff {expected}ms was {elapsed:?}"
            );
            println!(
                "actual pre-ready attempt={} delay>={expected}ms observed={elapsed:?}",
                previous + 1
            );
            ensure!(
                bridge.commands.send_at(previous + 1, Command::Refresh) == Err(SendError::NotReady)
            );
            if expected == 5000 {
                resume.send(())?;
                break;
            }
            let next_pause = install(&bridge.commands.shared.hooks.before_hello);
            kill_owned(&bridge, &root)?;
            resume.send(())?;
            (paused, resume) = next_pause;
            previous += 1;
        }
        let (ready_generation, _) = ready(&bridge)?;
        let (mut paused, mut resume) = install(&bridge.commands.shared.hooks.before_hello);
        kill_owned(&bridge, &root)?;
        previous = ready_generation;
        for expected in [100, 500, 1000, 2000] {
            next(
                &bridge,
                |e| matches!(e,BridgeEvent::Connection {generation,phase:ConnectionPhase::Reconnecting} if generation==previous),
            )?;
            paused.recv_timeout(LIMIT)?;
            let elapsed = scheduled_delay(&bridge, previous);
            ensure!(elapsed >= Duration::from_millis(expected));
            if expected == 100 {
                ensure!(
                    elapsed < Duration::from_secs(2),
                    "Ready did not reset backoff: {elapsed:?}"
                );
            }
            println!("after Ready g{ready_generation}: {expected}ms -> {elapsed:?}");
            let next_pause = if expected != 2000 {
                Some(install(&bridge.commands.shared.hooks.before_hello))
            } else {
                None
            };
            kill_owned(&bridge, &root)?;
            resume.send(())?;
            previous += 1;
            if let Some(next_pause) = next_pause {
                (paused, resume) = next_pause;
            }
        }
        next(
            &bridge,
            |e| matches!(e,BridgeEvent::Connection {generation,phase:ConnectionPhase::Reconnecting} if generation==previous),
        )?;
        let began = Instant::now();
        bridge.shutdown()?;
        let stop_duration = began.elapsed();
        ensure!(
            stop_duration < Duration::from_millis(400),
            "stop waited for5s backoff"
        );
        thread::sleep(Duration::from_millis(150));
        ensure!(
            bridge.commands.connection() == (previous, ConnectionPhase::Stopped)
                && bridge.runtime_pid() == 0,
            "spawn after explicit stop"
        );
        println!(
            "stop interrupted5s backoff in {:?}; generation remains{previous}",
            stop_duration
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn normal_child_exit_restarts_but_compatible_hello_does_not_open_business_gate() -> Result<()> {
        let (root, mut bridge) = setup()?;
        let (g1, _) = ready(&bridge)?;
        let old_pid = bridge.runtime_pid();
        let (paused, resume) = install(&bridge.commands.shared.hooks.after_hello);
        // Test-only access requests an orderly child exit, not a supervisor stop.
        // Production exposes Shutdown only through the independent lifecycle path.
        bridge
            .commands
            .shared
            .state
            .lock()
            .unwrap()
            .sender
            .as_ref()
            .unwrap()
            .send(Outgoing::Stop)?;
        paused.recv_timeout(LIMIT)?;
        let (g2, phase) = bridge.commands.connection();
        ensure!(g2 == g1 + 1 && phase == ConnectionPhase::Starting);
        ensure!(bridge.runtime_pid() != old_pid);
        ensure!(
            bridge.commands.send_at(g2, Command::Refresh) == Err(SendError::NotReady),
            "Hello opened business input before domain Ready"
        );
        resume.send(())?;
        ensure!(ready(&bridge)?.0 == g2);
        bridge.shutdown()?;
        ensure!(bridge.commands.connection() == (g2, ConnectionPhase::Stopped));
        fs::remove_dir_all(root)?;
        Ok(())
    }
    fn sql(root: &Path, statement: &str) -> Result<String> {
        let output = ProcessCommand::new("/usr/bin/sqlite3")
            .arg(root.join("data/matou.sqlite"))
            .arg(statement)
            .output()?;
        ensure!(
            output.status.success(),
            "sqlite read: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }
    #[test]
    fn normal_shutdown_preserves_previously_accepted_durable_command_fifo() -> Result<()> {
        let (root, mut bridge) = setup()?;
        let (generation, _) = ready(&bridge)?;
        let task = sql(
            &root,
            "SELECT id FROM tasks WHERE archived_at IS NULL LIMIT 1",
        )?;
        ensure!(!task.is_empty());
        let (paused, resume) = install(&bridge.commands.shared.hooks.before_business);
        bridge.commands.send_at(
            generation,
            Command::RenameTask {
                task_id: task.clone(),
                title: "NORMAL_QUIT_DURABLE_FIFO".into(),
            },
        )?;
        paused.recv_timeout(LIMIT)?;
        let commands = bridge.commands.clone();
        let shutdown = thread::spawn(move || bridge.shutdown());
        let state = commands.shared.state.lock().unwrap();
        let (state, _) = commands
            .shared
            .changed
            .wait_timeout_while(state, LIMIT, |state| !state.stopping)
            .unwrap();
        let stopping = state.stopping;
        drop(state);
        let rejected = commands.send_at(generation, Command::Refresh);
        resume.send(())?;
        shutdown
            .join()
            .map_err(|_| anyhow!("shutdown test thread panicked"))??;
        ensure!(
            stopping && rejected == Err(SendError::Stopped),
            "normal shutdown did not close admission"
        );
        let actual = sql(&root, &format!("SELECT title FROM tasks WHERE id='{task}'"))?;
        ensure!(
            actual == "NORMAL_QUIT_DURABLE_FIFO",
            "normal shutdown dropped an accepted durable command: title={actual:?}, fixture={}",
            root.display()
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn transient_initial_spawn_error_retries_then_runs_real_child() -> Result<()> {
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::NotADirectory,
            std::io::ErrorKind::InvalidInput,
        ] {
            ensure!(spawn_is_configuration(&std::io::Error::from(kind)));
        }
        for kind in [
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::Other,
        ] {
            ensure!(!spawn_is_configuration(&std::io::Error::from(kind)));
        }
        let hooks = TestHooks::default();
        *hooks.spawn_error_once.lock().unwrap() = Some(std::io::ErrorKind::WouldBlock);
        let (root, mut bridge) = setup_with_hooks(hooks)?;
        let verification = (|| -> Result<()> {
            ensure!(
                bridge.runtime_pid() == 0,
                "transient spawn failure invented a child PID"
            );
            ensure!(bridge.commands.send_at(1, Command::Refresh) == Err(SendError::NotReady));
            let (generation, session) = ready(&bridge)?;
            ensure!(generation == 2 && bridge.runtime_pid() > 0);
            let delay = scheduled_delay(&bridge, 1);
            ensure!(delay >= Duration::from_millis(100) && delay < Duration::from_secs(2));
            bridge.commands.send_at(
                generation,
                Command::Input {
                    session_id: session,
                    bytes: b"printf REAL_CHILD_SPAWN_RECOVERY > REAL_CHILD_SPAWN_RECOVERY\r"
                        .to_vec(),
                    defer_ordering: false,
                },
            )?;
            let marker = root.join("workspace/REAL_CHILD_SPAWN_RECOVERY");
            let deadline = Instant::now() + LIMIT;
            let mut last_observed = None;
            loop {
                ensure!(
                    Instant::now() < deadline,
                    "spawn marker did not reach exact content; last={last_observed:?}"
                );
                match fs::read_to_string(&marker) {
                    Ok(content) if content == "REAL_CHILD_SPAWN_RECOVERY" => break,
                    Ok(content) => last_observed = Some(content),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).context("read actual child spawn marker"),
                }
                thread::sleep(Duration::from_millis(3));
            }
            println!(
                "initial WouldBlock: pid0/notReady -> actual child={} Ready g{generation}, backoff={delay:?}",
                bridge.runtime_pid()
            );
            Ok(())
        })();
        // Execute both cleanup steps before propagating any verification failure.
        let shutdown = bridge.shutdown();
        let cleanup = fs::remove_dir_all(&root).context("remove this test's temporary root");
        let mut failures = Vec::new();
        for (stage, result) in [
            ("verification", verification),
            ("shutdown", shutdown),
            ("cleanup", cleanup),
        ] {
            if let Err(error) = result {
                failures.push(format!("{stage}: {error:#}"));
            }
        }
        ensure!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }

    #[test]
    fn active_shutdown_returns_actual_child_and_transport_failure() -> Result<()> {
        let (root, mut bridge) = setup()?;
        let (generation, _) = ready(&bridge)?;
        let pid = bridge.runtime_pid();
        let (paused, resume) = install(&bridge.commands.shared.hooks.before_business);
        bridge.commands.send_at(generation, Command::Refresh)?;
        paused.recv_timeout(LIMIT)?;
        let commands = bridge.commands.clone();
        let shutdown = thread::spawn(move || bridge.shutdown());
        let state = commands.shared.state.lock().unwrap();
        let (state, _) = commands
            .shared
            .changed
            .wait_timeout_while(state, LIMIT, |state| !state.stopping)
            .unwrap();
        let stopping = state.stopping;
        drop(state);
        kill_pid_owned(pid, &root)?;
        let deadline = Instant::now() + LIMIT;
        loop {
            let status = ProcessCommand::new("ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()?;
            // Reader EOF can arrive before the child closes its stdin. The
            // supervisor cannot reap it until this test releases the writer.
            if status.status.success()
                && String::from_utf8_lossy(&status.stdout)
                    .trim_start()
                    .starts_with('Z')
                && commands.connection().1 != ConnectionPhase::Ready
            {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "killed runtime {pid} did not exit and retire before resuming the writer"
            );
            thread::sleep(Duration::from_millis(2));
        }
        resume.send(())?;
        let result = shutdown
            .join()
            .map_err(|_| anyhow!("shutdown test thread panicked"))?;
        ensure!(stopping);
        let error = result.expect_err("active shutdown swallowed real child failure");
        let message = format!("{error:#}");
        ensure!(
            message.contains("reader:")
                && message.contains("writer:")
                && message.contains("child exit:"),
            "lost error sources: {message}"
        );
        println!("active shutdown error preserved: {message}");
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
