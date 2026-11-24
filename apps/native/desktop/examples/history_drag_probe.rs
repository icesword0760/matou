//! Exercise the production drag handlers against a laid-out history view.
#[cfg(target_os = "macos")]
pub use matou_native_desktop::macos;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod ui_text {
    include!("../src/ui_text.rs");
}

#[cfg(target_os = "macos")]
mod probe {
    include!("../src/history_view.rs");

    use matou_native_desktop::assets::NativeAssets;
    use matou_native_runtime::history::{HistoryCursor, HistoryGap, HistoryGapCode, HistoryLine};

    fn history(prefix: &str) -> HistoryViewState {
        HistoryViewState::Ready {
            result_index: 0,
            result_count: 1,
            page: HistoryPage {
                lines: (0..80)
                    .map(|sequence| HistoryLine {
                        sequence,
                        cursor: HistoryCursor {
                            sequence,
                            line_index: 0,
                        },
                        text: format!("{prefix}-{sequence:02}  🙂"),
                    })
                    .collect(),
                ..HistoryPage::default()
            },
        }
    }

    fn draw(window: WindowHandle<HistoryView>, cx: &mut App) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .expect("draw hidden history view");
    }

    impl HistoryView {
        fn entry_point(&self, index: usize, end: bool) -> Point<Pixels> {
            let geometry = self.entries[index].geometry.borrow();
            let bounds = geometry.as_ref().expect("laid-out entry").bounds;
            let x = if end {
                bounds.right() + px(0.25)
            } else {
                bounds.left() + px(0.25)
            };
            point(x, bounds.center().y)
        }

        fn press(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
            self.begin_selection(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                },
                window,
                cx,
            );
        }

        fn begin_probe_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            let first = self.body_entries.start;
            self.press(self.entry_point(first, false), window, cx);
            let start = self.selection.expect("drag started").anchor;
            assert_eq!((start.row, start.byte), (first, 0));
            let viewport = self.scroll.bounds();
            assert!(self.scroll.max_offset().height > px(100.));
            let pointer = point(viewport.center().x, viewport.bottom() - px(1.));
            self.extend_selection(pointer, cx);
        }

        fn verify_copy_range(&self) -> (usize, usize) {
            let range = self.selection.expect("non-empty selection").ordered();
            let page = self.page().expect("visible history page");
            let start = range.start.row - self.body_entries.start;
            let end = range.end.row - self.body_entries.start;
            assert_eq!(range.start.byte, 0);
            assert_eq!(range.end.byte, page.lines[end].text.len());
            let expected = page.lines[start..=end]
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                self.selected_text().as_deref(),
                Some(expected.as_str()),
                "copy must match the new Unicode rows byte for byte, with no line numbers"
            );
            (start, end)
        }
    }

    fn verify(window: WindowHandle<HistoryView>, cx: &mut App) {
        draw(window, cx);
        let (before, first_row_top) = window
            .update(cx, |view, window, cx| {
                view.begin_probe_drag(window, cx);
                assert_eq!(view.scroll.offset(), Point::default());
                (
                    view.verify_copy_range(),
                    view.entries[view.body_entries.start]
                        .bounds
                        .get()
                        .unwrap()
                        .top(),
                )
            })
            .expect("begin drag on painted text");

        for _ in 0..4 {
            draw(window, cx);
            window
                .update(cx, |view, window, cx| {
                    assert!(view.drag_frame_pending, "paint queued a drag frame");
                    // The hidden window stays inactive. Exercise the same post-guard
                    // frame step directly, alternating it with real GPUI draws.
                    view.advance_drag_frame(window.scale_factor(), cx);
                })
                .expect("advance held drag against the last drawn rows");
        }

        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                let after = view.verify_copy_range();
                let scroll = view.scroll.offset();
                assert_eq!(after.0, before.0, "keep the selection anchor");
                assert!(after.1 > before.1, "drag selects rows exposed by scrolling");
                assert!(scroll.y < px(0.));
                assert!(view.entries[view.body_entries.start].bounds.get().unwrap().top() < first_row_top);
                let copied = view.selected_text().unwrap();
                println!(
                    "Hidden GPUI draws with synthetic inputs: archive rows {}–{} → {}–{}, Y offset 0 → {:?}, copied {} rows ({} UTF-8 bytes), first={:?}, last={:?}",
                    before.0 + 1,
                    before.1 + 1,
                    after.0 + 1,
                    after.1 + 1,
                    scroll.y,
                    copied.lines().count(),
                    copied.len(),
                    copied.lines().next().unwrap(),
                    copied.lines().last().unwrap(),
                );

                assert!(view.drag_frame_pending);
                view.stop_drag();
                view.advance_drag_frame(window.scale_factor(), cx);
                assert_eq!(view.scroll.offset(), scroll);
                assert_eq!(view.verify_copy_range(), after);
                println!("Stopping the drag preserves scroll position and copied range.");
            })
            .expect("verify scroll, selection and stopping");

        window
            .update(cx, |view, _, cx| view.set_state(history("换页"), cx))
            .expect("prepare another history page");
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.begin_probe_drag(window, cx);
                assert!(view.drag.is_some());
                assert!(view.selected_text().is_some());
                view.set_state(history("新正文"), cx);
                view.advance_drag_frame(window.scale_factor(), cx);
                assert!(view.drag.is_none());
                assert!(view.selection.is_none());
                assert!(view.selected_text().is_none());
                assert_eq!(view.scroll.offset(), Point::default());
                println!("Replacing the page during a drag clears selection and keeps the replacement at offset zero.");
            })
            .expect("verify page replacement stops an in-progress drag");
    }

    fn verify_panel_selection(window: WindowHandle<HistoryView>, cx: &mut App) {
        let mut state = history("跨区");
        if let HistoryViewState::Ready { page, .. } = &mut state {
            page.has_more_before = Some(true);
            page.has_more_after = Some(true);
            page.gaps.push(HistoryGap {
                segment_index: 3,
                code: HistoryGapCode::MissingSegment,
                message: "synthetic history gap".into(),
            });
        }
        window
            .update(cx, |view, _, cx| view.set_state(state, cx))
            .unwrap();
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                let first_line = view.body_entries.start + 1;
                let third_line = first_line + 2;
                view.press(view.entry_point(third_line, false), window, cx);
                assert!(view.drag.as_ref().expect("body drag started").scroll_body);
                view.extend_selection(view.entry_point(view.entries.len() - 1, true), cx);
                let lines = &view.page().unwrap().lines;
                let mut expected: Vec<_> = lines[2..].iter().map(|line| line.text.as_str()).collect();
                expected.extend([
                    "下方还有更新记录",
                    "命中行前后各最多 250 行",
                    "1 处历史缺口",
                    "Esc 返回",
                ]);
                let copied = view.selected_text().unwrap();
                assert_eq!(copied, expected.join("\n"));
                println!("Body → footer copies {} items through Esc 返回, with no line numbers.", copied.lines().count());
                view.stop_drag();

                view.press(view.entry_point(third_line, false), window, cx);
                view.extend_selection(view.entry_point(0, false), cx);
                let lines = &view.page().unwrap().lines;
                let expected = [
                    "历史记录",
                    "只读",
                    "1/1",
                    "返回实时终端",
                    "上方还有更早记录",
                    &lines[0].text,
                    &lines[1].text,
                    "",
                ].join("\n");
                assert_eq!(view.selected_text().as_deref(), Some(expected.as_str()));
                println!("Body → heading copies all title labels, the earlier-history hint, and the trailing newline.");
                view.stop_drag();

                view.press(view.return_bounds.get().unwrap().center(), window, cx);
                assert!(view.drag.is_none(), "the return button can receive its own click");

                view.press(view.entry_point(0, false), window, cx);
                assert!(!view.drag.as_ref().expect("heading drag started").scroll_body);
                let viewport = view.scroll.bounds();
                view.extend_selection(point(viewport.center().x, viewport.bottom() - px(1.)), cx);
                let before = view.scroll.offset();
                view.advance_drag_frame(window.scale_factor(), cx);
                assert_eq!(view.scroll.offset(), before);
                println!("Starting on the heading selects into the body without autoscrolling it.");
            })
            .expect("verify selection spanning the history panel");
    }

    pub fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(120.), px(120.)),
                            size(px(420.), px(260.)),
                        ))),
                        show: false,
                        focus: false,
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|cx| {
                            let mut view = HistoryView::new(cx);
                            view.set_state(history("归档"), cx);
                            view
                        })
                    },
                )
                .expect("open hidden history drag probe");
            verify(window, cx);
            verify_panel_selection(window, cx);
            cx.quit();
        });
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
}
