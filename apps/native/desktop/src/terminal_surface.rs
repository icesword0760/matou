use std::{ops::Range, time::Duration};

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle as PointerStyle,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Font, FontStyle,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels,
    Point, Render, ScrollWheelEvent, ShapedLine, SharedString, Styled, Task, TextRun,
    UTF16Selection, Window, canvas, div, fill, font, point, px, rgb, size,
};
use matou_native_terminal::{Cell, Color, CursorStyle, InputModes, Screen, Theme};

use crate::terminal_input::{
    Composition, GridPoint, GridSelection, byte_from_utf16, encode_paste, grid_boundary,
    terminal_key, utf16_from_byte,
};

const FONT_SIZE: Pixels = px(11.);
const BLINK_INTERVAL: Duration = Duration::from_millis(500);
const FAMILIES: &[&str] = &["SF Mono", "Monaco", "Menlo", "Consolas", "Courier New"];

pub enum SurfaceEvent {
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    /// Positive lines move into scrollback.
    Scroll(i32),
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
            cols: f32::from(bounds.size.width / self.width).floor().max(2.) as u16,
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

pub struct TerminalSurface {
    pub focus: FocusHandle,
    font: Font,
    metrics: Metrics,
    last_dimensions: Option<Dimensions>,
    screen: Option<Screen>,
    modes: InputModes,
    theme: Theme,
    grid: Option<VisibleGrid>,
    selection: Option<GridSelection>,
    selecting: bool,
    composition: Composition,
    caret: Option<Bounds<Pixels>>,
    wheel_lines: f32,
    blink: Option<Task<()>>,
    blink_visible: bool,
    input_disabled: bool,
}

impl EventEmitter<SurfaceEvent> for TerminalSurface {}

impl TerminalSurface {
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
            },
            theme: Theme::default(),
            grid: None,
            selection: None,
            selecting: false,
            composition: Composition::default(),
            caret: None,
            wheel_lines: 0.,
            blink: None,
            blink_visible: true,
            input_disabled: false,
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

    pub fn set_screen(&mut self, screen: Screen, modes: InputModes, cx: &mut Context<Self>) {
        self.modes = modes;
        if self.screen.as_ref() == Some(&screen) {
            return;
        }
        if let Some(old) = &self.screen {
            let grid_changed = old.cols != screen.cols
                || old.rows != screen.rows
                || old.alternate != screen.alternate
                || old.scrollbar.offset != screen.scrollbar.offset;
            let cursor_changed = old.cursor_col != screen.cursor_col
                || old.cursor_row != screen.cursor_row
                || old.cursor_visible != screen.cursor_visible
                || old.cursor_style != screen.cursor_style
                || old.cursor_blinking != screen.cursor_blinking;
            if grid_changed {
                self.clear_selection();
            }
            if cursor_changed {
                self.reset_blink();
            }
        }
        self.screen = Some(screen);
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
        self.clear_selection();
        self.reset_blink();
        cx.emit(SurfaceEvent::Input(bytes));
        cx.notify();
    }

    fn reset_blink(&mut self) {
        self.blink = None;
        self.blink_visible = true;
    }

    fn schedule_blink(&mut self, focused: bool, cx: &mut Context<Self>) {
        let blink = focused
            && self.screen.as_ref().is_some_and(|screen| {
                screen.cursor_visible && screen.cursor_has_position && screen.cursor_blinking
                    || screen.cells.iter().any(|cell| cell.blink)
            });
        if !blink {
            self.reset_blink();
        } else if self.blink.is_none() {
            let timer = cx.background_executor().timer(BLINK_INTERVAL);
            self.blink = Some(cx.spawn(async move |this, cx| {
                timer.await;
                let _ = this.update(cx, |this, cx| {
                    this.blink = None;
                    this.blink_visible = !this.blink_visible;
                    cx.notify();
                });
            }));
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.composition.is_active() {
            cx.stop_propagation();
            return;
        }
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        let mac = cfg!(target_os = "macos");
        let copy_mod = if mac { mods.platform } else { mods.control };
        if key == "c" && copy_mod && !mods.alt && !mods.shift {
            let copied = self.selection_text();
            if mac || !copied.is_empty() {
                if !copied.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(copied));
                }
                cx.stop_propagation();
                return;
            }
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
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
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

    fn selection_text(&self) -> String {
        match (self.selection, self.screen.as_ref(), self.grid) {
            (Some(selection), Some(screen), Some(grid)) => selection.text(screen, grid.cols),
            _ => String::new(),
        }
    }

    fn clear_selection(&mut self) {
        self.selection = None;
        self.selecting = false;
    }

    fn selection_point(&self, position: Point<Pixels>) -> Option<GridPoint> {
        let screen = self.screen.as_ref()?;
        let grid = self.grid?;
        let row =
            f32::from((position.y - grid.bounds.top()) / self.metrics.height).max(0.) as usize;
        let x = f32::from((position.x - grid.bounds.left()) / self.metrics.width);
        Some(grid_boundary(screen, row.min(grid.rows - 1), x, grid.cols))
    }

    fn update_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.selecting
            && let Some(end) = self.selection_point(position)
            && let Some(selection) = &mut self.selection
            && selection.end != end
        {
            selection.end = end;
            cx.notify();
        }
    }

    pub fn contains_position(&self, position: Point<Pixels>) -> bool {
        self.grid
            .is_some_and(|grid| grid.bounds.contains(&position))
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
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
            self.wheel_lines -= lines as f32;
            cx.emit(SurfaceEvent::Scroll(lines));
        }
        cx.stop_propagation();
    }

    fn paint(
        &mut self,
        entity: Entity<Self>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let metrics = self.metrics;
        let dimensions = metrics.dimensions(bounds, window.scale_factor());
        if bounds.size.width >= metrics.width * 2.
            && bounds.size.height >= metrics.height
            && self.last_dimensions.replace(dimensions) != Some(dimensions)
        {
            let surface = entity.downgrade();
            window.defer(cx, move |_, cx| {
                let _ = surface.update(cx, |_, cx| {
                    cx.emit(SurfaceEvent::Resize {
                        cols: dimensions.cols,
                        rows: dimensions.rows,
                        cell_width_px: dimensions.cell_width_px,
                        cell_height_px: dimensions.cell_height_px,
                    });
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
        let caret = screen
            .filter(|screen| screen.cursor_has_position)
            .map_or_else(
                || metrics.cell(bounds.origin, 0, 0, 1),
                |screen| {
                    metrics.cell(
                        bounds.origin,
                        screen.cursor_col.min(cols.saturating_sub(1)),
                        screen.cursor_row.min(rows.saturating_sub(1)),
                        1,
                    )
                },
            );
        self.caret = Some(caret);
        if !self.input_disabled {
            window.handle_input(&self.focus, ElementInputHandler::new(caret, entity), cx);
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
            if focused && self.composition.is_active() {
                self.paint_composition(caret, window, cx);
            }
        });
    }

    fn paint_composition(&self, caret: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let line = self.shape_composition(window);
        let marked = &self.composition.text;
        let selection = &self.composition.selection;
        let left = line.x_for_index(byte_from_utf16(marked, selection.start));
        let right = line.x_for_index(byte_from_utf16(marked, selection.end));
        let width = line.width.max(caret.size.width);
        let base = self
            .screen
            .as_ref()
            .map_or(self.theme.background, |screen| screen.background);
        window.paint_quad(fill(
            Bounds::new(caret.origin, size(width, caret.size.height)),
            rgb(base),
        ));
        if left != right {
            window.paint_quad(fill(
                Bounds::new(
                    point(caret.left() + left, caret.top()),
                    size(right - left, caret.size.height),
                ),
                rgb(self.theme.selection_background),
            ));
        }
        let _ = line.paint(caret.origin, caret.size.height, window, cx);
        paint_rule(
            window,
            caret.left(),
            caret.bottom() - px(1.),
            width,
            self.theme.cursor,
        );
        if left == right {
            window.paint_quad(fill(
                Bounds::new(
                    point(caret.left() + left, caret.top()),
                    size(px(1.), caret.size.height),
                ),
                rgb(self.theme.cursor),
            ));
        }
    }

    fn shape_composition(&self, window: &mut Window) -> ShapedLine {
        let fg = self
            .screen
            .as_ref()
            .map_or(self.theme.foreground, |screen| screen.foreground);
        shape(
            self.composition.text.replace(['\n', '\r'], " ").into(),
            &self.font,
            fg,
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
                let range = selection.range(row, cols.min(cells.len()));
                if !range.is_empty() {
                    let bounds = self.metrics.cell(origin, range.start, row, range.len());
                    window.paint_quad(fill(bounds, rgb(self.theme.selection_background)));
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
        self.schedule_blink(
            self.focus.is_focused(window) && window.is_window_active(),
            cx,
        );
        let surface = cx.entity();
        div()
            .size_full()
            .overflow_hidden()
            .cursor(PointerStyle::IBeam)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.handle_key(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    this.focus.focus(window);
                    this.reset_blink();
                    this.clear_selection();
                    if this
                        .grid
                        .is_some_and(|grid| grid.bounds.contains(&event.position))
                        && let Some(anchor) = this.selection_point(event.position)
                    {
                        this.selection = Some(GridSelection {
                            anchor,
                            end: anchor,
                        });
                        this.selecting = true;
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if event.dragging() {
                    this.update_selection(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    this.update_selection(event.position, cx);
                    this.selecting = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    this.update_selection(event.position, cx);
                    this.selecting = false;
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.on_scroll(event, cx)))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        surface.update(cx, |this, cx| {
                            this.paint(surface.clone(), bounds, window, cx)
                        });
                    },
                )
                .size_full(),
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
