use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};

pub const MAGIC: &[u8; 8] = b"MTJRNL2\n";
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const SEGMENT_BYTES: u64 = 16 * 1024 * 1024;
pub const RAW_HOT_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// The existing journal replay envelope, with payload bytes owned by each frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Frame {
    Output {
        sequence: u64,
        data: Vec<u8>,
    },
    Resize {
        sequence: u64,
        cols: u32,
        rows: u32,
    },
    Reset {
        sequence: u64,
        screen_epoch: u64,
    },
    Encoding {
        sequence: u64,
        encoding: String,
    },
    DomainCursor {
        sequence: u64,
        domain_event_sequence: u64,
    },
    Exit {
        sequence: u64,
        exit_code: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
    },
}

impl Frame {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::Output { sequence, .. }
            | Self::Resize { sequence, .. }
            | Self::Reset { sequence, .. }
            | Self::Encoding { sequence, .. }
            | Self::DomainCursor { sequence, .. }
            | Self::Exit { sequence, .. } => *sequence,
        }
    }

    pub fn output(&self) -> &[u8] {
        match self {
            Self::Output { data, .. } => data,
            _ => &[],
        }
    }

    pub fn domain_cursor(&self) -> Option<u64> {
        match self {
            Self::DomainCursor {
                domain_event_sequence,
                ..
            } => Some(*domain_event_sequence),
            _ => None,
        }
    }
}

/// Timestamp is persisted but omitted from the existing replay envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub timestamp: i64,
    pub frame: Frame,
}

#[derive(Debug)]
pub struct JournalCorruptionError {
    pub message: String,
    pub offset: u64,
    pub quarantine_path: Option<PathBuf>,
}

impl JournalCorruptionError {
    pub fn new(message: impl Into<String>, offset: u64) -> Self {
        Self {
            message: message.into(),
            offset,
            quarantine_path: None,
        }
    }
}

impl fmt::Display for JournalCorruptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for JournalCorruptionError {}
