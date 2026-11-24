use std::{collections::BTreeMap, env, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    hierarchy::focus_session,
    session_graph::{RecordInteractionInput, record_interaction},
    storage::{CommandMetadata, CommandReplayConflictError, configure_connection},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[path = "common/frozen_graph.rs"]
mod frozen_graph;
use frozen_graph::apply_permission_default;

const REFERENCE: &str = include_str!("fixtures/session-interaction-reference.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    seed_base64: String,
    invalid_kind_message: String,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", rename_all_fields = "camelCase")]
enum Operation {
    Record {
        command: CommandMetadata,
        input: RecordInteractionInput,
        committed_at: i64,
    },
    Focus {
        input: FocusInput,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FocusInput {
    window_id: String,
    scene_id: String,
    session_id: String,
    now: i64,
}

#[derive(Deserialize)]
struct Step {
    label: String,
    sql: Vec<String>,
    result: Option<Value>,
    error: Option<String>,
    snapshot: BTreeMap<String, Vec<Value>>,
    #[serde(flatten)]
    operation: Operation,
}

#[test]
fn interaction_transactions_and_focus_follow_the_frozen_database() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    replay_interactions(reference, false)?;
    Ok(())
}

#[test]
#[ignore = "requires a fresh K-storage-session frozen export"]
fn export_native_session_interaction() -> Result<()> {
    let gate = PathBuf::from(env::var("MATOU_SESSION_GATE")?);
    let reference = serde_json::from_slice(&fs::read(gate.join("interaction.frozen.json"))?)?;
    let observations = replay_interactions(reference, true)?;
    fs::write(
        gate.join("interaction.native.json"),
        serde_json::to_vec_pretty(&observations)?,
    )?;
    Ok(())
}

fn replay_interactions(reference: Reference, capture: bool) -> Result<Vec<Value>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let mut db = Connection::open(path)?;
    configure_connection(&db)?;

    let mut observations = Vec::new();
    for step in reference.steps {
        let label = &step.label;
        let op = match &step.operation {
            Operation::Record { .. } => "record",
            Operation::Focus { .. } => "focus",
        };
        for sql in step.sql {
            db.execute_batch(&sql).with_context(|| label.clone())?;
        }
        let result = match step.operation {
            Operation::Record {
                command,
                input,
                committed_at,
            } => record_interaction(
                &mut db,
                &command,
                &input,
                &reference.invalid_kind_message,
                committed_at,
            ),
            Operation::Focus { input } => focus_session(
                &mut db,
                &input.window_id,
                &input.scene_id,
                &input.session_id,
                input.now,
                "Session does not belong to the Scene",
            ),
        };
        if capture {
            let (result, error, error_code) = match result {
                Ok(value) => (Some(value), None, None),
                Err(error) => {
                    let code = error
                        .downcast_ref::<CommandReplayConflictError>()
                        .map(|_| CommandReplayConflictError::CODE);
                    (None, Some(error.to_string()), code)
                }
            };
            let snapshot = step
                .snapshot
                .keys()
                .map(|table| {
                    Ok((
                        table,
                        read_rows(&db, table).with_context(|| format!("{label}: {table}"))?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            observations.push(
                json!({ "op": op, "label": label, "result": result, "error": error,
                "errorCode": error_code, "snapshot": snapshot }),
            );
            continue;
        }
        match (result, step.result, step.error) {
            (Ok(actual), Some(mut expected), None) => {
                apply_permission_default(&mut expected);
                assert_eq!(actual, expected, "{label}: result");
            }
            (Err(actual), None, Some(expected)) => {
                assert_eq!(actual.to_string(), expected, "{label}: error");
            }
            (actual, expected, error) => {
                bail!("{label}: expected result {expected:?}, error {error:?}, got {actual:?}");
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
    Ok(observations)
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
