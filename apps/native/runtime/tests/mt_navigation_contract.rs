#![cfg(unix)]

use anyhow::{Context, Result, bail, ensure};
use matou_native_core::hierarchy::HierarchyResult;
use matou_native_runtime::host::{Command, Event, LaunchOptions, OfflineFixtures, RuntimeHost};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command as Process, Output},
    thread,
    time::{Duration, Instant},
};

const RUNTIME: &str = env!("CARGO_BIN_EXE_matou-native-runtime");
const WINDOW: &str = "main-window-1";
const WAIT: Duration = Duration::from_secs(8);

fn event(host: &RuntimeHost, expected: impl Fn(&Event) -> bool) -> Result<Event> {
    let deadline = Instant::now() + WAIT;
    loop {
        ensure!(
            Instant::now() < deadline,
            "timed out waiting for navigation event"
        );
        match host.events.try_recv() {
            Ok(event) if expected(&event) => return Ok(event),
            Ok(Event::Diagnostic(message)) => bail!("RuntimeHost diagnostic: {message}"),
            Ok(Event::NavigationFailed { message, .. }) => {
                bail!("RuntimeHost navigation: {message}")
            }
            Ok(Event::Closed) => bail!("RuntimeHost stopped before the expected event"),
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(error.into()),
        }
    }
}

fn final_path(hierarchy: &HierarchyResult) -> Value {
    json!({
        "routeWindowId": WINDOW,
        "targetWindowId": WINDOW,
        "workspaceId": hierarchy.workspace.as_ref().unwrap().id,
        "taskId": hierarchy.task.as_ref().unwrap().id,
        "sceneId": hierarchy.scene.as_ref().unwrap().id,
        "sessionId": hierarchy.session.as_ref().unwrap().id,
    })
}

fn navigate(host: &RuntimeHost, command: Command) -> Result<Value> {
    host.commands.send(command)?;
    let Event::Navigation { hierarchy, .. } =
        event(host, |event| matches!(event, Event::Navigation { .. }))?
    else {
        unreachable!()
    };
    Ok(final_path(&hierarchy))
}

fn invoke(endpoint: &Path, token: &str, args: &[&str]) -> Result<Output> {
    Ok(Process::new(RUNTIME)
        .env_clear()
        .env("MATOU_CONTROL_ENDPOINT", endpoint)
        .env("MATOU_CONTROL_TOKEN", token)
        .arg("--internal-mt")
        .args(args)
        .output()?)
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
}

fn assert_reply(output: Output, path: &Value) -> Result<()> {
    assert_success(&output);
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?,
        json!({"kind": "navigated", "finalPath": path}),
    );
    Ok(())
}

fn assert_fault(output: Output, status: i32, code: &str) -> Result<()> {
    assert_eq!(
        output.status.code(),
        Some(status),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr)?["code"],
        code
    );
    Ok(())
}

fn observe_navigation(host: &RuntimeHost) -> Result<Value> {
    let Event::Navigation {
        hierarchy, graph, ..
    } = event(host, |event| matches!(event, Event::Navigation { .. }))?
    else {
        unreachable!()
    };
    let path = final_path(&hierarchy);
    assert_eq!(graph["sceneId"], path["sceneId"]);
    assert_eq!(graph["focusedSessionId"], path["sessionId"]);
    event(
        host,
        |event| matches!(event, Event::Graph(projection) if projection == &graph),
    )?;
    Ok(path)
}

fn persisted_session(db: &Connection) -> Result<String> {
    Ok(db.query_row(
        "SELECT scene.active_session_id FROM window_navigation AS nav
         JOIN window_workspace_focus AS workspace
           ON workspace.window_id = nav.window_id AND workspace.workspace_id = nav.active_workspace_id
         JOIN window_task_focus AS task
           ON task.window_id = nav.window_id AND task.task_id = workspace.active_task_id
         JOIN window_scene_focus AS scene
           ON scene.window_id = nav.window_id AND scene.scene_id = task.active_scene_id
         WHERE nav.window_id = ?",
        [WINDOW],
        |row| row.get(0),
    )?)
}

fn id<'a>(path: &'a Value, field: &str) -> &'a str {
    path[field].as_str().unwrap()
}

fn identity(home: &Path, session: &Value) -> Result<String> {
    let path = home.join(format!("identity.{}", session.as_str().unwrap()));
    let deadline = Instant::now() + WAIT;
    loop {
        match fs::read_to_string(&path) {
            Ok(token) => return Ok(token.trim_end().to_owned()),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error).context("wait for isolated managed shell identity"),
        }
    }
}

#[test]
fn cli_navigates_on_live_owner_and_publishes_each_complete_path() -> Result<()> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join(".git").exists())
        .context("locate short worktree socket path")?;
    let isolated = tempfile::Builder::new()
        .prefix(".tmp-nav-")
        .tempdir_in(repo)?;
    let root = isolated.path();
    let data = root.join("data");
    let home = root.join("home");
    let workspace = root.join("workspace");
    let second_workspace = root.join("second-workspace");
    for dir in [&home, &workspace, &second_workspace] {
        fs::create_dir(dir)?;
    }
    let shell = root.join("shell");
    fs::write(
        &shell,
        r#"#!/bin/sh
umask 077
session=$MATOU_CONTROL_CALLER_SESSION
printf '%s\n' "$MATOU_CONTROL_TOKEN" > "$HOME/identity.$session.tmp"
/bin/mv "$HOME/identity.$session.tmp" "$HOME/identity.$session"
while [ ! -f "$HOME/exit.$session" ]; do /bin/sleep 0.05; done
"#,
    )?;
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
    let mut host = RuntimeHost::start_with_offline_fixtures(
        LaunchOptions {
            data_root: data.clone(),
            workspace,
            shell_home: home.clone(),
            shell,
        },
        OfflineFixtures::default(),
    )?;
    let Event::Ready { hierarchy, .. } =
        event(&host, |event| matches!(event, Event::Ready { .. }))?
    else {
        unreachable!()
    };
    let original = final_path(&hierarchy);
    let token = identity(&home, &original["sessionId"])?;
    let endpoint = data.join("control/runtime.sock");
    let db =
        Connection::open_with_flags(data.join("matou.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let first = navigate(
        &host,
        Command::CreateWorkspace {
            root_directory: second_workspace.to_string_lossy().into_owned(),
        },
    )?;
    let first_canvas = navigate(
        &host,
        Command::CreateTask {
            workspace_id: id(&first, "workspaceId").into(),
        },
    )?;
    let second_canvas = navigate(
        &host,
        Command::CreateCanvas {
            task_id: id(&first_canvas, "taskId").into(),
        },
    )?;
    let saved = navigate(
        &host,
        Command::CreateShellSibling {
            scene_id: id(&second_canvas, "sceneId").into(),
            source_session_id: id(&second_canvas, "sessionId").into(),
            parent_session_id: None,
        },
    )?;
    assert_ne!(saved["taskId"], first["taskId"]);
    assert_ne!(saved["sceneId"], first_canvas["sceneId"]);
    assert_ne!(saved["sessionId"], second_canvas["sessionId"]);
    assert_eq!(
        navigate(
            &host,
            Command::FocusSession {
                scene_id: id(&original, "sceneId").into(),
                session_id: id(&original, "sessionId").into(),
            }
        )?,
        original
    );

    let workspace_ref = format!("workspace:{}", id(&saved, "workspaceId"));
    let task_ref = format!("task:{}", id(&saved, "taskId"));
    let canvas_ref = format!("canvas:{}", id(&first_canvas, "sceneId"));
    let session_ref = format!("session:{}", id(&saved, "sessionId"));
    for (args, path) in [
        (
            vec!["switch", "workspace", &workspace_ref, "--json"],
            &saved,
        ),
        (vec!["switch", "task", "self", "--json"], &original),
        (vec!["switch", "task", &task_ref, "--json"], &saved),
        (
            vec!["switch", "canvas", &canvas_ref, "--json"],
            &first_canvas,
        ),
        (vec!["focus", &session_ref, "--json"], &saved),
        (vec!["switch", "canvas", "current", "--json"], &original),
    ] {
        assert_reply(invoke(&endpoint, &token, &args)?, path)?;
        assert_eq!(&observe_navigation(&host)?, path);
        assert_eq!(persisted_session(&db)?, path["sessionId"]);
    }

    let output = invoke(&endpoint, &token, &["focus", "sibling:1"])?;
    assert_success(&output);
    let text = String::from_utf8(output.stdout)?;
    assert!(text.starts_with("已切换到：窗口 1 / 工作空间 "));
    assert!(text.contains(" / 事项 ") && text.contains(" / 画布 ") && text.contains(" / 会话 1（"));
    assert_eq!(observe_navigation(&host)?, original);

    let saved_token = identity(&home, &saved["sessionId"])?;
    let output = invoke(&endpoint, &saved_token, &["identify", "--json"])?;
    assert_success(&output);
    let caller: Value = serde_json::from_slice(&output.stdout)?;
    let direction = if caller["target"]["session"]["ordinal"] == 1 {
        "right"
    } else {
        "left"
    };
    assert_reply(
        invoke(&endpoint, &saved_token, &["focus", direction, "--json"])?,
        &second_canvas,
    )?;
    assert_eq!(observe_navigation(&host)?, second_canvas);
    assert_reply(
        invoke(&endpoint, &token, &["focus", &session_ref, "--json"])?,
        &saved,
    )?;
    assert_eq!(observe_navigation(&host)?, saved);

    // Two concurrent connections must each receive their own committed path,
    // and the owner publishes complete navigation in the same order it commits.
    let (one, two) = thread::scope(|scope| -> Result<_> {
        let one = scope.spawn(|| invoke(&endpoint, &token, &["focus", &session_ref, "--json"]));
        let two =
            scope.spawn(|| invoke(&endpoint, &token, &["switch", "task", "current", "--json"]));
        Ok((one.join().unwrap()?, two.join().unwrap()?))
    })?;
    assert_reply(one, &saved)?;
    assert_reply(two, &original)?;
    let first_event = observe_navigation(&host)?;
    let last_event = observe_navigation(&host)?;
    assert!(
        first_event == saved && last_event == original
            || first_event == original && last_event == saved
    );
    assert_eq!(persisted_session(&db)?, last_event["sessionId"]);

    navigate(
        &host,
        Command::CloseScene {
            scene_id: id(&first_canvas, "sceneId").into(),
            confirmed: true,
        },
    )?;
    assert!(db.query_row(
        "SELECT archived_at IS NOT NULL FROM scenes WHERE id = ?",
        [id(&first_canvas, "sceneId")],
        |row| row.get::<_, bool>(0)
    )?);
    let archived_session_ref = format!("session:{}", id(&first_canvas, "sessionId"));
    let before = persisted_session(&db)?;
    for args in [
        vec!["focus", "session:does-not-exist", "--json"],
        vec!["focus", &archived_session_ref, "--json"],
        vec!["focus", &task_ref, "--json"],
        vec!["switch", "canvas", &canvas_ref, "--json"],
    ] {
        assert_fault(invoke(&endpoint, &token, &args)?, 3, "TARGET_NOT_FOUND")?;
    }
    for args in [
        vec!["switch", "session", "self", "--json"],
        vec!["focus", "self", "--json", "--json"],
        vec!["focus", "self", "--json", "--", "--invalid"],
    ] {
        assert_fault(invoke(&endpoint, &token, &args)?, 2, "INVALID_REQUEST")?;
    }
    assert_eq!(persisted_session(&db)?, before);

    fs::write(
        home.join(format!("exit.{}", id(&original, "sessionId"))),
        b"",
    )?;
    let deadline = Instant::now() + WAIT;
    while !db.query_row(
        "SELECT EXISTS (SELECT 1 FROM session_runs WHERE session_id = ? AND status = 'exited')",
        [id(&original, "sessionId")],
        |row| row.get::<_, bool>(0),
    )? {
        ensure!(
            Instant::now() < deadline,
            "managed run token was not revoked"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_fault(
        invoke(&endpoint, &token, &["focus", "self", "--json"])?,
        4,
        "CAPABILITY_DENIED",
    )?;
    assert_eq!(persisted_session(&db)?, before);
    host.shutdown()?;
    assert!(!endpoint.exists());
    Ok(())
}
