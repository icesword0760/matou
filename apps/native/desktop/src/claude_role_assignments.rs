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
use matou_native_core::{
    claude_role_budget::{CLAUDE_ROLE_LABELS, CLAUDE_ROLES as ROLES},
    sessions::InterfaceLocale,
};

use super::{
    ACCENT, DANGER_BG, DANGER_TEXT, FocusRingStyle, LINE, LINE_STRONG, MUTED, PANEL, PANEL_MUTED,
    TEXT, focus_ring_with, icon, shadow,
};
use crate::rename_input::{RenameInput, RenameInputEvent};
use matou_native_desktop::theme_tokens::{LIGHT, ThemePalette};

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
const VIEW_SEARCH_TIER: &str = "网页搜索";
const VIEW_SEARCH_UNASSIGNED: &str = "未指定";
const VIEW_SEARCH_FOLLOW_TIER: &str = "未指定：搜索请求使用当前档位供应商";
const VIEW_SEARCH_NOTE: &str = "搜索请求会连同当前对话内容发送给该供应商";

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

/// Resolved colors for one assignment panel frame.
///
/// Light keeps the frozen CSS literals verbatim (including alpha). Dark maps
/// the same roles onto the active workspace/settings palette so the inline
/// card, selects, and dropdown menu no longer paint a light surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AssignmentColors {
    panel: u32,
    panel_muted: u32,
    text: u32,
    muted: u32,
    line: u32,
    line_strong: u32,
    accent: u32,
    accent_hover: u32,
    primary_bg: u32,
    primary_text: u32,
    primary_hover: u32,
    link_hover: u32,
    role_error: u32,
    view_error: u32,
    danger_bg: u32,
    success_text: u32,
    success_bg: u32,
    menu_selected_text: u32,
    menu_selected_bg: u32,
    menu_active_bg: u32,
    secondary_hover_bg: u32,
}

/// Expands a frozen 24-bit CSS color to an opaque packed RGBA value, so every
/// resolved field can flow through `rgba(..)` without channel shifts.
const fn packed(hex: u32) -> u32 {
    (hex << 8) | 0xff
}

impl AssignmentColors {
    fn new(palette: &ThemePalette, dark: bool) -> Self {
        if dark {
            Self {
                panel: palette.workspace.panel,
                panel_muted: palette.workspace.panel_muted,
                text: palette.workspace.text,
                muted: palette.workspace.muted,
                line: palette.workspace.line,
                line_strong: palette.workspace.line_strong,
                accent: palette.workspace.accent,
                accent_hover: palette.settings.language_primary_hover,
                // The dark accent is a light blue; keep primary buttons on a
                // readable dark-blue base with white text.
                primary_bg: 0x2f79dfff,
                primary_text: 0xffffffff,
                primary_hover: 0x2274dfff,
                link_hover: palette.workspace.accent,
                role_error: palette.settings.language_error,
                view_error: palette.settings.language_error,
                danger_bg: palette.cards.remove_hover,
                success_text: 0x6fd39bff,
                success_bg: VIEW_SUCCESS_BG,
                menu_selected_text: palette.settings.language_menu_text,
                menu_selected_bg: palette.settings.language_menu_background,
                menu_active_bg: palette.settings.language_menu_active_background,
                secondary_hover_bg: palette.settings.language_button_hover,
            }
        } else {
            Self {
                panel: packed(PANEL),
                panel_muted: packed(PANEL_MUTED),
                text: packed(TEXT),
                muted: packed(MUTED),
                line: LINE,
                line_strong: LINE_STRONG,
                accent: packed(ACCENT),
                accent_hover: packed(0x2f79df),
                primary_bg: packed(ACCENT),
                primary_text: 0xffffffff,
                primary_hover: packed(0x2f79df),
                link_hover: packed(0x1f6dc4),
                role_error: packed(DANGER_TEXT),
                view_error: packed(VIEW_ERROR),
                danger_bg: packed(DANGER_BG),
                success_text: packed(VIEW_SUCCESS_TEXT),
                success_bg: VIEW_SUCCESS_BG,
                menu_selected_text: packed(VIEW_MENU_SELECTED_TEXT),
                menu_selected_bg: packed(VIEW_MENU_SELECTED_BG),
                menu_active_bg: packed(VIEW_MENU_ACTIVE_BG),
                secondary_hover_bg: packed(0xf7faff),
            }
        }
    }
}

/// One frame of the frozen assignment block, supplied by the settings logic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignmentPanel {
    locale: InterfaceLocale,
    pub(crate) rows: [AssignmentRow; ASSIGNMENT_ROWS],
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

/// One tier row. The web search row follows the model tiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignmentRow {
    pub(crate) tier: &'static str,
    pub(crate) provider: AssignmentSelect,
    pub(crate) account: String,
    pub(crate) model: AssignmentSelect,
    pub(crate) effort: AssignmentSelect,
    pub(crate) manual_model: Option<String>,
    pub(crate) target: String,
    /// Row-local explanatory copy, rendered in the row's meta area.
    pub(crate) note: Option<String>,
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
    RetryCatalog(usize),
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
    locale: InterfaceLocale,
    /// Colors resolved from the palette passed to the current frame.
    colors: AssignmentColors,
    /// [row][provider / model / effort] trigger focus handles. The final row is web search.
    controls: [[FocusHandle; 3]; ASSIGNMENT_ROWS],
    /// Slot 0 is the disconnect retry; the remaining slots are per-row catalog retries.
    retry_focus: [FocusHandle; ASSIGNMENT_ROWS + 1],
    restore_focus: FocusHandle,
    save_focus: FocusHandle,
    manual_inputs: [Entity<RenameInput>; ASSIGNMENT_ROWS],
    _manual_subscriptions: Vec<Subscription>,
    manual_edit: [bool; ASSIGNMENT_ROWS],
    manual_sync_suppress: [bool; ASSIGNMENT_ROWS],
    open_menu: Option<OpenAssignmentMenu>,
    trigger_bounds: Rc<RefCell<[[Option<Bounds<Pixels>>; 3]; ASSIGNMENT_ROWS]>>,
    menu_scroll: ScrollHandle,
    pending_focus: Option<FocusHandle>,
}

impl AssignmentView {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let controls: [[FocusHandle; 3]; ASSIGNMENT_ROWS] =
            std::array::from_fn(|_| std::array::from_fn(|_| cx.focus_handle().tab_stop(true)));
        let retry_focus = std::array::from_fn(|_| cx.focus_handle().tab_stop(true));
        let restore_focus = cx.focus_handle().tab_stop(true);
        let save_focus = cx.focus_handle().tab_stop(true);
        let manual_inputs: [Entity<RenameInput>; ASSIGNMENT_ROWS] = std::array::from_fn(|_| {
            cx.new(|cx| {
                let mut input = RenameInput::new(String::new(), cx);
                // Model ids are routinely longer than the editor's 20-unit
                // label cap; the frozen field is unbounded and shows no counter.
                input.search_style = true;
                input.placeholder.clear();
                input
            })
        });

        let mut subscriptions = Vec::with_capacity(ASSIGNMENT_ROWS);
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
            locale: InterfaceLocale::default(),
            colors: AssignmentColors::new(&LIGHT, false),
            controls,
            retry_focus,
            restore_focus,
            save_focus,
            manual_inputs,
            _manual_subscriptions: subscriptions,
            manual_edit: [false; ASSIGNMENT_ROWS],
            manual_sync_suppress: [false; ASSIGNMENT_ROWS],
            open_menu: None,
            trigger_bounds: Rc::new(RefCell::new([[None; 3]; ASSIGNMENT_ROWS])),
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
        palette: &ThemePalette,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.colors = AssignmentColors::new(palette, dark);
        let colors = self.colors;
        let manual_text = dark.then(|| rgba(palette.settings.input_text));
        let manual_placeholder = dark.then(|| rgba(palette.workspace.muted));
        for input in &self.manual_inputs {
            input.update(cx, |input, _| {
                input.text_color = manual_text;
                input.placeholder_color = manual_placeholder;
            });
        }
        if let Some(focus) = self.pending_focus.take() {
            window.focus(&focus);
        }

        let AssignmentPanel {
            locale,
            rows,
            status,
            can_save,
            can_restore,
            saving,
            saved,
            save_error,
        } = panel;
        self.locale = locale;

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
                    .text_color(rgba(colors.text))
                    .child(self.locale.text(VIEW_TITLE, "Model assignments")),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgba(colors.muted))
                    .whitespace_normal()
                    .child(self.locale.text(VIEW_SUBTITLE, "Assign a provider and model to each tier. Unassigned tiers follow the default provider.")),
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
            .border_color(rgba(colors.line))
            .bg(rgba(colors.panel_muted));

        if !narrow {
            card = card.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(VIEW_GRID_GAP))
                    .pt(px(10.))
                    .pb(px(8.))
                    .border_b_1()
                    .border_color(rgba(colors.line))
                    .text_size(px(10.))
                    .font_weight(FontWeight(650.))
                    .text_color(rgba(colors.muted))
                    .child(
                        div()
                            .w(px(VIEW_ROLE_WIDTH))
                            .flex_none()
                            .child(self.locale.text(VIEW_HEAD_TIER, "Tier")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.locale.text(VIEW_HEAD_PROVIDER, "Provider / account")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.locale.text(VIEW_HEAD_MODEL, "Model")),
                    )
                    .child(
                        div()
                            .w(px(VIEW_EFFORT_WIDTH))
                            .flex_none()
                            .child(self.locale.text(VIEW_HEAD_EFFORT, "Reasoning effort")),
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
                .text_color(rgba(colors.muted))
                .whitespace_normal()
                .child(self.locale.text(
                    VIEW_PRIVACY,
                    "Conversation content needed for a request is sent to the assigned provider.",
                )),
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
                self.locale.text(VIEW_RESTORE, "Restore defaults"),
                false,
                can_restore && !saving,
                self.restore_focus.clone(),
                keyboard_focus,
                &colors,
                cx,
                |this, _, cx| {
                    this.manual_edit = [false; ASSIGNMENT_ROWS];
                    this.manual_sync_suppress = [false; ASSIGNMENT_ROWS];
                    cx.emit(AssignmentAction::Restore);
                    cx.notify();
                },
            ))
            .child(assignment_action_button(
                "claude-assignment-save",
                if saving {
                    self.locale.text(VIEW_SAVING, "Saving…")
                } else {
                    self.locale.text(VIEW_SAVE, "Save assignments")
                },
                true,
                can_save && !saving,
                self.save_focus.clone(),
                keyboard_focus,
                &colors,
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
                    .bg(rgba(colors.success_bg))
                    .text_size(px(11.))
                    .font_weight(FontWeight(600.))
                    .line_height(relative(1.5))
                    .text_color(rgba(colors.success_text))
                    .child(self.locale.text(
                        VIEW_SAVED,
                        "Model assignments saved. New sessions will use this configuration.",
                    )),
            );
        }

        let mut footer = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgba(colors.muted))
                    .child(self.locale.text(
                        VIEW_SCOPE,
                        "New sessions use these assignments. Running sessions are unaffected.",
                    )),
            )
            .child(actions);
        if save_error {
            footer = footer.child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.55))
                    .text_color(rgba(colors.view_error))
                    .child(self.locale.text(
                        VIEW_SAVE_ERROR,
                        "Save was not confirmed. Your selections were kept; please retry.",
                    )),
            );
        }
        section = section.child(footer);

        let mut outer = div()
            .mt(px(22.))
            .pt(px(20.))
            .border_t_1()
            .border_color(rgba(colors.line))
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
        let colors = self.colors;
        match status {
            AssignmentStatus::Ready => None,
            AssignmentStatus::Loading => Some(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgba(colors.muted))
                    .child(self.locale.text(VIEW_LOADING, "Loading model assignments…"))
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
                    .text_color(rgba(colors.view_error))
                    .child(
                        self.locale
                            .text(VIEW_DISCONNECTED, "Settings service is disconnected."),
                    )
                    .child(self.retry_link(0, keyboard_focus, cx))
                    .into_any_element(),
            ),
            AssignmentStatus::Empty => Some(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgba(colors.muted))
                    .child(self.locale.text(
                        VIEW_EMPTY,
                        "No providers are available. Add a provider first.",
                    ))
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
        let colors = self.colors;
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
                    .text_color(rgba(colors.muted))
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

        let role_color = if problem {
            colors.role_error
        } else {
            colors.text
        };

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
                        .text_color(rgba(role_color))
                        .child(row.tier),
                )
                .child(assignment_labeled_field(
                    self.locale.text(VIEW_HEAD_PROVIDER, "Provider / account"),
                    provider_field.into_any_element(),
                    &colors,
                ))
                .child(assignment_labeled_field(
                    self.locale.text(VIEW_HEAD_MODEL, "Model"),
                    model_field.into_any_element(),
                    &colors,
                ))
                .child(assignment_labeled_field(
                    self.locale.text(VIEW_HEAD_EFFORT, "Reasoning effort"),
                    effort_field.into_any_element(),
                    &colors,
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
                        .text_color(rgba(role_color))
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
                    .text_color(rgba(colors.muted))
                    .whitespace_normal()
                    .child(row.target.clone()),
            );
            has_meta = true;
        }
        if let Some(note) = row.note.as_deref() {
            meta = meta.child(
                div()
                    .text_size(px(10.))
                    .line_height(relative(1.55))
                    .text_color(rgba(colors.muted))
                    .whitespace_normal()
                    .child(note.to_owned()),
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
        if index < SEARCH_INDEX {
            container = container.border_b_1().border_color(rgba(colors.line));
        }
        if problem {
            container = container.bg(with_alpha(colors.danger_bg, 0.48));
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
        let colors = self.colors;
        let mut line = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(6.))
            .text_size(px(11.))
            .line_height(relative(1.55))
            .text_color(rgba(colors.view_error))
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
        let colors = self.colors;
        let enabled = select.enabled && !saving;
        let focus = self.controls[row][column.index()].clone();
        let open_here = self
            .open_menu
            .as_ref()
            .is_some_and(|menu| menu.row == row && menu.column == column);
        let label = assignment_display_label(select);
        let border = field_border(problem, open_here || focus.is_focused(window), &colors);

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
            .bg(rgba(colors.panel))
            .text_size(px(12.))
            .text_color(rgba(colors.text))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .child(icon("chevron-down", 12., colors.muted >> 8));

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
        let colors = self.colors;
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
                .border_color(field_border(invalid, false, &colors))
                .bg(rgba(colors.panel))
                .text_size(px(12.))
                .text_color(rgba(colors.text))
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
            .border_color(field_border(invalid, focused, &colors))
            .bg(rgba(colors.panel))
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
            self.locale.text(VIEW_RETRY, "Retry"),
            focus,
            keyboard_focus,
            &self.colors,
            cx,
            move |_, _, cx| {
                cx.emit(if slot == 0 {
                    AssignmentAction::Retry
                } else {
                    AssignmentAction::RetryCatalog(slot - 1)
                });
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
        let colors = self.colors;
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
            .border_color(rgba(colors.line))
            .bg(rgba(colors.panel))
            .shadow(vec![shadow(0x1a304b, 0.16, 8., 24.)])
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

        for (index, option) in open.items.iter().enumerate() {
            let selected = option.value == open.selected;
            let active = index == open.active;
            let background = if selected {
                colors.menu_selected_bg
            } else if active {
                colors.menu_active_bg
            } else {
                colors.panel
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
                .text_color(rgba(if selected {
                    colors.menu_selected_text
                } else {
                    colors.text
                }))
                .bg(rgba(background))
                .hover(move |style| style.bg(rgba(colors.menu_selected_bg)))
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

fn with_alpha(color: u32, alpha: f32) -> Rgba {
    let mut color = rgba(color);
    color.a = alpha;
    color
}

fn field_border(problem: bool, focused: bool, colors: &AssignmentColors) -> Rgba {
    if problem {
        rgba(colors.view_error)
    } else if focused {
        rgba(colors.accent)
    } else {
        rgba(colors.line_strong)
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

fn assignment_labeled_field(
    label: &'static str,
    field: AnyElement,
    colors: &AssignmentColors,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight(650.))
                .text_color(rgba(colors.muted))
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
    colors: &AssignmentColors,
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
        base.border_color(rgba(colors.primary_bg))
            .text_color(rgba(colors.primary_text))
            .bg(rgba(colors.primary_bg))
    } else {
        base.border_color(rgba(colors.line_strong))
            .text_color(rgba(colors.text))
            .bg(rgba(colors.panel))
    };

    if !enabled {
        return button
            .opacity(0.5)
            .cursor(CursorStyle::Arrow)
            .into_any_element();
    }

    let button = if primary {
        button
            .hover(move |style| style.bg(rgba(colors.primary_hover)))
            .cursor_pointer()
    } else {
        button
            .hover(move |style| {
                style
                    .border_color(rgba(colors.accent))
                    .text_color(rgba(colors.accent))
                    .bg(rgba(colors.secondary_hover_bg))
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
    colors: &AssignmentColors,
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
        .text_color(rgba(colors.accent))
        .cursor_pointer()
        .hover(move |style| style.text_color(rgba(colors.link_hover)))
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
use matou_native_runtime::protocol::{
    ClaudeModelCatalogReply, ClaudeModelCatalogResult, ClaudeRoleAssignmentsReply,
};
use serde_json::Value;

use super::effort_name;
use crate::{
    provider_settings::ProviderSettings,
    rename_input::trim_name,
    runtime_bridge::{Command, ConnectionPhase, RuntimeCommands},
};

const SEARCH_INDEX: usize = ROLES.len();
const ASSIGNMENT_ROWS: usize = SEARCH_INDEX + 1;
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
    fn localized(self, locale: InterfaceLocale) -> &'static str {
        locale.text(self.text(), match self {
            Self::Provider => "Provider no longer exists. Your selection was kept; choose another provider.",
            Self::Key => "API key is missing. Edit the provider first.",
            Self::Account => "Account is unavailable. Sign in again or choose another provider.",
            Self::Empty => "Choose or enter a model.",
            Self::Model => "Model is unavailable. Your selection was kept; choose another model.",
            Self::Catalog => "Model directory is unavailable. Please retry.",
            Self::Invalid => "Invalid model ID. Please check your input.",
            Self::Effort => "The selected reasoning effort is not supported by this model. Its value was kept; choose another effort.",
        })
    }

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

fn catalog<'a>(provider: &'a Value, native: &'a [Value]) -> Option<&'a [Value]> {
    if official(provider) {
        return Some(native);
    }
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

/// Web search has no role mapping: an omitted effort inherits the provider's
/// global effort, while an explicit `follow` stays on the row and blocks that
/// inheritance.
fn global_effort(provider: &Value) -> &str {
    provider["reasoningEffort"]
        .as_str()
        .filter(|effort| EFFORTS.contains(effort))
        .unwrap_or("follow")
}

fn search_models(provider: &Value, native: &[Value]) -> Vec<AssignmentOption> {
    // Reuse the tier directory behavior: with no authoritative directory the
    // startup/tier models stay as suggestions and manual ids remain allowed.
    // An existing but empty directory yields no fallback suggestions.
    available_models(provider, native)
}

fn effort_choices(provider: &Value, model: &str, native: &[Value]) -> Vec<&'static str> {
    let mut choices = vec!["follow"];
    if let Some(supported) = model_metadata(catalog(provider, native).unwrap_or_default(), model)
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
            && ROLES
                .into_iter()
                .chain(["claude"])
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
    native: &[Value],
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

    let official = official(provider);
    if official && native.is_empty() && (assigned || model.is_empty()) {
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
    if let Some(models) = catalog(provider, native)
        && !(official && models.is_empty())
        && model_metadata(models, id).is_none()
    {
        return Some(Problem::Model);
    }
    (!effort_choices(provider, id, native).contains(&effort)).then_some(Problem::Effort)
}

/// A web-search row is stricter than a tier row: the target must be a
/// Claude Code API Key target, and a manual model id is checked after trimming.
fn search_problem(
    provider: Option<&Value>,
    model: &str,
    effort: &str,
    native: &[Value],
) -> Option<Problem> {
    let Some(provider) = provider else {
        // A stored target whose provider was deleted keeps its old value.
        return Some(Problem::Provider);
    };
    // The frozen runtime rejects subscription targets as unavailable search
    // providers; the row keeps the stored value so the user can clear it.
    if subscription(provider) {
        return Some(Problem::Provider);
    }
    if provider["hasApiKey"] != true {
        return Some(Problem::Key);
    }
    // An existing but empty authoritative/native directory is reported before
    // the empty-model prompt, so the user is not asked to pick a model that
    // cannot exist yet.
    let directory = catalog(provider, native);
    if directory.is_some_and(|models| models.is_empty()) {
        return Some(Problem::Catalog);
    }
    let id = trim_name(model);
    if id.is_empty() {
        return Some(Problem::Empty);
    }
    if model.encode_utf16().count() > 240 || model.chars().any(|ch| ch.is_ascii_control()) {
        return Some(Problem::Invalid);
    }
    // After trimming both ends the frozen search validator rejects any
    // whitespace, Unicode control (including U+0085 and DEL) and an interior
    // BOM in the remaining value.
    if id
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\u{feff}')
    {
        return Some(Problem::Invalid);
    }
    if let Some(models) = directory
        && model_metadata(models, id).is_none()
    {
        return Some(Problem::Model);
    }
    (!effort_choices(provider, id, native).contains(&effort)).then_some(Problem::Effort)
}

fn search_patch(search: Option<&ClaudeRoleAssignment>) -> Option<Value> {
    match search {
        Some(target) => serde_json::to_value(target).ok(),
        None => Some(Value::Null),
    }
}

/// A search-row effort selection is always explicit, including `follow`.
/// Storing it prevents a later provider-global change from re-enabling
/// inheritance on a row the user deliberately set to follow.
fn store_search_effort(search: &mut Option<ClaudeRoleAssignment>, effort: &str) {
    if let Some(target) = search {
        target.effort = Some(effort.to_owned());
    }
}

fn assignment_effort_name(effort: &str, locale: InterfaceLocale) -> &'static str {
    locale.text(
        effort_name(effort),
        match effort {
            "low" => "Low",
            "medium" => "Medium",
            "high" => "High",
            "xhigh" => "XHigh",
            "max" => "Max",
            _ => "Follow session",
        },
    )
}

fn account_caption(provider: Option<&Value>, assigned: bool, locale: InterfaceLocale) -> String {
    if !assigned {
        let name = provider
            .and_then(|provider| provider["name"].as_str())
            .unwrap_or(locale.text("默认供应商不可用", "Default provider unavailable"));
        return format!("{name} · {}", locale.text("默认供应商", "Default provider"));
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

fn available_models(provider: &Value, native: &[Value]) -> Vec<AssignmentOption> {
    if let Some(models) = catalog(provider, native) {
        return models
            .iter()
            .filter_map(|model| {
                let id = model["id"].as_str()?;
                Some(option(id, model["displayName"].as_str().unwrap_or(id)))
            })
            .collect();
    }
    let mut models = Vec::new();
    if !subscription(provider) {
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
    !subscription(provider) && provider["builtIn"] != true && catalog(provider, &[]).is_none()
}

/// The search row only trusts an explicit directory. A built-in provider whose
/// endpoint was changed to a custom API is no longer `official`, so it can use
/// a manually typed model when it has no `modelMetadata` field.
fn allows_manual_search(provider: &Value) -> bool {
    !subscription(provider) && !official(provider) && provider.get("modelMetadata").is_none()
}

fn default_model<'a>(provider: &'a Value, role: &str, native: &'a [Value]) -> Option<&'a str> {
    if let Some(mapped) = provider["modelMapping"][role]
        .as_str()
        .filter(|id| !id.is_empty())
    {
        return Some(mapped);
    }
    if !official(provider) {
        return None;
    }
    let prefix = format!("claude-{role}-");
    model_metadata(native, role)
        .or_else(|| {
            native
                .iter()
                .find(|row| row["id"].as_str().is_some_and(|id| id.starts_with(&prefix)))
        })
        .and_then(|row| row["id"].as_str())
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

#[derive(Default)]
enum NativeCatalog {
    #[default]
    Unrequested,
    Pending {
        id: u64,
        generation: u64,
    },
    Loaded(Vec<Value>),
}

impl NativeCatalog {
    fn models(&self) -> &[Value] {
        match self {
            Self::Loaded(models) => models,
            _ => &[],
        }
    }
}

pub(crate) struct ClaudeAssignments {
    commands: RuntimeCommands,
    pending: Option<PendingAssignment>,
    baseline: Option<ClaudeRoleAssignments>,
    draft: ClaudeRoleAssignments,
    search_baseline: Option<ClaudeRoleAssignment>,
    search_draft: Option<ClaudeRoleAssignment>,
    manual: [bool; ASSIGNMENT_ROWS],
    load_error: bool,
    save_error: bool,
    saved: bool,
    native_catalog: NativeCatalog,
}

impl ClaudeAssignments {
    pub(crate) fn new(commands: RuntimeCommands) -> Self {
        Self {
            commands,
            pending: None,
            baseline: None,
            draft: ClaudeRoleAssignments::new(),
            search_baseline: None,
            search_draft: None,
            manual: [false; ASSIGNMENT_ROWS],
            load_error: false,
            save_error: false,
            saved: false,
            native_catalog: NativeCatalog::default(),
        }
    }

    pub(crate) fn reset(&mut self) {
        let native_catalog = std::mem::take(&mut self.native_catalog);
        *self = Self::new(self.commands.clone());
        self.native_catalog = native_catalog;
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
        if matches!(self.native_catalog, NativeCatalog::Pending { id, .. } if id == request_id) {
            self.native_catalog = NativeCatalog::Loaded(Vec::new());
            return true;
        }
        let Some(pending) = self.pending.take_if(|pending| pending.id == request_id) else {
            return false;
        };
        self.fail(pending.operation);
        true
    }

    pub(crate) fn load_catalog(
        &mut self,
        providers: &ProviderSettings,
        retry: bool,
    ) -> Option<u64> {
        if matches!(self.native_catalog, NativeCatalog::Pending { .. })
            || (!retry && !matches!(self.native_catalog, NativeCatalog::Unrequested))
        {
            return None;
        }
        let needed = providers.providers().iter().any(|provider| {
            official(provider)
                && provider["id"].as_str().is_some_and(|id| {
                    providers.active_id() == Some(id)
                        || self.draft.values().any(|row| row.provider_id == id)
                        || self
                            .search_draft
                            .as_ref()
                            .is_some_and(|row| row.provider_id == id)
                })
        });
        let (generation, phase) = self.commands.connection();
        if !needed || phase != ConnectionPhase::Ready {
            return None;
        }
        let id = NEXT_ASSIGNMENT_REQUEST.fetch_add(1, Ordering::Relaxed);
        self.commands
            .send_at(
                generation,
                Command::GetClaudeModelCatalog { request_id: id },
            )
            .ok()?;
        self.native_catalog = NativeCatalog::Pending { id, generation };
        Some(id)
    }

    pub(crate) fn receive_catalog(&mut self, reply: ClaudeModelCatalogReply) {
        let NativeCatalog::Pending { id, generation } = self.native_catalog else {
            return;
        };
        if id != reply.request_id
            || self.commands.connection() != (generation, ConnectionPhase::Ready)
        {
            return;
        }
        // The protocol already uses the persisted metadata field names.
        let models = match reply.result {
            ClaudeModelCatalogResult::Models(models) => models
                .into_iter()
                .filter_map(|model| serde_json::to_value(model).ok())
                .collect(),
            ClaudeModelCatalogResult::Error(_) => Vec::new(),
        };
        self.native_catalog = NativeCatalog::Loaded(models);
    }

    pub(crate) fn saving(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.operation == AssignmentRequest::Save)
    }

    fn dirty(&self) -> bool {
        self.baseline.as_ref().is_some_and(|baseline| {
            baseline != &self.draft || self.search_baseline != self.search_draft
        })
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
        let saved = pending.operation == AssignmentRequest::Save;
        if saved && reply.web_search.is_none() && self.search_baseline != self.search_draft {
            self.fail(pending.operation);
            return;
        }
        // An older runtime can acknowledge a tiers-only save without this field.
        let search = match reply.web_search {
            Some(search) => Some(search),
            None if saved => None,
            None => Some(None),
        };
        let Some(assignments) = reply.assignments.filter(|_| reply.error.is_none()) else {
            self.fail(pending.operation);
            return;
        };
        if saved || !self.dirty() {
            self.draft = assignments.clone();
            self.manual = [false; ASSIGNMENT_ROWS];
            if let Some(search) = &search {
                self.search_draft = search.clone();
            }
        }
        if let Some(search) = search {
            self.search_baseline = search;
        }
        self.baseline = Some(assignments);
        self.load_error = false;
        if saved {
            self.saved = true;
            self.save_error = false;
        }
    }

    pub(crate) fn connection_changed(&mut self, visible: bool) -> Option<u64> {
        let (generation, phase) = self.commands.connection();
        if matches!(self.native_catalog, NativeCatalog::Pending { generation: pending, .. }
            if pending != generation || phase != ConnectionPhase::Ready)
        {
            self.native_catalog = NativeCatalog::Loaded(Vec::new());
        }
        if !visible {
            return None;
        }
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
        self.panel_localized(providers, InterfaceLocale::default())
    }

    pub(crate) fn panel_localized(
        &self,
        providers: &ProviderSettings,
        locale: InterfaceLocale,
    ) -> AssignmentPanel {
        let status = self.status(providers);
        let saving = self.saving();
        let enabled = matches!(status, AssignmentStatus::Ready) && !saving;
        let rows = std::array::from_fn(|index| self.row(index, providers, enabled, locale));
        let can_save = enabled && self.dirty() && rows.iter().all(|row| row.problem.is_none());
        AssignmentPanel {
            locale,
            rows,
            status,
            can_save,
            can_restore: self.baseline.is_some() && !saving,
            saving,
            saved: self.saved,
            save_error: self.save_error,
        }
    }

    fn follows_opus(&self, role: &str, providers: &ProviderSettings) -> bool {
        role == "fable"
            && !self.draft.contains_key(role)
            && providers
                .providers()
                .iter()
                .find(|provider| provider["id"].as_str() == providers.active_id())
                .and_then(|provider| provider["modelMapping"][role].as_str())
                .is_none_or(|model| model.trim().is_empty())
    }

    fn row_provider<'a>(&self, index: usize, providers: &'a ProviderSettings) -> Option<&'a Value> {
        let (provider_id, fallback) = if index == SEARCH_INDEX {
            (
                self.search_draft
                    .as_ref()
                    .map(|row| row.provider_id.as_str()),
                None,
            )
        } else {
            let role = *ROLES.get(index)?;
            let role = if self.follows_opus(role, providers) {
                "opus"
            } else {
                role
            };
            (
                self.draft.get(role).map(|row| row.provider_id.as_str()),
                providers.active_id(),
            )
        };
        let effective_id = provider_id.or(fallback);
        providers.providers().iter().find(|provider| {
            provider["id"]
                .as_str()
                .is_some_and(|id| Some(id) == effective_id)
        })
    }

    pub(crate) fn uses_native_catalog(&self, index: usize, providers: &ProviderSettings) -> bool {
        index <= SEARCH_INDEX && self.row_provider(index, providers).is_some_and(official)
    }

    fn row(
        &self,
        index: usize,
        providers: &ProviderSettings,
        enabled: bool,
        locale: InterfaceLocale,
    ) -> AssignmentRow {
        if index == SEARCH_INDEX {
            return self.search_row(providers, enabled, locale);
        }
        let role = ROLES[index];
        let tier = CLAUDE_ROLE_LABELS[index];
        let assigned = self.draft.get(role);
        let provider_id = assigned.map(|row| row.provider_id.as_str());
        let provider = self.row_provider(index, providers);
        let provider_value = provider.unwrap_or(&Value::Null);
        let provider_name = provider_value["name"]
            .as_str()
            .or(provider_id)
            .unwrap_or("—");
        let native = self.native_catalog.models();
        let models = catalog(provider_value, native).unwrap_or_default();
        let follows_opus = self.follows_opus(role, providers);
        let effective_role = if follows_opus { "opus" } else { role };
        let inherited = follows_opus.then(|| self.draft.get("opus")).flatten();
        let default = inherited
            .map(|row| row.model.as_str())
            .or_else(|| default_model(provider_value, effective_role, native));
        let selected = assigned.map(|row| row.model.as_str()).unwrap_or_default();
        let effective_model = assigned.map(|_| selected).or(default).unwrap_or_default();
        let effort = assigned
            .or(inherited)
            .and_then(|row| row.effort.as_deref())
            .unwrap_or_else(|| inherited_effort(provider_value, effective_role));
        let manual_allowed = provider.is_some_and(allows_manual);
        let choices = available_models(provider_value, native);
        let unavailable_model =
            !selected.is_empty() && !choices.iter().any(|model| model.value == selected);
        let manual = manual_allowed && (self.manual[index] || unavailable_model);
        let problem = if self.draft.is_empty() || follows_opus {
            None
        } else {
            row_problem(
                provider,
                effective_model,
                effort,
                assigned.is_some(),
                native,
            )
        };
        let selectable = enabled
            && assigned.is_some()
            && provider.is_some_and(|provider| !subscription(provider));

        let mut provider_items = vec![option(
            "",
            if follows_opus {
                locale.text("沿用 Opus", "Follow Opus")
            } else {
                locale.text(FOLLOW_PROVIDER, "Follow default provider")
            },
        )];
        if let Some(id) = provider_id.filter(|_| provider.is_none_or(subscription)) {
            provider_items.push(option(
                id,
                format!("{id} · {}", locale.text("不可用", "Unavailable")),
            ));
        }
        provider_items.extend(providers.providers().iter().filter_map(|provider| {
            if subscription(provider) {
                return None;
            }
            let id = provider["id"].as_str()?;
            Some(option(id, provider["name"].as_str().unwrap_or(id)))
        }));
        let mut model_items = vec![option("", locale.text(CHOOSE_MODEL, "Choose a model"))];
        let model_selection = if assigned.is_none() {
            model_items[0].label = default
                .map(|id| model_name(models, id).to_owned())
                .unwrap_or_else(|| {
                    format!(
                        "{tier} {}",
                        locale.text(
                            "原生默认档位（目录暂不可用）",
                            "native default (directory unavailable)"
                        )
                    )
                });
            ""
        } else {
            if unavailable_model && !manual {
                model_items.push(option(
                    selected,
                    format!("{selected} · {}", locale.text("不可用", "Unavailable")),
                ));
            }
            model_items.extend(choices);
            if manual_allowed {
                model_items.push(option(
                    MANUAL_MODEL_SENTINEL,
                    locale.text("手动填写模型 ID…", "Enter a model ID…"),
                ));
            }
            if manual {
                MANUAL_MODEL_SENTINEL
            } else {
                selected
            }
        };
        let mut efforts: Vec<_> = effort_choices(provider_value, effective_model, native)
            .into_iter()
            .map(|level| option(level, assignment_effort_name(level, locale)))
            .collect();
        if !efforts.iter().any(|item| item.value == effort) {
            efforts.push(option(
                effort,
                format!(
                    "{} {}",
                    assignment_effort_name(effort, locale),
                    locale.text("（不支持）", "(unsupported)")
                ),
            ));
        }
        if assigned.is_none() {
            efforts = vec![option(effort, assignment_effort_name(effort, locale))];
        }
        let target_model = if assigned.is_some() || default.is_some() {
            model_name(models, effective_model)
        } else {
            locale.text("原生默认档位待确认", "Native default tier pending")
        };
        let prefix = if assigned.is_some() {
            locale.text("当前目标：", "Current target: ")
        } else {
            locale.text("最终：", "Effective target: ")
        };
        let mut target = format!("{prefix}{provider_name} · {target_model}");
        if effort != "follow" {
            target.push_str(" · ");
            target.push_str(assignment_effort_name(effort, locale));
        }
        let problem_text = problem.map(|problem| {
            if assigned.is_some() {
                problem.localized(locale).to_owned()
            } else {
                let id = default.unwrap_or(tier);
                match locale {
                    InterfaceLocale::Chinese => format!("沿用默认：{provider_name} · {id}。{}请为此档位重新选择供应商和模型。", problem.text()),
                    InterfaceLocale::English => format!("Default target: {provider_name} · {id}. {} Choose another provider and model for this tier.", problem.localized(locale)),
                }
            }
        });
        AssignmentRow {
            tier,
            provider: select(provider_items, provider_id.unwrap_or_default(), enabled),
            account: account_caption(provider, assigned.is_some(), locale),
            model: select(model_items, model_selection, selectable),
            effort: select(efforts, effort, selectable),
            manual_model: manual.then(|| selected.to_owned()),
            target,
            note: None,
            problem: problem_text,
            retry_catalog: problem == Some(Problem::Catalog),
        }
    }

    /// The final row is an independent web-search target. It is unassigned by
    /// default and does not fall back to the active provider in the editor.
    fn search_row(
        &self,
        providers: &ProviderSettings,
        enabled: bool,
        locale: InterfaceLocale,
    ) -> AssignmentRow {
        let assigned = self.search_draft.as_ref();
        let provider_id = assigned.map(|row| row.provider_id.as_str());
        let provider = self.row_provider(SEARCH_INDEX, providers);
        let provider_value = provider.unwrap_or(&Value::Null);
        let provider_name = provider_value["name"]
            .as_str()
            .or(provider_id)
            .unwrap_or("—");
        let native = self.native_catalog.models();
        let choices = search_models(provider_value, native);
        let selected = assigned.map(|row| row.model.as_str()).unwrap_or_default();
        let effort = assigned
            .and_then(|row| row.effort.as_deref())
            .unwrap_or_else(|| global_effort(provider_value));
        // With no authoritative directory the startup/tier models stay as
        // suggestions and a manual id is allowed; validation only trusts an
        // existing authoritative or native directory.
        let manual_allowed = provider.is_some_and(allows_manual_search);
        let unavailable_model =
            !selected.is_empty() && !choices.iter().any(|model| model.value == selected);
        let manual = manual_allowed && (self.manual[SEARCH_INDEX] || unavailable_model);
        let problem = assigned.and_then(|_| search_problem(provider, selected, effort, native));
        let selectable = enabled
            && assigned.is_some()
            && provider.is_some_and(|provider| !subscription(provider));

        let mut provider_items = vec![option(
            "",
            locale.text(VIEW_SEARCH_UNASSIGNED, "Unassigned"),
        )];
        if let Some(id) = provider_id.filter(|_| provider.is_none_or(subscription)) {
            provider_items.push(option(
                id,
                format!("{id} · {}", locale.text("不可用", "Unavailable")),
            ));
        }
        provider_items.extend(providers.providers().iter().filter_map(|provider| {
            // Only Claude Code provider targets are selectable; subscription
            // providers stay out of the menu. A keyless target stays
            // selectable so its frozen "API Key 未配置" reason is visible, and
            // a stored subscription remains visible until the user clears it.
            if subscription(provider) {
                return None;
            }
            let id = provider["id"].as_str()?;
            Some(option(id, provider["name"].as_str().unwrap_or(id)))
        }));

        let mut model_items = vec![option("", locale.text(CHOOSE_MODEL, "Choose a model"))];
        let model_selection = if assigned.is_none() {
            model_items[0].label = locale.text(VIEW_SEARCH_UNASSIGNED, "Unassigned").into();
            ""
        } else {
            if unavailable_model && !manual {
                model_items.push(option(
                    selected,
                    format!("{selected} · {}", locale.text("不可用", "Unavailable")),
                ));
            }
            model_items.extend(choices);
            if manual_allowed {
                model_items.push(option(
                    MANUAL_MODEL_SENTINEL,
                    locale.text("手动填写模型 ID…", "Enter a model ID…"),
                ));
            }
            if manual {
                MANUAL_MODEL_SENTINEL
            } else {
                selected
            }
        };

        let mut efforts: Vec<_> = effort_choices(provider_value, selected, native)
            .into_iter()
            .map(|level| option(level, assignment_effort_name(level, locale)))
            .collect();
        if !efforts.iter().any(|item| item.value == effort) {
            efforts.push(option(
                effort,
                format!(
                    "{} {}",
                    assignment_effort_name(effort, locale),
                    locale.text("（不支持）", "(unsupported)")
                ),
            ));
        }
        let effort_selection = if assigned.is_none() {
            efforts = vec![option(
                "",
                locale.text(VIEW_SEARCH_UNASSIGNED, "Unassigned"),
            )];
            ""
        } else {
            effort
        };

        let target = if assigned.is_none() {
            locale
                .text(
                    VIEW_SEARCH_FOLLOW_TIER,
                    "Unassigned: search requests use the current tier provider",
                )
                .into()
        } else {
            let target_model = model_name(
                catalog(provider_value, native).unwrap_or_default(),
                selected,
            );
            let mut target = format!(
                "{}{provider_name} · {target_model}",
                locale.text("当前目标：", "Current target: ")
            );
            if effort != "follow" {
                target.push_str(" · ");
                target.push_str(assignment_effort_name(effort, locale));
            }
            target
        };

        AssignmentRow {
            tier: locale.text(VIEW_SEARCH_TIER, "Web search"),
            provider: select(provider_items, provider_id.unwrap_or_default(), enabled),
            account: if assigned.is_some() {
                account_caption(provider, true, locale)
            } else {
                String::new()
            },
            model: select(model_items, model_selection, selectable),
            effort: select(efforts, effort_selection, selectable),
            manual_model: manual.then(|| selected.to_owned()),
            target,
            note: Some(locale.text(VIEW_SEARCH_NOTE, "Search requests include the current conversation content and are sent to that provider.").into()),
            problem: problem.map(|problem| problem.localized(locale).to_owned()),
            retry_catalog: problem == Some(Problem::Catalog),
        }
    }

    pub(crate) fn change(&mut self, action: &AssignmentAction, providers: &ProviderSettings) {
        if self.saving() {
            return;
        }
        if matches!(action, AssignmentAction::Restore) && self.baseline.is_some() {
            self.draft.clear();
            self.search_draft = None;
            self.manual = [false; ASSIGNMENT_ROWS];
        } else if !matches!(self.status(providers), AssignmentStatus::Ready) {
            return;
        } else {
            match action {
                AssignmentAction::Provider(index, id) => {
                    let index = *index;
                    if index == SEARCH_INDEX {
                        if self
                            .search_draft
                            .as_ref()
                            .map_or("", |row| row.provider_id.as_str())
                            == id
                        {
                            return;
                        }
                        self.manual[SEARCH_INDEX] = false;
                        self.search_draft = if id.is_empty() {
                            None
                        } else {
                            Some(ClaudeRoleAssignment {
                                provider_id: id.clone(),
                                model: String::new(),
                                effort: None,
                            })
                        };
                    } else {
                        let Some(role) = ROLES.get(index).copied() else {
                            return;
                        };
                        if self
                            .draft
                            .get(role)
                            .map_or("", |row| row.provider_id.as_str())
                            == id
                        {
                            return;
                        }
                        self.manual[index] = false;
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
                }
                AssignmentAction::Model(index, model) => {
                    let index = *index;
                    if index == SEARCH_INDEX {
                        let manual = model == MANUAL_MODEL_SENTINEL
                            && self.search_draft.as_ref().is_some_and(|row| {
                                providers
                                    .providers()
                                    .iter()
                                    .find(|provider| provider["id"] == row.provider_id)
                                    .is_some_and(allows_manual_search)
                            });
                        let Some(row) = self.search_draft.as_mut() else {
                            return;
                        };
                        self.manual[SEARCH_INDEX] = manual;
                        if manual || row.model == *model {
                            return;
                        }
                        row.model.clone_from(model);
                    } else {
                        let Some(role) = ROLES.get(index).copied() else {
                            return;
                        };
                        if let Some(row) = self.draft.get_mut(role) {
                            self.manual[index] = model == MANUAL_MODEL_SENTINEL
                                && providers
                                    .providers()
                                    .iter()
                                    .find(|provider| provider["id"] == row.provider_id)
                                    .is_some_and(allows_manual);
                            if self.manual[index] || row.model == *model {
                                return;
                            }
                            row.model.clone_from(model);
                        }
                    }
                }
                AssignmentAction::Manual(index, model) => {
                    let index = *index;
                    if index == SEARCH_INDEX {
                        let Some(row) = self.search_draft.as_mut() else {
                            return;
                        };
                        if row.model == *model {
                            return;
                        }
                        self.manual[SEARCH_INDEX] = true;
                        row.model.clone_from(model);
                    } else {
                        let Some(role) = ROLES.get(index).copied() else {
                            return;
                        };
                        if let Some(row) = self.draft.get_mut(role) {
                            if row.model == *model {
                                return;
                            }
                            self.manual[index] = true;
                            row.model.clone_from(model);
                        }
                    }
                }
                AssignmentAction::Effort(index, effort) => {
                    let index = *index;
                    if index == SEARCH_INDEX {
                        if self.search_draft.is_none() {
                            return;
                        }
                        // Keep an explicit follow even when the provider's
                        // current global effort is also follow, so a later
                        // global change cannot make this row inherit again.
                        store_search_effort(&mut self.search_draft, effort);
                    } else {
                        let Some(role) = ROLES.get(index).copied() else {
                            return;
                        };
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
            .zip(search_patch(self.search_draft.as_ref()))
            .and_then(|(assignments, web_search)| {
                self.send(AssignmentRequest::Save, |request_id| {
                    Command::SetClaudeRoleAssignments {
                        request_id,
                        assignments,
                        web_search: Some(web_search),
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
    use super::{
        Problem, VIEW_PRIVACY, VIEW_SAVE_ERROR, VIEW_SEARCH_FOLLOW_TIER, VIEW_SEARCH_NOTE,
        VIEW_SEARCH_TIER, VIEW_SEARCH_UNASSIGNED, allows_manual, allows_manual_search,
        available_models, effort_choices, global_effort, row_problem, search_models, search_patch,
        search_problem, store_search_effort,
    };
    use matou_native_core::provider_launch::ClaudeRoleAssignment;
    use serde_json::{Value, json};

    #[test]
    fn manual_model_requires_a_concrete_id_and_keeps_frozen_validation_order() {
        let mut custom = json!({
            "id": "custom", "model": "CLI 默认", "hasApiKey": true,
            "modelMapping": {"opus": "custom-opus", "sonnet": "custom-opus"},
        });
        assert!(allows_manual(&custom));
        assert_eq!(
            available_models(&custom, &[])
                .iter()
                .map(|model| model.value.as_str())
                .collect::<Vec<_>>(),
            vec!["custom-opus"]
        );
        assert_eq!(
            row_problem(Some(&custom), "", "follow", true, &[]),
            Some(Problem::Empty)
        );
        assert_eq!(
            row_problem(Some(&custom), "", "follow", false, &[]),
            Some(Problem::Catalog)
        );
        assert_eq!(
            row_problem(Some(&custom), "actual-model-id", "follow", true, &[]),
            None
        );
        assert_eq!(
            row_problem(Some(&custom), "bad\tmodel", "follow", true, &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(
            row_problem(Some(&custom), &"🙂".repeat(121), "follow", true, &[]),
            Some(Problem::Invalid)
        );
        custom["hasApiKey"] = json!(false);
        assert_eq!(
            row_problem(Some(&custom), "", "follow", true, &[]),
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
        assert_eq!(
            effort_choices(&provider, "known", &[]),
            ["follow", "medium"]
        );
        assert_eq!(
            row_problem(Some(&provider), "missing", "medium", true, &[]),
            Some(Problem::Model)
        );
        assert_eq!(
            row_problem(Some(&provider), "known", "high", true, &[]),
            Some(Problem::Effort)
        );
        assert_eq!(
            row_problem(Some(&provider), "known", "medium", true, &[]),
            None
        );
        assert_eq!(
            row_problem(None, "known", "medium", true, &[]),
            Some(Problem::Provider)
        );
    }

    #[test]
    fn official_without_installed_model_directory_reports_catalog_unavailable() {
        let provider = json!({"builtIn": true, "endpoint": "https://api.anthropic.com/"});
        assert!(!allows_manual(&provider));
        assert!(available_models(&provider, &[]).is_empty());
        assert_eq!(
            row_problem(Some(&provider), "", "follow", true, &[]),
            Some(Problem::Catalog)
        );
    }

    #[test]
    fn web_search_requires_an_api_key_target_and_rejects_blank_or_control_ids() {
        let api = json!({"id": "api", "hasApiKey": true});
        assert_eq!(
            search_problem(None, "a", "follow", &[]),
            Some(Problem::Provider)
        );
        assert_eq!(
            search_problem(
                Some(&json!({"id": "sub", "subscription": {"provider": "codex"}})),
                "a",
                "follow",
                &[],
            ),
            Some(Problem::Provider)
        );
        assert_eq!(
            search_problem(
                Some(&json!({"id": "api", "hasApiKey": false})),
                "a",
                "follow",
                &[]
            ),
            Some(Problem::Key)
        );
        assert_eq!(
            search_problem(Some(&api), "", "follow", &[]),
            Some(Problem::Empty)
        );
        assert_eq!(
            search_problem(Some(&api), "  \t ", "follow", &[]),
            Some(Problem::Empty)
        );
        assert_eq!(
            search_problem(Some(&api), "a\tb", "follow", &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(
            search_problem(Some(&api), "a\u{7f}b", "follow", &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(
            search_problem(Some(&api), "a b", "follow", &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(
            search_problem(Some(&api), "a\u{85}b", "follow", &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(
            search_problem(Some(&api), "a\u{9f}b", "follow", &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(
            search_problem(Some(&api), "a\u{feff}b", "follow", &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(
            search_problem(Some(&api), "\ta", "follow", &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(search_problem(Some(&api), " a ", "follow", &[]), None);
        assert_eq!(
            search_problem(Some(&api), "\u{feff}vendor/model\u{feff}", "follow", &[]),
            None
        );
        assert_eq!(
            search_problem(Some(&api), &"🙂".repeat(121), "follow", &[]),
            Some(Problem::Invalid)
        );
        assert_eq!(
            search_problem(Some(&api), "vendor/model", "follow", &[]),
            None
        );
    }

    #[test]
    fn web_search_inherits_only_the_provider_global_effort() {
        let provider = json!({
            "id": "api", "hasApiKey": true,
            "reasoningEffort": "low",
            "effortMapping": {"opus": "max", "sonnet": "high"},
        });
        assert_eq!(global_effort(&provider), "low");
        assert_eq!(
            global_effort(&json!({"reasoningEffort": "bogus"})),
            "follow"
        );
        assert_eq!(
            search_problem(Some(&provider), "vendor/model", "low", &[]),
            None
        );
        assert_eq!(
            search_problem(Some(&provider), "vendor/model", "follow", &[]),
            None
        );
        let no_catalog_model = json!([{"id": "vendor/model"}]);
        assert_eq!(
            search_problem(
                Some(&provider),
                "vendor/model",
                "low",
                no_catalog_model.as_array().unwrap()
            ),
            None
        );
    }

    #[test]
    fn web_search_uses_the_authoritative_or_native_directory_when_present() {
        let official = json!({
            "id": "official", "builtIn": true, "hasApiKey": true,
            "endpoint": "https://api.anthropic.com/",
        });
        assert_eq!(
            search_problem(Some(&official), "opus", "follow", &[]),
            Some(Problem::Catalog)
        );
        let native = json!([{"id": "opus", "supportedEfforts": ["low"]}]);
        let native = native.as_array().unwrap();
        assert_eq!(
            search_problem(Some(&official), "sonnet", "follow", native),
            Some(Problem::Model)
        );
        assert_eq!(
            search_problem(Some(&official), "opus", "high", native),
            Some(Problem::Effort)
        );
        assert_eq!(search_problem(Some(&official), "opus", "low", native), None);
        let empty = json!({"id": "api", "hasApiKey": true, "modelMetadata": []});
        assert!(search_models(&empty, &[]).is_empty());
        assert_eq!(
            search_problem(Some(&empty), "", "follow", &[]),
            Some(Problem::Catalog)
        );
        assert_eq!(
            search_problem(Some(&empty), "startup", "follow", &[]),
            Some(Problem::Catalog)
        );
        let legacy = json!({
            "id": "api", "hasApiKey": true,
            "model": "startup",
            "modelMapping": {"opus": "tier-opus", "sonnet": "tier-sonnet"},
        });
        assert!(allows_manual(&legacy));
        assert!(allows_manual_search(&legacy));
        assert_eq!(
            search_models(&legacy, &[])
                .into_iter()
                .map(|model| model.value)
                .collect::<Vec<_>>(),
            ["startup", "tier-opus", "tier-sonnet"]
        );
        assert_eq!(
            search_problem(Some(&legacy), "startup", "follow", &[]),
            None
        );
        // A built-in provider pointed at a custom endpoint is no longer
        // official. The search row may use a manual id without a directory,
        // while the tier rows keep their original manual rule.
        let built_in_custom = json!({
            "id": "builtin", "builtIn": true,
            "endpoint": "https://api.example.com", "hasApiKey": true,
        });
        assert!(!allows_manual(&built_in_custom));
        assert!(allows_manual_search(&built_in_custom));
        assert_eq!(
            search_problem(Some(&built_in_custom), "vendor/model", "follow", &[]),
            None
        );
        assert!(!allows_manual_search(&official));
        let known = json!({
            "id": "api", "hasApiKey": true,
            "modelMetadata": [{"id": "a", "supportedEfforts": ["low"]}],
        });
        assert_eq!(
            search_problem(Some(&known), "b", "follow", &[]),
            Some(Problem::Model)
        );
        assert_eq!(search_problem(Some(&known), " a ", "follow", &[]), None);
        assert_eq!(
            search_problem(Some(&known), "a", "high", &[]),
            Some(Problem::Effort)
        );
        assert_eq!(
            search_models(&known, &[])
                .into_iter()
                .map(|model| model.value)
                .collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn web_search_patch_distinguishes_preserve_clear_and_set() {
        assert_eq!(search_patch(None), Some(Value::Null));
        assert_eq!(
            search_patch(Some(&ClaudeRoleAssignment {
                provider_id: "api".into(),
                model: "vendor/model".into(),
                effort: Some("low".into()),
            })),
            Some(json!({
                "providerId": "api",
                "model": "vendor/model",
                "effort": "low",
            }))
        );
        // An explicit follow is a stored value, not an omitted effort.
        assert_eq!(
            search_patch(Some(&ClaudeRoleAssignment {
                provider_id: "api".into(),
                model: "vendor/model".into(),
                effort: Some("follow".into()),
            })),
            Some(json!({
                "providerId": "api",
                "model": "vendor/model",
                "effort": "follow",
            }))
        );
    }

    #[test]
    fn web_search_keeps_an_explicit_follow_even_when_it_is_currently_inherited() {
        let mut target = Some(ClaudeRoleAssignment {
            provider_id: "api".into(),
            model: "vendor/model".into(),
            effort: None,
        });
        store_search_effort(&mut target, "follow");
        assert_eq!(target.unwrap().effort.as_deref(), Some("follow"));
        store_search_effort(&mut None, "follow");
    }

    #[test]
    fn web_search_copy_matches_the_frozen_strings() {
        assert_eq!(VIEW_SEARCH_TIER, "网页搜索");
        assert_eq!(VIEW_SEARCH_UNASSIGNED, "未指定");
        assert_eq!(
            VIEW_SEARCH_FOLLOW_TIER,
            "未指定：搜索请求使用当前档位供应商"
        );
        assert_eq!(
            VIEW_PRIVACY,
            "使用指定供应商时，请求所需的对话内容会发送给该供应商。"
        );
        assert_eq!(VIEW_SEARCH_NOTE, "搜索请求会连同当前对话内容发送给该供应商");
        assert_eq!(VIEW_SAVE_ERROR, "保存未确认，请重试。原选择已保留。");
        assert_eq!(
            Problem::Provider.text(),
            "供应商已不存在；原选择已保留，请重新选择。"
        );
        assert_eq!(Problem::Key.text(), "API Key 未配置，请先编辑供应商。");
        assert_eq!(
            Problem::Model.text(),
            "模型已不可用；原选择已保留，请重新选择。"
        );
        assert_eq!(Problem::Catalog.text(), "模型目录暂不可用，请重试。");
        assert_eq!(
            Problem::Effort.text(),
            "当前模型不支持所选推理强度；原选择已保留，请重新选择。"
        );
        assert_eq!(Problem::Empty.text(), "请选择或输入模型。");
        assert_eq!(Problem::Invalid.text(), "模型 ID 无效，请检查输入。");
    }
}
