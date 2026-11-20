//! The native Claude model-assignment section, draft, and request handling.
//!
//! The view renders a projection of the current draft and emits UI actions.
//! The draft and saved baseline only adopt a write when the runtime confirms it.
//!
//! The dropdowns are real GPUI menus rather than a fake select: the panel is
//! rendered inside the settings scroll container, so the popup and its dismiss
//! backdrop are drawn through `deferred(anchored(...))` and cannot be clipped
//! by that container. Keyboard, mouse, Escape, Tab, and outside click all close
//! the menu.
//!
//! The manual model field reuses [`RenameInput`] so IME composition, selection,
//! clipboard, and native key handling match the provider editor. Its search
//! presentation is used because a model id is not capped at the editor's
//! 20-character label limit and the frozen manual field has no counter.
use std::{cell::RefCell, rc::Rc};

use gpui::{prelude::*, *};

use super::{
    ACCENT, DANGER_BG, DANGER_TEXT, FocusRingStyle, LINE, LINE_STRONG, MUTED, PANEL, PANEL_MUTED,
    TEXT, color_alpha, focus_ring_with, icon, shadow,
};
use crate::rename_input::{RenameInput, RenameInputEvent};

/// Model select value that switches the row to the manual model-id field.
pub(crate) const MANUAL_MODEL_SENTINEL: &str = "__manual__";

// Frozen copy. These values are shared with the renderer source and must not be
// paraphrased.
const VIEW_TITLE: &str = "模型分配";
const VIEW_SUBTITLE: &str = "为不同档位指定供应商和模型，未指定时沿用默认供应商。";
const VIEW_HEAD_TIER: &str = "档位";
const VIEW_HEAD_PROVIDER: &str = "供应商 / 账号";
const VIEW_HEAD_MODEL: &str = "模型";
const VIEW_HEAD_EFFORT: &str = "推理强度";
const VIEW_LOADING: &str = "正在读取模型分配…";
const VIEW_DISCONNECTED: &str = "设置服务未连接。";
const VIEW_EMPTY: &str = "没有可用供应商，请先添加供应商。";
const VIEW_RETRY: &str = "重试";
const VIEW_RESTORE: &str = "恢复默认";
const VIEW_SAVE: &str = "保存分配";
const VIEW_SAVING: &str = "保存中…";
const VIEW_SAVED: &str = "模型分配已保存，新会话将使用此配置";
const VIEW_SAVE_ERROR: &str = "保存未确认，请重试。原选择已保留。";
const VIEW_PRIVACY: &str = "使用指定供应商时，请求所需的对话内容会发送给该供应商。";
const VIEW_SCOPE: &str = "新会话使用此分配，正在运行的会话不受影响";

// Frozen status colors from `claude-role-assignments.css`; the shell currently
// has no success token, so these stay local to the assignment block.
const VIEW_SUCCESS_TEXT: u32 = 0x16794b;
const VIEW_SUCCESS_BG: u32 = 0x22965a1f; // rgba(34,150,90,.12)
// `.model-allocation__error { color: #b34d47 }`; the row role keeps the shell's
// darker DANGER_TEXT used by the frozen invalid-role rule.
const VIEW_ERROR: u32 = 0xb34d47;

// Existing settings-shell dropdown feedback (provider effort menu).
const VIEW_MENU_SELECTED_TEXT: u32 = 0x2679d8;
const VIEW_MENU_SELECTED_BG: u32 = 0xe4efff;
const VIEW_MENU_ACTIVE_BG: u32 = 0xeef5ff;

const VIEW_ROW_PADDING_Y: f32 = 12.;
const VIEW_FIELD_GAP: f32 = 5.;
const VIEW_GRID_GAP: f32 = 14.;
const VIEW_ROLE_WIDTH: f32 = 76.;
const VIEW_EFFORT_WIDTH: f32 = 108.;
const VIEW_NARROW_BREAKPOINT: f32 = 640.;

/// One frame of the frozen assignment block, supplied by the settings logic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignmentPanel {
    pub(crate) rows: [AssignmentRow; 3],
    pub(crate) status: AssignmentStatus,
    pub(crate) can_save: bool,
    pub(crate) can_restore: bool,
    pub(crate) saving: bool,
    pub(crate) saved: bool,
    pub(crate) save_error: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AssignmentStatus {
    Loading,
    Disconnected,
    Empty,
    Ready,
}

/// One tier row. Index order is Opus / Sonnet / Haiku.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignmentRow {
    pub(crate) tier: &'static str,
    pub(crate) provider: AssignmentSelect,
    pub(crate) account: String,
    pub(crate) model: AssignmentSelect,
    pub(crate) effort: AssignmentSelect,
    pub(crate) manual_model: Option<String>,
    pub(crate) target: String,
    pub(crate) problem: Option<String>,
    pub(crate) retry_catalog: bool,
}

/// A closed dropdown.
///
/// `label` is the text shown on the closed control. It is not a column heading:
/// the frozen block already has fixed table headings, and disabled selects show
/// values that are intentionally absent from `items` (for example the inherited
/// default model while a row is unassigned). `items` are the entries shown when
/// the menu opens; `selected` is the currently chosen value, or an empty string
/// for "follow / unassigned".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignmentSelect {
    pub(crate) label: String,
    pub(crate) enabled: bool,
    pub(crate) items: Vec<AssignmentOption>,
    pub(crate) selected: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignmentOption {
    pub(crate) value: String,
    pub(crate) label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AssignmentAction {
    Provider(usize, String),
    Model(usize, String),
    Effort(usize, String),
    Manual(usize, String),
    Retry,
    Restore,
    Save,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AssignmentColumn {
    Provider,
    Model,
    Effort,
}

impl AssignmentColumn {
    fn index(self) -> usize {
        match self {
            AssignmentColumn::Provider => 0,
            AssignmentColumn::Model => 1,
            AssignmentColumn::Effort => 2,
        }
    }

    fn menu_min_width(self) -> f32 {
        match self {
            AssignmentColumn::Provider => 200.,
            AssignmentColumn::Model => 220.,
            AssignmentColumn::Effort => 132.,
        }
    }
}

/// The open dropdown and its keyboard cursor.
#[derive(Clone)]
struct OpenAssignmentMenu {
    row: usize,
    column: AssignmentColumn,
    active: usize,
    trigger_focus: FocusHandle,
    items: Vec<AssignmentOption>,
    selected: String,
}

impl EventEmitter<AssignmentAction> for AssignmentView {}

/// The assignment block view. The parent keeps one entity for the settings
/// lifetime and calls [`AssignmentView::render_panel`] with the current panel.
pub(crate) struct AssignmentView {
    /// [row][provider / model / effort] trigger focus handles.
    controls: [[FocusHandle; 3]; 3],
    /// Slot 0 is the disconnect retry; slots 1..=3 are the per-row catalog retry.
    retry_focus: [FocusHandle; 4],
    restore_focus: FocusHandle,
    save_focus: FocusHandle,
    manual_inputs: [Entity<RenameInput>; 3],
    _manual_subscriptions: Vec<Subscription>,
    manual_edit: [bool; 3],
    manual_sync_suppress: [bool; 3],
    open_menu: Option<OpenAssignmentMenu>,
    trigger_bounds: Rc<RefCell<[[Option<Bounds<Pixels>>; 3]; 3]>>,
    menu_scroll: ScrollHandle,
    pending_focus: Option<FocusHandle>,
}

impl AssignmentView {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let controls: [[FocusHandle; 3]; 3] =
            std::array::from_fn(|_| std::array::from_fn(|_| cx.focus_handle().tab_stop(true)));
        let retry_focus = std::array::from_fn(|_| cx.focus_handle().tab_stop(true));
        let restore_focus = cx.focus_handle().tab_stop(true);
        let save_focus = cx.focus_handle().tab_stop(true);
        let manual_inputs: [Entity<RenameInput>; 3] = std::array::from_fn(|_| {
            cx.new(|cx| {
                let mut input = RenameInput::new(String::new(), cx);
                // Model ids are routinely longer than the editor's 20-unit
                // label cap; the frozen field is unbounded and shows no counter.
                input.search_style = true;
                input.placeholder.clear();
                input
            })
        });

        let mut subscriptions = Vec::with_capacity(3);
        for (index, input) in manual_inputs.iter().enumerate() {
            let model_focus = controls[index][AssignmentColumn::Model.index()].clone();
            subscriptions.push(cx.subscribe(input, move |this, _, event, cx| match event {
                RenameInputEvent::Changed => {
                    if this.manual_sync_suppress[index] {
                        this.manual_sync_suppress[index] = false;
                        return;
                    }
                    let text = this.manual_inputs[index].read(cx).text.clone();
                    cx.emit(AssignmentAction::Manual(index, text));
                }
                RenameInputEvent::Confirm | RenameInputEvent::Cancel => {
                    // The frozen text field has no submit action. Return focus to
                    // the row's model trigger; Escape must not close settings
                    // while the user is editing this field.
                    this.pending_focus = Some(model_focus.clone());
                    cx.notify();
                }
            }));
        }

        Self {
            controls,
            retry_focus,
            restore_focus,
            save_focus,
            manual_inputs,
            _manual_subscriptions: subscriptions,
            manual_edit: [false; 3],
            manual_sync_suppress: [false; 3],
            open_menu: None,
            trigger_bounds: Rc::new(RefCell::new([[None; 3]; 3])),
            menu_scroll: ScrollHandle::new(),
            pending_focus: None,
        }
    }

    /// Close the open menu and restore focus to its trigger.
    ///
    /// The settings shell calls this before its own Escape handling and before
    /// leaving the page. Returns whether a menu was actually open.
    pub(crate) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.open_menu.take() else {
            return false;
        };
        window.focus(&menu.trigger_focus);
        // A local-state change must repaint even when the parent does not
        // observe this non-rendered entity.
        window.refresh();
        cx.notify();
        true
    }

    #[cfg(any(test, debug_assertions))]
    pub(super) fn focus_first_provider(&self, window: &mut Window) {
        window.focus(&self.controls[0][AssignmentColumn::Provider.index()]);
    }

    /// Render one full assignment panel. The panel is supplied by the shell on
    /// every frame; menu, manual-field, and focus state live in this entity.
    pub(crate) fn render_panel(
        &mut self,
        panel: AssignmentPanel,
        keyboard_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(focus) = self.pending_focus.take() {
            window.focus(&focus);
        }

        let AssignmentPanel {
            rows,
            status,
            can_save,
            can_restore,
            saving,
            saved,
            save_error,
        } = panel;

        // A menu cannot survive a state transition that disables its controls.
        if self.open_menu.is_some() && (saving || status != AssignmentStatus::Ready) {
            self.close_menu(window, cx);
        }
        // Keep the trigger in the settings tab order while the menu is open.
        // Workspace handles Tab before the popup sees it.
        self.open_menu
            .take_if(|menu| !menu.trigger_focus.is_focused(window));

        let narrow = f32::from(window.viewport_size().width) <= VIEW_NARROW_BREAKPOINT;

        let header = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight(680.))
                    .text_color(rgb(TEXT))
                    .child(VIEW_TITLE),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child(VIEW_SUBTITLE),
            );

        let mut section = div().flex().flex_col().gap(px(10.)).child(header);

        if let Some(status_block) = self.status_block(status, keyboard_focus, cx) {
            section = section.child(status_block);
        }

        let mut card = div()
            .flex()
            .flex_col()
            .pt(px(2.))
            .px(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(LINE))
            .bg(rgb(PANEL_MUTED));

        if !narrow {
            card = card.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(VIEW_GRID_GAP))
                    .pt(px(10.))
                    .pb(px(8.))
                    .border_b_1()
                    .border_color(rgba(LINE))
                    .text_size(px(10.))
                    .font_weight(FontWeight(650.))
                    .text_color(rgb(MUTED))
                    .child(
                        div()
                            .w(px(VIEW_ROLE_WIDTH))
                            .flex_none()
                            .child(VIEW_HEAD_TIER),
                    )
                    .child(div().flex_1().min_w_0().child(VIEW_HEAD_PROVIDER))
                    .child(div().flex_1().min_w_0().child(VIEW_HEAD_MODEL))
                    .child(
                        div()
                            .w(px(VIEW_EFFORT_WIDTH))
                            .flex_none()
                            .child(VIEW_HEAD_EFFORT),
                    ),
            );
        }

        for (index, row) in rows.iter().enumerate() {
            if let Some(desired) = row.manual_model.as_deref() {
                self.sync_manual_input(index, desired, window, cx);
            }
            card = card.child(self.row_element(
                index,
                row,
                narrow,
                saving,
                keyboard_focus,
                window,
                cx,
            ));
        }

        section = section.child(card);
        section = section.child(
            div()
                .px(px(2.))
                .text_size(px(11.))
                .line_height(relative(1.65))
                .text_color(rgb(MUTED))
                .whitespace_normal()
                .child(VIEW_PRIVACY),
        );

        let mut actions = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(10.))
            .pt(px(2.))
            .pb(px(14.))
            .child(assignment_action_button(
                "claude-assignment-restore",
                VIEW_RESTORE,
                false,
                can_restore && !saving,
                self.restore_focus.clone(),
                keyboard_focus,
                cx,
                |this, _, cx| {
                    this.manual_edit = [false; 3];
                    this.manual_sync_suppress = [false; 3];
                    cx.emit(AssignmentAction::Restore);
                    cx.notify();
                },
            ))
            .child(assignment_action_button(
                "claude-assignment-save",
                if saving { VIEW_SAVING } else { VIEW_SAVE },
                true,
                can_save && !saving,
                self.save_focus.clone(),
                keyboard_focus,
                cx,
                |_, _, cx| {
                    cx.emit(AssignmentAction::Save);
                    cx.notify();
                },
            ));
        if saved {
            actions = actions.child(
                div()
                    .flex()
                    .items_center()
                    .px(px(9.))
                    .py(px(3.))
                    .rounded_full()
                    .bg(rgba(VIEW_SUCCESS_BG))
                    .text_size(px(11.))
                    .font_weight(FontWeight(600.))
                    .line_height(relative(1.5))
                    .text_color(rgb(VIEW_SUCCESS_TEXT))
                    .child(VIEW_SAVED),
            );
        }

        let mut footer = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgb(MUTED))
                    .child(VIEW_SCOPE),
            )
            .child(actions);
        if save_error {
            footer = footer.child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.55))
                    .text_color(rgb(VIEW_ERROR))
                    .child(VIEW_SAVE_ERROR),
            );
        }
        section = section.child(footer);

        let mut outer = div()
            .mt(px(22.))
            .pt(px(20.))
            .border_t_1()
            .border_color(rgba(LINE))
            .flex()
            .flex_col()
            .w_full()
            .child(section);

        if let Some(open) = self.open_menu.clone() {
            let bounds = (*self.trigger_bounds.borrow())[open.row][open.column.index()];
            if let Some(bounds) = bounds {
                outer = outer.child(self.render_menu_backdrop(window, cx));
                if let Some(menu) = self.render_menu(&open, bounds, window, cx) {
                    outer = outer.child(menu);
                }
            }
        }

        outer.into_any_element()
    }

    fn status_block(
        &self,
        status: AssignmentStatus,
        keyboard_focus: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match status {
            AssignmentStatus::Ready => None,
            AssignmentStatus::Loading => Some(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgb(MUTED))
                    .child(VIEW_LOADING)
                    .into_any_element(),
            ),
            AssignmentStatus::Disconnected => Some(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.))
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgb(VIEW_ERROR))
                    .child(VIEW_DISCONNECTED)
                    .child(self.retry_link(0, keyboard_focus, cx))
                    .into_any_element(),
            ),
            AssignmentStatus::Empty => Some(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgb(MUTED))
                    .child(VIEW_EMPTY)
                    .into_any_element(),
            ),
        }
    }

    fn row_element(
        &mut self,
        index: usize,
        row: &AssignmentRow,
        narrow: bool,
        saving: bool,
        keyboard_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let problem = row.problem.is_some();
        let provider = self.select_control(
            index,
            AssignmentColumn::Provider,
            &row.provider,
            problem,
            saving,
            keyboard_focus,
            window,
            cx,
        );
        let model = self.select_control(
            index,
            AssignmentColumn::Model,
            &row.model,
            problem,
            saving,
            keyboard_focus,
            window,
            cx,
        );
        let effort = self.select_control(
            index,
            AssignmentColumn::Effort,
            &row.effort,
            problem,
            saving,
            keyboard_focus,
            window,
            cx,
        );

        let manual_active = row.manual_model.is_some()
            || (self.manual_edit[index] && row.model.enabled)
            || row.model.selected == MANUAL_MODEL_SENTINEL;

        let mut provider_field = div()
            .flex()
            .flex_col()
            .gap(px(VIEW_FIELD_GAP))
            .min_w_0()
            .child(provider);
        if !row.account.is_empty() {
            provider_field = provider_field.child(
                div()
                    .text_size(px(10.))
                    .line_height(relative(1.5))
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child(row.account.clone()),
            );
        }

        let mut model_field = div()
            .flex()
            .flex_col()
            .gap(px(VIEW_FIELD_GAP))
            .min_w_0()
            .child(model);
        if manual_active {
            model_field = model_field.child(self.manual_field(index, row, saving, window, cx));
        }

        let effort_field = div()
            .flex()
            .flex_col()
            .gap(px(VIEW_FIELD_GAP))
            .min_w_0()
            .child(effort);

        let role_color = if problem { DANGER_TEXT } else { TEXT };

        let top = if narrow {
            div()
                .flex()
                .flex_col()
                .gap(px(9.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight(650.))
                        .line_height(relative(1.4))
                        .text_color(rgb(role_color))
                        .child(row.tier),
                )
                .child(assignment_labeled_field(
                    VIEW_HEAD_PROVIDER,
                    provider_field.into_any_element(),
                ))
                .child(assignment_labeled_field(
                    VIEW_HEAD_MODEL,
                    model_field.into_any_element(),
                ))
                .child(assignment_labeled_field(
                    VIEW_HEAD_EFFORT,
                    effort_field.into_any_element(),
                ))
        } else {
            div()
                .flex()
                .items_start()
                .gap(px(VIEW_GRID_GAP))
                .child(
                    div()
                        .w(px(VIEW_ROLE_WIDTH))
                        .flex_none()
                        .pt(px(9.))
                        .text_size(px(12.))
                        .font_weight(FontWeight(650.))
                        .line_height(relative(1.4))
                        .text_color(rgb(role_color))
                        .child(row.tier),
                )
                .child(provider_field.flex_1().min_w_0())
                .child(model_field.flex_1().min_w_0())
                .child(
                    div()
                        .w(px(VIEW_EFFORT_WIDTH))
                        .flex_none()
                        .child(effort_field),
                )
        };

        let mut meta = div().flex().flex_col().gap(px(4.)).min_w_0();
        let mut has_meta = false;
        if !row.target.is_empty() {
            meta = meta.child(
                div()
                    .text_size(px(10.))
                    .line_height(relative(1.55))
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child(row.target.clone()),
            );
            has_meta = true;
        }
        if let Some(problem) = row.problem.as_deref() {
            meta = meta.child(self.problem_line(
                index,
                problem,
                row.retry_catalog,
                keyboard_focus,
                cx,
            ));
            has_meta = true;
        }

        let mut container = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .py(px(VIEW_ROW_PADDING_Y));
        if index < 2 {
            container = container.border_b_1().border_color(rgba(LINE));
        }
        if problem {
            container = container.bg(color_alpha(DANGER_BG, 0.48));
        }
        container = container.child(top);
        if has_meta {
            if !narrow {
                meta = meta.ml(px(VIEW_ROLE_WIDTH + VIEW_GRID_GAP));
            }
            container = container.child(meta);
        }
        container.into_any_element()
    }

    fn problem_line(
        &self,
        index: usize,
        problem: &str,
        retry_catalog: bool,
        keyboard_focus: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut line = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(6.))
            .text_size(px(11.))
            .line_height(relative(1.55))
            .text_color(rgb(VIEW_ERROR))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .child(problem.to_owned()),
            );
        if retry_catalog {
            line = line.child(self.retry_link(index + 1, keyboard_focus, cx));
        }
        line.into_any_element()
    }

    fn select_control(
        &mut self,
        row: usize,
        column: AssignmentColumn,
        select: &AssignmentSelect,
        problem: bool,
        saving: bool,
        keyboard_focus: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = select.enabled && !saving;
        let focus = self.controls[row][column.index()].clone();
        let open_here = self
            .open_menu
            .as_ref()
            .is_some_and(|menu| menu.row == row && menu.column == column);
        let label = assignment_display_label(select);
        let border = field_border(problem, open_here || focus.is_focused(window));

        let mut trigger = div()
            .id(SharedString::from(format!(
                "claude-assignment-{row}-{}",
                column.index()
            )))
            .w_full()
            .h(px(34.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .px(px(9.))
            .rounded(px(7.))
            .border_1()
            .border_color(border)
            .bg(rgb(PANEL))
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .child(icon("chevron-down", 12., MUTED));

        let bounds_cell = self.trigger_bounds.clone();
        trigger = trigger.child(
            canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    bounds_cell.borrow_mut()[row][column.index()] = Some(bounds);
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        );

        if !enabled {
            return trigger.opacity(0.55).into_any_element();
        }

        let click_select = select.clone();
        let key_select = select.clone();
        trigger = trigger
            .track_focus(&focus)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, {
                let focus = focus.clone();
                move |_, window, cx| {
                    window.focus(&focus);
                    cx.stop_propagation();
                }
            })
            .on_click(cx.listener(move |this, event, window, cx| {
                if matches!(event, ClickEvent::Keyboard(_)) {
                    return;
                }
                cx.stop_propagation();
                this.toggle_select_menu(row, column, &click_select, window, cx);
            }))
            .on_key_down(cx.listener(move |this, event, window, cx| {
                this.select_control_key(row, column, &key_select, event, window, cx);
            }))
            .child(focus_ring_with(
                focus,
                7.,
                keyboard_focus,
                FocusRingStyle::Shell,
            ));
        trigger.into_any_element()
    }

    fn manual_field(
        &self,
        index: usize,
        row: &AssignmentRow,
        saving: bool,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        let input = self.manual_inputs[index].clone();
        let focus = input.read(cx).focus.clone();
        let invalid = row.problem.is_some();

        if saving {
            let text = row
                .manual_model
                .clone()
                .unwrap_or_else(|| input.read(cx).text.clone());
            return div()
                .flex()
                .items_center()
                .w_full()
                .h(px(34.))
                .px(px(9.))
                .rounded(px(7.))
                .border_1()
                .border_color(field_border(invalid, false))
                .bg(rgb(PANEL))
                .text_size(px(12.))
                .text_color(rgb(TEXT))
                .opacity(0.55)
                .child(text)
                .into_any_element();
        }

        let focused = focus.is_focused(window);
        let field = div()
            .flex()
            .items_center()
            .w_full()
            .h(px(34.))
            .px(px(9.))
            .rounded(px(7.))
            .border_1()
            .border_color(field_border(invalid, focused))
            .bg(rgb(PANEL))
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, {
                let focus = focus.clone();
                move |_, window, cx| {
                    window.focus(&focus);
                    cx.stop_propagation();
                }
            })
            .child(input);
        field.into_any_element()
    }

    /// Copy a panel-owned manual value into the editor without fighting the
    /// user's cursor. The editor exposes no selection setter, so the write is
    /// performed through its input handler after the current render and only
    /// while the field is unfocused.
    fn sync_manual_input(
        &self,
        index: usize,
        desired: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.manual_inputs[index].clone();
        let current = input.read(cx);
        if current.text == desired || current.focus.is_focused(window) {
            return;
        }

        let desired = desired.to_owned();
        let view = cx.entity();
        window.defer(cx, move |window, cx| {
            let current = input.read(cx);
            if current.focus.is_focused(window) || current.text == desired {
                return;
            }
            let len = current.text.encode_utf16().count();
            view.update(cx, |this, _| this.manual_sync_suppress[index] = true);
            input.update(cx, |input, cx| {
                input.replace_text_in_range(Some(0..len), &desired, window, cx);
            });
        });
    }

    fn seed_manual_input(
        &self,
        index: usize,
        seed: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.manual_inputs[index].clone();
        if input.read(cx).text == seed {
            return;
        }
        let len = input.read(cx).text.encode_utf16().count();
        input.update(cx, |input, cx| {
            input.replace_text_in_range(Some(0..len), seed, window, cx);
        });
    }

    fn toggle_select_menu(
        &mut self,
        row: usize,
        column: AssignmentColumn,
        select: &AssignmentSelect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .open_menu
            .as_ref()
            .is_some_and(|menu| menu.row == row && menu.column == column)
        {
            self.close_menu(window, cx);
        } else {
            self.open_select_menu(row, column, select, window, cx);
        }
    }

    fn select_control_key(
        &mut self,
        row: usize,
        column: AssignmentColumn,
        select: &AssignmentSelect,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open_here = self
            .open_menu
            .as_ref()
            .is_some_and(|menu| menu.row == row && menu.column == column);
        if open_here {
            self.menu_key(event, window, cx);
            return;
        }
        let key = event.keystroke.key.as_str();
        if matches!(key, "enter" | "space" | "down") && !event.keystroke.modifiers.modified() {
            self.open_select_menu(row, column, select, window, cx);
            cx.stop_propagation();
        }
    }

    fn open_select_menu(
        &mut self,
        row: usize,
        column: AssignmentColumn,
        select: &AssignmentSelect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !select.enabled || select.items.is_empty() {
            return;
        }
        let active = select
            .items
            .iter()
            .position(|item| item.value == select.selected)
            .unwrap_or(0);
        let trigger_focus = self.controls[row][column.index()].clone();
        window.focus(&trigger_focus);
        self.open_menu = Some(OpenAssignmentMenu {
            row,
            column,
            active,
            trigger_focus,
            items: select.items.clone(),
            selected: select.selected.clone(),
        });
        self.menu_scroll.scroll_to_item(active);
        window.refresh();
        cx.notify();
    }

    fn menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((len, active, row, column)) = self
            .open_menu
            .as_ref()
            .map(|menu| (menu.items.len(), menu.active, menu.row, menu.column))
        else {
            return;
        };
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => {
                self.close_menu(window, cx);
                cx.stop_propagation();
            }
            "tab" => {
                // Restore the trigger and let the window move focus. Do not
                // stop propagation: Tab is a focus order operation, not a menu
                // selection.
                self.close_menu(window, cx);
            }
            "down" => {
                let next = if len == 0 { 0 } else { (active + 1) % len };
                self.set_menu_active(next, window, cx);
                cx.stop_propagation();
            }
            "up" => {
                let next = if len == 0 {
                    0
                } else {
                    (active + len - 1) % len
                };
                self.set_menu_active(next, window, cx);
                cx.stop_propagation();
            }
            "home" => {
                self.set_menu_active(0, window, cx);
                cx.stop_propagation();
            }
            "end" => {
                self.set_menu_active(len.saturating_sub(1), window, cx);
                cx.stop_propagation();
            }
            "enter" | "space" => {
                let value = self
                    .open_menu
                    .as_ref()
                    .and_then(|menu| menu.items.get(menu.active))
                    .map(|item| item.value.clone());
                if let Some(value) = value {
                    self.select_option(row, column, value, window, cx);
                }
                cx.stop_propagation();
            }
            _ => {
                // A list popup is modal for plain keys; never let a stray
                // shortcut fire behind it while it is open.
                cx.stop_propagation();
            }
        }
    }

    fn set_menu_active(&mut self, active: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.open_menu.as_mut() else {
            return;
        };
        if menu.items.is_empty() {
            return;
        }
        menu.active = active.min(menu.items.len() - 1);
        let active = menu.active;
        self.menu_scroll.scroll_to_item(active);
        window.refresh();
        cx.notify();
    }

    fn select_option(
        &mut self,
        row: usize,
        column: AssignmentColumn,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = self.open_menu.take();
        let selected_before = menu
            .as_ref()
            .map(|menu| menu.selected.clone())
            .unwrap_or_default();

        match column {
            AssignmentColumn::Provider => {
                self.manual_edit[row] = false;
                self.manual_sync_suppress[row] = false;
                if let Some(menu) = &menu {
                    window.focus(&menu.trigger_focus);
                }
                cx.emit(AssignmentAction::Provider(row, value));
            }
            AssignmentColumn::Model => {
                let manual = value == MANUAL_MODEL_SENTINEL;
                cx.emit(AssignmentAction::Model(row, value));
                if manual {
                    self.manual_sync_suppress[row] = false;
                    let was_manual =
                        self.manual_edit[row] || selected_before == MANUAL_MODEL_SENTINEL;
                    self.manual_edit[row] = true;
                    if !was_manual {
                        // The frozen manual field starts from the model that was
                        // selected before the sentinel (or empty for an unset
                        // row). Clearing is important: a previous manual value
                        // must not leak into a fresh selection.
                        let seed = if selected_before.is_empty()
                            || selected_before == MANUAL_MODEL_SENTINEL
                        {
                            String::new()
                        } else {
                            selected_before
                        };
                        self.seed_manual_input(row, &seed, window, cx);
                    }
                    let focus = self.manual_inputs[row].read(cx).focus.clone();
                    window.focus(&focus);
                } else {
                    self.manual_edit[row] = false;
                    self.manual_sync_suppress[row] = false;
                    if let Some(menu) = &menu {
                        window.focus(&menu.trigger_focus);
                    }
                }
            }
            AssignmentColumn::Effort => {
                if let Some(menu) = &menu {
                    window.focus(&menu.trigger_focus);
                }
                cx.emit(AssignmentAction::Effort(row, value));
            }
        }
        window.refresh();
        cx.notify();
    }

    fn retry_link(&self, slot: usize, keyboard_focus: bool, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.retry_focus[slot].clone();
        assignment_link_button(
            SharedString::from(format!("claude-assignment-retry-{slot}")),
            VIEW_RETRY,
            focus,
            keyboard_focus,
            cx,
            |_, _, cx| {
                cx.emit(AssignmentAction::Retry);
                cx.notify();
            },
        )
    }

    fn render_menu_backdrop(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let viewport = window.viewport_size();
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("claude-assignment-menu-backdrop")
                    .w(viewport.width)
                    .h(viewport.height)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.close_menu(window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
            ),
        )
        .with_priority(100)
        .into_any_element()
    }

    fn render_menu(
        &self,
        open: &OpenAssignmentMenu,
        bounds: Bounds<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let viewport = window.viewport_size();
        let width = f32::from(bounds.size.width)
            .max(open.column.menu_min_width())
            .min((f32::from(viewport.width) - 16.).max(120.));
        let max_height = (f32::from(viewport.height) - 16.).min(300.).max(96.);

        let mut menu = div()
            .id("claude-assignment-menu")
            .w(px(width))
            .max_h(px(max_height))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(LINE))
            .bg(rgb(PANEL))
            .shadow(vec![shadow(0x1a304b, 0.16, 8., 24.)])
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

        for (index, option) in open.items.iter().enumerate() {
            let selected = option.value == open.selected;
            let active = index == open.active;
            let background = if selected {
                VIEW_MENU_SELECTED_BG
            } else if active {
                VIEW_MENU_ACTIVE_BG
            } else {
                PANEL
            };
            let label = if option.label.is_empty() {
                option.value.clone()
            } else {
                option.label.clone()
            };
            let value = option.value.clone();
            let row = open.row;
            let column = open.column;
            let item = div()
                .id(SharedString::from(format!(
                    "claude-assignment-option-{row}-{}-{index}",
                    column.index()
                )))
                .flex_none()
                .h(px(30.))
                .flex()
                .items_center()
                .px(px(9.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(rgb(if selected {
                    VIEW_MENU_SELECTED_TEXT
                } else {
                    TEXT
                }))
                .bg(rgb(background))
                .hover(|style| style.bg(rgb(VIEW_MENU_SELECTED_BG)))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, event, window, cx| {
                    if matches!(event, ClickEvent::Keyboard(_)) {
                        return;
                    }
                    cx.stop_propagation();
                    this.select_option(row, column, value.clone(), window, cx);
                }))
                .child(label);
            menu = menu.child(item);
        }

        Some(
            deferred(
                anchored()
                    .position(point(bounds.left(), bounds.bottom() + px(4.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(menu),
            )
            .with_priority(101)
            .into_any_element(),
        )
    }
}

fn field_border(problem: bool, focused: bool) -> Rgba {
    if problem {
        rgb(VIEW_ERROR)
    } else if focused {
        rgb(ACCENT)
    } else {
        rgba(LINE_STRONG)
    }
}

fn assignment_display_label(select: &AssignmentSelect) -> String {
    if !select.label.is_empty() {
        return select.label.clone();
    }
    if let Some(item) = select
        .items
        .iter()
        .find(|item| item.value == select.selected)
    {
        return item.label.clone();
    }
    select.selected.clone()
}

fn assignment_labeled_field(label: &'static str, field: AnyElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight(650.))
                .text_color(rgb(MUTED))
                .child(label),
        )
        .child(field)
}

fn assignment_action_button<F>(
    id: &'static str,
    label: &'static str,
    primary: bool,
    enabled: bool,
    focus: FocusHandle,
    keyboard_focus: bool,
    cx: &mut Context<AssignmentView>,
    activate: F,
) -> AnyElement
where
    F: Fn(&mut AssignmentView, &mut Window, &mut Context<AssignmentView>) + 'static,
{
    let base = div()
        .id(id)
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .px(px(14.))
        .rounded(px(7.))
        .border_1()
        .text_size(px(11.))
        .font_weight(FontWeight(650.))
        .whitespace_nowrap()
        .child(label);

    let button = if primary {
        base.border_color(rgb(ACCENT))
            .text_color(rgb(0xffffff))
            .bg(rgb(ACCENT))
    } else {
        base.border_color(rgba(LINE_STRONG))
            .text_color(rgb(TEXT))
            .bg(rgb(PANEL))
    };

    if !enabled {
        return button
            .opacity(0.5)
            .cursor(CursorStyle::Arrow)
            .into_any_element();
    }

    let button = if primary {
        button
            .hover(|style| style.bg(rgb(0x2f79df)))
            .cursor_pointer()
    } else {
        button
            .hover(|style| {
                style
                    .border_color(rgb(ACCENT))
                    .text_color(rgb(ACCENT))
                    .bg(rgb(0xf7faff))
            })
            .cursor_pointer()
    };

    let activate: Rc<dyn Fn(&mut AssignmentView, &mut Window, &mut Context<AssignmentView>)> =
        Rc::new(activate);
    let key_activate = activate.clone();
    let click_activate = activate;
    let mouse_focus = focus.clone();
    button
        .relative()
        .track_focus(&focus)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&mouse_focus);
            cx.stop_propagation();
        })
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                && !event.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
                key_activate(this, window, cx);
            }
        }))
        .on_click(cx.listener(move |this, event, window, cx| {
            if matches!(event, ClickEvent::Keyboard(_)) {
                return;
            }
            cx.stop_propagation();
            click_activate(this, window, cx);
        }))
        .child(focus_ring_with(
            focus,
            7.,
            keyboard_focus,
            FocusRingStyle::Shell,
        ))
        .into_any_element()
}

fn assignment_link_button<F>(
    id: SharedString,
    label: &'static str,
    focus: FocusHandle,
    keyboard_focus: bool,
    cx: &mut Context<AssignmentView>,
    activate: F,
) -> AnyElement
where
    F: Fn(&mut AssignmentView, &mut Window, &mut Context<AssignmentView>) + 'static,
{
    let activate: Rc<dyn Fn(&mut AssignmentView, &mut Window, &mut Context<AssignmentView>)> =
        Rc::new(activate);
    let key_activate = activate.clone();
    let click_activate = activate;
    let mouse_focus = focus.clone();
    div()
        .id(id)
        .h(px(18.))
        .flex()
        .items_center()
        .px(px(2.))
        .text_size(px(11.))
        .font_weight(FontWeight(600.))
        .text_color(rgb(ACCENT))
        .cursor_pointer()
        .hover(|style| style.text_color(rgb(0x1f6dc4)))
        .relative()
        .track_focus(&focus)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&mouse_focus);
            cx.stop_propagation();
        })
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                && !event.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
                key_activate(this, window, cx);
            }
        }))
        .on_click(cx.listener(move |this, event, window, cx| {
            if matches!(event, ClickEvent::Keyboard(_)) {
                return;
            }
            cx.stop_propagation();
            click_activate(this, window, cx);
        }))
        .child(label)
        .child(focus_ring_with(
            focus,
            4.,
            keyboard_focus,
            FocusRingStyle::Shell,
        ))
        .into_any_element()
}

// Assignment preferences and runtime replies.

use std::sync::atomic::{AtomicU64, Ordering};

use matou_native_core::provider_launch::{ClaudeRoleAssignment, ClaudeRoleAssignments, EFFORTS};
use matou_native_runtime::protocol::ClaudeRoleAssignmentsReply;
use serde_json::Value;

use super::effort_name;
use crate::{
    provider_settings::ProviderSettings,
    rename_input::trim_name,
    runtime_bridge::{Command, ConnectionPhase, RuntimeCommands},
};

const ROLES: [&str; 3] = ["opus", "sonnet", "haiku"];
const TIERS: [&str; 3] = ["Opus", "Sonnet", "Haiku"];
const FOLLOW_PROVIDER: &str = "沿用默认供应商";
const CHOOSE_MODEL: &str = "请选择模型";
static NEXT_ASSIGNMENT_REQUEST: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Problem {
    Provider,
    Key,
    Account,
    Empty,
    Model,
    Catalog,
    Invalid,
    Effort,
}

impl Problem {
    fn text(self) -> &'static str {
        match self {
            Self::Provider => "供应商已不存在；原选择已保留，请重新选择。",
            Self::Key => "API Key 未配置，请先编辑供应商。",
            Self::Account => "账号已失效，请重新登录或选择其他供应商。",
            Self::Empty => "请选择或输入模型。",
            Self::Model => "模型已不可用；原选择已保留，请重新选择。",
            Self::Catalog => "模型目录暂不可用，请重试。",
            Self::Invalid => "模型 ID 无效，请检查输入。",
            Self::Effort => "当前模型不支持所选推理强度；原选择已保留，请重新选择。",
        }
    }
}

fn subscription(provider: &Value) -> bool {
    provider["subscription"].is_object()
}

fn official(provider: &Value) -> bool {
    provider["builtIn"] == true
        && provider.get("modelMetadata").is_none()
        && provider["endpoint"].as_str().is_some_and(|endpoint| {
            endpoint.strip_suffix('/').unwrap_or(endpoint) == "https://api.anthropic.com"
        })
}

fn catalog(provider: &Value) -> Option<&[Value]> {
    provider["subscription"]
        .get("modelMetadata")
        .or_else(|| provider.get("modelMetadata"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

fn model_metadata<'a>(models: &'a [Value], id: &str) -> Option<&'a Value> {
    models.iter().find(|model| model["id"] == id)
}

fn model_name<'a>(models: &'a [Value], id: &'a str) -> &'a str {
    model_metadata(models, id)
        .and_then(|model| model["displayName"].as_str())
        .unwrap_or(id)
}

fn inherited_effort<'a>(provider: &'a Value, role: &str) -> &'a str {
    provider["effortMapping"][role]
        .as_str()
        .or_else(|| provider["reasoningEffort"].as_str())
        .filter(|effort| EFFORTS.contains(effort))
        .unwrap_or("follow")
}

fn effort_choices(provider: &Value, model: &str) -> Vec<&'static str> {
    let mut choices = vec!["follow"];
    if let Some(supported) = model_metadata(catalog(provider).unwrap_or_default(), model)
        .and_then(|metadata| metadata["supportedEfforts"].as_array())
    {
        for level in supported.iter().filter_map(Value::as_str) {
            if let Some(&effort) = EFFORTS.iter().find(|&&effort| effort == level)
                && !choices.contains(&effort)
            {
                choices.push(effort);
            }
        }
        return choices;
    }

    let label = format!(
        "{} {} {model}",
        provider["id"].as_str().unwrap_or_default(),
        provider["name"].as_str().unwrap_or_default()
    )
    .to_ascii_lowercase();
    let gpt_family = label
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|word| {
            matches!(word, "gpt" | "codex") || matches!(word.as_bytes(), [b'o', b'1'..=b'9'])
        });
    let claude_family = !subscription(provider)
        || (provider["subscription"]["provider"] != "codex"
            && ["claude", "opus", "sonnet", "haiku"]
                .iter()
                .any(|role| label.contains(role)));
    choices.extend_from_slice(&EFFORTS[1..5]);
    if !gpt_family && claude_family {
        choices.push("max");
    }
    choices
}

fn row_problem(
    provider: Option<&Value>,
    model: &str,
    effort: &str,
    assigned: bool,
) -> Option<Problem> {
    let Some(provider) = provider else {
        return Some(Problem::Provider);
    };
    if subscription(provider) {
        return Some(Problem::Account);
    }
    if !subscription(provider) && !official(provider) && provider["hasApiKey"] != true {
        return Some(Problem::Key);
    }

    // The native runtime does not expose the dynamic official model directory.
    if official(provider) && (assigned || model.is_empty()) {
        return Some(Problem::Catalog);
    }
    let id = trim_name(model);
    if id.is_empty() {
        return Some(if assigned {
            Problem::Empty
        } else {
            Problem::Catalog
        });
    }
    if model.encode_utf16().count() > 240 || model.chars().any(|ch| ch.is_ascii_control()) {
        return Some(Problem::Invalid);
    }
    if let Some(models) = catalog(provider)
        && model_metadata(models, id).is_none()
    {
        return Some(Problem::Model);
    }
    (!effort_choices(provider, id).contains(&effort)).then_some(Problem::Effort)
}

fn account_caption(provider: Option<&Value>, assigned: bool) -> String {
    if !assigned {
        let name = provider
            .and_then(|provider| provider["name"].as_str())
            .unwrap_or("默认供应商不可用");
        return format!("{name} · 默认供应商");
    }
    let Some(provider) = provider else {
        return String::new();
    };
    if subscription(provider) {
        return String::new();
    }
    let endpoint = provider["endpoint"].as_str().unwrap_or_default();
    endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))
        .unwrap_or(endpoint)
        .to_owned()
}

fn option(value: impl Into<String>, label: impl Into<String>) -> AssignmentOption {
    AssignmentOption {
        value: value.into(),
        label: label.into(),
    }
}

fn available_models(provider: &Value) -> Vec<AssignmentOption> {
    if let Some(models) = catalog(provider) {
        return models
            .iter()
            .filter_map(|model| {
                let id = model["id"].as_str()?;
                Some(option(id, model["displayName"].as_str().unwrap_or(id)))
            })
            .collect();
    }
    let mut models = Vec::new();
    if !official(provider) && !subscription(provider) {
        let ids = std::iter::once(provider["model"].as_str()).chain(
            ROLES
                .into_iter()
                .map(|role| provider["modelMapping"][role].as_str()),
        );
        for id in ids.flatten().filter(|id| !matches!(*id, "" | "CLI 默认")) {
            if !models
                .iter()
                .any(|model: &AssignmentOption| model.value == id)
            {
                models.push(option(id, id));
            }
        }
    }
    models
}

fn allows_manual(provider: &Value) -> bool {
    !subscription(provider) && provider["builtIn"] != true && catalog(provider).is_none()
}

fn select(items: Vec<AssignmentOption>, selected: &str, enabled: bool) -> AssignmentSelect {
    let label = items
        .iter()
        .find(|item| item.value == selected)
        .map(|item| item.label.clone())
        .unwrap_or_default();
    AssignmentSelect {
        label,
        enabled,
        items,
        selected: selected.into(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssignmentRequest {
    Load,
    Save,
}

struct PendingAssignment {
    id: u64,
    generation: u64,
    operation: AssignmentRequest,
}

pub(crate) struct ClaudeAssignments {
    commands: RuntimeCommands,
    pending: Option<PendingAssignment>,
    baseline: Option<ClaudeRoleAssignments>,
    draft: ClaudeRoleAssignments,
    manual: [bool; 3],
    load_error: bool,
    save_error: bool,
    saved: bool,
}

impl ClaudeAssignments {
    pub(crate) fn new(commands: RuntimeCommands) -> Self {
        Self {
            commands,
            pending: None,
            baseline: None,
            draft: ClaudeRoleAssignments::new(),
            manual: [false; 3],
            load_error: false,
            save_error: false,
            saved: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.commands.clone());
    }

    fn send(
        &mut self,
        operation: AssignmentRequest,
        command: impl FnOnce(u64) -> Command,
    ) -> Option<u64> {
        let (generation, phase) = self.commands.connection();
        if self.pending.is_some() || phase != ConnectionPhase::Ready {
            return None;
        }
        let id = NEXT_ASSIGNMENT_REQUEST.fetch_add(1, Ordering::Relaxed);
        self.commands.send_at(generation, command(id)).ok()?;
        self.pending = Some(PendingAssignment {
            id,
            generation,
            operation,
        });
        Some(id)
    }

    pub(crate) fn load(&mut self) -> Option<u64> {
        if self.pending.is_some() {
            return None;
        }
        let request = self.send(AssignmentRequest::Load, |request_id| {
            Command::GetClaudeRoleAssignments { request_id }
        });
        self.load_error = request.is_none();
        request
    }

    pub(crate) fn expire(&mut self, request_id: u64) -> bool {
        let Some(pending) = self.pending.take_if(|pending| pending.id == request_id) else {
            return false;
        };
        self.fail(pending.operation);
        true
    }

    pub(crate) fn saving(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.operation == AssignmentRequest::Save)
    }

    fn dirty(&self) -> bool {
        self.baseline
            .as_ref()
            .is_some_and(|baseline| baseline != &self.draft)
    }

    pub(crate) fn receive(&mut self, reply: ClaudeRoleAssignmentsReply) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if pending.id != reply.request_id
            || self.commands.connection() != (pending.generation, ConnectionPhase::Ready)
        {
            return;
        }
        let pending = self.pending.take().expect("matching assignment request");
        let Some(assignments) = reply.assignments.filter(|_| reply.error.is_none()) else {
            self.fail(pending.operation);
            return;
        };
        let saved = pending.operation == AssignmentRequest::Save;
        if saved || !self.dirty() {
            self.draft = assignments.clone();
            self.manual = [false; 3];
        }
        self.baseline = Some(assignments);
        self.load_error = false;
        if saved {
            self.saved = true;
            self.save_error = false;
        }
    }

    pub(crate) fn connection_changed(&mut self, visible: bool) -> Option<u64> {
        if !visible {
            return None;
        }
        let (generation, phase) = self.commands.connection();
        if self.pending.as_ref().is_some_and(|pending| {
            pending.generation != generation || phase != ConnectionPhase::Ready
        }) {
            let pending = self.pending.take().expect("interrupted assignment request");
            self.fail(pending.operation);
            self.load_error = true;
        }
        if phase != ConnectionPhase::Ready {
            self.load_error = true;
        } else if self.pending.is_none() && self.load_error {
            return self.load();
        }
        None
    }

    fn fail(&mut self, operation: AssignmentRequest) {
        match operation {
            AssignmentRequest::Load => self.load_error = true,
            AssignmentRequest::Save => self.save_error = true,
        }
    }

    fn status(&self, providers: &ProviderSettings) -> AssignmentStatus {
        if self.commands.connection().1 != ConnectionPhase::Ready
            || self.load_error
            || providers.load_error().is_some()
        {
            AssignmentStatus::Disconnected
        } else if self.baseline.is_none()
            || providers.loading()
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.operation == AssignmentRequest::Load)
        {
            AssignmentStatus::Loading
        } else if providers.providers().is_empty() {
            AssignmentStatus::Empty
        } else {
            AssignmentStatus::Ready
        }
    }

    pub(crate) fn panel(&self, providers: &ProviderSettings) -> AssignmentPanel {
        let status = self.status(providers);
        let saving = self.saving();
        let enabled = matches!(status, AssignmentStatus::Ready) && !saving;
        let rows = std::array::from_fn(|index| self.row(index, providers, enabled));
        let can_save = enabled && self.dirty() && rows.iter().all(|row| row.problem.is_none());
        AssignmentPanel {
            rows,
            status,
            can_save,
            can_restore: self.baseline.is_some() && !saving,
            saving,
            saved: self.saved,
            save_error: self.save_error,
        }
    }

    fn row(&self, index: usize, providers: &ProviderSettings, enabled: bool) -> AssignmentRow {
        let role = ROLES[index];
        let tier = TIERS[index];
        let assigned = self.draft.get(role);
        let provider_id = assigned.map(|row| row.provider_id.as_str());
        let effective_id = provider_id.or(providers.active_id());
        let provider = providers.providers().iter().find(|provider| {
            provider["id"]
                .as_str()
                .is_some_and(|id| Some(id) == effective_id)
        });
        let provider_value = provider.unwrap_or(&Value::Null);
        let provider_name = provider_value["name"]
            .as_str()
            .or(provider_id)
            .unwrap_or("—");
        let models = catalog(provider_value).unwrap_or_default();
        let mapped = provider_value["modelMapping"][role]
            .as_str()
            .filter(|id| !id.is_empty());
        let selected = assigned.map(|row| row.model.as_str()).unwrap_or_default();
        let effective_model = assigned.map(|_| selected).or(mapped).unwrap_or_default();
        let effort = assigned
            .and_then(|row| row.effort.as_deref())
            .unwrap_or_else(|| inherited_effort(provider_value, role));
        let manual_allowed = provider.is_some_and(allows_manual);
        let choices = available_models(provider_value);
        let unavailable_model =
            !selected.is_empty() && !choices.iter().any(|model| model.value == selected);
        let manual = manual_allowed && (self.manual[index] || unavailable_model);
        let problem = if self.draft.is_empty() {
            None
        } else {
            row_problem(provider, effective_model, effort, assigned.is_some())
        };
        let selectable = enabled
            && assigned.is_some()
            && provider.is_some_and(|provider| !subscription(provider));

        let mut provider_items = vec![option("", FOLLOW_PROVIDER)];
        if let Some(id) = provider_id.filter(|_| provider.is_none_or(subscription)) {
            provider_items.push(option(id, format!("{id} · 不可用")));
        }
        provider_items.extend(providers.providers().iter().filter_map(|provider| {
            if subscription(provider) {
                return None;
            }
            let id = provider["id"].as_str()?;
            Some(option(id, provider["name"].as_str().unwrap_or(id)))
        }));
        let mut model_items = vec![option("", CHOOSE_MODEL)];
        let model_selection = if assigned.is_none() {
            model_items[0].label = mapped
                .map(|id| model_name(models, id).to_owned())
                .unwrap_or_else(|| format!("{tier} 原生默认档位（目录暂不可用）"));
            ""
        } else {
            if unavailable_model && !manual {
                model_items.push(option(selected, format!("{selected} · 不可用")));
            }
            model_items.extend(choices);
            if manual_allowed {
                model_items.push(option(MANUAL_MODEL_SENTINEL, "手动填写模型 ID…"));
            }
            if manual {
                MANUAL_MODEL_SENTINEL
            } else {
                selected
            }
        };
        let mut efforts: Vec<_> = effort_choices(provider_value, effective_model)
            .into_iter()
            .map(|level| option(level, effort_name(level)))
            .collect();
        if !efforts.iter().any(|item| item.value == effort) {
            efforts.push(option(
                effort,
                format!("{} （不支持）", effort_name(effort)),
            ));
        }
        if assigned.is_none() {
            efforts = vec![option(effort, effort_name(effort))];
        }
        let target_model = if assigned.is_some() || mapped.is_some() {
            model_name(models, effective_model)
        } else {
            "原生默认档位待确认"
        };
        let prefix = if assigned.is_some() {
            "当前目标："
        } else {
            "最终："
        };
        let mut target = format!("{prefix}{provider_name} · {target_model}");
        if effort != "follow" {
            target.push_str(" · ");
            target.push_str(effort_name(effort));
        }
        let problem_text = problem.map(|problem| {
            if assigned.is_some() {
                problem.text().to_owned()
            } else {
                let id = mapped.unwrap_or(tier);
                format!(
                    "沿用默认：{provider_name} · {id}。{}请为此档位重新选择供应商和模型。",
                    problem.text()
                )
            }
        });
        AssignmentRow {
            tier,
            provider: select(provider_items, provider_id.unwrap_or_default(), enabled),
            account: account_caption(provider, assigned.is_some()),
            model: select(model_items, model_selection, selectable),
            effort: select(efforts, effort, selectable),
            manual_model: manual.then(|| selected.to_owned()),
            target,
            problem: problem_text,
            retry_catalog: problem == Some(Problem::Catalog),
        }
    }

    pub(crate) fn change(&mut self, action: &AssignmentAction, providers: &ProviderSettings) {
        if self.saving() {
            return;
        }
        if matches!(action, AssignmentAction::Restore) && self.baseline.is_some() {
            self.draft.clear();
            self.manual = [false; 3];
        } else if !matches!(self.status(providers), AssignmentStatus::Ready) {
            return;
        } else {
            match action {
                AssignmentAction::Provider(index, id) => {
                    let role = ROLES[*index];
                    if self
                        .draft
                        .get(role)
                        .map_or("", |row| row.provider_id.as_str())
                        == id
                    {
                        return;
                    }
                    self.manual[*index] = false;
                    if id.is_empty() {
                        self.draft.remove(role);
                    } else {
                        self.draft.insert(
                            role.into(),
                            ClaudeRoleAssignment {
                                provider_id: id.clone(),
                                model: String::new(),
                                effort: None,
                            },
                        );
                    }
                }
                AssignmentAction::Model(index, model) => {
                    if let Some(row) = self.draft.get_mut(ROLES[*index]) {
                        self.manual[*index] = model == MANUAL_MODEL_SENTINEL
                            && providers
                                .providers()
                                .iter()
                                .find(|provider| provider["id"] == row.provider_id)
                                .is_some_and(allows_manual);
                        if self.manual[*index] || row.model == *model {
                            return;
                        }
                        row.model.clone_from(model);
                    }
                }
                AssignmentAction::Manual(index, model) => {
                    if let Some(row) = self.draft.get_mut(ROLES[*index]) {
                        if row.model == *model {
                            return;
                        }
                        self.manual[*index] = true;
                        row.model.clone_from(model);
                    }
                }
                AssignmentAction::Effort(index, effort) => {
                    let role = ROLES[*index];
                    if let Some(row) = self.draft.get_mut(role) {
                        let inherited = providers
                            .providers()
                            .iter()
                            .find(|provider| provider["id"] == row.provider_id)
                            .map_or("follow", |provider| inherited_effort(provider, role));
                        row.effort = if effort == "follow" && inherited == "follow" {
                            None
                        } else {
                            Some(effort.clone())
                        };
                    }
                }
                _ => return,
            }
        }
        self.saved = false;
        self.save_error = false;
    }

    pub(crate) fn save(&mut self, providers: &ProviderSettings) -> Option<u64> {
        if !self.panel(providers).can_save {
            return None;
        }
        self.saved = false;
        let request = serde_json::to_value(&self.draft)
            .ok()
            .and_then(|assignments| {
                self.send(AssignmentRequest::Save, |request_id| {
                    Command::SetClaudeRoleAssignments {
                        request_id,
                        assignments,
                    }
                })
            });
        self.save_error = request.is_none();
        request
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn diagnostics(&self) -> (bool, bool, bool, bool) {
        (self.dirty(), self.saving(), self.saved, self.save_error)
    }
}

#[cfg(test)]
mod assignment_validation_tests {
    use super::{Problem, allows_manual, available_models, effort_choices, row_problem};
    use serde_json::json;

    #[test]
    fn manual_model_requires_a_concrete_id_and_keeps_frozen_validation_order() {
        let mut custom = json!({
            "id": "custom", "model": "CLI 默认", "hasApiKey": true,
            "modelMapping": {"opus": "custom-opus", "sonnet": "custom-opus"},
        });
        assert!(allows_manual(&custom));
        assert_eq!(
            available_models(&custom)
                .iter()
                .map(|model| model.value.as_str())
                .collect::<Vec<_>>(),
            vec!["custom-opus"]
        );
        assert_eq!(
            row_problem(Some(&custom), "", "follow", true),
            Some(Problem::Empty)
        );
        assert_eq!(
            row_problem(Some(&custom), "", "follow", false),
            Some(Problem::Catalog)
        );
        assert_eq!(
            row_problem(Some(&custom), "actual-model-id", "follow", true),
            None
        );
        assert_eq!(
            row_problem(Some(&custom), "bad\tmodel", "follow", true),
            Some(Problem::Invalid)
        );
        assert_eq!(
            row_problem(Some(&custom), &"🙂".repeat(121), "follow", true),
            Some(Problem::Invalid)
        );
        custom["hasApiKey"] = json!(false);
        assert_eq!(
            row_problem(Some(&custom), "", "follow", true),
            Some(Problem::Key)
        );
    }

    #[test]
    fn authoritative_catalog_rejects_stale_models_and_efforts() {
        let provider = json!({
            "hasApiKey": true,
            "modelMetadata": [{"id": "known", "supportedEfforts": ["medium", "medium"]}],
        });
        assert!(!allows_manual(&provider));
        assert_eq!(effort_choices(&provider, "known"), ["follow", "medium"]);
        assert_eq!(
            row_problem(Some(&provider), "missing", "medium", true),
            Some(Problem::Model)
        );
        assert_eq!(
            row_problem(Some(&provider), "known", "high", true),
            Some(Problem::Effort)
        );
        assert_eq!(row_problem(Some(&provider), "known", "medium", true), None);
        assert_eq!(
            row_problem(None, "known", "medium", true),
            Some(Problem::Provider)
        );
    }

    #[test]
    fn official_without_installed_model_directory_reports_catalog_unavailable() {
        let provider = json!({"builtIn": true, "endpoint": "https://api.anthropic.com/"});
        assert!(!allows_manual(&provider));
        assert!(available_models(&provider).is_empty());
        assert_eq!(
            row_problem(Some(&provider), "", "follow", true),
            Some(Problem::Catalog)
        );
    }
}
