use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::Path,
};

use anyhow::{Context, Result};
use matou_native_core::{
    provider_launch::{attach_claude_settings, prepare_claude_launch},
    pty::ShellLaunch,
};
use serde_json::{Value, json};

const MARKERS: [&str; 13] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_PID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_NAME",
    "CLAUDE_CODE_SESSION_KIND",
    "CLAUDE_BG_SOURCE",
    "CLAUDE_BG_BACKEND",
    "CLAUDE_BG_RENDEZVOUS_SOCK",
    "CLAUDE_CODE_RESUME_PROMPT",
    "CLAUDE_CODE_RESUME_INTERRUPTED_TURN",
];

fn config(providers: Value, active: &str) -> Value {
    json!({
        "version": 1,
        "revision": 7,
        "providers": {"claude-code": providers},
        "activeProviderIds": {"claude-code": active}
    })
}

fn prepare(root: &Path, binding: Value, overrides: &[(&str, &str)]) -> Result<ShellLaunch> {
    let mut inherited = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    inherited.extend(
        overrides
            .iter()
            .map(|&(key, value)| (OsString::from(key), OsString::from(value))),
    );
    Ok(prepare_claude_launch(root, root, &binding, None, inherited, 80, 24)?.launch)
}

fn assert_env(launch: &ShellLaunch, name: &str, expected: &str) {
    // Keep potentially sensitive values out of assertion failures.
    assert!(
        launch
            .env
            .get(OsStr::new(name))
            .is_some_and(|value| value == expected),
        "{name}"
    );
}

#[test]
fn resume_id_follows_model_and_effort_and_precedes_permission_flags() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let executable = std::env::current_exe()?;
    let settings = root.join("Claude settings.json");
    let metadata = json!({"model": "fixture-model", "permissionMode": "plan"});
    fs::write(
        root.join("provider-config.json"),
        config(
            json!([{
                "id": "fixture", "model": "fixture-model", "endpoint": "https://example.invalid",
                "reasoningEffort": "high",
            }]),
            "fixture",
        )
        .to_string(),
    )?;
    let mut launch = prepare_claude_launch(
        root,
        root,
        &metadata,
        Some("Original-ID"),
        BTreeMap::from([("MATOU_CLAUDE_COMMAND".into(), executable.into_os_string())]),
        80,
        24,
    )?
    .launch;
    attach_claude_settings(&mut launch, &settings)?;
    assert_eq!(
        launch.argv,
        [
            "--settings",
            settings.to_str().context("settings path")?,
            "--model",
            "fixture-model",
            "--effort",
            "high",
            "--resume",
            "Original-ID",
            "--permission-mode",
            "plan",
        ]
    );
    Ok(())
}

#[test]
fn missing_config_uses_official_without_setting_credentials() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let launch = prepare(temp.path(), Value::Null, &[])?;
    assert!(launch.argv.is_empty());
    assert_env(&launch, "ANTHROPIC_BASE_URL", "https://api.anthropic.com");
    for key in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
        assert!(!launch.env.contains_key(OsStr::new(key)));
    }
    assert!(!temp.path().join("provider-config.json").exists());

    let inherited = prepare(
        temp.path(),
        Value::Null,
        &[("ANTHROPIC_API_KEY", "inherited")],
    )?;
    assert_env(&inherited, "ANTHROPIC_API_KEY", "inherited");
    assert!(
        !inherited
            .env
            .contains_key(OsStr::new("ANTHROPIC_AUTH_TOKEN"))
    );
    Ok(())
}

#[test]
fn projects_model_effort_and_permissions_in_order() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("provider-config.json"),
        config(json!([{
            "id": "active", "model": " claude-opus-current ", "endpoint": "https://example.invalid",
            "reasoningEffort": "medium",
            "effortMapping": {"opus": "high", "haiku": "follow"},
            "modelMapping": {"haiku": "custom-small"}
        }]), "active").to_string(),
    )?;
    for (permission, suffix) in [
        (None, &[][..]),
        (Some("default"), &[][..]),
        (Some("ask"), &[][..]),
        (Some("auto"), &["--permission-mode", "auto"][..]),
        (
            Some("acceptEdits"),
            &["--permission-mode", "acceptEdits"][..],
        ),
        (Some("plan"), &["--permission-mode", "plan"][..]),
        (
            Some("bypassPermissions"),
            &["--dangerously-skip-permissions"][..],
        ),
    ] {
        let launch = prepare(temp.path(), json!({"permissionMode": permission}), &[])?;
        assert_eq!(
            launch.argv,
            ["--model", "claude-opus-current", "--effort", "high"]
                .into_iter()
                .chain(suffix.iter().copied())
                .collect::<Vec<_>>()
        );
    }
    let launch = prepare(temp.path(), json!({"model": "custom-small[1M]"}), &[])?;
    assert_eq!(launch.argv, ["--model", "custom-small[1M]"]);
    let launch = prepare(temp.path(), json!({"model": ""}), &[])?;
    assert_eq!(launch.argv, ["--effort", "medium"]);
    Ok(())
}

#[test]
fn projects_bound_api_provider_and_sanitizes_environment() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("provider-config.json");
    let key = "fixture-credential-never-log";
    let stored = config(
        json!([
            {"id": "first", "model": "first-model", "endpoint": ""},
            {"id": "active", "model": "active-model", "endpoint": ""},
            {"id": "bound", "kind": "api-key", "model": "bound-model",
             "endpoint": "https://example.invalid//", "apiKey": key,
             "modelMapping": {"opus": "big", "sonnet": "balanced", "haiku": ""}}
        ]),
        "active",
    )
    .to_string();
    fs::write(&path, &stored)?;

    let mut environment: Vec<_> = MARKERS.into_iter().map(|key| (key, "inherited")).collect();
    environment.extend([
        ("NO_COLOR", "1"),
        ("TERM", "dumb"),
        ("HTTPS_PROXY", "http://localhost:1234"),
        ("CLAUDE_CONFIG_DIR", "isolated"),
    ]);
    let launch = prepare(
        temp.path(),
        json!({"providerConfigId": "bound"}),
        &environment,
    )?;
    assert_eq!(launch.argv, ["--model", "bound-model"]);
    for (name, value) in [
        ("ANTHROPIC_API_KEY", key),
        ("ANTHROPIC_AUTH_TOKEN", key),
        ("ANTHROPIC_BASE_URL", "https://example.invalid/"),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", "big"),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", "balanced"),
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("FORCE_COLOR", "1"),
        ("HTTPS_PROXY", "http://localhost:1234"),
        ("CLAUDE_CONFIG_DIR", "isolated"),
    ] {
        assert_env(&launch, name, value);
    }
    for name in MARKERS
        .into_iter()
        .chain(["NO_COLOR", "ANTHROPIC_DEFAULT_HAIKU_MODEL"])
    {
        assert!(!launch.env.contains_key(OsStr::new(name)), "{name}");
    }
    assert!(!temp.path().join("shell-integration").exists());
    assert_eq!(
        prepare(temp.path(), Value::Null, &[])?.argv,
        ["--model", "active-model"]
    );

    let error = prepare(temp.path(), json!({"providerConfigId": "removed"}), &[])
        .err()
        .context("missing binding should fail")?;
    assert_eq!(error.to_string(), "会话绑定的供应商配置不存在");
    assert!(!format!("{error:?}").contains(key));
    assert!(fs::read_to_string(&path)? == stored);
    let stale = stored.replace("\"claude-code\":\"active\"", "\"claude-code\":\"stale\"");
    fs::write(&path, &stale)?;
    assert_eq!(
        prepare(temp.path(), Value::Null, &[])?.argv,
        ["--model", "first-model"]
    );
    assert!(fs::read_to_string(&path)? == stale);
    Ok(())
}

#[test]
fn configuration_errors_never_echo_the_input() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("provider-config.json");
    let secret = "value-within-invalid-input";
    let malformed = format!(r#"{{"apiKey":"{secret}""#);
    fs::write(&path, &malformed)?;
    let error = prepare(temp.path(), Value::Null, &[])
        .err()
        .context("invalid JSON should fail")?;
    assert!(
        error
            .to_string()
            .contains("provider-config.json: invalid JSON")
    );
    assert!(!format!("{error:?}").contains(secret));
    assert!(fs::read_to_string(&path)? == malformed);

    fs::write(
        &path,
        config(
            json!([{
                "id": "bad", "model": "", "endpoint": "", "apiKey": [secret]
            }]),
            "bad",
        )
        .to_string(),
    )?;
    let error = prepare(temp.path(), Value::Null, &[])
        .err()
        .context("invalid credential type should fail")?;
    assert!(
        error
            .to_string()
            .contains("provider-config.json: invalid Claude Code provider entry")
    );
    assert!(!format!("{error:?}").contains(secret));
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn isolated_path_resolution_never_reads_login_rc() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let marker = root.join("probe-ran");
    fs::write(
        root.join(".zshrc"),
        "printf invoked > \"$ZDOTDIR/probe-ran\"\nexport PATH=/usr/bin:/bin\n",
    )?;
    let environment = |isolated| {
        BTreeMap::from([
            ("HOME".into(), root.as_os_str().into()),
            ("ZDOTDIR".into(), root.as_os_str().into()),
            ("SHELL".into(), "/bin/zsh".into()),
            ("PATH".into(), OsString::new()),
            ("MATOU_CLAUDE_COMMAND".into(), "true".into()),
            (
                "MATOU_DISABLE_LOGIN_PATH_PROBE".into(),
                if isolated { "1" } else { "0" }.into(),
            ),
        ])
    };
    let error = prepare_claude_launch(root, root, &Value::Null, None, environment(true), 80, 24)
        .err()
        .context("closed PATH should fail without a probe")?;
    assert!(error.to_string().contains("PATH"));
    assert!(!marker.exists());

    let launch = prepare_claude_launch(root, root, &Value::Null, None, environment(false), 80, 24)?;
    assert_eq!(launch.launch.executable, Path::new("/usr/bin/true"));
    assert!(marker.exists());
    Ok(())
}

#[test]
fn settings_precede_existing_flags_and_bypass_only_loopback() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut launch = prepare(
        temp.path(),
        json!({"model": "fixture-model", "permissionMode": "plan"}),
        &[
            ("HTTPS_PROXY", "http://localhost:1234"),
            ("NO_PROXY", "example.invalid"),
        ],
    )?;
    let settings = temp.path().join("private settings.json");
    attach_claude_settings(&mut launch, &settings)?;
    assert_eq!(
        &launch.argv[..2],
        ["--settings", settings.to_str().unwrap()]
    );
    assert_eq!(
        &launch.argv[2..],
        ["--model", "fixture-model", "--permission-mode", "plan"]
    );
    assert_env(&launch, "HTTPS_PROXY", "http://localhost:1234");
    #[cfg(not(windows))]
    for name in ["NO_PROXY", "no_proxy"] {
        assert_env(&launch, name, "example.invalid,127.0.0.1");
    }
    #[cfg(windows)]
    assert!(launch.env.iter().any(|(key, value)| {
        key.to_str()
            .is_some_and(|key| key.eq_ignore_ascii_case("no_proxy"))
            && value == "example.invalid,127.0.0.1"
    }));
    Ok(())
}
