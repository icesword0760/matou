//! Read-only projections of accepted provider observations. Run ownership, file reads
//! and publishing belong to the caller.
use std::{collections::HashSet, sync::LazyLock};

use matou_native_core::sessions::InterfaceLocale;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::provider_hook::{nonempty, one_line};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HudProvider {
    ClaudeCode,
    Codex,
    Antigravity,
    Dsh,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    #[default]
    Idle,
    Running,
    NeedsInput,
    Error,
}

impl TaskStatus {
    fn parse(value: &Value) -> Option<Self> {
        match value.as_str()? {
            "idle" => Some(Self::Idle),
            "running" => Some(Self::Running),
            "needs-input" => Some(Self::NeedsInput),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextTokens {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_creation_input: Option<f64>,
    pub cache_read_input: Option<f64>,
    pub total: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunningTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Running,
    Completed,
    Error,
}

impl ToolStatus {
    fn parse(value: &Value) -> Option<Self> {
        match value.as_str()? {
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolActivity {
    #[serde(flatten)]
    pub tool: RunningTool,
    pub status: ToolStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolCount {
    pub name: String,
    pub count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    fn parse(value: &Value) -> Self {
        match value
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "completed" | "complete" | "done" => Self::Completed,
            "in_progress" | "running" => Self::InProgress,
            _ => Self::Pending,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Error,
    Warning,
    Waiting,
    Running,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSource {
    Claude,
    External,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivity {
    pub id: String,
    pub label: String,
    pub source: AgentSource,
    pub state: AgentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub id: String,
    pub label: String,
    pub used_percent: f64,
    pub reset_text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigCounts {
    pub mcps: usize,
    pub hooks: usize,
    pub claude_md: usize,
    pub rules: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudSnapshot {
    pub session_id: String,
    pub mode: &'static str,
    pub provider: HudProvider,
    pub started_at: f64,
    pub task_status: TaskStatus,
    pub resumable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_permission_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_strategy: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_tools: Option<Vec<RunningTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_counts: Option<Vec<ToolCount>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<ToolActivity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_errors: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todos: Option<Vec<Todo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<AgentActivity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_windows: Option<Vec<UsageWindow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_usage_windows: Option<Vec<UsageWindow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_counts: Option<ConfigCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_names: Option<Vec<String>>,
}

pub struct AgentHudProjection {
    hud: HudSnapshot,
    context_tokens: Option<ContextTokens>,
    context_used_tokens: Option<u64>,
    status_context_at: Option<f64>,
    usage_resets: Vec<(String, f64)>,
    quota_model_id: Option<String>,
    provider_tool_history: ClaudeTranscript,
    provider_started_at: f64,
    active_tools: Vec<(String, RunningTool)>,
    completed_tools: Vec<(String, String)>,
    hook_counts: Vec<ToolCount>,
    claude_hook_tools: std::collections::BTreeMap<String, String>,
    live_claude: Option<ClaudeTranscript>,
    failed_mcp_servers: Vec<String>,
    optimistic_agents: Vec<(String, AgentActivity)>,
    transcript_agents: Vec<(AgentActivity, Option<f64>)>,
    external_agents: Vec<AgentActivity>,
    session_ended: bool,
    permission_observed: bool,
    pending_permission: Option<&'static str>,
    model_observed: bool,
    cycle_mode_observed: bool,
    dsh_identity: Option<String>,
}

impl AgentHudProjection {
    pub fn new(session_id: impl Into<String>, provider: HudProvider, now_ms: f64) -> Self {
        let agent_details = provider != HudProvider::Dsh;
        let tools = provider != HudProvider::Dsh;
        Self {
            hud: HudSnapshot {
                session_id: session_id.into(),
                mode: "agent",
                provider,
                started_at: now_ms,
                task_status: TaskStatus::Idle,
                resumable: false,
                cwd: None,
                permission_mode: tools.then_some("default"),
                provider_permission_label: None,
                model_strategy: (provider == HudProvider::ClaudeCode).then_some("opusplan"),
                model: None,
                effort: None,
                session_name: None,
                context_percent: None,
                context_window_size: None,
                running_tools: tools.then(Vec::new),
                tool_counts: tools.then(Vec::new),
                last_tool: None,
                mcp_errors: agent_details.then(Vec::new),
                todos: agent_details.then(Vec::new),
                agents: agent_details.then(Vec::new),
                subagent_count: agent_details.then_some(0),
                subagents: agent_details.then(Vec::new),
                usage_windows: None,
                all_usage_windows: None,
                config_counts: None,
                hook_names: None,
            },
            context_tokens: None,
            context_used_tokens: None,
            status_context_at: None,
            usage_resets: Vec::new(),
            quota_model_id: None,
            provider_tool_history: ClaudeTranscript::default(),
            provider_started_at: now_ms,
            active_tools: Vec::new(),
            completed_tools: Vec::new(),
            hook_counts: Vec::new(),
            claude_hook_tools: Default::default(),
            live_claude: None,
            failed_mcp_servers: Vec::new(),
            optimistic_agents: Vec::new(),
            transcript_agents: Vec::new(),
            external_agents: Vec::new(),
            session_ended: false,
            permission_observed: false,
            pending_permission: None,
            model_observed: false,
            cycle_mode_observed: false,
            dsh_identity: None,
        }
    }

    pub fn seed_settings(
        &mut self,
        cwd: Option<&str>,
        permission_mode: Option<&str>,
        model_strategy: Option<&str>,
        model: Option<&str>,
        resumable: bool,
    ) {
        self.hud.cwd = cwd.filter(|value| !value.is_empty()).map(str::to_owned);
        self.hud.model = model.filter(|value| !value.is_empty()).map(str::to_owned);
        self.hud.resumable = resumable;
        if self.hud.provider != HudProvider::Dsh {
            self.hud.permission_mode = permission_mode
                .and_then(normalize_permission)
                .or(Some("default"));
        }
        let strategy = model_strategy.or(model).and_then(normalize_model_strategy);
        self.hud.model_strategy = match self.hud.provider {
            HudProvider::ClaudeCode => strategy.or(Some("opusplan")),
            HudProvider::Antigravity => strategy,
            HudProvider::Codex | HudProvider::Dsh => None,
        };
    }

    pub fn update_permission(&mut self, mode: &str, pending: bool) {
        if let Some(mode) =
            normalize_permission(mode).filter(|_| self.hud.provider != HudProvider::Dsh)
        {
            self.hud.permission_mode = Some(mode);
            self.pending_permission = pending.then_some(mode);
        }
    }

    pub fn set_resumable(&mut self, resumable: bool) {
        self.hud.resumable = resumable;
    }

    pub fn set_external_agents(&mut self, agents: Vec<AgentActivity>) {
        self.external_agents = agents;
    }

    /// Last reported token figures behind the context meter, kept separate from the HUD wire.
    pub fn context_tokens(&self) -> Option<ContextTokens> {
        self.context_tokens
    }

    pub(crate) fn context_usage(&self) -> (Option<u64>, Option<u64>) {
        (
            self.context_used_tokens,
            self.hud.context_window_size.map(|size| size as u64),
        )
    }

    pub(crate) fn set_config(&mut self, counts: ConfigCounts, hook_names: Vec<String>) {
        self.hud.config_counts = Some(counts);
        self.hud.hook_names = Some(hook_names);
    }

    pub(crate) fn cwd(&self) -> Option<&str> {
        self.hud.cwd.as_deref()
    }

    pub fn snapshot(&self, now_ms: f64) -> HudSnapshot {
        self.snapshot_with_agents(now_ms, Vec::new())
    }

    pub(crate) fn snapshot_with_agents(
        &self,
        now_ms: f64,
        provider_agents: Vec<AgentActivity>,
    ) -> HudSnapshot {
        let mut hud = self.hud.clone();
        if let Some(history) = self
            .live_claude
            .as_ref()
            .filter(|_| hud.provider == HudProvider::ClaudeCode)
        {
            if history.todos_observed {
                hud.todos = Some(history.todos.clone());
            }
            let mut errors = history.mcp_errors.clone();
            for error in &self.failed_mcp_servers {
                if !errors.contains(error) {
                    errors.push(error.clone());
                }
            }
            hud.mcp_errors = Some(errors);
        }
        if hud.provider == HudProvider::Antigravity {
            let history = &self.provider_tool_history;
            let tools_active = !self.session_ended && hud.task_status != TaskStatus::Idle;
            hud.running_tools = Some(if !tools_active {
                Vec::new()
            } else {
                history
                    .active_tools
                    .iter()
                    .map(|(_, tool)| tool.clone())
                    .collect()
            });
            hud.tool_counts = Some(history.tool_counts.clone());
            hud.last_tool = tools_active
                .then(|| history.active_tools.last())
                .flatten()
                .map(|(_, tool)| ToolActivity {
                    tool: tool.clone(),
                    status: ToolStatus::Running,
                })
                .or_else(|| history.last_tool.clone());
            hud.mcp_errors = Some(history.mcp_errors.clone());
            hud.todos = Some(history.todos.clone());
        }
        for window in hud
            .usage_windows
            .iter_mut()
            .chain(hud.all_usage_windows.iter_mut())
            .flatten()
        {
            if let Some((_, reset)) = self.usage_resets.iter().find(|(id, _)| *id == window.id) {
                window.reset_text = Some(reset_text(*reset, now_ms));
            }
        }
        if hud.provider == HudProvider::ClaudeCode
            && let Some(history) = &self.live_claude
        {
            let mut tools = history.tool_counts.clone();
            for (id, name) in &self.claude_hook_tools {
                if !history.completed_tools.contains(id) {
                    increment(&mut tools, name);
                }
            }
            hud.tool_counts = Some(tools);
            if !self.session_ended {
                let mut running = history.active_tools.clone();
                for (id, tool) in &self.active_tools {
                    if !history.completed_tools.contains(id) {
                        upsert(&mut running, id.clone(), tool.clone());
                    }
                }
                running.retain(|(id, _)| !self.claude_hook_tools.contains_key(id));
                hud.running_tools = Some(running.into_iter().map(|(_, tool)| tool).collect());
            }
        }
        if hud.agents.is_some() || (!self.session_ended && !self.external_agents.is_empty()) {
            let mut agents = Vec::new();
            if !self.session_ended {
                for (agent, failed_at) in &self.transcript_agents {
                    if failed_at.is_none_or(|time| now_ms - time < 300_000.) {
                        upsert(&mut agents, agent.id.clone(), agent.clone());
                    }
                }
                for (id, agent) in &self.optimistic_agents {
                    if self.live_claude.as_ref().is_none_or(|history| {
                        !history
                            .observed_agents
                            .iter()
                            .any(|(observed, _)| observed == id)
                    }) {
                        upsert(&mut agents, id.clone(), agent.clone());
                    }
                }
                if let Some(history) = if hud.provider == HudProvider::Antigravity {
                    Some(&self.provider_tool_history)
                } else {
                    self.live_claude.as_ref()
                } {
                    for (id, (agent, failed_at)) in &history.agents {
                        if failed_at.is_none_or(|time| now_ms - time < 300_000.) {
                            upsert(&mut agents, id.clone(), agent.clone());
                        }
                    }
                }
            }
            let mut agents: Vec<_> = agents.into_iter().map(|(_, agent)| agent).collect();
            if !self.session_ended {
                agents.extend(provider_agents);
                agents.extend(self.external_agents.iter().cloned());
                agents.retain(|agent| {
                    agent.source != AgentSource::Claude
                        || agent
                            .started_at
                            .is_none_or(|time| time >= self.provider_started_at)
                });
                sort_agents(&mut agents);
            }
            hud.subagent_count = Some(agents.len());
            hud.subagents = Some(agents.iter().map(|agent| agent.label.clone()).collect());
            hud.agents = Some(agents);
        }
        hud
    }

    /// Apply one payload delivered by the provider HUD handler. DSH supplies its
    /// normalized snapshot after the provider's run/generation validation.
    pub fn ingest_provider(&mut self, payload: &Value, now_ms: f64) {
        if self.hud.provider == HudProvider::Dsh {
            self.ingest_dsh(payload);
            return;
        }

        if let Some(cwd) = nonempty(&payload["cwd"]) {
            self.hud.cwd = Some(cwd.to_owned());
        }
        let event = first_text(payload, &["hook_event_name", "hookEventName"]);
        self.observe_settings(payload, event);
        self.observe_context(payload, now_ms);
        self.observe_usage_windows(payload);
        if let Some(duration) = first_value(
            &payload["cost"],
            &["total_duration_ms", "totalDurationMs"],
            Value::as_f64,
        ) && duration > (now_ms - self.hud.started_at).max(0.)
        {
            self.hud.started_at = now_ms - duration;
        }
        let Some(event) = event else {
            if self.hud.provider == HudProvider::Antigravity {
                match nonempty(&payload["agent_state"]) {
                    Some("working") => self.hud.task_status = TaskStatus::Running,
                    Some("tool_use") => self.hud.task_status = TaskStatus::NeedsInput,
                    Some("idle") => {
                        if self.hud.task_status != TaskStatus::Error {
                            self.hud.task_status = TaskStatus::Idle;
                        }
                        self.active_tools.clear();
                    }
                    _ => {}
                }
                self.publish_running_tools();
            }
            return;
        };

        match event {
            "SessionStart" => {
                self.session_ended = false;
                if self.hud.provider == HudProvider::Codex {
                    self.hud.task_status = TaskStatus::Idle;
                }
            }
            "UserPromptSubmit" | "PreInvocation" | "PreToolUse" => {
                self.hud.task_status = TaskStatus::Running
            }
            "PostToolUse" if self.hud.provider == HudProvider::Codex => {
                self.hud.task_status = TaskStatus::Running
            }
            "PermissionRequest" => self.hud.task_status = TaskStatus::NeedsInput,
            "Notification" => {
                let has_error = ["message", "body", "text", "error"]
                    .iter()
                    .filter_map(|key| nonempty(&payload[*key]))
                    .any(|value| {
                        let value = value.to_lowercase();
                        ["error", "failed", "exception"]
                            .iter()
                            .any(|keyword| value.contains(keyword))
                    });
                self.hud.task_status = if has_error {
                    TaskStatus::Error
                } else {
                    TaskStatus::NeedsInput
                };
            }
            "Stop" => {
                let failed = self.hud.provider == HudProvider::Antigravity
                    && (nonempty(&payload["terminationReason"]) == Some("ERROR")
                        || nonempty(&payload["error"]).is_some());
                self.hud.task_status = if failed {
                    TaskStatus::Error
                } else {
                    TaskStatus::Idle
                };
                self.active_tools.clear();
            }
            "SessionEnd" => {
                self.session_ended = true;
                self.hud.task_status = TaskStatus::Idle;
                self.active_tools.clear();
                self.optimistic_agents.clear();
                self.transcript_agents.clear();
                self.provider_tool_history.active_tools.clear();
            }
            _ => {}
        }

        self.observe_hook_tool(event, payload, now_ms);
        self.publish_running_tools();
        self.hud.tool_counts = Some(if self.hud.provider == HudProvider::Codex {
            counts(self.completed_tools.iter().map(|(_, name)| name.as_str()))
        } else {
            self.hook_counts.clone()
        });
        if self.hud.mcp_errors.is_some() {
            self.hud.mcp_errors = Some(self.failed_mcp_servers.clone());
        }
    }

    fn observe_settings(&mut self, payload: &Value, event: Option<&str>) {
        let antigravity = self.hud.provider == HudProvider::Antigravity;
        let cycle_mode = first_text(payload, &["cycle_mode", "cycleMode"]);
        let cycle_tick = antigravity
            && event.is_none()
            && (payload
                .get("cycle_mode")
                .or_else(|| payload.get("cycleMode"))
                .is_some()
                || self.cycle_mode_observed
                    && ["model", "context_window", "agent_state"]
                        .iter()
                        .any(|key| payload.get(*key).is_some()));
        self.cycle_mode_observed |= cycle_tick;
        let raw_permission = if antigravity {
            cycle_mode
                .map(|mode| {
                    if mode == "auto" && self.hud.permission_mode != Some("auto") {
                        "default"
                    } else {
                        mode
                    }
                })
                .or_else(|| first_text(payload, &["permission_mode", "permissionMode"]))
        } else {
            first_text(payload, &["permission_mode", "permissionMode"])
        };
        if let Some(mode) = raw_permission
            .and_then(normalize_permission)
            .or_else(|| (cycle_tick && cycle_mode.is_none()).then_some("ask"))
            .filter(|mode| {
                self.pending_permission
                    .is_none_or(|pending| pending == *mode)
            })
        {
            self.hud.permission_mode = Some(mode);
            self.pending_permission = None;
            self.permission_observed = true;
        }

        let model = &payload["model"];
        if antigravity && !model.is_null() {
            self.quota_model_id = first_text(model, &["id", "modelId"]).map(str::to_owned);
        }
        let base_model = if self.hud.provider == HudProvider::Codex || antigravity {
            nonempty(model)
        } else {
            None
        }
        .or_else(|| first_text(model, &["display_name", "displayName", "id"]));
        if let Some(model_name) = base_model {
            if self.hud.provider == HudProvider::Codex
                && self.hud.model.as_deref() != Some(model_name)
            {
                self.hud.effort = None;
            }
            if antigravity {
                self.hud.effort = nonempty(&model["effort"]).map(str::to_owned);
            }
            if let Some(effort) = antigravity.then(|| nonempty(&model["effort"])).flatten() {
                self.hud.effort = Some(effort.to_owned());
                self.hud.model = Some(
                    if model_name.to_lowercase().contains(&effort.to_lowercase()) {
                        model_name.to_owned()
                    } else {
                        format!("{model_name} ({effort})")
                    },
                );
            } else {
                self.hud.model = Some(model_name.to_owned());
            }
            self.model_observed = true;
        }
    }

    fn observe_context(&mut self, payload: &Value, now_ms: f64) {
        if let Some(window) = first_value(payload, &["context_window", "contextWindow"], |value| {
            value.is_object().then_some(value)
        }) {
            let size = first_value(
                window,
                &["context_window_size", "contextWindowSize"],
                Value::as_f64,
            );
            if let Some(size) = size.filter(|size| *size > 0.) {
                self.hud.context_window_size = Some(size);
            }
            let usage = first_value(window, &["current_usage", "currentUsage"], |value| {
                value.is_object().then_some(value)
            });
            let tokens = usage
                .filter(|usage| sum_usage_tokens(usage).is_some())
                .map(|usage| ContextTokens {
                    input: first_value(usage, &["input_tokens", "inputTokens"], Value::as_f64),
                    output: first_value(usage, &["output_tokens", "outputTokens"], Value::as_f64),
                    cache_creation_input: first_value(
                        usage,
                        &["cache_creation_input_tokens", "cacheCreationInputTokens"],
                        Value::as_f64,
                    ),
                    cache_read_input: first_value(
                        usage,
                        &["cache_read_input_tokens", "cacheReadInputTokens"],
                        Value::as_f64,
                    ),
                    total: None,
                });
            let null_usage = [
                "used_percentage",
                "usedPercentage",
                "current_usage",
                "currentUsage",
            ]
            .iter()
            .any(|key| window.get(*key).is_some_and(Value::is_null));
            if tokens.is_some() || null_usage {
                self.context_tokens = tokens;
            }
            let reported_percent = first_value(
                window,
                &["used_percentage", "usedPercentage"],
                Value::as_f64,
            )
            .filter(|percent| *percent >= 0.);
            let used_tokens = usage.and_then(sum_usage_tokens).or_else(|| {
                let size = self.hud.context_window_size?;
                let percent = reported_percent.filter(|percent| *percent >= 0.)?;
                Some((size * percent / 100.).round() as u64)
            });
            if used_tokens.is_some() || null_usage {
                self.context_used_tokens = used_tokens;
                self.status_context_at = Some(now_ms);
            }
            let percent = reported_percent.or_else(|| {
                let size = self.hud.context_window_size?;
                let usage = tokens?;
                Some(
                    (usage.input.unwrap_or_default()
                        + usage.cache_creation_input.unwrap_or_default()
                        + usage.cache_read_input.unwrap_or_default())
                        / size
                        * 100.,
                )
            });
            if let Some(percent) = percent {
                self.hud.context_percent = Some(js_round(percent));
            } else if null_usage {
                self.hud.context_percent = None;
            }
        }
    }

    fn observe_usage_windows(&mut self, payload: &Value) {
        if self.hud.provider == HudProvider::Antigravity {
            if let Some(quota) = payload["quota"].as_object() {
                self.usage_resets.clear();
                let preferred = ["gemini-5h", "gemini-weekly", "3p-5h", "3p-weekly"];
                let keys = preferred
                    .into_iter()
                    .filter(|key| quota.contains_key(*key))
                    .chain(
                        quota
                            .keys()
                            .map(String::as_str)
                            .filter(|key| !preferred.contains(key)),
                    );
                self.hud.all_usage_windows = Some(
                    keys.filter_map(|key| {
                        let entry = &quota[key];
                        let remaining = first_value(
                            entry,
                            &["remaining_fraction", "remainingFraction"],
                            Value::as_f64,
                        )?;
                        Some(usage_window(
                            &mut self.usage_resets,
                            key,
                            &quota_label(key),
                            (1. - remaining) * 100.,
                            entry,
                        ))
                    })
                    .collect(),
                );
            }
            if let Some(windows) = &self.hud.all_usage_windows {
                let model = self.quota_model_id.as_deref().or(self.hud.model.as_deref());
                self.hud.usage_windows = Some(match_antigravity_quota(windows, model));
            }
        } else if self.hud.provider == HudProvider::ClaudeCode {
            let windows = first_value(payload, &["usageWindows", "usage_windows"], Value::as_array);
            let limits = first_value(payload, &["rateLimits", "rate_limits"], Value::as_object);
            if windows.is_none() && limits.is_none() {
                return;
            }
            self.usage_resets.clear();
            let mut projected = Vec::new();
            if let Some(windows) = windows {
                projected.extend(windows.iter().filter_map(|entry| {
                    let label = nonempty(&entry["label"])?;
                    let percent = first_value(entry, &["usedPercent", "percent"], Value::as_f64)?;
                    Some(usage_window(
                        &mut self.usage_resets,
                        nonempty(&entry["id"]).unwrap_or(label),
                        label,
                        percent,
                        entry,
                    ))
                }));
            } else {
                let limits = limits.unwrap();
                for (id, alias, label) in [
                    ("five_hour", "fiveHour", "5h"),
                    ("seven_day", "sevenDay", "Weekly"),
                ] {
                    if let Some(entry) = limits.get(id).or_else(|| limits.get(alias))
                        && let Some(percent) = first_value(
                            entry,
                            &["used_percentage", "usedPercentage"],
                            Value::as_f64,
                        )
                    {
                        projected.push(usage_window(
                            &mut self.usage_resets,
                            id,
                            label,
                            percent,
                            entry,
                        ));
                    }
                }
                for entry in limits
                    .get("model_scoped")
                    .or_else(|| limits.get("modelScoped"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some((label, percent)) =
                        first_text(entry, &["display_name", "displayName"])
                            .zip(entry["utilization"].as_f64())
                    {
                        projected.push(usage_window(
                            &mut self.usage_resets,
                            nonempty(&entry["id"]).unwrap_or(label),
                            label,
                            percent,
                            entry,
                        ));
                    }
                }
            }
            self.hud.usage_windows = Some(projected);
        }
    }

    fn observe_hook_tool(&mut self, event: &str, payload: &Value, now_ms: f64) {
        if !matches!(event, "PreToolUse" | "PostToolUse" | "PostToolUseFailure") {
            return;
        }
        let codex = self.hud.provider == HudProvider::Codex;
        if self.hud.provider == HudProvider::Antigravity {
            let call = &payload["toolCall"];
            let Some(id) = first_text(payload, &["tool_use_id", "toolUseId"])
                .or_else(|| first_text(call, &["id", "call_id"]))
            else {
                return;
            };
            let name =
                first_text(payload, &["tool_name", "toolName"]).or_else(|| nonempty(&call["name"]));
            let args = payload
                .get("tool_input")
                .or_else(|| payload.get("toolInput"))
                .unwrap_or(&call["args"]);
            if event == "PreToolUse" {
                if let Some(name) = name {
                    self.provider_tool_history.ingest_tool_use(
                        &serde_json::json!({"id": id, "name": name, "input": tool_arguments(args)}),
                        Some(now_ms),
                    );
                }
            } else {
                if let Some(name) = name {
                    self.provider_tool_history.observe_missing_tool(id, name);
                }
                let result = payload
                    .get("toolResponse")
                    .or_else(|| payload.get("toolResult"))
                    .unwrap_or(&Value::Null);
                self.provider_tool_history.ingest_tool_result(
                    &serde_json::json!({"tool_use_id": id, "is_error": event == "PostToolUseFailure" || !payload["error"].is_null(),
                        "content": payload["error"]}), result, Some(now_ms),
                );
            }
            return;
        }
        let explicit_id = first_text(payload, &["tool_use_id", "toolUseId"]);
        if codex && explicit_id.is_none() {
            return;
        }
        let call = &payload["toolCall"];
        let Some(name) =
            first_text(payload, &["tool_name", "toolName"]).or_else(|| nonempty(&call["name"]))
        else {
            return;
        };
        let id = explicit_id
            .or_else(|| nonempty(&call["id"]))
            .map_or_else(|| format!("{name}:{now_ms}"), str::to_owned);
        if event == "PreToolUse" {
            if codex && self.completed_tools.iter().any(|(key, _)| key == &id) {
                return;
            }
            let input = first_value(payload, &["tool_input", "toolInput"], |value| {
                value.is_object().then_some(value)
            })
            .unwrap_or(&call["args"]);
            if let Some(todos) = &mut self.hud.todos {
                update_todos(todos, name, input);
            }
            if !codex && name == "Agent" {
                let ordinal = self.transcript_agents.len() + self.optimistic_agents.len() + 1;
                upsert(
                    &mut self.optimistic_agents,
                    id.clone(),
                    new_agent(&id, input, ordinal, Some(now_ms)),
                );
            }
            let tool = RunningTool {
                name: name.to_owned(),
                target: target_for_tool(name, input),
            };
            upsert(&mut self.active_tools, id, tool.clone());
            self.hud.last_tool = Some(ToolActivity {
                tool,
                status: ToolStatus::Running,
            });
        } else {
            let status = if event == "PostToolUseFailure" || nonempty(&payload["error"]).is_some() {
                ToolStatus::Error
            } else {
                ToolStatus::Completed
            };
            if codex {
                upsert(&mut self.completed_tools, id.clone(), name.to_owned());
            } else if self.hud.provider == HudProvider::ClaudeCode {
                if self
                    .claude_hook_tools
                    .insert(id.clone(), name.to_owned())
                    .is_none()
                {
                    increment(&mut self.hook_counts, name);
                }
            } else {
                increment(&mut self.hook_counts, name);
            }
            let target = remove(&mut self.active_tools, &id).and_then(|tool| tool.target);
            self.hud.last_tool = Some(ToolActivity {
                tool: RunningTool {
                    name: name.to_owned(),
                    target,
                },
                status,
            });
            update_mcp_error(&mut self.failed_mcp_servers, name, status);
        }
    }

    fn publish_running_tools(&mut self) {
        self.hud.running_tools = Some(
            self.active_tools
                .iter()
                .map(|(_, tool)| tool.clone())
                .collect(),
        );
    }

    fn ingest_dsh(&mut self, payload: &Value) {
        let identity = nonempty(&payload["dsh_session_id"]);
        let changed = identity.is_some_and(|id| self.dsh_identity.as_deref() != Some(id));
        if changed {
            self.dsh_identity = identity.map(str::to_owned);
        }
        let source = &payload["dsh_hud"];
        if changed || source.is_object() {
            self.hud.model = bounded_text(&source["model"], 256);
            self.hud.effort = self
                .hud
                .model
                .as_ref()
                .and_then(|_| bounded_text(&source["effort"], 64));
            self.hud.provider_permission_label =
                bounded_text(&source["providerPermissionLabel"], 256);
            self.hud.cwd = bounded_text(&source["cwd"], 4096).or(self.hud.cwd.take());
            let context = source["contextPercent"]
                .as_f64()
                .filter(|percent| *percent >= 0.)
                .zip(
                    source["contextWindowSize"]
                        .as_f64()
                        .filter(|size| *size > 0.),
                );
            self.hud.context_percent = context.map(|(percent, _)| js_round(percent).min(100.));
            self.hud.context_window_size = context.map(|(_, size)| size);
            self.hud.running_tools = source["runningTools"].as_array().map(|tools| {
                tools
                    .iter()
                    .take(128)
                    .filter_map(|row| {
                        Some(RunningTool {
                            name: bounded_text(&row["name"], 256)?,
                            target: None,
                        })
                    })
                    .collect()
            });
            self.hud.tool_counts = source["toolCounts"].as_array().map(|tools| {
                tools
                    .iter()
                    .take(128)
                    .filter_map(|row| {
                        let count = row["count"].as_f64().filter(|count| {
                            count.fract() == 0. && (1. ..=9_007_199_254_740_991.).contains(count)
                        })?;
                        Some(ToolCount {
                            name: bounded_text(&row["name"], 256)?,
                            count: count as usize,
                        })
                    })
                    .collect()
            });
            let last = &source["lastTool"];
            self.hud.last_tool = bounded_text(&last["name"], 256)
                .zip(ToolStatus::parse(&last["status"]))
                .map(|(name, status)| ToolActivity {
                    tool: RunningTool { name, target: None },
                    status,
                });
        }
        if let Some(status) = TaskStatus::parse(&payload["dsh_work_status"]) {
            self.hud.task_status = status;
        }
    }
}

pub(crate) fn sort_agents(agents: &mut [AgentActivity]) {
    agents.sort_by(|a, b| {
        a.state
            .cmp(&b.state)
            .then_with(|| {
                a.started_at
                    .unwrap_or(f64::INFINITY)
                    .total_cmp(&b.started_at.unwrap_or(f64::INFINITY))
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn sum_usage_tokens(usage: &Value) -> Option<u64> {
    let input = first_value(usage, &["input_tokens", "inputTokens"], Value::as_u64);
    let output = first_value(usage, &["output_tokens", "outputTokens"], Value::as_u64);
    let create = first_value(
        usage,
        &["cache_creation_input_tokens", "cacheCreationInputTokens"],
        Value::as_u64,
    );
    let read = first_value(
        usage,
        &["cache_read_input_tokens", "cacheReadInputTokens"],
        Value::as_u64,
    );
    [input, output, create, read]
        .into_iter()
        .flatten()
        .reduce(u64::saturating_add)
}

fn is_compact_boundary(row: &Value) -> bool {
    row["subtype"] == "compact_boundary" || row["type"] == "compact_boundary"
}

fn transcript_context(row: &Value) -> Option<Option<u64>> {
    if row["isSidechain"] == true || row["is_sidechain"] == true {
        return None;
    }
    if is_compact_boundary(row) {
        return Some(None);
    }
    let message = &row["message"];
    if row["type"] == "assistant" || message["role"] == "assistant" {
        return sum_usage_tokens(&message["usage"]).map(Some);
    }
    None
}

fn reset_text(resets_at: f64, now_ms: f64) -> String {
    let minutes = ((resets_at - now_ms).max(0.) / 60_000.).ceil() as u64;
    match minutes {
        0 => "0m".into(),
        1..=59 => format!("{minutes}m"),
        60..=1439 => format!("{}h {}m", minutes / 60, minutes % 60),
        _ => format!("{}d {}h", minutes / 1440, minutes % 1440 / 60),
    }
}

fn usage_window(
    resets: &mut Vec<(String, f64)>,
    id: &str,
    label: &str,
    used: f64,
    entry: &Value,
) -> UsageWindow {
    if let Some(reset) = first_value(
        entry,
        &["reset_time", "resetTime", "resets_at", "resetsAt"],
        |value| {
            if let Some(number) = value.as_f64() {
                return (number > 0.).then_some(if number >= 1e12 {
                    number
                } else {
                    number * 1000.
                });
            }
            timestamp_ms(value)
        },
    ) {
        upsert(resets, id.to_owned(), reset);
    }
    UsageWindow {
        id: id.to_owned(),
        label: utf16_prefix(label, 64),
        used_percent: used.clamp(0., 100.).round(),
        reset_text: nonempty(&entry["resetText"]).map(str::to_owned),
    }
}

fn quota_label(key: &str) -> String {
    for (suffix, label) in [("-5h", "5h"), ("-weekly", "Weekly")] {
        if let Some(model) = key.strip_suffix(suffix) {
            let title = model
                .split('-')
                .filter(|part| !part.is_empty())
                .map(|part| {
                    if part == "3p" {
                        return "3P".to_owned();
                    }
                    let mut chars = part.chars();
                    chars.next().unwrap().to_uppercase().to_string() + chars.as_str()
                })
                .collect::<Vec<_>>()
                .join(" ");
            return format!("{title} {label}");
        }
    }
    key.to_owned()
}

fn match_antigravity_quota(windows: &[UsageWindow], model: Option<&str>) -> Vec<UsageWindow> {
    let Some(model) = model else {
        return windows.to_vec();
    };
    let model = model.to_ascii_lowercase();
    let words: Vec<_> = model
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .collect();
    let third_party = words.iter().any(|word| {
        [
            "claude", "opus", "fable", "sonnet", "haiku", "gpt", "o1", "o3", "o4", "chatgpt",
            "openai", "codex", "deepseek", "qwen", "llama", "mistral", "grok", "kimi", "glm",
            "minimax",
        ]
        .contains(word)
    });
    let family = if third_party {
        Some("3p")
    } else if words.contains(&"gemini") {
        Some("gemini")
    } else {
        None
    };
    let mut matched = Vec::new();
    for (suffix, label) in [("-5h", "5h"), ("-weekly", "Weekly")] {
        let dedicated = windows
            .iter()
            .filter_map(|window| {
                let token = window.id.strip_suffix(suffix)?.to_ascii_lowercase();
                if matches!(token.as_str(), "" | "gemini" | "3p") {
                    return None;
                }
                model
                    .match_indices(&token)
                    .any(|(start, _)| {
                        let end = start + token.len();
                        (start == 0 || !model.as_bytes()[start - 1].is_ascii_alphanumeric())
                            && (end == model.len()
                                || !model.as_bytes()[end].is_ascii_alphanumeric())
                    })
                    .then_some((token.len(), window))
            })
            .max_by_key(|(length, _)| *length)
            .map(|(_, window)| window);
        let selected = dedicated.or_else(|| {
            let id = format!("{}{suffix}", family?);
            windows.iter().find(|window| window.id == id)
        });
        if let Some(window) = selected {
            let mut window = window.clone();
            window.label = label.into();
            matched.push(window);
        }
    }
    if matched.is_empty() {
        windows.to_vec()
    } else {
        matched
    }
}

pub(crate) fn tool_arguments(value: &Value) -> Value {
    match value {
        Value::Object(_) => value.clone(),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_default(),
        _ => Value::Null,
    }
}

pub(crate) fn agent_tool(name: &str) -> bool {
    matches!(
        name,
        "Agent"
            | "Task"
            | "Subagent"
            | "subagent"
            | "spawn_agent"
            | "SpawnAgent"
            | "start_subagent"
            | "collab"
    )
}

fn first_value<'a, T>(
    value: &'a Value,
    keys: &[&str],
    parse: impl Fn(&'a Value) -> Option<T>,
) -> Option<T> {
    keys.iter().find_map(|key| parse(&value[*key]))
}

fn first_text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    first_value(value, keys, nonempty)
}

fn bounded_text(value: &Value, limit: usize) -> Option<String> {
    (value.as_str()?.encode_utf16().count() <= limit)
        .then(|| nonempty(value))
        .flatten()
        .map(str::to_owned)
}

fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor < 0.5 {
        floor
    } else {
        floor + 1.
    }
}

// The provider maps preserve insertion order even when an existing id changes.
fn upsert<T>(entries: &mut Vec<(String, T)>, id: String, value: T) {
    if let Some((_, current)) = entries.iter_mut().find(|(key, _)| key == &id) {
        *current = value;
    } else {
        entries.push((id, value));
    }
}

fn remove<T>(entries: &mut Vec<(String, T)>, id: &str) -> Option<T> {
    let index = entries.iter().position(|(key, _)| key == id)?;
    Some(entries.remove(index).1)
}

fn counts<'a>(names: impl Iterator<Item = &'a str>) -> Vec<ToolCount> {
    let mut counts = Vec::new();
    for name in names {
        increment(&mut counts, name);
    }
    counts
}

fn increment(counts: &mut Vec<ToolCount>, name: &str) {
    if let Some(count) = counts.iter_mut().find(|count| count.name == name) {
        count.count += 1;
    } else {
        counts.push(ToolCount {
            name: name.to_owned(),
            count: 1,
        });
    }
}

pub(crate) fn update_mcp_error(errors: &mut Vec<String>, name: &str, status: ToolStatus) {
    let Some((server, tool)) = name
        .strip_prefix("mcp__")
        .and_then(|name| name.split_once("__"))
    else {
        return;
    };
    if server.is_empty() || tool.is_empty() {
        return;
    }
    if status == ToolStatus::Error {
        if !errors.iter().any(|name| name == server) {
            errors.push(server.to_owned());
        }
    } else {
        errors.retain(|name| name != server);
    }
}

fn normalize_permission(value: &str) -> Option<&'static str> {
    let normalized: String = value
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-') && !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    match normalized.as_str() {
        "default" => Some("default"),
        "auto" => Some("auto"),
        "acceptedits" => Some("acceptEdits"),
        "plan" | "planmode" => Some("plan"),
        "ask" => Some("ask"),
        "bypasspermissions" | "bypass" => Some("bypassPermissions"),
        _ => None,
    }
}

fn normalize_model_strategy(value: &str) -> Option<&'static str> {
    let value = value.to_lowercase();
    if value.contains("opusplan") || value.contains("opus plan") {
        Some("opusplan")
    } else if value.contains("sonnet") {
        Some("claude-sonnet-4-6")
    } else if value.contains("opus") {
        Some("claude-opus-4-6")
    } else {
        None
    }
}

pub(crate) fn update_todos(todos: &mut Vec<Todo>, name: &str, input: &Value) {
    match name {
        "TodoWrite" | "update_plan" => {
            let key = if name == "TodoWrite" { "todos" } else { "plan" };
            let Some(items) = input[key].as_array() else {
                return;
            };
            *todos = items
                .iter()
                .filter_map(|item| {
                    Some(Todo {
                        content: first_text(item, &["content", "step", "subject", "description"])?
                            .to_owned(),
                        status: TodoStatus::parse(&item["status"]),
                    })
                })
                .collect();
        }
        "TaskCreate" => {
            if let Some(content) = first_text(input, &["subject", "description"]) {
                todos.push(Todo {
                    content: content.to_owned(),
                    status: TodoStatus::parse(&input["status"]),
                });
            }
        }
        "TaskUpdate" => {
            let index = first_text(input, &["taskId", "task_id"])
                .filter(|id| id.bytes().all(|byte| byte.is_ascii_digit()))
                .and_then(|id| id.parse::<usize>().ok())
                .or_else(|| {
                    ["taskId", "task_id"]
                        .iter()
                        .find_map(|key| input[*key].as_u64())
                        .and_then(|id| usize::try_from(id).ok())
                })
                .and_then(|id| id.checked_sub(1));
            if let Some(todo) = index.and_then(|index| todos.get_mut(index)) {
                if let Some(content) = first_text(input, &["subject", "description"]) {
                    todo.content = content.to_owned();
                }
                if let Some(status) = input.get("status") {
                    todo.status = TodoStatus::parse(status);
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn target_for_tool(name: &str, input: &Value) -> Option<String> {
    match name {
        "Read"
        | "Write"
        | "Edit"
        | "MultiEdit"
        | "NotebookEdit"
        | "read_file"
        | "write_file"
        | "edit_file"
        | "view_file"
        | "replace_file_content"
        | "multi_replace_file_content" => first_text(
            input,
            &["file_path", "path", "Path", "TargetFile", "AbsolutePath"],
        )
        .map(str::to_owned),
        "Glob" | "Grep" | "find_by_name" | "grep_search" => {
            first_text(input, &["pattern", "Pattern", "Query"]).map(str::to_owned)
        }
        "Bash" | "run_command" => first_text(input, &["command", "CommandLine"]).map(|command| {
            if command.encode_utf16().count() > 30 {
                format!("{}...", utf16_prefix(command, 27))
            } else {
                command.to_owned()
            }
        }),
        _ => None,
    }
}

fn utf16_prefix(value: &str, limit: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(limit).collect::<Vec<_>>())
}

pub(crate) fn new_agent(
    id: &str,
    input: &Value,
    ordinal: usize,
    started_at: Option<f64>,
) -> AgentActivity {
    AgentActivity {
        id: id.to_owned(),
        label: first_text(
            input,
            &[
                "name",
                "nickname",
                "description",
                "agent_type",
                "subagent_type",
                "subagentType",
            ],
        )
        .map_or_else(|| format!("Agent {ordinal}"), str::to_owned),
        source: AgentSource::Claude,
        state: AgentState::Running,
        started_at,
        detail: None,
    }
}

impl AgentHudProjection {
    pub(crate) fn ingest_antigravity_row(&mut self, row: Option<&Value>) {
        if let Some(row) = row {
            self.provider_tool_history.ingest(row);
            if !self.model_observed
                && let Some(model) = &self.provider_tool_history.model
            {
                self.hud.model = Some(model.clone());
                self.observe_usage_windows(&Value::Null);
            }
            if self.status_context_at.is_none() {
                self.context_used_tokens = self.provider_tool_history.context_used_tokens;
            }
        }
    }

    pub(crate) fn seed_transcript_context<'a>(
        &mut self,
        rows: impl IntoIterator<Item = &'a Value>,
    ) {
        for row in rows {
            if let Some(usage) = transcript_context(row) {
                self.context_used_tokens = usage;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn ingest_live_claude_row(&mut self, row: Option<&Value>) {
        self.ingest_live_claude_history(row);
        self.publish_transcript_context();
    }

    pub(crate) fn ingest_live_claude_history(&mut self, row: Option<&Value>) {
        if let Some(row) = row {
            if is_compact_boundary(row)
                && self.status_context_at.is_some_and(|at| {
                    timestamp_ms(&row["timestamp"]).is_some_and(|event_at| event_at >= at)
                })
            {
                self.status_context_at = None;
                self.context_used_tokens = None;
                self.context_tokens = None;
                self.hud.context_percent = None;
            }
            let history = self
                .live_claude
                .get_or_insert_with(ClaudeTranscript::default);
            history.ingest(row);
            if !self.model_observed && history.model.is_some() {
                self.hud.model = history.model.clone();
            }
        } else {
            self.live_claude = None;
        }
    }

    pub(crate) fn publish_transcript_context(&mut self) {
        if self.status_context_at.is_none() {
            self.context_used_tokens = self
                .live_claude
                .as_ref()
                .and_then(|history| history.context_used_tokens);
        }
    }

    /// Refresh from the complete sequence of decoded transcript rows for this run.
    pub fn refresh_transcript(&mut self, rows: &[Value]) {
        match self.hud.provider {
            HudProvider::ClaudeCode => self.refresh_claude_transcript(rows),
            HudProvider::Antigravity => {
                self.provider_tool_history = ClaudeTranscript::default();
                for row in rows {
                    self.ingest_antigravity_row(Some(row));
                }
            }
            HudProvider::Codex if !self.session_ended => self.refresh_codex_transcript(rows),
            HudProvider::Codex | HudProvider::Dsh => {}
        }
    }

    fn refresh_claude_transcript(&mut self, rows: &[Value]) {
        let mut history = ClaudeTranscript::default();
        for row in rows {
            history.ingest(row);
        }
        if let Some(name) = history.titles.into_iter().flatten().next() {
            self.hud.session_name = Some(utf16_prefix(&name, 120));
        }
        if let Some(started_at) = history.started_at {
            self.hud.started_at = self.hud.started_at.min(started_at);
        }
        if !self.permission_observed
            && let Some(mode) = history.permission.as_deref().and_then(normalize_permission)
        {
            self.hud.permission_mode = Some(mode);
        }
        if !self.model_observed && history.model.is_some() {
            self.hud.model = history.model;
        }
        if !self.session_ended {
            self.optimistic_agents.clear();
            self.transcript_agents = history.agents.into_iter().map(|(_, agent)| agent).collect();
        }
        self.hud.running_tools = Some(
            history
                .active_tools
                .into_iter()
                .map(|(_, tool)| tool)
                .collect(),
        );
        self.hud.tool_counts = Some(history.tool_counts);
        self.hud.last_tool = history.last_tool;
        self.hud.mcp_errors = Some(history.mcp_errors);
        self.hud.todos = Some(history.todos);
        if self.status_context_at.is_none() {
            self.context_used_tokens = history.context_used_tokens;
        }
    }

    fn refresh_codex_transcript(&mut self, rows: &[Value]) {
        let mut effort = None;
        let mut context = None;
        let mut last_tool = None;
        let mut commands = HashSet::new();
        for row in rows {
            let payload = &row["payload"];
            match (row["type"].as_str(), payload["type"].as_str()) {
                (Some("turn_context"), _) => {
                    if let Some(model) = nonempty(&payload["model"]) {
                        self.hud.model = Some(model.to_owned());
                    }
                    effort = nonempty(&payload["effort"])
                        .or_else(|| {
                            nonempty(&payload["collaboration_mode"]["settings"]["reasoning_effort"])
                        })
                        .map(str::to_owned);
                }
                (Some("compacted"), _) | (Some("event_msg"), Some("context_compacted")) => {
                    context = None;
                }
                (Some("event_msg"), Some("token_count")) => {
                    let info = &payload["info"];
                    context = info["last_token_usage"]["total_tokens"]
                        .as_f64()
                        .zip(info["model_context_window"].as_f64())
                        .filter(|(used, size)| *used >= 0. && *size > 0.);
                }
                (Some("event_msg"), Some("item_completed")) => {
                    let item = &payload["item"];
                    let Some(id) = nonempty(&item["id"]).filter(|id| {
                        item["type"] == "CommandExecution"
                            && matches!(item["status"].as_str(), Some("completed" | "failed"))
                            && commands.insert(*id)
                    }) else {
                        continue;
                    };
                    if !self
                        .completed_tools
                        .iter()
                        .any(|(command, _)| command == id)
                    {
                        self.completed_tools
                            .push((id.to_owned(), "Bash".to_owned()));
                    }
                    remove(&mut self.active_tools, id);
                    let command = &item["command"];
                    let target = command
                        .as_array()
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .or_else(|| nonempty(command).map(str::to_owned))
                        .filter(|command| !command.is_empty())
                        .map(|command| utf16_prefix(&command, 120));
                    let status = if item["status"] == "failed"
                        || item["exit_code"].as_f64().is_some_and(|code| code != 0.)
                    {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Completed
                    };
                    last_tool = Some(ToolActivity {
                        tool: RunningTool {
                            name: "Bash".to_owned(),
                            target,
                        },
                        status,
                    });
                }
                _ => {}
            }
        }
        self.hud.effort = effort;
        self.hud.context_percent = context.map(|(used, size)| js_round(100. * used / size));
        self.hud.context_window_size = context.map(|(_, size)| size);
        self.context_tokens = context.map(|(used, _)| ContextTokens {
            total: Some(used),
            ..ContextTokens::default()
        });
        self.hud.tool_counts = Some(counts(
            self.completed_tools.iter().map(|(_, name)| name.as_str()),
        ));
        self.publish_running_tools();
        if self.active_tools.is_empty() && last_tool.is_some() {
            self.hud.last_tool = last_tool;
        }
    }
}

#[derive(Default)]
struct ClaudeTranscript {
    active_tools: Vec<(String, RunningTool)>,
    completed_tools: HashSet<String>,
    tool_counts: Vec<ToolCount>,
    last_tool: Option<ToolActivity>,
    mcp_errors: Vec<String>,
    todos: Vec<Todo>,
    todos_observed: bool,
    context_used_tokens: Option<u64>,
    observed_agents: Vec<(String, AgentActivity)>,
    agents: Vec<(String, (AgentActivity, Option<f64>))>,
    agent_ids: Vec<(String, String)>,
    titles: [Option<String>; 4],
    started_at: Option<f64>,
    permission: Option<String>,
    model: Option<String>,
}

impl ClaudeTranscript {
    fn ingest(&mut self, row: &Value) {
        if !row.is_object() || row["type"] == "queue-operation" {
            return;
        }
        let timestamp =
            timestamp_ms(&row["timestamp"]).or_else(|| timestamp_ms(&row["created_at"]));
        if let Some(usage) = transcript_context(row) {
            self.context_used_tokens = usage;
        }
        if row["isSidechain"] == true || row["is_sidechain"] == true {
            return;
        }
        if row["type"] == "PLANNER_RESPONSE" || row["source"] == "MODEL" {
            if let Some(model) = nonempty(&row["model"]) {
                self.model = Some(model.to_owned());
            }
            if let Some(used) = sum_usage_tokens(&row["usage"]) {
                self.context_used_tokens = Some(used);
            }
            for tool in row["tool_calls"].as_array().into_iter().flatten() {
                let Some(id) = first_text(tool, &["id", "call_id", "tool_call_id"]) else {
                    continue;
                };
                self.ingest_tool_use(
                    &serde_json::json!({
                        "id": id, "name": tool["name"], "input": tool_arguments(&tool["arguments"]),
                    }),
                    timestamp,
                );
                if let Some(result) = tool.get("result") {
                    self.ingest_antigravity_result(id, result, timestamp);
                }
            }
        }
        if matches!(row["type"].as_str(), Some("TOOL_RESULT" | "TOOL_RESPONSE"))
            && let Some(id) = first_text(row, &["tool_call_id", "tool_use_id", "call_id"])
        {
            self.ingest_antigravity_result(id, row, timestamp);
        }
        if let Some(time) = timestamp {
            self.started_at = Some(self.started_at.unwrap_or(time).min(time));
        }
        for (index, key) in ["customTitle", "aiTitle", "slug"].into_iter().enumerate() {
            if let Some(value) = nonempty(&row[key]) {
                self.titles[index] = Some(value.to_owned());
            }
        }
        let message = &row["message"];
        if let Some(mode) =
            nonempty(&message["permissionMode"]).or_else(|| nonempty(&row["permissionMode"]))
        {
            self.permission = Some(mode.to_owned());
        }
        if let Some(model) = nonempty(&message["model"]) {
            self.model = Some(model.to_owned());
        }
        let content = &message["content"];
        if self.titles[3].is_none()
            && message["role"] == "user"
            && let Some(text) = nonempty(content).filter(|value| !value.starts_with('<'))
        {
            self.titles[3] = Some(utf16_prefix(&one_line(text), 80));
        }
        if row["type"] == "user" || message["role"] == "user" {
            self.ingest_task_notifications(&content_text(content), timestamp);
        }
        for item in content.as_array().into_iter().flatten() {
            match item["type"].as_str() {
                Some("tool_use") => self.ingest_tool_use(item, timestamp),
                Some("tool_result") => {
                    self.ingest_tool_result(item, &row["toolUseResult"], timestamp)
                }
                _ => {}
            }
        }
    }

    fn ingest_tool_use(&mut self, item: &Value, timestamp: Option<f64>) {
        let Some((id, name)) = nonempty(&item["id"]).zip(nonempty(&item["name"])) else {
            return;
        };
        if self.completed_tools.contains(id) || self.active_tools.iter().any(|(key, _)| key == id) {
            return;
        }
        let input = &item["input"];
        let tool = RunningTool {
            name: name.to_owned(),
            target: target_for_tool(name, input),
        };
        upsert(&mut self.active_tools, id.to_owned(), tool);
        if matches!(
            name,
            "TodoWrite" | "TaskCreate" | "TaskUpdate" | "update_plan"
        ) {
            self.todos_observed = true;
            update_todos(&mut self.todos, name, input);
        }

        if agent_tool(name) {
            let agent = new_agent(id, input, self.observed_agents.len() + 1, timestamp);
            upsert(&mut self.observed_agents, id.to_owned(), agent.clone());
            upsert(&mut self.agents, id.to_owned(), (agent, None));
        } else if name == "SendMessage"
            && let Some(to) = nonempty(&input["to"])
            && let Some((_, id)) = self.agent_ids.iter().find(|(key, _)| key == to)
            && let Some((_, agent)) = self.observed_agents.iter().find(|(key, _)| key == id)
        {
            let mut agent = agent.clone();
            agent.started_at = timestamp.or(agent.started_at);
            upsert(&mut self.agents, id.clone(), (agent, None));
        }
    }

    fn observe_missing_tool(&mut self, id: &str, name: &str) {
        if !self.completed_tools.contains(id) && !self.active_tools.iter().any(|(key, _)| key == id)
        {
            self.active_tools.push((
                id.to_owned(),
                RunningTool {
                    name: name.to_owned(),
                    target: None,
                },
            ));
        }
    }

    fn ingest_antigravity_result(&mut self, id: &str, result: &Value, timestamp: Option<f64>) {
        let failed = result["is_error"] == true
            || !result["error"].is_null()
            || matches!(result["status"].as_str(), Some("failed" | "error"));
        self.ingest_tool_result(
            &serde_json::json!({
                "tool_use_id": id, "is_error": failed, "content": result["error"],
            }),
            result,
            timestamp,
        );
    }

    fn ingest_tool_result(&mut self, item: &Value, result: &Value, timestamp: Option<f64>) {
        let Some(id) = first_text(item, &["tool_use_id", "toolUseId"]) else {
            return;
        };
        let Some(tool) = remove(&mut self.active_tools, id) else {
            return;
        };
        let status = if item["is_error"] == true || item["isError"] == true {
            ToolStatus::Error
        } else {
            ToolStatus::Completed
        };
        if agent_tool(&tool.name) {
            if status != ToolStatus::Error
                && (result["isAsync"] == true
                    || result["status"] == "async_launched"
                    || result["status"] == "running")
            {
                if let Some(agent_id) = first_text(result, &["agentId", "agent_id"]) {
                    upsert(&mut self.agent_ids, agent_id.to_owned(), id.to_owned());
                }
            } else {
                self.finish_agent(
                    id,
                    status == ToolStatus::Error,
                    &content_text(&item["content"]),
                    timestamp,
                );
            }
        }
        self.completed_tools.insert(id.to_owned());
        increment(&mut self.tool_counts, &tool.name);
        update_mcp_error(&mut self.mcp_errors, &tool.name, status);
        self.last_tool = Some(ToolActivity { tool, status });
    }

    fn ingest_task_notifications(&mut self, content: &str, timestamp: Option<f64>) {
        static TASKS: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?s)<task-notification>(.*?)</task-notification>").unwrap()
        });
        for task in TASKS.captures_iter(content) {
            let body = &task[1];
            let Some(id) = notification_tag(body, "tool-use-id") else {
                continue;
            };
            let failed = matches!(notification_tag(body, "status"), Some("error" | "failed"));
            self.finish_agent(
                id,
                failed,
                notification_tag(body, "summary").unwrap_or_default(),
                timestamp,
            );
        }
    }

    fn finish_agent(&mut self, id: &str, failed: bool, detail: &str, timestamp: Option<f64>) {
        if !failed {
            remove(&mut self.agents, id);
        } else if let Some(agent) = self
            .agents
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, (agent, _))| agent)
            .or_else(|| {
                self.observed_agents
                    .iter()
                    .find(|(key, _)| key == id)
                    .map(|(_, agent)| agent)
            })
        {
            let mut agent = agent.clone();
            agent.state = AgentState::Error;
            let first = detail.split('\n').next().unwrap_or_default();
            let first = if detail.contains('\n') {
                first.strip_suffix('\r').unwrap_or(first)
            } else {
                first
            };
            agent.detail = Some(utf16_prefix(first, 120));
            upsert(
                &mut self.agents,
                id.to_owned(),
                (agent, Some(timestamp.unwrap_or_default())),
            );
        }
    }
}

fn notification_tag<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let (_, after) = body.split_once(&format!("<{tag}>"))?;
    let (text, _) = after.split_once(&format!("</{tag}>"))?;
    Some(text.trim())
}

fn content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "text")
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn timestamp_ms(value: &Value) -> Option<f64> {
    if let Some(time) = value.as_f64() {
        return Some(time);
    }
    // Provider JSONL timestamps use ISO 8601. Decode without consulting the local clock or time zone.
    static ISO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"^([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]+))?(Z|[+-][0-9]{2}:[0-9]{2})$"
    ).unwrap()
    });
    let date = ISO.captures(value.as_str()?)?;
    let part = |index: usize| date.get(index)?.as_str().parse::<i64>().ok();
    let (year, month, day, hour, minute, second) =
        (part(1)?, part(2)?, part(3)?, part(4)?, part(5)?, part(6)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let fraction = date.get(7).map_or(0, |part| {
        part.as_str()
            .bytes()
            .chain(std::iter::repeat(b'0'))
            .take(3)
            .fold(0, |sum, digit| sum * 10 + i64::from(digit - b'0'))
    });
    let zone = &date[8];
    let offset_minutes = if zone == "Z" {
        0
    } else {
        let hours = zone[1..3].parse::<i64>().ok()?;
        let minutes = zone[4..6].parse::<i64>().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        (hours * 60 + minutes) * if zone.starts_with('-') { -1 } else { 1 }
    };
    // March-based Gregorian years make the leap day the last day in its year.
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let days = era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100
        + day_of_year
        - 719_468;
    Some(
        (((days * 24 + hour) * 60 + minute - offset_minutes) * 60_000 + second * 1_000 + fraction)
            as f64,
    )
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeammateObservation {
    pub run_id: String,
    pub lead_session_id: String,
    pub teammate_id: String,
    pub team_id: String,
    pub name: String,
    pub work_status: TaskStatus,
    pub latest_lines: Vec<String>,
}

impl TeammateObservation {
    fn add_line(&mut self, value: &Value) {
        if let Some(text) = nonempty(value) {
            self.add_text(text);
        }
    }

    fn add_text(&mut self, value: &str) {
        if value.is_empty() || self.latest_lines.last().is_some_and(|line| line == value) {
            return;
        }
        self.latest_lines.push(value.to_owned());
        if self.latest_lines.len() > 4 {
            self.latest_lines.remove(0);
        }
    }
}

/// Project the Claude team list from the caller's transcript tail. The result is
/// rebuilt for each tail, so replay never produces duplicate teammate cards.
pub fn project_teammates(
    run_id: &str,
    lead_session_id: &str,
    rows: &[Value],
    locale: InterfaceLocale,
) -> Vec<TeammateObservation> {
    static LISTING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^\s*(.+?)\s+\[[^\]]+\]\s+·\s+(running|idle)\b").unwrap());
    static MESSAGES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r#"(?s)<(?:agent-message\s+from|teammate-message\s+teammate_id)="([^"]+)"[^>]*>(.*?)</(?:agent-message|teammate-message)>"#
    ).unwrap()
    });

    let mut members: Vec<TeammateObservation> = Vec::new();
    for row in rows {
        let result = &row["toolUseResult"];
        if nonempty(&result["status"]) == Some("teammate_spawned")
            && let Some(id) = first_text(result, &["teammate_id", "agent_id"])
        {
            let mut id_parts = id.split('@');
            let id_name = id_parts.next().unwrap_or_default();
            let name = nonempty(&result["name"]).unwrap_or(id_name);
            if !name.is_empty() {
                let team_id = nonempty(&result["team_name"]);
                let member =
                    if let Some(index) = members.iter().position(|member| member.name == name) {
                        &mut members[index]
                    } else {
                        members.push(TeammateObservation {
                            run_id: run_id.to_owned(),
                            lead_session_id: lead_session_id.to_owned(),
                            teammate_id: id.to_owned(),
                            team_id: team_id
                                .or_else(|| id_parts.next())
                                .unwrap_or_default()
                                .to_owned(),
                            name: name.to_owned(),
                            work_status: TaskStatus::Running,
                            latest_lines: Vec::new(),
                        });
                        members.last_mut().unwrap()
                    };
                member.teammate_id = id.to_owned();
                if let Some(team_id) = team_id {
                    member.team_id = team_id.to_owned();
                }
                member.work_status = TaskStatus::Running;
                member.add_line(&result["prompt"]);
            }
        }
        if let Some(listing) = nonempty(&result["listing"]) {
            for row in listing.lines() {
                if let Some(parts) = LISTING.captures(row)
                    && let Some(member) = members
                        .iter_mut()
                        .find(|member| member.name == parts[1].trim())
                {
                    member.work_status = if parts[2].eq_ignore_ascii_case("idle") {
                        TaskStatus::Idle
                    } else {
                        TaskStatus::Running
                    };
                }
            }
        }
        let content = &row["message"]["content"];
        let block_text = content.as_array().map(|parts| {
            parts
                .iter()
                .filter_map(|part| first_text(part, &["text", "content"]))
                .collect::<Vec<_>>()
                .join("\n")
        });
        for message in [
            nonempty(&row["content"]),
            content.as_str().or(block_text.as_deref()),
        ]
        .into_iter()
        .flatten()
        {
            for parts in MESSAGES.captures_iter(message) {
                let name = parts[1].split('@').next().unwrap_or_default();
                let Some(member) = members.iter_mut().find(|member| member.name == name) else {
                    continue;
                };
                let body = parts[2].trim();
                if let Ok(event) = serde_json::from_str::<Value>(body)
                    && event["type"] == "idle_notification"
                {
                    member.work_status = TaskStatus::Idle;
                    member.add_text(nonempty(&event["result"]).unwrap_or_else(|| {
                        locale.text("队友已完成当前任务", "A teammate finished the current task")
                    }));
                } else {
                    member.add_text(body);
                }
            }
        }
    }
    members
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_live_counts_move_tools_from_running_to_completed_once() {
        let mut projection = AgentHudProjection::new("claude", HudProvider::ClaudeCode, 1.);
        projection.ingest_provider(
            &json!({"hook_event_name":"PreToolUse", "tool_use_id":"read", "tool_name":"Read"}),
            2.,
        );
        let uses = json!({"type":"assistant", "message":{"content":[
            {"type":"tool_use", "id":"read", "name":"Read"},
            {"type":"tool_use", "id":"grep", "name":"Grep"}
        ]}});
        projection.ingest_live_claude_row(Some(&uses));
        assert_eq!(projection.snapshot(2.).running_tools.unwrap().len(), 2);
        let result = json!({"type":"user", "message":{"content":[
            {"type":"tool_result", "tool_use_id":"read"}
        ]}});
        projection.ingest_live_claude_row(Some(&result));
        projection.ingest_provider(
            &json!({"hook_event_name":"PostToolUse", "tool_use_id":"grep", "tool_name":"Grep"}),
            3.,
        );
        let hud = projection.snapshot(3.);
        assert!(hud.running_tools.unwrap().is_empty());
        assert_eq!(
            json!(hud.tool_counts),
            json!([{"name":"Read","count":1},{"name":"Grep","count":1}])
        );
    }

    #[test]
    fn agent_hud_projection_context_preserves_zero_and_clears_only_explicit_null() {
        let mut projection = AgentHudProjection::new("agent-context", HudProvider::ClaudeCode, 1.);
        let mut observe = |payload| {
            projection.ingest_provider(&payload, 1.);
            let hud = projection.snapshot(1.);
            (
                hud.context_percent,
                hud.context_window_size,
                projection.context_tokens(),
            )
        };
        let size = Some(1_000_000.);
        assert_eq!(
            observe(
                json!({"context_window": {"context_window_size": size, "used_percentage": 100}})
            ),
            (Some(100.), size, None)
        );
        assert_eq!(
            observe(json!({"hook_event_name": "Stop"})),
            (Some(100.), size, None)
        );
        assert_eq!(
            observe(json!({"context_window": null})),
            (Some(100.), size, None)
        );
        assert_eq!(
            observe(json!({"contextWindow": {"contextWindowSize": size, "currentUsage": null}})),
            (None, size, None)
        );
        assert_eq!(
            observe(json!({"context_window": {"usedPercentage": 0}})),
            (Some(0.), size, None)
        );
        assert_eq!(
            observe(json!({"context_window": {"used_percentage": 108}})),
            (Some(108.), size, None)
        );
        assert_eq!(
            observe(json!({"context_window": {
                "context_window_size": size,
                "used_percentage": null,
                "current_usage": {
                    "input_tokens": 100_000,
                    "output_tokens": 9_000,
                    "cache_creation_input_tokens": 50_000,
                    "cache_read_input_tokens": 50_000
                }
            }})),
            (
                Some(20.),
                size,
                Some(ContextTokens {
                    input: Some(100_000.),
                    output: Some(9_000.),
                    cache_creation_input: Some(50_000.),
                    cache_read_input: Some(50_000.),
                    total: None,
                })
            ),
        );
        assert_eq!(
            observe(json!({"context_window": {"used_percentage": null, "current_usage": null}})),
            (None, size, None)
        );
        assert_eq!(
            observe(json!({"context_window": {"used_percentage": 0, "current_usage": null}})),
            (Some(0.), size, None)
        );
    }

    #[test]
    fn claude_context_usage_uses_last_main_reply_statusline_and_compaction() {
        let mut hud = AgentHudProjection::new("restored", HudProvider::ClaudeCode, 1.);
        let assistant = json!({"type":"assistant","message":{"usage":{
            "input_tokens":150_000,"output_tokens":500,"cache_read_input_tokens":35_000,
            "cache_creation_input_tokens":1_000
        }}});
        let sidechain =
            json!({"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":1}}});
        hud.seed_transcript_context([&assistant, &sidechain]);
        assert_eq!(hud.context_usage(), (Some(186_500), None));
        hud.ingest_provider(
            &json!({"contextWindow":{"contextWindowSize":512_000,"usedPercentage":25}}),
            10.,
        );
        assert_eq!(hud.context_usage(), (Some(128_000), Some(512_000)));
        hud.ingest_live_claude_row(Some(&json!({
            "type":"system","subtype":"compact_boundary","timestamp":9
        })));
        assert_eq!(hud.context_usage(), (Some(128_000), Some(512_000)));
        hud.ingest_live_claude_row(Some(&json!({
            "type":"system","subtype":"compact_boundary","timestamp":11
        })));
        assert_eq!(hud.context_usage(), (None, Some(512_000)));
        hud.ingest_live_claude_row(Some(&assistant));
        assert_eq!(hud.context_usage(), (Some(186_500), Some(512_000)));
    }

    #[test]
    fn agent_hud_projection_codex_counts_hook_and_transcript_command_once() {
        let mut projection = AgentHudProjection::new("cdx", HudProvider::Codex, 1.);
        for event in ["PreToolUse", "PostToolUse", "PostToolUse", "PreToolUse"] {
            projection.ingest_provider(
                &json!({
                    "hook_event_name": event, "tool_name": "Bash", "tool_use_id": "exec-1",
                    "tool_input": {"command": "echo sample"}
                }),
                2.,
            );
        }
        let command = json!({"type": "event_msg", "payload": {"type": "item_completed", "item": {
            "id": "exec-1", "type": "CommandExecution", "status": "completed", "command": ["echo", "sample"]
        }}});
        let tokens = json!({"type": "event_msg", "payload": {"type": "token_count", "info": {
            "last_token_usage": {"total_tokens": 34_175}, "total_token_usage": {"total_tokens": 9_999_999},
            "model_context_window": 258_400
        }}});
        let turn = json!({"type":"turn_context", "payload":{"model":"sample", "effort":"medium"}});
        projection.refresh_transcript(&[turn, tokens.clone(), command.clone(), command]);
        let snapshot = serde_json::to_value(projection.snapshot(2.)).unwrap();
        assert_eq!(
            snapshot,
            json!({
                "sessionId":"cdx", "mode":"agent", "provider":"codex", "startedAt":1.0,
                "taskStatus":"running", "resumable":false, "permissionMode":"default",
                "model":"sample", "effort":"medium", "contextPercent":13.0, "contextWindowSize":258_400.0,
                "runningTools":[], "toolCounts":[{"name":"Bash","count":1}],
                "lastTool":{"name":"Bash","target":"echo sample","status":"completed"},
                "mcpErrors":[], "todos":[], "agents":[], "subagents":[], "subagentCount":0
            })
        );
        assert_eq!(projection.context_tokens().unwrap().total, Some(34_175.));
        projection.refresh_transcript(&[tokens, json!({"type": "compacted"})]);
        let hud = projection.snapshot(2.);
        assert_eq!(
            (hud.context_percent, hud.context_window_size, hud.effort),
            (None, None, None)
        );
        assert_eq!(projection.context_tokens(), None);
    }

    #[test]
    fn agent_hud_projection_dsh_snapshot_replaces_the_previous_observations() {
        let mut projection = AgentHudProjection::new("s", HudProvider::Dsh, 1.);
        projection.ingest_provider(
            &json!({"dsh_session_id":"a", "dsh_work_status":"running", "dsh_hud":{
                "model":"Sample", "effort":"medium", "providerPermissionLabel":"Ask",
                "cwd":"/sample", "contextPercent":110, "contextWindowSize":1000,
                "runningTools":[{"name":"Read","target":"ignore"}, {"name":" "}],
                "toolCounts":[{"name":"Read","count":2},{"name":"Bash","count":-1}],
                "lastTool":{"name":"Read","status":"running"}
            }}),
            2.,
        );
        projection.ingest_provider(
            &json!({"dsh_session_id":"a", "dsh_work_status":"needs-input"}),
            2.,
        );
        assert_eq!(
            serde_json::to_value(projection.snapshot(2.)).unwrap(),
            json!({
                "sessionId":"s", "mode":"agent", "provider":"dsh", "startedAt":1.0,
                "taskStatus":"needs-input", "resumable":false, "cwd":"/sample",
                "model":"Sample", "effort":"medium", "providerPermissionLabel":"Ask",
                "contextPercent":100.0, "contextWindowSize":1000.0,
                "runningTools":[{"name":"Read"}], "toolCounts":[{"name":"Read","count":2}],
                "lastTool":{"name":"Read","status":"running"}
            })
        );
        projection.ingest_provider(
            &json!({"dsh_session_id":"a", "dsh_hud":{"model":"New"}}),
            2.,
        );
        let snapshot = projection.snapshot(2.);
        assert_eq!(snapshot.model.as_deref(), Some("New"));
        assert_eq!(snapshot.context_percent, None);
        assert!(snapshot.running_tools.is_none());
        projection.ingest_provider(&json!({"dsh_session_id":"b", "dsh_work_status":"idle"}), 2.);
        assert_eq!(
            serde_json::to_value(projection.snapshot(2.)).unwrap(),
            json!({
                "sessionId":"s", "mode":"agent", "provider":"dsh", "startedAt":1.0,
                "taskStatus":"idle", "resumable":false, "cwd":"/sample"
            })
        );
    }
}
