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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudPill {
    pub label: String,
    pub tone: HudPillTone,
}

/// Pure presentation model parsed from `live["contextGuard"]` for rendering
/// saved noise and handoff actions in the workspace HUD bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextGuardHudView {
    pub saved_pill: Option<HudPill>,
    pub show_handoff_button: bool,
    pub handoff_urgent: bool,
}

impl ContextGuardHudView {
    pub fn from_live_hud(live: &Value, locale: InterfaceLocale) -> Option<Self> {
        let guard = live.get("contextGuard")?.as_object()?;
        let saved_bytes = guard
            .get("savedNoiseBytes")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let saved_pill = (saved_bytes > 0).then(|| HudPill {
            label: format_saved_noise_bytes(saved_bytes, locale),
            tone: HudPillTone::Neutral,
        });

        let pressure = guard
            .get("contextPressure")
            .and_then(Value::as_str)
            .unwrap_or("normal");
        let handoff_urgent = pressure == "critical";
        let show_handoff_button = handoff_urgent || pressure == "warning";

        if saved_pill.is_none() && !show_handoff_button {
            return None;
        }
        Some(Self {
            saved_pill,
            show_handoff_button,
            handoff_urgent,
        })
    }
}

const ADVANCED_ITEMS: [OptimizerToggle; 7] = {
    use OptimizerToggle::*;
    [
        CacheMonitor,
        CacheExpiryNotify,
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
}

fn advanced_copy(field: OptimizerToggle, locale: InterfaceLocale) -> AdvancedCopy {
    use OptimizerToggle::*;
    let t = |zh, en| locale.text(zh, en);
    match field {
        CacheMonitor => AdvancedCopy {
            title: t("快满了提前告诉你", "Warn when context is nearly full"),
            description: t(
                "底栏显示上下文占用与缓存剩余时间，接近上限时提醒。只读状态，不改变 AI 行为。",
                "Shows context usage and remaining cache time in the footer, alerting near the limit. Read-only.",
            ),
        },
        CacheExpiryNotify => AdvancedCopy {
            title: t("缓存快过期时提醒", "Alert before cache expires"),
            description: t(
                "已读缓存即将失效前发一条提醒，方便及时继续对话；不会向 AI 发送请求。",
                "Notifies shortly before cached context expires so you can resume in time. Sends no request to the model.",
            ),
        },
        NoiseFilter => AdvancedCopy {
            title: t("长日志自动压缩", "Compress verbose logs"),
            description: t(
                "跑测试、构建时只把报错和摘要交给 AI，完整原文留本地随时可查；需要 AI 逐行看全量输出时可临时关掉。",
                "Passes errors and summaries to the model while keeping full logs locally. Turn off temporarily when the model needs every raw line.",
            ),
        },
        ReadDedup => AdvancedCopy {
            title: t("别让 AI 反复读同一个文件", "Skip duplicate file reads"),
            description: t(
                "文件未改动时二次读取会提醒复用已有内容，120 秒内再试自动放行。",
                "Prompts the model to reuse unchanged files on repeat reads, and automatically allows retries within 120 seconds.",
            ),
        },
        BlockSensitiveReads => AdvancedCopy {
            title: t("拦住直接读 .env", "Block direct .env reads"),
            description: t(
                "阻止 AI 直接读取 .env 里的密钥，.env.example 等样板文件照常放行；确需读取真实配置时可临时关掉。",
                "Blocks direct reads of secret .env files while allowing samples like .env.example. Turn off temporarily when real config access is needed.",
            ),
        },
        CliTools => AdvancedCopy {
            title: t("更省 token 的查找工具", "Token-saving lookup tools"),
            description: t(
                "在新会话启动提示里加入 4 行指引，让 AI 优先用大纲、批量检索和日志回查工具处理大文件。",
                "Adds 4 lines to new session prompts guiding the model to use outline, batch search, and log lookup tools on large files.",
            ),
        },
        CacheKeepalive => AdvancedCopy {
            title: t("自动续命", "Auto keep-alive"),
            description: t(
                "缓存快过期前尝试静默保活，减少缓存变冷后的重复计费。",
                "Attempts to keep prompt cache warm before expiry to reduce cold-cache re-billing.",
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

    #[cfg(debug_assertions)]
    pub fn probe_focus(&self, agent: Option<&str>, window: &mut Window) {
        let focus = match agent {
            Some(agent) => {
                &self.agent_focus[AGENT_TABS.iter().position(|&id| id == agent).unwrap()]
            }
            None => &self.advanced_focus,
        };
        window.focus(focus);
    }

    #[cfg(debug_assertions)]
    pub fn probe_agent_focused_with_advanced_open(&self, agent: &str, window: &Window) -> bool {
        let index = AGENT_TABS.iter().position(|&id| id == agent).unwrap();
        self.advanced_open
            && self.state.selected_agent() == agent
            && self.agent_focus[index].is_focused(window)
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
            let focus_ring = rgba(self.palette.workspace.accent_line);
            switch
                .track_focus(&self.toggle_focus[field as usize])
                .focus(|style| style.shadow(vec![outline_shadow(focus_ring)]))
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
        let badge = if reason.is_some() {
            Some(self.pill(
                locale.text("暂不可用", "Unavailable"),
                palette.muted,
                palette.panel_muted,
            ))
        } else if field.checked(&AgentOptimizerSettings::recommended_for(&entry.agent_id)) {
            Some(self.pill(locale.text("推荐", "Recommended"), good, good_bg))
        } else {
            None
        };
        let mut text = div()
            .w_full()
            .min_w_0()
            .pr(px(60.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
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
                    .children(badge),
            )
            .child(self.label(copy.description).mt(px(4.)));
        if let Some(reason) = reason {
            let focus_ring = rgba(palette.accent_line);
            text = text.child(
                self.label(reason)
                    .id(("ctx-opt-reason", field as usize))
                    .track_focus(focus)
                    .focus(|style| style.shadow(vec![outline_shadow(focus_ring)]))
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
            .pl(px(0.))
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
        let focus_ring = rgba(palette.accent_line);
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
                    .focus(|style| style.shadow(vec![outline_shadow(focus_ring)]))
                    .px(px(16.))
                    .py(px(7.))
                    .rounded(px(10.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .font_weight(FontWeight(600.))
                    .bg(rgba(if !active {
                        palette.panel
                    } else if self.dark {
                        0x1b2e47ff
                    } else {
                        0xe8f1fcff
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
                .border_1()
                .border_color(transparent_black())
                .focus(|style| style.border_color(focus_ring))
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
                .focus(|style| style.shadow(vec![outline_shadow(focus_ring)]))
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
        ContextOptimizerConfig, ContextOptimizerPanelState, InterfaceLocale,
        OptimizerAvailability, OptimizerToggle, advanced_copy, can_toggle,
        format_saved_noise_bytes, unavailable_reason,
    };
    use std::rc::Rc;

    #[test]
    fn cache_expiry_notification_row_starts_at_the_same_left_edge() {
        use super::ContextOptimizerSettingsPanel;
        use crate::theme_tokens::AppTheme;
        use OptimizerToggle::{CacheExpiryNotify, CacheMonitor};
        use gpui::{AppContext, ScrollHandle, Styled, TestAppContext, px};

        let root = std::env::temp_dir().join(format!("matou-ctx-opt-row-{}", uuid::Uuid::new_v4()));
        let config = Rc::new(ContextOptimizerConfig::for_data_root(&root));
        let mut cx = TestAppContext::single();
        cx.update(|cx| {
            cx.new(|cx| {
                ContextOptimizerSettingsPanel::new(
                    config,
                    InterfaceLocale::Chinese,
                    AppTheme::Light,
                    ScrollHandle::new(),
                    cx,
                )
                .unwrap()
            })
            .update(cx, |panel, cx| {
                let entry = panel.state.current_entry().unwrap();
                for field in [CacheMonitor, CacheExpiryNotify] {
                    let mut row = panel.render_advanced_row(field, entry, None, cx);
                    assert_eq!(row.style().padding.left, Some(px(0.).into()), "{field:?}");
                }
            });
        });
    }

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
            ("antigravity", Available),
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
            }
            let settings = &state.current_entry().unwrap().settings;
            assert!(
                settings.cache_keepalive && settings.cache_expiry_notify,
                "{id}"
            );
            assert!(!settings.cache_monitor, "{id}");
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
        for id in ["codex", "dsh"] {
            reopened.select_agent(id);
            let saved = &reopened.current_entry().unwrap().settings;
            assert!(
                saved.noise_filter && saved.read_dedup && saved.block_sensitive_reads,
                "{id}"
            );
            assert!(saved.cache_keepalive && saved.cache_expiry_notify, "{id}");
        }
        reopened.select_agent("antigravity");
        let agy_saved = &reopened.current_entry().unwrap().settings;
        assert!(
            !agy_saved.noise_filter
                && !agy_saved.read_dedup
                && !agy_saved.block_sensitive_reads
        );
        assert!(agy_saved.cache_keepalive && agy_saved.cache_expiry_notify);
        reopened.select_agent("codex");
        assert!(reopened.restore_defaults());
        let saved = &config.snapshot().unwrap().agents["codex"].settings;
        assert!(saved.noise_filter && saved.read_dedup && saved.block_sensitive_reads);
        assert!(saved.cache_keepalive && !saved.cache_expiry_notify);
        assert!(saved.cli_tools);
        assert_eq!(saved.cache_keepalive_max_per_turn, 3);
        assert_eq!(saved.max_log_bytes, 32_768);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn advanced_copy_keeps_new_session_scope_in_both_languages() {
        let zh = advanced_copy(OptimizerToggle::CliTools, InterfaceLocale::Chinese);
        let en = advanced_copy(OptimizerToggle::CliTools, InterfaceLocale::English);
        assert!(zh.description.contains("新会话启动提示里加入 4 行指引"));
        assert!(
            en.description
                .contains("Adds 4 lines to new session prompts")
        );
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
            format_saved_noise_bytes(150 * 1024, InterfaceLocale::Chinese),
            "已省 150 KB"
        );
        assert_eq!(
            format_saved_noise_bytes(2_097_152, InterfaceLocale::English),
            "Saved 2.0 MB"
        );
    }

    #[test]
    fn hud_handoff_visibility_and_urgency_follow_context_pressure() {
        for (pressure, ready, visible, urgent) in [
            ("normal", true, false, false),
            ("warning", false, true, false),
            ("critical", false, true, true),
        ] {
            let live = serde_json::json!({
                "contextGuard": {
                    "contextPressure": pressure,
                    "handoffReady": ready,
                    "savedNoiseBytes": 2048
                }
            });
            let view = ContextGuardHudView::from_live_hud(&live, InterfaceLocale::Chinese).unwrap();
            assert_eq!(view.show_handoff_button, visible, "{pressure}");
            assert_eq!(view.handoff_urgent, urgent, "{pressure}");
        }
    }

    #[test]
    fn hud_view_renders_saved_noise_and_critical_handoff() {
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
                }
            }
        });
        let view = ContextGuardHudView::from_live_hud(&live, InterfaceLocale::Chinese).unwrap();
        assert_eq!(view.saved_pill.as_ref().unwrap().label, "已省 96 KB");
        assert!(view.show_handoff_button);
        assert!(view.handoff_urgent);

        // Missing contextGuard returns None cleanly.
        assert!(
            ContextGuardHudView::from_live_hud(&serde_json::json!({}), InterfaceLocale::Chinese)
                .is_none()
        );
    }
}
