//! Exercise the production drag handlers against a laid-out history view.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod ui_text {
    include!("../src/ui_text.rs");
}

#[cfg(target_os = "macos")]
mod probe {
    include!("../src/history_view.rs");

    use matou_native_desktop::assets::NativeAssets;
    use matou_native_runtime::history::{HistoryCursor, HistoryLine};

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
        fn begin_probe_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            let geometry = self.rows[0].text.borrow();
            let layout = geometry
                .as_ref()
                .expect("first row's rendered text geometry");
            let row = self.rows[0].bounds.get().expect("first row's bounds");
            let position = point(layout.bounds.left() + px(0.25), row.center().y);
            drop(geometry);
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
            let start = self.selection.expect("drag started").anchor;
            assert_eq!((start.row, start.byte), (0, 0));
            let viewport = self.scroll.bounds();
            assert!(self.scroll.max_offset().height > px(100.));
            let pointer = point(viewport.center().x, viewport.bottom() - px(1.));
            self.extend_selection(pointer, cx);
        }

        fn verify_copy_range(&self) -> (usize, usize) {
            let range = self.selection.expect("non-empty selection").ordered();
            let page = self.page().expect("visible history page");
            assert_eq!(range.start.byte, 0);
            assert_eq!(range.end.byte, page.lines[range.end.row].text.len());
            let expected = page.lines[range.start.row..=range.end.row]
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                self.selected_text().as_deref(),
                Some(expected.as_str()),
                "copy must match the new Unicode rows byte for byte, with no line numbers"
            );
            (range.start.row, range.end.row)
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
                    view.rows[0].bounds.get().unwrap().top(),
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
                assert!(view.rows[0].bounds.get().unwrap().top() < first_row_top);
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
            cx.quit();
        });
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
}
