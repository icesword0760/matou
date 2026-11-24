//! Synchronous PreToolUse / PostToolUse hook decision engine for
//! `ProviderHookServer`.
//!
//! Enforces per-agent settings from `ContextOptimizerConfig` with strict
//! fail-open semantics: whenever switches are off, the agent lacks sync-hook
//! capability, or anything errors, it returns `"{}"` (allow) with zero side
//! effects.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};

use matou_native_core::{
    context_optimizer_config::ContextOptimizerConfig,
    pty::LaunchProfile,
};
use serde_json::{Value, json};

use crate::context_guard::{
    dedup::{ReadDecision, ReadDedupGuard},
    filter::{HARD_CAP_BYTES, is_sensitive_path, trim_terminal_output},
    redact_secrets,
    store::ContextGuardStore,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookEvaluation {
    pub response_json: String,
    pub saved_noise_bytes: u64,
}

impl HookEvaluation {
    pub fn allow() -> Self {
        Self {
            response_json: "{}".to_owned(),
            saved_noise_bytes: 0,
        }
    }
}

#[derive(Default)]
pub struct HookInterceptor {
    dedup: Mutex<ReadDedupGuard>,
    saved_by_session: Mutex<HashMap<String, u64>>,
}

impl HookInterceptor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn saved_noise_bytes(&self, session_id: &str) -> u64 {
        self.saved_by_session
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).copied())
            .unwrap_or(0)
    }

    pub fn evaluate_hook(
        &self,
        data_root: &Path,
        profile: LaunchProfile,
        session_id: &str,
        payload: &Value,
    ) -> HookEvaluation {
        let Some(agent_id) = agent_id_for_profile(profile) else {
            return HookEvaluation::allow();
        };
        let Ok(settings) = ContextOptimizerConfig::for_data_root(data_root).effective_for(agent_id)
        else {
            return HookEvaluation::allow();
        };
        if !settings.enabled || (!settings.noise_filter && !settings.read_dedup) {
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
            "PreToolUse" => self.evaluate_pre_tool_use(session_id, payload, &settings),
            "PostToolUse" => {
                self.evaluate_post_tool_use(data_root, session_id, payload, &settings)
            }
            _ => HookEvaluation::allow(),
        }
    }

    fn evaluate_pre_tool_use(
        &self,
        session_id: &str,
        payload: &Value,
        settings: &matou_native_core::context_optimizer_config::AgentOptimizerSettings,
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
            .filter(|s| !s.trim().is_empty())
        else {
            return HookEvaluation::allow();
        };
        let path = PathBuf::from(raw_path);

        if settings.noise_filter && settings.block_sensitive_reads && is_sensitive_path(&path) {
            let reason = format!(
                "[matou context-guard] Blocked direct read of sensitive file `{}`. Disable sensitive-file protection in Matou Settings if you intentionally need to inspect it.",
                path.display()
            );
            return HookEvaluation {
                response_json: deny_pre_tool_json(&reason),
                saved_noise_bytes: 0,
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
            } => {
                self.add_saved_bytes(session_id, saved_bytes as u64);
                HookEvaluation {
                    response_json: deny_pre_tool_json(&reason),
                    saved_noise_bytes: saved_bytes as u64,
                }
            }
            ReadDecision::ReturnDiff { diff, saved_bytes } => {
                self.add_saved_bytes(session_id, saved_bytes as u64);
                HookEvaluation {
                    response_json: deny_pre_tool_json(&diff),
                    saved_noise_bytes: saved_bytes as u64,
                }
            }
            ReadDecision::BlockOversized { reason, .. } => HookEvaluation {
                response_json: deny_pre_tool_json(&reason),
                saved_noise_bytes: 0,
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
        }
    }

    fn add_saved_bytes(&self, session_id: &str, bytes: u64) {
        if bytes == 0 {
            return;
        }
        if let Ok(mut map) = self.saved_by_session.lock() {
            let entry = map.entry(session_id.to_owned()).or_insert(0);
            *entry = entry.saturating_add(bytes);
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

fn extract_line_range(
    input: &serde_json::Map<String, Value>,
) -> (Option<usize>, Option<usize>) {
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

    #[test]
    fn default_off_interception_always_allows() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join(".env");
        fs::write(&env_file, "SECRET=123").unwrap();

        let interceptor = HookInterceptor::new();
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": env_file }
        });
        let eval = interceptor.evaluate_hook(
            dir.path(),
            LaunchProfile::ClaudeCode,
            "s1",
            &payload,
        );
        assert_eq!(eval.response_json, "{}");
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
        );
        assert!(env_eval.response_json.contains("\"permissionDecision\":\"deny\""));

        // PostToolUse with huge test log stores full log in SQLite and trims response.
        let mut big_log = "test ok\n".repeat(3_000);
        big_log.push_str("error[E0308]: mismatched types\n");
        let post_bash = json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "tool_response": { "stdout": big_log }
        });
        let post_eval = interceptor.evaluate_hook(
            dir.path(),
            LaunchProfile::ClaudeCode,
            "s1",
            &post_bash,
        );
        assert!(post_eval.saved_noise_bytes > 10_000);
        assert!(post_eval.response_json.contains("mismatched types"));
        assert!(post_eval.response_json.contains("log_0001"));
        assert!(interceptor.saved_noise_bytes("s1") > 10_000);
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
        let eval = interceptor.evaluate_hook(dir.path(), LaunchProfile::Dsh, "s1", &payload);
        assert_eq!(eval.response_json, "{}");
    }
}
