//! Native desktop connection to the serialized domain owner.
//!
//! No SQLite, PTY, or Ghostty handle is accessed on the GPUI thread. The current
//! vertical slice hosts the owner in a background thread; standalone runtime
//! transport and additional windows remain separate migration work.
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    ffi::OsString,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    SessionEvent, SessionOptions,
    history_worker::{HistoryReply, HistoryRequest, HistoryWorker},
    owner::{OwnerEvent, ShellOwner},
    search::{SearchRequest, SearchResult},
    worker::{SelectionCopy, WorkerCommand},
    workspace_paths::WorkspacePathChecks,
};
use anyhow::{Context, Result, anyhow, bail};
use matou_native_core::{
    hierarchy::{
        self, BootstrapWindowInput, HierarchyIds, HierarchyResult, Scene, Task, Workspace,
        canvas::{CanvasIds, CanvasMessages, CreateCanvasInput, CreateShellSiblingInput},
        cards::{CloseSessionCardInput, RemoveScope, RemoveSessionBranchInput},
        tabs::{CloseSceneInput, RenameSceneInput, ReorderSceneInput},
        tasks, workspaces,
    },
    journal::JournalOptions,
    pty::ShellLaunch,
    startup::{self, BootstrapResult},
    storage::CommandMetadata,
    workspace_paths::WorkspacePathState,
};
use matou_native_terminal::{
    GridSelection, InputModes, Screen, SelectionAction, TerminalOptions, Theme,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone)]
pub struct LaunchOptions {
    pub data_root: PathBuf,
    pub workspace: PathBuf,
    /// The acceptance launcher supplies a disposable home. No user shell files
    /// are rewritten or copied to this directory.
    pub shell_home: PathBuf,
    pub shell: PathBuf,
}

impl LaunchOptions {
    pub fn from_args(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut data_root = None;
        let mut workspace = None;
        let mut shell_home = None;
        let mut shell = PathBuf::from(if cfg!(windows) {
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        } else {
            "/bin/zsh"
        });
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .context("native acceptance launcher requires a path after each flag")?;
            let path = PathBuf::from(value);
            match flag.to_str() {
                Some("--data-root") => data_root = Some(path),
                Some("--workspace") => workspace = Some(path),
                Some("--shell-home") => shell_home = Some(path),
                Some("--shell") => shell = path,
                _ => bail!("Unknown native launcher option: {}", flag.to_string_lossy()),
            }
        }
        let options = LaunchOptions {
            data_root: data_root
                .context("Pass --data-root with an isolated native data directory")?,
            workspace: workspace.context("Pass --workspace with an isolated working directory")?,
            shell_home: shell_home.context("Pass --shell-home with an isolated Shell home")?,
            shell,
        };
        for path in [
            &options.data_root,
            &options.workspace,
            &options.shell_home,
            &options.shell,
        ] {
            if !path.is_absolute() {
                bail!("Native launcher paths must be absolute: {}", path.display());
            }
        }
        Ok(options)
    }
}

#[derive(Serialize, Deserialize)]
pub enum Command {
    History(HistoryRequest),
    Search {
        session_id: String,
        request: SearchRequest,
    },
    ClearSearch {
        session_id: String,
    },
    ScrollSelection {
        session_id: String,
        lines: i32,
    },
    Select {
        session_id: String,
        action: SelectionAction,
    },
    CopySelection {
        session_id: String,
        request_id: u64,
        interrupt_if_empty: bool,
    },
    Input {
        session_id: String,
        bytes: Vec<u8>,
        defer_ordering: bool,
    },
    Resize {
        session_id: String,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    Scroll {
        session_id: String,
        lines: i32,
    },
    ScrollTo {
        session_id: String,
        row: u64,
        request_id: u64,
    },
    Theme {
        session_id: String,
        theme: Theme,
    },
    Applied {
        session_id: String,
        sequence: u64,
    },
    RetryStorage {
        session_id: String,
    },
    RetryRecovery {
        session_id: String,
    },
    CreateWorkspace {
        root_directory: String,
    },
    SelectWorkspace {
        workspace_id: String,
    },
    RelinkWorkspace {
        workspace_id: String,
        root_directory: String,
    },
    SetWorkspacePinned {
        workspace_id: String,
        pinned: bool,
    },
    ReorderPinnedWorkspace {
        workspace_id: String,
        before_workspace_id: Option<String>,
    },
    RemoveWorkspace {
        workspace_id: String,
    },
    CreateTask {
        workspace_id: String,
    },
    SelectTask {
        task_id: String,
    },
    RenameTask {
        task_id: String,
        title: String,
    },
    SetTaskPinned {
        task_id: String,
        pinned: bool,
    },
    ReorderPinnedTask {
        workspace_id: String,
        task_id: String,
        before_task_id: Option<String>,
    },
    DeleteTask {
        task_id: String,
    },
    CreateCanvas {
        task_id: String,
    },
    CreateShellSibling {
        scene_id: String,
        source_session_id: String,
        parent_session_id: Option<String>,
    },
    SelectScene {
        scene_id: String,
    },
    RenameScene {
        scene_id: String,
        name: String,
    },
    ReorderScene {
        scene_id: String,
        before_scene_id: Option<String>,
    },
    CloseScene {
        scene_id: String,
        confirmed: bool,
    },
    PreviewSceneClose {
        scene_id: String,
    },
    CloseSessionCard {
        scene_id: String,
        session_id: String,
        confirmed_intent: Option<String>,
        #[serde(default)]
        preserve_scene_on_last_session: bool,
    },
    RemoveSessionBranch {
        scene_id: String,
        session_id: String,
        #[serde(default)]
        scope: RemoveScope,
    },
    FocusSession {
        scene_id: String,
        session_id: String,
    },
    PutGeometry {
        sequence: u64,
        update: matou_native_core::geometry::GeometryUpdate,
    },
    Refresh,
    #[cfg(all(test, target_os = "macos"))]
    #[serde(skip)]
    TestPause {
        paused: Sender<()>,
        resume: Receiver<()>,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryState {
    Restoring,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupBlockKind {
    ReadOnly,
    RecoveryRequired,
    MigrationChecksum,
    Configuration,
    Protocol,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupBlock {
    pub kind: StartupBlockKind,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub enum Event {
    StartupBlocked(StartupBlock),
    History(HistoryReply),
    SearchResults {
        session_id: String,
        sequence: u64,
        result: Result<SearchResult, String>,
    },
    Recovery {
        session_id: String,
        state: RecoveryState,
    },
    Workspaces(Vec<Workspace>),
    WorkspacePaths(Vec<WorkspacePathState>),
    Tasks(Vec<Task>),
    WorkspaceActionResult {
        action: String,
        workspace_id: Option<String>,
        error: Option<String>,
    },
    TaskRenameResult {
        task_id: String,
        title: String,
        error: Option<String>,
    },
    Ready {
        hierarchy: HierarchyResult,
        scenes: Vec<Scene>,
        graph: Value,
        geometry: Vec<matou_native_core::geometry::GeometryUpdate>,
    },
    Navigation {
        hierarchy: HierarchyResult,
        scenes: Vec<Scene>,
        graph: Value,
        geometry: Vec<matou_native_core::geometry::GeometryUpdate>,
    },
    NavigationFailed {
        message: String,
        attempted_focus: Option<(String, String)>,
    },
    /// Session IDs retired by a navigation command, before its projection.
    ScenesDisposed(Vec<String>),
    SceneClosePreview {
        scene_id: String,
        graph: Value,
        is_last: bool,
    },
    GeometryResult {
        sequence: u64,
        update: matou_native_core::geometry::GeometryUpdate,
        error: Option<String>,
    },
    /// Graph and optional `sessionCardAction` result for the final-card notice.
    Graph(Value),
    ClipboardWrite {
        session_id: String,
        text: String,
    },
    Copied {
        request_id: u64,
        text: String,
    },
    Frame {
        session_id: String,
        screen: Screen,
        selection: Option<GridSelection>,
        modes: InputModes,
        sequence: u64,
        /// All absolute scroll requests up to this id preceded the view read.
        scroll_request_id: u64,
    },
    StorageFault {
        session_id: String,
        message: String,
    },
    StorageRecovered {
        session_id: String,
    },
    Diagnostic(String),
    Closed,
}

pub struct RuntimeHost {
    pub commands: Sender<Command>,
    pub events: async_channel::Receiver<Event>,
    shutdown_requested: Arc<OnceLock<bool>>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl RuntimeHost {
    pub fn start(options: LaunchOptions) -> Result<Self> {
        let (commands, incoming) = mpsc::channel();
        let (send, events) = async_channel::unbounded();
        let shutdown_requested = Arc::new(OnceLock::new());
        let domain_shutdown = Arc::clone(&shutdown_requested);
        let thread = thread::Builder::new()
            .name("matou-domain-owner".into())
            .spawn(move || {
                let result = run(options, incoming, &send, &domain_shutdown);
                if let Err(error) = &result {
                    let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                }
                let _ = send.send_blocking(Event::Closed);
                result
            })?;
        Ok(Self {
            commands,
            events,
            shutdown_requested,
            thread: Some(thread),
        })
    }

    /// Lets the input thread request shutdown while the caller still owns the join handle.
    pub fn shutdown_request(&self) -> impl FnOnce(bool) + Send + use<> {
        let commands = self.commands.clone();
        let shutdown_requested = Arc::clone(&self.shutdown_requested);
        move |clean_exit| request_shutdown(&commands, &shutdown_requested, clean_exit)
    }

    pub fn shutdown(&mut self) -> Result<()> {
        self.shutdown_and_join(false)
    }

    pub fn shutdown_clean(&mut self) -> Result<()> {
        self.shutdown_and_join(true)
    }

    fn shutdown_and_join(&mut self, clean_exit: bool) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        request_shutdown(&self.commands, &self.shutdown_requested, clean_exit);
        thread
            .join()
            .map_err(|_| anyhow!("native runtime thread panicked"))?
    }
}

impl Drop for RuntimeHost {
    fn drop(&mut self) {
        request_shutdown(&self.commands, &self.shutdown_requested, false);
    }
}

fn request_shutdown(commands: &Sender<Command>, requested: &OnceLock<bool>, clean_exit: bool) {
    // Publish the first request's intent before waking the domain thread.
    if requested.set(clean_exit).is_ok() {
        let _ = commands.send(Command::Shutdown);
    }
}

struct PendingView {
    receive: Receiver<Result<(Screen, InputModes, Option<GridSelection>)>>,
    sequence: u64,
    scroll_request_id: u64,
}

struct RecoveryRetry {
    workspace_id: String,
    validated: bool,
}

struct PendingInput {
    workspace_id: String,
    session_id: String,
    bytes: Vec<u8>,
    defer_ordering: bool,
}

const WORKSPACE_PATH_INVALID: &str = "工作区目录不可用，请先在本地恢复原路径，或移出该工作区";
const ACTIVE_PATH_INTERVAL: Duration = Duration::from_millis(400);
const WORKSPACE_POLL_INTERVAL: Duration = Duration::from_secs(30);
type WorkerOperation = (String, Receiver<Result<()>>, bool);

fn run(
    options: LaunchOptions,
    incoming: Receiver<Command>,
    send: &async_channel::Sender<Event>,
    shutdown_requested: &OnceLock<bool>,
) -> Result<()> {
    let initialization = (|| -> Result<_> {
        std::fs::create_dir_all(&options.workspace)?;
        std::fs::create_dir_all(&options.shell_home)?;
        let history = HistoryWorker::new(options.data_root.clone())?;
        let generation = id();
        Ok((history, startup::open(&options.data_root, &generation)?))
    })();
    let (history, bootstrap) = match initialization {
        Ok(value) => value,
        Err(error) => {
            let kind = if error
                .downcast_ref::<matou_native_core::storage::MigrationChecksumMismatch>()
                .is_some()
            {
                Some(StartupBlockKind::MigrationChecksum)
            } else if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::NotADirectory
                        | std::io::ErrorKind::AlreadyExists
                )
            }) {
                Some(StartupBlockKind::Configuration)
            } else {
                None
            };
            if let Some(kind) = kind {
                let _ = send.send_blocking(Event::StartupBlocked(StartupBlock {
                    kind,
                    message: format!("{error:#}"),
                }));
            }
            return Err(error);
        }
    };
    let (mut connection, ownership, data_root) = match bootstrap {
        BootstrapResult::Writable {
            connection,
            ownership,
            data_root,
        } => (connection, ownership, data_root),
        BootstrapResult::ReadOnly { reason, .. } => {
            let message = format!("Native startup requires the recovery interface: {reason:?}");
            let _ = send.send_blocking(Event::StartupBlocked(StartupBlock {
                kind: StartupBlockKind::ReadOnly,
                message: message.clone(),
            }));
            bail!(message)
        }
        BootstrapResult::RecoveryRequired(recovery) => {
            let message = format!(
                "Database recovery required: {} ({})",
                recovery.reason,
                recovery.durable_database_path.display()
            );
            let _ = send.send_blocking(Event::StartupBlocked(StartupBlock {
                kind: StartupBlockKind::RecoveryRequired,
                message: message.clone(),
            }));
            bail!(message)
        }
    };
    let window_id = "main-window-1";
    let input = BootstrapWindowInput {
        window_id: window_id.into(),
        default_root_directory: options.workspace.to_string_lossy().into_owned(),
        default_name: options
            .workspace
            .file_name()
            .context("workspace name")?
            .to_string_lossy()
            .into_owned(),
        now: now(),
    };
    let hierarchy = hierarchy::bootstrap_window(
        &mut connection,
        &CommandMetadata {
            command_id: id(),
            command_type: "window.bootstrap".into(),
            request_hash: serde_json::to_string(&input)?,
            causation_id: None,
            correlation_id: None,
        },
        &input,
        &HierarchyIds {
            workspace_id: id(),
            execution_context_id: id(),
            task_id: id(),
            scene_id: id(),
            root_node_id: id(),
            session_id: id(),
            mount_id: id(),
        },
        "默认",
        now(),
    )?;
    let mut scene_id = hierarchy.scene.as_ref().map(|scene| scene.id.clone());
    let mut active_task = hierarchy.task.as_ref().map(|task| task.id.clone());
    let mut owner = ShellOwner::new(connection, ownership, id)?;
    let mut sessions = HashSet::new();
    let mut active_workspace = hierarchy
        .workspace
        .as_ref()
        .map(|workspace| workspace.id.clone());
    let mut checks = WorkspacePathChecks::default();
    let graph = active_graph(&owner, scene_id.as_deref(), window_id)?;
    let mut pending_launch = unstarted_shells(&graph, &sessions);
    let mut foreground_shells: HashSet<_> = shell_ids(&graph).map(str::to_owned).collect();
    let mut deferred_recovery = HashSet::new();
    let mut launch_directories = HashMap::<String, Receiver<Option<String>>>::new();
    let mut recovering = HashMap::<String, String>::new();
    // A failed launch stays blocked until an explicit retry starts a worker.
    let mut failed_recovery = HashSet::new();
    let mut recovery_retries = HashMap::<String, RecoveryRetry>::new();
    let mut launch_validated = false;
    let mut pending_input = VecDeque::<PendingInput>::new();
    let mut terminal_layouts = HashMap::new();
    let mut next_active_check = Instant::now() + ACTIVE_PATH_INTERVAL;
    let mut next_workspace_poll = Instant::now() + WORKSPACE_POLL_INTERVAL;
    if let Some(workspace_id) = &active_workspace {
        checks.request(&owner, workspace_id)?;
    }
    let mut projected_paths = owner
        .workspace_path_states()?
        .into_iter()
        .map(|state| (state.workspace_id.clone(), state))
        .collect::<BTreeMap<_, _>>();
    let mut active_path_invalid =
        workspace_is_invalid(&projected_paths, active_workspace.as_deref());
    send.send_blocking(Event::WorkspacePaths(
        projected_paths.values().cloned().collect(),
    ))?;
    let scenes = hierarchy
        .task
        .as_ref()
        .map(|task| owner.scenes(&task.id))
        .transpose()?
        .unwrap_or_default();
    send_sidebar(&owner, window_id, send)?;
    send.send_blocking(Event::Ready {
        hierarchy,
        scenes,
        graph,
        geometry: scene_id
            .as_deref()
            .map(|id| owner.geometry(id))
            .transpose()?
            .unwrap_or_default(),
    })?;
    let mut dirty = HashSet::<String>::new();
    let mut sequence = HashMap::<String, u64>::new();
    let mut views = HashMap::<String, PendingView>::new();
    let mut scroll_requests = HashMap::<String, u64>::new();
    let mut awaiting_ui = HashMap::<String, u64>::new();
    let mut operations = Vec::<WorkerOperation>::new();
    let mut searches = Vec::<(String, u64, Receiver<Result<SearchResult>>)>::new();
    let mut copies = Vec::<(u64, String, bool, Receiver<Result<SelectionCopy>>)>::new();
    let mut navigation_publications = VecDeque::new();
    let mut graph_dirty = false;
    let mut tasks_dirty = false;
    let clean_exit = 'running: loop {
        let now = Instant::now();
        if now >= next_active_check {
            if let Some(workspace_id) = &active_workspace {
                checks.request(&owner, workspace_id)?;
            }
            next_active_check = now + ACTIVE_PATH_INTERVAL;
        }
        if now >= next_workspace_poll {
            for workspace_id in owner.workspace_path_poll_targets()? {
                if let Err(error) = checks.request(&owner, &workspace_id) {
                    eprintln!("[workspace-path:{workspace_id}] {error:#}");
                }
            }
            next_workspace_poll = now + WORKSPACE_POLL_INTERVAL;
        }
        let completed_checks = checks.poll(&mut owner);
        let path_checked = !completed_checks.is_empty();
        for completion in completed_checks {
            let state = match completion {
                Ok(state) => state,
                Err(error) => {
                    send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                    continue;
                }
            };
            projected_paths.insert(state.workspace_id.clone(), state.clone());
            let invalid = state.status == "invalid";
            if invalid {
                let before = pending_input.len();
                pending_input.retain(|input| input.workspace_id != state.workspace_id);
                if pending_input.len() != before {
                    send.send_blocking(Event::Diagnostic(WORKSPACE_PATH_INVALID.into()))?;
                }
            }
            let retries: Vec<_> = recovery_retries
                .iter_mut()
                .filter(|(_, retry)| retry.workspace_id == state.workspace_id)
                .filter_map(|(session_id, retry)| {
                    retry.validated = !invalid;
                    invalid.then(|| session_id.clone())
                })
                .collect();
            for session_id in retries {
                recovery_retries.remove(&session_id);
                report_failed_recovery(send, &session_id)?;
            }
            if active_workspace.as_ref() != Some(&state.workspace_id) {
                continue;
            }
            if invalid {
                let failed: Vec<_> = pending_launch
                    .iter()
                    .filter(|id| !recovering.contains_key(*id))
                    .cloned()
                    .collect();
                for id in failed {
                    pending_launch.remove(&id);
                    report_failed_recovery(send, &id)?;
                    failed_recovery.insert(id);
                }
            } else {
                if active_path_invalid && let Some(task_id) = &active_task {
                    for scene in owner.scenes(task_id)? {
                        let graph = owner.graph(&scene.id, None)?;
                        deferred_recovery.extend(
                            shell_ids(&graph)
                                .filter(|id| sessions.contains(*id))
                                .map(str::to_owned),
                        );
                    }
                }
                // The reference increments the background pane's revision, then
                // replaces its worker only when the pane attaches in the foreground.
                let running: Vec<_> = foreground_shells
                    .iter()
                    .filter(|id| deferred_recovery.remove(*id))
                    .cloned()
                    .collect();
                owner.retire_sessions(&running);
                operations.retain(|(id, _, _)| !running.contains(id));
                for id in running {
                    sessions.remove(&id);
                    dirty.remove(&id);
                    sequence.remove(&id);
                    scroll_requests.remove(&id);
                    views.remove(&id);
                    awaiting_ui.remove(&id);
                    recovering.insert(id, state.workspace_id.clone());
                }
            }
            active_path_invalid = invalid;
            launch_validated = !invalid;
        }
        if path_checked {
            send.send_blocking(Event::WorkspacePaths(
                projected_paths.values().cloned().collect(),
            ))?;
        }

        let mut launch_candidates = HashSet::new();
        if launch_validated {
            launch_candidates.extend(
                pending_launch
                    .iter()
                    .filter(|id| !recovering.contains_key(*id))
                    .cloned(),
            );
        }
        for (session_id, workspace_id) in &recovering {
            if !owner.has_shell_worker(session_id)
                && owner
                    .workspace_path_state(workspace_id)?
                    .is_some_and(|state| state.status == "valid")
            {
                launch_candidates.insert(session_id.clone());
            }
        }
        launch_candidates.extend(
            recovery_retries
                .iter()
                .filter(|(_, retry)| retry.validated)
                .map(|(id, _)| id.clone()),
        );
        launch_directories.retain(|id, _| launch_candidates.contains(id));
        for session_id in launch_candidates {
            if !launch_directories.contains_key(&session_id) {
                let candidates =
                    owner.shell_directory_candidates(&session_id, &options.shell_home)?;
                launch_directories.insert(session_id, choose_launch_directory(candidates)?);
            }
        }
        let mut completed_launches = Vec::new();
        launch_directories.retain(|id, result| match result.try_recv() {
            Ok(directory) => {
                completed_launches.push((id.clone(), directory));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                completed_launches.push((id.clone(), None));
                false
            }
        });
        let mut started_shell = false;
        for (session_id, directory) in completed_launches {
            pending_launch.remove(&session_id);
            recovering.remove(&session_id);
            recovery_retries.remove(&session_id);
            let result = directory
                .context("execution context is not registered")
                .and_then(|directory| {
                    owner.set_shell_launch_directory(&session_id, &directory)?;
                    launch_shell(
                        &mut owner,
                        &options,
                        &session_id,
                        directory,
                        &terminal_layouts,
                    )
                });
            match result {
                Ok(()) => {
                    failed_recovery.remove(&session_id);
                    send.send_blocking(Event::Recovery {
                        session_id: session_id.clone(),
                        state: RecoveryState::Ready,
                    })?;
                    dirty.insert(session_id.clone());
                    sessions.insert(session_id);
                    graph_dirty = true;
                    started_shell = true;
                }
                Err(error) => {
                    pending_input.retain(|input| input.session_id != session_id);
                    report_failed_recovery(send, &session_id)?;
                    failed_recovery.insert(session_id);
                    send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                }
            }
        }
        if path_checked || started_shell {
            for _ in 0..pending_input.len() {
                let input = pending_input.pop_front().expect("queued terminal input");
                if sessions.contains(&input.session_id)
                    && !(foreground_shells.contains(&input.session_id)
                        && deferred_recovery.contains(&input.session_id))
                    && owner
                        .workspace_path_state(&input.workspace_id)?
                        .is_some_and(|state| state.status == "valid")
                {
                    if send_input(&mut owner, input, &mut operations, send)? {
                        tasks_dirty = true;
                        graph_dirty = true;
                    }
                } else {
                    pending_input.push_back(input);
                }
            }
        }
        for _ in 0..128 {
            // Resolve Ctrl+C before a subsequent focus/shutdown can overtake its
            // control interaction. Ordinary copy does not hold this queue.
            if copies.iter().any(|(_, _, interrupt, _)| *interrupt) {
                break;
            }
            let command = match incoming.try_recv() {
                Ok(command) => command,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'running false,
            };
            if matches!(
                &command,
                Command::CreateWorkspace { .. }
                    | Command::SelectWorkspace { .. }
                    | Command::RelinkWorkspace { .. }
                    | Command::SetWorkspacePinned { .. }
                    | Command::ReorderPinnedWorkspace { .. }
                    | Command::RemoveWorkspace { .. }
                    | Command::CreateTask { .. }
                    | Command::SelectTask { .. }
                    | Command::RenameTask { .. }
                    | Command::SetTaskPinned { .. }
                    | Command::ReorderPinnedTask { .. }
                    | Command::DeleteTask { .. }
                    | Command::CreateCanvas { .. }
                    | Command::CreateShellSibling { .. }
                    | Command::SelectScene { .. }
                    | Command::RenameScene { .. }
                    | Command::ReorderScene { .. }
                    | Command::CloseScene { .. }
                    | Command::CloseSessionCard { .. }
                    | Command::RemoveSessionBranch { .. }
                    | Command::FocusSession { .. }
            ) {
                // Each command mutates the authoritative domain before the view changes.
                // A failed launch leaves its real created structure available for recovery.
                let attempted_focus = match &command {
                    Command::FocusSession {
                        scene_id,
                        session_id,
                    } => Some((scene_id.clone(), session_id.clone())),
                    _ => None,
                };
                let rename = match &command {
                    Command::RenameTask { task_id, title } => {
                        Some((task_id.clone(), title.clone()))
                    }
                    _ => None,
                };
                let workspace_action = match &command {
                    Command::CreateWorkspace { .. } => Some(("create", None)),
                    Command::RelinkWorkspace { workspace_id, .. } => {
                        Some(("relink", Some(workspace_id.clone())))
                    }
                    Command::SetWorkspacePinned { workspace_id, .. } => {
                        Some(("pin", Some(workspace_id.clone())))
                    }
                    _ => None,
                };
                let check_target = matches!(
                    &command,
                    Command::CreateWorkspace { .. }
                        | Command::SelectWorkspace { .. }
                        | Command::RelinkWorkspace { .. }
                        | Command::CreateTask { .. }
                        | Command::SelectTask { .. }
                        | Command::CreateCanvas { .. }
                        | Command::CreateShellSibling { .. }
                        | Command::SelectScene { .. }
                );
                match navigate(&mut owner, window_id, command) {
                    Ok((hierarchy, disposed, card_action)) => {
                        if let Some(action) = &card_action
                            && action["outcome"] == "hide-window"
                        {
                            let mut graph = active_graph(&owner, scene_id.as_deref(), window_id)?;
                            graph["sessionCardAction"] = action.clone();
                            let event = Event::Graph(graph);
                            if navigation_publications.is_empty() {
                                send.send_blocking(event)?;
                            } else {
                                navigation_publications.push_back((Vec::new(), false, event));
                            }
                            continue;
                        }
                        let wait_for_workers = card_action.is_some();
                        let defer_publication =
                            wait_for_workers || !navigation_publications.is_empty();
                        if !disposed.is_empty() {
                            for id in &disposed {
                                sessions.remove(id);
                                dirty.remove(id);
                                sequence.remove(id);
                                scroll_requests.remove(id);
                                views.remove(id);
                                awaiting_ui.remove(id);
                                terminal_layouts.remove(id);
                                recovering.remove(id);
                                deferred_recovery.remove(id);
                                failed_recovery.remove(id);
                                recovery_retries.remove(id);
                                launch_directories.remove(id);
                            }
                            pending_input.retain(|input| !disposed.contains(&input.session_id));
                            operations.retain(|(id, _, _)| sessions.contains(id));
                            searches.retain(|(id, _, _)| !disposed.contains(id));
                            copies.retain(|(_, id, _, _)| !disposed.contains(id));
                            if !defer_publication {
                                send.send_blocking(Event::ScenesDisposed(disposed.clone()))?;
                            }
                        }
                        let next_scene = hierarchy.scene.as_ref().map(|scene| scene.id.clone());
                        let next_workspace = hierarchy
                            .workspace
                            .as_ref()
                            .map(|workspace| workspace.id.clone());
                        let workspace_changed = active_workspace != next_workspace;
                        let target_changed = scene_id != next_scene || workspace_changed;
                        if workspace_changed {
                            active_path_invalid =
                                workspace_is_invalid(&projected_paths, next_workspace.as_deref());
                        }
                        scene_id = next_scene;
                        active_task = hierarchy.task.as_ref().map(|task| task.id.clone());
                        active_workspace = next_workspace;
                        let mut graph = active_graph(&owner, scene_id.as_deref(), window_id)?;
                        pending_launch = unstarted_shells(&graph, &sessions);
                        pending_launch.retain(|id| !failed_recovery.contains(id));
                        foreground_shells = shell_ids(&graph).map(str::to_owned).collect();
                        if (target_changed || check_target || !pending_launch.is_empty())
                            && let Some(workspace_id) = &active_workspace
                        {
                            checks.request(&owner, workspace_id)?;
                            launch_validated = false;
                            next_active_check = Instant::now() + ACTIVE_PATH_INTERVAL;
                        }
                        let scenes = hierarchy
                            .task
                            .as_ref()
                            .map(|task| owner.scenes(&task.id))
                            .transpose()?
                            .unwrap_or_default();
                        if !defer_publication {
                            send_sidebar(&owner, window_id, send)?;
                        }
                        if let Some((action, workspace_id)) = workspace_action {
                            if action == "relink"
                                && let Some(workspace_id) = &workspace_id
                                && Some(workspace_id) != active_workspace.as_ref()
                            {
                                checks.request(&owner, workspace_id)?;
                            }
                            let workspace_id = workspace_id.or_else(|| {
                                hierarchy
                                    .workspace
                                    .as_ref()
                                    .map(|workspace| workspace.id.clone())
                            });
                            send.send_blocking(Event::WorkspaceActionResult {
                                action: action.into(),
                                workspace_id,
                                error: None,
                            })?;
                        }
                        if let Some((task_id, title)) = rename {
                            send.send_blocking(Event::TaskRenameResult {
                                task_id,
                                title,
                                error: None,
                            })?;
                        }
                        if let Some(mut action) = card_action {
                            // The outer graph is already the affected canvas for foreground actions.
                            if action["sceneId"] == graph["sceneId"]
                                && let Some(action) = action.as_object_mut()
                            {
                                action.remove("graph");
                            }
                            graph["sessionCardAction"] = action;
                        }
                        let event = Event::Navigation {
                            hierarchy,
                            scenes,
                            graph,
                            geometry: scene_id
                                .as_deref()
                                .map(|id| owner.geometry(id))
                                .transpose()?
                                .unwrap_or_default(),
                        };
                        if defer_publication {
                            navigation_publications.push_back((disposed, wait_for_workers, event));
                        } else {
                            send.send_blocking(event)?;
                        }
                        // Existing workers and terminal surfaces remain alive across tab changes.
                        dirty.extend(sessions.iter().cloned());
                    }
                    Err(error) => {
                        if let Some((action, workspace_id)) = workspace_action {
                            send.send_blocking(Event::WorkspaceActionResult {
                                action: action.into(),
                                workspace_id,
                                error: Some(format!("{error:#}")),
                            })?;
                        } else if let Some((task_id, title)) = rename {
                            send.send_blocking(Event::TaskRenameResult {
                                task_id,
                                title,
                                error: Some(format!("{error:#}")),
                            })?;
                        } else {
                            send.send_blocking(Event::NavigationFailed {
                                message: format!("{error:#}"),
                                attempted_focus,
                            })?;
                        }
                    }
                }
                continue;
            }
            let (reply, result) = mpsc::channel();
            let (session_id, request, refresh) = match command {
                Command::Input {
                    session_id,
                    bytes,
                    defer_ordering,
                } => {
                    if failed_recovery.contains(&session_id) {
                        continue;
                    }
                    let Some(workspace_id) = owner.session_workspace(&session_id)? else {
                        continue;
                    };
                    let state = owner.workspace_path_state(&workspace_id)?;
                    if state
                        .as_ref()
                        .is_some_and(|state| state.status == "invalid")
                    {
                        send.send_blocking(Event::Diagnostic(WORKSPACE_PATH_INVALID.into()))?;
                        continue;
                    }
                    let input = PendingInput {
                        workspace_id,
                        session_id,
                        bytes,
                        defer_ordering,
                    };
                    if state.is_none()
                        || !sessions.contains(&input.session_id)
                        || (foreground_shells.contains(&input.session_id)
                            && deferred_recovery.contains(&input.session_id))
                    {
                        checks.request(&owner, &input.workspace_id)?;
                        pending_input.push_back(input);
                    } else if send_input(&mut owner, input, &mut operations, send)? {
                        tasks_dirty = true;
                        graph_dirty = true;
                    }
                    continue;
                }
                Command::Resize {
                    session_id,
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                } => {
                    let live = sessions.contains(&session_id);
                    if live || owner.session_workspace(&session_id)?.is_some() {
                        let options = terminal_layouts
                            .entry(session_id.clone())
                            .or_insert_with(default_terminal_options);
                        options.cols = cols;
                        options.rows = rows;
                        options.cell_width_px = cell_width_px;
                        options.cell_height_px = cell_height_px;
                    }
                    if !live {
                        continue;
                    }
                    (
                        session_id,
                        WorkerCommand::Resize {
                            cols,
                            rows,
                            cell_width_px,
                            cell_height_px,
                            reply,
                        },
                        true,
                    )
                }
                Command::History(request) => {
                    history.send(request)?;
                    continue;
                }
                Command::Search {
                    session_id,
                    request,
                } => {
                    if sessions.contains(&session_id) {
                        let sequence = request.sequence;
                        let (reply, receive) = mpsc::channel();
                        owner.send(&session_id, WorkerCommand::Search { request, reply })?;
                        searches.push((session_id, sequence, receive));
                    }
                    continue;
                }
                Command::ClearSearch { session_id } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (session_id, WorkerCommand::ClearSearch { reply }, true)
                }
                Command::CopySelection {
                    session_id,
                    request_id,
                    interrupt_if_empty,
                } => {
                    if sessions.contains(&session_id) {
                        let (reply, receive) = mpsc::channel();
                        owner.send(
                            &session_id,
                            WorkerCommand::CopySelection {
                                reply,
                                interrupt_if_empty,
                            },
                        )?;
                        copies.push((request_id, session_id, interrupt_if_empty, receive));
                    }
                    continue;
                }
                Command::ScrollSelection { session_id, lines } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (
                        session_id,
                        WorkerCommand::ScrollSelection { lines, reply },
                        true,
                    )
                }
                Command::Select { session_id, action } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (session_id, WorkerCommand::Select { action, reply }, true)
                }
                Command::Scroll { session_id, lines } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (session_id, WorkerCommand::Scroll { lines, reply }, true)
                }
                Command::ScrollTo {
                    session_id,
                    row,
                    request_id,
                } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    scroll_requests.insert(session_id.clone(), request_id);
                    (session_id, WorkerCommand::ScrollTo { row, reply }, true)
                }
                Command::Theme { session_id, theme } => {
                    let live = sessions.contains(&session_id);
                    if live || owner.session_workspace(&session_id)?.is_some() {
                        terminal_layouts
                            .entry(session_id.clone())
                            .or_insert_with(default_terminal_options)
                            .theme = theme;
                    }
                    if !live {
                        continue;
                    }
                    (session_id, WorkerCommand::SetTheme { theme, reply }, true)
                }
                Command::Applied {
                    session_id,
                    sequence,
                } => {
                    if awaiting_ui.get(&session_id) != Some(&sequence) {
                        continue;
                    }
                    awaiting_ui.remove(&session_id);
                    // A layout frame before any PTY output has no output credit.
                    if sequence == 0 || !sessions.contains(&session_id) {
                        continue;
                    }
                    (
                        session_id,
                        WorkerCommand::Acknowledge { sequence, reply },
                        false,
                    )
                }
                Command::RetryRecovery { session_id } => {
                    if !failed_recovery.contains(&session_id)
                        || recovery_retries.contains_key(&session_id)
                    {
                        continue;
                    }
                    let Some(workspace_id) = owner.session_workspace(&session_id)? else {
                        continue;
                    };
                    send.send_blocking(Event::Recovery {
                        session_id: session_id.clone(),
                        state: RecoveryState::Restoring,
                    })?;
                    match checks.request(&owner, &workspace_id) {
                        Ok(()) => {
                            recovery_retries.insert(
                                session_id,
                                RecoveryRetry {
                                    workspace_id,
                                    validated: false,
                                },
                            );
                        }
                        Err(error) => {
                            report_failed_recovery(send, &session_id)?;
                            send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                        }
                    }
                    continue;
                }
                Command::RetryStorage { session_id } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (session_id, WorkerCommand::RetryStorage { reply }, true)
                }
                Command::PutGeometry { sequence, update } => {
                    let error = owner
                        .put_geometry(&update)
                        .err()
                        .map(|error| format!("{error:#}"));
                    send.send_blocking(Event::GeometryResult {
                        sequence,
                        update,
                        error,
                    })?;
                    continue;
                }
                Command::PreviewSceneClose { scene_id } => {
                    let event = match owner.scene_close_preview(window_id, &scene_id) {
                        Ok((graph, is_last)) => Event::SceneClosePreview {
                            scene_id,
                            graph,
                            is_last,
                        },
                        Err(error) => Event::NavigationFailed {
                            message: format!("{error:#}"),
                            attempted_focus: None,
                        },
                    };
                    if navigation_publications.is_empty() {
                        send.send_blocking(event)?;
                    } else {
                        navigation_publications.push_back((Vec::new(), false, event));
                    }
                    continue;
                }
                Command::Refresh => {
                    dirty.extend(sessions.iter().cloned());
                    awaiting_ui.clear();
                    let graph = active_graph(&owner, scene_id.as_deref(), window_id)?;
                    pending_launch = unstarted_shells(&graph, &sessions);
                    pending_launch.retain(|id| !failed_recovery.contains(id));
                    foreground_shells = shell_ids(&graph).map(str::to_owned).collect();
                    if let Some(workspace_id) = &active_workspace {
                        checks.request(&owner, workspace_id)?;
                        launch_validated = false;
                    }
                    if navigation_publications.is_empty() {
                        send.send_blocking(Event::Graph(graph))?;
                    } else {
                        graph_dirty = true;
                    }
                    continue;
                }
                Command::CreateWorkspace { .. }
                | Command::SelectWorkspace { .. }
                | Command::RelinkWorkspace { .. }
                | Command::SetWorkspacePinned { .. }
                | Command::ReorderPinnedWorkspace { .. }
                | Command::RemoveWorkspace { .. }
                | Command::CreateTask { .. }
                | Command::SelectTask { .. }
                | Command::RenameTask { .. }
                | Command::SetTaskPinned { .. }
                | Command::ReorderPinnedTask { .. }
                | Command::DeleteTask { .. }
                | Command::CreateCanvas { .. }
                | Command::CreateShellSibling { .. }
                | Command::SelectScene { .. }
                | Command::RenameScene { .. }
                | Command::ReorderScene { .. }
                | Command::CloseScene { .. }
                | Command::CloseSessionCard { .. }
                | Command::RemoveSessionBranch { .. }
                | Command::FocusSession { .. } => unreachable!("navigation handled above"),
                #[cfg(all(test, target_os = "macos"))]
                Command::TestPause { paused, resume } => {
                    paused.send(())?;
                    resume.recv_timeout(Duration::from_secs(8))?;
                    continue;
                }
                Command::Shutdown => {
                    break 'running shutdown_requested.get() == Some(&true);
                }
            };
            if let Err(error) = owner.send(&session_id, request) {
                send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                continue;
            }
            operations.push((session_id, result, refresh));
        }
        for event in owner.poll(Duration::from_millis(4))? {
            match event {
                OwnerEvent::Domain(_) => graph_dirty = true,
                OwnerEvent::Diagnostic { message, .. } => {
                    send.send_blocking(Event::Diagnostic(message))?
                }
                OwnerEvent::Terminal(event) => {
                    let session_id = event.session_id;
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    match event.event {
                        SessionEvent::Output { sequence: next, .. } => {
                            sequence.insert(session_id.clone(), next);
                            dirty.insert(session_id);
                        }
                        // The display credit window accounts for output only;
                        // start/resize/exit records are not acknowledged as bytes.
                        SessionEvent::Started { .. }
                        | SessionEvent::Resized { .. }
                        | SessionEvent::Exited { .. } => {
                            dirty.insert(session_id);
                        }
                        SessionEvent::SearchUpdated { result } => {
                            dirty.insert(session_id.clone());
                            send.send_blocking(Event::SearchResults {
                                session_id,
                                sequence: result.sequence,
                                result: Ok(result),
                            })?;
                        }
                        SessionEvent::ClipboardWrite { text } => {
                            send.send_blocking(Event::ClipboardWrite { session_id, text })?;
                        }
                        SessionEvent::ScreenInvalidated => {
                            dirty.insert(session_id);
                        }
                        SessionEvent::StorageFault { message, .. } => {
                            send.send_blocking(Event::StorageFault {
                                session_id,
                                message,
                            })?
                        }
                        SessionEvent::StorageRecovered { .. } => {
                            dirty.insert(session_id.clone());
                            send.send_blocking(Event::StorageRecovered { session_id })?;
                        }
                        SessionEvent::Diagnostic { message } => {
                            send.send_blocking(Event::Diagnostic(message))?
                        }
                        _ => {}
                    }
                }
            }
        }
        while let Some(reply) = history.try_recv()? {
            send.send_blocking(Event::History(reply))?;
        }
        searches.retain(|(session_id, sequence, receive)| {
            let result = match receive.try_recv() {
                Ok(result) => result.map_err(|error| format!("{error:#}")),
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => Err("Shell search disconnected".into()),
            };
            dirty.insert(session_id.clone());
            let _ = send.send_blocking(Event::SearchResults {
                session_id: session_id.clone(),
                sequence: *sequence,
                result,
            });
            false
        });
        copies.retain(
            |(request_id, session_id, _, receive)| match receive.try_recv() {
                Ok(Ok(SelectionCopy { text, interrupted })) => {
                    if interrupted {
                        match owner.record_interaction(session_id, "control", true) {
                            Ok(_) => {
                                graph_dirty = true;
                                tasks_dirty = true;
                            }
                            Err(error) => {
                                let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                            }
                        }
                    }
                    dirty.insert(session_id.clone());
                    let _ = send.send_blocking(Event::Copied {
                        request_id: *request_id,
                        text,
                    });
                    false
                }
                Ok(Err(error)) => {
                    let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                    false
                }
                Err(mpsc::TryRecvError::Empty) => true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = send.send_blocking(Event::Diagnostic("Shell copy disconnected".into()));
                    false
                }
            },
        );
        operations.retain(|(session_id, result, refresh)| match result.try_recv() {
            Ok(Ok(())) => {
                if *refresh {
                    dirty.insert(session_id.clone());
                }
                false
            }
            Ok(Err(error)) => {
                let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = send.send_blocking(Event::Diagnostic("Shell request disconnected".into()));
                false
            }
        });
        // Keep processing other Shell traffic while the existing owner loop
        // drains SavedExit and joins each retired worker. Publish in command order.
        while navigation_publications
            .front()
            .is_some_and(|(disposed, wait, _)| {
                !wait || disposed.iter().all(|id| !owner.has_shell_worker(id))
            })
        {
            let (disposed, _, event) = navigation_publications
                .pop_front()
                .expect("navigation result");
            if !disposed.is_empty() {
                send.send_blocking(Event::ScenesDisposed(disposed))?;
            }
            if matches!(event, Event::Navigation { .. }) {
                send_sidebar(&owner, window_id, send)?;
            }
            send.send_blocking(event)?;
        }
        if graph_dirty && navigation_publications.is_empty() {
            send.send_blocking(Event::Graph(active_graph(
                &owner,
                scene_id.as_deref(),
                window_id,
            )?))?;
            graph_dirty = false;
        }
        if tasks_dirty && navigation_publications.is_empty() {
            send_sidebar(&owner, window_id, send)?;
            tasks_dirty = false;
        }
        views.retain(|session_id, pending| match pending.receive.try_recv() {
            Ok(Ok((screen, modes, selection))) => {
                awaiting_ui.insert(session_id.clone(), pending.sequence);
                let _ = send.send_blocking(Event::Frame {
                    session_id: session_id.clone(),
                    screen,
                    selection,
                    modes,
                    sequence: pending.sequence,
                    scroll_request_id: pending.scroll_request_id,
                });
                false
            }
            Ok(Err(error)) => {
                let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = send.send_blocking(Event::Diagnostic("Shell view disconnected".into()));
                false
            }
        });
        for session_id in dirty.clone() {
            if views.contains_key(&session_id) || awaiting_ui.contains_key(&session_id) {
                continue;
            }
            let (reply, receive) = mpsc::channel();
            owner.send(&session_id, WorkerCommand::SelectedView { reply })?;
            views.insert(
                session_id.clone(),
                PendingView {
                    receive,
                    sequence: sequence.get(&session_id).copied().unwrap_or(0),
                    scroll_request_id: scroll_requests.get(&session_id).copied().unwrap_or(0),
                },
            );
            dirty.remove(&session_id);
        }
    };
    let sessions = owner.shutdown().context("stop native runtime sessions");
    let backup = clean_exit.then(|| owner.backup_for_clean_exit(&data_root));
    drop(owner);
    shutdown_result(sessions.map(|_| ()), backup)
}

fn shutdown_result(sessions: Result<()>, backup: Option<Result<()>>) -> Result<()> {
    match (sessions, backup) {
        (Err(sessions), Some(Err(backup))) => Err(sessions.context(format!(
            "session shutdown and clean exit backup both failed ({backup:#})"
        ))),
        (Err(sessions), Some(Ok(()))) => Err(sessions.context("clean exit backup completed")),
        (Ok(()), Some(backup)) => backup,
        (sessions, None) => sessions,
    }
}

fn report_failed_recovery(send: &async_channel::Sender<Event>, session_id: &str) -> Result<()> {
    send.send_blocking(Event::Recovery {
        session_id: session_id.to_owned(),
        state: RecoveryState::Failed(format!(
            "Session {session_id} did not reach a running state"
        )),
    })?;
    Ok(())
}

fn send_input(
    owner: &mut ShellOwner,
    input: PendingInput,
    operations: &mut Vec<WorkerOperation>,
    send: &async_channel::Sender<Event>,
) -> Result<bool> {
    let PendingInput {
        session_id,
        bytes,
        defer_ordering,
        ..
    } = input;
    let interaction = match bytes.as_slice() {
        [3] | [4] => Some("control"),
        [13] | [10] => Some("submit"),
        _ => None,
    };
    let (reply, result) = mpsc::channel();
    if let Err(error) = owner.send(&session_id, WorkerCommand::Write { bytes, reply }) {
        send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
        return Ok(false);
    }
    // Record the interaction after queueing bytes and before a queued focus or shutdown.
    let changed = if let Some(kind) = interaction {
        match owner.record_interaction(&session_id, kind, defer_ordering) {
            Ok(_) => true,
            Err(error) => {
                send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                false
            }
        }
    } else {
        false
    };
    operations.push((session_id, result, true));
    Ok(changed)
}

fn workspace_is_invalid(paths: &BTreeMap<String, WorkspacePathState>, id: Option<&str>) -> bool {
    id.and_then(|id| paths.get(id))
        .is_some_and(|state| state.status == "invalid")
}

fn shell_ids(graph: &Value) -> impl Iterator<Item = &str> {
    graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["currentMode"] == "shell" && node.get("archivedAt").is_none())
        .filter_map(|node| node["sessionId"].as_str())
}

fn unstarted_shells(graph: &Value, sessions: &HashSet<String>) -> HashSet<String> {
    shell_ids(graph)
        .filter(|id| !sessions.contains(*id))
        .map(str::to_owned)
        .collect()
}

fn default_terminal_options() -> TerminalOptions {
    TerminalOptions::new(80, 24, 7, 14)
}

fn choose_launch_directory(candidates: Vec<String>) -> Result<Receiver<Option<String>>> {
    let (send, receive) = mpsc::channel();
    thread::Builder::new()
        .name("shell-directory".into())
        .spawn(move || {
            let directory = candidates.into_iter().find(|path| Path::new(path).is_dir());
            let _ = send.send(directory);
        })?;
    Ok(receive)
}

fn launch_shell(
    owner: &mut ShellOwner,
    options: &LaunchOptions,
    session_id: &str,
    cwd: String,
    terminal_layouts: &HashMap<String, TerminalOptions>,
) -> Result<()> {
    let terminal = terminal_layouts
        .get(session_id)
        .copied()
        .unwrap_or_else(default_terminal_options);
    let mut inherited_env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    inherited_env.insert("HOME".into(), options.shell_home.clone().into_os_string());
    inherited_env.insert(
        "ZDOTDIR".into(),
        options.shell_home.clone().into_os_string(),
    );
    inherited_env.insert("SHELL".into(), options.shell.clone().into_os_string());
    owner.launch(
        SessionOptions {
            session_id: session_id.to_owned(),
            launch: ShellLaunch {
                executable: options.shell.clone(),
                cwd: cwd.into(),
                data_root: options.data_root.clone(),
                inherited_env,
                env: BTreeMap::new(),
                cols: terminal.cols,
                rows: terminal.rows,
            },
            terminal,
            journal: JournalOptions::default(),
            restored_history: Vec::new(),
            collect_history: true,
        },
        true,
        "──────── 会话已恢复 ────────",
    )
}

fn active_graph(owner: &ShellOwner, scene_id: Option<&str>, window_id: &str) -> Result<Value> {
    match scene_id {
        Some(scene_id) => owner.graph(scene_id, Some(window_id)),
        None => Ok(serde_json::json!({"nodes": []})),
    }
}

fn send_sidebar(
    owner: &ShellOwner,
    window_id: &str,
    send: &async_channel::Sender<Event>,
) -> Result<()> {
    let mut workspaces = owner.workspaces()?;
    workspaces.sort_by(|left, right| {
        right
            .is_pinned
            .cmp(&left.is_pinned)
            .then_with(|| {
                if left.is_pinned {
                    left.pin_sort_key.cmp(&right.pin_sort_key)
                } else {
                    right
                        .last_opened_at
                        .cmp(&left.last_opened_at)
                        .then(left.created_at.cmp(&right.created_at))
                }
            })
            .then(left.id.cmp(&right.id))
    });
    let mut tasks = Vec::new();
    for workspace in &workspaces {
        tasks.extend(owner.tasks(window_id, &workspace.id)?);
    }
    send.send_blocking(Event::Workspaces(workspaces))?;
    send.send_blocking(Event::Tasks(tasks))?;
    Ok(())
}

fn resolve_workspace_directory(directory: &str) -> Result<PathBuf> {
    let mut resolved = PathBuf::new();
    let directory = Path::new(if directory.is_empty() { "." } else { directory });
    for component in std::path::absolute(directory)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    Ok(resolved)
}

fn navigate(
    owner: &mut ShellOwner,
    window_id: &str,
    command: Command,
) -> Result<(HierarchyResult, Vec<String>, Option<Value>)> {
    let time = now();
    let ids = CanvasIds {
        scene_id: id(),
        root_node_id: id(),
        secondary_node_id: id(),
        session_id: id(),
        mount_id: id(),
        relation_id: id(),
    };
    let messages = CanvasMessages {
        new_canvas: "新画布",
        duplicate_scene_name: "当前事项下已存在同名页签",
        workspace_path_invalid: WORKSPACE_PATH_INVALID,
        detached_session_must_return: "独立窗口中的会话需要先回到原会话列表",
    };
    let metadata = |kind: &str, input: &Value| CommandMetadata {
        command_id: id(),
        command_type: kind.into(),
        request_hash: input.to_string(),
        causation_id: None,
        correlation_id: None,
    };
    match command {
        Command::CreateWorkspace { root_directory } => {
            let directory = resolve_workspace_directory(&root_directory)?;
            let input = workspaces::CreateWorkspaceInput {
                window_id: window_id.into(),
                name: directory
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "工作区".into()),
                root_directory: directory.to_string_lossy().into_owned(),
                navigation: None,
                now: time,
            };
            owner.create_workspace(
                &metadata("hierarchy.create-workspace", &serde_json::to_value(&input)?),
                &input,
                &hierarchy_ids(),
                time,
            )?;
        }
        Command::SelectWorkspace { workspace_id } => {
            owner.select_workspace(&workspaces::SelectWorkspaceInput {
                window_id: window_id.into(),
                workspace_id,
                now: time,
            })?;
        }
        Command::RelinkWorkspace {
            workspace_id,
            root_directory,
        } => {
            let directory = resolve_workspace_directory(&root_directory)?;
            let input = workspaces::RelinkWorkspaceInput {
                workspace_id,
                root_directory: directory.to_string_lossy().into_owned(),
                now: time,
            };
            owner.relink_workspace(
                &metadata("hierarchy.relink-workspace", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
        }
        Command::SetWorkspacePinned {
            workspace_id,
            pinned,
        } => {
            let input = workspaces::SetWorkspacePinnedInput {
                workspace_id,
                pinned,
                now: time,
            };
            owner.set_workspace_pinned(
                &metadata(
                    "hierarchy.set-workspace-pinned",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                time,
            )?;
        }
        Command::ReorderPinnedWorkspace {
            workspace_id,
            before_workspace_id,
        } => {
            let input = workspaces::ReorderPinnedWorkspaceInput {
                workspace_id,
                before_workspace_id,
                now: time,
            };
            owner.reorder_pinned_workspace(
                &metadata(
                    "hierarchy.reorder-pinned-workspace",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                time,
            )?;
        }
        Command::RemoveWorkspace { workspace_id } => {
            let input = workspaces::RemoveWorkspaceInput {
                window_id: window_id.into(),
                confirmed_intent: format!("remove-workspace:{workspace_id}"),
                workspace_id,
                now: time,
            };
            let disposed = owner.remove_workspace(
                &metadata("hierarchy.remove-workspace", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
            return Ok((owner.hierarchy(window_id)?, disposed, None));
        }
        Command::CreateTask { workspace_id } => {
            let input = tasks::CreateTaskInput {
                window_id: window_id.into(),
                workspace_id,
                title: None,
                navigation: None,
                now: time,
            };
            owner.create_task(
                &metadata("hierarchy.create-task", &serde_json::to_value(&input)?),
                &input,
                &hierarchy_ids(),
                time,
            )?;
        }
        Command::SelectTask { task_id } => {
            owner.select_task(&tasks::SelectTaskInput {
                window_id: window_id.into(),
                task_id,
                now: time,
            })?;
        }
        Command::RenameTask { task_id, title } => {
            let input = tasks::RenameTaskInput {
                task_id,
                title,
                now: time,
            };
            owner.rename_task(
                &metadata("hierarchy.rename-task", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
        }
        Command::SetTaskPinned { task_id, pinned } => {
            let input = tasks::SetTaskPinnedInput {
                task_id,
                pinned,
                now: time,
            };
            owner.set_task_pinned(
                &metadata("hierarchy.set-task-pinned", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
        }
        Command::ReorderPinnedTask {
            workspace_id,
            task_id,
            before_task_id,
        } => {
            let input = tasks::ReorderPinnedTaskInput {
                workspace_id,
                task_id,
                before_task_id,
                now: time,
            };
            owner.reorder_pinned_task(
                &metadata(
                    "hierarchy.reorder-pinned-task",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                time,
            )?;
        }
        Command::DeleteTask { task_id } => {
            let input = tasks::DeleteTaskInput {
                window_id: window_id.into(),
                confirmed_intent: Some(format!("delete-task:{task_id}")),
                task_id,
                now: time,
            };
            let disposed = owner.delete_task(
                &metadata("hierarchy.delete-task", &serde_json::to_value(&input)?),
                &input,
                &hierarchy_ids(),
                time,
            )?;
            return Ok((owner.hierarchy(window_id)?, disposed, None));
        }
        Command::CreateCanvas { task_id } => {
            let input = CreateCanvasInput {
                window_id: window_id.into(),
                task_id,
                title: None,
                navigation: None,
                now: time,
            };
            owner.create_canvas(
                &metadata("hierarchy.create-canvas", &serde_json::to_value(&input)?),
                &input,
                &ids,
                &messages,
                time,
            )?;
        }
        Command::CreateShellSibling {
            scene_id,
            source_session_id,
            parent_session_id,
        } => {
            let input = CreateShellSiblingInput {
                window_id: window_id.into(),
                scene_id,
                source_session_id,
                parent_session_id,
                execution_context_id: None,
                title: None,
                navigation: None,
                now: time,
            };
            owner.create_shell_sibling(
                &metadata(
                    "hierarchy.create-shell-sibling",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                &ids,
                &messages,
                time,
            )?;
        }
        Command::SelectScene { scene_id } => {
            owner.select_scene(window_id, &scene_id, time)?;
        }
        Command::RenameScene { scene_id, name } => {
            let input = RenameSceneInput {
                scene_id,
                name,
                now: time,
            };
            owner.rename_scene(
                &metadata("hierarchy.rename-scene", &serde_json::to_value(&input)?),
                &input,
                messages.duplicate_scene_name,
                time,
            )?;
        }
        Command::ReorderScene {
            scene_id,
            before_scene_id,
        } => {
            let input = ReorderSceneInput {
                window_id: window_id.into(),
                scene_id,
                before_scene_id,
                now: time,
            };
            owner.reorder_scene(
                &metadata("hierarchy.reorder-scene", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
        }
        Command::CloseScene {
            scene_id,
            confirmed,
        } => {
            let confirmed_intent = confirmed.then(|| format!("close-scene:{scene_id}"));
            let input = CloseSceneInput {
                window_id: window_id.into(),
                scene_id,
                confirmed_intent,
                now: time,
            };
            let disposed = owner.close_scene(
                &metadata("hierarchy.close-scene", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
            return Ok((owner.hierarchy(window_id)?, disposed, None));
        }
        Command::CloseSessionCard {
            scene_id,
            session_id,
            confirmed_intent,
            preserve_scene_on_last_session,
        } => {
            let input = CloseSessionCardInput {
                window_id: window_id.into(),
                scene_id,
                session_id,
                confirmed_intent,
                preserve_scene_on_last_session,
                now: time,
            };
            let result = owner.close_session_card(
                &metadata("hierarchy.delete-session", &serde_json::to_value(&input)?),
                &input,
                &hierarchy_ids(),
                time,
            )?;
            let disposed = serde_json::from_value(result["disposedSessionIds"].clone())?;
            let mut action = json!({
                "action": "close-session-card",
                "sceneId": input.scene_id,
                "sessionId": input.session_id,
                "outcome": result["outcome"],
                "rebuildCanvas": result["rebuildCanvas"],
                "disposedSessionIds": disposed,
            });
            if action["outcome"] == "hide-window" {
                action["notice"] = json!({
                    "body": "当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。",
                    "confirmLabel": "我知道了",
                });
            }
            return Ok((owner.hierarchy(window_id)?, disposed, Some(action)));
        }
        Command::RemoveSessionBranch {
            scene_id,
            session_id,
            scope,
        } => {
            let input = RemoveSessionBranchInput {
                window_id: window_id.into(),
                scene_id,
                session_id,
                scope,
                now: time,
            };
            let result = owner.remove_session_branch(
                &metadata(
                    "hierarchy.remove-session-branch",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                time,
            )?;
            let disposed = serde_json::from_value(result["disposedSessionIds"].clone())?;
            let action = json!({
                "action": "remove-session-branch",
                "sceneId": input.scene_id,
                "sessionId": input.session_id,
                "scope": scope,
                "graph": result["graph"],
                "removedSessionIds": result["removedSessionIds"],
                "disposedSessionIds": disposed,
            });
            return Ok((owner.hierarchy(window_id)?, disposed, Some(action)));
        }
        Command::FocusSession {
            scene_id,
            session_id,
        } => {
            owner.focus_session(
                window_id,
                &scene_id,
                &session_id,
                time,
                "会话不在当前画布中",
            )?;
        }
        _ => unreachable!("only navigation commands reach navigate"),
    }
    Ok((owner.hierarchy(window_id)?, Vec::new(), None))
}

fn hierarchy_ids() -> HierarchyIds {
    HierarchyIds {
        workspace_id: id(),
        execution_context_id: id(),
        task_id: id(),
        scene_id: id(),
        root_node_id: id(),
        session_id: id(),
        mount_id: id(),
    }
}
fn id() -> String {
    Uuid::new_v4().to_string()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn session_shutdown_failure_reports_backup_outcome() {
        let session_error = || std::io::Error::other("worker cleanup failed").into();
        let completed = shutdown_result(Err(session_error()), Some(Ok(()))).unwrap_err();
        assert!(format!("{completed:#}").contains("clean exit backup completed"));
        assert!(completed.downcast_ref::<std::io::Error>().is_some());

        let backup =
            anyhow!("backup file could not be written").context("clean exit backup failed");
        let failed = shutdown_result(Err(session_error()), Some(Err(backup))).unwrap_err();
        let report = format!("{failed:#}");
        assert!(report.contains("worker cleanup failed"));
        assert!(report.contains("clean exit backup failed: backup file could not be written"));
        assert!(failed.downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn interaction_precedes_queued_focus_and_shutdown() -> Result<()> {
        let root = std::env::temp_dir().join(format!("matou-interaction-order-{}", id()));
        check_interaction_order(&root)
            .with_context(|| format!("isolated bridge data: {}", root.display()))?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    fn check_interaction_order(root: &std::path::Path) -> Result<()> {
        let options = LaunchOptions {
            data_root: root.join("data"),
            workspace: root.join("workspace"),
            shell_home: root.join("home"),
            shell: "/bin/zsh".into(),
        };
        let database = options.data_root.join("matou.sqlite");
        let window_id = "main-window-1";
        let (scene, first, second) = {
            let BootstrapResult::Writable {
                mut connection,
                ownership,
                ..
            } = startup::open(&options.data_root, "seed")?
            else {
                bail!("test database is not writable");
            };
            let original = hierarchy::bootstrap_window(
                &mut connection,
                &CommandMetadata {
                    command_id: id(),
                    command_type: "window.bootstrap".into(),
                    request_hash: "seed".into(),
                    causation_id: None,
                    correlation_id: None,
                },
                &BootstrapWindowInput {
                    window_id: window_id.into(),
                    default_root_directory: options.workspace.to_string_lossy().into_owned(),
                    default_name: "Workspace".into(),
                    now: 1,
                },
                &hierarchy_ids(),
                "默认",
                1,
            )?;
            let scene = original.scene.context("seed scene")?.id;
            let first = original.session.context("seed session")?.id;
            let mut owner = ShellOwner::new(connection, ownership, id)?;
            let (selection, _, _) = navigate(
                &mut owner,
                window_id,
                Command::CreateShellSibling {
                    scene_id: scene.clone(),
                    source_session_id: first.clone(),
                    parent_session_id: None,
                },
            )?;
            (
                scene,
                first,
                selection.session.context("sibling session")?.id,
            )
        };

        let mut bridge = RuntimeHost::start(options)?;
        let mut ready = HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !ready.contains(&first) || !ready.contains(&second) {
            anyhow::ensure!(
                Instant::now() < deadline,
                "timed out waiting for Shell startup"
            );
            match bridge.events.try_recv() {
                Ok(Event::Frame { session_id, .. }) => {
                    ready.insert(session_id);
                }
                Ok(Event::Diagnostic(message)) => bail!("{message}"),
                Err(async_channel::TryRecvError::Closed) => {
                    bail!("bridge stopped during Shell startup")
                }
                Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(2)),
                _ => {}
            }
        }
        // Hold the owner at a test barrier so all four messages are in one batch.
        let (paused, wait_paused) = mpsc::channel();
        let (resume, wait_resume) = mpsc::channel();
        bridge
            .commands
            .send(Command::TestPause {
                paused,
                resume: wait_resume,
            })
            .map_err(|_| anyhow!("runtime command channel closed"))?;
        wait_paused.recv_timeout(Duration::from_secs(8))?;
        let input = |session: &str| Command::Input {
            session_id: session.to_owned(),
            bytes: vec![13],
            defer_ordering: true,
        };
        for command in [
            input(&second),
            Command::FocusSession {
                scene_id: scene,
                session_id: first.clone(),
            },
            input(&first),
            Command::Shutdown,
        ] {
            bridge
                .commands
                .send(command)
                .map_err(|_| anyhow!("runtime command channel closed"))?;
        }
        resume.send(())?;
        let events = bridge.events.clone();
        bridge.shutdown()?;

        let focus_graph = std::iter::from_fn(|| events.try_recv().ok())
            .find_map(|event| match event {
                Event::Navigation { graph, .. } => Some(graph),
                _ => None,
            })
            .context("focus navigation graph")?;
        let previous = focus_graph["nodes"]
            .as_array()
            .context("focus navigation nodes")?
            .iter()
            .find(|node| node["sessionId"] == second)
            .context("previously focused Shell")?;
        let connection = startup::open_read_only(&database)?;
        let membership = |session: &str| {
            connection.query_row(
                "SELECT last_user_interaction_seq, pending_user_interaction_seq
                 FROM session_canvas_memberships WHERE session_id = ?",
                [session],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
        };
        let receipts: i64 = connection.query_row(
            "SELECT count(*) FROM command_deduplication WHERE command_type = 'session.user-interaction'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            (
                previous["lastUserInteractionSeq"].as_i64(),
                membership(&second)?,
                membership(&first)?,
                receipts
            ),
            (Some(1), (1, 0), (0, 2), 2),
            "the switch must apply the old Shell interaction and shutdown must retain the new one"
        );
        Ok(())
    }
}
