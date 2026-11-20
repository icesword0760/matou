use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    hierarchy::canvas::{CanvasError, CanvasIds, CanvasMessages, create_fork_child},
    session_graph::{project_scene_graph, set_work_status},
    sessions::{ClaudeCatalogLoad, load_claude_catalog_binding},
    storage::{CommandMetadata, CommandReplayConflictError, configure_connection},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[path = "common/frozen_graph.rs"]
mod frozen_graph;
use frozen_graph::apply_permission_default;

const REFERENCE: &str = include_str!("fixtures/session-graph-reference.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    seed_base64: String,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", rename_all_fields = "camelCase")]
enum Step {
    Project {
        label: String,
        sql: Vec<String>,
        scene_id: String,
        window_id: Option<String>,
        result: Option<Value>,
        error: Option<String>,
    },
    Status {
        command: CommandMetadata,
        input: WorkStatusInput,
        committed_at: i64,
        result: Option<Value>,
        error: Option<String>,
        snapshot: BTreeMap<String, Vec<Value>>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkStatusInput {
    session_id: String,
    work_status: String,
    now: i64,
}

#[test]
fn projection_and_status_transactions_match_the_frozen_database() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let mut db = Connection::open(path)?;
    configure_connection(&db)?;
    let mut committed = HashSet::new();
    for (index, step) in reference.steps.into_iter().enumerate() {
        match step {
            Step::Project {
                label,
                sql,
                scene_id,
                window_id,
                result,
                error,
            } => {
                for statement in sql {
                    db.execute_batch(&statement)
                        .with_context(|| format!("step {index}: {label}"))?;
                }
                compare_result(
                    project_scene_graph(&db, &scene_id, window_id.as_deref()),
                    result,
                    error,
                    &format!("step {index}: {label}"),
                )?;
            }
            Step::Status {
                command,
                input,
                committed_at,
                result,
                error,
                snapshot,
            } => {
                let label = format!("step {index}: {}", command.command_id);
                let observed = set_work_status(
                    &mut db,
                    &command,
                    &input.session_id,
                    &input.work_status,
                    input.now,
                    committed_at,
                );
                if let Ok(commit) = &observed {
                    let receipt = snapshot["command_deduplication"]
                        .iter()
                        .find(|row| row["command_id"].as_str() == Some(command.command_id.as_str()))
                        .with_context(|| format!("{label}: no source receipt"))?;
                    assert_eq!(
                        commit.first_event_sequence,
                        receipt["first_event_seq"].as_i64(),
                        "{label}"
                    );
                    assert_eq!(
                        commit.last_event_sequence,
                        receipt["last_event_seq"].as_i64(),
                        "{label}"
                    );
                    assert_eq!(
                        commit.replayed,
                        !committed.insert(command.command_id),
                        "{label}"
                    );
                }
                compare_result(observed.map(|commit| commit.result), result, error, &label)?;
                for table in ["sessions", "domain_events", "command_deduplication"] {
                    let actual = read_rows(&db, table)?;
                    assert_eq!(
                        normalize_json_columns(actual)?,
                        normalize_json_columns(snapshot[table].clone())?,
                        "{label} table {table}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires a fresh K-storage-session frozen export"]
fn export_native_session_graph() -> Result<()> {
    let gate = PathBuf::from(env::var("MATOU_SESSION_GATE")?);
    let reference: Value = serde_json::from_slice(&fs::read(gate.join("graph.frozen.json"))?)?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(text(&reference, "seedBase64")?)?)?;
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    let sqlite: String = db.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    eprintln!("Native SQLite {sqlite}");
    let steps = reference["steps"].as_array().context("graph steps")?;
    let scene_id = text(&steps[0], "sceneId")?;
    let ids: CanvasIds = serde_json::from_value(
        steps
            .iter()
            .find(|step| step["label"] == "fork-accepted")
            .context("fork acceptance input")?["ids"]
            .clone(),
    )?;
    let messages = CanvasMessages {
        new_canvas: "新画布",
        duplicate_scene_name: "当前事项下已存在同名页签",
        workspace_path_invalid: "工作区路径不可用",
        detached_session_must_return: "请先把会话返回当前画布",
    };
    let mut observations = Vec::new();
    for step in steps {
        let op = text(step, "op")?;
        if let Some(sql) = step["sql"].as_array() {
            for statement in sql {
                db.execute_batch(statement.as_str().context("graph setup statement")?)
                    .with_context(|| format!("setup for {}", step["label"]))?;
            }
        }
        let observed = match op {
            "project" => {
                project_scene_graph(&db, text(step, "sceneId")?, step["windowId"].as_str())
            }
            "reopen" => {
                db.close().map_err(|(_, error)| error)?;
                db = Connection::open(&path)?;
                configure_connection(&db)?;
                project_scene_graph(&db, scene_id, Some("window"))
            }
            _ => {
                let command: CommandMetadata = serde_json::from_value(step["command"].clone())?;
                let input = &step["input"];
                let committed_at = step["committedAt"].as_i64().context("commit time")?;
                match op {
                    "status" | "work-status" => {
                        let input: WorkStatusInput = serde_json::from_value(input.clone())?;
                        set_work_status(
                            &mut db,
                            &command,
                            &input.session_id,
                            &input.work_status,
                            input.now,
                            committed_at,
                        )
                        .map(|commit| commit.result)
                    }
                    "fork" => create_fork_child(
                        &mut db,
                        &command,
                        &serde_json::from_value(input.clone())?,
                        &ids,
                        &messages,
                        committed_at,
                    ),
                    "catalog" => load_claude_catalog_binding(
                        &mut db,
                        &command,
                        &ClaudeCatalogLoad {
                            binding_id: text(input, "bindingId")?,
                            session_id: text(input, "sessionId")?,
                            scene_id: text(input, "sceneId")?,
                            cwd: text(input, "cwd")?,
                            provider_session_id: text(input, "providerSessionId")?,
                            title: text(input, "title")?,
                            model: input["model"].as_str(),
                            permission_mode: text(input, "permissionMode")?,
                        },
                        input["now"].as_i64().context("catalog input time")?,
                    )
                    .map(|commit| commit.result),
                    _ => bail!("unknown graph operation {op}"),
                }
            }
        };
        let (result, error, error_code, error_detail) = match observed {
            Ok(result) => (Some(result), None, None, None),
            Err(error) => {
                let code = error
                    .downcast_ref::<CanvasError>()
                    .map(|error| error.code)
                    .or_else(|| {
                        error
                            .downcast_ref::<CommandReplayConflictError>()
                            .map(|_| CommandReplayConflictError::CODE)
                    });
                let sqlite_code = error
                    .downcast_ref::<rusqlite::Error>()
                    .and_then(|error| error.sqlite_error())
                    .map(|error| error.extended_code);
                (None, Some(error.to_string()), code, sqlite_code)
            }
        };
        let projection = if step.get("projection").is_some() {
            Some(project_scene_graph(&db, scene_id, Some("window"))?)
        } else {
            None
        };
        let snapshot = step["snapshot"]
            .as_object()
            .map(|tables| {
                tables
                    .keys()
                    .map(|table| Ok((table, read_rows(&db, table)?)))
                    .collect::<Result<BTreeMap<_, _>>>()
            })
            .transpose()?;
        let label = step["label"]
            .as_str()
            .or(step["command"]["commandId"].as_str());
        observations.push(json!({ "op": op, "label": label, "result": result,
            "error": error, "errorCode": error_code, "errorDetail": error_detail,
            "projection": projection, "snapshot": snapshot }));
    }
    fs::write(
        gate.join("graph.native.json"),
        serde_json::to_vec_pretty(&observations)?,
    )?;
    Ok(())
}

fn text<'a>(object: &'a Value, key: &str) -> Result<&'a str> {
    object[key]
        .as_str()
        .with_context(|| format!("missing {key}"))
}

#[test]
fn permission_and_model_come_from_the_latest_valid_binding() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let Step::Project { scene_id, .. } = &reference.steps[0] else {
        bail!("expected initial graph fixture");
    };
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let db = Connection::open(path)?;
    let initial = project_scene_graph(&db, scene_id, None)?;
    let node = &initial["nodes"][0];
    let session_id = node["sessionId"].as_str().context("initial session")?;
    assert_eq!(node["permissionMode"], "default");
    assert!(node.get("model").is_none());
    db.execute(
        "UPDATE sessions SET kind = 'claude-code' WHERE id = ?",
        [session_id],
    )?;

    for (id, state, when, metadata) in [
        (
            "earlier",
            "available",
            1,
            json!({"permissionMode":"auto", "model":"old-model"}),
        ),
        (
            "latest",
            "available",
            2,
            json!({"permissionMode":"plan", "model":"bound-model"}),
        ),
        (
            "expired",
            "expired",
            3,
            json!({"permissionMode":"bypassPermissions", "model":"expired-model"}),
        ),
    ] {
        db.execute(
            "INSERT INTO provider_bindings
                (id, session_id, provider, provider_session_id, resume_state, metadata_json,
                 created_at, updated_at, validated_at)
             VALUES (?1, ?2, 'claude-code', ?1, ?3, ?4, ?5, ?5, ?5)",
            rusqlite::params![id, session_id, state, metadata.to_string(), when],
        )?;
    }
    let graph = project_scene_graph(&db, scene_id, None)?;
    assert_eq!(graph["nodes"][0]["permissionMode"], "plan");
    assert_eq!(graph["nodes"][0]["model"], "bound-model");
    db.execute(
        "UPDATE provider_bindings SET provider = 'antigravity', metadata_json = ?
         WHERE id = 'latest'",
        [json!({"permissionMode": "ask"}).to_string()],
    )?;
    let graph = project_scene_graph(&db, scene_id, None)?;
    assert_eq!(graph["nodes"][0]["permissionMode"], "ask");
    assert!(graph["nodes"][0].get("model").is_none());
    db.execute("DELETE FROM provider_bindings WHERE id <> 'expired'", [])?;
    for (state, invalidated_at, restore_state, can_fork) in [
        ("expired", Some(5), "none", true),
        ("failed", Some(5), "none", true),
        ("available", None, "none", true),
        ("available", None, "failed", false),
    ] {
        db.execute(
            "UPDATE provider_bindings SET resume_state=?, invalidated_at=?, restore_state=?, metadata_json=?
             WHERE id='expired'",
            rusqlite::params![state, invalidated_at, restore_state, r#"{"canFork":true}"#],
        )?;
        let graph = project_scene_graph(&db, scene_id, None)?;
        assert_eq!(graph["nodes"][0]["canFork"], can_fork);
    }
    Ok(())
}

fn compare_result(
    actual: Result<Value>,
    result: Option<Value>,
    error: Option<String>,
    label: &str,
) -> Result<()> {
    match (actual, result, error) {
        (Ok(actual), Some(mut expected), None) => {
            apply_permission_default(&mut expected);
            assert_eq!(actual, expected, "{label}");
        }
        (Err(actual), None, Some(expected)) => assert_eq!(actual.to_string(), expected, "{label}"),
        (actual, result, error) => {
            bail!("{label}: expected result {result:?}, error {error:?}, observed {actual:?}")
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
                let mut parsed: Value = serde_json::from_str(text)
                    .with_context(|| format!("invalid JSON text in {column}"))?;
                apply_permission_default(&mut parsed);
                *value = json!({ "json": parsed });
            }
        }
    }
    Ok(rows)
}
