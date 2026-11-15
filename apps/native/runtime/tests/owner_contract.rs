#![cfg(target_os = "macos")]

use anyhow::{Result, ensure};
use matou_native_core::{
    hierarchy::{BootstrapWindowInput, HierarchyIds, bootstrap_window},
    journal::{Frame, JournalOptions, for_each_frame},
    ownership::DatabaseOwner,
    pty::ShellLaunch,
    shell_history,
    storage::{CommandMetadata, configure_connection, migrate},
};
use matou_native_runtime::{
    SessionEvent, SessionOptions,
    owner::ShellOwner,
    worker::{WorkerCommand, WorkerEvent},
};
use matou_native_terminal::TerminalOptions;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

const ID: &str = "shell-session";
const DIVIDER: &str = "──────── 会话已恢复 ────────";

fn owner(root: &Path, generation: &str) -> Result<ShellOwner> {
    let database = root.join("matou.sqlite");
    let lease = DatabaseOwner::acquire(&database, generation)?;
    let mut connection = Connection::open(&database)?;
    configure_connection(&connection)?;
    migrate(&mut connection, 1)?;
    bootstrap_window(
        &mut connection,
        &CommandMetadata {
            command_id: "bootstrap".into(),
            command_type: "window.bootstrap".into(),
            request_hash: "frozen-input".into(),
            causation_id: None,
            correlation_id: None,
        },
        &BootstrapWindowInput {
            window_id: "window".into(),
            default_root_directory: root.join("workspace").to_string_lossy().into_owned(),
            default_name: "Workspace".into(),
            now: 1,
        },
        &HierarchyIds {
            workspace_id: "workspace".into(),
            execution_context_id: "context".into(),
            task_id: "task".into(),
            scene_id: "scene".into(),
            root_node_id: "node".into(),
            session_id: ID.into(),
            mount_id: "mount".into(),
        },
        "新事项",
        1,
    )?;
    let prefix = generation.to_owned();
    let mut next = 0;
    Ok(ShellOwner::new(connection, lease, move || {
        next += 1;
        format!("{prefix}-block-{next}")
    }))
}

fn launch(root: &Path) -> SessionOptions {
    let home = root.join("home");
    SessionOptions {
        session_id: ID.into(),
        launch: ShellLaunch {
            executable: "/bin/zsh".into(),
            cwd: root.join("workspace"),
            data_root: root.to_owned(),
            inherited_env: BTreeMap::from([
                (OsString::from("HOME"), home.clone().into_os_string()),
                ("ZDOTDIR".into(), home.into_os_string()),
                ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
                ("SHELL".into(), "/bin/zsh".into()),
                ("LANG".into(), "en_US.UTF-8".into()),
            ]),
            env: BTreeMap::new(),
            cols: 100,
            rows: 24,
        },
        terminal: TerminalOptions::new(100, 24, 9, 18),
        journal: JournalOptions {
            compress_sealed: false,
            ..JournalOptions::default()
        },
        restored_history: Vec::new(),
    }
}

fn collect(
    owner: &mut ShellOwner,
    until: impl Fn(&[WorkerEvent]) -> bool,
) -> Result<Vec<WorkerEvent>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while !until(&events) {
        ensure!(
            Instant::now() < deadline,
            "timed out collecting owner events: {events:?}"
        );
        events.extend(owner.poll(Duration::from_millis(50))?);
        for event in &events {
            ensure!(
                !matches!(
                    event.event,
                    SessionEvent::Diagnostic { .. } | SessionEvent::StorageFault { .. }
                ),
                "unexpected event: {event:?}"
            );
        }
    }
    Ok(events)
}

fn output(events: &[WorkerEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match &event.event {
            SessionEvent::Output { data, .. } => Some(String::from_utf8_lossy(data).into_owned()),
            _ => None,
        })
        .collect()
}

fn write(owner: &ShellOwner, command: &str) -> Result<()> {
    let (reply, result) = mpsc::channel();
    owner.send(
        ID,
        WorkerCommand::Write {
            bytes: format!("{command}\r").into_bytes(),
            reply,
        },
    )?;
    result.recv_timeout(Duration::from_secs(5))??;
    Ok(())
}

fn screen(owner: &ShellOwner) -> Result<String> {
    let (reply, result) = mpsc::channel();
    owner.send(ID, WorkerCommand::Screen { reply })?;
    let screen = result.recv_timeout(Duration::from_secs(5))??;
    Ok(screen
        .cells
        .chunks(screen.cols)
        .map(|row| {
            row.iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

#[test]
fn completed_history_survives_owner_and_database_restart_before_live_shell() -> Result<()> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("home"))?;
    fs::create_dir(root.path().join("workspace"))?;
    fs::write(root.path().join("home/.zshrc"), "PROMPT='OWNER_READY> '\n")?;
    let mut first = owner(root.path(), "first")?;
    first.launch(launch(root.path()), true, DIVIDER)?;
    collect(&mut first, |events| {
        output(events).contains("OWNER_READY> ")
    })?;
    let completed = "printf 'SAVED_%s\\n' '中文🙂'";
    write(&first, completed)?;
    collect(&mut first, |events| {
        events.iter().any(|event| matches!(&event.event, SessionEvent::CompletedBlock(block) if block.command == completed))
    })?;
    write(&first, "printf 'UNFINISHED_%s\\n' not_saved; sleep 30")?;
    collect(&mut first, |events| {
        output(events).contains("UNFINISHED_not_saved\r\n")
    })?;
    first.shutdown()?;
    drop(first);
    let connection = Connection::open(root.path().join("matou.sqlite"))?;
    let blocks = shell_history::list(&connection, ID)?;
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].command, completed);
    assert!(blocks[0].output.contains("SAVED_中文🙂"));
    drop(connection);
    let mut old_sequence = 0;
    for_each_frame(root.path(), ID, 0, None, |record| {
        old_sequence = record.frame.sequence();
        Ok(())
    })?;

    let mut second = owner(root.path(), "second")?;
    second.launch(launch(root.path()), true, DIVIDER)?;
    collect(&mut second, |events| {
        output(events).contains("OWNER_READY> ")
    })?;
    let restored = screen(&second)?;
    assert!(restored.contains("SAVED_中文🙂"), "{restored}");
    assert!(restored.contains(DIVIDER), "{restored}");
    assert!(!restored.contains("UNFINISHED_not_saved"));
    second.shutdown()?;
    drop(second);
    let mut new_output = Vec::new();
    for_each_frame(root.path(), ID, old_sequence + 1, None, |record| {
        if let Frame::Output { data, .. } = record.frame {
            new_output.extend(data);
        }
        Ok(())
    })?;
    assert!(!String::from_utf8_lossy(&new_output).contains("SAVED_"));
    assert!(!String::from_utf8_lossy(&new_output).contains(DIVIDER));

    let mut disabled = owner(root.path(), "disabled")?;
    disabled.launch(launch(root.path()), false, DIVIDER)?;
    collect(&mut disabled, |events| {
        output(events).contains("OWNER_READY> ")
    })?;
    let fresh = screen(&disabled)?;
    assert!(!fresh.contains("SAVED_") && !fresh.contains(DIVIDER));
    disabled.shutdown()?;
    drop(disabled);
    let connection = Connection::open(root.path().join("matou.sqlite"))?;
    assert_eq!(shell_history::list(&connection, ID)?, blocks);
    Ok(())
}
