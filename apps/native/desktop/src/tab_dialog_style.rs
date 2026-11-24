use crate::theme_tokens::{DARK, LIGHT, TabPalette};
use crate::ui_text::ui_text;
use gpui::{prelude::*, *};

fn color_alpha(hex: u32, alpha: f32) -> Rgba {
    let mut color = rgb(hex);
    color.a = alpha;
    color
}

fn action_hover(palette: &TabPalette) -> Rgba {
    if palette == &LIGHT.tabs {
        color_alpha(0x000000, 0.08)
    } else {
        rgba(palette.action_hover)
    }
}

fn action_text(palette: &TabPalette) -> Rgba {
    if palette == &DARK.tabs {
        rgba(DARK.cards.compact_text)
    } else {
        rgba(palette.action_text)
    }
}

fn panel_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: color_alpha(0x000000, 0.1).into(),
        offset: point(px(0.), px(4.)),
        blur_radius: px(20.),
        spread_radius: px(0.),
    }]
}

/// `.hierarchy-shell[data-theme="light"] .tab-item` (+ `.active` / `:hover`).
pub fn tab(id: impl Into<ElementId>, active: bool, palette: &TabPalette) -> Stateful<Div> {
    let base = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .min_w(px(100.))
        .max_w(px(200.))
        .h(px(28.))
        .pl(px(12.))
        .pr(px(10.))
        .rounded(px(6.))
        .text_size(px(12.))
        .cursor_pointer();
    if active {
        base.text_color(rgba(palette.active_text))
            .bg(rgba(palette.active_background))
    } else {
        let hover_text = if palette == &LIGHT.tabs {
            color_alpha(0x000000, 0.65)
        } else {
            rgba(palette.hover_text)
        };
        base.text_color(rgba(palette.inactive_text))
            .bg(rgba(0x00000000))
            .hover(move |style| style.text_color(hover_text))
    }
}

/// `.tab-item .tab-title`.
pub fn tab_title(title: impl Into<SharedString>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_normal()
        .line_clamp(1)
        .text_ellipsis()
        .text_left()
        .child(title.into())
}

/// `.hierarchy-shell[data-theme="light"] .tab-close` (`visible` controls opacity only).
pub fn tab_close(id: impl Into<ElementId>, visible: bool, palette: &TabPalette) -> Stateful<Div> {
    let hover = action_hover(palette);
    let foreground = action_text(palette);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded(px(4.))
        .text_color(foreground)
        .bg(rgba(0x00000000))
        .opacity(if visible { 1. } else { 0. })
        .cursor_pointer()
        .hover(move |style| style.text_color(rgba(palette.action_hover_text)).bg(hover))
        .child(
            svg()
                .path("icons/x.svg")
                .text_color(foreground)
                .size(px(13.))
                .flex_none(),
        )
}

/// `.tab-status-dot`.
pub fn tab_status_dot() -> Div {
    div()
        .flex_none()
        .size(px(8.))
        .rounded_full()
        .bg(rgb(0xff4524))
}

/// `.hierarchy-shell[data-theme="light"] .tab-overflow-btn`.
pub fn tab_overflow_btn(id: impl Into<ElementId>, palette: &TabPalette) -> Stateful<Div> {
    let hover = action_hover(palette);
    let foreground = action_text(palette);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(6.))
        .text_color(foreground)
        .bg(rgba(0x00000000))
        .cursor_pointer()
        .hover(move |style| style.text_color(rgba(palette.action_hover_text)).bg(hover))
        .child(
            svg()
                .path("icons/ellipsis.svg")
                .text_color(foreground)
                .size(px(16.))
                .flex_none(),
        )
}

/// `.hierarchy-shell[data-theme="light"] .tab-add-btn`.
pub fn tab_add(id: impl Into<ElementId>, enabled: bool, palette: &TabPalette) -> Stateful<Div> {
    let hover = action_hover(palette);
    let foreground = action_text(palette);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(24.))
        .ml(px(4.))
        .rounded(px(6.))
        .text_color(foreground)
        .bg(rgba(palette.add_background))
        .when(enabled, move |button| {
            button
                .cursor_pointer()
                .hover(move |style| style.text_color(rgba(palette.action_hover_text)).bg(hover))
        })
        .child(
            svg()
                .path("icons/plus.svg")
                .text_color(foreground)
                .size(px(15.))
                .flex_none(),
        )
}

/// `.dialog-overlay` (`inset: 0`, flex center, `rgba(0, 0, 0, .58)`).
pub fn dialog_overlay(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .occlude()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(color_alpha(0x000000, 0.58))
}

/// `.rename-dialog, [role="alertdialog"]` and its `.light-theme` override.
pub fn dialog_panel(viewport_width: f32, dark: bool) -> Div {
    let width = 420.0_f32.min((viewport_width - 40.0).max(0.0));
    div()
        .flex()
        .flex_col()
        .w(px(width))
        .overflow_hidden()
        .rounded(px(12.))
        .text_color(if dark { rgb(0xf4f4f4) } else { rgb(0x2f3547) })
        .bg(if dark { rgb(0x292929) } else { rgb(0xffffff) })
}

/// `.rename-dialog header, [role="alertdialog"] header`.
pub fn dialog_header() -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .pt(px(20.))
        .px(px(24.))
        .pb(px(16.))
}

/// `.rename-dialog h2, [role="alertdialog"] h2` and its light override.
pub fn dialog_title(text: impl Into<SharedString>, dark: bool) -> Div {
    div()
        .text_color(if dark {
            rgb(0xf4f4f4)
        } else {
            color_alpha(0x000000, 0.85)
        })
        .text_size(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(ui_text(text).normal_line_height())
}

/// `.dialog-close`: the frozen CSS keeps a low-opacity white glyph even on the
/// light panel; the light surface uses the same alpha over black so the close
/// affordance stays legible.
pub fn dialog_close(id: impl Into<ElementId>, dark: bool) -> Stateful<Div> {
    let (foreground, hover_text, hover_background) = if dark {
        (
            color_alpha(0xffffff, 0.34),
            color_alpha(0xffffff, 0.6),
            color_alpha(0xffffff, 0.06),
        )
    } else {
        (
            color_alpha(0x000000, 0.34),
            color_alpha(0x000000, 0.6),
            color_alpha(0x000000, 0.06),
        )
    };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(6.))
        .text_color(foreground)
        .bg(rgba(0x00000000))
        .text_size(px(20.))
        .line_height(relative(1.))
        .cursor_pointer()
        .hover(move |style| style.text_color(hover_text).bg(hover_background))
        .child("×")
}

/// `.rename-dialog-body` / `[role="alertdialog"] > p` in both themes.
pub fn dialog_body(dark: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .pt(px(8.))
        .px(px(24.))
        .pb(px(24.))
        .text_color(if dark {
            color_alpha(0xffffff, 0.7)
        } else {
            color_alpha(0x000000, 0.65)
        })
        .text_size(px(14.))
        .line_height(relative(1.6))
}

/// `.rename-input-wrap` + `input` container styling in both themes.
pub fn rename_input_wrap(focused: bool, dark: bool) -> Div {
    let base = div()
        .relative()
        .w_full()
        .min_h(px(32.))
        .pl(px(14.))
        .pr(px(50.))
        .rounded(px(8.))
        .text_color(if dark {
            rgb(0xe8e8e8)
        } else {
            color_alpha(0x000000, 0.85)
        })
        .bg(if dark {
            rgb(0x464646)
        } else {
            color_alpha(0x000000, 0.04)
        })
        .text_size(px(14.));
    if focused {
        base.shadow(vec![BoxShadow {
            color: color_alpha(if dark { 0xffffff } else { 0x000000 }, 0.15).into(),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
        }])
    } else {
        base
    }
}

/// `.rename-input-wrap > span`. The frozen CSS only declares the white alpha
/// used on the dark panel; the light panel uses the same alpha over black so
/// the character counter remains readable on white.
pub fn rename_counter(count: usize, dark: bool) -> Div {
    div()
        .absolute()
        .right(px(14.))
        .top(px(6.))
        .text_color(color_alpha(if dark { 0xffffff } else { 0x000000 }, 0.34))
        .text_size(px(13.))
        .line_height(px(20.))
        .child(format!("{count}/20"))
}

/// `.rename-dialog-body [role="alert"]`.
pub fn dialog_error(text: impl Into<SharedString>, dark: bool) -> Div {
    div()
        .mt(px(8.))
        .text_color(if dark { rgb(0xff7b72) } else { rgb(0xd94238) })
        .text_size(px(12.))
        .child(ui_text(text))
}

/// `.rename-dialog footer, [role="alertdialog"] footer`.
pub fn dialog_footer() -> Div {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(10.))
        .pt(px(16.))
        .px(px(24.))
        .pb(px(24.))
}

/// `.light-theme .rename-dialog footer button` / `.dialog-primary`.
/// Frozen light specificity keeps primary buttons blue (#347fd6) even for danger confirmations.
pub fn dialog_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    disabled: bool,
    dark: bool,
) -> Stateful<Div> {
    let base = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(80.))
        .h(px(32.))
        .px(px(16.))
        .rounded(px(8.))
        .text_size(px(14.))
        .whitespace_nowrap()
        .child(ui_text(label).normal_line_height());
    let styled = if primary {
        let button = base.text_color(rgb(0xffffff)).bg(rgb(0x347fd6));
        if disabled {
            button
        } else {
            button.hover(|style| style.bg(rgb(0x2f72c1)))
        }
    } else if dark {
        let button = base
            .border_1()
            .border_color(rgb(0x383838))
            .text_color(rgb(0xf2f2f2))
            .bg(rgba(0x00000000));
        if disabled {
            button
        } else {
            button.hover(|style| {
                style
                    .border_color(color_alpha(0xffffff, 0.14))
                    .bg(color_alpha(0xffffff, 0.04))
            })
        }
    } else {
        let button = base
            .border_1()
            .border_color(color_alpha(0x000000, 0.12))
            .text_color(color_alpha(0x000000, 0.7))
            .bg(rgba(0x00000000));
        if disabled {
            button
        } else {
            button.hover(|style| style.bg(color_alpha(0x000000, 0.04)))
        }
    };
    if disabled {
        styled
            .opacity(0.45)
            .cursor(CursorStyle::OperationNotAllowed)
    } else {
        styled.cursor_pointer()
    }
}

/// `.light-theme .scene-tab-menu`.
pub fn context_menu(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .flex()
        .flex_col()
        .w(px(128.))
        .p(px(4.))
        .rounded(px(8.))
        .text_color(rgb(0x2f3547))
        .bg(rgb(0xffffff))
        .shadow(panel_shadow())
}

/// `.light-theme .scene-tab-menu button`.
pub fn menu_item(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .w_full()
        .h(px(28.))
        .px(px(9.))
        .rounded(px(5.))
        .text_color(rgb(0x2f3547))
        .text_size(px(12.))
        .text_left()
        .bg(rgba(0x00000000))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xeceef3)))
        .child(ui_text(label))
}

/// `.light-theme .tab-overflow-panel`.
pub fn overflow_panel(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .flex()
        .flex_col()
        .min_w(px(200.))
        .max_w(px(360.))
        .max_h(px(320.))
        .py(px(6.))
        .overflow_y_scroll()
        .rounded(px(10.))
        .text_color(rgb(0x2f3547))
        .bg(rgb(0xffffff))
        .shadow(panel_shadow())
}

/// `.light-theme .tab-overflow-item` (+ `.is-active` / `:hover`).
pub fn overflow_item(id: impl Into<ElementId>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .w_full()
        .gap(px(8.))
        .line_height(px(18.))
        .py(px(8.))
        .px(px(12.))
        .text_color(if active { rgb(0x58a6ff) } else { rgb(0x2f3547) })
        .bg(if active {
            color_alpha(0x58a6ff, 0.1)
        } else {
            rgba(0x00000000)
        })
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xeceef3)))
}

/// `.tab-overflow-dot`.
pub fn overflow_dot() -> Div {
    div()
        .flex_none()
        .size(px(8.))
        .rounded_full()
        .bg(rgb(0xff4524))
}

/// `.light-theme .tab-overflow-title` (+ `.is-active`).
pub fn overflow_title(title: impl Into<SharedString>, active: bool) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_normal()
        .line_clamp(1)
        .text_ellipsis()
        .text_size(px(13.))
        .text_color(if active { rgb(0x58a6ff) } else { rgb(0x2f3547) })
        .child(title.into())
}

/// `.tab-overflow-check`.
pub fn overflow_check() -> Div {
    div().flex_none().text_color(rgb(0x58a6ff)).child("✓")
}
