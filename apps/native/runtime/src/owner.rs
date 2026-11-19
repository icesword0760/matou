use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::{
    backup::{BackupService, CLEAN_EXIT_BACKUP_TIMEOUT},
    hierarchy::{
        self, HierarchyIds, HierarchyResult, Scene, Session as StoredSession, Task, Workspace,
        canvas, cards, tabs, tasks, workspaces,
    },
    ownership::DatabaseOwner,
    provider_launch::{ClaudeLaunch, attach_claude_settings},
    pty::LaunchProfile,
    session_graph,
    sessions::{self, ClaudeBinding, ClaudeRestore, SessionRun, StartRunInput},
    shell_history,
    storage::{CommandMetadata, DomainCommit},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::{
    SessionEvent, SessionOptions,
    derived::{CwdTracker, WorkStatusTracker, terminal_summary_lines, utf16_tail},
    provider_hook::{
        HookMessage, HookSetup, IDENTITY_TIMEOUT, Observation, ProviderHookServer, Registration,
        Scope,
    },
    provider_resume::{IDENTITY_MISMATCH, ResumeMonitor},
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};

struct RunningShell {
    worker: SessionWorker,
    run_id: String,
    cwd: String,
    is_shell: bool,
    restore_history: bool,
    cwd_tracker: CwdTracker,
    work_tracker: WorkStatusTracker,
    emitted_output: bool,
    ended: bool,
    retiring: bool,
    cwd_capture: Option<(Instant, bool)>,
    hook: Option<RunHook>,
}

struct RunHook {
    _registration: Registration,
    launch_metadata: Value,
    secrets: Vec<String>,
    deadline: Option<Instant>,
    last_observation: Option<Observation>,
    last_submission: Option<Instant>,
    resume: Option<ResumeMonitor>,
    reported_missing: bool,
}

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
    Diagnostic {
        session_id: String,
        message: String,
    },
}

/// Serialized database-side owner, used on the runtime thread, never the UI thread.
pub struct ShellOwner {
    shells: HashMap<String, RunningShell>,
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
    closing: bool,
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
        let (send, receive) = mpsc::channel();
        let mut owner = Self {
            shells: HashMap::new(),
            summaries: HashMap::new(),
            send,
            receive,
            pending: Vec::new(),
            connection,
            _lease: lease,
            generation,
            next_id: Box::new(next_id),
            hook_server: None,
            closing: false,
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
        Ok(owner)
    }

    pub fn session(&self, id: &str) -> Result<Option<StoredSession>> {
        sessions::get_session(&self.connection, id)
    }
    pub fn runs(&self, id: &str) -> Result<Vec<SessionRun>> {
        sessions::list_runs(&self.connection, id)
    }
    pub fn graph(&self, scene_id: &str, window_id: Option<&str>) -> Result<Value> {
        session_graph::project_scene_graph(&self.connection, scene_id, window_id)
    }

    pub(crate) fn scene_workspace_session_count(&self, scene_id: &str) -> Result<i64> {
        cards::scene_workspace_session_count(&self.connection, scene_id)
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
        // The owner serializes writes between the shortcut check and the workflow.
        if cards::close_shortcut_is_last(&self.connection, &input.scene_id, &input.session_id)? {
            return Ok(serde_json::json!({
                "outcome": "hide-window",
                "disposedSessionIds": [],
                "rebuildCanvas": false,
            }));
        }
        let run_id = self
            .shells
            .get(&input.session_id)
            .filter(|shell| !shell.retiring && !shell.ended)
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
        for id in disposed {
            self.summaries.remove(id);
            if let Some(shell) = self.shells.get_mut(id)
                && !shell.retiring
            {
                shell.retiring = true;
                shell.hook = None;
                shell.cwd_capture = None;
                // Keep the run registered until its final exit record is handled.
                // If the channel already closed, poll will reap and report the worker.
                let _ = shell.worker.send(WorkerCommand::Shutdown { reply: None });
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
                matches!(session.kind.as_str(), "shell" | "claude-code")
                    && session.archived_at.is_none()
                    && (start_failed || session.status == "exited")
            })
            && !self
                .claude_binding(session_id)?
                .is_some_and(|binding| binding.restore_state == "failed"))
    }

    pub(crate) fn claude_binding(&self, session_id: &str) -> Result<Option<ClaudeBinding>> {
        sessions::claude_binding(&self.connection, session_id)
    }

    pub(crate) fn load_claude_catalog_binding(
        &mut self,
        input: &sessions::ClaudeCatalogLoad<'_>,
    ) -> Result<i64> {
        let id = (self.next_id)();
        let commit = sessions::load_claude_catalog_binding(
            &mut self.connection,
            &command(id.clone(), "session.load-provider-catalog", id),
            input,
            now(),
        )?;
        let revision = commit.result["spawnRevision"]
            .as_i64()
            .context("catalog binding is missing its spawn revision")?;
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(revision)
    }

    pub(crate) fn catalog_binding_owners(
        &self,
        provider_ids: &[&str],
    ) -> Result<HashMap<String, Value>> {
        let mut query = self.connection.prepare(
            "SELECT binding.provider_session_id, session.id, session.title
             FROM provider_bindings AS binding
             JOIN sessions AS session ON session.id = binding.session_id
             JOIN tasks ON tasks.id = session.task_id
             JOIN workspaces ON workspaces.id = tasks.workspace_id
             WHERE binding.provider = 'claude-code' AND binding.resume_state = 'available'
               AND binding.invalidated_at IS NULL AND binding.user_exited_at IS NULL
               AND session.kind = 'claude-code' AND session.archived_at IS NULL
               AND tasks.archived_at IS NULL AND workspaces.archived_at IS NULL
               AND binding.provider_session_id IN (SELECT value FROM json_each(?))
             ORDER BY binding.updated_at DESC, session.id",
        )?;
        let rows = query.query_map([serde_json::to_string(provider_ids)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut owners = HashMap::new();
        for row in rows {
            let (provider_id, session_id, title) = row?;
            owners
                .entry(provider_id)
                .or_insert_with(|| json!({"sessionId": session_id, "title": title}));
        }
        Ok(owners)
    }

    pub(crate) fn startup_provider_restores(
        &self,
        scene_id: Option<&str>,
    ) -> Result<Vec<(String, String)>> {
        let mut query = self.connection.prepare(
            "SELECT sessions.id, tasks.workspace_id FROM sessions
             JOIN tasks ON tasks.id = sessions.task_id AND tasks.archived_at IS NULL
             JOIN workspaces ON workspaces.id = tasks.workspace_id AND workspaces.archived_at IS NULL
             WHERE sessions.kind = 'claude-code' AND sessions.archived_at IS NULL
               AND EXISTS (
                   SELECT 1 FROM session_mounts JOIN scenes ON scenes.id = session_mounts.scene_id
                   WHERE session_mounts.session_id = sessions.id AND scenes.archived_at IS NULL
               )
             ORDER BY EXISTS (
                   SELECT 1 FROM session_mounts WHERE session_id = sessions.id AND scene_id = ?
               ) DESC, sessions.last_activity_at DESC, sessions.id",
        )?;
        let rows = query.query_map([scene_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?))
        })?;
        let mut selected = Vec::new();
        for row in rows {
            let (id, workspace) = row?;
            if self
                .claude_binding(&id)?
                .is_some_and(|binding| binding.resumable)
            {
                selected.push((id, workspace));
                if selected.len() == 16 {
                    break;
                }
            }
        }
        Ok(selected)
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
            ClaudeRestore::Fail { error, .. } => ("restore-failed", error == IDENTITY_MISMATCH),
            ClaudeRestore::Retry => ("retrying", false),
            ClaudeRestore::Fresh => ("starting-fresh", false),
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

    pub(crate) fn waiting_for_provider_restore(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .and_then(|shell| shell.hook.as_ref())
            .is_some_and(|hook| hook.resume.is_some() && hook.deadline.is_some())
    }

    pub(crate) fn provider_allows_derivation(&self, session_id: &str) -> bool {
        self.shells.get(session_id).is_some_and(|shell| {
            !shell.retiring
                && (shell.is_shell
                    || shell
                        .hook
                        .as_ref()
                        .is_some_and(|hook| hook.last_observation.is_some()))
        })
    }

    pub(crate) fn can_launch_provider(&self, session_id: &str) -> bool {
        self.shells
            .get(session_id)
            .is_none_or(|shell| shell.ended && !shell.retiring && shell.worker.is_finished())
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
        self.launch_run(options, restore_history, divider, None)
    }

    pub(crate) fn launch_claude(
        &mut self,
        session_id: String,
        terminal: matou_native_terminal::TerminalOptions,
        plan: ClaudeLaunch,
        binding: Option<&ClaudeBinding>,
    ) -> Result<()> {
        let mut launch = plan.launch;
        launch.cols = terminal.cols;
        launch.rows = terminal.rows;
        let secrets = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
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
        let safe = |value: &str| safe_hook_text(value, &secrets).unwrap_or("").to_owned();
        let metadata = json!({
            "providerConfigId": safe(&plan.provider_config_id), "model": safe(&plan.model),
            "permissionMode": safe(&plan.permission_mode), "cwd": safe(&launch.cwd.to_string_lossy()),
        });
        let mut metadata = metadata;
        if let Some(revision) = binding.and_then(|binding| binding.metadata.get("spawnRevision")) {
            metadata["spawnRevision"] = revision.clone();
        }
        let resume = binding.filter(|binding| binding.resumable).map(|binding| {
            ResumeMonitor::new(binding, plan.permission_mode == "bypassPermissions")
        });
        self.launch_run(
            SessionOptions {
                session_id,
                terminal,
                launch,
                journal: Default::default(),
                restored_history: Vec::new(),
                collect_history: false,
            },
            false,
            "",
            Some((metadata, secrets, resume)),
        )
    }

    fn launch_run(
        &mut self,
        mut options: SessionOptions,
        restore_history: bool,
        divider: &str,
        provider: Option<(Value, Vec<String>, Option<ResumeMonitor>)>,
    ) -> Result<()> {
        ensure!(!self.closing, "Runtime is shutting down");
        if let Some(previous) = self.shells.get(&options.session_id) {
            ensure!(
                previous.ended && !previous.retiring,
                "Shell worker already exists or is retiring"
            );
        }
        if let Some(mut previous) = self.shells.remove(&options.session_id) {
            previous.worker.shutdown()?;
        }
        let stored = self
            .session(&options.session_id)?
            .context("read the authoritative session")?;
        let profile = options.launch.profile;
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
        let session_id = options.session_id.clone();
        let cols = options.launch.cols.into();
        let rows = options.launch.rows.into();
        let run_id = (self.next_id)();
        let hook = if let Some((launch_metadata, secrets, resume)) = provider {
            let registration = self
                .register_hook(
                    &mut options,
                    &run_id,
                    resume.as_ref().map(|resume| resume.expected_id.clone()),
                )
                .context(HookSetup)?;
            Some(RunHook {
                _registration: registration,
                launch_metadata,
                secrets,
                deadline: Some(Instant::now() + IDENTITY_TIMEOUT),
                last_observation: None,
                last_submission: None,
                resume,
                reported_missing: false,
            })
        } else {
            None
        };
        let mut worker = SessionWorker::spawn(options, self.send.clone(), run_id.clone())?;
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
                let _ = worker.shutdown();
                return Err(error);
            }
        };
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
                restore_history,
                cwd_tracker: CwdTracker::default(),
                work_tracker: WorkStatusTracker::default(),
                emitted_output: false,
                ended: false,
                retiring: false,
                cwd_capture: None,
                hook,
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
    ) -> Result<Registration> {
        if self.hook_server.is_none() {
            self.hook_server = Some(ProviderHookServer::start()?);
        }
        let registration = self.hook_server.as_ref().unwrap().register(
            &options.launch.data_root,
            Scope {
                session_id: options.session_id.clone(),
                run_id: run_id.to_owned(),
                generation: self.generation.clone(),
                expected_id,
            },
        )?;
        attach_claude_settings(&mut options.launch, &registration.settings_path)?;
        Ok(registration)
    }

    pub fn send(&self, session_id: &str, command: WorkerCommand) -> Result<()> {
        self.shells
            .get(session_id)
            .filter(|shell| !shell.retiring)
            .context("Shell worker does not exist")?
            .worker
            .send(command)
    }

    pub fn poll(&mut self, max_wait: Duration) -> Result<Vec<OwnerEvent>> {
        let mut events = std::mem::take(&mut self.pending);
        let finished_before_poll: Vec<_> = self
            .shells
            .iter()
            .filter(|(_, shell)| shell.retiring && !shell.ended && shell.worker.is_finished())
            .map(|(id, _)| id.clone())
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
            match shell.worker.join_after_shutdown() {
                Err(error) => diagnostic(
                    &mut events,
                    id,
                    format!("shut down closed Shell session: {error:#}"),
                ),
                Ok(()) if !shell.ended => {
                    diagnostic(
                        &mut events,
                        id,
                        "closed Shell stopped before recording its exit".into(),
                    );
                }
                Ok(()) => {}
            }
            false
        });
        self.poll_provider_hooks(&mut events);
        self.flush_due_summaries(&mut events);
        self.start_due_cwd_captures();
        Ok(events)
    }

    fn poll_provider_hooks(&mut self, events: &mut Vec<OwnerEvent>) {
        let Some(server) = &self.hook_server else {
            return;
        };
        let hooks: Vec<_> = server.messages().take(64).collect();
        for hook in hooks {
            match hook {
                HookMessage::Observed(scope, observation) => {
                    self.apply_hook(scope, observation, events)
                }
                HookMessage::Retired(scope) => reject_identity(events, &scope, "retired-run"),
            }
        }
        for (session_id, shell) in &mut self.shells {
            if let Some(hook) = &mut shell.hook
                && hook
                    .deadline
                    .is_some_and(|deadline| deadline <= Instant::now())
            {
                hook.deadline = None;
                if let Some(resume) = &mut hook.resume {
                    resume.release_readiness();
                }
                events.push(OwnerEvent::ProviderIdentity(json!({
                    "result": "timed-out", "sessionId": session_id, "runId": shell.run_id,
                    "provider": "claude-code",
                    "providerSessionId": hook.resume.as_ref().map(|resume| &resume.expected_id),
                    "restoreState": if hook.resume.is_some() { "restoring" } else { "none" },
                    "restoreError": null, "spawnRevision": hook.launch_metadata.get("spawnRevision"),
                })));
            }
        }
        for (session_id, shell) in &mut self.shells {
            if let Some(bytes) = shell
                .hook
                .as_mut()
                .filter(|hook| hook.last_observation.is_none())
                .and_then(|hook| hook.resume.as_mut())
                .and_then(ResumeMonitor::due_input)
            {
                write_resume_input(session_id, shell, bytes, events);
            }
        }
    }

    fn apply_hook(
        &mut self,
        scope: Scope,
        mut observation: Observation,
        events: &mut Vec<OwnerEvent>,
    ) {
        let Some(hook) = self
            .shells
            .get_mut(&scope.session_id)
            .filter(|shell| {
                !self.closing
                    && !shell.ended
                    && !shell.retiring
                    && shell.run_id == scope.run_id
                    && scope.generation == self.generation
            })
            .and_then(|shell| shell.hook.as_mut())
        else {
            reject_identity(events, &scope, "retired-run");
            return;
        };
        if matches!(observation.event, "unknown" | "SessionEnd") {
            return;
        }
        if let (Some(expected), Some(identity)) = (&scope.expected_id, &observation.identity)
            && expected != identity
            && let Some(resume) = &hook.resume
        {
            let binding_id = resume.binding_id.clone();
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
        observation.model = safe(observation.model);
        observation.permission_mode = safe(observation.permission_mode);
        observation.cwd = safe(observation.cwd)
            .filter(|cwd| Path::new(cwd).is_absolute() && Path::new(cwd).is_dir());
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
            let Ok(commit) = commit else {
                reject_identity(events, &scope, "binding-write-failed");
                return;
            };
            hook.deadline = None;
            hook.last_observation = Some(observation);
            if confirms {
                let mut payload = commit.result.clone();
                payload["result"] = json!("confirmed");
                events.push(OwnerEvent::ProviderIdentity(payload));
            }
            events.push(OwnerEvent::Domain(commit));
        }
        // Worker events and HTTP hooks arrive on separate queues. A queued
        // hook must not overwrite a newer submission already applied above it.
        if let Some((kind, received_at)) = notification
            && !confirms
            && hook
                .last_submission
                .is_none_or(|submitted| submitted <= received_at)
            && let Err(error) = self.set_work_status(&scope.session_id, events, |current| {
                kind.work_status(current)
            })
        {
            diagnostic(
                events,
                &scope.session_id,
                format!("persist provider hook work status: {error:#}"),
            );
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
        self.retire_sessions(&[session_id.to_owned()]);
        if let Err(error) = self.commit_claude_restore(
            session_id,
            ClaudeRestore::Fail { binding_id, error },
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
                if shell.ended {
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
                }
                if let Err(error) = self.set_work_status(&event.session_id, events, |_| "running") {
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
                status, disposed, ..
            } => self.finish_run(&event.session_id, *status, *disposed, events),
            _ => {
                events.push(OwnerEvent::Terminal(event));
                return;
            }
        };
        if let Err(error) = result {
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
        if !self.closing
            && let Some(hook) = &mut shell.hook
            && hook.last_observation.is_none()
            && let Some(resume) = &mut hook.resume
        {
            let (failure, input) = resume.ingest(data);
            if let Some(error) = failure {
                let run_id = shell.run_id.clone();
                let binding_id = resume.binding_id.clone();
                self.park_provider_run(id, &run_id, &binding_id, error, events);
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
            if let Err(error) = self.set_work_status(id, events, |_| &status) {
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
        next_status: impl FnOnce(&str) -> &'a str,
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
        let status = next_status(&current);
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
        events: &mut Vec<OwnerEvent>,
    ) -> Result<()> {
        let retiring = self.shells[id].retiring;
        if !retiring {
            self.flush_summary(id, events);
        }
        let shell = self.shells.get_mut(id).expect("registered Shell exit");
        if shell.ended {
            return Ok(());
        }
        shell.ended = true;
        let hook = shell.hook.take();
        let had_identity = hook
            .as_ref()
            .is_some_and(|hook| hook.last_observation.is_some());
        let pending_restore = hook.and_then(|hook| hook.resume).filter(|_| !had_identity);
        shell.cwd_capture = None;
        let shutdown = disposed && self.closing && !retiring;
        let run = shell.run_id.clone();
        let pid = shell.worker.process_id();
        let startup_failed =
            !shell.emitted_output && !shutdown && !retiring && (shell.is_shell || had_identity);
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
        let (prefix, hash) = if shutdown {
            ("runtime-shutdown", "shutdown")
        } else {
            ("runtime-exit", "exit")
        };
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
        if !disposed
            && !self.closing
            && !retiring
            && let Some(resume) = pending_restore
        {
            self.park_provider_run(
                id,
                &run,
                &resume.binding_id,
                &format!("provider resume exited with code {}", status.exit_code),
                events,
            );
        } else if startup_failed {
            self.set_work_status(id, events, |_| "error")?;
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
        self.closing = true;
        for shell in self.shells.values_mut() {
            shell.hook = None;
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
                first_error.get_or_insert(error);
            }
        }
        let mut events = std::mem::take(&mut self.pending);
        while let Ok(event) = self.receive.try_recv() {
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

fn reject_identity(events: &mut Vec<OwnerEvent>, scope: &Scope, reason: &str) {
    events.push(OwnerEvent::ProviderIdentity(json!({
        "result": "rejected", "sessionId": scope.session_id, "runId": scope.run_id,
        "provider": "claude-code", "reason": reason,
    })));
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
        reply,
    }) {
        diagnostic(
            events,
            session_id,
            format!("answer Claude Code restore prompt: {error:#}"),
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
        let output = std::process::Command::new("lsof")
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

    fn assert_provider_work_status(owner: &mut ShellOwner, scope: &Scope) -> Result<()> {
        let session = &scope.session_id;
        let mut events = Vec::new();
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
        let mut stale_stop = hook("Stop", "");
        stale_stop.notification.as_mut().unwrap().1 -= Duration::from_millis(1);
        submit(owner, &mut events);
        owner.apply_hook(scope.clone(), stale_stop, &mut events);
        assert_eq!(work_state(owner, session)?, running);

        owner.apply_hook(scope.clone(), hook("Stop", ""), &mut events);
        assert_eq!(work_state(owner, session)?.0, "idle");
        submit(owner, &mut events);
        owner.apply_hook(scope.clone(), hook("Stop", ""), &mut events);
        let idle = work_state(owner, session)?;
        assert_eq!(idle.0, "idle");
        owner.apply_hook(scope.clone(), hook("Stop", ""), &mut events);
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
        assert_eq!(work_state(owner, session)?.0, "needs-input");

        owner
            .connection
            .execute("UPDATE sessions SET archived_at=1 WHERE id=?", [session])?;
        let archived = work_state(owner, session)?;
        owner.apply_hook(scope.clone(), hook("Notification", "Done"), &mut events);
        assert_eq!(work_state(owner, session)?, archived);
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
        let writes: i64 = owner.connection.query_row(
            "SELECT COUNT(*) FROM command_deduplication WHERE command_type='session.work-status'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(writes, committed_statuses.len() as i64);

        let (reply, result) = mpsc::channel();
        owner.send(
            session,
            WorkerCommand::Write {
                bytes: vec![b'\n'],
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
        owner.apply_hook(scope.clone(), hook("Stop", ""), &mut events);
        assert_eq!(work_state(owner, session)?, exited);
        Ok(())
    }

    #[test]
    fn current_provider_run_alone_can_project_identity() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("workspace");
        fs::create_dir(&cwd)?;
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
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'provider output before identity\\n'\nread -r line\n",
        )?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let plan = || ClaudeLaunch {
            launch: ShellLaunch {
                profile: LaunchProfile::ClaudeCode,
                executable: executable.clone(),
                argv: Vec::new(),
                cwd: cwd.clone(),
                data_root: root.path().to_owned(),
                inherited_env: Default::default(),
                env: Default::default(),
                cols: 80,
                rows: 24,
            },
            provider_config_id: "fixture-config".into(),
            model: "fixture-model".into(),
            permission_mode: "default".into(),
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
            session_id: session.into(),
            run_id: owner.shells[session].run_id.clone(),
            generation: owner.generation.clone(),
            expected_id: None,
        };
        let observation = |event, identity: Option<&str>| Observation {
            event,
            identity: identity.map(str::to_owned),
            model: None,
            permission_mode: None,
            cwd: None,
            notification: None,
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
        thread::scope(|threads| -> Result<()> {
            let post = || -> Result<()> {
                let mut socket = TcpStream::connect(("127.0.0.1", url.port().unwrap()))?;
                socket.set_read_timeout(Some(Duration::from_secs(3)))?;
                let body = r#"{"hook_event_name":"UserPromptSubmit","session_id":"first-id"}"#;
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
            let first = threads.spawn(post);
            let second = threads.spawn(post);
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
        assert_provider_work_status(&mut owner, &scope)?;

        owner.retire_sessions(&[session.into()]);
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner.has_shell_worker(session) && Instant::now() < deadline {
            owner.poll(Duration::from_millis(25))?;
        }
        assert!(!owner.has_shell_worker(session));
        owner.launch_claude(session.into(), terminal, plan(), None)?;
        let current = Scope {
            run_id: owner.shells[session].run_id.clone(),
            ..scope.clone()
        };
        let stop = |id| {
            let payload = json!({"hook_event_name": "Stop", "session_id": id});
            Observation::parse(payload.as_object().unwrap())
        };
        let before = work_state(&owner, session)?;
        owner.apply_hook(current.clone(), stop("current-id"), &mut events);
        assert!(owner.provider_allows_derivation(session));
        assert_eq!(work_state(&owner, session)?, before);
        owner.apply_hook(scope, stop("late-id"), &mut events);
        owner.apply_hook(
            Scope {
                generation: "old-generation".into(),
                ..current.clone()
            },
            stop("current-id"),
            &mut events,
        );
        assert_eq!(work_state(&owner, session)?, before);
        assert_eq!(count(&owner)?, 2);
        assert!(
            matches!(events.last(), Some(OwnerEvent::ProviderIdentity(value)) if value["reason"] == "retired-run")
        );
        owner.retire_sessions(&[session.into()]);
        owner.apply_hook(current, stop("current-id"), &mut events);
        assert_eq!(work_state(&owner, session)?, before);
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner.has_shell_worker(session) && Instant::now() < deadline {
            owner.poll(Duration::from_millis(25))?;
        }
        let binding = owner
            .claude_binding(session)?
            .context("existing provider binding")?;
        assert!(binding.resumable);
        assert!(owner.change_claude_restore(session, ClaudeRestore::Begin(&binding.id))?);
        owner.launch_claude(session.into(), terminal, plan(), Some(&binding))?;
        let resume = Scope {
            session_id: session.into(),
            run_id: owner.shells[session].run_id.clone(),
            generation: owner.generation.clone(),
            expected_id: Some(binding.provider_session_id.clone()),
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
            if event["result"] == "timed-out" && event["restoreState"] == "restoring")
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
