use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Result;
use matou_native_runtime::{
    agent_hud_projection::{AgentHudProjection, HudProvider},
    antigravity_catalog::{
        Catalog, DetailOptions, EventKind, ListOptions, RecoveryCandidate, SearchScope,
        catalog_root,
    },
    antigravity_events::{
        FORK_COMMAND, Outcome, RecoveryFailure, RecoveryState, Run, Source, Update, WorkStatus,
    },
    antigravity_launch::{
        LaunchInput, LaunchMode, PermissionMode, ensure_plugin_installed, interactive_permission,
        plugin_directory, prepare,
    },
};
use serde_json::{Value, json};
use tempfile::TempDir;
use url::Url;

fn sandbox() -> Result<TempDir> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("runtime is inside the native workspace")
        .join("target");
    fs::create_dir_all(&directory)?;
    Ok(tempfile::Builder::new()
        .prefix("antigravity-contract-")
        .tempdir_in(directory)?)
}

fn environment(values: &[(&str, &str)]) -> BTreeMap<OsString, OsString> {
    values
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}

fn executable(directory: &Path, name: &str) -> Result<PathBuf> {
    let path = directory.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    });
    fs::write(&path, "#!/bin/sh\nexit 99\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

#[test]
fn antigravity_launch_contract() -> Result<()> {
    assert_eq!(interactive_permission("agy"), Some(PermissionMode::Default));
    assert_eq!(
        interactive_permission(" agy  --mode accept-edits "),
        Some(PermissionMode::AcceptEdits)
    );
    assert_eq!(
        interactive_permission("agy --mode=plan"),
        Some(PermissionMode::Plan)
    );
    assert_eq!(
        interactive_permission("agy --dangerously-skip-permissions"),
        Some(PermissionMode::BypassPermissions)
    );
    for command in [
        "agy --dangerously-skip-permissions --mode auto",
        "agy --mode=plan --dangerously-skip-permissions",
    ] {
        assert_eq!(
            interactive_permission(command),
            Some(PermissionMode::BypassPermissions)
        );
    }
    for command in [
        "agy --help",
        "agy --mode",
        "agy --mode typo",
        "agy acp",
        "agy --mode plan && echo",
    ] {
        assert_eq!(interactive_permission(command), None);
    }
    let temp = sandbox()?;
    let root = temp.path().canonicalize()?;
    let fake = executable(&root, "agy")?;
    let override_command = executable(&root, "custom agy")?;
    let settings = root.join("settings with spaces.json");
    let file_settings = json!({
        "agentMode": "plan", "toolPermission": "always-proceed",
        "enableTerminalSandbox": true, "allowNonWorkspaceAccess": true,
        "allow": ["command(echo)"], "deny": ["command(sudo)"]
    });
    fs::write(&settings, file_settings.to_string())?;
    let system_message = root.join("system message.json");
    let path = std::env::join_paths([&root])?;
    let launch = |mode, permission, model, command: Option<&Path>| {
        let mut inherited = environment(&[
            ("MATOU_PROVIDER_HOOK_URL", "stale-url"),
            ("MATOU_PROVIDER_HOOK_TOKEN", "stale-token"),
            ("MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE", "stale-file"),
            ("MATOU_CONTROL_TOKEN", "stale-control"),
            ("MATOU_AGENT_STATUS_DIR", "stale-status"),
            ("CLAUDE_CODE_SESSION_ID", "parent"),
            ("TERM", "dumb"),
            ("TZ", "UTC"),
            ("NO_COLOR", "1"),
            ("HTTPS_PROXY", "local-fixture"),
        ]);
        inherited.insert("PATH".into(), path.clone());
        if let Some(command) = command {
            inherited.insert(
                "MATOU_ANTIGRAVITY_COMMAND".into(),
                command.as_os_str().into(),
            );
        }
        prepare(LaunchInput {
            cwd: &root,
            settings_path: Some(Path::new("settings with spaces.json")),
            model,
            permission,
            mode,
            inherited_environment: inherited,
            run_environment: environment(&[
                ("MATOU_PROVIDER_HOOK_URL", "http://127.0.0.1:1/hooks/local"),
                ("MATOU_PROVIDER_HOOK_TOKEN", "local"),
                (
                    "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE",
                    "system message.json",
                ),
                ("MATOU_CONTROL_CALLER_RUN", "run1"),
            ]),
        })
    };

    let fresh = launch(LaunchMode::Fresh, PermissionMode::Default, None, None)?;
    let settings_arg = settings.as_os_str();
    assert_eq!(fresh.executable, fake);
    assert_eq!(fresh.cwd, root);
    assert_eq!(
        fresh.argv,
        [std::ffi::OsStr::new("--settings"), settings_arg]
    );
    let env = &fresh.environment;
    assert_eq!(env[&OsString::from("TERM")], "xterm-256color");
    assert_eq!(env[&OsString::from("MATOU_PROVIDER_HOOK_TOKEN")], "local");
    assert_eq!(
        env[&OsString::from("MATOU_PROVIDER_HOOK_URL")],
        "http://127.0.0.1:1/hooks/local"
    );
    assert_eq!(
        env[&OsString::from("MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE")],
        system_message.as_os_str()
    );
    assert_eq!(env[&OsString::from("MATOU_CONTROL_CALLER_RUN")], "run1");
    assert_eq!(env[&OsString::from("HTTPS_PROXY")], "local-fixture");
    assert_eq!(env[&OsString::from("TZ")], "UTC");
    assert_eq!(env[&OsString::from("NO_COLOR")], "1");
    for key in [
        "CLAUDE_CODE_SESSION_ID",
        "MATOU_AGENT_STATUS_DIR",
        "MATOU_CONTROL_TOKEN",
    ] {
        assert!(!env.contains_key(&OsString::from(key)));
    }
    #[cfg(unix)]
    assert_eq!(env[&OsString::from("PWD")], root.as_os_str());

    for (permission, flags) in [
        (PermissionMode::Auto, &["--mode", "auto"][..]),
        (PermissionMode::Plan, &["--mode", "plan"]),
        (PermissionMode::AcceptEdits, &["--mode", "accept-edits"]),
        (
            PermissionMode::BypassPermissions,
            &["--dangerously-skip-permissions"],
        ),
        (PermissionMode::Turbo, &["--dangerously-skip-permissions"]),
        (PermissionMode::Ask, &[]),
    ] {
        let mode = LaunchMode::Resume("agy-conv-1".into());
        let resume = launch(
            mode.clone(),
            permission,
            Some("  local model  "),
            Some(&override_command),
        )?;
        assert_eq!(resume.argv[0], "--settings");
        if permission == PermissionMode::BypassPermissions {
            assert_eq!(resume.argv[1], settings_arg);
        } else {
            let settings: Value = serde_json::from_str(resume.argv[1].to_str().unwrap())?;
            assert_eq!(settings["allow"], file_settings["allow"]);
            assert_eq!(settings["deny"], file_settings["deny"]);
            if permission == PermissionMode::Turbo {
                assert_eq!(settings["agentMode"], "accept-edits");
                assert_eq!(settings["toolPermission"], "always-proceed");
                assert_eq!(settings["enableTerminalSandbox"], false);
                assert_eq!(settings["allowNonWorkspaceAccess"], true);
            } else {
                let agent_mode = if permission == PermissionMode::Ask {
                    ""
                } else {
                    "plan"
                };
                assert_eq!(settings["agentMode"], agent_mode);
                assert_eq!(settings["toolPermission"], "request-review");
                assert_eq!(settings["enableTerminalSandbox"], true);
                assert_eq!(settings["allowNonWorkspaceAccess"], false);
            }
        }
        let mut expected = Vec::from(
            ["--model", "local model", "--conversation", "agy-conv-1"].map(OsString::from),
        );
        expected.extend(flags.iter().map(OsString::from));
        assert_eq!(resume.argv[2..], expected);
        assert_eq!(resume.mode, mode);
        assert_eq!(resume.executable, override_command);
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&settings)?)?,
        file_settings
    );

    let turbo = prepare(LaunchInput {
        cwd: &root,
        settings_path: None,
        model: None,
        permission: PermissionMode::Turbo,
        mode: LaunchMode::Fresh,
        inherited_environment: BTreeMap::from([("PATH".into(), path.clone())]),
        run_environment: BTreeMap::new(),
    })?;
    assert_eq!(turbo.argv.len(), 3);
    assert_eq!(turbo.argv[0], "--settings");
    assert_eq!(
        serde_json::from_str::<Value>(turbo.argv[1].to_str().unwrap())?,
        json!({
            "agentMode": "accept-edits", "toolPermission": "always-proceed",
            "enableTerminalSandbox": false, "allowNonWorkspaceAccess": true
        })
    );
    assert_eq!(turbo.argv[2], "--dangerously-skip-permissions");
    let fork = launch(
        LaunchMode::Fork(" agy-conv-1 ".into()),
        PermissionMode::Default,
        None,
        None,
    )?;
    assert_eq!(fork.mode, LaunchMode::Fork("agy-conv-1".into()));
    assert_eq!(fork.argv[2..], ["--conversation", "agy-conv-1"]);
    for id in ["", "../escape", "--flag", "folder/child", "abc\0def"] {
        assert!(
            launch(
                LaunchMode::Resume(id.into()),
                PermissionMode::Default,
                None,
                None
            )
            .is_err()
        );
    }
    assert!(
        launch(
            LaunchMode::Fresh,
            PermissionMode::Default,
            Some("--flag"),
            None
        )
        .is_err()
    );
    let missing = launch(
        LaunchMode::Fresh,
        PermissionMode::Default,
        None,
        Some(&root.join("missing")),
    );
    assert!(missing.err().unwrap().to_string().contains("Antigravity"));
    assert!(
        launch(
            LaunchMode::Fresh,
            PermissionMode::Default,
            None,
            Some(&root)
        )
        .is_err()
    );
    let invalid_settings = prepare(LaunchInput {
        cwd: &root,
        settings_path: Some(Path::new("bad\0path")),
        model: None,
        permission: PermissionMode::Default,
        mode: LaunchMode::Fresh,
        inherited_environment: BTreeMap::from([("PATH".into(), path)]),
        run_environment: BTreeMap::new(),
    });
    assert!(invalid_settings.is_err());
    Ok(())
}

#[test]
fn antigravity_missing_cli_uses_frozen_localized_messages() -> Result<()> {
    let temp = sandbox()?;
    let cwd = temp.path();
    fs::write(cwd.join("agy"), "not executable")?;
    for (locale, expected) in [
        (
            "zh-CN",
            "未检测到 Antigravity CLI（命令：agy）。请先安装并确保它在 PATH 中。",
        ),
        (
            "en",
            "Antigravity CLI (agy) was not found. Install it and ensure it is on PATH.",
        ),
    ] {
        let mut inherited = environment(&[("MATOU_LOCALE", locale), ("SHELL", "./missing-shell")]);
        inherited.insert("PATH".into(), cwd.as_os_str().into());
        inherited.insert("MATOU_DISABLE_LOGIN_PATH_PROBE".into(), "1".into());
        let error = prepare(LaunchInput {
            cwd,
            settings_path: None,
            model: None,
            permission: PermissionMode::Default,
            mode: LaunchMode::Fresh,
            inherited_environment: inherited,
            run_environment: BTreeMap::new(),
        })
        .err()
        .expect("no executable agy");
        assert_eq!(error.to_string(), expected);
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn antigravity_and_dsh_share_the_login_path() -> Result<()> {
    use matou_native_runtime::dsh_launch::{self, DshLaunchInput};

    let temp = sandbox()?;
    let cwd = temp.path();
    let agy = executable(cwd, "agy")?;
    let dsh = executable(cwd, "dsh")?;
    let login_shell = executable(cwd, "login-shell")?;
    fs::write(
        &login_shell,
        "#!/bin/sh\nprintf 'probe\\n' >> \"$HOME/probes\"\nprintf '__MATOU_LOGIN_PATH__=%s\\n' \"$HOME\"\n",
    )?;
    let inherited = BTreeMap::from([
        ("HOME".into(), cwd.as_os_str().into()),
        ("SHELL".into(), login_shell.into_os_string()),
        ("PATH".into(), "missing-bin".into()),
    ]);
    let launch = prepare(LaunchInput {
        cwd,
        settings_path: None,
        model: None,
        permission: PermissionMode::Default,
        mode: LaunchMode::Fresh,
        inherited_environment: inherited.clone(),
        run_environment: BTreeMap::new(),
    })?;
    assert_eq!(launch.executable, agy);
    assert_eq!(launch.environment[&OsString::from("PATH")], cwd.as_os_str());

    let launch = dsh_launch::prepare(DshLaunchInput {
        cwd,
        patch_path: None,
        resume_session_id: None,
        inherited_environment: inherited,
        run_environment: BTreeMap::new(),
    })?;
    assert_eq!(launch.executable, dsh);
    assert_eq!(launch.environment[&OsString::from("PATH")], cwd.as_os_str());
    assert_eq!(fs::read_to_string(cwd.join("probes"))?, "probe\n");
    Ok(())
}

#[test]
fn antigravity_plugin_install_update_and_repair() -> Result<()> {
    let temp = sandbox()?;
    let plugin = plugin_directory(temp.path(), &environment(&[]));
    assert_eq!(plugin, temp.path().join(".gemini/config/plugins/matou"));
    assert_eq!(
        plugin_directory(
            temp.path(),
            &environment(&[("MATOU_ANTIGRAVITY_PLUGIN_DIR", " /custom/matou ")])
        ),
        Path::new("/custom/matou")
    );
    let third_party = plugin.with_file_name("third-party");
    fs::create_dir_all(&third_party)?;
    fs::write(third_party.join("plugin.json"), r#"{"name":"third-party"}"#)?;

    std::thread::scope(|scope| {
        let installers: Vec<_> = (0..3)
            .map(|_| scope.spawn(|| ensure_plugin_installed(&plugin)))
            .collect();
        for installer in installers {
            installer.join().unwrap().unwrap();
        }
    });
    let manifest = plugin.join("plugin.json");
    let hooks = plugin.join("hooks.json");
    let stamp = plugin.join(".matou-plugin-version");
    let hooks_json: Value = serde_json::from_slice(&fs::read(&hooks)?)?;
    let runtime = std::env::current_exe()?;
    for (event, hint) in [
        ("PreInvocation", "observe"),
        ("PostInvocation", "observe"),
        ("Stop", "observe"),
        ("SessionStart", "session-start"),
        ("PreToolUse", "pre-tool-use"),
        ("PostToolUse", "post-tool-use"),
    ] {
        let handler = &hooks_json["matou"][event][0];
        let command = handler["command"].as_str()
            .or_else(|| handler["hooks"][0]["command"].as_str()).unwrap();
        assert!(command.contains(&*runtime.to_string_lossy()));
        assert!(command.ends_with(&format!(" --hook agy {hint}")));
    }
    assert!(!plugin.join("hooks").exists());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&manifest)?)?["name"],
        "matou"
    );
    let expected_stamp = fs::read_to_string(&stamp)?;
    let modified = fs::metadata(&stamp)?.modified()?;
    ensure_plugin_installed(&plugin)?;
    assert_eq!(fs::metadata(&stamp)?.modified()?, modified);

    fs::create_dir(plugin.join("hooks"))?;
    fs::write(plugin.join("hooks/extra.sh"), "user file")?;
    fs::write(&stamp, "0.9.0:obsolete\n")?;
    fs::write(&hooks, "{bad")?;
    ensure_plugin_installed(&plugin)?;
    assert_eq!(fs::read_to_string(&stamp)?, expected_stamp);
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&hooks)?)?, hooks_json);
    fs::remove_file(&hooks)?;
    ensure_plugin_installed(&plugin)?;
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&hooks)?)?, hooks_json);
    assert_eq!(fs::read_to_string(plugin.join("hooks/extra.sh"))?, "user file");
    assert!(ensure_plugin_installed(&third_party).is_err());
    assert_eq!(
        fs::read_to_string(third_party.join("plugin.json"))?,
        r#"{"name":"third-party"}"#
    );
    assert_eq!(fs::read_dir(third_party)?.count(), 1);
    assert_eq!(fs::read_dir(&plugin)?.count(), 4);
    Ok(())
}

#[cfg(unix)]
#[test]
fn antigravity_plugin_rejects_symlink_targets() -> Result<()> {
    use std::os::unix::fs::symlink;
    let temp = sandbox()?;
    let plugin = temp.path().join("matou");
    let other = temp.path().join("other");
    fs::create_dir(&other)?;
    let outside = other.join("matou-hook.sh");
    fs::write(&outside, "outside")?;
    let parent = temp.path().join("plugins");
    symlink(&other, &parent)?;
    assert!(ensure_plugin_installed(&parent.join("matou")).is_err());
    symlink(&other, &plugin)?;
    assert!(ensure_plugin_installed(&plugin).is_err());
    fs::remove_file(&plugin)?;
    fs::create_dir(&plugin)?;
    symlink(&other, plugin.join("hooks"))?;
    assert!(ensure_plugin_installed(&plugin).is_err());
    fs::remove_file(plugin.join("hooks"))?;
    fs::create_dir(plugin.join("hooks"))?;
    symlink(&outside, plugin.join("hooks/matou-hook.sh"))?;
    assert!(ensure_plugin_installed(&plugin).is_err());
    assert_eq!(fs::read_to_string(&outside)?, "outside");
    assert_eq!(fs::read_dir(other)?.count(), 1);
    Ok(())
}

fn observed(outcome: Outcome<'_>) -> Update<'_> {
    match outcome {
        Outcome::Observed(update) => update,
        other => panic!("expected accepted observation, got {other:?}"),
    }
}

#[test]
fn antigravity_high_permission_survives_native_cycle_reports() {
    for permission in [PermissionMode::Turbo, PermissionMode::BypassPermissions] {
        let mut run = Run::new("current", LaunchMode::Fresh, permission);
        let mut hud = AgentHudProjection::new("session", HudProvider::Antigravity, 0.);
        hud.seed_settings(None, Some(permission.as_str()), None, None, false);
        let auto =
            json!({"conversationId":"agy-fixture", "agent_state":"idle", "cycle_mode":"auto"});
        assert_eq!(
            run.ingest("retired", Source::Statusline, &auto, 1),
            Outcome::Ignored
        );
        let update = observed(run.ingest("current", Source::Statusline, &auto, 2));
        assert_eq!(update.identity.unwrap().permission, Some(permission));
        assert_eq!(update.permission_changed, None);
        hud.ingest_provider(&auto, 2.);
        assert_eq!(hud.snapshot(2.).permission_mode, Some(permission.as_str()));

        let plan =
            json!({"conversationId":"agy-fixture", "agent_state":"idle", "cycle_mode":"plan"});
        let update = observed(run.ingest("current", Source::Statusline, &plan, 3));
        assert_eq!(update.permission_changed, None);
        assert!(update.identity.is_none());
        hud.ingest_provider(&plan, 3.);
        assert_eq!(hud.snapshot(3.).permission_mode, Some(permission.as_str()));
    }
}

#[test]
fn antigravity_default_follows_native_modes_without_persisting_explicit_auto() {
    let mut run = Run::new("current", LaunchMode::Fresh, PermissionMode::Default);
    let mut hud = AgentHudProjection::new("session", HudProvider::Antigravity, 0.);
    let auto = json!({"conversationId":"agy-fixture", "agent_state":"idle", "cycle_mode":"auto"});
    let update = observed(run.ingest("current", Source::Statusline, &auto, 1));
    assert_eq!(
        update.identity.unwrap().permission,
        Some(PermissionMode::Default)
    );
    assert_eq!(update.permission_changed, Some(PermissionMode::Auto));
    hud.ingest_provider(&auto, 1.);
    assert_eq!(hud.snapshot(1.).permission_mode, Some("auto"));

    let malformed =
        json!({"conversationId":"agy-fixture", "agent_state":"idle", "cycle_mode":false});
    let update = observed(run.ingest("current", Source::Statusline, &malformed, 2));
    assert_eq!(update.permission_changed, None);
    hud.ingest_provider(&malformed, 2.);
    assert_eq!(hud.snapshot(2.).permission_mode, Some("auto"));

    let ask = json!({"conversationId":"agy-fixture", "agent_state":"idle", "cycle_mode":""});
    let update = observed(run.ingest("current", Source::Statusline, &ask, 3));
    assert_eq!(update.permission_changed, Some(PermissionMode::Ask));
    hud.ingest_provider(&ask, 3.);
    assert_eq!(hud.snapshot(3.).permission_mode, Some("ask"));

    let plan = json!({"conversationId":"agy-fixture", "agent_state":"idle", "cycleMode":"plan"});
    let update = observed(run.ingest("current", Source::Statusline, &plan, 4));
    assert_eq!(update.permission_changed, Some(PermissionMode::Plan));
    hud.ingest_provider(&plan, 4.);
    assert_eq!(hud.snapshot(4.).permission_mode, Some("plan"));
    let update = observed(run.ingest("current", Source::Statusline, &auto, 5));
    assert_eq!(update.identity.unwrap().permission, Some(PermissionMode::Auto));

    let mut explicit = Run::new("current", LaunchMode::Fresh, PermissionMode::Auto);
    let update = observed(explicit.ingest("current", Source::Statusline, &auto, 1));
    assert_eq!(
        update.identity.unwrap().permission,
        Some(PermissionMode::Auto)
    );
}

#[test]
fn antigravity_event_statusline_and_hook_transitions() {
    let mut run = Run::new("current", LaunchMode::Fresh, PermissionMode::Default);
    let startup = json!({"agent_state": "authenticating"});
    assert_eq!(
        observed(run.ingest("current", Source::Statusline, &startup, 1)).work_status,
        Some(WorkStatus::Starting)
    );
    let idle = json!({
        "conversation_id": "agy-conv-1", "agent_state": "idle", "cycle_mode": "auto",
        "model": {"id": "fixture-model", "effort": "medium"},
        "context_window": {"used_percentage": 12},
        "quota": {"gemini-5h": {"remaining_fraction": 0.75}},
        "transcriptPath": "/ignored"
    });
    let update = observed(run.ingest("current", Source::Statusline, &idle, 2));
    assert_eq!(update.work_status, Some(WorkStatus::Idle));
    assert_eq!(update.identity.unwrap().model, Some("fixture-model"));
    assert_eq!(
        update.payload["quota"]["gemini-5h"]["remaining_fraction"],
        0.75
    );
    assert_eq!(update.transcript_path, None);
    assert_eq!(run.conversation_id(), Some("agy-conv-1"));
    assert!(!run.can_fork());
    let duplicate = observed(run.ingest("current", Source::Statusline, &idle, 3));
    assert!(duplicate.work_status.is_none());
    assert!(duplicate.identity.is_none());

    let plan =
        json!({"conversation_id": "agy-conv-1", "agent_state": "idle", "cycle_mode": "plan"});
    assert_eq!(
        observed(run.ingest("current", Source::Statusline, &plan, 4)).permission_changed,
        Some(PermissionMode::Plan)
    );
    assert_eq!(
        observed(run.ingest("current", Source::Statusline, &plan, 5)).permission_changed,
        None
    );
    let ask = json!({"conversation_id": "agy-conv-1", "agent_state": "idle"});
    assert_eq!(
        observed(run.ingest("current", Source::Statusline, &ask, 6)).permission_changed,
        Some(PermissionMode::Ask)
    );
    let accept_edits = json!({"conversation_id": "agy-conv-1", "agent_state": "idle", "cycle_mode": "accept-edits"});
    assert_eq!(
        observed(run.ingest("current", Source::Statusline, &accept_edits, 7)).permission_changed,
        Some(PermissionMode::AcceptEdits)
    );

    let prompt = json!({"hookEventName": "PreInvocation", "conversationId": "agy-conv-1"});
    assert_eq!(
        observed(run.ingest("current", Source::Hook, &prompt, 8)).work_status,
        Some(WorkStatus::Running)
    );
    let approval = json!({"conversation_id": "agy-conv-1", "agent_state": "tool_use"});
    let approval_notice = observed(run.ingest("current", Source::Statusline, &approval, 9));
    assert_eq!(approval_notice.work_status, Some(WorkStatus::NeedsInput));
    assert_eq!(
        approval_notice.notification.unwrap().event_type,
        "permission"
    );
    assert!(
        observed(run.ingest("current", Source::Statusline, &approval, 10))
            .notification
            .is_none()
    );
    let tool = json!({"event": "PreToolUse", "timestamp": 12, "data": {
        "conversationId": "agy-conv-1", "toolCall": {"id": "t1", "name": "write_file", "args": {"Path": "/repo/file"}}
    }});
    let update = observed(run.ingest("current", Source::Hook, &tool, 11));
    assert_eq!(update.work_status, Some(WorkStatus::Running));
    assert_eq!(update.semantic_events[0].kind, "agent.tool-started");
    assert_eq!(update.semantic_events[0].occurred_at, 12.0);
    let post_tool = json!({"hookEventName": "PostToolUse", "conversationId": "agy-conv-1",
        "toolUseId": "t1", "toolName": "write_file", "toolInput": {"TargetFile": "/repo/file"}, "error": false});
    let update = observed(run.ingest("current", Source::Hook, &post_tool, 13));
    assert_eq!(update.work_status, None);
    assert_eq!(update.semantic_events[0].payload["error"], false);
    assert_eq!(
        update.semantic_events[1].provider_event_id.as_deref(),
        Some("t1:file:/repo/file")
    );
    assert_eq!(update.semantic_events[1].kind, "file.changed");
    assert!(!run.can_fork());

    let stop = json!({"hookEventName": "Stop", "conversationId": "agy-conv-1", "cwd": "/repo",
        "lastAssistantMessage":"Done\nwith checks", "transcriptPath": "/catalog/transcript.jsonl"});
    let update = observed(run.ingest("current", Source::Hook, &stop, 14));
    assert_eq!(update.work_status, Some(WorkStatus::Idle));
    assert_eq!(update.title_candidate.as_deref(), Some("agy-conv-1"));
    assert_eq!(update.transcript_path, Some("/catalog/transcript.jsonl"));
    assert_eq!(update.notification.unwrap().body, "Done with checks");
    assert_eq!(
        update.semantic_events[0].payload["terminalState"],
        "waiting"
    );
    assert!(run.can_fork());

    let error = json!({"hookEventName": "Stop", "conversationId": "agy-conv-1", "terminationReason": "ERROR"});
    let update = observed(run.ingest("current", Source::Hook, &error, 15));
    assert_eq!(update.work_status, Some(WorkStatus::Error));
    assert_eq!(update.notification.unwrap().event_type, "error");
    assert_eq!(update.semantic_events[0].payload["terminalState"], "error");
    assert_eq!(
        observed(run.ingest("current", Source::Statusline, &idle, 16)).work_status,
        None
    );
    assert_eq!(run.work_status(), WorkStatus::Error);
    let working = json!({"conversation_id": "agy-conv-1", "agent_state": "working"});
    assert_eq!(
        observed(run.ingest("current", Source::Statusline, &working, 17)).work_status,
        Some(WorkStatus::Running)
    );
    let cancel = json!({"hookEventName": "Stop", "conversationId": "agy-conv-1", "terminationReason": "USER_CANCELED"});
    let update = observed(run.ingest("current", Source::Hook, &cancel, 18));
    assert_eq!(update.work_status, Some(WorkStatus::Idle));
    assert!(update.notification.is_none());
    let invalid = json!({"hookEventName": "PostToolUse", "toolCall": {"name": "write_file"}});
    assert!(
        observed(run.ingest("current", Source::Hook, &invalid, 19))
            .semantic_events
            .is_empty()
    );
    assert_eq!(
        run.ingest("current", Source::Hook, &json!(["invalid"]), 20),
        Outcome::Ignored
    );
}

#[test]
fn antigravity_recovery_fences_runs_and_requires_authoritative_identity() {
    let mut resume = Run::new(
        "resume",
        LaunchMode::Resume("target".into()),
        PermissionMode::Plan,
    );
    let confirmation =
        json!({"conversation_id": "target", "agent_state": "idle", "cycle_mode": "plan"});
    let unrelated =
        json!({"hookEventName": "Stop", "conversationId": "old", "error": "must not escape"});
    let no_identity = json!({"hookEventName": "Stop", "transcriptPath": "/untrusted"});
    let end = json!({"hookEventName": "SessionEnd", "conversationId": "target"});
    assert_eq!(
        resume.ingest("prior-run", Source::Hook, &unrelated, 1),
        Outcome::Ignored
    );
    assert_eq!(
        resume.ingest("resume", Source::Hook, &no_identity, 2),
        Outcome::Ignored
    );
    assert_eq!(
        resume.ingest("resume", Source::Hook, &end, 3),
        Outcome::Ignored
    );
    assert_eq!(resume.deadline_elapsed(), RecoveryState::AwaitingIdentity);
    let update = observed(resume.ingest("resume", Source::Statusline, &confirmation, 4));
    assert_eq!(update.identity.unwrap().conversation_id, "target");
    assert_eq!(resume.recovery_state(), RecoveryState::Confirmed);
    let changed = json!({"hookEventName": "SessionStart", "conversationId": "changed"});
    assert_eq!(
        observed(resume.ingest("resume", Source::Hook, &changed, 5))
            .identity
            .unwrap()
            .conversation_id,
        "changed"
    );
    resume.retire();
    assert_eq!(
        resume.ingest("resume", Source::Hook, &unrelated, 6),
        Outcome::Ignored
    );

    let mut resume = Run::new(
        "fresh-run",
        LaunchMode::Resume("target".into()),
        PermissionMode::Default,
    );
    assert_eq!(
        resume.ingest("fresh-run", Source::Hook, &unrelated, 7),
        Outcome::IdentityMismatch {
            expected: "target".into(),
            actual: "old"
        }
    );
    assert_eq!(
        resume.recovery_state(),
        RecoveryState::Failed(RecoveryFailure::IdentityMismatch)
    );
    assert_eq!(
        resume.ingest("fresh-run", Source::Statusline, &confirmation, 8),
        Outcome::Ignored
    );
    let mut exited = Run::new(
        "exited",
        LaunchMode::Resume("target".into()),
        PermissionMode::Default,
    );
    exited.retire();
    assert_eq!(
        exited.recovery_state(),
        RecoveryState::Failed(RecoveryFailure::Exited)
    );
}

#[test]
fn antigravity_fork_waits_for_parent_idle_sends_once_and_retries() {
    let mode = LaunchMode::Fork("parent".into());
    let auth = json!({"conversation_id": "parent", "agent_state": "authenticating"});
    let parent = json!({"conversation_id": "parent", "agent_state": "idle"});
    let child = json!({"conversation_id": "child", "agent_state": "idle"});
    let mut run = Run::new("fork", mode.clone(), PermissionMode::Default);
    assert_eq!(
        run.ingest("fork", Source::Statusline, &auth, 1),
        Outcome::Ignored
    );
    assert_eq!(
        run.ingest("fork", Source::Statusline, &child, 2),
        Outcome::Ignored
    );
    assert_eq!(
        run.ingest("fork", Source::Statusline, &parent, 3),
        Outcome::SendForkCommand
    );
    assert_eq!(FORK_COMMAND, "/fork\r");
    assert_eq!(
        run.ingest("fork", Source::Statusline, &parent, 4),
        Outcome::Ignored
    );
    assert_eq!(run.conversation_id(), None);
    assert_eq!(
        run.deadline_elapsed(),
        RecoveryState::Failed(RecoveryFailure::ForkTimeout)
    );
    assert_eq!(
        run.ingest("fork", Source::Statusline, &child, 5),
        Outcome::Ignored
    );

    let mut retry = Run::new("retry", mode, PermissionMode::Default);
    assert_eq!(
        retry.ingest("fork", Source::Statusline, &parent, 6),
        Outcome::Ignored
    );
    assert_eq!(
        retry.ingest("retry", Source::Statusline, &parent, 7),
        Outcome::SendForkCommand
    );
    assert_eq!(
        observed(retry.ingest("retry", Source::Statusline, &child, 8))
            .identity
            .unwrap()
            .conversation_id,
        "child"
    );
    assert_eq!(retry.recovery_state(), RecoveryState::Confirmed);
    assert!(retry.can_fork());
    assert_eq!(
        retry.ingest("retry", Source::Statusline, &parent, 9),
        Outcome::Ignored
    );
    assert_eq!(retry.conversation_id(), Some("child"));
}

fn write_metadata(root: &Path, data: &Value) -> Result<()> {
    let path = root.join("cache");
    fs::create_dir_all(&path)?;
    fs::write(
        path.join("conversation_metadata.json"),
        serde_json::to_vec(data)?,
    )?;
    Ok(())
}

fn transcript(root: &Path, id: &str) -> PathBuf {
    root.join("brain")
        .join(id)
        .join(".system_generated/logs/transcript.jsonl")
}

fn create_summaries(root: &Path) -> Result<rusqlite::Connection> {
    let database = rusqlite::Connection::open(root.join("conversation_summaries.db"))?;
    database.execute_batch(
        "CREATE TABLE conversation_summaries (
            conversation_id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            step_count INTEGER NOT NULL DEFAULT 0,
            last_modified_time DATETIME NOT NULL,
            workspace_uris TEXT NOT NULL,
            app_data_dir TEXT NOT NULL DEFAULT '',
            parent_conversation_id TEXT NOT NULL DEFAULT '',
            nesting_depth INTEGER NOT NULL DEFAULT 0
        );",
    )?;
    Ok(database)
}

#[test]
fn antigravity_catalog_replays_provider_history_fixture() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../tooling/native-parity/fixtures/provider-history-reference.json"
    ))?;
    let mut metadata = fixture["datasets"]["antigravity-source"][0]["json"].clone();
    let mut expected = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "provider-source-tabs")
        .unwrap()["expected"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["provider"] == "antigravity")
        .unwrap()
        .clone();
    let temp = sandbox()?;
    let workspace = if cfg!(windows) {
        let workspace = temp.path().join("中文 workspace");
        let uri = Url::from_file_path(&workspace).expect("absolute fixture workspace");
        let conversations = metadata["conversations"].as_object_mut().unwrap();
        for conversation in conversations.values_mut() {
            let paths = conversation["summary"]["WorkspaceURIs"]
                .as_array_mut()
                .unwrap();
            for path in paths {
                match path.as_str().unwrap() {
                    "file:///__fixture__/workspace" => *path = json!(uri.as_str()),
                    "/__fixture__/workspace" => *path = json!(workspace),
                    "/__fixture__/other" => *path = json!(temp.path().join("other")),
                    unexpected => panic!("Unexpected fixture workspace: {unexpected}"),
                }
            }
        }
        for session in expected["sessions"].as_array_mut().unwrap() {
            assert_eq!(session["cwd"], "/__fixture__/workspace");
            session["cwd"] = json!(workspace);
        }
        workspace
    } else {
        PathBuf::from("/__fixture__/workspace")
    };
    write_metadata(temp.path(), &metadata)?;
    let mut catalog = Catalog::new(temp.path());
    let actual = catalog.list(&workspace, ListOptions::default())?;
    assert_eq!(serde_json::to_value(actual)?, expected);
    Ok(())
}

#[test]
fn antigravity_catalog_reads_and_recovers_sqlite_only_history() -> Result<()> {
    let temp = sandbox()?;
    let root = temp.path();
    let cwd = root.join("中文 workspace");
    let uri = Url::from_file_path(&cwd).unwrap();
    let id = "agy-sqlite-only";
    let database = create_summaries(root)?;
    database.execute(
        "INSERT INTO conversation_summaries VALUES
            (?1, '  SQLite title  ', 7, '2026-11-12T10:00:02Z', ?2, 'antigravity-cli', '', 0),
            ('untitled', ' ', -1, 1794477601000, ?3, '', '', 0),
            ('internal-parent', 'Private child', 1, 100, ?2, '', ?1, 0),
            ('internal-depth', 'Private nested', 1, 100, ?2, '', '', 1),
            ('wrong-app', 'Another application', 1, 100, ?2, 'other', '', 0),
            ('../invalid', 'Invalid id', 1, 100, ?2, '', '', 0),
            (NULL, 'Missing id', 1, 100, ?2, '', '', 0)",
        rusqlite::params![id, json!([uri.as_str()]).to_string(), uri.as_str()],
    )?;
    drop(database);

    let path = transcript(root, id);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(
        path,
        concat!(
            "{\"type\":\"USER_INPUT\",\"content\":\"Fixture question\"}\n",
            "{\"type\":\"PLANNER_RESPONSE\",\"content\":\"Fixture SQLite answer\"}\n",
        ),
    )?;
    let mut catalog = Catalog::new(root);
    let list = catalog.list(&cwd, ListOptions::default())?;
    assert_eq!(list.page.total, 2);
    assert_eq!(list.sessions[0].provider_session_id, id);
    assert_eq!(list.sessions[0].title, "SQLite title");
    assert_eq!(list.sessions[0].event_count, 7);
    assert_eq!(list.sessions[0].updated_at.as_i64(), Some(1794477602000));
    assert_eq!(list.sessions[1].provider_session_id, "untitled");
    assert_eq!(list.sessions[1].title, "未命名 Antigravity 会话");
    assert_eq!(list.sessions[1].event_count, 0);
    assert_eq!(list.sessions[1].updated_at.as_i64(), Some(1794477601000));
    let english = Catalog::new(root)
        .with_english_titles()
        .list(&cwd, ListOptions::default())?;
    assert_eq!(english.sessions[1].title, "Untitled Antigravity session");
    let detail = catalog.detail(&cwd, id, DetailOptions::default())?;
    assert_eq!(detail.page.total, 2);
    assert_eq!(detail.events[0].kind, EventKind::User);
    assert_eq!(detail.events[1].kind, EventKind::Assistant);
    assert_eq!(detail.events[1].text, "Fixture SQLite answer");
    let search = catalog.search(&cwd, id, "SQLITE ANSWER", 0, None)?;
    assert_eq!(search.page.total, 1);
    assert_eq!(search.hits[0].event_index, 2);
    assert_eq!(
        catalog.recovery_candidate(&cwd, id)?,
        RecoveryCandidate::AwaitingIdentity(LaunchMode::Resume(id.into()))
    );
    assert_eq!(
        catalog.auto_title(&cwd, id)?.as_deref(),
        Some("SQLite title")
    );
    assert_eq!(
        catalog.latest_auto_title(id)?.as_deref(),
        Some("SQLite title")
    );
    assert_eq!(catalog.latest_auto_title("untitled")?, None);
    for hidden in ["internal-parent", "internal-depth", "wrong-app"] {
        assert_eq!(
            catalog.recovery_candidate(&cwd, hidden)?,
            RecoveryCandidate::Unavailable
        );
        assert_eq!(catalog.latest_auto_title(hidden)?, None);
    }
    assert_eq!(
        catalog.recovery_candidate(&cwd.join("nested"), id)?,
        RecoveryCandidate::Unavailable
    );
    Ok(())
}

#[test]
fn antigravity_catalog_refreshes_sqlite_wal_alongside_legacy_metadata() -> Result<()> {
    let temp = sandbox()?;
    let root = temp.path();
    let uri = Url::from_file_path(root).unwrap();
    write_metadata(
        root,
        &json!({"conversations": {
            "legacy": {"summary":{"AppDataDir":"antigravity-cli","WorkspaceURIs":[uri.as_str()],
                "Title":"Legacy title","NumSteps":5}},
            "legacy-internal": {"summary":{"AppDataDir":"antigravity-cli","WorkspaceURIs":[uri.as_str()],
                "Internal":true}}
        }}),
    )?;
    let mut catalog = Catalog::new(root);
    assert_eq!(catalog.list(root, ListOptions::default())?.page.total, 1);
    let database = create_summaries(root)?;
    let workspace = json!(uri.as_str()).to_string();
    database.execute(
        "INSERT INTO conversation_summaries
           (conversation_id, title, step_count, last_modified_time, workspace_uris)
         VALUES ('legacy', 'SQLite duplicate', 1, 200, ?1),
                ('legacy-internal', 'Hidden', 1, 150, ?1),
                ('sqlite-first', 'First SQLite session', 1, 100, ?1)",
        [&workspace],
    )?;
    database.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")?;
    database.execute(
        "UPDATE conversation_summaries SET title = 'First WAL title'
         WHERE conversation_id = 'sqlite-first'",
        [],
    )?;
    let db_path = root.join("conversation_summaries.db");
    let wal_path = root.join("conversation_summaries.db-wal");
    let original = fs::read(&db_path)?;
    let original_version = fs::metadata(&db_path)?.modified()?;
    let original_wal_size = fs::metadata(&wal_path)?.len();
    let before = catalog.list(root, ListOptions::default())?;
    assert_eq!(before.page.total, 2);
    assert_eq!(before.sessions[0].provider_session_id, "sqlite-first");
    assert_eq!(before.sessions[0].title, "First WAL title");
    assert_eq!(before.sessions[1].title, "Legacy title");
    assert_eq!(before.sessions[1].event_count, 5);

    database.execute(
        "INSERT INTO conversation_summaries
           (conversation_id, title, last_modified_time, workspace_uris)
         VALUES ('sqlite-new', 'WAL title', 300, ?1)",
        [&workspace],
    )?;
    assert_eq!(fs::metadata(&db_path)?.modified()?, original_version);
    assert_eq!(fs::read(db_path)?, original);
    assert!(fs::metadata(wal_path)?.len() > original_wal_size);
    let after = catalog.list(root, ListOptions::default())?;
    assert_eq!(after.page.total, 3);
    assert_eq!(after.sessions[0].provider_session_id, "sqlite-new");
    assert_eq!(
        catalog.latest_auto_title("sqlite-new")?.as_deref(),
        Some("WAL title")
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn antigravity_catalog_rejects_symlinked_summary_files() -> Result<()> {
    use std::os::unix::fs::symlink;

    let temp = sandbox()?;
    let root = temp.path().join("provider");
    fs::create_dir(&root)?;
    let database = create_summaries(temp.path())?;
    database.execute(
        "INSERT INTO conversation_summaries
            (conversation_id, title, last_modified_time, workspace_uris)
         VALUES ('outside', 'Outside title', 100, ?1)",
        [json!([temp.path()]).to_string()],
    )?;
    drop(database);
    let outside = temp.path().join("conversation_summaries.db");
    let db_path = root.join("conversation_summaries.db");
    let mut catalog = Catalog::new(&root);
    symlink(&outside, &db_path)?;
    assert_eq!(
        catalog
            .list(temp.path(), ListOptions::default())?
            .page
            .total,
        0
    );
    assert_eq!(catalog.latest_auto_title("outside")?, None);

    fs::remove_file(&db_path)?;
    fs::copy(&outside, &db_path)?;
    let wal = root.join("conversation_summaries.db-wal");
    symlink(&outside, &wal)?;
    assert_eq!(
        catalog
            .list(temp.path(), ListOptions::default())?
            .page
            .total,
        0
    );
    assert_eq!(catalog.latest_auto_title("outside")?, None);
    fs::remove_file(wal)?;
    assert_eq!(
        catalog
            .list(temp.path(), ListOptions::default())?
            .page
            .total,
        1
    );
    Ok(())
}

#[test]
fn antigravity_catalog_pages_searches_and_recovers_after_partial_writes() -> Result<()> {
    let temp = sandbox()?;
    let workspace = temp.path().join("中文 workspace");
    let root = catalog_root(temp.path(), &environment(&[]));
    fs::create_dir(&workspace)?;
    let cwd = workspace.canonicalize()?;
    let uri = Url::from_file_path(&cwd).expect("absolute fixture workspace URI");
    let other = cwd.join("subdir");
    let id = "agy-transcript";
    let mut catalog = Catalog::new(&root);
    assert_eq!(catalog.list(&cwd, ListOptions::default())?.page.total, 0);
    let metadata = json!({"conversations": {
        id: {"summary": {"AppDataDir": "antigravity-cli", "WorkspaceURIs": [uri.as_str()],
            "Title": "  项目检索  ", "NumSteps": 12, "UpdatedAt": "2026-11-12T10:00:02Z"}},
        "agy-preview": {"summary": {"AppDataDir": "antigravity-cli", "WorkspaceURIs": [cwd],
            "Preview": "  备用   标题 ", "UpdatedAt": "invalid"}, "last_modified_time": 100},
        "agy-other": {"summary": {"AppDataDir": "antigravity-cli", "WorkspaceURIs": [other]}},
        "agy-internal": {"summary": {"AppDataDir": "antigravity-cli", "WorkspaceURIs": [cwd], "Internal": true}},
        "agy-wrong-source": {"summary": {"AppDataDir": "other", "WorkspaceURIs": [cwd]}},
        "../invalid": {"summary": {"AppDataDir": "antigravity-cli", "WorkspaceURIs": [cwd]}}
    }});
    write_metadata(&root, &metadata)?;
    let list = catalog.list(
        &cwd,
        ListOptions {
            limit: Some(1),
            ..Default::default()
        },
    )?;
    assert_eq!(list.sessions[0].provider_session_id, id);
    assert_eq!(list.sessions[0].event_count, 12);
    assert_eq!(list.page.total, 2);
    assert!(list.page.has_more);
    let list = catalog.list(
        &cwd,
        ListOptions {
            offset: 1,
            limit: Some(1),
            ..Default::default()
        },
    )?;
    assert_eq!(list.sessions[0].title, "备用 标题");
    assert_eq!(list.sessions[0].updated_at.as_i64(), Some(100));
    assert!(!list.page.has_more);
    assert_eq!(catalog.auto_title(&cwd, id)?.as_deref(), Some("项目检索"));
    assert_eq!(catalog.auto_title(&cwd, "agy-preview")?, None);
    assert_eq!(
        catalog.recovery_candidate(&cwd, id)?,
        RecoveryCandidate::AwaitingIdentity(LaunchMode::Resume(id.into()))
    );
    assert_eq!(
        catalog.recovery_candidate(&other, id)?,
        RecoveryCandidate::Unavailable
    );
    assert!(
        catalog
            .detail(&other, id, DetailOptions::default())
            .is_err()
    );
    assert!(catalog.search(&cwd, "../invalid", "", 0, None).is_err());
    let missing = catalog.detail(&cwd, id, DetailOptions::default())?;
    assert!(missing.events.is_empty() && missing.page.total == 0);
    assert_eq!(missing.summary.event_count, 12);

    let path = transcript(&root, id);
    fs::create_dir_all(path.parent().unwrap())?;
    let user = json!({"source":"USER_EXPLICIT", "content":" Check control-assets "});
    let internal = json!({"type":"INTERNAL", "content":"control-assets private"});
    let model = json!({"type":"PLANNER_RESPONSE", "content":"已检查文件", "created_at":"2026-11-12T10:00:02Z",
        "tool_calls":[{"name":"read_file", "arguments":{"Path":"/repo/control-assets"}}, {"name":"another", "arguments":" next "}]});
    fs::write(
        &path,
        format!(
            "{user}\r\n\nnot JSON\n{internal}\n{model}\n{{\"source\":\"MODEL\",\"content\":\"half"
        ),
    )?;
    let detail = catalog.detail(
        &cwd,
        id,
        DetailOptions {
            query: " CONTROL-ASSETS ",
            limit: Some(1),
            ..Default::default()
        },
    )?;
    assert_eq!(detail.page.total, 2);
    assert_eq!(detail.summary.event_count, 12);
    assert_eq!(detail.summary.match_count, 2);
    assert_eq!(detail.events[0].index, 2);
    assert_eq!(detail.events[0].kind, EventKind::Tool);
    assert_eq!(detail.events[0].role, "assistant");
    assert_eq!(detail.events[0].tool_name.as_deref(), Some("read_file"));
    assert!(detail.events[0].text.ends_with("another next"));
    assert_eq!(
        detail.events[0].timestamp.as_ref().unwrap().as_i64(),
        Some(1794477602000)
    );
    assert!(detail.events[0].matched);
    assert!(detail.page.has_earlier);
    assert!(!detail.page.has_later);
    let earlier = catalog.detail(
        &cwd,
        id,
        DetailOptions {
            before_event_index: Some(2),
            limit: Some(1),
            ..Default::default()
        },
    )?;
    assert_eq!(earlier.events[0].kind, EventKind::User);
    assert_eq!(earlier.events[0].text, "Check control-assets");
    assert!(earlier.page.has_later);
    let search = catalog.search(&cwd, id, "  CONTROL-ASSETS  ", 1, Some(1))?;
    assert_eq!(search.query, "control-assets");
    assert_eq!(search.page.total, 2);
    assert_eq!(search.hits[0].event_index, 2);
    assert_eq!(
        catalog
            .search(&cwd, id, "control-assets", 99, None)?
            .page
            .offset,
        2
    );
    assert_eq!(
        catalog
            .list(
                &cwd,
                ListOptions {
                    query: "control-assets",
                    scope: SearchScope::Metadata,
                    ..Default::default()
                }
            )?
            .page
            .total,
        0
    );
    assert_eq!(
        catalog
            .list(
                &cwd,
                ListOptions {
                    query: "control-assets",
                    ..Default::default()
                }
            )?
            .sessions[0]
            .match_count,
        2
    );
    let renamed = |session: &str| (session == id).then(|| "Renamed".to_owned());
    assert_eq!(
        catalog
            .list(
                &cwd,
                ListOptions {
                    query: "renamed",
                    title_for_session: Some(&renamed),
                    ..Default::default()
                }
            )?
            .sessions[0]
            .match_count,
        1
    );

    OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(b" ready\"}")?;
    let around = catalog.detail(
        &cwd,
        id,
        DetailOptions {
            around_event_index: Some(3),
            limit: Some(2),
            ..Default::default()
        },
    )?;
    assert_eq!(around.page.total, 3);
    assert_eq!(around.events[1].text, "half ready");
    assert_eq!(around.events[1].kind, EventKind::Assistant);
    fs::remove_file(path)?;
    assert_eq!(
        catalog
            .detail(&cwd, id, DetailOptions::default())?
            .page
            .total,
        0
    );
    fs::write(
        root.join("cache/conversation_metadata.json"),
        b"{\"conversations\":",
    )?;
    assert_eq!(catalog.list(&cwd, ListOptions::default())?.page.total, 0);
    write_metadata(&root, &metadata)?;
    assert_eq!(catalog.list(&cwd, ListOptions::default())?.page.total, 2);
    Ok(())
}

#[test]
fn antigravity_catalog_latest_title_prefers_cli_database_and_excludes_internal() -> Result<()> {
    let temp = sandbox()?;
    write_metadata(
        temp.path(),
        &json!({"conversations": {
            "meta": {"summary":{"Title":"  元数据标题  "}},
            "live": {"summary":{"Title":"旧标题"}},
            "preview": {"summary":{"Title":" ", "Preview":"placeholder"}},
            "internal": {"summary":{"Title":"隐藏", "Internal":true}},
            "wrong-source": {"summary":{"Title":"回退"}}
        }}),
    )?;
    let db_path = temp.path().join("conversation_summaries.db");
    let database = rusqlite::Connection::open(&db_path)?;
    database.execute_batch(
        "
        CREATE TABLE conversation_summaries (conversation_id TEXT, title TEXT, app_data_dir TEXT);
        INSERT INTO conversation_summaries VALUES
           ('live', '  实时标题  ', 'antigravity-cli'),
           ('internal', '隐藏', 'antigravity-cli'),
           ('wrong-source', '隐藏', 'other-app'),
           ('db-only', '数据库标题', '');
    ",
    )?;
    drop(database);
    let original = fs::read(&db_path)?;
    let catalog = Catalog::new(temp.path());
    for (id, expected) in [
        ("meta", Some("元数据标题")),
        ("live", Some("实时标题")),
        ("db-only", Some("数据库标题")),
        ("wrong-source", Some("回退")),
        ("preview", None),
        ("internal", None),
        ("../../escape", None),
    ] {
        assert_eq!(catalog.latest_auto_title(id)?.as_deref(), expected, "{id}");
    }
    assert_eq!(fs::read(db_path)?, original);
    Ok(())
}

#[cfg(unix)]
#[test]
fn antigravity_catalog_will_not_index_transcripts_outside_the_root() -> Result<()> {
    use std::os::unix::fs::symlink;
    let temp = sandbox()?;
    let root = temp.path().join("provider");
    let id = "agy";
    write_metadata(
        &root,
        &json!({"conversations": {id: {"summary": {
            "AppDataDir": "antigravity-cli", "WorkspaceURIs": [temp.path()]
        }}}}),
    )?;
    let outside = temp.path().join("outside");
    let path = transcript(&root, id);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(
        &outside,
        "{\"type\":\"USER_INPUT\",\"content\":\"outside\"}",
    )?;
    symlink(&outside, &path)?;
    let mut catalog = Catalog::new(root);
    assert_eq!(
        catalog
            .detail(temp.path(), id, DetailOptions::default())?
            .page
            .total,
        0
    );
    assert_eq!(
        catalog
            .search(temp.path(), id, "outside", 0, None)?
            .page
            .total,
        0
    );
    Ok(())
}
