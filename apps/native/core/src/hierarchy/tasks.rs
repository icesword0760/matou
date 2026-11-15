use std::{collections::HashSet, error::Error, fmt};

use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    HierarchyIds, HierarchyResult, Task, Workspace,
    navigation::{
        activate_task_in_transaction, preferred_task, read_created_path, read_hierarchy,
        read_navigation, register_window, restore_navigation_in_transaction,
    },
};
use crate::storage::{CommandMetadata, DomainEventInput, execute};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskInput {
    pub window_id: String,
    pub workspace_id: String,
    pub title: Option<String>,
    pub navigation: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameTaskInput {
    pub task_id: String,
    pub title: String,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderTaskInput {
    pub window_id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub before_task_id: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetTaskPinnedInput {
    pub task_id: String,
    pub pinned: bool,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderPinnedTaskInput {
    pub workspace_id: String,
    pub task_id: String,
    pub before_task_id: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteTaskInput {
    pub window_id: String,
    pub task_id: String,
    pub confirmed_intent: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectTaskInput {
    pub window_id: String,
    pub task_id: String,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordSessionInteractionInput {
    pub session_id: String,
    pub now: i64,
}

#[derive(Debug)]
pub struct TaskError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for TaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for TaskError {}

pub fn create_task(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &CreateTaskInput,
    ids: &HierarchyIds,
    new_task_title: &str,
    workspace_path_invalid: &str,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let window_id = &input.window_id;
        let now = input.now;
        register_window(tx, window_id, now)?;
        let navigation = read_navigation(tx, window_id)?;
        let workspace = require_workspace(tx, &input.workspace_id)?;
        if !workspace_path_is_available(tx, &workspace.id)? {
            return Err(TaskError {
                code: "WORKSPACE_PATH_INVALID",
                message: workspace_path_invalid.to_owned(),
            }
            .into());
        }
        let title = match &input.title {
            Some(title) => required_title(title)?.to_owned(),
            None => next_task_title(tx, &workspace.id, new_task_title)?,
        };
        assert_unique_title(tx, &workspace.id, &title, None)?;
        let created = insert_task_hierarchy(
            tx,
            events,
            NewTask {
                command_id: &command.command_id,
                window_id,
                workspace: &workspace,
                ids,
                title: &title,
                now,
                last_opened_at: 0,
            },
        )?;
        if input.navigation.as_deref() == Some("preserve") {
            restore_navigation_in_transaction(tx, &navigation, now)?;
        }
        let mut result = serde_json::to_value(read_hierarchy(tx, window_id)?)?;
        result["created"] = created;
        Ok(result)
    })?;
    Ok(commit.result)
}

pub fn rename_task(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &RenameTaskInput,
    committed_at: i64,
) -> Result<Value> {
    let title = required_title(&input.title)?;
    let commit = execute(connection, command, committed_at, |tx, events| {
        let mut task = require_task(tx, &input.task_id)?;
        assert_unique_title(tx, &task.workspace_id, title, Some(&task.id))?;
        tx.execute(
            "UPDATE tasks SET title = ?, updated_at = ?, version = version + 1 WHERE id = ?",
            params![title, input.now, task.id],
        )?;
        task.title = title.to_owned();
        task.updated_at = input.now;
        task.version += 1;
        events.push(task_event(
            command,
            &task,
            "task-renamed",
            "task.renamed",
            json!({"title": title}),
            input.now,
        ));
        Ok(serde_json::to_value(task)?)
    })?;
    Ok(commit.result)
}

pub fn reorder_task(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &ReorderTaskInput,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let now = input.now;
        register_window(tx, &input.window_id, now)?;
        let workspace = require_workspace(tx, &input.workspace_id)?;
        let active_ids = active_task_ids(tx, &workspace.id)?;
        ensure!(
            active_ids.contains(&input.task_id),
            "Task must belong to the Workspace"
        );
        ensure!(
            input
                .before_task_id
                .as_ref()
                .is_none_or(|id| active_ids.contains(id)),
            "before Task must belong to the Workspace"
        );
        let mut order = workspace.task_order;
        order.retain(|id| active_ids.contains(id));
        let source = order.iter().position(|id| id == &input.task_id);
        let target = match &input.before_task_id {
            Some(before) => order.iter().position(|id| id == before),
            None => order.len().checked_sub(1),
        };
        if let (Some(source), Some(target)) = (source, target)
            && source != target
        {
            let task_id = order.remove(source);
            // The reference uses the destination index from before removing the source.
            order.insert(target, task_id);
            update_task_order(tx, &input.workspace_id, &order, now, true)?;
            events.push(workspace_event(
                &command.command_id,
                &input.workspace_id,
                "task-order-changed",
                "workspace.task-order-changed",
                json!({"taskOrder": order}),
                now,
            ));
        }
        let mut result = serde_json::to_value(read_hierarchy(tx, &input.window_id)?)?;
        result["taskOrder"] = serde_json::to_value(order)?;
        Ok(result)
    })?;
    Ok(commit.result)
}

pub fn set_task_pinned(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &SetTaskPinnedInput,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let mut task = require_task(tx, &input.task_id)?;
        let key = if input.pinned {
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM tasks WHERE workspace_id = ?
                 AND archived_at IS NULL AND is_pinned = 1",
                [&task.workspace_id],
                |row| row.get(0),
            )?;
            format!("a{count:08}")
        } else {
            String::new()
        };
        tx.execute(
            "UPDATE tasks SET is_pinned = ?, pin_sort_key = ?, updated_at = ?, version = version + 1
             WHERE id = ?",
            params![input.pinned, key, input.now, task.id],
        )?;
        task.is_pinned = input.pinned;
        task.pin_sort_key = key;
        task.updated_at = input.now;
        task.version += 1;
        events.push(task_event(
            command,
            &task,
            "task-pinned",
            "task.pin-changed",
            json!({"isPinned": task.is_pinned, "pinSortKey": task.pin_sort_key}),
            input.now,
        ));
        Ok(serde_json::to_value(task)?)
    })?;
    Ok(commit.result)
}

pub fn reorder_pinned_task(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &ReorderPinnedTaskInput,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let mut query = tx.prepare(
            "SELECT id FROM tasks WHERE workspace_id = ? AND archived_at IS NULL AND is_pinned = 1
             ORDER BY pin_sort_key, id",
        )?;
        let mut order = query
            .query_map([&input.workspace_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            order.contains(&input.task_id),
            "Pinned item is outside the requested group"
        );
        ensure!(
            input
                .before_task_id
                .as_ref()
                .is_none_or(|id| order.contains(id)),
            "Pinned destination is outside the requested group"
        );
        order.retain(|id| id != &input.task_id);
        let target = match &input.before_task_id {
            // JavaScript splice(-1, 0, source) inserts just before the last item.
            Some(before) => order
                .iter()
                .position(|id| id == before)
                .unwrap_or_else(|| order.len().saturating_sub(1)),
            None => order.len(),
        };
        order.insert(target, input.task_id.clone());
        for (index, id) in order.iter().enumerate() {
            tx.execute(
                "UPDATE tasks SET pin_sort_key = ?, updated_at = ?, version = version + 1
                 WHERE id = ?",
                params![sort_key(index), input.now, id],
            )?;
        }
        events.push(workspace_event(
            &command.command_id,
            &input.workspace_id,
            "task-pin-order",
            "task.pin-order-changed",
            json!({"taskIds": order}),
            input.now,
        ));
        let tasks = order
            .iter()
            .map(|id| require_task(tx, id))
            .collect::<Result<Vec<_>>>()?;
        Ok(serde_json::to_value(tasks)?)
    })?;
    Ok(commit.result)
}

pub fn select_task(
    connection: &mut Connection,
    input: &SelectTaskInput,
) -> Result<HierarchyResult> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    register_window(&tx, &input.window_id, input.now)?;
    activate_task_in_transaction(&tx, &input.window_id, &input.task_id, input.now)?;
    let result = read_hierarchy(&tx, &input.window_id)?;
    tx.commit()?;
    Ok(result)
}

pub fn list_tasks(
    connection: &Connection,
    window_id: &str,
    workspace_id: &str,
) -> Result<Vec<Task>> {
    let mut query = connection.prepare(
        "SELECT tasks.* FROM tasks
         WHERE (NOT EXISTS (SELECT 1 FROM window_task_placements)
           OR EXISTS (SELECT 1 FROM window_task_placements AS placement WHERE placement.task_id = tasks.id AND placement.window_id = ?))
           AND tasks.workspace_id = ? AND tasks.archived_at IS NULL
         ORDER BY tasks.is_pinned DESC,
           CASE WHEN tasks.is_pinned = 1 THEN tasks.pin_sort_key END,
           CASE WHEN tasks.is_pinned = 1 THEN tasks.id END,
           tasks.last_opened_at DESC, tasks.created_at, tasks.id",
    )?;
    Ok(query
        .query_map([window_id, workspace_id], Task::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn record_session_interaction(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &RecordSessionInteractionInput,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let (task_id, workspace_id): (String, String) = tx
            .query_row(
                "SELECT sessions.task_id, tasks.workspace_id
                 FROM sessions JOIN tasks ON tasks.id = sessions.task_id
                 WHERE sessions.id = ? AND sessions.archived_at IS NULL AND tasks.archived_at IS NULL",
                [&input.session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| missing("Session"))?;
        tx.execute(
            "UPDATE tasks SET last_opened_at = ? WHERE id = ?",
            params![input.now, task_id],
        )?;
        tx.execute(
            "UPDATE workspaces SET last_opened_at = ? WHERE id = ?",
            params![input.now, workspace_id],
        )?;
        let result = json!({
            "sessionId": input.session_id,
            "taskId": task_id,
            "workspaceId": workspace_id,
            "lastOpenedAt": input.now
        });
        events.push(DomainEventInput {
            event_id: format!("{}:navigation-recency", command.command_id),
            event_type: "navigation.recency-changed".into(),
            aggregate_type: "task-navigation".into(),
            aggregate_id: task_id.clone(),
            workspace_id: Some(workspace_id),
            task_id: Some(task_id),
            session_id: Some(input.session_id.clone()),
            payload: result.clone(),
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: input.now,
        });
        Ok(result)
    })?;
    Ok(commit.result)
}

pub fn delete_task(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &DeleteTaskInput,
    ids: &HierarchyIds,
    default_task_title: &str,
    committed_at: i64,
) -> Result<Value> {
    ensure!(
        input.confirmed_intent.as_deref()
            == Some(format!("delete-task:{}", input.task_id).as_str()),
        "Task deletion intent is stale"
    );
    let commit = execute(connection, command, committed_at, |tx, events| {
        let window_id = &input.window_id;
        let now = input.now;
        register_window(tx, window_id, now)?;
        let task = require_task(tx, &input.task_id)?;
        let workspace = require_workspace(tx, &task.workspace_id)?;
        let mut sessions = tx.prepare(
            "SELECT id FROM sessions WHERE task_id = ? AND archived_at IS NULL ORDER BY created_at",
        )?;
        let disposed_session_ids = sessions
            .query_map([&task.id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut windows = tx.prepare(
            "SELECT window_id FROM window_workspace_focus
             WHERE workspace_id = ? AND active_task_id = ?",
        )?;
        let affected_windows = windows
            .query_map(params![workspace.id, task.id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        archive_task(tx, &task.id, now)?;
        let order = workspace
            .task_order
            .into_iter()
            .filter(|id| id != &task.id)
            .collect::<Vec<_>>();
        update_task_order(tx, &workspace.id, &order, now, false)?;
        events.push(task_event(
            command,
            &task,
            "task-archived",
            "task.archived",
            json!({"archivedAt": now, "disposedSessionIds": disposed_session_ids}),
            now,
        ));

        if let Some(next_task) = preferred_task(tx, window_id, &workspace.id)? {
            activate_task_in_transaction(tx, window_id, &next_task, now)?;
        } else if workspace_path_is_available(tx, &workspace.id)? {
            let workspace = require_workspace(tx, &workspace.id)?;
            insert_task_hierarchy(
                tx,
                events,
                NewTask {
                    command_id: &command.command_id,
                    window_id,
                    workspace: &workspace,
                    ids,
                    title: default_task_title,
                    now,
                    last_opened_at: now,
                },
            )?;
        } else {
            tx.execute(
                "INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at)
                 VALUES (?, ?, NULL, ?)
                 ON CONFLICT(window_id, workspace_id) DO UPDATE SET
                   active_task_id = NULL, updated_at = excluded.updated_at",
                params![window_id, workspace.id, now],
            )?;
        }
        for affected in affected_windows {
            if let Some(next_task) = preferred_task(tx, &affected, &workspace.id)? {
                activate_task_in_transaction(tx, &affected, &next_task, now)?;
            } else {
                tx.execute(
                    "UPDATE window_workspace_focus SET active_task_id = NULL, updated_at = ?
                     WHERE window_id = ? AND workspace_id = ?",
                    params![now, affected, workspace.id],
                )?;
            }
        }
        let mut result = serde_json::to_value(read_hierarchy(tx, window_id)?)?;
        result["disposedSessionIds"] = serde_json::to_value(disposed_session_ids)?;
        Ok(result)
    })?;
    Ok(commit.result)
}

fn archive_task(tx: &Connection, task_id: &str, now: i64) -> Result<()> {
    tx.execute(
        "UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE task_id = ? AND archived_at IS NULL",
        params![now, now, task_id],
    )?;
    tx.execute(
        "UPDATE scenes SET archived_at = ?, updated_at = ? WHERE task_id = ? AND archived_at IS NULL",
        params![now, now, task_id],
    )?;
    tx.execute(
        "UPDATE tasks SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE id = ?",
        params![now, now, task_id],
    )?;
    for statement in [
        "DELETE FROM window_task_placements WHERE task_id = ?",
        "DELETE FROM window_scene_focus WHERE scene_id IN (SELECT id FROM scenes WHERE task_id = ?)",
        "DELETE FROM window_task_focus WHERE task_id = ?",
        "DELETE FROM task_status_entries WHERE task_id = ?",
        "DELETE FROM task_progress WHERE task_id = ?",
        "DELETE FROM task_logs WHERE task_id = ?",
    ] {
        tx.execute(statement, [task_id])?;
    }
    Ok(())
}

struct NewTask<'a> {
    command_id: &'a str,
    window_id: &'a str,
    workspace: &'a Workspace,
    ids: &'a HierarchyIds,
    title: &'a str,
    now: i64,
    last_opened_at: i64,
}

fn insert_task_hierarchy(
    tx: &Connection,
    events: &mut Vec<DomainEventInput>,
    task: NewTask<'_>,
) -> Result<Value> {
    let NewTask {
        command_id,
        window_id,
        workspace,
        ids,
        title,
        now,
        last_opened_at,
    } = task;
    let mut order = Vec::new();
    let mut exists =
        tx.prepare_cached("SELECT 1 FROM tasks WHERE id = ? AND archived_at IS NULL")?;
    for id in &workspace.task_order {
        if exists.exists([id])? {
            order.push(id.clone());
        }
    }
    let ordinal = order.len();
    order.push(ids.task_id.clone());
    let title = required_title(title)?;
    let root = &workspace.root_directory;
    tx.execute(
        "INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
         VALUES (?, ?, 'plain-directory', ?, ?)",
        params![ids.execution_context_id, workspace.id, root, now],
    )?;
    tx.execute(
        "INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, sort_key,
           created_at, updated_at, version, last_opened_at)
         VALUES (?, ?, ?, ?, 'planned', ?, ?, ?, 1, ?)",
        params![
            ids.task_id,
            workspace.id,
            ids.execution_context_id,
            title,
            sort_key(ordinal),
            now,
            now,
            last_opened_at
        ],
    )?;
    tx.execute(
        "INSERT INTO scenes (id, task_id, name, mode, root_node_id, title_pinned, sort_key,
           layout_revision, created_at, updated_at)
         VALUES (?, ?, ?, 'tile', ?, 0, 'a0', 1, ?, ?)",
        params![
            ids.scene_id,
            ids.task_id,
            format!("Shell · {root}"),
            ids.root_node_id,
            now,
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
         VALUES (?, ?, 'root', 0, ?)",
        params![ids.root_node_id, ids.scene_id, now],
    )?;
    tx.execute(
        "INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title,
           created_at, updated_at, last_activity_at, version)
         VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, 1)",
        params![
            ids.session_id,
            ids.task_id,
            ids.execution_context_id,
            now,
            now,
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at)
         VALUES (?, ?, ?, ?, ?)",
        params![
            ids.mount_id,
            ids.scene_id,
            ids.root_node_id,
            ids.session_id,
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO window_task_placements (window_id, task_id, ordinal, updated_at)
         VALUES (?, ?, ?, ?)",
        params![window_id, ids.task_id, ordinal as i64, now],
    )?;
    update_workspace_order(tx, &workspace.id, &order, now)?;
    activate_task_in_transaction(tx, window_id, &ids.task_id, now)?;
    let created = read_created_path(tx, &ids.session_id)?;
    for (field, id, event_type, aggregate_type, aggregate_id, session_id) in [
        (
            "task",
            &ids.task_id,
            "task.created",
            "task",
            &ids.task_id,
            None,
        ),
        (
            "scene",
            &ids.scene_id,
            "scene.created",
            "scene",
            &ids.scene_id,
            None,
        ),
        (
            "session",
            &ids.session_id,
            "session.created",
            "session",
            &ids.session_id,
            Some(&ids.session_id),
        ),
        (
            "mount",
            &ids.mount_id,
            "scene.session-mounted",
            "scene",
            &ids.scene_id,
            Some(&ids.session_id),
        ),
    ] {
        events.push(DomainEventInput {
            event_id: format!("{command_id}:{id}:created"),
            event_type: event_type.into(),
            aggregate_type: aggregate_type.into(),
            aggregate_id: aggregate_id.clone(),
            workspace_id: Some(workspace.id.clone()),
            task_id: Some(ids.task_id.clone()),
            session_id: session_id.cloned(),
            payload: created[field].clone(),
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: now,
        });
    }
    events.push(workspace_event(
        command_id,
        &workspace.id,
        "workspace-task-order",
        "workspace.task-order-changed",
        json!({"taskOrder": order}),
        now,
    ));
    Ok(created)
}

fn update_task_order(
    tx: &Connection,
    workspace_id: &str,
    order: &[String],
    now: i64,
    reorder: bool,
) -> Result<()> {
    if !reorder {
        update_workspace_order(tx, workspace_id, order, now)?;
    }
    for (index, id) in order.iter().enumerate() {
        let key = sort_key(index);
        if reorder {
            tx.execute(
                "UPDATE tasks SET sort_key = ?, updated_at = ?, version = version + 1 WHERE id = ?",
                params![key, now, id],
            )?;
        } else {
            tx.execute(
                "UPDATE tasks SET sort_key = ? WHERE id = ?",
                params![key, id],
            )?;
        }
        tx.execute(
            "UPDATE window_task_placements SET ordinal = ?, updated_at = ? WHERE task_id = ?",
            params![index as i64, now, id],
        )?;
    }
    if reorder {
        update_workspace_order(tx, workspace_id, order, now)?;
    }
    Ok(())
}

fn update_workspace_order(
    tx: &Connection,
    workspace_id: &str,
    order: &[String],
    now: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE workspaces SET task_order_json = ?, updated_at = ?, version = version + 1
         WHERE id = ?",
        params![serde_json::to_string(order)?, now, workspace_id],
    )?;
    Ok(())
}

fn active_task_ids(tx: &Connection, workspace_id: &str) -> Result<HashSet<String>> {
    let mut query =
        tx.prepare("SELECT id FROM tasks WHERE workspace_id = ? AND archived_at IS NULL")?;
    Ok(query
        .query_map([workspace_id], |row| row.get(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?)
}

fn require_task(tx: &Connection, task_id: &str) -> Result<Task> {
    Ok(tx
        .query_row(
            "SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL",
            [task_id],
            Task::from_row,
        )
        .optional()?
        .ok_or_else(|| missing("Task"))?)
}

fn require_workspace(tx: &Connection, workspace_id: &str) -> Result<Workspace> {
    Ok(tx
        .query_row(
            "SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL",
            [workspace_id],
            Workspace::from_row,
        )
        .optional()?
        .ok_or_else(|| missing("Workspace"))?)
}

fn missing(entity: &str) -> TaskError {
    TaskError {
        code: "ENTITY_NOT_FOUND",
        message: format!("{entity} does not exist"),
    }
}

fn workspace_path_is_available(tx: &Connection, workspace_id: &str) -> Result<bool> {
    Ok(!tx
        .prepare(
            "SELECT 1 FROM workspace_path_state WHERE workspace_id = ? AND status = 'invalid'",
        )?
        .exists([workspace_id])?)
}

fn next_task_title(tx: &Connection, workspace_id: &str, base: &str) -> Result<String> {
    let mut query =
        tx.prepare("SELECT title FROM tasks WHERE workspace_id = ? AND archived_at IS NULL")?;
    let titles = query
        .query_map([workspace_id], |row| row.get(0))?
        .collect::<rusqlite::Result<HashSet<String>>>()?;
    let mut title = base.to_owned();
    let mut suffix = 2;
    while titles.contains(&title) {
        title = format!("{base} {suffix}");
        suffix += 1;
    }
    Ok(title)
}

fn assert_unique_title(
    tx: &Connection,
    workspace_id: &str,
    title: &str,
    except: Option<&str>,
) -> Result<()> {
    let duplicate = tx
        .prepare(
            "SELECT 1 FROM tasks WHERE workspace_id = ? AND title = ? AND archived_at IS NULL
         AND (? IS NULL OR id <> ?)",
        )?
        .exists(params![workspace_id, title, except, except])?;
    ensure!(
        !duplicate,
        "an active Task named \"{title}\" already exists in this Workspace"
    );
    Ok(())
}

fn required_title(value: &str) -> Result<&str> {
    // Match ECMAScript String.trim, including U+FEFF and excluding U+0085.
    let title = value.trim_matches(|character| {
        matches!(character,
            '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}'
            | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
            | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    });
    ensure!(!title.is_empty(), "Task title must not be empty");
    Ok(title)
}

fn sort_key(index: usize) -> String {
    format!("a{index:08}")
}

fn task_event(
    command: &CommandMetadata,
    task: &Task,
    suffix: &str,
    event_type: &str,
    payload: Value,
    now: i64,
) -> DomainEventInput {
    DomainEventInput {
        event_id: format!("{}:{suffix}", command.command_id),
        event_type: event_type.into(),
        aggregate_type: "task".into(),
        aggregate_id: task.id.clone(),
        workspace_id: Some(task.workspace_id.clone()),
        task_id: Some(task.id.clone()),
        session_id: None,
        payload,
        schema_version: None,
        required_terminal_sequence: None,
        occurred_at: now,
    }
}

fn workspace_event(
    command_id: &str,
    workspace_id: &str,
    suffix: &str,
    event_type: &str,
    payload: Value,
    now: i64,
) -> DomainEventInput {
    DomainEventInput {
        event_id: format!("{command_id}:{suffix}"),
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
