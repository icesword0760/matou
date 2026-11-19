//! Persistent lifecycle of ordinary Shell sessions and their process runs.
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
pub struct StartShellRunInput {
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

pub fn start_shell_run(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &StartShellRunInput,
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
             ) VALUES (?, ?, ?, ?, 'shell', ?, 'running', ?, ?, ?)",
            params![
                input.id,
                input.session_id,
                ordinal,
                input.runtime_generation,
                input.pid,
                input.cols,
                input.rows,
                input.now
            ],
        )?;
        tx.execute(
            "UPDATE sessions
             SET status = 'running', work_status = 'idle', updated_at = ?, last_activity_at = ?,
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
        tx.execute(
            "UPDATE session_runs SET status = 'exited', ended_at = ?, exit_code = ?, signal = ?
             WHERE id = ?",
            params![now, exit_code, signal, run_id],
        )?;
        tx.execute(
            "UPDATE sessions
             SET status = CASE WHEN archived_at IS NULL THEN 'exited' ELSE 'archived' END,
                 work_status = 'exited', updated_at = ?, last_activity_at = ?,
                 version = version + 1
             WHERE id = ?",
            params![now, now, before.session_id],
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
