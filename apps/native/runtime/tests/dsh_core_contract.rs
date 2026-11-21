use std::{collections::BTreeMap, ffi::OsStr, path::Path};

use matou_native_runtime::{
    dsh_events::{DshEventOutcome, DshEvents, DshUpdate, DshWorkStatus},
    dsh_launch::{self, DshLaunchInput},
};
use serde_json::{Value, json};

const EVENTS_FIXTURE: &str =
    include_str!("../../../../tooling/native-parity/fixtures/provider-events-reference.json");
#[cfg(unix)]
const HISTORY_FIXTURE: &str =
    include_str!("../../../../tooling/native-parity/fixtures/provider-history-reference.json");

#[test]
fn dsh_shell_upgrade_matches_only_frozen_forms() {
    for command in [
        "dsh",
        "dsh dsh-tui",
        "dsh --profile dsh-tui",
        " \u{feff}dsh\t --profile   dsh-tui\u{a0}",
    ] {
        assert!(dsh_launch::matches_shell_command(command), "{command:?}");
    }
    for command in [
        "",
        "DSH",
        "dsh-tui",
        "./dsh",
        "/bin/dsh",
        "sudo dsh",
        "echo dsh",
        "MATOU_DSH_COMMAND=test dsh",
        "'dsh'",
        "dsh --profile=dsh-tui",
        "dsh --profile other",
        "dsh dsh-tui --resume session",
        "dsh --resume session",
        "dsh --help",
        "dsh # comment",
        "dsh; pwd",
        "dsh | cat",
        "dsh && pwd",
        "dsh\u{85}",
    ] {
        assert!(!dsh_launch::matches_shell_command(command), "{command:?}");
    }
}

fn launch_input(cwd: &Path) -> DshLaunchInput<'_> {
    DshLaunchInput {
        cwd,
        patch_path: None,
        resume_session_id: None,
        inherited_environment: BTreeMap::new(),
        run_environment: BTreeMap::new(),
    }
}

#[test]
fn dsh_missing_cli_uses_frozen_localized_messages() {
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (locale, expected) in [
        (
            "zh-CN",
            "未检测到 DSH（命令：dsh）。请先安装并确保 `dsh` 在 PATH 中，再重试。",
        ),
        (
            "en",
            "DSH CLI (dsh) was not found. Install it and ensure `dsh` is on PATH, then try again.",
        ),
    ] {
        let mut input = launch_input(cwd);
        input
            .inherited_environment
            .insert("MATOU_LOCALE".into(), locale.into());
        let error = dsh_launch::prepare(input)
            .err()
            .expect("empty PATH must not find DSH");
        assert_eq!(error.to_string(), expected);
    }
}

#[cfg(unix)]
#[test]
fn dsh_stub_receives_frozen_argv_cwd_and_environment() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path();
    let executable = cwd.join("stub dsh");
    fs::write(&executable, r#"#!/bin/sh
if [ "$1" = dsh-tui ] && [ "$2" = --dump-config ]; then
  printf '%s\n' '- id: dsh-tui' '  config:' '    model: stub'
  exit 0
fi
printf 'cwd=%s\nterm=%s\ntz=%s\nbackup=%s\n' "$PWD" "$TERM" "${TZ-unset}" "${DSH_MATOU_CONTROL_TOKEN-unset}"
printf 'arg=%s\n' "$@"
"#).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(cwd.join("dsh")).unwrap();
    let bin = cwd.join("bin");
    fs::create_dir(&bin).unwrap();
    let default_executable = bin.join("dsh");
    fs::copy(&executable, &default_executable).unwrap();

    let mut input = launch_input(cwd);
    input
        .inherited_environment
        .insert("PATH".into(), std::env::join_paths([cwd, &bin]).unwrap());
    let fresh = dsh_launch::prepare(input).unwrap();
    assert_eq!(fresh.executable, default_executable);
    assert_eq!(fresh.argv, ["dsh-tui"]);
    assert_eq!(fresh.mode.expected_session_id(), None);
    assert!(!fresh.environment.contains_key(OsStr::new("TZ")));

    let history: Value = serde_json::from_str(HISTORY_FIXTURE).unwrap();
    let history_case = history["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "dsh-index-tools-and-search")
        .unwrap();
    let expected_id = history_case["expected"]["steps"][0]["sessions"][0]["providerSessionId"]
        .as_str()
        .unwrap();
    let patch_path = cwd.join("current run.patch.yml");
    let mut input = launch_input(cwd);
    input.patch_path = Some(&patch_path);
    input.resume_session_id = Some(expected_id);
    input.inherited_environment.extend([
        ("MATOU_DSH_COMMAND".into(), "./stub dsh".into()),
        ("TZ".into(), "Asia/Singapore".into()),
        ("TERM".into(), "dumb".into()),
        ("DSH_HOME".into(), cwd.join(".fixture-dsh").into_os_string()),
        ("CLAUDE_CODE_CHILD_SESSION".into(), "previous-agent".into()),
        ("DSH_MATOU_CONTROL_TOKEN".into(), "stale-fallback".into()),
        ("MATOU_CONTROL_TOKEN".into(), "stale-primary".into()),
        ("MATOU_PROVIDER_HOOK_URL".into(), "stale-hook".into()),
    ]);
    input.run_environment.insert(
        "MATOU_PROVIDER_HOOK_URL".into(),
        "http://127.0.0.1:1/hooks/current".into(),
    );
    let plan = dsh_launch::prepare(input).unwrap();
    assert_eq!(plan.mode.expected_session_id(), Some(expected_id));
    assert_eq!(plan.cwd, cwd);
    assert_eq!(
        plan.executable.components().collect::<std::path::PathBuf>(),
        executable
    );
    assert_eq!(
        plan.environment[OsStr::new("DSH_HOME")],
        cwd.join(".fixture-dsh")
    );
    assert_eq!(
        plan.environment[OsStr::new("MATOU_PROVIDER_HOOK_URL")],
        "http://127.0.0.1:1/hooks/current"
    );
    assert!(
        !plan
            .environment
            .contains_key(OsStr::new("CLAUDE_CODE_CHILD_SESSION"))
    );
    assert!(dsh_launch::control_token(&plan.environment).is_none());
    assert!(
        !patch_path.exists(),
        "preparing a plan does not create a file"
    );

    let run_stub = |args: &[&OsStr]| {
        let output = Command::new(&plan.executable)
            .args(args)
            .current_dir(&plan.cwd)
            .env_clear()
            .envs(&plan.environment)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    let dump = run_stub(&dsh_launch::DUMP_CONFIG_ARGS.map(OsStr::new));
    let patch = dsh_launch::observation_patch(
        &dump,
        "/fixture/bridge.mjs",
        "http://127.0.0.1:1/hooks/current",
        "run-a",
    )
    .unwrap();
    assert!(patch.contains("    model: stub\n\n    backend: dsh\n"));
    let output = run_stub(
        &plan
            .argv
            .iter()
            .map(|arg| arg.as_os_str())
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        output,
        format!(
            "cwd={}\nterm=xterm-256color\ntz=Asia/Singapore\nbackup=unset\narg=dsh-tui\narg=--patch\narg={}\narg=--resume\narg={expected_id}\n",
            cwd.display(),
            patch_path.display()
        )
    );

    let mut events = DshEvents::new("run-a", plan.mode.expected_session_id());
    let mut ready = ready_event("run-a", expected_id, 1, false);
    assert_eq!(events.ingest(&ready), DshEventOutcome::Ignored);
    ready["persisted"] = json!(true);
    let update = observed(&mut events, &ready, DshWorkStatus::Idle);
    assert_eq!(update.identity.unwrap().session_id, expected_id);
}

#[test]
fn dsh_resume_build_rejects_invalid_history_identity() {
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
    for id in [
        "",
        "  ",
        "../other",
        "path/other",
        "-option",
        "bad id",
        &"a".repeat(161),
    ] {
        let mut input = launch_input(cwd);
        input.resume_session_id = Some(id);
        assert_eq!(
            dsh_launch::prepare(input).err().unwrap().to_string(),
            "Invalid DSH session ID",
            "{id:?}"
        );
    }
}

#[test]
fn dsh_patch_preserves_literal_config_and_limits_observer_changes() {
    let dump = "- id: dsh-tui
  inject:
    - agents
    - matouDshBridge
    - agents
  config:
    model: native-model
    preset: !!js process.env.DSH_PRESET ?? undefined
    backend: original
    taskNotifications: all
    upstreamRetry:
      maxRetries: 5
    fullscreen: true
- id: other-plugin
  config:
    backend: untouched
";
    let patch = dsh_launch::observation_patch(
        dump,
        "/assets/bridge \"test\".mjs",
        "http://127.0.0.1:1/hooks/current",
        "run-a",
    )
    .unwrap();
    assert_eq!(
        patch,
        "- insert:
    - id: matou-dsh-observer
      name: \"/assets/bridge \\\"test\\\".mjs\"
      config:
        hookUrl: \"http://127.0.0.1:1/hooks/current\"
        runId: \"run-a\"
- id: dsh-tui
  inject:
    - agents
    - matouDshBridge
  config:
    model: native-model
    preset: !!js process.env.DSH_PRESET ?? undefined
    fullscreen: true
    backend: dsh
    taskNotifications: \"off\"
    upstreamRetry: false
"
    );
    for invalid in [
        "- id: other\n",
        "- id: dsh-tui\n  config: !!js makeConfig()\n",
        "- id: dsh-tui\n  inject: !!js makeInject()\n  config:\n",
        "- id: dsh-tui\n  inject:\n    - [agents]\n  config:\n",
    ] {
        assert!(dsh_launch::observation_patch(invalid, "bridge", "url", "run").is_err());
    }
}

#[test]
fn dsh_control_instructions_and_token_are_scoped_to_the_managed_run() {
    let mut environment = BTreeMap::from([
        ("MATOU_CONTROL_PROTOCOL".into(), "1".into()),
        ("MATOU_CONTROL_CALLER_RUN".into(), "run-a".into()),
        (
            "MATOU_CONTROL_CALLER_SESSION".into(),
            "fixture-private-session".into(),
        ),
        (
            "MATOU_CONTROL_ENDPOINT".into(),
            "/fixture/private-control.sock".into(),
        ),
        (
            "MATOU_CONTROL_NODE_EXECUTABLE".into(),
            "/fixture/node".into(),
        ),
        ("MATOU_CONTROL_TOKEN".into(), "fixture-private-token".into()),
    ]);
    let instructions = dsh_launch::system_instructions(&environment, "run-a").unwrap();
    assert!(instructions.contains("Matou-managed DSH terminal"));
    assert!(instructions.contains("mt identify --json"));
    for secret in [
        "fixture-private-token",
        "fixture-private-session",
        "/fixture/private-control.sock",
        "run-a",
    ] {
        assert!(!instructions.contains(secret));
    }
    assert!(dsh_launch::system_instructions(&environment, "old-run").is_none());
    assert_eq!(
        dsh_launch::foreground_shell_token(&environment, "run-a", true),
        Some((
            dsh_launch::FALLBACK_CONTROL_TOKEN,
            OsStr::new("fixture-private-token")
        ))
    );
    assert!(dsh_launch::foreground_shell_token(&environment, "run-a", false).is_none());
    assert!(dsh_launch::foreground_shell_token(&environment, "old-run", true).is_none());

    environment.insert(
        dsh_launch::FALLBACK_CONTROL_TOKEN.into(),
        "fixture-fallback".into(),
    );
    assert_eq!(
        dsh_launch::control_token(&environment),
        Some(OsStr::new("fixture-private-token"))
    );
    environment.insert("MATOU_CONTROL_TOKEN".into(), "".into());
    assert_eq!(
        dsh_launch::control_token(&environment),
        Some(OsStr::new("fixture-fallback"))
    );
    assert!(dsh_launch::system_instructions(&environment, "run-a").is_none());
    assert!(dsh_launch::foreground_shell_token(&environment, "run-a", true).is_none());
}

fn ready_event(run: &str, session: &str, sequence: u64, persisted: bool) -> Value {
    json!({
        "dsh_observation": 1, "run_id": run, "foreground": true,
        "sequence": sequence, "generation": 1,
        "session_id": session, "event_type": "ready", "ready": true, "persisted": persisted
    })
}

fn observed<'a>(
    events: &mut DshEvents,
    payload: &'a Value,
    status: DshWorkStatus,
) -> DshUpdate<'a> {
    let DshEventOutcome::Observed(update) = events.ingest(payload) else {
        panic!("expected accepted observation: {payload}");
    };
    assert_eq!(update.work_status, status);
    update
}

#[test]
fn dsh_restore_requires_current_foreground_ready_and_retires_on_wrong_identity() {
    let mut events = DshEvents::new("run-a", Some("dsh-expected"));
    for rejected in [
        json!("ready dsh-expected"),
        json!({"session_id": "dsh-expected", "type": "ready"}),
        ready_event("old-run", "dsh-expected", 10, true),
        ready_event("run-a", "dsh-expected", 10, false),
    ] {
        assert_eq!(events.ingest(&rejected), DshEventOutcome::Ignored);
    }
    let mut wrong = ready_event("run-a", "dsh-wrong", 1, false);
    wrong["foreground"] = json!(false);
    assert_eq!(events.ingest(&wrong), DshEventOutcome::Ignored);
    wrong["foreground"] = json!(true);
    wrong["ready"] = json!(false);
    assert_eq!(events.ingest(&wrong), DshEventOutcome::Ignored);
    wrong["ready"] = json!(true);
    assert_eq!(
        events.ingest(&wrong),
        DshEventOutcome::IdentityMismatch {
            expected_session_id: "dsh-expected".into(),
            actual_session_id: "dsh-wrong"
        }
    );
    assert_eq!(
        events.ingest(&ready_event("run-a", "dsh-expected", 2, true)),
        DshEventOutcome::Ignored
    );
}

#[test]
fn dsh_ready_forwards_cwd_and_hud_and_deduplicates_title_after_restore() {
    let mut events = DshEvents::new("run-a", Some("dsh-expected"));
    let mut ready = ready_event("run-a", "dsh-expected", 1, true);
    ready["cwd"] = json!("/fixture/project");
    ready["hud"] = json!({"title": "  Fixture title  ", "model": "fixture-model"});
    let update = observed(&mut events, &ready, DshWorkStatus::Idle);
    assert_eq!(update.identity.unwrap().cwd, Some("/fixture/project"));
    assert_eq!(update.hud, ready["hud"].as_object());
    assert_eq!(update.title, Some("Fixture title"));

    ready["sequence"] = json!(2);
    let update = observed(&mut events, &ready, DshWorkStatus::Idle);
    assert!(update.identity.is_none());
    assert!(update.title.is_none());

    ready["session_id"] = json!("dsh-next");
    ready["sequence"] = json!(3);
    ready["generation"] = json!(2);
    let update = observed(&mut events, &ready, DshWorkStatus::Idle);
    assert_eq!(update.identity.unwrap().session_id, "dsh-next");
    assert_eq!(update.title, Some("Fixture title"));
}

#[test]
fn dsh_only_whole_turn_errors_stick_until_retry_and_cancellation_is_silent() {
    use DshWorkStatus::{Error, Idle, Running};

    let mut events = DshEvents::new("run-a", None);
    observed(&mut events, &ready_event("run-a", "dsh-a", 1, true), Idle);
    for (index, (kind, data, agent_status, status, notification)) in [
        ("turn/start", json!({"turn": 1}), None, Running, None),
        (
            "tool/result",
            json!({"error": "tool error"}),
            None,
            Running,
            None,
        ),
        (
            "approval/asked",
            json!({"id": "audit-only"}),
            None,
            Running,
            None,
        ),
        (
            "turn/end",
            json!({"turn": 1, "reason": {"kind": "error", "error": {"code": "NETWORK"}}}),
            None,
            Error,
            Some(("error", "NETWORK")),
        ),
        ("agent/status", json!({}), Some("idle"), Error, None),
        (
            "turn/end",
            json!({"turn": 1, "reason": {"kind": "completed"}}),
            None,
            Error,
            None,
        ),
        ("turn/start", json!({"turn": 2}), None, Running, None),
        (
            "turn/end",
            json!({"turn": 2, "reason": {"kind": "aborted"}}),
            None,
            Idle,
            None,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let payload = json!({
            "dsh_observation": 1, "run_id": "run-a", "foreground": true, "generation": 1,
            "sequence": index + 2, "session_id": "dsh-a", "event_type": kind,
            "event_data": data, "agent_status": agent_status
        });
        let update = observed(&mut events, &payload, status);
        assert_eq!(
            update
                .notification
                .as_ref()
                .map(|notice| (notice.event_type, notice.body.as_str())),
            notification
        );
    }
}

#[test]
fn dsh_event_reducer_matches_all_frozen_provider_hook_steps() {
    let fixture: Value = serde_json::from_str(EVENTS_FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut count = 0;
    for case in cases
        .iter()
        .filter(|case| case["input"]["provider"] == "dsh")
    {
        count += 1;
        let input = &case["input"];
        let registration = &input["registration"];
        let run = registration["runId"].as_str().unwrap();
        let session = registration["sessionId"].as_str().unwrap();
        let mut reducer = DshEvents::new(run, registration["expectedProviderSessionId"].as_str());
        let stimuli = input["steps"].as_array().unwrap();
        let outputs = case["expected"]["steps"].as_array().unwrap();
        let mut work_status = input["initialWorkStatus"].clone();
        let mut hud_status = json!("idle");
        assert_eq!(stimuli.len(), outputs.len());
        for (index, (step, expected)) in stimuli.iter().zip(outputs).enumerate() {
            let (mut writes, mut records, mut mismatches, mut notices, mut statuses) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let mut hud_publishes = 0;
            match reducer.ingest(&step["payload"]) {
                DshEventOutcome::Ignored => {}
                DshEventOutcome::IdentityMismatch {
                    expected_session_id,
                    actual_session_id,
                } => {
                    mismatches.push(json!({
                        "runId": run, "sessionId": session, "provider": "dsh",
                        "expectedProviderSessionId": expected_session_id, "actualProviderSessionId": actual_session_id,
                        "eventName": "ready"
                    }));
                }
                DshEventOutcome::Observed(update) => {
                    hud_publishes += 1;
                    work_status = json!(update.work_status);
                    hud_status = work_status.clone();
                    statuses.push(work_status.clone());
                    if let Some(identity) = update.identity {
                        writes.push(json!({
                            "sessionId": session, "provider": "dsh", "providerSessionId": identity.session_id,
                            "hookEvent": "ready", "provisional": !identity.persisted
                        }));
                        records.push(json!({
                            "runId": run, "sessionId": session, "provider": "dsh", "providerSessionId": identity.session_id,
                            "eventName": "ready", "resumable": identity.persisted
                        }));
                    }
                    if let Some(notice) = update.notification {
                        notices.push(json!({
                            "eventType": "agent.notification", "targetSessionId": session,
                            "runId": run, "provider": "dsh", "event": notice
                        }));
                    }
                }
            }
            let context = format!("{} step {}", case["id"], index + 1);
            for (field, actual) in [
                ("identityWrites", json!(writes)),
                ("identityRecorded", json!(records)),
                ("identityMismatches", json!(mismatches)),
                ("notifications", json!(notices)),
                ("providerWorkStatuses", json!(statuses)),
                ("workStatus", work_status.clone()),
                ("hudTaskStatus", hud_status.clone()),
                ("hudPublishes", json!(hud_publishes)),
            ] {
                assert_eq!(actual, expected[field], "{context} {field}");
            }
        }
    }
    assert_eq!(count, 6, "all frozen DSH cases must exercise the reducer");
}
