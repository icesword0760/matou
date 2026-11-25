#![cfg(target_os = "macos")]

use std::{
    fs,
    os::unix::fs::symlink,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use matou_native_core::{sessions::list_runs, startup::open_read_only};
use matou_native_runtime::host::{
    Command, Event, LaunchOptions, OfflineFixtures, RecoveryState, RuntimeHost,
};

#[test]
fn shell_launch_failure_restarts_in_place_and_rebuild_failure_stays_recovery() -> Result<()> {
    let root = tempfile::tempdir()?;
    let options = LaunchOptions {
        data_root: root.path().join("data"),
        workspace: root.path().join("workspace"),
        shell_home: root.path().join("home"),
        shell: root.path().join("zsh"),
    };
    fs::create_dir_all(&options.shell_home)?;
    fs::write(
        options.shell_home.join(".zshrc"),
        "printf 'STARTUP_RETRY_PTY\n'\n",
    )?;
    let mut runtime =
        RuntimeHost::start_with_offline_fixtures(options.clone(), OfflineFixtures::default())?;
    let (scene_id, session_id) = ready(&runtime)?;
    let restart = || Command::RestartSession {
        session_id: session_id.clone(),
    };
    let recover = || Command::RetryRecovery {
        session_id: session_id.clone(),
    };
    let database = open_read_only(&options.data_root.join("matou.sqlite"))?;
    let error = failed_launch(&runtime, &session_id, false)?;
    assert!(error.contains(&format!("start Shell {}", options.shell.display())));
    assert!(error.contains("No such file or directory"), "{error}");
    assert!(list_runs(&database, &session_id)?.is_empty());

    runtime.commands.send(recover())?;
    assert_retry_ignored(&runtime, &scene_id, &session_id)?;

    // A failed retry must report the latest failure, even before a worker exists.
    fs::remove_dir(&options.workspace)?;
    runtime.commands.send(restart())?;
    expect_state(&runtime, &session_id, RecoveryState::Restarting(error))?;
    let second_error = failed_launch(&runtime, &session_id, false)?;
    assert!(second_error.contains("工作区目录不可用") && second_error.contains("missing"));
    assert!(list_runs(&database, &session_id)?.is_empty());

    fs::create_dir(&options.workspace)?;
    symlink("/bin/zsh", &options.shell)?;
    runtime.commands.send(restart())?;
    runtime.commands.send(restart())?;
    expect_state(
        &runtime,
        &session_id,
        RecoveryState::Restarting(second_error),
    )?;
    expect_state(&runtime, &session_id, RecoveryState::Ready)?;
    wait_for_shell_output(&runtime, &session_id)?;

    runtime.commands.send(restart())?;
    assert_retry_ignored(&runtime, &scene_id, &session_id)?;
    assert_eq!(list_runs(&database, &session_id)?.len(), 1);

    // The command also accepts exited Shells. The desktop need not expose a
    // restart action for them.
    runtime.commands.send(Command::Input {
        session_id: session_id.clone(),
        bytes: b"exit\r".to_vec(),
        defer_ordering: false,
    })?;
    wait(&runtime, "normal exit", |event| match event {
        Event::Graph(graph) => Ok(graph["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|node| node["sessionId"] == session_id && node["workStatus"] == "exited")
            .then_some(())),
        Event::Recovery { .. } => bail!("normal exit emitted a failure state"),
        _ => Ok(None),
    })?;
    runtime.commands.send(restart())?;
    expect_state(
        &runtime,
        &session_id,
        RecoveryState::Restarting(String::new()),
    )?;
    expect_state(&runtime, &session_id, RecoveryState::Ready)?;
    wait_for_shell_output(&runtime, &session_id)?;
    assert_eq!(list_runs(&database, &session_id)?.len(), 2);
    runtime.shutdown()?;

    fs::remove_file(&options.shell)?;
    let mut runtime =
        RuntimeHost::start_with_offline_fixtures(options.clone(), OfflineFixtures::default())?;
    assert_eq!(ready(&runtime)?, (scene_id.clone(), session_id.clone()));
    let recovery_error = failed_launch(&runtime, &session_id, true)?;
    assert!(recovery_error.contains(&format!("start Shell {}", options.shell.display())));
    runtime.commands.send(restart())?;
    assert_retry_ignored(&runtime, &scene_id, &session_id)?;
    symlink("/bin/zsh", &options.shell)?;
    runtime.commands.send(recover())?;
    expect_state(&runtime, &session_id, RecoveryState::Restoring)?;
    expect_state(&runtime, &session_id, RecoveryState::Ready)?;
    wait_for_shell_output(&runtime, &session_id)?;
    assert_eq!(list_runs(&database, &session_id)?.len(), 3);

    fs::remove_file(&options.shell)?;
    runtime.commands.send(Command::CreateShellSibling {
        scene_id: scene_id.clone(),
        source_session_id: session_id,
        parent_session_id: None,
    })?;
    let removed = wait(&runtime, "new sibling", |event| match event {
        Event::Navigation { hierarchy, .. } => Ok(Some(hierarchy.session.context("sibling")?.id)),
        _ => Ok(None),
    })?;
    failed_launch(&runtime, &removed, false)?;
    runtime.commands.send(Command::CloseSessionCard {
        scene_id: scene_id.clone(),
        session_id: removed.clone(),
        confirmed_intent: Some(format!("delete-session:{removed}")),
        preserve_scene_on_last_session: false,
    })?;
    symlink("/bin/zsh", &options.shell)?;
    runtime.commands.send(Command::RestartSession {
        session_id: removed.clone(),
    })?;
    assert_retry_ignored(&runtime, &scene_id, &removed)?;
    assert!(list_runs(&database, &removed)?.is_empty());
    runtime.shutdown()
}

fn ready(runtime: &RuntimeHost) -> Result<(String, String)> {
    wait(runtime, "ready hierarchy", |event| match event {
        Event::Ready { hierarchy, .. } => Ok(Some((
            hierarchy.scene.context("scene")?.id,
            hierarchy.session.context("Shell session")?.id,
        ))),
        _ => Ok(None),
    })
}

fn failed_launch(runtime: &RuntimeHost, id: &str, rebuilding: bool) -> Result<String> {
    let reason = wait(runtime, "failure state", |event| match event {
        Event::Recovery {
            session_id, state, ..
        } if session_id == id => match state {
            RecoveryState::StartFailed(error) if !rebuilding => Ok(Some(error)),
            RecoveryState::Failed(error) if rebuilding => Ok(Some(error)),
            _ => bail!("wrong failure state: {state:?}"),
        },
        Event::Frame { session_id, .. } if session_id == id => {
            bail!("failed Shell produced a frame")
        }
        _ => Ok(None),
    })?;
    wait(runtime, "matching diagnostic", |event| match event {
        Event::Diagnostic(message) => {
            assert_eq!(message, reason);
            Ok(Some(()))
        }
        _ => Ok(None),
    })?;
    Ok(reason)
}

fn expect_state(runtime: &RuntimeHost, id: &str, expected: RecoveryState) -> Result<()> {
    wait(runtime, "Shell state", |event| match event {
        Event::Recovery {
            session_id, state, ..
        } if session_id == id => {
            assert_eq!(state, expected);
            Ok(Some(()))
        }
        _ => Ok(None),
    })
}

fn assert_retry_ignored(runtime: &RuntimeHost, scene: &str, session: &str) -> Result<()> {
    // A preview is a command-queue barrier with no launch side effects.
    runtime.commands.send(Command::PreviewSceneClose {
        scene_id: scene.into(),
    })?;
    wait(runtime, "retry rejection barrier", |event| match event {
        Event::SceneClosePreview { scene_id, .. } if scene_id == scene => Ok(Some(())),
        Event::Recovery {
            session_id, state, ..
        } if session_id == session => {
            bail!("rejected retry unexpectedly emitted {state:?}")
        }
        _ => Ok(None),
    })
}

fn wait_for_shell_output(runtime: &RuntimeHost, id: &str) -> Result<()> {
    wait(runtime, "live Shell output", |event| match event {
        Event::Frame {
            session_id, screen, ..
        } if session_id == id => Ok((screen
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("STARTUP_RETRY_PTY"))
        .then_some(())),
        Event::Diagnostic(message) => bail!("{message}"),
        _ => Ok(None),
    })
}

fn wait<T>(
    runtime: &RuntimeHost,
    label: &str,
    accept: impl Fn(Event) -> Result<Option<T>>,
) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        match runtime.events.try_recv() {
            Ok(Event::Closed) => bail!("runtime closed while waiting for {label}"),
            Ok(event) => {
                if let Event::Frame {
                    session_id,
                    sequence,
                    ..
                } = &event
                {
                    runtime.commands.send(Command::Applied {
                        session_id: session_id.clone(),
                        sequence: *sequence,
                    })?;
                }
                if let Some(value) = accept(event)? {
                    return Ok(value);
                }
            }
            Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(async_channel::TryRecvError::Closed) => bail!("event channel closed"),
        }
    }
    bail!("timed out waiting for {label}")
}
