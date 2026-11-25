#![cfg(unix)]

use anyhow::{Context, Result, ensure};
use matou_native_runtime::{
    host::{Command, Event},
    mt_cli::Client,
    protocol::{ClientMessage, ServerMessage, VERSION, read_messages, write_message},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command as Process, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;

const RUNTIME: &str = env!("CARGO_BIN_EXE_matou-native-runtime");
const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../tooling/native-parity/fixtures/host-control-reference.json"
));
const MAX_FRAME: u32 = 2 * 1024 * 1024;

fn root() -> Result<TempDir> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join(".git").exists())
        .context("find the repository for short Unix socket paths")?;
    Ok(tempfile::Builder::new()
        .prefix(".mtd-")
        .tempdir_in(repository)?)
}

fn fixture(id: &str) -> Value {
    serde_json::from_str::<Value>(FIXTURE).unwrap()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap()
        .clone()
}

fn expected_help() -> String {
    fixture("cli-help-no-identity")["expected_response"]["stdout"][0]
        .as_str()
        .unwrap()
        .replace(
            "mt send TARGET TEXT [--enter] [--json]",
            "mt send TARGET TEXT [--enter] [--replace | --append] [--json]",
        )
}

fn invoke(args: &[&str], endpoint: Option<&Path>, token: Option<&str>) -> Result<Output> {
    let mut command = Process::new(RUNTIME);
    command
        .env_clear()
        .arg("--internal-mt")
        .args(args)
        .stdin(Stdio::null());
    if let Some(endpoint) = endpoint {
        command.env("MATOU_CONTROL_ENDPOINT", endpoint);
    }
    if let Some(token) = token {
        command.env("MATOU_CONTROL_TOKEN", token);
    }
    Ok(command.output()?)
}

fn compare(output: Output, code: i32, stdout: &str, stderr: &str, label: &str) {
    assert_eq!(output.status.code(), Some(code), "{label}");
    assert_eq!(output.stdout, stdout.as_bytes(), "{label}");
    assert_eq!(output.stderr, stderr.as_bytes(), "{label}");
}

fn line(value: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(value).unwrap())
}

fn no_connection(listener: &UnixListener) -> Result<()> {
    listener.set_nonblocking(true)?;
    match listener.accept() {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => anyhow::bail!("unexpected extra control connection"),
    }
}

fn accept(listener: &UnixListener) -> Result<UnixStream> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((socket, _)) => {
                socket.set_nonblocking(false)?;
                socket.set_read_timeout(Some(Duration::from_secs(7)))?;
                return Ok(socket);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < until => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error).context("accept first control connection"),
        }
    }
}

#[test]
fn help_missing_identity_and_usage_never_connect() -> Result<()> {
    let root = root()?;
    let socket = root.path().join("s");
    let listener = UnixListener::bind(&socket)?;
    let expected = expected_help();
    compare(
        invoke(&["--help"], None, None)?,
        0,
        &format!("{expected}\n"),
        "",
        "help",
    );
    compare(
        invoke(&["--help"], Some(&socket), Some("test-token"))?,
        0,
        &format!("{expected}\n"),
        "",
        "help with available endpoint",
    );
    println!("PASS cli-help-no-identity");

    let missing = fixture("cli-missing-managed-identity");
    let expected = missing["expected_response"]["stderr"][0].as_str().unwrap();
    for (endpoint, token, label) in [
        (Some(socket.as_path()), None, "cli-missing-managed-identity"),
        (None, Some("test-token"), "cli-missing-endpoint"),
        (Some(socket.as_path()), Some(""), "cli-empty-token"),
    ] {
        compare(
            invoke(&["identify", "--json"], endpoint, token)?,
            4,
            "",
            &format!("{expected}\n"),
            label,
        );
        println!("PASS {label}");
    }
    compare(
        invoke(&["identify"], Some(&socket), None)?,
        4,
        "",
        "`mt` 仅在 Matou 托管终端中可用；当前进程缺少会话控制身份\n",
        "cli-missing-identity-text",
    );
    println!("PASS cli-missing-identity-text");
    compare(
        invoke(&["list", "--invalid"], Some(&socket), Some("test-token"))?,
        2,
        "",
        "未知选项：--invalid\n",
        "invalid option",
    );
    no_connection(&listener)?;
    println!("PASS cli-invalid-args-zero-connections");
    Ok(())
}

fn read_request(socket: &mut UnixStream) -> Result<Value> {
    let mut length = [0; 4];
    socket.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length);
    ensure!((1..=MAX_FRAME).contains(&length), "unexpected frame size");
    let mut body = vec![0; length as usize];
    socket.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

fn write_body(socket: &mut UnixStream, body: &[u8]) -> Result<()> {
    let prefix = (body.len() as u32).to_be_bytes();
    socket.write_all(&prefix[..2])?;
    socket.write_all(&prefix[2..])?;
    socket.write_all(body)?;
    Ok(())
}

fn with_peer(
    args: &[&str],
    method: &str,
    params: Value,
    reply: impl FnOnce(&mut UnixStream, &Value) -> Result<()> + Send,
) -> Result<Output> {
    let root = root()?;
    let path = root.path().join("s");
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    thread::scope(|scope| {
        let peer = scope.spawn(|| -> Result<()> {
            let mut socket = accept(&listener)?;
            let request = read_request(&mut socket)?;
            assert_eq!(request["version"], 1);
            let id = request["requestId"].as_str().unwrap();
            assert_eq!(uuid::Uuid::parse_str(id)?.get_version_num(), 4);
            assert_eq!(request["token"], "test-token");
            assert_eq!(request["method"], method);
            assert_eq!(request["params"], params);
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
            assert!((now + 4000..=now + 5000).contains(&request["deadlineAt"].as_i64().unwrap()));
            reply(&mut socket, &request)?;
            assert_eq!(
                socket.read(&mut [0])?,
                0,
                "client sent a second request on the same connection"
            );
            Ok(())
        });
        let output = invoke(args, Some(&path), Some("test-token"))?;
        peer.join().unwrap()?;
        no_connection(&listener)?;
        Ok(output)
    })
}

#[test]
fn frozen_client_response_envelopes_and_cli_stubs() -> Result<()> {
    for id in [
        "client-response-extra-fields",
        "client-response-no-result",
        "client-response-version-mismatch",
        "client-response-required-fields",
        "client-response-request-id-mismatch",
    ] {
        let case = fixture(id);
        let mut response = case["request"]["response"].clone();
        let expected = &case["expected_response"];
        let output = with_peer(
            &["identify", "--json"],
            "host.identify",
            json!({}),
            move |socket, request| {
                if id != "client-response-request-id-mismatch" {
                    response["requestId"] = request["requestId"].clone();
                }
                write_body(socket, &serde_json::to_vec(&response)?)
            },
        )?;
        if expected["status"] == "ok" {
            let stdout = expected
                .get("result")
                .map(line)
                .unwrap_or_else(|| "undefined\n".into());
            compare(output, 0, &stdout, "", id);
        } else {
            compare(output, 1, "", &line(&expected["error"]), id);
        }
        println!("PASS {id}");
    }

    let case = fixture("cli-discovery-modes");
    let expected = case["expected_response"]["stdout"][0].as_str().unwrap();
    let result = case["request"]["host_stubs"][0]["result"].clone();
    let output = with_peer(
        &["list", "--all", "--json"],
        "host.list",
        json!({"scope":"all"}),
        move |socket, request| {
            let response =
                json!({"version":1,"requestId":request["requestId"],"ok":true,"result":result});
            write_body(socket, &serde_json::to_vec(&response)?)
        },
    )?;
    compare(
        output,
        0,
        &format!("{expected}\n"),
        "",
        "cli-discovery-modes",
    );
    println!("PASS cli-discovery-modes");

    let case = fixture("cli-ambiguity-sanitized-details");
    let mut upstream = case["request"]["host_stubs"][1]["error"].clone();
    upstream["details"]["candidates"][0]["sessionId"] = json!("not-public");
    upstream["details"]["token"] = json!("not-public");
    let output = with_peer(
        &["list", "--json"],
        "host.list",
        json!({"scope":"current-level"}),
        move |socket, request| {
            let response =
                json!({"version":1,"requestId":request["requestId"],"ok":false,"error":upstream});
            write_body(socket, &serde_json::to_vec(&response)?)
        },
    )?;
    let expected = case["expected_response"]["stderr"][0].as_str().unwrap();
    compare(
        output,
        3,
        "",
        &format!("{expected}\n"),
        "cli-ambiguity-sanitized-details",
    );
    println!("PASS cli-ambiguity-sanitized-details");
    Ok(())
}

#[test]
fn connection_eof_malformed_oversize_and_timeout_are_terminal() -> Result<()> {
    let root = root()?;
    let endpoint = root.path().join("absent");
    let output = invoke(&["identify", "--json"], Some(&endpoint), Some("test-token"))?;
    compare(
        output,
        5,
        "",
        &line(&json!({
            "code": "CONNECTION_ERROR",
            "message": "未连接到 Matou Host Control；请在 Matou 托管终端中重试"
        })),
        "failed connection",
    );
    println!("PASS client-connect-failure");

    let eof = fixture("client-response-eof");
    let expected = line(&eof["expected_response"]["error"]);
    let output = with_peer(
        &["identify", "--json"],
        "host.identify",
        json!({}),
        |socket, _| {
            socket.write_all(&[0, 0])?;
            socket.shutdown(std::net::Shutdown::Write)?;
            Ok(())
        },
    )?;
    compare(output, 1, "", &expected, "client-response-eof");
    println!("PASS client-response-eof-partial-prefix");

    let output = with_peer(
        &["identify", "--json"],
        "host.identify",
        json!({}),
        |socket, _| write_body(socket, b"{"),
    )?;
    compare(output, 1, "", &expected, "client malformed response");
    println!("PASS client-response-malformed");
    let output = with_peer(
        &["identify", "--json"],
        "host.identify",
        json!({}),
        |socket, _| {
            socket.write_all(&(MAX_FRAME + 1).to_be_bytes())?;
            Ok(())
        },
    )?;
    compare(
        output,
        1,
        "",
        &line(&json!({
            "code": "INVALID_RESPONSE",
            "message": "Host Control 响应超过大小限制"
        })),
        "oversize response",
    );
    println!("PASS client-response-oversize");

    let timeout = fixture("client-response-timeout");
    let start = Instant::now();
    let output = with_peer(
        &["identify", "--json"],
        "host.identify",
        json!({}),
        |socket, _| {
            for _ in 0..3 {
                thread::sleep(Duration::from_secs(1));
                socket.write_all(&[0])?;
            }
            Ok(())
        },
    )?;
    assert!(start.elapsed() >= Duration::from_secs(5));
    compare(
        output,
        5,
        "",
        &line(&timeout["expected_response"]["error"]),
        "client-response-timeout",
    );
    println!("PASS client-response-timeout-trickle-with-no-retry");
    Ok(())
}

#[test]
fn request_limit_and_interrupted_connection() -> Result<()> {
    let root = root()?;
    let endpoint = root.path().join("s");
    let listener = UnixListener::bind(&endpoint)?;
    let client = Client::new(&endpoint, "test-token").unwrap();
    let error = client
        .request(
            "host.identify",
            json!({"padding": "x".repeat(MAX_FRAME as usize)}),
        )
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(error)?,
        json!({
            "code": "INVALID_REQUEST",
            "message": "Host Control 请求超过大小限制"
        })
    );
    no_connection(&listener)?;
    println!("PASS client-request-oversize-zero-connections");

    let error = thread::scope(|scope| -> Result<_> {
        let peer = scope.spawn(|| -> Result<()> {
            let mut socket = accept(&listener)?;
            // Disconnect while the sender still has buffered bytes to write.
            socket.read_exact(&mut [0])?;
            Ok(())
        });
        let result = client.request("host.identify", json!({"padding": "x".repeat(1024 * 1024)}));
        peer.join().unwrap()?;
        Ok(result.unwrap_err())
    })?;
    let case = fixture("client-connection-interrupted");
    assert_eq!(
        serde_json::to_value(error)?,
        case["expected_response"]["error"]
    );
    no_connection(&listener)?;
    println!("PASS client-connection-interrupted");
    Ok(())
}

#[test]
fn terminal_cli_requests_formats_and_exit_codes() -> Result<()> {
    for (args, method, params, result, stdout) in [
        (
            vec!["read", "self", "--lines", "1", "--bytes", "1048576"],
            "terminal.read-current",
            json!({"target":{"kind":"self"}, "maxLines":1,"maxBytes":1_048_576}),
            json!({"text":"尾🙂","cols":80,"rows":24,"truncated":true,"source":"screen"}),
            "尾🙂\n".into(),
        ),
        (
            vec!["history", "left", "--json"],
            "terminal.read-history",
            json!({"target":{"kind":"relative","direction":"left"}, "maxLines":1000,"maxBytes":65_536}),
            json!({"text":"saved", "truncated":false,"firstSequence":3,"lastSequence":8,"source":"journal"}),
            line(
                &json!({"text":"saved", "truncated":false,"firstSequence":3,"lastSequence":8,"source":"journal"}),
            ),
        ),
        (
            vec!["commands", "parent", "--limit", "5"],
            "terminal.read-commands",
            json!({"target":{"kind":"relation","relation":"parent"}, "limit":5}),
            json!(["pwd", {"command":"date"}, {"commandText":"ls"}]),
            "1. pwd\n2. date\n3. {\"commandText\":\"ls\"}\n".into(),
        ),
    ] {
        let output = with_peer(&args, method, params, move |socket, request| {
            write_body(
                socket,
                &serde_json::to_vec(&json!({
                    "version":1,"requestId":request["requestId"],"ok":true,"result":result
                }))?,
            )
        })?;
        compare(output, 0, &stdout, "", method);
    }

    let root = root()?;
    let endpoint = root.path().join("s");
    let listener = UnixListener::bind(&endpoint)?;
    for (args, message) in [
        (vec!["read", "--json"], "read 需要目标"),
        (
            vec!["read", "self", "--lines", "0", "--json"],
            "--lines 需要正整数",
        ),
        (
            vec!["history", "self", "--bytes", "1048577", "--json"],
            "--bytes 需要 1-1048576 的整数",
        ),
        (
            vec!["commands", "self", "--limit", "1001", "--json"],
            "--limit 需要 1-1000 的整数",
        ),
        (
            vec!["commands", "self", "--lines", "1", "--json"],
            "未知选项：--lines",
        ),
    ] {
        compare(
            invoke(&args, Some(&endpoint), Some("test-token"))?,
            2,
            "",
            &line(&json!({"code":"INVALID_REQUEST","message":message})),
            message,
        );
    }
    no_connection(&listener)?;

    for (code, exit) in [
        ("TARGET_NOT_FOUND", 3),
        ("STALE_PROJECTION", 3),
        ("AMBIGUOUS_TARGET", 3),
        ("CAPABILITY_DENIED", 4),
        ("TIMEOUT", 5),
        ("INTERNAL_ERROR", 1),
    ] {
        let error = json!({"code":code, "message":"public failure"});
        let expected = line(&error);
        let output = with_peer(
            &["commands", "self", "--json"],
            "terminal.read-commands",
            json!({"target":{"kind":"self"}, "limit":100}),
            move |socket, request| {
                write_body(
                    socket,
                    &serde_json::to_vec(&json!({
                        "version":1,"requestId":request["requestId"],"ok":false,"error":error
                    }))?,
                )
            },
        )?;
        compare(output, exit, "", &expected, code);
    }
    Ok(())
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn managed_launch(root: &Path, runtime: &Path, restart: bool) -> Result<PathBuf> {
    let data = root.join("data");
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let shell = root.join("shell");
    fs::write(
        &shell,
        r#"#!/bin/sh
command -v mt > "$HOME/mt-path"
mt identify --json > "$HOME/mt-result.tmp" 2> "$HOME/mt-error" || exit
printf '\033[31mmanaged-screen\033[0m'
for attempt in 1 2 3 4 5; do
  mt read self --json > "$HOME/screen.json" 2>> "$HOME/mt-error" || exit
  grep -q managed-screen "$HOME/screen.json" && break
  sleep 0.05
done
mt history self --json > "$HOME/history.json" 2>> "$HOME/mt-error" || exit
mt commands self --json > "$HOME/commands.json" 2>> "$HOME/mt-error" || exit
mv "$HOME/mt-result.tmp" "$HOME/mt-result"
"#,
    )?;
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
    let _ = fs::remove_file(home.join("mt-result"));
    let mut running = Running(
        Process::new(runtime)
            .env_clear()
            .env("MATOU_DISABLE_LIVE_EGRESS", "1")
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .arg("--data-root")
            .arg(&data)
            .arg("--workspace")
            .arg(root.join("workspace"))
            .arg("--shell-home")
            .arg(&home)
            .arg("--shell")
            .arg(&shell)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?,
    );
    let mut input = running.0.stdin.take().context("runtime stdin")?;
    let stdout = running.0.stdout.take().context("runtime stdout")?;
    let (send, receive) = mpsc::channel();
    let reader = thread::spawn(move || {
        for message in read_messages::<ServerMessage>(stdout) {
            if send.send(message).is_err() {
                break;
            }
        }
    });
    write_message(&mut input, &ClientMessage::Hello { version: VERSION })?;
    assert!(matches!(
        receive.recv_timeout(Duration::from_secs(8))??,
        ServerMessage::Hello { version: VERSION }
    ));
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        let remaining = until
            .checked_duration_since(Instant::now())
            .context("runtime Ready deadline")?;
        if let ServerMessage::Event(Event::Ready { hierarchy, .. }) =
            receive.recv_timeout(remaining)??
        {
            if restart {
                let session_id = hierarchy.session.context("restart the isolated Shell")?.id;
                write_message(
                    &mut input,
                    &ClientMessage::Command(Command::RestartSession { session_id }),
                )?;
            }
            break;
        }
    }
    while !home.join("mt-result").is_file() {
        ensure!(
            Instant::now() < until,
            "managed Shell mt did not finish: {}",
            fs::read_to_string(home.join("mt-error")).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(10));
    }
    let path = PathBuf::from(fs::read_to_string(home.join("mt-path"))?.trim_end());
    assert_eq!(path, data.join("control/bin/mt"));
    let result: Value = serde_json::from_slice(&fs::read(home.join("mt-result"))?)?;
    assert_eq!(result["caller"]["sessionId"], result["target"]["sessionId"]);
    assert_eq!(result["target"]["profile"], "shell");
    let screen: Value = serde_json::from_slice(&fs::read(home.join("screen.json"))?)?;
    let history: Value = serde_json::from_slice(&fs::read(home.join("history.json"))?)?;
    let commands: Value = serde_json::from_slice(&fs::read(home.join("commands.json"))?)?;
    assert_eq!(screen["text"], "managed-screen");
    assert_eq!(screen["source"], "screen");
    assert_eq!(screen["truncated"], false);
    assert!(screen["cols"].as_u64().is_some_and(|n| n > 0));
    assert!(screen["rows"].as_u64().is_some_and(|n| n > 0));
    assert!(
        history["text"]
            .as_str()
            .unwrap()
            .ends_with("\x1b[31mmanaged-screen\x1b[0m")
    );
    assert_eq!(history["source"], "journal");
    assert!(history["firstSequence"].as_u64().is_some_and(|n| n > 0));
    assert!(history["lastSequence"].as_u64() >= history["firstSequence"].as_u64());
    assert_eq!(commands, json!([]));
    write_message(&mut input, &ClientMessage::Command(Command::Shutdown))?;
    drop(input);
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = running.0.try_wait()? {
            assert!(status.success());
            break;
        }
        ensure!(Instant::now() < until, "runtime shutdown deadline");
        thread::sleep(Duration::from_millis(10));
    }
    reader.join().unwrap();
    Ok(path)
}

#[test]
fn shim_is_first_on_managed_path_and_refreshes_after_runtime_move() -> Result<()> {
    let root = root()?;
    let before = root.path().join("runtime 'before'");
    let after = root.path().join("runtime 'after'");
    fs::copy(RUNTIME, &before)?;
    let path = managed_launch(root.path(), &before, false)?;
    let original = fs::read_to_string(&path)?;
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(path.parent().unwrap())?.permissions().mode() & 0o777,
        0o700
    );
    assert!(!original.contains("MATOU_CONTROL_TOKEN"));
    let help = expected_help();
    compare(
        Process::new(&path).arg("--help").env_clear().output()?,
        0,
        &format!("{help}\n"),
        "",
        "shim help without identity",
    );
    println!("PASS managed-pty-path-shim-mode-and-help");

    fs::rename(&before, &after)?;
    assert_eq!(managed_launch(root.path(), &after, true)?, path);
    assert_ne!(fs::read_to_string(&path)?, original);
    compare(
        Process::new(&path).arg("--help").env_clear().output()?,
        0,
        &format!("{help}\n"),
        "",
        "shim help after move",
    );
    println!("PASS shim-refresh-after-runtime-move");
    Ok(())
}
