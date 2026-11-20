use std::{cell::Cell, ops::Range, rc::Rc};

use gpui::{prelude::*, *};
use matou_native_runtime::history::{HistoryCursor as HistoryPageCursor, HistoryPage};

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
    /// Journal history read without a PTY. Pagination is driven by the two
    /// explicit actions; there is no live terminal to return to.
    Offline {
        page: Option<HistoryPage>,
        pending: bool,
        show_latest: bool,
        error: Option<String>,
    },
}

pub enum HistoryViewEvent {
    ReturnToLive,
    OlderPage(HistoryPageCursor),
    LatestPage,
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
    scroll_body: bool,
}

type BoundsHandle = Rc<Cell<Option<Bounds<Pixels>>>>;

#[derive(Default)]
struct SelectableText {
    source: String,
    bounds: BoundsHandle,
    geometry: TextGeometryHandle,
}

impl SelectableText {
    fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            ..Self::default()
        }
    }
}

const RETURN_LABEL: usize = 3;

const OFFLINE_OLDER_LABEL: usize = 2;
const OFFLINE_LATEST_LABEL: usize = 3;
const OFFLINE_EMPTY: &str = "暂无历史记录";
const OFFLINE_LOADING: &str = "正在读取历史记录…";
const OFFLINE_OLDER: &str = "加载更早记录";
const OFFLINE_LATEST: &str = "返回最新记录";

#[derive(Clone, Copy)]
enum PageAction {
    Older(HistoryPageCursor),
    Latest,
}

impl PageAction {
    fn event(self) -> HistoryViewEvent {
        match self {
            Self::Older(cursor) => HistoryViewEvent::OlderPage(cursor),
            Self::Latest => HistoryViewEvent::LatestPage,
        }
    }
}

/// A read-only overlay. The live surface remains owned by the workspace.
pub struct HistoryView {
    pub focus: FocusHandle,
    text_focus: FocusHandle,
    state: Option<HistoryViewState>,
    entries: Vec<SelectableText>,
    body_entries: Range<usize>,
    return_bounds: BoundsHandle,
    older_focus: FocusHandle,
    latest_focus: FocusHandle,
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
    pub fn retire(&mut self, cx: &mut Context<Self>) {
        self.stop_drag();
        self.selection = None;
        self.state = None;
        self.entries.clear();
        cx.notify();
    }
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
            entries: Vec::new(),
            body_entries: 0..0,
            return_bounds: BoundsHandle::default(),
            older_focus: cx.focus_handle().tab_stop(true),
            latest_focus: cx.focus_handle().tab_stop(true),
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
        if matches!(&state, HistoryViewState::Offline { .. }) {
            self.set_offline_state(state, cx);
            return;
        }
        self.center_on_anchor =
            matches!(&state, HistoryViewState::Ready { page, .. } if page.anchor_index.is_some());
        self.scroll.set_offset(Point::default());
        let (result_index, result_count, page, gap_count) = match &state {
            HistoryViewState::Loading {
                result_index,
                result_count,
                gap_count,
            } => (*result_index, *result_count, None, *gap_count),
            HistoryViewState::Ready {
                result_index,
                result_count,
                page,
            } => (*result_index, *result_count, Some(page), page.gaps.len()),
            HistoryViewState::Offline { .. } => {
                unreachable!("offline history is handled before the live state")
            }
        };

        // Keep the same text order as the panel's block and flex children.
        // The row numbers are rendered separately and never enter this list.
        let previous_body = self.body_entries.clone();
        let previous_entries = std::mem::replace(
            &mut self.entries,
            vec![
                SelectableText::new("历史记录"),
                SelectableText::new("只读"),
                SelectableText::new(format!("{}/{result_count}", result_index + 1)),
                SelectableText::new("返回实时终端"),
            ],
        );
        let body_start = self.entries.len();
        if let Some(page) = page {
            if page.has_more_before == Some(true) {
                self.entries.push(SelectableText::new("上方还有更早记录"));
            }
            self.entries.extend(page.lines.iter().map(|line| {
                SelectableText::new(if line.text.is_empty() {
                    " "
                } else {
                    &line.text
                })
            }));
            if page.has_more_after == Some(true) {
                self.entries.push(SelectableText::new("下方还有更新记录"));
            }
        } else {
            self.entries
                .push(SelectableText::new("正在读取历史上下文…"));
        }
        self.body_entries = body_start..self.entries.len();
        self.entries
            .push(SelectableText::new("命中行前后各最多 250 行"));
        if gap_count > 0 {
            self.entries
                .push(SelectableText::new(format!("{gap_count} 处历史缺口")));
        }
        self.entries.push(SelectableText::new("Esc 返回"));
        self.return_bounds.set(None);
        self.selection = self.selection_after_state(&previous_entries, previous_body, page);
        self.stop_drag();
        self.state = Some(state);
        cx.notify();
    }

    fn set_offline_state(&mut self, state: HistoryViewState, cx: &mut Context<Self>) {
        let HistoryViewState::Offline {
            page,
            pending,
            show_latest,
            error,
        } = &state
        else {
            return;
        };
        let same_page = self.is_offline() && self.page() == page.as_ref();
        let previous_body = self.body_entries.clone();
        let previous_entries = std::mem::replace(
            &mut self.entries,
            vec![
                SelectableText::new("历史记录"),
                SelectableText::new("只读"),
                SelectableText::new(OFFLINE_OLDER),
            ],
        );
        if *show_latest {
            self.entries.push(SelectableText::new(OFFLINE_LATEST));
        }
        let body_start = self.entries.len();
        if let Some(page) = page.as_ref().filter(|page| !page.lines.is_empty()) {
            self.entries.extend(page.lines.iter().map(|line| {
                SelectableText::new(if line.text.is_empty() {
                    " "
                } else {
                    &line.text
                })
            }));
        } else {
            let status = if *pending {
                OFFLINE_LOADING
            } else if let Some(error) = error {
                error.as_str()
            } else {
                OFFLINE_EMPTY
            };
            self.entries.push(SelectableText::new(status));
        }
        self.body_entries = body_start..self.entries.len();
        if let Some(page) = page {
            if !page.gaps.is_empty() {
                self.entries.push(SelectableText::new(format!(
                    "{} 处历史缺口",
                    page.gaps.len()
                )));
            }
            if !page.lines.is_empty() {
                if let Some(error) = error {
                    self.entries.push(SelectableText::new(error.as_str()));
                }
            }
        }

        if !same_page {
            self.scroll.set_offset(Point::default());
            self.center_on_anchor = page
                .as_ref()
                .is_some_and(|page| page.anchor_index.is_some());
            if !show_latest && page.as_ref().is_some_and(|page| !page.lines.is_empty()) {
                self.scroll.scroll_to_bottom();
            }
        }
        let same_layout = same_page
            && previous_body == self.body_entries
            && previous_entries.len() == self.entries.len()
            && previous_entries
                .iter()
                .zip(&self.entries)
                .all(|(left, right)| left.source == right.source);
        self.return_bounds.set(None);
        if !same_layout {
            self.selection = None;
        }
        self.stop_drag();
        self.state = Some(state);
        cx.notify();
    }

    fn selection_after_state(
        &self,
        previous_entries: &[SelectableText],
        previous_body: Range<usize>,
        page: Option<&HistoryPage>,
    ) -> Option<TextSelection> {
        let selection = self.selection?;
        let ordered = selection.ordered();
        if ordered.is_empty() {
            return None;
        }

        let remap_row = |row| {
            if ordered.start.row >= previous_body.end {
                // The range and Escape labels keep their roles when the optional gap changes.
                let footer = self.body_entries.end;
                if row == previous_body.end {
                    Some(footer)
                } else if row == previous_entries.len() - 1 {
                    Some(self.entries.len() - 1)
                } else if self.entries.len() - footer == 3 {
                    Some(footer + 1)
                } else {
                    None
                }
            } else {
                let previous_page = self.page()?;
                let page = page?;
                let previous_lines = history_line_entries(previous_body.start, previous_page, true);
                if !previous_lines.contains(&row) {
                    return None;
                }
                // The frozen view keys each row by archive cursor, even if its position changes.
                let cursor = previous_page.lines[row - previous_lines.start].cursor;
                let index = page.lines.iter().position(|line| line.cursor == cursor)?;
                Some(history_line_entries(self.body_entries.start, page, true).start + index)
            }
        };
        let remap = |point: TextPoint| {
            let row = remap_row(point.row)?;
            (previous_entries[point.row].source == self.entries[row].source)
                .then_some(TextPoint { row, ..point })
        };
        Some(TextSelection {
            anchor: remap(selection.anchor)?,
            head: remap(selection.head)?,
        })
    }

    fn page(&self) -> Option<&HistoryPage> {
        match &self.state {
            Some(HistoryViewState::Ready { page, .. }) => Some(page),
            Some(HistoryViewState::Offline {
                page: Some(page), ..
            }) => Some(page),
            _ => None,
        }
    }

    fn is_offline(&self) -> bool {
        matches!(&self.state, Some(HistoryViewState::Offline { .. }))
    }

    fn scrolls_body(&self) -> bool {
        match &self.state {
            Some(HistoryViewState::Ready { .. }) => true,
            Some(HistoryViewState::Offline {
                page: Some(page), ..
            }) => !page.lines.is_empty(),
            _ => false,
        }
    }

    fn body_leading_boundary(&self) -> bool {
        match &self.state {
            Some(HistoryViewState::Offline { .. }) => false,
            _ => self
                .page()
                .is_some_and(|page| page.has_more_before == Some(true)),
        }
    }

    fn is_button_entry(&self, index: usize) -> bool {
        match &self.state {
            Some(HistoryViewState::Offline { show_latest, .. }) => {
                index == OFFLINE_OLDER_LABEL || (*show_latest && index == OFFLINE_LATEST_LABEL)
            }
            _ => index == RETURN_LABEL,
        }
    }

    fn text_at_x(&self, entries: Range<usize>, x: Pixels) -> Option<usize> {
        entries
            .filter_map(|index| {
                let layout = self.entries[index].geometry.borrow();
                let bounds = layout.as_ref()?.bounds;
                let distance = (bounds.left() - x).max(x - bounds.right()).max(px(0.));
                Some((index, distance))
            })
            .min_by(|a, b| f32::from(a.1).total_cmp(&f32::from(b.1)))
            .map(|(index, _)| index)
    }

    fn point_at(
        &self,
        position: Point<Pixels>,
        unit: SelectionUnit,
        starting: bool,
    ) -> Option<TextPoint> {
        let scrolling = self.scrolls_body();
        let body = if scrolling {
            self.scroll.bounds()
        } else {
            self.entries.get(self.body_entries.start)?.bounds.get()?
        };
        let row = if position.y < body.top() {
            self.text_at_x(0..self.body_entries.start, position.x)?
        } else if position.y >= body.bottom() {
            self.text_at_x(self.body_entries.end..self.entries.len(), position.x)
                .or_else(|| (self.is_offline() && !starting).then(|| self.body_entries.end - 1))?
        } else {
            let entries = &self.entries[self.body_entries.clone()];
            let last = entries.len().checked_sub(1)?;
            let row = entries.partition_point(|entry| {
                entry
                    .bounds
                    .get()
                    .is_some_and(|bounds| position.y >= bounds.bottom())
            });
            self.body_entries.start + row.min(last)
        };
        let entry = &self.entries[row];
        let geometry = entry.geometry.borrow();
        let geometry = geometry.as_ref()?;
        if starting && scrolling && self.body_entries.contains(&row) {
            let row_bounds = entry.bounds.get()?;
            if position.x < geometry.bounds.left()
                || position.y < row_bounds.top()
                || position.y >= row_bounds.bottom()
            {
                return None;
            }
        }

        let source = &entry.source;
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
        if matches!(unit, SelectionUnit::Line)
            && self.page().is_some_and(|page| {
                history_line_entries(self.body_entries.start, page, self.body_leading_boundary())
                    .next_back()
                    == Some(point.row)
            })
        {
            // End at the start of the next block to include the final line break.
            return TextPoint { byte: 0, ..point }..TextPoint {
                row: point.row + 1,
                byte: 0,
            };
        }
        let text = &self.entries[point.row].source;
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
        if self
            .return_bounds
            .get()
            .is_some_and(|bounds| bounds.contains(&event.position))
        {
            return;
        }
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
            scroll_body: self.scrolls_body() && self.body_entries.contains(&point.row),
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub fn stop_drag(&mut self) {
        self.drag = None;
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.stop_drag();
        if self.selection.take().is_some() {
            cx.notify();
        }
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
        if self.drag_frame_pending || self.drag_scroll_position().is_none() {
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

    fn drag_scroll_position(&self) -> Option<Point<Pixels>> {
        self.drag.as_ref().filter(|drag| drag.scroll_body)?.position
    }

    fn advance_drag_frame(&mut self, scale: f32, cx: &mut Context<Self>) {
        let Some(position) = self.drag_scroll_position() else {
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
        let selection = self.selection?.ordered();
        if selection.is_empty() {
            return None;
        }

        let mut text = String::new();
        for row in selection.start.row..=selection.end.row {
            if row != selection.start.row {
                text.push('\n');
            }
            if let Some(entry) = self.entries.get(row)
                && let Some(range) = selected_bytes(&selection, row, entry.source.len())
            {
                text.push_str(&entry.source[range]);
            }
        }
        Some(text)
    }

    fn selectable_text(
        &self,
        index: usize,
        selection_background: Rgba,
    ) -> impl Iterator<Item = AnyElement> + use<> {
        let entry = &self.entries[index];
        let geometry = entry.geometry.clone();
        let continues = self.selection.is_some_and(|selection| {
            index < selection.ordered().end.row && !self.is_button_entry(index)
        });
        let highlight = self
            .selection
            .and_then(|selection| selected_bytes(&selection.ordered(), index, entry.source.len()))
            .filter(|range| !range.is_empty() || continues)
            .map(|range| {
                let geometry = geometry.clone();
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let layout = geometry.borrow();
                        let Some(layout) = layout.as_ref() else {
                            return;
                        };
                        let a = layout.layout.x_for_index(range.start);
                        let mut b = layout.layout.x_for_index(range.end);
                        // Blink extends a selected block-ending line by its
                        // primary font's space advance. The button is an
                        // inline-block, whose final line has no extension.
                        if continues {
                            let style = window.text_style();
                            let font = window.text_system().resolve_font(&style.font());
                            let size = style.font_size.to_pixels(window.rem_size());
                            match window.text_system().advance(font, size, ' ') {
                                Ok(space) => b += space.width,
                                Err(error) => {
                                    eprintln!("Measure history selection break: {error:#}")
                                }
                            }
                        }
                        let bounds = Bounds::new(
                            point(layout.bounds.left() + a.min(b), layout.bounds.top()),
                            size((b - a).abs(), layout.bounds.size.height),
                        );
                        window.paint_quad(fill(bounds, selection_background));
                    },
                )
                .absolute()
                .inset_0()
                .size_full()
                .into_any_element()
            });
        let text = if entry.source.is_ascii() {
            into_text_element(StyledText::new(entry.source.clone()), Some(geometry))
        } else {
            ui_text(entry.source.clone())
                .track_geometry(geometry)
                .into_any_element()
        };
        highlight.into_iter().chain(std::iter::once(text))
    }

    fn heading(&self, selection_background: Rgba, cx: &mut Context<Self>) -> AnyElement {
        if let Some(HistoryViewState::Offline {
            page,
            pending,
            show_latest,
            ..
        }) = &self.state
        {
            return self
                .offline_heading(
                    page.as_ref(),
                    *pending,
                    *show_latest,
                    selection_background,
                    cx,
                )
                .into_any_element();
        }
        let focus = self.focus.clone();
        let return_bounds = self.return_bounds.clone();
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
            .children(self.selectable_text(RETURN_LABEL, selection_background))
            .child(
                canvas(
                    move |bounds, _, _| return_bounds.set(Some(bounds)),
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
                            .relative()
                            .text_color(rgb(0x202735))
                            .font_weight(FontWeight::BOLD)
                            .children(self.selectable_text(0, selection_background)),
                    )
                    .child(
                        div()
                            .relative()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .text_color(rgb(0xc98a39))
                            .bg(rgba(0xc98a3921))
                            .children(self.selectable_text(1, selection_background)),
                    )
                    .child(
                        div()
                            .relative()
                            .children(self.selectable_text(2, selection_background)),
                    ),
            )
            .child(return_button)
            .into_any_element()
    }

    fn offline_heading(
        &self,
        page: Option<&HistoryPage>,
        pending: bool,
        show_latest: bool,
        selection_background: Rgba,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let older_cursor = page.and_then(offline_older_cursor);
        let older = self.pagination_button(
            "history-offline-older",
            "history-offline-older-page",
            OFFLINE_OLDER_LABEL,
            older_cursor.map(PageAction::Older),
            older_cursor.is_some() && !pending,
            &self.older_focus,
            selection_background,
            cx,
        );
        let latest = show_latest.then(|| {
            self.pagination_button(
                "history-offline-latest",
                "history-offline-latest-page",
                OFFLINE_LATEST_LABEL,
                Some(PageAction::Latest),
                !pending,
                &self.latest_focus,
                selection_background,
                cx,
            )
        });
        let actions = div()
            .id("history-offline-actions")
            .debug_selector(|| "history-offline-actions".to_owned())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .child(older)
            .children(latest);

        div()
            .id("history-offline-heading")
            .debug_selector(|| "history-offline-heading".to_owned())
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
                            .id("history-offline-title")
                            .debug_selector(|| "history-offline-title".to_owned())
                            .relative()
                            .text_color(rgb(0x202735))
                            .font_weight(FontWeight::BOLD)
                            .children(self.selectable_text(0, selection_background)),
                    )
                    .child(
                        div()
                            .id("history-offline-read-only")
                            .debug_selector(|| "history-offline-read-only".to_owned())
                            .relative()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .text_color(rgb(0xc98a39))
                            .bg(rgba(0xc98a3921))
                            .children(self.selectable_text(1, selection_background)),
                    ),
            )
            .child(actions)
    }

    #[allow(clippy::too_many_arguments)]
    fn pagination_button(
        &self,
        id: &'static str,
        selector: &'static str,
        label_index: usize,
        action: Option<PageAction>,
        enabled: bool,
        focus: &FocusHandle,
        selection_background: Rgba,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let button = div()
            .id(id)
            .debug_selector(move || selector.to_owned())
            .relative()
            .flex_none()
            .px(px(9.))
            .py(px(4.))
            .border_1()
            .border_color(rgba(0x30384829))
            .rounded(px(6.))
            .bg(rgba(0x3038480d))
            .whitespace_nowrap()
            .children(self.selectable_text(label_index, selection_background));
        if !enabled {
            return button
                .opacity(0.55)
                .cursor(CursorStyle::OperationNotAllowed)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                });
        }
        let action = action.expect("enabled history pagination has a cursor");
        let focus = focus.clone();
        let mouse_focus = focus.clone();
        let key_focus = focus.clone();
        button
            .hover(|style| style.bg(rgba(0x30384817)))
            .cursor_pointer()
            .track_focus(&focus)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&mouse_focus);
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space")
                    && !event.keystroke.modifiers.modified()
                {
                    cx.stop_propagation();
                    cx.emit(action.event());
                }
            }))
            .on_click(cx.listener(move |_, event, _, cx| {
                if matches!(event, ClickEvent::Keyboard(_)) {
                    return;
                }
                cx.stop_propagation();
                cx.emit(action.event());
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        if key_focus.is_focused(window) {
                            window.paint_quad(
                                outline(
                                    bounds.dilate(px(3.)),
                                    rgb(0x548cff),
                                    BorderStyle::default(),
                                )
                                .corner_radii(px(9.))
                                .border_widths(px(2.)),
                            );
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
    }

    fn body(
        &self,
        page: &HistoryPage,
        selection_background: Rgba,
        boundaries: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let view = cx.entity().downgrade();
        let before = boundaries && page.has_more_before == Some(true);
        let line_start = history_line_entries(self.body_entries.start, page, boundaries).start;
        let rows = page.lines.iter().enumerate().map(|(index, _)| {
            let current = page.anchor_index == Some(index);
            let entry = line_start + index;
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
                .child(track_bounds(self.entries[entry].bounds.clone()))
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .min_w_0()
                        .cursor(CursorStyle::IBeam)
                        .children(self.selectable_text(entry, selection_background)),
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
            .py(px(8.))
            .children(before.then(|| self.boundary(self.body_entries.start, selection_background)))
            .children(rows)
            .children(
                (boundaries && page.has_more_after == Some(true))
                    .then(|| self.boundary(self.body_entries.end - 1, selection_background)),
            )
    }

    fn boundary(&self, index: usize, selection_background: Rgba) -> Div {
        div()
            .relative()
            .flex_none()
            .min_w_full()
            .pl(px(56.))
            .pr(px(12.))
            .py(px(4.))
            .text_color(rgb(0x727b8e))
            .italic()
            .whitespace_nowrap()
            .child(track_bounds(self.entries[index].bounds.clone()))
            .children(self.selectable_text(index, selection_background))
    }

    fn offline_status(
        &self,
        error: bool,
        selector: &'static str,
        selection_background: Rgba,
    ) -> AnyElement {
        let entry = self.body_entries.start;
        div()
            .id(selector)
            .debug_selector(move || selector.to_owned())
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .px(px(24.))
            .text_color(if error { rgb(0xa12525) } else { rgb(0x8d95a7) })
            .whitespace_normal()
            .child(track_bounds(self.entries[entry].bounds.clone()))
            .child(
                div()
                    .relative()
                    .children(self.selectable_text(entry, selection_background)),
            )
            .into_any_element()
    }

    fn offline_footer(
        &self,
        page: Option<&HistoryPage>,
        error: Option<&str>,
        selection_background: Rgba,
    ) -> Option<Stateful<Div>> {
        let page = page?;
        let show_gaps = !page.gaps.is_empty();
        let show_error = !page.lines.is_empty() && error.is_some();
        if !show_gaps && !show_error {
            return None;
        }
        let gap_index = self.body_entries.end;
        let error_index = gap_index + usize::from(show_gaps);
        let mut children = Vec::new();
        if show_gaps {
            children.push(
                div()
                    .relative()
                    .debug_selector(|| "history-offline-gaps".to_owned())
                    .children(self.selectable_text(gap_index, selection_background))
                    .into_any_element(),
            );
        }
        if show_error {
            children.push(
                div()
                    .relative()
                    .debug_selector(|| "history-offline-footer-error".to_owned())
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .text_color(rgb(0xa12525))
                    .children(self.selectable_text(error_index, selection_background))
                    .into_any_element(),
            );
        }
        Some(
            div()
                .id("history-offline-footer")
                .debug_selector(|| "history-offline-footer".to_owned())
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
                .children(children),
        )
    }
}

fn history_line_entries(
    body_start: usize,
    page: &HistoryPage,
    leading_boundary: bool,
) -> Range<usize> {
    let start = body_start + usize::from(leading_boundary && page.has_more_before == Some(true));
    start..start + page.lines.len()
}

/// The cursor to page before. A search hit context carries newer lines after
/// the anchor too, so only `has_more_before` may enable its older-page action.
fn offline_older_cursor(page: &HistoryPage) -> Option<HistoryPageCursor> {
    let has_earlier = if page.anchor_index.is_some() {
        page.has_more_before == Some(true)
    } else {
        page.has_more || page.has_more_before == Some(true)
    };
    if !has_earlier {
        return None;
    }
    page.lines.first().map(|line| line.cursor)
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

    #[allow(unexpected_cfgs)]
    #[cfg(matou_history_selection_gate)]
    mod selection {
        use super::super::{
            HistoryPage, HistoryView, HistoryViewState, SelectionUnit, TextPoint, TextSelection,
        };
        use gpui::{AppContext, Context, TestAppContext};
        use matou_native_runtime::history::{
            HistoryCursor, HistoryGap, HistoryGapCode, HistoryLine,
        };

        fn ready(first: u64, last: u64, before: bool, after: bool) -> HistoryViewState {
            HistoryViewState::Ready {
                result_index: 0,
                result_count: 1,
                page: HistoryPage {
                    lines: (first..=last)
                        .map(|sequence| HistoryLine {
                            sequence,
                            cursor: HistoryCursor {
                                sequence,
                                line_index: 0,
                            },
                            text: if sequence == 5 {
                                "A中文🙂 e\u{301} Z".into()
                            } else {
                                format!("record {sequence} tail")
                            },
                        })
                        .collect(),
                    gaps: vec![HistoryGap {
                        segment_index: 0,
                        code: HistoryGapCode::MissingSegment,
                        message: String::new(),
                    }],
                    has_more_before: Some(before),
                    has_more_after: Some(after),
                    ..HistoryPage::default()
                },
            }
        }

        fn with_view(f: impl FnOnce(&mut HistoryView, &mut Context<HistoryView>)) {
            TestAppContext::single().update(|cx| cx.new(HistoryView::new).update(cx, f));
        }

        #[test]
        fn last_line_triple_click_includes_only_the_trailing_newline() {
            with_view(|view, cx| {
                for has_more_after in [false, true] {
                    view.set_state(ready(4, 5, false, has_more_after), cx);
                    let first = view.body_entries.start;
                    for (row, expected) in [
                        (first, "record 4 tail"),
                        (first + 1, "A中文🙂 e\u{301} Z\n"),
                    ] {
                        let range =
                            view.unit_range(TextPoint { row, byte: 1 }, SelectionUnit::Line);
                        view.selection = Some(TextSelection {
                            anchor: range.start,
                            head: range.end,
                        });
                        assert_eq!(
                            view.selected_text().unwrap().as_bytes(),
                            expected.as_bytes()
                        );
                        if row == first + 1 {
                            assert_eq!(range.end.row, row + 1);
                            assert_eq!(range.end.byte, 0);
                            assert_eq!(range.end.row == view.body_entries.end, !has_more_after);
                        }
                    }
                }
            });
        }

        #[test]
        fn offline_last_line_can_be_selected_and_copied_without_a_live_terminal() {
            use gpui::{KeyDownEvent, Keystroke};
            let mut cx = TestAppContext::single();
            let window = cx.add_window(|_, cx| HistoryView::new(cx));
            let HistoryViewState::Ready { mut page, .. } = ready(4, 5, false, false) else {
                unreachable!()
            };
            page.gaps.clear();
            page.has_more = true;
            let cursor = page.lines[0].cursor;
            window
                .update(&mut cx, |view, window, cx| {
                    view.set_state(
                        HistoryViewState::Offline {
                            page: Some(page),
                            pending: false,
                            show_latest: false,
                            error: None,
                        },
                        cx,
                    );
                    assert_eq!(
                        super::super::offline_older_cursor(view.page().unwrap()),
                        Some(cursor)
                    );
                    assert!(
                        !view
                            .entries
                            .iter()
                            .any(|entry| entry.source == "返回实时终端")
                    );
                    let range = view.unit_range(
                        TextPoint {
                            row: view.body_entries.end - 1,
                            byte: 1,
                        },
                        SelectionUnit::Line,
                    );
                    view.selection = Some(TextSelection {
                        anchor: range.start,
                        head: range.end,
                    });
                    assert_eq!(view.selected_text().as_deref(), Some("A中文🙂 é Z\n"));
                    view.copy(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse(if cfg!(target_os = "macos") {
                                "cmd-c"
                            } else {
                                "ctrl-c"
                            })
                            .unwrap(),
                            is_held: false,
                        },
                        window,
                        cx,
                    );
                })
                .unwrap();
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("A中文🙂 é Z\n")
            );
        }

        #[test]
        fn footer_selection_survives_loading_and_ready() {
            with_view(|view, cx| {
                view.set_state(ready(1, 3, false, false), cx);
                let select_footer = |view: &HistoryView| TextSelection {
                    anchor: TextPoint {
                        row: view.entries.len() - 1,
                        byte: 3,
                    },
                    head: TextPoint {
                        row: view.body_entries.end,
                        byte: "命".len(),
                    },
                };
                view.selection = Some(select_footer(view));
                let expected = "中行前后各最多 250 行\n1 处历史缺口\nEsc";
                for state in [
                    HistoryViewState::Loading {
                        result_index: 0,
                        result_count: 1,
                        gap_count: 1,
                    },
                    ready(1, 3, false, false),
                ] {
                    view.set_state(state, cx);
                    assert!(view.selection == Some(select_footer(view)));
                    assert_eq!(view.selected_text().as_deref(), Some(expected));
                }
            });
        }

        #[test]
        fn refreshed_history_selection_follows_archive_cursors() {
            with_view(|view, cx| {
                view.set_state(ready(2, 6, false, false), cx);
                let selection = |first: usize| TextSelection {
                    anchor: TextPoint {
                        row: first + 3,
                        byte: "A中文🙂".len(),
                    },
                    head: TextPoint {
                        row: first + 2,
                        byte: 2,
                    },
                };
                let expected = "cord 4 tail\nA中文🙂";
                let first = view.body_entries.start;
                view.selection = Some(selection(first));

                view.set_state(ready(2, 16, false, false), cx);
                assert!(view.selection == Some(selection(first)));
                assert_eq!(view.selected_text().as_deref(), Some(expected));

                // One new row and the leading boundary shift both endpoints two entries.
                view.set_state(ready(1, 16, true, false), cx);
                assert!(view.selection == Some(selection(first + 2)));
                assert_eq!(view.selected_text().as_deref(), Some(expected));

                view.set_state(ready(5, 16, true, false), cx);
                assert!(view.selection.is_none());
            });
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

fn track_bounds(bounds: BoundsHandle) -> impl IntoElement {
    canvas(move |rect, _, _| bounds.set(Some(rect)), |_, _, _, _| {})
        .absolute()
        .inset_0()
        .size_full()
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
        let offline = self.is_offline();
        let view = div().id("terminal-history-context");
        let view = if offline {
            view.debug_selector(|| "history-offline".to_owned())
        } else {
            view
        };
        let view = view
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
            .track_focus(&self.text_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_selection))
            .on_key_down(cx.listener(Self::copy))
            .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    if !offline {
                        cx.emit(HistoryViewEvent::ReturnToLive);
                    }
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
        if self.state.is_none() {
            return view;
        }
        let selection_background = rgb(0xb5d5ff);
        #[cfg(target_os = "macos")]
        let selection_background = if self.selection.is_some() {
            crate::macos::history_selection_background(window.is_window_active()).unwrap_or_else(
                |error| {
                    eprintln!("Read history selection background: {error:#}");
                    selection_background
                },
            )
        } else {
            selection_background
        };
        let heading = self.heading(selection_background, cx);
        let body = if offline {
            let (page, pending, error) = match &self.state {
                Some(HistoryViewState::Offline {
                    page,
                    pending,
                    error,
                    ..
                }) => (page.as_ref(), *pending, error.as_deref()),
                _ => unreachable!("offline history state changed during render"),
            };
            if let Some(page) = page.filter(|page| !page.lines.is_empty()) {
                self.body(page, selection_background, false, cx)
                    .into_any_element()
            } else {
                let (is_error, selector) = if pending {
                    (false, "history-offline-loading")
                } else if error.is_some() {
                    (true, "history-offline-error")
                } else {
                    (false, "history-offline-empty")
                };
                self.offline_status(is_error, selector, selection_background)
            }
        } else {
            match self.page() {
                Some(page) => self
                    .body(page, selection_background, true, cx)
                    .into_any_element(),
                None => div()
                    .relative()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0x8d95a7))
                    .child(track_bounds(
                        self.entries[self.body_entries.start].bounds.clone(),
                    ))
                    .child(div().relative().children(
                        self.selectable_text(self.body_entries.start, selection_background),
                    ))
                    .into_any_element(),
            }
        };
        let mut view = view.child(heading).child(body);
        if offline {
            let (page, error) = match &self.state {
                Some(HistoryViewState::Offline { page, error, .. }) => {
                    (page.as_ref(), error.as_deref())
                }
                _ => unreachable!("offline history state changed during render"),
            };
            if let Some(footer) = self.offline_footer(page, error, selection_background) {
                view = view.child(footer);
            }
        } else {
            view = view.child(
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
                    .children((self.body_entries.end..self.entries.len()).map(|index| {
                        div()
                            .relative()
                            .children(self.selectable_text(index, selection_background))
                    })),
            );
        }
        view
    }
}

// The contract runner enables GPUI's test platform without changing desktop dependencies.
#[allow(unexpected_cfgs)]
#[cfg(all(test, matou_history_selection_gate))]
#[path = "../../../../artifacts/native-parity/native/evidence/porting-map-01/gates/K-history-selection/capture_native.rs"]
mod history_selection_gate;
