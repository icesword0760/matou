use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::{
    backup::{BackupReason, BackupService},
    ownership::DatabaseOwner,
    startup::{self, BootstrapResult, ReadOnlyReason, RecoveryRequired},
    storage::{configure_connection, migrate, supported_schema_version},
};
use rusqlite::Connection;

const DATABASE: &str = "matou.sqlite";
const OWNER: &str = "matou.sqlite.owner";
const MARKER: &str = "matou.sqlite.recovery.json";
const CHILD_ENV: &str = "MATOU_STARTUP_TEST_WAL_ROOT";

fn recovery(root: &Path, generation: &str) -> Result<RecoveryRequired> {
    let BootstrapResult::RecoveryRequired(state) = startup::open(root, generation)? else {
        anyhow::bail!("expected startup to require recovery");
    };
    assert_eq!(state.durable_database_path, root.join(DATABASE));
    assert_eq!(state.marker_path, root.join(MARKER));
    ensure!(!state.recovery_id.is_empty(), "recovery id is absent");
    Ok(state)
}

fn sentinel_database(root: &Path) -> Result<Connection> {
    let mut db = Connection::open(root.join(DATABASE))?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    db.execute(
        "INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
         VALUES ('sentinel', 'Original Workspace', ?, 1, 1)",
        [root.to_str().context("disposable root is not UTF-8")?],
    )?;
    Ok(db)
}

fn check_read_only_workspace(connection: &Connection, expected_version: i64) -> Result<()> {
    let name: String = connection.query_row(
        "SELECT name FROM workspaces WHERE id = 'sentinel'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(name, "Original Workspace");
    assert_eq!(
        connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
            .get::<_, i64>(
            0
        ))?,
        expected_version
    );
    assert!(connection.is_readonly(rusqlite::MAIN_DB)?);
    assert!(connection.query_row("PRAGMA query_only", [], |row| row.get::<_, bool>(0))?);
    assert!(
        connection
            .execute(
                "UPDATE workspaces SET name = 'Changed' WHERE id = 'sentinel'",
                []
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn first_startup_keeps_a_pre_migration_backup_and_reopen_does_not_add_one() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let BootstrapResult::Writable {
        connection,
        ownership,
        data_root,
    } = startup::open(root, "first")?
    else {
        anyhow::bail!("first startup must be writable");
    };
    assert_eq!(data_root, root);
    assert!(root.join(OWNER).exists());
    let version = supported_schema_version()?;
    assert_eq!(
        connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
            .get::<_, i64>(
            0
        ))?,
        version
    );
    assert_eq!(
        connection.query_row(
            "SELECT value FROM _runtime_meta WHERE key = 'runtime_generation'",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "first"
    );
    let service = BackupService::new(root)?;
    let first = service.list_valid()?;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].reason, BackupReason::PreMigration);
    assert_eq!(first[0].schema_version, 0);
    let saved = startup::open_read_only(&first[0].path)?;
    assert!(!saved.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'schema_migrations')",
        [],
        |row| row.get::<_, bool>(0),
    )?);
    drop(saved);

    drop(connection);
    drop(ownership);
    assert!(!root.join(OWNER).exists());
    let BootstrapResult::Writable {
        connection,
        ownership,
        ..
    } = startup::open(root, "second")?
    else {
        anyhow::bail!("reopen must remain writable");
    };
    assert_eq!(
        connection.query_row(
            "SELECT value FROM _runtime_meta WHERE key = 'runtime_generation'",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "second"
    );
    assert_eq!(service.list_valid()?, first);
    drop(connection);
    drop(ownership);
    Ok(())
}

#[test]
fn newer_schema_stays_in_place_and_cannot_be_mutated() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let initial = sentinel_database(root)?;
    initial.execute(
        "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES
         (999, 'future-version', 'future-checksum', 2)",
        [],
    )?;
    drop(initial);
    let original = fs::read(root.join(DATABASE))?;
    let BootstrapResult::ReadOnly {
        connection,
        data_root,
        reason,
    } = startup::open(root, "newer")?
    else {
        anyhow::bail!("newer schema must open read-only");
    };
    assert_eq!(reason, ReadOnlyReason::NewerSchema);
    assert_eq!(data_root, root);
    check_read_only_workspace(&connection, 999)?;
    drop(connection);
    assert_eq!(fs::read(root.join(DATABASE))?, original);
    assert!(!root.join(OWNER).exists());
    assert!(!root.join(MARKER).exists());
    assert!(BackupService::new(root)?.list_valid()?.is_empty());
    Ok(())
}

#[cfg(target_os = "macos")]
struct PermissionGuard(Vec<(PathBuf, fs::Permissions)>);

#[cfg(target_os = "macos")]
impl PermissionGuard {
    fn protect(entries: &[(&Path, u32)]) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let mut guard = Self(Vec::new());
        for (path, mode) in entries {
            guard
                .0
                .push((path.to_path_buf(), fs::metadata(path)?.permissions()));
            fs::set_permissions(path, fs::Permissions::from_mode(*mode))?;
        }
        Ok(guard)
    }
}

#[cfg(target_os = "macos")]
impl Drop for PermissionGuard {
    fn drop(&mut self) {
        for (path, permissions) in self.0.drain(..).rev() {
            if let Err(error) = fs::set_permissions(&path, permissions) {
                eprintln!(
                    "restore disposable test permissions {}: {error}",
                    path.display()
                );
            }
        }
    }
}

#[test]
#[cfg(target_os = "macos")]
fn chmod_read_only_database_remains_in_place() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let path = root.join(DATABASE);
    drop(sentinel_database(root)?);
    let original = fs::read(&path)?;
    let _permissions = PermissionGuard::protect(&[(&path, 0o444), (root, 0o555)])?;
    let BootstrapResult::ReadOnly {
        connection,
        data_root,
        reason,
    } = startup::open(root, "readonly")?
    else {
        anyhow::bail!("write-protected database must open read-only");
    };
    assert_eq!(reason, ReadOnlyReason::FilesystemReadOnly);
    assert_eq!(data_root, root);
    check_read_only_workspace(&connection, supported_schema_version()?)?;
    drop(connection);
    assert_eq!(fs::read(&path)?, original);
    assert!(!root.join(OWNER).exists());
    assert!(!root.join(MARKER).exists());
    Ok(())
}

#[test]
fn a_live_owner_blocks_inspection_and_quarantine() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let path = root.join(DATABASE);
    let damaged = b"live runtime owns this corrupt database".repeat(16);
    fs::write(&path, &damaged)?;
    let mut owner = DatabaseOwner::acquire(&path, "live-owner")?;
    let original_owner = fs::read(root.join(OWNER))?;

    let check = || -> Result<()> {
        let Err(error) = startup::open(root, "contender") else {
            anyhow::bail!("startup accepted a database with another live owner");
        };
        assert!(
            format!("{error:#}").contains("database is already owned by a live Runtime"),
            "{error:#}"
        );
        assert_eq!(fs::read(&path)?, damaged);
        assert_eq!(fs::read(root.join(OWNER))?, original_owner);
        assert!(!root.join(MARKER).exists());
        assert!(!fs::read_dir(root)?.any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".corrupt-")
        }));
        Ok(())
    };
    check()?;
    #[cfg(target_os = "macos")]
    {
        let _permissions = PermissionGuard::protect(&[(&path, 0o444), (root, 0o555)])?;
        check()?;
    }
    owner.release()?;
    Ok(())
}

#[test]
fn physical_corruption_is_quarantined_once_and_backup_choices_stay_available() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let initial = sentinel_database(root)?;
    let good = BackupService::new(root)?.create(&initial, BackupReason::CleanExit, 10)?;
    drop(initial);
    let damaged = b"damaged data must be preserved".repeat(16);
    fs::write(root.join(DATABASE), &damaged)?;
    let state = recovery(root, "quarantine")?;
    assert_eq!(state.reason, "physical-corruption");
    assert_eq!(state.backups, vec![good]);
    assert!(state.marker_error.is_none() && state.move_error.is_none());
    assert!(!root.join(DATABASE).exists());
    assert!(!root.join(OWNER).exists());
    assert_eq!(fs::read(&state.quarantined_path)?, damaged);
    let marker = fs::read(root.join(MARKER))?;
    assert_eq!(recovery(root, "reopen-quarantine")?, state);
    assert_eq!(fs::read(root.join(MARKER))?, marker);
    assert!(!root.join(DATABASE).exists());
    Ok(())
}

#[test]
fn broken_backup_directory_does_not_replace_a_quarantined_database() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let damaged = b"corrupt database and invalid backup directory".repeat(16);
    fs::write(root.join(DATABASE), &damaged)?;
    fs::write(root.join("backups"), b"intentionally not a directory")?;
    let state = recovery(root, "failed-backup-list")?;
    assert_eq!(state.reason, "physical-corruption");
    assert!(state.backups.is_empty());
    let failure = state
        .backup_list_error
        .as_ref()
        .context("no backup list error")?;
    assert_eq!(failure.code, "BACKUP_LIST_FAILED");
    assert!(failure.retryable);
    assert_eq!(fs::read(&state.quarantined_path)?, damaged);
    assert_eq!(recovery(root, "reopen-backup-list")?, state);
    assert!(!root.join(DATABASE).exists());
    Ok(())
}

#[test]
fn malformed_owner_recovers_in_place_and_preserves_database_bytes() -> Result<()> {
    for malformed in [Some(b"".as_slice()), Some(b"{\"pid\":".as_slice()), None] {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        drop(sentinel_database(root)?);
        let path = root.join(DATABASE);
        let original = fs::read(&path)?;
        if let Some(bytes) = malformed {
            fs::write(root.join(OWNER), bytes)?;
        } else {
            fs::create_dir(root.join(OWNER))?;
        }
        let state = recovery(root, "malformed-owner")?;
        assert_eq!(state.reason, "ownership-recovery-required");
        assert_eq!(
            state.ownership_issue.as_deref(),
            Some("owner-record-malformed")
        );
        assert_eq!(state.quarantined_path, path);
        assert_eq!(recovery(root, "reopen-malformed-owner")?, state);
        assert_eq!(fs::read(&path)?, original);
        assert!(!fs::read_dir(root)?.any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".corrupt-")
        }));
    }
    Ok(())
}

#[test]
#[ignore = "run as a helper by the startup WAL tests"]
fn crashed_wal_writer_process() -> Result<()> {
    let root = PathBuf::from(std::env::var_os(CHILD_ENV).context("missing WAL test root")?);
    let connection = Connection::open(root.join(DATABASE))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA wal_autocheckpoint = 0;
         CREATE TABLE wal_values(value TEXT NOT NULL);
         INSERT INTO wal_values VALUES ('base');
         PRAGMA wal_checkpoint(TRUNCATE);
         INSERT INTO wal_values VALUES ('committed-in-wal');",
    )?;
    fs::write(root.join("writer-ready"), b"ready")?;
    // The parent kills this process so SQLite never checkpoints on close.
    loop {
        thread::park();
    }
}

struct ChildCleanup(Child);

impl Drop for ChildCleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn crash_wal_writer(root: &Path) -> Result<()> {
    let mut child = ChildCleanup(
        Command::new(std::env::current_exe()?)
            .args(["--ignored", "--exact", "crashed_wal_writer_process"])
            .env(CHILD_ENV, root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.join("writer-ready").is_file() {
        ensure!(
            child.0.try_wait()?.is_none(),
            "WAL fixture writer exited prematurely"
        );
        ensure!(
            Instant::now() < deadline,
            "WAL fixture writer did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }
    child.0.kill()?;
    assert!(!child.0.wait()?.success());
    assert!(fs::metadata(root.join("matou.sqlite-wal"))?.len() > 32);
    assert!(fs::metadata(root.join("matou.sqlite-shm"))?.len() >= 32_768);
    Ok(())
}

#[test]
fn valid_wal_bundle_exposes_the_uncheckpointed_commit() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    crash_wal_writer(root)?;
    #[cfg(target_os = "macos")]
    let _permissions = PermissionGuard::protect(&[
        (&root.join(DATABASE), 0o444),
        (&root.join("matou.sqlite-wal"), 0o444),
        (&root.join("matou.sqlite-shm"), 0o444),
        (root, 0o555),
    ])?;
    let outcome = startup::open(root, "valid-wal")?;
    let connection = match &outcome {
        BootstrapResult::ReadOnly {
            connection, reason, ..
        } => {
            assert_eq!(*reason, ReadOnlyReason::FilesystemReadOnly);
            connection
        }
        #[cfg(not(target_os = "macos"))]
        BootstrapResult::Writable { connection, .. } => connection,
        _ => anyhow::bail!("cannot open healthy committed WAL bundle"),
    };
    let mut query = connection.prepare("SELECT value FROM wal_values ORDER BY rowid")?;
    let values = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert_eq!(values, ["base", "committed-in-wal"]);
    Ok(())
}

#[test]
fn missing_truncated_or_corrupt_shm_preserves_committed_wal_for_recovery() -> Result<()> {
    for broken in [None, Some(vec![0; 16]), Some(vec![0; 32_768])] {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        crash_wal_writer(root)?;
        let database = root.join(DATABASE);
        let wal = root.join("matou.sqlite-wal");
        let shm = root.join("matou.sqlite-shm");
        let main_bytes = fs::read(&database)?;
        let wal_bytes = fs::read(&wal)?;
        if let Some(bytes) = &broken {
            fs::write(&shm, bytes)?;
        } else {
            fs::remove_file(&shm)?;
        }
        let state = recovery(root, "broken-wal")?;
        assert_eq!(state.reason, "wal-recovery-required");
        assert!(state.move_error.is_none());
        assert_eq!(fs::read(&state.quarantined_path)?, main_bytes);
        let suffix = |suffix: &str| {
            let mut path = state.quarantined_path.as_os_str().to_owned();
            path.push(suffix);
            PathBuf::from(path)
        };
        assert_eq!(fs::read(suffix("-wal"))?, wal_bytes);
        if let Some(bytes) = broken {
            assert_eq!(fs::read(suffix("-shm"))?, bytes);
        } else {
            assert!(!suffix("-shm").exists());
        }
        assert!(!database.exists() && !wal.exists() && !shm.exists());
        assert_eq!(recovery(root, "repeat-broken-wal")?, state);
        assert!(!database.exists());
    }
    Ok(())
}

// Called by the shared startup gate driver with a disposable database copy.
#[test]
#[ignore = "fixture-driven probe; run through K-storage-recovery/run.sh"]
fn startup_recovery_gate_probe() -> Result<()> {
    use matou_native_core::recovery_marker;
    use rusqlite::OpenFlags;
    use serde_json::{Value, json};

    fn capture(root: &Path, action: &str, output: &Path, id: &str) -> Result<Value> {
        let path = root.join(DATABASE);
        let marker = root.join(MARKER);
        if action == "quick-check" {
            let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            startup::assert_startup_integrity(&db)?;
            return Ok(json!({"kind": "integrity-ok"}));
        }
        if action == "retry-open" {
            // The core exposes the marker primitive. The native runtime does
            // not expose the frozen recovery-page retry action.
            return Ok(json!({"kind": "unsupported"}));
        }
        if action == "resolve" {
            let was_resolved = recovery_marker::is_resolved(&marker, id)?;
            recovery_marker::resolve(
                &marker,
                id,
                456,
                "数据库恢复状态已更新，请使用最新恢复页面重试",
            )?;
            return Ok(json!({
                "kind": "resolved", "wasResolved": was_resolved,
                "resolved": recovery_marker::is_resolved(&marker, id)?,
            }));
        }

        fn rows(connection: &Connection) -> Result<Vec<Value>> {
            let mut statement =
                connection.prepare("SELECT value FROM gate_payload ORDER BY rowid")?;
            Ok(statement
                .query_map([], |row| Ok(json!({"value": row.get::<_, String>(0)?})))?
                .collect::<rusqlite::Result<_>>()?)
        }

        Ok(
            match startup::open(root, &uuid::Uuid::new_v4().to_string())? {
                BootstrapResult::RecoveryRequired(recovery) => {
                    let mut value = serde_json::to_value(recovery)?;
                    value["kind"] = json!("recovery-required");
                    value
                }
                BootstrapResult::ReadOnly {
                    connection,
                    data_root,
                    reason,
                } => {
                    let write_error = connection
                        .execute("UPDATE gate_payload SET value = value", [])
                        .expect_err("read-only fixture unexpectedly accepted a write");
                    json!({
                        "kind": "read-only", "dataRoot": data_root, "reason": reason, "rows": rows(&connection)?,
                        "writeError": write_error.to_string(), "writeSqliteCode": write_error.sqlite_error().map(|error| error.extended_code),
                    })
                }
                BootstrapResult::Writable {
                    connection,
                    ownership,
                    data_root,
                } => {
                    let read = rows(&connection)?;
                    if action == "hold" {
                        connection.execute_batch(
                            "PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE);",
                        )?;
                        connection.execute(
                            "INSERT INTO gate_payload VALUES (?1)",
                            ["committed-in-wal"],
                        )?;
                        write_gate_output(output, &json!({"kind": "holding", "rows": read}))?;
                        loop {
                            thread::park();
                        }
                    }
                    if action == "clean-close" {
                        let backup_time = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_millis()
                            .try_into()?;
                        BackupService::new(root)?.backup_for_clean_exit(
                            &connection,
                            backup_time,
                            matou_native_core::backup::CLEAN_EXIT_BACKUP_TIMEOUT,
                        )?;
                    }
                    drop(connection);
                    drop(ownership);
                    json!({"kind": "writable", "dataRoot": data_root, "rows": read})
                }
            },
        )
    }

    fn write_gate_output(output: &Path, value: &Value) -> Result<()> {
        let staging = output.with_extension("partial");
        fs::write(&staging, serde_json::to_vec(value)?)?;
        fs::rename(staging, output)?;
        Ok(())
    }

    let root =
        PathBuf::from(std::env::var_os("MATOU_RECOVERY_GATE_ROOT").context("missing gate root")?);
    let output = PathBuf::from(
        std::env::var_os("MATOU_RECOVERY_GATE_OUTPUT").context("missing gate output")?,
    );
    let action = std::env::var("MATOU_RECOVERY_GATE_ACTION")?;
    let id = std::env::var("MATOU_RECOVERY_GATE_ID")?;
    let value = capture(&root, &action, &output, &id).unwrap_or_else(|error| {
        let sqlite_code = error
            .downcast_ref::<rusqlite::Error>()
            .and_then(|error| error.sqlite_error())
            .map(|error| error.extended_code);
        let io_code = error
            .downcast_ref::<std::io::Error>()
            .and_then(|error| error.raw_os_error());
        json!({"kind": "error", "error": {
            "message": error.to_string(), "chain": format!("{error:#}"),
            "sqliteCode": sqlite_code, "osCode": io_code,
        }})
    });
    write_gate_output(&output, &value)
}
