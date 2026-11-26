//! Thread-affine owner for one live Shell session.
use std::{
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use matou_native_terminal::{GridSelection, InputModes, Screen, SelectionAction, Theme};

#[cfg(windows)]
use crate::powershell_cwd::{PowerShellCwd, read_report};
use crate::{
    SessionEvent, SessionOptions,
    search::{SearchRequest, SearchResult},
    session::{ControlInputCheck, InputOrigin, PendingInput, Session, SessionWaker},
    shell_input::{InteractiveLaunch, ShellInput},
};

const MAX_POLL_WAIT: Duration = Duration::from_millis(250);
#[cfg(windows)]
const POWERSHELL_CWD_POLL_INTERVAL: Duration = Duration::from_millis(125);

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

pub struct ShellSubmission {
    pub(crate) launch: InteractiveLaunch,
    pub(crate) before: ShellInput,
    pub(crate) cleared: Receiver<Result<bool>>,
}

pub type CurrentScreen = (Screen, Option<PendingInput>);

/// Requests can be forwarded without blocking the central owner on a busy session.
pub enum WorkerCommand {
    Write {
        bytes: Vec<u8>,
        origin: InputOrigin,
        reply: Sender<Result<()>>,
    },
    WriteShellInput {
        bytes: Vec<u8>,
        origin: InputOrigin,
        submission: Option<Sender<Option<ShellSubmission>>>,
        reply: Sender<Result<()>>,
    },
    WriteControlInput {
        bytes: Vec<u8>,
        origin: InputOrigin,
        check: ControlInputCheck,
        reply: Sender<Result<u64>>,
    },
    CheckProcess {
        result: Arc<OnceLock<Result<()>>>,
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
    EndStorage,
    SetEgressFrozen {
        frozen: bool,
        reply: Sender<Result<()>>,
    },
    #[cfg(all(test, target_os = "macos"))]
    TestPause {
        paused: Sender<()>,
        resume: Receiver<()>,
    },
    #[cfg(test)]
    TestFailPromotionClear,
    Screen {
        reply: Sender<Result<Screen>>,
    },
    ReadCurrentScreen {
        reply: Sender<Result<CurrentScreen>>,
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
        pixel_target: bool,
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
    pub(crate) recovery_history_gap: Option<crate::history::HistoryGapCode>,
    shell_input: Arc<Mutex<ShellInput>>,
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
        let shell_input = Arc::new(Mutex::new(ShellInput::default()));
        let worker_input = Arc::clone(&shell_input);
        let thread = thread::Builder::new()
            .name("matou-shell-session".into())
            .spawn(move || {
                let session_id = options.session_id.clone();
                #[cfg(windows)]
                let (options, powershell_cwd) = {
                    let mut options = options;
                    let report =
                        PowerShellCwd::attach(&mut options.launch).unwrap_or_else(|error| {
                            eprintln!("could not enable PowerShell directory tracking: {error:#}");
                            None
                        });
                    (options, report)
                };
                match Session::launch(options) {
                    Ok(mut session) => {
                        if started
                            .send(Ok((
                                session.waker(),
                                session.process_id(),
                                session.recovery_history_gap(),
                            )))
                            .is_err()
                        {
                            return finish(&mut session, &session_id, &run_id, &events);
                        }
                        run(
                            &mut session,
                            &session_id,
                            &run_id,
                            &incoming,
                            &events,
                            &worker_input,
                            #[cfg(windows)]
                            powershell_cwd,
                        )
                    }
                    Err(error) => {
                        let _ = started.send(Err(error));
                        Ok(())
                    }
                }
            })
            .context("start Shell session worker")?;
        match startup.recv() {
            Ok(Ok((wake, process_id, recovery_history_gap))) => Ok(Self {
                commands,
                wake,
                process_id,
                recovery_history_gap,
                shell_input,
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
            origin: InputOrigin::System,
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

    pub(crate) fn restore_shell_input(&self, before: ShellInput) {
        *self.shell_input.lock().unwrap() = before;
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

fn write_shell_input(
    session: &mut Session,
    shared: &Mutex<ShellInput>,
    bytes: &[u8],
    origin: InputOrigin,
    submission: Option<Sender<Option<ShellSubmission>>>,
    write_promotion: impl FnOnce(&mut Session, &[u8]) -> Result<bool>,
) -> Result<()> {
    if let Some(submission) = submission {
        session.ensure_writable()?;
        let mut next = shared.lock().unwrap().clone();
        if let Some(launch) = next.observe(bytes) {
            let before = std::mem::replace(&mut *shared.lock().unwrap(), next);
            let (reply, cleared) = mpsc::channel();
            let _ = submission.send(Some(ShellSubmission {
                launch,
                before,
                cleared,
            }));
            // True means the line cleared. False means the original input was forwarded.
            let result = write_promotion(session, bytes);
            if matches!(result, Ok(true)) {
                *shared.lock().unwrap() = ShellInput::default();
            }
            let _ = reply.send(result);
            return Ok(());
        }
        let _ = submission.send(None);
        session.write_with_origin(bytes, origin)?;
        *shared.lock().unwrap() = next;
    } else {
        session.write_with_origin(bytes, origin)?;
        shared.lock().unwrap().observe(bytes);
    }
    Ok(())
}

fn clear_for_promotion(input: &[u8], mut write: impl FnMut(&[u8]) -> Result<()>) -> Result<bool> {
    match write(&[0x15]) {
        Ok(()) => Ok(true),
        Err(clear_error) => write(input).map(|()| false).with_context(|| {
            format!("could not forward Shell input after Ctrl+U failed: {clear_error:#}")
        }),
    }
}

fn run(
    session: &mut Session,
    id: &str,
    run_id: &str,
    incoming: &Receiver<WorkerCommand>,
    events: &Sender<WorkerEvent>,
    shell_input: &Mutex<ShellInput>,
    #[cfg(windows)] mut powershell_cwd: Option<PowerShellCwd>,
) -> Result<()> {
    let mut next = None;
    #[cfg(windows)]
    let mut cwd_poll_at = Instant::now();
    #[cfg(windows)]
    let mut last_reported_cwd = None;
    #[cfg(test)]
    let mut fail_promotion_clear = false;
    loop {
        loop {
            let command = match next.take().map_or_else(|| incoming.try_recv(), Ok) {
                Ok(command) => command,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return finish(session, id, run_id, events),
            };
            match command {
                WorkerCommand::Write {
                    bytes,
                    origin,
                    reply,
                } => {
                    let _ = reply.send(session.write_with_origin(&bytes, origin));
                }
                WorkerCommand::WriteShellInput {
                    bytes,
                    origin,
                    submission,
                    reply,
                } => {
                    let result = write_shell_input(
                        session,
                        shell_input,
                        &bytes,
                        origin,
                        submission,
                        |session, bytes| {
                            clear_for_promotion(bytes, |bytes| {
                                #[cfg(test)]
                                if bytes == [0x15] && std::mem::take(&mut fail_promotion_clear) {
                                    anyhow::bail!("injected Ctrl+U failure");
                                }
                                let source = if bytes == [0x15] {
                                    InputOrigin::System
                                } else {
                                    origin
                                };
                                session.write_with_origin(bytes, source)
                            })
                        },
                    );
                    let _ = reply.send(result);
                }
                WorkerCommand::WriteControlInput {
                    bytes,
                    origin,
                    check,
                    reply,
                } => {
                    let _ = reply.send(session.write_control_input(&bytes, origin, check));
                }
                WorkerCommand::CheckProcess { result } => {
                    let _ = result.set(session.check_process());
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
                WorkerCommand::EndStorage => session.request_storage_end(),
                WorkerCommand::SetEgressFrozen { frozen, reply } => {
                    let _ = reply.send(session.set_egress_frozen(frozen));
                }
                #[cfg(all(test, target_os = "macos"))]
                WorkerCommand::TestPause { paused, resume } => {
                    let _ = paused.send(());
                    let _ = resume.recv();
                }
                #[cfg(test)]
                WorkerCommand::TestFailPromotionClear => {
                    fail_promotion_clear = true;
                }
                WorkerCommand::Screen { reply } => {
                    let _ = reply.send(session.screen());
                }
                WorkerCommand::ReadCurrentScreen { reply } => {
                    let result = session
                        .read_current_screen()
                        .map(|screen| (screen, session.pending_input()));
                    let _ = reply.send(result);
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
                            shell_input.lock().unwrap().clear();
                        }
                        Ok(SelectionCopy { text, interrupted })
                    })();
                    let _ = reply.send(result);
                }
                WorkerCommand::Scroll { lines, reply } => {
                    let _ = reply.send(session.scroll(lines));
                }
                WorkerCommand::ScrollTo {
                    row,
                    pixel_target,
                    reply,
                } => {
                    let (mut row, mut reply) = (row, reply);
                    // Multiple pointer samples can arrive before this worker runs.
                    // A different target kind can change the resize anchor in between.
                    for _ in 0..127 {
                        match incoming.try_recv() {
                            Ok(WorkerCommand::ScrollTo {
                                row: newer_row,
                                pixel_target: newer_pixel_target,
                                reply: newer_reply,
                            }) if newer_pixel_target == pixel_target => {
                                let _ = reply.send(Ok(()));
                                (row, reply) = (newer_row, newer_reply);
                            }
                            Ok(other) => {
                                next = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    let _ = reply.send(session.scroll_to(row, pixel_target));
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
        #[cfg(windows)]
        let wait = if powershell_cwd.is_some() {
            wait.min(cwd_poll_at.saturating_duration_since(Instant::now()))
        } else {
            wait
        };
        match session.poll(wait) {
            Ok(batch) => {
                #[cfg(windows)]
                let mut batch = batch;
                #[cfg(windows)]
                if let Some(report) = &powershell_cwd
                    && Instant::now() >= cwd_poll_at
                {
                    cwd_poll_at = Instant::now() + POWERSHELL_CWD_POLL_INTERVAL;
                    if let Some(cwd) = read_report(report.path())
                        && std::path::Path::new(&cwd).is_absolute()
                        && last_reported_cwd.as_ref() != Some(&cwd)
                    {
                        last_reported_cwd = Some(cwd.clone());
                        batch.insert(
                            0,
                            SessionEvent::CwdCaptured {
                                run_id: run_id.to_owned(),
                                cwd,
                            },
                        );
                    }
                }
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
            #[cfg(windows)]
            drop(powershell_cwd.take());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_clear_writes_no_part_of_a_matching_submission() -> Result<()> {
        let mut attempts = Vec::new();
        assert!(clear_for_promotion(b"touch x\rclaude\rtail", |bytes| {
            attempts.push(bytes.to_vec());
            Ok(())
        })?);
        assert_eq!(attempts, [vec![0x15]]);
        Ok(())
    }

    #[test]
    fn failed_promotion_clear_attempts_the_whole_original_write() {
        let input = b"touch x\rclaude\rmore";
        for writable in [true, false] {
            let mut attempts = Vec::new();
            let result = clear_for_promotion(input, |bytes| {
                attempts.push(bytes.to_vec());
                match bytes {
                    [0x15] => Err(anyhow!("clear failed")),
                    _ if writable => Ok(()),
                    _ => Err(anyhow!("write failed")),
                }
            });
            assert_eq!(attempts, [vec![0x15], input.to_vec()]);
            if writable {
                assert!(!result.unwrap());
            } else {
                let error = format!("{:#}", result.unwrap_err());
                assert!(
                    error.contains("clear failed") && error.contains("write failed"),
                    "{error}"
                );
            }
        }
    }
}
