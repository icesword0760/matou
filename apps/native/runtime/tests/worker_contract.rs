#![cfg(target_os = "macos")]

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use matou_native_core::{
    journal::{Frame, JournalOptions, for_each_frame},
    pty::ShellLaunch,
};
use matou_native_runtime::{
    SessionEvent, SessionOptions,
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};
use matou_native_terminal::{GridPoint, SelectionAction, SelectionMode, TerminalOptions};

fn options(root: &Path, id: &str) -> Result<SessionOptions> {
    let home = root.join("home");
    let cwd = root.join("workspace");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&cwd)?;
    fs::write(home.join(".zshrc"), "PROMPT='WORKER_READY> '\n")?;
    Ok(SessionOptions {
        session_id: id.into(),
        launch: ShellLaunch {
            profile: matou_native_core::pty::LaunchProfile::Shell,
            argv: Vec::new(),
            executable: "/bin/zsh".into(),
            cwd,
            data_root: root.to_owned(),
            inherited_env: BTreeMap::from([
                ("HOME".into(), home.clone().into_os_string()),
                ("ZDOTDIR".into(), home.into_os_string()),
                ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
                ("SHELL".into(), "/bin/zsh".into()),
                ("LANG".into(), "en_US.UTF-8".into()),
            ]),
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
        },
        terminal: TerminalOptions::new(80, 24, 9, 18),
        journal: JournalOptions {
            compress_sealed: false,
            ..JournalOptions::default()
        },
        restored_history: Vec::new(),
        collect_history: true,
    })
}

#[derive(Default)]
struct Seen {
    pids: BTreeMap<String, u32>,
    output: BTreeMap<String, Vec<(u64, Vec<u8>)>>,
    exited: Vec<String>,
}

impl Seen {
    fn wait(&mut self, inbox: &Receiver<WorkerEvent>, done: impl Fn(&Seen) -> bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !done(self) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let WorkerEvent {
                session_id, event, ..
            } = inbox
                .recv_timeout(remaining)
                .context("wait for real Shell worker events")?;
            match event {
                SessionEvent::Started { pid, .. } => {
                    self.pids.insert(session_id, pid);
                }
                SessionEvent::Output { sequence, data } => {
                    self.output
                        .entry(session_id)
                        .or_default()
                        .push((sequence, data));
                }
                SessionEvent::Exited { .. } => self.exited.push(session_id),
                SessionEvent::Diagnostic { message } => {
                    bail!("unexpected {session_id} diagnostic: {message}")
                }
                SessionEvent::StorageFault { message, .. } => {
                    bail!("unexpected {session_id} storage fault: {message}")
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn frame_with(&self, id: &str, marker: &[u8]) -> Option<&(u64, Vec<u8>)> {
        self.output
            .get(id)?
            .iter()
            .find(|(_, bytes)| bytes.windows(marker.len()).any(|part| part == marker))
    }
}

#[test]
fn real_workers_keep_shell_output_independent_and_return_owned_screens() -> Result<()> {
    let root = tempfile::tempdir()?;
    let root_a = root.path().join("a");
    let root_b = root.path().join("b");
    let (events, incoming) = mpsc::channel();
    let mut a = SessionWorker::spawn(options(&root_a, "a")?, events.clone(), "run-a".into())?;
    let mut b = SessionWorker::spawn(options(&root_b, "b")?, events, "run-b".into())?;
    let (pid_a, pid_b) = (a.process_id(), b.process_id());
    assert!(pid_a > 0 && pid_b > 0);
    assert_ne!(pid_a, pid_b);
    let mut seen = Seen::default();
    seen.wait(&incoming, |seen| {
        seen.frame_with("a", b"WORKER_READY> ").is_some()
            && seen.frame_with("b", b"WORKER_READY> ").is_some()
    })?;
    assert_eq!((seen.pids["a"], seen.pids["b"]), (pid_a, pid_b));

    a.set_egress_frozen(true)?;
    // The central owner can forward requests directly and continue serving
    // events before collecting the command's one-shot response.
    let (reply, written) = mpsc::channel();
    b.send(WorkerCommand::Write {
        bytes: b"printf 'WORKER_%s\\n' 'B_ONLY'\r".to_vec(),
        reply,
    })?;
    seen.wait(&incoming, |seen| {
        seen.frame_with("b", b"WORKER_B_ONLY\r\n").is_some()
    })?;
    written.recv_timeout(Duration::from_secs(2))??;
    let (b_sequence, b_data) = seen.frame_with("b", b"WORKER_B_ONLY\r\n").unwrap();
    let (b_sequence, b_data) = (*b_sequence, b_data.clone());
    let mut stored = Vec::new();
    for_each_frame(&root_b, "b", b_sequence, Some(b_sequence), |record| {
        stored.push(record.frame);
        Ok(())
    })?;
    assert_eq!(
        stored,
        [Frame::Output {
            sequence: b_sequence,
            data: b_data
        }]
    );

    let b_screen = b.screen()?;
    assert!(screen_has(&b_screen, "WORKER_B_ONLY"));
    let output_row = b_screen
        .cells
        .chunks(b_screen.cols)
        .position(|row| {
            row.iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .starts_with("WORKER_B_ONLY")
        })
        .context("find the saved output row in the worker viewport")?;
    let (reply, selected) = mpsc::channel();
    b.send(WorkerCommand::Select {
        action: SelectionAction::Begin {
            mode: SelectionMode::Word,
            row: usize::try_from(b_screen.scrollbar.offset)? + output_row,
            x: 1.,
        },
        reply,
    })?;
    let (reply, copied) = mpsc::channel();
    b.send(WorkerCommand::CopySelection {
        reply,
        interrupt_if_empty: false,
    })?;
    // Copy is queued before waiting for selection to finish.
    assert_eq!(
        copied.recv_timeout(Duration::from_secs(2))??.text,
        "WORKER_B_ONLY"
    );
    selected.recv_timeout(Duration::from_secs(2))??;
    let (reply, view) = mpsc::channel();
    b.send(WorkerCommand::SelectedView { reply })?;
    let (selected_screen, _, range) = view.recv_timeout(Duration::from_secs(2))??;
    let range = range.context("selection travels with its frame")?;
    assert_eq!(
        range.anchor.row,
        usize::try_from(selected_screen.scrollbar.offset)? + output_row
    );
    assert_eq!(
        range.end.col - range.anchor.col,
        isize::try_from("WORKER_B_ONLY".len())?
    );

    a.set_egress_frozen(false)?;
    a.write(b"printf 'WORKER_%s\\n' 'A_BACK'\r")?;
    seen.wait(&incoming, |seen| {
        seen.frame_with("a", b"WORKER_A_BACK\r\n").is_some()
    })?;
    let a_screen = a.screen()?;
    assert!(screen_has(&a_screen, "WORKER_A_BACK"));

    b.write(b"exit\r")?;
    seen.wait(&incoming, |seen| seen.exited.iter().any(|id| id == "b"))?;
    b.acknowledge(b_sequence)?;
    assert!(screen_has(&b.screen()?, "WORKER_B_ONLY"));
    assert!(b.write(b"closed\r").is_err());
    a.shutdown()?;
    b.shutdown()?;
    wait_until_exited(seen.pids["a"])?;
    wait_until_exited(seen.pids["b"])?;
    assert!(screen_has(&a_screen, "WORKER_A_BACK"));
    assert!(screen_has(&b_screen, "WORKER_B_ONLY"));
    Ok(())
}

#[test]
fn drag_scroll_extends_selection_in_the_new_worker_viewport() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut fixture = options(root.path(), "drag-scroll")?;
    fixture.restored_history = (0..100)
        .map(|row| format!("line{row:03}\r\n"))
        .collect::<String>()
        .into_bytes();
    let (events, incoming) = mpsc::channel();
    let mut worker = SessionWorker::spawn(fixture, events, "run-drag".into())?;
    Seen::default().wait(&incoming, |seen| {
        seen.frame_with("drag-scroll", b"WORKER_READY> ").is_some()
    })?;
    let timeout = Duration::from_secs(2);
    let initial = worker.screen()?;
    assert!(initial.scrollbar.offset > 40);
    assert!(screen_has(&initial, "line099"));

    let scroll = |lines| -> Result<_> {
        let (reply, scrolled) = mpsc::channel();
        worker.send(WorkerCommand::ScrollSelection { lines, reply })?;
        let (reply, view) = mpsc::channel();
        worker.send(WorkerCommand::SelectedView { reply })?;
        let (screen, _, selection) = view.recv_timeout(timeout)??;
        scrolled.recv_timeout(timeout)??;
        Ok((screen, selection))
    };
    let copy = |interrupt_if_empty| -> Result<_> {
        let (reply, copied) = mpsc::channel();
        worker.send(WorkerCommand::CopySelection {
            reply,
            interrupt_if_empty,
        })?;
        copied.recv_timeout(timeout)?
    };

    let (unchanged, selection) = scroll(2)?;
    assert_eq!(unchanged.scrollbar.offset, initial.scrollbar.offset);
    assert_eq!(selection, None);

    let (reply, selected) = mpsc::channel();
    worker.send(WorkerCommand::Select {
        action: SelectionAction::Begin {
            mode: SelectionMode::Word,
            row: 99,
            x: 1.,
        },
        reply,
    })?;
    selected.recv_timeout(timeout)??;
    let (unchanged, selection) = scroll(2)?;
    assert_eq!(unchanged.scrollbar.offset, initial.scrollbar.offset);
    assert!(selection.is_some());
    let (reply, extended) = mpsc::channel();
    worker.send(WorkerCommand::Select {
        action: SelectionAction::Extend { row: 99, x: 2. },
        reply,
    })?;
    extended.recv_timeout(timeout)??;
    let (top, selection) = scroll(1000)?;
    assert_eq!(top.scrollbar.offset, 0);
    let selection = selection.context("selection after scrolling up")?;
    assert_eq!(selection.anchor, GridPoint { row: 0, col: 0 });
    assert_eq!(selection.end, GridPoint { row: 99, col: 7 });
    let upward_text = copy(false)?.text;
    assert!(upward_text.starts_with("line000\n"));
    assert!(upward_text.ends_with("line099"));

    let (bottom, selection) = scroll(-1000)?;
    assert_eq!(bottom.scrollbar.offset, initial.scrollbar.offset);
    let selection = selection.context("selection after scrolling down")?;
    assert_eq!(selection.anchor, GridPoint { row: 99, col: 0 });
    assert_eq!(
        selection.end,
        GridPoint {
            row: usize::try_from(bottom.scrollbar.total)? - 1,
            col: isize::try_from(bottom.cols)?,
        }
    );
    let downward_text = copy(false)?.text;
    assert!(downward_text.starts_with("line099\n"));
    assert!(downward_text.contains("WORKER_READY>"));

    let (reply, selected) = mpsc::channel();
    worker.send(WorkerCommand::Select {
        action: SelectionAction::Begin {
            mode: SelectionMode::Column,
            row: 97,
            x: 2.,
        },
        reply,
    })?;
    selected.recv_timeout(timeout)??;
    let (reply, extended) = mpsc::channel();
    worker.send(WorkerCommand::Select {
        action: SelectionAction::Extend { row: 99, x: 2. },
        reply,
    })?;
    extended.recv_timeout(timeout)??;
    let empty_column = copy(true)?;
    assert!(empty_column.text.is_empty());
    assert!(!empty_column.interrupted);
    let (reply, selected) = mpsc::channel();
    worker.send(WorkerCommand::Selection { reply })?;
    let range = selected
        .recv_timeout(timeout)??
        .context("empty column selection remains active after copying")?;
    assert!(range.column);
    assert_eq!(range.anchor, GridPoint { row: 97, col: 2 });
    assert_eq!(range.end, GridPoint { row: 99, col: 2 });
    worker.shutdown()?;
    Ok(())
}

fn screen_has(screen: &matou_native_terminal::Screen, line: &str) -> bool {
    screen.cells.chunks(screen.cols).any(|row| {
        row.iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains(line)
    })
}

fn wait_until_exited(pid: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()?
        .status
        .success()
    {
        ensure!(Instant::now() < deadline, "Shell worker left process {pid}");
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
