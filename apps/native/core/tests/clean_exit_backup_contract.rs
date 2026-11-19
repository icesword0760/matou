use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Result;
use matou_native_core::{
    backup::{
        BackupDescriptor, BackupReason, BackupService, CLEAN_EXIT_BACKUP_TIMEOUT,
        DEFAULT_MAX_BACKUPS,
    },
    storage::configure_connection,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

fn source(root: &Path) -> Result<Connection> {
    let source = Connection::open(root.join("source.sqlite"))?;
    configure_connection(&source)?;
    source.execute_batch("CREATE TABLE marker(value TEXT NOT NULL) STRICT;")?;
    source.execute("INSERT INTO marker VALUES (?)", ["CLEAN_EXIT_已提交"])?;
    assert!(fs::metadata(root.join("source.sqlite-wal"))?.len() > 0);
    Ok(source)
}

fn names(directory: &Path) -> std::io::Result<BTreeSet<OsString>> {
    fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect()
}

fn backup_names(backup: &BackupDescriptor) -> [OsString; 2] {
    [
        format!("{}.sqlite", backup.id).into(),
        format!("{}.json", backup.id).into(),
    ]
}

#[test]
fn clean_exit_backs_up_committed_wal_rotates_seven_and_keeps_source_open() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = source(root.path())?;
    let mut backups = BackupService::new(root.path())?;
    for now in 1..=8 {
        backups.create(&source, BackupReason::PreMigration, now)?;
    }
    let descriptor = backups.backup_for_clean_exit(&source, 9, CLEAN_EXIT_BACKUP_TIMEOUT)?;
    assert_eq!(descriptor.reason, BackupReason::CleanExit);
    let valid = backups.list_valid()?;
    assert_eq!(valid.len(), DEFAULT_MAX_BACKUPS);
    assert_eq!(valid[0], descriptor);
    assert_eq!(valid.last().unwrap().created_at, 3);
    assert_eq!(CLEAN_EXIT_BACKUP_TIMEOUT, Duration::from_millis(5_000));

    let bytes = fs::read(&descriptor.path)?;
    assert_eq!(descriptor.size, bytes.len() as u64);
    assert_eq!(descriptor.sha256, format!("{:x}", Sha256::digest(&bytes)));
    let snapshot = Connection::open_with_flags(&descriptor.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    assert_eq!(
        snapshot.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?,
        "ok"
    );
    for database in [&snapshot, &source] {
        assert_eq!(
            database.query_row("SELECT value FROM marker", [], |row| row
                .get::<_, String>(0))?,
            "CLEAN_EXIT_已提交"
        );
    }
    drop(snapshot);
    let entries = names(&root.path().join("backups"))?;
    assert_eq!(entries, valid.iter().flat_map(backup_names).collect());
    println!(
        "rotation directory ({} entries): {entries:?}",
        entries.len()
    );
    Ok(())
}

#[test]
fn expired_clean_exit_budget_returns_error_without_publication_or_source_close() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = source(root.path())?;
    let mut backups = BackupService::new(root.path())?;
    let previous = backups.create(&source, BackupReason::PreMigration, 1)?;
    let before = fs::read(&previous.path)?;
    let directory = root.path().join("backups");
    let error = backups
        .backup_for_clean_exit(&source, 2, Duration::ZERO)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("clean shutdown backup timed out after 0ms"),
        "{error:#}"
    );
    assert_eq!(backups.list_valid()?, vec![previous.clone()]);
    assert_eq!(fs::read(&previous.path)?, before);
    assert_eq!(
        source.query_row("SELECT count(*) FROM marker", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    let entries = names(&directory)?;
    assert_eq!(entries, BTreeSet::from(backup_names(&previous)));
    println!("timeout directory: {entries:?}");
    Ok(())
}

#[test]
fn clean_exit_manifest_publication_error_preserves_existing_backup_and_source() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = source(root.path())?;
    let mut backups = BackupService::new(root.path())?;
    let previous = backups.create(&source, BackupReason::PreMigration, 1)?;
    let before = fs::read(&previous.path)?;
    let directory = root.path().join("backups");
    fs::create_dir(directory.join("matou-2-clean-exit-v0.json"))?;
    backups
        .backup_for_clean_exit(&source, 2, CLEAN_EXIT_BACKUP_TIMEOUT)
        .unwrap_err();
    assert_eq!(backups.list_valid()?, vec![previous.clone()]);
    assert_eq!(fs::read(&previous.path)?, before);
    assert_eq!(
        source.query_row("SELECT count(*) FROM marker", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    assert!(directory.join("matou-2-clean-exit-v0.sqlite").is_file());
    let mut expected = BTreeSet::from(backup_names(&previous));
    expected.extend([
        OsString::from("matou-2-clean-exit-v0.sqlite"),
        OsString::from("matou-2-clean-exit-v0.json"),
    ]);
    let entries = names(&directory)?;
    assert_eq!(entries, expected);
    println!("failed manifest publication directory: {entries:?}");
    Ok(())
}

#[test]
#[ignore = "manual clean-exit timing check with a 200 MiB payload"]
fn clean_exit_large_wal_database_produces_two_files() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = source(root.path())?;
    source.execute_batch(
        "CREATE TABLE payload(data BLOB NOT NULL) STRICT;
         WITH RECURSIVE rows(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM rows WHERE n<200)
         INSERT INTO payload SELECT zeroblob(1048576) FROM rows;
         PRAGMA wal_checkpoint(TRUNCATE);
         INSERT INTO marker VALUES ('after-checkpoint');",
    )?;
    let source_size = fs::metadata(root.path().join("source.sqlite"))?.len();
    assert!(source_size >= 200 * 1024 * 1024);
    let mut backups = BackupService::new(root.path())?;
    let started = Instant::now();
    let backup = backups.backup_for_clean_exit(&source, 1, CLEAN_EXIT_BACKUP_TIMEOUT)?;
    println!(
        "source={source_size} bytes; backup_for_clean_exit={:?}",
        started.elapsed()
    );

    let directory = root.path().join("backups");
    let manifest: BackupDescriptor = serde_json::from_reader(fs::File::open(
        directory.join(format!("{}.json", backup.id)),
    )?)?;
    assert_eq!(manifest, backup);
    let bytes = fs::read(&backup.path)?;
    assert_eq!(manifest.size, bytes.len() as u64);
    assert_eq!(manifest.sha256, format!("{:x}", Sha256::digest(&bytes)));
    let snapshot = Connection::open_with_flags(&backup.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let quick_check: String = snapshot.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    assert_eq!(quick_check, "ok");
    assert_eq!(
        snapshot.query_row("SELECT count(*) FROM marker", [], |row| row
            .get::<_, i64>(0))?,
        2
    );
    drop(snapshot);
    println!(
        "quick_check={quick_check}; manifest sha256 matches: {}; SQLite header read/write versions={:?}",
        manifest.sha256,
        &bytes[18..20]
    );
    let entries = names(&directory)?;
    for entry in &entries {
        println!(
            "{} {} bytes",
            entry.to_string_lossy(),
            fs::metadata(directory.join(entry))?.len()
        );
    }
    assert_eq!(entries, BTreeSet::from(backup_names(&backup)));
    Ok(())
}
