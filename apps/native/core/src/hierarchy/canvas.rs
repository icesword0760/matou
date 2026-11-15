use std::{collections::HashSet, error::Error, fmt};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::navigation::{
    activate_session_in_transaction, read_created_path, read_hierarchy, register_window,
    restore_navigation_in_transaction,
};
use crate::{
    session_graph::project_scene_graph,
    storage::{CommandMetadata, DomainEventInput, execute},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasIds {
    pub scene_id: String,
    pub root_node_id: String,
    pub secondary_node_id: String,
    pub session_id: String,
    pub mount_id: String,
    pub relation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCanvasInput {
    pub window_id: String,
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigation: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShellSiblingInput {
    pub window_id: String,
    pub scene_id: String,
    pub source_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigation: Option<String>,
    pub now: i64,
}

pub struct CanvasMessages<'a> {
    pub new_canvas: &'a str,
    pub duplicate_scene_name: &'a str,
    pub workspace_path_invalid: &'a str,
    pub detached_session_must_return: &'a str,
}

#[derive(Debug)]
pub struct CanvasError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for CanvasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CanvasError {}

pub fn create_canvas(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &CreateCanvasInput,
    ids: &CanvasIds,
    messages: &CanvasMessages<'_>,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let window_id = &input.window_id;
        let task_id = &input.task_id;
        let now = input.now;
        register_window(tx, window_id, now)?;
        let navigation = read_hierarchy(tx, window_id)?.navigation;
        let (workspace_id, execution_context_id) = require_task(tx, task_id)?;
        let root: String = tx
            .query_row(
                "SELECT root_directory FROM workspaces WHERE id = ? AND archived_at IS NULL",
                [&workspace_id],
                |row| row.get(0),
            )
            .optional()?
            .context("Workspace does not exist")?;
        assert_workspace_path_available(tx, &workspace_id, messages.workspace_path_invalid)?;

        let title_pinned = input.title.is_some();
        let name = match &input.title {
            Some(title) => {
                let name = required_title(title, "Canvas title")?;
                let duplicate = tx
                    .query_row(
                        "SELECT id FROM scenes
                     WHERE task_id = ? AND name = ? AND title_pinned = 1 AND archived_at IS NULL",
                        params![task_id, name],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if duplicate {
                    return Err(CanvasError {
                        code: "DUPLICATE_SCENE_NAME",
                        message: messages.duplicate_scene_name.into(),
                    }
                    .into());
                }
                name.to_owned()
            }
            None => next_canvas_name(tx, task_id, messages.new_canvas)?,
        };
        let ordinal: i64 = tx.query_row(
            "SELECT COUNT(*) FROM scenes WHERE task_id = ? AND archived_at IS NULL",
            [task_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO scenes (
               id, task_id, name, mode, root_node_id, title_pinned, sort_key,
               layout_revision, created_at, updated_at
             ) VALUES (?, ?, ?, 'tile', ?, ?, ?, 1, ?, ?)",
            params![
                ids.scene_id,
                task_id,
                name,
                ids.root_node_id,
                title_pinned,
                format!("a{ordinal:08}"),
                now,
                now
            ],
        )?;
        tx.execute(
            "INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at) VALUES (?, ?, 'root', 0, ?)",
            params![ids.root_node_id, ids.scene_id, now],
        )?;
        tx.execute(
            "INSERT INTO sessions (
               id, task_id, execution_context_id, kind, status, title, cwd,
               created_at, updated_at, last_activity_at, version
             ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, ?, 1)",
            params![
                ids.session_id,
                task_id,
                execution_context_id,
                root,
                now,
                now,
                now
            ],
        )?;
        mount_session(tx, ids, &ids.scene_id, now)?;
        activate_session_in_transaction(tx, window_id, &ids.session_id, now)?;
        let created = read_created_path(tx, &ids.session_id)?;
        let membership = read_membership(tx, &ids.session_id)?;
        if input.navigation.as_deref() == Some("preserve") {
            restore_navigation_in_transaction(tx, &navigation, now)?;
        }

        let hierarchy = read_hierarchy(tx, window_id)?;
        let graph = project_scene_graph(tx, &ids.scene_id, Some(window_id))?;
        let context = CreationEvents {
            command_id: &command.command_id,
            workspace_id: &workspace_id,
            task_id,
            scene_id: &ids.scene_id,
            session_id: &ids.session_id,
            now,
        };
        events.push(context.event(
            "scene-created",
            "scene.created",
            "scene",
            &ids.scene_id,
            None,
            created["scene"].clone(),
        ));
        context.emit_session_creation(events, &created, membership);
        events.push(context.event(
            "canvas-created",
            "scene.canvas-created",
            "scene",
            &ids.scene_id,
            Some(&ids.session_id),
            json!({"graph": graph}),
        ));
        let mut result = serde_json::to_value(hierarchy)?;
        result["created"] = created;
        result["graph"] = graph;
        Ok(result)
    })?;
    Ok(commit.result)
}

pub fn create_shell_sibling(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &CreateShellSiblingInput,
    ids: &CanvasIds,
    messages: &CanvasMessages<'_>,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let window_id = &input.window_id;
        let scene_id = &input.scene_id;
        let now = input.now;
        register_window(tx, window_id, now)?;
        let navigation = read_hierarchy(tx, window_id)?.navigation;
        let task_id: String = tx
            .query_row(
                "SELECT task_id FROM scenes WHERE id = ? AND archived_at IS NULL",
                [scene_id],
                |row| row.get(0),
            )
            .optional()?
            .context("Scene does not exist")?;
        let (workspace_id, _) = require_task(tx, &task_id)?;
        assert_workspace_path_available(tx, &workspace_id, messages.workspace_path_invalid)?;
        let source = tx
            .query_row(
                "SELECT sessions.execution_context_id, sessions.cwd
             FROM sessions
             JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
             WHERE sessions.id = ? AND membership.scene_id = ?
               AND (sessions.archived_at IS NULL OR ? IS NOT NULL)",
                params![input.source_session_id, scene_id, input.parent_session_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .context("Session does not exist")?;
        let (execution_context_id, cwd) = match input
            .execution_context_id
            .as_deref()
            .filter(|id| !id.is_empty())
        {
            Some(id) => tx
                .query_row(
                    "SELECT id, cwd FROM execution_contexts
                 WHERE id = ? AND workspace_id = ? AND archived_at IS NULL",
                    params![id, workspace_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .context("ExecutionContext does not exist")?,
            None => source,
        };
        let (source_node, source_window): (Option<String>, Option<String>) = tx
            .query_row(
                "SELECT mounts.scene_node_id, mounts.scene_window_id
             FROM session_mounts AS mounts
             JOIN sessions ON sessions.id = mounts.session_id
             WHERE mounts.scene_id = ? AND mounts.scene_node_id IS NOT NULL
               AND mounts.scene_window_id IS NULL AND sessions.archived_at IS NULL
             ORDER BY
               CASE WHEN mounts.session_id = ? THEN 0
                    WHEN mounts.session_id = ? THEN 1 ELSE 2 END,
               sessions.last_activity_at DESC, mounts.created_at, mounts.id LIMIT 1",
                params![scene_id, input.source_session_id, input.parent_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("SessionMount does not exist")?;
        let source_node = match (source_node, source_window) {
            (Some(node), None) => node,
            _ => bail!("{}", messages.detached_session_must_return),
        };
        let (parent_node, ordinal): (Option<String>, i64) = tx
            .query_row(
                "SELECT parent_node_id, ordinal FROM scene_nodes WHERE id = ?",
                [&source_node],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("SceneNode does not exist")?;
        let structural_parent: Option<String> =
            match input.parent_session_id.as_ref().filter(|id| !id.is_empty()) {
                Some(id) => Some(id.clone()),
                None => tx
                    .query_row(
                        "SELECT to_session_id FROM session_relations_current
                 WHERE from_session_id = ? AND relation_kind IN ('derived-from', 'forked-from')",
                        [&input.source_session_id],
                        |row| row.get(0),
                    )
                    .optional()?,
            };
        let (title, title_source) = match &input.title {
            Some(title) => (required_title(title, "Session title")?, "manual"),
            None => ("Shell", "default"),
        };

        tx.execute(
            "INSERT INTO scene_nodes (
               id, scene_id, parent_node_id, kind, direction, ordinal, created_at
             ) VALUES (?, ?, ?, 'split', 'horizontal', ?, ?)",
            params![ids.secondary_node_id, scene_id, parent_node, ordinal, now],
        )?;
        tx.execute(
            "UPDATE scene_nodes SET parent_node_id = ?, kind = 'mount', direction = NULL, ordinal = 0
             WHERE id = ?",
            params![ids.secondary_node_id, source_node],
        )?;
        tx.execute(
            "INSERT INTO scene_nodes (id, scene_id, parent_node_id, kind, ordinal, created_at)
             VALUES (?, ?, ?, 'mount', 1, ?)",
            params![ids.root_node_id, scene_id, ids.secondary_node_id, now],
        )?;
        tx.execute(
            "INSERT INTO sessions (
               id, task_id, execution_context_id, kind, status, title, title_source, cwd,
               created_at, updated_at, last_activity_at, version
             ) VALUES (?, ?, ?, 'shell', 'created', ?, ?, ?, ?, ?, ?, 1)",
            params![
                ids.session_id,
                task_id,
                execution_context_id,
                title,
                title_source,
                cwd,
                now,
                now,
                now
            ],
        )?;
        mount_session(tx, ids, scene_id, now)?;
        tx.execute(
            "UPDATE scenes
             SET root_node_id = CASE WHEN root_node_id = ? THEN ? ELSE root_node_id END,
                 layout_revision = layout_revision + 1, updated_at = ?
             WHERE id = ?",
            params![source_node, ids.secondary_node_id, now, scene_id],
        )?;
        if let Some(parent) = &structural_parent {
            let sequence = tx
                .prepare(
                    "INSERT INTO session_relation_events (
                   event_id, relation_id, operation, task_id, from_session_id, to_session_id,
                   relation_kind, metadata_json, command_id, occurred_at
                 ) VALUES (?, ?, 'created', ?, ?, ?, 'derived-from', '{}', ?, ?)",
                )?
                .insert(params![
                    format!("{}:derived-relation-created", command.command_id),
                    ids.relation_id,
                    task_id,
                    ids.session_id,
                    parent,
                    command.command_id,
                    now
                ])?;
            tx.execute(
                "INSERT INTO session_relations_current (
                   relation_id, task_id, from_session_id, to_session_id, relation_kind,
                   metadata_json, created_at, updated_at, source_event_sequence
                 ) VALUES (?, ?, ?, ?, 'derived-from', '{}', ?, ?, ?)",
                params![
                    ids.relation_id,
                    task_id,
                    ids.session_id,
                    parent,
                    now,
                    now,
                    sequence
                ],
            )?;
        }
        activate_session_in_transaction(tx, window_id, &ids.session_id, now)?;
        let created = read_created_path(tx, &ids.session_id)?;
        let membership = read_membership(tx, &ids.session_id)?;
        if input.navigation.as_deref() == Some("preserve") {
            restore_navigation_in_transaction(tx, &navigation, now)?;
        }

        let hierarchy = read_hierarchy(tx, window_id)?;
        let graph = project_scene_graph(tx, scene_id, Some(window_id))?;
        let context = CreationEvents {
            command_id: &command.command_id,
            workspace_id: &workspace_id,
            task_id: &task_id,
            scene_id,
            session_id: &ids.session_id,
            now,
        };
        context.emit_session_creation(events, &created, membership);
        if let Some(parent) = structural_parent {
            events.push(context.event(
                "domain-relation-created",
                "session-relation.created",
                "session-relation",
                &ids.relation_id,
                Some(&ids.session_id),
                json!({
                    "id": ids.relation_id,
                    "taskId": task_id,
                    "fromSessionId": ids.session_id,
                    "toSessionId": parent,
                    "kind": "derived-from",
                    "metadata": {},
                    "createdAt": now,
                    "updatedAt": now,
                }),
            ));
        }
        events.push(context.event(
            "shell-sibling-created",
            "session.graph-summary-changed",
            "scene",
            scene_id,
            Some(&ids.session_id),
            json!({"graph": graph, "sourceSessionId": input.source_session_id}),
        ));
        let mut result = serde_json::to_value(hierarchy)?;
        result["created"] = created;
        result["graph"] = graph;
        Ok(result)
    })?;
    Ok(commit.result)
}

fn require_task(connection: &Connection, task_id: &str) -> Result<(String, String)> {
    connection.query_row(
        "SELECT workspace_id, execution_context_id FROM tasks WHERE id = ? AND archived_at IS NULL",
        [task_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?.context("Task does not exist")
}

fn assert_workspace_path_available(
    connection: &Connection,
    workspace_id: &str,
    message: &str,
) -> Result<()> {
    let status: Option<String> = connection
        .query_row(
            "SELECT status FROM workspace_path_state WHERE workspace_id = ?",
            [workspace_id],
            |row| row.get(0),
        )
        .optional()?;
    if status.as_deref() == Some("invalid") {
        return Err(CanvasError {
            code: "WORKSPACE_PATH_INVALID",
            message: message.into(),
        }
        .into());
    }
    Ok(())
}

fn mount_session(connection: &Connection, ids: &CanvasIds, scene_id: &str, now: i64) -> Result<()> {
    connection.execute(
        "INSERT INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at)
         VALUES (?, ?, ?, ?, ?)",
        params![
            ids.mount_id,
            scene_id,
            ids.root_node_id,
            ids.session_id,
            now
        ],
    )?;
    Ok(())
}

fn next_canvas_name(connection: &Connection, task_id: &str, base_name: &str) -> Result<String> {
    let mut query =
        connection.prepare("SELECT name FROM scenes WHERE task_id = ? AND archived_at IS NULL")?;
    let names = query
        .query_map([task_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    if !names.contains(base_name) {
        return Ok(base_name.to_owned());
    }
    for suffix in 2.. {
        let candidate = format!("{base_name} {suffix}");
        if !names.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!()
}

fn required_title<'a>(title: &'a str, label: &str) -> Result<&'a str> {
    // ECMAScript String.trim includes U+FEFF and does not remove U+0085.
    let title = title.trim_matches(|character| {
        matches!(character,
            '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}'
            | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
            | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    });
    ensure!(!title.is_empty(), "{label} must not be empty");
    Ok(title)
}

fn read_membership(connection: &Connection, id: &str) -> Result<Value> {
    connection.query_row(
        "SELECT session_id, scene_id, sibling_created_seq, last_user_interaction_seq, created_at, updated_at
         FROM session_canvas_memberships WHERE session_id = ?",
        [id],
        |row| Ok(json!({
            "sessionId": row.get::<_, String>(0)?,
            "sceneId": row.get::<_, String>(1)?,
            "siblingCreatedSeq": row.get::<_, i64>(2)?,
            "lastUserInteractionSeq": row.get::<_, i64>(3)?,
            "createdAt": row.get::<_, i64>(4)?,
            "updatedAt": row.get::<_, i64>(5)?,
        })),
    ).optional()?.context("SessionCanvasMembership does not exist")
}

struct CreationEvents<'a> {
    command_id: &'a str,
    workspace_id: &'a str,
    task_id: &'a str,
    scene_id: &'a str,
    session_id: &'a str,
    now: i64,
}

impl CreationEvents<'_> {
    fn event(
        &self,
        suffix: &str,
        event_type: &str,
        aggregate_type: &str,
        aggregate_id: &str,
        session_id: Option<&str>,
        payload: Value,
    ) -> DomainEventInput {
        DomainEventInput {
            event_id: format!("{}:{suffix}", self.command_id),
            event_type: event_type.to_owned(),
            aggregate_type: aggregate_type.to_owned(),
            aggregate_id: aggregate_id.to_owned(),
            workspace_id: Some(self.workspace_id.to_owned()),
            task_id: Some(self.task_id.to_owned()),
            session_id: session_id.map(str::to_owned),
            payload,
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: self.now,
        }
    }

    fn emit_session_creation(
        &self,
        events: &mut Vec<DomainEventInput>,
        created: &Value,
        membership: Value,
    ) {
        let session = Some(self.session_id);
        events.push(self.event(
            "session-created",
            "session.created",
            "session",
            self.session_id,
            session,
            created["session"].clone(),
        ));
        events.push(self.event(
            "session-mounted",
            "scene.session-mounted",
            "scene",
            self.scene_id,
            session,
            created["mount"].clone(),
        ));
        events.push(self.event(
            "canvas-membership-created",
            "session.canvas-membership-created",
            "session",
            self.session_id,
            session,
            json!({"membership": membership}),
        ));
    }
}
