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
use matou_native_runtime::{flow, history};
#[cfg(target_os = "macos")]
use probe_model::{SessionEvent, SessionOptions, StorageFaultCode};
#[cfg(target_os = "macos")]
#[path = "../src/checkpoint_sync.rs"]
#[allow(dead_code)]
mod checkpoint_sync;
#[cfg(target_os = "macos")]
#[path = "../src/model.rs"]
#[allow(dead_code)]
mod probe_model;
#[cfg(target_os = "macos")]
#[path = "../src/search.rs"]
#[allow(dead_code)]
mod search;
#[cfg(target_os = "macos")]
#[path = "../src/transcript_backfill.rs"]
#[allow(dead_code)]
mod transcript_backfill;

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
        journal::{
            Frame, JournalOptions, Record, SegmentJournal, checkpoint::CheckpointCandidate,
            for_each_frame,
        },
        pty::{LaunchProfile, ShellLaunch},
    };
    use matou_native_runtime::{
        ScreenRecoveryOptions, host::Command, protocol::ClientMessage,
        session::Session as RuntimeSession,
    };
    use matou_native_terminal::{Screen, TerminalOptions};
    use serde::Deserialize;
    use serde_json::json;
    use sha2::{Digest, Sha256};
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
        Ok(pty_launch(root, grid, profile, executable))
    }

    fn pty_launch(
        root: &Path,
        grid: Grid,
        profile: LaunchProfile,
        executable: PathBuf,
    ) -> ShellLaunch {
        let child_env = [
            ("HOME".into(), root.as_os_str().to_owned()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("SHELL".into(), executable.as_os_str().to_owned()),
            ("LANG".into(), "en_US.UTF-8".into()),
        ];
        ShellLaunch {
            profile,
            executable,
            argv: Vec::new(),
            cwd: root.to_owned(),
            data_root: root.to_owned(),
            inherited_env: child_env.clone().into(),
            env: child_env.into(),
            cols: grid.cols,
            rows: grid.rows,
        }
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
        Ok(options_for_launch(id, launch(root, grid, profile)?))
    }

    fn options_for_launch(id: &str, launch: ShellLaunch) -> matou_native_runtime::SessionOptions {
        matou_native_runtime::SessionOptions {
            session_id: id.into(),
            terminal: TerminalOptions::new(launch.cols, launch.rows, 9, 18),
            launch,
            journal: journal_options(),
            restored_history: Vec::new(),
            screen_recovery: None,
            collect_history: false,
        }
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
            screen_recovery: None,
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

    fn viewport(screen: &Screen) -> String {
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
        rows.join("\n")
    }

    fn seed_checkpoint(
        root: &Path,
        session_id: &str,
        generation: u64,
        domain_event_sequence: u64,
        mut header: Value,
    ) -> Result<CheckpointCandidate> {
        let snapshot = header
            .as_object_mut()
            .context("checkpoint fixture")?
            .remove("snapshotHex")
            .context("checkpoint snapshot")?;
        let snapshot = unhex(snapshot.as_str().context("checkpoint snapshot bytes")?)?;
        let id = format!("checkpoint-{generation}");
        header["id"] = json!(id);
        header["sessionId"] = json!(session_id);
        header["generation"] = json!(generation);
        header["domainEventSequence"] = json!(domain_event_sequence);
        header["snapshotLength"] = json!(snapshot.len());
        let header_bytes = serde_json::to_vec(&header)?;
        let mut encoded = b"MTCPV4\n".to_vec();
        encoded.extend_from_slice(&u32::try_from(header_bytes.len())?.to_be_bytes());
        encoded.extend(header_bytes);
        encoded.extend(snapshot);
        let file_path = root.join(&id);
        fs::write(&file_path, &encoded)?;
        Ok(CheckpointCandidate {
            id,
            session_id: session_id.into(),
            generation,
            terminal_sequence: header["terminalSequence"]
                .as_u64()
                .context("checkpoint sequence")?,
            domain_event_sequence,
            screen_epoch: header["screenEpoch"].as_u64().context("checkpoint epoch")?,
            file_path,
            checksum: format!("{:x}", Sha256::digest(&encoded)),
        })
    }

    fn screen(test: &Value, timeline: &Value, root: &Path, id: &str) -> Result<Value> {
        let last = seed_journal(timeline, root, id)?;
        let grid: Grid = serde_json::from_value(timeline["launchGrid"].clone())?;
        let initial: Grid = serde_json::from_value(timeline["initialGrid"].clone())?;
        let mut checkpoints = Vec::new();
        if let Some(checkpoint) = test.get("checkpoint") {
            let sequence = checkpoint["terminalSequence"]
                .as_u64()
                .context("checkpoint sequence")?;
            let mut domain = 0;
            for_each_frame(root, id, 1, Some(sequence), |record| {
                if let Some(cursor) = record.frame.domain_cursor() {
                    domain = cursor;
                }
                Ok(())
            })?;
            checkpoints.push(seed_checkpoint(root, id, 1, domain, checkpoint.clone())?);
        }
        let mut options = runtime_options(root, id, grid, LaunchProfile::ClaudeCode)?;
        options.screen_recovery = Some(ScreenRecoveryOptions {
            initial_grid: Some((initial.cols, initial.rows)),
            checkpoints,
            transcript_path: None,
        });
        let mut session = RuntimeSession::launch(options)?;
        let launch_screen = session.screen()?;
        // The run boundary moved the pre-launch viewport up by the launch height.
        session.scroll(i32::from(grid.rows))?;
        let restored_screen = session.screen()?;
        let startup_frames = replay_range(root, id, last + 1, Some(session.last_sequence()))?;
        let after_boundary_sequence = session.last_sequence();
        session.shutdown()?;
        Ok(json!({
            "fields": {
                "restoredViewportHex": hex(viewport(&restored_screen).as_bytes()),
                "restoredGrid": Grid::capture(&restored_screen),
                "startupFrames": startup_frames, "launchGrid": Grid::capture(&launch_screen),
                "launchViewportHex": hex(viewport(&launch_screen).as_bytes()),
            },
            "details": {"afterBoundarySequence": after_boundary_sequence, "scrollbackRows": launch_screen.history_lines},
        }))
    }

    const RESTART_ID: &str = "restart";
    const RESTART_GRID: Grid = Grid { cols: 24, rows: 5 };

    fn restart_options(
        root: &Path,
        profile: LaunchProfile,
        command: &str,
    ) -> matou_native_runtime::SessionOptions {
        let mut launch = pty_launch(root, RESTART_GRID, profile, "/bin/sh".into());
        launch.argv = vec!["-c".into(), command.into()];
        options_for_launch(RESTART_ID, launch)
    }

    fn scroll_to_previous_viewport(session: &mut RuntimeSession) -> Result<String> {
        session.scroll(i32::from(RESTART_GRID.rows))?;
        Ok(viewport(&session.screen()?))
    }

    fn live_output(
        session: &mut RuntimeSession,
        started_at: u64,
    ) -> Result<Vec<matou_native_runtime::SessionEvent>> {
        use matou_native_runtime::SessionEvent;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while !events
            .iter()
            .any(|event| matches!(event, SessionEvent::SavedExit { .. }))
        {
            anyhow::ensure!(
                Instant::now() < deadline,
                "timed out waiting for PTY output and exit"
            );
            events.extend(session.poll(Duration::from_millis(20))?);
        }
        assert!(
            matches!(events.first(), Some(SessionEvent::Started { sequence, .. }) if *sequence == started_at)
        );
        Ok(events)
    }

    #[test]
    fn provider_exit_writes_a_checkpoint_that_restores_before_new_journal_frames() -> Result<()> {
        use matou_native_core::{
            hierarchy::{BootstrapWindowInput, HierarchyIds, bootstrap_window},
            journal::checkpoint::protected_terminal_sequences,
            storage::{CommandMetadata, configure_connection, migrate},
        };
        use rusqlite::Connection;

        let root = tempfile::tempdir()?;
        let root = root.path();
        let mut db = Connection::open(root.join("matou.sqlite"))?;
        configure_connection(&db)?;
        migrate(&mut db, 1)?;
        bootstrap_window(
            &mut db,
            &CommandMetadata {
                command_id: "bootstrap".into(),
                command_type: "bootstrap".into(),
                request_hash: "bootstrap".into(),
                causation_id: None,
                correlation_id: None,
            },
            &BootstrapWindowInput {
                window_id: "main".into(),
                default_root_directory: root.display().to_string(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &HierarchyIds {
                workspace_id: "workspace".into(),
                execution_context_id: "context".into(),
                task_id: "task".into(),
                scene_id: "scene".into(),
                root_node_id: "root".into(),
                session_id: RESTART_ID.into(),
                mount_id: "mount".into(),
            },
            "Task",
            1,
        )?;
        seed_journal(
            &json!({"frames": [
                {"kind": "reset", "sequence": 1, "screenEpoch": 3},
                {"kind": "domain-cursor", "sequence": 2, "domainEventSequence": 8},
            ]}),
            root,
            RESTART_ID,
        )?;
        let mut options = restart_options(root, LaunchProfile::Codex, "printf 'CAPTURED\\n'");
        options.screen_recovery = Some(ScreenRecoveryOptions::default());
        let mut session = RuntimeSession::launch(options)?;
        let events = live_output(&mut session, 2)?;
        assert!(
            !events.iter().any(|event| matches!(
                event,
                matou_native_runtime::SessionEvent::Diagnostic { .. }
            )),
            "{events:?}"
        );
        let through = session.last_sequence();
        assert_eq!(
            protected_terminal_sequences(&db, RESTART_ID)?,
            vec![through]
        );
        let candidate = db.query_row(
            "SELECT id, generation, terminal_sequence, domain_event_sequence, screen_epoch,
                    file_path, checksum FROM journal_checkpoints WHERE session_id = ? AND valid = 1",
            [RESTART_ID],
            |row| Ok(CheckpointCandidate {
                id: row.get(0)?,
                session_id: RESTART_ID.into(),
                generation: row.get::<_, i64>(1)? as u64,
                terminal_sequence: row.get::<_, i64>(2)? as u64,
                domain_event_sequence: row.get::<_, i64>(3)? as u64,
                screen_epoch: row.get::<_, i64>(4)? as u64,
                file_path: row.get::<_, String>(5)?.into(),
                checksum: row.get(6)?,
            })
        )?;
        assert_eq!(
            (
                candidate.terminal_sequence,
                candidate.domain_event_sequence,
                candidate.screen_epoch
            ),
            (through, 8, 3)
        );
        assert_eq!(
            candidate.read()?.grid,
            Some((RESTART_GRID.cols, RESTART_GRID.rows))
        );
        drop(session);

        // Take away the pre-checkpoint journal so a full replay cannot produce
        // the first line. Retain a domain watermark and output after the snapshot.
        fs::rename(
            root.join("journal").join(RESTART_ID),
            root.join("journal-prefix"),
        )?;
        let mut journal = SegmentJournal::open(root, RESTART_ID, journal_options())?;
        journal.append(&Record {
            timestamp: 1,
            frame: Frame::DomainCursor {
                sequence: through + 1,
                domain_event_sequence: 8,
            },
        })?;
        journal.append(&Record {
            timestamp: 2,
            frame: Frame::Output {
                sequence: through + 2,
                data: b"AFTER".to_vec(),
            },
        })?;
        journal.close()?;
        let mut options = restart_options(root, LaunchProfile::Codex, "exec /bin/sleep 30");
        options.journal.checkpoint_protected_sequences = vec![through];
        options.screen_recovery = Some(ScreenRecoveryOptions {
            initial_grid: Some((RESTART_GRID.cols, RESTART_GRID.rows)),
            checkpoints: vec![candidate],
            transcript_path: None,
        });
        let mut session = RuntimeSession::launch(options)?;
        assert_eq!(
            scroll_to_previous_viewport(&mut session)?,
            "CAPTURED\nAFTER"
        );
        session.shutdown()?;
        assert_eq!(
            protected_terminal_sequences(&db, RESTART_ID)?,
            vec![through]
        );
        Ok(())
    }

    #[test]
    fn recovers_reset_and_resized_viewport_before_the_first_live_pty_output() -> Result<()> {
        use matou_native_runtime::SessionEvent;
        let root = tempfile::tempdir()?;
        let root = root.path();
        let discarded = hex("DISCARDED\r\n".repeat(8).as_bytes());
        let old = hex(b"OLD-A\r\nOLD-B\x1b[6n\x1b]52;c;T0xE\x07");
        seed_journal(
            &json!({"frames": [
                {"kind": "output", "sequence": 1, "dataHex": discarded},
                {"kind": "resize", "sequence": 2, "cols": 18, "rows": 4},
                {"kind": "reset", "sequence": 3, "screenEpoch": 1},
                {"kind": "output", "sequence": 4, "dataHex": old},
            ]}),
            root,
            RESTART_ID,
        )?;

        let options = restart_options(root, LaunchProfile::ClaudeCode, "printf LIVE");
        let mut session = RuntimeSession::launch(options)?;
        let initial = session.screen()?;
        assert_eq!(viewport(&initial), "");
        assert_eq!(initial.history_lines, 5);
        assert_eq!(scroll_to_previous_viewport(&mut session)?, "OLD-A\nOLD-B");
        let events = live_output(&mut session, 6)?;
        assert!(!events.iter().any(|event| matches!(
            event,
            SessionEvent::Diagnostic { .. } | SessionEvent::ClipboardWrite { .. }
        )));
        let saved: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::SavedOutput { sequence, data, .. } => {
                    Some((*sequence, data.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(saved, [(7, "LIVE")]);
        let frames = replay_range(root, RESTART_ID, 5, Some(7))?;
        assert_eq!(
            frames[0],
            json!({"kind": "resize", "sequence": 5, "cols": 24, "rows": 5})
        );
        assert_eq!(frames[1]["sequence"], 6);
        assert!(
            frames[1]["data"]["hex"]
                .as_str()
                .unwrap()
                .starts_with("181b5b3f313034396c")
        );
        assert_eq!(
            frames[2],
            json!({"kind": "output", "sequence": 7, "data": {"hex": "4c495645"}})
        );
        session.shutdown()
    }

    #[test]
    fn selects_an_older_checkpoint_or_replays_everything_if_all_candidates_fail() -> Result<()> {
        for use_older in [true, false] {
            let root = tempfile::tempdir()?;
            let root = root.path();
            seed_journal(
                &json!({"frames": [
                    {"kind": "output", "sequence": 1, "dataHex": hex(b"JOURNAL\r\n")},
                    {"kind": "domain-cursor", "sequence": 2, "domainEventSequence": 10},
                    {"kind": "output", "sequence": 3, "dataHex": hex(b"SUFFIX")},
                ]}),
                root,
                RESTART_ID,
            )?;
            let mut options = restart_options(root, LaunchProfile::Codex, "exec /bin/sleep 30");
            let make = |generation, sequence, domain, snapshot: &[u8]| {
                seed_checkpoint(
                    root,
                    RESTART_ID,
                    generation,
                    domain,
                    json!({
                        "terminalSequence": sequence, "screenEpoch": 0, "snapshotHex": hex(snapshot),
                        "cols": 24, "rows": 5,
                    }),
                )
            };
            let mut bad_checksum = make(3, 2, 10, b"BAD")?;
            bad_checksum.checksum = "wrong".into();
            let missing = make(2, 2, 10, b"MISSING")?;
            fs::remove_file(&missing.file_path)?;
            let mut checkpoints = vec![
                make(5, 2, 11, b"DOMAIN AHEAD")?,
                make(4, 4, 10, b"TERMINAL AHEAD")?,
                bad_checksum,
                missing,
            ];
            if use_older {
                checkpoints.push(make(1, 2, 10, b"CHECKPOINT\r\n")?);
            }
            options.screen_recovery = Some(ScreenRecoveryOptions {
                initial_grid: Some((24, 5)),
                checkpoints,
                transcript_path: None,
            });
            let mut session = RuntimeSession::launch(options)?;
            assert_eq!(session.last_sequence(), 4, "only the boundary was appended");
            assert_eq!(viewport(&session.screen()?), "");
            assert_eq!(
                scroll_to_previous_viewport(&mut session)?,
                if use_older {
                    "CHECKPOINT\nSUFFIX"
                } else {
                    "JOURNAL\nSUFFIX"
                },
            );
            session.shutdown()?;
        }
        Ok(())
    }

    #[test]
    fn newest_readable_checkpoint_without_dimensions_replays_the_full_journal() -> Result<()> {
        let root = tempfile::tempdir()?;
        let root = root.path();
        seed_journal(
            &json!({"frames": [
                {"kind": "output", "sequence": 1, "dataHex": hex(b"JOURNAL\r\n")},
                {"kind": "output", "sequence": 2, "dataHex": hex(b"SUFFIX")},
            ]}),
            root,
            RESTART_ID,
        )?;
        let without_grid = seed_checkpoint(
            root,
            RESTART_ID,
            2,
            0,
            json!({"terminalSequence": 1, "screenEpoch": 0, "snapshotHex": hex(b"NO GRID\r\n")}),
        )?;
        assert_eq!(without_grid.read()?.grid, None);
        let older = seed_checkpoint(
            root,
            RESTART_ID,
            1,
            0,
            json!({
                "terminalSequence": 1, "screenEpoch": 0, "snapshotHex": hex(b"OLDER\r\n"),
                "cols": 24, "rows": 5,
            }),
        )?;
        let mut options = restart_options(root, LaunchProfile::Codex, "exec /bin/sleep 30");
        options.screen_recovery = Some(ScreenRecoveryOptions {
            initial_grid: Some((20, 5)),
            checkpoints: vec![without_grid, older],
            transcript_path: None,
        });

        let mut session = RuntimeSession::launch(options)?;
        assert_eq!(
            scroll_to_previous_viewport(&mut session)?,
            "JOURNAL\nSUFFIX"
        );
        assert_eq!(
            replay_range(root, RESTART_ID, 3, Some(session.last_sequence()))?[0],
            json!({"kind": "resize", "sequence": 3, "cols": 24, "rows": 5}),
        );
        session.shutdown()
    }

    #[test]
    fn damaged_unindexed_segment_keeps_its_valid_prefix_and_allows_launch() -> Result<()> {
        use matou_native_runtime::SessionEvent;
        let root = tempfile::tempdir()?;
        let root = root.path();
        let mut options = restart_options(root, LaunchProfile::Antigravity, "exec /bin/sleep 30");
        options.journal.max_segment_bytes = 210;
        let mut journal = SegmentJournal::open(root, RESTART_ID, options.journal.clone())?;
        for (index, data) in [
            b"VALID PREFIX".to_vec(),
            b"DAMAGED".to_vec(),
            vec![b'X'; 100],
        ]
        .into_iter()
        .enumerate()
        {
            journal.append(&Record {
                timestamp: 1,
                frame: Frame::Output {
                    sequence: index as u64 + 1,
                    data,
                },
            })?;
        }
        let directory = journal.directory().to_owned();
        journal.close()?;
        let sealed = directory.join("segment-000001.mtj");
        let mut bytes = fs::read(&sealed)?;
        assert!(bytes.ends_with(b"DAMAGED"));
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&sealed, bytes)?;
        fs::remove_file(directory.join("range-index.json"))?;

        let mut session = RuntimeSession::launch(options)?;
        assert_eq!(session.last_sequence(), 4);
        assert_eq!(scroll_to_previous_viewport(&mut session)?, "VALID PREFIX");
        let events = session.poll(Duration::ZERO)?;
        assert!(matches!(
            events.first(),
            Some(SessionEvent::Started { sequence: 4, .. })
        ));
        assert!(events.iter().any(|event| matches!(event, SessionEvent::Diagnostic { message } if message.contains("checksum"))));
        session.shutdown()
    }

    #[test]
    fn successive_non_shell_restarts_keep_both_runs_in_scrollback() -> Result<()> {
        let root = tempfile::tempdir()?;
        let root = root.path();
        seed_journal(
            &json!({"frames": [
                {"kind": "output", "sequence": 1, "dataHex": hex(b"FIRST")},
            ]}),
            root,
            RESTART_ID,
        )?;
        let mut session =
            RuntimeSession::launch(restart_options(root, LaunchProfile::Dsh, "printf SECOND"))?;
        assert_eq!(scroll_to_previous_viewport(&mut session)?, "FIRST");
        live_output(&mut session, 2)?;
        session.shutdown()?;

        let mut session = RuntimeSession::launch(restart_options(
            root,
            LaunchProfile::Dsh,
            "exec /bin/sleep 30",
        ))?;
        assert_eq!(session.last_sequence(), 5);
        assert_eq!(viewport(&session.screen()?), "");
        assert_eq!(scroll_to_previous_viewport(&mut session)?, "SECOND");
        assert_eq!(scroll_to_previous_viewport(&mut session)?, "FIRST");
        session.shutdown()
    }

    #[test]
    fn shell_restarts_continue_to_seed_d01_history_without_startup_frames() -> Result<()> {
        let root = tempfile::tempdir()?;
        let root = root.path();
        let mut last = seed_journal(
            &json!({"frames": [
                {"kind": "output", "sequence": 1, "dataHex": hex(b"UNRELATED JOURNAL")},
            ]}),
            root,
            RESTART_ID,
        )?;
        for _ in 0..2 {
            let mut options = restart_options(root, LaunchProfile::Shell, "exec /bin/sleep 30");
            options.restored_history = b"D01 HISTORY".to_vec();
            let mut session = RuntimeSession::launch(options)?;
            assert_eq!(session.last_sequence(), last);
            assert_eq!(viewport(&session.screen()?), "D01 HISTORY");
            session.shutdown()?;
            last = session.last_sequence();
        }
        Ok(())
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
                "screen" => screen(test, timeline()?, &root, id)?,
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
