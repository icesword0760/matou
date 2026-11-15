//! Bounded write-ahead gate for the journal records of one session.
use std::collections::VecDeque;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::journal::{MAX_SAFE_INTEGER, Record};

pub const DEFAULT_MAX_RETAINED_BYTES: usize = 4 * 1024 * 1024;
const HIGH_WATERMARK_BYTES: usize = 1024 * 1024;
const LOW_WATERMARK_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Healthy,
    Pausing,
    Paused,
    Recovering,
    Ended,
}

pub trait ExecutionPauser {
    fn pause(&mut self) -> Result<()>;
    fn resume(&mut self) -> Result<()>;
}

pub struct PendingFrame {
    pub record: Record,
    /// Length of the frame's retained bytes as counted by the session.
    pub retained_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Fault {
        failed_sequence: u64,
        retained_bytes: usize,
        error: String,
    },
    Recovered {
        through_sequence: u64,
    },
    SideEffectError {
        sequence: u64,
        error: String,
    },
}

pub struct Gate {
    state: State,
    queue: VecDeque<PendingFrame>,
    retained_bytes: usize,
    last_accepted_sequence: u64,
    last_persisted_sequence: u64,
    max_retained_bytes: usize,
    high_watermark: usize,
    low_watermark: usize,
    throughput_paused: bool,
}

impl Gate {
    pub fn new(initial_sequence: u64, max_retained_bytes: usize) -> Result<Self> {
        ensure!(
            max_retained_bytes > 0 && max_retained_bytes as u64 <= MAX_SAFE_INTEGER,
            "maxRetainedBytes must be a positive safe integer"
        );
        let high_watermark = HIGH_WATERMARK_BYTES.min((max_retained_bytes / 2).max(1));
        let low_watermark = LOW_WATERMARK_BYTES.min(high_watermark / 2);
        Ok(Self {
            state: State::Healthy,
            queue: VecDeque::new(),
            retained_bytes: 0,
            last_accepted_sequence: initial_sequence,
            last_persisted_sequence: initial_sequence,
            max_retained_bytes,
            high_watermark,
            low_watermark,
            throughput_paused: false,
        })
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn last_accepted_sequence(&self) -> u64 {
        self.last_accepted_sequence
    }

    pub fn last_persisted_sequence(&self) -> u64 {
        self.last_persisted_sequence
    }

    /// Only admits work. The session owner calls drain after admitting its batch.
    pub fn append(
        &mut self,
        mut frame: PendingFrame,
        pauser: &mut impl ExecutionPauser,
    ) -> Result<()> {
        self.check_open()?;
        let sequence = frame.record.frame.sequence();
        ensure!(
            sequence <= MAX_SAFE_INTEGER
                && self.last_accepted_sequence.checked_add(1) == Some(sequence),
            "durability frame sequence must be contiguous"
        );
        frame.retained_bytes = frame.retained_bytes.max(1);
        ensure!(
            frame.retained_bytes <= self.max_retained_bytes - self.retained_bytes,
            "session durability buffer exceeds {} bytes",
            self.max_retained_bytes
        );
        self.retained_bytes += frame.retained_bytes;
        self.last_accepted_sequence = sequence;
        self.queue.push_back(frame);
        if self.state == State::Healthy
            && !self.throughput_paused
            && self.retained_bytes >= self.high_watermark
        {
            self.throughput_paused = true;
            let _ = pauser.pause();
        }
        Ok(())
    }

    pub fn drain(
        &mut self,
        pauser: &mut impl ExecutionPauser,
        persist: &mut impl FnMut(&Record) -> Result<()>,
        after: &mut impl FnMut(&Record) -> Result<()>,
        emit: &mut impl FnMut(Event),
    ) -> Result<()> {
        while self.state == State::Healthy {
            let Some(frame) = self.queue.front() else {
                break;
            };
            if let Err(error) = persist(&frame.record) {
                self.state = State::Pausing;
                if !self.throughput_paused {
                    let _ = pauser.pause();
                }
                self.state = State::Paused;
                emit(Event::Fault {
                    failed_sequence: frame.record.frame.sequence(),
                    retained_bytes: self.retained_bytes,
                    error: error.to_string(),
                });
                return Ok(());
            }
            self.commit_head(after, emit);
            if self.throughput_paused && self.retained_bytes <= self.low_watermark {
                pauser.resume()?;
                self.throughput_paused = false;
            }
        }
        Ok(())
    }

    pub fn retry(
        &mut self,
        pauser: &mut impl ExecutionPauser,
        persist: &mut impl FnMut(&Record) -> Result<()>,
        after: &mut impl FnMut(&Record) -> Result<()>,
        emit: &mut impl FnMut(Event),
    ) -> Result<()> {
        self.check_open()?;
        if self.state == State::Healthy {
            return self.drain(pauser, persist, after, emit);
        }

        self.state = State::Recovering;
        while let Some(frame) = self.queue.front() {
            if let Err(error) = persist(&frame.record) {
                self.state = State::Paused;
                return Err(error);
            }
            self.commit_head(after, emit);
        }
        if let Err(error) = pauser.resume() {
            self.state = State::Paused;
            return Err(error);
        }
        self.throughput_paused = false;
        self.state = State::Healthy;
        emit(Event::Recovered {
            through_sequence: self.last_persisted_sequence,
        });
        Ok(())
    }

    /// Releases a stopped process for termination and drops undurable records.
    pub fn end(&mut self, pauser: &mut impl ExecutionPauser) -> Result<()> {
        let needs_resume = matches!(
            self.state,
            State::Paused | State::Pausing | State::Recovering
        );
        self.queue.clear();
        self.retained_bytes = 0;
        self.state = State::Ended;
        if needs_resume {
            pauser.resume()?;
        }
        Ok(())
    }

    fn check_open(&self) -> Result<()> {
        ensure!(
            self.state != State::Ended,
            "session durability gate is ended"
        );
        Ok(())
    }

    fn commit_head(
        &mut self,
        after: &mut impl FnMut(&Record) -> Result<()>,
        emit: &mut impl FnMut(Event),
    ) {
        let frame = self
            .queue
            .pop_front()
            .expect("successful persist has a queued frame");
        self.retained_bytes -= frame.retained_bytes;
        let sequence = frame.record.frame.sequence();
        self.last_persisted_sequence = sequence;
        if let Err(error) = after(&frame.record) {
            emit(Event::SideEffectError {
                sequence,
                error: error.to_string(),
            });
        }
    }
}
