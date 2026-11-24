use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::recovery_marker::stringify;

const MAX_SAFE_REVISION: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeometryUpdate {
    pub scene_id: String,
    pub owner_key: String,
    pub layout_revision: f64,
    pub geometry: Value,
    pub now: i64,
}

pub type StoredGeometry = GeometryUpdate;

pub fn put(connection: &Connection, update: &GeometryUpdate) -> Result<StoredGeometry> {
    let revision = update.layout_revision;
    ensure!(
        (0.0..=MAX_SAFE_REVISION).contains(&revision) && revision.fract() == 0.0,
        "layout revision must be a non-negative safe integer"
    );
    ensure!(
        owner_exists(connection, &update.scene_id, &update.owner_key)?,
        "geometry owner {} does not exist in Scene {}",
        update.owner_key,
        update.scene_id
    );
    let current = get(connection, &update.scene_id, &update.owner_key)?;
    if let Some(current) = &current {
        ensure!(
            revision >= current.layout_revision,
            "stale layout revision {revision}; current revision is {}",
            current.layout_revision
        );
    }
    let geometry = stringify(&update.geometry)?;
    if let Some(current) = current
        && revision == current.layout_revision
        && stringify(&current.geometry)? == geometry
    {
        return Ok(current);
    }
    connection.execute(
        "INSERT INTO scene_geometry (
           scene_id, owner_key, layout_revision, geometry_json, updated_at
         ) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(scene_id, owner_key) DO UPDATE SET
           layout_revision = excluded.layout_revision,
           geometry_json = excluded.geometry_json,
           updated_at = excluded.updated_at",
        params![
            update.scene_id,
            update.owner_key,
            revision as i64,
            geometry,
            update.now
        ],
    )?;
    Ok(update.clone())
}

pub fn get(
    connection: &Connection,
    scene_id: &str,
    owner_key: &str,
) -> Result<Option<StoredGeometry>> {
    Ok(connection
        .query_row(
            "SELECT * FROM scene_geometry WHERE scene_id = ? AND owner_key = ?",
            params![scene_id, owner_key],
            map_geometry,
        )
        .optional()?
        .flatten())
}

pub fn list(connection: &Connection, scene_id: &str) -> Result<Vec<StoredGeometry>> {
    let mut statement =
        connection.prepare("SELECT * FROM scene_geometry WHERE scene_id = ? ORDER BY owner_key")?;
    let rows = statement.query_map([scene_id], map_geometry)?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect())
}

pub fn list_all(connection: &Connection) -> Result<Vec<StoredGeometry>> {
    let mut statement =
        connection.prepare("SELECT * FROM scene_geometry ORDER BY scene_id, owner_key")?;
    let rows = statement.query_map([], map_geometry)?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect())
}

pub fn discard_invalid(connection: &Connection, scene_id: &str) -> Result<usize> {
    let mut statement =
        connection.prepare("SELECT owner_key FROM scene_geometry WHERE scene_id = ?")?;
    let owners = statement
        .query_map([scene_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut removed = 0;
    for owner in owners {
        if !owner_exists(connection, scene_id, &owner)? {
            connection.execute(
                "DELETE FROM scene_geometry WHERE scene_id = ? AND owner_key = ?",
                params![scene_id, owner],
            )?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn map_geometry(row: &Row<'_>) -> rusqlite::Result<Option<StoredGeometry>> {
    let raw: String = row.get("geometry_json")?;
    let Ok(geometry) = serde_json::from_str(&raw) else {
        return Ok(None);
    };
    Ok(Some(StoredGeometry {
        scene_id: row.get("scene_id")?,
        owner_key: row.get("owner_key")?,
        layout_revision: row.get::<_, i64>("layout_revision")? as f64,
        geometry,
        now: row.get("updated_at")?,
    }))
}

fn scene_exists(connection: &Connection, scene_id: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT id FROM scenes WHERE id = ?)",
        [scene_id],
        |row| row.get(0),
    )
}

fn owner_exists(connection: &Connection, scene_id: &str, owner_key: &str) -> Result<bool> {
    if owner_key == "scene" {
        return Ok(scene_exists(connection, scene_id)?);
    }
    if let Some(owner) = parse_canvas_owner(owner_key) {
        if owner.scene_id != scene_id || !scene_exists(connection, scene_id)? {
            return Ok(false);
        }
        return match owner.session_id {
            None | Some("root") => Ok(true),
            Some(session) => Ok(connection.query_row(
                "SELECT EXISTS(SELECT session_id FROM session_canvas_memberships
                 WHERE scene_id = ? AND session_id = ?)",
                params![scene_id, session],
                |row| row.get(0),
            )?),
        };
    }
    let Some((kind, id)) = owner_key.split_once(':') else {
        return Ok(false);
    };
    let table = match kind {
        "node" => "scene_nodes",
        "mount" => "session_mounts",
        "window" => "scene_windows",
        _ => return Ok(false),
    };
    Ok(connection.query_row(
        &format!("SELECT EXISTS(SELECT id FROM {table} WHERE id = ? AND scene_id = ?)"),
        params![id, scene_id],
        |row| row.get(0),
    )?)
}

struct CanvasOwner<'a> {
    scene_id: &'a str,
    session_id: Option<&'a str>,
}

fn parse_canvas_owner(key: &str) -> Option<CanvasOwner<'_>> {
    let mut parts = key.split(':');
    let (kind, scene_id, value) = (parts.next()?, parts.next()?, parts.next());
    if scene_id.is_empty() || parts.next().is_some() {
        return None;
    }
    let session_id = match (kind, value) {
        ("dag-viewport", None) => None,
        ("session-group" | "dag-node-layout", Some(session)) if !session.is_empty() => {
            Some(session)
        }
        _ => return None,
    };
    Some(CanvasOwner {
        scene_id,
        session_id,
    })
}
