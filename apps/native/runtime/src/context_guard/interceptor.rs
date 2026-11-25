//! Synchronous PreToolUse / PostToolUse hook decision engine for
//! `ProviderHookServer`.
//!
//! Enforces per-agent settings from `ContextOptimizerConfig` with strict
//! fail-open semantics: whenever switches are off, the agent lacks sync-hook
//! capability, or anything errors, it returns `"{}"` (allow) with zero side
//! effects.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Instant, SystemTime},
};

use matou_native_core::{context_optimizer_config::ContextOptimizerConfig, pty::LaunchProfile};
use serde_json::{Value, json};

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
    saved_by_session: Mutex<HashMap<String, SavedNoise>>,
    replacement_sources: Mutex<HashMap<(String, String, String), String>>,
}

#[derive(Default)]
struct SavedNoise {
    bytes: u64,
    counted_events: HashSet<(&'static str, SavedEvent)>,
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

    pub fn evaluate_hook(
        &self,
        data_root: &Path,
        profile: LaunchProfile,
        session_id: &str,
        payload: &Value,
        deadline: Instant,
    ) -> HookEvaluation {
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
        let sensitive_reads = settings.block_sensitive_reads;
        if !settings.enabled || !(settings.noise_filter || settings.read_dedup || sensitive_reads) {
            return HookEvaluation::allow();
        }

        let event = extract_event_name(payload);
        match event {
            "PreCompact" | "SessionStart" => {
                if let Ok(mut dedup) = self.dedup.lock() {
                    dedup.reset_session(session_id);
                }
                HookEvaluation::allow()
            }
            "PreToolUse" => self.evaluate_pre_tool_use(
                agent_id,
                session_id,
                payload,
                &settings,
                sensitive_reads,
            ),
            "PostToolUse" if profile == LaunchProfile::ClaudeCode => self
                .replace_claude_tool_output(
                    data_root, agent_id, session_id, payload, &settings, deadline,
                )
                .unwrap_or_else(HookEvaluation::allow),
            "PostToolUse" => self.evaluate_post_tool_use(data_root, session_id, payload, &settings),
            _ => HookEvaluation::allow(),
        }
    }

    fn evaluate_pre_tool_use(
        &self,
        agent_id: &'static str,
        session_id: &str,
        payload: &Value,
        settings: &matou_native_core::context_optimizer_config::AgentOptimizerSettings,
        sensitive_reads: bool,
    ) -> HookEvaluation {
        let tool_name = extract_tool_name(payload);
        if !is_read_tool(tool_name) {
            return HookEvaluation::allow();
        }
        let Some(input) = payload
            .get("tool_input")
            .or_else(|| payload.get("toolInput"))
            .and_then(Value::as_object)
        else {
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

        if sensitive_reads && is_sensitive_path(&path) {
            let reason = format!(
                "[matou context-guard] Blocked direct read of sensitive file `{}`. Disable sensitive-file protection in Matou Settings if you intentionally need to inspect it.",
                path.display()
            );
            return HookEvaluation {
                response_json: deny_pre_tool_json(&reason),
                saved_noise_bytes: 0,
                savings: None,
            };
        }

        if !settings.read_dedup {
            return HookEvaluation::allow();
        }

        let (start_line, end_line) = extract_line_range(input);
        let decision = match self.dedup.lock() {
            Ok(mut dedup) => {
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
                self.add_saved_bytes(session_id, saved_bytes as u64);
                HookEvaluation {
                    response_json: deny_pre_tool_json(&reason),
                    saved_noise_bytes: saved_bytes as u64,
                    savings: payload
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.trim().is_empty())
                        .map(|id| (agent_id, SavedEvent::Read(id.to_owned()))),
                }
            }
            ReadDecision::BlockOversized { reason, .. } => HookEvaluation {
                response_json: deny_pre_tool_json(&reason),
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

        let command = payload
            .get("tool_input")
            .or_else(|| payload.get("toolInput"))
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
        let is_replacement = matches!(event, SavedEvent::Replacement(_));
        let first_delivery = self.saved_by_session.lock().is_ok_and(|mut map| {
            let saved = map.entry(session_id.to_owned()).or_default();
            let first_delivery = saved.counted_events.insert((agent_id, event));
            if first_delivery && is_replacement {
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
    payload
        .get("tool_name")
        .or_else(|| payload.get("toolName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn is_read_tool(name: &str) -> bool {
    matches!(name, "Read" | "read_file" | "view_file")
}

fn is_shell_tool(name: &str) -> bool {
    matches!(name, "Bash" | "run_command" | "shell" | "exec_command")
}

fn extract_line_range(input: &serde_json::Map<String, Value>) -> (Option<usize>, Option<usize>) {
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

fn deny_pre_tool_json(reason: &str) -> String {
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
            (LaunchProfile::Antigravity, false),
        ] {
            assert_eq!(
                read(profile, ".env")
                    .response_json
                    .contains("\"permissionDecision\":\"deny\""),
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
