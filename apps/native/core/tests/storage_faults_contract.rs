use std::fs;

use anyhow::Result;
use matou_native_core::{
    startup::{assert_startup_integrity, open_read_only},
    storage::{configure_connection, migrate},
};
use rusqlite::{Connection, params};
use serde::Deserialize;

const DATABASE: &str = "matou.sqlite";
const WAL: &str = "matou.sqlite-wal";
const SEED_AT: i64 = 1_000;
const UPGRADE_AT: i64 = 2_000;

#[derive(Deserialize)]
struct Migration {
    version: i64,
    name: String,
    checksum: String,
    sql: String,
}

fn seed_schema(db: &Connection, through: i64) -> Result<()> {
    configure_connection(db)?;
    let catalog: Vec<Migration> = serde_json::from_str(include_str!("../schema/migrations.json"))?;
    db.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL,
                       checksum TEXT NOT NULL, applied_at INTEGER NOT NULL) STRICT;",
    )?;
    for migration in catalog
        .iter()
        .take_while(|migration| migration.version <= through)
    {
        db.execute_batch(&migration.sql)?;
        db.execute(
            "INSERT INTO schema_migrations VALUES (?, ?, ?, ?)",
            params![
                migration.version,
                migration.name,
                migration.checksum,
                SEED_AT
            ],
        )?;
    }
    db.execute_batch(
        "INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
           VALUES ('work', 'Saved Workspace', '/workspace', 1, 1);
         INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
           VALUES ('context', 'work', 'plain-directory', '/workspace', 1);
         INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at)
           VALUES ('task', 'work', 'context', 'Saved Task', 'active', 1, 1);
         INSERT INTO sessions (id, task_id, execution_context_id, kind, status, created_at, updated_at, last_activity_at)
           VALUES ('shell', 'task', 'context', 'shell', 'created', 1, 1, 1);",
    )?;
    Ok(())
}

type MigrationRow = (i64, String, String, i64);

fn history(db: &Connection) -> Result<Vec<MigrationRow>> {
    Ok(db
        .prepare(
            "SELECT version, name, checksum, applied_at FROM schema_migrations ORDER BY version",
        )?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn schema(db: &Connection) -> Result<Vec<String>> {
    Ok(db
        .prepare("SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type, name")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn assert_intact(db: &Connection) -> Result<()> {
    assert_startup_integrity(db)?;
    assert!(
        db.prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_none()
    );
    Ok(())
}

fn assert_saved_session(db: &Connection) -> Result<()> {
    assert_eq!(
        db.query_row(
            "SELECT workspaces.name, tasks.title, sessions.kind FROM sessions
             JOIN tasks ON tasks.id = sessions.task_id
             JOIN workspaces ON workspaces.id = tasks.workspace_id WHERE sessions.id = 'shell'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?,
        (
            String::from("Saved Workspace"),
            String::from("Saved Task"),
            String::from("shell")
        )
    );
    Ok(())
}

#[cfg(unix)]
mod sigkill {
    use super::*;
    use std::{
        io::{self, BufRead, BufReader, Write},
        os::unix::process::ExitStatusExt,
        path::Path,
        process::{Child, Command, Stdio},
        sync::mpsc,
        thread,
        time::Duration,
    };

    use anyhow::{Context, bail, ensure};
    use matou_native_core::{
        backup::{BackupReason, BackupService},
        storage::migrate_with_backup,
    };
    use rusqlite::hooks::Action;

    const READY: &str = "migration transaction ready for SIGKILL";

    #[test]
    #[ignore = "subprocess helper for migration SIGKILL tests"]
    fn migration_writer_process() -> Result<()> {
        let version: i64 = io::stdin()
            .lock()
            .lines()
            .next()
            .context("missing stop version")??
            .parse()?;
        ensure!(
            matches!(version, 3 | 35),
            "unexpected stop version {version}"
        );
        let root = std::env::current_dir()?;
        let mut db = Connection::open_with_flags(
            root.join(DATABASE),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        configure_connection(&db)?;
        db.execute_batch("PRAGMA wal_autocheckpoint = 0;")?;
        db.update_hook(Some(move |action, _: &str, table: &str, rowid| {
            if action == Action::SQLITE_INSERT && table == "schema_migrations" && rowid == version {
                // The migration SQL ran and SQLite inserted its history row inside
                // the same transaction. The native runner cannot reach COMMIT yet.
                let mut stdout = io::stdout().lock();
                writeln!(stdout, "\n{READY} {version}").unwrap();
                stdout.flush().unwrap();
                drop(stdout);
                loop {
                    thread::park();
                }
            }
        }))?;
        let mut backups = BackupService::new(&root)?;
        migrate_with_backup(&mut db, UPGRADE_AT, |db| {
            backups.create(db, BackupReason::PreMigration, UPGRADE_AT)?;
            Ok(())
        })?;
        bail!("migration finished without reaching stop version {version}");
    }

    struct KillOnDrop(Child);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn kill_during_migration(root: &Path, version: i64) -> Result<()> {
        let mut child = KillOnDrop(
            Command::new(std::env::current_exe()?)
                .args([
                    "--ignored",
                    "--exact",
                    "sigkill::migration_writer_process",
                    "--nocapture",
                ])
                .current_dir(root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?,
        );
        writeln!(child.0.stdin.take().unwrap(), "{version}")?;
        let stdout = child.0.stdout.take().unwrap();
        let (ready, wait) = mpsc::channel();
        let reader = thread::spawn(move || -> Result<()> {
            let expected = format!("{READY} {version}");
            for line in BufReader::new(stdout).lines() {
                if line? == expected {
                    ready.send(())?;
                    return Ok(());
                }
            }
            bail!("migration writer exited before reaching version {version}");
        });
        let reached = wait.recv_timeout(Duration::from_secs(10));
        let kill = child.0.kill();
        let status = child.0.wait()?;
        reached
            .with_context(|| format!("waiting for migration {version}; child status {status}"))?;
        reader.join().expect("read migration writer output")?;
        kill?;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        Ok(())
    }

    fn assert_migration_crash_recovers(seed: i64, interrupted: i64) -> Result<()> {
        let directory = tempfile::tempdir()?;
        // Backup descriptors store absolute paths. Match the subprocess's cwd
        // even when the platform's temporary directory uses a symlink alias.
        let root = fs::canonicalize(directory.path())?;
        let path = root.join(DATABASE);
        let db = Connection::open(&path)?;
        seed_schema(&db, seed)?;
        let seed_schema_sql = schema(&db)?;
        let seed_history = history(&db)?;
        drop(db);

        kill_during_migration(&root, interrupted)?;

        let mut db = Connection::open(&path)?;
        let committed = interrupted - 1;
        let expected = Connection::open_in_memory()?;
        seed_schema(&expected, committed)?;
        let mut expected_history = history(&expected)?;
        for row in &mut expected_history[seed_history.len()..] {
            row.3 = UPGRADE_AT;
        }
        assert_eq!(
            schema(&db)?,
            schema(&expected)?,
            "uncommitted DDL rolled back"
        );
        assert_eq!(
            history(&db)?,
            expected_history,
            "only committed history survived"
        );
        assert_saved_session(&db)?;
        assert_intact(&db)?;

        let mut backups = BackupService::new(&root)?;
        let validated = backups.list_valid()?;
        assert_eq!(validated.len(), 1);
        let original = &validated[0];
        assert_eq!(original.schema_version, seed);
        assert_eq!(original.reason, BackupReason::PreMigration);
        let backup = open_read_only(&original.path)?;
        assert_eq!(schema(&backup)?, seed_schema_sql);
        assert_eq!(history(&backup)?, seed_history);
        assert_saved_session(&backup)?;
        assert_intact(&backup)?;
        drop(backup);

        configure_connection(&db)?;
        let resumed = migrate_with_backup(&mut db, UPGRADE_AT + 1, |db| {
            backups.create(db, BackupReason::PreMigration, UPGRADE_AT + 1)?;
            Ok(())
        })?;
        assert_eq!(
            resumed.applied_versions,
            (interrupted..=36).collect::<Vec<_>>()
        );
        assert_eq!(resumed.current_version, 36);
        drop(db);

        let db = open_read_only(&path)?;
        let resumed_history = history(&db)?;
        assert_eq!(resumed_history.len(), 36);
        assert_eq!(resumed_history[..committed as usize], expected_history);
        assert!(
            resumed_history[committed as usize..]
                .iter()
                .all(|row| row.3 == UPGRADE_AT + 1)
        );
        assert_saved_session(&db)?;
        assert_intact(&db)?;
        let validated = backups.list_valid()?;
        assert_eq!(validated.len(), 2);
        assert_eq!(validated[0].schema_version, committed);
        assert_eq!(validated[1].sha256, original.sha256);
        Ok(())
    }

    #[test]
    fn sigkill_during_v3_keeps_v2_and_resumes_migration() -> Result<()> {
        assert_migration_crash_recovers(1, 3)
    }

    #[test]
    fn sigkill_during_v35_rolls_back_table_rebuild_and_resumes_migration() -> Result<()> {
        assert_migration_crash_recovers(34, 35)
    }
}

fn checkpoint_truncate(db: &Connection) -> Result<(i64, i64, i64)> {
    Ok(db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?)
}

#[test]
fn truncated_wal_frame_recovers_complete_commits() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    let damaged = directory.path().join("damaged");
    fs::create_dir(&source)?;
    fs::create_dir(&damaged)?;
    let writer = Connection::open(source.join(DATABASE))?;
    configure_connection(&writer)?;
    writer.execute_batch(
        "PRAGMA wal_autocheckpoint = 0;
         CREATE TABLE wal_values (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO wal_values VALUES (1, 'base');",
    )?;
    assert_eq!(checkpoint_truncate(&writer)?, (0, 0, 0));
    let main = fs::read(source.join(DATABASE))?;
    for (id, value) in [(2, "first"), (3, "second"), (4, "torn")] {
        // Each autocommit changes just one table leaf page.
        writer.execute("INSERT INTO wal_values VALUES (?, ?)", params![id, value])?;
    }
    assert_eq!(wal_values(&writer)?, ["base", "first", "second", "torn"]);
    assert_eq!(fs::read(source.join(DATABASE))?, main);

    let wal = fs::read(source.join(WAL))?;
    let page_size = u32::from_be_bytes(wal[8..12].try_into()?) as usize;
    let frame_size = 24 + page_size;
    assert_eq!(wal.len(), 32 + 3 * frame_size);
    for frame in wal[32..].chunks_exact(frame_size) {
        assert_ne!(&frame[4..8], &[0; 4], "frame must mark a commit");
    }
    // Clone the uncheckpointed bundle without the transient WAL index, so SQLite
    // must rebuild it from the two complete frames and the partial third frame.
    fs::write(damaged.join(DATABASE), main)?;
    let truncated = damaged.join(WAL);
    fs::write(&truncated, wal)?;
    fs::OpenOptions::new()
        .write(true)
        .open(&truncated)?
        .set_len((32 + 2 * frame_size + 24 + page_size / 2) as u64)?;
    drop(writer);

    let path = damaged.join(DATABASE);
    let recovered = Connection::open(&path)?;
    configure_connection(&recovered)?;
    assert_eq!(wal_values(&recovered)?, ["base", "first", "second"]);
    assert_intact(&recovered)?;
    recovered.execute("INSERT INTO wal_values VALUES (4, 'after recovery')", [])?;
    drop(recovered);
    let reopened = open_read_only(&path)?;
    assert_eq!(
        wal_values(&reopened)?,
        ["base", "first", "second", "after recovery"]
    );
    assert_intact(&reopened)
}

fn wal_values(db: &Connection) -> Result<Vec<String>> {
    Ok(db
        .prepare("SELECT value FROM wal_values ORDER BY id")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

#[test]
fn read_only_snapshot_survives_v35_migration_and_delays_checkpoint() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(DATABASE);
    let mut writer = Connection::open(&path)?;
    seed_schema(&writer, 34)?;
    assert_eq!(checkpoint_truncate(&writer)?, (0, 0, 0));
    let mut reader = open_read_only(&path)?;
    assert!(reader.is_readonly(rusqlite::MAIN_DB)?);
    let snapshot = reader.transaction()?;
    let old_schema = schema(&snapshot)?;
    let old_history = history(&snapshot)?;
    assert_saved_session(&snapshot)?;

    // WAL permits DDL to commit with the reader pinned, but prevents a truncate
    // checkpoint until that reader releases its old snapshot.
    writer.busy_timeout(std::time::Duration::ZERO)?;
    let report = migrate(&mut writer, UPGRADE_AT)?;
    assert_eq!(report.applied_versions, [35, 36]);
    assert_ne!(schema(&writer)?, old_schema);
    assert_eq!(schema(&snapshot)?, old_schema);
    assert_eq!(history(&snapshot)?, old_history);
    assert_saved_session(&snapshot)?;
    let (busy, frames, checkpointed) = checkpoint_truncate(&writer)?;
    assert_eq!(busy, 1);
    assert!(frames > checkpointed, "migration frames are still pinned");

    snapshot.commit()?;
    drop(reader);
    assert_eq!(checkpoint_truncate(&writer)?, (0, 0, 0));
    drop(writer);
    let mut reopened = Connection::open(&path)?;
    configure_connection(&reopened)?;
    let report = migrate(&mut reopened, UPGRADE_AT + 1)?;
    assert_eq!(report.current_version, 36);
    assert!(report.applied_versions.is_empty());
    assert_saved_session(&reopened)?;
    assert_intact(&reopened)
}
