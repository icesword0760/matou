//! Persistent lifecycle of sessions and their process runs.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    hierarchy::Session,
    storage::{CommandMetadata, DomainCommit, DomainEventInput, execute},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShellSessionInput {
    pub id: String,
    pub task_id: String,
    pub execution_context_id: String,
    pub title: String,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunInput {
    pub id: String,
    pub session_id: String,
    pub runtime_generation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    pub cols: i64,
    pub rows: i64,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRun {
    pub id: String,
    pub session_id: String,
    pub ordinal: i64,
    pub runtime_generation: String,
    pub profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    pub status: String,
    pub cols: i64,
    pub rows: i64,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i64>,
}

impl SessionRun {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            session_id: row.get("session_id")?,
            ordinal: row.get("ordinal")?,
            runtime_generation: row.get("runtime_generation")?,
            profile: row.get("profile")?,
            pid: row.get("pid")?,
            status: row.get("status")?,
            cols: row.get("cols")?,
            rows: row.get("rows")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            exit_code: row.get("exit_code")?,
            signal: row.get("signal")?,
        })
    }
}

pub fn create_shell_session(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &CreateShellSessionInput,
    committed_at: i64,
) -> Result<DomainCommit> {
    let title = trim(&input.title);
    ensure!(!title.is_empty(), "Session title must not be empty");
    execute(db, command, committed_at, |tx, events| {
        let task_workspace: String = tx
            .query_row(
                "SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL",
                [&input.task_id],
                |row| row.get(0),
            )
            .optional()?
            .with_context(|| format!("Task {} does not exist or is archived", input.task_id))?;
        let context_workspace: Option<String> = tx
            .query_row(
                "SELECT workspace_id FROM execution_contexts WHERE id = ? AND archived_at IS NULL",
                [&input.execution_context_id],
                |row| row.get(0),
            )
            .optional()?;
        ensure!(
            context_workspace.as_deref() == Some(task_workspace.as_str()),
            "Session execution context must belong to the Task Workspace"
        );
        let title_source = if title == "Shell" {
            "default"
        } else {
            "manual"
        };
        tx.execute(
            "INSERT INTO sessions (
               id, task_id, execution_context_id, kind, status, title, title_source,
               created_at, updated_at, last_activity_at, version
             ) VALUES (?, ?, ?, 'shell', 'created', ?, ?, ?, ?, ?, 1)",
            params![
                input.id,
                input.task_id,
                input.execution_context_id,
                title,
                title_source,
                input.now,
                input.now,
                input.now
            ],
        )?;
        let session = require_session(tx, &input.id)?;
        let value = serde_json::to_value(&session)?;
        let mut event = session_event(
            command,
            "session.created",
            &session,
            input.now,
            value.clone(),
        );
        event.event_id = format!("{}:session-created", command.command_id);
        event.workspace_id = Some(task_workspace);
        events.push(event);
        Ok(value)
    })
}

pub fn start_run(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &StartRunInput,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(db, command, committed_at, |tx, events| {
        let session = require_session(tx, &input.session_id)?;
        ensure!(
            session.archived_at.is_none(),
            "archived Session cannot start a new run"
        );
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM session_runs WHERE session_id = ?",
            [&input.session_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO session_runs (
               id, session_id, ordinal, runtime_generation, profile, pid,
               status, cols, rows, started_at
             ) VALUES (?, ?, ?, ?, ?, ?, 'running', ?, ?, ?)",
            params![
                input.id,
                input.session_id,
                ordinal,
                input.runtime_generation,
                session.kind,
                input.pid,
                input.cols,
                input.rows,
                input.now
            ],
        )?;
        tx.execute(
            "UPDATE session_fork_intents
             SET state = 'starting', stage = 'restoring-provider', completed_steps = 1,
                 attempt = 1, attempt_count = 1, started_at = ?, updated_at = ?
             WHERE session_id = ? AND operation_id = ? || session_id AND state = 'pending'",
            params![input.now, input.now, input.session_id, NATIVE_FORK_PREFIX],
        )?;
        tx.execute(
            "UPDATE sessions
             SET status = 'running',
                 work_status = CASE WHEN kind = 'claude-code' THEN work_status ELSE 'idle' END,
                 updated_at = ?, last_activity_at = ?,
                 version = version + 1
             WHERE id = ?",
            params![input.now, input.now, input.session_id],
        )?;
        let run = serde_json::to_value(require_run(tx, &input.id)?)?;
        events.push(session_event(
            command,
            "session.run-started",
            &session,
            input.now,
            json!({"run": run}),
        ));
        Ok(run)
    })
}

pub fn finish_run(
    db: &mut Connection,
    command: &CommandMetadata,
    run_id: &str,
    exit_code: i64,
    signal: Option<i64>,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(db, command, committed_at, |tx, events| {
        let before = require_run(tx, run_id)?;
        let session = require_session(tx, &before.session_id)?;
        let owns_work_status = before.profile != "claude-code"
            || tx
                .prepare(
                    "SELECT 1 FROM domain_events WHERE session_id = ?
                 AND event_type = 'provider-binding.recorded'
                 AND json_extract(payload_json, '$.runId') = ?",
                )?
                .exists(params![before.session_id, run_id])?;
        tx.execute(
            "UPDATE session_runs SET status = 'exited', ended_at = ?, exit_code = ?, signal = ?
             WHERE id = ?",
            params![now, exit_code, signal, run_id],
        )?;
        tx.execute(
            "UPDATE sessions
             SET status = CASE WHEN archived_at IS NULL THEN 'exited' ELSE 'archived' END,
                 work_status = CASE WHEN ? AND NOT (work_status = 'error' AND EXISTS (
                     SELECT 1 FROM provider_bindings WHERE session_id = sessions.id
                       AND provider = sessions.kind AND restore_state = 'failed'
                 )) THEN 'exited' ELSE work_status END,
                 updated_at = ?, last_activity_at = ?,
                 version = version + 1
             WHERE id = ?",
            params![owns_work_status, now, now, before.session_id],
        )?;
        let run = serde_json::to_value(require_run(tx, run_id)?)?;
        events.push(session_event(
            command,
            "session.run-exited",
            &session,
            now,
            json!({"run": run}),
        ));
        Ok(run)
    })
}

pub struct ClaudeBinding {
    pub id: String,
    pub provider_session_id: String,
    pub metadata: Value,
    pub resumable: bool,
    pub restore_state: String,
}

pub(crate) const NATIVE_FORK_PREFIX: &str = "native-fork:";

pub struct ClaudeFork {
    pub source_id: String,
    pub metadata: Value,
}

pub fn pending_claude_fork(db: &Connection, session_id: &str) -> Result<Option<ClaudeFork>> {
    Ok(db
        .query_row(
            "SELECT fork.source_provider_session_id, fork.permission_mode, binding.metadata_json
         FROM session_fork_intents AS fork
         LEFT JOIN provider_bindings AS binding ON binding.session_id = fork.source_session_id
           AND binding.provider = fork.source_provider
           AND binding.provider_session_id = fork.source_provider_session_id
         WHERE fork.session_id = ? AND fork.operation_id = ? || fork.session_id
           AND fork.state = 'pending'",
            params![session_id, NATIVE_FORK_PREFIX],
            |row| {
                let permission: String = row.get(1)?;
                let stored: Option<String> = row.get(2)?;
                let binding: Value = stored
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(Value::Null);
                Ok(ClaudeFork {
                    source_id: row.get(0)?,
                    metadata: json!({
                        "providerConfigId": binding["providerConfigId"],
                        "model": binding["model"],
                        "permissionMode": permission,
                    }),
                })
            },
        )
        .optional()?)
}

pub fn unfinished_claude_forks(db: &Connection) -> Result<Vec<String>> {
    let mut query = db.prepare(
        "SELECT fork.session_id FROM session_fork_intents AS fork
         JOIN sessions ON sessions.id = fork.session_id
         WHERE fork.operation_id = ? || fork.session_id AND fork.state = 'starting'
           AND sessions.archived_at IS NULL",
    )?;
    Ok(query
        .query_map([NATIVE_FORK_PREFIX], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn has_unconfirmed_claude_fork(db: &Connection, session_id: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS (SELECT 1 FROM session_fork_intents
         WHERE session_id = ? AND operation_id = ? || session_id AND state <> 'succeeded')",
        params![session_id, NATIVE_FORK_PREFIX],
        |row| row.get(0),
    )?)
}

pub fn fail_claude_fork(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    error: &str,
    now: i64,
) -> Result<DomainCommit> {
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, session_id)?;
        if session.archived_at.is_some()
            || tx.execute(
                "UPDATE session_fork_intents
             SET state = 'failed', stage = 'failed', error_message = ?,
                 completed_at = ?, updated_at = ?
             WHERE session_id = ? AND operation_id = ? || session_id
               AND state IN ('pending', 'starting')",
                params![error, now, now, session_id, NATIVE_FORK_PREFIX],
            )? == 0
        {
            return Ok(Value::Null);
        }
        tx.execute(
            "UPDATE sessions SET status = 'interrupted', work_status = 'error',
                 updated_at = ?, version = version + 1 WHERE id = ?",
            params![now, session_id],
        )?;
        let payload = json!({
            "result": "fork-failed", "sessionId": session_id,
            "provider": "claude-code", "forkError": error,
        });
        events.push(session_event(
            command,
            "session.fork-failed",
            &session,
            now,
            payload.clone(),
        ));
        Ok(payload)
    })
}

pub fn claude_binding(db: &Connection, session_id: &str) -> Result<Option<ClaudeBinding>> {
    Ok(db
        .query_row(
            "SELECT id, provider_session_id, metadata_json, restore_state,
                (resume_state = 'available' AND (validated_at IS NOT NULL
                    OR CASE WHEN json_valid(metadata_json)
                       THEN json_extract(metadata_json, '$.loadedFromCatalog') = 1
                       ELSE 0 END IS TRUE)
                    AND invalidated_at IS NULL AND user_exited_at IS NULL
                    AND restore_state <> 'failed' AND provider_session_id <> '') AS resumable
         FROM provider_bindings WHERE session_id = ? AND provider = 'claude-code'
           AND EXISTS (SELECT 1 FROM sessions WHERE id = session_id AND kind = 'claude-code')
         ORDER BY updated_at DESC, (invalidated_at IS NULL) DESC, id DESC LIMIT 1",
            [session_id],
            |row| {
                let metadata: String = row.get("metadata_json")?;
                Ok(ClaudeBinding {
                    id: row.get("id")?,
                    provider_session_id: row.get("provider_session_id")?,
                    metadata: serde_json::from_str(&metadata).unwrap_or(Value::Null),
                    resumable: row.get("resumable")?,
                    restore_state: row.get("restore_state")?,
                })
            },
        )
        .optional()?)
}

#[derive(Clone, Copy)]
pub enum ClaudeRestore<'a> {
    Begin(&'a str),
    Fail { binding_id: &'a str, error: &'a str },
    Retry,
    Fresh,
}

pub struct ClaudeCatalogLoad<'a> {
    pub binding_id: &'a str,
    pub session_id: &'a str,
    pub scene_id: &'a str,
    pub cwd: &'a str,
    pub provider_session_id: &'a str,
    pub title: &'a str,
    pub model: Option<&'a str>,
    pub permission_mode: &'a str,
}

pub fn load_claude_catalog_binding(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ClaudeCatalogLoad<'_>,
    now: i64,
) -> Result<DomainCommit> {
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, input.session_id)?;
        let in_scene: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM session_canvas_memberships AS member
                JOIN scenes ON scenes.id = member.scene_id AND scenes.archived_at IS NULL
                WHERE member.session_id = ? AND member.scene_id = ?)",
            params![input.session_id, input.scene_id],
            |row| row.get(0),
        )?;
        ensure!(
            session.archived_at.is_none()
                && in_scene
                && session.cwd == input.cwd
                && matches!(session.kind.as_str(), "shell" | "claude-code"),
            "Card is no longer available in this scene and working directory"
        );
        let running: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM session_runs
                WHERE session_id = ? AND status IN ('starting', 'running'))",
            [input.session_id],
            |row| row.get(0),
        )?;
        ensure!(!running, "Card already has a running process");
        ensure!(
            !has_unconfirmed_claude_fork(tx, input.session_id)?,
            "Card has an unconfirmed fork"
        );

        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id, metadata_json FROM provider_bindings
                 WHERE session_id = ? AND provider = 'claude-code' AND provider_session_id = ?",
                params![input.session_id, input.provider_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let revision: i64 = tx.query_row(
            "SELECT COALESCE(MAX(CASE WHEN json_valid(metadata_json)
                THEN CAST(json_extract(metadata_json, '$.spawnRevision') AS INTEGER) END), 0)
             FROM provider_bindings WHERE session_id = ? AND provider = 'claude-code'",
            [input.session_id],
            |row| row.get(0),
        )?;
        let binding_id = existing.as_ref().map_or(input.binding_id, |(id, _)| id);
        let mut metadata: serde_json::Map<String, Value> = existing
            .as_ref()
            .and_then(|(_, metadata)| serde_json::from_str(metadata).ok())
            .unwrap_or_default();
        metadata.remove("invalidationReason");
        metadata.remove("provisional");
        metadata.remove("lastHookEvent");
        if let Some(model) = input.model {
            metadata.insert("model".into(), json!(model));
        } else {
            metadata.remove("model");
        }
        let revision = revision.saturating_add(1);
        metadata.extend([
            ("permissionMode".into(), json!(input.permission_mode)),
            ("loadedFromCatalog".into(), json!(true)),
            ("spawnRevision".into(), json!(revision)),
            ("observedUserPrompt".into(), json!(false)),
            ("observedNormalStop".into(), json!(false)),
            ("canFork".into(), json!(false)),
        ]);
        tx.execute(
            "UPDATE provider_bindings SET resume_state = 'expired', restore_state = 'none',
                restore_error = NULL, invalidated_at = COALESCE(invalidated_at, ?), updated_at = ?
             WHERE session_id = ? AND provider = 'claude-code' AND id <> ?",
            params![now, now, input.session_id, binding_id],
        )?;
        tx.execute(
            "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id,
                resume_state, restore_state, metadata_json, created_at, updated_at)
             VALUES (?, ?, 'claude-code', ?, 'available', 'none', ?, ?, ?)
             ON CONFLICT(session_id, provider, provider_session_id) DO UPDATE SET
                resume_state = 'available', restore_state = 'none', restore_error = NULL,
                invalidated_at = NULL, user_exited_at = NULL, validated_at = NULL,
                metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
            params![
                binding_id,
                input.session_id,
                input.provider_session_id,
                serde_json::to_string(&metadata)?,
                now,
                now
            ],
        )?;
        tx.execute(
            "UPDATE sessions SET kind = 'claude-code', title = ?, status = 'starting',
                 work_status = 'starting', updated_at = ?, last_activity_at = ?, version = version + 1
             WHERE id = ?",
            params![input.title, now, now, input.session_id],
        )?;
        let payload = json!({
            "sessionId": input.session_id, "provider": "claude-code", "bindingId": binding_id,
            "providerSessionId": input.provider_session_id, "restoreState": "none",
            "restoreError": null, "spawnRevision": revision,
        });
        events.push(session_event(
            command,
            "session.mode-changed",
            &session,
            now,
            payload.clone(),
        ));
        Ok(payload)
    })
}

pub fn change_claude_restore(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    change: ClaudeRestore<'_>,
    now: i64,
) -> Result<Option<DomainCommit>> {
    let Some(binding) = claude_binding(db, session_id)? else {
        return Ok(None);
    };
    let mut metadata = binding.metadata.as_object().cloned().unwrap_or_default();
    let (resume, restore, error, update_session) = match change {
        ClaudeRestore::Begin(id)
            if binding.id == id && binding.resumable && binding.restore_state != "restoring" =>
        {
            ("available", "restoring", None, None)
        }
        ClaudeRestore::Fail { binding_id, error }
            if binding.id == binding_id && binding.resumable =>
        {
            ("failed", "failed", Some(error), Some("error"))
        }
        ClaudeRestore::Retry | ClaudeRestore::Fresh if binding.restore_state == "failed" => {
            let revision = metadata
                .get("spawnRevision")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            metadata.insert("spawnRevision".into(), json!(revision.saturating_add(1)));
            match change {
                ClaudeRestore::Retry => ("available", "restoring", None, Some("starting")),
                _ => ("expired", "none", None, Some("starting")),
            }
        }
        _ => return Ok(None),
    };
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, session_id)?;
        ensure!(
            session.archived_at.is_none() && session.kind == "claude-code",
            "Claude restore requires an active Claude Code card"
        );
        tx.execute(
            "UPDATE provider_bindings SET resume_state = ?, restore_state = ?, restore_error = ?,
                 user_exited_at = NULL, metadata_json = ?, updated_at = ?,
                 invalidated_at = CASE WHEN ? = 'available' THEN NULL
                   WHEN ? = 'expired' THEN COALESCE(invalidated_at, ?) ELSE ? END
             WHERE id = ?",
            params![
                resume,
                restore,
                error,
                serde_json::to_string(&metadata)?,
                now,
                resume,
                resume,
                now,
                now,
                binding.id
            ],
        )?;
        if let Some(work_status) = update_session {
            tx.execute(
                "UPDATE sessions SET
                     status = CASE WHEN ? = 'starting' THEN 'starting' ELSE status END,
                     work_status = ?, updated_at = ?, version = version + 1 WHERE id = ?",
                params![work_status, work_status, now, session_id],
            )?;
        }
        let payload = json!({
            "sessionId": session_id, "provider": "claude-code",
            "bindingId": binding.id, "providerSessionId": binding.provider_session_id,
            "restoreState": restore, "restoreError": error,
            "spawnRevision": metadata.get("spawnRevision"),
        });
        events.push(session_event(
            command,
            "session.restore-state-changed",
            &session,
            now,
            payload.clone(),
        ));
        Ok(payload)
    })
    .map(Some)
}

pub struct ClaudeIdentity<'a> {
    pub binding_id: &'a str,
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub generation: &'a str,
    pub provider_session_id: &'a str,
    pub expected_id: Option<&'a str>,
    pub launch_metadata: &'a Value,
    pub observed_metadata: &'a Value,
    pub event_name: &'a str,
    pub first_for_run: bool,
}

pub fn record_claude_identity(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ClaudeIdentity<'_>,
    now: i64,
) -> Result<DomainCommit> {
    let provider_session_id = if let Some(expected) = input.expected_id {
        ensure!(
            expected == input.provider_session_id,
            "Provider session identity mismatch"
        );
        ensure!(
            !matches!(input.event_name, "unknown" | "SessionEnd"),
            "Provider restore requires a lifecycle event that confirms the conversation"
        );
        input.provider_session_id
    } else {
        trim(input.provider_session_id)
    };
    ensure!(
        !provider_session_id.is_empty(),
        "Provider session identity must not be empty"
    );
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, input.session_id)?;
        let run = tx
            .query_row(
                "SELECT id, runtime_generation, status, profile FROM session_runs
             WHERE session_id = ? ORDER BY ordinal DESC LIMIT 1",
                [input.session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        ensure!(
            session.archived_at.is_none()
                && session.kind == "claude-code"
                && run
                    .as_ref()
                    .is_some_and(|(id, generation, status, profile)| {
                        id == input.run_id
                            && generation == input.generation
                            && status == "running"
                            && profile == "claude-code"
                    }),
            "Provider hook no longer owns its SessionRun"
        );
        let fork: Option<(String, String)> = tx
            .query_row(
                "SELECT state, source_provider_session_id FROM session_fork_intents
             WHERE session_id = ? AND operation_id = ? || session_id
               AND state <> 'succeeded'",
                params![input.session_id, NATIVE_FORK_PREFIX],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((state, source_id)) = &fork {
            ensure!(
                state == "starting"
                    && input.first_for_run
                    && input.expected_id.is_none()
                    && !matches!(input.event_name, "unknown" | "SessionEnd"),
                "Fork requires a lifecycle identity from its active launch"
            );
            ensure!(
                provider_session_id != trim(source_id),
                "Fork returned the source conversation"
            );
        }
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id, metadata_json FROM provider_bindings
             WHERE session_id = ? AND provider = 'claude-code' AND provider_session_id = ?",
                params![input.session_id, provider_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if input.expected_id.is_some() {
            ensure!(
                claude_binding(tx, input.session_id)?.is_some_and(|binding| binding.resumable
                    && binding.provider_session_id == provider_session_id),
                "Provider hook no longer owns its restore binding"
            );
        }
        let binding_id = existing.as_ref().map_or(input.binding_id, |(id, _)| id);
        let mut metadata: serde_json::Map<String, Value> = existing
            .as_ref()
            .and_then(|(_, metadata)| serde_json::from_str(metadata).ok())
            .unwrap_or_default();
        for (key, value) in input.launch_metadata.as_object().into_iter().flatten() {
            if key == "providerConfigId" {
                metadata.insert(key.clone(), value.clone());
            } else {
                metadata.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        metadata.extend(
            input
                .observed_metadata
                .as_object()
                .into_iter()
                .flatten()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        metadata.remove("invalidationReason");
        metadata.remove("provisional");
        metadata.insert("lastHookEvent".into(), json!(input.event_name));
        for (key, observed) in [
            ("observedUserPrompt", input.event_name == "UserPromptSubmit"),
            ("observedNormalStop", input.event_name == "Stop"),
        ] {
            metadata.insert(
                key.into(),
                Value::Bool(observed || metadata.get(key).and_then(Value::as_bool) == Some(true)),
            );
        }
        let can_fork =
            metadata["observedUserPrompt"] == true && metadata["observedNormalStop"] == true;
        metadata.insert("canFork".into(), json!(can_fork));
        tx.execute(
            "INSERT INTO provider_bindings (
                 id, session_id, provider, provider_session_id, resume_state,
                 metadata_json, created_at, updated_at, validated_at
             ) VALUES (?, ?, 'claude-code', ?, 'available', ?, ?, ?, ?)
             ON CONFLICT(session_id, provider, provider_session_id) DO UPDATE SET
                 resume_state = 'available', metadata_json = excluded.metadata_json,
                 updated_at = excluded.updated_at, validated_at = excluded.validated_at,
                 invalidated_at = NULL, restore_state = 'none', restore_error = NULL",
            params![
                binding_id,
                input.session_id,
                provider_session_id,
                serde_json::to_string(&metadata)?,
                now,
                now,
                now
            ],
        )?;
        if fork.is_some() {
            tx.execute(
                "UPDATE session_fork_intents SET state = 'succeeded', stage = 'succeeded',
                     completed_steps = total_steps, completed_at = ?, updated_at = ?
                 WHERE session_id = ?",
                params![now, now, input.session_id],
            )?;
            tx.execute(
                "UPDATE sessions SET work_status = 'idle', updated_at = ?, version = version + 1
                 WHERE id = ? AND work_status = 'starting'",
                params![now, input.session_id],
            )?;
        }
        // Publish only the fields owned by this observation. Existing binding
        // metadata can contain fields from other versions of the runtime.
        let mut public_metadata = serde_json::Map::new();
        for key in [
            "providerConfigId",
            "model",
            "permissionMode",
            "loadedFromCatalog",
            "cwd",
            "lastHookEvent",
            "observedUserPrompt",
            "observedNormalStop",
            "canFork",
        ] {
            if let Some(value) = metadata.get(key) {
                public_metadata.insert(key.into(), value.clone());
            }
        }
        let payload = json!({
            "sessionId": input.session_id, "runId": input.run_id,
            "provider": "claude-code", "providerSessionId": provider_session_id,
            "bindingId": binding_id, "metadata": public_metadata,
            "restoreState": "none", "restoreError": null,
            "spawnRevision": metadata.get("spawnRevision")
                .or_else(|| input.launch_metadata.get("spawnRevision")),
        });
        events.push(session_event(
            command,
            "provider-binding.recorded",
            &session,
            now,
            payload.clone(),
        ));
        if input.expected_id.is_some() && input.first_for_run {
            let mut event = session_event(
                command,
                "session.restore-state-changed",
                &session,
                now,
                payload.clone(),
            );
            event.event_id.push_str(":restore");
            events.push(event);
        }
        let cwd = input.observed_metadata["cwd"]
            .as_str()
            .unwrap_or(&session.cwd);
        if cwd != session.cwd {
            tx.execute(
                "UPDATE sessions SET cwd = ?, updated_at = ?, version = version + 1
                 WHERE id = ?",
                params![cwd, now, input.session_id],
            )?;
        }
        if cwd != session.cwd || input.first_for_run {
            let work_status: String = tx.query_row(
                "SELECT work_status FROM sessions WHERE id = ?",
                [input.session_id],
                |row| row.get(0),
            )?;
            let mut event = session_event(
                command,
                "session.mode-changed",
                &session,
                now,
                json!({"runId": input.run_id, "provider": "claude-code", "workStatus": work_status}),
            );
            event.event_id.push_str(":mode");
            events.push(event);
        }
        Ok(payload)
    })
}

pub fn interrupt_run(
    db: &mut Connection,
    command: &CommandMetadata,
    run_id: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(db, command, committed_at, |tx, events| {
        let (run, event) = interrupt_run_in_transaction(tx, command, run_id, now)?;
        events.push(event);
        Ok(run)
    })
}

pub(crate) fn interrupt_run_in_transaction(
    tx: &Connection,
    command: &CommandMetadata,
    run_id: &str,
    now: i64,
) -> Result<(Value, DomainEventInput)> {
    let before = require_run(tx, run_id)?;
    let session = require_session(tx, &before.session_id)?;
    ensure!(
        matches!(before.status.as_str(), "starting" | "running"),
        "only an active SessionRun can be interrupted"
    );
    tx.execute(
        "UPDATE session_runs SET status = 'interrupted', ended_at = ? WHERE id = ?",
        params![now, run_id],
    )?;
    tx.execute(
        "UPDATE sessions SET
               status = CASE WHEN archived_at IS NULL THEN 'interrupted' ELSE status END,
               work_status = CASE WHEN archived_at IS NULL THEN 'interrupted' ELSE work_status END,
               updated_at = ?, last_activity_at = ?, version = version + 1
             WHERE id = ?",
        params![now, now, before.session_id],
    )?;
    let run = serde_json::to_value(require_run(tx, run_id)?)?;
    let event = session_event(
        command,
        "session.run-interrupted",
        &session,
        now,
        json!({"run": run}),
    );
    Ok((run, event))
}

pub fn rename_session(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    title: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    let title = trim(title);
    ensure!(!title.is_empty(), "Session title must not be empty");
    execute(db, command, committed_at, |tx, events| {
        let before = require_session(tx, session_id)?;
        ensure!(
            before.archived_at.is_none(),
            "archived Session cannot be renamed"
        );
        tx.execute(
            "UPDATE sessions SET title = ?, title_source = 'manual', updated_at = ?,
               version = version + 1 WHERE id = ?",
            params![title, now, session_id],
        )?;
        let session = serde_json::to_value(require_session(tx, session_id)?)?;
        events.push(session_event(
            command,
            "session.updated",
            &before,
            now,
            json!({"session": session}),
        ));
        Ok(session)
    })
}

pub fn update_cwd(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    cwd: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    let cwd = trim(cwd);
    ensure!(!cwd.is_empty(), "Session cwd must not be empty");
    execute(db, command, committed_at, |tx, events| {
        let before = require_session(tx, session_id)?;
        ensure!(
            before.archived_at.is_none(),
            "archived Session cwd cannot be modified"
        );
        tx.execute(
            "UPDATE sessions SET cwd = ?, updated_at = ?, last_activity_at = ?,
               version = version + 1 WHERE id = ?",
            params![cwd, now, now, session_id],
        )?;
        let session = serde_json::to_value(require_session(tx, session_id)?)?;
        events.push(session_event(
            command,
            "session.cwd-updated",
            &before,
            now,
            json!({"cwd": cwd}),
        ));
        Ok(session)
    })
}

pub fn archive_session(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(db, command, committed_at, |tx, events| {
        let before = require_session(tx, session_id)?;
        require_no_active_run(tx, session_id)?;
        tx.execute(
            "UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?,
               version = version + 1 WHERE id = ?",
            params![now, now, session_id],
        )?;
        let session = serde_json::to_value(require_session(tx, session_id)?)?;
        events.push(session_event(
            command,
            "session.archived",
            &before,
            now,
            json!({"archivedAt": now}),
        ));
        Ok(session)
    })
}

pub(crate) fn require_no_active_run(db: &Connection, session_id: &str) -> Result<()> {
    let active = db
        .prepare(
            "SELECT 1 FROM session_runs WHERE session_id = ? AND status IN ('starting', 'running')",
        )?
        .exists([session_id])?;
    ensure!(
        !active,
        "active SessionRuns must be stopped before archiving a Session"
    );
    Ok(())
}

pub fn get_session(db: &Connection, id: &str) -> Result<Option<Session>> {
    Ok(db
        .query_row(
            "SELECT * FROM sessions WHERE id = ?",
            [id],
            Session::from_row,
        )
        .optional()?)
}

pub fn list_sessions(db: &Connection) -> Result<Vec<Session>> {
    let mut query = db.prepare("SELECT * FROM sessions ORDER BY created_at, id")?;
    Ok(query
        .query_map([], Session::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn get_run(db: &Connection, id: &str) -> Result<Option<SessionRun>> {
    Ok(db
        .query_row(
            "SELECT * FROM session_runs WHERE id = ?",
            [id],
            SessionRun::from_row,
        )
        .optional()?)
}

pub fn list_runs(db: &Connection, session_id: &str) -> Result<Vec<SessionRun>> {
    let mut query =
        db.prepare("SELECT * FROM session_runs WHERE session_id = ? ORDER BY ordinal")?;
    Ok(query
        .query_map([session_id], SessionRun::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn list_all_runs(db: &Connection) -> Result<Vec<SessionRun>> {
    let mut query = db.prepare("SELECT * FROM session_runs ORDER BY session_id, ordinal")?;
    Ok(query
        .query_map([], SessionRun::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

fn require_session(db: &Connection, id: &str) -> Result<Session> {
    get_session(db, id)?.context("Session does not exist")
}

fn require_run(db: &Connection, id: &str) -> Result<SessionRun> {
    get_run(db, id)?.context("SessionRun does not exist")
}

fn session_event(
    command: &CommandMetadata,
    event_type: &str,
    session: &Session,
    occurred_at: i64,
    payload: Value,
) -> DomainEventInput {
    DomainEventInput {
        event_id: command.command_id.clone(),
        event_type: event_type.to_owned(),
        aggregate_type: "session".into(),
        aggregate_id: session.id.clone(),
        workspace_id: None,
        task_id: Some(session.task_id.clone()),
        session_id: Some(session.id.clone()),
        payload,
        schema_version: None,
        required_terminal_sequence: None,
        occurred_at,
    }
}

fn trim(text: &str) -> &str {
    // ECMAScript String.trim includes U+FEFF and does not remove U+0085.
    text.trim_matches(|character| {
        matches!(character,
            '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}'
            | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
            | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    })
}
