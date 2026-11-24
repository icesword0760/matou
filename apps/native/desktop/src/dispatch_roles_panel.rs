//! GPUI panel for Matou-local per-agent ecological niche and default model/effort/speed settings.

use gpui::{prelude::*, *};
use matou_native_core::dispatch_roles::{
    DispatchRoleMode, DispatchRolesConfig, DispatchRolesSnapshot, agent_display_name,
    effort_is_fixed, effort_options, model_options, speed_options,
};

use crate::{
    rename_input::{RenameInput, RenameInputEvent},
    settings_shell::{ACCENT, LINE, LINE_STRONG, MUTED, PANEL, PANEL_MUTED, TEXT},
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
                rgb(ACCENT)
            } else {
                rgba(LINE_STRONG)
            })
            .bg(if selected {
                rgb(PILL_SELECTED_BG)
            } else {
                rgb(PANEL)
            })
            .text_color(if selected {
                rgb(PILL_SELECTED_TEXT)
            } else {
                rgb(TEXT)
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

    fn render_agent_card(&self, index: usize, cx: &mut Context<Self>) -> Div {
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
                rgb(ACCENT)
            } else {
                rgba(LINE_STRONG)
            })
            .bg(if enabled { rgb(ACCENT) } else { rgb(PANEL) })
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
                move |this, cx| this.set_mode(index, DispatchRoleMode::Auto, cx),
                cx,
            ))
            .child(Self::render_pill(
                SharedString::from(format!("dispatch-role-mode-explicit-{id}")),
                DispatchRoleMode::Explicit.label_zh(),
                agent.mode == DispatchRoleMode::Explicit,
                !enabled,
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
                            .text_color(rgb(TEXT))
                            .child(agent_display_name(&id).to_owned()),
                    )
                    .when(!enabled, |row| {
                        row.child(
                            div()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(rgba(LINE))
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child("已停用"),
                        )
                    }),
            )
            .child(mode_row);

        let mut model_group = div().flex().flex_wrap().items_center().gap(px(6.)).child(
            div()
                .w(px(64.))
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child("默认模型"),
        );
        for &(value, label) in models {
            model_group = model_group.child(Self::render_pill(
                SharedString::from(format!("dispatch-role-model-{id}-{value}")),
                label,
                agent.model == value,
                !enabled,
                move |this, cx| this.set_model(index, value, cx),
                cx,
            ));
        }

        let mut effort_group = div().flex().flex_wrap().items_center().gap(px(6.)).child(
            div()
                .w(px(64.))
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child("推理档位"),
        );
        if fixed_effort {
            effort_group = effort_group.child(
                div()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(LINE))
                    .bg(rgb(PANEL))
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child("固定（无档位）"),
            );
        } else {
            for &(value, label) in efforts {
                effort_group = effort_group.child(Self::render_pill(
                    SharedString::from(format!("dispatch-role-effort-{id}-{value}")),
                    label,
                    agent.effort == value,
                    !enabled,
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
            .border_color(rgba(LINE))
            .bg(rgb(PANEL))
            .when(!enabled, |card| card.opacity(0.6))
            .child(header)
            .when(!models.is_empty(), |card| card.child(model_group))
            .when(!efforts.is_empty(), |card| card.child(effort_group));

        if !speeds.is_empty() {
            let mut speed_group = div().flex().flex_wrap().items_center().gap(px(6.)).child(
                div()
                    .w(px(64.))
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child("推理速度"),
            );
            let current_speed = agent.speed.as_deref().unwrap_or("default");
            for &(value, label) in speeds {
                speed_group = speed_group.child(Self::render_pill(
                    SharedString::from(format!("dispatch-role-speed-{id}-{value}")),
                    label,
                    current_speed == value,
                    !enabled,
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
                            .text_color(rgb(MUTED))
                            .child("生态位与职责"),
                    )
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgba(LINE_STRONG))
                            .bg(rgb(PANEL_MUTED))
                            .child(input.clone()),
                    ),
            );
        }

        card
    }
}

impl Render for DispatchRolesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight(680.))
                    .text_color(rgb(TEXT))
                    .child(TITLE),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgb(MUTED))
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
            .border_color(rgba(LINE))
            .bg(rgb(PANEL))
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
                        rgb(ACCENT)
                    } else {
                        rgba(LINE_STRONG)
                    })
                    .bg(if sub_enabled {
                        rgb(ACCENT)
                    } else {
                        rgb(PANEL)
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
                            .text_color(rgb(TEXT))
                            .child(SUB_DISPATCH_TITLE),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(relative(1.55))
                            .text_color(rgb(MUTED))
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
            .border_color(rgba(LINE))
            .bg(rgb(PANEL_MUTED))
            .child(sub_toggle);

        for index in 0..self.draft.agents.len() {
            container = container.child(self.render_agent_card(index, cx));
        }

        let dirty = self.dirty();
        let save_button = div()
            .id("dispatch-roles-save")
            .px(px(14.))
            .py(px(6.))
            .rounded(px(7.))
            .text_size(px(12.))
            .font_weight(FontWeight(600.))
            .bg(rgb(ACCENT))
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
            .border_color(rgba(LINE_STRONG))
            .bg(rgb(PANEL))
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.restore_defaults(window, cx)))
            .child(RESTORE_DEFAULTS);

        let status_view = match &self.status {
            Some(Ok(message)) => Some(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .bg(rgba(SUCCESS_BG))
                    .text_size(px(11.))
                    .text_color(rgb(SUCCESS_TEXT))
                    .child(message.clone()),
            ),
            Some(Err(message)) => Some(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(ERROR_TEXT))
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
            .border_color(rgba(LINE))
            .child(header)
            .child(container)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(format!("生态位配置文件：{}", self.path_display)),
            )
    }
}
