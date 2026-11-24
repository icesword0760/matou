use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Result;
use matou_native_core::{
    journal::{
        SegmentJournal,
        checkpoint::{CreateCheckpointInput, create_checkpoint, protected_terminal_sequences},
    },
    pty::LaunchProfile,
};
use rusqlite::{Connection, OpenFlags};

use crate::SessionOptions;

const OUTPUT_BYTES: usize = 256 * 1024;
const OUTPUT_FRAMES: usize = 200;
const MIN_INTERVAL: Duration = Duration::from_secs(15);
const MAX_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) fn protect_existing(
    connection: &Connection,
    options: &mut SessionOptions,
) -> Result<()> {
    options.journal.checkpoint_protected_sequences =
        protected_terminal_sequences(connection, &options.session_id)?;
    Ok(())
}

pub(crate) struct CapturedCheckpoint {
    pub terminal_sequence: u64,
    pub screen_epoch: u64,
    pub cols: u16,
    pub rows: u16,
    pub snapshot: Vec<u8>,
}

pub(crate) struct CheckpointSync {
    pub last_checkpoint_sequence: u64,
    bytes_since_checkpoint: usize,
    frames_since_checkpoint: usize,
    last_checkpoint_at: Instant,
    retry_after: Option<Instant>,
}

impl CheckpointSync {
    pub fn for_session(options: &SessionOptions) -> Option<Self> {
        // Shell cards restore their completed command history. Provider cards
        // are the sessions whose startup screen is restored from the journal.
        if options.launch.profile == LaunchProfile::Shell
            || options.screen_recovery.is_none()
            || !options.launch.data_root.join("matou.sqlite").is_file()
        {
            return None;
        }
        Some(Self {
            last_checkpoint_sequence: options
                .journal
                .checkpoint_protected_sequences
                .iter()
                .copied()
                .max()
                .unwrap_or_default(),
            bytes_since_checkpoint: 0,
            frames_since_checkpoint: 0,
            last_checkpoint_at: Instant::now(),
            retry_after: None,
        })
    }

    pub fn record_output(&mut self, bytes: usize) {
        self.bytes_since_checkpoint = self.bytes_since_checkpoint.saturating_add(bytes);
        self.frames_since_checkpoint = self.frames_since_checkpoint.saturating_add(1);
    }

    pub fn deadline(&self) -> Option<Instant> {
        if self.bytes_since_checkpoint == 0 {
            return None;
        }
        let interval = if self.bytes_since_checkpoint >= OUTPUT_BYTES
            || self.frames_since_checkpoint >= OUTPUT_FRAMES
        {
            MIN_INTERVAL
        } else {
            MAX_INTERVAL
        };
        let due = self.last_checkpoint_at + interval;
        Some(self.retry_after.map_or(due, |retry| due.max(retry)))
    }

    pub fn attempted(&mut self, now: Instant) {
        self.retry_after = Some(now + MIN_INTERVAL);
    }

    pub fn persist(
        &mut self,
        data_root: &Path,
        session_id: &str,
        journal: &mut SegmentJournal,
        captured: CapturedCheckpoint,
    ) -> Result<()> {
        // This runs on the session worker. A busy owner or read-only store
        // must not add a SQLite lock wait to a normal terminal close.
        let mut connection = Connection::open_with_flags(
            data_root.join("matou.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        connection.busy_timeout(Duration::ZERO)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")?;
        journal.flush()?;
        create_checkpoint(
            data_root,
            &mut connection,
            CreateCheckpointInput {
                session_id,
                terminal_sequence: captured.terminal_sequence,
                domain_event_sequence: journal
                    .domain_event_sequence_at_or_before(captured.terminal_sequence),
                screen_epoch: captured.screen_epoch,
                cols: captured.cols,
                rows: captured.rows,
                snapshot: &captured.snapshot,
            },
        )?;
        self.last_checkpoint_sequence = captured.terminal_sequence;
        self.last_checkpoint_at = Instant::now();
        self.bytes_since_checkpoint = 0;
        self.frames_since_checkpoint = 0;
        self.retry_after = None;
        journal
            .protect_checkpoint_sequences(&protected_terminal_sequences(&connection, session_id)?)
    }
}
