use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::MultiGzDecoder;
use matou_native_core::journal::{
    Frame, JournalOptions, Record, SegmentJournal, for_each_frame, wait_for_compression,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    records: Vec<Record>,
    segments: Vec<ReferenceSegment>,
    tail_snapshot: Value,
    range_index: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceSegment {
    name: String,
    base64: String,
    gzip_base64: String,
}

fn reference() -> Result<Reference> {
    Ok(serde_json::from_str(include_str!(
        "fixtures/journal-reference.json"
    ))?)
}

fn options() -> JournalOptions {
    JournalOptions {
        max_segment_bytes: 512,
        compress_sealed: false,
        ..JournalOptions::default()
    }
}

fn collect(root: &Path, id: &str, first: u64, last: Option<u64>) -> Result<Vec<Record>> {
    let mut result = Vec::new();
    for_each_frame(root, id, first, last, |record| {
        result.push(record);
        Ok(())
    })?;
    Ok(result)
}

fn output(sequence: u64, bytes: usize) -> Record {
    Record {
        timestamp: 1_794_719_966_000 + sequence as i64,
        frame: Frame::Output {
            sequence,
            data: vec![b'x'; bytes],
        },
    }
}

#[test]
fn matches_captured_segments_and_indexes_and_reopens() -> Result<()> {
    let reference = reference()?;
    let root = tempfile::tempdir()?;
    let mut journal = SegmentJournal::open(root.path(), "reference", options())?;
    let directory = journal.directory().to_owned();
    for frame in &reference.records {
        journal.append(frame)?;
    }
    assert_eq!(journal.last_sequence(), 8);
    assert_eq!(journal.tail_start(1)?, 6);
    assert_eq!(journal.domain_event_sequence_at_or_before(4), 0);
    assert_eq!(journal.domain_event_sequence_at_or_before(8), 8);
    assert_eq!(
        serde_json::to_value(journal.tail_snapshot())?,
        reference.tail_snapshot
    );
    journal.flush()?;
    journal.close()?;
    assert!(
        journal
            .append(&output(9, 100))
            .unwrap_err()
            .to_string()
            .contains("closed")
    );

    for segment in &reference.segments {
        assert_eq!(
            fs::read(directory.join(&segment.name))?,
            STANDARD.decode(&segment.base64)?
        );
    }
    let tail: Value = serde_json::from_slice(&fs::read(directory.join("tail-index.json"))?)?;
    let range: Value = serde_json::from_slice(&fs::read(directory.join("range-index.json"))?)?;
    assert_eq!(tail, reference.tail_snapshot);
    assert_eq!(range, reference.range_index);
    assert_eq!(
        collect(root.path(), "reference", 2, Some(6))?,
        reference.records[1..6]
    );
    assert_eq!(
        collect(root.path(), "reference", 1, None)?,
        reference.records
    );

    let mut journal = SegmentJournal::open(root.path(), "reference", options())?;
    assert_eq!(journal.last_sequence(), 8);
    journal.append(&output(9, 100))?;
    journal.close()?;
    assert_eq!(
        collect(root.path(), "reference", 9, None)?,
        [output(9, 100)]
    );
    Ok(())
}

#[test]
fn falls_back_to_gzip_without_duplicate_frames_and_appends_to_legacy_raw() -> Result<()> {
    let reference = reference()?;
    let root = tempfile::tempdir()?;
    let directory = root.path().join("journal").join("legacy");
    fs::create_dir_all(&directory)?;
    let first = &reference.segments[0];
    let second = &reference.segments[1];
    let mut damaged = STANDARD.decode(&first.base64)?;
    let last = damaged.last_mut().context("recorded segment")?;
    *last ^= 0x40;
    fs::write(directory.join(&first.name), damaged)?;
    fs::write(
        directory.join(format!("{}.gz", first.name)),
        STANDARD.decode(&first.gzip_base64)?,
    )?;
    let legacy = directory.join(second.name.replace(".mtj", ".bin"));
    fs::write(&legacy, STANDARD.decode(&second.base64)?)?;

    let mut journal = SegmentJournal::open(root.path(), "legacy", options())?;
    assert_eq!(journal.last_sequence(), 8);
    assert_eq!(collect(root.path(), "legacy", 0, None)?, reference.records);
    journal.append(&output(9, 4))?;
    journal.close()?;
    assert!(fs::metadata(&legacy)?.len() > STANDARD.decode(&second.base64)?.len() as u64);
    assert!(!directory.join(&second.name).exists());
    assert_eq!(collect(root.path(), "legacy", 6, None)?.len(), 4);
    Ok(())
}

#[test]
fn uses_raw_hot_budget_and_checkpoint_protection_when_compressing() -> Result<()> {
    let root = tempfile::tempdir()?;
    let options = JournalOptions {
        max_segment_bytes: 200,
        raw_hot_bytes: 375,
        compress_sealed: true,
        checkpoint_protected_sequences: vec![1],
    };
    let mut journal = SegmentJournal::open(root.path(), "pressure", options)?;
    for sequence in 1..=5 {
        journal.append(&output(sequence, 80))?;
    }
    let directory = journal.directory().to_owned();
    wait_for_compression();
    assert!(directory.join("segment-000001.mtj").is_file());
    assert!(directory.join("segment-000002.mtj.gz").is_file());
    assert!(directory.join("segment-000003.mtj.gz").is_file());
    assert!(directory.join("segment-000004.mtj").is_file());
    assert!(directory.join("segment-000005.mtj").is_file());
    let first_raw = fs::read(directory.join("segment-000001.mtj"))?;
    journal.protect_checkpoint_sequences(&[])?;
    wait_for_compression();
    assert!(!directory.join("segment-000001.mtj").exists());
    let mut first_gzip = Vec::new();
    MultiGzDecoder::new(fs::File::open(directory.join("segment-000001.mtj.gz"))?)
        .read_to_end(&mut first_gzip)?;
    assert_eq!(first_gzip, first_raw);
    journal.close()?;
    let sequences: Vec<_> = collect(root.path(), "pressure", 1, None)?
        .iter()
        .map(|record| record.frame.sequence())
        .collect();
    assert_eq!(sequences, [1, 2, 3, 4, 5]);
    Ok(())
}

#[test]
fn active_tail_is_repaired_and_cold_damage_does_not_prevent_new_output() -> Result<()> {
    let reference = reference()?;
    let root = tempfile::tempdir()?;
    let mut journal = SegmentJournal::open(root.path(), "repair", options())?;
    let directory = journal.directory().to_owned();
    for row in &reference.records {
        journal.append(row)?;
    }
    journal.close()?;

    let first_path = directory.join(&reference.segments[0].name);
    let second_path = directory.join(&reference.segments[1].name);
    let mut cold = fs::read(&first_path)?;
    cold[20] ^= 0x40;
    fs::write(&first_path, &cold)?;
    fs::write(directory.join("tail-index.json"), b"invalid")?;
    fs::write(directory.join("range-index.json"), b"invalid")?;
    let valid_len = fs::metadata(&second_path)?.len();
    fs::OpenOptions::new()
        .append(true)
        .open(&second_path)?
        .write_all(&[0, 0, 0])?;

    let mut journal = SegmentJournal::open(root.path(), "repair", options())?;
    assert_eq!(fs::metadata(&second_path)?.len(), valid_len);
    assert_eq!(journal.last_sequence(), 8);
    assert_eq!(journal.tail_snapshot().first_sequence, 5);
    journal.append(&output(9, 4))?;
    assert_eq!(journal.last_sequence(), 9);
    journal.close()?;
    assert_eq!(fs::read(first_path)?, cold);
    assert!(collect(root.path(), "repair", 1, None).is_err());
    Ok(())
}

#[test]
fn follows_a_stale_tail_index_into_a_new_segment() -> Result<()> {
    let reference = reference()?;
    let root = tempfile::tempdir()?;
    let mut journal = SegmentJournal::open(root.path(), "catch-up", options())?;
    for record in &reference.records[..4] {
        journal.append(record)?;
    }
    journal.close()?;
    let second = &reference.segments[1];
    fs::write(
        journal.directory().join(&second.name),
        STANDARD.decode(&second.base64)?,
    )?;
    let mut journal = SegmentJournal::open(root.path(), "catch-up", options())?;
    assert_eq!(journal.last_sequence(), 8);
    assert_eq!(
        serde_json::to_value(journal.tail_snapshot())?,
        reference.tail_snapshot
    );
    journal.close()
}

#[test]
fn keeps_output_available_when_opportunistic_sidecar_publication_fails() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut journal = SegmentJournal::open(
        root.path(),
        "sidecars",
        JournalOptions {
            max_segment_bytes: 200,
            ..options()
        },
    )?;
    let directory = journal.directory().to_owned();

    // Block asynchronous tail publication during a rotation and close.
    let tail = directory.join("tail-index.json");
    fs::remove_file(&tail)?;
    fs::create_dir(&tail)?;
    let first = output(1, 80);
    let second = output(2, 80);
    journal.append(&first)?;
    journal.append(&second)?;
    assert!(directory.join("segment-000002.mtj").exists());
    journal.close()?;

    // Rebuilt read-side range publication is optional, while writer-side
    // persistence of active bounds still returns its original storage error.
    let range = directory.join("range-index.json");
    let mut reopened = {
        fs::remove_dir(&tail)?;
        SegmentJournal::open(root.path(), "sidecars", options())?
    };
    let third = output(3, 4);
    reopened.append(&third)?;
    fs::remove_file(&range)?;
    fs::create_dir(&range)?;
    assert_eq!(
        collect(root.path(), "sidecars", 1, None)?,
        [first, second, third]
    );
    assert!(reopened.close().is_err());
    assert!(range.is_dir());
    Ok(())
}

#[derive(Deserialize)]
struct GateSegmentInput {
    records: Vec<Record>,
    layouts: std::collections::BTreeMap<String, Vec<GateFile>>,
    ranges: Vec<GateRange>,
    tails: Vec<GateTail>,
}

#[derive(Deserialize)]
struct GateFile {
    name: String,
    base64: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GateRange {
    id: String,
    layout: String,
    from_sequence: u64,
    through_sequence: Option<u64>,
}

#[derive(Deserialize)]
struct GateTail {
    id: String,
    layout: String,
    limit: usize,
}

fn gate_install(root: &Path, case: &str, files: &[GateFile]) -> Result<std::path::PathBuf> {
    let path = root.join("journal").join(case);
    fs::create_dir_all(&path)?;
    for file in files {
        fs::write(path.join(&file.name), STANDARD.decode(&file.base64)?)?;
    }
    Ok(path)
}

fn gate_index(directory: &Path, name: &str) -> Result<Value> {
    use serde_json::json;

    match fs::read(directory.join(name)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "invalidBase64": STANDARD.encode(bytes) }))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(error) => Err(error.into()),
    }
}

fn gate_segment_error(error: &anyhow::Error) -> Value {
    use matou_native_core::journal::JournalCorruptionError;
    use serde_json::json;

    if let Some(fault) = error.downcast_ref::<JournalCorruptionError>() {
        return json!({
            "type": "JournalCorruptionError",
            "message": fault.message,
            "offset": fault.offset,
            "quarantined": fault.quarantine_path.is_some(),
        });
    }
    json!({ "type": "anyhow::Error", "message": error.to_string() })
}

fn gate_range_read(root: &Path, case: &str, from: u64, through: Option<u64>) -> Value {
    use serde_json::json;

    let mut frames = Vec::new();
    let result = for_each_frame(root, case, from, through, |record| {
        frames.push(record.frame);
        Ok(())
    });
    json!({ "frames": frames, "error": result.err().as_ref().map(gate_segment_error) })
}

fn gate_write(
    root: &Path,
    id: &str,
    records: &[Record],
    compress_sealed: bool,
) -> Result<(std::path::PathBuf, Vec<Value>)> {
    use serde_json::json;

    let mut journal = SegmentJournal::open(
        root,
        id,
        JournalOptions {
            raw_hot_bytes: 1,
            compress_sealed,
            ..options()
        },
    )?;
    for record in records {
        journal.append(record)?;
    }
    journal.close()?;
    wait_for_compression();

    let directory = journal.directory().to_owned();
    let mut paths = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    let mut segments = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("segment filename")?;
        if name.ends_with(".mtj") {
            let mut entry = json!({ "name": name, "base64": STANDARD.encode(fs::read(&path)?) });
            if compress_sealed {
                entry["gzipBase64"] = Value::Null;
            }
            segments.push(entry);
        } else if name.ends_with(".mtj.gz") {
            let bytes = fs::read(&path)?;
            let mut raw = Vec::new();
            MultiGzDecoder::new(bytes.as_slice()).read_to_end(&mut raw)?;
            segments.push(json!({
                "name": name,
                "base64": STANDARD.encode(raw),
                "gzipBase64": STANDARD.encode(bytes),
            }));
        }
    }
    Ok((directory, segments))
}

#[test]
#[ignore = "requires K-storage-journal run.sh to supply the fixed inputs"]
fn export_fixed_journal_segments() -> Result<()> {
    use std::path::PathBuf;

    use serde_json::json;

    let stage = PathBuf::from(
        std::env::var_os("MATOU_JOURNAL_GATE_RUN").context("MATOU_JOURNAL_GATE_RUN is required")?,
    );
    let input: GateSegmentInput = serde_json::from_slice(&fs::read(stage.join("inputs.json"))?)?;
    let directory = tempfile::tempdir_in(&stage)?;
    let root = directory.path();
    let (raw_directory, raw) = gate_write(root, "raw", &input.records, false)?;
    let (_, compressed) = gate_write(root, "compressed", &input.records, true)?;
    let mut observations = vec![
        json!({
            "id": "raw-encoding-and-rollover",
            "segments": raw,
            "tailIndex": gate_index(&raw_directory, "tail-index.json")?,
            "rangeIndex": gate_index(&raw_directory, "range-index.json")?,
        }),
        json!({
            "id": "gzip-publication",
            "segments": compressed,
            "strict": gate_range_read(root, "compressed", 0, None),
        }),
    ];
    for case in input.ranges {
        let directory = gate_install(root, &case.id, &input.layouts[&case.layout])?;
        let strict = gate_range_read(root, &case.id, case.from_sequence, case.through_sequence);
        observations.push(json!({
            "id": case.id, "strict": strict,
            "rangeIndex": gate_index(&directory, "range-index.json")?,
        }));
    }
    for case in input.tails {
        let directory = gate_install(root, &case.id, &input.layouts[&case.layout])?;
        let mut journal = SegmentJournal::open(root, &case.id, options())?;
        let snapshot = journal.tail_snapshot();
        let (replay, error) = match journal.tail_start(case.limit) {
            Ok(tail_from) => (
                json!({
                    "firstSequence": snapshot.first_sequence,
                    "lastSequence": snapshot.last_sequence,
                    "tailFromSequence": tail_from,
                    "domainEventSequence": journal.domain_event_sequence_at_or_before(snapshot.last_sequence),
                }),
                Value::Null,
            ),
            Err(error) => (Value::Null, gate_segment_error(&error)),
        };
        journal.close()?;
        observations.push(json!({
            "id": case.id, "replay": replay, "error": error,
            "tailIndex": gate_index(&directory, "tail-index.json")?,
        }));
    }
    fs::write(
        stage.join("segments.native.json"),
        serde_json::to_vec(&observations)?,
    )?;
    Ok(())
}
