//! Filesystem and Git checks for session execution targets.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use matou_native_core::git_repo::{ChangeCounts, Head, LocalBranch, Repository, WorktreeScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentKind {
    Local,
    Worktree,
}

#[derive(Debug, Clone, Copy)]
pub struct WorktreeExpectation<'a> {
    pub repository_root: &'a Path,
    pub path: &'a Path,
    pub head: &'a Head,
}

#[derive(Debug, Clone, Copy)]
pub enum EnvironmentTarget<'a> {
    Local(&'a Path),
    Worktree(WorktreeExpectation<'a>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct GitState {
    pub repository: Repository,
    pub head: Head,
    pub changes: ChangeCounts,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReadyEnvironment {
    pub kind: EnvironmentKind,
    pub cwd: PathBuf,
    pub git: Option<GitState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingEnvironment {
    PathMissing,
    NotListedByGit,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UnavailableEnvironment {
    NotDirectory,
    WrongRepository,
    WrongBranch,
    WrongHead,
    Io(String),
    Git(String),
    UnsafeRepair(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnvironmentProblem {
    Missing(MissingEnvironment),
    Unavailable(UnavailableEnvironment),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RepairReadiness {
    Ready(ReadyEnvironment),
    RepairableMoved { cwd: PathBuf },
}

pub fn probe_environment(
    target: EnvironmentTarget<'_>,
) -> Result<ReadyEnvironment, EnvironmentProblem> {
    inspect(target, true)
}

/// Checks a selected location before allowing Git to repair the registration.
/// The caller can apply the repair through the scope, then probe the target again.
pub fn check_repair(
    scope: &WorktreeScope,
    expectation: WorktreeExpectation<'_>,
    candidate: &Path,
) -> Result<RepairReadiness, EnvironmentProblem> {
    let target = EnvironmentTarget::Worktree(WorktreeExpectation {
        path: candidate,
        ..expectation
    });
    let ready = inspect(target, false)?;
    match scope.needs_moved_repair(expectation.path, candidate, expectation.head) {
        Ok(false) => Ok(RepairReadiness::Ready(ready)),
        Ok(true) => Ok(RepairReadiness::RepairableMoved { cwd: ready.cwd }),
        Err(error) => Err(unavailable(UnavailableEnvironment::UnsafeRepair(format!(
            "{error:#}"
        )))),
    }
}

fn inspect(
    target: EnvironmentTarget<'_>,
    require_registration: bool,
) -> Result<ReadyEnvironment, EnvironmentProblem> {
    let (path, kind) = match target {
        EnvironmentTarget::Local(path) => (path, EnvironmentKind::Local),
        EnvironmentTarget::Worktree(worktree) => (worktree.path, EnvironmentKind::Worktree),
    };
    let cwd = directory(path)?;
    let repository = Repository::discover(&cwd).map_err(git_error)?;

    if let EnvironmentTarget::Worktree(expected) = target {
        let source = match directory(expected.repository_root) {
            Ok(path) => Repository::discover(&path).map_err(git_error)?,
            Err(EnvironmentProblem::Missing(_))
            | Err(EnvironmentProblem::Unavailable(UnavailableEnvironment::NotDirectory)) => None,
            Err(error) => return Err(error),
        };
        let (Some(repository), Some(source)) = (&repository, source) else {
            return Err(unavailable(UnavailableEnvironment::WrongRepository));
        };
        if !repository.same_repository(&source)
            || !repository.is_linked_worktree()
            || repository.root() != cwd
        {
            return Err(unavailable(UnavailableEnvironment::WrongRepository));
        }
        if require_registration
            && !source
                .worktrees()
                .map_err(git_error)?
                .iter()
                .any(|entry| entry.path == cwd && !entry.main && !entry.bare)
        {
            return Err(EnvironmentProblem::Missing(
                MissingEnvironment::NotListedByGit,
            ));
        }
    }

    let git = if let Some(repository) = repository {
        let head = repository.head().map_err(git_error)?;
        if let EnvironmentTarget::Worktree(expected) = target
            && &head != expected.head
        {
            let reason = match (&head, expected.head) {
                (Head::Branch(_), Head::Branch(_)) => UnavailableEnvironment::WrongBranch,
                _ => UnavailableEnvironment::WrongHead,
            };
            return Err(unavailable(reason));
        }
        let changes = repository.changes().map_err(git_error)?;
        Some(GitState {
            repository,
            head,
            changes,
        })
    } else {
        None
    };
    Ok(ReadyEnvironment { kind, cwd, git })
}

fn directory(path: &Path) -> Result<PathBuf, EnvironmentProblem> {
    let canonical = fs::canonicalize(path).map_err(|error| match error.kind() {
        ErrorKind::NotFound => EnvironmentProblem::Missing(MissingEnvironment::PathMissing),
        ErrorKind::NotADirectory => unavailable(UnavailableEnvironment::NotDirectory),
        _ => unavailable(UnavailableEnvironment::Io(error.to_string())),
    })?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| unavailable(UnavailableEnvironment::Io(error.to_string())))?;
    if !metadata.is_dir() {
        return Err(unavailable(UnavailableEnvironment::NotDirectory));
    }
    Ok(canonical)
}

fn unavailable(reason: UnavailableEnvironment) -> EnvironmentProblem {
    EnvironmentProblem::Unavailable(reason)
}

fn git_error(error: anyhow::Error) -> EnvironmentProblem {
    unavailable(UnavailableEnvironment::Git(format!("{error:#}")))
}

pub struct SessionDirectory<'a> {
    pub workspace_id: &'a str,
    pub cwd: &'a Path,
    pub archived: bool,
}

/// The frozen session graph groups by saved cwd within a workspace.
pub fn shared_session_count(
    workspace_id: &str,
    cwd: &Path,
    sessions: &[SessionDirectory<'_>],
) -> usize {
    sessions
        .iter()
        .filter(|session| {
            !session.archived
                && session.workspace_id == workspace_id
                && session.cwd.as_os_str() == cwd.as_os_str()
        })
        .count()
        .max(1)
}

#[derive(Debug, PartialEq, Eq)]
pub enum CheckoutDisposition {
    CurrentBranch,
    CheckedOutElsewhere,
    Checkout,
    ConfirmShared {
        other_sessions: usize,
        uncommitted: usize,
    },
}

/// The count is supplied when the active session shares the saved local directory.
pub fn checkout_disposition(
    branch: &LocalBranch,
    kind: EnvironmentKind,
    owns_worktree: bool,
    shared_count: Option<usize>,
    changes: ChangeCounts,
) -> CheckoutDisposition {
    if branch.current {
        return CheckoutDisposition::CurrentBranch;
    }
    if branch.checked_out_path.is_some() {
        return CheckoutDisposition::CheckedOutElsewhere;
    }
    if kind == EnvironmentKind::Local
        && !owns_worktree
        && let Some(count) = shared_count.filter(|count| *count > 1)
    {
        return CheckoutDisposition::ConfirmShared {
            other_sessions: count - 1,
            uncommitted: changes.total(),
        };
    }
    CheckoutDisposition::Checkout
}
