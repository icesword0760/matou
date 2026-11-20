use std::{
    ops::Range,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use core_graphics::{
    event::{CGEventType, CGMouseButton},
    event_source::CGEventSourceStateID,
};
use gpui::{
    App, Bounds, ContentMask, Context, CursorStyle as PointerStyle, DispatchPhase,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Font, FontStyle,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels,
    Point, Render, ScrollWheelEvent, ShapedLine, SharedString, StatefulInteractiveElement, Styled,
    Task, TextRun, UTF16Selection, Window, canvas, div, fill, font, point, px, rgb, rgba, size,
};
use matou_native_terminal::{
    Cell, Color, CursorStyle, GridSelection, InputModes, MouseEncoding, MouseEvent, MouseProtocol,
    MouseReporter, MouseWheel, Screen, SelectionAction, SelectionMode, Theme,
};

use crate::terminal_input::{
    Composition, byte_from_utf16, encode_paste, read_clipboard_text, selection_scroll_amount,
    terminal_key, utf16_from_byte,
};
use crate::terminal_scrollbar::ScrollbarMetrics;

const FONT_SIZE: Pixels = px(11.);
const BLINK_INTERVAL: Duration = Duration::from_millis(500);
const FAMILIES: &[&str] = &["SF Mono", "Monaco", "Menlo", "Consolas", "Courier New"];

pub struct SurfaceEvent {
    pub generation: u64,
    pub kind: SurfaceEventKind,
}
pub enum SurfaceEventKind {
    Input(Vec<u8>),
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

pub struct TerminalSurface {
    transport_generation: u64,
    transport_suspended: bool,
    search: Option<matou_native_runtime::search::SearchResult>,
    pub focus: FocusHandle,
    font: Font,
    metrics: Metrics,
    last_dimensions: Option<Dimensions>,
    screen: Option<Screen>,
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
    wheel_lines: f32,
    mouse_reporter: MouseReporter,
    mouse_wheel: MouseWheel,
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
        self.mouse_buttons = 0;
        self.wheel_lines = 0.;
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
        let id = text.resolve_font(&face);
        let width = text
            .ch_advance(id, FONT_SIZE)
            .unwrap_or_else(|_| text.bounding_box(id, FONT_SIZE).size.width);
        // font-kit reports descenders below the baseline as a negative value.
        // Adding it makes rows shorter than the glyphs and overlaps text.
        let height = text.ascent(id, FONT_SIZE) - text.descent(id, FONT_SIZE);
        Self {
            transport_generation: 0,
            transport_suspended: false,
            search: None,
            focus: cx.focus_handle().tab_stop(true),
            font: face,
            metrics: Metrics {
                width: width.max(px(1.)),
                height: px(f32::from(height).ceil()).max(px(1.)),
            },
            last_dimensions: None,
            screen: None,
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
            wheel_lines: 0.,
            mouse_reporter: MouseReporter::default(),
            mouse_wheel: MouseWheel::default(),
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
        }
    }

    pub fn set_input_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.input_disabled == disabled {
            return;
        }
        self.input_disabled = disabled;
        if disabled {
            self.composition.clear();
            self.reset_blink();
        }
        cx.notify();
    }

    pub fn set_screen(
        &mut self,
        screen: Screen,
        modes: InputModes,
        selection: Option<GridSelection>,
        scroll_request_id: u64,
        cx: &mut Context<Self>,
    ) {
        self.scroll_request_id = self.scroll_request_id.max(scroll_request_id);
        let modes_changed = self.modes != modes;
        if self.modes.input_epoch != modes.input_epoch {
            self.scrollbar_drag = None;
            self.scrollbar_position = None;
            self.mouse_reporter = MouseReporter::default();
            self.mouse_wheel = MouseWheel::default();
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
            self.scrollbar_position = None;
        }
        if !modes_changed && self.screen.as_ref() == Some(&screen) && self.selection == selection {
            if clear_position {
                cx.notify();
            }
            return;
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

    fn send(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if self.input_disabled || bytes.is_empty() {
            return;
        }
        self.selection_drag = None;
        self.stop_drag();
        self.reset_blink();
        self.scroll_on_input(cx);
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

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.composition.is_active() {
            self.scroll_on_input(cx);
            cx.stop_propagation();
            return;
        }
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
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
        if self.input_disabled {
            return;
        }
        let paste_mod = if mac {
            mods.platform && !mods.control
        } else {
            mods.control && !mods.platform
        };
        if key == "v" && paste_mod && !mods.alt {
            if let Some(text) = read_clipboard_text(cx) {
                self.send(encode_paste(&text, self.modes), cx);
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
                    screen.scrollbar.offset as f64 * f64::from(f32::from(self.metrics.height)),
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
    ) {
        let Some(grid) = self.grid else {
            return;
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
        }
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
        let reports_wheel = matches!(
            self.modes.mouse_protocol,
            MouseProtocol::Vt200 | MouseProtocol::Drag | MouseProtocol::Any
        );
        let alternate = self.screen.as_ref().is_some_and(|screen| screen.alternate);
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
                if reports_wheel {
                    self.report_mouse(
                        event.position,
                        event.modifiers,
                        4,
                        if delta < 0. { 0 } else { 1 },
                        cx,
                    );
                } else {
                    let prefix = if self.modes.application_cursor_keys {
                        b'O'
                    } else {
                        b'['
                    };
                    self.send(vec![27, prefix, if delta < 0. { b'A' } else { b'B' }], cx);
                }
            }
            cx.stop_propagation();
            return;
        }

        if event.modifiers.control || event.modifiers.platform {
            return;
        }
        let delta = f32::from(event.delta.pixel_delta(self.metrics.height).y / self.metrics.height);
        if delta == 0. {
            return;
        }
        if delta.signum() != self.wheel_lines.signum() {
            self.wheel_lines = 0.;
        }
        self.wheel_lines += delta;
        let lines = self.wheel_lines.trunc() as i32;
        if lines != 0 {
            self.scrollbar_position = None;
            self.wheel_lines -= lines as f32;
            self.emit(SurfaceEventKind::Scroll(lines), cx);
        }
        cx.stop_propagation();
    }

    fn scrollbar_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if event.modifiers.control || event.modifiers.platform {
            return;
        }
        let Some(metrics) = self.scrollbar_metrics() else {
            return;
        };
        let position = self
            .scrollbar_position
            .map_or(metrics.position, |position| position.pixels);
        // Match the viewport's 50px per wheel unit. A precise unit is 40px.
        let delta = match event.delta {
            gpui::ScrollDelta::Pixels(delta) => f64::from(f32::from(delta.y)) * (50. / 40.),
            gpui::ScrollDelta::Lines(delta) => f64::from(delta.y) * 50.,
        } * if event.modifiers.alt { 5. } else { 1. };
        if delta == 0. {
            return;
        }
        let movement = delta.signum() * delta.abs().ceil();
        self.scroll_to_pixels(metrics.clamp(position - movement), cx);
        cx.stop_propagation();
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
                        let line = shape(cell.text.clone().into(), fonts.face(cell), fg, window);
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
        div()
            .id("terminal-surface")
            .size_full()
            .relative()
            .px(px(14.))
            .py(px(12.))
            .overflow_hidden()
            .cursor(if self.modes.mouse_protocol == MouseProtocol::None {
                PointerStyle::IBeam
            } else {
                PointerStyle::Arrow
            })
            .track_focus(&self.focus)
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
            .on_key_down(cx.listener(move |this, event, _, cx| {
                if this.transport_generation == generation && !this.transport_suspended {
                    this.handle_key(event, cx);
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
                                            && this.contains_position(event.position);
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

fn shape(text: SharedString, face: &Font, fg: u32, window: &mut Window) -> ShapedLine {
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
        .shape_line(text, FONT_SIZE, &[run], None)
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
