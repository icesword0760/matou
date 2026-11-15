use std::{cell::RefCell, collections::BTreeMap};

use anyhow::{Result, bail};
use matou_native_core::{
    durability::{DEFAULT_MAX_RETAINED_BYTES, Event, ExecutionPauser, Gate, PendingFrame, State},
    journal::{Frame, Record},
};
use serde::Deserialize;

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

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum Action {
    Append { sequence: u64, bytes: usize },
    Burst { frames: Vec<FrameInput> },
    Retry,
    End,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct FrameInput {
    sequence: u64,
    bytes: usize,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    state: State,
    retained_bytes: usize,
    last_accepted_sequence: u64,
    last_persisted_sequence: u64,
    trace: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct Step {
    action: Action,
    admitted: Option<Snapshot>,
    #[serde(flatten)]
    snapshot: Snapshot,
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

        assert_eq!(input.actions.len(), steps.len());
        for (index, (action, expected)) in input.actions.into_iter().zip(steps).enumerate() {
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
            assert_eq!(actual, expected, "{} step {}", input.name, index + 1);
        }
    }
    Ok(())
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
