#![cfg(target_os = "macos")]

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use matou_native_desktop::runtime_bridge::{Command, Event, LaunchOptions, RuntimeBridge};
use matou_native_terminal::Screen;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(8);
const PROMPT: &str = "MATOU_BRIDGE_READY>";
const DIVIDER: &str = "──────── 会话已恢复 ────────";

#[test]
fn frames_continue_after_acknowledgment_and_completed_output_survives_reopen() -> Result<()> {
    let root = std::env::temp_dir().join(format!("matou-native-bridge-{}", Uuid::new_v4()));
    verify_bridge(&root).with_context(|| format!("isolated bridge data: {}", root.display()))?;
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn coalesced_scroll_frames_identify_the_requests_already_applied() -> Result<()> {
    let root = std::env::temp_dir().join(format!("matou-native-scroll-bridge-{}", Uuid::new_v4()));
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
    let mut bridge = support::start(LaunchOptions {
        data_root: root.join("data"),
        workspace: root.join("workspace"),
        shell_home: home,
        shell: PathBuf::from("/bin/zsh"),
    })?;
    let session_id = wait_for_ready(&bridge)?;
    wait_for_frame(&bridge, &session_id, "initial scroll prompt", |rows| {
        has_row(rows, PROMPT)
    })?;
    input(
        &bridge,
        &session_id,
        "printf 'BRIDGE_SCROLL_%03d\\n' {0..99}; printf 'SCROLL_%s\\n' DONE; read scroll_hold",
    )?;
    wait_for_frame(&bridge, &session_id, "scrollable history", |rows| {
        has_row(rows, "SCROLL_DONE")
    })?;
    for command in [
        Command::ScrollTo {
            session_id: session_id.clone(),
            row: 30,
            request_id: 1,
        },
        Command::ScrollTo {
            session_id: session_id.clone(),
            row: 0,
            request_id: 2,
        },
        Command::Scroll {
            session_id: session_id.clone(),
            lines: -5,
        },
    ] {
        bridge.commands.send(command)?;
    }
    wait_for_event(&bridge, |event| {
        if let Event::Frame {
            session_id: id,
            sequence,
            screen,
            scroll_request_id,
            ..
        } = event
        {
            bridge.commands.send(Command::Applied {
                session_id: id,
                sequence,
            })?;
            if scroll_request_id == 2 && screen.scrollbar.offset == 5 {
                return Ok(Some(()));
            }
        }
        Ok(None)
    })?
    .context(
        "coalesced frame did not preserve the latest scroll request and following wheel position",
    )?;
    bridge.shutdown()?;
    fs::remove_dir_all(root)?;
    Ok(())
}

fn verify_bridge(root: &Path) -> Result<()> {
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
    let options = LaunchOptions {
        data_root: root.join("data"),
        workspace: root.join("workspace"),
        shell_home: home,
        shell: PathBuf::from("/bin/zsh"),
    };

    let mut bridge = support::start(options.clone())?;
    let session_id = wait_for_ready(&bridge)?;
    let initial = wait_for_frame(&bridge, &session_id, "initial prompt", |rows| {
        has_row(rows, PROMPT)
    })?;

    input(
        &bridge,
        &session_id,
        "printf 'MATOU_%s\\n' '中文_开始'; sleep 0.2; printf 'MATOU_%s\\n' '中文_完成'",
    )?;
    let started = wait_for_frame(&bridge, &session_id, "first output frame", |rows| {
        has_row(rows, "MATOU_中文_开始") && !has_row(rows, "MATOU_中文_完成")
    })?;
    ensure!(started > initial, "first output did not advance sequence");
    let finished = wait_for_frame(
        &bridge,
        &session_id,
        "final output without more input",
        |rows| {
            has_row(rows, "MATOU_中文_开始")
                && has_row(rows, "MATOU_中文_完成")
                && has_row(rows, PROMPT)
        },
    )?;
    ensure!(finished > started, "final output did not advance sequence");

    input(&bridge, &session_id, "printf 'MATOU_%s\\n' '确认后_继续'")?;
    let second = wait_for_frame(
        &bridge,
        &session_id,
        "next command after frame acknowledgment",
        |rows| has_row(rows, "MATOU_确认后_继续") && has_row(rows, PROMPT),
    )?;
    ensure!(second > finished, "second command did not advance sequence");
    bridge.shutdown()?;

    let mut reopened = support::start(options)?;
    let reopened_id = wait_for_ready(&reopened)?;
    ensure!(reopened_id == session_id, "reopened a different session");
    wait_for_frame(
        &reopened,
        &session_id,
        "completed output after reopen",
        |rows| {
            has_row(rows, "MATOU_中文_开始")
                && has_row(rows, "MATOU_中文_完成")
                && has_row(rows, "MATOU_确认后_继续")
                && has_row(rows, DIVIDER)
                && has_row(rows, PROMPT)
        },
    )?;
    input(&reopened, &session_id, "printf 'MATOU_%s\\n' '重开后_可用'")?;
    wait_for_frame(
        &reopened,
        &session_id,
        "live command after reopen",
        |rows| has_row(rows, "MATOU_重开后_可用") && has_row(rows, PROMPT),
    )?;
    reopened.shutdown()?;
    Ok(())
}

fn input(bridge: &RuntimeBridge, session_id: &str, command: &str) -> Result<()> {
    bridge
        .commands
        .send(Command::Input {
            session_id: session_id.to_owned(),
            bytes: format!("{command}\r").into_bytes(),
            defer_ordering: false,
        })
        .map_err(|_| anyhow!("bridge disconnected while sending input"))
}

fn wait_for_ready(bridge: &RuntimeBridge) -> Result<String> {
    wait_for_event(bridge, |event| match event {
        Event::Ready { graph, .. } => {
            let id = graph["nodes"]
                .as_array()
                .and_then(|nodes| nodes.iter().find(|node| node["currentMode"] == "shell"))
                .and_then(|node| node["sessionId"].as_str())
                .context("Ready graph has no shell session")?;
            Ok(Some(id.to_owned()))
        }
        _ => Ok(None),
    })?
    .context("timed out waiting for bridge Ready")
}

fn wait_for_frame(
    bridge: &RuntimeBridge,
    session_id: &str,
    phase: &str,
    matches: impl Fn(&[String]) -> bool,
) -> Result<u64> {
    let mut rows = Vec::new();
    let mut last_sequence = None;
    let sequence = wait_for_event(bridge, |event| {
        let Event::Frame {
            session_id: actual_id,
            screen,
            sequence,
            ..
        } = event
        else {
            return Ok(None);
        };
        ensure!(
            actual_id == session_id,
            "unexpected frame for session {actual_id}"
        );
        bridge
            .commands
            .send(Command::Applied {
                session_id: actual_id,
                sequence,
            })
            .map_err(|_| anyhow!("bridge disconnected while acknowledging frame {sequence}"))?;
        rows = screen_rows(&screen);
        last_sequence = Some(sequence);
        Ok(matches(&rows).then_some(sequence))
    })
    .with_context(|| format!("{phase}, last acknowledged frame: {last_sequence:?}"))?;
    sequence.with_context(|| {
        format!(
            "timed out waiting for {phase}, last screen:\n{}",
            rows.join("\n")
        )
    })
}

fn wait_for_event<T>(
    bridge: &RuntimeBridge,
    mut accept: impl FnMut(Event) -> Result<Option<T>>,
) -> Result<Option<T>> {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match bridge.events.try_recv() {
            Ok(Event::Diagnostic(message)) => bail!("bridge diagnostic: {message}"),
            Ok(Event::StorageFault { message, .. }) => bail!("bridge storage fault: {message}"),
            Ok(Event::Closed) | Err(async_channel::TryRecvError::Closed) => {
                bail!("bridge closed while waiting for a frame")
            }
            Ok(event) => {
                if let Some(value) = accept(event)? {
                    return Ok(Some(value));
                }
            }
            Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(4)),
        }
    }
    Ok(None)
}

fn screen_rows(screen: &Screen) -> Vec<String> {
    screen
        .cells
        .chunks(screen.cols)
        .map(|cells| {
            let row: String = cells.iter().map(|cell| cell.text.as_str()).collect();
            row.trim_end_matches(' ').to_owned()
        })
        .collect()
}

fn has_row(rows: &[String], expected: &str) -> bool {
    rows.iter().any(|row| row == expected)
}
