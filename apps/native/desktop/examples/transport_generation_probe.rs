//! Hidden production-handler probe for connection generations and recovery qualification.

#[cfg(target_os = "macos")]
pub use matou_native_desktop::{assets, macos, rename_input, tab_dialog_style, workspace_drag};

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
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    use crate::runtime_bridge::ProbeReceiver;
    use matou_native_terminal::{Terminal, TerminalOptions};

    impl Workspace {
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
                show_summary: true,
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
#[cfg(target_os = "macos")]
fn main() {
    use gpui::{
        AnyWindowHandle, AppContext as _, Application, Bounds, WindowBounds, WindowOptions, point,
        px, size,
    };
    use matou_native_runtime::host::{Event, LaunchOptions, RuntimeHost};
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
