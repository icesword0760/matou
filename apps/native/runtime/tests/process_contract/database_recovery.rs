use super::*;
use matou_native_core::{ownership::DatabaseOwner, startup::ReadOnlyReason};
use matou_native_runtime::host::{
    DatabaseRecoveryAction, DatabaseRecoveryDetails, DatabaseRecoveryReason,
    DatabaseRecoveryResult, ReadOnlyQuery, STORAGE_READ_ONLY, STORAGE_RECOVERY_WAITING,
    database_recovery_request_id,
};
use serde_json::Value;

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
        request_id: Some(42), message, ..
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
fn invalid_marker_keeps_existing_startup_path() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut child = RuntimeChild::spawn(root.path())?;
    let healthy = seed(&child.data)?;
    let marker = child.data.join(MARKER);
    fs::write(&marker, b"{")?;
    hello(&mut child)?;
    let event = next(&mut child, |event| matches!(event, Event::Diagnostic(_)))?;
    ensure!(
        matches!(event, Event::Diagnostic(text) if text.contains("recovery marker is invalid"))
    );
    ensure!(fs::read(&marker)? == b"{");
    child.closed()?;
    ensure!(!child.wait()?.0.success());
    ensure!(fs::read(child.data.join(DB))? == healthy);
    ensure!(!child.workspace.exists());
    Ok(())
}

fn seed_read_only(data: &Path, newer: bool) -> Result<()> {
    use matou_native_core::{
        hierarchy::{self, BootstrapWindowInput, HierarchyIds, workspaces::CreateWorkspaceInput},
        journal::{Frame, JournalOptions, Record, SegmentJournal},
        storage::{CommandMetadata, configure_connection, migrate},
    };
    let ids = |suffix: &str| HierarchyIds {
        workspace_id: format!("workspace-{suffix}"),
        execution_context_id: format!("context-{suffix}"),
        task_id: format!("task-{suffix}"),
        scene_id: format!("scene-{suffix}"),
        root_node_id: format!("root-{suffix}"),
        session_id: format!("session-{suffix}"),
        mount_id: format!("mount-{suffix}"),
    };
    let metadata = |name: &str| CommandMetadata {
        command_id: name.into(),
        command_type: name.into(),
        request_hash: name.into(),
        causation_id: None,
        correlation_id: None,
    };
    fs::create_dir_all(data)?;
    let scratch = tempfile::tempdir()?;
    let source = scratch.path().join(DB);
    let mut db = Connection::open(&source)?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    let window_id = "main-window-1";
    let root = data.parent().context("fixture root")?;
    hierarchy::bootstrap_window(
        &mut db,
        &metadata("bootstrap"),
        &BootstrapWindowInput {
            window_id: window_id.into(),
            default_root_directory: root.join("first").display().to_string(),
            default_name: "第一工作空间".into(),
            now: 2,
        },
        &ids("first"),
        "第一事项",
        2,
    )?;
    hierarchy::workspaces::create_workspace(
        &mut db,
        &metadata("create"),
        &CreateWorkspaceInput {
            window_id: window_id.into(),
            name: "第二工作空间".into(),
            root_directory: root.join("second").display().to_string(),
            navigation: None,
            now: 3,
        },
        &ids("second"),
        "第二事项",
        3,
    )?;
    matou_native_core::geometry::put(
        &db,
        &matou_native_core::geometry::GeometryUpdate {
            scene_id: "scene-first".into(),
            owner_key: "scene".into(),
            layout_revision: 1.,
            geometry: serde_json::json!({"scrollLeft": 11}),
            now: 4,
        },
    )?;
    if newer {
        db.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE);
             INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (999, 'newer', 'newer', 5);",
        )?;
        for name in [DB, "matou.sqlite-wal", "matou.sqlite-shm"] {
            fs::copy(scratch.path().join(name), data.join(name))?;
        }
        fs::write(
            data.join("matou.sqlite.owner"),
            br#"{"pid":9007199254740991,"runtimeGeneration":"expired-fixture"}"#,
        )?;
    } else {
        db.close().map_err(|(_, error)| error)?;
        fs::copy(source, data.join(DB))?;
    }
    let mut journal = SegmentJournal::open(data, "session-first", JournalOptions::default())?;
    journal.append(&Record {
        timestamp: 3,
        frame: Frame::Output {
            sequence: 1,
            data: format!(
                "{}\r\n末行哨兵\r\n",
                (0..1100)
                    .map(|line| format!("历史 {line}\r\n"))
                    .collect::<String>()
            )
            .into_bytes(),
        },
    })?;
    journal.close()?;
    Ok(())
}

fn file_tree(root: &Path) -> Result<std::collections::BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(
        root: &Path,
        path: &Path,
        tree: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
    ) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                visit(root, &path, tree)?;
            } else {
                tree.insert(path.strip_prefix(root)?.to_owned(), fs::read(&path)?);
            }
        }
        Ok(())
    }
    let mut tree = std::collections::BTreeMap::new();
    visit(root, root, &mut tree)?;
    Ok(tree)
}

#[test]
fn both_read_only_sources_can_browse_search_and_export_without_changing_source() -> Result<()> {
    use matou_native_runtime::{
        history::HistorySearchOptions,
        history_worker::{HistoryReply, HistoryRequest},
    };
    for reason in [
        ReadOnlyReason::FilesystemReadOnly,
        ReadOnlyReason::NewerSchema,
    ] {
        let root = tempfile::tempdir()?;
        let destination = root.path().join("导出");
        let mut child = RuntimeChild::spawn_configured_with(
            root.path(),
            "exit 97",
            Path::new("/shell-must-not-open"),
            None,
            |command| {
                command.env("MATOU_RECOVERY_EXPORT_DIR", &destination);
            },
        )?;
        seed_read_only(&child.data, reason == ReadOnlyReason::NewerSchema)?;
        let before = file_tree(&child.data)?;
        let owner = child.data.join("matou.sqlite.owner");
        let owner_mode = owner
            .metadata()
            .ok()
            .map(|metadata| metadata.permissions().mode());
        // Restore permissions before removal even when a later assertion fails.
        struct ReadOnlyDirectory(PathBuf);
        impl Drop for ReadOnlyDirectory {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
        }
        let _permissions = if reason == ReadOnlyReason::FilesystemReadOnly {
            fs::set_permissions(&child.data.join(DB), fs::Permissions::from_mode(0o444))?;
            fs::set_permissions(&child.data, fs::Permissions::from_mode(0o555))?;
            Some(ReadOnlyDirectory(child.data.clone()))
        } else {
            None
        };
        hello(&mut child)?;
        let event = next(&mut child, |event| {
            matches!(
                event,
                Event::ReadOnlyReady { .. } | Event::Diagnostic(_) | Event::Frame { .. }
            )
        })?;
        let Event::ReadOnlyReady {
            reason: actual,
            projection,
        } = event
        else {
            bail!("expected read-only ready")
        };
        ensure!(actual == reason);
        let projection = projection.map_err(anyhow::Error::msg)?;
        ensure!(projection.workspaces.len() == 2 && projection.tasks.len() == 2);
        ensure!(
            projection
                .active
                .hierarchy
                .workspace
                .context("saved workspace")?
                .id
                == "workspace-second"
        );
        ensure!(projection.active.hierarchy.scene.context("saved scene")?.id == "scene-second");
        for (request_id, query) in [
            (
                1,
                ReadOnlyQuery::Workspace {
                    workspace_id: "workspace-first".into(),
                },
            ),
            (
                2,
                ReadOnlyQuery::Task {
                    task_id: "task-first".into(),
                },
            ),
            (
                3,
                ReadOnlyQuery::Scene {
                    scene_id: "scene-first".into(),
                },
            ),
            (
                4,
                ReadOnlyQuery::Session {
                    scene_id: "scene-first".into(),
                    session_id: "session-first".into(),
                },
            ),
            (
                5,
                ReadOnlyQuery::Session {
                    scene_id: "scene-first".into(),
                    session_id: "session-second".into(),
                },
            ),
        ] {
            child.send(Command::ReadOnlyQuery { request_id, query })?;
            let reply = next(
                &mut child,
                |event| matches!(event, Event::ReadOnlyQueryResult { request_id: id, .. } if *id == request_id),
            )?;
            let Event::ReadOnlyQueryResult { result, .. } = reply else {
                unreachable!()
            };
            if request_id == 5 {
                ensure!(result.is_err());
                continue;
            }
            let active = result.map_err(anyhow::Error::msg)?;
            ensure!(active.graph["focusedSessionId"] == "session-first");
            ensure!(active.geometry[0].geometry["scrollLeft"] == 11);
        }
        for (session_id, sequence) in [("session-first", 6), ("../missing", 7)] {
            child.send(Command::History(HistoryRequest::Search {
                session_id: session_id.into(),
                sequence,
                query: "末行哨兵".into(),
                options: HistorySearchOptions::default(),
            }))?;
            let reply = next(
                &mut child,
                |event| matches!(event, Event::History(HistoryReply::Search { sequence: id, .. }) if *id == sequence),
            )?;
            let Event::History(HistoryReply::Search { result, .. }) = reply else {
                unreachable!()
            };
            if sequence == 6 {
                ensure!(result.map_err(anyhow::Error::msg)?.matches[0].text == "末行哨兵");
            } else {
                ensure!(result.is_err_and(|error| error.contains("会话不存在")));
            }
        }
        let mut before_cursor = None;
        for sequence in [8, 9] {
            child.send(Command::History(HistoryRequest::Page {
                session_id: "session-first".into(),
                sequence,
                before: before_cursor,
            }))?;
            let reply = next(
                &mut child,
                |event| matches!(event, Event::History(HistoryReply::Page { sequence: id, .. }) if *id == sequence),
            )?;
            let Event::History(HistoryReply::Page { before, result, .. }) = reply else {
                unreachable!()
            };
            ensure!(before == before_cursor);
            let page = result.map_err(anyhow::Error::msg)?;
            if let Some(cursor) = before {
                ensure!(page.lines.last().context("older page")?.cursor < cursor);
                ensure!(page.lines.first().context("first archived line")?.text == "历史 0");
                ensure!(!page.has_more);
            } else {
                ensure!(
                    page.has_more && page.lines.last().context("latest page")?.text == "末行哨兵"
                );
                before_cursor = Some(page.lines.first().context("latest page cursor")?.cursor);
            }
        }
        let writes = [
            (
                None,
                Command::CreateWorkspace {
                    root_directory: "/unused".into(),
                },
            ),
            (
                None,
                Command::Input {
                    session_id: "session-first".into(),
                    bytes: vec![b'x'],
                    defer_ordering: false,
                },
            ),
            (
                None,
                Command::RestartSession {
                    session_id: "session-first".into(),
                },
            ),
            (
                None,
                Command::SelectScene {
                    scene_id: "scene-second".into(),
                },
            ),
            (
                Some(10),
                Command::SetClaudeRoleAssignments {
                    request_id: 10,
                    assignments: Value::Null,
                },
            ),
        ];
        for (expected, command) in writes {
            child.send(command)?;
            let rejected = next(&mut child, |event| {
                matches!(event, Event::StorageCommandRejected { .. })
            })?;
            ensure!(
                matches!(rejected, Event::StorageCommandRejected { request_id, code, .. }
                if request_id == expected && code == STORAGE_READ_ONLY)
            );
        }
        let request_id = database_recovery_request_id();
        child.send(Command::DatabaseRecovery {
            request_id: request_id.clone(),
            action: DatabaseRecoveryAction::ExportRecoveryBundle,
        })?;
        let export = result(&mut child, &request_id)?;
        ensure!(export.error.is_none(), "export: {:?}", export.error);
        let export = export.exported_path.context("read-only export path")?;
        ensure!(export.starts_with(destination.canonicalize()?));
        let manifest: Value = serde_json::from_slice(&fs::read(export.join("manifest.json"))?)?;
        ensure!(manifest["mode"] == "read-only" && manifest["exportedAt"].as_i64().is_some());
        ensure!(manifest["sourceDatabasePath"] == child.data.join(DB).display().to_string());
        let expected: Vec<_> = [DB, "matou.sqlite-wal", "matou.sqlite-shm"]
            .into_iter()
            .filter(|name| before.contains_key(Path::new(name)))
            .collect();
        ensure!(manifest["exportedFiles"] == serde_json::json!(expected));
        ensure!(file_tree(&export)?.len() == expected.len() + 1);
        ensure!(
            fs::metadata(export.join("manifest.json"))?
                .permissions()
                .mode()
                & 0o777
                == 0o600
        );
        for name in expected {
            ensure!(fs::read(export.join(name))? == before[Path::new(name)]);
        }
        let denied = retry(&mut child, "no-recovery-marker")?;
        let error = result(&mut child, &denied)?
            .error
            .context("write recovery action rejected")?;
        ensure!(error.code == STORAGE_READ_ONLY && !error.retryable);
        shutdown(&mut child)?;
        ensure!(!child.workspace.exists());
        ensure!(
            owner
                .metadata()
                .ok()
                .map(|metadata| metadata.permissions().mode())
                == owner_mode
        );
        let after = file_tree(&child.data)?;
        ensure!(
            before.keys().eq(after.keys())
                && before.iter().all(|(name, bytes)| after[name] == *bytes),
            "read-only startup or shutdown changed source: {:?}",
            before
                .keys()
                .chain(after.keys())
                .filter(|name| before.get(*name) != after.get(*name))
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}

#[test]
fn read_only_export_rejects_the_source_tree_and_reports_destination_errors() -> Result<()> {
    for inside_source in [true, false] {
        let root = tempfile::tempdir()?;
        let destination = if inside_source {
            root.path().join("数据 空间/nested/export")
        } else {
            let file = root.path().join("blocked");
            fs::write(&file, "not a directory")?;
            file
        };
        let mut child = RuntimeChild::spawn_configured_with(
            root.path(),
            "exit 97",
            Path::new("/shell-must-not-open"),
            None,
            |command| {
                command.env("MATOU_RECOVERY_EXPORT_DIR", &destination);
            },
        )?;
        seed_read_only(&child.data, true)?;
        let before = file_tree(&child.data)?;
        hello(&mut child)?;
        next(&mut child, |event| {
            matches!(event, Event::ReadOnlyReady { .. })
        })?;
        let request_id = database_recovery_request_id();
        child.send(Command::DatabaseRecovery {
            request_id: request_id.clone(),
            action: DatabaseRecoveryAction::ExportRecoveryBundle,
        })?;
        let error = result(&mut child, &request_id)?
            .error
            .context("export must fail")?;
        ensure!(error.code == "DATABASE_EXPORT_FAILED");
        ensure!(error.message.contains(if inside_source {
            "数据库目录之外"
        } else {
            "blocked"
        }));
        shutdown(&mut child)?;
        ensure!(file_tree(&child.data)? == before);
        ensure!(!child.data.join("nested").exists());
    }
    Ok(())
}
