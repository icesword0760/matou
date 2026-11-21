use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use matou_native_core::git_repo::{
    ChangeCounts, ForkBranchError, GitActionError, Head, Removal, Repository, WorktreeBranch,
    WorktreeScope, current_branch, git_checkout, git_commit, git_create_branch, git_push,
    git_status_detail, git_worktree_create, git_worktree_remove,
};
use tempfile::TempDir;

fn git(path: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
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

fn isolated_git_writes(name: &str, test: impl FnOnce()) {
    if std::env::var_os("MATOU_GIT_WRITE_FIXTURE").is_some() {
        test();
        return;
    }
    let config = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .env("MATOU_GIT_WRITE_FIXTURE", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", config.path().join("no-global-config"))
        .env("GIT_ALLOW_PROTOCOL", "file")
        .args(["--exact", name, "--test-threads=1"])
        .status()
        .unwrap();
    assert!(result.success());
}

fn issue(error: anyhow::Error) -> &'static str {
    error
        .downcast_ref::<GitActionError>()
        .unwrap_or_else(|| panic!("expected a Git issue, got {error:#}"))
        .0
}

#[test]
fn git_menu_status_checkout_and_branch_creation() {
    isolated_git_writes("git_menu_status_checkout_and_branch_creation", || {
        let (_temp, cwd, repo) = fixture();
        let initial = repo.resolve_commit("HEAD").unwrap();
        git(&cwd, &["branch", "base"]);
        fs::write(cwd.join("tracked.txt"), "line two\nline three\n").unwrap();
        git(&cwd, &["add", "tracked.txt"]);
        fs::write(cwd.join("tracked.txt"), "line two\nline three\nline four\n").unwrap();
        fs::write(cwd.join("untracked.txt"), "untracked\n").unwrap();
        let status = git_status_detail(&cwd).unwrap();
        assert_eq!(status.current_branch.as_deref(), Some("main"));
        assert_eq!(status.dirty_file_count, 2);
        assert_eq!(status.staged_count, 1);
        assert_eq!((status.additions, status.deletions), (3, 1));
        assert!(status.dirty && status.worktrees[0].main && status.worktrees[0].current);

        let committed = git_commit(&cwd, "update tracked", false).unwrap();
        assert_eq!(committed.dirty_file_count, 2);
        assert_eq!(committed.staged_count, 0);
        assert_eq!(
            issue(git_checkout(&cwd, "base").unwrap_err()),
            "GIT_CHECKOUT_CONFLICT"
        );
        assert_eq!(
            fs::read_to_string(cwd.join("tracked.txt")).unwrap(),
            "line two\nline three\nline four\n"
        );
        git(&cwd, &["restore", "tracked.txt"]);
        let switched = git_checkout(&cwd, "base").unwrap();
        assert_eq!(switched.current_branch.as_deref(), Some("base"));
        assert_eq!(switched.dirty_file_count, 1);
        assert!(
            switched
                .branches
                .iter()
                .any(|branch| branch.current && branch.name == "base")
        );

        for invalid in ["feature//new", "-b", "@{-1}", "HEAD", "new branch"] {
            assert_eq!(
                issue(git_create_branch(&cwd, invalid, None).unwrap_err()),
                "GIT_INVALID_BRANCH"
            );
        }
        assert_eq!(
            issue(git_create_branch(&cwd, "base", None).unwrap_err()),
            "GIT_BRANCH_EXISTS"
        );
        assert_eq!(
            issue(git_create_branch(&cwd, "uncreated", Some("--help")).unwrap_err()),
            "GIT_INVALID_START_POINT"
        );
        let created = git_create_branch(&cwd, "feature/菜单", Some("main")).unwrap();
        assert_eq!(created.current_branch.as_deref(), Some("feature/菜单"));
        assert_eq!(
            repo.resolve_commit("HEAD").unwrap(),
            repo.resolve_commit("main").unwrap()
        );
        git(&cwd, &["checkout", "--detach", &initial]);
        let detached = git_status_detail(&cwd).unwrap();
        assert_eq!(detached.detached_head.as_deref(), Some(initial.as_str()));
        assert_eq!(issue(git_push(&cwd).unwrap_err()), "GIT_DETACHED_HEAD");
    });
}

#[test]
fn git_menu_push_sets_upstream_and_pushes_only_current_branch_offline() {
    isolated_git_writes(
        "git_menu_push_sets_upstream_and_pushes_only_current_branch_offline",
        || {
            let (temp, cwd, repo) = fixture();
            let origin = temp.path().join("bare origin.git");
            fs::create_dir(&origin).unwrap();
            git(&origin, &["init", "--bare", "--initial-branch=main"]);
            assert_eq!(issue(git_push(&cwd).unwrap_err()), "GIT_NO_REMOTE");
            git(&cwd, &["remote", "add", "origin", origin.to_str().unwrap()]);
            git(&cwd, &["config", "push.default", "matching"]);
            git(&cwd, &["branch", "keep-local"]);
            let before = git_status_detail(&cwd).unwrap();
            assert!(before.has_remote && before.can_push && before.upstream.is_none());
            let pushed = git_push(&cwd).unwrap();
            assert_eq!(pushed.upstream.as_deref(), Some("refs/remotes/origin/main"));
            assert_eq!((pushed.ahead, pushed.behind), (0, 0));
            assert_eq!(
                git(
                    &origin,
                    &["for-each-ref", "--format=%(refname)", "refs/heads"]
                ),
                "refs/heads/main"
            );
            assert!(!pushed.can_push);
            assert_eq!(
                issue(git_commit(&cwd, "   ", true).unwrap_err()),
                "GIT_COMMIT_MESSAGE_REQUIRED"
            );
            assert_eq!(
                issue(git_commit(&cwd, "no changes", true).unwrap_err()),
                "GIT_NOTHING_TO_COMMIT"
            );

            fs::write(cwd.join("added.txt"), "offline push\n").unwrap();
            let committed = git_commit(&cwd, "push me", true).unwrap();
            assert!(committed.can_push && !committed.dirty);
            assert_eq!(committed.ahead, 1);
            let pushed = git_push(&cwd).unwrap();
            assert_eq!(pushed.ahead, 0);
            assert_eq!(
                git(&origin, &["rev-parse", "refs/heads/main"]),
                repo.resolve_commit("HEAD").unwrap()
            );

            // Move the fixture remote ahead with a conflicting commit.
            let other = temp.path().join("other checkout");
            git(
                temp.path(),
                &["clone", origin.to_str().unwrap(), other.to_str().unwrap()],
            );
            git(&other, &["config", "user.name", "Fixture"]);
            git(&other, &["config", "user.email", "fixture@example.invalid"]);
            fs::write(other.join("tracked.txt"), "from remote\n").unwrap();
            git_commit(&other, "remote change", true).unwrap();
            git_push(&other).unwrap();
            git(&cwd, &["fetch", "origin"]);
            fs::write(cwd.join("tracked.txt"), "local change\n").unwrap();
            let diverged = git_commit(&cwd, "local change", true).unwrap();
            assert_eq!((diverged.ahead, diverged.behind), (1, 1));
            assert_eq!(issue(git_push(&cwd).unwrap_err()), "GIT_PUSH_REJECTED");
            assert_eq!(git(&origin, &["show", "main:tracked.txt"]), "from remote");
        },
    );
}

#[test]
fn git_menu_worktree_creation_and_safe_removal() {
    isolated_git_writes("git_menu_worktree_creation_and_safe_removal", || {
        let (temp, cwd, _repo) = fixture();
        let target = temp.path().join("linked checkout");
        let status = git_worktree_create(&cwd, "feature/wt", Some(&target)).unwrap();
        let target = target.canonicalize().unwrap();
        assert_eq!(status.worktrees.len(), 2);
        let linked = &status.worktrees[1];
        assert_eq!(linked.path, target);
        assert_eq!(linked.branch.as_deref(), Some("feature/wt"));
        assert!(!linked.main && !linked.current);
        assert_eq!(
            issue(git_checkout(&cwd, "feature/wt").unwrap_err()),
            "GIT_BRANCH_IN_WORKTREE"
        );
        assert_eq!(
            issue(git_worktree_remove(&cwd, &cwd).unwrap_err()),
            "GIT_WORKTREE_PROTECTED"
        );
        assert_eq!(
            issue(git_worktree_remove(&target, &cwd).unwrap_err()),
            "GIT_WORKTREE_PROTECTED"
        );
        assert_eq!(
            issue(git_worktree_remove(&target, &target).unwrap_err()),
            "GIT_WORKTREE_PROTECTED"
        );

        git(&cwd, &["worktree", "lock", target.to_str().unwrap()]);
        assert_eq!(
            issue(git_worktree_remove(&cwd, &target).unwrap_err()),
            "GIT_WORKTREE_PROTECTED"
        );
        git(&cwd, &["worktree", "unlock", target.to_str().unwrap()]);
        fs::write(target.join("dirty.txt"), "must survive\n").unwrap();
        assert!(git_status_detail(&cwd).unwrap().worktrees[1].dirty);
        assert_eq!(
            issue(git_worktree_remove(&cwd, &target).unwrap_err()),
            "GIT_WORKTREE_DIRTY"
        );
        assert!(target.join("dirty.txt").is_file());
        fs::remove_file(target.join("dirty.txt")).unwrap();

        let clean = git_worktree_remove(&cwd, &target).unwrap();
        assert_eq!(clean.worktrees.len(), 1);
        assert!(!target.exists());
        let recreated = git_worktree_create(&cwd, "feature/wt", None).unwrap();
        assert_eq!(recreated.worktrees[1].branch.as_deref(), Some("feature/wt"));
        assert!(recreated.worktrees[1].path.is_dir());
        assert_eq!(
            git_worktree_remove(&cwd, &recreated.worktrees[1].path)
                .unwrap()
                .worktrees
                .len(),
            1
        );
    });
}

#[test]
fn git_menu_status_and_commit_support_unborn_and_merged_conflict_states() {
    isolated_git_writes(
        "git_menu_status_and_commit_support_unborn_and_merged_conflict_states",
        || {
            let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let cwd = dir.path().join("unborn");
            init(&cwd);
            let status = git_status_detail(&cwd).unwrap();
            assert_eq!(status.current_branch.as_deref(), Some("main"));
            assert!(!status.dirty && status.branches.is_empty());
            fs::write(cwd.join("tracked.txt"), "initial\n").unwrap();
            git(&cwd, &["add", "tracked.txt"]);
            assert_eq!(git_status_detail(&cwd).unwrap().additions, 1);
            git_commit(&cwd, "initial", true).unwrap();
            git_create_branch(&cwd, "side", None).unwrap();
            fs::write(cwd.join("tracked.txt"), "side\n").unwrap();
            git_commit(&cwd, "side", true).unwrap();
            git_checkout(&cwd, "main").unwrap();
            fs::write(cwd.join("tracked.txt"), "main\n").unwrap();
            git_commit(&cwd, "main", true).unwrap();
            let merge = Command::new("git")
                .current_dir(&cwd)
                .args(["merge", "side"])
                .output()
                .unwrap();
            assert!(!merge.status.success());
            assert!(git_status_detail(&cwd).unwrap().dirty);
            assert_eq!(
                issue(git_commit(&cwd, "resolve", true).unwrap_err()),
                "GIT_UNMERGED"
            );
            assert_eq!(
                git(&cwd, &["diff", "--name-only", "--diff-filter=U"]),
                "tracked.txt"
            );
        },
    );
}
