//! Built-in context & KV-cache optimizer engine (Phase 1: observation,
//! KV-cache countdown, context pressure reminders, and redacted handoff briefs).
//!
//! All state stays in-process inside `matou-native-runtime` with zero MCP
//! servers and zero extra background processes. Behavior is gated per-agent by
//! `matou_native_core::context_optimizer_config::AgentOptimizerSettings`.

use std::collections::{HashSet, VecDeque};

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
use serde_json::{Value, json};
use std::sync::LazyLock;

use crate::agent_hud_projection::{ContextUsageSample, HudProvider, ToolActivity, ToolStatus};

#[path = "rescue.rs"]
pub(crate) mod rescue;

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
pub enum ContextPressure {
    Normal,
    Warning,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheTiming {
    Precise,
    Estimate,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheHudStatus {
    pub timing: CacheTiming,
    pub ttl_sec: u64,
    pub remaining_sec: u64,
    pub warm: bool,
    pub expiring_soon: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_ratio_percent: Option<f64>,
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
    pub handoff_ready: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolTraceEntry {
    pub tool_name: String,
    pub target: Option<String>,
    pub succeeded: Option<bool>,
    pub exit_code: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandoffBrief {
    pub provider: HudProvider,
    pub session_name: Option<String>,
    pub cwd: Option<String>,
    pub context_percent: Option<f64>,
    pub modified_files: Vec<String>,
    pub recent_tools: Vec<ToolTraceEntry>,
    pub log_ids: Vec<String>,
    pub markdown: String,
}

/// Per-session KV-cache, context pressure, and handoff state.
#[derive(Clone, Debug)]
pub struct SessionContextGuard {
    provider: HudProvider,
    usage: Option<ContextUsageSample>,
    warned_pressure: ContextPressure,
    warned_cache_deadline_ms: Option<f64>,
    has_active_tool: bool,
    context_percent: Option<f64>,
    saved_noise_bytes: u64,
    modified_files: VecDeque<String>,
    recent_tools: VecDeque<ToolTraceEntry>,
    observed_last_tool: Option<ToolActivity>,
}

impl SessionContextGuard {
    pub fn new(provider: HudProvider) -> Self {
        Self {
            provider,
            usage: None,
            warned_pressure: ContextPressure::Normal,
            warned_cache_deadline_ms: None,
            has_active_tool: false,
            context_percent: None,
            saved_noise_bytes: 0,
            modified_files: VecDeque::new(),
            recent_tools: VecDeque::new(),
            observed_last_tool: None,
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

    pub(crate) fn set_active_tool(&mut self, active: bool) {
        self.has_active_tool = active;
    }

    pub(crate) fn observe_last_tool(&mut self, tool: Option<&ToolActivity>) {
        if self.observed_last_tool.as_ref() == tool {
            return;
        }
        self.observed_last_tool = tool.cloned();
        if let Some(tool) = tool.filter(|tool| tool.status != ToolStatus::Running) {
            self.record_tool_result(
                &tool.tool.name,
                tool.tool.target.as_deref(),
                tool.status == ToolStatus::Completed,
                None,
            );
        }
    }

    pub(crate) fn has_critical_context(&self) -> bool {
        classify_context_pressure(self.context_percent) == ContextPressure::Critical
    }

    pub(crate) fn context_percent(&self) -> Option<f64> {
        self.context_percent
    }

    pub fn on_context_percent(&mut self, percent: Option<f64>) {
        self.context_percent = percent;
    }

    pub(crate) fn observe_usage(&mut self, usage: Option<ContextUsageSample>) {
        self.usage = usage;
    }

    pub(crate) fn on_compaction(&mut self) {
        self.context_percent = None;
        self.usage = None;
        self.warned_pressure = ContextPressure::Normal;
        self.warned_cache_deadline_ms = None;
    }

    pub(crate) fn pending_reminders(
        &self,
        now_ms: f64,
        settings: &AgentOptimizerSettings,
    ) -> impl Iterator<Item = ContextReminder> {
        let pressure = classify_context_pressure(self.context_percent);
        let water = settings.enabled
            && settings.cache_monitor
            && match pressure {
                ContextPressure::Normal => false,
                ContextPressure::Warning => self.warned_pressure == ContextPressure::Normal,
                ContextPressure::Critical => self.warned_pressure != ContextPressure::Critical,
            };
        let cache = (settings.enabled && settings.cache_monitor && settings.cache_expiry_notify)
            .then(|| self.cache_deadline())
            .flatten()
            .filter(|(deadline, _, _)| {
                let remaining = *deadline - now_ms;
                remaining > 0.0
                    && remaining <= CACHE_EXPIRING_SOON_SEC as f64 * 1000.0
                    && self.warned_cache_deadline_ms != Some(*deadline)
            });
        water
            .then_some(ContextReminder::Pressure(pressure))
            .into_iter()
            .chain(
                cache.map(|(deadline_ms, timing, _)| ContextReminder::CacheExpiry {
                    deadline_ms,
                    timing,
                }),
            )
    }

    pub(crate) fn mark_reminded(&mut self, reminder: ContextReminder) {
        match reminder {
            ContextReminder::Pressure(pressure) => self.warned_pressure = pressure,
            ContextReminder::CacheExpiry { deadline_ms, .. } => {
                self.warned_cache_deadline_ms = Some(deadline_ms);
            }
        }
    }

    pub(crate) fn record_tool_result(
        &mut self,
        name: &str,
        target: Option<&str>,
        succeeded: bool,
        exit_code: Option<i64>,
    ) {
        self.push_tool_trace(ToolTraceEntry {
            tool_name: name.to_owned(),
            target: target.map(str::to_owned),
            succeeded: Some(succeeded),
            exit_code,
        });
    }

    pub(crate) fn record_modified_file(&mut self, path: &str) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Some(pos) = self.modified_files.iter().position(|p| p == trimmed) {
            self.modified_files.remove(pos);
        }
        self.modified_files.push_back(trimmed.to_owned());
        while self.modified_files.len() > MAX_RECENT_FILES {
            self.modified_files.pop_front();
        }
    }

    /// Computes the compact HUD projection respecting the effective per-agent
    /// settings. Requires both the master switch and cache monitoring.
    pub fn project_hud(
        &self,
        now_ms: f64,
        raw_settings: &AgentOptimizerSettings,
    ) -> Option<ContextGuardHudSummary> {
        let settings = raw_settings.effective_for(self.provider_id());
        if !settings.enabled || !settings.cache_monitor {
            return None;
        }

        let context_pressure = classify_context_pressure(self.context_percent);
        let handoff_ready = context_pressure == ContextPressure::Critical
            || !self.modified_files.is_empty()
            || !self.recent_tools.is_empty()
            || self.has_active_tool;

        Some(ContextGuardHudSummary {
            context_pressure,
            saved_noise_bytes: self.saved_noise_bytes,
            cache: Some(self.compute_cache_status(now_ms)),
            handoff_ready,
        })
    }

    /// Generates a redacted, localized rescue handoff brief ready to copy or
    /// seed into a new/forked session.
    pub fn build_handoff_brief(
        &self,
        session_name: Option<&str>,
        cwd: Option<&str>,
        locale: InterfaceLocale,
    ) -> HandoffBrief {
        self.build_handoff_with_observations(session_name, cwd, locale, &[], &[], &[])
    }

    pub(crate) fn recorded_modified_files(&self) -> impl Iterator<Item = &str> {
        self.modified_files.iter().map(String::as_str)
    }

    pub(crate) fn build_handoff_with_observations(
        &self,
        session_name: Option<&str>,
        cwd: Option<&str>,
        locale: InterfaceLocale,
        modified_files: &[String],
        log_ids: &[String],
        secrets: &[String],
    ) -> HandoffBrief {
        let redact = |text: &str| rescue::redact(text, secrets);
        let session_name = session_name.map(redact);
        let cwd = cwd.map(redact);
        let mut seen_files = HashSet::new();
        let modified_files: Vec<String> = self
            .recorded_modified_files()
            .chain(modified_files.iter().map(String::as_str))
            .map(redact)
            .filter(|path| seen_files.insert(path.clone()))
            .collect();
        let mut recent_tools: Vec<_> = self
            .recent_tools
            .iter()
            .map(|tool| ToolTraceEntry {
                tool_name: redact(&tool.tool_name),
                target: tool.target.as_deref().map(redact),
                succeeded: tool.succeeded,
                exit_code: tool.exit_code,
            })
            .collect();

        let mut markdown = render_handoff_markdown(
            self.provider,
            session_name.as_deref(),
            cwd.as_deref(),
            self.context_percent,
            &modified_files,
            &recent_tools,
            locale,
        );
        for tool in &mut recent_tools {
            if let Some(target) = &mut tool.target {
                target.truncate(target.floor_char_boundary(target.len().min(640)));
            }
        }

        if !log_ids.is_empty() {
            markdown.push_str(locale.text(
                "\n\n## 可检索的截断日志\n",
                "\n\n## Retrievable truncated logs\n",
            ));
            for id in log_ids {
                markdown.push_str(&format!("- `{id}`\n"));
            }
        }

        HandoffBrief {
            provider: self.provider,
            session_name,
            cwd,
            context_percent: self.context_percent,
            modified_files,
            recent_tools,
            log_ids: log_ids.to_vec(),
            markdown: redact_secrets(&markdown),
        }
    }

    fn cache_deadline(&self) -> Option<(f64, CacheTiming, u64)> {
        let usage = self.usage?;
        if let Some(expires_at) = usage.cache_expires_at_ms {
            let ttl = usage
                .observed_at_ms
                .map_or(0, |at| ((expires_at - at).max(0.0) / 1000.0).ceil() as u64);
            return Some((expires_at, CacheTiming::Precise, ttl));
        }
        let at = usage.observed_at_ms?;
        let ttl = usage.estimated_cache_ttl_sec?;
        usage.tokens.has_cache_activity().then_some((
            at + ttl as f64 * 1000.0,
            CacheTiming::Estimate,
            ttl,
        ))
    }

    fn compute_cache_status(&self, now_ms: f64) -> CacheHudStatus {
        let (remaining_sec, timing, ttl_sec) = self.cache_deadline().map_or(
            (0, CacheTiming::Unknown, 0),
            |(deadline, timing, ttl)| {
                (
                    ((deadline - now_ms).max(0.0) / 1000.0).ceil() as u64,
                    timing,
                    ttl,
                )
            },
        );
        let warm = remaining_sec > 0;
        CacheHudStatus {
            timing,
            ttl_sec,
            remaining_sec,
            warm,
            expiring_soon: warm && remaining_sec <= CACHE_EXPIRING_SOON_SEC,
            hit_ratio_percent: self.usage.and_then(|usage| usage.tokens.cache_hit_ratio()),
        }
    }

    fn push_tool_trace(&mut self, entry: ToolTraceEntry) {
        if let Some(previous) = self.recent_tools.back_mut().filter(|previous| {
            previous.tool_name == entry.tool_name && previous.succeeded == entry.succeeded
        }) {
            // Only these HUD targets are shortened. File paths may literally end in `...`.
            if matches!(entry.tool_name.as_str(), "Bash" | "run_command")
                && let (Some(previous_target), Some(target)) = (&previous.target, &entry.target)
            {
                if is_truncated_target(target, previous_target) {
                    return;
                }
                if is_truncated_target(previous_target, target) {
                    let exit_code = entry.exit_code.or(previous.exit_code);
                    *previous = ToolTraceEntry { exit_code, ..entry };
                    return;
                }
            }
            if entry.exit_code.is_none() && previous.target == entry.target {
                return;
            }
        }
        self.recent_tools.push_back(entry);
        while self.recent_tools.len() > MAX_RECENT_TOOLS {
            self.recent_tools.pop_front();
        }
    }
}

fn is_truncated_target(short: &str, full: &str) -> bool {
    short.len() < full.len()
        && short
            .strip_suffix("...")
            .is_some_and(|prefix| !prefix.is_empty() && full.starts_with(prefix))
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ContextReminder {
    Pressure(ContextPressure),
    CacheExpiry {
        deadline_ms: f64,
        timing: CacheTiming,
    },
}

impl ContextReminder {
    pub(crate) fn notification(self, locale: InterfaceLocale) -> Value {
        let open = locale.text("点击查看这张卡片。", "Click to open the card.");
        let (kind, title, body, sound) = match self {
            Self::Pressure(ContextPressure::Warning) => (
                "context-water-warning",
                locale.text("上下文占用达到 75%", "Context usage reached 75%"),
                open,
                false,
            ),
            Self::Pressure(_) => (
                "context-water-critical",
                locale.text("上下文占用达到 85%", "Context usage reached 85%"),
                open,
                false,
            ),
            Self::CacheExpiry { timing: CacheTiming::Estimate, .. } => (
                "context-cache-expiry",
                locale.text("缓存预计即将过期", "Cache estimated to expire soon"),
                locale.text(
                    "按该路由的默认时长估算，剩余不超过 60 秒。点击查看这张卡片。",
                    "About 60 seconds or less based on the route’s default lifetime. Click to open the card.",
                ),
                true,
            ),
            Self::CacheExpiry { .. } => (
                "context-cache-expiry",
                locale.text("缓存即将过期", "Cache expiring soon"),
                locale.text(
                    "路由报告的到期时间距现在不超过 60 秒。点击查看这张卡片。",
                    "The route reports an expiry within 60 seconds. Click to open the card.",
                ),
                true,
            ),
        };
        json!({"eventType": kind, "title": title, "body": body, "sound": sound})
    }
}

fn classify_context_pressure(percent: Option<f64>) -> ContextPressure {
    match percent {
        Some(p) if p >= CONTEXT_CRITICAL_PERCENT => ContextPressure::Critical,
        Some(p) if p >= CONTEXT_WARN_PERCENT => ContextPressure::Warning,
        _ => ContextPressure::Normal,
    }
}

fn render_handoff_markdown(
    provider: HudProvider,
    session_name: Option<&str>,
    cwd: Option<&str>,
    context_percent: Option<f64>,
    modified_files: &[String],
    recent_tools: &[ToolTraceEntry],
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
        if !modified_files.is_empty() {
            lines.push(String::new());
            lines.push("## Modified Files".to_owned());
            lines.push("Includes tool writes and shared worktree changes.".to_owned());
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
                let status = step.exit_code.map_or_else(
                    || status.to_owned(),
                    |code| format!("{status} (exit {code})"),
                );
                match handoff_target(step, cwd) {
                    Some(target) => {
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
        if !modified_files.is_empty() {
            lines.push(String::new());
            lines.push("## 已修改文件".to_owned());
            lines.push("来自工具写入记录与共享工作树。".to_owned());
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
                let status = step.exit_code.map_or_else(
                    || status.to_owned(),
                    |code| format!("{status}（退出码 {code}）"),
                );
                match handoff_target(step, cwd) {
                    Some(target) => {
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

fn handoff_target(step: &ToolTraceEntry, cwd: Option<&str>) -> Option<String> {
    let target = step.target.as_deref()?.trim();
    let command = matches!(
        step.tool_name.as_str(),
        "Bash" | "run_command" | "shell" | "exec_command"
    );
    let target = if command {
        cwd.and_then(|cwd| strip_workspace_cd(target, cwd))
            .unwrap_or(target)
    } else {
        target
    };
    let separator = if command { "; " } else { " " };
    let mut text = target
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(separator);
    if let Some((end, _)) = text.char_indices().nth(320) {
        text.truncate(end);
        text.push('…');
    }
    (!text.is_empty()).then_some(text)
}

fn strip_workspace_cd<'a>(target: &'a str, cwd: &str) -> Option<&'a str> {
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return None;
    }
    let after_cd = target.strip_prefix("cd")?;
    if !after_cd.starts_with([' ', '\t']) {
        return None;
    }
    let after_cd = after_cd.trim_start_matches([' ', '\t']);
    let quoted = [format!("'{cwd}'"), format!("\"{cwd}\"")];
    let after_path = quoted
        .iter()
        .map(String::as_str)
        .chain((!cwd.contains(char::is_whitespace)).then_some(cwd))
        .find_map(|path| after_cd.strip_prefix(path))?
        .trim_start_matches([' ', '\t']);
    let command = after_path
        .strip_prefix("&&")
        .or_else(|| after_path.strip_prefix(';'))
        .or_else(|| after_path.strip_prefix("\r\n"))
        .or_else(|| after_path.strip_prefix('\n'))?
        .trim_start();
    (!command.is_empty()).then_some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::agent_hud_projection::ContextTokens;

    fn cache_sample(at: f64, expires_at: Option<f64>) -> ContextUsageSample {
        ContextUsageSample {
            tokens: ContextTokens {
                input: Some(10_000.),
                output: Some(2_000.),
                cache_creation_input: Some(20_000.),
                cache_read_input: Some(40_000.),
                total: None,
            },
            observed_at_ms: Some(at),
            cache_expires_at_ms: expires_at,
            estimated_cache_ttl_sec: Some(300),
            window: Some(200_000),
        }
    }

    #[test]
    fn cache_timing_uses_request_timestamp_and_distinguishes_all_three_levels() {
        let mut guard = SessionContextGuard::new(HudProvider::ClaudeCode);
        let settings = AgentOptimizerSettings::default();
        let at = 1_000_000.;
        let sample = cache_sample(at, None);
        guard.observe_usage(Some(sample));
        let cache = guard
            .project_hud(at + 250_000., &settings)
            .unwrap()
            .cache
            .unwrap();
        assert_eq!(cache.timing, CacheTiming::Estimate);
        assert_eq!(cache.remaining_sec, 50);
        assert!(cache.expiring_soon);
        assert!((cache.hit_ratio_percent.unwrap() - 57.142857).abs() < 0.001);

        guard.observe_usage(Some(cache_sample(at, Some(at + 310_500.))));
        let cache = guard
            .project_hud(at + 250_000., &settings)
            .unwrap()
            .cache
            .unwrap();
        assert_eq!(cache.timing, CacheTiming::Precise);
        assert_eq!(cache.remaining_sec, 61);
        assert!(!cache.expiring_soon);

        for sample in [
            ContextUsageSample {
                observed_at_ms: None,
                ..sample
            },
            ContextUsageSample {
                estimated_cache_ttl_sec: None,
                ..sample
            },
            ContextUsageSample {
                tokens: ContextTokens {
                    input: Some(10_000.),
                    ..Default::default()
                },
                ..sample
            },
        ] {
            guard.observe_usage(Some(sample));
            let cache = guard
                .project_hud(at + 250_000., &settings)
                .unwrap()
                .cache
                .unwrap();
            assert_eq!(cache.timing, CacheTiming::Unknown);
            assert!(!cache.warm);
        }
    }

    #[test]
    fn reminders_fire_once_per_water_level_and_deadline_until_compaction() {
        let mut guard = SessionContextGuard::new(HudProvider::ClaudeCode);
        let at = 1_000_000.;
        let settings = AgentOptimizerSettings {
            cache_expiry_notify: true,
            ..Default::default()
        };
        let due = at + 240_000.;
        guard.observe_usage(Some(cache_sample(at, None)));
        guard.on_context_percent(Some(74.99));
        assert_eq!(guard.pending_reminders(due - 1., &settings).count(), 0);
        guard.on_context_percent(Some(75.));
        let reminders: Vec<_> = guard.pending_reminders(due, &settings).collect();
        assert_eq!(
            reminders,
            [
                ContextReminder::Pressure(ContextPressure::Warning),
                ContextReminder::CacheExpiry {
                    deadline_ms: at + 300_000.,
                    timing: CacheTiming::Estimate,
                }
            ]
        );
        for reminder in reminders {
            guard.mark_reminded(reminder);
        }
        assert_eq!(guard.pending_reminders(due + 1., &settings).count(), 0);
        guard.on_context_percent(Some(85.));
        let critical = ContextReminder::Pressure(ContextPressure::Critical);
        assert_eq!(
            guard.pending_reminders(due, &settings).collect::<Vec<_>>(),
            [critical]
        );
        guard.mark_reminded(critical);
        guard.on_context_percent(Some(80.));
        assert_eq!(guard.pending_reminders(due, &settings).count(), 0);

        guard.on_compaction();
        assert_eq!(guard.pending_reminders(due, &settings).count(), 0);
        guard.on_context_percent(Some(85.));
        guard.observe_usage(Some(cache_sample(at, None)));
        for disabled in [
            AgentOptimizerSettings {
                enabled: false,
                ..settings.clone()
            },
            AgentOptimizerSettings {
                cache_monitor: false,
                ..settings.clone()
            },
        ] {
            assert_eq!(guard.pending_reminders(due, &disabled).count(), 0);
        }
        assert_eq!(
            guard
                .pending_reminders(due, &AgentOptimizerSettings::default())
                .collect::<Vec<_>>(),
            [critical]
        );
        assert_eq!(guard.pending_reminders(due, &settings).count(), 2);
    }

    #[test]
    fn handoff_brief_redacts_secrets_and_respects_locale() {
        let mut guard = SessionContextGuard::new(HudProvider::Antigravity);
        guard.on_context_percent(Some(88.0));
        guard.record_modified_file("apps/native/runtime/src/host.rs");
        guard.record_tool_result(
            "Bash",
            Some("curl -H 'Authorization: Bearer sk-secret1234567890abcdef'"),
            false,
            None,
        );

        let brief_zh = guard.build_handoff_brief(
            Some("修复路由"),
            Some("/repo/matou"),
            InterfaceLocale::Chinese,
        );
        assert!(brief_zh.markdown.contains("# 会话交接摘要"));
        assert!(brief_zh.markdown.contains("apps/native/runtime/src/host.rs"));
        assert!(!brief_zh.markdown.contains("sk-secret1234567890abcdef"));
        assert!(brief_zh.markdown.contains("[REDACTED]"));

        let brief_en = guard.build_handoff_brief(
            Some("Fix router"),
            Some("/repo/matou"),
            InterfaceLocale::English,
        );
        assert!(brief_en.markdown.contains("# Session Handoff Brief"));
    }

    #[test]
    fn handoff_keeps_full_commands_when_hud_reports_truncated_targets() {
        use crate::agent_hud_projection::{RunningTool, target_for_tool};

        let mut guard = SessionContextGuard::new(HudProvider::ClaudeCode);
        let command = "cd '/repo/long workspace name' && printf first\n\tprintf    second";
        let hud_tool = ToolActivity {
            tool: RunningTool {
                name: "Bash".into(),
                target: target_for_tool("Bash", &serde_json::json!({"command": command})),
            },
            status: ToolStatus::Completed,
        };
        for hud_first in [false, true] {
            guard.recent_tools.clear();
            guard.observed_last_tool = None;
            if hud_first {
                guard.observe_last_tool(Some(&hud_tool));
            }
            guard.record_tool_result("Bash", Some(command), true, Some(0));
            if !hud_first {
                guard.observe_last_tool(Some(&hud_tool));
            }

            for locale in [InterfaceLocale::Chinese, InterfaceLocale::English] {
                let brief =
                    guard.build_handoff_brief(None, Some("/repo/long workspace name"), locale);
                assert_eq!(brief.recent_tools.len(), 1);
                assert_eq!(brief.recent_tools[0].target.as_deref(), Some(command));
                assert_eq!(brief.recent_tools[0].exit_code, Some(0));
                let steps: Vec<_> = brief
                    .markdown
                    .lines()
                    .filter(|line| line.starts_with("- `Bash`"))
                    .collect();
                assert_eq!(steps.len(), 1);
                assert!(steps[0].contains("printf first; printf second"));
                assert!(!steps[0].contains("cd "));
            }
        }

        guard.record_tool_result("Bash", Some(command), false, Some(1));
        assert_eq!(guard.recent_tools.len(), 2);

        guard.record_tool_result("Read", Some("/repo/file..."), true, None);
        guard.record_tool_result("Read", Some("/repo/filename"), true, None);
        assert_eq!(guard.recent_tools.len(), 4);
    }

    #[test]
    fn handoff_display_preserves_other_directories_and_a_useful_command_prefix() {
        let mut guard = SessionContextGuard::new(HudProvider::ClaudeCode);
        let cwd = format!("/repo/{}", "x".repeat(700));
        let command = format!("cd {cwd}\r\nprintf {}", "测试".repeat(170));
        guard.record_tool_result("Bash", Some(&command), true, None);
        guard.record_tool_result("Bash", Some("cd /repo/other && pwd"), true, None);
        let brief = guard.build_handoff_brief(None, Some(&cwd), InterfaceLocale::English);
        let mut steps = brief
            .markdown
            .lines()
            .filter(|line| line.starts_with("- `Bash`"));
        let first = steps.next().unwrap();
        assert!(first.starts_with("- `Bash` (printf 测试"));
        let (target, _) = first.strip_prefix("- `Bash` (").unwrap().split_once('…').unwrap();
        assert_eq!(target.chars().count(), 320);
        assert_eq!(steps.next(), Some("- `Bash` (cd /repo/other && pwd) — ok"));
    }

    #[test]
    fn hud_projection_requires_an_enabled_cache_monitor() {
        let mut guard = SessionContextGuard::new(HudProvider::ClaudeCode);
        guard.on_context_percent(Some(90.0));
        for settings in [
            AgentOptimizerSettings {
                enabled: false,
                ..Default::default()
            },
            AgentOptimizerSettings {
                cache_monitor: false,
                ..Default::default()
            },
        ] {
            assert!(guard.project_hud(2_000.0, &settings).is_none());
        }
    }

    #[test]
    fn managed_cli_instructions_respect_recommendations_and_saved_opt_out() {
        let dir = tempfile::tempdir().unwrap();
        let text = managed_cli_instructions(dir.path(), "claude-code").unwrap();
        assert!(text.contains("mt ctx outline"));

        let cfg = ContextOptimizerConfig::for_data_root(dir.path());
        cfg.set_agent_settings(
            "claude-code",
            AgentOptimizerSettings {
                cli_tools: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(managed_cli_instructions(dir.path(), "claude-code").is_none());
        assert!(managed_cli_instructions(dir.path(), "codex").is_some());
    }

    #[test]
    fn keepalive_controller_respects_opt_in_cap_subagents_and_turn_reset() {
        let mut ctrl = KeepaliveController::new();
        let expiring = CacheHudStatus {
            timing: CacheTiming::Estimate,
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
