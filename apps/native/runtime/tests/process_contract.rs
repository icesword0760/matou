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
    scene: String,
    session: String,
}

impl RuntimeChild {
    fn spawn(root: &Path) -> Result<Self> {
        Self::spawn_with_zshrc(root, &format!("PROMPT='{PROMPT} '\n"))
    }

    fn spawn_with_zshrc(root: &Path, zshrc: &str) -> Result<Self> {
        Self::spawn_configured(root, zshrc, Path::new("/bin/zsh"), None)
    }

    fn spawn_configured(
        root: &Path,
        zshrc: &str,
        shell: &Path,
        provider: Option<&Path>,
    ) -> Result<Self> {
        Self::spawn_configured_with(root, zshrc, shell, provider, |_| {})
    }

    fn spawn_configured_with(
        root: &Path,
        zshrc: &str,
        shell: &Path,
        provider: Option<&Path>,
        configure: impl FnOnce(&mut ProcessCommand),
    ) -> Result<Self> {
        let data = root.join("数据 空间");
        let workspace = root.join("工作 区");
        let home = root.join("Shell 家目录");
        fs::create_dir_all(&home)?;
        fs::write(home.join(".zshrc"), zshrc)?;
        let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_matou-native-runtime"));
        command
            .arg("--data-root")
            .arg(&data)
            .arg("--workspace")
            .arg(&workspace)
            .arg("--shell-home")
            .arg(&home)
            .arg("--shell")
            .arg(shell)
            .env("HOME", &home)
            .env("CLAUDE_CONFIG_DIR", home.join("claude"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(provider) = provider {
            command.env("MATOU_CLAUDE_COMMAND", provider);
        }
        configure(&mut command);
        let mut child = command.spawn()?;
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
            scene: String::new(),
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
        self.ready()?;
        self.frame(|rows, _, _| rows.iter().any(|row| row == PROMPT))?;
        Ok(())
    }

    fn ready(&mut self) -> Result<()> {
        ensure!(
            !self.data.exists() && !self.workspace.exists(),
            "host started before Hello"
        );
        self.connect()
    }

    fn connect(&mut self) -> Result<()> {
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
                    self.scene = hierarchy.scene.context("Ready scene")?.id;
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
fn clean_shutdown_signals_both_shells_before_waiting() -> Result<()> {
    let root = tempfile::tempdir()?;
    let zshrc = format!(
        r#"PROMPT='{PROMPT} '
TRAPHUP() {{
    : > "$HOME/stopping-$$"
    local -a stopping
    until (( ${{#stopping}} == 2 )); do
        stopping=("$HOME"/stopping-*(N))
        /bin/sleep 0.01
    done
    printf '\nPARALLEL_SHUTDOWN_%s\n' "$$"
    exit 0
}}
printf '\nREADY_PID=%s\n' "$$"
"#
    );
    let mut child = RuntimeChild::spawn_with_zshrc(root.path(), &zshrc)?;
    child.hello()?;
    let first_pid = child.shell_pid()?;
    child.send(Command::CreateShellSibling {
        scene_id: child.scene.clone(),
        source_session_id: child.session.clone(),
        parent_session_id: None,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    let mut shells = std::collections::BTreeMap::from([(child.session.clone(), first_pid)]);
    while shells.len() < 2 {
        match child.event(deadline, true)? {
            Event::Frame {
                session_id, screen, ..
            } => {
                if let Some(pid) = screen.cells.chunks(screen.cols).find_map(|cells| {
                    let row: String = cells.iter().map(|cell| cell.text.as_str()).collect();
                    row.trim_end().strip_prefix("READY_PID=")?.parse().ok()
                }) {
                    shells.insert(session_id, pid);
                }
            }
            Event::Diagnostic(error) => bail!("host diagnostic: {error}"),
            Event::NavigationFailed { message, .. } => bail!("create sibling: {message}"),
            Event::Closed => bail!("host closed while waiting for both Shells"),
            _ => {}
        }
    }

    child.send(Command::Shutdown)?;
    let diagnostics = child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "clean shutdown: {stderr}");
    assert!(
        diagnostics.is_empty(),
        "shutdown diagnostics: {diagnostics:?}"
    );
    let backups: Vec<_> = BackupService::new(&child.data)?
        .list_valid()?
        .into_iter()
        .filter(|backup| backup.reason == BackupReason::CleanExit)
        .collect();
    assert_eq!(backups.len(), 1);
    let snapshot = Connection::open_with_flags(&backups[0].path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    for (session, pid) in shells {
        assert_shell_gone(pid)?;
        let summary: String = snapshot.query_row(
            "SELECT latest_lines_json FROM session_graph_summaries WHERE session_id = ?",
            [&session],
            |row| row.get(0),
        )?;
        // Sequential shutdown forces one Shell to exit before the other enters its HUP trap.
        assert!(
            summary.contains(&format!("PARALLEL_SHUTDOWN_{pid}")),
            "Shell {pid} did not observe both shutdown signals: {summary}"
        );
    }
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

#[test]
fn card_commands_retire_only_the_target_before_publishing_navigation() -> Result<()> {
    use matou_native_core::hierarchy::cards::RemoveScope;
    use serde_json::{Value, json};

    for scope in [
        Some(RemoveScope::NodeOnly),
        Some(RemoveScope::NodeAndDescendants),
        None,
    ] {
        let root = tempfile::tempdir()?;
        let mut child = RuntimeChild::spawn(root.path())?;
        child.hello()?;
        let survivor = child.session.clone();
        let survivor_pid = child.shell_pid()?;
        let marker = child.workspace.join("project-file");
        fs::write(&marker, "keep")?;
        child.send(Command::CreateShellSibling {
            scene_id: child.scene.clone(),
            source_session_id: survivor.clone(),
            parent_session_id: Some(survivor.clone()),
        })?;
        let deadline = Instant::now() + TIMEOUT;
        let target = loop {
            match child.event(deadline, true)? {
                Event::Navigation {
                    hierarchy, graph, ..
                } => {
                    let target = hierarchy.session.context("sibling focus")?.id;
                    assert_ne!(target, survivor);
                    assert_eq!(graph["focusedSessionId"], target);
                    break target;
                }
                Event::NavigationFailed { message, .. } | Event::Diagnostic(message) => {
                    bail!("{message}")
                }
                _ => {}
            }
        };
        loop {
            match child.event(deadline, true)? {
                Event::Frame {
                    session_id, screen, ..
                } if session_id == target && screen_has_line(&screen, PROMPT) => break,
                Event::Diagnostic(message) => bail!("{message}"),
                _ => {}
            }
        }

        let db = Connection::open_with_flags(
            child.data.join("matou.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let target_pid: i64 = db.query_row(
            "SELECT pid FROM session_runs WHERE session_id = ? AND status = 'running'",
            [&target],
            |row| row.get(0),
        )?;
        child.send(Command::CloseSessionCard {
            scene_id: child.scene.clone(),
            session_id: survivor.clone(),
            confirmed_intent: Some(format!("delete-session:{target}")),
            preserve_scene_on_last_session: false,
        })?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match child.event(deadline, true)? {
                Event::NavigationFailed { message, .. } => {
                    assert_eq!(message, "Session deletion intent is stale");
                    break;
                }
                Event::ScenesDisposed(ids) => bail!("failed close disposed {ids:?}"),
                Event::Diagnostic(message) => bail!("{message}"),
                _ => {}
            }
        }
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM session_runs WHERE status = 'running'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            2
        );

        child.send(match scope {
            Some(scope) => Command::RemoveSessionBranch {
                scene_id: child.scene.clone(),
                session_id: target.clone(),
                scope,
            },
            None => Command::CloseSessionCard {
                scene_id: child.scene.clone(),
                session_id: target.clone(),
                confirmed_intent: None,
                preserve_scene_on_last_session: false,
            },
        })?;
        // Submit traffic to the other Shell immediately, even if the worker
        // being removed is still finishing its journal.
        child.input("printf 'SURVIVOR_%s\\n' OK")?;
        let deadline = Instant::now() + TIMEOUT;
        let mut disposed = false;
        let mut published = false;
        let mut survivor_output = false;
        while !(published && survivor_output) {
            match child.event(deadline, true)? {
                Event::ScenesDisposed(ids) => {
                    assert_eq!(ids, [target.clone()]);
                    assert!(!disposed);
                    disposed = true;
                    assert!(
                        !ProcessCommand::new("ps")
                            .args(["-p", &target_pid.to_string(), "-o", "pid="])
                            .output()?
                            .status
                            .success(),
                        "target still alive at disposal event"
                    );
                }
                Event::Navigation {
                    hierarchy, graph, ..
                } => {
                    assert!(disposed, "graph published before disposal");
                    let action = &graph["sessionCardAction"];
                    assert_eq!(action["sessionId"], target);
                    assert_eq!(action["disposedSessionIds"], json!([target]));
                    assert_eq!(graph["focusedSessionId"], survivor);
                    assert_eq!(hierarchy.session.context("surviving focus")?.id, survivor);
                    if let Some(scope) = scope {
                        assert_eq!(action["action"], "remove-session-branch");
                        assert_eq!(action["scope"], serde_json::to_value(scope)?);
                        assert_eq!(action["removedSessionIds"], json!([target]));
                        assert_eq!(graph["nodes"].as_array().unwrap().len(), 1);
                    } else {
                        assert_eq!(action["action"], "close-session-card");
                        assert_eq!(action["outcome"], "scene-remains");
                        assert_eq!(action["rebuildCanvas"], false);
                        assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
                        assert!(graph["nodes"].as_array().unwrap().iter().any(|node| {
                            node["sessionId"] == target && !node["archivedAt"].is_null()
                        }));
                    }
                    published = true;
                }
                Event::Frame {
                    session_id, screen, ..
                } if session_id == survivor => {
                    survivor_output |= screen_has_line(&screen, "SURVIVOR_OK");
                }
                Event::NavigationFailed { message, .. } | Event::Diagnostic(message) => {
                    bail!("{message}")
                }
                _ => {}
            }
        }
        assert_eq!(
            db.query_row(
                "SELECT status FROM sessions WHERE id = ?",
                [&target],
                |row| row.get::<_, String>(0),
            )?,
            "archived"
        );
        assert_eq!(
            db.query_row(
                "SELECT status FROM session_runs WHERE session_id = ?",
                [&target],
                |row| row.get::<_, String>(0),
            )?,
            "exited"
        );
        assert_eq!(
            db.query_row(
                "SELECT pid FROM session_runs WHERE session_id = ? AND status = 'running'",
                [&survivor],
                |row| row.get::<_, u32>(0),
            )?,
            survivor_pid
        );
        assert_eq!(fs::read_to_string(&marker)?, "keep");

        let before = card_write_count(&db)?;
        child.send(Command::CloseSessionCard {
            scene_id: child.scene.clone(),
            session_id: survivor.clone(),
            confirmed_intent: Some(format!("delete-session:{survivor}")),
            preserve_scene_on_last_session: false,
        })?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match child.event(deadline, true)? {
                Event::Graph(graph) if !graph["sessionCardAction"].is_null() => {
                    let action: &Value = &graph["sessionCardAction"];
                    assert_eq!(action["outcome"], "hide-window");
                    assert_eq!(action["notice"]["confirmLabel"], "我知道了");
                    assert_eq!(action["disposedSessionIds"], json!([]));
                    assert_eq!(action["rebuildCanvas"], false);
                    assert_eq!(graph["focusedSessionId"], survivor);
                    break;
                }
                Event::ScenesDisposed(ids) => bail!("last card disposed {ids:?}"),
                Event::NavigationFailed { message, .. } | Event::Diagnostic(message) => {
                    bail!("{message}")
                }
                _ => {}
            }
        }
        assert_eq!(card_write_count(&db)?, before);
        assert_eq!(
            db.query_row(
                "SELECT archived_at FROM sessions WHERE id = ?",
                [&survivor],
                |row| row.get::<_, Option<i64>>(0),
            )?,
            None
        );

        child.send(Command::Shutdown)?;
        assert!(child.closed()?.is_empty());
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "runtime shutdown: {stderr}");
        assert_shell_gone(survivor_pid)?;
    }
    Ok(())
}

fn screen_has_line(screen: &Screen, expected: &str) -> bool {
    screen.cells.chunks(screen.cols).any(|cells| {
        let line: String = cells.iter().map(|cell| cell.text.as_str()).collect();
        line.trim_end() == expected
    })
}

fn card_write_count(db: &Connection) -> Result<(i64, i64)> {
    Ok(db.query_row(
        "SELECT (SELECT COUNT(*) FROM command_deduplication WHERE command_type = 'hierarchy.delete-session'),
                (SELECT COUNT(*) FROM domain_events WHERE event_type = 'session.archived')",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

fn seed_catalog(root: &Path, workspace: &Path, id: &str) -> Result<()> {
    let cwd = workspace.to_str().context("workspace UTF-8")?;
    let bucket_name: String = cwd
        .encode_utf16()
        .map(|unit| match unit {
            48..=57 | 65..=90 | 97..=122 => char::from(unit as u8),
            _ => '-',
        })
        .collect();
    let bucket = root.join("Shell 家目录/claude/projects").join(bucket_name);
    fs::create_dir_all(&bucket)?;
    fs::write(
        bucket.join(format!("{id}.jsonl")),
        serde_json::json!({
            "cwd": cwd, "timestamp": 1234,
            "message": {"role": "user", "content": "Synthetic command fixture"}
        })
        .to_string(),
    )?;
    Ok(())
}

#[test]
fn parked_startup_card_reads_its_binding_until_explicit_recovery() -> Result<()> {
    use matou_native_core::{
        hierarchy::canvas::{
            CanvasIds, CanvasMessages, CreateShellSiblingInput, create_shell_sibling,
        },
        sessions::list_runs,
        storage::CommandMetadata,
    };
    use matou_native_runtime::host::RecoveryState;

    let root = tempfile::tempdir()?;
    let no_shell = root.path().join("missing-shell");
    let mut seed = RuntimeChild::spawn_configured(root.path(), "", &no_shell, None)?;
    seed.ready()?;
    seed.send(Command::Shutdown)?;
    seed.closed()?;
    let (status, stderr) = seed.wait()?;
    ensure!(status.success(), "seed runtime shutdown: {stderr}");

    let mut db = Connection::open(seed.data.join("matou.sqlite"))?;
    let parked = &seed.session;
    for index in 0..19 {
        let card = if index == 0 {
            parked.clone()
        } else {
            format!("recovery-card-{index}")
        };
        let now = 1 + i64::from(index);
        if index != 0 {
            create_shell_sibling(
                &mut db,
                &CommandMetadata {
                    command_id: card.clone(),
                    command_type: "hierarchy.create-shell-sibling".into(),
                    request_hash: card.clone(),
                    causation_id: None,
                    correlation_id: None,
                },
                &CreateShellSiblingInput {
                    window_id: "main-window-1".into(),
                    scene_id: seed.scene.clone(),
                    source_session_id: parked.clone(),
                    parent_session_id: None,
                    execution_context_id: None,
                    title: None,
                    navigation: None,
                    now,
                },
                &CanvasIds {
                    scene_id: seed.scene.clone(),
                    session_id: card.clone(),
                    root_node_id: format!("node-{index}"),
                    secondary_node_id: format!("second-node-{index}"),
                    mount_id: format!("mount-{index}"),
                    relation_id: format!("relation-{index}"),
                },
                &CanvasMessages {
                    new_canvas: "Canvas",
                    duplicate_scene_name: "Duplicate",
                    workspace_path_invalid: "Workspace unavailable",
                    detached_session_must_return: "Card is detached",
                },
                now,
            )?;
        }
        if index == 2 {
            db.execute(
                "INSERT INTO session_runs
                    (id, session_id, ordinal, runtime_generation, profile, status, cols, rows, started_at, ended_at)
                 VALUES ('saved-shell-run', ?1, 1, 'older-runtime', 'shell', 'interrupted', 80, 24, ?2, ?2)",
                (&card, now),
            )?;
            db.execute(
                "UPDATE sessions SET status='interrupted', work_status='interrupted' WHERE id=?1",
                [&card],
            )?;
            continue;
        }
        db.execute(
            "UPDATE sessions SET kind='claude-code', created_at=?1, last_activity_at=?1 WHERE id=?2",
            (now, &card),
        )?;
        db.execute(
            "INSERT INTO provider_bindings
                (id, session_id, provider, provider_session_id, resume_state, metadata_json,
                 created_at, updated_at, validated_at)
             VALUES (?1, ?2, 'claude-code', ?3, 'available', '{}', ?4, ?4, ?4)",
            (
                format!("binding-{index}"),
                &card,
                format!("bound-{index}"),
                now,
            ),
        )?;
    }
    seed_catalog(root.path(), &seed.workspace, "bound-0")?;
    seed_catalog(root.path(), &seed.workspace, "unrelated-conversation")?;
    let stub = root.path().join("provider-stub");
    fs::write(
        &stub,
        "#!/bin/sh\nwhile [ $# -gt 1 ]; do\n  if [ \"$1\" = '--resume' ] && [ \"$2\" = 'bound-0' ]; then\n    printf 'parked binding resumed\\n'\n    exec /bin/sleep 60\n  fi\n  shift\ndone\nexit 3\n",
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let before = list_runs(&db, parked)?.len();
    let mut child = RuntimeChild::spawn_configured(root.path(), "", &no_shell, Some(&stub))?;
    child.connect()?;
    let mut parked_ids = std::collections::HashSet::new();
    let deadline = Instant::now() + TIMEOUT;
    while parked_ids.len() < 3 {
        if let Event::Recovery {
            session_id,
            state: RecoveryState::Deferred,
        } = child.event(deadline, true)?
        {
            parked_ids.insert(session_id);
        }
    }
    assert!(parked_ids.contains(parked));
    assert!(parked_ids.contains("recovery-card-1"));
    assert!(parked_ids.contains("recovery-card-2"));
    assert_eq!(list_runs(&db, "recovery-card-2")?.len(), 1);
    for (card, expected) in [(parked.as_str(), true), ("recovery-card-1", false)] {
        child.send(Command::GetDeferredRecoveryPreview {
            session_id: card.into(),
        })?;
        loop {
            match child.event(deadline, true)? {
                Event::DeferredRecoveryPreview {
                    session_id,
                    cwd,
                    preview,
                } if session_id == card => {
                    assert_eq!(Path::new(&cwd), seed.workspace);
                    assert_eq!(preview.is_some(), expected);
                    if let Some(preview) = preview {
                        assert_eq!(preview.title.as_deref(), Some("Synthetic command fixture"));
                        assert_eq!(preview.events[0].text, "Synthetic command fixture");
                        assert_eq!(preview.events[0].role.as_deref(), Some("user"));
                    }
                    break;
                }
                Event::Frame { session_id, .. } if session_id == card => {
                    bail!("parked PTY already produced a frame")
                }
                _ => {}
            }
        }
    }
    child.send(Command::GetProviderSession {
        cwd: seed.workspace.to_string_lossy().into_owned(),
        provider_session_id: "bound-0".into(),
        start_event_index: None,
        limit: None,
    })?;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(history) = graph["providerSessionDetailResults"].get(0)
        {
            assert_eq!(history["providerSessionId"], "bound-0");
            assert_eq!(history["events"][0]["text"], "Synthetic command fixture");
            let card = graph["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|node| node["sessionId"].as_str() == Some(parked))
                .unwrap();
            assert_eq!(card["historyProviderSessionId"], "bound-0");
            break;
        }
    }
    assert_eq!(list_runs(&db, parked)?.len(), before);
    child.send(Command::RetryRecovery {
        session_id: parked.clone(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Frame {
            session_id, screen, ..
        } = child.event(deadline, true)?
            && &session_id == parked
            && screen_has_line(&screen, "parked binding resumed")
        {
            break;
        }
    }
    assert_eq!(list_runs(&db, parked)?.len(), before + 1);
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");
    Ok(())
}

fn permission_mode_result(child: &mut RuntimeChild, mode: &str) -> Result<serde_json::Value> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(result) = graph["providerPermissionModeResults"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|result| {
                    result["permissionMode"] == mode && result["sessionId"] == child.session
                })
        {
            return Ok(result.clone());
        }
    }
}

fn post_permission_identity(settings_path: &Path, provider_id: &str, mode: &str) -> Result<()> {
    let settings: serde_json::Value = serde_json::from_slice(&fs::read(settings_path)?)?;
    let url = settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["url"]
        .as_str()
        .context("test hook URL")?
        .strip_prefix("http://")
        .context("HTTP hook")?;
    let (address, path) = url.split_once('/').context("hook path")?;
    let payload = serde_json::to_string(&serde_json::json!({
        "hook_event_name": "SessionStart", "session_id": provider_id,
        "permission_mode": mode, "model": "fixture-model"
    }))?;
    let mut connection = std::net::TcpStream::connect(address)?;
    connection.set_read_timeout(Some(TIMEOUT))?;
    write!(
        connection,
        "POST /{path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    )?;
    let mut response = String::new();
    connection.read_to_string(&mut response)?;
    ensure!(
        response.starts_with("HTTP/1.1 200"),
        "identity hook was rejected"
    );
    Ok(())
}

fn wait_permission_identity(child: &mut RuntimeChild) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && graph["providerIdentityEvents"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|event| event["result"] == "confirmed" && event["sessionId"] == child.session)
        {
            return Ok(());
        }
    }
}

fn spawn_fresh_permission_card(
    root: &Path,
    stub: &Path,
    unusable: Option<&str>,
    permission_startup_window_ms: Option<u64>,
) -> Result<RuntimeChild> {
    let missing_shell = root.join("missing-shell");
    let mut seed = RuntimeChild::spawn_configured(root, "", &missing_shell, None)?;
    seed.ready()?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Recovery { .. } = seed.event(deadline, true)? {
            break;
        }
    }
    seed.send(Command::Shutdown)?;
    seed.closed()?;
    let (status, stderr) = seed.wait()?;
    ensure!(status.success(), "seed runtime shutdown: {stderr}");
    let db = Connection::open(seed.data.join("matou.sqlite"))?;
    db.execute(
        "UPDATE sessions SET kind='claude-code' WHERE id=?",
        [&seed.session],
    )?;
    if let Some(state) = unusable {
        db.execute(
            "INSERT INTO provider_bindings
                (id, session_id, provider, provider_session_id, resume_state, restore_state,
                 restore_error, metadata_json, created_at, updated_at, invalidated_at, validated_at)
             VALUES ('old-binding', ?, 'claude-code', 'old-id', ?, ?, ?,
                 '{\"permissionMode\":\"acceptEdits\"}', 1, 1, ?, 1)",
            rusqlite::params![
                seed.session,
                if state == "failed" {
                    "available"
                } else {
                    state
                },
                if state == "failed" { state } else { "none" },
                (state == "failed").then_some("previous restore failure"),
                (state != "failed").then_some(1),
            ],
        )?;
    }
    let mut child = RuntimeChild::spawn_configured_with(
        root,
        "",
        Path::new("/bin/zsh"),
        Some(stub),
        |command| {
            if let Some(millis) = permission_startup_window_ms {
                command.env("MATOU_PERMISSION_STARTUP_WINDOW_MS", millis.to_string());
            }
        },
    )?;
    child.connect()?;
    assert_eq!(child.session, seed.session);
    Ok(child)
}

#[test]
fn fresh_permission_respawn_expires_unusable_bindings_and_coalesces() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, list_runs};
    for unusable in [None, Some("expired"), Some("failed")] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("claude-fresh-permission-stub");
        fs::write(
            &stub,
            r#"#!/bin/sh
printf '%s\n' "$@" > "$0.argv"
printf '%s' "$2" > "$0.settings"
case "$*" in *'--permission-mode plan'*) printf 'plan ready\n';;
  *) printf 'initial ready\n';;
esac
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let mut child = spawn_fresh_permission_card(root.path(), &stub, unusable, None)?;
        if unusable != Some("failed") {
            child.frame(|_, screen, _| screen_has_line(screen, "initial ready"))?;
        }
        let session = child.session.clone();
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        let original_runs = list_runs(&db, &session)?;
        assert_eq!(original_runs.len(), usize::from(unusable != Some("failed")));
        let set = |mode: &str| Command::SetProviderPermissionMode {
            session_id: session.clone(),
            mode: mode.into(),
        };
        child.send(set("plan"))?;
        child.send(set("plan"))?;
        child.send(set("auto"))?;
        assert!(
            permission_mode_result(&mut child, "auto")?["error"]
                .as_str()
                .unwrap()
                .contains("stopping")
        );
        child.frame(|_, screen, _| screen_has_line(screen, "plan ready"))?;
        let argv = fs::read_to_string(stub.with_extension("argv"))?;
        assert!(
            argv.lines()
                .collect::<Vec<_>>()
                .ends_with(&["--permission-mode", "plan"])
        );
        assert!(!argv.lines().any(|argument| argument == "--resume"));
        if unusable.is_some() {
            let old = db.query_row(
                "SELECT resume_state, restore_state, restore_error IS NULL, invalidated_at IS NOT NULL,
                        json_extract(metadata_json, '$.permissionMode')
                 FROM provider_bindings WHERE id='old-binding'",
                [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                              row.get::<_, bool>(2)?, row.get::<_, bool>(3)?,
                              row.get::<_, String>(4)?)),
            )?;
            assert_eq!(
                old,
                (
                    "expired".into(),
                    "none".into(),
                    true,
                    true,
                    "acceptEdits".into()
                )
            );
        } else {
            assert!(claude_binding(&db, &session)?.is_none());
        }
        post_permission_identity(
            Path::new(&fs::read_to_string(stub.with_extension("settings"))?),
            "fresh-id",
            "default",
        )?;
        let result = permission_mode_result(&mut child, "plan")?;
        assert!(result.get("error").is_none(), "{result}");
        let binding = claude_binding(&db, &session)?.context("new binding")?;
        assert_eq!(binding.provider_session_id, "fresh-id");
        assert_eq!(binding.metadata["permissionMode"], "plan");
        assert!(binding.resumable);
        let runs = list_runs(&db, &session)?;
        assert_eq!(runs.len(), original_runs.len() + 1);
        let new_run = runs.last().context("fresh run")?;
        if let Some(first) = original_runs.first() {
            assert_eq!(runs[0].id, first.id);
            assert_eq!(runs[0].status, "exited");
            assert!(runs[0].ended_at.unwrap() <= new_run.started_at);
            assert_shell_gone(first.pid.context("original process")? as u32)?;
        }
        child.send(set("plan"))?;
        assert!(
            permission_mode_result(&mut child, "plan")?
                .get("error")
                .is_none()
        );
        assert_eq!(list_runs(&db, &session)?.len(), runs.len());
        child.send(Command::Shutdown)?;
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "runtime shutdown: {stderr}");
    }
    Ok(())
}

#[test]
fn fresh_permission_respawn_failure_does_not_record_an_identity() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, list_runs};
    for unusable in [None, Some("failed")] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("claude-fresh-permission-stub");
        fs::write(
            &stub,
            r#"#!/bin/sh
case "$*" in
  *'--permission-mode auto'*)
    url=$(/usr/bin/plutil -extract hooks.UserPromptSubmit.0.hooks.0.url raw -o - "$2") || exit 91
    /usr/bin/curl --silent --fail --noproxy '*' --max-time 3 -H 'content-type: application/json' \
      --data-binary '{"hook_event_name":"SessionStart","session_id":"early-id"}' "$url" || exit 92
    exit 23
    ;;
  *'--permission-mode plan'*)
    printf '%s' "$2" > "$0.settings"
    printf 'plan ready\n'
    ;;
  *) printf 'initial ready\n' ;;
esac
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let startup_window_ms = unusable.is_none().then_some(3000);
        let mut child =
            spawn_fresh_permission_card(root.path(), &stub, unusable, startup_window_ms)?;
        let session = child.session.clone();
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        let (mode, expected_error) = if unusable.is_some() {
            ("plan", "returned a previous conversation ID")
        } else {
            child.frame(|_, screen, _| screen_has_line(screen, "initial ready"))?;
            ("auto", "process exited with code 23")
        };
        child.send(Command::SetProviderPermissionMode {
            session_id: session.clone(),
            mode: mode.into(),
        })?;
        if unusable.is_some() {
            child.frame(|_, screen, _| screen_has_line(screen, "plan ready"))?;
            post_permission_identity(
                Path::new(&fs::read_to_string(stub.with_extension("settings"))?),
                "old-id",
                "plan",
            )?;
        }
        let result = permission_mode_result(&mut child, mode)?;
        assert!(
            result["error"].as_str().unwrap().contains(expected_error),
            "{result}"
        );
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM provider_bindings WHERE session_id = ?",
            [&session],
            |row| row.get(0),
        )?;
        assert_eq!(count, i64::from(unusable.is_some()));
        if unusable.is_some() {
            let old = claude_binding(&db, &session)?.context("expired binding")?;
            assert_eq!(old.id, "old-binding");
            assert_eq!(old.restore_state, "none");
            assert!(!old.resumable);
            assert_eq!(old.metadata["permissionMode"], "acceptEdits");
        } else {
            assert_eq!(
                list_runs(&db, &session)?.last().unwrap().exit_code,
                Some(23)
            );
        }
        child.send(Command::Shutdown)?;
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "runtime shutdown: {stderr}");
    }
    Ok(())
}

#[test]
fn permission_respawn_validates_coalesces_blocks_input_and_rolls_back_an_early_exit() -> Result<()>
{
    use matou_native_core::sessions::{claude_binding, list_runs};
    let root = tempfile::tempdir()?;
    let stub = root.path().join("claude-permission-stub");
    let ready_settings = stub.with_extension("settings");
    let received_input = stub.with_extension("input");
    fs::write(
        &stub,
        r#"#!/bin/sh
case "$*" in *'--permission-mode auto'*) exit 23;; esac
printf '%s' "$2" > "$0.settings"
printf 'stub ready\n'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$0.input"
  printf 'input received\n'
done
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut child = RuntimeChild::spawn_configured(
        root.path(),
        &format!("PROMPT='{PROMPT} '\n"),
        Path::new("/bin/zsh"),
        Some(&stub),
    )?;
    child.hello()?;
    let session = child.session.clone();
    let set = |mode: &str| Command::SetProviderPermissionMode {
        session_id: session.clone(),
        mode: mode.into(),
    };
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    child.send(set("invalid"))?;
    assert_eq!(
        permission_mode_result(&mut child, "invalid")?["error"],
        "invalid permission mode"
    );
    child.send(set("plan"))?;
    assert_eq!(
        permission_mode_result(&mut child, "plan")?["error"],
        "active AI Session is required for permission respawn"
    );
    assert_eq!(list_runs(&db, &session)?.len(), 1);
    assert!(claude_binding(&db, &session)?.is_none());

    let provider_id = "permission-fixture-id";
    seed_catalog(root.path(), &child.workspace, provider_id)?;
    child.send(Command::LoadProviderSession {
        session_id: session.clone(),
        provider_session_id: provider_id.into(),
    })?;
    child.frame(|_, screen, _| screen_has_line(screen, "stub ready"))?;
    child.send(set("plan"))?;
    assert!(
        permission_mode_result(&mut child, "plan")?["error"]
            .as_str()
            .unwrap()
            .contains("recovering")
    );
    post_permission_identity(
        Path::new(&fs::read_to_string(&ready_settings)?),
        provider_id,
        "default",
    )?;
    wait_permission_identity(&mut child)?;
    let source = claude_binding(&db, &session)?.context("active binding")?;
    db.execute(
        "UPDATE provider_bindings SET metadata_json = json_set(metadata_json, '$.custom', json('[1,2]'))
         WHERE id = ?",
        [&source.id],
    )?;
    child.send(set("default"))?;
    assert!(
        permission_mode_result(&mut child, "default")?
            .get("error")
            .is_none()
    );
    assert_eq!(list_runs(&db, &session)?.len(), 2);

    child.send(set("plan"))?;
    child.send(set("plan"))?;
    child.send(set("bypassPermissions"))?;
    assert!(
        permission_mode_result(&mut child, "bypassPermissions")?["error"]
            .as_str()
            .unwrap()
            .contains("stopping")
    );
    child.frame(|_, screen, _| screen_has_line(screen, "stub ready"))?;
    child.send(Command::Input {
        session_id: session.clone(),
        bytes: b"must be discarded\r".to_vec(),
        defer_ordering: false,
    })?;
    post_permission_identity(
        Path::new(&fs::read_to_string(&ready_settings)?),
        provider_id,
        "plan",
    )?;
    assert!(
        permission_mode_result(&mut child, "plan")?
            .get("error")
            .is_none()
    );
    let runs = list_runs(&db, &session)?;
    assert_eq!(runs.len(), 3);
    assert!(runs[1].ended_at.unwrap() <= runs[2].started_at);
    assert_shell_gone(runs[1].pid.unwrap() as u32)?;
    child.send(Command::Input {
        session_id: session.clone(),
        bytes: b"after confirmation\r".to_vec(),
        defer_ordering: false,
    })?;
    child.frame(|_, screen, _| screen_has_line(screen, "input received"))?;
    assert_eq!(fs::read_to_string(received_input)?, "after confirmation\n");

    let previous = claude_binding(&db, &session)?.context("plan binding")?;
    assert_eq!(previous.id, source.id);
    assert_eq!(previous.metadata["permissionMode"], "plan");
    assert_eq!(previous.metadata["custom"], serde_json::json!([1, 2]));
    child.send(set("auto"))?;
    let failure = permission_mode_result(&mut child, "auto")?;
    assert!(
        failure["error"]
            .as_str()
            .unwrap()
            .contains("process exited with code 23"),
        "{failure}"
    );
    let restored = claude_binding(&db, &session)?.context("rolled back binding")?;
    assert_eq!(restored.id, source.id);
    assert_eq!(restored.metadata, previous.metadata);
    assert_eq!(restored.restore_state, previous.restore_state);
    let runs = list_runs(&db, &session)?;
    assert_eq!(runs.len(), 4);
    assert_eq!(runs[3].exit_code, Some(23));
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");
    Ok(())
}

#[test]
fn permission_respawn_passes_only_cli_consent_keys_and_keeps_the_mode_when_declined() -> Result<()>
{
    use matou_native_core::sessions::{claude_binding, list_runs};
    use matou_native_runtime::host::RecoveryState;
    const PROVIDER_ID: &str = "permission-consent-id";
    for accept in [true, false] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("claude-permission-stub");
        fs::write(
            &stub,
            r#"#!/bin/sh
printf '%s' "$2" > "$0.settings"
case "$*" in
  *--dangerously-skip-permissions*)
    printf 'WARNING: Claude Code running in Bypass Permissions mode\n'
    printf 'No, exit\nYes, I accept\nEnter to confirm · Esc to cancel\n'
    IFS= read -r choice || exit 19
    printf '%s' "$choice" > "$0.choice"
    [ "$choice" = "$(printf '\033[B')" ] || exit 17
    printf 'consent accepted\n'
    ;;
  *) printf 'stub ready\n' ;;
esac
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let mut child = RuntimeChild::spawn_configured(
            root.path(),
            &format!("PROMPT='{PROMPT} '\n"),
            Path::new("/bin/zsh"),
            Some(&stub),
        )?;
        child.hello()?;
        let session = child.session.clone();
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        let hook_path = || -> Result<PathBuf> {
            Ok(fs::read_to_string(stub.with_extension("settings"))?.into())
        };
        seed_catalog(root.path(), &child.workspace, PROVIDER_ID)?;
        child.send(Command::LoadProviderSession {
            session_id: session.clone(),
            provider_session_id: PROVIDER_ID.into(),
        })?;
        child.frame(|_, screen, _| screen_has_line(screen, "stub ready"))?;
        post_permission_identity(&hook_path()?, PROVIDER_ID, "default")?;
        wait_permission_identity(&mut child)?;
        let original = claude_binding(&db, &session)?.context("source binding")?;
        child.send(Command::SetProviderPermissionMode {
            session_id: session.clone(),
            mode: "bypassPermissions".into(),
        })?;
        let deadline = Instant::now() + TIMEOUT;
        let mut saw_restoring = false;
        loop {
            if let Event::Recovery { session_id, state } = child.event(deadline, true)?
                && session_id == session
            {
                if state == RecoveryState::Restoring {
                    saw_restoring = true;
                }
                if saw_restoring && state == RecoveryState::Ready {
                    break;
                }
            }
        }
        let input = |bytes: &[u8]| Command::Input {
            session_id: session.clone(),
            bytes: bytes.to_vec(),
            defer_ordering: false,
        };
        child.send(input(b"must not reach this dialog\r"))?;
        if accept {
            child.send(input(b"\x1b[B"))?;
        }
        child.send(input(b"\r"))?;
        if accept {
            child.frame(|_, screen, _| screen_has_line(screen, "consent accepted"))?;
            post_permission_identity(&hook_path()?, PROVIDER_ID, "bypassPermissions")?;
        }
        let result = permission_mode_result(&mut child, "bypassPermissions")?;
        let needs_confirmation = claude_binding(&db, &session)?.context("binding after consent")?;
        let runs = list_runs(&db, &session)?;
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].status, "exited");
        assert_eq!(needs_confirmation.id, original.id);
        assert_eq!(needs_confirmation.provider_session_id, PROVIDER_ID);
        assert_eq!(needs_confirmation.restore_state, "none");
        if accept {
            assert!(result.get("error").is_none(), "{result}");
            assert_eq!(fs::read(stub.with_extension("choice"))?, b"\x1b[B");
            assert_eq!(
                needs_confirmation.metadata["permissionMode"],
                "bypassPermissions"
            );
        } else {
            let error = result["error"].as_str().context("decline error")?;
            assert!(
                error.contains("exited during bypass permissions confirmation"),
                "{error}"
            );
            assert!(error.contains("process exited with code 17"), "{error}");
            assert!(fs::read(stub.with_extension("choice"))?.is_empty());
            assert_eq!(runs[2].exit_code, Some(17));
            assert_eq!(needs_confirmation.metadata, original.metadata);
        }
        child.send(Command::Shutdown)?;
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "runtime shutdown: {stderr}");
    }
    Ok(())
}

#[test]
fn catalog_replaces_a_live_process_once_and_requires_its_persisted_exit() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, list_runs};

    for fail_exit in [false, true] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("claude-stub");
        fs::write(
            &stub,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = '--resume' ] && [ \"$2\" = 'replace-id' ]; then printf 'resuming replace-id\\n'; exec /bin/sleep 60; fi\n  shift\ndone\nexit 3\n",
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let mut child = RuntimeChild::spawn_configured(
            root.path(),
            &format!("PROMPT='{PROMPT} '\ntrap 'exit 42' HUP\n"),
            Path::new("/bin/zsh"),
            Some(&stub),
        )?;
        child.hello()?;
        seed_catalog(root.path(), &child.workspace, "replace-id")?;
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        let session = child.session.clone();
        let old_run = list_runs(&db, &session)?.remove(0);
        assert_eq!(old_run.status, "running");
        let old_pid = old_run.pid.context("old PTY PID")? as u32;
        if fail_exit {
            db.execute_batch(
                "CREATE TRIGGER fail_exit BEFORE UPDATE OF status ON session_runs
                 WHEN NEW.status = 'exited'
                 BEGIN SELECT RAISE(ABORT, 'injected exit persistence failure'); END;",
            )?;
        }
        for _ in 0..2 {
            child.send(Command::LoadProviderSession {
                session_id: session.clone(),
                provider_session_id: "replace-id".into(),
            })?;
        }
        let deadline = Instant::now() + TIMEOUT;
        if fail_exit {
            loop {
                if let Event::Graph(graph) = child.event(deadline, true)?
                    && let Some(error) = graph["providerSessionLoadErrors"].get(0)
                {
                    assert_eq!(error["providerSessionId"], "replace-id");
                    assert!(
                        error["error"]
                            .as_str()
                            .unwrap()
                            .contains("injected exit persistence failure")
                    );
                    break;
                }
            }
            assert!(claude_binding(&db, &session)?.is_none());
            let runs = list_runs(&db, &session)?;
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].status, "running");
            db.execute_batch("DROP TRIGGER fail_exit;")?;
        } else {
            child.frame(|_, screen, _| screen_has_line(screen, "resuming replace-id"))?;
            let runs = list_runs(&db, &session)?;
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[0].id, old_run.id);
            assert_eq!(runs[0].status, "exited");
            assert_eq!(runs[0].exit_code, Some(42));
            assert_eq!(runs[0].signal, Some(0));
            assert!(runs[0].ended_at.unwrap() <= runs[1].started_at);
            assert_eq!(runs[1].status, "running");
            let binding = claude_binding(&db, &session)?.context("replacement binding")?;
            assert_eq!(binding.provider_session_id, "replace-id");
            assert_eq!(binding.restore_state, "restoring");
            assert_eq!(binding.metadata["loadedFromCatalog"], true);
            assert_eq!(binding.metadata["spawnRevision"], 1);
            let event_order: (i64, i64) = db.query_row(
                "SELECT
                   (SELECT rowid FROM domain_events WHERE event_type='session.run-exited'),
                   (SELECT rowid FROM domain_events WHERE event_type='session.mode-changed')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert!(event_order.0 < event_order.1);
        }
        assert_shell_gone(old_pid)?;
        child.send(Command::Shutdown)?;
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "runtime shutdown: {stderr}");
    }
    Ok(())
}

#[test]
fn closing_a_card_during_catalog_retirement_cancels_the_load() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, get_session, list_runs};

    let root = tempfile::tempdir()?;
    let shell_home = root.path().join("Shell 家目录");
    let mut child = RuntimeChild::spawn_configured(
        root.path(),
        &format!(
            "PROMPT='{PROMPT} '\ntrap 'print stopping > \"$HOME/retiring\"; \
             while [[ ! -f \"$HOME/release\" ]]; do /bin/sleep 0.01; done; exit 43' HUP\n"
        ),
        Path::new("/bin/zsh"),
        Some(Path::new("/usr/bin/false")),
    )?;
    child.hello()?;
    seed_catalog(root.path(), &child.workspace, "replace-id")?;
    let session = child.session.clone();
    let scene = child.scene.clone();
    let db = startup::open_read_only(&child.data.join("matou.sqlite"))?;
    let pid = list_runs(&db, &session)?[0].pid.context("old PTY PID")? as u32;
    child.send(Command::CreateShellSibling {
        scene_id: scene.clone(),
        source_session_id: session.clone(),
        parent_session_id: None,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    while !matches!(child.event(deadline, true)?, Event::Navigation { .. }) {}
    child.send(Command::LoadProviderSession {
        session_id: session.clone(),
        provider_session_id: "replace-id".into(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    while !shell_home.join("retiring").is_file() {
        ensure!(
            Instant::now() < deadline,
            "Shell did not receive its stop signal"
        );
        thread::sleep(Duration::from_millis(5));
    }
    child.send(Command::CloseSessionCard {
        scene_id: scene,
        session_id: session.clone(),
        confirmed_intent: Some(format!("delete-session:{session}")),
        preserve_scene_on_last_session: true,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    while get_session(&db, &session)?
        .context("closing card")?
        .archived_at
        .is_none()
    {
        ensure!(
            Instant::now() < deadline,
            "card was not archived during retirement"
        );
        thread::sleep(Duration::from_millis(5));
    }
    fs::write(shell_home.join("release"), "")?;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(error) = graph["providerSessionLoadErrors"].get(0)
        {
            assert_eq!(error["sessionId"], session);
            assert_eq!(
                error["error"],
                "Card was closed while loading the conversation"
            );
            break;
        }
    }
    let runs = list_runs(&db, &session)?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "exited");
    assert_eq!(runs[0].exit_code, Some(43));
    assert!(claude_binding(&db, &session)?.is_none());
    assert_shell_gone(pid)?;
    child.send(Command::Shutdown)?;
    child.closed()?;
    assert!(child.wait()?.0.success());
    Ok(())
}

#[test]
fn catalog_rejects_absent_ids_and_closing_a_pending_load_stops_its_only_run() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, list_runs};

    let root = tempfile::tempdir()?;
    let stub = root.path().join("claude-stub");
    // A real PTY with a deterministic stand-in which never confirms identity.
    fs::write(
        &stub,
        "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = '--resume' ] && [ \"$2\" = 'catalog-test-id' ]; then exec /bin/sleep 60; fi\n  shift\ndone\nexit 3\n",
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut child = RuntimeChild::spawn_configured(
        root.path(),
        "",
        &root.path().join("missing-shell"),
        Some(&stub),
    )?;
    child.ready()?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if matches!(
            child
                .event(deadline, true)
                .context("initial Shell start failure")?,
            Event::Recovery {
                state: matou_native_runtime::host::RecoveryState::StartFailed(_),
                ..
            }
        ) {
            break;
        }
    }

    seed_catalog(root.path(), &child.workspace, "catalog-test-id")?;
    let session = child.session.clone();
    let scene = child.scene.clone();
    // With a sibling, Close actually archives the target instead of hiding the window.
    child.send(Command::CreateShellSibling {
        scene_id: scene.clone(),
        source_session_id: session.clone(),
        parent_session_id: None,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if matches!(
            child.event(deadline, true).context("create sibling")?,
            Event::Navigation { .. }
        ) {
            break;
        }
    }
    let load = |id: &str| Command::LoadProviderSession {
        session_id: session.clone(),
        provider_session_id: id.into(),
    };
    let db = startup::open_read_only(&child.data.join("matou.sqlite"))?;
    child.send(load("absent"))?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child
            .event(deadline, true)
            .context("absent catalog result")?
            && let Some(error) = graph["providerSessionLoadErrors"].get(0)
        {
            assert_eq!(error["providerSessionId"], "absent");
            assert_eq!(
                error["error"],
                "Provider session was not found in this working directory"
            );
            break;
        }
    }
    assert!(claude_binding(&db, &session)?.is_none());
    assert!(list_runs(&db, &session)?.is_empty());

    child.send(load("catalog-test-id"))?;
    child.send(load("catalog-test-id"))?;
    let deadline = Instant::now() + TIMEOUT;
    let pid = loop {
        if let Event::Graph(graph) = child.event(deadline, true).context("catalog PTY startup")?
            && let Some(run) = list_runs(&db, &session)?.first()
        {
            assert!(graph["providerSessionLoadErrors"].is_null());
            assert_eq!(run.status, "running");
            break run.pid.context("PTY PID")? as u32;
        }
    };
    let active = claude_binding(&db, &session)?.context("catalog binding")?;
    assert_eq!(active.restore_state, "restoring");
    assert_eq!(active.metadata["spawnRevision"], 1);

    child.send(Command::CloseSessionCard {
        scene_id: scene,
        session_id: session.clone(),
        confirmed_intent: Some(format!("delete-session:{session}")),
        preserve_scene_on_last_session: true,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::ScenesDisposed(sessions) = child
            .event(deadline, true)
            .context("dispose pending catalog PTY")?
            && sessions.contains(&session)
        {
            break;
        }
    }
    assert_shell_gone(pid)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child
            .event(deadline, true)
            .context("cancelled catalog result")?
            && let Some(error) = graph["providerSessionLoadErrors"].get(0)
        {
            assert_eq!(error["sessionId"], session);
            assert_eq!(
                error["error"],
                "Card was closed while loading the conversation"
            );
            break;
        }
    }
    let runs = list_runs(&db, &session)?;
    assert_eq!(runs.len(), 1);
    assert_ne!(runs[0].status, "running");
    child.send(Command::Shutdown)?;
    child.closed()?;
    assert!(child.wait()?.0.success());
    Ok(())
}
