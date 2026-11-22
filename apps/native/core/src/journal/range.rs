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
    let mut saw_not_found = false;
    if let Some(result) = try_variants(
        variants.iter().copied(),
        &mut previous,
        &mut failure,
        &mut saw_not_found,
        &mut visitor,
    )? {
        return Ok(result);
    }

    // Compression publishes gzip before deleting raw. Refresh the listing once
    // when an initially listed path went missing.
    if saw_not_found
        && let Some(first) = variants.first()
        && let Some(directory) = first.path.parent()
        && let Ok(mut described) = describe_segments(directory)
    {
        described.retain(|candidate| {
            candidate.index == first.index
                && !variants
                    .iter()
                    .any(|initial| initial.path == candidate.path)
        });
        let new_variants = groups(&described).remove(&first.index).unwrap_or_default();
        if let Some(result) = try_variants(
            new_variants,
            &mut previous,
            &mut failure,
            &mut saw_not_found,
            &mut visitor,
        )? {
            return Ok(result);
        }
    }
    Err(failure
        .unwrap_or_else(|| JournalCorruptionError::new("Journal segment is unavailable", 0).into()))
}

fn try_variants<'a>(
    variants: impl IntoIterator<Item = &'a Segment>,
    previous: &mut u64,
    failure: &mut Option<anyhow::Error>,
    saw_not_found: &mut bool,
    visitor: &mut impl FnMut(Record) -> Result<bool>,
) -> Result<Option<bool>> {
    let mut record_failure = |error: anyhow::Error| {
        *saw_not_found |= error
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::NotFound);
        *failure = Some(error);
    };
    'variants: for variant in variants {
        let reader = match read_segment(&variant.path) {
            Ok(reader) => reader,
            Err(error) => {
                record_failure(error);
                continue;
            }
        };
        for record in reader {
            match record {
                Ok(record) if record.frame.sequence() > *previous => {
                    *previous = record.frame.sequence();
                    if !visitor(record)? {
                        return Ok(Some(false));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    record_failure(error);
                    continue 'variants;
                }
            }
        }
        return Ok(Some(true));
    }
    Ok(None)
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
    #[cfg(test)]
    if let Some(after_initial_listing) = tests::AFTER_INITIAL_LISTING.take() {
        after_initial_listing(&described)?;
    }
    let mut previous = 0;
    let mut visit_segment = |variants: &[&Segment]| {
        visit_preferred(variants, |record| {
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
        })
    };
    let bounds = match read_bounds(&directory, &grouped) {
        Ok(bounds) => bounds,
        Err(_) => {
            // An unindexed damaged segment can interrupt the bounds scan.
            // Stream the segments so readers still receive the valid prefix.
            for variants in grouped.values() {
                if !visit_segment(variants)? {
                    break;
                }
            }
            return Ok(());
        }
    };
    for entry in bounds {
        if entry.last_sequence < from_sequence {
            continue;
        }
        if through_sequence.is_some_and(|through| entry.first_sequence > through)
            || !visit_segment(&grouped[&entry.index])?
        {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::journal::{Frame, JournalOptions, SegmentJournal, wait_for_compression};

    type ListingHook = Box<dyn FnOnce(&[Segment]) -> Result<()>>;

    thread_local! {
        pub(super) static AFTER_INITIAL_LISTING: Cell<Option<ListingHook>> = const { Cell::new(None) };
    }

    const SESSION_ID: &str = "publish-race";

    fn prepare_raw_segments(root: &Path) -> Result<Vec<Record>> {
        let options = JournalOptions {
            max_segment_bytes: 400,
            compress_sealed: false,
            ..JournalOptions::default()
        };
        let mut journal = SegmentJournal::open(root, SESSION_ID, options)?;
        let mut expected = Vec::new();
        for sequence in 1..=4 {
            let record = Record {
                timestamp: sequence as i64,
                frame: Frame::Output {
                    sequence,
                    data: vec![b'a' + sequence as u8; 100],
                },
            };
            journal.append(&record)?;
            expected.push(record);
        }
        journal.close()?;
        let index = load_range(journal.directory()).context("missing fixture range index")?;
        let bounds: Vec<_> = index
            .segments
            .iter()
            .map(|bound| (bound.index, bound.first_sequence, bound.last_sequence))
            .collect();
        assert_eq!(bounds, [(1, 1, 2), (2, 3, 4)]);
        Ok(expected)
    }

    fn publish_after_listing(root: &Path) {
        let root = root.to_owned();
        AFTER_INITIAL_LISTING.set(Some(Box::new(move |described| {
            let raw = directory(&root, SESSION_ID)?.join("segment-000001.mtj");
            let gzip = raw.with_extension("mtj.gz");
            let listed: Vec<_> = described
                .iter()
                .filter(|segment| segment.index == 1)
                .map(|segment| &segment.path)
                .collect();
            assert_eq!(listed, [&raw]);
            assert!(raw.is_file());
            assert!(!gzip.try_exists()?);

            let options = JournalOptions {
                raw_hot_bytes: 0,
                ..JournalOptions::default()
            };
            let mut journal = SegmentJournal::open(&root, SESSION_ID, options)?;
            wait_for_compression();
            assert!(gzip.is_file());
            assert!(!raw.try_exists()?);
            journal.close()
        })));
    }

    #[test]
    fn publish_race_replays_with_range_index() -> Result<()> {
        let root = tempfile::tempdir()?;
        let expected = prepare_raw_segments(root.path())?;
        publish_after_listing(root.path());

        let mut observed = Vec::new();
        visit_frames(root.path(), SESSION_ID, 1, None, |record| {
            observed.push(record);
            Ok(true)
        })?;
        assert_eq!(observed, expected);
        Ok(())
    }

    #[test]
    fn publish_race_streams_valid_prefix_before_bad_tail() -> Result<()> {
        let root = tempfile::tempdir()?;
        let expected = prepare_raw_segments(root.path())?;
        // An unindexed gzip tail is scanned for bounds, but reopening the raw
        // journal for compression leaves this truncated header in place.
        let tail = directory(root.path(), SESSION_ID)?.join("segment-000003.mtj.gz");
        fs::write(tail, b"\x1f\x8b")?;
        publish_after_listing(root.path());

        let mut observed = Vec::new();
        let error = for_each_frame(root.path(), SESSION_ID, 1, None, |record| {
            observed.push(record);
            Ok(())
        })
        .expect_err("the damaged tail must still be reported");
        assert_eq!(observed, expected, "replay stopped early: {error:#}");
        assert!(error.downcast_ref::<JournalCorruptionError>().is_some());
        assert_eq!(
            error.to_string(),
            "unexpected end of file while reading gzip journal"
        );
        Ok(())
    }

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
