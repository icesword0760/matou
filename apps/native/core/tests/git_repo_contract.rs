use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use matou_native_core::git_repo::{
    ChangeCounts, ForkBranchError, Head, Removal, Repository, WorktreeBranch, WorktreeScope,
    current_branch,
};
use tempfile::TempDir;

fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", path.join("no-global-config"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init(path: &Path) {
    fs::create_dir(path).unwrap();
    git(path, &["init", "--initial-branch=main"]);
    git(path, &["config", "user.name", "Fixture"]);
    git(path, &["config", "user.email", "fixture@example.invalid"]);
}

fn fixture() -> (TempDir, PathBuf, Repository) {
    let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let path = dir.path().join("仓库 with spaces");
    init(&path);
    fs::write(path.join("tracked.txt"), "initial\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "initial"]);
    let repo = Repository::discover(&path).unwrap().unwrap();
    (dir, path, repo)
}

#[test]
fn discovers_directory_root_unborn_branch_and_detached_head() {
    let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let path = dir.path().join("empty");
    init(&path);
    fs::create_dir(path.join("子 目录")).unwrap();
    let repo = Repository::discover(&path.join("子 目录"))
        .unwrap()
        .unwrap();
    assert_eq!(repo.root(), path.canonicalize().unwrap());
    assert_eq!(repo.head().unwrap(), Head::Branch("main".into()));
    assert_eq!(
        current_branch(&path.join("子 目录")).as_deref(),
        Some("main")
    );
    assert!(repo.local_branches().unwrap().branches.is_empty());

    git(&path, &["commit", "--allow-empty", "-m", "initial"]);
    let revision = repo.resolve_commit("HEAD").unwrap();
    git(&path, &["checkout", "--detach", "HEAD"]);
    assert_eq!(repo.head().unwrap(), Head::Detached(revision.clone()));
    assert_eq!(current_branch(&path), None);
    let main = &repo.worktrees().unwrap()[0];
    assert!(main.main && main.detached);
    assert_eq!(main.revision.as_deref(), Some(revision.as_str()));
    assert_eq!(main.branch, None);
}

#[test]
fn discovers_non_git_directory() {
    // The temporary directories live inside this project checkout. Prevent Git
    // from walking into the enclosing checkout for this non-repository fixture.
    let ceiling = fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
    if std::env::var_os("GIT_CEILING_DIRECTORIES").as_deref() != Some(ceiling.as_os_str()) {
        let status = Command::new(std::env::current_exe().unwrap())
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .args(["--exact", "discovers_non_git_directory"])
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    assert!(Repository::discover(dir.path()).unwrap().is_none());
    assert_eq!(current_branch(dir.path()), None);
}

#[test]
fn local_branch_checks_reject_conflicts_and_expanded_shorthand() {
    let (_dir, path, repo) = fixture();
    git(&path, &["branch", "feature/review"]);
    git(&path, &["branch", "feature/review-2"]);
    assert!(matches!(
        repo.check_fork_branch("feature/review"),
        Err(ForkBranchError::LocalConflict)
    ));
    assert!(repo.check_fork_branch("feature/review-3").is_ok());
    assert!(repo.check_fork_branch("中文/功能").is_ok());
    git(&path, &["checkout", "feature/review"]);
    git(&path, &["checkout", "main"]);
    for branch in [
        "feature//review",
        "@{-1}",
        "-x",
        " HEAD ",
        "HEAD",
        "a.lock",
        "a..b",
        "a~b",
    ] {
        assert!(
            matches!(
                repo.check_fork_branch(branch),
                Err(ForkBranchError::Invalid)
            ),
            "{branch}"
        );
    }
    assert!(repo.check_fork_branch("feature/review-3").is_ok());
    assert!(
        repo.local_branches()
            .unwrap()
            .branches
            .iter()
            .all(|branch| branch.name != "feature/review-3")
    );
}

#[test]
fn creates_lists_and_removes_scoped_worktree_without_losing_dirty_files_or_branch() {
    let (dir, path, repo) = fixture();
    let scope = WorktreeScope::new(dir.path(), &repo).unwrap();
    let target = dir.path().join("独立 worktree");
    let worktree = scope
        .create(
            &target,
            WorktreeBranch::New {
                name: "feature/review",
                base_ref: "HEAD",
            },
        )
        .unwrap();
    assert_eq!(worktree.path, target.canonicalize().unwrap());
    assert_eq!(worktree.branch.as_deref(), Some("feature/review"));
    assert!(!worktree.main && !worktree.bare);
    assert_eq!(
        worktree.revision.as_deref(),
        Some(repo.resolve_commit("HEAD").unwrap().as_str())
    );

    let child = target.join("子 folder");
    fs::create_dir(&child).unwrap();
    assert_eq!(current_branch(&child).as_deref(), Some("feature/review"));
    let linked = Repository::discover(&child).unwrap().unwrap();
    assert!(repo.same_repository(&linked) && linked.is_linked_worktree());
    assert_eq!(
        repo.worktree_for_path(&child).unwrap(),
        Some(worktree.clone())
    );
    let branches = repo.local_branches().unwrap();
    assert_eq!(branches.default.as_deref(), Some("main"));
    assert_eq!(
        branches.branches[1].checked_out_path.as_ref(),
        Some(&worktree.path)
    );
    assert!(
        scope
            .create(
                &dir.path().join("blocked"),
                WorktreeBranch::Existing("feature/review")
            )
            .is_err()
    );

    fs::write(target.join("tracked.txt"), "staged\n").unwrap();
    git(&target, &["add", "tracked.txt"]);
    fs::write(target.join("tracked.txt"), "unstaged\n").unwrap();
    fs::write(target.join("notes.txt"), "untracked\n").unwrap();
    assert_eq!(
        linked.changes().unwrap(),
        ChangeCounts {
            staged: 1,
            unstaged: 1,
            untracked: 1
        }
    );
    assert_eq!(scope.remove(&target).unwrap(), Removal::RetainedDirty);
    assert_eq!(
        fs::read_to_string(target.join("notes.txt")).unwrap(),
        "untracked\n"
    );
    git(&target, &["reset", "--hard", "HEAD"]);
    fs::remove_file(target.join("notes.txt")).unwrap();
    assert_eq!(scope.remove(&target).unwrap(), Removal::Removed);
    assert!(!target.exists());
    assert_eq!(repo.worktrees().unwrap().len(), 1);
    assert_eq!(repo.head().unwrap(), Head::Branch("main".into()));
    assert_eq!(
        fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "initial\n"
    );
    assert!(matches!(
        repo.check_fork_branch("feature/review"),
        Err(ForkBranchError::LocalConflict)
    ));
    scope
        .create(&target, WorktreeBranch::Existing(" feature/review "))
        .unwrap();
    assert_eq!(scope.remove(&target).unwrap(), Removal::Removed);
}

#[test]
fn failed_checkout_rolls_back_new_branch_and_path_but_preserves_an_existing_branch() {
    let (dir, path, repo) = fixture();
    fs::write(path.join(".gitattributes"), "tracked.txt filter=fixture\n").unwrap();
    git(&path, &["add", ".gitattributes"]);
    git(&path, &["commit", "-m", "configure a filter"]);
    git(&path, &["config", "filter.fixture.smudge", "false"]);
    git(&path, &["config", "filter.fixture.required", "true"]);
    git(&path, &["branch", "existing"]);
    let scope = WorktreeScope::new(dir.path(), &repo).unwrap();
    let target = dir.path().join("rollback");

    for branch in [
        WorktreeBranch::New {
            name: "new-branch",
            base_ref: "HEAD",
        },
        WorktreeBranch::Existing("existing"),
    ] {
        let error = scope.create(&target, branch).unwrap_err();
        assert!(
            format!("{error:#}").contains("smudge filter fixture failed"),
            "{error:#}"
        );
        assert!(!target.exists());
        assert_eq!(repo.worktrees().unwrap().len(), 1);
        assert!(repo.check_fork_branch("new-branch").is_ok());
        assert!(matches!(
            repo.check_fork_branch("existing"),
            Err(ForkBranchError::LocalConflict)
        ));
    }

    git(&path, &["config", "--remove-section", "filter.fixture"]);
    scope
        .create(
            &target,
            WorktreeBranch::New {
                name: "new-branch",
                base_ref: "",
            },
        )
        .unwrap();
    assert_eq!(scope.remove(&target).unwrap(), Removal::Removed);
}

#[test]
fn refuses_outside_nested_and_preexisting_paths_and_replaced_worktrees() {
    let (dir, path, repo) = fixture();
    let outside = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    assert!(WorktreeScope::new(outside.path(), &repo).is_err());
    let scope = WorktreeScope::new(dir.path(), &repo).unwrap();
    let occupied = dir.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("keep"), "untouched").unwrap();
    for target in [
        outside.path().join("escape"),
        path.join("nested"),
        occupied.clone(),
        dir.path().to_path_buf(),
    ] {
        assert!(
            scope
                .create(
                    &target,
                    WorktreeBranch::New {
                        name: "safe",
                        base_ref: "HEAD"
                    }
                )
                .is_err()
        );
        assert!(repo.check_fork_branch("safe").is_ok());
    }
    assert!(scope.remove(&path).is_err());
    #[cfg(unix)]
    {
        let alias = dir.path().join("outside-link");
        std::os::unix::fs::symlink(outside.path(), &alias).unwrap();
        assert!(
            scope
                .create(
                    &alias.join("escape"),
                    WorktreeBranch::New {
                        name: "safe",
                        base_ref: "HEAD"
                    }
                )
                .is_err()
        );
    }
    let target = dir.path().join("replacement");
    scope
        .create(
            &target,
            WorktreeBranch::New {
                name: "safe",
                base_ref: "HEAD",
            },
        )
        .unwrap();
    fs::rename(&target, dir.path().join("moved")).unwrap();
    init(&target);
    fs::write(target.join("keep"), "other repository").unwrap();
    assert!(scope.remove(&target).is_err());
    assert_eq!(
        fs::read_to_string(target.join("keep")).unwrap(),
        "other repository"
    );
    assert_eq!(
        fs::read_to_string(occupied.join("keep")).unwrap(),
        "untouched"
    );
    assert!(!outside.path().join("escape").exists());
}

#[test]
#[cfg(unix)]
fn porcelain_keeps_newline_paths_and_missing_removal_is_exact() {
    let (dir, _, repo) = fixture();
    let scope = WorktreeScope::new(dir.path(), &repo).unwrap();
    let missing = dir.path().join("带空格\n第一项");
    let other = dir.path().join("第二项\n");
    for (target, name) in [(&missing, "first"), (&other, "second")] {
        let entry = scope
            .create(
                target,
                WorktreeBranch::New {
                    name,
                    base_ref: "HEAD",
                },
            )
            .unwrap();
        assert_eq!(entry.path, target.canonicalize().unwrap());
        fs::remove_dir_all(target).unwrap();
    }
    let worktrees = repo.worktrees().unwrap();
    assert!(
        worktrees
            .iter()
            .any(|entry| entry.path.ends_with("带空格\n第一项") && entry.prunable)
    );
    assert_eq!(scope.remove(&missing).unwrap(), Removal::Removed);
    assert!(
        repo.worktrees()
            .unwrap()
            .iter()
            .any(|entry| entry.path.ends_with("第二项\n") && entry.prunable)
    );
}

#[test]
fn concurrent_creates_keep_the_winning_branch_and_checkout() {
    let (dir, _, repo) = fixture();
    let scope = WorktreeScope::new(dir.path(), &repo).unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    let add = |path: &Path| {
        scope.create(
            path,
            WorktreeBranch::New {
                name: "parallel",
                base_ref: "HEAD",
            },
        )
    };
    let (a, b) = std::thread::scope(|thread| {
        let a = thread.spawn(|| add(&first));
        let b = add(&second);
        (a.join().unwrap(), b)
    });
    assert_ne!(a.is_ok(), b.is_ok());
    let winner = a.or(b).unwrap();
    assert_eq!(repo.worktrees().unwrap().len(), 2);
    assert_eq!(
        usize::from(first.exists()) + usize::from(second.exists()),
        1
    );
    assert!(matches!(
        repo.check_fork_branch("parallel"),
        Err(ForkBranchError::LocalConflict)
    ));
    assert_eq!(scope.remove(&winner.path).unwrap(), Removal::Removed);
}

#[test]
fn removal_refuses_a_checkout_pointing_to_another_linked_worktree() {
    let (dir, _, repo) = fixture();
    let scope = WorktreeScope::new(dir.path(), &repo).unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    for (path, name) in [(&first, "first"), (&second, "second")] {
        scope
            .create(
                path,
                WorktreeBranch::New {
                    name,
                    base_ref: "HEAD",
                },
            )
            .unwrap();
    }
    fs::copy(second.join(".git"), first.join(".git")).unwrap();
    assert!(scope.remove(&first).is_err());
    assert_eq!(repo.worktrees().unwrap().len(), 3);
    assert!(first.join("tracked.txt").is_file());
    assert!(second.join("tracked.txt").is_file());
}

#[test]
fn removal_retains_an_ignored_nested_registered_worktree() {
    let (dir, path, repo) = fixture();
    let scope = WorktreeScope::new(dir.path(), &repo).unwrap();
    let parent = dir.path().join("parent");
    scope
        .create(
            &parent,
            WorktreeBranch::New {
                name: "parent",
                base_ref: "HEAD",
            },
        )
        .unwrap();
    fs::write(path.join(".git/info/exclude"), "/nested\n").unwrap();
    let nested = parent.join("nested");
    git(
        &path,
        &["worktree", "add", "-b", "nested", nested.to_str().unwrap()],
    );
    assert_eq!(
        Repository::discover(&parent)
            .unwrap()
            .unwrap()
            .changes()
            .unwrap()
            .total(),
        0
    );
    assert!(scope.remove(&parent).is_err());
    assert!(nested.join("tracked.txt").is_file());
    assert_eq!(repo.worktrees().unwrap().len(), 3);
}
