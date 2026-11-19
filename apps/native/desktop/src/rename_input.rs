//! Single-line native editor for the frozen RenameDialog input.
//! GPUI's EntityInputHandler supplies platform text composition and selection.
use gpui::{prelude::*, *};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub fn trim_name(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() && c != '\u{0085}' || c == '\u{feff}')
}

pub enum RenameInputEvent {
    Changed,
    Confirm,
    Cancel,
}

struct EditState {
    text: String,
    selection: Range<usize>,
    reversed: bool,
}

pub struct RenameInput {
    pub focus: FocusHandle,
    pub text: String,
    pub placeholder: String,
    pub search_style: bool,
    session_loader_search_style: bool,
    selection: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    before_composition: Option<EditState>,
    undo: Vec<EditState>,
    redo: Vec<EditState>,
    typing: bool,
    layout: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    selecting: bool,
}
impl EventEmitter<RenameInputEvent> for RenameInput {}
impl RenameInput {
    pub fn new(text: String, cx: &mut Context<Self>) -> Self {
        let end = text.len();
        Self {
            focus: cx.focus_handle().tab_stop(true),
            text,
            placeholder: "输入标签页名称".into(),
            search_style: false,
            session_loader_search_style: false,
            selection: end..end,
            reversed: false,
            marked: None,
            before_composition: None,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            layout: None,
            bounds: None,
            scroll: px(0.),
            selecting: false,
        }
    }

    pub fn session_loader_search(placeholder: &str, cx: &mut Context<Self>) -> Self {
        let mut input = Self::new(String::new(), cx);
        input.placeholder = placeholder.into();
        input.search_style = true;
        input.session_loader_search_style = true;
        input
    }
    pub fn composing(&self) -> bool {
        self.marked.is_some()
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
    fn bytes(&self, r: Range<usize>) -> Range<usize> {
        Self::to_byte(&self.text, r.start)..Self::to_byte(&self.text, r.end)
    }

    fn edit_state(&self) -> EditState {
        EditState {
            text: self.text.clone(),
            selection: self.selection.clone(),
            reversed: self.reversed,
        }
    }

    fn restore_edit_state(&mut self, state: EditState, cx: &mut Context<Self>) {
        self.text = state.text;
        self.selection = state.selection;
        self.reversed = state.reversed;
        self.marked = None;
        cx.emit(RenameInputEvent::Changed);
        cx.notify();
    }

    fn restore_history(&mut self, redo: bool, cx: &mut Context<Self>) {
        self.typing = false;
        let state = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(state) = state {
            let current = self.edit_state();
            if redo {
                self.undo.push(current);
            } else {
                self.redo.push(current);
            }
            self.restore_edit_state(state, cx);
        }
    }

    fn replace(&mut self, range: Range<usize>, text: &str, typing: bool, cx: &mut Context<Self>) {
        let remaining = (if self.search_style {
            usize::MAX
        } else {
            20usize
        })
        .saturating_sub(
            self.text[..range.start].encode_utf16().count()
                + self.text[range.end..].encode_utf16().count(),
        );
        let mut units = 0;
        let insert: String = text
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .take_while(|c| {
                units += c.len_utf16();
                units <= remaining
            })
            .collect();
        let composition = self.before_composition.take();
        let typing = typing
            && composition.is_none()
            && !text.is_empty()
            && text.graphemes(true).nth(1).is_none();
        let previous = composition.unwrap_or_else(|| self.edit_state());
        self.text.replace_range(range.clone(), &insert);
        let end = range.start + insert.len();
        self.selection = end..end;
        self.reversed = false;
        self.marked = None;
        if self.text != previous.text {
            if !(typing && self.typing) {
                self.undo.push(previous);
            }
            self.redo.clear();
            self.typing = typing;
        } else if !typing {
            self.typing = false;
        }
        cx.emit(RenameInputEvent::Changed);
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, extend: bool, cx: &mut Context<Self>) {
        self.typing = false;
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
            .find_map(|(i, _)| (i < offset).then_some(i))
            .unwrap_or(0)
    }
    fn next(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(i, _)| (i > offset).then_some(i))
            .unwrap_or(self.text.len())
    }

    fn previous_word(&self, offset: usize) -> usize {
        self.text
            .unicode_word_indices()
            .take_while(|(i, _)| *i < offset)
            .last()
            .map_or(0, |(i, _)| i)
    }

    fn next_word(&self, offset: usize, from_start: bool) -> usize {
        self.text
            .unicode_word_indices()
            .map(|(i, word)| if from_start { i } else { i + word.len() })
            .find(|i| *i > offset)
            .unwrap_or(self.text.len())
    }
    fn hit(&self, position: Point<Pixels>) -> usize {
        self.bounds
            .zip(self.layout.as_ref())
            .map(|(b, l)| {
                l.closest_index_for_x(position.x - b.left() + self.scroll)
                    .min(self.text.len())
            })
            .unwrap_or(0)
    }
    fn key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &e.keystroke.key;
        let m = &e.keystroke.modifiers;
        let shortcut = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.control
        };
        let word = if cfg!(target_os = "macos") {
            m.alt
        } else {
            m.control
        };
        let line = cfg!(target_os = "macos") && m.platform;
        if self.marked.is_some() && !matches!(key.as_str(), "escape") {
            return;
        }
        match key.as_str() {
            "escape" => cx.emit(RenameInputEvent::Cancel),
            "enter" => cx.emit(RenameInputEvent::Confirm),
            "z" if shortcut => self.restore_history(m.shift, cx),
            "y" if !cfg!(target_os = "macos") && shortcut => self.restore_history(true, cx),
            "a" if shortcut => {
                self.typing = false;
                self.selection = 0..self.text.len();
                self.reversed = false;
            }
            "c" | "x" if shortcut => {
                if !self.selection.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.text[self.selection.clone()].to_owned(),
                    ));
                }
                if key == "x" {
                    self.replace(self.selection.clone(), "", false, cx);
                }
            }
            "v" if shortcut => {
                self.typing = false;
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.replace(self.selection.clone(), &text, false, cx);
                }
            }
            "left" | "right" | "home" | "end" => {
                let end = key == "right" || key == "end";
                let cursor = self.cursor();
                let offset = if key == "home" || key == "end" || line {
                    if end { self.text.len() } else { 0 }
                } else if !m.shift && !self.selection.is_empty() {
                    if end {
                        self.selection.end
                    } else {
                        self.selection.start
                    }
                } else if word && end {
                    self.next_word(cursor, cfg!(target_os = "windows"))
                } else if word {
                    self.previous_word(cursor)
                } else if end {
                    self.next(cursor)
                } else {
                    self.previous(cursor)
                };
                self.move_to(offset, m.shift, cx);
            }
            "backspace" | "delete" => {
                let mut range = self.selection.clone();
                if range.is_empty() {
                    let cursor = self.cursor();
                    if key == "backspace" {
                        range.start = if line {
                            0
                        } else if word {
                            self.previous_word(cursor)
                        } else {
                            self.previous(cursor)
                        };
                    } else {
                        range.end = if line {
                            self.text.len()
                        } else if word {
                            self.next_word(cursor, false)
                        } else {
                            self.next(cursor)
                        };
                    }
                    self.selection = range.clone();
                }
                self.replace(range, "", false, cx);
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
        let display: SharedString = if self.text.is_empty() {
            self.placeholder.clone().into()
        } else {
            self.text.clone().into()
        };
        let mut run = TextRun {
            len: display.len(),
            font: window.text_style().font(),
            color: rgba(if self.session_loader_search_style {
                if self.text.is_empty() {
                    0x858e9bff
                } else {
                    0xe7eaf0ff
                }
            } else if self.text.is_empty() {
                if self.search_style {
                    0x747474ff
                } else {
                    0x00000066
                }
            } else {
                if self.search_style {
                    0x2f3547ff
                } else {
                    0x000000d9
                }
            })
            .into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked) = &self.marked {
            let before = TextRun {
                len: marked.start,
                ..run.clone()
            };
            let after = TextRun {
                len: display.len() - marked.end,
                ..run.clone()
            };
            run.len = marked.len();
            run.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            });
            vec![before, run, after]
                .into_iter()
                .filter(|r| r.len > 0)
                .collect::<Vec<_>>()
        } else {
            vec![run]
        };
        let line = window.text_system().shape_line(
            display,
            px(if self.session_loader_search_style {
                11.
            } else if self.search_style {
                12.
            } else {
                14.
            }),
            &runs,
            None,
        );
        let cursor = line.x_for_index(self.cursor());
        self.scroll = self
            .scroll
            .min((line.x_for_index(self.text.len()) - bounds.size.width + px(1.)).max(px(0.)));
        if cursor - self.scroll > bounds.size.width - px(1.) {
            self.scroll = (cursor - bounds.size.width + px(1.)).max(px(0.));
        }
        if cursor < self.scroll {
            self.scroll = cursor;
        }
        if self.text.is_empty() {
            self.scroll = px(0.);
        }
        let origin = point(bounds.left() - self.scroll, bounds.top());
        window.handle_input(&self.focus, ElementInputHandler::new(bounds, entity), cx);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if !self.selection.is_empty() {
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(
                            origin.x + line.x_for_index(self.selection.start),
                            bounds.top(),
                        ),
                        point(
                            origin.x + line.x_for_index(self.selection.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x347fd64d),
                ));
            }
            let _ = line.paint(origin, bounds.size.height, window, cx);
            if self.selection.is_empty() && self.focus.is_focused(window) {
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x + cursor, bounds.top() + px(2.)),
                        size(px(1.), bounds.size.height - px(4.)),
                    ),
                    rgba(if self.session_loader_search_style {
                        0xe7eaf0ff
                    } else {
                        0x000000d9
                    }),
                ));
            }
        });
        self.bounds = Some(bounds);
        self.layout = Some(line);
    }
}
impl EntityInputHandler for RenameInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.bytes(range);
        *actual = Some(self.to_utf16(range.start)..self.to_utf16(range.end));
        Some(self.text[range].to_owned())
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
            .map(|r| self.to_utf16(r.start)..self.to_utf16(r.end))
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
        let typing = range.is_none();
        let range = range
            .map(|r| self.bytes(r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        self.replace(range, text, typing, cx);
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
            if let Some(previous) = self.before_composition.take() {
                self.restore_edit_state(previous, cx);
            }
            return;
        }
        if self.before_composition.is_none() {
            self.before_composition = Some(self.edit_state());
            self.typing = false;
        }
        let range = range
            .map(|r| self.bytes(r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        self.text.replace_range(range.clone(), text);
        self.marked = Some(range.start..range.start + text.len());
        self.selection = selected
            .map(|r| {
                range.start + Self::to_byte(text, r.start)..range.start + Self::to_byte(text, r.end)
            })
            .unwrap_or(range.start + text.len()..range.start + text.len());
        self.reversed = false;
        cx.emit(RenameInputEvent::Changed);
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
        let b = self.bounds?;
        let l = self.layout.as_ref()?;
        Some(Bounds::from_corners(
            point(b.left() + l.x_for_index(range.start) - self.scroll, b.top()),
            point(
                b.left() + l.x_for_index(range.end) - self.scroll,
                b.bottom(),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.to_utf16(self.hit(p)))
    }
}
impl Render for RenameInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id("rename-native-input")
            .track_focus(&self.focus)
            .h(px(if self.search_style { 28. } else { 32. }))
            .w_full()
            .relative()
            .rounded(px(8.))
            .bg(rgba(if self.search_style {
                0x00000000
            } else {
                0x0000000a
            }))
            .pl(px(if self.search_style { 0. } else { 14. }))
            .pr(px(if self.search_style { 0. } else { 50. }))
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.selecting = true;
                    this.typing = false;
                    let offset = this.hit(e.position);
                    if e.click_count >= 3 {
                        this.selection = 0..this.text.len();
                        this.reversed = false;
                    } else if e.click_count == 2 {
                        let word = this
                            .text
                            .unicode_word_indices()
                            .find(|(i, w)| *i <= offset && offset <= *i + w.len())
                            .map(|(i, w)| i..i + w.len());
                        this.selection = word.unwrap_or(offset..offset);
                        this.reversed = false;
                    } else {
                        this.move_to(offset, e.modifiers.shift, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.selecting {
                    this.move_to(this.hit(e.position), true, cx);
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
                .mt(px(if self.search_style { 4. } else { 6. })),
            )
            .when(!self.search_style, |el| {
                el.child(
                    div()
                        .absolute()
                        .right(px(14.))
                        .top(px(6.))
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(rgba(0xffffff57))
                        .child(format!("{}/20", self.text.encode_utf16().count())),
                )
            })
    }
}
