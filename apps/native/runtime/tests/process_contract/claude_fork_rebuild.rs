use super::*;
use matou_native_runtime::host::RecoveryState;
use rusqlite::params;
use serde_json::json;

const PARENT: &str = "f1f4132a-f6f8-4b49-9f6d-4b07be37f5c2";
const CHILD: &str = "7ec4132a-f6f8-4b49-9f6d-4b07be37f5c2";
const MISSING: &str = "provider session not found";

#[test]
fn claude_fork_rebuild_process_contract() -> Result<()> {
    for (case, manual, submitted, child_file, rebuild_failure) in [
        ("automatic direct rebuild", false, false, false, None),
        ("manual direct rebuild", true, false, false, None),
        (
            "parent appears during ordinary retry",
            true,
            false,
            false,
            None,
        ),
        ("parent absent", false, false, false, None),
        ("has prompt", false, true, false, None),
        (
            "has transcript in another project",
            false,
            false,
            true,
            None,
        ),
        (
            "parent disappeared after preflight",
            false,
            false,
            false,
            Some(format!("No conversation found with session ID: {PARENT}")),
        ),
        (
            "child ID already in use",
            false,
            false,
            false,
            Some(format!("Error: Session ID {CHILD} is already in use.")),
        ),
        ("silent exit", false, false, false, None),
    ] {
        println!("Check Claude fork restore: {case}");
        let root = tempfile::tempdir()?;
        let no_shell = root.path().join("missing-shell");
        let mut seed = RuntimeChild::spawn_configured(root.path(), "", &no_shell, None)?;
        seed.ready()?;
        seed.send(Command::Shutdown)?;
        seed.closed()?;
        let (status, stderr) = seed.wait()?;
        ensure!(status.success(), "seed runtime shutdown: {stderr}");
        fs::write(
            seed.data.join("provider-config.json"),
            json!({
                "version": 1,
                "providers": {"claude-code": [{
                    "id": "fixture", "model": "claude-sonnet-fixture", "reasoningEffort": "low",
                    "endpoint": "http://127.0.0.1:9", "apiKey": "fixture-key"
                }]},
                "activeProviderIds": {"claude-code": "fixture"}
            })
            .to_string(),
        )?;
        let db = Connection::open(seed.data.join("matou.sqlite"))?;
        db.execute(
            "UPDATE sessions SET kind='claude-code' WHERE id=?",
            [&seed.session],
        )?;
        db.execute(
            "INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, cwd,
                                   created_at, updated_at, last_activity_at)
             SELECT 'parent-card', task_id, execution_context_id, kind, 'created', 'Parent', cwd,
                    created_at, updated_at, last_activity_at FROM sessions WHERE id=?",
            [&seed.session],
        )?;
        db.execute(
            "INSERT INTO session_fork_intents (session_id, source_session_id, source_provider,
                                         source_provider_session_id, state, stage, created_at)
             VALUES (?, 'parent-card', 'claude-code', ?, 'succeeded', 'succeeded', 1)",
            params![seed.session, PARENT],
        )?;
        let state = if manual { "failed" } else { "none" };
        db.execute(
            "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id,
                 resume_state, restore_state, restore_error, metadata_json, created_at, updated_at, validated_at)
             VALUES ('saved-binding', ?, 'claude-code', ?, ?, ?, ?, ?, 1, 1, 1)",
            params![seed.session, CHILD, if manual { "failed" } else { "available" }, state,
                manual.then_some(MISSING), json!({
                    "providerConfigId": "fixture", "model": "claude-sonnet-fixture", "permissionMode": "plan",
                    "observedUserPrompt": submitted, "lastHookEvent": "SessionStart",
                }).to_string()],
        )?;
        let parent_absent = case == "parent absent";
        let late_parent = case == "parent appears during ordinary retry";
        let projects = root.path().join("Shell 家目录/claude/projects");
        let parent = projects.join("parent-project");
        let parent_file = parent.join(format!("{PARENT}.jsonl"));
        if !parent_absent && !late_parent {
            fs::create_dir_all(&parent)?;
            fs::write(&parent_file, b"synthetic parent transcript")?;
        }
        if child_file {
            let other = root
                .path()
                .join("Shell 家目录/claude/projects/other-project");
            fs::create_dir_all(&other)?;
            fs::write(
                other.join(format!("{CHILD}.jsonl")),
                b"unreadable Claude data",
            )?;
        }
        let stub = root.path().join("claude-rebuild");
        fs::write(
            &stub,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$PWD|$CLAUDE_CONFIG_DIR|$MATOU_TEST_REBUILD_MARKER" >> "$0.env"
case "$*" in
  *--fork-session*)
    printf '%s\n' "$@" > "$0.rebuild-argv"
    printf 'rebuild\n' >> "$0.calls"
    if [ "$MATOU_TEST_REBUILD_FAILURE" = exit-only ]; then
      exit 42
    fi
    if [ -n "$MATOU_TEST_REBUILD_FAILURE" ]; then
      printf '%s\n' "$MATOU_TEST_REBUILD_FAILURE" >&2
      exit 1
    fi
    printf 'rebuilt child terminal\n'
    exec /bin/sleep 60
    ;;
  *)
    printf '%s\n' "$@" > "$0.resume-argv"
    printf 'resume\n' >> "$0.calls"
    if [ -n "$MATOU_TEST_WAIT_FOR_PARENT" ]; then
      attempts=0
      until [ -f "$MATOU_TEST_WAIT_FOR_PARENT" ]; do
        [ "$attempts" -lt 200 ] || exit 43
        attempts=$((attempts + 1))
        /bin/sleep 0.02
      done
    fi
    printf 'No conversation found with session ID: {CHILD}\n' >&2
    exit 1
    ;;
esac
"#
            ),
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let silent_exit = case == "silent exit";
        let mut child = RuntimeChild::spawn_configured_with(
            root.path(),
            "",
            &no_shell,
            Some(&stub),
            |command| {
                command.env(
                    "MATOU_TEST_REBUILD_FAILURE",
                    if silent_exit {
                        "exit-only"
                    } else {
                        rebuild_failure.as_deref().unwrap_or("")
                    },
                );
                command.env("MATOU_TEST_REBUILD_MARKER", "same launch environment");
                if late_parent {
                    command.env("MATOU_TEST_WAIT_FOR_PARENT", &parent_file);
                }
            },
        )?;
        child.connect()?;
        assert_eq!(child.session, seed.session);
        if manual {
            child.send(Command::RetryProviderRestore {
                session_id: child.session.clone(),
            })?;
        }
        if late_parent {
            let deadline = Instant::now() + TIMEOUT;
            while !stub.with_extension("resume-argv").is_file() {
                ensure!(Instant::now() < deadline, "ordinary retry did not start");
                thread::sleep(Duration::from_millis(20));
            }
            fs::create_dir_all(&parent)?;
            fs::write(&parent_file, b"synthetic parent transcript")?;
        }

        let rebuilt = !submitted && !child_file && !parent_absent;
        let parent_error = format!("No conversation found with session ID: {PARENT}");
        let failure = if silent_exit {
            Some("provider fork rebuild failed: provider resume exited with code 42")
        } else if parent_absent {
            Some(parent_error.as_str())
        } else if rebuilt {
            rebuild_failure.as_deref()
        } else {
            Some(MISSING)
        };
        if let Some(expected) = failure {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                if let Event::Recovery {
                    session_id,
                    state: RecoveryState::Failed(actual),
                } = child
                    .event(deadline, true)
                    .with_context(|| format!("{case}: wait for restore failure"))?
                    && session_id == child.session
                {
                    assert_eq!(actual, expected, "{case}");
                    break;
                }
            }
            let persisted: (String, String) = db.query_row(
                "SELECT restore_state, restore_error FROM provider_bindings WHERE id='saved-binding'",
                [], |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(persisted, ("failed".into(), expected.into()));
        }
        if rebuilt {
            let deadline = Instant::now() + TIMEOUT;
            let argv_path = stub.with_extension("rebuild-argv");
            let argv = loop {
                if let Ok(argv) = fs::read_to_string(&argv_path)
                    && argv.ends_with("plan\n")
                {
                    break argv;
                }
                ensure!(
                    Instant::now() < deadline,
                    "{case}: replacement CLI never started; calls={:?}",
                    fs::read_to_string(stub.with_extension("calls"))
                );
                thread::sleep(Duration::from_millis(20));
            };
            let argv = argv.lines().collect::<Vec<_>>();
            assert_eq!(
                &argv[2..],
                [
                    "--model",
                    "claude-sonnet-fixture",
                    "--effort",
                    "low",
                    "--resume",
                    PARENT,
                    "--fork-session",
                    "--session-id",
                    CHILD,
                    "--permission-mode",
                    "plan"
                ]
            );
            if failure.is_none() {
                post_claude_identity(Path::new(argv[1]), CHILD, "plan", "claude-sonnet-fixture")?;
                let mut confirmed = false;
                let mut rendered = false;
                let deadline = Instant::now() + TIMEOUT;
                while !confirmed || !rendered {
                    match child.event(deadline, true).with_context(|| {
                        format!("{case}: wait for child identity and terminal (confirmed={confirmed}, rendered={rendered})")
                    })? {
                        Event::Graph(graph) => {
                            confirmed |= graph["providerIdentityEvents"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .any(|event| {
                                    event["sessionId"] == child.session
                                        && event["providerSessionId"] == CHILD
                                        && event["result"] == "confirmed"
                                });
                        }
                        Event::Frame {
                            session_id, screen, ..
                        } if session_id == child.session => {
                            rendered |= screen_has_line(&screen, "rebuilt child terminal");
                        }
                        Event::Diagnostic(message) => bail!("{case}: {message}"),
                        _ => {}
                    }
                }
                let binding: (String, String, String) = db.query_row(
                    "SELECT id, provider_session_id, restore_state FROM provider_bindings WHERE session_id=?",
                    [&child.session], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                assert_eq!(
                    binding,
                    ("saved-binding".into(), CHILD.into(), "none".into())
                );
            }
            println!("{case} replacement argv: {:?}", &argv[2..]);
        }
        child.send(Command::Shutdown)?;
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "runtime shutdown: {stderr}");
        if silent_exit {
            let mut restarted =
                RuntimeChild::spawn_configured(root.path(), "", &no_shell, Some(&stub))?;
            restarted.connect()?;
            restarted.send(Command::Refresh)?;
            let deadline = Instant::now() + TIMEOUT;
            loop {
                if let Event::Graph(graph) = restarted.event(deadline, true)?
                    && graph["nodes"].as_array().into_iter().flatten().any(|node| {
                        node["sessionId"] == restarted.session
                            && node["providerRestoreState"] == "failed"
                    })
                {
                    break;
                }
            }
            restarted.send(Command::Shutdown)?;
            restarted.closed()?;
            let (status, stderr) = restarted.wait()?;
            ensure!(status.success(), "runtime restart shutdown: {stderr}");
        }
        let expected = if late_parent {
            "resume\nrebuild\n"
        } else if rebuilt {
            "rebuild\n"
        } else {
            "resume\n"
        };
        let calls = fs::read_to_string(stub.with_extension("calls"))?;
        assert_eq!(calls, expected, "{case}");
        assert_eq!(
            stub.with_extension("resume-argv").is_file(),
            !rebuilt || late_parent,
            "{case}: ordinary child resume argv"
        );
        if late_parent {
            let env = fs::read_to_string(stub.with_extension("env"))?;
            let env = env.lines().collect::<Vec<_>>();
            assert_eq!(env.len(), 2);
            assert_eq!(env[0], env[1]);
        }
        println!(
            "{case}: launches={:?}; failure={failure:?}",
            calls.lines().collect::<Vec<_>>()
        );
    }
    Ok(())
}
