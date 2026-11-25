//! Exercise the public RuntimeHost channel against detached filesystem and CLI fixtures.
#![cfg(unix)]

use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpStream,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use matou_native_core::link_routing::BROWSER_RELEASES_URL;
use matou_native_runtime::{
    dsh_catalog::encode_project_path,
    egress_probe::{EgressSample, Family, HostPair, HostSample, RuleMode},
    host::{
        Command, Event, ForkPlacement, ForkWorktreeMode, LaunchOptions, OfflineFixtures,
        OfflineRetention, OfflineUpdate, RuntimeHost,
    },
    retention_policy::{IndexedCheckpoint, SessionRetention},
    update_download::{Manifest, ManifestFile},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TIMEOUT: Duration = Duration::from_secs(15);
const PARENT: &str = "01a1ddc1-88fd-7fc1-8c66-ab8d38719da8";
const CHILD: &str = "01a1ddc1-e832-7cf2-83b7-5adb2f12224b";

fn options(root: &Path) -> Result<LaunchOptions> {
    let workspace = root.join("workspace");
    let shell_home = root.join("home");
    for path in [&workspace, &shell_home] {
        fs::create_dir_all(path)?;
    }
    fs::write(
        shell_home.join(".zshrc"),
        "PROMPT='FIXTURE> '
",
    )?;
    Ok(LaunchOptions {
        data_root: root.join("data"),
        workspace: workspace.canonicalize()?,
        shell_home,
        shell: "/bin/zsh".into(),
    })
}

fn provider_environment(options: &LaunchOptions) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), options.shell_home.clone().into_os_string()),
        ("MATOU_LOCALE".into(), "en".into()),
    ])
}

fn executable(path: &Path, script: &str) -> Result<()> {
    fs::write(path, script)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn send(host: &RuntimeHost, command: Command) -> Result<()> {
    host.commands
        .send(command)
        .map_err(|_| anyhow::anyhow!("Runtime command channel closed"))
}

fn wait<T>(mut poll: impl FnMut() -> Result<Option<T>>) -> Result<T> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(result) = poll()? {
            return Ok(result);
        }
        ensure!(
            Instant::now() < deadline,
            "Timed out waiting for the Runtime fixture"
        );
        thread::sleep(Duration::from_millis(8));
    }
}

fn next<T>(host: &RuntimeHost, mut accept: impl FnMut(Event) -> Option<T>) -> Result<T> {
    wait(|| {
        let Ok(event) = host.events.try_recv() else {
            return Ok(None);
        };
        if let Event::Frame {
            session_id,
            sequence,
            ..
        } = &event
        {
            send(
                host,
                Command::Applied {
                    session_id: session_id.clone(),
                    sequence: *sequence,
                },
            )?;
        }
        if let Event::Diagnostic(message) = &event {
            eprintln!("Runtime fixture diagnostic: {message}");
        }
        if let Event::Closed = event {
            bail!("Runtime closed while the fixture was waiting");
        }
        if let Event::NavigationFailed { message, .. } = &event {
            bail!("Runtime navigation failed: {message}");
        }
        Ok(accept(event))
    })
}

fn ready(host: &RuntimeHost) -> Result<(String, String)> {
    next(host, |event| match event {
        Event::Ready { graph, .. } => Some((
            graph["sceneId"].as_str()?.to_owned(),
            graph["focusedSessionId"].as_str()?.to_owned(),
        )),
        _ => None,
    })
}

fn input(host: &RuntimeHost, session_id: &str, bytes: &[u8]) -> Result<()> {
    send(
        host,
        Command::Input {
            session_id: session_id.to_owned(),
            bytes: bytes.to_vec(),
            defer_ordering: false,
        },
    )
}

fn reply(host: &RuntimeHost, command: Command, id: u64, kind: &str) -> Result<Value> {
    send(host, command)?;
    next(host, |event| match event {
        Event::Graph(graph) if graph["type"] == kind && graph["requestId"] == id => Some(graph),
        _ => None,
    })
    .with_context(|| format!("waiting for {kind} request {id}"))
}

fn result(host: &RuntimeHost, command: Command, id: u64, kind: &str) -> Result<Value> {
    let reply = reply(host, command, id, kind)?;
    ensure!(
        reply.get("error").is_none() && reply.get("result").is_some(),
        "{reply}"
    );
    Ok(reply["result"].clone())
}

fn graph(host: &RuntimeHost, mut accepts: impl FnMut(&Value) -> bool) -> Result<Value> {
    next(host, |event| match event {
        Event::Graph(graph) | Event::Navigation { graph, .. } if accepts(&graph) => Some(graph),
        _ => None,
    })
}

fn node<'a>(graph: &'a Value, id: &str) -> Option<&'a Value> {
    graph["nodes"]
        .as_array()?
        .iter()
        .find(|node| node["sessionId"] == id)
}

fn identity(host: &RuntimeHost, id: &str, provider_id: &str) -> Result<Value> {
    let found = graph(host, |graph| {
        graph["providerIdentityEvents"]
            .as_array()
            .is_some_and(|events| {
                events.iter().any(|event| {
                    event["sessionId"] == id
                        && ((event["result"] == "confirmed"
                            && event["providerSessionId"] == provider_id)
                            || event["result"] == "restore-failed"
                            || event["result"] == "fork-failed")
                })
            })
    })
    .with_context(|| format!("waiting for identity {provider_id} on {id}"))?;
    ensure!(
        found["providerIdentityEvents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["result"] == "confirmed"
                && event["sessionId"] == id
                && event["providerSessionId"] == provider_id),
        "Runtime declined identity {provider_id}: {}",
        found["providerIdentityEvents"]
    );
    Ok(found)
}

fn post(url: &str, payload: &Value) -> Result<()> {
    let url = url::Url::parse(url)?;
    ensure!(
        url.host_str() == Some("127.0.0.1"),
        "Only the fixture loopback endpoint is allowed"
    );
    let mut socket = TcpStream::connect(("127.0.0.1", url.port().context("hook port")?))?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    let target = url.query().map_or_else(
        || url.path().to_owned(),
        |query| format!("{}?{query}", url.path()),
    );
    let payload = serde_json::to_vec(payload)?;
    write!(
        socket,
        "POST {} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        target,
        payload.len()
    )?;
    socket.write_all(&payload)?;
    let mut response = String::new();
    socket.read_to_string(&mut response)?;
    ensure!(
        response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.1 204"),
        "{response}"
    );
    Ok(())
}

fn marker(path: &Path, condition: impl Fn(&str) -> bool) -> Result<String> {
    wait(|| {
        Ok(fs::read_to_string(path)
            .ok()
            .filter(|value| condition(value)))
    })
    .with_context(|| format!("waiting for fixture marker {}", path.display()))
}

#[test]
fn account_summary_projects_frozen_subscription_files_without_leaking_secrets() -> Result<()> {
    let root = tempfile::tempdir()?;
    let options = options(root.path())?;
    let directory = options.data_root.join("model-router/credentials");
    fs::create_dir_all(&directory)?;
    let access = "fixture-subscription-access-canary";
    let refresh = "fixture-subscription-refresh-canary";
    let identity = "fixture-subscription-identity-canary";
    let api_key = "fixture-unrelated-api-key-canary";
    let auth_index = "fixture-private-auth-index-canary";
    let active_file = "fixture-private-codex-ref.json";
    let expired_file = "fixture-private-antigravity-ref.json";
    let orphan_file = "fixture-unassigned-codex-ref.json";
    let router_key = "fixture-router-key-canary";
    let config = json!({
        "version": 1,
        "revision": 5,
        "activeProviderIds": {"claude-code": "codex-subscription", "codex": "openai-official"},
        "providers": {
            "claude-code": [
                {"id": "api-provider", "kind": "api-key", "apiKey": api_key},
                {
                    "id": "codex-subscription", "kind": "subscription", "name": access,
                    "subscription": {
                        "provider": "codex",
                        "credentialRef": {"name": active_file, "authIndex": auth_index},
                        "account": {"status": "ready", "email": "member@example.test", "plan": "Plus"},
                        "modelMetadata": []
                    }
                },
                {
                    "id": "google-subscription", "kind": "subscription",
                    "subscription": {
                        "provider": "antigravity",
                        "credentialRef": {"name": expired_file},
                        "account": {"status": "ready", "plan": api_key, "reason": identity},
                        "modelMetadata": []
                    }
                }
            ],
            "codex": []
        }
    });
    let active = json!({
        "type": "codex", "email": "member@example.test", "auth_index": auth_index,
        "access_token": access, "refresh_token": refresh, "id_token": identity,
        "expired": "2099-01-01T00:00:00Z"
    });
    let expired = json!({
        "type": "antigravity", "email": format!("fixture@{refresh}.example.test"),
        "access_token": access, "expired": "2000-01-01T00:00:00Z"
    });
    let unassigned = json!({
        "type": "codex", "email": "unassigned@example.test", "plan_type": "Free",
        "access_token": access, "expired": "2099-01-01T00:00:00Z"
    });
    let files = [
        (options.data_root.join("provider-config.json"), config),
        (
            options.data_root.join("model-router/config.json"),
            json!({
                "config-version": 8, "access": {"api-keys": [router_key]},
                "oauth": {"auth-dir": directory}
            }),
        ),
        (directory.join(active_file), active),
        (directory.join(expired_file), expired),
        (directory.join(orphan_file), unassigned),
    ];
    for (path, contents) in &files {
        fs::write(path, contents.to_string())?;
    }
    let legacy_path = options.data_root.join("accounts.json");
    assert!(!legacy_path.exists());

    let mut host = RuntimeHost::start_with_offline_fixtures(options, OfflineFixtures::default())?;
    ready(&host)?;
    let response = reply(
        &host,
        Command::GetAccountSummary { request_id: 1 },
        1,
        "accountSummaryResult",
    )?;
    let wire = response.to_string();
    for forbidden in [
        access,
        refresh,
        identity,
        api_key,
        auth_index,
        active_file,
        expired_file,
        orphan_file,
        router_key,
        "access_token",
        "refresh_token",
        "id_token",
        "apiKey",
        "credentialRef",
    ] {
        assert!(
            !wire.contains(forbidden),
            "account summary exposed a private field or value"
        );
    }
    ensure!(response.get("error").is_none(), "summary request failed");
    assert_eq!(response["result"]["loadState"], "loaded");
    let summary = &response["result"]["accountSummary"];
    assert_eq!(summary["currentId"], "codex-subscription");
    let accounts = summary["accounts"].as_array().context("summary accounts")?;
    assert_eq!(accounts.len(), 3);
    assert_eq!(accounts[0]["provider"], "codex");
    assert_eq!(accounts[0]["status"], "active");
    assert_eq!(accounts[0]["plan"], "Plus");
    assert_eq!(accounts[0]["email"], "me•••@example.test");
    assert_eq!(accounts[1]["provider"], "antigravity");
    assert_eq!(accounts[1]["status"], "expired");
    assert_eq!(accounts[1]["plan"], "[redacted]");
    assert_eq!(accounts[1]["email"], "•••");
    assert_eq!(accounts[2]["provider"], "codex");
    assert_eq!(accounts[2]["status"], "needs-activation");
    assert_eq!(accounts[2]["plan"], "Free");
    assert!(!legacy_path.exists());
    for (path, contents) in files {
        assert_eq!(fs::read_to_string(path)?, contents.to_string());
    }
    host.shutdown()
}

#[test]
fn offline_queries_update_retention_and_storage_gate() -> Result<()> {
    let root = tempfile::tempdir()?;
    let options = options(root.path())?;
    fs::create_dir(&options.data_root)?;
    let canary = "wave2b-private-token-canary";
    fs::write(options.data_root.join("accounts.json"), json!({
        "currentId": "account-1",
        "accounts": [{
            "id": "account-1", "name": format!("name with {canary}"), "email": "private@example.test",
            "plan": "pro", "claudeAiOauth": {"accessToken": canary, "refreshToken": "another-secret-canary"}
        }],
    }).to_string())?;
    let browser = root.path().join("Browser.app/Contents/MacOS/browser");
    fs::create_dir_all(browser.parent().unwrap())?;
    executable(&browser, "#!/bin/sh\nexit 99\n")?;
    fs::write(
        browser
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>7.2.1</string></dict></plist>"#,
    )?;
    let git_root = options.workspace.clone();
    init_git(&git_root)?;
    let retention = tempfile::tempdir()?;
    let retention_root = retention.path().to_owned();
    let journal = retention_root.join("journal/inactive");
    let checkpoints = retention_root.join("checkpoints/inactive");
    fs::create_dir_all(&journal)?;
    fs::create_dir_all(&checkpoints)?;
    let raw = journal.join("segment-000001.bin");
    let compressed = journal.join("segment-000002.bin.gz");
    fs::File::create(&raw)?.set_len(2 * 1024 * 1024 * 1024)?;
    fs::write(&compressed, b"compressed-history")?;
    for generation in 1..=3 {
        fs::write(
            checkpoints.join(format!("checkpoint-{generation:06}.bin")),
            b"checkpoint",
        )?;
    }
    let installer = if cfg!(windows) {
        "fixture.exe"
    } else {
        "fixture.dmg"
    };
    let artifact = b"offline installer fixture".to_vec();
    let manifest = Manifest {
        version: "99.0.1".into(),
        files: vec![ManifestFile {
            url: Some(installer.into()),
            size: Some(artifact.len() as u64),
            sha256: Some(format!("{:x}", Sha256::digest(&artifact))),
            sha512: None,
        }],
        ..Default::default()
    };
    let mut host = RuntimeHost::start_with_offline_fixtures(
        options.clone(),
        OfflineFixtures {
            browser_executable: Some(browser),
            update: Some(OfflineUpdate {
                manifest,
                artifact: artifact.clone(),
            }),
            retention: Some(OfflineRetention {
                directory: retention,
                sessions: vec![(
                    "inactive".into(),
                    SessionRetention::Inactive,
                    (1..=3)
                        .map(|generation| IndexedCheckpoint {
                            generation,
                            created_at_ms: generation as i64,
                        })
                        .collect(),
                )],
            }),
            ..Default::default()
        },
    )?;
    ready(&host)?;
    let repo = result(
        &host,
        Command::CheckGitRepo {
            request_id: 1,
            cwd: git_root.clone(),
        },
        1,
        "gitRepoResult",
    )?;
    assert_eq!(repo["root"], json!(git_root));
    let environment = result(
        &host,
        Command::GetEnvironmentDiagnostics {
            request_id: 2,
            cwd: git_root,
            repair: None,
        },
        2,
        "environmentDiagnosticsResult",
    )?;
    assert_eq!(environment["state"], "ready");
    assert_eq!(environment["git"]["head"]["branch"], "main");
    let browser = result(
        &host,
        Command::GetBrowserDiagnostics { request_id: 3 },
        3,
        "browserDiagnosticsResult",
    )?;
    assert_eq!(browser["status"]["installed"], true);
    #[cfg(target_os = "macos")]
    assert_eq!(browser["status"]["version"], "7.2.1");
    for (request_id, url, allowed) in [
        (4, BROWSER_RELEASES_URL, true),
        (5, "file:///tmp/no", false),
    ] {
        let link = result(
            &host,
            Command::EvaluateExternalLink {
                request_id,
                url: Some(url.into()),
                sign_in: false,
            },
            request_id,
            "externalLinkResult",
        )?;
        assert_eq!(link["allowed"], allowed);
    }
    let response = reply(
        &host,
        Command::GetAccountSummary { request_id: 6 },
        6,
        "accountSummaryResult",
    )?;
    assert_eq!(response["sceneId"].as_str().is_some(), true);
    let summary = &response["result"];
    assert_eq!(summary["loadState"], "loaded");
    assert!(summary["subscriptionSupported"].is_boolean());
    let account = &summary["accountSummary"]["accounts"][0];
    assert_eq!(account["name"], "[redacted]");
    assert!(!account["email"].as_str().unwrap().contains("private@"));
    let wire = response.to_string();
    assert!(
        !wire.contains(canary)
            && !wire.contains("another-secret-canary")
            && !wire.contains("accessToken")
    );
    for (command, id, kind, status) in [
        (
            Command::GetUpdateStatus { request_id: 7 },
            7,
            "updateStatusResult",
            "idle",
        ),
        (
            Command::CheckForUpdate { request_id: 8 },
            8,
            "updateCheckResult",
            "available",
        ),
        (
            Command::DownloadUpdate { request_id: 9 },
            9,
            "updateDownloadResult",
            "downloaded",
        ),
        (
            Command::GetUpdateStatus { request_id: 10 },
            10,
            "updateStatusResult",
            "downloaded",
        ),
    ] {
        assert_eq!(result(&host, command, id, kind)?["status"], status);
    }
    assert_eq!(
        fs::read(options.data_root.join("updates").join(installer))?,
        artifact
    );
    let report = result(
        &host,
        Command::RunRetentionCleanup { request_id: 11 },
        11,
        "retentionCleanupResult",
    )?;
    assert_eq!(report["globalQuotaBytes"], 2_u64 * 1024 * 1024 * 1024);
    assert_eq!(report["sessionQuotaBytes"], 256_u64 * 1024 * 1024);
    assert_eq!(report["actions"].as_array().unwrap().len(), 1);
    assert_eq!(report["actions"][0]["generation"], 1);
    assert!(report["remainingGlobalOverage"].as_u64().unwrap() > 0);
    assert!(!checkpoints.join("checkpoint-000001.bin").exists());
    assert!(
        checkpoints.join("checkpoint-000002.bin").exists()
            && checkpoints.join("checkpoint-000003.bin").exists()
    );
    assert_eq!(fs::metadata(raw)?.len(), 2 * 1024 * 1024 * 1024);
    assert_eq!(fs::read(compressed)?, b"compressed-history");
    host.shutdown()?;

    let database = rusqlite::Connection::open(options.data_root.join("matou.sqlite"))?;
    database.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE);
         INSERT INTO schema_migrations (version, name, checksum, applied_at)
         VALUES (9999, 'fixture-newer', 'fixture-newer', 5);",
    )?;
    drop(database);
    let mut readonly =
        RuntimeHost::start_with_offline_fixtures(options, OfflineFixtures::default())?;
    next(&readonly, |event| {
        matches!(event, Event::ReadOnlyReady { .. }).then_some(())
    })?;
    assert_eq!(
        result(
            &readonly,
            Command::GetEgressStatus { request_id: 12 },
            12,
            "egressStatusResult"
        )?["state"],
        "checking"
    );
    for (request_id, command) in [
        (13, Command::RunRetentionCleanup { request_id: 13 }),
        (14, Command::RecheckEgress { request_id: 14 }),
        (
            15,
            Command::SetEgressExemption {
                request_id: 15,
                family: Family::OpenAi,
            },
        ),
        (16, Command::DownloadUpdate { request_id: 16 }),
        (
            17,
            Command::LoadDshSession {
                request_id: 17,
                session_id: "any".into(),
                provider_session_id: "any".into(),
            },
        ),
        (
            18,
            Command::SetEgressRuleMode {
                request_id: 18,
                mode: RuleMode::Ip,
            },
        ),
        (
            19,
            Command::SetEgressProtectionEnabled {
                request_id: 19,
                enabled: false,
            },
        ),
        (
            20,
            Command::AcceptEgressBaseline {
                request_id: 20,
                family: Family::OpenAi,
            },
        ),
    ] {
        send(&readonly, command)?;
        let code = next(&readonly, |event| match event {
            Event::Graph(graph) if graph["requestId"] == request_id => {
                graph["code"].as_str().map(str::to_owned)
            }
            _ => None,
        })?;
        assert_eq!(code, "STORAGE_READ_ONLY");
    }
    readonly.shutdown()
}

#[test]
fn dsh_shell_promotion_and_saved_restore_use_authenticated_observations() -> Result<()> {
    let root = tempfile::tempdir()?;
    let options = options(root.path())?;
    let history = root.path().join("dsh-history");
    let session_log = history
        .join(encode_project_path(options.workspace.to_str().unwrap()))
        .join("dsh-saved");
    fs::create_dir_all(&session_log)?;
    fs::write(
        session_log.join("session.v4.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"type":"session","version":4,"id":"dsh-saved","cwd":options.workspace,"createdAt":1000}),
            json!({"type":"user/message","seq":1,"surfaceOp":"append","data":{"content":[{"text":"needle conversation"}]}})
        ),
    )?;
    let stub = root.path().join("dsh-fixture");
    executable(
        &stub,
        r#"#!/bin/sh
set -eu
if [ "$2" = --dump-config ]; then
  printf '%s\n' '- id: dsh-tui' '  config:' '    backend: dsh'
  exit 0
fi
test "$1" = dsh-tui && test "$2" = --patch
printf '%s\n' "$@" > "$HOME/dsh-argv"
/bin/cat "$3" > "$HOME/dsh-patch"
printf 'DSH FIXTURE READY\n'
exec /bin/cat
"#,
    )?;
    let mut env = provider_environment(&options);
    env.insert("MATOU_DSH_COMMAND".into(), stub.into_os_string());
    let mut host = RuntimeHost::start_with_offline_fixtures(
        options.clone(),
        OfflineFixtures {
            dsh_session_root: Some(history),
            provider_environment: Some(env),
            ..Default::default()
        },
    )?;
    let (_, id) = ready(&host)?;
    next(&host, |event| {
        matches!(event, Event::Frame { .. }).then_some(())
    })?;
    let cwd = options.workspace.to_string_lossy().into_owned();
    let list = result(
        &host,
        Command::ListDshSessions {
            request_id: 1,
            cwd: cwd.clone(),
            query: "needle".into(),
            full_text: true,
            offset: None,
            limit: None,
        },
        1,
        "dshSessionListResult",
    )?;
    assert_eq!(list["sessions"][0]["providerSessionId"], "dsh-saved");
    let detail = result(
        &host,
        Command::GetDshSession {
            request_id: 2,
            cwd: cwd.clone(),
            provider_session_id: "dsh-saved".into(),
            before_event_index: None,
            around_event_index: None,
            limit: None,
        },
        2,
        "dshSessionDetailResult",
    )?;
    assert_eq!(detail["events"][0]["text"], "needle conversation");
    let search = result(
        &host,
        Command::SearchDshSession {
            request_id: 3,
            cwd,
            provider_session_id: "dsh-saved".into(),
            query: "needle".into(),
            offset: None,
            limit: None,
        },
        3,
        "dshSessionSearchResult",
    )?;
    assert_eq!(search["hits"].as_array().unwrap().len(), 1);
    input(&host, &id, b"dsh --profile dsh-tui\r")?;
    let patch_path = options.shell_home.join("dsh-patch");
    let patch = marker(&patch_path, |value| value.contains("runId:"))?;
    let (url, run) = dsh_registration(&patch)?;
    post(&url, &dsh_ready(&run, &options.workspace, "dsh-fresh"))?;
    let fresh = identity(&host, &id, "dsh-fresh")?;
    assert_eq!(
        node(&fresh, &id).context("DSH card")?["historyProviderSessionId"],
        "dsh-fresh"
    );
    let loaded = result(
        &host,
        Command::LoadDshSession {
            request_id: 4,
            session_id: id.clone(),
            provider_session_id: "dsh-saved".into(),
        },
        4,
        "dshSessionLoadResult",
    )?;
    assert_eq!(loaded["status"], "queued");
    let args = marker(&options.shell_home.join("dsh-argv"), |value| {
        value.contains("--resume\ndsh-saved")
    })?;
    assert!(args.starts_with("dsh-tui\n--patch\n"));
    let patch = marker(&patch_path, |value| {
        value.contains("runId:") && !value.contains(&run)
    })?;
    let (url, run) = dsh_registration(&patch)?;
    post(&url, &dsh_ready(&run, &options.workspace, "dsh-saved"))?;
    let resumed = identity(&host, &id, "dsh-saved")?;
    assert_eq!(
        node(&resumed, &id).context("restored DSH card")?["historyProviderSessionId"],
        "dsh-saved"
    );
    let hud = result(
        &host,
        Command::GetAgentHud {
            request_id: 5,
            session_id: id,
        },
        5,
        "agentHudResult",
    )?;
    assert_eq!(hud["hud"]["provider"], "dsh");
    assert_eq!(hud["hud"]["contextPercent"].as_f64(), Some(18.0));
    assert_eq!(hud["hud"]["runningTools"][0]["name"], "Read");
    host.shutdown()
}

fn dsh_registration(patch: &str) -> Result<(String, String)> {
    let key = |name: &str| -> Result<String> {
        let value = patch
            .lines()
            .find_map(|line| line.trim().strip_prefix(name))
            .context("DSH patch field")?;
        Ok(serde_json::from_str(value)?)
    };
    Ok((key("hookUrl: ")?, key("runId: ")?))
}

fn dsh_ready(run: &str, cwd: &Path, provider_id: &str) -> Value {
    json!({"dsh_observation":1,"run_id":run,"sequence":1,"generation":1,"session_id":provider_id,
        "foreground":true,"event_type":"ready","ready":true,"persisted":true,"cwd":cwd,"agent_status":"idle",
        "hud":{"title":"fixture DSH","contextPercent":18,"contextWindowSize":1000,"runningTools":[{"name":"Read"}]}})
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = process::Command::new("/usr/bin/git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "Git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

fn init_git(root: &Path) -> Result<()> {
    git(root, &["init", "-b", "main"])?;
    git(
        root,
        &[
            "-c",
            "user.name=Runtime Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--allow-empty",
            "-m",
            "fixture baseline",
        ],
    )?;
    Ok(())
}

struct CodexFixture {
    options: LaunchOptions,
    home: PathBuf,
    environment: BTreeMap<OsString, OsString>,
}

impl CodexFixture {
    fn new(root: &Path) -> Result<Self> {
        let options = options(root)?;
        let home = root.join("codex-home");
        fs::create_dir_all(home.join("sessions"))?;
        let stub = root.join("codex-fixture");
        executable(
            &stub,
            r#"#!/bin/sh
set -eu
name=source
for word in "$@"; do
    if [ "$word" = fork ]; then name=fork; fi
done
printf '%s\n' "$@" > "$CODEX_HOME/$name-argv"
printf '%s\n' "$PWD" > "$CODEX_HOME/$name-cwd"
printf 'CODEX FIXTURE READY\n'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$CODEX_HOME/$name-input"
done
"#,
        )?;
        let mut environment = provider_environment(&options);
        environment.insert("CODEX_HOME".into(), home.clone().into_os_string());
        environment.insert("MATOU_CODEX_COMMAND".into(), stub.into_os_string());
        Ok(Self {
            options,
            home,
            environment,
        })
    }

    fn start(&self, fixtures: OfflineFixtures) -> Result<RuntimeHost> {
        RuntimeHost::start_with_offline_fixtures(
            self.options.clone(),
            OfflineFixtures {
                provider_environment: Some(self.environment.clone()),
                ..fixtures
            },
        )
    }

    fn rollout(&self, id: &str, cwd: &Path) -> Result<PathBuf> {
        let path = self
            .home
            .join("sessions")
            .join(format!("rollout-fixture-{id}.jsonl"));
        fs::write(
            &path,
            format!(
                "{}\n",
                json!({"type":"session_meta", "payload":{
                    "id":id,"cwd":cwd,"timestamp":"2026-01-01T12:00:00.000Z"
                }})
            ),
        )?;
        Ok(path)
    }

    fn hook(&self, name: &str) -> Result<String> {
        let args = marker(&self.home.join(format!("{name}-argv")), |args| {
            args.contains("hooks={ ")
        })?;
        let pattern = regex::Regex::new(r#"command = ("(?:\\.|[^"\\])*")"#)?;
        let entry = &pattern.captures(&args).context("Codex hook command")?[1];
        let command: String = serde_json::from_str(entry)?;
        let quoted = command.trim_matches('\'');
        let script = fs::read_to_string(quoted)?;
        let url = regex::Regex::new(r"http://127\.0\.0\.1:[0-9]+/hooks/[A-Za-z0-9_-]+")?;
        Ok(url
            .find(&script)
            .context("Codex hook URL")?
            .as_str()
            .to_owned())
    }

    fn source(&self, host: &RuntimeHost) -> Result<(String, String, String, PathBuf)> {
        let (scene, id) = ready(host)?;
        next(host, |event| {
            matches!(event, Event::Frame { .. }).then_some(())
        })?;
        input(host, &id, b"codex\r")?;
        let hook = self.hook("source")?;
        let rollout = self.rollout(PARENT, &self.options.workspace)?;
        post(
            &hook,
            &codex_hook("SessionStart", PARENT, &self.options.workspace, &rollout),
        )?;
        identity(host, &id, PARENT)?;
        Ok((scene, id, hook, rollout))
    }
}

fn codex_hook(event: &str, id: &str, cwd: &Path, rollout: &Path) -> Value {
    json!({"hook_event_name":event,"session_id":id,"cwd":cwd,"transcript_path":rollout,"turn_id":"fixture-turn"})
}

fn egress(ip: &str, country: &str) -> EgressSample {
    let family = Family::OpenAi;
    let (api, login) = family.hosts();
    let host = |name| HostSample {
        ip: Some(ip.into()),
        country: Some(country.into()),
        ..HostSample::new(name)
    };
    EgressSample::new(BTreeMap::from([(
        family,
        HostPair::new(family, host(api), host(login)),
    )]))
}

#[test]
fn egress_freezes_the_live_pty_and_exemption_is_scoped_to_the_exit_ip() -> Result<()> {
    let root = tempfile::tempdir()?;
    let fixture = CodexFixture::new(root.path())?;
    let mut samples = VecDeque::from([
        egress("8.8.8.8", "US"),
        egress("1.2.3.4", "CN"),
        egress("1.2.3.5", "CN"),
    ]);
    let mut host = fixture.start(OfflineFixtures {
        egress_sample: Some(Box::new(move || {
            samples.pop_front().context("sample fixture exhausted")
        })),
        ..Default::default()
    })?;
    let (_, id, _, _) = fixture.source(&host)?;
    let recheck = |request_id| {
        result(
            &host,
            Command::RecheckEgress { request_id },
            request_id,
            "egressRecheckResult",
        )
    };
    assert_eq!(recheck(1)?["sessionFamilies"][&id][0], "openai");
    let status = recheck(2)?;
    assert_eq!(status["state"], "abnormal");
    assert_eq!(status["frozenSessionIds"], json!([id]));
    input(&host, &id, b"blocked fixture input\r")?;
    next(&host, |event| match event {
        Event::Diagnostic(message) if message.contains("frozen") => Some(()),
        _ => None,
    })?;
    let rejected = reply(
        &host,
        Command::AcceptEgressBaseline {
            request_id: 5,
            family: Family::OpenAi,
        },
        5,
        "egressBaselineResult",
    )?;
    assert!(
        rejected["error"]
            .as_str()
            .unwrap()
            .contains("mainland check")
    );
    let thawed = result(
        &host,
        Command::SetEgressExemption {
            request_id: 3,
            family: Family::OpenAi,
        },
        3,
        "egressExemptionResult",
    )?;
    assert_eq!(thawed["frozenSessionIds"], json!([]));
    assert_eq!(thawed["families"][0]["overridden"], true);
    input(&host, &id, b"allowed fixture input\r")?;
    let actual = marker(&fixture.home.join("source-input"), |value| {
        value.contains("allowed fixture input")
    })?;
    assert!(!actual.contains("blocked"));
    assert_eq!(recheck(4)?["frozenSessionIds"], json!([id]));
    host.shutdown()
}

#[test]
fn egress_rule_protection_and_baseline_apply_to_the_live_pty() -> Result<()> {
    let root = tempfile::tempdir()?;
    let fixture = CodexFixture::new(root.path())?;
    let mut samples = VecDeque::from([egress("8.8.8.8", "US"), egress("8.8.4.4", "US")]);
    let mut host = fixture.start(OfflineFixtures {
        egress_sample: Some(Box::new(move || {
            samples.pop_front().context("sample fixture exhausted")
        })),
        ..Default::default()
    })?;
    let (_, id, _, _) = fixture.source(&host)?;
    for request_id in 1..=2 {
        let status = result(
            &host,
            Command::RecheckEgress { request_id },
            request_id,
            "egressRecheckResult",
        )?;
        assert_eq!(status["state"], "ok");
        assert_eq!(status["families"][0]["baseline"]["ip"], "8.8.8.8");
    }

    let set_rule = |request_id, mode| {
        result(
            &host,
            Command::SetEgressRuleMode { request_id, mode },
            request_id,
            "egressRuleModeResult",
        )
    };
    let set_protection = |request_id, enabled| {
        result(
            &host,
            Command::SetEgressProtectionEnabled {
                request_id,
                enabled,
            },
            request_id,
            "egressProtectionEnabledResult",
        )
    };
    let assert_blocked = |text: &[u8]| -> Result<()> {
        input(&host, &id, text)?;
        next(&host, |event| match event {
            Event::Diagnostic(message) if message.contains("frozen") => Some(()),
            _ => None,
        })
    };
    let assert_delivered = |text: &str| -> Result<()> {
        input(&host, &id, format!("{text}\r").as_bytes())?;
        let actual = marker(&fixture.home.join("source-input"), |value| {
            value.contains(text)
        })?;
        assert!(!actual.contains("blocked"));
        Ok(())
    };

    let strict = set_rule(3, RuleMode::Ip)?;
    assert_eq!(strict["ruleMode"], "ip");
    assert_eq!(strict["frozenSessionIds"], json!([id]));
    assert_blocked(b"blocked by stricter rule\r")?;
    assert_eq!(
        set_rule(4, RuleMode::Country)?["frozenSessionIds"],
        json!([])
    );
    assert_delivered("resumed by country rule")?;
    assert_eq!(set_rule(5, RuleMode::Ip)?["frozenSessionIds"], json!([id]));

    let disabled = set_protection(6, false)?;
    assert_eq!(disabled["protectionEnabled"], false);
    assert_eq!(disabled["frozenSessionIds"], json!([]));
    assert_delivered("resumed with protection off")?;
    let enabled = set_protection(7, true)?;
    assert_eq!(enabled["protectionEnabled"], true);
    assert_eq!(enabled["frozenSessionIds"], json!([id]));
    assert_blocked(b"blocked by re-enabled protection\r")?;

    let accepted = result(
        &host,
        Command::AcceptEgressBaseline {
            request_id: 8,
            family: Family::OpenAi,
        },
        8,
        "egressBaselineResult",
    )?;
    assert_eq!(accepted["families"][0]["baseline"]["ip"], "8.8.4.4");
    assert_eq!(accepted["frozenSessionIds"], json!([]));
    assert_delivered("resumed by baseline update")?;
    host.shutdown()
}

fn fork(
    host: &RuntimeHost,
    scene: &str,
    source: &str,
    mode: ForkWorktreeMode,
    submission: &str,
) -> Result<String> {
    send(
        host,
        Command::ForkSession {
            scene_id: scene.to_owned(),
            source_session_id: source.to_owned(),
            placement: ForkPlacement::Child,
            name: submission.to_owned(),
            worktree_mode: mode,
            submission_key: submission.to_owned(),
        },
    )?;
    let graph = graph(host, |graph| {
        graph["forkSessionResult"]["submissionKey"] == submission
    })
    .with_context(|| format!("fork submission {submission}"))?;
    Ok(graph["forkSessionResult"]["sessionId"]
        .as_str()
        .context("fork child")?
        .to_owned())
}

fn stage(host: &RuntimeHost, id: &str, stage: &str) -> Result<Value> {
    let graph = graph(host, |graph| {
        node(graph, id).is_some_and(|node| node["forkProgress"]["stage"] == stage)
    })
    .with_context(|| format!("waiting for fork {id} to reach {stage}"))?;
    Ok(node(&graph, id).unwrap().clone())
}

fn fork_started(host: &RuntimeHost, id: &str) -> Result<()> {
    let reply = graph(host, |graph| graph["forkSessionResult"]["sessionId"] == id)?;
    ensure!(
        reply["forkSessionResult"]["error"].is_null(),
        "{}",
        reply["forkSessionResult"]
    );
    let progressed = graph(host, |graph| {
        node(graph, id).is_some_and(|node| {
            matches!(
                node["forkProgress"]["stage"].as_str(),
                Some("starting-window" | "failed")
            )
        })
    })?;
    let progress = &node(&progressed, id).unwrap()["forkProgress"];
    ensure!(progress["stage"] == "starting-window", "{progress}");
    Ok(())
}

#[test]
fn codex_fork_leases_roll_back_unbound_checkout_and_retry_bound_checkout_after_restart()
-> Result<()> {
    let root = tempfile::tempdir()?;
    let fixture = CodexFixture::new(root.path())?;
    init_git(&fixture.options.workspace)?;
    let isolation = root.path().canonicalize()?;
    let initial_reject = Arc::new(AtomicBool::new(true));
    let reject = Arc::clone(&initial_reject);
    let mut host = fixture.start(OfflineFixtures {
        fork_worktree_root: Some(isolation.clone()),
        fork_setup: Some(Box::new(move |_| {
            ensure!(
                !reject.swap(false, Ordering::AcqRel),
                "fixture setup rejected"
            );
            Ok(())
        })),
        ..Default::default()
    })?;
    let (scene, source, source_hook, parent) = fixture.source(&host)?;
    for name in ["UserPromptSubmit", "Stop"] {
        post(
            &source_hook,
            &codex_hook(name, PARENT, &fixture.options.workspace, &parent),
        )?;
    }
    graph(&host, |graph| {
        node(graph, &source).is_some_and(|node| node["canFork"] == true)
    })
    .context("Codex source readiness")?;

    let current = fork(
        &host,
        &scene,
        &source,
        ForkWorktreeMode::Current,
        "current fork",
    )?;
    let hook = fixture.hook("fork")?;
    let child = fixture.rollout(CHILD, &fixture.options.workspace)?;
    post(
        &hook,
        &codex_hook("SessionStart", CHILD, &fixture.options.workspace, &child),
    )?;
    let confirmed = identity(&host, &current, CHILD)?;
    assert_eq!(
        node(&confirmed, &current).unwrap()["forkProgress"]["stage"],
        "succeeded"
    );
    assert_eq!(
        fork(
            &host,
            &scene,
            &source,
            ForkWorktreeMode::Current,
            "current fork"
        )?,
        current
    );
    fs::remove_file(fixture.home.join("fork-argv"))?;

    let managed = fork(
        &host,
        &scene,
        &source,
        ForkWorktreeMode::New,
        "managed fork",
    )?;
    let failed = stage(&host, &managed, "failed")?;
    assert!(
        failed["forkProgress"]["error"]
            .as_str()
            .unwrap()
            .contains("fixture setup rejected")
    );
    let checkout = isolation.join(format!("fork-{managed}"));
    assert!(!checkout.exists());
    assert_eq!(
        git(
            &fixture.options.workspace,
            &["branch", "--format=%(refname:short)"]
        )?
        .trim(),
        "main"
    );

    send(
        &host,
        Command::RetryFork {
            session_id: managed.clone(),
        },
    )?;
    fork_started(&host, &managed).context("retry after rollback")?;
    let hook = fixture.hook("fork")?;
    assert_eq!(
        marker(&fixture.home.join("fork-cwd"), |value| value.trim()
            == checkout.to_str().unwrap())?
        .trim(),
        checkout.to_str().unwrap()
    );
    post(
        &hook,
        &codex_hook("SessionStart", PARENT, &checkout, &parent),
    )?;
    let failed = stage(&host, &managed, "failed")?;
    assert_eq!(failed["environment"]["kind"], "worktree");
    assert!(checkout.is_dir());
    let branch = git(&checkout, &["branch", "--show-current"])?
        .trim()
        .to_owned();
    host.shutdown()?;
    fs::remove_file(fixture.home.join("fork-argv"))?;

    let mut restarted = fixture.start(OfflineFixtures {
        fork_worktree_root: Some(isolation),
        ..Default::default()
    })?;
    ready(&restarted)?;
    send(
        &restarted,
        Command::RetryFork {
            session_id: managed.clone(),
        },
    )?;
    fork_started(&restarted, &managed).context("retry after runtime restart")?;
    let hook = fixture.hook("fork")?;
    let third = "01a1ddc1-e832-7cf2-83b7-5adb2f12224e";
    let rollout = fixture.rollout(third, &checkout)?;
    post(
        &hook,
        &codex_hook("SessionStart", third, &checkout, &rollout),
    )?;
    let confirmed = identity(&restarted, &managed, third)?;
    let child = node(&confirmed, &managed).unwrap();
    assert_eq!(child["forkProgress"]["stage"], "succeeded");
    assert_eq!(child["environment"]["path"], json!(checkout));
    assert_eq!(
        git(&checkout, &["branch", "--show-current"])?.trim(),
        branch
    );
    let database = matou_native_core::startup::open_read_only(
        &fixture.options.data_root.join("matou.sqlite"),
    )?;
    let (fence, lease_token, count): (i64, Option<String>, i64) = database.query_row(
        "SELECT lease_fence, lease_token, attempt FROM session_fork_intents WHERE session_id = ?",
        [&managed],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!((fence, lease_token, count), (3, None, 3));
    drop(database);
    let repo = result(
        &restarted,
        Command::CheckGitRepo {
            request_id: 21,
            cwd: checkout,
        },
        21,
        "gitRepoResult",
    )?;
    assert_eq!(repo["linkedWorktree"], true);
    restarted.shutdown()
}

#[test]
fn hud_projects_verified_claude_team_context_and_running_tools() -> Result<()> {
    let root = tempfile::tempdir()?;
    let options = options(root.path())?;
    git(&options.workspace, &["init", "-q"])?;
    fs::write(options.workspace.join("worktree-only.txt"), "fixture change")?;
    let claude = root.path().join("claude-fixture");
    let config = root.path().join("claude-config");
    let projects = config.join("projects/project");
    fs::create_dir_all(&projects)?;
    executable(
        &claude,
        r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  if [ "$1" = --settings ]; then
    printf '%s\n' "$2" > "$HOME/claude-settings"
    break
  fi
  shift
done
printf 'CLAUDE FIXTURE READY\n'
exec /bin/cat
"#,
    )?;
    let roll = projects.join(format!("{PARENT}.jsonl"));
    fs::write(
        &roll,
        format!(
            "{}\n{}\n",
            json!({"type":"user","toolUseResult":{"status":"teammate_spawned","teammate_id":"reviewer@team","name":"reviewer","team_name":"team","prompt":"Check the fixture"}}),
            json!({"type":"user","toolUseResult":{"listing":"Teammates (1):\n  reviewer [id]  ·  idle"}})
        ),
    )?;
    let mut environment = provider_environment(&options);
    environment.insert("MATOU_CLAUDE_COMMAND".into(), claude.into_os_string());
    environment.insert("CLAUDE_CONFIG_DIR".into(), config.into_os_string());
    let mut host = RuntimeHost::start_with_offline_fixtures(
        options.clone(),
        OfflineFixtures {
            provider_environment: Some(environment),
            ..Default::default()
        },
    )?;
    let (_, id) = ready(&host)?;
    next(&host, |event| {
        matches!(event, Event::Frame { .. }).then_some(())
    })?;
    input(&host, &id, b"claude\r")?;
    let settings = marker(&options.shell_home.join("claude-settings"), |path| {
        Path::new(path.trim()).is_file()
    })?;
    let settings: Value = serde_json::from_slice(&fs::read(settings.trim())?)?;
    let url = settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["url"]
        .as_str()
        .context("Claude fixture hook URL")?;
    post(
        url,
        &json!({"hook_event_name":"SessionStart","session_id":PARENT,"transcript_path":roll,"cwd":options.workspace}),
    )?;
    identity(&host, &id, PARENT)?;
    post(
        &format!("{url}?kind=statusline"),
        &json!({
            "session_id": PARENT,
            "context_window": {"used_percentage": 21, "context_window_size": 1000},
        }),
    )?;
    post(
        url,
        &json!({
            "hook_event_name":"PreToolUse","session_id":PARENT,"transcript_path":roll,
            "tool_name":"Read","tool_use_id":"fixture-tool",
        }),
    )?;
    // The command and hook are delivered on separate queues.
    let mut request_id = 30;
    let hud = wait(|| {
        request_id += 1;
        let hud = result(
            &host,
            Command::GetAgentHud {
                request_id,
                session_id: id.clone(),
            },
            request_id,
            "agentHudResult",
        )?;
        Ok((hud["hud"]["runningTools"][0]["name"] == "Read"
            && hud["hud"]["contextPercent"].as_f64() == Some(21.0))
        .then_some(hud))
    })?;
    assert_eq!(hud["hud"]["contextPercent"].as_f64(), Some(21.0));
    assert_eq!(hud["teammates"][0]["teammateId"], "reviewer@team");
    assert_eq!(hud["teammates"][0]["workStatus"], "idle");
    let guard = &hud["hud"]["contextGuard"];
    assert!(guard["cache"].is_object());
    assert_eq!(
        *guard,
        json!({
            "contextPressure": "normal",
            "savedNoiseBytes": 0,
            "cache": guard["cache"],
            "handoffReady": guard["handoffReady"]
        })
    );
    let details = result(
        &host,
        Command::GetContextGuardDetails {
            request_id: 70,
            session_id: id.clone(),
        },
        70,
        "contextGuardDetailsResult",
    )?;
    assert!(details["cache"].is_object());
    assert_eq!(
        details,
        json!({"sessionId": id, "cache": details["cache"], "logs": []})
    );
    let brief = result(
        &host,
        Command::GetSessionHandoffBrief {
            request_id: 71,
            session_id: id.clone(),
        },
        71,
        "sessionHandoffBriefResult",
    )?;
    assert!(brief["markdown"].as_str().is_some_and(|text| !text.is_empty()));
    assert_eq!(
        brief,
        json!({
            "sessionId": id,
            "markdown": brief["markdown"],
            "modifiedFiles": ["worktree-only.txt"],
            "logIds": [],
            "contextPercent": 21.0
        })
    );
    host.shutdown()
}

#[test]
fn claude_roster_uses_only_the_injected_engine_and_claude_preferences() -> Result<()> {
    use matou_native_runtime::claude_accounts::ClaudeAccountCommand as Action;

    let root = tempfile::tempdir()?;
    let options = options(root.path())?;
    let accounts = options.data_root.join("claude-accounts");
    fs::create_dir_all(&accounts)?;
    fs::write(
        options.data_root.join("provider-config.json"),
        json!({
            "version": 1, "revision": 2,
            "activeProviderIds": {"claude-code":"codex-subscription", "codex":"openai-official"},
            "providers":{"claude-code":[
                {"id":"codex-subscription", "kind":"subscription",
                    "subscription":{"provider":"codex", "account":{"status":"ready","plan":"Pro 500"}, "modelMetadata":[]}},
                {"id":"google-subscription", "kind":"subscription",
                    "subscription":{"provider":"antigravity","account":{"status":"ready","plan":"Google AI Ultra 5x"}, "modelMetadata":[]}}
            ], "codex":[]}
        }).to_string(),
    )?;
    let preferences = accounts.join("preferences.json");
    fs::write(
        &preferences,
        json!({
            "names":{"first":"001"}, "rememberScope":false, "renewedIds":[],
            "usage":{"first":{"checkedAt":1800000000000_u64,"stale":true,
                "fiveHour":{"used":91,"resetsAt":"2099-01-01T00:00:00Z"}}}
        })
        .to_string(),
    )?;
    let engine = root.path().join("fixture-engine");
    executable(
        &engine,
        r#"#!/usr/bin/python3
import json, pathlib, sys

root = pathlib.Path(sys.argv[1])
assert root.name == "claude-accounts"
assert (root / "preferences.json").is_file()
request = json.loads(sys.stdin.readline())
action = request["action"]
current = root / "fixture-current"
first = dict(id="first", name="alias", email="aa•••@example.invalid", needsLogin=False)
second = dict(id="second", name="gmail", email="gm•••@example.invalid", needsLogin=False)
with (root / "fixture-calls").open("a") as calls:
    print(action, file=calls)
value = {}
if action == "login":
    code = json.loads(sys.stdin.readline())
    assert code == dict(action="submit-login-code", nonce=request["nonce"], code="fixture-code#fixture-state")
    value = {"draft": second}
elif action in ("snapshot", "switch", "reauth", "remove", "save-draft"):
    if action in ("switch", "reauth"):
        current.write_text(request["id"])
    accounts = [first, second]
    if action == "remove":
        accounts = [account for account in accounts if account["id"] != request["id"]]
    value = dict(accounts=accounts, currentId=current.read_text() if current.exists() else "first")
elif action not in ("cleanup-logins", "cancel-login"):
    raise AssertionError(action)
print(json.dumps(dict(ok=True, value=value)))
"#,
    )?;
    let mut host = RuntimeHost::start_with_offline_fixtures(
        options,
        OfflineFixtures {
            claude_accounts_engine: Some(engine),
            ..Default::default()
        },
    )?;
    ready(&host)?;
    let first = result(
        &host,
        Command::GetClaudeAccounts { request_id: 1 },
        1,
        "claudeAccountsResult",
    )?;
    let serialized = first.to_string();
    for unwanted in ["Pro 500", "Google AI Ultra 5x", "codex", "antigravity"] {
        assert!(!serialized.contains(unwanted));
    }
    assert_eq!(first["currentId"], "first");
    assert_eq!(first["accounts"][0]["name"], "001");
    assert_eq!(
        first["accounts"][0]["usage"]["fiveHour"]["used"].as_f64(),
        Some(91.0)
    );
    let command = |request_id, action| Command::ClaudeAccountCommand { request_id, action };
    let request = |id, action| result(&host, command(id, action), id, "claudeAccountsResult");
    let refreshed = request(2, Action::RefreshUsage)?;
    assert_eq!(refreshed["accounts"], first["accounts"]);
    let switched = request(
        3,
        Action::Switch {
            id: "second".into(),
            remember_scope: true,
        },
    )?;
    assert_eq!(switched["currentId"], "second");
    assert_eq!(switched["rememberScope"], true);
    let renamed = request(
        4,
        Action::Rename {
            id: "second".into(),
            name: "Work".into(),
        },
    )?;
    assert_eq!(renamed["accounts"][1]["name"], "Work");
    let login = request(
        5,
        Action::Login {
            replace_id: Some("second".into()),
        },
    )?;
    let attempt = login["loginAttemptId"].as_str().context("awaiting code")?;
    let invalid = request(
        6,
        Action::SubmitLoginCode {
            attempt_id: attempt.into(),
            code: "https://example.invalid/".into(),
        },
    )?;
    assert_eq!(invalid["error"], "login-code-invalid");
    send(
        &host,
        command(
            7,
            Action::SubmitLoginCode {
                attempt_id: attempt.into(),
                code: "fixture-code#fixture-state".into(),
            },
        ),
    )?;
    let draft = next(&host, |event| match event {
        Event::Graph(value)
            if value["type"] == "claudeAccountsResult"
                && value["requestId"] == 5
                && value["result"]["draft"].is_object() =>
        {
            Some(value["result"].clone())
        }
        _ => None,
    })?;
    assert_eq!(draft["draft"]["id"], "second");
    let saved = request(
        8,
        Action::SaveDraft {
            name: "Work".into(),
        },
    )?;
    assert_eq!(saved["renewedIds"], json!(["second"]));
    let applied = request(
        9,
        Action::Reauth {
            id: "second".into(),
        },
    )?;
    assert_eq!(applied["renewedIds"], json!([]));
    let removed = request(10, Action::Remove { id: "first".into() })?;
    assert_eq!(removed["accounts"].as_array().unwrap().len(), 1);
    let prefs: Value = serde_json::from_slice(&fs::read(preferences)?)?;
    assert_eq!(prefs["names"], json!({"second":"Work"}));
    assert_eq!(prefs["usage"], json!({}));
    assert_eq!(prefs["rememberScope"], true);
    assert_eq!(
        fs::read_to_string(accounts.join("fixture-calls"))?
            .lines()
            .collect::<Vec<_>>(),
        [
            "cleanup-logins",
            "snapshot",
            "snapshot",
            "switch",
            "login",
            "save-draft",
            "reauth",
            "remove"
        ]
    );
    host.shutdown()?;
    Ok(())
}
