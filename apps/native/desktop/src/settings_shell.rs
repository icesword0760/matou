//! Frozen settings shell for Claude Code provider management.
//!
//! Scope of this level:
//! - the bottom-bar entry button and the full right-workspace page shell;
//! - the two frozen navigation entries under static section headings
//!   (Claude Code selected by default);
//! - the Claude Code provider cards (display, switch, add, edit);
//! - the API provider picker and add/edit dialog.
//!
//! Deferred by the assignment and deliberately not rendered as fake controls:
//! - subscription sign-in and refresh;
//! - the compaction block below the Claude model assignments.
//!
//! GPUI has no ARIA surface; keyboard-operable buttons and the selected/pressed
//! visual state stand in for `aria-pressed` / `aria-current` and this limitation
//! is reported to the integrator.
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use gpui::{prelude::*, *};
use matou_native_core::claude_role_budget::{
    CLAUDE_ROLE_LABELS as ROLE_LABELS, CLAUDE_ROLES as ROLES,
};
use matou_native_core::context_optimizer_config::ContextOptimizerConfig;
use matou_native_core::dispatch_config::{
    DispatchConfig,
    form::{DispatchForm, LOAD_FAILED, RETRY_LABEL},
};
use matou_native_core::sessions::{InterfaceLocale, LocalePreference};
use matou_native_desktop::context_optimizer_settings::ContextOptimizerSettingsPanel;
use matou_native_desktop::dispatch_settings::DispatchSettingsPanel;
use matou_native_desktop::provider_context_editor::{
    ACTIVE_PROVIDER_BOTTOM_EDGE_RGB, context_mode_subtitle, context_mode_title,
    context_mode_tokens, context_multi_model_hint, context_unknown_hint, context_window_badge_view,
    format_tokens, provider_switch_impact_row, provider_window_badge_label, resolve_context_tokens,
};
use matou_native_desktop::theme_tokens::{
    AppTheme, ThemePalette, ThemePreference, ThemeState, theme_preference_picker,
};
use matou_native_runtime::protocol::{
    ClaudeModelCatalogReply, ClaudeModelPreferencesReply, ClaudeRoleAssignmentsReply,
    TimeZoneSettingsReply, TimeZoneSettingsSnapshot,
};
use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

use crate::provider_settings::{DELETE_FAILED, ProviderDraft, ProviderSettings, ProviderUpdate};
use crate::rename_input::{RenameInput, RenameInputEvent};
use crate::runtime_bridge::{Command, ConnectionPhase, RuntimeCommands};
use crate::terminal_input::read_clipboard_text;

// The desktop probes compile the settings shell directly alongside a test transport.
#[path = "claude_role_assignments.rs"]
pub mod claude_role_assignments;
#[path = "compaction_settings.rs"]
pub mod compaction_settings;
#[path = "subscription_settings.rs"]
pub mod subscription_settings;
#[cfg(any(test, debug_assertions))]
use self::claude_role_assignments::AssignmentPanel;
use self::claude_role_assignments::{
    AssignmentAction, AssignmentView, ClaudeAssignments, is_official_endpoint, official,
};
use self::compaction_settings::{
    CompactionDraft, CompactionOption, effort_fallback, effort_label as compaction_effort_label,
    effort_values, model_name as compaction_model_name, model_options, provider_available,
    provider_options, selected_provider, validation as compaction_validation,
};
use self::subscription_settings::{
    normalize_subscription_draft, quota_text, quota_title, subscription_kind,
};

// Frozen light palette (`hierarchy.css` / `provider-setup.css`).
pub(super) const PANEL: u32 = 0xffffff;
pub(super) const PANEL_MUTED: u32 = 0xfbfcfd;
pub(super) const TEXT: u32 = 0x303744;
pub(super) const MUTED: u32 = 0x7e8794;
const FAINT: u32 = 0xa3aab4;
pub(super) const ACCENT: u32 = 0x347fd6;
const ACCENT_LINE: u32 = 0x347fd647; // rgba(52,127,214,.28)
pub(super) const LINE: u32 = 0x424b581a; // rgba(66,75,88,.1)
pub(super) const LINE_STRONG: u32 = 0x424b5829; // rgba(66,75,88,.16)
pub(super) const DANGER_TEXT: u32 = 0xa13d39;
pub(super) const DANGER_BG: u32 = 0xfff2f1;
const DANGER_LINE: u32 = 0xf4d4d1;
const NOTICE_BG: u32 = 0xeaf4ff;
const NOTICE_LINE: u32 = 0xd1e6fc;
const CONFIRM_BG: u32 = 0xfff7e8;
const CONFIRM_LINE: u32 = 0xebd6af;
const DIALOG_BORDER: u32 = 0xe0e6ee;
const DIALOG_LABEL: u32 = 0x344257;
const DIALOG_MUTED: u32 = 0x78879b;
const DIALOG_INPUT_LINE: u32 = 0xd8dfe8;
const DIALOG_FOCUS: u32 = 0x5c9ced;
const DIALOG_FOCUS_RING: u32 = 0xe8f1ff;
const DIALOG_PRIMARY: u32 = 0x3284ed;
const DIALOG_PRIMARY_HOVER: u32 = 0x2274df;
const DIALOG_HEADER_LINE: u32 = 0xedf0f4;
const DIALOG_FOOTER_LINE: u32 = 0xe7ecf3;
const PRESET_ICON_BG: u32 = 0xedf1f6;
const PRESET_ICON_FG: u32 = 0x52647d;
const SEARCH_PLACEHOLDER: u32 = 0x8c99aa;
const SETTINGS_NAV_WIDTH: f32 = 176.;
const SETTINGS_NAV_WIDTH_NARROW: f32 = 146.;
const WORKSPACE_SIDEBAR_WIDTH: f32 = 260.;
const SETTINGS_ANIMATION_MS: u64 = 180;
const DIALOG_ANIMATION_MS: u64 = 160;
const CLI_DEFAULT_MODEL: &str = "CLI 默认";

/// Resolved colors for the frozen provider setup dialog and its status cards.
///
/// Light keeps the literal `provider-setup.css` values (including the white
/// input surface) so the frozen screenshot baseline does not move. Dark maps
/// the dialog shell, labels, selects, menus, and error/confirm cards onto the
/// active workspace/settings palette. The text fields use the same palette
/// for their surfaces, glyphs and carets.
#[derive(Clone, Copy)]
struct EditorColors {
    dialog: u32,
    panel: u32,
    panel_muted: u32,
    label: u32,
    muted: u32,
    subtitle: u32,
    section_title: u32,
    header_icon: u32,
    header_icon_hover: u32,
    secondary_text: u32,
    role_text: u32,
    hint_text: u32,
    notice_body: u32,
    line: u32,
    line_strong: u32,
    header_line: u32,
    footer_line: u32,
    focus: u32,
    focus_ring: u32,
    input_background: u32,
    input_line: u32,
    input_text: Option<u32>,
    input_placeholder: Option<u32>,
    primary: u32,
    primary_hover: u32,
    link: u32,
    link_hover: u32,
    card_line: u32,
    card_selected_line: u32,
    card_hover_line: u32,
    card_selected_bg: u32,
    card_hover_bg: u32,
    card_marker: u32,
    check: u32,
    icon_bg: u32,
    icon_fg: u32,
    info_bg: u32,
    info_line: u32,
    menu_selected_bg: u32,
    error_text: u32,
    error_border: u32,
    error_bg: u32,
    error_line: u32,
    confirm_bg: u32,
    confirm_line: u32,
}

impl EditorColors {
    fn new(palette: &ThemePalette, dark: bool) -> Self {
        if dark {
            Self {
                dialog: palette.settings.dialog,
                panel: palette.workspace.panel,
                panel_muted: palette.workspace.panel_muted,
                label: palette.settings.dialog_label,
                muted: palette.workspace.muted,
                subtitle: palette.workspace.muted,
                section_title: palette.workspace.muted,
                header_icon: palette.workspace.muted,
                header_icon_hover: palette.settings.language_button_hover,
                secondary_text: palette.workspace.text,
                role_text: palette.workspace.muted,
                hint_text: palette.workspace.muted,
                notice_body: palette.workspace.muted,
                line: palette.workspace.line,
                line_strong: palette.workspace.line_strong,
                header_line: palette.workspace.line,
                footer_line: palette.workspace.line,
                focus: palette.settings.input_focus_line,
                focus_ring: palette.settings.input_focus_ring,
                input_background: palette.settings.input_background,
                input_line: palette.workspace.line_strong,
                input_text: Some(palette.settings.input_text),
                input_placeholder: Some(palette.workspace.muted),
                // The dark workspace accent is a light blue; pair primary
                // buttons with a readable dark-blue base and white text.
                primary: 0x2f79dfff,
                primary_hover: 0x2274dfff,
                link: palette.settings.language_menu_text,
                link_hover: palette.workspace.accent,
                card_line: palette.workspace.line_strong,
                card_selected_line: palette.workspace.accent,
                card_hover_line: palette.workspace.accent_line,
                card_selected_bg: palette.settings.provider_selected,
                card_hover_bg: palette.settings.language_button_hover,
                card_marker: palette.workspace.faint,
                check: palette.workspace.accent,
                icon_bg: palette.workspace.panel_muted,
                icon_fg: palette.workspace.muted,
                info_bg: palette.settings.language_button_hover,
                info_line: palette.workspace.line,
                menu_selected_bg: palette.settings.language_menu_background,
                error_text: palette.settings.language_error,
                error_border: palette.settings.language_error,
                error_bg: 0x2a1c1cff,
                error_line: 0x4a2a28ff,
                confirm_bg: 0x2a2418ff,
                confirm_line: 0x5a4a2aff,
            }
        } else {
            Self {
                dialog: 0xffffffff,
                panel: 0xffffffff,
                panel_muted: 0xf2f5f9ff,
                label: 0x344257ff,
                muted: opaque(DIALOG_MUTED),
                subtitle: 0x7c899cff,
                section_title: 0x6f7d91ff,
                header_icon: 0x8694a7ff,
                header_icon_hover: 0xf0f4f8ff,
                secondary_text: 0x3b4b60ff,
                role_text: 0x667991ff,
                hint_text: 0x8190a4ff,
                notice_body: 0x596575ff,
                line: opaque(DIALOG_BORDER),
                line_strong: 0xd8dfe8ff,
                header_line: opaque(DIALOG_HEADER_LINE),
                footer_line: opaque(DIALOG_FOOTER_LINE),
                focus: opaque(DIALOG_FOCUS),
                focus_ring: opaque(DIALOG_FOCUS_RING),
                input_background: 0xffffffff,
                input_line: opaque(DIALOG_INPUT_LINE),
                input_text: None,
                input_placeholder: None,
                primary: opaque(DIALOG_PRIMARY),
                primary_hover: opaque(DIALOG_PRIMARY_HOVER),
                link: 0x2679d8ff,
                link_hover: 0x1f6dc4ff,
                card_line: 0xdde4edff,
                card_selected_line: 0x4d92f3ff,
                card_hover_line: 0x8cb7f1ff,
                card_selected_bg: 0xedf5ffff,
                card_hover_bg: 0xf7faffff,
                card_marker: 0x9aaac0ff,
                check: 0x3284edff,
                icon_bg: 0xedf1f6ff,
                icon_fg: 0x52647dff,
                info_bg: 0xf7faffff,
                info_line: 0xdce6f4ff,
                menu_selected_bg: 0xe4efffff,
                error_text: 0xa13d39ff,
                error_border: 0xb34d47ff,
                error_bg: 0xfff2f1ff,
                error_line: (DANGER_LINE << 8) | 0xff,
                confirm_bg: opaque(CONFIRM_BG),
                confirm_line: opaque(CONFIRM_LINE),
            }
        }
    }
}

/// Expands a frozen 24-bit CSS color to an opaque packed RGBA value.
const fn opaque(hex: u32) -> u32 {
    (hex << 8) | 0xff
}

pub(super) fn color_alpha(hex: u32, alpha: f32) -> Rgba {
    let mut color = rgb(hex);
    color.a = alpha;
    color
}

pub(super) fn icon(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

pub(super) fn shadow(color: u32, alpha: f32, y: f32, blur: f32) -> BoxShadow {
    BoxShadow {
        color: color_alpha(color, alpha).into(),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
    }
}

fn outline_shadow(color: u32, spread: f32) -> BoxShadow {
    BoxShadow {
        color: rgba(color).into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(spread),
    }
}

/// CSS ease-out, cubic-bezier(0, 0, .58, 1).
fn surface_easing(progress: f32) -> f32 {
    if !(0.0..1.0).contains(&progress) || progress == 0. {
        return progress.clamp(0., 1.);
    }
    let mut low = 0.;
    let mut high = 1.;
    for _ in 0..18 {
        let parameter = (low + high) * 0.5;
        let x = parameter * parameter * (1.74 - 0.74 * parameter);
        if x < progress {
            low = parameter;
        } else {
            high = parameter;
        }
    }
    let parameter = (low + high) * 0.5;
    parameter * parameter * (3. - 2. * parameter)
}

fn reduced_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::reduced_motion()
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
        };
        let mut enabled: i32 = 1;
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                &mut enabled as *mut _ as _,
                0,
            );
        }
        enabled == 0
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

#[derive(Clone, Copy)]
pub(super) enum FocusRingStyle {
    /// `.model-settings__heading > button:focus-visible` etc.: 2px accent-line,
    /// 1px outside the control.
    Shell,
    /// `.provider-setup button:focus-visible`: 3px #b9d8ff, 2px outside.
    Dialog,
}

pub(super) fn focus_ring_with(
    focus: FocusHandle,
    radius: f32,
    keyboard_focus: bool,
    style: FocusRingStyle,
) -> AnyElement {
    let (color, width, offset) = match style {
        FocusRingStyle::Shell => (ACCENT_LINE, 2., 1.),
        FocusRingStyle::Dialog => (0xb9d8ff, 3., 2.),
    };
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if keyboard_focus && focus.is_focused(window) {
                let outer = Bounds::new(
                    point(bounds.origin.x - px(offset), bounds.origin.y - px(offset)),
                    size(
                        bounds.size.width + px(offset * 2.),
                        bounds.size.height + px(offset * 2.),
                    ),
                );
                window.paint_quad(
                    outline(outer, rgb(color), BorderStyle::default())
                        .corner_radii(px(radius + offset))
                        .border_widths(px(width)),
                );
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
    .into_any_element()
}

fn dialog_focus_ring(focus: FocusHandle, radius: f32, keyboard_focus: bool) -> AnyElement {
    focus_ring_with(focus, radius, keyboard_focus, FocusRingStyle::Dialog)
}

fn input_focus_shadow(colors: &EditorColors) -> Vec<BoxShadow> {
    vec![outline_shadow(colors.focus_ring, 3.)]
}

// ---------------------------------------------------------------------------
// Masked API-key input
// ---------------------------------------------------------------------------

pub(crate) enum SecretInputEvent {
    Changed,
    Confirm,
    Cancel,
}

/// Masked single-line input for the provider API key.
///
/// The real value never reaches the painted text: only one bullet per grapheme
/// is shaped and painted. Copy/cut/reveal are intentionally not implemented and
/// the input starts empty; the saved snapshot key is never read by this module.
struct SecretInput {
    focus: FocusHandle,
    placeholder: SharedString,
    text: String,
    /// Dialog-only visibility toggle. The runtime contract still redacts saved
    /// keys; this flag never changes what is stored or sent.
    revealed: bool,
    /// Optional paint overrides for the themed settings dialog. `None` keeps
    /// the frozen light values.
    text_color: Option<Rgba>,
    placeholder_color: Option<Rgba>,
    selection: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    before_composition: Option<(String, Range<usize>, bool)>,
    layout: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    selecting: bool,
}

impl EventEmitter<SecretInputEvent> for SecretInput {}

impl SecretInput {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            placeholder: SharedString::default(),
            text: String::new(),
            revealed: false,
            text_color: None,
            placeholder_color: None,
            selection: 0..0,
            reversed: false,
            marked: None,
            before_composition: None,
            layout: None,
            bounds: None,
            scroll: px(0.),
            selecting: false,
        }
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    fn to_byte(text: &str, utf16: usize) -> usize {
        let mut units = 0;
        for (offset, ch) in text.char_indices() {
            if units >= utf16 {
                return offset;
            }
            units += ch.len_utf16();
        }
        text.len()
    }

    fn to_utf16(&self, offset: usize) -> usize {
        self.text[..offset].encode_utf16().count()
    }

    fn bytes(&self, range: Range<usize>) -> Range<usize> {
        Self::to_byte(&self.text, range.start)..Self::to_byte(&self.text, range.end)
    }

    fn grapheme_count_before(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .graphemes(true)
            .count()
    }

    fn byte_for_grapheme(&self, index: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .nth(index)
            .map_or(self.text.len(), |(offset, _)| offset)
    }

    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.before_composition = None;
        self.text.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.selection = end..end;
        self.reversed = false;
        self.marked = None;
        cx.emit(SecretInputEvent::Changed);
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, extend: bool, cx: &mut Context<Self>) {
        let anchor = if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        };
        if extend {
            self.reversed = offset < anchor;
            self.selection = offset.min(anchor)..offset.max(anchor);
        } else {
            self.reversed = false;
            self.selection = offset..offset;
        }
        cx.notify();
    }

    fn previous(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.text.len())
    }

    fn hit(&self, position: Point<Pixels>) -> usize {
        self.bounds
            .zip(self.layout.as_ref())
            .map(|(bounds, layout)| {
                let index = layout
                    .closest_index_for_x(position.x - bounds.left() + self.scroll)
                    .min(self.display().len());
                let bullets = self.display()[..index].chars().count();
                self.byte_for_grapheme(bullets)
            })
            .unwrap_or(0)
    }

    fn display(&self) -> String {
        if self.text.is_empty() {
            self.placeholder.to_string()
        } else if self.revealed {
            self.text.clone()
        } else {
            // One bullet per user-perceived character, never the real key.
            "\u{2022}".repeat(self.text.graphemes(true).count())
        }
    }

    fn display_index(&self, byte: usize) -> usize {
        if self.revealed {
            byte.min(self.text.len())
        } else {
            self.grapheme_count_before(byte) * '\u{2022}'.len_utf8()
        }
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke.key;
        let modifiers = &event.keystroke.modifiers;
        let shortcut = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        };
        let word = if cfg!(target_os = "macos") {
            modifiers.alt
        } else {
            modifiers.control
        };
        if self.marked.is_some() && !matches!(key.as_str(), "escape") {
            return;
        }
        match key.as_str() {
            "escape" => cx.emit(SecretInputEvent::Cancel),
            "enter" => cx.emit(SecretInputEvent::Confirm),
            "a" if shortcut => {
                self.selection = 0..self.text.len();
                self.reversed = false;
            }
            "v" if shortcut => {
                if let Some(text) = read_clipboard_text(cx) {
                    self.replace(self.selection.clone(), &text, cx);
                }
            }
            "left" | "right" | "home" | "end" => {
                let end = key == "right" || key == "end";
                let cursor = self.cursor();
                let offset = if key == "home" || key == "end" {
                    if end { self.text.len() } else { 0 }
                } else if !modifiers.shift && !self.selection.is_empty() {
                    if end {
                        self.selection.end
                    } else {
                        self.selection.start
                    }
                } else if word && end {
                    self.text
                        .unicode_word_indices()
                        .map(|(index, word)| {
                            if cfg!(target_os = "windows") {
                                index
                            } else {
                                index + word.len()
                            }
                        })
                        .find(|index| *index > cursor)
                        .unwrap_or(self.text.len())
                } else if word {
                    self.text
                        .unicode_word_indices()
                        .take_while(|(index, _)| *index < cursor)
                        .last()
                        .map_or(0, |(index, _)| index)
                } else if end {
                    self.next(cursor)
                } else {
                    self.previous(cursor)
                };
                self.move_to(offset, modifiers.shift, cx);
            }
            "backspace" | "delete" => {
                let mut range = self.selection.clone();
                if range.is_empty() {
                    let cursor = self.cursor();
                    if key == "backspace" {
                        range.start = self.previous(cursor);
                    } else {
                        range.end = self.next(cursor);
                    }
                    self.selection = range.clone();
                }
                self.replace(range, "", cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn paint(
        &mut self,
        entity: Entity<Self>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let display = self.display();
        let empty = self.text.is_empty();
        let default_color = if empty {
            rgba(SEARCH_PLACEHOLDER)
        } else {
            rgba(DIALOG_LABEL)
        };
        let color = if empty {
            self.placeholder_color.unwrap_or(default_color)
        } else {
            self.text_color.unwrap_or(default_color)
        };
        let run = TextRun {
            len: display.len(),
            font: window.text_style().font(),
            color: color.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let display_len = display.len();
        let line = window
            .text_system()
            .shape_line(display.into(), px(12.), &[run], None);
        let cursor_x = line.x_for_index(self.display_index(self.cursor()));
        self.scroll = self
            .scroll
            .min((line.x_for_index(display_len) - bounds.size.width + px(1.)).max(px(0.)));
        if cursor_x - self.scroll > bounds.size.width - px(1.) {
            self.scroll = (cursor_x - bounds.size.width + px(1.)).max(px(0.));
        }
        if cursor_x < self.scroll {
            self.scroll = cursor_x;
        }
        if empty {
            self.scroll = px(0.);
        }
        let origin = point(bounds.left() - self.scroll, bounds.top());
        window.handle_input(&self.focus, ElementInputHandler::new(bounds, entity), cx);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if !empty && !self.selection.is_empty() {
                let start = self.display_index(self.selection.start.min(self.text.len()));
                let end = self.display_index(self.selection.end.min(self.text.len()));
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(origin.x + line.x_for_index(start), bounds.top()),
                        point(origin.x + line.x_for_index(end), bounds.bottom()),
                    ),
                    rgba(0x347fd64d),
                ));
            }
            let _ = line.paint(origin, bounds.size.height, window, cx);
            if self.focus.is_focused(window) && self.selection.is_empty() {
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x + cursor_x, bounds.top() + px(2.)),
                        size(px(1.), bounds.size.height - px(4.)),
                    ),
                    self.text_color.unwrap_or_else(|| rgb(TEXT)),
                ));
            }
        });
        self.bounds = Some(bounds);
        self.layout = Some(line);
    }
}

impl EntityInputHandler for SecretInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.bytes(range);
        *actual = Some(self.to_utf16(range.start)..self.to_utf16(range.end));
        if self.revealed {
            return Some(self.text[range].to_owned());
        }
        // Never hand the masked key to the OS text query / IME. Keep the same
        // UTF-16 length so platform ranges remain aligned; each bullet is one unit.
        Some("\u{2022}".repeat(self.text[range].encode_utf16().count()))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.to_utf16(self.selection.start)..self.to_utf16(self.selection.end),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| self.to_utf16(range.start)..self.to_utf16(range.end))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(range) = self.marked.clone() {
            let text = self.text[range].to_owned();
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.bytes(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        self.replace(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            if let Some((value, selection, reversed)) = self.before_composition.take() {
                self.text = value;
                self.selection = selection;
                self.reversed = reversed;
                self.marked = None;
                cx.emit(SecretInputEvent::Changed);
                cx.notify();
            }
            return;
        }
        if self.before_composition.is_none() {
            self.before_composition =
                Some((self.text.clone(), self.selection.clone(), self.reversed));
        }
        let range = range
            .map(|range| self.bytes(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        self.text.replace_range(range.clone(), text);
        self.marked = Some(range.start..range.start + text.len());
        self.selection = selected
            .map(|range| {
                range.start + Self::to_byte(text, range.start)
                    ..range.start + Self::to_byte(text, range.end)
            })
            .unwrap_or(range.start + text.len()..range.start + text.len());
        self.reversed = false;
        cx.emit(SecretInputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.bytes(range);
        let bounds = self.bounds?;
        let layout = self.layout.as_ref()?;
        let start = self.display_index(range.start);
        let end = self.display_index(range.end);
        Some(Bounds::from_corners(
            point(
                bounds.left() + layout.x_for_index(start) - self.scroll,
                bounds.top(),
            ),
            point(
                bounds.left() + layout.x_for_index(end) - self.scroll,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.to_utf16(self.hit(point)))
    }
}

impl Render for SecretInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id("provider-secret-input")
            .track_focus(&self.focus)
            .relative()
            .w_full()
            .h(px(24.))
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.selecting = true;
                    let offset = this.hit(event.position);
                    this.move_to(offset, event.modifiers.shift, cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.selecting {
                    this.move_to(this.hit(event.position), true, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        entity.update(cx, |this, cx| {
                            this.paint(entity.clone(), bounds, window, cx)
                        });
                    },
                )
                .w_full()
                .h(px(20.))
                .mt(px(2.)),
            )
    }
}

// ---------------------------------------------------------------------------
// Static frozen data
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct ProviderPreset {
    id: &'static str,
    name: &'static str,
    endpoint: &'static str,
    key_url: &'static str,
    model: &'static str,
    mapping: [&'static str; ROLES.len()],
}

/// `PROVIDER_PRESETS['claude-code']` from the frozen contracts source.
const PRESETS: [ProviderPreset; 4] = [
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        endpoint: "https://api.deepseek.com/anthropic",
        key_url: "https://platform.deepseek.com/api_keys",
        model: "deepseek-v4-pro",
        mapping: ["deepseek-v4-pro", "", "deepseek-v4-pro", "deepseek-flash"],
    },
    ProviderPreset {
        id: "glm",
        name: "智谱 GLM",
        endpoint: "https://open.bigmodel.cn/api/anthropic",
        key_url: "https://open.bigmodel.cn",
        model: "glm-5.3",
        mapping: ["glm-5.3", "", "glm-5.3", "glm-5.3"],
    },
    ProviderPreset {
        id: "kimi",
        name: "Kimi",
        endpoint: "https://api.moonshot.cn/anthropic",
        key_url: "https://platform.kimi.com/console/api-keys",
        model: "kimi-k2.7-code",
        mapping: ["kimi-k2.7-code", "", "kimi-k2.7-code", "kimi-k2.7-code"],
    },
    ProviderPreset {
        id: "minimax",
        name: "MiniMax",
        endpoint: "https://api.minimax.cn/anthropic",
        key_url: "https://platform.minimax.cn/subscribe/token-plan",
        model: "MiniMax-M3[1M]",
        mapping: ["MiniMax-M3[1M]", "", "MiniMax-M3[1M]", "MiniMax-M3[1M]"],
    },
];

const CODEX_PRESETS: [ProviderPreset; 4] = [
    ProviderPreset {
        endpoint: "https://api.deepseek.com",
        model: "deepseek-flash",
        ..PRESETS[0]
    },
    ProviderPreset {
        endpoint: "https://open.bigmodel.cn/api/v1",
        ..PRESETS[1]
    },
    ProviderPreset {
        endpoint: "https://api.moonshot.cn/v1",
        model: "kimi-k3",
        ..PRESETS[2]
    },
    ProviderPreset {
        endpoint: "https://api.minimax.cn/v1",
        model: "MiniMax-M3",
        ..PRESETS[3]
    },
];

fn provider_preset_for_endpoint(codex: bool, endpoint: &str) -> Option<&'static ProviderPreset> {
    let presets = if codex { &CODEX_PRESETS } else { &PRESETS };
    let endpoint = endpoint.trim().strip_suffix('/').unwrap_or(endpoint.trim());
    presets.iter().find(|preset| preset.endpoint == endpoint)
}

fn provider_preset_badge(preset: Option<&ProviderPreset>, name: &str) -> Div {
    let (background, text) = match preset.map(|preset| preset.id) {
        Some("deepseek") => (0x4a85ee, 0xffffff),
        Some("glm") => (0xeeeaff, 0x6756de),
        Some("kimi") => (0x26303a, 0xffffff),
        Some("minimax") => (0xed386e, 0xffffff),
        _ => (PRESET_ICON_BG, PRESET_ICON_FG),
    };
    let glyph = preset
        .map_or(name, |preset| preset.name)
        .trim()
        .chars()
        .next()
        .unwrap_or('＋');
    div()
        .size(px(38.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(9.))
        .bg(rgb(background))
        .text_size(px(24.))
        .font_weight(FontWeight(650.))
        .text_color(rgb(text))
        .child(glyph.to_string())
}

fn preset_by_id(id: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

fn preset_by_endpoint(endpoint: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|preset| preset.endpoint == endpoint)
}

// ---------------------------------------------------------------------------
// Navigation and editor state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    ClaudeCode,
    Language,
    Dispatch,
    Diagnostics,
    ContextOptimizer,
}

impl SettingsPage {
    fn title(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Language => locale.text("语言与时区", "Language & time zone"),
            Self::Dispatch => locale.text("派活", "Dispatch"),
            Self::Diagnostics => locale.text("诊断", "Diagnostics"),
            Self::ContextOptimizer => locale.text("上下文优化", "Context optimizer"),
        }
    }

    fn subtitle(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::ClaudeCode => locale.text("配置 Claude Code 的服务供应商、各通道上下文窗口与对话压缩模型。", "Configure Claude Code providers, context windows and compaction model."),
            Self::Language => locale.text("选择界面语言，并设置码头启动 CLI 进程的运行环境时区。", "Choose the interface language and the time zone for CLI processes launched by Matou."),
            // The dispatch panel already renders its own heading and copy; this
            // subtitle is only a fallback for non-rendered metadata.
            Self::Dispatch => locale.text(
                "设置新派任务的全局默认执行者、模型、时限与重试次数。",
                "Set the global defaults for new dispatched tasks: executor, model, time limits and retries.",
            ),
            Self::Diagnostics => locale.text(
                "只读查看账号订阅、运行环境与受控浏览器状态，并手动运行保留清理。",
                "Read-only account subscription, runtime environment and controlled browser status, with a manual retention cleanup.",
            ),
            Self::ContextOptimizer => locale.text(
                "按 Agent 独立开关上下文噪音剪裁、防重复读、KV-Cache 倒计时与卡死救援（零 MCP 内置服务）。",
                "Configure per-agent noise trimming, read dedup, KV-cache monitoring, and stall rescue (zero-MCP built-in service).",
            ),
        }
    }

    fn index(self) -> usize {
        match self {
            Self::ClaudeCode => 0,
            Self::Language => 1,
            Self::Dispatch => 2,
            Self::Diagnostics => 3,
            Self::ContextOptimizer => 4,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorStep {
    Pick,
    Form,
    /// The OAuth/risk dialog before an account draft exists. The phase itself
    /// lives in `ProviderSettings`; this step only selects the renderer.
    Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickChoice {
    Preset(&'static str),
    Subscription(&'static str),
    Custom,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmKind {
    Close,
    Back,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderSelect {
    Mapping(usize),
    Effort(usize),
}

impl ProviderSelect {
    fn slot(self) -> usize {
        match self {
            Self::Mapping(index) => index,
            Self::Effort(index) => index + ROLES.len(),
        }
    }
}

enum ProviderChoice {
    Model(String),
    Manual,
    Effort(String),
}

struct SelectOption {
    choice: ProviderChoice,
    label: String,
    detail: Option<String>,
    selected: bool,
    enabled: bool,
}

const CONTEXT_MODES: [&str; 3] = ["default", "512k", "maximum"];

struct EditorFocus {
    close: FocusHandle,
    back: FocusHandle,
    presets: [FocusHandle; 4],
    subscription_choices: [FocusHandle; 2],
    custom: FocusHandle,
    change: FocusHandle,
    cancel: FocusHandle,
    next: FocusHandle,
    key: FocusHandle,
    reveal: FocusHandle,
    key_link: FocusHandle,
    endpoint: FocusHandle,
    model: FocusHandle,
    fetch: FocusHandle,
    chooser: FocusHandle,
    model_search: FocusHandle,
    name: FocusHandle,
    mappings: [FocusHandle; ROLES.len()],
    mapping_choices: [FocusHandle; ROLES.len()],
    manual_links: [FocusHandle; ROLES.len()],
    efforts: [FocusHandle; ROLES.len()],
    context_modes: [FocusHandle; 3],
    delete: FocusHandle,
    save: FocusHandle,
    save_switch: FocusHandle,
    keep_editing: FocusHandle,
    confirm_action: FocusHandle,
    error: FocusHandle,
    subscription_risk: FocusHandle,
    subscription_primary: FocusHandle,
    subscription_cancel: FocusHandle,
    subscription_reopen: FocusHandle,
    subscription_system_browser: FocusHandle,
}

struct Editor {
    step: EditorStep,
    /// Present only while the dialog belongs to a subscription sign-in or the
    /// draft that sign-in produced. `None` keeps the ordinary API editor.
    subscription: Option<SubscriptionFlow>,
    query: Entity<RenameInput>,
    selection: Option<PickChoice>,
    draft: ProviderDraft,
    baseline: FormValues,
    baseline_metadata: Value,
    context_touched: bool,
    connection_changed: bool,
    key: Entity<SecretInput>,
    endpoint: Entity<RenameInput>,
    model: Entity<RenameInput>,
    name: Entity<RenameInput>,
    mappings: [Entity<RenameInput>; ROLES.len()],
    efforts: [String; ROLES.len()],
    manual_mappings: [bool; ROLES.len()],
    model_query: Entity<RenameInput>,
    model_chooser: bool,
    fetch_error: Option<String>,
    fetch_snapshot: Option<(String, String)>,
    revealed_key: bool,
    select_open: Option<(ProviderSelect, usize)>,
    select_bounds: Rc<RefCell<[Option<Bounds<Pixels>>; ROLES.len() * 2]>>,
    select_scroll: ScrollHandle,
    model_scroll: ScrollHandle,
    model_cursor: usize,
    error: Option<String>,
    confirm: Option<ConfirmKind>,
    confirm_return: Option<(Option<FocusHandle>, Point<Pixels>)>,
    opener: FocusHandle,
    focus: EditorFocus,
    body_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

/// UI-only state of one subscription dialog. Risk acceptance and the current
/// phase live in `ProviderSettings`; this struct keeps the checkbox, provider
/// kind and the single polling chain's generation.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SubscriptionFlow {
    provider: String,
    existing: bool,
    /// Saved identity kept while re-authenticating. The runtime preserves the
    /// effective models and name too; keeping these lets a late or partial
    /// draft still update the same row.
    existing_id: Option<String>,
    existing_name: Option<String>,
    risk_checked: bool,
    /// Incremented whenever the dialog closes or becomes a different attempt so
    /// a late timer cannot apply a reply to the next flow.
    generation: u64,
    /// One chain per dialog: true from scheduling until the timer runs.
    polling: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompactionSelect {
    Provider,
    Model,
    Effort,
}

impl CompactionSelect {
    fn index(self) -> usize {
        match self {
            Self::Provider => 0,
            Self::Model => 1,
            Self::Effort => 2,
        }
    }

    fn menu_min_width(self) -> f32 {
        match self {
            Self::Provider => 240.,
            Self::Model => 220.,
            Self::Effort => 150.,
        }
    }
}

struct CompactionMenu {
    select: CompactionSelect,
    active: usize,
    items: Vec<CompactionOption>,
    selected: String,
    trigger_focus: FocusHandle,
}

/// Editable state for the compaction block. Persistence belongs to
/// `ProviderSettings`; this keeps the user's in-progress choices until a reply
/// confirms them.
struct CompactionUi {
    draft: Option<CompactionDraft>,
    baseline: Option<Value>,
    manual_model: bool,
    saved: bool,
    was_saving: bool,
    open: Option<CompactionMenu>,
    bounds: Rc<RefCell<[Option<Bounds<Pixels>>; 3]>>,
    scroll: ScrollHandle,
    focus: [FocusHandle; 3],
    mode_focus: [FocusHandle; 2],
    save_focus: FocusHandle,
    retry_focus: FocusHandle,
    relogin_focus: FocusHandle,
    manual_input: Entity<RenameInput>,
    _subscriptions: Vec<Subscription>,
}

impl CompactionUi {
    fn new(window: &mut Window, cx: &mut Context<SettingsShell>) -> Self {
        let manual_input = cx.new(|cx| {
            let mut input = RenameInput::new(String::new(), cx);
            input.search_style = true;
            input.placeholder = "填写模型 ID".into();
            input
        });
        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.subscribe_in(
                &manual_input,
                window,
                |this, _, event, window, cx| match event {
                    RenameInputEvent::Changed => {
                        let text = this.compaction.manual_input.read(cx).text.clone();
                        if let Some(draft) = &mut this.compaction.draft
                            && draft.model != text
                        {
                            draft.model = text;
                            this.compaction.saved = false;
                            cx.notify();
                        }
                    }
                    RenameInputEvent::Confirm => {
                        let focus = this.compaction.save_focus.clone();
                        window.focus(&focus);
                        cx.notify();
                    }
                    RenameInputEvent::Cancel => {
                        let focus = this.compaction.focus[CompactionSelect::Model.index()].clone();
                        window.focus(&focus);
                        cx.notify();
                    }
                },
            ),
        );
        Self {
            draft: None,
            baseline: None,
            manual_model: false,
            saved: false,
            was_saving: false,
            open: None,
            bounds: Rc::new(RefCell::new([None; 3])),
            scroll: ScrollHandle::new(),
            focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            mode_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            save_focus: cx.focus_handle().tab_stop(true),
            retry_focus: cx.focus_handle().tab_stop(true),
            relogin_focus: cx.focus_handle().tab_stop(true),
            manual_input,
            _subscriptions: subscriptions,
        }
    }

    fn reset(&mut self) {
        self.draft = None;
        self.baseline = None;
        self.manual_model = false;
        self.saved = false;
        self.was_saving = false;
        self.open = None;
    }
}

#[derive(Clone, PartialEq)]
struct FormValues {
    name: String,
    endpoint: String,
    model: String,
    key: String,
    mappings: [String; ROLES.len()],
    efforts: [String; ROLES.len()],
    reasoning_effort: String,
    context_mode: String,
}

fn input_entity(
    value: impl Into<String>,
    placeholder: impl Into<String>,
    cx: &mut Context<SettingsShell>,
) -> Entity<RenameInput> {
    let value = value.into();
    let placeholder = placeholder.into();
    cx.new(|cx| {
        let mut input = RenameInput::new(value, cx);
        // `search_style` removes RenameInput's 20-UTF-16-unit ceiling and gives
        // the 12px field typography used by the frozen provider dialog.
        input.search_style = true;
        input.placeholder = placeholder;
        input
    })
}

fn secret_entity(
    placeholder: impl Into<String>,
    cx: &mut Context<SettingsShell>,
) -> Entity<SecretInput> {
    let placeholder = placeholder.into();
    cx.new(|cx| {
        let mut input = SecretInput::new(cx);
        input.placeholder = placeholder.into();
        input
    })
}

fn subscribe_rename(
    input: &Entity<RenameInput>,
    window: &mut Window,
    cx: &mut Context<SettingsShell>,
    subscriptions: &mut Vec<Subscription>,
) {
    subscriptions.push(
        cx.subscribe_in(input, window, |this, _, event, window, cx| match event {
            RenameInputEvent::Changed => this.clear_editor_error(cx),
            RenameInputEvent::Cancel => this.escape(window, cx),
            RenameInputEvent::Confirm => {}
        }),
    );
}

fn subscribe_secret(
    input: &Entity<SecretInput>,
    window: &mut Window,
    cx: &mut Context<SettingsShell>,
    subscriptions: &mut Vec<Subscription>,
) {
    subscriptions.push(
        cx.subscribe_in(input, window, |this, _, event, window, cx| match event {
            SecretInputEvent::Changed => {
                this.provider_connection_field_changed(cx);
            }
            SecretInputEvent::Cancel => this.escape(window, cx),
            SecretInputEvent::Confirm => {}
        }),
    );
}

fn subscribe_provider_connection(
    input: &Entity<RenameInput>,
    window: &mut Window,
    cx: &mut Context<SettingsShell>,
    subscriptions: &mut Vec<Subscription>,
) {
    subscriptions.push(
        cx.subscribe_in(input, window, |this, _, event, window, cx| match event {
            RenameInputEvent::Changed => this.provider_connection_field_changed(cx),
            RenameInputEvent::Cancel => this.escape(window, cx),
            RenameInputEvent::Confirm => {}
        }),
    );
}

fn subscribe_model_query(
    input: &Entity<RenameInput>,
    window: &mut Window,
    cx: &mut Context<SettingsShell>,
    subscriptions: &mut Vec<Subscription>,
) {
    subscriptions.push(
        cx.subscribe_in(input, window, |this, _, event, window, cx| match event {
            RenameInputEvent::Changed => {
                if let Some(editor) = &mut this.editor {
                    editor.model_cursor = 0;
                    editor.model_scroll.set_offset(Point::default());
                }
                cx.notify();
            }
            RenameInputEvent::Confirm => this.choose_first_visible_model(window, cx),
            RenameInputEvent::Cancel => {
                let focus = if let Some(editor) = &mut this.editor {
                    editor.model_chooser = false;
                    editor.focus.model.clone()
                } else {
                    return;
                };
                window.focus(&focus);
                cx.notify();
            }
        }),
    );
}

impl Editor {
    fn new(
        locale: InterfaceLocale,
        existing: Option<Value>,
        step: EditorStep,
        preset_id: Option<&'static str>,
        query_text: String,
        selection: Option<PickChoice>,
        opener: FocusHandle,
        window: &mut Window,
        cx: &mut Context<SettingsShell>,
    ) -> Self {
        let draft = if let Some(provider) = &existing {
            ProviderDraft::from_provider(provider)
        } else if step == EditorStep::Form {
            preset_draft(preset_id)
        } else {
            ProviderDraft::default()
        };
        let mut editor = Self::from_draft(locale, draft, opener, window, cx);
        editor.step = step;
        editor.selection = selection;
        if !query_text.is_empty() {
            editor.query.update(cx, |input, _| input.text = query_text);
        }
        let focus = if step == EditorStep::Pick {
            editor.query.read(cx).focus.clone()
        } else if step == EditorStep::Subscription {
            editor.focus.subscription_primary.clone()
        } else if editor.draft.subscription_kind.is_some() {
            editor.name.read(cx).focus.clone()
        } else {
            editor.focus.key.clone()
        };
        window.focus(&focus);
        editor
    }

    /// Build the shared four-tier editor around a draft supplied by the
    /// subscription sign-in. The caller chooses the step and focus target.
    fn from_draft(
        locale: InterfaceLocale,
        mut draft: ProviderDraft,
        opener: FocusHandle,
        window: &mut Window,
        cx: &mut Context<SettingsShell>,
    ) -> Self {
        let keep_empty_fable = draft.subscription_kind.is_some()
            && draft.id.is_some()
            && draft.model_mapping[1].trim().is_empty();
        normalize_subscription_draft(&mut draft);
        if keep_empty_fable {
            // An existing subscription may have left this optional mapping unset.
            draft.model_mapping[1].clear();
        }
        for effort in &mut draft.effort_mapping {
            if effort.is_empty() {
                *effort = "follow".into();
            }
        }
        if draft.reasoning_effort.is_empty() {
            draft.reasoning_effort = "follow".into();
        }
        let baseline = FormValues {
            name: draft.name.clone(),
            endpoint: draft.endpoint.clone(),
            model: draft.model.clone(),
            key: String::new(),
            mappings: draft.model_mapping.clone(),
            efforts: draft.effort_mapping.clone(),
            reasoning_effort: draft.reasoning_effort.clone(),
            context_mode: draft.context_mode().to_owned(),
        };
        let baseline_metadata = draft.model_metadata.clone();
        let query = input_entity(
            String::new(),
            locale.text("搜索供应商", "Search providers"),
            cx,
        );
        let key = secret_entity(
            if draft.has_api_key {
                locale.text(
                    "已配置 · 留空保持原密钥",
                    "Configured · leave blank to keep",
                )
            } else {
                locale.text("粘贴 API Key", "Paste API key")
            },
            cx,
        );
        let endpoint = input_entity(draft.endpoint.clone(), "https://api.example.com/v1", cx);
        let model = input_entity(
            draft.model.clone(),
            if draft.built_in {
                locale.text("使用 CLI 默认模型", "Use CLI default")
            } else {
                locale.text("选择或填写模型名称", "Choose or enter model ID")
            },
            cx,
        );
        let model_query = input_entity(
            String::new(),
            locale.text("搜索模型名称或 ID", "Search model name or ID"),
            cx,
        );
        let name = input_entity(draft.name.clone(), "", cx);
        let mappings = std::array::from_fn(|index| {
            input_entity(
                draft.model_mapping[index].clone(),
                locale.text("输入模型 ID", "Enter model ID"),
                cx,
            )
        });
        let efforts = draft.effort_mapping.clone();

        let mut subscriptions = Vec::new();
        subscribe_rename(&query, window, cx, &mut subscriptions);
        subscribe_provider_connection(&endpoint, window, cx, &mut subscriptions);
        subscribe_rename(&model, window, cx, &mut subscriptions);
        subscribe_model_query(&model_query, window, cx, &mut subscriptions);
        subscribe_rename(&name, window, cx, &mut subscriptions);
        for input in &mappings {
            subscribe_rename(input, window, cx, &mut subscriptions);
        }
        subscribe_secret(&key, window, cx, &mut subscriptions);

        let focus = EditorFocus {
            close: cx.focus_handle().tab_stop(true),
            back: cx.focus_handle().tab_stop(true),
            presets: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            subscription_choices: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            custom: cx.focus_handle().tab_stop(true),
            change: cx.focus_handle().tab_stop(true),
            cancel: cx.focus_handle().tab_stop(true),
            next: cx.focus_handle().tab_stop(true),
            key: key.read(cx).focus.clone(),
            reveal: cx.focus_handle().tab_stop(true),
            key_link: cx.focus_handle().tab_stop(true),
            endpoint: endpoint.read(cx).focus.clone(),
            model: model.read(cx).focus.clone(),
            fetch: cx.focus_handle().tab_stop(true),
            chooser: cx.focus_handle().tab_stop(true),
            model_search: model_query.read(cx).focus.clone(),
            name: name.read(cx).focus.clone(),
            mappings: mappings
                .each_ref()
                .map(|input| input.read(cx).focus.clone()),
            mapping_choices: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            manual_links: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            efforts: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            context_modes: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            delete: cx.focus_handle().tab_stop(true),
            save: cx.focus_handle().tab_stop(true),
            save_switch: cx.focus_handle().tab_stop(true),
            keep_editing: cx.focus_handle().tab_stop(true),
            confirm_action: cx.focus_handle().tab_stop(true),
            error: cx.focus_handle().tab_stop(false),
            subscription_risk: cx.focus_handle().tab_stop(true),
            subscription_primary: cx.focus_handle().tab_stop(true),
            subscription_cancel: cx.focus_handle().tab_stop(true),
            subscription_reopen: cx.focus_handle().tab_stop(true),
            subscription_system_browser: cx.focus_handle().tab_stop(true),
        };
        Self {
            step: EditorStep::Form,
            subscription: None,
            query,
            selection: None,
            draft,
            baseline,
            baseline_metadata,
            context_touched: false,
            connection_changed: false,
            key,
            endpoint,
            model,
            name,
            mappings,
            efforts,
            manual_mappings: [false; ROLES.len()],
            model_query,
            model_chooser: false,
            fetch_error: None,
            fetch_snapshot: None,
            revealed_key: false,
            select_open: None,
            select_bounds: Rc::new(RefCell::new([None; ROLES.len() * 2])),
            select_scroll: ScrollHandle::new(),
            model_scroll: ScrollHandle::new(),
            model_cursor: 0,
            error: None,
            confirm: None,
            confirm_return: None,
            opener,
            focus,
            body_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn values(&self, cx: &App) -> FormValues {
        FormValues {
            name: self.name.read(cx).text.clone(),
            endpoint: self.endpoint.read(cx).text.clone(),
            model: self.model.read(cx).text.clone(),
            key: self.key.read(cx).text().to_owned(),
            mappings: self
                .mappings
                .each_ref()
                .map(|input| input.read(cx).text.clone()),
            efforts: self.efforts.clone(),
            reasoning_effort: self.draft.reasoning_effort.clone(),
            context_mode: self.draft.context_mode().to_owned(),
        }
    }

    fn working_draft(&self, cx: &App) -> ProviderDraft {
        let values = self.values(cx);
        let mut draft = self.draft.clone();
        draft.name = values.name;
        draft.endpoint = values.endpoint;
        draft.model = values.model;
        draft.api_key = values.key;
        draft.model_mapping = values.mappings;
        draft.effort_mapping = values.efforts;
        draft.ignore_model_efforts = self.fetch_error.is_some();
        draft
    }

    fn is_dirty(&self, cx: &App) -> bool {
        let metadata = self
            .draft
            .fetched_metadata
            .as_ref()
            .unwrap_or(&self.draft.model_metadata);
        self.values(cx) != self.baseline
            || *metadata != self.baseline_metadata
            || (self.context_touched
                && self.baseline.context_mode == self.draft.context_mode()
                && self.draft.legacy_context_window)
    }

    fn visible_fetched_models(&self, cx: &App) -> Vec<&Value> {
        let query = self.model_query.read(cx).text.trim().to_lowercase();
        let Some(rows) = self
            .draft
            .fetched_metadata
            .as_ref()
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        rows.iter()
            .filter(|row| {
                let Some(id) = row["id"].as_str() else {
                    return false;
                };
                let name = row["displayName"].as_str().unwrap_or_default();
                query.is_empty()
                    || id.to_lowercase().contains(&query)
                    || name.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn ask_confirmation(
        &mut self,
        kind: ConfirmKind,
        window: &mut Window,
        cx: &mut Context<SettingsShell>,
    ) {
        self.confirm_return = Some((window.focused(cx), self.body_scroll.offset()));
        self.confirm = Some(kind);
        self.select_open = None;
        self.body_scroll.scroll_to_bottom();
        window.focus(&self.focus.keep_editing);
        cx.notify();
    }

    fn select_focus(&self, select: ProviderSelect) -> &FocusHandle {
        match select {
            ProviderSelect::Mapping(index) => &self.focus.mapping_choices[index],
            ProviderSelect::Effort(index) => &self.focus.efforts[index],
        }
    }

    fn choose_model(&mut self, id: String, window: &mut Window, cx: &mut Context<SettingsShell>) {
        let model_focus = self.model.read(cx).focus.clone();
        self.model.update(cx, |input, cx| {
            input.text.clone_from(&id);
            cx.notify();
        });
        self.model_chooser = false;
        window.focus(&model_focus);
        cx.notify();
    }

    fn choose_first_visible_model(&mut self, window: &mut Window, cx: &mut Context<SettingsShell>) {
        let first = self
            .visible_fetched_models(cx)
            .get(self.model_cursor)
            .and_then(|row| row["id"].as_str().map(str::to_owned));
        if let Some(id) = first {
            self.choose_model(id, window, cx);
        }
    }
}

fn preset_draft(preset_id: Option<&'static str>) -> ProviderDraft {
    let Some(preset) = preset_id.and_then(preset_by_id) else {
        return ProviderDraft::default();
    };
    ProviderDraft {
        name: preset.name.into(),
        endpoint: preset.endpoint.into(),
        model: preset.model.into(),
        model_mapping: preset.mapping.map(str::to_owned),
        effort_mapping: ROLES.map(|_| "follow".into()),
        ..Default::default()
    }
}

pub(super) fn effort_name(effort: &str) -> &'static str {
    match effort {
        "low" => "低",
        "medium" => "中",
        "high" => "高",
        "xhigh" => "极高",
        "max" => "最大",
        _ => "跟随会话",
    }
}

fn editor_effort_name(effort: &str, locale: InterfaceLocale) -> &'static str {
    if locale == InterfaceLocale::Chinese {
        return effort_name(effort);
    }
    match effort {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "XHigh",
        "max" => "Max",
        _ => "Follow session",
    }
}

fn window_text(row: &Value, locale: InterfaceLocale) -> String {
    row["contextWindow"]
        .as_u64()
        .or_else(|| row["maxContextWindow"].as_u64())
        .filter(|tokens| *tokens > 0)
        .map(|tokens| format!("{} tokens", format_tokens(tokens)))
        .unwrap_or_else(|| locale.text("窗口未知", "Window unknown").into())
}

fn short_endpoint(value: &str) -> String {
    let value = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    value.strip_suffix('/').unwrap_or(value).to_owned()
}

fn provider_tone(name: &str) -> u8 {
    let lower = name.to_ascii_lowercase();
    if lower.contains("openai") {
        1
    } else if lower.contains("anthropic") {
        2
    } else {
        0
    }
}

fn provider_initial(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "P".into())
}

fn model_label(model: &str) -> &str {
    if model == CLI_DEFAULT_MODEL {
        "CLI 默认"
    } else {
        model
    }
}

// ---------------------------------------------------------------------------
// Settings shell
// ---------------------------------------------------------------------------

pub enum SettingsShellEvent {
    Close,
    ProviderActivated,
    SaveLocale(LocalePreference),
    SaveTheme(ThemePreference),
    OpenProviderKeyPage(&'static str),
}

impl EventEmitter<SettingsShellEvent> for SettingsShell {}

static NEXT_TIME_ZONE_REQUEST: AtomicU64 = AtomicU64::new(1);
static NEXT_DIAGNOSTIC_REQUEST: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct RuntimeDiagnostic {
    pending: Option<(u64, u64)>,
    result: Option<Result<serde_json::Value, String>>,
}

pub struct SettingsShell {
    commands: RuntimeCommands,
    locale: InterfaceLocale,
    language: LocalePreference,
    language_block: LanguageZoneBlock,
    forced_locale: Option<InterfaceLocale>,
    time_zone: Option<TimeZoneSettingsSnapshot>,
    time_zone_draft: String,
    time_zone_block: LanguageZoneBlock,
    time_zone_request: Option<(u64, u64, bool)>,
    time_zone_menu_open: bool,
    language_zone_focus: LanguageZoneFocus,
    providers: ProviderSettings,
    compaction: CompactionUi,
    subscription_settings_open: bool,
    assignments: ClaudeAssignments,
    assignment_view: Entity<AssignmentView>,
    _assignment_subscriptions: [Subscription; 2],
    page: SettingsPage,
    editor: Option<Editor>,
    dispatch_config: Option<Rc<DispatchConfig>>,
    dispatch_panel: Option<Result<Entity<DispatchSettingsPanel>, String>>,
    context_optimizer_config: Option<Rc<ContextOptimizerConfig>>,
    context_optimizer_panel: Option<Result<Entity<ContextOptimizerSettingsPanel>, String>>,
    theme: ThemeState,
    theme_error: Option<String>,
    keyboard_focus: bool,
    scroll: ScrollHandle,
    nav_focus: [FocusHandle; 5],
    diagnostics_cleanup_focus: FocusHandle,
    diagnostics_cwd: Option<std::path::PathBuf>,
    diagnostics_generation: Option<u64>,
    runtime_diagnostics: HashMap<&'static str, RuntimeDiagnostic>,
    close_focus: FocusHandle,
    notice_close_focus: FocusHandle,
    notice_retry_focus: FocusHandle,
    load_retry_focus: FocusHandle,
    dispatch_retry_focus: FocusHandle,
    new_provider_focus: FocusHandle,
    card_controls: HashMap<String, [FocusHandle; 3]>,
}

impl SettingsShell {
    pub fn new(commands: RuntimeCommands, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut providers = ProviderSettings::new(commands.clone());
        // First read on mount; the shared selection is kept by ProviderSettings.
        providers.load();
        providers.load_compaction();
        let assignment_view = cx.new(AssignmentView::new);
        let assignment_actions =
            cx.subscribe_in(&assignment_view, window, |shell, _, action, window, cx| {
                shell.assignment_action(action, Some(window), cx)
            });
        let assignment_updates = cx.observe(&assignment_view, |_, _, cx| cx.notify());
        let mut shell = Self {
            commands: commands.clone(),
            locale: InterfaceLocale::default(),
            language: LocalePreference::default(),
            language_block: LanguageZoneBlock {
                available: true,
                ..Default::default()
            },
            forced_locale: None,
            time_zone: None,
            time_zone_draft: "auto".into(),
            time_zone_block: LanguageZoneBlock::default(),
            time_zone_request: None,
            time_zone_menu_open: false,
            language_zone_focus: LanguageZoneFocus::new(cx),
            providers,
            compaction: CompactionUi::new(window, cx),
            subscription_settings_open: false,
            assignments: ClaudeAssignments::new(commands),
            assignment_view,
            _assignment_subscriptions: [assignment_actions, assignment_updates],
            page: SettingsPage::ClaudeCode,
            editor: None,
            dispatch_config: std::env::home_dir()
                .map(|home| Rc::new(DispatchConfig::for_home(home))),
            dispatch_panel: None,
            context_optimizer_config: std::env::var_os("MATOU_DATA_ROOT")
                .map(std::path::PathBuf::from)
                .map(|root| Rc::new(ContextOptimizerConfig::for_data_root(root))),
            context_optimizer_panel: None,
            theme: ThemeState::new(ThemePreference::Light, window.appearance()),
            theme_error: None,
            keyboard_focus: false,
            scroll: ScrollHandle::new(),
            nav_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            diagnostics_cleanup_focus: cx.focus_handle().tab_stop(true),
            diagnostics_cwd: None,
            diagnostics_generation: None,
            runtime_diagnostics: HashMap::new(),
            close_focus: cx.focus_handle().tab_stop(true),
            notice_close_focus: cx.focus_handle().tab_stop(true),
            notice_retry_focus: cx.focus_handle().tab_stop(true),
            load_retry_focus: cx.focus_handle().tab_stop(true),
            dispatch_retry_focus: cx.focus_handle().tab_stop(true),
            new_provider_focus: cx.focus_handle().tab_stop(true),
            card_controls: HashMap::new(),
        };
        shell.load_assignments(cx);
        shell.open_subscription_settings_round(cx);
        shell
    }

    pub fn can_close(&self) -> bool {
        self.editor.is_none() && !self.is_switching()
    }

    pub(crate) fn is_switching(&self) -> bool {
        self.providers.switching().is_some()
    }

    fn subscription_connected(&self) -> bool {
        self.commands.connection().1 == ConnectionPhase::Ready
    }

    /// The Claude Code pane holds one settings round while it is visible. The
    /// runtime reads account details and quota once for this lease; repeated
    /// calls from the same view are ignored until it is released.
    fn open_subscription_settings_round(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.subscription_settings_open) {
            // Already open: keep the existing round instead of reopening it.
            self.subscription_settings_open = true;
            return;
        }
        if !self.subscription_connected() {
            return;
        }
        let _ = self.providers.open_subscription_settings();
        self.subscription_settings_open = true;
        cx.notify();
    }

    fn release_subscription_settings_round(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.subscription_settings_open) {
            let _ = self.providers.close_subscription_settings();
            cx.notify();
        }
    }

    /// Called by the workspace before it drops this shell, whether the user
    /// closes settings, switches away from the pane or the window goes away.
    /// Cancels a pending OAuth attempt, discards the unsaved draft and releases
    /// the settings round so the runtime stops holding a model-router lease.
    pub(crate) fn close_subscription_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_subscription_flow();
        self.editor = None;
        self.keyboard_focus = false;
        self.release_subscription_settings_round(cx);
        window.refresh();
        cx.notify();
    }

    /// One chain per subscription dialog. The timer only reads status while
    /// the flow is waiting; closing, cancelling or switching phase invalidates
    /// the generation so a late tick cannot touch the next dialog.
    fn schedule_subscription_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.providers.subscription_login().phase != "waiting" {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some(flow) = &mut editor.subscription else {
            return;
        };
        if flow.polling {
            return;
        }
        flow.polling = true;
        let generation = flow.generation;
        let timer = cx.background_executor().timer(Duration::from_millis(800));
        cx.spawn_in(window, async move |shell, cx| {
            timer.await;
            let _ = cx.update(|window, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.subscription_poll(generation, window, cx)
                });
            });
        })
        .detach();
    }

    fn subscription_poll(&mut self, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.editor.as_ref().and_then(|editor| {
            editor
                .subscription
                .as_ref()
                .filter(|flow| flow.generation == generation && flow.polling)
        });
        if current.is_none() {
            return;
        }
        if let Some(flow) = self
            .editor
            .as_mut()
            .and_then(|editor| editor.subscription.as_mut())
        {
            flow.polling = false;
        }
        if self.providers.subscription_login().phase != "waiting" {
            return;
        }
        let update = self.providers.poll_subscription_login();
        let waiting = self.providers.subscription_login().phase == "waiting";
        let _ = self.apply_subscription_update(update, window, cx);
        if waiting
            && self.editor.as_ref().is_some_and(|editor| {
                editor
                    .subscription
                    .as_ref()
                    .is_some_and(|flow| flow.generation == generation)
            })
        {
            self.schedule_subscription_poll(window, cx);
        }
        cx.notify();
    }

    fn apply_subscription_update(
        &mut self,
        update: ProviderUpdate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ProviderUpdate {
        match update {
            ProviderUpdate::SubscriptionDraft(draft) => {
                self.adopt_subscription_draft(draft, window, cx);
                ProviderUpdate::None
            }
            ProviderUpdate::SaveFailed(message) => {
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(message.clone());
                    editor.body_scroll.scroll_to_bottom();
                }
                self.apply_update(ProviderUpdate::SaveFailed(message), window, cx);
                ProviderUpdate::None
            }
            other => {
                self.apply_update(other, window, cx);
                ProviderUpdate::None
            }
        }
    }

    fn adopt_subscription_draft(
        &mut self,
        mut draft: ProviderDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = self.editor.as_ref();
        let opener = previous
            .map(|editor| editor.opener.clone())
            .unwrap_or_else(|| self.new_provider_focus.clone());
        let flow = previous
            .and_then(|editor| editor.subscription.clone())
            .or_else(|| {
                draft
                    .subscription_kind
                    .clone()
                    .map(|provider| SubscriptionFlow {
                        provider,
                        existing: draft.id.is_some(),
                        existing_id: draft.id.clone(),
                        existing_name: Some(draft.name.clone())
                            .filter(|name| !name.trim().is_empty()),
                        risk_checked: false,
                        generation: 1,
                        polling: false,
                    })
            });
        let Some(flow) = flow else {
            return;
        };
        normalize_subscription_draft(&mut draft);
        if draft.id.is_none() {
            draft.id = flow.existing_id.clone();
        }
        if draft.name.trim().is_empty() {
            draft.name = flow.existing_name.clone().unwrap_or_else(|| {
                if flow.provider == "antigravity" {
                    "Antigravity".into()
                } else {
                    "ChatGPT".into()
                }
            });
        }
        self.editor = None;
        let mut editor = Editor::from_draft(self.locale, draft, opener, window, cx);
        editor.step = EditorStep::Form;
        editor.subscription = Some(SubscriptionFlow {
            polling: false,
            generation: flow.generation + 1,
            risk_checked: flow.risk_checked,
            ..flow
        });
        let focus = editor.name.read(cx).focus.clone();
        self.editor = Some(editor);
        window.focus(&focus);
        cx.notify();
    }

    fn start_subscription_from_pick(
        &mut self,
        kind: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.providers.saving() || !self.subscription_connected() {
            return;
        }
        let existing = self
            .providers
            .providers()
            .iter()
            .find(|provider| subscription_kind(provider) == Some(kind))
            .cloned();
        self.begin_subscription_flow(kind, existing.as_ref(), window, cx);
    }

    fn begin_subscription_flow(
        &mut self,
        kind: &str,
        existing: Option<&Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let generation = editor
            .subscription
            .as_ref()
            .map_or(1, |flow| flow.generation + 1);
        editor.step = EditorStep::Subscription;
        editor.error = None;
        editor.confirm = None;
        editor.subscription = Some(SubscriptionFlow {
            provider: kind.to_owned(),
            existing: existing.is_some(),
            existing_id: existing
                .and_then(|provider| provider["id"].as_str())
                .map(str::to_owned),
            existing_name: existing
                .and_then(|provider| provider["name"].as_str())
                .map(str::to_owned),
            risk_checked: false,
            generation,
            polling: false,
        });
        let _ = self.providers.begin_subscription(kind);
        let phase = self.providers.subscription_login().phase;
        let focus = self.editor.as_ref().map(|editor| {
            if phase == "risk" {
                editor.focus.subscription_risk.clone()
            } else {
                editor.focus.subscription_primary.clone()
            }
        });
        if let Some(focus) = focus {
            window.focus(&focus);
        }
        if phase == "waiting" {
            self.schedule_subscription_poll(window, cx);
        }
        cx.notify();
    }

    fn open_subscription_relogin(
        &mut self,
        provider: Value,
        opener: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_some() || self.providers.saving() || !self.subscription_connected() {
            return;
        }
        let Some(kind) = subscription_kind(&provider).map(str::to_owned) else {
            return;
        };
        self.editor = Some(Editor::new(
            self.locale,
            Some(provider.clone()),
            EditorStep::Subscription,
            None,
            String::new(),
            None,
            opener,
            window,
            cx,
        ));
        self.begin_subscription_flow(&kind, Some(&provider), window, cx);
    }

    fn toggle_subscription_risk(&mut self, cx: &mut Context<Self>) {
        if !self.subscription_connected() || self.providers.subscription_login().busy {
            return;
        }
        if let Some(flow) = self
            .editor
            .as_mut()
            .and_then(|editor| editor.subscription.as_mut())
        {
            flow.risk_checked = !flow.risk_checked;
            cx.notify();
        }
    }

    fn confirm_subscription_risk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.subscription.as_ref())
        else {
            return;
        };
        if !flow.risk_checked
            || !self.subscription_connected()
            || self.providers.subscription_login().busy
        {
            return;
        }
        let _ = self.providers.confirm_subscription_risk();
        let phase = self.providers.subscription_login().phase;
        let focus = self.editor.as_ref().map(|editor| {
            if phase == "risk" {
                editor.focus.subscription_risk.clone()
            } else {
                editor.focus.subscription_primary.clone()
            }
        });
        if let Some(focus) = focus {
            window.focus(&focus);
        }
        cx.notify();
    }

    fn start_subscription_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.subscription_connected() || self.providers.subscription_login().busy {
            return;
        }
        let _ = self
            .providers
            .start_subscription_login_localized(self.locale != InterfaceLocale::English);
        if self.providers.subscription_login().phase == "waiting" {
            if let Some(editor) = &self.editor {
                window.focus(&editor.focus.subscription_cancel);
            }
            self.schedule_subscription_poll(window, cx);
        }
        cx.notify();
    }

    fn reopen_subscription_login(&mut self, cx: &mut Context<Self>) {
        let login = self.providers.subscription_login();
        if !self.subscription_connected() || login.busy || !login.can_reopen {
            return;
        }
        let _ = self.providers.reopen_subscription_login();
        if self.providers.subscription_login().phase == "waiting" {
            // The running status chain stays in charge; never start another.
            cx.notify();
        }
    }

    fn reopen_subscription_in_system_browser(&mut self, cx: &mut Context<Self>) {
        let login = self.providers.subscription_login();
        if !self.subscription_connected()
            || login.busy
            || !login.can_reopen
            || !login.system_browser_available
        {
            return;
        }
        let _ = self.providers.reopen_subscription_in_system_browser();
        cx.notify();
    }

    fn cancel_subscription_login_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.providers.cancel_subscription_login();
        if let Some(editor) = &mut self.editor
            && let Some(flow) = &mut editor.subscription
        {
            flow.polling = false;
            flow.generation = flow.generation.wrapping_add(1);
        }
        if let Some(editor) = &self.editor {
            window.focus(&editor.focus.subscription_primary);
        }
        cx.notify();
    }

    /// Discard the pending flow and its draft. Called on close, page leave and
    /// workspace shutdown. A saved flow is cleared before `close_editor` so a
    /// successful commit is never followed by an attempt cancellation.
    fn cancel_subscription_flow(&mut self) {
        let had_flow = self
            .editor
            .as_ref()
            .and_then(|editor| editor.subscription.as_ref())
            .is_some();
        if !had_flow {
            return;
        }
        if let Some(editor) = &mut self.editor
            && let Some(flow) = &mut editor.subscription
        {
            flow.polling = false;
            flow.generation = flow.generation.wrapping_add(1);
            editor.subscription = None;
        }
        let _ = self.providers.cancel_subscription_login();
    }

    // ------------------------------------------------------------------
    // Compaction model block
    // ------------------------------------------------------------------

    fn dismiss_compaction_menu(&mut self, cx: &mut Context<Self>) -> bool {
        if self.compaction.open.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    fn sync_compaction(&mut self, cx: &mut Context<Self>) {
        let saving = self.providers.saving_compaction();
        let error = self.providers.compaction_error();
        if self.compaction.was_saving && !saving {
            self.compaction.saved = error.is_none();
        }
        self.compaction.was_saving = saving;
        let Some(persisted) = self.providers.compaction().cloned() else {
            return;
        };
        let normalized = CompactionDraft::from_value(&persisted).input();
        let dirty = self
            .compaction
            .draft
            .as_ref()
            .is_some_and(|draft| draft.is_dirty(&persisted));
        let baseline_changed = self.compaction.baseline.as_ref() != Some(&normalized);
        if self.compaction.draft.is_none() || (baseline_changed && !dirty) {
            self.compaction.draft = Some(CompactionDraft::from_value(&persisted));
            self.compaction.baseline = Some(normalized);
            self.compaction.manual_model = false;
        } else if baseline_changed {
            self.compaction.baseline = Some(normalized);
        }
        let _ = cx;
    }

    fn compaction_selected(&self, select: CompactionSelect) -> String {
        let Some(draft) = &self.compaction.draft else {
            return String::new();
        };
        match select {
            CompactionSelect::Provider => draft.provider_id.clone().unwrap_or_default(),
            CompactionSelect::Model if self.compaction.manual_model => "__manual__".into(),
            CompactionSelect::Model => draft.model.clone(),
            CompactionSelect::Effort => draft.effort.clone(),
        }
    }

    fn compaction_options(&self, select: CompactionSelect) -> Vec<CompactionOption> {
        let Some(draft) = &self.compaction.draft else {
            return Vec::new();
        };
        match select {
            CompactionSelect::Provider => provider_options(self.providers.providers(), self.locale),
            CompactionSelect::Model => {
                let Some(provider) = selected_provider(draft, self.providers.providers()) else {
                    return Vec::new();
                };
                if !provider_available(provider) {
                    return Vec::new();
                }
                let mut options = model_options(provider);
                let current = draft.model.trim();
                if !current.is_empty() && !options.iter().any(|option| option.value == current) {
                    let label = if self.locale == InterfaceLocale::Chinese {
                        format!("{current}（不在当前列表中）")
                    } else {
                        format!("{current} (not in this list)")
                    };
                    options.push(CompactionOption {
                        value: current.to_owned(),
                        label,
                        enabled: true,
                    });
                }
                options.push(CompactionOption {
                    value: "__manual__".into(),
                    label: self
                        .locale
                        .text("手动填写模型 ID…", "Enter model ID manually…")
                        .into(),
                    enabled: true,
                });
                options
            }
            CompactionSelect::Effort => {
                let Some(provider) = selected_provider(draft, self.providers.providers()) else {
                    return Vec::new();
                };
                if !provider_available(provider) {
                    return Vec::new();
                }
                effort_values(provider, draft.model.trim(), &draft.effort)
                    .into_iter()
                    .map(|effort| CompactionOption {
                        value: effort.clone(),
                        label: compaction_effort_label(&effort, self.locale).to_owned(),
                        enabled: true,
                    })
                    .collect()
            }
        }
    }

    fn compaction_trigger_enabled(&self, select: CompactionSelect) -> bool {
        if self.providers.saving_compaction() {
            return false;
        }
        let Some(draft) = &self.compaction.draft else {
            return false;
        };
        match select {
            CompactionSelect::Provider => true,
            CompactionSelect::Model | CompactionSelect::Effort => {
                selected_provider(draft, self.providers.providers()).is_some_and(provider_available)
            }
        }
    }

    fn compaction_label(&self, select: CompactionSelect) -> String {
        let selected = self.compaction_selected(select);
        let options = self.compaction_options(select);
        if let Some(option) = options.iter().find(|option| option.value == selected) {
            return option.label.clone();
        }
        if !selected.is_empty() && selected != "__manual__" {
            if select == CompactionSelect::Effort {
                return compaction_effort_label(&selected, self.locale).into();
            }
            if select == CompactionSelect::Model
                && let Some(draft) = &self.compaction.draft
                && let Some(provider) = selected_provider(draft, self.providers.providers())
            {
                return compaction_model_name(provider, &selected);
            }
            return selected;
        }
        match select {
            CompactionSelect::Provider => {
                self.locale.text("请选择供应商", "Choose a provider").into()
            }
            CompactionSelect::Model => self
                .locale
                .text("请选择压缩模型", "Choose a compaction model")
                .into(),
            CompactionSelect::Effort => compaction_effort_label("follow", self.locale).into(),
        }
    }

    fn open_compaction_menu(
        &mut self,
        select: CompactionSelect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.compaction_trigger_enabled(select) {
            return;
        }
        if self
            .compaction
            .open
            .as_ref()
            .is_some_and(|menu| menu.select == select)
        {
            self.compaction.open = None;
            cx.notify();
            return;
        }
        let items = self.compaction_options(select);
        if items.is_empty() {
            return;
        }
        let selected = self.compaction_selected(select);
        let active = items
            .iter()
            .position(|item| item.value == selected)
            .unwrap_or(0);
        let trigger_focus = self.compaction.focus[select.index()].clone();
        window.focus(&trigger_focus);
        self.compaction.scroll.scroll_to_item(active);
        self.compaction.open = Some(CompactionMenu {
            select,
            active,
            items,
            selected,
            trigger_focus,
        });
        window.refresh();
        cx.notify();
    }

    fn compaction_menu_key(
        &mut self,
        select: CompactionSelect,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open_here = self
            .compaction
            .open
            .as_ref()
            .is_some_and(|menu| menu.select == select);
        if !open_here {
            if matches!(event.keystroke.key.as_str(), "enter" | "space" | "down")
                && !event.keystroke.modifiers.modified()
            {
                self.open_compaction_menu(select, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        let Some((len, active)) = self
            .compaction
            .open
            .as_ref()
            .map(|menu| (menu.items.len(), menu.active))
        else {
            return;
        };
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => {
                self.dismiss_compaction_menu(cx);
                cx.stop_propagation();
            }
            "tab" => {
                self.dismiss_compaction_menu(cx);
            }
            "down" => {
                let next = if len == 0 { 0 } else { (active + 1) % len };
                self.set_compaction_active(next, window, cx);
                cx.stop_propagation();
            }
            "up" => {
                let next = if len == 0 {
                    0
                } else {
                    (active + len - 1) % len
                };
                self.set_compaction_active(next, window, cx);
                cx.stop_propagation();
            }
            "home" => {
                self.set_compaction_active(0, window, cx);
                cx.stop_propagation();
            }
            "end" => {
                self.set_compaction_active(len.saturating_sub(1), window, cx);
                cx.stop_propagation();
            }
            "enter" | "space" => {
                let value = self
                    .compaction
                    .open
                    .as_ref()
                    .and_then(|menu| menu.items.get(menu.active))
                    .filter(|item| item.enabled)
                    .map(|item| item.value.clone());
                if let Some(value) = value {
                    self.choose_compaction_option(select, value, window, cx);
                }
                cx.stop_propagation();
            }
            _ => {
                cx.stop_propagation();
            }
        }
    }

    fn set_compaction_active(
        &mut self,
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.compaction.open.as_mut() else {
            return;
        };
        if menu.items.is_empty() {
            return;
        }
        menu.active = active.min(menu.items.len() - 1);
        let active = menu.active;
        self.compaction.scroll.scroll_to_item(active);
        window.refresh();
        cx.notify();
    }

    fn choose_compaction_option(
        &mut self,
        select: CompactionSelect,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.compaction.open.take() else {
            return;
        };
        if let Some(draft) = &mut self.compaction.draft {
            match select {
                CompactionSelect::Provider => {
                    draft.provider_id = Some(value.clone());
                    self.compaction.manual_model = false;
                    let first = self
                        .providers
                        .providers()
                        .iter()
                        .find(|provider| provider["id"].as_str() == Some(value.as_str()))
                        .and_then(|provider| model_options(provider).into_iter().next())
                        .map(|option| option.value)
                        .unwrap_or_default();
                    draft.model = first;
                }
                CompactionSelect::Model if value == "__manual__" => {
                    self.compaction.manual_model = true;
                    let current = draft.model.clone();
                    self.seed_compaction_manual(&current, cx);
                    let focus = self.compaction.manual_input.read(cx).focus.clone();
                    window.focus(&focus);
                }
                CompactionSelect::Model => {
                    draft.model = value;
                    self.compaction.manual_model = false;
                }
                CompactionSelect::Effort => {
                    draft.effort = value;
                }
            }
        }
        self.compaction.saved = false;
        window.focus(&menu.trigger_focus);
        window.refresh();
        cx.notify();
    }

    fn seed_compaction_manual(&mut self, text: &str, cx: &mut Context<Self>) {
        let input = self.compaction.manual_input.clone();
        input.update(cx, |input, cx| {
            if input.text != text {
                input.text = text.to_owned();
                cx.notify();
            }
        });
    }

    fn select_compaction_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        if self.providers.saving_compaction() {
            return;
        }
        let Some(draft) = &mut self.compaction.draft else {
            return;
        };
        if draft.mode == mode {
            return;
        }
        let previous = draft.mode.clone();
        draft.mode = mode.to_owned();
        self.compaction.saved = false;
        let persisted_custom = self
            .providers
            .compaction()
            .is_some_and(|value| value["mode"] == "custom");
        if mode == "follow" && (previous == "custom" || persisted_custom) {
            self.save_compaction_now(cx);
        }
        cx.notify();
    }

    fn save_compaction_now(&mut self, cx: &mut Context<Self>) {
        if self.providers.saving_compaction() {
            return;
        }
        let Some(draft) = self.compaction.draft.clone() else {
            return;
        };
        if self.providers.compaction().is_none() {
            return;
        }
        if compaction_validation(&draft, self.providers.providers(), self.locale).is_some() {
            return;
        }
        self.compaction.saved = false;
        self.compaction.was_saving = true;
        self.compaction.open = None;
        self.providers.save_compaction(&draft.input());
        cx.notify();
    }

    fn render_compaction(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let palette = *self.theme.effective().palette();
        let colors = EditorColors::new(&palette, self.is_dark());
        let saving = self.providers.saving_compaction();
        let error = self.providers.compaction_error().map(str::to_owned);
        let persisted = self.providers.compaction().is_some();
        let draft = self.compaction.draft.clone();
        if !persisted {
            let text = if let Some(error) = error {
                let mut row = div()
                    .mt(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(12.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgba(colors.error_line))
                    .bg(rgba(colors.error_bg))
                    .text_size(px(11.))
                    .text_color(rgba(colors.error_text))
                    .child(div().flex_1().whitespace_normal().child(error));
                row = row.child(link_button(
                    "compaction-load-retry",
                    self.locale.text("重试", "Retry"),
                    &self.compaction.retry_focus,
                    self.keyboard_focus,
                    FocusRingStyle::Shell,
                    &colors,
                    cx,
                    |this, _, cx| {
                        this.providers.load_compaction();
                        cx.notify();
                    },
                ));
                row
            } else {
                div()
                    .mt(px(8.))
                    .text_size(px(11.))
                    .text_color(rgba(colors.muted))
                    .child(self.locale.text(
                        "正在加载压缩模型设置…",
                        "Loading compaction model settings…",
                    ))
            };
            return div()
                .mt(px(24.))
                .pt(px(18.))
                .border_t_1()
                .border_color(rgba(colors.line))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight(680.))
                        .text_color(rgba(colors.label))
                        .child(self.locale.text("对话压缩模型", "Compaction model")),
                )
                .child(text);
        }
        let Some(draft) = draft else {
            return div();
        };
        let providers = self.providers.providers();
        let validation = compaction_validation(&draft, providers, self.locale);
        let dirty = self
            .providers
            .compaction()
            .is_some_and(|persisted| draft.is_dirty(persisted));
        let provider = selected_provider(&draft, providers);
        let needs_sign_in = provider.is_some_and(|provider| {
            provider["subscription"].is_object()
                && provider["subscription"]["account"]["status"] == "needs-sign-in"
        });
        let cross_provider = draft.mode == "custom"
            && draft.provider_id.as_deref().is_some_and(|id| {
                self.providers
                    .active_id()
                    .is_some_and(|active| active != id)
            });
        let fallback = draft.mode == "custom"
            && provider.is_some_and(|provider| {
                provider_available(provider) && effort_fallback(provider, draft.model.trim())
            });
        let mut section = div()
            .mt(px(24.))
            .pt(px(18.))
            .border_t_1()
            .border_color(rgba(colors.line))
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight(680.))
                                    .text_color(rgba(colors.label))
                                    .child(self.locale.text("对话压缩模型", "Compaction model")),
                            )
                            .child(
                                div()
                                    .mt(px(4.))
                                    .text_size(px(11.))
                                    .line_height(relative(1.65))
                                    .text_color(rgba(colors.muted))
                                    .whitespace_normal()
                                    .child(self.locale.text(
                                        "长对话触发 /compact 时，默认跟随会话模型；也可单独指定轻量快模型生成摘要，减少高推理档位下的等待。",
                                        "Follows the session model during /compact by default, or use a lighter model for faster summaries without changing your working model.",
                                    )),
                            ),
                    )
                    .when(
                        self.compaction.saved && draft.mode == "follow",
                        |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight(600.))
                                    .text_color(rgb(0x16794b))
                                    .child(self.locale.text(
                                        "已保存，新会话生效",
                                        "Saved · applies to new sessions",
                                    )),
                            )
                        },
                    ),
            );
        let mut card = div()
            .mt(px(10.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.panel))
            .flex()
            .flex_col()
            .gap(px(12.));
        // Two mode choices.
        let mut choices = div().flex().flex_wrap().gap(px(8.));
        for (index, (mode, title, subtitle)) in [
            (
                "follow",
                self.locale.text("跟随会话", "Follow session"),
                self.locale.text(
                    "使用会话当前的模型和推理强度",
                    "Use the session model and reasoning effort",
                ),
            ),
            (
                "custom",
                self.locale.text("独立配置", "Separate configuration"),
                self.locale.text(
                    "仅影响压缩请求，不改变工作模型",
                    "Affects compaction requests only",
                ),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = draft.mode == mode;
            let mut choice = div()
                .id(SharedString::from(format!("compaction-mode-{mode}")))
                .flex_1()
                .min_w(px(190.))
                .flex()
                .items_start()
                .gap(px(9.))
                .p(px(11.))
                .rounded(px(8.))
                .border_1()
                .border_color(rgba(if selected {
                    colors.focus
                } else {
                    colors.line_strong
                }))
                .bg(rgba(if selected {
                    colors.menu_selected_bg
                } else {
                    colors.panel
                }))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(13.))
                        .text_color(rgba(if selected { colors.link } else { colors.muted }))
                        .child(if selected { "◉" } else { "○" }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight(650.))
                                .text_color(rgba(colors.label))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .line_height(relative(1.5))
                                .text_color(rgba(colors.muted))
                                .whitespace_normal()
                                .child(subtitle),
                        ),
                );
            if saving {
                choice = choice.opacity(0.55);
            } else {
                choice = choice.cursor_pointer();
                choice = wire_button(
                    choice,
                    &self.compaction.mode_focus[index],
                    8.,
                    self.keyboard_focus,
                    move |this, _, cx| this.select_compaction_mode(mode, cx),
                    cx,
                );
            }
            choices = choices.child(mark_provider(
                choice,
                format!("compaction-mode-{mode}"),
                title,
                !saving,
            ));
        }
        card = card.child(choices);
        if draft.mode == "custom" {
            let mut grid = div().flex().flex_wrap().gap(px(12.));
            let mut provider_field = div().flex().flex_col().gap(px(5.)).flex_1().min_w(px(170.));
            provider_field = provider_field.child(
                div()
                    .text_size(px(10.))
                    .font_weight(FontWeight(650.))
                    .text_color(rgba(colors.muted))
                    .child(self.locale.text("压缩供应商", "Compaction provider")),
            );
            provider_field = provider_field.child(self.render_compaction_trigger(
                CompactionSelect::Provider,
                &colors,
                window,
                cx,
            ));
            if needs_sign_in {
                let label = self.locale.text("重新登录", "Sign in again");
                provider_field = provider_field.child(link_button(
                    "compaction-relogin",
                    label,
                    &self.compaction.relogin_focus,
                    self.keyboard_focus,
                    FocusRingStyle::Shell,
                    &colors,
                    cx,
                    |this, window, cx| {
                        let Some(provider) = this
                            .compaction
                            .draft
                            .as_ref()
                            .and_then(|draft| selected_provider(draft, this.providers.providers()))
                            .cloned()
                        else {
                            return;
                        };
                        let opener = this.compaction.relogin_focus.clone();
                        this.open_subscription_relogin(provider, opener, window, cx);
                    },
                ));
            }
            grid = grid.child(provider_field);
            let mut model_field = div().flex().flex_col().gap(px(5.)).flex_1().min_w(px(190.));
            model_field = model_field.child(
                div()
                    .text_size(px(10.))
                    .font_weight(FontWeight(650.))
                    .text_color(rgba(colors.muted))
                    .child(self.locale.text("压缩模型", "Compaction model")),
            );
            if self.compaction.manual_model {
                let list_label = self.locale.text("从列表选择", "Choose from list");
                let manual = rename_field(
                    self.compaction.manual_input.clone(),
                    34.,
                    10.,
                    window,
                    &colors,
                    cx,
                );
                model_field = model_field.child(manual).child(link_button(
                    "compaction-model-list",
                    list_label,
                    &self.compaction.focus[CompactionSelect::Model.index()],
                    self.keyboard_focus,
                    FocusRingStyle::Shell,
                    &colors,
                    cx,
                    |this, window, cx| {
                        this.compaction.manual_model = false;
                        let focus = this.compaction.focus[CompactionSelect::Model.index()].clone();
                        window.focus(&focus);
                        cx.notify();
                    },
                ));
            } else {
                model_field = model_field.child(self.render_compaction_trigger(
                    CompactionSelect::Model,
                    &colors,
                    window,
                    cx,
                ));
            }
            grid = grid.child(model_field);
            let mut effort_field = div().flex().flex_col().gap(px(5.)).flex_1().min_w(px(150.));
            effort_field = effort_field.child(
                div()
                    .text_size(px(10.))
                    .font_weight(FontWeight(650.))
                    .text_color(rgba(colors.muted))
                    .child(self.locale.text("推理强度", "Reasoning effort")),
            );
            effort_field = effort_field.child(self.render_compaction_trigger(
                CompactionSelect::Effort,
                &colors,
                window,
                cx,
            ));
            grid = grid.child(effort_field);
            card = card.child(grid);
            if fallback {
                card = card.child(
                    div()
                        .text_size(px(10.))
                        .line_height(relative(1.5))
                        .text_color(rgba(colors.muted))
                        .child(self.locale.text(
                            "推理强度选项为候选，实际可用性以供应商支持为准。",
                            "Reasoning effort options are candidates; actual availability depends on provider support.",
                        )),
                );
            }
            if let Some(problem) = validation {
                card = card.child(mark_provider(
                    div()
                        .text_size(px(11.))
                        .line_height(relative(1.55))
                        .text_color(rgba(colors.error_text))
                        .child(problem),
                    "compaction-validation",
                    problem,
                    true,
                ));
            }
            card = card.child(
                div()
                    .text_size(px(10.))
                    .line_height(relative(1.6))
                    .text_color(rgba(colors.muted))
                    .whitespace_normal()
                    .child(if cross_provider {
                        self.locale.text(
                            "跨供应商压缩时，当前对话历史会交给所选供应商；不会修改会话的工作模型。",
                            "With another provider, the current conversation history is sent to that provider; the session working model stays unchanged.",
                        )
                    } else {
                        self.locale.text(
                            "仅压缩请求使用所选模型与推理强度，不会修改会话的工作模型。",
                            "Only compaction requests use the chosen model and effort; the session working model stays unchanged.",
                        )
                    }),
            );
        }
        let can_save = draft.mode == "custom"
            && dirty
            && validation.is_none()
            && !saving
            && self.providers.compaction().is_some();
        if draft.mode == "custom" || dirty {
            let mut actions = div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(10.))
                .mt(px(2.));
            let mut save = div()
                .id("compaction-save")
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .px(px(14.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgba(colors.primary))
                .bg(rgba(colors.primary))
                .text_size(px(11.))
                .font_weight(FontWeight(650.))
                .text_color(rgb(0xffffff))
                .child(if saving {
                    self.locale.text("保存中…", "Saving…")
                } else {
                    self.locale.text("保存", "Save")
                });
            if can_save || (draft.mode == "follow" && dirty && !saving) {
                save = save
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgba(colors.primary_hover)));
                save = wire_button(
                    save,
                    &self.compaction.save_focus,
                    7.,
                    self.keyboard_focus,
                    |this, _, cx| this.save_compaction_now(cx),
                    cx,
                );
            } else {
                save = save.opacity(0.55).cursor(CursorStyle::OperationNotAllowed);
            }
            actions = actions.child(mark_provider(
                save,
                "compaction-save",
                self.locale.text("保存", "Save"),
                can_save || (draft.mode == "follow" && dirty && !saving),
            ));
            if self.compaction.saved && draft.mode == "custom" {
                actions = actions.child(
                    div()
                        .text_size(px(10.))
                        .line_height(relative(1.5))
                        .text_color(rgb(0x16794b))
                        .whitespace_normal()
                        .child(self.locale.text(
                            "已保存，下次启动或恢复会话时生效；正在运行的会话不会被打断。",
                            "Saved. Applies to the next launched or resumed session; running sessions are not interrupted.",
                        )),
                );
            }
            card = card.child(actions);
        }
        section = section.child(card);
        if let Some(error) = self
            .providers
            .compaction_error()
            .filter(|_| !saving && self.providers.compaction().is_some())
        {
            section = section.child(
                div()
                    .mt(px(8.))
                    .text_size(px(11.))
                    .text_color(rgba(colors.error_text))
                    .whitespace_normal()
                    .child(error.to_owned()),
            );
        }
        section
    }

    fn render_compaction_trigger(
        &self,
        select: CompactionSelect,
        colors: &EditorColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.compaction_trigger_enabled(select);
        let focus = self.compaction.focus[select.index()].clone();
        let open_here = self
            .compaction
            .open
            .as_ref()
            .is_some_and(|menu| menu.select == select);
        let label = self.compaction_label(select);
        let mut trigger = div()
            .id(SharedString::from(format!(
                "compaction-trigger-{}",
                select.index()
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
            .border_color(rgba(if open_here || focus.is_focused(window) {
                colors.focus
            } else {
                colors.input_line
            }))
            .bg(rgba(colors.input_background))
            .text_size(px(12.))
            .text_color(rgba(colors.label))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label.clone()),
            )
            .child(icon("chevron-down", 12., colors.muted >> 8));
        let bounds = self.compaction.bounds.clone();
        trigger = trigger.child(
            canvas(
                |_, _, _| (),
                move |painted, _, _, _| {
                    bounds.borrow_mut()[select.index()] = Some(painted);
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        );
        let marker = match select {
            CompactionSelect::Provider => "compaction-provider",
            CompactionSelect::Model => "compaction-model",
            CompactionSelect::Effort => "compaction-effort",
        };
        let trigger = if enabled {
            trigger
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
                    this.open_compaction_menu(select, window, cx);
                }))
                .on_key_down(cx.listener(move |this, event, window, cx| {
                    this.compaction_menu_key(select, event, window, cx);
                }))
                .child(focus_ring_with(
                    focus,
                    7.,
                    self.keyboard_focus,
                    FocusRingStyle::Shell,
                ))
        } else {
            trigger
                .opacity(0.55)
                .cursor(CursorStyle::OperationNotAllowed)
        };
        mark_provider(trigger, marker, label, enabled).into_any_element()
    }

    fn render_compaction_menu(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(menu) = &self.compaction.open else {
            return Vec::new();
        };
        let Some(bounds) = (*self.compaction.bounds.borrow())[menu.select.index()] else {
            return Vec::new();
        };
        let palette = *self.theme.effective().palette();
        let colors = EditorColors::new(&palette, self.is_dark());
        let viewport = window.viewport_size();
        let backdrop = deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("compaction-menu-backdrop")
                    .w(viewport.width)
                    .h(viewport.height)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.compaction.open = None;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
            ),
        )
        .with_priority(100)
        .into_any_element();
        let width = f32::from(bounds.size.width).max(menu.select.menu_min_width());
        let mut items = div()
            .id("compaction-menu")
            .w(px(width))
            .max_h(px(280.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.panel))
            .shadow(vec![shadow(0x1a304b, 0.16, 8., 24.)])
            .overflow_y_scroll()
            .track_scroll(&self.compaction.scroll)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        for (index, option) in menu.items.iter().enumerate() {
            let selected = option.value == menu.selected;
            let active = index == menu.active;
            let enabled = option.enabled;
            let label = option.label.clone();
            let value = option.value.clone();
            let select = menu.select;
            let mut item = div()
                .id(SharedString::from(format!(
                    "compaction-option-{}-{index}",
                    menu.select.index()
                )))
                .flex_none()
                .min_h(px(30.))
                .flex()
                .items_center()
                .px(px(9.))
                .py(px(4.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(rgba(if !enabled {
                    colors.muted
                } else if selected {
                    colors.link
                } else {
                    colors.label
                }))
                .bg(rgba(if selected || active {
                    colors.menu_selected_bg
                } else {
                    colors.panel
                }))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label);
            if enabled {
                item = item
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgba(colors.menu_selected_bg)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, event, window, cx| {
                        if matches!(event, ClickEvent::Keyboard(_)) {
                            return;
                        }
                        cx.stop_propagation();
                        this.choose_compaction_option(select, value.clone(), window, cx);
                    }));
            } else {
                item = item.opacity(0.65);
            }
            items = items.child(item);
        }
        let menu = deferred(
            anchored()
                .position(point(bounds.left(), bounds.bottom() + px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(items),
        )
        .with_priority(101)
        .into_any_element();
        vec![backdrop, menu]
    }

    fn load_dispatch(&mut self, cx: &mut Context<Self>) {
        self.dispatch_panel = Some(
            self.dispatch_config
                .clone()
                .ok_or_else(|| LOAD_FAILED.to_owned())
                .and_then(|config| {
                    let snapshot = config.snapshot().map_err(|error| {
                        eprintln!("Read dispatch defaults: {error}");
                        LOAD_FAILED.to_owned()
                    })?;
                    let form = DispatchForm::new(snapshot).map_err(str::to_owned)?;
                    Ok(cx.new(|cx| {
                        DispatchSettingsPanel::new(
                            form,
                            move |defaults| {
                                config
                                    .save_defaults(defaults)
                                    .map_err(|error| error.to_string())
                            },
                            cx,
                        )
                    }))
                }),
        );
        cx.notify();
    }

    pub(crate) fn configure_context_optimizer_root(&mut self, data_root: std::path::PathBuf) {
        self.context_optimizer_config = Some(Rc::new(ContextOptimizerConfig::for_data_root(data_root)));
        self.context_optimizer_panel = None;
    }

    fn load_context_optimizer(&mut self, cx: &mut Context<Self>) {
        let locale = self.locale;
        let theme = self.theme.effective();
        self.context_optimizer_panel = Some(
            self.context_optimizer_config
                .clone()
                .ok_or_else(|| {
                    locale
                        .text("未能定位数据目录", "Data root unavailable")
                        .to_owned()
                })
                .and_then(|config| {
                    let panel = ContextOptimizerSettingsPanel::new(config, locale, theme)?;
                    Ok(cx.new(|_| panel))
                }),
        );
        cx.notify();
    }

    fn context_optimizer_content(&mut self, cx: &mut Context<Self>) -> Div {
        if self.context_optimizer_panel.is_none() {
            self.load_context_optimizer(cx);
        }
        match self.context_optimizer_panel.as_ref() {
            Some(Ok(panel)) => div().w_full().child(panel.clone()),
            Some(Err(err)) => div()
                .w_full()
                .py(px(12.))
                .text_size(px(11.5))
                .text_color(rgb(0xb34d47))
                .child(err.clone()),
            None => div(),
        }
    }

    pub(crate) fn configure_theme(&mut self, theme: ThemeState) {
        self.theme = theme;
    }

    pub(crate) fn theme_saved(&mut self, theme: ThemeState, cx: &mut Context<Self>) {
        self.configure_theme(theme);
        self.theme_error = None;
        cx.notify();
    }

    pub(crate) fn theme_save_failed(&mut self, cx: &mut Context<Self>) {
        self.theme_error = Some(
            self.locale
                .text(
                    "主题保存失败，请重试。",
                    "The theme couldn’t be saved. Try again.",
                )
                .into(),
        );
        cx.notify();
    }

    pub(crate) fn configure_locale(
        &mut self,
        preference: LocalePreference,
        locale: InterfaceLocale,
        saving: bool,
    ) {
        self.locale = locale;
        self.language = preference;
        self.language_block.busy = saving;
    }

    pub(crate) fn locale_saved(
        &mut self,
        preference: LocalePreference,
        locale: InterfaceLocale,
        from_this_view: bool,
        cx: &mut Context<Self>,
    ) {
        self.configure_locale(preference, locale, false);
        self.language_block.error = LanguageZoneError::None;
        self.forced_locale = (from_this_view
            && preference != LocalePreference::System
            && preference.value() != locale.value())
        .then_some(locale);
        cx.notify();
    }

    pub(crate) fn locale_save_failed(&mut self, cx: &mut Context<Self>) {
        self.language_block.busy = false;
        self.language_block.error = LanguageZoneError::Save;
        cx.notify();
    }

    fn request_time_zone(&mut self, save: bool, cx: &mut Context<Self>) {
        let (generation, phase) = self.commands.connection();
        if self.time_zone_request.is_some()
            || (save && matches!(phase, ConnectionPhase::ReadOnly { .. }))
        {
            return;
        }
        let request_id = NEXT_TIME_ZONE_REQUEST.fetch_add(1, Ordering::Relaxed);
        let command = if save {
            Command::SetTimeZoneSettings {
                request_id,
                time_zone: json!(self.time_zone_draft),
            }
        } else {
            self.time_zone_block.available = false;
            Command::GetTimeZoneSettings { request_id }
        };
        self.time_zone_block.saved = false;
        self.time_zone_block.error = LanguageZoneError::None;
        if self.commands.send_at(generation, command).is_err() {
            self.time_zone_failed(save, true);
            return;
        }
        self.time_zone_request = Some((request_id, generation, save));
        self.time_zone_block.busy = save;
        let timer = cx.background_executor().timer(Duration::from_secs(10));
        cx.spawn(async move |shell, cx| {
            timer.await;
            let _ = shell.update(cx, |shell, cx| {
                if shell.time_zone_request == Some((request_id, generation, save)) {
                    shell.time_zone_failed(save, false);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn time_zone_failed(&mut self, save: bool, disconnected: bool) {
        self.time_zone_request = None;
        self.time_zone_block.busy = false;
        self.time_zone_block.error = if save {
            LanguageZoneError::Save
        } else if disconnected {
            LanguageZoneError::Unavailable
        } else {
            LanguageZoneError::Read
        };
    }

    pub(crate) fn receive_time_zone(
        &mut self,
        reply: TimeZoneSettingsReply,
        cx: &mut Context<Self>,
    ) {
        let Some((request, generation, save)) = self.time_zone_request else {
            return;
        };
        if reply.request_id != request || self.commands.connection().0 != generation {
            return;
        }
        self.time_zone_request = None;
        self.time_zone_block.busy = false;
        match reply.result {
            Ok(snapshot) => {
                self.time_zone_draft.clone_from(&snapshot.time_zone);
                self.time_zone = Some(snapshot);
                self.time_zone_block = LanguageZoneBlock {
                    available: true,
                    saved: save,
                    ..Default::default()
                };
            }
            Err(_) => self.time_zone_failed(save, false),
        }
        cx.notify();
    }

    fn language_zone_action(
        &mut self,
        action: LanguageZoneAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use LanguageZoneAction::*;

        match action {
            SelectLanguage(_) if self.language_block.busy => {
                return;
            }
            RetryTimeZone if self.time_zone_block.busy => return,
            SelectTimeZone(_) | SaveTimeZone | CancelTimeZone | ResetTimeZone
            | ToggleTimeZoneMenu
                if self.time_zone_block.busy || self.time_zone_read_only() =>
            {
                return;
            }
            SelectLanguage(preference) => {
                let Some(preference) = LocalePreference::from_value(&preference) else {
                    return;
                };
                if preference == self.language {
                    return;
                }
                self.language_block.busy = true;
                self.language_block.error = LanguageZoneError::None;
                cx.emit(SettingsShellEvent::SaveLocale(preference));
            }
            SelectTimeZone(value) if self.time_zone_block.available => {
                self.time_zone_draft = value;
                self.time_zone_menu_open = false;
                self.time_zone_block.saved = false;
                self.time_zone_block.error = LanguageZoneError::None;
            }
            SaveTimeZone if self.time_zone_dirty() => self.request_time_zone(true, cx),
            CancelTimeZone | ResetTimeZone if self.time_zone_block.available => {
                let zone = self.time_zone.as_ref().unwrap();
                self.time_zone_draft = if action == ResetTimeZone {
                    "auto".into()
                } else {
                    zone.time_zone.clone()
                };
                self.time_zone_block.saved = false;
                self.time_zone_block.error = LanguageZoneError::None;
                window.focus(&self.language_zone_focus.time_zone_trigger);
            }
            RetryTimeZone => self.request_time_zone(false, cx),
            ToggleTimeZoneMenu if self.time_zone_block.available => {
                self.time_zone_menu_open = !self.time_zone_menu_open
            }
            _ => return,
        }
        cx.notify();
    }

    fn time_zone_dirty(&self) -> bool {
        self.time_zone_block.available
            && self
                .time_zone
                .as_ref()
                .is_some_and(|zone| zone.time_zone != self.time_zone_draft)
    }

    fn time_zone_read_only(&self) -> bool {
        matches!(
            self.commands.connection().1,
            ConnectionPhase::ReadOnly { .. }
        )
    }

    fn render_language_zone(&self, window: &Window, cx: &Context<Self>) -> Div {
        let snapshot = self
            .time_zone
            .as_ref()
            .filter(|_| self.time_zone_block.available);
        let mut view = LanguageZoneView {
            english: self.locale == InterfaceLocale::English,
            language: self.language.value(),
            time_zone: &self.time_zone_draft,
            system_time_zone: snapshot.map_or("", |zone| zone.system_time_zone.as_str()),
            auto_time_zone: snapshot.and_then(|zone| zone.auto_time_zone.as_deref()),
            zones: snapshot.map_or(&[], |zone| zone.zones.as_slice()),
            preview: None,
            forced_locale: self.forced_locale.map(InterfaceLocale::value),
            language_block: self.language_block,
            time_zone_block: LanguageZoneBlock {
                dirty: self.time_zone_dirty(),
                ..self.time_zone_block
            },
            time_zone_menu_open: self.time_zone_menu_open,
            time_zone_read_only: self.time_zone_read_only(),
            keyboard_focus: self.keyboard_focus,
            focus: &self.language_zone_focus,
        };
        let preview = snapshot
            .and_then(|_| format_time_zone_preview(view.effective_time_zone(), view.english));
        view.preview = preview.as_deref();
        let shell = cx.weak_entity();
        let palette = self.theme.effective().palette();
        let surface = rgba(palette.workspace.canvas);
        language_zone_content(
            view,
            window,
            Rc::new(move |action, window, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.language_zone_action(action, window, cx)
                });
            }),
            palette,
            surface,
        )
    }

    /// Workspace consults this before moving focus with Tab so focus cannot
    /// escape the dialog. The dialog registers its own Tab/Escape path.
    pub fn modal_open(&self) -> bool {
        self.editor.is_some()
    }

    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_time_zone_menu(cx) {
            window.focus(&self.language_zone_focus.time_zone_trigger);
            return;
        }
        if self.dismiss_compaction_menu(cx) {
            return;
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.step == EditorStep::Subscription)
        {
            self.editor_request_close(window, cx);
            return;
        }
        if let Some(editor) = &mut self.editor {
            if self.providers.saving() || self.providers.deleting() {
                return;
            }
            // Escape is a keyboard path; keep the focus ring visible when this
            // opens the discard confirmation or returns focus to a control.
            self.keyboard_focus = true;
            // Frozen order: cancel the confirmation first, then close
            // the effort picker, then request the dialog close.
            if editor.confirm.is_some() {
                self.editor_keep_editing(window, cx);
                return;
            }
            if let Some((select, _)) = editor.select_open.take() {
                let focus = editor.select_focus(select).clone();
                window.focus(&focus);
                cx.notify();
                return;
            }
            if editor.step == EditorStep::Form && editor.is_dirty(cx) {
                editor.ask_confirmation(ConfirmKind::Close, window, cx);
                return;
            }
            self.close_editor(window, cx);
            return;
        }
        if self.providers.switching().is_some() {
            return;
        }
        if self
            .assignment_view
            .update(cx, |view, cx| view.close_menu(window, cx))
        {
            cx.notify();
            return;
        }
        cx.emit(SettingsShellEvent::Close);
    }

    pub(crate) fn dismiss_time_zone_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let open = std::mem::take(&mut self.time_zone_menu_open);
        if open {
            cx.notify();
        }
        open
    }

    fn load_assignments(&mut self, cx: &mut Context<Self>) {
        let request = self.assignments.load();
        self.watch_assignment_reply(request, cx);
    }

    fn watch_assignment_reply(&self, request: Option<u64>, cx: &mut Context<Self>) {
        if let Some(request_id) = request {
            // The frozen settings client times out unanswered requests at 10 seconds.
            let timer = cx.background_executor().timer(Duration::from_secs(10));
            cx.spawn(async move |shell, cx| {
                timer.await;
                let _ = shell.update(cx, |shell, cx| {
                    if shell.assignments.expire(request_id) {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn assignment_action(
        &mut self,
        action: &AssignmentAction,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        match action {
            AssignmentAction::Relogin(provider_id) => {
                let Some(window) = window else {
                    return;
                };
                let Some(provider) = self
                    .providers
                    .providers()
                    .iter()
                    .find(|provider| provider["id"] == provider_id.as_str())
                    .cloned()
                else {
                    return;
                };
                let opener = self.new_provider_focus.clone();
                self.open_subscription_relogin(provider, opener, window, cx);
                return;
            }
            AssignmentAction::RetryCatalog(index)
                if self
                    .assignments
                    .uses_native_catalog(*index, &self.providers) =>
            {
                self.load_assignment_catalog(true, cx);
            }
            AssignmentAction::Retry | AssignmentAction::RetryCatalog(_) => {
                self.providers.load();
                self.load_assignments(cx);
            }
            AssignmentAction::Save => {
                let request = self.assignments.save(&self.providers);
                self.watch_assignment_reply(request, cx);
            }
            _ => self.assignments.change(action, &self.providers),
        }
        self.load_assignment_catalog(false, cx);
        cx.notify();
    }

    fn load_assignment_catalog(&mut self, retry: bool, cx: &mut Context<Self>) {
        if self.page == SettingsPage::ClaudeCode {
            let request = self.assignments.load_catalog(&self.providers, retry);
            self.watch_assignment_reply(request, cx);
        }
    }

    pub fn receive_model_catalog(
        &mut self,
        reply: ClaudeModelCatalogReply,
        cx: &mut Context<Self>,
    ) {
        self.assignments.receive_catalog(reply);
        cx.notify();
    }

    pub fn receive_assignments(
        &mut self,
        reply: ClaudeRoleAssignmentsReply,
        cx: &mut Context<Self>,
    ) {
        self.assignments.receive(reply);
        self.load_assignment_catalog(false, cx);
        cx.notify();
    }

    /// Workspace routes the shared Claude model-preferences reply here. Only a
    /// matching compaction request updates the block; assignment replies are
    /// still consumed by the existing assignment path.
    pub fn receive_model_preferences(
        &mut self,
        reply: &ClaudeModelPreferencesReply,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.providers.receive_compaction(reply) {
            self.sync_compaction(cx);
            cx.notify();
            true
        } else {
            false
        }
    }

    pub fn receive(
        &mut self,
        reply: matou_native_runtime::protocol::ProviderSettingsReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let switching = self.providers.switching().map(str::to_owned);
        let active_before = switching
            .as_ref()
            .and_then(|_| self.providers.active_id().map(str::to_owned));
        let update = self.providers.receive(reply);
        if let Some(target) = switching.as_deref()
            && self.providers.active_id() == Some(target)
            && active_before.as_deref() != Some(target)
        {
            cx.emit(SettingsShellEvent::ProviderActivated);
        }
        self.apply_update(update, window, cx);
        // A start/reopen reply can move the flow back to waiting without any
        // timer running; restart the single chain here.
        if self.editor.as_ref().is_some_and(|editor| {
            editor.subscription.is_some() && self.providers.subscription_login().phase == "waiting"
        }) {
            self.schedule_subscription_poll(window, cx);
        }
        self.load_assignment_catalog(false, cx);
        cx.notify();
    }

    pub fn connection_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let update = self.providers.connection_changed();
        self.apply_update(update, window, cx);
        let request = self
            .assignments
            .connection_changed(self.page == SettingsPage::ClaudeCode);
        self.watch_assignment_reply(request, cx);
        self.load_assignment_catalog(false, cx);
        if self.page == SettingsPage::Diagnostics {
            let (generation, phase) = self.commands.connection();
            if matches!(
                phase,
                ConnectionPhase::Ready | ConnectionPhase::ReadOnly { .. }
            ) {
                if self.diagnostics_generation != Some(generation) {
                    self.refresh_runtime_diagnostics(cx);
                }
            } else {
                self.diagnostics_generation = None;
                self.runtime_diagnostics.clear();
            }
        }
        if self.page == SettingsPage::ClaudeCode {
            if self.subscription_connected() {
                self.providers.load_compaction();
                self.open_subscription_settings_round(cx);
            } else {
                // The runtime may already have dropped the round with the
                // generation; re-open it after reconnecting.
                self.subscription_settings_open = false;
            }
        }
        if self.page == SettingsPage::Language {
            let (generation, phase) = self.commands.connection();
            let ready = matches!(phase, ConnectionPhase::Ready);
            let read_only = matches!(phase, ConnectionPhase::ReadOnly { .. });
            if !ready {
                self.time_zone_menu_open = false;
            }
            let current = self.time_zone_request.is_some_and(|(_, requested, save)| {
                requested == generation && (ready || (read_only && !save))
            });
            if !current {
                self.time_zone_request = None;
                self.request_time_zone(false, cx);
            }
        }
        cx.notify();
    }

    fn apply_update(
        &mut self,
        update: ProviderUpdate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match update {
            ProviderUpdate::None => {}
            ProviderUpdate::SubscriptionDraft(draft) => {
                self.adopt_subscription_draft(draft, window, cx);
            }
            ProviderUpdate::Models(result) => {
                self.apply_model_fetch(result, window, cx);
            }
            ProviderUpdate::Saved => {
                // The commit already adopted and cleaned its draft; clear the
                // flow first so closing never cancels the saved attempt.
                if let Some(editor) = &mut self.editor {
                    editor.subscription = None;
                }
                self.close_editor(window, cx);
            }
            ProviderUpdate::Deleted => {
                if let Some(editor) = &mut self.editor {
                    editor.subscription = None;
                }
                self.editor = None;
                self.keyboard_focus = false;
                window.focus(&self.new_provider_focus);
            }
            ProviderUpdate::DeleteFailed => {
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(DELETE_FAILED.into());
                    editor.body_scroll.scroll_to_bottom();
                    window.focus(&editor.focus.error);
                }
            }
            ProviderUpdate::SaveFailed(message) => {
                let committed = self.providers.take_committed_provider();
                if let Some(editor) = &mut self.editor {
                    if let Some(provider) = committed {
                        // The save completed but the follow-up switch failed.
                        // Adopt the committed identity so a retry updates the
                        // same row instead of creating a duplicate.
                        editor.draft.id = provider["id"].as_str().map(str::to_owned);
                        editor.draft.has_api_key = provider["hasApiKey"] == true;
                        editor.draft.original_endpoint = provider["endpoint"]
                            .as_str()
                            .unwrap_or(&editor.draft.original_endpoint)
                            .to_owned();
                    }
                    editor.error = Some(message);
                    editor.confirm = None;
                    editor.body_scroll.scroll_to_bottom();
                    let focus = editor.focus.error.clone();
                    window.focus(&focus);
                }
            }
        }
    }

    fn apply_model_fetch(&mut self, result: Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some((sent_key, sent_endpoint)) = editor.fetch_snapshot.take() else {
            return;
        };
        let key = editor.key.read(cx).text().trim().to_owned();
        let endpoint = editor.endpoint.read(cx).text.trim().to_owned();
        if sent_key != key || sent_endpoint != endpoint {
            // The request belonged to the previous connection fields.
            return;
        }
        if let Some(error) = result["error"].as_str() {
            editor.fetch_error = Some(error.to_owned());
            editor.model_chooser = false;
            cx.notify();
            return;
        }
        let metadata = result["modelMetadata"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let ids = result["models"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                metadata
                    .iter()
                    .filter_map(|row| row["id"].as_str().map(str::to_owned))
                    .collect()
            });
        let mut by_id: HashMap<_, _> = metadata
            .into_iter()
            .filter_map(|row| Some((row["id"].as_str()?.to_owned(), row)))
            .collect();
        let rows: Vec<_> = ids
            .into_iter()
            .map(|id| by_id.remove(&id).unwrap_or_else(|| json!({ "id": id })))
            .collect();
        if rows.is_empty() {
            editor.fetch_error = Some("empty".into());
            editor.model_chooser = false;
            cx.notify();
            return;
        }
        editor.draft.fetched_metadata = Some(Value::Array(rows));
        editor.fetch_error = None;
        editor.model_chooser = true;
        editor.model_cursor = 0;
        editor.model_scroll.set_offset(Point::default());
        editor.model_query.update(cx, |input, cx| {
            input.text.clear();
            cx.notify();
        });
        let search = editor.focus.model_search.clone();
        window.focus(&search);
        cx.notify();
    }

    fn clear_editor_error(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.editor {
            editor.error = None;
            cx.notify();
        }
    }

    fn provider_connection_field_changed(&mut self, cx: &mut Context<Self>) {
        self.providers.discard_fetch();
        if let Some(editor) = &mut self.editor {
            editor.error = None;
            editor.fetch_snapshot = None;
            editor.fetch_error = None;
            editor.model_chooser = false;
            editor.draft.fetched_metadata = None;
            editor.draft.model_metadata = json!([]);
            editor.connection_changed = true;
        }
        cx.notify();
    }

    fn choose_first_visible_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.editor {
            editor.choose_first_visible_model(window, cx);
        }
    }

    fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.providers.discard_fetch();
        // An unsaved subscription draft owns a temporarily disabled credential
        // in the runtime; closing must release it. A saved flow is cleared in
        // `apply_update` before this runs.
        self.cancel_subscription_flow();
        if let Some(editor) = self.editor.take() {
            let opener = editor.opener.clone();
            window.focus(&opener);
        }
        self.keyboard_focus = false;
        cx.notify();
    }

    fn editor_request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_busy() {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.confirm.is_some() {
            return;
        }
        if editor.step == EditorStep::Subscription {
            // A start/confirm request in flight blocks closing; the waiting
            // phase keeps its close path because it can still be cancelled.
            let phase = self.providers.subscription_login().phase;
            if self.providers.subscription_login().busy && phase != "waiting" {
                return;
            }
            self.close_editor(window, cx);
            return;
        }
        if editor.step == EditorStep::Form && editor.is_dirty(cx) {
            editor.ask_confirmation(ConfirmKind::Close, window, cx);
            return;
        }
        self.close_editor(window, cx);
    }

    fn editor_go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.providers.saving() {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.confirm.is_some() {
            return;
        }
        if editor.is_dirty(cx) {
            editor.ask_confirmation(ConfirmKind::Back, window, cx);
            return;
        }
        self.editor_back_to_pick(window, cx);
    }

    fn editor_back_to_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.providers.discard_fetch();
        let Some(current) = self.editor.take() else {
            return;
        };
        let query = current.query.read(cx).text.clone();
        let selection = current.selection;
        let opener = current.opener.clone();
        let editor = Editor::new(
            self.locale,
            None,
            EditorStep::Pick,
            None,
            query,
            selection,
            opener,
            window,
            cx,
        );
        let focus = editor.query.read(cx).focus.clone();
        self.editor = Some(editor);
        window.focus(&focus);
        cx.notify();
    }

    fn editor_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.editor.take() else {
            return;
        };
        let Some(selection) = current.selection else {
            self.editor = Some(current);
            return;
        };
        if let PickChoice::Subscription(kind) = selection {
            self.editor = Some(current);
            self.start_subscription_from_pick(kind, window, cx);
            return;
        }
        let query = current.query.read(cx).text.clone();
        let opener = current.opener.clone();
        let preset_id = match selection {
            PickChoice::Preset(id) => Some(id),
            PickChoice::Subscription(_) | PickChoice::Custom => None,
        };
        let editor = Editor::new(
            self.locale,
            None,
            EditorStep::Form,
            preset_id,
            query,
            Some(selection),
            opener,
            window,
            cx,
        );
        let focus = editor.focus.key.clone();
        self.editor = Some(editor);
        window.focus(&focus);
        cx.notify();
    }

    fn editor_save(&mut self, activate: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_busy() {
            return;
        }
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.step != EditorStep::Form {
            return;
        }
        let mut draft = editor.working_draft(cx);
        if draft.subscription_kind.is_some() {
            // A new Codex account keeps the frozen 512K request by omitting the
            // override; an existing account keeps its saved window unless the
            // user edits the picker in this dialog.
            if draft.is_codex() || (!editor.context_touched && editor.draft.id.is_none()) {
                draft.context_window_mode = None;
            }
        } else if !editor.context_touched {
            draft.context_window_mode = None;
        }
        if editor.connection_changed && draft.fetched_metadata.is_none() {
            draft.fetched_metadata = Some(json!([]));
        }
        if editor.subscription.is_some() {
            if draft.name.trim().is_empty() {
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(
                        self.locale
                            .text("请填写显示名称。", "Enter a display name.")
                            .into(),
                    );
                    editor.body_scroll.scroll_to_bottom();
                    let focus = editor.focus.name.clone();
                    window.focus(&focus);
                }
                cx.notify();
                return;
            }
            if !provider_editor_can_save(&draft) {
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(
                        self.locale
                            .text(
                                "没有读到可用的模型，请重新选择。",
                                "No available models were found. Choose another model.",
                            )
                            .into(),
                    );
                    editor.body_scroll.scroll_to_bottom();
                }
                cx.notify();
                return;
            }
            let _ = self.providers.commit_subscription(&draft, activate);
            let validation_error = self.providers.subscription_login().error.clone();
            if let Some(editor) = &mut self.editor {
                editor.error = validation_error;
                if editor.error.is_some() {
                    editor.body_scroll.scroll_to_bottom();
                }
            }
            cx.notify();
            return;
        }
        match self.providers.save(&draft, activate) {
            Ok(()) => {
                if let Some(editor) = &mut self.editor {
                    editor.error = None;
                }
            }
            Err(message) => {
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(message);
                    editor.body_scroll.scroll_to_bottom();
                    let focus = editor.focus.error.clone();
                    window.focus(&focus);
                }
            }
        }
        cx.notify();
    }

    fn open_new_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.providers.saving() || self.editor.is_some() {
            return;
        }
        let opener = self.new_provider_focus.clone();
        let editor = Editor::new(
            self.locale,
            None,
            EditorStep::Pick,
            None,
            String::new(),
            None,
            opener,
            window,
            cx,
        );
        let focus = editor.query.read(cx).focus.clone();
        self.editor = Some(editor);
        window.focus(&focus);
        cx.notify();
    }

    fn open_edit_editor(
        &mut self,
        provider: Value,
        opener: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.providers.saving() || self.editor.is_some() {
            return;
        }
        let preset_id = preset_by_endpoint(provider["endpoint"].as_str().unwrap_or_default())
            .map(|preset| preset.id);
        let editor = Editor::new(
            self.locale,
            Some(provider),
            EditorStep::Form,
            preset_id,
            String::new(),
            None,
            opener,
            window,
            cx,
        );
        let focus = if editor.draft.subscription_kind.is_some() {
            editor.name.read(cx).focus.clone()
        } else {
            editor.focus.key.clone()
        };
        self.editor = Some(editor);
        window.focus(&focus);
        cx.notify();
    }

    fn select_page(&mut self, page: SettingsPage, window: &mut Window, cx: &mut Context<Self>) {
        if self.providers.switching().is_some() || self.editor.is_some() {
            return;
        }
        if self.page == page {
            return;
        }
        if self.page == SettingsPage::ClaudeCode {
            // Leaving the provider page discards page-local editor/picker state
            // and the loaded provider projection; returning reads it again.
            self.cancel_subscription_flow();
            self.compaction.reset();
            self.providers.discard_fetch();
            self.release_subscription_settings_round(cx);
            self.editor = None;
            self.card_controls.clear();
            self.keyboard_focus = false;
            self.providers.reset_for_page();
            self.assignments.reset();
            self.assignment_view
                .update(cx, |view, cx| view.close_menu(window, cx));
        }
        if self.page == SettingsPage::Dispatch {
            // The panel owns an unsaved draft; leaving the page drops it so a
            // return reads the persisted defaults again.
            self.dispatch_panel = None;
        }
        self.page = page;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.time_zone_menu_open = false;
        match page {
            SettingsPage::ClaudeCode => {
                self.providers.load();
                self.providers.load_compaction();
                self.open_subscription_settings_round(cx);
                self.load_assignments(cx);
            }
            SettingsPage::Language => {
                if self.time_zone_request.is_none() {
                    self.request_time_zone(false, cx);
                }
            }
            SettingsPage::Dispatch => {
                if self.dispatch_panel.is_none() {
                    self.load_dispatch(cx);
                }
            }
            SettingsPage::Diagnostics => {
                // Entering the page asks the host for fresh read-only
                // account, environment and browser projections.
                self.refresh_runtime_diagnostics(cx);
            }
            SettingsPage::ContextOptimizer => {
                self.load_context_optimizer(cx);
            }
        }
        let focus = self.nav_focus[page.index()].clone();
        window.focus(&focus);
        cx.notify();
    }

    fn editor_busy(&self) -> bool {
        self.providers.saving() || self.providers.deleting()
    }

    fn busy(&self) -> bool {
        self.editor_busy() || self.providers.switching().is_some()
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_assignments(&self) -> (AssignmentPanel, (bool, bool, bool, bool)) {
        (
            self.assignments.panel(&self.providers),
            self.assignments.diagnostics(),
        )
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_assignment_action(
        &mut self,
        action: &AssignmentAction,
        cx: &mut Context<Self>,
    ) {
        if matches!(action, AssignmentAction::Relogin(_)) {
            return;
        }
        self.assignment_action(action, None, cx);
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_assignment_action_with_window(
        &mut self,
        action: &AssignmentAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assignment_action(action, Some(window), cx);
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_focus_assignment_provider(&self, window: &mut Window, cx: &App) {
        self.assignment_view.read(cx).focus_first_provider(window);
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn probe_language_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_page(SettingsPage::Language, window, cx);
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn select_dispatch_for_probe(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_page(SettingsPage::Dispatch, window, cx);
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn probe_dispatch_focus(&self, index: Option<usize>, window: &mut Window, cx: &App) {
        let panel = self
            .dispatch_panel
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .read(cx);
        if let Some(index) = index {
            panel.probe_focus_number(index, window, cx);
        } else {
            panel.probe_focus_save(window);
        }
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn probe_dispatch_config(&mut self, config: DispatchConfig) {
        self.dispatch_config = Some(Rc::new(config));
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn probe_dispatch_load_failed(&self) -> bool {
        matches!(self.dispatch_panel, Some(Err(_)))
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn probe_dispatch_retry_focus(&self, window: &mut Window) {
        window.focus(&self.dispatch_retry_focus);
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn probe_settings_nav_focus(&self, page: SettingsPage, window: &mut Window) {
        window.focus(&self.nav_focus[page.index()]);
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn probe_language_focus(&self, control: &str, window: &mut Window) {
        let focus = &self.language_zone_focus;
        window.focus(match control {
            "english" => &focus.language_options[2],
            "chinese" => &focus.language_options[1],
            "zone" => &focus.time_zone_trigger,
            "zone-reset" => &focus.time_zone_reset,
            "zone-cancel" => &focus.time_zone_cancel,
            "zone-save" => &focus.time_zone_save,
            "zone-retry" => &focus.time_zone_retry,
            _ => panic!("Unknown settings control"),
        });
    }

    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) fn probe_invalid_zone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.language_zone_action(
            LanguageZoneAction::SelectTimeZone("Mars/Olympus".into()),
            window,
            cx,
        );
    }

    // Probe surface for hidden GPUI integration tests. These helpers expose
    // only UI state and the empty/never-plaintext key marker.
    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn page(&self) -> SettingsPage {
        self.page
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_dialog(&self) -> Option<EditorStep> {
        self.editor.as_ref().map(|editor| editor.step)
    }

    #[cfg(debug_assertions)]
    pub(crate) fn focus_provider_key_link_for_probe(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = SettingsPage::ClaudeCode;
        self.open_new_editor(window, cx);
        self.editor.as_mut().unwrap().selection = Some(PickChoice::Preset("deepseek"));
        self.editor_next(window, cx);
        window.focus(&self.editor.as_ref().unwrap().focus.key_link);
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_dialog_error(&self) -> Option<&str> {
        self.editor
            .as_ref()
            .and_then(|editor| editor.error.as_deref())
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_confirm(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.confirm.is_some())
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_focus_provider_edit(&self, id: &str, window: &mut Window) {
        window.focus(&self.card_controls[id][0]);
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_delete_editor(
        &self,
        window: &Window,
        cx: &App,
    ) -> (bool, bool, String) {
        let editor = self.editor.as_ref().expect("provider editor is open");
        (
            editor.focus.delete.is_focused(window),
            editor.confirm == Some(ConfirmKind::Delete),
            editor.key.read(cx).text().to_owned(),
        )
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_providers(&self) -> (&[Value], Option<(&str, &str, bool)>) {
        (self.providers.providers(), self.providers.notice())
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_key_empty(&self, cx: &App) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.key.read(cx).text().is_empty())
    }

    #[cfg(any(test, debug_assertions))]
    #[allow(dead_code)]
    pub(crate) fn diagnostics_key_masked(&self) -> bool {
        self.editor.is_some()
    }
}

/// Bottom-bar entry. The parent supplies the focus handle and mouse/keyboard
/// wiring; this function owns only the frozen 30px visual and its content.
pub fn settings_button(pressed: bool, locale: InterfaceLocale) -> Stateful<Div> {
    let base = div()
        .id("settings-shell-toggle")
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(7.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(7.))
        .text_size(px(12.))
        .font_weight(FontWeight(600.))
        .cursor_pointer();
    let icon_color = if pressed { 0x80b9ff } else { 0x989da7 };
    let button = if pressed {
        base.text_color(rgb(0x80b9ff)).bg(rgba(0x4d91e826)).child(
            canvas(
                |_, _, _| (),
                |bounds, _, window, _| {
                    window.paint_quad(
                        outline(bounds, rgba(0x67a5f029), BorderStyle::default())
                            .corner_radii(px(7.))
                            .border_widths(px(1.)),
                    );
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
    } else {
        base.text_color(rgb(0x989da7))
            .bg(rgba(0x00000000))
            .hover(|style| style.text_color(rgb(0xe8ebf0)).bg(rgba(0xffffff11)))
    };
    button
        .child(icon("settings-2", 16., icon_color))
        .child(locale.text("设置", "Settings"))
}

fn wire_button_with_ring<E: StatefulInteractiveElement + ParentElement + Styled>(
    element: E,
    focus: &FocusHandle,
    radius: f32,
    keyboard_focus: bool,
    ring: FocusRingStyle,
    activate: impl Fn(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>) + 'static,
    cx: &mut Context<SettingsShell>,
) -> E {
    let activate = Rc::new(activate);
    let key_activate = activate.clone();
    let mouse_focus = focus.clone();
    element
        .relative()
        .track_focus(focus)
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
            activate(this, window, cx);
        }))
        .child(focus_ring_with(focus.clone(), radius, keyboard_focus, ring))
}

fn wire_button<E: StatefulInteractiveElement + ParentElement + Styled>(
    element: E,
    focus: &FocusHandle,
    radius: f32,
    keyboard_focus: bool,
    activate: impl Fn(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>) + 'static,
    cx: &mut Context<SettingsShell>,
) -> E {
    wire_button_with_ring(
        element,
        focus,
        radius,
        keyboard_focus,
        FocusRingStyle::Shell,
        activate,
        cx,
    )
}

fn wire_dialog_button<E: StatefulInteractiveElement + ParentElement + Styled>(
    element: E,
    focus: &FocusHandle,
    radius: f32,
    keyboard_focus: bool,
    activate: impl Fn(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>) + 'static,
    cx: &mut Context<SettingsShell>,
) -> E {
    wire_button_with_ring(
        element,
        focus,
        radius,
        keyboard_focus,
        FocusRingStyle::Dialog,
        activate,
        cx,
    )
}

fn rename_field(
    input: Entity<RenameInput>,
    height: f32,
    horizontal: f32,
    window: &Window,
    colors: &EditorColors,
    cx: &mut App,
) -> Div {
    let text_color = colors.input_text.map(rgba);
    let placeholder_color = colors.input_placeholder.map(rgba);
    input.update(cx, |input, _| {
        input.text_color = text_color;
        input.placeholder_color = placeholder_color;
    });
    let focus = input.read(cx).focus.clone();
    let focused = focus.is_focused(window);
    let field = div()
        .flex()
        .items_center()
        .w_full()
        .h(px(height))
        .px(px(horizontal))
        .rounded(px(9.))
        .border_1()
        .border_color(rgba(if focused {
            colors.focus
        } else {
            colors.input_line
        }))
        .bg(rgba(colors.input_background));
    let field = if focused {
        field.shadow(input_focus_shadow(colors))
    } else {
        field
    };
    field
        .on_mouse_down(MouseButton::Left, move |_, window, _| {
            window.focus(&focus);
        })
        .child(input)
}

fn secret_field(
    input: Entity<SecretInput>,
    height: f32,
    horizontal: f32,
    revealed: bool,
    reveal_focus: &FocusHandle,
    keyboard_focus: bool,
    window: &Window,
    colors: &EditorColors,
    cx: &mut Context<SettingsShell>,
    activate: impl Fn(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>) + 'static,
) -> Div {
    let text_color = colors.input_text.map(rgba);
    let placeholder_color = colors.input_placeholder.map(rgba);
    input.update(cx, |input, _| {
        input.text_color = text_color;
        input.placeholder_color = placeholder_color;
        input.revealed = revealed;
    });
    let focus = input.read(cx).focus.clone();
    let focused = focus.is_focused(window);
    let field = div()
        .relative()
        .flex()
        .items_center()
        .w_full()
        .h(px(height))
        .pl(px(horizontal))
        .pr(px(horizontal + 31.))
        .rounded(px(9.))
        .border_1()
        .border_color(rgba(if focused {
            colors.focus
        } else {
            colors.input_line
        }))
        .bg(rgba(colors.input_background));
    let field = if focused {
        field.shadow(input_focus_shadow(colors))
    } else {
        field
    };
    let reveal = div()
        .id("provider-key-reveal")
        .size(px(34.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .text_size(px(20.))
        .text_color(rgba(colors.muted))
        .hover(move |style| style.bg(rgba(colors.card_hover_bg)))
        .child(if revealed { "◉" } else { "◎" });
    field
        .on_mouse_down(MouseButton::Left, move |_, window, _| {
            window.focus(&focus);
        })
        .child(input)
        .child(
            wire_dialog_button(reveal, reveal_focus, 7., keyboard_focus, activate, cx)
                .absolute()
                .right(px(5.))
                .top(px(5.)),
        )
}

fn preset_matches(preset: &ProviderPreset, query: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    query.is_empty()
        || preset.name.to_ascii_lowercase().contains(&query)
        || preset.id.to_ascii_lowercase().contains(&query)
        || preset.endpoint.to_ascii_lowercase().contains(&query)
}

// ---------------------------------------------------------------------------
// Shell and page rendering
// ---------------------------------------------------------------------------

// Static section heading in the frozen nav (`hierarchy.css`
// `.model-settings__section-label`: 9/700, `letter-spacing: .08em`).
// GPUI has no CSS letter-spacing, so this tiny label is split into glyphs and
// each glyph gets 0.72pt tracking; spaces keep their normal glyph width.
fn nav_section_label(text: &'static str, color: u32) -> Div {
    let glyphs: Vec<char> = text.chars().collect();
    let mut label = div()
        .flex()
        .items_center()
        .h(px(14.))
        .mx(px(9.))
        .mb(px(7.))
        .text_size(px(9.))
        .font_weight(FontWeight(700.))
        .text_color(rgba(color));
    for (index, glyph) in glyphs.iter().enumerate() {
        // Keep the word gap measurable when the label is split into flex cells;
        // the no-break space has the normal space advance.
        let glyph = if *glyph == ' ' {
            '\u{00a0}'.to_string()
        } else {
            glyph.to_string()
        };
        let mut cell = div().flex_none().child(glyph);
        if index + 1 < glyphs.len() {
            cell = cell.mr(px(0.72));
        }
        label = label.child(cell);
    }
    label
}

impl SettingsShell {
    fn render_nav(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        let switching = self.providers.switching().is_some();
        let palette = *self.theme.effective().palette();
        let dark = self.theme.effective() == AppTheme::Dark;
        let mut nav = div()
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .px(px(10.))
            .py(px(22.))
            .border_r_1()
            .border_color(rgba(palette.workspace.line))
            .bg(rgba(palette.settings.nav));

        nav = nav.child(nav_section_label(
            self.locale.text("AI 服务", "AI services"),
            palette.workspace.muted,
        ));
        for page in [
            SettingsPage::ClaudeCode,
            SettingsPage::ContextOptimizer,
            SettingsPage::Language,
        ] {
            let active = self.page == page;
            let focus = self.nav_focus[page.index()].clone();
            let icon_color = if active {
                palette.workspace.accent >> 8
            } else {
                palette.settings.nav_text
            };
            let mut row = div()
                .id(SharedString::from(format!("settings-nav-{}", page.index())))
                .w_full()
                .h(px(33.))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .rounded(px(7.))
                .text_size(px(11.))
                .font_weight(FontWeight(650.))
                .text_color(rgb(icon_color))
                .bg(if active {
                    rgba(palette.workspace.accent_soft)
                } else {
                    rgba(0x00000000)
                });
            if !active {
                let hover = if dark {
                    palette.workspace.line
                } else {
                    0x5a67780a
                };
                row = row.hover(move |style| style.bg(rgba(hover)));
            }
            if !switching {
                row = wire_button(
                    row,
                    &focus,
                    7.,
                    true,
                    move |this, window, cx| this.select_page(page, window, cx),
                    cx,
                );
            } else {
                row = row.opacity(0.55);
            }
            let row = match page {
                SettingsPage::ClaudeCode => row
                    .child(icon("settings-2", 15., icon_color))
                    .child("Claude Code"),
                SettingsPage::ContextOptimizer => row
                    .child(icon("settings-2", 15., icon_color))
                    .child(page.title(self.locale)),
                SettingsPage::Language => row.child(globe_icon(15., icon_color)).child(
                    // At 720 the frozen English label wraps onto two lines
                    // inside the same 33pt row; the Chinese label stays on
                    // one. Let the text column shrink and wrap instead of
                    // pushing past the nav edge.
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_height(px(14.))
                        .whitespace_normal()
                        .child(language_zone_label(page.title(self.locale))),
                ),
                _ => unreachable!(),
            };
            nav = nav.child(row);
            if page == SettingsPage::ContextOptimizer {
                nav = nav.child(nav_section_label(
                    self.locale.text("通用", "General"),
                    palette.workspace.muted,
                ));
            }
        }
        // Frozen DOM places the Claude note after the navigation entries.
        if self.page == SettingsPage::ClaudeCode {
            nav = nav.child(
                div()
                    .mt(px(9.))
                    .mx(px(9.))
                    .text_size(px(9.))
                    .line_height(px(14.))
                    .text_color(rgba(palette.workspace.muted))
                    .child(self.locale.text(
                        "供应商与上下文配置将同步影响所有 码头 窗口。",
                        "Provider and context settings apply to every Matou window.",
                    )),
            );
        }
        nav
    }

    fn page_header(&self, page: SettingsPage, cx: &mut Context<Self>) -> Div {
        let palette = *self.theme.effective().palette();
        let dark = self.theme.effective() == AppTheme::Dark;
        let switching = self.providers.switching().is_some();
        let mut close = div()
            .id("settings-close")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded(px(7.))
            .text_size(px(16.))
            .line_height(relative(1.))
            .text_color(rgba(palette.workspace.muted))
            .bg(rgba(if dark {
                palette.workspace.line
            } else {
                0x5a67780e
            }))
            .child("×");
        if switching {
            close = close.opacity(0.55);
        } else {
            let hover_text = palette.workspace.text;
            let hover_bg = palette.workspace.line_strong;
            close = close.hover(move |style| style.text_color(rgba(hover_text)).bg(rgba(hover_bg)));
            close = wire_button(
                close,
                &self.close_focus,
                7.,
                true,
                |this, _, cx| {
                    if this.can_close() {
                        cx.emit(SettingsShellEvent::Close);
                    }
                },
                cx,
            );
        }
        // The dispatch panel already renders its own title and heading; keep
        // only the close control so the page never shows the same title twice.
        if page == SettingsPage::Dispatch {
            return div()
                .flex()
                .items_start()
                .justify_end()
                .w_full()
                .child(close);
        }
        let mut heading = div().flex().flex_col().min_w_0().child(
            div()
                .text_size(px(19.))
                .line_height(px(23.))
                .font_weight(FontWeight(680.))
                .text_color(rgba(palette.workspace.text))
                .child(language_zone_label(page.title(self.locale))),
        );
        heading = heading.child(
            div()
                .mt(px(4.))
                .text_size(px(10.))
                .line_height(px(13.5))
                .text_color(rgba(palette.workspace.muted))
                .child(language_zone_label(page.subtitle(self.locale))),
        );
        div()
            .flex()
            .items_start()
            .justify_between()
            .w_full()
            .child(heading)
            .child(close)
    }

    fn render_page(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let header = self.page_header(page, cx);
        let mut body = div().flex().flex_col().w_full();
        match page {
            SettingsPage::ClaudeCode => {
                let content = self.claude_content(window, cx);
                body = body.child(self.legacy_surface(content));
            }
            SettingsPage::Language => {
                let content = self.render_language_zone(window, cx);
                body = body
                    .child(self.legacy_surface(content))
                    .child(self.render_theme_section(cx));
            }
            SettingsPage::Dispatch => {
                let content = self.dispatch_content(cx);
                body = body.child(self.legacy_surface(content));
            }
            SettingsPage::Diagnostics => {
                let content = self.diagnostics_content(cx);
                body = body.child(content);
            }
            SettingsPage::ContextOptimizer => {
                let content = self.context_optimizer_content(cx);
                body = body.child(content);
            }
        }
        div().flex().flex_col().w_full().child(header).child(body)
    }

    /// Read-only diagnostics page. Each block owns a title and one agreed
    /// status string; the host supplies the strings after its Runtime reads
    /// complete, so this view never invents or caches diagnostic data.
    fn diagnostics_content(&mut self, cx: &mut Context<Self>) -> Div {
        let mut content = div().flex().flex_col().w_full();
        content = content.child(self.diagnostic_section(
            self.locale.text("账号摘要", "Account summary"),
            "accountSummaryResult",
            true,
        ));
        content = content.child(self.diagnostic_section(
            self.locale.text("运行环境", "Runtime environment"),
            "environmentDiagnosticsResult",
            false,
        ));
        content = content.child(self.diagnostic_section(
            self.locale.text("受控浏览器", "Controlled browser"),
            "browserDiagnosticsResult",
            false,
        ));
        content.child(self.retention_cleanup_section(cx))
    }

    fn diagnostic_section(&self, title: &str, key: &str, first: bool) -> Div {
        self.diagnostic_section_frame(title, first)
            .child(self.diagnostic_status_panel(key))
    }

    fn diagnostic_section_frame(&self, title: &str, first: bool) -> Div {
        let palette = *self.theme.effective().palette();
        let mut section = div().flex().flex_col().w_full();
        if first {
            section = section.mt(px(20.));
        } else {
            section = section
                .mt(px(24.))
                .pt(px(18.))
                .border_t_1()
                .border_color(rgba(palette.workspace.line));
        }
        section.child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight(650.))
                .text_color(rgba(palette.workspace.text))
                .child(title.to_owned()),
        )
    }

    fn diagnostic_status_panel(&self, key: &str) -> Div {
        let palette = *self.theme.effective().palette();
        div()
            .mt(px(9.))
            .w_full()
            .min_h(px(44.))
            .p(px(12.))
            .rounded(px(9.))
            .border_1()
            .border_color(rgba(palette.workspace.line))
            .bg(rgba(palette.settings.providers))
            .text_size(px(11.))
            .line_height(px(17.6))
            .text_color(rgba(palette.workspace.text))
            .whitespace_normal()
            .child(self.runtime_diagnostic_text(key))
    }

    fn retention_cleanup_section(&self, cx: &mut Context<Self>) -> Div {
        self.diagnostic_section_frame(self.locale.text("保留清理", "Retention cleanup"), false)
            .child(self.diagnostic_status_panel("retentionCleanupResult"))
            .child(self.retention_cleanup_button(cx))
    }

    fn retention_cleanup_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let palette = *self.theme.effective().palette();
        let can_run = self.can_run_runtime_retention_cleanup();
        let mut button = div()
            .id("settings-run-retention-cleanup")
            .mt(px(10.))
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .px(px(12.))
            .rounded(px(7.))
            .border_1()
            .text_size(px(11.))
            .font_weight(FontWeight(650.))
            .child(self.locale.text("运行保留清理", "Run retention cleanup"));
        if can_run {
            let hover_bg = palette.workspace.panel_muted;
            let hover_text = palette.workspace.text;
            button = button
                .border_color(rgba(palette.workspace.line_strong))
                .bg(rgba(palette.settings.provider_action_background))
                .text_color(rgba(palette.settings.provider_action_text))
                .cursor_pointer()
                .hover(move |style| style.bg(rgba(hover_bg)).text_color(rgba(hover_text)));
            wire_button(
                button,
                &self.diagnostics_cleanup_focus,
                7.,
                self.keyboard_focus,
                |this, _, cx| {
                    this.run_runtime_retention_cleanup(cx);
                    cx.notify();
                },
                cx,
            )
        } else {
            button
                .opacity(0.55)
                .cursor(CursorStyle::OperationNotAllowed)
                .border_color(rgba(palette.workspace.line))
                .bg(rgba(palette.settings.provider_disabled_background))
                .text_color(rgba(palette.settings.provider_disabled_text))
        }
    }

    pub(crate) fn configure_diagnostics_cwd(&mut self, cwd: Option<std::path::PathBuf>) {
        self.diagnostics_cwd = cwd;
    }

    fn refresh_runtime_diagnostics(&mut self, cx: &mut Context<Self>) {
        let (generation, phase) = self.commands.connection();
        if !matches!(
            phase,
            ConnectionPhase::Ready | ConnectionPhase::ReadOnly { .. }
        ) {
            return;
        }
        if self.diagnostics_generation != Some(generation) {
            self.runtime_diagnostics.clear();
            self.diagnostics_generation = Some(generation);
        }
        self.request_runtime_diagnostic("accountSummaryResult", |request_id| {
            Command::GetAccountSummary { request_id }
        });
        if let Some(cwd) = self.diagnostics_cwd.clone() {
            self.request_runtime_diagnostic("environmentDiagnosticsResult", |request_id| {
                Command::GetEnvironmentDiagnostics {
                    request_id,
                    cwd,
                    repair: None,
                }
            });
        }
        self.request_runtime_diagnostic("browserDiagnosticsResult", |request_id| {
            Command::GetBrowserDiagnostics { request_id }
        });
        cx.notify();
    }

    fn request_runtime_diagnostic(
        &mut self,
        key: &'static str,
        command: impl FnOnce(u64) -> Command,
    ) {
        let (generation, _) = self.commands.connection();
        let request_id = NEXT_DIAGNOSTIC_REQUEST.fetch_add(1, Ordering::Relaxed);
        let diagnostic = self.runtime_diagnostics.entry(key).or_default();
        diagnostic.pending = None;
        match self.commands.send_at(generation, command(request_id)) {
            Ok(()) => diagnostic.pending = Some((generation, request_id)),
            Err(error) => diagnostic.result = Some(Err(error.to_string())),
        }
    }

    pub(crate) fn receive_runtime_diagnostic(
        &mut self,
        key: &str,
        request_id: u64,
        reply: &serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let (generation, _) = self.commands.connection();
        let Some(diagnostic) = self.runtime_diagnostics.get_mut(key) else {
            return;
        };
        if diagnostic.pending != Some((generation, request_id)) {
            return;
        }
        diagnostic.pending = None;
        diagnostic.result = Some(if let Some(error) = reply["error"].as_str() {
            Err(if reply["code"] == "STORAGE_READ_ONLY" {
                self.locale
                    .text(
                        "数据库处于只读恢复模式",
                        "The database is in read-only recovery mode",
                    )
                    .into()
            } else {
                error.into()
            })
        } else {
            Ok(reply["result"].clone())
        });
        cx.notify();
    }

    fn runtime_diagnostic_text(&self, key: &str) -> String {
        let locale = self.locale;
        let (_, phase) = self.commands.connection();
        if key == "retentionCleanupResult" && matches!(phase, ConnectionPhase::ReadOnly { .. }) {
            return locale
                .text(
                    "数据库处于只读恢复模式，无法运行保留清理。",
                    "Retention cleanup is unavailable in read-only recovery mode.",
                )
                .into();
        }
        if !matches!(
            phase,
            ConnectionPhase::Ready | ConnectionPhase::ReadOnly { .. }
        ) {
            return locale
                .text("正在等待运行时连接…", "Waiting for the runtime…")
                .into();
        }
        let diagnostic = self.runtime_diagnostics.get(key);
        if diagnostic.is_some_and(|entry| entry.pending.is_some()) {
            return if key == "retentionCleanupResult" {
                locale.text("正在清理…", "Running cleanup…")
            } else {
                locale.text("正在读取…", "Loading…")
            }
            .into();
        }
        let Some(result) = diagnostic.and_then(|entry| entry.result.as_ref()) else {
            return if key == "retentionCleanupResult" {
                locale.text("尚未运行保留清理。", "Retention cleanup has not run yet.")
            } else {
                locale.text("暂无诊断信息。", "No diagnostic information yet.")
            }
            .into();
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => return error.clone(),
        };
        match key {
            "accountSummaryResult" => {
                let mut lines = Vec::new();
                let account_summary = &result["accountSummary"];
                for account in account_summary["accounts"].as_array().into_iter().flatten() {
                    let status = match account["status"].as_str() {
                        Some("active") => locale.text("有效", "Active"),
                        Some("expired") => locale.text("已过期", "Expired"),
                        Some("needs-activation") => locale.text("待激活", "Needs activation"),
                        Some("rate-limited") => locale.text("已达限额", "Rate limited"),
                        _ => locale.text("未知", "Unknown"),
                    };
                    let current =
                        account["id"].is_string() && account["id"] == account_summary["currentId"];
                    let plan = account["plan"]
                        .as_str()
                        .filter(|plan| !plan.is_empty())
                        .unwrap_or_else(|| locale.text("未知套餐", "Unknown plan"));
                    lines.push(if current {
                        format!(
                            "{plan} · {status} · {}",
                            locale.text("当前账号", "Current account")
                        )
                    } else {
                        format!("{plan} · {status}")
                    });
                }
                if lines.is_empty() {
                    lines.push(if result["loadState"] == "corrupt" {
                        locale
                            .text(
                                "无法读取本地账号摘要。",
                                "The local account summary could not be read.",
                            )
                            .into()
                    } else {
                        locale
                            .text("暂无官方账号记录。", "No official account records.")
                            .into()
                    });
                }
                if result["subscriptionSupported"] == false {
                    lines.push(
                        locale
                            .text(
                                "此设备暂不支持订阅启动。",
                                "Subscription launch is unavailable on this device.",
                            )
                            .into(),
                    );
                }
                lines.join(
                    "
",
                )
            }
            "environmentDiagnosticsResult" => {
                let state = match result["state"].as_str() {
                    Some("ready") => locale.text("就绪", "Ready"),
                    Some("missing") => locale.text("目录缺失", "Directory missing"),
                    _ => locale.text("环境不可用", "Environment unavailable"),
                };
                let mut lines = vec![state.to_owned()];
                lines.extend(result["cwd"].as_str().map(str::to_owned));
                if result["git"]["isRepository"] == true {
                    let head = &result["git"]["head"];
                    if let Some(branch) = head["branch"]
                        .as_str()
                        .or_else(|| head["revision"].as_str())
                    {
                        lines.push(format!("Git · {branch}"));
                    }
                }
                lines.extend(result["reason"].as_str().map(str::to_owned));
                lines.join(
                    "
",
                )
            }
            "browserDiagnosticsResult" => {
                let status = &result["status"];
                if status["installed"] == true {
                    let label = locale.text("已安装", "Installed");
                    status["version"]
                        .as_str()
                        .map_or_else(|| label.into(), |version| format!("{label} · {version}"))
                } else {
                    match status["diagnostic"].as_str() {
                        Some("Missing") => locale.text("未安装", "Missing"),
                        Some("NotExecutable") => {
                            locale.text("浏览器不可执行", "Browser is not executable")
                        }
                        _ => locale.text("浏览器不可用", "Browser unavailable"),
                    }
                    .into()
                }
            }
            "retentionCleanupResult" => {
                let actions = result["actions"].as_array().map_or(0, Vec::len);
                let bytes = result["reclaimedBytes"].as_u64().unwrap_or_default();
                if locale == InterfaceLocale::Chinese {
                    format!("清理完成 · 移除 {actions} 项 · 释放 {bytes} 字节")
                } else {
                    format!("Cleanup complete · {actions} removed · {bytes} bytes freed")
                }
            }
            _ => String::new(),
        }
    }

    fn run_runtime_retention_cleanup(&mut self, cx: &mut Context<Self>) {
        if self.can_run_runtime_retention_cleanup() {
            self.request_runtime_diagnostic("retentionCleanupResult", |request_id| {
                Command::RunRetentionCleanup { request_id }
            });
            cx.notify();
        }
    }

    fn can_run_runtime_retention_cleanup(&self) -> bool {
        matches!(self.commands.connection().1, ConnectionPhase::Ready)
            && !self
                .runtime_diagnostics
                .get("retentionCleanupResult")
                .is_some_and(|entry| entry.pending.is_some())
    }

    #[cfg(debug_assertions)]
    pub(crate) fn select_diagnostics_for_probe(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_page(SettingsPage::Diagnostics, window, cx);
    }

    #[cfg(debug_assertions)]
    pub(crate) fn diagnostics_text_for_probe(&self, key: &str) -> String {
        self.runtime_diagnostic_text(key)
    }

    #[cfg(debug_assertions)]
    pub(crate) fn cleanup_for_probe(&mut self, cx: &mut Context<Self>) {
        self.run_runtime_retention_cleanup(cx);
    }

    fn is_dark(&self) -> bool {
        self.theme.effective() == AppTheme::Dark
    }

    /// Older sections still use the frozen light palette. In dark mode the
    /// surrounding surface follows the active palette; the inline cards below
    /// are themed individually where they live in this module.
    fn legacy_surface(&self, content: Div) -> Div {
        if self.is_dark() {
            let palette = self.theme.effective().palette();
            div()
                .mt(px(14.))
                .w_full()
                .rounded(px(12.))
                .border_1()
                .border_color(rgba(palette.workspace.line_strong))
                .bg(rgba(palette.workspace.canvas))
                .text_color(rgba(palette.workspace.text))
                .p(px(18.))
                .child(content)
        } else {
            content
        }
    }

    fn dispatch_content(&mut self, cx: &mut Context<Self>) -> Div {
        let palette = *self.theme.effective().palette();
        let state = self.dispatch_panel.clone();
        let content = div().flex().flex_col().w_full();
        match state {
            None => content.child(
                div()
                    .mt(px(22.))
                    .p(px(14.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(rgba(palette.workspace.line))
                    .bg(rgba(palette.workspace.panel))
                    .text_size(px(10.))
                    .text_color(rgba(palette.workspace.muted))
                    .child(
                        self.locale
                            .text("正在载入派活默认值…", "Loading dispatch defaults…"),
                    ),
            ),
            Some(Ok(panel)) => content.child(panel),
            Some(Err(_)) => content.child(self.dispatch_error_block(cx)),
        }
    }

    fn dispatch_error_block(&self, cx: &mut Context<Self>) -> Div {
        let palette = self.theme.effective().palette();
        let dark = self.is_dark();
        let colors = EditorColors::new(palette, dark);
        let retry_border = if dark {
            palette.workspace.line_strong
        } else {
            0xa34a4433
        };
        let retry_hover = if dark {
            colors.card_hover_bg
        } else {
            0xfff7f6ff
        };
        let retry = wire_button(
            div()
                .id("settings-dispatch-retry")
                .h(px(24.))
                .flex()
                .items_center()
                .px(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgba(retry_border))
                .bg(rgba(colors.panel))
                .text_size(px(10.))
                .font_weight(FontWeight(600.))
                .text_color(rgba(colors.error_text))
                .hover(move |style| style.bg(rgba(retry_hover)))
                .child(RETRY_LABEL),
            &self.dispatch_retry_focus,
            6.,
            true,
            |this, _, cx| this.load_dispatch(cx),
            cx,
        );
        div()
            .mt(px(22.))
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(colors.error_line))
            .bg(rgba(colors.error_bg))
            .text_size(px(11.))
            .line_height(px(17.6))
            .text_color(rgba(colors.error_text))
            .child(div().flex_1().whitespace_normal().child(LOAD_FAILED))
            .child(retry)
    }

    fn render_theme_section(&self, cx: &mut Context<Self>) -> Div {
        let palette = *self.theme.effective().palette();
        let dark = self.theme.effective() == AppTheme::Dark;
        let weak = cx.weak_entity();
        let picker = theme_preference_picker(
            "settings-theme-picker",
            self.theme.preference(),
            &palette,
            self.locale,
            move |preference, _window, cx| {
                let _ = weak.update(cx, |_shell, cx| {
                    cx.emit(SettingsShellEvent::SaveTheme(preference));
                    cx.notify();
                });
            },
        );
        let mut section = div()
            .flex()
            .flex_col()
            .w_full()
            .mt(px(24.))
            .pt(px(18.))
            .border_t_1()
            .border_color(rgba(palette.workspace.line))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight(650.))
                    .text_color(rgba(palette.workspace.text))
                    .child(self.locale.text("主题", "Theme")),
            )
            .child(div().mt(px(10.)).w_full().child(picker));
        if let Some(error) = self
            .theme_error
            .as_deref()
            .filter(|error| !error.is_empty())
        {
            section = section.child(
                div()
                    .mt(px(9.))
                    .text_size(px(11.))
                    .line_height(relative(1.7))
                    .text_color(rgb(if dark { 0xff9b94 } else { DANGER_TEXT }))
                    .whitespace_normal()
                    .child(error.to_owned()),
            );
        }
        section
    }

    fn claude_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let palette = *self.theme.effective().palette();
        let dark = self.is_dark();
        let mut content = div().flex().flex_col().w_full();
        let panel = self
            .assignments
            .panel_localized(&self.providers, self.locale);
        let assignments = self.assignment_view.update(cx, |view, cx| {
            view.render_panel(panel, self.keyboard_focus, &palette, dark, window, cx)
        });
        if self.providers.loading() {
            return content
                .child(
                    div()
                        .mt(px(23.))
                        .p(px(14.))
                        .rounded(px(9.))
                        .border_1()
                        .border_color(rgba(palette.workspace.line))
                        .bg(rgba(palette.workspace.panel))
                        .text_size(px(10.))
                        .text_color(rgba(palette.workspace.muted))
                        .child(
                            self.locale
                                .text("正在载入供应商配置…", "Loading the provider configuration…"),
                        ),
                )
                .child(assignments);
        }

        if let Some((title, body, error)) = self.providers.notice() {
            let title = title.to_owned();
            let body = body.to_owned();
            content = content.child(self.notice_block(&title, &body, error, cx));
        }

        content = content.child(self.provider_toolbar(cx));

        if let Some(error) = self.providers.load_error().map(str::to_owned) {
            content = content.child(self.load_error_block(&error, cx));
        }

        content = content.child(self.provider_cards(window, cx));
        // Reuse the verified provider-switch note once, below the card list;
        // repeating it in every card would duplicate the same global warning.
        let app_name = self.locale.text("码头", "Matou");
        #[cfg(debug_assertions)]
        LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
            if let Some(probe) = probe {
                let impact = matou_native_desktop::provider_context_editor::provider_switch_impact(
                    self.locale,
                    app_name,
                );
                probe.labels.extend([impact.title, impact.body.to_owned()]);
            }
        });
        content = content.child(provider_switch_impact_row(self.locale, app_name, dark));
        content = content.child(assignments);
        self.sync_compaction(cx);
        content.child(self.render_compaction(window, cx))
    }

    fn notice_block(&self, title: &str, body: &str, error: bool, cx: &mut Context<Self>) -> Div {
        let palette = *self.theme.effective().palette();
        let dark = self.is_dark();
        let colors = EditorColors::new(&palette, dark);
        let accent = if error {
            colors.error_text
        } else {
            colors.link
        };
        let icon_bg = if error {
            if dark { 0x5a2a28 } else { 0xbc5550 }
        } else {
            colors.primary
        };
        let dismiss_hover = if dark {
            palette.workspace.line
        } else {
            0x0000000a
        };
        let mut dismiss = div()
            .id("settings-notice-dismiss")
            .size(px(27.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .text_size(px(16.))
            .line_height(relative(1.))
            .text_color(rgba(accent))
            .hover(move |style| style.bg(rgba(dismiss_hover)))
            .child("×");
        dismiss = wire_button(
            dismiss,
            &self.notice_close_focus,
            7.,
            true,
            |this, _, cx| {
                this.providers.dismiss_notice();
                cx.notify();
            },
            cx,
        );
        let retry_id = self.providers.retry_switch_id().map(str::to_owned);
        let mut text = div().flex_1().min_w_0().child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight(600.))
                .text_color(rgba(if error {
                    colors.error_text
                } else {
                    palette.workspace.text
                }))
                .child(title.to_owned()),
        );
        if !body.is_empty() {
            text = text.child(
                div()
                    .mt(px(6.))
                    .text_size(px(12.))
                    .line_height(px(19.2))
                    .text_color(rgba(if error {
                        colors.error_text
                    } else {
                        colors.notice_body
                    }))
                    .whitespace_normal()
                    .child(body.to_owned()),
            );
        }
        if error && let Some(id) = retry_id {
            text = text.child(link_button(
                "settings-notice-retry",
                self.locale.text("重试", "Retry"),
                &self.notice_retry_focus,
                true,
                FocusRingStyle::Shell,
                &colors,
                cx,
                move |this, _, cx| {
                    this.providers.switch(&id);
                    cx.notify();
                },
            ));
        }
        let border = if error {
            colors.error_line
        } else if dark {
            palette.workspace.line
        } else {
            NOTICE_LINE
        };
        let background = if error {
            colors.error_bg
        } else if dark {
            palette.settings.language_selected
        } else {
            NOTICE_BG
        };
        div()
            .flex()
            .items_start()
            .gap(px(12.))
            .p(px(17.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(border))
            .bg(rgba(background))
            .child(
                div()
                    .size(px(27.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgba(icon_bg))
                    .text_size(px(13.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(0xffffff))
                    .child(if error { "!" } else { "✓" }),
            )
            .child(text)
            .child(dismiss)
    }

    fn provider_toolbar(&self, cx: &mut Context<Self>) -> Div {
        let palette = *self.theme.effective().palette();
        let dark = self.is_dark();
        let add_bg = if dark {
            0x2f79dfff
        } else {
            palette.workspace.accent
        };
        let add_border = if dark {
            palette.workspace.accent_line
        } else {
            0x2670c52e
        };
        let add_hover = if dark {
            0x2274dfff
        } else {
            palette.settings.language_primary_hover
        };
        let busy = self.busy();
        let count = self.providers.providers().len();
        let mut add = div()
            .id("settings-add-provider")
            .h(px(34.))
            .flex()
            .items_center()
            .justify_center()
            .px(px(11.))
            .rounded(px(7.))
            .border_1()
            .border_color(rgba(add_border))
            .text_size(px(12.))
            .font_weight(FontWeight(650.))
            .text_color(rgb(0xffffff))
            .bg(rgba(add_bg))
            .shadow(vec![shadow(0x357bcc, 0.13, 4., 11.)])
            .child(self.locale.text("＋ 新增供应商", "+ New provider"));
        if busy {
            add = add.opacity(0.55).cursor(CursorStyle::OperationNotAllowed);
        } else {
            add = add.hover(move |style| style.bg(rgba(add_hover)));
            add = wire_button(
                add,
                &self.new_provider_focus,
                7.,
                true,
                |this, window, cx| this.open_new_editor(window, cx),
                cx,
            );
        }
        div()
            .flex()
            .items_center()
            .justify_between()
            .mt(px(19.))
            .mb(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(650.))
                            .text_color(rgba(palette.workspace.text))
                            .child(self.locale.text("服务供应商", "Providers")),
                    )
                    .child(
                        div()
                            .text_size(px(9.))
                            .font_weight(FontWeight(500.))
                            .text_color(rgba(palette.workspace.muted))
                            .child(match self.locale {
                                InterfaceLocale::Chinese => format!("{count} 个配置"),
                                InterfaceLocale::English if count == 1 => "1 configuration".into(),
                                InterfaceLocale::English => format!("{count} configurations"),
                            }),
                    ),
            )
            .child(add)
    }

    fn load_error_block(&self, error: &str, cx: &mut Context<Self>) -> Div {
        let palette = *self.theme.effective().palette();
        let dark = self.is_dark();
        let colors = EditorColors::new(&palette, dark);
        let retry_bg = if dark { colors.panel } else { 0xffffffff };
        let retry_border = if dark {
            palette.workspace.line_strong
        } else {
            0xa34a4433
        };
        let retry_hover = if dark {
            colors.card_hover_bg
        } else {
            0xfff7f6ff
        };
        let retry = wire_button(
            div()
                .id("settings-load-retry")
                .h(px(24.))
                .flex()
                .items_center()
                .px(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgba(retry_border))
                .bg(rgba(retry_bg))
                .text_size(px(10.))
                .font_weight(FontWeight(600.))
                .text_color(rgba(colors.error_text))
                .hover(move |style| style.bg(rgba(retry_hover)))
                .child(self.locale.text("重试", "Retry")),
            &self.load_retry_focus,
            6.,
            true,
            |this, _, cx| {
                this.providers.load();
                cx.notify();
            },
            cx,
        );
        div()
            .mt(px(8.))
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(colors.error_line))
            .bg(rgba(colors.error_bg))
            .text_size(px(11.))
            .line_height(px(17.6))
            .text_color(rgba(colors.error_text))
            .child(div().flex_1().whitespace_normal().child(error.to_owned()))
            .child(retry)
    }

    fn provider_cards(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let providers = self.providers.providers().to_vec();
        let active = self.providers.active_id().map(str::to_owned);
        let switching = self.providers.switching().map(str::to_owned);
        let busy = self.busy();
        let viewport_width = f32::from(window.viewport_size().width);
        let shell_width = self.shell_width(window);
        let pad_x = if viewport_width <= 680. {
            14.
        } else if viewport_width <= 940. {
            18.
        } else {
            28.
        };
        let container_width = (shell_width - pad_x * 2.).max(0.);
        let palette = self.theme.effective().palette();
        let mut list = div()
            .id("provider-list")
            .flex()
            .flex_col()
            .w_full()
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(palette.workspace.line))
            .bg(rgba(palette.workspace.panel));
        let card_count = providers.len();
        for (index, provider) in providers.into_iter().enumerate() {
            let id = provider["id"].as_str().unwrap_or_default().to_owned();
            let controls = self
                .card_controls
                .entry(id.clone())
                .or_insert_with(|| std::array::from_fn(|_| cx.focus_handle().tab_stop(true)))
                .clone();
            let current = active.as_deref() == Some(id.as_str());
            let is_last = index + 1 == card_count;
            let card = if provider["kind"] == "subscription" && provider["subscription"].is_object()
            {
                let details = self.providers.subscription_details(&id).cloned();
                let refreshing = self.providers.refreshing_subscription(&id);
                self.subscription_card(
                    &provider,
                    details.as_ref(),
                    current,
                    is_last,
                    viewport_width,
                    switching.as_deref(),
                    busy,
                    refreshing,
                    &controls,
                    cx,
                )
            } else {
                self.api_card(
                    &provider,
                    current,
                    is_last,
                    switching.as_deref(),
                    busy,
                    &controls,
                    container_width,
                    viewport_width,
                    cx,
                )
            };
            list = list.child(card);
        }
        list
    }

    fn provider_actions(
        &self,
        provider: &Value,
        current: bool,
        switching: Option<&str>,
        busy: bool,
        controls: &[FocusHandle; 3],
        relogin: bool,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, Stateful<Div>) {
        let palette = self.theme.effective().palette();
        let edit_provider = provider.clone();
        let edit_opener = controls[0].clone();
        let mut edit = div()
            .id(SharedString::from(format!(
                "provider-edit-{}",
                provider["id"].as_str().unwrap_or_default()
            )))
            .h(px(34.))
            .flex()
            .items_center()
            .justify_center()
            .px(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(palette.workspace.line_strong))
            .text_size(px(12.))
            .font_weight(FontWeight(650.))
            .text_color(rgba(palette.settings.provider_action_text))
            .bg(rgba(palette.settings.provider_action_background))
            .child(self.locale.text("编辑", "Edit"));
        if busy {
            edit = edit.opacity(0.55).cursor(CursorStyle::OperationNotAllowed);
        } else {
            edit = edit.hover(|style| style.bg(rgba(palette.workspace.panel)));
            edit = wire_button(
                edit,
                &controls[0],
                6.,
                true,
                move |this, window, cx| {
                    this.open_edit_editor(edit_provider.clone(), edit_opener.clone(), window, cx)
                },
                cx,
            );
        }

        let switch_id = provider["id"].as_str().unwrap_or_default().to_owned();
        let relogin_provider = provider.clone();
        let relogin_opener = controls[1].clone();
        let switching_this = switching == Some(switch_id.as_str());
        let mut switch = div()
            .id(SharedString::from(format!(
                "provider-switch-{}",
                provider["id"].as_str().unwrap_or_default()
            )))
            .h(px(34.))
            .flex()
            .items_center()
            .justify_center()
            .px(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(if (current && !relogin) || busy {
                rgba(0x00000000)
            } else {
                rgba(palette.workspace.accent_line)
            })
            .text_size(px(12.))
            .font_weight(FontWeight(650.))
            .child(if switching_this {
                self.locale.text("切换中…", "Switching…")
            } else if relogin {
                self.locale.text("重新登录", "Sign in again")
            } else if current {
                self.locale.text("默认使用", "Default")
            } else {
                self.locale.text("切换", "Switch")
            });
        if (current && !relogin) || busy {
            switch = switch
                .text_color(rgb(FAINT))
                .bg(rgba(palette.settings.provider_disabled_background))
                .cursor(CursorStyle::OperationNotAllowed);
        } else {
            switch = switch
                .text_color(rgba(palette.workspace.accent))
                .bg(rgba(palette.workspace.accent_soft))
                .hover(|style| style.bg(rgba(0x347fd622)));
            switch = wire_button(
                switch,
                &controls[1],
                6.,
                true,
                move |this, window, cx| {
                    if relogin {
                        this.open_subscription_relogin(
                            relogin_provider.clone(),
                            relogin_opener.clone(),
                            window,
                            cx,
                        );
                    } else {
                        this.providers.switch(&switch_id);
                        cx.notify();
                    }
                },
                cx,
            );
        }

        (edit, switch)
    }

    fn api_card(
        &mut self,
        provider: &Value,
        current: bool,
        is_last: bool,
        switching: Option<&str>,
        busy: bool,
        controls: &[FocusHandle; 3],
        container_width: f32,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *self.theme.effective().palette();
        let name = provider["name"].as_str().unwrap_or_default();
        let endpoint = short_endpoint(provider["endpoint"].as_str().unwrap_or_default());
        let model = provider["model"].as_str().unwrap_or(CLI_DEFAULT_MODEL);
        let metadata = provider["modelMetadata"]
            .as_array()
            .and_then(|models| models.iter().find(|row| row["id"] == model));
        let model_name = metadata
            .and_then(|row| row["displayName"].as_str())
            .unwrap_or_else(|| model_label(model));
        let model_sub = format!(
            "{}{}{}",
            metadata
                .and_then(|row| row["displayName"].as_str())
                .map(|_display| format!("{model} · "))
                .unwrap_or_default(),
            self.locale.text("默认模型", "Default model"),
            if provider["hasApiKey"] == true {
                self.locale.text(" · Key 已配置", " · Key configured")
            } else {
                ""
            }
        );
        let tone = provider_tone(name);
        let logo_bg = match tone {
            1 => linear_gradient(
                145.,
                linear_color_stop(rgb(0x198875), 0.),
                linear_color_stop(rgb(0x087363), 1.),
            ),
            2 => linear_gradient(
                145.,
                linear_color_stop(rgb(0xc98559), 0.),
                linear_color_stop(rgb(0xae6740), 1.),
            ),
            _ => linear_gradient(
                145.,
                linear_color_stop(rgb(0x4a96e5), 0.),
                linear_color_stop(rgb(0x7169df), 1.),
            ),
        };

        let (edit, switch) =
            self.provider_actions(provider, current, switching, busy, controls, false, cx);
        let card = self
            .provider_list_row(provider, current, is_last)
            .flex_col()
            .min_h(px(62.))
            .px(px(12.))
            .py(px(10.));
        let mut header = div().flex().items_center().w_full().gap(px(11.));
        header = header.child(
            div()
                .size(px(32.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .text_size(px(12.))
                .font_weight(FontWeight(750.))
                .text_color(rgb(0xffffff))
                .bg(logo_bg)
                .child(provider_initial(name)),
        );
        let mut name_column = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .min_w(px(150.))
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(13.))
                    .line_height(px(17.55))
                    .font_weight(FontWeight(650.))
                    .text_color(rgba(palette.settings.provider_text))
                    .whitespace_normal()
                    .child(name.to_owned())
                    .when(current, |row| {
                        row.child(
                            div()
                                .px(px(5.))
                                .py(px(2.))
                                .rounded_full()
                                .text_size(px(8.))
                                .font_weight(FontWeight(500.))
                                .text_color(rgba(palette.workspace.accent))
                                .bg(rgba(palette.workspace.accent_soft))
                                .child(self.locale.text("使用中", "In use")),
                        )
                    })
                    .child(self.render_provider_window_badge(provider)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(px(15.4))
                    .text_color(rgba(palette.workspace.muted))
                    .whitespace_normal()
                    .child(endpoint),
            );
        name_column.style().flex_grow = Some(1.2);
        let model_block = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .min_w(px(130.))
            .flex_1()
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(16.8))
                    .font_weight(FontWeight(650.))
                    .text_color(rgba(palette.settings.provider_text))
                    .whitespace_normal()
                    .child(model_name.to_owned()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(px(15.4))
                    .text_color(rgba(palette.workspace.muted))
                    .whitespace_normal()
                    .child(model_sub),
            );
        // Base media query hides the model column below 940px viewport; the
        // provider-settings container query restores it by stacking when the
        // card container itself is 640px or narrower.
        let stacked = container_width <= 640.;
        let hide_model = viewport_width <= 940. && !stacked;
        if stacked {
            header = header.child(name_column.flex_1().min_w_0().child(model_block));
        } else {
            header = header.child(name_column);
            if !hide_model {
                header = header.child(model_block);
            }
        }
        header = header.child(
            div()
                .flex()
                .flex_none()
                .gap(px(5.))
                .child(edit)
                .child(switch),
        );
        card.child(header).into_any_element()
    }

    fn provider_list_row(&self, provider: &Value, current: bool, is_last: bool) -> Stateful<Div> {
        let palette = self.theme.effective().palette();
        div()
            .id(SharedString::from(format!(
                "provider-card-{}",
                provider["id"].as_str().unwrap_or_default()
            )))
            .flex()
            .w_full()
            .when(current, |row| {
                row.rounded(px(10.))
                    .border_1()
                    .border_color(rgb(ACTIVE_PROVIDER_BOTTOM_EDGE_RGB))
                    .bg(rgba(palette.settings.provider_selected))
            })
            .when(!current && !is_last, |row| {
                row.border_b_1().border_color(rgba(palette.workspace.line))
            })
    }

    fn subscription_card(
        &self,
        provider: &Value,
        details: Option<&Value>,
        current: bool,
        is_last: bool,
        viewport_width: f32,
        switching: Option<&str>,
        busy: bool,
        refreshing: bool,
        controls: &[FocusHandle; 3],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *self.theme.effective().palette();
        let subscription = &provider["subscription"];
        let account = if subscription["account"].is_object() {
            &subscription["account"]
        } else {
            details
                .and_then(|details| details.get("account"))
                .unwrap_or(&subscription["account"])
        };
        let status = account["status"].as_str().unwrap_or("ready");
        let kind = subscription["provider"].as_str().unwrap_or("codex");
        let quota = quota_text(kind, details, self.locale);
        let status_label = match status {
            "needs-sign-in" => self.locale.text("需要重新登录", "Sign in again"),
            "account-unavailable" => self.locale.text("账号不可用", "Account unavailable"),
            "router-not-running" => self
                .locale
                .text("模型路由未运行", "Model router is not running"),
            _ => "",
        };
        let account_line = [account["email"].as_str(), account["plan"].as_str()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let opus_id = provider["modelMapping"]["opus"].as_str();
        let models = subscription["modelMetadata"].as_array();
        let opus = opus_id
            .and_then(|id| models.and_then(|models| models.iter().find(|row| row["id"] == id)));
        let model_name = opus
            .and_then(|row| row["displayName"].as_str())
            .or(opus_id)
            .unwrap_or_else(|| {
                model_label(provider["model"].as_str().unwrap_or(CLI_DEFAULT_MODEL))
            });
        let missing_opus =
            opus_id.is_some() && models.is_some_and(|models| !models.is_empty()) && opus.is_none();
        let initial = if subscription["provider"] == "antigravity" {
            "A"
        } else {
            "C"
        };
        let tone = if subscription["provider"] == "antigravity" {
            linear_gradient(
                145.,
                linear_color_stop(rgb(0x4a96e5), 0.),
                linear_color_stop(rgb(0x7169df), 1.),
            )
        } else {
            linear_gradient(
                145.,
                linear_color_stop(rgb(0x198875), 0.),
                linear_color_stop(rgb(0x087363), 1.),
            )
        };
        let mut name_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .text_size(px(13.))
            .line_height(px(17.55))
            .font_weight(FontWeight(650.))
            .text_color(rgba(palette.settings.provider_text))
            .whitespace_normal()
            .child(provider["name"].as_str().unwrap_or_default().to_owned());
        if current {
            name_row = name_row.child(
                div()
                    .px(px(5.))
                    .py(px(2.))
                    .rounded_full()
                    .text_size(px(8.))
                    .font_weight(FontWeight(500.))
                    .text_color(rgba(palette.workspace.accent))
                    .bg(rgba(palette.workspace.accent_soft))
                    .child(self.locale.text("使用中", "In use")),
            );
        }
        if !status_label.is_empty() {
            name_row = name_row.child(
                div()
                    .px(px(5.))
                    .py(px(2.))
                    .rounded_full()
                    .text_size(px(8.))
                    .line_height(px(10.))
                    .font_weight(FontWeight(500.))
                    .text_color(rgb(DANGER_TEXT))
                    .bg(rgb(DANGER_BG))
                    .whitespace_nowrap()
                    .child(status_label),
            );
        }
        name_row = name_row.child(self.render_provider_window_badge(provider));
        let mut details = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .min_w_0()
            .flex_1()
            .child(name_row);
        if !account_line.is_empty() {
            details = details.child(
                div()
                    .text_size(px(11.))
                    .line_height(px(15.4))
                    .text_color(rgba(palette.workspace.muted))
                    .whitespace_normal()
                    .child(account_line),
            );
        }
        if !quota.is_empty() {
            details = details.child(mark_provider(
                div()
                    .text_size(px(10.))
                    .line_height(px(14.))
                    .text_color(rgba(palette.workspace.muted))
                    .whitespace_normal()
                    .child(quota.clone()),
                "quota",
                quota,
                true,
            ));
        }
        if status == "account-unavailable"
            && let Some(reason) = account["reason"].as_str()
        {
            details = details.child(
                div()
                    .text_size(px(10.))
                    .line_height(px(14.))
                    .text_color(rgba(palette.workspace.muted))
                    .whitespace_normal()
                    .child(reason.to_owned()),
            );
        }
        let mut card = self
            .provider_list_row(provider, current, is_last)
            .items_start()
            .min_h(px(if status == "account-unavailable" {
                120.
            } else {
                80.
            }))
            .gap(px(11.))
            .px(px(12.))
            .py(px(10.))
            .child(
                div()
                    .mt(px(1.))
                    .size(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .text_size(px(12.))
                    .font_weight(FontWeight(750.))
                    .text_color(rgb(0xffffff))
                    .bg(tone)
                    .child(initial),
            )
            .child(details);
        if viewport_width > 940. {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .min_w(px(130.))
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.8))
                            .font_weight(FontWeight(650.))
                            .text_color(rgba(palette.settings.provider_text))
                            .whitespace_normal()
                            .child(model_name.to_owned()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(15.4))
                            .text_color(rgba(palette.workspace.muted))
                            .whitespace_normal()
                            .child(format!(
                                "默认模型{}",
                                if missing_opus { " · 不可用" } else { "" }
                            )),
                    ),
            );
        }
        let (edit, switch) = self.provider_actions(
            provider,
            current,
            switching,
            busy,
            controls,
            status == "needs-sign-in",
            cx,
        );
        let refresh_disabled = busy
            || refreshing
            || switching.is_some()
            || matches!(
                status,
                "needs-sign-in" | "account-unavailable" | "router-not-running"
            );
        let refresh_label = if refreshing {
            self.locale.text("刷新中…", "Refreshing…")
        } else {
            self.locale.text("刷新模型", "Refresh models")
        };
        let mut refresh = div()
            .id("subscription-refresh-models")
            .h(px(34.))
            .flex()
            .items_center()
            .justify_center()
            .px(px(8.))
            .rounded(px(6.))
            .text_size(px(12.))
            .font_weight(FontWeight(600.))
            .text_color(rgba(palette.workspace.muted))
            .child(refresh_label);
        if refresh_disabled {
            refresh = refresh
                .opacity(0.55)
                .cursor(CursorStyle::OperationNotAllowed);
        } else {
            let provider_id = provider["id"].as_str().unwrap_or_default().to_owned();
            refresh = refresh
                .cursor_pointer()
                .hover(|style| style.bg(rgba(palette.workspace.panel)));
            refresh = wire_button(
                refresh,
                &controls[2],
                6.,
                true,
                move |this, _, cx| {
                    this.providers.dismiss_notice();
                    let _ = this.providers.refresh_subscription_models(&provider_id);
                    cx.notify();
                },
                cx,
            );
        }
        card.child(
            div()
                .flex()
                .flex_none()
                .gap(px(5.))
                .child(mark_provider(
                    refresh,
                    "refresh-models",
                    refresh_label,
                    !refresh_disabled,
                ))
                .child(edit)
                .child(switch),
        )
        .into_any_element()
    }

    fn shell_width(&self, window: &Window) -> f32 {
        (f32::from(window.viewport_size().width) - WORKSPACE_SIDEBAR_WIDTH).max(0.)
    }
}

/// Frozen 15×15 globe glyph for the language nav entry. No SVG asset exists in
/// this repository, so the same circle/meridian/equator path is painted
/// directly at the requested size.
fn globe_icon(size: f32, color: u32) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = size / 2. - 0.9;
            let mut builder = PathBuilder::stroke(px(1.5));
            builder.move_to(point(center.x - px(radius), center.y));
            builder.arc_to(
                point(px(radius), px(radius)),
                px(0.),
                false,
                true,
                point(center.x + px(radius), center.y),
            );
            builder.arc_to(
                point(px(radius), px(radius)),
                px(0.),
                false,
                true,
                point(center.x - px(radius), center.y),
            );
            builder.move_to(point(center.x, center.y - px(radius)));
            builder.cubic_bezier_to(
                point(center.x, center.y + px(radius)),
                point(center.x + px(radius * 0.55), center.y - px(radius * 0.55)),
                point(center.x + px(radius * 0.55), center.y + px(radius * 0.55)),
            );
            builder.cubic_bezier_to(
                point(center.x, center.y - px(radius)),
                point(center.x - px(radius * 0.55), center.y + px(radius * 0.55)),
                point(center.x - px(radius * 0.55), center.y - px(radius * 0.55)),
            );
            builder.move_to(point(center.x - px(radius), center.y));
            builder.line_to(point(center.x + px(radius), center.y));
            if let Ok(path) = builder.build() {
                window.paint_path(path, rgb(color));
            }
        },
    )
    .flex_none()
    .size(px(size))
    .into_any_element()
}

impl SettingsShell {
    fn editor_request_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_busy() {
            return;
        }
        if let Some(editor) = &mut self.editor
            && self
                .providers
                .deletable_provider(editor.draft.id.as_deref())
                .is_some()
        {
            editor.ask_confirmation(ConfirmKind::Delete, window, cx);
        }
    }

    fn editor_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_busy() {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        match editor.confirm {
            Some(ConfirmKind::Close) => self.close_editor(window, cx),
            Some(ConfirmKind::Back) => self.editor_back_to_pick(window, cx),
            Some(ConfirmKind::Delete) => {
                editor.error = None;
                if !editor
                    .draft
                    .id
                    .as_deref()
                    .is_some_and(|id| self.providers.delete(id))
                {
                    self.apply_update(ProviderUpdate::DeleteFailed, window, cx);
                }
                cx.notify();
            }
            None => {}
        }
    }

    fn editor_keep_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.providers.deleting() {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.confirm = None;
        if let Some((focus, offset)) = editor.confirm_return.take() {
            editor.body_scroll.set_offset(offset);
            if let Some(focus) = focus {
                window.focus(&focus);
            }
        }
        cx.notify();
    }

    fn cycle_modal_focus(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_busy() {
            return;
        }
        let Some(editor) = &self.editor else {
            return;
        };
        let mut order: Vec<FocusHandle> = Vec::new();
        match editor.step {
            EditorStep::Pick => {
                order.push(editor.focus.close.clone());
                order.push(editor.query.read(cx).focus.clone());
                let query = editor.query.read(cx).text.clone();
                let query_lower = query.trim().to_lowercase();
                for (index, (kind, title, description)) in [
                    (
                        "codex",
                        self.locale.text("ChatGPT 账号", "ChatGPT account"),
                        self.locale.text(
                            "使用你的 ChatGPT 订阅额度，不需要 API Key",
                            "Use your ChatGPT subscription. No API key needed.",
                        ),
                    ),
                    (
                        "antigravity",
                        self.locale.text("Antigravity 账号", "Antigravity account"),
                        self.locale.text(
                            "使用 Google AI 订阅中的 Claude 与 Gemini 模型",
                            "Use Claude and Gemini models from your Google AI subscription.",
                        ),
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    if query_lower.is_empty()
                        || format!("{title} {description} {kind}")
                            .to_lowercase()
                            .contains(&query_lower)
                    {
                        order.push(editor.focus.subscription_choices[index].clone());
                    }
                }
                for (index, preset) in PRESETS.iter().enumerate() {
                    if preset_matches(preset, &query) {
                        order.push(editor.focus.presets[index].clone());
                    }
                }
                order.push(editor.focus.custom.clone());
                order.push(editor.focus.cancel.clone());
                if editor.selection.is_some() {
                    order.push(editor.focus.next.clone());
                }
            }
            EditorStep::Subscription => {
                order.push(editor.focus.close.clone());
                let phase = self.providers.subscription_login().phase;
                if phase == "risk" {
                    order.push(editor.focus.subscription_risk.clone());
                    order.push(editor.focus.subscription_cancel.clone());
                    order.push(editor.focus.subscription_primary.clone());
                } else if phase == "waiting" {
                    order.push(editor.focus.subscription_reopen.clone());
                    if self.providers.subscription_login().system_browser_available {
                        order.push(editor.focus.subscription_system_browser.clone());
                    }
                    order.push(editor.focus.subscription_cancel.clone());
                } else if phase == "error" {
                    order.push(editor.focus.subscription_cancel.clone());
                    order.push(editor.focus.subscription_primary.clone());
                } else {
                    order.push(editor.focus.subscription_cancel.clone());
                    order.push(editor.focus.subscription_primary.clone());
                }
            }
            EditorStep::Form => {
                if editor.draft.id.is_none() && editor.subscription.is_none() {
                    order.push(editor.focus.back.clone());
                }
                order.push(editor.focus.close.clone());
                if editor.draft.id.is_none() && editor.subscription.is_none() {
                    order.push(editor.focus.change.clone());
                }
                let work = editor.working_draft(cx);
                let subscription = work.subscription_kind.is_some();
                if !subscription {
                    if provider_preset_for_endpoint(work.is_codex(), &work.original_endpoint)
                        .is_some()
                        || (!work.id.is_some()
                            && matches!(editor.selection, Some(PickChoice::Preset(_))))
                    {
                        order.push(editor.focus.key_link.clone());
                    }
                    order.push(editor.focus.key.clone());
                    order.push(editor.focus.reveal.clone());
                    order.push(editor.focus.endpoint.clone());
                    if !self.providers.fetching()
                        && self.commands.connection().1 == ConnectionPhase::Ready
                    {
                        order.push(editor.focus.fetch.clone());
                    }
                    order.push(editor.focus.model.clone());
                    if editor.model_chooser {
                        order.push(editor.focus.model_search.clone());
                    } else if editor.draft.fetched_metadata.is_some()
                        && editor.fetch_error.is_none()
                        && !self.providers.fetching()
                    {
                        order.push(editor.focus.chooser.clone());
                    }
                    if work.is_codex() {
                        order.push(editor.focus.efforts[0].clone());
                    }
                }
                order.push(editor.focus.name.clone());
                if !work.is_codex() && (!subscription || !work.available_models().is_empty()) {
                    for index in 0..ROLES.len() {
                        if editor.manual_mappings[index] {
                            order.push(editor.focus.mappings[index].clone());
                            order.push(editor.focus.manual_links[index].clone());
                        } else {
                            order.push(editor.focus.mapping_choices[index].clone());
                        }
                        order.push(editor.focus.efforts[index].clone());
                    }
                    order.extend(editor.focus.context_modes.iter().cloned());
                }
                if editor.confirm.is_some() {
                    order.push(editor.focus.keep_editing.clone());
                    order.push(editor.focus.confirm_action.clone());
                }
                if self
                    .providers
                    .deletable_provider(editor.draft.id.as_deref())
                    .is_some()
                {
                    order.push(editor.focus.delete.clone());
                }
                if provider_editor_can_save(&work) {
                    order.push(editor.focus.save.clone());
                    order.push(editor.focus.save_switch.clone());
                }
            }
        }
        if order.is_empty() {
            return;
        }
        let current = order
            .iter()
            .position(|focus| focus.is_focused(window))
            .unwrap_or(if reverse { 0 } else { order.len() - 1 });
        let next = if reverse {
            (current + order.len() - 1) % order.len()
        } else {
            (current + 1) % order.len()
        };
        self.keyboard_focus = true;
        window.focus(&order[next]);
        cx.notify();
    }

    fn render_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = *self.theme.effective().palette();
        let colors = EditorColors::new(&palette, self.is_dark());
        let Some(editor) = self.editor.as_ref() else {
            return None;
        };
        let viewport = window.viewport_size();
        let viewport_height = f32::from(viewport.height);
        // The real baseline centers the dialog in the right workspace. Use
        // that area for the `min(520px, 100%)` width so it also fits the
        // 720px minimum window instead of overflowing into the sidebar.
        let panel_width = 520.0_f32.min((self.shell_width(window) - 44.).max(0.));
        let subscription_step = editor.step == EditorStep::Subscription;
        let login = self.providers.subscription_login();
        let login_phase = if subscription_step { login.phase } else { "" };
        let login_busy = subscription_step && login.busy;
        let connected = self.subscription_connected();
        let busy = self.editor_busy() || login_busy;
        // The frozen subscription dialog keeps one height across risk, waiting
        // and the four-tier form so the modal does not jump between phases.
        let panel_height = if subscription_step || editor.subscription.is_some() {
            620.0_f32.min((viewport_height - 44.).max(0.))
        } else {
            820.0_f32.min((viewport_height - 44.).max(0.))
        };
        let editing = editor.draft.id.is_some();
        let pick = editor.step == EditorStep::Pick;
        let app = if editor.draft.is_codex() {
            "Codex"
        } else {
            "Claude Code"
        };
        let title = if pick {
            self.locale.text("添加供应商", "Add provider").to_owned()
        } else if subscription_step {
            if login_phase == "risk" {
                self.locale
                    .text(
                        "使用 Antigravity 账号有封号风险",
                        "Using an Antigravity account may get it suspended",
                    )
                    .to_owned()
            } else if editor
                .subscription
                .as_ref()
                .is_some_and(|flow| flow.existing)
            {
                self.locale.text("重新登录", "Sign in again").to_owned()
            } else {
                self.locale.text("添加供应商", "Add provider").to_owned()
            }
        } else if editor.subscription.is_some() {
            if editing {
                self.locale.text("编辑供应商", "Edit provider").to_owned()
            } else {
                self.locale.text("添加供应商", "Add provider").to_owned()
            }
        } else if editing {
            self.locale.text("编辑供应商", "Edit provider").to_owned()
        } else {
            match editor.selection {
                Some(PickChoice::Preset(id)) => format!(
                    "{} {}",
                    self.locale.text("配置", "Configure"),
                    preset_by_id(id).map_or(
                        self.locale.text("自定义服务", "custom service"),
                        |preset| preset.name
                    )
                ),
                _ => self
                    .locale
                    .text("配置 自定义服务", "Configure custom service")
                    .to_owned(),
            }
        };

        let mut back = div()
            .id("provider-setup-back")
            .size(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .text_size(px(22.))
            .line_height(relative(1.))
            .text_color(rgba(colors.header_icon))
            .child("←");
        if busy {
            back = back.opacity(0.55);
        } else {
            back = back.hover(move |style| style.bg(rgba(colors.header_icon_hover)));
            back = wire_dialog_button(
                back,
                &editor.focus.back,
                7.,
                self.keyboard_focus,
                |this, window, cx| this.editor_go_back(window, cx),
                cx,
            );
        }
        let mut close = div()
            .id("provider-setup-close")
            .size(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .text_size(px(24.))
            .line_height(relative(1.))
            .text_color(rgba(colors.header_icon))
            .child("×");
        let close_locked = busy && !(subscription_step && login_phase == "waiting");
        if close_locked {
            close = close.opacity(0.55);
        } else {
            close = close.hover(move |style| style.bg(rgba(colors.header_icon_hover)));
            close = wire_dialog_button(
                close,
                &editor.focus.close,
                7.,
                self.keyboard_focus,
                |this, window, cx| this.editor_request_close(window, cx),
                cx,
            );
        }
        let mut header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(26.))
            .pt(px(24.))
            .pb(px(20.))
            .border_b_1()
            .border_color(rgba(colors.header_line));
        if !pick && !editing && editor.subscription.is_none() {
            header = header.child(back);
        }
        let subtitle = format!("{} {app}", self.locale.text("用于", "For"));
        header = header
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(mark_provider(
                        div()
                            .text_size(px(21.))
                            .line_height(px(28.35))
                            .font_weight(FontWeight(650.))
                            .text_color(rgba(colors.label))
                            .child(title.clone()),
                        "title",
                        title,
                        true,
                    ))
                    .child(mark_provider(
                        div()
                            .mt(px(5.))
                            .text_size(px(12.))
                            .line_height(px(16.8))
                            .text_color(rgba(colors.subtitle))
                            .child(subtitle.clone()),
                        "subtitle",
                        subtitle,
                        true,
                    )),
            )
            .child(close);

        let work = editor.working_draft(cx);
        let body = if pick {
            self.render_pick_body(editor, panel_width, window, cx)
        } else if subscription_step {
            self.render_subscription_body(editor, &colors, connected, window, cx)
        } else {
            self.render_form_body(editor, &work, window, cx)
        };
        let body = div()
            .id("provider-setup-body")
            .flex_1()
            .min_h_0()
            .px(px(26.))
            .py(px(22.))
            .overflow_y_scroll()
            .track_scroll(&editor.body_scroll)
            .child(body);

        let save_enabled = !busy && provider_editor_can_save(&work);
        let footer = if subscription_step {
            self.render_subscription_footer(editor, &colors, connected, window, cx)
        } else if pick {
            let cancel = dialog_button(
                "provider-setup-cancel",
                self.locale.text("取消", "Cancel"),
                false,
                !busy,
                &editor.focus.cancel,
                self.keyboard_focus,
                &colors,
                cx,
                |this, window, cx| this.editor_request_close(window, cx),
            );
            let next = dialog_button(
                "provider-setup-next",
                self.locale.text("下一步", "Next"),
                true,
                editor.selection.is_some() && !busy,
                &editor.focus.next,
                self.keyboard_focus,
                &colors,
                cx,
                |this, window, cx| this.editor_next(window, cx),
            );
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(26.))
                .py(px(16.))
                .border_t_1()
                .border_color(rgba(colors.footer_line))
                .bg(rgba(colors.dialog))
                .child(div().flex_1())
                .child(cancel)
                .child(next)
        } else if editor.subscription.is_some() {
            self.subscription_models_footer(editor, &work, save_enabled, &colors, cx)
        } else if editing {
            let delete = self
                .providers
                .deletable_provider(editor.draft.id.as_deref())
                .map(|_| {
                    let button = div()
                        .id("provider-setup-delete")
                        .min_h(px(42.))
                        .flex()
                        .items_center()
                        .pr(px(12.))
                        .text_size(px(13.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgba(colors.error_text))
                        .child(self.locale.text("删除", "Delete"));
                    let button = if busy {
                        button
                            .opacity(0.55)
                            .cursor(CursorStyle::OperationNotAllowed)
                    } else {
                        wire_dialog_button(
                            button.cursor_pointer().hover(|style| style.underline()),
                            &editor.focus.delete,
                            7.,
                            self.keyboard_focus,
                            |this, window, cx| this.editor_request_delete(window, cx),
                            cx,
                        )
                    };
                    mark_provider(button, "delete", self.locale.text("删除", "Delete"), !busy)
                });
            let save = dialog_button(
                "provider-setup-save",
                if self.providers.saving() {
                    self.locale.text("保存中…", "Saving…")
                } else {
                    self.locale.text("保存", "Save")
                },
                true,
                save_enabled,
                &editor.focus.save,
                self.keyboard_focus,
                &colors,
                cx,
                |this, window, cx| this.editor_save(false, window, cx),
            );
            let save_switch = dialog_button(
                "provider-setup-save-switch",
                if self.providers.saving() {
                    self.locale.text("保存中…", "Saving…")
                } else {
                    self.locale.text("保存并切换", "Save and switch")
                },
                false,
                save_enabled,
                &editor.focus.save_switch,
                self.keyboard_focus,
                &colors,
                cx,
                |this, window, cx| this.editor_save(true, window, cx),
            );
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(26.))
                .py(px(16.))
                .border_t_1()
                .border_color(rgba(colors.footer_line))
                .bg(rgba(colors.dialog))
                .children(delete)
                .child(div().flex_1())
                .child(mark_provider(
                    div().child(save),
                    "save",
                    self.locale.text("保存", "Save"),
                    save_enabled,
                ))
                .child(mark_provider(
                    div().child(save_switch),
                    "save-switch",
                    self.locale.text("保存并切换", "Save and switch"),
                    save_enabled,
                ))
        } else {
            let save_only = dialog_button(
                "provider-setup-save-only",
                if busy {
                    self.locale.text("保存中…", "Saving…")
                } else {
                    self.locale.text("仅保存", "Save only")
                },
                false,
                save_enabled,
                &editor.focus.save,
                self.keyboard_focus,
                &colors,
                cx,
                |this, window, cx| this.editor_save(false, window, cx),
            );
            let add_use = dialog_button(
                "provider-setup-add-use",
                if busy {
                    self.locale.text("处理中…", "Working…")
                } else {
                    self.locale.text("添加并使用", "Add and use")
                },
                true,
                save_enabled,
                &editor.focus.save_switch,
                self.keyboard_focus,
                &colors,
                cx,
                |this, window, cx| this.editor_save(true, window, cx),
            );
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(26.))
                .py(px(16.))
                .border_t_1()
                .border_color(rgba(colors.footer_line))
                .bg(rgba(colors.dialog))
                .child(div().flex_1())
                .child(save_only)
                .child(add_use)
        };

        let mut panel = div()
            .id("provider-setup-panel")
            .relative()
            .w(px(panel_width))
            .max_h(px(panel_height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(15.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.dialog))
            .text_color(rgba(colors.label))
            .text_size(px(14.))
            .shadow(vec![shadow(0x1a304b, 0.19, 18., 70.)])
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(editor) = &mut this.editor
                        && editor.select_open.take().is_some()
                    {
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        this.escape(window, cx);
                    }
                    "tab" => {
                        cx.stop_propagation();
                        this.cycle_modal_focus(event.keystroke.modifiers.shift, window, cx);
                    }
                    _ => {}
                }
            }))
            .child(header)
            .child(body)
            .child(footer);
        let overlay_busy = busy && !(subscription_step && login_phase == "waiting");
        if overlay_busy {
            panel = panel.child(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            );
        }
        let panel = if reduced_motion() {
            panel.into_any_element()
        } else {
            panel
                .with_animation(
                    "provider-setup-in",
                    Animation::new(Duration::from_millis(DIALOG_ANIMATION_MS))
                        .with_easing(surface_easing),
                    |panel, delta| panel.opacity(delta).top(px(5. * (1. - delta))),
                )
                .into_any_element()
        };
        // Frozen real-window baseline: the 520px dialog is centered inside the
        // right workspace. Only the terminal/entry surface is dimmed; the
        // product sidebar keeps its color and is not covered by this layer.
        let layer = div()
            .id("provider-setup-layer")
            .absolute()
            .occlude()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(22.))
            .bg(rgba(0x19283e3d))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if let Some(editor) = &mut this.editor
                        && let Some((select, _)) = editor.select_open.take()
                    {
                        let focus = editor.select_focus(select).clone();
                        window.focus(&focus);
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    this.editor_request_close(window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(panel);
        Some(div().absolute().inset_0().child(layer).into_any_element())
    }

    fn render_subscription_body(
        &self,
        editor: &Editor,
        colors: &EditorColors,
        connected: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let login = self.providers.subscription_login();
        let fl = editor.subscription.as_ref();
        let phase = login.phase;
        let existing = fl.is_some_and(|flow| flow.existing);
        let risk_checked = fl.is_some_and(|flow| flow.risk_checked);
        let busy = login.busy;
        let mut body = div().flex().flex_col().w_full();
        if phase == "risk" {
            let text = self.locale.text(
                "Google 的 Antigravity 条款不允许第三方工具使用该账号。被判定违规时，Antigravity 与 Gemini CLI 账号可能被停用，申诉机会有限。",
                "Google's Antigravity terms do not allow third-party tools to use the account. If a violation is detected, your Antigravity and Gemini CLI accounts may be suspended, with limited appeals.",
            );
            let display_text = self.locale.text(
                "Google 的 Antigravity 条款不允许第三方工具使用该账号。\n被判定违规时，Antigravity 与 Gemini CLI 账号可能被停用，申诉机会有限。",
                "Google's Antigravity terms do not allow third-party tools to use the account. If a violation is detected, your Antigravity and Gemini CLI accounts may be suspended, with limited appeals.",
            );
            body = body.child(mark_provider(
                div()
                    .mb(px(16.))
                    .text_size(px(13.))
                    .line_height(px(22.75))
                    .text_color(rgba(colors.secondary_text))
                    .whitespace_normal()
                    .child(display_text),
                "subscription-risk-body",
                text,
                true,
            ));
            let label = self.locale.text(
                "我了解风险，仍要继续",
                "I understand the risk and want to continue",
            );
            let mut check = div()
                .id("subscription-risk-check")
                .flex()
                .items_start()
                .gap(px(10.))
                .p(px(13.))
                .rounded(px(10.))
                .border_1()
                .border_color(rgba(colors.line_strong))
                .bg(rgba(colors.info_bg))
                .text_size(px(13.))
                .font_weight(FontWeight(600.))
                .line_height(px(20.15))
                .text_color(rgba(colors.label))
                .child(
                    div()
                        .size(px(18.))
                        .flex_none()
                        .mt(px(1.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(rgba(if risk_checked {
                            colors.primary
                        } else {
                            colors.line_strong
                        }))
                        .bg(rgba(if risk_checked {
                            colors.primary
                        } else {
                            colors.panel
                        }))
                        .text_size(px(12.))
                        .text_color(rgb(0xffffff))
                        .child(if risk_checked { "✓" } else { "" }),
                )
                .child(div().flex_1().min_w_0().child(label));
            if busy {
                check = check.opacity(0.55);
            } else {
                check = check.cursor_pointer();
                check = wire_dialog_button(
                    check,
                    &editor.focus.subscription_risk,
                    10.,
                    self.keyboard_focus,
                    |this, _, cx| this.toggle_subscription_risk(cx),
                    cx,
                );
            }
            body = body.child(mark_provider(
                check,
                "subscription-risk-check",
                label,
                !busy,
            ));
        } else {
            let kind = fl.map(|flow| flow.provider.as_str()).unwrap_or("codex");
            let app = if kind == "antigravity" {
                "Antigravity"
            } else {
                "ChatGPT"
            };
            let description = if kind == "antigravity" {
                self.locale.text(
                    "使用 Google AI 订阅中的 Claude 与 Gemini 模型",
                    "Use Claude and Gemini models from your Google AI subscription.",
                )
            } else {
                self.locale.text(
                    "使用你的 ChatGPT 订阅额度，不需要 API Key",
                    "Use your ChatGPT subscription. No API key needed.",
                )
            };
            let account_label = format!("{app} {}", self.locale.text("账号", "account"));
            let tone = if kind == "antigravity" {
                linear_gradient(
                    145.,
                    linear_color_stop(rgb(0x4a96e5), 0.),
                    linear_color_stop(rgb(0x7169df), 1.),
                )
            } else {
                linear_gradient(
                    145.,
                    linear_color_stop(rgb(0x198875), 0.),
                    linear_color_stop(rgb(0x0d7a69), 1.),
                )
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(12.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgba(colors.line))
                    .bg(rgba(colors.panel_muted))
                    .child(
                        div()
                            .size(px(38.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(9.))
                            .text_size(px(20.))
                            .font_weight(FontWeight(650.))
                            .text_color(rgb(0xffffff))
                            .bg(tone)
                            .child(if kind == "antigravity" { "A" } else { "C" }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight(650.))
                                    .text_color(rgba(colors.label))
                                    .child(account_label),
                            )
                            .child(
                                div()
                                    .mt(px(4.))
                                    .text_size(px(11.))
                                    .line_height(px(15.4))
                                    .text_color(rgba(colors.hint_text))
                                    .whitespace_normal()
                                    .child(description),
                            ),
                    ),
            );
            if phase == "waiting" {
                let waiting = self.locale.text(
                    "等待浏览器完成登录…",
                    "Waiting for the browser to finish signing in…",
                );
                let dot = div()
                    .size(px(8.))
                    .flex_none()
                    .rounded_full()
                    .bg(rgba(0x3284edff));
                let dot = if reduced_motion() {
                    dot.into_any_element()
                } else {
                    dot.with_animation(
                        "subscription-waiting-pulse",
                        Animation::new(Duration::from_millis(1100)).repeat(),
                        |dot, pulse| dot.opacity(1. - 0.75 * pulse),
                    )
                    .into_any_element()
                };
                body = body.child(mark_provider(
                    div()
                        .mt(px(18.))
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .text_size(px(13.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgba(colors.label))
                        .child(dot)
                        .child(waiting),
                    "subscription-waiting",
                    waiting,
                    true,
                ));
                let reopen_enabled = connected && login.can_reopen && !login.busy;
                let reopen_label = self.locale.text("重新打开浏览器", "Open browser again");
                body = body.child(mark_provider(
                    self.subscription_reopen_row(editor, colors, connected, cx),
                    "subscription-reopen",
                    reopen_label,
                    reopen_enabled,
                ));
            }
            if phase == "error" {
                let message = login
                    .error
                    .clone()
                    .map(|error| {
                        if error == "Sign-in was cancelled" {
                            self.locale
                                .text("登录已取消。", "Sign-in was cancelled.")
                                .to_owned()
                        } else {
                            error
                        }
                    })
                    .unwrap_or_else(|| {
                        self.locale
                            .text("登录未完成，请重试。", "Sign-in did not finish. Try again.")
                            .to_owned()
                    });
                body = body.child(self.subscription_error_box(&message, editor, colors));
                if login.can_reopen {
                    body = body.child(self.subscription_reopen_row(editor, colors, connected, cx));
                }
            } else if let Some(message) = login.error.as_deref() {
                // A failed browser launch keeps the attempt alive; show the
                // reason while the waiting chain continues.
                body = body.child(self.subscription_error_box(message, editor, colors));
            }
            if existing {
                let text = self.locale.text(
                    "已有该类账号，重新登录会替换它，模型设置会保留。",
                    "You already have this account type. Signing in again replaces it and keeps your model settings.",
                );
                body = body.child(mark_provider(
                    div()
                        .mt(px(18.))
                        .text_size(px(12.))
                        .line_height(px(19.2))
                        .text_color(rgba(colors.muted))
                        .whitespace_normal()
                        .child(text),
                    "subscription-replacement",
                    text,
                    true,
                ));
            }
        }
        if !connected {
            let text = self.locale.text(
                "供应商配置服务正在连接，请稍后重试",
                "The provider service is connecting. Try again shortly.",
            );
            body = body.child(
                div()
                    .mt(px(12.))
                    .text_size(px(11.))
                    .line_height(px(18.15))
                    .text_color(rgba(colors.muted))
                    .child(text),
            );
        }
        body
    }

    /// "Open browser again" plus the explicit system-browser fallback. Shown
    /// while waiting and after a recoverable browser failure, matching the
    /// frozen dialog's recovery path.
    fn subscription_reopen_row(
        &self,
        editor: &Editor,
        colors: &EditorColors,
        connected: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let login = self.providers.subscription_login();
        let reopen_enabled = connected && login.can_reopen && !login.busy;
        let reopen_label = self.locale.text("重新打开浏览器", "Open browser again");
        let reopen = if reopen_enabled {
            link_button(
                "subscription-reopen",
                reopen_label,
                &editor.focus.subscription_reopen,
                self.keyboard_focus,
                FocusRingStyle::Dialog,
                colors,
                cx,
                |this, _, cx| this.reopen_subscription_login(cx),
            )
        } else {
            div()
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(rgba(colors.muted))
                .child(reopen_label)
                .into_any_element()
        };
        let system_label = self.locale.text("使用系统浏览器", "Use system browser");
        let system_browser =
            if login.system_browser_available && connected && login.can_reopen && !login.busy {
                Some(link_button(
                    "subscription-system-browser",
                    system_label,
                    &editor.focus.subscription_system_browser,
                    self.keyboard_focus,
                    FocusRingStyle::Dialog,
                    colors,
                    cx,
                    |this, _, cx| this.reopen_subscription_in_system_browser(cx),
                ))
            } else if login.system_browser_available {
                Some(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgba(colors.muted))
                        .child(system_label)
                        .into_any_element(),
                )
            } else {
                None
            };
        div()
            .mt(px(10.))
            .flex()
            .items_center()
            .gap(px(14.))
            .flex_wrap()
            .child(reopen)
            .children(system_browser)
    }

    fn subscription_error_box(&self, message: &str, editor: &Editor, colors: &EditorColors) -> Div {
        mark_provider(
            div()
                .mt(px(14.))
                .p(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(rgba(colors.error_line))
                .bg(rgba(colors.error_bg))
                .text_size(px(12.))
                .line_height(px(19.2))
                .text_color(rgba(colors.error_text))
                .whitespace_normal()
                .child(message.to_owned())
                .track_focus(&editor.focus.error)
                .child(dialog_focus_ring(
                    editor.focus.error.clone(),
                    8.,
                    self.keyboard_focus,
                )),
            "subscription-error",
            message,
            true,
        )
    }

    fn render_subscription_footer(
        &self,
        editor: &Editor,
        colors: &EditorColors,
        connected: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let login = self.providers.subscription_login();
        let phase = login.phase;
        let busy = login.busy || self.providers.saving() || self.providers.deleting();
        let mut footer = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(26.))
            .py(px(16.))
            .border_t_1()
            .border_color(rgba(colors.footer_line))
            .bg(rgba(colors.dialog));
        if phase == "waiting" {
            return footer.child(dialog_button(
                "subscription-cancel",
                self.locale.text("取消", "Cancel"),
                false,
                true,
                &editor.focus.subscription_cancel,
                self.keyboard_focus,
                colors,
                cx,
                |this, window, cx| this.cancel_subscription_login_ui(window, cx),
            ));
        }
        let cancel = dialog_button(
            "subscription-cancel",
            self.locale.text("取消", "Cancel"),
            false,
            !busy,
            &editor.focus.subscription_cancel,
            self.keyboard_focus,
            colors,
            cx,
            |this, window, cx| this.editor_request_close(window, cx),
        );
        footer = footer.child(div().flex_1()).child(cancel);
        if phase == "risk" {
            let enabled = !busy
                && connected
                && editor
                    .subscription
                    .as_ref()
                    .is_some_and(|flow| flow.risk_checked);
            footer = footer.child(mark_provider(
                div().child(dialog_button(
                    "subscription-continue",
                    if busy {
                        self.locale.text("处理中…", "Working…")
                    } else {
                        self.locale.text("继续", "Continue")
                    },
                    true,
                    enabled,
                    &editor.focus.subscription_primary,
                    self.keyboard_focus,
                    colors,
                    cx,
                    |this, window, cx| this.confirm_subscription_risk(window, cx),
                )),
                "subscription-continue",
                self.locale.text("继续", "Continue"),
                enabled,
            ));
        } else if phase == "error" {
            let enabled = !busy && connected;
            footer = footer.child(mark_provider(
                div().child(dialog_button(
                    "subscription-retry",
                    if busy {
                        self.locale.text("处理中…", "Working…")
                    } else {
                        self.locale.text("重试", "Retry")
                    },
                    true,
                    enabled,
                    &editor.focus.subscription_primary,
                    self.keyboard_focus,
                    colors,
                    cx,
                    |this, window, cx| this.start_subscription_login(window, cx),
                )),
                "subscription-retry",
                self.locale.text("重试", "Retry"),
                enabled,
            ));
        } else {
            let enabled = !busy && connected;
            footer = footer.child(mark_provider(
                div().child(dialog_button(
                    "subscription-login",
                    self.locale.text("在浏览器中登录", "Sign in with browser"),
                    true,
                    enabled,
                    &editor.focus.subscription_primary,
                    self.keyboard_focus,
                    colors,
                    cx,
                    |this, window, cx| this.start_subscription_login(window, cx),
                )),
                "subscription-login",
                self.locale.text("在浏览器中登录", "Sign in with browser"),
                enabled,
            ));
        }
        footer
    }

    fn subscription_models_footer(
        &self,
        editor: &Editor,
        work: &ProviderDraft,
        save_enabled: bool,
        colors: &EditorColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.providers.saving();
        let editing = work.id.is_some();
        let delete = self
            .providers
            .deletable_provider(editor.draft.id.as_deref())
            .map(|_| {
                let button = div()
                    .id("subscription-delete")
                    .min_h(px(42.))
                    .flex()
                    .items_center()
                    .pr(px(12.))
                    .text_size(px(13.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgba(colors.error_text))
                    .child(self.locale.text("删除", "Delete"));
                let button = if busy {
                    button
                        .opacity(0.55)
                        .cursor(CursorStyle::OperationNotAllowed)
                } else {
                    wire_dialog_button(
                        button.cursor_pointer().hover(|style| style.underline()),
                        &editor.focus.delete,
                        7.,
                        self.keyboard_focus,
                        |this, window, cx| this.editor_request_delete(window, cx),
                        cx,
                    )
                };
                mark_provider(button, "delete", self.locale.text("删除", "Delete"), !busy)
            });
        let save_label = if busy {
            self.locale.text("保存中…", "Saving…")
        } else {
            self.locale.text("保存", "Save")
        };
        let save = dialog_button(
            "subscription-save",
            save_label,
            editing,
            save_enabled,
            &editor.focus.save,
            self.keyboard_focus,
            colors,
            cx,
            |this, window, cx| this.editor_save(false, window, cx),
        );
        let save_switch = dialog_button(
            "subscription-save-switch",
            save_label,
            !editing,
            save_enabled,
            &editor.focus.save_switch,
            self.keyboard_focus,
            colors,
            cx,
            |this, window, cx| this.editor_save(true, window, cx),
        );
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(26.))
            .py(px(16.))
            .border_t_1()
            .border_color(rgba(colors.footer_line))
            .bg(rgba(colors.dialog))
            .children(delete)
            .child(div().flex_1())
            .child(mark_provider(
                div().child(save),
                "save",
                save_label,
                save_enabled,
            ))
            .child(mark_provider(
                div().child(save_switch),
                "save-switch",
                self.locale.text("保存并切换", "Save and switch"),
                save_enabled,
            ))
    }

    fn render_pick_body(
        &self,
        editor: &Editor,
        panel_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let palette = *self.theme.effective().palette();
        let colors = EditorColors::new(&palette, self.is_dark());
        let busy = self.providers.saving();
        let query = editor.query.read(cx).text.clone();
        let body_width = (panel_width - 2. - 52.).max(0.);
        let card_width = if body_width >= 252. {
            (body_width - 12.) / 2.
        } else {
            body_width
        };
        let mut body = div().flex().flex_col().w(px(body_width));
        body = body.child(
            rename_field(editor.query.clone(), 44., 13., window, &colors, cx).w(px(body_width)),
        );
        let query_lower = query.trim().to_lowercase();
        let matches = |title: &str, description: &str, id: &str| {
            query_lower.is_empty()
                || format!("{title} {description} {id}")
                    .to_lowercase()
                    .contains(&query_lower)
        };
        let mut any_subscription = false;
        let mut subscription_grid = div().flex().flex_wrap().w_full().gap(px(12.));
        for (kind, title, description, icon, tone) in [
            (
                "codex",
                self.locale.text("ChatGPT 账号", "ChatGPT account"),
                self.locale.text(
                    "使用你的 ChatGPT 订阅额度，不需要 API Key",
                    "Use your ChatGPT subscription. No API key needed.",
                ),
                "C",
                linear_gradient(
                    145.,
                    linear_color_stop(rgb(0x198875), 0.),
                    linear_color_stop(rgb(0x0d7a69), 1.),
                ),
            ),
            (
                "antigravity",
                self.locale.text("Antigravity 账号", "Antigravity account"),
                self.locale.text(
                    "使用 Google AI 订阅中的 Claude 与 Gemini 模型",
                    "Use Claude and Gemini models from your Google AI subscription.",
                ),
                "A",
                linear_gradient(
                    145.,
                    linear_color_stop(rgb(0x4a96e5), 0.),
                    linear_color_stop(rgb(0x7169df), 1.),
                ),
            ),
        ] {
            if !matches(title, description, kind) {
                continue;
            }
            any_subscription = true;
            let selected = editor.selection == Some(PickChoice::Subscription(kind));
            let mut card = div()
                .id(SharedString::from(format!("provider-subscription-{kind}")))
                .w(px(card_width))
                .min_h(px(83.))
                .flex()
                .items_center()
                .gap(px(10.))
                .p(px(13.))
                .rounded(px(10.))
                .border_1()
                .border_color(rgba(if selected {
                    colors.card_selected_line
                } else {
                    colors.card_line
                }))
                .bg(rgba(if selected {
                    colors.card_selected_bg
                } else {
                    colors.panel
                }))
                .hover(move |style| {
                    style
                        .border_color(rgba(colors.card_hover_line))
                        .bg(rgba(colors.card_hover_bg))
                })
                .child(
                    div()
                        .size(px(38.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(9.))
                        .text_size(px(20.))
                        .font_weight(FontWeight(650.))
                        .text_color(rgb(0xffffff))
                        .bg(tone)
                        .child(icon),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight(650.))
                                .text_color(rgba(colors.label))
                                .whitespace_normal()
                                .child(title),
                        )
                        .child(
                            div()
                                .mt(px(5.))
                                .text_size(px(11.))
                                .line_height(px(15.4))
                                .text_color(rgba(colors.hint_text))
                                .whitespace_normal()
                                .child(description),
                        ),
                )
                .child(
                    div()
                        .text_size(px(17.))
                        .text_color(rgba(if selected {
                            colors.check
                        } else {
                            colors.card_marker
                        }))
                        .child(if selected { "✓" } else { "○" }),
                );
            if busy {
                card = card.opacity(0.55);
            } else {
                card = wire_dialog_button(
                    card,
                    if kind == "codex" {
                        &editor.focus.subscription_choices[0]
                    } else {
                        &editor.focus.subscription_choices[1]
                    },
                    10.,
                    self.keyboard_focus,
                    move |this, _, cx| {
                        if let Some(editor) = &mut this.editor {
                            editor.selection = Some(PickChoice::Subscription(kind));
                        }
                        cx.notify();
                    },
                    cx,
                );
            }
            subscription_grid = subscription_grid.child(mark_provider(
                card,
                format!("subscription-{kind}"),
                title,
                !busy,
            ));
        }
        if any_subscription {
            body = body.child(
                div()
                    .mt(px(22.))
                    .mb(px(10.))
                    .text_size(px(12.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgba(colors.section_title))
                    .child(self.locale.text("订阅账号", "Subscription accounts")),
            );
            body = body.child(subscription_grid);
        }
        body = body.child(
            div()
                .mt(px(22.))
                .mb(px(10.))
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(rgba(colors.section_title))
                .child(self.locale.text("API 服务", "API services")),
        );
        let mut grid = div().flex().flex_wrap().w_full().gap(px(12.));
        let mut any = false;
        for (index, preset) in PRESETS.iter().enumerate() {
            if !preset_matches(preset, &query) {
                continue;
            }
            any = true;
            let selected = editor.selection == Some(PickChoice::Preset(preset.id));
            let (icon_bg, icon_fg) = match preset.id {
                "deepseek" => (0x4a85ee, 0xffffff),
                "glm" => (0xeeeaff, 0x6756de),
                "kimi" => (0x26303a, 0xffffff),
                _ => (0xed386e, 0xffffff),
            };
            let mut card = div()
                .id(SharedString::from(format!("provider-preset-{}", preset.id)))
                .w(px(card_width))
                .min_h(px(83.))
                .flex()
                .items_center()
                .gap(px(10.))
                .p(px(13.))
                .rounded(px(10.))
                .border_1()
                .border_color(rgba(if selected {
                    colors.card_selected_line
                } else {
                    colors.card_line
                }))
                .bg(rgba(if selected {
                    colors.card_selected_bg
                } else {
                    colors.panel
                }))
                .hover(move |style| {
                    style
                        .border_color(rgba(colors.card_hover_line))
                        .bg(rgba(colors.card_hover_bg))
                })
                .child(
                    div()
                        .size(px(38.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(9.))
                        .text_size(px(24.))
                        .font_weight(FontWeight(650.))
                        .text_color(rgb(icon_fg))
                        .bg(rgb(icon_bg))
                        .child(preset.name.chars().next().unwrap_or('P').to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight(650.))
                                .text_color(rgba(colors.label))
                                .whitespace_normal()
                                .child(preset.name),
                        )
                        .child(
                            div()
                                .mt(px(5.))
                                .text_size(px(11.))
                                .line_height(px(15.4))
                                .text_color(rgba(colors.hint_text))
                                .whitespace_normal()
                                .child(self.locale.text("预设接入配置", "Preset configuration")),
                        ),
                )
                .child(
                    div()
                        .text_size(px(17.))
                        .text_color(rgba(if selected {
                            colors.check
                        } else {
                            colors.card_marker
                        }))
                        .child(if selected { "✓" } else { "○" }),
                );
            if busy {
                card = card.opacity(0.55);
            } else {
                let id = preset.id;
                card = wire_dialog_button(
                    card,
                    &editor.focus.presets[index],
                    10.,
                    self.keyboard_focus,
                    move |this, _, cx| {
                        if let Some(editor) = &mut this.editor {
                            editor.selection = Some(PickChoice::Preset(id));
                        }
                        cx.notify();
                    },
                    cx,
                );
            }
            grid = grid.child(card);
        }
        body = body.child(grid);
        if !any && !any_subscription && !query.trim().is_empty() {
            body = body.child(
                div()
                    .mt(px(8.))
                    .text_size(px(11.))
                    .line_height(px(18.15))
                    .text_color(rgba(colors.muted))
                    .child(self.locale.text(
                        "没有匹配的 API 服务，试试自定义配置。",
                        "No matching service. Try a custom configuration.",
                    )),
            );
        }
        let selected = editor.selection == Some(PickChoice::Custom);
        let mut custom =
            div()
                .id("provider-preset-custom")
                .mt(px(14.))
                .w_full()
                .min_h(px(83.))
                .flex()
                .items_center()
                .gap(px(10.))
                .p(px(13.))
                .rounded(px(10.))
                .border_1()
                .border_color(rgba(if selected {
                    colors.card_selected_line
                } else {
                    colors.card_line
                }))
                .bg(rgba(if selected {
                    colors.card_selected_bg
                } else {
                    colors.panel
                }))
                .hover(move |style| {
                    style
                        .border_color(rgba(colors.card_hover_line))
                        .bg(rgba(colors.card_hover_bg))
                })
                .child(
                    div()
                        .size(px(38.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(9.))
                        .bg(rgba(colors.icon_bg))
                        .text_size(px(24.))
                        .font_weight(FontWeight(650.))
                        .text_color(rgba(colors.icon_fg))
                        .child("＋"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight(650.))
                                .text_color(rgba(colors.label))
                                .child(self.locale.text("自定义服务", "custom service")),
                        )
                        .child(
                            div()
                                .mt(px(5.))
                                .text_size(px(11.))
                                .line_height(px(15.4))
                                .text_color(rgba(colors.hint_text))
                                .child(self.locale.text(
                                    "使用自己的 API 地址和密钥",
                                    "Use your own API URL and key",
                                )),
                        ),
                )
                .child(
                    div()
                        .text_size(px(17.))
                        .text_color(rgba(if selected {
                            colors.check
                        } else {
                            colors.card_marker
                        }))
                        .child("›"),
                );
        if busy {
            custom = custom.opacity(0.55);
        } else {
            custom = wire_dialog_button(
                custom,
                &editor.focus.custom,
                10.,
                self.keyboard_focus,
                |this, _, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor.selection = Some(PickChoice::Custom);
                    }
                    cx.notify();
                },
                cx,
            );
        }
        body = body.child(custom);

        body.child(
            div()
                .mt(px(22.))
                .text_size(px(11.))
                .line_height(px(18.15))
                .text_color(rgba(colors.muted))
                .child("ⓘ 选择服务后，自动填写推荐配置。预设不代表连接已验证。"),
        )
    }

    fn render_form_body(
        &self,
        editor: &Editor,
        work: &ProviderDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let palette = *self.theme.effective().palette();
        let colors = EditorColors::new(&palette, self.is_dark());
        let busy = self.editor_busy();
        let editing = work.id.is_some();
        let subscription = work.subscription_kind.is_some();
        let models = work.available_models();
        let mut body = div().flex().flex_col().w_full();
        if !subscription {
            body = body.child(self.render_api_fields(editor, work, &models, window, &colors, cx));
        }
        let name = self.locale.text("供应商名称", "Provider name");
        body = body.child(mark_provider(
            field_label(name, &colors).child(rename_field(
                editor.name.clone(),
                44.,
                13.,
                window,
                &colors,
                cx,
            )),
            "provider-name",
            name,
            !busy,
        ));

        if subscription && models.is_empty() {
            body = body.child(provider_inline_error(
                self.locale
                    .text("没有读到可用的模型", "No available models were found"),
                &colors,
            ));
        } else if !work.is_codex() {
            let mut roles = div().flex().flex_col().gap(px(10.)).mt(px(4.));
            for index in 0..ROLES.len() {
                let current = work.model_mapping[index].trim();
                let known = models.iter().any(|row| row["id"] == current);
                let model = if editor.manual_mappings[index] {
                    let label = self.locale.text("从列表选择", "Choose from list");
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(rename_field(
                            editor.mappings[index].clone(),
                            38.,
                            10.,
                            window,
                            &colors,
                            cx,
                        ))
                        .child(mark_provider(
                            div().child(link_button(
                                SharedString::from(format!("provider-mapping-list-{index}")),
                                label,
                                &editor.focus.manual_links[index],
                                self.keyboard_focus,
                                FocusRingStyle::Dialog,
                                &colors,
                                cx,
                                move |this, window, cx| {
                                    this.set_manual_mapping(index, false, window, cx)
                                },
                            )),
                            format!("mapping-list-{index}"),
                            label,
                            !busy,
                        ))
                } else {
                    self.provider_select_trigger(
                        editor,
                        work,
                        ProviderSelect::Mapping(index),
                        &colors,
                        cx,
                    )
                };
                let row = div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(58.))
                            .flex_none()
                            .pt(px(11.))
                            .text_size(px(12.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgba(colors.role_text))
                            .child(ROLE_LABELS[index]),
                    )
                    .child(div().flex_1().min_w_0().child(model))
                    .child(self.provider_select_trigger(
                        editor,
                        work,
                        ProviderSelect::Effort(index),
                        &colors,
                        cx,
                    ));
                let mut column = div().flex().flex_col().gap(px(5.)).child(row);
                if subscription && !known && !current.is_empty() {
                    column = column.child(self.provider_effort_error(
                        self.locale.text(
                            "模型已不可用；原选择已保留，请重新选择。",
                            "Model is unavailable. Your selection was kept; choose another model.",
                        ),
                        index,
                        &colors,
                    ));
                } else if !work.effort_supported(index, &work.effort_mapping[index]) {
                    column = column.child(self.provider_effort_error(
                        self.locale.text(
                            "该模型不支持当前推理强度。",
                            "This model does not support the selected reasoning effort.",
                        ),
                        index,
                        &colors,
                    ));
                }
                roles = roles.child(column);
            }
            let known_efforts = work
                .model_mapping
                .iter()
                .filter(|id| {
                    !work.ignore_model_efforts
                        && models
                            .iter()
                            .any(|row| row["id"] == id.trim() && row["supportedEfforts"].is_array())
                })
                .count();
            if known_efforts < ROLES.len() {
                let hint = if known_efforts > 0 && !subscription {
                    self.locale.text(
                        "已获取模型的推理强度来自接口返回范围；其余模型仍为候选，待服务确认。",
                        "Fetched models use their returned effort ranges; other models remain candidates pending service confirmation.",
                    )
                } else {
                    self.effort_fallback()
                };
                roles = roles.child(help_text(hint, &colors));
            }
            body = body.child(roles);
            body = body.child(self.render_context_picker(editor, work, &models, &colors, cx));
        }
        if !subscription {
            body = body.child(help_text(
                if editing {
                    self.locale.text(
                        "ⓘ 保存修改不会重新连接运行中的会话。",
                        "ⓘ Saving does not reconnect running sessions.",
                    )
                } else {
                    self.locale.text(
                        "ⓘ 仅保存不会切换；添加并使用会更新默认供应商。",
                        "ⓘ Save only keeps the configuration; Add and use changes the default.",
                    )
                },
                &colors,
            ));
        }

        if let Some(error) = editor.error.as_deref() {
            let mut error_box = div()
                .id("provider-setup-error")
                .relative()
                .mt(px(12.))
                .p(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(rgba(colors.error_line))
                .bg(rgba(colors.error_bg))
                .text_size(px(12.))
                .line_height(px(19.2))
                .text_color(rgba(colors.error_text))
                .whitespace_normal()
                .child(error.to_owned());
            error_box = error_box
                .track_focus(&editor.focus.error)
                .child(dialog_focus_ring(
                    editor.focus.error.clone(),
                    8.,
                    self.keyboard_focus,
                ));
            body = body.child(error_box);
        }

        if let Some(kind) = editor.confirm {
            let deleting = kind == ConfirmKind::Delete;
            let (title, description, action_id, action) = if deleting {
                let name = self
                    .providers
                    .deletable_provider(editor.draft.id.as_deref())
                    .and_then(|provider| provider["name"].as_str())
                    .unwrap_or(&editor.draft.name);
                (
                    format!("删除「{name}」？"),
                    self.locale.text(
                        "此操作移除保存的配置，不会注销账户。",
                        "This removes the saved configuration, not your account.",
                    ),
                    "provider-setup-confirm-delete",
                    self.locale.text("确认删除", "Delete"),
                )
            } else {
                (
                    self.locale
                        .text("有尚未保存的修改", "You have unsaved changes")
                        .to_owned(),
                    self.locale.text(
                        "离开后，这次填写的内容将被丢弃。",
                        "Leaving discards this draft.",
                    ),
                    "provider-setup-discard",
                    self.locale.text("放弃修改", "Discard"),
                )
            };
            let mut confirm = div()
                .mt(px(14.))
                .p(px(14.))
                .rounded(px(10.))
                .border_1()
                .border_color(rgba(colors.confirm_line))
                .bg(rgba(colors.confirm_bg))
                .text_size(px(12.))
                .child(mark_provider(
                    div()
                        .font_weight(FontWeight(650.))
                        .text_color(rgba(colors.label))
                        .child(title.clone()),
                    "confirmation",
                    title,
                    true,
                ))
                .child(
                    div()
                        .mt(px(6.))
                        .line_height(px(19.2))
                        .text_color(rgba(colors.muted))
                        .child(description),
                )
                .child(
                    div()
                        .mt(px(10.))
                        .flex()
                        .gap(px(10.))
                        .child(confirm_button(
                            "provider-setup-keep-editing",
                            self.locale.text("继续编辑", "Keep editing"),
                            !self.providers.deleting(),
                            &editor.focus.keep_editing,
                            self.keyboard_focus,
                            &colors,
                            cx,
                            |this, window, cx| this.editor_keep_editing(window, cx),
                        ))
                        .child(confirm_button(
                            action_id,
                            action,
                            !self.providers.deleting(),
                            &editor.focus.confirm_action,
                            self.keyboard_focus,
                            &colors,
                            cx,
                            |this, window, cx| this.editor_confirm(window, cx),
                        )),
                );
            confirm = confirm.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            body = body.child(confirm);
        }
        body
    }

    fn render_api_fields(
        &self,
        editor: &Editor,
        work: &ProviderDraft,
        models: &[&Value],
        window: &mut Window,
        colors: &EditorColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.editor_busy();
        let editing = work.id.is_some();
        let preset = if editing {
            provider_preset_for_endpoint(work.is_codex(), &work.original_endpoint)
        } else {
            match editor.selection {
                Some(PickChoice::Preset(id)) => preset_by_id(id),
                _ => None,
            }
        };
        let display_name = preset
            .map_or(self.locale.text("自定义服务", "Custom service"), |p| {
                p.name
            });
        let protocol = if work.is_codex() {
            "OpenAI Responses"
        } else {
            "Anthropic Messages"
        };
        let mut strip = div()
            .flex()
            .items_center()
            .gap(px(11.))
            .p(px(12.))
            .mb(px(20.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.panel_muted))
            .child(provider_preset_badge(preset, &work.name))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(650.))
                            .child(display_name),
                    )
                    .child(mark_provider(
                        div()
                            .mt(px(5.))
                            .text_size(px(11.))
                            .line_height(px(15.4))
                            .text_color(rgba(colors.hint_text))
                            .child(protocol),
                        "protocol",
                        protocol,
                        true,
                    )),
            );
        if !editing && !busy {
            strip = strip.child(link_button(
                "provider-setup-change",
                self.locale.text("更换", "Change"),
                &editor.focus.change,
                self.keyboard_focus,
                FocusRingStyle::Dialog,
                colors,
                cx,
                |this, window, cx| this.editor_go_back(window, cx),
            ));
        }

        let key_heading = div()
            .flex()
            .items_center()
            .justify_between()
            .child(mark_provider(
                div().child("API Key"),
                "key-label",
                "API Key",
                true,
            ))
            .children(preset.map(|preset| {
                let text = self.locale.text("获取密钥 ↗", "Get API key ↗");
                mark_provider(
                    div().child(link_button(
                        "provider-key-link",
                        text,
                        &editor.focus.key_link,
                        self.keyboard_focus,
                        FocusRingStyle::Dialog,
                        colors,
                        cx,
                        move |_, _, cx| {
                            cx.emit(SettingsShellEvent::OpenProviderKeyPage(preset.key_url))
                        },
                    )),
                    "key-link",
                    text,
                    !busy,
                )
            }));
        let key_field = field_container(colors)
            .child(key_heading)
            .child(secret_field(
                editor.key.clone(),
                44.,
                13.,
                editor.revealed_key,
                &editor.focus.reveal,
                self.keyboard_focus,
                window,
                colors,
                cx,
                |this, _, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor.revealed_key = !editor.revealed_key;
                    }
                    cx.notify();
                },
            ))
            .child(help_text(
                self.locale.text(
                    "密钥保存在本机，仅向你配置的服务发送。",
                    "Stored locally and sent only to your configured service.",
                ),
                colors,
            ));
        let endpoint_field = field_label(self.locale.text("API 地址", "API URL"), colors)
            .child(rename_field(editor.endpoint.clone(), 44., 13., window, colors, cx))
            .child(help_text(self.locale.text(
                "填写基础地址，不含 /messages 或 /responses；需支持上方协议。",
                "Use a base URL without /messages or /responses, supporting the protocol above.",
            ), colors));

        let fetching = self.providers.fetching();
        let fetch_text = if fetching {
            self.locale.text("正在获取…", "Fetching…")
        } else {
            self.locale.text("获取模型", "Get models")
        };
        let can_fetch =
            !busy && !fetching && self.commands.connection().1 == ConnectionPhase::Ready;
        let fetch_button = if can_fetch {
            link_button(
                "provider-model-fetch",
                fetch_text,
                &editor.focus.fetch,
                self.keyboard_focus,
                FocusRingStyle::Dialog,
                colors,
                cx,
                |this, _, cx| this.request_provider_models(cx),
            )
        } else {
            div()
                .text_size(px(12.))
                .text_color(rgba(colors.muted))
                .child(fetch_text)
                .into_any_element()
        };
        let model_label = self.locale.text("默认模型", "Default model");
        let model_id = work.model.trim();
        let metadata = models.iter().find(|row| row["id"] == model_id).copied();
        let mut model_field = field_container(colors)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(mark_provider(
                        div().child(model_label),
                        "model-label",
                        model_label,
                        true,
                    ))
                    .child(mark_provider(
                        div().child(fetch_button),
                        "fetch",
                        fetch_text,
                        can_fetch,
                    )),
            )
            .child(rename_field(
                editor.model.clone(),
                44.,
                13.,
                window,
                colors,
                cx,
            ));
        if let Some(row) = metadata {
            model_field = model_field.child(self.model_details(row, "model-detail", true, colors));
        }
        let confirmed = editor.fetch_error.is_none() && work.fetched_metadata.is_some();
        let source = self.model_source(
            fetching,
            editor.fetch_error.is_some(),
            confirmed,
            editing,
            models.len(),
            model_id,
            metadata,
            preset,
        );
        model_field = model_field.child(mark_provider(
            help_text(source.clone(), colors),
            "model-source",
            source.clone(),
            true,
        ));
        if confirmed && let Some(row) = metadata {
            let no_name = row["displayName"].as_str().is_none();
            let no_window = row["contextWindow"].is_null() && row["maxContextWindow"].is_null();
            let gap = match (no_name, no_window) {
                (true, true) => self.locale.text(
                    "本次响应未提供显示名称与窗口。",
                    "This response did not provide a display name or window.",
                ),
                (true, false) => self.locale.text(
                    "本次响应只返回模型 ID，未提供显示名称。",
                    "This response returned only the model ID; no display name was provided.",
                ),
                (false, true) => self.locale.text(
                    "本次响应未提供窗口。",
                    "This response did not provide a window.",
                ),
                (false, false) => "",
            };
            if !gap.is_empty() {
                model_field = model_field.child(help_text(gap, colors));
            }
        }
        if let Some(error) = &editor.fetch_error {
            model_field = model_field.child(help_text(self.model_fetch_message(error), colors));
        } else if confirmed && !fetching {
            let message = if self.locale == InterfaceLocale::Chinese {
                format!("已获取 {} 个模型；尚未验证实际对话能力。", models.len())
            } else {
                format!(
                    "{} models fetched; actual inference has not been tested.",
                    models.len()
                )
            };
            model_field = model_field.child(help_text(message, colors));
        }
        if editor.model_chooser {
            model_field =
                model_field.child(self.render_model_chooser(editor, work, window, colors, cx));
        } else if confirmed && !fetching {
            model_field = model_field.child(link_button(
                "provider-model-choose",
                self.locale
                    .text("从已获取列表选择", "Choose from fetched models"),
                &editor.focus.chooser,
                self.keyboard_focus,
                FocusRingStyle::Dialog,
                colors,
                cx,
                |this, window, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor.model_chooser = true;
                        window.focus(&editor.focus.model_search);
                    }
                    cx.notify();
                },
            ));
        }
        if work.is_codex() {
            let title = self.locale.text("推理强度", "Reasoning effort");
            let explanation = if !work.ignore_model_efforts
                && metadata.is_some_and(|row| row["supportedEfforts"].is_array())
            {
                self.locale.text(
                    "推理强度选项来自接口返回的模型信息。",
                    "Reasoning effort options come from the fetched model information.",
                )
            } else {
                self.effort_fallback()
            };
            let mut section = div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .mt(px(7.))
                .child(div().child(title))
                .child(self.provider_select_trigger(
                    editor,
                    work,
                    ProviderSelect::Effort(0),
                    colors,
                    cx,
                ))
                .child(help_text(explanation, colors));
            if !work.effort_supported(0, &work.reasoning_effort) {
                section = section.child(self.provider_effort_error(
                    self.locale.text(
                        "当前模型不支持所选推理强度。",
                        "This model does not support the selected reasoning effort.",
                    ),
                    0,
                    colors,
                ));
            }
            model_field = model_field.child(mark_provider(section, "codex-effort", title, !busy));
        }

        div()
            .flex()
            .flex_col()
            .child(strip)
            .child(key_field)
            .child(endpoint_field)
            .children(work.endpoint.starts_with("http:").then(|| {
                provider_inline_error(
                    self.locale.text(
                        "当前为 HTTP，密钥将以明文传输。远程服务建议使用 HTTPS。",
                        "HTTP sends the key unencrypted. Use HTTPS for remote services.",
                    ),
                    colors,
                )
                .mb(px(20.))
            }))
            .child(model_field)
    }

    fn model_source(
        &self,
        fetching: bool,
        failed: bool,
        confirmed: bool,
        editing: bool,
        count: usize,
        model: &str,
        metadata: Option<&Value>,
        preset: Option<&ProviderPreset>,
    ) -> String {
        let locale = self.locale;
        let text = if fetching {
            locale.text(
                "正在向当前地址获取模型信息…",
                "Fetching model information from this endpoint…",
            )
        } else if failed {
            if count > 0 {
                locale.text("本次刷新失败；以下为上次记录的模型信息，名称与窗口可能已变化。", "Refresh failed. The last recorded model information is shown; names and windows may have changed.")
            } else {
                locale.text(
                    "本次刷新失败；当前没有可用的模型名称或窗口信息。",
                    "Refresh failed. No model names or windows are available.",
                )
            }
        } else if confirmed {
            if model.is_empty() {
                return if locale == InterfaceLocale::Chinese {
                    format!("已获取 {count} 个模型；请选择模型，或手动填写模型 ID。")
                } else {
                    format!("{count} models fetched. Choose a model or enter an ID manually.")
                };
            } else if metadata.is_some() {
                locale.text(
                    "已在本次获取的模型列表中确认该 ID。",
                    "This ID was confirmed in the models fetched in this request.",
                )
            } else {
                locale.text("手动填写的模型 ID 不在本次获取列表中；名称与窗口未知。", "The manually entered model ID is not in this response; its name and window are unknown.")
            }
        } else if count > 0 {
            if metadata.is_some() || model.is_empty() {
                locale.text(
                    "显示已保存的模型信息，可获取模型刷新。",
                    "Showing saved model information. Fetch models to refresh it.",
                )
            } else {
                locale.text("手动填写的模型 ID 不在已保存的模型信息中；名称与窗口未知。", "The manually entered ID is not in the saved model information; its name and window are unknown.")
            }
        } else if editing {
            locale.text(
                "当前仅有模型 ID；尚未获取名称或窗口信息。",
                "Only a model ID is available; its name and window have not been fetched.",
            )
        } else if preset.is_some_and(|preset| preset.model == model) {
            locale.text("预设模型尚未向服务验证名称、窗口或对话能力。", "The preset model has not been verified against the service for its name, window, or inference.")
        } else {
            locale.text(
                "手动填写模型 ID；名称与窗口未知。",
                "Entered manually; the name and window are unknown.",
            )
        };
        text.into()
    }

    fn model_fetch_message(&self, error: &str) -> &'static str {
        let l = self.locale;
        match error {
            "auth" => l.text(
                "密钥未通过验证，请检查密钥和对应的服务地址。",
                "The key was rejected. Check the key and service URL.",
            ),
            "unsupported" => l.text(
                "该地址暂不支持模型列表，请手动填写模型名称。",
                "This endpoint does not offer a compatible model list. Enter the model manually.",
            ),
            "timeout" => l.text(
                "获取超时，已保留填写内容。可以重试或手动填写。",
                "Request timed out. Your draft is kept; retry or enter a model manually.",
            ),
            "invalid" => l.text(
                "请先填写有效的 API 基础地址。",
                "Enter a valid API base URL first.",
            ),
            "key-required" => l.text(
                "请填写此地址对应的 API Key；修改地址后需重新填写密钥。",
                "Enter the key for this endpoint. A changed endpoint requires re-entering the key.",
            ),
            "empty" => l.text(
                "服务没有返回模型，可以手动填写模型名称。",
                "No models returned. Enter a model manually.",
            ),
            "too-large" => l.text(
                "模型列表过大，请手动填写模型名称。",
                "Model list is too large. Enter a model manually.",
            ),
            _ => l.text(
                "连接未完成，请检查网络或服务地址后重试。",
                "Connection failed. Check the network and service URL, then retry.",
            ),
        }
    }

    fn model_details(&self, row: &Value, marker: &str, card: bool, colors: &EditorColors) -> Div {
        let id = row["id"].as_str().unwrap_or_default();
        let name = row["displayName"].as_str().unwrap_or(id);
        let id_text = if row["displayName"].is_string() {
            id
        } else {
            self.locale.text("仅返回模型 ID", "Model ID only")
        };
        let tokens = window_text(row, self.locale);
        let mut view = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .font_weight(FontWeight(650.))
                            .child(name.to_owned()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(10.))
                            .font_weight(FontWeight(400.))
                            .text_color(rgba(colors.muted))
                            .child(id_text.to_owned()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgba(colors.link))
                    .child(tokens.clone()),
            );
        if card {
            view = view
                .p(px(9.))
                .px(px(11.))
                .rounded(px(9.))
                .border_1()
                .border_color(rgba(colors.info_line))
                .bg(rgba(colors.info_bg));
        }
        mark_provider(view, marker, format!("{name} {id_text} {tokens}"), true)
    }

    fn render_model_chooser(
        &self,
        editor: &Editor,
        work: &ProviderDraft,
        window: &mut Window,
        colors: &EditorColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = editor.visible_fetched_models(cx);
        let mut list = div()
            .id("provider-model-options")
            .max_h(px(190.))
            .overflow_y_scroll()
            .track_scroll(&editor.model_scroll);
        if rows.is_empty() {
            list = list.child(help_text(
                self.locale.text("没有匹配模型", "No matching model"),
                colors,
            ));
        }
        for (index, row) in rows.iter().enumerate() {
            let id = row["id"].as_str().unwrap_or_default().to_owned();
            let selected = id == work.model.trim();
            let check = if selected { "✓" } else { "" };
            let marker = format!("option-{id}");
            let label = format!(
                "{} {} {} {check}",
                row["displayName"].as_str().unwrap_or(&id),
                id,
                window_text(row, self.locale)
            );
            let option = div()
                .id(SharedString::from(format!("provider-model-option-{id}")))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .bg(rgba(if index == editor.model_cursor || selected {
                    colors.menu_selected_bg
                } else {
                    colors.panel
                }))
                .hover(move |s| s.bg(rgba(colors.menu_selected_bg)))
                .cursor_pointer()
                .child(self.model_details(row, "model-option-details", false, colors))
                .child(
                    div()
                        .w(px(12.))
                        .flex_none()
                        .text_color(rgba(colors.link))
                        .child(check),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor.choose_model(id.clone(), window, cx);
                    }
                    cx.stop_propagation();
                }));
            list = list.child(mark_provider(option, marker, label, true));
        }
        div()
            .id("provider-model-chooser")
            .flex()
            .flex_col()
            .gap(px(5.))
            .p(px(6.))
            .rounded(px(9.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.panel))
            .child(rename_field(
                editor.model_query.clone(),
                34.,
                8.,
                window,
                colors,
                cx,
            ))
            .child(list)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let Some(editor) = &mut this.editor else {
                    return;
                };
                let count = editor.visible_fetched_models(cx).len();
                if count == 0 {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "arrowdown" => editor.model_cursor = (editor.model_cursor + 1) % count,
                    "arrowup" => editor.model_cursor = (editor.model_cursor + count - 1) % count,
                    _ => return,
                }
                editor.model_scroll.scroll_to_item(editor.model_cursor);
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element()
    }

    fn effort_fallback(&self) -> &'static str {
        self.locale.text(
            "推理强度选项为候选，实际可用性以供应商支持为准。",
            "Reasoning effort options are candidates; actual availability depends on provider support.",
        )
    }

    fn provider_effort_error(
        &self,
        text: &'static str,
        index: usize,
        colors: &EditorColors,
    ) -> Div {
        mark_provider(
            div()
                .pl(px(68.))
                .text_size(px(11.))
                .font_weight(FontWeight(600.))
                .text_color(rgba(colors.error_text))
                .child(text),
            format!("effort-invalid-{index}"),
            text,
            true,
        )
    }

    fn render_provider_window_badge(&self, provider: &Value) -> Div {
        let label = provider_window_badge_label(
            provider,
            self.assignments.native_catalog_models(),
            self.locale,
        );
        mark_provider(
            context_window_badge_view(label.clone()),
            format!(
                "provider-window-badge-{}",
                provider["id"].as_str().unwrap_or_default()
            ),
            label,
            true,
        )
    }

    fn render_context_picker(
        &self,
        editor: &Editor,
        work: &ProviderDraft,
        models: &[&Value],
        colors: &EditorColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let local_models;
        let catalog = if let Some(native) = official_context_picker_models(
            work,
            self.providers.providers(),
            self.assignments.native_catalog_models(),
        ) {
            native
        } else {
            local_models = models.iter().map(|row| (*row).clone()).collect::<Vec<_>>();
            &local_models
        };
        let selected_ids: Vec<_> = work
            .model_mapping
            .iter()
            .chain((work.subscription_kind.is_none()).then_some(&work.model))
            .filter(|id| !id.trim().is_empty() && id.trim() != "__cli_default__")
            .cloned()
            .collect();
        let busy = self.editor_busy();
        let mode = work.context_mode();
        let mut options = div().flex().gap(px(8.));
        for (index, choice) in CONTEXT_MODES.into_iter().enumerate() {
            let selected = choice == mode;
            let label = context_mode_title(choice, self.locale);
            let tokens = context_mode_tokens(choice, catalog, &selected_ids, self.locale);
            let option = div()
                .id(SharedString::from(format!("provider-context-{choice}")))
                .flex()
                .items_start()
                .gap(px(6.))
                .flex_1()
                .min_w_0()
                .p(px(8.))
                .rounded(px(8.))
                .border_1()
                .border_color(rgba(if selected {
                    colors.focus
                } else {
                    colors.line_strong
                }))
                .bg(rgba(if selected {
                    colors.menu_selected_bg
                } else {
                    colors.panel
                }))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(13.))
                        .text_color(rgba(if selected { colors.link } else { colors.muted }))
                        .child(if selected { "◉" } else { "○" }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(3.))
                        .child(
                            div()
                                .font_weight(FontWeight(650.))
                                .text_size(px(12.))
                                .child(label),
                        )
                        .child(mark_provider(
                            div()
                                .font_weight(FontWeight(650.))
                                .text_size(px(11.))
                                .text_color(rgba(colors.link))
                                .child(tokens.clone()),
                            format!("context-{choice}-tokens"),
                            tokens,
                            true,
                        ))
                        .child(help_text(
                            context_mode_subtitle(choice, self.locale),
                            colors,
                        )),
                );
            let option = if busy {
                option.opacity(0.55)
            } else {
                wire_dialog_button(
                    option.cursor_pointer(),
                    &editor.focus.context_modes[index],
                    8.,
                    self.keyboard_focus,
                    move |this, _, cx| {
                        if let Some(editor) = &mut this.editor {
                            editor.draft.context_window_mode = Some(choice.into());
                            editor.context_touched = true;
                            editor.error = None;
                        }
                        cx.notify();
                    },
                    cx,
                )
            };
            options = options.child(mark_provider(
                option,
                format!("context-{choice}"),
                label,
                !busy,
            ));
        }
        let title = self.locale.text("上下文窗口长度", "Context window");
        let mut picker = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .mt(px(12.))
            .mb(px(14.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight(600.))
                    .child(title),
            )
            .child(options);
        if selected_ids
            .iter()
            .map(|id| id.trim())
            .collect::<HashSet<_>>()
            .len()
            > 1
        {
            picker = picker.child(help_text(context_multi_model_hint(self.locale), colors));
        }
        if mode != "512k" && resolve_context_tokens(mode, catalog, &selected_ids).is_none() {
            let hint = context_unknown_hint(self.locale);
            picker = picker.child(mark_provider(
                help_text(hint, colors),
                "context-unknown",
                hint,
                true,
            ));
        }
        #[cfg(debug_assertions)]
        PROVIDER_EDITOR_PROBE.with_borrow_mut(|probe| {
            if let Some(probe) = probe {
                probe.context_mode = Some(mode.to_owned());
            }
        });
        mark_provider(picker, "context", title, !busy)
    }

    fn request_provider_models(&mut self, cx: &mut Context<Self>) {
        if self.editor_busy() || self.providers.fetching() {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        let draft = editor.working_draft(cx);
        editor.model_chooser = false;
        editor.select_open = None;
        editor.fetch_error = None;
        match self.providers.fetch_models(&draft) {
            Ok(()) => {
                editor.fetch_snapshot =
                    Some((draft.api_key.trim().into(), draft.endpoint.trim().into()))
            }
            Err(_) => editor.fetch_error = Some("network".into()),
        }
        cx.notify();
    }

    fn set_manual_mapping(
        &mut self,
        index: usize,
        manual: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &mut self.editor {
            editor.manual_mappings[index] = manual;
            editor.select_open = None;
            window.focus(if manual {
                &editor.focus.mappings[index]
            } else {
                &editor.focus.mapping_choices[index]
            });
            cx.notify();
        }
    }

    fn mapping_label(&self, work: &ProviderDraft, index: usize) -> (String, bool) {
        let id = work.model_mapping[index].trim();
        if id.is_empty() {
            (
                self.locale.text("请选择模型", "Select a model").into(),
                true,
            )
        } else if !work.ignore_model_efforts
            && let Some(row) = work
                .available_models()
                .into_iter()
                .find(|row| row["id"] == id)
        {
            (row["displayName"].as_str().unwrap_or(id).into(), true)
        } else if work.subscription_kind.is_some() {
            (
                format!("{id} · {}", self.locale.text("不可用", "Unavailable")),
                false,
            )
        } else if work.ignore_model_efforts || work.available_models().is_empty() {
            (
                format!(
                    "{id} {}",
                    self.locale.text("（尚未获取列表）", "(list not fetched)")
                ),
                true,
            )
        } else {
            (
                format!(
                    "{id} {}",
                    self.locale.text("（未在列表中）", "(not in list)")
                ),
                true,
            )
        }
    }

    fn provider_select_options(
        &self,
        work: &ProviderDraft,
        select: ProviderSelect,
    ) -> Vec<SelectOption> {
        match select {
            ProviderSelect::Effort(index) => {
                let current = if work.is_codex() {
                    &work.reasoning_effort
                } else {
                    &work.effort_mapping[index]
                };
                work.effort_choices(index)
                    .into_iter()
                    .map(|effort| {
                        let enabled = work.effort_supported(index, &effort);
                        let label = editor_effort_name(&effort, self.locale);
                        SelectOption {
                            label: if enabled {
                                label.into()
                            } else {
                                format!(
                                    "{label} {}",
                                    self.locale.text("（不支持）", "(unsupported)")
                                )
                            },
                            selected: current == &effort,
                            choice: ProviderChoice::Effort(effort),
                            detail: None,
                            enabled,
                        }
                    })
                    .collect()
            }
            ProviderSelect::Mapping(index) => {
                let id = work.model_mapping[index].trim();
                let subscription = work.subscription_kind.is_some();
                let models = if work.ignore_model_efforts && !subscription {
                    Vec::new()
                } else {
                    work.available_models()
                };
                let known = models.iter().any(|row| row["id"] == id);
                let mut options = Vec::with_capacity(models.len() + 2);
                options.extend(models.into_iter().map(|row| {
                    let model_id = row["id"].as_str().unwrap_or_default();
                    let window = window_text(row, self.locale);
                    SelectOption {
                        choice: ProviderChoice::Model(model_id.into()),
                        label: row["displayName"].as_str().unwrap_or(model_id).into(),
                        detail: Some(format!("{model_id} · {window}")),
                        selected: model_id == id,
                        enabled: true,
                    }
                }));
                if !known && !id.is_empty() {
                    let (label, enabled) = self.mapping_label(work, index);
                    options.push(SelectOption {
                        choice: ProviderChoice::Model(id.into()),
                        label,
                        detail: None,
                        selected: true,
                        enabled,
                    });
                }
                if !subscription || ROLES[index] == "fable" {
                    options.push(SelectOption {
                        choice: ProviderChoice::Manual,
                        label: self
                            .locale
                            .text("手动填写模型 ID…", "Enter model ID manually…")
                            .into(),
                        detail: None,
                        selected: false,
                        enabled: true,
                    });
                }
                options
            }
        }
    }

    fn provider_select_trigger(
        &self,
        editor: &Editor,
        work: &ProviderDraft,
        select: ProviderSelect,
        colors: &EditorColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let opened = editor.select_open.is_some_and(|(kind, _)| kind == select);
        let focus = editor.select_focus(select).clone();
        let slot = select.slot();
        let cell = editor.select_bounds.clone();
        let busy = self.editor_busy();
        let (marker, label, valid, width) = match select {
            ProviderSelect::Mapping(index) => {
                let (label, valid) = self.mapping_label(work, index);
                (format!("mapping-{index}"), label, valid, None)
            }
            ProviderSelect::Effort(index) => {
                let current = if work.is_codex() {
                    &work.reasoning_effort
                } else {
                    &work.effort_mapping[index]
                };
                let valid = work.effort_supported(index, current);
                let label = editor_effort_name(current, self.locale);
                let label = if valid {
                    label.into()
                } else {
                    format!(
                        "{label} {}",
                        self.locale.text("（不支持）", "(unsupported)")
                    )
                };
                (
                    format!("effort-{index}"),
                    label,
                    valid,
                    (!work.is_codex()).then_some(132.),
                )
            }
        };
        let mut trigger = div()
            .id(SharedString::from(format!("provider-{marker}")))
            .relative()
            .h(px(38.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .px(px(10.))
            .rounded(px(9.))
            .border_1()
            .border_color(rgba(if !valid {
                colors.error_border
            } else if opened {
                colors.focus
            } else {
                colors.line_strong
            }))
            .bg(rgba(colors.panel))
            .text_size(px(12.))
            .text_color(rgba(if valid {
                colors.label
            } else {
                colors.error_text
            }))
            .child(div().flex_1().min_w_0().truncate().child(label.clone()))
            .child(icon("chevron-down", 12., colors.muted >> 8))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, _, _| {
                        cell.borrow_mut()[slot] = Some(bounds);
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            );
        trigger = if let Some(width) = width {
            trigger.w(px(width)).flex_none()
        } else {
            trigger.w_full()
        };
        if busy {
            trigger = trigger.opacity(0.55);
        } else {
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
                .on_click(cx.listener(move |this, event, _, cx| {
                    if !matches!(event, ClickEvent::Keyboard(_)) {
                        this.toggle_provider_select(select, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    if !matches!(key, "enter" | "space" | "arrowdown" | "arrowup") {
                        return;
                    }
                    this.provider_select_key(select, key, window, cx);
                    cx.stop_propagation();
                }))
                .child(dialog_focus_ring(focus, 9., self.keyboard_focus));
        }
        mark_provider(
            div()
                .w_full()
                .when_some(width, |element, width| element.w(px(width)).flex_none())
                .child(trigger),
            marker,
            label,
            !busy,
        )
    }

    fn toggle_provider_select(&mut self, select: ProviderSelect, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let work = editor.working_draft(cx);
        let options = self.provider_select_options(&work, select);
        let editor = self.editor.as_mut().unwrap();
        if editor.select_open.is_some_and(|(kind, _)| kind == select) {
            editor.select_open = None;
        } else if !options.is_empty() {
            let index = options
                .iter()
                .position(|option| option.selected)
                .unwrap_or_default();
            editor.select_open = Some((select, index));
            editor.select_scroll.set_offset(Point::default());
            editor.select_scroll.scroll_to_item(index);
        }
        cx.notify();
    }

    fn provider_select_key(
        &mut self,
        select: ProviderSelect,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        let Some((kind, index)) = editor.select_open.filter(|(kind, _)| *kind == select) else {
            self.toggle_provider_select(select, cx);
            return;
        };
        let options = self.provider_select_options(&editor.working_draft(cx), kind);
        if options.is_empty() {
            return;
        }
        if matches!(key, "enter" | "space") {
            if let Some(option) = options
                .into_iter()
                .nth(index)
                .filter(|option| option.enabled)
            {
                self.choose_provider_option(kind, &option.choice, window, cx);
            }
        } else {
            let next = if key == "arrowup" {
                (index + options.len() - 1) % options.len()
            } else {
                (index + 1) % options.len()
            };
            let editor = self.editor.as_mut().unwrap();
            editor.select_open = Some((kind, next));
            editor.select_scroll.scroll_to_item(next);
            cx.notify();
        }
    }

    fn choose_provider_option(
        &mut self,
        select: ProviderSelect,
        choice: &ProviderChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match (select, choice) {
            (ProviderSelect::Mapping(index), ProviderChoice::Manual) => {
                self.set_manual_mapping(index, true, window, cx);
                return;
            }
            (ProviderSelect::Mapping(index), ProviderChoice::Model(id)) => {
                editor.mappings[index].update(cx, |input, cx| {
                    input.text.clone_from(id);
                    cx.notify();
                });
            }
            (ProviderSelect::Effort(index), ProviderChoice::Effort(effort)) => {
                if editor.draft.is_codex() {
                    editor.draft.reasoning_effort.clone_from(effort);
                } else {
                    editor.efforts[index].clone_from(effort);
                }
            }
            _ => return,
        }
        editor.select_open = None;
        editor.error = None;
        window.focus(editor.select_focus(select));
        cx.notify();
    }

    fn render_provider_select(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = *self.theme.effective().palette();
        let colors = EditorColors::new(&palette, self.is_dark());
        let editor = self.editor.as_ref()?;
        let (select, cursor) = editor.select_open?;
        let bounds = (*editor.select_bounds.borrow())[select.slot()]?;
        let options = self.provider_select_options(&editor.working_draft(cx), select);
        let mut menu = div()
            .id("provider-select-menu")
            .w(bounds.size.width.max(px(132.)))
            .max_h(px(250.))
            .overflow_y_scroll()
            .track_scroll(&editor.select_scroll)
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(colors.line))
            .bg(rgba(colors.panel))
            .shadow(vec![shadow(0x1a304b, 0.16, 8., 24.)])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        for (index, option) in options.into_iter().enumerate() {
            let marker = match &option.choice {
                ProviderChoice::Model(id) => format!("mapping-option-{id}"),
                ProviderChoice::Manual => "mapping-option-manual".into(),
                ProviderChoice::Effort(id) => format!("effort-option-{id}"),
            };
            let check = if option.selected { "✓" } else { "" };
            let label = format!(
                "{} {} {check}",
                option.label,
                option.detail.as_deref().unwrap_or_default()
            );
            let enabled = option.enabled;
            let mut item = div()
                .id(SharedString::from(format!("provider-{marker}-{index}")))
                .min_h(px(30.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.))
                .px(px(9.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(rgba(if !enabled {
                    colors.muted
                } else if option.selected {
                    colors.link
                } else {
                    colors.label
                }))
                .bg(rgba(if cursor == index || option.selected {
                    colors.menu_selected_bg
                } else {
                    colors.panel
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .flex_1()
                        .child(div().truncate().child(option.label))
                        .children(option.detail.map(|detail| {
                            div()
                                .truncate()
                                .text_size(px(10.))
                                .text_color(rgba(colors.muted))
                                .child(detail)
                        })),
                )
                .child(div().flex_none().child(check));
            if enabled {
                let choice = option.choice;
                item = item
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgba(colors.menu_selected_bg)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_provider_option(select, &choice, window, cx);
                        cx.stop_propagation();
                    }));
            }
            menu = menu.child(mark_provider(item, marker, label, enabled));
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

fn official_context_picker_models<'a>(
    draft: &ProviderDraft,
    providers: &[Value],
    native_catalog: &'a [Value],
) -> Option<&'a [Value]> {
    if !draft.built_in
        || draft.fetched_metadata.is_some()
        || !is_official_endpoint(draft.endpoint.trim())
    {
        return None;
    }
    let id = draft.id.as_deref()?;
    let saved = providers.iter().find(|provider| provider["id"] == id)?;
    official(saved).then_some(native_catalog)
}

#[cfg(test)]
mod official_context_picker_tests {
    use super::{ProviderDraft, official_context_picker_models};
    use serde_json::json;

    #[test]
    fn official_editor_uses_native_catalog_until_endpoint_or_metadata_overrides_it() {
        let mut provider = json!({
            "id": "anthropic-official", "builtIn": true,
            "endpoint": "https://api.anthropic.com/",
        });
        let native = [json!({
            "id": "default", "contextWindow": 1_000_000, "maxContextWindow": 1_000_000,
        })];
        let mut draft = ProviderDraft::from_provider(&provider);
        let models = |draft: &ProviderDraft| {
            official_context_picker_models(draft, std::slice::from_ref(&provider), &native)
        };
        assert_eq!(models(&draft), Some(native.as_slice()));
        draft.endpoint = "https://proxy.example.invalid".into();
        assert_eq!(models(&draft), None);
        draft.endpoint = "https://api.anthropic.com".into();
        draft.fetched_metadata = Some(json!([]));
        assert_eq!(models(&draft), None);

        draft.fetched_metadata = None;
        provider["modelMetadata"] = json!([]);
        assert_eq!(
            official_context_picker_models(&draft, &[provider], &native),
            None
        );
    }
}

fn provider_editor_can_save(draft: &ProviderDraft) -> bool {
    let efforts_valid = if draft.is_codex() {
        draft.effort_supported(0, &draft.reasoning_effort)
    } else {
        draft
            .effort_mapping
            .iter()
            .enumerate()
            .all(|(index, effort)| draft.effort_supported(index, effort))
    };
    efforts_valid && draft.subscription_models_valid()
}

#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct ProviderEditorProbe {
    pub bounds: HashMap<String, Bounds<Pixels>>,
    pub labels: HashMap<String, String>,
    pub enabled: HashMap<String, bool>,
    pub context_mode: Option<String>,
}

#[cfg(debug_assertions)]
thread_local! {
    static PROVIDER_EDITOR_PROBE: RefCell<Option<ProviderEditorProbe>> = const { RefCell::new(None) };
}

#[cfg(debug_assertions)]
#[allow(dead_code)]
pub(crate) fn probe_provider_editor(draw: impl FnOnce()) -> ProviderEditorProbe {
    PROVIDER_EDITOR_PROBE.set(Some(ProviderEditorProbe::default()));
    draw();
    PROVIDER_EDITOR_PROBE.take().unwrap()
}

fn mark_provider<E: ParentElement + Styled>(
    element: E,
    marker: impl Into<String>,
    label: impl Into<String>,
    enabled: bool,
) -> E {
    #[cfg(debug_assertions)]
    if PROVIDER_EDITOR_PROBE.with_borrow(|probe| probe.is_some()) {
        let marker = marker.into();
        let label = label.into();
        return element.relative().child(
            canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    PROVIDER_EDITOR_PROBE.with_borrow_mut(|probe| {
                        if let Some(probe) = probe {
                            probe.bounds.insert(marker.clone(), bounds);
                            probe.labels.insert(marker.clone(), label.clone());
                            probe.enabled.insert(marker.clone(), enabled);
                        }
                    });
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        );
    }
    let _ = (marker, label, enabled);
    element
}

#[cfg(debug_assertions)]
impl SettingsShell {
    #[allow(dead_code)]
    pub(crate) fn probe_open_provider_value(
        &mut self,
        provider: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_edit_editor(provider, self.new_provider_focus.clone(), window, cx);
    }

    #[allow(dead_code)]
    pub(crate) fn probe_open_subscription_flow(
        &mut self,
        provider: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = SettingsPage::ClaudeCode;
        self.open_new_editor(window, cx);
        if let Some(editor) = &mut self.editor {
            editor.selection = Some(PickChoice::Subscription(provider));
        }
        self.editor_next(window, cx);
    }

    #[allow(dead_code)]
    pub(crate) fn probe_subscription_state(&self) -> Option<(&'static str, bool, bool, bool)> {
        let login = self.providers.subscription_login();
        self.editor
            .as_ref()
            .and_then(|editor| editor.subscription.as_ref())
            .map(|flow| {
                (
                    login.phase,
                    flow.risk_checked,
                    login.can_reopen,
                    login.system_browser_available,
                )
            })
    }

    #[allow(dead_code)]
    pub(crate) fn probe_subscription_text(&self) -> (String, String) {
        let login = self.providers.subscription_login();
        (
            login.phase.to_owned(),
            login.error.clone().unwrap_or_default(),
        )
    }

    #[allow(dead_code)]
    pub(crate) fn probe_apply_subscription_draft(
        &mut self,
        draft: ProviderDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ =
            self.apply_subscription_update(ProviderUpdate::SubscriptionDraft(draft), window, cx);
    }

    #[allow(dead_code)]
    pub(crate) fn probe_subscription_card_focus(
        &self,
        provider_id: &str,
        control: &str,
        window: &mut Window,
    ) {
        let controls = &self.card_controls[provider_id];
        window.focus(match control {
            "refresh" => &controls[2],
            "edit" => &controls[0],
            "relogin" => &controls[1],
            _ => panic!("Unknown subscription card control: {control}"),
        });
    }

    #[allow(dead_code)]
    pub(crate) fn probe_subscription_quota(&self, provider_id: &str) -> String {
        let kind = self
            .providers
            .providers()
            .iter()
            .find(|provider| provider["id"] == provider_id)
            .and_then(subscription_kind)
            .unwrap_or("codex");
        quota_text(
            kind,
            self.providers.subscription_details(provider_id),
            self.locale,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn probe_subscription_quota_title(&self, provider_id: &str) -> String {
        quota_title(
            self.providers.subscription_details(provider_id),
            self.locale,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn probe_subscription_settings_open(&self) -> bool {
        self.subscription_settings_open
    }

    #[allow(dead_code)]
    pub(crate) fn probe_compaction_state(
        &self,
    ) -> Option<(String, String, Option<String>, String, bool, bool)> {
        self.compaction.draft.as_ref().map(|draft| {
            (
                draft.mode.clone(),
                draft.effort.clone(),
                draft.provider_id.clone(),
                draft.model.clone(),
                self.compaction.manual_model,
                self.compaction.saved,
            )
        })
    }

    #[allow(dead_code)]
    pub(crate) fn probe_compaction_validation(&self) -> Option<&'static str> {
        let draft = self.compaction.draft.as_ref()?;
        compaction_validation(draft, self.providers.providers(), self.locale)
    }

    #[allow(dead_code)]
    pub(crate) fn probe_compaction_focus(&self, control: &str, window: &mut Window, cx: &App) {
        let focus = match control {
            "provider" => &self.compaction.focus[CompactionSelect::Provider.index()],
            "model" => &self.compaction.focus[CompactionSelect::Model.index()],
            "effort" => &self.compaction.focus[CompactionSelect::Effort.index()],
            "follow" => &self.compaction.mode_focus[0],
            "custom" => &self.compaction.mode_focus[1],
            "save" => &self.compaction.save_focus,
            "retry" => &self.compaction.retry_focus,
            "relogin" => &self.compaction.relogin_focus,
            "manual" => &self.compaction.manual_input.read(cx).focus,
            _ => panic!("Unknown compaction control: {control}"),
        };
        window.focus(focus);
    }

    #[allow(dead_code)]
    pub(crate) fn probe_compaction_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        self.select_compaction_mode(mode, cx);
    }

    #[allow(dead_code)]
    pub(crate) fn probe_compaction_save(&mut self, cx: &mut Context<Self>) {
        self.save_compaction_now(cx);
    }

    #[allow(dead_code)]
    pub(crate) fn probe_compaction_set_provider(
        &mut self,
        provider_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = &mut self.compaction.draft {
            draft.provider_id = Some(provider_id.to_owned());
            self.compaction.manual_model = false;
            let first = self
                .providers
                .providers()
                .iter()
                .find(|provider| provider["id"].as_str() == Some(provider_id))
                .and_then(|provider| model_options(provider).into_iter().next())
                .map(|option| option.value)
                .unwrap_or_default();
            draft.model = first;
            cx.notify();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn probe_compaction_set_model(&mut self, model: &str, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.compaction.draft {
            draft.model = model.to_owned();
            self.compaction.manual_model = false;
            cx.notify();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn probe_compaction_set_effort(&mut self, effort: &str, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.compaction.draft {
            draft.effort = effort.to_owned();
            cx.notify();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn probe_provider_editor_focus(&self, control: &str, window: &mut Window) {
        if control == "new" {
            window.focus(&self.new_provider_focus);
            return;
        }
        let focus = &self.editor.as_ref().expect("provider editor open").focus;
        let target = match control {
            "close" => &focus.close,
            "back" => &focus.back,
            "custom" => &focus.custom,
            "next" => &focus.next,
            "fetch" => &focus.fetch,
            "search" => &focus.model_search,
            "reveal" => &focus.reveal,
            "name" => &focus.name,
            "mapping-0" => &focus.mapping_choices[0],
            "mapping-1" => &focus.mapping_choices[1],
            "mapping-2" => &focus.mapping_choices[2],
            "manual-0" => &focus.mappings[0],
            "manual-1" => &focus.mappings[1],
            "mapping-list-0" => &focus.manual_links[0],
            "mapping-list-1" => &focus.manual_links[1],
            "subscription-risk" => &focus.subscription_risk,
            "subscription-continue" | "subscription-login" | "subscription-retry" => {
                &focus.subscription_primary
            }
            "subscription-cancel" => &focus.subscription_cancel,
            "subscription-reopen" => &focus.subscription_reopen,
            "subscription-system-browser" => &focus.subscription_system_browser,
            "effort-0" => &focus.efforts[0],
            "effort-1" => &focus.efforts[1],
            "context-512k" => &focus.context_modes[1],
            "context-maximum" => &focus.context_modes[2],
            "delete" => &focus.delete,
            "save" => &focus.save,
            "save-switch" => &focus.save_switch,
            "keep-editing" => &focus.keep_editing,
            "discard" => &focus.confirm_action,
            _ => panic!("Unknown provider editor control: {control}"),
        };
        window.focus(target);
    }
}

fn field_label(label: impl Into<SharedString>, colors: &EditorColors) -> Div {
    field_container(colors).child(div().child(label.into()))
}

fn field_container(colors: &EditorColors) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .mb(px(20.))
        .text_size(px(13.))
        .font_weight(FontWeight(600.))
        .text_color(rgba(colors.label))
}

fn provider_inline_error(text: &'static str, colors: &EditorColors) -> Div {
    div()
        .p(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(rgba(colors.error_line))
        .bg(rgba(colors.error_bg))
        .text_size(px(12.))
        .line_height(px(19.2))
        .text_color(rgba(colors.error_text))
        .child(text)
}

fn help_text(text: impl Into<SharedString>, colors: &EditorColors) -> Div {
    div()
        .text_size(px(11.))
        .line_height(px(18.15))
        .font_weight(FontWeight(400.))
        .text_color(rgba(colors.muted))
        .whitespace_normal()
        .child(text.into())
}

fn dialog_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    focus: &FocusHandle,
    keyboard_focus: bool,
    colors: &EditorColors,
    cx: &mut Context<SettingsShell>,
    activate: impl Fn(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>) + 'static,
) -> AnyElement {
    let base = div()
        .id(id)
        .min_h(px(42.))
        .flex()
        .items_center()
        .justify_center()
        .px(px(17.))
        .py(px(10.))
        .rounded(px(9.))
        .border_1()
        .text_size(px(13.))
        .font_weight(FontWeight(600.))
        .whitespace_nowrap()
        .child(label.into());
    let button = if primary {
        base.border_color(rgba(colors.primary))
            .text_color(rgb(0xffffff))
            .bg(rgba(colors.primary))
    } else {
        base.border_color(rgba(colors.line_strong))
            .text_color(rgba(colors.secondary_text))
            .bg(rgba(colors.panel))
    };
    let button = if !enabled {
        button
            .opacity(0.55)
            .cursor(CursorStyle::OperationNotAllowed)
    } else if primary {
        button
            .hover(move |style| style.bg(rgba(colors.primary_hover)))
            .cursor_pointer()
    } else {
        button
            .hover(move |style| style.bg(rgba(colors.card_hover_bg)))
            .cursor_pointer()
    };
    if enabled {
        wire_dialog_button(button, focus, 9., keyboard_focus, activate, cx).into_any_element()
    } else {
        button.into_any_element()
    }
}

fn link_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    focus: &FocusHandle,
    keyboard_focus: bool,
    ring: FocusRingStyle,
    colors: &EditorColors,
    cx: &mut Context<SettingsShell>,
    activate: impl Fn(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>) + 'static,
) -> AnyElement {
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .px(px(2.))
        .text_size(px(12.))
        .font_weight(FontWeight(600.))
        .text_color(rgba(colors.link))
        .cursor_pointer()
        .hover(move |style| style.text_color(rgba(colors.link_hover)))
        .child(label.into());
    wire_button_with_ring(button, focus, 4., keyboard_focus, ring, activate, cx).into_any_element()
}

fn confirm_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    focus: &FocusHandle,
    keyboard_focus: bool,
    colors: &EditorColors,
    cx: &mut Context<SettingsShell>,
    activate: impl Fn(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>) + 'static,
) -> AnyElement {
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(12.))
        .py(px(8.))
        .rounded(px(7.))
        .border_1()
        .border_color(rgba(colors.line_strong))
        .bg(rgba(colors.panel))
        .text_size(px(12.))
        .text_color(rgba(colors.label))
        .child(label.into());
    if enabled {
        let button = button
            .cursor_pointer()
            .hover(move |style| style.bg(rgba(colors.card_hover_bg)));
        wire_dialog_button(button, focus, 7., keyboard_focus, activate, cx).into_any_element()
    } else {
        button
            .opacity(0.55)
            .cursor(CursorStyle::OperationNotAllowed)
            .into_any_element()
    }
}

impl Render for SettingsShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport_width = f32::from(window.viewport_size().width);
        let palette = self.theme.effective().palette();
        let background = rgba(palette.workspace.canvas);
        #[cfg(debug_assertions)]
        LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
            if let Some(probe) = probe {
                probe.settings_canvas = Some(background);
            }
        });
        let hide_nav = viewport_width <= 680.;
        let (nav_width, pad_top, pad_x) = if hide_nav {
            (0., 18., 14.)
        } else if viewport_width <= 940. {
            (SETTINGS_NAV_WIDTH_NARROW, 21., 18.)
        } else {
            (SETTINGS_NAV_WIDTH, 24., 28.)
        };
        let nav = if hide_nav {
            None
        } else {
            Some(self.render_nav(window, cx).w(px(nav_width)))
        };
        let page = self.render_page(self.page, window, cx);
        let main = div()
            .id("settings-main-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(pad_x))
            .pt(px(pad_top))
            .pb(px(28.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(page);
        let mut surface = div().relative().w_full().h_full().flex().bg(background);
        if let Some(nav) = nav {
            surface = surface.child(nav);
        }
        surface = surface.child(main);
        // The fused Windows title bar occupies the first 48px of the stage.
        // Reserve it as padding so no settings control can slide under the
        // caption cluster; the drag strip itself is added below.
        #[cfg(target_os = "windows")]
        let surface = surface.pt(px(48.));
        let duration_ms = if reduced_motion() {
            1
        } else {
            SETTINGS_ANIMATION_MS
        };
        let surface = surface.with_animation(
            "settings-surface-in",
            Animation::new(Duration::from_millis(duration_ms)).with_easing(surface_easing),
            |surface, delta| surface.opacity(delta).top(px(3. * (1. - delta))),
        );
        let dialog = self.render_editor(window, cx);
        let compaction_menu = self.render_compaction_menu(window, cx);
        let menu = self.render_provider_select(cx);
        let root = div()
            .id("settings-shell")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(".AppleSystemUIFont")
            .text_color(rgba(palette.workspace.text))
            .bg(background)
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                if this.keyboard_focus {
                    this.keyboard_focus = false;
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "tab" && !this.keyboard_focus {
                    this.keyboard_focus = true;
                    cx.notify();
                }
            }))
            .child(surface);
        #[cfg(target_os = "windows")]
        let root = root.child(
            div()
                .id("settings-windows-drag")
                .absolute()
                .top_0()
                .left_0()
                .right(px(crate::windows_titlebar::WINDOWS_CAPTION_CONTROLS_WIDTH))
                .h(px(48.))
                .window_control_area(WindowControlArea::Drag)
                .occlude(),
        );
        root.children(dialog)
            .children(menu)
            .children(compaction_menu)
    }
}

// ---------------------------------------------------------------------------
// Language & time zone view (SET-08)
// ---------------------------------------------------------------------------
//
// This block is a projection only. The settings host owns the drafts, the
// saved baselines, the preference reads/writes, and the effective locale.
// `LanguageZoneView::english` is the language currently applied by the shell.
// Choosing a language radio emits `SelectLanguage`; the host saves it and feeds
// the saved value back, so this view has no language Save/Cancel/Restore row.
//
// The two blocks are independent. Each carries its own availability, busy,
// dirty, saved, and error state; a language save must not disable the time zone
// controls, and vice versa.
//
// Menu contract: the trigger and Escape/Tab/outside-click emit
// `ToggleTimeZoneMenu` (the host flips `time_zone_menu_open`). Choosing an
// option emits `SelectTimeZone`; the host updates the draft and closes the menu
// in that action. `Enter`/`Space` on the trigger opens the menu; while it is
// open, Up/Down/Home/End move the highlighted row, Enter/Space commits it, and
// Escape closes it. The highlighted row is view-local, like a native popup.
//
// Frozen copy is transcribed verbatim from
// `apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.tsx`,
// `ClaudeTimeZoneSettings.tsx`, `hierarchyShell.ts`, `common.ts`, and
// `claude-time-zone-settings.css`.

/// One block's status. `available` is false while the first read is pending or
/// the preference service is disconnected; `dirty` means the draft differs
/// from the saved value; `saved` is set only after a write is confirmed and the
/// draft still matches it; `busy` is one write in flight for this block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LanguageZoneBlock {
    pub available: bool,
    pub busy: bool,
    pub dirty: bool,
    pub saved: bool,
    pub error: LanguageZoneError,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LanguageZoneError {
    #[default]
    None,
    /// Settings service disconnected. Controls are disabled and Retry is shown.
    Unavailable,
    /// The saved time zone could not be read. Retry remains available.
    Read,
    /// The last write was not confirmed. The saved preference is unchanged.
    Save,
}

/// The user-visible choices emitted by the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LanguageZoneAction {
    SelectLanguage(String),
    SelectTimeZone(String),
    SaveTimeZone,
    CancelTimeZone,
    ResetTimeZone,
    RetryTimeZone,
    ToggleTimeZoneMenu,
}

/// The focus and popup state the host keeps for the lifetime of the settings
/// page. Build one with [`LanguageZoneFocus::new`] and pass a reference in each
/// [`LanguageZoneView`]. It is separate from `LanguageZoneView` because focus
/// handles and the menu scroll position must survive re-renders, while the
/// projection is rebuilt from host state.
pub struct LanguageZoneFocus {
    language_options: [FocusHandle; 3],
    time_zone_trigger: FocusHandle,
    time_zone_save: FocusHandle,
    time_zone_cancel: FocusHandle,
    time_zone_reset: FocusHandle,
    time_zone_retry: FocusHandle,
    menu_scroll: ScrollHandle,
    menu_active: Rc<Cell<usize>>,
    menu_open_seen: Rc<Cell<bool>>,
    trigger_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl LanguageZoneFocus {
    pub fn new(cx: &App) -> Self {
        let handle = || cx.focus_handle().tab_stop(true);
        Self {
            language_options: std::array::from_fn(|_| handle()),
            time_zone_trigger: handle(),
            time_zone_save: handle(),
            time_zone_cancel: handle(),
            time_zone_reset: handle(),
            time_zone_retry: handle(),
            menu_scroll: ScrollHandle::new(),
            menu_active: Rc::new(Cell::new(0)),
            menu_open_seen: Rc::new(Cell::new(false)),
            trigger_bounds: Rc::new(RefCell::new(None)),
        }
    }
}

/// One frame of the language and time zone blocks.
///
/// - `language` is the saved language preference (`system`, `zh-CN`, `en`).
/// - `time_zone` is the time zone draft (`auto`, `system`, or an IANA name).
/// - `system_time_zone` is the host zone used for `system` and as the fallback
///   effective zone for `auto`.
/// - `auto_time_zone` is the host-supplied automatic candidate.
/// - `zones` is the host-supplied IANA list after the two fixed entries.
/// - `preview` is the already formatted clock time. Format it with
///   [`LanguageZoneView::effective_time_zone`] so the suffix and the reading
///   agree.
/// - `forced_locale` is the `MATOU_LOCALE` override code (`en`, `zh-CN`), not a
///   display label.
/// - `keyboard_focus` mirrors the shell's keyboard-focus flag.
#[derive(Clone, Copy)]
pub struct LanguageZoneView<'a> {
    pub english: bool,
    pub language: &'a str,
    pub time_zone: &'a str,
    pub system_time_zone: &'a str,
    pub auto_time_zone: Option<&'a str>,
    pub zones: &'a [String],
    pub preview: Option<&'a str>,
    pub forced_locale: Option<&'a str>,
    pub language_block: LanguageZoneBlock,
    pub time_zone_block: LanguageZoneBlock,
    pub time_zone_menu_open: bool,
    pub time_zone_read_only: bool,
    pub keyboard_focus: bool,
    pub focus: &'a LanguageZoneFocus,
}

impl<'a> LanguageZoneView<'a> {
    /// The zone the frozen preview uses. Keep the host formatter and this view
    /// in sync by formatting the preview clock with this value.
    pub fn effective_time_zone(&self) -> &'a str {
        match self.time_zone {
            "auto" => self
                .auto_time_zone
                .filter(|zone| !zone.is_empty())
                .unwrap_or(self.system_time_zone),
            "system" => self.system_time_zone,
            zone => zone,
        }
    }
}

#[derive(Clone)]
struct TimeZoneChoice {
    value: String,
    label: String,
}

const VIEW_LANGUAGE_FOLLOW: &str = "跟随系统";
const VIEW_LANGUAGE_FOLLOW_EN: &str = "Follow system";
const VIEW_LANGUAGE_CHINESE: &str = "中文";
const VIEW_LANGUAGE_ENGLISH: &str = "English";
const VIEW_LANGUAGE_NOTE: &str = "界面立即切换；运行时消息在重启后切换";
const VIEW_LANGUAGE_NOTE_EN: &str =
    "The interface switches immediately; runtime messages switch after a restart";
const VIEW_TIME_ZONE_LABEL: &str = "时区";
const VIEW_TIME_ZONE_LABEL_EN: &str = "Time zone";
const VIEW_TIME_ZONE_HELP: &str = "自动：本机是大陆时区时，Claude Code、Codex 和 Antigravity 使用同为东八区的新加坡时区（出口在台湾时用台北），时间读数不变。";
const VIEW_TIME_ZONE_HELP_EN: &str = "Automatic: when this Mac uses a mainland China time zone, Claude Code, Codex and Antigravity use another UTC+8 zone (Singapore, or Taipei when your exit is in Taiwan), so clock times stay the same.";
const VIEW_TIME_ZONE_AUTO: &str = "自动（隐藏大陆时区，推荐）";
const VIEW_TIME_ZONE_AUTO_EN: &str = "Automatic (hide mainland China time zone, recommended)";
const VIEW_TIME_ZONE_PREVIEW: &str = "当前时间";
const VIEW_TIME_ZONE_PREVIEW_EN: &str = "Current time";
const VIEW_TIME_ZONE_SCOPE: &str = "仅影响码头启动的 Claude Code、Codex、Antigravity 及其子进程。保存后，新建、恢复和重启的会话自动使用该时区。正在运行的会话需重启后生效；保存不会中断任务。";
const VIEW_TIME_ZONE_SCOPE_EN: &str = "Applies only to Claude Code, Codex, Antigravity and their child processes launched by Matou. New, resumed and restarted sessions use the saved zone automatically. Running sessions need a restart; saving does not interrupt tasks.";
const VIEW_SAVE: &str = "保存";
const VIEW_SAVE_EN: &str = "Save";
const VIEW_SAVING: &str = "保存中…";
const VIEW_SAVING_EN: &str = "Saving…";
const VIEW_CANCEL: &str = "取消";
const VIEW_CANCEL_EN: &str = "Cancel";
const VIEW_RESTORE: &str = "恢复默认";
const VIEW_RESTORE_EN: &str = "Restore defaults";
const VIEW_RETRY: &str = "重试";
const VIEW_RETRY_EN: &str = "Retry";
const VIEW_TIME_ZONE_SAVED: &str = "已保存，下次启动 Claude Code 时自动生效。";
const VIEW_TIME_ZONE_SAVED_EN: &str =
    "Saved. Applies automatically on the next Claude Code launch.";
const VIEW_DISCONNECTED: &str = "设置服务未连接，请重试。";
const VIEW_DISCONNECTED_EN: &str = "Settings service is disconnected. Please retry.";
const VIEW_TIME_ZONE_READ: &str = "无法读取时区设置，请重试。";
const VIEW_TIME_ZONE_READ_EN: &str = "Could not load the time zone. Please retry.";
const VIEW_SAVE_UNCONFIRMED: &str = "保存未确认，请重试。";
const VIEW_SAVE_UNCONFIRMED_EN: &str = "Save was not confirmed. Please retry.";
const VIEW_TIME_ZONE_READ_ONLY: &str = "数据库只读";
const VIEW_TIME_ZONE_READ_ONLY_EN: &str = "The database is read-only.";
const VIEW_TIME_ZONE_MAX_WIDTH: f32 = 520.;
const VIEW_MENU_MIN_WIDTH: f32 = 240.;
const VIEW_MENU_MAX_HEIGHT: f32 = 300.;

fn language_zone_text<'a>(english: bool, zh: &'a str, en: &'a str) -> &'a str {
    if english { en } else { zh }
}

fn language_zone_locale_label(locale: &str) -> &str {
    match locale {
        "en" => VIEW_LANGUAGE_ENGLISH,
        "zh-CN" => VIEW_LANGUAGE_CHINESE,
        _ => locale,
    }
}

fn language_zone_error_text(error: LanguageZoneError, english: bool) -> Option<&'static str> {
    match error {
        LanguageZoneError::None => None,
        LanguageZoneError::Unavailable => Some(language_zone_text(
            english,
            VIEW_DISCONNECTED,
            VIEW_DISCONNECTED_EN,
        )),
        LanguageZoneError::Read => Some(language_zone_text(
            english,
            VIEW_TIME_ZONE_READ,
            VIEW_TIME_ZONE_READ_EN,
        )),
        LanguageZoneError::Save => Some(language_zone_text(
            english,
            VIEW_SAVE_UNCONFIRMED,
            VIEW_SAVE_UNCONFIRMED_EN,
        )),
    }
}

fn language_zone_id(mut element: Div, id: &'static str) -> Div {
    element.interactivity().element_id = Some(ElementId::Name(SharedString::from(id)));
    element
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WorkspaceSurfaceProbe {
    pub sidebar: Option<Rgba>,
    pub topbar: Option<Rgba>,
    pub footer: Option<Rgba>,
    pub card: Option<Rgba>,
    pub card_header: Option<Rgba>,
}

#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct LanguageZoneProbe {
    pub labels: Vec<String>,
    pub bounds: Vec<Bounds<Pixels>>,
    pub enabled: HashMap<&'static str, bool>,
    pub selected_language: String,
    pub settings_canvas: Option<Rgba>,
    pub workspace_canvas: Option<Rgba>,
    /// Actual surfaces painted by `workspace.rs` in the same frame.
    pub workspace_surfaces: WorkspaceSurfaceProbe,
    /// Background behind the language/time-zone block (its outer surface).
    pub language_zone_surface: Option<Rgba>,
    /// Background of the language options list card.
    pub language_zone_card: Option<Rgba>,
}

#[cfg(debug_assertions)]
thread_local! {
    static LANGUAGE_ZONE_PROBE: RefCell<Option<LanguageZoneProbe>> = const { RefCell::new(None) };
}

#[cfg(debug_assertions)]
#[allow(dead_code)]
pub(crate) fn probe_language_zone(draw: impl FnOnce()) -> LanguageZoneProbe {
    LANGUAGE_ZONE_PROBE.set(Some(LanguageZoneProbe::default()));
    draw();
    LANGUAGE_ZONE_PROBE.take().unwrap()
}

#[cfg(debug_assertions)]
pub(crate) fn probe_workspace_canvas(background: Rgba) {
    LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.workspace_canvas = Some(background);
        }
    });
}

/// Mirrors the surfaces `workspace.rs` actually paints so probes can compare
/// the workspace against the settings language zone without duplicating the
/// palette constants.
#[cfg(debug_assertions)]
pub(crate) fn probe_workspace_surfaces(record: impl FnOnce(&mut WorkspaceSurfaceProbe)) {
    LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            record(&mut probe.workspace_surfaces);
        }
    });
}

fn language_zone_label<T: AsRef<str>>(text: T) -> T {
    #[cfg(debug_assertions)]
    LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.labels.push(text.as_ref().to_owned());
        }
    });
    text
}

fn language_zone_status(id: &'static str, text: &'static str, palette: &ThemePalette) -> Div {
    language_zone_id(
        div()
            .min_w_0()
            .text_size(px(11.))
            .line_height(relative(1.5))
            .text_color(rgba(palette.workspace.muted))
            .whitespace_normal()
            .child(language_zone_label(text)),
        id,
    )
}

fn language_zone_radio(selected: bool, palette: &ThemePalette) -> Div {
    let mut radio = div()
        .flex_none()
        .size(px(13.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(rgba(if selected {
            palette.workspace.accent
        } else {
            palette.settings.language_radio_border
        }))
        .bg(rgba(if selected {
            palette.workspace.accent
        } else {
            palette.workspace.panel
        }));
    if selected {
        radio = radio.child(
            div()
                .size(px(5.))
                .rounded_full()
                .bg(rgba(palette.workspace.panel)),
        );
    }
    radio
}

fn language_zone_row_ring(
    focus: FocusHandle,
    keyboard_focus: bool,
    palette: &ThemePalette,
) -> AnyElement {
    let accent_line = palette.workspace.accent_line;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if keyboard_focus && focus.is_focused(window) {
                window.paint_quad(
                    outline(
                        bounds.inset(px(1.)),
                        rgba(accent_line),
                        BorderStyle::default(),
                    )
                    .corner_radii(px(0.))
                    .border_widths(px(2.)),
                );
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
    .into_any_element()
}

fn language_option_row(
    index: usize,
    value: &'static str,
    label: &'static str,
    selected: bool,
    enabled: bool,
    options: &[FocusHandle; 3],
    keyboard_focus: bool,
    on_action: Rc<dyn Fn(LanguageZoneAction, &mut Window, &mut App)>,
    palette: &ThemePalette,
) -> AnyElement {
    let palette = *palette;
    let focus = options[index].clone();
    let mut row = div()
        .id(SharedString::from(format!(
            "settings-language-option-{value}"
        )))
        .relative()
        .w_full()
        .h(px(42.))
        .flex()
        .items_center()
        .gap(px(9.))
        .px(px(13.))
        .text_size(px(10.))
        .font_weight(FontWeight(650.))
        .text_color(rgba(if selected {
            palette.workspace.accent
        } else {
            palette.settings.language_text
        }))
        .bg(rgba(if selected {
            palette.settings.language_selected
        } else {
            palette.settings.language_list
        }));
    if index < 2 {
        row = row.border_b_1().border_color(rgba(palette.workspace.line));
    }
    if selected {
        row = row.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(2.))
                .bg(rgba(palette.workspace.accent)),
        );
    }
    if enabled && !selected {
        row = row.hover(move |style| style.bg(rgba(palette.settings.language_hover)));
    }
    row = row
        .child(language_zone_radio(selected, &palette))
        .child(language_zone_label(label));

    if !enabled {
        return row.opacity(0.55).into_any_element();
    }

    let all_options = options.clone();
    let mouse_focus = focus.clone();
    let key_action = on_action.clone();
    let click_action = on_action;
    row.relative()
        .track_focus(&focus)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&mouse_focus);
            cx.stop_propagation();
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.modifiers.modified() {
                return;
            }
            match event.keystroke.key.as_str() {
                "enter" | "space" => {
                    cx.stop_propagation();
                    key_action(
                        LanguageZoneAction::SelectLanguage(value.to_owned()),
                        window,
                        cx,
                    );
                }
                "down" | "right" => {
                    cx.stop_propagation();
                    window.focus(&all_options[(index + 1) % all_options.len()]);
                }
                "up" | "left" => {
                    cx.stop_propagation();
                    window.focus(&all_options[(index + all_options.len() - 1) % all_options.len()]);
                }
                "home" => {
                    cx.stop_propagation();
                    window.focus(&all_options[0]);
                }
                "end" => {
                    cx.stop_propagation();
                    window.focus(&all_options[all_options.len() - 1]);
                }
                _ => {}
            }
        })
        .on_click(move |event, window, cx| {
            if matches!(event, ClickEvent::Keyboard(_)) {
                return;
            }
            cx.stop_propagation();
            click_action(
                LanguageZoneAction::SelectLanguage(value.to_owned()),
                window,
                cx,
            );
        })
        .child(language_zone_row_ring(focus, keyboard_focus, &palette))
        .into_any_element()
}

fn language_zone_action_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    enabled: bool,
    focus: FocusHandle,
    keyboard_focus: bool,
    on_action: Rc<dyn Fn(LanguageZoneAction, &mut Window, &mut App)>,
    action: LanguageZoneAction,
    palette: &ThemePalette,
) -> AnyElement {
    #[cfg(debug_assertions)]
    LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.enabled.insert(id, enabled);
        }
    });
    let palette = *palette;
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
        .child(language_zone_label(label));
    let button = if primary {
        base.border_color(rgba(palette.settings.language_primary))
            .text_color(rgb(0xffffff))
            .bg(rgba(palette.settings.language_primary))
    } else {
        base.border_color(rgba(palette.workspace.line_strong))
            .text_color(rgba(palette.workspace.text))
            .bg(rgba(palette.workspace.panel))
    };
    if !enabled {
        return button
            .opacity(0.5)
            .cursor(CursorStyle::Arrow)
            .into_any_element();
    }
    let button = if primary {
        button
            .hover(move |style| style.bg(rgba(palette.settings.language_primary_hover)))
            .cursor_pointer()
    } else {
        button
            .hover(move |style| {
                style
                    .border_color(rgba(palette.workspace.accent))
                    .text_color(rgba(palette.workspace.accent))
                    .bg(rgba(palette.settings.language_button_hover))
            })
            .cursor_pointer()
    };
    let key_action = action.clone();
    let click_action = action;
    let key_activate = on_action.clone();
    let click_activate = on_action;
    let mouse_focus = focus.clone();
    button
        .relative()
        .track_focus(&focus)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&mouse_focus);
            cx.stop_propagation();
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                && !event.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
                key_activate(key_action.clone(), window, cx);
            }
        })
        .on_click(move |event, window, cx| {
            if matches!(event, ClickEvent::Keyboard(_)) {
                return;
            }
            cx.stop_propagation();
            click_activate(click_action.clone(), window, cx);
        })
        .child(focus_ring_with(
            focus,
            7.,
            keyboard_focus,
            FocusRingStyle::Shell,
        ))
        .into_any_element()
}

fn language_zone_error_line(
    id: &'static str,
    retry_id: &'static str,
    text: &'static str,
    english: bool,
    retry: Option<(FocusHandle, LanguageZoneAction)>,
    retry_enabled: bool,
    keyboard_focus: bool,
    on_action: Rc<dyn Fn(LanguageZoneAction, &mut Window, &mut App)>,
    palette: &ThemePalette,
) -> Div {
    let mut line = div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap(px(8.))
        .mt(px(14.))
        .text_size(px(11.))
        .line_height(relative(1.55))
        .text_color(rgba(palette.settings.language_error))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .child(language_zone_label(text)),
        );
    if let Some((focus, action)) = retry {
        line = line.child(language_zone_action_button(
            retry_id,
            language_zone_text(english, VIEW_RETRY, VIEW_RETRY_EN),
            retry_id == "settings-time-zone-retry",
            retry_enabled,
            focus,
            keyboard_focus,
            on_action,
            action,
            palette,
        ));
    }
    language_zone_id(line, id)
}

fn language_zone_choices(view: &LanguageZoneView<'_>) -> Vec<TimeZoneChoice> {
    let english = view.english;
    let system_label = if view.system_time_zone.is_empty() {
        language_zone_text(english, VIEW_LANGUAGE_FOLLOW, VIEW_LANGUAGE_FOLLOW_EN).to_owned()
    } else {
        format!(
            "{} ({})",
            language_zone_text(english, VIEW_LANGUAGE_FOLLOW, VIEW_LANGUAGE_FOLLOW_EN),
            view.system_time_zone
        )
    };
    let mut choices = vec![
        TimeZoneChoice {
            value: "auto".to_owned(),
            label: language_zone_text(english, VIEW_TIME_ZONE_AUTO, VIEW_TIME_ZONE_AUTO_EN)
                .to_owned(),
        },
        TimeZoneChoice {
            value: "system".to_owned(),
            label: system_label,
        },
    ];
    let mut seen: HashSet<&str> = HashSet::new();
    seen.insert("auto");
    seen.insert("system");
    for zone in view.zones {
        if !seen.insert(zone.as_str()) {
            continue;
        }
        choices.push(TimeZoneChoice {
            value: zone.clone(),
            label: zone.clone(),
        });
    }
    choices
}

fn language_zone_selected_index(choices: &[TimeZoneChoice], time_zone: &str) -> usize {
    choices
        .iter()
        .position(|choice| choice.value.as_str() == time_zone)
        .unwrap_or(0)
}

fn language_zone_trigger_label(view: &LanguageZoneView<'_>, choices: &[TimeZoneChoice]) -> String {
    match view.time_zone {
        "auto" => choices
            .first()
            .map(|choice| choice.label.clone())
            .unwrap_or_default(),
        "system" => choices
            .get(1)
            .map(|choice| choice.label.clone())
            .unwrap_or_default(),
        zone => zone.to_owned(),
    }
}

fn language_zone_language_section(
    view: LanguageZoneView<'_>,
    on_action: Rc<dyn Fn(LanguageZoneAction, &mut Window, &mut App)>,
    palette: &ThemePalette,
) -> Div {
    let english = view.english;
    let block = view.language_block;
    let enabled = block.available && !block.busy;
    #[cfg(debug_assertions)]
    LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.selected_language = view.language.to_owned();
        }
    });
    let options: [(&'static str, &'static str); 3] = [
        (
            "system",
            language_zone_text(english, VIEW_LANGUAGE_FOLLOW, VIEW_LANGUAGE_FOLLOW_EN),
        ),
        ("zh-CN", VIEW_LANGUAGE_CHINESE),
        ("en", VIEW_LANGUAGE_ENGLISH),
    ];

    let background = rgba(palette.settings.language_list);
    #[cfg(debug_assertions)]
    LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.language_zone_card = Some(background);
        }
    });
    let mut card = div()
        .flex()
        .flex_col()
        .w_full()
        .overflow_hidden()
        .rounded(px(10.))
        .border_1()
        .border_color(rgba(palette.workspace.line))
        .bg(background)
        .shadow(vec![shadow(0x222d3c, 0.02, 1., 2.)]);
    for (index, (value, label)) in options.into_iter().enumerate() {
        card = card.child(language_option_row(
            index,
            value,
            label,
            view.language == value,
            enabled,
            &view.focus.language_options,
            view.keyboard_focus,
            on_action.clone(),
            palette,
        ));
    }
    card = language_zone_id(card, "settings-language-options");

    let mut section = div().flex().flex_col().w_full().mt(px(23.)).child(card);
    section = section.child(
        div()
            .id("settings-language-note")
            .mt(px(11.))
            .mx(px(2.))
            .text_size(px(9.))
            .line_height(relative(1.5))
            .text_color(rgba(palette.workspace.muted))
            .whitespace_normal()
            .child(language_zone_label(language_zone_text(
                english,
                VIEW_LANGUAGE_NOTE,
                VIEW_LANGUAGE_NOTE_EN,
            ))),
    );
    if let Some(forced) = view.forced_locale {
        let label = language_zone_locale_label(forced);
        let line = if english {
            format!("Currently fixed to {label} by the MATOU_LOCALE environment variable")
        } else {
            format!("当前由环境变量 MATOU_LOCALE 固定为 {label}")
        };
        section = section.child(
            div()
                .id("settings-language-forced")
                .mt(px(11.))
                .mx(px(2.))
                .text_size(px(9.))
                .line_height(relative(1.5))
                .text_color(rgba(palette.workspace.muted))
                .whitespace_normal()
                .child(language_zone_label(line)),
        );
    }

    if block.error == LanguageZoneError::Save {
        section = section.child(language_zone_error_line(
            "settings-language-error",
            "settings-language-retry",
            language_zone_text(english, VIEW_SAVE_UNCONFIRMED, VIEW_SAVE_UNCONFIRMED_EN),
            english,
            None,
            !block.busy,
            view.keyboard_focus,
            on_action,
            palette,
        ));
    }
    section
}

fn language_zone_time_zone_section(
    view: LanguageZoneView<'_>,
    choices: &[TimeZoneChoice],
    menu_open: bool,
    on_action: Rc<dyn Fn(LanguageZoneAction, &mut Window, &mut App)>,
    palette: &ThemePalette,
) -> Div {
    let english = view.english;
    let block = view.time_zone_block;
    // Read-only recovery has a saved zone to show but no writable preference
    // service behind it. Keep the value, preview and controls on screen, while
    // every write path (menu, save, cancel, restore defaults) is disabled and
    // explained by the frozen read-only sentence below.
    let read_only = view.time_zone_read_only;
    let enabled = block.available && !block.busy && !read_only;
    #[cfg(debug_assertions)]
    LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.enabled.insert("settings-time-zone-trigger", enabled);
            probe
                .enabled
                .insert("settings-time-zone-read-only", read_only);
        }
    });
    let trigger_label = language_zone_trigger_label(&view, choices);

    let trigger_focus = view.focus.time_zone_trigger.clone();
    let bounds_cell = view.focus.trigger_bounds.clone();
    let mut trigger = div()
        .id("settings-time-zone-trigger")
        .mt(px(12.))
        .w_full()
        .h(px(36.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.))
        .px(px(10.))
        .rounded(px(7.))
        .border_1()
        .border_color(rgba(if menu_open {
            palette.workspace.accent_line
        } else {
            palette.workspace.line_strong
        }))
        .bg(rgba(palette.workspace.panel))
        .text_size(px(12.))
        .text_color(rgba(palette.workspace.text))
        .child(
            div()
                .id("settings-time-zone-value")
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(language_zone_label(trigger_label)),
        )
        .child(icon("chevron-down", 12., palette.workspace.muted >> 8))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    *bounds_cell.borrow_mut() = Some(bounds);
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        );

    let mut section = div()
        .flex()
        .flex_col()
        .w_full()
        .max_w(px(VIEW_TIME_ZONE_MAX_WIDTH));
    section = section.child(
        div()
            .id("settings-time-zone-label")
            .mt(px(18.))
            .text_size(px(12.))
            .font_weight(FontWeight(650.))
            .text_color(rgba(palette.workspace.text))
            .child(language_zone_label(language_zone_text(
                english,
                VIEW_TIME_ZONE_LABEL,
                VIEW_TIME_ZONE_LABEL_EN,
            ))),
    );
    section = section.child(
        div()
            .id("settings-time-zone-help")
            .mt(px(10.))
            .text_size(px(11.))
            .line_height(relative(1.7))
            .text_color(rgba(palette.workspace.muted))
            .whitespace_normal()
            .child(language_zone_label(language_zone_text(
                english,
                VIEW_TIME_ZONE_HELP,
                VIEW_TIME_ZONE_HELP_EN,
            ))),
    );

    if enabled {
        let key_choices = choices.to_vec();
        let menu_active = view.focus.menu_active.clone();
        let menu_scroll = view.focus.menu_scroll.clone();
        let mouse_focus = trigger_focus.clone();
        let click_focus = trigger_focus.clone();
        let key_focus = trigger_focus.clone();
        let click_action = on_action.clone();
        let key_action = on_action.clone();
        trigger = trigger
            .relative()
            .track_focus(&trigger_focus)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&mouse_focus);
                cx.stop_propagation();
            })
            .on_click(move |event, window, cx| {
                if matches!(event, ClickEvent::Keyboard(_)) {
                    return;
                }
                cx.stop_propagation();
                window.focus(&click_focus);
                click_action(LanguageZoneAction::ToggleTimeZoneMenu, window, cx);
            })
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.modified() {
                    return;
                }
                let key = event.keystroke.key.as_str();
                if menu_open {
                    if key_choices.is_empty() {
                        return;
                    }
                    match key {
                        "escape" => {
                            cx.stop_propagation();
                            window.focus(&key_focus);
                            key_action(LanguageZoneAction::ToggleTimeZoneMenu, window, cx);
                        }
                        "tab" => {
                            key_action(LanguageZoneAction::ToggleTimeZoneMenu, window, cx);
                        }
                        "down" => {
                            cx.stop_propagation();
                            let next = (menu_active.get() + 1) % key_choices.len();
                            menu_active.set(next);
                            menu_scroll.scroll_to_item(next);
                            window.refresh();
                        }
                        "up" => {
                            cx.stop_propagation();
                            let len = key_choices.len();
                            let next = (menu_active.get() + len - 1) % len;
                            menu_active.set(next);
                            menu_scroll.scroll_to_item(next);
                            window.refresh();
                        }
                        "home" => {
                            cx.stop_propagation();
                            menu_active.set(0);
                            menu_scroll.scroll_to_item(0);
                            window.refresh();
                        }
                        "end" => {
                            cx.stop_propagation();
                            let last = key_choices.len() - 1;
                            menu_active.set(last);
                            menu_scroll.scroll_to_item(last);
                            window.refresh();
                        }
                        "enter" | "space" => {
                            cx.stop_propagation();
                            let index = menu_active.get().min(key_choices.len() - 1);
                            let value = key_choices[index].value.clone();
                            window.focus(&key_focus);
                            key_action(LanguageZoneAction::SelectTimeZone(value), window, cx);
                        }
                        _ => {
                            cx.stop_propagation();
                        }
                    }
                } else if matches!(key, "enter" | "space" | "down" | "up") {
                    cx.stop_propagation();
                    window.focus(&key_focus);
                    key_action(LanguageZoneAction::ToggleTimeZoneMenu, window, cx);
                }
            })
            .child(focus_ring_with(
                trigger_focus,
                7.,
                view.keyboard_focus,
                FocusRingStyle::Shell,
            ));
    } else {
        trigger = trigger.opacity(0.55).cursor(CursorStyle::Arrow);
    }
    section = section.child(trigger);

    if let Some(preview) = view.preview.filter(|value| !value.is_empty()) {
        let effective = view.effective_time_zone();
        let line = if effective.is_empty() {
            format!(
                "{}：{}",
                language_zone_text(english, VIEW_TIME_ZONE_PREVIEW, VIEW_TIME_ZONE_PREVIEW_EN),
                preview
            )
        } else {
            format!(
                "{}：{} · {}",
                language_zone_text(english, VIEW_TIME_ZONE_PREVIEW, VIEW_TIME_ZONE_PREVIEW_EN),
                preview,
                effective
            )
        };
        section = section.child(
            div()
                .id("settings-time-zone-preview")
                .mt(px(8.))
                .text_size(px(11.))
                .line_height(relative(1.7))
                .text_color(rgba(palette.workspace.muted))
                .whitespace_normal()
                .child(language_zone_label(line)),
        );
    }
    section = section.child(
        div()
            .id("settings-time-zone-scope")
            .mt(px(8.))
            .text_size(px(11.))
            .line_height(relative(1.7))
            .text_color(rgba(palette.workspace.muted))
            .whitespace_normal()
            .child(language_zone_label(language_zone_text(
                english,
                VIEW_TIME_ZONE_SCOPE,
                VIEW_TIME_ZONE_SCOPE_EN,
            ))),
    );

    let dirty = block.dirty;
    let can_save = enabled && dirty;
    let can_cancel = enabled && dirty;
    let can_reset = enabled && (view.time_zone != "auto" || dirty);
    let mut actions = div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap(px(12.))
        .mt(px(18.))
        .child(language_zone_action_button(
            "settings-time-zone-reset",
            language_zone_text(english, VIEW_RESTORE, VIEW_RESTORE_EN),
            false,
            can_reset,
            view.focus.time_zone_reset.clone(),
            view.keyboard_focus,
            on_action.clone(),
            LanguageZoneAction::ResetTimeZone,
            palette,
        ))
        .child(language_zone_action_button(
            "settings-time-zone-cancel",
            language_zone_text(english, VIEW_CANCEL, VIEW_CANCEL_EN),
            false,
            can_cancel,
            view.focus.time_zone_cancel.clone(),
            view.keyboard_focus,
            on_action.clone(),
            LanguageZoneAction::CancelTimeZone,
            palette,
        ))
        .child(language_zone_action_button(
            "settings-time-zone-save",
            if block.busy {
                language_zone_text(english, VIEW_SAVING, VIEW_SAVING_EN)
            } else {
                language_zone_text(english, VIEW_SAVE, VIEW_SAVE_EN)
            },
            true,
            can_save,
            view.focus.time_zone_save.clone(),
            view.keyboard_focus,
            on_action.clone(),
            LanguageZoneAction::SaveTimeZone,
            palette,
        ));
    if block.available && block.saved && !dirty && !block.busy {
        actions = actions.child(language_zone_status(
            "settings-time-zone-status",
            language_zone_text(english, VIEW_TIME_ZONE_SAVED, VIEW_TIME_ZONE_SAVED_EN),
            palette,
        ));
    }
    section = section.child(actions);

    if read_only {
        // The host may still carry the read failure that produced the read-only
        // state. Show the actual cause (database read-only) instead of the
        // misleading disconnected sentence.
        section = section.child(
            language_zone_status(
                "settings-time-zone-read-only",
                language_zone_text(
                    english,
                    VIEW_TIME_ZONE_READ_ONLY,
                    VIEW_TIME_ZONE_READ_ONLY_EN,
                ),
                palette,
            )
            .mt(px(14.)),
        );
    }
    if (!read_only || block.error == LanguageZoneError::Read)
        && let Some(text) = language_zone_error_text(block.error, english)
    {
        let retryable = matches!(
            block.error,
            LanguageZoneError::Unavailable | LanguageZoneError::Read
        );
        let retry = retryable.then(|| {
            (
                view.focus.time_zone_retry.clone(),
                LanguageZoneAction::RetryTimeZone,
            )
        });
        section = section.child(language_zone_error_line(
            "settings-time-zone-error",
            "settings-time-zone-retry",
            text,
            english,
            retry,
            !block.busy,
            view.keyboard_focus,
            on_action,
            palette,
        ));
    }
    section
}

fn language_zone_menu_backdrop(
    window: &Window,
    trigger_focus: FocusHandle,
    on_action: Rc<dyn Fn(LanguageZoneAction, &mut Window, &mut App)>,
) -> AnyElement {
    let viewport = window.viewport_size();
    deferred(
        anchored().position(point(px(0.), px(0.))).child(
            div()
                .id("settings-time-zone-menu-backdrop")
                .w(viewport.width)
                .h(viewport.height)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    window.focus(&trigger_focus);
                    cx.stop_propagation();
                    on_action(LanguageZoneAction::ToggleTimeZoneMenu, window, cx);
                })
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
        ),
    )
    .with_priority(100)
    .into_any_element()
}

fn language_zone_menu(
    choices: &[TimeZoneChoice],
    selected_value: &str,
    active: usize,
    bounds: Bounds<Pixels>,
    viewport: Size<Pixels>,
    menu_scroll: &ScrollHandle,
    trigger_focus: FocusHandle,
    on_action: Rc<dyn Fn(LanguageZoneAction, &mut Window, &mut App)>,
    palette: &ThemePalette,
) -> AnyElement {
    let palette = *palette;
    let width = f32::from(bounds.size.width)
        .max(VIEW_MENU_MIN_WIDTH)
        .min((f32::from(viewport.width) - 16.).max(160.));
    let max_height = (f32::from(viewport.height) - 16.)
        .min(VIEW_MENU_MAX_HEIGHT)
        .max(96.);
    let mut menu = div()
        .id("settings-time-zone-menu")
        .w(px(width))
        .max_h(px(max_height))
        .flex()
        .flex_col()
        .p(px(4.))
        .rounded(px(8.))
        .border_1()
        .border_color(rgba(palette.workspace.line))
        .bg(rgba(palette.workspace.panel))
        .shadow(vec![shadow(0x1a304b, 0.16, 8., 24.)])
        .overflow_y_scroll()
        .track_scroll(menu_scroll)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

    for (index, choice) in choices.iter().enumerate() {
        let selected = choice.value.as_str() == selected_value;
        let is_active = index == active;
        let background = if selected {
            palette.settings.language_menu_background
        } else if is_active {
            palette.settings.language_menu_active_background
        } else {
            palette.workspace.panel
        };
        let text_color = if selected {
            palette.settings.language_menu_text
        } else {
            palette.workspace.text
        };
        let value = choice.value.clone();
        let item_focus = trigger_focus.clone();
        let item_action = on_action.clone();
        menu = menu.child(
            div()
                .id(SharedString::from(format!(
                    "settings-time-zone-option-{index}"
                )))
                .flex_none()
                .h(px(32.))
                .flex()
                .items_center()
                .px(px(9.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(rgba(text_color))
                .bg(rgba(background))
                .hover(move |style| style.bg(rgba(palette.settings.language_menu_background)))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |event, window, cx| {
                    if matches!(event, ClickEvent::Keyboard(_)) {
                        return;
                    }
                    cx.stop_propagation();
                    window.focus(&item_focus);
                    item_action(
                        LanguageZoneAction::SelectTimeZone(value.clone()),
                        window,
                        cx,
                    );
                })
                .child(language_zone_label(choice.label.clone())),
        );
    }

    deferred(
        anchored()
            .position(point(bounds.left(), bounds.bottom() + px(4.)))
            .snap_to_window_with_margin(px(8.))
            .child(menu),
    )
    .with_priority(101)
    .into_any_element()
}

/// Render the language and time zone blocks. The caller owns all preference
/// state and applies the returned actions; this function only draws the frozen
/// controls and reports user intent.
///
/// The actual signature adds `window` before `on_action` because the popup's
/// backdrop and maximum width need the current viewport. `&Window` is read-only;
/// the action callback receives the live `&mut Window`.
pub fn language_zone_content(
    view: LanguageZoneView<'_>,
    window: &Window,
    on_action: Rc<dyn Fn(LanguageZoneAction, &mut Window, &mut App)>,
    palette: &ThemePalette,
    surface: Rgba,
) -> Div {
    #[cfg(debug_assertions)]
    LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.language_zone_surface = Some(surface);
        }
    });
    #[cfg(not(debug_assertions))]
    let _ = surface;
    let choices = language_zone_choices(&view);
    let menu_open = view.time_zone_menu_open
        && view.time_zone_block.available
        && !view.time_zone_block.busy
        && !view.time_zone_read_only;
    let active = if menu_open {
        if !view.focus.menu_open_seen.replace(true) {
            let index = language_zone_selected_index(&choices, view.time_zone);
            view.focus.menu_active.set(index);
            view.focus.menu_scroll.scroll_to_item(index);
        }
        let index = view
            .focus
            .menu_active
            .get()
            .min(choices.len().saturating_sub(1));
        view.focus.menu_active.set(index);
        index
    } else {
        view.focus.menu_open_seen.set(false);
        0
    };

    let language = language_zone_language_section(view, on_action.clone(), palette);
    let divider = div()
        .w_full()
        .mt(px(24.))
        .pt(px(6.))
        .border_t_1()
        .border_color(rgba(palette.workspace.line));
    let time_zone =
        language_zone_time_zone_section(view, &choices, menu_open, on_action.clone(), palette);

    let mut root = div().flex().flex_col().w_full();
    root = language_zone_id(root, "settings-language-zone");
    root = root.child(language).child(divider).child(time_zone);
    #[cfg(debug_assertions)]
    if LANGUAGE_ZONE_PROBE.with_borrow(|probe| probe.is_some()) {
        root = root.on_children_prepainted(|bounds, _, _| {
            LANGUAGE_ZONE_PROBE.with_borrow_mut(|probe| {
                if let Some(probe) = probe {
                    probe.bounds = bounds;
                }
            });
        });
    }

    if menu_open {
        if let Some(bounds) = (*view.focus.trigger_bounds.borrow()).clone() {
            root = root.child(language_zone_menu_backdrop(
                window,
                view.focus.time_zone_trigger.clone(),
                on_action.clone(),
            ));
            root = root.child(language_zone_menu(
                &choices,
                view.time_zone,
                active,
                bounds,
                window.viewport_size(),
                &view.focus.menu_scroll,
                view.focus.time_zone_trigger.clone(),
                on_action,
                palette,
            ));
        }
    }
    root
}

#[cfg(target_os = "macos")]
fn format_time_zone_preview(zone: &str, english: bool) -> Option<String> {
    use std::ffi::{CStr, CString, c_char, c_void};

    type CF = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFDateFormatterTimeZone: CF;
        fn CFAbsoluteTimeGetCurrent() -> f64;
        fn CFDateFormatterCreate(
            allocator: CF,
            locale: CF,
            date_style: isize,
            time_style: isize,
        ) -> CF;
        fn CFDateFormatterSetFormat(formatter: CF, format: CF);
        fn CFDateFormatterSetProperty(formatter: CF, key: CF, value: CF);
        fn CFDateFormatterCreateStringWithAbsoluteTime(
            allocator: CF,
            formatter: CF,
            time: f64,
        ) -> CF;
        fn CFLocaleCreate(allocator: CF, identifier: CF) -> CF;
        fn CFStringCreateWithCString(allocator: CF, string: *const c_char, encoding: u32) -> CF;
        fn CFStringGetCString(string: CF, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFTimeZoneCreateWithName(allocator: CF, name: CF, try_abbreviation: u8) -> CF;
        fn CFRelease(value: CF);
    }

    struct Owned(CF);
    impl Owned {
        fn new(pointer: CF) -> Option<Self> {
            if pointer.is_null() {
                None
            } else {
                Some(Self(pointer))
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // This wrapper owns the reference returned by a Create function.
                unsafe { CFRelease(self.0) };
            }
        }
    }

    let allocator = std::ptr::null();
    let make_string =
        |s: &CStr| Owned::new(unsafe { CFStringCreateWithCString(allocator, s.as_ptr(), UTF8) });
    let zone_name = make_string(&CString::new(zone).ok()?)?;
    let locale_id = make_string(if english { c"en_US" } else { c"zh_CN" })?;
    let format = make_string(if english {
        c"MMM d, HH:mm"
    } else {
        c"M月d日 HH:mm"
    })?;
    // Foundation formats the specified zone without modifying the process TZ.
    unsafe {
        let timezone = Owned::new(CFTimeZoneCreateWithName(allocator, zone_name.0, 0))?;
        let locale = Owned::new(CFLocaleCreate(allocator, locale_id.0))?;
        let formatter = Owned::new(CFDateFormatterCreate(allocator, locale.0, 0, 0))?;
        CFDateFormatterSetFormat(formatter.0, format.0);
        CFDateFormatterSetProperty(formatter.0, kCFDateFormatterTimeZone, timezone.0);
        let value = Owned::new(CFDateFormatterCreateStringWithAbsoluteTime(
            allocator,
            formatter.0,
            CFAbsoluteTimeGetCurrent(),
        ))?;
        let mut buffer = [0; 128];
        (CFStringGetCString(value.0, buffer.as_mut_ptr(), 128, UTF8) != 0).then(|| {
            CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned()
        })
    }
}

#[cfg(not(target_os = "macos"))]
fn format_time_zone_preview(_zone: &str, _english: bool) -> Option<String> {
    None
}
