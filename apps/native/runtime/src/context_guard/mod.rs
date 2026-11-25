//! Built-in context & KV-cache optimizer engine (Phase 1: observation,
//! KV-cache countdown, sleep-safe stall watchdog, and redacted handoff briefs).
//!
//! All state stays in-process inside `matou-native-runtime` with zero MCP
//! servers and zero extra background processes. Behavior is gated per-agent by
//! `matou_native_core::context_optimizer_config::AgentOptimizerSettings`.

use std::collections::VecDeque;

pub mod cli;
mod claude_output;
pub mod dedup;
pub mod filter;
pub mod interceptor;
pub mod outline;
pub mod store;

use matou_native_core::{
    context_optimizer_config::{AgentOptimizerSettings, ContextOptimizerConfig},
    sessions::InterfaceLocale,
};
use regex::Regex;
use std::path::Path;
use serde::Serialize;
use std::sync::LazyLock;

use crate::agent_hud_projection::{ContextTokens, HudProvider, TaskStatus};

const DEFAULT_CLAUDE_CACHE_TTL_SEC: u64 = 300;
const DEFAULT_LONG_CACHE_TTL_SEC: u64 = 900;
const MAX_CALIBRATED_TTL_SEC: u64 = 3_600;
const CACHE_EXPIRING_SOON_SEC: u64 = 60;
const KEEPALIVE_TRIGGER_WINDOW_SEC: u64 = 45;

const MT_CTX_MANAGED_INSTRUCTIONS: &str = "\
## Matou Context Helpers (`mt ctx`)
- Before reading an unfamiliar large file, run `mt ctx outline <file>` and read only targeted line ranges.
- Combine multiple symbol lookups in one call with `mt ctx batch-search <q1> <q2> ...`.
- When a log is trimmed and saved as `log_XXXX`, search it with `mt ctx search <query> --source log_XXXX`.
- After `/compact`, run `mt ctx resume` to recover saved log sources and progress clues.";

/// Returns the short opt-in `mt ctx` routing note only when `cli_tools` is
/// enabled for `agent_id`. Disabled agents receive `None` (zero extra tokens).
pub fn managed_cli_instructions(data_root: &Path, agent_id: &str) -> Option<&'static str> {
    let settings = ContextOptimizerConfig::for_data_root(data_root)
        .effective_for(agent_id)
        .ok()?;
    (settings.enabled && settings.cli_tools).then_some(MT_CTX_MANAGED_INSTRUCTIONS)
}

/// Optional KV-cache keepalive controller.
///
/// Strictly opt-in (`cache_keepalive == false` by default). Only triggers for
/// active primary sessions within the pre-expiry window and caps pings per
/// user turn until the user sends a new message.
#[derive(Clone, Debug, Default)]
pub struct KeepaliveController {
    pings_this_turn: u8,
    in_flight: bool,
}

impl KeepaliveController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resets the per-turn ping counter whenever the user sends a new message.
    pub fn on_user_message(&mut self) {
        self.pings_this_turn = 0;
        self.in_flight = false;
    }

    pub fn on_ping_completed(&mut self) {
        self.in_flight = false;
    }

    /// Returns `true` and reserves a ping slot if a keepalive ping should be
    /// dispatched right now.
    pub fn should_dispatch(
        &mut self,
        agent_id: &str,
        settings: &AgentOptimizerSettings,
        cache: Option<&CacheHudStatus>,
        is_subagent: bool,
        session_ended: bool,
    ) -> bool {
        let effective = settings.effective_for(agent_id);
        if !effective.enabled || !effective.cache_keepalive || is_subagent || session_ended {
            return false;
        }
        if self.in_flight || self.pings_this_turn >= effective.cache_keepalive_max_per_turn {
            return false;
        }
        let Some(cache) = cache else {
            return false;
        };
        if !cache.warm || cache.remaining_sec == 0 || cache.remaining_sec > KEEPALIVE_TRIGGER_WINDOW_SEC {
            return false;
        }
        self.in_flight = true;
        self.pings_this_turn = self.pings_this_turn.saturating_add(1);
        true
    }
}

const SLEEP_DRIFT_THRESHOLD_MS: f64 = 15_000.0;
const NORMAL_WARN_SILENCE_MS: f64 = 90_000.0;
const NORMAL_STALL_SILENCE_MS: f64 = 180_000.0;
const LONG_TOOL_WARN_SILENCE_MS: f64 = 600_000.0;
const LONG_TOOL_STALL_SILENCE_MS: f64 = 1_200_000.0;

const CONTEXT_WARN_PERCENT: f64 = 75.0;
const CONTEXT_CRITICAL_PERCENT: f64 = 85.0;

const MAX_RECENT_FILES: usize = 20;
const MAX_RECENT_TOOLS: usize = 10;

static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\b(bearer\s+)[A-Za-z0-9._\-]{16,}",
        r"\bsk-[A-Za-z0-9_\-]{16,}",
        r"\bAIza[0-9A-Za-z_\-]{20,}",
        r"\bgh[pousr]_[A-Za-z0-9]{20,}",
        r"\bAKIA[0-9A-Z]{16}\b",
    ]
    .into_iter()
    .filter_map(|p| Regex::new(p).ok())
    .collect()
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StallSeverity {
    Normal,
    Warning,
    Stalled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextPressure {
    Normal,
    Warning,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheHudStatus {
    pub ttl_sec: u64,
    pub remaining_sec: u64,
    pub warm: bool,
    pub expiring_soon: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_ratio_percent: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StallHudStatus {
    pub severity: StallSeverity,
    pub silent_sec: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stalled_tool: Option<String>,
    pub sleep_guard_recoveries: u32,
}

/// Compact optimizer projection embedded into `HudSnapshot` whenever enabled
/// for the session's agent provider.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextGuardHudSummary {
    pub context_pressure: ContextPressure,
    pub saved_noise_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheHudStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stall: Option<StallHudStatus>,
    pub handoff_ready: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolTraceEntry {
    pub tool_name: String,
    pub target: Option<String>,
    pub succeeded: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandoffBrief {
    pub provider: HudProvider,
    pub session_name: Option<String>,
    pub cwd: Option<String>,
    pub context_percent: Option<f64>,
    pub modified_files: Vec<String>,
    pub recent_tools: Vec<ToolTraceEntry>,
    pub stalled_tool: Option<String>,
    pub markdown: String,
}

#[derive(Clone, Debug)]
struct ActiveToolLease {
    name: String,
    target: Option<String>,
    started_at_ms: f64,
    long_running: bool,
}

/// Per-session state machine for KV-cache countdown, stall detection, and
/// rescue handoff generation.
#[derive(Clone, Debug)]
pub struct SessionContextGuard {
    provider: HudProvider,
    ttl_sec: u64,
    last_cache_activity_ms: Option<f64>,
    last_hit_ratio_percent: Option<f64>,
    last_tokens: Option<ContextTokens>,
    last_activity_ms: Option<f64>,
    last_wall_tick_ms: Option<f64>,
    last_mono_tick_ms: Option<f64>,
    sleep_guard_recoveries: u32,
    active_tool: Option<ActiveToolLease>,
    task_status: TaskStatus,
    context_percent: Option<f64>,
    saved_noise_bytes: u64,
    modified_files: VecDeque<String>,
    recent_tools: VecDeque<ToolTraceEntry>,
}

impl SessionContextGuard {
    pub fn new(provider: HudProvider) -> Self {
        let ttl_sec = match provider {
            HudProvider::Antigravity => DEFAULT_LONG_CACHE_TTL_SEC,
            HudProvider::ClaudeCode | HudProvider::Codex | HudProvider::Dsh => {
                DEFAULT_CLAUDE_CACHE_TTL_SEC
            }
        };
        Self {
            provider,
            ttl_sec,
            last_cache_activity_ms: None,
            last_hit_ratio_percent: None,
            last_tokens: None,
            last_activity_ms: None,
            last_wall_tick_ms: None,
            last_mono_tick_ms: None,
            sleep_guard_recoveries: 0,
            active_tool: None,
            task_status: TaskStatus::Idle,
            context_percent: None,
            saved_noise_bytes: 0,
            modified_files: VecDeque::new(),
            recent_tools: VecDeque::new(),
        }
    }

    pub fn provider_id(&self) -> &'static str {
        provider_id(self.provider)
    }

    pub fn record_saved_noise(&mut self, bytes: u64) {
        self.saved_noise_bytes = self.saved_noise_bytes.saturating_add(bytes);
    }

    pub fn set_saved_noise_bytes(&mut self, bytes: u64) {
        self.saved_noise_bytes = bytes;
    }

    pub fn on_task_status(&mut self, status: TaskStatus, now_ms: f64) {
        self.task_status = status;
        self.last_activity_ms = Some(now_ms);
        if matches!(status, TaskStatus::Idle | TaskStatus::NeedsInput) {
            self.active_tool = None;
        }
    }

    /// Idempotent polling sync used by `RuntimeOwner::agent_hud`. Only
    /// advances activity/tool timestamps when observed state actually changes,
    /// so periodic HUD refreshes never mask a genuine stall.
    pub fn sync_observed_state(
        &mut self,
        status: TaskStatus,
        context_percent: Option<f64>,
        tokens: Option<ContextTokens>,
        active_tool: Option<(&str, Option<&str>)>,
        now_ms: f64,
    ) {
        if status != self.task_status {
            self.on_task_status(status, now_ms);
        }
        if context_percent != self.context_percent {
            self.on_context_percent(context_percent, now_ms);
        }
        if let Some(tok) = tokens
            && self.last_tokens != Some(tok)
        {
            self.last_tokens = Some(tok);
            self.on_token_usage(tok, now_ms);
        }
        match active_tool {
            Some((name, target)) => {
                let same_tool = self.active_tool.as_ref().is_some_and(|cur| {
                    cur.name == name && cur.target.as_deref() == target
                });
                if !same_tool {
                    self.on_tool_started(name, target, now_ms);
                }
            }
            None => {
                if let Some(prev) = self.active_tool.take() {
                    self.on_tool_finished(
                        &prev.name,
                        prev.target.as_deref(),
                        status != TaskStatus::Error,
                        now_ms,
                    );
                }
            }
        }
    }

    pub fn on_context_percent(&mut self, percent: Option<f64>, now_ms: f64) {
        self.context_percent = percent;
        if percent.is_some() {
            self.last_activity_ms = Some(now_ms);
        }
    }

    /// Observe token usage from either the request gateway or transcript tail.
    /// Calibrates TTL upward if a cache hit is observed after a gap longer than
    /// the current default TTL.
    pub fn on_token_usage(&mut self, tokens: ContextTokens, now_ms: f64) {
        let input = tokens.input.unwrap_or(0.0).max(0.0);
        let creation = tokens.cache_creation_input.unwrap_or(0.0).max(0.0);
        let read = tokens.cache_read_input.unwrap_or(0.0).max(0.0);
        let total_prompt = input + creation + read;

        if read > 0.0 {
            if let Some(prev_ms) = self.last_cache_activity_ms {
                let gap_sec = ((now_ms - prev_ms).max(0.0) / 1000.0).floor() as u64;
                if gap_sec > self.ttl_sec && gap_sec <= MAX_CALIBRATED_TTL_SEC {
                    self.ttl_sec = gap_sec;
                }
            }
        }

        if total_prompt > 0.0 {
            self.last_hit_ratio_percent = Some(((read / total_prompt) * 100.0).clamp(0.0, 100.0));
        }

        if read > 0.0 || creation > 0.0 || total_prompt > 0.0 {
            self.last_cache_activity_ms = Some(now_ms);
            self.last_activity_ms = Some(now_ms);
        }
    }

    pub fn on_tool_started(&mut self, name: &str, target: Option<&str>, now_ms: f64) {
        let long_running = is_long_running_tool(name, target);
        self.active_tool = Some(ActiveToolLease {
            name: name.to_owned(),
            target: target.map(str::to_owned),
            started_at_ms: now_ms,
            long_running,
        });
        self.task_status = TaskStatus::Running;
        self.last_activity_ms = Some(now_ms);
    }

    pub fn on_tool_finished(&mut self, name: &str, target: Option<&str>, succeeded: bool, now_ms: f64) {
        self.active_tool = None;
        self.last_activity_ms = Some(now_ms);
        self.push_tool_trace(ToolTraceEntry {
            tool_name: name.to_owned(),
            target: target.map(str::to_owned),
            succeeded: Some(succeeded),
        });
    }

    pub fn on_file_modified(&mut self, path: &str, now_ms: f64) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return;
        }
        self.last_activity_ms = Some(now_ms);
        if let Some(pos) = self.modified_files.iter().position(|p| p == trimmed) {
            self.modified_files.remove(pos);
        }
        self.modified_files.push_back(trimmed.to_owned());
        while self.modified_files.len() > MAX_RECENT_FILES {
            self.modified_files.pop_front();
        }
    }

    /// Advances the watchdog clock and applies OS sleep/lid-close drift
    /// protection so waking from suspend never triggers a false stall alert.
    pub fn tick(&mut self, wall_now_ms: f64, mono_now_ms: f64) {
        if let (Some(prev_wall), Some(prev_mono)) = (self.last_wall_tick_ms, self.last_mono_tick_ms) {
            let delta_wall = (wall_now_ms - prev_wall).max(0.0);
            let delta_mono = (mono_now_ms - prev_mono).max(0.0);
            let drift = delta_wall - delta_mono;
            if drift > SLEEP_DRIFT_THRESHOLD_MS {
                self.sleep_guard_recoveries = self.sleep_guard_recoveries.saturating_add(1);
                if let Some(last) = self.last_activity_ms.as_mut() {
                    *last += drift;
                }
                if let Some(tool) = self.active_tool.as_mut() {
                    tool.started_at_ms += drift;
                }
            }
        }
        self.last_wall_tick_ms = Some(wall_now_ms);
        self.last_mono_tick_ms = Some(mono_now_ms);
    }

    /// Computes the compact HUD projection respecting the effective per-agent
    /// settings. Returns `None` when the master switch is off or neither
    /// monitoring feature is enabled.
    pub fn project_hud(
        &self,
        now_ms: f64,
        raw_settings: &AgentOptimizerSettings,
    ) -> Option<ContextGuardHudSummary> {
        let settings = raw_settings.effective_for(self.provider_id());
        if !settings.enabled || (!settings.cache_monitor && !settings.stall_rescue) {
            return None;
        }

        let cache = settings.cache_monitor.then(|| self.compute_cache_status(now_ms)).flatten();
        let stall = settings.stall_rescue.then(|| self.compute_stall_status(now_ms));
        let context_pressure = classify_context_pressure(self.context_percent);
        let handoff_ready = settings.stall_rescue
            && (!self.modified_files.is_empty()
                || !self.recent_tools.is_empty()
                || self.active_tool.is_some()
                || matches!(
                    stall.as_ref().map(|s| s.severity),
                    Some(StallSeverity::Stalled)
                ));

        Some(ContextGuardHudSummary {
            context_pressure,
            saved_noise_bytes: self.saved_noise_bytes,
            cache,
            stall,
            handoff_ready,
        })
    }

    /// Generates a redacted, localized rescue handoff brief ready to copy or
    /// seed into a new/forked session.
    pub fn build_handoff_brief(
        &self,
        session_name: Option<&str>,
        cwd: Option<&str>,
        now_ms: f64,
        locale: InterfaceLocale,
    ) -> HandoffBrief {
        let stall = self.compute_stall_status(now_ms);
        let modified_files: Vec<String> = self.modified_files.iter().cloned().collect();
        let recent_tools: Vec<ToolTraceEntry> = self.recent_tools.iter().cloned().collect();
        let stalled_tool = stall.stalled_tool.clone();

        let markdown = render_handoff_markdown(
            self.provider,
            session_name,
            cwd,
            self.context_percent,
            &modified_files,
            &recent_tools,
            stalled_tool.as_deref(),
            stall.severity,
            stall.silent_sec,
            locale,
        );

        HandoffBrief {
            provider: self.provider,
            session_name: session_name.map(str::to_owned),
            cwd: cwd.map(str::to_owned),
            context_percent: self.context_percent,
            modified_files,
            recent_tools,
            stalled_tool,
            markdown: redact_secrets(&markdown),
        }
    }

    fn compute_cache_status(&self, now_ms: f64) -> Option<CacheHudStatus> {
        let last_ms = self.last_cache_activity_ms?;
        let elapsed_sec = ((now_ms - last_ms).max(0.0) / 1000.0).floor() as u64;
        let remaining_sec = self.ttl_sec.saturating_sub(elapsed_sec);
        let warm = remaining_sec > 0;
        let expiring_soon = warm && remaining_sec <= CACHE_EXPIRING_SOON_SEC;
        Some(CacheHudStatus {
            ttl_sec: self.ttl_sec,
            remaining_sec,
            warm,
            expiring_soon,
            hit_ratio_percent: self.last_hit_ratio_percent,
        })
    }

    fn compute_stall_status(&self, now_ms: f64) -> StallHudStatus {
        if self.task_status != TaskStatus::Running {
            return StallHudStatus {
                severity: StallSeverity::Normal,
                silent_sec: 0,
                stalled_tool: None,
                sleep_guard_recoveries: self.sleep_guard_recoveries,
            };
        }

        let reference_ms = self
            .active_tool
            .as_ref()
            .map(|t| t.started_at_ms)
            .or(self.last_activity_ms)
            .unwrap_or(now_ms);
        let silent_ms = (now_ms - reference_ms).max(0.0);
        let (warn_ms, stall_ms) = if self.active_tool.as_ref().is_some_and(|t| t.long_running) {
            (LONG_TOOL_WARN_SILENCE_MS, LONG_TOOL_STALL_SILENCE_MS)
        } else {
            (NORMAL_WARN_SILENCE_MS, NORMAL_STALL_SILENCE_MS)
        };

        let severity = if silent_ms >= stall_ms {
            StallSeverity::Stalled
        } else if silent_ms >= warn_ms {
            StallSeverity::Warning
        } else {
            StallSeverity::Normal
        };

        let stalled_tool = (severity != StallSeverity::Normal)
            .then(|| {
                self.active_tool.as_ref().map(|t| match t.target.as_deref() {
                    Some(target) if !target.trim().is_empty() => format!("{} ({})", t.name, target),
                    _ => t.name.clone(),
                })
            })
            .flatten();

        StallHudStatus {
            severity,
            silent_sec: (silent_ms / 1000.0).floor() as u64,
            stalled_tool,
            sleep_guard_recoveries: self.sleep_guard_recoveries,
        }
    }

    fn push_tool_trace(&mut self, entry: ToolTraceEntry) {
        self.recent_tools.push_back(entry);
        while self.recent_tools.len() > MAX_RECENT_TOOLS {
            self.recent_tools.pop_front();
        }
    }
}

pub fn provider_id(provider: HudProvider) -> &'static str {
    match provider {
        HudProvider::ClaudeCode => "claude-code",
        HudProvider::Codex => "codex",
        HudProvider::Antigravity => "antigravity",
        HudProvider::Dsh => "dsh",
    }
}

pub fn redact_secrets(input: &str) -> String {
    let mut out = input.to_owned();
    for re in SECRET_PATTERNS.iter() {
        out = re.replace_all(&out, "[REDACTED]").into_owned();
    }
    out
}

fn classify_context_pressure(percent: Option<f64>) -> ContextPressure {
    match percent {
        Some(p) if p >= CONTEXT_CRITICAL_PERCENT => ContextPressure::Critical,
        Some(p) if p >= CONTEXT_WARN_PERCENT => ContextPressure::Warning,
        _ => ContextPressure::Normal,
    }
}

fn is_long_running_tool(name: &str, target: Option<&str>) -> bool {
    let lower_name = name.to_ascii_lowercase();
    let lower_target = target.unwrap_or_default().to_ascii_lowercase();
    if lower_name.contains("bash") || lower_name.contains("command") || lower_name.contains("shell") {
        return [
            "cargo ",
            "pnpm ",
            "npm ",
            "yarn ",
            "pytest",
            "playwright",
            "vitest",
            "jest",
            "docker ",
            "xcodebuild",
            "gradle",
            "make ",
            "build",
            "test",
        ]
        .iter()
        .any(|kw| lower_target.contains(kw));
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn render_handoff_markdown(
    provider: HudProvider,
    session_name: Option<&str>,
    cwd: Option<&str>,
    context_percent: Option<f64>,
    modified_files: &[String],
    recent_tools: &[ToolTraceEntry],
    stalled_tool: Option<&str>,
    severity: StallSeverity,
    silent_sec: u64,
    locale: InterfaceLocale,
) -> String {
    let is_en = matches!(locale, InterfaceLocale::English);
    let mut lines = Vec::new();
    if is_en {
        lines.push("# Session Handoff Brief".to_owned());
        lines.push(format!("- **Agent**: {}", provider_id(provider)));
        if let Some(name) = session_name.filter(|s| !s.trim().is_empty()) {
            lines.push(format!("- **Session**: {}", name.trim()));
        }
        if let Some(dir) = cwd.filter(|s| !s.trim().is_empty()) {
            lines.push(format!("- **Workspace**: `{}`", dir.trim()));
        }
        if let Some(pct) = context_percent {
            lines.push(format!("- **Context Usage**: {:.0}%", pct));
        }
        if severity != StallSeverity::Normal {
            let tool_note = stalled_tool.unwrap_or("waiting for model response");
            lines.push(format!(
                "- **Interrupted At**: {} (silent {}s)",
                tool_note, silent_sec
            ));
        }
        if !modified_files.is_empty() {
            lines.push(String::new());
            lines.push("## Modified Files".to_owned());
            for file in modified_files {
                lines.push(format!("- `{file}`"));
            }
        }
        if !recent_tools.is_empty() {
            lines.push(String::new());
            lines.push("## Recent Steps".to_owned());
            for step in recent_tools {
                let status = match step.succeeded {
                    Some(true) => "ok",
                    Some(false) => "failed",
                    None => "running",
                };
                match step.target.as_deref() {
                    Some(target) if !target.trim().is_empty() => {
                        lines.push(format!("- `{}` ({target}) — {status}", step.tool_name));
                    }
                    _ => lines.push(format!("- `{}` — {status}", step.tool_name)),
                }
            }
        }
        lines.push(String::new());
        lines.push(
            "Please continue from the state above without repeating completed edits.".to_owned(),
        );
    } else {
        lines.push("# 会话交接摘要".to_owned());
        lines.push(format!("- **Agent**：{}", provider_id(provider)));
        if let Some(name) = session_name.filter(|s| !s.trim().is_empty()) {
            lines.push(format!("- **原会话**：{}", name.trim()));
        }
        if let Some(dir) = cwd.filter(|s| !s.trim().is_empty()) {
            lines.push(format!("- **工作目录**：`{}`", dir.trim()));
        }
        if let Some(pct) = context_percent {
            lines.push(format!("- **原上下文占用**：{:.0}%", pct));
        }
        if severity != StallSeverity::Normal {
            let tool_note = stalled_tool.unwrap_or("等待模型响应");
            lines.push(format!(
                "- **中断位置**：{}（静默 {} 秒）",
                tool_note, silent_sec
            ));
        }
        if !modified_files.is_empty() {
            lines.push(String::new());
            lines.push("## 已修改文件".to_owned());
            for file in modified_files {
                lines.push(format!("- `{file}`"));
            }
        }
        if !recent_tools.is_empty() {
            lines.push(String::new());
            lines.push("## 最近执行步骤".to_owned());
            for step in recent_tools {
                let status = match step.succeeded {
                    Some(true) => "成功",
                    Some(false) => "失败",
                    None => "进行中",
                };
                match step.target.as_deref() {
                    Some(target) if !target.trim().is_empty() => {
                        lines.push(format!("- `{}`（{target}）— {status}", step.tool_name));
                    }
                    _ => lines.push(format!("- `{}` — {status}", step.tool_name)),
                }
            }
        }
        lines.push(String::new());
        lines.push("请基于以上进度直接继续推进，无需重复已完成改动。".to_owned());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_countdown_and_ttl_calibration_work() {
        let mut guard = SessionContextGuard::new(HudProvider::ClaudeCode);
        let settings = AgentOptimizerSettings::default();
        let t0 = 1_000_000.0;

        guard.on_token_usage(
            ContextTokens {
                input: Some(10_000.0),
                cache_creation_input: Some(40_000.0),
                cache_read_input: Some(0.0),
                ..Default::default()
            },
            t0,
        );
        let hud = guard.project_hud(t0 + 250_000.0, &settings).unwrap();
        let cache = hud.cache.unwrap();
        assert_eq!(cache.ttl_sec, 300);
        assert_eq!(cache.remaining_sec, 50);
        assert!(cache.warm);
        assert!(cache.expiring_soon);

        // Observing a cache hit after 420s calibrates TTL upward.
        guard.on_token_usage(
            ContextTokens {
                input: Some(2_000.0),
                cache_creation_input: Some(0.0),
                cache_read_input: Some(48_000.0),
                ..Default::default()
            },
            t0 + 420_000.0,
        );
        let hud2 = guard.project_hud(t0 + 430_000.0, &settings).unwrap();
        let cache2 = hud2.cache.unwrap();
        assert_eq!(cache2.ttl_sec, 420);
        assert_eq!(cache2.remaining_sec, 410);
        assert!((cache2.hit_ratio_percent.unwrap() - 96.0).abs() < 0.01);
    }

    #[test]
    fn sleep_drift_guard_prevents_false_stall_alerts() {
        let mut guard = SessionContextGuard::new(HudProvider::Codex);
        let settings = AgentOptimizerSettings::default();
        let wall0 = 10_000_000.0;
        let mono0 = 1_000.0;

        guard.tick(wall0, mono0);
        guard.on_tool_started("Read", Some("src/main.rs"), wall0);

        // Laptop slept for 10 minutes: wall clock jumped 600s, monotonic advanced 2s.
        let wall1 = wall0 + 600_000.0;
        let mono1 = mono0 + 2_000.0;
        guard.tick(wall1, mono1);

        let hud = guard.project_hud(wall1, &settings).unwrap();
        let stall = hud.stall.unwrap();
        assert_eq!(stall.severity, StallSeverity::Normal);
        assert_eq!(stall.sleep_guard_recoveries, 1);

        // Genuine 200s silence without sleep triggers Stalled.
        let wall2 = wall1 + 200_000.0;
        let mono2 = mono1 + 200_000.0;
        guard.tick(wall2, mono2);
        let hud2 = guard.project_hud(wall2, &settings).unwrap();
        assert_eq!(hud2.stall.unwrap().severity, StallSeverity::Stalled);
    }

    #[test]
    fn long_running_build_uses_extended_stall_window() {
        let mut guard = SessionContextGuard::new(HudProvider::ClaudeCode);
        let settings = AgentOptimizerSettings::default();
        let t0 = 5_000_000.0;
        guard.on_tool_started("Bash", Some("cargo test --workspace"), t0);

        // 5 minutes into a cargo build should still be Normal.
        let hud = guard.project_hud(t0 + 300_000.0, &settings).unwrap();
        assert_eq!(hud.stall.unwrap().severity, StallSeverity::Normal);
    }

    #[test]
    fn handoff_brief_redacts_secrets_and_respects_locale() {
        let mut guard = SessionContextGuard::new(HudProvider::Antigravity);
        let t0 = 2_000_000.0;
        guard.on_context_percent(Some(88.0), t0);
        guard.on_file_modified("apps/native/runtime/src/host.rs", t0);
        guard.on_tool_finished(
            "Bash",
            Some("curl -H 'Authorization: Bearer sk-secret1234567890abcdef'"),
            false,
            t0 + 1_000.0,
        );

        let brief_zh = guard.build_handoff_brief(
            Some("修复路由"),
            Some("/repo/matou"),
            t0 + 2_000.0,
            InterfaceLocale::Chinese,
        );
        assert!(brief_zh.markdown.contains("# 会话交接摘要"));
        assert!(brief_zh.markdown.contains("apps/native/runtime/src/host.rs"));
        assert!(!brief_zh.markdown.contains("sk-secret1234567890abcdef"));
        assert!(brief_zh.markdown.contains("[REDACTED]"));

        let brief_en = guard.build_handoff_brief(
            Some("Fix router"),
            Some("/repo/matou"),
            t0 + 2_000.0,
            InterfaceLocale::English,
        );
        assert!(brief_en.markdown.contains("# Session Handoff Brief"));
    }

    #[test]
    fn disabled_master_switch_returns_no_hud_projection() {
        let mut guard = SessionContextGuard::new(HudProvider::ClaudeCode);
        guard.on_context_percent(Some(90.0), 1_000.0);
        let off = AgentOptimizerSettings {
            enabled: false,
            ..Default::default()
        };
        assert!(guard.project_hud(2_000.0, &off).is_none());
    }

    #[test]
    fn managed_cli_instructions_only_emits_when_opted_in() {
        let dir = tempfile::tempdir().unwrap();
        assert!(managed_cli_instructions(dir.path(), "claude-code").is_none());

        let cfg = ContextOptimizerConfig::for_data_root(dir.path());
        cfg.set_agent_settings(
            "claude-code",
            AgentOptimizerSettings {
                enabled: true,
                cli_tools: true,
                ..Default::default()
            },
        )
        .unwrap();
        let text = managed_cli_instructions(dir.path(), "claude-code").unwrap();
        assert!(text.contains("mt ctx outline"));
        assert!(managed_cli_instructions(dir.path(), "codex").is_none());
    }

    #[test]
    fn keepalive_controller_respects_opt_in_cap_subagents_and_turn_reset() {
        let mut ctrl = KeepaliveController::new();
        let expiring = CacheHudStatus {
            ttl_sec: 300,
            remaining_sec: 30,
            warm: true,
            expiring_soon: true,
            hit_ratio_percent: Some(90.0),
        };
        // Default settings have keepalive OFF.
        let default_settings = AgentOptimizerSettings::default();
        assert!(!ctrl.should_dispatch("claude-code", &default_settings, Some(&expiring), false, false));

        // Opt-in with max 1 ping per turn.
        let enabled = AgentOptimizerSettings {
            enabled: true,
            cache_keepalive: true,
            cache_keepalive_max_per_turn: 1,
            ..Default::default()
        };
        // Subagents & DSH are always suppressed.
        assert!(!ctrl.should_dispatch("claude-code", &enabled, Some(&expiring), true, false));
        assert!(!ctrl.should_dispatch("dsh", &enabled, Some(&expiring), false, false));

        // Primary session fires once, blocks second ping until user sends a message.
        assert!(ctrl.should_dispatch("claude-code", &enabled, Some(&expiring), false, false));
        ctrl.on_ping_completed();
        assert!(!ctrl.should_dispatch("claude-code", &enabled, Some(&expiring), false, false));

        ctrl.on_user_message();
        assert!(ctrl.should_dispatch("claude-code", &enabled, Some(&expiring), false, false));
    }
}
