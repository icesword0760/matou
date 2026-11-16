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
        AnyWindowHandle, AppContext as _, Application, MouseDownEvent, MouseMoveEvent,
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
        match event {
            SurfaceEvent::ScrollTo { row, request_id } => Observed::Absolute(*row, *request_id),
            SurfaceEvent::Scroll(lines) => Observed::Lines(*lines),
            SurfaceEvent::Input(bytes) => Observed::Input(bytes.clone()),
            SurfaceEvent::Select(_) => Observed::Select,
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
        let position = view.scrollbar_position.expect("optimistic scroll position");
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
                assert!(view.scrollbar_move(&event, cx));
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
            view.on_scroll(
                &ScrollWheelEvent {
                    position: click,
                    delta: ScrollDelta::Lines(point(0., 2.)),
                    ..Default::default()
                },
                cx,
            );
            assert!(view.scrollbar_position.is_none());
        })?;
        assert_eq!(
            events.take(),
            [Observed::Absolute(latest_row, 5), Observed::Lines(2)]
        );
        println!("Ordinary wheel scroll drops the optimistic thumb and queues its own delta.");

        window.update(cx, |view, _, cx| {
            view.surface_hovered = false;
            view.set_screen(authority, modes, None, 5, cx);
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
        Ok(())
    }

    pub fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
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
                    cx.quit();
                })
                .expect("finish the async scrollbar probe");
            })
            .detach();
        });
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
}
