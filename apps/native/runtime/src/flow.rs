use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};

pub const BATCH_DELAY: Duration = Duration::from_millis(16);
pub const MAX_CODE_UNITS: usize = 64 * 1024;
pub const CREDIT_HIGH_WATERMARK_BYTES: usize = 1024 * 1024;
pub const CREDIT_LOW_WATERMARK_BYTES: usize = 512 * 1024;

/// Preserve a structurally unfinished UTF-8 character between PTY reads.
#[derive(Default)]
pub struct Utf8Decoder {
    tail: [u8; 3],
    tail_len: usize,
}

impl Utf8Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::new();
        }

        let combined;
        let bytes = if self.tail_len == 0 {
            bytes
        } else {
            combined = [&self.tail[..self.tail_len], bytes].concat();
            &combined
        };
        let unfinished = unfinished_bytes(bytes);
        let complete = bytes.len() - unfinished;
        self.tail[..unfinished].copy_from_slice(&bytes[complete..]);
        self.tail_len = unfinished;
        String::from_utf8_lossy(&bytes[..complete]).into_owned()
    }

    pub fn finish(&mut self) -> String {
        let pending = std::mem::take(&mut self.tail_len);
        String::from_utf8_lossy(&self.tail[..pending]).into_owned()
    }
}

fn unfinished_bytes(bytes: &[u8]) -> usize {
    for (trailing, &byte) in bytes.iter().rev().take(3).enumerate() {
        let width = match byte {
            0x80..=0xbf => continue,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => return 0,
        };
        let present = trailing + 1;
        // The reference delays a structurally incomplete suffix even when
        // its lead or second byte already proves the scalar invalid.
        return if present < width { present } else { 0 };
    }
    0
}

pub struct OutputBatcher {
    pending: String,
    code_units: usize,
    delay: Duration,
    max_code_units: usize,
    deadline: Option<Instant>,
}

impl OutputBatcher {
    pub fn new(delay: Duration, max_code_units: usize) -> Result<Self> {
        ensure!(!delay.is_zero(), "delayMs must be a positive integer");
        ensure!(
            max_code_units > 0,
            "maxCodeUnits must be a positive integer"
        );
        Ok(Self {
            pending: String::new(),
            code_units: 0,
            delay,
            max_code_units,
            deadline: None,
        })
    }

    pub fn offer(&mut self, data: &str, now: Instant) -> Option<String> {
        if data.is_empty() {
            return None;
        }
        self.pending.push_str(data);
        self.code_units += data.encode_utf16().count();
        if self.code_units >= self.max_code_units {
            return self.flush();
        }
        self.deadline.get_or_insert_with(|| now + self.delay);
        None
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn take_due(&mut self, now: Instant) -> Option<String> {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            return self.flush();
        }
        None
    }

    pub fn flush(&mut self) -> Option<String> {
        self.deadline = None;
        self.code_units = 0;
        (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
    }
}

pub struct CreditWindow {
    high: usize,
    low: usize,
    pending: VecDeque<(u64, usize)>,
    latest_sent: Option<u64>,
    latest_acknowledged: Option<u64>,
    unacked_bytes: usize,
    paused: bool,
}

impl CreditWindow {
    pub fn new(high: usize, low: usize) -> Result<Self> {
        ensure!(
            high > low,
            "high watermark must be greater than low watermark"
        );
        Ok(Self {
            high,
            low,
            pending: VecDeque::new(),
            latest_sent: None,
            latest_acknowledged: None,
            unacked_bytes: 0,
            paused: false,
        })
    }

    pub fn record_sent(&mut self, sequence: u64, bytes: usize) -> Result<()> {
        ensure!(
            self.latest_sent.is_none_or(|latest| sequence > latest),
            "sent sequence must increase monotonically"
        );
        self.unacked_bytes = self
            .unacked_bytes
            .checked_add(bytes)
            .context("unacknowledged byte count overflow")?;
        self.latest_sent = Some(sequence);
        self.pending.push_back((sequence, bytes));
        if self.unacked_bytes > self.high {
            self.paused = true;
        }
        Ok(())
    }

    /// Apply a cumulative acknowledgement. Return true when sending can resume.
    pub fn acknowledge(&mut self, through_sequence: u64) -> Result<bool> {
        ensure!(
            self.latest_sent
                .is_some_and(|latest| through_sequence <= latest),
            "acknowledgement exceeds latest sent sequence"
        );
        if self
            .latest_acknowledged
            .is_some_and(|latest| through_sequence <= latest)
        {
            return Ok(false);
        }
        while let Some(&(_, bytes)) = self
            .pending
            .front()
            .filter(|(sequence, _)| *sequence <= through_sequence)
        {
            self.pending.pop_front();
            self.unacked_bytes -= bytes;
        }
        self.latest_acknowledged = Some(through_sequence);
        let resumed = self.paused && self.unacked_bytes <= self.low;
        if resumed {
            self.paused = false;
        }
        Ok(resumed)
    }

    pub fn unacked_bytes(&self) -> usize {
        self.unacked_bytes
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }
}
