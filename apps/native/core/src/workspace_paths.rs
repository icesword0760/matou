//! Frozen path-state persistence; filesystem inspection runs outside the domain owner.
use crate::storage::{CommandMetadata, DomainCommit, DomainEventInput, execute};
use anyhow::{Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePathState {
    pub workspace_id: String,
    pub status: String,
    pub reason: String,
    pub checked_at: i64,
    pub validation_generation: i64,
}
fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspacePathState> {
    Ok(WorkspacePathState {
        workspace_id: row.get(0)?,
        status: row.get(1)?,
        reason: row.get(2)?,
        checked_at: row.get(3)?,
        validation_generation: row.get(4)?,
    })
}
pub fn get(connection: &Connection, workspace_id: &str) -> Result<Option<WorkspacePathState>> {
    Ok(connection.query_row("SELECT workspace_id,status,reason,checked_at,validation_generation FROM workspace_path_state WHERE workspace_id=?", [workspace_id], from_row).optional()?)
}
pub fn list(connection: &Connection) -> Result<Vec<WorkspacePathState>> {
    let mut q=connection.prepare("SELECT workspace_id,status,reason,checked_at,validation_generation FROM workspace_path_state ORDER BY workspace_id")?;
    Ok(q.query_map([], from_row)?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn target(connection: &Connection, workspace_id: &str) -> Result<String> {
    connection
        .query_row(
            "SELECT root_directory FROM workspaces WHERE id=? AND archived_at IS NULL",
            [workspace_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow!("Workspace {workspace_id} does not exist"))
}
pub fn poll_targets(connection: &Connection) -> Result<Vec<String>> {
    let mut q=connection.prepare("SELECT DISTINCT workspace_id FROM (SELECT active_workspace_id AS workspace_id FROM window_navigation WHERE active_workspace_id IS NOT NULL UNION SELECT workspace_id FROM workspace_path_state WHERE status='invalid') ORDER BY workspace_id")?;
    Ok(q.query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn session_workspace(connection: &Connection, session_id: &str) -> Result<Option<String>> {
    Ok(connection.query_row("SELECT tasks.workspace_id FROM sessions JOIN tasks ON tasks.id=sessions.task_id WHERE sessions.id=? AND sessions.archived_at IS NULL",[session_id],|r|r.get(0)).optional()?)
}
pub fn commit(
    connection: &mut Connection,
    command_id: String,
    state: &WorkspacePathState,
    committed_at: i64,
) -> Result<DomainCommit> {
    let previous = get(connection, &state.workspace_id)?;
    execute(
        connection,
        &CommandMetadata {
            command_id,
            command_type: "workspace.validate-path".into(),
            request_hash: format!(
                "{}:{}:{}:{}",
                state.workspace_id, state.validation_generation, state.status, state.reason
            ),
            causation_id: None,
            correlation_id: None,
        },
        committed_at,
        |tx, events| {
            tx.execute("INSERT INTO workspace_path_state (workspace_id,status,reason,checked_at,validation_generation) VALUES (?,?,?,?,?) ON CONFLICT(workspace_id) DO UPDATE SET status=excluded.status,reason=excluded.reason,checked_at=excluded.checked_at,validation_generation=excluded.validation_generation WHERE workspace_path_state.validation_generation < excluded.validation_generation",params![state.workspace_id,state.status,state.reason,state.checked_at,state.validation_generation])?;
            if previous
                .as_ref()
                .is_none_or(|p| p.status != state.status || p.reason != state.reason)
            {
                events.push(DomainEventInput {
                    event_id: format!(
                        "workspace-path:{}:{}",
                        state.workspace_id, state.validation_generation
                    ),
                    event_type: "workspace.path-status-changed".into(),
                    aggregate_type: "workspace".into(),
                    aggregate_id: state.workspace_id.clone(),
                    workspace_id: Some(state.workspace_id.clone()),
                    task_id: None,
                    session_id: None,
                    payload: json!(state),
                    schema_version: None,
                    required_terminal_sequence: None,
                    occurred_at: state.checked_at,
                });
            }
            Ok(json!(state))
        },
    )
}
/// Mirrors stat + access(R_OK|X_OK), following symlinks without rewriting stored paths.
pub fn inspect(path: &Path) -> &'static str {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return if e.kind() == std::io::ErrorKind::NotFound {
                "missing"
            } else {
                "unknown"
            };
        }
    };
    if !metadata.is_dir() {
        return "not-directory";
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return "no-access";
        };
        if unsafe { libc::access(path.as_ptr(), libc::R_OK | libc::X_OK) } != 0 {
            return "no-access";
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
        };
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // libuv fs__access checks attributes; R_OK/X_OK require no ACL probe on Windows.
        if unsafe { GetFileAttributesW(path.as_ptr()) } == INVALID_FILE_ATTRIBUTES {
            return "no-access";
        }
    }
    ""
}
