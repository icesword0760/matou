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
    let directory = root.path().join("backups");
    let unchanged = [
        previous.path.clone(),
        previous.path.with_extension("json"),
        root.path().join("source.sqlite"),
        root.path().join("source.sqlite-wal"),
    ]
    .map(|path| Ok((fs::read(&path)?, path)))
    .into_iter()
    .collect::<std::io::Result<Vec<_>>>()?;
    fs::create_dir(directory.join("matou-2-clean-exit-v0.json"))?;
    let error = backups
        .backup_for_clean_exit(&source, 2, CLEAN_EXIT_BACKUP_TIMEOUT)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "failed to publish backup manifest at matou-2-clean-exit-v0.json"
    );
    assert_rename_os_error(&error);
    assert_eq!(backups.list_valid()?, vec![previous.clone()]);
    for (bytes, path) in unchanged {
        assert_eq!(fs::read(&path)?, bytes, "{}", path.display());
    }
    assert_eq!(
        source.query_row("SELECT count(*) FROM marker", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    let mut expected = BTreeSet::from(backup_names(&previous));
    expected.insert("matou-2-clean-exit-v0.json".into());
    let entries = names(&directory)?;
    assert_eq!(entries, expected);
    println!("failed manifest publication directory: {entries:?}");
    Ok(())
}

fn assert_rename_os_error(error: &anyhow::Error) {
    let code = error
        .downcast_ref::<std::io::Error>()
        .unwrap()
        .raw_os_error();
    #[cfg(unix)]
    assert_eq!(code, Some(libc::EISDIR));
    #[cfg(windows)]
    assert!(code.is_some());
}

#[test]
fn database_and_rollback_rename_errors_identify_the_stage_and_target() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = source(root.path())?;
    let mut backups = BackupService::new(root.path())?;
    let previous = backups.create(&source, BackupReason::PreMigration, 1)?;
    let blocked = root.path().join("backups/matou-2-clean-exit-v0.sqlite");
    fs::create_dir(&blocked)?;
    let error = backups
        .create(&source, BackupReason::CleanExit, 2)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "failed to publish backup database at matou-2-clean-exit-v0.sqlite"
    );
    assert_rename_os_error(&error);
    assert!(blocked.is_dir());

    let target = root.path().join("source.sqlite");
    fs::create_dir(root.path().join("source.sqlite.replaced-3"))?;
    let error = backups.restore(&previous.id, &target, 3).unwrap_err();
    assert_eq!(
        error.to_string(),
        "failed to publish restore rollback at source.sqlite.replaced-3"
    );
    assert_rename_os_error(&error);
    assert_eq!(backups.list_valid()?, vec![previous]);
    Ok(())
}

#[test]
fn colliding_backup_name_preserves_the_existing_open_backup() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = source(root.path())?;
    let previous = BackupService::new(root.path())?.create(&source, BackupReason::CleanExit, 1)?;
    let held = Connection::open_with_flags(&previous.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let before = fs::read(&previous.path)?;
    let manifest_before = fs::read(previous.path.with_extension("json"))?;
    source.execute("INSERT INTO marker VALUES ('later')", [])?;

    let mut restarted = BackupService::new(root.path())?;
    assert!(
        restarted
            .create(&source, BackupReason::CleanExit, 1)
            .is_err()
    );
    assert_eq!(fs::read(&previous.path)?, before);
    assert_eq!(
        fs::read(previous.path.with_extension("json"))?,
        manifest_before
    );
    assert_eq!(
        held.query_row("SELECT count(*) FROM marker", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    assert_eq!(restarted.list_valid()?, vec![previous]);
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

#[allow(unexpected_cfgs)]
#[cfg(all(unix, matou_backup_gate))]
mod gate {
    use super::*;
    use matou_native_core::startup::assert_startup_integrity;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::{os::unix::fs::PermissionsExt, path::PathBuf};

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Inputs {
        timestamp: i64,
        restore_timestamp: i64,
        budget_ms: u64,
        upper_bound_ms: u64,
        rotation_count: u64,
        seed_sql: String,
        wal_sql: String,
        sidecar_sql: String,
        invalid_sql: String,
        old_target_sql: String,
        copy_sql: String,
    }

    fn ids(service: &BackupService) -> Result<Vec<String>> {
        Ok(service
            .list_valid()?
            .into_iter()
            .map(|item| item.id)
            .collect())
    }

    fn immutable(path: &Path) -> Result<Connection> {
        let mut uri =
            url::Url::from_file_path(path).map_err(|_| anyhow::anyhow!("invalid fixture path"))?;
        uri.query_pairs_mut().append_pair("immutable", "1");
        Ok(Connection::open_with_flags(
            uri.as_str(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?)
    }

    fn strings(db: &Connection, sql: &str) -> Result<Vec<String>> {
        Ok(db
            .prepare(sql)?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    fn clean(text: &str, root: &Path) -> String {
        text.replace(root.to_str().unwrap(), "<root>")
    }

    fn error_of<T>(result: Result<T>, root: &Path) -> Option<String> {
        result.err().map(|error| clean(&format!("{error:#}"), root))
    }

    fn files(root: &Path, relative: &str, prefix: &str) -> Result<Vec<Value>> {
        let dir = root.join(relative);
        let mut result = Vec::new();
        for name in names(&dir)? {
            let name = name.to_str().unwrap();
            if !name.starts_with(prefix) {
                continue;
            }
            let path = dir.join(name);
            let metadata = path.metadata()?;
            if metadata.is_dir() {
                result.push(json!({"name": name, "kind": "directory"}));
                continue;
            }
            let mut entry = json!({
                "name": name, "kind": "file", "bytes": metadata.len(),
                "mode": format!("{:o}", metadata.permissions().mode() & 0o777),
            });
            if name.ends_with(".json") {
                let raw = fs::read_to_string(&path)?;
                let manifest: BackupDescriptor = serde_json::from_str(&raw)?;
                let bytes = fs::read(&manifest.path)?;
                entry["text"] = json!(clean(&raw, root));
                entry["sha256Matches"] =
                    json!(manifest.sha256 == format!("{:x}", Sha256::digest(&bytes)));
                entry["sizeMatches"] = json!(manifest.size == bytes.len() as u64);
            }
            if name.ends_with(".sqlite")
                || name
                    .split_once(".sqlite.replaced-")
                    .is_some_and(|(_, stamp)| stamp.bytes().all(|byte| byte.is_ascii_digit()))
            {
                let bytes = fs::read(&path)?;
                let db = immutable(&path)?;
                entry["header"] = json!(&bytes[18..20]);
                entry["integrity"] = json!(strings(&db, "PRAGMA integrity_check")?);
                entry["rows"] = json!(strings(&db, "SELECT value FROM marker ORDER BY rowid")?);
            }
            result.push(entry);
        }
        Ok(result)
    }

    fn wal_state(root: &Path) -> Result<Value> {
        let wal_bytes = match root.join("source.sqlite-wal").metadata() {
            Ok(metadata) => Some(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(json!({
            "walBytes": wal_bytes,
            "mainRows": strings(&immutable(&root.join("source.sqlite"))?, "SELECT value FROM marker ORDER BY rowid")?,
        }))
    }

    fn clean_exit_case(root: &Path, db: Connection, input: &Inputs, name: &str) -> Result<Value> {
        if name == "wal" {
            db.execute_batch(&input.wal_sql)?;
        } else if name == "sidecar" {
            db.execute_batch(&input.sidecar_sql)?;
        }
        let mut service = BackupService::new(root)?;
        let mut created = Vec::new();
        if name == "rotation" {
            for _ in 1..input.rotation_count {
                created.push(
                    service
                        .create(&db, BackupReason::PreMigration, input.timestamp)?
                        .id,
                );
            }
        }
        let before = wal_state(root)?;
        let started = Instant::now();
        let outcome =
            service.backup_for_clean_exit(&db, input.timestamp, CLEAN_EXIT_BACKUP_TIMEOUT);
        let within_bound = started.elapsed() <= CLEAN_EXIT_BACKUP_TIMEOUT;
        let after_backup = wal_state(root)?;
        db.close().map_err(|(_, error)| error)?;
        let error = match outcome {
            Ok(descriptor) => {
                created.push(descriptor.id);
                None
            }
            Err(error) => error_of::<()>(Err(error), root),
        };
        Ok(json!({
            "error": error, "withinBound": within_bound, "created": created, "retained": ids(&service)?,
            "before": before, "afterBackup": after_backup, "afterClose": wal_state(root)?,
            "files": files(root, "backups", "")?,
        }))
    }

    fn invalid(root: &Path, db: Connection, input: &Inputs) -> Result<Value> {
        let mut service = BackupService::new(root)?;
        let previous = service.create(&db, BackupReason::PreMigration, input.timestamp)?;
        let editor = Connection::open(root.join("source.sqlite"))?;
        editor.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, false)?;
        editor.execute_batch(&input.invalid_sql)?;
        editor.close().map_err(|(_, error)| error)?;
        let integrity = strings(&db, "PRAGMA integrity_check")?;
        let startup_error = error_of(assert_startup_integrity(&db), root);
        let error = error_of(
            service.backup_for_clean_exit(&db, input.timestamp, CLEAN_EXIT_BACKUP_TIMEOUT),
            root,
        );
        let file = root.join("backups").join(format!("{}.json", previous.id));
        let manifest = fs::read(&file)?;
        let mut invalid_candidates = Vec::new();
        for (field, value) in [
            ("size", json!(previous.size + 1)),
            ("sha256", json!("0".repeat(64))),
        ] {
            let mut changed = serde_json::to_value(&previous)?;
            changed[field] = value;
            fs::write(&file, serde_json::to_vec(&changed)?)?;
            invalid_candidates.push(json!({
                "field": field, "retained": ids(&service)?,
                "error": error_of(service.restore(&previous.id, &root.join("target.sqlite"), input.restore_timestamp), root),
            }));
        }
        fs::write(file, manifest)?;
        Ok(json!({
            "integrity": integrity, "startupError": startup_error, "error": error,
            "invalidCandidates": invalid_candidates, "retained": ids(&service)?, "files": files(root, "backups", "")?,
        }))
    }

    fn restore(root: &Path, db: Connection, input: &Inputs, obstruct: bool) -> Result<Value> {
        let mut service = BackupService::new(root)?;
        let backup = service.create(&db, BackupReason::CleanExit, input.timestamp)?;
        let target = root.join("target.sqlite");
        let old = Connection::open(&target)?;
        old.execute_batch(&input.old_target_sql)?;
        let old_wal_bytes = root.join("target.sqlite-wal").metadata()?.len();
        if obstruct {
            fs::create_dir(root.join(format!(
                "target.sqlite.replaced-{}",
                input.restore_timestamp
            )))?;
        }
        let error = error_of(
            BackupService::new(root)?.restore(&backup.id, &target, input.restore_timestamp),
            root,
        );
        let before_old_close: Vec<_> = names(root)?
            .into_iter()
            .filter(|name| name == "target.sqlite-wal" || name == "target.sqlite-shm")
            .map(|name| name.into_string().unwrap())
            .collect();
        old.close().map_err(|(_, error)| error)?;
        Ok(json!({
            "error": error, "oldWalBytes": old_wal_bytes, "beforeOldClose": before_old_close,
            "retained": ids(&service)?, "files": files(root, ".", "target.sqlite")?, "backups": files(root, "backups", "")?,
        }))
    }

    fn publication(root: &Path, db: Connection, input: &Inputs, obstruct: bool) -> Result<Value> {
        let mut service = BackupService::new(root)?;
        let previous = service.create(&db, BackupReason::PreMigration, input.timestamp)?;
        let previous_bytes = fs::read(&previous.path)?;
        db.execute_batch(&input.wal_sql)?;
        let dir = root.join("backups");
        if obstruct {
            fs::create_dir(dir.join(format!("matou-{}-clean-exit-v0.json", input.timestamp + 1)))?;
        } else {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o500))?;
        }
        let error = error_of(
            service.backup_for_clean_exit(&db, input.timestamp, CLEAN_EXIT_BACKUP_TIMEOUT),
            root,
        );
        if !obstruct {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755))?;
        }
        db.close().map_err(|(_, error)| error)?;
        Ok(json!({
            "error": error, "sourceAfterFailure": wal_state(root)?,
            "previousUnchanged": previous_bytes == fs::read(&previous.path)?,
            "retained": ids(&service)?, "files": files(root, "backups", "")?,
        }))
    }

    // Compile the unchanged implementation with its existing test-only hook
    // so the gate can suspend the real copy after a SQLite backup step.
    #[allow(dead_code)]
    mod copy_probe {
        include!("../src/backup.rs");

        pub(super) fn run(
            root: &Path,
            db: &Connection,
            now: i64,
            duration: Duration,
        ) -> Result<(anyhow::Error, bool)> {
            use std::{cell::Cell, rc::Rc};
            let mut service = BackupService::new(root)?;
            let first_step = Rc::new(Cell::new(false));
            let entered = first_step.clone();
            tests::STAGE_HOOK.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move |stage, budget| {
                    if stage == TestStage::BackupStep {
                        entered.set(true);
                        std::thread::sleep(
                            budget.deadline.saturating_duration_since(Instant::now()),
                        );
                    }
                }));
            });
            let outcome = service.backup_for_clean_exit(db, now, duration);
            tests::STAGE_HOOK.with_borrow_mut(|hook| *hook = None);
            Ok((outcome.unwrap_err(), first_step.get()))
        }
    }

    fn timeout(root: &Path, db: Connection, input: &Inputs) -> Result<Value> {
        db.execute_batch(&input.copy_sql)?;
        let page_count: i64 = db.query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let started = Instant::now();
        let (failure, entered) = copy_probe::run(
            root,
            &db,
            input.timestamp,
            Duration::from_millis(input.budget_ms),
        )?;
        let within_bound = started.elapsed() <= Duration::from_millis(input.upper_bound_ms);
        assert!(entered && page_count > 100 && within_bound);
        let source_error_on_return = error_of(
            db.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .map_err(Into::into),
            root,
        );
        let files_on_return = files(root, "backups", "")?;
        let service = BackupService::new(root)?;
        Ok(json!({
            "error": error_of::<()>(Err(failure), root), "withinBound": within_bound, "pageCount": page_count,
            "sourceErrorOnReturn": source_error_on_return, "filesOnReturn": files_on_return,
            "retained": ids(&service)?, "files": files(root, "backups", "")?,
        }))
    }

    #[test]
    fn export_fixed_backup_cases() -> Result<()> {
        let Some(gate) = std::env::var_os("MATOU_BACKUP_GATE_RUN").map(PathBuf::from) else {
            return Ok(());
        };
        let input: Inputs = serde_json::from_reader(fs::File::open(
            std::env::var_os("MATOU_BACKUP_GATE_INPUT").unwrap(),
        )?)?;
        let mut cases = serde_json::Map::new();
        for name in [
            "normal",
            "wal",
            "rotation",
            "sidecar",
            "invalid",
            "existing-target",
            "rollback",
            "disk-publication",
            "permission",
            "timeout",
        ] {
            let root = gate.join("native").join(name);
            fs::create_dir_all(&root)?;
            let db = Connection::open(root.join("source.sqlite"))?;
            configure_connection(&db)?;
            db.execute_batch(&input.seed_sql)?;
            let checkpoint = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            assert_eq!(checkpoint, (0, 0, 0));
            let result = match name {
                "invalid" => invalid(&root, db, &input),
                "existing-target" | "rollback" => restore(&root, db, &input, name == "rollback"),
                "disk-publication" | "permission" => {
                    publication(&root, db, &input, name == "disk-publication")
                }
                "timeout" => timeout(&root, db, &input),
                _ => clean_exit_case(&root, db, &input, name),
            };
            cases.insert(
                name.to_owned(),
                result.map_err(|error| error.context(name.to_owned()))?,
            );
        }
        let output = json!({"sqlite": rusqlite::version(), "cases": cases});
        fs::write(
            gate.join("native.json"),
            serde_json::to_string_pretty(&output)? + "\n",
        )?;
        Ok(())
    }
}
