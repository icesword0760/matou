use std::{collections::BTreeSet, fs, time::Duration};

use anyhow::Result;
use matou_native_core::{
    backup::{BackupReason, BackupService, CLEAN_EXIT_BACKUP_TIMEOUT, DEFAULT_MAX_BACKUPS},
    storage::configure_connection,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

fn source(root: &std::path::Path) -> Result<Connection> {
    let source = Connection::open(root.join("source.sqlite"))?;
    configure_connection(&source)?;
    source.execute_batch("CREATE TABLE marker(value TEXT NOT NULL) STRICT;")?;
    source.execute("INSERT INTO marker VALUES (?)", ["CLEAN_EXIT_已提交"])?;
    assert!(fs::metadata(root.join("source.sqlite-wal"))?.len() > 0);
    Ok(source)
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
    Ok(())
}

#[test]
fn expired_clean_exit_budget_returns_error_without_publication_or_source_close() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = source(root.path())?;
    let mut backups = BackupService::new(root.path())?;
    let previous = backups.create(&source, BackupReason::PreMigration, 1)?;
    let before = fs::read(&previous.path)?;
    let names = || -> std::io::Result<BTreeSet<_>> {
        fs::read_dir(root.path().join("backups"))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    };
    let before_names = names()?;
    let error = backups
        .backup_for_clean_exit(&source, 2, Duration::ZERO)
        .unwrap_err();
    assert_eq!(
        names()?,
        before_names,
        "expired call changed backup artifacts"
    );
    assert!(
        error
            .to_string()
            .contains("clean shutdown backup timed out after 0ms"),
        "{error:#}"
    );
    assert_eq!(backups.list_valid()?, vec![previous.clone()]);
    assert_eq!(fs::read(previous.path)?, before);
    assert_eq!(
        source.query_row("SELECT count(*) FROM marker", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
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
    assert_eq!(fs::read(previous.path)?, before);
    assert_eq!(
        source.query_row("SELECT count(*) FROM marker", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    assert!(directory.join("matou-2-clean-exit-v0.sqlite").is_file());
    for entry in fs::read_dir(directory)? {
        assert!(!entry?.file_name().to_string_lossy().ends_with(".partial"));
    }
    Ok(())
}
