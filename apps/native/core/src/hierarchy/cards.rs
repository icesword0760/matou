use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    HierarchyIds,
    navigation::{
        activate_scene_in_transaction, activate_session_in_transaction,
        activate_task_in_transaction, preferred_scene, preferred_session, preferred_task,
        read_hierarchy, register_window,
    },
    tasks::{
        NewTask, insert_task_hierarchy, require_workspace, update_workspace_order,
        workspace_path_is_available,
    },
};
use crate::{
    session_graph::project_scene_graph,
    sessions::{get_run, interrupt_run_in_transaction, require_no_active_run},
    storage::{CommandMetadata, DomainEventInput, execute},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseSessionCardInput {
    pub window_id: String,
    pub scene_id: String,
    pub session_id: String,
    pub confirmed_intent: Option<String>,
    pub preserve_scene_on_last_session: bool,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoveScope {
    #[default]
    NodeOnly,
    NodeAndDescendants,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveSessionBranchInput {
    pub window_id: String,
    pub scene_id: String,
    pub session_id: String,
    pub scope: RemoveScope,
    pub now: i64,
}

/// The shortcut displays a notice for the workspace's final live card. The
/// hierarchy close workflow also serves callers that explicitly end that card.
pub fn close_shortcut_is_last(db: &Connection, scene_id: &str, session_id: &str) -> Result<bool> {
    let (_, workspace_id, _) = active_session(db, session_id)?;
    let is_last = workspace_session_count(db, &workspace_id)? == 1;
    if is_last {
        close_mount(db, scene_id, session_id)?;
    }
    Ok(is_last)
}

/// Close the mounted session. Only the runtime owner can supply the ID of a
/// run it can retire after commit. Every other active run blocks archival.
pub fn close_card(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &CloseSessionCardInput,
    replacement_ids: &HierarchyIds,
    default_task_title: &str,
    owned_run_id: Option<&str>,
    committed_at: i64,
) -> Result<Value> {
    let result = execute(db, command, committed_at, |tx, events| {
        let CloseSessionCardInput {
            window_id,
            scene_id,
            session_id,
            confirmed_intent,
            preserve_scene_on_last_session,
            now,
        } = input;
        let now = *now;
        register_window(tx, window_id, now)?;
        let (task_id, workspace_id, work_status) = active_session(tx, session_id)?;
        let child_exists = tx
            .prepare(
                "SELECT 1 FROM session_relations_current WHERE to_session_id = ?
               AND relation_kind IN ('derived-from', 'forked-from')",
            )?
            .exists([session_id])?;
        let requires_confirmation = workspace_session_count(tx, &workspace_id)? == 1
            || child_exists
            || matches!(work_status.as_str(), "running" | "needs-input");
        ensure!(
            !requires_confirmation
                || confirmed_intent.as_deref()
                    == Some(format!("delete-session:{session_id}").as_str()),
            "Session deletion intent is stale"
        );
        let (mount_id, detached_window) = close_mount(tx, scene_id, session_id)?;
        let scene_session_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM session_mounts AS mount
             JOIN sessions ON sessions.id = mount.session_id
             WHERE mount.scene_id = ? AND sessions.archived_at IS NULL",
            [scene_id],
            |row| row.get(0),
        )?;

        if let Some(run_id) = owned_run_id
            && let Some(run) = get_run(tx, run_id)?
            && run.session_id == *session_id
            && matches!(run.status.as_str(), "starting" | "running")
        {
            let (_, mut event) = interrupt_run_in_transaction(tx, command, run_id, now)?;
            event.event_id = format!("{}:run-interrupted", command.command_id);
            events.push(event);
        }
        require_no_active_run(tx, session_id)?;
        tx.execute(
            "UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
             WHERE id = ?",
            params![now, now, session_id],
        )?;
        events.push(DomainEventInput {
            event_id: format!("{}:session-archived", command.command_id),
            event_type: "session.archived".into(),
            aggregate_type: "session".into(),
            aggregate_id: session_id.clone(),
            workspace_id: Some(workspace_id.clone()),
            task_id: Some(task_id.clone()),
            session_id: Some(session_id.clone()),
            payload: json!({"archivedAt": now}),
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: now,
        });
        if let Some(detached) = detached_window {
            tx.execute(
                "UPDATE session_mounts SET scene_window_id = NULL WHERE id = ?",
                [&mount_id],
            )?;
            tx.execute(
                "UPDATE scene_windows SET state = 'closed', updated_at = ? WHERE id = ?",
                params![now, detached],
            )?;
            tx.execute(
                "UPDATE app_windows SET state = 'closed', updated_at = ? WHERE id = ?",
                params![now, detached],
            )?;
        }

        let rebuild_canvas = scene_session_count <= 1 && !preserve_scene_on_last_session;
        let mut outcome = "scene-remains";
        if scene_session_count > 1 {
            tx.execute("DELETE FROM session_mounts WHERE id = ?", [&mount_id])?;
            if let Some(sibling) = preferred_session(tx, window_id, scene_id)? {
                activate_session_in_transaction(tx, window_id, &sibling, now)?;
            }
        } else if *preserve_scene_on_last_session {
            outcome = "session-stopped-remains";
        } else {
            outcome = "scene-archives";
            tx.execute(
                "UPDATE scenes SET archived_at = ?, updated_at = ? WHERE id = ?",
                params![now, now, scene_id],
            )?;
            tx.execute(
                "DELETE FROM window_scene_focus WHERE scene_id = ?",
                [scene_id],
            )?;
            if let Some(scene) = preferred_scene(tx, window_id, &task_id)? {
                activate_scene_in_transaction(tx, window_id, &scene, now)?;
            } else {
                tx.execute(
                    "UPDATE tasks SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
                     WHERE id = ?",
                    params![now, now, task_id],
                )?;
                tx.execute(
                    "DELETE FROM window_task_focus WHERE task_id = ?",
                    [&task_id],
                )?;
                tx.execute(
                    "DELETE FROM window_task_placements WHERE task_id = ?",
                    [&task_id],
                )?;
                let mut workspace = require_workspace(tx, &workspace_id)?;
                workspace.task_order.retain(|id| id != &task_id);
                update_workspace_order(tx, &workspace_id, &workspace.task_order, now)?;
                if let Some(task) = preferred_task(tx, window_id, &workspace_id)? {
                    activate_task_in_transaction(tx, window_id, &task, now)?;
                } else if workspace_path_is_available(tx, &workspace_id)? {
                    insert_task_hierarchy(
                        tx,
                        events,
                        NewTask {
                            command_id: &command.command_id,
                            window_id,
                            workspace: &workspace,
                            ids: replacement_ids,
                            title: default_task_title,
                            now,
                            last_opened_at: now,
                        },
                    )?;
                    outcome = "default-task-created";
                }
            }
        }
        let mut result = serde_json::to_value(read_hierarchy(tx, window_id)?)?;
        result["outcome"] = json!(outcome);
        result["disposedSessionIds"] = json!([session_id]);
        result["rebuildCanvas"] = json!(rebuild_canvas);
        Ok(result)
    })?;
    Ok(result.result)
}

fn active_session(db: &Connection, session_id: &str) -> Result<(String, String, String)> {
    let (task_id, workspace_id, work_status): (String, Option<String>, String) = db
        .query_row(
            "SELECT sessions.task_id, tasks.workspace_id, sessions.work_status
             FROM sessions LEFT JOIN tasks ON tasks.id = sessions.task_id AND tasks.archived_at IS NULL
             WHERE sessions.id = ? AND sessions.archived_at IS NULL",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .context("Session does not exist")?;
    Ok((
        task_id,
        workspace_id.context("Task does not exist")?,
        work_status,
    ))
}

/// Reads the same workspace-wide count used by the final-card command guard.
pub fn scene_workspace_session_count(db: &Connection, scene_id: &str) -> Result<i64> {
    let workspace_id: String = db.query_row(
        "SELECT tasks.workspace_id FROM scenes JOIN tasks ON tasks.id = scenes.task_id
         WHERE scenes.id = ?",
        [scene_id],
        |row| row.get(0),
    )?;
    workspace_session_count(db, &workspace_id)
}

fn workspace_session_count(db: &Connection, workspace_id: &str) -> Result<i64> {
    Ok(db.query_row(
        "SELECT COUNT(*) FROM sessions JOIN tasks ON tasks.id = sessions.task_id
         WHERE tasks.workspace_id = ? AND sessions.archived_at IS NULL AND tasks.archived_at IS NULL",
        [workspace_id], |row| row.get(0),
    )?)
}

fn close_mount(
    db: &Connection,
    scene_id: &str,
    session_id: &str,
) -> Result<(String, Option<String>)> {
    let (mount, mounted_scene, window): (String, String, Option<String>) = db
        .query_row(
            "SELECT id, scene_id, scene_window_id FROM session_mounts
         WHERE session_id = ? ORDER BY created_at LIMIT 1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .context("SessionMount does not exist")?;
    ensure!(
        mounted_scene == scene_id,
        "Session is not mounted in this Scene"
    );
    ensure!(
        db.prepare("SELECT 1 FROM scenes WHERE id = ? AND archived_at IS NULL")?
            .exists([scene_id])?,
        "Scene does not exist"
    );
    Ok((mount, window))
}

pub fn remove_branch(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &RemoveSessionBranchInput,
    committed_at: i64,
) -> Result<Value> {
    let result = execute(db, command, committed_at, |tx, events| {
        let RemoveSessionBranchInput {
            window_id,
            scene_id,
            session_id,
            scope,
            now,
        } = input;
        let now = *now;
        register_window(tx, window_id, now)?;
        let (task_id, workspace_id): (String, String) = tx
            .query_row(
                "SELECT sessions.task_id, tasks.workspace_id FROM sessions
             JOIN tasks ON tasks.id = sessions.task_id
             JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
             WHERE sessions.id = ? AND membership.scene_id = ?",
                params![session_id, scene_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("Session does not exist in this Scene")?;

        let removed = match scope {
            RemoveScope::NodeOnly => vec![session_id.clone()],
            RemoveScope::NodeAndDescendants => tx.prepare(
                "WITH RECURSIVE branch(session_id) AS (
                   SELECT ?1
                   UNION
                   SELECT relation.from_session_id FROM session_relations_current AS relation
                   JOIN branch ON relation.to_session_id = branch.session_id
                   JOIN session_canvas_memberships AS membership
                     ON membership.session_id = relation.from_session_id AND membership.scene_id = ?2
                   WHERE relation.relation_kind IN ('derived-from', 'forked-from')
                 )
                 SELECT session_id FROM branch",
            )?.query_map(params![session_id, scene_id], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        let removed_json = serde_json::to_string(&removed)?;
        ensure!(
            tx.prepare(
                "SELECT 1 FROM session_canvas_memberships
                 WHERE scene_id = ? AND session_id NOT IN (SELECT value FROM json_each(?))",
            )?
            .exists(params![scene_id, removed_json])?,
            "Scene must keep one Session"
        );

        let parent: Option<String> = if *scope == RemoveScope::NodeOnly {
            tx.query_row(
                "SELECT relation.to_session_id FROM session_relations_current AS relation
                 JOIN session_canvas_memberships AS membership
                   ON membership.session_id = relation.to_session_id AND membership.scene_id = ?
                 WHERE relation.from_session_id = ? AND relation.relation_kind IN ('derived-from', 'forked-from')
                 ORDER BY relation.updated_at DESC, relation.relation_id LIMIT 1",
                params![scene_id, session_id], |row| row.get(0),
            ).optional()?
        } else {
            None
        };
        let children = if parent.is_some() {
            tx.prepare(
                "SELECT relation.relation_id, relation.task_id, relation.from_session_id,
                        relation.to_session_id, relation.relation_kind, relation.metadata_json
                 FROM session_relations_current AS relation
                 JOIN session_canvas_memberships AS membership
                   ON membership.session_id = relation.from_session_id AND membership.scene_id = ?
                 WHERE relation.to_session_id = ? AND relation.relation_kind IN ('derived-from', 'forked-from')
                 ORDER BY membership.sibling_created_seq, relation.from_session_id",
            )?.query_map(params![scene_id, session_id], Relation::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let removes_focus = tx
            .prepare(
                "SELECT 1 FROM window_scene_focus WHERE window_id = ? AND scene_id = ?
               AND active_session_id IN (SELECT value FROM json_each(?))",
            )?
            .exists(params![window_id, scene_id, removed_json])?;
        let replacement = if removes_focus {
            replacement_session(tx, input, &removed_json)?
        } else {
            None
        };
        let disposed = tx
            .prepare(
                "SELECT id FROM sessions WHERE id IN (SELECT value FROM json_each(?))
               AND archived_at IS NULL ORDER BY created_at, id",
            )?
            .query_map([&removed_json], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        tx.execute(
            "UPDATE sessions SET status = 'archived', archived_at = COALESCE(archived_at, ?),
               updated_at = ?, version = version + 1 WHERE id IN (SELECT value FROM json_each(?))",
            params![now, now, removed_json],
        )?;
        tx.execute(
            "DELETE FROM session_mounts WHERE session_id IN (SELECT value FROM json_each(?))",
            [&removed_json],
        )?;
        let revoked = tx.prepare(
            "SELECT relation_id, task_id, from_session_id, to_session_id, relation_kind, metadata_json
             FROM session_relations_current WHERE from_session_id IN (SELECT value FROM json_each(?1))
               OR to_session_id IN (SELECT value FROM json_each(?1)) ORDER BY relation_id",
        )?.query_map([&removed_json], Relation::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for relation in revoked {
            relation.record(tx, command, "revoked", &relation.id, &relation.to, now)?;
            tx.execute(
                "DELETE FROM session_relations_current WHERE relation_id = ?",
                [&relation.id],
            )?;
        }
        if let Some(parent) = parent {
            for child in children {
                let relation_id = Uuid::new_v4().to_string();
                let sequence = child.record(tx, command, "created", &relation_id, &parent, now)?;
                tx.execute(
                    "INSERT INTO session_relations_current (
                       relation_id, task_id, from_session_id, to_session_id, relation_kind,
                       metadata_json, created_at, updated_at, source_event_sequence
                     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        relation_id,
                        child.task,
                        child.from,
                        parent,
                        child.kind,
                        child.metadata,
                        now,
                        now,
                        sequence
                    ],
                )?;
            }
        }
        tx.execute(
            "DELETE FROM session_canvas_memberships WHERE session_id IN (SELECT value FROM json_each(?))",
            [&removed_json],
        )?;
        if let Some(replacement) = replacement {
            activate_session_in_transaction(tx, window_id, &replacement, now)?;
        } else if removes_focus {
            tx.execute(
                "UPDATE window_scene_focus SET active_session_id = NULL, updated_at = ?
                 WHERE window_id = ? AND scene_id = ?",
                params![now, window_id, scene_id],
            )?;
        }
        let result = json!({
            "graph": project_scene_graph(tx, scene_id, Some(window_id))?,
            "removedSessionIds": removed,
            "disposedSessionIds": disposed,
        });
        events.push(DomainEventInput {
            event_id: format!("{}:session-branch-removed", command.command_id),
            event_type: "session.graph-summary-changed".into(),
            aggregate_type: "scene".into(),
            aggregate_id: scene_id.clone(),
            workspace_id: Some(workspace_id),
            task_id: Some(task_id),
            session_id: Some(session_id.clone()),
            payload: result.clone(),
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: now,
        });
        Ok(result)
    })?;
    Ok(result.result)
}

fn replacement_session(
    db: &Connection,
    input: &RemoveSessionBranchInput,
    removed_json: &str,
) -> Result<Option<String>> {
    let scene_id = &input.scene_id;
    let session_id = &input.session_id;
    let parent = db.query_row(
        "SELECT parent.id FROM session_relations_current AS relation
         JOIN sessions AS parent ON parent.id = relation.to_session_id
         JOIN session_canvas_memberships AS membership ON membership.session_id = parent.id
         WHERE relation.from_session_id = ? AND relation.relation_kind IN ('derived-from', 'forked-from')
           AND membership.scene_id = ? AND parent.archived_at IS NULL
           AND parent.id NOT IN (SELECT value FROM json_each(?))
         LIMIT 1",
        params![session_id, scene_id, removed_json], |row| row.get(0),
    ).optional()?;
    if parent.is_some() {
        return Ok(parent);
    }
    if input.scope == RemoveScope::NodeOnly {
        let child = db.query_row(
            "SELECT child.id FROM session_relations_current AS relation
             JOIN sessions AS child ON child.id = relation.from_session_id
             JOIN session_canvas_memberships AS membership
               ON membership.session_id = child.id AND membership.scene_id = ?
             WHERE relation.to_session_id = ? AND relation.relation_kind IN ('derived-from', 'forked-from')
               AND child.archived_at IS NULL
             ORDER BY membership.sibling_created_seq, child.id LIMIT 1",
            params![scene_id, session_id], |row| row.get(0),
        ).optional()?;
        if child.is_some() {
            return Ok(child);
        }
    }
    Ok(db.query_row(
        "SELECT sessions.id FROM session_canvas_memberships AS membership
         JOIN sessions ON sessions.id = membership.session_id
         WHERE membership.scene_id = ? AND sessions.archived_at IS NULL
           AND sessions.id NOT IN (SELECT value FROM json_each(?))
         ORDER BY membership.last_user_interaction_seq DESC, membership.sibling_created_seq, sessions.id
         LIMIT 1",
        params![scene_id, removed_json], |row| row.get(0),
    ).optional()?)
}

struct Relation {
    id: String,
    task: String,
    from: String,
    to: String,
    kind: String,
    metadata: String,
}

impl Relation {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            task: row.get(1)?,
            from: row.get(2)?,
            to: row.get(3)?,
            kind: row.get(4)?,
            metadata: row.get(5)?,
        })
    }

    fn record(
        &self,
        db: &Connection,
        command: &CommandMetadata,
        operation: &str,
        relation_id: &str,
        to: &str,
        now: i64,
    ) -> Result<i64> {
        let event = if operation == "revoked" {
            "revoked"
        } else {
            "reconnected"
        };
        Ok(db
            .prepare(
                "INSERT INTO session_relation_events (
               event_id, relation_id, operation, task_id, from_session_id, to_session_id,
               relation_kind, metadata_json, command_id, occurred_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?
            .insert(params![
                format!("{}:relation-{event}:{}", command.command_id, self.id),
                relation_id,
                operation,
                self.task,
                self.from,
                to,
                self.kind,
                self.metadata,
                command.command_id,
                now,
            ])?)
    }
}
