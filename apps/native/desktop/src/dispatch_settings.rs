//! Standalone GPUI panel for the global dispatch defaults.
//!
//! The panel owns only presentation and input plumbing. Every value, option,
//! hint, validation message, and the final write come from
//! `matou_native_core::dispatch_config::form::DispatchForm`; the host supplies
//! the single write callback. That keeps this view reusable outside the
//! settings shell and keeps the frozen form rules in one place.
//!
//! Choice fields are real GPUI menus rather than a fake select: the panel is
//! rendered inside the settings scroll container, so the popup and its dismiss
//! backdrop use `deferred(anchored(..))` and cannot be clipped by that
//! container. Numeric fields reuse [`RenameInput`] with `search_style` so the
//! frozen fields have no 20-character ceiling, no placeholder, and native IME,
//! selection, and clipboard behavior. Enter and focus loss commit the number
//! in place; a rejected save returns the first invalid field and this panel
//! moves focus there.

use std::{cell::RefCell, rc::Rc};

use crate::{
    rename_input::{RenameInput, RenameInputEvent},
    settings_shell::{
        ACCENT, FocusRingStyle, LINE, LINE_STRONG, MUTED, PANEL, PANEL_MUTED, TEXT,
        focus_ring_with, icon, shadow,
    },
    theme_tokens::AppTheme,
    workspace::{popover_colors, workspace_theme_dark},
};
use gpui::{prelude::*, *};
use matou_native_core::dispatch_config::{
    DispatchConfigSnapshot, DispatchDefaults,
    form::{
        ChoiceField, ChoiceView, DispatchForm, MISSING_NOTE, NumberField, SAVED_MESSAGE, SCOPE_NOTE,
    },
};

// Frozen copy shared with the renderer's `dispatchSettings` messages.
const TITLE: &str = "派活";
const HEADING: &str = "设置新派任务的全局默认执行者、模型、时限与重试次数。";
const PATH_LABEL: &str = "配置文件";
const RESTORE_DEFAULTS: &str = "恢复默认";
const SAVE: &str = "保存";

// Frozen `dispatch-settings.css` local colors. The settings shell has no
// success or row-error token, so these stay local to the dispatch block, as
// the assignment block does.
const ERROR_TEXT: u32 = 0xb34d47;
const SUCCESS_TEXT: u32 = 0x16794b;
const SUCCESS_BG: u32 = 0x22965a1f; // rgba(34,150,90,.12)
const MENU_SELECTED_TEXT: u32 = 0x2679d8ff;
const MENU_SELECTED_BG: u32 = 0xe4efffff;
const MENU_ACTIVE_BG: u32 = 0xeef5ffff;

/// The choice menu follows the workspace palette (`dispatch-settings.css`),
/// while the light selected/hover values stay the frozen local blues.
struct MenuColors {
    panel: u32,
    border: u32,
    text: u32,
    selected_text: u32,
    selected_bg: u32,
    active_bg: u32,
    hover_bg: u32,
    shadow: BoxShadow,
}

impl MenuColors {
    fn for_dark(dark: bool) -> Self {
        if dark {
            let palette = AppTheme::Dark.palette();
            Self {
                panel: palette.workspace.panel,
                border: palette.workspace.line_strong,
                text: palette.workspace.text,
                selected_text: palette.workspace.accent,
                selected_bg: palette.workspace.accent_soft,
                active_bg: 0xffffff14,
                hover_bg: 0xffffff14,
                // `.perm-menu` / `.workbench-action-popover` shadow.
                shadow: popover_colors(true).menu_shadow(),
            }
        } else {
            let palette = AppTheme::Light.palette();
            Self {
                panel: palette.workspace.panel,
                border: palette.workspace.line,
                text: palette.workspace.text,
                selected_text: MENU_SELECTED_TEXT,
                selected_bg: MENU_SELECTED_BG,
                active_bg: MENU_ACTIVE_BG,
                hover_bg: MENU_SELECTED_BG,
                shadow: shadow(0x1a304b, 0.16, 8., 24.),
            }
        }
    }

    fn item_background(&self, selected: bool, active: bool) -> Rgba {
        if selected {
            rgba(self.selected_bg)
        } else if active {
            rgba(self.active_bg)
        } else {
            // Transparent lets the translucent panel show through once.
            rgba(0x00000000)
        }
    }

    fn item_hover(&self, selected: bool) -> Rgba {
        if selected {
            rgba(self.selected_bg)
        } else {
            rgba(self.hover_bg)
        }
    }
}

const ROW_LABEL_WIDTH: f32 = 168.;
const ROW_BODY_MAX_WIDTH: f32 = 280.;
const NARROW_BREAKPOINT: f32 = 640.;

#[derive(Clone, Copy)]
struct MenuOption {
    value: &'static str,
    label: &'static str,
}

#[derive(Clone)]
struct OpenMenu {
    field: ChoiceField,
    active: usize,
    trigger_focus: FocusHandle,
    items: Vec<MenuOption>,
    selected: String,
}

/// The reusable dispatch settings panel.
///
/// Create it with the already-read form so the host keeps ownership of loading
/// and retry behavior:
///
/// `cx.new(|cx| DispatchSettingsPanel::new(form, write, cx))`
///
/// `write` receives the complete validated defaults and returns the snapshot
/// the host persisted, or a message that is shown verbatim in the panel.
pub struct DispatchSettingsPanel {
    form: DispatchForm,
    write: Box<dyn FnMut(DispatchDefaults) -> Result<DispatchConfigSnapshot, String>>,
    choice_focus: [FocusHandle; 3],
    number_inputs: [Entity<RenameInput>; 4],
    _number_subscriptions: Vec<Subscription>,
    blur_subscriptions: Vec<Subscription>,
    blur_subscribed: bool,
    trigger_bounds: Rc<RefCell<[Option<Bounds<Pixels>>; 3]>>,
    open_menu: Option<OpenMenu>,
    menu_scroll: ScrollHandle,
    restore_focus: FocusHandle,
    save_focus: FocusHandle,
}

impl DispatchSettingsPanel {
    pub fn new(
        form: DispatchForm,
        write: impl FnMut(DispatchDefaults) -> Result<DispatchConfigSnapshot, String> + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let choice_focus: [FocusHandle; 3] =
            std::array::from_fn(|_| cx.focus_handle().tab_stop(true));
        let number_inputs: [Entity<RenameInput>; 4] = std::array::from_fn(|index| {
            let field = NumberField::ALL[index];
            let text = form.number(field).to_owned();
            cx.new(|cx| {
                let mut input = RenameInput::new(text, cx);
                // The frozen dispatch number field is unbounded and empty when
                // it has no value; `search_style` removes the label cap and
                // the counter and gives the 12px field typography.
                input.search_style = true;
                input.placeholder.clear();
                input
            })
        });

        let mut subscriptions = Vec::with_capacity(4);
        for (index, input) in number_inputs.iter().enumerate() {
            let field = NumberField::ALL[index];
            subscriptions.push(cx.subscribe(input, move |this, _, event, cx| match event {
                RenameInputEvent::Changed => this.number_edited(field, cx),
                RenameInputEvent::Confirm => this.number_committed(field, cx),
                // Escape is not a form action in the frozen dispatch field.
                RenameInputEvent::Cancel => {}
            }));
        }

        Self {
            form,
            write: Box::new(write),
            choice_focus,
            number_inputs,
            _number_subscriptions: subscriptions,
            blur_subscriptions: Vec::new(),
            blur_subscribed: false,
            trigger_bounds: Rc::new(RefCell::new([None; 3])),
            open_menu: None,
            menu_scroll: ScrollHandle::new(),
            restore_focus: cx.focus_handle().tab_stop(true),
            save_focus: cx.focus_handle().tab_stop(true),
        }
    }

    fn ensure_blur_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blur_subscribed {
            return;
        }
        self.blur_subscribed = true;
        let targets: Vec<(NumberField, FocusHandle)> = NumberField::ALL
            .iter()
            .enumerate()
            .map(|(index, field)| (*field, self.number_inputs[index].read(cx).focus.clone()))
            .collect();
        for (field, focus) in targets {
            self.blur_subscriptions
                .push(cx.on_blur(&focus, window, move |this, window, cx| {
                    this.number_committed(field, cx);
                    this.sync_number(field, window, cx);
                    window.refresh();
                    cx.notify();
                }));
        }
    }

    fn number_edited(&mut self, field: NumberField, cx: &mut Context<Self>) {
        let text = &self.number_inputs[field as usize].read(cx).text;
        // GPUI delivers Changed after the editor update. A programmatic
        // writeback already matches the form's value and must keep its status.
        if text != self.form.number(field) {
            self.form.edit_number(field, text.clone());
            cx.notify();
        }
    }

    fn number_committed(&mut self, field: NumberField, cx: &mut Context<Self>) {
        self.form.commit_number(field);
        cx.notify();
    }

    /// Copy a form-owned number back into an unfocused editor. The editor
    /// cannot be written while focused without fighting the caret; save and
    /// restore both move focus to their button first, so their sync lands.
    fn sync_number(&mut self, field: NumberField, window: &mut Window, cx: &mut Context<Self>) {
        let index = field as usize;
        let input = self.number_inputs[index].clone();
        let desired = self.form.number(field).to_owned();
        let length = {
            let current = input.read(cx);
            if current.focus.is_focused(window) || current.text == desired {
                return;
            }
            current.text.encode_utf16().count()
        };

        input.update(cx, |input, cx| {
            input.replace_text_in_range(Some(0..length), &desired, window, cx);
        });
    }

    #[cfg(debug_assertions)]
    pub fn probe_focus_number(&self, index: usize, window: &mut Window, cx: &App) {
        window.focus(&self.number_inputs[index].read(cx).focus);
    }

    #[cfg(debug_assertions)]
    pub fn probe_focus_save(&self, window: &mut Window) {
        window.focus(&self.save_focus);
    }

    fn sync_all_numbers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in NumberField::ALL {
            self.sync_number(field, window, cx);
        }
    }

    fn restore_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        self.form.restore_defaults();
        self.sync_all_numbers(window, cx);
        window.refresh();
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.form.is_dirty() {
            return;
        }
        let write = &mut self.write;
        let first_error = self.form.save(|defaults| write(defaults));
        if let Some(field) = first_error {
            let focus = self.number_inputs[field as usize].read(cx).focus.clone();
            window.focus(&focus);
            window.refresh();
        } else if self.form.saved() {
            self.open_menu = None;
            self.sync_all_numbers(window, cx);
            window.refresh();
        }
        cx.notify();
    }

    fn toggle_choice_menu(
        &mut self,
        field: ChoiceField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open_here = self
            .open_menu
            .as_ref()
            .is_some_and(|menu| menu.field == field);
        if open_here {
            self.close_menu(window, cx);
        } else {
            self.open_choice_menu(field, window, cx);
        }
    }

    fn open_choice_menu(
        &mut self,
        field: ChoiceField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self.form.choice(field);
        if view.disabled || view.read_only || view.options.is_empty() {
            return;
        }
        let selected = view.value.clone();
        let items: Vec<MenuOption> = view
            .options
            .into_iter()
            .map(|option| MenuOption {
                value: option.value,
                label: option.label,
            })
            .collect();
        let active = items
            .iter()
            .position(|option| option.value == selected)
            .unwrap_or(0);
        let trigger_focus = self.choice_focus[field as usize].clone();
        window.focus(&trigger_focus);
        self.open_menu = Some(OpenMenu {
            field,
            active,
            trigger_focus,
            items,
            selected,
        });
        self.menu_scroll.scroll_to_item(active);
        window.refresh();
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.open_menu.take() else {
            return;
        };
        window.focus(&menu.trigger_focus);
        window.refresh();
        cx.notify();
    }

    fn select_menu_option(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.open_menu.take() else {
            return;
        };
        let Some(option) = menu.items.get(index).copied() else {
            self.open_menu = Some(menu);
            return;
        };
        if option.value != menu.selected {
            self.form.choose(menu.field, option.value);
        }
        window.focus(&menu.trigger_focus);
        window.refresh();
        cx.notify();
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

    fn menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((len, active)) = self
            .open_menu
            .as_ref()
            .map(|menu| (menu.items.len(), menu.active))
        else {
            return false;
        };
        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_menu(window, cx);
                cx.stop_propagation();
            }
            "tab" => {
                // Restore the trigger and let the window move focus. Tab is a
                // focus-order operation, not a menu selection.
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
                self.select_menu_option(active, window, cx);
                cx.stop_propagation();
            }
            _ => {
                // A list popup is modal for plain keys; never let a stray
                // shortcut fire behind it while it is open.
                cx.stop_propagation();
            }
        }
        true
    }

    fn choice_key(
        &mut self,
        field: ChoiceField,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_menu.is_some() {
            self.menu_key(event, window, cx);
            return;
        }
        let key = event.keystroke.key.as_str();
        if matches!(key, "enter" | "space" | "down") && !event.keystroke.modifiers.modified() {
            self.open_choice_menu(field, window, cx);
            cx.stop_propagation();
        }
    }

    fn choice_control(
        &mut self,
        field: ChoiceField,
        view: &ChoiceView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.choice_focus[field as usize].clone();
        let open_here = self
            .open_menu
            .as_ref()
            .is_some_and(|menu| menu.field == field);
        let border = if open_here {
            rgb(ACCENT)
        } else {
            rgba(LINE_STRONG)
        };

        if view.read_only {
            let mouse_focus = focus.clone();
            return div()
                .relative()
                .w_full()
                .h(px(34.))
                .flex()
                .items_center()
                .px(px(9.))
                .rounded(px(7.))
                .border_1()
                .border_color(border)
                .bg(rgb(PANEL_MUTED))
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(view.shown_label.clone())
                .track_focus(&focus)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    window.focus(&mouse_focus);
                    cx.stop_propagation();
                })
                .child(focus_ring_with(focus, 7., true, FocusRingStyle::Shell))
                .into_any_element();
        }

        let mut trigger = div()
            .id(SharedString::from(format!(
                "dispatch-choice-{}",
                field.key()
            )))
            .relative()
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
                    .child(view.shown_label.clone()),
            )
            .child(icon("chevron-down", 12., MUTED))
            .child(
                canvas(|_, _, _| (), {
                    let bounds_cell = self.trigger_bounds.clone();
                    let index = field as usize;
                    move |bounds, _, _, _| {
                        bounds_cell.borrow_mut()[index] = Some(bounds);
                    }
                })
                .absolute()
                .inset_0()
                .size_full(),
            );

        if view.disabled {
            return trigger.opacity(0.55).into_any_element();
        }

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
                this.toggle_choice_menu(field, window, cx);
            }))
            .on_key_down(cx.listener(move |this, event, window, cx| {
                this.choice_key(field, event, window, cx);
            }))
            .child(focus_ring_with(focus, 7., true, FocusRingStyle::Shell));
        trigger.into_any_element()
    }

    fn choice_row(
        &mut self,
        field: ChoiceField,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = self.form.choice(field);
        let label_focus = (!view.disabled).then(|| self.choice_focus[field as usize].clone());
        let control = self.choice_control(field, &view, cx);
        let mut body = div().flex().flex_col().gap(px(5.)).min_w_0();
        body = if narrow {
            body.w_full()
        } else {
            body.flex_1().max_w(px(ROW_BODY_MAX_WIDTH))
        };
        body = body.child(control);
        for hint in view.hints {
            body = body.child(
                div()
                    .text_size(px(10.))
                    .line_height(relative(1.55))
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child(hint),
            );
        }
        setting_row(field.label(), body.into_any_element(), narrow, label_focus)
    }

    fn number_row(
        &mut self,
        field: NumberField,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let index = field as usize;
        let input = self.number_inputs[index].clone();
        let focus = input.read(cx).focus.clone();
        let click_focus = focus.clone();
        let label_focus = Some(focus.clone());
        let error = self.form.number_error(field).map(str::to_owned);
        let unit = field.unit();

        let mut number = div().flex().items_center().gap(px(8.)).child(
            div()
                .relative()
                .w(px(88.))
                .h(px(34.))
                .flex()
                .items_center()
                .px(px(9.))
                .rounded(px(7.))
                .border_1()
                .border_color(if error.is_some() {
                    rgb(ERROR_TEXT)
                } else {
                    rgba(LINE_STRONG)
                })
                .bg(rgb(PANEL))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    window.focus(&click_focus);
                    cx.stop_propagation();
                })
                .child(input)
                .child(focus_ring_with(focus, 7., true, FocusRingStyle::Shell)),
        );
        if !unit.is_empty() {
            number = number.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .whitespace_nowrap()
                    .child(unit),
            );
        }

        let mut body = div().flex().flex_col().gap(px(5.)).min_w_0();
        body = if narrow {
            body.w_full()
        } else {
            body.flex_1().max_w(px(ROW_BODY_MAX_WIDTH))
        };
        body = body.child(number);
        if let Some(error) = error {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .line_height(relative(1.55))
                    .text_color(rgb(ERROR_TEXT))
                    .whitespace_normal()
                    .child(error),
            );
        }
        setting_row(field.label(), body.into_any_element(), narrow, label_focus)
    }

    fn actions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let can_save = self.form.can_save();
        // The frozen save button is disabled only while nothing is dirty and
        // nothing was just saved. Invalid edits still make it clickable so the
        // click can surface every row error and focus the first one.
        let save_focusable = self.form.is_dirty() || self.form.saved();
        let mut actions = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(12.))
            .py(px(12.))
            .child(self.action_button(
                "dispatch-restore",
                RESTORE_DEFAULTS,
                false,
                true,
                true,
                self.restore_focus.clone(),
                cx,
                |this, window, cx| this.restore_defaults(window, cx),
            ))
            .child(self.action_button(
                "dispatch-save",
                SAVE,
                true,
                can_save,
                save_focusable,
                self.save_focus.clone(),
                cx,
                |this, window, cx| this.save(window, cx),
            ));
        if self.form.saved() {
            actions = actions.child(
                div()
                    .flex()
                    .items_center()
                    .px(px(9.))
                    .py(px(3.))
                    .rounded_full()
                    .bg(rgba(SUCCESS_BG))
                    .text_size(px(11.))
                    .font_weight(FontWeight(600.))
                    .line_height(relative(1.5))
                    .text_color(rgb(SUCCESS_TEXT))
                    .child(SAVED_MESSAGE),
            );
        }
        if let Some(error) = self.form.save_error() {
            actions = actions.child(
                div()
                    .min_w_0()
                    .max_w(px(420.))
                    .text_size(px(11.))
                    .line_height(relative(1.55))
                    .text_color(rgb(ERROR_TEXT))
                    .whitespace_normal()
                    .child(error.to_owned()),
            );
        }
        actions.into_any_element()
    }

    fn action_button<F>(
        &self,
        id: &'static str,
        label: &'static str,
        primary: bool,
        active: bool,
        focusable: bool,
        focus: FocusHandle,
        cx: &mut Context<Self>,
        activate: F,
    ) -> AnyElement
    where
        F: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    {
        let base = div()
            .id(id)
            .relative()
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

        let styled = if primary {
            base.border_color(rgb(ACCENT))
                .text_color(rgb(0xffffff))
                .bg(rgb(ACCENT))
        } else {
            base.border_color(rgba(LINE_STRONG))
                .text_color(rgb(TEXT))
                .bg(rgb(PANEL))
        };

        let styled = if !focusable {
            return styled
                .opacity(0.5)
                .cursor(CursorStyle::Arrow)
                .into_any_element();
        } else if active {
            if primary {
                styled
                    .hover(|style| style.bg(rgb(0x2f79df)))
                    .cursor_pointer()
            } else {
                styled
                    .hover(|style| {
                        style
                            .border_color(rgb(ACCENT))
                            .text_color(rgb(ACCENT))
                            .bg(rgb(0xf7faff))
                    })
                    .cursor_pointer()
            }
        } else {
            styled.opacity(0.5).cursor(CursorStyle::Arrow)
        };

        let activate: Rc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> = Rc::new(activate);
        let key_activate = activate.clone();
        let click_activate = activate;
        let mouse_focus = focus.clone();

        styled
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
            .child(focus_ring_with(focus, 7., true, FocusRingStyle::Shell))
            .into_any_element()
    }

    fn render_menu_backdrop(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let viewport = window.viewport_size();
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("dispatch-settings-menu-backdrop")
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
        menu: &OpenMenu,
        bounds: Bounds<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let width = f32::from(bounds.size.width)
            .max(menu_min_width(menu.field))
            .min((f32::from(viewport.width) - 16.).max(120.));
        let max_height = (f32::from(viewport.height) - 16.).min(300.).max(96.);
        let colors = MenuColors::for_dark(workspace_theme_dark(cx));

        let mut popup = div()
            .id("dispatch-settings-menu")
            .w(px(width))
            .max_h(px(max_height))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(colors.border))
            .bg(rgba(colors.panel))
            .shadow(vec![colors.shadow.clone()])
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

        for (index, option) in menu.items.iter().enumerate() {
            let selected = option.value == menu.selected;
            let active = index == menu.active;
            let background = colors.item_background(selected, active);
            let hover = colors.item_hover(selected);
            let item = div()
                .id(SharedString::from(format!(
                    "dispatch-choice-{}-{index}",
                    menu.field.key()
                )))
                .flex_none()
                .h(px(30.))
                .flex()
                .items_center()
                .px(px(9.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(rgba(if selected {
                    colors.selected_text
                } else {
                    colors.text
                }))
                .bg(background)
                .hover(move |style| style.bg(hover))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, event, window, cx| {
                    if matches!(event, ClickEvent::Keyboard(_)) {
                        return;
                    }
                    cx.stop_propagation();
                    this.select_menu_option(index, window, cx);
                }))
                .child(option.label);
            popup = popup.child(item);
        }

        deferred(
            anchored()
                .position(point(bounds.left(), bounds.bottom() + px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(popup),
        )
        .with_priority(101)
        .into_any_element()
    }
}

impl Render for DispatchSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_blur_subscriptions(window, cx);

        // A menu cannot survive a state transition that disables or replaces
        // its control, nor losing focus to the surrounding shell.
        let stale = self.open_menu.as_ref().is_some_and(|menu| {
            let view = self.form.choice(menu.field);
            view.disabled || view.read_only || !menu.trigger_focus.is_focused(window)
        });
        if stale {
            self.open_menu = None;
        }

        let narrow = f32::from(window.viewport_size().width) <= NARROW_BREAKPOINT;

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

        let mut card = div()
            .flex()
            .flex_col()
            .pt(px(2.))
            .px(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(LINE))
            .bg(rgb(PANEL_MUTED));
        for field in ChoiceField::ALL {
            card = card.child(self.choice_row(field, narrow, cx));
        }
        for field in NumberField::ALL {
            card = card.child(self.number_row(field, narrow, cx));
        }
        card = card.child(self.actions(cx));

        let section = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(header)
            .child(card)
            .child(
                div()
                    .mt(px(2.))
                    .text_size(px(11.))
                    .line_height(relative(1.65))
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child(SCOPE_NOTE),
            )
            .child(self.path_line());

        let mut root = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .w_full()
            .mt(px(22.))
            .pt(px(20.))
            .border_t_1()
            .border_color(rgba(LINE))
            .child(section);

        if let Some(menu) = self.open_menu.clone() {
            let bounds = (*self.trigger_bounds.borrow())[menu.field as usize];
            if let Some(bounds) = bounds {
                root = root.child(self.render_menu_backdrop(window, cx));
                root = root.child(self.render_menu(&menu, bounds, window, cx));
            }
        }

        root
    }
}

impl DispatchSettingsPanel {
    fn path_line(&self) -> AnyElement {
        let path = self.form.config_path();
        let mut line = div()
            .flex()
            .items_baseline()
            .flex_wrap()
            .gap(px(6.))
            .text_size(px(11.))
            .line_height(relative(1.65))
            .text_color(rgb(MUTED))
            .child(PATH_LABEL)
            .child(
                div()
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(rgba(LINE))
                    .bg(rgb(PANEL_MUTED))
                    .font_family(monospace_family())
                    .text_size(px(10.))
                    .text_color(rgb(TEXT))
                    .child(path),
            );
        if !self.form.exists() {
            line = line.child(format!("· {MISSING_NOTE}"));
        }
        line.into_any_element()
    }
}

fn setting_row(
    label: &'static str,
    body: AnyElement,
    narrow: bool,
    focus: Option<FocusHandle>,
) -> AnyElement {
    let mut label_element = div()
        .text_size(px(11.))
        .font_weight(FontWeight(650.))
        .line_height(relative(1.4))
        .text_color(rgb(TEXT))
        .child(label);
    if let Some(focus) = focus {
        label_element = label_element
            .on_mouse_down(MouseButton::Left, move |_, window, _| window.focus(&focus));
    }

    let row = div()
        .flex()
        .items_start()
        .gap(px(16.))
        .py(px(10.))
        .border_b_1()
        .border_color(rgba(LINE));
    if narrow {
        row.flex_col()
            .gap(px(5.))
            .child(label_element)
            .child(body)
            .into_any_element()
    } else {
        row.child(label_element.w(px(ROW_LABEL_WIDTH)).flex_none().pt(px(9.)))
            .child(body)
            .into_any_element()
    }
}

fn menu_min_width(field: ChoiceField) -> f32 {
    match field {
        ChoiceField::Agent => 200.,
        ChoiceField::Model => 220.,
        ChoiceField::Effort => 132.,
    }
}

fn monospace_family() -> &'static str {
    if cfg!(target_os = "macos") {
        "SF Mono"
    } else {
        "Consolas"
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChoiceField, DispatchConfigSnapshot, DispatchDefaults, DispatchForm, MenuColors,
        NumberField,
    };
    use gpui::rgba;

    #[test]
    fn dispatch_choice_menu_uses_light_and_dark_surfaces_and_item_states() {
        let light = MenuColors::for_dark(false);
        assert_eq!(light.panel, 0xffffffff);
        assert_eq!(light.border, 0x424b581a);
        assert_eq!(light.text, 0x303744ff);
        assert_eq!(light.selected_text, 0x2679d8ff);
        assert_eq!(light.item_background(true, false), rgba(0xe4efffff));
        assert_eq!(light.item_background(false, true), rgba(0xeef5ffff));
        assert_eq!(light.item_hover(false), rgba(0xe4efffff));
        assert_eq!(light.shadow.color.a, 0.16);

        let dark = MenuColors::for_dark(true);
        assert_eq!(dark.panel, 0x2c2f35f5);
        assert_eq!(dark.border, 0xffffff26);
        assert_eq!(dark.text, 0xe1e5ebff);
        assert_eq!(dark.selected_text, 0x67a6efff);
        assert_eq!(dark.item_background(false, false), rgba(0x00000000));
        assert_eq!(dark.item_background(false, true), rgba(0xffffff14));
        assert_eq!(dark.item_hover(false), rgba(0xffffff14));
        assert_eq!(dark.item_background(true, true), rgba(0x4a91e224));
        assert_eq!(dark.item_hover(true), rgba(0x4a91e224));
        assert_eq!(dark.shadow.color.a, 0.5);
    }

    fn snapshot(defaults: DispatchDefaults) -> DispatchConfigSnapshot {
        DispatchConfigSnapshot {
            exists: true,
            path: "/test/.claude/agent-foreman/config.json".into(),
            defaults,
        }
    }

    #[test]
    fn edit_and_restore_only_change_the_draft() {
        let initial = DispatchDefaults {
            model: "luna".into(),
            idle_min: 18.0,
            ..DispatchDefaults::default()
        };
        let mut form = DispatchForm::new(snapshot(initial.clone())).unwrap();
        assert!(!form.is_dirty());
        form.edit_number(NumberField::IdleMin, " 19.0 ".into());
        form.commit_number(NumberField::IdleMin);
        assert_eq!(form.number(NumberField::IdleMin), " 19.0 ");
        assert!(form.can_save());

        form.restore_defaults();
        assert_eq!(form.choice(ChoiceField::Model).value, "sol");
        assert_eq!(form.number(NumberField::IdleMin), "10");
        assert!(form.can_save());
        assert_eq!(
            DispatchForm::new(snapshot(initial))
                .unwrap()
                .number(NumberField::IdleMin),
            "18"
        );
    }

    #[test]
    fn invalid_draft_shows_errors_and_never_calls_the_writer() {
        let mut form = DispatchForm::new(snapshot(DispatchDefaults::default())).unwrap();
        form.edit_number(NumberField::IdleMin, "0".into());
        form.edit_number(NumberField::Retries, "1.5".into());
        assert!(form.is_dirty());
        assert!(!form.can_save());
        assert_eq!(form.number_error(NumberField::IdleMin), None);
        form.commit_number(NumberField::Retries);
        assert_eq!(
            form.number_error(NumberField::Retries),
            Some("请输入整数。")
        );
        assert_eq!(
            form.save(|_| panic!("invalid drafts must never reach the writer")),
            Some(NumberField::IdleMin)
        );
        assert_eq!(
            form.number_error(NumberField::IdleMin),
            Some("请输入 1–60 之间的数字。")
        );
    }

    #[test]
    fn fallback_labels_do_not_replace_stored_values() {
        let mut form = DispatchForm::new(snapshot(DispatchDefaults {
            model: "custom/model".into(),
            effort: "xhigh".into(),
            ..DispatchDefaults::default()
        }))
        .unwrap();
        let model = form.choice(ChoiceField::Model);
        assert_eq!(model.value, "sol");
        assert_eq!(model.hints.len(), 1);

        form.choose(ChoiceField::Agent, "dsh");
        let model = form.choice(ChoiceField::Model);
        assert_eq!(model.value, "deepseek-flash");
        assert!(model.read_only);
        let effort = form.choice(ChoiceField::Effort);
        assert_eq!(effort.value, "high");
        assert_eq!(effort.hints.len(), 2);
        assert_eq!(
            form.save(|defaults| {
                assert_eq!(defaults.model, "custom/model");
                assert_eq!(defaults.effort, "xhigh");
                Ok(snapshot(defaults))
            }),
            None
        );
        assert!(form.saved());
    }

    #[test]
    fn failed_callback_keeps_the_edit_and_retry_normalizes_it() {
        let mut form = DispatchForm::new(snapshot(DispatchDefaults::default())).unwrap();
        form.edit_number(NumberField::IdleMin, " 20.0 ".into());
        assert_eq!(
            form.save(|defaults| {
                assert_eq!(defaults.idle_min, 20.0);
                Err("磁盘写入失败".into())
            }),
            None
        );
        assert_eq!(
            form.save_error(),
            Some("保存失败，修改已保留。磁盘写入失败")
        );
        assert_eq!(form.number(NumberField::IdleMin), " 20.0 ");
        assert!(form.can_save());
        assert!(!form.saved());

        assert_eq!(
            form.save(|defaults| {
                assert_eq!(defaults.idle_min, 20.0);
                Ok(snapshot(defaults))
            }),
            None
        );
        assert_eq!(form.save_error(), None);
        assert_eq!(form.number(NumberField::IdleMin), "20");
        assert!(form.saved());
        assert!(!form.is_dirty());
    }
}
