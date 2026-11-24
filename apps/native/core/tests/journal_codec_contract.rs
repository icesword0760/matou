use std::{
    fs,
    io::{self, Read},
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::journal::{
    Frame, JournalCorruptionError, MAGIC, MAX_FRAME_BYTES, MAX_SAFE_INTEGER, Record, TailIndex,
    TailIndexSnapshot,
    codec::{FrameReader, encode_frame, read_segment, repair_segment_tail},
};
use serde::Deserialize;
use serde_json::Value;

const REFERENCE: &str = include_str!("fixtures/journal-reference.json");

#[test]
fn truncated_gzip_header_preserves_the_decoder_eof() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let gzip = STANDARD.decode(&reference.segments[0].gzip_base64)?;
    let root = tempfile::tempdir()?;
    let path = root.path().join("segment.mtj.gz");
    fs::write(&path, &gzip[..5])?;

    let error = read_segment(&path)
        .err()
        .context("expected truncated gzip header")?;
    let corruption = error.downcast_ref::<JournalCorruptionError>().unwrap();
    assert_eq!(corruption.offset, 0);
    assert_eq!(
        corruption.message,
        "unexpected end of file while reading gzip journal"
    );
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::UnexpectedEof
    );
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    records: Vec<Record>,
    segments: Vec<Segment>,
    corruptions: Vec<Corruption>,
    tail_cases: Vec<TailCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Segment {
    name: String,
    base64: String,
    gzip_base64: String,
    frames: Vec<Frame>,
    gzip_frames: Vec<Frame>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Corruption {
    kind: String,
    base64: String,
    strict_error: Fault,
    repair: Option<Repair>,
    repair_error: Option<Fault>,
    repaired_base64: Option<String>,
}

#[derive(Deserialize)]
struct Fault {
    message: String,
    offset: u64,
    #[serde(default)]
    quarantined: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repair {
    truncated_bytes: u64,
    last_sequence: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TailCase {
    name: String,
    maximum: usize,
    inputs: Vec<TailFrame>,
    snapshot: Value,
    tail_start: u64,
    domain_at_first: u64,
    domain_at_last: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TailFrame {
    sequence: u64,
    data: Vec<u8>,
    domain_event_sequence: Option<u64>,
}

#[test]
fn frozen_segments_encode_identically_and_read_raw_and_gzip() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    assert_eq!(reference.records.len(), 8);
    assert_eq!(reference.segments.len(), 2);
    let directory = tempfile::tempdir()?;
    let mut records = reference.records.into_iter();
    for segment in reference.segments {
        let raw = STANDARD.decode(segment.base64)?;
        let gzip = STANDARD.decode(segment.gzip_base64)?;
        let expected: Vec<_> = records.by_ref().take(segment.frames.len()).collect();
        assert_eq!(
            expected
                .iter()
                .map(|record| &record.frame)
                .collect::<Vec<_>>(),
            segment.frames.iter().collect::<Vec<_>>(),
            "{} source frame order",
            segment.name,
        );
        let mut encoded = MAGIC.to_vec();
        for record in &expected {
            encoded.extend(encode_frame(record)?);
        }
        assert_eq!(encoded, raw, "{} byte encoding", segment.name);

        let raw_path = directory.path().join(&segment.name);
        let gzip_path = directory.path().join(format!("{}.gz", segment.name));
        fs::write(&raw_path, &raw)?;
        fs::write(&gzip_path, &gzip)?;
        for (path, expected_frames) in [
            (&raw_path, segment.frames),
            (&gzip_path, segment.gzip_frames),
        ] {
            let mut reader = read_segment(path)?;
            let records = reader.by_ref().collect::<Result<Vec<_>>>()?;
            assert_eq!(records, expected, "{path:?} all records and timestamps");
            assert_eq!(
                records
                    .into_iter()
                    .map(|record| record.frame)
                    .collect::<Vec<_>>(),
                expected_frames,
                "{path:?} replay envelope",
            );
            assert_eq!(reader.last_good_offset(), raw.len() as u64, "{path:?}");
            assert_eq!(
                reader.last_sequence(),
                expected.last().unwrap().frame.sequence(),
                "{path:?}"
            );
        }
    }
    assert!(records.next().is_none());
    Ok(())
}

fn check_fault(error: &anyhow::Error, fault: &Fault, label: &str) -> Result<()> {
    let error = error
        .downcast_ref::<JournalCorruptionError>()
        .with_context(|| format!("{label}: expected journal corruption, got {error}"))?;
    assert_eq!(error.message, fault.message, "{label}");
    assert_eq!(error.offset, fault.offset, "{label}");
    assert_eq!(
        error.quarantine_path.is_some(),
        fault.quarantined,
        "{label}"
    );
    Ok(())
}

#[test]
fn frozen_corruptions_repair_only_torn_tails_and_quarantine_middle_damage() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    assert_eq!(reference.corruptions.len(), 5);
    let directory = tempfile::tempdir()?;
    for case in reference.corruptions {
        let path = directory.path().join(format!("{}.mtj", case.kind));
        let bytes = STANDARD.decode(case.base64)?;
        fs::write(&path, &bytes)?;
        let strict = read_segment(&path).and_then(Iterator::collect::<Result<Vec<_>>>);
        check_fault(&strict.unwrap_err(), &case.strict_error, &case.kind)?;
        assert_eq!(
            fs::read(&path)?,
            bytes,
            "{} strict reads do not mutate",
            case.kind
        );

        let repair = repair_segment_tail(&path, 42);
        match (case.repair, case.repair_error) {
            (Some(expected), None) => {
                let repaired = repair?;
                assert_eq!(
                    repaired.truncated_bytes, expected.truncated_bytes,
                    "{}",
                    case.kind
                );
                assert_eq!(
                    repaired.last_sequence, expected.last_sequence,
                    "{}",
                    case.kind
                );
                assert_eq!(
                    fs::read(&path)?,
                    STANDARD.decode(case.repaired_base64.unwrap())?,
                    "{} repaired bytes",
                    case.kind
                );
                let records = read_segment(&path)?.collect::<Result<Vec<_>>>()?;
                assert_eq!(
                    records.last().unwrap().frame.sequence(),
                    expected.last_sequence
                );
            }
            (None, Some(expected)) => {
                let fault = repair.unwrap_err();
                check_fault(&fault, &expected, &case.kind)?;
                let error = fault.downcast::<JournalCorruptionError>()?;
                let quarantine_path = directory
                    .path()
                    .join(format!("{}.mtj.corrupt-42", case.kind));
                assert_eq!(error.quarantine_path, Some(quarantine_path.clone()));
                assert!(
                    !path.exists(),
                    "{} must be removed from active path",
                    case.kind
                );
                assert_eq!(
                    fs::read(quarantine_path)?,
                    bytes,
                    "{} preserved original",
                    case.kind
                );
            }
            _ => anyhow::bail!("{} has no coherent frozen repair outcome", case.kind),
        }
    }
    Ok(())
}

#[test]
fn frozen_tail_indexes_match_after_snapshot_restore_mid_stream() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    assert_eq!(reference.tail_cases.len(), 3);
    for case in reference.tail_cases {
        let mut index = TailIndex::new(case.maximum)?;
        let expected_count = case.inputs.len() as u64;
        for (offset, input) in case.inputs.into_iter().enumerate() {
            index.record(input.sequence, &input.data, input.domain_event_sequence)?;
            if offset == 1 {
                let sidecar = serde_json::to_vec(&index.snapshot(2))?;
                drop(index);
                index = TailIndex::from_snapshot(serde_json::from_slice(&sidecar)?)?;
            }
        }
        assert_eq!(
            serde_json::to_value(index.snapshot(2))?,
            case.snapshot,
            "{}",
            case.name
        );
        assert_eq!(
            index.tail_start(case.maximum)?,
            case.tail_start,
            "{}",
            case.name
        );
        assert_eq!(index.frames_recorded(), expected_count, "{}", case.name);
        assert_eq!(
            index.domain_event_sequence_at_or_before(1),
            case.domain_at_first,
            "{}",
            case.name
        );
        assert_eq!(
            index.domain_event_sequence_at_or_before(999_999),
            case.domain_at_last,
            "{}",
            case.name
        );
    }
    Ok(())
}

struct OneByte<R>(R);

impl<R: Read> Read for OneByte<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let limit = output.len().min(1);
        self.0.read(&mut output[..limit])
    }
}

fn frame_with_header(header: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(header.len() as u32).to_be_bytes());
    body.extend_from_slice(header);
    body.extend_from_slice(payload);
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(body.len() as u32).to_be_bytes());
    encoded.extend_from_slice(&crc32fast::hash(&body).to_be_bytes());
    encoded.extend_from_slice(&body);
    encoded
}

#[test]
fn invalid_headers_and_sequences_are_fatal_even_at_the_tail() -> Result<()> {
    let record = Record {
        timestamp: 123,
        frame: Frame::Output {
            sequence: 9,
            data: b"one".to_vec(),
        },
    };
    let valid_frame = encode_frame(&record)?;
    let mut oversized_prefix = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec();
    oversized_prefix.extend_from_slice(&[0; 4]);
    let cases = [
        (oversized_prefix, "invalid journal frame length"),
        (
            frame_with_header(b"{}", b""),
            "non-monotonic journal sequence",
        ),
        (
            frame_with_header(b"{x", b""),
            "invalid journal frame header JSON",
        ),
        (
            frame_with_header(b" ", b""),
            "invalid journal frame header length",
        ),
        (
            frame_with_header(
                br#"{"kind":"output","sequence":10,"timestamp":123,"dataLength":2}"#,
                b"one",
            ),
            "journal payload length mismatch",
        ),
        (
            frame_with_header(
                br#"{"kind":"resize","sequence":10,"timestamp":123,"dataLength":1}"#,
                b"x",
            ),
            "control frame contains an unexpected payload",
        ),
        (
            frame_with_header(
                br#"{"kind":"output","sequence":9007199254740992,"timestamp":123,"dataLength":0}"#,
                b"",
            ),
            "non-monotonic journal sequence",
        ),
        (valid_frame.clone(), "non-monotonic journal sequence"),
    ];
    for (suffix, message) in cases {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&valid_frame);
        let valid_offset = bytes.len() as u64;
        bytes.extend_from_slice(&suffix);
        let mut reader = FrameReader::new(OneByte(bytes.as_slice()), true)?;
        assert_eq!(reader.next().context("missing first frame")??, record);
        let error = reader
            .next()
            .context("invalid last frame was treated as a torn write")?
            .unwrap_err();
        check_fault(
            &error,
            &Fault {
                message: message.into(),
                offset: valid_offset,
                quarantined: false,
            },
            message,
        )?;
        assert_eq!(reader.last_good_offset(), valid_offset);
        assert_eq!(reader.last_sequence(), 9);
        assert!(reader.next().is_none());
    }
    let invalid = Record {
        timestamp: 123,
        frame: Frame::Output {
            sequence: MAX_SAFE_INTEGER + 1,
            data: vec![],
        },
    };
    assert!(encode_frame(&invalid).is_err());
    Ok(())
}

#[test]
fn invalid_tail_snapshots_and_decreasing_sequences_are_rejected() -> Result<()> {
    assert!(TailIndex::new(0).is_err());
    let mut index = TailIndex::new(2)?;
    assert!(index.tail_start(3).is_err());
    index.record(1, b"a\n", Some(1))?;
    assert!(index.record(1, b"duplicate", None).is_err());
    assert!(index.record(MAX_SAFE_INTEGER + 1, b"", None).is_err());
    assert_eq!(index.frames_recorded(), 1);
    index.record(2, b"b", Some(2))?;
    let snapshot = index.snapshot(2);
    let mut invalid_line = snapshot.clone();
    invalid_line.line_start_sequences.push(999);
    let mut invalid_prefix = snapshot.clone();
    invalid_prefix.escape_bytes = b"not an escape".to_vec();
    let mut invalid_cursor = snapshot.clone();
    invalid_cursor.domain_cursors[1].domain_event_sequence = 0;
    let invalid_segment = TailIndexSnapshot {
        active_segment_index: MAX_SAFE_INTEGER + 1,
        ..snapshot
    };
    for invalid in [
        invalid_line,
        invalid_prefix,
        invalid_cursor,
        invalid_segment,
    ] {
        let Err(error) = TailIndex::from_snapshot(invalid) else {
            anyhow::bail!("invalid snapshot was accepted");
        };
        assert_eq!(error.to_string(), "snapshot fields are invalid");
    }
    Ok(())
}

#[derive(Deserialize)]
struct GateCodecInput {
    now: i64,
    codec: Vec<GateCodecCase>,
}

#[derive(Deserialize)]
struct GateCodecCase {
    id: String,
    name: String,
    base64: String,
    repair: bool,
}

fn gate_codec_error(error: &anyhow::Error) -> Value {
    use serde_json::json;

    if let Some(fault) = error.downcast_ref::<JournalCorruptionError>() {
        return json!({
            "type": "JournalCorruptionError",
            "message": fault.message,
            "offset": fault.offset,
            "quarantined": fault.quarantine_path.is_some(),
        });
    }
    if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
        return json!({
            "type": "std::io::Error",
            "message": io_error.to_string(),
            "code": format!("{:?}", io_error.kind()),
        });
    }
    json!({ "type": "anyhow::Error", "message": error.to_string() })
}

fn gate_strict_read(path: &std::path::Path) -> Value {
    use serde_json::json;

    let mut frames = Vec::new();
    let result = read_segment(path).and_then(|reader| {
        for result in reader {
            frames.push(result?.frame);
        }
        Ok(())
    });
    json!({ "frames": frames, "error": result.err().as_ref().map(gate_codec_error) })
}

#[test]
#[ignore = "requires K-storage-journal run.sh to supply the fixed inputs"]
fn export_fixed_journal_codec() -> Result<()> {
    use std::path::PathBuf;

    use serde_json::json;

    let stage = PathBuf::from(
        std::env::var_os("MATOU_JOURNAL_GATE_RUN").context("MATOU_JOURNAL_GATE_RUN is required")?,
    );
    let input: GateCodecInput = serde_json::from_slice(&fs::read(stage.join("inputs.json"))?)?;
    let directory = tempfile::tempdir_in(&stage)?;
    let mut observations = Vec::new();
    for case in input.codec {
        let path = directory.path().join(&case.name);
        let bytes = STANDARD.decode(&case.base64)?;
        fs::write(&path, &bytes)?;
        let strict = gate_strict_read(&path);
        assert_eq!(
            fs::read(&path)?,
            bytes,
            "{} strict read mutated input",
            case.id
        );

        let repair = if case.repair {
            match repair_segment_tail(&path, input.now) {
                Ok(report) => json!({
                    "report": { "lastSequence": report.last_sequence, "truncatedBytes": report.truncated_bytes },
                    "bytes": STANDARD.encode(fs::read(&path)?),
                    "after": gate_strict_read(&path),
                }),
                Err(error) => {
                    let mut observation = json!({ "error": gate_codec_error(&error) });
                    if let Some(quarantine) = error
                        .downcast_ref::<JournalCorruptionError>()
                        .and_then(|fault| fault.quarantine_path.as_ref())
                    {
                        observation["quarantine"] = json!(
                            quarantine
                                .file_name()
                                .and_then(|name| name.to_str())
                                .context("fixed quarantine filename is UTF-8")?
                        );
                        observation["quarantinedBytes"] =
                            json!(STANDARD.encode(fs::read(quarantine)?));
                        observation["originalPresent"] = json!(path.try_exists()?);
                    }
                    observation
                }
            }
        } else {
            Value::Null
        };
        observations.push(json!({ "id": case.id, "strict": strict, "repair": repair }));
    }
    fs::write(
        stage.join("codec.native.json"),
        serde_json::to_vec(&observations)?,
    )?;
    Ok(())
}
