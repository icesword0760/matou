#![cfg(target_os = "macos")]

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use matou_native_core::{
    journal::{Frame, JournalOptions, for_each_frame},
    pty::ShellLaunch,
};
use matou_native_runtime::{
    SessionEvent, SessionOptions,
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};
use matou_native_terminal::{Screen, SelectionAction, SelectionMode, TerminalOptions};

const SESSION_ID: &str = "scroll-position";

fn options(root: &Path) -> Result<SessionOptions> {
    let home = root.join("home");
    let cwd = root.join("workspace");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&cwd)?;
    fs::write(home.join(".zshrc"), "PROMPT='SCROLL_READY> '\n")?;
    Ok(SessionOptions {
        session_id: SESSION_ID.into(),
        launch: ShellLaunch {
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

fn wait_for_output(incoming: &Receiver<WorkerEvent>, marker: &[u8]) -> Result<u64> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut output = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match incoming
            .recv_timeout(remaining)
            .context("wait for real Shell output")?
            .event
        {
            SessionEvent::Output { sequence, data } => {
                output.extend(data);
                if output.windows(marker.len()).any(|part| part == marker) {
                    return Ok(sequence);
                }
            }
            SessionEvent::Diagnostic { message } | SessionEvent::StorageFault { message, .. } => {
                bail!("unexpected Shell worker diagnostic: {message}")
            }
            SessionEvent::Exited { .. } => bail!("Shell exited before writing all rows"),
            _ => {}
        }
    }
}

fn screen_rows(screen: &Screen) -> impl Iterator<Item = String> {
    screen.cells.chunks(screen.cols).map(|row| {
        row.iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
    })
}

#[test]
fn queued_absolute_scroll_targets_use_the_latest_committed_viewport() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (events, incoming) = mpsc::channel();
    let mut worker = SessionWorker::spawn(options(root.path())?, events, "run-scroll".into())?;
    wait_for_output(&incoming, b"SCROLL_READY> ")?;
    // Leave the shell waiting after the marker so no prompt can append more rows.
    worker.write(
        b"printf 'SCROLL_ROW_%03d\\n' {0..79}; printf 'SCROLL_%s\\n' 'DONE'; read scroll_hold\r",
    )?;
    let sequence = wait_for_output(&incoming, b"SCROLL_DONE\r\n")?;
    let mut persisted = Vec::new();
    for_each_frame(root.path(), SESSION_ID, 1, Some(sequence), |record| {
        if let Frame::Output { data, .. } = record.frame {
            persisted.extend(data);
        }
        Ok(())
    })?;
    assert!(
        persisted
            .windows(b"SCROLL_DONE\r\n".len())
            .any(|part| part == b"SCROLL_DONE\r\n")
    );

    let initial = worker.screen()?;
    let last = initial.scrollbar.total - initial.rows as u64;
    assert_eq!(initial.scrollbar.offset, last);
    assert!(last >= 50, "the live Shell output creates scrollback");
    let selected = screen_rows(&initial)
        .position(|row| row.starts_with("SCROLL_ROW_079"))
        .context("find the last emitted line in the committed viewport")?;
    let selected = last + selected as u64;
    let first_output = selected - 79;
    let middle = last / 2;

    let (reply, selection_ready) = mpsc::channel();
    worker.send(WorkerCommand::Select {
        action: SelectionAction::Begin {
            mode: SelectionMode::Word,
            row: usize::try_from(selected)?,
            x: 1.,
        },
        reply,
    })?;
    let mut reads = Vec::new();
    for (target, expected) in [
        (0, 0),
        (middle, middle),
        (last, last),
        (middle, middle),
        (u64::MAX, last),
    ] {
        let (reply, scrolled) = mpsc::channel();
        worker.send(WorkerCommand::ScrollTo { row: target, reply })?;
        let (reply, view) = mpsc::channel();
        worker.send(WorkerCommand::SelectedView { reply })?;
        reads.push((expected, scrolled, view));
    }
    let (reply, copied) = mpsc::channel();
    worker.send(WorkerCommand::CopySelection {
        reply,
        interrupt_if_empty: false,
    })?;

    // Every move and screen read was enqueued before waiting for any reply.
    let timeout = Duration::from_secs(2);
    selection_ready.recv_timeout(timeout)??;
    for (expected, scrolled, view) in reads {
        scrolled.recv_timeout(timeout)??;
        let (screen, _, selection) = view.recv_timeout(timeout)??;
        assert_eq!(screen.scrollbar.offset, expected);
        let selection = selection.context("scrolling preserves the existing selection")?;
        assert_eq!(selection.anchor.row as u64, selected);
        if expected == middle {
            let first_visible = screen_rows(&screen).next().context("read top row")?;
            assert!(first_visible.starts_with(&format!("SCROLL_ROW_{:03}", middle - first_output)));
        }
    }
    assert_eq!(copied.recv_timeout(timeout)??.text, "SCROLL_ROW_079");
    worker.shutdown()?;
    Ok(())
}
