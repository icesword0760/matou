#![cfg(target_os = "macos")]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command as ProcessCommand, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use matou_native_runtime::{
    host::Command,
    protocol::{ClientMessage, ProviderSettingsReply, VERSION, write_message},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(8);
const DATA_FILE: &str = "provider-config.json";
const TEMP_FILE: &str = "provider-config.json.tmp";
const CLAUDE: &str = "claude-code";
const CODEX: &str = "codex";
const OFFICIAL: &str = "anthropic-official";
const KEY: &str = "SYNTHETIC-FIXTURE-ONLY";
const QUEUED_KEY: &str = "SYNTHETIC-QUEUED-PROVIDER-ONLY";
const CREDENTIAL: &str = "fixture-only.json";
const FROZEN: &str =
    include_str!("../../core/tests/fixtures/provider-config-reference.json");

struct RuntimeChild {
    child: Child,
    input: ChildStdin,
    messages: Receiver<Value>,
    stdout: Option<JoinHandle<Result<()>>>,
    stderr: Option<JoinHandle<Result<()>>>,
}

impl RuntimeChild {
    fn start(root: &Path, locale: &str) -> Result<Self> {
        let home = root.join("home");
        fs::create_dir_all(&home)?;
        let mut child = ProcessCommand::new(env!("CARGO_BIN_EXE_matou-native-runtime"))
            .env("MATOU_DISABLE_LIVE_EGRESS", "1")
            .arg("--data-root")
            .arg(root.join("data"))
            .arg("--workspace")
            .arg(root.join("workspace"))
            .arg("--shell-home")
            .arg(&home)
            .arg("--shell")
            .arg("/bin/sh")
            .env("HOME", &home)
            .env("ZDOTDIR", &home)
            .env("CLAUDE_CONFIG_DIR", home.join("claude"))
            .env("CODEX_HOME", home.join("codex"))
            .env("TMPDIR", root)
            .env("MATOU_LOCALE", locale)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take().context("child stdin")?;
        let stdout = child.stdout.take().context("child stdout")?;
        let mut stderr = child.stderr.take().context("child stderr")?;
        let (send, messages) = mpsc::channel();
        let stdout = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line?;
                assert_public(&line)?;
                // Continue to check every line through EOF, even during shutdown.
                let _ = send.send(serde_json::from_str(&line).context("decode child message")?);
            }
            Ok(())
        });
        let stderr = thread::spawn(move || {
            let mut log = Vec::new();
            stderr.read_to_end(&mut log)?;
            assert_public(&String::from_utf8_lossy(&log))
        });
        let mut runtime = Self {
            child,
            input,
            messages,
            stdout: Some(stdout),
            stderr: Some(stderr),
        };
        write_message(
            &mut runtime.input,
            &ClientMessage::Hello { version: VERSION },
        )?;
        assert_eq!(
            runtime.messages.recv_timeout(TIMEOUT)?,
            json!({"Hello": {"version": VERSION}})
        );
        runtime.message(|msg| msg["Event"].get("Ready").cloned())?;
        Ok(runtime)
    }

    fn send(&mut self, commands: impl IntoIterator<Item = Command>) -> Result<()> {
        let mut batch = Vec::new();
        for command in commands {
            write_message(&mut batch, &ClientMessage::Command(command))?;
        }
        self.input.write_all(&batch)?;
        self.input.flush()?;
        Ok(())
    }

    fn message<T>(&self, accept: impl Fn(&Value) -> Option<T>) -> Result<T> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let message = self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("waiting for runtime message")?;
            if let Some(result) = accept(&message) {
                return Ok(result);
            }
            ensure!(message["Event"] != "Closed", "runtime closed before reply");
        }
    }

    fn reply(&self, id: u64) -> Result<Value> {
        let reply = self.message(|message| {
            message["Event"]["Graph"]
                .get("providerSettingsResult")
                .cloned()
        })?;
        let decoded: ProviderSettingsReply = serde_json::from_value(reply.clone())?;
        assert_eq!(
            decoded.request_id, id,
            "provider settings replies arrived out of order"
        );
        assert_eq!(reply.as_object().context("reply object")?.len(), 2);
        Ok(reply)
    }

    fn finish(mut self) -> Result<()> {
        self.send([Command::Shutdown])?;
        self.message(|message| (message["Event"] == "Closed").then_some(()))?;
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            ensure!(
                Instant::now() < deadline,
                "runtime did not exit after Closed"
            );
            thread::sleep(Duration::from_millis(10));
        };
        self.stdout
            .take()
            .unwrap()
            .join()
            .expect("stdout reader panicked")?;
        self.stderr
            .take()
            .unwrap()
            .join()
            .expect("stderr reader panicked")?;
        ensure!(status.success(), "runtime exited with {status}");
        Ok(())
    }
}

impl Drop for RuntimeChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn assert_public(text: &str) -> Result<()> {
    for private in [
        KEY,
        QUEUED_KEY,
        CREDENTIAL,
        "\"apiKey\"",
        "\"credentialRef\"",
    ] {
        ensure!(
            !text.contains(private),
            "runtime output exposed provider credentials"
        );
    }
    Ok(())
}

fn isolated() -> Result<(TempDir, PathBuf)> {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR"))?;
    let path = temp.path().join("data").join(DATA_FILE);
    Ok((temp, path))
}

fn save(request_id: u64, input: Value) -> Command {
    Command::SaveProvider { request_id, input }
}

fn get(request_id: u64) -> Command {
    Command::GetProviderSettings { request_id }
}

fn activate(request_id: u64, id: &str) -> Command {
    Command::SetActiveProvider {
        request_id,
        cli: CLAUDE.into(),
        id: id.into(),
    }
}

fn delete(request_id: u64, id: &str) -> Command {
    Command::DeleteProvider {
        request_id,
        cli: CLAUDE.into(),
        id: id.into(),
    }
}

fn api(cli: &str, name: &str, key: &str) -> Value {
    json!({
        "cli": cli, "name": name, "endpoint": " HTTPS://EXAMPLE.INVALID:443/base/ ",
        "model": "demo-a", "apiKey": key
    })
}

#[test]
fn saves_edits_and_applies_provider_guards_through_the_child() -> Result<()> {
    let (temp, path) = isolated()?;
    let mut runtime = RuntimeChild::start(temp.path(), "zh-CN")?;
    runtime.send([get(1)])?;
    let seed = &runtime.reply(1)?["snapshot"];
    assert_eq!(seed["revision"], 1);
    assert_eq!(
        seed["activeProviderIds"],
        json!({CLAUDE: OFFICIAL, CODEX: "openai-official"})
    );
    assert_eq!(seed["providers"][CLAUDE][0]["name"], "Anthropic 官方");
    for cli in [CLAUDE, CODEX] {
        let official = &seed["providers"][cli][0];
        assert_eq!(official["model"], "CLI 默认");
        assert_eq!(official["builtIn"], true);
        assert_eq!(official["hasApiKey"], false);
    }
    assert!(
        !path.exists(),
        "Get must not write the default configuration"
    );

    let mut input = api(CLAUDE, " 隔离示例（编辑） ", KEY);
    input["modelMapping"] = json!({"opus": "demo-a", "haiku": "demo-z"});
    input["effortMapping"] = json!({"opus": "high", "sonnet": "follow"});
    input["modelMetadata"] = json!([
        { "id": "demo-z" },
        { "id": "demo-a", "displayName": "示例模型", "contextWindow": 200000,
          "maxContextWindow": 1000000, "maxOutputTokens": 4096,
          "supportedEfforts": ["high", "follow", "high", "low"] }
    ]);
    runtime.send([save(2, input), get(3)])?;
    let saved = &runtime.reply(2)?["provider"];
    let id = saved["id"].as_str().context("new provider ID")?;
    assert_eq!(uuid::Uuid::parse_str(id)?.get_version_num(), 4);
    assert_eq!(saved["hasApiKey"], true);
    assert_eq!(saved["endpoint"], "https://example.invalid/base");
    assert_eq!(
        saved["modelMapping"],
        json!({"opus": "demo-a", "sonnet": "", "haiku": "demo-z"})
    );
    assert_eq!(
        saved["effortMapping"],
        json!({"opus": "high", "sonnet": "follow"})
    );
    assert_eq!(
        saved["modelMetadata"][0]["supportedEfforts"],
        json!(["high", "low"])
    );
    let snapshot = &runtime.reply(3)?["snapshot"];
    assert_eq!(snapshot["revision"], 2);
    assert_eq!(&snapshot["providers"][CLAUDE][1], saved);

    // Project the frozen sample to its first API write with this process's generated UUID.
    let mut expected: Value = serde_json::from_str(FROZEN)?;
    expected["revision"] = json!(2);
    expected["activeProviderIds"][CLAUDE] = json!(OFFICIAL);
    expected["providers"][CLAUDE]
        .as_array_mut()
        .unwrap()
        .truncate(2);
    let row = &mut expected["providers"][CLAUDE][1];
    row["id"] = json!(id);
    row.as_object_mut().unwrap().shift_remove("legacyProvider");
    let root = expected.as_object_mut().unwrap();
    root.shift_remove("legacyRoot");
    root.shift_remove("policies");
    let expected = format!("{}\n", serde_json::to_string_pretty(&expected)?);
    assert!(
        fs::read(&path)? == expected.as_bytes(),
        "first save differs from frozen bytes"
    );

    let mut edit = api(CLAUDE, "留空密钥", "  ");
    edit["id"] = json!(id);
    runtime.send([save(4, edit.clone())])?;
    assert_eq!(runtime.reply(4)?["provider"]["hasApiKey"], true);
    let before = fs::read(&path)?;
    let disk: Value = serde_json::from_slice(&before)?;
    assert_eq!(disk["revision"], 3);
    assert!(
        disk["providers"][CLAUDE][1]["apiKey"] == KEY,
        "blank edit changed the stored key"
    );
    edit["endpoint"] = json!("https://changed.invalid");
    runtime.send([save(5, edit)])?;
    assert_eq!(
        runtime.reply(5)?,
        json!({"requestId": 5, "error": "Endpoint changed: enter the API key for the new service"})
    );
    assert!(
        fs::read(&path)? == before,
        "rejected endpoint change rewrote the file"
    );

    runtime.send([activate(6, id)])?;
    let active = &runtime.reply(6)?["snapshot"];
    assert_eq!(active["activeProviderIds"][CLAUDE], id);
    assert_eq!(active["revision"], 4);
    let before = fs::read(&path)?;
    runtime.send([delete(7, id), delete(8, OFFICIAL)])?;
    assert_eq!(
        runtime.reply(7)?,
        json!({"requestId": 7, "error": "使用中的供应商需要先切换后再删除"})
    );
    assert_eq!(
        runtime.reply(8)?,
        json!({"requestId": 8, "error": "官方供应商配置保留为默认入口"})
    );
    assert!(
        fs::read(&path)? == before,
        "rejected deletes rewrote the file"
    );

    runtime.send([activate(9, id), get(10)])?;
    assert_eq!(runtime.reply(9)?["snapshot"]["revision"], 5);
    assert_eq!(
        runtime.reply(10)?["snapshot"]["activeProviderIds"][CLAUDE],
        id
    );
    let disk: Value = serde_json::from_slice(&fs::read(&path)?)?;
    assert_eq!(disk["revision"], 5, "reapplying the active ID must write");
    assert_eq!(disk["activeProviderIds"][CLAUDE], id);

    runtime.send([activate(11, OFFICIAL), delete(12, id)])?;
    assert_eq!(runtime.reply(11)?["snapshot"]["revision"], 6);
    let deleted = &runtime.reply(12)?["snapshot"];
    assert_eq!(deleted["revision"], 7);
    assert_eq!(deleted["providers"][CLAUDE].as_array().unwrap().len(), 1);
    assert!(!path.with_file_name(TEMP_FILE).exists());
    runtime.finish()
}

#[test]
fn pending_saves_and_reads_follow_fifo_and_survive_restart() -> Result<()> {
    let (temp, path) = isolated()?;
    let mut runtime = RuntimeChild::start(temp.path(), "en")?;
    let mut first = api(CODEX, "First queued", QUEUED_KEY);
    first["reasoningEffort"] = json!("high");
    runtime.send([
        save(30, first),
        get(28),
        save(17, api(CODEX, "Second queued", KEY)),
        get(2),
    ])?;
    let first = runtime.reply(30)?["provider"].clone();
    let after_first = &runtime.reply(28)?["snapshot"];
    assert_eq!(after_first["revision"], 2);
    assert_eq!(after_first["providers"][CODEX].as_array().unwrap().len(), 2);
    assert_eq!(after_first["providers"][CODEX][1], first);
    let second = runtime.reply(17)?["provider"].clone();
    assert_eq!(first["reasoningEffort"], "high");
    assert_ne!(first["id"], second["id"]);
    let snapshot = runtime.reply(2)?["snapshot"].clone();
    assert_eq!(snapshot["revision"], 3);
    let rows = snapshot["providers"][CODEX]
        .as_array()
        .context("Codex providers")?;
    assert_eq!(rows[0]["name"], "OpenAI (official)");
    assert_eq!(&rows[1..], [first, second]);
    assert_eq!(
        snapshot["providers"][CLAUDE][0]["name"],
        "Anthropic (official)"
    );
    let before_restart = fs::read(&path)?;
    let disk: Value = serde_json::from_slice(&before_restart)?;
    assert!(disk["providers"][CODEX][1]["apiKey"] == QUEUED_KEY);
    assert!(disk["providers"][CODEX][2]["apiKey"] == KEY);
    runtime.finish()?;

    let mut restarted = RuntimeChild::start(temp.path(), "zh-CN")?;
    restarted.send([get(1)])?;
    assert_eq!(
        restarted.reply(1)?,
        json!({"requestId": 1, "snapshot": snapshot})
    );
    assert!(fs::read(path)? == before_restart);
    restarted.finish()
}

#[test]
fn existing_subscription_is_redacted_and_risk_confirmation_preserves_frozen_bytes() -> Result<()> {
    let (temp, path) = isolated()?;
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, FROZEN)?;
    let mut runtime = RuntimeChild::start(temp.path(), "zh-CN")?;
    runtime.send([get(1)])?;
    let snapshot = &runtime.reply(1)?["snapshot"];
    assert_eq!(snapshot["revision"], 6);
    let subscription = &snapshot["providers"][CLAUDE][2];
    assert_eq!(subscription["kind"], "subscription");
    assert_eq!(subscription["hasApiKey"], false);
    assert_eq!(subscription["subscription"]["provider"], "codex");
    assert_eq!(
        subscription["subscription"]["account"],
        json!({"status": "ready", "plan": "Fixture"})
    );
    assert_eq!(
        subscription["subscription"]["modelMetadata"][0]["id"],
        "demo-sub"
    );
    runtime.send([Command::ConfirmAntigravityRisk { request_id: 2 }, get(3)])?;
    assert_eq!(runtime.reply(2)?, json!({"requestId": 2, "accepted": true}));
    assert_eq!(runtime.reply(3)?["snapshot"]["revision"], 7);
    let expected = FROZEN.replacen("\"revision\": 6", "\"revision\": 7", 1);
    assert!(
        fs::read(path)? == expected.as_bytes(),
        "repeat confirmation must preserve the rest of the frozen bytes"
    );
    runtime.finish()
}

#[test]
fn invalid_json_does_not_block_startup_or_fall_back_on_read_or_write() -> Result<()> {
    let (temp, path) = isolated()?;
    fs::create_dir_all(path.parent().unwrap())?;
    let corrupt = format!("{{\"apiKey\":\"{KEY}\"");
    fs::write(&path, &corrupt)?;
    let mut runtime = RuntimeChild::start(temp.path(), "zh-CN")?;
    runtime.send([
        get(1),
        save(2, api(CLAUDE, "Cannot save", KEY)),
        delete(3, OFFICIAL),
        activate(4, OFFICIAL),
        Command::ConfirmAntigravityRisk { request_id: 5 },
        get(6),
    ])?;
    for id in 1..=6 {
        assert_eq!(
            runtime.reply(id)?,
            json!({"requestId": id, "error": "Invalid provider configuration JSON"})
        );
    }
    assert!(
        fs::read(&path)? == corrupt.as_bytes(),
        "corrupt file was modified"
    );
    assert!(!path.with_file_name(TEMP_FILE).exists());
    runtime.finish()
}

fn model_endpoint(
    status: &'static str,
    body: &'static str,
    cli: &'static str,
) -> Result<(String, JoinHandle<Result<()>>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let endpoint = format!("http://{}/v1", listener.local_addr()?);
    let server = thread::spawn(move || {
        let deadline = Instant::now() + TIMEOUT;
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        };
        // Accepted sockets on macOS can inherit the listener's nonblocking flag.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        let mut request = BufReader::new(&mut stream);
        let mut line = String::new();
        request.read_line(&mut line)?;
        assert_eq!(line.trim(), "GET /v1/models HTTP/1.1");
        let mut headers = std::collections::BTreeMap::new();
        loop {
            line.clear();
            ensure!(request.read_line(&mut line)? != 0, "truncated request");
            if line.trim().is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').context("expected header")?;
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
        assert_eq!(headers["accept"], "application/json");
        assert!(headers["authorization"] == format!("Bearer {KEY}"));
        if cli == CLAUDE {
            assert!(headers["x-api-key"] == KEY);
            assert_eq!(headers["anthropic-version"], "2023-06-01");
        } else {
            assert!(!headers.contains_key("x-api-key"));
        }
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes())?;
        stream.shutdown(Shutdown::Write)?;
        Ok(())
    });
    Ok((endpoint, server))
}

fn fetch_models(request_id: u64, input: Value) -> Command {
    Command::FetchProviderModels { request_id, input }
}

#[test]
fn discovers_models_and_classifies_loopback_http_results() -> Result<()> {
    let (temp, path) = isolated()?;
    let mut runtime = RuntimeChild::start(temp.path(), "en")?;
    let (endpoint, server) = model_endpoint(
        "200 OK",
        r#"{"data":[{"id":"z"},{"id":"a"},{"id":"a","name":"Discovery A",
             "context_window":200000,"max_context_window":1000000,"max_output_tokens":4096,
             "effort":{"supported_levels":["high","bogus","high","low"]}},{"id":42}]}"#,
        CLAUDE,
    )?;
    runtime.send([save(
        1,
        json!({
            "cli": CLAUDE, "name": "Discovery fixture", "endpoint": endpoint,
            "model": "a", "apiKey": KEY,
        }),
    )])?;
    let saved = runtime.reply(1)?;
    let id = saved["provider"]["id"].as_str().context("provider ID")?;
    let before = fs::read(&path)?;
    runtime.send([fetch_models(
        2,
        json!({
            "cli": CLAUDE, "providerId": id, "endpoint": format!(" {endpoint}/ "),
        }),
    )])?;
    let found = runtime.reply(2)?;
    server.join().expect("model endpoint panicked")?;
    assert_eq!(
        found["models"],
        json!({
            "models": ["a", "z"],
            "modelMetadata": [{
                "id": "a", "displayName": "Discovery A", "contextWindow": 200000,
                "maxContextWindow": 1000000, "maxOutputTokens": 4096,
                "supportedEfforts": ["high", "low"]
            }, {"id": "z"}]
        })
    );
    let closed = TcpListener::bind(("127.0.0.1", 0))?;
    let changed = format!("http://{}/v1", closed.local_addr()?);
    runtime.send([fetch_models(
        3,
        json!({
            "cli": CLAUDE, "providerId": id, "endpoint": changed,
        }),
    )])?;
    assert_eq!(
        runtime.reply(3)?["models"],
        json!({"error": "key-required"})
    );

    for (request_id, status, body, expected) in [
        (4, "401 Unauthorized", "{}", json!({"error": "auth"})),
        (5, "404 Not Found", "{}", json!({"error": "unsupported"})),
        (6, "200 OK", r#"{"data":[]}"#, json!({"error": "empty"})),
        (
            7,
            "200 OK",
            r#"{"data":[{"id":"only-id"}]}"#,
            json!({"models":["only-id"]}),
        ),
    ] {
        let (endpoint, server) = model_endpoint(status, body, CODEX)?;
        runtime.send([fetch_models(
            request_id,
            json!({
                "cli": CODEX, "endpoint": endpoint, "apiKey": KEY,
            }),
        )])?;
        let discovered = runtime.reply(request_id)?;
        server.join().expect("model endpoint panicked")?;
        assert_eq!(discovered["models"], expected);
    }
    runtime.send([fetch_models(
        8,
        json!({
            "cli": CODEX, "endpoint": "http://127.0.0.1/v1?secret=x", "apiKey": KEY,
        }),
    )])?;
    assert_eq!(runtime.reply(8)?["models"], json!({"error": "invalid"}));
    assert_eq!(
        fs::read(path)?,
        before,
        "discovery must not change settings"
    );
    runtime.finish()
}

#[test]
fn edits_subscription_and_context_mode_without_replacing_the_account() -> Result<()> {
    let (temp, path) = isolated()?;
    fs::create_dir_all(path.parent().unwrap())?;
    let mut seed: Value = serde_json::from_str(FROZEN)?;
    let subscription = &mut seed["providers"][CLAUDE][2];
    let id = subscription["id"]
        .as_str()
        .context("subscription ID")?
        .to_owned();
    subscription["subscription"]["modelMetadata"]
        .as_array_mut()
        .unwrap()
        .extend([
            json!({"id": "demo-medium", "supportedEfforts": ["medium", "max"]}),
            json!({"id": "demo-fast"}),
            json!({"id": "demo-image"}),
        ]);
    let original_account = subscription["subscription"].clone();
    fs::write(&path, serde_json::to_vec(&seed)?)?;
    let mut runtime = RuntimeChild::start(temp.path(), "en")?;
    let input = json!({
        "cli": CLAUDE, "id": id, "kind": "subscription", "name": " Renamed subscription ",
        "modelMapping": {"opus": "demo-sub", "sonnet": "demo-medium", "haiku": "demo-fast"},
        "effortMapping": {"opus": "high", "sonnet": "medium", "haiku": "low"},
        "contextWindowMode": "maximum",
    });
    let mut invalid = input.clone();
    invalid["modelMapping"]["haiku"] = json!("demo-image");
    let mut bad_effort = input.clone();
    bad_effort["effortMapping"]["sonnet"] = json!("max");
    let mut credentials = input.clone();
    credentials["subscription"] = json!({"credentialRef": {"name": "replacement.json"}});
    runtime.send([save(1, invalid), save(2, bad_effort), save(3, credentials)])?;
    for request_id in 1..=3 {
        assert!(runtime.reply(request_id)?["error"].is_string());
    }
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&path)?)?, seed);
    runtime.send([save(4, input.clone()), activate(5, &id)])?;
    let edited = runtime.reply(4)?["provider"].clone();
    assert_eq!(edited["kind"], "subscription");
    assert_eq!(edited["name"], "Renamed subscription");
    assert_eq!(edited["model"], "demo-medium");
    assert_eq!(edited["modelMapping"], input["modelMapping"]);
    assert_eq!(edited["effortMapping"], input["effortMapping"]);
    assert_eq!(edited["contextWindowMode"], "maximum");
    // This settings fixture has a stored reference and no credential file.
    // A missing credential must fail the activation without undoing the edit.
    assert!(
        runtime.reply(5)?["error"].as_str().is_some_and(|message| {
            message.contains("Subscription credential is missing or no longer valid")
        })
    );
    let disk: Value = serde_json::from_slice(&fs::read(&path)?)?;
    assert_eq!(disk["activeProviderIds"][CLAUDE], seed["activeProviderIds"][CLAUDE]);
    assert_eq!(
        disk["providers"][CLAUDE][2]["subscription"],
        original_account
    );
    runtime.finish()?;

    let db = rusqlite::Connection::open(path.with_file_name("matou.sqlite"))?;
    let preferences: String = db.query_row(
        "SELECT value_json FROM preferences WHERE key = 'agent.modelPreferences'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        serde_json::from_str::<Value>(&preferences)?["contextWindows"][&id],
        "maximum"
    );
    let mut runtime = RuntimeChild::start(temp.path(), "en")?;
    runtime.send([get(6)])?;
    assert_eq!(
        runtime.reply(6)?["snapshot"]["providers"][CLAUDE][2],
        edited
    );
    runtime.finish()
}
