//! Codex Fork acceptance, launch planning and fenced provider confirmation.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, ensure};
use matou_native_core::{git_repo::BranchReservation, pty::ShellLaunch, sessions::ProviderBinding};
use uuid::Uuid;

use crate::{
    codex_catalog::Catalog,
    codex_resume,
    environment_runtime::{EnvironmentTarget, probe_environment},
    fork_worktree::{
        ForkIdentity, ForkLease, ForkStage, ForkWorktreePlan, NewWorktree, WorktreeBinding,
    },
};

pub struct CodexForkSource<'a> {
    pub provider: &'a str,
    pub cwd: &'a Path,
    pub codex_home: &'a Path,
    pub binding: &'a ProviderBinding,
}

pub enum CodexForkTarget {
    Current,
    NewWorktree(NewWorktree),
}

pub struct CodexForkSettings<'a> {
    pub data_root: &'a Path,
    pub environment: BTreeMap<OsString, OsString>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexForkAuthority {
    pub lease: ForkLease,
    pub run_id: Uuid,
}

pub struct CodexForkLaunch {
    pub authority: CodexForkAuthority,
    pub launch: ShellLaunch,
    pub permission_mode: &'static str,
    pub permission_warning: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexForkBinding {
    pub identity: ForkIdentity,
    pub provider_session_id: Uuid,
    pub cwd: PathBuf,
    pub provider_config_id: String,
    pub model: String,
    pub permission_mode: &'static str,
    pub inherited_conversation: bool,
}

/// Creation metadata from a Codex rollout header. File modification times are
/// insufficient to associate a rollout with the current launch.
pub struct CodexForkCandidate<'a> {
    pub provider_session_id: &'a str,
    pub forked_from_id: &'a str,
    pub cwd: &'a Path,
    pub created_at_ms: u64,
}

pub struct CodexForkHook<'a> {
    pub event_name: &'a str,
    pub provider_session_id: &'a str,
    pub cwd: &'a Path,
}

/// Kept by the runtime owner. Durable intent and lease persistence belong to
/// the host integration; this coordinator performs no database migrations.
#[derive(Default)]
pub struct CodexForks {
    intents: HashMap<String, CodexForkIntent>,
}

pub struct CodexForkIntent {
    identity: ForkIdentity,
    source_cwd: PathBuf,
    source_binding: ProviderBinding,
    parent: Uuid,
    worktree: Option<ForkWorktreePlan>,
    worktree_binding: Option<WorktreeBinding>,
    stage: ForkStage,
    fence: u64,
    active: Option<ActiveAttempt>,
    rejected_ids: HashSet<Uuid>,
    error: Option<String>,
}

struct ActiveAttempt {
    authority: CodexForkAuthority,
    started_at_ms: u64,
    candidate: Option<Uuid>,
    cwd: PathBuf,
    provider_config_id: String,
    model: String,
    permission_mode: &'static str,
}

impl CodexForks {
    /// Replays an accepted submission before checking the source or Git state.
    /// The same key always addresses the original child and frozen settings.
    pub fn accept(
        &mut self,
        identity: ForkIdentity,
        source: CodexForkSource<'_>,
        target: CodexForkTarget,
        reservations: &[BranchReservation<'_>],
    ) -> Result<&mut CodexForkIntent> {
        let key = identity.submission_key.clone();
        if !self.intents.contains_key(&key) {
            identity.validate()?;
            ensure!(
                self.intents
                    .values()
                    .all(
                        |intent| intent.identity.operation_id != identity.operation_id
                            && intent.identity.session_id != identity.session_id
                    ),
                "Fork operation or child session already exists"
            );
            let binding = source.binding;
            let parent =
                provider_id(&binding.provider_session_id).context("Invalid Codex source UUID")?;
            ensure!(
                source.provider == "codex"
                    && binding.resumable
                    && binding.restore_state == "none"
                    && binding.metadata["canFork"].as_bool() == Some(true),
                "Codex Fork source is not ready"
            );
            let source_cwd = ready_cwd(source.cwd)?;
            Catalog::new(Some(source.codex_home.to_owned()))
                .load(
                    source_cwd
                        .to_str()
                        .context("Invalid Codex source directory")?,
                    &parent.to_string(),
                )
                .map_err(|error| anyhow!("Codex Fork source: {error}"))?;
            let worktree = match target {
                CodexForkTarget::Current => None,
                CodexForkTarget::NewWorktree(target) => {
                    let plan = ForkWorktreePlan::reserve(
                        identity.clone(),
                        &source_cwd,
                        target,
                        reservations,
                    )?;
                    ensure!(
                        !self
                            .intents
                            .values()
                            .filter_map(|intent| intent.worktree.as_ref())
                            .any(|reserved| plan.conflicts_with(reserved)),
                        "Branch reserved by another Fork"
                    );
                    Some(plan)
                }
            };
            let intent = CodexForkIntent {
                identity,
                source_cwd,
                source_binding: ProviderBinding {
                    id: binding.id.clone(),
                    provider_session_id: parent.to_string(),
                    metadata: binding.metadata.clone(),
                    resumable: binding.resumable,
                    restore_state: binding.restore_state.clone(),
                },
                parent,
                worktree,
                worktree_binding: None,
                stage: ForkStage::Queued,
                fence: 0,
                active: None,
                rejected_ids: HashSet::new(),
                error: None,
            };
            self.intents.insert(key.clone(), intent);
        }
        Ok(self.intents.get_mut(&key).expect("accepted Fork intent"))
    }

    pub fn get_mut(&mut self, submission_key: &str) -> Option<&mut CodexForkIntent> {
        self.intents.get_mut(submission_key)
    }
}

impl CodexForkIntent {
    pub fn identity(&self) -> &ForkIdentity {
        &self.identity
    }

    pub fn stage(&self) -> ForkStage {
        self.stage
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn worktree_binding(&self) -> Option<&WorktreeBinding> {
        self.worktree_binding.as_ref()
    }

    /// Rehydrate only before the first launch in this owner. The binding is
    /// authoritative storage's already-committed checkout, under this intent.
    pub(crate) fn resume_persisted(
        &mut self,
        fence: u64,
        worktree: Option<WorktreeBinding>,
    ) -> Result<()> {
        ensure!(
            self.stage == ForkStage::Queued && self.fence == 0 && self.active.is_none(),
            "Codex Fork has already started"
        );
        if let Some(binding) = &worktree {
            ensure!(
                binding.identity == self.identity && self.worktree.is_none(),
                "Stored Worktree does not belong to the Codex Fork"
            );
            binding.verify()?;
        }
        self.fence = fence;
        self.worktree_binding = worktree;
        Ok(())
    }

    /// Starts a queued intent or explicitly retries a failed one. The owner
    /// launches the returned command and feeds its hooks and exit back here.
    pub fn begin(
        &mut self,
        retry: bool,
        settings: CodexForkSettings<'_>,
        mut checkpoint: impl FnMut(ForkStage, &ForkLease) -> Result<()>,
        setup: impl FnOnce(&WorktreeBinding, &ForkLease) -> Result<()>,
        bind_worktree: impl FnOnce(&WorktreeBinding, &ForkLease) -> Result<()>,
    ) -> Result<CodexForkLaunch> {
        ensure!(
            (self.stage == ForkStage::Queued && !retry)
                || (self.stage == ForkStage::Failed && retry),
            "Fork is not queued or eligible for retry"
        );
        ensure!(self.active.is_none(), "Fork attempt still needs cleanup");
        self.fence += 1;
        self.error = None;
        let authority = CodexForkAuthority {
            lease: ForkLease {
                operation_id: self.identity.operation_id.clone(),
                token: Uuid::new_v4(),
                fence: self.fence,
            },
            run_id: Uuid::new_v4(),
        };
        let result = (|| {
            let lease = &authority.lease;
            let cwd = if let Some(binding) = &self.worktree_binding {
                binding.verify()?;
                binding.cwd.clone()
            } else if let Some(plan) = &self.worktree {
                let binding = plan.execute(
                    lease,
                    |stage, lease| {
                        checkpoint(stage, lease)?;
                        self.stage = stage;
                        Ok(())
                    },
                    setup,
                    bind_worktree,
                )?;
                let cwd = binding.cwd.clone();
                self.worktree_binding = Some(binding);
                // The worktree binding already committed this transition.
                self.stage = ForkStage::RestoringProvider;
                cwd
            } else {
                ready_cwd(&self.source_cwd)?
            };
            if self.stage != ForkStage::RestoringProvider {
                checkpoint(ForkStage::RestoringProvider, lease)?;
                self.stage = ForkStage::RestoringProvider;
            }
            let (mut plan, _) = codex_resume::prepare(
                settings.data_root,
                &cwd,
                Some(&self.source_binding),
                true,
                settings.environment,
                settings.cols,
                settings.rows,
            )?;
            plan.launch.argv.extend([
                "-c".into(),
                "tui.resume_cwd=\"current\"".into(),
                "fork".into(),
                self.source_binding.provider_session_id.clone(),
            ]);
            checkpoint(ForkStage::StartingWindow, lease)?;
            let started_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .try_into()?;
            self.stage = ForkStage::StartingWindow;
            self.active = Some(ActiveAttempt {
                authority: authority.clone(),
                started_at_ms,
                candidate: None,
                cwd,
                provider_config_id: plan.provider_config_id,
                model: plan.model,
                permission_mode: plan.permission_mode,
            });
            Ok(CodexForkLaunch {
                authority,
                launch: plan.launch,
                permission_mode: plan.permission_mode,
                permission_warning: plan.permission_warning,
            })
        })();
        if let Err(error) = &result {
            self.mark_failed(error);
        }
        result
    }

    /// A single rollout with matching lineage, cwd and creation time can pin
    /// the expected child UUID. Only the current run's SessionStart confirms it.
    pub fn discover_candidate(
        &mut self,
        authority: &CodexForkAuthority,
        rollouts: &[CodexForkCandidate<'_>],
    ) -> Option<Uuid> {
        let active = self.active.as_mut().filter(|active| {
            self.stage == ForkStage::StartingWindow && active.authority == *authority
        })?;
        if active.candidate.is_some() {
            return active.candidate;
        }
        let mut candidates = rollouts.iter().filter_map(|rollout| {
            let id = provider_id(rollout.provider_session_id)?;
            (id != self.parent
                && !self.rejected_ids.contains(&id)
                && provider_id(rollout.forked_from_id) == Some(self.parent)
                && rollout.created_at_ms > active.started_at_ms
                && matches_cwd(rollout.cwd, &active.cwd))
            .then_some(id)
        });
        let first = candidates.next()?;
        if candidates.all(|id| id == first) {
            active.candidate = Some(first);
        }
        active.candidate
    }

    /// The binding callback atomically commits this child's identity under the
    /// current authority. A failed commit or mismatched hook retires the
    /// attempt through the same path as an early process exit.
    pub fn observe_hook(
        &mut self,
        authority: &CodexForkAuthority,
        hook: CodexForkHook<'_>,
        bind: impl FnOnce(&CodexForkBinding, &CodexForkAuthority) -> Result<()>,
        cleanup: impl FnOnce(&CodexForkAuthority) -> Result<()>,
    ) -> Result<Option<CodexForkBinding>> {
        let Some(active) = self.active.as_ref().filter(|active| {
            self.stage == ForkStage::StartingWindow
                && active.authority == *authority
                && hook.event_name == "SessionStart"
        }) else {
            return Ok(None);
        };
        let actual = provider_id(hook.provider_session_id);
        let result = (|| {
            let id = actual.context("Invalid Codex Fork UUID")?;
            ensure!(
                id != self.parent
                    && !self.rejected_ids.contains(&id)
                    && active.candidate.is_none_or(|expected| expected == id)
                    && matches_cwd(hook.cwd, &active.cwd),
                "Codex Fork SessionStart identity or directory mismatch"
            );
            let binding = CodexForkBinding {
                identity: self.identity.clone(),
                provider_session_id: id,
                cwd: active.cwd.clone(),
                provider_config_id: active.provider_config_id.clone(),
                model: active.model.clone(),
                permission_mode: active.permission_mode,
                inherited_conversation: true,
            };
            bind(&binding, authority)?;
            Ok(binding)
        })();
        match result {
            Ok(binding) => {
                self.active = None;
                self.stage = ForkStage::Succeeded;
                Ok(Some(binding))
            }
            Err(error) => {
                self.rejected_ids
                    .extend(actual.filter(|id| *id != self.parent));
                match self.fail(authority, &error, cleanup) {
                    Ok(_) => Err(error),
                    Err(cleanup) => {
                        Err(error.context(format!("Codex Fork cleanup incomplete: {cleanup:#}")))
                    }
                }
            }
        }
    }

    /// Handles a spawn error, process exit or timeout. Cleanup is fenced to
    /// this run (PTY, hook registration and provisional binding). An already
    /// bound worktree stays available across attempts.
    pub fn fail(
        &mut self,
        authority: &CodexForkAuthority,
        error: &anyhow::Error,
        cleanup: impl FnOnce(&CodexForkAuthority) -> Result<()>,
    ) -> Result<bool> {
        let Some(active) = self
            .active
            .as_ref()
            .filter(|active| active.authority == *authority)
        else {
            return Ok(false);
        };
        self.rejected_ids.extend(active.candidate);
        if self.stage != ForkStage::Failed {
            self.mark_failed(error);
        }
        cleanup(authority)?;
        self.active = None;
        Ok(true)
    }

    fn mark_failed(&mut self, error: &anyhow::Error) {
        self.stage = ForkStage::Failed;
        self.error = Some(format!("{error:#}"));
    }
}

fn provider_id(value: &str) -> Option<Uuid> {
    (value.len() == 36)
        .then(|| Uuid::parse_str(value).ok())
        .flatten()
}

fn ready_cwd(path: &Path) -> Result<PathBuf> {
    let ready = probe_environment(EnvironmentTarget::Local(path))
        .map_err(|problem| anyhow!("Codex Fork directory unavailable: {problem:?}"))?;
    Ok(ready.cwd)
}

fn matches_cwd(actual: &Path, expected: &Path) -> bool {
    actual.is_absolute() && fs::canonicalize(actual).is_ok_and(|path| path == expected)
}

#[cfg(test)]
mod tests {
    use super::provider_id;

    #[test]
    fn codex_fork_uuid_requires_the_provider_format_and_compares_case_insensitively() {
        assert_eq!(
            provider_id("12345678-1234-4123-8123-1234567890ab"),
            provider_id("12345678-1234-4123-8123-1234567890AB")
        );
        assert!(provider_id("12345678-1234-4123-8123-1234567890ab").is_some());
        assert!(provider_id("123456781234412381231234567890ab").is_none());
        assert!(provider_id("urn:uuid:12345678-1234-4123-8123-1234567890ab").is_none());
    }
}
