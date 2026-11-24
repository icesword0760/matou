use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::MAX_SAFE_INTEGER;

const MAGIC: &[u8] = b"MTCPV4\n";
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const RETAINED_GENERATIONS: i64 = 2;

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

#[derive(Deserialize, Serialize)]
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

pub struct CreateCheckpointInput<'a> {
    pub session_id: &'a str,
    pub terminal_sequence: u64,
    pub domain_event_sequence: u64,
    pub screen_epoch: u64,
    pub cols: u16,
    pub rows: u16,
    pub snapshot: &'a [u8],
}

pub struct CreatedCheckpoint {
    pub id: String,
    pub generation: u64,
    pub file_path: PathBuf,
}

pub fn create_checkpoint(
    data_root: &Path,
    connection: &mut Connection,
    input: CreateCheckpointInput<'_>,
) -> Result<CreatedCheckpoint> {
    let session_id = input.session_id;
    ensure!(
        session_id.len() <= 160
            && session_id.starts_with(|c: char| c.is_ascii_alphanumeric())
            && session_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "checkpoint session ID contains unsupported characters"
    );
    ensure!(input.cols >= 2 && input.rows > 0, "invalid checkpoint grid");
    ensure!(
        input.snapshot.len() <= MAX_SNAPSHOT_BYTES,
        "checkpoint snapshot exceeds 16 MiB"
    );
    ensure!(
        [
            input.terminal_sequence,
            input.domain_event_sequence,
            input.screen_epoch
        ]
        .into_iter()
        .all(|value| value <= MAX_SAFE_INTEGER),
        "checkpoint sequences and screen epoch must be safe integers"
    );
    ensure!(
        !connection.is_readonly("main")?
            && !connection.pragma_query_value(None, "query_only", |row| row.get::<_, bool>(0))?,
        "checkpoint storage is read-only"
    );

    // Serialize generation allocation and publication across SQLite writers.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let latest = transaction
        .query_row(
            "SELECT terminal_sequence, domain_event_sequence FROM journal_checkpoints
         WHERE session_id = ? AND valid = 1 ORDER BY generation DESC LIMIT 1",
            [session_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    ensure!(
        latest.is_none_or(|(terminal, domain)| {
            input.terminal_sequence as i64 >= terminal
                && input.domain_event_sequence as i64 >= domain
        }),
        "checkpoint watermark must not move backwards"
    );
    let maximum = transaction.query_row(
        "SELECT COALESCE(MAX(generation), 0) FROM journal_checkpoints WHERE session_id = ?",
        [session_id],
        |row| row.get::<_, i64>(0),
    )?;
    let generation = u64::try_from(maximum)? + 1;
    ensure!(
        generation <= MAX_SAFE_INTEGER,
        "checkpoint generation overflow"
    );

    let id = Uuid::new_v4().to_string();
    let directory = data_root.join("checkpoints").join(session_id);
    let file_path = directory.join(checkpoint_filename(generation));
    let temporary = directory.join(format!("{}.tmp-{id}", checkpoint_filename(generation)));
    let path_text = file_path.to_str().context("checkpoint path is not UTF-8")?;
    let header = serde_json::to_vec(&Header {
        id: id.clone(),
        session_id: session_id.to_owned(),
        generation,
        terminal_sequence: input.terminal_sequence,
        domain_event_sequence: input.domain_event_sequence,
        screen_epoch: input.screen_epoch,
        cols: Some(input.cols),
        rows: Some(input.rows),
        snapshot_length: input.snapshot.len(),
    })?;
    let length = u32::try_from(header.len())?.to_be_bytes();
    let obsolete = {
        let mut query = transaction.prepare(
            "SELECT id, generation, file_path FROM journal_checkpoints
             WHERE session_id = ? AND id NOT IN (
                SELECT id FROM journal_checkpoints WHERE session_id = ? AND valid = 1
                ORDER BY generation DESC LIMIT ?
             )",
        )?;
        query
            .query_map(
                params![session_id, session_id, RETAINED_GENERATIONS - 1],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut published = false;
    let commit = (|| -> Result<()> {
        create_directory(&directory)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let mut checksum = Sha256::new();
        for bytes in [MAGIC, &length, &header, input.snapshot] {
            file.write_all(bytes)?;
            checksum.update(bytes);
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &file_path)?;
        published = true;
        sync_directory(&directory)?;
        transaction.execute(
            "INSERT INTO journal_checkpoints (
                id, session_id, generation, terminal_sequence, domain_event_sequence,
                screen_epoch, file_path, checksum, created_at, valid
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
            params![
                id,
                session_id,
                generation as i64,
                input.terminal_sequence as i64,
                input.domain_event_sequence as i64,
                input.screen_epoch as i64,
                path_text,
                format!("{:x}", checksum.finalize()),
                i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?,
            ],
        )?;
        transaction.execute(
            "UPDATE journal_checkpoints SET valid = 0 WHERE session_id = ? AND valid = 1
             AND id NOT IN (
                SELECT id FROM journal_checkpoints WHERE session_id = ? AND valid = 1
                ORDER BY generation DESC LIMIT ?
             )",
            params![session_id, session_id, RETAINED_GENERATIONS],
        )?;
        transaction.commit()?;
        Ok(())
    })();
    if let Err(error) = commit {
        let _ = fs::remove_file(&temporary);
        // A later writer may reuse this generation after a rollback. Leave a
        // published but unindexed file for its atomic rename to replace.
        return Err(error).context(if published {
            "index published terminal checkpoint"
        } else {
            "publish terminal checkpoint"
        });
    }
    for (old_id, old_generation, old_path) in obsolete {
        if let Ok(generation) = u64::try_from(old_generation)
            && directory.join(checkpoint_filename(generation)) == Path::new(&old_path)
            && let Err(error) = fs::remove_file(&old_path)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("Remove obsolete terminal checkpoint: {error}");
            continue;
        }
        if let Err(error) = connection.execute(
            "DELETE FROM journal_checkpoints WHERE id = ? AND valid = 0",
            [&old_id],
        ) {
            eprintln!("Remove obsolete terminal checkpoint index: {error}");
        }
    }
    Ok(CreatedCheckpoint {
        id,
        generation,
        file_path,
    })
}

pub fn protected_terminal_sequences(connection: &Connection, session_id: &str) -> Result<Vec<u64>> {
    let mut query = connection.prepare(
        "SELECT terminal_sequence FROM journal_checkpoints
         WHERE session_id = ? AND valid = 1 ORDER BY generation",
    )?;
    query
        .query_map([session_id], |row| row.get::<_, i64>(0))?
        .map(|sequence| Ok(u64::try_from(sequence?)?))
        .collect()
}

fn checkpoint_filename(generation: u64) -> String {
    format!("checkpoint-{generation:06}.bin")
}

fn create_directory(directory: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return fs::File::open(directory)?.sync_all();
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(())
    }
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
    fn writes_v4_and_retains_two_valid_generations() -> Result<()> {
        use crate::{
            hierarchy::{BootstrapWindowInput, HierarchyIds, bootstrap_window},
            storage::{CommandMetadata, configure_connection, migrate},
        };

        let root = tempfile::tempdir()?;
        let mut db = Connection::open_in_memory()?;
        configure_connection(&db)?;
        migrate(&mut db, 1)?;
        bootstrap_window(
            &mut db,
            &CommandMetadata {
                command_id: "bootstrap".into(),
                command_type: "bootstrap".into(),
                request_hash: "bootstrap".into(),
                causation_id: None,
                correlation_id: None,
            },
            &BootstrapWindowInput {
                window_id: "main".into(),
                default_root_directory: root.path().display().to_string(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &HierarchyIds {
                workspace_id: "workspace".into(),
                execution_context_id: "context".into(),
                task_id: "task".into(),
                scene_id: "scene".into(),
                root_node_id: "root".into(),
                session_id: "session".into(),
                mount_id: "mount".into(),
            },
            "Task",
            1,
        )?;
        let input = |terminal, domain, snapshot| CreateCheckpointInput {
            session_id: "session",
            terminal_sequence: terminal,
            domain_event_sequence: domain,
            screen_epoch: 4,
            cols: 24,
            rows: 5,
            snapshot,
        };
        let snapshot = b"\x1b[31mred\x1b[0m";
        let oversized = vec![0; MAX_SNAPSHOT_BYTES + 1];
        assert!(create_checkpoint(root.path(), &mut db, input(10, 5, &oversized)).is_err());
        assert!(!root.path().join("checkpoints").exists());

        let first = create_checkpoint(root.path(), &mut db, input(10, 5, snapshot))?;
        assert_eq!(first.generation, 1);
        let error = create_checkpoint(root.path(), &mut db, input(9, 5, snapshot))
            .err()
            .context("terminal watermark regression was accepted")?;
        assert!(error.to_string().contains("watermark"));
        let error = create_checkpoint(root.path(), &mut db, input(10, 4, snapshot))
            .err()
            .context("domain watermark regression was accepted")?;
        assert!(error.to_string().contains("watermark"));

        let second = create_checkpoint(root.path(), &mut db, input(11, 6, snapshot))?;
        let third = create_checkpoint(root.path(), &mut db, input(12, 6, snapshot))?;
        assert_eq!((second.generation, third.generation), (2, 3));
        assert!(!first.file_path.exists());
        assert!(second.file_path.exists());
        assert_eq!(fs::read_dir(third.file_path.parent().unwrap())?.count(), 2);
        assert_eq!(protected_terminal_sequences(&db, "session")?, vec![11, 12]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                third.file_path.metadata()?.permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                third
                    .file_path
                    .parent()
                    .unwrap()
                    .metadata()?
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }

        let candidate = db.query_row(
            "SELECT terminal_sequence, domain_event_sequence, screen_epoch, file_path, checksum
             FROM journal_checkpoints WHERE id = ? AND valid = 1",
            [&third.id],
            |row| {
                Ok(CheckpointCandidate {
                    id: third.id.clone(),
                    session_id: "session".into(),
                    generation: third.generation,
                    terminal_sequence: row.get::<_, i64>(0)? as u64,
                    domain_event_sequence: row.get::<_, i64>(1)? as u64,
                    screen_epoch: row.get::<_, i64>(2)? as u64,
                    file_path: row.get::<_, String>(3)?.into(),
                    checksum: row.get(4)?,
                })
            },
        )?;
        assert_eq!(
            (
                candidate.terminal_sequence,
                candidate.domain_event_sequence,
                candidate.screen_epoch
            ),
            (12, 6, 4)
        );
        let read = candidate.read()?;
        assert_eq!(read.grid, Some((24, 5)));
        assert_eq!(read.snapshot(), snapshot);
        let expected_header = format!(
            r#"{{"id":"{}","sessionId":"session","generation":3,"terminalSequence":12,"domainEventSequence":6,"screenEpoch":4,"cols":24,"rows":5,"snapshotLength":{}}}"#,
            third.id,
            snapshot.len(),
        );
        let mut expected = MAGIC.to_vec();
        expected.extend((expected_header.len() as u32).to_be_bytes());
        expected.extend(expected_header.as_bytes());
        expected.extend(snapshot);
        assert_eq!(fs::read(&third.file_path)?, expected);

        db.pragma_update(None, "query_only", true)?;
        assert!(create_checkpoint(root.path(), &mut db, input(13, 6, snapshot)).is_err());
        assert_eq!(fs::read_dir(third.file_path.parent().unwrap())?.count(), 2);
        Ok(())
    }

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
