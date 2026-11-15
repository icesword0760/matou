use std::{collections::HashMap, sync::mpsc, time::Duration};

use anyhow::{Context, Result, ensure};
use matou_native_core::{ownership::DatabaseOwner, shell_history};
use rusqlite::Connection;

use crate::{
    SessionEvent, SessionOptions,
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};

struct RunningShell {
    worker: SessionWorker,
    // The frozen Shell history collector records the launch cwd.
    cwd: String,
}

/// Serialized database-side owner of ordinary Shell workers.
///
/// Construct on the runtime thread after the database bootstrap has acquired its
/// lease, configured/migrated the connection, and handled backup/recovery. The UI
/// forwards requests here; it must not call blocking launch/shutdown on its thread.
pub struct ShellOwner {
    shells: HashMap<String, RunningShell>,
    send: mpsc::Sender<WorkerEvent>,
    receive: mpsc::Receiver<WorkerEvent>,
    connection: Connection,
    // Field order keeps the lease until after SQLite closes.
    _lease: DatabaseOwner,
    next_id: Box<dyn FnMut() -> String>,
}

impl ShellOwner {
    pub fn new(
        connection: Connection,
        lease: DatabaseOwner,
        next_id: impl FnMut() -> String + 'static,
    ) -> Self {
        let (send, receive) = mpsc::channel();
        Self {
            shells: HashMap::new(),
            send,
            receive,
            connection,
            _lease: lease,
            next_id: Box::new(next_id),
        }
    }

    /// The caller has already resolved environment, geometry and Shell executable.
    /// Restoration comes from SQLite, never a previous live PTY or raw transcript.
    pub fn launch(
        &mut self,
        mut options: SessionOptions,
        restore_history: bool,
        divider: &str,
    ) -> Result<()> {
        ensure!(
            !self.shells.contains_key(&options.session_id),
            "Shell worker already exists"
        );
        let (kind, cwd): (String, String) = self
            .connection
            .query_row(
                "SELECT kind, cwd FROM sessions WHERE id = ?",
                [&options.session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context("read the authoritative Shell session")?;
        ensure!(kind == "shell", "session is not an ordinary Shell");
        ensure!(
            std::path::Path::new(&cwd) == options.launch.cwd,
            "Shell launch cwd differs from its stored session"
        );
        let blocks =
            shell_history::list_for_launch(&self.connection, &options.session_id, restore_history)?;
        options.restored_history =
            shell_history::format_shell_history_for_terminal(&blocks, divider).into_bytes();
        let session_id = options.session_id.clone();
        let worker = SessionWorker::spawn(options, self.send.clone())?;
        self.shells.insert(session_id, RunningShell { worker, cwd });
        Ok(())
    }

    /// Forward without waiting for that session's disk or PTY. The reply goes
    /// directly to the requester's one-shot channel, leaving this owner available.
    pub fn send(&self, session_id: &str, command: WorkerCommand) -> Result<()> {
        self.shells
            .get(session_id)
            .context("Shell worker does not exist")?
            .worker
            .send(command)
    }

    /// Process one received batch. Completed blocks reach SQLite before their
    /// accompanying Output is returned to the UI-facing caller.
    pub fn poll(&mut self, max_wait: Duration) -> Result<Vec<WorkerEvent>> {
        let mut events = Vec::new();
        match self.receive.recv_timeout(max_wait) {
            Ok(event) => self.apply(event, &mut events),
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(events),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!("Shell event channel disconnected"));
            }
        }
        // Bound one turn so sustained producers do not starve domain commands.
        for _ in 0..127 {
            let Ok(event) = self.receive.try_recv() else {
                break;
            };
            self.apply(event, &mut events);
        }
        Ok(events)
    }

    fn apply(&mut self, event: WorkerEvent, events: &mut Vec<WorkerEvent>) {
        if let SessionEvent::CompletedBlock(block) = &event.event {
            let result = (|| {
                let cwd = &self
                    .shells
                    .get(&event.session_id)
                    .context("completed Shell is not registered")?
                    .cwd;
                shell_history::complete(
                    &mut self.connection,
                    &(self.next_id)(),
                    &event.session_id,
                    cwd,
                    block.clone(),
                )?;
                Ok::<_, anyhow::Error>(())
            })();
            if let Err(error) = result {
                // As in the frozen runtime, supplementary history failure is
                // reported but does not turn saved terminal output into a fault.
                events.push(WorkerEvent {
                    session_id: event.session_id.clone(),
                    event: SessionEvent::Diagnostic {
                        message: format!("save completed Shell history: {error:#}"),
                    },
                });
            }
        }
        events.push(event);
    }

    pub fn shutdown(&mut self) -> Result<Vec<WorkerEvent>> {
        let mut first_error = None;
        for shell in self.shells.values_mut() {
            if let Err(error) = shell.worker.shutdown() {
                first_error.get_or_insert(error);
            }
        }
        let mut events = Vec::new();
        while let Ok(event) = self.receive.try_recv() {
            self.apply(event, &mut events);
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
        // Drain already committed command blocks before the central database closes.
        let _ = self.shutdown();
    }
}
