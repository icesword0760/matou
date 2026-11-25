//! Built-in context & KV-cache optimizer engine (Phase 1: observation,
//! KV-cache countdown, sleep-safe stall watchdog, and redacted handoff briefs).
//!
//! All state stays in-process inside `matou-native-runtime` with zero MCP
//! servers and zero extra background processes. Behavior is gated per-agent by
//! `matou_native_core::context_optimizer_config::AgentOptimizerSettings`.

use std::collections::VecDeque;

pub mod cli;
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

use crate::agent_hud_projection::{ContextUsageSample, HudProvider, TaskStatus};

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
    usage: Option<ContextUsageSample>,
    warned_pressure: ContextPressure,
    warned_cache_deadline_ms: Option<f64>,
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
        Self {
            provider,
            usage: None,
            warned_pressure: ContextPressure::Normal,
            warned_cache_deadline_ms: None,
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
        active_tool: Option<(&str, Option<&str>)>,
        now_ms: f64,
    ) {
        if status != self.task_status {
            self.on_task_status(status, now_ms);
        }
        if context_percent != self.context_percent {
            self.on_context_percent(context_percent, now_ms);
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

    pub(crate) fn context_percent(&self) -> Option<f64> {
        self.context_percent
    }

    pub fn on_context_percent(&mut self, percent: Option<f64>, now_ms: f64) {
        self.context_percent = percent;
        if percent.is_some() {
            self.last_activity_ms = Some(now_ms);
        }
    }

    pub(crate) fn observe_usage(&mut self, usage: Option<ContextUsageSample>) {
        if let Some(at) = usage.and_then(|sample| sample.observed_at_ms)
            && self.last_activity_ms.is_none_or(|last| at > last)
        {
            self.last_activity_ms = Some(at);
        }
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

        let cache = settings
            .cache_monitor
            .then(|| self.compute_cache_status(now_ms));
        let stall = settings
            .stall_rescue
            .then(|| self.compute_stall_status(now_ms));
        let context_pressure = if settings.cache_monitor {
            classify_context_pressure(self.context_percent)
        } else {
            ContextPressure::Normal
        };
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
                true,
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
        guard.on_context_percent(Some(74.99), at);
        assert_eq!(guard.pending_reminders(due - 1., &settings).count(), 0);
        guard.on_context_percent(Some(75.), at);
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
        guard.on_context_percent(Some(85.), at);
        let critical = ContextReminder::Pressure(ContextPressure::Critical);
        assert_eq!(
            guard.pending_reminders(due, &settings).collect::<Vec<_>>(),
            [critical]
        );
        guard.mark_reminded(critical);
        guard.on_context_percent(Some(80.), at);
        assert_eq!(guard.pending_reminders(due, &settings).count(), 0);

        guard.on_compaction();
        assert_eq!(guard.pending_reminders(due, &settings).count(), 0);
        guard.on_context_percent(Some(85.), due);
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
