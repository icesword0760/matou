//! Visual shell for the Windows main-window quit confirmation.
//!
//! State, focus, events, and stacking belong to the workspace. This module only
//! assembles the same dialog chrome as the stop-session confirmation so the
//! quit prompt matches the existing tab dialogs in both themes.

use crate::tab_dialog_style as tabs_ui;
use crate::ui_text::ui_text;
use gpui::{prelude::*, *};

/// `#b63838`, the database recovery page's destructive confirmation fill.
const DANGER_BACKGROUND: u32 = 0xb63838;
/// `#ad3733`, the existing hover fill of the destructive primary dialog action.
const DANGER_HOVER: u32 = 0xad3733;

/// The modal veil used by the existing tab confirmation dialogs.
pub(super) fn backdrop() -> Stateful<Div> {
    tabs_ui::dialog_overlay("quit-confirmation-backdrop")
}

/// The centered 420px confirmation panel with the caller's copy and actions.
pub(super) fn panel(
    viewport_width: f32,
    dark: bool,
    title: &'static str,
    body: &'static str,
    cancel: Stateful<Div>,
    quit: Stateful<Div>,
) -> Stateful<Div> {
    tabs_ui::dialog_panel(viewport_width, dark)
        .id("quit-confirmation-panel")
        // The recovery page's modal confirmation already defines the elevated
        // dialog shadow. Reuse it so this prompt stays legible above both the
        // normal workspace and the recovery page.
        .shadow(vec![BoxShadow {
            color: {
                let mut color: Hsla = rgb(0x000000).into();
                color.a = 0.24;
                color
            },
            offset: point(px(0.), px(20.)),
            blur_radius: px(80.),
            spread_radius: px(0.),
        }])
        .child(tabs_ui::dialog_header().child(tabs_ui::dialog_title(title, dark)))
        .child(tabs_ui::dialog_body(dark).child(ui_text(body)))
        .child(tabs_ui::dialog_footer().child(cancel).child(quit))
}

/// A footer action matching the tab dialog button metrics.
///
/// The danger action only changes its fill and hover color; the ordinary
/// action is exactly the existing secondary dialog button. The caller adds
/// focus tracking, keyboard activation, and click handlers.
pub(super) fn button(
    id: &'static str,
    label: &'static str,
    danger: bool,
    dark: bool,
) -> Stateful<Div> {
    if !danger {
        return tabs_ui::dialog_button(id, label, false, false, dark);
    }

    // `dialog_button` hard-codes the blue primary hover and GPUI only allows a
    // single hover style, so the dangerous primary keeps its own fill here.
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(80.))
        .h(px(32.))
        .px(px(16.))
        .rounded(px(8.))
        .text_color(rgb(0xffffff))
        .bg(rgb(DANGER_BACKGROUND))
        .text_size(px(14.))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(DANGER_HOVER)))
        .child(ui_text(label).normal_line_height())
}
