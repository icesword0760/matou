use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::storage::{CommandMetadata, DomainCommit, DomainEventInput, execute};

const ENVIRONMENT_PROJECTION: &str = r#"environment.active_target AS environment_active_target,
         environment.state AS environment_state,
         environment.error_message AS environment_error,
         environment.local_execution_context_id,
         local_environment.cwd AS local_environment_path,
         environment.managed_worktree_id,
         managed_worktree.execution_context_id AS managed_worktree_execution_context_id,
         managed_worktree.worktree_path AS managed_worktree_path,
         managed_worktree.branch_name AS managed_worktree_branch,
         managed_worktree.state AS managed_worktree_state,"#;

const NO_ENVIRONMENT_PROJECTION: &str = r#"NULL AS environment_active_target,
         NULL AS environment_state,
         NULL AS environment_error,
         NULL AS local_execution_context_id,
         NULL AS local_environment_path,
         NULL AS managed_worktree_id,
         NULL AS managed_worktree_execution_context_id,
         NULL AS managed_worktree_path,
         NULL AS managed_worktree_branch,
         NULL AS managed_worktree_state,"#;

const ENVIRONMENT_JOINS: &str = r#"LEFT JOIN session_environment_bindings AS environment
         ON environment.session_id = sessions.id
       LEFT JOIN execution_contexts AS local_environment
         ON local_environment.id = environment.local_execution_context_id
       LEFT JOIN worktrees AS managed_worktree
         ON managed_worktree.id = environment.managed_worktree_id"#;

const GIT_PROJECTION: &str = r#"context_git.state AS context_git_state,
         context_git.repository_root AS context_git_repository_root,
         context_git.branch AS context_git_branch,
         context_git.detached_head AS context_git_detached_head,
         context_git.dirty AS context_git_dirty,"#;

const NO_GIT_PROJECTION: &str = r#"NULL AS context_git_state,
         NULL AS context_git_repository_root,
         NULL AS context_git_branch,
         NULL AS context_git_detached_head,
         NULL AS context_git_dirty,"#;

const GIT_JOIN: &str = r#"LEFT JOIN execution_context_git_states AS context_git
         ON context_git.execution_context_id = sessions.execution_context_id"#;

const FORK_PROGRESS_PROJECTION: &str = r#"fork.operation_id AS fork_operation_id,
         fork.submission_key AS fork_submission_key,
         fork.stage AS fork_stage,
         fork.completed_steps AS fork_completed_steps,
         fork.total_steps AS fork_total_steps,
         fork.attempt AS fork_operation_attempt,"#;

const NO_FORK_PROGRESS_PROJECTION: &str = r#"NULL AS fork_operation_id,
         NULL AS fork_submission_key,
         NULL AS fork_stage,
         NULL AS fork_completed_steps,
         NULL AS fork_total_steps,
         NULL AS fork_operation_attempt,"#;

const CHILD_COUNTS_SQL: &str = r#"SELECT
           relation.to_session_id AS parent_session_id,
           SUM(CASE WHEN child.archived_at IS NULL THEN 1 ELSE 0 END) AS active_count,
           SUM(CASE WHEN child.archived_at IS NOT NULL THEN 1 ELSE 0 END) AS stopped_count,
           SUM(CASE WHEN child.archived_at IS NULL AND child.kind = 'shell' THEN 1 ELSE 0 END) AS shell_count,
           SUM(CASE WHEN child.archived_at IS NULL
                     AND child.kind IN ('claude-code', 'agent-team-member')
                    THEN 1 ELSE 0 END) AS claude_count
         FROM session_relations_current AS relation
         JOIN sessions AS child ON child.id = relation.from_session_id
         JOIN session_canvas_memberships AS membership ON membership.session_id = child.id
         WHERE relation.relation_kind IN ('derived-from', 'forked-from')
           AND membership.scene_id = ?
         GROUP BY relation.to_session_id"#;

const DIRECTORY_COUNTS_SQL: &str = r#"SELECT sessions.cwd, COUNT(*) AS count
         FROM sessions
         JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
         WHERE membership.scene_id = ? AND sessions.archived_at IS NULL
         GROUP BY sessions.cwd"#;

const EDGES_SQL: &str = r#"SELECT
         relation.to_session_id AS parent_session_id,
         relation.from_session_id AS child_session_id,
         relation.relation_kind,
         relation.created_at
       FROM session_relations_current AS relation
       JOIN session_canvas_memberships AS membership
         ON membership.session_id = relation.from_session_id
       WHERE membership.scene_id = ?
         AND relation.relation_kind IN ('derived-from', 'forked-from')
       ORDER BY relation.created_at, relation.relation_id"#;

fn node_query(environment: bool, git: bool, fork: bool) -> String {
    let environment_projection = if environment {
        ENVIRONMENT_PROJECTION
    } else {
        NO_ENVIRONMENT_PROJECTION
    };
    let environment_joins = if environment { ENVIRONMENT_JOINS } else { "" };
    let git_projection = if git {
        GIT_PROJECTION
    } else {
        NO_GIT_PROJECTION
    };
    let git_join = if git { GIT_JOIN } else { "" };
    let fork_progress = if fork {
        FORK_PROGRESS_PROJECTION
    } else {
        NO_FORK_PROGRESS_PROJECTION
    };
    format!(
        r#"SELECT
         membership.*,
         sessions.task_id,
         sessions.execution_context_id,
         sessions.kind,
         sessions.status,
         sessions.work_status,
         sessions.title,
         sessions.cwd,
         sessions.created_at AS session_created_at,
         sessions.updated_at AS session_updated_at,
         sessions.last_activity_at,
         sessions.archived_at,
         structural.to_session_id AS parent_session_id,
         structural.relation_kind,
         structural.created_at AS relation_created_at,
         provider.restore_state,
         provider.restore_error,
         provider.metadata_json AS provider_metadata_json,
         fork.state AS fork_state,
         fork.error_message AS fork_error,
         fork.attempt_count AS fork_attempt,
         {fork_progress}
         worktrees.worktree_path,
         worktrees.branch_name,
         worktrees.base_revision AS actual_worktree_base_revision,
         worktrees.state AS actual_worktree_state,
         {environment_projection}
         {git_projection}
         detached.native_window_key AS detached_window_id
         , summary.latest_lines_json
       FROM session_canvas_memberships AS membership
       JOIN sessions ON sessions.id = membership.session_id
       LEFT JOIN session_relations_current AS structural
         ON structural.from_session_id = sessions.id
        AND structural.relation_kind IN ('derived-from', 'forked-from')
       LEFT JOIN provider_bindings AS provider
         ON provider.id = (
           SELECT binding.id FROM provider_bindings AS binding
           WHERE binding.session_id = sessions.id
           ORDER BY
             CASE
               WHEN binding.restore_state IN ('restoring', 'failed') THEN 0
               WHEN binding.invalidated_at IS NULL
                 AND binding.resume_state NOT IN ('failed', 'expired') THEN 1
               ELSE 2
             END,
             binding.updated_at DESC,
             binding.id DESC
           LIMIT 1
         )
       LEFT JOIN session_fork_intents AS fork ON fork.session_id = sessions.id
       LEFT JOIN worktrees ON worktrees.execution_context_id = sessions.execution_context_id
       {environment_joins}
       {git_join}
       LEFT JOIN scene_windows AS detached
         ON detached.scene_id = membership.scene_id
        AND detached.state = 'detached'
        AND EXISTS (
          SELECT 1 FROM session_mounts
          WHERE session_mounts.scene_window_id = detached.id
            AND session_mounts.session_id = sessions.id
        )
       LEFT JOIN session_graph_summaries AS summary ON summary.session_id = sessions.id
       WHERE membership.scene_id = ?
       ORDER BY
         COALESCE(structural.to_session_id, ''),
         membership.last_user_interaction_seq DESC,
         membership.sibling_created_seq ASC,
         sessions.id"#
    )
}

/// Read the current scene graph, including stopped sessions and window focus.
pub fn project_scene_graph(
    connection: &Connection,
    scene_id: &str,
    window_id: Option<&str>,
) -> Result<Value> {
    let layout_revision = connection
        .query_row(
            "SELECT layout_revision FROM scenes WHERE id = ?",
            [scene_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .with_context(|| format!("Scene {scene_id} does not exist"))?;
    let has_environment = has_table(connection, "session_environment_bindings")?;
    let has_git = has_table(connection, "execution_context_git_states")?;
    let has_fork_progress: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('session_fork_intents') WHERE name = 'operation_id')",
        [],
        |row| row.get(0),
    )?;

    let mut statement = connection.prepare(CHILD_COUNTS_SQL)?;
    let children = statement
        .query_map([scene_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ChildCount {
                    active: row.get(1)?,
                    stopped: row.get(2)?,
                    shell: row.get(3)?,
                    claude_code: row.get(4)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    let mut statement = connection.prepare(DIRECTORY_COUNTS_SQL)?;
    let sharing = statement
        .query_map([scene_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    let sql = node_query(has_environment, has_git, has_fork_progress);
    let mut statement = connection.prepare(&sql)?;
    let nodes = statement
        .query_map([scene_id], |row| map_node(row, &children, &sharing))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement = connection.prepare(EDGES_SQL)?;
    let edges = statement
        .query_map([scene_id], |row| {
            Ok(json!({
                "parentSessionId": row.get::<_, String>(0)?,
                "childSessionId": row.get::<_, String>(1)?,
                "relationKind": row.get::<_, String>(2)?,
                "createdAt": row.get::<_, i64>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut graph = json!({
        "sceneId": scene_id,
        "layoutRevision": layout_revision,
        "nodes": nodes,
        "edges": edges,
    });
    if let Some(window_id) = window_id {
        let focus = connection.query_row(
            "SELECT active_session_id FROM window_scene_focus WHERE window_id = ? AND scene_id = ?",
            params![window_id, scene_id],
            |row| row.get::<_, Option<String>>(0),
        ).optional()?.flatten();
        insert_optional(&mut graph, "focusedSessionId", focus);
    }
    Ok(graph)
}

fn has_table(connection: &Connection, table: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
        [table],
        |row| row.get(0),
    )
}

#[derive(Default, Clone, Copy)]
struct ChildCount {
    active: i64,
    stopped: i64,
    shell: i64,
    claude_code: i64,
}

pub(crate) fn binding_can_fork(metadata: &Value) -> bool {
    match metadata.get("canFork") {
        Some(value) => value == true,
        None => metadata["observedUserPrompt"] == true && metadata["observedNormalStop"] == true,
    }
}

fn map_node(
    row: &Row<'_>,
    children: &HashMap<String, ChildCount>,
    sharing: &HashMap<String, i64>,
) -> rusqlite::Result<Value> {
    let session_id: String = row.get("session_id")?;
    let kind: String = row.get("kind")?;
    let status: String = row.get("work_status")?;
    let fork_stage: Option<String> = row.get("fork_stage")?;
    let fork_state: Option<String> = row.get("fork_state")?;
    let restore_state: Option<String> = row.get("restore_state")?;
    let work_status = if fork_stage.as_deref() == Some("failed")
        || fork_state.as_deref() == Some("failed")
        || restore_state.as_deref() == Some("failed")
    {
        "error"
    } else if fork_stage
        .as_deref()
        .is_some_and(|stage| stage != "succeeded")
        || matches!(fork_state.as_deref(), Some("pending" | "starting"))
    {
        "starting"
    } else {
        &status
    };
    let provider: Option<String> = row.get("provider_metadata_json")?;
    let metadata: Option<Value> = provider
        .as_deref()
        .and_then(|serialized| serde_json::from_str(serialized).ok());
    let can_fork = matches!(kind.as_str(), "claude-code" | "antigravity")
        && restore_state.as_deref() == Some("none")
        && metadata.as_ref().is_some_and(binding_can_fork);
    let cwd: String = row.get("cwd")?;
    let shared = sharing.get(&cwd).is_some_and(|&count| count > 1);
    let counts = children.get(&session_id).copied().unwrap_or_default();

    let mut node = json!({"sessionId": session_id, "sceneId": row.get::<_, String>("scene_id")?});
    insert_optional(
        &mut node,
        "parentSessionId",
        row.get::<_, Option<String>>("parent_session_id")?,
    );
    insert_optional(
        &mut node,
        "relationKind",
        row.get::<_, Option<String>>("relation_kind")?,
    );
    node["currentMode"] = json!(kind);
    node["workStatus"] = json!(work_status);
    node["providerRestoreState"] = json!(restore_state.as_deref().unwrap_or("none"));
    insert_optional(
        &mut node,
        "providerRestoreError",
        row.get::<_, Option<String>>("restore_error")?,
    );
    map_fork(&mut node, row, &session_id, fork_stage, fork_state)?;
    insert_optional(
        &mut node,
        "providerSpawnRevision",
        metadata
            .as_ref()
            .and_then(|value| value.get("spawnRevision"))
            .and_then(safe_integer),
    );
    node["canFork"] = json!(can_fork);
    node["title"] = json!(row.get::<_, String>("title")?);
    node["cwd"] = json!(cwd);
    insert_optional(&mut node, "environment", map_environment(row)?);
    node["hasOwnedWorktree"] = json!(
        row.get::<_, Option<String>>("managed_worktree_id")?
            .is_some()
    );
    insert_optional(&mut node, "git", map_git(row)?);
    node["sharedWorkingDirectory"] = json!(shared);
    if let (Some(path), Some(branch)) = (
        row.get::<_, Option<String>>("worktree_path")?,
        row.get::<_, Option<String>>("branch_name")?,
    ) {
        node["worktree"] = json!({"branch": branch, "path": path, "shared": shared});
    }
    node["activeChildCount"] = json!(counts.active);
    node["stoppedChildCount"] = json!(counts.stopped);
    node["childModeCounts"] = json!({"shell": counts.shell, "claudeCode": counts.claude_code});
    node["latestLines"] = json!(latest_lines(
        row.get::<_, Option<String>>("latest_lines_json")?
    ));
    node["siblingCreatedSeq"] = json!(row.get::<_, i64>("sibling_created_seq")?);
    node["lastUserInteractionSeq"] = json!(row.get::<_, i64>("last_user_interaction_seq")?);
    node["lastActivityAt"] = json!(row.get::<_, i64>("last_activity_at")?);
    insert_optional(
        &mut node,
        "archivedAt",
        row.get::<_, Option<i64>>("archived_at")?,
    );
    insert_optional(
        &mut node,
        "detachedWindowId",
        row.get::<_, Option<String>>("detached_window_id")?,
    );
    Ok(node)
}

fn map_fork(
    node: &mut Value,
    row: &Row<'_>,
    session_id: &str,
    stage: Option<String>,
    state: Option<String>,
) -> rusqlite::Result<()> {
    let operation: Option<String> = row.get("fork_operation_id")?;
    let submission: Option<String> = row.get("fork_submission_key")?;
    let completed: Option<i64> = row.get("fork_completed_steps")?;
    let total: Option<i64> = row.get("fork_total_steps")?;
    let attempt: Option<i64> = row.get("fork_operation_attempt")?;
    let error: Option<String> = row.get("fork_error")?;
    if let (
        Some(operation),
        Some(submission),
        Some(stage),
        Some(completed),
        Some(total),
        Some(attempt),
    ) = (
        operation.filter(|value| !value.is_empty()),
        submission.filter(|value| !value.is_empty()),
        stage.filter(|value| !value.is_empty()),
        completed,
        total,
        attempt,
    ) {
        let mut progress = json!({
            "operationId": operation, "sessionId": session_id, "submissionKey": submission,
            "stage": stage, "completedSteps": completed, "totalSteps": total, "attempt": attempt
        });
        insert_optional(&mut progress, "error", error);
        node["forkProgress"] = progress;
    } else {
        insert_optional(node, "forkState", state);
        insert_optional(node, "forkError", error);
        insert_optional(
            node,
            "forkAttempt",
            row.get::<_, Option<i64>>("fork_attempt")?,
        );
    }
    Ok(())
}

fn map_environment(row: &Row<'_>) -> rusqlite::Result<Option<Value>> {
    let (Some(target), Some(state), Some(local_id), Some(local_path)) = (
        row.get::<_, Option<String>>("environment_active_target")?,
        row.get::<_, Option<String>>("environment_state")?,
        row.get::<_, Option<String>>("local_execution_context_id")?,
        row.get::<_, Option<String>>("local_environment_path")?,
    ) else {
        return Ok(None);
    };
    let error: Option<String> = row.get("environment_error")?;
    if target == "local" {
        if state == "missing" {
            return Ok(None);
        }
        let mut environment = json!({
            "kind": "local", "state": state, "path": local_path, "localExecutionContextId": local_id,
        });
        if state == "failed" {
            environment["error"] = json!(error.as_deref().unwrap_or("Local environment failed"));
        } else if state != "ready" {
            insert_optional(&mut environment, "error", error);
        }
        return Ok(Some(environment));
    }

    let (Some(id), Some(context), Some(path)) = (
        row.get::<_, Option<String>>("managed_worktree_id")?,
        row.get::<_, Option<String>>("managed_worktree_execution_context_id")?,
        row.get::<_, Option<String>>("managed_worktree_path")?,
    ) else {
        return Ok(None);
    };
    let mut environment = json!({
        "kind": "worktree", "path": path, "localExecutionContextId": local_id,
        "worktreeId": id, "worktreeExecutionContextId": context, "state": state,
    });
    if matches!(state.as_str(), "missing" | "failed") {
        environment["error"] =
            json!(error.unwrap_or_else(|| format!("Worktree environment is {state}")));
    } else if state != "ready" {
        insert_optional(&mut environment, "error", error);
    }
    Ok(Some(environment))
}

fn map_git(row: &Row<'_>) -> rusqlite::Result<Option<Value>> {
    let target: Option<String> = row.get("environment_active_target")?;
    let environment_state: Option<String> = row.get("environment_state")?;
    let worktree_state: Option<String> = row.get("actual_worktree_state")?;
    let active_worktree = matches!(
        worktree_state.as_deref(),
        Some("ready" | "dirty" | "retained")
    );
    let context_state: Option<String> = row.get("context_git_state")?;
    let unavailable = || Some(json!({"state": "unavailable", "dirty": false}));
    if (target.as_deref() == Some("worktree") && environment_state.as_deref() != Some("ready"))
        || (worktree_state.is_some() && !active_worktree)
        || context_state.as_deref() == Some("unavailable")
    {
        return Ok(unavailable());
    }

    if context_state.as_deref() == Some("ready") {
        let dirty = row.get::<_, Option<i64>>("context_git_dirty")? == Some(1);
        if let Some(branch) = row.get::<_, Option<String>>("context_git_branch")? {
            return Ok(Some(
                json!({"state": "ready", "branch": branch, "dirty": dirty}),
            ));
        }
        if let Some(revision) = row.get::<_, Option<String>>("context_git_detached_head")? {
            return Ok(Some(
                json!({"state": "ready", "detachedHead": revision, "dirty": dirty}),
            ));
        }
        return Ok(unavailable());
    }

    if let Some(branch) = row.get::<_, Option<String>>("branch_name")?
        && active_worktree
    {
        let dirty = matches!(worktree_state.as_deref(), Some("dirty" | "retained"));
        if branch != "(detached)" {
            return Ok(Some(
                json!({"state": "ready", "branch": branch, "dirty": dirty}),
            ));
        }
        if let Some(revision) = row.get::<_, Option<String>>("actual_worktree_base_revision")? {
            return Ok(Some(
                json!({"state": "ready", "detachedHead": revision, "dirty": dirty}),
            ));
        }
        return Ok(unavailable());
    }
    Ok(None)
}

fn safe_integer(value: &Value) -> Option<i64> {
    const LIMIT: f64 = 9_007_199_254_740_991.0;
    let value = value.as_f64()?;
    (value.fract() == 0.0 && value.abs() <= LIMIT).then_some(value as i64)
}

fn latest_lines(value: Option<String>) -> Vec<String> {
    let Some(lines) = value
        .as_deref()
        .and_then(|value| serde_json::from_str::<Vec<Value>>(value).ok())
    else {
        return Vec::new();
    };
    let mut strings: Vec<_> = lines
        .into_iter()
        .filter_map(|line| {
            if let Value::String(line) = line {
                Some(line)
            } else {
                None
            }
        })
        .collect();
    strings.drain(..strings.len().saturating_sub(4));
    strings
}

fn insert_optional(object: &mut Value, field: &str, value: Option<impl Into<Value>>) {
    if let Some(value) = value {
        object[field] = value.into();
    }
}

/// Persist a work status and its graph event in the standard replayable command transaction.
pub fn set_work_status(
    connection: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    status: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(connection, command, committed_at, |tx, events| {
        let (scene_id, task_id, workspace_id, previous_status) = tx.query_row(
            "SELECT membership.scene_id, sessions.task_id, tasks.workspace_id, sessions.work_status
             FROM sessions
             JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
             JOIN tasks ON tasks.id = sessions.task_id
             WHERE sessions.id = ?",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                      row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
        ).optional()?.with_context(|| format!("Session {session_id} does not exist in a canvas"))?;
        if previous_status != status {
            tx.execute(
                "UPDATE sessions SET work_status = ?, updated_at = ?, version = version + 1 WHERE id = ?",
                params![status, now, session_id],
            )?;
        }
        let graph = project_scene_graph(tx, &scene_id, None)?;
        let result = json!({
            "sessionId": session_id, "sceneId": scene_id, "previousStatus": previous_status,
            "workStatus": status, "graph": graph,
        });
        events.push(DomainEventInput {
            event_id: format!("{}:work-status", command.command_id),
            event_type: "session.graph-summary-changed".into(),
            aggregate_type: "session".into(),
            aggregate_id: session_id.into(),
            workspace_id: Some(workspace_id),
            task_id: Some(task_id),
            session_id: Some(session_id.into()),
            payload: result.clone(),
            occurred_at: now,
            schema_version: None,
            required_terminal_sequence: None,
        });
        Ok(result)
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordInteractionInput {
    pub session_id: String,
    pub interaction_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defer_ordering: Option<bool>,
    pub now: i64,
}

/// Persist a terminal interaction and keep its ordering deferred while the card has focus.
pub fn record_interaction(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &RecordInteractionInput,
    invalid_kind_message: &str,
    committed_at: i64,
) -> Result<Value> {
    ensure!(
        matches!(
            input.interaction_kind.as_str(),
            "submit" | "control" | "provider-action"
        ),
        "{invalid_kind_message}"
    );
    let commit = execute(connection, command, committed_at, |tx, events| {
        let session_id = &input.session_id;
        let now = input.now;
        let (scene_id, task_id, workspace_id): (String, String, String) = tx
            .query_row(
                "SELECT membership.scene_id, sessions.task_id, tasks.workspace_id
                 FROM session_canvas_memberships AS membership
                 JOIN sessions ON sessions.id = membership.session_id
                 JOIN tasks ON tasks.id = sessions.task_id
                 WHERE membership.session_id = ?
                   AND sessions.archived_at IS NULL
                   AND tasks.archived_at IS NULL",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .with_context(|| format!("Session {session_id} does not exist in an active canvas"))?;
        tx.execute(
            "UPDATE runtime_sequences SET value = value + 1 WHERE name = 'session-user-interaction'",
            [],
        )?;
        let sequence: i64 = tx
            .query_row(
                "SELECT value FROM runtime_sequences WHERE name = 'session-user-interaction'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .context("session-user-interaction sequence does not exist")?;

        let active = input.defer_ordering == Some(true)
            || tx
                .prepare("SELECT 1 FROM window_scene_focus WHERE active_session_id = ? LIMIT 1")?
                .exists([session_id])?;
        if active {
            tx.execute(
                "UPDATE session_canvas_memberships
                 SET pending_user_interaction_seq = ?, updated_at = ?
                 WHERE session_id = ?",
                params![sequence, now, session_id],
            )?;
        } else {
            tx.execute(
                "UPDATE session_canvas_memberships
                 SET last_user_interaction_seq = ?, pending_user_interaction_seq = 0, updated_at = ?
                 WHERE session_id = ?",
                params![sequence, now, session_id],
            )?;
        }
        tx.execute(
            "UPDATE sessions
             SET last_activity_at = ?, updated_at = ?, version = version + 1
             WHERE id = ?",
            params![now, now, session_id],
        )?;
        tx.execute(
            "UPDATE tasks SET last_opened_at = ? WHERE id = ?",
            params![now, task_id],
        )?;
        tx.execute(
            "UPDATE workspaces SET last_opened_at = ? WHERE id = ?",
            params![now, workspace_id],
        )?;

        let focus_window: Option<String> = tx
            .query_row(
                "SELECT window_id FROM window_scene_focus
             WHERE scene_id = ?
             ORDER BY (active_session_id = ?) DESC, updated_at DESC
             LIMIT 1",
                params![scene_id, session_id],
                |row| row.get(0),
            )
            .optional()?;
        let graph = project_scene_graph(tx, &scene_id, focus_window.as_deref())?;
        let result = json!({
            "sessionId": session_id,
            "sceneId": scene_id,
            "taskId": task_id,
            "workspaceId": workspace_id,
            "interactionKind": input.interaction_kind,
            "sequence": sequence,
            "graph": graph
        });
        events.push(DomainEventInput {
            event_id: format!("{}:session-user-interacted", command.command_id),
            event_type: "session.user-interacted".into(),
            aggregate_type: "session".into(),
            aggregate_id: session_id.clone(),
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
    Ok(commit.result)
}
