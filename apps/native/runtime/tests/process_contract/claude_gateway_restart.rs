use super::*;
use matou_native_core::sessions::{ClaudeBinding, claude_binding, list_runs};
use matou_native_runtime::host::{ForkPlacement, ForkWorktreeMode, RecoveryState};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
};

const PARENT_ID: &str = "gateway-parent-conversation";
const CHILD_ID: &str = "gateway-fork-conversation";
const RESTORED_MODEL: &str = "literal-restored-model";

struct Launch {
    settings: PathBuf,
    address: SocketAddr,
    url: String,
}

fn started(
    runtime: &mut RuntimeChild,
    stub: &Path,
    phase: &str,
    argv: &[&str],
) -> Result<(String, Launch)> {
    let marker = format!("gateway {phase} ready");
    let deadline = Instant::now() + TIMEOUT;
    let session = loop {
        match runtime
            .event(deadline, true)
            .with_context(|| format!("wait for the {phase} CLI"))?
        {
            Event::Frame {
                session_id, screen, ..
            } if screen_has_line(&screen, &marker) => {
                break session_id;
            }
            Event::Recovery {
                state: RecoveryState::Failed(error) | RecoveryState::StartFailed(error),
                ..
            }
            | Event::NavigationFailed { message: error, .. }
            | Event::Diagnostic(error) => bail!("{phase} startup: {error}"),
            Event::Graph(graph) if graph["forkSessionResult"].get("error").is_some() => {
                bail!("fork launch: {}", graph["forkSessionResult"]["error"]);
            }
            _ => {}
        }
    };
    let recorded = fs::read_to_string(stub.with_extension(phase))?;
    let lines: Vec<_> = recorded.lines().collect();
    let [url, "--settings", settings_path, args @ ..] = lines.as_slice() else {
        bail!("substitute CLI did not receive private settings");
    };
    assert_eq!(args, argv);
    let settings = PathBuf::from(settings_path);
    let payload: Value = serde_json::from_slice(&fs::read(&settings)?)?;
    let env = &payload["env"];
    assert_eq!(env["ANTHROPIC_BASE_URL"], *url);
    assert_eq!(env["ANTHROPIC_API_KEY"], "matou-local-role-gateway");
    assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "matou-role/opus");
    assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "matou-role/sonnet");
    assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "matou-role/haiku");
    assert_eq!(env["ANTHROPIC_DEFAULT_FABLE_MODEL"], "matou-role/opus");
    assert_eq!(env["CLAUDE_CODE_GATEWAY_HINT_HEADERS"], "1");
    assert_eq!(env["NO_PROXY"], "fixture.invalid,127.0.0.1,localhost");
    assert_eq!(env["no_proxy"], env["NO_PROXY"]);
    assert_eq!(
        payload["modelPicker"]["options"].as_array().unwrap().len(),
        3
    );
    assert_eq!(
        payload["modelOverrides"]["claude-fable-5"],
        "matou-role/opus-fallback"
    );
    let parsed = url::Url::parse(url)?;
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.path().len(), 65);
    let port = parsed.port().context("per-run gateway port")?;
    Ok((
        session,
        Launch {
            settings,
            address: (Ipv4Addr::LOCALHOST, port).into(),
            url: (*url).to_owned(),
        },
    ))
}

fn upstream_request(upstream: &TcpListener, provider: &str, expected: Value) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    let stream = loop {
        match upstream.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                ensure!(
                    Instant::now() < deadline,
                    "substitute CLI did not contact the upstream"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.into()),
        }
    };
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line)?;
    assert_eq!(
        line.trim(),
        format!("POST /{provider}/v1/messages HTTP/1.1")
    );
    let mut content_length = None;
    let mut api_key = None;
    loop {
        line.clear();
        ensure!(stream.read_line(&mut line)? > 0, "unexpected upstream EOF");
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.trim().split_once(':').context("upstream header")?;
        match name.to_ascii_lowercase().as_str() {
            "content-length" => content_length = Some(value.trim().parse()?),
            "x-api-key" => api_key = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    assert_eq!(
        api_key.as_deref(),
        Some(format!("synthetic-restart-{provider}").as_str())
    );
    let mut body = vec![0; content_length.context("upstream body length")?];
    stream.read_exact(&mut body)?;
    assert_eq!(serde_json::from_slice::<Value>(&body)?, expected);
    stream
        .get_mut()
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")?;
    Ok(())
}

fn search_request(
    upstream: &TcpListener,
    launch: &Launch,
    provider: &str,
    model: &str,
) -> Result<()> {
    let body = json!({
        "model": "matou-role/haiku",
        "tools": [{"type": "web_search_20250305", "name": "web_search"}],
        "messages": [{"role": "user", "content": "snapshot sentinel"}]
    });
    let mut client = TcpStream::connect_timeout(&launch.address, TIMEOUT)?;
    client.set_read_timeout(Some(TIMEOUT))?;
    let url = url::Url::parse(&launch.url)?;
    let bytes = body.to_string();
    write!(
        client,
        "POST {}/v1/messages HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{bytes}",
        url.path(),
        bytes.len()
    )?;
    let mut expected = body;
    expected["model"] = json!(model);
    upstream_request(upstream, provider, expected)?;
    let mut status = String::new();
    BufReader::new(client).read_line(&mut status)?;
    assert!(status.starts_with("HTTP/1.1 200 "), "{status}");
    Ok(())
}

fn confirm(
    runtime: &mut RuntimeChild,
    db: &Connection,
    session: &str,
    launch: &Launch,
    identity: &str,
    mode: &str,
) -> Result<ClaudeBinding> {
    post_claude_identity(&launch.settings, identity, mode, RESTORED_MODEL)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = runtime
            .event(deadline, true)
            .context("wait for CLI identity confirmation")?
            && graph["providerIdentityEvents"]
                .as_array()
                .is_some_and(|events| {
                    events.iter().any(|event| {
                        event["result"] == "confirmed" && event["sessionId"] == session
                    })
                })
        {
            let binding = claude_binding(db, session)?.context("confirmed binding")?;
            assert_eq!(binding.provider_session_id, identity);
            assert_eq!(binding.metadata["model"], RESTORED_MODEL);
            assert_eq!(binding.metadata["permissionMode"], mode);
            return Ok(binding);
        }
    }
}

fn save_sonnet(
    runtime: &mut RuntimeChild,
    request_id: u64,
    provider: &str,
    model: &str,
    search: Option<(&str, &str)>,
) -> Result<()> {
    runtime.send(Command::SetClaudeRoleAssignments {
        request_id,
        web_search: search.map(|(provider, model)| json!({"providerId": provider, "model": model})),
        assignments: json!({"sonnet": {"providerId": provider, "model": model}}),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = runtime.event(deadline, true)?
            && let Some(result) = graph.get("claudeRoleAssignmentsResult")
        {
            let reply: ClaudeRoleAssignmentsReply = serde_json::from_value(result.clone())?;
            assert_eq!(reply.request_id, request_id);
            assert!(reply.error.is_none(), "{:?}", reply.error);
            if let Some((provider, model)) = search {
                assert_eq!(
                    result["webSearch"],
                    json!({"providerId": provider, "model": model})
                );
            }
            return Ok(());
        }
    }
}

fn refused(launch: &Launch) {
    let outcome = TcpStream::connect_timeout(&launch.address, Duration::from_secs(1));
    assert!(
        matches!(&outcome, Err(error) if error.kind() == io::ErrorKind::ConnectionRefused),
        "retired gateway did not refuse connections: {outcome:?}"
    );
}

#[test]
fn claude_gateway_rebuilds_on_cold_resume_fork_and_permission_restart() -> Result<()> {
    assert_gateway_restarts(false)
}

#[test]
fn web_search_gateway_rebuilds_on_cold_resume_fork_and_permission_restart() -> Result<()> {
    assert_gateway_restarts(true)
}

fn assert_gateway_restarts(web_search: bool) -> Result<()> {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let no_shell = root.path().join("missing-shell");
    let mut seed = RuntimeChild::spawn_configured(root.path(), "", &no_shell, None)?;
    seed.ready()?;
    seed.send(Command::Shutdown)?;
    seed.closed()?;
    assert!(seed.wait()?.0.success());

    let upstream = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    upstream.set_nonblocking(true)?;
    let provider = |id| -> Result<Value> {
        let mut provider = json!({
            "id": id, "endpoint": format!("http://{}/{id}", upstream.local_addr()?),
            "apiKey": format!("synthetic-restart-{id}"), "model": "initial-model"
        });
        if web_search {
            provider["modelMapping"] = json!({"opus": "default-opus", "sonnet": "default-sonnet", "haiku": "default-haiku"});
        }
        Ok(provider)
    };
    fs::write(
        seed.data.join("provider-config.json"),
        json!({
            "version": 1, "activeProviderIds": {"claude-code": "a"},
            "providers": {"claude-code": [provider("a")?, provider("b")?]}
        })
        .to_string(),
    )?;
    let db = Connection::open(seed.data.join("matou.sqlite"))?;
    db.execute(
        "UPDATE sessions SET kind='claude-code' WHERE id=?1",
        [&seed.session],
    )?;
    let mut preferences = json!({
        "contextWindows": {},
        "compaction": {"mode": "follow", "effort": "follow"},
        "claudeRoleAssignments": {"sonnet": {"providerId": "a", "model": "first-sonnet"}}
    });
    if web_search {
        preferences["claudeWebSearch"] = json!({"providerId": "b", "model": "first-search"});
    }
    db.execute(
        "INSERT INTO preferences VALUES ('agent.modelPreferences', ?1, 1)",
        [preferences.to_string()],
    )?;
    let stub = root.path().join("gateway-restart-cli");
    fs::write(
        &stub,
        r#"#!/bin/sh
set -eu
case "$*" in
  *--fork-session*) phase=fork ;;
  *'--permission-mode plan'*) phase=permission ;;
  *--resume*) phase=resume ;;
  *) phase=initial ;;
esac
test "$ANTHROPIC_API_KEY" = matou-local-role-gateway
test "$CLAUDE_CODE_GATEWAY_HINT_HEADERS" = 1
test "$NO_PROXY" = fixture.invalid,127.0.0.1,localhost
test "$no_proxy" = "$NO_PROXY"
printf '%s\n' "$ANTHROPIC_BASE_URL" "$@" > "$0.$phase"
printf 'gateway %s ready\n' "$phase"
/usr/bin/curl --silent --fail --max-time 8 --noproxy '*' \
    -H 'content-type: application/json' -H "x-api-key: $ANTHROPIC_API_KEY" \
    --data "{\"model\":\"$ANTHROPIC_DEFAULT_SONNET_MODEL\"}" "$ANTHROPIC_BASE_URL/v1/messages"
printf '\n'
exec /bin/sleep 60
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let spawn = || {
        RuntimeChild::spawn_configured_with(root.path(), "", &no_shell, Some(&stub), |command| {
            command
                .env("no_proxy", "fixture.invalid")
                .env("NO_PROXY", "overridden.invalid");
        })
    };
    let mut parent = spawn()?;
    parent.connect()?;
    let (parent_session, initial) =
        started(&mut parent, &stub, "initial", &["--model", "initial-model"])?;
    assert_eq!(parent_session, seed.session);
    upstream_request(&upstream, "a", json!({"model": "first-sonnet"}))?;
    if web_search {
        search_request(&upstream, &initial, "b", "first-search")?;
    }
    let binding = confirm(
        &mut parent,
        &db,
        &parent_session,
        &initial,
        PARENT_ID,
        "default",
    )?;
    let initial_run = list_runs(&db, &parent_session)?.remove(0);
    save_sonnet(
        &mut parent,
        1,
        "b",
        "resumed-sonnet",
        web_search.then_some(("a", "resumed-search")),
    )?;
    if web_search {
        search_request(&upstream, &initial, "b", "first-search")?;
    }
    parent.send(Command::Shutdown)?;
    parent.closed()?;
    assert!(parent.wait()?.0.success());
    refused(&initial);

    let mut runtime = spawn()?;
    runtime.connect()?;
    let (restored_session, resumed) = started(
        &mut runtime,
        &stub,
        "resume",
        &["--model", RESTORED_MODEL, "--resume", PARENT_ID],
    )?;
    assert_eq!(restored_session, parent_session);
    assert_ne!(initial.url, resumed.url);
    refused(&initial);
    upstream_request(&upstream, "b", json!({"model": "resumed-sonnet"}))?;
    if web_search {
        search_request(&upstream, &resumed, "a", "resumed-search")?;
    }
    assert_eq!(
        confirm(
            &mut runtime,
            &db,
            &parent_session,
            &resumed,
            PARENT_ID,
            "default"
        )?
        .id,
        binding.id
    );
    let resumed_run = list_runs(&db, &parent_session)?
        .into_iter()
        .find(|run| run.status == "running")
        .context("resumed run")?;
    assert_ne!(initial_run.id, resumed_run.id);
    println!(
        "Cold resume: new run used the updated Sonnet assignment, confirmed the original identity, and the old listener refused connections"
    );

    for event in ["UserPromptSubmit", "Stop"] {
        assert_eq!(
            post_claude_hook(
                &resumed.settings,
                json!({
                    "hook_event_name": event, "session_id": PARENT_ID, "model": RESTORED_MODEL
                })
            )?,
            200
        );
    }
    let deadline = Instant::now() + TIMEOUT;
    while claude_binding(&db, &parent_session)?
        .context("parent binding")?
        .metadata["canFork"]
        != true
    {
        runtime.event(deadline, true)?;
    }
    save_sonnet(
        &mut runtime,
        2,
        "a",
        "forked-sonnet",
        web_search.then_some(("b", "forked-search")),
    )?;
    runtime.send(Command::ForkSession {
        scene_id: runtime.scene.clone(),
        source_session_id: parent_session.clone(),
        placement: ForkPlacement::Child,
        name: "Gateway child fixture".into(),
        worktree_mode: ForkWorktreeMode::Current,
        submission_key: "gateway-child".into(),
    })?;
    let (child_session, forked) = started(
        &mut runtime,
        &stub,
        "fork",
        &[
            "--model",
            RESTORED_MODEL,
            "--resume",
            PARENT_ID,
            "--fork-session",
        ],
    )?;
    assert_ne!(child_session, parent_session);
    assert_ne!(forked.url, resumed.url);
    TcpStream::connect_timeout(&resumed.address, Duration::from_secs(1))?;
    upstream_request(&upstream, "a", json!({"model": "forked-sonnet"}))?;
    if web_search {
        search_request(&upstream, &forked, "b", "forked-search")?;
        search_request(&upstream, &resumed, "a", "resumed-search")?;
    }
    confirm(
        &mut runtime,
        &db,
        &child_session,
        &forked,
        CHILD_ID,
        "default",
    )?;
    assert_eq!(
        claude_binding(&db, &parent_session)?
            .context("parent binding")?
            .provider_session_id,
        PARENT_ID
    );
    println!(
        "Fork: child used the newly saved Sonnet assignment and confirmed its own identity, while the live parent listener stayed reachable"
    );

    save_sonnet(
        &mut runtime,
        3,
        "b",
        "permission-sonnet",
        web_search.then_some(("a", "permission-search")),
    )?;
    runtime.send(Command::SetProviderPermissionMode {
        session_id: parent_session.clone(),
        mode: "plan".into(),
    })?;
    let (permission_session, restarted) = started(
        &mut runtime,
        &stub,
        "permission",
        &[
            "--model",
            RESTORED_MODEL,
            "--resume",
            PARENT_ID,
            "--permission-mode",
            "plan",
        ],
    )?;
    assert_eq!(permission_session, parent_session);
    assert_ne!(restarted.url, resumed.url);
    assert_ne!(restarted.url, forked.url);
    refused(&resumed);
    TcpStream::connect_timeout(&forked.address, Duration::from_secs(1))?;
    assert_shell_gone(u32::try_from(
        resumed_run.pid.context("resumed CLI process id")?,
    )?)?;
    upstream_request(&upstream, "b", json!({"model": "permission-sonnet"}))?;
    if web_search {
        search_request(&upstream, &restarted, "a", "permission-search")?;
        search_request(&upstream, &forked, "b", "forked-search")?;
    }
    post_claude_identity(&restarted.settings, PARENT_ID, "plan", RESTORED_MODEL)?;
    let result = permission_mode_result(&mut runtime, "plan")?;
    assert!(result.get("error").is_none(), "{result}");
    let confirmed = claude_binding(&db, &parent_session)?.context("permission restart binding")?;
    assert_eq!(confirmed.id, binding.id);
    assert_eq!(confirmed.provider_session_id, PARENT_ID);
    assert_eq!(confirmed.metadata["model"], RESTORED_MODEL);
    assert_eq!(confirmed.metadata["permissionMode"], "plan");
    println!(
        "Permission restart: the previous CLI exited and its listener refused connections by the new launch; the new run kept the original identity"
    );

    runtime.send(Command::Shutdown)?;
    runtime.closed()?;
    assert!(runtime.wait()?.0.success());
    refused(&restarted);
    refused(&forked);
    Ok(())
}
