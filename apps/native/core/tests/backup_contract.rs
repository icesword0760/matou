use std::{collections::BTreeSet, ffi::OsString, fs, path::Path};

use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    backup::{BackupDescriptor, BackupReason, BackupService, DEFAULT_MAX_BACKUPS},
    storage::configure_connection,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REFERENCE: &str = include_str!("fixtures/backup-reference.json");

fn read_snapshot(path: &Path, table: &str) -> Result<Value> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection.prepare(&format!("SELECT value FROM {table} ORDER BY rowid"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| row.map(|value| json!({ "value": value })))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut integrity = connection.prepare("PRAGMA integrity_check")?;
    let integrity = integrity
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| row.map(|value| json!({ "integrity_check": value })))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({ "rows": rows, "integrity": integrity }))
}

fn seed_wal(path: &Path, table: &str, value: &str) -> Result<Connection> {
    let connection = Connection::open(path)?;
    configure_connection(&connection)?;
    connection.execute_batch(&format!("CREATE TABLE {table}(value TEXT NOT NULL) STRICT"))?;
    connection.execute(&format!("INSERT INTO {table} VALUES (?)"), [value])?;
    Ok(connection)
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn online_wal_backup_and_frozen_source_backup_are_independently_restorable() -> Result<()> {
    let reference: Value = serde_json::from_str(REFERENCE)?;
    let expected = &reference["wal"];
    let root = tempfile::tempdir()?;
    let native_root = root.path().join("native");
    fs::create_dir(&native_root)?;
    let source = seed_wal(
        &native_root.join("source.sqlite"),
        "restored_rows",
        "from-backup",
    )?;
    assert!(fs::metadata(native_root.join("source.sqlite-wal"))?.len() > 0);
    let mut native = BackupService::new(&native_root)?;
    assert!(native.list_valid()?.is_empty());
    let produced = native.create(&source, BackupReason::CleanExit, 2_000)?;
    let published = fs::read(&produced.path)?;
    let source_info: BackupDescriptor = serde_json::from_value(expected["descriptor"].clone())?;
    assert_eq!(
        (
            produced.id.as_str(),
            produced.created_at,
            produced.reason,
            produced.schema_version
        ),
        (
            source_info.id.as_str(),
            source_info.created_at,
            source_info.reason,
            source_info.schema_version
        )
    );
    assert_eq!(produced.size, published.len() as u64);
    assert_eq!(produced.sha256, checksum(&published));
    let manifest = native_root
        .join("backups")
        .join(format!("{}.json", produced.id));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&manifest)?)?,
        serde_json::to_value(&produced)?
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&manifest)?.permissions().mode() & 0o777, 0o600);
    }
    let snapshot = json!({ "rows": expected["rows"], "integrity": expected["integrity"] });
    assert_eq!(read_snapshot(&produced.path, "restored_rows")?, snapshot);
    assert_eq!(native.list_valid()?, vec![produced.clone()]);
    let entries = fs::read_dir(native_root.join("backups"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    assert_eq!(
        entries,
        BTreeSet::from([
            OsString::from(format!("{}.sqlite", produced.id)),
            OsString::from(format!("{}.json", produced.id)),
        ])
    );
    println!("unbudgeted backup directory: {entries:?}");

    let frozen_root = root.path().join("reference");
    let frozen_dir = frozen_root.join("backups");
    fs::create_dir_all(&frozen_dir)?;
    let mut imported = source_info;
    imported.path = frozen_dir.join(format!("{}.sqlite", imported.id));
    let frozen_bytes = STANDARD.decode(expected["bytesBase64"].as_str().unwrap())?;
    assert_eq!(frozen_bytes.len() as u64, imported.size);
    assert_eq!(checksum(&frozen_bytes), imported.sha256);
    fs::write(&imported.path, &frozen_bytes)?;
    fs::write(
        frozen_dir.join(format!("{}.json", imported.id)),
        serde_json::to_vec(&imported)?,
    )?;
    let mut service = BackupService::new(&frozen_root)?;
    assert_eq!(service.list_valid()?, vec![imported.clone()]);
    let target = frozen_root.join("restored.sqlite");
    service.restore(&imported.id, &target, 3_000)?;
    assert_eq!(read_snapshot(&target, "restored_rows")?, snapshot);
    assert_eq!(fs::read(&imported.path)?, frozen_bytes);
    Ok(())
}

#[test]
fn rotation_keeps_seven_valid_backups_and_leaves_invalid_artifacts() -> Result<()> {
    let reference: Value = serde_json::from_str(REFERENCE)?;
    let root = tempfile::tempdir()?;
    let source = seed_wal(&root.path().join("source.sqlite"), "rotated", "0")?;
    let mut service = BackupService::new(root.path())?;
    let mut created = Vec::new();
    for version in 0..9 {
        source.execute("UPDATE rotated SET value = ?", [version.to_string()])?;
        let backup = service.create(&source, BackupReason::CleanExit, 2_000)?;
        created.push(backup);
    }
    let projected = created
        .iter()
        .map(|backup| {
            json!({
                "id": &backup.id,
                "createdAt": backup.created_at,
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(json!(projected), reference["rotation"]["created"]);
    let bytes = fs::read(&created[0].path)?;
    let backup_dir = root.path().join("backups");
    let mut invalid = Vec::new();
    for (index, kind) in ["checksum", "integrity", "path", "manifest"]
        .into_iter()
        .enumerate()
    {
        let now = 9_001 + index as i64;
        let id = format!("matou-{now}-clean-exit-v0");
        let data = if kind == "integrity" {
            b"not SQLite".as_slice()
        } else {
            &bytes
        };
        let path = backup_dir.join(format!("{id}.sqlite"));
        let manifest = backup_dir.join(format!("{id}.json"));
        fs::write(&path, data)?;
        let descriptor = BackupDescriptor {
            id,
            created_at: now,
            size: data.len() as u64,
            sha256: if kind == "checksum" {
                "0".repeat(64)
            } else {
                checksum(data)
            },
            path: if kind == "path" {
                backup_dir.join(".").join(path.file_name().unwrap())
            } else {
                path.clone()
            },
            ..created[0].clone()
        };
        let manifest_bytes = if kind == "manifest" {
            b"incomplete {".to_vec()
        } else {
            serde_json::to_vec(&descriptor)?
        };
        fs::write(&manifest, manifest_bytes)?;
        invalid.extend([path, manifest]);
    }
    let partial = backup_dir.join("ignored.sqlite.partial");
    fs::write(&partial, b"partial")?;
    invalid.push(partial);
    assert_eq!(service.list_valid()?.len(), 9);

    service.rotate(DEFAULT_MAX_BACKUPS)?;
    let actual = service
        .list_valid()?
        .into_iter()
        .map(|backup| backup.id)
        .collect::<Vec<_>>();
    assert_eq!(json!(actual), reference["rotation"]["retained"]);
    for entry in invalid {
        assert!(entry.exists(), "rotation touched {}", entry.display());
    }
    for expired in &created[..2] {
        assert!(!expired.path.exists());
        assert!(!backup_dir.join(format!("{}.json", expired.id)).exists());
    }
    Ok(())
}

#[test]
#[ignore = "run as a child by the restore WAL test"]
fn write_restore_target_wal() -> Result<()> {
    let target =
        std::env::var_os("MATOU_BACKUP_TEST_WAL_TARGET").expect("missing restore WAL fixture path");
    let _connection = seed_wal(Path::new(&target), "original_rows", "committed-old-wal")?;
    // Exit without running SQLite's close-time checkpoint.
    std::process::exit(0);
}

#[test]
fn restore_keeps_committed_old_wal_in_rollback_and_removes_obsolete_sidecars() -> Result<()> {
    let reference: Value = serde_json::from_str(REFERENCE)?;
    let root = tempfile::tempdir()?;
    let source = seed_wal(
        &root.path().join("source.sqlite"),
        "restored_rows",
        "from-backup",
    )?;
    let mut service = BackupService::new(root.path())?;
    let descriptor = service.create(&source, BackupReason::CleanExit, 2_000)?;
    let source_bytes = fs::read(&descriptor.path)?;
    let target = root.path().join("target.sqlite");
    let child = std::process::Command::new(std::env::current_exe()?)
        .args(["--ignored", "--exact", "write_restore_target_wal"])
        .env("MATOU_BACKUP_TEST_WAL_TARGET", &target)
        .output()?;
    assert!(child.status.success(), "{child:?}");
    let old_wal = root.path().join("target.sqlite-wal");
    let old_shm = root.path().join("target.sqlite-shm");
    assert!(fs::metadata(&old_wal)?.len() > 0 && old_shm.is_file());

    service.restore(&descriptor.id, &target, 2_000)?;
    // Opening a restored SQLite WAL-mode database may create new sidecars.
    let sidecars: Vec<&str> = [&old_shm, &old_wal]
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(json!(sidecars), reference["restored"]["sidecarsPresent"]);
    assert_eq!(
        read_snapshot(&target, "restored_rows")?,
        reference["restored"]["target"]
    );
    assert_eq!(
        read_snapshot(
            &root.path().join("target.sqlite.replaced-2001"),
            "original_rows"
        )?,
        reference["restored"]["rollback"]
    );
    assert_eq!(fs::read(&descriptor.path)?, source_bytes);
    Ok(())
}

#[test]
fn failed_manifest_publication_removes_the_unpaired_database_and_allows_retry() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = seed_wal(
        &root.path().join("source.sqlite"),
        "restored_rows",
        "from-backup",
    )?;
    source.execute_batch("CREATE TABLE schema_migrations(version INTEGER NOT NULL); INSERT INTO schema_migrations VALUES (5);")?;
    let mut service = BackupService::new(root.path())?;
    let backup_dir = root.path().join("backups");
    fs::create_dir_all(backup_dir.join("matou-1000-pre-migration-v5.json"))?;

    assert!(
        service
            .create(&source, BackupReason::PreMigration, 1_000)
            .is_err()
    );
    assert!(service.list_valid()?.is_empty());
    assert!(
        !backup_dir
            .join("matou-1000-pre-migration-v5.sqlite")
            .exists()
    );
    assert!(
        !backup_dir
            .join("matou-1000-pre-migration-v5.sqlite.partial")
            .exists()
    );
    assert!(
        !backup_dir
            .join("matou-1000-pre-migration-v5.json.partial")
            .exists()
    );
    let next = service.create(&source, BackupReason::PreMigration, 1_000)?;
    assert_eq!(next.id, "matou-1001-pre-migration-v5");
    assert_eq!(
        read_snapshot(&next.path, "restored_rows")?["rows"],
        json!([{"value": "from-backup"}])
    );
    assert_eq!(service.list_valid()?, vec![next]);
    Ok(())
}
