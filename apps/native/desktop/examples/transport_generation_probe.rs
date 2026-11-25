//! Hidden production-handler probe for connection generations and recovery qualification.

#[cfg(target_os = "macos")]
pub use matou_native_desktop::{
    assets, recovery_water, recovery_water_macos, rename_input, tab_dialog_style, workspace_drag,
};

#[cfg(target_os = "macos")]
mod macos {
    pub use matou_native_desktop::macos::*;
    thread_local! { pub(super) static PROBE_VISIBLE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) }; }

    pub(crate) fn is_window_visible(window: &gpui::Window) -> anyhow::Result<bool> {
        match PROBE_VISIBLE.get() {
            Some(visible) => Ok(visible),
            None => matou_native_desktop::macos::is_window_visible(window),
        }
    }
}
#[cfg(target_os = "macos")]
#[path = "../src/recovery_water_view.rs"]
mod recovery_water_view;

#[cfg(target_os = "macos")]
#[path = "../src/terminal_input.rs"]
mod terminal_input;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_scrollbar.rs"]
mod terminal_scrollbar;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_surface.rs"]
mod terminal_surface;

#[cfg(target_os = "macos")]
#[path = "support/runtime_probe.rs"]
mod runtime_bridge;

#[cfg(target_os = "macos")]
#[path = "../src/carousel_motion.rs"]
mod carousel_motion;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod ui_text {
    include!("../src/ui_text.rs");
}

#[cfg(target_os = "macos")]
mod sidebar_scrollbar {
    include!("../src/sidebar_scrollbar.rs");
}

#[cfg(target_os = "macos")]
mod history_view {
    include!("../src/history_view.rs");

    impl HistoryView {
        pub(super) fn probe_state(&self) -> Option<&HistoryViewState> {
            self.state.as_ref()
        }
    }
}

#[cfg(target_os = "macos")]
#[path = "../src/provider_settings.rs"]
mod provider_settings;
#[cfg(target_os = "macos")]
#[path = "../src/settings_shell.rs"]
mod settings_shell;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    use crate::runtime_bridge::ProbeReceiver;
    use matou_native_terminal::{Terminal, TerminalOptions};

    impl Workspace {
        #[cfg(debug_assertions)]
        pub(super) fn probe_water_id(&self) -> Option<EntityId> {
            assert!(self.recovery_water.len() <= 1);
            assert_eq!(
                self.recovery_water_visibility.is_some(),
                !self.recovery_water.is_empty()
            );
            self.recovery_water.values().next().map(Entity::entity_id)
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_water_card(&self) -> Bounds<Pixels> {
            self.carousel_scroll.bounds_for_item(0).unwrap()
        }

        #[cfg(debug_assertions)]
        pub(super) fn arm_water_click(&mut self, window: &mut Window) {
            let scene_id = self
                .hierarchy
                .as_ref()
                .unwrap()
                .scene
                .as_ref()
                .unwrap()
                .id
                .clone();
            self.geometry
                .remove(&format!("session-group:{scene_id}:root"));
            self.pending_focus = Some((scene_id, "probe-previous-session".into()));
            window.blur();
        }

        #[cfg(debug_assertions)]
        pub(super) fn water_click_state(
            &self,
            window: &Window,
            cx: &Context<Self>,
        ) -> (bool, bool, bool) {
            let id = &self.order[0];
            (
                self.geometry_focus() == Some(id.as_str()),
                !self.geometry.is_empty(),
                self.terminals[id].read(cx).focus.is_focused(window),
            )
        }

        #[cfg(debug_assertions)]
        pub(super) fn focus_water_terminal(&self, window: &mut Window, cx: &Context<Self>) {
            window.focus(&self.terminals[&self.order[0]].read(cx).focus);
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_water_generation(
            &mut self,
            fixture: &Value,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: self.runtime_generation + 1,
                    event: serde_json::from_value(fixture.clone()).unwrap(),
                },
                window,
                cx,
            );
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_water_frame(
            &mut self,
            content: &[u8],
            sequence: u64,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let id = self.order[0].clone();
            let (cols, rows, w, h) = self.terminals[&id].read(cx).target_dimensions().unwrap();
            let mut terminal = Terminal::new(TerminalOptions::new(cols, rows, w, h)).unwrap();
            terminal.feed(content);
            self.apply(
                BridgeEvent::Runtime {
                    generation: self.runtime_generation,
                    event: Event::Frame {
                        session_id: id,
                        screen: terminal.screen().unwrap(),
                        modes: terminal.input_modes().unwrap(),
                        selection: None,
                        sequence,
                        scroll_request_id: 0,
                    },
                },
                window,
                cx,
            );
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_water_recovery(
            &mut self,
            state: RecoveryState,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: self.runtime_generation,
                    event: Event::Recovery {
                        session_id: self.order[0].clone(),
                        state,
                        history_gap: None,
                    },
                },
                window,
                cx,
            );
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_water_directory(
            &mut self,
            valid: bool,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let workspace_id = self
                .hierarchy
                .as_ref()
                .unwrap()
                .workspace
                .as_ref()
                .unwrap()
                .id
                .clone();
            let paths = if valid {
                vec![]
            } else {
                vec![WorkspacePathState {
                    workspace_id,
                    status: "invalid".into(),
                    reason: "probe".into(),
                    checked_at: 1,
                    validation_generation: 1,
                }]
            };
            self.apply(
                BridgeEvent::Runtime {
                    generation: self.runtime_generation,
                    event: Event::WorkspacePaths(paths),
                },
                window,
                cx,
            );
        }

        pub(super) fn prepare(
            &mut self,
            fixture: &serde_json::Value,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: serde_json::from_value(fixture.clone()).unwrap(),
                },
                window,
                cx,
            );
        }
        pub(super) fn verify(
            &mut self,
            fixture: &serde_json::Value,
            sink: &ProbeReceiver,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let id = self.order[0].clone();
            let surface = self.terminals[&id].clone();
            let target = surface
                .read(cx)
                .target_dimensions()
                .expect("actual hidden GPUI paint target");
            let old_id = surface.entity_id();
            // A retained tab can have measured dimensions without belonging to the
            // current graph. A reconnect must not wait for another resize gesture.
            while sink.try_recv().is_ok() {}
            let current_order = std::mem::take(&mut self.order);
            self.resend_terminal_sizes(cx);
            assert!(
                matches!(sink.try_recv(), Ok(Command::Resize { session_id, cols, rows, .. })
                if session_id == id && (cols, rows) == (target.0, target.1)),
                "retained noncurrent surface target was not resent"
            );
            self.order = current_order;
            while sink.try_recv().is_ok() {}
            self.open_search(window, cx);
            self.search.as_mut().unwrap().query = "retained query".into();
            let input = self.search.as_ref().unwrap().input.clone();
            input.update(cx, |input, _| input.text = "retained query".into());
            let cursor = HistoryCursor {
                sequence: 17,
                line_index: 0,
            };
            let search = self.search.as_mut().unwrap();
            search.session_id = Some(id.clone());
            search.archive = Some(ArchivedMatches {
                query: search.query.clone(),
                options: search.options,
                sequence: search.sequence,
                index: 0,
                matches: vec![HistoryLine {
                    sequence: 17,
                    cursor,
                    text: "old archive".into(),
                }],
                gap_count: 0,
                has_more: false,
            });
            let old_search_sequence = search.sequence;
            self.open_history_context(window, cx);
            let old_history = self.history_view.as_ref().unwrap().clone();
            assert!(old_history.read(cx).probe_state().is_some());
            self.pending_focus = Some(("old-scene".into(), id.clone()));
            self.pending_close = Some("old-scene".into());
            self.creating_workspace = true;
            self.save_geometry(123., None, None, false, cx);
            let current_hierarchy = self.hierarchy.clone();
            self.hierarchy.as_mut().unwrap().scene.as_mut().unwrap().id =
                "probe-background-scene".into();
            self.save_geometry(456., None, None, false, cx);
            self.hierarchy = current_hierarchy;
            let background_owner = "session-group:probe-background-scene:root";
            self.handle_surface_event(
                &id,
                &SurfaceEvent {
                    generation: 1,
                    kind: SurfaceEventKind::Resize {
                        cols: target.0 + 5,
                        rows: target.1,
                        cell_width_px: target.2,
                        cell_height_px: target.3,
                    },
                },
                cx,
            );
            while sink.try_recv().is_ok() {}
            cx.write_to_clipboard(ClipboardItem::new_string(
                "TRANSPORT_CLIPBOARD_SENTINEL".into(),
            ));
            self.apply(
                BridgeEvent::TransportDiagnostic {
                    generation: 1,
                    message: "old transport".into(),
                },
                window,
                cx,
            );
            self.fault = Some("real domain/storage fault".into());
            // Intentionally omit Down: the first current-generation Runtime event must retire g1.
            sink.generation(2);
            let mut next = fixture.clone();
            next["Ready"]["graph"]["layoutRevision"] = 200.into();
            self.apply(
                BridgeEvent::Runtime {
                    generation: 2,
                    event: serde_json::from_value(next).unwrap(),
                },
                window,
                cx,
            );
            assert_eq!(
                self.terminals[&id].entity_id(),
                old_id,
                "existing card/surface was rebuilt"
            );
            assert_eq!(
                self.terminals[&id].read(cx).target_dimensions(),
                Some(target)
            );
            assert_eq!(self.fault.as_deref(), Some("real domain/storage fault"));
            assert!(self.transport_fault.is_none());
            assert!(self.history_view.is_none() && self.history_request.is_none());
            assert!(
                old_history.read(cx).probe_state().is_none(),
                "retired archive retained state"
            );
            assert!(self.pending_focus.is_none() && self.pending_close.is_none());
            assert!(!self.creating_workspace);
            assert_eq!(self.search.as_ref().unwrap().query, "retained query");
            assert!(
                input.read(cx).focus.is_focused(window),
                "search focus was stolen"
            );
            let mut resized = false;
            let mut geometry = false;
            while let Ok(command) = sink.try_recv() {
                match command {
                    Command::Resize {
                        session_id,
                        cols,
                        rows,
                        ..
                    } => {
                        assert_eq!(session_id, id);
                        assert_eq!((cols, rows), (target.0, target.1));
                        resized = true;
                    }
                    Command::PutGeometry { sequence, update } => {
                        assert_ne!(
                            update.owner_key, background_owner,
                            "background desired was written without its authority"
                        );
                        assert_eq!(update.layout_revision, 200.);
                        assert!(sequence > 0);
                        assert_eq!(update.geometry["scrollLeft"], 123.);
                        geometry = true;
                    }
                    Command::Search { .. } => panic!("search before usable first frame"),
                    _ => {}
                }
            }
            assert!(
                resized && geometry,
                "same-size resend/new-revision dirty geometry missing"
            );
            assert!(self.reconnect_geometry.contains(background_owner));
            assert_eq!(self.geometry[background_owner].geometry["scrollLeft"], 456.);
            for kind in [
                SurfaceEventKind::Resize {
                    cols: target.0 + 5,
                    rows: target.1,
                    cell_width_px: target.2,
                    cell_height_px: target.3,
                },
                SurfaceEventKind::Scroll(99),
                SurfaceEventKind::Copy {
                    interrupt_if_empty: false,
                },
            ] {
                self.handle_surface_event(
                    &id,
                    &SurfaceEvent {
                        generation: 1,
                        kind,
                    },
                    cx,
                );
            }
            self.finish_directory_choice(1, "/must-not-create".into(), None, cx);
            self.finish_directory_choice(
                1,
                "/must-not-relink".into(),
                Some("old-workspace".into()),
                cx,
            );
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::History(HistoryReply::Context {
                        session_id: id.clone(),
                        sequence: old_search_sequence,
                        cursor,
                        result: Ok(matou_native_runtime::history::HistoryPage::default()),
                    }),
                },
                window,
                cx,
            );
            assert!(
                self.history_view.is_none(),
                "retired archive reply reopened the overlay"
            );
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Copied {
                        request_id: self.copy_request_id,
                        text: "STALE_COPY".into(),
                    },
                },
                window,
                cx,
            );
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::ClipboardWrite {
                        session_id: id.clone(),
                        text: "STALE_OSC52".into(),
                    },
                },
                window,
                cx,
            );
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("TRANSPORT_CLIPBOARD_SENTINEL")
            );
            assert!(
                sink.try_recv().is_err(),
                "retired producer emitted a command"
            );
            self.apply(
                BridgeEvent::Runtime {
                    generation: 2,
                    event: Event::Recovery {
                        session_id: id.clone(),
                        state: RecoveryState::Ready,
                        history_gap: None,
                    },
                },
                window,
                cx,
            );
            assert!(
                self.recovery_blocked(&id),
                "RecoveryReady alone enabled input"
            );
            let mut wrong = Terminal::new(TerminalOptions::new(
                target.0 + 1,
                target.1,
                target.2,
                target.3,
            ))
            .unwrap();
            let modes = wrong.input_modes().unwrap();
            self.apply(
                BridgeEvent::Runtime {
                    generation: 2,
                    event: Event::Frame {
                        session_id: id.clone(),
                        screen: wrong.screen().unwrap(),
                        modes,
                        selection: None,
                        sequence: 1,
                        scroll_request_id: 0,
                    },
                },
                window,
                cx,
            );
            assert!(
                self.recovery_blocked(&id),
                "wrong-sized first frame enabled input"
            );
            let mut correct =
                Terminal::new(TerminalOptions::new(target.0, target.1, target.2, target.3))
                    .unwrap();
            for sequence in [2, 3] {
                self.apply(
                    BridgeEvent::Runtime {
                        generation: 2,
                        event: Event::Frame {
                            session_id: id.clone(),
                            screen: correct.screen().unwrap(),
                            modes,
                            selection: None,
                            sequence,
                            scroll_request_id: 0,
                        },
                    },
                    window,
                    cx,
                );
            }
            assert!(
                !self.recovery_blocked(&id),
                "matching current frame + RecoveryReady did not enable"
            );
            let mut searches = 0;
            while let Ok(command) = sink.try_recv() {
                if let Command::Search { request, .. } = command {
                    assert_eq!(request.query, "retained query");
                    searches += 1;
                }
            }
            assert_eq!(searches, 1, "retained query was not reissued exactly once");
            // Ordinary resize remains usable; this must not re-enter recovery.
            self.handle_surface_event(
                &id,
                &SurfaceEvent {
                    generation: 2,
                    kind: SurfaceEventKind::Resize {
                        cols: target.0,
                        rows: target.1,
                        cell_width_px: target.2,
                        cell_height_px: target.3,
                    },
                },
                cx,
            );
            assert!(
                !self.recovery_blocked(&id),
                "ordinary resize re-entered recovery"
            );
            self.handle_surface_event(
                &id,
                &SurfaceEvent {
                    generation: 2,
                    kind: SurfaceEventKind::Input(b"ordinary-resize-input".to_vec()),
                },
                cx,
            );
            assert!(
                matches!(sink.try_recv(), Ok(Command::Input { bytes, .. })
                if bytes == b"ordinary-resize-input"),
                "ordinary resize blocked input"
            );

            // The reverse order also requires both pieces: first frame, then RecoveryReady.
            self.apply(
                BridgeEvent::Runtime {
                    generation: 2,
                    event: Event::Recovery {
                        session_id: id.clone(),
                        state: RecoveryState::Restoring,
                        history_gap: None,
                    },
                },
                window,
                cx,
            );
            self.apply(
                BridgeEvent::Runtime {
                    generation: 2,
                    event: Event::Frame {
                        session_id: id.clone(),
                        screen: correct.screen().unwrap(),
                        modes,
                        selection: None,
                        sequence: 4,
                        scroll_request_id: 0,
                    },
                },
                window,
                cx,
            );
            assert!(
                self.recovery_blocked(&id),
                "frame without RecoveryReady enabled input"
            );
            self.apply(
                BridgeEvent::Runtime {
                    generation: 2,
                    event: Event::Recovery {
                        session_id: id.clone(),
                        state: RecoveryState::Ready,
                        history_gap: None,
                    },
                },
                window,
                cx,
            );
            assert!(
                !self.recovery_blocked(&id),
                "frame-before-ready order stayed blocked"
            );
            while sink.try_recv().is_ok() {}

            let mut background = fixture["Ready"].clone();
            background["hierarchy"]["scene"]["id"] = "probe-background-scene".into();
            background["graph"]["sceneId"] = "probe-background-scene".into();
            background["graph"]["layoutRevision"] = 300.into();
            background["geometry"] = serde_json::json!([]);
            self.apply(
                BridgeEvent::Runtime {
                    generation: 2,
                    event: serde_json::from_value(serde_json::json!({"Navigation": background}))
                        .unwrap(),
                },
                window,
                cx,
            );
            let mut background_write = false;
            while let Ok(command) = sink.try_recv() {
                if let Command::PutGeometry { update, .. } = command {
                    assert_eq!(update.owner_key, background_owner);
                    assert_eq!(update.layout_revision, 300.);
                    assert_eq!(update.geometry["scrollLeft"], 456.);
                    background_write = true;
                }
            }
            assert!(
                background_write,
                "background desired not reconciled on its Navigation"
            );
            assert!(!self.reconnect_geometry.contains(background_owner));
            println!(
                "Ordinary resize remains usable; frame-before-ready gate passes; background dirty desired retained until Navigation, then written at revision300."
            );
            println!(
                "Real hidden paint: target={target:?}; retained entity, same-size g2 resize, fresh geometry revision, stale Resize/Scroll/Copy/replies/picker rejected; clipboard sentinel unchanged; recovery+matching frame gate and exactly one retained search pass."
            );
        }
        pub(super) fn after_timers(&self, sink: &ProbeReceiver, cx: &Context<Self>) {
            while let Ok(command) = sink.try_recv() {
                match command {
                    Command::PutGeometry { update, .. } => assert!(
                        [200., 300.].contains(&update.layout_revision),
                        "old geometry timer/retry crossed transport"
                    ),
                    Command::Resize { cols, .. } => assert_ne!(
                        cols,
                        self.terminals[&self.order[0]]
                            .read(cx)
                            .target_dimensions()
                            .unwrap()
                            .0
                            + 5
                    ),
                    Command::CreateWorkspace { .. } | Command::RelinkWorkspace { .. } => {
                        panic!("old picker crossed transport")
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(all(target_os = "macos", debug_assertions))]
#[allow(unexpected_cfgs)]
fn click_hidden_card(
    window: gpui::WindowHandle<workspace::Workspace>,
    cx: &mut gpui::AsyncApp,
    inset: f32,
) -> (bool, bool, bool) {
    use cocoa::{
        appkit::{NSEvent, NSEventModifierFlags, NSEventType},
        base::{id, nil},
        foundation::{NSInteger, NSPoint},
    };
    use gpui::px;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let (view, location, number) = window
        .update(cx, |workspace, window, _| {
            workspace.arm_water_click(window);
            let card = workspace.probe_water_card();
            let RawWindowHandle::AppKit(handle) =
                HasWindowHandle::window_handle(window).unwrap().as_raw()
            else {
                unreachable!();
            };
            let view = handle.ns_view.as_ptr() as id;
            let number: NSInteger = unsafe {
                let native: id = msg_send![view, window];
                msg_send![native, windowNumber]
            };
            let location = NSPoint::new(
                f64::from(f32::from(card.center().x)),
                f64::from(f32::from(
                    window.viewport_size().height - card.top() - px(inset),
                )),
            );
            (view, location, number)
        })
        .unwrap();
    // Dispatch to this hidden probe view after returning the Window to GPUI.
    // No desktop event is posted and no other application can receive the click.
    unsafe {
        let event = |kind| {
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
            nil, kind, location, NSEventModifierFlags::empty(), 0., number, nil, 0, 1, 1.,
        )
        };
        let _: () = msg_send![view, mouseDown: event(NSEventType::NSLeftMouseDown)];
        let _: () = msg_send![view, mouseUp: event(NSEventType::NSLeftMouseUp)];
    }
    window
        .update(cx, |view, window, cx| view.water_click_state(window, cx))
        .unwrap()
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn run_water_probe(cx: &mut gpui::App, fixture: serde_json::Value) {
    use crate::runtime_bridge::{Command, RecoveryState};
    use gpui::{
        AnyWindowHandle, AppContext as _, Bounds, Keystroke, WindowBounds, WindowOptions, point,
        px, size,
    };
    use matou_native_terminal::Theme;
    use serde_json::json;
    use std::{collections::HashSet, time::Duration};

    let (commands, sink) = runtime_bridge::probe_channel();
    let (_events, incoming) = async_channel::unbounded();
    let window = cx
        .open_window(
            WindowOptions {
                show: false,
                focus: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1447.), px(739.)),
                ))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| workspace::Workspace::new(commands, incoming, window, cx)),
        )
        .unwrap();
    let draw = move |cx: &mut gpui::App| {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
    };
    let press = move |cx: &mut gpui::App, key| {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
            })
            .unwrap();
    };
    window
        .update(cx, |view, window, cx| view.prepare(&fixture, window, cx))
        .unwrap();
    draw(cx);
    assert!(
        window
            .update(cx, |view, _, _| view.probe_water_id())
            .unwrap()
            .is_none()
    );
    // Keep the AppKit window ordered out while exercising real layout and
    // painting. Only the visibility result is supplied by the probe.
    macos::PROBE_VISIBLE.set(Some(true));
    draw(cx);
    let id = window
        .update(cx, |view, _, _| view.probe_water_id())
        .unwrap()
        .unwrap();
    let id = id.as_u64();
    let clock = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        // Let GPUI upload accepted images before changing the generation.
        for _ in 0..12 {
            clock.timer(Duration::from_millis(15)).await;
            cx.update(draw).unwrap();
        }
        cx.update(|cx| {
            sink.generation(2);
            window.update(cx, |view, window, cx| {
                view.probe_water_generation(&fixture, window, cx);
                assert_eq!(view.probe_water_id().unwrap().as_u64(), id);
            }).unwrap();
            draw(cx);
        }).unwrap();
        for _ in 0..12 {
            clock.timer(Duration::from_millis(15)).await;
            cx.update(draw).unwrap();
        }
        while sink.try_recv().is_ok() {}
        assert!(window.update(cx, |view, _, _| view.probe_water_id()).unwrap().is_some());
        let (body_activated, body_geometry, body_terminal_focus) = click_hidden_card(window, cx, 64.);
        assert!(body_activated && body_geometry, "water body click did not activate the card and capture geometry");
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|cmd| matches!(cmd, Command::FocusSession { .. })));
        assert_eq!(click_hidden_card(window, cx, 16.), (true, true, false), "header click did not activate card");
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|cmd| matches!(cmd, Command::FocusSession { .. })));
        cx.update(|cx| {
            window.update(cx, |view, window, cx| view.focus_water_terminal(window, cx)).unwrap();
            press(cx, "u");
        }).unwrap();
        assert!(!std::iter::from_fn(|| sink.try_recv().ok()).any(|cmd| matches!(cmd, Command::Input { .. })));
        cx.update(|cx| {
            window.update(cx, |view, window, cx| {
                view.probe_water_recovery(RecoveryState::Ready, window, cx);
                view.probe_water_frame(b"   ", 1, window, cx);
                assert!(view.probe_water_id().is_some());
            }).unwrap();
            draw(cx);
            press(cx, "u");
        }).unwrap();
        assert!(!std::iter::from_fn(|| sink.try_recv().ok()).any(|cmd| matches!(cmd, Command::Input { .. })));
        cx.update(|cx| {
            window.update(cx, |view, window, cx| {
                view.probe_water_frame(b"RW first real output", 2, window, cx);
                assert!(view.probe_water_id().is_none());
            }).unwrap();
            draw(cx);
        }).unwrap();
        assert_eq!(click_hidden_card(window, cx, 64.), (true, true, true), "ready terminal click did not focus");
        cx.update(|cx| press(cx, "v")).unwrap();
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|cmd| matches!(cmd, Command::Input { bytes, .. } if bytes == b"v")));
        let hidden_id = cx.update(|cx| {
            window.update(cx, |view, window, cx| {
                view.probe_water_frame(b"", 3, window, cx);
                assert!(view.probe_water_id().is_none());
                view.probe_water_recovery(RecoveryState::Ready, window, cx);
                assert!(view.probe_water_id().is_none());
                view.probe_water_recovery(RecoveryState::Restoring, window, cx);
            }).unwrap();
            draw(cx);
            window.update(cx, |view, window, cx| {
                assert!(view.probe_water_id().is_some());
                view.probe_water_recovery(RecoveryState::Failed("probe".into()), window, cx);
                assert!(view.probe_water_id().is_none());
                view.probe_water_directory(false, window, cx);
                view.probe_water_recovery(RecoveryState::Restoring, window, cx);
            }).unwrap();
            draw(cx);
            window.update(cx, |view, window, cx| {
                assert!(view.probe_water_id().is_none());
                view.probe_water_directory(true, window, cx);
            }).unwrap();
            draw(cx);
            let id = window.update(cx, |view, _, _| view.probe_water_id()).unwrap().unwrap();
            macos::PROBE_VISIBLE.set(Some(false));
            id.as_u64()
        }).unwrap();
        // No subsequent draw is allowed to perform the hidden-window removal.
        clock.timer(Duration::from_millis(240)).await;
        cx.update(|cx| {
            assert!(window.update(cx, |view, _, _| view.probe_water_id()).unwrap().is_none());
            let trace = recovery_water_view::debug_diagnostics();
            assert_eq!(trace["dropped"], 0);
            let all_rows = trace["rows"].as_array().unwrap();
            assert!(all_rows.iter().any(|row| row["id"] == hidden_id && row["event"] == "release"));
            let rows: Vec<_> = trace["rows"].as_array().unwrap().iter()
                .filter(|row| row["id"] == id).collect();
            let event = |name| rows.iter().copied().find(|row| row["event"] == name).unwrap();
            let layout = &event("size")["data"];
            assert_eq!(layout["width"], 560.);
            assert_eq!(layout["height"], 600.);
            assert_eq!(layout["dpr"], 2.);
            assert_eq!(layout["background"], "fbfcff");
            assert_eq!(layout["bottom_radius"], 12.);
            let card = window.update(cx, |view, _, _| view.probe_water_card()).unwrap();
            let (left, top) = (f64::from(f32::from(card.left())), f64::from(f32::from(card.top())));
            assert_eq!(layout["y"].as_f64().unwrap() - top, 32.);
            assert_eq!(layout["x"].as_f64().unwrap(), left);
            let created: Vec<_> = rows.iter().filter(|row| row["event"] == "image_create")
                .map(|row| row["data"]["image"].as_str().unwrap()).collect();
            let dropped: Vec<_> = rows.iter().filter(|row| row["event"] == "image_drop")
                .map(|row| row["data"]["image"].as_str().unwrap()).collect();
            assert_eq!(created.len(), dropped.len());
            assert_eq!(created.iter().copied().collect::<HashSet<_>>(), dropped.iter().copied().collect());
            let mut inflight = false;
            let mut generation = 1;
            let mut painted = HashSet::new();
            let mut requested = std::collections::HashMap::new();
            let mut last_painted = None;
            for row in &rows {
                let data = &row["data"];
                let sequence = data["sequence"].as_u64();
                match row["event"].as_str().unwrap() {
                    "revision" => generation = data["generation"].as_u64().unwrap(),
                    "request" => {
                        assert!(!inflight);
                        inflight = true;
                        requested.insert(sequence.unwrap(), generation);
                    }
                    "finish" => {
                        assert!(inflight);
                        inflight = false;
                    }
                    "accept" => assert_eq!(requested[&sequence.unwrap()], generation, "stale animation job accepted"),
                    "paint" => {
                        let sequence = sequence.unwrap();
                        let complete = data["images"].as_array().unwrap().iter().all(|image| !image.is_null());
                        if let Some(last) = last_painted {
                            assert!(complete && sequence >= last, "water flattened or restarted during reconnect");
                        }
                        if complete {
                            painted.insert(requested[&sequence]);
                            last_painted = Some(sequence);
                        }
                    }
                    _ => {}
                }
            }
            assert!(!inflight);
            assert_eq!(painted, HashSet::from([1, 2]));
            assert!(rows.iter().any(|row| row["data"]["reason"] == "unmount"));
            event("release");

            let pixel = |bitmap: &recovery_water_macos::Bitmap, x, y| {
                let index = ((y * bitmap.width + x) * 4) as usize;
                <[u8; 4]>::try_from(&bitmap.premult_rgba[index..index + 4]).unwrap()
            };
            let light = recovery_water_macos::draw_background(560., 600., 2., Theme::LIGHT);
            let dark = recovery_water_macos::draw_background(560., 600., 2., Theme::DARK);
            let edges = recovery_water_macos::draw_edges(560., 600., 2.);
            assert_eq!(pixel(&light, 0, 0), [251, 252, 255, 255]);
            assert_eq!(pixel(&dark, 0, 0), [27, 27, 27, 255]);
            assert_ne!(pixel(&light, 560, 420), pixel(&light, 0, 0));
            assert_ne!(pixel(&dark, 560, 420), pixel(&dark, 0, 0));
            assert!(pixel(&edges, 0, 420)[3] > pixel(&edges, 2, 420)[3] + 8);
            assert!(pixel(&edges, 560, 1180)[3] > pixel(&edges, 560, 420)[3]);
            println!("{}", json!({
                "layout":layout, "card_top":top, "top_inset":32, "window_ordered_out":true,
                "background": {"center":[0.5,0.35], "transparent_at":0.48,
                    "light":[86,153,219,0.055], "dark":[71,147,222,0.05],
                    "light_center":pixel(&light,560,420), "dark_center":pixel(&dark,560,420)},
                "edges": {"inset":[1,181,216,255,0.055], "shadow":[-28,48,19,92,159,0.055],
                    "corner_alpha":pixel(&edges,0,0)[3], "bottom_alpha":pixel(&edges,560,1180)[3]},
                "same_entity_generations": [1,2], "ready_removed":true, "hidden_removed_without_draw":true,
                "failed_removed":true, "invalid_path_suppressed":true,
                "mouse": {"body_at_64_activates":body_activated, "body_geometry_saved":body_geometry,
                    "body_terminal_focused":body_terminal_focus, "title_at_16_activates":true,
                    "ready_body_activates_and_focuses":true},
                "keyboard": {"during_recovery_blocked":true, "after_ready_forwarded":true},
                "handoff": {"ready_and_blank_frames_keep_water":true,"first_content_removes_water":true,"later_blank_does_not_remount":true},
                "images_created":created.len(), "images_dropped":dropped.len(), "inflight":inflight,
                "trace":trace
            }));
            cx.quit();
        }).unwrap();
    }).detach();
}

#[cfg(target_os = "macos")]
fn main() {
    use gpui::{
        AnyWindowHandle, AppContext as _, Application, Bounds, WindowBounds, WindowOptions, point,
        px, size,
    };
    use matou_native_runtime::host::{Event, LaunchOptions, RuntimeHost};

    #[cfg(debug_assertions)]
    let recovery_water = std::env::args().any(|arg| arg == "--recovery-water");
    #[cfg(debug_assertions)]
    if recovery_water {
        // SAFETY: Set the flag before starting RuntimeHost or GPUI threads.
        unsafe { std::env::set_var("MATOU_RECOVERY_WATER_TRACE", "1") };
    }

    let root = std::env::temp_dir().join(format!(
        "matou-transport-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut host = RuntimeHost::start(LaunchOptions {
        data_root: root.join("data"),
        workspace: root.join("workspace"),
        shell_home: root.join("home"),
        shell: "/bin/zsh".into(),
    })
    .unwrap();
    let fixture = loop {
        let event = host.events.recv_blocking().unwrap();
        if matches!(event, Event::Ready { .. }) {
            break serde_json::to_value(event).unwrap();
        }
    };
    host.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    Application::new()
        .with_assets(assets::NativeAssets)
        .run(move |cx| {
            #[cfg(debug_assertions)]
            if recovery_water {
                run_water_probe(cx, fixture);
                return;
            }
            let (commands, sink) = runtime_bridge::probe_channel();
            let (_events, incoming) = async_channel::unbounded();
            let window = cx
                .open_window(
                    WindowOptions {
                        show: false,
                        focus: false,
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(1200.), px(780.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        cx.new(|cx| workspace::Workspace::new(commands, incoming, window, cx))
                    },
                )
                .unwrap();
            window
                .update(cx, |view, window, cx| view.prepare(&fixture, window, cx))
                .unwrap();
            AnyWindowHandle::from(window)
                .update(cx, |_, window, cx| {
                    window.refresh();
                    window.draw(cx).clear();
                })
                .unwrap();
            window
                .update(cx, |view, window, cx| {
                    view.verify(&fixture, &sink, window, cx)
                })
                .unwrap();
            let delay = cx
                .background_executor()
                .timer(std::time::Duration::from_millis(240));
            cx.spawn(async move |cx| {
                delay.await;
                cx.update(|cx| {
                    window
                        .update(cx, |view, _, cx| view.after_timers(&sink, cx))
                        .unwrap();
                    println!(
                        "Old resize/geometry callbacks remained retired after their deadlines."
                    );
                    cx.quit();
                })
                .unwrap();
            })
            .detach();
        });
}
#[cfg(not(target_os = "macos"))]
fn main() {}
