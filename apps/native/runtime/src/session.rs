use std::{
    cell::{Cell, RefCell},
    io::{self, Read},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use encoding_rs::UTF_8;
#[cfg(unix)]
use matou_native_core::pty::ForegroundProcessGroup;
use matou_native_core::{
    durability::{
        DEFAULT_MAX_RETAINED_BYTES, Event as GateEvent, ExecutionPauser, Gate, PendingFrame, State,
    },
    journal::{Frame, Record, SegmentJournal, visit_frames},
    pty::{LaunchProfile, ProcessExit, PtyProcess},
    shell_history::Collector,
};
use matou_native_terminal::{
    GridSelection, InputModes, Screen, SelectionAction, Terminal, TerminalOptions, Theme,
};

use crate::{
    SessionEvent, SessionOptions, StorageFaultCode,
    checkpoint_sync::{CapturedCheckpoint, CheckpointSync},
    flow::{
        BATCH_DELAY, CREDIT_HIGH_WATERMARK_BYTES, CREDIT_LOW_WATERMARK_BYTES, CreditWindow,
        MAX_CODE_UNITS, OutputBatcher, Utf8Decoder,
    },
    search::{SearchRequest, SearchResult, SessionSearch},
};

const READ_BYTES: usize = 8 * 1024;
const READ_QUEUE: usize = 128;
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(unix)]
#[derive(Debug)]
pub(crate) struct PtyShutdownIncomplete;

#[cfg(unix)]
impl std::fmt::Display for PtyShutdownIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("could not confirm that the Shell and its PTY foreground group exited")
    }
}

#[cfg(unix)]
impl std::error::Error for PtyShutdownIncomplete {}

enum ReaderEvent {
    Data(Vec<u8>),
    Eof,
    Error(io::Error),
    Wake,
}

/// A cross-thread wake for the session owner. It carries no terminal state.
#[derive(Clone)]
pub struct SessionWaker(mpsc::SyncSender<ReaderEvent>);

impl SessionWaker {
    pub fn wake(&self) {
        let _ = self.0.try_send(ReaderEvent::Wake);
    }
}

#[derive(Default)]
struct ReadControl {
    state: Mutex<ReadState>,
    changed: Condvar,
}

#[derive(Default)]
struct ReadState {
    paused: bool,
    stopped: bool,
}

impl ReadControl {
    fn wait_to_read(&self) -> bool {
        let state = self.state.lock().unwrap();
        let state = self
            .changed
            .wait_while(state, |state| state.paused && !state.stopped)
            .unwrap();
        !state.stopped
    }

    fn pause(&self, paused: bool) {
        self.state.lock().unwrap().paused = paused;
        if !paused {
            self.changed.notify_all();
        }
    }

    fn stop(&self) {
        self.state.lock().unwrap().stopped = true;
        self.changed.notify_all();
    }
}

fn start_reader(
    mut reader: Box<dyn Read + Send>,
    control: Arc<ReadControl>,
) -> Result<(mpsc::Receiver<ReaderEvent>, SessionWaker)> {
    let (send, receive) = mpsc::sync_channel(READ_QUEUE);
    let wake = SessionWaker(send.clone());
    thread::Builder::new()
        .name("matou-shell-read".into())
        .spawn(move || {
            let mut buffer = [0; READ_BYTES];
            while control.wait_to_read() {
                let event = match reader.read(&mut buffer) {
                    Ok(0) => ReaderEvent::Eof,
                    Ok(count) => ReaderEvent::Data(buffer[..count].to_vec()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => ReaderEvent::Error(error),
                };
                let finished = !matches!(event, ReaderEvent::Data(_));
                if send.send(event).is_err() || finished {
                    break;
                }
            }
        })
        .context("start Shell PTY reader")?;
    Ok((receive, wake))
}

#[derive(Default)]
struct PauseReasons {
    durability: bool,
    egress: bool,
}

struct Pauser<'a> {
    reasons: &'a mut PauseReasons,
    control: &'a ReadControl,
    process: Option<&'a PtyProcess>,
    diagnostics: &'a mut Vec<SessionEvent>,
}

impl Pauser<'_> {
    fn change(&mut self, durability: bool, frozen: bool) -> Result<()> {
        let was_paused = self.reasons.durability || self.reasons.egress;
        let (reason, other) = if durability {
            (&mut self.reasons.durability, self.reasons.egress)
        } else {
            (&mut self.reasons.egress, self.reasons.durability)
        };
        if *reason == frozen {
            return Ok(());
        }
        if !was_paused && frozen {
            self.control.pause(true);
            // Keep read admission stopped even if an OS process-control operation fails.
            *reason = frozen;
            if let Some(process) = self.process {
                process.pause_execution()?;
            }
        } else if was_paused && !other && !frozen {
            if let Some(process) = self.process {
                process.resume_execution()?;
            }
            self.control.pause(false);
        }
        *reason = frozen;
        Ok(())
    }
}

impl ExecutionPauser for Pauser<'_> {
    fn pause(&mut self) -> Result<()> {
        let result = self.change(true, true);
        if let Err(error) = &result {
            self.diagnostics
                .push(diagnostic(format!("pause Shell for storage: {error:#}")));
        }
        result
    }

    fn resume(&mut self) -> Result<()> {
        let result = self.change(true, false);
        if let Err(error) = &result {
            self.diagnostics
                .push(diagnostic(format!("resume Shell after storage: {error:#}")));
        }
        result
    }
}

struct Delivery {
    credit: CreditWindow,
    replay_from: Option<u64>,
}

impl Delivery {
    fn output(
        &mut self,
        events: &mut Vec<SessionEvent>,
        sequence: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        self.credit.record_sent(sequence, data.len())?;
        events.push(SessionEvent::Output { sequence, data });
        Ok(())
    }

    fn publish_output(
        &mut self,
        events: &mut Vec<SessionEvent>,
        sequence: u64,
        data: &[u8],
    ) -> Result<()> {
        if self.credit.is_paused() || self.replay_from.is_some() {
            self.replay_from.get_or_insert(sequence);
        } else {
            self.output(events, sequence, data.to_vec())?;
        }
        Ok(())
    }

    fn publish_exit(&mut self, events: &mut Vec<SessionEvent>, sequence: u64, status: ProcessExit) {
        if self.credit.is_paused() || self.replay_from.is_some() {
            self.replay_from.get_or_insert(sequence);
        } else {
            events.push(SessionEvent::Exited { sequence, status });
        }
    }
}

/// Owns the live terminal state on its caller's thread. Raw reads use one bounded worker.
pub struct Session {
    data_root: PathBuf,
    session_id: String,
    journal: SegmentJournal,
    checkpoint: Option<CheckpointSync>,
    screen_epoch: u64,
    gate: Gate,
    protocol_terminal: Terminal,
    display_terminal: Terminal,
    search: SessionSearch,
    visible_screen: Screen,
    geometry: (u16, u16, u32, u32),
    display_grid: (u16, u16),
    process: Option<PtyProcess>,
    pid: u32,
    reader: Option<mpsc::Receiver<ReaderEvent>>,
    waker: SessionWaker,
    read_control: Arc<ReadControl>,
    pauses: PauseReasons,
    child_exit: Option<ProcessExit>,
    child_exit_observed_at: Option<Instant>,
    reader_finished: bool,
    decoder: Utf8Decoder,
    batcher: OutputBatcher,
    collector: Option<Collector>,
    delivery: Delivery,
    events: Vec<SessionEvent>,
    exit_enqueued: bool,
    disposed: bool,
    closed: bool,
}

impl Session {
    pub fn launch(mut options: SessionOptions) -> Result<Self> {
        let checkpoint = CheckpointSync::for_session(&options);
        let mut journal = SegmentJournal::open(
            &options.launch.data_root,
            &options.session_id,
            std::mem::take(&mut options.journal),
        )?;
        let batcher = OutputBatcher::new(BATCH_DELAY, MAX_CODE_UNITS)?;
        let credit = CreditWindow::new(CREDIT_HIGH_WATERMARK_BYTES, CREDIT_LOW_WATERMARK_BYTES)?;
        let geometry = (
            options.terminal.cols,
            options.terminal.rows,
            options.terminal.cell_width_px,
            options.terminal.cell_height_px,
        );
        ensure!(
            (geometry.0, geometry.1) == (options.launch.cols, options.launch.rows),
            "initial Shell PTY and terminal geometry differ"
        );
        let mut recovery_diagnostic = None;
        let mut screen_epoch = 0;
        let [protocol_terminal, mut display_terminal] =
            if options.launch.profile != LaunchProfile::Shell && journal.last_sequence() > 0 {
                let mut recovered = recover_screen(&options, &journal)?;
                recovery_diagnostic = recovered.diagnostic;
                screen_epoch = recovered.screen_epoch;
                if recovered.resized {
                    journal
                        .append(&Record {
                            timestamp: unix_millis(),
                            frame: Frame::Resize {
                                sequence: journal.last_sequence() + 1,
                                cols: geometry.0.into(),
                                rows: geometry.1.into(),
                            },
                        })
                        .context("persist terminal startup resize")?;
                }
                if recovered.has_history {
                    let data = terminal_run_boundary(geometry.1);
                    feed_recovered(&mut recovered.terminals, &data);
                    journal
                        .append(&Record {
                            timestamp: unix_millis(),
                            frame: Frame::Output {
                                sequence: journal.last_sequence() + 1,
                                data,
                            },
                        })
                        .context("persist terminal run boundary")?;
                }
                if recovered.resized || recovered.has_history {
                    journal.flush().context("flush terminal startup frames")?;
                }
                recovered.terminals
            } else {
                let mut terminals = [
                    Terminal::new(options.terminal)?,
                    Terminal::new(options.terminal)?,
                ];
                // D01 display history is seeded without replaying its side effects.
                feed_recovered(&mut terminals, &options.restored_history);
                terminals
            };
        let visible_screen = display_terminal.screen()?;
        let sequence = journal.last_sequence();
        let gate = Gate::new(sequence, DEFAULT_MAX_RETAINED_BYTES)?;
        let (process, reader) = PtyProcess::spawn(&options.launch)?;
        let pid = process
            .process_id()
            .context("the Shell PTY did not report a process ID")?;
        let read_control = Arc::new(ReadControl::default());
        let (reader, waker) = start_reader(reader, read_control.clone())?;
        let mut events = vec![SessionEvent::Started { pid, sequence }];
        events.extend(recovery_diagnostic);
        Ok(Self {
            data_root: options.launch.data_root,
            session_id: options.session_id,
            journal,
            checkpoint,
            screen_epoch,
            gate,
            protocol_terminal,
            display_terminal,
            search: SessionSearch::default(),
            visible_screen,
            geometry,
            display_grid: (geometry.0, geometry.1),
            process: Some(process),
            pid,
            reader: Some(reader),
            waker,
            read_control,
            pauses: PauseReasons::default(),
            child_exit: None,
            child_exit_observed_at: None,
            reader_finished: false,
            decoder: Utf8Decoder::new(),
            batcher,
            collector: options.collect_history.then(Collector::new),
            delivery: Delivery {
                credit,
                replay_from: None,
            },
            events,
            exit_enqueued: false,
            disposed: false,
            closed: false,
        })
    }

    pub fn waker(&self) -> SessionWaker {
        self.waker.clone()
    }

    pub fn poll(&mut self, max_wait: Duration) -> Result<Vec<SessionEvent>> {
        self.advance(
            if self.events.is_empty() {
                max_wait
            } else {
                Duration::ZERO
            },
            true,
        )?;
        Ok(std::mem::take(&mut self.events))
    }

    pub(crate) fn ensure_writable(&self) -> Result<()> {
        ensure!(
            !self.disposed && !self.closed && self.child_exit.is_none(),
            "Shell session is closed"
        );
        ensure!(
            self.gate.state() == State::Healthy,
            "session storage is paused"
        );
        ensure!(!self.pauses.egress, "session output is frozen");
        self.process
            .as_ref()
            .context("Shell process not available")?;
        Ok(())
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.ensure_writable()?;
        let process = self
            .process
            .as_mut()
            .context("Shell process not available")?;
        self.display_terminal
            .selection_action(SelectionAction::Clear)?;
        if bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
            self.events
                .push(SessionEvent::InputSubmitted(Instant::now()));
        }
        let result = process.write(bytes);
        if let Err(error) = &result {
            self.events
                .push(diagnostic(format!("write to Shell PTY: {error:#}")));
        }
        result
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        ensure!(
            !self.disposed && !self.closed && self.child_exit.is_none(),
            "Shell session is closed"
        );
        let geometry = (cols, rows, cell_width_px, cell_height_px);
        if self.geometry == geometry {
            return Ok(());
        }
        ensure!(
            cols >= 2
                && rows > 0
                && cell_width_px > 0
                && cell_height_px > 0
                && cell_width_px.checked_mul(u32::from(cols)).is_some()
                && cell_height_px.checked_mul(u32::from(rows)).is_some(),
            "invalid Shell terminal geometry",
        );
        self.flush_output()?;
        let grid_changed = (self.geometry.0, self.geometry.1) != (cols, rows);
        let result = (|| {
            if grid_changed {
                self.process
                    .as_ref()
                    .context("Shell process not available")?
                    .resize(cols, rows)?;
            }
            self.protocol_terminal
                .resize(cols, rows, cell_width_px, cell_height_px)
        })();
        if let Err(error) = result {
            self.events
                .push(diagnostic(format!("resize Shell terminal: {error:#}")));
            return Err(error);
        }
        self.send_terminal_replies();
        if !grid_changed {
            let (display_cols, display_rows) = self.display_grid;
            self.display_terminal
                .resize(display_cols, display_rows, cell_width_px, cell_height_px)
                .inspect_err(|error| {
                    self.events
                        .push(diagnostic(format!("resize saved Shell view: {error:#}")));
                })?;
            self.display_terminal.drain_responses();
        }
        self.geometry = geometry;
        if grid_changed {
            self.admit(
                Frame::Resize {
                    sequence: self.last_sequence() + 1,
                    cols: cols.into(),
                    rows: rows.into(),
                },
                format!("{cols}:{rows}").len(),
            )?;
        }
        Ok(())
    }

    pub fn acknowledge(&mut self, through: u64) -> Result<()> {
        self.delivery.credit.acknowledge(through)?;
        let Some(from) = self
            .delivery
            .replay_from
            .filter(|_| !self.delivery.credit.is_paused())
        else {
            return Ok(());
        };
        let through = self.gate.last_persisted_sequence();
        let result = visit_frames(
            &self.data_root,
            &self.session_id,
            from,
            Some(through),
            |record| {
                let sequence = record.frame.sequence();
                match record.frame {
                    Frame::Output { data, .. } => {
                        self.delivery.output(&mut self.events, sequence, data)?
                    }
                    Frame::Exit {
                        exit_code, signal, ..
                    } => self.events.push(SessionEvent::Exited {
                        sequence,
                        status: ProcessExit { exit_code, signal },
                    }),
                    _ => {}
                }
                self.delivery.replay_from = (sequence < through).then_some(sequence + 1);
                Ok(!self.delivery.credit.is_paused())
            },
        );
        if let Err(error) = &result {
            self.events
                .push(diagnostic(format!("replay saved Shell output: {error:#}")));
        }
        result
    }

    pub fn retry_storage(&mut self) -> Result<()> {
        ensure!(!self.closed, "Shell session is closed");
        self.flush_output()?;
        self.drive_gate(true)
    }

    pub fn set_egress_frozen(&mut self, frozen: bool) -> Result<()> {
        if self.closed || (self.disposed && frozen) {
            return Ok(());
        }
        self.flush_output()?;
        self.change_egress(frozen)
    }

    pub fn dispose(&mut self) -> Result<()> {
        if self.closed || self.disposed {
            return Ok(());
        }
        self.flush_output()?;
        self.disposed = true;
        if self.gate.state() != State::Healthy {
            self.end_storage();
            return Ok(());
        }
        let thawed = self.change_egress(false);
        let terminated = self.process.as_mut().map(PtyProcess::terminate).transpose();
        if let Err(error) = &terminated {
            self.events
                .push(diagnostic(format!("terminate Shell: {error:#}")));
        }
        thawed?;
        terminated?;
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        #[cfg(unix)]
        let foreground = self
            .process
            .as_ref()
            .and_then(PtyProcess::foreground_process_group);
        let disposal = self.dispose();
        if !self.disposed {
            return disposal;
        }
        #[cfg(unix)]
        if let Some(group) = &foreground
            && let Err(error) = group.hang_up()
        {
            self.events.push(diagnostic(format!(
                "hang up Shell foreground group: {error}"
            )));
        }
        // A termination request can partly succeed (the PTY child received
        // SIGHUP, but the process-group signal failed). Drain its real exit
        // even in that case so the owner can finish and retire the run.
        let complete = self.await_shutdown(
            Duration::from_millis(1_500),
            #[cfg(unix)]
            foreground.as_ref(),
        )?;
        if !complete {
            self.force_kill();
            #[cfg(unix)]
            if let Some(group) = foreground.as_ref()
                && let Err(error) = group.force_kill()
            {
                self.events.push(diagnostic(format!(
                    "force termination of Shell foreground group: {error}"
                )));
            }
            #[cfg_attr(windows, allow(unused_variables))]
            let complete = self.await_shutdown(
                Duration::from_millis(1_000),
                #[cfg(unix)]
                foreground.as_ref(),
            )?;
            #[cfg(unix)]
            if !complete
                && foreground
                    .as_ref()
                    .is_some_and(|group| group.is_running().unwrap_or(true))
            {
                self.end_storage();
                return Err(PtyShutdownIncomplete.into());
            }
        }
        if !self.closed {
            // Drain the already queued chunks before giving up on the backend closing.
            self.advance(Duration::ZERO, false)?;
            self.finish_decoder()?;
            if !self.closed {
                self.events.push(diagnostic(
                    "Shell did not close before the shutdown deadline",
                ));
                self.end_storage();
            }
        }
        #[cfg(unix)]
        ensure!(self.child_exit.is_some(), PtyShutdownIncomplete);
        disposal
    }

    pub fn screen(&mut self) -> Result<Screen> {
        self.expire_sync();
        if !self.display_terminal.render_held() {
            self.visible_screen = self.display_terminal.screen()?;
        }
        Ok(self.visible_screen.clone())
    }

    pub(crate) fn read_current_screen(&mut self) -> Result<Screen> {
        // The protocol buffer follows the live bottom independently of the UI
        // viewport. Capturing it must not expire sync or send PTY replies.
        self.protocol_terminal.screen()
    }

    pub fn view(&mut self) -> Result<(Screen, InputModes)> {
        Ok((self.screen()?, self.display_terminal.input_modes()?))
    }

    pub fn search(&mut self, request: SearchRequest) -> Result<SearchResult> {
        self.search.search(&mut self.display_terminal, request)
    }

    pub fn clear_search(&mut self) {
        self.search.clear();
    }

    pub fn selection_action(&mut self, action: SelectionAction) -> Result<()> {
        self.display_terminal.selection_action(action)
    }

    pub fn selection_range(&mut self) -> Result<Option<GridSelection>> {
        self.display_terminal.selection_range()
    }

    pub fn selected_text(&mut self) -> Result<String> {
        self.display_terminal.selected_text()
    }

    pub fn scroll(&mut self, lines: i32) -> Result<()> {
        self.display_terminal.scroll(lines)
    }

    pub fn scroll_to(&mut self, row: u64, pixel_target: bool) -> Result<()> {
        self.display_terminal.scroll_to(row, pixel_target)
    }

    pub fn scroll_selection(&mut self, lines: i32) -> Result<()> {
        self.display_terminal
            .selection_action(SelectionAction::Scroll { lines })
    }

    pub fn set_theme(&mut self, theme: Theme) -> Result<()> {
        self.protocol_terminal.set_theme(theme)?;
        self.display_terminal.set_theme(theme)
    }

    pub fn screen_deadline(&self) -> Option<Instant> {
        [
            self.protocol_terminal.sync_deadline(),
            self.display_terminal.sync_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub(crate) fn poll_deadline(&self) -> Option<Instant> {
        [
            self.screen_deadline(),
            self.search.deadline(),
            self.checkpoint.as_ref().and_then(CheckpointSync::deadline),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub(crate) fn check_process(&mut self) -> Result<()> {
        self.observe_exit()?;
        if let Some(exit) = self.child_exit {
            anyhow::bail!("process exited with code {}", exit.exit_code);
        }
        ensure!(
            !self.closed && self.process.is_some(),
            "PTY process is unavailable"
        );
        Ok(())
    }

    pub fn durability_state(&self) -> State {
        self.gate.state()
    }
    pub fn last_sequence(&self) -> u64 {
        self.gate.last_accepted_sequence()
    }
    pub fn process_id(&self) -> u32 {
        self.pid
    }
    pub fn unacked_bytes(&self) -> usize {
        self.delivery.credit.unacked_bytes()
    }

    fn await_shutdown(
        &mut self,
        max_wait: Duration,
        #[cfg(unix)] foreground: Option<&ForegroundProcessGroup>,
    ) -> Result<bool> {
        let deadline = Instant::now() + max_wait;
        loop {
            #[cfg(unix)]
            let group_running = foreground
                .map(ForegroundProcessGroup::is_running)
                .transpose()
                .context(PtyShutdownIncomplete)?
                .unwrap_or(false);
            #[cfg(windows)]
            let group_running = false;
            if self.closed && !group_running {
                return Ok(true);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let poll = remaining.min(EXIT_POLL_INTERVAL);
            let next = Instant::now() + poll;
            self.advance(poll, false)?;
            if !self.closed && self.gate.state() != State::Healthy {
                self.end_storage();
            }
            if group_running && (self.closed || self.reader_finished) {
                thread::sleep(next.saturating_duration_since(Instant::now()));
            }
        }
    }

    fn advance(&mut self, max_wait: Duration, wake_on_events: bool) -> Result<()> {
        self.expire_sync();
        self.refresh_search();
        if self.closed {
            return Ok(());
        }
        self.flush_due()?;
        self.observe_exit()?;
        self.enqueue_exit_if_ready()?;
        if self.closed {
            return Ok(());
        }

        let now = Instant::now();
        let wait = [self.batcher.deadline(), self.poll_deadline()]
            .into_iter()
            .flatten()
            .map(|deadline| deadline.saturating_duration_since(now))
            .fold(max_wait, Duration::min);
        let wait = if wake_on_events && !self.events.is_empty() {
            Duration::ZERO
        } else if self.reader_finished && self.child_exit.is_none() {
            wait.min(EXIT_POLL_INTERVAL)
        } else {
            wait
        };

        for index in 0..READ_QUEUE {
            let Some(receiver) = self.reader.as_ref() else {
                break;
            };
            let event = if index == 0 && !wait.is_zero() {
                receiver.recv_timeout(wait).ok()
            } else {
                receiver.try_recv().ok()
            };
            let Some(event) = event else {
                break;
            };
            match event {
                ReaderEvent::Wake => break,
                ReaderEvent::Data(data) => {
                    let decoded = self.decoder.feed(&data);
                    self.offer_output(&decoded)?;
                }
                ReaderEvent::Eof => {
                    self.reader_finished = true;
                    self.finish_decoder()?;
                    break;
                }
                ReaderEvent::Error(error) => {
                    self.events
                        .push(diagnostic(format!("read Shell PTY: {error}")));
                    self.reader_finished = true;
                    self.finish_decoder()?;
                    self.dispose()?;
                    break;
                }
            }
        }
        self.expire_sync();
        self.flush_due()?;
        self.observe_exit()?;
        self.enqueue_exit_if_ready()?;
        self.refresh_search();
        self.sync_checkpoint(false);
        Ok(())
    }

    fn capture_checkpoint_snapshot(&mut self) -> Result<Option<CapturedCheckpoint>> {
        let Some(checkpoint) = &self.checkpoint else {
            return Ok(None);
        };
        let sequence = self.journal.last_sequence();
        let (cols, rows) = self.display_grid;
        if sequence <= checkpoint.last_checkpoint_sequence
            || self.gate.state() != State::Healthy
            || cols < 2
            || rows == 0
            || !self.display_terminal.can_format_vt_snapshot()?
        {
            return Ok(None);
        }
        Ok(Some(CapturedCheckpoint {
            terminal_sequence: sequence,
            screen_epoch: self.screen_epoch,
            cols,
            rows,
            snapshot: self.display_terminal.format_vt_snapshot()?,
        }))
    }

    fn sync_checkpoint(&mut self, closing: bool) {
        if self.closed {
            return;
        }
        let now = Instant::now();
        let Some(deadline) = self.checkpoint.as_ref().and_then(CheckpointSync::deadline) else {
            return;
        };
        if !closing && now < deadline {
            return;
        }
        self.checkpoint.as_mut().unwrap().attempted(now);
        let result = self.capture_checkpoint_snapshot().and_then(|captured| {
            if let Some(captured) = captured {
                self.checkpoint.as_mut().unwrap().persist(
                    &self.data_root,
                    &self.session_id,
                    &mut self.journal,
                    captured,
                )?;
            }
            Ok(())
        });
        if let Err(error) = result {
            self.events
                .push(diagnostic(format!("write terminal checkpoint: {error:#}")));
        }
    }

    fn refresh_search(&mut self) {
        if let Some(result) = self.search.refresh_due(&mut self.display_terminal) {
            match result {
                Ok(result) => self.events.push(SessionEvent::SearchUpdated { result }),
                Err(error) => self
                    .events
                    .push(diagnostic(format!("refresh Shell search: {error:#}"))),
            }
        }
    }

    fn flush_due(&mut self) -> Result<()> {
        if let Some(data) = self.batcher.take_due(Instant::now()) {
            self.enqueue_output(data)?;
        }
        Ok(())
    }

    fn offer_output(&mut self, data: &str) -> Result<()> {
        if let Some(batch) = self.batcher.offer(data, Instant::now()) {
            self.enqueue_output(batch)?;
        }
        Ok(())
    }

    fn flush_output(&mut self) -> Result<()> {
        if let Some(data) = self.batcher.flush() {
            self.enqueue_output(data)?;
        }
        Ok(())
    }

    fn finish_decoder(&mut self) -> Result<()> {
        let decoded = self.decoder.finish();
        self.offer_output(&decoded)?;
        self.flush_output()
    }

    fn enqueue_output(&mut self, data: String) -> Result<()> {
        let bytes = data.into_bytes();
        self.protocol_terminal.feed(&bytes);
        self.protocol_terminal.drain_clipboard();
        self.send_terminal_replies();
        let len = bytes.len();
        self.admit(
            Frame::Output {
                sequence: self.last_sequence() + 1,
                data: bytes,
            },
            len,
        )
    }

    fn expire_sync(&mut self) {
        if let Err(error) = self.protocol_terminal.expire_sync() {
            self.events.push(diagnostic(format!(
                "end Shell protocol synchronized output: {error:#}"
            )));
        }
        self.send_terminal_replies();
        let held = self.display_terminal.render_held();
        if let Err(error) = self.display_terminal.expire_sync() {
            self.events.push(diagnostic(format!(
                "end Shell display synchronized output: {error:#}"
            )));
        } else if held && !self.display_terminal.render_held() {
            self.events.push(SessionEvent::ScreenInvalidated);
        }
        self.display_terminal.drain_responses();
    }

    fn send_terminal_replies(&mut self) {
        let replies = self.protocol_terminal.drain_responses();
        if !replies.is_empty()
            && self.child_exit.is_none()
            && let Some(process) = self.process.as_mut()
            && let Err(error) = process.write(&replies)
        {
            self.events
                .push(diagnostic(format!("reply to Shell PTY: {error:#}")));
        }
    }

    fn observe_exit(&mut self) -> Result<()> {
        if self.child_exit.is_none()
            && let Some(process) = self.process.as_mut()
        {
            self.child_exit = process.try_wait().inspect_err(|error| {
                self.events
                    .push(diagnostic(format!("poll Shell process: {error:#}")));
            })?;
            if self.child_exit.is_some() {
                self.child_exit_observed_at = Some(Instant::now());
                #[cfg(windows)]
                {
                    // ConPTY keeps the output pipe open after its child exits.
                    // Close it while the reader drains the last output and emits EOF.
                    let process = self.process.take();
                    thread::Builder::new()
                        .name("matou-conpty-close".into())
                        .spawn(move || drop(process))
                        .context("close exited Shell pseudoconsole")?;
                }
            }
        }
        Ok(())
    }

    fn enqueue_exit_if_ready(&mut self) -> Result<()> {
        if self.closed || self.exit_enqueued || !self.reader_finished {
            return Ok(());
        }
        if let Some(ProcessExit { exit_code, signal }) = self.child_exit {
            let retained = format!(
                "{exit_code}:{}",
                signal.map_or_else(String::new, |signal| signal.to_string())
            )
            .len();
            self.exit_enqueued = true;
            self.admit(
                Frame::Exit {
                    sequence: self.last_sequence() + 1,
                    exit_code,
                    signal,
                },
                retained,
            )?;
            if self.disposed && self.gate.state() != State::Healthy {
                self.end_storage();
            }
        }
        Ok(())
    }

    fn admit(&mut self, frame: Frame, retained_bytes: usize) -> Result<()> {
        let mut diagnostics = Vec::new();
        let mut pauser = Pauser {
            reasons: &mut self.pauses,
            control: &self.read_control,
            process: self.process.as_ref(),
            diagnostics: &mut diagnostics,
        };
        let admitted = self.gate.append(
            PendingFrame {
                record: Record {
                    timestamp: unix_millis(),
                    frame,
                },
                retained_bytes,
            },
            &mut pauser,
        );
        self.events.extend(diagnostics);
        if let Err(error) = admitted {
            self.events
                .push(diagnostic(format!("admit Shell frame: {error:#}")));
            self.end_storage();
            return Err(error);
        }
        self.drive_gate(false)
    }

    fn drive_gate(&mut self, retry: bool) -> Result<()> {
        let mut diagnostics = Vec::new();
        let mut exit = None;
        let result = {
            let mut pauser = Pauser {
                reasons: &mut self.pauses,
                control: &self.read_control,
                process: self.process.as_ref(),
                diagnostics: &mut diagnostics,
            };
            let delivery = &mut self.delivery;
            let collector = &mut self.collector;
            let display_terminal = &mut self.display_terminal;
            let search = &mut self.search;
            let display_grid = &mut self.display_grid;
            let checkpoint = &mut self.checkpoint;
            let (_, _, cell_width_px, cell_height_px) = self.geometry;
            let events = RefCell::new(&mut self.events);
            let fault_code = Cell::new(StorageFaultCode::WriteFailed);
            let mut persist = |record: &Record| {
                self.journal.append(record).inspect_err(|error| {
                    fault_code.set(storage_fault_code(error));
                })
            };
            let mut after = |record: &Record| -> Result<()> {
                let mut events = events.borrow_mut();
                match &record.frame {
                    Frame::Output { sequence, data } => {
                        let decoded = std::str::from_utf8(data)?;
                        display_terminal.feed(data);
                        if let Some(checkpoint) = checkpoint {
                            checkpoint.record_output(data.len());
                        }
                        search.buffer_changed();
                        display_terminal.drain_responses();
                        let clipboard = display_terminal.drain_clipboard();
                        events.push(SessionEvent::SavedOutput {
                            sequence: *sequence,
                            data: decoded.to_owned(),
                            completed_blocks: collector
                                .as_mut()
                                .map(|collector| collector.ingest(decoded, record.timestamp))
                                .unwrap_or_default(),
                        });
                        delivery.publish_output(&mut events, *sequence, data)?;
                        events.extend(
                            clipboard
                                .into_iter()
                                .map(|text| SessionEvent::ClipboardWrite { text }),
                        );
                    }
                    Frame::Resize {
                        sequence,
                        cols,
                        rows,
                    } => {
                        let cols = (*cols).try_into()?;
                        let rows = (*rows).try_into()?;
                        events.push(SessionEvent::Resized {
                            sequence: *sequence,
                            cols,
                            rows,
                        });
                        display_terminal.resize(cols, rows, cell_width_px, cell_height_px)?;
                        display_terminal.drain_responses();
                        *display_grid = (cols, rows);
                        search.buffer_changed();
                    }
                    Frame::Exit {
                        sequence,
                        exit_code,
                        signal,
                    } => {
                        exit = Some((
                            *sequence,
                            ProcessExit {
                                exit_code: *exit_code,
                                signal: *signal,
                            },
                        ));
                    }
                    _ => {}
                }
                Ok(())
            };
            let mut emit = |event| {
                events.borrow_mut().push(match event {
                    GateEvent::Fault {
                        failed_sequence,
                        retained_bytes,
                        error,
                    } => SessionEvent::StorageFault {
                        sequence: failed_sequence,
                        retained_bytes,
                        code: fault_code.get(),
                        message: error,
                    },
                    GateEvent::Recovered { through_sequence } => SessionEvent::StorageRecovered {
                        sequence: through_sequence,
                    },
                    GateEvent::SideEffectError { sequence, error } => {
                        diagnostic(format!("apply saved Shell frame {sequence}: {error}"))
                    }
                })
            };
            if retry {
                self.gate
                    .retry(&mut pauser, &mut persist, &mut after, &mut emit)
            } else {
                self.gate
                    .drain(&mut pauser, &mut persist, &mut after, &mut emit)
            }
        };
        self.events.extend(diagnostics);
        if let Some((sequence, status)) = exit {
            let disposed = self.disposed;
            self.sync_checkpoint(true);
            self.close_resources();
            self.events.push(SessionEvent::SavedExit {
                sequence,
                status,
                disposed,
                observed_at: self.child_exit_observed_at.expect("observed process exit"),
            });
            self.delivery
                .publish_exit(&mut self.events, sequence, status);
        }
        result
    }

    fn change_egress(&mut self, frozen: bool) -> Result<()> {
        let mut pauser = Pauser {
            reasons: &mut self.pauses,
            control: &self.read_control,
            process: self.process.as_ref(),
            diagnostics: &mut self.events,
        };
        pauser.change(false, frozen).inspect_err(|error| {
            self.events
                .push(diagnostic(format!("change Shell output pause: {error:#}")));
        })
    }

    fn force_kill(&mut self) {
        if let Some(process) = self.process.as_mut()
            && let Err(error) = process.force_kill()
        {
            self.events
                .push(diagnostic(format!("force termination of Shell: {error:#}")));
        }
    }

    fn end_storage(&mut self) {
        self.disposed = true;
        let _ = self.change_egress(false);
        let mut pauser = Pauser {
            reasons: &mut self.pauses,
            control: &self.read_control,
            process: self.process.as_ref(),
            diagnostics: &mut self.events,
        };
        let _ = self.gate.end(&mut pauser);
        self.force_kill();
        self.close_resources();
    }

    fn close_resources(&mut self) {
        self.closed = true;
        self.disposed = true;
        self.read_control.stop();
        self.reader.take();
        self.process.take();
        if let Err(error) = self.journal.close() {
            self.events
                .push(diagnostic(format!("close Shell journal: {error:#}")));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.read_control.stop();
        self.reader.take();
        // PtyProcess owns bounded termination and deferred reaping.
        self.process.take();
    }
}

struct RecoveredScreen {
    terminals: [Terminal; 2],
    screen_epoch: u64,
    has_history: bool,
    resized: bool,
    diagnostic: Option<SessionEvent>,
}

fn recover_screen(options: &SessionOptions, journal: &SegmentJournal) -> Result<RecoveredScreen> {
    let through = journal.last_sequence();
    let domain_watermark = journal.domain_event_sequence_at_or_before(through);
    let target = options.terminal;
    let recovery = options.screen_recovery.as_ref();
    let checkpoint = recovery.and_then(|recovery| {
        recovery.checkpoints.iter().find_map(|candidate| {
            if candidate.session_id != options.session_id
                || candidate.terminal_sequence > through
                || candidate.domain_event_sequence > domain_watermark
            {
                return None;
            }
            let checkpoint = candidate.read().ok()?;
            Some((
                candidate.terminal_sequence,
                candidate.screen_epoch,
                checkpoint,
            ))
        })
    });
    // A readable checkpoint without a grid requires replay from the beginning.
    let checkpoint = checkpoint.and_then(|(sequence, epoch, checkpoint)| {
        Some((sequence, epoch, checkpoint.grid?, checkpoint))
    });
    let mut grid = checkpoint
        .as_ref()
        .map(|(_, _, grid, _)| *grid)
        .or_else(|| recovery.and_then(|recovery| recovery.initial_grid))
        .unwrap_or((target.cols, target.rows));
    let terminals_at = |(cols, rows)| {
        let options = TerminalOptions {
            cols,
            rows,
            ..target
        };
        Ok::<_, anyhow::Error>([Terminal::new(options)?, Terminal::new(options)?])
    };
    let mut terminals = terminals_at(grid)?;
    let mut has_history = false;
    let mut from = 1;
    let mut screen_epoch = 0;
    if let Some((sequence, epoch, _, checkpoint)) = checkpoint {
        from = sequence + 1;
        screen_epoch = epoch;
        let snapshot = checkpoint.snapshot();
        has_history = !snapshot.is_empty();
        feed_recovered(
            &mut terminals,
            UTF_8.decode_with_bom_removal(snapshot).0.as_bytes(),
        );
    }

    // Like the reference's streaming TextDecoder, retain a split UTF-8
    // character across output frames and resizes, and discard it on reset.
    let mut decoder = UTF_8.new_decoder_with_bom_removal();
    let mut decoded = String::new();
    let mut feed_output =
        |terminals: &mut [Terminal; 2], decoder: &mut encoding_rs::Decoder, bytes: &[u8], last| {
            decoded.clear();
            decoded.reserve(
                decoder
                    .max_utf8_buffer_length(bytes.len())
                    .context("recovery output too large")?,
            );
            let (status, read, _) = decoder.decode_to_string(bytes, &mut decoded, last);
            debug_assert_eq!(status, encoding_rs::CoderResult::InputEmpty);
            debug_assert_eq!(read, bytes.len());
            feed_recovered(terminals, decoded.as_bytes());
            Ok::<_, anyhow::Error>(())
        };
    let mut replay_diagnostic = None;
    if from <= through {
        let replay = visit_frames(
            &options.launch.data_root,
            &options.session_id,
            from,
            Some(through),
            |record| {
                match record.frame {
                    Frame::Output { data, .. } => {
                        has_history |= !data.is_empty();
                        feed_output(&mut terminals, &mut decoder, &data, false)?;
                    }
                    Frame::Resize { cols, rows, .. } => {
                        let next = (cols.try_into()?, rows.try_into()?);
                        resize_recovered(&mut terminals, next, target)?;
                        grid = next;
                    }
                    Frame::Reset {
                        screen_epoch: epoch,
                        ..
                    } => {
                        terminals = terminals_at(grid)?;
                        screen_epoch = epoch;
                        decoder = UTF_8.new_decoder_with_bom_removal();
                        has_history = false;
                    }
                    _ => {}
                }
                Ok(true)
            },
        );
        if let Err(error) = replay {
            replay_diagnostic = Some(diagnostic(format!("recover terminal screen: {error:#}")));
        }
        feed_output(&mut terminals, &mut decoder, &[], true)?;
    }

    let launch_grid = (target.cols, target.rows);
    let resized = grid != launch_grid;
    if resized {
        resize_recovered(&mut terminals, launch_grid, target)?;
    }
    Ok(RecoveredScreen {
        terminals,
        screen_epoch,
        has_history,
        resized,
        diagnostic: replay_diagnostic,
    })
}

fn feed_recovered(terminals: &mut [Terminal; 2], data: &[u8]) {
    for terminal in terminals {
        terminal.feed(data);
        terminal.drain_responses();
        terminal.drain_clipboard();
    }
}

fn resize_recovered(
    terminals: &mut [Terminal; 2],
    (cols, rows): (u16, u16),
    options: TerminalOptions,
) -> Result<()> {
    for terminal in terminals {
        terminal.resize(cols, rows, options.cell_width_px, options.cell_height_px)?;
        terminal.drain_responses();
        terminal.drain_clipboard();
    }
    Ok(())
}

fn terminal_run_boundary(rows: u16) -> Vec<u8> {
    let mut boundary = concat!(
        "\x18\x1b[?1049l\x1b[!p",
        "\x1b[?9;1000;1002;1003;1004;1005;1006;1015;1007;2004;2026l\x1b[?25h",
        "\x1b[?6l\x1b[r\x1b[0m\x1b[9999;1H",
    )
    .as_bytes()
    .to_vec();
    for _ in 0..rows {
        boundary.extend_from_slice(b"\r\n");
    }
    boundary.extend_from_slice(b"\x1b[H");
    boundary
}

fn diagnostic(message: impl Into<String>) -> SessionEvent {
    SessionEvent::Diagnostic {
        message: message.into(),
    }
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn storage_fault_code(error: &anyhow::Error) -> StorageFaultCode {
    match error
        .chain()
        .find_map(|cause| cause.downcast_ref::<io::Error>())
        .map(io::Error::kind)
    {
        Some(io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded) => {
            StorageFaultCode::QuotaExceeded
        }
        Some(io::ErrorKind::ReadOnlyFilesystem | io::ErrorKind::PermissionDenied) => {
            StorageFaultCode::ReadOnly
        }
        _ => StorageFaultCode::WriteFailed,
    }
}
