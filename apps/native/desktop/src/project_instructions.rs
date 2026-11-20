//! Frozen light project-instructions entry and read-only preview dialog.
//!
//! Render-only: the caller owns the workspace/file lookup, loading and error
//! states, focus routing and close/outside-click events. This module keeps the
//! frozen light appearance and the stable element ids the workspace probes.
//!
//! Frozen sources:
//! - apps/desktop/src/renderer/src/hud/TerminalHud.tsx:162-170,418-493
//! - apps/desktop/src/renderer/src/i18n/messages/hud.ts:66-76
//! - apps/desktop/src/renderer/src/hierarchy/hierarchy.css:369,384-385,420-455,867-868

use gpui::{prelude::*, *};

use crate::ui_text::ui_text;

/// Frozen light entry: the screenshot's neutral dark label on a light gray
/// pill. The pill is the existing `#00000014` light-theme hover wash and the
/// label is its `#2f3547` text; hover deepens the same wash one step.
const ENTRY_COLOR: u32 = 0x2f3547;
const ENTRY_BG: u32 = 0x00000014;
/// `.hierarchy-shell[data-theme="light"] .is-clickable:hover`.
const ENTRY_HOVER_COLOR: u32 = 0x2f3547;
const ENTRY_HOVER_BG: u32 = 0x0000001f;
const ENTRY_PADDING_X: f32 = 6.;
const ENTRY_PADDING_Y: f32 = 1.;
const ENTRY_RADIUS: f32 = 4.;

const PANEL_WIDTH: f32 = 760.;
const PANEL_HEIGHT: f32 = 640.;
const VIEWPORT_MARGIN_X: f32 = 64.;
const VIEWPORT_MARGIN_Y: f32 = 80.;
const PANEL_PADDING: f32 = 32.;
const PANEL_RADIUS: f32 = 14.;
const HEADER_HEIGHT: f32 = 64.;
const FOOTER_HEIGHT: f32 = 54.;
const PANEL_BG: u32 = 0xffffff;
const PANEL_BORDER: u32 = 0xd0d7de;
const PANEL_TEXT: u32 = 0x24292f;
const MUTED_TEXT: u32 = 0x6e7781;
const SECTION_BORDER: u32 = 0xd8dee4;
const BODY_BG: u32 = 0xf6f8fa;
const FOCUS_RING: u32 = 0x58a6ff6b;
const OVERLAY_BG: u32 = 0x1f23283d;
const ERROR_TEXT: u32 = 0xffa198;
const ERROR_BORDER: u32 = 0xf8514940;
const ERROR_BG: u32 = 0xf8514914;
const BUTTON_BG: u32 = 0xf6f8fa;
const BUTTON_HOVER_BG: u32 = 0xeef1f4;

const TITLE: &str = "AGENTS.md";
const FALLBACK_PATH: &str = "当前项目 / AGENTS.md";
const LOADING_LABEL: &str = "正在读取…";
const CANCEL_LABEL: &str = "取消";
const CLOSE_LABEL: &str = "×";
const MONO_LINE_HEIGHT: f32 = 1.6;
const MONO_SIZE: f32 = 13.;
const MONO_PADDING_X: f32 = 20.;
const MONO_PADDING_Y: f32 = 18.;
const TAB_SIZE: usize = 2;

/// Frozen `.status-field.status-instructions.is-clickable` entry. The caller
/// supplies the label element and wires `Workspace::button` around it, which
/// owns the shared focus outline and activation.
pub(super) fn entry(label: impl IntoElement) -> Stateful<Div> {
    div()
        .id("footer-instructions")
        .flex_none()
        .flex()
        .items_center()
        .px(px(ENTRY_PADDING_X))
        .py(px(ENTRY_PADDING_Y))
        .rounded(px(ENTRY_RADIUS))
        .bg(rgba(ENTRY_BG))
        .text_color(rgb(ENTRY_COLOR))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|style| {
            style
                .text_color(rgb(ENTRY_HOVER_COLOR))
                .bg(rgba(ENTRY_HOVER_BG))
        })
        .child(label)
}

/// Frozen `.instruction-file-dialog header button` with the light-theme colors.
pub(super) fn close_button() -> Stateful<Div> {
    div()
        .id("project-instructions-close")
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(6.))
        .text_size(px(22.))
        .text_color(rgb(MUTED_TEXT))
        .cursor_pointer()
        .hover(|style| style.text_color(rgb(PANEL_TEXT)).bg(rgba(0x1f232812)))
        .child(ui_text(CLOSE_LABEL).normal_line_height())
}

/// Frozen footer cancel button. The read-only slice intentionally has no save
/// button and no save-hint copy.
pub(super) fn cancel_button() -> Stateful<Div> {
    div()
        .id("project-instructions-cancel")
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(68.))
        .h(px(30.))
        .px(px(12.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(PANEL_BORDER))
        .text_size(px(11.))
        .text_color(rgb(PANEL_TEXT))
        .bg(rgb(BUTTON_BG))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(BUTTON_HOVER_BG)))
        .child(ui_text(CANCEL_LABEL).normal_line_height())
}

/// Frozen read-only textarea replacement: a light monospace surface that keeps
/// line breaks and spaces, wraps long lines and paints the frozen
/// blue inset focus ring without changing its geometry.
pub(super) fn preview(
    content: SharedString,
    focus: &FocusHandle,
    scroll: &ScrollHandle,
    focused: bool,
) -> Stateful<Div> {
    div()
        .id("project-instructions-preview")
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .w_full()
        .track_focus(focus)
        .bg(rgb(BODY_BG))
        .text_color(rgb(PANEL_TEXT))
        .text_size(px(MONO_SIZE))
        .line_height(relative(MONO_LINE_HEIGHT))
        .font_family(monospace_family())
        .child(
            // The scroll surface is a sibling of the focus ring: an absolute
            // ring inside the scroller would move out of view with the text.
            div()
                .id("project-instructions-scroll")
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .px(px(MONO_PADDING_X))
                        .py(px(MONO_PADDING_Y))
                        // The frozen textarea wraps at the viewport width.
                        // GPUI text keeps the original whitespace when wrapping.
                        .whitespace_normal()
                        .child(display_content(content)),
                ),
        )
        .when(focused, |preview| preview.child(focus_ring()))
}

/// Frozen `.instruction-file-state` light surface. The caller chooses when to
/// mount it; the dot-dot-dot label is the frozen Chinese copy.
pub(super) fn loading() -> Stateful<Div> {
    div()
        .id("project-instructions-loading")
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(BODY_BG))
        .text_color(rgb(PANEL_TEXT))
        .text_size(px(12.))
        .child(ui_text(LOADING_LABEL).normal_line_height())
}

/// Frozen `.instruction-file-dialog` light panel. `path` is displayed verbatim
/// and falls back to the frozen "当前项目 / AGENTS.md" line when empty; the body
/// owns the remaining height, the footer only keeps the right-aligned cancel.
pub(super) fn panel(
    viewport: Size<Pixels>,
    path: SharedString,
    close: Stateful<Div>,
    body: Stateful<Div>,
    error: Option<&str>,
    cancel: Stateful<Div>,
) -> Stateful<Div> {
    let width = PANEL_WIDTH.min((f32::from(viewport.width) - VIEWPORT_MARGIN_X).max(0.));
    let height = PANEL_HEIGHT.min((f32::from(viewport.height) - VIEWPORT_MARGIN_Y).max(0.));
    let path = if path.as_ref().is_empty() {
        SharedString::from(FALLBACK_PATH)
    } else {
        path
    };

    let header = div()
        .flex_none()
        .h(px(HEADER_HEIGHT))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .px(px(18.))
        .border_b_1()
        .border_color(rgb(SECTION_BORDER))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .max_w(px(620.))
                .child(
                    div()
                        .whitespace_nowrap()
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(PANEL_TEXT))
                        .child(ui_text(TITLE).normal_line_height()),
                )
                .child(
                    div()
                        .mt(px(4.))
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED_TEXT))
                        .child(ui_text(path).normal_line_height()),
                ),
        )
        .child(close);

    let error = error.map(|error| {
        div()
            .flex_none()
            .px(px(18.))
            .py(px(8.))
            .border_t_1()
            .border_color(rgba(ERROR_BORDER))
            .bg(rgba(ERROR_BG))
            .text_color(rgb(ERROR_TEXT))
            .text_size(px(12.))
            .child(ui_text(error.to_owned()))
    });

    let footer = div()
        .flex_none()
        .h(px(FOOTER_HEIGHT))
        .flex()
        .items_center()
        .justify_end()
        .pl(px(18.))
        .pr(px(14.))
        .border_t_1()
        .border_color(rgb(SECTION_BORDER))
        .text_color(rgb(MUTED_TEXT))
        .text_size(px(11.))
        .child(cancel);

    div()
        .id("project-instructions-dialog")
        .flex()
        .flex_col()
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .rounded(px(PANEL_RADIUS))
        .border_1()
        .border_color(rgb(PANEL_BORDER))
        .bg(rgb(PANEL_BG))
        .text_color(rgb(PANEL_TEXT))
        .shadow(vec![BoxShadow {
            color: rgba(0x1f232842).into(),
            offset: point(px(0.), px(24.)),
            blur_radius: px(70.),
            spread_radius: px(0.),
        }])
        .child(header)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(body),
        )
        .children(error)
        .child(footer)
}

/// Frozen `.instruction-file-overlay` light backdrop. `occlude` keeps the
/// workspace underneath inert; the caller attaches the outside-click close and
/// keeps panel events from bubbling here.
pub(super) fn overlay(panel: Stateful<Div>) -> Stateful<Div> {
    div()
        .id("project-instructions-overlay")
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .p(px(PANEL_PADDING))
        .bg(rgba(OVERLAY_BG))
        .child(panel)
}

fn focus_ring() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            window.paint_quad(
                outline(bounds, rgba(FOCUS_RING), BorderStyle::default()).border_widths(px(2.)),
            );
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn monospace_family() -> &'static str {
    if cfg!(target_os = "macos") {
        "SF Mono"
    } else {
        "Consolas"
    }
}

/// Display-only textarea normalization: CRLF/lone CR become LF (as a browser
/// textarea value does) and the frozen `tab-size: 2` is expanded to matching
/// two-column stops. Every other newline, space and glyph is kept verbatim.
fn display_content(content: SharedString) -> SharedString {
    let source = content.as_ref();
    if !source.contains('\t') && !source.contains('\r') {
        return content;
    }
    let mut expanded = String::with_capacity(source.len());
    let mut column = 0usize;
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                characters.next_if_eq(&'\n');
                expanded.push('\n');
                column = 0;
            }
            '\n' => {
                expanded.push('\n');
                column = 0;
            }
            '\t' => {
                let spaces = TAB_SIZE - column % TAB_SIZE;
                for _ in 0..spaces {
                    expanded.push(' ');
                }
                column += spaces;
            }
            _ => {
                expanded.push(character);
                column += 1;
            }
        }
    }
    expanded.into()
}
