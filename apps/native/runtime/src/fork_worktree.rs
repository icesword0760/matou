//! Reserve a new branch, create its checkout, and bind it under the Fork lease.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, anyhow, ensure};
use matou_native_core::git_repo::{
    BranchReservation, Head, Removal, Repository, WorktreeBranch, WorktreeScope,
    check_branch_reservations,
};
use serde::Serialize;
use uuid::Uuid;

use crate::environment_runtime::{EnvironmentTarget, WorktreeExpectation, probe_environment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkIdentity {
    pub operation_id: String,
    pub submission_key: String,
    pub source_session_id: String,
    pub session_id: String,
}

impl ForkIdentity {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            [
                &self.operation_id,
                &self.submission_key,
                &self.source_session_id,
                &self.session_id,
            ]
            .iter()
            .all(|id| !id.trim().is_empty()),
            "Fork identity is incomplete"
        );
        ensure!(
            self.source_session_id != self.session_id,
            "Fork child must differ from its source session"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkLease {
    pub operation_id: String,
    pub token: Uuid,
    pub fence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForkStage {
    Queued,
    CreatingWorktree,
    ApplyingSetup,
    BindingSession,
    RestoringProvider,
    StartingWindow,
    Succeeded,
    Failed,
}

pub struct NewWorktree {
    pub isolation_root: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub worktree_id: String,
    pub execution_context_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeBinding {
    pub identity: ForkIdentity,
    pub worktree_id: String,
    pub execution_context_id: String,
    pub repository_root: PathBuf,
    pub cwd: PathBuf,
    pub branch: String,
    pub base_revision: String,
}

pub struct ForkWorktreePlan {
    scope: WorktreeScope,
    repository: Repository,
    binding: WorktreeBinding,
}

struct OwnedCheckout {
    repository: Repository,
    marker: File,
}

impl OwnedCheckout {
    fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            marker: File::open(path.join(".git"))?,
            repository: Repository::discover(path)?.context("Fork checkout is missing")?,
        })
    }

    fn verify(&self, path: &Path, current: &Repository) -> Result<()> {
        let path = path.join(".git");
        ensure!(
            *current == self.repository
                && fs::symlink_metadata(&path)?.is_file()
                && same_file(&self.marker, &path)?,
            "Fork checkout identity changed"
        );
        Ok(())
    }
}

#[cfg(unix)]
fn same_file(original: &File, path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let original = original.metadata()?;
    let found = fs::symlink_metadata(path)?;
    Ok((original.dev(), original.ino()) == (found.dev(), found.ino()))
}

#[cfg(windows)]
fn same_file(original: &File, path: &Path) -> Result<bool> {
    use std::{ffi::c_void, io, mem::size_of, os::windows::io::AsRawHandle};

    #[derive(Default, PartialEq, Eq)]
    #[repr(C)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut c_void,
            class: i32,
            info: *mut FileIdInfo,
            size: u32,
        ) -> i32;
    }

    fn file_id(file: &File) -> io::Result<FileIdInfo> {
        const FILE_ID_INFO: i32 = 18;
        let mut info = FileIdInfo::default();
        // The handle is live and the output has the FILE_ID_INFO layout.
        let status = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FILE_ID_INFO,
                &mut info,
                size_of::<FileIdInfo>() as u32,
            )
        };
        if status == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(info)
        }
    }

    Ok(file_id(original)? == file_id(&File::open(path)?)?)
}

impl ForkWorktreePlan {
    /// The caller supplies reservations from its acceptance transaction. Use the
    /// primary checkout's canonical path as the key for the shared Git refs.
    pub fn reserve(
        identity: ForkIdentity,
        source_cwd: &Path,
        target: NewWorktree,
        reservations: &[BranchReservation<'_>],
    ) -> Result<Self> {
        identity.validate()?;
        ensure!(
            target.path.is_absolute()
                && !target.worktree_id.trim().is_empty()
                && !target.execution_context_id.trim().is_empty(),
            "Invalid worktree target"
        );
        let repository =
            Repository::discover(source_cwd)?.context("Fork requires a Git repository")?;
        let scope = WorktreeScope::new(&target.isolation_root, &repository)?;
        let repository_root = repository
            .worktrees()?
            .into_iter()
            .find(|entry| entry.main)
            .context("Git repository has no primary worktree")?
            .path;
        repository.check_fork_branch(&target.branch)?;
        check_branch_reservations(
            &repository_root,
            &target.branch,
            &identity.operation_id,
            &identity.submission_key,
            reservations,
        )?;
        let base_revision = repository.resolve_commit("HEAD")?;
        Ok(Self {
            scope,
            repository,
            binding: WorktreeBinding {
                identity,
                worktree_id: target.worktree_id,
                execution_context_id: target.execution_context_id,
                repository_root,
                cwd: target.path,
                branch: target.branch,
                base_revision,
            },
        })
    }

    pub(crate) fn conflicts_with(&self, other: &Self) -> bool {
        self.repository.same_repository(&other.repository)
            && self.binding.branch == other.binding.branch
    }

    /// Checkpoints renew/verify the caller's lease. The bind callback must
    /// commit the child binding and the transition to restoring-provider
    /// atomically under that lease, or leave both unchanged on failure.
    pub fn execute(
        &self,
        lease: &ForkLease,
        mut checkpoint: impl FnMut(ForkStage, &ForkLease) -> Result<()>,
        setup: impl FnOnce(&WorktreeBinding, &ForkLease) -> Result<()>,
        bind: impl FnOnce(&WorktreeBinding, &ForkLease) -> Result<()>,
    ) -> Result<WorktreeBinding> {
        ensure!(
            lease.operation_id == self.binding.identity.operation_id,
            "Stale Fork lease"
        );
        checkpoint(ForkStage::CreatingWorktree, lease)?;
        let worktree = self.scope.create(
            &self.binding.cwd,
            WorktreeBranch::New {
                name: &self.binding.branch,
                base_ref: &self.binding.base_revision,
            },
        )?;
        let binding = WorktreeBinding {
            cwd: worktree.path,
            ..self.binding.clone()
        };
        // Hold the marker open so a checkout recreated in the same directory
        // with the same Git administrative name cannot pass the ownership check.
        let checkout = OwnedCheckout::open(&binding.cwd);
        let result = (|| {
            let checkout = checkout
                .as_ref()
                .map_err(|error| anyhow!("Fork checkout identity is unavailable: {error:#}"))?;
            checkpoint(ForkStage::ApplyingSetup, lease)?;
            setup(&binding, lease)?;
            checkout.verify(&binding.cwd, &binding.verify()?)?;
            checkpoint(ForkStage::BindingSession, lease)?;
            bind(&binding, lease)
        })();
        if let Err(error) = result {
            return match self.rollback(&binding, checkout.as_ref().ok()) {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("Fork rollback incomplete: {cleanup:#}")))
                }
            };
        }
        Ok(binding)
    }

    fn rollback(&self, binding: &WorktreeBinding, owned: Option<&OwnedCheckout>) -> Result<()> {
        let owned = owned.context("Fork checkout ownership is unavailable")?;
        let checkout = Repository::discover(&binding.cwd)?.context("Fork checkout is missing")?;
        owned.verify(&binding.cwd, &checkout)?;
        let reference = format!("refs/heads/{}", binding.branch);
        ensure!(
            checkout.head()? == Head::Branch(binding.branch.clone())
                && checkout.resolve_commit("HEAD")? == binding.base_revision,
            "Fork checkout or branch moved"
        );
        ensure!(
            git_output(
                &binding.cwd,
                &[
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "-z"
                ]
            )?
            .stdout
            .is_empty(),
            "Fork worktree contains ignored files"
        );
        ensure!(
            self.scope.remove(&binding.cwd)? == Removal::Removed,
            "Fork worktree contains local changes"
        );
        ensure!(
            Repository::discover(self.repository.root())?.as_ref() == Some(&self.repository)
                && !self
                    .repository
                    .worktrees()?
                    .iter()
                    .any(|entry| entry.branch.as_deref() == Some(binding.branch.as_str())),
            "Fork branch is now in use or repository identity changed"
        );
        // Compare-and-delete keeps an externally moved ref intact.
        git_output(
            self.repository.root(),
            &[
                "update-ref",
                "--no-deref",
                "-d",
                &reference,
                &binding.base_revision,
            ],
        )?;
        Ok(())
    }
}

impl WorktreeBinding {
    pub(crate) fn verify(&self) -> Result<Repository> {
        let head = Head::Branch(self.branch.clone());
        let ready = probe_environment(EnvironmentTarget::Worktree(WorktreeExpectation {
            repository_root: &self.repository_root,
            path: &self.cwd,
            head: &head,
        }))
        .map_err(|problem| anyhow!("Fork worktree unavailable: {problem:?}"))?;
        Ok(ready
            .git
            .context("Fork Git state is unavailable")?
            .repository)
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<Output> {
    let hooks = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut command = Command::new("git");
    command
        .current_dir(fs::canonicalize(cwd)?)
        .args([
            "-c",
            &format!("core.hooksPath={hooks}"),
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args);
    for (key, _) in std::env::vars_os().filter(|(key, _)| {
        key.to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
    }) {
        command.env_remove(key);
    }
    let output = command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .output()
        .context("Run Git during Fork rollback")?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::ForkStage;

    #[test]
    fn fork_worktree_stages_match_the_progress_contract() {
        for (stage, name) in [
            (ForkStage::Queued, "queued"),
            (ForkStage::CreatingWorktree, "creating-worktree"),
            (ForkStage::ApplyingSetup, "applying-setup"),
            (ForkStage::BindingSession, "binding-session"),
            (ForkStage::RestoringProvider, "restoring-provider"),
            (ForkStage::StartingWindow, "starting-window"),
            (ForkStage::Succeeded, "succeeded"),
            (ForkStage::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_value(stage).unwrap(), name);
        }
    }
}
