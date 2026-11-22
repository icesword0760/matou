use super::*;
use matou_native_core::sessions::{claude_binding, save_claude_time_zone};
use matou_native_runtime::{
    egress_probe::Family,
    host::{ForkPlacement, ForkWorktreeMode, RecoveryState},
    protocol::{ProviderSettingsReply, ProviderSettingsResult},
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const CODEX_ID: &str = "codex-subscription-fixture";
const ANTIGRAVITY_ID: &str = "antigravity-subscription-fixture";
const CODEX_REF: &str = "codex + fixture.json";
const ANTIGRAVITY_REF: &str = "antigravity-fixture.json";
const PARENT: &str = "subscription-parent";

struct Fixture {
    root: tempfile::TempDir,
    cli: PathBuf,
    router: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let router = root.path().join("router-fixture");
        fs::write(&router, include_str!("../fixtures/subscription-router.py"))?;
        fs::set_permissions(&router, fs::Permissions::from_mode(0o700))?;
        let cli = root.path().join("claude-subscription-fixture");
        fs::write(
            &cli,
            r#"#!/bin/sh
set -eu
case "$*" in
  *--fork-session*) phase=fork ;;
  *'--permission-mode plan'*) phase=permission ;;
  *--resume*) phase=resume ;;
  *) phase=initial ;;
esac
[ "$ANTHROPIC_API_KEY" = matou-local-role-gateway ]
[ -z "${ANTHROPIC_AUTH_TOKEN-}" ]
[ -z "${CLAUDE_CODE_OAUTH_TOKEN-}" ]
printf '%s\n' "$ANTHROPIC_BASE_URL" "${TZ-}" "$ANTHROPIC_DEFAULT_OPUS_MODEL" "$ANTHROPIC_DEFAULT_FABLE_MODEL" "$ANTHROPIC_DEFAULT_SONNET_MODEL" "$ANTHROPIC_DEFAULT_HAIKU_MODEL" "$@" > "$0.$phase"
if [ ! -e "$0.skip-messages" ]; then
    for model in "$ANTHROPIC_DEFAULT_OPUS_MODEL" "$ANTHROPIC_DEFAULT_FABLE_MODEL" "$ANTHROPIC_DEFAULT_SONNET_MODEL" "$ANTHROPIC_DEFAULT_HAIKU_MODEL"; do
        /usr/bin/curl --silent --fail --max-time 8 --noproxy '*' \
            -H 'Content-Type: application/json' --data "{\"model\":\"$model\",\"fixture\":\"pty-$phase\"}" \
            "$ANTHROPIC_BASE_URL/v1/messages" >/dev/null
    done
fi
printf 'subscription %s ready\n' "$phase"
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700))?;
        Ok(Self { root, cli, router })
    }

    fn spawn(&self, trace: Option<&str>) -> Result<RuntimeChild> {
        self.spawn_with_shell(trace, Path::new("/bin/zsh"))
    }

    fn spawn_with_shell(&self, trace: Option<&str>, shell: &Path) -> Result<RuntimeChild> {
        RuntimeChild::spawn_configured_with(
            self.root.path(),
            &format!("PROMPT='{PROMPT} '\n"),
            shell,
            Some(&self.cli),
            |command| {
                command
                    .env("MATOU_MODEL_ROUTER_BIN", &self.router)
                    .env("NO_PROXY", "*")
                    .env("no_proxy", "*")
                    .env("TZ", "Asia/Shanghai")
                    .env_remove("ANTHROPIC_BASE_URL")
                    .env_remove("ANTHROPIC_API_KEY")
                    .env_remove("ANTHROPIC_AUTH_TOKEN")
                    .env_remove("CLAUDE_CODE_OAUTH_TOKEN");
                if let Some(trace) = trace {
                    command
                        .env_remove("MATOU_DISABLE_LIVE_EGRESS")
                        .env("MATOU_EGRESS_TRACE_BASE_URL", trace);
                }
            },
        )
    }

    fn launch(&self, host: &mut RuntimeChild, phase: &str) -> Result<Launch> {
        let marker = format!("subscription {phase} ready");
        let deadline = Instant::now() + TIMEOUT;
        let session = loop {
            match host
                .event(deadline, true)
                .with_context(|| format!("waiting for {phase} Claude CLI"))?
            {
                Event::Frame {
                    session_id, screen, ..
                } if screen_has_line(&screen, &marker)
                    && (phase != "initial" || session_id == host.session) =>
                {
                    break session_id;
                }
                Event::Recovery {
                    state: RecoveryState::Failed(error) | RecoveryState::StartFailed(error),
                    ..
                }
                | Event::NavigationFailed { message: error, .. }
                | Event::Diagnostic(error) => bail!("{phase} Claude CLI: {error}"),
                Event::Graph(value) if value["forkSessionResult"].get("error").is_some() => {
                    bail!("fork failed: {}", value["forkSessionResult"])
                }
                _ => {}
            }
        };
        let captured = fs::read_to_string(self.cli.with_extension(phase))?;
        let lines: Vec<_> = captured.lines().collect();
        let [
            url,
            tz,
            opus,
            fable,
            sonnet,
            haiku,
            "--settings",
            settings,
            args @ ..,
        ] = lines.as_slice()
        else {
            bail!("unexpected Claude CLI capture: {captured}");
        };
        let settings = PathBuf::from(settings);
        let config: Value = serde_json::from_slice(&fs::read(&settings)?)?;
        let env = &config["env"];
        assert_eq!(env["ANTHROPIC_BASE_URL"], *url);
        assert_eq!(env["ANTHROPIC_API_KEY"], "matou-local-role-gateway");
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "");
        assert_eq!(env["CLAUDE_CODE_OAUTH_TOKEN"], "");
        for (role, model) in [
            ("OPUS", opus),
            ("FABLE", fable),
            ("SONNET", sonnet),
            ("HAIKU", haiku),
        ] {
            assert_eq!(env[format!("ANTHROPIC_DEFAULT_{role}_MODEL")], *model);
        }
        assert_eq!(
            config["modelPicker"]["options"].as_array().unwrap().len(),
            4
        );
        for id in ["claude-fable-5", "claude-fable-5-2", "claude-fable-5-5"] {
            assert_eq!(config["modelOverrides"][id], *fable);
        }
        let keys = host.data.join("model-router/keys.json");
        if keys.exists() {
            let keys: Value = serde_json::from_slice(&fs::read(keys)?)?;
            for key in ["client", "management"] {
                let key = keys[key].as_str().unwrap();
                assert!(!captured.contains(key) && !config.to_string().contains(key));
            }
        }
        assert!(!config.to_string().contains(CODEX_REF));
        assert!(!config.to_string().contains(ANTIGRAVITY_REF));
        Ok(Launch {
            session,
            url: url::Url::parse(url)?,
            settings,
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            tz: (*tz).to_owned(),
            config,
        })
    }
}

struct Launch {
    session: String,
    url: url::Url,
    settings: PathBuf,
    args: Vec<String>,
    tz: String,
    config: Value,
}

fn model_catalog(ids: &[&str]) -> Vec<Value> {
    ids.iter()
        .map(|id| {
            json!({"id": id, "contextWindow": 600000, "maxOutputTokens": 32000,
        "supportedEfforts": ["high", "medium"]})
        })
        .collect()
}

fn row(id: &str, provider: &str, name: &str, index: &str, prefix: &str, fable: bool) -> Value {
    let opus = format!("{prefix}-opus");
    let sonnet = format!("{prefix}-sonnet");
    let haiku = format!("{prefix}-haiku");
    let fable_model = if provider == "antigravity" {
        "anthropic/claude-fable-5-2"
    } else {
        "codex-fable"
    };
    let mut mapping = json!({"opus": opus, "sonnet": sonnet, "haiku": haiku});
    if fable {
        mapping["fable"] = json!(fable_model);
    }
    let extra = format!("{prefix}-extra");
    let search = format!("{prefix}-search");
    let compact = format!("{prefix}-compact");
    json!({
        "id": id, "kind": "subscription", "cli": "claude-code", "name": prefix, "endpoint": "",
        "model": sonnet, "modelMapping": mapping,
        "effortMapping": {"opus": "high", "fable": "medium"}, "subscription": {
            "provider": provider, "credentialRef": {"name": name, "authIndex": index},
            "account": {"email": format!("{prefix}@test.invalid"), "status": "ready"},
            "modelMetadata": model_catalog(&[&opus, fable_model, &sonnet, &haiku, &extra, &search, &compact])
        }
    })
}

fn codex(fable: bool) -> Value {
    row(CODEX_ID, "codex", CODEX_REF, "c-index", "codex", fable)
}

fn antigravity() -> Value {
    row(
        ANTIGRAVITY_ID,
        "antigravity",
        ANTIGRAVITY_REF,
        "a-index",
        "agy",
        true,
    )
}

fn api() -> Value {
    json!({"id": "api", "name": "API placeholder", "cli": "claude-code",
        "endpoint": "http://127.0.0.1:1", "apiKey": "fixture-api-key", "model": "api-primary",
        "modelMapping": {"opus":"api-primary", "sonnet":"api-primary", "haiku":"api-primary"},
        "modelMetadata": model_catalog(&["api-primary"]) })
}

fn write_config(host: &RuntimeChild, active: &str, rows: &[Value]) -> Result<()> {
    fs::write(
        host.data.join("provider-config.json"),
        json!({
            "version": 1, "activeProviderIds": {"claude-code": active},
            "providers": {"claude-code": rows},
        })
        .to_string(),
    )?;
    Ok(())
}

fn credential(host: &RuntimeChild, row: &Value, models: &[&str]) -> Result<()> {
    let subscription = &row["subscription"];
    let files = host.data.join("model-router/credentials");
    fs::create_dir_all(&files)?;
    fs::write(
        files.join(subscription["credentialRef"]["name"].as_str().unwrap()),
        json!({
            "provider": subscription["provider"], "status": "active",
            "auth_index": subscription["credentialRef"]["authIndex"],
            "email": subscription["account"]["email"], "models": models
        })
        .to_string(),
    )?;
    Ok(())
}

fn settings(
    host: &mut RuntimeChild,
    command: Command,
    request_id: u64,
) -> Result<ProviderSettingsResult> {
    host.send(command)?;
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match host.event(deadline, true)? {
            Event::Graph(graph) if graph["providerSettingsResult"]["requestId"] == request_id => {
                return Ok(serde_json::from_value::<ProviderSettingsReply>(
                    graph["providerSettingsResult"].clone(),
                )?
                .result);
            }
            Event::Diagnostic(error) => bail!("settings worker: {error}"),
            _ => {}
        }
    }
}

fn activate(host: &mut RuntimeChild, id: &str, request_id: u64) -> Result<ProviderSettingsResult> {
    settings(
        host,
        Command::SetActiveProvider {
            request_id,
            cli: "claude-code".into(),
            id: id.into(),
        },
        request_id,
    )
}

fn router_events(host: &RuntimeChild, event: &str) -> Result<Vec<Value>> {
    let path = host.data.join("model-router/fixture-events.jsonl");
    let contents = fs::read_to_string(path).unwrap_or_default();
    Ok(contents
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<serde_json::Result<Vec<_>>>()?
        .into_iter()
        .filter(|row| row["event"] == event)
        .collect())
}

fn gateway(launch: &Launch, body: Value, header: &str) -> Result<BufReader<TcpStream>> {
    gateway_http(launch, body, header, "1.1")
}

fn gateway_http(
    launch: &Launch,
    body: Value,
    header: &str,
    version: &str,
) -> Result<BufReader<TcpStream>> {
    let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, launch.url.port().unwrap()))?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    let body = body.to_string();
    write!(
        socket,
        "POST {}/v1/messages?fixture=1 HTTP/{version}\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nAuthorization: Bearer discard-me\r\n{header}\r\n{body}",
        launch.url.path(),
        body.len()
    )?;
    Ok(BufReader::new(socket))
}

fn response(mut socket: BufReader<TcpStream>) -> Result<(u16, String)> {
    let mut status = String::new();
    socket.read_line(&mut status)?;
    let status = status
        .split_ascii_whitespace()
        .nth(1)
        .context("HTTP status")?
        .parse()?;
    let mut rest = String::new();
    socket.read_to_string(&mut rest)?;
    Ok((status, rest))
}

fn request(launch: &Launch, model: &str, fixture: &str) -> Result<(u16, String)> {
    response(gateway(
        launch,
        json!({"model": model, "fixture": fixture}),
        "",
    )?)
}

fn assert_auth_error(
    (status, response): (u16, String),
    expected_status: u16,
    message: &str,
) -> Result<()> {
    assert_eq!(status, expected_status);
    let (headers, body) = response.split_once("\r\n\r\n").context("HTTP headers")?;
    assert!(headers.contains("content-type: application/json"));
    assert!(!headers.contains("content-encoding:"));
    assert!(!headers.contains("x-upstream:"));
    assert_eq!(
        serde_json::from_str::<Value>(body)?,
        json!({"type": "error", "error": {"type": "authentication_error", "message": message}})
    );
    Ok(())
}

fn english_subscription_launch() -> Result<(Fixture, RuntimeChild, Launch)> {
    let fixture = Fixture::new()?;
    let mut host = fixture.spawn(None)?;
    host.hello()?;
    fs::write(host.data.join("locale.json"), r#"{"preference":"en"}"#)?;
    let row = antigravity();
    write_config(&host, ANTIGRAVITY_ID, std::slice::from_ref(&row))?;
    credential(&host, &row, &["agy-opus", "agy-haiku"])?;
    preferences(
        &mut host,
        json!({
            "opus": target(ANTIGRAVITY_ID, "agy-opus"),
            "sonnet": target(ANTIGRAVITY_ID, "agy-sonnet"),
            "haiku": target(ANTIGRAVITY_ID, "agy-haiku"),
        }),
        Some(target(ANTIGRAVITY_ID, "agy-search")),
        Some(compact(ANTIGRAVITY_ID, "agy-compact")),
    )?;
    host.input_bytes(b"claude\r")?;
    let launch = fixture.launch(&mut host, "initial")?;
    Ok((fixture, host, launch))
}

#[test]
fn managed_subscription_classifies_only_bounded_auth_errors() -> Result<()> {
    let account_status = |host: &mut RuntimeChild| -> Result<Value> {
        let ProviderSettingsResult::Snapshot(snapshot) =
            settings(host, Command::GetProviderSettings { request_id: 56 }, 56)?
        else {
            bail!("provider status snapshot missing");
        };
        Ok(snapshot["providers"]["claude-code"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == ANTIGRAVITY_ID))
            .context("subscription provider snapshot")?["subscription"]["account"]["status"]
            .clone())
    };
    let body = |fixture| json!({"model": "matou-role/opus", "fixture": fixture});

    let (_fixture, mut host, launch) = english_subscription_launch()?;
    for (operation, expected_status, header) in [
        ("rate", 429, "retry-after: 7"),
        ("ordinary-bad-gateway", 502, "x-upstream: fixture"),
        ("large", 502, "x-upstream: fixture"),
        ("large-gzip", 429, "content-encoding: gzip"),
    ] {
        let mut socket = gateway_http(&launch, body(operation), "", "1.0")?;
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            ensure!(socket.read_line(&mut line)? > 0, "missing HTTP headers");
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
        }
        assert_eq!(
            headers.split_ascii_whitespace().nth(1),
            Some(expected_status.to_string().as_str())
        );
        assert!(headers.lines().any(|line| line == header), "{headers}");
        let mut actual = Vec::new();
        socket.read_to_end(&mut actual)?;
        let expected = match operation {
            "rate" => br#"{"model": "matou-antigravity/agy-opus"}"#.to_vec(),
            "ordinary-bad-gateway" => br#"{"error":{"message":"upstream overloaded"}}"#.to_vec(),
            "large" => [
                br#"{"error":{"code":"token_expired","message":""#.as_slice(),
                &vec![b'x'; 256 * 1024],
                br#""}}"#,
            ]
            .concat(),
            _ => fs::read(host.data.join("model-router/fixture-large-gzip"))?,
        };
        assert!(
            actual == expected,
            "{operation}: {} bytes instead of {}",
            actual.len(),
            expected.len()
        );
        assert_eq!(account_status(&mut host)?, "ready");
    }
    stop(&mut host)?;

    for (operation, status, label) in [
        ("expired", 429, "Web search"),
        ("expired-gzip", 500, "compaction"),
        ("expired-deflate", 502, "opus"),
    ] {
        let (_fixture, mut host, launch) = english_subscription_launch()?;
        let mut payload = body(operation);
        let marker = match label {
            "Web search" => {
                payload["tools"] = json!([{"type": "web_search_20250305", "name": "web_search"}]);
                ""
            }
            "compaction" => {
                payload["model"] = json!("unassigned-model");
                "x-claude-code-compaction: manual\r\n"
            }
            _ => "",
        };
        assert_auth_error(
            response(gateway(&launch, payload, marker)?)?,
            status,
            &format!(
                "{label} · agy sign-in has expired. Go to Settings → Claude Code, sign in again, then retry."
            ),
        )?;
        assert_eq!(account_status(&mut host)?, "needs-sign-in");
        stop(&mut host)?;
    }
    Ok(())
}

fn confirm(
    host: &mut RuntimeChild,
    launch: &Launch,
    conversation: &str,
    mode: &str,
    model: &str,
) -> Result<()> {
    post_claude_identity(&launch.settings, conversation, mode, model)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = host
            .event(deadline, true)
            .with_context(|| format!("confirm Claude identity {conversation}"))?
            && graph["providerIdentityEvents"]
                .as_array()
                .is_some_and(|events| {
                    events.iter().any(|event| {
                        event["result"] == "confirmed" && event["sessionId"] == launch.session
                    })
                })
        {
            let db = Connection::open(host.data.join("matou.sqlite"))?;
            let binding =
                claude_binding(&db, &launch.session)?.context("Claude session binding")?;
            assert_eq!(binding.provider_session_id, conversation);
            assert!(!binding.metadata.to_string().contains(CODEX_REF));
            return Ok(());
        }
    }
}

fn stop(host: &mut RuntimeChild) -> Result<()> {
    host.send(Command::Shutdown)?;
    assert!(host.closed()?.is_empty());
    let (status, stderr) = host.wait()?;
    ensure!(status.success(), "{stderr}");
    for row in router_events(host, "engine-start")? {
        assert_shell_gone(row["pid"].as_u64().unwrap() as u32)?;
    }
    assert!(router_events(host, "wrong-management-bearer")?.is_empty());
    assert!(router_events(host, "wrong-inference-bearer")?.is_empty());
    Ok(())
}

#[test]
fn codex_subscription_preflight_promotion_resume_permission_restart_and_fork() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut parent = fixture.spawn(None)?;
    parent.hello()?;
    let codex = codex(false);
    write_config(&parent, "api", &[api(), codex.clone()])?;
    credential(
        &parent,
        &codex,
        &["codex-opus", "codex-sonnet", "codex-haiku"],
    )?;
    let mode = parent.data.join("model-router/fixture-mode");
    fs::write(&mode, "wrong-version")?;
    let config_path = parent.data.join("provider-config.json");
    let before = fs::read(&config_path)?;
    assert!(
        matches!(activate(&mut parent, CODEX_ID, 1)?, ProviderSettingsResult::Error(message) if message.contains("8.0.69"))
    );
    assert_eq!(before, fs::read(&config_path)?);

    fs::remove_file(mode)?;
    let active = activate(&mut parent, CODEX_ID, 2)?;
    assert!(
        matches!(active, ProviderSettingsResult::Snapshot(snapshot) if
        snapshot["activeProviderIds"]["claude-code"] == CODEX_ID && !snapshot.to_string().contains(CODEX_REF))
    );
    let db = Connection::open(parent.data.join("matou.sqlite"))?;
    save_claude_time_zone(&db, "UTC", 1)?;
    parent.input_bytes(b"claude\r")?;
    let initial = fixture.launch(&mut parent, "initial")?;
    let model = "matou-chatgpt/matou-opus";
    assert_eq!(initial.args, ["--model", model, "--effort", "high"]);
    assert_eq!(initial.tz, "UTC");
    assert_eq!(initial.config["autoCompactWindow"], 512000);
    assert_eq!(
        initial.config["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"],
        "32000"
    );
    let pty_requests = router_events(&parent, "message")?;
    assert_eq!(
        pty_requests
            .iter()
            .map(|row| row["model"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            model,
            "matou-chatgpt/matou-fable",
            "matou-chatgpt/matou-sonnet",
            "matou-chatgpt/matou-haiku"
        ]
    );
    assert!(
        request(&initial, "claude-fable-5-2", "literal-fable")?
            .1
            .contains("matou-chatgpt/matou-fable")
    );
    assert!(
        request(&initial, "matou-chatgpt/matou-opus-fallback", "old-fable")?
            .1
            .contains("matou-chatgpt/matou-opus-fallback")
    );
    let (status, payload) = request(&initial, model, "retry")?;
    assert_eq!(status, 200, "{payload}");
    let replayed = router_events(&parent, "message")?
        .into_iter()
        .filter(|event| event["operation"] == "retry")
        .collect::<Vec<_>>();
    assert_eq!(replayed.len(), 2);
    assert_ne!(replayed[0]["port"], replayed[1]["port"]);
    assert_eq!(replayed[1]["query"], "fixture=1");
    let rate = request(&initial, model, "rate")?;
    assert_eq!(rate.0, 429);
    assert!(rate.1.contains("retry-after: 7"));
    assert_eq!(
        router_events(&parent, "message")?
            .iter()
            .filter(|event| event["operation"] == "rate")
            .count(),
        1
    );

    let mut stream = gateway(
        &initial,
        json!({"model": model, "fixture": "sse-crash", "stream": true}),
        "",
    )?;
    let mut first_frames = String::new();
    while !first_frames.contains("event: message_start") {
        ensure!(
            stream.read_line(&mut first_frames)? > 0,
            "SSE ended before its first event"
        );
    }
    assert!(stream.read_line(&mut String::new())? > 0);
    let _ = stream.read_to_string(&mut String::new());
    assert_eq!(
        router_events(&parent, "message")?
            .iter()
            .filter(|event| event["operation"] == "sse-crash")
            .count(),
        1
    );
    confirm(&mut parent, &initial, PARENT, "default", model)?;
    stop(&mut parent).context("shut down promoter and router")?;

    let mut resumed_host = fixture.spawn(None)?;
    resumed_host.connect()?;
    let resumed = fixture.launch(&mut resumed_host, "resume")?;
    assert_eq!(resumed.session, initial.session);
    assert!(
        resumed
            .args
            .windows(2)
            .any(|args| args == ["--resume", PARENT])
    );
    confirm(&mut resumed_host, &resumed, PARENT, "default", model)?;
    resumed_host.send(Command::SetProviderPermissionMode {
        session_id: resumed.session.clone(),
        mode: "plan".into(),
    })?;
    let restarted = fixture.launch(&mut resumed_host, "permission")?;
    assert!(
        restarted
            .args
            .windows(2)
            .any(|args| args == ["--permission-mode", "plan"])
    );
    post_claude_identity(&restarted.settings, PARENT, "plan", model)?;
    assert!(
        permission_mode_result(&mut resumed_host, "plan")?
            .get("error")
            .is_none()
    );

    for hook in ["UserPromptSubmit", "Stop"] {
        assert_eq!(
            post_claude_hook(
                &restarted.settings,
                json!({
                    "hook_event_name": hook, "session_id": PARENT, "model": model,
                })
            )?,
            200
        );
    }
    let deadline = Instant::now() + TIMEOUT;
    while claude_binding(&db, &resumed.session)?
        .context("resumed binding")?
        .metadata["canFork"]
        != true
    {
        resumed_host
            .event(deadline, true)
            .context("wait until Claude permits deriving the conversation")?;
    }
    resumed_host.send(Command::ForkSession {
        scene_id: resumed_host.scene.clone(),
        source_session_id: resumed.session.clone(),
        placement: ForkPlacement::Child,
        name: "Subscription fork".into(),
        worktree_mode: ForkWorktreeMode::Current,
        submission_key: "subscription-fork".into(),
    })?;
    let forked = fixture.launch(&mut resumed_host, "fork")?;
    assert_ne!(forked.session, resumed.session);
    assert!(forked.args.contains(&"--fork-session".into()));
    confirm(
        &mut resumed_host,
        &forked,
        "subscription-child",
        "plan",
        model,
    )?;
    assert_eq!(request(&restarted, model, "parent-still-live")?.0, 200);
    stop(&mut resumed_host)
}

fn preferences(
    host: &mut RuntimeChild,
    assignments: Value,
    search: Option<Value>,
    compact: Option<Value>,
) -> Result<()> {
    host.send(Command::SetClaudeModelPreferences {
        request_id: 20,
        assignments,
        web_search: search,
        compaction: compact,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = host.event(deadline, true)?
            && graph["claudeRoleAssignmentsResult"]["requestId"] == 20
        {
            ensure!(
                graph["claudeRoleAssignmentsResult"].get("error").is_none(),
                "{}",
                graph["claudeRoleAssignmentsResult"]
            );
            return Ok(());
        }
    }
}

#[test]
fn subscription_settings_worker_reads_once_and_persists_explicit_refresh() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut host = fixture.spawn(None)?;
    host.hello()
        .context("handshake before creating subscription fixture")?;
    let row = codex(true);
    write_config(&host, CODEX_ID, std::slice::from_ref(&row))
        .context("seed stored subscription")?;
    credential(
        &host,
        &row,
        &["codex-opus", "codex-fable", "codex-sonnet", "codex-haiku"],
    )?;
    assert!(matches!(
        activate(&mut host, CODEX_ID, 79)?,
        ProviderSettingsResult::Snapshot(_)
    ));
    assert!(router_events(&host, "account-probe")?.is_empty());

    let open = |host: &mut RuntimeChild, id| {
        settings(
            host,
            Command::OpenSubscriptionSettings {
                request_id: id,
                settings_id: id,
            },
            id,
        )
    };
    let close = |host: &mut RuntimeChild, id| {
        settings(
            host,
            Command::CloseSubscriptionSettings {
                request_id: id + 10,
                settings_id: id,
            },
            id + 10,
        )
    };
    let ProviderSettingsResult::SubscriptionSettings(view) = open(&mut host, 80)? else {
        bail!("expected account details on settings open")
    };
    assert_eq!(view.accounts[CODEX_ID].account["plan"], "Pro 200");
    assert_eq!(view.accounts[CODEX_ID].quota[0].used_percent, 35.0);
    assert_eq!(router_events(&host, "account-probe")?.len(), 1);
    let wire = serde_json::to_string(&view)?;
    assert!(
        !wire.contains("credentialRef") && !wire.contains(CODEX_REF) && !wire.contains("c-index")
    );
    assert!(matches!(
        close(&mut host, 80)?,
        ProviderSettingsResult::Accepted(true)
    ));

    assert!(matches!(
        open(&mut host, 81)?,
        ProviderSettingsResult::SubscriptionSettings(_)
    ));
    assert_eq!(router_events(&host, "account-probe")?.len(), 1);
    fs::write(
        host.data.join("model-router/fixture-catalog-mode"),
        "updated",
    )?;
    let refresh = settings(
        &mut host,
        Command::RefreshSubscriptionModels {
            request_id: 82,
            settings_id: 81,
            provider_id: CODEX_ID.into(),
        },
        82,
    )?;
    let ProviderSettingsResult::SubscriptionModels(view) = refresh else {
        bail!("expected refreshed subscription catalog")
    };
    assert!(
        view.details
            .models
            .iter()
            .any(|model| model["id"] == "fixture-updated-model")
    );
    assert_eq!(router_events(&host, "account-probe")?.len(), 2);
    preferences(
        &mut host,
        json!({"fable": {"providerId": CODEX_ID, "model": "codex-fable"}}),
        Some(json!({"providerId": CODEX_ID, "model": "codex-haiku"})),
        None,
    )?;
    preferences(
        &mut host,
        Value::Null,
        None,
        Some(
            json!({"mode": "custom", "providerId": CODEX_ID, "model": "codex-fable", "effort": "low"}),
        ),
    )?;
    host.send(Command::GetClaudeModelPreferences { request_id: 83 })?;
    loop {
        if let Event::Graph(graph) = host.event(Instant::now() + TIMEOUT, true)?
            && graph["claudeRoleAssignmentsResult"]["requestId"] == 83
        {
            let settings = &graph["claudeRoleAssignmentsResult"];
            assert_eq!(settings["assignments"]["fable"]["model"], "codex-fable");
            assert_eq!(settings["webSearch"]["model"], "codex-haiku");
            assert_eq!(settings["compaction"]["model"], "codex-fable");
            break;
        }
    }
    assert!(matches!(
        close(&mut host, 81)?,
        ProviderSettingsResult::Accepted(true)
    ));
    let port = router_events(&host, "engine-start")?.last().unwrap()["port"]
        .as_u64()
        .unwrap() as u16;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err() {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "settings lease remained after close"
        );
        thread::sleep(Duration::from_millis(20));
    }
    stop(&mut host)
}

struct Trace {
    url: String,
    mainland: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Trace {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let mainland = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let country = Arc::clone(&mainland);
        let quit = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !quit.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        socket
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut io = BufReader::new(socket);
                        loop {
                            let mut line = String::new();
                            if io.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                                break;
                            }
                        }
                        let loc = if country.load(Ordering::Acquire) {
                            "CN"
                        } else {
                            "TW"
                        };
                        let data = format!("ip=203.0.113.95\nloc={loc}\ncolo=SIN\n");
                        let _ = write!(
                            io.get_mut(),
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",
                            data.len()
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture trace: {error}"),
                }
            }
        });
        Ok(Self {
            url,
            mainland,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn egress(host: &mut RuntimeChild, command: Command, request_id: u64) -> Result<Value> {
    host.send(command)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = host.event(deadline, true)?
            && graph["requestId"] == request_id
            && graph["type"]
                .as_str()
                .is_some_and(|name| name.starts_with("egress"))
        {
            ensure!(graph.get("error").is_none(), "{graph}");
            return Ok(graph["result"].clone());
        }
    }
}

fn compact(id: &str, model: &str) -> Value {
    json!({"mode": "custom", "providerId": id, "model": model, "effort": "follow"})
}

fn target(id: &str, model: &str) -> Value {
    json!({"providerId": id, "model": model})
}

fn family(status: &Value, kind: &str) -> Option<Value> {
    status["families"]
        .as_array()?
        .iter()
        .find(|family| family["family"] == kind)
        .cloned()
}

fn next_workspace(host: &mut RuntimeChild, path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    host.send(Command::CreateWorkspace {
        root_directory: path.to_string_lossy().into_owned(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Navigation { hierarchy, .. } = host.event(deadline, true)?
            && hierarchy
                .workspace
                .is_some_and(|workspace| Path::new(&workspace.root_directory) == path)
        {
            host.session = hierarchy.session.context("new workspace shell")?.id;
            break;
        }
    }
    loop {
        if let Event::Frame {
            session_id, screen, ..
        } = host.event(deadline, true)?
            && session_id == host.session
            && screen_has_line(&screen, PROMPT)
        {
            return Ok(());
        }
    }
}

#[test]
fn mixed_targets_route_exact_models_and_revoke_only_the_replaced_subscription() -> Result<()> {
    let fixture = Fixture::new()?;
    let trace = Trace::start()?;
    let mut host = fixture.spawn(Some(&trace.url))?;
    host.hello()?;
    fs::write(host.data.join("locale.json"), r#"{"preference":"zh-CN"}"#)?;
    let codex = codex(false);
    let agy = antigravity();
    write_config(&host, CODEX_ID, &[api(), codex.clone(), agy.clone()])?;
    credential(&host, &codex, &["codex-opus", "codex-sonnet"])?;
    credential(&host, &agy, &["agy-opus", "agy-haiku"])?;
    let status = egress(&mut host, Command::RecheckEgress { request_id: 90 }, 90)?;
    assert_eq!(
        family(&status, "chatgpt-subscription").context("Codex exit")?["exit"]["country"],
        "TW"
    );
    preferences(
        &mut host,
        json!({
            "opus": {"providerId": ANTIGRAVITY_ID, "model": "agy-opus", "effort": "high"},
            "sonnet": {"providerId": CODEX_ID, "model": "codex-extra"},
            "haiku": {"providerId": ANTIGRAVITY_ID, "model": "agy-haiku"}
        }),
        Some(target(CODEX_ID, "codex-search")),
        Some(compact(ANTIGRAVITY_ID, "agy-compact")),
    )?;
    host.input_bytes(b"claude\r")?;
    let launch = fixture.launch(&mut host, "initial")?;
    assert_eq!(launch.tz, "Asia/Taipei");
    assert_eq!(
        launch.config["env"]["ANTHROPIC_DEFAULT_FABLE_MODEL"],
        "matou-role/fable"
    );
    let first = router_events(&host, "message")?;
    assert_eq!(
        first
            .iter()
            .map(|event| event["model"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "matou-antigravity/agy-opus",
            "matou-antigravity/agy-opus",
            "matou-chatgpt/codex-extra",
            "matou-antigravity/agy-haiku"
        ]
    );
    assert_eq!(first[1]["effort"]["effort"], "high");
    assert!(router_events(&host, "aliases")?.is_empty());
    assert!(router_events(&host, "patch")?.iter().any(|event| {
        event["name"] == CODEX_REF
            && event["fields"]
                .as_array()
                .unwrap()
                .contains(&json!("extra_models"))
    }));

    let search = response(gateway(
        &launch,
        json!({
            "model": "matou-role/haiku",
            "tools": [{"type": "web_search_20250305", "name": "web_search"}],
            "messages": [{"role": "user", "content": "fixture query"}],
        }),
        "",
    )?)?;
    assert_eq!(search.0, 200);
    assert!(search.1.contains("matou-chatgpt/codex-search"));
    let compacted = response(gateway(
        &launch,
        json!({"model": "matou-role/sonnet"}),
        "x-claude-code-compaction: manual\r\n",
    )?)?;
    assert_eq!(compacted.0, 200);
    assert!(compacted.1.contains("matou-antigravity/agy-compact"));
    let status = egress(&mut host, Command::GetEgressStatus { request_id: 91 }, 91)?;
    assert_eq!(
        status["sessionFamilies"][&launch.session],
        json!(["chatgpt-subscription"])
    );
    trace.mainland.store(true, Ordering::Release);
    let status = egress(&mut host, Command::RecheckEgress { request_id: 92 }, 92)?;
    assert!(
        family(&status, "chatgpt-subscription").unwrap()["frozenSessionIds"]
            .as_array()
            .unwrap()
            .contains(&json!(launch.session))
    );
    let blocked = request(&launch, "matou-role/sonnet", "frozen")?;
    assert_eq!(blocked.0, 502);
    assert!(blocked.1.contains("egress protection"));
    assert_eq!(
        request(&launch, "matou-role/opus", "unrelated-family")?.0,
        200
    );
    let _ = egress(
        &mut host,
        Command::SetEgressExemption {
            request_id: 93,
            family: Family::ChatgptSubscription,
        },
        93,
    )?;
    assert_eq!(
        request(&launch, "matou-role/sonnet", "after-exemption")?.0,
        200
    );
    trace.mainland.store(false, Ordering::Release);

    for (model, fixture, status, label) in [
        ("matou-role/sonnet", "401", 401, "sonnet · codex"),
        ("matou-role/opus", "403", 403, "opus · agy"),
    ] {
        let result = request(&launch, model, fixture)?;
        assert_auth_error(
            result,
            status,
            &format!("{label} 登录已过期，请到 设置 → Claude Code 重新登录后重试。"),
        )?;
    }
    let ProviderSettingsResult::Snapshot(snapshot) =
        settings(&mut host, Command::GetProviderSettings { request_id: 4 }, 4)?
    else {
        bail!("provider status snapshot missing");
    };
    for id in [CODEX_ID, ANTIGRAVITY_ID] {
        let account = snapshot["providers"]["claude-code"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .context("subscription provider snapshot")?;
        assert_eq!(
            account["subscription"]["account"]["status"],
            "needs-sign-in"
        );
        assert!(account["subscription"].get("credentialRef").is_none());
    }

    let mut held = gateway(
        &launch,
        json!({"model": "matou-role/opus", "fixture": "sse-hold", "stream": true}),
        "",
    )?;
    let mut first_frame = String::new();
    while !first_frame.contains("event: message_start") {
        ensure!(held.read_line(&mut first_frame)? > 0, "SSE did not start");
    }
    assert!(matches!(
        settings(
            &mut host,
            Command::DeleteProvider {
                request_id: 5,
                cli: "claude-code".into(),
                id: ANTIGRAVITY_ID.into()
            },
            5
        )?,
        ProviderSettingsResult::Snapshot(_)
    ));
    held.get_ref()
        .set_read_timeout(Some(Duration::from_secs(2)))?;
    let closed = held.read_to_string(&mut String::new());
    assert!(
        closed.as_ref().err().is_none_or(|error| !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )),
        "{closed:?}"
    );
    assert_eq!(request(&launch, "matou-role/opus", "revoked")?.0, 502);
    assert_eq!(
        request(&launch, "matou-role/sonnet", "other-account-still-live")?.0,
        200
    );

    let mut next = antigravity();
    next["subscription"]["credentialRef"] =
        json!({"name": "new-agy.json", "authIndex": "new-a-index"});
    write_config(&host, ANTIGRAVITY_ID, &[api(), codex, next.clone()])?;
    credential(
        &host,
        &next,
        &[
            "agy-opus",
            "agy-haiku",
            "agy-sonnet",
            "anthropic/claude-fable-5-2",
        ],
    )?;
    preferences(
        &mut host,
        json!({
            "opus": target(ANTIGRAVITY_ID, "agy-opus"),
            "fable": target(ANTIGRAVITY_ID, "anthropic/claude-fable-5-2"),
            "sonnet": target(ANTIGRAVITY_ID, "agy-sonnet"),
            "haiku": target(ANTIGRAVITY_ID, "agy-haiku"),
        }),
        Some(Value::Null),
        Some(json!({"mode":"follow","effort":"follow"})),
    )?;
    next_workspace(&mut host, &fixture.root.path().join("second-workspace"))?;
    host.input_bytes(b"claude\r")?;
    let replacement = fixture.launch(&mut host, "initial")?;
    assert_ne!(replacement.session, launch.session);
    assert_eq!(replacement.tz, "Asia/Singapore");
    assert!(
        request(&replacement, "matou-role/fable", "independent-fable")?
            .1
            .contains("matou-antigravity/anthropic/claude-fable-5-2")
    );
    assert_eq!(
        request(&launch, "matou-role/opus", "old-still-revoked")?.0,
        502
    );
    let status = egress(&mut host, Command::GetEgressStatus { request_id: 94 }, 94)?;
    assert!(status["sessionFamilies"][&replacement.session].is_null());
    stop(&mut host)
}

#[test]
fn complete_api_assignments_do_not_start_the_unused_subscription() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::write(fixture.cli.with_extension("skip-messages"), "")?;
    let mut host = fixture.spawn(None)?;
    host.hello()?;
    write_config(&host, CODEX_ID, &[api(), codex(false)])?;
    preferences(
        &mut host,
        json!({
            "opus": target("api", "api-primary"), "sonnet": target("api", "api-primary"),
            "haiku": target("api", "api-primary")
        }),
        None,
        None,
    )?;
    host.input_bytes(b"claude\r")?;
    let launch = fixture.launch(&mut host, "initial")?;
    assert_eq!(launch.args, ["--model", "matou-role/opus"]);
    assert!(!host.data.join("model-router/keys.json").exists());
    stop(&mut host)
}

#[test]
fn claude_launch_uses_explicit_control_asset_root() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::write(fixture.cli.with_extension("skip-messages"), "")?;
    let assets = fixture.root.path().join("control assets");
    let plugin = assets.join("providers/claude-plugin");
    fs::create_dir_all(plugin.join(".claude-plugin"))?;
    fs::write(plugin.join(".claude-plugin/plugin.json"), "{}")?;
    let mut host = RuntimeChild::spawn_configured_with(
        fixture.root.path(),
        &format!("PROMPT='{PROMPT} '\n"),
        Path::new("/bin/zsh"),
        Some(&fixture.cli),
        |command| {
            command.env("MATOU_CONTROL_ASSET_ROOT", &assets);
        },
    )?;
    host.hello()?;
    write_config(&host, "api", &[api()])?;
    preferences(
        &mut host,
        json!({
            "opus": target("api", "api-primary"), "sonnet": target("api", "api-primary"),
            "haiku": target("api", "api-primary")
        }),
        None,
        None,
    )?;
    host.input_bytes(b"claude\r")?;
    let launch = fixture.launch(&mut host, "initial")?;
    assert_eq!(
        launch.args,
        [
            "--plugin-dir",
            plugin.to_str().unwrap(),
            "--model",
            "matou-role/opus"
        ]
    );
    stop(&mut host)
}

#[test]
fn lazy_compaction_binds_only_on_request_and_watchdog_reaps_after_host_kill() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut seed = fixture.spawn_with_shell(None, &fixture.root.path().join("missing-shell"))?;
    seed.ready()?;
    let codex = codex(true);
    let agy = antigravity();
    write_config(&seed, ANTIGRAVITY_ID, &[api(), codex.clone(), agy.clone()])?;
    credential(
        &seed,
        &agy,
        &[
            "agy-opus",
            "agy-sonnet",
            "agy-haiku",
            "anthropic/claude-fable-5-2",
        ],
    )?;
    credential(&seed, &codex, &["codex-opus"])?;
    preferences(
        &mut seed,
        json!({}),
        None,
        Some(compact(CODEX_ID, "codex-compact")),
    )?;
    seed.send(Command::Shutdown)?;
    seed.closed()?;
    assert!(seed.wait()?.0.success());
    let db = Connection::open(seed.data.join("matou.sqlite"))?;
    db.execute(
        "UPDATE sessions SET kind='claude-code', status='created' WHERE id=?1",
        [&seed.session],
    )?;
    let mut host = fixture.spawn(None)?;
    host.connect()?;
    let launch = fixture.launch(&mut host, "initial")?;
    assert_eq!(
        launch.config["modelOverrides"]["claude-fable-5-2"],
        "matou-antigravity/claude-fable-5-2"
    );
    assert!(
        !router_events(&host, "model-list")?
            .iter()
            .any(|event| event["name"] == CODEX_REF)
    );
    let compacted = response(gateway(
        &launch,
        json!({"model": "matou-antigravity/matou-opus"}),
        "x-claude-code-compaction: auto\r\n",
    )?)?;
    assert_eq!(compacted.0, 200);
    assert!(compacted.1.contains("matou-chatgpt/codex-compact"));
    assert!(
        router_events(&host, "model-list")?
            .iter()
            .any(|event| event["name"] == CODEX_REF)
    );
    let model_lists = router_events(&host, "model-list")?.len();
    let compact = || {
        response(gateway(
            &launch,
            json!({"model": "matou-antigravity/matou-opus"}),
            "x-claude-code-compaction: auto\r\n",
        )?)
    };
    assert_eq!(compact()?.0, 200);
    assert_eq!(router_events(&host, "model-list")?.len(), model_lists);

    assert!(matches!(
        settings(
            &mut host,
            Command::DeleteProvider {
                request_id: 95,
                cli: "claude-code".into(),
                id: CODEX_ID.into(),
            },
            95
        )?,
        ProviderSettingsResult::Snapshot(_)
    ));
    assert_eq!(compact()?.0, 502);
    let mut replacement = codex;
    replacement["subscription"]["credentialRef"] =
        json!({"name": "replacement-codex.json", "authIndex": "replacement-c-index"});
    write_config(&host, ANTIGRAVITY_ID, &[api(), agy, replacement.clone()])?;
    credential(&host, &replacement, &["codex-opus"])?;
    assert!(compact()?.1.contains("matou-chatgpt/codex-compact"));
    assert!(
        router_events(&host, "model-list")?
            .iter()
            .any(|event| event["name"] == "replacement-codex.json")
    );
    let engines = router_events(&host, "engine-start")?;
    assert_eq!(engines.len(), 1);
    host.child.kill()?;
    let (status, _) = host.wait()?;
    assert!(!status.success());
    assert_shell_gone(engines[0]["pid"].as_u64().unwrap() as u32)?;
    Ok(())
}
