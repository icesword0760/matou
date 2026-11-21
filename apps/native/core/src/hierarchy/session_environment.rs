//! Durable journal and execution context updates for session environment changes.

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Local,
    Worktree,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Worktree => "worktree",
        }
    }

    fn parse(value: &str) -> Self {
        if value == "worktree" {
            Self::Worktree
        } else {
            Self::Local
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Restore,
    Locate,
    Handoff,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restore => "restore",
            Self::Locate => "locate",
            Self::Handoff => "handoff",
        }
    }
}

#[derive(Debug)]
pub struct Binding {
    pub target: Target,
    pub state: String,
    pub error: Option<String>,
    pub local_path: String,
}

#[derive(Debug)]
pub struct OwnedWorktree {
    pub session_id: String,
    pub path: String,
    pub repository_root: String,
    pub branch: String,
    pub base_revision: Option<String>,
    pub base_ref: Option<String>,
}

#[derive(Debug)]
pub struct Transition {
    pub session_id: String,
    pub operation_id: String,
    pub kind: Kind,
    pub previous_target: Target,
    pub previous_state: String,
    pub previous_error: Option<String>,
    pub target: Target,
    pub candidate: Option<String>,
}

pub fn binding(db: &Connection, id: &str) -> Result<Option<Binding>> {
    Ok(db
        .query_row(
            "SELECT binding.active_target, binding.state, binding.error_message, local.cwd
         FROM session_environment_bindings binding
         JOIN execution_contexts local ON local.id = binding.local_execution_context_id
         JOIN sessions ON sessions.id = binding.session_id
         WHERE binding.session_id = ? AND sessions.archived_at IS NULL",
            [id],
            |row| {
                Ok(Binding {
                    target: Target::parse(&row.get::<_, String>(0)?),
                    state: row.get(1)?,
                    error: row.get(2)?,
                    local_path: row.get(3)?,
                })
            },
        )
        .optional()?)
}

pub fn owned_worktrees(db: &Connection) -> Result<Vec<OwnedWorktree>> {
    let mut query = db.prepare(
        "SELECT binding.session_id, worktree.worktree_path, worktree.repository_root,
                worktree.branch_name, worktree.base_revision, worktree.base_ref
         FROM session_environment_bindings binding
         JOIN worktrees worktree ON worktree.id = binding.managed_worktree_id
         ORDER BY binding.session_id",
    )?;
    Ok(query
        .query_map([], |row| {
            Ok(OwnedWorktree {
                session_id: row.get(0)?,
                path: row.get(1)?,
                repository_root: row.get(2)?,
                branch: row.get(3)?,
                base_revision: row.get(4)?,
                base_ref: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn begin_transition(
    db: &mut Connection,
    id: &str,
    operation_id: String,
    kind: Kind,
    target: Target,
    candidate: Option<String>,
    now: i64,
) -> Result<Transition> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let binding = binding(&tx, id)?.context("session-unavailable")?;
    ensure!(
        !tx.prepare("SELECT 1 FROM session_environment_transitions WHERE session_id = ?")?
            .exists([id])?,
        "environment-transition-in-progress"
    );
    if target == Target::Worktree {
        ensure!(
            tx.prepare(
                "SELECT 1 FROM session_environment_bindings
            WHERE session_id = ? AND managed_worktree_id IS NOT NULL"
            )?
            .exists([id])?,
            "no-owned-worktree"
        );
    }
    let previous_state = match binding.state.as_str() {
        "recovering" | "handoff" => "failed".to_owned(),
        _ => binding.state,
    };
    tx.execute(
        "INSERT INTO session_environment_transitions
         (session_id, operation_id, kind, previous_active_target, previous_state,
          target, candidate_path, phase, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'accepted', ?, ?)",
        params![
            id,
            operation_id,
            kind.as_str(),
            binding.target.as_str(),
            previous_state,
            target.as_str(),
            candidate,
            now,
            now
        ],
    )?;
    tx.execute(
        "UPDATE session_environment_bindings SET state = ?, error_message = NULL, updated_at = ?
         WHERE session_id = ?",
        params![
            if kind == Kind::Handoff {
                "handoff"
            } else {
                "recovering"
            },
            now,
            id
        ],
    )?;
    tx.commit()?;
    Ok(Transition {
        session_id: id.into(),
        operation_id,
        kind,
        previous_target: binding.target,
        previous_state,
        previous_error: binding.error,
        target,
        candidate,
    })
}

pub fn list_transitions(db: &Connection) -> Result<Vec<Transition>> {
    let mut query = db.prepare(
        "SELECT session_id, operation_id, kind, previous_active_target, previous_state,
                target, candidate_path
         FROM session_environment_transitions ORDER BY created_at, session_id",
    )?;
    Ok(query
        .query_map([], |row| {
            Ok(Transition {
                session_id: row.get(0)?,
                operation_id: row.get(1)?,
                kind: match row.get::<_, String>(2)?.as_str() {
                    "restore" => Kind::Restore,
                    "locate" => Kind::Locate,
                    _ => Kind::Handoff,
                },
                previous_target: Target::parse(&row.get::<_, String>(3)?),
                previous_state: row.get(4)?,
                previous_error: None,
                target: Target::parse(&row.get::<_, String>(5)?),
                candidate: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn mark_transition_external_ready(db: &Connection, op: &Transition, now: i64) -> Result<()> {
    ensure!(
        db.execute(
            "UPDATE session_environment_transitions SET phase = 'external-ready', updated_at = ?
         WHERE session_id = ? AND operation_id = ?",
            params![now, op.session_id, op.operation_id],
        )? == 1,
        "stale environment transition"
    );
    Ok(())
}

pub fn complete_transition(
    db: &mut Connection,
    op: &Transition,
    cwd: &str,
    now: i64,
) -> Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure!(
        tx.execute(
            "DELETE FROM session_environment_transitions WHERE session_id = ? AND operation_id = ?
         AND (kind = 'handoff' OR phase = 'external-ready')",
            params![op.session_id, op.operation_id],
        )? == 1,
        "stale or unprepared environment transition"
    );
    ensure!(
        binding(&tx, &op.session_id)?.is_some(),
        "session-unavailable"
    );
    if op.kind != Kind::Handoff {
        let (worktree, context): (String, String) = tx.query_row(
            "SELECT worktree.id, worktree.execution_context_id
             FROM worktrees worktree JOIN session_environment_bindings binding
                 ON binding.managed_worktree_id = worktree.id WHERE binding.session_id = ?",
            [&op.session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        ensure!(
            !tx.prepare(
                "SELECT 1 FROM worktrees
            WHERE worktree_path = ? AND id <> ? AND state <> 'removed'"
            )?
            .exists(params![cwd, worktree])?,
            "path-owned-by-another-session"
        );
        tx.execute(
            "UPDATE worktrees SET worktree_path = CASE WHEN ? THEN ? ELSE worktree_path END,
             state = 'ready', updated_at = ? WHERE id = ?",
            params![op.kind == Kind::Locate, cwd, now, worktree],
        )?;
        if op.kind == Kind::Locate {
            tx.execute(
                "UPDATE execution_contexts SET cwd = ? WHERE id = ?",
                params![cwd, context],
            )?;
        }
    }
    set_context(&tx, &op.session_id, op.target, now)?;
    tx.execute(
        "UPDATE session_environment_bindings
         SET active_target = ?, state = 'ready', error_message = NULL, updated_at = ?
         WHERE session_id = ?",
        params![op.target.as_str(), now, op.session_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn rollback_transition(
    db: &mut Connection,
    op: &Transition,
    committed: bool,
    reason: &str,
    now: i64,
) -> Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if committed {
        ensure!(
            !tx.prepare("SELECT 1 FROM session_environment_transitions WHERE session_id = ?")?
                .exists([&op.session_id])?,
            "a newer environment transition exists"
        );
        if binding(&tx, &op.session_id)?.is_some() {
            set_context(&tx, &op.session_id, op.previous_target, now)?;
        }
    } else {
        ensure!(remove_transition(&tx, op)?, "stale environment transition");
    }
    let error = match op.previous_state.as_str() {
        "ready" => None,
        "failed" => Some(op.previous_error.as_deref().unwrap_or(reason)),
        _ => Some(reason),
    };
    tx.execute(
        "UPDATE session_environment_bindings
         SET active_target = ?, state = ?, error_message = ?, updated_at = ? WHERE session_id = ?",
        params![
            op.previous_target.as_str(),
            op.previous_state,
            error,
            now,
            op.session_id
        ],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn clean_stranded_bindings(db: &Connection, now: i64) -> Result<()> {
    db.execute(
        "UPDATE session_environment_bindings SET state = 'failed',
         error_message = 'environment-transition-interrupted', updated_at = ?
         WHERE state IN ('recovering', 'handoff') AND NOT EXISTS (
             SELECT 1 FROM session_environment_transitions transition
             WHERE transition.session_id = session_environment_bindings.session_id
         )",
        [now],
    )?;
    Ok(())
}

fn remove_transition(db: &Connection, op: &Transition) -> Result<bool> {
    Ok(db.execute(
        "DELETE FROM session_environment_transitions WHERE session_id = ? AND operation_id = ?",
        params![op.session_id, op.operation_id],
    )? == 1)
}

fn set_context(db: &Connection, session: &str, target: Target, now: i64) -> Result<()> {
    let (context, cwd): (String, String) = db.query_row(
        "SELECT context.id, context.cwd FROM session_environment_bindings binding
         LEFT JOIN worktrees worktree ON worktree.id = binding.managed_worktree_id
         JOIN execution_contexts context ON context.id = CASE WHEN ? = 'local'
             THEN binding.local_execution_context_id ELSE worktree.execution_context_id END
         WHERE binding.session_id = ? AND context.archived_at IS NULL",
        params![target.as_str(), session],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(db.execute(
        "UPDATE sessions SET execution_context_id = ?, cwd = ?, updated_at = ?, version = version + 1
         WHERE id = ? AND archived_at IS NULL",
        params![context, cwd, now, session],
    )? == 1, "session-unavailable");
    Ok(())
}
