#![cfg(target_os = "macos")]

use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command as ProcessCommand, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use matou_native_core::{
    backup::{BackupReason, BackupService, DEFAULT_MAX_BACKUPS},
    startup::{self, BootstrapResult},
};
use matou_native_runtime::{
    host::{Command, Event},
    protocol::{ClientMessage, ServerMessage, VERSION, read_messages, write_message},
};
use matou_native_terminal::Screen;
use rusqlite::{Connection, OpenFlags};

const TIMEOUT: Duration = Duration::from_secs(8);
const PROMPT: &str = "MATOU_CHILD_READY>";

struct RuntimeChild {
    child: Child,
    input: Option<ChildStdin>,
    messages: Receiver<serde_json::Result<ServerMessage>>,
    data: PathBuf,
    workspace: PathBuf,
    task: String,
    session: String,
}

impl RuntimeChild {
    fn spawn(root: &Path) -> Result<Self> {
        let data = root.join("数据 空间");
        let workspace = root.join("工作 区");
        let home = root.join("Shell 家目录");
        fs::create_dir_all(&home)?;
        fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
        let mut child = ProcessCommand::new(env!("CARGO_BIN_EXE_matou-native-runtime"))
            .arg("--data-root")
            .arg(&data)
            .arg("--workspace")
            .arg(&workspace)
            .arg("--shell-home")
            .arg(&home)
            .args(["--shell", "/bin/zsh"])
            .env("HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take().context("child stdin")?;
        let stdout = child.stdout.take().context("child stdout")?;
        let (send, messages) = mpsc::channel();
        thread::spawn(move || {
            for message in read_messages::<ServerMessage>(stdout) {
                let failed = message.is_err();
                if send.send(message).is_err() || failed {
                    break;
                }
            }
        });
        println!(
            "spawned real runtime child={} root={}",
            child.id(),
            root.display()
        );
        Ok(Self {
            child,
            input: Some(input),
            messages,
            data,
            workspace,
            task: String::new(),
            session: String::new(),
        })
    }

    fn send(&mut self, command: Command) -> Result<()> {
        write_message(
            self.input.as_mut().context("open child stdin")?,
            &ClientMessage::Command(command),
        )
    }

    fn hello(&mut self) -> Result<()> {
        ensure!(
            !self.data.exists() && !self.workspace.exists(),
            "host started before Hello"
        );
        write_message(
            self.input.as_mut().context("stdin")?,
            &ClientMessage::Hello { version: VERSION },
        )?;
        let message = self
            .messages
            .recv_timeout(TIMEOUT)
            .context("server Hello timeout")??;
        ensure!(
            matches!(message, ServerMessage::Hello { version: VERSION }),
            "first reply must be Hello, not Ready"
        );
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.event(deadline, true)? {
                Event::Ready { hierarchy, .. } => {
                    self.task = hierarchy.task.context("Ready task")?.id;
                    self.session = hierarchy.session.context("Ready Shell")?.id;
                    ensure!(
                        self.data.join("matou.sqlite").is_file(),
                        "Ready without real DB"
                    );
                    break;
                }
                Event::Diagnostic(error) => bail!("host initialization: {error}"),
                Event::Closed => bail!("host closed before Ready"),
                _ => {}
            }
        }
        self.frame(|rows, _, _| rows.iter().any(|row| row == PROMPT))?;
        Ok(())
    }

    fn event(&mut self, deadline: Instant, acknowledge: bool) -> Result<Event> {
        let left = deadline
            .checked_duration_since(Instant::now())
            .context("child event deadline")?;
        let message = self
            .messages
            .recv_timeout(left)
            .context("child event read")??;
        let ServerMessage::Event(event) = message else {
            bail!("unexpected repeated server Hello")
        };
        if acknowledge
            && let Event::Frame {
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
        Ok(event)
    }

    fn input(&mut self, command: &str) -> Result<()> {
        self.send(Command::Input {
            session_id: self.session.clone(),
            bytes: format!("{command}\r").into_bytes(),
            defer_ordering: false,
        })
    }

    fn frame(&mut self, mut accept: impl FnMut(&[String], &Screen, u64) -> bool) -> Result<u64> {
        let deadline = Instant::now() + TIMEOUT;
        let mut last = Vec::new();
        loop {
            match self
                .event(deadline, true)
                .with_context(|| format!("last frame:\n{}", last.join("\n")))?
            {
                Event::Frame {
                    session_id,
                    screen,
                    sequence,
                    scroll_request_id,
                    ..
                } => {
                    ensure!(session_id == self.session, "unexpected Shell frame");
                    last = screen
                        .cells
                        .chunks(screen.cols)
                        .map(|cells| {
                            cells
                                .iter()
                                .map(|cell| cell.text.as_str())
                                .collect::<String>()
                                .trim_end_matches(' ')
                                .to_owned()
                        })
                        .collect();
                    if accept(&last, &screen, scroll_request_id) {
                        return Ok(sequence);
                    }
                }
                Event::Diagnostic(error) => bail!("host diagnostic: {error}"),
                Event::Closed => bail!("host closed while waiting for frame"),
                _ => {}
            }
        }
    }

    fn shell_pid(&mut self) -> Result<u32> {
        self.input("printf 'CHILD_PID=%s\\n' \"$$\"")?;
        let mut pid = None;
        self.frame(|rows, _, _| {
            pid = rows
                .iter()
                .find_map(|row| row.strip_prefix("CHILD_PID=")?.parse().ok());
            pid.is_some()
        })?;
        let pid = pid.context("Shell PID")?;
        println!(
            "runtime child={} owns real Shell pid={pid}",
            self.child.id()
        );
        Ok(pid)
    }

    fn closed(&mut self) -> Result<Vec<String>> {
        let deadline = Instant::now() + TIMEOUT;
        let mut diagnostics = Vec::new();
        loop {
            match self.event(deadline, false)? {
                Event::Closed => return Ok(diagnostics),
                Event::Diagnostic(message) => diagnostics.push(message),
                _ => {}
            }
        }
    }

    fn wait(&mut self) -> Result<(ExitStatus, String)> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait()? {
                // The child is reaped before reading stderr to EOF.
                let mut stderr = String::new();
                self.child
                    .stderr
                    .take()
                    .context("child stderr")?
                    .read_to_string(&mut stderr)?;
                println!(
                    "runtime child={} exited={status} stderr={stderr:?}",
                    self.child.id()
                );
                return Ok((status, stderr));
            }
            ensure!(Instant::now() < deadline, "real runtime child did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for RuntimeChild {
    fn drop(&mut self) {
        self.input.take();
        // Failure cleanup only, limited to the exact child this test spawned.
        // Passing scenarios must exit naturally through wait() above.
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.child.try_wait().ok().flatten().is_none() {
            if Instant::now() >= deadline {
                eprintln!(
                    "FAILED test cleanup: terminate owned child {}",
                    self.child.id()
                );
                let _ = self.child.kill();
                let _ = self.child.wait();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn assert_shell_gone(pid: u32) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    while ProcessCommand::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()?
        .status
        .success()
    {
        ensure!(Instant::now() < deadline, "child left Shell {pid} behind");
        thread::sleep(Duration::from_millis(20));
    }
    println!("verified real Shell pid={pid} exited");
    Ok(())
}

fn assert_runtime_released(data: &Path) -> Result<()> {
    ensure!(
        !data.join("matou.sqlite.owner").exists(),
        "runtime left its ownership record behind"
    );
    let BootstrapResult::Writable { .. } = startup::open(data, "process-contract-reopen")? else {
        bail!("runtime database did not reopen in writable mode");
    };
    println!("verified lease released and database reopened writable");
    Ok(())
}

fn assert_no_clean_exit_backup(data: &Path) -> Result<()> {
    let directory = data.join("backups");
    if directory.exists() {
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            ensure!(
                !path
                    .file_name()
                    .context("backup name")?
                    .to_string_lossy()
                    .contains("-clean-exit-"),
                "unexpected clean exit artifact: {}",
                path.display()
            );
        }
    }
    println!("verified no clean exit backup artifacts");
    Ok(())
}

#[test]
fn invalid_or_missing_hello_cannot_create_database_or_pty() -> Result<()> {
    for first in [
        b"{\"Hello\":{\"version\":2}}".as_slice(),
        b"{\"Command\":\"Refresh\"}".as_slice(),
        b"".as_slice(),
    ] {
        let root = tempfile::tempdir()?;
        let mut child = RuntimeChild::spawn(root.path())?;
        child.input.as_mut().context("stdin")?.write_all(first)?;
        child.input.take();
        let (status, stderr) = child.wait()?;
        ensure!(
            !status.success() && !stderr.is_empty(),
            "invalid Hello succeeded"
        );
        ensure!(
            !child.data.exists() && !child.workspace.exists(),
            "rejected Hello started host"
        );
        ensure!(
            matches!(
                child.messages.recv_timeout(TIMEOUT),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ),
            "invalid Hello wrote protocol output"
        );
    }
    Ok(())
}

#[test]
fn real_pipe_owner_pty_ack_scroll_fifo_and_explicit_shutdown() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    child.hello()?;
    let shell = child.shell_pid()?;
    let mut backups = BackupService::new(&child.data)?;
    let source = startup::open_read_only(&child.data.join("matou.sqlite"))?;
    for timestamp in 1..=8 {
        backups.create(&source, BackupReason::PreMigration, timestamp)?;
    }
    drop(source);
    let before = backups.list_valid()?;
    let tag = child.child.id();
    let started = format!("PIPE_中文_{tag}_开始");
    let finished = format!("PIPE_中文_{tag}_完成");
    child.input(&format!(
        "printf 'PIPE_%s\\n' '中文_{tag}_开始'; sleep 0.2; printf 'PIPE_%s\\n' '中文_{tag}_完成'"
    ))?;
    let first = child.frame(|rows, _, _| rows.contains(&started) && !rows.contains(&finished))?;
    let second = child.frame(|rows, _, _| {
        rows.contains(&started) && rows.contains(&finished) && rows.iter().any(|row| row == PROMPT)
    })?;
    ensure!(second > first, "output stopped after first ACK");
    child.input(
        "printf 'PIPE_SCROLL_%03d\\n' {0..99}; printf 'SCROLL_%s\\n' DONE; read scroll_hold",
    )?;
    child.frame(|rows, _, _| rows.iter().any(|row| row == "SCROLL_DONE"))?;
    let request_id = u64::MAX - 8;
    for command in [
        Command::ScrollTo {
            session_id: child.session.clone(),
            row: 0,
            request_id,
        },
        Command::Scroll {
            session_id: child.session.clone(),
            lines: i32::MIN,
        },
        Command::Input {
            session_id: child.session.clone(),
            bytes: "\rprintf 'FIFO_%s\\n' '中文_继续'\r".as_bytes().to_vec(),
            defer_ordering: false,
        },
    ] {
        child.send(command)?;
    }
    child.frame(|rows, screen, tag| {
        rows.iter().any(|row| row == "FIFO_中文_继续")
            && tag == request_id
            && screen.scrollbar.offset + screen.scrollbar.len >= screen.scrollbar.total
    })?;
    println!("verified FIFO ScrollTo tag={request_id} -> bottom -> real PTY input");
    let title = "退出前已提交的任务";
    child.send(Command::RenameTask {
        task_id: child.task.clone(),
        title: title.into(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.event(deadline, true)? {
            Event::TaskRenameResult {
                task_id,
                title: saved,
                error,
            } => {
                ensure!(
                    task_id == child.task && saved == title && error.is_none(),
                    "task rename failed: {error:?}"
                );
                break;
            }
            Event::Diagnostic(error) => bail!("host diagnostic: {error}"),
            Event::Closed => bail!("host closed before committing task rename"),
            _ => {}
        }
    }
    let mut shutdown_messages = Vec::new();
    for _ in 0..2 {
        write_message(
            &mut shutdown_messages,
            &ClientMessage::Command(Command::Shutdown),
        )?;
    }
    child
        .input
        .as_mut()
        .context("stdin")?
        .write_all(&shutdown_messages)?;
    // Keep parent's stdin open: exit must not wait for an unnecessary reader join.
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "explicit shutdown: {stderr}");
    assert_shell_gone(shell)?;
    let after = backups.list_valid()?;
    assert_eq!(after.len(), DEFAULT_MAX_BACKUPS);
    assert_eq!(after[0].reason, BackupReason::CleanExit);
    assert_eq!(after[1..], before[..DEFAULT_MAX_BACKUPS - 1]);
    let snapshot = root.path().join("standalone.sqlite");
    fs::copy(&after[0].path, &snapshot)?;
    let snapshot = Connection::open_with_flags(snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    assert_eq!(
        snapshot.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?,
        "ok"
    );
    assert_eq!(
        snapshot.query_row(
            "SELECT title FROM tasks WHERE id = ?",
            [&child.task],
            |row| row.get::<_, String>(0)
        )?,
        title
    );
    let summary = |database: &Connection| {
        database.query_row(
            "SELECT latest_lines_json, updated_at FROM session_graph_summaries WHERE session_id = ?",
            [&child.session],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
    };
    let source = startup::open_read_only(&child.data.join("matou.sqlite"))?;
    let saved_summary = summary(&snapshot)?;
    assert!(saved_summary.0.contains("FIFO_中文_继续"));
    assert_eq!(saved_summary, summary(&source)?);
    drop(source);
    println!(
        "verified one clean exit snapshot, newest seven retained, standalone SQLite contains committed rename and terminal summary"
    );
    assert_runtime_released(&child.data)?;
    Ok(())
}

#[test]
fn stdin_eof_releases_real_owner_and_shell_without_backup() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    child.hello()?;
    let shell = child.shell_pid()?;
    child.input.take();
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "EOF shutdown: {stderr}");
    assert_shell_gone(shell)?;
    assert_no_clean_exit_backup(&child.data)?;
    assert_runtime_released(&child.data)?;
    Ok(())
}

#[test]
fn repeated_hello_invalid_json_and_truncation_fail_after_real_pty_cleanup() -> Result<()> {
    for invalid in [
        b"{\"Hello\":{\"version\":1}}".as_slice(),
        b"{!}".as_slice(),
        b"{\"Command\":".as_slice(),
    ] {
        let root = tempfile::tempdir()?;
        let mut child = RuntimeChild::spawn(root.path())?;
        child.hello()?;
        let shell = child.shell_pid()?;
        let input = child.input.as_mut().context("stdin")?;
        input.write_all(invalid)?;
        input.flush()?;
        child.input.take();
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(
            !status.success()
                && (stderr.contains("decode native runtime command")
                    || stderr.contains("Hello must not be repeated")),
            "decode error hidden by Closed"
        );
        assert_shell_gone(shell)?;
        assert_no_clean_exit_backup(&child.data)?;
        assert_runtime_released(&child.data)?;
    }
    Ok(())
}

#[test]
fn read_only_backup_directory_fails_clean_shutdown_and_releases_database() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    child.hello()?;
    let shell = child.shell_pid()?;
    let directory = child.data.join("backups");
    fs::create_dir_all(&directory)?;
    let original_mode = fs::metadata(&directory)?.permissions();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))?;
    let result = (|| {
        child.send(Command::Shutdown)?;
        let diagnostics = child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(
            !status.success(),
            "unwritable backup directory reported success"
        );
        ensure!(
            stderr.contains("clean exit backup failed")
                && stderr.contains("unable to open database file"),
            "backup error chain lost: {stderr}"
        );
        ensure!(
            diagnostics
                .iter()
                .any(|message| message.contains("clean exit backup failed")
                    && message.contains("unable to open database file")),
            "backup failure not reported before Closed: {diagnostics:?}"
        );
        Ok(())
    })();
    fs::set_permissions(&directory, original_mode)?;
    result?;
    assert_shell_gone(shell)?;
    assert_no_clean_exit_backup(&child.data)?;
    assert_runtime_released(&child.data)?;
    Ok(())
}

#[test]
fn domain_error_after_ready_releases_real_owner_and_shell_without_backup() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    child.hello()?;
    let shell = child.shell_pid()?;
    // Make the next active-workspace check fail using only this test's database.
    let source = Connection::open(child.data.join("matou.sqlite"))?;
    let workspace = child.workspace.to_str().context("test workspace path")?;
    assert_eq!(
        source.execute(
            "UPDATE workspaces SET archived_at = 1 WHERE root_directory = ?",
            [workspace],
        )?,
        1
    );
    let diagnostics = child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(!status.success(), "domain error reported success");
    ensure!(
        stderr.contains("does not exist"),
        "domain error lost: {stderr}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("does not exist"))
    );
    assert_shell_gone(shell)?;
    assert_no_clean_exit_backup(&child.data)?;
    drop(source);
    assert_runtime_released(&child.data)?;
    Ok(())
}

#[test]
fn host_failure_is_not_successful_closed_and_does_not_join_blocked_stdin() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    fs::write(&child.data, b"not a directory")?;
    write_message(
        child.input.as_mut().context("stdin")?,
        &ClientMessage::Hello { version: VERSION },
    )?;
    ensure!(
        matches!(
            child.messages.recv_timeout(TIMEOUT)??,
            ServerMessage::Hello { version: VERSION }
        ),
        "Hello reply"
    );
    child.closed()?;
    // stdin stays open and reader blocked while the owner fails independently.
    let (status, stderr) = child.wait()?;
    ensure!(
        !status.success(),
        "Event::Closed incorrectly became success"
    );
    ensure!(
        stderr.contains("Not a directory (os error 20)")
            && !stderr.contains("forward native runtime command"),
        "host error lost precedence: {stderr}"
    );
    Ok(())
}
