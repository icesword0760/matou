#![cfg(target_os = "macos")]

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use matou_native_core::{
    durability::State,
    journal::{Frame, JournalOptions, for_each_frame},
    pty::{ProcessExit, ShellLaunch},
    shell_history::CollectedShellBlock,
};
use matou_native_runtime::{
    SessionEvent, SessionOptions, StorageFaultCode,
    flow::{CREDIT_HIGH_WATERMARK_BYTES, MAX_CODE_UNITS},
    session::Session,
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};
use matou_native_terminal::{TerminalOptions, Theme};

fn shell(root: &Path, id: &str, segment_bytes: u64) -> Result<Session> {
    Session::launch(shell_options(root, id, segment_bytes)?)
}

fn shell_options(root: &Path, id: &str, segment_bytes: u64) -> Result<SessionOptions> {
    let home = root.join("home");
    let cwd = root.join("workspace");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&cwd)?;
    fs::write(home.join(".zshrc"), "PROMPT='MATOU_READY> '\n")?;
    let env = BTreeMap::from([
        (OsString::from("HOME"), home.clone().into_os_string()),
        ("ZDOTDIR".into(), home.into_os_string()),
        ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
        ("SHELL".into(), "/bin/zsh".into()),
        ("LANG".into(), "en_US.UTF-8".into()),
    ]);
    Ok(SessionOptions {
        session_id: id.into(),
        launch: ShellLaunch {
            executable: "/bin/zsh".into(),
            cwd,
            data_root: root.to_owned(),
            inherited_env: env,
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
        },
        terminal: TerminalOptions::new(80, 24, 9, 18),
        restored_history: Vec::new(),
        collect_history: true,
        journal: JournalOptions {
            max_segment_bytes: segment_bytes,
            compress_sealed: false,
            ..JournalOptions::default()
        },
    })
}

fn collect(
    session: &mut Session,
    events: &mut Vec<SessionEvent>,
    done: impl Fn(&[SessionEvent], &Session) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !done(events, session) {
        if Instant::now() >= deadline {
            let mut tail: Vec<u8> = events
                .iter()
                .rev()
                .filter_map(|event| match event {
                    SessionEvent::Output { data, .. } => Some(data.iter().rev().copied()),
                    _ => None,
                })
                .flatten()
                .take(512)
                .collect();
            tail.reverse();
            anyhow::bail!(
                "timed out waiting for Shell: {} events, sequence {}, {:?}, closed={}; output tail {:?}",
                events.len(),
                session.last_sequence(),
                session.durability_state(),
                session.is_closed(),
                String::from_utf8_lossy(&tail),
            );
        }
        events.extend(session.poll(Duration::from_millis(50))?);
        if let Some(message) = events.iter().find_map(|event| match event {
            SessionEvent::Diagnostic { message } => Some(message),
            _ => None,
        }) {
            anyhow::bail!("unexpected session diagnostic: {message}");
        }
    }
    Ok(())
}

fn collect_worker(
    incoming: &mpsc::Receiver<WorkerEvent>,
    events: &mut Vec<SessionEvent>,
    timeout: Duration,
    ready: impl Fn(&[SessionEvent]) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while !ready(events) {
        let event = incoming
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))?
            .event;
        if let SessionEvent::Diagnostic { message } = &event {
            anyhow::bail!("unexpected session diagnostic: {message}");
        }
        events.push(event);
    }
    Ok(())
}

fn output_bytes(events: &[SessionEvent]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for event in events {
        if let SessionEvent::Output { data, .. } = event {
            bytes.extend_from_slice(data);
        }
    }
    bytes
}

fn output_has(events: &[SessionEvent], text: &str) -> bool {
    output_bytes(events)
        .windows(text.len())
        .any(|span| span == text.as_bytes())
}

fn has_cursor_reply(events: &[SessionEvent]) -> bool {
    let output = output_bytes(events);
    let start = b"DSR_before:\x1b[2;3H\x1b[6n";
    let stop = b"DSR_after";
    let Some(offset) = output.windows(start.len()).position(|span| span == start) else {
        return false;
    };
    let rest = &output[offset + start.len()..];
    let Some(end) = rest.windows(stop.len()).position(|span| span == stop) else {
        return false;
    };
    rest[..end]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|token| !token.is_empty())
        .eq([b"1b", b"5b", b"32", b"3b", b"33", b"52"].map(|token| token.as_slice()))
}

fn has_block(events: &[SessionEvent], command: &str) -> bool {
    completed_blocks(events).any(|block| block.command == command)
}

fn completed_blocks(events: &[SessionEvent]) -> impl Iterator<Item = &CollectedShellBlock> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SavedOutput {
                completed_blocks, ..
            } => Some(completed_blocks),
            _ => None,
        })
        .flatten()
}

fn assert_persisted_events(events: &[SessionEvent], frames: &[Frame]) {
    let output_events: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SavedOutput { sequence, data, .. } => Some((*sequence, data.as_bytes())),
            _ => None,
        })
        .collect();
    let exit_events: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SavedExit {
                sequence, status, ..
            } => Some((*sequence, *status)),
            _ => None,
        })
        .collect();
    let output_frames: Vec<_> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Output { sequence, data } => Some((*sequence, data.as_slice())),
            _ => None,
        })
        .collect();
    let exit_frames: Vec<_> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Exit {
                sequence,
                exit_code,
                signal,
            } => Some((
                *sequence,
                ProcessExit {
                    exit_code: *exit_code,
                    signal: *signal,
                },
            )),
            _ => None,
        })
        .collect();
    assert_eq!(output_events, output_frames);
    assert_eq!(exit_events, exit_frames);
    assert_eq!(exit_events.len(), 1);
}

fn input(session: &mut Session, command: &str) -> Result<()> {
    session.write(format!("{command}\r").as_bytes())
}

fn stored_frames(root: &Path, id: &str) -> Result<Vec<Frame>> {
    let mut frames = Vec::new();
    for_each_frame(root, id, 0, None, |record| {
        frames.push(record.frame);
        Ok(())
    })?;
    Ok(frames)
}

fn require_started(session: &mut Session, events: &mut Vec<SessionEvent>) -> Result<()> {
    collect(session, events, |events, _| {
        output_has(events, "MATOU_READY> ")
    })?;
    assert!(
        matches!(events.first(), Some(SessionEvent::Started { pid, sequence: 0 }) if *pid == session.process_id())
    );
    Ok(())
}

#[test]
fn live_shell_persists_output_for_history_screen_resize_and_exit() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut session = shell(root.path(), "live", 16 * 1024 * 1024)?;
    let mut events = Vec::new();
    require_started(&mut session, &mut events)?;
    let command = r#"printf 'VISIBLE_%s\n' '中文🙂'"#;
    let before_submit = events.len();
    input(&mut session, command)?;
    collect(&mut session, &mut events, |events, _| {
        has_block(events, command) && output_has(events, "VISIBLE_中文🙂\r\n")
    })?;
    assert!(matches!(
        events.get(before_submit),
        Some(SessionEvent::InputSubmitted)
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::InputSubmitted))
            .count(),
        1
    );
    let (block, sequence) = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            SessionEvent::SavedOutput {
                sequence,
                completed_blocks,
                ..
            } if completed_blocks.iter().any(|block| {
                block.command == command
                    && block.exit_code == 0
                    && block.output.contains("VISIBLE_中文🙂")
            }) =>
            {
                Some((index, *sequence))
            }
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(&events[block + 1], SessionEvent::Output { sequence: display, .. } if *display == sequence)
    );
    let screen = session.screen()?;
    assert!(screen.cells.chunks(screen.cols).any(|row| {
        row.iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("VISIBLE_中文🙂")
    }));

    session.resize(97, 33, 11, 23)?;
    input(&mut session, "stty size")?;
    collect(&mut session, &mut events, |events, _| {
        output_has(events, "33 97\r\n")
    })?;
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Resized {
            cols: 97,
            rows: 33,
            ..
        }
    )));

    let reply_command = r#"stty -echo -icanon min 0 time 10; printf 'DSR_%s:' before; printf '\033[2;3H\033[6n'; dd bs=1 count=6 2>/dev/null | od -An -tx1; printf 'DSR_%s\n' after; stty sane"#;
    input(&mut session, reply_command)?;
    collect(&mut session, &mut events, |events, _| {
        has_cursor_reply(events) && has_block(events, reply_command)
    })?;
    input(&mut session, "exit 7")?;
    collect(&mut session, &mut events, |_, session| session.is_closed())?;

    let frames = stored_frames(root.path(), "live")?;
    assert!(
        frames
            .iter()
            .zip(1..)
            .all(|(frame, sequence)| frame.sequence() == sequence)
    );
    assert!(frames.iter().any(|frame| matches!(
        frame,
        Frame::Resize {
            cols: 97,
            rows: 33,
            ..
        }
    )));
    assert!(matches!(
        frames.last(),
        Some(Frame::Exit {
            exit_code: 7,
            signal: Some(0),
            ..
        })
    ));
    assert!(
        matches!(events.last(), Some(SessionEvent::Exited { sequence, status: ProcessExit { exit_code: 7, signal: Some(0) } })
        if *sequence == frames.last().unwrap().sequence())
    );
    assert!(
        matches!(events.get(events.len() - 2), Some(SessionEvent::SavedExit { sequence, status: ProcessExit { exit_code: 7, signal: Some(0) }, disposed: false })
        if *sequence == frames.last().unwrap().sequence())
    );
    let published: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { sequence, data } => Some((*sequence, data.as_slice())),
            _ => None,
        })
        .collect();
    let saved: Vec<_> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Output { sequence, data } => Some((*sequence, data.as_slice())),
            _ => None,
        })
        .collect();
    assert_eq!(published, saved);
    assert_persisted_events(&events, &frames);
    Ok(())
}

#[test]
fn real_storage_fault_stops_one_session_and_retries_the_same_sequence() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut blocked = shell(root.path(), "blocked", 512)?;
    let mut other = shell(root.path(), "other", 16 * 1024 * 1024)?;
    let mut blocked_events = Vec::new();
    let mut other_events = Vec::new();
    require_started(&mut blocked, &mut blocked_events)?;
    require_started(&mut other, &mut other_events)?;
    let warmup = r#"printf 'FIRST_%s\n' ready"#;
    input(&mut blocked, warmup)?;
    collect(&mut blocked, &mut blocked_events, |events, _| {
        has_block(events, warmup)
    })?;

    let before_fault = blocked.screen()?;
    let range = root.path().join("journal/blocked/range-index.json");
    if range.exists() {
        fs::remove_file(&range)?;
    }
    fs::create_dir(&range)?;
    let command = r#"printf 'FAULT_%04096d\n' 0"#;
    input(&mut blocked, command)?;
    collect(&mut blocked, &mut blocked_events, |_, session| {
        session.durability_state() == State::Paused
    })?;
    let sequence = blocked_events
        .iter()
        .find_map(|event| match event {
            SessionEvent::StorageFault {
                sequence,
                code: StorageFaultCode::WriteFailed,
                retained_bytes,
                ..
            } if *retained_bytes > 0 => Some(*sequence),
            _ => None,
        })
        .expect("real rotation failure should pause storage");
    assert!(blocked.write(b"no input during failure\r").is_err());
    assert_eq!(
        blocked.screen()?,
        before_fault,
        "undurable output reached the visible screen"
    );
    assert!(!blocked_events.iter().any(
        |event| matches!(event, SessionEvent::Output { sequence: sent, .. } |
            SessionEvent::SavedOutput { sequence: sent, .. } if *sent == sequence)
    ));

    let independent = r#"printf 'OTHER_%s\n' running"#;
    input(&mut other, independent)?;
    collect(&mut other, &mut other_events, |events, _| {
        output_has(events, "OTHER_running") && has_block(events, independent)
    })?;

    blocked.set_egress_frozen(true)?;
    fs::remove_dir(&range)?;
    blocked.retry_storage()?;
    assert_eq!(blocked.durability_state(), State::Healthy);
    assert!(blocked.write(b"still egress frozen\r").is_err());
    blocked.set_egress_frozen(false)?;
    collect(&mut blocked, &mut blocked_events, |events, _| {
        has_block(events, command) && events.iter().any(|event| matches!(event, SessionEvent::StorageRecovered { sequence: through } if *through >= sequence))
    })?;
    assert_eq!(blocked_events.iter().filter(|event| matches!(event, SessionEvent::Output { sequence: sent, .. } if *sent == sequence)).count(), 1);
    assert!(output_has(&blocked_events, "FAULT_000000000"));
    assert_ne!(
        blocked.screen()?,
        before_fault,
        "recovered output should refresh the visible screen"
    );
    assert_eq!(
        stored_frames(root.path(), "blocked")?
            .iter()
            .filter(|frame| frame.sequence() == sequence)
            .count(),
        1
    );
    blocked.shutdown()?;
    blocked_events.extend(blocked.poll(Duration::ZERO)?);
    assert_eq!(
        blocked_events
            .iter()
            .filter(|event| matches!(event, SessionEvent::InputSubmitted))
            .count(),
        2,
        "only the warmup and fault-triggering writes reached the shell"
    );
    let blocked_frames = stored_frames(root.path(), "blocked")?;
    assert_persisted_events(&blocked_events, &blocked_frames);
    assert!(
        blocked_events
            .iter()
            .any(|event| matches!(event, SessionEvent::SavedExit { disposed: true, .. }))
    );
    other.shutdown()?;
    Ok(())
}

#[test]
fn slow_delivery_replays_saved_real_output_without_redoing_history() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut session = shell(root.path(), "credit", 512 * 1024)?;
    let mut events = Vec::new();
    require_started(&mut session, &mut events)?;
    let command = r#"printf 'BULK_%s\n' start; head -c 2228224 /dev/zero | tr '\000' X; printf '\nBULK_%s\n' end"#;
    input(&mut session, command)?;
    collect(&mut session, &mut events, |events, _| {
        has_block(events, command)
    })?;
    assert!(session.unacked_bytes() > CREDIT_HIGH_WATERMARK_BYTES);
    assert!(session.unacked_bytes() <= CREDIT_HIGH_WATERMARK_BYTES + (MAX_CODE_UNITS + 8192));
    assert!(!output_has(&events, "BULK_end"));
    let initially_delivered = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::Output { .. }))
        .count();

    input(&mut session, "exit 0")?;
    collect(&mut session, &mut events, |_, session| session.is_closed())?;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::Exited { .. }))
    );
    let saved = stored_frames(root.path(), "credit")?;
    assert_persisted_events(&events, &saved);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::SavedExit {
            disposed: false,
            ..
        }
    )));
    let mut acknowledged = 0;
    let mut resume_count = 0;
    while !events
        .iter()
        .any(|event| matches!(event, SessionEvent::Exited { .. }))
    {
        let latest = events
            .iter()
            .rev()
            .find_map(|event| match event {
                SessionEvent::Output { sequence, .. } => Some(*sequence),
                _ => None,
            })
            .unwrap();
        assert!(
            latest > acknowledged,
            "acknowledgement did not make progress"
        );
        session.acknowledge(latest)?;
        acknowledged = latest;
        resume_count += 1;
        events.extend(session.poll(Duration::ZERO)?);
        assert!(session.unacked_bytes() <= CREDIT_HIGH_WATERMARK_BYTES + (MAX_CODE_UNITS + 8192));
    }
    assert!(resume_count >= 2);
    assert!(output_has(&events, "BULK_end\r\n"));
    assert_eq!(
        completed_blocks(&events)
            .filter(|block| block.command == command)
            .count(),
        1
    );
    assert_persisted_events(&events, &saved);
    let published: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { sequence, data } => Some((*sequence, data.as_slice())),
            _ => None,
        })
        .collect();
    let saved_output: Vec<_> = saved
        .iter()
        .filter_map(|frame| match frame {
            Frame::Output { sequence, data } => Some((*sequence, data.as_slice())),
            _ => None,
        })
        .collect();
    assert!(
        saved_output
            .iter()
            .map(|(_, data)| data.len())
            .sum::<usize>()
            > 2 * CREDIT_HIGH_WATERMARK_BYTES
    );
    assert!(published.len() > initially_delivered);
    assert_eq!(published, saved_output);
    Ok(())
}

#[test]
fn worker_view_scroll_and_theme_remain_available_after_shell_exit() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (send_event, receive_event) = mpsc::channel();
    let mut worker = SessionWorker::spawn(
        shell_options(root.path(), "view", 16 * 1024 * 1024)?,
        send_event,
        "view-run".to_owned(),
    )?;
    let mut events = Vec::new();
    let timeout = Duration::from_secs(5);
    collect_worker(&receive_event, &mut events, timeout, |events| {
        output_has(events, "MATOU_READY> ")
    })?;
    let command = r#"printf '\033[?1h\033[?2004h'; for n in {001..080}; do printf 'MATOU_HISTORY_%s\n' "$n"; done; printf 'MATOU_%s\n' VIEW_READY; read -r answer; exit 0"#;
    worker.write(format!("{command}\r").as_bytes())?;
    collect_worker(&receive_event, &mut events, timeout, |events| {
        output_has(events, "MATOU_VIEW_READY\r\n")
    })?;

    let view = || {
        let (reply, result) = mpsc::channel();
        worker.send(WorkerCommand::View { reply })?;
        result.recv_timeout(timeout)?
    };
    let scroll = |lines| {
        let (reply, result) = mpsc::channel();
        worker.send(WorkerCommand::Scroll { lines, reply })?;
        result.recv_timeout(timeout)?
    };
    let frames = stored_frames(root.path(), "view")?;
    let (bottom, modes) = view()?;
    assert!(modes.application_cursor_keys && modes.bracketed_paste);
    assert!(bottom.history_lines > 40);
    assert_eq!(bottom.background, Theme::LIGHT.background);

    scroll(10)?;
    let (older, _) = view()?;
    assert_eq!(older.scrollbar.offset + 10, bottom.scrollbar.offset);
    let (reply, result) = mpsc::channel();
    worker.send(WorkerCommand::SetTheme {
        theme: Theme::DARK,
        reply,
    })?;
    result.recv_timeout(timeout)??;
    let (dark, _) = view()?;
    assert_eq!(dark.scrollbar, older.scrollbar);
    assert_eq!(dark.foreground, Theme::DARK.foreground);
    assert_eq!(dark.background, Theme::DARK.background);
    assert_eq!(stored_frames(root.path(), "view")?, frames);

    worker.write(b"\r")?;
    collect_worker(&receive_event, &mut events, timeout, |events| {
        matches![
            events.last(),
            Some(SessionEvent::Exited {
                status: ProcessExit { exit_code: 0, .. },
                ..
            })
        ]
    })?;
    scroll(-100)?;
    let (finished, _) = view()?;
    assert_eq!(finished.background, Theme::DARK.background);
    assert!(finished.history_lines > 40);
    scroll(10)?;
    assert_eq!(view()?.0.scrollbar.offset + 10, finished.scrollbar.offset);
    worker.shutdown()?;
    Ok(())
}

#[test]
fn an_idle_synchronized_shell_invalidates_its_screen_on_timeout() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut session = shell(root.path(), "sync", 16 * 1024 * 1024)?;
    let mut events = Vec::new();
    require_started(&mut session, &mut events)?;
    let before = session.view()?.0;
    let command = r#"printf '\033[?2026hMATOU_SYNC_%s\n' HELD; read -r answer; exit 0"#;
    input(&mut session, command)?;
    collect(&mut session, &mut events, |events, _| {
        output_has(events, "\x1b[?2026hMATOU_SYNC_HELD\r\n")
    })?;
    assert_eq!(session.view()?.0, before);
    let sequence = session.last_sequence();
    let frames = stored_frames(root.path(), "sync")?;
    let mut timeout_events = Vec::new();
    collect(&mut session, &mut timeout_events, |events, _| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::ScreenInvalidated))
    })?;
    assert!(matches!(
        timeout_events.as_slice(),
        [SessionEvent::ScreenInvalidated]
    ));
    let (screen, _) = session.view()?;
    assert!(screen.cells.chunks(screen.cols).any(|row| {
        row.iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("MATOU_SYNC_HELD")
    }));
    assert_eq!(session.last_sequence(), sequence);
    assert_eq!(stored_frames(root.path(), "sync")?, frames);
    assert!(session.poll(Duration::ZERO)?.is_empty());
    session.shutdown()?;
    Ok(())
}

#[test]
fn synchronized_shell_still_invalidates_its_screen_after_exit() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (send_event, receive_event) = mpsc::channel();
    let mut worker = SessionWorker::spawn(
        shell_options(root.path(), "sync-exit", 16 * 1024 * 1024)?,
        send_event,
        "sync-exit-run".to_owned(),
    )?;
    let timeout = Duration::from_secs(5);
    let mut events = Vec::new();
    collect_worker(&receive_event, &mut events, timeout, |events| {
        output_has(events, "MATOU_READY> ")
    })?;
    worker.screen()?;
    let started = Instant::now();
    worker.write(b"printf '\\033[?2026hMATOU_SYNC_%s\\n' END; exit 0\r")?;
    collect_worker(&receive_event, &mut events, timeout, |events| {
        matches!(
            events.last(),
            Some(SessionEvent::Exited {
                status: ProcessExit { exit_code: 0, .. },
                ..
            })
        )
    })?;
    assert!(output_has(&events, "\x1b[?2026hMATOU_SYNC_END\r\n"));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ScreenInvalidated))
    );
    let frames = stored_frames(root.path(), "sync-exit")?;
    assert!(matches!(frames.last(), Some(Frame::Exit { .. })));
    assert!(
        frames[..frames.len() - 1]
            .iter()
            .all(|frame| matches!(frame, Frame::Output { .. }))
    );
    assert_persisted_events(&events, &frames);

    let released = receive_event.recv_timeout(timeout)?.event;
    assert!(matches!(released, SessionEvent::ScreenInvalidated));
    assert!(started.elapsed() >= Duration::from_millis(150));
    let (reply, view) = mpsc::channel();
    worker.send(WorkerCommand::View { reply })?;
    let (screen, _) = view.recv_timeout(timeout)??;
    assert!(screen.cells.chunks(screen.cols).any(|row| {
        row.iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("MATOU_SYNC_END")
    }));
    assert_eq!(stored_frames(root.path(), "sync-exit")?, frames);
    worker.shutdown()?;
    Ok(())
}

#[test]
fn a_cross_thread_wake_interrupts_a_waiting_owner() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut session = shell(root.path(), "wake", 16 * 1024 * 1024)?;
    let mut events = Vec::new();
    require_started(&mut session, &mut events)?;
    // Consume the rest of the prompt before blocking while zsh waits for input.
    while !session.poll(Duration::from_millis(50))?.is_empty() {}
    let waker = session.waker();
    let thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        waker.wake();
    });
    let started = Instant::now();
    assert!(session.poll(Duration::from_secs(4))?.is_empty());
    assert!(started.elapsed() < Duration::from_secs(2));
    thread.join().unwrap();
    session.shutdown()?;
    Ok(())
}
