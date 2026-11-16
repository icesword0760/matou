use std::{cell::Cell, ops::Range, rc::Rc};

use gpui::{prelude::*, *};
use matou_native_runtime::history::HistoryPage;

use unicode_segmentation::UnicodeSegmentation;

use crate::ui_text::{TextGeometryHandle, into_text_element, ui_text};

pub enum HistoryViewState {
    Loading {
        result_index: usize,
        result_count: usize,
        gap_count: usize,
    },
    Ready {
        result_index: usize,
        result_count: usize,
        page: HistoryPage,
    },
}

pub enum HistoryViewEvent {
    ReturnToLive,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TextPoint {
    row: usize,
    byte: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TextSelection {
    anchor: TextPoint,
    head: TextPoint,
}

impl TextSelection {
    fn ordered(self) -> Range<TextPoint> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

#[derive(Clone, Copy)]
enum SelectionUnit {
    Character,
    Word,
    Line,
}

struct SelectionDrag {
    initial: Range<TextPoint>,
    unit: SelectionUnit,
    position: Option<Point<Pixels>>,
}

#[derive(Default)]
struct RowGeometry {
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    text: TextGeometryHandle,
}

/// A read-only overlay. The live surface remains owned by the workspace.
pub struct HistoryView {
    pub focus: FocusHandle,
    text_focus: FocusHandle,
    state: Option<HistoryViewState>,
    rows: Vec<RowGeometry>,
    selection: Option<TextSelection>,
    drag: Option<SelectionDrag>,
    drag_frame_pending: bool,
    interaction_subscriptions: Option<[Subscription; 2]>,
    scroll: ScrollHandle,
    center_on_anchor: bool,
    font_family: SharedString,
}

impl EventEmitter<HistoryViewEvent> for HistoryView {}

impl HistoryView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let installed = cx.text_system().all_font_names();
        let font_family = ["SFMono-Regular", "Menlo", "Courier New"]
            .into_iter()
            .find(|family| {
                installed
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(family))
            })
            .unwrap_or("monospace");
        Self {
            focus: cx.focus_handle().tab_stop(true),
            text_focus: cx.focus_handle(),
            state: None,
            rows: Vec::new(),
            selection: None,
            drag: None,
            drag_frame_pending: false,
            interaction_subscriptions: None,
            scroll: ScrollHandle::new(),
            center_on_anchor: false,
            font_family: font_family.into(),
        }
    }

    pub fn set_state(&mut self, state: HistoryViewState, cx: &mut Context<Self>) {
        self.center_on_anchor =
            matches!(&state, HistoryViewState::Ready { page, .. } if page.anchor_index.is_some());
        self.scroll.set_offset(Point::default());
        self.rows.clear();
        if let HistoryViewState::Ready { page, .. } = &state {
            self.rows
                .resize_with(page.lines.len(), RowGeometry::default);
        }
        self.selection = None;
        self.stop_drag();
        self.state = Some(state);
        cx.notify();
    }

    fn page(&self) -> Option<&HistoryPage> {
        match &self.state {
            Some(HistoryViewState::Ready { page, .. }) => Some(page),
            _ => None,
        }
    }

    fn point_at(
        &self,
        position: Point<Pixels>,
        unit: SelectionUnit,
        starting: bool,
    ) -> Option<TextPoint> {
        let page = self.page()?;
        let last = self.rows.len().checked_sub(1)?;
        let row = self
            .rows
            .partition_point(|row| {
                row.bounds
                    .get()
                    .is_some_and(|bounds| position.y >= bounds.bottom())
            })
            .min(last);
        let geometry = self.rows[row].text.borrow();
        let geometry = geometry.as_ref()?;
        let row_bounds = self.rows[row].bounds.get()?;
        if starting
            && (position.x < geometry.bounds.left()
                || position.y < row_bounds.top()
                || position.y >= row_bounds.bottom())
        {
            return None;
        }

        let source = &page.lines[row].text;
        let x = position.x - geometry.bounds.left();
        let byte = if x <= px(0.) {
            0
        } else if let Some(glyph) = geometry.layout.index_for_x(x) {
            let (start, grapheme) = source
                .grapheme_indices(true)
                .take_while(|(index, _)| *index <= glyph)
                .last()
                .unwrap_or((0, ""));
            let end = start + grapheme.len();
            if matches!(unit, SelectionUnit::Character)
                && x - geometry.layout.x_for_index(start) >= geometry.layout.x_for_index(end) - x
            {
                end
            } else {
                start
            }
        } else {
            source.len()
        };
        Some(TextPoint { row, byte })
    }

    fn unit_range(&self, point: TextPoint, unit: SelectionUnit) -> Range<TextPoint> {
        let Some(text) = self
            .page()
            .and_then(|page| page.lines.get(point.row))
            .map(|line| line.text.as_str())
        else {
            return point..point;
        };
        let bytes = match unit {
            SelectionUnit::Character => point.byte..point.byte,
            SelectionUnit::Line => 0..text.len(),
            SelectionUnit::Word => text
                .split_word_bound_indices()
                .find_map(|(start, word)| {
                    let end = start + word.len();
                    (start <= point.byte && point.byte < end).then_some(start..end)
                })
                .unwrap_or(point.byte..point.byte),
        };
        TextPoint {
            byte: bytes.start,
            ..point
        }..TextPoint {
            byte: bytes.end,
            ..point
        }
    }

    fn begin_selection(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unit = if event.modifiers.shift {
            SelectionUnit::Character
        } else {
            match event.click_count {
                0 | 1 => SelectionUnit::Character,
                2 => SelectionUnit::Word,
                _ => SelectionUnit::Line,
            }
        };
        let Some(point) = self.point_at(event.position, unit, true) else {
            return;
        };
        window.focus(&self.text_focus);
        let initial = if let Some(selection) = self.selection.filter(|_| event.modifiers.shift) {
            self.selection = Some(TextSelection {
                anchor: selection.anchor,
                head: point,
            });
            selection.anchor..selection.anchor
        } else {
            let range = self.unit_range(point, unit);
            self.selection = Some(TextSelection {
                anchor: range.start,
                head: range.end,
            });
            range
        };
        self.drag = Some(SelectionDrag {
            initial,
            unit,
            position: None,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub fn stop_drag(&mut self) {
        self.drag = None;
    }

    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        drag.position = Some(position);
        self.update_selection(position);
        cx.stop_propagation();
        cx.notify();
    }

    fn update_selection(&mut self, position: Point<Pixels>) {
        let Some(drag) = &self.drag else { return };
        let Some(point) = self.point_at(position, drag.unit, false) else {
            return;
        };
        let range = self.unit_range(point, drag.unit);
        self.selection = Some(if point < drag.initial.start {
            TextSelection {
                anchor: drag.initial.end,
                head: range.start,
            }
        } else {
            TextSelection {
                anchor: drag.initial.start,
                head: drag.initial.end.max(range.end),
            }
        });
    }

    fn schedule_drag_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag_frame_pending || self.drag.as_ref().and_then(|drag| drag.position).is_none() {
            return;
        }
        self.drag_frame_pending = true;
        let view = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let _ = view.update(cx, |this, cx| {
                this.drag_frame_pending = false;
                if !window.is_window_active() || !this.text_focus.is_focused(window) {
                    this.stop_drag();
                    return;
                }
                this.advance_drag_frame(window.scale_factor(), cx);
            });
        });
    }

    fn advance_drag_frame(&mut self, scale: f32, cx: &mut Context<Self>) {
        let Some(position) = self.drag.as_ref().and_then(|drag| drag.position) else {
            return;
        };
        let selection = self.selection;
        // Chromium updates the selection from the last painted layout
        // before scrolling; the following frame then sees the new rows.
        self.update_selection(position);
        let offset = self.scroll.offset();
        let next = history_drag_offset(
            self.scroll.bounds(),
            position,
            offset,
            self.scroll.max_offset(),
            scale,
        );
        if next != offset {
            self.scroll.set_offset(next);
        }
        if next != offset || selection != self.selection {
            cx.notify();
        }
    }

    fn copy(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        let shortcut = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        };
        if event.keystroke.key != "c" || !shortcut || modifiers.alt || modifiers.shift {
            return;
        }
        cx.stop_propagation();
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn selected_text(&self) -> Option<String> {
        let page = self.page()?;
        let selection = self.selection?.ordered();
        if selection.is_empty() {
            return None;
        }

        let mut text = String::new();
        for row in selection.start.row..=selection.end.row {
            if row != selection.start.row {
                text.push('\n');
            }
            let line = &page.lines[row].text;
            if let Some(range) = selected_bytes(&selection, row, line.len()) {
                text.push_str(&line[range]);
            }
        }
        Some(text)
    }

    fn heading(&self, result_index: usize, result_count: usize, cx: &mut Context<Self>) -> Div {
        let focus = self.focus.clone();
        let return_button = div()
            .id("history-return-to-live")
            .relative()
            .flex_none()
            .px(px(9.))
            .py(px(4.))
            .border_1()
            .border_color(rgba(0x30384829))
            .rounded(px(6.))
            .bg(rgba(0x3038480d))
            .hover(|style| style.bg(rgba(0x30384817)))
            .cursor_pointer()
            .whitespace_nowrap()
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() {
                    cx.stop_propagation();
                    cx.emit(HistoryViewEvent::ReturnToLive);
                }
            }))
            .on_click(cx.listener(|_, event, _, cx| {
                if !matches!(event, ClickEvent::Keyboard(event) if event.button == KeyboardButton::Enter)
                {
                    cx.stop_propagation();
                    cx.emit(HistoryViewEvent::ReturnToLive);
                }
            }))
            .child(ui_text("返回实时终端"))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        if focus.is_focused(window) {
                            window.paint_quad(
                                outline(bounds.dilate(px(3.)), rgb(0x548cff), BorderStyle::default())
                                    .corner_radii(px(9.))
                                    .border_widths(px(2.)),
                            );
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            );

        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(12.))
            .py(px(9.))
            .border_b_1()
            .border_color(rgba(0x3038481f))
            .bg(rgb(0xeef1f6))
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap(px(9.))
                    .whitespace_nowrap()
                    .child(
                        div()
                            .text_color(rgb(0x202735))
                            .font_weight(FontWeight::BOLD)
                            .child(ui_text("历史记录")),
                    )
                    .child(
                        div()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .text_color(rgb(0xc98a39))
                            .bg(rgba(0xc98a3921))
                            .child(ui_text("只读")),
                    )
                    .child(format!("{}/{result_count}", result_index + 1)),
            )
            .child(return_button)
    }

    fn body(&self, page: &HistoryPage, cx: &mut Context<Self>) -> Stateful<Div> {
        let view = cx.entity().downgrade();
        let selection = self.selection.map(TextSelection::ordered);
        let rows = page.lines.iter().enumerate().map(|(index, line)| {
            let current = page.anchor_index == Some(index);
            let text = if line.text.is_empty() {
                " "
            } else {
                &line.text
            };
            let geometry = &self.rows[index];
            let row_bounds = geometry.bounds.clone();
            let layout = geometry.text.clone();
            let selection = selection
                .as_ref()
                .and_then(|selection| selected_bytes(selection, index, line.text.len()));
            let text = if text.is_ascii() {
                into_text_element(StyledText::new(text.to_owned()), Some(layout.clone()))
            } else {
                ui_text(text.to_owned())
                    .track_geometry(layout.clone())
                    .into_any_element()
            };
            let view = view.clone();
            div()
                .relative()
                .flex()
                .flex_none()
                .items_start()
                .min_w_full()
                .min_h(px(20.))
                .pr(px(12.))
                .whitespace_nowrap()
                .when(current, |row| {
                    row.bg(rgba(0xd392362b)).text_color(rgb(0x4f3814)).child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(px(3.))
                            .bg(rgb(0xd39236)),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .w(px(56.))
                        .justify_end()
                        .pr(px(12.))
                        .text_color(rgb(0x626b7c))
                        .child((index + 1).to_string()),
                )
                .child(
                    canvas(
                        move |bounds, _, _| row_bounds.set(Some(bounds)),
                        move |_, _, window, _| {
                            let Some(selection) =
                                selection.filter(|selection| !selection.is_empty())
                            else {
                                return;
                            };
                            let layout = layout.borrow();
                            let Some(geometry) = layout.as_ref() else {
                                return;
                            };
                            let a = geometry.layout.x_for_index(selection.start);
                            let b = geometry.layout.x_for_index(selection.end);
                            let bounds = Bounds::new(
                                point(geometry.bounds.left() + a.min(b), geometry.bounds.top()),
                                size((b - a).abs(), geometry.bounds.size.height),
                            );
                            window.paint_quad(fill(bounds, rgb(0xb5d5ff)));
                        },
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                )
                .child(
                    div()
                        .flex_none()
                        .min_w_0()
                        .cursor(CursorStyle::IBeam)
                        .child(text),
                )
                .when(current && self.center_on_anchor, |row| {
                    row.child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, _, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    if !this.center_on_anchor {
                                        return;
                                    }
                                    let viewport = this.scroll.bounds();
                                    if viewport.size.height <= px(0.) {
                                        return;
                                    }
                                    this.center_on_anchor = false;
                                    let offset = this.scroll.offset();
                                    let y = (offset.y + viewport.center().y - bounds.center().y)
                                        .clamp(-this.scroll.max_offset().height, px(0.));
                                    if y != offset.y {
                                        this.scroll.set_offset(point(offset.x, y));
                                        cx.notify();
                                    }
                                });
                            },
                        )
                        .absolute()
                        .inset_0()
                        .size_full(),
                    )
                })
        });

        div()
            .id("history-context-scroll")
            .flex()
            .flex_col()
            .items_start()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .track_focus(&self.text_focus)
            .on_key_down(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_selection))
            .py(px(8.))
            .children((page.has_more_before == Some(true)).then(|| boundary("上方还有更早记录")))
            .children(rows)
            .children((page.has_more_after == Some(true)).then(|| boundary("下方还有更新记录")))
    }
}

// Frozen Chromium 150's ordinary DOM selection scrolls a 1-device-pixel
// target to the nearest edge each frame. The 20px belt is in Blink space,
// so it is 10 logical pixels on the reference 2x display. This deliberately
// differs from the terminal grid's 50ms, line-based drag scrolling.
fn history_drag_offset(
    viewport: Bounds<Pixels>,
    position: Point<Pixels>,
    offset: Point<Pixels>,
    maximum: Size<Pixels>,
    scale: f32,
) -> Point<Pixels> {
    fn axis(
        start: Pixels,
        size: Pixels,
        pointer: Pixels,
        offset: Pixels,
        maximum: Pixels,
        scale: f32,
    ) -> Pixels {
        // Blink layout geometry and scroll-origin conversions use 1/64px.
        let start = (f32::from(start) * scale * 64.).trunc() / 64.;
        let size = (f32::from(size) * scale * 64.).trunc() / 64.;
        let mut pointer = (f32::from(pointer) * scale).floor();
        if pointer < start.floor() + 20. {
            pointer -= 20.;
        } else if pointer > (start + size).ceil() - 20. {
            pointer += 20.;
        }
        let current = -f32::from(offset) * scale;
        let visible_start = (current * 64.).round() / 64.;
        let target = pointer - start + (current * 64.).floor() / 64.;
        let next = if target < visible_start {
            target
        } else if target + 1. > visible_start + size {
            target + 1. - size
        } else {
            current
        };
        // gfx rounds halves toward positive infinity, then clamps in device pixels.
        px(-(next + 0.5)
            .floor()
            .clamp(0., (f32::from(maximum) * scale).trunc())
            / scale)
    }
    point(
        axis(
            viewport.left(),
            viewport.size.width,
            position.x,
            offset.x,
            maximum.width,
            scale,
        ),
        axis(
            viewport.top(),
            viewport.size.height,
            position.y,
            offset.y,
            maximum.height,
            scale,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::history_drag_offset;
    use gpui::{Bounds, point, px, size};

    #[test]
    fn history_drag_scroll_matches_frozen_chromium() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/history-autoscroll.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let n = |field: &str, index: usize| px(case[field][index].as_f64().unwrap() as f32);
            let actual = history_drag_offset(
                Bounds::new(
                    point(n("viewport", 0), n("viewport", 1)),
                    size(n("viewport", 2), n("viewport", 3)),
                ),
                point(n("pointer", 0), n("pointer", 1)),
                point(n("offset", 0), n("offset", 1)),
                size(n("maximum", 0), n("maximum", 1)),
                case["scale"].as_f64().unwrap() as f32,
            );
            assert_eq!(
                actual,
                point(n("next", 0), n("next", 1)),
                "{}",
                case["name"]
            );
        }
    }
}

fn selected_bytes(selection: &Range<TextPoint>, row: usize, len: usize) -> Option<Range<usize>> {
    if row < selection.start.row || row > selection.end.row {
        return None;
    }
    let start = if row == selection.start.row {
        selection.start.byte
    } else {
        0
    };
    let end = if row == selection.end.row {
        selection.end.byte
    } else {
        len
    };
    Some(start..end)
}

fn boundary(text: &'static str) -> Div {
    div()
        .flex_none()
        .min_w_full()
        .pl(px(56.))
        .pr(px(12.))
        .py(px(4.))
        .text_color(rgb(0x727b8e))
        .italic()
        .whitespace_nowrap()
        .child(ui_text(text))
}

impl Render for HistoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.interaction_subscriptions.is_none() {
            self.interaction_subscriptions = Some([
                cx.observe_window_activation(window, |this, window, _| {
                    if !window.is_window_active() {
                        this.stop_drag();
                    }
                }),
                cx.on_blur(&self.text_focus, window, |this, _, _| this.stop_drag()),
            ]);
        }
        let entity = cx.entity().downgrade();
        let view = div()
            .id("terminal-history-context")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .occlude()
            .text_size(px(11.))
            .line_height(relative(1.55))
            .font_family(self.font_family.clone())
            .text_color(rgb(0x303848))
            .bg(rgb(0xf7f8fb))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    cx.emit(HistoryViewEvent::ReturnToLive);
                }
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        let moving = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = moving.update(cx, |this, cx| {
                                    if event.pressed_button == Some(MouseButton::Left)
                                        && window.is_window_active()
                                        && this.text_focus.is_focused(window)
                                    {
                                        this.extend_selection(event.position, cx);
                                    } else {
                                        this.stop_drag();
                                    }
                                });
                            }
                        });
                        let released = entity.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                let _ = released.update(cx, |this, _| this.stop_drag());
                            }
                        });
                        let _ = entity.update(cx, |this, cx| this.schedule_drag_frame(window, cx));
                    },
                )
                .absolute()
                .size_full(),
            );
        let Some(state) = &self.state else {
            return view;
        };
        let (result_index, result_count, gap_count, page) = match state {
            HistoryViewState::Loading {
                result_index,
                result_count,
                gap_count,
            } => (*result_index, *result_count, *gap_count, None),
            HistoryViewState::Ready {
                result_index,
                result_count,
                page,
            } => (*result_index, *result_count, page.gaps.len(), Some(page)),
        };
        let heading = self.heading(result_index, result_count, cx);
        let body = match page {
            Some(page) => self.body(page, cx).into_any_element(),
            None => div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(rgb(0x8d95a7))
                .child(ui_text("正在读取历史上下文…"))
                .into_any_element(),
        };
        view.child(heading).child(body).child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(12.))
                .px(px(12.))
                .py(px(9.))
                .border_t_1()
                .border_color(rgba(0x3038481f))
                .bg(rgb(0xeef1f6))
                .text_color(rgb(0x8d95a7))
                .whitespace_nowrap()
                .child(ui_text("命中行前后各最多 250 行"))
                .children((gap_count > 0).then(|| ui_text(format!("{gap_count} 处历史缺口"))))
                .child(ui_text("Esc 返回")),
        )
    }
}
