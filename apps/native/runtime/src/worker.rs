//! Thread-affine owner for one live Shell session.
use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use matou_native_terminal::{GridSelection, InputModes, Screen, SelectionAction, Theme};

use crate::{
    SessionEvent, SessionOptions,
    search::{SearchRequest, SearchResult},
    session::{Session, SessionWaker},
};

const MAX_POLL_WAIT: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct WorkerEvent {
    pub session_id: String,
    pub run_id: String,
    pub event: SessionEvent,
}

pub struct SelectionCopy {
    pub text: String,
    pub interrupted: bool,
}

/// Requests can be forwarded without blocking the central owner on a busy session.
pub enum WorkerCommand {
    Write {
        bytes: Vec<u8>,
        reply: Sender<Result<()>>,
    },
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        reply: Sender<Result<()>>,
    },
    Acknowledge {
        sequence: u64,
        reply: Sender<Result<()>>,
    },
    RetryStorage {
        reply: Sender<Result<()>>,
    },
    SetEgressFrozen {
        frozen: bool,
        reply: Sender<Result<()>>,
    },
    Screen {
        reply: Sender<Result<Screen>>,
    },
    View {
        reply: Sender<Result<(Screen, InputModes)>>,
    },
    SelectedView {
        reply: Sender<Result<(Screen, InputModes, Option<GridSelection>)>>,
    },
    Search {
        request: SearchRequest,
        reply: Sender<Result<SearchResult>>,
    },
    ClearSearch {
        reply: Sender<Result<()>>,
    },
    Select {
        action: SelectionAction,
        reply: Sender<Result<()>>,
    },
    Selection {
        reply: Sender<Result<Option<GridSelection>>>,
    },
    CopySelection {
        interrupt_if_empty: bool,
        reply: Sender<Result<SelectionCopy>>,
    },
    Scroll {
        lines: i32,
        reply: Sender<Result<()>>,
    },
    ScrollTo {
        row: u64,
        reply: Sender<Result<()>>,
    },
    ScrollSelection {
        lines: i32,
        reply: Sender<Result<()>>,
    },
    SetTheme {
        theme: Theme,
        reply: Sender<Result<()>>,
    },
    Shutdown {
        reply: Option<Sender<Result<()>>>,
    },
}

pub struct SessionWorker {
    commands: Sender<WorkerCommand>,
    wake: SessionWaker,
    process_id: u32,
    thread: Option<JoinHandle<Result<()>>>,
}

pub(crate) struct PendingShutdown {
    reply: Result<Receiver<Result<()>>>,
    thread: JoinHandle<Result<()>>,
}

impl PendingShutdown {
    pub(crate) fn wait(self) -> Result<()> {
        let result = self.reply.and_then(|reply| {
            reply
                .recv()
                .context("Shell session worker did not report shutdown")?
        });
        // The join result can retain an earlier fatal poll error even if
        // the worker stopped before receiving this shutdown command.
        join(self.thread)?;
        result
    }
}

impl SessionWorker {
    pub fn spawn(
        options: SessionOptions,
        events: Sender<WorkerEvent>,
        run_id: String,
    ) -> Result<Self> {
        let (commands, incoming) = mpsc::channel();
        let (started, startup) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("matou-shell-session".into())
            .spawn(move || {
                let session_id = options.session_id.clone();
                match Session::launch(options) {
                    Ok(mut session) => {
                        if started
                            .send(Ok((session.waker(), session.process_id())))
                            .is_err()
                        {
                            return finish(&mut session, &session_id, &run_id, &events);
                        }
                        run(&mut session, &session_id, &run_id, &incoming, &events)
                    }
                    Err(error) => {
                        let _ = started.send(Err(error));
                        Ok(())
                    }
                }
            })
            .context("start Shell session worker")?;
        match startup.recv() {
            Ok(Ok((wake, process_id))) => Ok(Self {
                commands,
                wake,
                process_id,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = join(thread);
                Err(error)
            }
            Err(error) => {
                join(thread)?;
                Err(error).context("Shell session worker did not report startup")
            }
        }
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        self.request(move |reply| WorkerCommand::Write {
            bytes: bytes.to_vec(),
            reply,
        })
    }

    pub fn resize(
        &self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        self.request(move |reply| WorkerCommand::Resize {
            cols,
            rows,
            cell_width_px,
            cell_height_px,
            reply,
        })
    }

    pub fn acknowledge(&self, sequence: u64) -> Result<()> {
        self.request(move |reply| WorkerCommand::Acknowledge { sequence, reply })
    }

    pub fn retry_storage(&self) -> Result<()> {
        self.request(|reply| WorkerCommand::RetryStorage { reply })
    }

    pub fn set_egress_frozen(&self, frozen: bool) -> Result<()> {
        self.request(move |reply| WorkerCommand::SetEgressFrozen { frozen, reply })
    }

    pub fn screen(&self) -> Result<Screen> {
        self.request(|reply| WorkerCommand::Screen { reply })
    }

    pub fn search(&self, request: SearchRequest) -> Result<SearchResult> {
        self.request(move |reply| WorkerCommand::Search { request, reply })
    }

    pub fn clear_search(&self) -> Result<()> {
        self.request(|reply| WorkerCommand::ClearSearch { reply })
    }

    pub fn shutdown(&mut self) -> Result<()> {
        self.begin_shutdown().map_or(Ok(()), PendingShutdown::wait)
    }

    pub(crate) fn begin_shutdown(&mut self) -> Option<PendingShutdown> {
        let thread = self.thread.take()?;
        let (reply, receive) = mpsc::channel();
        let reply = self
            .send(WorkerCommand::Shutdown { reply: Some(reply) })
            .map(|()| receive);
        Some(PendingShutdown { reply, thread })
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub(crate) fn join_after_shutdown(&mut self) -> Result<()> {
        self.thread.take().map_or(Ok(()), join)
    }

    fn request<T>(&self, build: impl FnOnce(Sender<Result<T>>) -> WorkerCommand) -> Result<T> {
        let (reply, result) = mpsc::channel();
        self.send(build(reply))?;
        result
            .recv()
            .context("Shell session worker did not respond")?
    }

    pub fn send(&self, command: WorkerCommand) -> Result<()> {
        self.commands
            .send(command)
            .map_err(|_| anyhow!("Shell session worker stopped"))?;
        self.wake.wake();
        Ok(())
    }
}

impl Drop for SessionWorker {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.send(WorkerCommand::Shutdown { reply: None });
        }
    }
}

fn join(thread: JoinHandle<Result<()>>) -> Result<()> {
    thread
        .join()
        .map_err(|_| anyhow!("Shell session worker panicked"))?
}

fn run(
    session: &mut Session,
    id: &str,
    run_id: &str,
    incoming: &Receiver<WorkerCommand>,
    events: &Sender<WorkerEvent>,
) -> Result<()> {
    let mut next = None;
    loop {
        loop {
            let command = match next.take().map_or_else(|| incoming.try_recv(), Ok) {
                Ok(command) => command,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return finish(session, id, run_id, events),
            };
            match command {
                WorkerCommand::Write { bytes, reply } => {
                    let _ = reply.send(session.write(&bytes));
                }
                WorkerCommand::Resize {
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                    reply,
                } => {
                    let _ = reply.send(session.resize(cols, rows, cell_width_px, cell_height_px));
                }
                WorkerCommand::Acknowledge { sequence, reply } => {
                    let _ = reply.send(session.acknowledge(sequence));
                }
                WorkerCommand::RetryStorage { reply } => {
                    let _ = reply.send(session.retry_storage());
                }
                WorkerCommand::SetEgressFrozen { frozen, reply } => {
                    let _ = reply.send(session.set_egress_frozen(frozen));
                }
                WorkerCommand::Screen { reply } => {
                    let _ = reply.send(session.screen());
                }
                WorkerCommand::View { reply } => {
                    let _ = reply.send(session.view());
                }
                WorkerCommand::SelectedView { reply } => {
                    let result = session.view().and_then(|(screen, modes)| {
                        Ok((screen, modes, session.selection_range()?))
                    });
                    let _ = reply.send(result);
                }
                WorkerCommand::Search { request, reply } => {
                    let _ = reply.send(session.search(request));
                }
                WorkerCommand::ClearSearch { reply } => {
                    session.clear_search();
                    let _ = reply.send(Ok(()));
                }
                WorkerCommand::Select { action, reply } => {
                    let _ = reply.send(session.selection_action(action));
                }
                WorkerCommand::Selection { reply } => {
                    let _ = reply.send(session.selection_range());
                }
                WorkerCommand::CopySelection {
                    reply,
                    interrupt_if_empty,
                } => {
                    let result = (|| {
                        let has_selection = session
                            .selection_range()?
                            .is_some_and(|range| range.anchor != range.end);
                        let text = session.selected_text()?;
                        let interrupted = !has_selection && interrupt_if_empty;
                        if interrupted {
                            session.write(&[3])?;
                        }
                        Ok(SelectionCopy { text, interrupted })
                    })();
                    let _ = reply.send(result);
                }
                WorkerCommand::Scroll { lines, reply } => {
                    let _ = reply.send(session.scroll(lines));
                }
                WorkerCommand::ScrollTo { row, reply } => {
                    let _ = reply.send(session.scroll_to(row));
                }
                WorkerCommand::ScrollSelection { lines, reply } => {
                    let _ = reply.send(session.scroll_selection(lines));
                }
                WorkerCommand::SetTheme { theme, reply } => {
                    let _ = reply.send(session.set_theme(theme));
                }
                WorkerCommand::Shutdown { reply } => {
                    let result = finish(session, id, run_id, events);
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                        return Ok(());
                    }
                    if let Err(error) = &result {
                        diagnostic(
                            events,
                            id,
                            run_id,
                            format!("shut down Shell session: {error:#}"),
                        );
                    }
                    return result;
                }
            }
        }

        let wait = if session.is_closed() {
            Duration::ZERO
        } else {
            MAX_POLL_WAIT
        };
        match session.poll(wait) {
            Ok(batch) => {
                if let Err(error) = publish(events, id, run_id, batch) {
                    let _ = finish(session, id, run_id, events);
                    return Err(error);
                }
            }
            Err(error) => {
                diagnostic(events, id, run_id, format!("poll Shell session: {error:#}"));
                if let Err(cleanup) = finish(session, id, run_id, events) {
                    diagnostic(
                        events,
                        id,
                        run_id,
                        format!("shut down Shell session: {cleanup:#}"),
                    );
                }
                return Err(error);
            }
        }
        if session.is_closed() {
            // Keep serving the final screen and pending replay after exit.
            // Wake for a held frame or pending search refresh after the last output.
            let command = match session.poll_deadline() {
                Some(deadline) => {
                    incoming.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                }
                None => incoming.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match command {
                Ok(command) => next = Some(command),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return finish(session, id, run_id, events);
                }
            }
        }
    }
}

fn finish(
    session: &mut Session,
    id: &str,
    run_id: &str,
    events: &Sender<WorkerEvent>,
) -> Result<()> {
    let stopped = session.shutdown();
    let final_events = session
        .poll(Duration::ZERO)
        .and_then(|batch| publish(events, id, run_id, batch));
    stopped.and(final_events)
}

fn publish(
    events: &Sender<WorkerEvent>,
    id: &str,
    run_id: &str,
    batch: Vec<SessionEvent>,
) -> Result<()> {
    for event in batch {
        events
            .send(WorkerEvent {
                session_id: id.to_owned(),
                run_id: run_id.to_owned(),
                event,
            })
            .context("Shell event receiver stopped")?;
    }
    Ok(())
}

fn diagnostic(events: &Sender<WorkerEvent>, id: &str, run_id: &str, message: String) {
    let _ = publish(
        events,
        id,
        run_id,
        vec![SessionEvent::Diagnostic { message }],
    );
}
