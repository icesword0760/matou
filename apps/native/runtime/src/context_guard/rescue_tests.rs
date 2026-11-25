use super::*;

use matou_native_core::{
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
                default_name: "Watchdog workspace".into(),
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

#[test]
fn background_stall_notifies_once_and_collects_tool_paths_worktree_changes_and_logs() -> Result<()>
{
    let mut fixture = Fixture::new(LaunchProfile::ClaudeCode)?;
    let scene = fixture
        .owner
        .notification_scene_id(WINDOW, SOURCE)?
        .unwrap();
    fixture.launch(SOURCE, LaunchProfile::ClaudeCode)?;
    let other = fixture.root.path().join("other-workspace");
    fs::create_dir(&other)?;
    fixture.owner.create_workspace(
        &command(
            "other-workspace".into(),
            "hierarchy.create-workspace",
            "other".into(),
        ),
        &workspaces::CreateWorkspaceInput {
            window_id: WINDOW.into(),
            name: "Elsewhere".into(),
            root_directory: other.to_string_lossy().into_owned(),
            navigation: None,
            now: now(),
        },
        &hierarchy_ids(),
        now(),
    )?;
    assert_ne!(fixture.owner.hierarchy(WINDOW)?.scene.unwrap().id, scene);

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
    ContextOptimizerConfig::for_data_root(&fixture.data).set_agent_settings(
        "claude-code",
        matou_native_core::context_optimizer_config::AgentOptimizerSettings {
            cache_monitor: false,
            stall_rescue: true,
            ..Default::default()
        },
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
    fixture.hook("UserPromptSubmit", json!({}));
    // Publish the provider tool observations first so the simulated quiet
    // interval starts after the final tool result.
    fixture.owner.next_context_reminder_poll = Instant::now();
    fixture.owner.poll_context_reminders(&mut Vec::new());
    fixture
        .owner
        .shells
        .get_mut(SOURCE)
        .unwrap()
        .hook
        .as_mut()
        .unwrap()
        .context_guard
        .on_task_status(TaskStatus::Running, now() as f64 - 200_000.0);
    let notices = |owner: &mut ShellOwner| {
        owner.next_context_reminder_poll = Instant::now();
        let mut events = Vec::new();
        owner.poll_context_reminders(&mut events);
        events
            .into_iter()
            .filter_map(|event| match event {
                OwnerEvent::Domain(commit) => Some(commit.result["agentNotification"].clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let notification = loop {
        if let Some(notification) = notices(&mut fixture.owner).into_iter().next() {
            break notification;
        }
        ensure!(
            Instant::now() < deadline,
            "no stall notification within five seconds"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(notification["eventType"], "context-stall-detected");
    assert_eq!(notification["sessionId"], SOURCE);
    assert_eq!(notification["sound"], true);
    assert_eq!(
        notification["subtitle"],
        "Watchdog workspace · Original card"
    );
    assert_eq!(
        fixture.owner.graph(&scene, Some(WINDOW))?["nodes"][0]["contextCardAlert"],
        "stalled"
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
    assert!(notices(&mut fixture.owner).is_empty());
    let target = fixture
        .owner
        .notification_scene_id(WINDOW, SOURCE)?
        .unwrap();
    fixture.owner.focus_session(
        WINDOW,
        &target,
        SOURCE,
        now(),
        "notification target missing",
    )?;
    assert_eq!(fixture.owner.hierarchy(WINDOW)?.scene.unwrap().id, scene);
    assert_eq!(fixture.owner.hierarchy(WINDOW)?.session.unwrap().id, SOURCE);

    fixture
        .owner
        .apply_output(SOURCE, "provider resumed output\r\n", &[], &mut Vec::new());
    assert!(notices(&mut fixture.owner).is_empty());
    let hook = fixture.owner.shells[SOURCE].hook.as_ref().unwrap();
    assert!(hook.stall_handoff.is_none());
    assert!(!hook.stall_notified);
    assert!(hook.context_card_alert.is_none());
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
fn dsh_ready_heartbeat_does_not_mask_a_stall_and_completed_compaction_resets_old_usage()
-> Result<()> {
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
    apply(&mut fixture.owner, event(1, "ready", json!({}), 90));
    fixture.owner.next_context_reminder_poll = Instant::now();
    fixture.owner.poll_context_reminders(&mut Vec::new());
    let hook = fixture
        .owner
        .shells
        .get_mut(SOURCE)
        .unwrap()
        .hook
        .as_mut()
        .unwrap();
    assert_eq!(hook.context_guard.context_percent(), Some(90.));
    hook.context_guard
        .on_task_status(TaskStatus::Running, now() as f64 - 200_000.);
    apply(&mut fixture.owner, event(2, "ready", json!({}), 90));
    let hook = fixture.owner.shells[SOURCE].hook.as_ref().unwrap();
    assert!(hook.context_guard.is_stalled(now() as f64));

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
    apply(&mut fixture.owner, event(6, "ready", json!({}), 22));
    assert_eq!(
        fixture.owner.agent_hud(SOURCE, &fixture.data)?["hud"]["contextPercent"],
        22.0
    );
    Ok(())
}
