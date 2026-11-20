#![cfg(target_os = "macos")]

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use matou_native_core::{
    journal::{Frame, JournalOptions, for_each_frame},
    pty::ShellLaunch,
};
use matou_native_runtime::{
    SessionEvent, SessionOptions,
    host::{Command as HostCommand, Event as HostEvent, LaunchOptions, RuntimeHost},
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};
use matou_native_terminal::{Screen, SelectionAction, SelectionMode, Terminal, TerminalOptions};
use serde::Deserialize;
use serde_json::{Value, json};

#[path = "../../desktop/src/terminal_scrollbar.rs"]
mod terminal_scrollbar;
use terminal_scrollbar::ScrollbarMetrics;

const SESSION_ID: &str = "scroll-position";

fn options(root: &Path) -> Result<SessionOptions> {
    let home = root.join("home");
    let cwd = root.join("workspace");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&cwd)?;
    fs::write(home.join(".zshrc"), "PROMPT='SCROLL_READY> '\n")?;
    Ok(SessionOptions {
        session_id: SESSION_ID.into(),
        launch: ShellLaunch {
            profile: matou_native_core::pty::LaunchProfile::Shell,
            argv: Vec::new(),
            executable: "/bin/zsh".into(),
            cwd,
            data_root: root.to_owned(),
            inherited_env: BTreeMap::from([
                ("HOME".into(), home.clone().into_os_string()),
                ("ZDOTDIR".into(), home.into_os_string()),
                ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
                ("SHELL".into(), "/bin/zsh".into()),
                ("LANG".into(), "en_US.UTF-8".into()),
            ]),
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
        },
        terminal: TerminalOptions::new(80, 24, 9, 18),
        journal: JournalOptions {
            compress_sealed: false,
            ..JournalOptions::default()
        },
        restored_history: Vec::new(),
        collect_history: true,
    })
}

fn wait_for_output(incoming: &Receiver<WorkerEvent>, marker: &[u8]) -> Result<u64> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut output = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match incoming
            .recv_timeout(remaining)
            .context("wait for real Shell output")?
            .event
        {
            SessionEvent::Output { sequence, data } => {
                output.extend(data);
                if output.windows(marker.len()).any(|part| part == marker) {
                    return Ok(sequence);
                }
            }
            SessionEvent::Diagnostic { message } | SessionEvent::StorageFault { message, .. } => {
                bail!("unexpected Shell worker diagnostic: {message}")
            }
            SessionEvent::Exited { .. } => bail!("Shell exited before writing all rows"),
            _ => {}
        }
    }
}

fn screen_rows(screen: &Screen) -> impl Iterator<Item = String> {
    screen.cells.chunks(screen.cols).map(|row| {
        row.iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
    })
}

#[test]
fn queued_absolute_scroll_targets_use_the_latest_committed_viewport() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (events, incoming) = mpsc::channel();
    let mut worker = SessionWorker::spawn(options(root.path())?, events, "run-scroll".into())?;
    wait_for_output(&incoming, b"SCROLL_READY> ")?;
    // Leave the shell waiting after the marker so no prompt can append more rows.
    worker.write(
        b"printf 'SCROLL_ROW_%03d\\n' {0..79}; printf 'SCROLL_%s\\n' 'DONE'; read scroll_hold\r",
    )?;
    let sequence = wait_for_output(&incoming, b"SCROLL_DONE\r\n")?;
    let mut persisted = Vec::new();
    for_each_frame(root.path(), SESSION_ID, 1, Some(sequence), |record| {
        if let Frame::Output { data, .. } = record.frame {
            persisted.extend(data);
        }
        Ok(())
    })?;
    assert!(
        persisted
            .windows(b"SCROLL_DONE\r\n".len())
            .any(|part| part == b"SCROLL_DONE\r\n")
    );

    let initial = worker.screen()?;
    let last = initial.scrollbar.total - initial.rows as u64;
    assert_eq!(initial.scrollbar.offset, last);
    assert!(last >= 50, "the live Shell output creates scrollback");
    let selected = screen_rows(&initial)
        .position(|row| row.starts_with("SCROLL_ROW_079"))
        .context("find the last emitted line in the committed viewport")?;
    let selected = last + selected as u64;
    let first_output = selected - 79;
    let middle = last / 2;

    let (reply, selection_ready) = mpsc::channel();
    worker.send(WorkerCommand::Select {
        action: SelectionAction::Begin {
            mode: SelectionMode::Word,
            row: usize::try_from(selected)?,
            x: 1.,
        },
        reply,
    })?;
    let mut reads = Vec::new();
    for (target, expected) in [
        (0, 0),
        (middle, middle),
        (last, last),
        (middle, middle),
        (u64::MAX, last),
    ] {
        let (reply, scrolled) = mpsc::channel();
        worker.send(WorkerCommand::ScrollTo {
            row: target,
            pixel_target: false,
            reply,
        })?;
        let (reply, view) = mpsc::channel();
        worker.send(WorkerCommand::SelectedView { reply })?;
        reads.push((expected, scrolled, view));
    }
    let (reply, copied) = mpsc::channel();
    worker.send(WorkerCommand::CopySelection {
        reply,
        interrupt_if_empty: false,
    })?;

    // Every move and screen read was enqueued before waiting for any reply.
    let timeout = Duration::from_secs(2);
    selection_ready.recv_timeout(timeout)??;
    for (expected, scrolled, view) in reads {
        scrolled.recv_timeout(timeout)??;
        let (screen, _, selection) = view.recv_timeout(timeout)??;
        assert_eq!(screen.scrollbar.offset, expected);
        let selection = selection.context("scrolling preserves the existing selection")?;
        assert_eq!(selection.anchor.row as u64, selected);
        if expected == middle {
            let first_visible = screen_rows(&screen).next().context("read top row")?;
            assert!(first_visible.starts_with(&format!("SCROLL_ROW_{:03}", middle - first_output)));
        }
    }
    assert_eq!(copied.recv_timeout(timeout)??.text, "SCROLL_ROW_079");
    worker.shutdown()?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScrollGateInput {
    grid: Grid,
    scenarios: Vec<Scenario>,
    frame_case: FrameCase,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Grid {
    cols: u16,
    cell_height: u32,
}

#[derive(Deserialize)]
struct Scenario {
    id: String,
    rows: u16,
    seed: u32,
    history: Option<usize>,
    steps: Vec<Vec<Value>>,
}

#[derive(Deserialize)]
struct FrameCase {
    id: String,
    rows: u16,
    seed: u32,
    batches: Vec<Vec<Vec<Value>>>,
}

fn numbered_lines(start: u32, count: u32) -> Vec<u8> {
    let mut output = String::new();
    for row in start..start + count {
        if row > 0 {
            output.push_str("\r\n");
        }
        output.push_str(&format!("ROW_{row:05}"));
    }
    output.into_bytes()
}

fn snapshot(screen: &Screen) -> Value {
    json!({
        "rows": screen.rows,
        "total": screen.scrollbar.total,
        "offset": screen.scrollbar.offset,
        "firstLine": screen_rows(screen).next().unwrap().trim_end_matches(' '),
    })
}

fn geometry(screen: &Screen, cell_height: u32, pending: Option<f64>) -> Value {
    let cell = f64::from(cell_height);
    let viewport = screen.rows as f64 * cell;
    let extent = screen.scrollbar.total as f64 * cell;
    let position = pending.unwrap_or(
        screen
            .resize_position_row
            .unwrap_or(screen.scrollbar.offset) as f64
            * cell,
    );
    let metrics = ScrollbarMetrics::new(viewport, extent, position);
    json!({
        "viewport": viewport,
        "position": position,
        "roundedPosition": metrics.map_or(0., |metrics| metrics.position),
        "needed": metrics.is_some(),
        "thumbSize": metrics.map(|metrics| metrics.thumb_size),
        "thumbTop": metrics.map(|metrics| metrics.thumb_top),
    })
}

fn legacy_geometry(fixture: Value) -> Vec<Value> {
    fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let viewport = case["viewport"].as_f64().unwrap();
            let extent = case["extent"].as_f64().unwrap();
            let position = case["position"].as_f64().unwrap();
            let metrics = ScrollbarMetrics::new(viewport, extent, position);
            let probes: Vec<_> = case["probes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|probe| {
                    let op = probe["op"].as_str().unwrap();
                    let value = probe["value"].as_f64().unwrap();
                    let result = metrics.map(|metrics| match op {
                        "track" => metrics.track_position(value),
                        "drag" => metrics.drag_position(value),
                        _ => unreachable!(),
                    });
                    json!({ "op": op, "value": value, "result": result })
                })
                .collect();
            json!({
                "viewport": viewport, "extent": extent, "position": position,
                "needed": metrics.is_some(),
                "thumbSize": metrics.map(|metrics| metrics.thumb_size),
                "thumbTop": metrics.map(|metrics| metrics.thumb_top),
                "probes": probes,
            })
        })
        .collect()
}

fn run_mapping(probe: &str, cell: u32, args: &[String]) -> Result<Vec<String>> {
    let output = std::process::Command::new(
        std::env::var_os("MATOU_SCROLL_MAPPING").context("mapping probe path")?,
    )
    .arg(probe)
    .arg(cell.to_string())
    .args(args)
    .output()?;
    anyhow::ensure!(
        output.status.success(),
        "mapping {probe}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(str::to_owned)
        .collect())
}

fn capture_scenario(grid: &Grid, scenario: Scenario) -> Result<Value> {
    let mut options = TerminalOptions::new(grid.cols, scenario.rows, 9, grid.cell_height);
    if let Some(history) = scenario.history {
        options.history_lines = history;
    }
    let mut terminal = Terminal::new(options)?;
    terminal.feed(&numbered_lines(0, scenario.seed));
    let mut next_line = scenario.seed;
    let mut drag: Option<ScrollbarMetrics> = None;
    let mut pending = None;
    let mut states = Vec::new();
    let cell = f64::from(grid.cell_height);

    for step in std::iter::once(None).chain(scenario.steps.iter().map(Some)) {
        let mut action_output = Value::Null;
        if let Some(step) = step {
            let op = step[0].as_str().unwrap();
            let screen = terminal.screen()?;
            let number = || step[1].as_u64().unwrap();
            let viewport = screen.rows as f64 * cell;
            let extent = screen.scrollbar.total as f64 * cell;
            let position = pending.unwrap_or(
                screen
                    .resize_position_row
                    .unwrap_or(screen.scrollbar.offset) as f64
                    * cell,
            );
            if op != "drag" {
                drag = None;
            }
            match op {
                "absolute" => {
                    pending = None;
                    terminal.scroll_to(number(), false)?;
                }
                "track" | "drag" => {
                    let metrics = ScrollbarMetrics::new(viewport, extent, position)
                        .context("track or drag requires scrollable content")?;
                    let value = step[1].as_f64().unwrap();
                    let pixels = if op == "track" {
                        metrics.track_position(value)
                    } else {
                        drag.get_or_insert(metrics).drag_position(value)
                    };
                    let mapped = run_mapping("row", grid.cell_height, &[pixels.to_string()])?;
                    terminal.scroll_to(mapped[0].parse()?, true)?;
                    pending = Some(pixels);
                }
                "wheel" => {
                    let path = step[1].as_str().unwrap();
                    let mut args = vec![
                        step[2].as_str().unwrap().into(),
                        (-step[3].as_f64().unwrap()).to_string(),
                        step[4].as_bool().unwrap().to_string(),
                    ];
                    args.extend([viewport, extent, position].map(|n| n.to_string()));
                    let result = run_mapping(path, grid.cell_height, &args)?;
                    if let Some(output) = result.first() {
                        let (pixels, row) = output.split_once(' ').context("wheel target")?;
                        terminal.scroll_to(row.parse()?, true)?;
                        pending = Some(pixels.parse()?);
                    }
                }
                "page" => {
                    let result = run_mapping(
                        "page",
                        grid.cell_height,
                        &[
                            step[1].to_string(),
                            screen.rows.to_string(),
                            screen.scrollbar.total.to_string(),
                            position.to_string(),
                        ],
                    )?;
                    if let Some(output) = result.first() {
                        if let Some(bytes) = output.strip_prefix("bytes:") {
                            action_output = json!(bytes);
                        } else {
                            let (pixels, row) = output.split_once(' ').context("page target")?;
                            terminal.scroll_to(row.parse()?, true)?;
                            pending = Some(pixels.parse()?);
                        }
                    }
                }
                "append" => {
                    let count = u32::try_from(number())?;
                    terminal.feed(&numbered_lines(next_line, count));
                    next_line += count;
                    pending = None;
                }
                "clearHistory" => {
                    terminal.feed(b"\x1b[3J");
                    pending = None;
                }
                "resize" => {
                    terminal.resize(grid.cols, u16::try_from(number())?, 9, grid.cell_height)?;
                    pending = None;
                }
                _ => bail!("unrecognized scroll operation {op}"),
            }
        }
        let screen = terminal.screen()?;
        let mut state = snapshot(&screen);
        state["geometry"] = geometry(&screen, grid.cell_height, pending);
        state["actionOutput"] = action_output;
        state["historyTrimmed"] = json!(terminal.history_trimmed_rows()?);
        states.push(state);
    }
    Ok(json!({ "id": scenario.id, "states": states }))
}

fn wait_host_frame(
    host: &RuntimeHost,
    predicate: impl Fn(&str, &Screen, u64) -> bool,
) -> Result<(String, Screen, u64, u64)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match host.events.try_recv() {
            Ok(HostEvent::Frame {
                session_id,
                screen,
                sequence,
                scroll_request_id,
                ..
            }) => {
                if predicate(&session_id, &screen, scroll_request_id) {
                    return Ok((session_id, screen, sequence, scroll_request_id));
                }
                host.commands.send(HostCommand::Applied {
                    session_id,
                    sequence,
                })?;
            }
            Ok(HostEvent::Diagnostic(message) | HostEvent::StorageFault { message, .. }) => {
                bail!("native host diagnostic: {message}")
            }
            Ok(HostEvent::Closed) => bail!("native host closed"),
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(2)),
            Err(error) => return Err(error).context("waiting for native host frame"),
        }
    }
    bail!("native host frame timed out")
}

fn capture_host_frames(grid: &Grid, case: FrameCase) -> Result<Value> {
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    let data_root = root.path().join("data");
    fs::create_dir_all(&home)?;
    fs::write(home.join("seed"), numbered_lines(0, case.seed))?;
    fs::write(
        home.join(".zshrc"),
        "/bin/stty raw -echo\nprintf 'SCROLL_FRAME_READY'\nread -k 1\nprintf '\\033[2J\\033[H'\n/bin/cat \"$HOME/seed\"\nexec /bin/cat >/dev/null\n",
    )?;
    let mut host = RuntimeHost::start(LaunchOptions {
        data_root: data_root.clone(),
        workspace: root.path().join("workspace"),
        shell_home: home,
        shell: "/bin/zsh".into(),
    })?;
    let (id, _, ready_sequence, _) = wait_host_frame(&host, |_, screen, _| {
        screen_rows(screen).any(|row| row.starts_with("SCROLL_FRAME_READY"))
    })?;
    let resize = |rows| HostCommand::Resize {
        session_id: id.clone(),
        cols: grid.cols,
        rows,
        cell_width_px: 9,
        cell_height_px: grid.cell_height,
    };
    let ack = |sequence| HostCommand::Applied {
        session_id: id.clone(),
        sequence,
    };
    host.commands.send(resize(case.rows))?;
    host.commands.send(ack(ready_sequence))?;
    let (_, _, resized_sequence, _) = wait_host_frame(&host, |session, screen, _| {
        session == id && screen.rows == usize::from(case.rows)
    })?;
    host.commands.send(HostCommand::Input {
        session_id: id.clone(),
        bytes: vec![b's'],
        defer_ordering: false,
    })?;
    host.commands.send(ack(resized_sequence))?;
    let seed_end = format!("ROW_{:05}", case.seed - 1);
    let (_, initial, initial_sequence, _) = wait_host_frame(&host, |session, screen, _| {
        session == id && screen_rows(screen).any(|row| row.starts_with(&seed_end))
    })?;
    let mut request = 0;
    let mut previous_sequence = initial_sequence;
    let mut frames = Vec::new();
    let mut resizes_applied = 0;
    for (index, batch) in case.batches.iter().enumerate() {
        let mut target_rows = None;
        for step in batch {
            let value = step[1].as_u64().unwrap();
            let command = match step[0].as_str().unwrap() {
                "absolute" | "pixel" => {
                    request += 1;
                    HostCommand::ScrollTo {
                        session_id: id.clone(),
                        row: value,
                        request_id: request,
                        pixel_target: step[0] == "pixel",
                    }
                }
                "resize" => {
                    let rows = u16::try_from(value)?;
                    target_rows = Some(usize::from(rows));
                    resize(rows)
                }
                other => bail!("unexpected frame operation {other}"),
            };
            host.commands.send(command)?;
        }
        // Keep the last frame unacknowledged while the whole batch is sent.
        host.commands.send(ack(previous_sequence))?;
        let (_, screen, sequence, applied_request) =
            wait_host_frame(&host, |session, _, _| session == id)?;
        anyhow::ensure!(
            applied_request == request && target_rows.is_none_or(|rows| screen.rows == rows),
            "unexpected intermediate frame after batch {index}: request {applied_request}, rows {}",
            screen.rows
        );
        resizes_applied += usize::from(target_rows.is_some());
        let mut frame = snapshot(&screen);
        frame["ordinal"] = json!(index + 1);
        frame["resizesApplied"] = json!(resizes_applied);
        frame["geometry"] = geometry(&screen, grid.cell_height, None);
        frame["outputSequenceDelta"] = json!(i128::from(sequence) - i128::from(initial_sequence));
        frame["scrollRequestId"] = json!(applied_request);
        frames.push(frame);
        previous_sequence = sequence;
    }
    host.commands.send(ack(previous_sequence))?;
    host.shutdown()?;
    let mut resize_records = Vec::new();
    for_each_frame(&data_root, &id, initial_sequence + 1, None, |record| {
        if let Frame::Resize {
            sequence,
            cols,
            rows,
        } = record.frame
        {
            resize_records.push(
                json!({ "sequenceDelta": sequence - initial_sequence, "cols": cols, "rows": rows }),
            );
        }
        Ok(())
    })?;
    anyhow::ensure!(
        resize_records.len() == resizes_applied,
        "resize records do not match returned grids"
    );
    Ok(
        json!({ "id": case.id, "initial": snapshot(&initial), "frames": frames, "resizeRecords": resize_records }),
    )
}

#[test]
fn resize_preserves_order_between_pixel_and_line_targets() -> Result<()> {
    let result = capture_host_frames(
        &Grid {
            cols: 80,
            cell_height: 14,
        },
        FrameCase {
            id: "mixed-scroll-targets".into(),
            rows: 20,
            seed: 80,
            batches: serde_json::from_value(json!([
                [["absolute", 16]],
                [["resize", 30], ["pixel", 27], ["absolute", 37]],
                [["resize", 20], ["absolute", 20], ["pixel", 32]]
            ]))?,
        },
    )?;
    let frames = result["frames"].as_array().context("mixed target frames")?;
    assert_eq!(frames.len(), 3);
    for (frame, (ordinal, request, expected, line)) in frames.iter().zip([
        (1, 1, 16, "ROW_00016"),
        (2, 3, 37, "ROW_00037"),
        (3, 5, 32, "ROW_00032"),
    ]) {
        assert_eq!(frame["ordinal"], ordinal);
        assert_eq!(frame["scrollRequestId"], request);
        assert_eq!(frame["offset"], expected);
        assert_eq!(frame["firstLine"], line);
    }
    Ok(())
}

#[test]
#[ignore = "run through the terminal scroll gate with isolated inputs and outputs"]
fn capture_terminal_scroll_gate() -> Result<()> {
    let input_path = std::env::var_os("MATOU_SCROLL_INPUT").context("scroll input path")?;
    let input: ScrollGateInput = serde_json::from_slice(&fs::read(input_path)?)?;
    let fixture = serde_json::from_str(include_str!(
        "../../desktop/fixtures/terminal-scrollbar.json"
    ))?;
    let source: Value = serde_json::from_slice(&fs::read(
        std::env::var_os("MATOU_SCROLL_AUDIT").context("source audit path")?,
    )?)?;
    let scenarios = input
        .scenarios
        .into_iter()
        .map(|case| capture_scenario(&input.grid, case))
        .collect::<Result<Vec<_>>>()?;
    let frames = capture_host_frames(&input.grid, input.frame_case)?;
    let result = json!({
        "geometry": legacy_geometry(fixture),
        "insets": source["nativeInsets"],
        "scenarios": scenarios,
        "frameCase": frames,
    });
    let output = std::env::var_os("MATOU_SCROLL_OUTPUT").context("native scroll output path")?;
    fs::write(output, serde_json::to_string_pretty(&result)? + "\n")?;
    Ok(())
}
