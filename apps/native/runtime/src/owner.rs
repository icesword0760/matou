use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::{
    hierarchy::{self, HierarchyResult, Scene, Session as StoredSession, canvas},
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

    pub fn hierarchy(&self, window_id: &str) -> Result<HierarchyResult> {
        hierarchy::read_hierarchy(&self.connection, window_id)
    }

    pub fn scenes(&self, task_id: &str) -> Result<Vec<Scene>> {
        hierarchy::list_scenes(&self.connection, task_id)
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
            ensure!(previous.ended, "Shell worker already exists");
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
                cwd_capture: None,
            },
        );
        self.pending.push(OwnerEvent::Domain(commit));
        Ok(())
    }

    pub fn send(&self, session_id: &str, command: WorkerCommand) -> Result<()> {
        self.shells
            .get(session_id)
            .context("Shell worker does not exist")?
            .worker
            .send(command)
    }

    pub fn poll(&mut self, max_wait: Duration) -> Result<Vec<OwnerEvent>> {
        let mut events = std::mem::take(&mut self.pending);
        self.start_due_cwd_captures();
        let wait = if events.is_empty() {
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
        for _ in 0..127 {
            let Ok(event) = self.receive.try_recv() else {
                break;
            };
            self.apply(event, &mut events);
        }
        self.flush_due_summaries(&mut events);
        self.start_due_cwd_captures();
        Ok(events)
    }

    fn apply(&mut self, event: WorkerEvent, events: &mut Vec<OwnerEvent>) {
        // A failed run-start transaction has already shut down its unregistered PTY.
        if !self
            .shells
            .get(&event.session_id)
            .is_some_and(|shell| shell.run_id == event.run_id)
        {
            return;
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
        self.flush_summary(id, events);
        let shell = self.shells.get_mut(id).expect("registered Shell exit");
        if shell.ended {
            return Ok(());
        }
        shell.ended = true;
        shell.cwd_capture = None;
        let shutdown = disposed && self.closing;
        let run = shell.run_id.clone();
        let pid = shell.worker.process_id();
        let startup_failed = !shell.emitted_output && !shutdown;
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
        let mut first_error = None;
        for shell in self.shells.values_mut() {
            if let Err(error) = shell.worker.shutdown() {
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
}

impl Drop for ShellOwner {
    fn drop(&mut self) {
        let _ = self.shutdown();
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
