#![cfg(unix)]
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    storage::configure_connection,
    workspace_paths::{self, WorkspacePathState},
};
use rusqlite::{Connection, types::ValueRef};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::PathBuf};
#[test]
fn paths_match_frozen_service_through_loss_and_reopen() -> Result<()> {
    let gate = std::env::var_os("MATOU_WORKSPACE_PATH_GATE").map(PathBuf::from);
    let fixture: Value = match &gate {
        Some(gate) => serde_json::from_slice(&fs::read(gate.join("frozen-path.json"))?)?,
        None => serde_json::from_str(include_str!("fixtures/workspace-path-reference.json"))?,
    };
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("matou.sqlite");
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace)?;
    fs::write(
        &path,
        STANDARD.decode(fixture["seedBase64"].as_str().unwrap())?,
    )?;
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    let workspace_id = fixture["workspaceId"].as_str().unwrap();
    db.execute(
        "UPDATE workspaces SET root_directory=? WHERE id=?",
        [workspace.to_str().unwrap(), workspace_id],
    )?;
    let mut captures = Vec::new();
    for step in fixture["steps"].as_array().unwrap() {
        let label = step["action"].as_str().unwrap();
        match label {
            "missing" => fs::remove_dir(&workspace)?,
            "not-directory" => fs::write(&workspace, "file")?,
            "no-access" => {
                fs::remove_file(&workspace)?;
                fs::create_dir(&workspace)?;
                fs::set_permissions(&workspace, fs::Permissions::from_mode(0))?;
            }
            "restored" => fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))?,
            "reopen" => {
                drop(db);
                db = Connection::open(&path)?;
                configure_connection(&db)?;
            }
            _ => {}
        }
        let checked_at = step["state"]["checkedAt"].as_i64().context("check clock")?;
        let reason = workspace_paths::inspect(&workspace);
        let generation =
            workspace_paths::get(&db, workspace_id)?.map_or(0, |s| s.validation_generation) + 1;
        let state = WorkspacePathState {
            workspace_id: workspace_id.into(),
            status: if reason.is_empty() {
                "valid"
            } else {
                "invalid"
            }
            .into(),
            reason: reason.into(),
            checked_at,
            validation_generation: generation,
        };
        let commit = workspace_paths::commit(
            &mut db,
            step["commandId"].as_str().unwrap().into(),
            &state,
            step["committedAt"].as_i64().unwrap(),
        )?;
        let mut snapshot = BTreeMap::new();
        for (table, expected) in step["snapshot"].as_object().unwrap() {
            let rows = read_rows(&db, table)?;
            if gate.is_none() {
                assert_eq!(
                    normalize_json_columns(rows.clone())?,
                    normalize_json_columns(serde_json::from_value(expected.clone())?)?,
                    "{label}: {table}"
                );
            }
            snapshot.insert(table, rows);
        }
        if gate.is_none() {
            assert_eq!(commit.result, step["state"], "{label}: return value");
        }
        println!(
            "{label}: {} {:?}, generation {generation}",
            state.status, reason
        );
        captures.push(json!({
            "action": label, "state": commit.result, "commandId": step["commandId"],
            "committedAt": step["committedAt"], "snapshot": snapshot
        }));
    }
    if let Some(gate) = gate {
        fs::write(
            gate.join("native-path.json"),
            serde_json::to_string_pretty(&json!({
                "workspaceId": workspace_id, "sqliteVersion": rusqlite::version(), "steps": captures
            }))? + "\n",
        )?;
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
            }
        }
    }
    Ok(rows)
}
