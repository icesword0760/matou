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
        Arc, LazyLock, OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    SessionEvent, SessionOptions, claude_model_catalog,
    history_worker::{HistoryReply, HistoryRequest, HistoryWorker},
    owner::{OwnerEvent, ShellOwner},
    protocol::{
        ClaudeModelCatalogReply, ClaudeModelCatalogResult, ClaudeRoleAssignmentsReply,
        ProviderSettingsReply, ProviderSettingsResult,
    },
    provider_catalog,
    provider_hook::HookSetup,
    search::{SearchRequest, SearchResult},
    shell_input::ShellInput,
    worker::{SelectionCopy, ShellSubmission, WorkerCommand},
    workspace_paths::WorkspacePathChecks,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use matou_native_core::{
    backup::BackupReason,
    database_recovery::{RECOVERY_ID_CHANGED, RecoveryActions},
    hierarchy::{
        self, BootstrapWindowInput, HierarchyIds, HierarchyResult, Scene, Task, Workspace,
        canvas::{
            CanvasError, CanvasIds, CanvasMessages, CreateCanvasInput, CreateShellSiblingInput,
            ForkSessionInput,
        },
        cards::{CloseSessionCardInput, RemoveScope, RemoveSessionBranchInput},
        tabs::{CloseSceneInput, RenameSceneInput, ReorderSceneInput},
        tasks, workspaces,
    },
    journal::JournalOptions,
    provider_config::{ProviderConfigLocale, ProviderConfigStore},
    provider_launch::{
        CLAUDE_PERMISSION_MODES, ClaudeLaunch, ClaudeRoleAssignments, prepare_claude_fork,
        prepare_claude_launch, prepare_claude_permission_restart,
        prepare_new_claude_launch_with_budget, validate_claude_role_assignments, validate_cwd,
    },
    pty::{LaunchProfile, ShellLaunch},
    sessions::{
        ClaudeBinding, ClaudeCatalogLoad, ClaudeRestore, ClaudeRoleLaunchPreferences,
        read_claude_role_assignments, read_claude_role_launch_preferences,
        save_claude_role_assignments,
    },
    startup::{self, BootstrapResult, RecoveryError, RecoveryRequired},
    storage::{CommandMetadata, configure_connection},
    workspace_paths::WorkspacePathState,
};
use matou_native_terminal::{
    GridSelection, InputModes, Screen, SelectionAction, TerminalOptions, Theme,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub use crate::provider_catalog::{RecentReplies, RecentReply, deferred_summary_text};
pub use matou_native_core::hierarchy::canvas::{ForkPlacement, ForkWorktreeMode};

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
    DatabaseRecovery {
        #[serde(rename = "requestId")]
        request_id: String,
        action: DatabaseRecoveryAction,
    },
    History(HistoryRequest),
    GetClaudeModelCatalog {
        request_id: u64,
    },
    GetClaudeRoleAssignments {
        request_id: u64,
    },
    SetClaudeRoleAssignments {
        request_id: u64,
        assignments: Value,
    },
    GetProviderSettings {
        request_id: u64,
    },
    SaveProvider {
        request_id: u64,
        input: Value,
    },
    DeleteProvider {
        request_id: u64,
        cli: String,
        id: String,
    },
    SetActiveProvider {
        request_id: u64,
        cli: String,
        id: String,
    },
    ConfirmAntigravityRisk {
        request_id: u64,
    },
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
        /// The desktop already resolved the track position to a viewport row.
        #[serde(default)]
        pixel_target: bool,
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
    RestartSession {
        session_id: String,
    },
    RetryProviderRestore {
        session_id: String,
    },
    StartFreshProviderSession {
        session_id: String,
    },
    SetProviderPermissionMode {
        session_id: String,
        mode: String,
    },
    ListProviderSessions {
        cwd: String,
    },
    GetProviderSession {
        cwd: String,
        provider_session_id: String,
        start_event_index: Option<i64>,
        limit: Option<i64>,
    },
    GetDeferredRecoveryPreview {
        session_id: String,
    },
    LoadProviderSession {
        session_id: String,
        provider_session_id: String,
    },
    ForkSession {
        scene_id: String,
        source_session_id: String,
        placement: ForkPlacement,
        name: String,
        worktree_mode: ForkWorktreeMode,
        submission_key: String,
    },
    RetryFork {
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
    #[cfg(all(test, target_os = "macos"))]
    #[serde(skip)]
    TestPauseShellWorker {
        session_id: String,
        paused: Sender<()>,
        resume: Receiver<()>,
    },
    #[cfg(all(test, target_os = "macos"))]
    #[serde(skip)]
    TestFreezeShellWrites {
        session_id: String,
        frozen: bool,
        reply: Sender<Result<()>>,
    },
    #[cfg(all(test, target_os = "macos"))]
    #[serde(skip)]
    TestFailPromotionClear {
        session_id: String,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryState {
    Deferred,
    Restoring,
    Ready,
    Failed(String),
    StartFailed(String),
    Restarting(String),
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

pub const STORAGE_RECOVERY_WAITING: &str = "STORAGE_READ_ONLY: database is waiting for recovery";
pub const DATABASE_RECOVERY_INTERRUPTED: &str = "数据库恢复操作未完成：Runtime 在恢复操作期间退出";
pub const DATABASE_RECOVERY_NOT_PENDING: &str = "当前没有待处理的数据库恢复操作";
pub const DATABASE_RECOVERY_BUSY: &str = "正在处理数据库恢复，请保持 Matou 开启…";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseRecoveryReason {
    Ownership,
    Wal,
    Integrity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRecoveryBackup {
    pub id: String,
    pub created_at: i64,
    pub schema_version: i64,
    pub size: u64,
    pub reason: BackupReason,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRecoveryDetails {
    pub recovery_id: String,
    pub reason: DatabaseRecoveryReason,
    pub durable_database_path: PathBuf,
    pub quarantined_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ownership_issue: Option<String>,
    pub backups: Vec<DatabaseRecoveryBackup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RecoveryError>,
}

impl From<&RecoveryRequired> for DatabaseRecoveryDetails {
    fn from(recovery: &RecoveryRequired) -> Self {
        Self {
            recovery_id: recovery.recovery_id.clone(),
            reason: match recovery.reason.as_str() {
                "ownership-recovery-required" => DatabaseRecoveryReason::Ownership,
                "wal-recovery-required" => DatabaseRecoveryReason::Wal,
                _ => DatabaseRecoveryReason::Integrity,
            },
            durable_database_path: recovery.durable_database_path.clone(),
            quarantined_path: recovery.quarantined_path.clone(),
            ownership_issue: recovery.ownership_issue.clone(),
            backups: recovery
                .backups
                .iter()
                .map(|backup| DatabaseRecoveryBackup {
                    id: backup.id.clone(),
                    created_at: backup.created_at,
                    schema_version: backup.schema_version,
                    size: backup.size,
                    reason: backup.reason,
                    sha256: backup.sha256.clone(),
                })
                .collect(),
            error: {
                let errors = [
                    &recovery.backup_list_error,
                    &recovery.marker_error,
                    &recovery.move_error,
                ];
                let mut errors = errors.into_iter().flatten();
                errors.next().map(|first| {
                    let mut error = first.clone();
                    for next in errors {
                        error.message.push('；');
                        error.message.push_str(&next.message);
                    }
                    error
                })
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DatabaseRecoveryAction {
    RestoreBackup {
        backup_id: String,
        #[serde(default)]
        expected_recovery_id: String,
    },
    RetryOpen {
        #[serde(default)]
        expected_recovery_id: String,
    },
    StartEmptyDatabase {
        #[serde(default)]
        expected_recovery_id: String,
    },
    ExportRecoveryBundle,
}

impl DatabaseRecoveryAction {
    pub fn expected_recovery_id(&self) -> Option<&str> {
        match self {
            Self::RetryOpen {
                expected_recovery_id,
            }
            | Self::RestoreBackup {
                expected_recovery_id,
                ..
            }
            | Self::StartEmptyDatabase {
                expected_recovery_id,
            } => Some(expected_recovery_id),
            Self::ExportRecoveryBundle => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRecoveryResult {
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exported_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RecoveryError>,
}

impl DatabaseRecoveryResult {
    pub fn failed(request_id: String, code: &str, message: impl Into<String>) -> Self {
        Self {
            request_id,
            exported_path: None,
            error: Some(RecoveryError {
                code: code.into(),
                message: message.into(),
                retryable: true,
            }),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum Event {
    StartupBlocked(StartupBlock),
    DatabaseRecoveryRequired(DatabaseRecoveryDetails),
    DatabaseRecoveryResult(DatabaseRecoveryResult),
    StorageCommandRejected {
        request_id: Option<u64>,
        message: String,
    },
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
    DeferredRecoveryPreview {
        session_id: String,
        cwd: String,
        preview: Option<RecentReplies>,
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
    /// Graph with optional card, provider, catalog and new notification results.
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
        LazyLock::force(&PERMISSION_STARTUP_WINDOW);
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

struct LaunchRetry {
    workspace_id: String,
    validated: bool,
    recovering: bool,
}

enum RoleAssignmentsRequest {
    Get(u64),
    Set(u64, Value),
    ForLaunch(Sender<ClaudeRoleLaunchPreferences>),
}

enum ProviderSettingsRequest {
    Get(u64),
    Save(u64, Value),
    Delete(u64, String, String),
    SetActive(u64, String, String),
    ConfirmRisk(u64),
}

struct ProviderSettingsWorker {
    requests: Option<Sender<ProviderSettingsRequest>>,
    replies: Receiver<ProviderSettingsReply>,
    thread: Option<JoinHandle<()>>,
}

impl ProviderSettingsWorker {
    fn new(data_root: PathBuf) -> Result<Self> {
        let locale = match std::env::var("MATOU_LOCALE").as_deref() {
            Ok("en") => ProviderConfigLocale::En,
            _ => ProviderConfigLocale::ZhCn,
        };
        let (requests, incoming) = mpsc::channel();
        let (outgoing, replies) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("matou-provider-settings".into())
            .spawn(move || {
                // The host's owner lease excludes another native runtime for this root.
                // Legacy or external processes writing the file outside that lease can race.
                // Store construction does no I/O, so a bad file cannot block runtime startup.
                let store = ProviderConfigStore::new(data_root, locale);
                use ProviderSettingsResult::{Accepted, Error, Provider, Snapshot};
                for request in incoming {
                    let (request_id, result) = match request {
                        ProviderSettingsRequest::Get(id) => (id, store.snapshot().map(Snapshot)),
                        ProviderSettingsRequest::Save(id, input) => {
                            (id, store.upsert(&input).map(Provider))
                        }
                        ProviderSettingsRequest::Delete(id, cli, provider_id) => {
                            (id, store.delete(&cli, &provider_id).map(Snapshot))
                        }
                        ProviderSettingsRequest::SetActive(id, cli, provider_id) => {
                            (id, store.set_active(&cli, &provider_id).map(Snapshot))
                        }
                        ProviderSettingsRequest::ConfirmRisk(id) => (
                            id,
                            store.confirm_antigravity_risk().map(|()| Accepted(true)),
                        ),
                    };
                    let result = result.unwrap_or_else(|error| Error(error.to_string()));
                    if outgoing
                        .send(ProviderSettingsReply { request_id, result })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .context("start provider settings worker")?;
        Ok(Self {
            requests: Some(requests),
            replies,
            thread: Some(thread),
        })
    }

    fn send(&self, request: ProviderSettingsRequest) -> Result<()> {
        self.requests
            .as_ref()
            .context("provider settings worker stopped")?
            .send(request)
            .map_err(|_| anyhow!("provider settings worker stopped"))
    }

    fn shutdown(&mut self) -> Result<()> {
        // Close the queue and wait for accepted writes before releasing the owner lease.
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow!("provider settings worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for ProviderSettingsWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct PendingInput {
    workspace_id: String,
    session_id: String,
    bytes: Vec<u8>,
    defer_ordering: bool,
}

struct PendingShellSubmission {
    defer_ordering: bool,
    record_on_match: bool,
    decision: Receiver<Option<ShellSubmission>>,
    following: VecDeque<Command>,
}

const WORKSPACE_PATH_INVALID: &str = "工作区目录不可用，请先在本地恢复原路径，或移出该工作区";
const ACTIVE_PATH_INTERVAL: Duration = Duration::from_millis(400);
const WORKSPACE_POLL_INTERVAL: Duration = Duration::from_secs(30);
type WorkerOperation = (String, Receiver<Result<()>>, bool);

pub fn database_recovery_request_id() -> String {
    id()
}

fn recovery_result(
    send: &async_channel::Sender<Event>,
    result: DatabaseRecoveryResult,
) -> Result<()> {
    send.send_blocking(Event::DatabaseRecoveryResult(result))?;
    Ok(())
}

fn reject_storage_command(command: Command, send: &async_channel::Sender<Event>) -> Result<()> {
    if let Command::DatabaseRecovery { request_id, .. } = command {
        return recovery_result(
            send,
            DatabaseRecoveryResult::failed(
                request_id,
                "RECOVERY_NOT_PENDING",
                DATABASE_RECOVERY_NOT_PENDING,
            ),
        );
    }
    let request_id = match command {
        Command::GetClaudeModelCatalog { request_id }
        | Command::GetClaudeRoleAssignments { request_id }
        | Command::SetClaudeRoleAssignments { request_id, .. }
        | Command::GetProviderSettings { request_id }
        | Command::SaveProvider { request_id, .. }
        | Command::DeleteProvider { request_id, .. }
        | Command::SetActiveProvider { request_id, .. }
        | Command::ConfirmAntigravityRisk { request_id }
        | Command::CopySelection { request_id, .. }
        | Command::ScrollTo { request_id, .. } => Some(request_id),
        Command::History(
            HistoryRequest::Search { sequence, .. } | HistoryRequest::Context { sequence, .. },
        )
        | Command::PutGeometry { sequence, .. }
        | Command::Applied { sequence, .. } => Some(sequence),
        _ => None,
    };
    send.send_blocking(Event::StorageCommandRejected {
        request_id,
        message: STORAGE_RECOVERY_WAITING.into(),
    })?;
    Ok(())
}

type WritableDatabase = (
    Connection,
    matou_native_core::ownership::DatabaseOwner,
    PathBuf,
);

fn await_database_recovery(
    recovery: RecoveryRequired,
    generation: &str,
    incoming: &Receiver<Command>,
    send: &async_channel::Sender<Event>,
) -> Result<Option<(WritableDatabase, String)>> {
    let details = DatabaseRecoveryDetails::from(&recovery);
    send.send_blocking(Event::DatabaseRecoveryRequired(details.clone()))?;
    thread::scope(|scope| {
        type Attempt = (
            RecoveryActions,
            std::result::Result<
                WritableDatabase,
                matou_native_core::database_recovery::RecoveryActionError,
            >,
        );
        let mut active: Option<(String, thread::ScopedJoinHandle<'_, Attempt>)> = None;
        let mut actions = Some(RecoveryActions::default());
        let mut closing = false;
        loop {
            if active
                .as_ref()
                .is_some_and(|(_, worker)| worker.is_finished())
                || closing
            {
                let Some((request_id, worker)) = active.take() else {
                    return Ok(None);
                };
                let (state, result) = worker
                    .join()
                    .map_err(|_| anyhow!("database recovery worker panicked"))?;
                if closing {
                    return Ok(None);
                }
                actions = Some(state);
                match result {
                    Ok(database) => return Ok(Some((database, request_id))),
                    Err(error) => {
                        send.send_blocking(Event::DatabaseRecoveryRequired(details.clone()))?;
                        recovery_result(
                            send,
                            DatabaseRecoveryResult::failed(
                                request_id,
                                error.stage,
                                format!("{:#}", error.cause),
                            ),
                        )?;
                    }
                }
            }
            let command = if active.is_some() {
                incoming.recv_timeout(Duration::from_millis(10))
            } else {
                incoming
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            };
            match command {
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) | Ok(Command::Shutdown) => {
                    closing = true;
                }
                Ok(Command::DatabaseRecovery { request_id, action }) => {
                    let reject = if active.is_some() {
                        Some(("RECOVERY_BUSY", DATABASE_RECOVERY_BUSY))
                    } else if action
                        .expected_recovery_id()
                        .is_some_and(|id| id != recovery.recovery_id)
                    {
                        Some(("RECOVERY_ID_CHANGED", RECOVERY_ID_CHANGED))
                    } else if !matches!(action, DatabaseRecoveryAction::RetryOpen { .. }) {
                        Some(("RECOVERY_NOT_IMPLEMENTED", "此数据库恢复操作尚未实现"))
                    } else {
                        None
                    };
                    if let Some((code, message)) = reject {
                        recovery_result(
                            send,
                            DatabaseRecoveryResult::failed(request_id, code, message),
                        )?;
                        continue;
                    }
                    let Ok(archive_id) = Uuid::parse_str(&request_id) else {
                        recovery_result(
                            send,
                            DatabaseRecoveryResult::failed(
                                request_id,
                                "RECOVERY_REQUEST_ID_INVALID",
                                "无效的数据库恢复请求编号",
                            ),
                        )?;
                        continue;
                    };
                    let DatabaseRecoveryAction::RetryOpen {
                        expected_recovery_id,
                    } = action
                    else {
                        unreachable!()
                    };
                    let mut state = actions.take().expect("idle recovery state");
                    let recovery = &recovery;
                    let worker = thread::Builder::new()
                        .name("matou-database-recovery".into())
                        .spawn_scoped(scope, move || {
                            let result = state.retry_open(
                                recovery,
                                generation,
                                archive_id,
                                &expected_recovery_id,
                            );
                            (state, result)
                        })
                        .context("start database recovery worker")?;
                    active = Some((request_id, worker));
                }
                Ok(command) => reject_storage_command(command, send)?,
            }
        }
    })
}

fn run(
    options: LaunchOptions,
    incoming: Receiver<Command>,
    send: &async_channel::Sender<Event>,
    shutdown_requested: &OnceLock<bool>,
) -> Result<()> {
    let generation = id();
    let bootstrap = match startup::open(&options.data_root, &generation) {
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
    let (database, mut recovery_request) = match bootstrap {
        BootstrapResult::Writable {
            connection,
            ownership,
            data_root,
        } => ((connection, ownership, data_root), None),
        BootstrapResult::ReadOnly { reason, .. } => {
            let message = format!("Native startup requires the recovery interface: {reason:?}");
            let _ = send.send_blocking(Event::StartupBlocked(StartupBlock {
                kind: StartupBlockKind::ReadOnly,
                message: message.clone(),
            }));
            bail!(message)
        }
        BootstrapResult::RecoveryRequired(recovery) => {
            let Some((database, request_id)) =
                await_database_recovery(recovery, &generation, &incoming, send)?
            else {
                return Ok(());
            };
            (database, Some(request_id))
        }
    };
    let result = run_writable(
        options,
        incoming,
        send,
        shutdown_requested,
        database,
        &mut recovery_request,
    );
    if let (Some(request_id), Err(error)) = (recovery_request, &result) {
        recovery_result(
            send,
            DatabaseRecoveryResult::failed(
                request_id,
                "runtime-initialization",
                format!("{error:#}"),
            ),
        )?;
    }
    result
}

fn run_writable(
    options: LaunchOptions,
    incoming: Receiver<Command>,
    send: &async_channel::Sender<Event>,
    shutdown_requested: &OnceLock<bool>,
    (mut connection, ownership, data_root): WritableDatabase,
    recovery_request: &mut Option<String>,
) -> Result<()> {
    for path in [&options.workspace, &options.shell_home] {
        if let Err(error) = std::fs::create_dir_all(path) {
            send.send_blocking(Event::StartupBlocked(StartupBlock {
                kind: StartupBlockKind::Configuration,
                message: error.to_string(),
            }))?;
            return Err(error.into());
        }
    }
    let history = HistoryWorker::new(data_root.clone())?;
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
    // Declared after the lease-owning host state so early returns also join the worker first.
    let mut provider_settings = ProviderSettingsWorker::new(data_root.clone())?;
    let mut sessions = HashSet::new();
    let mut active_workspace = hierarchy
        .workspace
        .as_ref()
        .map(|workspace| workspace.id.clone());
    let mut checks = WorkspacePathChecks::default();
    let mut startup_order = HashMap::new();
    let mut startup_restores = Vec::new();
    // Startup parking is independent from the existing path-repair rebuild queue.
    let mut startup_deferred = HashSet::new();
    for (session_id, workspace_id, automatic) in
        owner.startup_recovery_candidates(scene_id.as_deref())?
    {
        if automatic && startup_order.len() < 16 {
            startup_order.insert(session_id.clone(), startup_order.len());
            startup_restores.push((session_id, workspace_id));
        } else {
            startup_deferred.insert(session_id);
        }
    }
    let graph = active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
    let mut focused_session = graph["focusedSessionId"].as_str().map(str::to_owned);
    let mut pending_launch = unstarted_sessions(&owner, &graph, &sessions)?;
    pending_launch.retain(|id| !startup_deferred.contains(id));
    let mut foreground_sessions: HashSet<_> = launchable_ids(&graph).map(str::to_owned).collect();
    let mut deferred_recovery = HashSet::new();
    let mut launch_preparations = HashMap::<String, Receiver<Result<PreparedLaunch>>>::new();
    let mut recovering = HashMap::<String, String>::new();
    // A failed launch stays blocked until an explicit retry starts a worker.
    let mut failed_launches = HashMap::new();
    let mut launch_retries = HashMap::<String, LaunchRetry>::new();
    let mut launch_validated = false;
    let mut pending_input = VecDeque::<PendingInput>::new();
    let mut shell_promotions = HashMap::<String, ShellPromotion>::new();
    let mut retry_pending_input = false;
    let mut terminal_layouts = HashMap::new();
    let mut next_active_check = Instant::now() + ACTIVE_PATH_INTERVAL;
    let mut next_workspace_poll = Instant::now() + WORKSPACE_POLL_INTERVAL;
    if let Some(workspace_id) = &active_workspace {
        checks.request(&owner, workspace_id)?;
    }
    for (session_id, workspace_id) in startup_restores {
        if !pending_launch.contains(&session_id) {
            checks.request(&owner, &workspace_id)?;
            launch_retries.insert(
                session_id,
                LaunchRetry {
                    workspace_id,
                    recovering: true,
                    validated: false,
                },
            );
        }
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
    let ready_event = Event::Ready {
        hierarchy,
        scenes,
        graph,
        geometry: scene_id
            .as_deref()
            .map(|id| owner.geometry(id))
            .transpose()?
            .unwrap_or_default(),
    };
    let mut dirty = HashSet::<String>::new();
    let mut sequence = HashMap::<String, u64>::new();
    let mut views = HashMap::<String, PendingView>::new();
    let mut scroll_requests = HashMap::<String, u64>::new();
    let mut awaiting_ui = HashMap::<String, u64>::new();
    let mut operations = Vec::<WorkerOperation>::new();
    let mut shell_submissions = HashMap::<String, PendingShellSubmission>::new();
    let mut replay_shell_inputs = VecDeque::<Command>::new();
    let mut searches = Vec::<(String, u64, Receiver<Result<SearchResult>>)>::new();
    let mut copies = Vec::<(u64, String, bool, Receiver<Result<SelectionCopy>>)>::new();
    let mut navigation_publications = VecDeque::new();
    let mut graph_dirty = false;
    let mut provider_identity_events = Vec::new();
    let mut agent_notifications = Vec::new();
    let mut provider_starting = HashMap::new();
    let mut claude_launch_targets = HashMap::new();
    let mut permission_changes = HashMap::<String, PermissionChange>::new();
    let mut permission_results = Vec::new();
    let (catalog_requests, catalog_replies) = start_provider_catalog()?;
    let (model_catalog_requests, model_catalog_commands) = mpsc::channel();
    let (model_catalog_results, model_catalog_replies) = mpsc::channel();
    let worker_results = model_catalog_results.clone();
    let model_catalog_worker = thread::Builder::new()
        .name("claude-model-catalog".into())
        .spawn(move || {
            for request_id in model_catalog_commands {
                let result = ClaudeModelCatalogResult::Models(claude_model_catalog::models());
                if worker_results
                    .send(ClaudeModelCatalogReply { request_id, result })
                    .is_err()
                {
                    break;
                }
            }
        });
    let (role_requests, role_commands) = mpsc::channel();
    let (role_results, role_replies) = mpsc::channel();
    let role_data_root = data_root.clone();
    let role_worker = thread::Builder::new()
        .name("claude-role-assignments".into())
        .spawn(move || {
            for request in role_commands {
                let (request_id, result) = match request {
                    RoleAssignmentsRequest::Get(id) => {
                        (id, Ok(load_role_assignments(&role_data_root)))
                    }
                    RoleAssignmentsRequest::Set(id, value) => {
                        (id, save_role_assignments(&role_data_root, &value))
                    }
                    RoleAssignmentsRequest::ForLaunch(reply) => {
                        let preferences =
                            startup::open_read_only(&role_data_root.join("matou.sqlite"))
                                .map(|db| read_claude_role_launch_preferences(&db))
                                .unwrap_or_default();
                        let _ = reply.send(preferences);
                        continue;
                    }
                };
                let reply = match result {
                    Ok(assignments) => ClaudeRoleAssignmentsReply {
                        request_id,
                        assignments: Some(assignments),
                        error: None,
                    },
                    Err(error) => ClaudeRoleAssignmentsReply {
                        request_id,
                        assignments: None,
                        error: Some(format!("{error:#}")),
                    },
                };
                if role_results.send(reply).is_err() {
                    break;
                }
            }
        })?;
    let mut catalog_loads = HashMap::<String, CatalogLoad>::new();
    let mut catalog_lists = Vec::new();
    let mut catalog_details = Vec::new();
    let mut catalog_load_errors = Vec::new();
    let mut tasks_dirty = false;
    if recovery_request.is_some() {
        // Reject commands already on the wire during the recovered bootstrap.
        // Neither a second action nor ordinary writes join the new workspace.
        for command in incoming.try_iter() {
            match command {
                Command::Shutdown => return Ok(()),
                Command::DatabaseRecovery { request_id, .. } => recovery_result(
                    send,
                    DatabaseRecoveryResult::failed(
                        request_id,
                        "RECOVERY_BUSY",
                        DATABASE_RECOVERY_BUSY,
                    ),
                )?,
                command => reject_storage_command(command, send)?,
            }
        }
    }
    send.send_blocking(ready_event)?;
    if let Some(request_id) = recovery_request.take() {
        recovery_result(
            send,
            DatabaseRecoveryResult {
                request_id,
                exported_path: None,
                error: None,
            },
        )?;
    }
    for session_id in &startup_deferred {
        send.send_blocking(Event::Recovery {
            session_id: session_id.clone(),
            state: RecoveryState::Deferred,
        })?;
    }
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
            let retries: Vec<_> = launch_retries
                .iter_mut()
                .filter(|(_, retry)| retry.workspace_id == state.workspace_id)
                .filter_map(|(session_id, retry)| {
                    retry.validated = !invalid;
                    invalid.then(|| session_id.clone())
                })
                .collect();
            for session_id in retries {
                let retry = launch_retries.remove(&session_id).expect("checked retry");
                claude_launch_targets.remove(&session_id);
                failed_launches.insert(
                    session_id.clone(),
                    report_session_launch_failure(
                        &mut owner,
                        send,
                        &session_id,
                        retry.recovering,
                        workspace_path_error(&state),
                    )?,
                );
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
                    claude_launch_targets.remove(&id);
                    let rebuilding = owner.has_shell_run(&id)?;
                    let message = match owner.session(&id)? {
                        Some(session) if session.kind == "claude-code" => {
                            validate_cwd(Path::new(&session.cwd)).err().map_or_else(
                                || workspace_path_error(&state),
                                |error| format!("{error:#}"),
                            )
                        }
                        _ => workspace_path_error(&state),
                    };
                    let failure =
                        report_session_launch_failure(&mut owner, send, &id, rebuilding, message)?;
                    failed_launches.insert(id, failure);
                }
            } else {
                if active_path_invalid && let Some(task_id) = &active_task {
                    for scene in owner.scenes(task_id)? {
                        let graph = owner.graph(&scene.id, None)?;
                        deferred_recovery.extend(
                            shell_ids(&graph)
                                .filter(|id| {
                                    sessions.contains(*id) && !shell_promotions.contains_key(*id)
                                })
                                .map(str::to_owned),
                        );
                    }
                }
                // The reference increments the background pane's revision, then
                // replaces its worker only when the pane attaches in the foreground.
                let running: Vec<_> = foreground_sessions
                    .iter()
                    .filter(|id| deferred_recovery.remove(*id))
                    .cloned()
                    .collect();
                owner.retire_sessions(&running);
                operations.retain(|(id, _, _)| !running.contains(id));
                for id in running {
                    shell_submissions.remove(&id);
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
            launch_retries
                .iter()
                .filter(|(id, retry)| retry.validated && !owner.shell_worker_is_retiring(id))
                .map(|(id, _)| id.clone()),
        );
        launch_preparations.retain(|id, _| launch_candidates.contains(id));
        let mut launch_candidates: Vec<_> = launch_candidates.into_iter().collect();
        launch_candidates.sort_unstable_by(|left, right| {
            foreground_sessions
                .contains(right)
                .cmp(&foreground_sessions.contains(left))
                .then_with(|| {
                    startup_order
                        .get(left)
                        .unwrap_or(&usize::MAX)
                        .cmp(startup_order.get(right).unwrap_or(&usize::MAX))
                })
                .then_with(|| left.cmp(right))
        });
        // A fork can wait for the user's first prompt. Only actual restores
        // consume a recovery slot during identity confirmation.
        let restoring = provider_starting
            .values()
            .filter(|&&restoring| restoring)
            .count();
        let mut available_slots = 4usize.saturating_sub(restoring);
        let mut eligible = HashSet::new();
        let mut completed_launches = Vec::new();
        for session_id in launch_candidates {
            if owner
                .claude_binding(&session_id)?
                .is_some_and(|binding| binding.resumable)
            {
                if available_slots == 0 {
                    continue;
                }
                available_slots -= 1;
            }
            eligible.insert(session_id.clone());
            if !launch_preparations.contains_key(&session_id) {
                match prepare_launch(
                    &owner,
                    &options,
                    &session_id,
                    LaunchIntent::Start,
                    &role_requests,
                ) {
                    Ok(result) => {
                        launch_preparations.insert(session_id, result);
                    }
                    Err(error) => completed_launches.push((session_id, Err(error))),
                }
            }
        }
        launch_preparations.retain(|id, _| eligible.contains(id));
        launch_preparations.retain(|id, result| match result.try_recv() {
            Ok(launch) => {
                completed_launches.push((id.clone(), launch));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                completed_launches.push((
                    id.clone(),
                    Err(anyhow!("session launch preparation disconnected")),
                ));
                false
            }
        });
        let mut started_shell = false;
        for (session_id, launch) in completed_launches {
            pending_launch.remove(&session_id);
            startup_deferred.remove(&session_id);
            startup_order.remove(&session_id);
            let rebuilding = recovering.remove(&session_id).is_some();
            let rebuilding = match launch_retries.remove(&session_id) {
                Some(retry) => retry.recovering,
                None => rebuilding || owner.has_shell_run(&session_id)?,
            };
            let binding = owner.claude_binding(&session_id)?;
            let resume = binding.as_ref().filter(|binding| binding.resumable);
            let mut waits_for_identity = resume.is_some();
            let result = launch.and_then(|launch| match launch {
                PreparedLaunch::Shell(directory) => {
                    owner.set_shell_launch_directory(&session_id, &directory)?;
                    launch_shell(
                        &mut owner,
                        &options,
                        &session_id,
                        directory,
                        &terminal_layouts,
                    )
                }
                PreparedLaunch::ClaudeCode {
                    launch,
                    fork_source,
                } => {
                    claude_launch_targets
                        .insert(session_id.clone(), claude_target_metadata(&launch));
                    let terminal = terminal_layouts
                        .get(&session_id)
                        .copied()
                        .unwrap_or_else(default_terminal_options);
                    if let Some(source_id) = fork_source {
                        waits_for_identity = true;
                        return owner.launch_claude_fork(
                            session_id.clone(),
                            terminal,
                            launch,
                            source_id,
                        );
                    }
                    if let Some(binding) = resume {
                        owner.change_claude_restore(
                            &session_id,
                            ClaudeRestore::Begin(&binding.id),
                        )?;
                    }
                    owner.launch_claude(session_id.clone(), terminal, launch, binding.as_ref())
                }
            });
            match result {
                Ok(()) => {
                    failed_launches.remove(&session_id);
                    if waits_for_identity {
                        provider_starting.insert(session_id.clone(), resume.is_some());
                    } else {
                        send.send_blocking(Event::Recovery {
                            session_id: session_id.clone(),
                            state: RecoveryState::Ready,
                        })?;
                    }
                    dirty.insert(session_id.clone());
                    sessions.insert(session_id);
                    graph_dirty = true;
                    started_shell = true;
                }
                Err(error) => {
                    claude_launch_targets.remove(&session_id);
                    pending_input.retain(|input| input.session_id != session_id);
                    let recovering = rebuilding && error.downcast_ref::<HookSetup>().is_none();
                    let failure = report_session_launch_failure(
                        &mut owner,
                        send,
                        &session_id,
                        recovering,
                        format!("{error:#}"),
                    )?;
                    failed_launches.insert(session_id, failure);
                }
            }
        }
        // Only the submitted Shell's later input waits for its worker.
        let mut resolved_submissions = Vec::new();
        for (session_id, pending) in &shell_submissions {
            match pending.decision.try_recv() {
                Ok(decision) => {
                    if let Some(ShellSubmission {
                        mode,
                        before,
                        cleared,
                    }) = decision
                    {
                        shell_promotions.insert(
                            session_id.clone(),
                            ShellPromotion {
                                mode,
                                phase: ShellPromotionPhase::Clearing {
                                    result: cleared,
                                    before,
                                },
                                promoted: false,
                                inputs: VecDeque::new(),
                            },
                        );
                        if pending.record_on_match
                            && record_terminal_interaction(
                                &mut owner,
                                session_id,
                                "submit",
                                pending.defer_ordering,
                                send,
                            )?
                        {
                            tasks_dirty = true;
                            graph_dirty = true;
                        }
                    }
                    resolved_submissions.push(session_id.clone());
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    resolved_submissions.push(session_id.clone());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        retry_pending_input |= path_checked || started_shell || !resolved_submissions.is_empty();
        for id in resolved_submissions {
            if let Some(mut pending) = shell_submissions.remove(&id) {
                replay_shell_inputs.append(&mut pending.following);
            }
        }
        if !copies.iter().any(|(_, _, interrupt, _)| *interrupt)
            && std::mem::take(&mut retry_pending_input)
        {
            for _ in 0..pending_input.len() {
                let input = pending_input.pop_front().expect("queued terminal input");
                if !shell_submissions.contains_key(&input.session_id)
                    && (sessions.contains(&input.session_id)
                        || shell_promotions.contains_key(&input.session_id))
                    && !(foreground_sessions.contains(&input.session_id)
                        && deferred_recovery.contains(&input.session_id))
                    && owner
                        .workspace_path_state(&input.workspace_id)?
                        .is_some_and(|state| state.status == "valid")
                {
                    if send_input(
                        &mut owner,
                        input,
                        &mut shell_promotions,
                        &mut shell_submissions,
                        &mut operations,
                        send,
                    )? {
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
            let command = match replay_shell_inputs
                .pop_front()
                .map_or_else(|| incoming.try_recv(), Ok)
            {
                Ok(command) => command,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'running false,
            };
            if let Command::Input { session_id, .. } | Command::CopySelection { session_id, .. } =
                &command
                && let Some(pending) = shell_submissions.get_mut(session_id)
            {
                pending.following.push_back(command);
                continue;
            }
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
                    | Command::ForkSession { .. }
                    | Command::RetryFork { .. }
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
                let (fork_submission, fork_retry) = match &command {
                    Command::ForkSession { submission_key, .. } => {
                        (Some(submission_key.clone()), None)
                    }
                    Command::RetryFork { session_id } => (None, Some(session_id.clone())),
                    _ => (None, None),
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
                        | Command::RetryFork { .. }
                        | Command::SelectScene { .. }
                );
                match navigate(&mut owner, window_id, command) {
                    Ok((hierarchy, disposed, action)) => {
                        let (card_action, fork_result) = match action {
                            Some(NavigationAction::Card(action)) => (Some(action), None),
                            Some(NavigationAction::Fork(mut result)) => {
                                if let Some(submission_key) = &fork_submission {
                                    result["submissionKey"] = json!(submission_key);
                                }
                                if result["replayed"] == true {
                                    let mut graph = active_graph(
                                        &owner,
                                        scene_id.as_deref(),
                                        window_id,
                                        &startup_deferred,
                                    )?;
                                    graph["forkSessionResult"] = result;
                                    send.send_blocking(Event::Graph(graph))?;
                                    continue;
                                }
                                (None, Some(result))
                            }
                            None => (None, None),
                        };
                        if let Some(action) = &card_action
                            && action["outcome"] == "hide-window"
                        {
                            let mut graph = active_graph(
                                &owner,
                                scene_id.as_deref(),
                                window_id,
                                &startup_deferred,
                            )?;
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
                                shell_submissions.remove(id);
                                shell_promotions.remove(id);
                                if let Some(load) = catalog_loads.remove(id) {
                                    catalog_load_errors.push(catalog_load_error(
                                        id,
                                        &load.provider_session_id,
                                        &anyhow!("Card was closed while loading the conversation"),
                                    ));
                                }
                                if let Some(change) = permission_changes.remove(id) {
                                    permission_results.push(permission_result(
                                        id,
                                        &change.mode,
                                        Some("Card was closed during permission respawn".into()),
                                    ));
                                    if let Err(error) = change.rollback_if_pending(&mut owner, id) {
                                        send.send_blocking(Event::Diagnostic(format!(
                                            "{error:#}"
                                        )))?;
                                    }
                                }
                                sessions.remove(id);
                                dirty.remove(id);
                                sequence.remove(id);
                                scroll_requests.remove(id);
                                views.remove(id);
                                awaiting_ui.remove(id);
                                terminal_layouts.remove(id);
                                recovering.remove(id);
                                deferred_recovery.remove(id);
                                startup_deferred.remove(id);
                                failed_launches.remove(id);
                                launch_retries.remove(id);
                                launch_preparations.remove(id);
                                claude_launch_targets.remove(id);
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
                        let mut graph = active_graph(
                            &owner,
                            scene_id.as_deref(),
                            window_id,
                            &startup_deferred,
                        )?;
                        let next_focus = graph["focusedSessionId"].as_str();
                        if !target_changed
                            && focused_session.as_deref() != next_focus
                            && let Some(pending) = focused_session
                                .as_ref()
                                .and_then(|id| shell_submissions.get_mut(id))
                        {
                            // The focus switch already flushed this card's deferred ordering.
                            pending.defer_ordering = false;
                        }
                        focused_session = next_focus.map(str::to_owned);
                        pending_launch = unstarted_sessions(&owner, &graph, &sessions)?;
                        pending_launch.retain(|id| {
                            !startup_deferred.contains(id)
                                && !failed_launches.contains_key(id)
                                && !launch_retries.contains_key(id)
                                && !catalog_loads.contains_key(id)
                                && !permission_changes.contains_key(id)
                                && !shell_promotions.contains_key(id)
                        });
                        foreground_sessions = launchable_ids(&graph).map(str::to_owned).collect();
                        if (target_changed || check_target || !pending_launch.is_empty())
                            && let Some(workspace_id) = &active_workspace
                        {
                            checks.request(&owner, workspace_id)?;
                            launch_validated = false;
                            next_active_check = Instant::now() + ACTIVE_PATH_INTERVAL;
                        }
                        if let Some(result) = fork_result {
                            let child = result["sessionId"]
                                .as_str()
                                .context("Fork result has no session")?;
                            let workspace_id = active_workspace
                                .clone()
                                .context("Fork result has no workspace")?;
                            pending_launch.remove(child);
                            failed_launches.remove(child);
                            launch_retries.insert(
                                child.to_owned(),
                                LaunchRetry {
                                    workspace_id,
                                    recovering: false,
                                    validated: false,
                                },
                            );
                            graph["forkSessionResult"] = result;
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
                        if fork_submission.is_some() || fork_retry.is_some() {
                            let mut graph = active_graph(
                                &owner,
                                scene_id.as_deref(),
                                window_id,
                                &startup_deferred,
                            )?;
                            let mut result = json!({
                                "code": error.downcast_ref::<CanvasError>().map_or("FORK_CREATE_FAILED", |error| error.code),
                                "error": format!("{error:#}"),
                            });
                            if let Some(submission_key) = fork_submission {
                                result["submissionKey"] = json!(submission_key);
                            }
                            if let Some(session_id) = fork_retry {
                                result["sessionId"] = json!(session_id);
                            }
                            graph["forkSessionResult"] = result;
                            send.send_blocking(Event::Graph(graph))?;
                        } else if let Some((action, workspace_id)) = workspace_action {
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
                Command::DatabaseRecovery { .. } => {
                    reject_storage_command(command, send)?;
                    continue;
                }
                Command::GetClaudeModelCatalog { request_id } => {
                    if model_catalog_worker.is_err()
                        || model_catalog_requests.send(request_id).is_err()
                    {
                        model_catalog_results.send(ClaudeModelCatalogReply {
                            request_id,
                            result: ClaudeModelCatalogResult::Error(
                                "Claude model catalog worker is unavailable".into(),
                            ),
                        })?;
                    }
                    continue;
                }
                Command::GetClaudeRoleAssignments { request_id } => {
                    role_requests.send(RoleAssignmentsRequest::Get(request_id))?;
                    continue;
                }
                Command::SetClaudeRoleAssignments {
                    request_id,
                    assignments,
                } => {
                    role_requests.send(RoleAssignmentsRequest::Set(request_id, assignments))?;
                    continue;
                }
                Command::GetProviderSettings { request_id } => {
                    provider_settings.send(ProviderSettingsRequest::Get(request_id))?;
                    continue;
                }
                Command::SaveProvider { request_id, input } => {
                    provider_settings.send(ProviderSettingsRequest::Save(request_id, input))?;
                    continue;
                }
                Command::DeleteProvider {
                    request_id,
                    cli,
                    id,
                } => {
                    provider_settings.send(ProviderSettingsRequest::Delete(request_id, cli, id))?;
                    continue;
                }
                Command::SetActiveProvider {
                    request_id,
                    cli,
                    id,
                } => {
                    provider_settings
                        .send(ProviderSettingsRequest::SetActive(request_id, cli, id))?;
                    continue;
                }
                Command::ConfirmAntigravityRisk { request_id } => {
                    provider_settings.send(ProviderSettingsRequest::ConfirmRisk(request_id))?;
                    continue;
                }
                Command::Input {
                    session_id,
                    bytes,
                    defer_ordering,
                } => {
                    if permission_changes.contains_key(&session_id) {
                        if let Err(error) = owner.send_permission_consent_input(&session_id, bytes)
                        {
                            send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                        }
                        continue;
                    }
                    if failed_launches.contains_key(&session_id)
                        || launch_retries.contains_key(&session_id)
                        || catalog_loads
                            .get(&session_id)
                            .is_some_and(|load| load.stopping.is_some())
                        || (!sessions.contains(&session_id)
                            && !shell_promotions.contains_key(&session_id)
                            && !pending_launch.contains(&session_id)
                            && owner
                                .claude_binding(&session_id)?
                                .is_some_and(|binding| binding.restore_state == "failed"))
                    {
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
                    if !shell_promotions.contains_key(&input.session_id)
                        && (state.is_none()
                            || !sessions.contains(&input.session_id)
                            || (foreground_sessions.contains(&input.session_id)
                                && deferred_recovery.contains(&input.session_id)))
                    {
                        checks.request(&owner, &input.workspace_id)?;
                        pending_input.push_back(input);
                    } else if send_input(
                        &mut owner,
                        input,
                        &mut shell_promotions,
                        &mut shell_submissions,
                        &mut operations,
                        send,
                    )? {
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
                        // Starting/Verifying use the current PTY for copy and empty-selection interrupts.
                        // Suppress interrupts only in Clearing/Stopping/Preparing/RollingBack/RestoringShell.
                        let interrupt_if_empty = interrupt_if_empty
                            && !permission_changes.contains_key(&session_id)
                            && shell_promotions
                                .get(&session_id)
                                .is_none_or(ShellPromotion::attached);
                        owner.send(
                            &session_id,
                            WorkerCommand::CopySelection {
                                reply,
                                interrupt_if_empty,
                            },
                        )?;
                        copies.push((request_id, session_id, interrupt_if_empty, receive));
                    } else if shell_promotions.contains_key(&session_id) {
                        send.send_blocking(Event::Copied {
                            request_id,
                            text: String::new(),
                        })?;
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
                    pixel_target,
                } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    scroll_requests.insert(session_id.clone(), request_id);
                    (
                        session_id,
                        WorkerCommand::ScrollTo {
                            row,
                            pixel_target,
                            reply,
                        },
                        false,
                    )
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
                Command::SetProviderPermissionMode { session_id, mode } => {
                    if !CLAUDE_PERMISSION_MODES.contains(&mode.as_str()) {
                        permission_results.push(permission_result(
                            &session_id,
                            &mode,
                            Some("invalid permission mode".into()),
                        ));
                        continue;
                    }
                    if let Some(change) = permission_changes.get(&session_id) {
                        if change.mode != mode {
                            permission_results.push(permission_result(
                                &session_id,
                                &mode,
                                Some("AI Session is already stopping or recovering".into()),
                            ));
                        }
                        continue;
                    }
                    let attempt = (|| -> Result<Option<PermissionChange>> {
                        ensure!(
                            !pending_launch.contains(&session_id)
                                && !recovering.contains_key(&session_id)
                                && !launch_retries.contains_key(&session_id)
                                && !shell_promotions.contains_key(&session_id)
                                && !catalog_loads.contains_key(&session_id),
                            "AI Session is already stopping or recovering"
                        );
                        let binding = owner.permission_binding(&session_id)?;
                        if binding.as_ref().is_some_and(|binding| {
                            binding.metadata["permissionMode"]
                                .as_str()
                                .filter(|mode| CLAUDE_PERMISSION_MODES.contains(mode))
                                .unwrap_or("default")
                                == mode
                        }) {
                            return Ok(None);
                        }
                        let stopped = owner.stop_for_permission_change(&session_id)?;
                        Ok(Some(PermissionChange {
                            mode: mode.clone(),
                            binding,
                            phase: PermissionPhase::Stopping(stopped),
                        }))
                    })();
                    match attempt {
                        Ok(Some(change)) => {
                            if change.binding.is_none() {
                                failed_launches.remove(&session_id);
                            }
                            sessions.remove(&session_id);
                            dirty.remove(&session_id);
                            sequence.remove(&session_id);
                            scroll_requests.remove(&session_id);
                            views.remove(&session_id);
                            awaiting_ui.remove(&session_id);
                            pending_input.retain(|input| input.session_id != session_id);
                            operations.retain(|(id, _, _)| id != &session_id);
                            searches.retain(|(id, _, _)| id != &session_id);
                            copies.retain(|(_, id, _, _)| id != &session_id);
                            permission_changes.insert(session_id.clone(), change);
                            send.send_blocking(Event::Recovery {
                                session_id,
                                state: RecoveryState::Restoring,
                            })?;
                        }
                        Ok(None) => {
                            permission_results.push(permission_result(&session_id, &mode, None))
                        }
                        Err(error) => permission_results.push(permission_result(
                            &session_id,
                            &mode,
                            Some(format!("{error:#}")),
                        )),
                    }
                    continue;
                }
                Command::ListProviderSessions { cwd } => {
                    if let Err(
                        mpsc::TrySendError::Full(CatalogRequest::Index { cwd, .. })
                        | mpsc::TrySendError::Disconnected(CatalogRequest::Index { cwd, .. }),
                    ) = catalog_requests.try_send(CatalogRequest::Index { cwd, load: None })
                    {
                        catalog_lists.push(json!({"cwd": cwd, "error": CATALOG_BUSY}));
                    }
                    continue;
                }
                Command::GetProviderSession {
                    cwd,
                    provider_session_id,
                    start_event_index,
                    limit,
                } => {
                    if let Err(
                        mpsc::TrySendError::Full(CatalogRequest::Detail(request))
                        | mpsc::TrySendError::Disconnected(CatalogRequest::Detail(request)),
                    ) = catalog_requests.try_send(CatalogRequest::Detail(CatalogDetailRequest {
                        cwd,
                        provider_session_id,
                        start_event_index,
                        limit,
                    })) {
                        catalog_details.push(request.result(Err(CATALOG_BUSY)));
                    }
                    continue;
                }
                Command::GetDeferredRecoveryPreview { session_id } => {
                    if !startup_deferred.contains(&session_id)
                        || !foreground_sessions.contains(&session_id)
                    {
                        continue;
                    }
                    let Some(session) = owner.session(&session_id)? else {
                        continue;
                    };
                    let cwd = session.cwd;
                    let binding = owner.claude_binding(&session_id)?;
                    if let Some(binding) =
                        binding.filter(|binding| !binding.provider_session_id.is_empty())
                    {
                        let request = CatalogRecentRequest {
                            session_id,
                            cwd,
                            provider_session_id: binding.provider_session_id,
                        };
                        if let Err(
                            mpsc::TrySendError::Full(CatalogRequest::Recent(request))
                            | mpsc::TrySendError::Disconnected(CatalogRequest::Recent(request)),
                        ) = catalog_requests.try_send(CatalogRequest::Recent(request))
                        {
                            send.send_blocking(request.event(None))?;
                        }
                    } else {
                        send.send_blocking(Event::DeferredRecoveryPreview {
                            session_id,
                            cwd,
                            preview: None,
                        })?;
                    }
                    continue;
                }
                Command::LoadProviderSession {
                    session_id,
                    provider_session_id,
                } => {
                    let attempt = (|| -> Result<()> {
                        anyhow::ensure!(
                            foreground_sessions.contains(&session_id),
                            "Card is no longer available in this scene"
                        );
                        ensure!(
                            !permission_changes.contains_key(&session_id)
                                && !shell_promotions.contains_key(&session_id),
                            "AI Session is already stopping or recovering"
                        );
                        if let Some(loading) = catalog_loads.get(&session_id) {
                            anyhow::ensure!(
                                loading.provider_session_id == provider_session_id,
                                "Another conversation is already loading"
                            );
                            return Ok(());
                        }
                        if let Some(identity) = owner
                            .already_loaded_catalog_identity(&session_id, &provider_session_id)?
                        {
                            provider_identity_events.push(identity);
                            return Ok(());
                        }
                        if launch_retries.contains_key(&session_id)
                            || owner.waiting_for_provider_restore(&session_id)
                        {
                            let duplicate =
                                owner.claude_binding(&session_id)?.is_some_and(|binding| {
                                    binding.resumable
                                        && binding.provider_session_id == provider_session_id
                                        && binding.metadata["loadedFromCatalog"] == true
                                });
                            anyhow::ensure!(
                                duplicate,
                                "Card already has a running or starting process"
                            );
                            return Ok(());
                        }
                        anyhow::ensure!(
                            !pending_launch.contains(&session_id)
                                && !recovering.contains_key(&session_id),
                            "Card already has a starting process"
                        );
                        anyhow::ensure!(
                            owner.can_launch_provider(&session_id)
                                || (owner.can_replace_catalog_run(&session_id)
                                    && !owner.claude_binding(&session_id)?.is_some_and(
                                        |binding| { binding.restore_state == "failed" }
                                    )),
                            "Card already has a running or starting process"
                        );
                        let stored = owner
                            .session(&session_id)?
                            .filter(|session| session.archived_at.is_none())
                            .context("Card is no longer available")?;
                        catalog_requests
                            .try_send(CatalogRequest::Index {
                                cwd: stored.cwd,
                                load: Some(CatalogLoadTarget {
                                    scene_id: scene_id.clone().context("No active scene")?,
                                    session_id: session_id.clone(),
                                    provider_session_id: provider_session_id.clone(),
                                }),
                            })
                            .map_err(|_| anyhow!(CATALOG_BUSY))?;
                        catalog_loads.insert(
                            session_id.clone(),
                            CatalogLoad {
                                provider_session_id: provider_session_id.clone(),
                                spawn_revision: None,
                                stopping: None,
                            },
                        );
                        Ok(())
                    })();
                    if let Err(error) = attempt {
                        catalog_load_errors.push(catalog_load_error(
                            &session_id,
                            &provider_session_id,
                            &error,
                        ));
                    }
                    continue;
                }
                Command::RetryProviderRestore { ref session_id }
                | Command::StartFreshProviderSession { ref session_id } => {
                    if !foreground_sessions.contains(session_id)
                        || permission_changes.contains_key(session_id)
                        || shell_promotions.contains_key(session_id)
                        || catalog_loads.contains_key(session_id)
                        || pending_launch.contains(session_id)
                        || launch_retries.contains_key(session_id)
                        || !owner.can_launch_provider(session_id)
                    {
                        continue;
                    }
                    let Some(workspace_id) = owner.session_workspace(session_id)? else {
                        continue;
                    };
                    let change = if matches!(command, Command::RetryProviderRestore { .. }) {
                        ClaudeRestore::Retry
                    } else {
                        ClaudeRestore::Fresh
                    };
                    if owner.session(session_id)?.is_none_or(|session| {
                        session.archived_at.is_some() || session.kind != "claude-code"
                    }) || !owner.change_claude_restore(session_id, change)?
                    {
                        continue;
                    }
                    failed_launches.remove(session_id);
                    sessions.remove(session_id);
                    dirty.remove(session_id);
                    sequence.remove(session_id);
                    scroll_requests.remove(session_id);
                    views.remove(session_id);
                    awaiting_ui.remove(session_id);
                    pending_input.retain(|input| input.session_id != *session_id);
                    operations.retain(|(id, _, _)| id != session_id);
                    searches.retain(|(id, _, _)| id != session_id);
                    copies.retain(|(_, id, _, _)| id != session_id);
                    checks.request(&owner, &workspace_id)?;
                    launch_retries.insert(
                        session_id.clone(),
                        LaunchRetry {
                            workspace_id,
                            recovering: matches!(change, ClaudeRestore::Retry),
                            validated: false,
                        },
                    );
                    continue;
                }
                Command::RetryRecovery { ref session_id }
                | Command::RestartSession { ref session_id } => {
                    let rebuilding = matches!(command, Command::RetryRecovery { .. });
                    if catalog_loads.contains_key(session_id)
                        || permission_changes.contains_key(session_id)
                        || shell_promotions.contains_key(session_id)
                        || launch_retries.contains_key(session_id)
                        || owner
                            .claude_binding(session_id)?
                            .is_some_and(|binding| binding.restore_state == "failed")
                    {
                        continue;
                    }
                    let parked = rebuilding && startup_deferred.contains(session_id);
                    if parked && !foreground_sessions.contains(session_id) {
                        continue;
                    }
                    let failed = failed_launches.get(session_id);
                    let state = if rebuilding {
                        if !parked && !matches!(failed, Some(RecoveryState::Failed(_))) {
                            continue;
                        }
                        RecoveryState::Restoring
                    } else {
                        let reason = match failed {
                            Some(RecoveryState::StartFailed(reason)) => Some(reason),
                            Some(_) => continue,
                            None => None,
                        };
                        if !foreground_sessions.contains(session_id)
                            || pending_launch.contains(session_id)
                            || recovering.contains_key(session_id)
                            || !owner.can_restart_session(session_id, reason.is_some())?
                        {
                            continue;
                        }
                        RecoveryState::Restarting(reason.cloned().unwrap_or_default())
                    };
                    let Some(workspace_id) = owner.session_workspace(session_id)? else {
                        continue;
                    };
                    startup_deferred.remove(session_id);
                    send.send_blocking(Event::Recovery {
                        session_id: session_id.clone(),
                        state,
                    })?;
                    match checks.request(&owner, &workspace_id) {
                        Ok(()) => {
                            if !rebuilding {
                                shell_submissions.remove(session_id);
                                deferred_recovery.remove(session_id);
                                sessions.remove(session_id);
                                dirty.remove(session_id);
                                sequence.remove(session_id);
                                scroll_requests.remove(session_id);
                                views.remove(session_id);
                                awaiting_ui.remove(session_id);
                                pending_input.retain(|input| input.session_id != *session_id);
                                operations.retain(|(id, _, _)| id != session_id);
                                searches.retain(|(id, _, _)| id != session_id);
                                copies.retain(|(_, id, _, _)| id != session_id);
                            }
                            launch_retries.insert(
                                session_id.clone(),
                                LaunchRetry {
                                    workspace_id,
                                    validated: false,
                                    recovering: rebuilding,
                                },
                            );
                        }
                        Err(error) => {
                            claude_launch_targets.remove(session_id);
                            let failure = report_launch_failure(
                                send,
                                session_id,
                                rebuilding,
                                format!("{error:#}"),
                            )?;
                            failed_launches.insert(session_id.clone(), failure);
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
                    let graph =
                        active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
                    pending_launch = unstarted_sessions(&owner, &graph, &sessions)?;
                    pending_launch.retain(|id| {
                        !startup_deferred.contains(id)
                            && !failed_launches.contains_key(id)
                            && !launch_retries.contains_key(id)
                            && !catalog_loads.contains_key(id)
                            && !permission_changes.contains_key(id)
                            && !shell_promotions.contains_key(id)
                    });
                    foreground_sessions = launchable_ids(&graph).map(str::to_owned).collect();
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
                | Command::ForkSession { .. }
                | Command::RetryFork { .. }
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
                #[cfg(all(test, target_os = "macos"))]
                Command::TestPauseShellWorker {
                    session_id,
                    paused,
                    resume,
                } => {
                    owner.send(&session_id, WorkerCommand::TestPause { paused, resume })?;
                    continue;
                }
                #[cfg(all(test, target_os = "macos"))]
                Command::TestFreezeShellWrites {
                    session_id,
                    frozen,
                    reply,
                } => {
                    owner.send(
                        &session_id,
                        WorkerCommand::SetEgressFrozen { frozen, reply },
                    )?;
                    continue;
                }
                #[cfg(all(test, target_os = "macos"))]
                Command::TestFailPromotionClear { session_id } => {
                    owner.send(&session_id, WorkerCommand::TestFailPromotionClear)?;
                    continue;
                }
                Command::Shutdown => {
                    break 'running shutdown_requested.get() == Some(&true);
                }
            };
            let absolute_scroll = matches!(request, WorkerCommand::ScrollTo { .. });
            if let Err(error) = owner.send(&session_id, request) {
                send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                continue;
            }
            // The view request enters the same worker queue, after the scroll.
            if absolute_scroll {
                dirty.insert(session_id.clone());
            }
            operations.push((session_id, result, refresh));
        }
        // Revalidate a selected card after the background read, after any queued
        // navigation/close commands have had a chance to change its authority.
        let stopped_loads: Vec<_> = catalog_loads
            .values_mut()
            .filter_map(|load| {
                let stopped = match load.stopping.as_ref()?.completion.try_recv() {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Empty) => return None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Err(anyhow!("Shell worker did not report its retirement"))
                    }
                };
                Some((load.stopping.take().unwrap().selection, Some(stopped)))
            })
            .collect();
        for (reply, stopped) in stopped_loads
            .into_iter()
            .chain(catalog_replies.try_iter().map(|reply| (reply, None)))
        {
            let (cwd, load, result) = match reply {
                CatalogReply::Detail(result) => {
                    catalog_details.push(result);
                    continue;
                }
                CatalogReply::Recent(event) => {
                    send.send_blocking(event)?;
                    continue;
                }
                CatalogReply::Index { cwd, load, result } => (cwd, load, result),
            };
            let Some(CatalogLoadTarget {
                scene_id: requested_scene,
                session_id,
                provider_session_id,
            }) = load
            else {
                let result = result.and_then(|mut entries| {
                    let ids: Vec<_> = entries
                        .iter()
                        .map(|entry| entry.provider_session_id.as_str())
                        .collect();
                    let mut owners = owner
                        .catalog_binding_owners(&ids)
                        .map_err(|_| "Could not read provider session bindings")?;
                    for entry in &mut entries {
                        entry.loaded_by = owners.remove(&entry.provider_session_id);
                    }
                    Ok(entries)
                });
                catalog_lists.push(match result {
                    Ok(sessions) => json!({"cwd": cwd, "sessions": sessions}),
                    Err(error) => json!({"cwd": cwd, "error": error}),
                });
                continue;
            };
            let Some(load) = catalog_loads.get_mut(&session_id) else {
                // The navigation command already rejected and disposed this load.
                continue;
            };
            let replaced = stopped.is_some();
            let attempt = (|| -> Result<Option<(String, i64)>> {
                if let Some(result) = stopped {
                    result?;
                }
                anyhow::ensure!(
                    scene_id.as_ref() == Some(&requested_scene)
                        && foreground_sessions.contains(&session_id),
                    "Card is no longer available in this scene"
                );
                anyhow::ensure!(
                    !pending_launch.contains(&session_id)
                        && !recovering.contains_key(&session_id)
                        && !launch_retries.contains_key(&session_id)
                        && !owner.waiting_for_provider_restore(&session_id),
                    "Card already has a running or starting process"
                );
                let entry = result
                    .map_err(|error| anyhow!(error))?
                    .into_iter()
                    .next()
                    .context("Provider session was not found in this working directory")?;
                if !owner.can_launch_provider(&session_id) {
                    anyhow::ensure!(!replaced, "Card already has a running or starting process");
                    anyhow::ensure!(
                        owner.session(&session_id)?.is_some_and(|session| {
                            session.archived_at.is_none() && session.cwd == cwd
                        }),
                        "Card is no longer available in this working directory"
                    );
                    let completion = owner.stop_for_provider_relaunch(&session_id)?;
                    load.stopping = Some(CatalogStop {
                        selection: CatalogReply::Index {
                            cwd,
                            load: Some(CatalogLoadTarget {
                                scene_id: requested_scene,
                                session_id: session_id.clone(),
                                provider_session_id: provider_session_id.clone(),
                            }),
                            result: Ok(vec![entry]),
                        },
                        completion,
                    });
                    return Ok(None);
                }
                let workspace_id = owner
                    .session_workspace(&session_id)?
                    .context("Card is no longer available")?;
                checks.request(&owner, &workspace_id)?;
                let revision = owner.load_claude_catalog_binding(&ClaudeCatalogLoad {
                    binding_id: &id(),
                    session_id: &session_id,
                    scene_id: &requested_scene,
                    cwd: &cwd,
                    provider_session_id: &entry.provider_session_id,
                    title: &entry.title,
                    model: entry.model.as_deref(),
                    permission_mode: &entry.permission_mode,
                })?;
                Ok(Some((workspace_id, revision)))
            })();
            match attempt {
                Ok(launch) => {
                    failed_launches.remove(&session_id);
                    deferred_recovery.remove(&session_id);
                    sessions.remove(&session_id);
                    dirty.remove(&session_id);
                    sequence.remove(&session_id);
                    scroll_requests.remove(&session_id);
                    views.remove(&session_id);
                    awaiting_ui.remove(&session_id);
                    pending_input.retain(|input| input.session_id != session_id);
                    operations.retain(|(id, _, _)| id != &session_id);
                    searches.retain(|(id, _, _)| id != &session_id);
                    copies.retain(|(_, id, _, _)| id != &session_id);
                    if let Some((workspace_id, revision)) = launch {
                        claude_launch_targets.remove(&session_id);
                        load.spawn_revision = Some(revision);
                        launch_retries.insert(
                            session_id,
                            LaunchRetry {
                                workspace_id,
                                recovering: true,
                                validated: false,
                            },
                        );
                    }
                }
                Err(error) => {
                    if replaced {
                        claude_launch_targets.remove(&session_id);
                        let failure =
                            report_launch_failure(send, &session_id, true, error.to_string())?;
                        failed_launches.insert(session_id.clone(), failure);
                    }
                    catalog_loads.remove(&session_id);
                    catalog_load_errors.push(catalog_load_error(
                        &session_id,
                        &provider_session_id,
                        &error,
                    ));
                }
            }
        }
        for event in owner.poll(Duration::from_millis(4))? {
            match event {
                OwnerEvent::Domain(mut commit) => {
                    graph_dirty = true;
                    // The persisted exit arrives even when terminal delivery is paused.
                    // Keep the target during intentional retirement for permission changes.
                    if commit.result["profile"] == "claude-code"
                        && commit.result["status"] == "exited"
                        && (commit.result["exitCode"]
                            .as_i64()
                            .is_some_and(|code| code != 0)
                            || commit.result["signal"]
                                .as_i64()
                                .is_some_and(|signal| signal != 0))
                        && let Some(id) = commit.result["sessionId"].as_str()
                        && claude_launch_targets.contains_key(id)
                        && !permission_changes.contains_key(id)
                        && !catalog_loads.contains_key(id)
                        && owner
                            .claude_binding(id)?
                            .is_none_or(|binding| !binding.resumable)
                    {
                        claude_launch_targets.remove(id);
                    }
                    if !commit.replayed
                        && let Some(notification) = commit
                            .result
                            .as_object_mut()
                            .and_then(|result| result.remove("agentNotification"))
                    {
                        agent_notifications.push(notification);
                    }
                }
                OwnerEvent::ProviderIdentity(result) => {
                    if let Some(change) = result["sessionId"]
                        .as_str()
                        .and_then(|id| permission_changes.get_mut(id))
                    {
                        change.observe_identity(&result);
                    }
                    if matches!(
                        result["result"].as_str(),
                        Some("confirmed" | "restore-failed")
                    ) && let Some(session_id) = result["sessionId"].as_str()
                        && catalog_loads.get(session_id).is_some_and(|load| {
                            load.spawn_revision.is_some()
                                && load.spawn_revision == result["spawnRevision"].as_i64()
                                && result["providerSessionId"] == load.provider_session_id
                        })
                    {
                        catalog_loads.remove(session_id);
                    }
                    if matches!(
                        result["result"].as_str(),
                        Some("restore-failed" | "fork-failed")
                    ) && let (Some(id), Some(error)) = (
                        result["sessionId"].as_str(),
                        result["restoreError"]
                            .as_str()
                            .or_else(|| result["forkError"].as_str()),
                    ) {
                        claude_launch_targets.remove(id);
                        pending_input.retain(|input| input.session_id != id);
                        failed_launches
                            .insert(id.to_owned(), RecoveryState::Failed(error.to_owned()));
                        if result.get("runId").is_some() {
                            sessions.remove(id);
                            dirty.remove(id);
                            views.remove(id);
                            awaiting_ui.remove(id);
                            send.send_blocking(Event::Recovery {
                                session_id: id.to_owned(),
                                state: RecoveryState::Failed(error.to_owned()),
                            })?;
                        }
                    }
                    provider_identity_events.push(result);
                }
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
        let mut finished_promotions = Vec::new();
        for (id, promotion) in &mut shell_promotions {
            match promotion.advance(
                &mut owner,
                &options,
                id,
                &terminal_layouts,
                &role_requests,
                &mut claude_launch_targets,
            ) {
                Ok(ShellPromotionProgress::Pending) => continue,
                Ok(ShellPromotionProgress::Attached) => {
                    sessions.insert(id.clone());
                    dirty.insert(id.clone());
                    graph_dirty = true;
                    continue;
                }
                Ok(ShellPromotionProgress::Ready) => {
                    sessions.insert(id.clone());
                    dirty.insert(id.clone());
                    pending_input.append(&mut promotion.inputs);
                    retry_pending_input = true;
                    finished_promotions.push(id.clone());
                    graph_dirty = true;
                    continue;
                }
                Ok(ShellPromotionProgress::Detached) => {}
                Err(error) => {
                    eprintln!("[session.shell-promote-agent:{id}] {error:#}");
                    claude_launch_targets.remove(id);
                    let clearing = matches!(promotion.phase, ShellPromotionPhase::Clearing { .. });
                    if clearing
                        || matches!(
                            promotion.phase,
                            ShellPromotionPhase::RestoringShell(_)
                                | ShellPromotionPhase::RollingBack
                        )
                    {
                        owner.retire_sessions(std::slice::from_ref(id));
                        let message = format!("Shell promotion for {id}: {error:#}");
                        let failure = if clearing {
                            let rebuilding = owner.has_shell_run(id)?;
                            report_session_launch_failure(
                                &mut owner, send, id, rebuilding, message,
                            )?
                        } else {
                            report_launch_failure(send, id, true, message)?
                        };
                        failed_launches.insert(id.clone(), failure);
                        pending_input.retain(|input| &input.session_id != id);
                        finished_promotions.push(id.clone());
                    } else {
                        owner.retire_sessions(std::slice::from_ref(id));
                        promotion.phase = ShellPromotionPhase::RollingBack;
                    }
                }
            }
            sessions.remove(id);
            dirty.remove(id);
            sequence.remove(id);
            scroll_requests.remove(id);
            views.remove(id);
            awaiting_ui.remove(id);
            operations.retain(|(session_id, _, _)| session_id != id);
            searches.retain(|(session_id, _, _)| session_id != id);
            graph_dirty = true;
        }
        for id in finished_promotions {
            shell_promotions.remove(&id);
        }
        let mut completed_changes = Vec::new();
        for (id, change) in &mut permission_changes {
            let terminal = terminal_layouts
                .get(id)
                .copied()
                .unwrap_or_else(default_terminal_options);
            match change.advance(
                &mut owner,
                &options,
                id,
                terminal,
                &role_requests,
                &mut claude_launch_targets,
            ) {
                Ok(PermissionProgress::Pending) => {}
                Ok(PermissionProgress::Launched) => {
                    sessions.insert(id.clone());
                    dirty.insert(id.clone());
                    graph_dirty = true;
                }
                Ok(PermissionProgress::ConsentPrompt) => {
                    // The terminal already knows how to accept keystrokes in Ready.
                    // The host continues to gate which bytes reach the replacement.
                    send.send_blocking(Event::Recovery {
                        session_id: id.clone(),
                        state: RecoveryState::Ready,
                    })?;
                }
                result => completed_changes.push((id.clone(), result)),
            }
        }
        for (id, result) in completed_changes {
            let change = permission_changes
                .remove(&id)
                .expect("completed permission respawn");
            let error = match result {
                Ok(PermissionProgress::Confirmed) => {
                    send.send_blocking(Event::Recovery {
                        session_id: id.clone(),
                        state: RecoveryState::Ready,
                    })?;
                    None
                }
                Ok(PermissionProgress::RestoreFailed(error)) => Some(error),
                Err(error) => {
                    claude_launch_targets.remove(&id);
                    if let Err(rollback) = change.rollback_if_pending(&mut owner, &id) {
                        send.send_blocking(Event::Diagnostic(format!(
                            "rollback permission mode: {rollback:#}"
                        )))?;
                    }
                    owner.retire_sessions(std::slice::from_ref(&id));
                    sessions.remove(&id);
                    dirty.remove(&id);
                    views.remove(&id);
                    awaiting_ui.remove(&id);
                    let message = format!("{error:#}");
                    let failure = report_launch_failure(send, &id, false, message.clone())?;
                    failed_launches.insert(id.clone(), failure);
                    Some(message)
                }
                _ => unreachable!("permission respawn still in progress"),
            };
            permission_results.push(permission_result(&id, &change.mode, error));
            graph_dirty = true;
        }
        let released: Vec<_> = provider_starting
            .keys()
            .filter(|id| !owner.waiting_for_provider_restore(id))
            .cloned()
            .collect();
        for session_id in released {
            provider_starting.remove(&session_id);
            if !failed_launches.contains_key(&session_id) && sessions.contains(&session_id) {
                send.send_blocking(Event::Recovery {
                    session_id,
                    state: RecoveryState::Ready,
                })?;
            }
        }
        while let Some(reply) = history.try_recv()? {
            send.send_blocking(Event::History(reply))?;
        }
        let role_graph_replies = role_replies
            .try_iter()
            .map(|reply| ("claudeRoleAssignmentsResult", serde_json::to_value(reply)));
        let provider_graph_replies = provider_settings
            .replies
            .try_iter()
            .map(|reply| ("providerSettingsResult", serde_json::to_value(reply)));
        let model_catalog_graph_replies = model_catalog_replies
            .try_iter()
            .map(|reply| ("claudeModelCatalogResult", serde_json::to_value(reply)));
        for (field, reply) in role_graph_replies
            .chain(provider_graph_replies)
            .chain(model_catalog_graph_replies)
        {
            let mut graph =
                active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
            graph[field] = reply?;
            let event = Event::Graph(graph);
            if navigation_publications.is_empty() {
                send.send_blocking(event)?;
            } else {
                navigation_publications.push_back((Vec::new(), false, event));
            }
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
                    if interrupted && owner.provider_allows_derivation(session_id) {
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
                    if sessions.contains(session_id) {
                        dirty.insert(session_id.clone());
                    }
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
        if (graph_dirty
            || !provider_identity_events.is_empty()
            || !permission_results.is_empty()
            || !catalog_lists.is_empty()
            || !catalog_details.is_empty()
            || !catalog_load_errors.is_empty())
            && navigation_publications.is_empty()
        {
            let mut graph =
                active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
            if !provider_identity_events.is_empty() {
                graph["providerIdentityEvents"] =
                    json!(std::mem::take(&mut provider_identity_events));
            }
            if !agent_notifications.is_empty() {
                for notification in &mut agent_notifications {
                    if let Some(session_id) = notification["sessionId"].as_str() {
                        match owner.notification_scene_id(window_id, session_id) {
                            Ok(Some(scene_id)) => notification["sceneId"] = json!(scene_id),
                            Ok(None) => {}
                            Err(error) => eprintln!("Resolve notification scene: {error:#}"),
                        }
                    }
                }
                graph["agentNotifications"] =
                    Value::Array(std::mem::take(&mut agent_notifications));
            }
            if !permission_results.is_empty() {
                graph["providerPermissionModeResults"] =
                    json!(std::mem::take(&mut permission_results));
            }
            if !catalog_lists.is_empty() {
                graph["providerSessionCatalogResults"] = json!(std::mem::take(&mut catalog_lists));
            }
            if !catalog_details.is_empty() {
                graph["providerSessionDetailResults"] =
                    Value::Array(std::mem::take(&mut catalog_details));
            }
            if !catalog_load_errors.is_empty() {
                graph["providerSessionLoadErrors"] =
                    json!(std::mem::take(&mut catalog_load_errors));
            }
            send.send_blocking(Event::Graph(graph))?;
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
    shell_promotions.retain(|id, promotion| !promotion.started_at_shutdown(&mut owner, id));
    for (id, change) in permission_changes {
        if let Err(error) = change.rollback_if_pending(&mut owner, &id) {
            eprintln!("[permission-respawn:{id}] {error:#}");
        }
    }
    drop(model_catalog_requests);
    drop(model_catalog_replies);
    drop(role_requests);
    let role_worker = role_worker
        .join()
        .map_err(|_| anyhow!("Claude role assignment worker panicked"));
    let provider_worker = provider_settings.shutdown();
    let mut sessions = owner.shutdown().context("stop native runtime sessions");
    if let Ok(worker) = model_catalog_worker {
        let _ = worker.join();
    }
    for (id, promotion) in shell_promotions {
        if !promotion.promoted {
            continue;
        }
        let restored = promotion.restore_kind(&mut owner, &id).and_then(|()| {
            // The old Shell was stopped for the handoff, not an explicit exit.
            owner.mark_shell_promotion_interrupted(&id)
        });
        if let Err(error) = restored {
            eprintln!("[session.shell-promote-rollback:{id}] {error:#}");
            sessions = sessions.and(Err(error));
        }
    }
    let backup = clean_exit.then(|| owner.backup_for_clean_exit(&data_root));
    drop(owner);
    shutdown_result(
        sessions.map(|_| ()).and(role_worker).and(provider_worker),
        backup,
    )
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

fn workspace_path_error(state: &WorkspacePathState) -> String {
    format!("{WORKSPACE_PATH_INVALID} ({})", state.reason)
}

const CATALOG_BUSY: &str = "Claude Code catalog is busy or unavailable";
// Process contract tests can allow more time for the provider stub to exit.
static PERMISSION_STARTUP_WINDOW: LazyLock<Duration> = LazyLock::new(|| {
    let millis = std::env::var("MATOU_PERMISSION_STARTUP_WINDOW_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(150);
    Duration::from_millis(millis)
});

// Give an immediately failing child time to exit before checking it on its
// worker. The replacement's frames can already be displayed.
const SHELL_PROMOTION_STARTUP_WINDOW: Duration = Duration::from_millis(150);
const SHELL_PROMOTION_SILENT_STARTUP_WINDOW: Duration = Duration::from_secs(2);

struct ShellPromotion {
    mode: &'static str,
    phase: ShellPromotionPhase,
    promoted: bool,
    inputs: VecDeque<PendingInput>,
}

enum ShellPromotionPhase {
    Clearing {
        result: Receiver<Result<bool>>,
        before: ShellInput,
    },
    Stopping(Receiver<Result<()>>),
    Preparing(Receiver<Result<PreparedLaunch>>),
    Starting {
        run_id: String,
        started_at: Instant,
    },
    Verifying {
        run_id: String,
    },
    RollingBack,
    RestoringShell(Receiver<Result<PreparedLaunch>>),
}

enum ShellPromotionProgress {
    Pending,
    Detached,
    Attached,
    Ready,
}

impl ShellPromotion {
    fn advance(
        &mut self,
        owner: &mut ShellOwner,
        options: &LaunchOptions,
        session_id: &str,
        terminal_layouts: &HashMap<String, TerminalOptions>,
        role_requests: &Sender<RoleAssignmentsRequest>,
        claude_launch_targets: &mut HashMap<String, Value>,
    ) -> Result<ShellPromotionProgress> {
        match &mut self.phase {
            ShellPromotionPhase::Clearing { result, before } => {
                let result = poll_promotion_result(result);
                if result.is_err() {
                    owner.restore_shell_input(session_id, std::mem::take(before));
                }
                let Some(cleared) = result? else {
                    return Ok(ShellPromotionProgress::Pending);
                };
                if !cleared {
                    return Ok(ShellPromotionProgress::Ready);
                }
                self.phase =
                    ShellPromotionPhase::Stopping(owner.stop_for_provider_relaunch(session_id)?);
                Ok(ShellPromotionProgress::Detached)
            }
            ShellPromotionPhase::Stopping(result) => {
                let Some(()) = poll_promotion_result(result)? else {
                    return Ok(ShellPromotionProgress::Pending);
                };
                owner.promote_shell_to_claude(session_id)?;
                self.promoted = true;
                self.phase = ShellPromotionPhase::Preparing(prepare_launch(
                    owner,
                    options,
                    session_id,
                    LaunchIntent::ShellPromotion(self.mode),
                    role_requests,
                )?);
                Ok(ShellPromotionProgress::Pending)
            }
            ShellPromotionPhase::Preparing(result) => {
                let Some(launch) = poll_promotion_result(result)? else {
                    return Ok(ShellPromotionProgress::Pending);
                };
                let PreparedLaunch::ClaudeCode {
                    launch,
                    fork_source: None,
                } = launch
                else {
                    bail!("promotion requires a fresh Claude launch");
                };
                let terminal = terminal_layouts
                    .get(session_id)
                    .copied()
                    .unwrap_or_else(default_terminal_options);
                claude_launch_targets
                    .insert(session_id.to_owned(), claude_target_metadata(&launch));
                let run_id = owner.launch_promoted_claude(session_id, terminal, launch)?;
                self.phase = ShellPromotionPhase::Starting {
                    run_id,
                    started_at: Instant::now(),
                };
                Ok(ShellPromotionProgress::Attached)
            }
            ShellPromotionPhase::Starting { run_id, started_at } => {
                let has_output = owner.check_provider_startup(session_id, run_id)?;
                let elapsed = started_at.elapsed();
                if elapsed < SHELL_PROMOTION_STARTUP_WINDOW
                    || (!has_output && elapsed < SHELL_PROMOTION_SILENT_STARTUP_WINDOW)
                {
                    return Ok(ShellPromotionProgress::Pending);
                }
                owner.begin_promotion_process_check(session_id)?;
                self.phase = ShellPromotionPhase::Verifying {
                    run_id: run_id.clone(),
                };
                Ok(ShellPromotionProgress::Pending)
            }
            ShellPromotionPhase::Verifying { run_id } => {
                if !owner.finish_promotion_startup(session_id, run_id)? {
                    return Ok(ShellPromotionProgress::Pending);
                }
                Ok(ShellPromotionProgress::Ready)
            }
            ShellPromotionPhase::RollingBack => {
                if owner.has_shell_worker(session_id) {
                    return Ok(ShellPromotionProgress::Pending);
                }
                self.restore_kind(owner, session_id)?;
                // A directory can disappear while the provider is starting. Use
                // the usual Shell directory selection, including its fallback.
                self.phase = ShellPromotionPhase::RestoringShell(prepare_launch(
                    owner,
                    options,
                    session_id,
                    LaunchIntent::Start,
                    role_requests,
                )?);
                Ok(ShellPromotionProgress::Pending)
            }
            ShellPromotionPhase::RestoringShell(result) => {
                let Some(launch) = poll_promotion_result(result)? else {
                    return Ok(ShellPromotionProgress::Pending);
                };
                let PreparedLaunch::Shell(directory) = launch else {
                    bail!("promotion rollback requires a Shell launch");
                };
                owner.set_shell_launch_directory(session_id, &directory)?;
                launch_shell(owner, options, session_id, directory, terminal_layouts)?;
                Ok(ShellPromotionProgress::Ready)
            }
        }
    }

    fn attached(&self) -> bool {
        matches!(
            self.phase,
            ShellPromotionPhase::Starting { .. } | ShellPromotionPhase::Verifying { .. }
        )
    }

    fn started_at_shutdown(&self, owner: &mut ShellOwner, session_id: &str) -> bool {
        matches!(&self.phase, ShellPromotionPhase::Verifying { run_id }
            if owner.keep_promotion_at_shutdown(session_id, run_id))
    }

    fn restore_kind(&self, owner: &mut ShellOwner, session_id: &str) -> Result<()> {
        // RestoringShell has already rolled the kind back, but the handoff
        // still happened and counts when shutting down that pending restart.
        if self.promoted && !matches!(self.phase, ShellPromotionPhase::RestoringShell(_)) {
            owner.return_promoted_claude_to_shell(session_id)?;
        }
        Ok(())
    }
}

fn poll_promotion_result<T>(result: &Receiver<Result<T>>) -> Result<Option<T>> {
    match result.try_recv() {
        Ok(result) => result.map(Some),
        Err(mpsc::TryRecvError::Empty) => Ok(None),
        Err(error) => Err(error).context("Shell promotion worker disconnected"),
    }
}

struct PermissionChange {
    mode: String,
    binding: Option<ClaudeBinding>,
    phase: PermissionPhase,
}

enum PermissionPhase {
    Stopping(Option<Receiver<Result<()>>>),
    Preparing(Option<Receiver<Result<PreparedLaunch>>>),
    AwaitingIdentity {
        run_id: String,
        startup_until: Option<Instant>,
        consent_shown: bool,
        outcome: Option<Result<(), String>>,
    },
}

enum PermissionProgress {
    Pending,
    Launched,
    ConsentPrompt,
    Confirmed,
    RestoreFailed(String),
}

impl PermissionChange {
    fn advance(
        &mut self,
        owner: &mut ShellOwner,
        options: &LaunchOptions,
        session_id: &str,
        terminal: TerminalOptions,
        role_requests: &Sender<RoleAssignmentsRequest>,
        claude_launch_targets: &mut HashMap<String, Value>,
    ) -> Result<PermissionProgress> {
        match &mut self.phase {
            PermissionPhase::Stopping(stopped) => {
                if let Some(stopped) = stopped {
                    match stopped.try_recv() {
                        Ok(result) => result?,
                        Err(mpsc::TryRecvError::Empty) => return Ok(PermissionProgress::Pending),
                        Err(error) => {
                            return Err(error).context("wait for the current AI Session to stop");
                        }
                    }
                }
                if let Some(binding) = &self.binding {
                    owner.write_claude_permission(
                        session_id,
                        &binding.id,
                        Some(&json!(self.mode)),
                    )?;
                } else {
                    owner.change_claude_restore(session_id, ClaudeRestore::PermissionFresh)?;
                }
                // Record that the metadata was written before scheduling fallible preparation.
                self.phase = PermissionPhase::Preparing(None);
                let fresh_permission = self.binding.is_none().then_some(self.mode.as_str());
                self.phase = PermissionPhase::Preparing(Some(prepare_launch(
                    owner,
                    options,
                    session_id,
                    LaunchIntent::PermissionRestart {
                        fresh_mode: fresh_permission,
                        previous_target: claude_launch_targets.get(session_id),
                    },
                    role_requests,
                )?));
                Ok(PermissionProgress::Pending)
            }
            PermissionPhase::Preparing(preparation) => {
                let preparation = preparation.as_ref().expect("started launch preparation");
                let launch = match preparation.try_recv() {
                    Ok(launch) => launch?,
                    Err(mpsc::TryRecvError::Empty) => return Ok(PermissionProgress::Pending),
                    Err(error) => {
                        return Err(error).context("prepare the AI Session permission respawn");
                    }
                };
                let PreparedLaunch::ClaudeCode {
                    launch: plan,
                    fork_source: None,
                } = launch
                else {
                    bail!("AI Session permission respawn lost its binding");
                };
                let binding = if let Some(previous) = &self.binding {
                    Some(
                        owner
                            .claude_binding(session_id)?
                            .filter(|binding| {
                                binding.id == previous.id
                                    && binding.resumable
                                    && binding.provider_session_id == previous.provider_session_id
                                    && binding.metadata["permissionMode"] == self.mode
                                    && plan.permission_mode == self.mode
                            })
                            .context("AI Session permission respawn lost its binding")?,
                    )
                } else {
                    None
                };
                claude_launch_targets.insert(session_id.to_owned(), claude_target_metadata(&plan));
                let run_id =
                    owner.launch_claude_permission(session_id, terminal, plan, binding.as_ref())?;
                self.phase = PermissionPhase::AwaitingIdentity {
                    run_id,
                    startup_until: Some(Instant::now() + *PERMISSION_STARTUP_WINDOW),
                    consent_shown: false,
                    outcome: None,
                };
                Ok(PermissionProgress::Launched)
            }
            PermissionPhase::AwaitingIdentity {
                run_id,
                startup_until,
                consent_shown,
                outcome,
            } => {
                if let Some(Err(error)) = outcome {
                    return Ok(PermissionProgress::RestoreFailed(std::mem::take(error)));
                }
                if let Some(until) = *startup_until {
                    if Instant::now() < until {
                        return Ok(PermissionProgress::Pending);
                    }
                    owner
                        .finish_provider_startup(session_id, run_id)
                        .context("AI Session permission respawn failed")?;
                    *startup_until = None;
                } else if outcome.is_none() {
                    let running = owner
                        .check_provider_startup(session_id, run_id)
                        .context("AI Session permission respawn failed");
                    if *consent_shown {
                        running
                            .context("Claude Code exited during bypass permissions confirmation")?;
                    } else {
                        running?;
                    }
                }
                if outcome.is_some() {
                    return Ok(PermissionProgress::Confirmed);
                }
                if !*consent_shown && owner.waiting_for_permission_consent(session_id) {
                    *consent_shown = true;
                    return Ok(PermissionProgress::ConsentPrompt);
                }
                Ok(PermissionProgress::Pending)
            }
        }
    }

    fn observe_identity(&mut self, identity: &Value) {
        if let PermissionPhase::AwaitingIdentity {
            run_id, outcome, ..
        } = &mut self.phase
            && identity["runId"] == *run_id
            && self
                .binding
                .as_ref()
                .is_none_or(|binding| identity["bindingId"] == binding.id)
        {
            match identity["result"].as_str() {
                Some("confirmed") => *outcome = Some(Ok(())),
                Some("restore-failed") => {
                    *outcome = Some(Err(identity["restoreError"]
                        .as_str()
                        .unwrap_or("AI Session permission respawn failed")
                        .to_owned()));
                }
                _ => {}
            }
        }
    }

    fn rollback_if_pending(&self, owner: &mut ShellOwner, session_id: &str) -> Result<()> {
        if let Some(binding) = &self.binding
            && matches!(
                self.phase,
                PermissionPhase::Preparing(_)
                    | PermissionPhase::AwaitingIdentity {
                        outcome: None | Some(Ok(())),
                        ..
                    }
            )
        {
            owner.write_claude_permission(
                session_id,
                &binding.id,
                binding.metadata.get("permissionMode"),
            )?;
        }
        Ok(())
    }
}

fn permission_result(session_id: &str, mode: &str, error: Option<String>) -> Value {
    let mut result = json!({ "sessionId": session_id, "permissionMode": mode });
    if let Some(error) = error {
        result["error"] = json!(error);
    }
    result
}

struct CatalogLoad {
    provider_session_id: String,
    spawn_revision: Option<i64>,
    stopping: Option<CatalogStop>,
}

struct CatalogStop {
    selection: CatalogReply,
    completion: Receiver<Result<()>>,
}

struct CatalogLoadTarget {
    scene_id: String,
    session_id: String,
    provider_session_id: String,
}

enum CatalogRequest {
    Index {
        cwd: String,
        load: Option<CatalogLoadTarget>,
    },
    Detail(CatalogDetailRequest),
    Recent(CatalogRecentRequest),
}

struct CatalogRecentRequest {
    session_id: String,
    cwd: String,
    provider_session_id: String,
}

impl CatalogRecentRequest {
    fn event(self, preview: Option<RecentReplies>) -> Event {
        Event::DeferredRecoveryPreview {
            session_id: self.session_id,
            cwd: self.cwd,
            preview,
        }
    }
}

struct CatalogDetailRequest {
    cwd: String,
    provider_session_id: String,
    start_event_index: Option<i64>,
    limit: Option<i64>,
}

impl CatalogDetailRequest {
    fn result(self, result: Result<provider_catalog::SessionDetail, &'static str>) -> Value {
        match result {
            Ok(detail) => json!({
                "cwd": self.cwd, "providerSessionId": self.provider_session_id,
                "events": detail.events, "page": detail.page,
            }),
            Err(error) => json!({
                "cwd": self.cwd, "providerSessionId": self.provider_session_id, "error": error,
            }),
        }
    }
}

enum CatalogReply {
    Index {
        cwd: String,
        load: Option<CatalogLoadTarget>,
        result: Result<Vec<provider_catalog::CatalogEntry>, &'static str>,
    },
    Detail(Value),
    Recent(Event),
}

fn start_provider_catalog() -> Result<(mpsc::SyncSender<CatalogRequest>, Receiver<CatalogReply>)> {
    let (requests, incoming) = mpsc::sync_channel::<CatalogRequest>(16);
    let (outgoing, replies) = mpsc::channel();
    let root = provider_catalog::projects_root();
    thread::Builder::new()
        .name("provider-catalog".into())
        .spawn(move || {
            for request in incoming {
                let reply = match request {
                    CatalogRequest::Index { cwd, load } => {
                        let selected = load.as_ref().map(|load| load.provider_session_id.as_str());
                        let result = provider_catalog::read(root.as_deref(), &cwd, selected);
                        CatalogReply::Index { cwd, load, result }
                    }
                    CatalogRequest::Recent(request) => {
                        let preview = provider_catalog::recent_replies(
                            root.as_deref(),
                            &request.cwd,
                            &request.provider_session_id,
                        )
                        .ok();
                        CatalogReply::Recent(request.event(preview))
                    }
                    CatalogRequest::Detail(request) => {
                        let result = provider_catalog::detail(
                            root.as_deref(),
                            &request.cwd,
                            &request.provider_session_id,
                            request.start_event_index,
                            request.limit,
                        );
                        CatalogReply::Detail(request.result(result))
                    }
                };
                if outgoing.send(reply).is_err() {
                    break;
                }
            }
        })?;
    Ok((requests, replies))
}

fn catalog_load_error(session_id: &str, provider_session_id: &str, error: &anyhow::Error) -> Value {
    json!({"sessionId": session_id, "providerSessionId": provider_session_id, "error": error.to_string()})
}

fn report_session_launch_failure(
    owner: &mut ShellOwner,
    send: &async_channel::Sender<Event>,
    session_id: &str,
    recovering: bool,
    message: String,
) -> Result<RecoveryState> {
    if owner.pending_claude_fork(session_id)?.is_some() {
        let reason = format!("Fork CLI could not start: {message}");
        owner.fail_claude_fork(session_id, &reason)?;
        return report_launch_failure(send, session_id, true, reason);
    }
    if let Some(binding) = owner
        .claude_binding(session_id)?
        .filter(|binding| binding.resumable)
        && owner
            .session(session_id)?
            .is_some_and(|session| session.kind == "claude-code")
    {
        let reason = format!("provider process could not start: {message}");
        if owner.change_claude_restore(
            session_id,
            ClaudeRestore::Fail {
                binding_id: &binding.id,
                error: &reason,
            },
        )? {
            return report_launch_failure(send, session_id, true, reason);
        }
    }
    report_launch_failure(send, session_id, recovering, message)
}

fn report_launch_failure(
    send: &async_channel::Sender<Event>,
    session_id: &str,
    recovering: bool,
    message: String,
) -> Result<RecoveryState> {
    let state = if recovering {
        RecoveryState::Failed(message.clone())
    } else {
        RecoveryState::StartFailed(message.clone())
    };
    send.send_blocking(Event::Recovery {
        session_id: session_id.to_owned(),
        state: state.clone(),
    })?;
    send.send_blocking(Event::Diagnostic(message))?;
    Ok(state)
}

fn send_input(
    owner: &mut ShellOwner,
    input: PendingInput,
    promotions: &mut HashMap<String, ShellPromotion>,
    shell_submissions: &mut HashMap<String, PendingShellSubmission>,
    operations: &mut Vec<WorkerOperation>,
    send: &async_channel::Sender<Event>,
) -> Result<bool> {
    if let Some(promotion) = promotions.get_mut(&input.session_id) {
        promotion.inputs.push_back(input);
        return Ok(false);
    }
    let PendingInput {
        session_id,
        bytes,
        defer_ordering,
        ..
    } = input;
    let interaction = match bytes.as_slice() {
        [13] | [10] => Some("submit"),
        [3] | [4] => Some("control"),
        _ => None,
    };
    let (reply, result) = mpsc::channel();
    let mut pending = None;
    let request = if owner.tracks_shell_input(&session_id) {
        let submission = if bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
            let (request, decision) = mpsc::channel();
            pending = Some(PendingShellSubmission {
                defer_ordering,
                record_on_match: interaction != Some("submit"),
                decision,
                following: VecDeque::new(),
            });
            Some(request)
        } else {
            None
        };
        WorkerCommand::WriteShellInput {
            bytes,
            submission,
            reply,
        }
    } else {
        WorkerCommand::Write { bytes, reply }
    };
    if let Err(error) = owner.send(&session_id, request) {
        send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
        return Ok(false);
    }
    operations.push((session_id.clone(), result, true));
    if let Some(pending) = pending {
        shell_submissions.insert(session_id.clone(), pending);
    }

    // Enter or Ctrl+C is known as soon as the bytes are queued. Only a match
    // within a larger write needs the worker's decision to count as a submit.
    match interaction {
        Some(kind) => record_terminal_interaction(owner, &session_id, kind, defer_ordering, send),
        None => Ok(false),
    }
}

fn record_terminal_interaction(
    owner: &mut ShellOwner,
    session_id: &str,
    kind: &str,
    defer_ordering: bool,
    send: &async_channel::Sender<Event>,
) -> Result<bool> {
    if !owner.provider_allows_derivation(session_id) {
        return Ok(false);
    }
    match owner.record_interaction(session_id, kind, defer_ordering) {
        Ok(_) => Ok(true),
        Err(error) => {
            send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
            Ok(false)
        }
    }
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

fn launchable_nodes(graph: &Value) -> impl Iterator<Item = &Value> {
    graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| {
            matches!(node["currentMode"].as_str(), Some("shell" | "claude-code"))
                && node.get("archivedAt").is_none()
        })
}

fn launchable_ids(graph: &Value) -> impl Iterator<Item = &str> {
    launchable_nodes(graph).filter_map(|node| node["sessionId"].as_str())
}

fn unstarted_sessions(
    owner: &ShellOwner,
    graph: &Value,
    sessions: &HashSet<String>,
) -> Result<HashSet<String>> {
    let mut pending = HashSet::new();
    for node in launchable_nodes(graph) {
        let Some(id) = node["sessionId"]
            .as_str()
            .filter(|id| !sessions.contains(*id))
        else {
            continue;
        };
        if node["currentMode"] == "shell" {
            pending.insert(id.to_owned());
            continue;
        }
        if owner.pending_claude_fork(id)?.is_some() {
            pending.insert(id.to_owned());
            continue;
        }
        let binding = owner.claude_binding(id)?;
        if binding
            .as_ref()
            .is_some_and(|binding| binding.restore_state == "failed")
        {
            continue;
        }
        if binding.as_ref().is_some_and(|binding| binding.resumable)
            || (owner
                .session(id)?
                .is_some_and(|session| session.status == "created")
                && !owner.has_shell_run(id)?)
            || (binding.is_some()
                && owner
                    .session(id)?
                    .is_some_and(|session| session.status == "starting"))
        {
            pending.insert(id.to_owned());
        }
    }
    Ok(pending)
}

fn default_terminal_options() -> TerminalOptions {
    TerminalOptions::new(80, 24, 7, 14)
}

enum PreparedLaunch {
    Shell(String),
    ClaudeCode {
        launch: ClaudeLaunch,
        fork_source: Option<String>,
    },
}

// A binding may be missing or stale before SessionStart confirms this launch.
fn claude_target_metadata(plan: &ClaudeLaunch) -> Value {
    json!({
        "providerConfigId": plan.provider_config_id,
        "model": plan.model,
        "assignedRole": plan.assigned_role,
        "effort": plan.effort,
        "clearAssignmentEnvironment": plan.clear_assignment_environment,
    })
}

enum LaunchIntent<'a> {
    Start,
    ShellPromotion(&'a str),
    PermissionRestart {
        fresh_mode: Option<&'a str>,
        previous_target: Option<&'a Value>,
    },
}

fn load_role_assignments(data_root: &Path) -> ClaudeRoleAssignments {
    startup::open_read_only(&data_root.join("matou.sqlite"))
        .map(|db| read_claude_role_assignments(&db))
        .unwrap_or_default()
}

fn save_role_assignments(data_root: &Path, value: &Value) -> Result<ClaudeRoleAssignments> {
    let assignments = validate_claude_role_assignments(data_root, value)?;
    let mut connection = open_role_assignments_writer(data_root)?;
    save_claude_role_assignments(&mut connection, &assignments, now())?;
    Ok(assignments)
}

fn open_role_assignments_writer(data_root: &Path) -> Result<Connection> {
    // The host holds the database lease. The worker writes the preferences row
    // through its own configured connection to the existing database.
    let connection = Connection::open_with_flags(
        data_root.join("matou.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    configure_connection(&connection)?;
    Ok(connection)
}

fn prepare_launch(
    owner: &ShellOwner,
    options: &LaunchOptions,
    session_id: &str,
    intent: LaunchIntent<'_>,
    role_requests: &Sender<RoleAssignmentsRequest>,
) -> Result<Receiver<Result<PreparedLaunch>>> {
    let session = owner
        .session(session_id)?
        .context("read the authoritative session")?;
    let (send, receive) = mpsc::channel();
    match session.kind.as_str() {
        "shell" => {
            let candidates = owner.shell_directory_candidates(session_id, &options.shell_home)?;
            thread::Builder::new()
                .name("shell-directory".into())
                .spawn(move || {
                    let directory = candidates.into_iter().find(|path| Path::new(path).is_dir());
                    let _ = send.send(
                        directory
                            .map(PreparedLaunch::Shell)
                            .context("no usable Shell launch directory"),
                    );
                })?;
        }
        "claude-code" => {
            // A promoted card always starts a new conversation even if old
            // data for this session ID includes a provider binding.
            let (binding, fork) = if matches!(intent, LaunchIntent::ShellPromotion(_)) {
                ensure!(
                    owner.pending_claude_fork(session_id)?.is_none(),
                    "Shell card has an unconfirmed fork"
                );
                (None, None)
            } else {
                (
                    owner.claude_binding(session_id)?,
                    owner.pending_claude_fork(session_id)?,
                )
            };
            let mut metadata = binding
                .as_ref()
                .map_or(Value::Null, |binding| binding.metadata.clone());
            let mut remembered = false;
            if let LaunchIntent::ShellPromotion(mode) = intent {
                metadata = json!({"permissionMode": mode});
            }
            let resume_id = if let LaunchIntent::PermissionRestart {
                fresh_mode: Some(mode),
                previous_target,
            } = intent
            {
                ensure!(fork.is_none(), "Card has an unconfirmed fork");
                if binding.as_ref().is_none_or(|binding| !binding.resumable)
                    && let Some(target) = previous_target
                {
                    metadata = target.clone();
                    remembered = true;
                }
                if !metadata.is_object() {
                    metadata = json!({});
                }
                metadata["permissionMode"] = json!(mode);
                None
            } else {
                binding
                    .filter(|binding| binding.resumable)
                    .map(|binding| binding.provider_session_id)
            };
            let fresh_launch = fork.is_none()
                && resume_id.is_none()
                && matches!(
                    intent,
                    LaunchIntent::Start | LaunchIntent::ShellPromotion(_)
                );
            let (reply, role_preferences) = mpsc::channel();
            // Every new PTY captures its own preferences. Use the save queue
            // so a prior save is visible before its reply reaches the client.
            role_requests.send(RoleAssignmentsRequest::ForLaunch(reply))?;
            let permission_fresh = matches!(
                intent,
                LaunchIntent::PermissionRestart {
                    fresh_mode: Some(_),
                    ..
                }
            );
            let data_root = options.data_root.clone();
            thread::Builder::new()
                .name("claude-launch".into())
                .spawn(move || {
                    let terminal = default_terminal_options();
                    let cwd = Path::new(&session.cwd);
                    let environment = std::env::vars_os().collect();
                    let result = (|| {
                        let preferences = role_preferences.recv()?;
                        let mut launch = if let Some(fork) = &fork {
                            prepare_claude_fork(
                                &data_root,
                                cwd,
                                &fork.metadata,
                                &fork.source_id,
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        } else if fresh_launch {
                            prepare_new_claude_launch_with_budget(
                                &data_root,
                                cwd,
                                &metadata,
                                &preferences,
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        } else if permission_fresh {
                            prepare_claude_permission_restart(
                                &data_root,
                                cwd,
                                &metadata,
                                remembered,
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        } else {
                            prepare_claude_launch(
                                &data_root,
                                cwd,
                                &metadata,
                                resume_id.as_deref(),
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        }?;
                        if !fresh_launch {
                            launch.capture_restart_gateway(&preferences)?;
                        }
                        Ok(PreparedLaunch::ClaudeCode {
                            launch,
                            fork_source: fork.map(|fork| fork.source_id),
                        })
                    })();
                    let _ = send.send(result);
                })?;
        }
        _ => bail!("unsupported session launch kind"),
    }
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
                profile: LaunchProfile::Shell,
                argv: Vec::new(),
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

fn active_graph(
    owner: &ShellOwner,
    scene_id: Option<&str>,
    window_id: &str,
    startup_deferred: &HashSet<String>,
) -> Result<Value> {
    match scene_id {
        Some(scene_id) => {
            let mut graph = owner.graph(scene_id, Some(window_id))?;
            graph["workspaceSessionCount"] = json!(owner.scene_workspace_session_count(scene_id)?);
            if !startup_deferred.is_empty()
                && let Some(nodes) = graph["nodes"].as_array_mut()
            {
                for node in nodes {
                    let Some(id) = node["sessionId"]
                        .as_str()
                        .filter(|id| startup_deferred.contains(*id))
                    else {
                        continue;
                    };
                    let Some(session) = owner.session(id)? else {
                        continue;
                    };
                    let binding = owner.claude_binding(id)?;
                    node["titleSource"] = json!(session.title_source);
                    node["providerTitle"] = json!(session.provider_title);
                    node["historyProviderSessionId"] =
                        json!(binding.map(|binding| binding.provider_session_id));
                }
            }
            Ok(graph)
        }
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

enum NavigationAction {
    Card(Value),
    Fork(Value),
}

fn navigate(
    owner: &mut ShellOwner,
    window_id: &str,
    command: Command,
) -> Result<(HierarchyResult, Vec<String>, Option<NavigationAction>)> {
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
        Command::ForkSession {
            scene_id,
            source_session_id,
            placement,
            name,
            worktree_mode,
            submission_key,
        } => {
            let input = ForkSessionInput {
                window_id: window_id.into(),
                scene_id,
                source_session_id,
                placement,
                name,
                worktree_mode,
                submission_key,
                now: time,
            };
            let result = owner.create_fork_child(
                &metadata("hierarchy.fork-session", &serde_json::to_value(&input)?),
                &input,
                &ids,
                &messages,
            )?;
            return Ok((
                owner.hierarchy(window_id)?,
                Vec::new(),
                Some(NavigationAction::Fork(result)),
            ));
        }
        Command::RetryFork { session_id } => {
            let result = owner.retry_fork(
                &metadata("hierarchy.retry-fork", &json!({"sessionId": session_id})),
                window_id,
                &session_id,
                time,
            )?;
            return Ok((
                owner.hierarchy(window_id)?,
                Vec::new(),
                Some(NavigationAction::Fork(result)),
            ));
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
            return Ok((
                owner.hierarchy(window_id)?,
                disposed,
                Some(NavigationAction::Card(action)),
            ));
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
            return Ok((
                owner.hierarchy(window_id)?,
                disposed,
                Some(NavigationAction::Card(action)),
            ));
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
    fn role_assignment_writer_uses_runtime_pragmas() -> Result<()> {
        let root = tempfile::tempdir()?;
        let owner = Connection::open(root.path().join("matou.sqlite"))?;
        configure_connection(&owner)?;
        let writer = open_role_assignments_writer(root.path())?;
        let pragma = |name| writer.pragma_query_value(None, name, |row| row.get::<_, i64>(0));
        let journal: String = writer.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        assert_eq!(journal, "wal");
        assert_eq!(pragma("busy_timeout")?, 5000);
        assert_eq!(pragma("foreign_keys")?, 1);
        assert_eq!(pragma("trusted_schema")?, 0);
        assert_eq!(pragma("synchronous")?, 2);
        Ok(())
    }

    #[test]
    fn permission_restart_with_expired_binding_reads_preferences_for_the_new_run() -> Result<()> {
        let root = tempfile::tempdir()?;
        let options = LaunchOptions {
            data_root: root.path().join("data"),
            workspace: root.path().join("workspace"),
            shell_home: root.path().join("home"),
            shell: "/bin/zsh".into(),
        };
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(&options.data_root, "seed")?
        else {
            bail!("test database is not writable");
        };
        let session = hierarchy::bootstrap_window(
            &mut connection,
            &CommandMetadata {
                command_id: id(),
                command_type: "window.bootstrap".into(),
                request_hash: "seed".into(),
                causation_id: None,
                correlation_id: None,
            },
            &BootstrapWindowInput {
                window_id: "main-window-1".into(),
                default_root_directory: options.workspace.to_string_lossy().into_owned(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &hierarchy_ids(),
            "默认",
            1,
        )?
        .session
        .context("seed session")?;
        connection.execute(
            "UPDATE sessions SET kind='claude-code' WHERE id=?",
            [&session.id],
        )?;
        connection.execute(
            "INSERT INTO provider_bindings
                (id, session_id, provider, provider_session_id, resume_state, metadata_json,
                 created_at, updated_at, invalidated_at, validated_at)
             VALUES ('old-binding', ?, 'claude-code', 'old-conversation', 'expired',
                     '{\"model\":\"bound-fixture-model\"}', 1, 1, 1, 1)",
            [&session.id],
        )?;
        let mut owner = ShellOwner::new(connection, ownership, id)?;
        assert!(
            !owner
                .claude_binding(&session.id)?
                .context("expired binding")?
                .resumable
        );
        let (role_requests, requests) = mpsc::channel();
        let mut change = PermissionChange {
            mode: "plan".into(),
            binding: None,
            phase: PermissionPhase::Stopping(None),
        };
        assert!(matches!(
            change.advance(
                &mut owner,
                &options,
                &session.id,
                default_terminal_options(),
                &role_requests,
                &mut HashMap::new(),
            )?,
            PermissionProgress::Pending
        ));
        let RoleAssignmentsRequest::ForLaunch(reply) = requests.try_recv()? else {
            bail!("permission restart did not request current preferences");
        };
        reply.send(Default::default())?;
        assert!(matches!(change.phase, PermissionPhase::Preparing(Some(_))));
        Ok(())
    }

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

    fn open_raw_test_shell(root: &Path) -> Result<(RuntimeHost, String)> {
        let home = root.join("home");
        std::fs::create_dir_all(&home)?;
        std::fs::write(
            home.join(".zshrc"),
            "/bin/stty raw -echo\nprintf 'MATOU_RAW_SHELL_READY\\r\\n'\nexec /bin/cat > \"$HOME/bytes\"\n",
        )?;
        let runtime = RuntimeHost::start(LaunchOptions {
            data_root: root.join("data"),
            workspace: root.join("workspace"),
            shell_home: home,
            shell: "/bin/zsh".into(),
        })?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            match next_test_event(&runtime, deadline)? {
                Event::Frame {
                    session_id, screen, ..
                } if screen.cells.chunks(screen.cols).any(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .contains("MATOU_RAW_SHELL_READY")
                }) =>
                {
                    return Ok((runtime, session_id));
                }
                Event::Diagnostic(message) => bail!("{message}"),
                _ => {}
            }
        }
    }

    fn next_test_event(runtime: &RuntimeHost, deadline: Instant) -> Result<Event> {
        loop {
            match runtime.events.try_recv() {
                Ok(event) => {
                    if let Event::Frame {
                        session_id,
                        sequence,
                        ..
                    } = &event
                    {
                        send_test_command(
                            runtime,
                            Command::Applied {
                                session_id: session_id.clone(),
                                sequence: *sequence,
                            },
                        )?;
                    }
                    return Ok(event);
                }
                Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(error).context("waiting for host event"),
            }
        }
    }

    fn send_test_command(runtime: &RuntimeHost, command: Command) -> Result<()> {
        runtime
            .commands
            .send(command)
            .map_err(|_| anyhow!("runtime command channel closed"))
    }

    fn test_input(session: &str, bytes: &[u8]) -> Command {
        Command::Input {
            session_id: session.to_owned(),
            bytes: bytes.to_vec(),
            defer_ordering: true,
        }
    }

    fn queue_test_batch(
        runtime: &RuntimeHost,
        commands: impl IntoIterator<Item = Command>,
    ) -> Result<()> {
        let (paused, wait_paused) = mpsc::channel();
        let (resume, wait_resume) = mpsc::channel();
        send_test_command(
            runtime,
            Command::TestPause {
                paused,
                resume: wait_resume,
            },
        )?;
        wait_paused.recv_timeout(Duration::from_secs(8))?;
        for command in commands {
            send_test_command(runtime, command)?;
        }
        resume.send(())?;
        Ok(())
    }

    fn assert_raw_shell_input(root: &Path, expected: &[u8]) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let actual = match std::fs::read(root.join("home/bytes")) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error.into()),
            };
            assert!(
                expected.starts_with(&actual),
                "unexpected Shell input: {actual:?}"
            );
            if actual == expected {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "incomplete Shell input: {actual:?}"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn frozen_write_does_not_advance_the_shell_tracker() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
        let freeze = |frozen| -> Result<()> {
            let (reply, result) = mpsc::channel();
            send_test_command(
                &runtime,
                Command::TestFreezeShellWrites {
                    session_id: session_id.clone(),
                    frozen,
                    reply,
                },
            )?;
            result.recv_timeout(Duration::from_secs(3))??;
            Ok(())
        };
        freeze(true)?;
        for bytes in [&b"claude\r"[..], b"clau"] {
            send_test_command(&runtime, test_input(&session_id, bytes))?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Event::Diagnostic(error) = next_test_event(&runtime, deadline)? {
                    assert_eq!(error, "session output is frozen");
                    break;
                }
            }
        }
        freeze(false)?;
        send_test_command(&runtime, test_input(&session_id, b"de\r"))?;
        assert_raw_shell_input(root.path(), b"de\r")?;
        runtime.shutdown()?;
        let db = startup::open_read_only(&root.path().join("data/matou.sqlite"))?;
        let card =
            matou_native_core::sessions::get_session(&db, &session_id)?.context("Shell card")?;
        assert_eq!(card.kind, "shell");
        assert_eq!(
            matou_native_core::sessions::list_runs(&db, &session_id)?.len(),
            1
        );
        Ok(())
    }

    #[test]
    fn storage_pause_drops_input_before_the_shell_tracker() -> Result<()> {
        let root = tempfile::tempdir()?;
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(root.path(), "storage-pause")?
        else {
            bail!("test database is not writable");
        };
        let ids = hierarchy_ids();
        hierarchy::bootstrap_window(
            &mut connection,
            &CommandMetadata {
                command_id: id(),
                command_type: "window.bootstrap".into(),
                request_hash: "fixture".into(),
                causation_id: None,
                correlation_id: None,
            },
            &BootstrapWindowInput {
                window_id: "test-window".into(),
                default_root_directory: root.path().to_string_lossy().into_owned(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &ids,
            "默认",
            1,
        )?;
        let shell = root.path().join("fixture-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\nprintf 'READY\\n'\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done\n",
        )?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700))?;
        let mut owner = ShellOwner::new(connection, ownership, id)?;
        let session_id = ids.session_id;
        owner.launch(
            SessionOptions {
                session_id: session_id.clone(),
                launch: ShellLaunch {
                    profile: LaunchProfile::Shell,
                    argv: Vec::new(),
                    executable: shell,
                    cwd: root.path().into(),
                    data_root: root.path().into(),
                    inherited_env: BTreeMap::from([("HOME".into(), root.path().into())]),
                    env: BTreeMap::new(),
                    cols: 80,
                    rows: 24,
                },
                terminal: default_terminal_options(),
                journal: JournalOptions {
                    max_segment_bytes: 512,
                    ..JournalOptions::default()
                },
                restored_history: Vec::new(),
                collect_history: false,
            },
            false,
            "",
        )?;

        fn wait_for_owner_event(
            owner: &mut ShellOwner,
            found: impl Fn(&SessionEvent) -> bool,
        ) -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                for event in owner.poll(Duration::from_millis(20))? {
                    if let OwnerEvent::Terminal(event) = event
                        && found(&event.event)
                    {
                        return Ok(());
                    }
                }
                ensure!(Instant::now() < deadline, "expected terminal event");
            }
        }
        wait_for_owner_event(&mut owner, |event| {
            matches!(event, SessionEvent::Output { .. })
        })?;
        let range = root
            .path()
            .join("journal")
            .join(&session_id)
            .join("range-index.json");
        std::fs::create_dir(&range)?;
        let (reply, written) = mpsc::channel();
        owner.send(
            &session_id,
            WorkerCommand::Write {
                bytes: format!("{}\n", "x".repeat(2048)).into_bytes(),
                reply,
            },
        )?;
        written.recv_timeout(Duration::from_secs(5))??;
        wait_for_owner_event(&mut owner, |event| {
            matches!(event, SessionEvent::StorageFault { .. })
        })?;

        let (events, _) = async_channel::unbounded();
        let mut promotions = HashMap::new();
        let mut submission = HashMap::new();
        let mut operations = Vec::new();
        let mut send_keys =
            |owner: &mut ShellOwner, operations: &mut Vec<WorkerOperation>, bytes: &[u8]| {
                send_input(
                    owner,
                    PendingInput {
                        workspace_id: ids.workspace_id.clone(),
                        session_id: session_id.clone(),
                        bytes: bytes.to_vec(),
                        defer_ordering: true,
                    },
                    &mut promotions,
                    &mut submission,
                    operations,
                    &events,
                )
            };
        send_keys(&mut owner, &mut operations, b"clau")?;
        let (_, rejected, _) = operations.pop().context("paused write")?;
        let error = rejected.recv_timeout(Duration::from_secs(5))?.unwrap_err();
        assert_eq!(error.to_string(), "session storage is paused");

        std::fs::remove_dir(range)?;
        let (reply, recovered) = mpsc::channel();
        owner.send(&session_id, WorkerCommand::RetryStorage { reply })?;
        recovered.recv_timeout(Duration::from_secs(5))??;
        wait_for_owner_event(&mut owner, |event| {
            matches!(event, SessionEvent::StorageRecovered { .. })
        })?;

        send_keys(&mut owner, &mut operations, b"de\r")?;
        let pending = submission
            .remove(&session_id)
            .context("resumed Shell submission")?;
        assert!(
            pending
                .decision
                .recv_timeout(Duration::from_secs(5))?
                .is_none()
        );
        let (_, written, _) = operations.pop().context("resumed write")?;
        written.recv_timeout(Duration::from_secs(5))??;
        assert!(promotions.is_empty());
        owner.shutdown()?;
        Ok(())
    }

    #[test]
    fn clearing_copy_suppresses_interrupt_and_forwards_the_failed_clear_input() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
        send_test_command(&runtime, test_input(&session_id, b"cla"))?;
        assert_raw_shell_input(root.path(), b"cla")?;

        // Queue Copy behind the qualifying submission and ahead of the owner's
        // Clearing poll. The failed Ctrl+U keeps the old PTY available for inspection.
        queue_test_batch(
            &runtime,
            [
                Command::TestFailPromotionClear {
                    session_id: session_id.clone(),
                },
                test_input(&session_id, b"ude\r"),
                Command::CopySelection {
                    session_id: session_id.clone(),
                    request_id: 42,
                    interrupt_if_empty: true,
                },
            ],
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match next_test_event(&runtime, deadline)? {
                Event::Copied {
                    request_id: 42,
                    text,
                } => {
                    assert_eq!(text, "");
                    break;
                }
                Event::Diagnostic(error) => bail!("{error}"),
                _ => {}
            }
        }

        assert_raw_shell_input(root.path(), b"claude\r")?;
        runtime.shutdown()?;
        Ok(())
    }

    #[test]
    fn successful_fallback_does_not_reuse_the_submitted_shell_prefix() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
        send_test_command(&runtime, test_input(&session_id, b"clau"))?;
        assert_raw_shell_input(root.path(), b"clau")?;
        send_test_command(
            &runtime,
            Command::TestFailPromotionClear {
                session_id: session_id.clone(),
            },
        )?;
        send_test_command(&runtime, test_input(&session_id, b"de\r"))?;
        assert_raw_shell_input(root.path(), b"claude\r")?;
        send_test_command(&runtime, test_input(&session_id, b"de\r"))?;
        assert_raw_shell_input(root.path(), b"claude\rde\r")?;
        runtime.shutdown()?;
        let db = startup::open_read_only(&root.path().join("data/matou.sqlite"))?;
        assert_eq!(
            matou_native_core::sessions::list_runs(&db, &session_id)?.len(),
            1
        );
        Ok(())
    }

    #[test]
    fn shutdown_in_clearing_has_the_same_saved_status_as_plain_shell_shutdown() -> Result<()> {
        for clearing in [false, true] {
            let root = tempfile::tempdir()?;
            let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
            if clearing {
                queue_test_batch(
                    &runtime,
                    [test_input(&session_id, b"claude\r"), Command::Shutdown],
                )?;
            }
            runtime.shutdown()?;
            let db = startup::open_read_only(&root.path().join("data/matou.sqlite"))?;
            let (kind, status): (String, String) = db.query_row(
                "SELECT kind, work_status FROM sessions WHERE id=?",
                [&session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!((kind.as_str(), status.as_str()), ("shell", "exited"));
            assert_eq!(
                matou_native_core::sessions::list_runs(&db, &session_id)?.len(),
                1
            );
        }
        Ok(())
    }

    #[test]
    fn interaction_precedes_queued_focus_and_shutdown() -> Result<()> {
        let root = std::env::temp_dir().join(format!("matou-interaction-order-{}", id()));
        check_interaction_order(&root)
            .with_context(|| format!("isolated bridge data: {}", root.display()))?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    fn two_test_shells(root: &Path) -> Result<(RuntimeHost, String, String, String)> {
        let options = LaunchOptions {
            data_root: root.join("data"),
            workspace: root.join("workspace"),
            shell_home: root.join("home"),
            shell: "/bin/zsh".into(),
        };
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

        let bridge = RuntimeHost::start(options)?;
        let mut ready = HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !ready.contains(&first) || !ready.contains(&second) {
            match next_test_event(&bridge, deadline)? {
                Event::Frame { session_id, .. } => {
                    ready.insert(session_id);
                }
                Event::Diagnostic(message) => bail!("{message}"),
                _ => {}
            }
        }
        Ok((bridge, scene, first, second))
    }

    #[test]
    fn undecided_shell_submission_does_not_block_other_input_focus_or_shutdown() -> Result<()> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        std::fs::create_dir_all(&home)?;
        std::fs::write(
            home.join(".zshrc"),
            "TRAPHUP() { if [[ $OTHER = 1 ]]; then : > \"$HOME/other-stopping\"; fi; exit 0; }\n",
        )?;
        let (mut bridge, scene, other, blocked) = two_test_shells(root.path())?;
        let pause_worker = |bridge: &RuntimeHost| -> Result<Sender<()>> {
            let (paused, wait_paused) = mpsc::channel();
            let (resume, wait_resume) = mpsc::channel();
            send_test_command(
                bridge,
                Command::TestPauseShellWorker {
                    session_id: blocked.clone(),
                    paused,
                    resume: wait_resume,
                },
            )?;
            wait_paused.recv_timeout(Duration::from_secs(3))?;
            Ok(resume)
        };
        let await_file = |name, expected| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(3);
            while std::fs::read_to_string(home.join(name)).ok().as_deref() != Some(expected) {
                ensure!(
                    Instant::now() < deadline,
                    "waiting for Shell to write {name}"
                );
                thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        };

        let resume = pause_worker(&bridge)?;
        for command in [
            test_input(&blocked, b"printf first > \"$HOME/ordered\"\r"),
            test_input(&blocked, b"printf second >> \"$HOME/ordered\"\r"),
            test_input(&other, b"OTHER=1; printf ready > \"$HOME/other\"\r"),
            Command::FocusSession {
                scene_id: scene,
                session_id: other.clone(),
            },
        ] {
            send_test_command(&bridge, command)?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match next_test_event(&bridge, deadline)? {
                Event::Navigation { graph, .. } if graph["focusedSessionId"] == other => break,
                Event::NavigationFailed { message, .. } | Event::Diagnostic(message) => {
                    bail!("{message}");
                }
                _ => {}
            }
        }
        await_file("other", "ready")?;
        assert!(!home.join("ordered").exists());
        resume.send(())?;
        await_file("ordered", "firstsecond")?;

        let resume = pause_worker(&bridge)?;
        send_test_command(&bridge, test_input(&blocked, b"printf queued\r"))?;
        send_test_command(&bridge, Command::Shutdown)?;
        // Shutdown must reach the other PTY while the first worker is still paused.
        await_file("other-stopping", "")?;
        resume.send(())?;
        bridge.shutdown()?;
        Ok(())
    }

    fn check_interaction_order(root: &Path) -> Result<()> {
        let database = root.join("data/matou.sqlite");
        let (mut bridge, scene, first, second) = two_test_shells(root)?;
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
