use std::fs;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::{
    geometry::{GeometryUpdate, StoredGeometry, discard_invalid, get, list, list_all, put},
    storage::configure_connection,
};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

const REFERENCE: &str = include_str!("fixtures/geometry-reference.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    seed_base64: String,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    #[serde(flatten)]
    operation: Operation,
    label: String,
    sql: Vec<String>,
    #[serde(default)]
    result: Value,
    error: Option<String>,
    rows: Vec<GeometryRow>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Operation {
    Put { update: GeometryUpdate },
    Get { scene_id: String, owner_key: String },
    List { scene_id: String },
    ListAll,
    Discard { scene_id: String },
    Reopen,
}

#[derive(Debug, PartialEq)]
enum Observed {
    Entry(Option<StoredGeometry>),
    List(Vec<StoredGeometry>),
    Count(usize),
}

#[derive(Debug, Deserialize, PartialEq)]
struct GeometryRow {
    scene_id: String,
    owner_key: String,
    layout_revision: i64,
    geometry_json: String,
    updated_at: i64,
}

#[test]
fn geometry_operations_and_persistence_match_the_frozen_source() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    fs::write(&path, STANDARD.decode(reference.seed_base64)?)?;
    let mut db = Connection::open(&path)?;
    configure_connection(&db)?;
    for step in reference.steps {
        let label = step.label;
        for sql in step.sql {
            db.execute_batch(&sql).with_context(|| label.clone())?;
        }
        let result = match &step.operation {
            Operation::Put { update } => put(&db, update).map(|value| Observed::Entry(Some(value))),
            Operation::Get {
                scene_id,
                owner_key,
            } => get(&db, scene_id, owner_key).map(Observed::Entry),
            Operation::List { scene_id } => list(&db, scene_id).map(Observed::List),
            Operation::ListAll => list_all(&db).map(Observed::List),
            Operation::Discard { scene_id } => discard_invalid(&db, scene_id).map(Observed::Count),
            Operation::Reopen => {
                db.close().map_err(|(_, error)| error)?;
                db = Connection::open(&path)?;
                configure_connection(&db)?;
                list_all(&db).map(Observed::List)
            }
        };
        match (result, step.error) {
            (Ok(actual), None) => {
                let expected = match step.operation {
                    Operation::Put { .. } | Operation::Get { .. } => {
                        Observed::Entry(serde_json::from_value(step.result)?)
                    }
                    Operation::List { .. } | Operation::ListAll | Operation::Reopen => {
                        Observed::List(serde_json::from_value(step.result)?)
                    }
                    Operation::Discard { .. } => {
                        Observed::Count(serde_json::from_value(step.result)?)
                    }
                };
                assert_eq!(actual, expected, "{label}: result");
            }
            (Err(actual), Some(expected)) => {
                assert_eq!(actual.to_string(), expected, "{label}: error")
            }
            (actual, expected_error) => {
                bail!("{label}: got {actual:?}, expected error {expected_error:?}")
            }
        }
        assert_eq!(rows(&db)?, step.rows, "{label}: scene_geometry");
    }
    Ok(())
}

fn rows(connection: &Connection) -> Result<Vec<GeometryRow>> {
    let mut query =
        connection.prepare("SELECT * FROM scene_geometry ORDER BY scene_id, owner_key")?;
    let rows = query.query_map([], |row| {
        Ok(GeometryRow {
            scene_id: row.get("scene_id")?,
            owner_key: row.get("owner_key")?,
            layout_revision: row.get("layout_revision")?,
            geometry_json: row.get("geometry_json")?,
            updated_at: row.get("updated_at")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
