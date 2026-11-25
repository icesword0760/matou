//! Exercise the real terminal surface and its handlers in a hidden GPUI window.
#[cfg(target_os = "macos")]
#[path = "../src/terminal_input.rs"]
mod terminal_input;

#[cfg(target_os = "macos")]
#[path = "../src/terminal_scrollbar.rs"]
mod terminal_scrollbar;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod probe {
    include!("../src/terminal_surface.rs");

    use std::{cell::RefCell, rc::Rc};

    use gpui::{
        AnyWindowHandle, AppContext as _, Application, Keystroke, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, ScrollDelta, WindowBounds, WindowHandle, WindowOptions,
    };
    use matou_native_desktop::assets::NativeAssets;
    use matou_native_terminal::{Terminal, TerminalOptions};

    #[derive(Debug, PartialEq, Eq)]
    enum Observed {
        Absolute(u64, u64),
        Lines(i32),
        Input(Vec<u8>),
        Select,
        Other,
    }

    fn observed(event: &SurfaceEvent) -> Observed {
        match &event.kind {
            SurfaceEventKind::ScrollTo { row, request_id } => Observed::Absolute(*row, *request_id),
            SurfaceEventKind::Scroll(lines) => Observed::Lines(*lines),
            SurfaceEventKind::Input(bytes) => Observed::Input(bytes.clone()),
            SurfaceEventKind::Select(_) => Observed::Select,
            _ => Observed::Other,
        }
    }

    fn draw(window: WindowHandle<TerminalSurface>, cx: &mut App) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .expect("draw the hidden terminal");
    }

    fn assert_position(view: &TerminalSurface, pixels: f64, request_id: u64) -> u64 {
        let position = view.scrollbar_position.expect("requested scroll position");
        let expected_row = (pixels / f64::from(f32::from(view.metrics.height))).round() as u64;
        assert_eq!(position.pixels, pixels);
        assert_eq!(position.row, expected_row);
        assert_eq!(position.request_id, request_id);
        expected_row
    }

    fn verify(window: WindowHandle<TerminalSurface>, cx: &mut App) -> anyhow::Result<()> {
        let mut terminal = Terminal::new(TerminalOptions::new(48, 16, 9, 18))?;
        for index in 0..80 {
            terminal.feed(format!("screen row {index:03}\r\n").as_bytes());
        }
        let modes = terminal.input_modes()?;
        let bottom = terminal.screen()?;
        assert_eq!((bottom.cols, bottom.rows), (48, 16));
        assert!(bottom.scrollbar.offset > 50);
        terminal.scroll(6)?;
        let authority = terminal.screen()?;
        assert_eq!(authority.scrollbar.offset + 6, bottom.scrollbar.offset);

        let events = Rc::new(RefCell::new(Vec::new()));
        let _subscription = AnyWindowHandle::from(window).update(cx, |root, _, cx| {
            let view = root.downcast::<TerminalSurface>().unwrap();
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &SurfaceEvent, _| {
                events.borrow_mut().push(observed(event));
            })
        })?;
        window.update(cx, |view, _, cx| {
            view.set_screen(bottom.clone(), modes, None, 0, cx);
        })?;
        draw(window, cx);
        events.borrow_mut().clear();

        let (click, track_raw) = window.update(cx, |view, _, _| {
            let outer = view
                .scrollbar_bounds
                .expect("real painted scrollbar bounds");
            assert_eq!(outer.top(), px(14.));
            assert_eq!(outer.size.height, view.metrics.height * 16.);
            let track = view.scrollbar_metrics().unwrap();
            let click = point(outer.right() - px(2.5), outer.center().y);
            let y = f64::from(f32::from(click.y - outer.top()));
            assert!(
                y < track.thumb_top,
                "click the track above the initial thumb"
            );
            view.reveal_scrollbar();
            (click, track.track_position(y))
        })?;

        let existing_focus = cx.focus_handle();
        let track_row = window.update(cx, |view, window, cx| {
            window.focus(&existing_focus);
            let consumed = view.scrollbar_down(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: click,
                    click_count: 1,
                    ..Default::default()
                },
                cx,
            );
            assert!(
                consumed,
                "the surface listener must skip its text-selection path"
            );
            assert!(existing_focus.is_focused(window));
            assert!(view.scrollbar_drag.is_some());
            assert_position(view, track_raw, 1)
        })?;
        assert_eq!(events.take(), [Observed::Absolute(track_row, 1)]);
        println!("Track click: {track_raw}px → row {track_row}, sole ScrollTo; focus preserved.");

        let (first_row, latest_row, latest_raw) = window.update(cx, |view, _, cx| {
            let capture = view.scrollbar_drag.unwrap();
            let mut move_up = |delta, request_id| {
                let position = point(click.x, click.y - px(delta));
                let event = MouseMoveEvent {
                    position,
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                };
                assert!(view.scrollbar_move(&event, true, true, cx));
                let observed_delta = f64::from(f32::from(position.y - capture.pointer.y));
                let expected = capture.metrics.drag_position(observed_delta);
                (assert_position(view, expected, request_id), expected)
            };
            let (first_row, _) = move_up(12., 2);
            let (latest_row, latest_raw) = move_up(24., 3);
            assert!(latest_row < first_row && first_row < track_row);
            (first_row, latest_row, latest_raw)
        })?;
        assert_eq!(
            events.take(),
            [
                Observed::Absolute(first_row, 2),
                Observed::Absolute(latest_row, 3)
            ]
        );
        println!(
            "Held moves use the captured thumb origin: rows {first_row}, {latest_row} ({latest_raw}px)."
        );

        window.update(cx, |view, window, cx| {
            assert_ne!(authority.scrollbar.offset, latest_row);
            view.set_screen(authority.clone(), modes, None, 2, cx);
            assert_eq!(assert_position(view, latest_raw, 3), latest_row);
            view.set_screen(authority.clone(), modes, None, 3, cx);
            assert!(view.scrollbar_position.is_none());
            assert_eq!(view.screen.as_ref().unwrap().scrollbar.offset, authority.scrollbar.offset);
            println!(
                "Stale request 2 retains row {latest_row}; current request 3 with identical authoritative frame clears the optimistic position to row {}.",
                authority.scrollbar.offset,
            );

            let padding = point(click.x, window.viewport_size().height - px(6.));
            assert!(!view.scrollbar_bounds.unwrap().contains(&padding));
            assert!(view.scrollbar_up(&MouseUpEvent {
                button: MouseButton::Left,
                position: padding,
                ..Default::default()
            }, cx));
            assert!(view.scrollbar_drag.is_none());
            assert!(!view.scrollport_hovered);
            println!("Release in bottom padding ends the drag and leaves scrollport hover off.");

            view.set_screen(bottom.clone(), modes, None, 3, cx);
            view.scroll_to_pixels(latest_raw, cx);
            assert!(view.scrollbar_position.is_some());
            view.send(b"x".to_vec(), cx);
            assert!(view.scrollbar_position.is_none());
        })?;
        assert_eq!(
            events.take(),
            [
                Observed::Absolute(latest_row, 4),
                Observed::Lines(i32::MIN),
                Observed::Input(b"x".to_vec())
            ]
        );
        println!("Typing with a scroll request in flight queues return to bottom before input.");

        window.update(cx, |view, _, cx| {
            view.scroll_to_pixels(latest_raw, cx);
            let expected = view.scrollbar_metrics().unwrap().clamp(latest_raw - 100.);
            view.on_scroll(
                &ScrollWheelEvent {
                    position: click,
                    delta: ScrollDelta::Lines(point(0., 2.)),
                    ..Default::default()
                },
                cx,
            );
            assert_position(view, expected, 6);
        })?;
        let wheel_events = events.take();
        assert_eq!(wheel_events.len(), 2);
        assert_eq!(wheel_events[0], Observed::Absolute(latest_row, 5));
        assert!(matches!(wheel_events[1], Observed::Absolute(row, 6) if row < latest_row));
        println!(
            "Viewport wheel advances the pending pixel position and queues an absolute target."
        );

        window.update(cx, |view, _, cx| {
            view.surface_hovered = false;
            view.set_screen(authority, modes, None, 6, cx);
            assert!(view.scrollbar_fade.is_some());
        })?;
        draw(window, cx);
        window.update(cx, |view, _, _| {
            assert!(!view.surface_hovered);
            assert!(!view.scrollbar_frame_pending);
        })?;
        println!(
            "An unhovered terminal draws a changed scrollback without starting a fade frame loop."
        );
        events.borrow_mut().clear();
        let (track, drag) = window.update(cx, |view, window, cx| -> anyhow::Result<_> {
            let previous_row = view.screen.as_ref().unwrap().scrollbar.offset;
            view.reveal_scrollbar();
            assert!(view.scrollbar_down(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: click,
                    click_count: 1,
                    ..Default::default()
                },
                cx,
            ));
            let track = view.scrollbar_position.expect("track request");
            assert!(view.scrollbar_move(
                &MouseMoveEvent {
                    position: point(click.x, click.y - px(24.)),
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                },
                true,
                true,
                cx,
            ));
            let drag = view.scrollbar_position.expect("final drag request");
            assert_ne!(previous_row, drag.row);

            let padding = point(click.x, window.viewport_size().height - px(6.));
            assert!(!view.scrollbar_bounds.unwrap().contains(&padding));
            assert!(view.scrollbar_up(
                &MouseUpEvent {
                    button: MouseButton::Left,
                    position: padding,
                    ..Default::default()
                },
                cx,
            ));
            assert!(!view.scrollbar_revealed);
            let fade = view
                .scrollbar_fade
                .as_ref()
                .map(|fade| (fade.start, fade.target));
            assert!(fade.is_none_or(|(_, target)| target == 0.));

            terminal.scroll(i32::try_from(
                i128::from(previous_row) - i128::from(drag.row),
            )?)?;
            let late = terminal.screen()?;
            assert_eq!(late.scrollbar.offset, drag.row);
            view.set_screen(late, modes, None, drag.request_id, cx);
            assert_eq!(view.screen.as_ref().unwrap().scrollbar.offset, drag.row);
            assert!(
                !view.scrollbar_revealed,
                "the late response must not reveal the bar"
            );
            assert_eq!(
                view.scrollbar_fade
                    .as_ref()
                    .map(|fade| (fade.start, fade.target)),
                fade
            );
            Ok((track, drag))
        })??;
        assert_eq!(
            events.take(),
            [
                Observed::Absolute(track.row, track.request_id),
                Observed::Absolute(drag.row, drag.request_id),
            ]
        );
        println!(
            "The response to drag request {} arrives after padding release, updates the viewport to row {}, and leaves the hide/fade state intact.",
            drag.request_id, drag.row
        );

        verify_ime(
            window,
            cx,
            &events,
            bottom.clone(),
            &mut terminal,
            latest_raw,
        )?;
        verify_ime_drag(window, cx, &events, bottom, modes)?;
        verify_resized_targets(window, cx, &events)?;
        window.update(cx, |view, _, cx| {
            let old_generation = view.transport_generation;
            let dimensions = view
                .last_dimensions
                .expect("actual hidden paint dimensions");
            let prior_offset = view.screen.as_ref().unwrap().scrollbar.offset;
            view.begin_transport_generation(old_generation + 1, cx);
            view.emit_resize_for_generation(old_generation, dimensions, cx);
            assert_eq!(view.screen.as_ref().unwrap().scrollbar.offset, prior_offset);
            assert!(view.last_dimensions == Some(dimensions));
            assert!(view.scrollbar_drag.is_none() && view.scrollbar_position.is_none());
            assert!(view.drag_scroll_timer.is_none() && view.scrollbar_hide_timer.is_none());
            assert!(!view.composition.is_active() && view.caret.is_none());
            view.set_transport_available(true, cx);
            view.set_input_disabled(false, cx);
        })?;
        assert!(events.take().is_empty(), "retired resize emitted an event");
        println!(
            "Generation reset preserves the painted screen/target, retires interaction tasks and rejects an old deferred resize."
        );
        Ok(())
    }

    fn verify_resized_targets(
        window: WindowHandle<TerminalSurface>,
        cx: &mut App,
        events: &RefCell<Vec<Observed>>,
    ) -> anyhow::Result<()> {
        for gesture in ["wheel", "track", "drag"] {
            let mut terminal = Terminal::new(TerminalOptions::new(48, 16, 9, 18))?;
            for index in 0..80 {
                terminal.feed(format!("screen row {index:03}\r\n").as_bytes());
            }
            let modes = terminal.input_modes()?;
            terminal.scroll_to(16, false)?;
            terminal.resize(48, 20, 9, 18)?;
            let resized = terminal.screen()?;
            assert_eq!(resized.scrollbar.offset, 12);
            assert_eq!(resized.resize_position_row, Some(16));
            window.update(cx, |view, _, cx| {
                view.scrollbar_position = None;
                view.scrollbar_drag = None;
                view.set_screen(resized, modes, None, view.scroll_request_id, cx);
            })?;
            draw(window, cx);
            events.borrow_mut().clear();
            let (row, request_id) = window.update(cx, |view, _, cx| {
                view.reveal_scrollbar();
                let outer = view.scrollbar_bounds.unwrap();
                let metrics = view.scrollbar_metrics().unwrap();
                let left = outer.right() - px(2.5);
                let row_height = f64::from(f32::from(view.metrics.height));
                assert_eq!(metrics.position, 16. * row_height);
                let expected = match gesture {
                    "wheel" => {
                        view.on_scroll(
                            &ScrollWheelEvent {
                                position: outer.center(),
                                delta: ScrollDelta::Pixels(point(px(0.), px(-4.))),
                                ..Default::default()
                            },
                            cx,
                        );
                        metrics.position + 5.
                    }
                    _ => {
                        let y = if gesture == "drag" {
                            metrics.thumb_top + metrics.thumb_size / 2.
                        } else {
                            metrics.viewport * 0.75
                        };
                        let grip = point(left, outer.top() + px(y as f32));
                        assert!(view.scrollbar_down(
                            &MouseDownEvent {
                                button: MouseButton::Left,
                                position: grip,
                                click_count: 1,
                                ..Default::default()
                            },
                            cx,
                        ));
                        if gesture == "drag" {
                            assert!(view.scrollbar_move(
                                &MouseMoveEvent {
                                    position: grip + point(px(0.), px(12.)),
                                    pressed_button: Some(MouseButton::Left),
                                    ..Default::default()
                                },
                                true,
                                true,
                                cx,
                            ));
                            metrics.drag_position(12.)
                        } else {
                            metrics.track_position(f64::from(f32::from(grip.y - outer.top())))
                        }
                    }
                };
                let request = view.scroll_request_id;
                (assert_position(view, expected, request), request)
            })?;
            assert_eq!(events.take(), [Observed::Absolute(row, request_id)]);
            terminal.scroll_to(row, true)?;
            assert_eq!(terminal.screen()?.scrollbar.offset, row);
            println!("First {gesture} after growing the window lands on viewport row {row}.");
        }
        Ok(())
    }

    fn verify_ime_drag(
        window: WindowHandle<TerminalSurface>,
        cx: &mut App,
        events: &RefCell<Vec<Observed>>,
        bottom: Screen,
        modes: InputModes,
    ) -> anyhow::Result<()> {
        for release in [true, false] {
            events.borrow_mut().clear();
            let (pending, wheel, moved) = window.update(cx, |view, window, cx| {
                view.set_screen(bottom.clone(), modes, None, view.scroll_request_id, cx);
                view.reveal_scrollbar();
                let bounds = view.scrollbar_bounds.expect("painted scrollbar bounds");
                let metrics = view.scrollbar_metrics().unwrap();
                let pointer = point(
                    bounds.right() - px(2.5),
                    bounds.top() + px((metrics.thumb_top + metrics.thumb_size / 2.) as f32),
                );
                assert!(view.scrollbar_down(
                    &MouseDownEvent {
                        button: MouseButton::Left,
                        position: pointer,
                        click_count: 1,
                        ..Default::default()
                    },
                    cx,
                ));
                let capture = view.scrollbar_drag.expect("left-button thumb capture");
                let first_move = MouseMoveEvent {
                    position: point(pointer.x, pointer.y - px(24.)),
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                };
                assert!(view.scrollbar_move(&first_move, true, true, cx));
                let pending = view.scrollbar_position.expect("queued absolute drag");
                assert_eq!(
                    view.scrollbar_metrics().unwrap().position,
                    metrics.position,
                    "the thumb must still represent the displayed rows"
                );
                view.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
                assert_eq!(view.marked_text_range(window, cx), Some(0..2));
                assert!(view.scrollbar_move(&first_move, true, true, cx));
                assert!(view.scrollbar_position.is_none());

                view.scrollbar_wheel(
                    &ScrollWheelEvent {
                        position: first_move.position,
                        delta: ScrollDelta::Pixels(point(px(0.), px(32.))),
                        ..Default::default()
                    },
                    cx,
                );
                let wheel = view.scrollbar_position.expect("held wheel scroll");
                assert_eq!(wheel.pixels, metrics.position - 40.);
                assert!(wheel.row < bottom.scrollbar.offset);
                assert!(view.scrollbar_move(&first_move, true, true, cx));
                assert_eq!(
                    view.scrollbar_position.unwrap().request_id,
                    wheel.request_id
                );

                // The return-to-live request must not rebase the held gesture.
                let position = point(pointer.x, pointer.y - px(36.));
                assert!(
                    view.scrollbar_move(
                        &MouseMoveEvent {
                            position,
                            pressed_button: Some(MouseButton::Left),
                            ..Default::default()
                        },
                        true,
                        true,
                        cx,
                    ),
                    "preedit must retain capture for the next held scrollbar move"
                );
                let expected = capture
                    .metrics
                    .drag_position(f64::from(f32::from(position.y - capture.pointer.y)));
                let moved = view.scrollbar_position.expect("continued absolute drag");
                assert_position(view, expected, pending.request_id + 2);
                assert!(moved.row < pending.row);
                view.unmark_text(window, cx);
                assert_eq!(view.marked_text_range(window, cx), None);

                if release {
                    assert!(view.scrollbar_up(
                        &MouseUpEvent {
                            button: MouseButton::Left,
                            position,
                            ..Default::default()
                        },
                        cx,
                    ));
                } else {
                    assert!(!view.scrollbar_move(
                        &MouseMoveEvent {
                            position,
                            pressed_button: None,
                            ..Default::default()
                        },
                        true,
                        true,
                        cx,
                    ));
                }
                assert!(view.scrollbar_drag.is_none());
                assert!(!view.scrollbar_move(
                    &MouseMoveEvent {
                        position: point(pointer.x, pointer.y - px(48.)),
                        pressed_button: Some(MouseButton::Left),
                        ..Default::default()
                    },
                    true,
                    true,
                    cx,
                ));
                (pending, wheel, moved)
            })?;
            assert_eq!(
                events.take(),
                [
                    Observed::Absolute(pending.row, pending.request_id),
                    Observed::Lines(i32::MIN),
                    Observed::Absolute(wheel.row, wheel.request_id),
                    Observed::Absolute(moved.row, moved.request_id),
                ]
            );
            println!(
                "Held preedit: queued row {} → return-to-live → wheel row {} → original-origin row {}; stationary events leave the viewport alone.",
                pending.row, wheel.row, moved.row,
            );
        }
        Ok(())
    }

    fn verify_ime(
        window: WindowHandle<TerminalSurface>,
        cx: &mut App,
        events: &RefCell<Vec<Observed>>,
        bottom: Screen,
        terminal: &mut Terminal,
        scroll_pixels: f64,
    ) -> anyhow::Result<()> {
        let history = terminal.screen()?;
        let modes = terminal.input_modes()?;
        assert!(bottom.cursor_has_position);
        assert!(!history.cursor_has_position);
        let key = KeyDownEvent {
            keystroke: Keystroke::parse("backspace")?,
            is_held: false,
        };

        window.update(cx, |view, _, cx| {
            view.set_screen(bottom.clone(), modes, None, view.scroll_request_id, cx);
        })?;
        draw(window, cx);
        let caret = window.update(cx, |view, _, cx| {
            let caret = view.caret.expect("painted live cursor");
            assert!(caret.top() > view.grid.unwrap().bounds.top());
            view.set_screen(history.clone(), modes, None, view.scroll_request_id, cx);
            caret
        })?;
        draw(window, cx);
        events.borrow_mut().clear();
        window.update(cx, |view, window, cx| {
            view.replace_and_mark_text_in_range(None, "a", Some(1..1), window, cx);
            assert_eq!(view.marked_text_range(window, cx), Some(0..1));
        })?;
        assert_eq!(events.take(), [Observed::Lines(i32::MIN)]);
        draw(window, cx);
        window.update(cx, |view, window, cx| {
            assert_eq!(view.caret, Some(caret));
            assert_eq!(view.composition_caret(view.grid.unwrap()), Some(caret));
            let marked = view.bounds_for_range(0..1, caret, window, cx).unwrap();
            assert_eq!(marked.origin, caret.origin);
            // Model an acknowledged return to live, then another wheel scroll
            // before the composition has finished.
            view.set_screen(bottom.clone(), modes, None, view.scroll_request_id, cx);
            view.set_screen(history.clone(), modes, None, view.scroll_request_id, cx);
            view.handle_key(&key, window, cx);
        })?;
        assert_eq!(events.take(), [Observed::Lines(i32::MIN)]);
        println!(
            "Marking text and then typing during a second scrollback visit request only the live position; the last valid candidate anchor is preserved."
        );

        window.update(cx, |view, window, cx| {
            view.unmark_text(window, cx);
            assert_eq!(view.marked_text_range(window, cx), None);
            assert!(view.composition_caret(view.grid.unwrap()).is_none());
        })?;
        assert!(events.take().is_empty());
        println!("Cancelling preedit never writes to the terminal.");

        terminal.scroll(-100)?;
        terminal.feed(b"\x1b[4C");
        let returned = terminal.screen()?;
        assert_eq!(returned.scrollbar.offset, bottom.scrollbar.offset);
        assert_ne!(returned.cursor_col, bottom.cursor_col);
        window.update(cx, |view, window, cx| {
            view.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
            view.set_screen(returned, modes, None, view.scroll_request_id, cx);
        })?;
        assert_eq!(events.take(), [Observed::Lines(i32::MIN)]);
        draw(window, cx);
        window.update(cx, |view, window, cx| {
            let live_caret = view.caret.expect("repainted live cursor");
            assert_eq!(live_caret.left(), caret.left() + view.metrics.width * 4.);
            assert_eq!(live_caret.top(), caret.top());
            assert!(view.composition.is_active());
            assert_eq!(view.composition_caret(view.grid.unwrap()), Some(live_caret));
            let mark = view.bounds_for_range(0..2, live_caret, window, cx).unwrap();
            assert_eq!(mark.origin, live_caret.origin);
            view.replace_text_in_range(None, "你", window, cx);
            assert!(!view.composition.is_active());
            assert!(view.composition_caret(view.grid.unwrap()).is_none());
            view.unmark_text(window, cx);
        })?;
        assert_eq!(events.take(), [Observed::Input("你".as_bytes().to_vec())]);
        println!(
            "After repainting the returned live cursor, committing preedit emits the chosen UTF-8 bytes exactly once."
        );

        let pending = window.update(cx, |view, window, cx| {
            view.scroll_to_pixels(scroll_pixels, cx);
            let pending = view
                .scrollbar_position
                .expect("an unacknowledged absolute scroll");
            assert_eq!(
                view.screen.as_ref().unwrap().scrollbar.offset,
                bottom.scrollbar.offset
            );
            view.replace_and_mark_text_in_range(None, "a", Some(1..1), window, cx);
            assert!(view.scrollbar_position.is_none());
            view.unmark_text(window, cx);
            pending
        })?;
        assert_eq!(
            events.take(),
            [
                Observed::Absolute(pending.row, pending.request_id),
                Observed::Lines(i32::MIN)
            ]
        );
        println!(
            "Preedit follows a pending history jump with a return-to-live command even while the displayed frame is already at the bottom."
        );

        window.update(cx, |view, window, cx| {
            view.set_screen(history, modes, None, view.scroll_request_id, cx);
            view.set_input_disabled(true, cx);
            view.replace_and_mark_text_in_range(None, "a", Some(1..1), window, cx);
            assert!(view.marked_text_range(window, cx).is_none());
            view.handle_key(&key, window, cx);
            view.replace_text_in_range(None, "字", window, cx);
            assert!(!view.composition.is_active());
            view.set_input_disabled(false, cx);
        })?;
        assert!(events.take().is_empty());
        println!(
            "Disabled input ignores marking, normal keys, and commits without scrolling or sending bytes."
        );

        terminal.feed(b"\x1b[1;4H");
        let live = terminal.screen()?;
        assert_eq!((live.cursor_col, live.cursor_row), (3, 0));
        window.update(cx, |view, window, cx| {
            view.set_screen(live, modes, None, view.scroll_request_id, cx);
            view.replace_and_mark_text_in_range(None, "ni'hao", Some(6..6), window, cx);
        })?;
        draw(window, cx);
        let anchor = window.update(cx, |view, _, _| {
            let grid = view.grid.unwrap();
            let anchor = view.composition_caret(grid).unwrap();
            assert_eq!(anchor, view.metrics.cell(grid.bounds.origin, 3, 0, 1));
            anchor
        })?;
        for (lines, visible) in [(1, true), (20, false)] {
            terminal.scroll(lines)?;
            let scrolled = terminal.screen()?;
            assert_eq!(scrolled.cursor_has_position, visible);
            window.update(cx, |view, _, cx| {
                view.set_screen(scrolled, modes, None, view.scroll_request_id, cx);
            })?;
            draw(window, cx);
            window.update(cx, |view, _, _| {
                let grid = view.grid.unwrap();
                assert_eq!(view.composition_caret(grid), Some(anchor));
                assert_eq!(
                    view.caret.unwrap(),
                    view.metrics.cell(grid.bounds.origin, 3, 1, 1)
                );
            })?;
        }
        window.update(cx, |view, window, cx| view.unmark_text(window, cx))?;
        assert!(events.take().is_empty());
        println!(
            "The composition view stays on the active row across partial and full scrollback."
        );
        Ok(())
    }

    fn verify_file_drop_async(
        window: WindowHandle<TerminalSurface>,
        cx: &mut gpui::AsyncApp,
    ) -> anyhow::Result<()> {
        use cocoa::{
            appkit::NSFilenamesPboardType,
            base::{BOOL, NO, YES, id, nil},
            foundation::{NSArray, NSPoint, NSString},
        };
        use objc::{
            class,
            declare::ClassDecl,
            msg_send,
            runtime::{Class, Object, Sel},
            sel, sel_impl,
        };
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let drop_events = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
        let sink = drop_events.clone();
        #[allow(deprecated, unexpected_cfgs)]
        let native_window: id = cx.update(|cx| {
            let entity = window.entity(cx).unwrap();
            cx.subscribe(&entity, move |_, event: &SurfaceEvent, _| {
                if let SurfaceEventKind::Input(bytes) = &event.kind {
                    sink.borrow_mut().push(bytes.clone());
                }
            })
            .detach();
            draw(window, cx);
            AnyWindowHandle::from(window)
                .update(cx, |_, window, _| {
                    let RawWindowHandle::AppKit(handle) =
                        HasWindowHandle::window_handle(window).unwrap().as_raw()
                    else {
                        panic!("expected AppKit window");
                    };
                    unsafe {
                        let view = handle.ns_view.as_ptr() as id;
                        let ns_window: id = msg_send![view, window];
                        ns_window
                    }
                })
                .unwrap()
        })?;

        #[allow(deprecated, unexpected_cfgs)]
        unsafe {
            let pasteboard: id = msg_send![class!(NSPasteboard), pasteboardWithUniqueName];
            let items = NSArray::arrayWithObjects(
                nil,
                &[
                    NSString::alloc(nil).init_str("/tmp/matou-mac-drop-test/drop-test.txt"),
                    NSString::alloc(nil).init_str("/tmp/matou-mac-drop-test/drop test.txt"),
                ],
            );
            let types = NSArray::arrayWithObjects(nil, &[NSFilenamesPboardType]);
            let _: isize = msg_send![pasteboard, declareTypes: types owner: nil];
            let ok: BOOL =
                msg_send![pasteboard, setPropertyList: items forType: NSFilenamesPboardType];
            assert_eq!(ok, YES, "populate unique drag pasteboard");

            let cls: *const Class = Class::get("MatouProbeDraggingInfo").unwrap_or_else(|| {
                let mut decl = ClassDecl::new("MatouProbeDraggingInfo", class!(NSObject))
                    .expect("declare MatouProbeDraggingInfo");
                decl.add_ivar::<*mut std::ffi::c_void>("_pb");
                extern "C" fn dragging_pasteboard(this: &Object, _: Sel) -> id {
                    unsafe { *this.get_ivar::<*mut std::ffi::c_void>("_pb") as id }
                }
                extern "C" fn dragging_location(_: &Object, _: Sel) -> NSPoint {
                    NSPoint::new(80.0, 160.0)
                }
                decl.add_method(
                    sel!(draggingPasteboard),
                    dragging_pasteboard as extern "C" fn(&Object, Sel) -> id,
                );
                decl.add_method(
                    sel!(draggingLocation),
                    dragging_location as extern "C" fn(&Object, Sel) -> NSPoint,
                );
                decl.register()
            });
            let info: id = msg_send![cls, new];
            (*info).set_ivar::<*mut std::ffi::c_void>("_pb", pasteboard as *mut std::ffi::c_void);

            // 1. draggingEntered -> active drag; draggingExited -> clears drag without input.
            let op: usize = msg_send![native_window, draggingEntered: info];
            assert_eq!(op, 1, "NSDragOperationCopy expected on draggingEntered");
            cx.update(|cx| {
                assert!(
                    cx.has_active_drag(),
                    "draggingEntered must set GPUI active ExternalPaths drag"
                );
                draw(window, cx);
            })?;
            let _: () = msg_send![native_window, draggingExited: info];
            cx.update(|cx| {
                assert!(
                    !cx.has_active_drag(),
                    "draggingExited must clear active drag via stop_active_drag"
                );
            })?;
            assert!(
                drop_events.borrow_mut().drain(..).next().is_none(),
                "draggingExited must not emit terminal input"
            );

            // 2. draggingEntered + performDragOperation -> inserts POSIX plain & space paths and focuses terminal.
            cx.update(|cx| {
                AnyWindowHandle::from(window)
                    .update(cx, |_, window, _| window.blur())
                    .unwrap();
            })?;
            let op: usize = msg_send![native_window, draggingEntered: info];
            assert_eq!(op, 1);
            cx.update(|cx| draw(window, cx))?;
            let performed: BOOL = msg_send![native_window, performDragOperation: info];
            assert_eq!(performed, YES, "performDragOperation must succeed");
            assert_eq!(
                drop_events.borrow_mut().drain(..).collect::<Vec<_>>(),
                vec![
                    b" /tmp/matou-mac-drop-test/drop-test.txt \"/tmp/matou-mac-drop-test/drop test.txt\""
                        .to_vec()
                ]
            );
            let focused = cx.update(|cx| {
                window
                    .update(cx, |view, window, _| view.focus.is_focused(window))
                    .unwrap()
            })?;
            assert!(focused, "terminal must receive focus after macOS file drop");

            // 3. Disabled terminal ignores performDragOperation.
            cx.update(|cx| {
                window
                    .update(cx, |view, _, cx| view.set_input_disabled(true, cx))
                    .unwrap();
                draw(window, cx);
            })?;
            let _: usize = msg_send![native_window, draggingEntered: info];
            let _: BOOL = msg_send![native_window, performDragOperation: info];
            assert!(
                drop_events.borrow_mut().drain(..).next().is_none(),
                "disabled terminal must ignore macOS file drops"
            );
            cx.update(|cx| {
                window
                    .update(cx, |view, _, cx| view.set_input_disabled(false, cx))
                    .unwrap();
            })?;

            let _: () = msg_send![pasteboard, releaseGlobally];
            let _: () = msg_send![info, release];
            let _ = NO;
        }
        println!(
            "Hidden macOS NSWindow NSDraggingDestination verified draggingEntered/Exited clear, POSIX plain+space path drop, focus, and disabled guard."
        );
        Ok(())
    }

    pub fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
            #[allow(deprecated, unexpected_cfgs)]
            unsafe {
                use cocoa::appkit::NSApp;
                use objc::{msg_send, sel, sel_impl};
                let app = NSApp();
                // NSApplicationActivationPolicyProhibited = 2: prevents background probe from taking focus.
                let _: bool = msg_send![app, setActivationPolicy: 2isize];
            }
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(120.), px(120.)),
                            size(px(440.), px(320.)),
                        ))),
                        show: false,
                        focus: false,
                        ..Default::default()
                    },
                    |_, cx| cx.new(TerminalSurface::new),
                )
                .expect("open hidden terminal scrollbar probe");
            verify(window, cx).expect("verify terminal scrollbar handlers");
            window
                .update(cx, |view, _, _| {
                    view.surface_hovered = true;
                    view.scrollport_hovered = false;
                    // Start after the reveal animation, so the draw can only
                    // schedule the hide deadline, not an animation frame.
                    view.scrollbar_opacity = 1.;
                    view.scrollbar_fade = None;
                    view.reveal_scrollbar();
                })
                .expect("prepare a visible scrollbar");
            draw(window, cx);
            window
                .update(cx, |view, _, _| {
                    assert!(view.scrollbar_revealed);
                    assert!(view.scrollbar_hide_at.is_some());
                    assert!(view.scrollbar_hide_timer.is_some());
                    assert!(!view.scrollbar_frame_pending);
                })
                .expect("paint scheduled one hide deadline without a fade frame");

            let verification = cx.background_executor().timer(Duration::from_millis(650));
            cx.spawn(async move |cx| {
                verification.await;
                cx.update(|cx| {
                    window
                        .update(cx, |view, _, _| {
                            assert!(!view.scrollbar_revealed);
                            assert!(view.scrollbar_hide_at.is_none());
                            assert!(view.scrollbar_hide_timer.is_none());
                        })
                        .expect("the deadline hides the bar without another manual draw");
                    println!("The one-shot hide deadline expires without a fade frame loop or another manual draw.");
                })
                .expect("finish the async scrollbar probe");
                verify_file_drop_async(window, cx).expect("verify macOS NSDraggingDestination file drop");
                cx.update(|cx| cx.quit()).expect("quit probe");
            })
            .detach();
        });
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
}
