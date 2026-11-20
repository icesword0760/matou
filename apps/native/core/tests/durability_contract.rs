use std::{cell::RefCell, collections::BTreeMap};

use anyhow::{Result, bail};
use matou_native_core::{
    durability::{DEFAULT_MAX_RETAINED_BYTES, Event, ExecutionPauser, Gate, PendingFrame, State},
    journal::{Frame, Record},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    input: Input,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    name: String,
    max: Option<usize>,
    #[serde(default)]
    failures: BTreeMap<u64, usize>,
    side_failure: Option<u64>,
    #[serde(default)]
    resume_failures: usize,
    actions: Vec<Action>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum Action {
    Append { sequence: u64, bytes: usize },
    Burst { frames: Vec<FrameInput> },
    Retry,
    End,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
struct FrameInput {
    sequence: u64,
    bytes: usize,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    state: State,
    retained_bytes: usize,
    last_accepted_sequence: u64,
    last_persisted_sequence: u64,
    trace: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
struct Step {
    action: Action,
    #[serde(skip_serializing_if = "Option::is_none")]
    admitted: Option<Snapshot>,
    #[serde(flatten)]
    snapshot: Snapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct Pauser<'a> {
    trace: &'a RefCell<Vec<String>>,
    resume_failures: usize,
}

impl ExecutionPauser for Pauser<'_> {
    fn pause(&mut self) -> Result<()> {
        self.trace.borrow_mut().push("pause".into());
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        self.trace.borrow_mut().push("resume".into());
        if self.resume_failures > 0 {
            self.resume_failures -= 1;
            bail!("resume-fault");
        }
        Ok(())
    }
}

fn output(sequence: u64, bytes: usize) -> PendingFrame {
    PendingFrame {
        record: Record {
            timestamp: 0,
            frame: Frame::Output {
                sequence,
                data: vec![0; bytes],
            },
        },
        retained_bytes: bytes,
    }
}

fn snapshot(gate: &Gate, trace: &RefCell<Vec<String>>) -> Snapshot {
    Snapshot {
        state: gate.state(),
        retained_bytes: gate.retained_bytes(),
        last_accepted_sequence: gate.last_accepted_sequence(),
        last_persisted_sequence: gate.last_persisted_sequence(),
        trace: trace.borrow().clone(),
    }
}

#[test]
fn matches_captured_admission_and_persistence_steps() -> Result<()> {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/durability-reference.json"))?;
    assert_eq!(fixture.cases.len(), 6);
    for Case { input, steps } in fixture.cases {
        let name = input.name.clone();
        let actual = replay_gate(input)?;
        assert_eq!(actual.len(), steps.len());
        for (index, (actual, expected)) in actual.into_iter().zip(steps).enumerate() {
            assert_eq!(actual, expected, "{name} step {}", index + 1);
        }
    }
    Ok(())
}

fn replay_gate(input: Input) -> Result<Vec<Step>> {
    let mut gate = Gate::new(0, input.max.unwrap_or(DEFAULT_MAX_RETAINED_BYTES))?;
    let trace = RefCell::new(Vec::new());
    let mut pauser = Pauser {
        trace: &trace,
        resume_failures: input.resume_failures,
    };

    let mut failures = input.failures;
    let mut persist = |record: &Record| -> Result<()> {
        let sequence = record.frame.sequence();
        trace.borrow_mut().push(format!("attempt:{sequence}"));
        if let Some(remaining) = failures.get_mut(&sequence)
            && *remaining > 0
        {
            *remaining -= 1;
            bail!("storage-fault");
        }
        trace.borrow_mut().push(format!("persisted:{sequence}"));
        Ok(())
    };
    let mut after = |record: &Record| -> Result<()> {
        let sequence = record.frame.sequence();
        trace.borrow_mut().push(format!("publish:{sequence}"));
        if input.side_failure == Some(sequence) {
            bail!("side-fault");
        }
        Ok(())
    };
    let mut emit = |event| {
        let text = match event {
            Event::Fault {
                failed_sequence,
                retained_bytes,
                error,
            } => {
                assert_eq!(error, "storage-fault");
                format!("fault:{failed_sequence}:{retained_bytes}")
            }
            Event::Recovered { through_sequence } => format!("recovered:{through_sequence}"),
            Event::SideEffectError { sequence, error } => {
                assert_eq!(error, "side-fault");
                format!("side-error:{sequence}")
            }
        };
        trace.borrow_mut().push(text);
    };

    let mut steps = Vec::new();
    for action in input.actions {
        let mut admitted = None;
        let result = match &action {
            Action::Append { sequence, bytes } => gate
                .append(output(*sequence, *bytes), &mut pauser)
                .and_then(|()| gate.drain(&mut pauser, &mut persist, &mut after, &mut emit)),
            Action::Burst { frames } => (|| {
                for FrameInput { sequence, bytes } in frames {
                    gate.append(output(*sequence, *bytes), &mut pauser)?;
                }
                admitted = Some(snapshot(&gate, &trace));
                gate.drain(&mut pauser, &mut persist, &mut after, &mut emit)
            })(),
            Action::Retry => gate.retry(&mut pauser, &mut persist, &mut after, &mut emit),
            Action::End => gate.end(&mut pauser),
        };
        let actual = Step {
            action,
            admitted,
            snapshot: snapshot(&gate, &trace),
            error: result.err().map(|error| error.to_string()),
        };
        steps.push(actual);
    }
    Ok(steps)
}

#[test]
fn publishes_side_effect_error_before_returning_a_healthy_resume_error() -> Result<()> {
    let mut gate = Gate::new(10, 8)?;
    let trace = RefCell::new(Vec::new());
    let mut pauser = Pauser {
        trace: &trace,
        resume_failures: 1,
    };
    gate.append(output(11, 4), &mut pauser)?;
    let mut effects = Vec::new();
    let error = gate
        .drain(
            &mut pauser,
            &mut |_| Ok(()),
            &mut |_| bail!("side-fault"),
            &mut |event| effects.push(event),
        )
        .expect_err("resume failure");
    assert_eq!(error.to_string(), "resume-fault");
    assert_eq!(
        effects,
        [Event::SideEffectError {
            sequence: 11,
            error: "side-fault".into()
        }]
    );
    assert_eq!(gate.state(), State::Healthy);
    assert_eq!(gate.retained_bytes(), 0);
    assert_eq!(gate.last_persisted_sequence(), 11);
    assert_eq!(trace.into_inner(), ["pause", "resume"]);
    Ok(())
}

#[cfg(target_os = "macos")]
mod gate_capture {
    use std::{
        env, fs, io,
        path::{Path, PathBuf},
    };

    use anyhow::Context;
    use matou_native_core::journal::{
        JournalOptions, SegmentJournal, TailIndexSnapshot, for_each_frame, wait_for_compression,
    };
    use serde_json::{Map, Value, json};

    use super::*;

    #[derive(Deserialize)]
    struct JournalFixture {
        timestamp: i64,
        cases: Vec<JournalInput>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct JournalInput {
        name: String,
        max_segment_bytes: Option<u64>,
        raw_hot_bytes: Option<u64>,
        steps: Vec<Value>,
    }

    #[derive(Deserialize)]
    #[serde(tag = "op", rename_all = "kebab-case")]
    enum JournalAction {
        QueueLimit {
            max: usize,
        },
        Open {
            #[serde(default)]
            compress: bool,
        },
        Append {
            sequence: u64,
            bytes: usize,
        },
        Burst {
            frames: Vec<FrameInput>,
        },
        Close,
        Read {
            from: u64,
            through: Option<u64>,
        },
        Remember {
            slot: String,
        },
        RestoreIndex {
            slot: String,
        },
        Truncate {
            slot: String,
        },
        Arm,
        Disarm,
    }

    const RAW: &str = "segment-000001.mtj";
    const TAIL: &str = "tail-index.json";
    const RANGE: &str = "range-index.json";

    struct Saved {
        tail: Vec<u8>,
        range: Vec<u8>,
        offset: u64,
    }

    fn optional_file(path: PathBuf) -> Result<Option<Vec<u8>>> {
        match fs::read(path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn tail_watermark(tail: &TailIndexSnapshot) -> Value {
        json!({
            "firstSequence": tail.first_sequence,
            "lastSequence": tail.last_sequence,
            "activeSegmentIndex": tail.active_segment_index,
        })
    }

    fn disk_snapshot(directory: &Path, journal: Option<&SegmentJournal>) -> Result<Value> {
        let mut files = BTreeMap::new();
        let mut raw_retained_bytes = 0;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("segment-") {
                continue;
            }
            let bytes = entry.metadata()?.len();
            if name.ends_with(".mtj") {
                raw_retained_bytes += bytes;
            }
            files.insert(name, bytes);
        }
        let watermark = journal.map(|journal| {
            let tail = journal.tail_snapshot();
            let segment = format!("segment-{:06}.mtj", tail.active_segment_index);
            json!({
                "sequence": journal.last_sequence(),
                "offset": files[&segment],
                "tail": tail_watermark(&tail),
            })
        });
        let saved_tail: Option<TailIndexSnapshot> = optional_file(directory.join(TAIL))?
            .map(|content| serde_json::from_slice(&content))
            .transpose()?;
        let saved_range: Option<Value> = optional_file(directory.join(RANGE))?
            .map(|content| serde_json::from_slice(&content))
            .transpose()?;
        let trace =
            String::from_utf8(optional_file(directory.join(".trace"))?.unwrap_or_default())?;
        Ok(json!({
            "watermark": watermark,
            "disk": {
                "files": files,
                "rawRetainedBytes": raw_retained_bytes,
                "tail": saved_tail.as_ref().map(tail_watermark),
                "range": saved_range,
            },
            "trace": trace.lines().collect::<Vec<_>>(),
        }))
    }

    fn sample_record(sequence: u64, bytes: usize, timestamp: i64) -> Record {
        Record {
            timestamp,
            frame: Frame::Output {
                sequence,
                data: vec![sequence as u8; bytes],
            },
        }
    }

    fn replay_journal(input: JournalInput, root: &Path, timestamp: i64) -> Result<Vec<Value>> {
        let directory = root.join("journal").join(&input.name);
        let mut saved = BTreeMap::<String, Saved>::new();
        let mut journal = None;
        let burst_trace = RefCell::new(Vec::new());
        let mut steps = Vec::new();
        for raw_action in input.steps {
            let action = serde_json::from_value::<JournalAction>(raw_action.clone())?;
            if let JournalAction::QueueLimit { max } = action {
                let error = Gate::new(0, max)
                    .err()
                    .context("queue limit was accepted")?;
                steps.push(json!({ "action": raw_action, "error": error.to_string() }));
                continue;
            }

            let mut readback = None;
            let mut admitted = None;
            let outcome = (|| -> Result<()> {
                match action {
                    JournalAction::Open { compress } => {
                        let mut options = JournalOptions {
                            compress_sealed: compress,
                            ..JournalOptions::default()
                        };
                        if let Some(max) = input.max_segment_bytes {
                            options.max_segment_bytes = max;
                        }
                        if let Some(cap) = input.raw_hot_bytes {
                            options.raw_hot_bytes = cap;
                        }
                        journal = Some(SegmentJournal::open(root, &input.name, options)?);
                        wait_for_compression();
                    }
                    JournalAction::Append { sequence, bytes } => {
                        journal
                            .as_mut()
                            .context("missing journal")?
                            .append(&sample_record(sequence, bytes, timestamp))?;
                    }
                    JournalAction::Burst { frames } => {
                        let active = journal.as_mut().context("missing journal")?;
                        let mut gate =
                            Gate::new(active.last_sequence(), DEFAULT_MAX_RETAINED_BYTES)?;
                        let pauser_trace = RefCell::new(Vec::new());
                        let mut pauser = Pauser {
                            trace: &pauser_trace,
                            resume_failures: 0,
                        };
                        for FrameInput { sequence, bytes } in frames {
                            gate.append(
                                PendingFrame {
                                    record: sample_record(sequence, bytes, timestamp),
                                    retained_bytes: bytes,
                                },
                                &mut pauser,
                            )?;
                        }
                        admitted = Some(json!({
                            "lastAcceptedSequence": gate.last_accepted_sequence(),
                            "lastPersistedSequence": gate.last_persisted_sequence(),
                            "retainedBytes": gate.retained_bytes(),
                        }));
                        gate.drain(
                            &mut pauser,
                            &mut |record| {
                                let sequence = record.frame.sequence();
                                burst_trace.borrow_mut().push(format!("attempt:{sequence}"));
                                active.append(record)?;
                                burst_trace
                                    .borrow_mut()
                                    .push(format!("persisted:{sequence}"));
                                Ok(())
                            },
                            &mut |record| {
                                let sequence = record.frame.sequence();
                                burst_trace.borrow_mut().push(format!("publish:{sequence}"));
                                Ok(())
                            },
                            &mut |event| burst_trace.borrow_mut().push(format!("{event:?}")),
                        )?;
                    }
                    JournalAction::Close => {
                        journal.take().context("missing journal")?.close()?;
                    }
                    JournalAction::Read { from, through } => {
                        let frames = readback.insert(Vec::new());
                        for_each_frame(root, &input.name, from, through, |record| {
                            let Frame::Output { sequence, data } = record.frame else {
                                bail!("unexpected journal frame kind");
                            };
                            let data_hex: String =
                                data.iter().map(|byte| format!("{byte:02x}")).collect();
                            frames.push(json!({ "sequence": sequence, "dataHex": data_hex }));
                            Ok(())
                        })?;
                    }
                    JournalAction::Remember { slot } => {
                        saved.insert(
                            slot,
                            Saved {
                                tail: fs::read(directory.join(TAIL))?,
                                range: fs::read(directory.join(RANGE))?,
                                offset: fs::metadata(directory.join(RAW))?.len(),
                            },
                        );
                    }
                    JournalAction::RestoreIndex { slot } => {
                        let index = saved.get(&slot).context("unknown saved index")?;
                        fs::write(directory.join(TAIL), &index.tail)?;
                        fs::write(directory.join(RANGE), &index.range)?;
                    }
                    JournalAction::Truncate { slot } => {
                        let previous = saved.get(&slot).context("unknown saved offset")?;
                        fs::OpenOptions::new()
                            .write(true)
                            .open(directory.join(RAW))?
                            .set_len(previous.offset)?;
                    }
                    JournalAction::Arm => fs::write(directory.join(".inject"), "")?,
                    JournalAction::Disarm => fs::remove_file(directory.join(".inject"))?,
                    JournalAction::QueueLimit { .. } => unreachable!(),
                }
                Ok(())
            })();

            let mut step = disk_snapshot(&directory, journal.as_ref())?;
            step["action"] = raw_action;
            if !burst_trace.borrow().is_empty() {
                step["trace"] = json!(*burst_trace.borrow());
            }
            if let Some(admitted) = admitted {
                step["admitted"] = admitted;
            }
            if let Some(readback) = readback {
                step["readback"] = json!(readback);
            }
            if let Err(error) = outcome {
                let message = error
                    .to_string()
                    .replace(&root.to_string_lossy().to_string(), "<journal-root>");
                step["error"] = json!({ "message": message });
                if let Some(errno) = error
                    .downcast_ref::<io::Error>()
                    .and_then(io::Error::raw_os_error)
                {
                    step["error"]["code"] = if errno == libc::EIO {
                        json!("EIO")
                    } else {
                        json!(errno)
                    };
                }
            }
            steps.push(step);
        }
        Ok(steps)
    }

    #[test]
    #[ignore = "run via the durability gate with the instrumented journal I/O"]
    fn capture_native_observations() -> Result<()> {
        let stage = PathBuf::from(env::var("MATOU_DURABILITY_STAGE")?);
        let inputs: Vec<Input> =
            serde_json::from_slice(&fs::read(stage.join("queue-inputs.json"))?)?;
        let queue = inputs
            .into_iter()
            .map(|input| {
                let name = input.name.clone();
                Ok((name, serde_json::to_value(replay_gate(input)?)?))
            })
            .collect::<Result<Map<String, Value>>>()?;

        let fixture: JournalFixture =
            serde_json::from_slice(&fs::read(env::var("MATOU_DURABILITY_JOURNAL_INPUT")?)?)?;
        let root = stage.join("native-root");
        let journal = fixture
            .cases
            .into_iter()
            .map(|input| {
                let name = input.name.clone();
                let steps = replay_journal(input, &root, fixture.timestamp)
                    .with_context(|| name.clone())?;
                Ok((name, serde_json::to_value(steps)?))
            })
            .collect::<Result<Map<String, Value>>>()?;

        let output = json!({ "queue": queue, "journal": journal });
        fs::write(stage.join("native.tmp.json"), serde_json::to_vec(&output)?)?;
        Ok(())
    }
}
