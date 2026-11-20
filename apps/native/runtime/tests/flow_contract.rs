use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use matou_native_runtime::flow::{
    BATCH_DELAY, CreditWindow, MAX_CODE_UNITS, OutputBatcher, Utf8Decoder,
};
use serde_json::Value;

const STREAM_REFERENCE: &str = include_str!("../../core/tests/fixtures/stream-reference.json");

#[test]
fn streams_utf8_with_reference_chunk_boundaries() -> Result<()> {
    let fixture: Value = serde_json::from_str(STREAM_REFERENCE)?;
    let cases = fixture["decoding"].as_array().context("decoding cases")?;
    assert_eq!(cases.len(), 7);
    for case in cases {
        let chunks: Vec<Vec<u8>> = serde_json::from_value(case["chunks"].clone())?;
        let mut decoder = Utf8Decoder::new();
        let outputs: Vec<_> = chunks.iter().map(|chunk| decoder.feed(chunk)).collect();
        assert_eq!(serde_json::to_value(outputs)?, case["outputs"]);
        assert_eq!(decoder.finish(), case["end"]);
    }

    // StringDecoder waits for the structural suffix, including an invalid
    // surrogate triplet split across reads, before emitting replacements.
    let mut decoder = Utf8Decoder::new();
    assert_eq!(decoder.feed(&[0xed, 0xa0]), "");
    assert_eq!(decoder.feed(&[0x80]), "���");
    assert_eq!(decoder.finish(), "");
    Ok(())
}

#[test]
fn batches_by_utf16_units_and_first_chunk_deadline() -> Result<()> {
    let fixture: Value = serde_json::from_str(STREAM_REFERENCE)?;
    let cases = fixture["batching"].as_array().context("batching cases")?;
    assert_eq!(cases.len(), 3);
    for case in cases {
        let units: usize = serde_json::from_value(case["maxCodeUnits"].clone())?;
        let mut batcher = OutputBatcher::new(BATCH_DELAY, units)?;
        let start = Instant::now();
        let mut emitted = Vec::new();
        for step in case["steps"].as_array().context("batching steps")? {
            emitted.extend(batcher.offer(step["chunk"].as_str().context("chunk")?, start));
            assert_eq!(serde_json::to_value(&emitted)?, step["emitted"]);
        }
        emitted.extend(batcher.flush());
        assert_eq!(serde_json::to_value(emitted)?, case["flushed"]);
        assert!(batcher.flush().is_none());
        assert!(batcher.deadline().is_none());
    }

    let mut batcher = OutputBatcher::new(BATCH_DELAY, MAX_CODE_UNITS)?;
    let now = Instant::now();
    assert!(batcher.offer("", now).is_none());
    assert_eq!(batcher.deadline(), None);
    assert!(batcher.offer("中", now).is_none());
    assert!(
        batcher
            .offer("🙂", now + Duration::from_millis(15))
            .is_none()
    );
    assert_eq!(batcher.deadline(), Some(now + Duration::from_millis(16)));
    assert_eq!(batcher.take_due(now + Duration::from_millis(15)), None);
    assert_eq!(
        batcher.take_due(now + Duration::from_millis(16)).as_deref(),
        Some("中🙂")
    );
    assert_eq!(batcher.deadline(), None);
    Ok(())
}

#[test]
fn matches_reference_credit_transitions_and_cumulative_acknowledgements() -> Result<()> {
    let fixture: Value = serde_json::from_str(STREAM_REFERENCE)?;
    let cases = fixture["credit"].as_array().context("credit cases")?;
    assert_eq!(cases.len(), 2);
    for case in cases {
        let mut window = CreditWindow::new(100, 40)?;
        let mut events = Vec::new();
        for step in case["steps"].as_array().context("credit steps")? {
            let action = &step["action"];
            let sequence: u64 = serde_json::from_value(action[1].clone())?;
            let previously_paused = window.is_paused();
            let result = match action[0].as_str() {
                Some("send") => {
                    let bytes = serde_json::from_value(action[2].clone())?;
                    window.record_sent(sequence, bytes).map(|()| false)
                }
                Some("ack") => window.acknowledge(sequence),
                _ => bail!("unknown credit action {action}"),
            };
            if !previously_paused && window.is_paused() {
                events.push("pause");
            }
            if matches!(&result, Ok(true)) {
                assert!(previously_paused && !window.is_paused());
                events.push("resume");
            }
            assert_eq!(
                serde_json::to_value(result.err().map(|error| error.to_string()))?,
                step["error"]
            );
            assert_eq!(
                serde_json::to_value(window.unacked_bytes())?,
                step["unackedBytes"]
            );
            assert_eq!(Value::Bool(window.is_paused()), step["paused"]);
            assert_eq!(serde_json::to_value(&events)?, step["events"]);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
use matou_native_runtime::flow;
#[cfg(target_os = "macos")]
use probe_model::{SessionEvent, SessionOptions, StorageFaultCode};
#[cfg(target_os = "macos")]
#[path = "../src/model.rs"]
#[allow(dead_code)]
mod probe_model;
#[cfg(target_os = "macos")]
#[path = "../src/search.rs"]
#[allow(dead_code)]
mod search;

/// Enter immediately after decoding and batching so credit sees fixed frames.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod fixed_delivery {
    include!("../src/session.rs");

    pub fn output(session: &mut Session, data: String) -> Result<()> {
        session.enqueue_output(data)
    }
}

#[cfg(target_os = "macos")]
mod gate {
    use super::*;
    use matou_native_core::{
        journal::{Frame, JournalOptions, Record, SegmentJournal, for_each_frame},
        pty::{LaunchProfile, ShellLaunch},
    };
    use matou_native_runtime::{
        host::Command, protocol::ClientMessage, session::Session as RuntimeSession,
    };
    use matou_native_terminal::{Screen, TerminalOptions};
    use serde::Deserialize;
    use serde_json::json;
    use std::{
        env, fs,
        path::{Path, PathBuf},
        thread,
    };

    #[derive(Clone, Copy, Deserialize)]
    struct Grid {
        cols: u16,
        rows: u16,
    }

    impl Grid {
        fn capture(screen: &Screen) -> Value {
            json!({"cols": screen.cols, "rows": screen.rows})
        }
    }

    fn hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8] = b"0123456789abcdef";
        let mut result = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            result.push(char::from(DIGITS[usize::from(byte >> 4)]));
            result.push(char::from(DIGITS[usize::from(byte & 15)]));
        }
        result
    }

    fn unhex(input: &str) -> Result<Vec<u8>> {
        let mut pairs = input.as_bytes().chunks_exact(2);
        anyhow::ensure!(pairs.remainder().is_empty(), "incomplete hex byte");
        pairs
            .by_ref()
            .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?))
            .collect()
    }

    fn packed(data: &[u8]) -> Value {
        if data.len() >= 1024 && data.iter().all(|byte| *byte == data[0]) {
            json!({"byteHex": hex(&data[..1]), "repeat": data.len()})
        } else {
            json!({"hex": hex(data)})
        }
    }

    fn decode(test: &Value) -> Result<Value> {
        let chunks: Vec<Vec<u8>> = serde_json::from_value(test["chunks"].clone())?;
        let mut decoder = Utf8Decoder::new();
        let outputs: Vec<_> = chunks.iter().map(|chunk| decoder.feed(chunk)).collect();
        let output_hex: Vec<_> = outputs
            .iter()
            .map(|output| hex(output.as_bytes()))
            .collect();
        let end = decoder.finish();
        let end_hex = hex(end.as_bytes());
        let stream_hex = format!("{}{}", output_hex.concat(), end_hex);
        Ok(json!({
            "outputs": outputs, "outputHex": output_hex, "end": end, "endHex": end_hex, "streamHex": stream_hex
        }))
    }

    fn batch(test: &Value) -> Result<Value> {
        let max = test["maxCodeUnits"]
            .as_u64()
            .map(usize::try_from)
            .transpose()?
            .unwrap_or(MAX_CODE_UNITS);
        let mut batcher = OutputBatcher::new(BATCH_DELAY, max)?;
        let start = Instant::now();
        let mut steps = Vec::new();
        let mut batches = Vec::new();
        for step in test["steps"].as_array().context("batch steps")? {
            let at = step[0].as_u64().context("batch time")?;
            let now = start + Duration::from_millis(at);
            if let Some(data) = batcher.take_due(now) {
                batches.push((at, data));
            }
            let output = match step[1].as_str().context("batch action")? {
                "offer" => batcher.offer(step[2].as_str().context("batch text")?, now),
                "flush" => batcher.flush(),
                "tick" => None,
                action => bail!("unknown batch action {action}"),
            };
            batches.extend(output.map(|data| (at, data)));
            steps.push(json!({
                "at": at, "totalBatches": batches.len(),
                "deadline": batcher.deadline().map(|time| time.duration_since(start).as_millis()),
            }));
        }
        let windows: Vec<(u64, u64)> = serde_json::from_value(test["windows"].clone())?;
        let counts: Vec<_> = windows
            .iter()
            .map(|(from, through)| {
                batches
                    .iter()
                    .filter(|(at, _)| (from..=through).contains(&at))
                    .count()
            })
            .collect();
        let stream_hex: String = batches
            .iter()
            .map(|(_, data)| hex(data.as_bytes()))
            .collect();
        let batches: Vec<_> = batches
            .iter()
            .map(|(at, data)| json!({"at": at, "dataHex": hex(data.as_bytes())}))
            .collect();
        Ok(json!({"steps": steps, "batches": batches, "counts": counts, "streamHex": stream_hex}))
    }

    fn credit(test: &Value) -> Result<Value> {
        let mut window = CreditWindow::new(
            serde_json::from_value(test["high"].clone())?,
            serde_json::from_value(test["low"].clone())?,
        )?;
        let mut events = Vec::new();
        let mut steps = Vec::new();
        for action in test["actions"].as_array().context("credit actions")? {
            let sequence = action[1].as_u64().context("credit sequence")?;
            let paused = window.is_paused();
            let result = match action[0].as_str().context("credit action")? {
                "send" => window
                    .record_sent(sequence, serde_json::from_value(action[2].clone())?)
                    .map(|()| false),
                "ack" => window.acknowledge(sequence),
                name => bail!("unknown credit action {name}"),
            };
            if !paused && window.is_paused() {
                events.push("pause");
            }
            if matches!(&result, Ok(true)) {
                events.push("resume");
            }
            steps.push(json!({
                "unackedBytes": window.unacked_bytes(), "paused": window.is_paused(),
                "events": events, "error": result.err().map(|error| error.to_string()),
            }));
        }
        Ok(json!({"steps": steps}))
    }

    fn launch(root: &Path, grid: Grid, profile: LaunchProfile) -> Result<ShellLaunch> {
        let executable = PathBuf::from(env::var_os("MATOU_FLOW_PROBE").context("PTY probe")?);
        let child_env = [
            ("HOME".into(), root.as_os_str().to_owned()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("SHELL".into(), executable.as_os_str().to_owned()),
            ("LANG".into(), "en_US.UTF-8".into()),
        ];
        Ok(ShellLaunch {
            profile,
            executable,
            argv: Vec::new(),
            cwd: root.to_owned(),
            data_root: root.to_owned(),
            inherited_env: child_env.clone().into(),
            env: child_env.into(),
            cols: grid.cols,
            rows: grid.rows,
        })
    }

    fn journal_options() -> JournalOptions {
        JournalOptions {
            compress_sealed: false,
            ..Default::default()
        }
    }

    fn runtime_options(
        root: &Path,
        id: &str,
        grid: Grid,
        profile: LaunchProfile,
    ) -> Result<matou_native_runtime::SessionOptions> {
        Ok(matou_native_runtime::SessionOptions {
            session_id: id.into(),
            launch: launch(root, grid, profile)?,
            terminal: TerminalOptions::new(grid.cols, grid.rows, 9, 18),
            journal: journal_options(),
            restored_history: Vec::new(),
            collect_history: false,
        })
    }

    fn wait_until(check: impl Fn() -> bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            anyhow::ensure!(
                Instant::now() < deadline,
                "timed out waiting for raw PTY probe"
            );
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    fn submit(test: &Value, root: &Path, id: &str) -> Result<Value> {
        let grid = Grid { cols: 24, rows: 5 };
        let mut session =
            RuntimeSession::launch(runtime_options(root, id, grid, LaunchProfile::Shell)?)?;
        wait_until(|| root.join("ready").is_file())?;
        let file = root.join("input.bin");
        let mut consumed = 0;
        let mut writes = Vec::new();
        let mut submission_events = Vec::new();
        for write in test["writes"].as_array().context("raw writes")? {
            let bytes = write.as_str().context("raw write")?.as_bytes();
            session.write(bytes)?;
            let end = consumed + bytes.len();
            wait_until(|| fs::metadata(&file).is_ok_and(|meta| meta.len() >= end as u64))?;
            let received = fs::read(&file)?;
            anyhow::ensure!(received.len() == end, "extra bytes from raw PTY probe");
            writes.push(hex(&received[consumed..end]));
            consumed = end;
            let events = session.poll(Duration::ZERO)?;
            submission_events.push(
                events
                    .iter()
                    .filter(|event| {
                        matches!(event, matou_native_runtime::SessionEvent::InputSubmitted(_))
                    })
                    .count(),
            );
        }
        let stream_hex = hex(&fs::read(file)?);
        session.shutdown()?;
        Ok(json!({
            "fields": {"writes": writes, "streamHex": stream_hex},
            "details": {"submissionEventsByWrite": submission_events},
        }))
    }

    fn send_text(test: &Value, id: &str) -> Result<Value> {
        let body = test["body"].as_str().context("submitted body")?;
        let raw_probe = json!({"Command": {"Input": {
            "session_id": id, "bytes": body.as_bytes(), "defer_ordering": false,
        }}});
        let ClientMessage::Command(Command::Input { .. }) = serde_json::from_value(raw_probe)?
        else {
            bail!("native raw input command unavailable")
        };
        let request = json!({"Command": {"terminal.send-text": {
            "sessionId": id, "text": body, "submit": test["submit"],
        }}});
        let error = match serde_json::from_value::<ClientMessage>(request) {
            Err(error) => error,
            Ok(_) => bail!("native accepts a send-text request; capture its writes"),
        };
        Ok(json!({
            "fields": {"entryAvailable": false, "writes": [], "streamHex": ""},
            "details": {"rawInputCommandAvailable": true, "dispatchError": error.to_string()},
        }))
    }

    fn observed_frame(frame: Frame) -> Result<Value> {
        match frame {
            Frame::Output { sequence, data } => Ok(json!({
                "kind": "output", "sequence": sequence, "data": packed(&data)
            })),
            frame => Ok(serde_json::to_value(frame)?),
        }
    }

    fn replay_range(root: &Path, id: &str, from: u64, through: Option<u64>) -> Result<Vec<Value>> {
        let mut frames = Vec::new();
        for_each_frame(root, id, from, through, |record| {
            frames.push(observed_frame(record.frame)?);
            Ok(())
        })?;
        Ok(frames)
    }

    fn delivery(test: &Value, root: &Path, id: &str) -> Result<Value> {
        let grid = Grid { cols: 24, rows: 4 };
        let mut session = fixed_delivery::Session::launch(SessionOptions {
            session_id: id.into(),
            launch: launch(root, grid, LaunchProfile::Shell)?,
            terminal: TerminalOptions::new(grid.cols, grid.rows, 9, 18),
            journal: journal_options(),
            restored_history: Vec::new(),
            collect_history: false,
        })?;
        let mut steps = Vec::new();
        let mut credit_bytes = Vec::new();
        for step in test["steps"].as_array().context("delivery steps")? {
            let error = match step["kind"].as_str().context("delivery action")? {
                "output" => {
                    let repeats = step["repeat"].as_u64().unwrap_or(1).try_into()?;
                    let data = step["text"]
                        .as_str()
                        .context("delivery text")?
                        .repeat(repeats);
                    fixed_delivery::output(&mut session, data)?;
                    None
                }
                "resize" => {
                    let grid: Grid = serde_json::from_value(step.clone())?;
                    session.resize(grid.cols, grid.rows, 9, 18)?;
                    None
                }
                "ack" => session
                    .acknowledge(
                        step["sequence"]
                            .as_u64()
                            .context("replay acknowledgement")?,
                    )
                    .err()
                    .map(|error| error.to_string()),
                action => bail!("unknown delivery action {action}"),
            };
            let mut delivered = Vec::new();
            for event in session.poll(Duration::ZERO)? {
                match event {
                    SessionEvent::Output { sequence, data } => {
                        delivered.push(observed_frame(Frame::Output { sequence, data })?)
                    }
                    SessionEvent::Diagnostic { message } => bail!("session diagnostic: {message}"),
                    SessionEvent::StorageFault { message, .. } => {
                        bail!("session storage: {message}")
                    }
                    _ => {}
                }
            }
            credit_bytes.push(session.unacked_bytes());
            steps.push(
                json!({"error": error, "delivered": delivered, "through": session.last_sequence()}),
            );
        }
        let journal = replay_range(root, id, 1, Some(session.last_sequence()))?;
        let maximum = credit_bytes.iter().max().context("empty delivery steps")?;
        session.shutdown()?;
        Ok(json!({
            "fields": {"steps": steps, "journal": journal, "maximumUnackedBytes": maximum},
            "details": {"unackedBytesByStep": credit_bytes},
        }))
    }

    fn seed_journal(timeline: &Value, root: &Path, id: &str) -> Result<u64> {
        let mut journal = SegmentJournal::open(root, id, journal_options())?;
        for frame in timeline["frames"]
            .as_array()
            .context("seed journal frames")?
        {
            let mut frame = frame.clone();
            if let Some(data) = frame
                .as_object_mut()
                .context("seed frame")?
                .remove("dataHex")
            {
                frame["data"] = json!(unhex(data.as_str().context("seed bytes")?)?);
            }
            journal.append(&Record {
                timestamp: 1,
                frame: serde_json::from_value(frame)?,
            })?;
        }
        let last = journal.last_sequence();
        journal.close()?;
        Ok(last)
    }

    fn replay(test: &Value, timeline: &Value, root: &Path, id: &str) -> Result<Value> {
        seed_journal(timeline, root, id)?;
        let ranges: Vec<(u64, Option<u64>)> = serde_json::from_value(test["ranges"].clone())?;
        let frames = ranges
            .into_iter()
            .map(|(from, through)| replay_range(root, id, from, through))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"ranges": frames}))
    }

    fn screen(timeline: &Value, root: &Path, id: &str) -> Result<Value> {
        let last = seed_journal(timeline, root, id)?;
        let grid = serde_json::from_value(timeline["launchGrid"].clone())?;
        let mut session =
            RuntimeSession::launch(runtime_options(root, id, grid, LaunchProfile::ClaudeCode)?)?;
        let screen = session.screen()?;
        let mut rows: Vec<String> = screen
            .cells
            .chunks_exact(screen.cols)
            .map(|row| {
                row.iter()
                    .filter_map(|cell| {
                        if cell.width == 0 {
                            None
                        } else if cell.text.is_empty() {
                            Some(" ")
                        } else {
                            Some(cell.text.as_str())
                        }
                    })
                    .collect::<String>()
                    .trim_end_matches(' ')
                    .to_owned()
            })
            .collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        let viewport = hex(rows.join("\n").as_bytes());
        let launch_grid = Grid::capture(&screen);
        let startup_frames = replay_range(root, id, last + 1, Some(session.last_sequence()))?;
        let after_boundary_sequence = session.last_sequence();
        session.shutdown()?;
        Ok(json!({
            "fields": {
                "restoredViewportHex": viewport, "restoredGrid": launch_grid,
                "startupFrames": startup_frames, "launchGrid": launch_grid,
                "launchViewportHex": viewport,
            },
            "details": {"afterBoundarySequence": after_boundary_sequence, "scrollbackRows": screen.history_lines},
        }))
    }

    #[test]
    #[ignore = "run via the K-pty-flow contract evidence runner"]
    fn captures_fixed_pty_flow_cases() -> Result<()> {
        let input: Value = serde_json::from_slice(&fs::read(env::var("MATOU_FLOW_INPUT")?)?)?;
        let stage = PathBuf::from(env::var_os("MATOU_FLOW_STAGE").context("stage path")?);
        let mut captures = Vec::new();
        for test in input["cases"].as_array().context("flow cases")? {
            let id = test["id"].as_str().context("case ID")?;
            let root = stage.join("native-data").join(id);
            fs::create_dir_all(&root)?;
            let timeline = || -> Result<&Value> {
                Ok(&input["journals"][test["journal"].as_str().context("journal ID")?])
            };
            let mut capture = match test["kind"].as_str().context("case kind")? {
                "decode" => json!({"fields": decode(test)?}),
                "batch" => json!({"fields": batch(test)?}),
                "credit" => json!({"fields": credit(test)?}),
                "submit" => submit(test, &root, id)?,
                "sendText" => send_text(test, id)?,
                "delivery" => delivery(test, &root, id)?,
                "replay" => json!({"fields": replay(test, timeline()?, &root, id)?}),
                "screen" => screen(timeline()?, &root, id)?,
                kind => bail!("unknown flow case {kind}"),
            };
            capture["id"] = json!(id);
            captures.push(capture);
        }
        let mut bytes = serde_json::to_vec_pretty(&captures)?;
        bytes.push(b'\n');
        fs::write(env::var("MATOU_FLOW_OUTPUT")?, bytes)?;
        println!("Native observations: {} cases", captures.len());
        Ok(())
    }
}
