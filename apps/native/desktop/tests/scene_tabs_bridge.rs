#![cfg(target_os = "macos")]

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use matou_native_core::{
    hierarchy::{HierarchyResult, Scene},
    sessions::{SessionRun, list_runs},
    startup::open_read_only,
};
use matou_native_desktop::runtime_bridge::{Command, Event, LaunchOptions, RuntimeBridge};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(8);
const PROMPT: &str = "MATOU_TABS_READY>";

#[test]
fn closed_tab_stops_its_shell_and_the_other_tab_survives_reopen() -> Result<()> {
    let root = std::env::temp_dir().join(format!("matou-native-tabs-{}", Uuid::new_v4()));
    verify_tabs(&root).with_context(|| format!("isolated tab data: {}", root.display()))?;
    fs::remove_dir_all(root)?;
    Ok(())
}

fn verify_tabs(root: &Path) -> Result<()> {
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
    let options = LaunchOptions {
        data_root: root.join("data"),
        workspace: root.join("workspace"),
        shell_home: home,
        shell: PathBuf::from("/bin/zsh"),
    };
    let database = options.data_root.join("matou.sqlite");
    let mut bridge = support::start(options.clone())?;
    let (initial, _) = wait_ready(&bridge)?;
    let original = initial.scene.context("original scene")?.id;
    let task_id = initial.task.context("active task")?.id;

    let (kept, _) = navigate(
        &bridge,
        "create retained canvas",
        Command::CreateCanvas {
            task_id: task_id.clone(),
        },
    )?;
    let kept_scene = kept.scene.context("retained canvas")?.id;
    let kept_session = kept.session.context("retained Shell")?.id;
    wait_rows(&bridge, &kept_session, "retained Shell prompt", &[PROMPT])?;
    input(
        &bridge,
        &kept_session,
        "export MATOU_TABS_TOKEN=RETAINED; printf 'MATOU_%s\\n' SAVED_BEFORE_CLOSE",
    )?;
    wait_rows(
        &bridge,
        &kept_session,
        "retained Shell output",
        &["MATOU_SAVED_BEFORE_CLOSE", PROMPT],
    )?;

    let (closing, _) = navigate(
        &bridge,
        "create second canvas",
        Command::CreateCanvas { task_id },
    )?;
    let closing_scene = closing.scene.context("second canvas")?.id;
    let closing_session = closing.session.context("second Shell")?.id;
    wait_rows(&bridge, &closing_session, "second Shell prompt", &[PROMPT])?;
    let (renamed, scenes) = navigate(
        &bridge,
        "rename second canvas",
        Command::RenameScene {
            scene_id: closing_scene.clone(),
            name: "  等待输入的画布  ".into(),
        },
    )?;
    assert_eq!(
        renamed.scene.context("renamed scene")?.name,
        "等待输入的画布"
    );
    assert!(scenes.iter().any(|scene| scene.id == closing_scene
        && scene.name == "等待输入的画布"
        && scene.title_pinned));
    let (reordered, scenes) = navigate(
        &bridge,
        "move second canvas before the retained canvas",
        Command::ReorderScene {
            scene_id: closing_scene.clone(),
            before_scene_id: Some(kept_scene.clone()),
        },
    )?;
    assert_eq!(
        reordered.scene.context("reordered scene")?.id,
        closing_scene
    );
    assert_eq!(ids(&scenes), [&original, &closing_scene, &kept_scene]);

    input(
        &bridge,
        &closing_session,
        "trap '' HUP; printf 'MATOU_%s%s\\n' CLOSE_PID_ \"$$\"; builtin read -r line",
    )?;
    let pid = wait_for(
        &bridge,
        "second Shell PID while its command waits for input",
        |event| {
            if let Event::Frame {
                session_id, screen, ..
            } = event
                && session_id == closing_session
            {
                for cells in screen.cells.chunks(screen.cols) {
                    let row: String = cells.iter().map(|cell| cell.text.as_str()).collect();
                    if let Some(pid) = row.trim_end().strip_prefix("MATOU_CLOSE_PID_") {
                        return Ok(Some(pid.parse::<u32>()?));
                    }
                }
            }
            Ok(None)
        },
    )?;
    assert!(
        process_exists(pid)?,
        "closing Shell PID {pid} was already gone"
    );

    let (selected, _) = navigate(
        &bridge,
        "select retained canvas",
        Command::SelectScene {
            scene_id: kept_scene.clone(),
        },
    )?;
    assert_eq!(selected.scene.context("retained canvas")?.id, kept_scene);
    send(
        &bridge,
        Command::PreviewSceneClose {
            scene_id: closing_scene.clone(),
        },
    )?;
    wait_for(
        &bridge,
        "preview close of a background canvas",
        |event| match event {
            Event::SceneClosePreview {
                scene_id,
                graph,
                is_last,
            } => {
                assert_eq!(scene_id, closing_scene);
                assert!(!is_last);
                let nodes = graph["nodes"].as_array().context("close preview nodes")?;
                assert!(nodes.iter().any(|node| node["sessionId"] == closing_session
                    && matches!(node["workStatus"].as_str(), Some("running" | "needs-input"))));
                Ok(Some(()))
            }
            Event::Navigation { .. } => bail!("preview unexpectedly changed navigation"),
            _ => Ok(None),
        },
    )?;

    send(
        &bridge,
        Command::CloseScene {
            scene_id: closing_scene.clone(),
            confirmed: true,
        },
    )?;
    input(
        &bridge,
        &kept_session,
        "printf 'MATOU_%s\\n' \"$MATOU_TABS_TOKEN\"",
    )?;
    let disposed = wait_for(
        &bridge,
        "closed Shell removal before navigation",
        |event| match event {
            Event::ScenesDisposed(ids) => Ok(Some(ids)),
            Event::Navigation { .. } => bail!("navigation arrived before disposal"),
            _ => Ok(None),
        },
    )?;
    assert_eq!(disposed, [closing_session.clone()]);
    let (active, still_open) = wait_navigation(&bridge, "navigation after close")?;
    assert_eq!(active.scene.context("remaining canvas")?.id, kept_scene);
    assert_eq!(active.session.context("remaining Shell")?.id, kept_session);
    assert_eq!(ids(&still_open), [&original, &kept_scene]);

    // A previously sent frame can be acknowledged after the tab has been retired.
    send(
        &bridge,
        Command::Applied {
            session_id: closing_session.clone(),
            sequence: 1,
        },
    )?;
    wait_rows(
        &bridge,
        &kept_session,
        "retained PTY accepts input during background Shell retirement",
        &["MATOU_RETAINED", PROMPT],
    )?;
    assert!(
        process_exists(pid)?,
        "retained Shell could only accept input after the closing Shell exited"
    );

    let closed_run = wait_closed_run(&database, &closing_session, pid)?;
    assert!(
        closed_run
            .ended_at
            .is_some_and(|end| end >= closed_run.started_at)
    );
    assert_eq!(closed_run.exit_code, Some(0));
    assert_eq!(closed_run.signal, Some(9));
    bridge.shutdown()?;

    let mut reopened = support::start(options)?;
    let (restored, still_open) = wait_ready(&reopened)?;
    assert_eq!(restored.scene.context("restored canvas")?.id, kept_scene);
    assert_eq!(restored.session.context("restored Shell")?.id, kept_session);
    assert_eq!(ids(&still_open), [&original, &kept_scene]);
    assert_eq!(read_run(&database, &closing_session, pid)?, closed_run);
    wait_rows(
        &reopened,
        &kept_session,
        "retained completed Shell output after reopen",
        &[
            "MATOU_SAVED_BEFORE_CLOSE",
            "MATOU_RETAINED",
            "──────── 会话已恢复 ────────",
            PROMPT,
        ],
    )?;
    reopened.shutdown()?;
    Ok(())
}

fn ids(scenes: &[Scene]) -> Vec<&str> {
    scenes.iter().map(|scene| scene.id.as_str()).collect()
}

fn process_exists(pid: u32) -> Result<bool> {
    ensure!(pid > 0, "invalid Shell PID");
    Ok(ProcessCommand::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()?
        .success())
}

fn read_run(database: &Path, session_id: &str, pid: u32) -> Result<SessionRun> {
    list_runs(&open_read_only(database)?, session_id)?
        .into_iter()
        .find(|run| run.pid == Some(i64::from(pid)))
        .context("closed Shell run is missing from SQLite")
}

fn wait_closed_run(database: &Path, session_id: &str, pid: u32) -> Result<SessionRun> {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if !process_exists(pid)? {
            let run = read_run(database, session_id, pid)?;
            if run.status == "exited" {
                return Ok(run);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    bail!("timed out waiting for the closed Shell PID {pid} to exit and record its status")
}

fn send(bridge: &RuntimeBridge, command: Command) -> Result<()> {
    bridge
        .commands
        .send(command)
        .map_err(|_| anyhow!("runtime command channel closed"))
}

fn input(bridge: &RuntimeBridge, session_id: &str, command: &str) -> Result<()> {
    send(
        bridge,
        Command::Input {
            session_id: session_id.into(),
            bytes: format!("{command}\r").into_bytes(),
            defer_ordering: false,
        },
    )
}

fn wait_ready(bridge: &RuntimeBridge) -> Result<(HierarchyResult, Vec<Scene>)> {
    wait_for(bridge, "bridge Ready", |event| match event {
        Event::Ready {
            hierarchy, scenes, ..
        } => Ok(Some((hierarchy, scenes))),
        _ => Ok(None),
    })
}

fn navigate(
    bridge: &RuntimeBridge,
    phase: &str,
    command: Command,
) -> Result<(HierarchyResult, Vec<Scene>)> {
    send(bridge, command)?;
    wait_navigation(bridge, phase)
}

fn wait_navigation(bridge: &RuntimeBridge, phase: &str) -> Result<(HierarchyResult, Vec<Scene>)> {
    wait_for(bridge, phase, |event| match event {
        Event::Navigation {
            hierarchy, scenes, ..
        } => Ok(Some((hierarchy, scenes))),
        _ => Ok(None),
    })
}

fn wait_rows(bridge: &RuntimeBridge, session: &str, phase: &str, expected: &[&str]) -> Result<()> {
    let mut rows = Vec::new();
    wait_for(bridge, phase, |event| {
        if let Event::Frame {
            session_id, screen, ..
        } = event
            && session_id == session
        {
            rows = screen
                .cells
                .chunks(screen.cols)
                .map(|cells| {
                    cells
                        .iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                })
                .collect();
            if expected
                .iter()
                .all(|item| rows.iter().any(|row| row.trim_end() == *item))
            {
                return Ok(Some(()));
            }
        }
        Ok(None)
    })
    .with_context(|| format!("last screen:\n{}", rows.join("\n")))
}

fn wait_for<T>(
    bridge: &RuntimeBridge,
    phase: &str,
    mut accepts: impl FnMut(Event) -> Result<Option<T>>,
) -> Result<T> {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        match bridge.events.try_recv() {
            Ok(Event::Diagnostic(message))
            | Ok(Event::NavigationFailed { message, .. })
            | Ok(Event::StorageFault { message, .. }) => bail!("{phase}: {message}"),
            Ok(Event::Closed) | Err(async_channel::TryRecvError::Closed) => {
                bail!("{phase}: bridge closed")
            }
            Ok(event) => {
                if let Event::Frame {
                    session_id,
                    sequence,
                    ..
                } = &event
                {
                    send(
                        bridge,
                        Command::Applied {
                            session_id: session_id.clone(),
                            sequence: *sequence,
                        },
                    )?;
                }
                if let Some(result) = accepts(event)? {
                    return Ok(result);
                }
            }
            Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(4)),
        }
    }
    bail!("timed out waiting for {phase}")
}
