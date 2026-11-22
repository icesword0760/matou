use super::{codex_launch::fixture_with_events, post_claude_hook_url as post_hook_url, *};
use matou_native_core::sessions::list_runs;
use serde_json::{Value, json};

const UUID: &str = "01a1ddc1-88fd-7fc1-8c66-ab8d38719da8";
const FOREIGN: &str = "01a1ddc1-e832-7cf2-83b7-5adb2f12224b";

fn payload(event: &str, turn: Option<&str>, rollout: &Path) -> Value {
    let mut value = json!({
        "hook_event_name": event,
        "session_id": UUID,
        "transcript_path": rollout,
    });
    if let Some(turn) = turn {
        value["turn_id"] = json!(turn);
    }
    value
}

pub(super) fn emit(child: &mut RuntimeChild, home: &Path, name: &str, hook: Value) -> Result<()> {
    fs::write(
        home.join(format!("{name}.json")),
        serde_json::to_vec(&hook)?,
    )?;
    child.input(&format!("hook {name}"))
}

pub(super) fn node<'a>(graph: &'a Value, session: &str) -> Option<&'a Value> {
    graph["nodes"]
        .as_array()?
        .iter()
        .find(|node| node["sessionId"] == session)
}

fn graph_matching(
    child: &mut RuntimeChild,
    description: &str,
    mut matches: impl FnMut(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + TIMEOUT;
    let mut last = Value::Null;
    loop {
        match child
            .event(deadline, true)
            .with_context(|| format!("waiting for {description}; last graph: {last}"))?
        {
            Event::Graph(graph) | Event::Navigation { graph, .. } => {
                if matches(&graph) {
                    return Ok(graph);
                }
                last = graph;
            }
            Event::Diagnostic(error) | Event::NavigationFailed { message: error, .. } => {
                bail!("{error}")
            }
            _ => {}
        }
    }
}

fn state(child: &mut RuntimeChild, expected: &str, notice: Option<(&str, &str)>) -> Result<Value> {
    let session = child.session.clone();
    let scene = child.scene.clone();
    let graph = graph_matching(child, expected, |graph| {
        node(graph, &session).is_some_and(|node| node["workStatus"] == expected)
            && notice.is_none_or(|(kind, _)| graph["agentNotifications"][0]["eventType"] == kind)
    })?;
    let current = node(&graph, &session).unwrap();
    assert_eq!(current["currentMode"], "codex");
    assert_eq!(current["sceneId"], scene);
    let notifications = graph["agentNotifications"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if let Some((kind, body)) = notice {
        assert_eq!(notifications.len(), 1);
        let entry = &notifications[0];
        assert_eq!(entry["eventType"], kind);
        assert_eq!(entry["title"], "Codex");
        assert_eq!(entry["body"], body);
        assert_eq!(entry["sessionId"], session);
        assert_eq!(entry["sceneId"], scene);
    } else {
        assert!(notifications.is_empty(), "{notifications:?}");
    }
    println!(
        "{expected} {} graph card and notification {notice:?}",
        current["currentMode"]
    );
    Ok(graph)
}

fn work_status(db: &Connection, session: &str) -> Result<String> {
    Ok(db.query_row(
        "SELECT work_status FROM sessions WHERE id=?",
        [session],
        |row| row.get(0),
    )?)
}

fn binding_count(db: &Connection) -> Result<i64> {
    Ok(db.query_row(
        "SELECT COUNT(*) FROM provider_bindings WHERE provider = 'codex'",
        [],
        |row| row.get(0),
    )?)
}

fn persisted_effects(
    db: &Connection,
    session: &str,
    run: &str,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut statements = db.prepare(
        "SELECT event_type, payload_json FROM domain_events WHERE session_id=?
         AND (event_type = 'agent.notification'
              OR (event_type = 'session.graph-summary-changed'
                  AND json_extract(payload_json, '$.workStatus') IS NOT NULL))
         ORDER BY seq",
    )?;
    let mut states = Vec::new();
    let mut notices = Vec::new();
    for row in statements.query_map([session], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (kind, raw) = row?;
        let value: Value = serde_json::from_str(&raw)?;
        if kind == "agent.notification" {
            assert_eq!(value["runId"], run);
            assert_eq!(value["provider"], "codex");
            assert_eq!(value["targetSessionId"], session);
            notices.push(
                value["event"]["eventType"]
                    .as_str()
                    .context("notification kind")?
                    .to_owned(),
            );
        } else {
            states.push(
                value["workStatus"]
                    .as_str()
                    .context("work state")?
                    .to_owned(),
            );
        }
    }
    Ok((states, notices))
}

fn hook_url(data: &Path) -> Result<String> {
    let path = fs::read_dir(data.join("provider-hooks"))?
        .next()
        .context("run hook script")??
        .path();
    let script = fs::read_to_string(path)?;
    script
        .split('\'')
        .find(|text| text.starts_with("http://127.0.0.1:"))
        .map(str::to_owned)
        .context("run-scoped loopback URL")
}

pub(super) fn shutdown(mut child: RuntimeChild) -> Result<()> {
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    Ok(())
}

fn start(child: &mut RuntimeChild) -> Result<()> {
    child.input("codex")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.event(deadline, true)? {
            Event::Frame {
                session_id, screen, ..
            } if session_id == child.session && screen_has_line(&screen, "FIXTURE CODEX TUI") => {
                break;
            }
            Event::Diagnostic(error) => bail!("{error}"),
            _ => {}
        }
    }
    child.send(Command::Refresh)?;
    state(child, "starting", None)?;
    Ok(())
}

pub(super) fn transcript(home: &Path, id: &str) -> Result<PathBuf> {
    let rollout = home
        .join("sessions")
        .join(format!("rollout-fixture-{id}.jsonl"));
    fs::create_dir_all(rollout.parent().unwrap())?;
    fs::write(&rollout, "")?;
    Ok(rollout)
}

fn append(rollout: &Path, event: &str, turn: &str, error: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new().append(true).open(rollout)?;
    writeln!(
        file,
        "{}",
        json!({
            "type": "event_msg", "payload": {"type": event, "turn_id": turn, "error": {"message": error}}
        })
    )?;
    Ok(())
}

#[test]
fn codex_events_shell_promotion_late_identity_approval_and_normal_stop() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home) = fixture_with_events(root.path(), "Codex fixture", None, true)?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    let shell = list_runs(&db, &child.session)?[0].clone();
    start(&mut child)?;
    let runs = list_runs(&db, &child.session)?;
    let run = &runs[1];
    assert_eq!(runs[0].id, shell.id);
    assert_eq!(runs[0].status, "exited");
    assert_eq!(
        (run.profile.as_str(), run.status.as_str()),
        ("codex", "running")
    );
    assert_ne!(shell.pid, run.pid);
    assert_eq!(binding_count(&db)?, 0, "bytes alone cannot bind");

    let rollout = transcript(&home, UUID)?;
    for (name, hook) in [
        ("end-before-start", payload("SessionEnd", None, &rollout)),
        (
            "prompt-before-start",
            payload("UserPromptSubmit", Some("early"), &rollout),
        ),
        (
            "missing-file",
            payload("SessionStart", None, &home.join("missing.jsonl")),
        ),
        (
            "no-transcript",
            json!({"hook_event_name": "SessionStart", "session_id": FOREIGN, "transcript_path": null}),
        ),
        (
            "uuid-path-mismatch",
            json!({"hook_event_name": "SessionStart", "session_id": FOREIGN, "transcript_path": rollout}),
        ),
    ] {
        emit(&mut child, &home, name, hook)?;
    }
    child.frame(|_, screen, _| screen_has_line(screen, "HOOK SENT <uuid-path-mismatch>"))?;
    assert_eq!(binding_count(&db)?, 0, "untrusted hooks cannot bind");
    assert_eq!(work_status(&db, &child.session)?, "starting");
    let url = hook_url(&child.data)?;
    let (prefix, _) = url.rsplit_once('/').unwrap();
    assert_eq!(
        post_hook_url(
            &format!("{prefix}/wrong-token"),
            payload("SessionStart", None, &rollout)
        )?,
        404
    );
    let mut started = payload("SessionStart", None, &rollout);
    started["model"] = json!("fixture-observed-model");
    started.as_object_mut().unwrap().remove("session_id"); // UUID from rollout filename
    emit(&mut child, &home, "late-start", started.clone())?;
    let graph = state(&mut child, "idle", None)?;
    let identity = &graph["providerIdentityEvents"][0];
    assert_eq!(identity["provider"], "codex");
    assert_eq!(identity["providerSessionId"], UUID);
    assert_eq!(identity["runId"], run.id);
    assert_eq!(
        node(&graph, &child.session).unwrap()["historyProviderSessionId"],
        UUID
    );
    assert_eq!(
        node(&graph, &child.session).unwrap()["model"],
        "fixture-observed-model"
    );
    let binding: (String, String, Option<i64>, Value) = db.query_row(
        "SELECT provider_session_id, resume_state, validated_at, metadata_json
         FROM provider_bindings WHERE session_id=? AND provider='codex'",
        [&child.session],
        |row| {
            let raw: String = row.get(3)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                serde_json::from_str(&raw).unwrap(),
            ))
        },
    )?;
    assert_eq!(binding.0, UUID);
    assert_eq!(binding.1, "available");
    assert!(binding.2.is_some_and(|time| time >= run.started_at));
    assert_eq!(binding.3["lastHookEvent"], "SessionStart");
    assert!(
        !binding
            .3
            .to_string()
            .contains(&rollout.display().to_string())
    );
    println!(
        "late verified SessionStart: UUID fallback, available binding, validated_at, unchanged card and distinct run"
    );

    let submit = payload("UserPromptSubmit", Some("turn-normal"), &rollout);
    emit(&mut child, &home, "normal-prompt", submit.clone())?;
    state(&mut child, "running", None)?;
    emit(&mut child, &home, "replay-start", started)?;
    let mut foreign = payload("Stop", Some("turn-normal"), &rollout);
    foreign["session_id"] = json!(FOREIGN);
    emit(&mut child, &home, "foreign-stop", foreign)?;
    let mut bypass = payload("Stop", Some("turn-normal"), &rollout);
    bypass["transcript_path"] = Value::Null;
    emit(&mut child, &home, "bypass-stop", bypass)?;
    emit(
        &mut child,
        &home,
        "wrong-turn-stop",
        payload("Stop", Some("wrong-turn"), &rollout),
    )?;
    let mut stop = payload("Stop", Some("turn-normal"), &rollout);
    stop["last_assistant_message"] = json!("Normal answer");
    emit(&mut child, &home, "normal-stop", stop.clone())?;
    state(&mut child, "idle", Some(("completed", "Normal answer")))?;
    emit(&mut child, &home, "duplicate-stop", stop)?;
    emit(&mut child, &home, "old-prompt", submit)?;

    emit(
        &mut child,
        &home,
        "approval-prompt",
        payload("UserPromptSubmit", Some("turn-approval"), &rollout),
    )?;
    state(&mut child, "running", None)?;
    let mut pre = payload("PreToolUse", Some("turn-approval"), &rollout);
    pre["tool_use_id"] = json!("call-a");
    emit(&mut child, &home, "before-tool", pre.clone())?;
    let mut permission = payload("PermissionRequest", Some("turn-approval"), &rollout);
    permission["tool_name"] = json!("Bash");
    permission["tool_input"] = json!({"command": "echo allowed"});
    emit(&mut child, &home, "approval", permission.clone())?;
    state(
        &mut child,
        "needs-input",
        Some(("permission", "Bash: echo allowed")),
    )?;
    emit(&mut child, &home, "replay-before-tool", pre)?;
    emit(&mut child, &home, "duplicate-approval", permission)?;
    let mut post = payload("PostToolUse", Some("turn-approval"), &rollout);
    post["tool_use_id"] = json!("call-a");
    post["exit_code"] = json!(1);
    emit(&mut child, &home, "after-tool", post)?;
    state(&mut child, "running", None)?;
    let mut stop = payload("Stop", Some("turn-approval"), &rollout);
    stop["result"] = json!("Approval turn answer");
    emit(&mut child, &home, "approved-stop", stop)?;
    state(
        &mut child,
        "idle",
        Some(("completed", "Approval turn answer")),
    )?;
    emit(
        &mut child,
        &home,
        "end",
        payload("SessionEnd", None, &rollout),
    )?;
    state(&mut child, "exited", None)?;
    assert_eq!(binding_count(&db)?, 1);
    assert_eq!(
        persisted_effects(&db, &child.session, &run.id)?,
        (
            [
                "running",
                "idle",
                "running",
                "needs-input",
                "running",
                "idle",
                "exited"
            ]
            .map(str::to_owned)
            .to_vec(),
            ["completed", "permission", "completed"]
                .map(str::to_owned)
                .to_vec()
        )
    );
    println!(
        "duplicates, wrong token, wrong turn, wrong session and non-transcript bypass add zero writes/notifications"
    );
    shutdown(child)
}

#[test]
fn codex_events_exact_turn_error_and_visible_esc() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home) = fixture_with_events(root.path(), "Codex fixture", None, true)?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    let rollout = transcript(&home, UUID)?;
    append(&rollout, "task_complete", "old-turn", "Old failure")?;
    append(&rollout, "tool_result", "live-turn", "Exit 1")?;
    start(&mut child)?;
    let run = list_runs(&db, &child.session)?[1].id.clone();
    emit(
        &mut child,
        &home,
        "start",
        payload("SessionStart", None, &rollout),
    )?;
    state(&mut child, "idle", None)?;
    emit(
        &mut child,
        &home,
        "live-prompt",
        payload("UserPromptSubmit", Some("live-turn"), &rollout),
    )?;
    state(&mut child, "running", None)?;
    child.input("noise")?;
    child.frame(|_, screen, _| screen_has_line(screen, "model error: Server timed out"))?;
    assert_eq!(work_status(&db, &child.session)?, "running");
    append(
        &rollout,
        "task_complete",
        "live-turn",
        "Server\n timed out.",
    )?;
    let graph = state(&mut child, "error", Some(("error", "Server timed out.")))?;
    assert_eq!(
        graph["agentNotifications"][0]["replacementKey"],
        "codex-turn-error:live-turn"
    );
    append(
        &rollout,
        "task_complete",
        "live-turn",
        "Duplicate exact error",
    )?;
    emit(
        &mut child,
        &home,
        "error-stop",
        payload("Stop", Some("live-turn"), &rollout),
    )?;
    emit(
        &mut child,
        &home,
        "error-stop-duplicate",
        payload("Stop", Some("live-turn"), &rollout),
    )?;

    emit(
        &mut child,
        &home,
        "esc-prompt",
        payload("UserPromptSubmit", Some("esc-turn"), &rollout),
    )?;
    state(&mut child, "running", None)?;
    child.input("interrupt")?;
    let deadline = Instant::now() + TIMEOUT;
    let mut saw_banner = false;
    let mut saw_graph = false;
    while !saw_banner || !saw_graph {
        match child.event(deadline, true)? {
            Event::Frame {
                screen, session_id, ..
            } if session_id == child.session => {
                saw_banner |= screen_has_line(&screen, "■ Conversation interrupted");
            }
            Event::Graph(graph) => {
                saw_graph |= node(&graph, &child.session)
                    .is_some_and(|node| node["workStatus"] == "interrupted");
                assert!(graph["agentNotifications"].is_null(), "{graph}");
            }
            Event::Diagnostic(error) => bail!("{error}"),
            _ => {}
        }
    }
    child.input("interrupt")?;
    emit(
        &mut child,
        &home,
        "esc-stop",
        payload("Stop", Some("esc-turn"), &rollout),
    )?;
    emit(
        &mut child,
        &home,
        "flush-prompt",
        payload("UserPromptSubmit", Some("flush-turn"), &rollout),
    )?;
    state(&mut child, "running", None)?;
    let flush = payload("Stop", Some("flush-turn"), &rollout);
    append(&rollout, "task_complete", "flush-turn", "Final error")?;
    emit(&mut child, &home, "flush-stop", flush)?;
    state(&mut child, "error", Some(("error", "Final error")))?;
    assert_eq!(
        persisted_effects(&db, &child.session, &run)?,
        (
            [
                "running",
                "error",
                "running",
                "interrupted",
                "running",
                "error"
            ]
            .map(str::to_owned)
            .to_vec(),
            ["error", "error"].map(str::to_owned).to_vec()
        )
    );
    println!(
        "old errors, tool exit, OSC and lookalike text ignored; current JSONL error observed pre-Stop; split ANSI Esc banner shown once per turn"
    );
    shutdown(child)
}

#[test]
fn codex_events_background_delivery_and_retired_token() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home) = fixture_with_events(root.path(), "Codex fixture", None, true)?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    let rollout = transcript(&home, UUID)?;
    start(&mut child)?;
    let session = child.session.clone();
    let scene = child.scene.clone();
    let run = list_runs(&db, &session)?[1].id.clone();
    let url = hook_url(&child.data)?;
    emit(
        &mut child,
        &home,
        "start",
        payload("SessionStart", None, &rollout),
    )?;
    state(&mut child, "idle", None)?;
    emit(
        &mut child,
        &home,
        "prompt",
        payload("UserPromptSubmit", Some("background"), &rollout),
    )?;
    state(&mut child, "running", None)?;

    child.send(Command::CreateCanvas {
        task_id: child.task.clone(),
    })?;
    let graph = graph_matching(&mut child, "another tab focused", |graph| {
        graph["sceneId"].as_str().is_some_and(|id| id != scene)
            && graph["focusedSessionId"].is_string()
    })?;
    child.scene = graph["sceneId"].as_str().unwrap().to_owned();
    child.session = graph["focusedSessionId"].as_str().unwrap().to_owned();
    let other_before = work_status(&db, &child.session)?;
    let mut late_stop = payload("Stop", Some("background"), &rollout);
    late_stop["result"] = json!("Background answer");
    assert_eq!(post_hook_url(&url, late_stop)?, 200);
    let graph = graph_matching(&mut child, "background notification", |graph| {
        graph["agentNotifications"][0]["body"] == "Background answer"
    })?;
    assert_eq!(graph["sceneId"], child.scene);
    assert_eq!(graph["focusedSessionId"], child.session);
    let entry = &graph["agentNotifications"][0];
    assert_eq!(entry["sceneId"], scene);
    assert_eq!(entry["sessionId"], session);
    assert_eq!(entry["eventType"], "completed");
    assert_eq!(work_status(&db, &child.session)?, other_before);
    assert_eq!(work_status(&db, &session)?, "idle");
    println!(
        "while another tab is focused, delayed Stop updates its original run/card and routes notification to its original canvas"
    );

    child.send(Command::SelectScene {
        scene_id: scene.clone(),
    })?;
    graph_matching(&mut child, "original canvas idle", |graph| {
        graph["sceneId"] == scene
            && graph["focusedSessionId"] == session
            && node(graph, &session).is_some_and(|node| node["workStatus"] == "idle")
    })?;
    child.send(Command::Input {
        session_id: session.clone(),
        bytes: b"quit\r".to_vec(),
        defer_ordering: false,
    })?;
    graph_matching(&mut child, "original run exited", |graph| {
        node(graph, &session).is_some_and(|node| node["workStatus"] == "exited")
    })?;
    assert_eq!(list_runs(&db, &session)?[1].status, "exited");

    child.send(Command::SelectScene {
        scene_id: child.scene.clone(),
    })?;
    let other_scene = child.scene.clone();
    graph_matching(&mut child, "second canvas focused", |graph| {
        graph["sceneId"] == other_scene
    })?;
    start(&mut child)?;
    let second_run = list_runs(&db, &child.session)?[1].id.clone();
    assert_ne!(second_run, run);
    let second_rollout = transcript(&home, FOREIGN)?;
    let mut attempted = payload("SessionStart", None, &second_rollout);
    attempted["session_id"] = json!(FOREIGN);
    let status = post_hook_url(&url, attempted.clone())?;
    assert!(matches!(status, 200 | 404), "{status}");
    assert_eq!(binding_count(&db)?, 1);
    assert_eq!(work_status(&db, &child.session)?, "starting");
    emit(&mut child, &home, "second-start", attempted)?;
    let graph = state(&mut child, "idle", None)?;
    let accepted = graph["providerIdentityEvents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["result"] == "confirmed")
        .collect::<Vec<_>>();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["runId"], second_run);
    assert_eq!(accepted[0]["providerSessionId"], FOREIGN);
    assert_eq!(binding_count(&db)?, 2);
    assert_eq!(persisted_effects(&db, &session, &run)?.1, ["completed"]);
    assert!(
        persisted_effects(&db, &child.session, &second_run)?
            .1
            .is_empty()
    );
    println!("retired first token returns {status} without claiming or updating the second run");
    shutdown(child)
}

#[test]
fn codex_events_session_start_cannot_bind_an_early_exit() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home) = fixture_with_events(root.path(), "Codex fixture", Some(23), true)?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    let rollout = transcript(&home, UUID)?;
    fs::write(
        home.join("startup-hook.json"),
        serde_json::to_vec(&payload("SessionStart", None, &rollout))?,
    )?;
    child.input("codex")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Diagnostic(message) = child.event(deadline, true).with_context(|| {
            format!(
                "early exit: startup hook sent={}, runs={:?}",
                home.join("startup-hook.sent").exists(),
                list_runs(&db, &child.session).map(|runs| runs
                    .iter()
                    .map(|run| { (run.profile.clone(), run.status.clone(), run.exit_code) })
                    .collect::<Vec<_>>())
            )
        })? {
            assert!(message.contains("process exited with code 23"), "{message}");
            break;
        }
    }
    child.frame(|_, screen, _| screen_has_line(screen, PROMPT))?;
    let runs = list_runs(&db, &child.session)?;
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[1].profile, "codex");
    assert_eq!(runs[1].exit_code, Some(23));
    assert_eq!(
        (runs[2].profile.as_str(), runs[2].status.as_str()),
        ("shell", "running")
    );
    assert_eq!(binding_count(&db)?, 0);
    assert!(home.join("startup-hook.sent").exists());
    println!(
        "early exit 23 after a real startup hook POST: shell restored, no binding or notification"
    );
    shutdown(child)
}
