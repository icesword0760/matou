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
    session_graph,
    sessions::{self, SessionRun, StartShellRunInput},
    shell_history,
    storage::{CommandMetadata, DomainCommit},
};
use rusqlite::{Connection, params};
use serde_json::Value;

use crate::{
    SessionEvent, SessionOptions,
    derived::{CwdTracker, WorkStatusTracker, terminal_summary_lines, utf16_tail},
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};

struct RunningShell {
    worker: SessionWorker,
    run_id: String,
    cwd: String,
    restore_history: bool,
    cwd_tracker: CwdTracker,
    work_tracker: WorkStatusTracker,
    emitted_output: bool,
    ended: bool,
    retiring: bool,
    cwd_capture: Option<(Instant, bool)>,
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

    pub(crate) fn can_restart_shell(&self, session_id: &str, start_failed: bool) -> Result<bool> {
        Ok(!self.closing
            && self
                .shells
                .get(session_id)
                .is_none_or(|shell| shell.ended && !shell.retiring)
            && self.session(session_id)?.is_some_and(|session| {
                session.kind == "shell"
                    && session.archived_at.is_none()
                    && (start_failed || session.status == "exited")
            }))
    }

    /// Resolve environment and geometry before calling. Startup handshaking stays
    /// on the runtime thread; subsequent session requests are forwarded without waiting.
    pub fn launch(
        &mut self,
        mut options: SessionOptions,
        restore_history: bool,
        divider: &str,
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
            .context("read the authoritative Shell session")?;
        ensure!(stored.kind == "shell", "session is not an ordinary Shell");
        ensure!(
            stored.archived_at.is_none(),
            "archived Session cannot start a new run"
        );
        ensure!(
            Path::new(&stored.cwd) == options.launch.cwd,
            "Shell launch cwd differs from its stored session"
        );
        options.collect_history = restore_history;
        let blocks =
            shell_history::list_for_launch(&self.connection, &options.session_id, restore_history)?;
        options.restored_history =
            shell_history::format_shell_history_for_terminal(&blocks, divider).into_bytes();
        let session_id = options.session_id.clone();
        let cols = options.launch.cols.into();
        let rows = options.launch.rows.into();
        let run_id = (self.next_id)();
        let mut worker = SessionWorker::spawn(options, self.send.clone(), run_id.clone())?;
        let time = now();
        let commit = sessions::start_shell_run(
            &mut self.connection,
            &command(
                format!("runtime-start-{run_id}"),
                "session.run-start",
                format!("start:{run_id}:{session_id}:shell"),
            ),
            &StartShellRunInput {
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
        self.shells.insert(
            session_id,
            RunningShell {
                worker,
                run_id,
                cwd: stored.cwd,
                restore_history,
                cwd_tracker: CwdTracker::default(),
                work_tracker: WorkStatusTracker::default(),
                emitted_output: false,
                ended: false,
                retiring: false,
                cwd_capture: None,
            },
        );
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(())
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
        self.flush_due_summaries(&mut events);
        self.start_due_cwd_captures();
        Ok(events)
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
            SessionEvent::InputSubmitted => {
                if let Some(shell) = self.shells.get_mut(&event.session_id) {
                    shell.cwd_capture = Some((Instant::now() + Duration::from_millis(180), true));
                }
                if let Err(error) = self.set_work_status(&event.session_id, "running", events) {
                    diagnostic(
                        events,
                        &event.session_id,
                        format!("persist submitted Shell input: {error:#}"),
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
        let summary = self.summaries.entry(id.to_owned()).or_default();
        summary.raw.push_str(data);
        summary.raw = utf16_tail(&summary.raw, 32_768);
        summary
            .deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(180));
        let shell = self.shells.get_mut(id).expect("registered Shell output");
        shell.emitted_output = true;
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
            if let Err(error) = self.set_work_status(id, &status, events) {
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

    fn set_work_status(
        &mut self,
        id: &str,
        status: &str,
        events: &mut Vec<OwnerEvent>,
    ) -> Result<()> {
        let current: String = self.connection.query_row(
            "SELECT work_status FROM sessions WHERE id=?",
            [id],
            |row| row.get(0),
        )?;
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
        shell.cwd_capture = None;
        let shutdown = disposed && self.closing && !retiring;
        let run = shell.run_id.clone();
        let pid = shell.worker.process_id();
        let startup_failed = !shell.emitted_output && !shutdown && !retiring;
        if shutdown {
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
        if startup_failed {
            self.set_work_status(id, "error", events)?;
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
