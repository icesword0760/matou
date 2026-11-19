use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, bail};
use matou_native_core::{
    hierarchy::{BootstrapWindowInput, HierarchyIds, bootstrap_window},
    sessions::{
        CreateShellSessionInput, StartRunInput, archive_session, create_shell_session, finish_run,
        get_run, get_session, interrupt_run, list_all_runs, list_runs, list_sessions,
        read_claude_role_assignments, rename_session, save_claude_role_assignments, start_run,
        update_cwd,
    },
    storage::{CommandMetadata, DomainCommit, configure_connection, migrate},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const REFERENCE: &str = include_str!("fixtures/session-repository-reference.json");
const TABLES: [&str; 4] = [
    "sessions",
    "session_runs",
    "domain_events",
    "command_deduplication",
];

#[test]
fn role_assignments_merge_with_existing_preferences_and_fill_missing_defaults() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    let read = |db: &Connection| -> Result<Value> {
        let stored: String = db.query_row(
            "SELECT value_json FROM preferences WHERE key = 'agent.modelPreferences'",
            [],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&stored)?)
    };
    let written = json!({"haiku": {"providerId": "direct", "model": "fast"}});
    let assignments = serde_json::from_value(written.clone())?;
    assert!(read_claude_role_assignments(&db).is_empty());
    save_claude_role_assignments(&mut db, &assignments, 2)?;
    assert_eq!(
        read(&db)?,
        json!({
            "claudeRoleAssignments": written,
            "contextWindows": {},
            "compaction": {"mode": "follow", "effort": "follow"}
        })
    );

    let legacy = json!({
        "claudeWebSearch": {"providerId": "other", "model": "web"},
        "contextWindows": {"direct": "512k"},
        "compaction": {"mode": "custom", "effort": "high", "providerId": "other", "model": "compact"},
        "otherSetting": {"enabled": true}
    });
    db.execute(
        "UPDATE preferences SET value_json = ? WHERE key = 'agent.modelPreferences'",
        [legacy.to_string()],
    )?;
    assert!(read_claude_role_assignments(&db).is_empty());
    save_claude_role_assignments(&mut db, &assignments, 3)?;
    let mut expected = legacy;
    expected.as_object_mut().unwrap().remove("otherSetting");
    expected["claudeRoleAssignments"] = written.clone();
    assert_eq!(read(&db)?, expected);
    assert_eq!(
        serde_json::to_value(read_claude_role_assignments(&db))?,
        written
    );
    save_claude_role_assignments(&mut db, &Default::default(), 4)?;
    expected["claudeRoleAssignments"] = json!({});
    assert_eq!(read(&db)?, expected);

    let incomplete =
        json!({"claudeWebSearch": expected["claudeWebSearch"], "claudeRoleAssignments": written});
    db.execute(
        "UPDATE preferences SET value_json = ? WHERE key = 'agent.modelPreferences'",
        [incomplete.to_string()],
    )?;
    assert!(read_claude_role_assignments(&db).is_empty());
    save_claude_role_assignments(&mut db, &assignments, 5)?;
    assert_eq!(
        read(&db)?,
        json!({
            "contextWindows": {},
            "compaction": {"mode": "follow", "effort": "follow"},
            "claudeRoleAssignments": written
        })
    );
    Ok(())
}

#[test]
fn invalid_sibling_preferences_reset_the_entire_row_before_read_and_save() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    let valid = json!({
        "contextWindows": {"direct": "512k"},
        "compaction": {"mode": "custom", "effort": "high", "providerId": "other", "model": "compact"},
        "claudeWebSearch": {"providerId": "other", "model": "web"},
        "claudeRoleAssignments": {"opus": {"providerId": "direct", "model": "assigned"}}
    });
    let written = json!({"haiku": {"providerId": "direct", "model": "fast"}});
    let assignments = serde_json::from_value(written.clone())?;
    for (field, invalid) in [
        ("contextWindows", json!({"direct": "invalid"})),
        ("compaction", json!({"mode": "custom", "effort": "high"})),
        ("claudeWebSearch", json!({"providerId": "other"})),
    ] {
        let mut stored = valid.clone();
        stored[field] = invalid;
        db.execute(
            "REPLACE INTO preferences (key, value_json, updated_at) VALUES ('agent.modelPreferences', ?, 1)",
            [stored.to_string()],
        )?;
        assert!(read_claude_role_assignments(&db).is_empty(), "{field}");
        save_claude_role_assignments(&mut db, &assignments, 2)?;
        let saved: String = db.query_row(
            "SELECT value_json FROM preferences WHERE key = 'agent.modelPreferences'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            serde_json::from_str::<Value>(&saved)?,
            json!({
                "contextWindows": {},
                "compaction": {"mode": "follow", "effort": "follow"},
                "claudeRoleAssignments": written
            }),
            "{field}"
        );
    }
    Ok(())
}

#[test]
fn malformed_persisted_role_assignments_use_the_entire_default() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    let valid = json!({"providerId": "direct", "model": "fast"});
    for assignments in [
        json!({"opus": valid, "other": valid}),
        json!({"opus": valid, "sonnet": {"providerId": "direct", "model": false}}),
        json!({"opus": valid, "sonnet": null}),
        json!([valid]),
        json!(false),
    ] {
        let stored = json!({
            "contextWindows": {},
            "compaction": {"mode": "follow", "effort": "follow"},
            "claudeRoleAssignments": assignments
        });
        db.execute(
            "REPLACE INTO preferences (key, value_json, updated_at) VALUES ('agent.modelPreferences', ?, 1)",
            [stored.to_string()],
        )?;
        assert!(read_claude_role_assignments(&db).is_empty(), "{stored}");
    }
    db.execute(
        "UPDATE preferences SET value_json = 'false' WHERE key = 'agent.modelPreferences'",
        [],
    )?;
    assert!(read_claude_role_assignments(&db).is_empty());
    Ok(())
}

#[derive(Deserialize)]
struct Reference {
    bootstrap: Bootstrap,
    tables: Vec<String>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    input: BootstrapWindowInput,
    command: CommandMetadata,
    ids: HierarchyIds,
    committed_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    op: String,
    input: Value,
    command: CommandMetadata,
    committed_at: i64,
    result: Option<Value>,
    error: Option<String>,
    snapshot: BTreeMap<String, Vec<Value>>,
}

#[test]
fn shell_lifecycle_matches_frozen_source_across_reopen() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    assert_eq!(reference.tables, TABLES);
    assert_eq!(reference.steps.len(), 22);
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("matou.sqlite");
    let mut db = open(&path)?;
    bootstrap_window(
        &mut db,
        &reference.bootstrap.command,
        &reference.bootstrap.input,
        &reference.bootstrap.ids,
        "默认",
        reference.bootstrap.committed_at,
    )?;

    for (index, step) in reference.steps.into_iter().enumerate() {
        let label = format!("{} {} ({})", index + 1, step.op, step.command.command_id);
        if step.op == "reopen" {
            db.close().map_err(|(_, error)| error)?;
            db = open(&path)?;
            assert!(get_run(&db, "missing")?.is_none());
            assert_eq!(
                get_run(&db, "run-2")?,
                list_runs(&db, "shell-2")?.get(1).cloned()
            );
            let reopened = json!({
                "sessions": list_sessions(&db)?,
                "runs": list_all_runs(&db)?,
                "sessionRuns": list_runs(&db, "shell-2")?,
                "missing": get_session(&db, "missing")?,
            });
            assert_eq!(Some(reopened), step.result, "{label}");
        } else {
            let observed = apply(&mut db, &step);
            match (observed, step.result, step.error) {
                (Ok(commit), Some(expected), None) => {
                    assert_eq!(serde_json::to_value(commit)?, expected, "{label}");
                }
                (Err(error), None, Some(expected)) => {
                    assert_eq!(error.to_string(), expected, "{label}");
                }
                (actual, result, error) => {
                    bail!(
                        "{label}: expected result {result:?}, error {error:?}, observed {actual:?}"
                    );
                }
            }
        }
        for table in TABLES {
            compare_rows(
                &db,
                &format!("SELECT * FROM {table} ORDER BY rowid"),
                step.snapshot
                    .get(table)
                    .with_context(|| format!("{label}: fixture {table}"))?,
                &label,
            )?;
        }
    }
    Ok(())
}

fn apply(db: &mut Connection, step: &Step) -> Result<DomainCommit> {
    let Step {
        op,
        input,
        command,
        committed_at,
        ..
    } = step;
    let string = |key: &str| {
        input[key]
            .as_str()
            .with_context(|| format!("missing {key}"))
    };
    let integer = |key: &str| {
        input[key]
            .as_i64()
            .with_context(|| format!("missing {key}"))
    };
    let now = integer("now")?;
    match op.as_str() {
        "create" => create_shell_session(
            db,
            command,
            &serde_json::from_value::<CreateShellSessionInput>(input.clone())?,
            *committed_at,
        ),
        "start" => start_run(
            db,
            command,
            &serde_json::from_value::<StartRunInput>(input.clone())?,
            *committed_at,
        ),
        "finish" => finish_run(
            db,
            command,
            string("runId")?,
            integer("exitCode")?,
            input["signal"].as_i64(),
            now,
            *committed_at,
        ),
        "interrupt" => interrupt_run(db, command, string("runId")?, now, *committed_at),
        "rename" => rename_session(
            db,
            command,
            string("sessionId")?,
            string("title")?,
            now,
            *committed_at,
        ),
        "cwd" => update_cwd(
            db,
            command,
            string("sessionId")?,
            string("cwd")?,
            now,
            *committed_at,
        ),
        "archive" => archive_session(db, command, string("sessionId")?, now, *committed_at),
        _ => bail!("unexpected fixture operation {op}"),
    }
}

fn open(path: &Path) -> Result<Connection> {
    let mut db = Connection::open(path)?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    Ok(db)
}

fn read_rows(db: &Connection, sql: &str) -> Result<Vec<Value>> {
    let mut statement = db.prepare(sql)?;
    let columns: Vec<_> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut rows = statement.query([])?;
    let mut mapped = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = Map::new();
        for (index, column) in columns.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => json!(value),
                ValueRef::Real(value) => json!(value),
                ValueRef::Text(_) => json!(row.get::<_, String>(index)?),
                ValueRef::Blob(_) => bail!("unexpected BLOB column {column} in {sql}"),
            };
            object.insert(column.clone(), value);
        }
        mapped.push(Value::Object(object));
    }
    Ok(mapped)
}

fn normalize_json_columns(rows: &mut [Value]) -> Result<()> {
    for row in rows {
        for (column, value) in row.as_object_mut().context("expected an SQLite row")? {
            if column.ends_with("_json")
                && let Some(text) = value.as_str()
            {
                let parsed: Value = serde_json::from_str(text)
                    .with_context(|| format!("invalid JSON in {column}"))?;
                // The wrapper preserves SQL NULL versus JSON text containing null.
                *value = json!({ "json": parsed });
            }
        }
    }
    Ok(())
}

fn compare_rows(db: &Connection, sql: &str, expected: &[Value], step: &str) -> Result<()> {
    let mut actual = read_rows(db, sql)?;
    let mut expected = expected.to_vec();
    normalize_json_columns(&mut actual)?;
    normalize_json_columns(&mut expected)?;
    assert_eq!(actual, expected, "{step}: {sql}");
    Ok(())
}

#[test]
fn shell_promotion_updates_the_existing_card_with_frozen_title_rules() -> Result<()> {
    use matou_native_core::sessions::{promote_shell_to_claude, return_promoted_claude_to_shell};

    let seed = serde_json::from_str::<Reference>(REFERENCE)?.bootstrap;
    let mut db = open(Path::new(":memory:"))?;
    bootstrap_window(
        &mut db,
        &seed.command,
        &seed.input,
        &seed.ids,
        "默认",
        seed.committed_at,
    )?;
    let id = &seed.ids.session_id;
    let command = |name: &str| CommandMetadata {
        command_id: name.into(),
        command_type: name.into(),
        request_hash: name.into(),
        causation_id: None,
        correlation_id: None,
    };
    let mounts = read_rows(&db, "SELECT * FROM session_mounts")?;
    let memberships = read_rows(&db, "SELECT * FROM session_canvas_memberships")?;
    let read_card = |db: &Connection| {
        db.query_row(
            "SELECT kind, title, title_source, status, work_status FROM sessions WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
    };
    for (index, (title, source, promoted_title, rollback_title)) in [
        ("Shell", "default", "Claude", "Shell"),
        ("My workspace", "manual", "My workspace", "My workspace"),
        ("Claude", "manual", "Claude", "Shell"),
        (id.as_str(), "default", "Claude", "Shell"),
        ("Shell", "manual", "Claude", "Shell"),
        (id.as_str(), "manual", "Claude", "Shell"),
        ("Codex", "manual", "Codex", "Shell"),
        ("DSH", "manual", "DSH", "Shell"),
        ("Antigravity", "manual", "Antigravity", "Shell"),
    ]
    .into_iter()
    .enumerate()
    {
        db.execute(
            "UPDATE sessions SET title=?, title_source=?, status='exited' WHERE id=?",
            [title, source, id],
        )?;
        promote_shell_to_claude(&mut db, &command(&format!("promote-{index}")), id, 100)?;
        assert_eq!(
            read_card(&db)?,
            (
                "claude-code".into(),
                promoted_title.into(),
                source.into(),
                "exited".into(),
                "starting".into()
            )
        );
        return_promoted_claude_to_shell(&mut db, &command(&format!("rollback-{index}")), id, 101)?;
        assert_eq!(
            read_card(&db)?,
            (
                "shell".into(),
                rollback_title.into(),
                source.into(),
                "exited".into(),
                "idle".into()
            )
        );
    }
    promote_shell_to_claude(&mut db, &command("promote-before-rename"), id, 102)?;
    db.execute("UPDATE sessions SET title=id WHERE id=?", [id])?;
    return_promoted_claude_to_shell(&mut db, &command("rollback-after-rename"), id, 103)?;
    assert_eq!(read_card(&db)?.1, "Shell");
    assert_eq!(list_sessions(&db)?.len(), 1);
    assert_eq!(read_rows(&db, "SELECT * FROM session_mounts")?, mounts);
    assert_eq!(
        read_rows(&db, "SELECT * FROM session_canvas_memberships")?,
        memberships
    );
    assert!(read_rows(&db, "SELECT * FROM provider_bindings")?.is_empty());
    Ok(())
}

#[test]
fn provider_binding_requires_current_run_and_validation_replays_once() -> Result<()> {
    use matou_native_core::sessions::{ClaudeIdentity, record_claude_identity};

    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let temporary = tempfile::tempdir()?;
    let mut db = open(&temporary.path().join("matou.sqlite"))?;
    bootstrap_window(
        &mut db,
        &reference.bootstrap.command,
        &reference.bootstrap.input,
        &reference.bootstrap.ids,
        "默认",
        reference.bootstrap.committed_at,
    )?;
    let session = reference.bootstrap.ids.session_id;
    db.execute(
        "UPDATE sessions SET kind='claude-code', work_status='needs-input' WHERE id=?",
        [&session],
    )?;
    let command = |id: &str| CommandMetadata {
        command_id: id.into(),
        command_type: "test.provider-identity".into(),
        request_hash: id.into(),
        causation_id: None,
        correlation_id: None,
    };
    for (id, generation) in [("older", "previous"), ("current", "active")] {
        start_run(
            &mut db,
            &command(id),
            &StartRunInput {
                id: id.into(),
                session_id: session.clone(),
                runtime_generation: generation.into(),
                pid: None,
                cols: 80,
                rows: 24,
                now: 10,
            },
            10,
        )?;
    }
    let work = |db: &Connection| -> rusqlite::Result<String> {
        db.query_row(
            "SELECT work_status FROM sessions WHERE id=?",
            [&session],
            |row| row.get(0),
        )
    };
    assert_eq!(work(&db)?, "needs-input");
    db.execute(
        "INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, invalidated_at, restore_state, restore_error
         ) VALUES ('existing', ?, 'claude-code', 'from-cli', 'failed',
                   '{\"unrelated\":true,\"invalidationReason\":\"fixture\"}', 2, 3, 3, 'failed', 'fixture')",
        [&session],
    )?;
    let metadata = json!({"providerConfigId": "configuration", "model": "launch-model",
                          "permissionMode": "default", "cwd": "/launch"});
    let observed = json!({"model": "observed-model", "permissionMode": "plan", "cwd": "/observed"});
    let mut input = ClaudeIdentity {
        binding_id: "unused-on-update",
        session_id: &session,
        run_id: "older",
        generation: "previous",
        provider_session_id: "from-cli",
        expected_id: None,
        event_name: "UserPromptSubmit",
        launch_metadata: &metadata,
        observed_metadata: &observed,
        first_for_run: true,
    };
    assert!(record_claude_identity(&mut db, &command("stale"), &input, 11).is_err());
    input.run_id = "current";
    assert!(record_claude_identity(&mut db, &command("wrong-generation"), &input, 12).is_err());
    assert_eq!(work(&db)?, "needs-input");
    input.generation = "active";
    let first = record_claude_identity(&mut db, &command("prompt"), &input, 13)?;
    let replay = record_claude_identity(&mut db, &command("prompt"), &input, 14)?;
    assert!(replay.replayed);
    assert_eq!(first.result, replay.result);
    assert_eq!(first.result["bindingId"], "existing");
    assert_eq!(first.result["metadata"]["observedNormalStop"], false);
    assert!(first.result["metadata"].get("unrelated").is_none());
    assert_eq!(work(&db)?, "needs-input");
    input.event_name = "Stop";
    input.observed_metadata = &Value::Null;
    input.first_for_run = false;
    record_claude_identity(&mut db, &command("stop"), &input, 15)?;
    assert_eq!(work(&db)?, "needs-input");
    let mut rows = db.prepare("SELECT * FROM provider_bindings")?;
    let row = rows.query_row([], |row| {
        Ok(json!({
            "id": row.get::<_, String>("id")?,
            "created": row.get::<_, i64>("created_at")?,
            "validated": row.get::<_, i64>("validated_at")?,
            "invalidated": row.get::<_, Option<i64>>("invalidated_at")?,
            "resume": row.get::<_, String>("resume_state")?,
            "restore": row.get::<_, String>("restore_state")?,
            "error": row.get::<_, Option<String>>("restore_error")?,
            "metadata": row.get::<_, String>("metadata_json")?,
        }))
    })?;
    assert_eq!(row["id"], "existing");
    assert_eq!(row["created"], 2);
    assert_eq!(row["validated"], 15);
    assert_eq!(row["invalidated"], Value::Null);
    assert_eq!(row["resume"], "available");
    assert_eq!(row["restore"], "none");
    assert_eq!(row["error"], Value::Null);
    assert_eq!(
        serde_json::from_str::<Value>(row["metadata"].as_str().unwrap())?,
        json!({"providerConfigId": "configuration", "model": "observed-model",
                      "permissionMode": "plan", "cwd": "/observed", "lastHookEvent": "Stop",
                      "observedUserPrompt": true, "observedNormalStop": true, "canFork": true, "unrelated": true})
    );
    let count = |query| db.query_row(query, [], |row| row.get::<_, i64>(0));
    assert_eq!(count("SELECT COUNT(*) FROM provider_bindings")?, 1);
    assert_eq!(
        count("SELECT COUNT(*) FROM domain_events WHERE event_type='provider-binding.recorded'")?,
        2
    );
    drop(rows);
    use matou_native_core::sessions::{ClaudeRestore, change_claude_restore, claude_binding};
    let change = |db: &mut Connection, id, change, time| {
        change_claude_restore(db, &command(id), &session, change, time)
    };
    assert!(claude_binding(&db, &session)?.unwrap().resumable);
    assert!(change(&mut db, "begin", ClaudeRestore::Begin("existing"), 16)?.is_some());
    input.first_for_run = true;
    input.expected_id = Some("from-cli");
    for (id, event, actual) in [
        ("case", "SessionStart", "FROM-CLI"),
        ("space", "SessionStart", "from-cli "),
        ("end", "SessionEnd", "from-cli"),
        ("statusline", "unknown", "from-cli"),
    ] {
        input.event_name = event;
        input.provider_session_id = actual;
        assert!(record_claude_identity(&mut db, &command(id), &input, 17).is_err());
    }
    assert_eq!(
        claude_binding(&db, &session)?.unwrap().restore_state,
        "restoring"
    );
    input.event_name = "SessionStart";
    input.provider_session_id = "from-cli";
    let restored = record_claude_identity(&mut db, &command("resumed"), &input, 18)?;
    assert_eq!(restored.result["bindingId"], "existing");
    assert_eq!(restored.result["restoreState"], "none");
    assert_eq!(restored.result["restoreError"], Value::Null);

    let fail = ClaudeRestore::Fail {
        binding_id: "existing",
        error: "provider process could not start: fixture",
    };
    let failed = change(&mut db, "failed", fail, 19)?.unwrap();
    assert_eq!(
        failed.result["restoreError"],
        "provider process could not start: fixture"
    );
    assert_eq!(work(&db)?, "error");
    assert!(!claude_binding(&db, &session)?.unwrap().resumable);
    assert!(change(&mut db, "auto", ClaudeRestore::Begin("existing"), 20)?.is_none());
    let retry = change(&mut db, "retry", ClaudeRestore::Retry, 21)?.unwrap();
    assert_eq!(retry.result["providerSessionId"], "from-cli");
    assert_eq!(retry.result["restoreState"], "restoring");
    assert_eq!(retry.result["restoreError"], Value::Null);
    assert_eq!(retry.result["spawnRevision"], 1);
    assert!(change(&mut db, "duplicate", ClaudeRestore::Retry, 22)?.is_none());
    assert!(change(&mut db, "failed-again", fail, 23)?.is_some());
    let fresh = change(&mut db, "fresh", ClaudeRestore::Fresh, 24)?.unwrap();
    assert_eq!(fresh.result["spawnRevision"], 2);
    assert_eq!(fresh.result["restoreState"], "none");
    assert!(change(&mut db, "duplicate-fresh", ClaudeRestore::Fresh, 25)?.is_none());
    assert_eq!(db.query_row(
        "SELECT resume_state, invalidated_at, restore_error FROM provider_bindings WHERE id='existing'",
        [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<String>>(2)?)),
    )?, ("expired".into(), 23, None));
    Ok(())
}

#[test]
fn catalog_binding_replaces_available_history_without_validating_the_new_id() -> Result<()> {
    use matou_native_core::sessions::{
        ClaudeCatalogLoad, ClaudeRestore, change_claude_restore, claude_binding,
        load_claude_catalog_binding,
    };

    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let seed = reference.bootstrap;
    let mut db = open(Path::new(":memory:"))?;
    bootstrap_window(
        &mut db,
        &seed.command,
        &seed.input,
        &seed.ids,
        "默认",
        seed.committed_at,
    )?;
    let session_id = seed.ids.session_id.as_str();
    let cwd = get_session(&db, session_id)?.context("seed session")?.cwd;
    db.execute(
        "UPDATE sessions SET kind='claude-code' WHERE id=?",
        [session_id],
    )?;
    db.execute(
        "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id,
            resume_state, metadata_json, created_at, updated_at, validated_at)
         VALUES ('old', ?, 'claude-code', 'old-conversation', 'available',
                 '{\"spawnRevision\":7}', 9, 9, 9)",
        [session_id],
    )?;
    let command = |name: &str| CommandMetadata {
        command_id: name.into(),
        command_type: "session.load-provider-catalog".into(),
        request_hash: name.into(),
        causation_id: None,
        correlation_id: None,
    };
    let input = ClaudeCatalogLoad {
        binding_id: "catalog-binding",
        session_id,
        scene_id: &seed.ids.scene_id,
        cwd: &cwd,
        provider_session_id: "catalog-conversation",
        title: "Previously saved conversation",
        model: Some("test-model"),
        permission_mode: "plan",
    };
    assert!(
        load_claude_catalog_binding(
            &mut db,
            &command("wrong-scene"),
            &ClaudeCatalogLoad {
                scene_id: "another",
                ..input
            },
            10
        )
        .is_err()
    );
    assert!(
        load_claude_catalog_binding(
            &mut db,
            &command("changed-cwd"),
            &ClaudeCatalogLoad {
                cwd: "/changed",
                ..input
            },
            10
        )
        .is_err()
    );
    assert_eq!(claude_binding(&db, session_id)?.unwrap().id, "old");

    load_claude_catalog_binding(&mut db, &command("load"), &input, 10)?;
    let binding = claude_binding(&db, session_id)?.unwrap();
    assert_eq!(binding.id, "catalog-binding");
    assert_eq!(binding.provider_session_id, "catalog-conversation");
    assert_eq!(binding.restore_state, "none");
    assert!(binding.resumable);
    assert_eq!(binding.metadata["loadedFromCatalog"], true);
    assert_eq!(binding.metadata["model"], "test-model");
    assert_eq!(binding.metadata["permissionMode"], "plan");
    assert_eq!(binding.metadata["spawnRevision"], 8);
    let rows = read_rows(
        &db,
        "SELECT id, resume_state, validated_at, invalidated_at FROM provider_bindings ORDER BY id",
    )?;
    assert_eq!(
        rows,
        [
            json!({"id": "catalog-binding", "resume_state": "available", "validated_at": null, "invalidated_at": null}),
            json!({"id": "old", "resume_state": "expired", "validated_at": 9, "invalidated_at": 10})
        ]
    );
    let session = get_session(&db, session_id)?.unwrap();
    assert_eq!(session.title, input.title);
    assert_eq!(session.status, "starting");

    change_claude_restore(
        &mut db,
        &command("begin"),
        session_id,
        ClaudeRestore::Begin(&binding.id),
        11,
    )?;
    assert_eq!(
        claude_binding(&db, session_id)?.unwrap().restore_state,
        "restoring"
    );

    start_run(
        &mut db,
        &command("run"),
        &StartRunInput {
            id: "running".into(),
            session_id: session_id.into(),
            runtime_generation: "generation".into(),
            pid: None,
            cols: 80,
            rows: 24,
            now: 12,
        },
        12,
    )?;
    for status in ["running", "starting"] {
        db.execute(
            "UPDATE session_runs SET status = ? WHERE id = 'running'",
            [status],
        )?;
        let error = load_claude_catalog_binding(&mut db, &command(status), &input, 13).unwrap_err();
        assert_eq!(error.to_string(), "Card already has a running process");
    }
    assert_eq!(
        claude_binding(&db, session_id)?.unwrap().metadata["spawnRevision"],
        8
    );
    Ok(())
}
