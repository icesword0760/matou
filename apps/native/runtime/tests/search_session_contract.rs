#![cfg(target_os = "macos")]

use std::{
    collections::BTreeMap,
    fs,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use matou_native_core::{journal::JournalOptions, pty::ShellLaunch};
use matou_native_runtime::{
    SessionEvent, SessionOptions,
    search::SearchRequest,
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};
use matou_native_terminal::{SearchOptions, TerminalOptions};

const WAIT: Duration = Duration::from_secs(4);

#[test]
fn worker_search_selects_cycles_and_refreshes_without_scrolling() -> Result<()> {
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    fs::create_dir(&home)?;
    fs::write(home.join(".zshrc"), "PROMPT='SEARCH_READY> '\n")?;

    let mut restored_history = String::new();
    for row in 0..35 {
        if [5, 17, 29].contains(&row) {
            restored_history.push_str(&format!("row {row}: SESSION_HIT\r\n"));
        } else {
            restored_history.push_str(&format!("row {row}\r\n"));
        }
    }
    let (send, events) = mpsc::channel();
    let mut worker = SessionWorker::spawn(
        SessionOptions {
            session_id: "search-worker".into(),
            launch: ShellLaunch {
                profile: matou_native_core::pty::LaunchProfile::Shell,
                argv: Vec::new(),
                executable: "/bin/zsh".into(),
                cwd: root.path().to_path_buf(),
                data_root: root.path().to_path_buf(),
                inherited_env: BTreeMap::from([
                    ("HOME".into(), home.clone().into_os_string()),
                    ("ZDOTDIR".into(), home.into_os_string()),
                    ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
                    ("SHELL".into(), "/bin/zsh".into()),
                    ("LANG".into(), "en_US.UTF-8".into()),
                ]),
                env: BTreeMap::new(),
                cols: 80,
                rows: 8,
            },
            terminal: TerminalOptions::new(80, 8, 9, 18),
            journal: JournalOptions {
                compress_sealed: false,
                ..JournalOptions::default()
            },
            restored_history: restored_history.into_bytes(),
            collect_history: false,
        },
        send,
        "search-run".into(),
    )?;
    wait_for(
        &events,
        |event| matches!(event, SessionEvent::SavedOutput { data, .. } if data.contains("SEARCH_READY> ")),
    )?;

    let request = |sequence, previous| SearchRequest {
        sequence,
        query: "session_hit".into(),
        options: SearchOptions::default(),
        previous,
    };
    // A mouse click leaves equal endpoints: source getSelectionPosition treats
    // this as no selection, so the first search still starts at the buffer top.
    let (reply, response) = mpsc::channel();
    worker.send(WorkerCommand::Select {
        action: matou_native_terminal::SelectionAction::Select {
            row: 28,
            col: 0,
            length: 0,
        },
        reply,
    })?;
    response.recv_timeout(WAIT)??;
    let first = worker.search(request(1, false))?;
    assert_eq!(first.sequence, 1);
    assert_eq!(first.matches.len(), 3);
    let first_row = first.active.context("first match")?.row;
    let first_screen = worker.screen()?;
    assert_eq!(
        first_screen.scrollbar.offset,
        u64::try_from(first_row - first_screen.rows / 2)?
    );
    assert_eq!(selected_text(&worker)?, "SESSION_HIT");
    let indices = [(2, false), (3, false), (4, false), (5, true)]
        .into_iter()
        .map(|(sequence, previous)| {
            worker
                .search(request(sequence, previous))
                .map(|result| result.result_index)
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(
        [
            first.result_index,
            indices[0],
            indices[1],
            indices[2],
            indices[3]
        ],
        [0, 1, 2, 0, 2]
    );

    let mut case_sensitive = request(6, false);
    case_sensitive.options.case_sensitive = true;
    let cached = worker.search(case_sensitive)?;
    assert!(cached.active.is_none());
    assert_eq!(cached.result_index, -1);
    // This query is unchanged. The frozen addon keeps its existing highlights
    // until a new term or an output-triggered refresh.
    assert_eq!(cached.matches.len(), 3);
    assert!(selected_text(&worker)?.is_empty());

    let missing = worker.search(SearchRequest {
        query: "not in this terminal".into(),
        ..request(7, false)
    })?;
    assert!(missing.active.is_none());
    assert!(missing.matches.is_empty());
    assert_eq!(missing.result_index, -1);
    assert!(selected_text(&worker)?.is_empty());

    worker.search(SearchRequest {
        query: "[".into(),
        ..request(8, false)
    })?;
    let invalid = worker.search(SearchRequest {
        query: "[".into(),
        options: SearchOptions {
            regex: true,
            ..SearchOptions::default()
        },
        ..request(8, false)
    });
    assert!(
        invalid
            .unwrap_err()
            .to_string()
            .contains("invalid terminal search regular expression")
    );
    worker.write(b"printf 'INVALID_QUERY_OUTPUT\\n'\r")?;
    wait_for(
        &events,
        |event| matches!(event, SessionEvent::SavedOutput { data, .. } if data.contains("INVALID_QUERY_OUTPUT")),
    )?;
    let invalid_deadline = Instant::now() + Duration::from_millis(300);
    while let Ok(event) =
        events.recv_timeout(invalid_deadline.saturating_duration_since(Instant::now()))
    {
        assert!(
            !matches!(
                event.event,
                SessionEvent::SearchUpdated { .. } | SessionEvent::Diagnostic { .. }
            ),
            "invalid regex must remain a field-local error"
        );
    }
    let restored = worker.search(request(9, false))?;
    assert_eq!(restored.result_index, 0);
    assert_eq!(selected_text(&worker)?, "SESSION_HIT");

    let offset = worker.screen()?.scrollbar.offset;
    let before_write = Instant::now();
    worker.write(b"printf 'SESSION_%s\\n' 'HIT'\r")?;
    wait_for(
        &events,
        |event| matches!(event, SessionEvent::SavedOutput { data, .. } if data.contains("SESSION_HIT")),
    )?;
    let SessionEvent::SearchUpdated { result } = wait_for(&events, |event| {
        matches!(event, SessionEvent::SearchUpdated { .. })
    })?
    else {
        unreachable!()
    };
    assert!(before_write.elapsed() >= Duration::from_millis(200));
    assert_eq!(result.sequence, 9);
    assert_eq!(result.matches.len(), 4);
    assert_eq!(result.result_index, 3);
    let after = worker.screen()?;
    assert_eq!(after.scrollbar.offset, offset);
    let matched_row = u64::try_from(result.active.context("auto-selected match")?.row)?;
    assert!(matched_row >= after.scrollbar.offset + after.scrollbar.len);
    assert_eq!(selected_text(&worker)?, "SESSION_HIT");

    worker.clear_search()?;
    assert_eq!(selected_text(&worker)?, "SESSION_HIT");
    worker.write(b"printf 'AFTER_%s\\n' 'CLEAR'\r")?;
    wait_for(
        &events,
        |event| matches!(event, SessionEvent::SavedOutput { data, .. } if data.contains("AFTER_CLEAR")),
    )?;
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(WorkerEvent {
                event: SessionEvent::SearchUpdated { .. },
                ..
            }) => {
                bail!("clear search published a later refresh");
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => break,
            Err(error) => return Err(error.into()),
        }
    }

    let empty = worker.search(SearchRequest {
        query: String::new(),
        ..request(10, false)
    })?;
    assert_eq!(empty.result_index, 0);
    assert!(empty.active.is_none());
    assert!(empty.matches.is_empty());
    worker.shutdown()
}

fn wait_for(
    events: &Receiver<WorkerEvent>,
    ready: impl Fn(&SessionEvent) -> bool,
) -> Result<SessionEvent> {
    let deadline = Instant::now() + WAIT;
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .context("wait for retained search worker event")?
            .event;
        if let SessionEvent::Diagnostic { message } = &event {
            bail!("unexpected Shell diagnostic: {message}");
        }
        if ready(&event) {
            return Ok(event);
        }
    }
}

fn selected_text(worker: &SessionWorker) -> Result<String> {
    let (reply, response) = mpsc::channel();
    worker.send(WorkerCommand::CopySelection {
        interrupt_if_empty: false,
        reply,
    })?;
    Ok(response.recv_timeout(WAIT)??.text)
}
