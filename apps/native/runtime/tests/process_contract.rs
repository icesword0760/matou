#![cfg(target_os = "macos")]

#[path = "process_contract/claude_fork_rebuild.rs"]
mod claude_fork_rebuild;
#[path = "process_contract/claude_gateway_restart.rs"]
mod claude_gateway_restart;
#[path = "process_contract/claude_model_catalog.rs"]
mod claude_model_catalog;
#[cfg(unix)]
#[path = "process_contract/claude_plugin_entries.rs"]
mod claude_plugin_entries;
#[path = "process_contract/codex_events.rs"]
mod codex_events;
#[path = "process_contract/codex_launch.rs"]
mod codex_launch;
#[path = "process_contract/codex_resume.rs"]
mod codex_resume;
#[path = "process_contract/database_recovery.rs"]
mod database_recovery;
#[path = "process_contract/quit.rs"]
mod quit;
#[path = "process_contract/subscription_router.rs"]
mod subscription_router;
#[path = "process_contract/time_zone_settings.rs"]
mod time_zone_settings;

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
    protocol::{
        ClaudeRoleAssignmentsReply, ClientMessage, ServerMessage, VERSION, read_messages,
        write_message,
    },
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
        let rc = home.join(".zshrc");
        if !zshrc.is_empty() || rc.exists() {
            fs::write(rc, zshrc)?;
        }
        let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_matou-native-runtime"));
        command
            .env_remove("MATOU_CONTROL_ASSET_ROOT")
            .env_remove("MATOU_LOCALE")
            .env_remove("CLAUDE_CODE_MAX_CONTEXT_TOKENS")
            .env_remove("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
            .env_remove("CLAUDE_CODE_DISABLE_1M_CONTEXT")
            .env("MATOU_DISABLE_LIVE_EGRESS", "1")
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

    fn input_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.send(Command::Input {
            session_id: self.session.clone(),
            bytes: bytes.to_vec(),
            defer_ordering: false,
        })
    }

    fn expect_diagnostic(&mut self, expected: &str) -> Result<()> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.event(deadline, true)? {
                Event::Diagnostic(message) => {
                    ensure!(
                        message.contains(expected),
                        "unexpected diagnostic: {message}"
                    );
                    return Ok(());
                }
                Event::Closed => bail!("host closed while waiting for {expected}"),
                _ => {}
            }
        }
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
        b"{\"Hello\":{\"version\":1}}".as_slice(),
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
            row: 30,
            pixel_target: false,
            request_id: request_id - 1,
        },
        Command::ScrollTo {
            session_id: child.session.clone(),
            row: 0,
            pixel_target: false,
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
fn active_workspace_path_poll_only_persists_and_publishes_changes() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    child.ready()?;
    let database = child.data.join("matou.sqlite");
    let db = startup::open_read_only(&database)?;
    let await_state = |child: &mut RuntimeChild, status, reason, generation| -> Result<String> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match child.event(deadline, true)? {
                Event::WorkspacePaths(paths) => {
                    let [state] = paths.as_slice() else {
                        bail!("expected one workspace path: {paths:?}");
                    };
                    assert_eq!(
                        (
                            state.status.as_str(),
                            state.reason.as_str(),
                            state.validation_generation
                        ),
                        (status, reason, generation)
                    );
                    assert_eq!(
                        matou_native_core::workspace_paths::get(&db, &state.workspace_id)?.as_ref(),
                        Some(state)
                    );
                    return Ok(state.workspace_id.clone());
                }
                Event::Diagnostic(error) => bail!("path check diagnostic: {error}"),
                Event::Closed => bail!("host closed while waiting for {status}"),
                _ => {}
            }
        }
    };
    let receipt_count = || -> Result<i64> {
        Ok(db.query_row(
            "SELECT COUNT(*) FROM command_deduplication WHERE command_type='workspace.validate-path'",
            [],
            |row| row.get(0),
        )?)
    };
    let workspace_id = await_state(&mut child, "valid", "", 1)?;
    // Drain two scheduled checks while the directory is healthy.
    let deadline = Instant::now() + Duration::from_millis(4_250);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match child.messages.recv_timeout(left) {
            Ok(message) => {
                assert!(
                    !matches!(
                        message?,
                        ServerMessage::Event(
                            Event::WorkspacePaths(_) | Event::Diagnostic(_) | Event::Closed
                        )
                    ),
                    "unexpected event during unchanged checks"
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(error) => return Err(error.into()),
        }
    }
    assert_eq!(receipt_count()?, 1);

    let moved = root.path().join("moved-workspace");
    fs::rename(&child.workspace, &moved)?;
    await_state(&mut child, "invalid", "missing", 2)?;
    assert_eq!(receipt_count()?, 2);
    fs::rename(&moved, &child.workspace)?;
    await_state(&mut child, "valid", "", 3)?;
    assert_eq!(receipt_count()?, 3);

    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");

    // Relinking clears the row before the async check. Model a stop in between.
    assert_eq!(
        Connection::open(&database)?.execute(
            "DELETE FROM workspace_path_state WHERE workspace_id = ?",
            [&workspace_id],
        )?,
        1
    );
    let mut reopened = RuntimeChild::spawn(root.path())?;
    reopened.connect()?;
    assert_eq!(await_state(&mut reopened, "valid", "", 4)?, workspace_id);
    assert_eq!(receipt_count()?, 4);
    reopened.send(Command::Shutdown)?;
    assert!(reopened.closed()?.is_empty());
    let (status, stderr) = reopened.wait()?;
    ensure!(status.success(), "{stderr}");
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
    use serde_json::json;

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
        let mut disposed = false;
        let replacement = loop {
            match child.event(deadline, true)? {
                Event::ScenesDisposed(ids) => {
                    assert_eq!(ids, [survivor.clone()]);
                    disposed = true;
                }
                Event::Navigation {
                    hierarchy, graph, ..
                } if !graph["sessionCardAction"].is_null() => {
                    assert!(disposed, "last card published before disposal");
                    let action = &graph["sessionCardAction"];
                    assert_eq!(action["outcome"], "default-task-created");
                    assert_eq!(action["disposedSessionIds"], json!([survivor]));
                    assert_eq!(action["rebuildCanvas"], true);
                    assert_eq!(hierarchy.task.context("replacement task")?.title, "默认");
                    let scene = hierarchy.scene.context("replacement scene")?;
                    assert_ne!(scene.id, child.scene);
                    let replacement = hierarchy.session.context("replacement Shell")?;
                    assert_eq!(replacement.kind, "shell");
                    assert_ne!(replacement.id, survivor);
                    assert_eq!(graph["focusedSessionId"], replacement.id);
                    break replacement.id;
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
                } if session_id == replacement && screen_has_line(&screen, PROMPT) => break,
                Event::Diagnostic(message) => bail!("{message}"),
                _ => {}
            }
        }
        assert_eq!(card_write_count(&db)?, (before.0 + 1, before.1 + 1));
        assert!(
            db.query_row(
                "SELECT archived_at FROM sessions WHERE id = ?",
                [&survivor],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .is_some()
        );
        assert!(
            db.query_row(
                "SELECT archived_at FROM scenes WHERE id = ?",
                [&child.scene],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .is_some()
        );
        assert!(db.query_row(
            "SELECT task.archived_at FROM tasks task JOIN scenes scene ON scene.task_id = task.id WHERE scene.id = ?",
            [&child.scene],
            |row| row.get::<_, Option<i64>>(0),
        )?.is_some());

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

fn seed_catalog(root: &Path, workspace: &Path, id: &str) -> Result<PathBuf> {
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
    let transcript = bucket.join(format!("{id}.jsonl"));
    fs::write(
        &transcript,
        serde_json::json!({
            "cwd": cwd, "timestamp": 1234,
            "message": {"role": "user", "content": "Synthetic command fixture"}
        })
        .to_string(),
    )?;
    Ok(transcript)
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
            ..
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
        request_id: None,
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

#[test]
fn shell_promotion_keeps_the_card_and_launches_fresh_with_saved_assignments() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, list_runs};
    use matou_native_runtime::host::{ForkPlacement, ForkWorktreeMode};
    use serde_json::{Value, json};

    for (mode, expected_title) in [("default", "Claude"), ("bypassPermissions", "My workspace")] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("promotion-cli");
        fs::write(
            &stub,
            r#"#!/bin/sh
case "$*" in
  *--fork-session*)
    printf '%s\n' "$@" > "$0.fork"
    printf 'PROMOTED_FORK_READY\n'
    exec /bin/sleep 60
    ;;
esac
printf '%s\n' "$@" > "$0.argv"
printf '%s\n' "$ANTHROPIC_BASE_URL" "$ANTHROPIC_DEFAULT_OPUS_MODEL" > "$0.env"
printf 'PROMOTED_CLAUDE_READY\n'
IFS= read -r line
printf 'CLI_INPUT=%s\n' "$line"
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let mut child = RuntimeChild::spawn_configured(
            root.path(),
            &format!(
                "PROMPT='{PROMPT} '\nclaude() {{ printf 'wrong' > \"$HOME/ran-in-shell\"; }}\n"
            ),
            Path::new("/bin/zsh"),
            Some(&stub),
        )?;
        child.hello()?;
        let shell_pid = child.shell_pid()?;
        let session = child.session.clone();
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        db.busy_timeout(TIMEOUT)?;
        let mount: String = db.query_row(
            "SELECT id FROM session_mounts WHERE session_id=?",
            [&session],
            |row| row.get(0),
        )?;
        if mode == "bypassPermissions" {
            db.execute(
                "UPDATE sessions SET title=?, title_source='manual' WHERE id=?",
                [expected_title, &session],
            )?;
        } else {
            db.execute(
                "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id, resume_state,
                     metadata_json, created_at, updated_at, validated_at)
                 VALUES ('stale', ?, 'claude-code', 'old-conversation', 'available',
                         '{\"model\":\"old-model\",\"permissionMode\":\"plan\",\"canFork\":true,
                           \"observedUserPrompt\":true,\"observedNormalStop\":true}', 1, 1, 1)",
                [&session],
            )?;
        }
        fs::write(
            child.data.join("provider-config.json"),
            json!({
                "version": 1,
                "activeProviderIds": {"claude-code": "anthropic-official"},
                "providers": {"claude-code": [{
                    "id": "anthropic-official", "model": "", "builtIn": true,
                    "endpoint": "https://api.anthropic.com"
                }, {
                    "id": "assigned-provider", "model": "provider-default",
                    "apiKey": "fixture-key", "endpoint": "https://assigned.example.invalid/",
                    "modelMetadata": [{"id": "assigned-model", "supportedEfforts": ["high"]}]
                }]}
            })
            .to_string(),
        )?;
        child.send(Command::SetClaudeRoleAssignments {
            request_id: 1,
            web_search: None,
            assignments: json!({"opus": {
                "providerId": "assigned-provider", "model": "assigned-model", "effort": "high"
            }}),
        })?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Event::Graph(graph) = child.event(deadline, true)?
                && let Some(reply) = graph.get("claudeRoleAssignmentsResult")
            {
                assert_eq!(
                    reply["assignments"]["opus"]["model"], "assigned-model",
                    "{reply}"
                );
                break;
            }
        }
        let command = if mode == "default" {
            &b"de  \rcombined-"[..]
        } else {
            &b"de --dangerously-skip-permissions\ncombined-"[..]
        };
        for keys in [&b"  clau"[..], command, b"queued\r"] {
            child.send(Command::Input {
                session_id: session.clone(),
                bytes: keys.to_vec(),
                defer_ordering: true,
            })?;
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match child.event(deadline, true)? {
                Event::Graph(graph) if graph["nodes"][0]["currentMode"] == "claude-code" => {
                    let nodes = graph["nodes"].as_array().unwrap();
                    assert_eq!(nodes.len(), 1);
                    assert_eq!(nodes[0]["sessionId"], session);
                    assert_eq!(nodes[0]["title"], expected_title);
                    assert_eq!(nodes[0]["workStatus"], "starting");
                    assert_eq!(nodes[0]["canFork"], false);
                    assert_eq!(graph["focusedSessionId"], session);
                    break;
                }
                Event::Diagnostic(error) => bail!("{error}"),
                _ => {}
            }
        }
        child.frame(|rows, _, _| rows.iter().any(|row| row == "CLI_INPUT=queued"))?;
        assert_shell_gone(shell_pid)?;
        assert!(!root.path().join("Shell 家目录/ran-in-shell").exists());
        let runs = list_runs(&db, &session)?;
        assert_eq!(runs.len(), 2);
        assert_eq!((&*runs[0].profile, &*runs[0].status), ("shell", "exited"));
        assert_eq!(
            (&*runs[1].profile, &*runs[1].status),
            ("claude-code", "running")
        );
        assert_eq!(
            db.query_row(
                "SELECT id FROM session_mounts WHERE session_id=?",
                [&session],
                |row| row.get::<_, String>(0)
            )?,
            mount
        );
        if mode == "default" {
            let binding = claude_binding(&db, &session)?.context("retained old binding")?;
            assert_eq!(binding.provider_session_id, "old-conversation");
            assert!(!binding.resumable);
            assert_eq!(binding.restore_state, "none");
            for key in ["canFork", "observedUserPrompt", "observedNormalStop"] {
                assert_eq!(binding.metadata[key], false, "{key}");
            }
            assert_eq!(binding.metadata["model"], "old-model");
            assert_eq!(
                db.query_row(
                    "SELECT resume_state = 'expired' AND invalidated_at > 1
                 FROM provider_bindings WHERE id = 'stale'",
                    [],
                    |row| row.get::<_, bool>(0),
                )?,
                true
            );
        }
        let argv = fs::read_to_string(stub.with_extension("argv"))?;
        let argv: Vec<_> = argv.lines().collect();
        assert!(
            argv.windows(2)
                .any(|args| args == ["--model", "assigned-model"])
        );
        assert!(argv.windows(2).any(|args| args == ["--effort", "high"]));
        for flag in ["--resume", "--fork-session", "--permission-mode"] {
            assert!(!argv.contains(&flag), "{argv:?}");
        }
        assert_eq!(
            argv.contains(&"--dangerously-skip-permissions"),
            mode == "bypassPermissions"
        );
        assert_eq!(
            fs::read_to_string(stub.with_extension("env"))?,
            "https://assigned.example.invalid\nassigned-model\n"
        );
        let settings_path = Path::new(
            argv.windows(2)
                .find(|args| args[0] == "--settings")
                .context("settings argv")?[1],
        );
        let settings: Value = serde_json::from_slice(&fs::read(settings_path)?)?;
        assert_eq!(
            settings["hooks"]["SessionStart"][0]["hooks"][0]["type"],
            "command"
        );
        let provider_id = "promoted-conversation";
        post_claude_identity(settings_path, provider_id, mode, "assigned-model")?;
        wait_permission_identity(&mut child)?;
        let binding = claude_binding(&db, &session)?.context("promoted Claude binding")?;
        assert_eq!(binding.provider_session_id, provider_id);
        assert_eq!(binding.metadata["permissionMode"], mode);
        assert_eq!(binding.metadata["assignedRole"], "opus");
        if mode == "default" {
            for event in ["UserPromptSubmit", "Stop"] {
                let status = post_claude_hook(
                    settings_path,
                    json!({
                        "hook_event_name": event, "session_id": provider_id
                    }),
                )?;
                assert_eq!(status, 200);
            }
            let deadline = Instant::now() + TIMEOUT;
            loop {
                if let Event::Graph(graph) = child.event(deadline, true)?
                    && graph["nodes"][0]["canFork"] == true
                {
                    break;
                }
            }
            child.send(Command::ForkSession {
                scene_id: child.scene.clone(),
                source_session_id: session.clone(),
                placement: ForkPlacement::Child,
                name: "Fork the promoted conversation".into(),
                worktree_mode: ForkWorktreeMode::Current,
                submission_key: "fork-promoted".into(),
            })?;
            let deadline = Instant::now() + TIMEOUT;
            loop {
                match child.event(deadline, true)? {
                    Event::Frame { screen, .. }
                        if screen_has_line(&screen, "PROMOTED_FORK_READY") =>
                    {
                        break;
                    }
                    Event::Graph(graph) if graph["forkSessionResult"].get("error").is_some() => {
                        bail!("fork failed: {}", graph["forkSessionResult"]);
                    }
                    Event::Diagnostic(error) => bail!("{error}"),
                    _ => {}
                }
            }
            let fork = fs::read_to_string(stub.with_extension("fork"))?;
            assert!(
                fork.lines()
                    .collect::<Vec<_>>()
                    .windows(3)
                    .any(|args| args == ["--resume", provider_id, "--fork-session"]),
                "{fork}"
            );
            assert!(!fork.contains("old-conversation"), "{fork}");
        }
        child.send(Command::Shutdown)?;
        assert!(child.closed()?.is_empty());
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "runtime shutdown: {stderr}");
    }
    Ok(())
}

#[test]
fn shell_promotion_uses_the_last_submission_and_forwards_a_miss_verbatim() -> Result<()> {
    use matou_native_core::journal::for_each_frame;
    use matou_native_core::sessions::{get_session, list_runs};

    for (label, writes, copy_interrupt, promotes, shell_claude, shell_ls) in [
        (
            "paste",
            vec![&b"claude\r"[..]],
            false,
            true,
            Some(false),
            false,
        ),
        (
            "crlf",
            vec![&b"claude\r\n"[..]],
            false,
            false,
            Some(true),
            false,
        ),
        (
            "multi",
            vec![&b"claude\rls\r"[..]],
            false,
            false,
            Some(true),
            true,
        ),
        (
            "prefix-and-tail",
            vec![&b"touch x\rclaude\rSHOULD_NOT_REACH_OLD_SHELL"[..]],
            false,
            true,
            Some(false),
            false,
        ),
        (
            "del-bs",
            vec![&b"claudexy"[..], b"\x7f\x08\r"],
            false,
            true,
            Some(false),
            false,
        ),
        (
            "emoji-once",
            vec!["claude😀\x7f\r".as_bytes()],
            false,
            false,
            None, // Whether zsh deletes the whole emoji depends on its locale.
            false,
        ),
        (
            "emoji-twice",
            vec!["claude😀".as_bytes(), b"\x7f\x08\r"],
            false,
            true,
            Some(false),
            false,
        ),
        (
            "ctrl-c",
            vec![&b"claude"[..], b"\x03", b"\r"],
            false,
            false,
            Some(false),
            false,
        ),
        (
            "ctrl-u",
            vec![&b"claude"[..], b"\x15", b"\r"],
            false,
            false,
            Some(false),
            false,
        ),
        (
            "copy-interrupt",
            vec![&b"claude"[..]],
            true,
            false,
            Some(false),
            false,
        ),
    ] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("promotion-cli");
        fs::write(
            &stub,
            "#!/bin/sh\nprintf 'INPUT_CONTRACT_CLI\n'\nexec /bin/sleep 60\n",
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let mut child = RuntimeChild::spawn_configured(
            root.path(),
            &format!(
                r#"PROMPT='{PROMPT} '
bindkey -e
bindkey '^?' backward-delete-char
bindkey '^H' backward-delete-char
claude() {{ touch "$HOME/shell-claude"; }}
ls() {{ touch "$HOME/shell-ls"; }}
"#
            ),
            Path::new("/bin/zsh"),
            Some(&stub),
        )?;
        child.hello()?;
        for bytes in writes {
            child.input_bytes(bytes)?;
        }
        if copy_interrupt {
            child.send(Command::CopySelection {
                session_id: child.session.clone(),
                request_id: 23,
                interrupt_if_empty: true,
            })?;
            // Send Return without waiting for the copy acknowledgement.
            child.input_bytes(b"\r")?;
        }
        if promotes {
            child
                .frame(|_, screen, _| screen_has_line(screen, "INPUT_CONTRACT_CLI"))
                .with_context(|| label.to_owned())?;
        } else {
            child.input("printf 'AFTER_SHELL_INPUT\n'")?;
            child
                .frame(|_, screen, _| screen_has_line(screen, "AFTER_SHELL_INPUT"))
                .with_context(|| label.to_owned())?;
        }
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        let session = get_session(&db, &child.session)?.context("input contract card")?;
        assert_eq!(
            session.kind,
            if promotes { "claude-code" } else { "shell" },
            "{label}"
        );
        assert_eq!(
            list_runs(&db, &child.session)?.len(),
            if promotes { 2 } else { 1 },
            "{label}"
        );
        let home = root.path().join("Shell 家目录");
        if let Some(shell_claude) = shell_claude {
            assert_eq!(home.join("shell-claude").exists(), shell_claude, "{label}");
        }
        assert_eq!(home.join("shell-ls").exists(), shell_ls, "{label}");
        assert!(!child.workspace.join("x").exists(), "{label}");
        child.send(Command::Shutdown)?;
        assert!(child.closed()?.is_empty(), "{label}");
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "{label}: {stderr}");
        if label == "prefix-and-tail" {
            let mut output = Vec::new();
            for_each_frame(&child.data, &child.session, 1, None, |record| {
                output.extend_from_slice(record.frame.output());
                Ok(())
            })?;
            assert!(!String::from_utf8_lossy(&output).contains("SHOULD_NOT_REACH_OLD_SHELL"));
        }
    }
    Ok(())
}

#[test]
fn configured_cc_alias_promotes_only_claude_with_its_permission_mode() -> Result<()> {
    use matou_native_core::sessions::{get_session, list_runs};
    use serde_json::Value;

    for (alias, bypass) in [
        ("claude", Some(false)),
        ("claude --dangerously-skip-permissions", Some(true)),
        ("printf OLD_CUSTOM_CC", None),
    ] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("cc-cli");
        fs::write(
            &stub,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.argv\"\nprintf 'CC_PROMOTED\\n'\nexec /bin/sleep 60\n",
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let rc = format!(
            "PROMPT='{PROMPT} '\nalias cc='{alias}'\nclaude() {{ printf 'CC_STAYED_IN_SHELL\\n'; }}\n"
        );
        let mut child =
            RuntimeChild::spawn_configured(root.path(), &rc, Path::new("/bin/zsh"), Some(&stub))?;
        child.hello()?;
        child.input(if bypass.is_some() { "cc" } else { "cc --help" })?;
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        db.busy_timeout(TIMEOUT)?;
        if let Some(bypass) = bypass {
            child.frame(|_, screen, _| screen_has_line(screen, "CC_PROMOTED"))?;
            child.send(Command::GetEgressStatus { request_id: 90 })?;
            let deadline = Instant::now() + TIMEOUT;
            loop {
                if let Event::Graph(reply) = child.event(deadline, true)?
                    && reply["type"] == "egressStatusResult"
                    && reply["requestId"] == 90
                {
                    assert_eq!(
                        reply["result"]["sessionFamilies"][&child.session],
                        serde_json::json!(["anthropic"])
                    );
                    break;
                }
            }
            let argv = fs::read_to_string(stub.with_extension("argv"))?;
            assert_eq!(
                argv.contains("--dangerously-skip-permissions"),
                bypass,
                "{argv}"
            );
            let mut args = argv.lines();
            assert_eq!(args.next(), Some("--settings"));
            let config: Value =
                serde_json::from_slice(&fs::read(args.next().context("settings arg")?)?)?;
            assert_eq!(config["statusLine"]["type"], "command");
            assert_eq!(config["statusLine"]["padding"], 0);
            assert_eq!(
                get_session(&db, &child.session)?
                    .context("promoted Claude")?
                    .kind,
                "claude-code"
            );
            assert_eq!(list_runs(&db, &child.session)?.len(), 2);
        } else {
            child.frame(|rows, _, _| rows.iter().any(|row| row.contains("CC_STAYED_IN_SHELL")))?;
            assert!(!stub.with_extension("argv").exists());
            assert_eq!(
                get_session(&db, &child.session)?
                    .context("ordinary Shell")?
                    .kind,
                "shell"
            );
            assert_eq!(list_runs(&db, &child.session)?.len(), 1);
        }
        child.send(Command::Shutdown)?;
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "cc runtime shutdown: {stderr}");
    }
    Ok(())
}

#[test]
fn shell_promotion_keeps_a_verified_cli_on_shutdown_and_reopens_without_resume() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, get_session, list_runs};

    let root = tempfile::tempdir()?;
    let stub = root.path().join("promotion-cli");
    fs::write(
        &stub,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$0.argv\"\nprintf 'ATTACHED_CLAUDE\\n'\nIFS= read -r line\nprintf 'VERIFIED_CLAUDE_%s\\n' \"$line\"\nexec /bin/sleep 60\n",
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let zshrc = format!("PROMPT='{PROMPT} '\n");
    let mut child =
        RuntimeChild::spawn_configured(root.path(), &zshrc, Path::new("/bin/zsh"), Some(&stub))?;
    child.hello()?;
    let session = child.session.clone();
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    db.execute(
        "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id, resume_state,
             metadata_json, created_at, updated_at, validated_at)
         VALUES ('old-binding', ?, 'claude-code', 'should-never-resume', 'available',
                 '{\"canFork\":true}', 1, 1, 1)",
        [&session],
    )?;
    child.input_bytes(b"claude\r")?;
    child.frame(|_, screen, _| screen_has_line(screen, "ATTACHED_CLAUDE"))?;
    child.input("PROBE")?;
    child.frame(|_, screen, _| screen_has_line(screen, "VERIFIED_CLAUDE_PROBE"))?;
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "shutdown after attachment: {stderr}");
    assert_eq!(
        get_session(&db, &session)?.context("attached card")?.kind,
        "claude-code"
    );
    assert!(
        !claude_binding(&db, &session)?
            .context("old binding retained")?
            .resumable
    );
    assert_eq!(list_runs(&db, &session)?.len(), 2);

    let mut reopened =
        RuntimeChild::spawn_configured(root.path(), &zshrc, Path::new("/bin/zsh"), Some(&stub))?;
    reopened.connect()?;
    assert_eq!(reopened.session, session);
    // A card without a resumable binding may be parked on startup.
    reopened.send(Command::RetryRecovery {
        session_id: session.clone(),
    })?;
    reopened.frame(|_, screen, _| screen_has_line(screen, "ATTACHED_CLAUDE"))?;
    assert_eq!(list_runs(&db, &session)?.len(), 3);
    let argv = fs::read_to_string(stub.with_extension("argv"))?;
    assert_eq!(argv.lines().filter(|line| *line == "--settings").count(), 2);
    assert!(!argv.contains("--resume"), "{argv}");
    assert!(!argv.contains("should-never-resume"), "{argv}");
    reopened.send(Command::Shutdown)?;
    assert!(reopened.closed()?.is_empty());
    let (status, stderr) = reopened.wait()?;
    ensure!(status.success(), "reopened runtime: {stderr}");
    Ok(())
}

#[test]
#[cfg(debug_assertions)]
fn verified_promotion_keeps_the_card_and_resumes_a_hook_received_during_shutdown() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, get_session, list_runs};

    let root = tempfile::tempdir()?;
    let stub = root.path().join("promotion-cli");
    fs::write(
        &stub,
        r#"#!/bin/sh
printf '%s' "$2" > "$0.settings"
printf '%s\n' "$@" > "$0.argv"
case " $* " in
    *" --resume shutdown-conversation "*)
        printf 'RESUMED_AFTER_SHUTDOWN\n'
        exec /bin/sleep 60
        ;;
esac
trap '
    : > "$0.stopping"
    until [ -f "$0.release" ]; do /bin/sleep 0.01; done
    exit 0
' HUP
printf 'PROMOTION_CHECK_PENDING\n'
while :; do /bin/sleep 0.01; done
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let verified = root.path().join("promotion-verified");
    let zshrc = format!("PROMPT='{PROMPT} '\n");
    let mut child = RuntimeChild::spawn_configured_with(
        root.path(),
        &zshrc,
        Path::new("/bin/zsh"),
        Some(&stub),
        |command| {
            command.env("MATOU_TEST_HOLD_VERIFIED_PROMOTION", &verified);
        },
    )?;
    child.hello()?;
    child.input("claude")?;
    child.frame(|_, screen, _| screen_has_line(screen, "PROMOTION_CHECK_PENDING"))?;
    let wait_for_marker = |path: &Path| -> Result<()> {
        let deadline = Instant::now() + TIMEOUT;
        while !path.is_file() {
            ensure!(Instant::now() < deadline, "waiting for {}", path.display());
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    };
    wait_for_marker(&verified)?;
    let session = child.session.clone();
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    let runs = list_runs(&db, &session)?;
    assert_eq!(runs.len(), 2);
    assert_eq!(fs::read_to_string(&verified)?, runs[1].id);
    assert!(claude_binding(&db, &session)?.is_none());

    let settings = PathBuf::from(fs::read_to_string(stub.with_extension("settings"))?);
    child.send(Command::Shutdown)?;
    // The child receives SIGHUP only after the owner has entered shutdown.
    wait_for_marker(&stub.with_extension("stopping"))?;
    let posted = post_claude_identity(&settings, "shutdown-conversation", "default", "fixture");
    fs::write(stub.with_extension("release"), "")?;
    posted?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "shutdown before Ready: {stderr}");
    let card = get_session(&db, &session)?.context("verified card")?;
    assert_eq!(card.kind, "claude-code");
    let work_status: String = db.query_row(
        "SELECT work_status FROM sessions WHERE id=?",
        [&session],
        |row| row.get(0),
    )?;
    assert_ne!(work_status, "interrupted");
    let binding = claude_binding(&db, &session)?.context("binding from shutdown hook")?;
    assert_eq!(binding.provider_session_id, "shutdown-conversation");
    assert!(binding.resumable);

    let mut reopened =
        RuntimeChild::spawn_configured(root.path(), &zshrc, Path::new("/bin/zsh"), Some(&stub))?;
    reopened.connect()?;
    assert_eq!(reopened.session, session);
    reopened.frame(|_, screen, _| screen_has_line(screen, "RESUMED_AFTER_SHUTDOWN"))?;
    let argv = fs::read_to_string(stub.with_extension("argv"))?;
    assert!(
        argv.lines()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|args| args == ["--resume", "shutdown-conversation"]),
        "{argv}"
    );
    assert_eq!(list_runs(&db, &session)?.len(), 3);
    reopened.send(Command::Shutdown)?;
    assert!(reopened.closed()?.is_empty());
    let (status, stderr) = reopened.wait()?;
    ensure!(status.success(), "reopened runtime: {stderr}");
    Ok(())
}

#[test]
fn shell_promotion_shutdown_before_process_check_reopens_as_shell() -> Result<()> {
    use matou_native_core::sessions::{get_session, list_runs};

    for stage in ["stopping", "preparing", "starting"] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("promotion-cli");
        fs::write(&stub, "#!/bin/sh\n: > \"$0.started\"\nexec /bin/sleep 60\n")?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let zshrc = if stage == "stopping" {
            format!(
                r#"PROMPT='{PROMPT} '
TRAPHUP() {{
    /bin/stty -isig -icanon -echo
    : > "$HOME/stopping"
    until [[ -f "$HOME/release" ]]; do
        if read -r -k 1 -t 0.02 key; then
            printf '%s' "$key" >> "$HOME/stopping-input"
        fi
    done
    exit 0
}}
"#
            )
        } else {
            format!("PROMPT='{PROMPT} '\n")
        };
        let mut child = RuntimeChild::spawn_configured(
            root.path(),
            &zshrc,
            Path::new("/bin/zsh"),
            Some(&stub),
        )?;
        child.hello()?;
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        db.busy_timeout(TIMEOUT)?;
        db.execute(
            "UPDATE sessions SET title='Shell', title_source='manual' WHERE id=?",
            [&child.session],
        )?;
        let provider_config = child.data.join("provider-config.json");
        if stage == "preparing" {
            // Hold the background provider preparation in its config read.
            ensure!(
                ProcessCommand::new("/usr/bin/mkfifo")
                    .arg(&provider_config)
                    .status()?
                    .success()
            );
        }
        child.input("claude")?;
        let home = root.path().join("Shell 家目录");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let stored = get_session(&db, &child.session)?.context("promotion card")?;
            let reached = match stage {
                "stopping" => home.join("stopping").exists() && stored.kind == "shell",
                "preparing" => stored.kind == "claude-code" && stored.title == "Claude",
                "starting" => {
                    stub.with_extension("started").exists() && stored.kind == "claude-code"
                }
                _ => unreachable!(),
            };
            if reached {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "did not reach promotion phase {stage}"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            list_runs(&db, &child.session)?.len(),
            if stage == "starting" { 2 } else { 1 }
        );
        if stage != "starting" {
            let request_id = 32;
            child.send(Command::Select {
                session_id: child.session.clone(),
                action: matou_native_terminal::SelectionAction::Clear,
            })?;
            child.send(Command::Scroll {
                session_id: child.session.clone(),
                lines: 1,
            })?;
            child.send(Command::Search {
                session_id: child.session.clone(),
                request: matou_native_runtime::search::SearchRequest {
                    sequence: 31,
                    query: PROMPT.into(),
                    options: Default::default(),
                    previous: false,
                },
            })?;
            child.send(Command::ClearSearch {
                session_id: child.session.clone(),
            })?;
            child.send(Command::CopySelection {
                session_id: child.session.clone(),
                request_id,
                interrupt_if_empty: true,
            })?;
            let deadline = Instant::now() + TIMEOUT;
            loop {
                match child.event(deadline, true)? {
                    Event::Copied {
                        request_id: id,
                        text,
                    } if id == request_id => {
                        assert_eq!(text, "", "{stage}");
                        break;
                    }
                    Event::SearchResults { sequence: 31, .. } => {
                        bail!("search ran during {stage}")
                    }
                    Event::Diagnostic(error) => bail!("{stage}: {error}"),
                    _ => {}
                }
            }
        }
        child.send(Command::Shutdown)?;
        if stage == "stopping" {
            thread::sleep(Duration::from_millis(75));
            fs::write(home.join("release"), "")?;
        }
        assert!(child.closed()?.is_empty(), "{stage}");
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "{stage}: {stderr}");
        let stored = get_session(&db, &child.session)?.context("rolled back card")?;
        assert_eq!(
            (stored.kind.as_str(), stored.title.as_str()),
            ("shell", "Shell"),
            "{stage}"
        );
        let work_status: String = db.query_row(
            "SELECT work_status FROM sessions WHERE id=?",
            [&child.session],
            |row| row.get(0),
        )?;
        assert_eq!(
            work_status,
            if stage == "stopping" {
                "exited"
            } else {
                "interrupted"
            },
            "{stage}"
        );
        if stage == "stopping" {
            let stopping_input = home.join("stopping-input");
            if stopping_input.exists() {
                assert!(!fs::read(stopping_input)?.contains(&0x03));
            }
            let promoted: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM domain_events WHERE session_id=?
                 AND event_type='session.updated' AND json_extract(payload_json,'$.session.kind')='claude-code')",
                [&child.session],
                |row| row.get(0),
            )?;
            assert!(
                !promoted,
                "Shutdown must be handled before Stopping advances"
            );
        }
        if stage == "preparing" {
            fs::remove_file(&provider_config)?;
        }
        let mut reopened = RuntimeChild::spawn(root.path())?;
        reopened.connect()?;
        assert_eq!(reopened.session, child.session);
        if stage == "stopping" {
            reopened.send(Command::Refresh)?;
            let deadline = Instant::now() + TIMEOUT;
            loop {
                match reopened.event(deadline, true)? {
                    Event::Graph(graph) if graph["nodes"][0]["sessionId"] == child.session => {
                        assert_eq!(graph["nodes"][0]["workStatus"], "exited");
                        break;
                    }
                    Event::Frame { .. } => bail!("the stopped Shell relaunched without a request"),
                    Event::Diagnostic(error) => bail!("{error}"),
                    _ => {}
                }
            }
            assert_eq!(list_runs(&db, &child.session)?.len(), 1);
            reopened.send(Command::RestartSession {
                session_id: child.session.clone(),
            })?;
        }
        reopened.frame(|_, screen, _| screen_has_line(screen, PROMPT))?;
        reopened.input("printf 'REOPENED_SHELL_%s\\n' VERIFIED")?;
        reopened.frame(|_, screen, _| screen_has_line(screen, "REOPENED_SHELL_VERIFIED"))?;
        reopened.send(Command::Shutdown)?;
        assert!(reopened.closed()?.is_empty(), "{stage}");
        let (status, stderr) = reopened.wait()?;
        ensure!(status.success(), "{stage}: {stderr}");
    }
    Ok(())
}

#[test]
fn shell_promotion_copy_interrupts_the_attached_pty_during_startup() -> Result<()> {
    let root = tempfile::tempdir()?;
    let stub = root.path().join("promotion-cli");
    fs::write(
        &stub,
        r#"#!/bin/sh
/bin/stty -isig -icanon -echo
: > "$0.raw"
/bin/dd bs=1 count=1 of="$0.byte" 2>/dev/null
printf 'ATTACHED_PTY_RECEIVED_CONTROL\n'
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
    child.input("claude")?;
    child.frame(|_, screen, _| !screen_has_line(screen, PROMPT))?;
    let deadline = Instant::now() + TIMEOUT;
    while !stub.with_extension("raw").exists() {
        ensure!(
            Instant::now() < deadline,
            "waiting for the attached CLI's terminal mode"
        );
        thread::sleep(Duration::from_millis(5));
    }
    child.send(Command::CopySelection {
        session_id: child.session.clone(),
        request_id: 31,
        interrupt_if_empty: true,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    let (mut copied, mut control_received) = (false, false);
    while !copied || !control_received {
        match child.event(deadline, true)? {
            Event::Copied {
                request_id: 31,
                text,
            } => {
                assert_eq!(text, "");
                copied = true;
            }
            Event::Frame { screen, .. }
                if screen_has_line(&screen, "ATTACHED_PTY_RECEIVED_CONTROL") =>
            {
                control_received = true;
            }
            Event::Diagnostic(error) => bail!("{error}"),
            _ => {}
        }
    }
    assert_eq!(fs::read(stub.with_extension("byte"))?, b"\x03");
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    Ok(())
}

#[test]
fn shell_promotion_keeps_the_claude_card_when_cli_exits_after_the_startup_window() -> Result<()> {
    use matou_native_core::journal::for_each_frame;
    use matou_native_core::sessions::{get_session, list_runs};

    let root = tempfile::tempdir()?;
    let stub = root.path().join("promotion-cli");
    fs::write(
        &stub,
        r#"#!/bin/sh
while [ ! -e "$0.attached" ]; do /bin/sleep 0.02; done
printf 'CLI_WINDOW_PASSED\n'
/bin/sleep 0.2
exit 24
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
    child.input_bytes(b"claude\r")?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    // Start the fixture's 200 ms lifetime after the real PTY is attached.
    // This keeps process spawning and database setup out of the interval.
    child.frame(|_, screen, _| !screen_has_line(screen, PROMPT))?;
    assert_eq!(list_runs(&db, &child.session)?.len(), 2);
    fs::write(stub.with_extension("attached"), "")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let runs = list_runs(&db, &child.session)?;
        if runs
            .iter()
            .any(|run| run.profile == "claude-code" && run.exit_code == Some(24))
        {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "waiting for the CLI's exit record"
        );
        thread::sleep(Duration::from_millis(5));
    }
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    assert_eq!(
        get_session(&db, &child.session)?
            .context("exited card")?
            .kind,
        "claude-code"
    );
    assert_eq!(list_runs(&db, &child.session)?.len(), 2);
    let mut output = Vec::new();
    for_each_frame(&child.data, &child.session, 1, None, |record| {
        output.extend_from_slice(record.frame.output());
        Ok(())
    })?;
    assert!(String::from_utf8_lossy(&output).contains("CLI_WINDOW_PASSED"));
    Ok(())
}

#[test]
fn missing_claude_reports_diagnostic_and_restores_usable_shell() -> Result<()> {
    use matou_native_core::sessions::{get_session, list_runs};

    let root = tempfile::tempdir()?;
    let missing = root.path().join("missing-claude");
    let mut child = RuntimeChild::spawn_configured(
        root.path(),
        &format!("PROMPT='{PROMPT} '\n"),
        Path::new("/bin/zsh"),
        Some(&missing),
    )?;
    child.hello()?;
    let old_pid = child.shell_pid()?;

    child.input("claude")?;
    child.expect_diagnostic("未检测到 Claude Code")?;
    child.input("printf '%s\\n' SHELL_RESTORED")?;
    child.frame(|rows, _, _| rows.iter().any(|row| row == "SHELL_RESTORED"))?;
    assert_shell_gone(old_pid)?;
    let db = startup::open_read_only(&child.data.join("matou.sqlite"))?;
    let session = get_session(&db, &child.session)?.context("original Shell card")?;
    assert_eq!(
        (session.kind.as_str(), session.status.as_str()),
        ("shell", "running")
    );
    assert_eq!(
        list_runs(&db, &child.session)?
            .iter()
            .map(|run| run.profile.as_str())
            .collect::<Vec<_>>(),
        ["shell", "shell"]
    );

    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    Ok(())
}

#[test]
fn shell_promotion_rolls_back_an_immediate_cli_exit_and_replays_input() -> Result<()> {
    use matou_native_core::sessions::{get_session, list_runs};

    let root = tempfile::tempdir()?;
    let stub = root.path().join("exiting-cli");
    fs::write(&stub, "#!/bin/sh\nexit 23\n")?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut child = RuntimeChild::spawn_configured(
        root.path(),
        &format!("PROMPT='{PROMPT} '\n"),
        Path::new("/bin/zsh"),
        Some(&stub),
    )?;
    child.hello()?;
    let shell_pid = child.shell_pid()?;
    child.input_bytes(b"claude\r\xe4\xbd")?;
    child.input("printf '%s\\n' READY_AFTER_ROLLBACK")?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    child.expect_diagnostic("process exited with code 23")?;
    child
        .frame(|rows, _, _| rows.iter().any(|row| row == "READY_AFTER_ROLLBACK"))
        .with_context(|| format!("rollback runs: {:?}", list_runs(&db, &child.session)))?;
    assert_shell_gone(shell_pid)?;
    let session = get_session(&db, &child.session)?.context("original Shell card")?;
    assert_eq!(
        (&*session.kind, &*session.title, &*session.status),
        ("shell", "Shell", "running")
    );
    assert_eq!(session.title_source.as_deref(), Some("default"));
    let runs = list_runs(&db, &child.session)?;
    assert_eq!(
        runs.iter()
            .map(|run| run.profile.as_str())
            .collect::<Vec<_>>(),
        ["shell", "claude-code", "shell"]
    );
    assert_eq!(runs[1].exit_code, Some(23));
    child.input("claude")?;
    child.input("printf '%s\\n' READY_AFTER_SECOND_ROLLBACK")?;
    child.expect_diagnostic("process exited with code 23")?;
    child.frame(|rows, _, _| rows.iter().any(|row| row == "READY_AFTER_SECOND_ROLLBACK"))?;
    let runs = list_runs(&db, &child.session)?;
    assert_eq!(
        runs.iter()
            .map(|run| run.profile.as_str())
            .collect::<Vec<_>>(),
        ["shell", "claude-code", "shell", "claude-code", "shell"]
    );
    assert_eq!(runs[3].exit_code, Some(23));
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM provider_bindings", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(
        status.success() && stderr.contains("process exited with code 23"),
        "runtime shutdown: {stderr}"
    );
    Ok(())
}

#[test]
fn promoted_cli_replays_startup_observations_in_order_and_rejects_the_retired_run() -> Result<()> {
    use matou_native_core::journal::{Frame, for_each_frame};
    use matou_native_core::sessions::{claude_binding, list_runs};
    use serde_json::{Value, json};

    let root = tempfile::tempdir()?;
    let stub = root.path().join("promotion-cli");
    fs::write(
        &stub,
        r#"#!/bin/sh
if [ ! -e "$0.first-settings" ]; then
    printf '%s' "$2" > "$0.first-settings"
    while [ ! -e "$0.exit-first" ]; do /bin/sleep 0.01; done
    printf 'LATE_OUTPUT_FROM_RETIRED_RUN\n'
    exit 27
fi
printf '%s' "$2" > "$0.second-settings"
while [ ! -e "$0.release-output" ]; do /bin/sleep 0.01; done
printf 'NEW_RUN_OUTPUT\n'
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
    let read_settings_location = |suffix| -> Result<PathBuf> {
        let path = stub.with_extension(suffix);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Ok(value) = fs::read_to_string(&path)
                && Path::new(&value).is_file()
            {
                return Ok(value.into());
            }
            ensure!(Instant::now() < deadline, "waiting for {}", path.display());
            thread::sleep(Duration::from_millis(5));
        }
    };
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    child.input("claude")?;
    let old_settings = read_settings_location("first-settings")?;
    let old_hook = claude_hook_url(&old_settings)?;
    post_claude_identity(&old_settings, "never-bind-old", "default", "retired-model")?;
    fs::write(stub.with_extension("exit-first"), "")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Frame { screen, .. } = child
            .event(deadline, true)
            .context("waiting for the Shell after the first CLI exits")?
            && screen_has_line(&screen, PROMPT)
            && list_runs(&db, &child.session)?.len() == 3
        {
            break;
        }
    }
    child.input("claude")?;
    let new_settings = read_settings_location("second-settings")?;
    let retired_status = post_claude_hook_url(
        &old_hook,
        json!({
            "hook_event_name": "Notification", "session_id": "never-bind-old", "message": "Retired notice"
        }),
    )?;
    // During the grace period the server acknowledges retired requests and
    // reports the rejection. After that, the old URL is removed altogether.
    if retired_status == 200 {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Event::Graph(graph) = child
                .event(deadline, true)
                .context("waiting for rejection of the retired hook")?
                && graph["providerIdentityEvents"]
                    .as_array()
                    .is_some_and(|events| {
                        events.iter().any(|event| event["reason"] == "retired-run")
                    })
            {
                break;
            }
        }
    } else {
        assert_eq!(retired_status, 404);
    }
    for payload in [
        json!({"hook_event_name": "SessionStart", "session_id": "new-conversation"}),
        json!({"hook_event_name": "UserPromptSubmit", "session_id": "new-conversation",
               "model": "startup-model", "permission_mode": "plan"}),
        json!({"hook_event_name": "Notification", "session_id": "new-conversation",
               "model": "notification-model", "message": "Awaiting input", "notification_type":"permission_prompt"}),
    ] {
        assert_eq!(post_claude_hook(&new_settings, payload)?, 200);
    }
    assert!(claude_binding(&db, &child.session)?.is_none());
    fs::write(stub.with_extension("release-output"), "")?;
    let mut saw_output = false;
    let mut saw_observations = false;
    let deadline = Instant::now() + TIMEOUT;
    while !saw_output || !saw_observations {
        match child.event(deadline, true).with_context(|| {
            format!("replacement CLI output seen: {saw_output}, hook observations seen: {saw_observations}")
        })? {
            Event::Frame { screen, .. } => {
                assert!(!screen_has_line(&screen, "LATE_OUTPUT_FROM_RETIRED_RUN"));
                saw_output |= screen_has_line(&screen, "NEW_RUN_OUTPUT");
            }
            Event::Graph(graph) => {
                saw_observations |= graph["nodes"][0]["model"] == "notification-model"
                    && graph["nodes"][0]["permissionMode"] == "plan"
                    && graph["nodes"][0]["workStatus"] == "needs-input"
            }
            Event::Diagnostic(error) => bail!("{error}"),
            _ => {}
        }
    }
    let runs = list_runs(&db, &child.session)?;
    assert_eq!(runs[1].exit_code, Some(27));
    assert_eq!(runs[3].profile, "claude-code");
    let mut records = db.prepare(
        "SELECT event_type, payload_json FROM domain_events
         WHERE event_type IN ('provider-binding.recorded', 'agent.notification') ORDER BY seq",
    )?;
    let records = records
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .map(|row| -> Result<_> {
            let (kind, payload) = row?;
            Ok((kind, serde_json::from_str::<Value>(&payload)?))
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(records.len(), 4, "{records:?}");
    for (_, payload) in &records {
        assert_eq!(payload["runId"], runs[3].id);
    }
    assert_eq!(records[0].1["providerSessionId"], "new-conversation");
    assert_eq!(records[1].1["metadata"]["model"], "startup-model");
    assert_eq!(records[2].1["metadata"]["model"], "notification-model");
    assert_eq!(records[3].0, "agent.notification");
    assert_eq!(
        claude_binding(&db, &child.session)?
            .context("new binding")?
            .metadata["permissionMode"],
        "plan"
    );

    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    let mut markers = Vec::new();
    for_each_frame(&child.data, &child.session, 1, None, |record| {
        match record.frame {
            Frame::Output { data, .. } => {
                let output = String::from_utf8_lossy(&data);
                for marker in ["LATE_OUTPUT_FROM_RETIRED_RUN", "NEW_RUN_OUTPUT"] {
                    if output.contains(marker) {
                        markers.push(marker);
                    }
                }
            }
            Frame::Exit { exit_code: 27, .. } => markers.push("old-run-exited"),
            _ => {}
        }
        Ok(())
    })?;
    assert_eq!(
        markers,
        [
            "LATE_OUTPUT_FROM_RETIRED_RUN",
            "old-run-exited",
            "NEW_RUN_OUTPUT"
        ]
    );
    Ok(())
}

#[test]
fn shell_promotion_rollback_write_failure_leaves_a_failed_card_and_the_host_serves_another()
-> Result<()> {
    use matou_native_runtime::host::RecoveryState;

    let root = tempfile::tempdir()?;
    let stub = root.path().join("promotion-cli");
    fs::write(
        &stub,
        r#"#!/bin/sh
if [ ! -e "$0.started" ]; then
    /usr/bin/touch "$0.started"
    exit 29
fi
printf 'ANOTHER_PROMOTION_READY\n'
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
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    let failed = child.session.clone();
    let target: String = db.query_row("SELECT quote(?)", [&failed], |row| row.get(0))?;
    db.execute_batch(&format!(
        "CREATE TRIGGER reject_promotion_rollback BEFORE UPDATE OF kind ON sessions
         WHEN OLD.id = {target} AND OLD.kind = 'claude-code' AND NEW.kind = 'shell'
         BEGIN SELECT RAISE(ABORT, 'fixture rollback write failed'); END;",
    ))?;
    child.input("claude")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Recovery {
            session_id,
            state: RecoveryState::Failed(reason),
            ..
        } = child.event(deadline, true)?
            && session_id == failed
        {
            assert!(reason.contains("fixture rollback write failed"), "{reason}");
            break;
        }
    }
    child.send(Command::CreateShellSibling {
        scene_id: child.scene.clone(),
        source_session_id: failed.clone(),
        parent_session_id: None,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    let second = loop {
        if let Event::Frame {
            session_id, screen, ..
        } = child.event(deadline, true)?
            && session_id != failed
            && screen_has_line(&screen, PROMPT)
        {
            break session_id;
        }
    };
    child.session = second;
    child.input("claude")?;
    child.frame(|_, screen, _| screen_has_line(screen, "ANOTHER_PROMOTION_READY"))?;
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    Ok(())
}

#[test]
fn claude_role_budget_projects_the_actual_fresh_process_environment() -> Result<()> {
    use matou_native_runtime::host::RecoveryState;
    use serde_json::{Value, json};

    let absent = ["unset"; 5];
    for (scenario, assigned_role, mode, compact_window, expected) in [
        (
            "search excluded",
            Some("opus"),
            "maximum",
            Some(180_000),
            Ok(["260000", "90", "0", "unset", "unset"]),
        ),
        (
            "1M cap",
            Some("opus"),
            "maximum",
            None,
            Ok(["1000000", "90", "0", "unset", "unset"]),
        ),
        (
            "fallback tier limits",
            Some("opus"),
            "512k",
            None,
            Ok(["200000", "90", "1", "unset", "unset"]),
        ),
        (
            "100K",
            Some("opus"),
            "512k",
            Some(100_000),
            Ok(["100000", "90", "1", "unset", "unset"]),
        ),
        (
            "under 100K",
            Some("opus"),
            "512k",
            Some(99_999),
            Err("compaction · Compaction API: context window is under 100K"),
        ),
        ("unknown", Some("opus"), "512k", None, Ok(absent)),
        (
            "under 100K with an assignment on another tier",
            Some("sonnet"),
            "512k",
            Some(99_999),
            Err("compaction · Compaction API: context window is under 100K"),
        ),
        ("no assignments", None, "512k", Some(99_999), Ok(["old"; 5])),
    ] {
        let root = tempfile::tempdir()?;
        let no_shell = root.path().join("missing-shell");
        let mut seed = RuntimeChild::spawn_configured(root.path(), "", &no_shell, None)?;
        seed.ready()?;
        seed.send(Command::Shutdown)?;
        seed.closed()?;
        let (status, stderr) = seed.wait()?;
        ensure!(
            status.success(),
            "{scenario}: seed runtime shutdown: {stderr}"
        );

        let metadata = |id, window, max| {
            if scenario == "unknown" {
                json!({"id": id})
            } else {
                json!({"id": id, "contextWindow": window, "maxContextWindow": max})
            }
        };
        fs::write(seed.data.join("provider-config.json"), json!({
            "version": 1,
            "activeProviderIds": {"claude-code": "source"},
            "providers": {"claude-code": [
                {"id": "source", "name": "Source API", "model": "main",
                 "endpoint": "http://127.0.0.1:1/source", "apiKey": "fixture",
                 "modelMapping": {"opus": "main", "sonnet": "medium", "haiku": "small"},
                 "modelMetadata": [metadata("main", 300_000, 1_500_000),
                                   metadata("medium", 200_000, 1_200_000),
                                   metadata("small", 600_000, 1_300_000)]},
                {"id": "assigned", "name": "Assigned API", "model": "chosen",
                 "endpoint": "http://127.0.0.1:1/assigned", "apiKey": "fixture",
                 "modelMetadata": [metadata("chosen", 800_000, 1_600_000)]},
                {"id": "compact", "name": "Compaction API", "model": "summary",
                 "endpoint": "http://127.0.0.1:1/compact", "apiKey": "fixture",
                 "modelMetadata": [metadata("summary", compact_window.unwrap_or(90_000), 260_000)]},
                {"id": "search", "name": "Search API", "model": "search-model",
                 "endpoint": "http://127.0.0.1:1/search", "apiKey": "fixture",
                 "modelMetadata": [metadata("search-model", 32_000, 32_000)]}
            ]}
        }).to_string())?;
        let compaction = if compact_window.is_some() {
            json!({"mode": "custom", "providerId": "compact", "model": "summary", "effort": "follow"})
        } else {
            json!({"mode": "follow", "effort": "follow"})
        };
        let mut assignments = json!({});
        if let Some(role) = assigned_role {
            assignments[role] = json!({"providerId": "assigned", "model": "chosen"});
        }
        let assigned_mode = if scenario == "search excluded" {
            "512k"
        } else {
            mode
        };
        let mut prefs: Value = json!({
            "claudeRoleAssignments": assignments,
            "compaction": compaction,
            "contextWindows": {"source": mode, "assigned": assigned_mode, "compact": mode}
        });
        if scenario == "search excluded" {
            prefs["claudeWebSearch"] = json!({"providerId": "search", "model": "search-model"});
        }
        let db = Connection::open(seed.data.join("matou.sqlite"))?;
        db.execute(
            "UPDATE sessions SET kind='claude-code' WHERE id=?",
            [&seed.session],
        )?;
        db.execute(
            "INSERT INTO preferences VALUES ('agent.modelPreferences', ?, 1)",
            [prefs.to_string()],
        )?;
        drop(db);

        let stub = root.path().join("claude-role-budget-cli");
        fs::write(
            &stub,
            r#"#!/bin/sh
printf '%s\n' "${CLAUDE_CODE_MAX_CONTEXT_TOKENS-unset}" "${CLAUDE_AUTOCOMPACT_PCT_OVERRIDE-unset}" \
    "${CLAUDE_CODE_DISABLE_1M_CONTEXT-unset}" "${CLAUDE_CODE_MAX_OUTPUT_TOKENS-unset}" \
    "${CLAUDE_CODE_AUTO_COMPACT_WINDOW-unset}" > "$0.env"
printf 'budget process ready\n'
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let mut child = RuntimeChild::spawn_configured_with(
            root.path(),
            "",
            &no_shell,
            Some(&stub),
            |command| {
                for key in [
                    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
                    "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
                    "CLAUDE_CODE_DISABLE_1M_CONTEXT",
                    "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
                ] {
                    command.env(key, "old");
                }
            },
        )?;
        child.connect()?;
        assert_eq!(child.session, seed.session, "{scenario}");
        let environment = stub.with_extension("env");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match child
                .event(deadline, true)
                .with_context(|| scenario.to_owned())?
            {
                Event::Frame {
                    session_id, screen, ..
                } if session_id == child.session
                    && screen_has_line(&screen, "budget process ready") =>
                {
                    let expected = expected.expect("under-limit budget must stop the spawn");
                    let actual = fs::read_to_string(&environment)?;
                    assert_eq!(actual.lines().collect::<Vec<_>>(), expected, "{scenario}");
                    break;
                }
                Event::Recovery {
                    session_id,
                    state: RecoveryState::StartFailed(reason),
                    ..
                } if session_id == child.session => {
                    let expected = expected.expect_err(&reason);
                    assert!(reason.ends_with(expected), "{scenario}: {reason}");
                    assert!(
                        !environment.exists(),
                        "{scenario}: CLI spawned before budget check"
                    );
                    break;
                }
                _ => {}
            }
        }
        child.send(Command::Shutdown)?;
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "{scenario}: runtime shutdown: {stderr}");
    }
    Ok(())
}

#[test]
fn web_search_settings_preserve_replace_clear_and_validate_atomically() -> Result<()> {
    use serde_json::{Value, json};

    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let mut child =
        RuntimeChild::spawn_configured(root.path(), "", &root.path().join("no-shell"), None)?;
    child.ready()?;
    let config = json!({
        "version": 1,
        "revision": 1,
        "activeProviderIds": {"claude-code": "work"},
        "providers": {
            "claude-code": [
                {"id": "work", "name": "工作供应商", "endpoint": "http://127.0.0.1:1/a",
                 "apiKey": "work-fixture", "model": "work-opus",
                 "modelMapping": {"opus": "work-opus", "sonnet": "work-sonnet", "haiku": "work-haiku"}},
                {"id": "search", "name": "搜索供应商", "endpoint": "http://127.0.0.1:1/b",
                 "apiKey": "search-fixture", "model": "exact",
                 "modelMetadata": [{"id": "exact", "supportedEfforts": ["high"]}]},
                {"id": "empty", "name": "空目录", "endpoint": "http://127.0.0.1:1/c",
                 "apiKey": "empty-fixture", "model": "exact", "modelMetadata": []},
                {"id": "no-key", "name": "缺少 Key", "endpoint": "http://127.0.0.1:1/d", "model": "exact"}
            ],
            "codex": [{"id": "other-cli", "name": "非 Claude", "endpoint": "http://127.0.0.1:1/e",
                       "apiKey": "other-fixture", "model": "exact"}]
        }
    });
    fs::write(child.data.join("provider-config.json"), config.to_string())?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    let read_row = || -> Result<Value> {
        let json: String = db.query_row(
            "SELECT value_json FROM preferences WHERE key='agent.modelPreferences'",
            [],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&json)?)
    };
    let send = |child: &mut RuntimeChild, id, command| -> Result<Value> {
        child.send(command)?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Event::Graph(graph) = child.event(deadline, true)?
                && let Some(result) = graph.get("claudeRoleAssignmentsResult")
            {
                let reply: ClaudeRoleAssignmentsReply = serde_json::from_value(result.clone())?;
                assert_eq!(reply.request_id, id);
                assert!(!result.to_string().contains("-fixture"));
                return Ok(result.clone());
            }
        }
    };
    let set = |child: &mut RuntimeChild, id, assignments: Value, web_search: Option<Value>| {
        let mut wire =
            json!({"SetClaudeRoleAssignments": {"request_id": id, "assignments": assignments}});
        if let Some(target) = web_search {
            wire["SetClaudeRoleAssignments"]["webSearch"] = target;
        }
        let command: Command = serde_json::from_value(wire.clone())?;
        assert_eq!(serde_json::to_value(&command)?, wire);
        send(child, id, command)
    };
    let get = |child: &mut RuntimeChild, id| {
        send(
            child,
            id,
            Command::GetClaudeRoleAssignments { request_id: id },
        )
    };
    assert!(get(&mut child, 1)?.get("webSearch").is_none());
    let role = json!({"opus": {"providerId": "work", "model": "work-opus"}});
    let target = json!({"providerId": "search", "model": "exact", "effort": "high"});
    let reply = set(
        &mut child,
        2,
        role.clone(),
        Some(json!({"providerId": " search ", "model": " exact ", "effort": "high"})),
    )?;
    assert_eq!(reply["assignments"], role);
    assert_eq!(reply["webSearch"], target);
    assert_eq!(read_row()?["claudeWebSearch"], target);
    let old_client_reply = set(&mut child, 3, json!({}), None)?;
    assert_eq!(old_client_reply["webSearch"], target);
    assert_eq!(old_client_reply["assignments"], json!({}));
    assert_eq!(get(&mut child, 4)?["webSearch"], target);
    let before = read_row()?;
    for (index, invalid, expected) in [
        (
            0,
            json!({"providerId": "search", "model": "ex act"}),
            "模型已不可用",
        ),
        (
            1,
            json!({"providerId": "search", "model": "exact\u{7f}"}),
            "模型已不可用",
        ),
        (
            2,
            json!({"providerId": "search", "model": "exact\u{85}"}),
            "模型已不可用",
        ),
        (
            3,
            json!({"providerId": "search\t", "model": "exact"}),
            "模型已不可用",
        ),
        (
            4,
            json!({"providerId": "x".repeat(257), "model": "exact"}),
            "模型已不可用",
        ),
        (
            5,
            json!({"providerId": "search", "model": "𠀀".repeat(121)}),
            "模型已不可用",
        ),
        (
            6,
            json!({"providerId": "other-cli", "model": "exact"}),
            "供应商已不存在",
        ),
        (
            7,
            json!({"providerId": "no-key", "model": "exact"}),
            "API Key 未配置",
        ),
        (
            8,
            json!({"providerId": "search", "model": "EXACT"}),
            "模型已不可用",
        ),
        (
            9,
            json!({"providerId": "empty", "model": "exact"}),
            "模型目录暂不可用",
        ),
        (
            10,
            json!({"providerId": "search", "model": "exact", "effort": "max"}),
            "当前模型不支持所选推理强度",
        ),
        (
            11,
            json!({"providerId": "search", "model": "exact", "effort": null}),
            "模型已不可用",
        ),
    ] {
        let reply = set(&mut child, 20 + index, role.clone(), Some(invalid))?;
        assert!(
            reply["error"]
                .as_str()
                .is_some_and(|error| error.starts_with(expected)),
            "{reply}"
        );
        assert!(reply.get("assignments").is_none() && reply.get("webSearch").is_none());
        assert_eq!(
            read_row()?,
            before,
            "invalid save changed a sibling assignment"
        );
    }
    let manual = json!({"providerId": "work", "model": "𠀀".repeat(120), "effort": "follow"});
    assert_eq!(
        set(&mut child, 5, role.clone(), Some(manual.clone()))?["webSearch"],
        manual
    );
    assert_eq!(
        set(&mut child, 6, role.clone(), Some(target.clone()))?["webSearch"],
        target
    );

    child.send(Command::DeleteProvider {
        request_id: 99,
        cli: "claude-code".into(),
        id: "search".into(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(result) = graph.get("providerSettingsResult")
        {
            assert_eq!(result["requestId"], 99);
            assert!(result.get("error").is_none(), "{result}");
            break;
        }
    }
    assert_eq!(get(&mut child, 7)?["webSearch"], target);
    let cleared = set(&mut child, 8, role.clone(), Some(Value::Null))?;
    assert_eq!(cleared.get("webSearch"), Some(&Value::Null));
    assert_eq!(
        serde_json::from_value::<ClaudeRoleAssignmentsReply>(cleared)?.web_search,
        Some(None)
    );
    assert_eq!(get(&mut child, 9)?["assignments"], role);
    assert!(get(&mut child, 10)?.get("webSearch").is_none());
    assert!(read_row()?.get("claudeWebSearch").is_none());

    db.execute(
        "UPDATE preferences SET value_json=json_set(value_json, '$.claudeWebSearch', json(?)) WHERE key='agent.modelPreferences'",
        [json!({"providerId": "work", "model": false}).to_string()],
    )?;
    let corrupted = get(&mut child, 11)?;
    assert_eq!(corrupted["assignments"], json!({}));
    assert!(corrupted.get("webSearch").is_none());
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    Ok(())
}

#[test]
fn first_claude_start_and_recovery_without_binding_use_saved_opus_assignment() -> Result<()> {
    use matou_native_core::sessions::{claude_binding, get_session, list_runs};
    use matou_native_runtime::host::RecoveryState;
    use serde_json::{Value, json};

    let root = tempfile::tempdir()?;
    let missing_shell = root.path().join("missing-shell");
    let mut child = RuntimeChild::spawn_configured(root.path(), "", &missing_shell, None)?;
    child.ready()?;
    let deadline = Instant::now() + TIMEOUT;
    while !matches!(child.event(deadline, true)?, Event::Recovery { .. }) {}
    let config = child.data.join("provider-config.json");
    let stored_config = json!({
        "version": 1,
        "providers": {"claude-code": [
            {
                "id": "anthropic-official", "builtIn": true, "model": "",
                "endpoint": "https://api.anthropic.com"
            },
            {
                "id": "target", "model": "target-default", "apiKey": "target-fixture-key",
                "endpoint": "https://target.example.invalid/",
                "modelMapping": {"opus": "target-opus", "sonnet": "target-sonnet", "haiku": "target-haiku"},
                "modelMetadata": [{"id": "assigned-model", "supportedEfforts": ["high"]}]
            }
        ]},
        "activeProviderIds": {"claude-code": "anthropic-official"}
    }).to_string();
    fs::write(&config, &stored_config)?;
    let exchange =
        |child: &mut RuntimeChild, request_id, command| -> Result<ClaudeRoleAssignmentsReply> {
            child.send(command)?;
            let deadline = Instant::now() + TIMEOUT;
            loop {
                if let Event::Graph(graph) = child.event(deadline, true)?
                    && let Some(value) = graph.get("claudeRoleAssignmentsResult")
                {
                    let raw = value.to_string();
                    assert!(!raw.contains("fixture-key"));
                    assert!(!raw.contains("apiKey"));
                    assert!(!raw.contains("example.invalid"));
                    let fields = value.as_object().context("flat reply object")?;
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields.get("requestId"), Some(&json!(request_id)));
                    match (fields.get("assignments"), fields.get("error")) {
                        (Some(assignments), None) => assert!(assignments.is_object()),
                        (None, Some(error)) => assert!(error.is_string()),
                        _ => panic!("expected assignments or error in the Graph reply: {value}"),
                    }
                    let reply: ClaudeRoleAssignmentsReply = serde_json::from_value(value.clone())?;
                    return Ok(reply);
                }
            }
        };
    let get = |child: &mut RuntimeChild, request_id| {
        exchange(
            child,
            request_id,
            Command::GetClaudeRoleAssignments { request_id },
        )
    };
    let set = |child: &mut RuntimeChild, request_id, assignments| {
        exchange(
            child,
            request_id,
            Command::SetClaudeRoleAssignments {
                request_id,
                web_search: None,
                assignments,
            },
        )
    };
    assert!(get(&mut child, 1)?.assignments.unwrap().is_empty());
    let official =
        json!({"opus": {"providerId": "anthropic-official", "model": "opus", "effort": "max"}});
    assert_eq!(
        serde_json::to_value(set(&mut child, 8, official.clone())?.assignments.unwrap())?,
        official,
    );
    let mut custom_official: Value = serde_json::from_str(&stored_config)?;
    custom_official["providers"]["claude-code"][0]["endpoint"] =
        json!("https://custom.example.invalid");
    fs::write(&config, custom_official.to_string())?;
    assert_eq!(
        set(&mut child, 13, official)?.error.unwrap(),
        "opus assignment: Assignment API key is unavailable"
    );
    fs::write(&config, &stored_config)?;
    let assignment =
        json!({"opus": {"providerId": "target", "model": "assigned-model", "effort": "high"}});
    let mut draft = assignment.clone();
    draft["opus"]["apiKey"] = json!("ignored-fixture-key");
    let saved = set(&mut child, 2, draft)?.assignments.unwrap();
    assert_eq!(serde_json::to_value(&saved)?, assignment);
    for (request, provider, model, effort, reason) in [
        (
            3,
            "absent",
            "assigned-model",
            "high",
            "provider is unavailable",
        ),
        (4, "target", " ", "high", "model is empty"),
        (
            5,
            "target",
            "assigned-model",
            "max",
            "Unsupported reasoning effort",
        ),
    ] {
        let error = set(
            &mut child,
            request,
            json!({"opus": {
                "providerId": provider, "model": model, "effort": effort
            }}),
        )?
        .error
        .unwrap();
        assert!(error.starts_with("opus assignment:"), "{error}");
        assert!(error.contains(reason), "{error}");
    }
    assert_eq!(get(&mut child, 6)?.assignments.unwrap(), saved);
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    let mut malformed = assignment.clone();
    malformed["sonnet"] = json!({"providerId": "target", "model": false});
    db.execute(
        "UPDATE preferences SET value_json=json_set(value_json, '$.claudeRoleAssignments', json(?)) WHERE key='agent.modelPreferences'",
        [malformed.to_string()],
    )?;
    assert!(get(&mut child, 9)?.assignments.unwrap().is_empty());
    assert_eq!(
        set(&mut child, 10, assignment.clone())?
            .assignments
            .unwrap(),
        saved
    );
    db.execute(
        "UPDATE preferences SET value_json=json_set(value_json, '$.contextWindows.target', 'invalid') WHERE key='agent.modelPreferences'",
        [],
    )?;
    assert!(get(&mut child, 11)?.assignments.unwrap().is_empty());
    assert_eq!(
        set(&mut child, 12, assignment.clone())?
            .assignments
            .unwrap(),
        saved
    );
    db.execute(
        "UPDATE preferences SET value_json = CAST(x'ff' AS TEXT) WHERE key='agent.modelPreferences'",
        [],
    )?;
    assert!(get(&mut child, 14)?.assignments.unwrap().is_empty());
    assert_eq!(
        set(&mut child, 15, assignment.clone())?
            .assignments
            .unwrap(),
        saved
    );
    let repaired: String = db.query_row(
        "SELECT value_json FROM preferences WHERE key='agent.modelPreferences'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        serde_json::from_str::<Value>(&repaired)?,
        json!({
            "contextWindows": {},
            "compaction": {"mode": "follow", "effort": "follow"},
            "claudeRoleAssignments": assignment
        })
    );
    drop(db);
    assert_eq!(fs::read_to_string(&config)?, stored_config);
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");

    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.execute(
        "UPDATE sessions SET kind = 'claude-code' WHERE id = ?",
        [&child.session],
    )?;
    drop(db);
    let stub = root.path().join("claude-role-launch-stub");
    fs::write(
        &stub,
        r#"#!/bin/sh
if [ ! -f "$0.first-argv" ]; then
    printf '%s\n' "$@" > "$0.first-argv"
    exit 23
fi
printf '%s\n' "$@" > "$0.argv"
printf '%s\n' "$ANTHROPIC_BASE_URL" "$ANTHROPIC_API_KEY" "$ANTHROPIC_AUTH_TOKEN" \
    "$ANTHROPIC_DEFAULT_OPUS_MODEL" "$ANTHROPIC_DEFAULT_SONNET_MODEL" \
    "$ANTHROPIC_DEFAULT_HAIKU_MODEL" "$ANTHROPIC_DEFAULT_FABLE_MODEL" \
    "${CLAUDE_CODE_SUBAGENT_MODEL-unset}" "${ANTHROPIC_MODEL-unset}" > "$0.env"
case "$*" in
  *'--permission-mode acceptEdits'*) printf 'resumed permission process ready\n';;
  *'--permission-mode plan'*) printf 'permission process ready\n';;
  *) printf 'assigned process ready\n';;
esac
exec /bin/sleep 60
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut unavailable: Value = serde_json::from_str(&stored_config)?;
    unavailable["providers"]["claude-code"][1]["modelMetadata"] = json!([{"id": "replacement"}]);
    fs::write(&config, unavailable.to_string())?;
    let spawn_assigned = || {
        RuntimeChild::spawn_configured_with(
            root.path(),
            "",
            &missing_shell,
            Some(&stub),
            |command| {
                command
                    .env("ANTHROPIC_DEFAULT_FABLE_MODEL", "old-fable")
                    .env("CLAUDE_CODE_SUBAGENT_MODEL", "old-subagent")
                    .env("ANTHROPIC_MODEL", "old-main");
            },
        )
    };
    let mut next = spawn_assigned()?;
    next.connect()?;
    assert_eq!(next.session, child.session);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Recovery {
            session_id,
            state: RecoveryState::StartFailed(reason),
            ..
        } = next.event(deadline, true)?
            && session_id == next.session
        {
            assert!(
                reason.contains("opus assignment: Assignment model is unavailable"),
                "{reason}"
            );
            break;
        }
    }
    assert!(!stub.with_extension("first-argv").exists());
    let db = Connection::open(next.data.join("matou.sqlite"))?;
    let status: String = db.query_row(
        "SELECT status FROM sessions WHERE id=?",
        [&next.session],
        |row| row.get(0),
    )?;
    assert_eq!(status, "created");
    assert_eq!(get(&mut next, 7)?.assignments.unwrap(), saved);
    fs::write(&config, &stored_config)?;
    next.send(Command::RestartSession {
        session_id: next.session.clone(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    while get_session(&db, &next.session)?
        .context("Claude card")?
        .status
        != "exited"
    {
        ensure!(
            Instant::now() < deadline,
            "first Claude process did not exit"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let runs = list_runs(&db, &next.session)?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].exit_code, Some(23));
    assert!(claude_binding(&db, &next.session)?.is_none());
    let first_argv = fs::read_to_string(stub.with_extension("first-argv"))?;
    assert_eq!(
        first_argv.lines().skip(2).collect::<Vec<_>>(),
        ["--model", "assigned-model", "--effort", "high"]
    );
    next.send(Command::Shutdown)?;
    next.closed()?;
    let (status, stderr) = next.wait()?;
    ensure!(
        status.success(),
        "runtime shutdown after first Claude exit: {stderr}"
    );

    let mut next = spawn_assigned()?;
    next.connect()?;
    assert_eq!(next.session, child.session);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Recovery {
            session_id,
            state: RecoveryState::Deferred,
            ..
        } = next.event(deadline, true)?
            && session_id == next.session
        {
            break;
        }
    }
    assert_eq!(list_runs(&db, &next.session)?.len(), 1);
    next.send(Command::RetryRecovery {
        session_id: next.session.clone(),
    })?;
    next.frame(|_, screen, _| screen_has_line(screen, "assigned process ready"))?;
    assert_eq!(list_runs(&db, &next.session)?.len(), 2);
    let argv = fs::read_to_string(stub.with_extension("argv"))?;
    let argv: Vec<_> = argv.lines().collect();
    assert_eq!(argv.len(), 6);
    assert_eq!(argv[0], "--settings");
    assert!(Path::new(argv[1]).is_file());
    assert_eq!(argv[2..], ["--model", "assigned-model", "--effort", "high"]);
    let env = fs::read_to_string(stub.with_extension("env"))?;
    assert_eq!(
        env.lines().collect::<Vec<_>>(),
        [
            "https://target.example.invalid",
            "",
            "target-fixture-key",
            "assigned-model",
            "target-sonnet",
            "target-haiku",
            "assigned-model",
            "unset",
            "unset",
        ]
    );
    let prefs: String = db.query_row(
        "SELECT value_json FROM preferences WHERE key = 'agent.modelPreferences'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        serde_json::from_str::<Value>(&prefs)?,
        json!({
            "contextWindows": {},
            "compaction": {"mode": "follow", "effort": "follow"},
            "claudeRoleAssignments": assignment
        })
    );
    assert!(claude_binding(&db, &next.session)?.is_none());
    // The replacement must still use the initial target after preferences change.
    assert!(
        set(&mut next, 16, json!({}))?
            .assignments
            .unwrap()
            .is_empty()
    );
    let mut edited_config: Value = serde_json::from_str(&stored_config)?;
    let target = &mut edited_config["providers"]["claude-code"][1];
    // The retained explicit effort survives changes to the provider's
    // fallback. Keep this direct-launch fixture on follow for other models.
    target["effortMapping"] = json!({"opus": "follow"});
    target["reasoningEffort"] = json!("follow");
    fs::write(&config, edited_config.to_string())?;
    next.send(Command::SetProviderPermissionMode {
        session_id: next.session.clone(),
        mode: "plan".into(),
    })?;
    next.frame(|_, screen, _| screen_has_line(screen, "permission process ready"))?;
    assert_eq!(
        fs::read_to_string(stub.with_extension("argv"))?
            .lines()
            .skip(2)
            .collect::<Vec<_>>(),
        [
            "--model",
            "assigned-model",
            "--effort",
            "high",
            "--permission-mode",
            "plan"
        ]
    );
    assert_eq!(fs::read_to_string(stub.with_extension("env"))?, env);
    assert!(claude_binding(&db, &next.session)?.is_none());
    let settings_path = || -> Result<_> {
        let argv = fs::read_to_string(stub.with_extension("argv"))?;
        Ok(Path::new(argv.lines().nth(1).context("settings arg")?).to_owned())
    };
    post_claude_identity(&settings_path()?, "assigned-id", "plan", "assigned-model")?;
    assert!(
        permission_mode_result(&mut next, "plan")?
            .get("error")
            .is_none()
    );
    let binding = claude_binding(&db, &next.session)?.context("assigned binding")?;
    assert!(binding.resumable);
    assert_eq!(binding.metadata["providerConfigId"], "target");
    assert_eq!(binding.metadata["model"], "assigned-model");
    assert_eq!(binding.metadata["assignedRole"], "opus");
    assert_eq!(binding.metadata["effort"], "high");
    assert!(!binding.metadata.to_string().contains("fixture-key"));

    next.send(Command::SetProviderPermissionMode {
        session_id: next.session.clone(),
        mode: "acceptEdits".into(),
    })?;
    next.frame(|_, screen, _| screen_has_line(screen, "resumed permission process ready"))?;
    assert_eq!(
        fs::read_to_string(stub.with_extension("argv"))?
            .lines()
            .skip(2)
            .collect::<Vec<_>>(),
        [
            "--model",
            "assigned-model",
            "--effort",
            "high",
            "--resume",
            "assigned-id",
            "--permission-mode",
            "acceptEdits"
        ]
    );
    assert_eq!(fs::read_to_string(stub.with_extension("env"))?, env);
    post_claude_identity(
        &settings_path()?,
        "assigned-id",
        "acceptEdits",
        "assigned-model",
    )?;
    assert!(
        permission_mode_result(&mut next, "acceptEdits")?
            .get("error")
            .is_none()
    );
    next.send(Command::Shutdown)?;
    next.closed()?;
    let (status, stderr) = next.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");
    Ok(())
}

#[test]
fn claude_restore_uses_binding_and_start_fresh_survives_deleted_provider() -> Result<()> {
    use matou_native_runtime::host::RecoveryState;
    use serde_json::json;

    let root = tempfile::tempdir()?;
    let no_shell = root.path().join("missing-shell");
    let mut seed = RuntimeChild::spawn_configured(root.path(), "", &no_shell, None)?;
    seed.ready()?;
    seed.send(Command::Shutdown)?;
    seed.closed()?;
    let (status, stderr) = seed.wait()?;
    ensure!(status.success(), "seed runtime shutdown: {stderr}");

    let config = seed.data.join("provider-config.json");
    let mut providers = json!({
            "version": 1,
            "providers": {"claude-code": [
                {"id": "active", "model": "opus", "endpoint": "https://active.example.invalid"},
                {
                    "id": "bound", "model": "claude-opus-5-5", "reasoningEffort": "low",
                    "endpoint": "https://bound.example.invalid"
                },
                {
                    "id": "target", "model": "target-default", "apiKey": "target-fixture-key",
                    "endpoint": "https://target.example.invalid"
                }
            ]},
            "activeProviderIds": {"claude-code": "active"}
    });
    fs::write(&config, providers.to_string())?;
    let db = Connection::open(seed.data.join("matou.sqlite"))?;
    db.execute(
        "UPDATE sessions SET kind='claude-code' WHERE id=?",
        [&seed.session],
    )?;
    db.execute(
        "INSERT INTO provider_bindings
            (id, session_id, provider, provider_session_id, resume_state, restore_state,
             metadata_json, created_at, updated_at, validated_at)
         VALUES ('saved-binding', ?, 'claude-code', 'saved-conversation', 'available', 'restoring',
                 '{\"providerConfigId\":\"bound\",\"model\":\"claude-opus-5-5\",
                   \"permissionMode\":\"plan\",\"canFork\":true,
                   \"observedUserPrompt\":true,\"observedNormalStop\":true}', 1, 1, 1)",
        [&seed.session],
    )?;
    db.execute(
        "INSERT INTO preferences (key, value_json, updated_at) VALUES ('agent.modelPreferences', ?, 1)",
        [json!({
            "contextWindows": {}, "compaction": {"mode": "follow", "effort": "follow"},
            "claudeRoleAssignments": {
                "opus": {"providerId": "target", "model": "assigned-model", "effort": "high"}
            }
        })
        .to_string()],
    )?;
    let stub = root.path().join("claude-restore-stub");
    fs::write(
        &stub,
        r#"#!/bin/sh
for arg in "$@"; do
    if [ "$arg" = '--resume' ]; then
        printf '%s\n' "$@" > "$0.resume-argv"
        /bin/cp "$2" "$0.resume-settings"
        exit 23
    fi
done
printf '%s\n' "$@" > "$0.fresh-argv"
printf '%s\n' "$ANTHROPIC_BASE_URL" > "$0.endpoint"
printf 'fresh process ready\n'
exec /bin/sleep 60
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut child = RuntimeChild::spawn_configured(root.path(), "", &no_shell, Some(&stub))?;
    child.connect()?;
    assert_eq!(child.session, seed.session);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Recovery {
            session_id,
            state: RecoveryState::Failed(_),
            ..
        } = child.event(deadline, true)?
            && session_id == child.session
        {
            break;
        }
    }
    let argv = fs::read_to_string(stub.with_extension("resume-argv"))?;
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(stub.with_extension("resume-settings"))?)?;
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["type"],
        "command"
    );
    assert_eq!(
        settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["type"],
        "http"
    );
    assert_eq!(
        argv.lines().skip(2).collect::<Vec<_>>(),
        [
            "--model",
            "claude-opus-5-5",
            "--effort",
            "low",
            "--resume",
            "saved-conversation",
            "--permission-mode",
            "plan"
        ]
    );
    assert!(!stub.with_extension("fresh-argv").exists());

    providers["providers"]["claude-code"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    fs::write(&config, providers.to_string())?;
    child.send(Command::StartFreshProviderSession {
        session_id: child.session.clone(),
    })?;
    child.frame(|_, screen, _| screen_has_line(screen, "fresh process ready"))?;
    let states: (String, String) = db.query_row(
        "SELECT resume_state, restore_state FROM provider_bindings WHERE id='saved-binding'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(states, ("expired".into(), "none".into()));
    child.send(Command::Refresh)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(node) = graph["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|node| {
                    node["sessionId"] == child.session && node["providerRestoreState"] == "none"
                })
        {
            assert_eq!(node["canFork"], true);
            break;
        }
    }
    let argv = fs::read_to_string(stub.with_extension("fresh-argv"))?;
    assert_eq!(
        argv.lines().skip(2).collect::<Vec<_>>(),
        [
            "--model",
            "assigned-model",
            "--effort",
            "high",
            "--permission-mode",
            "plan"
        ]
    );
    assert_eq!(
        fs::read_to_string(stub.with_extension("endpoint"))?.trim(),
        "https://target.example.invalid"
    );
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");
    Ok(())
}

#[test]
fn start_fresh_uses_the_queued_assignment_or_the_active_provider() -> Result<()> {
    use serde_json::{Value, json};

    for scenario in [
        "save",
        "cleared",
        "never configured, old provider deleted",
        "invalid JSON",
        "invalid UTF-8",
    ] {
        let root = tempfile::tempdir()?;
        let no_shell = root.path().join("missing-shell");
        let mut seed = RuntimeChild::spawn_configured(root.path(), "", &no_shell, None)?;
        seed.ready()?;
        seed.send(Command::Shutdown)?;
        seed.closed()?;
        let (status, stderr) = seed.wait()?;
        ensure!(status.success(), "seed runtime shutdown: {stderr}");
        let mut providers = vec![json!({
            "id": "active", "model": "active-main", "endpoint": "https://active.example.invalid",
            "apiKey": "active-fixture-key", "modelMapping": {"opus": "active-main"}
        })];
        if scenario != "never configured, old provider deleted" {
            providers.push(json!({
                "id": "target", "model": "target-default", "endpoint": "https://target.example.invalid",
                "apiKey": "target-fixture-key", "modelMapping": {"sonnet": "last-assigned-sonnet"},
                "modelMetadata": [{"id": "next-opus"}, {"id": "next-sonnet"}]
            }));
        }
        let config = json!({
            "version": 1,
            "providers": {"claude-code": providers},
            "activeProviderIds": {"claude-code": "active"}
        });
        fs::write(seed.data.join("provider-config.json"), config.to_string())?;
        let db = Connection::open(seed.data.join("matou.sqlite"))?;
        db.execute(
            "UPDATE sessions SET kind='claude-code' WHERE id=?",
            [&seed.session],
        )?;
        db.execute(
            "INSERT INTO provider_bindings
                (id, session_id, provider, provider_session_id, resume_state, restore_state,
                 metadata_json, created_at, updated_at, validated_at)
             VALUES ('old-binding', ?, 'claude-code', 'old-conversation', 'failed', 'failed',
                     '{\"providerConfigId\":\"target\",\"model\":\"last-assigned-sonnet\",\"permissionMode\":\"acceptEdits\"}', 1, 1, 1)",
            [&seed.session],
        )?;
        let stub = root.path().join("claude-queued-save-stub");
        fs::write(
            &stub,
            r#"#!/bin/sh
printf '%s\n' "$@" > "$0.argv"
printf '%s\n' "$ANTHROPIC_BASE_URL" > "$0.endpoint"
printf 'fresh process ready\n'
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        // The explicit proxy must leave the queued assignment's gateway enabled.
        let mut child = RuntimeChild::spawn_configured_with(
            root.path(),
            "",
            &no_shell,
            Some(&stub),
            |command| {
                command
                    .env("HTTPS_PROXY", "http://127.0.0.1:1")
                    .env("NO_PROXY", "127.0.0.1")
                    .env("no_proxy", "127.0.0.1");
            },
        )?;
        child.connect()?;
        assert_eq!(child.session, seed.session);

        let assignments = json!({
            "opus": {"providerId": "target", "model": "next-opus", "effort": "high"},
            "sonnet": {"providerId": "target", "model": "next-sonnet", "effort": "follow"}
        });
        match scenario {
            "cleared" => db.execute(
                "INSERT INTO preferences VALUES ('agent.modelPreferences', ?, 1)",
                [json!({
                    "contextWindows": {},
                    "compaction": {"mode": "follow", "effort": "follow"},
                    "claudeRoleAssignments": assignments
                })
                .to_string()],
            )?,
            "invalid JSON" => db.execute(
                "INSERT INTO preferences VALUES ('agent.modelPreferences', '{', 1)",
                [],
            )?,
            "invalid UTF-8" => db.execute(
                "INSERT INTO preferences VALUES ('agent.modelPreferences', CAST(x'ff' AS TEXT), 1)",
                [],
            )?,
            _ => 0,
        };
        let save = match scenario {
            "save" => Some(assignments),
            "cleared" => Some(json!({})),
            _ => None,
        };
        if let Some(assignments) = &save {
            child.send(Command::SetClaudeRoleAssignments {
                request_id: 1,
                web_search: None,
                assignments: assignments.clone(),
            })?;
        }
        // Send the launch immediately, before consuming any queued save reply.
        child.send(Command::StartFreshProviderSession {
            session_id: child.session.clone(),
        })?;
        let deadline = Instant::now() + TIMEOUT;
        let (mut launched, mut got_save_reply) = (false, save.is_none());
        while !launched || !got_save_reply {
            match child.event(deadline, true)? {
                Event::Frame {
                    session_id, screen, ..
                } if session_id == child.session => {
                    launched |= screen_has_line(&screen, "fresh process ready");
                }
                Event::Graph(graph) => {
                    let Some(reply) = graph.get("claudeRoleAssignmentsResult") else {
                        continue;
                    };
                    let reply: ClaudeRoleAssignmentsReply = serde_json::from_value(reply.clone())?;
                    assert_eq!(reply.request_id, 1);
                    assert_eq!(
                        Some(serde_json::to_value(reply.assignments.unwrap())?),
                        save
                    );
                    assert!(reply.error.is_none());
                    got_save_reply = true;
                }
                Event::Recovery {
                    session_id,
                    state: matou_native_runtime::host::RecoveryState::StartFailed(error),
                    ..
                } if session_id == child.session => bail!("{scenario}: {error}"),
                _ => {}
            }
        }
        let argv = fs::read_to_string(stub.with_extension("argv"))?;
        let settings: Value = serde_json::from_slice(&fs::read(
            argv.lines().nth(1).context("direct CLI settings path")?,
        )?)?;
        if scenario == "save" {
            assert_eq!(
                settings["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"],
                "matou-role/opus"
            );
            assert_eq!(
                settings["modelPicker"]["options"][0]["label"],
                "Opus · next-opus"
            );
            assert!(
                settings["env"]["ANTHROPIC_BASE_URL"]
                    .as_str()
                    .is_some_and(|url| url.starts_with("http://127.0.0.1:"))
            );
        } else {
            assert_eq!(
                settings
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["hooks", "statusLine"],
                "{scenario}: the hook settings leave the terminal mode to Claude",
            );
        }
        let expected: &[&str] = if scenario == "save" {
            &[
                "--model",
                "matou-role/opus",
                "--effort",
                "high",
                "--permission-mode",
                "acceptEdits",
            ]
        } else {
            &["--model", "active-main", "--permission-mode", "acceptEdits"]
        };
        assert_eq!(
            argv.lines().skip(2).collect::<Vec<_>>(),
            expected,
            "{scenario}"
        );
        let endpoint = if scenario == "save" {
            settings["env"]["ANTHROPIC_BASE_URL"]
                .as_str()
                .context("gateway endpoint")?
        } else {
            "https://active.example.invalid"
        };
        assert_eq!(
            fs::read_to_string(stub.with_extension("endpoint"))?.trim(),
            endpoint,
            "{scenario}"
        );
        child.send(Command::Shutdown)?;
        child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "runtime shutdown: {stderr}");
    }
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

fn assert_fresh_permission_confirmed(child: &mut RuntimeChild, mode: &str) -> Result<()> {
    use matou_native_runtime::host::RecoveryState;

    let deadline = Instant::now() + TIMEOUT;
    let mut restoring = false;
    let mut ready = false;
    loop {
        match child.event(deadline, true)? {
            Event::Recovery {
                session_id, state, ..
            } if session_id == child.session => match state {
                RecoveryState::Restoring => restoring = true,
                RecoveryState::Ready if restoring => ready = true,
                _ => {}
            },
            Event::Graph(graph) => {
                let receipt = graph["providerPermissionModeResults"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|result| {
                        result["sessionId"] == child.session && result["permissionMode"] == mode
                    });
                if let Some(receipt) = receipt {
                    assert!(receipt.get("error").is_none(), "{receipt}");
                    assert!(
                        ready,
                        "permission switch did not return the terminal to Ready"
                    );
                    let card = graph["nodes"]
                        .as_array()
                        .context("session graph nodes")?
                        .iter()
                        .find(|node| node["sessionId"] == child.session)
                        .context("permission switch card")?;
                    assert_eq!(card["permissionMode"], mode);
                    assert_eq!(card["providerRestoreState"], "none");
                    assert_eq!(card["resumable"], false);
                    assert!(card["historyProviderSessionId"].is_null());
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}

fn post_permission_identity(settings_path: &Path, provider_id: &str, mode: &str) -> Result<()> {
    post_claude_identity(settings_path, provider_id, mode, "fixture-model")
}

fn post_claude_identity(
    settings_path: &Path,
    provider_id: &str,
    mode: &str,
    model: &str,
) -> Result<()> {
    let status = post_claude_hook(
        settings_path,
        serde_json::json!({
            "hook_event_name": "SessionStart", "session_id": provider_id,
            "permission_mode": mode, "model": model
        }),
    )?;
    ensure!(status == 200, "identity hook was rejected: {status}");
    Ok(())
}

fn post_claude_hook(settings_path: &Path, payload: serde_json::Value) -> Result<u16> {
    post_claude_hook_url(&claude_hook_url(settings_path)?, payload)
}

fn claude_hook_url(settings_path: &Path) -> Result<String> {
    let settings: serde_json::Value = serde_json::from_slice(
        &fs::read(settings_path)
            .with_context(|| format!("read hook settings {}", settings_path.display()))?,
    )?;
    Ok(settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["url"]
        .as_str()
        .context("test hook URL")?
        .to_owned())
}

fn post_claude_hook_url(url: &str, payload: serde_json::Value) -> Result<u16> {
    let url = url.strip_prefix("http://").context("HTTP hook")?;
    let (address, path) = url.split_once('/').context("hook path")?;
    let payload = payload.to_string();
    let mut connection = std::net::TcpStream::connect(address)?;
    connection.set_read_timeout(Some(TIMEOUT))?;
    write!(
        connection,
        "POST /{path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    )?;
    let mut response = String::new();
    connection.read_to_string(&mut response)?;
    Ok(response
        .split_whitespace()
        .nth(1)
        .context("hook HTTP status")?
        .parse()?)
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
    db.execute(
        "INSERT INTO preferences (key, value_json, updated_at) VALUES ('agent.modelPreferences', ?, 1)",
        [serde_json::json!({
            "contextWindows": {}, "compaction": {"mode": "follow", "effort": "follow"},
            "claudeRoleAssignments": {
                "opus": {"providerId": "anthropic-official", "model": "assigned-only", "effort": "high"}
            }
        }).to_string()],
    )?;
    if let Some(state) = unusable {
        db.execute(
            "INSERT INTO provider_bindings
                (id, session_id, provider, provider_session_id, resume_state, restore_state,
                 restore_error, metadata_json, created_at, updated_at, invalidated_at, validated_at)
             VALUES ('old-binding', ?, 'claude-code', 'old-id', ?, ?, ?,
                 json_object('providerConfigId', ?, 'model', 'bound-fixture-model',
                             'permissionMode', 'acceptEdits'), 1, 1, ?, 1)",
            rusqlite::params![
                seed.session,
                if state == "failed" {
                    "available"
                } else {
                    state
                },
                if state == "failed" { state } else { "none" },
                (state == "failed").then_some("previous restore failure"),
                if state == "expired" {
                    "removed-provider"
                } else {
                    "anthropic-official"
                },
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
printf '%s' "$ANTHROPIC_BASE_URL" > "$0.endpoint"
case "$*" in *'--permission-mode plan'*) printf 'plan ready\n';;
  *) printf 'initial ready\n';;
esac
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
        let mut child =
            spawn_fresh_permission_card(root.path(), &stub, unusable.map(|_| "expired"), None)?;
        child.frame(|_, screen, _| screen_has_line(screen, "initial ready"))?;
        let argv = fs::read_to_string(stub.with_extension("argv"))?;
        let mut expected = vec!["--model", "assigned-only", "--effort", "high"];
        if unusable.is_some() {
            expected.extend(["--permission-mode", "acceptEdits"]);
        }
        assert_eq!(argv.lines().skip(2).collect::<Vec<_>>(), expected);
        let endpoint = fs::read_to_string(stub.with_extension("endpoint"))?;
        assert_eq!(endpoint, "https://api.anthropic.com");
        let session = child.session.clone();
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        if unusable == Some("failed") {
            db.execute(
                "UPDATE provider_bindings
                 SET resume_state='available', restore_state='failed',
                     restore_error='previous restore failure', invalidated_at=NULL
                 WHERE id='old-binding'",
                [],
            )?;
        }
        let original_runs = list_runs(&db, &session)?;
        assert_eq!(original_runs.len(), 1);
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
        assert_eq!(
            argv.lines().skip(2).collect::<Vec<_>>(),
            [
                "--model",
                "assigned-only",
                "--effort",
                "high",
                "--permission-mode",
                "plan"
            ]
        );
        assert_eq!(
            fs::read_to_string(stub.with_extension("endpoint"))?,
            endpoint
        );
        if unusable.is_some() {
            let old = db.query_row(
                "SELECT resume_state, restore_state, restore_error IS NULL, invalidated_at IS NOT NULL,
                        json_extract(metadata_json, '$.permissionMode'), json_extract(metadata_json, '$.model')
                 FROM provider_bindings WHERE id='old-binding'",
                [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                              row.get::<_, bool>(2)?, row.get::<_, bool>(3)?,
                              row.get::<_, String>(4)?, row.get::<_, String>(5)?)),
            )?;
            assert_eq!(
                old,
                (
                    "expired".into(),
                    "none".into(),
                    true,
                    true,
                    "acceptEdits".into(),
                    "bound-fixture-model".into()
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
        assert_eq!(
            db.query_row("SELECT work_status FROM sessions WHERE id=?", [&session], |row| {
                row.get::<_, String>(0)
            })?,
            "idle"
        );
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
fn failed_initial_claude_process_drops_remembered_launch_target() -> Result<()> {
    use matou_native_core::sessions::list_runs;

    let root = tempfile::tempdir()?;
    let stub = root.path().join("claude-failed-launch-stub");
    fs::write(
        &stub,
        r#"#!/bin/sh
case "$*" in
  *'--permission-mode plan'*)
    printf '%s\n' "$@" > "$0.argv"
    printf 'plan ready\n'
    exec /bin/sleep 60
    ;;
  *) exit 23;;
esac
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut child = spawn_fresh_permission_card(root.path(), &stub, None, None)?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    let deadline = Instant::now() + TIMEOUT;
    while !list_runs(&db, &child.session)?
        .last()
        .is_some_and(|run| run.exit_code == Some(23))
    {
        ensure!(
            Instant::now() < deadline,
            "initial Claude process did not fail"
        );
        thread::sleep(Duration::from_millis(10));
    }
    child.send(Command::SetProviderPermissionMode {
        session_id: child.session.clone(),
        mode: "plan".into(),
    })?;
    child.frame(|_, screen, _| screen_has_line(screen, "plan ready"))?;
    let argv = fs::read_to_string(stub.with_extension("argv"))?;
    assert_eq!(
        argv.lines().skip(2).collect::<Vec<_>>(),
        ["--permission-mode", "plan"]
    );
    post_permission_identity(
        Path::new(argv.lines().nth(1).context("settings arg")?),
        "new-id",
        "plan",
    )?;
    assert!(
        permission_mode_result(&mut child, "plan")?
            .get("error")
            .is_none()
    );
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");
    Ok(())
}

#[test]
fn claude_command_start_confirms_fresh_and_fork_and_exposes_active_history() -> Result<()> {
    use matou_native_core::sessions::list_runs;
    use matou_native_runtime::host::{ForkPlacement, ForkWorktreeMode};
    use serde_json::json;

    let root = tempfile::tempdir()?;
    let stub = root.path().join("claude-fork-stub");
    fs::write(
        &stub,
        r#"#!/bin/sh
case "$*" in
  *--fork-session*)
    printf '%s\n' "$ANTHROPIC_BASE_URL" > "$0.endpoint"
    printf '%s\n' "$@" > "$0.fork"
    printf 'fork ready\n'
    ;;
  *) printf '%s' "$2" > "$0.settings"; printf 'source ready\n';;
esac
exec /bin/sleep 60
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut child = spawn_fresh_permission_card(root.path(), &stub, None, None)?;
    child.frame(|_, screen, _| screen_has_line(screen, "source ready"))?;
    let send_start = |settings_path: &Path, id| -> Result<()> {
        let settings: serde_json::Value = serde_json::from_slice(&fs::read(settings_path)?)?;
        let start = &settings["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(start["type"], "command");
        let command = start["command"].as_str().context("SessionStart command")?;
        assert!(command.contains("--noproxy '*'"));
        let mut hook = ProcessCommand::new("/bin/sh")
            .args(["-c", command])
            .env_clear()
            .env("ALL_PROXY", "http://127.0.0.1:1")
            .stdin(Stdio::piped())
            .spawn()?;
        serde_json::to_writer(
            hook.stdin.take().context("SessionStart stdin")?,
            &json!({"hook_event_name":"SessionStart","session_id":id,"permission_mode":"default"}),
        )?;
        ensure!(hook.wait()?.success(), "SessionStart command failed");
        Ok(())
    };
    send_start(
        Path::new(&fs::read_to_string(stub.with_extension("settings"))?),
        "fork-source",
    )?;
    wait_permission_identity(&mut child)?;
    let transcript = seed_catalog(root.path(), &child.workspace, "fork-source")?;
    let mut transcript = fs::OpenOptions::new().append(true).open(transcript)?;
    for row_type in [
        "last-prompt",
        "mode",
        "permission-mode",
        "atis-latch",
        "attachment",
        "api-request",
        "api-request-start",
        "api-request-end",
        "cost-state",
    ] {
        writeln!(
            transcript,
            "\n{}",
            json!({"type": row_type, "version": "2.1.330"})
        )?;
    }
    for row in [
        json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Before compaction"}]}}),
        json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto"}}),
        json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"After compaction"}]}}),
        json!({"type":"assistant","message":{"role":"assistant","content":"Text reply"}}),
    ] {
        writeln!(transcript, "{row}")?;
    }
    drop(transcript);
    child.send(Command::GetProviderSession {
        request_id: Some(42),
        cwd: child.workspace.to_string_lossy().into_owned(),
        provider_session_id: "fork-source".into(),
        start_event_index: Some(1),
        limit: None,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(history) = graph["providerSessionDetailResults"].get(0)
        {
            assert_eq!(history["providerSessionId"], "fork-source");
            assert_eq!(history["provider"], "claude-code");
            assert_eq!(history["requestId"], 42);
            assert_eq!(history["page"]["total"], 4);
            let events = history["events"].as_array().unwrap();
            assert_eq!(events.len(), 4);
            for (event, (role, text)) in events.iter().zip([
                ("user", "Synthetic command fixture"),
                ("assistant", "Before compaction"),
                ("user", "After compaction"),
                ("assistant", "Text reply"),
            ]) {
                assert_eq!(event["role"], role);
                assert_eq!(event["text"], text);
            }
            let source = codex_events::node(&graph, &child.session).context("source node")?;
            assert_eq!(source["historyProviderSessionId"], "fork-source");
            break;
        }
    }
    fs::write(
        child.data.join("provider-config.json"),
        json!({
            "version": 1,
            "providers": {"claude-code": [
                {"id": "active", "model": "opus", "endpoint": "https://active.example.invalid"},
                {"id": "bound", "model": "opus", "endpoint": "https://bound.example.invalid"},
                {"id": "anthropic-official", "builtIn": true, "model": "", "endpoint": "https://api.anthropic.com"}
            ]},
            "activeProviderIds": {"claude-code": "active"}
        }).to_string(),
    )?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    assert_eq!(
        db.execute(
            "UPDATE provider_bindings
         SET metadata_json=json_set(metadata_json, '$.providerConfigId', 'bound',
                                    '$.model', 'opus', '$.canFork', json('true'))
         WHERE session_id=?",
            [&child.session],
        )?,
        1
    );
    child.send(Command::ForkSession {
        scene_id: child.scene.clone(),
        source_session_id: child.session.clone(),
        placement: ForkPlacement::Child,
        name: "Fork fixture".into(),
        worktree_mode: ForkWorktreeMode::Current,
        submission_key: "fork-with-role-assignment".into(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    let argv_path = stub.with_extension("fork");
    let forked = loop {
        match child.event(deadline, true)? {
            Event::Frame {
                session_id, screen, ..
            } if screen_has_line(&screen, "fork ready") => {
                break session_id;
            }
            Event::Graph(graph) => {
                if let Some(error) = graph["forkSessionResult"]["error"].as_str() {
                    bail!("fork was rejected: {error}");
                }
            }
            _ => {}
        }
    };
    let argv = fs::read_to_string(argv_path)?;
    assert_eq!(
        argv.lines().skip(2).collect::<Vec<_>>(),
        [
            "--model",
            "opus",
            "--resume",
            "fork-source",
            "--fork-session"
        ]
    );
    assert_eq!(
        fs::read_to_string(stub.with_extension("endpoint"))?.trim(),
        "https://bound.example.invalid"
    );
    assert_eq!(list_runs(&db, &child.session)?[0].status, "running");
    assert_eq!(
        db.query_row(
            "SELECT stage FROM session_fork_intents WHERE session_id=?",
            [&forked],
            |row| row.get::<_, String>(0),
        )?,
        "restoring-provider"
    );
    send_start(
        Path::new(argv.lines().nth(1).context("fork settings path")?),
        "fork-child",
    )?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && graph["providerIdentityEvents"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|event| {
                    event["result"] == "confirmed"
                        && event["sessionId"] == forked
                        && event["providerSessionId"] == "fork-child"
                })
        {
            let branch = codex_events::node(&graph, &forked).context("fork node")?;
            assert_eq!(branch["historyProviderSessionId"], "fork-child");
            assert_eq!(branch["providerRestoreState"], "none");
            assert_eq!(branch["forkProgress"]["stage"], "succeeded");
            break;
        }
    }
    assert_eq!(
        db.query_row(
            "SELECT stage FROM session_fork_intents WHERE session_id=?",
            [&forked],
            |row| row.get::<_, String>(0),
        )?,
        "succeeded"
    );
    assert_eq!(list_runs(&db, &forked)?[0].status, "running");
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");
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
        takeover_from: None,
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
    assert_eq!(
        db.query_row("SELECT work_status FROM sessions WHERE id=?", [&session], |row| {
            row.get::<_, String>(0)
        })?,
        "idle"
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
    let error = failure["error"].as_str().unwrap();
    // The same exit can reach either the startup check or the resume monitor.
    assert!(
        error
            .rsplit(": ")
            .next()
            .is_some_and(|cause| cause == "process exited with code 23")
            || error == "provider resume exited with code 23",
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
            takeover_from: None,
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
            if let Event::Recovery {
                session_id, state, ..
            } = child.event(deadline, true)?
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
                takeover_from: None,
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
            let deadline = Instant::now() + TIMEOUT;
            let mut saw_resume = false;
            let mut dismissal = None;
            while !saw_resume || dismissal.is_none() {
                match child.event(deadline, true)? {
                    Event::Frame {
                        session_id, screen, ..
                    } if session_id == session => {
                        saw_resume |= screen_has_line(&screen, "resuming replace-id");
                    }
                    Event::Graph(graph) if !graph["agentNotifications"].is_null() => {
                        let notifications = graph["agentNotifications"].as_array().unwrap();
                        assert_eq!(notifications.len(), 1);
                        assert_eq!(notifications[0]["operation"], "dismiss");
                        assert_eq!(
                            notifications[0]["replacementKey"],
                            format!("provider-restore:{session}")
                        );
                        assert_eq!(notifications[0]["sessionId"], session);
                        assert_eq!(notifications[0]["sceneId"], child.scene);
                        assert_eq!(graph["nodes"][0]["providerRestoreState"], "restoring");
                        assert_eq!(graph["nodes"][0]["canFork"], false);
                        assert_eq!(
                            graph["nodes"][0]["providerSpawnRevision"],
                            notifications[0]["occurredAt"]
                        );
                        assert!(dismissal.replace(notifications[0].clone()).is_none());
                    }
                    Event::Diagnostic(error) => bail!("host diagnostic: {error}"),
                    _ => {}
                }
            }
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
            assert_eq!(binding.metadata["canFork"], true);
            let dismissal = dismissal.unwrap();
            assert_eq!(binding.metadata["spawnRevision"], dismissal["occurredAt"]);
            let validated_at: i64 = db.query_row(
                "SELECT validated_at FROM provider_bindings WHERE id = ?",
                [&binding.id],
                |row| row.get(0),
            )?;
            assert_eq!(dismissal["occurredAt"], validated_at);
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
fn claude_catalog_takeover_validates_stops_and_settles_one_owner() -> Result<()> {
    use matou_native_core::sessions::{catalog_binding_owners, claude_binding, list_runs};
    use serde_json::Value;

    fn reply(
        child: &mut RuntimeChild,
        key: &str,
        card: &str,
        mut predicate: impl FnMut(&Value) -> bool,
    ) -> Result<Value> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Event::Graph(graph) = child.event(deadline, true)?
                && let Some(row) = graph[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|row| row["sessionId"] == card && predicate(row))
            {
                return Ok(row.clone());
            }
        }
    }

    for fail_restore in [false, true] {
        let root = tempfile::tempdir()?;
        let stub = root.path().join("claude-stub");
        fs::write(
            &stub,
            r#"#!/bin/sh
root=$(dirname "$0")
while [ $# -gt 0 ]; do
  case "$1" in
    --settings) settings="$2"; shift ;;
    --resume) conversation="$2"; shift ;;
  esac
  shift
done
if [ -e "$root/fail-resume" ] && [ "$conversation" = 'source-conversation' ]; then
  echo 'fixture refused resume'
  exit 31
fi
printf '%s' "$settings" > "$root/$conversation.settings"
printf 'fixture resumed %s\n' "$conversation"
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
        let source = child.session.clone();
        child.send(Command::CreateShellSibling {
            scene_id: child.scene.clone(),
            source_session_id: source.clone(),
            parent_session_id: None,
        })?;
        let deadline = Instant::now() + TIMEOUT;
        let target = loop {
            if let Event::Navigation { hierarchy, .. } = child.event(deadline, true)?
                && let Some(session) = hierarchy.session
                && session.id != source
            {
                break session.id;
            }
        };
        // The newly created card starts its own isolated Shell before loading.
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if matches!(child.event(deadline, true)?, Event::Frame { session_id, .. } if session_id == target)
            {
                break;
            }
        }
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        let load =
            |target: &str, conversation: &str, from: Option<&str>| Command::LoadProviderSession {
                session_id: target.into(),
                provider_session_id: conversation.into(),
                takeover_from: from.map(str::to_owned),
            };
        let confirm = |child: &mut RuntimeChild, card: &str, conversation: &str| -> Result<()> {
            let deadline = Instant::now() + TIMEOUT;
            let marker = format!("fixture resumed {conversation}");
            loop {
                if let Event::Frame {
                    session_id, screen, ..
                } = child.event(deadline, true)?
                    && session_id == card
                    && screen_has_line(&screen, &marker)
                {
                    break;
                }
            }
            let settings =
                fs::read_to_string(root.path().join(format!("{conversation}.settings")))?;
            post_permission_identity(Path::new(&settings), conversation, "default")?;
            reply(child, "providerIdentityEvents", card, |row| {
                row["result"] == "confirmed"
            })?;
            Ok(())
        };
        for (card, conversation) in [
            (&source, "source-conversation"),
            (&target, "target-conversation"),
        ] {
            seed_catalog(root.path(), &child.workspace, conversation)?;
            child.send(load(card, conversation, None))?;
            confirm(&mut child, card, conversation)?;
        }
        let source_binding = claude_binding(&db, &source)?.context("source binding")?;
        let target_binding = claude_binding(&db, &target)?.context("target binding")?;
        let source_run = list_runs(&db, &source)?.pop().context("source run")?;
        let target_run = list_runs(&db, &target)?.pop().context("target run")?;
        let stop_reason = |child: &mut RuntimeChild| -> Result<String> {
            Ok(
                reply(child, "providerSessionLoadErrors", &target, |_| true)?["error"]
                    .as_str()
                    .context("catalog rejection")?
                    .into(),
            )
        };
        let owners = || -> Result<Vec<String>> {
            Ok(
                catalog_binding_owners(&db, "claude-code", &["source-conversation"])?
                    .into_iter()
                    .map(|(_, card, _)| card)
                    .collect(),
            )
        };
        let takeover = || load(&target, "source-conversation", Some(&source));

        child.send(load(&target, "source-conversation", None))?;
        assert!(stop_reason(&mut child)?.contains("Confirm the current source"));
        child.send(load(&target, "source-conversation", Some("wrong-card")))?;
        assert!(stop_reason(&mut child)?.contains("Confirm the current source"));

        db.execute(
            "INSERT INTO provider_bindings
             (id, session_id, provider, provider_session_id, resume_state, created_at, updated_at, validated_at)
             VALUES ('fixture-ambiguous', ?, 'claude-code', 'source-conversation', 'available', 1, 1, 1)",
            [&target],
        )?;
        child.send(takeover())?;
        assert!(stop_reason(&mut child)?.contains("ambiguous"));
        db.execute(
            "DELETE FROM provider_bindings WHERE id = 'fixture-ambiguous'",
            [],
        )?;

        db.execute(
            "UPDATE sessions SET archived_at = 1 WHERE id = ?",
            [&source],
        )?;
        child.send(takeover())?;
        assert!(stop_reason(&mut child)?.contains("no longer owns"));
        db.execute(
            "UPDATE sessions SET archived_at = NULL WHERE id = ?",
            [&source],
        )?;
        assert_eq!(list_runs(&db, &source)?.last(), Some(&source_run));
        assert_eq!(list_runs(&db, &target)?.last(), Some(&target_run));
        assert_eq!(owners()?, [source.clone()]);

        if fail_restore {
            fs::write(root.path().join("fail-resume"), "")?;
        } else {
            db.execute_batch(
                "CREATE TRIGGER fail_takeover_exit BEFORE UPDATE OF status ON session_runs
                 WHEN NEW.status = 'exited' BEGIN SELECT RAISE(ABORT, 'injected takeover exit failure'); END;",
            )?;
            child.send(takeover())?;
            let reason = stop_reason(&mut child)?;
            assert!(reason.contains("process exited, but its exit record"));
            assert!(reason.contains("injected takeover exit failure"));
            assert_eq!(owners()?, [source.clone()]);
            assert_eq!(list_runs(&db, &target)?.last(), Some(&target_run));
            assert_shell_gone(source_run.pid.context("source PID")? as u32)?;
            db.execute_batch("DROP TRIGGER fail_takeover_exit;")?;
        }

        child.send(takeover())?;
        child.send(takeover())?;
        if fail_restore {
            let failure = reply(&mut child, "providerIdentityEvents", &target, |row| {
                row["result"] == "restore-failed"
            })?;
            assert_eq!(failure["providerSessionId"], "source-conversation");
            assert_eq!(owners()?, [source.clone()]);
            assert_eq!(
                claude_binding(&db, &source)?
                    .context("rolled back source")?
                    .id,
                source_binding.id
            );
            let original = claude_binding(&db, &target)?.context("rolled back target")?;
            assert_eq!(original.id, target_binding.id);
            assert_eq!(original.provider_session_id, "target-conversation");
            assert!(original.resumable);
        } else {
            confirm(&mut child, &target, "source-conversation")?;
            assert_eq!(owners()?, [target.clone()]);
            assert!(
                !claude_binding(&db, &source)?
                    .context("historical source")?
                    .resumable
            );
            let source_runs = list_runs(&db, &source)?;
            let target_runs = list_runs(&db, &target)?;
            assert_eq!(source_runs.len(), 2);
            assert_eq!(target_runs.len(), 3);
            assert_eq!(source_runs[1].status, "exited");
            assert!(source_runs[1].ended_at.context("source exit")? <= target_runs[2].started_at);
            assert!(target_runs[1].ended_at.context("target exit")? <= target_runs[2].started_at);

            child.send(takeover())?;
            reply(&mut child, "providerIdentityEvents", &target, |row| {
                row["reason"] == "already-loaded"
            })?;
            assert_eq!(list_runs(&db, &target)?, target_runs);
        }
        assert_shell_gone(source_run.pid.context("source PID")? as u32)?;
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
        takeover_from: None,
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
        takeover_from: None,
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
    let revision: i64 = db.query_row(
        "SELECT validated_at FROM provider_bindings WHERE id = ?",
        [&active.id],
        |row| row.get(0),
    )?;
    assert_eq!(active.metadata["spawnRevision"], revision);

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

#[test]
fn assigned_claude_gateway_changes_target_without_restarting_the_cli() -> Result<()> {
    use std::{
        io::{BufRead, BufReader},
        net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    };

    use matou_native_core::sessions::{claude_binding, list_runs};
    use serde_json::{Value, json};

    const A_KEY: &str = "synthetic-gateway-provider-a";
    const B_KEY: &str = "synthetic-gateway-provider-b";

    let (observed, requests) = mpsc::channel();
    let fake = |lane: &'static str,
                expected_key: &'static str,
                markers: &'static [(&str, &str)]|
     -> Result<_> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let observed = observed.clone();
        let handle = thread::spawn(move || -> Result<()> {
            for &(path, marker) in markers {
                let deadline = Instant::now() + TIMEOUT;
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            ensure!(Instant::now() < deadline, "gateway fake upstream deadline");
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(TIMEOUT))?;
                stream.set_write_timeout(Some(TIMEOUT))?;
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                stream.read_line(&mut line)?;
                assert_eq!(line, format!("POST /{lane}{path} HTTP/1.1\r\n"));
                let mut length = None;
                let mut correct_key = false;
                loop {
                    line.clear();
                    ensure!(stream.read_line(&mut line)? > 0, "incomplete request heads");
                    if line == "\r\n" {
                        break;
                    }
                    let (name, value) = line.split_once(':').context("HTTP header")?;
                    match name.to_ascii_lowercase().as_str() {
                        "content-length" => length = Some(value.trim().parse::<usize>()?),
                        "x-api-key" => correct_key = value.trim() == expected_key,
                        "authorization" | "cookie" => bail!("caller credential leaked"),
                        _ => {}
                    }
                }
                assert!(
                    correct_key,
                    "incorrect synthetic credential at upstream {lane}"
                );
                let mut body = vec![0; length.context("missing content length")?];
                stream.read_exact(&mut body)?;
                let body: Value = serde_json::from_slice(&body)?;
                assert_eq!(body["model"], "shared-model");
                observed.send((lane, path, "shared-model"))?;
                if marker == "paused-stream" {
                    stream.get_mut().write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n15\r\ndata: paused-stream\n\n\r\n"
                    )?;
                    match stream.read(&mut [0]) {
                        Ok(0) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                        other => bail!("old gateway left upstream open: {other:?}"),
                    }
                    observed.send((lane, "cancelled", "shared-model"))?;
                } else {
                    write!(
                        stream.get_mut(),
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{marker}",
                        marker.len()
                    )?;
                }
            }
            Ok(())
        });
        Ok((address, handle))
    };
    let (a, a_task) = fake(
        "a",
        A_KEY,
        &[
            ("/v1/messages", "from-opus"),
            ("/v1/messages", "paused-stream"),
        ],
    )?;
    let (b, b_task) = fake(
        "b",
        B_KEY,
        &[("/v1/messages/count_tokens", "from-sonnet-count")],
    )?;

    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let no_shell = root.path().join("missing-shell");
    let mut seed = RuntimeChild::spawn_configured(root.path(), "", &no_shell, None)?;
    seed.ready()?;
    seed.send(Command::Shutdown)?;
    seed.closed()?;
    assert!(seed.wait()?.0.success());

    let provider = |id, endpoint, key| {
        json!({
            "id": id, "name": format!("Fixture provider {id}"),
            "endpoint": endpoint, "apiKey": key, "model": "source-opus",
            "modelMapping": {"opus": "source-opus", "haiku": "source-haiku"},
            "modelMetadata": [
                {"id": "shared-model"},
                {"id": "source-haiku", "resolvedModel": "upstream-haiku"}
            ]
        })
    };
    fs::write(seed.data.join("provider-config.json"), json!({
        "version": 1, "activeProviderIds": {"claude-code": "a"},
        "providers": {"claude-code": [
            provider("a", format!("http://{a}/a"), A_KEY), provider("b", format!("http://{b}/b"), B_KEY)
        ]}
    }).to_string())?;
    let db = Connection::open(seed.data.join("matou.sqlite"))?;
    db.execute(
        "UPDATE sessions SET kind='claude-code' WHERE id=?1",
        [&seed.session],
    )?;
    db.execute(
        "INSERT INTO preferences VALUES ('agent.modelPreferences', ?1, 1)",
        [json!({"contextWindows": {}, "compaction": {"mode":"follow","effort":"follow"}, "claudeRoleAssignments": {
            "opus": {"providerId": "a", "model": "shared-model"},
            "sonnet": {"providerId": "b", "model": "shared-model"}
        }})
        .to_string()],
    )?;
    let stub = root.path().join("gateway-cli");
    fs::write(
        &stub,
        r#"#!/bin/sh
set -eu
test "$ANTHROPIC_API_KEY" = matou-local-role-gateway || { echo 'gateway API placeholder missing'; exit 10; }
test -z "${ANTHROPIC_AUTH_TOKEN-}" && test -z "${CLAUDE_CODE_OAUTH_TOKEN-}" || { echo 'gateway CLI received an auth token'; exit 11; }
test "$ANTHROPIC_DEFAULT_FABLE_MODEL" = matou-role/fable
test "$3" = --model
test "$4" = matou-role/opus
printf '%s\n' "$ANTHROPIC_BASE_URL" > "$0.url"
printf '%s\n' "$2" > "$0.settings"
printf 'gateway-cli ready\n'
while IFS= read -r switch; do
    case "$switch" in
        opus) model=$ANTHROPIC_DEFAULT_OPUS_MODEL; path=/v1/messages ;;
        sonnet) model=$ANTHROPIC_DEFAULT_SONNET_MODEL; path=/v1/messages/count_tokens ;;
        stream) model=$ANTHROPIC_DEFAULT_SONNET_MODEL; path=/v1/messages ;;
        *) exit 12 ;;
    esac
    /usr/bin/curl --silent --fail --no-buffer --noproxy '*' --max-time 15 \
        -H 'content-type: application/json' -H "x-api-key: $ANTHROPIC_API_KEY" \
        --data "{\"model\":\"$model\",\"stream\":true}" "$ANTHROPIC_BASE_URL$path"
    printf '\n'
done
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut child = RuntimeChild::spawn_configured(root.path(), "", &no_shell, Some(&stub))?;
    child.connect()?;
    child.frame(|_, screen, _| screen_has_line(screen, "gateway-cli ready"))?;
    let initial = list_runs(&db, &child.session)?;
    assert_eq!(initial.len(), 1);
    let run = &initial[0];
    let pid = run.pid.context("substitute CLI PID")?;
    assert_eq!(run.status, "running");
    let settings_path = fs::read_to_string(stub.with_extension("settings"))?;
    let settings_path = Path::new(settings_path.trim());
    let settings: Value = serde_json::from_slice(&fs::read(settings_path)?)?;
    let model_picker = serde_json::to_string(&settings["modelPicker"])?;
    assert_eq!(
        model_picker,
        r#"{"replaceBuiltInOptions":true,"options":[{"model":"opus","label":"Opus · shared-model","description":"a"},{"model":"fable","label":"Fable · shared-model","description":"a"},{"model":"sonnet","label":"Sonnet · shared-model","description":"b"},{"model":"haiku","label":"Haiku · upstream-haiku","description":"a"}]}"#
    );
    println!("modelPicker from the live CLI settings file = {model_picker}");
    post_claude_identity(
        settings_path,
        "gateway-test-conversation",
        "default",
        "matou-role/opus",
    )?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(binding) = claude_binding(&db, &child.session)? {
            assert_eq!(binding.provider_session_id, "gateway-test-conversation");
            assert_eq!(binding.metadata["providerConfigId"], "a");
            assert_eq!(binding.metadata["model"], "shared-model");
            break;
        }
        child.event(deadline, true)?;
    }

    child.input("opus")?;
    child.frame(|_, screen, _| screen_has_line(screen, "from-opus"))?;
    assert_eq!(
        requests
            .recv_timeout(TIMEOUT)
            .context("upstream A message observation")?,
        ("a", "/v1/messages", "shared-model")
    );
    child.input("sonnet")?;
    child.frame(|_, screen, _| screen_has_line(screen, "from-sonnet-count"))?;
    assert_eq!(
        requests
            .recv_timeout(TIMEOUT)
            .context("upstream B count observation")?,
        ("b", "/v1/messages/count_tokens", "shared-model")
    );
    let after = list_runs(&db, &child.session)?;
    assert_eq!(after.len(), 1);
    assert_eq!((&after[0].id, after[0].pid), (&run.id, Some(pid)));

    child.send(Command::SetClaudeRoleAssignments {
        request_id: 42,
        web_search: None,
        assignments: json!({
            "opus": {"providerId": "a", "model": "shared-model"},
            "sonnet": {"providerId": "a", "model": "shared-model"}
        }),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && let Some(value) = graph.get("claudeRoleAssignmentsResult")
        {
            let reply: ClaudeRoleAssignmentsReply = serde_json::from_value(value.clone())?;
            assert_eq!(reply.request_id, 42);
            assert!(reply.error.is_none());
            break;
        }
    }
    // Sonnet still uses the same CLI alias, but now shares Opus's upstream.
    child.input("stream")?;
    assert_eq!(
        requests
            .recv_timeout(TIMEOUT)
            .context("upstream A open stream observation")?,
        ("a", "/v1/messages", "shared-model")
    );
    child.frame(|_, screen, _| screen_has_line(screen, "data: paused-stream"))?;
    let after_save = list_runs(&db, &child.session)?;
    assert_eq!(after_save.len(), 1);
    assert_eq!((&after_save[0].id, after_save[0].pid), (&run.id, Some(pid)));
    child.send(Command::RenameTask {
        task_id: child.task.clone(),
        title: "Gateway host responds mid-stream".into(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let event = child.event(deadline, true)?;
        let raw = serde_json::to_string(&event)?;
        assert!(
            !raw.contains(A_KEY) && !raw.contains(B_KEY),
            "key in runtime event"
        );
        if matches!(event, Event::TaskRenameResult { error: None, .. }) {
            break;
        }
    }

    let url = url::Url::parse(fs::read_to_string(stub.with_extension("url"))?.trim())?;
    let old_addr: SocketAddr = (
        Ipv4Addr::LOCALHOST,
        url.port().context("private gateway port")?,
    )
        .into();
    child.send(Command::CreateShellSibling {
        scene_id: child.scene.clone(),
        source_session_id: child.session.clone(),
        parent_session_id: None,
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.event(deadline, true)? {
            Event::Navigation { .. } => break,
            Event::NavigationFailed { message, .. } => bail!("create sibling: {message}"),
            _ => {}
        }
    }
    child.send(Command::CloseSessionCard {
        scene_id: child.scene.clone(),
        session_id: child.session.clone(),
        confirmed_intent: Some(format!("delete-session:{}", child.session)),
        preserve_scene_on_last_session: true,
    })?;
    assert_eq!(
        requests
            .recv_timeout(TIMEOUT)
            .context("upstream A cancellation observation")?,
        ("a", "cancelled", "shared-model")
    );
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.event(deadline, true)? {
            Event::ScenesDisposed(ids) if ids.contains(&child.session) => break,
            Event::NavigationFailed { message, .. } => bail!("retire gateway card: {message}"),
            _ => {}
        }
    }
    assert_eq!(
        TcpStream::connect_timeout(&old_addr, Duration::from_secs(1))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::ConnectionRefused
    );

    a_task.join().expect("upstream A panicked")?;
    b_task.join().expect("upstream B panicked")?;
    assert_eq!(db.query_row(
        "SELECT COUNT(*) FROM domain_events WHERE instr(payload_json, ?1)>0 OR instr(payload_json, ?2)>0 OR instr(payload_json, ?3)>0",
        (A_KEY, B_KEY, url.path()),
        |row| row.get::<_, i64>(0)
    )?, 0, "gateway credential in persisted domain events");
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    assert!(
        !stderr.contains(A_KEY) && !stderr.contains(B_KEY) && !stderr.contains(url.path()),
        "secret in runtime stderr"
    );
    ensure!(status.success(), "runtime clean shutdown failed");
    println!(
        "substitute CLI pid={pid} run={} reached A(messages) and B(count_tokens/messages) with the same upstream model and distinct key checks; host replied during SSE; card close cancelled upstream and refused old URL",
        run.id
    );
    Ok(())
}

#[test]
fn antigravity_fresh_permission_restart_confirms_without_a_conversation() -> Result<()> {
    use matou_native_core::sessions::{antigravity_binding, list_runs};

    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let stub = root.join("agy-fixture");
    fs::write(
        &stub,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$@" > "$AGY_FIXTURE_ROOT/argv"
case " $* " in *' --mode plan '*) exit 23;; esac
printf 'ANTIGRAVITY FIXTURE READY\n'
while IFS= read -r line; do printf 'INPUT <%s>\n' "$line"; done
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let mut child = RuntimeChild::spawn_configured_with(
        root,
        &format!("PROMPT='{PROMPT} '\nagy() {{ :; }}\n"),
        Path::new("/bin/zsh"),
        None,
        |command| {
            command
                .env("MATOU_ANTIGRAVITY_COMMAND", &stub)
                .env("MATOU_ANTIGRAVITY_HOME", root.join("catalog"))
                .env("MATOU_PERMISSION_STARTUP_WINDOW_MS", "600")
                .env("AGY_FIXTURE_ROOT", root);
        },
    )?;
    child.hello()?;
    child.input("agy")?;
    child.frame(|_, screen, _| screen_has_line(screen, "ANTIGRAVITY FIXTURE READY"))?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    let old = list_runs(&db, &child.session)?
        .pop()
        .context("initial run")?;
    assert_eq!(old.profile, "antigravity");
    assert!(antigravity_binding(&db, &child.session)?.is_none());

    let mode = "bypassPermissions";
    child.send(Command::SetProviderPermissionMode {
        session_id: child.session.clone(),
        mode: mode.into(),
    })?;
    assert_fresh_permission_confirmed(&mut child, mode)?;
    let args = fs::read_to_string(root.join("argv"))?;
    assert!(
        args.lines()
            .any(|arg| arg == "--dangerously-skip-permissions")
    );
    assert!(!args.lines().any(|arg| arg == "--conversation"));
    let runs = list_runs(&db, &child.session)?;
    let new = runs.last().context("replacement run")?;
    assert_eq!(new.ordinal, old.ordinal + 1);
    assert_eq!(new.profile, "antigravity");
    assert_eq!(new.status, "running");
    assert_eq!(runs[runs.len() - 2].status, "exited");
    assert!(antigravity_binding(&db, &child.session)?.is_none());
    child.input("after-switch")?;
    child.frame(|_, screen, _| screen_has_line(screen, "INPUT <after-switch>"))?;

    child.send(Command::SetProviderPermissionMode {
        session_id: child.session.clone(),
        mode: "plan".into(),
    })?;
    let failure = permission_mode_result(&mut child, "plan")?;
    assert!(
        failure["error"]
            .as_str()
            .is_some_and(|error| error.contains("process exited with code 23")),
        "{failure}"
    );
    assert!(antigravity_binding(&db, &child.session)?.is_none());

    child.send(Command::Shutdown)?;
    child.closed()?;
    let (exit, stderr) = child.wait()?;
    ensure!(exit.success(), "{stderr}");
    Ok(())
}

#[test]
fn antigravity_turbo_restarts_same_conversation_and_survives_auto_status() -> Result<()> {
    use matou_native_core::sessions::{antigravity_binding, list_runs};
    use serde_json::{Value, json};

    const CONVERSATION: &str = "agy-permission-fixture";
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let stub = root.join("agy-fixture");
    let catalog = root.join("catalog");
    fs::create_dir_all(catalog.join("cache"))?;
    fs::write(
        &stub,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$@" > "$AGY_FIXTURE_ROOT/argv"
printf '%s\n' "$MATOU_AGENT_STATUS_DIR" > "$AGY_FIXTURE_ROOT/status-path"
printf '%s\n' "$MATOU_PROVIDER_HOOK_URL" > "$AGY_FIXTURE_ROOT/hook-url"
printf 'ANTIGRAVITY FIXTURE READY\n'
while IFS= read -r line; do :; done
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;

    let mut child = RuntimeChild::spawn_configured_with(
        root,
        &format!("PROMPT='{PROMPT} '\nagy() {{ :; }}\n"),
        Path::new("/bin/zsh"),
        None,
        |command| {
            command
                .env("MATOU_ANTIGRAVITY_COMMAND", &stub)
                .env("MATOU_ANTIGRAVITY_HOME", &catalog)
                .env("AGY_FIXTURE_ROOT", root);
        },
    )?;
    child.hello()?;
    let cwd_uri = url::Url::from_directory_path(child.workspace.canonicalize()?)
        .map_err(|_| anyhow::anyhow!("fixture workspace URL"))?;
    fs::write(catalog.join("cache/conversation_metadata.json"), json!({
        "conversations": {CONVERSATION: {"summary": {
            "AppDataDir":"antigravity-cli", "WorkspaceURIs":[cwd_uri.as_str()], "Title":"Fixture"
        }}}
    }).to_string())?;
    fs::write(
        catalog.join("mcp_config.json"),
        r#"{"mcpServers":{"fixture":{}}}"#,
    )?;
    let post = |url: &str, value: Value| -> Result<()> {
        let status = ProcessCommand::new("/usr/bin/curl")
            .args([
                "--noproxy",
                "*",
                "--silent",
                "--show-error",
                "--fail",
                "--max-time",
                "2",
                "--header",
                "Content-Type: application/json",
                "--data",
                &value.to_string(),
                url,
            ])
            .stdout(Stdio::null())
            .status()?;
        ensure!(status.success(), "fixture hook post failed");
        Ok(())
    };
    let status = |url: &str, mode: &str| {
        post(
            &format!("{url}?kind=statusline"),
            json!({
                "conversationId": CONVERSATION, "agent_state":"idle", "cycle_mode":mode,
                "model":{"id":"gemini-fixture"}, "quota":{"gemini-5h":{"remaining_fraction":0.75}}
            }),
        )
    };

    child.input("agy")?;
    child.frame(|rows, _, _| {
        rows.iter()
            .any(|row| row.contains("ANTIGRAVITY FIXTURE READY"))
    })?;
    let old_url = fs::read_to_string(root.join("hook-url"))?;
    status(old_url.trim(), "ask")?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    let deadline = Instant::now() + TIMEOUT;
    let binding = loop {
        if let Some(binding) = antigravity_binding(&db, &child.session)?
            && binding.metadata["permissionMode"] == "ask"
        {
            break binding;
        }
        ensure!(
            Instant::now() < deadline,
            "Antigravity binding did not observe Ask"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let status_path = PathBuf::from(fs::read_to_string(root.join("status-path"))?.trim());
    assert_eq!(
        status_path,
        child.data.join("agent-status").join(&child.session)
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    fs::write(status_path.join("reviewer.json"), json!({
        "version":1,"label":"External reviewer","state":"running","startedAt":now,"updatedAt":now
    }).to_string())?;

    child.send(Command::GetAgentHud {
        request_id: 55,
        session_id: child.session.clone(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    let hud = loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && graph["type"] == "agentHudResult"
            && graph["requestId"] == 55
        {
            break graph["result"]["hud"].clone();
        }
    };
    assert_eq!(hud["permissionMode"], "ask");
    assert_eq!(hud["usageWindows"][0]["usedPercent"], 25.0);
    assert_eq!(hud["configCounts"]["mcps"], 1);
    assert_eq!(hud["agents"][0]["id"], "external:reviewer");
    assert_eq!(hud["gitBranch"], Value::Null);
    assert_eq!(hud["gitDirty"], false);

    let old_run = list_runs(&db, &child.session)?
        .pop()
        .context("initial run")?;
    child.send(Command::SetProviderPermissionMode {
        session_id: child.session.clone(),
        mode: "turbo".into(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    let new_url = loop {
        if let Ok(url) = fs::read_to_string(root.join("hook-url"))
            && !url.trim().is_empty()
            && url != old_url
        {
            break url;
        }
        ensure!(
            Instant::now() < deadline,
            "Antigravity permission restart did not launch"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let args = fs::read_to_string(root.join("argv"))?;
    let args: Vec<_> = args.lines().collect();
    assert!(
        args.windows(2)
            .any(|args| args == ["--conversation", CONVERSATION])
    );
    assert!(args.contains(&"--dangerously-skip-permissions"));
    assert_eq!(args.iter().filter(|arg| **arg == "--settings").count(), 1);
    let settings_path = args
        .windows(2)
        .find_map(|args| (args[0] == "--settings").then_some(args[1]))
        .context("expected per-session settings")?;
    let settings: Value = serde_json::from_slice(&fs::read(settings_path)?)?;
    assert_eq!(settings["statusLine"]["stack_with_default"], false);
    assert_eq!(settings["toolPermission"], "always-proceed");
    assert_eq!(settings["enableTerminalSandbox"], false);
    assert_eq!(settings["allowNonWorkspaceAccess"], true);
    status(new_url.trim(), "auto")?;
    let response = permission_mode_result(&mut child, "turbo")?;
    assert!(response.get("error").is_none(), "{response}");
    status(new_url.trim(), "auto")?;
    child.send(Command::GetAgentHud {
        request_id: 56,
        session_id: child.session.clone(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = child.event(deadline, true)?
            && graph["type"] == "agentHudResult"
            && graph["requestId"] == 56
        {
            assert_eq!(graph["result"]["hud"]["permissionMode"], "turbo");
            break;
        }
    }
    let confirmed = antigravity_binding(&db, &child.session)?.context("confirmed binding")?;
    assert_eq!(confirmed.id, binding.id);
    assert_eq!(confirmed.provider_session_id, CONVERSATION);
    assert_eq!(confirmed.metadata["permissionMode"], "turbo");
    assert_ne!(
        list_runs(&db, &child.session)?.pop().unwrap().id,
        old_run.id
    );
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (exit, stderr) = child.wait()?;
    ensure!(exit.success(), "{stderr}");
    Ok(())
}

#[test]
fn direct_claude_cli_receives_context_window_in_private_settings_and_environment() -> Result<()> {
    use serde_json::{Value, json};

    let root = tempfile::tempdir()?;
    let no_shell = root.path().join("no-shell");
    let mut seed = RuntimeChild::spawn_configured(root.path(), "", &no_shell, None)?;
    seed.ready()?;
    seed.send(Command::Shutdown)?;
    seed.closed()?;
    assert!(seed.wait()?.0.success());
    Connection::open(seed.data.join("matou.sqlite"))?.execute(
        "UPDATE sessions SET kind='claude-code' WHERE id=?",
        [&seed.session],
    )?;
    fs::write(
        seed.data.join("provider-config.json"),
        json!({
            "version": 1,
            "activeProviderIds": {"claude-code": "source"},
            "providers": {"claude-code": [{
                "id": "source", "model": "deepseek-flash", "endpoint": "http://127.0.0.1:1",
                "apiKey": "fixture-key",
                "modelMetadata": [{"id": "deepseek-flash", "contextWindow": 1_048_576}]
            }]}
        })
        .to_string(),
    )?;

    let stub = root.path().join("direct-context-cli");
    fs::write(
        &stub,
        r#"#!/bin/sh
set -eu
test "$ANTHROPIC_BASE_URL" = http://127.0.0.1:1
test "$1" = --settings
printf '%s\n' "$2" "${CLAUDE_CODE_MAX_CONTEXT_TOKENS-}" \
    "${CLAUDE_AUTOCOMPACT_PCT_OVERRIDE-}" "${CLAUDE_CODE_DISABLE_1M_CONTEXT-}" > "$0.launch"
printf 'DIRECT_CONTEXT_READY\n'
exec /bin/sleep 60
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let keys = [
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
        "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
        "CLAUDE_CODE_DISABLE_1M_CONTEXT",
    ];
    let mut runtime =
        RuntimeChild::spawn_configured_with(root.path(), "", &no_shell, Some(&stub), |command| {
            for key in keys {
                command.env_remove(key);
            }
        })?;
    runtime.connect()?;
    runtime.frame(|_, screen, _| screen_has_line(screen, "DIRECT_CONTEXT_READY"))?;
    let recorded = fs::read_to_string(stub.with_extension("launch"))?;
    let mut lines = recorded.lines();
    let settings: Value = serde_json::from_slice(&fs::read(
        lines.next().context("private Claude settings argument")?,
    )?)?;
    let process_env: serde_json::Map<String, Value> = keys
        .into_iter()
        .zip(lines)
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| (key.to_owned(), json!(value)))
        .collect();
    let settings_env: serde_json::Map<String, Value> = keys
        .into_iter()
        .filter_map(|key| Some((key.to_owned(), settings["env"].get(key)?.clone())))
        .collect();
    let window = &settings["autoCompactWindow"];
    // Capture only the portable settings fields, without per-run hook credentials.
    println!(
        "CLI context projection: {}",
        json!({
            "processEnv": process_env, "settings": {"autoCompactWindow": window, "env": settings_env}
        })
    );
    runtime.send(Command::Shutdown)?;
    runtime.closed()?;
    assert!(runtime.wait()?.0.success());

    assert_eq!(*window, json!(1_048_576));
    let expected = json!({
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "1048576",
        "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE": "90",
        "CLAUDE_CODE_DISABLE_1M_CONTEXT": "0"
    });
    assert_eq!(Value::Object(settings_env), expected);
    assert_eq!(Value::Object(process_env), expected);
    Ok(())
}
