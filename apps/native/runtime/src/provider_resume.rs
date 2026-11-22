use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use matou_native_core::{pty::LaunchProfile, sessions::ClaudeBinding};
use regex::Regex;

pub(crate) const IDENTITY_MISMATCH: &str = "The context returned by the AI session does not match the session being restored; try restoring again";

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
static MISSING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"nosession.{0,96}found|session.{0,96}notfound|noconversation.{0,96}found|invalidsession|failedtoresume").unwrap()
});
static TRUST_OPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^| )([>❯] *)?(?:[12][.)] *)?(no, exit|yes, i trust this folder)").unwrap()
});

const CLAUDE_WORKSPACE_TRUST_PROMPT_MARKERS: &[&str] = &[
    "accessing workspace:",
    "quick safety check:",
    "claude code'll be able to read, edit, and execute files here.",
    "no, exit",
    "yes, i trust this folder",
    "enter to confirm",
];
const ANTIGRAVITY_WORKSPACE_TRUST_PROMPT_MARKERS: &[&str] = &[
    "accessing workspace:",
    "do you trust the contents of this project?",
    "antigravity cli requires permission to read, edit, and execute files here.",
    "yes, i trust this folder",
    "no, exit",
    "enter confirm",
];

pub(crate) struct ResumeMonitor {
    pub binding_id: Option<String>,
    pub expected_id: Option<String>,
    pub permission_respawn: bool,
    recent: String,
    has_visible_output: bool,
    watching_failure_output: bool,
    full_prompt_handled: bool,
    trust_provider: LaunchProfile,
    trust: Option<Trust>,
    bypass_consent: Option<BypassConsent>,
}

#[derive(PartialEq, Eq)]
enum BypassConsent {
    Watching,
    Waiting,
    Submitted,
}

#[derive(Clone, Copy)]
enum Trust {
    Watching,
    Select { at: Instant, move_down: bool },
    Confirm(Instant),
}

impl ResumeMonitor {
    pub(crate) fn new(binding: &ClaudeBinding) -> Self {
        let mut monitor = Self::for_id(binding.provider_session_id.clone());
        monitor.binding_id = Some(binding.id.clone());
        monitor
    }

    pub(crate) fn for_permission_respawn(
        binding: Option<&ClaudeBinding>,
        bypass_permissions: bool,
    ) -> Self {
        let mut monitor = match binding {
            Some(binding) => Self::new(binding),
            None => Self::for_fresh_trust(LaunchProfile::ClaudeCode),
        };
        monitor.permission_respawn = true;
        monitor.bypass_consent = bypass_permissions.then_some(BypassConsent::Watching);
        monitor
    }

    pub(crate) fn for_id(expected_id: String) -> Self {
        let mut monitor = Self::for_fresh_trust(LaunchProfile::ClaudeCode);
        monitor.expected_id = Some(expected_id);
        monitor.watching_failure_output = true;
        monitor
    }

    pub(crate) fn for_fresh_trust(provider: LaunchProfile) -> Self {
        Self {
            binding_id: None,
            expected_id: None,
            permission_respawn: false,
            recent: String::new(),
            has_visible_output: false,
            watching_failure_output: false,
            full_prompt_handled: false,
            trust_provider: provider,
            trust: Some(Trust::Watching),
            bypass_consent: None,
        }
    }

    pub(crate) fn tracks_restore(&self) -> bool {
        self.expected_id.is_some() || self.permission_respawn
    }

    pub(crate) fn stop_trust(&mut self) {
        self.trust = None;
    }

    pub(crate) fn fresh_permission(&self) -> bool {
        self.permission_respawn && self.expected_id.is_none()
    }

    pub(crate) fn ingest(&mut self, data: &str) -> (Option<&'static str>, Option<&'static [u8]>) {
        if !self.tracks_restore() && self.trust.is_none() {
            return (None, None);
        }
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
        if self.bypass_consent == Some(BypassConsent::Watching)
            && has(&[
                "warning: claude code running in bypass permissions mode",
                "no, exit",
                "yes, i accept",
                "enter to confirm",
                "esc to cancel",
            ])
        {
            self.bypass_consent = Some(BypassConsent::Waiting);
        }
        let markers = match self.trust_provider {
            LaunchProfile::Antigravity => ANTIGRAVITY_WORKSPACE_TRUST_PROMPT_MARKERS,
            _ => CLAUDE_WORKSPACE_TRUST_PROMPT_MARKERS,
        };
        let prompt = output.rsplit(markers[0]).next().unwrap_or_default();
        if matches!(self.trust, Some(Trust::Watching | Trust::Select { .. }))
            && output.contains(markers[0])
            && markers[1..].iter().all(|marker| prompt.contains(marker))
        {
            let move_down = self.trust_provider == LaunchProfile::ClaudeCode && {
                let mut selected_yes = false;
                let mut selected_no = false;
                for option in TRUST_OPTION.captures_iter(prompt) {
                    if &option[2] == "no, exit" {
                        selected_no = option.get(1).is_some();
                    } else {
                        selected_yes = option.get(1).is_some();
                    }
                }
                !selected_yes || selected_no
            };
            let at = match self.trust {
                Some(Trust::Select { at, .. }) => at,
                _ => Instant::now() + Duration::from_millis(500),
            };
            self.trust = Some(Trust::Select { at, move_down });
        }
        if self.expected_id.is_some() && !self.full_prompt_handled {
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

    pub(crate) fn waiting_for_bypass_consent(&self) -> bool {
        self.bypass_consent == Some(BypassConsent::Waiting)
    }

    pub(crate) fn saw_bypass_consent(&self) -> bool {
        matches!(
            self.bypass_consent,
            Some(BypassConsent::Waiting | BypassConsent::Submitted)
        )
    }

    pub(crate) fn submit_bypass_consent(&mut self) {
        self.bypass_consent = Some(BypassConsent::Submitted);
    }

    pub(crate) fn release_readiness(&mut self) {
        if self.has_visible_output {
            self.watching_failure_output = false;
        }
    }

    fn failure_in(&self, output: &str) -> Option<&'static str> {
        let id = self.expected_id.as_ref()?.to_ascii_lowercase();
        // Terminal wrapping and cursor moves can split both the error text and the ID.
        let compact: String = output.split_whitespace().collect();
        if id.len() == 36
            && uuid::Uuid::parse_str(&id).is_ok()
            && compact.contains(&format!(
                "thatsessionisrunninginthebackground({}).",
                &id[..8]
            ))
        {
            return Some("provider session running in background");
        }
        let at = compact.rfind(&id)?;
        let start = compact.floor_char_boundary(at.saturating_sub(320));
        let end = compact.ceil_char_boundary((at + id.len() + 320).min(compact.len()));
        MISSING
            .is_match(&compact[start..end])
            .then_some("provider session not found")
    }

    pub(crate) fn due_input(&mut self) -> Option<&'static [u8]> {
        self.due_input_at(Instant::now())
    }

    fn due_input_at(&mut self, now: Instant) -> Option<&'static [u8]> {
        match self.trust {
            Some(Trust::Select {
                at,
                move_down: true,
            }) if at <= now => {
                self.trust = Some(Trust::Confirm(now + Duration::from_millis(150)));
                Some(b"\x1b[B")
            }
            Some(
                Trust::Select {
                    at,
                    move_down: false,
                }
                | Trust::Confirm(at),
            ) if at <= now => {
                self.stop_trust();
                Some(b"\r")
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "7ec4132a-f6f8-4b49-9f6d-4b07be37f5c2";
    const CLAUDE_TRUST_HEADER: &str = "Accessing workspace: /tmp/project\r\n\
        Quick safety check: Is this a project you created or one you trust?\r\n\
        Claude Code'll be able to read, edit, and execute files here.\r\n";

    #[test]
    fn claude_trust_respects_both_highlighted_rows_and_redraws() {
        let no = format!(
            "{CLAUDE_TRUST_HEADER}❯ 1. No, exit\r\n  2. Yes, I trust this folder\r\nEnter to confirm"
        );
        let yes = format!(
            "{CLAUDE_TRUST_HEADER}  1. No, exit\r\n\x1b[32m❯\x1b[0m 2. Yes, I trust this folder\r\nEnter to confirm"
        );
        let selection_delay = Duration::from_millis(500);
        let mut fresh = ResumeMonitor::for_fresh_trust(LaunchProfile::ClaudeCode);
        assert!(!fresh.tracks_restore());
        assert!(!fresh.fresh_permission());
        let before = Instant::now();
        fresh.ingest(&no);
        assert_eq!(
            fresh.due_input_at(before + selection_delay - Duration::from_millis(1)),
            None
        );
        let due = Instant::now() + selection_delay;
        assert_eq!(fresh.due_input_at(due), Some(b"\x1b[B".as_slice()));
        assert_eq!(fresh.due_input_at(due + Duration::from_millis(149)), None);
        assert_eq!(
            fresh.due_input_at(due + Duration::from_millis(150)),
            Some(b"\r".as_slice())
        );
        fresh.ingest(&no);
        assert_eq!(fresh.due_input_at(due + Duration::from_secs(1)), None);

        let mut fresh = ResumeMonitor::for_fresh_trust(LaunchProfile::ClaudeCode);
        fresh.ingest(&yes);
        assert_eq!(
            fresh.due_input_at(Instant::now() + selection_delay),
            Some(b"\r".as_slice())
        );

        let mut restoring = ResumeMonitor::for_id("restored-conversation".into());
        assert!(restoring.tracks_restore());
        restoring.ingest(&no);
        restoring.ingest("\r\n  1. No, exit\r\n❯ 2. Yes, I trust this folder");
        assert_eq!(
            restoring.due_input_at(Instant::now() + selection_delay),
            Some(b"\r".as_slice())
        );
    }

    #[test]
    fn antigravity_trust_confirms_directly_but_dialogue_and_completed_runs_get_no_input() {
        let prompt = "Accessing workspace: /tmp/project\r\n\
            Do you trust the contents of this project?\r\n\
            Antigravity CLI requires permission to read, edit, and execute files here.\r\n\
            > Yes, I trust this folder\r\nNo, exit\r\n↑/↓ Navigate · enter Confirm";
        let mut monitor = ResumeMonitor::for_fresh_trust(LaunchProfile::Antigravity);
        monitor.ingest("The instructions say Yes, I trust this folder.");
        assert_eq!(
            monitor.due_input_at(Instant::now() + Duration::from_secs(1)),
            None
        );
        monitor.ingest(prompt);
        assert_eq!(
            monitor.due_input_at(Instant::now() + Duration::from_millis(500)),
            Some(b"\r".as_slice())
        );

        let mut monitor = ResumeMonitor::for_fresh_trust(LaunchProfile::ClaudeCode);
        monitor.ingest("The instructions say > Yes, I trust this folder. Enter to confirm.");
        assert_eq!(
            monitor.due_input_at(Instant::now() + Duration::from_secs(1)),
            None
        );
        monitor.ingest(&format!(
            "{CLAUDE_TRUST_HEADER}> No, exit\r\nYes, I trust this folder\r\nEnter to confirm"
        ));
        monitor.stop_trust();
        assert_eq!(
            monitor.due_input_at(Instant::now() + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn startup_failures_are_scoped_to_the_requested_conversation() {
        let id = SESSION_ID;
        let binding = ClaudeBinding {
            id: "binding".into(),
            provider_session_id: id.into(),
            metadata: serde_json::Value::Null,
            resumable: true,
            restore_state: "restoring".into(),
        };
        let mut monitor = ResumeMonitor::new(&binding);
        assert_eq!(monitor.ingest("No\x1b[5Gconver").0, None);
        assert_eq!(
            monitor
                .ingest(&format!("sation\x1b[20Gfound with session ID: {id}"))
                .0,
            Some("provider session not found")
        );

        let mut monitor = ResumeMonitor::new(&binding);
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

        let mut monitor = ResumeMonitor::new(&binding);
        monitor.ingest("Interactive prompt");
        monitor.release_readiness();
        assert_eq!(
            monitor
                .ingest(&format!("No conversation found with session ID: {id}"))
                .0,
            None
        );

        let mut monitor = ResumeMonitor::new(&binding);
        monitor.ingest("\x1b[?25l");
        monitor.release_readiness();
        assert_eq!(
            monitor
                .ingest(&format!("No conversation found with session ID: {id}"))
                .0,
            Some("provider session not found")
        );
    }

    #[test]
    fn missing_session_is_recognized_with_a_wrapped_uuid() {
        for reason in [
            "No session found",
            "Session not found",
            "No conversation found",
            "Invalid session",
            "Failed to resume",
        ] {
            let mut monitor = ResumeMonitor::for_id(SESSION_ID.into());
            assert_eq!(
                monitor.ingest(&format!("{reason}: {SESSION_ID}")).0,
                Some("provider session not found"),
                "{reason}"
            );
        }
        for separator in ["\n", "\x1b[2G"] {
            for at in 1..SESSION_ID.len() {
                let (left, right) = SESSION_ID.split_at(at);
                let mut monitor = ResumeMonitor::for_id(SESSION_ID.to_ascii_uppercase());
                let output = format!(
                    "No conver{separator}sation found with session ID: {left}{separator}{right}"
                );
                assert_eq!(
                    monitor.ingest(&output).0,
                    Some("provider session not found"),
                    "ID split at {at} by {separator:?}"
                );
            }
        }

        for output in [
            "No conver\nsation found with session ID: 7ec4132a-f6f8-4b49-9\x1b[2Gf6d-4b07be37f5c3"
                .into(),
            format!("Session ID: {SESSION_ID}"),
            format!("No conversation found {} {SESSION_ID}", "界".repeat(330)),
        ] {
            let mut monitor = ResumeMonitor::for_id(SESSION_ID.into());
            assert_eq!(monitor.ingest(&output).0, None, "{output:?}");
        }
    }

    #[test]
    fn background_session_is_recognized_with_wrapped_output() {
        let background = "That session is running in the background (7ec4132a).";
        for separator in ["\n", "\x1b[2G"] {
            for at in 1..background.len() {
                let (left, right) = background.split_at(at);
                let mut monitor = ResumeMonitor::for_id(SESSION_ID.into());
                assert_eq!(
                    monitor.ingest(&format!("{left}{separator}{right}")).0,
                    Some("provider session running in background"),
                    "Background message split at {at} by {separator:?}"
                );
            }
        }

        for output in [
            "That session is running in the back\nground (7ec4\x1b[2G132b).",
            "That session is running in the back\nground (7ec4\x1b[2G132aff).",
        ] {
            let mut monitor = ResumeMonitor::for_id(SESSION_ID.into());
            assert_eq!(monitor.ingest(output).0, None, "{output:?}");
        }

        let mut monitor = ResumeMonitor::for_id("7ec4132a-not-a-uuid".into());
        assert_eq!(monitor.ingest(background).0, None);
    }

    #[test]
    fn recorded_32_column_claude_error_is_recognized_on_arrival() {
        // PTY reads from cli-fgw0b807/lost-resume-32cols.tty and its .reads.json.
        // Only the UUID bytes were replaced.
        let chunks = [
            "\x1b7\x1b[r\x1b8\x1b[?25h",
            "\x1b[38;5;211mNo\x1b[4Gconversation\x1b[17Gfound\x1b[23Gwith\x1b[39m\r\r\n\
             \x1b[38;5;211msession\x1b[9GID:\x1b[13G7ec4132a-f6f8-4b49-9\x1b[39m\r\r\n\
             \x1b[38;5;211mf6d-4b07be37f5c2\x1b[39m\r\r\n",
            "\x1b[?25h",
            "\x1b(B\x0f\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[>4m\x1b[?1004l\
             \x1b[?2031l\x1b[?2004l\x1b[?7h\x1b[<u\x1b[?25h\x1b7\x1b[r\x1b8",
            "\x1b[?25h",
            "\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l",
            "\x1b(B\x0f\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[>4m\x1b[?1004l\
             \x1b[?2031l\x1b[?2004l\x1b[?7h\x1b[<u\x1b[?25h\x1b7\x1b[r\x1b8\x1b]0;\x07",
            "\x1b[?25h",
        ];
        let mut monitor = ResumeMonitor::for_id(SESSION_ID.into());
        let first_error = chunks
            .iter()
            .enumerate()
            .find_map(|(index, chunk)| monitor.ingest(chunk).0.map(|error| (index + 1, error)));
        assert_eq!(first_error, Some((2, "provider session not found")));
    }
}
