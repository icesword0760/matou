#![cfg(target_os = "macos")]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, ChildStdin, Command as Process, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use matou_native_runtime::{
    host::{Command, Event},
    protocol::{ClientMessage, ServerMessage, VERSION, read_messages, write_message},
};
use serde_json::{Value, json};

const READY_WAIT: Duration = Duration::from_secs(8);
const QUICK_WAIT: Duration = Duration::from_secs(3);
const TRACE_WAIT: Duration = Duration::from_secs(13);
// Two Anthropic, two OpenAI, and one Google host.
const TRACE_COUNT: usize = 5;

struct Runtime {
    child: Child,
    input: ChildStdin,
    messages: Receiver<serde_json::Result<ServerMessage>>,
    session_id: String,
}

impl Runtime {
    fn start(root: &Path, base: &str, env: &[(&str, &str)]) -> Result<Self> {
        let home = root.join("home");
        fs::create_dir_all(&home)?;
        let shell = home.join("shell");
        fs::write(
            &shell,
            "#!/bin/sh\nstty -echo\nwhile IFS= read -r line; do printf 'PTY:%s\\n' \"$line\"; done\n",
        )?;
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
        let mut child = Process::new(env!("CARGO_BIN_EXE_matou-native-runtime"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("MATOU_EGRESS_TRACE_BASE_URL", base)
            .envs(env.iter().copied())
            .arg("--data-root")
            .arg(root.join("data"))
            .arg("--workspace")
            .arg(root.join("work"))
            .arg("--shell-home")
            .arg(&home)
            .arg("--shell")
            .arg(&shell)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take().context("runtime stdin")?;
        let stdout = child.stdout.take().context("runtime stdout")?;
        let (send, messages) = mpsc::channel();
        thread::spawn(move || {
            for message in read_messages::<ServerMessage>(stdout) {
                if send.send(message).is_err() {
                    break;
                }
            }
        });
        let mut runtime = Self {
            child,
            input,
            messages,
            session_id: String::new(),
        };
        write_message(
            &mut runtime.input,
            &ClientMessage::Hello { version: VERSION },
        )?;
        assert!(matches!(
            runtime.messages.recv_timeout(READY_WAIT)??,
            ServerMessage::Hello { version: VERSION }
        ));
        runtime.session_id = runtime.wait(READY_WAIT, |event| match event {
            Event::Ready { hierarchy, .. } => hierarchy.session.map(|session| session.id),
            _ => None,
        })?;
        Ok(runtime)
    }

    fn send(&mut self, command: Command) -> Result<()> {
        write_message(&mut self.input, &ClientMessage::Command(command))
    }

    fn wait<T>(
        &mut self,
        timeout: Duration,
        mut accept: impl FnMut(Event) -> Option<T>,
    ) -> Result<T> {
        let deadline = Instant::now() + timeout;
        loop {
            let ServerMessage::Event(event) = self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))??
            else {
                continue;
            };
            if let Event::Frame {
                session_id,
                sequence,
                ..
            } = &event
            {
                self.send(Command::Applied {
                    session_id: session_id.clone(),
                    sequence: *sequence,
                })?;
            }
            let closed = matches!(event, Event::Closed);
            if let Some(result) = accept(event) {
                return Ok(result);
            }
            ensure!(!closed, "Runtime closed before the expected event");
        }
    }

    fn status(&mut self, kind: &str, request_id: u64, timeout: Duration) -> Result<Value> {
        self.wait(timeout, |event| match event {
            Event::Graph(graph) if graph["type"] == kind && graph["requestId"] == request_id => {
                Some(graph)
            }
            _ => None,
        })
        .with_context(|| format!("wait for {kind} {request_id}"))
        .and_then(|graph| {
            ensure!(graph["error"].is_null(), "{graph}");
            Ok(graph["result"].clone())
        })
    }

    fn shutdown(&mut self) -> Result<()> {
        let deadline = Instant::now() + QUICK_WAIT;
        self.send(Command::Shutdown)?;
        self.wait(QUICK_WAIT, |event| {
            matches!(event, Event::Closed).then_some(())
        })?;
        loop {
            if let Some(status) = self.child.try_wait()? {
                ensure!(status.success(), "Runtime exited with {status}");
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Runtime still running after shutdown"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn listener() -> Result<TcpListener> {
    let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    server.set_nonblocking(true)?;
    Ok(server)
}

fn accept_batch(server: &TcpListener, request_target: &str) -> Result<Vec<TcpStream>> {
    let mut sockets = Vec::new();
    let deadline = Instant::now() + QUICK_WAIT;
    while sockets.len() < TRACE_COUNT {
        let stream = match server.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                ensure!(
                    Instant::now() < deadline,
                    "Only {} trace requests arrived",
                    sockets.len()
                );
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        // Darwin can inherit the listener's nonblocking mode before curl has
        // written its request. Keep the timeout effective on the accepted stream.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(QUICK_WAIT))?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        assert_eq!(
            line.split_whitespace().nth(1),
            Some(request_target),
            "{line}"
        );
        loop {
            line.clear();
            let bytes = reader.read_line(&mut line)?;
            ensure!(bytes > 0, "Incomplete HTTP request");
            if line == "\r\n" {
                break;
            }
        }
        sockets.push(reader.into_inner());
    }
    Ok(sockets)
}

fn respond(sockets: Vec<TcpStream>, status: &str, body: &str) -> Result<()> {
    for mut stream in sockets {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
    }
    Ok(())
}

fn assert_exit(status: &Value, ip: &str, country: &str) {
    assert_eq!(status["families"].as_array().unwrap().len(), 3);
    for family in status["families"].as_array().unwrap() {
        assert_eq!(family["exit"], json!({"ip": ip, "country": country}));
        assert_eq!(family["hosts"][0]["ip"], ip);
        assert_eq!(family["hosts"][0]["country"], country);
        if family["family"] == "google" {
            assert_eq!(family["hosts"][1]["notApplicable"], true);
        } else {
            assert_eq!(family["hosts"][1]["ip"], ip);
        }
    }
}

fn assert_failure(status: &Value, error: &str) {
    assert_ne!(status["state"], "checking");
    for family in status["families"].as_array().unwrap() {
        assert!(family["exit"].is_null());
        for host in family["hosts"].as_array().unwrap() {
            if host["notApplicable"] != true {
                assert_eq!(host["error"], error, "{host}");
            }
        }
    }
}

#[test]
fn first_sample_updates_status_and_manual_recheck_gets_new_results() -> Result<()> {
    let server = listener()?;
    let address = server.local_addr()?;
    let base = format!("http://{address}");
    let root = tempfile::tempdir()?;
    let mut runtime = Runtime::start(
        root.path(),
        &base,
        &[
            ("HTTP_PROXY", "127.0.0.1:1"),
            ("NO_PROXY", &address.to_string()),
        ],
    )?;
    runtime.send(Command::GetEgressStatus { request_id: 1 })?;
    let batch = accept_batch(&server, "/cdn-cgi/trace")?;
    runtime.send(Command::Input {
        session_id: runtime.session_id.clone(),
        bytes: b"still-responsive\r".to_vec(),
        defer_ordering: false,
    })?;
    runtime.wait(QUICK_WAIT, |event| match event {
        Event::Frame { screen, .. } => screen
            .cells
            .chunks(screen.cols)
            .any(|cells| {
                cells
                    .iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
                    .contains("PTY:still-responsive")
            })
            .then_some(()),
        _ => None,
    })?;
    respond(batch, "200 OK", "fl=sample\r\nip=203.0.113.7\r\nloc=US\r\n")?;
    let first = runtime.status("egressStatusChanged", 0, QUICK_WAIT)?;
    assert_eq!(first["state"], "ok");
    assert_exit(&first, "203.0.113.7", "US");
    let local: Vec<_> = first["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["family"].is_null())
        .map(|check| {
            (
                check["id"].as_str().unwrap(),
                check["result"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        local,
        [
            ("agent-proxy-path", "pass"),
            ("browser-path", "pass"),
            ("time-zone", "pass"),
            ("ipv6", "pass")
        ]
    );
    assert_eq!(runtime.status("egressStatusResult", 1, QUICK_WAIT)?, first);
    assert!(first["checkedAt"].is_number());

    runtime.send(Command::RecheckEgress { request_id: 2 })?;
    respond(
        accept_batch(&server, "/cdn-cgi/trace")?,
        "500 Internal Server Error",
        "ip=203.0.113.9\nloc=US\n",
    )?;
    assert_failure(
        &runtime.status("egressRecheckResult", 2, QUICK_WAIT)?,
        "invalid",
    );

    runtime.send(Command::RecheckEgress { request_id: 3 })?;
    respond(
        accept_batch(&server, "/cdn-cgi/trace")?,
        "200 OK",
        "ip=2001:db8::1\nloc=CA\n",
    )?;
    let new = runtime.status("egressRecheckResult", 3, QUICK_WAIT)?;
    assert_exit(&new, "2001:db8::1", "CA");
    assert_eq!(new["state"], "abnormal");
    assert_eq!(new["families"][0]["violation"], "rule");
    runtime.send(Command::GetEgressStatus { request_id: 4 })?;
    assert_eq!(runtime.status("egressStatusResult", 4, QUICK_WAIT)?, new);
    runtime.shutdown()
}

#[test]
fn saving_time_zone_schedules_a_new_loopback_sample() -> Result<()> {
    let server = listener()?;
    let base = format!("http://{}", server.local_addr()?);
    let root = tempfile::tempdir()?;
    let mut runtime = Runtime::start(root.path(), &base, &[("TZ", "Asia/Shanghai")])?;
    let body = "ip=203.0.113.7\nloc=US\n";
    let time_zone_check = |status: &Value| {
        status["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == "time-zone")
            .unwrap()["result"]
            .clone()
    };
    respond(accept_batch(&server, "/cdn-cgi/trace")?, "200 OK", body)?;
    let auto = runtime.status("egressStatusChanged", 0, QUICK_WAIT)?;
    assert_eq!(time_zone_check(&auto), "pass");

    for (request_id, setting, expected) in [(1, "system", "warn"), (2, "UTC", "pass")] {
        runtime.send(Command::SetTimeZoneSettings {
            request_id,
            time_zone: json!(setting),
        })?;
        let saved = runtime.wait(QUICK_WAIT, |event| match event {
            Event::TimeZoneSettings(reply) if reply.request_id == request_id => Some(reply.result),
            _ => None,
        })?;
        assert_eq!(saved.unwrap().time_zone, setting);
        respond(accept_batch(&server, "/cdn-cgi/trace")?, "200 OK", body)?;
        let updated = runtime.status("egressStatusChanged", 0, QUICK_WAIT)?;
        assert_eq!(time_zone_check(&updated), expected);
    }
    runtime.shutdown()
}

#[test]
fn times_out_within_one_batch_and_shutdown_closes_pending_connections() -> Result<()> {
    let server = listener()?;
    let base = format!("http://{}", server.local_addr()?);
    let root = tempfile::tempdir()?;
    let mut runtime = Runtime::start(root.path(), &base, &[])?;
    let sockets = accept_batch(&server, "/cdn-cgi/trace")?;
    let started = Instant::now();
    let timed_out = runtime.status("egressStatusChanged", 0, TRACE_WAIT)?;
    assert!(started.elapsed() >= Duration::from_secs(8));
    assert_failure(&timed_out, "timeout");
    drop(sockets);

    runtime.send(Command::RecheckEgress { request_id: 1 })?;
    let pending = accept_batch(&server, "/cdn-cgi/trace")?;
    runtime.shutdown()?;
    for mut stream in pending {
        assert_eq!(stream.read(&mut [0; 1])?, 0);
    }
    Ok(())
}

#[test]
fn uses_http_proxy_and_respects_the_disable_flag() -> Result<()> {
    let proxy = listener()?;
    let origin = listener()?;
    let base = format!("http://{}", origin.local_addr()?);
    let proxy_url = proxy.local_addr()?.to_string();
    let root = tempfile::tempdir()?;
    let mut runtime = Runtime::start(
        root.path(),
        &base,
        &[("HTTP_PROXY", &proxy_url), ("ALL_PROXY", "127.0.0.1:1")],
    )?;
    respond(
        accept_batch(&proxy, &format!("{base}/cdn-cgi/trace"))?,
        "200 OK",
        "ip=203.0.113.7\nloc=US\n",
    )?;
    let status = runtime.status("egressStatusChanged", 0, QUICK_WAIT)?;
    assert_eq!(status["state"], "ok");
    assert_exit(&status, "203.0.113.7", "US");
    runtime.shutdown()?;

    let root = tempfile::tempdir()?;
    let mut disabled = Runtime::start(root.path(), &base, &[("MATOU_DISABLE_LIVE_EGRESS", "1")])?;
    disabled.send(Command::GetEgressStatus { request_id: 8 })?;
    assert_eq!(
        disabled.status("egressStatusResult", 8, QUICK_WAIT)?["state"],
        "checking"
    );
    disabled.shutdown()?;
    assert_eq!(
        origin.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}
