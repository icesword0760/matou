use std::collections::VecDeque;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::model::MAX_SAFE_INTEGER;

const VERSION: u32 = 1;
const SPARSE_FRAME_INTERVAL: u64 = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SparseEntry {
    pub sequence: u64,
    pub completed_line_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainCursorEntry {
    pub sequence: u64,
    pub domain_event_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailIndexSnapshot {
    pub version: u32,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub completed_line_count: u64,
    pub frames_recorded: u64,
    pub max_tracked_lines: usize,
    pub line_start_sequences: Vec<u64>,
    pub sparse: Vec<SparseEntry>,
    pub pending_line_start: bool,
    pub alternate_screen: bool,
    pub escape_bytes: Vec<u8>,
    pub active_segment_index: u64,
    pub domain_cursors: Vec<DomainCursorEntry>,
}

/// Track the start frames of the newest primary-screen lines.
pub struct TailIndex {
    max_tracked_lines: usize,
    first_sequence: u64,
    last_sequence: u64,
    completed_line_count: u64,
    frames_recorded: u64,
    line_start_sequences: VecDeque<u64>,
    sparse: Vec<SparseEntry>,
    pending_line_start: bool,
    alternate_screen: bool,
    escape_bytes: Vec<u8>,
    domain_cursors: Vec<DomainCursorEntry>,
}

impl TailIndex {
    pub fn new(maximum: usize) -> Result<Self> {
        ensure!(
            maximum >= 1 && maximum as u64 <= MAX_SAFE_INTEGER,
            "maxTrackedLines must be a positive integer"
        );
        Ok(Self {
            max_tracked_lines: maximum,
            first_sequence: 0,
            last_sequence: 0,
            completed_line_count: 0,
            frames_recorded: 0,
            line_start_sequences: VecDeque::new(),
            sparse: Vec::new(),
            pending_line_start: false,
            alternate_screen: false,
            escape_bytes: Vec::new(),
            domain_cursors: Vec::new(),
        })
    }

    pub fn record(
        &mut self,
        sequence: u64,
        bytes: &[u8],
        domain_cursor: Option<u64>,
    ) -> Result<()> {
        ensure!(
            sequence <= MAX_SAFE_INTEGER && sequence > self.last_sequence,
            "tail index sequence must increase monotonically"
        );
        if self.first_sequence == 0 {
            self.first_sequence = sequence;
        }
        self.last_sequence = sequence;
        self.frames_recorded += 1;

        if let Some(domain_event_sequence) = domain_cursor {
            ensure!(
                domain_event_sequence <= MAX_SAFE_INTEGER,
                "domainEventSequence must be a non-negative integer"
            );
            ensure!(
                self.domain_cursors.last().is_none_or(|previous| {
                    domain_event_sequence >= previous.domain_event_sequence
                }),
                "domain event sequence must not move backwards"
            );
            self.domain_cursors.push(DomainCursorEntry {
                sequence,
                domain_event_sequence,
            });
        }

        let mut remaining = bytes;
        while let Some((&byte, rest)) = remaining.split_first() {
            if self.escape_bytes.is_empty() {
                let escape_at = remaining.iter().position(|&byte| byte == 0x1b);
                let plain_end = escape_at.unwrap_or(remaining.len());
                if !self.alternate_screen {
                    self.record_plain(sequence, &remaining[..plain_end]);
                }
                let Some(escape_at) = escape_at else {
                    break;
                };
                self.escape_bytes.push(0x1b);
                remaining = &remaining[escape_at + 1..];
            } else {
                self.record_escape_byte(byte);
                remaining = rest;
            }
        }

        if self.frames_recorded.is_multiple_of(SPARSE_FRAME_INTERVAL) {
            self.sparse.push(SparseEntry {
                sequence,
                completed_line_count: self.completed_line_count,
            });
        }
        Ok(())
    }

    pub fn frames_recorded(&self) -> u64 {
        self.frames_recorded
    }

    pub fn tail_start(&self, max_lines: usize) -> Result<u64> {
        ensure!(
            (1..=self.max_tracked_lines).contains(&max_lines),
            "maxLines must be between 1 and {}",
            self.max_tracked_lines
        );
        if self.first_sequence == 0 {
            return Ok(0);
        }
        if self.line_start_sequences.len() <= max_lines {
            return Ok(self.first_sequence);
        }
        Ok(self.line_start_sequences[self.line_start_sequences.len() - max_lines])
    }

    pub fn domain_event_sequence_at_or_before(&self, sequence: u64) -> u64 {
        let count = self
            .domain_cursors
            .partition_point(|entry| entry.sequence <= sequence);
        count
            .checked_sub(1)
            .map_or(0, |index| self.domain_cursors[index].domain_event_sequence)
    }

    pub fn snapshot(&self, active_segment_index: u64) -> TailIndexSnapshot {
        TailIndexSnapshot {
            version: VERSION,
            first_sequence: self.first_sequence,
            last_sequence: self.last_sequence,
            completed_line_count: self.completed_line_count,
            frames_recorded: self.frames_recorded,
            max_tracked_lines: self.max_tracked_lines,
            line_start_sequences: self.line_start_sequences.iter().copied().collect(),
            sparse: self.sparse.clone(),
            pending_line_start: self.pending_line_start,
            alternate_screen: self.alternate_screen,
            escape_bytes: self.escape_bytes.clone(),
            active_segment_index,
            domain_cursors: self.domain_cursors.clone(),
        }
    }

    pub fn from_snapshot(snapshot: TailIndexSnapshot) -> Result<Self> {
        validate_snapshot(&snapshot)?;
        Ok(Self {
            max_tracked_lines: snapshot.max_tracked_lines,
            first_sequence: snapshot.first_sequence,
            last_sequence: snapshot.last_sequence,
            completed_line_count: snapshot.completed_line_count,
            frames_recorded: snapshot.frames_recorded,
            line_start_sequences: snapshot.line_start_sequences.into(),
            sparse: snapshot.sparse,
            pending_line_start: snapshot.pending_line_start,
            alternate_screen: snapshot.alternate_screen,
            escape_bytes: snapshot.escape_bytes,
            domain_cursors: snapshot.domain_cursors,
        })
    }

    fn record_plain(&mut self, sequence: u64, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.pending_line_start || self.line_start_sequences.is_empty() {
            self.push_line_start(sequence);
            self.pending_line_start = false;
        }
        for (offset, &byte) in bytes.iter().enumerate() {
            if byte == b'\n' {
                self.completed_line_count += 1;
                self.pending_line_start = offset + 1 == bytes.len();
                if !self.pending_line_start {
                    self.push_line_start(sequence);
                }
            }
        }
    }

    fn push_line_start(&mut self, sequence: u64) {
        let limit = self.max_tracked_lines.saturating_add(1);
        if self.line_start_sequences.len() >= limit {
            self.line_start_sequences.pop_front();
        }
        self.line_start_sequences.push_back(sequence);
    }

    fn record_escape_byte(&mut self, byte: u8) {
        if let Some(introducer) = control_string_prefix(&self.escape_bytes) {
            let osc_bel = introducer == b']' && byte == 0x07;
            let st = self.escape_bytes.last() == Some(&0x1b) && byte == b'\\';
            if osc_bel || st {
                self.escape_bytes.clear();
            } else {
                // Keep just the prefix and a possible ST introducer across frames.
                self.escape_bytes.truncate(2);
                if byte == 0x1b {
                    self.escape_bytes.push(byte);
                }
            }
            return;
        }
        self.escape_bytes.push(byte);
        if escape_complete(&self.escape_bytes) {
            if let Some(alternate) = alternate_screen_mode(&self.escape_bytes) {
                self.alternate_screen = alternate;
            }
            self.escape_bytes.clear();
        } else if self.escape_bytes.len() > 256 {
            self.escape_bytes.clear();
        }
    }
}

fn control_string_prefix(bytes: &[u8]) -> Option<u8> {
    bytes
        .get(1)
        .copied()
        .filter(|byte| matches!(byte, b']' | b'P' | b'X' | b'^' | b'_'))
}

fn escape_complete(bytes: &[u8]) -> bool {
    match bytes {
        [_, b'[', .., last] => (0x40..=0x7e).contains(last),
        [_, b'['] | [] | [_] => false,
        _ => control_string_prefix(bytes).is_none(),
    }
}

fn alternate_screen_mode(bytes: &[u8]) -> Option<bool> {
    let (last, prefix) = bytes.split_last()?;
    // The original ASCII conversion strips the high bit of each byte.
    let valid_prefix = [b"\x1b[?47".as_slice(), b"\x1b[?1047", b"\x1b[?1049"]
        .iter()
        .any(|expected| {
            prefix.len() == expected.len()
                && prefix
                    .iter()
                    .zip(expected.iter())
                    .all(|(byte, expected)| (byte & 0x7f) == *expected)
        });
    if !valid_prefix {
        return None;
    }
    match last {
        b'h' => Some(true),
        b'l' => Some(false),
        _ => None,
    }
}

fn validate_snapshot(snapshot: &TailIndexSnapshot) -> Result<()> {
    let integers = [
        snapshot.first_sequence,
        snapshot.last_sequence,
        snapshot.completed_line_count,
        snapshot.frames_recorded,
        snapshot.max_tracked_lines as u64,
        snapshot.active_segment_index,
    ];
    let empty = snapshot.first_sequence == 0 && snapshot.last_sequence == 0;
    let sequences = snapshot.first_sequence..=snapshot.last_sequence;
    let header_valid = snapshot.version == VERSION
        && integers.iter().all(|&item| item <= MAX_SAFE_INTEGER)
        && snapshot.max_tracked_lines >= 1
        && snapshot.first_sequence <= snapshot.last_sequence
        && if empty {
            snapshot.frames_recorded == 0
        } else {
            snapshot.first_sequence != 0 && snapshot.frames_recorded != 0
        };
    let line_starts_valid = snapshot.line_start_sequences.len()
        <= snapshot.max_tracked_lines.saturating_add(1)
        && snapshot
            .line_start_sequences
            .iter()
            .all(|item| sequences.contains(item))
        && snapshot
            .line_start_sequences
            .windows(2)
            .all(|pair| pair[0] <= pair[1]);
    let sparse_valid = snapshot.sparse.iter().all(|item| {
        item.sequence >= 1
            && sequences.contains(&item.sequence)
            && item.completed_line_count <= snapshot.completed_line_count
    }) && snapshot.sparse.windows(2).all(|pair| {
        pair[0].sequence < pair[1].sequence
            && pair[0].completed_line_count <= pair[1].completed_line_count
    });
    let cursors_valid = snapshot.domain_cursors.iter().all(|item| {
        sequences.contains(&item.sequence) && item.domain_event_sequence <= MAX_SAFE_INTEGER
    }) && snapshot.domain_cursors.windows(2).all(|pair| {
        pair[0].sequence < pair[1].sequence
            && pair[0].domain_event_sequence <= pair[1].domain_event_sequence
    });
    let escape_valid = snapshot.escape_bytes.len() <= 256
        && snapshot
            .escape_bytes
            .first()
            .is_none_or(|&byte| byte == 0x1b);
    ensure!(
        header_valid && line_starts_valid && sparse_valid && cursors_valid && escape_valid,
        "snapshot fields are invalid"
    );
    Ok(())
}
