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
    read_query_rows(db, &format!("SELECT * FROM {table} ORDER BY rowid"), [])
}

fn read_query_rows(
    db: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Value>> {
    let mut statement = db.prepare(sql)?;
    let columns: Vec<_> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut rows = statement.query(params)?;
    let mut output = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = Map::new();
        for (index, column) in columns.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => json!(value),
                ValueRef::Real(value) => json!(value),
                ValueRef::Text(_) => json!(row.get::<_, String>(index)?),
                ValueRef::Blob(_) => bail!("unexpected BLOB column {column} in query {sql}"),
            };
            object.insert(column.clone(), value);
        }
        output.push(Value::Object(object));
    }
    Ok(output)
}

mod tab_close_gate {
    use super::*;
    use matou_native_core::hierarchy::{
        canvas::{CanvasIds, CanvasMessages, create_canvas},
        list_scenes, read_hierarchy, select_scene,
    };
    use serde_json::to_value;
    use std::{env, path::Path};

    // The native host supplies these strings to the core. Keep the gate independent
    // of the frozen exporter when collecting the native names and error text.
    const MESSAGES: CanvasMessages<'_> = CanvasMessages {
        new_canvas: "新画布",
        duplicate_scene_name: "当前事项下已存在同名页签",
        workspace_path_invalid: "工作区目录不可用，请先在本地恢复原路径，或移出该工作区",
        detached_session_must_return: "独立窗口中的会话需要先回到原会话列表",
    };

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Plan {
        version: u32,
        locale: String,
        bootstrap: Window,
        task_ids: Vec<String>,
        queries: Queries,
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Queries {
        sequence: String,
        events: String,
        receipt: String,
        persisted_events: String,
        tasks: String,
        scenes: String,
        sessions: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Window {
        window_id: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum GateOperation {
        Create,
        Rename,
        Reorder,
        Close,
        Select,
        Read,
        Reopen,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        label: String,
        op: GateOperation,
        input: Option<Value>,
        ids: Option<String>,
        prepare: Option<String>,
        command_id: Option<String>,
        read_receipt: Option<String>,
    }

    fn open(path: &Path) -> Result<Connection> {
        let db = Connection::open(path)?;
        configure_connection(&db)?;
        Ok(db)
    }

    fn invoke(db: &mut Connection, step: &Case, window: &str) -> Result<Value> {
        let Some(input) = &step.input else {
            anyhow::ensure!(matches!(
                step.op,
                GateOperation::Read | GateOperation::Reopen
            ));
            return Ok(to_value(read_hierarchy(db, window)?)?);
        };
        let now = input["now"].as_i64().context("case must set a clock")?;
        let command_id = step.command_id.as_deref().unwrap_or(&step.label);
        let metadata = |operation| CommandMetadata {
            command_id: command_id.to_owned(),
            command_type: format!("hierarchy.{operation}"),
            request_hash: format!("fixture:{command_id}"),
            causation_id: None,
            correlation_id: None,
        };
        match step.op {
            GateOperation::Create => {
                let prefix = step.ids.as_ref().context("create requires fixture IDs")?;
                let ids = CanvasIds {
                    scene_id: format!("{prefix}-scene"),
                    root_node_id: format!("{prefix}-root"),
                    secondary_node_id: format!("{prefix}-secondary"),
                    session_id: format!("{prefix}-session"),
                    mount_id: format!("{prefix}-mount"),
                    relation_id: format!("{prefix}-relation"),
                };
                create_canvas(
                    db,
                    &metadata("create-canvas"),
                    &serde_json::from_value(input.clone())?,
                    &ids,
                    &MESSAGES,
                    now,
                )
            }
            GateOperation::Rename => rename_scene(
                db,
                &metadata("rename-scene"),
                &serde_json::from_value(input.clone())?,
                MESSAGES.duplicate_scene_name,
                now,
            ),
            GateOperation::Reorder => reorder_scene(
                db,
                &metadata("reorder-scene"),
                &serde_json::from_value(input.clone())?,
                now,
            ),
            GateOperation::Close => close_scene(
                db,
                &metadata("close-scene"),
                &serde_json::from_value(input.clone())?,
                now,
            ),
            GateOperation::Select => {
                let window = input["windowId"].as_str().context("missing windowId")?;
                let scene = input["sceneId"].as_str().context("missing sceneId")?;
                Ok(to_value(select_scene(db, window, scene, now)?)?)
            }
            GateOperation::Read | GateOperation::Reopen => bail!("read case supplied input"),
        }
    }

    #[test]
    #[ignore = "run from K-tab-close/run.sh against the isolated seed"]
    fn capture_native() -> Result<()> {
        let stage = env::var("MATOU_TAB_CLOSE_STAGE")?;
        let plan: Plan =
            serde_json::from_str(&fs::read_to_string(env::var("MATOU_TAB_CLOSE_INPUT")?)?)?;
        assert_eq!(plan.locale, "zh-CN");
        let path = Path::new(&stage).join("native.sqlite");
        fs::copy(Path::new(&stage).join("seed.sqlite"), &path)?;
        let mut db = open(&path)?;
        let q = &plan.queries;
        let window = &plan.bootstrap.window_id;
        let mut steps = Vec::new();
        for step in &plan.cases {
            if let Some(prepare) = &step.prepare {
                db.execute_batch(prepare)
                    .with_context(|| step.label.clone())?;
            }
            if matches!(step.op, GateOperation::Reopen) {
                db.close().map_err(|(_, error)| error)?;
                db = open(&path)?;
            }
            let before: i64 = db.query_row(&q.sequence, [], |row| row.get(0))?;
            let receipt_id = step
                .read_receipt
                .as_deref()
                .or(step.command_id.as_deref())
                .unwrap_or(&step.label);
            let mut observation = json!({
                "label": step.label,
                "events": [],
                "receipt": null,
                "persistedEvents": [],
            });
            match invoke(&mut db, step, window) {
                Ok(result) => observation["result"] = result,
                Err(error) => {
                    let code = error
                        .downcast_ref::<CanvasError>()
                        .map(|error| error.code)
                        .or_else(|| {
                            error
                                .downcast_ref::<CommandReplayConflictError>()
                                .map(|_| CommandReplayConflictError::CODE)
                        });
                    observation["error"] = json!({"text": error.to_string(), "code": code});
                }
            }
            observation["events"] = json!(read_query_rows(&db, &q.events, [before])?);
            if let Some(receipt) = read_query_rows(&db, &q.receipt, [receipt_id])?.pop() {
                observation["receipt"] = receipt;
            }
            if step.read_receipt.is_some() {
                observation["persistedEvents"] =
                    json!(read_query_rows(&db, &q.persisted_events, [receipt_id])?);
            }
            observation["hierarchy"] = to_value(read_hierarchy(&db, window)?)?;
            let mut tabs = Map::new();
            for task in &plan.task_ids {
                tabs.insert(task.clone(), to_value(list_scenes(&db, task)?)?);
            }
            observation["tabsByTask"] = Value::Object(tabs);
            observation["tasks"] = json!(read_query_rows(&db, &q.tasks, [])?);
            observation["scenes"] = json!(read_query_rows(&db, &q.scenes, [])?);
            observation["sessions"] = json!(read_query_rows(&db, &q.sessions, [])?);
            steps.push(observation);
        }
        db.close().map_err(|(_, error)| error)?;
        let report = json!({"version": plan.version, "steps": steps});
        fs::write(
            Path::new(&stage).join("native.json"),
            serde_json::to_string_pretty(&report)? + "\n",
        )?;
        println!("Captured {} native cases", steps.len());
        Ok(())
    }
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
