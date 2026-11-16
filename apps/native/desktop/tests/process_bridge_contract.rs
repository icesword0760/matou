#![cfg(target_os = "macos")]
mod support;

use std::{
    fs,
    path::PathBuf,
    process::Command as ProcessCommand,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use matou_native_desktop::runtime_bridge::{Command, Event, LaunchOptions, RuntimeBridge};
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(8);
const PROMPT: &str = "MATOU_PROCESS_BRIDGE>";

fn options() -> Result<(PathBuf, LaunchOptions)> {
    let root = std::env::temp_dir().join(format!("matou-process-bridge-{}", Uuid::new_v4()));
    let home = root.join("Shell 家目录");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
    Ok((
        root.clone(),
        LaunchOptions {
            data_root: root.join("数据 空间"),
            workspace: root.join("工作 区"),
            shell_home: home,
            shell: "/bin/zsh".into(),
        },
    ))
}

fn next(bridge: &RuntimeBridge, deadline: Instant) -> Result<Event> {
    loop {
        match bridge.events.try_recv() {
            Ok(event) => return Ok(event),
            Err(async_channel::TryRecvError::Empty) => {}
            Err(error) => bail!("bridge disconnected before expected event: {error}"),
        }
        ensure!(Instant::now() < deadline, "bridge event deadline");
        thread::sleep(Duration::from_millis(5));
    }
}

fn gone(pid: u32) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    while ProcessCommand::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()?
        .status
        .success()
    {
        ensure!(Instant::now() < deadline, "owned process {pid} leaked");
        thread::sleep(Duration::from_millis(20));
    }
    println!("verified bridge-owned process {pid} exited");
    Ok(())
}

#[test]
fn missing_required_child_is_error_without_inprocess_fallback() -> Result<()> {
    let (root, options) = options()?;
    let missing = root.join("missing-matou-native-runtime");
    let error = match RuntimeBridge::start_with_binary(options.clone(), &missing) {
        Ok(mut bridge) => {
            bridge.shutdown()?;
            bail!("missing binary silently started a runtime")
        }
        Err(error) => format!("{error:#}"),
    };
    ensure!(error.contains("required native runtime child") && error.contains("missing-matou"));
    ensure!(!options.data_root.exists() && !options.workspace.exists());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn actual_child_owns_pty_and_shutdown_drop_or_closed_events_reap_it() -> Result<()> {
    for mode in ["shutdown", "drop", "events_closed"] {
        let (root, options) = options()?;
        let mut bridge = support::start(options)?;
        let child = bridge.runtime_pid();
        ensure!(child != std::process::id());
        println!(
            "bridge spawned actual child={child} test={}",
            std::process::id()
        );
        let deadline = Instant::now() + TIMEOUT;
        let mut session = None;
        let mut requested = false;
        let shell;
        loop {
            match next(&bridge, deadline)? {
                Event::Ready { hierarchy, .. } => {
                    session = Some(hierarchy.session.context("Ready Shell")?.id);
                }
                Event::Frame {
                    session_id,
                    sequence,
                    screen,
                    ..
                } => {
                    bridge.commands.send(Command::Applied {
                        session_id,
                        sequence,
                    })?;
                    let rows: Vec<_> = screen
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
                    if !requested && rows.iter().any(|row| row == PROMPT) {
                        bridge.commands.send(Command::Input {
                            session_id: session.clone().context("Ready before prompt")?,
                            bytes: b"printf 'BRIDGE_OWNER=%s:%s\\n' \"$$\" \"$PPID\"\r".to_vec(),
                            defer_ordering: false,
                        })?;
                        requested = true;
                    }
                    if let Some((pid, parent)) = rows
                        .iter()
                        .find_map(|row| row.strip_prefix("BRIDGE_OWNER=")?.split_once(':'))
                    {
                        shell = pid.parse::<u32>()?;
                        ensure!(
                            parent.parse::<u32>()? == child,
                            "PTY owned by desktop/test, not runtime child"
                        );
                        println!("actual Shell={shell} PPID={parent}, mode={mode}");
                        break;
                    }
                }
                Event::Diagnostic(error) => bail!(error),
                Event::Closed => bail!("child closed before real PTY ownership"),
                _ => {}
            }
        }
        if mode == "drop" {
            drop(bridge);
        } else if mode == "events_closed" {
            bridge.events.close();
            // Refresh forces a real event after the receiver has closed. Do not
            // call shutdown until exit: that would mask a missing stop wakeup.
            bridge.commands.send(Command::Refresh)?;
            gone(child)?;
            bridge.shutdown()?;
        } else {
            bridge.shutdown()?;
            let mut closed = 0;
            while let Ok(event) = bridge.events.try_recv() {
                match event {
                    Event::Closed => closed += 1,
                    Event::Diagnostic(error) => bail!(error),
                    _ => {}
                }
            }
            ensure!(closed == 1, "terminal Closed count={closed}");
            ensure!(
                bridge.commands.send(Command::Refresh).is_err(),
                "writer still receiving after exit"
            );
        }
        gone(child)?;
        gone(shell)?;
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

#[test]
fn real_host_failure_never_reports_ready_and_closes_once_after_actual_exit() -> Result<()> {
    let (root, options) = options()?;
    fs::write(&options.data_root, b"ordinary file cannot host a database")?;
    let mut bridge = support::start(options)?;
    let child = bridge.runtime_pid();
    println!("bridge host-failure child={child}");
    let deadline = Instant::now() + TIMEOUT;
    let mut diagnostic = false;
    loop {
        match next(&bridge, deadline)? {
            Event::Ready { .. } => bail!("spawn/Hello incorrectly became Ready"),
            Event::Diagnostic(_) => diagnostic = true,
            Event::Closed => break,
            _ => {}
        }
    }
    ensure!(diagnostic, "failed runtime emitted no Diagnostic");
    ensure!(
        bridge.shutdown().is_err(),
        "failed child became successful shutdown"
    );
    ensure!(
        bridge.events.try_recv().is_err(),
        "events after terminal Closed"
    );
    gone(child)?;
    fs::remove_dir_all(root)?;
    Ok(())
}
