use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Result, anyhow, bail};
use matou_native_core::{
    git_repo::{BranchReservation, ForkClaim, Repository},
    sessions::ProviderBinding,
};
use matou_native_runtime::{
    codex_fork::{
        CodexForkCandidate, CodexForkHook, CodexForkSettings, CodexForkSource, CodexForkTarget,
        CodexForks,
    },
    fork_worktree::{ForkIdentity, ForkLease, ForkStage, ForkWorktreePlan, NewWorktree},
};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

const PARENT: &str = "12345678-1234-4123-8123-1234567890ab";
const CHILD: &str = "22345678-1234-4123-8123-1234567890ab";
const OTHER: &str = "32345678-1234-4123-8123-1234567890ab";

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    codex_home: PathBuf,
    executable: PathBuf,
    binding: ProviderBinding,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let repo = root.path().join("主 仓库");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        // The product and the isolated fixture Git commands must use the same line endings.
        git(&repo, &["config", "--local", "core.autocrlf", "false"]);
        fs::write(repo.join(".gitignore"), "*.secret\n").unwrap();
        fs::write(repo.join("tracked"), "initial\n").unwrap();
        git(&repo, &["add", ".gitignore", "tracked"]);
        commit(&repo, "initial");
        let codex_home = root.path().join("Codex fixtures");
        let sessions = codex_home.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let rollout = json!({"type": "session_meta", "payload": {
            "id": PARENT, "cwd": repo, "timestamp": "2026-01-01T12:00:00.000Z"
        }});
        fs::write(
            sessions.join(format!("rollout-fixture-{PARENT}.jsonl")),
            format!("{rollout}\n"),
        )
        .unwrap();
        let executable = root.path().join(if cfg!(windows) {
            "fixture-codex.exe"
        } else {
            "fixture-codex"
        });
        fs::write(&executable, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let binding = ProviderBinding {
            id: "validated-codex-binding".into(),
            provider_session_id: PARENT.into(),
            metadata: json!({"canFork": true, "permissionMode": "plan", "model": "source-model"}),
            resumable: true,
            restore_state: "none".into(),
        };
        Self {
            root,
            repo,
            codex_home,
            executable,
            binding,
        }
    }

    fn source(&self) -> CodexForkSource<'_> {
        CodexForkSource {
            provider: "codex",
            cwd: &self.repo,
            codex_home: &self.codex_home,
            binding: &self.binding,
        }
    }

    fn settings(&self) -> CodexForkSettings<'_> {
        CodexForkSettings {
            data_root: self.root.path(),
            environment: [
                ("MATOU_CODEX_COMMAND".into(), self.executable.clone().into()),
                ("CODEX_HOME".into(), self.codex_home.clone().into()),
            ]
            .into(),
            cols: 100,
            rows: 30,
        }
    }

    fn worktree(&self, path: &str, branch: &str) -> NewWorktree {
        NewWorktree {
            isolation_root: self.root.path().to_owned(),
            path: self.root.path().join(path),
            branch: branch.into(),
            worktree_id: format!("worktree-{path}"),
            execution_context_id: format!("context-{path}"),
        }
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.repo).unwrap().unwrap()
    }

    fn has_branch(&self, branch: &str) -> bool {
        self.repository()
            .local_branches()
            .unwrap()
            .branches
            .iter()
            .any(|local| local.name == branch)
    }
}

fn identity(key: &str) -> ForkIdentity {
    ForkIdentity {
        operation_id: format!("operation-{key}"),
        submission_key: key.into(),
        source_session_id: "parent-session".into(),
        session_id: format!("child-{key}"),
    }
}

fn lease(identity: &ForkIdentity) -> ForkLease {
    ForkLease {
        operation_id: identity.operation_id.clone(),
        token: Uuid::new_v4(),
        fence: 1,
    }
}

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
    String::from_utf8(output.stdout).unwrap().trim_end().into()
}

fn commit(path: &Path, message: &str) {
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
            message,
        ],
    );
}

#[test]
fn new_worktree_binds_the_child_at_the_reserved_revision() -> Result<()> {
    let fixture = Fixture::new();
    let identity = identity("initial");
    let owner = lease(&identity);
    let target = fixture.worktree("子 目录", "feature/fork");
    let cwd = target.path.clone();
    let base = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let plan = ForkWorktreePlan::reserve(identity.clone(), &fixture.repo, target, &[])?;
    commit(&fixture.repo, "after reservation");
    let mut stages = Vec::new();
    let mut committed = None;
    let binding = plan.execute(
        &owner,
        |stage, lease| {
            assert_eq!(lease, &owner);
            stages.push(stage);
            Ok(())
        },
        |data, _| {
            assert!(data.cwd.join("tracked").is_file());
            Ok(())
        },
        |data, lease| {
            assert_eq!(lease, &owner);
            committed = Some(data.clone());
            Ok(())
        },
    )?;
    assert_eq!(
        stages,
        [
            ForkStage::CreatingWorktree,
            ForkStage::ApplyingSetup,
            ForkStage::BindingSession
        ]
    );
    assert_eq!(committed.as_ref(), Some(&binding));
    assert_eq!(binding.identity, identity);
    assert_eq!(binding.repository_root, fixture.repo.canonicalize()?);
    assert_eq!(binding.cwd, cwd.canonicalize()?);
    assert_eq!(binding.worktree_id, "worktree-子 目录");
    assert_eq!(binding.execution_context_id, "context-子 目录");
    assert_eq!(binding.branch, "feature/fork");
    assert_eq!(binding.base_revision, base);
    assert_eq!(git(&binding.cwd, &["rev-parse", "HEAD"]), base);
    assert!(fixture.has_branch("feature/fork"));
    Ok(())
}

#[test]
fn branch_reservations_git_conflicts_and_directory_conflicts_leave_existing_data() -> Result<()> {
    let fixture = Fixture::new();
    let identity = identity("conflict");
    let owner = lease(&identity);
    let branch = "fork-conflict";
    let repo = fixture.repository();
    let reservation = BranchReservation {
        repository_root: repo.root(),
        branch,
        worktree_removed: true,
        fork: Some(ForkClaim {
            operation_id: "another-operation",
            submission_key: "another-key",
            finished: false,
        }),
    };
    assert!(
        ForkWorktreePlan::reserve(
            identity.clone(),
            &fixture.repo,
            fixture.worktree("reserved", branch),
            &[reservation],
        )
        .is_err()
    );
    let plan = ForkWorktreePlan::reserve(
        identity.clone(),
        &fixture.repo,
        fixture.worktree("reserved", branch),
        &[],
    )?;
    git(&fixture.repo, &["branch", branch]);
    let old_ref = git(&fixture.repo, &["rev-parse", branch]);
    assert!(
        plan.execute(
            &owner,
            |_, _| Ok(()),
            |_, _| unreachable!(),
            |_, _| unreachable!()
        )
        .is_err()
    );
    assert_eq!(git(&fixture.repo, &["rev-parse", branch]), old_ref);
    assert!(!fixture.root.path().join("reserved").exists());

    let target = fixture.worktree("occupied", "directory-conflict");
    fs::create_dir(&target.path)?;
    let dirty = target.path.join("keep");
    fs::write(&dirty, "pre-existing")?;
    let plan = ForkWorktreePlan::reserve(identity, &fixture.repo, target, &[])?;
    assert!(
        plan.execute(
            &owner,
            |_, _| Ok(()),
            |_, _| unreachable!(),
            |_, _| unreachable!()
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(&dirty)?, "pre-existing");
    assert!(!fixture.has_branch("directory-conflict"));
    assert_eq!(fixture.repository().worktrees()?.len(), 1);
    Ok(())
}

#[test]
fn failed_binding_rolls_back_only_the_new_clean_checkout_and_branch() -> Result<()> {
    let fixture = Fixture::new();
    let identity = identity("bind-failed");
    let owner = lease(&identity);
    git(&fixture.repo, &["branch", "keep"]);
    fs::write(fixture.repo.join("tracked"), "source changes\n")?;
    fs::write(fixture.repo.join("outside-fork"), "keep this")?;
    let target = fixture.worktree("failed-checkout", "remove-this");
    let cwd = target.path.clone();
    let plan = ForkWorktreePlan::reserve(identity, &fixture.repo, target, &[])?;
    let error = plan
        .execute(
            &owner,
            |_, _| Ok(()),
            |_, _| Ok(()),
            |_, _| bail!("binding transaction failed"),
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "binding transaction failed");
    assert!(!cwd.exists());
    assert!(!fixture.has_branch("remove-this"));
    assert!(fixture.has_branch("keep"));
    assert_eq!(fixture.repository().worktrees()?.len(), 1);
    assert_eq!(
        fs::read_to_string(fixture.repo.join("tracked"))?,
        "source changes\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("outside-fork"))?,
        "keep this"
    );
    Ok(())
}

#[test]
fn rollback_preserves_dirty_ignored_and_advanced_checkouts() -> Result<()> {
    let fixture = Fixture::new();
    for (branch, file) in [
        ("dirty", "untracked"),
        ("ignored", "local.secret"),
        ("advanced", "tracked"),
    ] {
        let identity = identity(branch);
        let owner = lease(&identity);
        let target = fixture.worktree(branch, branch);
        let cwd = target.path.clone();
        let plan = ForkWorktreePlan::reserve(identity, &fixture.repo, target, &[])?;
        let error = plan
            .execute(
                &owner,
                |_, _| Ok(()),
                |binding, _| {
                    fs::write(binding.cwd.join(file), "preserve")?;
                    if branch == "advanced" {
                        git(&binding.cwd, &["add", "tracked"]);
                        commit(&binding.cwd, "user commit");
                    }
                    Ok(())
                },
                |_, _| bail!("binding transaction failed"),
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains("Fork rollback incomplete"));
        assert_eq!(fs::read_to_string(cwd.join(file))?, "preserve");
        assert!(fixture.has_branch(branch));
    }
    assert_eq!(fixture.repository().worktrees()?.len(), 4);
    Ok(())
}

#[test]
fn rollback_preserves_a_replacement_worktree_at_the_same_path() -> Result<()> {
    let fixture = Fixture::new();
    let identity = identity("replacement");
    let owner = lease(&identity);
    let target = fixture.worktree("replaceable", "replaceable");
    let cwd = target.path.clone();
    let plan = ForkWorktreePlan::reserve(identity, &fixture.repo, target, &[])?;
    let error = plan
        .execute(
            &owner,
            |_, _| Ok(()),
            |_, _| Ok(()),
            |binding, _| {
                assert_eq!(binding.cwd, cwd.canonicalize()?);
                let path = cwd.to_str().unwrap();
                git(&fixture.repo, &["worktree", "remove", "--", path]);
                git(
                    &fixture.repo,
                    &["worktree", "add", "--", path, &binding.branch],
                );
                bail!("binding transaction failed")
            },
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("checkout identity changed"));
    assert!(cwd.join("tracked").is_file());
    assert!(fixture.has_branch("replaceable"));
    assert_eq!(fixture.repository().worktrees()?.len(), 2);
    Ok(())
}

#[test]
fn a_stale_lease_cannot_commit_a_child_binding() -> Result<()> {
    let fixture = Fixture::new();
    let identity = identity("expired");
    let owner = lease(&identity);
    let target = fixture.worktree("expired", "expired");
    let cwd = target.path.clone();
    let plan = ForkWorktreePlan::reserve(identity, &fixture.repo, target, &[])?;
    let wrong_operation = lease(&self::identity("unrelated"));
    assert!(
        plan.execute(
            &wrong_operation,
            |_, _| unreachable!(),
            |_, _| unreachable!(),
            |_, _| unreachable!()
        )
        .is_err()
    );
    assert!(!cwd.exists());
    let result = plan.execute(
        &owner,
        |stage, _| {
            if stage == ForkStage::BindingSession {
                bail!("lease no longer current");
            }
            Ok(())
        },
        |_, _| Ok(()),
        |_, _| unreachable!(),
    );
    assert_eq!(result.unwrap_err().to_string(), "lease no longer current");
    assert!(!cwd.exists());
    assert!(!fixture.has_branch("expired"));
    Ok(())
}

#[test]
fn codex_source_must_be_validated_finished_and_in_the_source_directory() {
    let mut fixture = Fixture::new();
    let mut forks = CodexForks::default();
    fixture.binding.metadata["canFork"] = json!(false);
    assert!(
        forks
            .accept(
                identity("source"),
                fixture.source(),
                CodexForkTarget::Current,
                &[]
            )
            .is_err()
    );
    fixture.binding.metadata["canFork"] = json!(true);
    fixture.binding.restore_state = "restoring".into();
    assert!(
        forks
            .accept(
                identity("source"),
                fixture.source(),
                CodexForkTarget::Current,
                &[]
            )
            .is_err()
    );
    fixture.binding.restore_state = "none".into();
    let wrong_cwd = fixture.root.path();
    let source = CodexForkSource {
        cwd: wrong_cwd,
        ..fixture.source()
    };
    assert!(
        forks
            .accept(identity("source"), source, CodexForkTarget::Current, &[])
            .is_err()
    );
    assert!(forks.get_mut("source").is_none());
}

#[test]
fn codex_current_fork_uses_the_reserved_source_and_only_confirms_session_start() -> Result<()> {
    let fixture = Fixture::new();
    let mut forks = CodexForks::default();
    let intent = forks.accept(
        identity("current"),
        fixture.source(),
        CodexForkTarget::Current,
        &[],
    )?;
    assert_eq!(intent.stage(), ForkStage::Queued);
    let mut stages = Vec::new();
    let run = intent.begin(
        false,
        fixture.settings(),
        |stage, _| {
            stages.push(stage);
            Ok(())
        },
        |_, _| unreachable!(),
        |_, _| unreachable!(),
    )?;
    assert_eq!(
        stages,
        [ForkStage::RestoringProvider, ForkStage::StartingWindow]
    );
    assert_eq!(run.launch.cwd, fixture.repo.canonicalize()?);
    assert_eq!(
        run.launch.argv,
        [
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            r#"tui.status_line=["fast-mode"]"#,
            "--model",
            "source-model",
            "--ask-for-approval",
            "on-request",
            "-c",
            "sandbox_mode=\"read-only\"",
            "-c",
            "default_permissions=\":read-only\"",
            "-c",
            "approvals_reviewer=\"user\"",
            "-c",
            "tui.resume_cwd=\"current\"",
            "fork",
            PARENT,
        ],
    );
    assert!(
        intent
            .observe_hook(
                &run.authority,
                CodexForkHook {
                    event_name: "Stop",
                    provider_session_id: CHILD,
                    cwd: &run.launch.cwd
                },
                |_, _| unreachable!(),
                |_| unreachable!(),
            )?
            .is_none()
    );
    let bound = intent
        .observe_hook(
            &run.authority,
            CodexForkHook {
                event_name: "SessionStart",
                provider_session_id: CHILD,
                cwd: &run.launch.cwd,
            },
            |binding, authority| {
                assert_eq!(binding.identity, identity("current"));
                assert_eq!(authority, &run.authority);
                Ok(())
            },
            |_| unreachable!(),
        )?
        .unwrap();
    assert!(bound.inherited_conversation);
    assert_eq!(bound.permission_mode, "readOnly");
    assert_eq!(bound.model, "source-model");
    assert_eq!(bound.provider_session_id, Uuid::parse_str(CHILD)?);
    assert_eq!(intent.stage(), ForkStage::Succeeded);
    assert_eq!(fixture.repository().worktrees()?.len(), 1);
    Ok(())
}

#[test]
fn codex_worktree_retries_reuse_the_child_and_checkout_and_fence_old_runs() -> Result<()> {
    let fixture = Fixture::new();
    let mut forks = CodexForks::default();
    let request = identity("worktree");
    forks.accept(
        request.clone(),
        fixture.source(),
        CodexForkTarget::NewWorktree(fixture.worktree("child", "codex-child")),
        &[],
    )?;
    assert!(
        forks
            .accept(
                identity("competing"),
                fixture.source(),
                CodexForkTarget::NewWorktree(fixture.worktree("competing", "codex-child")),
                &[],
            )
            .is_err()
    );
    let stages = RefCell::new(Vec::new());
    let intent = forks.get_mut("worktree").unwrap();
    let run = intent.begin(
        false,
        fixture.settings(),
        |stage, _| {
            stages.borrow_mut().push(stage);
            Ok(())
        },
        |_, _| Ok(()),
        |binding, _| {
            assert_eq!(binding.identity, request);
            stages.borrow_mut().push(ForkStage::RestoringProvider);
            Ok(())
        },
    )?;
    assert_eq!(
        stages.into_inner(),
        [
            ForkStage::CreatingWorktree,
            ForkStage::ApplyingSetup,
            ForkStage::BindingSession,
            ForkStage::RestoringProvider,
            ForkStage::StartingWindow,
        ],
    );
    let marker = run.launch.cwd.join("preserve-on-retry");
    fs::write(&marker, "child changes")?;
    let mut cleaned = None;
    assert!(
        intent
            .observe_hook(
                &run.authority,
                CodexForkHook {
                    event_name: "SessionStart",
                    provider_session_id: PARENT,
                    cwd: &run.launch.cwd
                },
                |_, _| unreachable!(),
                |authority| {
                    cleaned = Some(authority.run_id);
                    Ok(())
                },
            )
            .is_err()
    );
    assert_eq!(cleaned, Some(run.authority.run_id));
    assert_eq!(intent.stage(), ForkStage::Failed);
    assert_eq!(intent.worktree_binding().unwrap().cwd, run.launch.cwd);

    let intent = forks.accept(
        request.clone(),
        fixture.source(),
        CodexForkTarget::NewWorktree(fixture.worktree("child", "codex-child")),
        &[],
    )?;
    assert_eq!(intent.identity(), &request);
    assert!(
        intent
            .begin(
                false,
                fixture.settings(),
                |_, _| unreachable!(),
                |_, _| unreachable!(),
                |_, _| unreachable!()
            )
            .is_err()
    );
    let retry = intent.begin(
        true,
        fixture.settings(),
        |_, _| Ok(()),
        |_, _| unreachable!(),
        |_, _| unreachable!(),
    )?;
    assert_eq!(retry.launch.argv, run.launch.argv);
    assert_eq!(retry.launch.cwd, run.launch.cwd);
    assert!(retry.authority.lease.fence > run.authority.lease.fence);
    assert!(
        intent
            .observe_hook(
                &run.authority,
                CodexForkHook {
                    event_name: "SessionStart",
                    provider_session_id: OTHER,
                    cwd: &retry.launch.cwd
                },
                |_, _| unreachable!(),
                |_| unreachable!(),
            )?
            .is_none()
    );
    assert!(!intent.fail(&run.authority, &anyhow!("late exit"), |_| unreachable!())?);
    assert!(
        intent
            .observe_hook(
                &retry.authority,
                CodexForkHook {
                    event_name: "SessionStart",
                    provider_session_id: CHILD,
                    cwd: &retry.launch.cwd
                },
                |_, _| Ok(()),
                |_| unreachable!(),
            )?
            .is_some()
    );
    assert_eq!(fs::read_to_string(marker)?, "child changes");
    assert_eq!(fixture.repository().worktrees()?.len(), 2);
    Ok(())
}

#[test]
fn codex_candidate_validation_and_provider_cleanup_gate_the_retry() -> Result<()> {
    let fixture = Fixture::new();
    let mut forks = CodexForks::default();
    let intent = forks.accept(
        identity("rollout"),
        fixture.source(),
        CodexForkTarget::Current,
        &[],
    )?;
    let run = intent.begin(
        false,
        fixture.settings(),
        |_, _| Ok(()),
        |_, _| unreachable!(),
        |_, _| unreachable!(),
    )?;
    let candidate = |id, parent, cwd, created_at_ms| CodexForkCandidate {
        provider_session_id: id,
        forked_from_id: parent,
        cwd,
        created_at_ms,
    };
    let cwd = run.launch.cwd.as_path();
    assert!(
        intent
            .discover_candidate(
                &run.authority,
                &[
                    candidate(CHILD, PARENT, cwd, 1),
                    candidate(CHILD, OTHER, cwd, u64::MAX),
                    candidate(CHILD, PARENT, fixture.root.path(), u64::MAX),
                    candidate(PARENT, PARENT, cwd, u64::MAX),
                    candidate("bad-uuid", PARENT, cwd, u64::MAX),
                ],
            )
            .is_none()
    );
    assert!(
        intent
            .discover_candidate(
                &run.authority,
                &[
                    candidate(CHILD, PARENT, cwd, u64::MAX),
                    candidate(OTHER, PARENT, cwd, u64::MAX)
                ],
            )
            .is_none()
    );
    assert_eq!(
        intent.discover_candidate(&run.authority, &[candidate(CHILD, PARENT, cwd, u64::MAX)]),
        Some(Uuid::parse_str(CHILD)?)
    );
    assert_eq!(intent.stage(), ForkStage::StartingWindow);
    let residue = fixture.root.path().join("pending-hook-registration");
    fs::write(&residue, run.authority.run_id.to_string())?;
    let error = intent
        .observe_hook(
            &run.authority,
            CodexForkHook {
                event_name: "SessionStart",
                provider_session_id: OTHER,
                cwd,
            },
            |_, _| unreachable!(),
            |_| bail!("cleanup interrupted"),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("cleanup interrupted"));
    assert_eq!(intent.stage(), ForkStage::Failed);
    assert!(
        intent
            .begin(
                true,
                fixture.settings(),
                |_, _| unreachable!(),
                |_, _| unreachable!(),
                |_, _| unreachable!()
            )
            .is_err()
    );
    assert!(intent.fail(&run.authority, &error, |authority| {
        assert_eq!(fs::read_to_string(&residue)?, authority.run_id.to_string());
        fs::remove_file(&residue)?;
        Ok(())
    })?);
    assert!(!residue.exists());
    let retry = intent.begin(
        true,
        fixture.settings(),
        |_, _| Ok(()),
        |_, _| unreachable!(),
        |_, _| unreachable!(),
    )?;
    assert!(
        intent
            .observe_hook(
                &retry.authority,
                CodexForkHook {
                    event_name: "SessionStart",
                    provider_session_id: CHILD,
                    cwd
                },
                |_, _| unreachable!(),
                |_| Ok(()),
            )
            .is_err()
    );
    let retry = intent.begin(
        true,
        fixture.settings(),
        |_, _| Ok(()),
        |_, _| unreachable!(),
        |_, _| unreachable!(),
    )?;
    assert!(
        intent
            .observe_hook(
                &retry.authority,
                CodexForkHook {
                    event_name: "SessionStart",
                    provider_session_id: "invalid-uuid",
                    cwd
                },
                |_, _| unreachable!(),
                |_| Ok(()),
            )
            .is_err()
    );
    Ok(())
}
