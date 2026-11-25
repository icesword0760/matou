use super::{
    codex_events::{emit, node, shutdown, transcript},
    post_claude_hook_url as post_hook, *,
};
use matou_native_core::{
    dispatch_roles::{DispatchRolesStore, format_roles_markdown},
    sessions::{SessionRun, codex_binding, list_runs},
};
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
            let assets = root.join("control assets");
            if assets.is_dir() {
                command.env("MATOU_CONTROL_ASSET_ROOT", assets);
            }
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
fn codex_hud_projects_current_rollout_and_admitted_tools_through_resume() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let previous = list_runs(&db, &child.session)?
        .pop()
        .context("initial run")?;
    let url = current_hook_url(&home)?;
    let session = child.session.clone();
    let hud = |child: &mut RuntimeChild, name: &str, accept: &dyn Fn(&Value) -> bool| {
        let graph = graph(child, name, |_, card| accept(&card["codexHud"]))?;
        let hud = node(&graph, &session).context("current card")?["codexHud"].clone();
        println!("codex HUD | {name} | {hud}");
        Ok::<_, anyhow::Error>(hud)
    };
    let append = |rows: &[Value]| -> Result<()> {
        let mut file = fs::OpenOptions::new().append(true).open(&rollout)?;
        for row in rows {
            writeln!(file, "{row}")?;
        }
        Ok(())
    };
    let turn = |payload: Value| json!({"type": "turn_context", "payload": payload});
    let tokens = |used: Value, window: Value| {
        json!({"type": "event_msg", "payload": {
            "type": "token_count", "info": {
                "last_token_usage": {"total_tokens": used},
                "total_token_usage": {"total_tokens": 999999},
                "model_context_window": window
            }
        }})
    };
    let command = |id: &str, status: &str, code: i64| {
        json!({"type": "event_msg", "payload": {
            "type": "item_completed", "item": {
                "id": id, "type": "CommandExecution", "status": status, "exit_code": code,
                "command": ["printf", "hud"]
            }
        }})
    };
    let hook = |event: &str, tool_id: Option<&str>| -> Result<()> {
        let mut body = json!({
            "hook_event_name": event, "session_id": UUID,
            "turn_id": "hud-turn", "transcript_path": rollout,
        });
        body["tool_name"] = json!("Bash");
        body["tool_input"] = json!({"command":"echo fixture tool"});
        if let Some(id) = tool_id {
            body["tool_use_id"] = json!(id);
        }
        assert_eq!(post_hook(&url, body)?, 200);
        Ok(())
    };

    child.send(Command::Refresh)?;
    hud(&mut child, "initial empty model, context and tools", &|v| {
        v.is_object()
            && v["model"].is_null()
            && v["contextPercent"].is_null()
            && v["toolCounts"] == json!([])
            && v["runningTools"] == json!([])
    })?;
    append(&[
        turn(
            json!({"model": "fixture-a", "effort": "high", "collaboration_mode": {
                "settings": {"reasoning_effort": "low"}
            }}),
        ),
        tokens(json!(51001), json!(200000)),
        json!({"type":"response_item","payload":{"model":"ignored", "type":"mcp_call"}}),
    ])?;
    let result = hud(
        &mut child,
        "direct effort and rounded last token usage",
        &|v| {
            v["model"] == "fixture-a"
                && v["effort"] == "high"
                && v["contextPercent"].as_f64() == Some(26.)
                && v["contextWindowSize"].as_f64() == Some(200000.)
        },
    )?;
    for key in [
        "account",
        "usageWindows",
        "agents",
        "todos",
        "mcpErrors",
        "teamRole",
        "configCounts",
    ] {
        assert!(result.get(key).is_none(), "{key} should not project");
    }
    println!("codex HUD | Claude-only fields absent from current-run graph");

    writeln!(
        fs::OpenOptions::new().append(true).open(&rollout)?,
        "{{unreadable"
    )?;
    let second = turn(json!({"model": "模型-B", "collaboration_mode": {
        "settings": {"reasoning_effort": "medium"}
    }}))
    .to_string();
    let split = second.find('模').unwrap() + 1;
    fs::OpenOptions::new()
        .append(true)
        .open(&rollout)?
        .write_all(&second.as_bytes()[..split])?;
    thread::sleep(Duration::from_millis(350));
    child.send(Command::Refresh)?;
    hud(
        &mut child,
        "bad line and split UTF-8 retain valid snapshot",
        &|v| v["model"] == "fixture-a" && v["effort"] == "high",
    )?;
    fs::OpenOptions::new()
        .append(true)
        .open(&rollout)?
        .write_all(&second.as_bytes()[split..])?;
    hud(
        &mut child,
        "collaboration effort and completed suffix without newline",
        &|v| v["model"] == "模型-B" && v["effort"] == "medium",
    )?;
    writeln!(fs::OpenOptions::new().append(true).open(&rollout)?)?;
    append(&[
        turn(json!({"model": "fixture-c"})),
        tokens(json!(-1), json!(200000)),
    ])?;
    hud(
        &mut child,
        "absent effort and invalid usage clear both indicators",
        &|v| {
            v["model"] == "fixture-c"
                && v["effort"].is_null()
                && v["contextPercent"].is_null()
                && v["contextWindowSize"].is_null()
        },
    )?;
    for compact in [
        json!({"type":"compacted","payload":{}}),
        json!({"type":"event_msg","payload":{"type":"context_compacted"}}),
    ] {
        append(&[tokens(json!(0), json!(1_200_000))])?;
        hud(&mut child, "valid zero percent", &|v| {
            v["contextPercent"].as_f64() == Some(0.)
        })?;
        append(&[compact.clone()])?;
        hud(&mut child, compact["type"].as_str().unwrap(), &|v| {
            v["contextPercent"].is_null() && v["contextWindowSize"].is_null()
        })?;
    }

    hook("UserPromptSubmit", None)?;
    hook("PermissionRequest", None)?;
    graph(&mut child, "permission without call id", |_, card| {
        card["workStatus"] == "needs-input" && card["codexHud"]["runningTools"] == json!([])
    })?;
    println!("codex HUD | missing call ID creates no phantom running tool");
    hook("PreToolUse", Some("mirrored-call"))?;
    hud(&mut child, "admitted active tool", &|v| {
        v["runningTools"][0]["name"] == "Bash"
            && v["runningTools"][0]["target"] == "echo fixture tool"
            && v["lastTool"]["status"] == "running"
    })?;
    hook("PostToolUse", Some("mirrored-call"))?;
    hook("PostToolUse", Some("mirrored-call"))?;
    hud(&mut child, "duplicate PostToolUse counts once", &|v| {
        v["runningTools"] == json!([]) && v["toolCounts"] == json!([{"name":"Bash","count":1}])
    })?;
    append(&[
        command("mirrored-call", "completed", 0),
        command("ignored-command", "in_progress", 0),
        command("rollout-failure", "failed", 1),
        command("rollout-failure", "completed", 0),
    ])?;
    hud(
        &mut child,
        "merged hook and rollout, failed command keeps error status",
        &|v| {
            v["toolCounts"] == json!([{"name":"Bash","count":2}])
                && v["lastTool"]["status"] == "error"
                && v["lastTool"]["target"] == "printf hud"
        },
    )?;
    hook("PreToolUse", Some("still-running"))?;
    hud(
        &mut child,
        "active tool before rollout-only completion",
        &|v| {
            v["runningTools"]
                .as_array()
                .is_some_and(|items| items.len() == 1)
        },
    )?;
    append(&[command("still-running", "completed", 0)])?;
    hud(
        &mut child,
        "rollout completion removes live tool without PostToolUse",
        &|v| {
            v["runningTools"] == json!([])
                && v["toolCounts"][0]["count"] == 3
                && v["lastTool"]["status"] == "completed"
        },
    )?;

    let detached = rollout.with_extension("detached");
    fs::rename(&rollout, &detached)?;
    hud(
        &mut child,
        "missing admitted file clears rollout-only data",
        &|v| {
            v["model"].is_null()
                && v["effort"].is_null()
                && v["contextPercent"].is_null()
                && v["toolCounts"] == json!([{"name":"Bash","count":1}])
        },
    )?;
    child.input("noise")?;
    child.frame(|_, screen, _| screen_has_line(screen, "model error: Server timed out"))?;
    println!("codex HUD | PTY still renders output with missing rollout");
    fs::rename(&detached, &rollout)?;
    hud(
        &mut child,
        "same admitted file returns and reads from start",
        &|v| v["model"] == "fixture-c" && v["toolCounts"][0]["count"] == 3,
    )?;

    hook("PreToolUse", Some("ending-tool"))?;
    hud(&mut child, "active tool before SessionEnd", &|v| {
        v["lastTool"]["status"] == "running"
    })?;
    hook("SessionEnd", None)?;
    graph(&mut child, "SessionEnd retires HUD reading", |_, card| {
        card["workStatus"] == "exited" && card["codexHud"]["runningTools"] == json!([])
    })?;
    let next = turn(json!({"model": "fixture-reopened", "effort": "low"}));
    fs::write(
        &rollout,
        format!(
            "{}\n{next}\n",
            json!({"type":"session_meta", "payload":{"id":UUID}})
        ),
    )?;
    thread::sleep(Duration::from_millis(400));
    child.send(Command::Refresh)?;
    hud(&mut child, "after SessionEnd late data is ignored", &|v| {
        v["model"] == "fixture-c"
    })?;
    fs::write(
        &rollout,
        format!(
            "{}\n",
            json!({"type":"session_meta", "payload":{"id":UUID}})
        ),
    )?;

    child.input("quit")?;
    graph(
        &mut child,
        "process exit removes HUD from the stopped run",
        |_, card| card["codexHud"].is_null(),
    )?;
    assert_eq!(list_runs(&db, &session)?.last().unwrap().status, "exited");
    assert_shell_gone(previous.pid.context("previous pid")? as u32)?;
    child.send(Command::RestartSession {
        session_id: session.clone(),
    })?;
    first_screen(&mut child, &home, Some(UUID), true)?;
    child.send(Command::Refresh)?;
    hud(
        &mut child,
        "replacement run before new SessionStart has no old HUD",
        &|v| v.is_object() && v["model"].is_null() && v["toolCounts"] == json!([]),
    )?;
    let new_url = current_hook_url(&home)?;
    assert_ne!(new_url, url);
    let mut start = started(UUID, &rollout);
    start["model"] = json!("fixture-hook-model");
    assert_eq!(post_hook(&new_url, start)?, 200);
    confirmed(&mut child, UUID)?;
    child.send(Command::Refresh)?;
    hud(&mut child, "current SessionStart model only", &|v| {
        v["model"] == "fixture-hook-model" && v["effort"].is_null()
    })?;
    let mut file = fs::OpenOptions::new().append(true).open(&rollout)?;
    file.write_all(&vec![b'x'; 1024 * 1024 + 1])?;
    writeln!(file)?;
    append(&[next])?;
    hud(
        &mut child,
        "same UUID resumed with new rollout snapshot and fresh tools",
        &|v| {
            v["model"] == "fixture-reopened"
                && v["effort"] == "low"
                && v["toolCounts"] == json!([])
                && v["contextPercent"].is_null()
        },
    )?;
    replacement(&db, &session, &previous)?;
    shutdown(child)
}

#[test]
fn codex_title_follows_the_active_index_and_preserves_manual_priority() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let session = child.session.clone();
    fs::write(
        &rollout,
        format!(
            "{}\n",
            json!({"type":"session_meta","payload": {
                "id":UUID, "cwd":child.workspace, "timestamp":"2026-11-15T06:12:00Z",
            }})
        ),
    )?;
    let index = home.join("session_index.jsonl");
    let old_url = current_hook_url(&home)?;
    let row = |id: &str, title: &str, updated: &str| {
        json!({"id":id, "thread_name":title, "updated_at":updated}).to_string()
    };
    let send_title = |child: &mut RuntimeChild, title: Option<&str>| {
        child.send(Command::SetSessionTitle {
            session_id: session.clone(),
            title: title.map(str::to_owned),
        })
    };
    let visible = |child: &mut RuntimeChild, title: &str, source: &str, provider: &str| {
        graph(child, title, |_, card| {
            card["title"] == title
                && card["titleSource"] == source
                && card["providerTitle"] == provider
        })
    };
    let updates = || {
        db.query_row(
            "SELECT COUNT(*) FROM domain_events WHERE session_id = ?
             AND event_type = 'session.updated' AND json_type(payload_json, '$.session') = 'object'",
            [&session],
            |row| row.get::<_, i64>(0),
        )
    };
    let receipts = || {
        db.query_row(
            "SELECT COUNT(*) FROM command_deduplication WHERE command_id LIKE 'codex-title-%'",
            [],
            |row| row.get::<_, i64>(0),
        )
    };

    let prompt = "合成首问：整理项目";
    writeln!(
        fs::OpenOptions::new().append(true).open(&rollout)?,
        "{}",
        json!({"type":"event_msg","payload":{"type":"user_message","message":prompt}})
    )?;
    assert_eq!(
        post_hook(
            &old_url,
            json!({
                "hook_event_name": "UserPromptSubmit", "session_id": UUID,
                "transcript_path": rollout, "turn_id": "title-turn", "prompt": prompt,
            })
        )?,
        200
    );
    graph(&mut child, "first prompt keeps the default", |_, card| {
        card["workStatus"] == "running"
            && card["title"] == "Codex"
            && card["providerTitle"].is_null()
    })?;
    println!(
        "codex-title first prompt: rollout and authenticated hook kept default while index missing"
    );

    send_title(&mut child, Some("手动一"))?;
    graph(&mut child, "named before auto", |_, card| {
        card["title"] == "手动一"
            && card["titleSource"] == "manual"
            && card["providerTitle"].is_null()
    })?;
    send_title(&mut child, None)?;
    child.send(Command::Refresh)?;
    graph(
        &mut child,
        "restore before any provider title",
        |_, card| {
            card["title"] == "手动一"
                && card["titleSource"] == "manual"
                && card["providerTitle"].is_null()
        },
    )?;
    println!("codex-title clear without candidate: manual name survives");

    let first = row(UUID, "  自动摘要一  ", "2026-11-15T06:14:00Z");
    let store = root.path().join("index staging");
    fs::write(
        &store,
        [
            first.clone(),
            row(UUID, "旧标题", "2026-11-15T06:13:00Z"),
            row(FOREIGN, "旁路会话", "2026-11-15T06:50:00Z"),
            row(UUID, " ", "2026-11-15T06:50:00Z"),
            row(UUID, "坏时间", "now"),
            "{bad json".into(),
            "null".into(),
            "{\"id\":".into(),
        ]
        .join("\n"),
    )?;
    fs::rename(&store, &index)?;
    visible(&mut child, "手动一", "manual", "自动摘要一")?;
    println!(
        "codex-title delayed index: atomic creation, exact UUID and latest valid time selected under manual lock"
    );
    let count = updates()?;
    let receipt_count = receipts()?;
    writeln!(
        fs::OpenOptions::new().append(true).open(&index)?,
        "\n{first}"
    )?;
    // Give the asynchronous observer several polls before asserting both writes are deduplicated.
    thread::sleep(Duration::from_millis(850));
    assert_eq!(updates()?, count);
    assert_eq!(receipts()?, receipt_count);
    println!("codex-title duplicate/bad/empty rows: no event or command receipt added");
    send_title(&mut child, None)?;
    visible(&mut child, "自动摘要一", "auto", "自动摘要一")?;

    let later = row(UUID, "自动摘要二", "2026-11-15T06:15:00.125Z");
    fs::write(&store, format!("{first}\n{later}\n"))?;
    fs::rename(&store, &index)?;
    visible(&mut child, "自动摘要二", "auto", "自动摘要二")?;
    send_title(&mut child, Some("手动二"))?;
    visible(&mut child, "手动二", "manual", "自动摘要二")?;
    let latest = row(UUID, "自动摘要三", "2026-11-15T06:16:00Z");
    writeln!(
        fs::OpenOptions::new().append(true).open(&index)?,
        "{latest}"
    )?;
    visible(&mut child, "手动二", "manual", "自动摘要三")?;
    let listing = list_catalog(&mut child, 91, "自动摘要三")?;
    let listed = &listing["sessions"][0];
    assert_eq!(listed["providerSessionId"], UUID, "{listing}");
    assert_eq!(listed["title"], "自动摘要三");
    assert_eq!(listed["loadedBy"]["title"], "手动二");
    println!(
        "codex-title clear and update: unlocked card followed a replacement index, then manual lock; catalog shows current provider title and manual card name"
    );

    // The replacement process has rendered its first frame but has not bound its UUID.
    restart(&mut child)?;
    first_screen(&mut child, &home, Some(UUID), true)?;
    let new_url = current_hook_url(&home)?;
    assert_ne!(old_url, new_url);
    let next = row(UUID, "仅新 run 可观察", "2026-11-15T06:17:00Z");
    writeln!(fs::OpenOptions::new().append(true).open(&index)?, "{next}")?;
    assert!(matches!(
        post_hook(&old_url, started(UUID, &rollout))?,
        200 | 404
    ));
    child.send(Command::Refresh)?;
    graph(&mut child, "old run could not write", |_, card| {
        card["providerRestoreState"] == "none"
            && card["workStatus"] == "starting"
            && card["providerTitle"] == "自动摘要三"
    })?;
    assert_eq!(
        matou_native_core::sessions::get_session(&db, &session)?
            .unwrap()
            .provider_title
            .as_deref(),
        Some("自动摘要三")
    );
    assert_eq!(post_hook(&new_url, started(UUID, &rollout))?, 200);
    visible(&mut child, "手动二", "manual", "仅新 run 可观察")?;
    send_title(&mut child, None)?;
    visible(&mut child, "仅新 run 可观察", "auto", "仅新 run 可观察")?;
    println!(
        "codex-title late run: retired hook stayed fenced until replacement UUID confirmation"
    );

    let old = retire_and_check(child, &db, &session)?;
    fs::remove_file(&index)?;
    let mut child = spawn(root.path())?;
    child.connect()?;
    assert_eq!(child.session, session);
    first_screen(&mut child, &home, Some(UUID), true)?;
    let new = replacement(&db, &session, &old)?;
    assert_ne!(new.runtime_generation, old.runtime_generation);
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(UUID, &rollout))?,
        200
    );
    child.send(Command::Refresh)?;
    visible(&mut child, "仅新 run 可观察", "auto", "仅新 run 可观察")?;
    child.input("hook missing-local-fixture")?;
    child.send(Command::Resize {
        session_id: session.clone(),
        cols: 91,
        rows: 27,
        cell_width_px: 8,
        cell_height_px: 15,
    })?;
    child.frame(|_, screen, _| screen.cols == 91 && screen.rows == 27)?;
    println!(
        "codex-title cold reopen: same card and saved title with missing index, terminal resized successfully"
    );
    shutdown(child)
}

#[test]
fn codex_resume_cold_start_preserves_card_binding_and_legacy_material() -> Result<()> {
    let root = tempfile::tempdir()?;
    super::codex_launch::fixture_foreman_assets(root.path())?;
    let (child, home, rollout, db) = establish(root.path())?;
    let (session, scene) = (child.session.clone(), child.scene.clone());
    let binding = codex_binding(&db, &session)?.unwrap();
    let roles = DispatchRolesStore::for_data_root(&child.data);
    let old = retire_and_check(child, &db, &session)?;
    let mut config = roles.snapshot()?.config;
    config.agents[0].role = "重新打开后读取最新职责".into();
    let expected_roles = format_roles_markdown(&roles.save(config)?.config);
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
    let argv = fs::read_to_string(home.join("argv"))?;
    let instructions: String = serde_json::from_str(
        argv.split_terminator('\0')
            .find_map(|arg| arg.strip_prefix("developer_instructions="))
            .context("resumed Codex instructions")?,
    )?;
    assert!(instructions.ends_with(&expected_roles));
    let environment = fs::read_to_string(home.join("dispatch-environment"))?;
    assert_eq!(
        Path::new(environment.lines().next().unwrap()).canonicalize()?,
        roles.path().canonicalize()?,
    );
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
    assert_eq!(codex_binding(&db, &session)?.unwrap().restore_state, "none");
    let deadline = Instant::now() + Duration::from_secs(14);
    let (mut ready, mut timed_out) = (false, false);
    while !ready || !timed_out {
        match child.event(deadline, true)? {
            Event::Recovery {
                session_id,
                state: matou_native_runtime::host::RecoveryState::Ready,
            } if session_id == session => ready = true,
            Event::Graph(graph) => {
                for event in graph["providerIdentityEvents"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if event["result"] == "timed-out"
                        && event["sessionId"] == session
                        && event["runId"] == new.id
                    {
                        assert_eq!(event["provider"], "codex");
                        assert_eq!(event["restoreState"], "none");
                        timed_out = true;
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(codex_binding(&db, &session)?.unwrap().restore_state, "none");
    println!(
        "identity timeout released startup without a banner; late SessionStart still owns the run"
    );
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
fn codex_resume_confirms_without_a_rollout_and_attaches_it_later() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let session = child.session.clone();
    let binding_id = codex_binding(&db, &session)?.unwrap().id;
    let ready = home.join("resume-screen-ready");
    for with_missing_path in [false, true] {
        if ready.exists() {
            fs::remove_file(&ready)?;
        }
        restart(&mut child)?;
        // The terminal replays earlier frames. Wait for this CLI instance
        // to read the file before simulating a transcript that arrives late.
        let deadline = Instant::now() + TIMEOUT;
        while !ready.exists() {
            ensure!(Instant::now() < deadline, "fresh resume screen not ready");
            thread::sleep(Duration::from_millis(10));
        }
        first_screen(&mut child, &home, Some(UUID), true)?;
        let url = current_hook_url(&home)?;
        let foreign = material(&home, FOREIGN)?;
        let outside = root.path().join(rollout.file_name().unwrap());
        fs::copy(&rollout, &outside)?;
        for path in [foreign, outside] {
            assert_eq!(post_hook(&url, started(UUID, &path))?, 200);
        }
        let binding = codex_binding(&db, &session)?.unwrap();
        assert_eq!(binding.restore_state, "none");
        assert_eq!(binding.id, binding_id);
        let work_status: String = db.query_row(
            "SELECT work_status FROM sessions WHERE id = ?",
            [&session],
            |row| row.get(0),
        )?;
        assert_eq!(work_status, "starting");

        let mut start = json!({"hook_event_name": "SessionStart", "session_id": UUID});
        if with_missing_path {
            fs::remove_file(&rollout)?;
            start["transcript_path"] = json!(rollout);
        }
        assert_eq!(post_hook(&url, start)?, 200);
        confirmed(&mut child, UUID)?;
        let binding = codex_binding(&db, &session)?.unwrap();
        assert_eq!(binding.restore_state, "none");
        assert_eq!(binding.id, binding_id);

        let turn = "late-rollout-turn";
        assert_eq!(
            post_hook(
                &url,
                json!({
                    "hook_event_name": "UserPromptSubmit", "session_id": UUID, "turn_id": turn,
                })
            )?,
            200
        );
        graph(
            &mut child,
            "prompt accepted before rollout attachment",
            |_, card| card["workStatus"] == "running",
        )?;
        if with_missing_path {
            material(&home, UUID)?;
        }
        let mut file = fs::OpenOptions::new().append(true).open(&rollout)?;
        writeln!(
            file,
            "{}",
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "task_complete", "turn_id": turn,
                    "error": {"message": "fixture late rollout error"}
                }
            })
        )?;
        graph(
            &mut child,
            "error read from recovered rollout",
            |_, card| card["workStatus"] == "error",
        )?;
    }
    shutdown(child)
}

#[test]
fn codex_resume_silent_timeout_parks_the_card_for_retry() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, _, db) = establish(root.path())?;
    let session = child.session.clone();
    let binding_id = codex_binding(&db, &session)?.unwrap().id;
    fs::write(home.join("silent-resume"), "")?;
    restart(&mut child)?;
    let deadline = Instant::now() + Duration::from_secs(14);
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(card) = node(&graph, &session)
            && card["providerRestoreState"] == "failed"
        {
            assert_eq!(card["workStatus"], "error");
            assert_eq!(
                card["providerRestoreError"],
                "provider resume identity confirmation timed out"
            );
            assert_eq!(card["historyProviderSessionId"], UUID);
            assert!(
                graph["providerIdentityEvents"]
                    .as_array()
                    .is_some_and(|events| events
                        .iter()
                        .any(|event| event["result"] == "restore-failed"))
            );
            break;
        }
    }
    let binding = codex_binding(&db, &session)?.unwrap();
    assert_eq!(binding.restore_state, "failed");
    assert!(!binding.resumable);
    assert_eq!(binding.id, binding_id);
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

fn set_permission(child: &mut RuntimeChild, mode: &str) -> Result<()> {
    child.send(Command::SetProviderPermissionMode {
        session_id: child.session.clone(),
        mode: mode.to_owned(),
    })
}

fn permission_receipt(graph: &Value, mode: &str) -> Option<Value> {
    graph["providerPermissionModeResults"]
        .as_array()?
        .iter()
        .find(|receipt| receipt["permissionMode"] == mode)
        .cloned()
}

fn wait_permission(child: &mut RuntimeChild, mode: &str) -> Result<(Value, Value)> {
    let mut receipt = Value::Null;
    let graph = graph(child, "permission receipt", |graph, _| {
        if let Some(found) = permission_receipt(graph, mode) {
            receipt = found;
            return true;
        }
        false
    })?;
    assert_eq!(receipt["sessionId"], child.session);
    Ok((graph, receipt))
}

fn assert_permission_argv(home: &Path, expected: &[&str]) -> Result<()> {
    let raw = fs::read_to_string(home.join("argv"))?;
    let argv: Vec<_> = raw.split_terminator('\0').collect();
    let hook = argv
        .iter()
        .position(|arg| arg.starts_with("hooks={ "))
        .context("Codex run hooks")?;
    assert_eq!(&argv[..2], ["-c", "check_for_update_on_startup=false"]);
    assert_eq!(argv[hook - 3], "-c");
    assert!(argv[hook - 2].starts_with("developer_instructions="));
    assert!(argv[hook - 2].contains("mt identify --json"));
    assert_eq!(argv[hook - 1], "-c");
    assert_eq!(&argv[2..hook - 3], expected);
    assert_eq!(
        &argv[hook + 1..],
        ["--dangerously-bypass-hook-trust", "resume", UUID]
    );
    assert_eq!(
        fs::read_to_string(home.join("environment"))?.lines().nth(2),
        Some("https://api.openai.com/v1")
    );
    Ok(())
}

#[test]
fn codex_permission_switches_keep_card_uuid_and_retire_each_replaced_run() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let session = child.session.clone();
    let binding = codex_binding(&db, &session)?.unwrap();
    let mut old = list_runs(&db, &session)?.pop().unwrap();
    let mut old_url = current_hook_url(&home)?;
    let config_path = home.join("config.toml");
    fs::write(
        &config_path,
        "approval_policy='never'\nsandbox_mode='danger-full-access'\n",
    )?;
    for invalid in ["auto", "unknown"] {
        set_permission(&mut child, invalid)?;
        let (_, receipt) = wait_permission(&mut child, invalid)?;
        assert_eq!(receipt["error"], "invalid permission mode");
    }
    set_permission(&mut child, "bypassPermissions")?;
    assert!(
        wait_permission(&mut child, "bypassPermissions")?
            .1
            .get("error")
            .is_none()
    );
    assert_eq!(list_runs(&db, &session)?.last().unwrap().id, old.id);
    println!("unsupported Auto and unknown modes rejected; current mode is a no-op");

    for (mode, flags) in [
        (
            "acceptEdits",
            [
                "--ask-for-approval",
                "on-request",
                "--sandbox",
                "workspace-write",
            ]
            .as_slice(),
        ),
        (
            "plan",
            &["--ask-for-approval", "on-request", "--sandbox", "read-only"],
        ),
        (
            "bypassPermissions",
            &["--dangerously-bypass-approvals-and-sandbox"],
        ),
        (
            "default",
            &[
                "--ask-for-approval",
                "untrusted",
                "--sandbox",
                "danger-full-access",
            ],
        ),
    ] {
        if mode == "default" {
            fs::write(
                &config_path,
                "approval_policy='untrusted'\nsandbox_mode='danger-full-access'\n[profiles.test]\nsandbox_mode='read-only'\n",
            )?;
        }
        let config = fs::read(&config_path)?;
        set_permission(&mut child, mode)?;
        set_permission(&mut child, mode)?;
        graph(
            &mut child,
            "permission restart before confirmation",
            |graph, card| {
                assert!(
                    permission_receipt(graph, mode).is_none(),
                    "confirmed before hook"
                );
                card["providerRestoreState"] == "none" && card["workStatus"] == "starting"
            },
        )?;
        child.frame(|_, screen, _| screen_has_line(screen, "FIXTURE CODEX RESUME"))?;
        let cwd = fs::read_to_string(home.join("cwd"))?;
        assert_eq!(
            Path::new(cwd.trim()).canonicalize()?,
            child.workspace.canonicalize()?
        );
        let environment = fs::read_to_string(home.join("environment"))?;
        assert_eq!(
            Path::new(environment.lines().next().unwrap()).canonicalize()?,
            home.canonicalize()?
        );
        assert_eq!(environment.lines().nth(1), Some("xterm-256color"));
        assert_permission_argv(&home, flags)?;
        assert_eq!(fs::read(&config_path)?, config);
        let new = replacement(&db, &session, &old)?;
        let current_url = current_hook_url(&home)?;
        assert_ne!(old_url, current_url);
        assert!(matches!(
            post_hook(&old_url, started(UUID, &rollout))?,
            200 | 404
        ));
        let waiting = codex_binding(&db, &session)?.unwrap();
        assert_eq!(waiting.restore_state, "none");
        assert_eq!(waiting.id, binding.id);
        child.send(Command::Refresh)?;
        graph(
            &mut child,
            "retired hook could not confirm permission",
            |graph, card| {
                assert!(permission_receipt(graph, mode).is_none());
                card["workStatus"] == "starting" && card["providerRestoreState"] == "none"
            },
        )?;
        assert_eq!(post_hook(&current_url, started(UUID, &rollout))?, 200);
        let (graph, receipt) = wait_permission(&mut child, mode)?;
        assert!(receipt.get("error").is_none(), "{receipt}");
        let card = node(&graph, &session).unwrap();
        assert_eq!(card["permissionMode"], mode);
        assert_eq!(card["workStatus"], "idle");
        assert_eq!(card["providerRestoreState"], "none");
        let confirmed = graph["providerIdentityEvents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["result"] == "confirmed" && event["runId"] == new.id)
            .unwrap();
        assert_eq!(confirmed["bindingId"], binding.id);
        assert_eq!(confirmed["providerSessionId"], UUID);
        assert_eq!(codex_binding(&db, &session)?.unwrap().id, binding.id);
        println!(
            "switch {mode}: {flags:?}; same card/scene/binding/UUID, run {} → {}, PID {:?} → {:?}; retired hook cannot confirm",
            old.ordinal, new.ordinal, old.pid, new.pid
        );
        if mode == "default" {
            let deadline = Instant::now() + TIMEOUT;
            while post_hook(&old_url, started(UUID, &rollout))? != 404 {
                ensure!(Instant::now() < deadline, "retired hook still registered");
                thread::sleep(Duration::from_millis(25));
            }
            println!("retired hook endpoint returned HTTP 404 after its existing grace period");
        }
        old = new;
        old_url = current_url;
    }
    assert_eq!(list_runs(&db, &session)?.len(), 6);
    for (event, expected) in [("UserPromptSubmit", "running"), ("Stop", "idle")] {
        emit(
            &mut child,
            &home,
            expected,
            json!({
                "hook_event_name": event, "session_id": UUID, "transcript_path": rollout,
                "turn_id": "turn-after-switch", "result": "Completed after permission switch",
            }),
        )?;
        graph(&mut child, expected, |graph, card| {
            card["workStatus"] == expected
                && (event != "Stop"
                    || graph["agentNotifications"][0]["body"]
                        == "Completed after permission switch")
        })?;
    }
    println!(
        "replacement's actual command hook updated card running → idle and published completion notification"
    );
    shutdown(child)
}

#[test]
fn codex_permission_failed_switch_stays_on_card_and_retries_target_mode() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let session = child.session.clone();
    let binding = codex_binding(&db, &session)?.unwrap();
    let stub = root.path().join("fixture Codex");
    let held_stub = root.path().join("held fixture Codex");
    for (mode, flags, reason) in [
        (
            "plan",
            ["--ask-for-approval", "on-request", "--sandbox", "read-only"],
            "provider resume exited with code 23",
        ),
        (
            "acceptEdits",
            [
                "--ask-for-approval",
                "on-request",
                "--sandbox",
                "workspace-write",
            ],
            "provider process could not start",
        ),
    ] {
        let previous = list_runs(&db, &session)?.pop().unwrap();
        if mode == "plan" {
            fs::write(home.join("early-exit"), "")?;
        } else {
            fs::rename(&stub, &held_stub)?;
        }
        set_permission(&mut child, mode)?;
        let mut receipt = false;
        let mut notice = false;
        graph(&mut child, "permission failure", |graph, card| {
            if let Some(result) = permission_receipt(graph, mode) {
                assert!(result["error"].is_string(), "{result}");
                receipt = true;
            }
            notice |= graph["agentNotifications"]
                .as_array()
                .is_some_and(|events| {
                    events.iter().any(|event| {
                        event["operation"] == "upsert"
                            && event["title"] == "Codex 恢复失败"
                            && event["body"]
                                .as_str()
                                .is_some_and(|body| body.contains(reason))
                    })
                });
            receipt
                && notice
                && card["workStatus"] == "error"
                && card["providerRestoreState"] == "failed"
                && card["providerRestoreError"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(reason)
        })?;
        assert_shell_gone(previous.pid.unwrap() as u32)?;
        let failed = list_runs(&db, &session)?.pop().unwrap();
        if mode == "plan" {
            assert_eq!(failed.exit_code, Some(23));
            assert_eq!(failed.ordinal, previous.ordinal + 1);
            assert_shell_gone(failed.pid.unwrap() as u32)?;
            fs::remove_file(home.join("early-exit"))?;
        } else {
            assert_eq!(failed.id, previous.id);
            fs::rename(&held_stub, &stub)?;
        }
        let persisted = codex_binding(&db, &session)?.unwrap();
        assert_eq!(
            (&persisted.id, persisted.provider_session_id.as_str()),
            (&binding.id, UUID)
        );
        assert_eq!(persisted.metadata["permissionMode"], mode);
        assert!(!persisted.resumable);
        child.send(Command::RetryProviderRestore {
            session_id: session.clone(),
        })?;
        graph(&mut child, "retry dismissed the failure", |graph, card| {
            card["providerRestoreState"] == "restoring"
                && graph["agentNotifications"][0]["operation"] == "dismiss"
        })?;
        first_screen(&mut child, &home, Some(UUID), false)?;
        assert_permission_argv(&home, &flags)?;
        let retried = replacement(&db, &session, &failed)?;
        emit(&mut child, &home, "retry-start", started(UUID, &rollout))?;
        confirmed(&mut child, UUID)?;
        let persisted = codex_binding(&db, &session)?.unwrap();
        assert_eq!(persisted.id, binding.id);
        assert_eq!(persisted.metadata["permissionMode"], mode);
        assert_eq!(persisted.restore_state, "none");
        println!(
            "failed {mode}: {reason}; failure card and notification; retry used {flags:?} in run {} and confirmed the same binding/UUID",
            retried.ordinal
        );
    }
    shutdown(child)
}

#[test]
fn codex_permission_invalid_defaults_and_stored_mode_emit_safe_launch_warnings() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let session = child.session.clone();
    let config = home.join("config.toml");
    let invalid = "approval_policy='untrusted'\nsandbox_mode='private-fixture-value'\n";
    fs::write(&config, invalid)?;
    set_permission(&mut child, "default")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Diagnostic(message) = child.event(deadline, true)? {
            assert_eq!(
                message,
                "Codex 权限配置无法读取或包含无效值，受影响的设置已使用默认值。"
            );
            break;
        }
    }
    child.frame(|_, screen, _| screen_has_line(screen, "FIXTURE CODEX RESUME"))?;
    assert_permission_argv(
        &home,
        &[
            "--ask-for-approval",
            "untrusted",
            "--sandbox",
            "workspace-write",
        ],
    )?;
    assert_eq!(fs::read_to_string(&config)?, invalid);
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(UUID, &rollout))?,
        200
    );
    assert!(
        wait_permission(&mut child, "default")?
            .1
            .get("error")
            .is_none()
    );
    println!(
        "invalid root sandbox left the valid approval in place and published the safe warning"
    );

    fs::write(&config, "[profiles.test]\napproval_policy='never'\n")?;
    assert_eq!(db.execute(
        "UPDATE provider_bindings SET metadata_json=json_set(metadata_json, '$.permissionMode', 'unknown-private-mode')
         WHERE session_id=? AND provider='codex'",
        [&session],
    )?, 1);
    restart(&mut child)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Diagnostic(message) = child.event(deadline, true)? {
            assert_eq!(message, "Codex 权限模式无效，已回退到 Default。");
            break;
        }
    }
    first_screen(&mut child, &home, Some(UUID), false)?;
    assert_permission_argv(
        &home,
        &[
            "--ask-for-approval",
            "on-request",
            "--sandbox",
            "workspace-write",
        ],
    )?;
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(UUID, &rollout))?,
        200
    );
    confirmed(&mut child, UUID)?;
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().metadata["permissionMode"],
        "default"
    );
    println!(
        "unknown stored mode fell back to Default, gave a generic warning and confirmation canonicalized the binding"
    );
    shutdown(child)
}

#[test]
fn codex_permission_requires_session_start_and_times_out_to_retryable_failure() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home, rollout, db) = establish(root.path())?;
    let session = child.session.clone();
    let binding = codex_binding(&db, &session)?.unwrap();
    set_permission(&mut child, "plan")?;
    first_screen(&mut child, &home, Some(UUID), false)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let reason = "provider resume identity confirmation timed out";
    loop {
        match child.event(deadline, true)? {
            Event::Recovery {
                state: matou_native_runtime::host::RecoveryState::Ready,
                ..
            } => {
                bail!("permission switch became ready without a matching hook");
            }
            Event::Graph(graph) if permission_receipt(&graph, "plan").is_some() => {
                assert_eq!(permission_receipt(&graph, "plan").unwrap()["error"], reason);
                assert_eq!(
                    node(&graph, &session).unwrap()["providerRestoreState"],
                    "failed"
                );
                assert_eq!(graph["agentNotifications"][0]["body"], reason);
                break;
            }
            _ => {}
        }
    }
    let failed = list_runs(&db, &session)?.pop().unwrap();
    assert_shell_gone(failed.pid.unwrap() as u32)?;
    assert_eq!(codex_binding(&db, &session)?.unwrap().id, binding.id);
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    first_screen(&mut child, &home, Some(UUID), false)?;
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().restore_state,
        "restoring"
    );
    assert_permission_argv(
        &home,
        &["--ask-for-approval", "on-request", "--sandbox", "read-only"],
    )?;
    let retried = replacement(&db, &session, &failed)?;
    let deadline = Instant::now() + Duration::from_secs(14);
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(ready) = graph["providerIdentityEvents"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|event| event["result"] == "timed-out" && event["runId"] == retried.id)
        {
            assert_eq!(ready["restoreState"], "none");
            break;
        }
    }
    assert_eq!(codex_binding(&db, &session)?.unwrap().restore_state, "none");
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(UUID, &rollout))?,
        200
    );
    confirmed(&mut child, UUID)?;
    assert_eq!(codex_binding(&db, &session)?.unwrap().id, binding.id);
    println!(
        "no SessionStart: confirmation timed out and the same card failed; Plan retry succeeded"
    );
    shutdown(child)
}

fn catalog_fixture(home: &Path, cwd: &Path, id: &str, title: &str) -> Result<PathBuf> {
    use std::io::Write;
    let path = transcript(home, id)?;
    fs::write(
        &path,
        format!(
            "{}\n{}\n",
            json!({"type": "session_meta", "payload": {"id": id, "cwd": cwd}}),
            json!({"type": "response_item", "payload": {
                "type": "message", "role": "user", "content": [{"text": "Find the synthetic needle"}]
            }}),
        ),
    )?;
    let mut titles = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(home.join("session_index.jsonl"))?;
    writeln!(
        titles,
        "{}",
        json!({"id": id, "thread_name": title, "updated_at": "2026-11-20T00:00:00Z"})
    )?;
    Ok(path)
}

fn spawn_catalog_fixture(root: &Path) -> Result<(RuntimeChild, PathBuf)> {
    let home = root.join("isolated Codex home");
    fs::create_dir(&home)?;
    fs::write(
        home.join("config.toml"),
        "approval_policy = \"untrusted\"\nsandbox_mode = \"read-only\"\n",
    )?;
    let fixture = root.join("fixture Codex");
    fs::write(&fixture, include_bytes!("../fixtures/codex-launch.sh"))?;
    fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700))?;
    let mut child = spawn(root)?;
    child.hello()?;
    Ok((child, home))
}

fn catalog_receipt(
    child: &mut RuntimeChild,
    key: &str,
    accepts: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child
            .event(deadline, true)
            .with_context(|| format!("catalog receipt {key}"))?
        {
            Event::Graph(graph) => {
                if let Some(row) = graph[key]
                    .as_array()
                    .and_then(|rows| rows.iter().find(|row| accepts(row)))
                {
                    return Ok(row.clone());
                }
            }
            Event::Diagnostic(message) => println!("catalog diagnostic: {message}"),
            Event::Closed => bail!("runtime closed waiting for {key}"),
            _ => {}
        }
    }
}

fn list_catalog(child: &mut RuntimeChild, request_id: u64, query: &str) -> Result<Value> {
    let cwd = child.workspace.display().to_string();
    child.send(Command::ListCodexSessions {
        request_id,
        cwd,
        query: query.into(),
        full_text: false,
        offset: None,
        limit: Some(1),
    })?;
    catalog_receipt(child, "providerSessionCatalogResults", |row| {
        row["provider"] == "codex" && row["requestId"] == request_id
    })
}

fn load_catalog(
    child: &mut RuntimeChild,
    session_id: &str,
    provider_session_id: &str,
) -> Result<()> {
    child.send(Command::LoadCodexSession {
        session_id: session_id.into(),
        provider_session_id: provider_session_id.into(),
    })
}

fn catalog_sibling(
    child: &mut RuntimeChild,
    db: &Connection,
    session: &str,
) -> Result<(String, SessionRun)> {
    child.send(Command::CreateShellSibling {
        scene_id: child.scene.clone(),
        source_session_id: session.to_owned(),
        parent_session_id: None,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    let sibling = loop {
        if let Event::Navigation { graph, .. } = child.event(deadline, true)?
            && let Some(id) = graph["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|node| {
                    node["sessionId"]
                        .as_str()
                        .filter(|id| *id != session)
                        .map(str::to_owned)
                })
        {
            break id;
        }
    };
    let other = loop {
        let runs = list_runs(db, &sibling)?;
        if let Some(run) = runs.last().filter(|run| run.status == "running") {
            break run.clone();
        }
        ensure!(Instant::now() < deadline, "sibling Shell did not start");
        thread::sleep(Duration::from_millis(20));
    };
    Ok((sibling, other))
}

#[test]
fn codex_catalog_load_replaces_same_card_and_rejects_duplicate_and_occupied() -> Result<()> {
    const ANCESTOR: &str = "01a1ddc1-88fd-7fc1-8c66-ab8d38719daf";
    let root = tempfile::tempdir()?;
    let (mut child, home) = spawn_catalog_fixture(root.path())?;
    let path = catalog_fixture(&home, &child.workspace, UUID, "续聊历史")?;
    let other_worktree = root.path().join("ancestor-worktree");
    fs::create_dir(&other_worktree)?;
    let parent = transcript(&home, ANCESTOR)?;
    fs::write(
        parent,
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":ANCESTOR,"cwd":other_worktree}}),
            json!({"type":"response_item","timestamp":1,"payload":{
                "type":"message","role":"assistant","content":"祖先线索 needle"
            }})
        ),
    )?;
    let transcript = fs::read_to_string(&path)?;
    let (meta, events) = transcript.split_once('\n').context("rollout body")?;
    let mut meta: Value = serde_json::from_str(meta)?;
    meta["payload"]["history_base"] = json!({"thread_id":ANCESTOR});
    fs::write(&path, format!("{meta}\n{events}"))?;
    let session = child.session.clone();
    let initial = list_catalog(&mut child, 10, "续聊")?;
    assert_eq!(initial["total"], 1);
    assert_eq!(initial["sessions"][0]["providerSessionId"], UUID);
    assert!(initial["sessions"][0]["loadedBy"].is_null());
    let cwd = child.workspace.display().to_string();
    child.send(Command::SearchCodexSession {
        request_id: 11,
        cwd: cwd.clone(),
        provider_session_id: UUID.into(),
        query: "NEEDLE".into(),
        offset: None,
        limit: None,
    })?;
    let search = catalog_receipt(&mut child, "providerSessionDetailResults", |row| {
        row["requestId"] == 11
    })?;
    assert_eq!(search["total"], 2);
    assert_eq!(search["hits"][0]["eventIndex"], 1);
    assert_eq!(search["hits"][0]["excerpt"], "祖先线索 needle");
    child.send(Command::GetCodexSession {
        request_id: 12,
        cwd,
        provider_session_id: UUID.into(),
        around_event_index: Some(1),
        start_event_index: None,
        limit: None,
    })?;
    let detail = catalog_receipt(&mut child, "providerSessionDetailResults", |row| {
        row["requestId"] == 12
    })?;
    assert_eq!(detail["providerSessionId"], UUID);
    assert_eq!(detail["events"][0]["text"], "祖先线索 needle");
    assert_eq!(detail["events"][1]["text"], "Find the synthetic needle");
    println!(
        "shared worker routed Codex list, ancestor search and preview for the selected child UUID"
    );

    let db = open_data(&child)?;
    let old = list_runs(&db, &session)?.pop().context("initial shell")?;
    load_catalog(&mut child, &session, UUID)?;
    load_catalog(&mut child, &session, UUID)?;
    let duplicate = catalog_receipt(&mut child, "providerSessionLoadErrors", |row| {
        row["sessionId"] == session
    })?;
    assert!(matches!(
        duplicate["error"].as_str(),
        Some("正在载入当前卡片" | "已载入当前卡片")
    ));
    child.frame(|_, screen, _| screen_has_line(screen, "FIXTURE CODEX RESUME"))?;
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().restore_state,
        "restoring"
    );
    let args = fs::read_to_string(home.join("argv"))?;
    let args: Vec<_> = args.split_terminator('\0').collect();
    assert!(args.ends_with(&["resume", UUID]));
    assert!(
        args.windows(2)
            .any(|args| args == ["--ask-for-approval", "untrusted"])
    );
    assert!(
        args.windows(2)
            .any(|args| args == ["--sandbox", "read-only"])
    );
    let cwd = fs::read_to_string(home.join("cwd"))?;
    assert_eq!(
        Path::new(cwd.trim()).canonicalize()?,
        child.workspace.canonicalize()?
    );
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(UUID, &path))?,
        200
    );
    confirmed(&mut child, UUID)?;
    replacement(&db, &session, &old)?;
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().provider_session_id,
        UUID
    );
    println!(
        "same card kept original scene and loaded the same UUID; default permissions, cwd and first screen verified; old PID retired"
    );

    load_catalog(&mut child, &session, UUID)?;
    let duplicate = catalog_receipt(&mut child, "providerSessionLoadErrors", |row| {
        row["sessionId"] == session
    })?;
    assert_eq!(duplicate["error"], "已载入当前卡片");
    let occupied = list_catalog(&mut child, 13, "续聊")?;
    assert_eq!(occupied["sessions"][0]["loadedBy"]["sessionId"], session);
    assert_eq!(occupied["sessions"][0]["loadedBy"]["title"], "续聊历史");

    let (sibling, other) = catalog_sibling(&mut child, &db, &session)?;
    load_catalog(&mut child, &sibling, UUID)?;
    let elsewhere = catalog_receipt(&mut child, "providerSessionLoadErrors", |row| {
        row["sessionId"] == sibling
    })?;
    assert_eq!(elsewhere["error"], "已载入“续聊历史”");
    assert_eq!(list_runs(&db, &session)?.len(), 2);
    assert_eq!(list_runs(&db, &sibling)?.last().unwrap().id, other.id);
    assert!(codex_binding(&db, &sibling)?.is_none());
    println!(
        "pending and confirmed repeat rejected; refreshed list marks owner; occupied sibling's Shell untouched"
    );

    let new_path = catalog_fixture(&home, &child.workspace, FOREIGN, "另一段历史")?;
    load_catalog(&mut child, &sibling, FOREIGN)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Frame {
            session_id, screen, ..
        } = child.event(deadline, true)?
            && session_id == sibling
            && screen_has_line(&screen, "FIXTURE CODEX RESUME")
        {
            break;
        }
    }
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(FOREIGN, &new_path))?,
        200
    );
    let identity = catalog_receipt(&mut child, "providerIdentityEvents", |row| {
        row["sessionId"] == sibling && row["result"] == "confirmed"
    })?;
    assert_eq!(identity["providerSessionId"], FOREIGN);
    replacement(&db, &sibling, &other)?;
    assert_eq!(
        codex_binding(&db, &sibling)?.unwrap().provider_session_id,
        FOREIGN
    );
    println!("fresh sibling card uses the same loader; retained both distinct UUIDs");
    shutdown(child)?;
    Ok(())
}

#[test]
fn codex_catalog_missing_selection_and_failed_respawn_recover_without_new_uuid() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, home) = spawn_catalog_fixture(root.path())?;
    let session = child.session.clone();
    let db = open_data(&child)?;
    let old = list_runs(&db, &session)?.pop().context("initial shell")?;
    load_catalog(&mut child, &session, UUID)?;
    let missing = catalog_receipt(&mut child, "providerSessionLoadErrors", |row| {
        row["sessionId"] == session
    })?;
    assert_eq!(
        missing["error"],
        "Provider session was not found in this working directory"
    );
    assert_eq!(list_runs(&db, &session)?.last().unwrap().id, old.id);
    assert!(codex_binding(&db, &session)?.is_none());
    println!(
        "missing selection rejected before changing the current process or creating a binding"
    );
    let path = catalog_fixture(&home, &child.workspace, UUID, "可重试历史")?;
    fs::write(home.join("early-exit"), "")?;
    load_catalog(&mut child, &session, UUID)?;
    let receipt = catalog_receipt(&mut child, "providerIdentityEvents", |row| {
        row["sessionId"] == session && row["result"] == "restore-failed"
    })?;
    assert_eq!(receipt["providerSessionId"], UUID);
    assert!(
        receipt["restoreError"]
            .as_str()
            .is_some_and(|error| error.contains("23")),
        "{receipt}"
    );
    let failed_binding = codex_binding(&db, &session)?.unwrap();
    assert_eq!(failed_binding.restore_state, "failed");
    assert_eq!(failed_binding.provider_session_id, UUID);
    assert_shell_gone(old.pid.context("old PID")? as u32)?;
    fs::remove_file(home.join("early-exit"))?;
    fs::write(home.join("config.toml"), "")?;
    // A competing loader can claim the released identity after a failure.
    let (sibling, _) = catalog_sibling(&mut child, &db, &session)?;
    load_catalog(&mut child, &sibling, UUID)?;
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    let rejected = catalog_receipt(&mut child, "providerIdentityEvents", |row| {
        row["sessionId"] == session && row["reason"] == "catalog-identity-occupied"
    })?;
    assert_eq!(rejected["result"], "rejected");
    assert_eq!(
        codex_binding(&db, &session)?.unwrap().restore_state,
        "failed"
    );
    child.send(Command::CloseSessionCard {
        scene_id: child.scene.clone(),
        session_id: sibling.clone(),
        confirmed_intent: Some(format!("delete-session:{sibling}")),
        preserve_scene_on_last_session: true,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::ScenesDisposed(cards) = child.event(deadline, true)?
            && cards.contains(&sibling)
        {
            break;
        }
    }
    println!("failed card could not retry while a different card claimed the same UUID");
    child.send(Command::RetryProviderRestore {
        session_id: session.clone(),
    })?;
    first_screen(&mut child, &home, Some(UUID), false)?;
    assert_eq!(
        post_hook(&current_hook_url(&home)?, started(UUID, &path))?,
        200
    );
    confirmed(&mut child, UUID)?;
    let recovered = codex_binding(&db, &session)?.unwrap();
    assert_eq!(recovered.id, failed_binding.id);
    assert_eq!(recovered.provider_session_id, UUID);
    assert_eq!(recovered.restore_state, "none");
    println!(
        "early exit 23 used the shared failure state; explicit Retry confirmed the same binding and UUID"
    );
    shutdown(child)?;
    Ok(())
}
