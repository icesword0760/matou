use std::{
    cell::{Cell, RefCell},
    io::{self, Read},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::{
    durability::{
        DEFAULT_MAX_RETAINED_BYTES, Event as GateEvent, ExecutionPauser, Gate, PendingFrame, State,
    },
    journal::{Frame, Record, SegmentJournal, visit_frames},
    pty::{ProcessExit, PtyProcess},
    shell_history::Collector,
};
use matou_native_terminal::{Screen, Terminal};

use crate::{
    SessionEvent, SessionOptions, StorageFaultCode,
    flow::{
        BATCH_DELAY, CREDIT_HIGH_WATERMARK_BYTES, CREDIT_LOW_WATERMARK_BYTES, CreditWindow,
        MAX_CODE_UNITS, OutputBatcher, Utf8Decoder,
    },
};

const READ_BYTES: usize = 8 * 1024;
const READ_QUEUE: usize = 128;
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    gate: Gate,
    terminal: Terminal,
    visible_screen: Screen,
    geometry: (u16, u16, u32, u32),
    process: Option<PtyProcess>,
    pid: u32,
    reader: Option<mpsc::Receiver<ReaderEvent>>,
    waker: SessionWaker,
    read_control: Arc<ReadControl>,
    pauses: PauseReasons,
    child_exit: Option<ProcessExit>,
    reader_finished: bool,
    decoder: Utf8Decoder,
    batcher: OutputBatcher,
    collector: Collector,
    delivery: Delivery,
    events: Vec<SessionEvent>,
    exit_enqueued: bool,
    disposed: bool,
    closed: bool,
}

impl Session {
    pub fn launch(options: SessionOptions) -> Result<Self> {
        let journal = SegmentJournal::open(
            &options.launch.data_root,
            &options.session_id,
            options.journal,
        )?;
        let sequence = journal.last_sequence();
        let gate = Gate::new(sequence, DEFAULT_MAX_RETAINED_BYTES)?;
        let batcher = OutputBatcher::new(BATCH_DELAY, MAX_CODE_UNITS)?;
        let credit = CreditWindow::new(CREDIT_HIGH_WATERMARK_BYTES, CREDIT_LOW_WATERMARK_BYTES)?;
        let mut terminal = Terminal::new(options.terminal)?;
        // Restored completed history belongs to this screen only. In particular,
        // any terminal replies from replay must never reach the new Shell.
        terminal.feed(&options.restored_history);
        terminal.drain_responses();
        let visible_screen = terminal.screen()?;
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
        let (process, reader) = PtyProcess::spawn(&options.launch)?;
        let pid = process
            .process_id()
            .context("the Shell PTY did not report a process ID")?;
        let read_control = Arc::new(ReadControl::default());
        let (reader, waker) = start_reader(reader, read_control.clone())?;
        Ok(Self {
            data_root: options.launch.data_root,
            session_id: options.session_id,
            journal,
            gate,
            terminal,
            visible_screen,
            geometry,
            process: Some(process),
            pid,
            reader: Some(reader),
            waker,
            read_control,
            pauses: PauseReasons::default(),
            child_exit: None,
            reader_finished: false,
            decoder: Utf8Decoder::new(),
            batcher,
            collector: Collector::new(),
            delivery: Delivery {
                credit,
                replay_from: None,
            },
            events: vec![SessionEvent::Started { pid, sequence }],
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

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            !self.disposed && !self.closed && self.child_exit.is_none(),
            "Shell session is closed"
        );
        ensure!(
            self.gate.state() == State::Healthy,
            "session storage is paused"
        );
        ensure!(!self.pauses.egress, "session output is frozen");
        let result = self
            .process
            .as_mut()
            .context("Shell process not available")?
            .write(bytes);
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
            self.terminal
                .resize(cols, rows, cell_width_px, cell_height_px)
        })();
        if let Err(error) = result {
            self.events
                .push(diagnostic(format!("resize Shell terminal: {error:#}")));
            return Err(error);
        }
        self.geometry = geometry;
        self.send_terminal_replies();
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
        self.dispose()?;
        self.await_shutdown(Duration::from_millis(1_500))?;
        if !self.closed {
            self.force_kill();
            self.await_shutdown(Duration::from_millis(1_000))?;
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
        Ok(())
    }

    pub fn screen(&mut self) -> Result<Screen> {
        self.expire_sync();
        // The live parser can already have rendered a frame whose journal write
        // failed. Keep exposing the last visible screen until storage recovers.
        if self.gate.state() == State::Healthy && !self.terminal.render_held() {
            self.visible_screen = self.terminal.screen()?;
        }
        Ok(self.visible_screen.clone())
    }

    pub fn is_closed(&self) -> bool {
        self.closed
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

    fn await_shutdown(&mut self, max_wait: Duration) -> Result<()> {
        let deadline = Instant::now() + max_wait;
        while !self.closed {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            self.advance(remaining.min(EXIT_POLL_INTERVAL), false)?;
            if !self.closed && self.gate.state() != State::Healthy {
                self.end_storage();
            }
        }
        Ok(())
    }

    fn advance(&mut self, max_wait: Duration, wake_on_events: bool) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.expire_sync();
        self.flush_due()?;
        self.observe_exit()?;
        self.enqueue_exit_if_ready()?;
        if self.closed {
            return Ok(());
        }

        let now = Instant::now();
        let wait = [self.batcher.deadline(), self.terminal.sync_deadline()]
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
        self.enqueue_exit_if_ready()
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
        self.terminal.feed(&bytes);
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
        if let Err(error) = self.terminal.expire_sync() {
            self.events.push(diagnostic(format!(
                "end Shell synchronized output: {error:#}"
            )));
        }
        self.send_terminal_replies();
    }

    fn send_terminal_replies(&mut self) {
        let replies = self.terminal.drain_responses();
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
                        events.extend(
                            collector
                                .ingest(decoded, record.timestamp)
                                .into_iter()
                                .map(SessionEvent::CompletedBlock),
                        );
                        delivery.publish_output(&mut events, *sequence, data)?;
                    }
                    Frame::Resize {
                        sequence,
                        cols,
                        rows,
                    } => events.push(SessionEvent::Resized {
                        sequence: *sequence,
                        cols: (*cols).try_into()?,
                        rows: (*rows).try_into()?,
                    }),
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
            self.close_resources();
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
