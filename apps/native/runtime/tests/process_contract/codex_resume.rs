use super::{
    codex_events::{emit, node, shutdown, transcript},
    post_claude_hook_url as post_hook, *,
};
use matou_native_core::sessions::{SessionRun, codex_binding, list_runs};
use serde_json::{Value, json};

const UUID: &str = "01a1ddc1-88fd-7fc1-8c66-ab8d38719da8";
const FOREIGN: &str = "01a1ddc1-e832-7cf2-83b7-5adb2f12224b";
const MISSING: &str = "provider session not found";

fn spawn(root: &Path) -> Result<RuntimeChild> {
    RuntimeChild::spawn_configured_with(
        root,
        &format!("PROMPT='{PROMPT} '\n"),
        Path::new("/bin/zsh"),
        None,
        |command| {
            command
                .env("CODEX_HOME", root.join("isolated Codex home"))
                .env("MATOU_CODEX_COMMAND", root.join("fixture Codex"))
                .env("MATOU_LOCALE", "zh-CN")
                .env("CODEX_FIXTURE_EVENTS", "1")
                .env("CODEX_FIXTURE_RESUME", "1")
                .env_remove("CODEX_FIXTURE_EXIT_CODE")
                .env_remove("OPENAI_API_KEY");
        },
    )
}

fn material(home: &Path, id: &str) -> Result<PathBuf> {
    let path = transcript(home, id)?;
    fs::write(
        &path,
        format!(
            "{}\n",
            json!({"type":"session_meta", "payload": {"id": id}})
        ),
    )?;
    Ok(path)
}

fn started(id: &str, path: &Path) -> Value {
    json!({"hook_event_name": "SessionStart", "sessionId": id, "transcript_path": path})
}

fn current_hook_url(home: &Path) -> Result<String> {
    let argv = fs::read_to_string(home.join("argv"))?;
    let (_, command) = argv.split_once("command = ").context("Codex hook config")?;
    let command = serde_json::Deserializer::from_str(command)
        .into_iter::<String>()
        .next()
        .context("hook command")??;
    let script = fs::read_to_string(command.trim_matches('\''))?;
    script
        .split('\'')
        .find(|part| part.starts_with("http://127.0.0.1:"))
        .map(str::to_owned)
        .context("current run hook URL")
}

fn open_data(child: &RuntimeChild) -> Result<Connection> {
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    Ok(db)
}

fn establish(root: &Path) -> Result<(RuntimeChild, PathBuf, PathBuf, Connection)> {
    let home = root.join("isolated Codex home");
    fs::create_dir(&home)?;
    let fixture = root.join("fixture Codex");
    fs::write(&fixture, include_bytes!("../fixtures/codex-launch.sh"))?;
    fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700))?;
    let rollout = material(&home, UUID)?;
    let mut child = spawn(root)?;
    child.hello()?;
    child.input("codex --dangerously-bypass-approvals-and-sandbox")?;
    child.frame(|_, screen, _| screen_has_line(screen, "FIXTURE CODEX TUI"))?;
    // The old Shell is gone, and the promotion has stopped buffering input.
    child.send(Command::Refresh)?;
    graph(&mut child, "promoted", |_, card| {
        card["currentMode"] == "codex"
    })?;
    let url = current_hook_url(&home)?;
    assert_eq!(post_hook(&url, started(UUID, &rollout))?, 200);
    confirmed(&mut child, UUID)?;
    let db = open_data(&child)?;
    Ok((child, home, rollout, db))
}

fn graph(
    child: &mut RuntimeChild,
    description: &str,
    mut accept: impl FnMut(&Value, &Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + TIMEOUT;
    let mut last = Value::Null;
    loop {
        match child
            .event(deadline, true)
            .with_context(|| format!("waiting for {description}; last graph {last}"))?
        {
            Event::Graph(graph) | Event::Navigation { graph, .. } => {
                if let Some(card) = node(&graph, &child.session)
                    && accept(&graph, card)
                {
                    assert_eq!(card["sceneId"], child.scene);
                    assert_eq!(card["currentMode"], "codex");
                    let cwd = card["cwd"].as_str().context("Codex graph cwd")?;
                    assert_eq!(
                        Path::new(cwd).canonicalize()?,
                        child.workspace.canonicalize()?
                    );
                    return Ok(graph);
                }
                last = graph;
            }
            Event::Recovery { state, .. } => println!("{description}: recovery={state:?}"),
            Event::Diagnostic(message) => println!("{description}: diagnostic={message}"),
            Event::Closed => bail!("runtime closed while waiting for {description}"),
            _ => {}
        }
    }
}

fn confirmed(child: &mut RuntimeChild, id: &str) -> Result<Value> {
    graph(child, "confirmed identity", |graph, card| {
        card["workStatus"] == "idle"
            && card["providerRestoreState"] == "none"
            && graph["providerIdentityEvents"]
                .as_array()
                .is_some_and(|events| {
                    events.iter().any(|event| {
                        event["result"] == "confirmed" && event["providerSessionId"] == id
                    })
                })
    })
}

fn failed(child: &mut RuntimeChild, reason: &str) -> Result<()> {
    graph(child, reason, |graph, card| {
        card["providerRestoreState"] == "failed"
            && card["providerRestoreError"] == reason
            && card["workStatus"] == "error"
            && graph["agentNotifications"]
                .as_array()
                .is_some_and(|events| {
                    events.iter().any(|e| {
                        e["operation"] == "upsert"
                            && e["title"] == "Codex 恢复失败"
                            && e["body"] == reason
                    })
                })
    })?;
    Ok(())
}

fn restart(child: &mut RuntimeChild) -> Result<()> {
    child.input("quit")?;
    graph(child, "process exited", |_, card| {
        card["workStatus"] == "exited"
    })?;
    child.send(Command::RestartSession {
        session_id: child.session.clone(),
    })
}

fn first_screen(
    child: &mut RuntimeChild,
    home: &Path,
    expected: Option<&str>,
    bypass: bool,
) -> Result<()> {
    let text = if expected.is_some() {
        "FIXTURE CODEX RESUME"
    } else {
        "FIXTURE CODEX TUI"
    };
    child.frame(|_, screen, _| screen_has_line(screen, text))?;
    let raw = fs::read_to_string(home.join("argv"))?;
    let argv: Vec<_> = raw.split_terminator('\0').collect();
    let suffix = expected.map(|id| ["resume", id]);
    if let Some(suffix) = &suffix {
        assert!(
            argv.ends_with(suffix),
            "{:?}",
            &argv[argv.len().saturating_sub(2)..]
        );
    } else {
        assert!(!argv.contains(&"resume"));
    }
    if bypass {
        assert!(argv.contains(&"--dangerously-bypass-approvals-and-sandbox"));
    } else {
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["--ask-for-approval", "on-request"])
        );
    }
    assert!(argv.contains(&"--dangerously-bypass-hook-trust"));
    let cwd = fs::read_to_string(home.join("cwd"))?;
    assert_eq!(
        Path::new(cwd.trim()).canonicalize()?,
        child.workspace.canonicalize()?
    );
    let env = fs::read_to_string(home.join("environment"))?;
    let codex_home = env.lines().next().context("Codex fixture CODEX_HOME")?;
    assert_eq!(Path::new(codex_home).canonicalize()?, home.canonicalize()?);
    assert_eq!(env.lines().nth(1), Some("xterm-256color"));
    Ok(())
}

fn replacement(db: &Connection, session: &str, previous: &SessionRun) -> Result<SessionRun> {
    let runs = list_runs(db, session)?;
    let new = runs.last().context("new run")?;
    assert_eq!(new.ordinal, previous.ordinal + 1);
    assert_eq!(new.profile, "codex");
    assert_eq!(new.status, "running");
    assert_ne!(new.id, previous.id);
    assert_ne!(new.pid, previous.pid);
    assert_eq!(runs[runs.len() - 2].status, "exited");
    assert_shell_gone(previous.pid.context("old PID")? as u32)?;
    Ok(new.clone())
}

fn retire_and_check(child: RuntimeChild, db: &Connection, session: &str) -> Result<SessionRun> {
    let run = list_runs(db, session)?.pop().context("existing run")?;
    shutdown(child)?;
    assert_shell_gone(run.pid.context("old PID")? as u32)?;
    Ok(run)
}

#[test]
fn codex_resume_cold_start_preserves_card_binding_and_legacy_material() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (child, home, rollout, db) = establish(root.path())?;
    let (session, scene) = (child.session.clone(), child.scene.clone());
    let binding = codex_binding(&db, &session)?.unwrap();
    let old = retire_and_check(child, &db, &session)?;
    // Old binding and independent rollout predate later lineage metadata.
    fs::write(
        root.path().join("数据 空间").join("provider-config.json"),
        serde_json::to_vec(&json!({
            "version": 1,
            "activeProviderIds": {"codex": "globally-selected"},
            "providers": {"codex": [
                {"id": "original-provider", "model": "configuration-model", "reasoningEffort": "high",
                 "endpoint": "http://127.0.0.1:4319/v1/", "apiKey":"synthetic-bound"},
                {"id": "globally-selected", "model": "global-model", "reasoningEffort": "low",
                 "endpoint": "http://127.0.0.1:4320/v1/", "apiKey":"synthetic-global"},
            ]}
        }))?,
    )?;
    db.execute(
        "UPDATE provider_bindings SET resume_state = 'resumed', metadata_json = ? WHERE id = ?",
        rusqlite::params![
            json!({
                "permissionMode": "bypassPermissions", "providerConfigId": "original-provider",
                "model": "recorded-model",
            })
            .to_string(),
            binding.id
        ],
    )?;
    assert!(!fs::read_to_string(&rollout)?.contains("history_base"));
    let mut child = spawn(root.path())?;
    child.connect()?;
    assert_eq!((&child.session, &child.scene), (&session, &scene));
    first_screen(&mut child, &home, Some(UUID), true)?;
    let raw = fs::read_to_string(home.join("argv"))?;
    let argv: Vec<_> = raw.split_terminator('\0').collect();
    assert!(
        argv.windows(2)
            .any(|args| args == ["--model", "recorded-model"])
    );
    assert!(argv.contains(&"model_reasoning_effort=\"high\""));
    assert!(!argv.contains(&"global-model"));
    let environment = fs::read_to_string(home.join("environment"))?;
    assert_eq!(environment.lines().nth(2), Some("http://127.0.0.1:4319/v1"));
    assert!(
        environment.lines().nth(3) == Some("synthetic-bound"),
        "bound credential selection"
    );
    let new = replacement(&db, &session, &old)?;
    assert_ne!(new.runtime_generation, old.runtime_generation);
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().restore_state,
        "restoring"
    );
    let deadline = Instant::now() + Duration::from_secs(14);
    loop {
        if let Event::Recovery {
            session_id,
            state: matou_native_runtime::host::RecoveryState::Ready,
        } = child.event(deadline, true)?
            && session_id == session
        {
            break;
        }
    }
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().restore_state,
        "restoring"
    );
    println!("identity timeout released the queue slot; late SessionStart still owns the run");
    emit(&mut child, &home, "restored-start", started(UUID, &rollout))?;
    let graph = confirmed(&mut child, UUID)?;
    assert_eq!(
        node(&graph, &session).unwrap()["permissionMode"],
        "bypassPermissions"
    );
    assert_eq!(codex_binding(&db, &session)?.unwrap().id, binding.id);

    emit(
        &mut child,
        &home,
        "prompt",
        json!({
            "hook_event_name": "UserPromptSubmit", "session_id": UUID,
            "transcript_path": rollout, "turn_id":"resumed-turn",
        }),
    )?;
    self::graph(&mut child, "resumed turn running", |_, card| {
        card["workStatus"] == "running"
    })?;
    emit(
        &mut child,
        &home,
        "stop",
        json!({
            "hook_event_name":"Stop", "session_id": UUID, "transcript_path":rollout,
            "turn_id":"resumed-turn", "result":"Retained context fixture",
        }),
    )?;
    self::graph(&mut child, "resumed turn completed", |graph, card| {
        card["workStatus"] == "idle"
            && graph["agentNotifications"][0]["body"] == "Retained context fixture"
    })?;
    println!(
        "cold restart: same session/scene/binding/UUID, prior PID reaped, new generation/run/PID, old data and hook state/notification verified"
    );
    shutdown(child)
}

#[test]
fn codex_resume_missing_and_corrupt_material_then_retry() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let session = child.session.clone();
    let initial = codex_binding(&db, &session)?.unwrap();
    fs::remove_file(&rollout)?;
    restart(&mut child)?;
    failed(&mut child, MISSING)?;
    let missing_run = list_runs(&db, &session)?.pop().unwrap();
    assert_shell_gone(missing_run.pid.unwrap() as u32)?;
    assert_eq!(codex_binding(&db, &session)?.unwrap().id, initial.id);
    assert!(!codex_binding(&db, &session)?.unwrap().resumable);
    println!("missing rollout: expected UUID output parked same binding and retired PTY");

    fs::write(&rollout, "corrupt json\n")?;
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    failed(&mut child, MISSING)?;
    let corrupt_run = list_runs(&db, &session)?.pop().unwrap();
    assert_eq!(corrupt_run.ordinal, missing_run.ordinal + 1);
    assert_shell_gone(corrupt_run.pid.unwrap() as u32)?;
    println!("corrupt rollout: failed again with same binding and one more run");

    material(&home, UUID)?;
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    child.send(Command::StartFreshProviderSession {
        session_id: session.clone(),
    })?;
    graph(&mut child, "duplicate rejected", |graph, _| {
        graph["providerIdentityEvents"]
            .as_array()
            .is_some_and(|events| {
                events.iter().any(|e| {
                    e["result"] == "rejected"
                        && e["reason"] == "restore-action-not-available"
                        && e["error"] == "只有恢复失败的 Codex 会话需要重试"
                })
            })
    })?;
    first_screen(&mut child, &home, Some(UUID), true)?;
    let run = replacement(&db, &session, &corrupt_run)?;
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(UUID, &rollout))?,
        200
    );
    confirmed(&mut child, UUID)?;
    assert_eq!(list_runs(&db, &session)?.last().unwrap().id, run.id);
    assert_eq!(codex_binding(&db, &session)?.unwrap().id, initial.id);
    let dismissals: i64 = db.query_row(
        "SELECT COUNT(*) FROM domain_events WHERE session_id = ? AND event_type = 'agent.notification'
           AND json_extract(payload_json, '$.event.operation') = 'dismiss'",
        [&session], |row| row.get(0),
    )?;
    assert_eq!(dismissals, 2);
    println!(
        "duplicate retry and overlapping fresh action rejected; one additional run; repaired retry confirmed same binding and dismissed failure"
    );
    shutdown(child)
}

#[test]
fn codex_resume_wrong_identity_early_exit_and_fresh_start() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let session = child.session.clone();
    let initial = codex_binding(&db, &session)?.unwrap();
    let old_url = current_hook_url(&home)?;
    child.send(Command::Resize {
        session_id: session.clone(),
        cols: 93,
        rows: 27,
        cell_width_px: 8,
        cell_height_px: 15,
    })?;
    child.frame(|_, screen, _| screen.cols == 93 && screen.rows == 27)?;
    restart(&mut child)?;
    first_screen(&mut child, &home, Some(UUID), true)?;
    let failed_url = current_hook_url(&home)?;
    assert_ne!(old_url, failed_url);
    let foreign = material(&home, FOREIGN)?;
    assert!(matches!(
        post_hook(&old_url, started(FOREIGN, &foreign))?,
        200 | 404
    ));
    assert_eq!(post_hook(&failed_url, started(FOREIGN, &foreign))?, 200);
    failed(
        &mut child,
        "AI 会话返回的上下文与待恢复会话不一致，请重试恢复",
    )?;
    let wrong_run = list_runs(&db, &session)?.pop().unwrap();
    assert_eq!((wrong_run.cols, wrong_run.rows), (93, 27));
    assert_shell_gone(wrong_run.pid.unwrap() as u32)?;
    assert_eq!(codex_binding(&db, &session)?.unwrap().id, initial.id);
    assert!(matches!(
        post_hook(&failed_url, started(UUID, &rollout))?,
        200 | 404
    ));
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().restore_state,
        "failed"
    );
    println!(
        "foreign SessionStart stopped the current run, previous token and late correct UUID cannot replace failed binding"
    );

    fs::write(home.join("background"), "")?;
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    failed(&mut child, "provider session running in background")?;
    let busy_run = list_runs(&db, &session)?.pop().unwrap();
    assert_eq!(busy_run.ordinal, wrong_run.ordinal + 1);
    assert_shell_gone(busy_run.pid.unwrap() as u32)?;
    assert_eq!(codex_binding(&db, &session)?.unwrap().id, initial.id);
    fs::remove_file(home.join("background"))?;
    println!("background occupancy returned the original card to a retryable failure");
    fs::write(home.join("early-exit"), "")?;
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    failed(&mut child, "provider resume exited with code 23")?;
    let exited = list_runs(&db, &session)?.pop().unwrap();
    assert_eq!(exited.ordinal, busy_run.ordinal + 1);
    assert_eq!(exited.exit_code, Some(23));
    println!("early exit 23 recorded and returned to retryable failure");

    child.send(Command::StartFreshProviderSession {
        session_id: session.clone(),
    })?;
    first_screen(&mut child, &home, None, true)?;
    let fresh = replacement(&db, &session, &exited)?;
    assert!(!codex_binding(&db, &session)?.unwrap().resumable);
    assert_eq!(codex_binding(&db, &session)?.unwrap().id, initial.id);
    let fresh_url = current_hook_url(&home)?;
    assert_eq!(post_hook(&fresh_url, started(UUID, &rollout))?, 200);
    graph(&mut child, "fresh run rejected expired UUID", |graph, _| {
        graph["providerIdentityEvents"]
            .as_array()
            .is_some_and(|events| {
                events
                    .iter()
                    .any(|e| e["reason"] == "binding-write-failed" && e["runId"] == fresh.id)
            })
    })?;
    assert!(!codex_binding(&db, &session)?.unwrap().resumable);
    shutdown(child)?;
    let mut child = spawn(root.path())?;
    child.connect()?;
    first_screen(&mut child, &home, None, true)?;
    child.send(Command::Refresh)?;
    graph(
        &mut child,
        "fresh launch waiting for identity",
        |_, card| card["workStatus"] == "starting" && card["providerRestoreState"] == "none",
    )?;
    let fresh = replacement(&db, &session, &fresh)?;
    assert!(!codex_binding(&db, &session)?.unwrap().resumable);
    emit(&mut child, &home, "fresh-start", started(FOREIGN, &foreign))?;
    let graph = confirmed(&mut child, FOREIGN)?;
    assert_eq!(graph["providerIdentityEvents"][0]["runId"], fresh.id);
    let current = codex_binding(&db, &session)?.unwrap();
    assert_ne!(current.id, initial.id);
    assert_eq!(current.provider_session_id, FOREIGN);
    let old_state: String = db.query_row(
        "SELECT resume_state FROM provider_bindings WHERE id = ?",
        [&initial.id],
        |row| row.get(0),
    )?;
    assert_eq!(old_state, "expired");
    println!(
        "new conversation: fresh argv and same card before and after restart, old target expired, new UUID only after SessionStart"
    );
    shutdown(child)
}

#[test]
fn codex_resume_no_binding_stays_retryable_across_restarts() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (child, home, _, db) = establish(root.path())?;
    let session = child.session.clone();
    let old = retire_and_check(child, &db, &session)?;
    db.execute(
        "DELETE FROM provider_bindings WHERE session_id = ?",
        [&session],
    )?;
    let mut child = spawn(root.path())?;
    child.connect()?;
    failed(&mut child, MISSING)?;
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    failed(&mut child, MISSING)?;
    assert_eq!(list_runs(&db, &session)?.last().unwrap().id, old.id);
    assert!(codex_binding(&db, &session)?.is_none());
    shutdown(child)?;

    let mut child = spawn(root.path())?;
    child.connect()?;
    child.send(Command::Refresh)?;
    graph(&mut child, "persisted no-binding failure", |_, card| {
        card["providerRestoreState"] == "failed"
    })?;
    child.send(Command::StartFreshProviderSession {
        session_id: session.clone(),
    })?;
    first_screen(&mut child, &home, None, false)?;
    replacement(&db, &session, &old)?;
    let rollout = material(&home, FOREIGN)?;
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(FOREIGN, &rollout))?,
        200
    );
    confirmed(&mut child, FOREIGN)?;
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().provider_session_id,
        FOREIGN
    );
    println!(
        "no binding: failed and retried without process or invented UUID; after restart, manual fresh start writes first verified binding"
    );
    shutdown(child)
}
