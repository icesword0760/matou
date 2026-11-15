use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};

use super::{
    codec::{encode_frame, read_segment, repair_segment_tail},
    compression,
    index_writer::{IndexWriter, file_options, write_json},
    model::{MAGIC, MAX_SAFE_INTEGER, RAW_HOT_BYTES, Record, SEGMENT_BYTES},
    range::{self, Bounds, Segment},
    tail_index::{TailIndex, TailIndexSnapshot},
};

const TAIL_FILE: &str = "tail-index.json";
const TAIL_LINES: usize = 10_000;

#[derive(Debug, Clone)]
pub struct JournalOptions {
    pub max_segment_bytes: u64,
    pub raw_hot_bytes: u64,
    pub compress_sealed: bool,
    pub checkpoint_protected_sequences: Vec<u64>,
}

impl Default for JournalOptions {
    fn default() -> Self {
        Self {
            max_segment_bytes: SEGMENT_BYTES,
            raw_hot_bytes: RAW_HOT_BYTES,
            compress_sealed: true,
            checkpoint_protected_sequences: Vec::new(),
        }
    }
}

pub struct SegmentJournal {
    directory: PathBuf,
    session_id: String,
    active: Segment,
    handle: Option<File>,
    first_sequence: u64,
    last_sequence: u64,
    segment_bounds: BTreeMap<u64, (u64, u64)>,
    protected: BTreeSet<u64>,
    options: JournalOptions,
    compression_step: u64,
    last_compression_scan: u64,
    tail: TailIndex,
    writer: IndexWriter,
    #[cfg(test)]
    fail_write: bool,
    #[cfg(test)]
    fail_rotate: bool,
}

impl SegmentJournal {
    pub fn open(data_root: &Path, session_id: &str, options: JournalOptions) -> Result<Self> {
        ensure!(
            options.max_segment_bytes >= 128,
            "maxSegmentBytes must be at least 128 bytes"
        );
        let protected = checkpoint_sequences(&options.checkpoint_protected_sequences)?;
        let directory = range::directory(data_root, session_id)?;
        create_directory(&directory)?;
        let mut described = range::describe_segments(&directory)?;
        let compressed: BTreeSet<_> = described
            .iter()
            .filter(|segment| segment.compressed)
            .map(|segment| segment.index)
            .collect();
        let existing = described
            .iter()
            .filter(|segment| !segment.compressed && !compressed.contains(&segment.index))
            .max_by_key(|segment| (segment.index, !segment.legacy));
        let (active, mut handle, first_sequence, active_last) = if let Some(existing) = existing {
            let mut active = existing.clone();
            let repair = repair_segment_tail(&active.path, system_millis()?)?;
            active.bytes = fs::metadata(&active.path)?.len();
            let mut handle = file_options().read(true).append(true).open(&active.path)?;
            restrict_file(&active.path)?;
            if active.bytes == 0 {
                handle.write_all(MAGIC)?;
                active.bytes = MAGIC.len() as u64;
            }
            let first = read_segment(&active.path)?
                .next()
                .transpose()?
                .map_or(0, |r| r.frame.sequence());
            (active, handle, first, repair.last_sequence)
        } else {
            let next = described
                .iter()
                .map(|segment| segment.index)
                .max()
                .unwrap_or(0)
                + 1;
            let active = new_segment(&directory, next);
            let handle = create_segment(&active.path, false)?;
            (active, handle, 0, 0)
        };
        // The worker sees the repaired size and the current segment even when
        // the current process just created it.
        if let Some(entry) = described.iter_mut().find(|entry| entry.path == active.path) {
            entry.bytes = active.bytes;
        } else {
            described.push(active.clone());
        }
        let tail_path = directory.join(TAIL_FILE);
        let grouped = range::groups(&described);
        let tail = restore_tail(&tail_path, &grouped, &active, active_last)?;
        let last_sequence = active_last.max(tail.snapshot(active.index).last_sequence);
        let mut segment_bounds = BTreeMap::new();
        if !protected.is_empty() {
            for result in range::read_bounds(&directory, &grouped)? {
                segment_bounds.insert(result.index, (result.first_sequence, result.last_sequence));
            }
        }
        let writer = IndexWriter::new(tail_path)?;
        let compression_step = 128.max(
            (1024 * 1024)
                .min(options.max_segment_bytes / 4)
                .min(options.raw_hot_bytes / 16),
        );
        handle.flush()?;
        let mut journal = Self {
            directory,
            session_id: session_id.to_owned(),
            active,
            handle: Some(handle),
            first_sequence,
            last_sequence,
            segment_bounds,
            protected,
            options,
            compression_step,
            last_compression_scan: 0,
            tail,
            writer,
            #[cfg(test)]
            fail_write: false,
            #[cfg(test)]
            fail_rotate: false,
        };
        journal.scan_compression();
        Ok(journal)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub fn tail_snapshot(&self) -> TailIndexSnapshot {
        self.tail.snapshot(self.active.index)
    }

    pub fn tail_start(&self, max_lines: usize) -> Result<u64> {
        self.tail.tail_start(max_lines)
    }

    pub fn domain_event_sequence_at_or_before(&self, sequence: u64) -> u64 {
        self.tail.domain_event_sequence_at_or_before(sequence)
    }

    pub fn append(&mut self, record: &Record) -> Result<()> {
        self.file()?;
        let sequence = record.frame.sequence();
        ensure!(
            sequence <= MAX_SAFE_INTEGER && sequence > self.last_sequence,
            "journal sequence must increase monotonically"
        );
        let encoded = encode_frame(record)?;
        if let Some(domain) = record.frame.domain_cursor() {
            ensure!(
                domain
                    >= self
                        .tail
                        .domain_event_sequence_at_or_before(self.last_sequence),
                "domain event sequence must not move backwards"
            );
        }
        if self.active.bytes > MAGIC.len() as u64
            && self.active.bytes.saturating_add(encoded.len() as u64)
                > self.options.max_segment_bytes
        {
            self.rotate()?;
        }
        let file = self.handle.as_mut().context("journal is closed")?;
        let result = write_frame(file, &encoded, {
            #[cfg(test)]
            {
                std::mem::take(&mut self.fail_write)
            }
            #[cfg(not(test))]
            {
                false
            }
        });
        if let Err(error) = result {
            let _ = file.set_len(self.active.bytes);
            return Err(error.into());
        }
        self.active.bytes += encoded.len() as u64;
        if self.first_sequence == 0 {
            self.first_sequence = sequence;
        }
        self.last_sequence = sequence;
        record_tail(&mut self.tail, record)?;
        if self.tail.frames_recorded() % 256 == 0 {
            self.schedule_index();
        }
        if self.active.bytes - self.last_compression_scan >= self.compression_step {
            self.scan_compression();
        }
        Ok(())
    }

    pub fn flush(&self) -> Result<()> {
        Ok(self.file()?.sync_all()?)
    }

    pub fn close(&mut self) -> Result<()> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        let sync = handle.sync_all();
        drop(handle);
        let bounds = self.record_active_bounds();
        self.schedule_index();
        self.writer.close();
        sync?;
        bounds
    }

    pub fn protect_checkpoint_sequences(&mut self, sequences: &[u64]) -> Result<()> {
        self.file()?;
        self.protected = checkpoint_sequences(sequences)?;
        self.scan_compression();
        Ok(())
    }

    fn file(&self) -> Result<&File> {
        self.handle.as_ref().context("journal is closed")
    }

    fn rotate(&mut self) -> Result<()> {
        self.flush()?;
        self.record_active_bounds()?;
        self.schedule_index();
        self.writer.wait();
        let next = new_segment(&self.directory, self.active.index + 1);
        let prepared = create_segment(&next.path, {
            #[cfg(test)]
            {
                std::mem::take(&mut self.fail_rotate)
            }
            #[cfg(not(test))]
            {
                false
            }
        })?;
        self.segment_bounds
            .insert(self.active.index, (self.first_sequence, self.last_sequence));
        self.handle = Some(prepared);
        self.active = next;
        self.first_sequence = 0;
        self.scan_compression();
        Ok(())
    }

    fn schedule_index(&self) {
        self.writer.schedule(self.tail_snapshot());
    }

    fn record_active_bounds(&self) -> Result<()> {
        if self.first_sequence > 0 {
            let bounds = Bounds::new(&self.active, self.first_sequence, self.last_sequence)?;
            range::record_bounds(&self.directory, bounds)?;
        }
        Ok(())
    }

    fn scan_compression(&mut self) {
        self.last_compression_scan = self.active.bytes;
        if !self.options.compress_sealed {
            return;
        }
        let Ok(described) = range::describe_segments(&self.directory) else {
            return;
        };
        let protected = self
            .segment_bounds
            .iter()
            .filter_map(|(&index, &(first, last))| {
                self.protected
                    .range(first..=last)
                    .next()
                    .is_some()
                    .then_some(index)
            })
            .collect();
        for segment in compression::candidates(
            &described,
            &self.active.path,
            self.active.bytes,
            self.options.raw_hot_bytes,
            &protected,
        ) {
            compression::schedule(&self.session_id, segment);
        }
    }
}

impl Drop for SegmentJournal {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn checkpoint_sequences(values: &[u64]) -> Result<BTreeSet<u64>> {
    ensure!(
        values.iter().all(|&value| value <= MAX_SAFE_INTEGER),
        "checkpoint protected sequence must be a non-negative safe integer"
    );
    Ok(values.iter().copied().collect())
}

fn new_segment(directory: &Path, index: u64) -> Segment {
    Segment {
        index,
        path: directory.join(format!("segment-{index:06}.mtj")),
        bytes: MAGIC.len() as u64,
        compressed: false,
        legacy: false,
    }
}

fn create_segment(path: &Path, partial_failure: bool) -> io::Result<File> {
    let mut handle = file_options()
        .read(true)
        .append(true)
        .create_new(true)
        .open(path)?;
    let prepared = write_frame(&mut handle, MAGIC, partial_failure)
        .and_then(|()| handle.sync_all())
        .and_then(|()| restrict_file(path));
    if let Err(error) = prepared {
        drop(handle);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(handle)
}

fn write_frame(file: &mut File, bytes: &[u8], partial_failure: bool) -> io::Result<()> {
    if partial_failure {
        file.write_all(&bytes[..bytes.len() / 2])?;
        return Err(io::Error::other("simulated partial journal write"));
    }
    file.write_all(bytes)
}

fn create_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    fs::create_dir_all(path)
}

fn restrict_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

fn system_millis() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}

fn record_tail(index: &mut TailIndex, record: &Record) -> Result<()> {
    index.record(
        record.frame.sequence(),
        record.frame.output(),
        record.frame.domain_cursor(),
    )
}

/// A segment only becomes part of the tail after its entire stream validates.
fn extend_from_clean_variant(
    base: &TailIndexSnapshot,
    variants: &[&Segment],
    through: u64,
) -> Result<TailIndex> {
    let mut failure = None;
    for variant in variants {
        let mut trial = TailIndex::from_snapshot(base.clone())?;
        let result = (|| -> Result<()> {
            for record in read_segment(&variant.path)? {
                let record = record?;
                if record.frame.sequence() > through {
                    record_tail(&mut trial, &record)?;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => return Ok(trial),
            Err(error) => failure = Some(error),
        }
    }
    Err(failure.unwrap_or_else(|| anyhow::anyhow!("journal segment is unavailable")))
}

fn restore_tail(
    path: &Path,
    grouped: &BTreeMap<u64, Vec<&Segment>>,
    active: &Segment,
    active_last: u64,
) -> Result<TailIndex> {
    let active_only = [active];
    let cached = (|| -> Result<TailIndex> {
        let snapshot: TailIndexSnapshot = serde_json::from_slice(&fs::read(path)?)?;
        let start = snapshot.active_segment_index;
        ensure!(
            (1..=active.index).contains(&start)
                && (start < active.index || snapshot.last_sequence <= active_last),
            "tail index is ahead of the Journal"
        );
        let through = snapshot.last_sequence;
        let mut tail = TailIndex::from_snapshot(snapshot)?;
        for (&index, variants) in grouped.range(start..=active.index) {
            let variants = if index == active.index {
                active_only.as_slice()
            } else {
                variants.as_slice()
            };
            tail = extend_from_clean_variant(&tail.snapshot(index), variants, through)?;
        }
        Ok(tail)
    })();
    if let Ok(tail) = cached {
        return Ok(tail);
    }

    let empty = TailIndex::new(TAIL_LINES)?.snapshot(0);
    let mut tail = TailIndex::from_snapshot(empty.clone())?;
    let mut previous = None;
    for (&index, variants) in grouped {
        if previous.is_some_and(|prior| prior + 1 != index) {
            tail = TailIndex::from_snapshot(empty.clone())?;
        }
        previous = Some(index);
        let variants = if index == active.index {
            active_only.as_slice()
        } else {
            variants.as_slice()
        };
        tail = match extend_from_clean_variant(&tail.snapshot(index), variants, 0) {
            Ok(next) => next,
            Err(_) => extend_from_clean_variant(&empty, variants, 0)
                .unwrap_or(TailIndex::from_snapshot(empty.clone())?),
        };
    }
    write_json(path, &tail.snapshot(active.index), true)?;
    Ok(tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Frame;

    #[test]
    fn write_and_rotation_failure_keep_the_previous_boundary() -> Result<()> {
        let root = tempfile::tempdir()?;
        let options = JournalOptions {
            compress_sealed: false,
            max_segment_bytes: 200,
            ..JournalOptions::default()
        };
        let record = |sequence| Record {
            timestamp: sequence as i64,
            frame: Frame::Output {
                sequence,
                data: vec![b'x'; 80],
            },
        };
        let mut journal = SegmentJournal::open(root.path(), "faults", options)?;
        let original = journal.active.path.clone();
        let original_magic = fs::read(&original)?;
        journal.fail_write = true;
        assert!(journal.append(&record(1)).is_err());
        assert_eq!(journal.last_sequence(), 0);
        assert_eq!(fs::read(&original)?, original_magic);
        journal.append(&record(1))?;
        let sealed = fs::read(&original)?;

        journal.fail_rotate = true;
        assert!(journal.append(&record(2)).is_err());
        assert_eq!(journal.last_sequence(), 1);
        assert!(!journal.directory().join("segment-000002.mtj").exists());
        assert_eq!(fs::read(original)?, sealed);
        assert!(journal.flush().is_ok());

        journal.append(&record(2))?;
        journal.close()?;
        let mut sequences = Vec::new();
        range::for_each_frame(root.path(), "faults", 0, None, |record| {
            sequences.push(record.frame.sequence());
            Ok(())
        })?;
        assert_eq!(sequences, [1, 2]);
        Ok(())
    }
}
