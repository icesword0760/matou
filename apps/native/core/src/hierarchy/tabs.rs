use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    HierarchyResult, Scene,
    canvas::CanvasError,
    navigation::{
        activate_scene_in_transaction, activate_workspace, read_hierarchy, register_window,
    },
};
use crate::storage::{CommandMetadata, DomainEventInput, execute};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameSceneInput {
    pub scene_id: String,
    pub name: String,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderSceneInput {
    pub window_id: String,
    pub scene_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_scene_id: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseSceneInput {
    pub window_id: String,
    pub scene_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_intent: Option<String>,
    pub now: i64,
}

pub fn rename_scene(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &RenameSceneInput,
    duplicate_scene_name: &str,
    committed_at: i64,
) -> Result<Value> {
    // ECMAScript String.trim includes U+FEFF and does not remove U+0085.
    let name = input.name.trim_matches(|character| {
        matches!(character,
            '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}'
            | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
            | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    });
    ensure!(!name.is_empty(), "Scene title must not be empty");
    let commit = execute(connection, command, committed_at, |tx, events| {
        let mut scene = require_scene(tx, &input.scene_id)?;
        let duplicate = tx
            .query_row(
                "SELECT id FROM scenes
             WHERE task_id = ? AND name = ? AND title_pinned = 1
               AND archived_at IS NULL AND id <> ?",
                params![scene.task_id, name, input.scene_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if duplicate {
            return Err(CanvasError {
                code: "DUPLICATE_SCENE_NAME",
                message: duplicate_scene_name.to_owned(),
            }
            .into());
        }
        tx.execute(
            "UPDATE scenes SET name = ?, title_pinned = 1, updated_at = ? WHERE id = ?",
            params![name, input.now, input.scene_id],
        )?;
        scene.name = name.to_owned();
        scene.title_pinned = true;
        scene.updated_at = input.now;
        events.push(DomainEventInput {
            event_id: format!("{}:scene-renamed", command.command_id),
            event_type: "scene.renamed".into(),
            aggregate_type: "scene".into(),
            aggregate_id: scene.id.clone(),
            workspace_id: None,
            task_id: Some(scene.task_id.clone()),
            session_id: None,
            payload: json!({"name": name, "titlePinned": true}),
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: input.now,
        });
        Ok(serde_json::to_value(scene)?)
    })?;
    Ok(commit.result)
}

pub fn reorder_scene(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &ReorderSceneInput,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        register_window(tx, &input.window_id, input.now)?;
        let task_id: String = tx
            .query_row(
                "SELECT task_id FROM scenes WHERE id = ? AND archived_at IS NULL",
                [&input.scene_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| missing("Scene"))?;
        let mut order = ordered_scenes(tx, &task_id)?;
        ensure!(
            order.contains(&input.scene_id),
            "Scene must belong to the Task"
        );
        ensure!(
            input
                .before_scene_id
                .as_ref()
                .is_none_or(|before| order.contains(before)),
            "before Scene must belong to the Task"
        );
        order.retain(|id| id != &input.scene_id);
        let target = input
            .before_scene_id
            .as_ref()
            .and_then(|before| order.iter().position(|id| id == before));
        order.insert(target.unwrap_or(order.len()), input.scene_id.clone());
        for (index, id) in order.iter().enumerate() {
            tx.execute(
                "UPDATE scenes SET sort_key = ?, updated_at = ? WHERE id = ?",
                params![format!("a{index:08}"), input.now, id],
            )?;
        }
        let workspace: String = tx
            .query_row(
                "SELECT workspace_id FROM tasks WHERE id = ?",
                [&task_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| missing("Task"))?;
        events.push(DomainEventInput {
            event_id: format!("{}:scene-order-changed", command.command_id),
            event_type: "task.scene-order-changed".into(),
            aggregate_type: "task".into(),
            aggregate_id: task_id.clone(),
            workspace_id: Some(workspace),
            task_id: Some(task_id),
            session_id: None,
            payload: json!({"sceneOrder": order}),
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: input.now,
        });
        activate_scene_in_transaction(tx, &input.window_id, &input.scene_id, input.now)?;
        Ok(serde_json::to_value(read_hierarchy(tx, &input.window_id)?)?)
    })?;
    Ok(commit.result)
}

pub fn close_scene(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &CloseSceneInput,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let window_id = &input.window_id;
        let scene_id = &input.scene_id;
        let now = input.now;
        register_window(tx, window_id, now)?;
        let scene = require_scene(tx, scene_id)?;
        let task_id = &scene.task_id;
        let workspace_id: String = tx
            .query_row(
                "SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL",
                [task_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| missing("Task"))?;
        let scenes = ordered_scenes(tx, task_id)?;
        let active_task_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM tasks WHERE workspace_id = ? AND archived_at IS NULL",
            [&workspace_id],
            |row| row.get(0),
        )?;
        if scenes.len() == 1 && active_task_count == 1 {
            return close_result(read_hierarchy(tx, window_id)?, "hide-window", &[]);
        }
        let affected_work_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM session_mounts
             JOIN sessions ON sessions.id = session_mounts.session_id
             WHERE session_mounts.scene_id = ? AND sessions.archived_at IS NULL
               AND sessions.work_status IN ('starting', 'running', 'needs-input')",
            [scene_id],
            |row| row.get(0),
        )?;
        ensure!(
            affected_work_count == 0
                || input.confirmed_intent.as_deref()
                    == Some(format!("close-scene:{scene_id}").as_str()),
            "Scene close intent is stale"
        );

        let mut mounted = tx.prepare(
            "SELECT sessions.id FROM session_mounts
             JOIN sessions ON sessions.id = session_mounts.session_id
             WHERE session_mounts.scene_id = ? AND sessions.archived_at IS NULL",
        )?;
        let disposed = mounted
            .query_map([scene_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        tx.execute(
            "UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
             WHERE id IN (SELECT session_id FROM session_mounts WHERE scene_id = ?)
               AND archived_at IS NULL",
            params![now, now, scene_id],
        )?;
        tx.execute(
            "UPDATE scenes SET archived_at = ?, updated_at = ? WHERE id = ?",
            params![now, now, scene_id],
        )?;
        tx.execute(
            "DELETE FROM window_scene_focus WHERE scene_id = ?",
            [scene_id],
        )?;
        events.push(DomainEventInput {
            event_id: format!("{}:scene-archived", command.command_id),
            event_type: "scene.archived".into(),
            aggregate_type: "scene".into(),
            aggregate_id: scene.id,
            workspace_id: Some(workspace_id.clone()),
            task_id: Some(task_id.clone()),
            session_id: None,
            payload: json!({"archivedAt": now, "disposedSessionIds": disposed}),
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: now,
        });

        if scenes.len() > 1 {
            let closed_index = scenes
                .iter()
                .position(|id| id == scene_id)
                .context("Scene must belong to the Task")?;
            let next = if closed_index + 1 < scenes.len() {
                closed_index + 1
            } else {
                closed_index - 1
            };
            activate_scene_in_transaction(tx, window_id, &scenes[next], now)?;
        } else {
            tx.execute(
                "UPDATE window_task_focus SET active_scene_id = NULL, updated_at = ?
                 WHERE task_id = ? AND active_scene_id = ?",
                params![now, task_id, scene_id],
            )?;
            if preferred_task(tx, window_id, &workspace_id)?.is_some_and(|next| next != *task_id) {
                // Activating this workspace chooses the same preferred task and
                // refreshes its existing scene and session focus in the same order.
                activate_workspace(tx, window_id, &workspace_id, now)?;
            }
        }
        close_result(read_hierarchy(tx, window_id)?, "closed", &disposed)
    })?;
    Ok(commit.result)
}

fn require_scene(connection: &Connection, scene_id: &str) -> Result<Scene> {
    Ok(connection
        .query_row(
            "SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL",
            [scene_id],
            Scene::from_row,
        )
        .optional()?
        .ok_or_else(|| missing("Scene"))?)
}

fn missing(label: &str) -> CanvasError {
    CanvasError {
        code: "ENTITY_NOT_FOUND",
        message: format!("{label} does not exist"),
    }
}

fn ordered_scenes(connection: &Connection, task_id: &str) -> Result<Vec<String>> {
    let mut query = connection.prepare(
        "SELECT id FROM scenes WHERE task_id = ? AND archived_at IS NULL
         ORDER BY sort_key, created_at, id",
    )?;
    Ok(query
        .query_map([task_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn preferred_task(
    connection: &Connection,
    window_id: &str,
    workspace_id: &str,
) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT tasks.id FROM tasks
         LEFT JOIN window_workspace_focus focus
           ON focus.window_id = ? AND focus.workspace_id = tasks.workspace_id
         WHERE tasks.workspace_id = ? AND tasks.archived_at IS NULL
         ORDER BY (tasks.id = focus.active_task_id) DESC, tasks.sort_key, tasks.created_at
         LIMIT 1",
            [window_id, workspace_id],
            |row| row.get(0),
        )
        .optional()?)
}

fn close_result(hierarchy: HierarchyResult, action: &str, disposed: &[String]) -> Result<Value> {
    let mut result = serde_json::to_value(hierarchy)?;
    result["action"] = action.into();
    result["disposedSessionIds"] = json!(disposed);
    Ok(result)
}
