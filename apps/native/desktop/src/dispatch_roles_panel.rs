//! GPUI panel for Matou-local per-agent ecological niche and default model/effort/speed settings.

use gpui::{prelude::*, *};
use matou_native_core::dispatch_roles::{
    DispatchRoleMode, DispatchRolesConfig, DispatchRolesSnapshot, agent_display_name,
    effort_is_fixed, effort_options, model_options, speed_options,
};

use crate::{
    rename_input::{RenameInput, RenameInputEvent},
    settings_shell::{ACCENT, LINE, LINE_STRONG, MUTED, PANEL, PANEL_MUTED, TEXT},
    theme_tokens::AppTheme,
    workspace::workspace_theme_dark,
};

const TITLE: &str = "Agent 生态位与独立配置";
const HEADING: &str = "为每个执行者独立设置启停、调度方式、默认模型、推理档位与职责；仅保存在码头应用目录，保存后即时生效。";
const SUB_DISPATCH_TITLE: &str = "允许执行者二次转包";
const SUB_DISPATCH_DESC: &str = "开启后，执行者干活时遇到其他「按职责自动派」Agent 擅长的部分可转派 1 层子任务（如 Codex 遇前端 UI 转派 DSH）；关闭后只允许顶层监工派活。";
const RESTORE_DEFAULTS: &str = "恢复默认";
const SAVE: &str = "保存";
const SAVED_MESSAGE: &str = "已保存 · 对当前和新开会话即时生效";

const ERROR_TEXT: u32 = 0xb34d47;
const SUCCESS_TEXT: u32 = 0x16794b;
const SUCCESS_BG: u32 = 0x22965a1f;
const PILL_SELECTED_BG: u32 = 0xe4efff;
const PILL_SELECTED_TEXT: u32 = 0x2679d8;

/// Converts a frozen RGB constant to the packed `0xRRGGBBAA` form used by
/// [`RolePanelColors`].
const fn solid(rgb: u32) -> u32 {
    rgb << 8 | 0xff
}

/// Outer card, agent sub-card, control, pill, and status colors. Field values
/// are packed as `0xRRGGBBAA`. Light keeps the frozen renderer values exactly;
/// dark uses the settings/workspace tokens and never paints a white card.
struct RolePanelColors {
    /// Outer section card: `--workspace-panel-muted`, or `settings.providers`.
    card: u32,
    /// Agent sub-cards and the sub-dispatch toggle: `--workspace-panel`, or
    /// `--workspace-panel-muted` in dark.
    sub_card: u32,
    line: u32,
    line_strong: u32,
    text: u32,
    muted: u32,
    /// Unchecked checkbox and unselected pill surface.
    control: u32,
    /// Role text field surface.
    role_input: u32,
    input_text: u32,
    input_placeholder: u32,
    accent: u32,
    pill_selected_bg: u32,
    pill_selected_text: u32,
    /// Primary save button, matching `settings.language_primary`.
    primary: u32,
    error: u32,
    success: u32,
    success_bg: u32,
}

impl RolePanelColors {
    fn for_dark(dark: bool) -> Self {
        if dark {
            let palette = AppTheme::Dark.palette();
            let workspace = palette.workspace;
            let settings = palette.settings;
            Self {
                card: settings.providers,
                sub_card: workspace.panel_muted,
                line: workspace.line,
                line_strong: workspace.line_strong,
                text: workspace.text,
                muted: workspace.muted,
                // `.model-provider__actions button` / `.provider-setup input`.
                control: settings.provider_action_background,
                role_input: settings.provider_action_background,
                input_text: workspace.text,
                input_placeholder: workspace.muted,
                accent: workspace.accent,
                pill_selected_bg: workspace.accent_soft,
                pill_selected_text: workspace.accent,
                primary: settings.language_primary,
                error: 0xff9b94ff,
                success: 0x57ab5aff,
                success_bg: SUCCESS_BG,
            }
        } else {
            Self {
                card: solid(PANEL_MUTED),
                sub_card: solid(PANEL),
                line: LINE,
                line_strong: LINE_STRONG,
                text: solid(TEXT),
                muted: solid(MUTED),
                control: solid(PANEL),
                role_input: solid(PANEL_MUTED),
                // `RenameInput::search_style` defaults; kept identical so the
                // frozen role fields do not change color in light mode.
                input_text: solid(0x2f3547),
                input_placeholder: solid(0x747474),
                accent: solid(ACCENT),
                pill_selected_bg: solid(PILL_SELECTED_BG),
                pill_selected_text: solid(PILL_SELECTED_TEXT),
                primary: solid(ACCENT),
                error: solid(ERROR_TEXT),
                success: solid(SUCCESS_TEXT),
                success_bg: SUCCESS_BG,
            }
        }
    }
}

/// Writes the theme colors into a child editor only when they differ, so a
/// re-render in the same theme never emits a redundant child notification.
fn sync_input_colors(input: &Entity<RenameInput>, text: u32, placeholder: u32, cx: &mut App) {
    let text = rgba(text);
    let placeholder = rgba(placeholder);
    let changed = {
        let current = input.read(cx);
        current.text_color != Some(text) || current.placeholder_color != Some(placeholder)
    };
    if changed {
        input.update(cx, |input, cx| {
            input.text_color = Some(text);
            input.placeholder_color = Some(placeholder);
            cx.notify();
        });
    }
}

pub struct DispatchRolesPanel {
    saved: DispatchRolesConfig,
    draft: DispatchRolesConfig,
    path_display: String,
    write: Box<dyn FnMut(DispatchRolesConfig) -> Result<DispatchRolesSnapshot, String>>,
    role_inputs: Vec<Entity<RenameInput>>,
    _subscriptions: Vec<Subscription>,
    status: Option<Result<String, String>>,
}

impl DispatchRolesPanel {
    pub fn new(
        snapshot: DispatchRolesSnapshot,
        write: impl FnMut(DispatchRolesConfig) -> Result<DispatchRolesSnapshot, String> + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let path_display = snapshot.path.display().to_string();
        let draft = snapshot.config.clone();
        let mut role_inputs = Vec::with_capacity(draft.agents.len());
        let mut subscriptions = Vec::with_capacity(draft.agents.len());
        for (index, agent) in draft.agents.iter().enumerate() {
            let text = agent.role.clone();
            let input = cx.new(|cx| {
                let mut editor = RenameInput::new(text, cx);
                editor.search_style = true;
                editor.placeholder = "填写这个 Agent 擅长的职责与使用场景…".into();
                editor
            });
            subscriptions.push(cx.subscribe(&input, move |this, _, event, cx| {
                if matches!(event, RenameInputEvent::Changed | RenameInputEvent::Confirm) {
                    this.role_edited(index, cx);
                }
            }));
            role_inputs.push(input);
        }
        Self {
            saved: snapshot.config,
            draft,
            path_display,
            write: Box::new(write),
            role_inputs,
            _subscriptions: subscriptions,
            status: None,
        }
    }

    fn dirty(&self) -> bool {
        self.draft != self.saved
    }

    fn role_edited(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(input) = self.role_inputs.get(index)
            && let Some(agent) = self.draft.agents.get_mut(index)
        {
            let text = input.read(cx).text.clone();
            if text != agent.role {
                agent.role = text;
                self.status = None;
                cx.notify();
            }
        }
    }

    fn sync_role_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (index, agent) in self.draft.agents.iter().enumerate() {
            if let Some(input) = self.role_inputs.get(index) {
                let desired = agent.role.clone();
                let length = {
                    let current = input.read(cx);
                    if current.text == desired {
                        continue;
                    }
                    current.text.encode_utf16().count()
                };
                input.update(cx, |editor, cx| {
                    editor.replace_text_in_range(Some(0..length), &desired, window, cx);
                });
            }
        }
    }

    fn toggle_sub_dispatch(&mut self, cx: &mut Context<Self>) {
        self.draft.allow_sub_dispatch = !self.draft.allow_sub_dispatch;
        self.status = None;
        cx.notify();
    }

    fn toggle_enabled(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(agent) = self.draft.agents.get(index) {
            if agent.enabled
                && self
                    .draft
                    .agents
                    .iter()
                    .filter(|item| item.enabled)
                    .count()
                    <= 1
            {
                self.status = Some(Err("至少保留一个启用的执行者".into()));
                cx.notify();
                return;
            }
        }
        if let Some(agent) = self.draft.agents.get_mut(index) {
            agent.enabled = !agent.enabled;
            self.status = None;
            cx.notify();
        }
    }

    fn set_mode(&mut self, index: usize, mode: DispatchRoleMode, cx: &mut Context<Self>) {
        if let Some(agent) = self.draft.agents.get_mut(index)
            && agent.enabled
            && agent.mode != mode
        {
            agent.mode = mode;
            self.status = None;
            cx.notify();
        }
    }

    fn set_model(&mut self, index: usize, model: &str, cx: &mut Context<Self>) {
        if let Some(agent) = self.draft.agents.get_mut(index)
            && agent.enabled
            && agent.model != model
        {
            agent.model = model.to_owned();
            self.status = None;
            cx.notify();
        }
    }

    fn set_effort(&mut self, index: usize, effort: &str, cx: &mut Context<Self>) {
        if let Some(agent) = self.draft.agents.get_mut(index)
            && agent.enabled
            && agent.effort != effort
        {
            agent.effort = effort.to_owned();
            self.status = None;
            cx.notify();
        }
    }

    fn set_speed(&mut self, index: usize, speed: &str, cx: &mut Context<Self>) {
        if let Some(agent) = self.draft.agents.get_mut(index)
            && agent.enabled
            && agent.speed.as_deref() != Some(speed)
        {
            agent.speed = Some(speed.to_owned());
            self.status = None;
            cx.notify();
        }
    }

    fn restore_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = DispatchRolesConfig::default();
        self.sync_role_inputs(window, cx);
        self.status = None;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (self.write)(self.draft.clone()) {
            Ok(snapshot) => {
                self.saved = snapshot.config.clone();
                self.draft = snapshot.config;
                self.path_display = snapshot.path.display().to_string();
                self.sync_role_inputs(window, cx);
                self.status = Some(Ok(SAVED_MESSAGE.into()));
            }
            Err(error) => {
                self.status = Some(Err(error));
            }
        }
        cx.notify();
    }

    fn render_pill(
        id: impl Into<ElementId>,
        label: &'static str,
        selected: bool,
        disabled: bool,
        colors: &RolePanelColors,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut pill = div()
            .id(id)
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .border_1()
            .text_size(px(11.))
            .cursor_pointer()
            .border_color(if selected {
                rgba(colors.accent)
            } else {
                rgba(colors.line_strong)
            })
            .bg(if selected {
                rgba(colors.pill_selected_bg)
            } else {
                rgba(colors.control)
            })
            .text_color(if selected {
                rgba(colors.pill_selected_text)
            } else {
                rgba(colors.text)
            })
            .child(label);
        if disabled {
            pill = pill.opacity(0.5).cursor_default();
        } else {
            pill = pill.on_click(cx.listener(move |this, _, _, cx| {
                on_click(this, cx);
            }));
        }
        pill
    }

    fn render_agent_card(
        &self,
        index: usize,
        colors: &RolePanelColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let agent = &self.draft.agents[index];
        let enabled = agent.enabled;
        let id = agent.id.clone();
        let models = model_options(&id);
        let efforts = effort_options(&id);
        let speeds = speed_options(&id);
        let fixed_effort = effort_is_fixed(&id, &agent.model);

        let checkbox = div()
            .id(SharedString::from(format!("dispatch-role-enable-{id}")))
            .w(px(15.))
            .h(px(15.))
            .rounded(px(4.))
            .border_1()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .border_color(if enabled {
                rgba(colors.accent)
            } else {
                rgba(colors.line_strong)
            })
            .bg(if enabled {
                rgba(colors.accent)
            } else {
                rgba(colors.control)
            })
            .text_size(px(10.))
            .text_color(rgb(0xffffff))
            .child(if enabled { "✓" } else { "" })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_enabled(index, cx);
            }));

        let mode_row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(Self::render_pill(
                SharedString::from(format!("dispatch-role-mode-auto-{id}")),
                DispatchRoleMode::Auto.label_zh(),
                agent.mode == DispatchRoleMode::Auto,
                !enabled,
                colors,
                move |this, cx| this.set_mode(index, DispatchRoleMode::Auto, cx),
                cx,
            ))
            .child(Self::render_pill(
                SharedString::from(format!("dispatch-role-mode-explicit-{id}")),
                DispatchRoleMode::Explicit.label_zh(),
                agent.mode == DispatchRoleMode::Explicit,
                !enabled,
                colors,
                move |this, cx| this.set_mode(index, DispatchRoleMode::Explicit, cx),
                cx,
            ));

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(checkbox)
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight(650.))
                            .text_color(rgba(colors.text))
                            .child(agent_display_name(&id).to_owned()),
                    )
                    .when(!enabled, |row| {
                        row.child(
                            div()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(rgba(colors.line))
                                .text_size(px(10.))
                                .text_color(rgba(colors.muted))
                                .child("已停用"),
                        )
                    }),
            )
            .child(mode_row);

        let mut model_group = div().flex().flex_wrap().items_center().gap(px(6.)).child(
            div()
                .w(px(64.))
                .text_size(px(11.))
                .text_color(rgba(colors.muted))
                .child("默认模型"),
        );
        for &(value, label) in models {
            model_group = model_group.child(Self::render_pill(
                SharedString::from(format!("dispatch-role-model-{id}-{value}")),
                label,
                agent.model == value,
                !enabled,
                colors,
                move |this, cx| this.set_model(index, value, cx),
                cx,
            ));
        }

        let mut effort_group = div().flex().flex_wrap().items_center().gap(px(6.)).child(
            div()
                .w(px(64.))
                .text_size(px(11.))
                .text_color(rgba(colors.muted))
                .child("推理档位"),
        );
        if fixed_effort {
            effort_group = effort_group.child(
                div()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(colors.line))
                    .bg(rgba(colors.sub_card))
                    .text_size(px(11.))
                    .text_color(rgba(colors.muted))
                    .child("固定（无档位）"),
            );
        } else {
            for &(value, label) in efforts {
                effort_group = effort_group.child(Self::render_pill(
                    SharedString::from(format!("dispatch-role-effort-{id}-{value}")),
                    label,
                    agent.effort == value,
                    !enabled,
                    colors,
                    move |this, cx| this.set_effort(index, value, cx),
                    cx,
                ));
            }
        }

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.sub_card))
            .when(!enabled, |card| card.opacity(0.6))
            .child(header)
            .when(!models.is_empty(), |card| card.child(model_group))
            .when(!efforts.is_empty(), |card| card.child(effort_group));

        if !speeds.is_empty() {
            let mut speed_group = div().flex().flex_wrap().items_center().gap(px(6.)).child(
                div()
                    .w(px(64.))
                    .text_size(px(11.))
                    .text_color(rgba(colors.muted))
                    .child("推理速度"),
            );
            let current_speed = agent.speed.as_deref().unwrap_or("default");
            for &(value, label) in speeds {
                speed_group = speed_group.child(Self::render_pill(
                    SharedString::from(format!("dispatch-role-speed-{id}-{value}")),
                    label,
                    current_speed == value,
                    !enabled,
                    colors,
                    move |this, cx| this.set_speed(index, value, cx),
                    cx,
                ));
            }
            card = card.child(speed_group);
        }

        if let Some(input) = self.role_inputs.get(index) {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgba(colors.muted))
                            .child("生态位与职责"),
                    )
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgba(colors.line_strong))
                            .bg(rgba(colors.role_input))
                            .child(input.clone()),
                    ),
            );
        }

        card
    }
}

impl Render for DispatchRolesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = RolePanelColors::for_dark(workspace_theme_dark(cx));
        for input in &self.role_inputs {
            sync_input_colors(input, colors.input_text, colors.input_placeholder, cx);
        }

        let header = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight(680.))
                    .text_color(rgba(colors.text))
                    .child(TITLE),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgba(colors.muted))
                    .whitespace_normal()
                    .child(HEADING),
            );

        let sub_enabled = self.draft.allow_sub_dispatch;
        let sub_toggle = div()
            .id("dispatch-roles-allow-sub-dispatch")
            .flex()
            .items_start()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.sub_card))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_sub_dispatch(cx);
            }))
            .child(
                div()
                    .mt(px(2.))
                    .w(px(15.))
                    .h(px(15.))
                    .rounded(px(4.))
                    .border_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_color(if sub_enabled {
                        rgba(colors.accent)
                    } else {
                        rgba(colors.line_strong)
                    })
                    .bg(if sub_enabled {
                        rgba(colors.accent)
                    } else {
                        rgba(colors.control)
                    })
                    .text_size(px(10.))
                    .text_color(rgb(0xffffff))
                    .child(if sub_enabled { "✓" } else { "" }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgba(colors.text))
                            .child(SUB_DISPATCH_TITLE),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(relative(1.55))
                            .text_color(rgba(colors.muted))
                            .whitespace_normal()
                            .child(SUB_DISPATCH_DESC),
                    ),
            );

        let mut container = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.card))
            .child(sub_toggle);

        for index in 0..self.draft.agents.len() {
            container = container.child(self.render_agent_card(index, &colors, cx));
        }

        let dirty = self.dirty();
        let save_button = div()
            .id("dispatch-roles-save")
            .px(px(14.))
            .py(px(6.))
            .rounded(px(7.))
            .text_size(px(12.))
            .font_weight(FontWeight(600.))
            .bg(rgba(colors.primary))
            .text_color(rgb(0xffffff))
            .when(!dirty, |btn| btn.opacity(0.45).cursor_default())
            .when(dirty, |btn| {
                btn.cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx)))
            })
            .child(SAVE);

        let restore_button = div()
            .id("dispatch-roles-restore")
            .px(px(10.))
            .py(px(6.))
            .rounded(px(7.))
            .border_1()
            .border_color(rgba(colors.line_strong))
            .bg(rgba(colors.control))
            .text_size(px(12.))
            .text_color(rgba(colors.text))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.restore_defaults(window, cx)))
            .child(RESTORE_DEFAULTS);

        let status_view = match &self.status {
            Some(Ok(message)) => Some(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .bg(rgba(colors.success_bg))
                    .text_size(px(11.))
                    .text_color(rgba(colors.success))
                    .child(message.clone()),
            ),
            Some(Err(message)) => Some(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(colors.error))
                    .child(message.clone()),
            ),
            None => None,
        };

        let actions = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .pt(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(restore_button)
                    .children(status_view),
            )
            .child(save_button);

        container = container.child(actions);

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .w_full()
            .mt(px(22.))
            .pt(px(20.))
            .border_t_1()
            .border_color(rgba(colors.line))
            .child(header)
            .child(container)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(colors.muted))
                    .child(format!("生态位配置文件：{}", self.path_display)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::RolePanelColors;
    use crate::theme_tokens::AppTheme;

    /// WCAG relative luminance of a packed `0xRRGGBBAA` color.
    fn luminance(color: u32) -> f32 {
        fn channel(color: u32, shift: u32) -> f32 {
            let value = ((color >> shift) & 0xff) as f32 / 255.;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(color, 24) + 0.7152 * channel(color, 16) + 0.0722 * channel(color, 8)
    }

    /// Composites a translucent `fg` over an opaque `bg`.
    fn over(fg: u32, bg: u32) -> u32 {
        let alpha = (fg & 0xff) as f32 / 255.;
        let mix = |shift: u32| {
            (((fg >> shift & 0xff) as f32 * alpha + (bg >> shift & 0xff) as f32 * (1. - alpha))
                .round()) as u32
        };
        mix(24) << 24 | mix(16) << 16 | mix(8) << 8 | 0xff
    }

    fn contrast(a: u32, b: u32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn role_panel_colors_keep_light_values_and_readable_dark_surfaces() {
        let light = RolePanelColors::for_dark(false);
        assert_eq!(light.card, 0xfbfcfdff);
        assert_eq!(light.sub_card, 0xffffffff);
        assert_eq!(light.line, 0x424b581a);
        assert_eq!(light.line_strong, 0x424b5829);
        assert_eq!(light.text, 0x303744ff);
        assert_eq!(light.muted, 0x7e8794ff);
        assert_eq!(light.control, 0xffffffff);
        assert_eq!(light.role_input, 0xfbfcfdff);
        assert_eq!(light.input_text, 0x2f3547ff);
        assert_eq!(light.input_placeholder, 0x747474ff);
        assert_eq!(light.accent, 0x347fd6ff);
        assert_eq!(light.pill_selected_bg, 0xe4efffff);
        assert_eq!(light.pill_selected_text, 0x2679d8ff);
        assert_eq!(light.primary, 0x347fd6ff);
        assert_eq!(light.error, 0xb34d47ff);
        assert_eq!(light.success, 0x16794bff);
        assert_eq!(light.success_bg, 0x22965a1f);

        let dark = RolePanelColors::for_dark(true);
        // Neither the main card nor the agent sub-card may stay white.
        assert_eq!(dark.card, 0x25282dc7);
        assert_eq!(dark.sub_card, 0x23262be0);
        assert_ne!(dark.card, 0xffffffff);
        assert_ne!(dark.sub_card, 0xffffffff);
        assert_ne!(dark.control, 0xffffffff);
        assert_ne!(dark.role_input, 0xffffffff);
        assert_eq!(dark.control, 0xffffff0b);
        assert_eq!(dark.role_input, 0xffffff0b);
        assert_eq!(dark.line, 0xffffff16);
        assert_eq!(dark.line_strong, 0xffffff26);
        assert_eq!(dark.text, 0xe1e5ebff);
        assert_eq!(dark.muted, 0x8d96a2ff);
        assert_eq!(dark.accent, 0x67a6efff);
        assert_eq!(dark.pill_selected_bg, 0x4a91e224);
        assert_eq!(dark.pill_selected_text, 0x67a6efff);
        assert_eq!(dark.input_text, 0xe1e5ebff);
        assert_eq!(dark.input_placeholder, 0x8d96a2ff);
        assert_eq!(dark.primary, 0x2f79dfff);
        assert_eq!(dark.error, 0xff9b94ff);
        assert_eq!(dark.success, 0x57ab5aff);

        let canvas = AppTheme::Dark.palette().workspace.canvas;
        let card = over(dark.card, canvas);
        let sub_card = over(dark.sub_card, card);
        let control = over(dark.control, sub_card);
        let pill = over(dark.pill_selected_bg, sub_card);
        assert_ne!(card, 0xffffffff);
        assert_ne!(sub_card, 0xffffffff);
        assert_ne!(control, 0xffffffff);
        assert!(contrast(dark.text, card) >= 4.5);
        assert!(contrast(dark.text, sub_card) >= 4.5);
        assert!(contrast(dark.input_text, control) >= 4.5);
        assert!(contrast(dark.input_placeholder, control) >= 4.0);
        assert!(contrast(dark.pill_selected_text, pill) >= 4.0);
        assert!(contrast(dark.success, control) >= 4.0);
        assert!(contrast(dark.error, card) >= 4.5);
    }
}
