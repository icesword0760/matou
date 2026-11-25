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

/// Pure, testable state controller for the per-agent optimizer settings view.
pub struct ContextOptimizerPanelState {
    config: Rc<ContextOptimizerConfig>,
    snapshot: ContextOptimizerSnapshot,
    selected_agent: String,
    locale: InterfaceLocale,
    status_message: Option<String>,
    error_message: Option<String>,
}

impl ContextOptimizerPanelState {
    pub fn load(config: Rc<ContextOptimizerConfig>, locale: InterfaceLocale) -> Result<Self, String> {
        let snapshot = config.snapshot().map_err(|err| err.to_string())?;
        Ok(Self {
            config,
            snapshot,
            selected_agent: KNOWN_AGENT_IDS[0].to_owned(),
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
        if self.snapshot.agents.contains_key(agent_id) {
            self.selected_agent = agent_id.to_owned();
            self.status_message = None;
            self.error_message = None;
        }
    }

    pub fn agent_ids(&self) -> Vec<String> {
        self.snapshot.agents.keys().cloned().collect()
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
            OptimizerToggle::MasterEnabled => next.enabled = !next.enabled,
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
        let saved = &entry.settings;
        let mut defaults = AgentOptimizerSettings::default();
        if entry.availability(OptimizerToggle::NoiseFilter) != OptimizerAvailability::Available {
            defaults.noise_filter = saved.noise_filter;
            defaults.max_log_bytes = saved.max_log_bytes;
        }
        if entry.availability(OptimizerToggle::ReadDedup) != OptimizerAvailability::Available {
            defaults.read_dedup = saved.read_dedup;
        }
        if entry.availability(OptimizerToggle::BlockSensitiveReads)
            != OptimizerAvailability::Available
        {
            defaults.block_sensitive_reads = saved.block_sensitive_reads;
        }
        if entry.availability(OptimizerToggle::CacheKeepalive) != OptimizerAvailability::Available {
            defaults.cache_keepalive = saved.cache_keepalive;
            defaults.cache_keepalive_max_per_turn = saved.cache_keepalive_max_per_turn;
        }
        self.save_current(defaults)
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
}

fn unavailable_reason(
    field: OptimizerToggle,
    entry: &AgentOptimizerEntry,
    locale: InterfaceLocale,
) -> Option<&'static str> {
    use OptimizerAvailability::{Available, Pending, Unsupported};
    use OptimizerToggle::{CacheKeepalive, NoiseFilter};
    match (entry.availability(field), field, entry.agent_id.as_str()) {
        (Available, _, _) => None,
        (Pending, CacheKeepalive, _) => Some(locale.text(
            "暂无可靠的静默续命方式，自动发送会进入对话并消耗额度。有可验证的静默方式后再开放。",
            "Silent keepalive is unavailable. Automatic messages would appear in the conversation and use quota. This will open once silent delivery can be verified.",
        )),
        (Pending, NoiseFilter, "codex") => Some(locale.text(
            "待适配。Codex 当前版本不支持在输出交给 AI 前替换结果，自动剪裁暂不可用。",
            "Pending integration. This Codex version cannot replace tool output before it reaches the model, so automatic trimming is unavailable.",
        )),
        (Pending, NoiseFilter, "antigravity") => Some(locale.text(
            "待适配和验证。反重力支持执行前改写命令，Matou 完成适配并登录验证后开放。",
            "Pending integration and testing. Antigravity can rewrite commands before execution. Matou will enable trimming after integration and signed-in testing.",
        )),
        (Pending, _, "codex") => Some(locale.text(
            "待适配。Codex 通过 Bash 读文件，当前读保护未覆盖该路径。",
            "Pending integration. Codex reads files through Bash, which the current read protection does not cover.",
        )),
        (Pending, _, "antigravity") => Some(locale.text(
            "待验证。Matou 需适配反重力的专用读文件工具，并在登录后验证保护生效。",
            "Pending validation. Matou needs to integrate Antigravity's dedicated file reader and verify protection in a signed-in session.",
        )),
        (Pending, _, _) => Some(locale.text(
            "待适配或验证，确认可用后开放。",
            "Pending integration or validation. This will open once verified.",
        )),
        (Unsupported, OptimizerToggle::NoiseFilter | OptimizerToggle::ReadDedup | OptimizerToggle::BlockSensitiveReads, _) => Some(locale.text(
            "该 Agent 仅支持观察，不支持自动拦截。",
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

/// GPUI settings panel rendered inside `SettingsPage::ContextOptimizer`.
pub struct ContextOptimizerSettingsPanel {
    state: ContextOptimizerPanelState,
    palette: ThemePalette,
    dark: bool,
    agent_focus: BTreeMap<String, FocusHandle>,
    toggle_focus: [FocusHandle; OptimizerToggle::BlockSensitiveReads as usize + 1],
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
            agent_focus: BTreeMap::new(),
            toggle_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
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

    fn render_toggle_row(
        &self,
        id: &'static str,
        title: &'static str,
        desc: &'static str,
        field: OptimizerToggle,
        checked: bool,
        entry: &AgentOptimizerEntry,
        reveal_focus: Option<&FocusHandle>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let interactive = can_toggle(field, entry);
        let reason = unavailable_reason(field, entry, self.state.locale);
        let focus = &self.toggle_focus[field as usize];
        let accent = rgba(self.palette.workspace.accent);
        let title_color = rgba(if interactive {
            self.palette.workspace.text
        } else {
            self.palette.workspace.muted
        });

        let pill_bg = if !interactive {
            self.palette.workspace.line
        } else if checked {
            self.palette.workspace.accent
        } else if self.dark {
            0x3a4250ff
        } else {
            0xcbd5e1ff
        };

        let mut pill = div()
            .id(SharedString::from(format!("ctx-opt-toggle-{id}")))
            .w(px(36.))
            .h(px(20.))
            .flex_none()
            .rounded(px(10.))
            .p(px(2.))
            .flex()
            .items_center()
            .justify_start()
            .bg(rgba(pill_bg))
            .child(
                div()
                    .size(px(16.))
                    .rounded(px(8.))
                    .ml(if checked { px(16.) } else { px(0.) })
                    .bg(rgb(0xffffff)),
            );

        if interactive {
            pill = pill
                .track_focus(focus)
                .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.toggle(field);
                    cx.notify();
                }));
        } else {
            pill = pill.opacity(0.45);
        }

        let mut text_col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight(600.))
                    .text_color(title_color)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .line_height(px(14.5))
                    .text_color(rgba(self.palette.workspace.muted))
                    .child(desc),
            );
        if let Some(reason) = reason {
            // Keep the explanation reachable by Tab while the switch has
            // neither a focus target nor an activation handler.
            text_col = text_col.child(
                div()
                    .id(("ctx-opt-reason", field as usize))
                    .track_focus(focus)
                    .rounded(px(3.))
                    .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                    .text_size(px(10.5))
                    .line_height(px(14.5))
                    .text_color(rgba(self.palette.workspace.muted))
                    .child(reason),
            );
        }

        let row = reveal_on_focus(div(), &self.scroll, reveal_focus == Some(focus))
            .id(("ctx-opt-row", field as usize))
            .w_full()
            .py(px(10.))
            .pr(px(12.))
            .pl(if field == OptimizerToggle::BlockSensitiveReads {
                px(24.)
            } else {
                px(12.)
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(px(14.))
            .border_b_1()
            .border_color(rgba(self.palette.workspace.line))
            .child(text_col)
            .child(pill);
        if let Some(reason) = reason {
            row.tooltip(move |_, cx| cx.new(|_| CardTooltip(reason.into())).into())
        } else {
            row
        }
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
        let selected = self.state.selected_agent().to_owned();
        let agent_ids = self.state.agent_ids();
        let accent = rgba(self.palette.workspace.accent);
        let previous_focus = std::mem::replace(&mut self.last_focus, window.focused(cx));
        let reveal_focus = self
            .last_focus
            .as_ref()
            .filter(|focus| Some(*focus) != previous_focus.as_ref());

        let mut tabs = div().flex().flex_wrap().gap(px(6.)).mb(px(12.));
        for id in agent_ids {
            let active = id == selected;
            let label = agent_display_name(&id).to_owned();
            let id_click = id.clone();
            let focus: &FocusHandle = self
                .agent_focus
                .entry(id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
            tabs = tabs.child(
                reveal_on_focus(div(), &self.scroll, reveal_focus == Some(focus))
                    .id(SharedString::from(format!("ctx-opt-agent-{id}")))
                    .track_focus(focus)
                    .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                    .px(px(12.))
                    .py(px(6.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .font_weight(FontWeight(620.))
                    .bg(rgba(if active {
                        self.palette.workspace.accent_soft
                    } else {
                        0x00000000
                    }))
                    .border_1()
                    .border_color(rgba(if active {
                        self.palette.workspace.accent
                    } else {
                        self.palette.workspace.line
                    }))
                    .text_color(rgba(if active {
                        self.palette.workspace.accent
                    } else {
                        self.palette.workspace.text
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.select_agent(&id_click);
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        let Some(entry) = self.state.current_entry().cloned() else {
            return div()
                .child(locale.text("未找到 Agent 配置", "Agent configuration unavailable"));
        };
        let s = &entry.settings;
        let row = |id, title, desc, field, checked, cx: &mut Context<Self>| {
            self.render_toggle_row(id, title, desc, field, checked, &entry, reveal_focus, cx)
        };

        let rows = div()
            .w_full()
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(self.palette.workspace.line))
            .flex()
            .flex_col()
            .child(row(
                "master",
                locale.text("为该 Agent 启用上下文与缓存优化", "Enable context & cache optimizer"),
                locale.text(
                    "开启后可调整下方设置。关闭时会保留已保存的偏好。",
                    "Enable to change the settings below. Turning this off keeps your saved preferences.",
                ),
                OptimizerToggle::MasterEnabled,
                s.enabled,
                cx,
            ))
            .child(row(
                "cache-monitor",
                locale.text("KV-Cache 监控与水位预警", "KV-cache countdown & context alerts"),
                locale.text(
                    "在卡片与底栏 HUD 显示上下文占用和缓存剩余有效期（零前缀污染）。",
                    "Shows context usage and cache TTL countdown in the HUD without mutating prompts.",
                ),
                OptimizerToggle::CacheMonitor,
                s.cache_monitor,
                cx,
            ))
            .child(row(
                "stall-rescue",
                locale.text("卡死检测与一键交接救援", "Stall watchdog & rescue handoff"),
                locale.text(
                    "监控后台会话静默卡死（带合盖休眠防误报），并自动准备脱敏交接摘要。",
                    "Detects silent background stalls with sleep-drift protection and prepares redacted handoff briefs.",
                ),
                OptimizerToggle::StallRescue,
                s.stall_rescue,
                cx,
            ))
            .child(row(
                "noise-filter",
                locale.text("终端噪音剪裁与密钥脱敏", "Terminal noise trimming & secret redaction"),
                locale.text(
                    "将超长测试/构建日志压缩为报错与摘要，完整原文存本地；自动遮蔽 API Key。",
                    "Trims noisy build/test logs to errors and summaries while saving full logs locally.",
                ),
                OptimizerToggle::NoiseFilter,
                s.noise_filter,
                cx,
            ))
            .child(row(
                "read-dedup",
                locale.text("防重复读与大文件分级保护", "Duplicate-read guard & tiered read caps"),
                locale.text(
                    "保护专用读文件工具。拦下未修改大文件的重复读取，120 秒内再试一次会放行。",
                    "Protects dedicated file reads. A repeated read of an unchanged large file is blocked, and retrying within 120 seconds is allowed.",
                ),
                OptimizerToggle::ReadDedup,
                s.read_dedup,
                cx,
            ))
            .child(row(
                "block-sensitive-reads",
                locale.text(".env 与敏感文件保护", "Protect .env and sensitive files"),
                locale.text(
                    "阻止专用读文件工具读取 .env 等敏感文件，放行 .env.example 等样板文件。",
                    "Blocks dedicated file reads of sensitive files such as .env and lets templates such as .env.example through.",
                ),
                OptimizerToggle::BlockSensitiveReads,
                s.block_sensitive_reads,
                cx,
            ))
            .child(row(
                "cli-tools",
                locale.text("内置提速工具（mt ctx）", "Built-in CLI helpers (mt ctx)"),
                locale.text(
                    "零 MCP 方式提供文件大纲、批量搜索、本地日志检索与 /compact 恢复。",
                    "Zero-MCP CLI for file outlines, batch search, local log search, and post-compact resume.",
                ),
                OptimizerToggle::CliTools,
                s.cli_tools,
                cx,
            ))
            .child(row(
                "cache-keepalive",
                locale.text("自动续命", "Automatic cache keepalive"),
                locale.text(
                    "在主会话缓存快过期时尝试延长有效期。",
                    "Attempts to extend the cache lifetime shortly before it expires.",
                ),
                OptimizerToggle::CacheKeepalive,
                s.cache_keepalive,
                cx,
            ))
            .child(row(
                "cache-expiry-notify",
                locale.text("临过期提醒", "Cache expiry reminder"),
                locale.text(
                    "KV-Cache 监控开启时，缓存快过期会提醒，不会向 Agent 发送请求。",
                    "With KV-cache monitoring on, shows a reminder shortly before expiry without sending a request to the agent.",
                ),
                OptimizerToggle::CacheExpiryNotify,
                s.cache_expiry_notify,
                cx,
            ));

        let mut footer = div()
            .mt(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                reveal_on_focus(
                    div(),
                    &self.scroll,
                    reveal_focus == Some(&self.restore_focus),
                )
                .id("ctx-opt-restore-defaults")
                .track_focus(&self.restore_focus)
                .focus(|style| style.shadow(vec![outline_shadow(accent)]))
                .px(px(10.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgba(self.palette.workspace.line))
                .cursor_pointer()
                .text_size(px(11.))
                .text_color(rgba(self.palette.workspace.text))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.restore_defaults();
                    cx.notify();
                }))
                .child(locale.text("恢复可用项默认值", "Restore available defaults")),
            );

        if let Some(err) = self.state.error_message() {
            footer = footer.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0xb34d47))
                    .child(err.to_owned()),
            );
        } else if let Some(msg) = self.state.status_message() {
            footer = footer.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x16794b))
                    .child(msg.to_owned()),
            );
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .mt(px(14.))
            .child(tabs)
            .child(rows)
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentOptimizerSettings, ContextGuardHudView, ContextOptimizerConfig,
        ContextOptimizerPanelState, HudPillTone, InterfaceLocale, OptimizerAvailability,
        OptimizerToggle, can_toggle, format_cache_countdown, format_saved_noise_bytes,
        unavailable_reason,
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
            }
            let settings = &state.current_entry().unwrap().settings;
            assert!(
                settings.cache_keepalive && settings.cache_expiry_notify,
                "{id}"
            );
            assert!(!settings.cache_monitor, "{id}");
            assert!(!settings.stall_rescue, "{id}");
            assert!(settings.cli_tools, "{id}");
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
        assert_eq!(saved.cache_keepalive_max_per_turn, 3);
        assert_eq!(saved.max_log_bytes, 32_768);

        std::fs::remove_dir_all(&dir).unwrap();
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
        assert!(ContextGuardHudView::from_live_hud(&serde_json::json!({}), InterfaceLocale::Chinese).is_none());
    }
}
