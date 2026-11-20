use super::*;
use matou_native_runtime::protocol::{ClaudeModelCatalogReply, ClaudeModelCatalogResult};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("../fixtures/claude-model-catalog.cjs");

fn executable(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn spawn_catalog(
    root: &Path,
    provider: &Path,
    configure: impl FnOnce(&mut ProcessCommand),
) -> Result<RuntimeChild> {
    let temp = root.join("tmp");
    fs::create_dir(&temp)?;
    let mut runtime = RuntimeChild::spawn_configured_with(
        root,
        &format!("PROMPT='{PROMPT} '\n"),
        Path::new("/bin/zsh"),
        Some(provider),
        |command| {
            command
                .env("TMPDIR", &temp)
                .env("ZDOTDIR", root.join("Shell 家目录"))
                .env("ANTHROPIC_API_KEY", "inherited-fixture-key")
                .env("ANTHROPIC_AUTH_TOKEN", "inherited-fixture-token")
                .env("ANTHROPIC_BASE_URL", "https://not-used.invalid")
                .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "0")
                .env("CLAUDE_CODE_OAUTH_TOKEN", "inherited-fixture-oauth")
                .env("MATOU_FIXTURE_MARKER", "must-be-removed");
            configure(command);
        },
    )?;
    runtime.hello()?;
    Ok(runtime)
}

fn configure_fixture(command: &mut ProcessCommand, root: &Path, mode: &str) {
    command
        .env("CATALOG_FIXTURE_LOG", root.join("catalog.jsonl"))
        .env("CATALOG_FIXTURE_MODE", mode)
        .env("CATALOG_FIXTURE_SENTINEL", "preserved");
}

fn catalog_result(runtime: &mut RuntimeChild, request_id: u64) -> Result<Value> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match runtime.event(deadline, true)? {
            Event::Graph(graph) => {
                if let Some(result) = graph.get("claudeModelCatalogResult") {
                    let decoded: ClaudeModelCatalogReply = serde_json::from_value(result.clone())?;
                    assert_eq!(decoded.request_id, request_id);
                    assert!(matches!(
                        decoded.result,
                        ClaudeModelCatalogResult::Models(_)
                    ));
                    return Ok(result.clone());
                }
            }
            Event::Diagnostic(error) => bail!("host diagnostic: {error}"),
            Event::Closed => bail!("host closed before model catalog response"),
            _ => {}
        }
    }
}

fn requests(log: &[Value]) -> Vec<Value> {
    log.iter()
        .filter_map(|row| row.get("message"))
        .map(|message| message["request"].clone())
        .collect()
}

fn fixture_log(root: &Path) -> Result<Vec<Value>> {
    let records: Vec<Value> = fs::read_to_string(root.join("catalog.jsonl"))
        .context("CLI fixture did not record its startup")?
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    for row in &records {
        assert!(row.get("error").is_none(), "CLI fixture assertion: {row}");
    }
    let start = &records[0];
    assert_eq!(
        records
            .iter()
            .filter(|row| row.get("pid").is_some())
            .count(),
        1
    );
    assert_eq!(
        start["home"],
        root.join("Shell 家目录").to_str().context("test HOME")?
    );
    let cwd = Path::new(start["cwd"].as_str().context("catalog cwd")?);
    assert_eq!(cwd.parent(), Some(root.join("tmp").as_path()));
    assert!(!cwd.exists(), "probe left its temporary directory behind");
    Ok(records)
}

fn shutdown(mut runtime: RuntimeChild) -> Result<()> {
    runtime.send(Command::Shutdown)?;
    assert!(runtime.closed()?.is_empty());
    assert!(runtime.wait()?.0.success());
    Ok(())
}

#[test]
fn claude_model_catalog_uses_frozen_protocol_and_caches_requests() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let cli = root.path().join("catalog-fixture.cjs");
    executable(&cli, FIXTURE)?;
    let login_shell = root.path().join("login-shell-fixture");
    executable(
        &login_shell,
        "#!/bin/sh\nprintf 'invoked\\n' >> \"$CATALOG_LOGIN_LOG\"\nprintf '__MATOU_LOGIN_PATH__=%s\\n' \"$CATALOG_LOGIN_PATH\"\n",
    )?;
    let login_path = std::env::join_paths(std::iter::once(root.path().to_owned()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))?;
    let mut runtime = spawn_catalog(root.path(), Path::new("catalog-fixture.cjs"), |command| {
        configure_fixture(command, root.path(), "catalog");
        command
            .env_remove("MATOU_DISABLE_LOGIN_PATH_PROBE")
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", &login_shell)
            .env("CATALOG_LOGIN_PATH", &login_path)
            .env("CATALOG_LOGIN_LOG", root.path().join("login.log"));
    })?;
    for request_id in [101, 102] {
        runtime.send(Command::GetClaudeModelCatalog { request_id })?;
    }
    let expected = json!([
        {"id":"claude-fixture-sonnet", "displayName":"Default fixture", "contextWindow":200000,
         "maxContextWindow":1000000, "supportedEfforts":["low","high","max"]},
        {"id":"default", "resolvedModel":"claude-fixture-sonnet", "displayName":"Default fixture",
         "contextWindow":200000, "maxContextWindow":1000000, "supportedEfforts":["low","high","max"]},
        {"id":"sonnet", "resolvedModel":"claude-fixture-sonnet", "displayName":"Sonnet fixture",
         "contextWindow":210000, "maxContextWindow":1000000, "supportedEfforts":["medium","xhigh"]},
        {"id":"claude-fixture-fable", "supportedEfforts":[]},
        {"id":"fable", "resolvedModel":"claude-fixture-fable", "supportedEfforts":[]},
    ]);
    for request_id in [101, 102, 103] {
        if request_id == 103 {
            runtime.send(Command::GetClaudeModelCatalog { request_id })?;
        }
        assert_eq!(
            catalog_result(&mut runtime, request_id)?,
            json!({"requestId": request_id, "models": expected})
        );
    }
    let set = |model| json!({"subtype": "set_model", "model": model});
    let context = || json!({"subtype": "get_context_usage", "detail": "summary"});
    assert_eq!(
        requests(&fixture_log(root.path())?),
        vec![
            json!({"subtype": "initialize"}),
            set("default"),
            context(),
            set("default[1m]"),
            context(),
            set("sonnet"),
            context(),
            set("sonnet[1m]"),
            context(),
            set("haiku"),
            set("opus"),
            context(),
            set("fable"),
            context(),
            set("fable[1m]"),
        ],
    );
    assert_eq!(
        fs::read_to_string(root.path().join("login.log"))?,
        "invoked\n"
    );
    shutdown(runtime)
}

#[test]
fn claude_model_catalog_times_out_kills_child_and_keeps_host_responsive() -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let cli = root.path().join("catalog-fixture.cjs");
    executable(&cli, FIXTURE)?;
    let mut runtime = spawn_catalog(root.path(), &cli, |command| {
        configure_fixture(command, root.path(), "timeout");
    })?;
    let started = Instant::now();
    runtime.send(Command::GetClaudeModelCatalog { request_id: 201 })?;
    runtime.send(Command::GetClaudeRoleAssignments { request_id: 202 })?;
    let deadline = started + Duration::from_secs(5);
    loop {
        match runtime.event(deadline, true)? {
            Event::Graph(graph) => {
                assert!(graph.get("claudeModelCatalogResult").is_none());
                if graph.get("claudeRoleAssignmentsResult").is_some() {
                    assert_eq!(graph["claudeRoleAssignmentsResult"]["requestId"], 202);
                    break;
                }
            }
            Event::Diagnostic(error) => bail!("host diagnostic: {error}"),
            Event::Closed => bail!("host closed during model catalog probe"),
            _ => {}
        }
    }
    assert_eq!(
        catalog_result(&mut runtime, 201)?,
        json!({"requestId":201, "models":[]})
    );
    let elapsed = started.elapsed();
    let log = fixture_log(root.path())?;
    assert!(
        elapsed >= Duration::from_secs(6),
        "catalog returned after {elapsed:?}"
    );
    let pid = log[0]["pid"].as_u64().context("fixture PID")?;
    let still_alive = ProcessCommand::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()?
        .status
        .success();
    assert!(!still_alive, "catalog CLI {pid} survived timeout");
    assert_eq!(
        requests(&log).last(),
        Some(&json!({"subtype": "set_model", "model": "sonnet"}))
    );
    runtime.send(Command::GetClaudeModelCatalog { request_id: 203 })?;
    assert_eq!(
        catalog_result(&mut runtime, 203)?,
        json!({"requestId":203, "models":[]})
    );
    assert_eq!(fixture_log(root.path())?, log);
    println!("catalog deadline elapsed={elapsed:?}; child {pid} reaped, partial results discarded");
    shutdown(runtime)
}

#[test]
fn claude_model_catalog_unavailable_returns_empty() -> Result<()> {
    for mode in ["missing", "exit"] {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let cli = root.path().join("catalog-fixture.cjs");
        if mode == "exit" {
            executable(&cli, FIXTURE)?;
        }
        let mut runtime = spawn_catalog(root.path(), &cli, |command| {
            configure_fixture(command, root.path(), mode);
        })?;
        runtime.send(Command::GetClaudeModelCatalog { request_id: 301 })?;
        assert_eq!(
            catalog_result(&mut runtime, 301)?,
            json!({"requestId":301, "models":[]})
        );
        if mode == "exit" {
            assert_eq!(
                requests(&fixture_log(root.path())?).last(),
                Some(&json!({"subtype": "set_model", "model": "sonnet"}))
            );
        } else {
            assert!(!root.path().join("catalog.jsonl").exists());
            assert!(fs::read_dir(root.path().join("tmp"))?.next().is_none());
        }
        shutdown(runtime)?;
    }
    Ok(())
}

#[test]
#[ignore = "set MATOU_REAL_CLAUDE to an installed CLI for the offline smoke test"]
fn claude_model_catalog_real_cli_offline() -> Result<()> {
    let actual_cli = PathBuf::from(
        std::env::var_os("MATOU_REAL_CLAUDE").context("set MATOU_REAL_CLAUDE to the CLI path")?,
    );
    assert!(actual_cli.is_absolute() && actual_cli.is_file());
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let sandbox = root.path().join("offline-cli");
    executable(
        &sandbox,
        r#"#!/bin/sh
exec /usr/bin/sandbox-exec -D "PROBE_ROOT=$CATALOG_TEST_ROOT" \
  -p '(version 1) (allow default) (deny network*) (deny file-write*) (allow file-write* (subpath (param "PROBE_ROOT")))' \
  "$CATALOG_REAL_CLI" "$@"
"#,
    )?;
    let mut runtime = spawn_catalog(root.path(), &sandbox, |command| {
        command
            .env("CATALOG_REAL_CLI", &actual_cli)
            .env("CATALOG_TEST_ROOT", root.path());
    })?;
    let started = Instant::now();
    runtime.send(Command::GetClaudeModelCatalog { request_id: 401 })?;
    let result = catalog_result(&mut runtime, 401)?;
    let models = result["models"].as_array().context("real CLI models")?;
    assert!(!models.is_empty(), "real CLI did not return models");
    println!(
        "offline real CLI returned {} rows in {:?}",
        models.len(),
        started.elapsed()
    );
    for model in models.iter().take(3) {
        println!("  {model}");
    }
    shutdown(runtime)
}
