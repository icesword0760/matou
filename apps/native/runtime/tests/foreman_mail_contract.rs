use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

fn runtime_binary() -> PathBuf {
    let cargo_binary = PathBuf::from(env!("CARGO_BIN_EXE_matou-native-runtime"));
    if cargo_binary.is_file() {
        return cargo_binary;
    }
    let runner = std::env::current_exe().expect("test executable path");
    let binary = format!("matou-native-runtime{}", std::env::consts::EXE_SUFFIX);
    runner
        .ancestors()
        .skip(1)
        .take(2)
        .map(|parent| parent.join(&binary))
        .find(|candidate| candidate.is_file())
        .expect("place the Runtime beside a copied test executable")
}

fn command(directory: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(runtime_binary());
    command
        .env_clear()
        .args(["--mt-foreman"])
        .args(args)
        .current_dir(directory)
        .env("PATH", directory.join("empty-path"))
        .env("HOME", directory)
        .env("USERPROFILE", directory)
        .env("CODEX_HOME", directory.join(".codex"))
        .env("FOREMAN_RUN_DIR", directory.join("任务 Runs"));
    #[cfg(windows)]
    for key in ["SystemRoot", "WINDIR", "COMSPEC", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}

fn create_running(root: &Path) -> Result<PathBuf> {
    let task = root.join("任务 Runs/等待");
    fs::create_dir_all(&task)?;
    fs::write(
        task.join("state.json"),
        serde_json::to_vec(&json!({
            "task": "等待", "agent": "codex", "status": "running",
            "supervisorPid": std::process::id(),
            "options": {"askWaitMin": 1}, "turns": [{"n": 1}]
        }))?,
    )?;
    Ok(task)
}

fn with_text(mut command: Command, body: &str) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(body.as_bytes())?;
    Ok(child.wait_with_output()?)
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    while !condition() {
        assert!(Instant::now() < deadline, "fixture did not finish");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn ask_answer_and_inbox_share_legacy_files_without_node_or_git() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fs::create_dir(root.join("empty-path"))?;
    let task = create_running(root)?;
    let mut question = command(root, &["ask", "等待", "--wait-min", "0.1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    question
        .stdin
        .take()
        .unwrap()
        .write_all("保留这个入口吗？".as_bytes())?;
    until(|| task.join("questions/q-1.json").is_file());
    let watch = command(root, &["watch", "等待", "--watch-min", "0.02"]).output()?;
    assert_eq!(watch.status.code(), Some(7));
    assert!(String::from_utf8_lossy(&watch.stdout).contains("保留这个入口吗？"));
    let answer = root.join("UTF-8 回答.md");
    fs::write(&answer, "保留")?;
    assert!(
        success(
            command(root, &["answer", "等待", "1", "--message"])
                .arg(answer)
                .output()?
        )
        .contains("#1")
    );
    assert_eq!(success(question.wait_with_output()?), "保留");
    assert!(task.join("read/answer-1.json").is_file());
    assert!(task.join("outcomes/q-1.json").is_file());
    let note = success(with_text(command(root, &["note", "等待"]), "补一条备注")?);
    assert!(note.contains("已留言"));
    assert!(success(command(root, &["inbox", "等待"]).output()?).contains("补一条备注"));
    assert!(success(command(root, &["inbox", "等待"]).output()?).contains("没有新的"));
    Ok(())
}

type Sends = Arc<Mutex<Vec<Value>>>;
const TRANSPORT_TIMEOUT: Duration = Duration::from_secs(3);

struct ControlFixture {
    endpoint: PathBuf,
    sends: Sends,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}

impl ControlFixture {
    fn start(root: &Path) -> Result<Self> {
        let endpoint = control_endpoint(root);
        let sends = Sends::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (started, ready) = mpsc::sync_channel(1);
        let client_endpoint = endpoint.clone();
        let client_sends = sends.clone();
        let client_stop = stop.clone();
        let worker = thread::spawn(move || {
            serve_control(&client_endpoint, &client_sends, &client_stop, started)
        });
        let mut fixture = Self {
            endpoint,
            sends,
            stop,
            worker: Some(worker),
        };
        if let Err(error) = ready.recv_timeout(TRANSPORT_TIMEOUT) {
            fixture.finish()?;
            return Err(error).context("mock control server did not start");
        }
        Ok(fixture)
    }

    fn finish(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("mock control server panicked")?;
        }
        Ok(())
    }
}

impl Drop for ControlFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn reply(body: &[u8], sends: &Sends) -> Result<Vec<u8>> {
    let request: Value = serde_json::from_slice(body)?;
    let result = match request["method"].as_str() {
        Some("host.identify") => {
            json!({"target": {"ref": "caller-reference", "title": "续跑卡片"}})
        }
        Some("host.list") => json!({"targets": [
            {"ref": "owner-reference", "workStatus": "idle"},
            {"ref": "caller-reference", "workStatus": "idle"},
        ], "projectionRevision": "revision-a"}),
        Some("terminal.send-text") => {
            sends.lock().unwrap().push(request["params"].clone());
            json!({})
        }
        other => bail!("unexpected request {other:?}"),
    };
    let encoded = serde_json::to_vec(&json!({
        "ok": true, "version": request["version"], "requestId": request["requestId"], "result": result
    }))?;
    let mut frame = (encoded.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&encoded);
    Ok(frame)
}

fn control_payload(length: [u8; 4]) -> Result<Vec<u8>> {
    let length = u32::from_be_bytes(length) as usize;
    ensure!(
        (1..=32_768).contains(&length),
        "unexpected request size {length}"
    );
    Ok(vec![0; length])
}

#[cfg(unix)]
fn control_endpoint(root: &Path) -> PathBuf {
    root.join("control.sock")
}

#[cfg(windows)]
fn control_endpoint(_: &Path) -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\matou-control-{}", uuid::Uuid::new_v4()))
}

#[cfg(unix)]
fn serve_control(
    endpoint: &Path,
    sends: &Sends,
    stop: &AtomicBool,
    started: mpsc::SyncSender<()>,
) -> Result<()> {
    use std::{io::Read, os::unix::net::UnixListener};

    let listener = UnixListener::bind(endpoint)?;
    listener.set_nonblocking(true)?;
    started.send(())?;
    while !stop.load(Ordering::Relaxed) {
        let (mut socket, _) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        socket.set_read_timeout(Some(TRANSPORT_TIMEOUT))?;
        socket.set_write_timeout(Some(TRANSPORT_TIMEOUT))?;
        let mut length = [0; 4];
        socket.read_exact(&mut length)?;
        let mut body = control_payload(length)?;
        socket.read_exact(&mut body)?;
        socket.write_all(&reply(&body, sends)?)?;
    }
    Ok(())
}

#[cfg(windows)]
fn serve_control(
    endpoint: &Path,
    sends: &Sends,
    stop: &AtomicBool,
    started: mpsc::SyncSender<()>,
) -> Result<()> {
    use tokio::{net::windows::named_pipe::ServerOptions, runtime::Builder, time::timeout};

    let runtime = Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let create = |first| {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create(endpoint)
        };
        let mut listener = create(true)?;
        started.send(())?;
        while !stop.load(Ordering::Relaxed) {
            let Ok(connected) = timeout(Duration::from_millis(20), listener.connect()).await else {
                continue;
            };
            connected?;
            let pipe = std::mem::replace(&mut listener, create(false)?);
            timeout(TRANSPORT_TIMEOUT, async {
                let mut length = [0; 4];
                pipe_read(&pipe, &mut length).await?;
                let mut body = control_payload(length)?;
                pipe_read(&pipe, &mut body).await?;
                pipe_write(&pipe, &reply(&body, sends)?).await?;
                // The client closes after reading its response. Keep this pipe alive until then.
                let _ = pipe_read(&pipe, &mut [0]).await;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .context("mock control pipe request timed out")??;
        }
        Ok(())
    })
}

#[cfg(windows)]
async fn pipe_read(
    pipe: &tokio::net::windows::named_pipe::NamedPipeServer,
    mut buffer: &mut [u8],
) -> io::Result<()> {
    while !buffer.is_empty() {
        pipe.readable().await?;
        match pipe.try_read(buffer) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => buffer = &mut buffer[read..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(windows)]
async fn pipe_write(
    pipe: &tokio::net::windows::named_pipe::NamedPipeServer,
    mut buffer: &[u8],
) -> io::Result<()> {
    while !buffer.is_empty() {
        pipe.writable().await?;
        match pipe.try_write(buffer) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => buffer = &buffer[written..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn fake_agent(directory: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let fixture = directory.join("Agent CLI");
    fs::write(
        &fixture,
        r#"#!/bin/sh
previous=
for arg; do
    if [ "$previous" = "-o" ]; then output="$arg"; fi
    previous="$arg"
done
if [ -z "$output" ]; then printf '%s\n' '{"models":[]}'; exit 0; fi
/bin/sleep 1
printf '%s\n' 'Foreman fixture reply.' > "$output"
printf '%s\n' '{"type":"thread.started","thread_id":"preserved-thread"}' '{"type":"turn.completed"}'
"#,
    )?;
    fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700))?;
    Ok(fixture)
}

#[cfg(windows)]
fn fake_agent(directory: &Path) -> Result<PathBuf> {
    let fixture = directory.join("Agent CLI.cmd");
    let script = r#"@echo off
setlocal DisableDelayedExpansion
:scan_args
if "%~1"=="" goto catalog
if "%~1"=="-o" goto run
shift
goto scan_args
:catalog
echo {"models":[]}
exit /b 0
:run
shift
set "output=%~1"
"%SystemRoot%\System32\ping.exe" -n 2 127.0.0.1 > nul
> "%output%" echo Foreman fixture reply.
echo {"type":"thread.started","thread_id":"preserved-thread"}
echo {"type":"turn.completed"}
exit /b 0
"#;
    fs::write(&fixture, script.replace('\n', "\r\n"))?;
    Ok(fixture)
}

#[test]
fn concurrent_resume_starts_one_round_and_notifies_only_the_original_session() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let task = root.join("任务 Runs/活动");
    let fixture_directory = root.join("模拟 Agent 中文目录");
    let workspace = root.join("含空格的 Workspace 中文");
    for dir in [
        &task,
        &fixture_directory,
        &workspace,
        &root.join("empty-path"),
    ] {
        fs::create_dir_all(dir)?;
    }
    fs::write(
        task.join("state.json"),
        serde_json::to_vec(&json!({
            "task": "活动", "agent": "codex", "status": "completed",
            "createdAt": "2026-11-25T03:00:00Z", "runStartedAt": "2026-11-25T03:00:00Z",
            "threadId": "preserved-thread", "exitCode": 0,
            "ownerSession": {"ref": "owner-reference", "title": "发起卡片"},
            "turns": [{"n": 1, "result": "completed", "untouched": true}],
            "options": {"cd": workspace, "model": "gpt-fixture", "effort": "high", "fullAccess": false,
                "addDirs": [], "retries": 0}
        }))?,
    )?;
    let fixture = fake_agent(&fixture_directory)?;
    let mut control = ControlFixture::start(root)?;
    let prompt = root.join("补充 交代.md");
    fs::write(&prompt, "继续验证")?;
    let invoke = |command_name| {
        let mut run = command(root, &[command_name, "活动", "--no-watch", "--message"]);
        run.arg(&prompt)
            .env("CODEX_BIN", &fixture)
            .env("MATOU_CONTROL_ENDPOINT", &control.endpoint)
            .env("MATOU_CONTROL_TOKEN", "local-fixture-control-token");
        run.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    };
    let first = invoke("send")?;
    let second = invoke("resume")?;
    let (first, second) = (first.wait_with_output()?, second.wait_with_output()?);
    assert_ne!(
        first.status.success(),
        second.status.success(),
        "send: {}\nresume: {}",
        String::from_utf8_lossy(&first.stderr),
        String::from_utf8_lossy(&second.stderr)
    );
    let state_path = task.join("state.json");
    let state = || serde_json::from_slice::<Value>(&fs::read(&state_path).unwrap()).unwrap();
    until(|| {
        let current = state();
        assert!(
            current["status"] != "failed",
            "continuation failed: {current}"
        );
        current["status"] == "completed" && current["turns"][1]["result"] == "completed"
    });
    until(|| !control.sends.lock().unwrap().is_empty());
    control.finish()?;
    let state = state();
    assert_eq!(state["turns"].as_array().unwrap().len(), 2);
    assert_eq!(state["turns"][0]["untouched"], true);
    assert_eq!(state["threadId"], "preserved-thread");
    assert_eq!(
        state["lastExternalSend"]["caller"]["ref"],
        "caller-reference"
    );
    let last = state["turns"][1]["files"]["last"]
        .as_str()
        .context("missing reply path")?;
    assert_eq!(fs::read_to_string(last)?.trim(), "Foreman fixture reply.");
    let sends = control.sends.lock().unwrap();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0]["target"]["ref"], "owner-reference");
    assert!(sends[0]["text"].as_str().unwrap().contains("续跑卡片"));
    Ok(())
}
