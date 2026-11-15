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
    generations: HashMap<String, i64>,
    pending: HashMap<String, Receiver<WorkspacePathState>>,
}
impl WorkspacePathChecks {
    /// Coalesce checks for a workspace exactly as the original path service does.
    pub fn request(&mut self, owner: &ShellOwner, workspace_id: &str) -> Result<()> {
        if self.pending.contains_key(workspace_id) {
            return Ok(());
        }
        let root = owner.workspace_path_target(workspace_id)?;
        let stored = owner
            .workspace_path_state(workspace_id)?
            .map_or(0, |s| s.validation_generation);
        let generation = stored.max(*self.generations.get(workspace_id).unwrap_or(&0)) + 1;
        let id = workspace_id.to_owned();
        let (send, receive) = mpsc::channel();
        thread::Builder::new()
            .name("workspace-path".into())
            .spawn(move || {
                let reason = inspect(std::path::Path::new(&root));
                let state = WorkspacePathState {
                    workspace_id: id,
                    status: if reason.is_empty() {
                        "valid"
                    } else {
                        "invalid"
                    }
                    .into(),
                    reason: reason.into(),
                    checked_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64,
                    validation_generation: generation,
                };
                let _ = send.send(state);
            })?;
        self.generations.insert(workspace_id.into(), generation);
        self.pending.insert(workspace_id.into(), receive);
        Ok(())
    }
    /// Return completed domain states to the caller for projection and pending launches.
    pub fn poll(&mut self, owner: &mut ShellOwner) -> Vec<Result<WorkspacePathState>> {
        let mut done = Vec::new();
        self.pending
            .retain(|id, receiver| match receiver.try_recv() {
                Ok(state) => {
                    done.push(owner.commit_workspace_path(&state).map(|()| state));
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
