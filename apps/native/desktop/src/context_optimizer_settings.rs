//! Standalone GPUI panel and presentation helpers for per-agent context &
//! KV-cache optimizer settings (`<data_root>/context-optimizer.json`).

use std::{collections::BTreeMap, rc::Rc};

use gpui::{prelude::*, *};
use matou_native_core::{
    context_optimizer_config::{
        AgentOptimizerEntry, AgentOptimizerSettings, ContextOptimizerConfig,
        ContextOptimizerSnapshot, KNOWN_AGENT_IDS, OptimizerAvailability,
    },
    sessions::InterfaceLocale,
};
use serde_json::Value;

use crate::{
    theme_tokens::{AppTheme, ThemePalette},
    workspace::CardTooltip,
};

pub use matou_native_core::context_optimizer_config::OptimizerToggle;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentSavings {
    pub bytes7d: u64,
    pub events7d: u64,
}

impl AgentSavings {
    fn labels(self, locale: InterfaceLocale) -> (String, Option<String>) {
        if self.bytes7d == 0 {
            return (
                locale
                    .text(
                        "还没有节省数据，正常使用后会显示",
                        "Savings will appear here after regular use.",
                    )
                    .into(),
                None,
            );
        }
        let tokens = self.bytes7d / 4;
        let kilobytes = self.bytes7d as f64 / 1024.;
        let kilobytes = format!("{kilobytes:.3}");
        let kilobytes = kilobytes.trim_end_matches('0').trim_end_matches('.');
        match locale {
            InterfaceLocale::Chinese => (
                format!("过去 7 天约少读 {tokens} token"),
                Some(format!("约 {kilobytes} KB")),
            ),
            InterfaceLocale::English => (
                format!("About {tokens} fewer tokens read in the past 7 days"),
                Some(format!("About {kilobytes} KB")),
            ),
        }
    }
}

/// Pure, testable state controller for the per-agent optimizer settings view.
pub struct ContextOptimizerPanelState {
    config: Rc<ContextOptimizerConfig>,
    snapshot: ContextOptimizerSnapshot,
    selected_agent: String,
    savings: BTreeMap<&'static str, AgentSavings>,
    locale: InterfaceLocale,
    status_message: Option<String>,
    error_message: Option<String>,
}

impl ContextOptimizerPanelState {
    pub fn load(
        config: Rc<ContextOptimizerConfig>,
        locale: InterfaceLocale,
    ) -> Result<Self, String> {
        let snapshot = config.snapshot().map_err(|err| err.to_string())?;
        Ok(Self {
            config,
            snapshot,
            selected_agent: KNOWN_AGENT_IDS[0].to_owned(),
            savings: BTreeMap::new(),
            locale,
            status_message: None,
            error_message: None,
        })
    }

    pub fn set_locale(&mut self, locale: InterfaceLocale) {
        self.locale = locale;
    }

    pub fn selected_agent(&self) -> &str {
        &self.selected_agent
    }

    pub fn select_agent(&mut self, agent_id: &str) {
        if KNOWN_AGENT_IDS.contains(&agent_id) {
            self.selected_agent = agent_id.to_owned();
            self.status_message = None;
            self.error_message = None;
        }
    }

    fn set_savings(&mut self, result: &Value) {
        self.savings = KNOWN_AGENT_IDS
            .into_iter()
            .map(|id| {
                let agent = &result["agents"][id];
                (
                    id,
                    AgentSavings {
                        bytes7d: agent["bytes7d"].as_u64().unwrap_or_default(),
                        events7d: agent["events7d"].as_u64().unwrap_or_default(),
                    },
                )
            })
            .collect();
    }

    pub fn current_savings(&self) -> AgentSavings {
        self.savings
            .get(self.selected_agent())
            .copied()
            .unwrap_or_default()
    }

    pub fn current_entry(&self) -> Option<&AgentOptimizerEntry> {
        self.snapshot.agents.get(&self.selected_agent)
    }

    pub fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    /// Toggles one switch for the currently selected agent and persists
    /// immediately to disk. Returns `true` when persisted.
    pub fn toggle(&mut self, field: OptimizerToggle) -> bool {
        let Some(entry) = self.snapshot.agents.get(&self.selected_agent) else {
            return false;
        };
        if !can_toggle(field, entry) {
            return false;
        }
        let mut next = entry.settings.clone();
        match field {
            OptimizerToggle::MasterEnabled if !next.enabled => {
                next = entry.restored_recommendations();
            }
            OptimizerToggle::MasterEnabled => next.enabled = false,
            OptimizerToggle::NoiseFilter => next.noise_filter = !next.noise_filter,
            OptimizerToggle::ReadDedup => next.read_dedup = !next.read_dedup,
            OptimizerToggle::CacheMonitor => next.cache_monitor = !next.cache_monitor,
            OptimizerToggle::CacheExpiryNotify => {
                next.cache_expiry_notify = !next.cache_expiry_notify
            }
            OptimizerToggle::CacheKeepalive => next.cache_keepalive = !next.cache_keepalive,
            OptimizerToggle::StallRescue => next.stall_rescue = !next.stall_rescue,
            OptimizerToggle::CliTools => next.cli_tools = !next.cli_tools,
            OptimizerToggle::BlockSensitiveReads => {
                next.block_sensitive_reads = !next.block_sensitive_reads
            }
        }
        self.save_current(next)
    }

    pub fn restore_defaults(&mut self) -> bool {
        let Some(entry) = self.current_entry() else {
            return false;
        };
        self.save_current(entry.restored_recommendations())
    }

    fn save_current(&mut self, next: AgentOptimizerSettings) -> bool {
        match self.config.set_agent_settings(&self.selected_agent, next) {
            Ok(updated) => {
                self.snapshot = updated;
                self.error_message = None;
                self.status_message = Some(self.locale.text("已保存", "Saved").to_owned());
                true
            }
            Err(err) => {
                self.status_message = None;
                self.error_message = Some(err.to_string());
                false
            }
        }
    }
}

fn can_toggle(field: OptimizerToggle, entry: &AgentOptimizerEntry) -> bool {
    entry.availability(field) == OptimizerAvailability::Available
        && (field == OptimizerToggle::MasterEnabled || entry.settings.enabled)
        && (field != OptimizerToggle::CacheExpiryNotify || entry.settings.cache_monitor)
}

fn unavailable_reason(
    field: OptimizerToggle,
    entry: &AgentOptimizerEntry,
    locale: InterfaceLocale,
) -> Option<&'static str> {
    use OptimizerAvailability::{Available, Pending, Unsupported};
    use OptimizerToggle::{
        BlockSensitiveReads, CacheExpiryNotify, CacheKeepalive, NoiseFilter, ReadDedup,
    };
    match (entry.availability(field), field, entry.agent_id.as_str()) {
        (Available, _, _) if field != OptimizerToggle::MasterEnabled && !entry.settings.enabled => {
            Some(locale.text("自动优化已关闭。", "Automatic optimization is off."))
        }
        (Available, CacheExpiryNotify, _) if !entry.settings.cache_monitor => Some(locale.text(
            "先开启「快满了提前告诉你」，才能收到缓存到期提醒。",
            "Turn on context alerts above to receive cache expiry reminders.",
        )),
        (Available, _, _) => None,
        (Pending, CacheKeepalive, _) => Some(locale.text(
            "暂无可靠的静默续命方式：自动发送会进入对话并消耗额度。等能确认送达后再开放。",
            "There is no reliable way to refresh the cache silently yet. An automatic message would appear in the conversation and use quota. This will be available once silent delivery can be verified.",
        )),
        (Pending, NoiseFilter, "codex") => Some(locale.text(
            "Codex 当前版本不支持在结果交给 AI 前替换输出，自动省日志暂不可用；已记录，待适配。",
            "Pending integration. This Codex version cannot replace tool output before it reaches the model, so automatic trimming is unavailable.",
        )),
        (Pending, NoiseFilter | ReadDedup | BlockSensitiveReads, "antigravity") => Some(locale.text(
            "协议适配已完成，待本机登录 Antigravity 完成端到端验证后开放。",
            "Protocol integration is complete. This will be available after end-to-end testing in a locally signed-in Antigravity session.",
        )),
        (Pending, BlockSensitiveReads, "codex") => Some(locale.text(
            "Codex 读文件走 Bash，.env 保护暂未覆盖，待适配。",
            "Codex reads files through Bash. Protection for .env needs an integration for that route.",
        )),
        (Pending, _, "codex") => Some(locale.text(
            "Codex 通过 Bash 读文件，当前防重复读没有覆盖这条路径，待适配。",
            "Codex reads files through Bash. Skipping duplicate reads needs an integration for that route.",
        )),
        (Pending, _, _) => Some(locale.text(
            "待适配或验证，确认可用后开放。",
            "Pending integration or validation. This will open once verified.",
        )),
        (Unsupported, OptimizerToggle::NoiseFilter | OptimizerToggle::ReadDedup | OptimizerToggle::BlockSensitiveReads, _) => Some(locale.text(
            "DSH 仅支持观察，不支持自动拦截。",
            "This agent supports observation only. Automatic interception is unsupported.",
        )),
        (Unsupported, _, _) => Some(locale.text(
            "该 Agent 不支持此功能。",
            "This agent does not support this feature.",
        )),
    }
}

pub fn agent_display_name(agent_id: &str) -> &str {
    match agent_id {
        "claude-code" => "Claude Code",
        "antigravity" => "Antigravity",
        "codex" => "Codex",
        "dsh" => "DSH",
        other => other,
    }
}

/// Compact HUD text for KV-cache remaining lifetime.
pub fn format_cache_countdown(remaining_sec: u64, warm: bool, locale: InterfaceLocale) -> String {
    if !warm || remaining_sec == 0 {
        return locale.text("缓存已冷", "Cache cold").to_owned();
    }
    let mins = remaining_sec / 60;
    let secs = remaining_sec % 60;
    if mins > 0 {
        match locale {
            InterfaceLocale::Chinese => format!("缓存 {mins}m {secs:02}s"),
            InterfaceLocale::English => format!("Cache {mins}m {secs:02}s"),
        }
    } else {
        match locale {
            InterfaceLocale::Chinese => format!("缓存 {secs}s"),
            InterfaceLocale::English => format!("Cache {secs}s"),
        }
    }
}

/// Compact HUD text for blocked noise volume.
pub fn format_saved_noise_bytes(bytes: u64, locale: InterfaceLocale) -> String {
    let amount = if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    };
    match locale {
        InterfaceLocale::Chinese => format!("已省 {amount}"),
        InterfaceLocale::English => format!("Saved {amount}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudPillTone {
    Neutral,
    Accent,
    Warning,
    Danger,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudPill {
    pub label: String,
    pub tone: HudPillTone,
}

/// Pure presentation model parsed from `live["contextGuard"]` for rendering
/// cache countdown, saved noise, stall badge, and one-click rescue handoff in
/// the workspace HUD bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextGuardHudView {
    pub cache_pill: Option<HudPill>,
    pub saved_pill: Option<HudPill>,
    pub stall_pill: Option<HudPill>,
    pub show_handoff_button: bool,
    pub handoff_urgent: bool,
}

impl ContextGuardHudView {
    pub fn from_live_hud(live: &Value, locale: InterfaceLocale) -> Option<Self> {
        let guard = live.get("contextGuard")?.as_object()?;
        let cache_pill = guard.get("cache").and_then(Value::as_object).map(|cache| {
            let remaining = cache
                .get("remainingSec")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let warm = cache.get("warm").and_then(Value::as_bool).unwrap_or(false);
            let expiring_soon = cache
                .get("expiringSoon")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tone = if !warm || remaining == 0 {
                HudPillTone::Neutral
            } else if expiring_soon {
                HudPillTone::Warning
            } else {
                HudPillTone::Accent
            };
            HudPill {
                label: format_cache_countdown(remaining, warm, locale),
                tone,
            }
        });

        let saved_bytes = guard
            .get("savedNoiseBytes")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let saved_pill = (saved_bytes > 0).then(|| HudPill {
            label: format_saved_noise_bytes(saved_bytes, locale),
            tone: HudPillTone::Neutral,
        });

        let stall_obj = guard.get("stall").and_then(Value::as_object);
        let stall_severity = stall_obj
            .and_then(|s| s.get("severity"))
            .and_then(Value::as_str)
            .unwrap_or("normal");
        let silent_sec = stall_obj
            .and_then(|s| s.get("silentSec"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let stall_pill = match stall_severity {
            "stalled" => Some(HudPill {
                label: locale.text("疑似卡死", "Stalled").to_owned(),
                tone: HudPillTone::Danger,
            }),
            "warning" => Some(HudPill {
                label: match locale {
                    InterfaceLocale::Chinese => format!("静默 {}m", (silent_sec / 60).max(1)),
                    InterfaceLocale::English => format!("Silent {}m", (silent_sec / 60).max(1)),
                },
                tone: HudPillTone::Warning,
            }),
            _ => None,
        };

        let pressure = guard
            .get("contextPressure")
            .and_then(Value::as_str)
            .unwrap_or("normal");
        let handoff_ready = guard
            .get("handoffReady")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let handoff_urgent = stall_severity == "stalled" || pressure == "critical";
        let show_handoff_button = handoff_ready || handoff_urgent || pressure == "warning";

        if cache_pill.is_none()
            && saved_pill.is_none()
            && stall_pill.is_none()
            && !show_handoff_button
        {
            return None;
        }
        Some(Self {
            cache_pill,
            saved_pill,
            stall_pill,
            show_handoff_button,
            handoff_urgent,
        })
    }
}

const ADVANCED_ITEMS: [OptimizerToggle; 8] = {
    use OptimizerToggle::*;
    [
        CacheMonitor,
        CacheExpiryNotify,
        StallRescue,
        NoiseFilter,
        ReadDedup,
        BlockSensitiveReads,
        CliTools,
        CacheKeepalive,
    ]
};
const AGENT_TABS: [&str; 4] = ["antigravity", "claude-code", "codex", "dsh"];

struct AdvancedCopy {
    title: &'static str,
    description: &'static str,
    recommendation: &'static str,
    benefit: &'static str,
    cost: &'static str,
    when_off: &'static str,
}

fn advanced_copy(field: OptimizerToggle, locale: InterfaceLocale) -> AdvancedCopy {
    use OptimizerToggle::*;
    let t = |zh, en| locale.text(zh, en);
    match field {
        CacheMonitor => AdvancedCopy {
            title: t("快满了提前告诉你", "Warn me when context is nearly full"),
            description: t(
                "底栏显示 AI 还记得多少、已读内容还能复用多久，接近上限时提醒你。只读状态，不改变 AI 的行为。",
                "See context usage and remaining cache time in the bottom bar. Get an alert near the limit. This only reads session status.",
            ),
            recommendation: t("推荐开启", "Recommended on"),
            benefit: t(
                "知道什么时候该压缩或换会话",
                "Know when to compact or start a new session",
            ),
            cost: t("没有代价", "None"),
            when_off: t(
                "不想被打扰可以关",
                "Turn it off if you prefer fewer alerts.",
            ),
        },
        CacheExpiryNotify => AdvancedCopy {
            title: t("缓存快过期时提醒", "Remind me before the cache expires"),
            description: t(
                "已读内容快不能复用时提醒你，不会向 AI 发送请求。",
                "Get an alert when the AI is about to lose the ability to reuse what it has read. This sends no request to the AI.",
            ),
            recommendation: t("默认关闭", "Off by default"),
            benefit: t(
                "想继续时可以赶在缓存过期前回来",
                "Return in time to reuse the cache",
            ),
            cost: t("多一条提醒", "An extra alert"),
            when_off: t(
                "不想被缓存提醒打扰时保持关闭",
                "Leave it off if you prefer fewer alerts.",
            ),
        },
        StallRescue => AdvancedCopy {
            title: t("卡住了提醒你", "Tell me when a session is stuck"),
            description: t(
                "后台会话长时间没有任何动静时提醒，并准备一份脱敏进度摘要，点一下就能带着摘要开新会话。",
                "Flag long-running sessions that go quiet and prepare a summary with sensitive details removed. Start a new session with that summary in one click.",
            ),
            recommendation: t("推荐开启", "Recommended on"),
            benefit: t(
                "不用切回去才发现白等半小时",
                "Spot a stalled session while working elsewhere",
            ),
            cost: t("极少数情况会多一条提醒", "An occasional extra alert"),
            when_off: t(
                "长构建、慢模型已做过防误报，一般不用关",
                "Usually safe to leave on. Long builds and slow models are accounted for.",
            ),
        },
        NoiseFilter => AdvancedCopy {
            title: t("长日志自动压缩", "Summarize long logs automatically"),
            description: t(
                "跑测试、构建时，AI 只读报错和摘要，不被几千行日志淹没。完整原文留在本地，随时能查回。",
                "Give the AI errors and a summary from long test and build logs. The full log stays available locally.",
            ),
            recommendation: t("经常跑测试的人推荐开", "Recommended for frequent testing"),
            benefit: t(
                "少占上下文，回复更快、更省额度",
                "Less context to read, faster replies, less quota used",
            ),
            cost: t(
                "AI 看不到全文，需要时你点开原文",
                "Open the original when you need the full log",
            ),
            when_off: t(
                "需要 AI 逐行看完整原始输出时先关掉",
                "Turn it off when the AI needs to inspect the entire raw output.",
            ),
        },
        ReadDedup => AdvancedCopy {
            title: t(
                "别让 AI 反复读同一个文件",
                "Skip repeat reads of unchanged files",
            ),
            description: t(
                "同一个文件没改过，第二次读会被提醒复用已有内容；120 秒内再试一次会自动放行。",
                "When the AI rereads an unchanged file, ask it to reuse what it already has. A retry within 120 seconds goes through.",
            ),
            recommendation: t("推荐开启", "Recommended on"),
            benefit: t(
                "省额度，也减少重复思考",
                "Use less quota and avoid repeated work",
            ),
            cost: t("偶尔会被拦一下，重试即可", "A repeat read can take a retry"),
            when_off: t(
                "正在逐行对照同一文件时可临时关",
                "Turn it off temporarily for line-by-line comparisons of the same file.",
            ),
        },
        BlockSensitiveReads => AdvancedCopy {
            title: t("拦住直接读 .env", "Block direct reads of .env"),
            description: t(
                "AI 想直接读 .env 里的密钥时会被拦下；.env.example 这类样板文件照常放行。",
                "Block direct reads of secrets in .env files. Templates such as .env.example are still readable.",
            ),
            recommendation: t("推荐开启", "Recommended on"),
            benefit: t("避免密钥被带进对话", "Keep secrets out of the conversation"),
            cost: t(
                "读取真实配置时需要先关闭",
                "Accessing real configuration requires turning it off",
            ),
            when_off: t(
                "你明确要让 AI 读真实配置时再关",
                "Turn it off only when you want the AI to read the real configuration.",
            ),
        },
        CliTools => AdvancedCopy {
            title: t("更省 token 的查找工具", "Search with fewer tokens"),
            description: t(
                "引导 AI 查看大文件大纲、批量搜索和查找被截断的日志。对新启动的会话生效。",
                "Guides the AI to outline large files, run batch searches, and find trimmed logs. Applies to newly started sessions.",
            ),
            recommendation: t("重度使用推荐开", "Recommended for heavy use"),
            benefit: t(
                "大文件和大日志处理更快",
                "Process large files and logs faster",
            ),
            cost: t(
                "会在新会话启动提示里加 4 行固定说明",
                "Adds four fixed lines of guidance to the startup prompt for new sessions",
            ),
            when_off: t(
                "只做简短问答时可以关",
                "Leave it off if you only ask brief questions.",
            ),
        },
        CacheKeepalive => AdvancedCopy {
            title: t("自动续命", "Keep the cache warm automatically"),
            description: t(
                "缓存快过期前自动发一次极短请求，让缓存别冷掉。",
                "Send a short request before the cache expires to keep it warm.",
            ),
            recommendation: t("暂不可用", "Unavailable"),
            benefit: t(
                "离开几分钟回来不用重新计算",
                "Reuse cached work after stepping away",
            ),
            cost: t(
                "自动发送会进入对话并消耗额度",
                "An automatic message appears in the chat and uses quota",
            ),
            when_off: t(
                "目前没有可靠的静默发送方式，先只做到期提醒",
                "Use the expiry reminder until silent refresh is available.",
            ),
        },
        MasterEnabled => unreachable!("the master switch is outside advanced settings"),
    }
}

/// GPUI settings panel rendered inside `SettingsPage::ContextOptimizer`.
pub struct ContextOptimizerSettingsPanel {
    state: ContextOptimizerPanelState,
    palette: ThemePalette,
    dark: bool,
    advanced_open: bool,
    agent_focus: [FocusHandle; AGENT_TABS.len()],
    toggle_focus: [FocusHandle; OptimizerToggle::BlockSensitiveReads as usize + 1],
    advanced_focus: FocusHandle,
    restore_focus: FocusHandle,
    last_focus: Option<FocusHandle>,
    scroll: ScrollHandle,
}

impl ContextOptimizerSettingsPanel {
    pub fn new(
        config: Rc<ContextOptimizerConfig>,
        locale: InterfaceLocale,
        theme: AppTheme,
        scroll: ScrollHandle,
        cx: &App,
    ) -> Result<Self, String> {
        let state = ContextOptimizerPanelState::load(config, locale)?;
        Ok(Self {
            state,
            palette: *theme.palette(),
            dark: theme == AppTheme::Dark,
            advanced_open: false,
            agent_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            toggle_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            advanced_focus: cx.focus_handle().tab_stop(true),
            restore_focus: cx.focus_handle().tab_stop(true),
            last_focus: None,
            scroll,
        })
    }

    pub fn set_locale(&mut self, locale: InterfaceLocale, cx: &mut Context<Self>) {
        self.state.set_locale(locale);
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        self.palette = *theme.palette();
        self.dark = theme == AppTheme::Dark;
        cx.notify();
    }

    pub fn set_savings(&mut self, result: &Value, cx: &mut Context<Self>) {
        self.state.set_savings(result);
        cx.notify();
    }

    fn switch(
        &self,
        field: OptimizerToggle,
        entry: &AgentOptimizerEntry,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let interactive = can_toggle(field, entry);
        let checked = interactive && field.checked(&entry.settings);
        let track_color = if checked {
            self.palette.workspace.accent
        } else if self.dark {
            0x3a4048ff
        } else {
            0xcfd4dcff
        };
        let switch = div()
            .id(("ctx-opt-toggle", field as usize))
            .w(px(44.))
            .h(px(26.))
            .mt(px(2.))
            .flex_none()
            .rounded_full()
            .p(px(3.))
            .flex()
            .items_center()
            .bg(rgba(track_color))
            .child(
                div()
                    .size(px(20.))
                    .rounded_full()
                    .ml(if checked { px(18.) } else { px(0.) })
                    .bg(rgb(0xffffff)),
            );
        if interactive {
            let accent = rgba(self.palette.workspace.accent);
            switch
                .track_focus(&self.toggle_focus[field as usize])
                .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.toggle(field);
                    cx.notify();
                }))
        } else {
            switch.opacity(0.55)
        }
    }

    fn label(&self, copy: impl Into<SharedString>) -> Div {
        div()
            .text_size(px(12.))
            .line_height(px(17.))
            .text_color(rgba(self.palette.workspace.muted))
            .child(copy.into())
    }

    fn pill(&self, copy: impl Into<SharedString>, color: u32, background: u32) -> Div {
        div()
            .max_w_full()
            .px(px(7.))
            .py(px(1.))
            .rounded(px(7.))
            .text_size(px(10.5))
            .line_height(px(15.))
            .bg(rgba(background))
            .text_color(rgba(color))
            .child(copy.into())
    }

    fn render_advanced_row(
        &self,
        field: OptimizerToggle,
        entry: &AgentOptimizerEntry,
        reveal_focus: Option<&FocusHandle>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let locale = self.state.locale;
        let copy = advanced_copy(field, locale);
        let focus = &self.toggle_focus[field as usize];
        let reason = unavailable_reason(field, entry, locale);
        let palette = self.palette.workspace;
        let (good, good_bg, warning, warning_bg) = if self.dark {
            (0x4cc596ff, 0x16332aff, 0xe0a34dff, 0x332715ff)
        } else {
            (0x187351ff, 0xe7f5eeff, 0x8c4d00ff, 0xfff4e2ff)
        };
        let recommendation = if entry.availability(field) == OptimizerAvailability::Available {
            copy.recommendation
        } else {
            locale.text("暂不可用", "Unavailable")
        };
        let recommendation_color = if reason.is_some() {
            (palette.muted, palette.panel_muted)
        } else if field == OptimizerToggle::CacheExpiryNotify {
            (palette.text, palette.panel_muted)
        } else {
            (good, good_bg)
        };
        let mut text = div()
            .w_full()
            .min_w_0()
            .pr(px(60.))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight(650.))
                    .text_color(rgba(if reason.is_some() {
                        palette.muted
                    } else {
                        palette.text
                    }))
                    .child(copy.title),
            )
            .child(self.label(copy.description).mt(px(4.)))
            .child(
                div()
                    .mt(px(6.))
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(self.pill(
                        recommendation,
                        recommendation_color.0,
                        recommendation_color.1,
                    ))
                    .child(self.pill(
                        format!("{} {}", locale.text("好处", "Benefit"), copy.benefit),
                        palette.text,
                        palette.panel_muted,
                    ))
                    .child(self.pill(
                        format!("{} {}", locale.text("代价", "Trade-off"), copy.cost),
                        if reason.is_some() {
                            palette.muted
                        } else {
                            warning
                        },
                        if reason.is_some() {
                            palette.panel_muted
                        } else {
                            warning_bg
                        },
                    )),
            )
            .child(
                self.label(format!(
                    "{} {}",
                    locale.text("什么时候别开", "When to leave it off"),
                    copy.when_off
                ))
                .mt(px(4.)),
            );
        if let Some(reason) = reason {
            let accent = rgba(palette.accent);
            text = text.child(
                self.label(reason)
                    .id(("ctx-opt-reason", field as usize))
                    .track_focus(focus)
                    .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                    .mt(px(4.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_color(rgba(warning))
                    .bg(rgba(warning_bg)),
            );
        }
        let mut row = reveal_on_focus(div(), &self.scroll, reveal_focus == Some(focus))
            .id(("ctx-opt-row", field as usize))
            .relative()
            .w_full()
            .py(px(12.))
            .pl(if field == OptimizerToggle::CacheExpiryNotify {
                px(18.)
            } else {
                px(0.)
            })
            .border_t_1()
            .border_color(rgba(palette.line))
            .child(text)
            .child(
                div()
                    .absolute()
                    .top(px(12.))
                    .right_0()
                    .child(self.switch(field, entry, cx)),
            );
        if let Some(reason) = reason {
            row = row.tooltip(move |_, cx| cx.new(|_| CardTooltip(reason.into())).into());
        }
        row
    }

    fn render_summary(&self, entry: &AgentOptimizerEntry) -> Div {
        let locale = self.state.locale;
        let palette = self.palette.workspace;
        let name = agent_display_name(&entry.agent_id);
        let (label, note) = if entry.settings.enabled {
            let label = if entry.agent_id == "dsh" {
                locale.text("观察模式已开启", "Observation mode is on")
            } else {
                locale.text("自动优化已开启", "Automatic optimization is on")
            };
            let defaults = AgentOptimizerSettings::recommended_for(&entry.agent_id);
            let count = ADVANCED_ITEMS
                .into_iter()
                .filter(|&field| can_toggle(field, entry) && field.checked(&entry.settings))
                .count();
            let recommended = ADVANCED_ITEMS.into_iter().all(|field| {
                entry.availability(field) != OptimizerAvailability::Available
                    || field.checked(&entry.settings) == field.checked(&defaults)
            });
            let note = match (locale, recommended) {
                (InterfaceLocale::Chinese, true) => {
                    format!("已按 {name} 当前可用的能力自动配置 {count} 项")
                }
                (InterfaceLocale::Chinese, false) => format!("{name} 当前已启用 {count} 项"),
                (InterfaceLocale::English, true) => {
                    format!("{count} options configured automatically for {name}")
                }
                (InterfaceLocale::English, false) => format!("{count} options enabled for {name}"),
            };
            (label, note)
        } else {
            (
                locale.text("已关闭", "Off"),
                locale
                    .text("优化和提醒已暂停", "Optimization and alerts are paused")
                    .to_owned(),
            )
        };
        let status_dot = if entry.settings.enabled {
            if self.dark { 0x4cc596ff } else { 0x18805bff }
        } else {
            palette.muted
        };
        let (primary, secondary) = self.state.current_savings().labels(locale);
        let savings = match secondary {
            Some(secondary) => div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight(700.))
                        .child(primary),
                )
                .child(self.label(secondary)),
            None => self.label(primary),
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(14.))
            .w_full()
            .p(px(16.))
            .rounded(px(12.))
            .border_1()
            .border_color(rgba(palette.line))
            .bg(rgba(palette.panel_muted))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_grow()
                    .flex_basis(px(205.))
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(14.))
                            .font_weight(FontWeight(650.))
                            .child(
                                div()
                                    .size(px(8.))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(rgba(status_dot)),
                            )
                            .child(label),
                    )
                    .child(self.label(note)),
            )
            .child(
                savings
                    .flex_grow()
                    .flex_basis(px(180.))
                    .min_w_0()
                    .text_right(),
            )
    }
}

fn outline_shadow(color: Rgba) -> BoxShadow {
    BoxShadow {
        color: color.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(2.),
    }
}

fn reveal_on_focus(element: Div, scroll: &ScrollHandle, newly_focused: bool) -> Div {
    element.when(newly_focused, |element| {
        let scroll = scroll.clone();
        element.on_children_prepainted(move |children, window, cx| {
            let Some(child) = children.first() else {
                return;
            };
            let viewport = scroll.bounds();
            let inset = px(4.);
            let delta = if child.top() < viewport.top() + inset {
                viewport.top() + inset - child.top()
            } else if child.bottom() > viewport.bottom() - inset {
                viewport.bottom() - inset - child.bottom()
            } else {
                return;
            };
            let focus = window.focused(cx);
            let scroll = scroll.clone();
            window.on_next_frame(move |window, cx| {
                if window.focused(cx) != focus {
                    return;
                }
                let mut offset = scroll.offset();
                offset.y += delta;
                scroll.set_offset(offset);
                window.refresh();
            });
        })
    })
}

impl Render for ContextOptimizerSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.state.locale;
        let palette = self.palette.workspace;
        let accent = rgba(palette.accent);
        let previous_focus = std::mem::replace(&mut self.last_focus, window.focused(cx));
        let reveal_focus = self
            .last_focus
            .as_ref()
            .filter(|focus| Some(*focus) != previous_focus.as_ref());

        let mut tabs = div().flex().flex_wrap().gap(px(8.));
        for (id, focus) in AGENT_TABS.into_iter().zip(&self.agent_focus) {
            let active = id == self.state.selected_agent();
            tabs = tabs.child(
                reveal_on_focus(div(), &self.scroll, reveal_focus == Some(focus))
                    .id(SharedString::from(format!("ctx-opt-agent-{id}")))
                    .track_focus(focus)
                    .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                    .px(px(16.))
                    .py(px(7.))
                    .rounded(px(10.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .font_weight(FontWeight(600.))
                    .bg(rgba(if active {
                        palette.accent_soft
                    } else {
                        palette.panel
                    }))
                    .border_1()
                    .border_color(rgba(if active { palette.accent } else { palette.line }))
                    .text_color(rgba(if active { palette.accent } else { palette.text }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.select_agent(id);
                        cx.notify();
                    }))
                    .child(agent_display_name(id)),
            );
        }

        let Some(entry) = self.state.current_entry() else {
            return div()
                .child(locale.text("未找到 Agent 配置", "Agent configuration unavailable"));
        };
        let master_focus = &self.toggle_focus[OptimizerToggle::MasterEnabled as usize];
        let master = reveal_on_focus(div(), &self.scroll, reveal_focus == Some(master_focus))
            .flex()
            .items_start()
            .gap(px(16.))
            .pt(px(12.))
            .border_t_1()
            .border_color(rgba(palette.line))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div().text_size(px(15.)).font_weight(FontWeight(700.)).child(
                            locale.text("自动优化（推荐）", "Automatic optimization (recommended)"),
                        ),
                    )
                    .child(self.label(locale.text(
                        "按当前 Agent 实际能用的能力自动配置。关闭后完全恢复原样，不拦截、不提醒、不显示额外状态。",
                        "Uses the features supported by this agent. Turn it off to stop filtering, alerts, and additional status indicators.",
                    ))),
            )
            .child(self.switch(OptimizerToggle::MasterEnabled, entry, cx));

        let advanced = reveal_on_focus(
            div(),
            &self.scroll,
            reveal_focus == Some(&self.advanced_focus),
        )
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .pt(px(8.))
        .border_t_1()
        .border_color(rgba(palette.line))
        .child(
            div()
                .id("ctx-opt-advanced")
                .track_focus(&self.advanced_focus)
                .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(4.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(13.))
                .font_weight(FontWeight(650.))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.advanced_open = !this.advanced_open;
                    cx.notify();
                }))
                .child(if self.advanced_open { "▾" } else { "▸" })
                .child(locale.text("高级设置", "Advanced settings")),
        )
        .child(self.label(locale.text(
            "不确定就保持默认，下面这些不用管",
            "The defaults are fine for most people.",
        )));

        let footer = reveal_on_focus(
            div(),
            &self.scroll,
            reveal_focus == Some(&self.restore_focus),
        )
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .px(px(12.))
        .py(px(10.))
        .rounded(px(8.))
        .bg(rgba(palette.panel_muted))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(self.label(locale.text(
                    "改动立即生效，不用重启会话",
                    "Changes take effect immediately, without restarting the session.",
                )))
                .when_some(self.state.error_message(), |text, error| {
                    text.child(self.label(error.to_owned()).text_color(rgb(if self.dark {
                        0xff9b94
                    } else {
                        0xb34d47
                    })))
                })
                .when_some(self.state.status_message(), |text, status| {
                    text.child(self.label(status.to_owned()))
                }),
        )
        .child(
            div()
                .id("ctx-opt-restore-defaults")
                .track_focus(&self.restore_focus)
                .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                .px(px(11.))
                .py(px(6.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgba(palette.line))
                .bg(rgba(palette.panel))
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.restore_defaults();
                    cx.notify();
                }))
                .child(locale.text("恢复推荐配置", "Restore recommended settings")),
        );

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(14.))
            .mt(px(16.))
            .text_color(rgba(palette.text))
            .child(tabs)
            .child(self.render_summary(entry))
            .child(master)
            .child(advanced)
            .when(self.advanced_open, |element| {
                element.child(
                    div().children(
                        ADVANCED_ITEMS
                            .into_iter()
                            .map(|field| self.render_advanced_row(field, entry, reveal_focus, cx)),
                    ),
                )
            })
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ADVANCED_ITEMS, AgentOptimizerSettings, AgentSavings, ContextGuardHudView,
        ContextOptimizerConfig, ContextOptimizerPanelState, HudPillTone, InterfaceLocale,
        OptimizerAvailability, OptimizerToggle, advanced_copy, can_toggle, format_cache_countdown,
        format_saved_noise_bytes, unavailable_reason,
    };
    use std::rc::Rc;

    #[test]
    fn panel_enforces_matrix_and_preserves_unavailable_preferences_across_reload() {
        use OptimizerAvailability::{Available, Pending, Unsupported};
        use OptimizerToggle::*;

        let dir = std::env::temp_dir().join(format!("matou-ctx-opt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Rc::new(ContextOptimizerConfig::for_data_root(&dir));
        for id in ["claude-code", "codex", "antigravity", "dsh"] {
            config
                .set_agent_settings(
                    id,
                    AgentOptimizerSettings {
                        noise_filter: true,
                        read_dedup: true,
                        cache_keepalive: true,
                        cli_tools: id == "claude-code",
                        cache_keepalive_max_per_turn: 3,
                        max_log_bytes: 32_768,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let mut state =
            ContextOptimizerPanelState::load(Rc::clone(&config), InterfaceLocale::Chinese).unwrap();
        assert_eq!(state.selected_agent(), "claude-code");
        for (id, interception) in [
            ("claude-code", Available),
            ("codex", Pending),
            ("antigravity", Pending),
            ("dsh", Unsupported),
        ] {
            state.select_agent(id);
            let entry = state.current_entry().unwrap().clone();
            assert!(entry.settings.block_sensitive_reads, "{id}");
            assert!(!entry.settings.cache_expiry_notify, "{id}");
            for (field, availability) in [
                (NoiseFilter, interception),
                (ReadDedup, interception),
                (BlockSensitiveReads, interception),
                (CacheKeepalive, Pending),
                (CacheExpiryNotify, Available),
                (CacheMonitor, Available),
                (StallRescue, Available),
                (CliTools, Available),
            ] {
                assert_eq!(entry.availability(field), availability, "{id} {field:?}");
                let available = availability == Available;
                assert_eq!(can_toggle(field, &entry), available, "{id} {field:?}");
                assert_eq!(state.toggle(field), available, "{id} {field:?}");
                for locale in [InterfaceLocale::Chinese, InterfaceLocale::English] {
                    let reason = unavailable_reason(field, &entry, locale);
                    assert_eq!(reason.is_none(), available, "{id} {field:?} {locale:?}");
                }
                if id == "antigravity"
                    && matches!(field, NoiseFilter | ReadDedup | BlockSensitiveReads)
                {
                    assert_eq!(
                        unavailable_reason(field, &entry, InterfaceLocale::Chinese),
                        Some("协议适配已完成，待本机登录 Antigravity 完成端到端验证后开放。")
                    );
                }
            }
            let settings = &state.current_entry().unwrap().settings;
            assert!(
                settings.cache_keepalive && settings.cache_expiry_notify,
                "{id}"
            );
            assert!(!settings.cache_monitor, "{id}");
            assert!(!settings.stall_rescue, "{id}");
            assert_eq!(settings.cli_tools, id != "claude-code");
            assert_eq!(settings.noise_filter, interception != Available, "{id}");
            assert_eq!(settings.read_dedup, interception != Available, "{id}");
            assert_eq!(
                settings.block_sensitive_reads,
                interception != Available,
                "{id}"
            );
        }

        // Loading a fresh panel from disk simulates reopening after restart.
        let new_store = Rc::new(ContextOptimizerConfig::for_data_root(&dir));
        let mut reopened =
            ContextOptimizerPanelState::load(new_store, InterfaceLocale::English).unwrap();
        assert!(
            !reopened
                .current_entry()
                .unwrap()
                .settings
                .block_sensitive_reads
        );
        for id in ["codex", "antigravity", "dsh"] {
            reopened.select_agent(id);
            let saved = &reopened.current_entry().unwrap().settings;
            assert!(
                saved.noise_filter && saved.read_dedup && saved.block_sensitive_reads,
                "{id}"
            );
            assert!(saved.cache_keepalive && saved.cache_expiry_notify, "{id}");
        }
        reopened.select_agent("codex");
        assert!(reopened.restore_defaults());
        let saved = &config.snapshot().unwrap().agents["codex"].settings;
        assert!(saved.noise_filter && saved.read_dedup && saved.block_sensitive_reads);
        assert!(saved.cache_keepalive && !saved.cache_expiry_notify);
        assert!(saved.cli_tools);
        assert_eq!(saved.cache_keepalive_max_per_turn, 3);
        assert_eq!(saved.max_log_bytes, 32_768);

        let copy = advanced_copy(CliTools, InterfaceLocale::Chinese);
        assert_eq!(copy.recommendation, "重度使用推荐开");
        assert_eq!(copy.benefit, "大文件和大日志处理更快");
        assert_eq!(copy.cost, "会在新会话启动提示里加 4 行固定说明");
        assert_eq!(copy.when_off, "只做简短问答时可以关");
        assert!(copy.description.contains("对新启动的会话生效"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn master_switch_preserves_preferences_when_off_and_recommends_when_enabled() {
        use OptimizerToggle::{CacheExpiryNotify, CacheMonitor, MasterEnabled, NoiseFilter};

        let dir = std::env::temp_dir().join(format!("matou-ctx-master-{}", uuid::Uuid::new_v4()));
        let config = Rc::new(ContextOptimizerConfig::for_data_root(&dir));
        let mut panel =
            ContextOptimizerPanelState::load(config.clone(), InterfaceLocale::Chinese).unwrap();
        assert!(panel.toggle(NoiseFilter));
        assert!(panel.toggle(CacheExpiryNotify));
        assert!(panel.toggle(CacheMonitor));
        assert!(!panel.toggle(CacheExpiryNotify));
        let customized = panel.current_entry().unwrap().settings.clone();
        assert!(!customized.noise_filter && !customized.cache_monitor);
        assert!(customized.cache_expiry_notify);

        assert!(panel.toggle(MasterEnabled));
        let disabled = AgentOptimizerSettings {
            enabled: false,
            ..customized
        };
        assert_eq!(
            config.snapshot().unwrap().agents["claude-code"].settings,
            disabled
        );
        for field in ADVANCED_ITEMS {
            assert!(!panel.toggle(field));
        }

        let mut reopened =
            ContextOptimizerPanelState::load(config.clone(), InterfaceLocale::Chinese).unwrap();
        assert_eq!(reopened.current_entry().unwrap().settings, disabled);
        assert!(reopened.toggle(MasterEnabled));
        assert_eq!(
            config.snapshot().unwrap().agents["claude-code"].settings,
            AgentOptimizerSettings::recommended_for("claude-code")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn savings_are_per_agent_and_only_display_reported_bytes() {
        let config = Rc::new(ContextOptimizerConfig::for_data_root(
            std::env::temp_dir().join(format!("matou-ctx-stats-{}", uuid::Uuid::new_v4())),
        ));
        let mut panel = ContextOptimizerPanelState::load(config, InterfaceLocale::Chinese).unwrap();
        panel.set_savings(&serde_json::json!({
            "agents": {
                "claude-code": {"bytes7d": 5123, "events7d": 2},
                "antigravity": {"bytes7d": 0, "events7d": 0},
                "codex": {"bytes7d": 1024, "events7d": 1},
                "dsh": {"bytes7d": "invalid"}
            }
        }));
        assert_eq!(
            panel.current_savings(),
            AgentSavings {
                bytes7d: 5123,
                events7d: 2
            }
        );
        assert_eq!(
            panel.current_savings().labels(InterfaceLocale::Chinese),
            (
                "过去 7 天约少读 1280 token".into(),
                Some("约 5.003 KB".into())
            )
        );
        panel.select_agent("codex");
        assert_eq!(
            panel.current_savings().labels(InterfaceLocale::English),
            (
                "About 256 fewer tokens read in the past 7 days".into(),
                Some("About 1 KB".into())
            )
        );
        for id in ["antigravity", "dsh"] {
            panel.select_agent(id);
            assert_eq!(
                panel.current_savings().labels(InterfaceLocale::Chinese),
                ("还没有节省数据，正常使用后会显示".into(), None)
            );
        }
        panel.select_agent("claude-code");
        panel.set_savings(&serde_json::Value::Null);
        assert_eq!(panel.current_savings(), AgentSavings::default());
    }

    #[test]
    fn hud_formatting_helpers_handle_locales_and_units() {
        assert_eq!(
            format_cache_countdown(185, true, InterfaceLocale::Chinese),
            "缓存 3m 05s"
        );
        assert_eq!(
            format_cache_countdown(0, false, InterfaceLocale::English),
            "Cache cold"
        );
        assert_eq!(
            format_saved_noise_bytes(150 * 1024, InterfaceLocale::Chinese),
            "已省 150 KB"
        );
        assert_eq!(
            format_saved_noise_bytes(2_097_152, InterfaceLocale::English),
            "Saved 2.0 MB"
        );
    }

    #[test]
    fn hud_view_parses_cache_saved_noise_stall_and_handoff_readiness() {
        let live = serde_json::json!({
            "contextGuard": {
                "contextPressure": "critical",
                "savedNoiseBytes": 98304,
                "handoffReady": true,
                "cache": {
                    "ttlSec": 300,
                    "remainingSec": 45,
                    "warm": true,
                    "expiringSoon": true
                },
                "stall": {
                    "severity": "stalled",
                    "silentSec": 210
                }
            }
        });
        let view = ContextGuardHudView::from_live_hud(&live, InterfaceLocale::Chinese).unwrap();
        assert_eq!(view.cache_pill.as_ref().unwrap().label, "缓存 45s");
        assert_eq!(view.cache_pill.as_ref().unwrap().tone, HudPillTone::Warning);
        assert_eq!(view.saved_pill.as_ref().unwrap().label, "已省 96 KB");
        assert_eq!(view.stall_pill.as_ref().unwrap().label, "疑似卡死");
        assert_eq!(view.stall_pill.as_ref().unwrap().tone, HudPillTone::Danger);
        assert!(view.show_handoff_button);
        assert!(view.handoff_urgent);

        // Missing contextGuard returns None cleanly.
        assert!(
            ContextGuardHudView::from_live_hud(&serde_json::json!({}), InterfaceLocale::Chinese)
                .is_none()
        );
    }
}
