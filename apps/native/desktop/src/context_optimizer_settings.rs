//! Standalone GPUI panel and presentation helpers for per-agent context &
//! KV-cache optimizer settings (`<data_root>/context-optimizer.json`).

use std::rc::Rc;

use gpui::{prelude::*, *};
use matou_native_core::{
    context_optimizer_config::{
        AgentOptimizerCapabilities, AgentOptimizerEntry, AgentOptimizerSettings,
        ContextOptimizerConfig, ContextOptimizerSnapshot, KNOWN_AGENT_IDS,
    },
    sessions::InterfaceLocale,
};

use crate::theme_tokens::{AppTheme, ThemePalette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizerToggle {
    MasterEnabled,
    NoiseFilter,
    ReadDedup,
    CacheMonitor,
    CacheExpiryNotify,
    CacheKeepalive,
    StallRescue,
    CliTools,
    BlockSensitiveReads,
}

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
        if !toggle_supported(field, entry.capabilities) {
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
        self.save_current(AgentOptimizerSettings::default())
    }

    fn save_current(&mut self, next: AgentOptimizerSettings) -> bool {
        match self.config.set_agent_settings(&self.selected_agent, next) {
            Ok(updated) => {
                self.snapshot = updated;
                self.error_message = None;
                self.status_message = Some(
                    self.locale
                        .text("已保存并即时生效", "Saved and applied")
                        .to_owned(),
                );
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

pub fn toggle_supported(field: OptimizerToggle, caps: AgentOptimizerCapabilities) -> bool {
    match field {
        OptimizerToggle::MasterEnabled => true,
        OptimizerToggle::NoiseFilter
        | OptimizerToggle::ReadDedup
        | OptimizerToggle::BlockSensitiveReads => caps.sync_hook_intercept,
        OptimizerToggle::CacheMonitor | OptimizerToggle::CacheExpiryNotify => caps.cache_monitor,
        OptimizerToggle::CacheKeepalive => caps.cache_keepalive,
        OptimizerToggle::StallRescue => caps.stall_rescue,
        OptimizerToggle::CliTools => caps.cli_tools,
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

/// GPUI settings panel rendered inside `SettingsPage::ContextOptimizer`.
pub struct ContextOptimizerSettingsPanel {
    state: ContextOptimizerPanelState,
    palette: ThemePalette,
    dark: bool,
}

impl ContextOptimizerSettingsPanel {
    pub fn new(
        config: Rc<ContextOptimizerConfig>,
        locale: InterfaceLocale,
        theme: AppTheme,
    ) -> Result<Self, String> {
        Ok(Self {
            state: ContextOptimizerPanelState::load(config, locale)?,
            palette: *theme.palette(),
            dark: theme == AppTheme::Dark,
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
        master_on: bool,
        caps: AgentOptimizerCapabilities,
        cx: &mut Context<Self>,
    ) -> Div {
        let supported = toggle_supported(field, caps);
        let interactive = supported && (field == OptimizerToggle::MasterEnabled || master_on);
        let unsupported_hint = (!supported).then(|| {
            self.state.locale.text(
                "该 Agent 当前仅支持观测，暂不支持此项自动拦截",
                "This agent currently supports observation only for this feature",
            )
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
                    .ml(if checked && supported { px(16.) } else { px(0.) })
                    .bg(rgb(0xffffff)),
            );

        if interactive {
            pill = pill.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
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
                    .text_color(rgba(self.palette.workspace.text))
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .line_height(px(14.5))
                    .text_color(rgba(self.palette.workspace.muted))
                    .child(desc),
            );
        if let Some(hint) = unsupported_hint {
            text_col = text_col.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(0xb45309))
                    .child(hint),
            );
        }

        div()
            .w_full()
            .py(px(10.))
            .px(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(14.))
            .border_b_1()
            .border_color(rgba(self.palette.workspace.line))
            .child(text_col)
            .child(pill)
    }
}

impl Render for ContextOptimizerSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.state.locale;
        let selected = self.state.selected_agent().to_owned();
        let agent_ids = self.state.agent_ids();

        let mut tabs = div().flex().flex_wrap().gap(px(6.)).mb(px(12.));
        for id in agent_ids {
            let active = id == selected;
            let label = agent_display_name(&id).to_owned();
            let id_click = id.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("ctx-opt-agent-{id}")))
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
            return div().child(locale.text("未找到 Agent 配置", "Agent configuration unavailable"));
        };
        let s = &entry.settings;
        let caps = entry.capabilities;

        let rows = div()
            .w_full()
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(self.palette.workspace.line))
            .flex()
            .flex_col()
            .child(self.render_toggle_row(
                "master",
                locale.text("为该 Agent 启用上下文与缓存优化", "Enable context & cache optimizer"),
                locale.text(
                    "关闭后零注入、零拦截、零 HUD 额外状态，不改动该 Agent 原有行为。",
                    "When off, zero hooks, zero interception, and zero prompt changes are applied.",
                ),
                OptimizerToggle::MasterEnabled,
                s.enabled,
                s.enabled,
                caps,
                cx,
            ))
            .child(self.render_toggle_row(
                "cache-monitor",
                locale.text("KV-Cache 监控与水位预警", "KV-cache countdown & context alerts"),
                locale.text(
                    "在卡片与底栏 HUD 显示上下文占用和缓存剩余有效期（零前缀污染）。",
                    "Shows context usage and cache TTL countdown in the HUD without mutating prompts.",
                ),
                OptimizerToggle::CacheMonitor,
                s.cache_monitor,
                s.enabled,
                caps,
                cx,
            ))
            .child(self.render_toggle_row(
                "stall-rescue",
                locale.text("卡死检测与一键交接救援", "Stall watchdog & rescue handoff"),
                locale.text(
                    "监控后台会话静默卡死（带合盖休眠防误报），并自动准备脱敏交接摘要。",
                    "Detects silent background stalls with sleep-drift protection and prepares redacted handoff briefs.",
                ),
                OptimizerToggle::StallRescue,
                s.stall_rescue,
                s.enabled,
                caps,
                cx,
            ))
            .child(self.render_toggle_row(
                "noise-filter",
                locale.text("终端噪音剪裁与密钥脱敏", "Terminal noise trimming & secret redaction"),
                locale.text(
                    "将超长测试/构建日志压缩为报错与摘要，完整原文存本地；自动遮蔽 API Key。",
                    "Trims noisy build/test logs to errors and summaries while saving full logs locally.",
                ),
                OptimizerToggle::NoiseFilter,
                s.noise_filter,
                s.enabled,
                caps,
                cx,
            ))
            .child(self.render_toggle_row(
                "read-dedup",
                locale.text("防重复读与大文件分级保护", "Duplicate-read guard & tiered read caps"),
                locale.text(
                    "拦下未修改大文件的重复读取，120 秒内二次重试自动放行。",
                    "Avoids re-reading unchanged large files; second attempt within 120s passes through.",
                ),
                OptimizerToggle::ReadDedup,
                s.read_dedup,
                s.enabled,
                caps,
                cx,
            ))
            .child(self.render_toggle_row(
                "cli-tools",
                locale.text("内置提速工具（mt ctx）", "Built-in CLI helpers (mt ctx)"),
                locale.text(
                    "零 MCP 方式提供文件大纲、批量搜索、本地日志检索与 /compact 恢复。",
                    "Zero-MCP CLI for file outlines, batch search, local log search, and post-compact resume.",
                ),
                OptimizerToggle::CliTools,
                s.cli_tools,
                s.enabled,
                caps,
                cx,
            ))
            .child(self.render_toggle_row(
                "cache-keepalive",
                locale.text("KV-Cache 临过期自动保活（默认关闭）", "Optional KV-cache keepalive (off by default)"),
                locale.text(
                    "主会话缓存快过期前自动发一次短请求续命，每轮默认最多 1 次。",
                    "Sends a short ping before primary-chat cache expiry (default max 1 per user turn).",
                ),
                OptimizerToggle::CacheKeepalive,
                s.cache_keepalive,
                s.enabled,
                caps,
                cx,
            ));

        let mut footer = div()
            .mt(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("ctx-opt-restore-defaults")
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
                    .child(locale.text("恢复该 Agent 默认值", "Restore defaults")),
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
        ContextOptimizerConfig, ContextOptimizerPanelState, InterfaceLocale, OptimizerToggle,
        format_cache_countdown, format_saved_noise_bytes,
    };
    use std::rc::Rc;

    #[test]
    fn panel_state_persists_per_agent_toggles_and_blocks_unsupported_dsh_switches() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("matou-ctx-opt-test-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Rc::new(ContextOptimizerConfig::for_data_root(&dir));
        let mut state =
            ContextOptimizerPanelState::load(Rc::clone(&config), InterfaceLocale::Chinese)
                .unwrap();

        assert_eq!(state.selected_agent(), "claude-code");
        assert!(state.toggle(OptimizerToggle::NoiseFilter));
        assert!(state.current_entry().unwrap().settings.noise_filter);

        // Switch to DSH: noise_filter is unsupported so toggle must be ignored.
        state.select_agent("dsh");
        assert!(!state.toggle(OptimizerToggle::NoiseFilter));
        assert!(!state.current_entry().unwrap().settings.noise_filter);
        // DSH supports cli_tools.
        assert!(state.toggle(OptimizerToggle::CliTools));
        assert!(state.current_entry().unwrap().settings.cli_tools);

        // Reload verifies persistence on disk.
        let reloaded =
            ContextOptimizerPanelState::load(config, InterfaceLocale::English).unwrap();
        assert!(reloaded.snapshot.agents["claude-code"].settings.noise_filter);
        assert!(reloaded.snapshot.agents["dsh"].settings.cli_tools);
        let _ = std::fs::remove_dir_all(&dir);
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
}
