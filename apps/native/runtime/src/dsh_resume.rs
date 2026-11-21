//! One restore handshake per card. The caller owns process lifetime and storage.

use std::path::Path;

use serde_json::Value;

use crate::{
    dsh_catalog::{CatalogError, DshSessionCatalog},
    dsh_events::{DshEventOutcome, DshEvents},
    dsh_launch::DshLaunchMode,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeFailure {
    HistoryMissing,
    HistoryCorrupt,
    HistoryUnreadable,
    IdentityMismatch { expected: String, actual: String },
    ExpiredIdentity { session_id: String },
    IdentityTimeout,
    ProcessExited,
    LaunchFailed,
}

impl From<CatalogError> for ResumeFailure {
    fn from(error: CatalogError) -> Self {
        match error {
            CatalogError::Missing => Self::HistoryMissing,
            CatalogError::Corrupt | CatalogError::InvalidId => Self::HistoryCorrupt,
            CatalogError::UnsupportedEvent | CatalogError::ReadLimit | CatalogError::Io(_) => {
                Self::HistoryUnreadable
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingState {
    Available,
    Failed,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeState {
    Idle,
    Starting(DshLaunchMode),
    Failed(ResumeFailure),
    Ready { session_id: String, persisted: bool },
}

#[derive(Debug, PartialEq, Eq)]
pub enum BeginOutcome {
    /// Sole permission to dispatch a launch for this card.
    Launch(DshLaunchMode),
    InFlight,
    Parked,
    Unavailable,
}

struct Run {
    id: String,
    events: DshEvents,
}

/// Keep one instance per card and serialize its methods through the card owner.
/// Run IDs must be unique across launches. Keeping the original binding
/// separate lets the caller retain expired history after a fresh launch.
pub struct DshResume {
    original_id: String,
    binding_state: BindingState,
    state: ResumeState,
    run: Option<Run>,
}

impl DshResume {
    pub fn new(provider_session_id: impl Into<String>) -> Self {
        Self {
            original_id: provider_session_id.into(),
            binding_state: BindingState::Available,
            state: ResumeState::Idle,
            run: None,
        }
    }

    pub fn from_failed(provider_session_id: impl Into<String>, reason: ResumeFailure) -> Self {
        let mut restore = Self::new(provider_session_id);
        restore.park(reason);
        restore
    }

    pub fn state(&self) -> &ResumeState {
        &self.state
    }

    pub fn original_binding(&self) -> (&str, BindingState) {
        (&self.original_id, self.binding_state)
    }

    pub fn start(
        &mut self,
        run_id: &str,
        catalog: &mut DshSessionCatalog,
        cwd: &Path,
    ) -> BeginOutcome {
        match self.state {
            ResumeState::Idle => self.start_resume(run_id, catalog, cwd),
            ResumeState::Starting(_) => BeginOutcome::InFlight,
            ResumeState::Failed(_) => BeginOutcome::Parked,
            ResumeState::Ready { .. } => BeginOutcome::Unavailable,
        }
    }

    pub fn retry(
        &mut self,
        run_id: &str,
        catalog: &mut DshSessionCatalog,
        cwd: &Path,
    ) -> BeginOutcome {
        if matches!(self.state, ResumeState::Starting(_)) {
            return BeginOutcome::InFlight;
        }
        if self.binding_state != BindingState::Failed {
            return BeginOutcome::Unavailable;
        }
        self.start_resume(run_id, catalog, cwd)
    }

    pub fn start_fresh(&mut self, run_id: &str) -> BeginOutcome {
        if matches!(self.state, ResumeState::Starting(_)) {
            return BeginOutcome::InFlight;
        }
        if self.binding_state != BindingState::Failed || !self.new_run_id(run_id) {
            return BeginOutcome::Unavailable;
        }
        self.binding_state = BindingState::Expired;
        self.begin(run_id, DshLaunchMode::Fresh)
    }

    fn new_run_id(&self, run_id: &str) -> bool {
        !run_id.is_empty() && self.run.as_ref().is_none_or(|run| run.id != run_id)
    }

    fn start_resume(
        &mut self,
        run_id: &str,
        catalog: &mut DshSessionCatalog,
        cwd: &Path,
    ) -> BeginOutcome {
        if !self.new_run_id(run_id) {
            return BeginOutcome::Unavailable;
        }
        if let Err(error) = catalog.validate_history(cwd, &self.original_id) {
            self.park(error.into());
            return BeginOutcome::Parked;
        }
        self.begin_bound_resume(run_id)
    }

    /// The runtime already holds a validated binding for this running card.
    /// Its catalog entry can still be indexed by the previous directory.
    pub(crate) fn start_for_environment(&mut self, run_id: &str) -> BeginOutcome {
        if self.state != ResumeState::Idle || !self.new_run_id(run_id) {
            return BeginOutcome::Unavailable;
        }
        self.begin_bound_resume(run_id)
    }

    fn begin_bound_resume(&mut self, run_id: &str) -> BeginOutcome {
        self.binding_state = BindingState::Available;
        self.begin(
            run_id,
            DshLaunchMode::Resume {
                expected_session_id: self.original_id.clone(),
            },
        )
    }

    fn begin(&mut self, run_id: &str, mode: DshLaunchMode) -> BeginOutcome {
        self.run = Some(Run {
            id: run_id.to_owned(),
            events: DshEvents::new(run_id, mode.expected_session_id()),
        });
        self.state = ResumeState::Starting(mode.clone());
        BeginOutcome::Launch(mode)
    }

    /// The hook caller authenticates the payload. DshEvents checks the native
    /// ready flag, foreground, generation, sequence, and expected identity.
    /// The accepted update can also feed the card's title and HUD observers.
    pub fn observe<'a>(&mut self, run_id: &str, payload: &'a Value) -> DshEventOutcome<'a> {
        let Some(run) = self.run.as_mut().filter(|run| run.id == run_id) else {
            return DshEventOutcome::Ignored;
        };
        if !matches!(
            self.state,
            ResumeState::Starting(_) | ResumeState::Ready { .. }
        ) {
            return DshEventOutcome::Ignored;
        }
        let outcome = run.events.ingest(payload);
        match &outcome {
            DshEventOutcome::IdentityMismatch {
                expected_session_id,
                actual_session_id,
            } => {
                self.park(ResumeFailure::IdentityMismatch {
                    expected: expected_session_id.clone(),
                    actual: (*actual_session_id).to_owned(),
                });
            }
            DshEventOutcome::Observed(update) => {
                if self.binding_state == BindingState::Expired
                    && update.session_id == self.original_id
                {
                    if matches!(self.state, ResumeState::Starting(_)) {
                        self.park(ResumeFailure::ExpiredIdentity {
                            session_id: update.session_id.to_owned(),
                        });
                    }
                    return DshEventOutcome::Ignored;
                }
                if let Some(identity) = &update.identity {
                    self.state = ResumeState::Ready {
                        session_id: identity.session_id.to_owned(),
                        persisted: identity.persisted,
                    };
                }
            }
            DshEventOutcome::Ignored => {}
        }
        outcome
    }

    /// A restore timeout applies to the handshake for a saved conversation.
    /// A fresh run can legitimately report a provisional identity.
    pub fn timeout(&mut self, run_id: &str) -> bool {
        if !matches!(
            self.state,
            ResumeState::Starting(DshLaunchMode::Resume { .. })
        ) {
            return false;
        }
        self.fail_start(run_id, ResumeFailure::IdentityTimeout)
    }

    /// Called for an explicit process or launch failure before its ready event.
    pub fn fail_start(&mut self, run_id: &str, reason: ResumeFailure) -> bool {
        if !matches!(self.state, ResumeState::Starting(_))
            || !self.run.as_ref().is_some_and(|run| run.id == run_id)
        {
            return false;
        }
        self.park(reason);
        true
    }

    fn park(&mut self, reason: ResumeFailure) {
        if self.binding_state != BindingState::Expired {
            self.binding_state = BindingState::Failed;
        }
        self.state = ResumeState::Failed(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_ready_cannot_reactivate_an_expired_binding() {
        let mut restore = DshResume::from_failed("old", ResumeFailure::HistoryCorrupt);
        assert_eq!(
            restore.start_fresh("fresh"),
            BeginOutcome::Launch(DshLaunchMode::Fresh)
        );
        let old = serde_json::json!({
            "dsh_observation": 1, "run_id": "fresh", "foreground": true, "generation": 1,
            "sequence": 1, "session_id": "old", "event_type": "ready", "ready": true,
            "persisted": true
        });
        assert_eq!(restore.observe("fresh", &old), DshEventOutcome::Ignored);
        assert_eq!(
            restore.state(),
            &ResumeState::Failed(ResumeFailure::ExpiredIdentity {
                session_id: "old".into()
            })
        );
        assert_eq!(restore.original_binding(), ("old", BindingState::Expired));
    }

    #[test]
    fn timeout_only_parks_the_current_restore() {
        let mut restore = DshResume::new("saved");
        restore.begin(
            "current",
            DshLaunchMode::Resume {
                expected_session_id: "saved".into(),
            },
        );
        assert!(!restore.timeout("old"));
        assert!(restore.timeout("current"));
        assert_eq!(
            restore.state(),
            &ResumeState::Failed(ResumeFailure::IdentityTimeout)
        );
        assert!(!restore.timeout("current"));
        assert!(matches!(
            restore.start_fresh("next"),
            BeginOutcome::Launch(DshLaunchMode::Fresh)
        ));
        assert!(!restore.timeout("next"));
        assert_eq!(restore.original_binding(), ("saved", BindingState::Expired));
    }
}
