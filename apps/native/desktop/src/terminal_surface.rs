use std::{
    ops::Range,
    path::PathBuf,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use core_graphics::{
    event::{CGEventType, CGMouseButton},
    event_source::CGEventSourceStateID,
};
use gpui::{
    App, Bounds, ContentMask, Context, CursorStyle as PointerStyle, DispatchPhase,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, ExternalPaths, FocusHandle,
    Font, FontStyle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Pixels, Point, Render, ScrollWheelEvent, ShapedLine, SharedString,
    StatefulInteractiveElement, Styled, Task, TextRun, UTF16Selection, Window, canvas, div, fill,
    font, point, prelude::FluentBuilder, px, rgb, rgba, size,
};
use matou_native_core::sessions::InterfaceLocale;
use matou_native_terminal::{
    Cell, Color, CursorStyle, GridSelection, InputModes, MouseEncoding, MouseEvent, MouseProtocol,
    MouseReporter, MouseWheel, Screen, SelectionAction, SelectionMode, Theme,
};
use serde_json::Value;

#[path = "session_history_seam.rs"]
pub mod session_history_seam;
use session_history_seam::{
    Action as HistoryAction, Binding as HistoryBinding, FALSE_START, HistoryStyle, HistoryView,
    INPUT_ROWS, Page as HistoryPage, READ_TIMEOUT, WHEEL_SETTLE, WheelProbe, fingerprint,
};

use crate::terminal_input::{
    Composition, byte_from_utf16, read_terminal_paste, selection_scroll_amount, terminal_key,
    utf16_from_byte,
};
use crate::terminal_scrollbar::ScrollbarMetrics;

pub const DEFAULT_TERMINAL_FONT_SIZE: u8 = 11;
pub const MIN_TERMINAL_FONT_SIZE: u8 = 10;
pub const MAX_TERMINAL_FONT_SIZE: u8 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontZoom {
    In,
    Out,
    Reset,
}
pub fn terminal_mod(modifiers: gpui::Modifiers, is_mac: bool) -> bool {
    modifiers.platform == is_mac && modifiers.control != is_mac && !modifiers.function
}

pub fn font_zoom_for_key(stroke: &gpui::Keystroke, is_mac: bool) -> Option<FontZoom> {
    let mods = stroke.modifiers;
    if !terminal_mod(mods, is_mac) || mods.alt {
        return None;
    }
    match stroke.key.as_str() {
        "=" | "+" => Some(FontZoom::In),
        "-" | "_" => Some(FontZoom::Out),
        "0" if !mods.shift => Some(FontZoom::Reset),
        _ => None,
    }
}

fn font_zoom_for_wheel(
    delta: gpui::ScrollDelta,
    modifiers: gpui::Modifiers,
    is_mac: bool,
) -> Option<FontZoom> {
    let gpui::ScrollDelta::Lines(delta) = delta else {
        return None;
    };
    if !terminal_mod(modifiers, is_mac) || modifiers.alt || delta.y == 0. {
        return None;
    }
    Some(if delta.y > 0. {
        FontZoom::In
    } else {
        FontZoom::Out
    })
}

const DEFAULT_VERTICAL_INSET: Pixels = px(14.);
const CANVAS_VERTICAL_INSET: Pixels = px(12.);
const BLINK_INTERVAL: Duration = Duration::from_millis(500);
const RESIZE_FRAME_QUIET: Duration = Duration::from_millis(100);
const RESIZE_FRAME_LIMIT: Duration = Duration::from_millis(500);
const FAMILIES: &[&str] = &["SF Mono", "Monaco", "Menlo", "Consolas", "Courier New"];

fn quote_dropped_path(path: &str, windows: bool) -> String {
    if path.is_empty() || path.contains('\0') {
        return String::new();
    }

    let ordinary = |byte: u8| {
        byte.is_ascii_alphanumeric() || b"_./:@%+=,-".contains(&byte) || (windows && byte == b'\\')
    };
    if !path.starts_with('=') {
        if path.bytes().all(ordinary) {
            return path.to_owned();
        }
        if !path.starts_with(' ')
            && !path.ends_with(' ')
            && path.bytes().all(|byte| byte == b' ' || ordinary(byte))
        {
            return format!("\"{path}\"");
        }
    }

    let escaped_quote = if windows { "''" } else { "'\\''" };
    format!("'{}'", path.replace('\'', escaped_quote))
}

fn format_dropped_paths(paths: &[PathBuf], windows: bool) -> String {
    paths
        .iter()
        .filter_map(|path| path.to_str())
        .map(|path| quote_dropped_path(path, windows))
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(all(target_os = "macos", debug_assertions))]
thread_local! {
    static BACKGROUND_PROBE: std::cell::Cell<Option<Option<u32>>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(all(target_os = "macos", debug_assertions))]
pub(crate) fn probe_terminal_background(draw: impl FnOnce()) -> Option<u32> {
    BACKGROUND_PROBE.set(Some(None));
    draw();
    BACKGROUND_PROBE.take().flatten()
}

type LinkHandler = dyn Fn(&str, &mut Window, &mut App);

pub struct SurfaceEvent {
    pub generation: u64,
    pub kind: SurfaceEventKind,
}
pub enum SurfaceEventKind {
    Input(Vec<u8>),
    SessionHistory(HistoryPage),
    FontZoom(FontZoom),
    Select(SelectionAction),
    Copy {
        interrupt_if_empty: bool,
    },
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    /// Positive lines move into scrollback.
    Scroll(i32),
    /// Absolute first visible history row; applied by the worker in FIFO order.
    ScrollTo {
        row: u64,
        request_id: u64,
    },
    ScrollSelection(i32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Dimensions {
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
}

#[derive(Clone, Copy)]
struct Metrics {
    width: Pixels,
    height: Pixels,
}

fn page_scroll_lines(key: &str, shift: bool, rows: usize) -> Option<f64> {
    if !shift {
        return None;
    }
    let page = rows.saturating_sub(1) as f64;
    match key {
        "pageup" => Some(-page),
        "pagedown" => Some(page),
        _ => None,
    }
}

fn wheel_scroll_pixels(delta: gpui::ScrollDelta, alt: bool) -> f64 {
    (match delta {
        gpui::ScrollDelta::Pixels(delta) => f64::from(f32::from(delta.y)) * (50. / 40.),
        gpui::ScrollDelta::Lines(delta) => f64::from(delta.y) * 50.,
    }) * if alt { 5. } else { 1. }
}

fn accumulate_wheel_pixels(remainder: &mut f64, delta: f64) -> f64 {
    if delta != 0. && remainder.signum() != delta.signum() {
        *remainder = 0.;
    }
    let total = *remainder + delta;
    let whole_pixels = total.trunc();
    let nearest_pixel = total.round();
    // GPUI supplies f32 deltas, so a whole pixel can land just below the threshold.
    if nearest_pixel != whole_pixels && (total - nearest_pixel).abs() <= f64::from(f32::EPSILON) {
        *remainder = 0.;
        return nearest_pixel;
    }
    *remainder = total - whole_pixels;
    whole_pixels
}

#[cfg(test)]
mod scroll_tests {
    use super::*;

    #[test]
    fn wheel_uses_the_same_pixel_scale_for_line_and_precise_events() {
        let notched = gpui::ScrollDelta::Lines(point(0., -1.));
        let precise = gpui::ScrollDelta::Pixels(point(px(0.), px(-4.)));
        assert_eq!(wheel_scroll_pixels(notched, false), -50.);
        assert_eq!(wheel_scroll_pixels(notched, true), -250.);
        assert_eq!(wheel_scroll_pixels(precise, false), -5.);
        assert_eq!(wheel_scroll_pixels(precise, true), -25.);
        assert_eq!(
            wheel_scroll_pixels(gpui::ScrollDelta::Pixels(point(px(0.), px(1.))), false),
            1.25
        );
    }

    #[test]
    fn wheel_accumulates_small_precise_events_before_scrolling() {
        for (raw, expected) in [(-0.4, -5.), (-0.32, -4.)] {
            let delta =
                wheel_scroll_pixels(gpui::ScrollDelta::Pixels(point(px(0.), px(raw))), false);
            let mut remainder = 0.;
            assert_eq!(accumulate_wheel_pixels(&mut remainder, delta), 0.);
            let pixels: f64 = (1..10)
                .map(|_| accumulate_wheel_pixels(&mut remainder, delta))
                .sum();
            assert_eq!(pixels, expected);
        }
    }

    #[test]
    fn wheel_reversal_discards_the_previous_direction_remainder() {
        for direction in [-1., 1.] {
            let mut remainder = 0.;
            assert_eq!(
                accumulate_wheel_pixels(&mut remainder, direction * 0.75),
                0.
            );
            assert_eq!(accumulate_wheel_pixels(&mut remainder, 0.), 0.);
            assert_eq!(remainder, direction * 0.75);
            assert_eq!(
                accumulate_wheel_pixels(&mut remainder, direction * -0.5),
                0.
            );
            assert_eq!(remainder, direction * -0.5);
            assert_eq!(
                accumulate_wheel_pixels(&mut remainder, direction * -0.5),
                -direction
            );
            assert_eq!(remainder, 0.);
        }
    }

    #[test]
    fn only_shift_page_keys_request_a_local_page() {
        let modes = InputModes {
            application_cursor_keys: false,
            bracketed_paste: false,
            input_epoch: 0,
            mouse_protocol: MouseProtocol::None,
            mouse_encoding: MouseEncoding::Default,
        };
        for (key, lines, bytes) in [("pageup", -19., b"\x1b[5~"), ("pagedown", 19., b"\x1b[6~")] {
            assert_eq!(page_scroll_lines(key, true, 20), Some(lines));
            assert_eq!(page_scroll_lines(key, true, 1), Some(0.));
            assert_eq!(page_scroll_lines(key, false, 20), None);
            assert_eq!(
                terminal_key(key, false, false, false, modes).as_deref(),
                Some(bytes.as_slice()),
            );
        }
    }
}

impl Metrics {
    fn dimensions(self, bounds: Bounds<Pixels>, scale: f32) -> Dimensions {
        Dimensions {
            cols: ((f32::from(bounds.size.width).floor() - 14.) / f32::from(self.width))
                .floor()
                .max(2.) as u16,
            rows: f32::from(bounds.size.height / self.height).floor().max(1.) as u16,
            cell_width_px: (f32::from(self.width) * scale).round().max(1.) as u32,
            cell_height_px: (f32::from(self.height) * scale).round().max(1.) as u32,
        }
    }

    fn cell(self, origin: Point<Pixels>, col: usize, row: usize, width: usize) -> Bounds<Pixels> {
        Bounds::new(
            point(
                origin.x + self.width * col as f32,
                origin.y + self.height * row as f32,
            ),
            size(self.width * width as f32, self.height),
        )
    }
}

#[derive(Clone, Copy)]
struct VisibleGrid {
    bounds: Bounds<Pixels>,
    cols: usize,
    rows: usize,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn CGEventSourceButtonState(source: CGEventSourceStateID, button: CGMouseButton) -> bool;
    fn CGEventSourceCounterForEventType(source: CGEventSourceStateID, event: CGEventType) -> u32;
}

#[derive(Clone, Copy)]
struct ScrollbarDrag {
    metrics: ScrollbarMetrics,
    pointer: Point<Pixels>,
    last_pointer: Point<Pixels>,
    #[cfg(target_os = "macos")]
    left_release_count: u32,
}

impl ScrollbarDrag {
    fn button_still_held(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            // A new press outside this window cannot revive an old gesture.
            let pressed = unsafe {
                CGEventSourceButtonState(
                    CGEventSourceStateID::CombinedSessionState,
                    CGMouseButton::Left,
                )
            };
            pressed && Self::left_button_release_count() == self.left_release_count
        }
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }

    #[cfg(target_os = "macos")]
    fn left_button_release_count() -> u32 {
        unsafe {
            CGEventSourceCounterForEventType(
                CGEventSourceStateID::CombinedSessionState,
                CGEventType::LeftMouseUp,
            )
        }
    }
}

#[derive(Clone, Copy)]
struct ScrollbarPosition {
    pixels: f64,
    row: u64,
    request_id: u64,
}

struct ScrollbarFade {
    start: Instant,
    from: f32,
    target: f32,
    duration: Duration,
}

impl ScrollbarFade {
    fn opacity(&self, now: Instant) -> f32 {
        let progress =
            (now.duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32()).min(1.);
        self.from + (self.target - self.from) * progress
    }
}

struct ResizeFrame {
    screen: Screen,
    selection: Option<GridSelection>,
    scroll_request_id: u64,
}

struct ResizeFrames {
    limit: Instant,
    present_at: Instant,
    latest: Option<ResizeFrame>,
    timer: Option<Task<()>>,
}

pub struct TerminalSurface {
    transport_generation: u64,
    transport_suspended: bool,
    search: Option<matou_native_runtime::search::SearchResult>,
    pub focus: FocusHandle,
    font: Font,
    font_size: u8,
    metrics: Metrics,
    vertical_inset: Pixels,
    last_dimensions: Option<Dimensions>,
    screen: Option<Screen>,
    resize_frames: Option<ResizeFrames>,
    open_link: Option<std::rc::Rc<LinkHandler>>,
    modes: InputModes,
    theme: Theme,
    grid: Option<VisibleGrid>,
    selection: Option<GridSelection>,
    selection_drag: Option<SelectionMode>,
    selecting: bool,
    drag_scroll_amount: i32,
    drag_scroll_timer: Option<Task<()>>,
    composition: Composition,
    composition_cell: Option<(usize, usize)>,
    caret: Option<Bounds<Pixels>>,
    mouse_reporter: MouseReporter,
    mouse_wheel: MouseWheel,
    wheel_accum: f64,
    mouse_buttons: u8,
    blink: Option<Task<()>>,
    blink_visible: bool,
    input_disabled: bool,
    scrollbar_bounds: Option<Bounds<Pixels>>,
    scrollbar_position: Option<ScrollbarPosition>,
    scrollbar_drag: Option<ScrollbarDrag>,
    surface_hovered: bool,
    scrollport_hovered: bool,
    scrollbar_fade: Option<ScrollbarFade>,
    scrollbar_opacity: f32,
    scrollbar_revealed: bool,
    scrollbar_hide_at: Option<Instant>,
    scrollbar_hide_timer: Option<Task<()>>,
    scrollbar_frame_pending: bool,
    scroll_request_id: u64,
    history_binding: Option<HistoryBinding>,
    history_locale: InterfaceLocale,
    history_probe: WheelProbe,
    session_history: Option<HistoryView>,
    history_request_serial: u64,
    history_updates: Option<Task<()>>,
    history_keyboard_navigation: bool,
}

impl EventEmitter<SurfaceEvent> for TerminalSurface {}

impl TerminalSurface {
    fn emit(&self, kind: SurfaceEventKind, cx: &mut Context<Self>) {
        if self.transport_suspended && !matches!(kind, SurfaceEventKind::Resize { .. }) {
            return;
        }
        cx.emit(SurfaceEvent {
            generation: self.transport_generation,
            kind,
        });
    }
    pub fn begin_transport_generation(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.transport_generation == generation {
            return;
        }
        self.transport_generation = generation;
        self.transport_suspended = true;
        self.resize_frames = None;
        self.set_input_disabled(true, cx);
        self.search = None;
        self.composition.clear();
        self.composition_cell = None;
        self.caret = None;
        self.grid = None;
        self.stop_drag();
        self.selection_drag = None;
        self.mouse_reporter = MouseReporter::default();
        self.mouse_wheel = MouseWheel::default();
        self.wheel_accum = 0.;
        self.mouse_buttons = 0;
        self.reset_blink();
        self.scrollbar_position = None;
        self.scrollbar_drag = None;
        self.scroll_request_id = 0;
        self.scrollbar_bounds = None;
        self.scrollbar_hide_timer = None;
        self.scrollbar_hide_at = None;
        self.scrollbar_fade = None;
        self.scrollbar_frame_pending = false;
        self.scrollbar_revealed = false;
        self.scrollbar_opacity = 0.;
        self.surface_hovered = false;
        self.scrollport_hovered = false;
        self.history_probe.reset();
        self.session_history = None;
        self.history_updates = None;
        cx.notify();
    }
    pub fn set_transport_available(&mut self, available: bool, cx: &mut Context<Self>) {
        if self.transport_suspended != !available {
            self.transport_suspended = !available;
            cx.notify();
        }
    }
    fn emit_resize_for_generation(
        &self,
        generation: u64,
        dimensions: Dimensions,
        cx: &mut Context<Self>,
    ) {
        if self.transport_generation != generation {
            return;
        }
        self.emit(
            SurfaceEventKind::Resize {
                cols: dimensions.cols,
                rows: dimensions.rows,
                cell_width_px: dimensions.cell_width_px,
                cell_height_px: dimensions.cell_height_px,
            },
            cx,
        );
    }
    pub fn frame_matches_target(&self) -> bool {
        self.screen.as_ref().is_some_and(|s| {
            s.cols > 0
                && s.rows > 0
                && s.cells.len() == s.cols * s.rows
                && self
                    .last_dimensions
                    .is_some_and(|d| s.cols == d.cols as usize && s.rows == d.rows as usize)
        })
    }
    pub fn target_dimensions(&self) -> Option<(u16, u16, u32, u32)> {
        self.last_dimensions
            .map(|d| (d.cols, d.rows, d.cell_width_px, d.cell_height_px))
    }
    pub fn set_search(
        &mut self,
        result: Option<matou_native_runtime::search::SearchResult>,
        cx: &mut Context<Self>,
    ) {
        if result.as_ref().map(|search| search.sequence)
            != self.search.as_ref().map(|search| search.sequence)
        {
            self.scrollbar_position = None;
        }
        self.search = result;
        cx.notify();
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let text = cx.text_system();
        let families = text.all_font_names();
        let family = FAMILIES
            .iter()
            .find(|family| {
                families
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(family))
            })
            .copied()
            .unwrap_or("monospace");
        let face = font(family);
        let metrics = Self::measure_font(&face, DEFAULT_TERMINAL_FONT_SIZE, cx);
        Self {
            transport_generation: 0,
            transport_suspended: false,
            search: None,
            focus: cx.focus_handle().tab_stop(true),
            font: face,
            font_size: DEFAULT_TERMINAL_FONT_SIZE,
            metrics,
            vertical_inset: DEFAULT_VERTICAL_INSET,
            last_dimensions: None,
            screen: None,
            resize_frames: None,
            open_link: None,
            modes: InputModes {
                application_cursor_keys: false,
                bracketed_paste: false,
                input_epoch: 0,
                mouse_protocol: matou_native_terminal::MouseProtocol::None,
                mouse_encoding: matou_native_terminal::MouseEncoding::Default,
            },
            theme: Theme::default(),
            grid: None,
            selection: None,
            selection_drag: None,
            selecting: false,
            drag_scroll_amount: 0,
            drag_scroll_timer: None,
            composition: Composition::default(),
            composition_cell: None,
            caret: None,
            mouse_reporter: MouseReporter::default(),
            mouse_wheel: MouseWheel::default(),
            wheel_accum: 0.,
            mouse_buttons: 0,
            blink: None,
            blink_visible: true,
            input_disabled: false,
            scrollbar_bounds: None,
            scrollbar_position: None,
            scrollbar_drag: None,
            surface_hovered: false,
            scrollport_hovered: false,
            scrollbar_fade: None,
            scrollbar_opacity: 0.,
            scrollbar_revealed: false,
            scrollbar_hide_at: None,
            scrollbar_hide_timer: None,
            scrollbar_frame_pending: false,
            scroll_request_id: 0,
            history_binding: None,
            history_locale: InterfaceLocale::default(),
            history_probe: WheelProbe::default(),
            session_history: None,
            history_request_serial: 0,
            history_updates: None,
            history_keyboard_navigation: true,
        }
    }

    pub(crate) fn with_canvas_inset(mut self) -> Self {
        self.vertical_inset = CANVAS_VERTICAL_INSET;
        self
    }

    pub(crate) fn with_link_handler(
        mut self,
        handler: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.open_link = Some(std::rc::Rc::new(handler));
        self
    }

    pub fn set_input_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.input_disabled == disabled {
            return;
        }
        self.input_disabled = disabled;
        if disabled {
            self.composition.clear();
            self.reset_blink();
            self.history_probe.reset();
        }
        cx.notify();
    }

    pub(crate) fn begin_resize(&mut self, cx: &mut Context<Self>) {
        if self.screen.is_none() || self.transport_suspended {
            return;
        }
        let limit = cx.background_executor().now() + RESIZE_FRAME_LIMIT;
        self.resize_frames = Some(ResizeFrames {
            limit,
            present_at: limit,
            latest: None,
            timer: None,
        });
    }

    fn schedule_resize_frame(&mut self, cx: &mut Context<Self>) {
        let Some(frames) = &mut self.resize_frames else {
            return;
        };
        if frames.timer.is_some() {
            return;
        }
        let now = cx.background_executor().now();
        if now >= frames.present_at {
            if let Some(frame) = self.resize_frames.take().and_then(|frames| frames.latest) {
                self.apply_screen(
                    frame.screen,
                    frame.selection,
                    frame.scroll_request_id,
                    false,
                    cx,
                );
            }
            return;
        }
        let timer = cx.background_executor().timer(frames.present_at - now);
        frames.timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(frames) = &mut this.resize_frames {
                    frames.timer = None;
                    this.schedule_resize_frame(cx);
                }
            });
        }));
    }

    pub fn set_screen(
        &mut self,
        screen: Screen,
        modes: InputModes,
        selection: Option<GridSelection>,
        scroll_request_id: u64,
        cx: &mut Context<Self>,
    ) {
        if screen.alternate {
            if self.history_probe.needs_observation() {
                self.history_probe.observe(fingerprint(&screen));
            }
        } else {
            self.history_probe.reset();
            self.session_history = None;
            self.history_updates = None;
        }
        self.scroll_request_id = self.scroll_request_id.max(scroll_request_id);
        let modes_changed = self.modes != modes;
        if self.modes.input_epoch != modes.input_epoch {
            self.scrollbar_drag = None;
            self.scrollbar_position = None;
            self.mouse_reporter = MouseReporter::default();
            self.mouse_wheel = MouseWheel::default();
            self.wheel_accum = 0.;
            self.mouse_buttons = 0;
        }
        if self.modes.mouse_protocol != modes.mouse_protocol {
            if modes.mouse_protocol == MouseProtocol::None {
                self.mouse_buttons = 0;
            } else {
                self.stop_drag();
            }
        }
        self.modes = modes;
        if let Some(frames) = &mut self.resize_frames {
            let now = cx.background_executor().now();
            if now < frames.present_at {
                frames.latest = Some(ResizeFrame {
                    screen,
                    selection,
                    scroll_request_id,
                });
                frames.present_at = (now + RESIZE_FRAME_QUIET).min(frames.limit);
                // One timer can recheck the quiet period when it wakes, even if
                // many redraw frames have arrived since it was scheduled.
                self.schedule_resize_frame(cx);
                if modes_changed {
                    cx.notify();
                }
                return;
            }
            self.resize_frames = None;
        }
        self.apply_screen(screen, selection, scroll_request_id, modes_changed, cx);
    }

    fn apply_screen(
        &mut self,
        screen: Screen,
        selection: Option<GridSelection>,
        scroll_request_id: u64,
        modes_changed: bool,
        cx: &mut Context<Self>,
    ) {
        let extent_changed = self.screen.as_ref().is_some_and(|old| {
            old.rows != screen.rows || old.scrollbar.total != screen.scrollbar.total
        });
        let absolute_scroll_reply = !extent_changed
            && self
                .scrollbar_position
                .as_ref()
                .is_some_and(|position| scroll_request_id <= position.request_id);
        let clear_position = self.scrollbar_position.as_ref().is_some_and(|position| {
            extent_changed
                || (scroll_request_id >= position.request_id
                    && screen.scrollbar.offset != position.row)
        });
        if clear_position {
            self.scrollbar_position = self.scrollbar_position.and_then(|position| {
                let old = self.screen.as_ref()?;
                let new_anchor = screen.resize_position_row?;
                if !extent_changed || old.scrollbar.offset != position.row {
                    return None;
                }
                let cell = f64::from(f32::from(self.metrics.height));
                let old_anchor = old.resize_position_row.unwrap_or(old.scrollbar.offset);
                let fraction = position.pixels - old_anchor as f64 * cell;
                let limit = screen.scrollbar.total.saturating_sub(screen.rows as u64) as f64 * cell;
                Some(ScrollbarPosition {
                    pixels: (new_anchor as f64 * cell + fraction).clamp(0., limit),
                    row: screen.scrollbar.offset,
                    request_id: scroll_request_id,
                })
            });
        }
        if !modes_changed && self.screen.as_ref() == Some(&screen) && self.selection == selection {
            if clear_position {
                cx.notify();
            }
            return;
        }
        #[cfg(debug_assertions)]
        if let Some(sent) = self
            .session_history
            .as_mut()
            .and_then(|history| history.input_sent_at.take())
        {
            session_history_seam::trace("input-to-next-screen", sent);
        }
        if self
            .session_history
            .as_ref()
            .is_some_and(|history| history.closing)
        {
            self.session_history = None;
            self.history_updates = None;
        } else if self
            .session_history
            .as_ref()
            .is_some_and(|history| history.ready)
        {
            self.schedule_history_updates(cx);
        }
        if let Some(old) = &self.screen {
            let scrollbar_changed = old.scrollbar != screen.scrollbar;
            let cursor_changed = old.cursor_col != screen.cursor_col
                || old.cursor_row != screen.cursor_row
                || old.cursor_visible != screen.cursor_visible
                || old.cursor_style != screen.cursor_style
                || old.cursor_blinking != screen.cursor_blinking;
            if cursor_changed {
                self.reset_blink();
            }
            // The absolute scroll already revealed the bar when it was requested.
            // A late frame must not reverse the fade after an outside release.
            if scrollbar_changed
                && (!absolute_scroll_reply
                    || self.scrollport_hovered
                    || self.scrollbar_drag.is_some())
            {
                self.reveal_scrollbar();
            }
        }
        self.screen = Some(screen);
        self.selection = selection;
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.theme != theme {
            self.theme = theme;
            cx.notify();
        }
    }

    pub(crate) fn set_history_binding(
        &mut self,
        binding: Option<HistoryBinding>,
        locale: InterfaceLocale,
        cx: &mut Context<Self>,
    ) {
        if self.history_binding != binding {
            self.history_binding = binding;
            self.history_probe.reset();
            self.session_history = None;
            self.history_updates = None;
            cx.notify();
        }
        if self.history_locale != locale {
            self.history_locale = locale;
            if self.session_history.is_some() {
                cx.notify();
            }
        }
    }

    pub(crate) fn history_binding(&self) -> Option<&HistoryBinding> {
        self.history_binding.as_ref()
    }

    fn history_body_contains(&self, position: Point<Pixels>) -> bool {
        self.grid.is_some_and(|grid| {
            grid.bounds.contains(&position)
                && position.y
                    < grid.bounds.top()
                        + self.metrics.height * grid.rows.saturating_sub(INPUT_ROWS) as f32
        })
    }

    fn history_covers(&self, position: Point<Pixels>) -> bool {
        self.history_body_contains(position)
            && self
                .session_history
                .as_ref()
                .is_some_and(HistoryView::visible)
    }

    fn start_history_probe(&mut self, cx: &mut Context<Self>) {
        if self.history_binding.is_none() || self.session_history.is_some() || self.search.is_some()
        {
            return;
        }
        let Some(screen) = &self.screen else {
            return;
        };
        let Some(serial) = self.history_probe.start(fingerprint(screen)) else {
            return;
        };
        let timer = cx.background_executor().timer(WHEEL_SETTLE);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                let Some(screen) = this.screen.as_ref().filter(|screen| screen.alternate) else {
                    return;
                };
                if this.input_disabled
                    || this.transport_suspended
                    || this.session_history.is_some()
                    || !this.history_probe.settled(serial, fingerprint(screen))
                {
                    return;
                }
                this.history_probe.reset();
                this.history_keyboard_navigation = true;
                this.session_history = Some(HistoryView::new(screen, cx.focus_handle()));
                this.request_history_page(HistoryPage::Latest, cx);
            });
        })
        .detach();
    }

    fn request_history_page(&mut self, page: HistoryPage, cx: &mut Context<Self>) {
        let Some(history) = self.session_history.as_mut() else {
            return;
        };
        if history.pending.is_some() || history.closing {
            return;
        }
        self.history_request_serial += 1;
        let serial = self.history_request_serial;
        history.pending = Some((page, serial));
        #[cfg(debug_assertions)]
        {
            history.request_sent_at = Instant::now();
        }
        if self
            .history_binding
            .as_ref()
            .and_then(|binding| binding.provider_session_id.as_ref())
            .is_none()
        {
            self.history_read_failed(page, cx);
            return;
        }
        self.emit(SurfaceEventKind::SessionHistory(page), cx);
        let timer = cx.background_executor().timer(READ_TIMEOUT);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .session_history
                    .as_ref()
                    .and_then(|history| history.pending)
                    == Some((page, serial))
                {
                    this.history_read_failed(page, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn history_read_failed(&mut self, page: HistoryPage, cx: &mut Context<Self>) {
        if let Some(history) = self.session_history.as_mut()
            && history.pending.is_some_and(|(pending, _)| pending == page)
        {
            history.fail(page);
            cx.notify();
        }
    }

    pub(crate) fn receive_history(&mut self, result: &Value, cx: &mut Context<Self>) {
        let Some(binding) = &self.history_binding else {
            return;
        };
        let Some(history) = self.session_history.as_mut() else {
            return;
        };
        let Some((page, _)) = history.pending else {
            return;
        };
        if result["provider"].is_null()
            && result["cwd"].as_str() == Some(binding.cwd.as_str())
            && result["providerSessionId"].as_str() == binding.provider_session_id.as_deref()
            && page.matches(result)
        {
            #[cfg(debug_assertions)]
            let received_at = Instant::now();
            history.receive(page, result);
            #[cfg(debug_assertions)]
            {
                let phase = match page {
                    HistoryPage::Latest => "latest-page",
                    HistoryPage::Earlier(_) => "earlier-page",
                    HistoryPage::Updates(_) => "updates-page",
                };
                session_history_seam::trace(phase, history.request_sent_at);
                session_history_seam::trace("integrate-page", received_at);
            }
            let next = match page {
                HistoryPage::Updates(start)
                    if result["page"]["hasLater"] == true && history.latest_index >= start =>
                {
                    Some(HistoryPage::Updates(history.latest_index + 1))
                }
                _ => None,
            };
            cx.notify();
            if let Some(next) = next {
                self.request_history_page(next, cx);
            }
        }
    }

    fn schedule_history_updates(&mut self, cx: &mut Context<Self>) {
        if self.history_updates.is_some() {
            return;
        }
        let timer = cx.background_executor().timer(Duration::from_secs(1));
        self.history_updates = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                this.history_updates = None;
                if let Some(history) = &this.session_history
                    && history.ready
                    && !history.closing
                {
                    if history.pending.is_some() {
                        this.schedule_history_updates(cx);
                    } else {
                        this.request_history_page(
                            HistoryPage::Updates(history.latest_index + 1),
                            cx,
                        );
                    }
                }
            });
        }));
    }

    fn history_action(&mut self, action: HistoryAction, cx: &mut Context<Self>) {
        let Some(history) = self.session_history.as_mut() else {
            return;
        };
        match action {
            HistoryAction::Retry => {
                if let Some(page) = history.retry {
                    self.request_history_page(page, cx);
                }
            }
            HistoryAction::Tool(index) => history.expand_tool(index),
            HistoryAction::Text(index) => history.expand_text(index),
        }
        cx.notify();
    }

    fn close_session_history(
        &mut self,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(history) = self.session_history.as_mut() else {
            return;
        };
        if history.closing {
            return;
        }
        if history.focus.is_focused(window) {
            self.focus.focus(window);
        }
        if !restore || self.input_disabled || self.transport_suspended {
            self.session_history = None;
            self.history_updates = None;
            cx.notify();
            return;
        }
        history.closing = true;
        self.send(crate::terminal_input::CLAUDE_SCROLL_TO_BOTTOM.to_vec(), cx);
        let timer = cx.background_executor().timer(WHEEL_SETTLE);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .session_history
                    .as_ref()
                    .is_some_and(|history| history.closing)
                {
                    this.session_history = None;
                    this.history_updates = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn maybe_prefetch_history(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self
            .session_history
            .as_ref()
            .filter(|view| view.near_top(self.metrics.height))
            .and_then(HistoryView::earlier_page)
        {
            self.request_history_page(page, cx);
        }
    }

    fn handle_history_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(history) = self.session_history.as_ref() else {
            return false;
        };
        if key == "escape" {
            self.close_session_history(history.ready, window, cx);
            return true;
        }
        if !history.ready || !self.history_keyboard_navigation {
            return false;
        }
        let page_height = (history.list.viewport_bounds().size.height - self.metrics.height)
            .max(self.metrics.height);
        match key {
            "pageup" => history.list.scroll_by(-page_height),
            "pagedown" => history.list.scroll_by(page_height),
            "home" => history.list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            }),
            "end" => history.list.scroll_to(gpui::ListOffset {
                item_ix: history.list.item_count(),
                offset_in_item: px(0.),
            }),
            _ => return false,
        }
        self.maybe_prefetch_history(cx);
        cx.notify();
        true
    }

    fn render_session_history(&self, cx: &Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        let history = self
            .session_history
            .as_ref()
            .filter(|history| history.visible())?;
        #[cfg(debug_assertions)]
        let paint_start = session_history_seam::tracing().then(Instant::now);
        let screen = self.screen.as_ref()?;
        let height = self.metrics.height;
        let rows = self.grid.map_or(screen.rows, |grid| grid.rows);
        let history_height = height * rows.saturating_sub(INPUT_ROWS) as f32;
        let dim = rgb(mix(screen.foreground, screen.background));
        let style = HistoryStyle {
            locale: self.history_locale,
            foreground: rgb(screen.foreground),
            dim,
            line_height: height,
        };
        let surface = cx.entity();
        let row_surface = surface.clone();
        let row_weak = surface.downgrade();
        let rows = gpui::list(history.list.clone(), move |index, _, cx| {
            let surface = row_surface.read(cx);
            let Some(history) = &surface.session_history else {
                return div().into_any_element();
            };
            if let Some(snapshot) = history.snapshot_at(index) {
                let snapshot_rows = snapshot.rows.saturating_sub(INPUT_ROWS);
                let painter = row_surface.clone();
                return canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        painter.update(cx, |surface, cx| {
                            let cols = (f32::from(bounds.size.width)
                                / f32::from(surface.metrics.width))
                                as usize;
                            surface.paint_grid(
                                &snapshot,
                                bounds.origin,
                                snapshot.cols.min(cols),
                                snapshot_rows,
                                None,
                                window,
                                cx,
                            );
                        });
                    },
                )
                .w_full()
                .h(height * snapshot_rows as f32)
                .into_any_element();
            }
            history.item(index, style, &row_weak)
        })
        .size_full();
        let return_text = self.history_locale.text("回到最新", "Jump to latest");
        let return_text = if history.new_messages == 0 {
            format!("{return_text} ↓")
        } else {
            let count = history.new_messages;
            format!("{return_text} · {count} ↓")
        };
        let layer = div()
            .id("session-history-seam")
            .absolute()
            .top(self.vertical_inset)
            .bottom(self.vertical_inset + height * INPUT_ROWS as f32)
            .max_h(history_height)
            .left(px(14.))
            .right(px(14.))
            .overflow_hidden()
            .occlude()
            .track_focus(&history.focus)
            .cursor(PointerStyle::Arrow)
            .bg(rgb(screen.background))
            .text_color(style.foreground)
            .text_size(px(self.font_size as f32))
            .font_family(self.font.family.clone())
            .line_height(height)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if let Some(history) = &this.session_history {
                        this.history_keyboard_navigation = true;
                        history.focus.focus(window);
                    }
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                if let Some(zoom) =
                    font_zoom_for_wheel(event.delta, event.modifiers, cfg!(target_os = "macos"))
                {
                    this.emit(SurfaceEventKind::FontZoom(zoom), cx);
                } else if let Some(history) = &this.session_history {
                    let down = event.delta.pixel_delta(px(20.)).y < px(0.);
                    if down && history.opened_at.elapsed() <= FALSE_START {
                        this.close_session_history(false, window, cx);
                    } else if down && history.ready && history.at_bottom() {
                        this.close_session_history(true, window, cx);
                    } else {
                        this.maybe_prefetch_history(cx);
                    }
                }
                cx.stop_propagation();
            }))
            .when(history.ready, |div| div.child(rows))
            .when(!history.ready, |div| {
                div.child(session_history_seam::error_row(
                    self.history_locale,
                    &surface.downgrade(),
                ))
            })
            .child(
                div()
                    .id("session-history-latest")
                    .absolute()
                    .right(px(8.))
                    .bottom(px(8.))
                    .px(px(8.))
                    .py(px(4.))
                    .border_1()
                    .border_color(dim)
                    .rounded(px(5.))
                    .bg(rgb(screen.background))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        let ready = this
                            .session_history
                            .as_ref()
                            .is_some_and(|history| history.ready);
                        this.close_session_history(ready, window, cx);
                        cx.stop_propagation();
                    }))
                    .child(return_text),
            );
        #[cfg(debug_assertions)]
        let layer = layer.children(paint_start.map(|start| {
            canvas(
                |_, _, _| (),
                move |_, (), _, _| session_history_seam::trace("render-to-paint", start),
            )
            .absolute()
            .top_0()
            .left_0()
        }));
        Some(layer)
    }

    fn send(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if self.input_disabled || bytes.is_empty() {
            return;
        }
        self.selection_drag = None;
        self.stop_drag();
        self.reset_blink();
        self.scroll_on_input(cx);
        #[cfg(debug_assertions)]
        if session_history_seam::tracing()
            && bytes
                .first()
                .is_some_and(|first| *first >= b' ' && *first != 127)
            && let Some(history) = self
                .session_history
                .as_mut()
                .filter(|history| history.ready)
        {
            history.input_sent_at = Some(Instant::now());
        }
        self.emit(SurfaceEventKind::Input(bytes), cx);
        cx.notify();
    }

    fn scroll_on_input(&mut self, cx: &mut Context<Self>) {
        if self.input_disabled {
            return;
        }
        // An absolute scroll can be queued while the last frame still shows
        // the bottom. Place the return-to-live command after it.
        let pending_scroll = self.scrollbar_position.take().is_some();
        if pending_scroll
            || self
                .screen
                .as_ref()
                .is_some_and(|s| s.scrollbar.offset + (s.rows as u64) < s.scrollbar.total)
        {
            self.emit(SurfaceEventKind::Scroll(i32::MIN), cx);
        }
    }

    fn reset_blink(&mut self) {
        self.blink = None;
        self.blink_visible = true;
    }

    fn schedule_blink(&mut self, focused: bool, cx: &mut Context<Self>) {
        let blink = focused
            && !self.transport_suspended
            && self.screen.as_ref().is_some_and(|screen| {
                screen.cursor_visible && screen.cursor_has_position && screen.cursor_blinking
                    || screen.cells.iter().any(|cell| cell.blink)
            });
        if !blink {
            self.reset_blink();
        } else if self.blink.is_none() {
            let timer = cx.background_executor().timer(BLINK_INTERVAL);
            let generation = self.transport_generation;
            self.blink = Some(cx.spawn(async move |this, cx| {
                timer.await;
                let _ = this.update(cx, |this, cx| {
                    if this.transport_generation != generation {
                        return;
                    }
                    this.blink = None;
                    this.blink_visible = !this.blink_visible;
                    cx.notify();
                });
            }));
        }
    }

    fn measure_font(face: &Font, size: u8, cx: &Context<Self>) -> Metrics {
        let text = cx.text_system();
        let font_size = px(f32::from(size));
        let id = text.resolve_font(face);
        let width = text
            .ch_advance(id, font_size)
            .unwrap_or_else(|_| text.bounding_box(id, font_size).size.width);
        let height = text.ascent(id, font_size) - text.descent(id, font_size);
        Metrics {
            width: width.max(px(1.)),
            height: px(f32::from(height).ceil()).max(px(1.)),
        }
    }

    pub fn set_font_size(&mut self, size: u8, cx: &mut Context<Self>) {
        if self.font_size != size {
            self.font_size = size;
            self.metrics = Self::measure_font(&self.font, size, cx);
            self.last_dimensions = None;
            self.history_probe.reset();
            if let Some(history) = &self.session_history {
                history.remeasure_rows();
            }
            cx.notify();
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.composition.is_active() {
            self.scroll_on_input(cx);
            cx.stop_propagation();
            return;
        }
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        if !mods.control
            && !mods.alt
            && !mods.platform
            && !mods.shift
            && self.handle_history_key(key, window, cx)
        {
            cx.stop_propagation();
            return;
        }
        let rows = self.screen.as_ref().map_or(0, |screen| screen.rows);
        if let Some(lines) = page_scroll_lines(key, mods.shift, rows) {
            self.move_scrollbar(lines * f64::from(f32::from(self.metrics.height)), cx);
            cx.stop_propagation();
            return;
        }
        let mac = cfg!(target_os = "macos");
        let copy_mod = if mac { mods.platform } else { mods.control };
        if key == "c" && copy_mod && !mods.alt && !mods.shift {
            self.emit(
                SurfaceEventKind::Copy {
                    interrupt_if_empty: !mac && !self.input_disabled,
                },
                cx,
            );
            cx.stop_propagation();
            return;
        }
        if mac && key == "a" && mods.platform && !mods.control && !mods.alt && !mods.shift {
            self.emit(SurfaceEventKind::Select(SelectionAction::SelectAll), cx);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.input_disabled {
            return;
        }
        let paste_mod = if mac {
            mods.platform && !mods.control
        } else {
            mods.control && !mods.platform
        };
        if key == "v" && paste_mod && !mods.alt {
            if let Some(bytes) = read_terminal_paste(cx, self.modes) {
                self.send(bytes, cx);
            }
            cx.stop_propagation();
            return;
        }
        if !mods.platform {
            if let Some(bytes) = terminal_key(key, mods.shift, mods.control, mods.alt, self.modes) {
                self.send(bytes, cx);
                cx.stop_propagation();
            }
        }
    }

    fn selection_location(&self, position: Point<Pixels>) -> Option<(VisibleGrid, usize, f32)> {
        let screen = self.screen.as_ref()?;
        let mut grid = self.grid?;
        grid.cols = grid.cols.min(screen.cols);
        grid.rows = grid.rows.min(screen.rows);
        let row =
            f32::from((position.y - grid.bounds.top()) / self.metrics.height).max(0.) as usize;
        let x = f32::from((position.x - grid.bounds.left()) / self.metrics.width);
        let row = screen.scrollbar.offset as usize + row.min(grid.rows - 1);
        Some((grid, row, x))
    }

    fn link_at(&self, position: Point<Pixels>) -> Option<String> {
        let screen = self.screen.as_ref()?;
        let grid = self.grid.filter(|grid| grid.bounds.contains(&position))?;
        let col = f32::from((position.x - grid.bounds.left()) / self.metrics.width) as usize;
        let row = f32::from((position.y - grid.bounds.top()) / self.metrics.height) as usize;
        if col >= screen.cols || row >= screen.rows {
            return None;
        }
        let cells = &screen.cells;
        let hit = row * screen.cols + col;
        let token_cell = |index: usize| {
            cells.get(index).is_some_and(|cell| {
                !cell.invisible
                    && (cell.width == 0
                        || cell.has_content && !cell.text.chars().any(char::is_whitespace))
            })
        };
        let follows = |index: usize| {
            index % screen.cols != 0
                || screen
                    .row_metadata
                    .get(index / screen.cols)
                    .is_some_and(|row| row.wrap_continuation)
        };
        if !token_cell(hit) {
            return None;
        }
        let mut start = hit;
        while start > 0 && follows(start) && token_cell(start - 1) {
            start -= 1;
        }
        let mut end = hit + 1;
        while end < cells.len() && follows(end) && token_cell(end) {
            end += 1;
        }
        if start == 0 && follows(start)
            || end == cells.len() && screen.row_metadata.last().is_some_and(|row| row.wrap)
        {
            // Never open a partial link cut off by the terminal viewport.
            return None;
        }
        let token: String = cells[start..end]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        let (scheme, rest) = token.split_once(':')?;
        if !scheme.is_empty()
            && (rest.starts_with("//")
                || [
                    "https",
                    "http",
                    "file",
                    "javascript",
                    "data",
                    "cmd",
                    "mailto",
                ]
                .iter()
                .any(|known| scheme.eq_ignore_ascii_case(known)))
        {
            Some(token)
        } else {
            None
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn cell_center_for_link_probe(&self, col: usize, row: usize) -> Point<Pixels> {
        self.metrics
            .cell(self.grid.expect("painted grid").bounds.origin, col, row, 1)
            .center()
    }

    fn update_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.selecting
            && let Some((grid, row, x)) = self.selection_location(position)
        {
            self.drag_scroll_amount = selection_scroll_amount(
                f32::from(position.y - grid.bounds.top()),
                f32::from(grid.bounds.size.height),
            );
            let action = if self.drag_scroll_amount == 0 {
                SelectionAction::Extend { row, x }
            } else {
                SelectionAction::ExtendOutside {
                    row,
                    x,
                    below: self.drag_scroll_amount < 0,
                }
            };
            self.emit(SurfaceEventKind::Select(action), cx);
        }
    }

    fn finish_selection(&mut self) {
        // The last move/timer owns the endpoint; mouseup only ends the gesture.
        self.stop_drag();
    }

    fn stop_drag(&mut self) {
        self.selecting = false;
        self.drag_scroll_amount = 0;
        self.drag_scroll_timer = None;
    }

    fn scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        let bounds = self.scrollbar_bounds?;
        let screen = self.screen.as_ref()?;
        ScrollbarMetrics::new(
            f64::from(f32::from(bounds.size.height)),
            screen.scrollbar.total as f64 * f64::from(f32::from(self.metrics.height)),
            // Paint the thumb for the same viewport as the displayed cells.
            self.scrollbar_position
                .filter(|position| position.row == screen.scrollbar.offset)
                .map_or(
                    screen
                        .resize_position_row
                        .unwrap_or(screen.scrollbar.offset) as f64
                        * f64::from(f32::from(self.metrics.height)),
                    |position| position.pixels,
                ),
        )
    }

    fn fade_scrollbar(&mut self, revealed: bool) {
        if self.scrollbar_revealed == revealed {
            return;
        }
        let now = Instant::now();
        let from = self
            .scrollbar_fade
            .as_ref()
            .map_or(self.scrollbar_opacity, |fade| fade.opacity(now));
        let target = if revealed { 1. } else { 0. };
        self.scrollbar_revealed = revealed;
        self.scrollbar_opacity = from;
        self.scrollbar_fade = ((target - from).abs() > f32::EPSILON).then(|| ScrollbarFade {
            start: now,
            from,
            target,
            duration: Duration::from_secs_f32(
                (if revealed { 0.1 } else { 0.8 }) * (target - from).abs(),
            ),
        });
    }

    fn reveal_scrollbar(&mut self) {
        self.fade_scrollbar(true);
        self.scrollbar_hide_at = (!self.scrollport_hovered && self.scrollbar_drag.is_none())
            .then(|| Instant::now() + Duration::from_millis(500));
    }

    fn hide_scrollbar(&mut self) {
        if !self.scrollport_hovered && self.scrollbar_drag.is_none() {
            self.scrollbar_hide_at = None;
            self.scrollbar_hide_timer = None;
            self.fade_scrollbar(false);
        }
    }

    fn schedule_scrollbar_hide(&mut self, cx: &mut Context<Self>) {
        if !self.surface_hovered || self.scrollbar_hide_timer.is_some() {
            return;
        }
        let Some(deadline) = self.scrollbar_hide_at else {
            return;
        };
        let timer = cx
            .background_executor()
            .timer(deadline.saturating_duration_since(Instant::now()));
        let generation = self.transport_generation;
        self.scrollbar_hide_timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.transport_generation != generation {
                    return;
                }
                this.scrollbar_hide_timer = None;
                if this
                    .scrollbar_hide_at
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    this.hide_scrollbar();
                    cx.notify();
                } else {
                    // A later scroll moved the same one-shot deadline.
                    this.schedule_scrollbar_hide(cx);
                }
            });
        }));
    }

    fn scroll_to_pixels(&mut self, position: f64, cx: &mut Context<Self>) {
        if self
            .scrollbar_position
            .is_some_and(|current| current.pixels == position)
        {
            return;
        }
        let row = (position / f64::from(f32::from(self.metrics.height)))
            .round()
            .max(0.) as u64;
        let row_is_visible = self
            .screen
            .as_ref()
            .is_some_and(|screen| screen.scrollbar.offset == row);
        if let Some(current) = &mut self.scrollbar_position
            && current.row == row
            && self
                .screen
                .as_ref()
                .is_none_or(|screen| screen.resize_position_row.is_none())
        {
            current.pixels = position;
            if row_is_visible {
                cx.notify();
            }
            return;
        }
        self.scroll_request_id += 1;
        self.scrollbar_position = Some(ScrollbarPosition {
            pixels: position,
            row,
            request_id: self.scroll_request_id,
        });
        self.emit(
            SurfaceEventKind::ScrollTo {
                row,
                request_id: self.scroll_request_id,
            },
            cx,
        );
        self.reveal_scrollbar();
        if row_is_visible {
            cx.notify();
        }
    }

    fn scrollbar_down(&mut self, event: &gpui::MouseDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(bounds) = self.scrollbar_bounds else {
            return false;
        };
        let track = Bounds::new(
            point(bounds.right() - px(5.), bounds.top()),
            size(px(5.), bounds.size.height),
        );
        let Some(metrics) = self.scrollbar_metrics() else {
            return false;
        };
        if !self.scrollbar_revealed || !track.contains(&event.position) {
            return false;
        }
        let offset = f64::from(f32::from(event.position.y - bounds.top()));
        let on_thumb = event.position.x >= track.left() + px(1.)
            && event.position.x < track.right() - px(1.)
            && offset >= metrics.thumb_top
            && offset < metrics.thumb_top + metrics.thumb_size;
        let metrics = if on_thumb {
            metrics
        } else {
            let position = metrics.track_position(offset);
            self.scroll_to_pixels(position, cx);
            ScrollbarMetrics::new(metrics.viewport, metrics.extent, position).unwrap()
        };
        if event.button == MouseButton::Left {
            self.stop_drag();
            self.scrollbar_drag = Some(ScrollbarDrag {
                metrics,
                pointer: event.position,
                last_pointer: event.position,
                #[cfg(target_os = "macos")]
                left_release_count: ScrollbarDrag::left_button_release_count(),
            });
            self.reveal_scrollbar();
        }
        cx.stop_propagation();
        true
    }

    pub(crate) fn scrollbar_hit(&self, position: Point<Pixels>) -> bool {
        self.scrollbar_revealed
            && self.scrollbar_metrics().is_some()
            && self.scrollbar_bounds.is_some_and(|bounds| {
                Bounds::new(
                    point(bounds.right() - px(5.), bounds.top()),
                    size(px(5.), bounds.size.height),
                )
                .contains(&position)
            })
    }

    fn scrollbar_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window_active: bool,
        button_held: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let hovered = self
            .scrollbar_bounds
            .is_some_and(|bounds| bounds.contains(&event.position));
        if let Some(drag) = &mut self.scrollbar_drag {
            if event.pressed_button != Some(MouseButton::Left) || !button_held {
                self.scrollport_hovered = hovered;
                self.scrollbar_drag = None;
                self.hide_scrollbar();
                cx.notify();
                return false;
            }
            cx.stop_propagation();
            // GPUI repeats the last drag event on macOS for selection autoscroll.
            // A stationary pointer must let a key or wheel change the viewport.
            if !window_active || event.position == drag.last_pointer {
                return true;
            }
            self.scrollport_hovered = hovered;
            drag.last_pointer = event.position;
            let position = if cfg!(target_os = "windows")
                && (event.position.x - drag.pointer.x).abs() > px(140.)
            {
                drag.metrics.position
            } else {
                drag.metrics
                    .drag_position(f64::from(f32::from(event.position.y - drag.pointer.y)))
            };
            self.scroll_to_pixels(position, cx);
            return true;
        }
        if self.scrollport_hovered != hovered {
            self.scrollport_hovered = hovered;
            if hovered {
                self.reveal_scrollbar();
            } else {
                self.hide_scrollbar();
            }
            cx.notify();
        }
        false
    }

    fn scrollbar_up(&mut self, event: &gpui::MouseUpEvent, cx: &mut Context<Self>) -> bool {
        if event.button != MouseButton::Left || self.scrollbar_drag.take().is_none() {
            return false;
        }
        self.scrollport_hovered = self
            .scrollbar_bounds
            .is_some_and(|bounds| bounds.contains(&event.position));
        self.hide_scrollbar();
        cx.notify();
        cx.stop_propagation();
        true
    }

    fn paint_scrollbar(
        &mut self,
        bounds: Bounds<Pixels>,
        rows: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scrollbar_bounds = Some(Bounds::new(
            bounds.origin,
            size(bounds.size.width, self.metrics.height * rows as f32),
        ));
        if self.scrollbar_metrics().is_none() {
            self.scrollbar_fade = None;
            self.scrollbar_opacity = 0.;
            self.scrollbar_revealed = false;
            self.scrollbar_hide_at = None;
            self.scrollbar_hide_timer = None;
            return;
        }
        let now = Instant::now();
        if self
            .scrollbar_hide_at
            .is_some_and(|deadline| now >= deadline)
        {
            self.hide_scrollbar();
        }
        if let Some(fade) = &self.scrollbar_fade {
            self.scrollbar_opacity = fade.opacity(now);
            if now.duration_since(fade.start) >= fade.duration {
                self.scrollbar_fade = None;
            }
        }
        if self.surface_hovered
            && let Some(metrics) = self.scrollbar_metrics()
        {
            let thumb = Bounds::new(
                point(
                    bounds.right() - px(4.),
                    bounds.top() + px(metrics.thumb_top as f32),
                ),
                size(px(3.), px(metrics.thumb_size as f32)),
            );
            let light = self.theme.background == 0xfcfcfd;
            let mut color: gpui::Hsla = rgba(if light { 0x6872803d } else { 0xffffff38 }).into();
            color.a = (if light { 0.24 } else { 0.22 }) * self.scrollbar_opacity;
            window.paint_quad(fill(thumb, color).corner_radii(px(1.5)));
        }
        // Wait once for the source's 500ms hide deadline rather than keeping
        // the display loop alive after the reveal animation has completed.
        self.schedule_scrollbar_hide(cx);
        // Background terminals remain transparent and do not animate.
        if self.surface_hovered && self.scrollbar_fade.is_some() && !self.scrollbar_frame_pending {
            self.scrollbar_frame_pending = true;
            let entity = cx.entity().downgrade();
            let generation = self.transport_generation;
            window.on_next_frame(move |_, cx| {
                let _ = entity.update(cx, |this, cx| {
                    if this.transport_generation != generation {
                        return;
                    }
                    this.scrollbar_frame_pending = false;
                    cx.notify();
                });
            });
        }
    }

    fn schedule_drag_scroll(&mut self, cx: &mut Context<Self>) {
        if !self.selecting || self.drag_scroll_timer.is_some() {
            return;
        }
        let timer = cx.background_executor().timer(Duration::from_millis(50));
        let generation = self.transport_generation;
        self.drag_scroll_timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.transport_generation != generation {
                    return;
                }
                this.drag_scroll_timer = None;
                if this.selecting {
                    if this.drag_scroll_amount != 0 {
                        this.emit(
                            SurfaceEventKind::ScrollSelection(this.drag_scroll_amount),
                            cx,
                        );
                    }
                    this.schedule_drag_scroll(cx);
                }
            });
        }));
    }

    pub fn contains_position(&self, position: Point<Pixels>) -> bool {
        self.grid
            .is_some_and(|grid| grid.bounds.contains(&position))
    }

    fn force_selection(modifiers: gpui::Modifiers) -> bool {
        // The frozen Mac configuration leaves Option force-selection disabled.
        !cfg!(target_os = "macos") && modifiers.shift
    }

    fn mouse_button(button: MouseButton) -> Option<u8> {
        match button {
            MouseButton::Left => Some(0),
            MouseButton::Middle => Some(1),
            MouseButton::Right => Some(2),
            _ => None,
        }
    }

    fn report_mouse(
        &mut self,
        position: Point<Pixels>,
        modifiers: gpui::Modifiers,
        button: u8,
        action: u8,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(grid) = self.grid else {
            return false;
        };
        let x = f32::from(position.x - grid.bounds.origin.x).clamp(
            0.,
            (f32::from(self.metrics.width) * grid.cols as f32 - 1.).max(0.),
        );
        let y = f32::from(position.y - grid.bounds.origin.y).clamp(
            0.,
            (f32::from(self.metrics.height) * grid.rows as f32 - 1.).max(0.),
        );
        let event = MouseEvent {
            col: (x / f32::from(self.metrics.width)).floor() as i32,
            row: (y / f32::from(self.metrics.height)).floor() as i32,
            x: x.floor() as i32,
            y: y.floor() as i32,
            button,
            action,
            ctrl: modifiers.control,
            alt: modifiers.alt,
            shift: modifiers.shift,
        };
        let bytes = self.mouse_reporter.report(
            self.modes.mouse_protocol,
            self.modes.mouse_encoding,
            event,
            grid.cols,
            grid.rows,
        );
        // The frozen product subscribes to onData only, not binary mouse reports.
        if self.modes.mouse_encoding != MouseEncoding::Default
            && let Some(bytes) = bytes
        {
            self.send(bytes, cx);
            return !self.input_disabled;
        }
        false
    }

    fn mouse_down(&mut self, event: &gpui::MouseDownEvent, cx: &mut Context<Self>) -> bool {
        if self.modes.mouse_protocol == MouseProtocol::None
            || Self::force_selection(event.modifiers)
        {
            return false;
        }
        if let Some(button) = Self::mouse_button(event.button) {
            self.mouse_buttons |= 1 << button;
            self.report_mouse(event.position, event.modifiers, button, 1, cx);
        }
        self.stop_drag();
        cx.stop_propagation();
        true
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if let Some(zoom) =
            font_zoom_for_wheel(event.delta, event.modifiers, cfg!(target_os = "macos"))
        {
            self.history_probe.reset();
            self.emit(SurfaceEventKind::FontZoom(zoom), cx);
            cx.stop_propagation();
            return;
        }
        let reports_wheel = matches!(
            self.modes.mouse_protocol,
            MouseProtocol::Vt200 | MouseProtocol::Drag | MouseProtocol::Any
        );
        let alternate = self.screen.as_ref().is_some_and(|screen| screen.alternate);
        let upwards = event.delta.pixel_delta(px(20.)).y > px(0.);
        let downwards = event.delta.pixel_delta(px(20.)).y < px(0.);
        if downwards {
            self.history_probe.reset();
            if self
                .session_history
                .as_ref()
                .is_some_and(|history| !history.visible())
            {
                self.session_history = None;
                cx.stop_propagation();
                return;
            }
        }
        if reports_wheel || alternate {
            let delta = match event.delta {
                gpui::ScrollDelta::Pixels(delta) => -f32::from(delta.y),
                gpui::ScrollDelta::Lines(delta) => -delta.y,
            };
            let lines = self.mouse_wheel.consume(
                delta,
                event.delta.precise(),
                f32::from(self.metrics.height),
                event.modifiers.shift,
                event.modifiers.alt,
                event.modifiers.control,
            );
            if lines != 0. {
                let sent = if reports_wheel {
                    self.report_mouse(
                        event.position,
                        event.modifiers,
                        4,
                        if delta < 0. { 0 } else { 1 },
                        cx,
                    )
                } else {
                    let prefix = if self.modes.application_cursor_keys {
                        b'O'
                    } else {
                        b'['
                    };
                    self.send(vec![27, prefix, if delta < 0. { b'A' } else { b'B' }], cx);
                    !self.input_disabled
                };
                if sent && alternate && upwards && self.history_body_contains(event.position) {
                    self.start_history_probe(cx);
                }
            }
            cx.stop_propagation();
            return;
        }

        self.scrollbar_wheel(event, cx);
    }

    fn scrollbar_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if !event.delta.precise() && (event.modifiers.control || event.modifiers.platform) {
            return;
        }
        let delta = wheel_scroll_pixels(event.delta, event.modifiers.alt);
        if delta == 0. {
            return;
        }
        let movement = accumulate_wheel_pixels(&mut self.wheel_accum, delta);
        let handled = if movement == 0. {
            // A wheel event handled during drag capture must not accumulate again on bubble.
            self.scrollbar_metrics().is_some()
        } else {
            self.move_scrollbar(-movement, cx)
        };
        if handled {
            cx.stop_propagation();
        } else {
            self.wheel_accum = 0.;
        }
    }

    fn move_scrollbar(&mut self, movement: f64, cx: &mut Context<Self>) -> bool {
        if movement == 0. {
            return false;
        }
        let Some(metrics) = self.scrollbar_metrics() else {
            return false;
        };
        let position = self
            .scrollbar_position
            .map_or(metrics.position, |position| position.pixels);
        self.scroll_to_pixels(metrics.clamp(position + movement), cx);
        true
    }

    fn paint(
        &mut self,
        entity: Entity<Self>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let metrics = self.metrics;
        let dimensions = metrics.dimensions(bounds, window.scale_factor());
        if bounds.size.width >= metrics.width * 2.
            && bounds.size.height >= metrics.height
            && self.last_dimensions.replace(dimensions) != Some(dimensions)
        {
            let surface = entity.downgrade();
            let generation = self.transport_generation;
            window.defer(cx, move |_, cx| {
                let _ = surface.update(cx, |this, cx| {
                    this.emit_resize_for_generation(generation, dimensions, cx);
                });
            });
        }
        let focused = self.focus.is_focused(window) && window.is_window_active();
        let screen = self.screen.as_ref();
        let background = screen.map_or(self.theme.background, |screen| screen.background);
        #[cfg(all(target_os = "macos", debug_assertions))]
        if BACKGROUND_PROBE.get().is_some() {
            BACKGROUND_PROBE.set(Some(Some(background)));
        }
        window.paint_quad(fill(bounds, rgb(background)));
        let (cols, rows) = screen.map_or((0, 0), |screen| {
            (
                screen.cols.min(dimensions.cols as usize),
                screen.rows.min(dimensions.rows as usize),
            )
        });
        self.grid = (cols > 0 && rows > 0).then(|| VisibleGrid {
            bounds: Bounds::new(
                bounds.origin,
                size(metrics.width * cols as f32, metrics.height * rows as f32),
            ),
            cols,
            rows,
        });
        let visible_caret = screen
            .filter(|screen| screen.cursor_has_position)
            .map(|screen| {
                metrics.cell(
                    bounds.origin,
                    screen.cursor_col.min(cols.saturating_sub(1)),
                    screen.cursor_row.min(rows.saturating_sub(1)),
                    1,
                )
            });
        if let Some(caret) = visible_caret {
            self.caret = Some(caret);
        }
        if let Some(screen) = screen.filter(|screen| screen.cursor_has_position) {
            let history_rows = screen
                .scrollbar
                .total
                .saturating_sub(screen.scrollbar.len)
                .saturating_sub(screen.scrollbar.offset);
            if let Some(row) = (screen.cursor_row as u64).checked_sub(history_rows) {
                self.composition_cell = Some((screen.cursor_col, row as usize));
            }
        }
        if !self.input_disabled {
            // Keep candidate positioning at the last valid cursor while the
            // render frame is still catching up to the return to live output.
            let input_bounds = self
                .caret
                .unwrap_or_else(|| metrics.cell(bounds.origin, 0, 0, 1));
            window.handle_input(
                &self.focus,
                ElementInputHandler::new(input_bounds, entity),
                cx,
            );
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(screen) = screen {
                let show_cursor = screen.cursor_visible
                    && screen.cursor_has_position
                    && screen.cursor_col < cols
                    && screen.cursor_row < rows
                    && (!focused || !screen.cursor_blinking || self.blink_visible)
                    && !self.composition.is_active();
                self.paint_grid(
                    screen,
                    bounds.origin,
                    cols,
                    rows,
                    show_cursor.then_some(focused),
                    window,
                    cx,
                );
            }
        });
        if focused
            && let Some(grid) = self.grid
            && let Some(caret) = self.composition_caret(grid)
        {
            window.with_content_mask(
                Some(ContentMask {
                    bounds: grid.bounds,
                }),
                |window| {
                    self.paint_composition(caret, window, cx);
                },
            );
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.paint_scrollbar(bounds, rows, window, cx);
        });
    }

    fn composition_caret(&self, grid: VisibleGrid) -> Option<Bounds<Pixels>> {
        if !self.composition.is_active() {
            return None;
        }
        // Keep the last active-screen cell while the cursor is outside the viewport.
        let (col, row) = self.composition_cell?;
        Some(self.metrics.cell(
            grid.bounds.origin,
            col.min(grid.cols - 1),
            row.min(grid.rows - 1),
            1,
        ))
    }

    fn paint_composition(&self, caret: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let line = self.shape_composition(window);
        // The composition view is shrink-to-fit: no padding, the width is the
        // natural advance of the single shaped line, and the height is the
        // terminal cell height. It always uses a solid black background and
        // plain white glyphs, independent of the terminal theme.
        window.paint_quad(fill(
            Bounds::new(caret.origin, size(line.width, caret.size.height)),
            rgb(0x000000),
        ));
        let _ = line.paint(caret.origin, caret.size.height, window, cx);
    }

    fn shape_composition(&self, window: &mut Window) -> ShapedLine {
        shape(
            self.composition.text.replace(['\n', '\r'], " ").into(),
            &self.font,
            self.font_size,
            0xffffff,
            window,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_grid(
        &self,
        screen: &Screen,
        origin: Point<Pixels>,
        cols: usize,
        rows: usize,
        cursor_focus: Option<bool>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let fonts = CellFonts::new(&self.font);
        let cursor_style = cursor_focus.map(|focused| {
            if focused {
                screen.cursor_style
            } else {
                CursorStyle::BlockHollow
            }
        });
        let cursor_color = screen.cursor_color.unwrap_or(self.theme.cursor);
        let first_row = screen.scrollbar.offset as usize;
        for (row, cells) in screen
            .cells
            .chunks(screen.cols.max(1))
            .take(rows)
            .enumerate()
        {
            for (col, cell) in cells.iter().take(cols).enumerate() {
                let (_, bg) = cell_colors(cell, screen);
                if bg != screen.background {
                    window.paint_quad(fill(self.metrics.cell(origin, col, row, 1), rgb(bg)));
                }
            }
            if let Some(selection) = self.selection {
                let range = selection.range(first_row + row, cols.min(cells.len()));
                if !range.is_empty() {
                    let bounds = self.metrics.cell(origin, range.start, row, range.len());
                    window.paint_quad(fill(bounds, rgb(self.theme.selection_background)));
                }
            }
            if let Some(search) = &self.search {
                for matched in search.matches.iter().chain(search.active.iter()) {
                    if matched.row > first_row + row || matched.col < 0 || matched.size <= 0 {
                        continue;
                    }
                    let relative_row = first_row + row - matched.row;
                    let line_start = relative_row * screen.cols;
                    let match_start = matched.col as usize;
                    let match_end = match_start + matched.size as usize;
                    let start = match_start.saturating_sub(line_start).min(cols);
                    let end = match_end.saturating_sub(line_start).min(cols);
                    let length = end.saturating_sub(start);
                    if length == 0 {
                        continue;
                    }
                    let active = search.active.as_ref() == Some(matched);
                    let bounds = self.metrics.cell(origin, start, row, length);
                    window.paint_quad(fill(bounds, rgb(if active { 0xff9632 } else { 0xffe792 })));
                    window.paint_quad(
                        gpui::outline(
                            bounds,
                            rgb(if active { 0xc86400 } else { 0xd6a800 }),
                            gpui::BorderStyle::Solid,
                        )
                        .border_widths(px(1.)),
                    );
                }
            }
            for (col, cell) in cells.iter().take(cols).enumerate() {
                if cell.width == 0 {
                    continue;
                }
                let cell_width = usize::from(cell.width);
                let bounds = self.metrics.cell(origin, col, row, cell_width);
                let cursor = cursor_style.filter(|_| {
                    row == screen.cursor_row
                        && col <= screen.cursor_col
                        && screen.cursor_col < col + cell_width
                });
                let (fg, bg) = cell_colors(cell, screen);
                let fg = if cursor == Some(CursorStyle::Block) {
                    window.paint_quad(fill(bounds, rgb(cursor_color)));
                    self.theme.cursor_accent
                } else if cell.dim {
                    mix(fg, bg)
                } else {
                    fg
                };
                if !cell.invisible && (!cell.blink || self.blink_visible) {
                    if !cell.text.is_empty() && cell.text != " " && !cell.text.contains('\n') {
                        let line = shape(
                            cell.text.clone().into(),
                            fonts.face(cell),
                            self.font_size,
                            fg,
                            window,
                        );
                        let _ = line.paint(bounds.origin, bounds.size.height, window, cx);
                    }
                    paint_decorations(cell, screen, bounds, fg, window);
                }
                if let Some(style) = cursor
                    && style != CursorStyle::Block
                {
                    paint_cursor(window, bounds, cursor_color, style);
                }
            }
        }
    }
}

impl Render for TerminalSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !window.is_window_active() {
            self.stop_drag();
            if self
                .scrollbar_drag
                .is_some_and(|drag| !cfg!(target_os = "macos") || !drag.button_still_held())
            {
                self.scrollbar_drag = None;
                self.hide_scrollbar();
            }
        }
        self.schedule_blink(
            self.focus.is_focused(window) && window.is_window_active(),
            cx,
        );
        let surface = cx.entity();
        let generation = self.transport_generation;
        let can_drop_files = !self.input_disabled && !self.transport_suspended;
        let drop_tint = rgba(0x436ff61f);
        div()
            .id("terminal-surface")
            .size_full()
            .relative()
            .px(px(14.))
            .py(self.vertical_inset)
            .overflow_hidden()
            .cursor(if self.modes.mouse_protocol == MouseProtocol::None {
                PointerStyle::IBeam
            } else {
                PointerStyle::Arrow
            })
            .track_focus(&self.focus)
            .when(can_drop_files, |element| {
                element.drag_over::<ExternalPaths>(move |style, _, _, _| {
                    style.rounded(px(4.)).border_1().border_color(drop_tint)
                })
            })
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                if this.transport_generation != generation
                    || this.transport_suspended
                    || this.input_disabled
                {
                    return;
                }
                let text = format_dropped_paths(paths.paths(), cfg!(target_os = "windows"));
                if !text.is_empty() {
                    this.send(format!(" {text}").into_bytes(), cx);
                }
                cx.activate(true);
                window.activate_window();
                this.focus.focus(window);
                cx.notify();
            }))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                if this.transport_generation != generation || this.transport_suspended {
                    return;
                }
                this.surface_hovered = *hovered;
                if !hovered {
                    this.scrollport_hovered = false;
                    this.hide_scrollbar();
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, event, window, cx| {
                if this.transport_generation == generation && !this.transport_suspended {
                    this.handle_key(event, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.transport_generation != generation || this.transport_suspended {
                        return;
                    }
                    if this.scrollbar_down(event, cx) {
                        return;
                    }
                    if this.session_history.is_some() && !this.history_body_contains(event.position)
                    {
                        this.history_keyboard_navigation = false;
                    }
                    let mods = event.modifiers;
                    let link_modifier = if cfg!(target_os = "macos") {
                        mods.platform && !mods.control
                    } else {
                        mods.control && !mods.platform
                    };
                    if link_modifier
                        && !mods.alt
                        && !mods.shift
                        && this.modes.mouse_protocol == MouseProtocol::None
                        && let Some(open) = &this.open_link
                        && let Some(url) = this.link_at(event.position)
                    {
                        open(&url, window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                        return;
                    }
                    this.focus.focus(window);
                    this.reset_blink();
                    if this.mouse_down(event, cx) {
                        cx.notify();
                        return;
                    }
                    if event.modifiers.shift && this.modes.mouse_protocol == MouseProtocol::None {
                        if let Some((_, row, x)) = this.selection_location(event.position) {
                            this.selecting = true;
                            this.drag_scroll_amount = 0;
                            this.schedule_drag_scroll(cx);
                            this.emit(
                                SurfaceEventKind::Select(SelectionAction::IncrementalClick {
                                    row,
                                    x,
                                }),
                                cx,
                            );
                        }
                        cx.notify();
                        return;
                    }
                    if event.click_count > 3 {
                        this.selecting = this.selection_drag.is_some();
                        this.drag_scroll_amount = 0;
                        this.schedule_drag_scroll(cx);
                        cx.notify();
                        return;
                    }
                    if let Some((grid, row, x)) = this.selection_location(event.position)
                        && grid.bounds.contains(&event.position)
                    {
                        let mode = match event.click_count {
                            1 if event.modifiers.alt => SelectionMode::Column,
                            2 if x <= grid.cols as f32 - 0.5 => SelectionMode::Word,
                            3 => SelectionMode::Line,
                            _ => SelectionMode::Character,
                        };
                        this.selection_drag = Some(mode);
                        this.selecting = true;
                        this.drag_scroll_amount = 0;
                        this.schedule_drag_scroll(cx);
                        this.emit(
                            SurfaceEventKind::Select(SelectionAction::Begin { mode, row, x }),
                            cx,
                        );
                    } else {
                        this.selection_drag = None;
                        this.stop_drag();
                        this.emit(SurfaceEventKind::Select(SelectionAction::Clear), cx);
                    }
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.transport_generation != generation || this.transport_suspended {
                        return;
                    }
                    if this.scrollbar_down(event, cx) {
                        return;
                    }
                    this.focus.focus(window);
                    this.mouse_down(event, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.transport_generation != generation || this.transport_suspended {
                        return;
                    }
                    if this.scrollbar_down(event, cx) {
                        return;
                    }
                    this.focus.focus(window);
                    this.mouse_down(event, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, event, _, cx| {
                if this.transport_generation == generation && !this.transport_suspended {
                    this.on_scroll(event, cx);
                }
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        surface.update(cx, |this, cx| {
                            if this.transport_generation != generation {
                                return;
                            }
                            this.paint(surface.clone(), bounds, window, cx)
                        });
                        let scrolling = surface.downgrade();
                        window.on_mouse_event(
                            move |event: &ScrollWheelEvent, phase, window, cx| {
                                if phase == DispatchPhase::Capture && window.is_window_active() {
                                    let _ = scrolling.update(cx, |this, cx| {
                                        if this.transport_generation == generation
                                            && !this.transport_suspended
                                            && this
                                                .scrollbar_drag
                                                .is_some_and(|drag| drag.button_still_held())
                                        {
                                            this.scrollbar_wheel(event, cx);
                                        }
                                    });
                                }
                            },
                        );
                        let moving = surface.downgrade();
                        window.on_mouse_event(
                            move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                                if phase == DispatchPhase::Capture {
                                    let _ = moving.update(cx, |this, cx| {
                                        if this.transport_generation != generation
                                            || this.transport_suspended
                                        {
                                            return;
                                        }
                                        let button_held = this
                                            .scrollbar_drag
                                            .is_none_or(|drag| drag.button_still_held());
                                        if this.scrollbar_move(
                                            event,
                                            window.is_window_active(),
                                            button_held,
                                            cx,
                                        ) {
                                            return;
                                        }
                                        let button =
                                            event.pressed_button.and_then(Self::mouse_button);
                                        let reporting_drag = button.is_some_and(|button| {
                                            this.mouse_buttons & (1 << button) != 0
                                        });
                                        let reporting_hover = event.pressed_button.is_none()
                                            && this.modes.mouse_protocol == MouseProtocol::Any
                                            && this.contains_position(event.position)
                                            && !this.history_covers(event.position);
                                        if reporting_drag || reporting_hover {
                                            this.report_mouse(
                                                event.position,
                                                event.modifiers,
                                                button.unwrap_or(3),
                                                32,
                                                cx,
                                            );
                                        } else if event.dragging() {
                                            this.update_selection(event.position, cx);
                                        } else {
                                            this.stop_drag();
                                        }
                                    });
                                }
                            },
                        );
                        let released = surface.downgrade();
                        window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = released.update(cx, |this, cx| {
                                    if this.transport_generation != generation
                                        || this.transport_suspended
                                    {
                                        return;
                                    }
                                    if this.scrollbar_up(event, cx) {
                                        return;
                                    }
                                    if let Some(button) = Self::mouse_button(event.button) {
                                        if this.mouse_buttons & (1 << button) != 0 {
                                            this.report_mouse(
                                                event.position,
                                                event.modifiers,
                                                button,
                                                0,
                                                cx,
                                            );
                                            this.mouse_buttons &= !(1 << button);
                                        }
                                    }
                                    if event.button == MouseButton::Left {
                                        this.finish_selection();
                                    }
                                });
                            }
                        });
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .right(px(14.))
                    .top(px(12.))
                    .w(px(5.))
                    .h(self.metrics.height
                        * self.screen.as_ref().map_or(0, |screen| screen.rows) as f32)
                    .cursor(PointerStyle::Arrow),
            )
            .children(self.render_session_history(cx))
            .when(can_drop_files, |element| {
                // Paint the tint above the opaque terminal canvas.
                element.child(
                    div()
                        .absolute()
                        .inset_0()
                        // Give the overlay a hitbox without blocking input underneath.
                        .group("terminal-drop-overlay")
                        .drag_over::<ExternalPaths>(move |style, _, _, _| {
                            style.rounded(px(4.)).bg(drop_tint)
                        }),
                )
            })
    }
}

impl EntityInputHandler for TerminalSurface {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (text, range) = self.composition.text_for_range(range);
        *adjusted = Some(range);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.composition.selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.input_disabled && self.composition.is_active())
            .then(|| 0..self.composition.text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.composition.clear();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition.clear();
        if self.input_disabled || text.is_empty() {
            cx.notify();
        } else {
            self.send(text.as_bytes().to_vec(), cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_disabled {
            self.composition.clear();
            cx.notify();
            return;
        }
        self.composition.replace(range, text, selected);
        if self.composition.is_active() {
            self.scroll_on_input(cx);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        caret: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if !self.composition.is_active() {
            return Some(caret);
        }
        let line = self.shape_composition(window);
        let text = &self.composition.text;
        let start = line.x_for_index(byte_from_utf16(text, range.start));
        let end = line.x_for_index(byte_from_utf16(text, range.end));
        Some(Bounds::new(
            point(caret.left() + start, caret.top()),
            size((end - start).max(px(1.)), caret.size.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let caret = self.caret?;
        if position.y < caret.top() || position.y > caret.bottom() || position.x < caret.left() {
            return None;
        }
        if !self.composition.is_active() {
            return caret.contains(&position).then_some(0);
        }
        let line = self.shape_composition(window);
        let x = position.x - caret.left();
        (x <= line.width)
            .then(|| utf16_from_byte(&self.composition.text, line.closest_index_for_x(x)))
    }
}

struct CellFonts {
    normal: Font,
    bold: Font,
    italic: Font,
    bold_italic: Font,
}

impl CellFonts {
    fn new(normal: &Font) -> Self {
        let bold = Font {
            weight: FontWeight::BOLD,
            ..normal.clone()
        };
        let italic = Font {
            style: FontStyle::Italic,
            ..normal.clone()
        };
        let bold_italic = Font {
            style: FontStyle::Italic,
            ..bold.clone()
        };
        Self {
            normal: normal.clone(),
            bold,
            italic,
            bold_italic,
        }
    }

    fn face(&self, cell: &Cell) -> &Font {
        match (cell.bold, cell.italic) {
            (true, true) => &self.bold_italic,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (false, false) => &self.normal,
        }
    }
}

fn shape(
    text: SharedString,
    face: &Font,
    font_size: u8,
    fg: u32,
    window: &mut Window,
) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: face.clone(),
        color: rgb(fg).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text, px(f32::from(font_size)), &[run], None)
}

fn resolve(color: Color, default: u32, screen: &Screen) -> u32 {
    match color {
        Color::Default => default,
        Color::Palette(index) => screen.palette[usize::from(index)],
        Color::Rgb(value) => value,
    }
}

fn cell_colors(cell: &Cell, screen: &Screen) -> (u32, u32) {
    let fg = resolve(cell.fg, screen.foreground, screen);
    let bg = resolve(cell.bg, screen.background, screen);
    if cell.inverse { (bg, fg) } else { (fg, bg) }
}

fn mix(fg: u32, bg: u32) -> u32 {
    let channel = |shift: u32| (((fg >> shift) & 0xff) + ((bg >> shift) & 0xff)) / 2_u32;
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

fn paint_rule(window: &mut Window, x: Pixels, y: Pixels, width: Pixels, fg: u32) {
    window.paint_quad(fill(Bounds::new(point(x, y), size(width, px(1.))), rgb(fg)));
}

fn paint_decorations(
    cell: &Cell,
    screen: &Screen,
    bounds: Bounds<Pixels>,
    fg: u32,
    window: &mut Window,
) {
    let x = bounds.left();
    let width = bounds.size.width;
    if cell.underline {
        let color = resolve(cell.underline_color, fg, screen);
        let bottom = bounds.bottom() - px(1.);
        match cell.underline_style {
            2 => {
                paint_rule(window, x, bottom - px(2.), width, color);
                paint_rule(window, x, bottom, width, color);
            }
            3..=5 => {
                let length = if cell.underline_style == 5 { 3 } else { 1 };
                for offset in (0..f32::from(width).ceil() as usize).step_by(length + 1) {
                    let offset_px = px(offset as f32);
                    let y = if cell.underline_style == 3 && offset % 4 == 0 {
                        bottom - px(1.)
                    } else {
                        bottom
                    };
                    paint_rule(
                        window,
                        x + offset_px,
                        y,
                        px(length as f32).min(width - offset_px),
                        color,
                    );
                }
            }
            _ => paint_rule(window, x, bottom, width, color),
        }
    }
    if cell.strikethrough {
        paint_rule(window, x, bounds.top() + bounds.size.height / 2., width, fg);
    }
    if cell.overline {
        paint_rule(window, x, bounds.top(), width, fg);
    }
}

fn paint_cursor(window: &mut Window, bounds: Bounds<Pixels>, color: u32, style: CursorStyle) {
    let vertical = |window: &mut Window, x, width| {
        window.paint_quad(fill(
            Bounds::new(point(x, bounds.top()), size(width, bounds.size.height)),
            rgb(color),
        ));
    };
    match style {
        CursorStyle::Bar => vertical(window, bounds.left(), px(1.)),
        CursorStyle::Underline => paint_rule(
            window,
            bounds.left(),
            bounds.bottom() - px(1.),
            bounds.size.width,
            color,
        ),
        CursorStyle::BlockHollow => {
            paint_rule(
                window,
                bounds.left(),
                bounds.top(),
                bounds.size.width,
                color,
            );
            paint_rule(
                window,
                bounds.left(),
                bounds.bottom() - px(1.),
                bounds.size.width,
                color,
            );
            vertical(window, bounds.left(), px(1.));
            vertical(window, bounds.right() - px(1.), px(1.));
        }
        CursorStyle::Block => {}
    }
}

#[cfg(test)]
mod file_drop_tests {
    use super::*;

    #[test]
    fn posix_keeps_ordinary_paths_and_double_quotes_interior_spaces() {
        assert_eq!(quote_dropped_path("/tmp/a.txt", false), "/tmp/a.txt");
        assert_eq!(
            quote_dropped_path("/tmp/a  b/c.txt", false),
            "\"/tmp/a  b/c.txt\""
        );
    }

    #[test]
    fn posix_single_quotes_shell_specials_and_edge_spaces() {
        for path in [
            "/tmp/a$(id);&`x`.txt",
            "/tmp/a\"b.txt",
            "/tmp/a\\b.txt",
            "/tmp/a\nb.txt",
            "/tmp/文件.txt",
            " /tmp/leading.txt",
            "/tmp/trailing.txt ",
        ] {
            assert_eq!(quote_dropped_path(path, false), format!("'{path}'"));
        }
        assert_eq!(
            quote_dropped_path("/tmp/a'b.txt", false),
            "'/tmp/a'\\''b.txt'"
        );
    }

    #[test]
    fn windows_keeps_drive_and_unc_paths_and_double_quotes_interior_spaces() {
        for path in [
            r"D:\MatouTest\v18-matrix\drop-test.txt",
            r"\\server\share\file.txt",
        ] {
            assert_eq!(quote_dropped_path(path, true), path);
        }
        let with_spaces = r"D:\MatouTest\v18-matrix\drop test.txt";
        assert_eq!(
            quote_dropped_path(with_spaces, true),
            format!("\"{with_spaces}\"")
        );
    }

    #[test]
    fn windows_single_quotes_specials_and_doubles_embedded_single_quotes() {
        for path in [
            r"D:\MatouTest\a$(x);&`b.txt",
            r"D:\MatouTest\(archive).txt",
            r"D:\MatouTest\文件.txt",
        ] {
            assert_eq!(quote_dropped_path(path, true), format!("'{path}'"));
        }
        assert_eq!(
            quote_dropped_path(r"D:\MatouTest\a'b.txt", true),
            r"'D:\MatouTest\a''b.txt'"
        );
    }

    #[test]
    fn leading_equals_is_single_quoted_on_both_platforms() {
        for windows in [false, true] {
            assert_eq!(quote_dropped_path("=name.txt", windows), "'=name.txt'");
            assert_eq!(quote_dropped_path("=a b.txt", windows), "'=a b.txt'");
        }
    }

    #[test]
    fn formats_multiple_paths_in_order_and_ignores_empty_and_nul_paths() {
        for (windows, plain, spaced) in [
            (false, "/tmp/a.txt", "/tmp/b c.txt"),
            (true, r"D:\tmp\a.txt", r"D:\tmp\b c.txt"),
        ] {
            assert_eq!(quote_dropped_path("", windows), "");
            assert_eq!(quote_dropped_path("bad\0path", windows), "");
            assert_eq!(format_dropped_paths(&[], windows), "");
            assert_eq!(
                format_dropped_paths(&["", "bad\0path"].map(PathBuf::from), windows),
                ""
            );
            assert_eq!(
                format_dropped_paths(
                    &[plain, "bad\0path", "", spaced].map(PathBuf::from),
                    windows
                ),
                format!("{plain} \"{spaced}\"")
            );
        }
    }
}

#[cfg(test)]
mod resize_frame_tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};
    use matou_native_terminal::{Terminal, TerminalOptions};

    fn publish(view: &Entity<TerminalSurface>, cx: &mut TestAppContext, contents: &str) -> Screen {
        let mut terminal = Terminal::new(TerminalOptions::new(24, 4, 8, 16)).unwrap();
        terminal.feed(contents.as_bytes());
        let screen = terminal.screen().unwrap();
        let modes = terminal.input_modes().unwrap();
        view.update(cx, |view, cx| {
            view.set_screen(screen.clone(), modes, None, 0, cx)
        });
        screen
    }

    fn advance(cx: &mut TestAppContext, ms: u64) {
        cx.background_executor
            .advance_clock(Duration::from_millis(ms));
        cx.run_until_parked();
    }

    fn displayed(view: &Entity<TerminalSurface>, cx: &TestAppContext, screen: &Screen) {
        cx.read_entity(view, |view, _| {
            assert_eq!(view.screen.as_ref(), Some(screen))
        });
    }

    #[test]
    fn resize_shows_only_the_last_redraw_frame() {
        let mut cx = TestAppContext::single();
        let view = cx.new(TerminalSurface::new);
        let old = publish(&view, &mut cx, "status\r\nold body");
        view.update(&mut cx, TerminalSurface::begin_resize);
        publish(&view, &mut cx, "status");
        displayed(&view, &cx, &old);
        advance(&mut cx, 80);
        publish(&view, &mut cx, "status\r\npartial");
        advance(&mut cx, 80);
        let latest = publish(&view, &mut cx, "status\r\ncomplete body");
        advance(&mut cx, 99);
        displayed(&view, &cx, &old);
        advance(&mut cx, 1);
        displayed(&view, &cx, &latest);
    }

    #[test]
    fn resize_presents_a_frame_after_100ms_of_quiet() {
        let mut cx = TestAppContext::single();
        let view = cx.new(TerminalSurface::new);
        let old = publish(&view, &mut cx, "old body");
        view.update(&mut cx, TerminalSurface::begin_resize);
        advance(&mut cx, 140);
        let blank = publish(&view, &mut cx, "");
        advance(&mut cx, 99);
        displayed(&view, &cx, &old);
        advance(&mut cx, 1);
        displayed(&view, &cx, &blank);
    }

    #[test]
    fn resize_presents_the_latest_frame_at_the_500ms_limit() {
        let mut cx = TestAppContext::single();
        let view = cx.new(TerminalSurface::new);
        let old = publish(&view, &mut cx, "old body");
        view.update(&mut cx, TerminalSurface::begin_resize);
        let mut latest = publish(&view, &mut cx, "frame 0");
        for index in 1..=6 {
            advance(&mut cx, 80);
            latest = publish(&view, &mut cx, &format!("frame {index}"));
            displayed(&view, &cx, &old);
        }
        advance(&mut cx, 19);
        displayed(&view, &cx, &old);
        advance(&mut cx, 1);
        displayed(&view, &cx, &latest);
    }

    #[test]
    fn ordinary_frames_present_immediately_including_clear() {
        let mut cx = TestAppContext::single();
        let view = cx.new(TerminalSurface::new);
        let initial = publish(&view, &mut cx, "visible body");
        displayed(&view, &cx, &initial);
        let blank = publish(&view, &mut cx, "");
        displayed(&view, &cx, &blank);
    }

    #[test]
    fn first_frame_is_immediate_after_initial_resize() {
        let mut cx = TestAppContext::single();
        let view = cx.new(TerminalSurface::new);
        view.update(&mut cx, TerminalSurface::begin_resize);
        let blank = publish(&view, &mut cx, "");
        displayed(&view, &cx, &blank);
    }

    #[test]
    fn consecutive_resize_restarts_the_window_from_the_latest_command() {
        let mut cx = TestAppContext::single();
        let view = cx.new(TerminalSurface::new);
        let old = publish(&view, &mut cx, "old body");
        view.update(&mut cx, TerminalSurface::begin_resize);
        publish(&view, &mut cx, "first resize");
        advance(&mut cx, 80);
        view.update(&mut cx, TerminalSurface::begin_resize);
        advance(&mut cx, 80);
        displayed(&view, &cx, &old);

        let mut latest = publish(&view, &mut cx, "second resize 1");
        for index in 2..=6 {
            advance(&mut cx, 80);
            latest = publish(&view, &mut cx, &format!("second resize {index}"));
            displayed(&view, &cx, &old);
        }
        advance(&mut cx, 19);
        displayed(&view, &cx, &old);
        advance(&mut cx, 1);
        displayed(&view, &cx, &latest);
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn font_zoom_only_uses_line_wheel_with_the_platform_modifier() {
        use gpui::{Modifiers, ScrollDelta};

        let up = ScrollDelta::Lines(point(0., 1.));
        let down = ScrollDelta::Lines(point(0., -1.));
        let horizontal = ScrollDelta::Lines(point(1., 0.));
        let precise = ScrollDelta::Pixels(point(px(0.), px(12.)));
        for is_mac in [true, false] {
            let modifier = Modifiers {
                platform: is_mac,
                control: !is_mac,
                ..Modifiers::default()
            };
            let with_alt = Modifiers {
                alt: true,
                ..modifier
            };
            assert_eq!(
                font_zoom_for_wheel(up, modifier, is_mac),
                Some(FontZoom::In)
            );
            assert_eq!(
                font_zoom_for_wheel(down, modifier, is_mac),
                Some(FontZoom::Out)
            );
            assert_eq!(font_zoom_for_wheel(horizontal, modifier, is_mac), None);
            assert_eq!(font_zoom_for_wheel(precise, modifier, is_mac), None);
            assert_eq!(font_zoom_for_wheel(up, with_alt, is_mac), None);
            assert_eq!(font_zoom_for_wheel(up, Modifiers::default(), is_mac), None);
            assert_eq!(font_zoom_for_wheel(up, modifier, !is_mac), None);
        }
    }

    #[test]
    fn font_shortcuts_use_command_on_mac_and_control_on_windows() {
        let mut stroke = gpui::Keystroke::parse("cmd-=").unwrap();
        assert_eq!(font_zoom_for_key(&stroke, true), Some(FontZoom::In));
        assert_eq!(font_zoom_for_key(&stroke, false), None);

        stroke.modifiers.platform = false;
        stroke.modifiers.control = true;
        for (key, action) in [
            ("=", FontZoom::In),
            ("+", FontZoom::In),
            ("-", FontZoom::Out),
            ("_", FontZoom::Out),
            ("0", FontZoom::Reset),
        ] {
            stroke.key = key.into();
            assert_eq!(font_zoom_for_key(&stroke, false), Some(action));
            assert_eq!(font_zoom_for_key(&stroke, true), None);
        }
        stroke.modifiers.alt = true;
        assert_eq!(font_zoom_for_key(&stroke, false), None);
    }
}
