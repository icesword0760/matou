//! Synchronous PreToolUse / PostToolUse hook decision engine for
//! `ProviderHookServer`.
//!
//! Tool decisions respect per-agent settings from `ContextOptimizerConfig`
//! and fail open on errors. Compaction also expires cached reads so that
//! enabling read deduplication again cannot reuse pre-compaction results.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

use matou_native_core::{context_optimizer_config::ContextOptimizerConfig, pty::LaunchProfile};
use serde_json::{Map, Value, json};

use crate::context_guard::{
    claude_output::ClaudeOutput,
    dedup::{ReadDecision, ReadDedupGuard},
    filter::{HARD_CAP_BYTES, is_sensitive_path, strip_ansi_and_progress, trim_terminal_output},
    redact_secrets,
    store::{ContextGuardStore, enqueue_saved_stats, prepare_saved_stats_writer},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookEvaluation {
    pub response_json: String,
    pub saved_noise_bytes: u64,
    savings: Option<(&'static str, SavedEvent)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SavedEvent {
    Read(String),
    AntigravityStep(String, u64),
    UnidentifiedRead,
    Replacement(String),
}

impl HookEvaluation {
    pub fn allow() -> Self {
        Self {
            response_json: "{}".to_owned(),
            saved_noise_bytes: 0,
            savings: None,
        }
    }
}

#[derive(Default)]
pub struct HookInterceptor {
    dedup: Mutex<ReadDedupGuard>,
    compact_resets: Mutex<HashMap<String, CompactReset>>,
    saved_by_session: Mutex<HashMap<String, SavedNoise>>,
    replacement_sources: Mutex<HashMap<(String, String, String), String>>,
}

#[derive(Default)]
struct SavedNoise {
    bytes: u64,
    counted_events: HashSet<(&'static str, SavedEvent)>,
}

const COMPACT_PAIR_WINDOW: Duration = Duration::from_secs(2);

struct CompactReset {
    profile: LaunchProfile,
    host_session: Option<String>,
    id: Option<String>,
    saw_session_start: bool,
    saw_post_compact: bool,
    reset_pending: bool,
    at: Instant,
}

impl HookInterceptor {
    pub fn new() -> Self {
        prepare_saved_stats_writer();
        Self::default()
    }

    pub fn saved_noise_bytes(&self, session_id: &str) -> u64 {
        self.saved_by_session
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).map(|saved| saved.bytes))
            .unwrap_or(0)
    }

    pub fn reset_session(&self, session_id: &str) {
        if let Ok(mut dedup) = self.dedup.lock() {
            dedup.reset_session(session_id);
        }
    }

    fn reset_after_compact(
        &self,
        profile: LaunchProfile,
        session_id: &str,
        payload: &Value,
        session_start: bool,
        deadline: Instant,
    ) {
        let Ok(mut resets) = self.compact_resets.lock() else {
            return;
        };
        let now = Instant::now();
        let host_session = text_field(payload, &["session_id", "sessionId"]);
        let compaction_id = text_field(payload, &["compaction_id", "compactionId"]);
        let id = compaction_id.or_else(|| text_field(payload, &["prompt_id", "turn_id"]));
        if let Some(last) = resets.get_mut(session_id)
            && last.profile == profile
            && last.host_session.as_deref() == host_session
            && now < deadline
        {
            let within_window = now.saturating_duration_since(last.at) <= COMPACT_PAIR_WINDOW;
            let new_signal = if session_start {
                !last.saw_session_start
            } else {
                !last.saw_post_compact
            };
            let same_round = match (last.id.as_deref(), id) {
                (Some(previous), Some(current)) => {
                    // A single prompt or turn can trigger multiple compactions.
                    previous == current && (compaction_id.is_some() || new_signal || within_window)
                }
                // Codex can defer its ID-less SessionStart until the next prompt.
                _ => {
                    within_window
                        || (profile == LaunchProfile::Codex && session_start && new_signal)
                }
            };
            if same_round {
                if last.id.is_none() {
                    last.id = id.map(str::to_owned);
                }
                if new_signal {
                    last.at = now;
                }
                last.saw_session_start |= session_start;
                last.saw_post_compact |= !session_start;
                return;
            }
        }
        if Instant::now() >= deadline {
            return;
        }
        // Another card can hold the read lock during filesystem I/O. Apply
        // the reset before this card's next read if the lock is busy now.
        let reset_pending = match self.dedup.try_lock() {
            Ok(mut dedup) => {
                dedup.reset_session(session_id);
                false
            }
            Err(_) => true,
        };
        resets.insert(
            session_id.to_owned(),
            CompactReset {
                profile,
                host_session: host_session.map(str::to_owned),
                id: id.map(str::to_owned),
                saw_session_start: session_start,
                saw_post_compact: !session_start,
                reset_pending,
                at: now,
            },
        );
    }

    pub fn evaluate_hook(
        &self,
        data_root: &Path,
        profile: LaunchProfile,
        session_id: &str,
        payload: &Value,
        deadline: Instant,
    ) -> HookEvaluation {
        let now = Instant::now();
        if now >= deadline {
            return HookEvaluation::allow();
        }
        let event = extract_event_name(payload);
        if matches!(profile, LaunchProfile::ClaudeCode | LaunchProfile::Codex)
            && (event == "PostCompact"
                || (event == "SessionStart"
                    && payload.get("source").and_then(Value::as_str) == Some("compact")))
        {
            self.reset_after_compact(
                profile,
                session_id,
                payload,
                event == "SessionStart",
                deadline,
            );
            return HookEvaluation::allow();
        }
        if !matches!(event, "PreToolUse" | "PostToolUse") {
            return HookEvaluation::allow();
        }
        let Some(agent_id) = agent_id_for_profile(profile) else {
            return HookEvaluation::allow();
        };
        let config = ContextOptimizerConfig::for_data_root(data_root);
        let Ok(snapshot) = config.snapshot() else {
            return HookEvaluation::allow();
        };
        // The config treats missing and invalid files alike. Invalid files
        // should fail open, while a fresh data root uses the displayed defaults.
        if !snapshot.exists && !matches!(config.path().try_exists(), Ok(false)) {
            return HookEvaluation::allow();
        }
        let Some(entry) = snapshot.agents.get(agent_id) else {
            return HookEvaluation::allow();
        };
        if !entry.capabilities.sync_hook_intercept {
            return HookEvaluation::allow();
        }
        let settings = entry.settings.effective_for(agent_id);
        if Instant::now() >= deadline || !settings.enabled {
            return HookEvaluation::allow();
        }

        match event {
            "PreToolUse"
                if profile == LaunchProfile::Antigravity
                    && settings.noise_filter
                    && extract_tool_name(payload) == "run_command" =>
            {
                wrap_antigravity_command(data_root, session_id, payload)
                    .unwrap_or_else(HookEvaluation::allow)
            }
            "PreToolUse" => {
                self.evaluate_pre_tool_use(profile, agent_id, session_id, payload, &settings)
            }
            "PostToolUse" if profile == LaunchProfile::ClaudeCode => self
                .replace_claude_tool_output(
                    data_root, agent_id, session_id, payload, &settings, deadline,
                )
                .unwrap_or_else(HookEvaluation::allow),
            "PostToolUse" if profile != LaunchProfile::Antigravity => {
                self.evaluate_post_tool_use(data_root, session_id, payload, &settings)
            }
            _ => HookEvaluation::allow(),
        }
    }

    fn evaluate_pre_tool_use(
        &self,
        profile: LaunchProfile,
        agent_id: &'static str,
        session_id: &str,
        payload: &Value,
        settings: &matou_native_core::context_optimizer_config::AgentOptimizerSettings,
    ) -> HookEvaluation {
        let tool_name = extract_tool_name(payload);
        if !is_read_tool(tool_name) {
            return HookEvaluation::allow();
        }
        let Some(input) = extract_tool_input(payload) else {
            return HookEvaluation::allow();
        };
        let Some(raw_path) = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .or_else(|| input.get("Path"))
            .or_else(|| input.get("AbsolutePath"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty() && !s.contains('\0'))
        else {
            return HookEvaluation::allow();
        };
        let path = PathBuf::from(raw_path);

        if settings.block_sensitive_reads && is_sensitive_path(&path) {
            let reason = format!(
                "[matou context-guard] Blocked direct read of sensitive file `{}`. Disable sensitive-file protection in Matou Settings if you intentionally need to inspect it.",
                path.display()
            );
            return HookEvaluation {
                response_json: deny_pre_tool_json(profile, &reason),
                saved_noise_bytes: 0,
                savings: None,
            };
        }

        if !settings.read_dedup {
            return HookEvaluation::allow();
        }

        let (start_line, end_line) = extract_line_range(input);
        let decision = match self.dedup.try_lock() {
            Ok(mut dedup) => {
                let Ok(mut resets) = self.compact_resets.lock() else {
                    return HookEvaluation::allow();
                };
                if let Some(reset) = resets
                    .get_mut(session_id)
                    .filter(|reset| reset.reset_pending)
                {
                    dedup.reset_session(session_id);
                    reset.reset_pending = false;
                }
                drop(resets);
                dedup.evaluate_read(session_id, &path, start_line, end_line, Instant::now())
            }
            Err(_) => return HookEvaluation::allow(),
        };

        match decision {
            ReadDecision::Allow => HookEvaluation::allow(),
            ReadDecision::BlockDuplicate {
                reason,
                saved_bytes,
            }
            | ReadDecision::ReturnDiff {
                diff: reason,
                saved_bytes,
            } => {
                if profile != LaunchProfile::Antigravity {
                    self.add_saved_bytes(session_id, saved_bytes as u64);
                }
                HookEvaluation {
                    response_json: deny_pre_tool_json(profile, &reason),
                    saved_noise_bytes: saved_bytes as u64,
                    savings: read_savings_event(profile, payload).map(|event| (agent_id, event)),
                }
            }
            ReadDecision::BlockOversized { reason, .. } => HookEvaluation {
                response_json: deny_pre_tool_json(profile, &reason),
                saved_noise_bytes: 0,
                savings: None,
            },
        }
    }

    fn evaluate_post_tool_use(
        &self,
        data_root: &Path,
        session_id: &str,
        payload: &Value,
        settings: &matou_native_core::context_optimizer_config::AgentOptimizerSettings,
    ) -> HookEvaluation {
        if !settings.noise_filter {
            return HookEvaluation::allow();
        }
        let tool_name = extract_tool_name(payload);
        if !is_shell_tool(tool_name) {
            return HookEvaluation::allow();
        }
        let Some(raw_output) = extract_tool_output(payload) else {
            return HookEvaluation::allow();
        };
        if raw_output.len() <= settings.max_log_bytes.max(HARD_CAP_BYTES) {
            return HookEvaluation::allow();
        }

        let command = extract_tool_input(payload)
            .and_then(|v| {
                v.get("command")
                    .or_else(|| v.get("CommandLine"))
                    .and_then(Value::as_str)
            })
            .unwrap_or(tool_name);

        let redacted_full = redact_secrets(&raw_output);
        let source_id = ContextGuardStore::open(data_root)
            .ok()
            .and_then(|store| {
                store
                    .save_log(session_id, tool_name, command, &redacted_full)
                    .ok()
            })
            .map(|rec| rec.id);

        let trimmed = trim_terminal_output(
            &redacted_full,
            command,
            settings.max_log_bytes,
            source_id.as_deref(),
        );
        if !trimmed.trimmed {
            return HookEvaluation::allow();
        }

        self.add_saved_bytes(session_id, trimmed.saved_bytes as u64);
        let response = json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": trimmed.text,
            }
        });
        HookEvaluation {
            response_json: response.to_string(),
            saved_noise_bytes: trimmed.saved_bytes as u64,
            savings: None,
        }
    }

    fn replace_claude_tool_output(
        &self,
        data_root: &Path,
        agent_id: &'static str,
        session_id: &str,
        payload: &Value,
        settings: &matou_native_core::context_optimizer_config::AgentOptimizerSettings,
        deadline: Instant,
    ) -> Option<HookEvaluation> {
        if !settings.noise_filter || Instant::now() >= deadline {
            return None;
        }
        let host_session = payload.get("session_id")?.as_str()?.trim();
        let tool_use_id = payload.get("tool_use_id")?.as_str()?.trim();
        if host_session.is_empty() || tool_use_id.is_empty() {
            return None;
        }
        let tool = extract_tool_name(payload);
        let threshold = settings.max_log_bytes.max(HARD_CAP_BYTES);
        let output = ClaudeOutput::parse(payload, tool, threshold, deadline)?;
        if output.reported_bytes <= threshold || Instant::now() >= deadline {
            return None;
        }
        let full = redact_secrets(&output.body);
        let cleaned = redact_secrets(&strip_ansi_and_progress(&full));
        let command = redact_secrets(output.command);
        let key = (
            session_id.to_owned(),
            host_session.to_owned(),
            tool_use_id.to_owned(),
        );
        // A retry of the same host tool call reuses its source. Concurrent DB
        // access follows the same immediate fail-open policy as SQLite locks.
        let mut sources = self.replacement_sources.try_lock().ok()?;
        if Instant::now() >= deadline {
            return None;
        }
        let source = if let Some(id) = sources.get(&key) {
            id.clone()
        } else {
            let store = ContextGuardStore::open_for_hook(data_root).ok()?;
            let record = store
                .save_log_before(session_id, tool, &command, &full, Some(deadline))
                .ok()?;
            sources.insert(key, record.id.clone());
            record.id
        };
        drop(sources);
        let (replacement, saved_bytes) = output.replacement(&cleaned, &command, &source)?;
        if Instant::now() >= deadline {
            return None;
        }
        Some(HookEvaluation {
            response_json: json!({"hookSpecificOutput": {
                "hookEventName": "PostToolUse", "updatedToolOutput": replacement,
            }})
            .to_string(),
            saved_noise_bytes: saved_bytes as u64,
            savings: Some((agent_id, SavedEvent::Replacement(source))),
        })
    }

    pub(crate) fn record_delivered_savings(
        &self,
        data_root: PathBuf,
        session_id: &str,
        decision: HookEvaluation,
        now: SystemTime,
    ) {
        let Some((agent_id, event)) = decision.savings else {
            return;
        };
        let count_session_total =
            agent_id == "antigravity" || matches!(event, SavedEvent::Replacement(_));
        let first_delivery = self.saved_by_session.lock().is_ok_and(|mut map| {
            let saved = map.entry(session_id.to_owned()).or_default();
            let first_delivery = event == SavedEvent::UnidentifiedRead
                || saved.counted_events.insert((agent_id, event));
            if first_delivery && count_session_total {
                saved.bytes = saved.bytes.saturating_add(decision.saved_noise_bytes);
            }
            first_delivery
        });
        if first_delivery && decision.saved_noise_bytes > 0 {
            enqueue_saved_stats(data_root, agent_id, decision.saved_noise_bytes, now);
        }
    }

    fn add_saved_bytes(&self, session_id: &str, bytes: u64) {
        if bytes == 0 {
            return;
        }
        if let Ok(mut map) = self.saved_by_session.lock() {
            let entry = map.entry(session_id.to_owned()).or_default();
            entry.bytes = entry.bytes.saturating_add(bytes);
        }
    }
}

fn agent_id_for_profile(profile: LaunchProfile) -> Option<&'static str> {
    match profile {
        LaunchProfile::ClaudeCode => Some("claude-code"),
        LaunchProfile::Antigravity => Some("antigravity"),
        LaunchProfile::Codex => Some("codex"),
        LaunchProfile::Dsh => Some("dsh"),
        LaunchProfile::Shell => None,
    }
}

fn extract_event_name(payload: &Value) -> &str {
    payload
        .get("hook_event_name")
        .or_else(|| payload.get("hookEventName"))
        .or_else(|| payload.get("event"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn extract_tool_name(payload: &Value) -> &str {
    text_field(payload, &["tool_name", "toolName"])
        .or_else(|| text_field(payload.get("toolCall")?, &["name"]))
        .unwrap_or_default()
}

fn text_field<'a>(payload: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        payload
            .get(*key)?
            .as_str()
            .filter(|text| !text.trim().is_empty())
    })
}

fn extract_tool_input(payload: &Value) -> Option<&Map<String, Value>> {
    payload
        .get("tool_input")
        .and_then(Value::as_object)
        .or_else(|| payload.get("toolInput")?.as_object())
        .or_else(|| payload.get("toolCall")?.get("args")?.as_object())
}

fn read_savings_event(profile: LaunchProfile, payload: &Value) -> Option<SavedEvent> {
    if let Some(id) = text_field(payload, &["tool_use_id", "toolUseId"])
        .or_else(|| text_field(payload.get("toolCall")?, &["id"]))
    {
        return Some(SavedEvent::Read(id.to_owned()));
    }
    if profile == LaunchProfile::Antigravity {
        if let Some(conversation) = text_field(payload, &["conversationId", "conversation_id"])
            && let Some(step) = payload.get("stepIdx").and_then(Value::as_u64)
        {
            return Some(SavedEvent::AntigravityStep(conversation.to_owned(), step));
        }
        return Some(SavedEvent::UnidentifiedRead);
    }
    None
}

fn wrap_antigravity_command(
    data_root: &Path,
    session_id: &str,
    payload: &Value,
) -> Option<HookEvaluation> {
    let input = extract_tool_input(payload)?;
    if !matches!(input.get("RunPersistent"), None | Some(Value::Bool(false)))
        || input
            .get("RequestedTerminalID")
            .is_some_and(|id| id.as_str() != Some(""))
        || input.get("WaitMsBeforeAsync")?.as_u64()? == 0
    {
        return None;
    }

    let command = input.get("CommandLine")?.as_str()?;
    if !is_foreground_command(command) {
        return None;
    }
    let (executable, entry) = mt_context_command(data_root)?;
    let command_line = format!(
        "{executable}{entry} ctx exec --session {} --data-root {} -- {}",
        shell_arg(session_id)?,
        shell_arg(data_root.to_str()?)?,
        shell_arg(command)?,
    );
    Some(HookEvaluation {
        response_json: json!({"decision":"allow", "overwrite":{"CommandLine":command_line}})
            .to_string(),
        ..HookEvaluation::allow()
    })
}

#[cfg(windows)]
fn mt_context_command(_: &Path) -> Option<(String, &'static str)> {
    let executable = std::env::current_exe().ok()?;
    Some((shell_arg(executable.to_str()?)?, " --mt"))
}

#[cfg(not(windows))]
fn mt_context_command(data_root: &Path) -> Option<(String, &'static str)> {
    let sibling = std::env::current_exe()
        .ok()
        .map(|path| path.with_file_name("mt"));
    let valid = |path: &Path| {
        path.is_absolute()
            && path
                .to_str()
                .is_some_and(|path| !path.chars().any(char::is_control))
            && std::fs::metadata(path).is_ok_and(|metadata| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o111 == 0 {
                        return false;
                    }
                }
                metadata.is_file()
            })
    };
    if let Some(sibling) = sibling.filter(|path| valid(path)) {
        return Some((shell_arg(sibling.to_str()?)?, ""));
    }
    // Native macOS/Linux installs generate this private shim; a shell-wide PATH
    // lookup would resolve against the runtime, not the agent's terminal.
    let shim = data_root.join("control/bin/mt");
    if !valid(&shim) {
        return None;
    }
    Some((shell_arg(shim.to_str()?)?, ""))
}

#[cfg(not(windows))]
fn shell_arg(value: &str) -> Option<String> {
    (!value.is_empty() && !value.chars().any(char::is_control))
        .then(|| format!("'{}'", value.replace('\'', "'\\''")))
}

#[cfg(windows)]
fn shell_arg(value: &str) -> Option<String> {
    (!value.is_empty() && !value.chars().any(|ch| {
        ch.is_control() || matches!(ch, '"' | '%' | '!' | '^')
    }))
    .then(|| format!(r#""{value}""#))
}

fn is_foreground_command(command: &str) -> bool {
    // Only accept plain words. Shell expansion, quoting, redirects, pipelines,
    // command substitution and compound commands all retain their original path.
    if !command
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b" \t-._/:=,+@".contains(&byte))
    {
        return false;
    }
    let mut words = command.split_ascii_whitespace();
    let (flags, valued_flags): (&[&str], &[&str]) = match (words.next(), words.next()) {
        (Some("cargo"), Some("test" | "check" | "build" | "clippy")) => (
            &[
                "-q",
                "-v",
                "--quiet",
                "--verbose",
                "--locked",
                "--offline",
                "--release",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--no-default-features",
                "--no-run",
                "--doc",
                "--lib",
                "--tests",
                "--nocapture",
                "--exact",
                "--test",
                "--bin",
                "--package",
                "-p",
                "--features",
                "--manifest-path",
                "--test-threads",
            ],
            &[
                "--features",
                "--manifest-path",
                "--test-threads",
                "--target",
            ],
        ),
        (Some("pnpm" | "npm" | "yarn" | "bun"), Some(task)) => {
            let task = if task == "run" {
                words.next().unwrap_or_default()
            } else {
                task
            };
            if !matches!(task, "test" | "build" | "lint" | "typecheck") {
                return false;
            }
            (&["--ci", "--run", "--runInBand", "--passWithNoTests"], &[])
        }
        (Some("git"), Some("diff" | "log" | "show" | "status")) => (
            &[
                "-p",
                "-s",
                "-v",
                "--short",
                "--porcelain",
                "--stat",
                "--name-only",
                "--name-status",
                "--oneline",
                "--cached",
                "--staged",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--max-count",
                "-n",
            ],
            &["--max-count"],
        ),
        (Some("go"), Some("test")) => (
            &[
                "-v",
                "-race",
                "-short",
                "-failfast",
                "-cover",
                "-count",
                "-run",
                "-timeout",
            ],
            &["-count", "-run", "-timeout"],
        ),
        (Some("pytest"), first) => {
            return first.into_iter().chain(words).all(|word| {
                is_foreground_arg(
                    word,
                    &[
                        "-q",
                        "-v",
                        "-vv",
                        "-x",
                        "-s",
                        "-k",
                        "-m",
                        "--collect-only",
                        "--maxfail",
                        "--disable-warnings",
                    ],
                    &["--maxfail"],
                )
            });
        }
        _ => return false,
    };
    words.all(|word| is_foreground_arg(word, flags, valued_flags))
}

fn is_foreground_arg(word: &str, flags: &[&str], valued_flags: &[&str]) -> bool {
    if word.starts_with('-') {
        return word == "--"
            || flags.contains(&word)
            || word
                .split_once('=')
                .is_some_and(|(flag, value)| !value.is_empty() && valued_flags.contains(&flag));
    }
    let word = word.to_ascii_lowercase();
    !matches!(
        word.as_str(),
        "watch" | "interactive" | "repl" | "dev" | "serve" | "start"
    ) && !word.starts_with("watch:")
}

fn is_read_tool(name: &str) -> bool {
    matches!(name, "Read" | "read_file" | "view_file")
}

fn is_shell_tool(name: &str) -> bool {
    matches!(name, "Bash" | "run_command" | "shell" | "exec_command")
}

fn extract_line_range(input: &Map<String, Value>) -> (Option<usize>, Option<usize>) {
    let start = input
        .get("StartLine")
        .or_else(|| input.get("start_line"))
        .or_else(|| input.get("offset"))
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    let end = input
        .get("EndLine")
        .or_else(|| input.get("end_line"))
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .or_else(|| {
            let limit = input.get("limit").and_then(Value::as_u64)? as usize;
            Some(start.unwrap_or(1).saturating_add(limit))
        });
    (start, end)
}

fn extract_tool_output(payload: &Value) -> Option<String> {
    let resp = payload
        .get("tool_response")
        .or_else(|| payload.get("toolResponse"))?;
    if let Some(s) = resp.as_str() {
        return Some(s.to_owned());
    }
    if let Some(obj) = resp.as_object() {
        if let Some(s) = obj
            .get("stdout")
            .or_else(|| obj.get("output"))
            .or_else(|| obj.get("content"))
            .and_then(Value::as_str)
        {
            return Some(s.to_owned());
        }
    }
    None
}

fn deny_pre_tool_json(profile: LaunchProfile, reason: &str) -> String {
    if profile == LaunchProfile::Antigravity {
        return json!({"decision": "deny", "reason": reason}).to_string();
    }
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use matou_native_core::context_optimizer_config::AgentOptimizerSettings;
    use std::fs;
    use std::time::Duration;

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn independent_sensitive_reads_block_env_but_allow_templates() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join(".env");
        fs::write(&env_file, "SECRET=123").unwrap();

        let interceptor = HookInterceptor::new();
        ContextOptimizerConfig::for_data_root(dir.path())
            .set_agent_settings(
                "claude-code",
                AgentOptimizerSettings {
                    noise_filter: false,
                    read_dedup: false,
                    block_sensitive_reads: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": env_file }
        });
        let eval = interceptor.evaluate_hook(
            dir.path(),
            LaunchProfile::ClaudeCode,
            "s1",
            &payload,
            deadline(),
        );
        assert!(
            eval.response_json
                .contains("\"permissionDecision\":\"deny\"")
        );
        payload["tool_input"]["file_path"] = json!(dir.path().join(".env.example"));
        let eval = interceptor.evaluate_hook(
            dir.path(),
            LaunchProfile::ClaudeCode,
            "s1",
            &payload,
            deadline(),
        );
        assert_eq!(eval.response_json, "{}");
    }

    #[test]
    fn sensitive_read_defaults_match_settings_on_fresh_data_and_fail_open_on_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = ContextOptimizerConfig::for_data_root(dir.path());
        let interceptor = HookInterceptor::new();
        let read = |profile, name| {
            interceptor.evaluate_hook(
                dir.path(),
                profile,
                "card",
                &json!({
                    "hook_event_name": "PreToolUse", "tool_name": "Read",
                    "tool_input": {"file_path": dir.path().join(name)}
                }),
                deadline(),
            )
        };
        assert!(!config.path().exists());
        for (profile, blocks_sensitive_reads) in [
            (LaunchProfile::ClaudeCode, true),
            (LaunchProfile::Codex, false),
            (LaunchProfile::Antigravity, true),
        ] {
            let read_reply = read(profile, ".env").response_json;
            assert_eq!(
                read_reply.contains("\"permissionDecision\":\"deny\"")
                    || read_reply.contains("\"decision\":\"deny\""),
                blocks_sensitive_reads,
            );
            assert_eq!(read(profile, ".env.example"), HookEvaluation::allow());
        }
        assert!(!config.path().exists());
        config
            .set_agent_settings(
                "claude-code",
                AgentOptimizerSettings {
                    block_sensitive_reads: false,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            read(LaunchProfile::ClaudeCode, ".env"),
            HookEvaluation::allow()
        );
        fs::write(config.path(), "{").unwrap();
        assert_eq!(
            read(LaunchProfile::ClaudeCode, ".env"),
            HookEvaluation::allow()
        );
    }

    #[test]
    fn antigravity_nested_reads_apply_native_denial_and_allow_a_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        fs::write(&path, "a sufficiently long source line\n".repeat(400)).unwrap();
        ContextOptimizerConfig::for_data_root(dir.path())
            .set_agent_settings(
                "antigravity",
                AgentOptimizerSettings {
                    read_dedup: true,
                    block_sensitive_reads: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let interceptor = HookInterceptor::new();
        let read = |path: &Path, start: usize, end: usize| {
            interceptor.evaluate_hook(
            dir.path(), LaunchProfile::Antigravity, "card", &json!({
                "hookEventName": "PreToolUse",
                "toolCall": {"name":"view_file", "args": {"AbsolutePath":path, "StartLine":start, "EndLine":end}}
            }), deadline(),
        )
        };
        assert_eq!(read(&path, 1, 200), HookEvaluation::allow());
        assert_eq!(read(&path, 201, 400), HookEvaluation::allow());
        let blocked = read(&path, 1, 200);
        let response: Value = serde_json::from_str(&blocked.response_json).unwrap();
        assert_eq!(response["decision"], "deny");
        assert!(response["reason"].as_str().unwrap().contains("120s"));
        assert!(blocked.saved_noise_bytes > 0);
        assert_eq!(read(&path, 1, 200), HookEvaluation::allow());
        let secret = read(&dir.path().join(".env"), 1, 10);
        assert_eq!(
            serde_json::from_str::<Value>(&secret.response_json).unwrap()["decision"],
            "deny"
        );
        assert_eq!(
            read(&dir.path().join(".env.example"), 1, 10),
            HookEvaluation::allow()
        );
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_rewrites_only_known_foreground_commands_as_one_shell_argument() {
        use std::{os::unix::fs::PermissionsExt, process::Command};

        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data's root");
        let shim = data_root.join("control/bin/mt");
        fs::create_dir_all(shim.parent().unwrap()).unwrap();
        fs::write(&shim, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        let config = ContextOptimizerConfig::for_data_root(&data_root);
        config
            .set_agent_settings(
                "antigravity",
                AgentOptimizerSettings {
                    noise_filter: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let interceptor = HookInterceptor::new();
        let payload = |args: Value| json!({"hookEventName":"PreToolUse", "toolCall":{"name":"run_command", "args":args}});
        let evaluate = |profile, args| {
            interceptor.evaluate_hook(&data_root, profile, "card's id", &payload(args), deadline())
        };
        let args = |command| json!({"CommandLine":command, "WaitMsBeforeAsync":5000, "Cwd":"/workspace/project"});
        assert_eq!(
            evaluate(LaunchProfile::Antigravity, args("cargo test")),
            HookEvaluation::allow()
        );
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
        for command in [
            "cargo test -- --test-threads=1",
            "cargo check --locked",
            "cargo build",
            "cargo clippy",
            "pnpm run typecheck",
            "npm test -- --runInBand",
            "yarn lint",
            "bun run build",
            "pytest -q",
            "go test ./...",
            "git diff --stat",
            "git log",
            "git show",
            "git status --short",
        ] {
            let result = evaluate(LaunchProfile::Antigravity, args(command));
            let reply: Value = serde_json::from_str(&result.response_json).unwrap();
            assert_eq!(reply["decision"], "allow", "{command}");
            assert!(reply["overwrite"].get("Cwd").is_none());
            let wrapped = reply["overwrite"]["CommandLine"].as_str().unwrap();
            let output = Command::new("/bin/sh")
                .args(["-c", wrapped])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .lines()
                    .collect::<Vec<_>>(),
                [
                    "ctx",
                    "exec",
                    "--session",
                    "card's id",
                    "--data-root",
                    data_root.to_str().unwrap(),
                    "--",
                    command
                ]
            );
        }
        for command in [
            "cargo test &",
            "cargo test && git status",
            "cargo test | cat",
            "cargo test > result",
            "cargo test\ngit status",
            "npm test -i",
            "npm run test -- --interactive",
            "pnpm test --watch",
            "pnpm run watch",
            "git diff --ext-diff",
            "cargo test $(ls)",
            "cargo test *",
            "mt ctx exec -- cargo test",
            "vim",
            "sudo cargo test",
            "git push",
        ] {
            assert_eq!(
                evaluate(LaunchProfile::Antigravity, args(command)),
                HookEvaluation::allow(),
                "{command}"
            );
        }
        for (field, value) in [
            ("RunPersistent", json!(true)),
            ("RequestedTerminalID", json!("terminal-1")),
            ("WaitMsBeforeAsync", json!(0)),
            ("WaitMsBeforeAsync", json!("5000")),
        ] {
            let mut input = args("cargo test");
            input[field] = value;
            assert_eq!(
                evaluate(LaunchProfile::Antigravity, input),
                HookEvaluation::allow(),
                "{field}"
            );
        }
        assert_eq!(
            evaluate(LaunchProfile::Codex, args("cargo test")),
            HookEvaluation::allow()
        );
        config
            .set_agent_settings("antigravity", AgentOptimizerSettings::default())
            .unwrap();
        assert_eq!(
            evaluate(LaunchProfile::Antigravity, args("cargo test")),
            HookEvaluation::allow()
        );
    }

    #[test]
    fn only_completed_compaction_clears_read_dedup_once_per_round() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        fs::write(&path, "same source content\n".repeat(400)).unwrap();
        let config = ContextOptimizerConfig::for_data_root(dir.path());
        let interceptor = HookInterceptor::new();
        for (profile, id_field) in [
            (LaunchProfile::ClaudeCode, "prompt_id"),
            (LaunchProfile::Codex, "turn_id"),
        ] {
            let card = profile.kind();
            config
                .set_agent_settings(
                    card,
                    AgentOptimizerSettings {
                        read_dedup: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            let evaluate = |payload| {
                interceptor.evaluate_hook(dir.path(), profile, card, &payload, deadline())
            };
            let read = || {
                evaluate(
                    json!({"hook_event_name":"PreToolUse", "tool_name":"Read", "tool_input":{"file_path":path}}),
                )
            };
            let mut start = json!({"hook_event_name":"SessionStart", "session_id":"provider", "source":"compact"});
            let mut post = json!({"hook_event_name":"PostCompact", "session_id":"provider"});
            post[id_field] = json!("first-compaction");

            assert_eq!(read(), HookEvaluation::allow());
            for event in [
                json!({"hook_event_name":"SessionStart", "source":"startup"}),
                json!({"hook_event_name":"SessionStart", "source":"resume"}),
                json!({"hook_event_name":"PreCompact"}),
            ] {
                assert_eq!(evaluate(event), HookEvaluation::allow());
                assert!(
                    read()
                        .response_json
                        .contains("\"permissionDecision\":\"deny\"")
                );
                assert_eq!(read(), HookEvaluation::allow()); // 120s retry passthrough
            }
            let (first, follow_up) = if profile == LaunchProfile::ClaudeCode {
                start[id_field] = json!("first-compaction");
                (&start, &post)
            } else {
                (&post, &start)
            };
            evaluate(first.clone());
            assert_eq!(read(), HookEvaluation::allow());
            // Codex's next prompt may arrive much later than a time-only window.
            interceptor
                .compact_resets
                .lock()
                .unwrap()
                .get_mut(card)
                .unwrap()
                .at -= Duration::from_secs(60);
            evaluate(follow_up.clone());
            assert!(
                read()
                    .response_json
                    .contains("\"permissionDecision\":\"deny\"")
            );
            assert_eq!(read(), HookEvaluation::allow());
            evaluate(post.clone());
            assert!(
                read()
                    .response_json
                    .contains("\"permissionDecision\":\"deny\"")
            );
            post[id_field] = json!("next-compaction");
            assert_eq!(read(), HookEvaluation::allow()); // consume retry passthrough
            let active_read = interceptor.dedup.lock().unwrap();
            evaluate(post);
            drop(active_read);
            assert_eq!(read(), HookEvaluation::allow());
            assert!(
                read()
                    .response_json
                    .contains("\"permissionDecision\":\"deny\"")
            );
            assert_eq!(read(), HookEvaluation::allow());
            interceptor.reset_session(card);
            assert_eq!(read(), HookEvaluation::allow());
        }
    }

    #[test]
    fn enabled_noise_filter_blocks_env_and_trims_large_shell_output() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ContextOptimizerConfig::for_data_root(dir.path());
        cfg.set_agent_settings(
            "claude-code",
            AgentOptimizerSettings {
                enabled: true,
                noise_filter: true,
                read_dedup: true,
                block_sensitive_reads: true,
                ..Default::default()
            },
        )
        .unwrap();

        let env_file = dir.path().join(".env");
        fs::write(&env_file, "API_KEY=secret").unwrap();
        let interceptor = HookInterceptor::new();

        let pre_env = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": env_file }
        });
        let env_eval = interceptor.evaluate_hook(
            dir.path(),
            LaunchProfile::ClaudeCode,
            "s1",
            &pre_env,
            deadline(),
        );
        assert!(
            env_eval
                .response_json
                .contains("\"permissionDecision\":\"deny\"")
        );

        // PostToolUse with huge test log stores full log in SQLite and trims response.
        let mut big_log = "test ok\n".repeat(3_000);
        big_log.push_str("error[E0308]: mismatched types\n");
        let post_bash = json!({
            "hook_event_name": "PostToolUse",
            "session_id": "provider-1",
            "tool_use_id": "toolu_1",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "tool_response": { "stdout": big_log, "stderr": "", "interrupted": false, "isImage": false }
        });
        let post_eval = interceptor.evaluate_hook(
            dir.path(),
            LaunchProfile::ClaudeCode,
            "s1",
            &post_bash,
            deadline(),
        );
        assert!(post_eval.saved_noise_bytes > 10_000);
        assert!(post_eval.response_json.contains("mismatched types"));
        assert!(post_eval.response_json.contains("log_0001"));
        let reply: Value = serde_json::from_str(&post_eval.response_json).unwrap();
        assert!(
            reply["hookSpecificOutput"]
                .get("additionalContext")
                .is_none()
        );
        let summary = reply["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap();
        assert!(summary.len() <= HARD_CAP_BYTES);
        assert_eq!(
            post_eval.saved_noise_bytes as usize,
            big_log.trim_end().len() - summary.len()
        );
        assert_eq!(interceptor.saved_noise_bytes("s1"), 0);
        interceptor.record_delivered_savings(
            dir.path().to_owned(),
            "s1",
            post_eval.clone(),
            SystemTime::now(),
        );
        let retry = interceptor.evaluate_hook(
            dir.path(),
            LaunchProfile::ClaudeCode,
            "s1",
            &post_bash,
            deadline(),
        );
        assert_eq!(retry, post_eval);
        interceptor.record_delivered_savings(dir.path().to_owned(), "s1", retry, SystemTime::now());
        assert_eq!(
            interceptor.saved_noise_bytes("s1"),
            post_eval.saved_noise_bytes
        );
        crate::context_guard::store::flush_saved_stats();
        let store = ContextGuardStore::open(dir.path()).unwrap();
        assert_eq!(store.list_session_logs("s1", 10).unwrap().len(), 1);
        assert_eq!(
            store.get_full_log("log_0001").unwrap().as_deref(),
            Some(big_log.as_str())
        );
    }

    #[test]
    fn dsh_never_intercepts_even_if_configured() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ContextOptimizerConfig::for_data_root(dir.path());
        cfg.set_agent_settings(
            "dsh",
            AgentOptimizerSettings {
                enabled: true,
                noise_filter: true,
                read_dedup: true,
                ..Default::default()
            },
        )
        .unwrap();
        let env_file = dir.path().join(".env");
        fs::write(&env_file, "SECRET=1").unwrap();
        let interceptor = HookInterceptor::new();
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": env_file }
        });
        let eval =
            interceptor.evaluate_hook(dir.path(), LaunchProfile::Dsh, "s1", &payload, deadline());
        assert_eq!(eval.response_json, "{}");
    }

    #[test]
    fn claude_post_tool_use_fails_open_for_unknown_outputs_disabled_switch_lock_and_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let config = ContextOptimizerConfig::for_data_root(dir.path());
        let interceptor = HookInterceptor::new();
        let mut payload = json!({
            "hook_event_name": "PostToolUse", "session_id": "provider", "tool_use_id": "toolu_1",
            "tool_name": "Bash", "tool_input": {"command": "test"},
            "tool_response": {"stdout": "ERROR\n".repeat(4_000), "stderr": "", "interrupted": false, "isImage": false}
        });
        let evaluate = |payload: &Value, deadline| {
            interceptor.evaluate_hook(
                dir.path(),
                LaunchProfile::ClaudeCode,
                "card",
                payload,
                deadline,
            )
        };
        config
            .set_agent_settings(
                "claude-code",
                AgentOptimizerSettings {
                    noise_filter: false,
                    ..AgentOptimizerSettings::recommended_for("claude-code")
                },
            )
            .unwrap();
        assert_eq!(evaluate(&payload, deadline()), HookEvaluation::allow());
        config
            .set_agent_settings(
                "claude-code",
                AgentOptimizerSettings {
                    noise_filter: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(evaluate(&payload, Instant::now()), HookEvaluation::allow());
        payload["tool_name"] = json!("Other");
        assert_eq!(evaluate(&payload, deadline()), HookEvaluation::allow());
        payload["tool_name"] = json!("Bash");
        for bad in [json!(true), json!("false"), Value::Null] {
            payload["tool_response"]["isImage"] = bad;
            assert_eq!(evaluate(&payload, deadline()), HookEvaluation::allow());
        }
        payload["tool_response"]["isImage"] = json!(false);
        payload["tool_response"]["backgroundTaskId"] = json!("task-1");
        assert_eq!(evaluate(&payload, deadline()), HookEvaluation::allow());
        payload["tool_response"]
            .as_object_mut()
            .unwrap()
            .remove("backgroundTaskId");
        assert!(!dir.path().join("context_guard.sqlite").exists());
        let store = ContextGuardStore::open(dir.path()).unwrap();
        let db = rusqlite::Connection::open(store.path()).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(evaluate(&payload, deadline()), HookEvaluation::allow());
        assert_eq!(interceptor.saved_noise_bytes("card"), 0);
        db.execute_batch("ROLLBACK").unwrap();
        assert!(store.list_session_logs("card", 10).unwrap().is_empty());
    }

    #[test]
    fn claude_text_read_replacement_keeps_errors_redacts_full_log_and_counts_rendered_bytes() {
        let dir = tempfile::tempdir().unwrap();
        ContextOptimizerConfig::for_data_root(dir.path())
            .set_agent_settings(
                "claude-code",
                AgentOptimizerSettings {
                    noise_filter: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let secret = "sk-testsecret0123456789012345";
        let content = format!(
            "{}ERROR first {secret}\n{}FAIL second\n{}panic third\n{}assert fourth\n",
            "ok\n".repeat(1700),
            "ok\n".repeat(1700),
            "ok\n".repeat(1700),
            "ok\n".repeat(1700)
        );
        let payload = json!({
            "hook_event_name": "PostToolUse", "session_id": "provider", "tool_use_id": "toolu_1",
            "tool_name": "Read", "tool_input": {"file_path": "/tmp/log.txt"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/log.txt", "content": content, "startLine": 17,
                "numLines": content.split('\n').count(), "totalLines": 10_000
            }}
        });
        let interceptor = HookInterceptor::new();
        let eval = interceptor.evaluate_hook(
            dir.path(),
            LaunchProfile::ClaudeCode,
            "card",
            &payload,
            deadline(),
        );
        let reply: Value = serde_json::from_str(&eval.response_json).unwrap();
        let replacement = &reply["hookSpecificOutput"]["updatedToolOutput"];
        assert_eq!(replacement["type"], "text");
        assert_eq!(replacement["file"]["startLine"], 17);
        let summary = replacement["file"]["content"].as_str().unwrap();
        for error in [
            "ERROR first [REDACTED]",
            "FAIL second",
            "panic third",
            "assert fourth",
        ] {
            assert!(summary.contains(error), "missing {error}");
        }
        assert!(!summary.contains(secret));
        let rendered_size = |text: &str| {
            text.split('\n')
                .enumerate()
                .map(|(index, line)| format!("{}\t{line}", index + 17))
                .collect::<Vec<_>>()
                .join("\n")
                .trim_end()
                .len()
        };
        assert!(rendered_size(summary) < HARD_CAP_BYTES);
        assert_eq!(
            eval.saved_noise_bytes as usize,
            rendered_size(&content) - rendered_size(summary)
        );
        let store = ContextGuardStore::open(dir.path()).unwrap();
        let full = store.get_full_log("log_0001").unwrap().unwrap();
        assert_eq!(full, content.replace(secret, "[REDACTED]"));
    }
}
