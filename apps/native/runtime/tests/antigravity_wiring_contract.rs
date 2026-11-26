//! RuntimeHost, PTY and authenticated callbacks with a local agy fixture.
#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command as Process, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use matou_native_runtime::host::{
    Command, Event, ForkPlacement, ForkWorktreeMode, LaunchOptions, OfflineFixtures, RuntimeHost,
    STORAGE_READ_ONLY,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const SOURCE: &str = "agy-source-fixture";
const OTHER: &str = "agy-other-fixture";
const CHILD: &str = "agy-child-fixture";
const TIMEOUT: Duration = Duration::from_secs(12);
const SETTINGS: &str = r#"{"fixture":"existing settings"}"#;

struct Fixture {
    _root: TempDir,
    data: PathBuf,
    cwd: PathBuf,
    shell_home: PathBuf,
    executable: PathBuf,
    traces: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
        fs::create_dir_all(&target)?;
        let root = tempfile::Builder::new().prefix("ag-").tempdir_in(target)?;
        let path = root.path().canonicalize()?;
        let fixture = Self {
            _root: root,
            data: path.join("data"),
            cwd: path.join("workspace"),
            shell_home: path.join("shell-home"),
            executable: path.join("agy"),
            traces: path.join("traces"),
        };
        for directory in [&fixture.cwd, &fixture.shell_home, &fixture.traces] {
            fs::create_dir_all(directory)?;
        }
        fs::write(fixture.shell_home.join(".zshrc"), "PROMPT='FIXTURE> '\n")?;
        fs::create_dir(fixture.shell_home.join(".gemini"))?;
        fs::write(fixture.shell_home.join(".gemini/settings.json"), SETTINGS)?;
        fs::write(
            &fixture.executable,
            r#"#!/bin/sh
set -eu
[ -z "${CLAUDECODE:-}" ] && [ "${MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE:-}" != "stale-file" ] && [ -f "${MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE:-}" ]
[ -f "$HOME/.gemini/settings.json" ]
trace="$AGY_TEST_TRACES/$MATOU_PROVIDER_HOOK_TOKEN"
mkdir "$trace"
printf '%s\n' "$@" > "$trace/args"
printf '%s\n' "$HOME" "$MATOU_PROVIDER_HOOK_URL" > "$trace/launch"
echo AGY_FIXTURE_READY
touch "$trace/ready"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$trace/input"
  [ "$line" != /exit ] || exit 17
done
"#,
        )?;
        fs::set_permissions(&fixture.executable, fs::Permissions::from_mode(0o700))?;
        Ok(fixture)
    }

    fn plugin(&self) -> PathBuf {
        self.shell_home.join(".gemini/config/plugins/matou")
    }

    fn history_file(&self) -> PathBuf {
        self.shell_home
            .join(".gemini/antigravity-cli/cache/conversation_metadata.json")
    }

    fn write_history(&self, title: &str) -> Result<()> {
        let history = self.history_file();
        fs::create_dir_all(history.parent().unwrap())?;
        fs::write(
            &history,
            serde_json::to_vec(&json!({"conversations": {
                SOURCE: {"last_modified_time": 200, "summary": {
                    "AppDataDir": "antigravity-cli", "WorkspaceURIs": [&self.cwd],
                    "Title": title, "NumSteps": 2,
                }},
                OTHER: {"last_modified_time": 100, "summary": {
                    "AppDataDir": "antigravity-cli", "WorkspaceURIs": [&self.cwd],
                    "Title": "Another conversation",
                }},
            }}))?,
        )?;
        let transcript = history.parent().unwrap().parent().unwrap().join(format!(
            "brain/{SOURCE}/.system_generated/logs/transcript.jsonl"
        ));
        fs::create_dir_all(transcript.parent().unwrap())?;
        fs::write(
            transcript,
            concat!(
                "{\"source\":\"USER_EXPLICIT\",\"content\":\"find fixture needle\"}\n",
                "{\"type\":\"PLANNER_RESPONSE\",\"content\":\"found fixture needle\"}\n",
            ),
        )?;
        Ok(())
    }

    fn start(&self) -> Result<RuntimeHost> {
        RuntimeHost::start_with_offline_fixtures(
            LaunchOptions {
                data_root: self.data.clone(),
                workspace: self.cwd.clone(),
                shell_home: self.shell_home.clone(),
                shell: "/bin/zsh".into(),
            },
            OfflineFixtures {
                provider_environment: Some(
                    [
                        ("PATH".into(), "/usr/bin:/bin".into()),
                        ("HOME".into(), self.traces.clone().into_os_string()),
                        (
                            "MATOU_ANTIGRAVITY_COMMAND".into(),
                            self.executable.clone().into_os_string(),
                        ),
                        (
                            "AGY_TEST_TRACES".into(),
                            self.traces.clone().into_os_string(),
                        ),
                        ("MATOU_PROVIDER_HOOK_URL".into(), "stale-hook".into()),
                        (
                            "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE".into(),
                            "stale-file".into(),
                        ),
                        ("CLAUDECODE".into(), "stale-caller".into()),
                    ]
                    .into(),
                ),
                ..Default::default()
            },
        )
    }

    fn run(&self, count: usize) -> Result<Run> {
        let trace = wait(|| {
            let mut ready = fs::read_dir(&self.traces)?
                .collect::<std::io::Result<Vec<_>>>()?
                .into_iter()
                .map(|entry| entry.path())
                .filter(|path| path.join("ready").is_file())
                .collect::<Vec<_>>();
            if ready.len() < count {
                return Ok(None);
            }
            ready.sort_by_key(|trace| {
                fs::metadata(trace.join("ready"))
                    .unwrap()
                    .modified()
                    .unwrap()
            });
            Ok(Some(ready.pop().unwrap()))
        })
        .with_context(|| format!("waiting for fixture PTY run {count}"))?;
        let launch = fs::read_to_string(trace.join("launch"))?;
        let mut lines = launch.lines();
        assert_eq!(lines.next(), self.shell_home.to_str());
        let endpoint = lines
            .next()
            .context("Missing fixture hook endpoint")?
            .to_owned();
        let args = fs::read_to_string(trace.join("args"))?
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(args.first().map(String::as_str), Some("--settings"));
        let settings: Value = serde_json::from_slice(&fs::read(&args[1])?)?;
        assert_eq!(settings["statusLine"]["stack_with_default"], false);
        Ok(Run {
            endpoint,
            statusline: settings["statusLine"]["command"]
                .as_str()
                .context("Missing statusline script")?
                .into(),
            args,
            trace,
        })
    }

    fn promote(&self, host: &RuntimeHost, session: &str) -> Result<()> {
        input(host, session, b"agy --mode=plan\r")?;
        graph(host, |graph| {
            node(graph, session)["currentMode"] == "antigravity"
        })?;
        Ok(())
    }
}

struct Run {
    endpoint: String,
    statusline: String,
    args: Vec<String>,
    trace: PathBuf,
}

impl Run {
    fn hook(&self, event: &str, conversation: &str) -> Result<()> {
        let code = self.post(
            "",
            json!({
                "hookEventName": event, "conversationId": conversation,
                "lastAssistantMessage": "Fixture completed",
            }),
        )?;
        assert_eq!(code, 200);
        Ok(())
    }

    fn status(&self, conversation: &str, state: &str) -> Result<()> {
        let code = self.post(
            "?kind=statusline",
            json!({
                "conversation_id": conversation, "agent_state": state, "cycle_mode": "plan",
            }),
        )?;
        assert_eq!(code, 200);
        Ok(())
    }

    fn post(&self, query: &str, payload: Value) -> Result<u16> {
        let endpoint = self
            .endpoint
            .strip_prefix("http://127.0.0.1:")
            .context("Hook escaped loopback")?;
        let (port, path) = endpoint.split_once('/').context("Invalid hook endpoint")?;
        let mut stream = TcpStream::connect(("127.0.0.1", port.parse()?))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let body = payload.to_string();
        write!(
            stream,
            "POST /{path}{query} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response
            .split_ascii_whitespace()
            .nth(1)
            .context("Missing hook HTTP response")?
            .parse()?)
    }

    fn run_statusline(&self, conversation: &str, state: &str) -> Result<()> {
        let mut process = Process::new("/bin/sh")
            .args(["-c", &self.statusline])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .spawn()?;
        write!(
            process
                .stdin
                .take()
                .context("Missing fixture statusline input")?,
            "{}",
            json!({"conversation_id": conversation, "agent_state": state, "cycle_mode": "plan"})
        )?;
        assert!(process.wait()?.success());
        Ok(())
    }

    fn resume_arg(&self) -> Option<&str> {
        self.args
            .windows(2)
            .find(|args| args[0] == "--conversation")
            .map(|args| args[1].as_str())
    }

    fn input_log(&self) -> String {
        fs::read_to_string(self.trace.join("input")).unwrap_or_default()
    }
}

fn wait<T>(mut poll: impl FnMut() -> Result<Option<T>>) -> Result<T> {
    let end = Instant::now() + TIMEOUT;
    loop {
        if let Some(found) = poll()? {
            return Ok(found);
        }
        ensure!(Instant::now() < end, "Timed out waiting for Runtime");
        thread::sleep(Duration::from_millis(5));
    }
}

fn send(host: &RuntimeHost, command: Command) -> Result<()> {
    host.commands
        .send(command)
        .map_err(|_| anyhow::anyhow!("Runtime command channel closed"))
}

fn input(host: &RuntimeHost, session: &str, bytes: &[u8]) -> Result<()> {
    send(
        host,
        Command::Input {
            session_id: session.into(),
            bytes: bytes.to_vec(),
            defer_ordering: false,
        },
    )
}

fn receive<T>(host: &RuntimeHost, mut accept: impl FnMut(Event) -> Option<T>) -> Result<T> {
    wait(|| {
        while let Ok(event) = host.events.try_recv() {
            match &event {
                Event::Frame {
                    session_id,
                    sequence,
                    ..
                } => send(
                    host,
                    Command::Applied {
                        session_id: session_id.clone(),
                        sequence: *sequence,
                    },
                )?,
                Event::Diagnostic(message) => eprintln!("Runtime diagnostic: {message}"),
                Event::Closed => bail!("Runtime closed during verification"),
                _ => {}
            }
            if let Some(value) = accept(event) {
                return Ok(Some(value));
            }
        }
        Ok(None)
    })
}

fn ready(host: &RuntimeHost) -> Result<(String, String)> {
    receive(host, |event| match event {
        Event::Ready { graph, .. } => Some((
            graph["sceneId"].as_str()?.to_owned(),
            graph["focusedSessionId"].as_str()?.to_owned(),
        )),
        _ => None,
    })
}

fn graph(host: &RuntimeHost, predicate: impl Fn(&Value) -> bool) -> Result<Value> {
    receive(host, |event| match event {
        Event::Graph(graph) | Event::Navigation { graph, .. } if predicate(&graph) => Some(graph),
        _ => None,
    })
}

fn node<'a>(graph: &'a Value, session: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|node| node["sessionId"] == session)
        .unwrap_or(&Value::Null)
}

fn identity(host: &RuntimeHost, session: &str, result: &str) -> Result<Value> {
    let predicate = |value: &Value| {
        value["sessionId"] == session
            && value["provider"] == "antigravity"
            && value["result"] == result
    };
    let projection = graph(host, |graph| {
        graph["providerIdentityEvents"]
            .as_array()
            .into_iter()
            .flatten()
            .any(predicate)
    })
    .with_context(|| format!("waiting for Antigravity identity {result}"))?;
    Ok(projection["providerIdentityEvents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| predicate(value))
        .unwrap()
        .clone())
}

fn reply(host: &RuntimeHost, command: Command, id: u64, kind: &str) -> Result<Value> {
    send(host, command)?;
    graph(host, |graph| {
        graph["requestId"] == id && graph["type"] == kind
    })
    .with_context(|| format!("waiting for {kind} request {id}"))
}

fn load(host: &RuntimeHost, id: u64, session: &str, conversation: &str) -> Result<Value> {
    reply(
        host,
        Command::LoadAntigravitySession {
            request_id: id,
            session_id: session.into(),
            provider_session_id: conversation.into(),
        },
        id,
        "antigravitySessionLoadResult",
    )
}

fn browse(fixture: &Fixture, host: &RuntimeHost) -> Result<()> {
    let cwd = fixture.cwd.to_string_lossy().into_owned();
    let list = reply(
        host,
        Command::ListAntigravitySessions {
            request_id: 1,
            cwd: cwd.clone(),
            query: String::new(),
            full_text: false,
            offset: None,
            limit: Some(1),
        },
        1,
        "antigravitySessionListResult",
    )?;
    assert_eq!(list["result"]["total"], 2, "{list}");
    assert_eq!(list["result"]["hasMore"], true);
    assert_eq!(list["result"]["sessions"][0]["providerSessionId"], SOURCE);
    let detail = reply(
        host,
        Command::GetAntigravitySession {
            request_id: 2,
            cwd: cwd.clone(),
            provider_session_id: SOURCE.into(),
            before_event_index: Some(2),
            around_event_index: None,
            limit: Some(1),
        },
        2,
        "antigravitySessionDetailResult",
    )?;
    assert_eq!(
        detail["result"]["events"][0]["text"], "find fixture needle",
        "{detail}"
    );
    let search = reply(
        host,
        Command::SearchAntigravitySession {
            request_id: 3,
            cwd,
            provider_session_id: SOURCE.into(),
            query: "FIXTURE NEEDLE".into(),
            offset: Some(1),
            limit: Some(1),
        },
        3,
        "antigravitySessionSearchResult",
    )?;
    assert_eq!(search["result"]["total"], 2, "{search}");
    assert_eq!(search["result"]["hits"][0]["eventIndex"], 2);
    Ok(())
}

#[test]
fn promotion_installs_and_repairs_plugin_and_projects_authenticated_activity() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.write_history("Observed title")?;
    let plugin = fixture.plugin();
    let third_party = plugin.with_file_name("third-party");
    fs::create_dir_all(&third_party)?;
    let third_party_manifest = third_party.join("plugin.json");
    let third_party_contents = r#"{"name":"third-party"}"#;
    fs::write(&third_party_manifest, third_party_contents)?;
    let mut host = fixture.start()?;
    let (_, session) = ready(&host)?;
    fixture.promote(&host, &session)?;
    let first = fixture.run(1)?;
    let egress = reply(
        &host,
        Command::GetEgressStatus { request_id: 90 },
        90,
        "egressStatusResult",
    )?;
    assert_eq!(
        egress["result"]["sessionFamilies"][&session],
        json!(["google"])
    );
    assert!(first.args.windows(2).any(|args| args == ["--mode", "plan"]));
    assert!(first.resume_arg().is_none());
    let managed_hook = plugin.join("hooks/session-start.sh");
    let contents = fs::read(&managed_hook)?;
    let version = fs::metadata(plugin.join("plugin.json"))?.modified()?;
    assert_eq!(
        fs::read_to_string(fixture.shell_home.join(".gemini/settings.json"))?,
        SETTINGS
    );
    assert_eq!(
        fs::read_to_string(third_party_manifest)?,
        third_party_contents
    );
    assert!(!fixture.traces.join(".gemini").exists());

    let mut start = Process::new(&managed_hook)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MATOU_PROVIDER_HOOK_URL", &first.endpoint)
        .env(
            "MATOU_PROVIDER_HOOK_TOKEN",
            first.trace.file_name().unwrap(),
        )
        .stdin(Stdio::piped())
        .spawn()?;
    write!(
        start.stdin.take().unwrap(),
        "{}",
        json!({
            "data": {"conversationId": SOURCE, "cwd": fixture.cwd}, "event": "SessionStart",
        })
    )?;
    assert!(start.wait()?.success());
    assert_eq!(
        identity(&host, &session, "confirmed")?["providerSessionId"],
        SOURCE
    );
    first.run_statusline(SOURCE, "working")?;
    let working = graph(&host, |graph| {
        node(graph, &session)["workStatus"] == "running"
    })
    .context("statusline running")?;
    assert_eq!(node(&working, &session)["historyProviderSessionId"], SOURCE);
    first.hook("PreInvocation", SOURCE)?;
    first.hook("Stop", SOURCE)?;
    let stopped = graph(&host, |graph| {
        node(graph, &session)["providerTitle"] == "Observed title"
            && graph["agentNotifications"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
    })
    .context("Stop title and notification")?;
    assert_eq!(node(&stopped, &session)["workStatus"], "idle");
    assert_eq!(node(&stopped, &session)["canFork"], true);

    send(
        &host,
        Command::SetSessionTitle {
            session_id: session.clone(),
            title: Some("Manual title".into()),
        },
    )?;
    graph(&host, |graph| {
        node(graph, &session)["titleSource"] == "manual"
    })?;
    first.hook("Stop", SOURCE)?;
    // Hook completion can precede the asynchronous catalog update.
    graph(&host, |graph| {
        graph["agentNotifications"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    })?;
    fixture.write_history("Later automatic title")?;
    let titled = graph(&host, |graph| {
        node(graph, &session)["providerTitle"] == "Later automatic title"
    })?;
    assert_eq!(node(&titled, &session)["title"], "Manual title");

    // Reload on the same card to exercise replacement and run token retirement.
    assert_eq!(
        load(&host, 4, &session, SOURCE)?["result"]["status"],
        "restoring"
    );
    let second = fixture.run(2)?;
    assert_eq!(second.resume_arg(), Some(SOURCE));
    assert!(second.args.windows(2).any(|args| args == ["--mode", "plan"]));
    assert_eq!(
        fs::metadata(plugin.join("plugin.json"))?.modified()?,
        version
    );
    assert_eq!(fs::read(&managed_hook)?, contents);
    assert_eq!(
        first.post("", json!({"hookEventName":"Stop", "conversationId": OTHER}))?,
        200
    );
    assert_eq!(
        identity(&host, &session, "rejected")?["reason"],
        "retired-run"
    );
    second.status(SOURCE, "idle")?;
    assert_eq!(
        identity(&host, &session, "confirmed")?["providerSessionId"],
        SOURCE
    );
    fs::write(&managed_hook, "broken")?;
    assert_eq!(
        load(&host, 5, &session, SOURCE)?["result"]["status"],
        "restoring"
    );
    let third = fixture.run(3)?;
    assert_eq!(fs::read(&managed_hook)?, contents);
    third.status(SOURCE, "idle")?;
    assert_eq!(
        identity(&host, &session, "confirmed")?["providerSessionId"],
        SOURCE
    );
    host.shutdown()
}

#[test]
fn restart_opens_never_bound_card_then_resumes_its_binding_without_workspace_uris() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.write_history("Bound conversation")?;
    let mut host = fixture.start()?;
    let (_, session) = ready(&host)?;
    fixture.promote(&host, &session)?;
    let first = fixture.run(1)?;
    assert_eq!(first.resume_arg(), None);
    // The first frame can precede the committed Shell-to-provider handoff.
    // Input reaches the stub only after the host releases the promotion queue.
    input(&host, &session, b"fixture-ready\r")?;
    wait(|| Ok(first.input_log().contains("fixture-ready").then_some(())))?;
    host.shutdown()?;

    let mut host = fixture.start()?;
    assert_eq!(ready(&host)?.1, session);
    let fresh = fixture.run(2)?;
    assert_eq!(fresh.resume_arg(), None);
    fresh.status(SOURCE, "idle")?;
    assert_eq!(
        identity(&host, &session, "confirmed")?["restoreState"],
        "none"
    );
    host.shutdown()?;
    assert_eq!(fs::read_dir(&fixture.traces)?.count(), 2);

    let mut metadata: Value = serde_json::from_slice(&fs::read(fixture.history_file())?)?;
    metadata["conversations"][SOURCE]["summary"]["WorkspaceURIs"] = Value::Null;
    fs::write(fixture.history_file(), serde_json::to_vec(&metadata)?)?;
    let mut catalog = matou_native_runtime::antigravity_catalog::Catalog::new(
        fixture.shell_home.join(".gemini/antigravity-cli"),
    );
    let listed = catalog.list(&fixture.cwd, Default::default())?;
    assert_eq!(listed.sessions.len(), 1);
    assert_eq!(listed.sessions[0].provider_session_id, OTHER);
    assert!(
        catalog
            .detail(&fixture.cwd, SOURCE, Default::default())
            .is_err()
    );

    let mut host = fixture.start()?;
    assert_eq!(ready(&host)?.1, session);
    let restored = fixture.run(3)?;
    assert_eq!(restored.resume_arg(), Some(SOURCE));
    restored.status(SOURCE, "idle")?;
    assert_eq!(
        identity(&host, &session, "confirmed")?["providerSessionId"],
        SOURCE
    );
    host.shutdown()?;
    assert_eq!(fs::read_dir(&fixture.traces)?.count(), 3);
    Ok(())
}

#[test]
fn catalog_recovers_retries_and_obeys_read_only_storage() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.write_history("Catalog title")?;
    let mut host = fixture.start()?;
    let (_, session) = ready(&host)?;
    browse(&fixture, &host)?;
    assert!(load(&host, 4, &session, "missing")?["error"].is_string());
    fs::write(fixture.history_file(), "{invalid")?;
    assert!(load(&host, 5, &session, SOURCE)?["error"].is_string());
    fixture.write_history("Repaired catalog title")?;
    assert_eq!(
        load(&host, 6, &session, SOURCE)?["result"]["status"],
        "restoring"
    );
    let first = fixture.run(1)?;
    assert_eq!(first.resume_arg(), Some(SOURCE));
    first.status(OTHER, "idle")?;
    assert_eq!(
        identity(&host, &session, "restore-failed")?["restoreState"],
        "failed"
    );

    fs::write(fixture.history_file(), "{invalid")?;
    send(
        &host,
        Command::RetryProviderRestore {
            session_id: session.clone(),
        },
    )?;
    let failed = identity(&host, &session, "restore-failed")?;
    assert!(
        failed["restoreError"]
            .as_str()
            .unwrap()
            .contains("history is unavailable")
    );
    fixture.write_history("Repaired catalog title")?;
    send(
        &host,
        Command::RetryProviderRestore {
            session_id: session.clone(),
        },
    )?;
    let second = fixture.run(2)?;
    assert_eq!(second.resume_arg(), Some(SOURCE));
    second.status(SOURCE, "idle")?;
    assert_eq!(
        identity(&host, &session, "confirmed")?["restoreState"],
        "none"
    );

    // A second failed restore can start a new conversation on the same card.
    assert_eq!(
        load(&host, 7, &session, SOURCE)?["result"]["status"],
        "restoring"
    );
    fixture.run(3)?.status(OTHER, "idle")?;
    identity(&host, &session, "restore-failed")?;
    send(
        &host,
        Command::StartFreshProviderSession {
            session_id: session.clone(),
        },
    )?;
    let fresh = fixture.run(4)?;
    assert_eq!(fresh.resume_arg(), None);
    fresh.status(OTHER, "idle")?;
    assert_eq!(
        identity(&host, &session, "confirmed")?["providerSessionId"],
        OTHER
    );
    host.shutdown()?;

    let database = rusqlite::Connection::open(fixture.data.join("matou.sqlite"))?;
    database.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE);
         INSERT INTO schema_migrations (version, name, checksum, applied_at)
         VALUES (9999, 'fixture-newer', 'fixture-newer', 5);",
    )?;
    drop(database);
    let mut readonly = fixture.start()?;
    receive(&readonly, |event| {
        matches!(event, Event::ReadOnlyReady { .. }).then_some(())
    })?;
    browse(&fixture, &readonly)?;
    let rejected = load(&readonly, 8, &session, SOURCE)?;
    assert_eq!(rejected["code"], STORAGE_READ_ONLY);
    assert!(rejected["error"].is_string());
    assert_eq!(fs::read_dir(&fixture.traces)?.count(), 4);
    readonly.shutdown()
}

#[test]
fn fork_waits_for_parent_idle_and_retry_uses_a_new_run() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.write_history("Fork source")?;
    let mut host = fixture.start()?;
    let (scene, source) = ready(&host)?;
    fixture.promote(&host, &source)?;
    let parent = fixture.run(1)?;
    parent.hook("SessionStart", SOURCE)?;
    identity(&host, &source, "confirmed")?;
    parent.hook("PreInvocation", SOURCE)?;
    parent.hook("Stop", SOURCE)?;
    graph(&host, |graph| node(graph, &source)["canFork"] == true)?;
    send(
        &host,
        Command::SetProviderPermissionMode {
            session_id: source.clone(),
            mode: "turbo".into(),
        },
    )?;
    let parent = fixture.run(2)?;
    parent.status(SOURCE, "idle")?;
    let receipt = graph(&host, |graph| {
        graph["providerPermissionModeResults"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|result| result["sessionId"] == source && result["permissionMode"] == "turbo")
    })?;
    assert!(receipt["providerPermissionModeResults"][0]["error"].is_null());
    let assert_turbo = |run: &Run| -> Result<()> {
        let settings: Value = serde_json::from_slice(&fs::read(&run.args[1])?)?;
        assert_eq!(settings["agentMode"], "accept-edits");
        assert_eq!(settings["toolPermission"], "always-proceed");
        assert_eq!(settings["enableTerminalSandbox"], false);
        assert!(run.args.iter().any(|arg| arg == "--dangerously-skip-permissions"));
        Ok(())
    };
    assert_turbo(&parent)?;
    send(
        &host,
        Command::ForkSession {
            scene_id: scene,
            source_session_id: source,
            placement: ForkPlacement::Child,
            name: "Fixture fork".into(),
            worktree_mode: ForkWorktreeMode::Current,
            submission_key: "fixture-fork".into(),
        },
    )?;
    let response = graph(&host, |graph| {
        graph["forkSessionResult"]["submissionKey"] == "fixture-fork"
    })?;
    let child = response["forkSessionResult"]["sessionId"]
        .as_str()
        .with_context(|| response.to_string())?
        .to_owned();
    let first = fixture.run(3)?;
    assert_eq!(first.resume_arg(), Some(SOURCE));
    assert_turbo(&first)?;
    first.status(SOURCE, "authenticating")?;
    first.status(CHILD, "idle")?;
    // This ordinary input is a PTY barrier while callbacks are processed.
    input(&host, &child, b"fixture barrier\r")?;
    wait(|| Ok(first.input_log().contains("fixture barrier").then_some(())))?;
    assert!(!first.input_log().contains("/fork"));
    first.status(SOURCE, "idle")?;
    first.status(SOURCE, "idle")?;
    wait(|| Ok(first.input_log().contains("/fork").then_some(())))?;
    input(&host, &child, b"/exit\r")?;
    identity(&host, &child, "fork-failed")?;
    assert_eq!(
        first
            .input_log()
            .lines()
            .filter(|line| *line == "/fork")
            .count(),
        1
    );

    send(
        &host,
        Command::RetryFork {
            session_id: child.clone(),
        },
    )?;
    let retry = graph(&host, |graph| {
        graph["forkSessionResult"]["sessionId"] == child
    })?;
    assert!(retry["forkSessionResult"]["error"].is_null(), "{retry}");
    let second = fixture.run(4)?;
    assert_eq!(second.resume_arg(), Some(SOURCE));
    assert_turbo(&second)?;
    assert_ne!(first.endpoint, second.endpoint);
    second.status(SOURCE, "idle")?;
    wait(|| Ok(second.input_log().contains("/fork").then_some(())))?;
    second.status(SOURCE, "idle")?;
    second.status(CHILD, "idle")?;
    let bound = identity(&host, &child, "confirmed")?;
    assert_eq!(bound["providerSessionId"], CHILD);
    assert_eq!(bound["metadata"]["canFork"], true);
    assert_eq!(bound["metadata"]["permissionMode"], "turbo");
    assert_eq!(
        second
            .input_log()
            .lines()
            .filter(|line| *line == "/fork")
            .count(),
        1
    );
    let mut history: Value = serde_json::from_slice(&fs::read(fixture.history_file())?)?;
    history["conversations"][CHILD] = history["conversations"][SOURCE].clone();
    fs::write(fixture.history_file(), serde_json::to_vec(&history)?)?;
    input(&host, &child, b"/exit\r")?;
    graph(&host, |graph| node(graph, &child)["workStatus"] == "exited")?;
    send(&host, Command::RestartSession { session_id: child.clone() })?;
    let restored = fixture.run(5)?;
    assert_eq!(restored.resume_arg(), Some(CHILD));
    assert_turbo(&restored)?;
    restored.status(CHILD, "idle")?;
    let restored = identity(&host, &child, "confirmed")?;
    assert_eq!(restored["metadata"]["permissionMode"], "turbo");
    host.shutdown()
}
