use super::*;
use matou_native_core::ownership::DatabaseOwner;
use matou_native_runtime::host::{
    DatabaseRecoveryAction, DatabaseRecoveryDetails, DatabaseRecoveryReason,
    DatabaseRecoveryResult, STORAGE_RECOVERY_WAITING, StartupBlockKind,
    database_recovery_request_id,
};

const MARKER: &str = "matou.sqlite.recovery.json";
const DB: &str = "matou.sqlite";

fn seed(data: &Path) -> Result<Vec<u8>> {
    fs::create_dir_all(data)?;
    let database = Connection::open(data.join(DB))?;
    database.execute_batch(
        "PRAGMA page_size=4096;
         CREATE TABLE gate_payload (value TEXT NOT NULL);
         INSERT INTO gate_payload VALUES ('sentinel');",
    )?;
    database.close().map_err(|(_, error)| error)?;
    Ok(fs::read(data.join(DB))?)
}

fn hello(child: &mut RuntimeChild) -> Result<()> {
    write_message(
        child.input.as_mut().context("child stdin")?,
        &ClientMessage::Hello { version: VERSION },
    )?;
    ensure!(matches!(
        child.messages.recv_timeout(TIMEOUT)??,
        ServerMessage::Hello { version: VERSION }
    ));
    Ok(())
}

fn next(child: &mut RuntimeChild, accept: impl Fn(&Event) -> bool) -> Result<Event> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let event = child.event(deadline, false)?;
        if accept(&event) {
            return Ok(event);
        }
        ensure!(
            !matches!(event, Event::Closed),
            "child closed before expected event"
        );
    }
}

fn recovery(child: &mut RuntimeChild) -> Result<DatabaseRecoveryDetails> {
    let event = next(child, |event| {
        matches!(event, Event::DatabaseRecoveryRequired(_))
    })?;
    let Event::DatabaseRecoveryRequired(details) = event else {
        unreachable!()
    };
    Ok(details)
}

fn retry(child: &mut RuntimeChild, id: &str) -> Result<String> {
    let request_id = database_recovery_request_id();
    child.send(Command::DatabaseRecovery {
        request_id: request_id.clone(),
        action: DatabaseRecoveryAction::RetryOpen {
            expected_recovery_id: id.into(),
        },
    })?;
    Ok(request_id)
}

fn result(child: &mut RuntimeChild, id: &str) -> Result<DatabaseRecoveryResult> {
    let event = next(
        child,
        |event| matches!(event, Event::DatabaseRecoveryResult(result) if result.request_id == id),
    )?;
    let Event::DatabaseRecoveryResult(result) = event else {
        unreachable!()
    };
    Ok(result)
}

fn shutdown(child: &mut RuntimeChild) -> Result<()> {
    child.send(Command::Shutdown)?;
    child.closed()?;
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "database recovery shutdown: {stderr}");
    Ok(())
}

#[test]
fn damaged_database_retry_keeps_evidence_and_enters_ready_in_same_process() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    let healthy = seed(&child.data)?;
    let source = startup::open_read_only(&child.data.join(DB))?;
    let backup = BackupService::new(&child.data)?.create(&source, BackupReason::CleanExit, 123)?;
    let backup_bytes = fs::read(&backup.path)?;
    drop(source);
    let mut damaged = healthy.clone();
    damaged[28..32].copy_from_slice(&3u32.to_be_bytes());
    damaged.resize(3 * 4096, 0);
    fs::write(child.data.join(DB), &damaged)?;
    hello(&mut child)?;

    let details = recovery(&mut child)?;
    let pid = child.child.id();
    ensure!(details.reason == DatabaseRecoveryReason::Integrity);
    ensure!(details.backups.len() == 1 && details.backups[0].id == backup.id);
    let projection = serde_json::to_value(&details)?;
    ensure!(
        projection.get("markerPath").is_none() && projection["backups"][0].get("path").is_none()
    );
    ensure!(!child.workspace.exists());
    ensure!(fs::read(&details.quarantined_path)? == damaged);
    let marker = child.data.join(MARKER);
    let marker_bytes = fs::read(&marker)?;

    let wrong_id = retry(&mut child, "../wrong")?;
    ensure!(
        result(&mut child, &wrong_id)?
            .error
            .context("wrong recovery ID")?
            .code
            == "RECOVERY_ID_CHANGED"
    );
    ensure!(!child.data.join(DB).exists() && fs::read(&marker)? == marker_bytes);

    let missing_original = retry(&mut child, &details.recovery_id)?;
    let published = recovery(&mut child)?;
    ensure!(published.recovery_id == details.recovery_id);
    let error = result(&mut child, &missing_original)?
        .error
        .context("missing original")?;
    ensure!(
        error.code == "open"
            && error.message == "原数据库尚未回到可检查的位置，请先恢复备份或导出恢复资料"
    );

    fs::write(child.data.join(DB), &healthy)?;
    let mut marker_document: serde_json::Value = serde_json::from_slice(&marker_bytes)?;
    marker_document["quarantinedPath"] =
        serde_json::json!(format!("{}-changed", details.quarantined_path.display()));
    fs::write(&marker, serde_json::to_vec(&marker_document)?)?;
    let changed = retry(&mut child, &details.recovery_id)?;
    ensure!(
        result(&mut child, &changed)?
            .error
            .context("changed marker")?
            .code
            == "marker"
    );
    ensure!(fs::read(child.data.join(DB))? == healthy);
    ensure!(!child.data.join(format!("{DB}.owner")).exists());
    fs::write(&marker, &marker_bytes)?;

    let settlement = child
        .data
        .join(format!("{MARKER}.resolved-{}", details.recovery_id));
    fs::create_dir(&settlement)?;
    let commit_failure = retry(&mut child, &details.recovery_id)?;
    ensure!(
        result(&mut child, &commit_failure)?
            .error
            .context("settlement collision")?
            .code
            == "marker-commit"
    );
    ensure!(fs::read(&marker)? == marker_bytes && !child.data.join(format!("{DB}.owner")).exists());
    fs::remove_dir(&settlement)?;

    let fence = Connection::open(child.data.join(format!("{DB}.recovery-action.sqlite")))?;
    fence.execute_batch("BEGIN EXCLUSIVE;")?;
    let request = retry(&mut child, &details.recovery_id)?;
    let duplicate = retry(&mut child, &details.recovery_id)?;
    child.send(Command::SaveProvider {
        request_id: 42,
        input: serde_json::json!({}),
    })?;
    ensure!(
        result(&mut child, &duplicate)?
            .error
            .context("duplicate action")?
            .code
            == "RECOVERY_BUSY"
    );
    let rejected = next(&mut child, |event| {
        matches!(event, Event::StorageCommandRejected { .. })
    })?;
    ensure!(matches!(rejected, Event::StorageCommandRejected {
        request_id: Some(42), message,
    } if message == STORAGE_RECOVERY_WAITING));
    fence.execute_batch("ROLLBACK;")?;
    drop(fence);

    let ready = next(&mut child, |event| {
        matches!(
            event,
            Event::Ready { .. } | Event::DatabaseRecoveryResult(_)
        )
    })?;
    ensure!(
        matches!(ready, Event::Ready { .. }),
        "recovery result must follow Ready"
    );
    ensure!(result(&mut child, &request)?.error.is_none());
    ensure!(child.child.id() == pid && child.child.try_wait()?.is_none());
    ensure!(fs::read(&marker)? == marker_bytes);
    ensure!(settlement.is_file());
    ensure!(fs::read(&details.quarantined_path)? == damaged);
    ensure!(fs::read(&backup.path)? == backup_bytes);
    let database = startup::open_read_only(&child.data.join(DB))?;
    let payload: String =
        database.query_row("SELECT value FROM gate_payload", [], |row| row.get(0))?;
    ensure!(payload == "sentinel");
    drop(database);
    let late = retry(&mut child, &details.recovery_id)?;
    ensure!(
        result(&mut child, &late)?
            .error
            .context("late action")?
            .code
            == "RECOVERY_NOT_PENDING"
    );
    shutdown(&mut child)?;
    for _ in 0..2 {
        let mut restarted = RuntimeChild::spawn(root.path())?;
        restarted.connect()?;
        shutdown(&mut restarted)?;
    }
    Ok(())
}

#[test]
fn ownership_retry_refuses_new_live_owner_and_archives_malformed_evidence() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    let healthy = seed(&child.data)?;
    let db = child.data.join(DB);
    let owner_path = child.data.join(format!("{DB}.owner"));
    fs::write(&owner_path, "{")?;
    hello(&mut child)?;
    let details = recovery(&mut child)?;
    ensure!(details.reason == DatabaseRecoveryReason::Ownership);
    ensure!(details.ownership_issue.as_deref() == Some("owner-record-malformed"));

    fs::remove_file(&owner_path)?;
    let mut lease = DatabaseOwner::acquire(&db, "competing-live-owner")?;
    let live_bytes = fs::read(&owner_path)?;
    let request = retry(&mut child, &details.recovery_id)?;
    ensure!(
        result(&mut child, &request)?
            .error
            .context("live owner")?
            .message
            .contains("live Runtime")
    );
    ensure!(fs::read(&owner_path)? == live_bytes && fs::read(&db)? == healthy);
    ensure!(!child.data.join("recovery-evidence").exists());

    lease.release()?;
    fs::write(&owner_path, "{")?;
    let request = retry(&mut child, &details.recovery_id)?;
    next(&mut child, |event| matches!(event, Event::Ready { .. }))?;
    ensure!(result(&mut child, &request)?.error.is_none());
    let archived = child
        .data
        .join("recovery-evidence")
        .join(request)
        .join(format!("{DB}.owner"));
    ensure!(fs::read_to_string(archived)? == "{");
    shutdown(&mut child)
}

#[test]
fn invalid_marker_and_permission_read_only_keep_existing_startup_paths() -> Result<()> {
    for malformed in [true, false] {
        let root = tempfile::tempdir()?;
        let mut child = RuntimeChild::spawn(root.path())?;
        let healthy = seed(&child.data)?;
        let database = child.data.join(DB);
        let marker = child.data.join(MARKER);
        if malformed {
            fs::write(&marker, b"{")?;
        } else {
            fs::set_permissions(&database, fs::Permissions::from_mode(0o444))?;
            fs::set_permissions(&child.data, fs::Permissions::from_mode(0o555))?;
        }
        hello(&mut child)?;
        let event = next(&mut child, |event| {
            matches!(
                event,
                Event::Diagnostic(_)
                    | Event::StartupBlocked(_)
                    | Event::DatabaseRecoveryRequired(_)
                    | Event::Ready { .. },
            )
        })?;
        if malformed {
            ensure!(
                matches!(event, Event::Diagnostic(text) if text.contains("recovery marker is invalid"))
            );
            ensure!(fs::read(&marker)? == b"{");
        } else {
            fs::set_permissions(&child.data, fs::Permissions::from_mode(0o755))?;
            ensure!(
                matches!(event, Event::StartupBlocked(reason) if reason.kind == StartupBlockKind::ReadOnly)
            );
            ensure!(!marker.exists());
        }
        child.closed()?;
        ensure!(!child.wait()?.0.success());
        ensure!(fs::read(&database)? == healthy);
        ensure!(!child.workspace.exists());
    }
    Ok(())
}
