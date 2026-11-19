//! Hidden GPUI layout and input probe backed by a disposable session graph.

#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub use matou_native_desktop::{
    assets, macos, recovery_water_view, rename_input, tab_dialog_style, workspace_drag,
};

#[cfg(target_os = "macos")]
#[path = "../src/carousel_motion.rs"]
mod carousel_motion;
#[cfg(target_os = "macos")]
#[path = "../src/history_view.rs"]
mod history_view;
#[cfg(target_os = "macos")]
#[path = "support/runtime_probe.rs"]
mod runtime_bridge;
#[cfg(target_os = "macos")]
#[path = "../src/sidebar_scrollbar.rs"]
mod sidebar_scrollbar;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_input.rs"]
mod terminal_input;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_scrollbar.rs"]
mod terminal_scrollbar;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_surface.rs"]
#[allow(dead_code)]
mod terminal_surface;
#[cfg(target_os = "macos")]
#[path = "../src/ui_text.rs"]
mod ui_text;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    use super::probe::{PARENT, SCENE};
    use matou_native_terminal::{Terminal, TerminalOptions};

    impl Workspace {
        pub(super) fn deliver(
            &mut self,
            event: Event,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: self.commands.connection().0,
                    event,
                },
                window,
                cx,
            );
        }

        pub(super) fn assert_level(
            &self,
            parent: Option<&str>,
            cards: &[&str],
            label: &str,
            count: &str,
            focused: &str,
        ) {
            assert_eq!(self.level_parent(), parent);
            assert_eq!(self.order, cards);
            for index in 0..cards.len() {
                let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
                assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
            }
            assert_eq!(self.card_geometry.len(), cards.len());
            if !cards.is_empty() {
                assert!(self.carousel_scroll.bounds_for_item(cards.len()).is_none());
            }
            let (actual_label, actual_count) = self.breadcrumb_labels();
            assert_eq!(
                (actual_label.as_str(), actual_count.as_str()),
                (label, count)
            );
            assert_eq!(self.focused.as_deref(), Some(focused));
            println!(
                "{label} {count} | cards={cards:?} | focus={focused} | scene={}",
                self.active_scene_id().unwrap()
            );
        }

        pub(super) fn badge_position(&self) -> Point<Pixels> {
            let index = self.order.iter().position(|id| id == PARENT).unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            point(
                bounds.right() + self.carousel_scroll.offset().x - px(50.),
                bounds.top() + px(20.),
            )
        }

        pub(super) fn compact_parent_position(
            &self,
            window: &Window,
            inset: Pixels,
        ) -> Point<Pixels> {
            assert!(window.viewport_size().width - px(260.) < px(720.));
            assert_eq!(self.level_focus(), Some(super::probe::PEER));
            let index = self.order.iter().position(|id| id == PARENT).unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            assert!(bounds.size.width <= px(176.));
            point(
                bounds.center().x + self.carousel_scroll.offset().x,
                bounds.top() + inset,
            )
        }

        pub(super) fn assert_root_badge(&self) {
            let badge = self.child_badge(PARENT).unwrap();
            assert_eq!(badge.summary, "4 分支 · 1 异常");
            assert_eq!(
                badge.detail,
                "Claude 1 · Codex 1 · Shell 2；运行中 1 · 待输入 1 · 错误 1"
            );
            assert!(self.child_badge(super::probe::PEER).is_none());
            println!("badge={} | hover={}", badge.summary, badge.detail);
        }

        pub(super) fn scroll_left(&self) -> Pixels {
            -self.carousel_scroll.offset().x
        }

        pub(super) fn assert_badge_hover(&self, hovered: bool) {
            assert_eq!(
                self.hovered_child_badge.as_deref(),
                hovered.then_some(PARENT)
            );
        }

        pub(super) fn assert_terminal_focus(
            &self,
            id: &str,
            expected: bool,
            window: &Window,
            cx: &Context<Self>,
        ) {
            assert_eq!(
                self.terminals[id].read(cx).focus.is_focused(window),
                expected
            );
        }

        pub(super) fn focus_return_button(&self, window: &mut Window) {
            window.focus(&self.control_focus["session-return-parent"]);
        }

        pub(super) fn scroll_for_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
            self.scroll_carousel(
                &ScrollWheelEvent {
                    position: point(px(700.), px(250.)),
                    delta: ScrollDelta::Pixels(point(px(-120.), px(0.))),
                    ..Default::default()
                },
                window,
                cx,
            );
        }

        pub(super) fn qualify(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
            let (cols, rows, cell_width, cell_height) = self.terminals[id]
                .read(cx)
                .target_dimensions()
                .expect("terminal painted in hidden GPUI layout");
            let mut terminal =
                Terminal::new(TerminalOptions::new(cols, rows, cell_width, cell_height)).unwrap();
            self.deliver(
                Event::Recovery {
                    session_id: id.into(),
                    state: RecoveryState::Ready,
                },
                window,
                cx,
            );
            self.deliver(
                Event::Frame {
                    session_id: id.into(),
                    screen: terminal.screen().unwrap(),
                    modes: terminal.input_modes().unwrap(),
                    selection: None,
                    sequence: 1,
                    scroll_request_id: 0,
                },
                window,
                cx,
            );
            assert!(!self.recovery_blocked(id));
        }

        pub(super) fn assert_sibling_command(
            &self,
            command: Command,
            parent: Option<&str>,
            source: &str,
        ) {
            assert!(matches!(command, Command::CreateShellSibling {
                scene_id, source_session_id, parent_session_id,
            } if scene_id == SCENE && source_session_id == source && parent_session_id.as_deref() == parent));
            println!("CreateShellSibling | source={source} | parent={parent:?}");
        }
    }
}

#[cfg(target_os = "macos")]
mod probe {
    use std::path::PathBuf;

    use cocoa::appkit::NSEventType;

    use gpui::{
        AnyWindowHandle, AppContext, Application, AsyncApp, Bounds, Keystroke, Pixels, Point,
        WindowBounds, WindowHandle, WindowOptions, point, px, size,
    };
    use matou_native_core::{
        geometry,
        hierarchy::{
            self, BootstrapWindowInput, HierarchyIds,
            canvas::{self, CanvasIds, CanvasMessages, CreateCanvasInput, CreateShellSiblingInput},
        },
        session_graph::project_scene_graph,
        startup::{self, BootstrapResult},
        storage::CommandMetadata,
    };
    use matou_native_runtime::host::{Command, Event, RecoveryState};

    use crate::{
        assets::NativeAssets,
        runtime_bridge::{self, ProbeReceiver},
        workspace::Workspace,
    };

    pub(super) const SCENE: &str = "levels-scene";
    pub(super) const PARENT: &str = "level-parent";
    pub(super) const PEER: &str = "level-peer";
    const CHILD: &str = "child-urgent";
    const WINDOW: &str = "levels-window";
    const SECOND_SCENE: &str = "other-scene";
    const SECOND_ROOT: &str = "other-root";
    const ROOTS: &[&str] = &[PARENT, PEER];
    const CHILDREN: &[&str] = &["child-first", "child-stopped", CHILD, "child-running"];
    const MESSAGES: CanvasMessages<'static> = CanvasMessages {
        new_canvas: "新画布",
        duplicate_scene_name: "画布名称重复",
        workspace_path_invalid: "工作区不可用",
        detached_session_must_return: "会话须返回画布",
    };

    struct Fixture {
        db: Option<BootstrapResult>,
        root: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.db.take());
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn command(id: &str) -> CommandMetadata {
        CommandMetadata {
            command_id: id.into(),
            command_type: id.into(),
            request_hash: id.into(),
            causation_id: None,
            correlation_id: None,
        }
    }

    fn canvas_ids(scene_id: &str, session_id: &str) -> CanvasIds {
        CanvasIds {
            scene_id: scene_id.into(),
            session_id: session_id.into(),
            root_node_id: format!("{session_id}-node"),
            secondary_node_id: format!("{session_id}-split"),
            mount_id: format!("{session_id}-mount"),
            relation_id: format!("{session_id}-relation"),
        }
    }

    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target")
                .join(format!("session-levels-probe-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let mut fixture = Self { db: None, root };
            fixture.db = Some(startup::open(&fixture.root, "session-levels-probe").unwrap());
            let Some(BootstrapResult::Writable { connection, .. }) = fixture.db.as_mut() else {
                panic!("expected writable isolated fixture")
            };
            hierarchy::bootstrap_window(
                connection,
                &command("bootstrap"),
                &BootstrapWindowInput {
                    window_id: WINDOW.into(),
                    default_root_directory: fixture.root.to_string_lossy().into_owned(),
                    default_name: "会话层级探针".into(),
                    now: 1,
                },
                &HierarchyIds {
                    workspace_id: "levels-workspace".into(),
                    execution_context_id: "levels-context".into(),
                    task_id: "levels-task".into(),
                    scene_id: SCENE.into(),
                    root_node_id: "levels-root-node".into(),
                    session_id: PARENT.into(),
                    mount_id: "levels-root-mount".into(),
                },
                "工作台",
                1,
            )
            .unwrap();
            for (index, id) in [PEER]
                .into_iter()
                .chain(CHILDREN.iter().copied())
                .enumerate()
            {
                let now = index as i64 + 2;
                canvas::create_shell_sibling(
                    connection,
                    &command(id),
                    &CreateShellSiblingInput {
                        window_id: WINDOW.into(),
                        scene_id: SCENE.into(),
                        source_session_id: PARENT.into(),
                        parent_session_id: (id != PEER).then(|| PARENT.into()),
                        execution_context_id: None,
                        title: Some(id.into()),
                        navigation: None,
                        now,
                    },
                    &canvas_ids(SCENE, id),
                    &MESSAGES,
                    now,
                )
                .unwrap();
            }
            connection.execute_batch(
                "UPDATE sessions SET title = '分支主会话' WHERE id = 'level-parent';
                 UPDATE sessions SET kind = 'claude-code', work_status = 'needs-input' WHERE id = 'child-urgent';
                 UPDATE sessions SET kind = 'codex', work_status = 'running' WHERE id = 'child-running';
                 UPDATE sessions SET archived_at = 8, work_status = 'error' WHERE id = 'child-stopped';",
            ).unwrap();
            canvas::create_canvas(
                connection,
                &command("other-canvas"),
                &CreateCanvasInput {
                    window_id: WINDOW.into(),
                    task_id: "levels-task".into(),
                    title: Some("另一画布".into()),
                    navigation: None,
                    now: 9,
                },
                &canvas_ids(SECOND_SCENE, SECOND_ROOT),
                &MESSAGES,
                9,
            )
            .unwrap();
            fixture.focus(SCENE, PEER);
            fixture
        }

        fn focus(&mut self, scene: &str, session: &str) {
            let Some(BootstrapResult::Writable { connection, .. }) = self.db.as_mut() else {
                unreachable!()
            };
            hierarchy::focus_session(connection, WINDOW, scene, session, 10, "会话不在当前画布中")
                .unwrap();
        }

        fn event(&self, ready: bool) -> Event {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            let hierarchy = hierarchy::read_hierarchy(connection, WINDOW).unwrap();
            let scene_id = &hierarchy.scene.as_ref().unwrap().id;
            let scenes = hierarchy::list_scenes(connection, "levels-task").unwrap();
            let graph = project_scene_graph(connection, scene_id, Some(WINDOW)).unwrap();
            let geometry = geometry::list(connection, scene_id).unwrap();
            if ready {
                Event::Ready {
                    hierarchy,
                    scenes,
                    graph,
                    geometry,
                }
            } else {
                Event::Navigation {
                    hierarchy,
                    scenes,
                    graph,
                    geometry,
                }
            }
        }

        fn process(
            &mut self,
            window: WindowHandle<Workspace>,
            cx: &mut AsyncApp,
            sink: &ProbeReceiver,
        ) -> Vec<Command> {
            let mut actions = Vec::new();
            while let Ok(command) = sink.try_recv() {
                match command {
                    Command::FocusSession {
                        ref scene_id,
                        ref session_id,
                    } => {
                        self.focus(scene_id, session_id);
                        deliver(window, cx, self.event(false));
                        actions.push(command);
                    }
                    Command::PutGeometry { sequence, update } => {
                        let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                            unreachable!()
                        };
                        let update = geometry::put(connection, &update).unwrap();
                        println!(
                            "PutGeometry | owner={} | {}",
                            update.owner_key, update.geometry
                        );
                        deliver(
                            window,
                            cx,
                            Event::GeometryResult {
                                sequence,
                                update,
                                error: None,
                            },
                        );
                    }
                    Command::CreateShellSibling { .. } => actions.push(command),
                    _ => {}
                }
            }
            actions
        }

        fn assert_geometry(&self) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            let saved = geometry::list(connection, SCENE).unwrap();
            assert_eq!(
                saved
                    .iter()
                    .map(|update| update.owner_key.as_str())
                    .collect::<Vec<_>>(),
                [
                    "session-group:levels-scene:level-parent",
                    "session-group:levels-scene:root"
                ],
            );
            assert!(saved[0].geometry["scrollLeft"].as_f64().unwrap() > 0.);
            assert_eq!(saved[0].geometry["anchorSessionId"], CHILD);
            assert_eq!(saved[1].geometry["scrollLeft"].as_f64(), Some(0.));
            println!("Persisted independent root and child geometry in SQLite");
        }

        fn archived_children_graph(&self) -> serde_json::Value {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            connection
                .execute_batch("UPDATE sessions SET archived_at = 11 WHERE id LIKE 'child-%'")
                .unwrap();
            project_scene_graph(connection, SCENE, Some(WINDOW)).unwrap()
        }
    }

    fn draw(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
    }

    fn deliver(window: WindowHandle<Workspace>, cx: &mut AsyncApp, event: Event) {
        window
            .update(cx, |view, window, cx| view.deliver(event, window, cx))
            .unwrap();
    }

    fn assert_focus(actions: Vec<Command>, focus: &str) {
        assert!(
            matches!(actions.as_slice(), [Command::FocusSession { scene_id, session_id }]
            if scene_id == SCENE && session_id == focus),
            "expected a single focus command for {focus}"
        );
    }

    fn mouse(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        position: Point<Pixels>,
        kind: NSEventType,
    ) {
        use cocoa::{
            appkit::{NSEvent, NSEventModifierFlags},
            base::{id, nil},
            foundation::{NSInteger, NSPoint},
        };
        use objc::{msg_send, sel, sel_impl};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let (view, location, number) = window
            .update(cx, |_, window, _| {
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
                    f64::from(f32::from(position.x)),
                    f64::from(f32::from(window.viewport_size().height - position.y)),
                );
                (view, location, number)
            })
            .unwrap();
        // Target only the probe NSView. The event never enters the desktop queue.
        unsafe {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
                nil, kind, location, NSEventModifierFlags::empty(), 0., number, nil, 0, 1, 1.,
            );
            match kind {
                NSEventType::NSMouseMoved => {
                    let _: () = msg_send![view, mouseMoved: event];
                }
                NSEventType::NSLeftMouseDown => {
                    let _: () = msg_send![view, mouseDown: event];
                }
                NSEventType::NSLeftMouseUp => {
                    let _: () = msg_send![view, mouseUp: event];
                }
                _ => unreachable!(),
            }
        }
    }

    fn press(window: WindowHandle<Workspace>, cx: &mut AsyncApp, key: &str) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                assert!(window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx));
            })
            .unwrap();
    }

    fn scroll(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        window
            .update(cx, |view, window, cx| view.scroll_for_geometry(window, cx))
            .unwrap();
    }

    fn verify_compact_card(cx: &mut AsyncApp) {
        let mut fixture = Fixture::new();
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_events, incoming) = async_channel::unbounded();
        let window = cx
            .open_window(
                WindowOptions {
                    show: false,
                    focus: false,
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(900.), px(780.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
            )
            .unwrap();
        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
            })
            .unwrap();
        // Sample the compact header and summary rows where the obsolete
        // branch button would take hover, then click that same area.
        for inset in [20., 34., 42., 50., 58., 66.] {
            let position = window
                .update(cx, |view, window, _| {
                    view.compact_parent_position(window, px(inset))
                })
                .unwrap();
            mouse(window, cx, position, NSEventType::NSMouseMoved);
            window
                .update(cx, |view, _, _| view.assert_badge_hover(false))
                .unwrap();
        }
        let summary = window
            .update(cx, |view, window, _| {
                view.compact_parent_position(window, px(50.))
            })
            .unwrap();
        mouse(window, cx, summary, NSEventType::NSLeftMouseDown);
        mouse(window, cx, summary, NSEventType::NSLeftMouseUp);
        assert_focus(fixture.process(window, cx, &sink), PARENT);
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PARENT);
            })
            .unwrap();
        println!(
            "Compact parent has no branch badge hit target; click selected it in the root level"
        );

        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PARENT.into(),
                state: RecoveryState::Failed("probe recovery failure".into()),
            },
        );
        draw(window, cx);
        let badge = window
            .update(cx, |view, _, _| view.badge_position())
            .unwrap();
        mouse(window, cx, badge, NSEventType::NSMouseMoved);
        window
            .update(cx, |view, _, _| view.assert_badge_hover(true))
            .unwrap();
        mouse(window, cx, badge, NSEventType::NSLeftMouseDown);
        mouse(window, cx, badge, NSEventType::NSLeftMouseUp);
        assert_focus(fixture.process(window, cx, &sink), CHILD);
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_level(
                    Some(PARENT),
                    CHILDREN,
                    "分支主会话 的子会话",
                    "· 4 个会话",
                    CHILD,
                );
            })
            .unwrap();
        println!("The expanded recovery-failed card still opens its child level");
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }

    pub(super) fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
            let mut fixture = Fixture::new();
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
                    |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
                )
                .unwrap();
            cx.spawn(async move |cx| {
                verify_compact_card(cx);
                deliver(window, cx, fixture.event(true));
                draw(window, cx);
                window
                    .update(cx, |view, window, cx| {
                        view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
                        view.assert_root_badge();
                        view.qualify(PARENT, window, cx);
                        view.qualify(PEER, window, cx);
                        view.assert_terminal_focus(PEER, true, window, cx);
                    })
                    .unwrap();
                draw(window, cx);
                scroll(window, cx);
                assert!(fixture.process(window, cx, &sink).is_empty());

                let badge = window
                    .update(cx, |view, _, _| view.badge_position())
                    .unwrap();
                mouse(window, cx, badge, NSEventType::NSMouseMoved);
                draw(window, cx);
                window
                    .update(cx, |view, _, _| view.assert_badge_hover(true))
                    .unwrap();
                mouse(
                    window,
                    cx,
                    point(px(100.), px(100.)),
                    NSEventType::NSMouseMoved,
                );
                draw(window, cx);
                window
                    .update(cx, |view, _, _| view.assert_badge_hover(false))
                    .unwrap();
                println!("Badge hover entered and left on GPUI mouse movement");

                for return_key in [None, Some("left"), Some("up")] {
                    let root_focus = if return_key.is_none() { PEER } else { PARENT };
                    let position = window
                        .update(cx, |view, _, _| view.badge_position())
                        .unwrap();
                    mouse(window, cx, position, NSEventType::NSLeftMouseDown);
                    window
                        .update(cx, |view, window, cx| {
                            view.assert_terminal_focus(root_focus, true, window, cx);
                        })
                        .unwrap();
                    assert!(
                        fixture.process(window, cx, &sink).is_empty(),
                        "badge mouse-down focused the card"
                    );
                    mouse(window, cx, position, NSEventType::NSLeftMouseUp);
                    draw(window, cx);
                    window
                        .update(cx, |view, _, _| {
                            view.assert_level(
                                Some(PARENT),
                                CHILDREN,
                                "分支主会话 的子会话",
                                "· 4 个会话",
                                root_focus,
                            );
                            if return_key.is_some() {
                                assert!(
                                    view.scroll_left() > px(0.),
                                    "child scroll was not restored"
                                );
                            }
                        })
                        .unwrap();
                    if return_key.is_none() {
                        window
                            .update(cx, |view, window, cx| view.qualify(CHILD, window, cx))
                            .unwrap();
                    }
                    assert_focus(fixture.process(window, cx, &sink), CHILD);
                    draw(window, cx);
                    window
                        .update(cx, |view, window, cx| {
                            view.assert_level(
                                Some(PARENT),
                                CHILDREN,
                                "分支主会话 的子会话",
                                "· 4 个会话",
                                CHILD,
                            );
                            view.assert_terminal_focus(CHILD, false, window, cx);
                        })
                        .unwrap();

                    if return_key.is_none() {
                        scroll(window, cx);
                        press(window, cx, "cmd-d");
                        let mut actions = fixture.process(window, cx, &sink);
                        assert_eq!(actions.len(), 1);
                        window
                            .update(cx, |view, _, _| {
                                view.assert_sibling_command(
                                    actions.pop().unwrap(),
                                    Some(PARENT),
                                    CHILD,
                                );
                            })
                            .unwrap();
                    }

                    if let Some(key) = return_key {
                        window
                            .update(cx, |view, window, _| view.focus_return_button(window))
                            .unwrap();
                        // Key listeners use the dispatch path from the most recent frame.
                        draw(window, cx);
                        press(window, cx, key);
                    } else {
                        let position = window
                            .update(cx, |_, window, _| {
                                point(px(300.), window.viewport_size().height - px(19.))
                            })
                            .unwrap();
                        mouse(window, cx, position, NSEventType::NSLeftMouseDown);
                        mouse(window, cx, position, NSEventType::NSLeftMouseUp);
                    }
                    assert_focus(fixture.process(window, cx, &sink), PARENT);
                    draw(window, cx);
                    window
                        .update(cx, |view, window, cx| {
                            view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PARENT);
                            assert_eq!(view.scroll_left(), px(0.));
                            view.assert_terminal_focus(PARENT, true, window, cx);
                        })
                        .unwrap();
                    println!(
                        "Return through {} focused the parent terminal",
                        return_key.unwrap_or("click")
                    );
                }
                fixture.assert_geometry();

                press(window, cx, "cmd-d");
                let mut actions = fixture.process(window, cx, &sink);
                assert_eq!(actions.len(), 1);
                window
                    .update(cx, |view, _, _| {
                        view.assert_sibling_command(actions.pop().unwrap(), None, PARENT);
                    })
                    .unwrap();

                let badge = window
                    .update(cx, |view, _, _| view.badge_position())
                    .unwrap();
                mouse(window, cx, badge, NSEventType::NSLeftMouseDown);
                mouse(window, cx, badge, NSEventType::NSLeftMouseUp);
                assert_focus(fixture.process(window, cx, &sink), CHILD);
                fixture.focus(SECOND_SCENE, SECOND_ROOT);
                deliver(window, cx, fixture.event(false));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(
                            None,
                            &[SECOND_ROOT],
                            "根会话",
                            "· 1 个会话",
                            SECOND_ROOT,
                        );
                    })
                    .unwrap();
                fixture.focus(SCENE, PEER);
                deliver(window, cx, fixture.event(false));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(
                            Some(PARENT),
                            CHILDREN,
                            "分支主会话 的子会话",
                            "· 4 个会话",
                            PEER,
                        );
                    })
                    .unwrap();
                sink.generation(2);
                deliver(window, cx, fixture.event(true));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
                    })
                    .unwrap();
                println!("Scene overrides survived scene switch and were cleared on reconnect");

                let mut graph = fixture.archived_children_graph();
                deliver(window, cx, Event::Graph(graph.clone()));
                draw(window, cx);
                let badge = window
                    .update(cx, |view, _, _| view.badge_position())
                    .unwrap();
                mouse(window, cx, badge, NSEventType::NSLeftMouseDown);
                mouse(window, cx, badge, NSEventType::NSLeftMouseUp);
                assert!(fixture.process(window, cx, &sink).is_empty());
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(
                            Some(PARENT),
                            CHILDREN,
                            "分支主会话 的子会话",
                            "· 4 个会话",
                            PEER,
                        );
                    })
                    .unwrap();
                println!("An archived-only level opened without changing focus");

                graph["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|node| node["sessionId"] != PARENT);
                deliver(window, cx, Event::Graph(graph.clone()));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(None, &[PEER], "根会话", "· 1 个会话", PEER);
                    })
                    .unwrap();
                graph["nodes"].as_array_mut().unwrap().clear();
                deliver(window, cx, Event::Graph(graph));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(None, &[], "根会话", "· 0 个会话", PEER);
                    })
                    .unwrap();
                drop(fixture);

                println!("Session levels GPUI probe passed");
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
    }
}

#[cfg(target_os = "macos")]
fn main() {
    probe::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {}
