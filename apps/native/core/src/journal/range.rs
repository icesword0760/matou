use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    codec::read_segment,
    index_writer::write_json,
    model::{JournalCorruptionError, MAX_SAFE_INTEGER, Record},
};

const RANGE_FILE: &str = "range-index.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Segment {
    pub index: u64,
    pub path: PathBuf,
    pub bytes: u64,
    pub compressed: bool,
    pub legacy: bool,
}

pub(super) fn describe_segments(directory: &Path) -> Result<Vec<Segment>> {
    for attempt in 0..=4 {
        let mut result = Vec::new();
        let mut changed = false;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let filename = entry.file_name();
            let Some((index, compressed, legacy)) = filename.to_str().and_then(parse_name) else {
                continue;
            };
            match entry.metadata() {
                Ok(metadata) => result.push(Segment {
                    index,
                    compressed,
                    legacy,
                    bytes: metadata.len(),
                    path: entry.path(),
                }),
                Err(error) if error.kind() == io::ErrorKind::NotFound => changed = true,
                Err(error) => return Err(error.into()),
            }
        }
        if !changed || attempt == 4 {
            return Ok(result);
        }
    }
    unreachable!()
}

fn parse_name(name: &str) -> Option<(u64, bool, bool)> {
    let suffix = name.strip_prefix("segment-")?;
    let (index, suffix) = suffix.split_once('.')?;
    if index.len() != 6 || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (legacy, compressed) = match suffix {
        "mtj" => (false, false),
        "bin" => (true, false),
        "mtj.gz" => (false, true),
        "bin.gz" => (true, true),
        _ => return None,
    };
    Some((index.parse().ok()?, compressed, legacy))
}

pub(super) fn groups(segments: &[Segment]) -> BTreeMap<u64, Vec<&Segment>> {
    let mut grouped = BTreeMap::<_, Vec<_>>::new();
    for segment in segments {
        grouped.entry(segment.index).or_default().push(segment);
    }
    for entries in grouped.values_mut() {
        entries.sort_by_key(|entry| (entry.compressed, entry.legacy));
    }
    grouped
}

pub(super) fn directory(data_root: &Path, session_id: &str) -> Result<PathBuf> {
    let mut components = Path::new(session_id).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none(),
        "invalid journal session ID"
    );
    Ok(data_root.join("journal").join(session_id))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Bounds {
    pub index: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub source_path: String,
    pub source_bytes: u64,
}

impl Bounds {
    pub(super) fn new(segment: &Segment, first: u64, last: u64) -> Result<Self> {
        Ok(Self {
            index: segment.index,
            first_sequence: first,
            last_sequence: last,
            source_path: filename(&segment.path)?.to_owned(),
            source_bytes: segment.bytes,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RangeIndex {
    version: u32,
    segments: Vec<Bounds>,
}

fn filename(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .context("invalid journal segment path")
}

fn validate(index: &RangeIndex) -> Result<()> {
    ensure!(index.version == 1, "range index fields are invalid");
    for (position, bound) in index.segments.iter().enumerate() {
        ensure!(
            (1..=MAX_SAFE_INTEGER).contains(&bound.index)
                && (1..=bound.last_sequence).contains(&bound.first_sequence)
                && bound.last_sequence <= MAX_SAFE_INTEGER
                && (1..=MAX_SAFE_INTEGER).contains(&bound.source_bytes)
                && filename(Path::new(&bound.source_path))
                    .is_ok_and(|name| name == bound.source_path)
                && (position == 0 || {
                    let previous = &index.segments[position - 1];
                    bound.index > previous.index && bound.first_sequence > previous.last_sequence
                }),
            "range index fields are invalid"
        );
    }
    Ok(())
}

fn load_range(directory: &Path) -> Option<RangeIndex> {
    let index = serde_json::from_slice(&fs::read(directory.join(RANGE_FILE)).ok()?).ok()?;
    validate(&index).ok()?;
    Some(index)
}

pub(super) fn record_bounds(directory: &Path, bounds: Bounds) -> Result<()> {
    let path = directory.join(RANGE_FILE);
    let current = load_range(directory);
    let mut index = current.unwrap_or(RangeIndex {
        version: 1,
        segments: Vec::new(),
    });
    match index
        .segments
        .binary_search_by_key(&bounds.index, |entry| entry.index)
    {
        Ok(position) if index.segments[position] == bounds => return Ok(()),
        Ok(position) => index.segments[position] = bounds,
        Err(position) => index.segments.insert(position, bounds),
    }
    validate(&index)?;
    write_json(&path, &index, false)
}

pub(super) fn read_bounds(
    directory: &Path,
    grouped: &BTreeMap<u64, Vec<&Segment>>,
) -> Result<Vec<Bounds>> {
    let stored = load_range(directory);
    let active_index = grouped.last_key_value().map(|(&index, _)| index);
    let mut bounds = Vec::new();
    for (&index, variants) in grouped {
        let current = variants[0];
        let current_name = filename(&current.path)?;
        let cached = stored.as_ref().and_then(|sidecar| {
            let pos = sidecar
                .segments
                .binary_search_by_key(&index, |candidate| candidate.index)
                .ok()?;
            let cached = &sidecar.segments[pos];
            (Some(index) != active_index
                || cached.source_path == current_name && cached.source_bytes == current.bytes)
                .then_some(cached)
        });
        let mut entry = if let Some(cached) = cached {
            cached.clone()
        } else {
            let mut first = 0;
            let mut last = 0;
            visit_preferred(variants, |record| {
                if first == 0 {
                    first = record.frame.sequence();
                }
                last = record.frame.sequence();
                Ok(true)
            })?;
            if first == 0 {
                continue;
            }
            Bounds::new(current, first, last)?
        };
        entry.source_path = current_name.to_owned();
        if bounds
            .last()
            .is_some_and(|previous: &Bounds| previous.last_sequence >= entry.first_sequence)
        {
            return Err(JournalCorruptionError::new(
                "non-monotonic sequence across journal segments",
                0,
            )
            .into());
        }
        bounds.push(entry);
    }
    let refreshed = RangeIndex {
        version: 1,
        segments: bounds,
    };
    if stored.as_ref() != Some(&refreshed) {
        validate(&refreshed)?;
        let _ = write_json(&directory.join(RANGE_FILE), &refreshed, false);
    }
    Ok(refreshed.segments)
}

/// Stop on callback false. A failed source may have already yielded some
/// records; alternatives skip that prefix without invoking the callback again.
pub(super) fn visit_preferred(
    variants: &[&Segment],
    mut visitor: impl FnMut(Record) -> Result<bool>,
) -> Result<bool> {
    let mut failure = None;
    let mut previous = 0;
    for variant in variants {
        let reader = match read_segment(&variant.path) {
            Ok(reader) => reader,
            Err(error) => {
                failure = Some(error);
                continue;
            }
        };
        let mut clean = true;
        for record in reader {
            match record {
                Ok(record) if record.frame.sequence() > previous => {
                    previous = record.frame.sequence();
                    if !visitor(record)? {
                        return Ok(false);
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    failure = Some(error);
                    clean = false;
                    break;
                }
            }
        }
        if clean {
            return Ok(true);
        }
    }
    Err(failure
        .unwrap_or_else(|| JournalCorruptionError::new("Journal segment is unavailable", 0).into()))
}

pub fn for_each_frame(
    data_root: &Path,
    session_id: &str,
    from_sequence: u64,
    through_sequence: Option<u64>,
    mut visitor: impl FnMut(Record) -> Result<()>,
) -> Result<()> {
    visit_frames(
        data_root,
        session_id,
        from_sequence,
        through_sequence,
        |record| {
            visitor(record)?;
            Ok(true)
        },
    )
}

pub fn visit_frames(
    data_root: &Path,
    session_id: &str,
    from_sequence: u64,
    through_sequence: Option<u64>,
    mut visitor: impl FnMut(Record) -> Result<bool>,
) -> Result<()> {
    ensure!(
        from_sequence <= MAX_SAFE_INTEGER,
        "fromSequence must be a non-negative integer"
    );
    ensure!(
        through_sequence.is_none_or(|sequence| sequence <= MAX_SAFE_INTEGER),
        "throughSequence must be a non-negative integer"
    );
    let directory = directory(data_root, session_id)?;
    let described = describe_segments(&directory)?;
    let grouped = groups(&described);
    let bounds = read_bounds(&directory, &grouped)?;
    let mut previous = 0;
    for entry in bounds {
        if entry.last_sequence < from_sequence {
            continue;
        }
        if through_sequence.is_some_and(|through| entry.first_sequence > through) {
            break;
        }
        if !visit_preferred(&grouped[&entry.index], |record| {
            let sequence = record.frame.sequence();
            if sequence <= previous {
                return Err(JournalCorruptionError::new(
                    "non-monotonic sequence across journal segments",
                    0,
                )
                .into());
            }
            previous = sequence;
            if through_sequence.is_some_and(|through| sequence > through) {
                return Ok(false);
            }
            if sequence >= from_sequence {
                return visitor(record);
            }
            Ok(true)
        })? {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Frame, JournalOptions, SegmentJournal};

    #[test]
    fn visitor_false_stops_after_first_frame() -> Result<()> {
        let root = tempfile::tempdir()?;
        let options = JournalOptions {
            compress_sealed: false,
            ..JournalOptions::default()
        };
        let mut journal = SegmentJournal::open(root.path(), "visitor", options)?;
        for sequence in 1..=3 {
            journal.append(&Record {
                timestamp: sequence as i64,
                frame: Frame::Output {
                    sequence,
                    data: vec![b'a'],
                },
            })?;
        }
        journal.close()?;
        let mut observed = Vec::new();
        visit_frames(root.path(), "visitor", 1, None, |record| {
            observed.push(record.frame.sequence());
            Ok(false)
        })?;
        assert_eq!(observed, [1]);
        Ok(())
    }
}
