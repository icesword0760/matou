use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::Path,
};

use anyhow::{Context, Result};
use matou_native_core::{
    provider_config::{ProviderConfigLocale, ProviderConfigStore},
    provider_launch::{
        attach_claude_settings, parse_claude_role_assignments, prepare_claude_fork,
        prepare_claude_fork_rebuild, prepare_claude_launch, prepare_claude_permission_restart,
        prepare_new_claude_launch, prepare_new_claude_launch_with_budget,
        validate_claude_role_assignments, validate_claude_web_search,
    },
    pty::ShellLaunch,
    sessions::{
        current_interface_locale, read_claude_role_launch_preferences, read_locale_preference,
        save_claude_model_assignments,
    },
    storage::migrate,
};
use rusqlite::Connection;
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

#[test]
fn provider_context_patch_preserves_config_and_reaches_claude_launch() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let root = root.path();
    let path = root.join("provider-config.json");
    let config = config(
        json!([
            {"id": "source", "name": "Source", "endpoint": "https://source.example.invalid",
             "model": "opus", "apiKey": "fixture-key", "unknown": {"keep": true},
             "modelMapping": {"opus": "opus", "sonnet": "sonnet", "haiku": "haiku"},
             "modelMetadata": [{"id": "haiku", "contextWindow": 300_000},
                               {"id": "opus", "contextWindow": 300_000},
                               {"id": "sonnet", "contextWindow": 300_000}]},
            {"id": "target", "name": "Target", "endpoint": "https://target.example.invalid",
             "model": "other", "apiKey": "target-fixture-key",
             "modelMetadata": [{"id": "other", "contextWindow": 200_000}]}
        ]),
        "source",
    );
    fs::write(&path, config.to_string())?;
    let store = ProviderConfigStore::new(root, ProviderConfigLocale::En);
    let patch = |id: &str, value: Value| {
        store.upsert(&json!({"cli": "claude-code", "id": id, "contextWindow": value}))
    };
    assert!(patch("source", json!(99_999)).is_err());
    assert!(patch("source", json!(100_000.5)).is_err());
    assert!(patch("source", json!(9_007_199_254_740_992_u64)).is_err());
    assert!(patch("missing", json!(100_000)).is_err());
    assert_eq!(fs::read_to_string(&path)?, config.to_string());
    let saved = patch("source", json!(700_000))?;
    assert_eq!(saved["contextWindow"], 700_000);
    assert!(saved.get("apiKey").is_none());
    let persisted: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let mut persisted_source = persisted["providers"]["claude-code"][0].clone();
    persisted_source
        .as_object_mut()
        .unwrap()
        .remove("contextWindow");
    assert_eq!(persisted_source, config["providers"]["claude-code"][0]);
    assert_eq!(persisted["activeProviderIds"]["claude-code"], "source");
    assert!(!root.join("provider-config.json.tmp").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    }

    let key = "CLAUDE_CODE_MAX_CONTEXT_TOKENS";
    assert_env(&prepare(root, Value::Null, &[])?, key, "700000");
    patch("target", json!(600_000))?;
    let mut db = Connection::open_in_memory()?;
    migrate(&mut db, 1)?;
    db.execute(
        "INSERT INTO preferences VALUES ('agent.modelPreferences', ?, 1)",
        [json!({
            "claudeRoleAssignments": {"opus": {"providerId": "target", "model": "other"}},
            "compaction": {"mode": "follow"},
            "contextWindows": {"source": "512k", "target": "default"}
        })
        .to_string()],
    )?;
    let preferences = read_claude_role_launch_preferences(&db);
    let launch = prepare_new_claude_launch_with_budget(
        root,
        root,
        &Value::Null,
        (&preferences, &Vec::new),
        BTreeMap::from([(
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        )]),
        80,
        24,
    )?;
    assert_env(&launch.launch, key, "600000");

    assert!(patch("source", Value::Null)?.get("contextWindow").is_none());
    let reopened = ProviderConfigStore::new(root, ProviderConfigLocale::En).snapshot()?;
    assert!(
        reopened["providers"]["claude-code"][0]
            .get("contextWindow")
            .is_none()
    );
    assert!(
        !prepare(root, Value::Null, &[])?
            .env
            .contains_key(OsStr::new(key))
    );
    Ok(())
}

#[test]
fn web_search_preferences_resolve_exactly_for_new_launches_in_two_workspaces() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let data = root.path().join("data");
    let workspace_a = root.path().join("a");
    let workspace_b = root.path().join("b");
    for directory in [&data, &workspace_a, &workspace_b] {
        fs::create_dir(directory)?;
    }
    let config_path = data.join("provider-config.json");
    let mut config = config(
        json!([
            {"id": "work", "name": "工作供应商", "endpoint": "http://127.0.0.1:1/work",
             "apiKey": "work-fixture", "model": "opus",
             "modelMapping": {"opus": "work-opus", "sonnet": "work-sonnet", "haiku": "work-haiku"}},
            {"id": "anthropic-official", "name": "搜索供应商", "endpoint": "https://api.anthropic.com/",
             "builtIn": true, "apiKey": "search-fixture", "model": "default",
             "reasoningEffort": "high", "effortMapping": {"opus": "low"}}
        ]),
        "work",
    );
    fs::write(&config_path, config.to_string())?;
    let db_path = data.join("matou.sqlite");
    let mut db = Connection::open(&db_path)?;
    migrate(&mut db, 1)?;
    let catalog = || {
        vec![json!({
            "id": "opus", "resolvedModel": "claude-search-fixture", "supportedEfforts": ["high"],
            "maxOutputTokens": 1024, "contextWindow": 50_000
        })]
    };
    let assignments = Default::default();
    let payload = json!({"providerId": " anthropic-official ", "model": " opus "});
    let target = validate_claude_web_search(&data, &payload, &assignments, &catalog)?;
    assert_eq!(target.provider_id, "anthropic-official");
    assert_eq!(target.model, "opus");
    assert_eq!(target.effort, None);
    save_claude_model_assignments(&mut db, &assignments, Some(Some(&target)), 2)?;
    let launch = |cwd: &Path| {
        let preferences = read_claude_role_launch_preferences(&Connection::open(&db_path)?);
        prepare_new_claude_launch_with_budget(
            &data,
            cwd,
            &Value::Null,
            (&preferences, &catalog),
            BTreeMap::from([(
                "MATOU_CLAUDE_COMMAND".into(),
                std::env::current_exe()?.into_os_string(),
            )]),
            80,
            24,
        )
    };
    for cwd in [&workspace_a, &workspace_b] {
        let plan = launch(cwd)?;
        assert_eq!(&plan.launch.cwd, cwd);
        let snapshot = plan.gateway.context("search-only snapshot")?;
        assert_eq!(
            [0, 1, 2, 3].map(|index| snapshot.role_target(index).unwrap().model.as_str()),
            ["work-opus", "work-opus", "work-sonnet", "work-haiku"]
        );
        let search = snapshot.web_search.context("search target")?;
        assert_eq!(search.model, "claude-search-fixture");
        assert_eq!(search.connection.api_key, "search-fixture");
        assert_eq!(search.effort.as_deref(), Some("high"));
        assert_eq!(search.max_output_tokens, Some(1024));
    }
    let follow = json!({"providerId": "anthropic-official", "model": "opus", "effort": "follow"});
    let target = validate_claude_web_search(&data, &follow, &assignments, &catalog)?;
    save_claude_model_assignments(&mut db, &assignments, Some(Some(&target)), 3)?;
    assert_eq!(
        launch(&workspace_b)?
            .gateway
            .unwrap()
            .web_search
            .unwrap()
            .effort,
        None
    );

    config["providers"]["claude-code"][0]["modelMapping"]
        .as_object_mut()
        .unwrap()
        .remove("haiku");
    fs::write(&config_path, config.to_string())?;
    // Search-only saves do not depend on whether all three working defaults can start.
    validate_claude_web_search(&data, &follow, &assignments, &catalog)?;
    let error = launch(&workspace_a)
        .err()
        .context("missing role should block startup")?;
    assert_eq!(
        error.to_string(),
        "haiku · 工作供应商: native role model is unavailable; configure this role's model"
    );

    config["providers"]["claude-code"][1]["apiKey"] = Value::Null;
    fs::write(&config_path, config.to_string())?;
    let error = launch(&workspace_b)
        .err()
        .context("unavailable search target should block startup")?;
    assert_eq!(
        error.to_string(),
        "网页搜索 · 搜索供应商: model target is unavailable"
    );
    assert_eq!(
        read_claude_role_launch_preferences(&db).web_search.as_ref(),
        Some(&target)
    );
    assert_eq!(
        validate_claude_web_search(&data, &follow, &assignments, &catalog)?.provider_id,
        "anthropic-official"
    );
    Ok(())
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

fn role_providers() -> Value {
    config(
        json!([
            {
                "id": "source", "model": "source-main[1M]",
                "endpoint": "https://source.example.invalid", "apiKey": "source-key",
                "modelMapping": {"opus": "source-main", "sonnet": "source-sonnet"},
                "effortMapping": {"opus": "low"}, "reasoningEffort": "medium"
            },
            {
                "id": "target", "model": "target-default",
                "endpoint": "https://target.example.invalid/", "apiKey": "target-key",
                "modelMapping": {"opus": "target-opus", "sonnet": "target-sonnet"},
                "effortMapping": {"opus": "high", "sonnet": "low"}, "reasoningEffort": "max",
                "modelMetadata": [{"id": "target-sonnet", "supportedEfforts": ["xhigh"]}]
            }
        ]),
        "source",
    )
}

fn prepare_new(
    root: &Path,
    assignments: &Value,
) -> Result<matou_native_core::provider_launch::ClaudeLaunch> {
    let plan = prepare_new_claude_launch(
        root,
        root,
        &Value::Null,
        Some(&parse_claude_role_assignments(assignments)?),
        BTreeMap::from([
            (
                "MATOU_CLAUDE_COMMAND".into(),
                std::env::current_exe()?.into_os_string(),
            ),
            (
                "ANTHROPIC_DEFAULT_HAIKU_MODEL".into(),
                "inherited-haiku".into(),
            ),
            ("ANTHROPIC_DEFAULT_FABLE_MODEL".into(), "old-fable".into()),
            (
                "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME".into(),
                "old-name".into(),
            ),
            (
                "ANTHROPIC_DEFAULT_OPUS_MODEL_DESCRIPTION".into(),
                "old-description".into(),
            ),
            ("ANTHROPIC_MODEL".into(), "old-main".into()),
            ("ANTHROPIC_SMALL_FAST_MODEL".into(), "old-fast".into()),
            ("CLAUDE_CODE_SUBAGENT_MODEL".into(), "old-subagent".into()),
            (
                "CLAUDE_CODE_FORCE_SUBAGENT_CUSTOM_MODEL".into(),
                "old-forced-model".into(),
            ),
            ("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(), "123".into()),
            (
                "CLAUDE_CODE_SUBAGENT_MODEL_NAME".into(),
                "unrelated-key".into(),
            ),
            ("CLAUDE_CODE_OAUTH_TOKEN".into(), "inherited-token".into()),
        ]),
        80,
        24,
    )?;
    Ok(plan)
}

#[test]
fn new_role_assignment_projects_exact_provider_model_and_effort() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("provider-config.json");
    let mut providers = role_providers();
    for (explicit, mapped, expected) in [
        (Some("xhigh"), Some("high"), Some("xhigh")),
        (Some("follow"), Some("high"), None),
        (None, Some("high"), Some("high")),
        (None, Some("follow"), None),
        (None, None, Some("max")),
    ] {
        let target = &mut providers["providers"]["claude-code"][1];
        target["effortMapping"] = match mapped {
            Some(effort) => json!({"opus": effort, "sonnet": "low"}),
            None => json!({"sonnet": "low"}),
        };
        fs::write(&path, providers.to_string())?;
        let mut assignments = json!({"opus": {"providerId": "target", "model": "target-sonnet"}});
        if let Some(effort) = explicit {
            assignments["opus"]["effort"] = json!(effort);
        }
        let plan = prepare_new(temp.path(), &assignments)?;
        assert_eq!(plan.provider_config_id, "target");
        assert_eq!(plan.model, "target-sonnet");
        assert_eq!(plan.assigned_role, Some("opus"));
        assert_eq!(plan.effort.as_deref(), expected);
        let mut launch = plan.launch;
        let settings = temp.path().join("settings.json");
        attach_claude_settings(&mut launch, &settings)?;
        let mut argv = vec![
            "--settings",
            settings.to_str().unwrap(),
            "--model",
            "target-sonnet",
        ];
        if let Some(effort) = expected {
            argv.extend(["--effort", effort]);
        }
        assert_eq!(launch.argv, argv);
        for (name, value) in [
            ("ANTHROPIC_AUTH_TOKEN", "target-key"),
            ("ANTHROPIC_BASE_URL", "https://target.example.invalid"),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", "target-sonnet"),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL_NAME", "target-sonnet"),
            ("ANTHROPIC_DEFAULT_FABLE_MODEL", "target-sonnet"),
            ("ANTHROPIC_DEFAULT_FABLE_MODEL_NAME", "target-sonnet"),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", "target-sonnet"),
            ("CLAUDE_CODE_SUBAGENT_MODEL_NAME", "unrelated-key"),
        ] {
            assert_env(&launch, name, value);
        }
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL_DESCRIPTION",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
            "CLAUDE_CODE_FORCE_SUBAGENT_CUSTOM_MODEL",
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            assert!(!launch.env.contains_key(OsStr::new(name)));
        }
    }
    Ok(())
}

#[test]
fn managed_launch_replaces_stale_model_environment_for_api_and_official() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("provider-config.json");
    let inherited = [
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME", "stale-gemini"),
        ("ANTHROPIC_DEFAULT_FABLE_MODEL", "matou-role/opus"),
        ("ANTHROPIC_DEFAULT_FABLE_MODEL_NAME", "stale-fable"),
        (
            "ANTHROPIC_DEFAULT_HAIKU_MODEL_DESCRIPTION",
            "old-description",
        ),
        ("ANTHROPIC_MODEL", "old-model"),
        ("ANTHROPIC_SMALL_FAST_MODEL", "old-fast"),
        ("CLAUDE_CODE_GATEWAY_HINT_HEADERS", "old-headers"),
        ("CLAUDE_CODE_SUBAGENT_MODEL_FORCE", "old-subagent"),
    ];
    let mut environment = inherited.to_vec();
    environment.extend([
        ("HTTPS_PROXY", "http://localhost:1234"),
        ("CLAUDE_CONFIG_DIR", "isolated"),
    ]);
    let official = prepare(temp.path(), Value::Null, &environment)?;
    for (key, _) in inherited {
        assert!(!official.env.contains_key(OsStr::new(key)), "{key}");
    }
    assert_env(&official, "HTTPS_PROXY", "http://localhost:1234");
    assert_env(&official, "CLAUDE_CONFIG_DIR", "isolated");

    let mut provider = json!({
        "id": "deepseek", "endpoint": "https://api.deepseek.com/anthropic",
        "model": "deepseek-v4-pro",
        "modelMapping": {"opus": "deepseek-v4-pro", "haiku": "deepseek-flash"}
    });
    for mapping in [None, Some("deepseek-fable")] {
        if let Some(mapping) = mapping {
            provider["modelMapping"]["fable"] = json!(mapping);
        }
        fs::write(&path, config(json!([provider]), "deepseek").to_string())?;
        let api = prepare(temp.path(), Value::Null, &environment)?;
        for (role, model) in [
            ("HAIKU", "deepseek-flash"),
            ("FABLE", mapping.unwrap_or("deepseek-v4-pro")),
        ] {
            let key = format!("ANTHROPIC_DEFAULT_{role}_MODEL");
            assert_env(&api, &key, model);
            assert_env(&api, &format!("{key}_NAME"), model);
        }
        for (key, _) in &inherited[3..] {
            assert!(!api.env.contains_key(OsStr::new(key)), "{key}");
        }
        assert_env(&api, "HTTPS_PROXY", "http://localhost:1234");
    }
    Ok(())
}

#[test]
fn fable_startup_selects_the_independent_assignment_before_opus() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    providers["providers"]["claude-code"][0]["model"] = json!("claude-fable-5-2");
    fs::write(
        temp.path().join("provider-config.json"),
        providers.to_string(),
    )?;
    let assignments = json!({
        "opus": {"providerId": "source", "model": "source-main"},
        "fable": {"providerId": "target", "model": "target-sonnet", "effort": "xhigh"},
    });
    let plan = prepare_new(temp.path(), &assignments)?;
    assert_eq!(plan.assigned_role, Some("fable"));
    assert_eq!(plan.provider_config_id, "target");
    assert_eq!(plan.model, "target-sonnet");
    assert_eq!(plan.effort.as_deref(), Some("xhigh"));
    assert_env(
        &plan.launch,
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "target-sonnet",
    );
    Ok(())
}

#[test]
fn fable_uses_the_target_opus_mapping_when_another_role_is_assigned() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    providers["providers"]["claude-code"][0]["model"] = json!("haiku");
    fs::write(
        temp.path().join("provider-config.json"),
        providers.to_string(),
    )?;
    let plan = prepare_new(
        temp.path(),
        &json!({"haiku": {"providerId": "target", "model": "target-sonnet"}}),
    )?;
    assert_env(
        &plan.launch,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "target-sonnet",
    );
    assert_env(
        &plan.launch,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
        "target-sonnet",
    );
    for name in [
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
    ] {
        assert_env(&plan.launch, name, "target-opus");
    }
    Ok(())
}

#[test]
fn empty_model_and_cli_aliases_select_the_startup_role_before_mappings() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    for (model, role) in [
        ("", "opus"),
        ("default", "opus"),
        ("default[1M]", "opus"),
        ("CLI 默认", "opus"),
        ("opus[1M]", "opus"),
        ("sonnet", "sonnet"),
        ("haiku[1m]", "haiku"),
        ("fable", "opus"),
        ("matou-role/opus-fallback", "opus"),
        ("claude-fable-5", "opus"),
        ("claude-fable-5-2", "opus"),
        ("claude-fable-5-5", "opus"),
        ("matou-role/fable", "opus"),
        ("matou-role/opus", "opus"),
        ("matou-role/haiku", "haiku"),
        ("matou-api/sonnet", "sonnet"),
        ("matou-api/fable", "opus"),
        ("matou-opus", "opus"),
        ("matou-sonnet", "sonnet"),
        ("matou-chatgpt/matou-opus-fallback", "opus"),
        ("matou-chatgpt/matou-haiku", "haiku"),
        ("matou-antigravity/matou-sonnet", "sonnet"),
        ("claude-opus-5", "opus"),
        ("claude-opus-5-5", "opus"),
        ("claude-sonnet-5[1m]", "sonnet"),
        ("claude-sonnet-5-5", "sonnet"),
        ("claude-haiku-5-5", "haiku"),
        ("matou-antigravity/claude-opus-5-5", "opus"),
        ("matou-antigravity/claude-sonnet-5-5[1M]", "sonnet"),
        ("matou-antigravity/claude-haiku-5-5", "haiku"),
        ("source-main[1M]", "opus"),
    ] {
        let source = &mut providers["providers"]["claude-code"][0];
        source["model"] = json!(model);
        source["modelMapping"]["sonnet"] = json!(model);
        if model == "source-main[1M]" {
            source["modelMapping"]["sonnet"] = json!("source-sonnet");
        }
        fs::write(
            temp.path().join("provider-config.json"),
            providers.to_string(),
        )?;
        let plan = prepare_new(
            temp.path(),
            &json!({(role): {"providerId": "target", "model": "target-sonnet", "effort": "follow"}}),
        )?;
        assert_eq!(plan.provider_config_id, "target", "{model:?}");
        assert_eq!(plan.launch.argv, ["--model", "target-sonnet"]);
    }
    Ok(())
}

#[test]
fn startup_aliases_match_case_and_whitespace_exactly_except_for_the_context_suffix() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    let assignments = json!({
        "opus": {"providerId": "absent", "model": "unexpected-opus"},
        "sonnet": {"providerId": "target", "model": "target-sonnet", "effort": "follow"}
    });
    for (model, mapping, expected_provider, expected_model) in [
        ("  ", "source-sonnet", "source", ""),
        (" default ", "source-sonnet", "source", "default"),
        (" CLI 默认 ", "source-sonnet", "source", "CLI 默认"),
        (" default ", " default ", "target", "target-sonnet"),
        ("OPUS", "source-sonnet", "source", "OPUS"),
        (" Opus ", "source-sonnet", "source", "Opus"),
        (" opus ", "source-sonnet", "source", "opus"),
        (
            "CLAUDE-SONNET-5[1m]",
            "source-sonnet",
            "source",
            "CLAUDE-SONNET-5[1m]",
        ),
        ("OPUS[1M]", "OPUS[1m]", "target", "target-sonnet"),
        ("OPUS", " OPUS ", "source", "OPUS"),
    ] {
        let source = &mut providers["providers"]["claude-code"][0];
        source["model"] = json!(model);
        source["modelMapping"]["sonnet"] = json!(mapping);
        fs::write(
            temp.path().join("provider-config.json"),
            providers.to_string(),
        )?;
        let plan = prepare_new(temp.path(), &assignments)?;
        assert_eq!(plan.provider_config_id, expected_provider, "{model:?}");
        assert_eq!(plan.model, expected_model, "{model:?}");
    }
    Ok(())
}

#[test]
fn unassigned_and_unmatched_startup_models_use_original_provider() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    let broken = json!({"providerId": "unavailable", "model": "missing-model"});
    for (model, sonnet, assignments, effort) in [
        (
            "source-main[1M]",
            "source-sonnet",
            json!({"sonnet": broken}),
            "low",
        ),
        (
            "claude-opus-current",
            "source-sonnet",
            json!({"opus": broken}),
            "low",
        ),
        (
            "another-model",
            "source-sonnet",
            json!({"opus": broken, "sonnet": broken, "haiku": broken}),
            "medium",
        ),
        (
            "matou-antigravity/claude-sonnet-private",
            "source-sonnet",
            json!({"opus": broken, "sonnet": broken, "haiku": broken}),
            "medium",
        ),
        (
            "other/matou-sonnet",
            "source-sonnet",
            json!({"opus": broken, "sonnet": broken, "haiku": broken}),
            "medium",
        ),
    ] {
        let source = &mut providers["providers"]["claude-code"][0];
        source["model"] = json!(model);
        source["modelMapping"]["sonnet"] = json!(sonnet);
        fs::write(
            temp.path().join("provider-config.json"),
            providers.to_string(),
        )?;
        let plan = prepare_new(temp.path(), &assignments)?;
        assert_eq!(plan.provider_config_id, "source");
        assert_eq!(plan.launch.argv, ["--model", model, "--effort", effort]);
        assert_env(
            &plan.launch,
            "ANTHROPIC_BASE_URL",
            "https://source.example.invalid",
        );
        assert_env(&plan.launch, "ANTHROPIC_API_KEY", "source-key");
    }
    Ok(())
}

#[test]
fn ambiguous_startup_model_mapping_uses_the_first_matching_role() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    let model = "source-main[1M]";
    let opus = json!({"providerId": "target", "model": "target-sonnet", "effort": "follow"});
    let broken = json!({"providerId": "unavailable", "model": "missing-model"});
    for (mapping, assignments) in [
        (
            json!({"opus": "source-main", "sonnet": "source-main[1m]"}),
            json!({"opus": opus, "sonnet": broken}),
        ),
        (
            json!({"opus": "source-main", "sonnet": "source-main", "haiku": "source-main"}),
            json!({"opus": opus}),
        ),
        (
            json!({"opus": "other-model", "sonnet": "source-main", "haiku": "source-main"}),
            json!({"sonnet": opus, "haiku": broken}),
        ),
    ] {
        let source = &mut providers["providers"]["claude-code"][0];
        source["model"] = json!(model);
        source["modelMapping"] = mapping;
        fs::write(
            temp.path().join("provider-config.json"),
            providers.to_string(),
        )?;
        let plan = prepare_new(temp.path(), &assignments)?;
        assert_eq!(plan.provider_config_id, "target", "{assignments}");
        assert_eq!(plan.launch.argv, ["--model", "target-sonnet"]);
    }
    Ok(())
}

#[test]
fn unassigned_role_intentionally_skips_frozen_fallback_target_validation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    providers["providers"]["claude-code"][0]["modelMetadata"] = json!([{"id": "source-sonnet"}]);
    fs::write(
        temp.path().join("provider-config.json"),
        providers.to_string(),
    )?;
    let plan = prepare_new(
        temp.path(),
        &json!({"sonnet": {"providerId": "target", "model": "target-sonnet"}}),
    )?;
    assert_eq!(plan.provider_config_id, "source");
    assert_eq!(
        plan.launch.argv,
        ["--model", "source-main[1M]", "--effort", "low"]
    );
    Ok(())
}

#[test]
fn sonnet_only_assignment_cleans_environment_when_starting_opus() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("provider-config.json"),
        role_providers().to_string(),
    )?;
    let plan = prepare_new(
        temp.path(),
        &json!({"sonnet": {"providerId": "target", "model": "target-sonnet"}}),
    )?;
    assert_eq!(plan.assigned_role, None);
    assert_eq!(plan.provider_config_id, "source");
    assert_env(&plan.launch, "ANTHROPIC_DEFAULT_OPUS_MODEL", "source-main");
    assert_env(
        &plan.launch,
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "source-sonnet",
    );
    let stale_models = ["ANTHROPIC_MODEL", "CLAUDE_CODE_SUBAGENT_MODEL"];
    assert_env(&plan.launch, "ANTHROPIC_DEFAULT_FABLE_MODEL", "source-main");
    for name in stale_models {
        assert!(!plan.launch.env.contains_key(OsStr::new(name)), "{name}");
    }
    let mut environment = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    environment
        .extend(stale_models.map(|name| (OsString::from(name), OsString::from("inherited-model"))));
    let restarted = prepare_claude_permission_restart(
        temp.path(),
        temp.path(),
        &json!({
            "providerConfigId": plan.provider_config_id,
            "model": plan.model,
            "effort": plan.effort,
            "clearAssignmentEnvironment": plan.clear_assignment_environment,
            "permissionMode": "plan",
        }),
        true,
        environment,
        80,
        24,
    )?;
    for name in stale_models {
        assert!(
            !restarted.launch.env.contains_key(OsStr::new(name)),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn null_supported_efforts_allows_explicit_assignment_effort() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    providers["providers"]["claude-code"][1]["modelMetadata"][0]["supportedEfforts"] = Value::Null;
    fs::write(
        temp.path().join("provider-config.json"),
        providers.to_string(),
    )?;
    let input =
        json!({"opus": {"providerId": "target", "model": "target-sonnet", "effort": "high"}});
    validate_claude_role_assignments(temp.path(), &input)?;
    assert_eq!(
        prepare_new(temp.path(), &input)?.launch.argv,
        ["--model", "target-sonnet", "--effort", "high"]
    );
    Ok(())
}

#[test]
fn active_subscription_source_reports_service_unavailable_even_with_an_api_opus_assignment()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    providers["providers"]["claude-code"][0] = json!({
        "id": "source", "model": "opus", "endpoint": "", "kind": "subscription",
        "modelMapping": {"opus": "subscription-opus"},
        "subscription": {"modelMetadata": [{"id": "subscription-opus"}]}
    });
    fs::write(
        temp.path().join("provider-config.json"),
        providers.to_string(),
    )?;
    let assignments = json!({"opus": {"providerId": "target", "model": "target-sonnet"}});
    validate_claude_role_assignments(temp.path(), &assignments)?;
    let error = prepare_new(temp.path(), &assignments)
        .err()
        .context("subscription launch must fail")?;
    assert_eq!(
        error.to_string(),
        current_interface_locale(read_locale_preference(temp.path())).text(
            "订阅服务未启动，请重新打开设置或重启码头",
            "The subscription service hasn't started. Reopen Settings or restart Matou.",
        )
    );
    Ok(())
}

#[test]
fn saved_subscription_targets_validate_but_an_offline_launch_still_requires_the_service()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut providers = role_providers();
    providers["providers"]["claude-code"][1] = json!({
        "id": "target", "model": "opus", "endpoint": "", "kind": "subscription",
        "modelMapping": {"opus": "subscription-opus"},
        "subscription": {"provider": "codex", "modelMetadata": [{"id": "subscription-opus"}]}
    });
    fs::write(
        temp.path().join("provider-config.json"),
        providers.to_string(),
    )?;
    let assignment = json!({"opus": {"providerId": "target", "model": "subscription-opus"}});
    validate_claude_role_assignments(temp.path(), &assignment)?;
    let error = prepare_new(temp.path(), &assignment)
        .err()
        .context("offline subscription launch")?;
    assert_eq!(
        error.to_string(),
        current_interface_locale(read_locale_preference(temp.path())).text(
            "订阅服务未启动，请重新打开设置或重启码头",
            "The subscription service hasn't started. Reopen Settings or restart Matou.",
        )
    );
    Ok(())
}

#[test]
fn invalid_selected_assignment_fails_without_using_the_source() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("provider-config.json");
    let mut providers = role_providers();
    providers["providers"]["claude-code"][1]["apiKey"] = json!("  ");
    fs::write(&path, providers.to_string())?;
    let providers_without_key = providers.to_string();
    providers["providers"]["claude-code"][1]["apiKey"] = json!("target-key");
    let providers_with_key = providers.to_string();
    for (config, provider, model, effort, reason) in [
        (
            &providers_with_key,
            "missing",
            "target-sonnet",
            "follow",
            "provider is unavailable",
        ),
        (
            &providers_without_key,
            "target",
            "target-sonnet",
            "follow",
            "API key is unavailable",
        ),
        (
            &providers_with_key,
            "target",
            "missing-model",
            "follow",
            "model is unavailable",
        ),
        (
            &providers_with_key,
            "target",
            "target-sonnet",
            "max",
            "Unsupported reasoning effort",
        ),
    ] {
        fs::write(&path, config)?;
        let assignments =
            json!({"opus": {"providerId": provider, "model": model, "effort": effort}});
        let error = prepare_new(temp.path(), &assignments)
            .err()
            .context("launch must fail")?;
        let error = format!("{error:#}");
        assert!(error.starts_with("opus assignment:"), "{error}");
        assert!(error.contains(reason), "{error}");
    }
    Ok(())
}

#[test]
fn validates_assignments_against_frozen_limits_and_stored_model_catalog() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("provider-config.json");
    let providers = role_providers().to_string();
    fs::write(&path, &providers)?;
    let cases = [
        (Value::Null, "must be an object"),
        (json!({"other": {}}), "Unknown Claude model role"),
        (json!({"opus": null}), "must be an object"),
        (
            json!({"opus": {"providerId": " ", "model": "valid"}}),
            "providerId is empty",
        ),
        (
            json!({"opus": {"providerId": "a".repeat(257), "model": "valid"}}),
            "exceeds 256",
        ),
        (
            json!({"opus": {"providerId": "source", "model": "🚂".repeat(121)}}),
            "exceeds 240",
        ),
        (
            json!({"opus": {"providerId": "source\t", "model": "valid"}}),
            "control character",
        ),
        (
            json!({"opus": {"providerId": "source", "model": "bad\u{7f}"}}),
            "control character",
        ),
        (
            json!({"opus": {"providerId": "source", "model": "valid", "effort": null}}),
            "Invalid reasoning effort",
        ),
        (
            json!({"opus": {"providerId": "source", "model": "valid", "effort": "extreme"}}),
            "Invalid reasoning effort",
        ),
        (
            json!({"opus": {"providerId": "missing", "model": "valid"}}),
            "provider is unavailable",
        ),
        (
            json!({"opus": {"providerId": "target", "model": "not-in-catalog"}}),
            "model is unavailable",
        ),
        (
            json!({"opus": {"providerId": "target", "model": "target-sonnet", "effort": "max"}}),
            "Unsupported reasoning effort",
        ),
    ];
    for (input, reason) in cases {
        let error = validate_claude_role_assignments(temp.path(), &input).unwrap_err();
        assert!(format!("{error:#}").contains(reason), "{error:#}");
    }
    for effort in ["follow", "xhigh"] {
        let input = json!({"haiku": {"providerId": " target ", "model": " target-sonnet ", "effort": effort}});
        let assignments = validate_claude_role_assignments(temp.path(), &input)?;
        assert_eq!(
            serde_json::to_value(assignments)?,
            json!({
                "haiku": {"providerId": "target", "model": "target-sonnet", "effort": effort}
            })
        );
    }
    let custom = json!({"sonnet": {"providerId": "source", "model": "手填模型", "effort": "max"}});
    assert_eq!(
        serde_json::to_value(validate_claude_role_assignments(temp.path(), &custom)?)?,
        custom
    );
    assert_eq!(fs::read_to_string(&path)?, providers);
    Ok(())
}

#[test]
fn official_target_without_catalog_intentionally_skips_frozen_membership_check() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("provider-config.json");
    let official = json!({
        "id": "anthropic-official", "builtIn": true, "model": "", "endpoint": "https://api.anthropic.com/"
    });
    let mut with_official = role_providers();
    with_official["providers"]["claude-code"][1] = official;
    let mut with_built_in = with_official.clone();
    with_built_in["providers"]["claude-code"][1]["id"] = json!("builtin");
    for (stored, id) in [
        (None, "anthropic-official"),
        (Some(&with_official), "anthropic-official"),
        (Some(&with_built_in), "builtin"),
    ] {
        if let Some(config) = stored {
            fs::write(&path, config.to_string())?;
        }
        let input = json!({"opus": {"providerId": id, "model": "uncatalogued-official-model", "effort": "high"}});
        validate_claude_role_assignments(temp.path(), &input)?;
        let plan = prepare_new(temp.path(), &input)?;
        assert_eq!(plan.provider_config_id, id);
        assert_eq!(
            plan.launch.argv,
            ["--model", "uncatalogued-official-model", "--effort", "high"]
        );
        assert_env(
            &plan.launch,
            "ANTHROPIC_BASE_URL",
            "https://api.anthropic.com",
        );
        assert_env(&plan.launch, "CLAUDE_CODE_OAUTH_TOKEN", "inherited-token");
        assert!(
            !plan
                .launch
                .env
                .contains_key(OsStr::new("ANTHROPIC_API_KEY"))
        );
    }

    let input =
        json!({"opus": {"providerId": "anthropic-official", "model": "opus", "effort": "high"}});
    for stored in [role_providers(), with_built_in] {
        fs::write(&path, stored.to_string())?;
        let error = validate_claude_role_assignments(temp.path(), &input).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "opus assignment: Assignment provider is unavailable"
        );
        let error = prepare_new(temp.path(), &input)
            .err()
            .context("launch must fail without a configured official provider")?;
        assert_eq!(
            format!("{error:#}"),
            "opus assignment: Assignment provider is unavailable"
        );
    }

    let catalog = json!([{"id": "opus", "supportedEfforts": ["high"]}]);
    for (field, value) in [
        ("endpoint", json!("https://custom.example.invalid")),
        ("endpoint", json!("https://api.anthropic.com//")),
        ("builtIn", json!(false)),
        ("modelMetadata", catalog.clone()),
    ] {
        let mut invalid = with_official.clone();
        invalid["providers"]["claude-code"][1][field] = value;
        fs::write(&path, invalid.to_string())?;
        let save_error = validate_claude_role_assignments(temp.path(), &input).unwrap_err();
        let launch_error = prepare_new(temp.path(), &input)
            .err()
            .context("launch must fail without an API key")?;
        for error in [save_error, launch_error] {
            assert_eq!(
                format!("{error:#}"),
                "opus assignment: Assignment API key is unavailable",
                "{field}"
            );
        }
    }

    let entry = &mut with_official["providers"]["claude-code"][1];
    entry["modelMetadata"] = catalog;
    entry["apiKey"] = json!("official-fixture-key");
    fs::write(&path, with_official.to_string())?;
    validate_claude_role_assignments(temp.path(), &input)?;
    assert_eq!(
        prepare_new(temp.path(), &input)?.launch.argv,
        ["--model", "opus", "--effort", "high",]
    );
    for (model, effort, reason) in [
        ("sonnet", "high", "model is unavailable"),
        ("opus", "max", "Unsupported reasoning effort"),
    ] {
        let input =
            json!({"opus": {"providerId": "anthropic-official", "model": model, "effort": effort}});
        let error = validate_claude_role_assignments(temp.path(), &input).unwrap_err();
        assert!(format!("{error:#}").contains(reason), "{error:#}");
        let error = prepare_new(temp.path(), &input)
            .err()
            .context("launch must fail")?;
        assert!(format!("{error:#}").contains(reason), "{error:#}");
    }
    Ok(())
}

#[test]
fn bound_resume_and_fork_keep_the_existing_launch_arguments() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(
        root.join("provider-config.json"),
        role_providers().to_string(),
    )?;
    let environment = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    let binding =
        json!({"providerConfigId": "target", "model": "target-opus", "permissionMode": "plan"});
    let resume = prepare_claude_launch(
        root,
        root,
        &binding,
        Some("original"),
        environment.clone(),
        80,
        24,
    )?;
    let fork = prepare_claude_fork(root, root, &binding, "original", environment, 80, 24)?;
    for plan in [&resume, &fork] {
        assert_eq!(plan.provider_config_id, "target");
        assert_eq!(plan.model, "target-opus");
        assert_env(
            &plan.launch,
            "ANTHROPIC_BASE_URL",
            "https://target.example.invalid",
        );
    }
    assert_eq!(
        resume.launch.argv,
        [
            "--model",
            "target-opus",
            "--effort",
            "high",
            "--resume",
            "original",
            "--permission-mode",
            "plan",
        ]
    );
    assert_eq!(
        fork.launch.argv,
        [
            "--model",
            "target-opus",
            "--effort",
            "high",
            "--resume",
            "original",
            "--fork-session",
            "--permission-mode",
            "plan",
        ]
    );
    Ok(())
}

#[test]
fn fork_rebuild_uses_the_parent_and_preserves_the_child_launch_settings() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(
        root.join("provider-config.json"),
        role_providers().to_string(),
    )?;
    let environment = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    let binding =
        json!({"providerConfigId": "target", "model": "target-opus", "permissionMode": "plan"});
    let parent = "fff4132a-f6f8-4b49-9f6d-4b07be37f5c2";
    let child = "7ec4132a-f6f8-4b49-9f6d-4b07be37f5c2";
    let resume = prepare_claude_launch(
        root,
        root,
        &binding,
        Some(child),
        environment.clone(),
        80,
        24,
    )?;
    let automatic = resume.for_fork_rebuild(parent, child)?;
    let manual = prepare_claude_fork_rebuild(
        root,
        root,
        &binding,
        (parent, child),
        environment.clone(),
        80,
        24,
    )?;
    for plan in [automatic, manual] {
        assert_eq!(
            plan.launch.argv,
            [
                "--model",
                "target-opus",
                "--effort",
                "high",
                "--resume",
                parent,
                "--fork-session",
                "--session-id",
                child,
                "--permission-mode",
                "plan",
            ]
        );
        assert_eq!(plan.launch.env, resume.launch.env);
        assert_eq!(plan.launch.cwd, resume.launch.cwd);
    }
    for same in [
        parent.to_owned(),
        parent.to_ascii_uppercase(),
        parent.replace('-', ""),
    ] {
        assert!(resume.for_fork_rebuild(parent, &same).is_err());
        assert!(
            prepare_claude_fork_rebuild(
                root,
                root,
                &binding,
                (parent, &same),
                environment.clone(),
                80,
                24
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn permission_restart_translates_legacy_bound_model_aliases() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(
        root.join("provider-config.json"),
        role_providers().to_string(),
    )?;
    let environment = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    for (alias, model, effort) in [
        ("matou-api/opus", "target-opus", "high"),
        ("matou-chatgpt/matou-opus", "target-default", "max"),
        ("matou-antigravity/claude-opus-5-5", "target-default", "max"),
    ] {
        let binding =
            json!({"providerConfigId": "target", "model": alias, "permissionMode": "plan"});
        let plan = prepare_claude_permission_restart(
            root,
            root,
            &binding,
            false,
            environment.clone(),
            80,
            24,
        )?;
        assert_eq!(
            plan.launch.argv,
            [
                "--model",
                model,
                "--effort",
                effort,
                "--permission-mode",
                "plan"
            ]
        );
    }
    Ok(())
}

#[test]
fn permission_restart_and_assigned_resume_preserve_omitted_effort() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(
        root.join("provider-config.json"),
        role_providers().to_string(),
    )?;
    let environment = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    let mut binding = json!({"providerConfigId": "target", "model": "target-sonnet"});
    let fresh =
        prepare_claude_permission_restart(root, root, &binding, true, environment.clone(), 80, 24)?;
    assert_eq!(fresh.launch.argv, ["--model", "target-sonnet"]);
    binding["assignedRole"] = json!("sonnet");
    let resumed =
        prepare_claude_launch(root, root, &binding, Some("original"), environment, 80, 24)?;
    assert_eq!(
        resumed.launch.argv,
        ["--model", "target-sonnet", "--resume", "original"]
    );
    assert_env(
        &resumed.launch,
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "target-opus",
    );
    Ok(())
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
    let error = prepare(
        temp.path(),
        json!({"providerConfigId": "removed", "model": "matou-chatgpt/matou-opus"}),
        &[],
    )
    .err()
    .context("removed subscription should fail")?;
    assert_eq!(
        error.to_string(),
        current_interface_locale(read_locale_preference(temp.path())).text(
            "还没有可用的订阅账号，请先在设置里添加并启用",
            "No subscription account is available. Add and enable one in Settings.",
        )
    );
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
            ("CLAUDE_CODE_NO_FLICKER", "1"),
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
    assert_env(&launch, "CLAUDE_CODE_NO_FLICKER", "1");
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

#[test]
fn direct_gemini_model_and_mapping_enable_gateway_for_fresh_and_resumed_launches() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let environment = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    for (model, mapping, effective_model) in [
        (
            "publisher/GeMiNi-fixture",
            json!({}),
            "publisher/GeMiNi-fixture",
        ),
        (
            "claude-opus-fixture",
            json!({"sonnet":"publisher/GEMINI-fixture"}),
            "claude-sonnet-fixture",
        ),
    ] {
        fs::write(
            root.path().join("provider-config.json"),
            config(
                json!([{
                    "id": "source", "model": model, "modelMapping": mapping,
                    "endpoint": "https://fixture.invalid", "apiKey": "fixture-key",
                }]),
                "source",
            )
            .to_string(),
        )?;
        for resume in [None, Some("history-id")] {
            let launch = prepare_claude_launch(
                root.path(),
                root.path(),
                &Value::Null,
                resume,
                environment.clone(),
                80,
                24,
            )?;
            let gateway = launch
                .gateway
                .context("Gemini launch must use the gateway")?;
            assert!(gateway.stream_keep_alive);
            assert!(gateway.literal_stream_keep_alive(effective_model));
            if model.starts_with("claude-") {
                assert!(!gateway.literal_stream_keep_alive(model));
            }
        }
    }
    Ok(())
}

#[test]
fn direct_deepseek_resume_retains_real_context_window() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let path = root.path().join("provider-config.json");
    fs::write(&path, config(json!([{
        "id":"source", "model":"deepseek-fixture", "endpoint":"https://api.deepseek.com/anthropic",
        "apiKey":"fixture-key", "contextWindow":512_000,
        "modelMetadata":[{"id":"deepseek-fixture","contextWindow":900_000}],
    },{
        "id":"active", "model":"claude-fixture", "endpoint":"https://fixture.invalid", "apiKey":"active-key",
    }]), "active").to_string())?;
    let mut plan = prepare_claude_launch(
        root.path(),
        root.path(),
        &json!({"providerConfigId": "source", "model": "deepseek-fixture"}),
        Some("history-id"),
        BTreeMap::from([(
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        )]),
        80,
        24,
    )?;
    plan.capture_restart_gateway(&Default::default(), &Vec::new, &BTreeMap::new())?;
    let gateway = plan
        .gateway
        .context("DeepSeek resume must use the gateway")?;
    assert_eq!(gateway.context_window, Some(512_000));
    assert_eq!(
        gateway.literal_context_window("deepseek-fixture"),
        Some(900_000)
    );
    Ok(())
}

fn single_provider_preferences(
    id: &str,
    mode: Value,
) -> Result<matou_native_core::sessions::ClaudeRoleLaunchPreferences> {
    let mut db = Connection::open_in_memory()?;
    migrate(&mut db, 1)?;
    db.execute(
        "INSERT INTO preferences VALUES ('agent.modelPreferences', ?, 1)",
        [json!({
            "compaction": {"mode": "follow"},
            "contextWindows": {id: mode},
        })
        .to_string()],
    )?;
    Ok(read_claude_role_launch_preferences(&db))
}

fn assert_single_provider_window(launch: &ShellLaunch, window: u64) {
    assert_env(
        launch,
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
        &window.to_string(),
    );
    assert_env(launch, "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "90");
    assert_env(
        launch,
        "CLAUDE_CODE_DISABLE_1M_CONTEXT",
        if window <= 200_000 { "1" } else { "0" },
    );
}

#[test]
fn single_custom_model_uses_known_default_window_on_each_launch_path() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let root = root.path();
    let path = root.join("provider-config.json");
    let mut provider = config(
        json!([{
            "id": "source", "model": "deepseek-flash", "endpoint": "http://127.0.0.1:1",
            "apiKey": "fixture-key",
            "modelMetadata": [{"id": "deepseek-flash", "contextWindow": 1_048_576,
                               "maxContextWindow": 2_000_000}]
        }]),
        "source",
    );
    fs::write(&path, provider.to_string())?;
    let environment = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    let binding = json!({"providerConfigId": "source", "model": "deepseek-flash[1M]"});
    let preferences = single_provider_preferences("source", json!("default"))?;
    let launch = prepare_new_claude_launch_with_budget(
        root,
        root,
        &Value::Null,
        (&preferences, &Vec::new),
        environment.clone(),
        80,
        24,
    )?;
    assert_single_provider_window(&launch.launch, 1_048_576);
    assert_single_provider_window(&prepare(root, Value::Null, &[])?, 1_048_576);

    let mut launches = [
        prepare_claude_launch(
            root,
            root,
            &binding,
            Some("source-id"),
            environment.clone(),
            80,
            24,
        )?,
        prepare_claude_permission_restart(root, root, &binding, true, environment.clone(), 80, 24)?,
        prepare_claude_fork(
            root,
            root,
            &binding,
            "source-id",
            environment.clone(),
            80,
            24,
        )?,
        prepare_claude_fork_rebuild(
            root,
            root,
            &binding,
            ("source-id", "child-id"),
            environment.clone(),
            80,
            24,
        )?,
    ];
    for launch in &mut launches {
        assert_single_provider_window(&launch.launch, 1_048_576);
        launch.capture_restart_gateway(&preferences, &Vec::new, &environment)?;
        assert_single_provider_window(&launch.launch, 1_048_576);
    }

    provider["providers"]["claude-code"][0]["contextWindow"] = json!(700_000);
    fs::write(&path, provider.to_string())?;
    assert_single_provider_window(&prepare(root, Value::Null, &[])?, 700_000);
    let launch = prepare_new_claude_launch_with_budget(
        root,
        root,
        &Value::Null,
        (
            &single_provider_preferences("source", json!("maximum"))?,
            &Vec::new,
        ),
        environment,
        80,
        24,
    )?;
    assert_single_provider_window(&launch.launch, 700_000);
    Ok(())
}

#[test]
fn single_api_window_preferences_apply_to_fresh_and_restored_launches() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let root = root.path();
    let path = root.join("provider-config.json");
    let environment = BTreeMap::from([(
        "MATOU_CLAUDE_COMMAND".into(),
        std::env::current_exe()?.into_os_string(),
    )]);
    for (mode, mapping, expected) in [
        ("512k", "role-model", 512_000),
        ("maximum", "role-model", 1_100_000),
        ("maximum", "unknown-model", 2_000_000),
        ("maximum", "codex-model", 872_000),
        ("maximum", "small-model", 200_000),
    ] {
        fs::write(&path, config(json!([{
            "id": "source", "model": "custom-model", "endpoint": "http://127.0.0.1:1",
            "apiKey": "fixture-key", "reasoningEffort": "high",
            "modelMapping": {"sonnet": mapping},
            "modelMetadata": [
                {"id": "custom-model", "contextWindow": 1_048_576, "maxContextWindow": 2_000_000},
                {"id": "role-model", "contextWindow": 1_100_000},
                {"id": "codex-model", "contextWindow": 272_000, "maxContextWindow": 872_000},
                {"id": "small-model", "contextWindow": 200_000}
            ]
        }]), "source").to_string())?;
        let preferences = single_provider_preferences("source", json!(mode))?;
        let fresh = prepare_new_claude_launch_with_budget(
            root,
            root,
            &Value::Null,
            (&preferences, &Vec::new),
            environment.clone(),
            80,
            24,
        )?;
        assert_single_provider_window(&fresh.launch, expected);
        let mut restored = prepare_claude_launch(
            root,
            root,
            &Value::Null,
            Some("source-id"),
            environment.clone(),
            80,
            24,
        )?;
        restored.capture_restart_gateway(&preferences, &Vec::new, &environment)?;
        assert_single_provider_window(&restored.launch, expected);
        assert_eq!(
            restored.gateway.context("effort gateway")?.context_window,
            Some(expected)
        );
    }
    Ok(())
}

#[test]
fn single_default_window_leaves_official_and_builtin_models_to_claude() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let root = root.path();
    for (id, endpoint, model, catalog_id) in [
        (
            "anthropic-official",
            "https://other.invalid",
            "custom-model",
            "custom-model",
        ),
        (
            "source",
            "https://api.anthropic.com/",
            "custom-model",
            "custom-model",
        ),
        ("source", "https://other.invalid", "opus", "opus"),
        (
            "source",
            "https://other.invalid",
            "claude-opus-5-5",
            "claude-opus-5-5",
        ),
        (
            "source",
            "https://other.invalid",
            "ClAuDe-opus-5-5[1M]",
            "ClAuDe-opus-5-5",
        ),
        ("source", "https://other.invalid", "FABLE", "FABLE"),
        ("source", "https://other.invalid", "default", "default"),
        ("source", "https://other.invalid", "CLI 默认", "CLI 默认"),
    ] {
        fs::write(
            root.join("provider-config.json"),
            config(
                json!([{
                    "id": id, "model": model, "endpoint": endpoint, "apiKey": "fixture-key",
                    "modelMetadata": [{"id": catalog_id, "contextWindow": 1_048_576}]
                }]),
                id,
            )
            .to_string(),
        )?;
        let launch = prepare_new_claude_launch_with_budget(
            root,
            root,
            &Value::Null,
            (
                &single_provider_preferences(id, json!("default"))?,
                &Vec::new,
            ),
            BTreeMap::from([(
                "MATOU_CLAUDE_COMMAND".into(),
                std::env::current_exe()?.into_os_string(),
            )]),
            80,
            24,
        )?;
        assert!(
            !launch
                .launch
                .env
                .contains_key(OsStr::new("CLAUDE_CODE_MAX_CONTEXT_TOKENS")),
            "{id}/{model}"
        );
    }
    Ok(())
}
