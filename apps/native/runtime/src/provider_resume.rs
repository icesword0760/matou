use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use matou_native_core::sessions::ClaudeBinding;
use regex::Regex;

pub(crate) const IDENTITY_MISMATCH: &str = "The context returned by the AI session does not match the session being restored; try restoring again";

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
static MISSING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"no session.{0,96}found|session.{0,96}not found|no conversation.{0,96}found|invalid session|failed to resume").unwrap()
});

pub(crate) struct ResumeMonitor {
    pub binding_id: String,
    pub expected_id: String,
    recent: String,
    has_visible_output: bool,
    watching_failure_output: bool,
    full_prompt_handled: bool,
    trust: Option<Trust>,
}

enum Trust {
    Watching,
    Select(Instant),
    Confirm(Instant),
}

impl ResumeMonitor {
    pub(crate) fn new(binding: &ClaudeBinding, bypass_permissions: bool) -> Self {
        Self {
            binding_id: binding.id.clone(),
            expected_id: binding.provider_session_id.clone(),
            recent: String::new(),
            has_visible_output: false,
            watching_failure_output: true,
            full_prompt_handled: false,
            trust: bypass_permissions.then_some(Trust::Watching),
        }
    }

    pub(crate) fn ingest(&mut self, data: &str) -> (Option<&'static str>, Option<&'static [u8]>) {
        self.recent.push_str(data);
        let keep = self
            .recent
            .ceil_char_boundary(self.recent.len().saturating_sub(16_384));
        self.recent.drain(..keep);
        let plain = ANSI.replace_all(&self.recent, |captures: &regex::Captures<'_>| {
            if captures[0].ends_with(['C', 'G']) {
                " "
            } else {
                ""
            }
        });
        let output = plain
            .split(|ch: char| ch.is_control() || ch.is_whitespace())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        self.has_visible_output |= !output.is_empty();
        if self.watching_failure_output {
            if let Some(error) = self.failure_in(&output) {
                return (Some(error), None);
            }
            self.watching_failure_output = output.encode_utf16().count() <= 2_000;
        }
        let has = |markers: &[&str]| markers.iter().all(|marker| output.contains(marker));
        if matches!(self.trust, Some(Trust::Watching))
            && has(&[
                "accessing workspace:",
                "quick safety check:",
                "claude code'll be able to read, edit, and execute files here.",
                "no, exit",
                "yes, i trust this folder",
                "enter to confirm",
            ])
        {
            self.trust = Some(Trust::Select(Instant::now() + Duration::from_millis(500)));
        }
        if !self.full_prompt_handled {
            let selection: Option<&[u8]> = if has(&[
                "resume this conversation?",
                "this conversation has been inactive for",
                "tokens long.",
                "1. resume",
                "2. start a new conversation",
                "enter to confirm",
                "esc to resume",
            ]) {
                Some(b"\r")
            } else if has(&[
                "resuming the full session will consume",
                "resume from summary",
                "resume full session as-is",
                "don't ask me again",
                "enter to confirm",
            ]) {
                Some(b"\x1b[B\r")
            } else {
                None
            };
            if selection.is_some() {
                self.full_prompt_handled = true;
                return (None, selection);
            }
        }
        (None, None)
    }

    pub(crate) fn release_readiness(&mut self) {
        if self.has_visible_output {
            self.watching_failure_output = false;
        }
    }

    fn failure_in(&self, output: &str) -> Option<&'static str> {
        let id = self.expected_id.to_ascii_lowercase();
        if id.len() == 36
            && uuid::Uuid::parse_str(&id).is_ok()
            && output.contains(&format!(
                "that session is running in the background ({}).",
                &id[..8]
            ))
        {
            return Some("provider session running in background");
        }
        if let Some(at) = output.rfind(&id) {
            let start = output.floor_char_boundary(at.saturating_sub(320));
            let end = output.ceil_char_boundary((at + id.len() + 320).min(output.len()));
            if MISSING.is_match(&output[start..end]) {
                return Some("provider session not found");
            }
        }
        None
    }

    pub(crate) fn due_input(&mut self) -> Option<&'static [u8]> {
        let now = Instant::now();
        match self.trust {
            Some(Trust::Select(at)) if at <= now => {
                self.trust = Some(Trust::Confirm(now + Duration::from_millis(150)));
                Some(b"\x1b[B")
            }
            Some(Trust::Confirm(at)) if at <= now => {
                self.trust = None;
                Some(b"\r")
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_failures_are_scoped_to_the_requested_conversation() {
        let id = "7ec4132a-f6f8-4b49-9f6d-4b07be37f5c2";
        let binding = ClaudeBinding {
            id: "binding".into(),
            provider_session_id: id.into(),
            metadata: serde_json::Value::Null,
            resumable: true,
            restore_state: "restoring".into(),
        };
        let mut monitor = ResumeMonitor::new(&binding, false);
        assert_eq!(monitor.ingest("No\x1b[5Gconver").0, None);
        assert_eq!(
            monitor
                .ingest(&format!("sation\x1b[20Gfound with session ID: {id}"))
                .0,
            Some("provider session not found")
        );

        let mut monitor = ResumeMonitor::new(&binding, false);
        assert_eq!(
            monitor
                .ingest("That session is running in the background (01234567).")
                .0,
            None
        );
        assert_eq!(
            monitor
                .ingest("That session is running in the background (7ec4132a).")
                .0,
            Some("provider session running in background")
        );

        let mut monitor = ResumeMonitor::new(&binding, false);
        monitor.ingest("Interactive prompt");
        monitor.release_readiness();
        assert_eq!(
            monitor
                .ingest(&format!("No conversation found with session ID: {id}"))
                .0,
            None
        );

        let mut monitor = ResumeMonitor::new(&binding, false);
        monitor.ingest("\x1b[?25l");
        monitor.release_readiness();
        assert_eq!(
            monitor
                .ingest(&format!("No conversation found with session ID: {id}"))
                .0,
            Some("provider session not found")
        );
    }
}
