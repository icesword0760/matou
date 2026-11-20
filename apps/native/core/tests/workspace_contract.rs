use std::{collections::BTreeMap, fs};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    hierarchy::{
        HierarchyIds, bootstrap_window,
        tasks::{TaskError, create_task},
        workspaces::{
            WorkspaceError, create_workspace, relink_workspace, remove_workspace, rename_workspace,
            reorder_pinned_workspace, select_workspace, set_workspace_pinned,
        },
    },
    storage::{CommandMetadata, CommandReplayConflictError, configure_connection},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const REFERENCE: &str = include_str!("fixtures/workspace-lifecycle-reference.json");

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
    default_task: String,
    new_task: String,
    workspace_path_invalid: String,
    workspace_name_follows_directory: String,
    default_workspace_is_home_directory: String,
    workspace_directory_taken: String,
    default_workspace_kept: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Operation {
    CreateWorkspace,
    ActivateWorkspace,
    RenameWorkspace,
    RelinkWorkspace,
    SetWorkspacePinned,
    ReorderPinnedWorkspace,
    RemoveWorkspace,
    CreateTask,
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
    result: Option<Value>,
    error: Option<String>,
    error_code: Option<String>,
    committed_at: i64,
    snapshot: BTreeMap<String, Vec<Value>>,
}

#[test]
fn workspace_lifecycle_matches_the_frozen_database() -> Result<()> {
    let live_reference = std::env::var_os("MATOU_WORKSPACE_LIFECYCLE_REFERENCE")
        .map(fs::read_to_string)
        .transpose()?;
    let reference: Reference =
        serde_json::from_str(live_reference.as_deref().unwrap_or(REFERENCE))?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    let unused_ids = HierarchyIds {
        workspace_id: "unused-workspace".into(),
        execution_context_id: "unused-context".into(),
        task_id: "unused-task".into(),
        scene_id: "unused-scene".into(),
        root_node_id: "unused-root".into(),
        session_id: "unused-session".into(),
        mount_id: "unused-mount".into(),
    };
    let messages = &reference.messages;
    let mut reopened = false;
    for step in reference.steps {
        let label = &step.label;
        for sql in &step.sql {
            db.execute_batch(sql).with_context(|| label.clone())?;
        }
        let ids = step.ids.as_ref().unwrap_or(&unused_ids);
        let command = &step.command;
        let now = step.committed_at;
        let input = step.input;
        let actual = match step.op {
            Operation::CreateWorkspace => create_workspace(
                &mut db,
                command,
                &serde_json::from_value(input)?,
                ids,
                &messages.default_task,
                now,
            ),
            Operation::ActivateWorkspace => {
                select_workspace(&mut db, &serde_json::from_value(input)?)
                    .and_then(|hierarchy| Ok(serde_json::to_value(hierarchy)?))
            }
            Operation::RenameWorkspace => rename_workspace(
                &db,
                &serde_json::from_value(input)?,
                &messages.workspace_name_follows_directory,
            ),
            Operation::RelinkWorkspace => relink_workspace(
                &mut db,
                command,
                &serde_json::from_value(input)?,
                &messages.default_workspace_is_home_directory,
                &messages.workspace_directory_taken,
                now,
            ),
            Operation::SetWorkspacePinned => {
                set_workspace_pinned(&mut db, command, &serde_json::from_value(input)?, now)
            }
            Operation::ReorderPinnedWorkspace => {
                reorder_pinned_workspace(&mut db, command, &serde_json::from_value(input)?, now)
            }
            Operation::RemoveWorkspace => remove_workspace(
                &mut db,
                command,
                &serde_json::from_value(input)?,
                &messages.default_workspace_kept,
                now,
            ),
            Operation::CreateTask => create_task(
                &mut db,
                command,
                &serde_json::from_value(input)?,
                ids,
                &messages.new_task,
                &messages.workspace_path_invalid,
                now,
            ),
        };
        match (actual, step.result, step.error) {
            (Ok(actual), Some(expected), None) => assert_eq!(actual, expected, "{label}: result"),
            (Err(actual), None, Some(expected)) => {
                assert_eq!(actual.to_string(), expected, "{label}: error message");
                let code = actual
                    .downcast_ref::<WorkspaceError>()
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
        if label == "remove-a-other-window-active" {
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
    assert!(reopened, "reopen SQLite before replaying workspace removal");
    Ok(())
}

#[test]
#[ignore = "run the K-workspace-path gate to supply isolated input and capture paths"]
fn capture_workspace_path_gate_lifecycle() -> Result<()> {
    let gate = std::path::PathBuf::from(std::env::var("MATOU_WORKSPACE_PATH_GATE")?);
    let frozen: Value = serde_json::from_slice(&fs::read(gate.join("frozen-lifecycle.json"))?)?;
    let paths: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(gate.join("native-resolved-paths.json"))?)?;
    let root = std::path::Path::new(frozen["root"].as_str().context("fixture root")?);
    let files = || {
        ["Default", "Alpha", "Beta", "Recovered"]
            .into_iter()
            .map(|name| Ok((name, fs::read_to_string(root.join(name).join("keep.txt"))?)))
            .collect::<Result<BTreeMap<_, _>>>()
    };
    let mut captures = Vec::new();
    for fixture in frozen["cases"].as_array().context("lifecycle cases")? {
        let id = fixture["id"].as_str().context("case ID")?;
        let op = fixture["op"].as_str().context("operation")?;
        let scratch = tempfile::tempdir()?;
        let path = scratch.path().join("matou.sqlite");
        fs::copy(fixture["seed"].as_str().context("seed path")?, &path)?;
        let mut db = Connection::open(&path)?;
        configure_connection(&db)?;
        let tables = fixture["before"].as_object().context("SQL tables")?;
        let snapshot = |db: &Connection| {
            tables
                .keys()
                .map(|table| Ok((table, read_rows(db, table)?)))
                .collect::<Result<BTreeMap<_, _>>>()
        };
        let before = snapshot(&db)?;
        let files_before = files()?;
        let command: CommandMetadata = serde_json::from_value(fixture["command"].clone())?;
        let ids = if fixture["ids"].is_null() {
            HierarchyIds {
                workspace_id: format!("{id}-workspace"),
                execution_context_id: format!("{id}-context"),
                task_id: format!("{id}-task"),
                scene_id: format!("{id}-scene"),
                root_node_id: format!("{id}-root"),
                session_id: format!("{id}-session"),
                mount_id: format!("{id}-mount"),
            }
        } else {
            serde_json::from_value(fixture["ids"].clone())?
        };
        let now = fixture["committedAt"].as_i64().context("commit clock")?;
        let mut input = fixture["input"].clone();
        if matches!(op, "create" | "relink") {
            input["rootDirectory"] =
                json!(paths.get(id).with_context(|| format!("{id} host path"))?);
        }
        let result = match op {
            "bootstrap" => bootstrap_window(
                &mut db,
                &command,
                &serde_json::from_value(input.clone())?,
                &ids,
                "默认",
                now,
            )
            .and_then(|value| Ok(serde_json::to_value(value)?)),
            "create" => create_workspace(
                &mut db,
                &command,
                &serde_json::from_value(input.clone())?,
                &ids,
                "默认",
                now,
            ),
            "relink" => relink_workspace(
                &mut db,
                &command,
                &serde_json::from_value(input.clone())?,
                "默认工作空间始终指向 macOS 用户目录",
                "该目录已经属于另一个工作空间",
                now,
            ),
            "createTask" => create_task(
                &mut db,
                &command,
                &serde_json::from_value(input.clone())?,
                &ids,
                "新事项",
                "工作区目录不可用，请先在本地恢复原路径，或移出该工作区",
                now,
            ),
            "remove" => remove_workspace(
                &mut db,
                &command,
                &serde_json::from_value(input.clone())?,
                "默认工作空间会保留在侧栏中",
                now,
            ),
            _ => bail!("unexpected operation {op}"),
        };
        let (result, error) = match result {
            Ok(result) => (result, Value::Null),
            Err(error) => {
                let code = error
                    .downcast_ref::<WorkspaceError>()
                    .map(|error| error.code)
                    .or_else(|| error.downcast_ref::<TaskError>().map(|error| error.code));
                (
                    Value::Null,
                    json!({"message": error.to_string(), "code": code}),
                )
            }
        };
        println!("{id} {op} {}", error["message"].as_str().unwrap_or("OK"));
        captures.push(json!({
            "id": id, "op": op, "input": input, "command": command, "committedAt": now,
            "result": result, "error": error, "before": before, "after": snapshot(&db)?,
            "filesBefore": files_before, "filesAfter": files()?,
        }));
    }
    fs::write(
        gate.join("native-lifecycle.json"),
        serde_json::to_string_pretty(&json!({
            "root": root, "sqliteVersion": rusqlite::version(), "cases": captures
        }))? + "\n",
    )?;
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
