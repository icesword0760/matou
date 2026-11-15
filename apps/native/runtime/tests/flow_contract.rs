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
