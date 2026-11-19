pub mod canvas;
pub mod cards;
mod model;
mod navigation;
pub mod tabs;
pub mod tasks;
pub mod workspaces;

use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::storage::{CommandMetadata, DomainEventInput, execute};

pub use model::{
    ExecutionContext, HierarchyResult, Mount, Scene, Session, Task, WindowNavigation, Workspace,
};
pub use navigation::{focus_session, list_scenes, read_created_path, read_hierarchy, select_scene};

use navigation::{activate_workspace, register_window};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HierarchyIds {
    pub workspace_id: String,
    pub execution_context_id: String,
    pub task_id: String,
    pub scene_id: String,
    pub root_node_id: String,
    pub session_id: String,
    pub mount_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapWindowInput {
    pub window_id: String,
    /// Absolute path, lexically resolved by the platform runtime.
    pub default_root_directory: String,
    pub default_name: String,
    pub now: i64,
}

/// Bootstrap a window and return its active hierarchy through the durable command receipt.
pub fn bootstrap_window(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &BootstrapWindowInput,
    ids: &HierarchyIds,
    default_task_title: &str,
    committed_at: i64,
) -> Result<HierarchyResult> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        register_window(tx, &input.window_id, input.now)?;

        let active_before: Option<String> = tx
            .query_row(
                "SELECT workspaces.id FROM window_navigation
             JOIN workspaces ON workspaces.id = window_navigation.active_workspace_id
             WHERE window_navigation.window_id = ? AND workspaces.archived_at IS NULL",
                [&input.window_id],
                |row| row.get(0),
            )
            .optional()?;
        let default_workspace_id: Option<String> = tx.query_row(
            "SELECT id FROM workspaces WHERE root_directory = ? AND archived_at IS NULL LIMIT 1",
            [&input.default_root_directory],
            |row| row.get(0),
        ).optional()?;
        let default_workspace_id = match default_workspace_id {
            Some(workspace_id) => {
                tx.execute(
                    "UPDATE workspaces SET is_default = 0, updated_at = ?, version = version + 1
                     WHERE is_default = 1 AND id <> ?",
                    params![input.now, workspace_id],
                )?;
                tx.execute(
                    "UPDATE workspaces SET is_default = 1, updated_at = ?, version = version + 1 WHERE id = ?",
                    params![input.now, workspace_id],
                )?;
                workspace_id
            }
            None => {
                create_complete_hierarchy(
                    tx,
                    events,
                    CompleteHierarchy {
                        window_id: &input.window_id,
                        name: &input.default_name,
                        root: &input.default_root_directory,
                        now: input.now,
                        is_default: true,
                        is_pinned: true,
                    },
                    ids,
                    default_task_title,
                    &command.command_id,
                )?;
                tx.execute(
                    "INSERT INTO bootstrap_state (key, value_json, updated_at)
                     VALUES ('default-workspace-created', ?, ?)
                     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                    params![serde_json::to_string(&ids.workspace_id)?, input.now],
                )?;
                ids.workspace_id.clone()
            }
        };
        activate_workspace(
            tx,
            &input.window_id,
            active_before.as_deref().unwrap_or(&default_workspace_id),
            input.now,
        )?;
        Ok(serde_json::to_value(read_hierarchy(tx, &input.window_id)?)?)
    })?;
    Ok(serde_json::from_value(commit.result)?)
}

struct CompleteHierarchy<'a> {
    window_id: &'a str,
    name: &'a str,
    root: &'a str,
    now: i64,
    is_default: bool,
    is_pinned: bool,
}

fn create_complete_hierarchy(
    tx: &Transaction<'_>,
    events: &mut Vec<DomainEventInput>,
    input: CompleteHierarchy<'_>,
    ids: &HierarchyIds,
    default_task_title: &str,
    command_id: &str,
) -> Result<()> {
    // Match ECMAScript String.trim, including the zero-width no-break space.
    let name = input.name.trim_matches(|character| {
        matches!(character,
            '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}' |
            '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' |
            '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    });
    ensure!(!name.is_empty(), "Workspace name must not be empty");
    let root = input.root;
    let now = input.now;

    tx.execute(
        "INSERT INTO workspaces (
           id, name, root_directory, path_identity, task_order_json,
           created_at, updated_at, version, is_default, is_pinned, pin_sort_key, last_opened_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, 2, ?, ?, ?, ?)",
        params![
            ids.workspace_id,
            name,
            root,
            format!("path:{root}"),
            serde_json::to_string(&[&ids.task_id])?,
            now,
            now,
            input.is_default,
            input.is_pinned,
            if input.is_pinned { "a0" } else { "" },
            now,
        ],
    )?;
    tx.execute(
        "INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
         VALUES (?, ?, 'plain-directory', ?, ?)",
        params![ids.execution_context_id, ids.workspace_id, root, now],
    )?;
    tx.execute(
        "INSERT INTO tasks (
           id, workspace_id, execution_context_id, title, status, sort_key,
           created_at, updated_at, version, last_opened_at
         ) VALUES (?, ?, ?, ?, 'planned', 'a0', ?, ?, 1, ?)",
        params![
            ids.task_id,
            ids.workspace_id,
            ids.execution_context_id,
            default_task_title,
            now,
            now,
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO scenes (
           id, task_id, name, mode, root_node_id, title_pinned, sort_key,
           layout_revision, created_at, updated_at
         ) VALUES (?, ?, ?, 'tile', ?, 0, 'a0', 1, ?, ?)",
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
        "INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, 1)",
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
        "INSERT INTO session_mounts (
           id, scene_id, scene_node_id, session_id, created_at
         ) VALUES (?, ?, ?, ?, ?)",
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
         VALUES (?, ?, 0, ?)",
        params![input.window_id, ids.task_id, now],
    )?;
    activate_workspace(tx, input.window_id, &ids.workspace_id, now)?;

    let workspace = tx.query_row(
        "SELECT * FROM workspaces WHERE id = ?",
        [&ids.workspace_id],
        Workspace::from_row,
    )?;
    let task = tx.query_row(
        "SELECT * FROM tasks WHERE id = ?",
        [&ids.task_id],
        Task::from_row,
    )?;
    let scene = tx.query_row(
        "SELECT * FROM scenes WHERE id = ?",
        [&ids.scene_id],
        Scene::from_row,
    )?;
    let session = tx.query_row(
        "SELECT * FROM sessions WHERE id = ?",
        [&ids.session_id],
        Session::from_row,
    )?;
    let mount = tx.query_row(
        "SELECT * FROM session_mounts WHERE id = ?",
        [&ids.mount_id],
        Mount::from_row,
    )?;
    let creations = [
        (
            &workspace.id,
            "workspace.created",
            "workspace",
            &workspace.id,
            None,
            None,
            serde_json::to_value(&workspace)?,
        ),
        (
            &task.id,
            "task.created",
            "task",
            &task.id,
            Some(&task.id),
            None,
            serde_json::to_value(&task)?,
        ),
        (
            &scene.id,
            "scene.created",
            "scene",
            &scene.id,
            Some(&task.id),
            None,
            serde_json::to_value(&scene)?,
        ),
        (
            &session.id,
            "session.created",
            "session",
            &session.id,
            Some(&task.id),
            Some(&session.id),
            serde_json::to_value(&session)?,
        ),
        (
            &mount.id,
            "scene.session-mounted",
            "scene",
            &scene.id,
            Some(&task.id),
            Some(&session.id),
            serde_json::to_value(&mount)?,
        ),
    ];
    for (id, event_type, aggregate_type, aggregate_id, task_id, session_id, payload) in creations {
        events.push(DomainEventInput {
            event_id: format!("{command_id}:{id}:created"),
            event_type: event_type.into(),
            aggregate_type: aggregate_type.into(),
            aggregate_id: aggregate_id.clone(),
            workspace_id: Some(workspace.id.clone()),
            task_id: task_id.cloned(),
            session_id: session_id.cloned(),
            payload,
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: now,
        });
    }
    Ok(())
}
