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

use matou_native_core::git_repo::worktree_has_head;
use matou_native_core::hierarchy::session_environment::{
    self, Binding, Kind, OwnedWorktree, Target, Transition,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum EnvironmentChangeResult {
    Ok {
        active_target: Target,
        cwd: String,
    },
    Rejected {
        reason: String,
    },
    SwitchSession {
        session_id: String,
        scene_id: Option<String>,
    },
}

impl EnvironmentChangeResult {
    pub(crate) fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
        }
    }
}

pub(crate) enum CheckedEnvironment {
    Ready(String),
    OtherSession(String),
}

pub(crate) fn check_change(
    kind: Kind,
    target: Target,
    id: &str,
    candidate: Option<&Path>,
    binding: &Binding,
    owned_worktrees: &[OwnedWorktree],
) -> anyhow::Result<CheckedEnvironment> {
    use anyhow::{Context, ensure};

    if target == Target::Local {
        let path = Path::new(&binding.local_path);
        directory(path).map_err(problem_reason)?;
        return Ok(CheckedEnvironment::Ready(binding.local_path.clone()));
    }

    let owned = owned_worktrees
        .iter()
        .find(|item| item.session_id == id)
        .context("no-owned-worktree")?;
    let original = Path::new(&owned.path);
    let head = managed_head(owned)?;
    let repository = Repository::discover(Path::new(&owned.repository_root))
        .ok()
        .flatten()
        .context("wrong-repository")?;
    let path = if kind == Kind::Locate {
        directory(candidate.context("path-missing")?).map_err(problem_reason)?
    } else {
        match directory(original) {
            Ok(path) => path,
            Err(EnvironmentProblem::Missing(MissingEnvironment::PathMissing))
                if kind == Kind::Restore =>
            {
                original.to_owned()
            }
            Err(error) => return Err(problem_reason(error)),
        }
    };

    // A stored path or the original Git registration also identifies an owner
    // when the checkout has been moved or selected through a symlink.
    let checkout = Repository::discover(&path)
        .ok()
        .flatten()
        .filter(|repo| repo.is_linked_worktree() && repo.root() == path);
    let registration = checkout
        .as_ref()
        .and_then(|repo| repo.linked_worktree_registration().ok());
    if let Some(owner) = owned_worktrees.iter().find(|owner| {
        if owner.session_id == id {
            return false;
        }
        let stored = Path::new(&owner.path);
        stored == path
            || fs::canonicalize(stored).is_ok_and(|stored| stored == path)
            || registration.as_ref().is_some_and(|registered| {
                // A moved checkout can retain a registration under a symlinked parent.
                Repository::registered_worktree_path(stored)
                    .is_ok_and(|stored| stored == *registered)
            })
    }) {
        return if kind == Kind::Locate {
            Ok(CheckedEnvironment::OtherSession(owner.session_id.clone()))
        } else {
            Err(anyhow::anyhow!("path-owned-by-another-session"))
        };
    }

    if kind == Kind::Locate
        && registration.as_deref() == Some(path.as_path())
        && let Some(checkout) = &checkout
    {
        let candidate_head = checkout.head()?;
        let mut owners = owned_worktrees.iter().filter(|owner| {
            let stored = Path::new(&owner.path);
            !stored.exists()
                && managed_head(owner).is_ok_and(|head| head == candidate_head)
                && Repository::discover(Path::new(&owner.repository_root))
                    .ok()
                    .flatten()
                    .is_some_and(|origin| origin.same_repository(checkout))
        });
        if let Some(owner) = owners.next() {
            if owners.next().is_some() {
                anyhow::bail!("path-owned-by-another-session");
            }
            if owner.session_id != id {
                // A detached HEAD alone never establishes unique ownership.
                let entries = checkout.worktrees()?;
                ensure!(
                    entries
                        .iter()
                        .filter(|entry| !entry.main
                            && !entry.bare
                            && worktree_has_head(entry, &candidate_head))
                        .count()
                        == 1
                        && !entries
                            .iter()
                            .any(|entry| entry.path == Path::new(&owner.path)),
                    "path-owned-by-another-session"
                );
                return Ok(CheckedEnvironment::OtherSession(owner.session_id.clone()));
            }
        }
    }

    let expected = WorktreeExpectation {
        repository_root: repository.root(),
        path: &path,
        head: &head,
    };
    if kind == Kind::Restore && !path.is_dir() {
        repository.restore_managed_worktree(
            original,
            &head,
            owned.base_revision.as_deref().or(owned.base_ref.as_deref()),
        )?;
    } else if kind != Kind::Handoff {
        let ready =
            inspect(EnvironmentTarget::Worktree(expected), false).map_err(problem_reason)?;
        let registration = registration.context("not-listed-by-git")?;
        let original = Repository::registered_worktree_path(original)?;
        if registration != original {
            // After a Git move, or a crash after repair, the original Git
            // registration is gone. Only a unique identity can be adopted.
            let entries = repository.worktrees()?;
            ensure!(
                kind == Kind::Locate
                    && registration == path
                    && !original.exists()
                    && !entries.iter().any(|entry| entry.path == original)
                    && entries
                        .iter()
                        .filter(|entry| !entry.main
                            && !entry.bare
                            && worktree_has_head(entry, &head))
                        .count()
                        == 1,
                "not-listed-by-git"
            );
        } else if ready.cwd != original
            || !repository
                .worktrees()?
                .iter()
                .any(|entry| entry.path == path)
        {
            repository.repair_managed_worktree(&original, &path, &head)?;
        }
    }
    let ready = probe_environment(EnvironmentTarget::Worktree(expected)).map_err(problem_reason)?;
    let path = if kind == Kind::Locate {
        ready.cwd.to_str().context("invalid-path")?.to_owned()
    } else {
        owned.path.clone()
    };
    Ok(CheckedEnvironment::Ready(path))
}

fn managed_head(owned: &OwnedWorktree) -> anyhow::Result<Head> {
    if owned.branch == "(detached)" {
        return owned
            .base_revision
            .clone()
            .map(Head::Detached)
            .ok_or_else(|| anyhow::anyhow!("wrong-head"));
    }
    Ok(Head::Branch(owned.branch.clone()))
}

fn problem_reason(problem: EnvironmentProblem) -> anyhow::Error {
    let reason = match problem {
        EnvironmentProblem::Missing(MissingEnvironment::PathMissing) => "path-missing",
        EnvironmentProblem::Missing(MissingEnvironment::NotListedByGit) => "not-listed-by-git",
        EnvironmentProblem::Unavailable(UnavailableEnvironment::NotDirectory) => "not-worktree",
        EnvironmentProblem::Unavailable(UnavailableEnvironment::WrongRepository) => {
            "wrong-repository"
        }
        EnvironmentProblem::Unavailable(UnavailableEnvironment::WrongBranch) => "wrong-branch",
        EnvironmentProblem::Unavailable(UnavailableEnvironment::WrongHead) => "wrong-head",
        EnvironmentProblem::Unavailable(
            UnavailableEnvironment::Io(error)
            | UnavailableEnvironment::Git(error)
            | UnavailableEnvironment::UnsafeRepair(error),
        ) => {
            return anyhow::anyhow!("{error}");
        }
    };
    anyhow::anyhow!("{reason}")
}

pub(crate) fn change_failure(kind: Kind, error: &anyhow::Error) -> String {
    let reason = error.to_string();
    if matches!(
        reason.as_str(),
        "path-missing"
            | "not-worktree"
            | "wrong-repository"
            | "not-listed-by-git"
            | "wrong-branch"
            | "wrong-head"
            | "path-owned-by-another-session"
            | "no-owned-worktree"
            | "invalid-path"
            | "session-unavailable"
            | "environment-transition-in-progress"
    ) {
        reason
    } else {
        format!("{}-failed:{error:#}", kind.as_str())
    }
}

pub(crate) fn reconcile_transitions(db: &mut rusqlite::Connection, now: i64) -> anyhow::Result<()> {
    for operation in session_environment::list_transitions(db)? {
        let result = reconcile_transition(db, &operation, now);
        if let Err(error) = result {
            session_environment::rollback_transition(
                db,
                &operation,
                false,
                &error.to_string(),
                now,
            )?;
        }
    }
    session_environment::clean_stranded_bindings(db, now)
}

fn reconcile_transition(
    db: &mut rusqlite::Connection,
    op: &Transition,
    now: i64,
) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    ensure!(op.kind != Kind::Handoff, "handoff-interrupted");
    let binding =
        session_environment::binding(db, &op.session_id)?.context("session-unavailable")?;
    let owned = session_environment::owned_worktrees(db)?;
    let CheckedEnvironment::Ready(path) = check_change(
        op.kind,
        op.target,
        &op.session_id,
        op.candidate.as_deref().map(Path::new),
        &binding,
        &owned,
    )?
    else {
        anyhow::bail!("path-owned-by-another-session");
    };
    session_environment::mark_transition_external_ready(db, op, now)?;
    session_environment::complete_transition(db, op, &path, now)
}
