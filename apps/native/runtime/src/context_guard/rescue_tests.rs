use super::*;

use matou_native_core::{
    context_optimizer_config::AgentOptimizerSettings,
    journal::JournalOptions,
    startup::{self, BootstrapResult},
};
use matou_native_terminal::TerminalOptions;
use std::{fs, os::unix::fs::PermissionsExt, process::Command, thread};

use crate::{
    context_guard::store::ContextGuardStore,
    host::{hierarchy_ids, start_context_rescue},
    host_control::{RescueDelivery, RescuePrompt},
};

const SOURCE: &str = "source-session";
const WINDOW: &str = "rescue-window";
const GENERATION: &str = "rescue-test-generation";

struct Fixture {
    root: tempfile::TempDir,
    cwd: PathBuf,
    data: PathBuf,
    executable: PathBuf,
    owner: ShellOwner,
}

impl Fixture {
    fn new(profile: LaunchProfile) -> Result<Self> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let cwd = root.path().join("worktree");
        let data = root.path().join("data");
        fs::create_dir_all(&cwd)?;
        fs::create_dir_all(&data)?;
        ensure!(
            Command::new("git")
                .current_dir(&cwd)
                .args(["init", "-q"])
                .status()?
                .success(),
            "fixture git initialization failed"
        );
        let executable = root.path().join("provider-fixture");
        fs::write(
            &executable,
            r#"#!/bin/sh
set -e
/bin/stty raw -echo
printf '%s\n%s\n%s\n' "$MATOU_DATA_ROOT" "$MATOU_SESSION_ID" "$PWD" > "$CAPTURE_FILE.env"
printf '\033[?2004hREADY\r\n'
exec /bin/cat > "$CAPTURE_FILE"
"#,
        )?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(&data, GENERATION)?
        else {
            anyhow::bail!("fixture storage was not writable");
        };
        let mut ids = hierarchy_ids();
        ids.session_id = SOURCE.into();
        hierarchy::bootstrap_window(
            &mut connection,
            &command(
                "rescue-bootstrap".into(),
                "window.bootstrap",
                "fixture".into(),
            ),
            &hierarchy::BootstrapWindowInput {
                window_id: WINDOW.into(),
                default_root_directory: cwd.to_string_lossy().into_owned(),
                default_name: "Handoff workspace".into(),
                now: 1,
            },
            &ids,
            "fixture",
            1,
        )?;
        connection.execute(
            "UPDATE sessions SET kind = ?, title = 'Original card' WHERE id = ?",
            params![profile.kind(), SOURCE],
        )?;
        let owner = ShellOwner::new(connection, ownership, || Uuid::new_v4().to_string())?;
        Ok(Self {
            root,
            cwd,
            data,
            executable,
            owner,
        })
    }

    fn launch(&mut self, session: &str, profile: LaunchProfile) -> Result<PathBuf> {
        let capture = self.root.path().join(format!("capture-{session}"));
        let home = self.root.path().join("home");
        fs::create_dir_all(&home)?;
        self.owner.launch_run(
            SessionOptions {
                session_id: session.into(),
                launch: ShellLaunch {
                    profile,
                    executable: self.executable.clone(),
                    argv: vec!["fixture".into(), "fixture".into()],
                    cwd: self.cwd.clone(),
                    data_root: self.data.clone(),
                    inherited_env: Default::default(),
                    env: [
                        ("HOME".into(), home.clone().into_os_string()),
                        ("CODEX_HOME".into(), home.join(".codex").into_os_string()),
                        (
                            "CLAUDE_CONFIG_DIR".into(),
                            home.join(".claude").into_os_string(),
                        ),
                        ("CAPTURE_FILE".into(), capture.clone().into_os_string()),
                        ("TERM".into(), "xterm-256color".into()),
                        ("MATOU_DATA_ROOT".into(), "stale-root".into()),
                        ("MATOU_SESSION_ID".into(), "stale-session".into()),
                    ]
                    .into(),
                    cols: 80,
                    rows: 24,
                },
                terminal: TerminalOptions::new(80, 24, 7, 14),
                journal: JournalOptions::default(),
                restored_history: Vec::new(),
                screen_recovery: None,
                collect_history: false,
            },
            false,
            "",
            Some((
                json!({"model":"fixture"}),
                Vec::new(),
                None,
                None,
                None,
                None,
            )),
            None,
            None,
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.owner.shells[session].emitted_output && Instant::now() < deadline {
            self.owner.poll(Duration::from_millis(10))?;
        }
        ensure!(
            self.owner.shells[session].emitted_output,
            "fixture did not write READY"
        );
        Ok(capture)
    }

    fn hook(&mut self, event: &str, fields: Value) {
        let scope = self.scope(SOURCE);
        let mut payload = fields;
        payload["hook_event_name"] = json!(event);
        payload["session_id"] = json!("provider-fixture-id");
        let mut observation = Observation::parse(payload.as_object().unwrap());
        observation.hud_payload = Some(payload);
        self.owner.apply_hook(scope, observation, &mut Vec::new());
    }

    fn scope(&self, session: &str) -> Scope {
        let shell = &self.owner.shells[session];
        Scope {
            profile: shell.hook.as_ref().unwrap().profile,
            session_id: session.into(),
            run_id: shell.run_id.clone(),
            generation: GENERATION.into(),
            expected_id: None,
            fork_source_id: None,
        }
    }
}

fn poll_idle_recovery(owner: &mut ShellOwner) -> Vec<OwnerEvent> {
    owner.next_context_reminder_poll = Instant::now();
    let mut events = Vec::new();
    owner.poll_context_reminders(&mut events);
    events
}

fn source_hook(owner: &mut ShellOwner) -> &mut RunHook {
    owner.shells.get_mut(SOURCE).unwrap().hook.as_mut().unwrap()
}

fn source_work_status(owner: &ShellOwner) -> Result<String> {
    Ok(owner.connection.query_row(
        "SELECT work_status FROM sessions WHERE id = ?",
        [SOURCE],
        |row| row.get(0),
    )?)
}

#[test]
fn claude_idle_recovery_requires_confirmed_hooks_and_continuous_idle() -> Result<()> {
    let mut fixture = Fixture::new(LaunchProfile::ClaudeCode)?;
    fixture.launch(SOURCE, LaunchProfile::ClaudeCode)?;
    let config = ContextOptimizerConfig::for_data_root(&fixture.data);
    config.set_agent_settings(
        "claude-code",
        AgentOptimizerSettings {
            enabled: false,
            ..Default::default()
        },
    )?;
    let scope = fixture.scope(SOURCE);
    let expire = |owner: &mut ShellOwner| {
        let hook = source_hook(owner);
        let past = Instant::now() - PROVIDER_IDLE_RECOVERY;
        hook.idle_while_running_since = Some(past);
        hook.last_submission = hook.last_submission.map(|_| past);
    };
    let submit = |owner: &mut ShellOwner| {
        owner.apply(
            WorkerEvent {
                session_id: SOURCE.into(),
                run_id: scope.run_id.clone(),
                event: SessionEvent::InputSubmitted(Instant::now()),
            },
            &mut Vec::new(),
        );
    };
    submit(&mut fixture.owner);
    assert_eq!(source_work_status(&fixture.owner)?, "running");
    assert!(source_hook(&mut fixture.owner).last_observation.is_none());
    expire(&mut fixture.owner);
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    assert_eq!(source_work_status(&fixture.owner)?, "running");
    assert!(
        source_hook(&mut fixture.owner)
            .idle_while_running_since
            .is_none()
    );

    fixture.hook("SessionStart", json!({}));
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    let hook = source_hook(&mut fixture.owner);
    assert!(hook.last_observation.is_some());
    assert!(hook.idle_while_running_since.is_some());
    expire(&mut fixture.owner);
    let events = poll_idle_recovery(&mut fixture.owner);
    assert_eq!(source_work_status(&fixture.owner)?, "idle");
    assert!(matches!(events.as_slice(), [OwnerEvent::Domain(commit)]
        if commit.first_event_sequence.is_some()
            && commit.result["sessionId"] == SOURCE
            && commit.result["previousStatus"] == "running"
            && commit.result["workStatus"] == "idle"
            && commit.result["graph"]["nodes"][0]["workStatus"] == "idle"));

    submit(&mut fixture.owner);
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    assert_eq!(source_work_status(&fixture.owner)?, "running");
    expire(&mut fixture.owner);
    submit(&mut fixture.owner);
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    assert_eq!(source_work_status(&fixture.owner)?, "running");
    expire(&mut fixture.owner);
    fixture.hook("UserPromptSubmit", json!({}));
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    assert_eq!(source_work_status(&fixture.owner)?, "running");
    let hook = source_hook(&mut fixture.owner);
    assert_eq!(
        hook.hud.snapshot(now() as f64).task_status,
        TaskStatus::Running
    );
    assert!(hook.idle_while_running_since.is_none());

    config.set_agent_settings(
        "claude-code",
        AgentOptimizerSettings {
            cache_monitor: false,
            ..Default::default()
        },
    )?;
    fixture.hook("Stop", json!({}));
    submit(&mut fixture.owner);
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    assert_eq!(source_work_status(&fixture.owner)?, "running");
    expire(&mut fixture.owner);
    poll_idle_recovery(&mut fixture.owner);
    assert_eq!(source_work_status(&fixture.owner)?, "idle");

    submit(&mut fixture.owner);
    let question = json!({
        "type": "assistant",
        "message": { "content": [{"type": "tool_use", "id": "ask", "name": "AskUserQuestion"}] }
    });
    source_hook(&mut fixture.owner)
        .hud
        .ingest_live_claude_row(Some(&question));
    expire(&mut fixture.owner);
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    assert_eq!(source_work_status(&fixture.owner)?, "running");
    source_hook(&mut fixture.owner)
        .hud
        .ingest_live_claude_row(None);

    fixture
        .owner
        .set_work_status(SOURCE, &mut Vec::new(), |_| Some("needs-input"))?;
    expire(&mut fixture.owner);
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    assert_eq!(source_work_status(&fixture.owner)?, "needs-input");
    Ok(())
}

#[test]
fn idle_recovery_does_not_reclaim_dsh() -> Result<()> {
    let mut fixture = Fixture::new(LaunchProfile::Dsh)?;
    fixture.launch(SOURCE, LaunchProfile::Dsh)?;
    fixture
        .owner
        .set_work_status(SOURCE, &mut Vec::new(), |_| Some("running"))?;
    let hook = source_hook(&mut fixture.owner);
    // Satisfy the hook and HUD checks to isolate the provider check.
    let payload = json!({"hook_event_name": "SessionStart", "session_id": "provider-fixture-id"});
    hook.last_observation = Some(Observation::parse(payload.as_object().unwrap()));
    hook.idle_while_running_since = Some(Instant::now() - PROVIDER_IDLE_RECOVERY);
    assert_eq!(
        hook.hud.snapshot(now() as f64).task_status,
        TaskStatus::Idle
    );
    assert!(poll_idle_recovery(&mut fixture.owner).is_empty());
    assert_eq!(source_work_status(&fixture.owner)?, "running");
    assert!(
        source_hook(&mut fixture.owner)
            .idle_while_running_since
            .is_none()
    );
    Ok(())
}

#[test]
fn handoff_collects_tool_paths_worktree_changes_and_logs_on_demand() -> Result<()> {
    let mut fixture = Fixture::new(LaunchProfile::ClaudeCode)?;
    fixture.launch(SOURCE, LaunchProfile::ClaudeCode)?;

    fs::write(fixture.cwd.join(".git/info/exclude"), "/already saved.rs\n")?;
    fs::write(fixture.cwd.join("already saved.rs"), "tool-written fixture")?;
    fs::write(
        fixture.cwd.join("worktree only.rs"),
        "actual changed content",
    )?;
    let entry = ContextGuardStore::open(&fixture.data)?.save_log(
        SOURCE,
        "Bash",
        "cargo test",
        "truncated fixture",
    )?;
    fixture.hook("SessionStart", json!({}));
    fixture.hook(
        "PostToolUse",
        json!({
            "tool_name":"MultiEdit", "tool_use_id":"edit-1",
            "tool_input":{"file_path":fixture.cwd.join("already saved.rs")},
        }),
    );
    fixture.hook(
        "PostToolUseFailure",
        json!({
            "tool_name":"Bash", "tool_use_id":"test-1",
            "tool_input":{"command":"cargo test TOKEN=fixture"},
            "tool_response":{"exit_code":17},
        }),
    );
    let brief = fixture
        .owner
        .session_handoff_brief(SOURCE, &fixture.data)
        .unwrap();
    assert!(
        brief
            .modified_files
            .contains(&"already saved.rs".to_owned())
    );
    assert!(
        brief
            .modified_files
            .contains(&"worktree only.rs".to_owned())
    );
    assert!(brief.markdown.contains(&entry.id));
    assert_eq!(brief.log_ids, [entry.id]);
    assert!(
        brief
            .recent_tools
            .iter()
            .any(|tool| tool.exit_code == Some(17) && tool.succeeded == Some(false))
    );
    Ok(())
}

#[test]
fn rescue_creates_and_submits_one_first_prompt_in_each_managed_profile() -> Result<()> {
    for profile in [
        LaunchProfile::ClaudeCode,
        LaunchProfile::Codex,
        LaunchProfile::Antigravity,
        LaunchProfile::Dsh,
    ] {
        let mut fixture = Fixture::new(profile)?;
        fixture.launch(SOURCE, profile)?;
        let (created, mut prompt) = start_context_rescue(
            &mut fixture.owner,
            &fixture.data,
            GENERATION,
            WINDOW,
            3,
            SOURCE,
        )?
        .unwrap();
        assert_eq!(created["session"]["kind"], profile.kind());
        let source = fixture.owner.session(SOURCE)?.unwrap();
        assert_eq!(created["session"]["cwd"], source.cwd);
        assert_eq!(
            created["session"]["executionContextId"],
            source.execution_context_id
        );
        assert_eq!(created["session"]["taskId"], source.task_id);
        assert_eq!(
            created["scene"]["id"],
            fixture
                .owner
                .notification_scene_id(WINDOW, SOURCE)?
                .unwrap()
        );
        assert_eq!(fixture.owner.hierarchy(WINDOW)?.session.unwrap().id, SOURCE);
        let card = prompt.session.clone();
        let capture = fixture.launch(&card, profile)?;
        let environment = fs::read_to_string(capture.with_extension("env"))?;
        assert_eq!(
            environment.lines().collect::<Vec<_>>(),
            [
                fixture.data.to_str().unwrap(),
                &card,
                fixture.cwd.to_str().unwrap()
            ]
        );
        assert_eq!(
            prompt.poll(&mut fixture.owner, false, Instant::now(), now()),
            None
        );
        assert_eq!(fs::metadata(&capture)?.len(), 0);

        // The fake PTY substitutes for a live provider. There is no model request.
        fixture
            .owner
            .shells
            .get_mut(&card)
            .unwrap()
            .hook
            .as_mut()
            .unwrap()
            .rescue_ready = true;
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            fixture.owner.poll(Duration::from_millis(10))?;
            if let Some(outcome) = prompt.poll(&mut fixture.owner, false, Instant::now(), now()) {
                assert_eq!(outcome, RescueDelivery::Delivered, "profile {profile:?}");
                break;
            }
            ensure!(Instant::now() < end, "fixture did not receive input");
        }
        let (_, mut replay) = start_context_rescue(
            &mut fixture.owner,
            &fixture.data,
            GENERATION,
            WINDOW,
            3,
            SOURCE,
        )?
        .unwrap();
        assert_eq!(replay.session, card);
        assert!(!replay.needs_launch());
        assert_eq!(
            replay.poll(&mut fixture.owner, false, Instant::now(), now()),
            Some(RescueDelivery::Delivered)
        );
        thread::sleep(Duration::from_millis(30));
        let received = fs::read(&capture)?;
        let expected = format!(
            "\x1b[200~{}\x1b[201~\r",
            prompt.markdown.replace('\n', "\r")
        );
        ensure!(
            received == expected.as_bytes(),
            "profile {profile:?} received {} bytes, expected exactly {} bytes in one paste and one Enter",
            received.len(),
            expected.len()
        );
    }
    Ok(())
}

#[test]
fn rescue_timeout_after_paste_fences_enter_and_replay() -> Result<()> {
    let mut fixture = Fixture::new(LaunchProfile::Codex)?;
    fixture.launch(SOURCE, LaunchProfile::Codex)?;
    let (_, mut prompt) = start_context_rescue(
        &mut fixture.owner,
        &fixture.data,
        GENERATION,
        WINDOW,
        4,
        SOURCE,
    )?
    .unwrap();
    let card = prompt.session.clone();
    let capture = fixture.launch(&card, LaunchProfile::Codex)?;
    assert_eq!(fs::metadata(&capture)?.len(), 0);
    fixture
        .owner
        .shells
        .get_mut(&card)
        .unwrap()
        .hook
        .as_mut()
        .unwrap()
        .rescue_ready = true;
    // Wait for the paste, then expire its receipt before a separate Enter is sent.
    let end = Instant::now() + Duration::from_secs(5);
    let pasted = loop {
        let bytes = fs::read(&capture)?;
        if bytes.ends_with(b"\x1b[201~") {
            break bytes;
        }
        ensure!(Instant::now() < end, "fixture did not receive paste body");
        assert_eq!(
            prompt.poll(&mut fixture.owner, false, Instant::now(), now()),
            None
        );
        fixture.owner.poll(Duration::from_millis(5))?;
    };
    let expired = Instant::now() + Duration::from_secs(21);
    assert_eq!(
        prompt.poll(&mut fixture.owner, false, expired, now()),
        Some(RescueDelivery::ClipboardFallback)
    );
    let mut replay = RescuePrompt::new(&mut fixture.owner, card, prompt.markdown.clone(), now())?;
    assert!(!replay.needs_launch());
    assert_eq!(
        replay.poll(&mut fixture.owner, false, Instant::now(), now()),
        Some(RescueDelivery::ClipboardFallback)
    );
    assert_eq!(replay.markdown, prompt.markdown);
    assert_eq!(fs::read(&capture)?, pasted);
    Ok(())
}

#[test]
fn dsh_context_readings_raise_water_reminders_and_clear_on_compaction() -> Result<()> {
    let mut fixture = Fixture::new(LaunchProfile::Dsh)?;
    fixture.launch(SOURCE, LaunchProfile::Dsh)?;
    let scope = fixture.scope(SOURCE);
    let shell = fixture.owner.shells.get_mut(SOURCE).unwrap();
    shell.hook.as_mut().unwrap().dsh =
        Some(DshObservation::Fresh(DshEvents::new(&scope.run_id, None)));
    let event = |sequence, kind, data: Value, percent| {
        json!({
            "dsh_observation":1, "run_id":scope.run_id, "foreground":true,
            "generation":1, "sequence":sequence, "session_id":"fixture-dsh",
            "event_type":kind, "event_data":data, "agent_status":"running",
            "ready":kind == "ready", "persisted":false,
            "hud":{"contextPercent":percent,"contextWindowSize":10000}
        })
    };
    let apply = |owner: &mut ShellOwner, payload| {
        owner.apply_dsh_hook(scope.clone(), &payload, &mut Vec::new())
    };
    let scene = fixture.owner.notification_scene_id(WINDOW, SOURCE)?.unwrap();
    let water_reminders = |owner: &mut ShellOwner| {
        owner.next_context_reminder_poll = Instant::now();
        let mut events = Vec::new();
        owner.poll_context_reminders(&mut events);
        events
            .into_iter()
            .filter_map(|event| match event {
                OwnerEvent::Domain(commit) => {
                    commit.result["agentNotification"]["eventType"].as_str().map(str::to_owned)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    apply(&mut fixture.owner, event(1, "ready", json!({}), 75));
    assert_eq!(water_reminders(&mut fixture.owner), ["context-water-warning"]);
    assert!(
        fixture.owner.graph(&scene, Some(WINDOW))?["nodes"][0]
            .get("contextCardAlert")
            .is_none()
    );
    apply(&mut fixture.owner, event(2, "ready", json!({}), 90));
    assert_eq!(water_reminders(&mut fixture.owner), ["context-water-critical"]);
    assert_eq!(
        fixture.owner.graph(&scene, Some(WINDOW))?["nodes"][0]["contextCardAlert"],
        "critical"
    );
    assert!(water_reminders(&mut fixture.owner).is_empty());

    apply(
        &mut fixture.owner,
        event(3, "compaction/end", json!({"completed":false}), 90),
    );
    assert_eq!(
        fixture.owner.agent_hud(SOURCE, &fixture.data)?["hud"]["contextPercent"],
        90.0
    );
    apply(
        &mut fixture.owner,
        event(4, "compaction/end", json!({"completed":true}), 90),
    );
    apply(&mut fixture.owner, event(5, "ready", json!({}), 90));
    assert!(fixture.owner.agent_hud(SOURCE, &fixture.data)?["hud"]["contextPercent"].is_null());
    assert!(water_reminders(&mut fixture.owner).is_empty());
    assert!(
        fixture.owner.graph(&scene, Some(WINDOW))?["nodes"][0]
            .get("contextCardAlert")
            .is_none()
    );
    apply(&mut fixture.owner, event(6, "ready", json!({}), 22));
    assert_eq!(
        fixture.owner.agent_hud(SOURCE, &fixture.data)?["hud"]["contextPercent"],
        22.0
    );
    Ok(())
}
