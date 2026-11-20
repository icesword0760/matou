use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail};
use matou_native_core::{
    hierarchy::{BootstrapWindowInput, HierarchyIds, bootstrap_window, read_hierarchy},
    storage::{CommandMetadata, configure_connection, migrate},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const REFERENCE: &str = include_str!("fixtures/bootstrap-reference.json");

#[derive(Deserialize)]
struct Reference {
    tables: Vec<String>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    name: String,
    input: BootstrapWindowInput,
    command: CommandMetadata,
    ids: Option<HierarchyIds>,
    result: Option<Value>,
    error: Option<String>,
    snapshot: BTreeMap<String, Vec<Value>>,
    events: Vec<Value>,
    receipts: Vec<Value>,
}

fn open_database(path: &Path) -> Result<Connection> {
    let mut db = Connection::open(path)?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    Ok(db)
}

fn read_rows(db: &Connection, sql: &str) -> Result<Vec<Value>> {
    let mut statement = db.prepare(sql)?;
    let names: Vec<_> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut rows = statement.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let mut mapped = Map::new();
        for (index, name) in names.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => json!(value),
                ValueRef::Real(value) => json!(value),
                ValueRef::Text(_) => json!(row.get::<_, String>(index)?),
                ValueRef::Blob(_) => bail!("unexpected BLOB column {name} in {sql}"),
            };
            mapped.insert(name.clone(), value);
        }
        result.push(Value::Object(mapped));
    }
    Ok(result)
}

fn parse_json_columns(rows: &mut [Value]) -> Result<()> {
    for row in rows {
        for (column, value) in row.as_object_mut().context("expected an SQLite row")? {
            if column.ends_with("_json") {
                if let Some(text) = value.as_str() {
                    let parsed: Value = serde_json::from_str(text)
                        .with_context(|| format!("invalid JSON in {column}"))?;
                    // Keep SQLite NULL distinct from JSON text containing null.
                    *value = json!({ "json": parsed });
                }
            }
        }
    }
    Ok(())
}

fn matching_rows(db: &Connection, sql: &str, expected: &[Value], step: &str) -> Result<()> {
    let mut actual = read_rows(db, sql)?;
    let mut expected = expected.to_vec();
    parse_json_columns(&mut actual)?;
    parse_json_columns(&mut expected)?;
    assert_eq!(actual, expected, "{step}: {sql}");
    Ok(())
}

fn uncommitted_ids() -> HierarchyIds {
    let id = |index| format!("00000000-0000-4000-8000-{index:012}");
    HierarchyIds {
        workspace_id: id(1),
        execution_context_id: id(2),
        task_id: id(3),
        scene_id: id(4),
        root_node_id: id(5),
        session_id: id(6),
        mount_id: id(7),
    }
}

#[test]
fn bootstrap_matches_frozen_source_at_each_step() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("matou.sqlite");
    let mut db = open_database(&path)?;

    let empty = read_hierarchy(&db, "unregistered-window")?;
    assert_eq!(
        serde_json::to_value(empty)?,
        json!({
            "workspace": null, "executionContext": null, "task": null,
            "scene": null, "session": null, "mount": null,
            "navigation": { "windowId": "unregistered-window", "taskByWorkspace": {},
                            "sceneByTask": {}, "sessionByScene": {} }
        })
    );

    // A failed command has no ID or timestamp stored in the frozen database.
    let uncommitted_ids = uncommitted_ids();
    let mut previous_window_result: Option<(&str, &Value)> = None;

    for step in &reference.steps {
        if step.name == "reopen" {
            let (window_id, expected) =
                previous_window_result.context("no result before reopen")?;
            drop(db);
            db = open_database(&path)?;
            let reopened = read_hierarchy(&db, window_id)?;
            assert_eq!(
                &serde_json::to_value(reopened)?,
                expected,
                "initial read after reopen"
            );
        }

        let committed_at = step
            .receipts
            .iter()
            .find(|row| row["command_id"].as_str() == Some(step.command.command_id.as_str()))
            .and_then(|receipt| receipt["committed_at"].as_i64());
        assert_eq!(
            committed_at.is_some(),
            step.result.is_some(),
            "{}: frozen receipt",
            step.name
        );

        let got = bootstrap_window(
            &mut db,
            &step.command,
            &step.input,
            step.ids.as_ref().unwrap_or(&uncommitted_ids),
            "默认",
            committed_at.unwrap_or(step.input.now),
        );

        match (&step.result, &step.error) {
            (Some(expected), None) => {
                let result = got.with_context(|| step.name.clone())?;
                assert_eq!(
                    &serde_json::to_value(result)?,
                    expected,
                    "{}: result",
                    step.name
                );
                let readback = read_hierarchy(&db, &step.input.window_id)?;
                assert_eq!(
                    &serde_json::to_value(readback)?,
                    expected,
                    "{}: readback",
                    step.name
                );
                previous_window_result = Some((&step.input.window_id, expected));
            }
            (None, Some(expected)) => {
                assert_eq!(
                    &got.unwrap_err().to_string(),
                    expected,
                    "{}: error",
                    step.name
                );
            }
            _ => bail!("{}: missing or conflicting frozen outcomes", step.name),
        }

        for table in &reference.tables {
            matching_rows(
                &db,
                &format!("SELECT * FROM {table} ORDER BY rowid"),
                &step.snapshot[table],
                &step.name,
            )?;
        }
        matching_rows(
            &db,
            "SELECT * FROM domain_events ORDER BY seq",
            &step.events,
            &step.name,
        )?;
        matching_rows(
            &db,
            "SELECT * FROM command_deduplication ORDER BY rowid",
            &step.receipts,
            &step.name,
        )?;
    }
    Ok(())
}

#[test]
#[ignore = "run by the gate with a freshly exported frozen transaction fixture"]
fn export_bootstrap_gate_transactions() -> Result<()> {
    let gate = std::env::var("MATOU_BOOTSTRAP_GATE")?;
    let gate = Path::new(&gate);
    let reference: Reference =
        serde_json::from_slice(&fs::read(gate.join("frozen-transaction.json"))?)?;
    let directory = tempfile::tempdir_in(gate.join(".work").join("tmp"))?;
    let path = directory.path().join("native.sqlite");
    let mut db = open_database(&path)?;
    let mut captures = Vec::new();

    for step in &reference.steps {
        if step.name == "reopen" {
            drop(db);
            db = open_database(&path)?;
        }
        // Use the frozen IDs when available. Supply distinct fixture IDs for
        // rejected commands so an unexpected insert remains visible in SQL.
        let ids = step.ids.clone().unwrap_or_else(uncommitted_ids);
        let outcome = bootstrap_window(
            &mut db,
            &step.command,
            &step.input,
            &ids,
            "默认",
            step.input.now + 10,
        );
        let (result, error) = match outcome {
            Ok(result) => (Some(result), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let mut snapshot = Map::new();
        for table in &reference.tables {
            let sql = format!("SELECT * FROM {table} ORDER BY rowid");
            snapshot.insert(table.clone(), json!(read_rows(&db, &sql)?));
        }
        let mut record = json!({
            "name": step.name,
            "input": step.input,
            "command": step.command,
            "snapshot": snapshot,
            "events": read_rows(&db, "SELECT * FROM domain_events ORDER BY seq")?,
            "receipts": read_rows(&db, "SELECT * FROM command_deduplication ORDER BY rowid")?,
        });
        if let Some(result) = result {
            record["result"] = json!(result);
        }
        if let Some(error) = error {
            record["error"] = json!(error);
        }
        match read_hierarchy(&db, &step.input.window_id) {
            Ok(readback) => record["readback"] = json!(readback),
            Err(error) => record["readbackError"] = json!(error.to_string()),
        }
        captures.push(record);
    }
    let report = json!({
        "source": "Native hierarchy on real SQLite; shared command inputs, source IDs, and fixed commit clock.",
        "sqlite": rusqlite::version(),
        "tables": reference.tables,
        "steps": captures,
    });
    fs::write(
        gate.join("native-transaction.json"),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    Ok(())
}
