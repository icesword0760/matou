use std::{collections::BTreeMap, fs};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    hierarchy::{
        canvas::CanvasError,
        tabs::{close_scene, rename_scene, reorder_scene},
    },
    storage::{CommandMetadata, CommandReplayConflictError, configure_connection},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const REFERENCE: &str = include_str!("fixtures/scene-tabs-reference.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    messages: Messages,
    seed_base64: String,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Messages {
    duplicate_scene_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Rename,
    Reorder,
    Close,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    op: Operation,
    label: String,
    input: Value,
    sql: Vec<String>,
    command: CommandMetadata,
    result: Option<Value>,
    error: Option<String>,
    error_code: Option<String>,
    committed_at: i64,
    snapshot: BTreeMap<String, Vec<Value>>,
}

#[test]
fn scene_tab_transactions_match_the_frozen_database() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    let mut reopened = false;
    for step in reference.steps {
        let label = &step.label;
        for sql in step.sql {
            db.execute_batch(&sql).with_context(|| label.clone())?;
        }
        let actual = match step.op {
            Operation::Rename => rename_scene(
                &mut db,
                &step.command,
                &serde_json::from_value(step.input)?,
                &reference.messages.duplicate_scene_name,
                step.committed_at,
            ),
            Operation::Reorder => reorder_scene(
                &mut db,
                &step.command,
                &serde_json::from_value(step.input)?,
                step.committed_at,
            ),
            Operation::Close => close_scene(
                &mut db,
                &step.command,
                &serde_json::from_value(step.input)?,
                step.committed_at,
            ),
        };
        let should_reopen = !reopened
            && step
                .result
                .as_ref()
                .is_some_and(|result| result["action"] == "closed");
        match (actual, step.result, step.error) {
            (Ok(actual), Some(expected), None) => assert_eq!(actual, expected, "{label}: result"),
            (Err(actual), None, Some(expected)) => {
                assert_eq!(actual.to_string(), expected, "{label}: error message");
                let code = actual
                    .downcast_ref::<CanvasError>()
                    .map(|error| error.code)
                    .or_else(|| {
                        actual
                            .downcast_ref::<CommandReplayConflictError>()
                            .map(|_| CommandReplayConflictError::CODE)
                    });
                assert_eq!(code, step.error_code.as_deref(), "{label}: error code");
            }
            (actual, expected, error) => {
                bail!("{label}: expected result {expected:?}, error {error:?}, got {actual:?}")
            }
        }
        if should_reopen {
            db.close().map_err(|(_, error)| error)?;
            db = Connection::open(&path)?;
            configure_connection(&db)?;
            reopened = true;
        }
        for (table, expected) in step.snapshot {
            let actual = read_rows(&db, &table).with_context(|| format!("{label}: {table}"))?;
            assert_eq!(
                normalize_json_columns(actual)?,
                normalize_json_columns(expected)?,
                "{label}: table {table}"
            );
        }
    }
    assert!(
        reopened,
        "reference fixture must include a successful scene close"
    );
    Ok(())
}

fn read_rows(db: &Connection, table: &str) -> Result<Vec<Value>> {
    let mut statement = db.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
    let columns: Vec<_> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut rows = statement.query([])?;
    let mut output = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = Map::new();
        for (index, column) in columns.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => json!(value),
                ValueRef::Real(value) => json!(value),
                ValueRef::Text(_) => json!(row.get::<_, String>(index)?),
                ValueRef::Blob(_) => bail!("unexpected BLOB column {column} in {table}"),
            };
            object.insert(column.clone(), value);
        }
        output.push(Value::Object(object));
    }
    Ok(output)
}

fn normalize_json_columns(mut rows: Vec<Value>) -> Result<Vec<Value>> {
    for row in &mut rows {
        for (column, value) in row
            .as_object_mut()
            .context("expected an SQLite row object")?
        {
            if column.ends_with("_json")
                && let Some(text) = value.as_str()
            {
                *value = serde_json::from_str(text)
                    .with_context(|| format!("invalid JSON in {column}"))?;
            }
        }
    }
    Ok(rows)
}
