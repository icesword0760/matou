use std::{collections::BTreeMap, env, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    hierarchy::{
        HierarchyIds,
        canvas::{CanvasError, CanvasIds, CanvasMessages, create_canvas},
        tabs::close_scene,
        tasks::{
            TaskError, create_task, delete_task, record_session_interaction, rename_task,
            reorder_pinned_task, reorder_task, select_task, set_task_pinned,
        },
        workspaces::{WorkspaceError, create_workspace},
    },
    storage::{CommandMetadata, CommandReplayConflictError, configure_connection},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const REFERENCE: &str = include_str!("fixtures/task-lifecycle-reference.json");

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
    new_task: String,
    default_task: String,
    workspace_path_invalid: String,
    new_canvas: Option<String>,
    duplicate_scene_name: Option<String>,
    detached_session_must_return: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Operation {
    CreateWorkspace,
    CreateTask,
    CreateCanvas,
    CloseScene,
    RenameTask,
    ReorderTask,
    ReorderPinnedTask,
    SetTaskPinned,
    RecordSessionInteraction,
    Activate,
    DeleteTask,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    op: Operation,
    label: String,
    input: Value,
    sql: Vec<String>,
    command: CommandMetadata,
    ids: Option<HierarchyIds>,
    canvas_ids: Option<CanvasIds>,
    result: Option<Value>,
    error: Option<String>,
    error_code: Option<String>,
    committed_at: i64,
    snapshot: Snapshot,
}

type Snapshot = BTreeMap<String, Vec<Value>>;

#[test]
fn task_lifecycle_matches_the_frozen_database() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    let mut reopened = false;
    for step in reference.steps {
        let label = &step.label;
        for sql in &step.sql {
            db.execute_batch(sql).with_context(|| label.clone())?;
        }
        let actual = execute_step(&mut db, &step, &reference.messages)?;
        match (actual, step.result, step.error) {
            (Ok(actual), Some(expected), None) => assert_eq!(actual, expected, "{label}: result"),
            (Err(actual), None, Some(expected)) => {
                assert_eq!(actual.to_string(), expected, "{label}: error message");
                assert_eq!(
                    error_code(&actual),
                    step.error_code.as_deref(),
                    "{label}: error code"
                );
            }
            (actual, expected, error) => {
                bail!("{label}: expected result {expected:?}, error {error:?}, got {actual:?}")
            }
        }
        if label == "delete-active" {
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
        "reopen SQLite before replaying the first deletion"
    );
    Ok(())
}

#[test]
#[ignore = "run through the K-task-delete evidence runner"]
fn capture_task_lifecycle_gate() -> Result<()> {
    let directory = PathBuf::from(env::var_os("MATOU_TASK_DELETE_GATE").context("gate directory")?);
    let reference: Reference =
        serde_json::from_slice(&fs::read(directory.join("reference.json"))?)?;
    let path = directory.join("native.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    let mut observations = Vec::new();
    for step in reference.steps {
        let label = &step.label;
        for sql in &step.sql {
            db.execute_batch(sql).with_context(|| label.clone())?;
        }
        let snapshot = |db: &Connection| -> Result<Snapshot> {
            step.snapshot
                .keys()
                .map(|table| Ok((table.clone(), read_rows(db, table)?)))
                .collect()
        };
        let before = snapshot(&db)?;
        let actual = execute_step(&mut db, &step, &reference.messages)
            .with_context(|| format!("{label}: invalid case input"))?;
        let (result, error, code, sqlite_extended_code) = match actual {
            Ok(result) => (Some(result), None, None, None),
            Err(error) => {
                let sqlite_extended_code = match error.downcast_ref::<rusqlite::Error>() {
                    Some(rusqlite::Error::SqliteFailure(error, _)) => Some(error.extended_code),
                    _ => None,
                };
                (
                    None,
                    Some(error.to_string()),
                    error_code(&error),
                    sqlite_extended_code,
                )
            }
        };
        if label == "delete-active" || label == "delete-running-retry" {
            db.close().map_err(|(_, error)| error)?;
            db = Connection::open(&path)?;
            configure_connection(&db)?;
        }
        observations.push(json!({
            "label": label,
            "result": result,
            "error": error,
            "errorCode": code,
            "sqliteExtendedCode": sqlite_extended_code,
            "before": before,
            "snapshot": snapshot(&db)?,
        }));
    }
    fs::write(
        directory.join("native-full.json"),
        serde_json::to_vec(&json!({
            "sqliteVersion": rusqlite::version(),
            "cases": observations,
        }))?,
    )?;
    Ok(())
}

fn execute_step(db: &mut Connection, step: &Step, messages: &Messages) -> Result<Result<Value>> {
    let unused_ids = HierarchyIds {
        workspace_id: "unused-workspace".into(),
        execution_context_id: "unused-context".into(),
        task_id: "unused-task".into(),
        scene_id: "unused-scene".into(),
        root_node_id: "unused-root".into(),
        session_id: "unused-session".into(),
        mount_id: "unused-mount".into(),
    };
    let ids = step.ids.as_ref().unwrap_or(&unused_ids);
    let command = &step.command;
    let now = step.committed_at;
    let input = &step.input;
    Ok(match step.op {
        Operation::CreateWorkspace => create_workspace(
            db,
            command,
            &Deserialize::deserialize(input)?,
            ids,
            &messages.default_task,
            now,
        ),
        Operation::CreateTask => create_task(
            db,
            command,
            &Deserialize::deserialize(input)?,
            ids,
            &messages.new_task,
            &messages.workspace_path_invalid,
            now,
        ),
        Operation::CreateCanvas => create_canvas(
            db,
            command,
            &Deserialize::deserialize(input)?,
            step.canvas_ids.as_ref().context("canvas IDs")?,
            &CanvasMessages {
                new_canvas: messages.new_canvas.as_deref().context("new canvas title")?,
                duplicate_scene_name: messages
                    .duplicate_scene_name
                    .as_deref()
                    .context("duplicate scene name")?,
                workspace_path_invalid: &messages.workspace_path_invalid,
                detached_session_must_return: messages
                    .detached_session_must_return
                    .as_deref()
                    .context("detached session message")?,
            },
            now,
        ),
        Operation::CloseScene => close_scene(db, command, &Deserialize::deserialize(input)?, now),
        Operation::RenameTask => rename_task(db, command, &Deserialize::deserialize(input)?, now),
        Operation::ReorderTask => reorder_task(db, command, &Deserialize::deserialize(input)?, now),
        Operation::ReorderPinnedTask => {
            reorder_pinned_task(db, command, &Deserialize::deserialize(input)?, now)
        }
        Operation::SetTaskPinned => {
            set_task_pinned(db, command, &Deserialize::deserialize(input)?, now)
        }
        Operation::RecordSessionInteraction => {
            record_session_interaction(db, command, &Deserialize::deserialize(input)?, now)
        }
        Operation::Activate => select_task(db, &Deserialize::deserialize(input)?)
            .and_then(|hierarchy| Ok(serde_json::to_value(hierarchy)?)),
        Operation::DeleteTask => delete_task(
            db,
            command,
            &Deserialize::deserialize(input)?,
            ids,
            &messages.default_task,
            now,
        ),
    })
}

fn error_code(error: &anyhow::Error) -> Option<&'static str> {
    error
        .downcast_ref::<TaskError>()
        .map(|error| error.code)
        .or_else(|| error.downcast_ref::<CanvasError>().map(|error| error.code))
        .or_else(|| {
            error
                .downcast_ref::<WorkspaceError>()
                .map(|error| error.code)
        })
        .or_else(|| {
            error
                .downcast_ref::<CommandReplayConflictError>()
                .map(|_| CommandReplayConflictError::CODE)
        })
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
