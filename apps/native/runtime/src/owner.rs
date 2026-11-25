use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::{
    backup::{BackupService, CLEAN_EXIT_BACKUP_TIMEOUT},
    context_optimizer_config::{ContextOptimizerConfig, ContextOptimizerSnapshot},
    dispatch_model_catalog::resolve_control_node,
    dispatch_roles::{DispatchRolesStore, format_roles_markdown},
    hierarchy::{
        self, HierarchyIds, HierarchyResult, Scene, Session as StoredSession, Task, Workspace,
        canvas, cards, tabs, tasks, workspaces,
    },
    journal::checkpoint::CheckpointCandidate,
    ownership::DatabaseOwner,
    provider_launch::{
        ClaudeGatewayProjection, ClaudeGatewayRefresh, EFFORTS, attach_claude_settings,
    },
    pty::{LaunchProfile, ShellLaunch, get_var, set_var},
    session_graph,
    sessions::{
        self, ClaudeBinding, ClaudeFork, ClaudeRestore, ProviderBinding, SessionRun, StartRunInput,
    },
    shell_history,
    storage::{CommandMetadata, DomainCommit},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    ScreenRecoveryOptions, SessionEvent, SessionOptions,
    agent_hud_projection::{
        AgentHudProjection, ConfigCounts, HudProvider, HudSnapshot, RunningTool, TaskStatus,
        ToolActivity, ToolStatus, project_teammates, timestamp_ms,
    },
    agent_hud_sources::{self, ExternalAgents},
    antigravity_catalog::Catalog as AntigravityCatalog,
    antigravity_events::{
        self, Outcome as AntigravityOutcome, RecoveryState as AntigravityRecovery,
    },
    antigravity_launch::{
        self, LaunchMode as AntigravityMode, PermissionMode as AntigravityPermission,
    },
    claude_request_gateway::{Gateway, ProxyEnvironment},
    codex_events::{self, title as codex_title},
    codex_fork::{
        CodexForkAuthority, CodexForkCandidate, CodexForkHook, CodexForkSettings, CodexForkSource,
        CodexForkTarget, CodexForks,
    },
    codex_launch::CodexLaunch,
    codex_resume::{self, Resume as CodexResume},
    context_guard::{HandoffBrief, SessionContextGuard, rescue},
    derived::{CwdTracker, WorkStatusTracker, terminal_summary_lines, utf16_tail},
    dsh_catalog::DshSessionCatalog,
    dsh_events::{DshEventOutcome, DshEvents, DshWorkStatus},
    dsh_launch::{self, DshLaunchInput, DshLaunchPlan},
    dsh_resume::{BeginOutcome, DshResume, ResumeFailure},
    egress_probe::{self, EgressMonitor, EgressSample, Family, RuleMode, SessionEffect},
    fork_worktree::{ForkIdentity, ForkLease, ForkStage, NewWorktree, WorktreeBinding},
    host_control::{RunControl, clear_control_environment},
    provider_catalog::LiveTranscript,
    provider_hook::{
        HOOK_QUEUE_CAPACITY, HookMessage, HookSetup, IDENTITY_TIMEOUT, NotificationKind,
        Observation, ProviderHookServer, Registration, Scope,
    },
    provider_resume::{
        self, FORK_REBUILD_FAILED, IDENTITY_MISMATCH, ResumeMonitor, SESSION_NOT_FOUND,
    },
    session::InputOrigin,
    shell_input::ShellInput,
    subscription_service::{
        LaunchIssue, PreparedClaudeLaunch, SessionSubscriptions, SubscriptionService,
    },
    worker::{CurrentScreen, SessionWorker, WorkerCommand, WorkerEvent},
};

// Allow time for a Claude prompt hook to arrive after terminal input.
const PROVIDER_IDLE_RECOVERY: Duration = Duration::from_secs(15);

pub(crate) struct DshLaunch {
    pub plan: DshLaunchPlan,
    pub config_dump: String,
    pub catalog_root: Option<PathBuf>,
    pub environment_handoff: bool,
}

enum DshObservation {
    Fresh(DshEvents),
    Resume(DshResume, String),
}

impl DshObservation {
    fn observe<'a>(&mut self, run: &str, payload: &'a Value) -> DshEventOutcome<'a> {
        match self {
            Self::Fresh(events) => events.ingest(payload),
            Self::Resume(resume, _) => resume.observe(run, payload),
        }
    }
}

struct AntigravityObservation {
    run: antigravity_events::Run,
    binding: Option<String>,
    catalog_root: PathBuf,
    transcript: LiveTranscript,
    transcript_path: Option<PathBuf>,
    transcript_poll: Instant,
    title_check: Option<(Instant, Instant)>,
    fork_pending: bool,
    startup: Vec<(Scope, antigravity_events::Source, Value, Instant)>,
}

enum SpecialLaunch {
    Antigravity {
        mode: AntigravityMode,
        permission: AntigravityPermission,
        binding: Option<String>,
    },
    Dsh {
        config_dump: String,
        catalog_root: Option<PathBuf>,
        binding: Option<ProviderBinding>,
        environment_handoff: bool,
    },
    CodexFork {
        key: String,
        authority: CodexForkAuthority,
    },
}

struct RunningShell {
    worker: SessionWorker,
    run_id: String,
    cwd: String,
    is_shell: bool,
    #[cfg(any(unix, windows))]
    alias_preheat: Option<ShellInput>,
    restore_history: bool,
    cwd_tracker: CwdTracker,
    work_tracker: WorkStatusTracker,
    emitted_output: bool,
    last_output: Option<Instant>,
    ended: bool,
    exit_error: Option<String>,
    retiring: bool,
    stop_completion: Option<mpsc::Sender<Result<()>>>,
    catalog_stop: bool,
    environment_stop: bool,
    provider_startup_pending: bool,
    promotion_process_check: Option<Arc<OnceLock<Result<()>>>>,
    cwd_capture: Option<(Instant, bool)>,
    hook: Option<RunHook>,
    gateway: Option<Gateway>,
}

impl RunningShell {
    fn promotion_process_result(&self) -> Option<&Result<()>> {
        self.promotion_process_check.as_ref()?.get()
    }

    fn promotion_verified(&self) -> bool {
        !self.retiring && self.promotion_process_result().is_some_and(Result::is_ok)
    }
}

struct RunHook {
    _registration: Registration,
    profile: LaunchProfile,
    codex: Option<codex_events::Run>,
    codex_resume: Option<CodexResume>,
    launch_metadata: Value,
    secrets: Vec<String>,
    uses_gateway: bool,
    default_claude_login: bool,
    deadline: Option<Instant>,
    last_observation: Option<Observation>,
    idle_while_running_since: Option<Instant>,
    last_submission: Option<Instant>,
    resume: Option<ResumeMonitor>,
    claude_fork_rebuild: Option<ClaudeForkRebuild>,
    reported_missing: bool,
    pending_startup_observations: Vec<(Scope, Observation, Instant)>,
    hud: AgentHudProjection,
    context_guard: SessionContextGuard,
    context_card_alert: Option<&'static str>,
    rescue_ready: bool,
    context_session_ended: bool,
    dsh_compacted_percent: Option<f64>,
    codex_context_tools: Vec<RunningTool>,
    codex_context_last_tool: Option<ToolActivity>,
    optimizer_data_root: PathBuf,
    codex_context_percent: Option<f64>,
    gateway_context: Option<Arc<Mutex<(Option<u64>, Option<u64>)>>>,
    external_agents: Option<(PathBuf, ExternalAgents)>,
    antigravity_config: Vec<PathBuf>,
    hud_config: Option<(PathBuf, Instant, Value)>,
    git_status: Option<(PathBuf, Instant, bool)>,
    dsh: Option<DshObservation>,
    antigravity: Option<AntigravityObservation>,
    dsh_pending: Vec<(Scope, Value)>,
    fork: Option<(String, CodexForkAuthority)>,
    codex_fork_ready: bool,
    dsh_status: Option<DshWorkStatus>,
    claude_projects: Option<PathBuf>,
    claude_transcript: Option<PathBuf>,
    claude_live: LiveTranscript,
    claude_waiting_since_ms: Option<f64>,
    claude_transcript_activity_at_ms: Option<f64>,
    claude_transcript_poll: Option<Instant>,
    claude_observed_title: Option<String>,
    permission_identity_error: Option<String>,
}

impl RunHook {
    fn sync_gateway_context(&self) {
        if let Some(context) = &self.gateway_context {
            *context.lock().unwrap() = self.hud.context_usage();
        }
    }

    fn observe_context_usage(
        &mut self,
        gateway: Option<&Gateway>,
    ) -> (Option<f64>, Option<u64>, Option<u64>) {
        let request = gateway.and_then(Gateway::context_usage).filter(|sample| {
            sample
                .observed_at_ms
                .is_some_and(|at| self.hud.accepts_request_usage(at))
        });
        let usage = request.or_else(|| {
            let mut fallback = self.hud.usage_sample()?;
            fallback.estimated_cache_ttl_sec = match self.profile {
                LaunchProfile::ClaudeCode if self.default_claude_login && !self.uses_gateway => {
                    Some(300)
                }
                LaunchProfile::Antigravity => Some(900),
                _ => None,
            };
            Some(fallback)
        });
        let (percent, used, window) = self.hud.context_metrics(request);
        let mut percent = self.codex_context_percent.or(percent);
        if let Some(before) = self.dsh_compacted_percent {
            match percent {
                Some(current) if current == before => percent = None,
                Some(_) => self.dsh_compacted_percent = None,
                None => {}
            }
        }
        self.context_guard.observe_usage(usage);
        if self.context_guard.context_percent() != percent {
            self.context_guard.on_context_percent(percent);
        }
        (percent, used, window)
    }

    fn context_tools<'a>(&'a self, snapshot: &'a HudSnapshot) -> &'a [RunningTool] {
        if self.codex_context_tools.is_empty() {
            snapshot.running_tools.as_deref().unwrap_or_default()
        } else {
            &self.codex_context_tools
        }
    }

    fn awaiting_user(tools: &[RunningTool]) -> bool {
        tools.iter().any(|tool| {
            matches!(
                tool.name.to_ascii_lowercase().as_str(),
                "askuserquestion" | "ask_user_question" | "request_user_input"
            )
        })
    }

    fn observe_context_and_tools(
        &mut self,
        gateway: Option<&Gateway>,
        cwd: &str,
        work_status: &str,
        snapshot: &HudSnapshot,
    ) {
        self.observe_context_usage(gateway);
        let tools = self.context_tools(snapshot);
        let status = if self.context_session_ended
            || matches!(work_status, "exited" | "interrupted" | "error")
        {
            TaskStatus::Idle
        } else if Self::awaiting_user(tools) || work_status == "needs-input" {
            TaskStatus::NeedsInput
        } else if snapshot.task_status == TaskStatus::Idle && work_status == "running" {
            TaskStatus::Running
        } else {
            snapshot.task_status
        };
        let active_tool = status == TaskStatus::Running && !tools.is_empty();
        self.context_guard.set_active_tool(active_tool);
        let last_tool = self
            .codex_context_last_tool
            .as_ref()
            .or(snapshot.last_tool.as_ref());
        self.context_guard.observe_last_tool(last_tool);
        if let Some(tool) = last_tool
            && tool.status == ToolStatus::Completed
            && let Some(target) = tool.tool.target.as_deref()
        {
            for target in rescue::write_targets(&tool.tool.name, &json!({"path": target})) {
                self.context_guard
                    .record_modified_file(&rescue::display_path(
                        Path::new(cwd),
                        Path::new(&target),
                    ));
            }
        }
    }

    fn observe_rescue_tool(&mut self, event: &str, payload: &Value, cwd: &str) {
        if !matches!(event, "PostToolUse" | "PostToolUseFailure") {
            return;
        }
        let input = payload
            .get("tool_input")
            .or_else(|| payload.get("toolInput"))
            .or_else(|| payload.get("input"))
            .unwrap_or(&Value::Null);
        let Some(name) = ["tool_name", "toolName"]
            .into_iter()
            .find_map(|key| payload[key].as_str())
        else {
            return;
        };
        let targets = rescue::write_targets(name, input);
        let command = input["command"]
            .as_str()
            .or_else(|| input["CommandLine"].as_str());
        let response = payload
            .get("tool_response")
            .or_else(|| payload.get("toolResponse"))
            .unwrap_or(&Value::Null);
        let exit_code = ["exitCode", "exit_code"]
            .into_iter()
            .find_map(|key| response[key].as_i64().or_else(|| payload[key].as_i64()));
        let failed = event == "PostToolUseFailure"
            || payload["success"] == false
            || response["success"] == false
            || response["is_error"] == true
            || response["isError"] == true
            || exit_code.is_some_and(|code| code != 0);
        let fallback = crate::agent_hud_projection::target_for_tool(name, input);
        let target = command
            .or_else(|| targets.first().map(String::as_str))
            .or(fallback.as_deref());
        self.context_guard
            .record_tool_result(name, target, !failed, exit_code);
        if !failed {
            for target in targets {
                self.context_guard
                    .record_modified_file(&rescue::display_path(
                        Path::new(cwd),
                        Path::new(&target),
                    ));
            }
        }
    }

    fn build_handoff(
        &self,
        session_id: &str,
        title: &str,
        cwd: &str,
        git_files: &[String],
    ) -> HandoffBrief {
        let locale = sessions::current_interface_locale(sessions::read_locale_preference(
            &self.optimizer_data_root,
        ));
        let log_ids = rescue::recent_log_ids(&self.optimizer_data_root, session_id);
        self.context_guard.build_handoff_with_observations(
            Some(title),
            Some(cwd),
            locale,
            git_files,
            &log_ids,
            &self.secrets,
        )
    }

    fn remember_claude_transcript(&mut self, payload: &Value) {
        let identity = self
            .last_observation
            .as_ref()
            .and_then(|last| last.identity.as_deref());
        let path = ["transcript_path", "transcriptPath"]
            .iter()
            .find_map(|key| payload[*key].as_str())
            .map(Path::new);
        if let Some((identity, path)) = identity.zip(path)
            && path.is_absolute()
            && path.file_name().is_some_and(|name| {
                name == std::ffi::OsStr::new(&format!("{}.jsonl", identity.trim()))
            })
            && self.claude_transcript.as_deref().is_none_or(|previous| {
                previous != path
                    && previous
                        .canonicalize()
                        .ok()
                        .zip(path.canonicalize().ok())
                        .is_none_or(|(previous, reported)| previous != reported)
            })
        {
            self.claude_transcript = Some(path.to_owned());
            self.claude_live = LiveTranscript::default();
            self.claude_transcript_activity_at_ms = None;
            self.hud.ingest_live_claude_history(None);
            self.hud.publish_transcript_context();
            self.claude_transcript_poll = Some(Instant::now());
        }
    }

    fn claude_transcript_resumed(&self) -> bool {
        !self.context_session_ended
            && self
                .claude_waiting_since_ms
                .zip(self.claude_transcript_activity_at_ms)
                .is_some_and(|(waiting, activity)| activity > waiting)
    }
}

const FORK_SOURCE_IDENTITY: &str = "CLI returned the source conversation ID for the fork";
const FORK_IDENTITY_TIMEOUT: &str = "Fork 会话身份确认超时，请重试";
const FORK_IDENTITY_WAIT: Duration = Duration::from_secs(60);
const ACTIVE_PERMISSION_SESSION: &str = "active AI Session is required for permission respawn";
const FRESH_PERMISSION_IDENTITY_TIMEOUT: &str =
    "provider fresh session identity confirmation timed out";

#[derive(Default)]
struct Summary {
    raw: String,
    deadline: Option<Instant>,
}

#[derive(Debug)]
pub enum OwnerEvent {
    Terminal(WorkerEvent),
    /// The event range is committed before consumers refresh their graph views.
    Domain(DomainCommit),
    ProviderIdentity(Value),
    RebuildClaudeFork {
        session_id: String,
        rebuild: Box<ClaudeForkRebuild>,
    },
    Diagnostic {
        session_id: String,
        message: String,
    },
}

pub struct ClaudeForkRebuild {
    pub(crate) parent: String,
    pub(crate) child: String,
    pub(crate) plan: PreparedClaudeLaunch,
}

impl std::fmt::Debug for ClaudeForkRebuild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaudeForkRebuild")
            .finish_non_exhaustive()
    }
}

/// Serialized database-side owner, used on the runtime thread, never the UI thread.
pub struct ShellOwner {
    shells: HashMap<String, RunningShell>,
    catalog_exit_retries: HashMap<String, CatalogExit>,
    summaries: HashMap<String, Summary>,
    send: mpsc::Sender<WorkerEvent>,
    receive: mpsc::Receiver<WorkerEvent>,
    pending: Vec<OwnerEvent>,
    connection: Connection,
    // Field order closes SQLite before releasing its lease.
    _lease: DatabaseOwner,
    generation: String,
    next_id: Box<dyn FnMut() -> String>,
    hook_server: Option<ProviderHookServer>,
    codex_titles: codex_title::Reader,
    codex_hud: codex_events::HudReader,
    run_control: Option<RunControl>,
    control_bin: Option<PathBuf>,
    closing: bool,
    egress: EgressMonitor,
    blocked_gateway_families: Arc<AtomicU8>,
    pub(crate) subscriptions: Option<Arc<SubscriptionService>>,
    egress_membership_changed: bool,
    pub(crate) provider_environment:
        Option<std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>>,
    pub(crate) dsh_session_root: Option<PathBuf>,
    pub(crate) fork_worktree_root: Option<PathBuf>,
    pub(crate) fork_setup: Option<Box<dyn Fn(&Path) -> Result<()> + Send>>,
    codex_forks: CodexForks,
    next_context_reminder_poll: Instant,
    context_cards_changed: bool,
}

struct CatalogExit {
    run: String,
    pid: u32,
    status: matou_native_core::pty::ProcessExit,
    time: i64,
}

pub(crate) const CATALOG_EXIT_NOT_SAVED: &str =
    "The process exited, but its exit record could not be saved";

impl CatalogExit {
    fn persist(&self, db: &mut Connection) -> Result<DomainCommit> {
        sessions::finish_run(
            db,
            &command(
                format!("runtime-exit-{}", self.run),
                "session.run-exit",
                format!("exit:{}:{}", self.run, self.pid),
            ),
            &self.run,
            i64::from(self.status.exit_code),
            self.status.signal.map(i64::from),
            self.time,
            self.time,
        )
        .map_err(|error| anyhow::anyhow!("{CATALOG_EXIT_NOT_SAVED}: {error:#}"))
    }
}

#[derive(PartialEq, Eq)]
enum EgressUpdate {
    Sample,
    Rule,
    Protection,
}

impl ShellOwner {
    /// Startup has already selected writable mode and migrated the database.
    /// Reconcile runs belonging to the previous process before launching new PTYs.
    pub fn new(
        connection: Connection,
        lease: DatabaseOwner,
        next_id: impl FnMut() -> String + 'static,
    ) -> Result<Self> {
        let generation = connection.query_row(
            "SELECT value FROM _runtime_meta WHERE key='runtime_generation'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        let (egress_rule, protection_enabled) = egress_probe::load_settings(&connection)?;
        let (send, receive) = mpsc::channel();
        let mut owner = Self {
            shells: HashMap::new(),
            catalog_exit_retries: HashMap::new(),
            summaries: HashMap::new(),
            send,
            receive,
            pending: Vec::new(),
            connection,
            _lease: lease,
            generation,
            next_id: Box::new(next_id),
            hook_server: None,
            codex_titles: codex_title::Reader::default(),
            codex_hud: codex_events::HudReader::default(),
            run_control: None,
            control_bin: None,
            closing: false,
            egress: EgressMonitor::new(egress_rule, protection_enabled),
            blocked_gateway_families: Arc::default(),
            subscriptions: None,
            egress_membership_changed: false,
            provider_environment: None,
            dsh_session_root: None,
            fork_worktree_root: None,
            fork_setup: None,
            codex_forks: CodexForks::default(),
            next_context_reminder_poll: Instant::now(),
            context_cards_changed: false,
        };
        let runs = {
            let mut query = owner.connection.prepare("SELECT id FROM session_runs WHERE status IN ('starting','running') AND runtime_generation <> ? ORDER BY started_at")?;
            query
                .query_map([&owner.generation], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for run_id in runs {
            let time = now();
            let commit = sessions::interrupt_run(
                &mut owner.connection,
                &command(
                    format!("runtime-recovery-interrupt-{run_id}"),
                    "session.run-interrupt",
                    format!("interrupt:{run_id}"),
                ),
                &run_id,
                time,
                time,
            )?;
            owner.pending.push(OwnerEvent::Domain(commit));
        }
        for session_id in sessions::unfinished_claude_forks(&owner.connection)? {
            owner.fail_claude_fork(
                &session_id,
                "Runtime exited before the fork identity was confirmed",
            )?;
        }
        Ok(owner)
    }

    pub(crate) fn set_run_control(&mut self, control: RunControl, bin: Option<PathBuf>) {
        self.run_control = Some(control);
        self.control_bin = bin;
    }

    pub(crate) fn launch_environment(
        &self,
    ) -> std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> {
        self.provider_environment
            .clone()
            .unwrap_or_else(|| std::env::vars_os().collect())
    }

    pub(crate) fn codex_session_root(&self) -> Option<PathBuf> {
        let Some(env) = &self.provider_environment else {
            return crate::codex_catalog::root();
        };
        crate::codex_launch::config_root(|key| get_var(env, key))
    }

    pub(crate) fn prepare_control_fork_worktree(&mut self, session_id: &str) -> Result<()> {
        let pending: Option<(String, String, String, String, String)> = self
            .connection
            .query_row(
                "SELECT fork.operation_id, fork.submission_key, fork.source_session_id,
                    source.cwd, fork.branch_name FROM session_fork_intents AS fork
             JOIN sessions AS source ON source.id = fork.source_session_id
             WHERE fork.session_id = ? AND fork.source_provider IN ('claude-code', 'antigravity')
               AND fork.state = 'pending' AND fork.stage = 'queued' AND fork.worktree_mode = 'new'
               AND fork.branch_name IS NOT NULL AND fork.worktree_id IS NULL",
                [session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((operation_id, submission_key, source_session_id, source_cwd, branch)) = pending
        else {
            return Ok(());
        };
        let root = self
            .fork_worktree_root
            .as_ref()
            .context("An isolated worktree root is required")?;
        let plan = crate::fork_worktree::ForkWorktreePlan::reserve(
            ForkIdentity {
                operation_id: operation_id.clone(),
                submission_key,
                source_session_id,
                session_id: session_id.into(),
            },
            Path::new(&source_cwd),
            NewWorktree {
                path: root.join(format!("fork-{session_id}")),
                isolation_root: root.clone(),
                branch,
                worktree_id: format!("fork-{session_id}"),
                execution_context_id: format!("fork-context-{session_id}"),
            },
            &[],
        )?;
        // This preparation runs to completion on the serialized owner. The
        // plan rolls back its owned checkout if the durable binding fails.
        let lease = ForkLease {
            operation_id,
            token: uuid::Uuid::new_v4(),
            fence: 1,
        };
        plan.execute(
            &lease,
            |_, _| Ok(()),
            |binding, _| {
                self.fork_setup
                    .as_ref()
                    .map_or(Ok(()), |setup| setup(&binding.cwd))
            },
            |binding, _| {
                let time = now();
                let metadata = command(
                    format!("control-fork-worktree:{session_id}"),
                    "session.fork-worktree-bound",
                    binding.worktree_id.clone(),
                );
                let commit = canvas::bind_control_fork_worktree(
                    &mut self.connection,
                    &metadata,
                    session_id,
                    &core_worktree(binding)?,
                    time,
                )?;
                self.pending.push(OwnerEvent::Domain(commit));
                Ok(())
            },
        )?;
        Ok(())
    }

    pub(crate) fn pending_codex_fork(
        &self,
        session_id: &str,
    ) -> Result<Option<canvas::NativeFork>> {
        canvas::native_codex_fork(&self.connection, session_id)
    }

    pub(crate) fn launch_codex_fork(
        &mut self,
        session_id: &str,
        terminal: matou_native_terminal::TerminalOptions,
        data_root: &Path,
    ) -> Result<()> {
        let record = self
            .pending_codex_fork(session_id)?
            .context("Codex Fork intent unavailable")?;
        let identity = ForkIdentity {
            operation_id: record.operation_id,
            submission_key: record.submission_key.clone(),
            source_session_id: record.source_session_id.clone(),
            session_id: session_id.to_owned(),
        };
        let continuing = self.codex_forks.get_mut(&record.submission_key).is_some();
        if !continuing {
            let source = self
                .session(&record.source_session_id)?
                .context("Codex Fork source unavailable")?;
            let home = self
                .codex_session_root()
                .context("Codex history directory unavailable")?;
            let mut binding = self
                .codex_binding(&source.id)?
                .filter(|binding| binding.provider_session_id == record.source_provider_session_id)
                .context("Codex Fork source identity changed")?;
            // A restored lease already accepted this exact saved source. Its
            // separate foreground PTY can be restoring the same identity.
            if record.lease_fence > 0 && binding.resumable && binding.restore_state == "restoring" {
                binding.restore_state = "none".into();
            }
            let bound = record.worktree.map(|binding| WorktreeBinding {
                identity: identity.clone(),
                worktree_id: binding.id,
                execution_context_id: binding.execution_context_id,
                repository_root: binding.repository_root.into(),
                cwd: binding.cwd.into(),
                branch: binding.branch,
                base_revision: binding.base_revision,
            });
            let target = if record.worktree_mode == "new" && bound.is_none() {
                let root = self
                    .fork_worktree_root
                    .as_ref()
                    .context("An isolated worktree root is required")?;
                CodexForkTarget::NewWorktree(NewWorktree {
                    path: root.join(format!("fork-{session_id}")),
                    isolation_root: root.clone(),
                    branch: record.branch_name.unwrap_or_else(|| {
                        matou_native_core::git_repo::create_git_branch_name(
                            &record.display_name,
                            session_id,
                        )
                    }),
                    worktree_id: format!("fork-{session_id}"),
                    execution_context_id: format!("fork-context-{session_id}"),
                })
            } else {
                CodexForkTarget::Current
            };
            self.codex_forks
                .accept(
                    identity,
                    CodexForkSource {
                        provider: &source.kind,
                        cwd: Path::new(&source.cwd),
                        codex_home: &home,
                        binding: &binding,
                    },
                    target,
                    &[],
                )?
                .resume_persisted(record.lease_fence.try_into()?, bound)?;
        }

        let environment = self.launch_environment();
        let commit = std::cell::RefCell::new((&mut self.connection, &mut self.pending));
        let save = |stage, lease: &ForkLease, binding: Option<&WorktreeBinding>| -> Result<()> {
            let stage = match stage {
                ForkStage::CreatingWorktree => "creating-worktree",
                ForkStage::ApplyingSetup => "applying-setup",
                ForkStage::BindingSession => "binding-session",
                ForkStage::RestoringProvider => "restoring-provider",
                ForkStage::StartingWindow => "starting-window",
                _ => anyhow::bail!("Invalid Codex Fork checkpoint"),
            };
            let token = lease.token.to_string();
            let lease = native_fork_lease(lease, &token, &self.generation)?;
            let binding = binding.map(core_worktree).transpose()?;
            let cmd = format!("fork-{token}-{stage}");
            let mut state = commit.borrow_mut();
            let result = canvas::checkpoint_codex_fork(
                state.0,
                &command(cmd.clone(), "session.fork-progress", cmd),
                session_id,
                &lease,
                stage,
                binding.as_ref(),
                now(),
            )?;
            state.1.push(OwnerEvent::Domain(result));
            Ok(())
        };
        let prepared = self
            .codex_forks
            .get_mut(&record.submission_key)
            .context("Codex Fork intent lost")?
            .begin(
                continuing,
                CodexForkSettings {
                    data_root,
                    environment,
                    cols: terminal.cols,
                    rows: terminal.rows,
                },
                |stage, lease| save(stage, lease, None),
                |binding, _| {
                    self.fork_setup
                        .as_ref()
                        .map_or(Ok(()), |setup| setup(&binding.cwd))
                },
                |binding, lease| save(ForkStage::RestoringProvider, lease, Some(binding)),
            )?;
        let mut launch = prepared.launch;
        let stored = self
            .session(session_id)?
            .context("Codex Fork child unavailable")?;
        ensure!(
            Path::new(&stored.cwd).canonicalize()? == launch.cwd.canonicalize()?,
            "Codex Fork checkout differs from the persisted environment"
        );
        launch.cwd = PathBuf::from(&stored.cwd);
        let metadata = json!({"provider": "codex", "cwd": launch.cwd});
        let secrets = launch
            .env
            .get(std::ffi::OsStr::new("OPENAI_API_KEY"))
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .into_iter()
            .collect();
        let authority = prepared.authority;
        let key = record.submission_key;
        let result = self.launch_run(
            SessionOptions {
                session_id: session_id.to_owned(),
                terminal,
                launch,
                journal: Default::default(),
                restored_history: Vec::new(),
                screen_recovery: None,
                collect_history: false,
            },
            false,
            "",
            Some((metadata, secrets, None, None, None, None)),
            None,
            Some(SpecialLaunch::CodexFork {
                key: key.clone(),
                authority: authority.clone(),
            }),
        );
        if let Err(error) = &result
            && let Some(intent) = self.codex_forks.get_mut(&key)
        {
            intent.fail(&authority, error, |_| Ok(()))?;
        }
        if let Some(warning) = prepared.permission_warning {
            diagnostic(&mut self.pending, session_id, warning.into());
        }
        result
    }

    pub(crate) fn fresh_control_session(&self, session_id: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM domain_events AS event
             JOIN command_deduplication AS command ON command.command_id = event.command_id
             JOIN sessions AS session ON session.id = event.session_id
             WHERE session.id = ? AND session.status = 'created' AND session.archived_at IS NULL
               AND event.event_type = 'session.created' AND command.command_type = 'structure.create.session'
               AND NOT EXISTS (SELECT 1 FROM session_runs WHERE session_id = session.id))",
            [session_id], |row| row.get(0),
        )?)
    }

    pub(crate) fn antigravity_binding(&self, session_id: &str) -> Result<Option<ProviderBinding>> {
        sessions::antigravity_binding(&self.connection, session_id)
    }

    pub(crate) fn pending_antigravity_fork(&self, session_id: &str) -> Result<Option<ClaudeFork>> {
        sessions::pending_provider_fork(&self.connection, session_id, "antigravity")
    }

    pub(crate) fn change_antigravity_restore(
        &mut self,
        session_id: &str,
        change: ClaudeRestore<'_>,
    ) -> Result<bool> {
        let mut events = Vec::new();
        let changed = self.commit_antigravity_restore(session_id, change, None, &mut events)?;
        self.pending.extend(events);
        Ok(changed)
    }

    fn commit_antigravity_restore(
        &mut self,
        session_id: &str,
        change: ClaudeRestore<'_>,
        run_id: Option<&str>,
        events: &mut Vec<OwnerEvent>,
    ) -> Result<bool> {
        let id = (self.next_id)();
        let Some(commit) = sessions::change_antigravity_restore(
            &mut self.connection,
            &command(id.clone(), "session.restore-state-changed", id),
            session_id,
            change,
            now(),
        )?
        else {
            return Ok(false);
        };
        let mut result = commit.result.clone();
        result.as_object_mut().unwrap().remove("agentNotification");
        result["result"] = json!(match change {
            ClaudeRestore::Begin(_) => "restoring",
            ClaudeRestore::Retry => "retrying",
            ClaudeRestore::Fresh => "starting-fresh",
            ClaudeRestore::Fail { .. } => "restore-failed",
            ClaudeRestore::PermissionFresh | ClaudeRestore::Ready { .. } => unreachable!(),
        });
        if let Some(run_id) = run_id {
            result["runId"] = json!(run_id);
        }
        events.push(OwnerEvent::ProviderIdentity(result));
        events.push(OwnerEvent::Domain(commit));
        Ok(true)
    }

    pub(crate) fn dsh_binding(&self, session_id: &str) -> Result<Option<ProviderBinding>> {
        sessions::dsh_binding(&self.connection, session_id)
    }

    pub(crate) fn change_dsh_restore(
        &mut self,
        session_id: &str,
        change: ClaudeRestore<'_>,
    ) -> Result<bool> {
        let id = (self.next_id)();
        let Some(commit) = sessions::change_dsh_restore(
            &mut self.connection,
            &command(id.clone(), "session.dsh-restore", id),
            session_id,
            change,
            now(),
        )?
        else {
            return Ok(false);
        };
        if commit.result["restoreState"] == "failed" {
            let mut result = commit.result.clone();
            result["result"] = json!("restore-failed");
            self.pending.push(OwnerEvent::ProviderIdentity(result));
        }
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(true)
    }

    pub(crate) fn egress(&self) -> &EgressMonitor {
        &self.egress
    }

    pub(crate) fn take_egress_membership_change(&mut self) -> bool {
        std::mem::take(&mut self.egress_membership_changed)
    }

    pub(crate) fn apply_egress_sample(&mut self, sample: EgressSample) -> Result<()> {
        self.update_egress(EgressUpdate::Sample, |monitor, now| {
            Ok(monitor.apply_full_sample(sample, now))
        })
    }

    pub(crate) fn exempt_egress(&mut self, family: Family) -> Result<()> {
        let effects = self
            .egress
            .override_family(family, now() as u64)
            .map_err(anyhow::Error::msg)?;
        self.apply_egress_effects(effects)
    }

    pub(crate) fn set_egress_rule_mode(&mut self, mode: RuleMode) -> Result<()> {
        self.update_egress(EgressUpdate::Rule, |monitor, now| {
            Ok(monitor.set_rule_mode(mode, now))
        })
    }

    pub(crate) fn set_egress_protection(&mut self, enabled: bool) -> Result<()> {
        self.update_egress(EgressUpdate::Protection, |monitor, now| {
            Ok(monitor.set_protection(enabled, now))
        })
    }

    pub(crate) fn accept_egress_baseline(&mut self, family: Family) -> Result<()> {
        self.update_egress(EgressUpdate::Rule, |monitor, now| {
            monitor
                .accept_current(family, now)
                .map_err(anyhow::Error::msg)
        })
    }

    fn update_egress(
        &mut self,
        kind: EgressUpdate,
        update: impl FnOnce(&mut EgressMonitor, u64) -> Result<Vec<SessionEffect>>,
    ) -> Result<()> {
        let time = now();
        // Publish session effects only after the settings they depend on are durable.
        let mut monitor = self.egress.clone();
        let effects = update(&mut monitor, time as u64)?;
        let rule = (kind == EgressUpdate::Rule || monitor.rule() != self.egress.rule())
            .then_some(monitor.rule());
        let protection = (kind == EgressUpdate::Protection).then_some(monitor.protection_enabled());
        if (rule.is_some() || protection.is_some()) && !self.control_is_read_only()? {
            egress_probe::save_settings(&mut self.connection, rule, protection, time)?;
        }
        self.egress = monitor;
        self.apply_egress_effects(effects)
    }

    fn apply_egress_effects(&self, effects: Vec<SessionEffect>) -> Result<()> {
        let blocked = egress_probe::FAMILIES
            .into_iter()
            .filter(|family| self.egress.assert_target_allowed(Some(*family)).is_err())
            .fold(0, |mask, family| mask | (1 << family as u8));
        self.blocked_gateway_families
            .store(blocked, Ordering::Release);
        let mut first_error = None;
        for effect in effects {
            let (id, frozen) = match effect {
                SessionEffect::Freeze(id) => (id, true),
                SessionEffect::Thaw(id) => (id, false),
            };
            if let Some(shell) = self
                .shells
                .get(&id)
                .filter(|shell| !shell.ended && !shell.retiring)
                && let Err(error) = shell.worker.set_egress_frozen(frozen)
            {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(crate) fn agent_hud(&mut self, session_id: &str, data_root: &Path) -> Result<Value> {
        let saved_noise_bytes = self
            .hook_server
            .as_ref()
            .map(|server| server.saved_noise_bytes(session_id))
            .unwrap_or(0);
        let Some(shell) = self
            .shells
            .get_mut(session_id)
            .filter(|shell| !shell.ended && !shell.retiring)
        else {
            return Ok(Value::Null);
        };
        let Some(hook) = shell.hook.as_mut() else {
            return Ok(Value::Null);
        };
        let rows = hook
            .claude_projects
            .as_deref()
            .zip(hook.claude_transcript.as_deref())
            .and_then(|(root, transcript)| read_hud_transcript(root, transcript))
            .unwrap_or_default();
        let locale =
            sessions::current_interface_locale(sessions::read_locale_preference(data_root));
        let teammates = project_teammates(&shell.run_id, session_id, &rows, locale);
        let time = now() as f64;
        let cwd = PathBuf::from(
            hook.hud
                .cwd()
                .filter(|cwd| Path::new(cwd).is_absolute())
                .unwrap_or(&shell.cwd),
        );
        let branch = matou_native_core::git_repo::current_branch(&cwd);
        let refresh_git = hook
            .git_status
            .as_ref()
            .is_none_or(|(path, at, _)| path != &cwd || at.elapsed() >= Duration::from_secs(2));
        if refresh_git {
            let previous = hook
                .git_status
                .as_ref()
                .filter(|(path, _, _)| path == &cwd)
                .map(|(_, _, dirty)| *dirty)
                .unwrap_or(false);
            let dirty = matou_native_core::git_repo::is_dirty(&cwd).unwrap_or(previous);
            hook.git_status = Some((cwd.clone(), Instant::now(), dirty));
        }
        let dirty = hook.git_status.as_ref().is_some_and(|(_, _, dirty)| *dirty);
        if hook
            .hud_config
            .as_ref()
            .is_none_or(|(path, at, _)| path != &cwd || at.elapsed() >= Duration::from_secs(2))
        {
            let legacy = match hook.profile {
                LaunchProfile::ClaudeCode => {
                    let config = hook
                        .claude_projects
                        .as_deref()
                        .and_then(Path::parent)
                        .map(|config| crate::provider_hook::claude_config_counts(&cwd, config));
                    if let Some(config) = &config {
                        let count = |key: &str| config[key].as_u64().unwrap_or_default() as usize;
                        let hook_names = config["hookNames"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect();
                        let mcp_names = config["mcpServerNames"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .map(str::to_owned)
                            .collect();
                        hook.hud.set_config(
                            ConfigCounts {
                                mcps: count("mcps"),
                                hooks: count("hooks"),
                                claude_md: count("claudeMd"),
                                rules: count("rules"),
                            },
                            hook_names,
                            mcp_names,
                        );
                    }
                    config.unwrap_or_default()
                }
                LaunchProfile::Codex => {
                    let (counts, events, mcp_names) = agent_hud_sources::codex_config(
                        hook.codex.as_ref().and_then(|codex| codex.home()),
                    );
                    hook.hud.set_config(counts, events, mcp_names);
                    Value::Null
                }
                LaunchProfile::Antigravity => {
                    let (counts, events, mcp_names) =
                        agent_hud_sources::antigravity_config(&hook.antigravity_config);
                    hook.hud.set_config(counts, events, mcp_names);
                    Value::Null
                }
                _ => Value::Null,
            };
            hook.hud_config = Some((cwd, Instant::now(), legacy));
        }
        if let Some((root, reader)) = &mut hook.external_agents {
            hook.hud.set_external_agents(reader.read(root, time));
        }
        let agents = hook
            .codex
            .as_ref()
            .map(|codex| codex.hud_agents(time))
            .unwrap_or_default();
        let (context_percent, context_used, context_window) =
            hook.observe_context_usage(shell.gateway.as_ref());
        hook.context_guard.set_saved_noise_bytes(saved_noise_bytes);
        let optimizer_settings = ContextOptimizerConfig::for_data_root(data_root)
            .effective_for(hook.context_guard.provider_id())
            .unwrap_or_default();
        hook.hud
            .set_context_guard(hook.context_guard.project_hud(time, &optimizer_settings));
        let mut hud = json!(hook.hud.snapshot_with_agents(time, agents));
        if let Some(percent) = context_percent {
            hud["contextPercent"] = json!(percent.round());
        } else if let Some(hud) = hud.as_object_mut() {
            hud.remove("contextPercent");
        }
        if let Some(window) = context_window {
            hud["contextWindowSize"] = json!(window);
        }
        if optimizer_settings.enabled
            && optimizer_settings.cache_monitor
            && let Some(used) = context_used
            && context_window.is_some()
        {
            hud["contextUsedTokens"] = json!(used);
        }
        hud["defaultAccountUsage"] = json!(
            hook.default_claude_login
                && (hook.last_observation.is_some()
                    || hook
                        .resume
                        .as_ref()
                        .is_some_and(|resume| resume.binding_id.is_some()))
        );
        hud["gitBranch"] = json!(branch);
        hud["gitDirty"] = json!(dirty);
        if let Some((_, _, config)) = &hook.hud_config
            && let Some(config) = config.as_object()
        {
            hud["hookNames"] = config.get("hookNames").cloned().unwrap_or_default();
            hud["configCounts"] = json!(config);
        }
        if let Some(codex) = &hook.codex {
            let transcript =
                codex.hud_projection(|value| safe_hook_text(value, &hook.secrets).is_some());
            if let (Some(hud), Value::Object(fields)) = (hud.as_object_mut(), transcript) {
                hud.extend(fields);
            }
        }
        let mut projection = json!({
            "hud": hud,
            "teammates": teammates,
        });
        redact_hud(&mut projection, &hook.secrets);
        Ok(projection)
    }

    pub fn session_handoff_brief(
        &self,
        session_id: &str,
        _data_root: &Path,
    ) -> Option<HandoffBrief> {
        let shell = self.shells.get(session_id)?;
        let hook = shell.hook.as_ref()?;
        let session = self.session(session_id).ok().flatten()?;
        let title = session.provider_title.as_deref().unwrap_or(&session.title);
        let files = rescue::start_git_snapshot(PathBuf::from(&shell.cwd))
            .ok()
            .and_then(|result| result.recv_timeout(Duration::from_millis(400)).ok())
            .unwrap_or_default();
        Some(hook.build_handoff(session_id, title, &shell.cwd, &files))
    }

    pub(crate) fn rescue_terminal(&self, session_id: &str) -> Option<&str> {
        let shell = self.shells.get(session_id)?;
        let hook = shell.hook.as_ref()?;
        if !shell.emitted_output
            || shell.provider_startup_pending
            || !hook.rescue_ready
            || shell.worker.is_finished()
        {
            return None;
        }
        self.control_terminal(session_id).map(|(run, _)| run)
    }

    pub(crate) fn create_rescue_sibling(
        &mut self,
        command: &CommandMetadata,
        input: &canvas::CreateShellSiblingInput,
        ids: &canvas::CanvasIds,
        messages: &canvas::CanvasMessages<'_>,
    ) -> Result<Value> {
        let session = self
            .session(&input.source_session_id)?
            .context("Session does not exist")?;
        let kind = match session.kind.as_str() {
            "claude-code" => LaunchProfile::ClaudeCode,
            "codex" => LaunchProfile::Codex,
            "antigravity" => LaunchProfile::Antigravity,
            "dsh" => LaunchProfile::Dsh,
            _ => anyhow::bail!("No handoff for a Shell session"),
        };
        let profile = canvas::SessionSiblingProfile {
            kind,
            cwd: Some(&session.cwd),
            permission_mode: None,
        };
        canvas::create_session_sibling(
            &mut self.connection,
            command,
            input,
            ids,
            messages,
            input.now,
            &profile,
        )
    }

    pub fn session(&self, id: &str) -> Result<Option<StoredSession>> {
        sessions::get_session(&self.connection, id)
    }
    pub fn runs(&self, id: &str) -> Result<Vec<SessionRun>> {
        sessions::list_runs(&self.connection, id)
    }
    pub fn graph(&self, scene_id: &str, window_id: Option<&str>) -> Result<Value> {
        let mut graph = session_graph::project_scene_graph(&self.connection, scene_id, window_id)?;
        let focused = graph["focusedSessionId"].as_str().map(str::to_owned);
        if let Some(nodes) = graph["nodes"].as_array_mut() {
            for node in nodes {
                if let Some(alert) = node["sessionId"]
                    .as_str()
                    .and_then(|id| self.shells.get(id))
                    .filter(|shell| !shell.ended && !shell.retiring)
                    .and_then(|shell| shell.hook.as_ref())
                    .and_then(|hook| hook.context_card_alert)
                {
                    node["contextCardAlert"] = json!(alert);
                }
                if node["environment"]["kind"] == "worktree"
                    && node["environment"]["state"] == "ready"
                    && node["environment"]["path"]
                        .as_str()
                        .is_some_and(|path| !Path::new(path).is_dir())
                {
                    node["environment"]["state"] = json!("missing");
                    node["environment"]["error"] = json!("path-missing");
                }
                if node["sessionId"].as_str() == focused.as_deref() {
                    let cwd = focused
                        .as_ref()
                        .and_then(|id| self.shells.get(id))
                        .and_then(|shell| shell.hook.as_ref())
                        .and_then(|hook| hook.hud.cwd())
                        .filter(|cwd| Path::new(cwd).is_absolute())
                        .or_else(|| node["cwd"].as_str());
                    let branch = cwd.and_then(|cwd| {
                        matou_native_core::git_repo::current_branch(Path::new(cwd))
                    });
                    if let Some(branch) = branch {
                        if node["git"].is_object() {
                            node["git"]["branch"] = json!(branch);
                            node["git"]["state"] = json!("ready");
                            node["git"].as_object_mut().unwrap().remove("detachedHead");
                        } else {
                            node["git"] =
                                json!({"state": "ready", "branch": branch, "dirty": false});
                        }
                    } else if let Some(git) = node["git"].as_object_mut() {
                        git.remove("branch");
                    }
                }
                if node["currentMode"] == "codex"
                    && node["providerRestoreState"] == "none"
                    && matches!(node["workStatus"].as_str(), Some("error" | "starting"))
                    && let Some(id) = node["sessionId"].as_str()
                    && let Some(previous) = sessions::unbound_codex_restore(&self.connection, id)?
                    && matches!(
                        previous["restoreState"].as_str(),
                        Some("failed" | "restoring")
                    )
                {
                    node["providerRestoreState"] = previous["restoreState"].clone();
                    node["providerRestoreError"] = previous["restoreError"].clone();
                    node["providerSpawnRevision"] = previous["spawnRevision"].clone();
                }
            }
        }
        Ok(graph)
    }

    pub(crate) fn notification_scene_id(
        &self,
        window_id: &str,
        session_id: &str,
    ) -> Result<Option<String>> {
        // FocusSession accepts the session's canvas membership. Among eligible
        // mounts prefer the latest activation, then the newest mount.
        Ok(self.connection.query_row(
            "SELECT mount.scene_id FROM session_mounts mount
             JOIN session_canvas_memberships member
               ON member.session_id = mount.session_id AND member.scene_id = mount.scene_id
             JOIN sessions ON sessions.id = mount.session_id AND sessions.archived_at IS NULL
             JOIN scenes ON scenes.id = mount.scene_id AND scenes.archived_at IS NULL
             LEFT JOIN window_scene_focus focus ON focus.scene_id = mount.scene_id AND focus.window_id = ?1
             WHERE mount.session_id = ?2
             ORDER BY focus.updated_at DESC, mount.created_at DESC, mount.id DESC LIMIT 1",
            params![window_id, session_id],
            |row| row.get(0),
        ).optional()?)
    }

    pub(crate) fn scene_workspace_session_counts(
        &self,
        scene_id: &str,
    ) -> Result<HashMap<String, i64>> {
        cards::scene_workspace_session_counts(&self.connection, scene_id)
    }

    pub fn hierarchy(&self, window_id: &str) -> Result<HierarchyResult> {
        hierarchy::read_hierarchy(&self.connection, window_id)
    }

    pub fn scenes(&self, task_id: &str) -> Result<Vec<Scene>> {
        hierarchy::list_scenes(&self.connection, task_id)
    }

    pub fn tasks(&self, window_id: &str, workspace_id: &str) -> Result<Vec<Task>> {
        tasks::list_tasks(&self.connection, window_id, workspace_id)
    }

    pub fn workspace_path_states(
        &self,
    ) -> Result<Vec<matou_native_core::workspace_paths::WorkspacePathState>> {
        matou_native_core::workspace_paths::list(&self.connection)
    }
    pub fn workspace_path_state(
        &self,
        workspace_id: &str,
    ) -> Result<Option<matou_native_core::workspace_paths::WorkspacePathState>> {
        matou_native_core::workspace_paths::get(&self.connection, workspace_id)
    }
    pub fn workspace_path_target(&self, workspace_id: &str) -> Result<String> {
        matou_native_core::workspace_paths::target(&self.connection, workspace_id)
    }
    pub(crate) fn last_workspace_validation_generation(&self, workspace_id: &str) -> Result<i64> {
        // Relinking deletes the current row. Receipts still include any generations
        // written by older runtimes between path change events.
        Ok(self.connection.query_row(
            "SELECT CASE WHEN EXISTS (
                SELECT 1 FROM domain_events
                WHERE aggregate_type = 'workspace' AND aggregate_id = ?1
                  AND event_type = 'workspace.path-status-changed'
             ) THEN (
                SELECT COALESCE(MAX(json_extract(response_json, '$.validationGeneration')), 0)
                FROM command_deduplication
                WHERE command_type = 'workspace.validate-path'
                  AND substr(request_hash, 1, length(?1) + 1) = ?1 || ':'
             ) ELSE 0 END",
            [workspace_id],
            |row| row.get(0),
        )?)
    }
    pub fn workspace_path_poll_targets(&self) -> Result<Vec<String>> {
        matou_native_core::workspace_paths::poll_targets(&self.connection)
    }
    pub fn session_workspace(&self, session_id: &str) -> Result<Option<String>> {
        matou_native_core::workspace_paths::session_workspace(&self.connection, session_id)
    }
    pub fn commit_workspace_path(
        &mut self,
        state: &matou_native_core::workspace_paths::WorkspacePathState,
    ) -> Result<()> {
        let command_id = format!("workspace-path-{}", (self.next_id)());
        matou_native_core::workspace_paths::commit(&mut self.connection, command_id, state, now())?;
        Ok(())
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        workspaces::list_workspaces(&self.connection)
    }

    pub fn create_workspace(
        &mut self,
        command: &CommandMetadata,
        input: &workspaces::CreateWorkspaceInput,
        ids: &HierarchyIds,
        committed_at: i64,
    ) -> Result<Value> {
        workspaces::create_workspace(
            &mut self.connection,
            command,
            input,
            ids,
            "默认",
            committed_at,
        )
    }

    pub fn select_workspace(
        &mut self,
        input: &workspaces::SelectWorkspaceInput,
    ) -> Result<HierarchyResult> {
        workspaces::select_workspace(&mut self.connection, input)
    }

    pub fn rename_workspace(&self, input: &workspaces::RenameWorkspaceInput) -> Result<Value> {
        workspaces::rename_workspace(&self.connection, input, "工作空间名称跟随目录名称")
    }

    pub fn relink_workspace(
        &mut self,
        command: &CommandMetadata,
        input: &workspaces::RelinkWorkspaceInput,
        committed_at: i64,
    ) -> Result<Value> {
        workspaces::relink_workspace(
            &mut self.connection,
            command,
            input,
            "默认工作空间始终指向 macOS 用户目录",
            "该目录已经属于另一个工作空间",
            committed_at,
        )
    }

    pub fn set_workspace_pinned(
        &mut self,
        command: &CommandMetadata,
        input: &workspaces::SetWorkspacePinnedInput,
        committed_at: i64,
    ) -> Result<Value> {
        workspaces::set_workspace_pinned(&mut self.connection, command, input, committed_at)
    }

    pub fn reorder_pinned_workspace(
        &mut self,
        command: &CommandMetadata,
        input: &workspaces::ReorderPinnedWorkspaceInput,
        committed_at: i64,
    ) -> Result<Value> {
        workspaces::reorder_pinned_workspace(&mut self.connection, command, input, committed_at)
    }

    pub fn remove_workspace(
        &mut self,
        command: &CommandMetadata,
        input: &workspaces::RemoveWorkspaceInput,
        committed_at: i64,
    ) -> Result<Vec<String>> {
        let result = workspaces::remove_workspace(
            &mut self.connection,
            command,
            input,
            "默认工作空间会保留在侧栏中",
            committed_at,
        )?;
        let disposed: Vec<String> = serde_json::from_value(result["disposedSessionIds"].clone())?;
        self.retire_sessions(&disposed);
        Ok(disposed)
    }

    pub fn record_interaction(
        &mut self,
        session_id: &str,
        kind: &str,
        defer_ordering: bool,
    ) -> Result<Value> {
        let time = now();
        let input = session_graph::RecordInteractionInput {
            session_id: session_id.into(),
            interaction_kind: kind.into(),
            defer_ordering: Some(defer_ordering),
            now: time,
        };
        let command = command(
            (self.next_id)(),
            "session.user-interaction",
            serde_json::to_string(&input)?,
        );
        session_graph::record_interaction(
            &mut self.connection,
            &command,
            &input,
            "用户交互类型不参与会话排序",
            time,
        )
    }

    pub fn create_task(
        &mut self,
        command: &CommandMetadata,
        input: &tasks::CreateTaskInput,
        ids: &HierarchyIds,
        committed_at: i64,
    ) -> Result<Value> {
        tasks::create_task(
            &mut self.connection,
            command,
            input,
            ids,
            "新事项",
            "工作区目录不可用，请先在本地恢复原路径，或移出该工作区",
            committed_at,
        )
    }

    pub fn select_task(&mut self, input: &tasks::SelectTaskInput) -> Result<HierarchyResult> {
        tasks::select_task(&mut self.connection, input)
    }

    pub fn rename_task(
        &mut self,
        command: &CommandMetadata,
        input: &tasks::RenameTaskInput,
        committed_at: i64,
    ) -> Result<Value> {
        tasks::rename_task(&mut self.connection, command, input, committed_at)
    }

    pub(crate) fn move_task_on_board(
        &mut self,
        command: &CommandMetadata,
        input: &tasks::MoveTaskOnBoardInput,
        committed_at: i64,
    ) -> Result<Value> {
        tasks::move_task_on_board(&mut self.connection, command, input, committed_at)
    }

    pub fn set_task_pinned(
        &mut self,
        command: &CommandMetadata,
        input: &tasks::SetTaskPinnedInput,
        committed_at: i64,
    ) -> Result<Value> {
        tasks::set_task_pinned(&mut self.connection, command, input, committed_at)
    }

    pub fn reorder_pinned_task(
        &mut self,
        command: &CommandMetadata,
        input: &tasks::ReorderPinnedTaskInput,
        committed_at: i64,
    ) -> Result<Value> {
        tasks::reorder_pinned_task(&mut self.connection, command, input, committed_at)
    }

    pub fn delete_task(
        &mut self,
        command: &CommandMetadata,
        input: &tasks::DeleteTaskInput,
        ids: &HierarchyIds,
        committed_at: i64,
    ) -> Result<Vec<String>> {
        let result = tasks::delete_task(
            &mut self.connection,
            command,
            input,
            ids,
            "默认",
            committed_at,
        )?;
        let disposed: Vec<String> = serde_json::from_value(result["disposedSessionIds"].clone())?;
        self.retire_sessions(&disposed);
        Ok(disposed)
    }

    pub fn geometry(
        &self,
        scene_id: &str,
    ) -> Result<Vec<matou_native_core::geometry::GeometryUpdate>> {
        matou_native_core::geometry::list(&self.connection, scene_id)
    }
    pub fn put_geometry(
        &self,
        update: &matou_native_core::geometry::GeometryUpdate,
    ) -> Result<matou_native_core::geometry::GeometryUpdate> {
        matou_native_core::geometry::put(&self.connection, update)
    }

    pub fn select_scene(
        &mut self,
        window_id: &str,
        scene_id: &str,
        now: i64,
    ) -> Result<HierarchyResult> {
        hierarchy::select_scene(&mut self.connection, window_id, scene_id, now)
    }

    pub fn focus_session(
        &mut self,
        window_id: &str,
        scene_id: &str,
        session_id: &str,
        now: i64,
        message: &str,
    ) -> Result<Value> {
        hierarchy::focus_session(
            &mut self.connection,
            window_id,
            scene_id,
            session_id,
            now,
            message,
        )
    }

    pub fn create_canvas(
        &mut self,
        command: &CommandMetadata,
        input: &canvas::CreateCanvasInput,
        ids: &canvas::CanvasIds,
        messages: &canvas::CanvasMessages<'_>,
        committed_at: i64,
    ) -> Result<Value> {
        canvas::create_canvas(
            &mut self.connection,
            command,
            input,
            ids,
            messages,
            committed_at,
        )
    }

    pub fn create_shell_sibling(
        &mut self,
        command: &CommandMetadata,
        input: &canvas::CreateShellSiblingInput,
        ids: &canvas::CanvasIds,
        messages: &canvas::CanvasMessages<'_>,
        committed_at: i64,
    ) -> Result<Value> {
        canvas::create_shell_sibling(
            &mut self.connection,
            command,
            input,
            ids,
            messages,
            committed_at,
        )
    }

    pub(crate) fn create_fork_child(
        &mut self,
        command: &CommandMetadata,
        input: &canvas::ForkSessionInput,
        ids: &canvas::CanvasIds,
        messages: &canvas::CanvasMessages<'_>,
    ) -> Result<Value> {
        canvas::create_fork_child(
            &mut self.connection,
            command,
            input,
            ids,
            messages,
            input.now,
        )
    }

    pub(crate) fn retry_fork(
        &mut self,
        command: &CommandMetadata,
        window_id: &str,
        session_id: &str,
        time: i64,
    ) -> Result<Value> {
        // An Antigravity exit is already committed when the failed fork is
        // published. The worker thread can still be unwinding at that point.
        if !self.can_launch_provider(session_id)
            && !(self.shell_worker_is_retiring(session_id)
                && self
                    .session(session_id)?
                    .is_some_and(|session| session.kind == "antigravity"))
        {
            return Err(canvas::CanvasError {
                code: "FORK_NOT_FAILED",
                message: "当前分支无需重试".into(),
            }
            .into());
        }
        canvas::retry_fork_with_navigation(
            &mut self.connection,
            command,
            window_id,
            session_id,
            time,
            command.command_type == "structure.fork.children",
        )
    }

    pub fn rename_scene(
        &mut self,
        command: &CommandMetadata,
        input: &tabs::RenameSceneInput,
        duplicate_scene_name: &str,
        committed_at: i64,
    ) -> Result<Value> {
        tabs::rename_scene(
            &mut self.connection,
            command,
            input,
            duplicate_scene_name,
            committed_at,
        )
    }

    pub fn reorder_scene(
        &mut self,
        command: &CommandMetadata,
        input: &tabs::ReorderSceneInput,
        committed_at: i64,
    ) -> Result<Value> {
        tabs::reorder_scene(&mut self.connection, command, input, committed_at)
    }

    pub fn scene_close_preview(&self, window_id: &str, scene_id: &str) -> Result<(Value, bool)> {
        let is_last = self.connection.query_row(
            "SELECT (SELECT COUNT(*) FROM scenes AS active
                       WHERE active.task_id = task.id AND active.archived_at IS NULL) = 1
                AND (SELECT COUNT(*) FROM tasks AS active
                       WHERE active.workspace_id = task.workspace_id AND active.archived_at IS NULL) = 1
             FROM scenes AS scene JOIN tasks AS task ON task.id = scene.task_id
             WHERE scene.id = ? AND scene.archived_at IS NULL AND task.archived_at IS NULL",
            [scene_id],
            |row| row.get(0),
        )?;
        Ok((self.graph(scene_id, Some(window_id))?, is_last))
    }

    pub fn close_scene(
        &mut self,
        command: &CommandMetadata,
        input: &tabs::CloseSceneInput,
        committed_at: i64,
    ) -> Result<Vec<String>> {
        let result = tabs::close_scene(&mut self.connection, command, input, committed_at)?;
        let disposed: Vec<String> = serde_json::from_value(result["disposedSessionIds"].clone())?;
        self.retire_sessions(&disposed);
        Ok(disposed)
    }

    pub fn close_session_card(
        &mut self,
        command: &CommandMetadata,
        input: &cards::CloseSessionCardInput,
        replacement_ids: &HierarchyIds,
        committed_at: i64,
    ) -> Result<Value> {
        let run_id = self
            .shells
            .get(&input.session_id)
            .filter(|shell| !shell.ended && (!shell.retiring || shell.stop_completion.is_some()))
            .map(|shell| shell.run_id.as_str());
        let result = cards::close_card(
            &mut self.connection,
            command,
            input,
            replacement_ids,
            "默认",
            run_id,
            committed_at,
        )?;
        self.retire_sessions(std::slice::from_ref(&input.session_id));
        Ok(result)
    }

    pub fn remove_session_branch(
        &mut self,
        command: &CommandMetadata,
        input: &cards::RemoveSessionBranchInput,
        committed_at: i64,
    ) -> Result<Value> {
        let result = cards::remove_branch(&mut self.connection, command, input, committed_at)?;
        let disposed: Vec<String> = serde_json::from_value(result["disposedSessionIds"].clone())?;
        self.retire_sessions(&disposed);
        Ok(result)
    }

    /// Stop runs without blocking the other sessions. The stable sessions remain in storage.
    pub fn retire_sessions(&mut self, disposed: &[String]) {
        self.retire_sessions_inner(disposed, true);
    }

    fn retire_sessions_inner(&mut self, disposed: &[String], queue_shutdown: bool) {
        for id in disposed {
            self.egress.unregister_session(id, now() as u64);
            self.summaries.remove(id);
            if let Some(shell) = self.shells.get_mut(id)
                && !shell.retiring
            {
                self.egress_membership_changed |= shell
                    .hook
                    .as_ref()
                    .is_some_and(|hook| tracks_egress(hook.profile));
                if let Some(control) = &self.run_control {
                    control.revoke(&shell.run_id);
                }
                shell.retiring = true;
                shell.gateway.take();
                shell.hook = None;
                shell.cwd_capture = None;
                // Keep the run registered until its final exit record is handled.
                // If the channel already closed, poll will reap and report the worker.
                if queue_shutdown {
                    let _ = shell.worker.send(WorkerCommand::Shutdown { reply: None });
                }
            }
        }
    }

    /// Candidate order follows the frozen runtime's firstUsableDirectory call.
    /// The desktop supplies its isolated HOME for both final home candidates.
    pub fn shell_directory_candidates(
        &self,
        session_id: &str,
        shell_home: &Path,
    ) -> Result<Vec<String>> {
        let (cwd, root, context): (String, String, Option<String>) = self.connection.query_row(
            "SELECT sessions.cwd, workspaces.root_directory, contexts.cwd FROM sessions
             JOIN tasks ON tasks.id=sessions.task_id
             JOIN workspaces ON workspaces.id=tasks.workspace_id
             LEFT JOIN execution_contexts contexts ON contexts.id=sessions.execution_context_id AND contexts.archived_at IS NULL
             WHERE sessions.id=? AND sessions.archived_at IS NULL",
            [session_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let mut candidates = vec![cwd, root];
        candidates.extend(context);
        candidates.push(shell_home.to_string_lossy().into_owned());
        Ok(candidates)
    }

    /// Called after the off-thread directory stat, before the launch authority check.
    pub fn set_shell_launch_directory(&mut self, session_id: &str, cwd: &str) -> Result<()> {
        let mut events = Vec::new();
        self.commit_cwd(session_id, cwd, &mut events)?;
        self.pending.extend(events);
        Ok(())
    }

    pub fn has_shell_worker(&self, session_id: &str) -> bool {
        self.shells.contains_key(session_id)
    }

    pub(crate) fn shell_worker_is_retiring(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .is_some_and(|shell| shell.retiring)
    }

    pub(crate) fn has_shell_run(&self, session_id: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM session_runs WHERE session_id=?)",
            [session_id],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn can_restart_session(&self, session_id: &str, start_failed: bool) -> Result<bool> {
        Ok(!self.closing
            && self
                .shells
                .get(session_id)
                .is_none_or(|shell| shell.ended && !shell.retiring)
            && self.session(session_id)?.is_some_and(|session| {
                matches!(
                    session.kind.as_str(),
                    "shell" | "claude-code" | "codex" | "dsh" | "antigravity"
                ) && session.archived_at.is_none()
                    && (start_failed || session.status == "exited")
            })
            && !sessions::has_unconfirmed_claude_fork(&self.connection, session_id)?
            && !self
                .claude_binding(session_id)?
                .is_some_and(|binding| binding.restore_state == "failed")
            && !self
                .antigravity_binding(session_id)?
                .is_some_and(|binding| binding.restore_state == "failed"))
    }

    pub(crate) fn claude_binding(&self, session_id: &str) -> Result<Option<ClaudeBinding>> {
        sessions::claude_binding(&self.connection, session_id)
    }

    pub(crate) fn pending_claude_fork(&self, session_id: &str) -> Result<Option<ClaudeFork>> {
        sessions::pending_claude_fork(&self.connection, session_id)
    }

    pub(crate) fn fail_claude_fork(&mut self, session_id: &str, error: &str) -> Result<bool> {
        let mut events = Vec::new();
        let failed = self.commit_fork_failure(session_id, None, error, &mut events)?;
        self.pending.extend(events);
        Ok(failed)
    }

    pub(crate) fn load_provider_catalog_binding(
        &mut self,
        input: &sessions::ClaudeCatalogLoad<'_>,
        provider: &str,
        completed_turn: bool,
        replacement: Option<sessions::ClaudeCatalogReplacement<'_>>,
    ) -> Result<i64> {
        let id = (self.next_id)();
        let commit = sessions::load_provider_catalog_binding_with_rollback(
            &mut self.connection,
            &command(id.clone(), "session.load-provider-catalog", id),
            input,
            provider,
            completed_turn,
            replacement,
            now(),
        )?;
        let revision = commit.result["spawnRevision"]
            .as_i64()
            .context("catalog binding is missing its spawn revision")?;
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(revision)
    }

    pub(crate) fn claude_catalog_source(
        &self,
        scene_id: &str,
        target: &str,
        provider_session_id: &str,
        expected_source: Option<&str>,
    ) -> Result<Option<String>> {
        sessions::claude_catalog_source(
            &self.connection,
            scene_id,
            target,
            provider_session_id,
            expected_source,
        )
    }

    pub(crate) fn cancel_claude_catalog_load(
        &mut self,
        session_id: &str,
        revision: i64,
    ) -> Result<()> {
        let id = (self.next_id)();
        sessions::cancel_claude_catalog_load(
            &mut self.connection,
            &command(id.clone(), "session.cancel-catalog-load", id),
            session_id,
            revision,
            now(),
        )
    }

    pub(crate) fn rollback_interrupted_catalog_loads(&mut self) -> Result<()> {
        sessions::rollback_interrupted_catalog_loads(&mut self.connection, now())
    }

    pub(crate) fn catalog_binding_owners(
        &self,
        provider: &str,
        provider_ids: &[&str],
    ) -> Result<HashMap<String, Value>> {
        let mut owners = HashMap::new();
        for (provider_id, session_id, title) in
            sessions::catalog_binding_owners(&self.connection, provider, provider_ids)?
        {
            owners
                .entry(provider_id)
                .or_insert_with(|| json!({"sessionId": session_id, "title": title}));
        }
        Ok(owners)
    }

    pub(crate) fn ensure_codex_catalog_available(
        &self,
        session_id: &str,
        provider_session_id: &str,
    ) -> Result<()> {
        sessions::ensure_codex_catalog_available(&self.connection, session_id, provider_session_id)
    }

    pub(crate) fn startup_recovery_candidates(
        &self,
        scene_id: Option<&str>,
    ) -> Result<Vec<(String, String, bool)>> {
        let mut query = self.connection.prepare(
            "SELECT sessions.id, tasks.workspace_id, sessions.kind, sessions.status,
                    EXISTS(SELECT 1 FROM session_runs WHERE session_id = sessions.id),
                    (sessions.work_status IN ('starting', 'running', 'needs-input', 'interrupted')
                        OR EXISTS (
                            SELECT 1 FROM domain_events
                            WHERE event_id = 'runtime-shutdown-' || (
                                SELECT id FROM session_runs WHERE session_id = sessions.id
                                ORDER BY ordinal DESC LIMIT 1
                            )
                        )) AS automatic,
                    CASE WHEN environment.active_target = 'worktree' THEN owned.worktree_path END
             FROM sessions
             JOIN tasks ON tasks.id = sessions.task_id AND tasks.archived_at IS NULL
             JOIN workspaces ON workspaces.id = tasks.workspace_id AND workspaces.archived_at IS NULL
             LEFT JOIN session_environment_bindings environment ON environment.session_id = sessions.id
             LEFT JOIN worktrees owned ON owned.id = environment.managed_worktree_id
             WHERE sessions.kind IN ('shell', 'claude-code', 'codex', 'dsh', 'antigravity') AND sessions.archived_at IS NULL
               AND COALESCE(environment.state, 'ready') = 'ready'
               AND EXISTS (
                   SELECT 1 FROM session_mounts JOIN scenes ON scenes.id = session_mounts.scene_id
                   WHERE session_mounts.session_id = sessions.id AND scenes.archived_at IS NULL
               )
             ORDER BY COALESCE((
                 SELECT MAX(membership.updated_at) FROM session_canvas_memberships AS membership
                 WHERE membership.session_id = sessions.id
                   AND max(membership.last_user_interaction_seq, membership.pending_user_interaction_seq) > 0
               ), sessions.created_at) DESC,
               EXISTS (
                   SELECT 1 FROM session_mounts WHERE session_id = sessions.id AND scene_id = ?
               ) DESC, sessions.id",
        )?;
        let rows = query.query_map([scene_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut candidates = Vec::new();
        for row in rows {
            let (id, workspace, kind, status, has_run, automatic, worktree_path) = row?;
            if worktree_path
                .as_deref()
                .is_some_and(|path| !Path::new(path).is_dir())
            {
                continue;
            }
            if kind == "shell" {
                if has_run {
                    candidates.push((id, workspace, automatic));
                }
                continue;
            }
            if kind == "antigravity" {
                if self.pending_antigravity_fork(&id)?.is_some()
                    || self
                        .antigravity_binding(&id)?
                        .is_some_and(|binding| binding.resumable)
                    || self.unbound_provider_can_start_fresh(&id, "antigravity")?
                {
                    candidates.push((id, workspace, true));
                }
                continue;
            }
            if kind == "dsh" {
                if self
                    .dsh_binding(&id)?
                    .is_some_and(|binding| binding.resumable)
                {
                    candidates.push((id, workspace, true));
                }
                continue;
            }
            if kind == "codex" {
                if sessions::has_unconfirmed_claude_fork(&self.connection, &id)? {
                    continue;
                }
                let binding = self.codex_binding(&id)?;
                let failed = binding
                    .as_ref()
                    .is_some_and(|b| b.restore_state == "failed")
                    || (binding.is_none()
                        && sessions::unbound_codex_restore(&self.connection, &id)?
                            .is_some_and(|event| event["restoreState"] == "failed"));
                if (!failed && (has_run || binding.is_some()))
                    || self.unbound_provider_can_start_fresh(&id, "codex")?
                {
                    candidates.push((id, workspace, true));
                }
                continue;
            }
            if self.pending_claude_fork(&id)?.is_some() {
                continue;
            }
            let binding = self.claude_binding(&id)?;
            if binding.as_ref().is_some_and(|binding| binding.resumable) {
                candidates.push((id, workspace, true));
                continue;
            }
            // A card that normally starts fresh still follows its existing launch
            // path. Failed restores have their own recovery banner and commands.
            let starts_fresh =
                (status == "created" && !has_run) || (status == "starting" && binding.is_some());
            if !starts_fresh
                && (has_run || binding.is_some())
                && !binding.is_some_and(|binding| binding.restore_state == "failed")
            {
                candidates.push((id, workspace, false));
            }
        }
        Ok(candidates)
    }

    pub(crate) fn change_claude_restore(
        &mut self,
        session_id: &str,
        change: ClaudeRestore<'_>,
    ) -> Result<bool> {
        let mut events = Vec::new();
        let changed = self.commit_claude_restore(session_id, change, None, &mut events)?;
        self.pending.extend(events);
        Ok(changed)
    }

    fn commit_claude_restore(
        &mut self,
        session_id: &str,
        change: ClaudeRestore<'_>,
        run_id: Option<&str>,
        events: &mut Vec<OwnerEvent>,
    ) -> Result<bool> {
        let (result, mismatch) = match change {
            ClaudeRestore::Begin(_) => ("restoring", false),
            ClaudeRestore::Fail { error, .. } => (
                "restore-failed",
                error.strip_prefix(FORK_REBUILD_FAILED).unwrap_or(error) == IDENTITY_MISMATCH,
            ),
            ClaudeRestore::Retry => ("retrying", false),
            ClaudeRestore::Fresh | ClaudeRestore::PermissionFresh => ("starting-fresh", false),
            ClaudeRestore::Ready { .. } => unreachable!(),
        };
        let id = (self.next_id)();
        let Some(commit) = sessions::change_claude_restore(
            &mut self.connection,
            &command(id.clone(), "session.restore-state-changed", id),
            session_id,
            change,
            now(),
        )?
        else {
            return Ok(false);
        };
        let mut payload = commit.result.clone();
        payload["result"] = json!(result);
        if let Some(run_id) = run_id {
            payload["runId"] = json!(run_id);
        }
        if mismatch {
            let mut rejected = payload.clone();
            rejected["result"] = json!("rejected");
            rejected["reason"] = json!("identity-mismatch");
            events.push(OwnerEvent::ProviderIdentity(rejected));
        }
        events.push(OwnerEvent::ProviderIdentity(payload));
        events.push(OwnerEvent::Domain(commit));
        Ok(true)
    }

    pub(crate) fn codex_binding(&self, id: &str) -> Result<Option<sessions::ProviderBinding>> {
        sessions::codex_binding(&self.connection, id)
    }

    pub(crate) fn unbound_provider_can_start_fresh(
        &self,
        id: &str,
        provider: &str,
    ) -> Result<bool> {
        // A previous run can have stopped before the provider created a conversation.
        // A title or a recorded identity is evidence of a conversation even if its binding is lost.
        Ok(self.connection.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM sessions WHERE id = ?1 AND kind = ?2
                  AND kind IN ('codex', 'antigravity') AND archived_at IS NULL
                  AND status IN ('exited', 'interrupted') AND provider_title IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM provider_bindings WHERE session_id = ?1 AND provider = ?2
                  ) AND NOT EXISTS (
                      SELECT 1 FROM domain_events WHERE session_id = ?1
                        AND event_type IN ('provider-binding.recorded', 'session.mode-changed', 'session.restore-state-changed')
                        AND json_extract(payload_json, '$.provider') = ?2
                        AND json_type(payload_json, '$.providerSessionId') = 'text'
                  )
            )",
            params![id, provider],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn prepare_fresh_codex_start(&mut self, id: &str) -> Result<bool> {
        if self.unbound_provider_can_start_fresh(id, "codex")? {
            // Clear a legacy no-binding failure and its notification before projecting the card.
            self.change_codex_restore(id, ClaudeRestore::Fresh)?;
            Ok(true)
        } else {
            self.pending_codex_fresh(id)
        }
    }

    pub(crate) fn pending_codex_fresh(&self, id: &str) -> Result<bool> {
        // A committed fresh action has no run ID. The first verified identity
        // supersedes it, even if the owner was restarted between the two.
        Ok(self
            .connection
            .query_row(
                "SELECT event_type = 'session.restore-state-changed'
                    AND json_extract(payload_json, '$.restoreState') = 'none'
                    AND json_type(payload_json, '$.runId') IS NULL
             FROM domain_events WHERE session_id = ?
               AND event_type IN ('session.restore-state-changed', 'provider-binding.recorded')
               AND json_extract(payload_json, '$.provider') = 'codex'
             ORDER BY seq DESC LIMIT 1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub(crate) fn change_codex_restore(
        &mut self,
        session_id: &str,
        change: ClaudeRestore<'_>,
    ) -> Result<bool> {
        let mut events = Vec::new();
        let changed = self.commit_codex_restore(session_id, change, None, &mut events)?;
        self.pending.extend(events);
        Ok(changed)
    }

    fn commit_codex_restore(
        &mut self,
        session_id: &str,
        change: ClaudeRestore<'_>,
        run_id: Option<&str>,
        events: &mut Vec<OwnerEvent>,
    ) -> Result<bool> {
        let result = match change {
            ClaudeRestore::Begin(_) => "restoring",
            ClaudeRestore::Ready { .. } => "timed-out",
            ClaudeRestore::Fail { .. } => "restore-failed",
            ClaudeRestore::Retry => "retrying",
            ClaudeRestore::Fresh => "starting-fresh",
            ClaudeRestore::PermissionFresh => unreachable!("Codex uses its own permission restart"),
        };
        let id = (self.next_id)();
        let Some(commit) = sessions::change_codex_restore(
            &mut self.connection,
            &command(id.clone(), "session.restore-state-changed", id),
            session_id,
            change,
            codex_resume::failure_title(),
            now(),
        )?
        else {
            return Ok(false);
        };
        let mut payload = commit.result.clone();
        payload
            .as_object_mut()
            .expect("restore payload")
            .remove("agentNotification");
        payload["result"] = json!(result);
        if let Some(run_id) = run_id {
            payload["runId"] = json!(run_id);
        }
        events.push(OwnerEvent::ProviderIdentity(payload));
        events.push(OwnerEvent::Domain(commit));
        Ok(true)
    }

    fn park_codex_run(
        &mut self,
        session_id: &str,
        run_id: &str,
        binding_id: &str,
        error: &str,
        events: &mut Vec<OwnerEvent>,
    ) {
        self.retire_sessions(&[session_id.to_owned()]);
        if let Err(error) = self.commit_codex_restore(
            session_id,
            ClaudeRestore::Fail { binding_id, error },
            Some(run_id),
            events,
        ) {
            diagnostic(
                events,
                session_id,
                format!("persist Codex restore failure: {error:#}"),
            );
        }
    }

    pub(crate) fn waiting_for_provider_restore(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .and_then(|shell| shell.hook.as_ref())
            .is_some_and(|hook| {
                hook.antigravity
                    .as_ref()
                    .is_some_and(|agy| agy.binding.is_some())
                    || ((hook
                        .resume
                        .as_ref()
                        .is_some_and(ResumeMonitor::tracks_restore)
                        || hook.codex_resume.is_some()
                        || hook.dsh.is_some()
                        || hook.antigravity.is_some()
                        || hook.fork.is_some())
                        && hook.deadline.is_some())
            })
    }

    pub(crate) fn provider_allows_derivation(&self, session_id: &str) -> bool {
        self.shells.get(session_id).is_some_and(|shell| {
            !shell.retiring
                && (shell.is_shell
                    || shell.hook.as_ref().is_some_and(|hook| {
                        hook.last_observation.is_some()
                            || hook
                                .antigravity
                                .as_ref()
                                .is_some_and(|agy| agy.run.conversation_id().is_some())
                    }))
        })
    }

    pub(crate) fn can_launch_provider(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .is_none_or(|shell| shell.ended && !shell.retiring && shell.worker.is_finished())
    }

    pub(crate) fn can_replace_catalog_run(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .is_some_and(|shell| !shell.ended || shell.retiring)
    }

    pub(crate) fn stop_for_provider_relaunch(
        &mut self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<Result<()>>> {
        let shell = self
            .shells
            .get_mut(session_id)
            .context("Card has no worker to stop")?;
        ensure!(
            shell.stop_completion.is_none(),
            "Card is already being stopped"
        );
        let (send, receive) = mpsc::channel();
        shell.stop_completion = Some(send);
        self.retire_sessions(&[session_id.to_owned()]);
        Ok(receive)
    }

    pub(crate) fn stop_for_catalog_load(
        &mut self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<Result<()>>> {
        let shell = self
            .shells
            .get_mut(session_id)
            .context("Card has no worker to stop")?;
        ensure!(
            !shell.retiring && shell.stop_completion.is_none(),
            "Card is already being stopped"
        );
        shell
            .worker
            .send(WorkerCommand::Shutdown { reply: None })
            .context("Could not request that the process stop")?;
        let (send, receive) = mpsc::channel();
        shell.stop_completion = Some(send);
        shell.catalog_stop = true;
        self.retire_sessions_inner(&[session_id.to_owned()], false);
        Ok(receive)
    }

    pub(crate) fn retry_catalog_exit(&mut self, session_id: &str) -> Result<()> {
        if let Some(exit) = self.catalog_exit_retries.get(session_id) {
            let commit = exit.persist(&mut self.connection)?;
            self.catalog_exit_retries.remove(session_id);
            self.pending.push(OwnerEvent::Domain(commit));
        }
        Ok(())
    }

    pub(crate) fn environment_worker_active(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .is_some_and(|shell| !shell.ended && !shell.retiring && !shell.worker.is_finished())
    }

    pub(crate) fn environment_provider_binding(
        &self,
        session_id: &str,
        profile: &str,
    ) -> Result<Option<ProviderBinding>> {
        match profile {
            "claude-code" => self.claude_binding(session_id),
            "codex" => self.codex_binding(session_id),
            "antigravity" => self.antigravity_binding(session_id),
            "dsh" => self.dsh_binding(session_id),
            _ => Ok(None),
        }
    }

    pub(crate) fn reset_environment_resume(
        &mut self,
        session_id: &str,
        profile: &str,
        binding_id: &str,
    ) -> Result<()> {
        if self
            .environment_provider_binding(session_id, profile)?
            .is_some_and(|binding| binding.id == binding_id && binding.restore_state == "failed")
        {
            match profile {
                "claude-code" => self.change_claude_restore(session_id, ClaudeRestore::Retry)?,
                "codex" => self.change_codex_restore(session_id, ClaudeRestore::Retry)?,
                "antigravity" => {
                    self.change_antigravity_restore(session_id, ClaudeRestore::Retry)?
                }
                "dsh" => self.change_dsh_restore(session_id, ClaudeRestore::Retry)?,
                _ => false,
            };
        }
        Ok(())
    }

    pub(crate) fn environment_storage(&self) -> &Connection {
        &self.connection
    }

    pub(crate) fn environment_storage_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    pub(crate) fn stop_for_environment_transition(
        &mut self,
        session_id: &str,
    ) -> Result<Option<mpsc::Receiver<Result<()>>>> {
        if !self.has_shell_worker(session_id) {
            return Ok(None);
        }
        let result = self.stop_for_provider_relaunch(session_id)?;
        self.shells.get_mut(session_id).unwrap().environment_stop = true;
        Ok(Some(result))
    }

    pub(crate) fn tracks_shell_input(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .is_some_and(|shell| shell.is_shell && !shell.ended && !shell.retiring)
    }

    pub(crate) fn restore_shell_input(&self, session_id: &str, before: ShellInput) {
        if let Some(shell) = self.shells.get(session_id) {
            shell.worker.restore_shell_input(before);
        }
    }

    pub(crate) fn promote_shell_to_agent(
        &mut self,
        session_id: &str,
        profile: LaunchProfile,
    ) -> Result<()> {
        let id = (self.next_id)();
        let commit = sessions::promote_shell_to_agent(
            &mut self.connection,
            &command(id.clone(), "session.shell-promote-agent", id),
            session_id,
            profile,
            now(),
        )?;
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(())
    }

    pub(crate) fn return_promoted_agent_to_shell(
        &mut self,
        session_id: &str,
        profile: LaunchProfile,
    ) -> Result<()> {
        let id = (self.next_id)();
        let commit = sessions::return_promoted_agent_to_shell(
            &mut self.connection,
            &command(id.clone(), "session.shell-promote-rollback", id),
            session_id,
            profile,
            now(),
        )?;
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(())
    }

    pub(crate) fn mark_shell_promotion_interrupted(&mut self, session_id: &str) -> Result<()> {
        let mut events = Vec::new();
        self.set_work_status(session_id, &mut events, |_| Some("interrupted"))
    }

    pub(crate) fn stop_for_permission_change(
        &mut self,
        session_id: &str,
    ) -> Result<Option<mpsc::Receiver<Result<()>>>> {
        self.shells
            .contains_key(session_id)
            .then(|| self.stop_for_provider_relaunch(session_id))
            .transpose()
    }

    pub(crate) fn permission_binding(&self, session_id: &str) -> Result<Option<ProviderBinding>> {
        let session = self
            .session(session_id)?
            .context(ACTIVE_PERMISSION_SESSION)?;
        ensure!(
            session.archived_at.is_none(),
            "archived Session permission mode cannot be modified"
        );
        let codex = session.kind == "codex";
        let antigravity = session.kind == "antigravity";
        ensure!(
            codex || antigravity || session.kind == "claude-code",
            "{ACTIVE_PERMISSION_SESSION}"
        );
        let shell = self.shells.get(session_id);
        ensure!(
            !shell.is_some_and(|shell| shell.retiring)
                && !self.waiting_for_provider_restore(session_id),
            "AI Session is already stopping or recovering"
        );
        let binding = if codex {
            self.codex_binding(session_id)?
        } else if antigravity {
            self.antigravity_binding(session_id)?
        } else {
            self.claude_binding(session_id)?
        };
        let Some(binding) = binding.filter(|binding| binding.resumable) else {
            if codex || antigravity {
                let shell = shell.context(ACTIVE_PERMISSION_SESSION)?;
                ensure!(
                    !shell.is_shell && !shell.ended && !shell.worker.is_finished(),
                    "{ACTIVE_PERMISSION_SESSION}"
                );
            } else {
                ensure!(
                    !shell.is_some_and(|shell| shell.is_shell),
                    "{ACTIVE_PERMISSION_SESSION}"
                );
            }
            let pending_fork = if codex {
                self.pending_codex_fork(session_id)?.is_some()
            } else if antigravity {
                self.pending_antigravity_fork(session_id)?.is_some()
            } else {
                self.pending_claude_fork(session_id)?.is_some()
            };
            ensure!(!pending_fork, "Card has an unconfirmed fork");
            return Ok(None);
        };
        let shell = shell.context(ACTIVE_PERMISSION_SESSION)?;
        ensure!(
            !shell.is_shell && !shell.ended && !shell.worker.is_finished(),
            "{ACTIVE_PERMISSION_SESSION}"
        );
        let identity = shell
            .hook
            .as_ref()
            .and_then(|hook| {
                if antigravity {
                    hook.antigravity.as_ref()?.run.conversation_id()
                } else {
                    hook.last_observation.as_ref()?.identity.as_deref()
                }
            })
            .context(ACTIVE_PERMISSION_SESSION)?;
        let same_identity = if codex {
            CodexResume::from_binding(&binding)
                .is_ok_and(|resume| Uuid::parse_str(identity) == Ok(resume.identity))
        } else {
            binding.provider_session_id == identity
        };
        ensure!(
            binding.restore_state == "none" && same_identity && binding.metadata.is_object(),
            "{ACTIVE_PERMISSION_SESSION}"
        );
        Ok(Some(binding))
    }

    pub(crate) fn write_provider_permission(
        &mut self,
        session_id: &str,
        binding_id: &str,
        permission: Option<&Value>,
    ) -> Result<()> {
        let id = (self.next_id)();
        let commit = sessions::write_provider_permission(
            &mut self.connection,
            &command(id.clone(), "session.set-permission-mode", id),
            session_id,
            binding_id,
            permission,
            now(),
        )?;
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(())
    }

    pub(crate) fn launch_claude_permission(
        &mut self,
        session_id: &str,
        terminal: matou_native_terminal::TerminalOptions,
        plan: impl Into<PreparedClaudeLaunch>,
        binding: Option<&ClaudeBinding>,
    ) -> Result<String> {
        let plan = plan.into();
        let resume = ResumeMonitor::for_permission_respawn(
            binding,
            plan.launch.permission_mode == "bypassPermissions",
        );
        self.launch_claude_with_monitor(
            session_id.to_owned(),
            terminal,
            plan,
            Some(resume),
            binding.and_then(|binding| binding.metadata.get("spawnRevision")),
        )?;
        Ok(self.shells[session_id].run_id.clone())
    }

    pub(crate) fn finish_provider_startup(&mut self, session_id: &str, run_id: &str) -> Result<()> {
        self.check_provider_startup(session_id, run_id)?;
        self.replay_provider_startup(session_id, run_id);
        Ok(())
    }

    pub(crate) fn begin_promotion_process_check(&mut self, session_id: &str) -> Result<()> {
        let result = Arc::new(OnceLock::new());
        self.send(
            session_id,
            WorkerCommand::CheckProcess {
                result: Arc::clone(&result),
            },
        )?;
        self.shells
            .get_mut(session_id)
            .unwrap()
            .promotion_process_check = Some(result);
        Ok(())
    }

    pub(crate) fn finish_promotion_startup(
        &mut self,
        session_id: &str,
        run_id: &str,
    ) -> Result<bool> {
        let check = self
            .shells
            .get(session_id)
            .filter(|shell| shell.run_id == run_id && !shell.retiring)
            .and_then(RunningShell::promotion_process_result);
        match check {
            Some(Ok(())) => {
                #[cfg(debug_assertions)]
                if let Some(marker) = std::env::var_os("MATOU_TEST_HOLD_VERIFIED_PROMOTION") {
                    let marker = Path::new(&marker);
                    if !marker.exists() {
                        std::fs::write(marker, run_id)?;
                    }
                    return Ok(false);
                }
                self.replay_provider_startup(session_id, run_id);
                Ok(true)
            }
            Some(Err(error)) => anyhow::bail!("{error:#}"),
            None => {
                self.check_provider_startup(session_id, run_id)?;
                Ok(false)
            }
        }
    }

    pub(crate) fn keep_promotion_at_shutdown(&mut self, session_id: &str, run_id: &str) -> bool {
        let Some(shell) = self
            .shells
            .get_mut(session_id)
            .filter(|shell| shell.run_id == run_id)
        else {
            return false;
        };
        if shell.promotion_verified() {
            return true;
        }
        shell.promotion_process_check = None;
        false
    }

    fn replay_provider_startup(&mut self, session_id: &str, run_id: &str) {
        let mut events = Vec::new();
        self.apply_startup_observations(session_id, run_id, &mut events);
        self.pending.extend(events);
    }

    fn apply_startup_observations(
        &mut self,
        session_id: &str,
        run_id: &str,
        events: &mut Vec<OwnerEvent>,
    ) {
        let Some(shell) = self
            .shells
            .get_mut(session_id)
            .filter(|shell| shell.run_id == run_id && !shell.ended && !shell.retiring)
        else {
            return;
        };
        shell.provider_startup_pending = false;
        if let Some(hook) = &mut shell.hook {
            let observations = std::mem::take(&mut hook.pending_startup_observations);
            let dsh = std::mem::take(&mut hook.dsh_pending);
            let antigravity = hook
                .antigravity
                .as_mut()
                .map(|run| std::mem::take(&mut run.startup))
                .unwrap_or_default();
            for (scope, observation, received_at) in observations {
                self.apply_hook_at(scope, observation, received_at, events);
            }
            for (scope, payload) in dsh {
                self.apply_dsh_hook(scope, &payload, events);
            }
            for (scope, source, payload, received_at) in antigravity {
                self.apply_antigravity_hook(scope, source, payload, received_at, events);
            }
        }
    }

    pub(crate) fn check_provider_startup(&self, session_id: &str, run_id: &str) -> Result<bool> {
        let shell = self
            .shells
            .get(session_id)
            .filter(|shell| shell.run_id == run_id && !shell.retiring)
            .context("AI Session worker is no longer active")?;
        if shell.ended || shell.worker.is_finished() {
            if let Some(error) = &shell.exit_error {
                anyhow::bail!("{error}");
            }
            let exit_code: Option<i64> = self.connection.query_row(
                "SELECT exit_code FROM session_runs WHERE id = ?",
                [run_id],
                |row| row.get(0),
            )?;
            match exit_code {
                Some(code) => anyhow::bail!("process exited with code {code}"),
                None => anyhow::bail!("AI Session worker exited"),
            }
        }
        if let Some(error) = shell
            .hook
            .as_ref()
            .and_then(|hook| hook.permission_identity_error.as_ref())
        {
            anyhow::bail!("{error}");
        }
        Ok(shell.emitted_output)
    }

    pub(crate) fn waiting_for_permission_consent(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .filter(|shell| !shell.provider_startup_pending && !shell.ended && !shell.retiring)
            .and_then(|shell| shell.hook.as_ref())
            .filter(|hook| hook.last_observation.is_none())
            .and_then(|hook| hook.resume.as_ref())
            .is_some_and(ResumeMonitor::waiting_for_bypass_consent)
    }

    pub(crate) fn send_permission_consent_input(
        &mut self,
        session_id: &str,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let submitted = matches!(bytes.as_slice(), b"\r" | b"\n" | b"\x1b");
        let arrow = matches!(bytes.as_slice(), [0x1b, b'[' | b'O', b'A'..=b'D']);
        if !(submitted || arrow) || !self.waiting_for_permission_consent(session_id) {
            return Ok(());
        }
        let shell = self.shells.get_mut(session_id).expect("CLI consent run");
        let (reply, _) = mpsc::channel();
        shell.worker.send(WorkerCommand::Write {
            bytes,
            origin: InputOrigin::User,
            reply,
        })?;
        if submitted {
            let hook = shell.hook.as_mut().unwrap();
            hook.resume.as_mut().unwrap().submit_bypass_consent();
            hook.deadline = Some(Instant::now() + IDENTITY_TIMEOUT);
        }
        Ok(())
    }

    pub(crate) fn already_loaded_catalog_identity(
        &self,
        session_id: &str,
        provider_session_id: &str,
    ) -> Result<Option<Value>> {
        let Some(shell) = self.shells.get(session_id).filter(|shell| {
            !shell.ended
                && !shell.retiring
                && shell.hook.as_ref().is_some_and(|hook| {
                    hook.last_observation
                        .as_ref()
                        .and_then(|observation| observation.identity.as_deref())
                        == Some(provider_session_id)
                })
        }) else {
            return Ok(None);
        };
        Ok(self
            .claude_binding(session_id)?
            .filter(|binding| {
                binding.resumable && binding.provider_session_id == provider_session_id
            })
            .map(|binding| {
                json!({
                    "result": "confirmed",
                    "reason": "already-loaded",
                    "sessionId": session_id,
                    "runId": shell.run_id,
                    "provider": "claude-code",
                    "providerSessionId": provider_session_id,
                    "bindingId": binding.id,
                    "restoreState": binding.restore_state,
                    "restoreError": null,
                    "spawnRevision": binding.metadata["spawnRevision"],
                })
            }))
    }

    /// Resolve environment and geometry before calling. Startup handshaking stays
    /// on the runtime thread; subsequent session requests are forwarded without waiting.
    pub fn launch(
        &mut self,
        options: SessionOptions,
        restore_history: bool,
        divider: &str,
    ) -> Result<()> {
        ensure!(
            options.launch.profile == LaunchProfile::Shell,
            "Provider launch requires a hook registration"
        );
        self.launch_run(options, restore_history, divider, None, None, None)
    }

    pub(crate) fn launch_claude(
        &mut self,
        session_id: String,
        terminal: matou_native_terminal::TerminalOptions,
        plan: impl Into<PreparedClaudeLaunch>,
        binding: Option<&ClaudeBinding>,
    ) -> Result<()> {
        let resume = binding
            .filter(|binding| binding.resumable)
            .map(ResumeMonitor::new);
        self.launch_claude_with_monitor(
            session_id,
            terminal,
            plan.into(),
            resume,
            binding.and_then(|binding| binding.metadata.get("spawnRevision")),
        )
    }

    pub(crate) fn launch_claude_restore(
        &mut self,
        session_id: String,
        terminal: matou_native_terminal::TerminalOptions,
        mut plan: PreparedClaudeLaunch,
        binding: &ClaudeBinding,
        rebuild: Option<(String, String)>,
    ) -> Result<()> {
        let mut monitor = ResumeMonitor::new(binding);
        let fallback = if let Some((parent, child)) = rebuild {
            ensure!(
                child == binding.provider_session_id,
                "Fork rebuild binding changed"
            );
            ensure!(
                provider_resume::empty_fork_parent(
                    &self.connection,
                    &session_id,
                    binding,
                    SESSION_NOT_FOUND
                )?
                .as_deref()
                    == Some(parent.as_str()),
                "Fork rebuild no longer owns an empty child binding"
            );
            if let Some(error) = provider_resume::fork_rebuild_blocker(
                claude_projects(&plan.launch.launch).as_deref(),
                &parent,
                &child,
            ) {
                self.change_claude_restore(
                    &session_id,
                    ClaudeRestore::Fail {
                        binding_id: &binding.id,
                        error: &error,
                    },
                )?;
                anyhow::bail!("{error}");
            }
            monitor.rebuild_parent = Some(parent);
            None
        } else if let Some(parent) = provider_resume::empty_fork_parent(
            &self.connection,
            &session_id,
            binding,
            SESSION_NOT_FOUND,
        )? {
            let child = &binding.provider_session_id;
            let launch = plan.launch.for_fork_rebuild(&parent, child)?;
            if provider_resume::fork_rebuild_blocker(
                claude_projects(&plan.launch.launch).as_deref(),
                &parent,
                child,
            )
            .is_none()
            {
                self.change_claude_restore(&session_id, ClaudeRestore::Begin(&binding.id))?;
                plan.launch = launch;
                monitor.rebuild_parent = Some(parent);
                None
            } else {
                // A transcript may appear while the ordinary resume starts.
                let subscriptions =
                    plan.subscriptions
                        .as_ref()
                        .map(|subscriptions| SessionSubscriptions {
                            service: subscriptions.service.clone(),
                            targets: subscriptions.targets.clone(),
                        });
                Some(ClaudeForkRebuild {
                    parent,
                    child: child.clone(),
                    plan: PreparedClaudeLaunch {
                        launch,
                        subscriptions,
                    },
                })
            }
        } else {
            None
        };
        self.launch_claude_with_monitor(
            session_id.clone(),
            terminal,
            plan,
            Some(monitor),
            binding.metadata.get("spawnRevision"),
        )?;
        self.shells
            .get_mut(&session_id)
            .unwrap()
            .hook
            .as_mut()
            .unwrap()
            .claude_fork_rebuild = fallback;
        Ok(())
    }

    pub(crate) fn launch_promoted_claude(
        &mut self,
        session_id: &str,
        terminal: matou_native_terminal::TerminalOptions,
        plan: impl Into<PreparedClaudeLaunch>,
    ) -> Result<String> {
        self.launch_claude(session_id.to_owned(), terminal, plan, None)?;
        let shell = self.shells.get_mut(session_id).unwrap();
        shell.provider_startup_pending = true;
        Ok(shell.run_id.clone())
    }

    pub(crate) fn launch_codex(
        &mut self,
        session_id: &str,
        terminal: matou_native_terminal::TerminalOptions,
        plan: CodexLaunch,
        resume: Option<CodexResume>,
        promotion: bool,
    ) -> Result<String> {
        let mut launch = plan.launch;
        launch.cols = terminal.cols;
        launch.rows = terminal.rows;
        let metadata = json!({
            "provider": "codex",
            "providerConfigId": plan.provider_config_id,
            "model": plan.model,
            "permissionMode": plan.permission_mode,
            "cwd": launch.cwd,
        });
        let secrets = launch
            .env
            .get(std::ffi::OsStr::new("OPENAI_API_KEY"))
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .into_iter()
            .collect();
        self.launch_run(
            SessionOptions {
                session_id: session_id.to_owned(),
                terminal,
                launch,
                journal: Default::default(),
                restored_history: Vec::new(),
                screen_recovery: None,
                collect_history: false,
            },
            false,
            "",
            Some((metadata, secrets, None, None, None, None)),
            resume,
            None,
        )?;
        if let Some(warning) = plan.permission_warning {
            diagnostic(&mut self.pending, session_id, warning.into());
        }
        let shell = self.shells.get_mut(session_id).unwrap();
        shell.provider_startup_pending = promotion;
        Ok(shell.run_id.clone())
    }

    pub(crate) fn launch_claude_fork(
        &mut self,
        session_id: String,
        terminal: matou_native_terminal::TerminalOptions,
        plan: impl Into<PreparedClaudeLaunch>,
        source_id: String,
    ) -> Result<()> {
        ensure!(
            self.pending_claude_fork(&session_id)?
                .is_some_and(|fork| fork.source_id == source_id),
            "Fork launch no longer owns its pending operation"
        );
        let monitor = ResumeMonitor::for_id(source_id);
        self.launch_claude_with_monitor(session_id, terminal, plan.into(), Some(monitor), None)
    }

    fn launch_claude_with_monitor(
        &mut self,
        session_id: String,
        terminal: matou_native_terminal::TerminalOptions,
        prepared: PreparedClaudeLaunch,
        resume: Option<ResumeMonitor>,
        revision: Option<&Value>,
    ) -> Result<()> {
        let PreparedClaudeLaunch {
            launch: plan,
            subscriptions,
        } = prepared;
        let mut launch = plan.launch;
        launch.cols = terminal.cols;
        launch.rows = terminal.rows;
        if let Some(gateway) = &plan.gateway
            && gateway.subscription_providers().next().is_some()
        {
            ensure!(
                subscriptions.is_some(),
                LaunchIssue::ServiceNotStarted.localized(sessions::current_interface_locale(
                    sessions::read_locale_preference(&launch.data_root)
                ))
            );
        }
        let gateway = plan.gateway;
        let mut secrets = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
            .into_iter()
            .filter_map(|name| {
                launch
                    .env
                    .iter()
                    .find(|(key, _)| {
                        key.to_str()
                            .is_some_and(|key| key.eq_ignore_ascii_case(name))
                    })
                    .and_then(|(_, value)| value.to_str())
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        if let Some(gateway) = &gateway {
            secrets.extend(
                gateway
                    .connections()
                    .filter(|connection| !connection.api_key.is_empty())
                    .map(|connection| connection.api_key.clone()),
            );
        }
        let safe = |value: &str| safe_hook_text(value, &secrets).unwrap_or("").to_owned();
        let mut metadata = json!({
            "providerConfigId": safe(&plan.provider_config_id), "model": safe(&plan.model),
            "permissionMode": safe(&plan.permission_mode), "cwd": safe(&launch.cwd.to_string_lossy()),
        });
        for (name, value) in [
            ("assignedRole", plan.assigned_role),
            (
                "effort",
                plan.effort
                    .as_deref()
                    .filter(|level| EFFORTS.contains(level)),
            ),
        ] {
            if let Some(value) = value.and_then(|value| safe_hook_text(value, &secrets)) {
                metadata[name] = json!(value);
            }
        }
        if let Some(revision) = revision {
            metadata["spawnRevision"] = revision.clone();
        }
        self.launch_run(
            SessionOptions {
                session_id,
                terminal,
                launch,
                journal: Default::default(),
                restored_history: Vec::new(),
                screen_recovery: None,
                collect_history: false,
            },
            false,
            "",
            Some((
                metadata,
                secrets,
                resume,
                gateway.map(|projection| (projection, plan.gateway_refresh)),
                subscriptions,
                plan.subscription_settings,
            )),
            None,
            None,
        )
    }

    pub(crate) fn launch_dsh(
        &mut self,
        session_id: &str,
        terminal: matou_native_terminal::TerminalOptions,
        data_root: &Path,
        prepared: DshLaunch,
        promotion: bool,
    ) -> Result<String> {
        let DshLaunch {
            plan,
            config_dump,
            catalog_root,
            environment_handoff,
        } = prepared;
        let binding = if plan.mode.expected_session_id().is_some() {
            self.dsh_binding(session_id)?
        } else {
            None
        };
        let metadata = json!({
            "provider": "dsh", "cwd": plan.cwd,
            "spawnRevision": binding.as_ref().and_then(|binding| binding.metadata.get("spawnRevision")),
        });
        self.launch_run(
            SessionOptions {
                session_id: session_id.to_owned(),
                terminal,
                journal: Default::default(),
                restored_history: Vec::new(),
                screen_recovery: None,
                collect_history: false,
                launch: matou_native_core::pty::ShellLaunch {
                    profile: LaunchProfile::Dsh,
                    executable: plan.executable,
                    argv: os_arguments(plan.argv)?,
                    cwd: plan.cwd,
                    data_root: data_root.to_owned(),
                    inherited_env: Default::default(),
                    env: plan.environment,
                    cols: terminal.cols,
                    rows: terminal.rows,
                },
            },
            false,
            "",
            Some((metadata, Vec::new(), None, None, None, None)),
            None,
            Some(SpecialLaunch::Dsh {
                config_dump,
                catalog_root,
                binding,
                environment_handoff,
            }),
        )?;
        let shell = self.shells.get_mut(session_id).unwrap();
        shell.provider_startup_pending = promotion;
        Ok(shell.run_id.clone())
    }

    pub(crate) fn launch_antigravity(
        &mut self,
        session_id: &str,
        terminal: matou_native_terminal::TerminalOptions,
        data_root: &Path,
        plan: antigravity_launch::LaunchPlan,
        permission: AntigravityPermission,
        promotion: bool,
    ) -> Result<String> {
        let binding = match &plan.mode {
            AntigravityMode::Resume(expected) => {
                let binding = self
                    .antigravity_binding(session_id)?
                    .filter(|binding| binding.resumable && &binding.provider_session_id == expected)
                    .context("Antigravity restore binding changed")?;
                Some(binding)
            }
            AntigravityMode::Fork(parent) => {
                ensure!(
                    self.pending_antigravity_fork(session_id)?
                        .is_some_and(|fork| &fork.source_id == parent),
                    "Antigravity fork intent changed"
                );
                None
            }
            AntigravityMode::Fresh => None,
        };
        let metadata = json!({
            "provider": "antigravity", "cwd": plan.cwd,
            "permissionMode": permission.as_str(),
            "spawnRevision": binding.as_ref().and_then(|binding| binding.metadata.get("spawnRevision")),
        });
        self.launch_run(
            SessionOptions {
                session_id: session_id.to_owned(),
                terminal,
                journal: Default::default(),
                restored_history: Vec::new(),
                screen_recovery: None,
                collect_history: false,
                launch: matou_native_core::pty::ShellLaunch {
                    profile: LaunchProfile::Antigravity,
                    executable: plan.executable,
                    argv: os_arguments(plan.argv)?,
                    cwd: plan.cwd,
                    data_root: data_root.to_owned(),
                    inherited_env: Default::default(),
                    env: plan.environment,
                    cols: terminal.cols,
                    rows: terminal.rows,
                },
            },
            false,
            "",
            Some((metadata, Vec::new(), None, None, None, None)),
            None,
            Some(SpecialLaunch::Antigravity {
                mode: plan.mode,
                permission,
                binding: binding.map(|binding| binding.id),
            }),
        )?;
        let shell = self.shells.get_mut(session_id).unwrap();
        shell.provider_startup_pending = promotion;
        Ok(shell.run_id.clone())
    }

    pub(crate) fn launch_antigravity_permission(
        &mut self,
        session_id: &str,
        terminal: matou_native_terminal::TerminalOptions,
        data_root: &Path,
        plan: antigravity_launch::LaunchPlan,
        permission: AntigravityPermission,
    ) -> Result<String> {
        let binding = self.antigravity_binding(session_id)?;
        let awaiting_resume = matches!(&plan.mode, AntigravityMode::Resume(_)) && binding.is_some();
        let run_id = match self
            .launch_antigravity(session_id, terminal, data_root, plan, permission, false)
        {
            Ok(run_id) => run_id,
            Err(error) => {
                if let Some(binding) = binding {
                    self.change_antigravity_restore(
                        session_id,
                        ClaudeRestore::Fail {
                            binding_id: &binding.id,
                            error: "Antigravity permission restart could not start",
                        },
                    )?;
                }
                return Err(error);
            }
        };
        if awaiting_resume {
            self.shells
                .get_mut(session_id)
                .unwrap()
                .hook
                .as_mut()
                .unwrap()
                .deadline = Some(Instant::now() + IDENTITY_TIMEOUT);
        }
        Ok(run_id)
    }

    pub(crate) fn fresh_provider_permission(&self, session_id: &str) -> Option<&str> {
        if !self.environment_worker_active(session_id) {
            return None;
        }
        let hook = self.shells.get(session_id)?.hook.as_ref()?;
        let fresh = match hook.profile {
            LaunchProfile::Codex => hook.codex_resume.is_none() && hook.fork.is_none(),
            LaunchProfile::Antigravity => hook
                .antigravity
                .as_ref()
                .is_some_and(|agy| agy.binding.is_none() && !agy.fork_pending),
            _ => false,
        };
        if fresh {
            hook.launch_metadata["permissionMode"].as_str()
        } else {
            None
        }
    }

    fn screen_recovery_options(&self, session_id: &str) -> Result<ScreenRecoveryOptions> {
        let initial_grid = self
            .connection
            .query_row(
                "SELECT cols, rows FROM session_runs WHERE session_id = ? ORDER BY ordinal LIMIT 1",
                [session_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?
            .filter(|&(cols, rows)| cols >= 2 && rows > 0)
            .and_then(|(cols, rows)| Some((cols.try_into().ok()?, rows.try_into().ok()?)));
        let mut query = self.connection.prepare(
            "SELECT id, generation, terminal_sequence, domain_event_sequence,
                    screen_epoch, file_path, checksum
             FROM journal_checkpoints WHERE session_id = ? AND valid = 1
               AND min(generation, terminal_sequence, domain_event_sequence, screen_epoch) >= 0
             ORDER BY generation DESC",
        )?;
        let checkpoints = query
            .query_map([session_id], |row| {
                Ok(CheckpointCandidate {
                    id: row.get(0)?,
                    session_id: session_id.to_owned(),
                    generation: row.get::<_, i64>(1)? as u64,
                    terminal_sequence: row.get::<_, i64>(2)? as u64,
                    domain_event_sequence: row.get::<_, i64>(3)? as u64,
                    screen_epoch: row.get::<_, i64>(4)? as u64,
                    file_path: PathBuf::from(row.get::<_, String>(5)?),
                    checksum: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ScreenRecoveryOptions {
            initial_grid,
            checkpoints,
            transcript_path: None,
        })
    }

    fn launch_run(
        &mut self,
        mut options: SessionOptions,
        restore_history: bool,
        divider: &str,
        provider: Option<(
            Value,
            Vec<String>,
            Option<ResumeMonitor>,
            Option<(ClaudeGatewayProjection, Option<ClaudeGatewayRefresh>)>,
            Option<SessionSubscriptions>,
            Option<Value>,
        )>,
        codex_resume: Option<CodexResume>,
        special: Option<SpecialLaunch>,
    ) -> Result<()> {
        ensure!(!self.closing, "Runtime is shutting down");
        let profile = options.launch.profile;
        // Classify the upstream before a managed gateway replaces its endpoint
        // with our own loopback URL.
        let launch_environment = options
            .launch
            .inherited_env
            .iter()
            .chain(&options.launch.env)
            .filter_map(|(key, value)| Some((key.to_str()?.to_owned(), value.to_str()?.to_owned())))
            .collect();
        let egress_families = launch_egress_families(
            profile,
            &launch_environment,
            provider
                .as_ref()
                .and_then(|p| p.3.as_ref().map(|(projection, _)| projection)),
        );
        for &family in &egress_families {
            self.egress
                .assert_target_allowed(Some(family))
                .map_err(anyhow::Error::msg)?;
        }
        if let Some(previous) = self.shells.get(&options.session_id) {
            ensure!(
                previous.ended && !previous.retiring,
                "Shell worker already exists or is retiring"
            );
        }
        if let Some(mut previous) = self.shells.remove(&options.session_id) {
            if let Some(control) = &self.run_control {
                control.revoke(&previous.run_id);
            }
            previous.gateway.take();
            previous.worker.shutdown()?;
        }
        let stored = self
            .session(&options.session_id)?
            .context("read the authoritative session")?;
        ensure!(
            stored.kind == profile.kind(),
            "launch profile differs from its stored session"
        );
        ensure!(
            stored.archived_at.is_none(),
            "archived Session cannot start a new run"
        );
        ensure!(
            Path::new(&stored.cwd) == options.launch.cwd,
            "{} launch cwd differs from its stored session",
            profile.kind()
        );
        let is_shell = profile == LaunchProfile::Shell;
        let restore_history = is_shell && restore_history;
        options.collect_history = restore_history;
        options.restored_history = if is_shell {
            let blocks = shell_history::list_for_launch(
                &self.connection,
                &options.session_id,
                restore_history,
            )?;
            shell_history::format_shell_history_for_terminal(&blocks, divider).into_bytes()
        } else {
            Vec::new()
        };
        if !is_shell {
            options.screen_recovery = Some(self.screen_recovery_options(&options.session_id)?);
            crate::checkpoint_sync::protect_existing(&self.connection, &mut options)?;
        }
        let session_id = options.session_id.clone();
        let cols = options.launch.cols.into();
        let rows = options.launch.rows.into();
        let run_id = match &special {
            Some(SpecialLaunch::CodexFork { authority, .. }) => authority.run_id.to_string(),
            _ => (self.next_id)(),
        };
        let mut permission_starting = false;
        let mut gateway = None;
        let mut agent_status_directory = None;
        let roles_path = options
            .launch
            .data_root
            .join(matou_native_core::dispatch_roles::ROLES_FILENAME);
        let hook = if let Some((
            launch_metadata,
            mut secrets,
            resume,
            gateway_plan,
            subscriptions,
            subscription_settings,
        )) = provider
        {
            let default_claude_login = profile == LaunchProfile::ClaudeCode
                && gateway_plan.is_none()
                && uses_default_claude_login(
                    launch_metadata["providerConfigId"].as_str(),
                    &options.launch,
                    self.provider_environment.as_ref().map_or_else(
                        || {
                            std::env::var_os("ANTHROPIC_BASE_URL")
                                .is_some_and(|url| !url.is_empty())
                        },
                        |env| get_var(env, "ANTHROPIC_BASE_URL").is_some_and(|url| !url.is_empty()),
                    ),
                );
            let resume = resume.or_else(|| {
                matches!(
                    profile,
                    LaunchProfile::ClaudeCode | LaunchProfile::Antigravity
                )
                .then(|| ResumeMonitor::for_fresh_trust(profile))
            });
            permission_starting = resume
                .as_ref()
                .is_some_and(|resume| resume.permission_respawn);
            let (mut expected_id, mut fork_source_id) = match &resume {
                Some(resume) if resume.binding_id.is_some() => (resume.expected_id.clone(), None),
                Some(resume) => (None, resume.expected_id.clone()),
                None => (None, None),
            };
            match &special {
                Some(SpecialLaunch::Antigravity {
                    mode: AntigravityMode::Resume(id),
                    ..
                }) => expected_id = Some(id.clone()),
                Some(SpecialLaunch::Antigravity {
                    mode: AntigravityMode::Fork(id),
                    ..
                }) => fork_source_id = Some(id.clone()),
                _ => {}
            }
            let identity_wait = if fork_source_id.is_some() {
                FORK_IDENTITY_WAIT
            } else {
                IDENTITY_TIMEOUT
            };
            let claude_projects = (profile == LaunchProfile::ClaudeCode)
                .then(|| claude_projects(&options.launch))
                .flatten();
            if profile == LaunchProfile::ClaudeCode
                && let (Some(root), Some(identity), Some(recovery)) = (
                    claude_projects.as_deref(),
                    expected_id.as_deref(),
                    options.screen_recovery.as_mut(),
                )
            {
                recovery.transcript_path =
                    crate::provider_catalog::resume_transcript(root, &options.launch.cwd, identity);
            }
            let hud_provider = match profile {
                LaunchProfile::ClaudeCode => HudProvider::ClaudeCode,
                LaunchProfile::Codex => HudProvider::Codex,
                LaunchProfile::Dsh => HudProvider::Dsh,
                LaunchProfile::Antigravity => HudProvider::Antigravity,
                LaunchProfile::Shell => unreachable!(),
            };
            let mut hud = AgentHudProjection::new(&session_id, hud_provider, now() as f64);
            hud.seed_settings(
                options.launch.cwd.to_str(),
                launch_metadata["permissionMode"].as_str(),
                None,
                launch_metadata["model"].as_str(),
                false,
            );
            let mut seeded_transcript = None;
            if let (Some(plan), Some(root), Some(identity)) = (
                gateway_plan.as_ref().map(|(projection, _)| projection),
                claude_projects.as_deref(),
                expected_id.as_deref(),
            ) && plan
                .connections()
                .any(|connection| connection.endpoint.host_str() == Some("api.deepseek.com"))
                && let Some(path) =
                    crate::provider_catalog::resume_transcript(root, &options.launch.cwd, identity)
                && let Some(rows) = read_hud_transcript(root, &path)
            {
                hud.seed_transcript_context(
                    rows.iter()
                        .filter(|row| row["sessionId"].as_str().is_none_or(|id| id == identity)),
                );
                seeded_transcript = Some(path);
            }
            let mut gateway_context = None;
            // The listener belongs to this owner generation, this session_id
            // and this run_id. Publish the environment only after bind succeeds.
            let gateway_settings = if let Some((plan, refresh)) = gateway_plan {
                let plan = Arc::new(plan);
                let context = Arc::new(Mutex::new(hud.context_usage()));
                let read_context = Arc::clone(&context);
                let listener = Gateway::start_with_context(
                    Arc::clone(&plan),
                    refresh.map(|context| (context, options.launch.data_root.clone())),
                    Some(Arc::new(move || {
                        read_context.lock().ok().map(|usage| *usage)
                    })),
                    subscriptions,
                    Arc::clone(&self.blocked_gateway_families),
                    sessions::current_interface_locale(sessions::read_locale_preference(
                        &options.launch.data_root,
                    )),
                    ProxyEnvironment::capture(&options.launch.inherited_env, &options.launch.env),
                )?;
                gateway_context = Some(context);
                let env = plan.project_environment(&mut options.launch, listener.base_url());
                secrets.push(listener.base_url().to_owned());
                gateway = Some(listener);
                Some((env, plan.model_picker(), subscription_settings))
            } else {
                None
            };
            // Configure hooks before the provider subcommand, as for Codex resume.
            let fork_argv = matches!(&special, Some(SpecialLaunch::CodexFork { .. })).then(|| {
                let end = options.launch.argv.len();
                options.launch.argv.split_off(end - 2)
            });
            let control_asset_root = std::env::var_os("MATOU_CONTROL_ASSET_ROOT");
            let current_executable = std::env::current_exe().ok();
            let foreman_available = crate::provider_hook::bundled_foreman_script(
                current_executable.as_deref(),
                control_asset_root.as_deref().map(Path::new),
            )
            .is_some();
            let roles_markdown = foreman_available.then(|| {
                let config = DispatchRolesStore::for_data_root(&options.launch.data_root)
                    .snapshot()
                    .map(|snapshot| snapshot.config)
                    .unwrap_or_default();
                format_roles_markdown(&config)
            });
            let mut registration = self
                .register_hook(
                    &mut options,
                    &run_id,
                    expected_id,
                    fork_source_id,
                    foreman_available,
                    roles_markdown.as_deref(),
                )
                .context(HookSetup)?;
            let mut dsh = None;
            let mut antigravity = None;
            let mut antigravity_config = Vec::new();
            let mut fork = None;
            match special {
                Some(SpecialLaunch::Antigravity {
                    mode,
                    permission,
                    binding,
                }) => {
                    let home = get_var(&options.launch.env, "HOME").filter(|home| !home.is_empty());
                    #[cfg(windows)]
                    let home = home.or_else(|| {
                        get_var(&options.launch.env, "USERPROFILE").filter(|home| !home.is_empty())
                    });
                    let home = home.context("Antigravity shell home is missing")?;
                    let catalog_root = crate::antigravity_catalog::catalog_root(
                        Path::new(home),
                        &options.launch.env,
                    );
                    let native_config = get_var(&options.launch.env, "ANTIGRAVITY_HOME")
                        .filter(|path| !path.is_empty())
                        .map(|path| options.launch.cwd.join(path));
                    antigravity_config.extend(native_config);
                    antigravity_config.extend([
                        Path::new(home).join(".gemini/antigravity"),
                        Path::new(home).join(".gemini/config"),
                        catalog_root.clone(),
                        options.launch.cwd.join(".gemini"),
                    ]);
                    let system_message_file = registration
                        .attach_antigravity_instructions(
                            foreman_available,
                            roles_markdown.as_deref(),
                        )
                        .context("Prepare Antigravity system instructions")?;
                    let run_environment =
                        registration.antigravity_environment(Some(&system_message_file));
                    secrets.extend(
                        run_environment
                            .iter()
                            .filter(|(key, _)| {
                                key.as_os_str() != "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE"
                            })
                            .filter_map(|(_, value)| value.to_str().map(str::to_owned)),
                    );
                    registration
                        .configure_antigravity_args(&mut options.launch.argv)
                        .context("Prepare Antigravity session settings")?;
                    options.launch.env.extend(run_environment);
                    antigravity = Some(AntigravityObservation {
                        run: antigravity_events::Run::new(&run_id, mode.clone(), permission),
                        binding,
                        catalog_root,
                        transcript: LiveTranscript::default(),
                        transcript_path: None,
                        transcript_poll: Instant::now(),
                        title_check: None,
                        fork_pending: matches!(mode, AntigravityMode::Fork(_)),
                        startup: Vec::new(),
                    });
                }
                Some(SpecialLaunch::Dsh {
                    config_dump,
                    catalog_root,
                    binding,
                    environment_handoff,
                }) => {
                    let patch = registration.attach_dsh(
                        &config_dump,
                        &run_id,
                        foreman_available,
                        roles_markdown.as_deref(),
                    )?;
                    let expected = binding
                        .as_ref()
                        .map(|binding| binding.provider_session_id.as_str());
                    if let Some(binding) = &binding {
                        let mut resume = DshResume::new(&binding.provider_session_id);
                        let outcome = if environment_handoff {
                            resume.start_for_environment(&run_id)
                        } else {
                            let mut catalog = DshSessionCatalog::new(
                                catalog_root.context("DSH history directory unavailable")?,
                            );
                            resume.start(&run_id, &mut catalog, &options.launch.cwd)
                        };
                        ensure!(
                            matches!(outcome, BeginOutcome::Launch(_)),
                            "DSH history could not be restored: {:?}",
                            resume.state()
                        );
                        dsh = Some(DshObservation::Resume(resume, binding.id.clone()));
                    } else {
                        dsh = Some(DshObservation::Fresh(DshEvents::new(&run_id, None)));
                    }
                    let plan = dsh_launch::prepare(DshLaunchInput {
                        cwd: &options.launch.cwd,
                        patch_path: Some(&patch),
                        resume_session_id: expected,
                        inherited_environment: std::mem::take(&mut options.launch.env),
                        run_environment: Default::default(),
                    })?;
                    options.launch.argv = os_arguments(plan.argv)?;
                    options.launch.env = plan.environment;
                }
                Some(SpecialLaunch::CodexFork { key, authority }) => fork = Some((key, authority)),
                None => {}
            }
            let context_window = get_var(&options.launch.env, "CLAUDE_CODE_MAX_CONTEXT_TOKENS")
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<u64>().ok());
            if profile == LaunchProfile::ClaudeCode
                && (gateway_settings.is_some() || context_window.is_some())
            {
                let path = &registration.settings_path;
                let mut settings: Value = serde_json::from_slice(&std::fs::read(path)?)
                    .map_err(|_| anyhow::anyhow!("Read Claude launch settings"))?;
                if let Some((env, model_picker, subscription_settings)) = gateway_settings {
                    if let Some(subscription) =
                        subscription_settings.as_ref().and_then(Value::as_object)
                    {
                        settings
                            .as_object_mut()
                            .expect("Claude run settings object")
                            .extend(subscription.clone());
                    }
                    if let Some(fable_alias) = env["ANTHROPIC_DEFAULT_FABLE_MODEL"].as_str() {
                        settings["modelOverrides"] = json!({
                            "claude-fable-5": fable_alias,
                            "claude-fable-5-2": fable_alias,
                            "claude-fable-5-5": fable_alias,
                        });
                    }
                    let settings_env = settings["env"].as_object_mut();
                    if let (Some(settings_env), Some(env)) = (settings_env, env.as_object()) {
                        settings_env.extend(env.clone());
                    } else {
                        settings["env"] = env;
                    }
                    if let Some(model_picker) = model_picker {
                        settings["modelPicker"] = model_picker;
                    }
                }
                if let Some(window) = context_window {
                    if settings["autoCompactWindow"].is_null() {
                        settings["autoCompactWindow"] = json!(window);
                    }
                    if !settings["env"].is_object() {
                        settings["env"] = json!({});
                    }
                    let settings_env = settings["env"]
                        .as_object_mut()
                        .expect("Claude settings env");
                    for key in [
                        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
                        "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
                        "CLAUDE_CODE_DISABLE_1M_CONTEXT",
                    ] {
                        if let Some(value) =
                            get_var(&options.launch.env, key).and_then(|value| value.to_str())
                        {
                            settings_env
                                .entry(key.to_owned())
                                .or_insert_with(|| json!(value));
                        }
                    }
                }
                std::fs::write(path, serde_json::to_vec(&settings)?)
                    .context("Write Claude launch settings")?;
            }
            let deadline = (profile == LaunchProfile::ClaudeCode
                || codex_resume.is_some()
                || matches!(dsh, Some(DshObservation::Resume(..)))
                || antigravity.as_ref().is_some_and(|agy| agy.fork_pending)
                || fork.is_some())
            .then(|| {
                Instant::now()
                    + if fork.is_some() || antigravity.as_ref().is_some_and(|agy| agy.fork_pending)
                    {
                        FORK_IDENTITY_WAIT
                    } else {
                        identity_wait
                    }
            });
            if matches!(
                Path::new(&session_id)
                    .components()
                    .collect::<Vec<_>>()
                    .as_slice(),
                [std::path::Component::Normal(_)]
            ) {
                let directory = options
                    .launch
                    .data_root
                    .join("agent-status")
                    .join(&session_id);
                if std::fs::create_dir_all(&directory).is_ok()
                    && directory.parent().is_some_and(|parent| {
                        std::fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.is_dir())
                    })
                    && std::fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.is_dir())
                {
                    agent_status_directory = Some(directory);
                }
            }
            if profile == LaunchProfile::Codex {
                let subcommand = fork_argv.or_else(|| {
                    codex_resume
                        .as_ref()
                        .map(|resume| vec!["resume".into(), resume.identity.to_string()])
                });
                crate::codex_launch::finish_argv(
                    &mut options.launch.argv,
                    agent_status_directory.as_deref(),
                    &roles_path,
                    subcommand,
                );
            }
            Some(RunHook {
                _registration: registration,
                profile,
                codex_resume,
                codex: (profile == LaunchProfile::Codex)
                    .then(|| codex_events::Run::new(&options.launch)),
                launch_metadata,
                secrets,
                uses_gateway: gateway.is_some(),
                default_claude_login,
                deadline,
                last_observation: None,
                idle_while_running_since: None,
                last_submission: None,
                resume,
                claude_fork_rebuild: None,
                reported_missing: false,
                pending_startup_observations: Vec::new(),
                hud,
                context_guard: SessionContextGuard::new(hud_provider),
                context_card_alert: None,
                rescue_ready: false,
                context_session_ended: false,
                dsh_compacted_percent: None,
                codex_context_tools: Vec::new(),
                codex_context_last_tool: None,
                optimizer_data_root: options.launch.data_root.clone(),
                codex_context_percent: None,
                gateway_context,
                external_agents: agent_status_directory
                    .clone()
                    .map(|path| (path, ExternalAgents::default())),
                antigravity_config,
                hud_config: None,
                git_status: None,
                claude_projects,
                claude_transcript_poll: seeded_transcript.as_ref().map(|_| Instant::now()),
                claude_transcript: seeded_transcript,
                claude_live: LiveTranscript::default(),
                claude_waiting_since_ms: None,
                claude_transcript_activity_at_ms: None,
                claude_observed_title: None,
                dsh,
                antigravity,
                dsh_pending: Vec::new(),
                fork,
                codex_fork_ready: false,
                dsh_status: None,
                permission_identity_error: None,
            })
        } else {
            None
        };
        clear_control_environment(&mut options.launch);
        if hook.is_some() {
            set_var(
                &mut options.launch.env,
                "MATOU_DATA_ROOT".into(),
                options.launch.data_root.clone().into_os_string(),
            );
            set_var(
                &mut options.launch.env,
                "MATOU_SESSION_ID".into(),
                session_id.clone().into(),
            );
        }
        options.launch.env.insert(
            "MATOU_FOREMAN_ROLES_FILE".into(),
            roles_path.into_os_string(),
        );
        if let Some(directory) = agent_status_directory {
            options
                .launch
                .env
                .insert("MATOU_AGENT_STATUS_DIR".into(), directory.into_os_string());
        }
        if let Some(control) = &self.run_control {
            match control.inject(&mut options.launch, &session_id, &run_id) {
                Ok(()) => {
                    if let Some(bin) = &self.control_bin {
                        let launch = &mut options.launch;
                        let path = get_var(&launch.env, "PATH")
                            .or_else(|| get_var(&launch.inherited_env, "PATH"));
                        let entries = path.into_iter().flat_map(std::env::split_paths);
                        match std::env::join_paths(std::iter::once(bin.clone()).chain(entries)) {
                            Ok(path) => {
                                set_var(&mut launch.env, "PATH".into(), path);
                            }
                            Err(error) => {
                                eprintln!("[host-control] could not prepend mt to PATH: {error}")
                            }
                        }
                    }
                    let launch = &mut options.launch;
                    if get_var(&launch.env, "MATOU_CONTROL_NODE_EXECUTABLE").is_none() {
                        let mut environment = launch.inherited_env.clone();
                        for (key, value) in &launch.env {
                            set_var(&mut environment, key.clone(), value.clone());
                        }
                        if let Some(node) = resolve_control_node(&launch.cwd, &environment) {
                            set_var(
                                &mut launch.env,
                                "MATOU_CONTROL_NODE_EXECUTABLE".into(),
                                node.into_os_string(),
                            );
                        }
                    }
                }
                Err(error) => eprintln!("[host-control] {error:#}"),
            }
        }
        #[cfg(any(unix, windows))]
        let alias_input = is_shell.then(|| ShellInput::with_shell_environment(&options.launch));
        let mut worker = match SessionWorker::spawn(options, self.send.clone(), run_id.clone()) {
            Ok(worker) => worker,
            Err(error) => {
                if let Some(control) = &self.run_control {
                    control.revoke(&run_id);
                }
                return Err(error);
            }
        };
        #[cfg(any(unix, windows))]
        if let Some(input) = alias_input.clone() {
            worker.restore_shell_input(input);
        }
        let time = now();
        let commit = sessions::start_run(
            &mut self.connection,
            &command(
                format!("runtime-start-{run_id}"),
                "session.run-start",
                format!("start:{run_id}:{session_id}:{}", profile.kind()),
            ),
            &StartRunInput {
                id: run_id.clone(),
                session_id: session_id.clone(),
                runtime_generation: self.generation.clone(),
                pid: Some(worker.process_id().into()),
                cols,
                rows,
                now: time,
            },
            time,
        );
        let commit = match commit {
            Ok(commit) => commit,
            Err(error) => {
                if let Some(control) = &self.run_control {
                    control.revoke(&run_id);
                }
                drop(gateway);
                let _ = worker.shutdown();
                return Err(error);
            }
        };
        if !egress_families.is_empty() {
            self.egress
                .register_session(session_id.clone(), &egress_families, time as u64);
            self.egress_membership_changed = true;
        }
        if hook.is_some() {
            self.summaries.remove(&session_id);
        }
        self.shells.insert(
            session_id,
            RunningShell {
                worker,
                run_id,
                cwd: stored.cwd,
                is_shell,
                #[cfg(any(unix, windows))]
                alias_preheat: alias_input,
                restore_history,
                cwd_tracker: CwdTracker::default(),
                work_tracker: WorkStatusTracker::default(),
                emitted_output: false,
                last_output: None,
                ended: false,
                exit_error: None,
                retiring: false,
                stop_completion: None,
                catalog_stop: false,
                environment_stop: false,
                provider_startup_pending: permission_starting,
                promotion_process_check: None,
                cwd_capture: None,
                hook,
                gateway,
            },
        );
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(())
    }

    fn register_hook(
        &mut self,
        options: &mut SessionOptions,
        run_id: &str,
        expected_id: Option<String>,
        fork_source_id: Option<String>,
        foreman_available: bool,
        roles_markdown: Option<&str>,
    ) -> Result<Registration> {
        if self.hook_server.is_none() {
            self.hook_server = Some(ProviderHookServer::start()?);
        }
        let registration = self.hook_server.as_ref().unwrap().register(
            &options.launch.data_root,
            Scope {
                profile: options.launch.profile,
                session_id: options.session_id.clone(),
                run_id: run_id.to_owned(),
                generation: self.generation.clone(),
                expected_id,
                fork_source_id,
            },
        )?;
        if options.launch.profile == LaunchProfile::Codex {
            registration.attach_codex(&mut options.launch, foreman_available, roles_markdown)?;
        } else if options.launch.profile == LaunchProfile::ClaudeCode {
            attach_claude_settings(&mut options.launch, &registration.settings_path)?;
            let control_asset_root = std::env::var_os("MATOU_CONTROL_ASSET_ROOT");
            let executable = std::env::current_exe().ok();
            if let Some(plugin) = crate::provider_hook::bundled_claude_plugin(
                executable.as_deref(),
                control_asset_root.as_deref().map(Path::new),
            ) {
                let path = plugin.to_str().context("Claude plugin path is not UTF-8")?;
                options
                    .launch
                    .argv
                    .splice(2..2, ["--plugin-dir".into(), path.to_owned()]);
            }
        }
        Ok(registration)
    }

    pub(crate) fn read_current_screen(
        &self,
        session_id: &str,
    ) -> Result<Option<mpsc::Receiver<Result<CurrentScreen>>>> {
        let Some(shell) = self
            .shells
            .get(session_id)
            .filter(|shell| !shell.retiring && !shell.ended && !shell.worker.is_finished())
        else {
            return Ok(None);
        };
        let (reply, result) = mpsc::channel();
        shell
            .worker
            .send(WorkerCommand::ReadCurrentScreen { reply })?;
        Ok(Some(result))
    }

    pub(crate) fn control_terminal(&self, session_id: &str) -> Option<(&str, Option<Instant>)> {
        self.shells
            .get(session_id)
            .filter(|shell| !shell.ended && !shell.retiring)
            .filter(|_| {
                self.connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ? AND archived_at IS NULL)",
                    [session_id], |row| row.get(0),
                ).unwrap_or(false)
            })
            .map(|shell| (shell.run_id.as_str(), shell.last_output))
    }

    pub(crate) fn control_storage(&mut self) -> &mut Connection {
        &mut self.connection
    }

    pub(crate) fn control_is_read_only(&self) -> Result<bool> {
        Ok(self.connection.is_readonly("main")?
            || self
                .connection
                .pragma_query_value(None, "query_only", |row| row.get(0))?)
    }

    pub fn send(&self, session_id: &str, command: WorkerCommand) -> Result<()> {
        self.shells
            .get(session_id)
            .filter(|shell| !shell.retiring)
            .context("Shell worker does not exist")?
            .worker
            .send(command)
    }

    pub(crate) fn recovery_history_gap(
        &self,
        session_id: &str,
    ) -> Option<crate::history::HistoryGapCode> {
        self.shells.get(session_id)?.worker.recovery_history_gap
    }

    pub fn poll(&mut self, max_wait: Duration) -> Result<Vec<OwnerEvent>> {
        let mut events = std::mem::take(&mut self.pending);
        if let Some(control) = &self.run_control {
            for shell in self.shells.values() {
                if !shell.ended && shell.worker.is_finished() {
                    control.revoke(&shell.run_id);
                }
            }
        }
        let finished_before_poll: Vec<_> = self
            .shells
            .iter_mut()
            .filter_map(|(id, shell)| {
                if shell.ended
                    || !(shell.retiring || shell.gateway.is_some())
                    || !shell.worker.is_finished()
                {
                    return None;
                }
                // A failed worker may finish without recording SavedExit.
                shell.gateway.take();
                shell.retiring.then(|| id.clone())
            })
            .collect();
        self.start_due_cwd_captures();
        let wait = if events.is_empty() && finished_before_poll.is_empty() {
            self.summaries
                .values()
                .filter_map(|s| s.deadline)
                .chain(
                    self.shells
                        .values()
                        .filter_map(|shell| shell.cwd_capture.map(|(deadline, _)| deadline)),
                )
                .map(|d| d.saturating_duration_since(Instant::now()))
                .fold(max_wait, Duration::min)
        } else {
            Duration::ZERO
        };
        match self.receive.recv_timeout(wait) {
            Ok(event) => self.apply(event, &mut events),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!("Shell event channel disconnected"));
            }
        }
        let mut drained = false;
        for _ in 0..127 {
            let Ok(event) = self.receive.try_recv() else {
                drained = true;
                break;
            };
            self.apply(event, &mut events);
        }
        self.shells.retain(|id, shell| {
            // For a worker without SavedExit, only an empty queue observed after
            // its thread stopped proves there is no final exit still to apply.
            let missing_exit = drained && finished_before_poll.contains(id);
            if !shell.retiring || !(shell.ended || missing_exit) || !shell.worker.is_finished() {
                return true;
            }
            let stopped = shell
                .worker
                .join_after_shutdown()
                .map_err(|error| anyhow::anyhow!("shut down closed Shell session: {error:#}"));
            if let Err(error) = &stopped {
                diagnostic(&mut events, id, error.to_string());
            } else if !shell.ended {
                diagnostic(
                    &mut events,
                    id,
                    "closed Shell stopped before recording its exit".into(),
                );
            }
            if let Some(reply) = shell.stop_completion.take() {
                let result = stopped.and_then(|()| {
                    ensure!(shell.ended, "Shell stopped before recording its exit");
                    if let Some(error) = &shell.exit_error {
                        if shell.catalog_stop {
                            anyhow::bail!("{error}");
                        }
                        anyhow::bail!("persist Shell exit: {error}");
                    }
                    Ok(())
                });
                let _ = reply.send(result);
            }
            false
        });
        self.poll_provider_hooks(&mut events);
        self.poll_context_reminders(&mut events);
        self.flush_due_summaries(&mut events);
        self.start_due_cwd_captures();
        Ok(events)
    }

    pub(crate) fn take_context_card_changes(&mut self) -> bool {
        std::mem::take(&mut self.context_cards_changed)
    }

    fn poll_context_reminders(&mut self, events: &mut Vec<OwnerEvent>) {
        let tick = Instant::now();
        if self.closing || tick < self.next_context_reminder_poll {
            return;
        }
        self.next_context_reminder_poll = tick + Duration::from_secs(1);
        let time = now();
        let mut idle_sessions = Vec::new();
        let mut settings_by_root: HashMap<PathBuf, Option<ContextOptimizerSnapshot>> =
            HashMap::new();
        for (session_id, shell) in &mut self.shells {
            if shell.ended
                || shell.retiring
                || shell.provider_startup_pending
                || shell.worker.is_finished()
            {
                continue;
            }
            let Some(hook) = &mut shell.hook else {
                continue;
            };
            let work_status = self
                .connection
                .query_row(
                    "SELECT work_status FROM sessions WHERE id = ? AND archived_at IS NULL",
                    [session_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .unwrap_or(None);
            let snapshot = (work_status.as_deref() == Some("running")
                && hook.context_guard.provider_id() == "claude-code"
                && hook.last_observation.is_some())
            .then(|| hook.hud.snapshot(time as f64));
            if snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.task_status == TaskStatus::Idle
                    && !RunHook::awaiting_user(hook.context_tools(snapshot))
            }) {
                let since = hook.idle_while_running_since.get_or_insert(tick);
                if let Some(submitted) = hook.last_submission {
                    // A new submission may still be waiting for its prompt hook.
                    *since = (*since).max(submitted);
                }
                if tick.duration_since(*since) >= PROVIDER_IDLE_RECOVERY {
                    idle_sessions.push(session_id.clone());
                    hook.idle_while_running_since = None;
                }
            } else {
                hook.idle_while_running_since = None;
            }
            let root = hook.optimizer_data_root.clone();
            let Some(settings) = settings_by_root
                .entry(root.clone())
                .or_insert_with(|| ContextOptimizerConfig::for_data_root(&root).snapshot().ok())
                .as_ref()
                .and_then(|snapshot| {
                    snapshot
                        .agents
                        .get(hook.context_guard.provider_id())
                        .map(|entry| &entry.settings)
                })
            else {
                continue;
            };
            if !settings.enabled || !settings.cache_monitor {
                self.context_cards_changed |= hook.context_card_alert.take().is_some();
                continue;
            }
            let Some(work_status) = work_status else {
                continue;
            };
            let snapshot = snapshot.unwrap_or_else(|| hook.hud.snapshot(time as f64));
            hook.observe_context_and_tools(
                shell.gateway.as_ref(),
                &shell.cwd,
                &work_status,
                &snapshot,
            );
            let card_alert = hook
                .context_guard
                .has_critical_context()
                .then_some("critical");
            self.context_cards_changed |= hook.context_card_alert != card_alert;
            hook.context_card_alert = card_alert;
            let reminders: Vec<_> = hook
                .context_guard
                .pending_reminders(time as f64, settings)
                .collect();
            if reminders.is_empty() {
                continue;
            }
            let locale =
                sessions::current_interface_locale(sessions::read_locale_preference(&root));
            for reminder in reminders {
                let id = (self.next_id)();
                let mut notification = reminder.notification(locale);
                // The guard already deduplicates threshold crossings and cache
                // deadlines. Distinct cycles can occur inside the UI cooldown.
                notification["cooldownKey"] = json!(id);
                match sessions::publish_provider_notification(
                    &mut self.connection,
                    &command(id.clone(), "agent.notification.publish", id),
                    session_id,
                    &shell.run_id,
                    &self.generation,
                    hook.context_guard.provider_id(),
                    notification,
                    time,
                ) {
                    Ok(commit) if commit.result["published"] == true => {
                        hook.context_guard.mark_reminded(reminder);
                        events.push(OwnerEvent::Domain(commit));
                    }
                    Ok(_) => {}
                    Err(error) => diagnostic(events, session_id, format!("{error:#}")),
                }
            }
        }
        for session_id in idle_sessions {
            if let Err(error) = self.set_work_status(&session_id, events, |current| {
                (current == "running").then_some("idle")
            }) {
                eprintln!("[runtime] persist Claude idle status for {session_id}: {error:#}");
            }
        }
    }

    fn poll_provider_hook_messages(
        &mut self,
        events: &mut Vec<OwnerEvent>,
        limit: usize,
        accepts: impl Fn(&Self, &Scope, Instant) -> bool,
    ) -> bool {
        let Some(server) = &self.hook_server else {
            return true;
        };
        let hooks: Vec<_> = server.messages().take(limit).collect();
        let drained = hooks.len() < limit;
        for hook in hooks {
            match hook {
                HookMessage::Observed(scope, observation, received_at) => {
                    if !accepts(self, &scope, received_at) {
                        reject_identity(events, &scope, "retired-run");
                        continue;
                    }
                    let late_identity = self
                        .shells
                        .get(&scope.session_id)
                        .filter(|shell| shell.run_id == scope.run_id)
                        .and_then(|shell| shell.hook.as_ref())
                        .is_some_and(|hook| {
                            (scope.fork_source_id.is_some()
                                || hook
                                    .resume
                                    .as_ref()
                                    .is_some_and(ResumeMonitor::fresh_permission))
                                && hook
                                    .deadline
                                    .is_some_and(|deadline| received_at >= deadline)
                        });
                    if !late_identity {
                        self.apply_hook_at(scope, observation, received_at, events);
                    }
                }
                HookMessage::DshObserved(scope, payload, received_at) => {
                    if accepts(self, &scope, received_at) {
                        self.apply_dsh_hook(scope, &payload, events);
                    }
                }
                HookMessage::AntigravityObserved(scope, source, payload, received_at) => {
                    if accepts(self, &scope, received_at) {
                        self.apply_antigravity_hook(scope, source, payload, received_at, events);
                    } else {
                        reject_identity(events, &scope, "retired-run");
                    }
                }
                HookMessage::Retired(scope) => reject_identity(events, &scope, "retired-run"),
            }
        }
        drained
    }

    fn poll_provider_hooks(&mut self, events: &mut Vec<OwnerEvent>) {
        let hooks_drained = self.poll_provider_hook_messages(events, 64, |_, _, _| true);
        let antigravity_timeouts: Vec<_> = self
            .shells
            .iter_mut()
            .filter_map(|(id, shell)| {
                let hook = shell.hook.as_mut()?;
                if shell.ended
                    || shell.retiring
                    || !hook
                        .deadline
                        .is_some_and(|deadline| deadline <= Instant::now())
                {
                    return None;
                }
                let agy = hook.antigravity.as_mut()?;
                (agy.binding.is_some()
                    || matches!(agy.run.deadline_elapsed(), AntigravityRecovery::Failed(_)))
                .then(|| (id.clone(), shell.run_id.clone(), agy.binding.is_some()))
            })
            .collect();
        for (id, run_id, resume) in antigravity_timeouts {
            if resume {
                let scope = Scope {
                    profile: LaunchProfile::Antigravity,
                    session_id: id,
                    run_id,
                    generation: self.generation.clone(),
                    expected_id: None,
                    fork_source_id: None,
                };
                self.park_antigravity_restore(
                    &scope,
                    "Antigravity restore identity timed out",
                    events,
                );
            } else {
                self.park_fork_run(&id, &run_id, FORK_IDENTITY_TIMEOUT, events);
            }
        }
        let dsh_timeouts = self
            .shells
            .iter_mut()
            .filter_map(|(id, shell)| {
                let hook = shell.hook.as_mut()?;
                if shell.ended
                    || shell.retiring
                    || !hook
                        .deadline
                        .is_some_and(|deadline| deadline <= Instant::now())
                {
                    return None;
                }
                if let Some(DshObservation::Resume(resume, _)) = &mut hook.dsh
                    && resume.timeout(&shell.run_id)
                {
                    return Some(id.clone());
                }
                None
            })
            .collect::<Vec<_>>();
        for id in dsh_timeouts {
            self.park_dsh_run(&id, "DSH restore identity timed out", events);
        }
        let mut codex_effects = Vec::new();
        if !self.closing {
            for (id, shell) in &mut self.shells {
                if !shell.ended
                    && !shell.retiring
                    && let Some(hook) = &mut shell.hook
                    && let Some(codex) = &mut hook.codex
                    && let Some(effect) = codex.poll_error(&shell.cwd, false)
                {
                    codex_effects.push((id.clone(), shell.run_id.clone(), effect));
                }
            }
        }
        for (id, run_id, effect) in codex_effects {
            self.apply_codex_effect(&id, &run_id, effect, None, events);
        }
        // Transcript reads can see output from hooks still queued in the next batch.
        self.poll_claude_transcripts(events, hooks_drained);
        self.poll_codex_titles(events);
        self.poll_antigravity_titles(events);
        self.poll_antigravity_transcripts();
        let expired_codex_forks = self
            .shells
            .iter()
            .filter_map(|(id, shell)| {
                let hook = shell.hook.as_ref()?;
                (!shell.ended
                    && !shell.retiring
                    && hook
                        .deadline
                        .is_some_and(|deadline| deadline <= Instant::now()))
                .then(|| {
                    hook.fork
                        .clone()
                        .map(|(key, authority)| (id.clone(), key, authority))
                })
                .flatten()
            })
            .collect::<Vec<_>>();
        for (id, key, authority) in expired_codex_forks {
            self.fail_codex_fork_run(&id, &key, &authority, FORK_IDENTITY_TIMEOUT, events);
        }
        let mut expired_forks = Vec::new();
        let mut expired_permission_restores = Vec::new();
        let mut expired_codex_restores = Vec::new();
        for (session_id, shell) in &mut self.shells {
            if !self.closing
                && !shell.ended
                && !shell.retiring
                && let Some(hook) = &mut shell.hook
                && !hook
                    .resume
                    .as_ref()
                    .is_some_and(ResumeMonitor::waiting_for_bypass_consent)
                && hook
                    .deadline
                    .is_some_and(|deadline| deadline <= Instant::now())
            {
                hook.deadline = None;
                if let Some(resume) = &hook.codex_resume {
                    if resume.permission_respawn {
                        expired_permission_restores.push((
                            session_id.clone(),
                            shell.run_id.clone(),
                            resume.binding_id.clone(),
                            true,
                        ));
                    } else {
                        let ready = !shell.worker.is_finished()
                            && (resume.has_visible_output()
                                || shell.emitted_output
                                    && hook.codex.as_ref().is_some_and(|codex| {
                                        codex.find_rollout(resume.identity).is_some()
                                    }));
                        expired_codex_restores.push((
                            session_id.clone(),
                            shell.run_id.clone(),
                            resume.binding_id.clone(),
                            ready,
                        ));
                    }
                    continue;
                }
                let restore = hook
                    .resume
                    .as_mut()
                    .filter(|resume| resume.tracks_restore());
                let mut restore_state = "none".to_owned();
                if let Some(resume) = restore {
                    if resume.fresh_permission() {
                        hook.permission_identity_error =
                            Some(FRESH_PERMISSION_IDENTITY_TIMEOUT.into());
                        continue;
                    }
                    if (resume.permission_respawn || resume.rebuild_parent.is_some())
                        && let Some(binding_id) = &resume.binding_id
                    {
                        expired_permission_restores.push((
                            session_id.clone(),
                            shell.run_id.clone(),
                            binding_id.clone(),
                            false,
                        ));
                        continue;
                    }
                    let Some(binding_id) = &resume.binding_id else {
                        expired_forks.push((session_id.clone(), shell.run_id.clone()));
                        continue;
                    };
                    match sessions::claude_binding(&self.connection, session_id) {
                        Ok(Some(binding)) if binding.id == *binding_id => {
                            restore_state = binding.restore_state;
                        }
                        Ok(_) => {}
                        Err(error) => diagnostic(
                            events,
                            session_id,
                            format!("Read Claude restore state: {error:#}"),
                        ),
                    }
                    resume.release_readiness();
                }
                events.push(OwnerEvent::ProviderIdentity(json!({
                    "result": "timed-out", "sessionId": session_id, "runId": shell.run_id,
                    "provider": hook.profile.kind(),
                    "providerSessionId": hook.resume.as_ref().and_then(|resume| resume.expected_id.as_deref()),
                    "restoreState": restore_state,
                    "restoreError": null, "spawnRevision": hook.launch_metadata.get("spawnRevision"),
                })));
            }
        }
        for (session_id, run_id) in expired_forks {
            self.park_fork_run(&session_id, &run_id, FORK_IDENTITY_TIMEOUT, events);
        }
        for (session_id, run_id, binding_id, ready) in expired_codex_restores {
            if ready {
                // Keep the identity and failure monitor armed for a late
                // SessionStart or an error after the CLI's interactive screen.
                let result = self.commit_codex_restore(
                    &session_id,
                    ClaudeRestore::Ready {
                        binding_id: &binding_id,
                        run_id: &run_id,
                    },
                    Some(&run_id),
                    events,
                );
                let result = result.and_then(|changed| {
                    // Automatic resumes already have no restore banner to clear.
                    if !changed
                        && let Some(binding) = self.codex_binding(&session_id)?
                        && binding.id == binding_id
                        && binding.restore_state == "none"
                    {
                        events.push(OwnerEvent::ProviderIdentity(json!({
                            "result": "timed-out", "sessionId": session_id, "runId": run_id,
                            "provider": "codex", "bindingId": binding.id,
                            "providerSessionId": binding.provider_session_id,
                            "restoreState": binding.restore_state, "restoreError": null,
                            "spawnRevision": binding.metadata["spawnRevision"],
                        })));
                    }
                    Ok(())
                });
                if let Err(error) = result {
                    diagnostic(
                        events,
                        &session_id,
                        format!("Clear Codex restore: {error:#}"),
                    );
                    if let Some(hook) = self
                        .shells
                        .get_mut(&session_id)
                        .and_then(|shell| shell.hook.as_mut())
                    {
                        hook.deadline = Some(Instant::now() + IDENTITY_TIMEOUT);
                    }
                }
            } else {
                self.park_codex_run(
                    &session_id,
                    &run_id,
                    &binding_id,
                    "provider resume identity confirmation timed out",
                    events,
                );
            }
        }
        for (session_id, run_id, binding_id, codex) in expired_permission_restores {
            let error = "provider resume identity confirmation timed out";
            if codex {
                self.park_codex_run(&session_id, &run_id, &binding_id, error, events);
            } else {
                self.park_provider_run(&session_id, &run_id, &binding_id, error, events);
            }
        }
        for (session_id, shell) in &mut self.shells {
            if !self.closing
                && !shell.ended
                && !shell.retiring
                && let Some(bytes) = shell
                    .hook
                    .as_mut()
                    .and_then(|hook| hook.resume.as_mut())
                    .and_then(ResumeMonitor::due_input)
            {
                write_resume_input(session_id, shell, bytes, events);
            }
        }
    }

    fn apply_antigravity_hook(
        &mut self,
        scope: Scope,
        source: antigravity_events::Source,
        payload: Value,
        received_at: Instant,
        events: &mut Vec<OwnerEvent>,
    ) {
        let Some(shell) = self.shells.get_mut(&scope.session_id).filter(|shell| {
            !self.closing
                && !shell.ended
                && !shell.retiring
                && shell.run_id == scope.run_id
                && scope.generation == self.generation
        }) else {
            reject_identity(events, &scope, "retired-run");
            return;
        };
        let Some(hook) = &mut shell.hook else {
            return;
        };
        let Some(agy) = &mut hook.antigravity else {
            return;
        };
        if shell.provider_startup_pending {
            if agy.startup.len() < HOOK_QUEUE_CAPACITY {
                agy.startup.push((scope, source, payload, received_at));
            }
            return;
        }
        if (agy.fork_pending || agy.binding.is_some())
            && hook
                .deadline
                .is_some_and(|deadline| received_at >= deadline)
        {
            return;
        }
        let time = now();
        let first = agy.run.conversation_id().is_none();
        let update = match agy.run.ingest(&scope.run_id, source, &payload, time) {
            AntigravityOutcome::Ignored => return,
            AntigravityOutcome::SendForkCommand => {
                let (reply, _) = mpsc::channel();
                if shell
                    .worker
                    .send(WorkerCommand::Write {
                        bytes: antigravity_events::FORK_COMMAND.as_bytes().to_vec(),
                        origin: InputOrigin::System,
                        reply,
                    })
                    .is_err()
                {
                    self.park_fork_run(
                        &scope.session_id,
                        &scope.run_id,
                        "Could not send the Antigravity fork command",
                        events,
                    );
                }
                return;
            }
            AntigravityOutcome::IdentityMismatch { .. } => {
                self.park_antigravity_restore(&scope, IDENTITY_MISMATCH, events);
                return;
            }
            AntigravityOutcome::Observed(update) => update,
        };
        if let Some(identity) = update.identity {
            let mut observed = serde_json::Map::new();
            observed.insert("cwd".into(), json!(shell.cwd));
            if let Some(model) = identity
                .model
                .and_then(|model| safe_hook_text(model, &hook.secrets))
            {
                observed.insert("model".into(), json!(model));
            }
            if let Some(permission) = identity.permission {
                observed.insert("permissionMode".into(), json!(permission.as_str()));
            }
            if agy.run.can_fork() {
                observed.insert("canFork".into(), json!(true));
            }
            let id = (self.next_id)();
            let commit = sessions::record_antigravity_identity(
                &mut self.connection,
                &command(id.clone(), "provider-hook.identity", id.clone()),
                &sessions::ProviderIdentity {
                    binding_id: &id,
                    session_id: &scope.session_id,
                    run_id: &scope.run_id,
                    generation: &scope.generation,
                    provider_session_id: identity.conversation_id,
                    expected_id: scope.expected_id.as_deref().filter(|_| first),
                    launch_metadata: &hook.launch_metadata,
                    observed_metadata: &Value::Object(observed),
                    event_name: update.event_name.unwrap_or("statusline"),
                    first_for_run: first,
                },
                time,
            );
            match commit {
                Ok(commit) => {
                    hook.deadline = None;
                    if let Some(resume) = &mut hook.resume {
                        resume.stop_trust();
                    }
                    agy.binding = None;
                    agy.fork_pending = false;
                    hook.hud.set_resumable(true);
                    if first {
                        let mut result = commit.result.clone();
                        result["result"] = json!("confirmed");
                        events.push(OwnerEvent::ProviderIdentity(result));
                    }
                    events.push(OwnerEvent::Domain(commit));
                }
                Err(_) => {
                    if agy.fork_pending {
                        self.park_fork_run(
                            &scope.session_id,
                            &scope.run_id,
                            "Could not save the Antigravity fork identity",
                            events,
                        );
                    } else if agy.binding.is_some() {
                        self.park_antigravity_restore(
                            &scope,
                            "Could not save the Antigravity identity",
                            events,
                        );
                    } else {
                        reject_identity(events, &scope, "binding-write-failed");
                    }
                    return;
                }
            }
        }
        if let Some(path) = update.transcript_path.map(Path::new)
            && let Some(identity) = agy.run.conversation_id()
            && let Some(relative) = crate::antigravity_catalog::transcript_path(identity)
            && let Some(conversation) = relative
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
            && path.is_absolute()
            && path.extension().is_some_and(|ext| ext == "jsonl")
            && agy.transcript_path.as_deref() != Some(path)
            && path
                .canonicalize()
                .ok()
                .zip(agy.catalog_root.canonicalize().ok())
                .is_some_and(|(path, root)| path.starts_with(root.join(conversation)))
        {
            agy.transcript_path = path.canonicalize().ok();
            agy.transcript = LiveTranscript::default();
            agy.transcript_poll = Instant::now();
        }
        let mut hud = Value::Object(update.payload.clone());
        if let Some(event) = update.event_name {
            hud["hookEventName"] = json!(event);
        }
        hook.rescue_ready |= agy.run.conversation_id().is_some()
            && !matches!(
                hud["agent_state"].as_str(),
                Some("initializing" | "authenticating")
            );
        let status = update
            .work_status
            .filter(|_| hook.last_submission.is_none_or(|sent| sent <= received_at));
        let notification = update.notification.map(|notification| {
            let mut payload = json!(notification);
            redact_hud(&mut payload, &hook.secrets);
            payload["operation"] = json!("upsert");
            payload
        });
        if update.title_candidate.is_some() {
            // The CLI can publish its title just after the Stop hook arrives.
            let start = Instant::now();
            agy.title_check = Some((
                start + Duration::from_millis(250),
                start + Duration::from_secs(4),
            ));
        }
        let title_root = update
            .title_candidate
            .as_ref()
            .map(|_| agy.catalog_root.clone());
        if let Some(event) = update.event_name {
            hook.observe_rescue_tool(event, &hud, &shell.cwd);
            hook.context_session_ended = event == "SessionEnd";
        }
        redact_hud(&mut hud, &hook.secrets);
        hook.hud.ingest_provider(&hud, time as f64);
        if let Some(status) = status {
            use antigravity_events::WorkStatus;
            let status = match status {
                WorkStatus::Starting => "starting",
                WorkStatus::Idle => "idle",
                WorkStatus::Running => "running",
                WorkStatus::NeedsInput => "needs-input",
                WorkStatus::Error => "error",
            };
            if let Err(error) = self.set_work_status(&scope.session_id, events, |_| Some(status)) {
                diagnostic(events, &scope.session_id, format!("{error:#}"));
            }
        }
        if let Some(payload) = notification {
            let id = (self.next_id)();
            match sessions::publish_provider_notification(
                &mut self.connection,
                &command(id.clone(), "agent.notification", id),
                &scope.session_id,
                &scope.run_id,
                &scope.generation,
                "antigravity",
                payload,
                time,
            ) {
                Ok(commit) => events.push(OwnerEvent::Domain(commit)),
                Err(error) => diagnostic(events, &scope.session_id, format!("{error:#}")),
            }
        }
        if let (Some(conversation), Some(data_root)) = (update.title_candidate, title_root) {
            self.observe_antigravity_title(&scope, &conversation, &data_root, events);
        }
    }

    fn poll_antigravity_transcripts(&mut self) {
        let at = Instant::now();
        for shell in self
            .shells
            .values_mut()
            .filter(|shell| !shell.ended && !shell.retiring)
        {
            let Some(hook) = &mut shell.hook else {
                continue;
            };
            let Some(agy) = &mut hook.antigravity else {
                continue;
            };
            if agy.transcript_poll > at {
                continue;
            }
            let Some(identity) = agy.run.conversation_id() else {
                continue;
            };
            let Some(relative) = crate::antigravity_catalog::transcript_path(identity) else {
                continue;
            };
            let path = agy
                .transcript_path
                .clone()
                .unwrap_or_else(|| agy.catalog_root.join(relative));
            let more = agy
                .transcript
                .poll(&agy.catalog_root, &path, identity, |row| {
                    hook.hud.ingest_antigravity_row(row)
                })
                .unwrap_or_default();
            agy.transcript_poll = at
                + if more {
                    Duration::from_millis(25)
                } else {
                    Duration::from_secs(1)
                };
        }
    }

    fn poll_antigravity_titles(&mut self, events: &mut Vec<OwnerEvent>) {
        let now = Instant::now();
        let due: Vec<_> = self
            .shells
            .iter_mut()
            .filter_map(|(session_id, shell)| {
                if self.closing || shell.ended || shell.retiring {
                    return None;
                }
                let agy = shell.hook.as_mut()?.antigravity.as_mut()?;
                let (next, until) = agy.title_check?;
                if next > now {
                    return None;
                }
                agy.title_check = (now < until).then(|| (now + Duration::from_secs(1), until));
                let conversation = agy.run.conversation_id()?.to_owned();
                Some((
                    session_id.clone(),
                    shell.run_id.clone(),
                    conversation,
                    agy.catalog_root.clone(),
                ))
            })
            .collect();
        for (session, run, conversation, root) in due {
            let scope = Scope {
                session_id: session,
                run_id: run,
                generation: self.generation.clone(),
                profile: LaunchProfile::Antigravity,
                expected_id: None,
                fork_source_id: None,
            };
            self.observe_antigravity_title(&scope, &conversation, &root, events);
        }
    }

    fn observe_antigravity_title(
        &mut self,
        scope: &Scope,
        conversation: &str,
        catalog_root: &Path,
        events: &mut Vec<OwnerEvent>,
    ) {
        let catalog = AntigravityCatalog::new(catalog_root);
        let Ok(Some(title)) = catalog.latest_auto_title(conversation) else {
            return;
        };
        let Some(title) = self
            .shells
            .get(&scope.session_id)
            .and_then(|shell| shell.hook.as_ref())
            .and_then(|hook| safe_hook_text(&title, &hook.secrets))
        else {
            return;
        };
        let id = (self.next_id)();
        match sessions::observe_provider_title(
            &mut self.connection,
            &command(id.clone(), "provider-hook.title", id),
            &scope.session_id,
            &scope.run_id,
            &scope.generation,
            "antigravity",
            conversation,
            title,
            now(),
        ) {
            Ok(Some(commit)) => {
                if let Some(agy) = self
                    .shells
                    .get_mut(&scope.session_id)
                    .and_then(|shell| shell.hook.as_mut())
                    .and_then(|hook| hook.antigravity.as_mut())
                {
                    agy.title_check = None;
                }
                events.push(OwnerEvent::Domain(commit));
            }
            Ok(None) => {}
            Err(error) => diagnostic(events, &scope.session_id, format!("{error:#}")),
        }
    }

    fn park_antigravity_restore(
        &mut self,
        scope: &Scope,
        reason: &str,
        events: &mut Vec<OwnerEvent>,
    ) {
        let binding = self
            .shells
            .get_mut(&scope.session_id)
            .and_then(|shell| shell.hook.as_mut())
            .and_then(|hook| hook.antigravity.as_mut())
            .and_then(|agy| agy.binding.take());
        self.retire_sessions(std::slice::from_ref(&scope.session_id));
        if let Some(binding_id) = binding
            && let Err(error) = self.commit_antigravity_restore(
                &scope.session_id,
                ClaudeRestore::Fail {
                    binding_id: &binding_id,
                    error: reason,
                },
                Some(&scope.run_id),
                events,
            )
        {
            diagnostic(events, &scope.session_id, format!("{error:#}"));
        }
    }

    fn apply_dsh_hook(&mut self, scope: Scope, payload: &Value, events: &mut Vec<OwnerEvent>) {
        let Some(shell) = self.shells.get_mut(&scope.session_id).filter(|shell| {
            !self.closing
                && !shell.ended
                && !shell.retiring
                && shell.run_id == scope.run_id
                && scope.generation == self.generation
        }) else {
            return;
        };
        let Some(hook) = &mut shell.hook else {
            return;
        };
        if shell.provider_startup_pending {
            if hook.dsh_pending.len() < HOOK_QUEUE_CAPACITY {
                hook.dsh_pending.push((scope, payload.clone()));
            }
            return;
        }
        let Some(observer) = &mut hook.dsh else {
            return;
        };
        let update = match observer.observe(&scope.run_id, payload) {
            DshEventOutcome::Observed(update) => update,
            DshEventOutcome::Ignored => return,
            DshEventOutcome::IdentityMismatch { .. } => {
                self.park_dsh_run(
                    &scope.session_id,
                    "DSH restore returned a different session identity",
                    events,
                );
                return;
            }
        };
        let event = payload["event_type"].as_str();
        let data = &payload["event_data"];
        hook.rescue_ready |= event == Some("ready") && payload["ready"] == true;
        if event == Some("compaction/end")
            && data.get("completed").and_then(Value::as_bool) != Some(false)
            && (data["error"].is_null() || data["error"] == "" || data["error"] == false)
        {
            hook.dsh_compacted_percent = hook.hud.context_metrics(None).0;
            hook.context_guard.on_compaction();
        }
        if let Some(identity) = update.identity
            && identity.persisted
        {
            let expected = match observer {
                DshObservation::Resume(resume, _) if hook.deadline.is_some() => {
                    Some(resume.original_binding().0)
                }
                _ => None,
            };
            if identity.cwd.is_some_and(|cwd| {
                Path::new(cwd).canonicalize().ok() != Path::new(&shell.cwd).canonicalize().ok()
            }) {
                self.park_dsh_run(
                    &scope.session_id,
                    "DSH restore returned a different working directory",
                    events,
                );
                return;
            }
            let id = (self.next_id)();
            match sessions::record_dsh_identity(
                &mut self.connection,
                &command(id.clone(), "provider-hook.dsh-identity", id.clone()),
                &sessions::ProviderIdentity {
                    binding_id: &id,
                    session_id: &scope.session_id,
                    run_id: &scope.run_id,
                    generation: &scope.generation,
                    provider_session_id: identity.session_id,
                    expected_id: expected,
                    launch_metadata: &hook.launch_metadata,
                    observed_metadata: &Value::Null,
                    event_name: "ready",
                    first_for_run: hook.deadline.is_some(),
                },
                now(),
            ) {
                Ok(commit) => {
                    let mut confirmed = commit.result.clone();
                    confirmed["result"] = json!("confirmed");
                    events.push(OwnerEvent::ProviderIdentity(confirmed));
                    events.push(OwnerEvent::Domain(commit));
                    hook.deadline = None;
                    hook.hud.set_resumable(true);
                }
                Err(_) => {
                    self.park_dsh_run(&scope.session_id, "Could not save the DSH identity", events);
                    return;
                }
            }
        }
        hook.hud.ingest_provider(
            &json!({
                "dsh_session_id": update.session_id,
                "dsh_hud": update.hud,
                "dsh_work_status": update.work_status,
            }),
            now() as f64,
        );
        let title = update
            .title
            .filter(|title| safe_hook_text(title, &hook.secrets).is_some());
        if hook.dsh_status != Some(update.work_status) || title.is_some() {
            let status = match update.work_status {
                DshWorkStatus::Idle => "idle",
                DshWorkStatus::Running => "running",
                DshWorkStatus::NeedsInput => "needs-input",
                DshWorkStatus::Error => "error",
            };
            let id = (self.next_id)();
            match sessions::project_dsh_status(
                &mut self.connection,
                &command(id.clone(), "provider-hook.dsh-status", id),
                &scope.session_id,
                &scope.run_id,
                status,
                title,
                now(),
            ) {
                Ok(commit) => {
                    hook.dsh_status = Some(update.work_status);
                    events.push(OwnerEvent::Domain(commit));
                }
                Err(error) => diagnostic(events, &scope.session_id, format!("{error:#}")),
            }
        }
        if let Some(notification) = update.notification {
            let mut payload = json!(notification);
            redact_hud(&mut payload, &hook.secrets);
            payload["operation"] = json!("upsert");
            let id = (self.next_id)();
            match sessions::publish_provider_notification(
                &mut self.connection,
                &command(id.clone(), "agent.notification", id),
                &scope.session_id,
                &scope.run_id,
                &scope.generation,
                "dsh",
                payload,
                now(),
            ) {
                Ok(commit) => events.push(OwnerEvent::Domain(commit)),
                Err(error) => diagnostic(events, &scope.session_id, format!("{error:#}")),
            }
        }
    }

    fn park_dsh_run(&mut self, session_id: &str, error: &str, events: &mut Vec<OwnerEvent>) {
        let binding = self
            .shells
            .get_mut(session_id)
            .and_then(|shell| shell.hook.as_mut())
            .and_then(|hook| {
                hook.deadline = None;
                match hook.dsh.take() {
                    Some(DshObservation::Resume(_, binding)) => Some(binding),
                    _ => None,
                }
            });
        if let Some(binding_id) = binding
            && let Err(failure) = self.change_dsh_restore(
                session_id,
                ClaudeRestore::Fail {
                    binding_id: &binding_id,
                    error,
                },
            )
        {
            diagnostic(events, session_id, format!("{failure:#}"));
        }
        self.retire_sessions(&[session_id.to_owned()]);
    }

    fn poll_claude_transcripts(&mut self, events: &mut Vec<OwnerEvent>, hooks_drained: bool) {
        if self.closing {
            return;
        }
        let at = Instant::now();
        let ready: Vec<_> = self
            .shells
            .iter()
            .filter_map(|(id, shell)| {
                let hook = shell.hook.as_ref()?;
                let due = hook.claude_transcript_poll.is_some_and(|due| due <= at);
                let resumed = hooks_drained && hook.claude_transcript_resumed();
                (!shell.ended && !shell.retiring && (due || resumed)).then(|| (id.clone(), due))
            })
            .collect();
        for (id, due) in ready {
            if due {
                self.observe_claude_transcript(&id, events);
            }
            if hooks_drained
                && self.shells[&id]
                    .hook
                    .as_ref()
                    .is_some_and(RunHook::claude_transcript_resumed)
            {
                self.recover_claude_work(&id, events);
            }
        }
    }

    fn observe_claude_transcript(&mut self, session_id: &str, events: &mut Vec<OwnerEvent>) {
        let Some(shell) = self
            .shells
            .get_mut(session_id)
            .filter(|shell| !shell.ended && !shell.retiring)
        else {
            return;
        };
        let Some(hook) = &mut shell.hook else {
            return;
        };
        let Some((root, path, identity)) = hook
            .claude_projects
            .as_deref()
            .zip(hook.claude_transcript.as_deref())
            .zip(
                hook.last_observation
                    .as_ref()
                    .and_then(|last| last.identity.as_deref()),
            )
            .map(|((root, path), identity)| (root, path, identity.trim()))
        else {
            // A seeded resume transcript becomes readable to this run once its
            // authenticated identity arrives.
            hook.claude_transcript_poll = hook
                .claude_transcript
                .as_ref()
                .map(|_| Instant::now() + Duration::from_millis(250));
            return;
        };
        let backlog = hook.claude_live.poll(root, path, identity, |row| {
            if let Some(row) = row {
                let content = &row["message"]["content"];
                if row["type"] == "assistant"
                    && row["isSidechain"] != true
                    && row["is_sidechain"] != true
                    && row["isApiErrorMessage"] != true
                    && row["message"]["model"] != "<synthetic>"
                    && (content.as_array().is_some_and(|content| !content.is_empty())
                        || crate::provider_hook::nonempty(content).is_some())
                    && let Some(at) = timestamp_ms(&row["timestamp"])
                    && hook
                        .claude_transcript_activity_at_ms
                        .is_none_or(|last| at > last)
                {
                    hook.claude_transcript_activity_at_ms = Some(at);
                }
            } else {
                hook.claude_transcript_activity_at_ms = None;
            }
            hook.hud.ingest_live_claude_history(row)
        });
        if matches!(backlog, Ok(false)) {
            hook.hud.publish_transcript_context();
            hook.sync_gateway_context();
        }
        let backlog = backlog.unwrap_or_default();
        hook.claude_transcript_poll = Some(
            Instant::now()
                + if backlog {
                    Duration::from_millis(25)
                } else {
                    Duration::from_secs(1)
                },
        );
        let Some(title) = hook
            .claude_live
            .title()
            .and_then(|title| safe_hook_text(title, &hook.secrets))
        else {
            return;
        };
        if hook.claude_observed_title.as_deref() == Some(title) {
            return;
        }
        let id = (self.next_id)();
        match sessions::observe_provider_title(
            &mut self.connection,
            &command(id.clone(), "provider-hook.title", id),
            session_id,
            &shell.run_id,
            &self.generation,
            "claude-code",
            identity,
            title,
            now(),
        ) {
            Ok(commit) => {
                hook.claude_observed_title = Some(title.to_owned());
                if let Some(commit) = commit {
                    events.push(OwnerEvent::Domain(commit));
                }
            }
            Err(error) => diagnostic(events, session_id, format!("{error:#}")),
        }
    }

    #[cfg(all(test, target_os = "macos"))]
    fn apply_hook(&mut self, scope: Scope, observation: Observation, events: &mut Vec<OwnerEvent>) {
        self.apply_hook_at(scope, observation, Instant::now(), events);
    }

    fn apply_hook_at(
        &mut self,
        scope: Scope,
        mut observation: Observation,
        received_at: Instant,
        events: &mut Vec<OwnerEvent>,
    ) {
        let Some(shell) = self.shells.get_mut(&scope.session_id).filter(|shell| {
            (!self.closing || shell.promotion_verified())
                && !shell.ended
                && !shell.retiring
                && shell.run_id == scope.run_id
                && scope.generation == self.generation
        }) else {
            reject_identity(events, &scope, "retired-run");
            return;
        };
        let Some(hook) = &mut shell.hook else {
            reject_identity(events, &scope, "retired-run");
            return;
        };
        if hook.codex.is_some() {
            self.apply_codex_hook(scope, observation, received_at, events);
            return;
        }
        if hook.permission_identity_error.is_some() {
            return;
        }
        if observation.statusline {
            if !shell.provider_startup_pending
                && hook
                    .last_observation
                    .as_ref()
                    .and_then(|last| last.identity.as_ref())
                    .is_some_and(|identity| observation.identity.as_ref() == Some(identity))
                && let Some(mut payload) = observation.hud_payload.take()
            {
                if let Some(payload) = payload.as_object_mut() {
                    payload.remove("hook_event_name");
                    payload.remove("hookEventName");
                }
                hook.remember_claude_transcript(&payload);
                hook.hud.ingest_provider(&payload, now() as f64);
                hook.sync_gateway_context();
            }
            return;
        }
        if observation.event == "SessionEnd" {
            if hook
                .last_observation
                .as_ref()
                .is_none_or(|last| last.identity == observation.identity)
            {
                hook.context_session_ended = true;
                hook.context_guard.set_active_tool(false);
            }
            return;
        }
        if observation.event == "unknown" {
            return;
        }
        if let (Some(expected), Some(identity)) = (&scope.expected_id, &observation.identity)
            && expected != identity
            && let Some(binding_id) = hook
                .resume
                .as_ref()
                .and_then(|resume| resume.binding_id.clone())
        {
            self.park_provider_run(
                &scope.session_id,
                &scope.run_id,
                &binding_id,
                IDENTITY_MISMATCH,
                events,
            );
            return;
        }
        let safe = |value: Option<String>| {
            value.filter(|value| safe_hook_text(value, &hook.secrets).is_some())
        };
        observation.identity = safe(observation.identity);
        let Some(identity) = observation.identity.as_deref() else {
            if !hook.reported_missing {
                hook.reported_missing = true;
                reject_identity(events, &scope, "missing-identity");
            }
            return;
        };
        let first_fresh_identity = hook.last_observation.is_none()
            && hook
                .resume
                .as_ref()
                .is_some_and(ResumeMonitor::fresh_permission);
        if shell.provider_startup_pending
            && (first_fresh_identity
                || !hook
                    .resume
                    .as_ref()
                    .is_some_and(ResumeMonitor::tracks_restore))
        {
            // A newly launched child can send a hook and then immediately exit.
            hook.pending_startup_observations
                .push((scope, observation, received_at));
            return;
        }
        if first_fresh_identity {
            if shell.worker.is_finished() {
                return;
            }
            let known = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_bindings
                 WHERE session_id = ? AND provider = 'claude-code' AND provider_session_id = ?)",
                params![scope.session_id, identity.trim()],
                |row| row.get::<_, bool>(0),
            );
            let error = match known {
                Ok(false) => None,
                Ok(true) => Some("Claude Code returned a previous conversation ID for the fresh permission session".into()),
                Err(error) => Some(format!("Could not verify the fresh permission session identity: {error}")),
            };
            if let Some(error) = error {
                hook.permission_identity_error = Some(error);
                hook.deadline = None;
                return;
            }
        }
        if let Some(source_id) = &scope.fork_source_id {
            if let Some(confirmed) = &hook.last_observation {
                if confirmed.identity.as_deref().map(str::trim) != Some(identity.trim()) {
                    reject_identity(events, &scope, "fork-identity-changed");
                    return;
                }
            } else if identity.trim() == source_id.trim() {
                self.park_fork_run(
                    &scope.session_id,
                    &scope.run_id,
                    FORK_SOURCE_IDENTITY,
                    events,
                );
                return;
            }
        }
        // Keep transient gateway selectors out of the restored model metadata.
        observation.model = safe(observation.model).filter(|model| {
            !hook.uses_gateway
                || !(model.starts_with("matou-role/") || model.starts_with("matou-api/"))
        });
        observation.permission_mode = if first_fresh_identity {
            hook.launch_metadata["permissionMode"]
                .as_str()
                .map(str::to_owned)
        } else {
            safe(observation.permission_mode).map(|mode| {
                if mode == "default" && hook.launch_metadata["permissionMode"] == "manual" {
                    "manual".into()
                } else {
                    mode
                }
            })
        };
        observation.cwd = safe(observation.cwd)
            .filter(|cwd| Path::new(cwd).is_absolute() && Path::new(cwd).is_dir());
        let hook_event = observation.event;
        let hud_payload = observation.hud_payload.take();
        // Hook status is independent of whether the identity metadata changed.
        let notification = observation.notification.take();
        let confirms = hook
            .last_observation
            .as_ref()
            .is_none_or(|last| last.identity != observation.identity);
        if hook.last_observation.as_ref() != Some(&observation) {
            let observed = [
                ("model", &observation.model),
                ("permissionMode", &observation.permission_mode),
                ("cwd", &observation.cwd),
            ]
            .into_iter()
            .filter_map(|(key, value)| value.as_ref().map(|value| (key.to_owned(), json!(value))))
            .collect();
            let id = (self.next_id)();
            let commit = sessions::record_claude_identity(
                &mut self.connection,
                &command(
                    format!("provider-hook-{}-{id}", scope.run_id),
                    "provider-hook.identity",
                    id.clone(),
                ),
                &sessions::ClaudeIdentity {
                    binding_id: &id,
                    session_id: &scope.session_id,
                    run_id: &scope.run_id,
                    generation: &scope.generation,
                    provider_session_id: identity,
                    expected_id: scope.expected_id.as_deref(),
                    launch_metadata: &hook.launch_metadata,
                    observed_metadata: &Value::Object(observed),
                    event_name: observation.event,
                    first_for_run: confirms,
                },
                now(),
            );
            let commit = match commit {
                Ok(commit) => commit,
                Err(error) => {
                    if first_fresh_identity {
                        hook.permission_identity_error = Some(format!(
                            "Could not record the fresh permission session identity: {error:#}"
                        ));
                        hook.deadline = None;
                    } else if scope.fork_source_id.is_some() && hook.last_observation.is_none() {
                        self.park_fork_run(
                            &scope.session_id,
                            &scope.run_id,
                            &format!("Could not record the fork identity: {error:#}"),
                            events,
                        );
                    } else {
                        reject_identity(events, &scope, "binding-write-failed");
                    }
                    return;
                }
            };
            hook.deadline = None;
            if let Some(resume) = &mut hook.resume {
                resume.stop_trust();
            }
            hook.last_observation = Some(observation);
            hook.claude_fork_rebuild = None;
            if confirms {
                if let Some(gateway) = &shell.gateway
                    && let Some(identity) = hook
                        .last_observation
                        .as_ref()
                        .and_then(|last| last.identity.as_deref())
                {
                    gateway.bind_usage_identity(identity);
                }
                hook.claude_transcript = None;
                hook.claude_transcript_poll = None;
                hook.claude_observed_title = None;
                hook.claude_live = LiveTranscript::default();
                hook.claude_waiting_since_ms = None;
                hook.claude_transcript_activity_at_ms = None;
                hook.hud.ingest_live_claude_history(None);
                hook.hud.publish_transcript_context();
                let mut payload = commit.result.clone();
                payload["result"] = json!("confirmed");
                events.push(OwnerEvent::ProviderIdentity(payload));
            }
            events.push(OwnerEvent::Domain(commit));
        }
        if let Some(payload) = hud_payload {
            hook.hud.set_resumable(true);
            hook.remember_claude_transcript(&payload);
            let time = now() as f64;
            if hook.hud.complete_claude_compaction(&payload, time) {
                hook.context_guard.on_compaction();
            }
            hook.observe_rescue_tool(hook_event, &payload, &shell.cwd);
            hook.rescue_ready |= hook_event == "SessionStart";
            if matches!(hook_event, "SessionStart" | "UserPromptSubmit") {
                hook.context_session_ended = false;
            }
            hook.hud.ingest_provider(&payload, time);
            hook.sync_gateway_context();
        }
        let permission_confirmed = confirms
            && hook
                .resume
                .as_ref()
                .is_some_and(|resume| resume.permission_respawn);
        // Worker events and HTTP hooks arrive on separate queues. A queued
        // hook must not overwrite a newer submission already applied above it.
        let after_submission = hook
            .last_submission
            .is_none_or(|submitted| submitted <= received_at);
        if let Some(notification) = notification {
            let update_work_status = !confirms && after_submission;
            if update_work_status {
                // Transcript polling can catch up on old rows after a wait starts.
                // Compare provider message timestamps to the notice's receipt time.
                hook.claude_waiting_since_ms = notification
                    .received_at
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .filter(|_| notification.kind != NotificationKind::Completed)
                    .map(|at| at.as_millis() as f64);
            }
            let time = now();
            let id = (self.next_id)();
            let commit = sessions::publish_claude_notification(
                &mut self.connection,
                &command(
                    format!("agent-notification-{id}"),
                    "agent.notification.publish",
                    id,
                ),
                &scope.session_id,
                &scope.run_id,
                &scope.generation,
                notification.payload(),
                time,
            );
            match commit {
                Ok(commit) if !commit.replayed && commit.first_event_sequence.is_some() => {
                    events.push(OwnerEvent::Domain(commit));
                }
                Ok(_) => {}
                Err(error) => diagnostic(
                    events,
                    &scope.session_id,
                    format!("persist provider hook notification: {error:#}"),
                ),
            }
            if update_work_status
                && let Err(error) = self.set_work_status(&scope.session_id, events, |current| {
                    Some(notification.kind.work_status(current))
                })
            {
                diagnostic(
                    events,
                    &scope.session_id,
                    format!("persist provider hook work status: {error:#}"),
                );
            }
        } else if after_submission
            && !permission_confirmed
            && matches!(
                hook_event,
                "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
            )
        {
            self.recover_claude_work(&scope.session_id, events);
        }
        if confirms || matches!(hook_event, "UserPromptSubmit" | "Stop") {
            self.observe_claude_transcript(&scope.session_id, events);
        }
        if permission_confirmed
            && let Err(error) = self.set_work_status(&scope.session_id, events, |_| Some("idle"))
        {
            diagnostic(
                events,
                &scope.session_id,
                format!("persist permission respawn work status: {error:#}"),
            );
        }
    }

    fn recover_claude_work(&mut self, session_id: &str, events: &mut Vec<OwnerEvent>) {
        match self.set_work_status(session_id, events, |current| {
            (current == "needs-input").then_some("running")
        }) {
            Ok(()) => {
                if let Some(hook) = self
                    .shells
                    .get_mut(session_id)
                    .and_then(|s| s.hook.as_mut())
                {
                    hook.claude_waiting_since_ms = None;
                }
            }
            Err(error) => diagnostic(
                events,
                session_id,
                format!("persist resumed provider work status: {error:#}"),
            ),
        }
    }

    fn fail_codex_fork_run(
        &mut self,
        id: &str,
        key: &str,
        authority: &CodexForkAuthority,
        error: &str,
        events: &mut Vec<OwnerEvent>,
    ) {
        if let Some(intent) = self.codex_forks.get_mut(key) {
            let _ = intent.fail(authority, &anyhow::anyhow!("{error}"), |_| Ok(()));
        }
        self.park_fork_run(id, &authority.run_id.to_string(), error, events);
    }

    fn apply_codex_hook(
        &mut self,
        scope: Scope,
        mut observation: Observation,
        received_at: Instant,
        events: &mut Vec<OwnerEvent>,
    ) {
        let shell = self
            .shells
            .get_mut(&scope.session_id)
            .expect("admitted run");
        let hook = shell.hook.as_mut().expect("admitted hook");
        if observation.event == "unknown" || observation.codex.is_none() {
            return;
        }
        if shell.provider_startup_pending {
            // Keep the same early-exit fence as a Shell-to-Claude promotion.
            if hook.pending_startup_observations.len() < HOOK_QUEUE_CAPACITY {
                hook.pending_startup_observations
                    .push((scope, observation, received_at));
            }
            return;
        }
        let Some(identity) = observation
            .identity
            .as_deref()
            .and_then(|identity| uuid::Uuid::parse_str(identity.trim()).ok())
        else {
            return;
        };
        let identity_text = identity.to_string();
        let codex = hook.codex.as_mut().expect("Codex run");
        let payload = observation.codex.take().expect("Codex payload");
        if let Some(confirmed) = &hook.last_observation {
            if confirmed.identity.as_deref() != Some(identity_text.as_str())
                || !codex.admits_hook(&payload, identity)
            {
                return;
            }
            let raw = observation.hud_payload.take();
            let Some(effect) = codex.apply_admitted(observation.event, payload, &shell.cwd) else {
                return;
            };
            if !hook.codex_fork_ready
                && effect.error.is_none()
                && matches!(observation.event, "UserPromptSubmit" | "Stop")
            {
                let id = (self.next_id)();
                match sessions::record_codex_fork_readiness(
                    &mut self.connection,
                    &command(id.clone(), "provider-hook.codex-fork-readiness", id.clone()),
                    &sessions::ProviderIdentity {
                        binding_id: &id,
                        session_id: &scope.session_id,
                        run_id: &scope.run_id,
                        generation: &scope.generation,
                        provider_session_id: &identity_text,
                        expected_id: Some(&identity_text),
                        launch_metadata: &Value::Null,
                        observed_metadata: &Value::Null,
                        event_name: observation.event,
                        first_for_run: false,
                    },
                    now(),
                ) {
                    Ok(commit) => {
                        hook.codex_fork_ready = commit.result["metadata"]["canFork"] == true;
                        events.push(OwnerEvent::Domain(commit));
                    }
                    Err(error) => diagnostic(
                        events,
                        &scope.session_id,
                        format!("Record Codex Fork readiness: {error:#}"),
                    ),
                }
            }
            hook.context_session_ended |= observation.event == "SessionEnd";
            if let Some(raw) = raw {
                let time = now() as f64;
                hook.observe_rescue_tool(observation.event, &raw, &shell.cwd);
                hook.hud.ingest_provider(&raw, time);
            }
            let notification = observation.notification.map(|notice| notice.payload());
            self.apply_codex_effect(
                &scope.session_id,
                &scope.run_id,
                effect,
                notification,
                events,
            );
            return;
        }
        if observation.event != "SessionStart" || shell.worker.is_finished() {
            return;
        }
        let Ok(rollout) = codex.start_rollout(&payload, identity, hook.codex_resume.is_some())
        else {
            return;
        };
        if let Some((key, authority)) = hook.fork.clone() {
            let Some(rollout) = rollout else {
                return;
            };
            let Some(intent) = self.codex_forks.get_mut(&key) else {
                return;
            };
            let cwd = observation
                .cwd
                .as_deref()
                .map(Path::new)
                .unwrap_or_else(|| Path::new(""));
            let header = codex_fork_header(&rollout.1);
            if let Some(header) = &header {
                let candidate = CodexForkCandidate {
                    provider_session_id: header["id"].as_str().unwrap_or_default(),
                    forked_from_id: header["forked_from_id"].as_str().unwrap_or_default(),
                    cwd: Path::new(header["cwd"].as_str().unwrap_or_default()),
                    created_at_ms: header["created_at_ms"].as_u64().unwrap_or_default(),
                };
                intent.discover_candidate(&authority, &[candidate]);
            }
            let id = (self.next_id)();
            let mut commit = None;
            let result = intent.observe_hook(
                &authority,
                CodexForkHook {
                    event_name: observation.event,
                    provider_session_id: &identity_text,
                    cwd,
                },
                |binding, authority| {
                    let token = authority.lease.token.to_string();
                    let lease = native_fork_lease(&authority.lease, &token, &scope.generation)?;
                    let metadata = json!({
                        "providerConfigId": binding.provider_config_id, "model": binding.model,
                        "permissionMode": binding.permission_mode, "cwd": binding.cwd,
                    });
                    commit = Some(sessions::record_codex_fork_identity(
                        &mut self.connection,
                        &command(id.clone(), "provider-hook.codex-fork", id.clone()),
                        &sessions::ProviderIdentity {
                            binding_id: &id,
                            session_id: &scope.session_id,
                            run_id: &scope.run_id,
                            generation: &scope.generation,
                            provider_session_id: &identity_text,
                            expected_id: None,
                            launch_metadata: &metadata,
                            observed_metadata: &Value::Null,
                            event_name: observation.event,
                            first_for_run: true,
                        },
                        &lease,
                        now(),
                    )?);
                    Ok(())
                },
                |_| Ok(()),
            );
            match result {
                Ok(Some(_)) => {
                    let commit = commit.expect("Codex Fork committed its identity");
                    let mut confirmed = commit.result.clone();
                    confirmed["result"] = json!("confirmed");
                    codex.confirm(Some(rollout), identity_text.clone());
                    hook.deadline = None;
                    hook.fork = None;
                    observation.identity = Some(identity_text);
                    if let Some(raw) = observation.hud_payload.take() {
                        hook.hud.ingest_provider(&raw, now() as f64);
                    }
                    hook.hud.set_resumable(true);
                    hook.last_observation = Some(observation);
                    events.push(OwnerEvent::ProviderIdentity(confirmed));
                    events.push(OwnerEvent::Domain(commit));
                }
                Ok(None) => {}
                Err(error) => self.fail_codex_fork_run(
                    &scope.session_id,
                    &key,
                    &authority,
                    &format!("{error:#}"),
                    events,
                ),
            }
            return;
        }
        if let Some(resume) = &hook.codex_resume
            && resume.identity != identity
        {
            let binding_id = resume.binding_id.clone();
            reject_identity(events, &scope, "identity-mismatch");
            self.park_codex_run(
                &scope.session_id,
                &scope.run_id,
                &binding_id,
                codex_resume::identity_mismatch(),
                events,
            );
            return;
        }
        let expected = hook
            .codex_resume
            .as_ref()
            .map(|resume| resume.provider_session_id.as_str());
        let observed = observation
            .model
            .as_deref()
            .and_then(|model| safe_hook_text(model, &hook.secrets))
            .map_or_else(|| json!({}), |model| json!({"model": model}));
        let id = (self.next_id)();
        let commit = sessions::record_codex_identity(
            &mut self.connection,
            &command(
                format!("provider-hook-{}-{id}", scope.run_id),
                "provider-hook.identity",
                id.clone(),
            ),
            &sessions::ProviderIdentity {
                binding_id: &id,
                session_id: &scope.session_id,
                run_id: &scope.run_id,
                generation: &scope.generation,
                provider_session_id: expected.unwrap_or(&identity_text),
                expected_id: expected,
                launch_metadata: &hook.launch_metadata,
                observed_metadata: &observed,
                event_name: observation.event,
                first_for_run: true,
            },
            now(),
        );
        let Ok(commit) = commit else {
            events.push(OwnerEvent::ProviderIdentity(json!({
                "result": "rejected", "sessionId": scope.session_id, "runId": scope.run_id,
                "provider": "codex", "reason": "binding-write-failed",
            })));
            return;
        };
        if let Some(raw) = observation.hud_payload.take() {
            hook.hud.ingest_provider(&raw, now() as f64);
        }
        hook.hud.set_resumable(true);
        hook.codex_fork_ready = commit.result["metadata"]["canFork"] == true;
        codex.confirm(rollout, expected.unwrap_or(&identity_text).to_owned());
        hook.rescue_ready = true;
        hook.deadline = None;
        hook.codex_resume = None;
        observation.identity = Some(identity_text);
        hook.last_observation = Some(observation);
        let mut confirmed = commit.result.clone();
        confirmed["result"] = json!("confirmed");
        events.push(OwnerEvent::ProviderIdentity(confirmed));
        events.push(OwnerEvent::Domain(commit));
    }

    pub(crate) fn codex_hud(&self, session_id: &str) -> Option<Value> {
        let shell = self.shells.get(session_id)?;
        if shell.ended || shell.retiring || shell.worker.is_finished() {
            return None;
        }
        let hook = shell.hook.as_ref()?;
        let safe = |value: &str| safe_hook_text(value, &hook.secrets).is_some();
        let mut hud = hook.codex.as_ref()?.hud_projection(safe);
        if hud["model"].is_null()
            && let Some(model) = hook
                .last_observation
                .as_ref()
                .and_then(|observation| observation.model.as_deref())
                .filter(|model| safe(model))
        {
            hud["model"] = json!(model);
        }
        Some(hud)
    }

    pub(crate) fn poll_codex_hud(&mut self) -> bool {
        if self.closing {
            return false;
        }
        if let Some(reply) = self.codex_hud.poll()
            && let Some(shell) = self.shells.get_mut(&reply.session).filter(|shell| {
                !shell.ended
                    && !shell.retiring
                    && shell.run_id == reply.run
                    && !shell.worker.is_finished()
            })
            && let Some(codex) = shell.hook.as_mut().and_then(|hook| hook.codex.as_mut())
        {
            codex.accept_hud(reply);
        }
        let now = Instant::now();
        let mut changed = false;
        let mut next = None;
        for (session, shell) in &mut self.shells {
            if shell.ended || shell.retiring || shell.worker.is_finished() {
                continue;
            }
            if let Some(hook) = &mut shell.hook
                && let Some(codex) = &mut hook.codex
            {
                if codex.take_completed_compaction() {
                    hook.context_guard.on_compaction();
                }
                if codex.take_hud_change() {
                    let projection = codex.hud_projection(|_| true);
                    hook.codex_context_percent = projection["contextPercent"].as_f64();
                    let tool = |value: &Value| {
                        Some(RunningTool {
                            name: value["name"].as_str()?.to_owned(),
                            target: value["target"].as_str().map(str::to_owned),
                        })
                    };
                    hook.codex_context_tools = projection["runningTools"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(tool)
                        .collect();
                    let last = &projection["lastTool"];
                    hook.codex_context_last_tool = tool(last).and_then(|tool| {
                        let status = match last["status"].as_str()? {
                            "completed" => ToolStatus::Completed,
                            "error" => ToolStatus::Error,
                            "running" => ToolStatus::Running,
                            _ => return None,
                        };
                        Some(ToolActivity { tool, status })
                    });
                    changed = true;
                }
                if self.codex_hud.idle()
                    && let Some(due) = codex.hud_due().filter(|due| *due <= now)
                    && next
                        .as_ref()
                        .is_none_or(|(earliest, _, _, _)| due < *earliest)
                {
                    next = Some((due, session, &shell.run_id, codex));
                }
            }
        }
        if let Some((_, session, run, codex)) = next
            && let Some(request) = codex.read_hud(session.clone(), run.clone())
            && let Some(reply) = self.codex_hud.submit(request)
        {
            codex.accept_hud(reply);
        }
        changed
    }

    fn poll_codex_titles(&mut self, events: &mut Vec<OwnerEvent>) {
        if self.closing {
            return;
        }
        if let Some(observation) = self.codex_titles.poll()
            && let Some(shell) = self.shells.get_mut(&observation.session).filter(|shell| {
                !shell.ended
                    && !shell.retiring
                    && shell.run_id == observation.run
                    && !shell.worker.is_finished()
            })
            && let Some(hook) = shell.hook.as_mut()
            && let Some(watch) = hook
                .codex
                .as_mut()
                .and_then(|codex| codex.title.as_mut())
                .filter(|watch| watch.identity == observation.identity)
            && let Some(title) = watch.accept(observation.read)
            && let Some(title) = safe_hook_text(&title, &hook.secrets)
        {
            let id = (self.next_id)();
            match sessions::observe_codex_title(
                &mut self.connection,
                &command(
                    format!("codex-title-{id}"),
                    "session.observe-provider-title",
                    id,
                ),
                &observation.session,
                &observation.run,
                &self.generation,
                &observation.identity,
                title,
                now(),
            ) {
                Ok(Some(commit)) => events.push(OwnerEvent::Domain(commit)),
                Ok(None) => {}
                Err(error) => {
                    watch.retry();
                    diagnostic(
                        events,
                        &observation.session,
                        format!("persist Codex title: {error:#}"),
                    );
                }
            }
        }
        if !self.codex_titles.idle() {
            return;
        }
        let now = Instant::now();
        let next = self
            .shells
            .iter_mut()
            .filter(|(_, shell)| !shell.ended && !shell.retiring && !shell.worker.is_finished())
            .filter_map(|(id, shell)| {
                let watch = shell.hook.as_mut()?.codex.as_mut()?.title.as_mut()?;
                watch.ready(now).then_some((id, &shell.run_id, watch))
            })
            .min_by_key(|(_, _, watch)| watch.due);
        if let Some((id, run, watch)) = next {
            self.codex_titles
                .submit(watch.request(id.clone(), run.clone(), now));
        }
    }

    pub(crate) fn set_session_title(
        &mut self,
        session_id: &str,
        title: Option<&str>,
    ) -> Result<Option<DomainCommit>> {
        let id = (self.next_id)();
        let time = now();
        if let Some(title) = title {
            sessions::rename_session(
                &mut self.connection,
                &command(format!("rename-card-{id}"), "hierarchy.rename-session", id),
                session_id,
                title,
                time,
                time,
            )
            .map(Some)
        } else {
            sessions::restore_codex_title(
                &mut self.connection,
                &command(
                    format!("restore-card-title-{id}"),
                    "hierarchy.restore-session-auto-title",
                    id,
                ),
                session_id,
                time,
            )
        }
    }

    fn apply_codex_effect(
        &mut self,
        session_id: &str,
        run_id: &str,
        effect: codex_events::Effect,
        hook_notification: Option<Value>,
        events: &mut Vec<OwnerEvent>,
    ) {
        if let Err(error) = self.set_work_status(session_id, events, |_| Some(effect.status)) {
            diagnostic(
                events,
                session_id,
                format!("persist Codex work status: {error:#}"),
            );
            return;
        }
        let notification = effect
            .error
            .or_else(|| hook_notification.filter(|_| effect.notify));
        if let Some(mut notification) = notification {
            let secrets = &self.shells[session_id]
                .hook
                .as_ref()
                .expect("Codex hook")
                .secrets;
            for field in ["body", "subtitle", "replacementKey"] {
                if let Some(value) = notification[field].as_str()
                    && safe_hook_text(value, secrets).is_none()
                {
                    notification[field] = json!("[redacted]");
                }
            }
            let id = (self.next_id)();
            let commit = sessions::publish_provider_notification(
                &mut self.connection,
                &command(
                    format!("agent-notification-{id}"),
                    "agent.notification.publish",
                    id,
                ),
                session_id,
                run_id,
                &self.generation,
                "codex",
                notification,
                now(),
            );
            match commit {
                Ok(commit) if !commit.replayed && commit.first_event_sequence.is_some() => {
                    events.push(OwnerEvent::Domain(commit));
                }
                Ok(_) => {}
                Err(error) => diagnostic(
                    events,
                    session_id,
                    format!("persist Codex notification: {error:#}"),
                ),
            }
        }
    }

    fn park_provider_run(
        &mut self,
        session_id: &str,
        run_id: &str,
        binding_id: &str,
        error: &str,
        events: &mut Vec<OwnerEvent>,
    ) {
        let mut error = self
            .shells
            .get(session_id)
            .filter(|shell| shell.run_id == run_id)
            .and_then(|shell| shell.hook.as_ref())
            .and_then(|hook| hook.resume.as_ref())
            .map_or_else(|| error.to_owned(), |resume| resume.failure_message(error));
        if error == SESSION_NOT_FOUND {
            let rebuild = (|| {
                let Some(hook) = self
                    .shells
                    .get(session_id)
                    .filter(|shell| shell.run_id == run_id && !shell.retiring)
                    .and_then(|shell| shell.hook.as_ref())
                    .filter(|hook| hook.last_observation.is_none())
                else {
                    return Ok(None);
                };
                let Some(rebuild) = &hook.claude_fork_rebuild else {
                    return Ok(None);
                };
                let Some(binding) = self
                    .claude_binding(session_id)?
                    .filter(|binding| binding.id == binding_id && binding.resumable)
                else {
                    return Ok(None);
                };
                let parent = provider_resume::empty_fork_parent(
                    &self.connection,
                    session_id,
                    &binding,
                    &error,
                )?;
                if parent.as_deref() != Some(rebuild.parent.as_str())
                    || rebuild.child != binding.provider_session_id
                {
                    return Ok(None);
                }
                if let Some(blocker) = provider_resume::fork_rebuild_blocker(
                    hook.claude_projects.as_deref(),
                    &rebuild.parent,
                    &rebuild.child,
                ) {
                    error = blocker;
                    return Ok(None);
                }
                self.commit_claude_restore(
                    session_id,
                    ClaudeRestore::Begin(binding_id),
                    Some(run_id),
                    events,
                )?;
                Ok::<_, anyhow::Error>(
                    self.shells
                        .get_mut(session_id)
                        .and_then(|shell| shell.hook.as_mut())
                        .and_then(|hook| hook.claude_fork_rebuild.take()),
                )
            })();
            match rebuild {
                Ok(Some(rebuild)) => {
                    self.retire_sessions(&[session_id.to_owned()]);
                    events.push(OwnerEvent::RebuildClaudeFork {
                        session_id: session_id.to_owned(),
                        rebuild: Box::new(rebuild),
                    });
                    return;
                }
                Ok(None) => {}
                Err(error) => diagnostic(
                    events,
                    session_id,
                    format!("Prepare Claude fork rebuild: {error:#}"),
                ),
            }
        }
        self.retire_sessions(&[session_id.to_owned()]);
        if let Err(error) = self.commit_claude_restore(
            session_id,
            ClaudeRestore::Fail {
                binding_id,
                error: &error,
            },
            Some(run_id),
            events,
        ) {
            diagnostic(
                events,
                session_id,
                format!("persist provider restore failure: {error:#}"),
            );
        }
    }

    fn park_fork_run(
        &mut self,
        session_id: &str,
        run_id: &str,
        error: &str,
        events: &mut Vec<OwnerEvent>,
    ) {
        self.retire_sessions(&[session_id.to_owned()]);
        if let Err(error) = self.commit_fork_failure(session_id, Some(run_id), error, events) {
            diagnostic(
                events,
                session_id,
                format!("persist fork failure: {error:#}"),
            );
        }
    }

    fn commit_fork_failure(
        &mut self,
        session_id: &str,
        run_id: Option<&str>,
        error: &str,
        events: &mut Vec<OwnerEvent>,
    ) -> Result<bool> {
        let id = (self.next_id)();
        let commit = sessions::fail_claude_fork(
            &mut self.connection,
            &command(id.clone(), "session.fork-failed", id),
            session_id,
            error,
            now(),
        )?;
        if commit.result.is_null() {
            return Ok(false);
        }
        let mut payload = commit.result.clone();
        if let Some(run_id) = run_id {
            payload["runId"] = json!(run_id);
        }
        events.push(OwnerEvent::ProviderIdentity(payload));
        events.push(OwnerEvent::Domain(commit));
        Ok(true)
    }

    fn apply(&mut self, event: WorkerEvent, events: &mut Vec<OwnerEvent>) {
        // A failed run-start transaction has already shut down its unregistered PTY.
        let Some(shell) = self
            .shells
            .get(&event.session_id)
            .filter(|shell| shell.run_id == event.run_id)
        else {
            return;
        };
        if shell.retiring {
            match &event.event {
                SessionEvent::SavedExit { .. } => {}
                SessionEvent::Diagnostic { message } => {
                    diagnostic(events, &event.session_id, message.clone());
                    return;
                }
                _ => return,
            }
        }
        let result = match &event.event {
            SessionEvent::InputSubmitted(submitted_at) => {
                if shell.ended || shell.hook.as_ref().is_some_and(|hook| hook.codex.is_some()) {
                    return;
                }
                let shell = self
                    .shells
                    .get_mut(&event.session_id)
                    .expect("live session");
                if shell.is_shell {
                    shell.cwd_capture = Some((Instant::now() + Duration::from_millis(180), true));
                }
                if let Some(hook) = &mut shell.hook {
                    hook.last_submission = Some(*submitted_at);
                    hook.claude_waiting_since_ms = None;
                }
                if let Err(error) =
                    self.set_work_status(&event.session_id, events, |_| Some("running"))
                {
                    diagnostic(
                        events,
                        &event.session_id,
                        format!("persist submitted terminal input: {error:#}"),
                    );
                }
                return;
            }
            SessionEvent::CwdCaptured { run_id, cwd } => {
                if !self.closing
                    && self
                        .shells
                        .get(&event.session_id)
                        .is_some_and(|shell| &shell.run_id == run_id && !shell.ended)
                {
                    if let Err(error) = self.persist_cwd(&event.session_id, cwd, events) {
                        diagnostic(
                            events,
                            &event.session_id,
                            format!("persist captured Shell cwd: {error:#}"),
                        );
                    }
                }
                return;
            }
            SessionEvent::SavedOutput {
                data,
                completed_blocks,
                ..
            } => {
                self.apply_output(&event.session_id, data, completed_blocks, events);
                return;
            }
            SessionEvent::SavedExit {
                status,
                disposed,
                observed_at,
                ..
            } => self.finish_run(
                &event.session_id,
                *status,
                *disposed,
                *observed_at,
                false,
                events,
            ),
            SessionEvent::StorageEndResult {
                result,
                observed_at,
            } => {
                let result = result.clone().and_then(|()| {
                    // The journal cannot accept an exit frame. Record the
                    // explicitly ended run through the normal domain exit path.
                    let status = matou_native_core::pty::ProcessExit {
                        exit_code: 1,
                        signal: None,
                    };
                    self.finish_run(&event.session_id, status, true, *observed_at, true, events)
                        .map_err(|error| format!("{error:#}"))
                });
                let observed_at = *observed_at;
                events.push(OwnerEvent::Terminal(WorkerEvent {
                    event: SessionEvent::StorageEndResult {
                        result,
                        observed_at,
                    },
                    ..event
                }));
                return;
            }
            _ => {
                events.push(OwnerEvent::Terminal(event));
                return;
            }
        };
        if let Err(error) = result {
            if let Some(shell) = self.shells.get_mut(&event.session_id) {
                shell.exit_error = Some(format!("{error:#}"));
            }
            diagnostic(
                events,
                &event.session_id,
                format!("persist Shell exit: {error:#}"),
            );
        }
    }

    fn apply_output(
        &mut self,
        id: &str,
        data: &str,
        blocks: &[matou_native_core::shell_history::CollectedShellBlock],
        events: &mut Vec<OwnerEvent>,
    ) {
        let shell = self.shells.get_mut(id).expect("registered session output");
        shell.emitted_output = true;
        shell.last_output = Some(Instant::now());
        #[cfg(any(unix, windows))]
        if !self.closing
            && let Some(input) = shell.alias_preheat.take()
        {
            // The PTY's own startup gets the first chance to run shell setup.
            input.preheat_cc();
        }
        if !self.closing
            && let Some(resume) = shell
                .hook
                .as_mut()
                .and_then(|hook| hook.codex_resume.as_mut())
            && let Some(error) = resume.ingest(data)
        {
            let run_id = shell.run_id.clone();
            let binding_id = resume.binding_id.clone();
            self.park_codex_run(id, &run_id, &binding_id, error, events);
            return;
        }
        if !self.closing
            && let Some(hook) = &mut shell.hook
            && hook.last_observation.is_none()
            && let Some(resume) = &mut hook.resume
        {
            let (failure, input) = resume.ingest(data);
            if let Some(error) = failure {
                let run_id = shell.run_id.clone();
                let binding_id = resume.binding_id.clone();
                if let Some(binding_id) = binding_id {
                    self.park_provider_run(id, &run_id, &binding_id, error, events);
                } else {
                    self.park_fork_run(id, &run_id, error, events);
                }
                return;
            }
            if let Some(bytes) = input {
                write_resume_input(id, shell, bytes, events);
            }
        }
        if !shell.is_shell
            && shell
                .hook
                .as_ref()
                .is_none_or(|hook| hook.last_observation.is_none())
        {
            return;
        }
        let summary = self.summaries.entry(id.to_owned()).or_default();
        summary.raw.push_str(data);
        summary.raw = utf16_tail(&summary.raw, 32_768);
        summary
            .deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(180));
        if !shell.is_shell {
            if !self.closing
                && let Some(effect) = shell
                    .hook
                    .as_mut()
                    .and_then(|hook| hook.codex.as_mut())
                    .and_then(|codex| codex.output(data))
            {
                let run_id = shell.run_id.clone();
                self.apply_codex_effect(id, &run_id, effect, None, events);
            }
            return;
        }
        let mut completed = false;
        if shell.restore_history {
            for block in blocks {
                match shell_history::complete(
                    &mut self.connection,
                    &(self.next_id)(),
                    id,
                    &shell.cwd,
                    block.clone(),
                ) {
                    Ok(_) => completed = true,
                    Err(error) => diagnostic(
                        events,
                        id,
                        format!("save completed Shell history: {error:#}"),
                    ),
                }
            }
        }
        let cwd = shell.cwd_tracker.ingest(data);
        let statuses = shell.work_tracker.ingest(data);
        if completed {
            self.flush_summary(id, events);
        }
        if let Some(cwd) = cwd {
            if let Err(error) = self.persist_cwd(id, &cwd, events) {
                diagnostic(events, id, format!("persist Shell cwd: {error:#}"));
            }
        }
        for status in statuses {
            if status == "error" {
                self.flush_summary(id, events);
            }
            if let Err(error) = self.set_work_status(id, events, |_| Some(&status)) {
                diagnostic(events, id, format!("persist Shell work status: {error:#}"));
            }
        }
    }

    fn persist_cwd(&mut self, id: &str, cwd: &str, events: &mut Vec<OwnerEvent>) -> Result<()> {
        if !Path::new(cwd).is_dir() {
            return Ok(());
        }
        self.commit_cwd(id, cwd, events)
    }

    fn commit_cwd(&mut self, id: &str, cwd: &str, events: &mut Vec<OwnerEvent>) -> Result<()> {
        let Some(session) = self.session(id)? else {
            return Ok(());
        };
        if session.cwd == cwd {
            return Ok(());
        }
        let time = now();
        let token = (self.next_id)();
        let commit = sessions::update_cwd(
            &mut self.connection,
            &command(
                format!("runtime-cwd-{id}-{time}-{token}"),
                "session.cwd-update",
                format!("cwd:{id}:{cwd}:{time}"),
            ),
            id,
            cwd,
            time,
            time,
        )?;
        events.push(OwnerEvent::Domain(commit));
        Ok(())
    }

    fn set_work_status<'a>(
        &mut self,
        id: &str,
        events: &mut Vec<OwnerEvent>,
        next_status: impl FnOnce(&str) -> Option<&'a str>,
    ) -> Result<()> {
        let Some(current): Option<String> = self
            .connection
            .query_row(
                "SELECT work_status FROM sessions WHERE id=? AND archived_at IS NULL",
                [id],
                |row| row.get(0),
            )
            .optional()?
        else {
            return Ok(());
        };
        let Some(status) = next_status(&current) else {
            return Ok(());
        };
        if current == status {
            return Ok(());
        }
        let time = now();
        let token = (self.next_id)();
        let commit = session_graph::set_work_status(
            &mut self.connection,
            &command(
                format!("runtime-work-status-{id}-{status}-{time}-{token}"),
                "session.work-status",
                format!("{id}:{status}:{time}"),
            ),
            id,
            status,
            time,
            time,
        )?;
        events.push(OwnerEvent::Domain(commit));
        Ok(())
    }

    fn finish_run(
        &mut self,
        id: &str,
        status: matou_native_core::pty::ProcessExit,
        disposed: bool,
        observed_at: Instant,
        storage_ended: bool,
        events: &mut Vec<OwnerEvent>,
    ) -> Result<()> {
        if let Some(control) = &self.run_control {
            control.revoke(&self.shells[id].run_id);
        }
        let retiring = self.shells[id].retiring;
        let promotion_verified = self.shells[id].promotion_verified();
        if !storage_ended && !self.closing && promotion_verified {
            // The worker publishes the check result before emitting its exit.
            // Drain accepted hooks while this run still owns its binding.
            let run_id = self.shells[id].run_id.clone();
            self.poll_provider_hook_messages(
                events,
                HOOK_QUEUE_CAPACITY,
                |_, scope, received_at| scope.run_id != run_id || received_at <= observed_at,
            );
            self.apply_startup_observations(id, &run_id, events);
        }
        if !retiring {
            self.flush_summary(id, events);
        }
        let shell = self.shells.get_mut(id).expect("registered Shell exit");
        if shell.ended {
            return Ok(());
        }
        // If writing the domain exit also fails, a repeated storage-end
        // request can finish recording it after storage is available again.
        shell.ended = !storage_ended;
        #[cfg(any(unix, windows))]
        {
            shell.alias_preheat = None;
        }
        self.egress.unregister_session(id, now() as u64);
        shell.gateway.take();
        let mut hook = shell.hook.take();
        self.egress_membership_changed |= hook
            .as_ref()
            .is_some_and(|hook| tracks_egress(hook.profile));
        let pending_dsh = hook.as_mut().and_then(|hook| {
            if let Some(DshObservation::Resume(resume, binding)) = &mut hook.dsh
                && resume.fail_start(&shell.run_id, ResumeFailure::ProcessExited)
            {
                return Some(binding.clone());
            }
            None
        });
        let pending_antigravity = hook
            .as_mut()
            .and_then(|hook| hook.antigravity.as_mut())
            .map(|agy| (agy.binding.take(), agy.fork_pending));
        let pending_codex_fork = hook.as_mut().and_then(|hook| hook.fork.take());
        let pending_codex = hook.as_mut().and_then(|hook| hook.codex_resume.take());
        let had_identity = hook
            .as_ref()
            .is_some_and(|hook| hook.last_observation.is_some());
        let pending_restore = hook
            .and_then(|hook| hook.resume)
            .filter(|resume| !had_identity && resume.tracks_restore());
        shell.cwd_capture = None;
        let shutdown = disposed && self.closing && !retiring && !storage_ended;
        let run = shell.run_id.clone();
        let pid = shell.worker.process_id();
        if shell.environment_stop {
            let time = now();
            let commit = sessions::interrupt_run_for_environment(
                &mut self.connection,
                &command(
                    format!("runtime-environment-transition-{run}"),
                    "session.environment-run-interrupted",
                    format!("{run}:{pid}"),
                ),
                &run,
                time,
            )?;
            events.push(OwnerEvent::Domain(commit));
            return Ok(());
        }
        let startup_failed = !shell.emitted_output
            && !shutdown
            && !retiring
            && !promotion_verified
            && (shell.is_shell || had_identity);
        let provider_startup_pending = shell.provider_startup_pending;
        if shutdown && shell.is_shell {
            let work: String = self.connection.query_row(
                "SELECT work_status FROM sessions WHERE id=?",
                [id],
                |row| row.get(0),
            )?;
            if matches!(work.as_str(), "starting" | "running" | "needs-input") {
                return Ok(());
            }
        }
        let time = now();
        let (prefix, hash) = if storage_ended {
            ("runtime-storage-end", "storage-end")
        } else if shutdown {
            ("runtime-shutdown", "shutdown")
        } else {
            ("runtime-exit", "exit")
        };
        if shell.catalog_stop {
            let exit = CatalogExit {
                run,
                pid,
                status,
                time,
            };
            match exit.persist(&mut self.connection) {
                Ok(commit) => events.push(OwnerEvent::Domain(commit)),
                Err(error) => {
                    self.catalog_exit_retries.insert(id.to_owned(), exit);
                    return Err(error);
                }
            }
            return Ok(());
        }
        let commit = sessions::finish_run(
            &mut self.connection,
            &command(
                format!("{prefix}-{run}"),
                "session.run-exit",
                format!("{hash}:{run}:{pid}"),
            ),
            &run,
            i64::from(status.exit_code),
            status.signal.map(i64::from),
            time,
            time,
        )?;
        events.push(OwnerEvent::Domain(commit));
        if storage_ended {
            self.set_work_status(id, events, |_| Some("exited"))?;
            self.shells
                .get_mut(id)
                .expect("registered Shell exit")
                .ended = true;
            return Ok(());
        }
        if let Some((binding, fork)) = pending_antigravity
            && !retiring
            && !disposed
            && !self.closing
        {
            let error = "Antigravity exited before confirming the conversation";
            if fork {
                self.retire_sessions(&[id.to_owned()]);
                self.commit_fork_failure(id, Some(&run), error, events)?;
                return Ok(());
            }
            if let Some(binding_id) = binding {
                self.retire_sessions(&[id.to_owned()]);
                self.commit_antigravity_restore(
                    id,
                    ClaudeRestore::Fail {
                        binding_id: &binding_id,
                        error,
                    },
                    Some(&run),
                    events,
                )?;
                return Ok(());
            }
        }
        if let Some((key, authority)) = pending_codex_fork {
            let reason = "Codex Fork exited before confirming its child identity";
            if let Some(intent) = self.codex_forks.get_mut(&key) {
                intent.fail(&authority, &anyhow::anyhow!("{reason}"), |_| Ok(()))?;
            }
            self.commit_fork_failure(id, Some(&run), reason, events)?;
            return Ok(());
        }
        if let Some(binding_id) = pending_dsh
            && !retiring
            && !disposed
            && !self.closing
        {
            self.change_dsh_restore(
                id,
                ClaudeRestore::Fail {
                    binding_id: &binding_id,
                    error: "DSH exited before confirming the restored session",
                },
            )?;
            return Ok(());
        }
        if let Some(resume) = pending_codex
            && !retiring
            && !disposed
            && !self.closing
        {
            self.park_codex_run(
                id,
                &run,
                &resume.binding_id,
                &format!("provider resume exited with code {}", status.exit_code),
                events,
            );
            return Ok(());
        }
        if provider_startup_pending
            || pending_restore
                .as_ref()
                .is_some_and(|resume| resume.saw_bypass_consent() || resume.fresh_permission())
        {
            return Ok(());
        }
        if let Some(resume) = pending_restore.filter(|_| !retiring) {
            if let Some(binding_id) = &resume.binding_id {
                if !disposed && !self.closing {
                    self.park_provider_run(
                        id,
                        &run,
                        binding_id,
                        &resume.failure_message(&format!(
                            "provider resume exited with code {}",
                            status.exit_code
                        )),
                        events,
                    );
                }
            } else {
                self.park_fork_run(
                    id,
                    &run,
                    &format!(
                        "CLI exited with code {} before confirming the fork identity",
                        status.exit_code
                    ),
                    events,
                );
            }
        } else if startup_failed {
            self.set_work_status(id, events, |_| Some("error"))?;
        }
        Ok(())
    }

    fn start_due_cwd_captures(&mut self) {
        if self.closing {
            return;
        }
        for (id, shell) in &mut self.shells {
            let Some((deadline, first)) = shell.cwd_capture else {
                continue;
            };
            if deadline > Instant::now() || shell.ended {
                continue;
            }
            shell.cwd_capture =
                first.then(|| (Instant::now() + Duration::from_millis(1_050), false));
            let pid = shell.worker.process_id();
            let id = id.clone();
            let run_id = shell.run_id.clone();
            let send = self.send.clone();
            // Match the existing asynchronous OS query; it must not block unrelated shells.
            std::thread::spawn(move || {
                if let Some(cwd) = process_working_directory(pid) {
                    let _ = send.send(WorkerEvent {
                        session_id: id,
                        run_id: run_id.clone(),
                        event: SessionEvent::CwdCaptured { run_id, cwd },
                    });
                }
            });
        }
    }

    fn flush_summary(&mut self, id: &str, events: &mut Vec<OwnerEvent>) {
        let Some(summary) = self.summaries.get(id) else {
            return;
        };
        let lines = terminal_summary_lines(&summary.raw);
        if lines.is_empty() {
            return;
        }
        let result = (|| {
            self.connection.execute("INSERT INTO session_graph_summaries(session_id,latest_lines_json,updated_at) VALUES (?,?,?) ON CONFLICT(session_id) DO UPDATE SET latest_lines_json=excluded.latest_lines_json,updated_at=excluded.updated_at",params![id,serde_json::to_string(&lines)?,now()])?;
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = result {
            diagnostic(events, id, format!("save Shell graph summary: {error:#}"));
        }
    }

    fn flush_due_summaries(&mut self, events: &mut Vec<OwnerEvent>) {
        let due: Vec<_> = self
            .summaries
            .iter_mut()
            .filter_map(|(id, summary)| {
                if summary
                    .deadline
                    .is_some_and(|deadline| deadline <= Instant::now())
                {
                    summary.deadline = None;
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        for id in due {
            self.flush_summary(&id, events);
        }
    }

    pub fn shutdown(&mut self) -> Result<Vec<OwnerEvent>> {
        let mut events = std::mem::take(&mut self.pending);
        if !self.closing && self.shells.values().any(RunningShell::promotion_verified) {
            // Apply an already queued exit first so its hook cutoff still holds.
            let queued: Vec<_> = self.receive.try_iter().collect();
            for event in queued {
                self.apply(event, &mut events);
            }
        }
        self.closing = true;
        for shell in self.shells.values_mut() {
            if let Some(control) = &self.run_control {
                control.revoke(&shell.run_id);
            }
            shell.gateway.take();
            if !shell.promotion_verified() {
                shell.hook = None;
            }
        }
        // Request every active Shell to stop before waiting for any worker.
        let shutdowns: Vec<_> = self
            .shells
            .values_mut()
            .filter(|shell| !shell.retiring)
            .filter_map(|shell| shell.worker.begin_shutdown())
            .collect();
        let retiring = self
            .shells
            .values_mut()
            .filter(|shell| shell.retiring)
            .map(|shell| shell.worker.join_after_shutdown());
        let mut first_error = None;
        for result in shutdowns
            .into_iter()
            .map(|shutdown| shutdown.wait())
            .chain(retiring)
        {
            if let Err(error) = result {
                #[cfg(unix)]
                if error.is::<crate::session::PtyShutdownIncomplete>() {
                    first_error = Some(error);
                    continue;
                }
                first_error.get_or_insert(error);
            }
        }
        if let Some(server) = &mut self.hook_server {
            server.finish_requests();
        }
        let queued: Vec<_> = self.receive.try_iter().collect();
        let exit_times: HashMap<_, _> = queued
            .iter()
            .filter_map(|event| match &event.event {
                SessionEvent::SavedExit { observed_at, .. } => Some((&event.run_id, *observed_at)),
                _ => None,
            })
            .collect();
        // Finish admitted hooks while their runs still own the bindings.
        // Each worker's observed exit is the cutoff for that run's messages.
        self.poll_provider_hook_messages(
            &mut events,
            HOOK_QUEUE_CAPACITY,
            |owner, scope, received_at| {
                owner.shells.get(&scope.session_id).is_some_and(|shell| {
                    shell.run_id == scope.run_id
                        && shell.promotion_verified()
                        && exit_times
                            .get(&scope.run_id)
                            .is_none_or(|cutoff| received_at <= *cutoff)
                })
            },
        );
        let verified: Vec<_> = self
            .shells
            .iter()
            .filter(|(_, shell)| shell.provider_startup_pending && shell.promotion_verified())
            .map(|(id, shell)| (id.clone(), shell.run_id.clone()))
            .collect();
        for (id, run_id) in verified {
            self.apply_startup_observations(&id, &run_id, &mut events);
        }
        for event in queued {
            self.apply(event, &mut events);
        }
        let ids: Vec<_> = self.summaries.keys().cloned().collect();
        for id in ids {
            self.flush_summary(&id, &mut events);
        }
        self.shells.clear();
        self.hook_server = None;
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(events)
    }

    pub(crate) fn backup_for_clean_exit(&self, data_root: &Path) -> Result<()> {
        BackupService::new(data_root)
            .and_then(|mut backups| {
                backups.backup_for_clean_exit(&self.connection, now(), CLEAN_EXIT_BACKUP_TIMEOUT)
            })
            .map(|_| ())
            .with_context(|| format!("clean exit backup failed for {}", data_root.display()))
    }
}

impl Drop for ShellOwner {
    fn drop(&mut self) {
        // After explicit shutdown the snapshot already contains the final summary.
        if !self.closing || !self.shells.is_empty() {
            let _ = self.shutdown();
        }
    }
}

fn safe_hook_text<'a>(value: &'a str, secrets: &[String]) -> Option<&'a str> {
    (value.len() <= 4096
        && !value.chars().any(char::is_control)
        && !secrets.iter().any(|secret| value.contains(secret)))
    .then_some(value)
}

fn uses_default_claude_login(
    provider: Option<&str>,
    launch: &ShellLaunch,
    inherited_endpoint_override: bool,
) -> bool {
    let env = |key| get_var(&launch.env, key).or_else(|| get_var(&launch.inherited_env, key));
    matches!(provider, None | Some("" | "anthropic-official"))
        && !inherited_endpoint_override
        // Native launch supplies this endpoint even for the built-in account.
        && env("ANTHROPIC_BASE_URL")
            .is_none_or(|url| url.is_empty() || url == "https://api.anthropic.com")
        && ![
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        ]
        .into_iter()
        .any(|key| env(key).is_some_and(|value| !value.is_empty()))
}

#[test]
fn default_claude_login_requires_builtin_provider_and_no_auth_override() {
    let mut launch = ShellLaunch {
        profile: LaunchProfile::ClaudeCode,
        executable: PathBuf::new(),
        argv: Vec::new(),
        cwd: PathBuf::new(),
        data_root: PathBuf::new(),
        inherited_env: Default::default(),
        env: [(
            "ANTHROPIC_BASE_URL".into(),
            "https://api.anthropic.com".into(),
        )]
        .into(),
        cols: 80,
        rows: 24,
    };
    assert!(uses_default_claude_login(None, &launch, false));
    let official = Some("anthropic-official");
    assert!(uses_default_claude_login(official, &launch, false));
    assert!(!uses_default_claude_login(official, &launch, true));
    assert!(!uses_default_claude_login(Some("custom"), &launch, false));
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ] {
        launch.inherited_env.insert(key.into(), "fixture".into());
        assert!(
            !uses_default_claude_login(official, &launch, false),
            "{key}"
        );
        launch.env.insert(key.into(), "".into());
        assert!(uses_default_claude_login(official, &launch, false), "{key}");
        launch.env.remove(std::ffi::OsStr::new(key));
        launch.inherited_env.remove(std::ffi::OsStr::new(key));
    }
    launch.env.insert(
        "ANTHROPIC_BASE_URL".into(),
        "https://example.invalid".into(),
    );
    assert!(!uses_default_claude_login(official, &launch, false));
}

fn reject_identity(events: &mut Vec<OwnerEvent>, scope: &Scope, reason: &str) {
    events.push(OwnerEvent::ProviderIdentity(json!({
        "result": "rejected", "sessionId": scope.session_id, "runId": scope.run_id,
        "provider": scope.profile.kind(), "reason": reason,
    })));
}

fn tracks_egress(profile: LaunchProfile) -> bool {
    matches!(
        profile,
        LaunchProfile::ClaudeCode | LaunchProfile::Codex | LaunchProfile::Antigravity
    )
}

fn claude_projects(launch: &ShellLaunch) -> Option<PathBuf> {
    get_var(&launch.env, "CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(|path| launch.cwd.join(path).join("projects"))
        .or_else(|| {
            let home = get_var(&launch.env, "HOME").filter(|path| !path.is_empty());
            #[cfg(windows)]
            let home = home
                .or_else(|| get_var(&launch.env, "USERPROFILE").filter(|path| !path.is_empty()));
            home.map(|home| launch.cwd.join(home).join(".claude/projects"))
        })
        .or_else(crate::provider_catalog::projects_root)
}

fn launch_egress_families(
    profile: LaunchProfile,
    environment: &std::collections::BTreeMap<String, String>,
    gateway: Option<&ClaudeGatewayProjection>,
) -> Vec<Family> {
    let subscription = gateway
        .and_then(|gateway| {
            gateway
                .source
                .as_ref()
                .and_then(|source| source.subscription.as_ref())
                .or_else(|| {
                    // A role snapshot with no source uses only its explicit connections.
                    gateway
                        .source
                        .is_none()
                        .then(|| {
                            gateway
                                .connections()
                                .find_map(|target| target.subscription.as_ref())
                        })
                        .flatten()
                })
        })
        .map(|target| target.provider);
    let mut families: Vec<_> = egress_probe::launch_plan(profile.kind(), environment, subscription)
        .family
        .into_iter()
        .collect();
    // The effective gateway can route one Claude run to several upstreams.
    // Classify the configured targets before the CLI sees a loopback URL.
    for connection in gateway
        .into_iter()
        .flat_map(ClaudeGatewayProjection::connections)
    {
        if connection.subscription.is_some() {
            continue;
        }
        let family = match connection.endpoint.host_str() {
            Some("api.anthropic.com") => Family::Anthropic,
            Some("api.openai.com") => Family::OpenAi,
            Some("chatgpt.com") => Family::ChatgptSubscription,
            Some(
                "generativelanguage.googleapis.com"
                | "aiplatform.googleapis.com"
                | "cloudcode-pa.googleapis.com",
            ) => Family::Google,
            _ => continue,
        };
        if !families.contains(&family) {
            families.push(family);
        }
    }
    for provider in gateway
        .into_iter()
        .flat_map(ClaudeGatewayProjection::subscription_providers)
    {
        if let Some(family) =
            egress_probe::launch_plan(profile.kind(), environment, Some(provider)).family
            && !families.contains(&family)
        {
            families.push(family);
        }
    }
    families
}

fn write_resume_input(
    session_id: &str,
    shell: &RunningShell,
    bytes: &[u8],
    events: &mut Vec<OwnerEvent>,
) {
    let (reply, _) = mpsc::channel();
    if let Err(error) = shell.worker.send(WorkerCommand::Write {
        bytes: bytes.to_vec(),
        origin: InputOrigin::System,
        reply,
    }) {
        diagnostic(
            events,
            session_id,
            format!("answer provider startup prompt: {error:#}"),
        );
    }
}

fn command(command_id: String, kind: &str, request_hash: String) -> CommandMetadata {
    CommandMetadata {
        command_id,
        command_type: kind.into(),
        request_hash,
        causation_id: None,
        correlation_id: None,
    }
}
fn native_fork_lease<'a>(
    lease: &'a ForkLease,
    token: &'a str,
    owner: &'a str,
) -> Result<canvas::NativeForkLease<'a>> {
    Ok(canvas::NativeForkLease {
        operation_id: &lease.operation_id,
        token,
        fence: lease.fence.try_into()?,
        owner,
    })
}

fn core_worktree(binding: &WorktreeBinding) -> Result<canvas::NativeForkWorktree> {
    Ok(canvas::NativeForkWorktree {
        id: binding.worktree_id.clone(),
        execution_context_id: binding.execution_context_id.clone(),
        repository_root: binding
            .repository_root
            .to_str()
            .context("Worktree repository is not UTF-8")?
            .to_owned(),
        cwd: binding
            .cwd
            .to_str()
            .context("Worktree path is not UTF-8")?
            .to_owned(),
        branch: binding.branch.clone(),
        base_revision: binding.base_revision.clone(),
    })
}

fn codex_fork_header(file: &std::fs::File) -> Option<Value> {
    use std::io::{BufRead, Read};
    let mut line = String::new();
    std::io::BufReader::new(file.try_clone().ok()?.take(64 * 1024))
        .read_line(&mut line)
        .ok()?;
    let row: Value = serde_json::from_str(&line).ok()?;
    (row["type"] == "session_meta").then(|| row["payload"].clone())
}

fn os_arguments(argv: Vec<std::ffi::OsString>) -> Result<Vec<String>> {
    argv.into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| anyhow::anyhow!("DSH argument is not UTF-8"))
        })
        .collect()
}

fn read_hud_transcript(root: &Path, transcript: &Path) -> Option<Vec<Value>> {
    use std::io::{BufRead, Read, Seek, SeekFrom};
    const TAIL_BYTES: u64 = 256 * 1024;
    let root = root.canonicalize().ok()?;
    let path = transcript.canonicalize().ok()?;
    if !path.starts_with(&root) || path.file_name() != transcript.file_name() || !path.is_file() {
        return None;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(&path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let offset = metadata.len().saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(offset.saturating_sub(1))).ok()?;
    let mut reader = std::io::BufReader::new(file.take(TAIL_BYTES + 1));
    if offset > 0 {
        reader.read_until(b'\n', &mut Vec::new()).ok()?;
    }
    Some(
        reader
            .split(b'\n')
            .filter_map(|row| serde_json::from_slice(&row.ok()?).ok())
            .collect(),
    )
}

fn redact_hud(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) if safe_hook_text(text, secrets).is_none() => {
            *text = "[redacted]".into()
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|value| redact_hud(value, secrets)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|value| redact_hud(value, secrets)),
        _ => {}
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn diagnostic(events: &mut Vec<OwnerEvent>, id: &str, message: String) {
    events.push(OwnerEvent::Diagnostic {
        session_id: id.into(),
        message,
    });
}

fn process_working_directory(pid: u32) -> Option<String> {
    #[cfg(target_os = "macos")]
    let cwd = {
        // lsof escapes non-ASCII path bytes under the C locale.
        let output = std::process::Command::new("/usr/sbin/lsof")
            .env("LC_ALL", "en_US.UTF-8")
            .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix('n').map(str::to_owned))?
    };
    #[cfg(target_os = "linux")]
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()?
        .to_string_lossy()
        .into_owned();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        return None;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Path::new(&cwd).is_dir().then_some(cwd)
}

#[cfg(test)]
mod egress_settings_tests {
    use super::*;
    use crate::egress_probe::{EgressRule, Exit, HostPair, HostSample};
    use matou_native_core::startup::{self, BootstrapResult};

    fn open(root: &Path, prepare: impl FnOnce(&Connection) -> Result<()>) -> Result<ShellOwner> {
        let BootstrapResult::Writable {
            connection,
            ownership,
            ..
        } = startup::open(root, &Uuid::new_v4().to_string())?
        else {
            anyhow::bail!("expected writable database");
        };
        prepare(&connection)?;
        ShellOwner::new(connection, ownership, || Uuid::new_v4().to_string())
    }

    fn fixture() -> Result<tempfile::TempDir> {
        Ok(tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?)
    }

    fn seed(db: &Connection, rule: &str, protection: &str) -> Result<()> {
        db.execute(
            "INSERT INTO preferences VALUES ('egress.rule', ?, 1), ('egress.protection', ?, 1)",
            [rule, protection],
        )?;
        Ok(())
    }

    fn sample(family: Family, ip: &str) -> (EgressSample, Exit) {
        let exit = Exit {
            ip: ip.into(),
            country: "US".into(),
        };
        let (api, login) = family.hosts();
        let pair = HostPair::new(
            family,
            HostSample::success(api, exit.clone()),
            HostSample::success(login, exit.clone()),
        );
        (EgressSample::new([(family, pair)].into()), exit)
    }

    #[test]
    fn egress_inherits_electron_preferences_and_keeps_changes_on_restart() -> Result<()> {
        let root = fixture()?;
        let stored = json!({
            "mode": "ip",
            "baselines": {
                "anthropic": {"ip":"203.0.113.1","country":"US"},
                "openai": {"ip":"203.0.113.2","country":"JP"},
                "chatgpt-subscription": {"ip":"203.0.113.3","country":"GB"},
                "google": {"ip":"203.0.113.4","country":"DE"}
            }
        });
        let mut owner = open(root.path(), |db| seed(db, &stored.to_string(), "false"))?;
        assert_eq!(serde_json::to_value(owner.egress().rule())?, stored);
        assert!(!owner.egress().protection_enabled());
        let (sample, new_exit) = sample(Family::Anthropic, "203.0.113.10");
        owner.apply_egress_sample(sample)?;
        assert_eq!(serde_json::to_value(owner.egress().rule())?, stored);
        owner.set_egress_rule_mode(RuleMode::Country)?;
        owner.set_egress_protection(true)?;
        owner.accept_egress_baseline(Family::Anthropic)?;
        let mut expected = stored;
        expected["mode"] = json!("country");
        expected["baselines"]["anthropic"] = serde_json::to_value(new_exit)?;
        drop(owner);

        let mut owner = open(root.path(), |_| Ok(()))?;
        assert_eq!(serde_json::to_value(owner.egress().rule())?, expected);
        assert!(owner.egress().protection_enabled());
        let rows = owner.connection.prepare(
            "SELECT value_json, updated_at FROM preferences WHERE key IN ('egress.rule', 'egress.protection') ORDER BY key",
        )?.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "true");
        assert_eq!(serde_json::from_str::<Value>(&rows[1].0)?, expected);
        assert!(rows.iter().all(|(_, updated_at)| *updated_at > 1));
        owner.set_egress_rule_mode(RuleMode::Ip)?;
        owner.set_egress_protection(false)?;
        drop(owner);

        let owner = open(root.path(), |_| Ok(()))?;
        expected["mode"] = json!("ip");
        assert_eq!(serde_json::to_value(owner.egress().rule())?, expected);
        assert!(!owner.egress().protection_enabled());
        Ok(())
    }

    #[test]
    fn egress_recovers_corrupt_preferences_and_persists_the_first_baseline() -> Result<()> {
        let root = fixture()?;
        let mut owner = open(root.path(), |db| seed(db, "{invalid", "\"false\""))?;
        assert_eq!(owner.egress().rule(), &EgressRule::default());
        assert!(owner.egress().protection_enabled());
        owner.set_egress_protection(true)?;
        assert_eq!(
            owner.connection.query_row(
                "SELECT value_json FROM preferences WHERE key = 'egress.protection'",
                [],
                |row| row.get::<_, String>(0),
            )?,
            "true",
        );
        let (sample, exit) = sample(Family::Google, "203.0.113.5");
        owner.apply_egress_sample(sample.clone())?;
        owner.connection.execute(
            "UPDATE preferences SET updated_at = 1 WHERE key = 'egress.rule'",
            [],
        )?;
        owner.apply_egress_sample(sample)?;
        assert_eq!(
            owner.connection.query_row(
                "SELECT updated_at FROM preferences WHERE key = 'egress.rule'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1,
        );
        drop(owner);

        let owner = open(root.path(), |_| Ok(()))?;
        assert_eq!(owner.egress().rule().mode, RuleMode::Country);
        assert_eq!(
            owner.egress().rule().baselines,
            [(Family::Google, exit)].into()
        );
        assert!(owner.egress().protection_enabled());
        Ok(())
    }

    #[test]
    fn egress_ignores_invalid_fields_and_does_not_write_in_read_only_mode() -> Result<()> {
        let root = fixture()?;
        let rule = json!({
            "mode": false,
            "baselines": {
                "anthropic": {"ip":0,"country":"US"},
                "openai": {"ip":"203.0.113.2","country":""},
                "chatgpt-subscription": ["203.0.113.3","GB"],
                "google": {"ip":"203.0.113.4","country":"DE","ignored":true},
                "unknown": {"ip":"203.0.113.9","country":"US"}
            }
        });
        let mut owner = open(root.path(), |db| seed(db, &rule.to_string(), "false"))?;
        assert_eq!(
            serde_json::to_value(owner.egress().rule())?,
            json!({"mode":"country","baselines":{"google":{"ip":"203.0.113.4","country":"DE"}}}),
        );
        owner.connection.pragma_update(None, "query_only", true)?;
        let (sample, _) = sample(Family::Anthropic, "203.0.113.8");
        owner.apply_egress_sample(sample)?;
        owner.accept_egress_baseline(Family::Anthropic)?;
        owner.set_egress_rule_mode(RuleMode::Ip)?;
        owner.set_egress_protection(true)?;
        assert_eq!(owner.egress().rule().mode, RuleMode::Ip);
        assert!(owner.egress().protection_enabled());
        drop(owner);

        let owner = open(root.path(), |_| Ok(()))?;
        assert_eq!(owner.egress().rule().mode, RuleMode::Country);
        assert_eq!(owner.egress().rule().baselines.len(), 1);
        assert!(!owner.egress().protection_enabled());
        assert_eq!(
            owner.connection.query_row(
                "SELECT COUNT(*) FROM preferences WHERE key LIKE 'egress.%' AND updated_at = 1",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            2,
        );
        Ok(())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod provider_tests {
    use super::*;
    use matou_native_core::{
        pty::ShellLaunch,
        startup::{self, BootstrapResult},
    };
    use matou_native_terminal::TerminalOptions;
    use std::{
        fs,
        io::{Read, Write},
        net::TcpStream,
        os::unix::fs::PermissionsExt,
        thread,
    };

    fn work_state(owner: &ShellOwner, session: &str) -> Result<(String, i64)> {
        Ok(owner.connection.query_row(
            "SELECT work_status, version FROM sessions WHERE id=?",
            [session],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    fn notification_count(owner: &ShellOwner) -> Result<i64> {
        Ok(owner.connection.query_row(
            "SELECT COUNT(*) FROM domain_events WHERE event_type='agent.notification'",
            [],
            |row| row.get(0),
        )?)
    }

    fn assert_provider_work_status(owner: &mut ShellOwner, scope: &Scope) -> Result<()> {
        let session = &scope.session_id;
        let mut events = Vec::new();
        let status_write_count = |owner: &ShellOwner| -> Result<i64> {
            Ok(owner.connection.query_row(
                "SELECT COUNT(*) FROM command_deduplication WHERE command_type='session.work-status'",
                [],
                |row| row.get(0),
            )?)
        };
        let initial_status_writes = status_write_count(owner)?;
        let hook = |event, message| {
            let payload =
                json!({"hook_event_name": event, "message": message, "session_id": "first-id"});
            Observation::parse(payload.as_object().unwrap())
        };
        let submit = |owner: &mut ShellOwner, events: &mut Vec<OwnerEvent>| {
            owner.apply(
                WorkerEvent {
                    session_id: session.clone(),
                    run_id: scope.run_id.clone(),
                    event: SessionEvent::InputSubmitted(Instant::now()),
                },
                events,
            );
        };
        submit(owner, &mut events);
        let running = work_state(owner, session)?;
        assert_eq!(running.0, "running");
        submit(owner, &mut events);
        assert_eq!(work_state(owner, session)?, running);
        let stale_at = Instant::now() - Duration::from_millis(1);
        submit(owner, &mut events);
        owner.apply_hook_at(scope.clone(), hook("Stop", ""), stale_at, &mut events);
        assert_eq!(work_state(owner, session)?, running);

        owner.apply_hook(scope.clone(), hook("Stop", ""), &mut events);
        assert_eq!(work_state(owner, session)?.0, "idle");
        submit(owner, &mut events);
        owner.apply_hook(scope.clone(), hook("Stop", ""), &mut events);
        let idle = work_state(owner, session)?;
        assert_eq!(idle.0, "idle");
        owner.apply_hook(scope.clone(), hook("Stop", ""), &mut events);
        owner.apply_hook(scope.clone(), hook("PostToolUse", ""), &mut events);
        assert_eq!(work_state(owner, session)?, idle);
        owner.apply_hook(scope.clone(), hook("PermissionRequest", ""), &mut events);
        assert_eq!(work_state(owner, session)?.0, "needs-input");
        owner.apply_hook(scope.clone(), hook("Notification", "Finished"), &mut events);
        assert_eq!(work_state(owner, session)?.0, "idle");
        owner.apply_hook(
            scope.clone(),
            hook("Notification", "Awaiting input"),
            &mut events,
        );
        let waiting = work_state(owner, session)?;
        assert_eq!(waiting.0, "needs-input");
        owner.apply_hook_at(
            scope.clone(),
            hook("PostToolUse", ""),
            stale_at,
            &mut events,
        );
        assert_eq!(work_state(owner, session)?, waiting);

        owner
            .connection
            .execute("UPDATE sessions SET archived_at=1 WHERE id=?", [session])?;
        let archived = work_state(owner, session)?;
        let before_archive_hook = notification_count(owner)?;
        owner.apply_hook(scope.clone(), hook("Notification", "Done"), &mut events);
        assert_eq!(work_state(owner, session)?, archived);
        assert_eq!(notification_count(owner)?, before_archive_hook);
        owner.connection.execute(
            "UPDATE sessions SET archived_at=NULL, work_status='error' WHERE id=?",
            [session],
        )?;
        let error = work_state(owner, session)?;
        for (event, message) in [
            ("Stop", ""),
            ("Notification", "Done"),
            ("Notification", "Waiting"),
            ("PermissionRequest", ""),
            ("PreToolUse", ""),
            ("PostToolUse", ""),
        ] {
            owner.apply_hook(scope.clone(), hook(event, message), &mut events);
            assert_eq!(work_state(owner, session)?, error);
        }
        submit(owner, &mut events);
        assert_eq!(work_state(owner, session)?.0, "running");
        let committed_statuses: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                OwnerEvent::Domain(commit) => commit.result["workStatus"].as_str(),
                _ => None,
            })
            .collect();
        assert_eq!(
            committed_statuses,
            [
                "running",
                "idle",
                "running",
                "idle",
                "needs-input",
                "idle",
                "needs-input",
                "running",
            ]
        );
        assert_eq!(
            status_write_count(owner)? - initial_status_writes,
            committed_statuses.len() as i64
        );

        let (reply, result) = mpsc::channel();
        owner.send(
            session,
            WorkerCommand::Write {
                bytes: vec![b'\n'],
                origin: InputOrigin::User,
                reply,
            },
        )?;
        result.recv_timeout(Duration::from_secs(5))??;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !owner.shells[session].ended && Instant::now() < deadline {
            owner.poll(Duration::from_millis(25))?;
        }
        let exited = work_state(owner, session)?;
        assert_eq!(exited.0, "exited");
        let before_exit_hook = notification_count(owner)?;
        owner.apply_hook(scope.clone(), hook("Stop", ""), &mut events);
        owner.apply_hook(scope.clone(), hook("PostToolUse", ""), &mut events);
        assert_eq!(work_state(owner, session)?, exited);
        assert_eq!(notification_count(owner)?, before_exit_hook);
        Ok(())
    }

    fn assert_claude_waiting_recovery(
        owner: &mut ShellOwner,
        session: &str,
        post: impl Fn(&str, &str) -> Result<()>,
        append: impl Fn(Value) -> Result<()>,
    ) -> Result<()> {
        let mut events = Vec::new();
        post("Notification", "Awaiting input")?;
        owner.poll_provider_hooks(&mut events);
        let waiting = work_state(owner, session)?;
        assert_eq!(waiting.0, "needs-input");
        owner.poll_provider_hooks(&mut events);
        assert_eq!(work_state(owner, session)?, waiting);

        // The fixture only posts a provider hook. No PTY input is submitted.
        post("PostToolUse", "")?;
        owner.poll_provider_hooks(&mut events);
        assert_eq!(work_state(owner, session)?.0, "running");

        events.clear();
        post("Notification", "Awaiting input")?;
        post("PreToolUse", "")?;
        owner.poll_provider_hooks(&mut events);
        assert_eq!(work_state(owner, session)?.0, "running");
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    OwnerEvent::Domain(commit) => commit.result["workStatus"].as_str(),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ["needs-input", "running"]
        );

        post("PostToolUse", "")?;
        post("PermissionRequest", "")?;
        owner.poll_provider_hooks(&mut events);
        let permission = work_state(owner, session)?;
        assert_eq!(permission.0, "needs-input");
        let waiting_since = owner.shells[session]
            .hook
            .as_ref()
            .unwrap()
            .claude_waiting_since_ms
            .context("permission notice time")?;
        let assistant = |timestamp, sidechain| {
            json!({
                "type": "assistant", "sessionId": "first-id", "timestamp": timestamp,
                "isSidechain": sidechain,
                "message": {"role": "assistant", "content": [{"type": "text", "text": "Continuing"}]}
            })
        };
        append(assistant(waiting_since - 1.0, false))?;
        append(assistant(waiting_since + 1.0, true))?;
        owner.observe_claude_transcript(session, &mut events);
        owner.poll_provider_hooks(&mut events);
        assert_eq!(work_state(owner, session)?, permission);

        append(assistant(waiting_since + 1.0, false))?;
        owner.observe_claude_transcript(session, &mut events);
        owner.poll_provider_hooks(&mut events);
        assert_eq!(work_state(owner, session)?.0, "running");

        post("Stop", "")?;
        owner.poll_provider_hooks(&mut events);
        assert_eq!(work_state(owner, session)?.0, "idle");
        Ok(())
    }

    #[test]
    fn fork_identity_is_scoped_to_the_child_run_and_must_be_new() -> Result<()> {
        let root = tempfile::tempdir()?;
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(root.path(), "fork-generation")?
        else {
            anyhow::bail!("expected writable database");
        };
        hierarchy::bootstrap_window(
            &mut connection,
            &command("bootstrap".into(), "window.bootstrap", "fixture".into()),
            &hierarchy::BootstrapWindowInput {
                window_id: "window".into(),
                default_root_directory: root.path().to_string_lossy().into(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &HierarchyIds {
                workspace_id: "workspace".into(),
                execution_context_id: "context".into(),
                task_id: "task".into(),
                scene_id: "scene".into(),
                session_id: "source".into(),
                root_node_id: "node".into(),
                mount_id: "mount".into(),
            },
            "默认",
            1,
        )?;
        connection.execute_batch(
            r#"UPDATE sessions SET kind='claude-code', work_status='running' WHERE id='source';
             INSERT INTO provider_bindings (id, session_id, provider, provider_session_id, resume_state,
                                            metadata_json, created_at, updated_at, validated_at)
             VALUES ('source-binding', 'source', 'claude-code', 'source-id', 'available',
                     '{"canFork":true}', 2, 2, 2);
             INSERT INTO session_graph_summaries (session_id, latest_lines_json, updated_at)
             VALUES ('source', '["source summary"]', 2);
             INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, cwd,
                                   work_status, created_at, updated_at, last_activity_at)
               SELECT 'parent', task_id, execution_context_id, kind, 'running', 'Parent', cwd,
                      'running', created_at, updated_at, last_activity_at FROM sessions WHERE id='source';
             INSERT INTO provider_bindings (id, session_id, provider, provider_session_id, resume_state,
                                            metadata_json, created_at, updated_at, validated_at)
             VALUES ('parent-binding', 'parent', 'claude-code', 'parent-id', 'available',
                     '{"canFork":true}', 2, 2, 2);
             INSERT INTO session_relation_events (event_id, relation_id, operation, task_id,
                                                   from_session_id, to_session_id, relation_kind, command_id, occurred_at)
             VALUES ('parent-event', 'parent-relation', 'created', 'task',
                     'source', 'parent', 'derived-from', 'fixture', 2);
             INSERT INTO session_relations_current (relation_id, task_id, from_session_id, to_session_id,
                                                    relation_kind, created_at, updated_at, source_event_sequence)
             VALUES ('parent-relation', 'task', 'source', 'parent', 'derived-from', 2, 2, last_insert_rowid());"#,
        )?;
        let mut owner =
            ShellOwner::new(connection, ownership, || uuid::Uuid::new_v4().to_string())?;
        let source = owner.session("source")?.unwrap();
        let authority = owner.claude_binding("source")?.unwrap();
        let executable = root.path().join("test-provider");
        fs::write(&executable, "#!/bin/sh\nread -r line\nexit 23\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let create =
            |owner: &mut ShellOwner, label: &str, placement| -> Result<canvas::ForkSessionInput> {
                let input = canvas::ForkSessionInput {
                    window_id: "window".into(),
                    scene_id: "scene".into(),
                    source_session_id: "source".into(),
                    placement,
                    name: label.into(),
                    worktree_mode: canvas::ForkWorktreeMode::Current,
                    submission_key: label.into(),
                    now: 3,
                };
                let created = owner.create_fork_child(
                    &command(
                        format!("create-{label}"),
                        "hierarchy.fork-session",
                        label.into(),
                    ),
                    &input,
                    &canvas::CanvasIds {
                        scene_id: "scene".into(),
                        session_id: label.into(),
                        root_node_id: format!("{label}-node"),
                        secondary_node_id: format!("{label}-split"),
                        mount_id: format!("{label}-mount"),
                        relation_id: format!("{label}-relation"),
                    },
                    &canvas::CanvasMessages {
                        new_canvas: "",
                        duplicate_scene_name: "",
                        workspace_path_invalid: "",
                        detached_session_must_return: "",
                    },
                )?;
                assert_eq!(created["sessionId"], label);
                Ok(input)
            };
        let spawn = |owner: &mut ShellOwner,
                     label: &str,
                     placement,
                     expected_source: &str|
         -> Result<Scope> {
            create(owner, label, placement)?;
            let target = owner.pending_claude_fork(label)?.unwrap();
            let plan = matou_native_core::provider_launch::prepare_claude_fork(
                root.path(),
                root.path(),
                &target.metadata,
                &target.source_id,
                [(
                    "MATOU_CLAUDE_COMMAND".into(),
                    executable.clone().into_os_string(),
                )]
                .into(),
                80,
                24,
            )?;
            assert_eq!(
                plan.launch.argv,
                ["--resume", expected_source, "--fork-session"]
            );
            owner.launch_claude_fork(
                label.into(),
                TerminalOptions::new(80, 24, 7, 14),
                plan,
                target.source_id,
            )?;
            Ok(Scope {
                profile: LaunchProfile::ClaudeCode,
                session_id: label.into(),
                run_id: owner.shells[label].run_id.clone(),
                generation: owner.generation.clone(),
                expected_id: None,
                fork_source_id: Some(expected_source.into()),
            })
        };
        let observation = |event, id| {
            let payload = json!({"hook_event_name": event, "session_id": id});
            Observation::parse(payload.as_object().unwrap())
        };
        let mut events = Vec::new();
        use canvas::ForkPlacement::{Child, Peer, Sibling};
        let rejected = spawn(&mut owner, "rejected", Child, "source-id")?;
        owner.apply_hook(
            rejected.clone(),
            observation("SessionEnd", "source-id"),
            &mut events,
        );
        owner.apply_hook(
            rejected.clone(),
            observation("unknown", "source-id"),
            &mut events,
        );
        assert!(owner.waiting_for_provider_restore("rejected"));
        assert!(owner.claude_binding("rejected")?.is_none());
        owner.apply_hook(
            rejected.clone(),
            observation("SessionStart", "source-id"),
            &mut events,
        );
        owner.apply_hook(
            rejected.clone(),
            observation("SessionStart", "other-id"),
            &mut events,
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, OwnerEvent::ProviderIdentity(value)
            if value["result"] == "fork-failed" && value["forkError"] == FORK_SOURCE_IDENTITY))
        );
        assert!(owner.claude_binding("rejected")?.is_none());
        assert_eq!(work_state(&owner, "rejected")?.0, "error");

        for (label, placement, expected_source) in [
            ("sibling-rejected", Sibling, "parent-id"),
            ("peer-rejected", Peer, "source-id"),
        ] {
            let scope = spawn(&mut owner, label, placement, expected_source)?;
            owner.apply_hook(
                scope.clone(),
                observation("SessionStart", expected_source),
                &mut events,
            );
            owner.apply_hook(scope, observation("SessionStart", "other-id"), &mut events);
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, OwnerEvent::ProviderIdentity(value)
                if value["sessionId"] == label && value["result"] == "fork-failed"
                   && value["forkError"] == FORK_SOURCE_IDENTITY))
            );
            assert!(owner.claude_binding(label)?.is_none());
            assert_eq!(work_state(&owner, label)?.0, "error");
        }

        let timeout = spawn(&mut owner, "timeout", Child, "source-id")?;
        owner
            .shells
            .get_mut("timeout")
            .unwrap()
            .hook
            .as_mut()
            .unwrap()
            .deadline = Some(Instant::now());
        owner.poll_provider_hooks(&mut events);
        owner.apply_hook(
            timeout,
            observation("SessionStart", "too-late"),
            &mut events,
        );
        assert!(owner.claude_binding("timeout")?.is_none());
        assert_eq!(work_state(&owner, "timeout")?.0, "error");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, OwnerEvent::ProviderIdentity(value)
            if value["sessionId"] == "timeout" && value["forkError"] == FORK_IDENTITY_TIMEOUT))
        );

        let early = spawn(&mut owner, "early-exit", Child, "source-id")?;
        let (reply, result) = mpsc::channel();
        owner.send(
            "early-exit",
            WorkerCommand::Write {
                bytes: vec![b'\n'],
                origin: InputOrigin::User,
                reply,
            },
        )?;
        result.recv_timeout(Duration::from_secs(5))??;
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner
            .shells
            .get("early-exit")
            .is_some_and(|shell| !shell.ended)
            && Instant::now() < deadline
        {
            events.extend(owner.poll(Duration::from_millis(25))?);
        }
        assert!(events.iter().any(|event| matches!(event, OwnerEvent::ProviderIdentity(value)
            if value["runId"] == early.run_id && value["forkError"].as_str().is_some_and(|error| error.contains("code 23")))));
        assert!(owner.claude_binding("early-exit")?.is_none());

        let never_started = create(&mut owner, "start-failure", Child)?;
        assert!(owner.fail_claude_fork("start-failure", "executable unavailable")?);
        let same = owner.create_fork_child(
            &command(
                "repeat-failed".into(),
                "hierarchy.fork-session",
                "repeat".into(),
            ),
            &never_started,
            &canvas::CanvasIds {
                scene_id: String::new(),
                root_node_id: String::new(),
                secondary_node_id: String::new(),
                session_id: String::new(),
                mount_id: String::new(),
                relation_id: String::new(),
            },
            &canvas::CanvasMessages {
                new_canvas: "",
                duplicate_scene_name: "",
                workspace_path_invalid: "",
                detached_session_must_return: "",
            },
        )?;
        assert_eq!(same["sessionId"], "start-failure");
        assert_eq!(same["stage"], "failed");
        assert_eq!(same["error"], "executable unavailable");
        assert!(owner.claude_binding("start-failure")?.is_none());
        assert_eq!(owner.session("source")?.unwrap(), source);
        let after = owner.claude_binding("source")?.unwrap();
        assert_eq!(
            (after.id, after.provider_session_id, after.metadata),
            (
                authority.id,
                authority.provider_session_id,
                authority.metadata
            )
        );
        assert_eq!(
            owner.connection.query_row(
                "SELECT latest_lines_json FROM session_graph_summaries WHERE session_id='source'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            r#"["source summary"]"#,
        );

        let confirmed = spawn(&mut owner, "confirmed", Child, "source-id")?;
        // The child owns its launch after acceptance, even if its source is removed.
        owner.remove_session_branch(
            &command(
                "remove-source".into(),
                "hierarchy.remove-session-branch",
                "source".into(),
            ),
            &cards::RemoveSessionBranchInput {
                window_id: "window".into(),
                scene_id: "scene".into(),
                session_id: "source".into(),
                scope: cards::RemoveScope::NodeOnly,
                now: 4,
            },
            4,
        )?;
        owner.apply_hook(
            Scope {
                generation: "stale".into(),
                ..confirmed.clone()
            },
            observation("SessionStart", "stale-id"),
            &mut events,
        );
        assert!(owner.claude_binding("confirmed")?.is_none());
        owner.apply_hook(
            confirmed.clone(),
            observation("SessionStart", "child-id"),
            &mut events,
        );
        owner.apply_hook(confirmed, observation("Stop", "source-id"), &mut events);
        assert_eq!(
            owner
                .claude_binding("confirmed")?
                .unwrap()
                .provider_session_id,
            "child-id"
        );
        assert_eq!(
            owner.connection.query_row(
                "SELECT COUNT(*) FROM provider_bindings WHERE session_id='confirmed'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            1,
        );
        assert_eq!(
            owner.connection.query_row(
                "SELECT stage FROM session_fork_intents WHERE session_id='confirmed'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "succeeded",
        );
        assert_eq!(
            owner.claude_binding("source")?.unwrap().provider_session_id,
            "source-id"
        );
        owner.shutdown()?;
        Ok(())
    }

    #[test]
    fn verified_promotion_records_a_queued_identity_before_the_child_exit() -> Result<()> {
        let root = tempfile::tempdir()?;
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(root.path(), "promotion-generation")?
        else {
            anyhow::bail!("expected writable database");
        };
        let session = "promoted-session";
        hierarchy::bootstrap_window(
            &mut connection,
            &command("bootstrap".into(), "window.bootstrap", "fixture".into()),
            &hierarchy::BootstrapWindowInput {
                window_id: "window".into(),
                default_root_directory: root.path().to_string_lossy().into(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &HierarchyIds {
                workspace_id: "workspace".into(),
                execution_context_id: "context".into(),
                task_id: "task".into(),
                scene_id: "scene".into(),
                session_id: session.into(),
                root_node_id: "node".into(),
                mount_id: "mount".into(),
            },
            "默认",
            1,
        )?;
        let executable = root.path().join("test-provider");
        fs::write(&executable, "#!/bin/sh\nread -r line\nexit 24\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let mut owner =
            ShellOwner::new(connection, ownership, || uuid::Uuid::new_v4().to_string())?;
        owner.set_shell_launch_directory(session, &root.path().to_string_lossy())?;
        owner.promote_shell_to_agent(session, LaunchProfile::ClaudeCode)?;
        let run = owner.launch_promoted_claude(
            session,
            TerminalOptions::new(80, 24, 7, 14),
            matou_native_core::provider_launch::ClaudeLaunch {
                launch: ShellLaunch {
                    profile: LaunchProfile::ClaudeCode,
                    executable,
                    argv: Vec::new(),
                    cwd: root.path().to_owned(),
                    data_root: root.path().to_owned(),
                    inherited_env: Default::default(),
                    env: Default::default(),
                    cols: 80,
                    rows: 24,
                },
                provider_config_id: "fixture-config".into(),
                model: "fixture-model".into(),
                permission_mode: "default".into(),
                assigned_role: None,
                effort: None,
                clear_assignment_environment: false,
                gateway: None,
                gateway_refresh: None,
                subscription_settings: None,
                subscription_role: None,
            },
        )?;
        assert_eq!(
            owner.egress().status().session_families.get(session),
            Some(&vec![Family::Anthropic])
        );
        let registration = &owner.shells[session].hook.as_ref().unwrap()._registration;
        let settings: Value = serde_json::from_slice(&fs::read(&registration.settings_path)?)?;
        let url = url::Url::parse(
            settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["url"]
                .as_str()
                .unwrap(),
        )?;
        let post = |identity: &str| -> Result<()> {
            let mut socket = TcpStream::connect(("127.0.0.1", url.port().unwrap()))?;
            socket.set_read_timeout(Some(Duration::from_secs(3)))?;
            let body =
                json!({"hook_event_name": "UserPromptSubmit", "session_id": identity}).to_string();
            write!(
                socket,
                "POST {} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
                url.path(),
                body.len()
            )?;
            let mut response = String::new();
            socket.read_to_string(&mut response)?;
            assert!(response.starts_with("HTTP/1.1 200"));
            Ok(())
        };

        owner.begin_promotion_process_check(session)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !owner.shells[session].promotion_verified() {
            ensure!(Instant::now() < deadline, "check did not pass");
            thread::sleep(Duration::from_millis(2));
        }
        // Keep the owner from polling until the real child exit is also queued.
        post("recoverable-conversation")?;
        assert!(owner.claude_binding(session)?.is_none());
        let (reply, written) = mpsc::channel();
        owner.send(
            session,
            WorkerCommand::Write {
                bytes: b"\r".to_vec(),
                origin: InputOrigin::User,
                reply,
            },
        )?;
        written.recv_timeout(Duration::from_secs(5))??;
        let mut queued = Vec::new();
        loop {
            let event = owner.receive.recv_timeout(Duration::from_secs(5))?;
            let exit = matches!(event.event, SessionEvent::SavedExit { .. });
            queued.push(event);
            if exit {
                break;
            }
        }
        post("too-late-conversation")?;
        let mut events = Vec::new();
        for event in queued {
            owner.apply(event, &mut events);
        }
        assert!(owner.finish_promotion_startup(session, &run)?);
        assert!(events.iter().any(|event| matches!(event,
            OwnerEvent::ProviderIdentity(value) if value["reason"] == "retired-run")));
        assert_eq!(owner.runs(session)?[0].exit_code, Some(24));
        owner.shutdown()?;
        drop(owner);
        let reopened = startup::open_read_only(&root.path().join("matou.sqlite"))?;
        let binding = sessions::claude_binding(&reopened, session)?.context("durable binding")?;
        assert_eq!(binding.provider_session_id, "recoverable-conversation");
        assert!(binding.resumable);
        Ok(())
    }

    #[test]
    fn current_provider_run_alone_can_project_identity() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("workspace");
        fs::create_dir(&cwd)?;
        ensure!(
            std::process::Command::new("git")
                .current_dir(&cwd)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(["init", "--quiet", "--initial-branch=main"])
                .status()?
                .success()
        );
        let config_root = root.path().join("claude");
        let projects = config_root.join("projects/project-bucket");
        fs::create_dir_all(&projects)?;
        fs::create_dir(cwd.join(".claude"))?;
        fs::write(
            config_root.join("settings.json"),
            r#"{"hooks":{"Stop":[],"PreToolUse":[]},"mcpServers":{"global":{},"shared":{}}}"#,
        )?;
        fs::write(
            cwd.join(".claude/settings.local.json"),
            r#"{"hooks":{"Stop":[],"SessionStart":[]},"mcpServers":{"local":{},"shared":{}}}"#,
        )?;
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(root.path(), "runtime-generation")?
        else {
            anyhow::bail!("expected writable database");
        };
        let session = "provider-session";
        hierarchy::bootstrap_window(
            &mut connection,
            &command("bootstrap".into(), "window.bootstrap", "fixture".into()),
            &hierarchy::BootstrapWindowInput {
                window_id: "window".into(),
                default_root_directory: cwd.to_string_lossy().into(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &HierarchyIds {
                workspace_id: "workspace".into(),
                execution_context_id: "context".into(),
                task_id: "task".into(),
                scene_id: "scene".into(),
                root_node_id: "node".into(),
                session_id: session.into(),
                mount_id: "mount".into(),
            },
            "默认",
            1,
        )?;
        connection.execute(
            "UPDATE sessions SET kind='claude-code', cwd=?, work_status='needs-input' WHERE id=?",
            params![cwd.to_string_lossy(), session],
        )?;
        let mut owner =
            ShellOwner::new(connection, ownership, || uuid::Uuid::new_v4().to_string())?;
        let executable = root.path().join("test-provider");
        assert_eq!(
            owner.notification_scene_id("window", session)?.as_deref(),
            Some("scene")
        );
        assert_eq!(
            owner.notification_scene_id("window", "missing-session")?,
            None
        );
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'provider output before identity\\n'\nread -r line\n",
        )?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let plan = || matou_native_core::provider_launch::ClaudeLaunch {
            launch: ShellLaunch {
                profile: LaunchProfile::ClaudeCode,
                executable: executable.clone(),
                argv: Vec::new(),
                cwd: cwd.clone(),
                data_root: root.path().to_owned(),
                inherited_env: Default::default(),
                env: [(
                    "CLAUDE_CONFIG_DIR".into(),
                    config_root.clone().into_os_string(),
                )]
                .into(),
                cols: 80,
                rows: 24,
            },
            provider_config_id: "fixture-config".into(),
            model: "fixture-model".into(),
            permission_mode: "default".into(),
            assigned_role: None,
            effort: None,
            clear_assignment_environment: false,
            gateway: None,
            gateway_refresh: None,
            subscription_settings: None,
            subscription_role: None,
        };
        let terminal = TerminalOptions::new(80, 24, 7, 14);
        let blocked_directory = root.path().join("provider-hooks");
        fs::write(&blocked_directory, "")?;
        let failure = owner
            .launch_claude(session.into(), terminal, plan(), None)
            .unwrap_err();
        assert!(failure.downcast_ref::<HookSetup>().is_some());
        assert!(!owner.has_shell_run(session)?);
        fs::remove_file(blocked_directory)?;
        owner.launch_claude(session.into(), terminal, plan(), None)?;
        assert!(
            owner
                .launch_claude(session.into(), terminal, plan(), None)
                .is_err()
        );
        let scope = Scope {
            profile: LaunchProfile::ClaudeCode,
            session_id: session.into(),
            run_id: owner.shells[session].run_id.clone(),
            generation: owner.generation.clone(),
            expected_id: None,
            fork_source_id: None,
        };
        let observation = |event, identity: Option<&str>| {
            let payload = json!({"hook_event_name": event, "session_id": identity});
            Observation::parse(payload.as_object().unwrap())
        };
        let mut events = Vec::new();
        for (event, id) in [
            ("SessionEnd", Some("end-id")),
            ("unknown", Some("statusline-id")),
            ("UserPromptSubmit", None),
            ("Stop", None),
        ] {
            owner.apply_hook(scope.clone(), observation(event, id), &mut events);
        }
        assert_eq!(notification_count(&owner)?, 0);
        let count = |owner: &ShellOwner| {
            owner
                .connection
                .query_row("SELECT COUNT(*) FROM provider_bindings", [], |row| {
                    row.get::<_, i64>(0)
                })
        };
        assert_eq!(count(&owner)?, 0);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], OwnerEvent::ProviderIdentity(value)
                         if value["reason"] == "missing-identity"));
        owner.apply_output(session, "dropped terminal text", &[], &mut events);
        assert!(!owner.summaries.contains_key(session));
        owner
            .shells
            .get_mut(session)
            .unwrap()
            .hook
            .as_mut()
            .unwrap()
            .deadline = Some(Instant::now());
        owner.poll_provider_hooks(&mut events);
        assert!(
            matches!(&events[1], OwnerEvent::ProviderIdentity(value) if value["result"] == "timed-out")
        );
        let registration = &owner.shells[session].hook.as_ref().unwrap()._registration;
        let config: Value = serde_json::from_slice(&fs::read(&registration.settings_path)?)?;
        let url = url::Url::parse(
            config["hooks"]["UserPromptSubmit"][0]["hooks"][0]["url"]
                .as_str()
                .unwrap(),
        )?;
        let post = |event: &str, message: &str| -> Result<()> {
            let mut socket = TcpStream::connect(("127.0.0.1", url.port().unwrap()))?;
            socket.set_read_timeout(Some(Duration::from_secs(3)))?;
            let body =
                json!({"hook_event_name": event, "session_id": "first-id", "message": message})
                    .to_string();
            write!(
                socket,
                "POST {} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
                url.path(),
                body.len()
            )?;
            let mut response = String::new();
            socket.read_to_string(&mut response)?;
            assert!(response.starts_with("HTTP/1.1 200"));
            Ok(())
        };
        thread::scope(|threads| -> Result<()> {
            let first = threads.spawn(|| post("UserPromptSubmit", ""));
            let second = threads.spawn(|| post("UserPromptSubmit", ""));
            first.join().unwrap()?;
            second.join().unwrap()?;
            Ok(())
        })?;
        owner.poll_provider_hooks(&mut events);
        assert_eq!(count(&owner)?, 1);
        assert_eq!(
            owner.connection.query_row(
                "SELECT COUNT(*) FROM domain_events WHERE event_type='provider-binding.recorded'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, OwnerEvent::ProviderIdentity(value)
            if value["result"] == "confirmed"))
                .count(),
            1
        );
        owner.apply_output(session, "accepted terminal text", &[], &mut events);
        assert_eq!(owner.summaries[session].raw, "accepted terminal text");
        let first_path = projects.join("first-id.jsonl");
        let append = |row: Value| -> Result<()> {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&first_path)?;
            writeln!(file, "{row}")?;
            Ok(())
        };
        let hook_with_transcript = |event| {
            let payload = json!({"hook_event_name": event, "session_id":"first-id", "transcript_path":first_path,
                "cwd":cwd, "tool_use_id":"read", "tool_name":"Read"});
            let mut observation = Observation::parse(payload.as_object().unwrap());
            observation.hud_payload = Some(payload);
            observation
        };
        append(
            json!({"type":"user","sessionId":"first-id","message":{"role":"user",
            "content":"<system-reminder>internal instructions</system-reminder>"}}),
        )?;
        append(
            json!({"type":"user","sessionId":"different-id","message":{"role":"user","content":"Wrong conversation"}}),
        )?;
        append(
            json!({"type":"user","sessionId":"first-id","message":{"role":"user",
            "content":[{"type":"text","text":"请接手  Matou 原生迁移"}]}}),
        )?;
        owner.apply_hook(
            scope.clone(),
            hook_with_transcript("UserPromptSubmit"),
            &mut events,
        );
        assert_eq!(
            owner.session(session)?.unwrap().title,
            "请接手 Matou 原生迁移"
        );
        let graph = owner.graph("scene", Some("window"))?;
        assert_eq!(graph["nodes"][0]["title"], "请接手 Matou 原生迁移");
        assert_eq!(graph["nodes"][0]["git"]["branch"], "main");
        append(json!({"type":"summary","summary":"Transcript summary"}))?;
        owner
            .shells
            .get_mut(session)
            .unwrap()
            .hook
            .as_mut()
            .unwrap()
            .claude_transcript_poll = Some(Instant::now());
        owner.poll_claude_transcripts(&mut events, true);
        assert_eq!(owner.session(session)?.unwrap().title, "Transcript summary");
        append(json!({"type":"ai-title","aiTitle":"Native migration"}))?;
        append(json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"read","name":"Read"},
            {"type":"tool_use","id":"agent","name":"Agent","input":{"name":"Reviewer"}},
            {"type":"tool_use","id":"agent-2","name":"Agent","input":{"name":"Verifier"}},
        ]}}))?;
        append(
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"read"}]}}),
        )?;
        owner.apply_hook(
            scope.clone(),
            hook_with_transcript("PostToolUse"),
            &mut events,
        );
        owner.observe_claude_transcript(session, &mut events);
        assert_eq!(owner.session(session)?.unwrap().title, "Native migration");
        let hud = owner.agent_hud(session, root.path())?;
        assert_eq!(hud["hud"]["defaultAccountUsage"], false);
        assert_eq!(hud["hud"]["gitBranch"], "main");
        assert_eq!(hud["hud"]["toolCounts"], json!([{"name":"Read","count":1}]));
        assert_eq!(hud["hud"]["subagentCount"], 2);
        assert_eq!(hud["hud"]["configCounts"]["mcps"], 3);
        assert_eq!(
            hud["hud"]["mcpServerNames"],
            json!(["global", "local", "shared"])
        );
        assert_eq!(hud["hud"]["configCounts"]["hooks"], 3);
        assert_eq!(
            hud["hud"]["configCounts"]["hookNames"],
            json!(["PreToolUse", "SessionStart", "Stop"])
        );
        owner.set_session_title(session, Some("My card"))?;
        append(json!({"type":"custom-title","customTitle":"New provider title"}))?;
        owner.observe_claude_transcript(session, &mut events);
        let card = owner.session(session)?.unwrap();
        assert_eq!(card.title, "My card");
        assert_eq!(card.provider_title.as_deref(), Some("New provider title"));

        assert_claude_waiting_recovery(&mut owner, session, post, append)?;
        assert_provider_work_status(&mut owner, &scope)?;
        owner.retire_sessions(&[session.into()]);
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner.has_shell_worker(session) && Instant::now() < deadline {
            owner.poll(Duration::from_millis(25))?;
        }
        assert!(!owner.has_shell_worker(session));
        let mut default_login = plan();
        default_login.provider_config_id = "anthropic-official".into();
        default_login.launch.env = [
            ("HOME".into(), root.path().as_os_str().to_owned()),
            (
                "ANTHROPIC_BASE_URL".into(),
                "https://api.anthropic.com".into(),
            ),
        ]
        .into();
        owner.provider_environment = Some(Default::default());
        owner.launch_claude(session.into(), terminal, default_login, None)?;
        assert_eq!(
            owner.agent_hud(session, root.path())?["hud"]["defaultAccountUsage"],
            false
        );
        let current = Scope {
            run_id: owner.shells[session].run_id.clone(),
            ..scope.clone()
        };
        let stop = |id| observation("Stop", Some(id));
        let before = work_state(&owner, session)?;
        let previous_notifications = notification_count(&owner)?;
        let start_event = events.len();
        owner.apply_hook(current.clone(), stop("current-id"), &mut events);
        assert_eq!(
            owner.agent_hud(session, root.path())?["hud"]["defaultAccountUsage"],
            true
        );
        assert!(owner.provider_allows_derivation(session));
        assert_eq!(work_state(&owner, session)?, before);
        assert_eq!(notification_count(&owner)?, previous_notifications + 1);
        let (seq, payload, command_id, request_hash): (i64, String, String, String) =
            owner.connection.query_row(
                "SELECT seq, payload_json, domain_events.command_id, request_hash
                 FROM domain_events JOIN command_deduplication USING (command_id)
                 WHERE event_type='agent.notification' AND workspace_id='workspace'
                   AND task_id='task' AND session_id=? AND aggregate_id=?
                   AND aggregate_type='session' AND schema_version=1
                   AND json_extract(payload_json, '$.runId')=?
                   AND command_type='agent.notification.publish'",
                params![session, session, current.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let payload: Value = serde_json::from_str(&payload)?;
        assert_eq!(
            payload,
            json!({
                "targetSessionId": session, "runId": current.run_id, "provider": "claude-code",
                "event": {
                    "eventType": "completed", "title": "Claude Code", "subtitle": "Completed",
                    "body": "Claude session completed", "sound": true, "cooldownKey": "Stop",
                },
            })
        );
        assert!(events[start_event..].iter().any(|event| matches!(event,
            OwnerEvent::Domain(commit) if commit.first_event_sequence == Some(seq)
                && commit.last_event_sequence == Some(seq))));
        let duplicate = sessions::publish_claude_notification(
            &mut owner.connection,
            &command(command_id, "agent.notification.publish", request_hash),
            session,
            &current.run_id,
            &current.generation,
            payload["event"].clone(),
            now(),
        )?;
        assert!(duplicate.replayed);
        assert_eq!(duplicate.first_event_sequence, Some(seq));
        append(json!({"type":"custom-title","customTitle":"Retired run title"}))?;
        owner.apply_hook(scope, hook_with_transcript("Stop"), &mut events);
        let card = owner.session(session)?.unwrap();
        assert_eq!(card.title, "My card");
        assert_eq!(card.provider_title.as_deref(), Some("New provider title"));
        owner.apply_hook(
            Scope {
                generation: "old-generation".into(),
                ..current.clone()
            },
            stop("current-id"),
            &mut events,
        );
        assert_eq!(work_state(&owner, session)?, before);
        assert_eq!(notification_count(&owner)?, previous_notifications + 1);
        assert_eq!(count(&owner)?, 2);
        assert!(
            matches!(events.last(), Some(OwnerEvent::ProviderIdentity(value)) if value["reason"] == "retired-run")
        );
        let config = ContextOptimizerConfig::for_data_root(root.path());
        let at = now() - 270_000;
        let payload = json!({
            "session_id": "current-id", "timestamp": at, "context_window":{
                "context_window_size": 200000, "current_usage":{
                    "input_tokens": 35000, "output_tokens": 10000,
                    "cache_read_input_tokens": 130000, "cache_creation_input_tokens": 0
                }
            }
        });
        let mut statusline = Observation::parse(payload.as_object().unwrap());
        statusline.statusline = true;
        statusline.hud_payload = Some(payload);
        owner.apply_hook(current.clone(), statusline, &mut events);
        let poll_reminders = |owner: &mut ShellOwner| {
            owner.next_context_reminder_poll = Instant::now();
            let mut events = Vec::new();
            owner.poll_context_reminders(&mut events);
            events
                .into_iter()
                .filter_map(|event| match event {
                    OwnerEvent::Domain(commit) => Some(commit.result["agentNotification"].clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        config.set_agent_settings(
            "claude-code",
            matou_native_core::context_optimizer_config::AgentOptimizerSettings {
                enabled: false,
                cache_expiry_notify: true,
                ..Default::default()
            },
        )?;
        assert!(poll_reminders(&mut owner).is_empty());
        assert!(owner.agent_hud(session, root.path())?["hud"]["contextGuard"].is_null());
        config.set_agent_settings(
            "claude-code",
            matou_native_core::context_optimizer_config::AgentOptimizerSettings {
                cache_expiry_notify: true,
                ..Default::default()
            },
        )?;
        let notices = poll_reminders(&mut owner);
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[0]["eventType"], "context-water-critical");
        assert_eq!(notices[1]["eventType"], "context-cache-expiry");
        for notice in notices {
            assert_eq!(notice["sessionId"], session);
            assert_eq!(notice["taskId"], "task");
            assert_eq!(notice["workspaceId"], "workspace");
        }
        let hud = owner.agent_hud(session, root.path())?;
        assert_eq!(hud["hud"]["contextUsedTokens"], 175000);
        assert_eq!(hud["hud"]["contextGuard"]["cache"]["timing"], "estimate");
        assert!(poll_reminders(&mut owner).is_empty());

        let compact = json!({
            "session_id": "current-id", "hook_event_name": "SessionStart",
            "source": "compact", "prompt_id": "fixture-compact"
        });
        let complete = || {
            let mut observation = Observation::parse(compact.as_object().unwrap());
            observation.hud_payload = Some(compact.clone());
            observation
        };
        owner.apply_hook(current.clone(), complete(), &mut events);
        owner.apply_hook(current.clone(), complete(), &mut events);
        let hud = owner.agent_hud(session, root.path())?;
        assert_eq!(hud["hud"]["contextGuard"]["contextPressure"], "normal");
        assert_eq!(hud["hud"]["contextGuard"]["cache"]["timing"], "unknown");
        assert!(poll_reminders(&mut owner).is_empty());
        let after_context_reminders = notification_count(&owner)?;
        assert_eq!(after_context_reminders, previous_notifications + 3);

        owner.retire_sessions(&[session.into()]);
        owner.apply_hook(current, stop("current-id"), &mut events);
        assert_eq!(work_state(&owner, session)?, before);
        assert_eq!(notification_count(&owner)?, after_context_reminders);
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner.has_shell_worker(session) && Instant::now() < deadline {
            owner.poll(Duration::from_millis(25))?;
        }
        let binding = owner
            .claude_binding(session)?
            .context("existing provider binding")?;
        assert!(binding.resumable);
        owner.launch_claude(session.into(), terminal, plan(), Some(&binding))?;
        let resume = Scope {
            profile: LaunchProfile::ClaudeCode,
            session_id: session.into(),
            run_id: owner.shells[session].run_id.clone(),
            generation: owner.generation.clone(),
            expected_id: Some(binding.provider_session_id.clone()),
            fork_source_id: None,
        };
        assert_eq!(resume.expected_id.as_deref(), Some("current-id"));
        assert!(owner.waiting_for_provider_restore(session));
        assert!(!owner.can_launch_provider(session));
        let last = events.len();
        for event in ["SessionEnd", "unknown"] {
            owner.apply_hook(
                resume.clone(),
                observation(event, Some("wrong-id")),
                &mut events,
            );
        }
        assert_eq!(events.len(), last);
        owner.apply_hook(
            Scope {
                generation: "old-generation".into(),
                ..resume.clone()
            },
            observation("SessionStart", Some("current-id")),
            &mut events,
        );
        assert!(
            matches!(events.last(), Some(OwnerEvent::ProviderIdentity(event))
            if event["reason"] == "retired-run")
        );
        owner
            .shells
            .get_mut(session)
            .unwrap()
            .hook
            .as_mut()
            .unwrap()
            .deadline = Some(Instant::now());
        owner.poll_provider_hooks(&mut events);
        assert!(!owner.waiting_for_provider_restore(session));
        assert!(!owner.provider_allows_derivation(session));
        assert!(
            matches!(events.last(), Some(OwnerEvent::ProviderIdentity(event))
            if event["result"] == "timed-out"
                && event["restoreState"] == "none"
                && event["provider"] == "claude-code"
                && event["sessionId"] == session
                && event["runId"] == resume.run_id
                && event["providerSessionId"] == "current-id")
        );
        owner.apply_hook(
            resume.clone(),
            observation("SessionStart", Some("current-id")),
            &mut events,
        );
        assert!(owner.provider_allows_derivation(session));
        assert_eq!(
            owner.claude_binding(session)?.unwrap().restore_state,
            "none"
        );
        // Even a later authoritative report cannot change a targeted run's identity.
        owner.apply_hook(
            resume.clone(),
            observation("Stop", Some("CURRENT-ID")),
            &mut events,
        );
        assert!(!owner.provider_allows_derivation(session));
        assert_eq!(count(&owner)?, 2);
        assert_eq!(notification_count(&owner)?, after_context_reminders);
        assert_eq!(
            owner.claude_binding(session)?.unwrap().restore_state,
            "failed"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner.has_shell_worker(session) && Instant::now() < deadline {
            owner.poll(Duration::from_millis(25))?;
        }
        assert!(owner.can_launch_provider(session));
        owner.apply_hook(resume, observation("Stop", Some("current-id")), &mut events);
        assert!(
            matches!(events.last(), Some(OwnerEvent::ProviderIdentity(event))
            if event["reason"] == "retired-run")
        );

        assert!(owner.change_claude_restore(session, ClaudeRestore::Retry)?);
        assert!(!owner.change_claude_restore(session, ClaudeRestore::Retry)?);
        let binding = owner.claude_binding(session)?.context("retried binding")?;
        fs::write(&executable, "#!/bin/sh\nexit 23\n")?;
        owner.launch_claude(session.into(), terminal, plan(), Some(&binding))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut failure_seen = false;
        while owner.has_shell_worker(session) && Instant::now() < deadline {
            for event in owner.poll(Duration::from_millis(25))? {
                if let OwnerEvent::ProviderIdentity(event) = event
                    && event["result"] == "restore-failed"
                {
                    assert_eq!(event["providerSessionId"], "current-id");
                    assert_eq!(event["restoreError"], "provider resume exited with code 23");
                    failure_seen = true;
                }
            }
        }
        assert!(failure_seen);
        assert!(owner.can_launch_provider(session));
        owner.shutdown()?;
        assert!(
            fs::read_dir(root.path().join("provider-hooks"))?
                .next()
                .is_none()
        );
        Ok(())
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "context_guard/rescue_tests.rs"]
mod context_rescue_tests;
