use std::{error::Error, fmt};

use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    CompleteHierarchy, HierarchyIds, HierarchyResult, Workspace, create_complete_hierarchy,
    navigation::{
        activate_workspace, read_hierarchy, read_navigation, register_window,
        restore_navigation_in_transaction,
    },
};
use crate::storage::{CommandMetadata, DomainEventInput, execute};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkspaceInput {
    pub window_id: String,
    pub name: String,
    /// Absolute path, lexically resolved by the platform runtime.
    pub root_directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigation: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectWorkspaceInput {
    pub window_id: String,
    pub workspace_id: String,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameWorkspaceInput {
    pub workspace_id: String,
    pub name: String,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkWorkspaceInput {
    pub workspace_id: String,
    /// Absolute path, lexically resolved by the platform runtime.
    pub root_directory: String,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveWorkspaceInput {
    pub window_id: String,
    pub workspace_id: String,
    pub confirmed_intent: String,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetWorkspacePinnedInput {
    pub workspace_id: String,
    pub pinned: bool,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderPinnedWorkspaceInput {
    pub workspace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_workspace_id: Option<String>,
    pub now: i64,
}

#[derive(Debug)]
pub struct WorkspaceError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for WorkspaceError {}

pub fn list_workspaces(connection: &Connection) -> Result<Vec<Workspace>> {
    let mut query = connection
        .prepare("SELECT * FROM workspaces WHERE archived_at IS NULL ORDER BY created_at, id")?;
    Ok(query
        .query_map([], Workspace::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn create_workspace(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &CreateWorkspaceInput,
    ids: &HierarchyIds,
    default_task_title: &str,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        register_window(tx, &input.window_id, input.now)?;
        let navigation = read_navigation(tx, &input.window_id)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM workspaces WHERE root_directory = ? AND archived_at IS NULL
             ORDER BY created_at LIMIT 1",
                [&input.root_directory],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(workspace_id) = existing {
            activate_workspace(tx, &input.window_id, &workspace_id, input.now)?;
        } else {
            create_complete_hierarchy(
                tx,
                events,
                CompleteHierarchy {
                    window_id: &input.window_id,
                    name: &input.name,
                    root: &input.root_directory,
                    now: input.now,
                    is_default: false,
                    is_pinned: false,
                },
                ids,
                default_task_title,
                &command.command_id,
            )?;
        }
        let hierarchy = read_hierarchy(tx, &input.window_id)?;
        let (Some(workspace), Some(task), Some(scene), Some(session), Some(mount)) = (
            hierarchy.workspace,
            hierarchy.task,
            hierarchy.scene,
            hierarchy.session,
            hierarchy.mount,
        ) else {
            bail!("Created hierarchy path is incomplete");
        };
        if input.navigation.as_deref() == Some("preserve") {
            restore_navigation_in_transaction(tx, &navigation, input.now)?;
        }
        let mut result = serde_json::to_value(read_hierarchy(tx, &input.window_id)?)?;
        result["created"] = json!({
            "workspace": workspace, "task": task, "scene": scene, "session": session, "mount": mount
        });
        Ok(result)
    })?;
    Ok(commit.result)
}

pub fn select_workspace(
    connection: &mut Connection,
    input: &SelectWorkspaceInput,
) -> Result<HierarchyResult> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    register_window(&tx, &input.window_id, input.now)?;
    ensure_workspace(&tx, &input.workspace_id)?;
    activate_workspace(&tx, &input.window_id, &input.workspace_id, input.now)?;
    let result = read_hierarchy(&tx, &input.window_id)?;
    tx.commit()?;
    Ok(result)
}

pub fn rename_workspace(
    connection: &Connection,
    input: &RenameWorkspaceInput,
    follows_directory_message: &str,
) -> Result<Value> {
    ensure_workspace(connection, &input.workspace_id)?;
    bail!("{follows_directory_message}")
}

pub fn relink_workspace(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &RelinkWorkspaceInput,
    default_home_message: &str,
    directory_taken_message: &str,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let workspace_id = &input.workspace_id;
        let mut workspace = require_workspace(tx, workspace_id)?;
        ensure!(!workspace.is_default, "{default_home_message}");
        let root = &input.root_directory;
        let duplicate = tx.prepare(
            "SELECT 1 FROM workspaces WHERE root_directory = ? AND archived_at IS NULL AND id <> ?",
        )?.exists(params![root, workspace_id])?;
        if duplicate {
            return Err(WorkspaceError {
                code: "WORKSPACE_DIRECTORY_TAKEN",
                message: directory_taken_message.to_owned(),
            }
            .into());
        }
        let identity = format!("path:{root}");
        let now = input.now;
        tx.execute(
            "UPDATE workspaces SET root_directory = ?, path_identity = ?, updated_at = ?, version = version + 1
             WHERE id = ?",
            params![root, identity, now, workspace_id],
        )?;
        tx.execute(
            "UPDATE execution_contexts SET cwd = ? WHERE workspace_id = ? AND archived_at IS NULL",
            params![root, workspace_id],
        )?;
        tx.execute(
            "UPDATE sessions SET cwd = ?, updated_at = ?, version = version + 1
             WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?) AND archived_at IS NULL",
            params![root, now, workspace_id],
        )?;
        tx.execute(
            "UPDATE scenes SET name = ?, updated_at = ?
             WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?)
                AND archived_at IS NULL AND title_pinned = 0",
            params![format!("Shell · {root}"), now, workspace_id],
        )?;
        tx.execute(
            "DELETE FROM workspace_path_state WHERE workspace_id = ?",
            [workspace_id],
        )?;
        events.push(workspace_event(
            command,
            workspace_id,
            "workspace-relinked",
            "workspace.relinked",
            json!({"rootDirectory": root, "pathIdentity": identity}),
            now,
        ));
        workspace.root_directory = root.clone();
        workspace.path_identity = Some(identity);
        workspace.updated_at = now;
        workspace.version += 1;
        Ok(serde_json::to_value(workspace)?)
    })?;
    Ok(commit.result)
}

pub fn set_workspace_pinned(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &SetWorkspacePinnedInput,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let workspace_id = &input.workspace_id;
        let mut workspace = require_workspace(tx, workspace_id)?;
        let key = if input.pinned {
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM workspaces WHERE archived_at IS NULL AND is_pinned = 1",
                [],
                |row| row.get(0),
            )?;
            format!("a{count:08}")
        } else {
            String::new()
        };
        tx.execute(
            "UPDATE workspaces SET is_pinned = ?, pin_sort_key = ?, updated_at = ?, version = version + 1
             WHERE id = ?",
            params![input.pinned, key, input.now, workspace_id],
        )?;
        events.push(workspace_event(
            command,
            workspace_id,
            "workspace-pinned",
            "workspace.pin-changed",
            json!({"isPinned": input.pinned, "pinSortKey": key}),
            input.now,
        ));
        workspace.is_pinned = input.pinned;
        workspace.pin_sort_key = key;
        workspace.updated_at = input.now;
        workspace.version += 1;
        Ok(serde_json::to_value(workspace)?)
    })?;
    Ok(commit.result)
}

pub fn reorder_pinned_workspace(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &ReorderPinnedWorkspaceInput,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let mut query = tx.prepare(
            "SELECT id FROM workspaces WHERE archived_at IS NULL AND is_pinned = 1
             ORDER BY pin_sort_key, id",
        )?;
        let mut order = query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            order.contains(&input.workspace_id),
            "Pinned item is outside the requested group"
        );
        ensure!(
            input
                .before_workspace_id
                .as_ref()
                .is_none_or(|id| order.contains(id)),
            "Pinned destination is outside the requested group"
        );
        order.retain(|id| id != &input.workspace_id);
        let target = match &input.before_workspace_id {
            // The frozen service's splice(-1, 0, source) inserts just before the last item.
            Some(id) => order
                .iter()
                .position(|candidate| candidate == id)
                .unwrap_or_else(|| order.len().saturating_sub(1)),
            None => order.len(),
        };
        order.insert(target, input.workspace_id.clone());
        for (index, id) in order.iter().enumerate() {
            tx.execute(
                "UPDATE workspaces SET pin_sort_key = ?, updated_at = ?, version = version + 1 WHERE id = ?",
                params![format!("a{index:08}"), input.now, id],
            )?;
        }
        events.push(DomainEventInput {
            event_id: format!("{}:workspace-pin-order", command.command_id),
            event_type: "workspace.pin-order-changed".into(),
            aggregate_type: "workspace-navigation".into(),
            aggregate_id: "global".into(),
            workspace_id: None,
            task_id: None,
            session_id: None,
            payload: json!({"workspaceIds": order}),
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: input.now,
        });
        let workspaces = order
            .iter()
            .map(|id| require_workspace(tx, id))
            .collect::<Result<Vec<_>>>()?;
        Ok(serde_json::to_value(workspaces)?)
    })?;
    Ok(commit.result)
}

pub fn remove_workspace(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &RemoveWorkspaceInput,
    default_kept_message: &str,
    committed_at: i64,
) -> Result<Value> {
    ensure!(
        input.confirmed_intent == format!("remove-workspace:{}", input.workspace_id),
        "Workspace removal intent is stale"
    );
    let commit =
        execute(connection, command, committed_at, |tx, events| {
            let workspace_id = &input.workspace_id;
            let now = input.now;
            register_window(tx, &input.window_id, now)?;
            let workspace = lookup_workspace(tx, workspace_id)?;
            let Some(workspace) = workspace else {
                bail!("Workspace {workspace_id} does not exist");
            };
            ensure!(!workspace.is_default, "{default_kept_message}");
            let mut sessions = tx.prepare(
                "SELECT sessions.id FROM sessions JOIN tasks ON tasks.id = sessions.task_id
             WHERE tasks.workspace_id = ? AND sessions.archived_at IS NULL",
            )?;
            let disposed_session_ids = sessions
                .query_map([workspace_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            archive_workspace(tx, workspace_id, now)?;
            let default_created: Option<String> = tx.query_row(
            "SELECT value_json FROM bootstrap_state WHERE key = 'default-workspace-created'",
            [],
            |row| row.get(0),
        ).optional()?;
            if default_created
                .as_deref()
                .and_then(|value| serde_json::from_str::<String>(value).ok())
                .as_ref()
                == Some(workspace_id)
            {
                tx.execute(
                    "INSERT INTO bootstrap_state (key, value_json, updated_at)
                 VALUES ('default-workspace-removed', 'true', ?)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json,
                     updated_at = excluded.updated_at",
                    [now],
                )?;
            }

            let next_workspace: Option<String> = tx
                .query_row(
                    "SELECT id FROM workspaces WHERE archived_at IS NULL
             ORDER BY is_pinned DESC,
               CASE WHEN is_pinned = 1 THEN pin_sort_key END,
               CASE WHEN is_pinned = 0 THEN last_opened_at END DESC,
               created_at DESC, id LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            let mut windows = tx.prepare(
                "SELECT app_windows.id FROM app_windows
             LEFT JOIN window_navigation ON window_navigation.window_id = app_windows.id
             WHERE window_navigation.active_workspace_id = ? OR app_windows.id = ?",
            )?;
            let affected_windows = windows
                .query_map(params![workspace_id, input.window_id], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for window_id in affected_windows {
                if let Some(next_workspace) = &next_workspace {
                    activate_workspace(tx, &window_id, next_workspace, now)?;
                } else {
                    tx.execute(
                        "UPDATE window_navigation SET active_workspace_id = NULL, updated_at = ?
                     WHERE window_id = ?",
                        params![now, window_id],
                    )?;
                }
            }

            events.push(workspace_event(
                command,
                workspace_id,
                "workspace-removed",
                "workspace.archived",
                json!({"archivedAt": now}),
                now,
            ));
            let mut result = serde_json::to_value(read_hierarchy(tx, &input.window_id)?)?;
            result["disposedSessionIds"] = serde_json::to_value(disposed_session_ids)?;
            Ok(result)
        })?;
    Ok(commit.result)
}

fn archive_workspace(tx: &Connection, workspace_id: &str, now: i64) -> Result<()> {
    tx.execute(
        "UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?) AND archived_at IS NULL",
        params![now, now, workspace_id],
    )?;
    tx.execute(
        "UPDATE scenes SET archived_at = ?, updated_at = ?
         WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?) AND archived_at IS NULL",
        params![now, now, workspace_id],
    )?;
    tx.execute(
        "UPDATE tasks SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE workspace_id = ? AND archived_at IS NULL",
        params![now, now, workspace_id],
    )?;
    tx.execute(
        "UPDATE execution_contexts SET archived_at = ? WHERE workspace_id = ? AND archived_at IS NULL",
        params![now, workspace_id],
    )?;
    tx.execute(
        "UPDATE workspaces SET archived_at = ?, updated_at = ?, version = version + 1
         WHERE id = ?",
        params![now, now, workspace_id],
    )?;
    for sql in [
        "DELETE FROM window_scene_focus WHERE scene_id IN (
           SELECT scenes.id FROM scenes JOIN tasks ON tasks.id = scenes.task_id
           WHERE tasks.workspace_id = ?
         )",
        "DELETE FROM window_task_focus WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?)",
        "DELETE FROM window_workspace_focus WHERE workspace_id = ?",
        "DELETE FROM window_task_placements WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?)",
    ] {
        tx.execute(sql, [workspace_id])?;
    }
    Ok(())
}

fn lookup_workspace(connection: &Connection, workspace_id: &str) -> Result<Option<Workspace>> {
    Ok(connection
        .query_row(
            "SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL",
            [workspace_id],
            Workspace::from_row,
        )
        .optional()?)
}

fn require_workspace(connection: &Connection, workspace_id: &str) -> Result<Workspace> {
    Ok(
        lookup_workspace(connection, workspace_id)?.ok_or_else(|| WorkspaceError {
            code: "ENTITY_NOT_FOUND",
            message: "Workspace does not exist".into(),
        })?,
    )
}

fn ensure_workspace(connection: &Connection, workspace_id: &str) -> Result<()> {
    let exists = connection
        .prepare("SELECT 1 FROM workspaces WHERE id = ? AND archived_at IS NULL")?
        .exists([workspace_id])?;
    ensure!(exists, "Workspace {workspace_id} does not exist");
    Ok(())
}

fn workspace_event(
    command: &CommandMetadata,
    workspace_id: &str,
    suffix: &str,
    event_type: &str,
    payload: Value,
    now: i64,
) -> DomainEventInput {
    DomainEventInput {
        event_id: format!("{}:{suffix}", command.command_id),
        event_type: event_type.into(),
        aggregate_type: "workspace".into(),
        aggregate_id: workspace_id.to_owned(),
        workspace_id: Some(workspace_id.to_owned()),
        task_id: None,
        session_id: None,
        payload,
        schema_version: None,
        required_terminal_sequence: None,
        occurred_at: now,
    }
}
