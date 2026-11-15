use std::{collections::BTreeMap, fs};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    hierarchy::{focus_session, list_scenes, read_created_path, select_scene},
    storage::configure_connection,
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    seed_base64: String,
    messages: Messages,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Messages {
    session_not_in_canvas: String,
}

#[derive(Deserialize)]
struct Step {
    #[serde(flatten)]
    operation: Operation,
    label: String,
    sql: Vec<String>,
    result: Option<Value>,
    error: Option<String>,
    snapshot: BTreeMap<String, Vec<Value>>,
}

#[derive(Deserialize)]
#[serde(
    tag = "op",
    content = "input",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
enum Operation {
    Select {
        window_id: String,
        scene_id: String,
        now: i64,
    },
    Focus {
        window_id: String,
        scene_id: String,
        session_id: String,
        now: i64,
    },
    Created {
        session_id: String,
    },
    List {
        task_id: String,
    },
}

#[test]
fn canvas_navigation_matches_frozen_reference() -> Result<()> {
    let reference: Reference =
        serde_json::from_str(include_str!("fixtures/canvas-navigation-reference.json"))?;
    let root = tempfile::tempdir()?;
    let path = root.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let mut connection = Connection::open(path)?;
    configure_connection(&connection)?;

    for step in reference.steps {
        let label = step.label;
        for sql in step.sql {
            connection
                .execute_batch(&sql)
                .with_context(|| label.clone())?;
        }
        let actual = match step.operation {
            Operation::Select {
                window_id,
                scene_id,
                now,
            } => select_scene(&mut connection, &window_id, &scene_id, now)
                .and_then(|result| Ok(serde_json::to_value(result)?)),
            Operation::Focus {
                window_id,
                scene_id,
                session_id,
                now,
            } => focus_session(
                &mut connection,
                &window_id,
                &scene_id,
                &session_id,
                now,
                &reference.messages.session_not_in_canvas,
            ),
            Operation::Created { session_id } => read_created_path(&connection, &session_id),
            Operation::List { task_id } => list_scenes(&connection, &task_id)
                .and_then(|result| Ok(serde_json::to_value(result)?)),
        };
        match (actual, step.result, step.error) {
            (Ok(actual), Some(expected), None) => assert_eq!(actual, expected, "{label}"),
            (Err(actual), None, Some(expected)) => {
                assert_eq!(actual.to_string(), expected, "{label}")
            }
            (actual, expected, error) => {
                bail!("{label}: expected result {expected:?}, error {error:?}, observed {actual:?}")
            }
        }
        for (table, expected) in step.snapshot {
            let actual = read_rows(&connection, &table)?;
            assert_eq!(
                normalize_json_columns(actual)?,
                normalize_json_columns(expected)?,
                "{label}: {table}"
            );
        }
    }
    Ok(())
}

fn read_rows(connection: &Connection, table: &str) -> Result<Vec<Value>> {
    let mut statement = connection.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut rows = statement.query([])?;
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
            .context("expected a SQLite row object")?
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
