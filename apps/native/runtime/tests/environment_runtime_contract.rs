use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use matou_native_core::git_repo::{
    ChangeCounts, Head, LocalBranch, Repository, WorktreeBranch, WorktreeScope,
};
use matou_native_runtime::environment_runtime::{
    CheckoutDisposition, EnvironmentKind, EnvironmentProblem, EnvironmentTarget,
    MissingEnvironment, RepairReadiness, SessionDirectory, UnavailableEnvironment,
    WorktreeExpectation, check_repair, checkout_disposition, probe_environment,
    shared_session_count,
};
use serde_json::Value;
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
    git(
        path,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
}

fn fixture() -> (TempDir, PathBuf, Repository, WorktreeScope) {
    let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let path = dir.path().join("主 仓库");
    init(&path);
    let repo = Repository::discover(&path).unwrap().unwrap();
    let scope = WorktreeScope::new(dir.path(), &repo).unwrap();
    (dir, path, repo, scope)
}

#[test]
fn plain_local_directory_is_ready_and_missing_or_file_paths_are_distinct() {
    let ceiling = fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
    if std::env::var_os("GIT_CEILING_DIRECTORIES").as_deref() != Some(ceiling.as_os_str()) {
        // Prevent the test directory from discovering the enclosing project checkout.
        let status = Command::new(std::env::current_exe().unwrap())
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .args([
                "--exact",
                "plain_local_directory_is_ready_and_missing_or_file_paths_are_distinct",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let plain = dir.path().join("共享 目录");
    fs::create_dir(&plain).unwrap();
    let ready = probe_environment(EnvironmentTarget::Local(&plain)).unwrap();
    assert_eq!(ready.cwd, plain.canonicalize().unwrap());
    assert_eq!(ready.kind, EnvironmentKind::Local);
    assert_eq!(ready.git, None);
    assert_eq!(
        probe_environment(EnvironmentTarget::Local(&plain.join("missing"))),
        Err(EnvironmentProblem::Missing(MissingEnvironment::PathMissing))
    );
    let file = plain.join("file");
    fs::write(&file, "").unwrap();
    assert_eq!(
        probe_environment(EnvironmentTarget::Local(&file)),
        Err(EnvironmentProblem::Unavailable(
            UnavailableEnvironment::NotDirectory
        ))
    );
}

#[test]
fn local_and_independent_targets_report_checkout_identity_and_changes() {
    let (dir, path, repo, scope) = fixture();
    let child = path.join("子 目录");
    fs::create_dir(&child).unwrap();
    let ready = probe_environment(EnvironmentTarget::Local(&child)).unwrap();
    assert_eq!(ready.kind, EnvironmentKind::Local);
    assert_eq!(ready.cwd, child.canonicalize().unwrap());
    assert_eq!(ready.git.unwrap().repository, repo);

    let target = dir.path().join("独立 checkout");
    scope
        .create(
            &target,
            WorktreeBranch::New {
                name: "feature/review",
                base_ref: "HEAD",
            },
        )
        .unwrap();
    fs::write(target.join("notes"), "edited").unwrap();
    let expected = WorktreeExpectation {
        repository_root: &path,
        path: &target,
        head: &Head::Branch("feature/review".into()),
    };
    let ready = probe_environment(EnvironmentTarget::Worktree(expected)).unwrap();
    assert_eq!(ready.kind, EnvironmentKind::Worktree);
    let state = ready.git.unwrap();
    assert_eq!(&state.head, expected.head);
    assert!(repo.same_repository(&state.repository));
    assert_eq!(state.changes.untracked, 1);
    let mismatch = WorktreeExpectation {
        head: &Head::Branch("different".into()),
        ..expected
    };
    assert_eq!(
        probe_environment(EnvironmentTarget::Worktree(mismatch)),
        Err(EnvironmentProblem::Unavailable(
            UnavailableEnvironment::WrongBranch
        ))
    );

    git(&target, &["checkout", "--detach", "HEAD"]);
    assert_eq!(
        probe_environment(EnvironmentTarget::Worktree(expected)),
        Err(EnvironmentProblem::Unavailable(
            UnavailableEnvironment::WrongHead
        ))
    );
    let detached = Head::Detached(repo.resolve_commit("HEAD").unwrap());
    let expected = WorktreeExpectation {
        head: &detached,
        ..expected
    };
    assert_eq!(
        probe_environment(EnvironmentTarget::Worktree(expected))
            .unwrap()
            .git
            .unwrap()
            .head,
        detached
    );
}

#[test]
fn moved_checkout_requires_a_verified_repair_and_preserves_other_environments() {
    let (dir, path, repo, scope) = fixture();
    let original = dir.path().join("旧目录");
    let moved = dir.path().join("选中的 新目录");
    let other = dir.path().join("另一个目录");
    scope
        .create(
            &original,
            WorktreeBranch::New {
                name: "first",
                base_ref: "HEAD",
            },
        )
        .unwrap();
    scope
        .create(
            &other,
            WorktreeBranch::New {
                name: "other",
                base_ref: "HEAD",
            },
        )
        .unwrap();
    let expected = WorktreeExpectation {
        repository_root: &path,
        path: &original,
        head: &Head::Branch("first".into()),
    };
    let other_expected = WorktreeExpectation {
        path: &other,
        head: &Head::Branch("other".into()),
        ..expected
    };

    fs::write(original.join("keep"), "work in progress").unwrap();
    fs::rename(&original, &moved).unwrap();
    assert_eq!(
        probe_environment(EnvironmentTarget::Worktree(expected)),
        Err(EnvironmentProblem::Missing(MissingEnvironment::PathMissing))
    );
    assert_eq!(
        probe_environment(EnvironmentTarget::Worktree(WorktreeExpectation {
            path: &moved,
            ..expected
        })),
        Err(EnvironmentProblem::Missing(
            MissingEnvironment::NotListedByGit
        ))
    );
    assert_eq!(
        check_repair(&scope, expected, &moved).unwrap(),
        RepairReadiness::RepairableMoved {
            cwd: moved.canonicalize().unwrap(),
        }
    );
    assert!(
        repo.worktrees()
            .unwrap()
            .iter()
            .any(|entry| entry.path.ends_with("旧目录"))
    );
    scope
        .repair_moved(&original, &moved, expected.head)
        .unwrap();

    let ready = probe_environment(EnvironmentTarget::Worktree(WorktreeExpectation {
        path: &moved,
        ..expected
    }))
    .unwrap();
    assert_eq!(ready.git.unwrap().changes.untracked, 1);
    assert_eq!(
        fs::read_to_string(moved.join("keep")).unwrap(),
        "work in progress"
    );
    assert!(probe_environment(EnvironmentTarget::Worktree(other_expected)).is_ok());
    assert_eq!(repo.head().unwrap(), Head::Branch("main".into()));
}

#[test]
fn repair_rejects_foreign_repositories_and_copied_checkout_markers() {
    let (dir, path, repo, scope) = fixture();
    let original = dir.path().join("original");
    scope
        .create(
            &original,
            WorktreeBranch::New {
                name: "first",
                base_ref: "HEAD",
            },
        )
        .unwrap();
    let expected = WorktreeExpectation {
        repository_root: &path,
        path: &original,
        head: &Head::Branch("first".into()),
    };
    let foreign = dir.path().join("foreign");
    init(&foreign);
    assert_eq!(
        check_repair(&scope, expected, &foreign),
        Err(EnvironmentProblem::Unavailable(
            UnavailableEnvironment::WrongRepository
        ))
    );

    let copy = dir.path().join("copy");
    fs::create_dir(&copy).unwrap();
    fs::copy(original.join(".git"), copy.join(".git")).unwrap();
    assert!(matches!(
        check_repair(&scope, expected, &copy),
        Err(EnvironmentProblem::Unavailable(
            UnavailableEnvironment::UnsafeRepair(_)
        ))
    ));
    assert!(scope.repair_moved(&original, &copy, expected.head).is_err());
    assert_eq!(repo.worktrees().unwrap().len(), 2);
    assert!(probe_environment(EnvironmentTarget::Worktree(expected)).is_ok());
}

#[test]
fn shared_checkout_rules_match_k_fixture_dispositions() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../tooling/native-parity/fixtures/git-branch-policy-reference.json"
    ))
    .unwrap();
    let mut checked = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        if input["operation"] != "shared-checkout" {
            continue;
        }
        let expected = &case["expected"];
        let cwd = Path::new(input["cwd"].as_str().unwrap());
        let shared = input["sessions"].as_array().map(|rows| {
            let sessions: Vec<_> = rows
                .iter()
                .map(|row| SessionDirectory {
                    workspace_id: if row[1] == "task-c" { "other" } else { "same" },
                    cwd: Path::new(row[2].as_str().unwrap()),
                    archived: !row[3].is_null(),
                })
                .collect();
            let count = shared_session_count("same", cwd, &sessions);
            assert_eq!(
                Some(count as u64),
                expected["projection"]["sharedSessionCount"].as_u64()
            );
            assert_eq!(
                count > 1,
                expected["projection"]["sharedWorkingDirectory"]
                    .as_bool()
                    .unwrap()
            );
            count
        });
        let row = input["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == input["branch"])
            .unwrap();
        let branch = LocalBranch {
            name: row["name"].as_str().unwrap().into(),
            current: row["current"].as_bool().unwrap(),
            commit_timestamp: row["commitTimestamp"].as_i64().unwrap(),
            checked_out_path: row["checkedOutPath"].as_str().map(PathBuf::from),
        };
        let kind = if input["context"]["environmentKind"] == "worktree" {
            EnvironmentKind::Worktree
        } else {
            EnvironmentKind::Local
        };
        let counts = &expected["statusCounts"];
        let changes = ChangeCounts {
            staged: counts["stagedCount"].as_u64().unwrap() as usize,
            unstaged: counts["unstagedCount"].as_u64().unwrap() as usize,
            untracked: counts["untrackedCount"].as_u64().unwrap() as usize,
        };
        let disposition = checkout_disposition(
            &branch,
            kind,
            input["context"]["hasOwnedWorktree"] == true,
            shared,
            changes,
        );
        match disposition {
            CheckoutDisposition::ConfirmShared {
                other_sessions,
                uncommitted,
            } => {
                assert_eq!(other_sessions + 1, shared.unwrap());
                assert_eq!(uncommitted, changes.total());
                assert!(expected["confirmation"].is_object());
            }
            CheckoutDisposition::CheckedOutElsewhere => assert_eq!(expected["rowDisabled"], true),
            CheckoutDisposition::CurrentBranch => {
                assert!(expected["confirmation"].is_null());
                assert!(expected["callsBeforeChoice"].as_array().unwrap().is_empty());
                assert_eq!(expected["rowDisabled"], false);
            }
            CheckoutDisposition::Checkout => {
                assert_eq!(expected["callsBeforeChoice"][0]["method"], "git.checkout")
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 12);
}
