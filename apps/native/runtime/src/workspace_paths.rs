//! Filesystem checks must not hold the SQLite/PTY owner or GPUI thread.
use crate::owner::ShellOwner;
use anyhow::{Result, anyhow};
use matou_native_core::workspace_paths::{WorkspacePathState, inspect};
use std::{
    collections::HashMap,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
};
#[derive(Default)]
pub struct WorkspacePathChecks {
    pending: HashMap<String, Receiver<(&'static str, i64)>>,
}
impl WorkspacePathChecks {
    /// Coalesce checks for a workspace exactly as the original path service does.
    pub fn request(&mut self, owner: &ShellOwner, workspace_id: &str) -> Result<()> {
        if self.pending.contains_key(workspace_id) {
            return Ok(());
        }
        let root = owner.workspace_path_target(workspace_id)?;
        let (send, receive) = mpsc::channel();
        thread::Builder::new()
            .name("workspace-path".into())
            .spawn(move || {
                let reason = inspect(std::path::Path::new(&root));
                let checked_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                let _ = send.send((reason, checked_at));
            })?;
        self.pending.insert(workspace_id.into(), receive);
        Ok(())
    }
    /// Return completed domain states to the caller for projection and pending launches.
    pub fn poll(&mut self, owner: &mut ShellOwner) -> Vec<Result<WorkspacePathState>> {
        let mut done = Vec::new();
        self.pending
            .retain(|id, receiver| match receiver.try_recv() {
                Ok((reason, checked_at)) => {
                    done.push(record_check(owner, id, reason, checked_at));
                    false
                }
                Err(TryRecvError::Empty) => true,
                Err(TryRecvError::Disconnected) => {
                    done.push(Err(anyhow!("Workspace {id} path check disconnected")));
                    false
                }
            });
        done
    }
}

fn record_check(
    owner: &mut ShellOwner,
    workspace_id: &str,
    reason: &str,
    checked_at: i64,
) -> Result<WorkspacePathState> {
    let status = if reason.is_empty() {
        "valid"
    } else {
        "invalid"
    };
    // An unchanged check still completes pending launches with the saved state.
    let generation = match owner.workspace_path_state(workspace_id)? {
        Some(state) if state.status == status && state.reason == reason => return Ok(state),
        Some(state) => state.validation_generation + 1,
        None => owner.last_workspace_validation_generation(workspace_id)? + 1,
    };
    let state = WorkspacePathState {
        workspace_id: workspace_id.into(),
        status: status.into(),
        reason: reason.into(),
        checked_at,
        validation_generation: generation,
    };
    owner.commit_workspace_path(&state)?;
    Ok(state)
}
