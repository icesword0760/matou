use std::{fs, path::PathBuf};

use anyhow::{Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAGIC: &[u8] = b"MTCPV4\n";

/// An existing SQLite index row. Reading the snapshot needs no database connection.
pub struct CheckpointCandidate {
    pub id: String,
    pub session_id: String,
    pub generation: u64,
    pub terminal_sequence: u64,
    pub domain_event_sequence: u64,
    pub screen_epoch: u64,
    pub file_path: PathBuf,
    pub checksum: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    id: String,
    session_id: String,
    generation: u64,
    terminal_sequence: u64,
    domain_event_sequence: u64,
    screen_epoch: u64,
    cols: Option<u16>,
    rows: Option<u16>,
    snapshot_length: usize,
}

pub struct Checkpoint {
    pub grid: Option<(u16, u16)>,
    encoded: Vec<u8>,
    snapshot_start: usize,
}

impl Checkpoint {
    pub fn snapshot(&self) -> &[u8] {
        &self.encoded[self.snapshot_start..]
    }
}

impl CheckpointCandidate {
    pub fn read(&self) -> Result<Checkpoint> {
        let encoded = fs::read(&self.file_path)?;
        ensure!(
            format!("{:x}", Sha256::digest(&encoded)) == self.checksum,
            "checkpoint checksum mismatch"
        );
        let header_start = MAGIC.len() + 4;
        ensure!(
            encoded.starts_with(MAGIC) && encoded.len() >= header_start,
            "invalid checkpoint magic or missing header length"
        );
        let header_length =
            u32::from_be_bytes(encoded[MAGIC.len()..header_start].try_into()?) as usize;
        ensure!(
            header_length <= encoded.len() - header_start,
            "truncated checkpoint header"
        );
        let snapshot_start = header_start + header_length;
        let header: Header = serde_json::from_slice(&encoded[header_start..snapshot_start])?;
        ensure!(
            header.id == self.id
                && header.session_id == self.session_id
                && header.generation == self.generation
                && header.terminal_sequence == self.terminal_sequence
                && header.domain_event_sequence == self.domain_event_sequence
                && header.screen_epoch == self.screen_epoch,
            "checkpoint header does not match its SQLite index"
        );
        ensure!(
            encoded.len() - snapshot_start == header.snapshot_length,
            "checkpoint snapshot length mismatch"
        );
        let grid = match (header.cols, header.rows) {
            (None, None) => None,
            (Some(cols), Some(rows)) if cols >= 2 && rows > 0 => Some((cols, rows)),
            _ => anyhow::bail!("invalid checkpoint grid"),
        };
        Ok(Checkpoint {
            grid,
            encoded,
            snapshot_start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn reads_only_checkpoints_matching_the_v4_index_and_envelope() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut candidate = CheckpointCandidate {
            id: "checkpoint".into(),
            session_id: "session".into(),
            generation: 3,
            terminal_sequence: 10,
            domain_event_sequence: 2,
            screen_epoch: 1,
            file_path: root.path().join("snapshot"),
            checksum: String::new(),
        };
        let header = json!({
            "id": candidate.id,
            "sessionId": candidate.session_id,
            "generation": 3,
            "terminalSequence": 10,
            "domainEventSequence": 2,
            "screenEpoch": 1,
            "snapshotLength": 3,
        });
        let encode = |magic: &[u8], header: &Value| -> Result<Vec<u8>> {
            let header = serde_json::to_vec(header)?;
            let mut encoded = magic.to_vec();
            encoded.extend_from_slice(&u32::try_from(header.len())?.to_be_bytes());
            encoded.extend(header);
            encoded.extend(b"old");
            Ok(encoded)
        };
        let mut read = |encoded: &[u8]| {
            fs::write(&candidate.file_path, encoded)?;
            candidate.checksum = format!("{:x}", Sha256::digest(encoded));
            candidate.read()
        };
        let valid = encode(MAGIC, &header)?;
        let checkpoint = read(&valid)?;
        assert_eq!(checkpoint.grid, None);
        assert_eq!(checkpoint.snapshot(), b"old");
        let mut with_grid = header.clone();
        with_grid["cols"] = json!(24);
        with_grid["rows"] = json!(5);
        assert_eq!(read(&encode(MAGIC, &with_grid)?)?.grid, Some((24, 5)));
        assert!(read(&encode(b"MTCPV3\n", &header)?).is_err());
        let mut truncated = valid;
        truncated[MAGIC.len()] = 1;
        assert!(read(&truncated).is_err());
        for (field, value) in [
            ("id", json!("different")),
            ("sessionId", json!("different")),
            ("generation", json!(4)),
            ("terminalSequence", json!(11)),
            ("domainEventSequence", json!(3)),
            ("screenEpoch", json!(2)),
            ("snapshotLength", json!(2)),
            ("cols", json!(24)),
        ] {
            let mut invalid = header.clone();
            invalid[field] = value;
            assert!(read(&encode(MAGIC, &invalid)?).is_err(), "{field}");
        }
        Ok(())
    }
}
