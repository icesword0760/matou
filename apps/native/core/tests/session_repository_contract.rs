use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, bail};
use matou_native_core::{
    hierarchy::{BootstrapWindowInput, HierarchyIds, bootstrap_window},
    sessions::{
        CreateShellSessionInput, StartShellRunInput, archive_session, create_shell_session,
        finish_run, get_run, get_session, interrupt_run, list_all_runs, list_runs, list_sessions,
        rename_session, start_shell_run, update_cwd,
    },
    storage::{CommandMetadata, DomainCommit, configure_connection, migrate},
};
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const REFERENCE: &str = include_str!("fixtures/session-repository-reference.json");
const TABLES: [&str; 4] = [
    "sessions",
    "session_runs",
    "domain_events",
    "command_deduplication",
];

#[derive(Deserialize)]
struct Reference {
    bootstrap: Bootstrap,
    tables: Vec<String>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    input: BootstrapWindowInput,
    command: CommandMetadata,
    ids: HierarchyIds,
    committed_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    op: String,
    input: Value,
    command: CommandMetadata,
    committed_at: i64,
    result: Option<Value>,
    error: Option<String>,
    snapshot: BTreeMap<String, Vec<Value>>,
}

#[test]
fn shell_lifecycle_matches_frozen_source_across_reopen() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    assert_eq!(reference.tables, TABLES);
    assert_eq!(reference.steps.len(), 22);
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("matou.sqlite");
    let mut db = open(&path)?;
    bootstrap_window(
        &mut db,
        &reference.bootstrap.command,
        &reference.bootstrap.input,
        &reference.bootstrap.ids,
        "默认",
        reference.bootstrap.committed_at,
    )?;

    for (index, step) in reference.steps.into_iter().enumerate() {
        let label = format!("{} {} ({})", index + 1, step.op, step.command.command_id);
        if step.op == "reopen" {
            db.close().map_err(|(_, error)| error)?;
            db = open(&path)?;
            assert!(get_run(&db, "missing")?.is_none());
            assert_eq!(
                get_run(&db, "run-2")?,
                list_runs(&db, "shell-2")?.get(1).cloned()
            );
            let reopened = json!({
                "sessions": list_sessions(&db)?,
                "runs": list_all_runs(&db)?,
                "sessionRuns": list_runs(&db, "shell-2")?,
                "missing": get_session(&db, "missing")?,
            });
            assert_eq!(Some(reopened), step.result, "{label}");
        } else {
            let observed = apply(&mut db, &step);
            match (observed, step.result, step.error) {
                (Ok(commit), Some(expected), None) => {
                    assert_eq!(serde_json::to_value(commit)?, expected, "{label}");
                }
                (Err(error), None, Some(expected)) => {
                    assert_eq!(error.to_string(), expected, "{label}");
                }
                (actual, result, error) => {
                    bail!(
                        "{label}: expected result {result:?}, error {error:?}, observed {actual:?}"
                    );
                }
            }
        }
        for table in TABLES {
            compare_rows(
                &db,
                &format!("SELECT * FROM {table} ORDER BY rowid"),
                step.snapshot
                    .get(table)
                    .with_context(|| format!("{label}: fixture {table}"))?,
                &label,
            )?;
        }
    }
    Ok(())
}

fn apply(db: &mut Connection, step: &Step) -> Result<DomainCommit> {
    let Step {
        op,
        input,
        command,
        committed_at,
        ..
    } = step;
    let string = |key: &str| {
        input[key]
            .as_str()
            .with_context(|| format!("missing {key}"))
    };
    let integer = |key: &str| {
        input[key]
            .as_i64()
            .with_context(|| format!("missing {key}"))
    };
    let now = integer("now")?;
    match op.as_str() {
        "create" => create_shell_session(
            db,
            command,
            &serde_json::from_value::<CreateShellSessionInput>(input.clone())?,
            *committed_at,
        ),
        "start" => start_shell_run(
            db,
            command,
            &serde_json::from_value::<StartShellRunInput>(input.clone())?,
            *committed_at,
        ),
        "finish" => finish_run(
            db,
            command,
            string("runId")?,
            integer("exitCode")?,
            input["signal"].as_i64(),
            now,
            *committed_at,
        ),
        "interrupt" => interrupt_run(db, command, string("runId")?, now, *committed_at),
        "rename" => rename_session(
            db,
            command,
            string("sessionId")?,
            string("title")?,
            now,
            *committed_at,
        ),
        "cwd" => update_cwd(
            db,
            command,
            string("sessionId")?,
            string("cwd")?,
            now,
            *committed_at,
        ),
        "archive" => archive_session(db, command, string("sessionId")?, now, *committed_at),
        _ => bail!("unexpected fixture operation {op}"),
    }
}

fn open(path: &Path) -> Result<Connection> {
    let mut db = Connection::open(path)?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    Ok(db)
}

fn read_rows(db: &Connection, sql: &str) -> Result<Vec<Value>> {
    let mut statement = db.prepare(sql)?;
    let columns: Vec<_> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut rows = statement.query([])?;
    let mut mapped = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = Map::new();
        for (index, column) in columns.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => json!(value),
                ValueRef::Real(value) => json!(value),
                ValueRef::Text(_) => json!(row.get::<_, String>(index)?),
                ValueRef::Blob(_) => bail!("unexpected BLOB column {column} in {sql}"),
            };
            object.insert(column.clone(), value);
        }
        mapped.push(Value::Object(object));
    }
    Ok(mapped)
}

fn normalize_json_columns(rows: &mut [Value]) -> Result<()> {
    for row in rows {
        for (column, value) in row.as_object_mut().context("expected an SQLite row")? {
            if column.ends_with("_json")
                && let Some(text) = value.as_str()
            {
                let parsed: Value = serde_json::from_str(text)
                    .with_context(|| format!("invalid JSON in {column}"))?;
                // The wrapper preserves SQL NULL versus JSON text containing null.
                *value = json!({ "json": parsed });
            }
        }
    }
    Ok(())
}

fn compare_rows(db: &Connection, sql: &str, expected: &[Value], step: &str) -> Result<()> {
    let mut actual = read_rows(db, sql)?;
    let mut expected = expected.to_vec();
    normalize_json_columns(&mut actual)?;
    normalize_json_columns(&mut expected)?;
    assert_eq!(actual, expected, "{step}: {sql}");
    Ok(())
}
