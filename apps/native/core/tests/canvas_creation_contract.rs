use std::{collections::BTreeMap, fs};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    hierarchy::canvas::{
        CanvasError, CanvasIds, CanvasMessages, create_canvas, create_shell_sibling,
    },
    storage::{CommandMetadata, configure_connection},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[path = "common/frozen_graph.rs"]
mod frozen_graph;
use frozen_graph::apply_permission_default;

const REFERENCE: &str = include_str!("fixtures/canvas-creation-reference.json");

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
    new_canvas: String,
    duplicate_scene_name: String,
    workspace_path_invalid: String,
    detached_session_must_return: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Canvas,
    Sibling,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    op: Operation,
    label: String,
    input: Value,
    sql: Vec<String>,
    command: CommandMetadata,
    ids: CanvasIds,
    result: Option<Value>,
    error: Option<String>,
    error_code: Option<String>,
    committed_at: i64,
    snapshot: BTreeMap<String, Vec<Value>>,
}

#[test]
fn canvas_and_shell_creation_match_the_frozen_database() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let messages = CanvasMessages {
        new_canvas: &reference.messages.new_canvas,
        duplicate_scene_name: &reference.messages.duplicate_scene_name,
        workspace_path_invalid: &reference.messages.workspace_path_invalid,
        detached_session_must_return: &reference.messages.detached_session_must_return,
    };
    let mut db = Connection::open(path)?;
    configure_connection(&db)?;
    for step in reference.steps {
        let label = &step.label;
        for sql in step.sql {
            db.execute_batch(&sql).with_context(|| label.clone())?;
        }
        let actual = match step.op {
            Operation::Canvas => create_canvas(
                &mut db,
                &step.command,
                &serde_json::from_value(step.input)?,
                &step.ids,
                &messages,
                step.committed_at,
            ),
            Operation::Sibling => create_shell_sibling(
                &mut db,
                &step.command,
                &serde_json::from_value(step.input)?,
                &step.ids,
                &messages,
                step.committed_at,
            ),
        };
        match (actual, step.result, step.error) {
            (Ok(actual), Some(mut expected), None) => {
                apply_permission_default(&mut expected);
                assert_eq!(actual, expected, "{label}: result");
            }
            (Err(actual), None, Some(expected)) => {
                assert_eq!(actual.to_string(), expected, "{label}: error message");
                assert_eq!(
                    actual.downcast_ref::<CanvasError>().map(|error| error.code),
                    step.error_code.as_deref(),
                    "{label}: error code"
                );
            }
            (actual, expected, error) => {
                bail!("{label}: expected result {expected:?}, error {error:?}, got {actual:?}")
            }
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
                apply_permission_default(value);
            }
        }
    }
    Ok(rows)
}
