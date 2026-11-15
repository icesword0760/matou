use anyhow::{Result, bail};
use matou_native_core::storage::{
    CommandMetadata, CommandReplayConflictError, DomainEventInput, configure_connection, execute,
    migrate,
};
use rusqlite::{Connection, OptionalExtension, params, types::ValueRef};
use serde_json::{Map, Value, json};

const REFERENCE: &str = include_str!("fixtures/storage-reference.json");
const CATALOG: &str = include_str!("../schema/migrations.json");

fn query_json(db: &Connection, sql: &str) -> Result<Value> {
    let mut statement = db.prepare(sql)?;
    let names: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let rows = statement.query_map([], |row| {
        let mut values = Map::new();
        for (index, name) in names.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => json!(value),
                ValueRef::Real(value) => json!(value),
                ValueRef::Text(_) => json!(row.get::<_, String>(index)?),
                ValueRef::Blob(_) => return Err(rusqlite::Error::InvalidQuery),
            };
            values.insert(name.to_owned(), value);
        }
        Ok(Value::Object(values))
    })?;
    Ok(Value::Array(rows.collect::<rusqlite::Result<_>>()?))
}

fn number(db: &Connection, sql: &str) -> Result<i64> {
    Ok(db.query_row(sql, [], |row| row.get(0))?)
}

fn configured_memory_db() -> Result<Connection> {
    let db = Connection::open_in_memory()?;
    configure_connection(&db)?;
    Ok(db)
}

#[test]
fn fresh_database_has_all_reference_schema_objects() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut db = Connection::open(directory.path().join("matou.sqlite"))?;
    configure_connection(&db)?;

    assert_eq!(
        db.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?,
        "wal"
    );
    assert_eq!(number(&db, "PRAGMA foreign_keys")?, 1);
    assert_eq!(number(&db, "PRAGMA synchronous")?, 2);
    assert_eq!(number(&db, "PRAGMA busy_timeout")?, 5000);
    assert_eq!(number(&db, "PRAGMA trusted_schema")?, 0);
    assert_eq!(number(&db, "SELECT COUNT(*) FROM _runtime_meta")?, 0);

    let migration = migrate(&mut db, 1234)?;
    assert_eq!(migration.applied_versions, (1..=35).collect::<Vec<_>>());
    assert_eq!(migration.current_version, 35);
    assert_eq!(
        number(
            &db,
            "SELECT COUNT(*) FROM schema_migrations WHERE applied_at = 1234"
        )?,
        35
    );
    assert_eq!(number(&db, "PRAGMA foreign_keys")?, 1);
    assert_eq!(number(&db, "PRAGMA legacy_alter_table")?, 0);

    let reference: Value = serde_json::from_str(REFERENCE)?;
    let actual = query_json(
        &db,
        "SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )?;
    let expected = reference["schema"].as_array().unwrap();
    let actual = actual.as_array().unwrap();
    assert_eq!(expected.len(), 87);
    assert_eq!(actual.len(), expected.len());
    for (current, frozen) in actual.iter().zip(expected) {
        assert_eq!(current, frozen, "frozen schema object {}", frozen["name"]);
    }
    Ok(())
}

fn event(event_id: &str, payload: Value, occurred_at: i64) -> DomainEventInput {
    DomainEventInput {
        event_id: event_id.into(),
        event_type: "workspace.parity-contract".into(),
        aggregate_type: "workspace".into(),
        aggregate_id: "workspace-contract".into(),
        workspace_id: None,
        task_id: None,
        session_id: None,
        payload,
        schema_version: None,
        required_terminal_sequence: None,
        occurred_at,
    }
}

#[test]
fn domain_transaction_matches_frozen_source_and_replays_after_reopen() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    migrate(&mut db, 1000)?;
    let reference: Value = serde_json::from_str(REFERENCE)?;
    let frozen = &reference["transaction"];

    let command: CommandMetadata = serde_json::from_value(json!({
        "commandId": "transaction-contract",
        "commandType": "parity.contract",
        "requestHash": "same-request",
        "causationId": "cause-1",
        "correlationId": "correlation-1",
    }))?;
    let committed = execute(&mut db, &command, 1150, |tx, events| {
        tx.execute(
            "INSERT INTO _runtime_meta (key, value) VALUES ('parity-contract', 'committed')",
            [],
        )?;
        events.push(event(
            "contract-event-1",
            json!({"value": "中文🙂", "retained": true}),
            1100,
        ));
        Ok(json!({"outcome": "committed", "count": 1}))
    })?;
    assert_eq!(serde_json::to_value(committed)?, frozen["result"]);
    let replay = execute(&mut db, &command, 1151, |_, _| {
        panic!("replay ran the mutation")
    })?;
    assert_eq!(serde_json::to_value(replay)?, frozen["replayed"]);

    let conflicting = CommandMetadata {
        request_hash: "different-request".into(),
        ..command.clone()
    };
    let error = execute(&mut db, &conflicting, 1152, |_, _| {
        panic!("conflict ran the mutation")
    })
    .unwrap_err();
    assert!(error.is::<CommandReplayConflictError>());
    assert_eq!(json!(error.to_string()), frozen["conflict"]);

    let failing: CommandMetadata = serde_json::from_value(json!({
        "commandId": "rollback-contract",
        "commandType": "parity.contract",
        "requestHash": "rollback",
    }))?;
    let error = execute(&mut db, &failing, 1200, |tx, events| {
        tx.execute(
            "INSERT INTO _runtime_meta (key, value) VALUES ('must-rollback', 'bad')",
            [],
        )?;
        events.push(event("must-not-commit", json!({}), 1200));
        bail!("forced mutation failure");
    })
    .unwrap_err();
    assert_eq!(json!(error.to_string()), frozen["rollback"]);

    drop(db);
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    let after_reopen = migrate(&mut db, 1250)?;
    assert_eq!(after_reopen.current_version, 35);
    assert!(after_reopen.applied_versions.is_empty());
    let replay = execute(&mut db, &command, 1300, |_, _| {
        panic!("durable replay ran the mutation")
    })?;
    assert_eq!(serde_json::to_value(replay)?, frozen["replayed"]);
    assert_eq!(
        db.query_row(
            "SELECT value FROM _runtime_meta WHERE key = 'parity-contract'",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "committed"
    );
    assert_eq!(
        query_json(
            &db,
            "SELECT * FROM domain_events WHERE command_id = 'transaction-contract' ORDER BY seq"
        )?,
        frozen["events"]
    );
    assert_eq!(
        query_json(&db, "SELECT command_id, command_type, request_hash, response_json, first_event_seq, last_event_seq
                         FROM command_deduplication WHERE command_id = 'transaction-contract'")?[0],
        frozen["dedup"]
    );
    assert_eq!(
        number(
            &db,
            "SELECT committed_at FROM command_deduplication WHERE command_id = 'transaction-contract'"
        )?,
        1150
    );
    for (query, field) in [
        (
            "SELECT EXISTS (SELECT 1 FROM _runtime_meta WHERE key = 'must-rollback')",
            "rollbackRowPresent",
        ),
        (
            "SELECT EXISTS (SELECT 1 FROM domain_events WHERE event_id = 'must-not-commit')",
            "rollbackEventPresent",
        ),
        (
            "SELECT EXISTS (SELECT 1 FROM command_deduplication WHERE command_id = 'rollback-contract')",
            "rollbackCommandPresent",
        ),
    ] {
        assert_eq!(json!(number(&db, query)? != 0), frozen[field]);
    }
    Ok(())
}

#[test]
fn event_order_and_defaults_survive_partial_event_failure() -> Result<()> {
    let mut db = configured_memory_db()?;
    migrate(&mut db, 1000)?;
    let command: CommandMetadata = serde_json::from_value(json!({
        "commandId": "event-batch",
        "commandType": "parity.batch",
        "requestHash": "batch-v1",
    }))?;

    let error = execute(&mut db, &command, 1100, |tx, events| {
        tx.execute("INSERT INTO _runtime_meta VALUES ('batch', 'applied')", [])?;
        let first = event("first", json!({}), 1001);
        events.push(first.clone());
        events.push(first);
        Ok(Value::Null)
    })
    .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<rusqlite::Error>()
            .and_then(rusqlite::Error::sqlite_error_code),
        Some(rusqlite::ErrorCode::ConstraintViolation),
    );
    for table in ["_runtime_meta", "domain_events", "command_deduplication"] {
        assert_eq!(number(&db, &format!("SELECT COUNT(*) FROM {table}"))?, 0);
    }
    let created = execute(&mut db, &command, 1200, |tx, events| {
        tx.execute("INSERT INTO _runtime_meta VALUES ('batch', 'applied')", [])?;
        let mut first = event("first", json!({}), 1002);
        first.schema_version = Some(3);
        first.required_terminal_sequence = Some(42);
        events.push(first);
        events.push(event("second", json!({}), 1001));
        Ok(Value::Null)
    })?;
    assert_eq!(created.first_event_sequence, Some(1));
    assert_eq!(created.last_event_sequence, Some(2));
    assert_eq!(
        query_json(
            &db,
            "SELECT event_id, schema_version, required_terminal_sequence FROM domain_events ORDER BY seq"
        )?,
        json!([
            {"event_id": "first", "schema_version": 3, "required_terminal_sequence": 42},
            {"event_id": "second", "schema_version": 1, "required_terminal_sequence": null},
        ])
    );

    let no_events = CommandMetadata {
        command_id: "no-events".into(),
        ..command
    };
    let committed = execute(&mut db, &no_events, 1300, |_, _| Ok(Value::Null))?;
    assert_eq!(
        serde_json::to_value(committed)?,
        json!({"result": null, "replayed": false})
    );
    let replay = execute(&mut db, &no_events, 1400, |_, _| {
        panic!("eventless replay ran the mutation")
    })?;
    assert_eq!(
        serde_json::to_value(replay)?,
        json!({"result": null, "replayed": true})
    );
    Ok(())
}

#[test]
fn upgrades_legacy_history_but_rejects_corrupt_or_future_history_without_writes() -> Result<()> {
    let mut db = configured_memory_db()?;
    migrate(&mut db, 1000)?;
    let catalog: Value = serde_json::from_str(CATALOG)?;
    let m = catalog.as_array().unwrap();
    for (version, legacy) in [
        (1, &m[0]["legacyChecksum"]),
        (18, &m[17]["acceptedLegacyChecksums"][0]),
    ] {
        db.execute(
            "UPDATE schema_migrations SET name = 'old display name', checksum = ? WHERE version = ?",
            params![legacy.as_str().unwrap(), version],
        )?;
    }
    db.execute(
        "UPDATE schema_migrations SET name = 'old display name' WHERE version = 2",
        [],
    )?;
    db.execute(
        "UPDATE schema_migrations SET checksum = 'v2:invalid' WHERE version = 17",
        [],
    )?;
    let history =
        "SELECT version, name, checksum, applied_at FROM schema_migrations ORDER BY version";
    let invalid = query_json(&db, history)?;
    assert_eq!(
        migrate(&mut db, 2000).unwrap_err().to_string(),
        "checksum mismatch for applied migration 17"
    );
    assert_eq!(query_json(&db, history)?, invalid);

    db.execute(
        "UPDATE schema_migrations SET checksum = ? WHERE version = 17",
        [m[16]["checksum"].as_str().unwrap()],
    )?;
    db.execute(
        "INSERT INTO schema_migrations VALUES (36, 'future', 'v2:unknown', 1500)",
        [],
    )?;
    let future = query_json(&db, history)?;
    assert_eq!(
        migrate(&mut db, 2000).unwrap_err().to_string(),
        "database schema version 36 is newer than supported version 35"
    );
    assert_eq!(query_json(&db, history)?, future);

    db.execute("DELETE FROM schema_migrations WHERE version = 36", [])?;
    let upgraded = migrate(&mut db, 2000)?;
    assert_eq!(upgraded.current_version, 35);
    assert!(upgraded.applied_versions.is_empty());
    let actual = query_json(&db, history)?;
    for (applied, migration) in actual.as_array().unwrap().iter().zip(m) {
        assert_eq!(
            applied,
            &json!({
                "version": migration["version"],
                "name": migration["name"],
                "checksum": migration["checksum"],
                "applied_at": 1000,
            })
        );
    }
    Ok(())
}

#[test]
fn migration_35_rolls_back_on_foreign_key_violation_and_can_be_retried() -> Result<()> {
    let mut db = configured_memory_db()?;
    let catalog: Value = serde_json::from_str(CATALOG)?;
    // Set up an old database using the original catalog, not the native migration runner.
    db.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL,
                       checksum TEXT NOT NULL, applied_at INTEGER NOT NULL) STRICT;",
    )?;
    for migration in &catalog.as_array().unwrap()[..34] {
        db.execute_batch(migration["sql"].as_str().unwrap())?;
        db.execute(
            "INSERT INTO schema_migrations VALUES (?, ?, ?, 1000)",
            params![
                migration["version"].as_i64().unwrap(),
                migration["name"].as_str().unwrap(),
                migration["checksum"].as_str().unwrap()
            ],
        )?;
    }
    db.execute_batch(
        "INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
           VALUES ('work', 'Workspace', '/tmp/workspace', 1, 1);
         INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
           VALUES ('context', 'work', 'plain-directory', '/tmp/workspace', 1);
         INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at)
           VALUES ('task', 'work', 'context', 'Task', 'active', 1, 1);
         INSERT INTO sessions (id, task_id, execution_context_id, kind, status, created_at, updated_at, last_activity_at)
           VALUES ('shell-1', 'task', 'context', 'shell', 'created', 1, 1, 1);
         PRAGMA foreign_keys = OFF;
         INSERT INTO session_runs (id, session_id, ordinal, runtime_generation, status, started_at)
           VALUES ('orphan-run', 'missing-session', 1, 'g', 'running', 1);
         PRAGMA foreign_keys = ON;"
    )?;
    let session_schema = "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'sessions'";
    let before = query_json(&db, session_schema)?;
    let error = migrate(&mut db, 2000).unwrap_err().to_string();
    assert!(
        error.starts_with("migration 35 left 1 foreign key violation(s):"),
        "{error}"
    );
    assert_eq!(query_json(&db, session_schema)?, before);
    assert_eq!(
        number(&db, "SELECT MAX(version) FROM schema_migrations")?,
        34
    );
    assert_eq!(number(&db, "PRAGMA foreign_keys")?, 1);
    assert_eq!(number(&db, "PRAGMA legacy_alter_table")?, 0);

    db.execute("DELETE FROM session_runs WHERE id = 'orphan-run'", [])?;
    let migrated = migrate(&mut db, 2100)?;
    assert_eq!(migrated.applied_versions, vec![35]);
    assert_eq!(migrated.current_version, 35);
    let cwd = db
        .query_row("SELECT cwd FROM sessions WHERE id = 'shell-1'", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    assert_eq!(cwd.as_deref(), Some("/tmp/workspace"));
    assert_eq!(query_json(&db, "PRAGMA foreign_key_check")?, json!([]));
    Ok(())
}
