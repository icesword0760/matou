//! Hidden GPUI interaction and offline Runtime wiring probe.
#[cfg(all(target_os = "macos", debug_assertions))]
pub use matou_native_desktop::{
    assets, macos, recovery_water_view, rename_input, tab_dialog_style, workspace_drag,
};

#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/carousel_motion.rs"]
mod carousel_motion;
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/history_view.rs"]
mod history_view;
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/provider_settings.rs"]
mod provider_settings;
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "support/runtime_probe.rs"]
mod runtime_bridge;
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/settings_shell.rs"]
mod settings_shell;
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/sidebar_scrollbar.rs"]
mod sidebar_scrollbar;
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/terminal_input.rs"]
mod terminal_input;
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/terminal_scrollbar.rs"]
mod terminal_scrollbar;
#[cfg(all(target_os = "macos", debug_assertions))]
#[allow(dead_code)]
mod terminal_surface {
    include!("../src/terminal_surface.rs");

    impl TerminalSurface {
        pub(crate) fn scrollbar_track_center_for_split_probe(&self) -> Point<Pixels> {
            let bounds = self.scrollbar_bounds.expect("painted terminal scrollport");
            point(bounds.right() - px(2.5), bounds.center().y)
        }

        pub(crate) fn scrollport_hovered_for_split_probe(&self) -> bool {
            self.scrollport_hovered
        }

        pub(crate) fn assert_no_terminal_drag_for_split_probe(&self) {
            assert!(!self.selecting, "divider started a terminal selection");
            assert!(
                self.scrollbar_drag.is_none(),
                "divider started a scrollbar drag"
            );
        }
    }
}
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/ui_text.rs"]
mod ui_text;

#[cfg(all(target_os = "macos", debug_assertions))]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    mod live_wiring {
        include!("support/live_wiring_probe.rs");
    }

    use crate::assets::NativeAssets;
    use matou_native_core::link_routing::ControlledBrowserFailure;
    use matou_native_desktop::{
        card_context_menu::CardMenuAction,
        detach_window::probe_detached_header,
        egress_panel::{
            EgressFamily, EgressFamilyStatus, EgressState, EgressStatus, EgressTransition,
            EgressViolation,
        },
        update_popover::{
            UpdateAction, UpdateInstallMode, UpdateProgress, UpdateRelease, UpdateStatus,
        },
    };
    use serde_json::json;
    use workspace_wiring::{LinkOpenTarget, capture_wiring_bounds};

    const A: &str = "shell-a";
    const B: &str = "shell-b";
    const TEAM: &str = "teammate";

    fn graph() -> Value {
        json!({"sceneId":"main", "workspaceSessionCount":3, "nodes":[
            {"sessionId":A, "currentMode":"shell", "title":"First shell", "workStatus":"idle"},
            {"sessionId":B, "currentMode":"shell", "title":"Second shell", "workStatus":"idle"},
            {"sessionId":TEAM, "currentMode":"agent-team-member", "title":"Teammate", "workStatus":"running", "latestLines":["Working on the report"]}
        ]})
    }

    fn install_fixture(view: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        view.hide_wiring_children_for_probe();
        view.runtime_generation = 1;
        view.transport_connected = true;
        view.domain_ready = true;
        let task = json!({
            "id":"task", "workspaceId":"workspace", "title":"Fixture task", "status":"open",
            "executionContextId":"local", "sortKey":"a", "isPinned":false, "pinSortKey":"",
            "lastOpenedAt":0, "createdAt":0, "updatedAt":0, "version":1
        });
        let workspace = json!({
            "id":"workspace", "name":"Fixture workspace", "rootDirectory":".", "taskOrder":["task"],
            "isDefault":true, "isPinned":false, "pinSortKey":"", "lastOpenedAt":0,
            "createdAt":0, "updatedAt":0, "version":1
        });
        let scene = json!({
            "id":"main", "taskId":"task", "name":"Fixture canvas", "mode":"terminal", "titlePinned":false,
            "sortKey":"a", "layoutRevision":0, "createdAt":0, "updatedAt":0
        });
        view.apply_workspaces(vec![serde_json::from_value(workspace.clone()).unwrap()], cx);
        view.apply_tasks(vec![serde_json::from_value(task.clone()).unwrap()], cx);
        view.apply(BridgeEvent::Runtime {
            generation: 1,
            event: Event::Navigation {
                hierarchy: serde_json::from_value(json!({
                    "workspace":workspace, "task":task, "scene":scene,
                    "session":{"id":A,"taskId":"task","executionContextId":"local","kind":"shell","status":"active",
                    "title":"First shell","cwd":".","createdAt":0,"updatedAt":0,"lastActivityAt":0,"version":1},
                    "navigation":{"windowId":"wiring-probe","activeWorkspaceId":"workspace","taskByWorkspace":{},
                    "sceneByTask":{},"sessionByScene":{}}
                })).unwrap(),
                scenes: vec![serde_json::from_value(scene).unwrap()],
                graph: graph(),
                geometry: vec![],
            },
        }, window, cx);
        for id in [A, B] {
            view.session_frames.insert(id.into());
            view.recovery.insert(id.into(), RecoveryState::Ready);
        }
        view.sync_terminal_input(window, cx);
        window.focus(&view.control_focus["sidebar-board"]);
    }

    type Handle = WindowHandle<Workspace>;

    fn draw(window: AnyWindowHandle, cx: &mut AsyncApp) -> HashMap<String, Bounds<Pixels>> {
        capture_wiring_bounds(|| {
            window
                .update(cx, |_, window, cx| {
                    assert!(!crate::macos::is_window_visible(window).unwrap());
                    window.refresh();
                    window.draw(cx).clear();
                })
                .unwrap()
        })
    }

    fn press(window: Handle, cx: &mut AsyncApp, key: &str) {
        dispatch_key(window.into(), cx, key);
    }

    fn dispatch_key(window: AnyWindowHandle, cx: &mut AsyncApp, key: &str) -> bool {
        window
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx)
            })
            .unwrap()
    }

    fn native_view(handle: AnyWindowHandle, cx: &mut AsyncApp) -> (cocoa::base::id, Size<Pixels>) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        handle
            .update(cx, |_, window, _| {
                let RawWindowHandle::AppKit(handle) =
                    HasWindowHandle::window_handle(window).unwrap().as_raw()
                else {
                    unreachable!("macOS window");
                };
                (
                    handle.ns_view.as_ptr() as cocoa::base::id,
                    window.viewport_size(),
                )
            })
            .unwrap()
    }

    fn click(
        window: AnyWindowHandle,
        cx: &mut AsyncApp,
        button: MouseButton,
        position: Point<Pixels>,
    ) {
        use cocoa::{
            appkit::{NSEvent, NSEventModifierFlags, NSEventType},
            base::{id, nil},
            foundation::{NSInteger, NSPoint},
        };
        use objc::{msg_send, sel, sel_impl};
        let (view, size) = native_view(window, cx);
        let location = NSPoint::new(f64::from(position.x), f64::from(size.height - position.y));
        // Send directly to the probe window's NSView, bypassing the desktop event queue.
        unsafe {
            let native: id = msg_send![view, window];
            let number: NSInteger = msg_send![native, windowNumber];
            let event = |kind| {
                NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
                nil, kind, location, NSEventModifierFlags::empty(), 0., number, nil, 0, 1, 1.,
            )
            };
            match button {
                MouseButton::Left => {
                    let _: () = msg_send![view, mouseDown: event(NSEventType::NSLeftMouseDown)];
                    let _: () = msg_send![view, mouseUp: event(NSEventType::NSLeftMouseUp)];
                }
                MouseButton::Right => {
                    use core_foundation::base::CFRelease;
                    use core_graphics::{
                        display::CGDisplay,
                        event::{CGEventType, CGMouseButton},
                        geometry::CGPoint,
                    };
                    use std::ffi::c_void;
                    unsafe extern "C" {
                        fn CGEventCreateMouseEvent(
                            source: *const c_void,
                            kind: CGEventType,
                            position: CGPoint,
                            button: CGMouseButton,
                        ) -> *mut c_void;
                    }
                    let position = CGPoint::new(
                        location.x,
                        CGDisplay::main().bounds().size.height - location.y,
                    );
                    let right_event = |kind| {
                        let event = CGEventCreateMouseEvent(
                            std::ptr::null(),
                            kind,
                            position,
                            CGMouseButton::Right,
                        );
                        assert!(!event.is_null());
                        let native = NSEvent::eventWithCGEvent_(nil, event);
                        CFRelease(event);
                        assert_eq!(native.buttonNumber(), 1);
                        native
                    };
                    let _: () =
                        msg_send![view, rightMouseDown: right_event(CGEventType::RightMouseDown)];
                    let _: () =
                        msg_send![view, rightMouseUp: right_event(CGEventType::RightMouseUp)];
                }
                _ => unreachable!(),
            }
        }
    }

    fn close_native(window: AnyWindowHandle, cx: &mut AsyncApp) {
        use cocoa::base::{id, nil};
        use objc::{msg_send, sel, sel_impl};
        let (view, _) = native_view(window, cx);
        unsafe {
            let native: id = msg_send![view, window];
            let _: () = msg_send![native, performClose: nil];
        }
    }

    fn overlay(window: Handle, cx: &mut AsyncApp, expected: Option<&str>) {
        window
            .update(cx, |view, _, _| {
                assert_eq!(view.wiring_overlay_for_probe(), expected)
            })
            .unwrap();
    }

    fn assert_terminal_input(
        window: Handle,
        sink: &crate::runtime_bridge::ProbeReceiver,
        cx: &mut AsyncApp,
        id: &str,
    ) {
        draw(window.into(), cx);
        window
            .update(cx, |view, window, cx| {
                assert!(view.terminals[id].read(cx).focus.is_focused(window), "{id}");
                assert!(!view.control_focus["graph"].is_focused(window));
            })
            .unwrap();
        press(window, cx, "x");
        press(window, cx, "enter");
        let input: Vec<_> = std::iter::from_fn(|| sink.try_recv().ok())
            .filter_map(|command| match command {
                Command::Input {
                    session_id, bytes, ..
                } => {
                    assert_eq!(session_id, id);
                    Some(bytes)
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(input, b"x\r");
    }

    fn card_bounds(window: Handle, cx: &mut AsyncApp, id: &str) -> Bounds<Pixels> {
        window
            .update(cx, |view, _, _| {
                let index = view.order.iter().position(|card| card == id).unwrap();
                let mut bounds = view
                    .carousel_scroll
                    .bounds_for_item(index)
                    .expect("drawn card");
                bounds.origin += view.carousel_scroll.offset();
                bounds
            })
            .unwrap()
    }

    fn card_header(window: Handle, cx: &mut AsyncApp, id: &str) -> Point<Pixels> {
        card_bounds(window, cx, id).origin + point(px(32.), px(20.))
    }

    fn card_action(window: Handle, cx: &mut AsyncApp, id: &str, action: CardMenuAction) {
        draw(window.into(), cx);
        let header = card_header(window, cx, id);
        click(window.into(), cx, MouseButton::Right, header);
        overlay(window, cx, Some("card menu"));
        window
            .update(cx, |view, _, cx| {
                view.select_card_menu_action_for_probe(action, cx);
            })
            .unwrap();
        draw(window.into(), cx);
        press(window, cx, "enter");
        overlay(window, cx, None);
    }

    fn frozen_status() -> EgressStatus {
        EgressStatus {
            state: EgressState::Abnormal,
            families: vec![EgressFamilyStatus {
                family: EgressFamily::Anthropic,
                state: EgressState::Abnormal,
                exit: None,
                baseline: None,
                violation: Some(EgressViolation::Mainland),
                overridden: false,
                frozen_session_ids: vec![A.into()],
            }],
            checks: vec![],
            rule_mode: Default::default(),
            protection_enabled: true,
            session_families: HashMap::from([(A.into(), EgressFamily::Anthropic)]),
            session_family_sets: HashMap::new(),
            checked_at: Some((9, 21)),
            last_transition: Some(EgressTransition::Frozen),
        }
    }

    fn open_probe_at(
        cx: &mut AsyncApp,
        width: f32,
    ) -> (Handle, crate::runtime_bridge::ProbeReceiver) {
        cx.update(|cx| {
            let (commands, sink) = crate::runtime_bridge::probe_channel();
            let (_, events) = async_channel::unbounded();
            let bounds = Bounds::centered(None, size(px(width), px(780.)), cx);
            let handle = cx
                .open_window(
                    WindowOptions {
                        show: false,
                        focus: false,
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |window, cx| cx.new(|cx| Workspace::new(commands, events, window, cx)),
                )
                .unwrap();
            handle.update(cx, install_fixture).unwrap();
            (handle, sink)
        })
        .unwrap()
    }

    fn trigger_point(window: Handle, cx: &mut AsyncApp, egress: bool) -> Point<Pixels> {
        window
            .update(cx, |view, _, _| view.trigger_for_probe(egress).center())
            .unwrap()
    }

    fn verify_theme_shortcut(
        window: Handle,
        sink: &crate::runtime_bridge::ProbeReceiver,
        cx: &mut AsyncApp,
        root: &std::path::Path,
    ) {
        write_theme_preference(root, ThemePreference::Light).unwrap();
        window
            .update(cx, |view, window, cx| {
                view.data_root = Some(root.to_path_buf());
                view.theme.set_preference(ThemePreference::Light);
                view.apply_theme_palette(cx);
                window.focus(&view.terminals[A].read(cx).focus);
            })
            .unwrap();
        while sink.try_recv().is_ok() {}
        for (preference, expected) in [
            (ThemePreference::Dark, AppTheme::Dark),
            (ThemePreference::Light, AppTheme::Light),
            (ThemePreference::Dark, AppTheme::Dark),
            (ThemePreference::Light, AppTheme::Light),
        ] {
            press(window, cx, "cmd-i");
            let chrome = crate::settings_shell::probe_language_zone(|| {
                draw(window.into(), cx);
            })
            .workspace_surfaces;
            let colors = [
                chrome.sidebar.expect("sidebar rendered"),
                chrome.topbar.expect("topbar rendered"),
                chrome.footer.expect("footer rendered"),
                chrome.card.expect("card rendered"),
                chrome.card_header.expect("card header rendered"),
            ];
            if expected == AppTheme::Light {
                assert_eq!(
                    colors,
                    [
                        rgb(0xf2f2f5), // Over the original AppKit sidebar material.
                        rgb(0xffffff),
                        rgb(0xffffff),
                        rgb(0xfcfcfd),
                        rgb(0xffffff),
                    ],
                );
            } else {
                let palette = expected.palette();
                assert_eq!(
                    colors,
                    [
                        rgba(palette.sidebar.background),
                        rgba(palette.tabs.bar),
                        rgba(palette.footer.background),
                        rgba(palette.cards.background),
                        rgba(palette.cards.header),
                    ],
                );
                for color in colors {
                    assert_eq!(color.a, 1.);
                    assert_ne!(color, rgb(0xffffff));
                    assert_ne!(color, rgb(0xeae9e4));
                }
            }
            window
                .update(cx, |view, window, cx| {
                    assert_eq!(view.theme.effective(), expected);
                    assert!(view.terminals[A].read(cx).focus.is_focused(window));
                })
                .unwrap();
            assert_eq!(read_theme_preference(root).unwrap(), preference);
            let commands = std::iter::from_fn(|| sink.try_recv().ok()).collect::<Vec<_>>();
            assert!(commands.iter().any(|command| matches!(
                command,
                Command::Theme { session_id, theme }
                    if session_id == A && *theme == expected.palette().terminal
            )));
        }
        for key in ["cmd-shift-i", "cmd-alt-i", "ctrl-i"] {
            press(window, cx, key);
            assert_eq!(read_theme_preference(root).unwrap(), ThemePreference::Light);
        }
        assert!(
            !std::iter::from_fn(|| sink.try_recv().ok())
                .any(|command| matches!(command, Command::Theme { .. }))
        );
        window
            .update(cx, |view, _, _| view.data_root = None)
            .unwrap();
        println!(
            "theme shortcut: four Cmd+I toggles verify shell colors, preserve terminal focus and persist theme.json"
        );
    }

    fn verify_detached_theme_shortcut(
        main: Handle,
        children: [AnyWindowHandle; 2],
        sink: &crate::runtime_bridge::ProbeReceiver,
        cx: &mut AsyncApp,
        root: &std::path::Path,
    ) {
        write_theme_preference(root, ThemePreference::Light).unwrap();
        let (focuses, dag) = main
            .update(cx, |view, window, cx| {
                view.data_root = Some(root.to_path_buf());
                let mut graph = view.graph.clone();
                graph["edges"] = json!([]);
                for node in graph["nodes"].as_array_mut().unwrap() {
                    node["cwd"] = json!(".");
                }
                let dag = DagWindow::open(&graph, A, view.locale, false, false, window, cx)
                    .expect("open hidden DAG");
                view.dag_window = Some(dag);
                (
                    [A, B].map(|id| view.terminals[id].read(cx).focus.clone()),
                    dag,
                )
            })
            .unwrap();
        draw(dag.into(), cx);
        dag.update(cx, |view, window, cx| {
            view.probe_canvas().1.update(cx, |canvas, cx| {
                assert!(!canvas.dark_for_probe());
                canvas.focus_search(window, cx);
            });
        })
        .unwrap();
        for (child, focus) in children.into_iter().zip(&focuses) {
            draw(child, cx);
            child
                .update(cx, |_, window, _| assert!(focus.is_focused(window)))
                .unwrap();
        }
        while sink.try_recv().is_ok() {}
        let verify_theme = |preference: ThemePreference, cx: &mut AsyncApp| {
            let (expected, header, topbar) = if preference == ThemePreference::Dark {
                (
                    AppTheme::Dark,
                    0x11151c,
                    rgba(AppTheme::Dark.palette().tabs.bar),
                )
            } else {
                (AppTheme::Light, 0xffffff, rgb(0xffffff))
            };
            assert_eq!(read_theme_preference(root).unwrap(), preference);
            assert_eq!(
                main.update(cx, |view, _, _| view.theme.effective())
                    .unwrap(),
                expected,
            );
            let chrome = crate::settings_shell::probe_language_zone(|| {
                draw(main.into(), cx);
            });
            assert_eq!(chrome.workspace_surfaces.topbar, Some(topbar));
            let terminal_theme = expected.palette().terminal;
            for (child, focus) in children.into_iter().zip(&focuses) {
                let background = crate::terminal_surface::probe_terminal_background(|| {
                    let (_, color) = probe_detached_header(|| {
                        draw(child, cx);
                    })
                    .expect("detached header rendered");
                    assert_eq!(color, rgb(header));
                });
                assert_eq!(background, Some(terminal_theme.background));
                child
                    .update(cx, |_, window, _| assert!(focus.is_focused(window)))
                    .unwrap();
            }
            draw(dag.into(), cx);
            dag.update(cx, |view, window, cx| {
                let canvas = view.probe_canvas().1;
                let canvas = canvas.read(cx);
                assert_eq!(canvas.dark_for_probe(), expected == AppTheme::Dark);
                assert!(canvas.search_focused(window, cx));
            })
            .unwrap();
            let commands = std::iter::from_fn(|| sink.try_recv().ok()).collect::<Vec<_>>();
            for id in [A, B] {
                assert!(commands.iter().any(|command| matches!(
                    command,
                    Command::Theme { session_id, theme } if session_id == id && *theme == terminal_theme
                )));
            }
            assert!(
                commands
                    .iter()
                    .all(|command| !matches!(command, Command::Input { .. }))
            );
        };
        for (source, preference) in [
            (children[0], ThemePreference::Dark),
            (children[0], ThemePreference::Light),
            (main.into(), ThemePreference::Dark),
            (children[1], ThemePreference::Light),
        ] {
            assert!(dispatch_key(source, cx, "cmd-i"));
            verify_theme(preference, cx);
        }
        assert!(!dispatch_key(children[0], cx, "cmd-shift-i"));
        assert!(!dispatch_key(children[0], cx, "cmd-alt-i"));
        assert_eq!(read_theme_preference(root).unwrap(), ThemePreference::Light);
        assert!(
            !std::iter::from_fn(|| sink.try_recv().ok())
                .any(|command| matches!(command, Command::Theme { .. }))
        );
        for preference in [ThemePreference::Dark, ThemePreference::Light] {
            main.update(cx, |view, window, cx| {
                view.toggle_settings(window, cx);
                view.settings.as_ref().unwrap().0.update(cx, |_, cx| {
                    cx.emit(SettingsShellEvent::SaveTheme(preference));
                });
            })
            .unwrap();
            main.update(cx, |view, window, cx| view.close_settings(window, cx))
                .unwrap();
            verify_theme(preference, cx);
        }
        dag.update(cx, |_, window, _| window.remove_window())
            .unwrap();
        main.update(cx, |view, _, _| {
            view.dag_window = None;
            view.data_root = None;
        })
        .unwrap();
        println!(
            "detached theme shortcut: Cmd+I and Settings synchronize the main window, both child headers and terminal palettes, and DAG; focus and theme.json verified; Cmd+Shift+I/Alt+Cmd+I unhandled"
        );
    }

    async fn verify(cx: &mut AsyncApp, fixture_root: &std::path::Path) {
        let (window, sink) = open_probe_at(cx, 1260.);
        draw(window.into(), cx);
        for key in ["cmd-/", "cmd-?"] {
            press(window, cx, key);
            overlay(window, cx, Some("shortcuts"));
            draw(window.into(), cx);
            press(window, cx, "escape");
            overlay(window, cx, None);
            window
                .update(cx, |view, window, _| {
                    assert!(view.control_focus["sidebar-board"].is_focused(window))
                })
                .unwrap();
            draw(window.into(), cx);
        }
        window
            .update(cx, |view, window, cx| {
                window.focus(&view.terminals[A].read(cx).focus)
            })
            .unwrap();
        draw(window.into(), cx);
        press(window, cx, "ctrl-/");
        press(window, cx, "/");
        overlay(window, cx, None);
        verify_theme_shortcut(window, &sink, cx, &fixture_root.join("theme-shortcut"));
        assert!(!draw(window.into(), cx).contains_key("shortcut-help"));
        press(window, cx, "cmd-/");
        overlay(window, cx, Some("shortcuts"));
        draw(window.into(), cx);
        press(window, cx, "escape");
        overlay(window, cx, None);
        assert_terminal_input(window, &sink, cx, A);

        for outside in [true, false] {
            press(window, cx, "cmd-/");
            overlay(window, cx, Some("shortcuts"));
            draw(window.into(), cx);
            cx.background_executor()
                .timer(Duration::from_millis(220))
                .await;
            draw(window.into(), cx);
            let point = if outside {
                point(px(14.), px(80.))
            } else {
                point(px(998.), px(87.))
            };
            click(window.into(), cx, MouseButton::Left, point);
            overlay(window, cx, None);
        }
        println!(
            "shortcuts: Mod+/ and Mod+?, close button, Escape, backdrop and restored terminal focus"
        );

        let account = draw(window.into(), cx)["footer-account"].center();
        click(window.into(), cx, MouseButton::Left, account);
        overlay(window, cx, Some("accounts"));
        let request_id = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GetClaudeAccounts { request_id } => Some(request_id),
                _ => None,
            })
            .expect("accounts open the dedicated local roster");
        let mut state = json!({
            "currentId":"first", "accounts":[
                {"id":"first","name":"001","email":"aa•••@example.invalid","needsLogin":true},
                {"id":"second","name":"gmail","email":"gm•••@example.invalid","needsLogin":false,
                 "usage":{"checkedAt":1800000000000_u64,"stale":true,
                    "fiveHour":{"used":90,"resetsAt":"2099-01-01T00:00:00Z"}}}
            ], "rememberScope":true, "renewedIds":[], "loginCodeSubmitted":false, "switched":false
        });
        let supply = |window: Handle, cx: &mut AsyncApp, request_id, state: &Value| {
            window
                .update(cx, |view, window, cx| {
                    view.receive_wiring_reply(
                        "claudeAccountsResult",
                        request_id,
                        &json!({"result":state}),
                        window,
                        cx,
                    );
                })
                .unwrap();
        };
        supply(window, cx, request_id, &state);
        let view = draw(window.into(), cx);
        assert_eq!(view["footer-account-panel"].size.width, px(410.));
        assert!(
            view["claude-account-card-first"].bottom() < view["claude-account-card-second"].top()
        );
        assert!(view.contains_key("claude-account-reauth-first"));

        click(
            window.into(),
            cx,
            MouseButton::Left,
            view["claude-account-refresh"].center(),
        );
        let refresh = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::ClaudeAccountCommand {
                    request_id,
                    action:
                        matou_native_runtime::claude_accounts::ClaudeAccountCommand::RefreshUsage,
                } => Some(request_id),
                _ => None,
            })
            .expect("refresh routes to Claude usage");
        supply(window, cx, refresh, &state);

        let switch = draw(window.into(), cx)["claude-account-switch-second"].center();
        click(window.into(), cx, MouseButton::Left, switch);
        let switched = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::ClaudeAccountCommand {
                    request_id,
                    action:
                        matou_native_runtime::claude_accounts::ClaudeAccountCommand::Switch {
                            id,
                            remember_scope: true,
                        },
                } if id == "second" => Some(request_id),
                _ => None,
            })
            .expect("switch routes to the selected Claude account");
        state["currentId"] = json!("second");
        state["switched"] = json!(true);
        supply(window, cx, switched, &state);
        let manage = draw(window.into(), cx)["footer-manage-accounts"].center();
        click(window.into(), cx, MouseButton::Left, manage);
        assert!(draw(window.into(), cx).contains_key("claude-account-edit-second"));
        press(window, cx, "escape");
        overlay(window, cx, None);
        window
            .update(cx, |view, window, _| {
                assert!(view.control_focus["accounts"].is_focused(window));
            })
            .unwrap();
        println!(
            "accounts: dedicated roster, expired sign-in and quota cards, refresh, switch, manage, Escape and restored trigger focus"
        );

        window
            .update(cx, |view, window, cx| {
                view.open_search(window, cx);
                view.search.as_mut().unwrap().query = "keep this query".into();
                window.focus(&view.control_focus["sidebar-board"]);
            })
            .unwrap();
        draw(window.into(), cx);
        let board_toggle = draw(window.into(), cx)["sidebar-board"].center();
        click(window.into(), cx, MouseButton::Left, board_toggle);
        let board = draw(window.into(), cx);
        assert!(board.contains_key("board-session-shell-a"));
        click(
            window.into(),
            cx,
            MouseButton::Left,
            board["board-session-shell-b"].center(),
        );
        window
            .update(cx, |view, _, _| {
                assert!(!view.wiring.board_open);
                assert_eq!(view.level_focus(), Some(B));
                let search = view.search.as_ref().unwrap();
                assert!(search.open);
                assert_eq!(search.query, "keep this query");
            })
            .unwrap();
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command|
            matches!(command, Command::FocusSession { scene_id, session_id } if scene_id == "main" && session_id == B)));
        window
            .update(cx, |view, window, cx| {
                view.close_search(window, cx);
                view.pending_focus = None;
                view.level_focus_request = None;
                view.focused = Some(A.into());
                view.refresh_level(cx);
                window.focus(&view.control_focus["sidebar-board"]);
            })
            .unwrap();
        draw(window.into(), cx);
        println!(
            "board: keyboard view toggle, actual session row click, canvas selection and preserved search"
        );

        let header = card_header(window, cx, A);
        click(window.into(), cx, MouseButton::Right, header);
        overlay(window, cx, Some("card menu"));
        window
            .update(cx, |view, _, _| {
                let actions = view
                    .wiring_card_menu(A)
                    .items
                    .into_iter()
                    .map(|item| item.action)
                    .collect::<Vec<_>>();
                assert_eq!(
                    actions,
                    [
                        CardMenuAction::Rename,
                        CardMenuAction::OpenDetached,
                        CardMenuAction::SplitHorizontal,
                        CardMenuAction::SplitVertical,
                        CardMenuAction::CloseSession,
                        CardMenuAction::RemoveNode
                    ]
                );
            })
            .unwrap();
        draw(window.into(), cx);
        press(window, cx, "escape");
        overlay(window, cx, None);
        window
            .update(cx, |view, window, _| {
                assert!(view.control_focus["sidebar-board"].is_focused(window))
            })
            .unwrap();
        draw(window.into(), cx);
        let header = card_header(window, cx, A);
        click(window.into(), cx, MouseButton::Right, header);
        overlay(window, cx, Some("card menu"));
        draw(window.into(), cx);
        click(
            window.into(),
            cx,
            MouseButton::Left,
            point(px(14.), px(80.)),
        );
        overlay(window, cx, None);
        card_action(window, cx, A, CardMenuAction::Rename);
        window.update(cx, |view, window, cx| {
            assert!(matches!(&view.dialog, Some(TabDialog::RenameSession {session_id, ..}) if session_id == A));
            view.close_dialog(window, cx);
        }).unwrap();
        card_action(window, cx, B, CardMenuAction::CloseSession);
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command|
            matches!(command, Command::CloseSessionCard { session_id, .. } if session_id == B)));
        window
            .update(cx, |view, _, _| view.pending_card_action = None)
            .unwrap();
        println!(
            "card menu: right-click, Shell actions, Escape and outside dismissal, focus, rename and close"
        );

        card_action(window, cx, A, CardMenuAction::OpenDetached);
        card_action(window, cx, B, CardMenuAction::OpenDetached);
        let (first, second) = window
            .update(cx, |view, _, _| {
                (
                    view.wiring_child_for_probe(A).unwrap(),
                    view.wiring_child_for_probe(B).unwrap(),
                )
            })
            .unwrap();
        assert_ne!(first.window_id(), second.window_id());
        let (left, _) = probe_detached_header(|| {
            draw(first, cx);
        })
        .expect("detached title region rendered");
        assert!(left >= px(76.), "{left:?}");
        verify_detached_theme_shortcut(
            window,
            [first, second],
            &sink,
            cx,
            &fixture_root.join("detached-theme-shortcut"),
        );
        draw(second, cx);
        let mounts = draw(window.into(), cx);
        assert!(
            mounts.contains_key("detached-mount-shell-a")
                && mounts.contains_key("detached-mount-shell-b")
        );
        click(
            window.into(),
            cx,
            MouseButton::Left,
            mounts["detached-mount-shell-a"].center() - point(px(0.), px(35.)),
        );
        window
            .update(cx, |view, window, _| {
                window.focus(&view.control_focus["graph"])
            })
            .unwrap();
        close_native(first, cx);
        let mounts = draw(window.into(), cx);
        assert!(
            !mounts.contains_key("detached-mount-shell-a")
                && mounts.contains_key("detached-mount-shell-b")
        );
        assert_terminal_input(window, &sink, cx, A);

        card_action(window, cx, A, CardMenuAction::OpenDetached);
        let reopened = window
            .update(cx, |view, window, _| {
                window.focus(&view.control_focus["graph"]);
                view.wiring_child_for_probe(A).unwrap()
            })
            .unwrap();
        assert_ne!(first.window_id(), reopened.window_id());
        draw(reopened, cx);
        for key in ["cmd-shift-w", "cmd-alt-w", "cmd-ctrl-w"] {
            assert!(!dispatch_key(reopened, cx, key), "{key} was intercepted");
        }
        assert!(reopened.update(cx, |_, _, _| ()).is_ok());
        assert!(dispatch_key(reopened, cx, "cmd-w"));
        assert!(reopened.update(cx, |_, _, _| ()).is_err());
        assert!(second.update(cx, |_, _, _| ()).is_ok());
        let mounts = draw(window.into(), cx);
        assert!(!mounts.contains_key("detached-mount-shell-a"));
        assert!(mounts.contains_key("detached-mount-shell-b"));
        assert_terminal_input(window, &sink, cx, A);
        println!(
            "detached Cmd+W: target child closed, sibling stayed open, original terminal remounted and received only typed input; Cmd+Shift+W, Cmd+Alt+W and Cmd+Ctrl+W unhandled"
        );

        window
            .update(cx, |view, window, _| {
                window.focus(&view.control_focus["graph"])
            })
            .unwrap();
        let return_at = second
            .update(cx, |_, window, _| {
                point(window.viewport_size().width - px(70.), px(21.))
            })
            .unwrap();
        click(second, cx, MouseButton::Left, return_at);
        let mounts = draw(window.into(), cx);
        assert!(!mounts.contains_key("detached-mount-shell-b"));
        assert!(second.update(cx, |_, _, _| ()).is_err());
        assert_terminal_input(window, &sink, cx, B);
        println!(
            "detached: title inset {}pt; native close and return button remount each shell and route typing to it",
            f32::from(left),
        );

        let release = UpdateRelease {
            version: "99.0.1".into(),
            install_mode: UpdateInstallMode::Automatic,
            size_bytes: Some(100),
            release_day: None,
            notes: vec!["Fixture release".into()],
        };
        let update = trigger_point(window, cx, false);
        click(window.into(), cx, MouseButton::Left, update);
        overlay(window, cx, Some("update"));
        draw(window.into(), cx);
        window
            .update(cx, |view, window, cx| {
                let idle = view.update_view_for_probe();
                assert_eq!(
                    idle.subtitle,
                    format!(
                        "{} {}",
                        view.locale.text("当前版本", "Current version"),
                        env!("CARGO_PKG_VERSION"),
                    )
                );
                assert_eq!(idle.message, None);
                assert!(idle.buttons.iter().any(|button| button.enabled));
                view.request_update_action_for_probe(UpdateAction::Check, window, cx);
                assert!(!view.update_view_for_probe().error);
            })
            .unwrap();
        draw(window.into(), cx);
        let request_id = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::CheckForUpdate { request_id } => Some(request_id),
                _ => None,
            })
            .expect("update check command");
        window
            .update(cx, |view, window, cx| {
                view.apply(BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Graph(json!({
                        "type": "updateCheckResult", "requestId": request_id, "error": "fixture error"
                    })),
                }, window, cx);
                assert!(view.update_view_for_probe().error);
            })
            .unwrap();
        window
            .update(cx, |view, _, cx| {
                view.accept_update_status(UpdateStatus::Available(release.clone()), cx)
            })
            .unwrap();
        draw(window.into(), cx);
        window
            .update(cx, |view, _, cx| {
                view.accept_update_status(
                    UpdateStatus::Downloading(
                        release.clone(),
                        UpdateProgress {
                            percent: 40.,
                            transferred_bytes: 40,
                            total_bytes: 100,
                            bytes_per_second: 10,
                            remaining_seconds: Some(6),
                        },
                    ),
                    cx,
                )
            })
            .unwrap();
        draw(window.into(), cx);
        window
            .update(cx, |view, _, _| {
                assert!(view.update_view_for_probe().progress.is_some())
            })
            .unwrap();
        press(window, cx, "escape");
        overlay(window, cx, None);
        assert_terminal_input(window, &sink, cx, B);
        println!(
            "update: trigger toggle, idle/checking/error and retry, controlled release and 40% download"
        );

        window
            .update(cx, |view, window, cx| {
                view.set_egress_status(frozen_status(), window, cx)
            })
            .unwrap();
        let painted = draw(window.into(), cx);
        assert!(
            painted.contains_key("egress-banner-shell-a")
                && painted.contains_key("team-member-teammate")
        );
        let egress = trigger_point(window, cx, true);
        click(window.into(), cx, MouseButton::Left, egress);
        overlay(window, cx, Some("egress"));
        draw(window.into(), cx);
        click(window.into(), cx, MouseButton::Left, egress);
        overlay(window, cx, None);
        assert_terminal_input(window, &sink, cx, B);
        println!("egress and team: trigger toggle, frozen session banner, drawn team summary");

        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        for width in [1260., 820.] {
            let (window, _sink) = open_probe_at(cx, width);
            window
                .update(cx, |view, window, cx| {
                    assert_eq!(window.viewport_size().width, px(width));
                    view.set_egress_status(frozen_status(), window, cx);
                })
                .unwrap();
            for theme in [ThemePreference::Light, ThemePreference::Dark] {
                for locale in [InterfaceLocale::Chinese, InterfaceLocale::English] {
                    window
                        .update(cx, |view, window, cx| {
                            view.locale = locale;
                            view.theme.set_preference(theme);
                            view.apply_theme_palette(cx);
                            window.focus(&view.control_focus["sidebar-board"]);
                        })
                        .unwrap();
                    draw(window.into(), cx);
                    press(window, cx, "cmd-/");
                    overlay(window, cx, Some("shortcuts"));
                    draw(window.into(), cx);
                    press(window, cx, "escape");
                    press(window, cx, "enter");
                    assert!(draw(window.into(), cx).contains_key("board-session-shell-a"));
                    press(window, cx, "enter");
                    draw(window.into(), cx);
                    let header = card_header(window, cx, A);
                    click(window.into(), cx, MouseButton::Right, header);
                    overlay(window, cx, Some("card menu"));
                    draw(window.into(), cx);
                    press(window, cx, "escape");
                    for (is_egress, name) in [(false, "update"), (true, "egress")] {
                        draw(window.into(), cx);
                        let position = trigger_point(window, cx, is_egress);
                        click(window.into(), cx, MouseButton::Left, position);
                        overlay(window, cx, Some(name));
                        draw(window.into(), cx);
                        press(window, cx, "escape");
                    }
                    card_action(window, cx, A, CardMenuAction::OpenDetached);
                    let child = window
                        .update(cx, |view, _, _| view.wiring_child_for_probe(A).unwrap())
                        .unwrap();
                    draw(child, cx);
                    assert!(draw(window.into(), cx).contains_key("detached-mount-shell-a"));
                    child
                        .update(cx, |_, window, _| window.remove_window())
                        .unwrap();
                    let painted = draw(window.into(), cx);
                    assert!(painted.contains_key("egress-banner-shell-a"));
                    assert!(painted.contains_key("team-member-teammate"));
                }
            }
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
        println!(
            "render matrix: 7 entries × 2 themes × 2 locales × 2 viewport widths; all windows hidden"
        );
    }

    fn dispatch_mouse(window: Handle, cx: &mut AsyncApp, event: impl InputEvent) {
        use cocoa::{
            appkit::{NSEvent, NSEventModifierFlags, NSEventType},
            base::{id, nil},
            foundation::{NSInteger, NSPoint},
        };
        use objc::{msg_send, sel, sel_impl};
        let (kind, position, modifiers) = match event.to_platform_input() {
            PlatformInput::MouseDown(event) => (
                NSEventType::NSLeftMouseDown,
                event.position,
                event.modifiers,
            ),
            PlatformInput::MouseUp(event) => {
                (NSEventType::NSLeftMouseUp, event.position, event.modifiers)
            }
            PlatformInput::MouseMove(event) => {
                let kind = match event.pressed_button {
                    Some(MouseButton::Left) => NSEventType::NSLeftMouseDragged,
                    None => NSEventType::NSMouseMoved,
                    _ => unreachable!(),
                };
                (kind, event.position, event.modifiers)
            }
            _ => unreachable!(),
        };
        let (view, size) = native_view(window.into(), cx);
        let location = NSPoint::new(f64::from(position.x), f64::from(size.height - position.y));
        let mut flags = NSEventModifierFlags::empty();
        flags.set(NSEventModifierFlags::NSCommandKeyMask, modifiers.platform);
        flags.set(NSEventModifierFlags::NSControlKeyMask, modifiers.control);
        unsafe {
            let native: id = msg_send![view, window];
            let number: NSInteger = msg_send![native, windowNumber];
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
                nil, kind, location, flags, 0., number, nil, 0, 1, 1.,
            );
            match kind {
                NSEventType::NSLeftMouseDown => {
                    let _: () = msg_send![view, mouseDown: event];
                }
                NSEventType::NSLeftMouseUp => {
                    let _: () = msg_send![view, mouseUp: event];
                }
                NSEventType::NSLeftMouseDragged => {
                    let _: () = msg_send![view, mouseDragged: event];
                }
                NSEventType::NSMouseMoved => {
                    let _: () = msg_send![view, mouseMoved: event];
                }
                _ => unreachable!(),
            }
        }
    }

    fn mouse_down(position: Point<Pixels>, modifiers: Modifiers) -> MouseDownEvent {
        MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }
    }

    fn mouse_up(position: Point<Pixels>) -> MouseUpEvent {
        MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        }
    }

    fn draw_board(window: Handle, cx: &mut AsyncApp) -> HashMap<String, Bounds<Pixels>> {
        matou_native_desktop::workspace_board::capture_board_bounds(|| {
            draw(window.into(), cx);
        })
    }

    fn drag_board_card(window: Handle, cx: &mut AsyncApp, source: &str, destination: &str) {
        let bounds = draw_board(window, cx);
        let from = bounds[source].origin + point(px(18.), px(16.));
        let to = bounds[destination].center();
        dispatch_mouse(window, cx, mouse_down(from, Modifiers::default()));
        for position in [from + point(px(10.), px(0.)), to] {
            dispatch_mouse(
                window,
                cx,
                MouseMoveEvent {
                    position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                },
            );
            draw_board(window, cx);
        }
        assert!(window.update(cx, |_, _, cx| cx.has_active_drag()).unwrap());
        dispatch_mouse(window, cx, mouse_up(to));
        assert!(!window.update(cx, |_, _, cx| cx.has_active_drag()).unwrap());
    }

    fn publish_shell(window: Handle, cx: &mut AsyncApp, id: &str) {
        window
            .update(cx, |view, window, cx| {
                let mut hierarchy = view.hierarchy.clone().unwrap();
                let session = hierarchy.session.as_mut().unwrap();
                session.id = id.into();
                session.title = id.into();
                let mut graph = view.graph.clone();
                graph["focusedSessionId"] = json!(id);
                graph["nodes"].as_array_mut().unwrap().push(json!({
                    "sessionId": id, "currentMode": "shell", "workStatus": "idle", "title": id
                }));
                view.session_frames.insert(id.into());
                view.recovery.insert(id.into(), RecoveryState::Ready);
                view.apply(
                    BridgeEvent::Runtime {
                        generation: 1,
                        event: Event::Navigation {
                            hierarchy,
                            scenes: view.scenes.clone(),
                            graph,
                            geometry: vec![],
                        },
                    },
                    window,
                    cx,
                );
            })
            .unwrap();
    }

    fn verify_horizontal_divider_drag(cx: &mut AsyncApp) {
        use matou_native_desktop::split_tree::with_split_drag_button_for_probe;
        use matou_native_terminal::{Terminal, TerminalOptions};

        const RIGHT: &str = "drag-right";
        const LEFT_PANE: &str = "split-pane-shell-a";
        const RIGHT_PANE: &str = "split-pane-drag-right";
        const RELEASE_COUNT: u32 = 42;
        let (window, sink) = open_probe_at(cx, 1200.);
        card_action(window, cx, A, CardMenuAction::SplitHorizontal);
        publish_shell(window, cx, RIGHT);
        draw(window.into(), cx);
        window
            .update(cx, |view, _, cx| {
                for id in [A, RIGHT] {
                    view.terminals[id].update(cx, |surface, cx| {
                        let (cols, rows, cell_width, cell_height) =
                            surface.target_dimensions().unwrap();
                        let mut terminal = Terminal::new(TerminalOptions::new(
                            cols,
                            rows,
                            cell_width,
                            cell_height,
                        ))
                        .unwrap();
                        for row in 0..180 {
                            terminal.feed(format!("history row {row:03}\r\n").as_bytes());
                        }
                        terminal.scroll(6).unwrap();
                        let screen = terminal.screen().unwrap();
                        assert!(screen.scrollbar.offset > 0);
                        assert!(screen.scrollbar.total > u64::from(rows));
                        surface.set_screen(screen, terminal.input_modes().unwrap(), None, 0, cx);
                    });
                }
            })
            .unwrap();
        let bounds = draw(window.into(), cx);
        let canvas = bounds["split-canvas"];
        let initial = bounds[LEFT_PANE].right();
        let track = window
            .update(cx, |view, _, cx| {
                view.terminals[A]
                    .read(cx)
                    .scrollbar_track_center_for_split_probe()
            })
            .unwrap();
        let move_at = |cx: &mut AsyncApp, position, pressed_button| {
            dispatch_mouse(
                window,
                cx,
                MouseMoveEvent {
                    position,
                    pressed_button,
                    ..Default::default()
                },
            );
        };
        move_at(cx, track, None);
        draw(window.into(), cx);
        window
            .update(cx, |view, _, cx| {
                assert!(view.terminals[A].read(cx).scrollbar_hit(track));
            })
            .unwrap();

        let assert_terminal_isolated = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, cx| {
                    for (id, hovered) in [(A, true), (RIGHT, false)] {
                        let surface = view.terminals[id].read(cx);
                        surface.assert_no_terminal_drag_for_split_probe();
                        // A terminal receiving the drag in capture would update its hover.
                        assert_eq!(surface.scrollport_hovered_for_split_probe(), hovered);
                    }
                })
                .unwrap();
        };
        let assert_divider_at = |cx: &mut AsyncApp, expected: Pixels| {
            let painted = draw(window.into(), cx);
            assert_eq!(painted["split-canvas"], canvas);
            let actual = painted[LEFT_PANE].right();
            assert_eq!(actual, painted[RIGHT_PANE].left());
            assert!(
                (actual - expected).abs() <= px(1.),
                "divider was at {actual:?}, expected {expected:?}"
            );
            actual
        };
        let start_drag = |cx: &mut AsyncApp, x, button_state| {
            with_split_drag_button_for_probe(button_state, || {
                dispatch_mouse(
                    window,
                    cx,
                    mouse_down(point(x - px(1.), track.y), Modifiers::default()),
                );
            });
        };
        let y = track.y;
        let left = Some(MouseButton::Left);
        while sink.try_recv().is_ok() {}
        for (origin, sign) in [(initial, 1.), (initial + px(180.), -1.)] {
            start_drag(cx, origin, Some(RELEASE_COUNT));
            let mut previous = origin;
            for step in 1..=35 {
                let x = origin + px(sign * 180. * step as f32 / 35.);
                move_at(cx, point(x, y), left);
                let actual = assert_divider_at(cx, x);
                assert!(
                    (actual - previous) * sign > px(0.),
                    "stalled at step {step}"
                );
                assert_terminal_isolated(cx);
                previous = actual;

                if sign < 0. && step == 13 {
                    with_split_drag_button_for_probe(Some(RELEASE_COUNT), || {
                        move_at(cx, point(x + px(17.), y), None);
                    });
                    assert_eq!(assert_divider_at(cx, x), actual);
                    assert_terminal_isolated(cx);
                }
            }
            dispatch_mouse(window, cx, mouse_up(point(previous, y)));
            move_at(cx, point(previous + px(30.), y), left);
            assert_eq!(assert_divider_at(cx, previous), previous);
            // Restore the pre-gesture hover using an ordinary move.
            let track = window
                .update(cx, |view, _, cx| {
                    view.terminals[A]
                        .read(cx)
                        .scrollbar_track_center_for_split_probe()
                })
                .unwrap();
            move_at(cx, track, None);
            assert_terminal_isolated(cx);
        }

        for (at_down, at_move) in [
            (None, None),
            (Some(RELEASE_COUNT), None),
            (Some(RELEASE_COUNT), Some(RELEASE_COUNT + 1)),
        ] {
            start_drag(cx, initial, at_down);
            with_split_drag_button_for_probe(at_move, || move_at(cx, point(initial, y), None));
            move_at(cx, point(initial + px(30.), y), left);
            assert_divider_at(cx, initial);
        }

        start_drag(cx, initial, Some(RELEASE_COUNT));
        with_split_drag_button_for_probe(None, || {
            let mut release = mouse_up(point(initial, y));
            release.modifiers.control = true;
            dispatch_mouse(window, cx, release);
        });
        move_at(cx, point(initial + px(30.), y), left);
        assert_divider_at(cx, initial);

        assert!(
            !std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(
                command,
                Command::Select { .. }
                    | Command::Scroll { .. }
                    | Command::ScrollTo { .. }
                    | Command::ScrollSelection { .. }
            )),
            "divider gesture generated terminal selection or scrolling"
        );
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "horizontal divider: 35 painted steps each way (±180 pt), scrollback terminals bypassed in capture, buttonless move recovered after repaint, releases ended the drag"
        );
    }

    fn scroll_to_zoom(window: AnyWindowHandle, cx: &mut AsyncApp, position: Point<Pixels>) {
        use cocoa::{appkit::NSEvent, base::nil};
        use core_foundation::base::CFRelease;
        use core_graphics::{
            display::CGDisplay,
            event::{CGEventFlags, CGScrollEventUnit, ScrollEventUnit},
            geometry::CGPoint,
        };
        use objc::{msg_send, sel, sel_impl};
        use std::ffi::c_void;

        unsafe extern "C" {
            fn CGEventCreateScrollWheelEvent2(
                source: *const c_void,
                units: CGScrollEventUnit,
                wheel_count: u32,
                wheel1: i32,
                wheel2: i32,
                wheel3: i32,
            ) -> *mut c_void;
            fn CGEventSetLocation(event: *mut c_void, location: CGPoint);
            fn CGEventSetFlags(event: *mut c_void, flags: CGEventFlags);
        }
        let (view, size) = native_view(window, cx);
        unsafe {
            let event = CGEventCreateScrollWheelEvent2(
                std::ptr::null(),
                ScrollEventUnit::PIXEL,
                1,
                32,
                0,
                0,
            );
            assert!(!event.is_null());
            CGEventSetLocation(
                event,
                CGPoint::new(
                    f64::from(position.x),
                    CGDisplay::main().bounds().size.height - f64::from(size.height - position.y),
                ),
            );
            CGEventSetFlags(event, CGEventFlags::CGEventFlagCommand);
            let native = NSEvent::eventWithCGEvent_(nil, event);
            CFRelease(event);
            assert_eq!(native.locationInWindow().x, f64::from(position.x));
            assert_eq!(
                native.locationInWindow().y,
                f64::from(size.height - position.y)
            );
            let _: () = msg_send![view, scrollWheel: native];
        }
    }

    fn alt_tap(window: AnyWindowHandle, cx: &mut AsyncApp) {
        use cocoa::{
            appkit::{NSEvent, NSEventModifierFlags, NSEventType},
            base::{NO, id, nil},
            foundation::{NSInteger, NSPoint, NSString},
        };
        use objc::{msg_send, sel, sel_impl};
        let (view, _) = native_view(window, cx);
        unsafe {
            let native: id = msg_send![view, window];
            let number: NSInteger = msg_send![native, windowNumber];
            let empty = NSString::alloc(nil).init_str("");
            for flags in [
                NSEventModifierFlags::NSAlternateKeyMask,
                NSEventModifierFlags::empty(),
            ] {
                let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode_(
                    nil, NSEventType::NSFlagsChanged, NSPoint::new(0., 0.), flags, 0., number, nil,
                    empty, empty, NO, 58,
                );
                assert_ne!(event, nil);
                let _: () = msg_send![view, flagsChanged: event];
            }
            let _: () = msg_send![empty, release];
        }
    }

    async fn verify_carousel_focus(cx: &mut AsyncApp) {
        use crate::carousel_motion::WidthRule;

        let (window, sink) = open_probe_at(cx, 1260.);
        let publish_focus = |cx: &mut AsyncApp, id: &str| {
            window
                .update(cx, |view, window, cx| {
                    let mut hierarchy = view.hierarchy.clone().unwrap();
                    hierarchy.session.as_mut().unwrap().id = id.to_owned();
                    let mut graph = view.graph.clone();
                    graph["nodes"][2]["currentMode"] = json!("shell");
                    if graph["nodes"].as_array().unwrap().len() == 3 {
                        // Keep room beyond the preview so the old card can
                        // shrink without clamping the saved viewport position.
                        graph["nodes"].as_array_mut().unwrap().push(json!({
                            "sessionId": "carousel-tail", "currentMode": "shell",
                            "workStatus": "idle", "title": "Tail"
                        }));
                    }
                    graph["focusedSessionId"] = json!(id);
                    view.apply(
                        BridgeEvent::Runtime {
                            generation: 1,
                            event: Event::Navigation {
                                hierarchy,
                                scenes: view.scenes.clone(),
                                graph,
                                geometry: vec![],
                            },
                        },
                        window,
                        cx,
                    );
                })
                .unwrap();
        };
        publish_focus(cx, A);
        draw(window.into(), cx);
        let move_at = |cx: &mut AsyncApp, position| {
            dispatch_mouse(
                window,
                cx,
                MouseMoveEvent {
                    position,
                    ..Default::default()
                },
            );
        };
        let offset = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, _| view.carousel_scroll.offset().x)
                .unwrap()
        };
        let hover = |cx: &mut AsyncApp, expected: Option<&str>| {
            window
                .update(cx, |view, _, _| {
                    assert_eq!(view.hovered.as_deref(), expected)
                })
                .unwrap();
        };
        let width_rule = |cx: &mut AsyncApp, id: &str| {
            window
                .update(cx, |view, _, _| {
                    view.card_motion[id].width.as_ref().unwrap().target_rule()
                })
                .unwrap()
        };
        let initial = offset(cx);
        let base_width = card_bounds(window, cx, B).size.width;
        let b = card_header(window, cx, B);
        move_at(cx, b);
        hover(cx, Some(B));
        draw(window.into(), cx);
        assert!(width_rule(cx, B) == WidthRule::WideExpanded);
        cx.background_executor()
            .timer(Duration::from_millis(400))
            .await;
        draw(window.into(), cx);
        let b = card_bounds(window, cx, B);
        assert!(b.size.width > base_width + px(20.));
        let a = card_bounds(window, cx, A);
        move_at(cx, point((a.right() + b.left()) / 2., b.center().y));
        hover(cx, Some(B));
        let viewport = window
            .update(cx, |view, _, _| view.carousel_scroll.bounds())
            .unwrap();
        move_at(cx, point(b.center().x, viewport.top() + px(1.)));
        hover(cx, None);
        draw(window.into(), cx);
        assert!(width_rule(cx, B) == WidthRule::Columns(3));
        assert_eq!(offset(cx), initial);
        cx.background_executor()
            .timer(Duration::from_millis(400))
            .await;
        draw(window.into(), cx);
        assert!((card_bounds(window, cx, B).size.width - base_width).abs() <= px(1.));

        let third = card_header(window, cx, TEAM);
        move_at(cx, third);
        hover(cx, Some(TEAM));
        for _ in 0..20 {
            draw(window.into(), cx);
            cx.background_executor()
                .timer(Duration::from_millis(22))
                .await;
        }
        draw(window.into(), cx);
        let preview_scroll = offset(cx);
        assert!(preview_scroll < initial - px(20.));
        let click_at = card_header(window, cx, TEAM);
        dispatch_mouse(window, cx, mouse_down(click_at, Modifiers::default()));
        window
            .update(cx, |view, _, _| {
                assert_eq!(view.focused.as_deref(), Some(A));
                assert_eq!(view.level_focus(), Some(TEAM));
                assert!(view.hovered.is_none());
                assert!(view.hover_baseline.is_none());
                assert!(view.hover_restore_at.is_none());
                assert!(view.hover_follow_until.is_none());
                assert!(view.focus_follow_until.is_none());
                let geometry = &view.geometry[view.carousel_owner.as_ref().unwrap()].geometry;
                assert_eq!(geometry["anchorSessionId"], TEAM);
                assert_eq!(geometry["scrollLeft"], json!(-f32::from(preview_scroll)));
            })
            .unwrap();
        draw(window.into(), cx);
        assert!(width_rule(cx, A) == WidthRule::Columns(3));
        assert!(width_rule(cx, B) == WidthRule::Columns(3));
        assert!(width_rule(cx, TEAM) == WidthRule::WideExpanded);
        assert_eq!(offset(cx), preview_scroll);
        dispatch_mouse(window, cx, mouse_up(click_at));
        publish_focus(cx, TEAM);
        draw(window.into(), cx);
        assert_eq!(offset(cx), preview_scroll);
        window
            .update(cx, |view, _, _| assert!(view.focus_follow_until.is_none()))
            .unwrap();

        while sink.try_recv().is_ok() {}
        press(window, cx, "cmd-[");
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command| {
            matches!(command, Command::FocusSession { session_id, .. } if session_id == B)
        }));
        publish_focus(cx, B);
        let before = offset(cx);
        draw(window.into(), cx);
        let first_step = (offset(cx) - before).abs();
        assert!(first_step > px(0.) && first_step <= px(28.));
        window
            .update(cx, |view, _, _| {
                assert_eq!(view.level_focus(), Some(B));
                assert!(view.focus_follow_until.is_some());
            })
            .unwrap();
        let grab = point(viewport.center().x, viewport.top() + px(1.));
        let dragged = grab + point(px(-36.), px(0.));
        let before_drag = offset(cx);
        dispatch_mouse(window, cx, mouse_down(grab, Modifiers::default()));
        dispatch_mouse(
            window,
            cx,
            MouseMoveEvent {
                position: dragged,
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            },
        );
        dispatch_mouse(window, cx, mouse_up(dragged));
        window
            .update(cx, |view, _, _| {
                assert!(view.focus_follow_until.is_none());
                assert!(view.hover_follow_until.is_none());
                assert!(view.hover_baseline.is_none());
                assert!(view.hover_restore_at.is_none());
                assert!(view.edge_next_at.is_none());
            })
            .unwrap();
        assert_eq!(offset(cx), before_drag - px(36.));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "carousel focus: gap, blank, optimistic preview activation, saved scroll, keyboard follow, and manual drag"
        );
    }

    async fn verify_carousel_and_zoom(cx: &mut AsyncApp, root: &std::path::Path) {
        let (window, sink) = open_probe_at(cx, 1260.);
        let toolbar = draw(window.into(), cx)["create-shell"].center();
        click(window.into(), cx, MouseButton::Left, toolbar);
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command|
            matches!(command, Command::CreateShellSibling { scene_id, source_session_id, parent_session_id: None }
                if scene_id == "main" && source_session_id == A)));
        publish_shell(window, cx, "new-float");
        let bounds = draw(window.into(), cx);
        assert!(!bounds.contains_key("split-canvas"));
        assert!(card_bounds(window, cx, "new-float").size.width > px(0.));
        window
            .update(cx, |view, window, cx| {
                view.data_root = Some(root.to_owned());
                window.focus(&view.terminals["new-float"].read(cx).focus);
            })
            .unwrap();
        let metrics = |cx: &mut AsyncApp| {
            draw(window.into(), cx);
            window
                .update(cx, |view, _, cx| {
                    view.terminals["new-float"]
                        .read(cx)
                        .target_dimensions()
                        .unwrap()
                })
                .unwrap()
        };
        let initial = metrics(cx);
        press(window, cx, "cmd-=");
        let zoomed = metrics(cx);
        assert!(
            zoomed.0 < initial.0 && zoomed.1 < initial.1,
            "{initial:?} -> {zoomed:?}"
        );
        assert_eq!(read_terminal_font_size(root), 12);
        let mut minus = Keystroke::parse("cmd-x").unwrap();
        minus.key = "-".into();
        let native_window: AnyWindowHandle = window.into();
        native_window
            .update(cx, |_, window, cx| window.dispatch_keystroke(minus, cx))
            .unwrap();
        assert_eq!(metrics(cx), initial);
        press(window, cx, "cmd-shift-+");
        assert_eq!(metrics(cx), zoomed);
        press(window, cx, "cmd-shift-_");
        assert_eq!(metrics(cx), initial);
        let card = card_bounds(window, cx, "new-float");
        let visible_left = card.left().max(px(262.));
        let visible_right = card.right().min(px(1258.));
        assert!(
            visible_right - visible_left > px(20.),
            "floating card must intersect the viewport: {card:?}"
        );
        let position = point((visible_left + visible_right) / 2., card.center().y);
        dispatch_mouse(
            window,
            cx,
            MouseMoveEvent {
                position,
                modifiers: Modifiers::default(),
                pressed_button: None,
            },
        );
        scroll_to_zoom(native_window, cx, position);
        assert_eq!(metrics(cx), zoomed);
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(
            |command| matches!(command, Command::Resize { session_id, cols, rows, .. }
                if session_id == "new-float" && cols == zoomed.0 && rows == zoomed.1)
        ));
        press(window, cx, "cmd-0");
        assert_eq!(metrics(cx), initial);
        assert_eq!(read_terminal_font_size(root), 11);
        alt_tap(native_window, cx);
        alt_tap(native_window, cx);
        overlay(window, cx, Some("shortcuts"));
        press(window, cx, "escape");
        overlay(window, cx, None);
        press(window, cx, "cmd-shift-?");
        overlay(window, cx, Some("shortcuts"));
        press(window, cx, "escape");
        std::iter::from_fn(|| sink.try_recv().ok()).for_each(drop);
        press(window, cx, "cmd-w");
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command|
            matches!(command, Command::CloseSessionCard { session_id, .. } if session_id == "new-float")));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "carousel: toolbar creates a floating sibling; Mod+W closes it; Alt double-tap, Mod+?, font keys and wheel update the hidden terminal"
        );
    }

    fn verify_splits(cx: &mut AsyncApp) {
        let (window, sink) = open_probe_at(cx, 1260.);
        const RIGHT: &str = "new-right";
        const BOTTOM: &str = "new-bottom";
        assert!(!draw(window.into(), cx).contains_key("split-canvas"));
        card_action(window, cx, A, CardMenuAction::SplitHorizontal);
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command|
            matches!(command, Command::CreateShellSibling { scene_id, source_session_id, parent_session_id: None }
                if scene_id == "main" && source_session_id == A)));
        publish_shell(window, cx, RIGHT);
        let bounds = draw(window.into(), cx);
        assert_eq!(
            bounds["split-pane-shell-a"].right(),
            bounds["split-pane-new-right"].left()
        );
        let header = bounds["split-pane-new-right"].origin + point(px(32.), px(20.));
        click(window.into(), cx, MouseButton::Right, header);
        overlay(window, cx, Some("card menu"));
        window
            .update(cx, |view, _, cx| {
                view.select_card_menu_action_for_probe(CardMenuAction::SplitVertical, cx);
            })
            .unwrap();
        draw(window.into(), cx);
        press(window, cx, "enter");
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command|
            matches!(command, Command::CreateShellSibling { source_session_id, .. } if source_session_id == RIGHT)));
        publish_shell(window, cx, BOTTOM);
        let bounds = draw(window.into(), cx);
        assert_eq!(
            bounds["split-pane-new-right"].bottom(),
            bounds["split-pane-new-bottom"].top()
        );
        assert_eq!(
            bounds["split-pane-new-right"].left(),
            bounds["split-pane-new-bottom"].left()
        );
        assert!(!bounds.contains_key("split-pane-shell-b"));

        for (theme, locale, label) in [
            (ThemePreference::Dark, InterfaceLocale::Chinese, "关闭分屏"),
            (
                ThemePreference::Light,
                InterfaceLocale::English,
                "Close pane",
            ),
        ] {
            window
                .update(cx, |view, _, cx| {
                    view.locale = locale;
                    view.theme.set_preference(theme);
                    view.apply_theme_palette(cx);
                    assert!(
                        view.wiring_card_menu(RIGHT)
                            .items
                            .iter()
                            .any(|item| item.action == CardMenuAction::ClosePane
                                && item.label == label)
                    );
                })
                .unwrap();
            assert_eq!(
                draw(window.into(), cx)["split-pane-new-right"],
                bounds["split-pane-new-right"]
            );
        }

        // The terminal moves into the child while its placeholder keeps the split allocation.
        click(
            window.into(),
            cx,
            MouseButton::Right,
            bounds["split-pane-shell-a"].origin + point(px(32.), px(20.)),
        );
        overlay(window, cx, Some("card menu"));
        window
            .update(cx, |view, _, cx| {
                view.select_card_menu_action_for_probe(CardMenuAction::OpenDetached, cx)
            })
            .unwrap();
        draw(window.into(), cx);
        press(window, cx, "enter");
        let child = window
            .update(cx, |view, _, _| view.wiring_child_for_probe(A).unwrap())
            .unwrap();
        draw(child, cx);
        assert!(draw(window.into(), cx).contains_key("detached-mount-shell-a"));
        child
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        assert!(!draw(window.into(), cx).contains_key("detached-mount-shell-a"));

        for (node, pane, horizontal) in [
            ("new-right", "split-pane-shell-a", true),
            ("new-bottom", "split-pane-new-right", false),
        ] {
            let bounds = draw(window.into(), cx);
            let first = bounds[pane];
            let press_at = if horizontal {
                point(first.right() - px(1.), first.center().y)
            } else {
                point(first.center().x, first.bottom() - px(1.))
            };
            dispatch_mouse(window, cx, mouse_down(press_at, Modifiers::default()));
            for (offset, expected) in [(-20., 0.1), (20., 0.9)] {
                let canvas = bounds["split-canvas"];
                let position = if horizontal {
                    point(
                        if offset < 0. {
                            canvas.left()
                        } else {
                            canvas.right()
                        } + px(offset),
                        press_at.y,
                    )
                } else {
                    point(
                        press_at.x,
                        if offset < 0. {
                            canvas.top()
                        } else {
                            canvas.bottom()
                        } + px(offset),
                    )
                };
                dispatch_mouse(
                    window,
                    cx,
                    MouseMoveEvent {
                        position,
                        pressed_button: Some(MouseButton::Left),
                        ..Default::default()
                    },
                );
                window
                    .update(cx, |view, _, _| {
                        assert_eq!(
                            view.active_split_tree()
                                .unwrap()
                                .ratio(&format!("split:{node}")),
                            Some(expected)
                        );
                    })
                    .unwrap();
                let painted = draw(window.into(), cx);
                let size = if horizontal {
                    painted[pane].size.width / painted["split-canvas"].size.width
                } else {
                    painted[pane].size.height / painted["split-canvas"].size.height
                };
                assert!((f64::from(size) - expected).abs() < 0.004, "{node} {size}");
            }
            dispatch_mouse(window, cx, mouse_up(press_at));
        }
        println!(
            "split tree: mixed panes painted in light and dark; both dividers clamp at 10% and 90%; detached pane keeps its slot"
        );

        let focus = |cx: &mut AsyncApp, shortcut: &str, session: &str| {
            draw(window.into(), cx);
            press(window, cx, shortcut);
            window
                .update(cx, |view, window, cx| {
                    assert_eq!(view.active_level_focus(), Some(session));
                    assert!(view.terminals[session].read(cx).focus.is_focused(window));
                })
                .unwrap();
        };
        focus(cx, "cmd-alt-up", RIGHT);
        focus(cx, "cmd-[", A);
        focus(cx, "cmd-]", RIGHT);
        focus(cx, "cmd-]", BOTTOM);
        press(window, cx, "cmd-w");
        let bounds = draw(window.into(), cx);
        assert!(!bounds.contains_key("split-pane-new-bottom"));
        assert_eq!(
            bounds["split-pane-new-right"].size.height,
            bounds["split-pane-shell-a"].size.height
        );
        press(window, cx, "cmd-w");
        assert!(!draw(window.into(), cx).contains_key("split-canvas"));
        assert!(card_bounds(window, cx, A).size.width > px(0.));
        window
            .update(cx, |view, window, _| {
                assert!(view.graph_node(BOTTOM).is_some());
                assert!(view.graph_node(RIGHT).is_some());
                window.remove_window();
            })
            .unwrap();
        assert!(
            std::iter::from_fn(|| sink.try_recv().ok())
                .all(|command| !matches!(command, Command::CloseSessionCard { .. }))
        );
        println!(
            "split focus and close: directional and ordered keyboard movement, branch collapse, carousel restored, sessions preserved"
        );
    }

    #[derive(Clone)]
    struct PromptRecord {
        title: String,
        detail: Option<String>,
        buttons: Vec<PromptButton>,
    }

    struct ProbePrompt {
        focus: FocusHandle,
        record: PromptRecord,
    }

    impl EventEmitter<PromptResponse> for ProbePrompt {}
    impl Focusable for ProbePrompt {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus.clone()
        }
    }
    impl Render for ProbePrompt {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .occlude()
                .track_focus(&self.focus)
                .child(
                    div()
                        .absolute()
                        .left(px(350.))
                        .top(px(30.))
                        .child(self.record.title.clone())
                        .children(self.record.detail.clone()),
                )
                .child(
                    div().absolute().left(px(30.)).top(px(30.)).children(
                        self.record
                            .buttons
                            .iter()
                            .enumerate()
                            .map(|(index, label)| {
                                div()
                                    .id(index)
                                    .w(px(300.))
                                    .h(px(35.))
                                    .child(label.label().clone())
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(PromptResponse(index))
                                    }))
                            }),
                    ),
                )
        }
    }

    async fn verify_links(cx: &mut AsyncApp) {
        use matou_native_terminal::{Terminal, TerminalOptions};
        use std::cell::RefCell;
        const HTTPS: &str = "https://ip.net.coffee/";
        const LOGIN: &str = "https://auth.openai.com/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback";
        const KEY_PAGE: &str = "https://platform.deepseek.com/api_keys";
        let (window, _) = open_probe_at(cx, 1260.);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let clips = Rc::new(RefCell::new(Vec::new()));
        let prompts = Rc::new(RefCell::new(Vec::<PromptRecord>::new()));
        let prompt_feed = prompts.clone();
        cx.update(|cx| {
            cx.set_prompt_builder(move |_, title, detail, buttons, handle, window, cx| {
                let record = PromptRecord {
                    title: title.into(),
                    detail: detail.map(str::to_owned),
                    buttons: buttons.to_vec(),
                };
                prompt_feed.borrow_mut().push(record.clone());
                let prompt = cx.new(|cx| ProbePrompt {
                    focus: cx.focus_handle(),
                    record,
                });
                handle.with_view(prompt, window, cx)
            })
        })
        .unwrap();
        window
            .update(cx, |view, _, _| {
                let (calls, clips) = (calls.clone(), clips.clone());
                view.mock_external_links_for_probe(
                    move |url, target, _| {
                        calls.borrow_mut().push((url.to_owned(), target));
                        gpui::Task::ready(match (url, target) {
                            (LOGIN, LinkOpenTarget::Controlled) => {
                                Err(ControlledBrowserFailure::LaunchFailed)
                            }
                            (KEY_PAGE, LinkOpenTarget::Controlled) => {
                                Err(ControlledBrowserFailure::Missing)
                            }
                            _ => Ok(()),
                        })
                    },
                    move |url, _| clips.borrow_mut().push(url.to_owned()),
                );
            })
            .unwrap();

        let terminal_click = |cx: &mut AsyncApp, value: &str, col: usize, row: usize| {
            let mut terminal = Terminal::new(TerminalOptions::new(44, 8, 9, 18)).unwrap();
            terminal.feed(value.as_bytes());
            window
                .update(cx, |view, _, cx| {
                    view.terminals[A].update(cx, |surface, cx| {
                        surface.set_screen(
                            terminal.screen().unwrap(),
                            terminal.input_modes().unwrap(),
                            None,
                            0,
                            cx,
                        );
                    });
                })
                .unwrap();
            draw(window.into(), cx);
            let hit = window
                .update(cx, |view, _, cx| {
                    view.terminals[A]
                        .read(cx)
                        .cell_center_for_link_probe(col, row)
                })
                .unwrap();
            dispatch_mouse(
                window,
                cx,
                mouse_down(
                    hit,
                    Modifiers {
                        platform: true,
                        ..Default::default()
                    },
                ),
            );
            dispatch_mouse(window, cx, mouse_up(hit));
        };
        terminal_click(cx, HTTPS, 4, 0);
        assert_eq!(
            *calls.borrow(),
            [(HTTPS.into(), LinkOpenTarget::Controlled)]
        );
        for locale in [InterfaceLocale::Chinese, InterfaceLocale::English] {
            window
                .update(cx, |view, _, _| view.locale = locale)
                .unwrap();
            for url in [
                "file:///tmp/file",
                "javascript:alert(1)",
                "data:text/plain,hi",
                "cmd:run",
                "http://ip.net.coffee/",
                "https://other.invalid/",
                "https://",
                "https://user@ip.net.coffee/",
            ] {
                terminal_click(cx, url, 3, 0);
                window
                    .update(cx, |view, _, _| {
                        assert_eq!(
                            view.task_toast.as_deref(),
                            Some(LinkRejection::Malformed.message(locale))
                        );
                    })
                    .unwrap();
            }
        }
        assert_eq!(calls.borrow().len(), 1);
        assert!(prompts.borrow().is_empty());
        wait_for_link(cx, window, false).await;

        let choose = |cx: &mut AsyncApp, index: usize| {
            draw(window.into(), cx);
            let position = point(px(50.), px(45. + 35. * index as f32));
            dispatch_mouse(window, cx, mouse_down(position, Modifiers::default()));
            dispatch_mouse(window, cx, mouse_up(position));
        };
        for button in [1, 0] {
            // Click a wrapped row of the OAuth URL, through the actual terminal handler.
            terminal_click(cx, LOGIN, 10, 1);
            for _ in 0..100 {
                if prompts.borrow().len() >= if button == 1 { 1 } else { 2 } {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;
            }
            let prompt = prompts.borrow().last().unwrap().clone();
            assert_eq!(prompt.title, "Mantle could not open");
            assert_eq!(
                prompt.detail.as_deref(),
                Some("Mantle could not open this page.")
            );
            assert_eq!(prompt.buttons[0].label().as_ref(), "Open in system browser");
            assert!(matches!(prompt.buttons[1], PromptButton::Cancel(_)));
            assert!(
                calls
                    .borrow()
                    .iter()
                    .all(|(_, target)| *target != LinkOpenTarget::System)
            );
            choose(cx, button);
            wait_for_link(cx, window, false).await;
        }
        assert_eq!(
            calls.borrow().last().unwrap(),
            &(LOGIN.into(), LinkOpenTarget::System)
        );
        window
            .update(cx, |view, window, cx| {
                view.toggle_settings(window, cx);
                view.settings
                    .as_ref()
                    .unwrap()
                    .0
                    .clone()
                    .update(cx, |settings, cx| {
                        settings.focus_provider_key_link_for_probe(window, cx);
                    });
            })
            .unwrap();
        draw(window.into(), cx);
        press(window, cx, "enter");
        for _ in 0..100 {
            if prompts.borrow().len() == 3 {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
        let fallback = prompts.borrow().last().unwrap().clone();
        assert_eq!(fallback.title, format!("Open in your browser: {KEY_PAGE}"));
        assert_eq!(fallback.buttons[0].label().as_ref(), "Copy link");
        assert!(clips.borrow().is_empty());
        choose(cx, 0);
        wait_for_link(cx, window, false).await;
        assert_eq!(*clips.borrow(), [KEY_PAGE]);
        assert_eq!(calls.borrow().len(), 5);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.update(|cx| cx.reset_prompt_builder()).unwrap();
        println!(
            "links: trusted HTTPS, bilingual rejection, wrapped OAuth confirmation/cancel/explicit system open, settings key-page copy; opener and clipboard mocked"
        );
    }

    async fn wait_for_link(cx: &mut AsyncApp, window: Handle, pending: bool) {
        for _ in 0..100 {
            if window
                .update(cx, |view, _, _| view.link_in_progress_for_probe())
                .unwrap()
                == pending
            {
                return;
            }
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
        panic!("link did not reach pending={pending}");
    }

    fn draw_hud(
        window: AnyWindowHandle,
        cx: &mut AsyncApp,
    ) -> HashMap<&'static str, Bounds<Pixels>> {
        FOOTER_READINGS.with_borrow_mut(|slot| *slot = Some(FooterReadings::default()));
        draw(window, cx);
        FOOTER_READINGS.with_borrow_mut(|slot| {
            let readings = slot.take().unwrap();
            std::mem::take(&mut *readings.bounds.borrow_mut())
        })
    }

    fn hud_text(window: AnyWindowHandle, cx: &mut AsyncApp, key: &'static str) -> Option<String> {
        FOOTER_READINGS.with_borrow_mut(|slot| *slot = Some(FooterReadings::default()));
        draw(window, cx);
        FOOTER_READINGS.with_borrow_mut(|slot| {
            let readings = slot.take().unwrap();
            readings
                .text
                .borrow_mut()
                .remove(key)
                .map(|(text, _)| text.to_string())
        })
    }

    async fn wait_for_hud_element(
        window: AnyWindowHandle,
        cx: &mut AsyncApp,
        element: &'static str,
    ) -> Bounds<Pixels> {
        for _ in 0..100 {
            if let Some(bounds) = draw_hud(window, cx).get(element) {
                return *bounds;
            }
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
        panic!("HUD did not render {element}");
    }

    fn verify_card_headers(cx: &mut AsyncApp) {
        let (window, sink) = open_probe_at(cx, 1200.);
        let handle = window.into();
        let actions = |id: &str| {
            [
                format!("session-history-open-{id}"),
                format!("session-loader-open-{id}"),
                format!("fork-child-{id}"),
                format!("fork-sibling-{id}"),
                format!("remove-session-{id}"),
            ]
        };
        let count = |bounds: &HashMap<String, Bounds<Pixels>>, id: &str| {
            actions(id)
                .iter()
                .filter(|id| bounds.contains_key(*id))
                .count()
        };

        let shell = draw(handle, cx);
        assert_eq!(count(&shell, A), 2);
        let scene_bar = shell["scene-bar"];
        assert_eq!(scene_bar.size.height, px(48.));
        assert_eq!(shell["session-header-shell-a"].size.height, px(42.));
        assert_eq!(
            shell["session-header-shell-a"].top() - scene_bar.bottom(),
            px(6.)
        );
        assert_eq!(
            shell["session-header-shell-b"].top() - scene_bar.bottom(),
            px(10.)
        );
        window
            .update(cx, |view, _, cx| {
                view.graph["nodes"].as_array_mut().unwrap().truncate(1);
                view.refresh_level(cx);
                cx.notify();
            })
            .unwrap();
        assert_eq!(count(&draw(handle, cx), A), 1);

        for provider in ["claude-code", "codex", "antigravity"] {
            window
                .update(cx, |view, window, cx| {
                    let mut graph = graph();
                    for node in graph["nodes"].as_array_mut().unwrap().iter_mut().take(2) {
                        node["currentMode"] = json!(provider);
                        node["canFork"] = json!(true);
                        node["cwd"] = json!(".");
                    }
                    graph["nodes"][1]["parentSessionId"] = json!(A);
                    view.level_parent_by_scene.remove("main");
                    view.focused = Some(A.into());
                    view.replace_fork_graph(graph, window, cx);
                    view.refresh_terminals(window, cx);
                    cx.notify();
                })
                .unwrap();
            let root = draw(handle, cx);
            assert_eq!(count(&root, A), 4, "{provider} root");
            click(handle, cx, MouseButton::Left, root[&actions(A)[0]].center());
            window
                .update(cx, |view, window, cx| {
                    let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
                        panic!("{provider} history did not open");
                    };
                    assert_eq!(loader.provider, provider);
                    view.close_dialog(window, cx);
                })
                .unwrap();
            if provider != "claude-code" {
                assert!(
                    std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(
                        (provider, command),
                        ("codex", Command::ListCodexSessions { .. })
                            | ("antigravity", Command::ListAntigravitySessions { .. }),
                    ))
                );
            }
            let root = draw(handle, cx);
            click(handle, cx, MouseButton::Left, root[&actions(A)[2]].center());
            window.update(cx, |view, window, cx| {
                assert!(matches!(
                    &view.dialog,
                    Some(TabDialog::Branch(state)) if state.placement == ForkPlacement::Child
                ), "{provider} child fork");
                view.close_dialog(window, cx);
                view.level_parent_by_scene.insert("main".into(), Some(A.into()));
                view.focused = Some(B.into());
                view.refresh_level(cx);
                cx.notify();
            }).unwrap();
            let child = draw(handle, cx);
            assert_eq!(count(&child, B), 5, "{provider} child");
            click(
                handle,
                cx,
                MouseButton::Left,
                child[&actions(B)[3]].center(),
            );
            window.update(cx, |view, window, cx| {
                assert!(matches!(
                    &view.dialog,
                    Some(TabDialog::Branch(state)) if state.placement == ForkPlacement::Sibling
                ), "{provider} sibling fork");
                view.close_dialog(window, cx);
            }).unwrap();
        }
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "card headers: provider history and fork buttons, four root and five child actions, one/two Shell actions"
        );
    }

    async fn verify_footer_hud(cx: &mut AsyncApp, root: &std::path::Path) {
        let cwd = root.join("footer-hud");
        std::fs::create_dir(&cwd).unwrap();
        let instruction_file = cwd.join("GEMINI.md");
        std::fs::write(&instruction_file, "# Project rules\n").unwrap();
        let (window, sink) = open_probe_at(cx, 1420.);
        assert!(draw(window.into(), cx).contains_key("remove-session-shell-a"));
        window
            .update(cx, |view, _, cx| {
                view.graph["nodes"].as_array_mut().unwrap().truncate(1);
                cx.notify();
            })
            .unwrap();
        assert!(!draw(window.into(), cx).contains_key("remove-session-shell-a"));
        window
            .update(cx, |view, _, cx| {
                view.graph = graph();
                cx.notify();
            })
            .unwrap();
        let live = json!({
            "model":"Long full model name","contextWindowSize":200000,"permissionMode":"ask","resumable":true,
            "usageWindows":[{"id":"current","label":"Current model","usedPercent":91,"resetText":"12:00"}],
            "allUsageWindows":[{"id":"group","label":"All models","usedPercent":77,"resetText":"13:00"}],
            "runningTools":[{"id":"read","name":"Read","target":"notes.md"}],
            "toolCounts":[{"name":"Find","count":2}],
            "agents":[{"name":"Reader","status":"running","detail":"Indexing","startedAt":1700000000000_f64}],
            "configCounts":{"mcps":2,"hooks":1},"hookNames":["PostToolUse"],"mcpErrors":["server offline"],
            "todos":[{"content":"Check results","status":"in_progress"}],
            "gitBranch":"review/live","gitDirty":true
        });
        window
            .update(cx, |view, window, cx| {
                let node = &mut view.graph["nodes"][0];
                node["currentMode"] = json!("antigravity");
                node["cwd"] = json!(cwd);
                node["environment"] = json!({"kind":"worktree","state":"ready","path":cwd});
                node["git"] = json!({"state":"ready","branch":"legacy","dirty":false});
                view.refresh_terminals(window, cx);
                view.refresh_live_wiring(cx);
                cx.notify();
            })
            .unwrap();
        let request_id = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GetAgentHud {
                    request_id,
                    session_id,
                } if session_id == A => Some(request_id),
                _ => None,
            })
            .expect("focused agent HUD request");
        window
            .update(cx, |view, window, cx| {
                view.receive_wiring_reply(
                    "agentHudResult",
                    request_id,
                    &json!({"result":{"hud":live}}),
                    window,
                    cx,
                )
            })
            .unwrap();
        wait_for_hud_element(window.into(), cx, "instructions").await;
        let bounds = draw_hud(window.into(), cx);
        for key in [
            "permission",
            "name",
            "context",
            "mcps",
            "tools",
            "agents",
            "hooks",
            "quota",
            "mcp-errors",
            "todos",
            "git",
            "environment",
            "egress",
        ] {
            assert!(
                bounds.contains_key(key),
                "missing painted footer cell {key}"
            );
        }
        assert!(!bounds.contains_key("account"));
        window
            .update(cx, |view, _, _| {
                assert_eq!(view.permission_chip().unwrap().mode, PermissionMode::Ask);
                assert_eq!(
                    view.footer_fields().unwrap().git.unwrap().label,
                    "review/live*"
                );
            })
            .unwrap();

        click(
            window.into(),
            cx,
            MouseButton::Left,
            bounds["quota"].center(),
        );
        assert!(draw_hud(window.into(), cx).contains_key("footer-quota-menu"));
        press(window, cx, "escape");
        assert!(!draw_hud(window.into(), cx).contains_key("footer-quota-menu"));
        window
            .update(cx, |view, window, _| {
                assert!(view.control_focus["footer-quota"].is_focused(window))
            })
            .unwrap();
        click(
            window.into(),
            cx,
            MouseButton::Left,
            bounds["quota"].center(),
        );
        assert!(draw_hud(window.into(), cx).contains_key("footer-quota-menu"));
        click(
            window.into(),
            cx,
            MouseButton::Left,
            point(px(20.), px(90.)),
        );
        assert!(!draw_hud(window.into(), cx).contains_key("footer-quota-menu"));

        use matou_native_runtime::host::{GitFailure, GitReply};

        let locale = window.update(cx, |view, _, _| view.locale).unwrap();
        let worktree = cwd.with_file_name("isolated-checkout");
        let git_status = |branch: &str, pending: usize, linked: bool, can_push: bool| {
            let mut branches = vec![
                json!({"name":"review/live","current":branch == "review/live","commitTimestamp":1}),
                json!({"name":"fix/popup","current":branch == "fix/popup","commitTimestamp":0}),
            ];
            if !matches!(branch, "review/live" | "fix/popup") {
                branches.push(json!({"name":branch,"current":true,"commitTimestamp":2}));
            }
            let mut worktrees = vec![json!({"path":cwd,"branch":branch,"current":true,
                "main":true,"dirty":pending > 0,"locked":false,"prunable":false})];
            if linked {
                worktrees.push(
                    json!({"path":worktree,"branch":"w","current":false,"main":false,
                    "dirty":false,"locked":false,"prunable":false}),
                );
            }
            serde_json::from_value(json!({
                "repositoryRoot":cwd,"currentBranch":branch,"branches":branches,"dirty":pending > 0,
                "dirtyFileCount":pending,"stagedCount":0,"additions":3,"deletions":1,"upstream":"origin/review/live",
                "ahead":1,"behind":0,"hasRemote":true,"canPush":can_push,"worktrees":worktrees,
            })).unwrap()
        };
        let deliver_git = |event: Event, cx: &mut AsyncApp| {
            window
                .update(cx, |view, window, cx| {
                    view.apply(
                        BridgeEvent::Runtime {
                            generation: 1,
                            event,
                        },
                        window,
                        cx,
                    );
                })
                .unwrap();
        };
        let reply = |request_id, result| GitReply {
            request_id,
            result,
            worktree_session_counts: Default::default(),
        };
        let git_click = |key, cx: &mut AsyncApp| {
            let menu = draw_hud(window.into(), cx);
            click(window.into(), cx, MouseButton::Left, menu[key].center());
        };
        let open_git = |cx: &mut AsyncApp| {
            git_click("git", cx);
            std::iter::from_fn(|| sink.try_recv().ok())
                .find_map(|command| match command {
                    Command::GitStatusDetail {
                        request_id,
                        cwd: path,
                    } if path == cwd => Some(request_id),
                    _ => None,
                })
                .expect("Git menu requests detailed status for the current worktree")
        };

        let inspect = open_git(cx);
        let loading = draw_hud(window.into(), cx);
        assert!(loading["footer-git-menu"].bottom() <= bounds["git"].top());
        assert_eq!(loading["footer-git-menu"].size.width, px(316.));
        assert!(loading.contains_key("hud-menu-git-search"));
        deliver_git(
            Event::GitStatusDetail(reply(
                inspect,
                Ok(git_status("review/live", 1, false, true)),
            )),
            cx,
        );
        let menu = draw_hud(window.into(), cx);
        for id in [
            "hud-menu-git-create-branch",
            "hud-menu-git-worktrees",
            "hud-menu-git-commit-view",
        ] {
            assert!(menu.contains_key(id), "missing original action {id}");
        }
        press(window, cx, "f");
        assert_eq!(
            hud_text(window.into(), cx, "git-menu-other").as_deref(),
            Some("fix/popup")
        );
        assert!(hud_text(window.into(), cx, "git-menu-current").is_none());
        git_click("hud-menu-git-branch-other", cx);
        let checkout = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GitCheckout {
                    request_id,
                    cwd: path,
                    branch,
                } if path == cwd && branch == "fix/popup" => Some(request_id),
                _ => None,
            })
            .expect("clicking a branch switches the existing checkout");
        deliver_git(
            Event::GitCheckout(reply(checkout, Ok(git_status("fix/popup", 1, false, true)))),
            cx,
        );
        assert!(!draw_hud(window.into(), cx).contains_key("footer-git-menu"));
        assert_eq!(
            hud_text(window.into(), cx, "git").as_deref(),
            Some("fix/popup*")
        );

        let inspect = open_git(cx);
        deliver_git(
            Event::GitStatusDetail(reply(inspect, Ok(git_status("fix/popup", 1, false, true)))),
            cx,
        );
        git_click("hud-menu-git-create-branch", cx);
        assert!(draw_hud(window.into(), cx).contains_key("hud-menu-git-name"));
        press(window, cx, "x");
        git_click("hud-menu-git-base", cx);
        assert!(draw_hud(window.into(), cx).contains_key("hud-menu-git-base-input"));
        for key in ["m", "a", "i", "n"] {
            press(window, cx, key);
        }
        git_click("hud-menu-git-create", cx);
        let create = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GitCreateBranch {
                    request_id,
                    cwd: path,
                    name,
                    start_point,
                } if path == cwd && name == "x" && start_point.as_deref() == Some("main") => {
                    Some(request_id)
                }
                _ => None,
            })
            .expect("branch form sends its name and optional start point");
        deliver_git(
            Event::GitCreateBranch(reply(create, Ok(git_status("x", 1, false, true)))),
            cx,
        );
        assert_eq!(hud_text(window.into(), cx, "git").as_deref(), Some("x*"));

        let inspect = open_git(cx);
        deliver_git(
            Event::GitStatusDetail(reply(inspect, Ok(git_status("x", 1, false, true)))),
            cx,
        );
        git_click("hud-menu-git-worktrees", cx);
        assert!(draw_hud(window.into(), cx).contains_key("hud-menu-git-worktree-more-current"));
        git_click("hud-menu-git-worktree-more-current", cx);
        let item = draw_hud(window.into(), cx);
        assert!(item.contains_key("hud-menu-git-reveal-worktree"));
        assert!(!item.contains_key("hud-menu-git-remove-worktree"));
        press(window, cx, "escape");
        git_click("hud-menu-git-create-worktree", cx);
        assert!(draw_hud(window.into(), cx).contains_key("hud-menu-git-name"));
        press(window, cx, "escape");
        assert!(draw_hud(window.into(), cx).contains_key("hud-menu-git-create-worktree"));
        git_click("hud-menu-git-create-worktree", cx);
        press(window, cx, "w");
        press(window, cx, "enter");
        let create_tree = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GitWorktreeCreate {
                    request_id,
                    cwd: path,
                    branch,
                    path: None,
                } if path == cwd && branch == "w" => Some(request_id),
                _ => None,
            })
            .expect("worktree form sends a create command");
        deliver_git(
            Event::GitWorktreeCreate(reply(create_tree, Ok(git_status("x", 1, true, true)))),
            cx,
        );
        git_click("hud-menu-git-worktree-more-other", cx);
        git_click("hud-menu-git-remove-worktree", cx);
        let remove = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GitWorktreeRemove {
                    request_id,
                    cwd: path,
                    worktree_path,
                } if path == cwd && worktree_path == worktree => Some(request_id),
                _ => None,
            })
            .expect("worktree menu removes the selected idle checkout");
        deliver_git(
            Event::GitWorktreeRemove(reply(remove, Ok(git_status("x", 1, false, true)))),
            cx,
        );
        assert!(!draw_hud(window.into(), cx).contains_key("hud-menu-git-worktree-more-other"));
        press(window, cx, "escape");
        assert!(draw_hud(window.into(), cx).contains_key("hud-menu-git-search"));

        git_click("hud-menu-git-commit-view", cx);
        let view = draw_hud(window.into(), cx);
        for id in [
            "hud-menu-git-message",
            "hud-menu-git-commit",
            "hud-menu-git-commit-push",
            "hud-menu-git-push",
        ] {
            assert!(view.contains_key(id), "missing commit control {id}");
        }
        assert_eq!(
            hud_text(window.into(), cx, "git-menu-commit-count").as_deref(),
            Some(locale.text("未提交：1 个文件", "1 uncommitted file"))
        );
        press(window, cx, "o");
        press(window, cx, "k");
        git_click("hud-menu-git-commit-push", cx);
        let commit = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GitCommit {
                    request_id,
                    cwd: path,
                    message,
                    stage_all: true,
                } if path == cwd && message == "ok" => Some(request_id),
                _ => None,
            })
            .expect("commit and push first commits the entered message and staged scope");
        git_click("hud-menu-git-push", cx);
        assert!(
            !std::iter::from_fn(|| sink.try_recv().ok())
                .any(|command| matches!(command, Command::GitPush { .. }))
        );
        deliver_git(
            Event::GitCommit(reply(commit, Ok(git_status("x", 0, false, true)))),
            cx,
        );
        assert_eq!(hud_text(window.into(), cx, "git").as_deref(), Some("x"));
        let push = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GitPush {
                    request_id,
                    cwd: path,
                } if path == cwd => Some(request_id),
                _ => None,
            })
            .expect("push follows successful commit");
        deliver_git(
            Event::GitPush(reply(
                push,
                Err(GitFailure {
                    code: "GIT_PUSH_REJECTED".into(),
                    message: "non-fast-forward".into(),
                }),
            )),
            cx,
        );
        assert!(
            hud_text(window.into(), cx, "git-menu-feedback")
                .unwrap()
                .starts_with(locale.text("已提交。推送失败。", "Commit complete. Push failed."))
        );
        git_click("hud-menu-git-push", cx);
        let push = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GitPush {
                    request_id,
                    cwd: path,
                } if path == cwd => Some(request_id),
                _ => None,
            })
            .expect("standalone push works after a failed combined push");
        deliver_git(
            Event::GitPush(reply(push, Ok(git_status("x", 0, false, false)))),
            cx,
        );
        assert_eq!(
            hud_text(window.into(), cx, "git-menu-feedback").as_deref(),
            Some(locale.text("推送已完成", "Push complete"))
        );
        press(window, cx, "escape");
        press(window, cx, "f");
        assert_eq!(
            hud_text(window.into(), cx, "git-menu-other").as_deref(),
            Some("fix/popup")
        );
        assert!(hud_text(window.into(), cx, "git-menu-current").is_none());
        press(window, cx, "escape");
        assert!(!draw_hud(window.into(), cx).contains_key("footer-git-menu"));
        window
            .update(cx, |view, window, _| {
                assert!(view.control_focus["footer-git"].is_focused(window));
            })
            .unwrap();

        click(
            window.into(),
            cx,
            MouseButton::Left,
            bounds["environment"].center(),
        );
        assert_eq!(
            hud_text(window.into(), cx, "environment-menu-title").as_deref(),
            Some("Worktree")
        );
        press(window, cx, "escape");
        window
            .update(cx, |view, _, cx| {
                view.graph["nodes"][0]["environment"]["kind"] = json!("local");
                cx.notify();
            })
            .unwrap();
        let local_trigger = draw_hud(window.into(), cx)["environment"];
        click(window.into(), cx, MouseButton::Left, local_trigger.center());
        let environment = draw_hud(window.into(), cx);
        assert!(environment["footer-environment-menu"].bottom() <= local_trigger.top());
        assert_eq!(environment["footer-environment-menu"].size.width, px(360.));
        assert!(environment.contains_key("hud-menu-environment-title"));
        assert!(environment.contains_key("hud-menu-environment-path"));
        assert_eq!(
            hud_text(window.into(), cx, "environment-menu-title").as_deref(),
            Some("Local")
        );
        assert_eq!(
            hud_text(window.into(), cx, "environment-menu-path").as_deref(),
            cwd.to_str()
        );
        assert!(!environment.contains_key("hud-menu-refresh"));
        let reveal = environment["hud-menu-reveal"];
        let terminal = environment["hud-menu-terminal"];
        assert_eq!(reveal.size, terminal.size);
        assert_eq!(reveal.top(), terminal.top());
        // Bounds are recorded inside the two 1px button borders.
        assert_eq!(terminal.left() - reveal.right() - px(2.), px(7.));
        assert!(
            std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(command,
            Command::GetEnvironmentDiagnostics { cwd: path, repair: None, .. } if path == cwd))
        );
        click(
            window.into(),
            cx,
            MouseButton::Left,
            environment["hud-menu-terminal"].center(),
        );
        assert!(!draw_hud(window.into(), cx).contains_key("footer-environment-menu"));
        window
            .update(cx, |view, _, cx| {
                view.graph["nodes"][0]["environment"]["kind"] = json!("worktree");
                cx.notify();
            })
            .unwrap();
        draw_hud(window.into(), cx);

        click(
            window.into(),
            cx,
            MouseButton::Left,
            bounds["instructions"].center(),
        );
        for _ in 0..100 {
            draw_hud(window.into(), cx);
            let loaded = window
                .update(cx, |view, window, _| {
                    view.project_instructions
                        .get(&window.window_handle())
                        .and_then(|state| state.preview.as_ref())
                        .is_some_and(|preview| preview.input.is_some())
                })
                .unwrap();
            if loaded {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
        let editor = wait_for_hud_element(window.into(), cx, "instructions-editor").await;
        click(
            window.into(),
            cx,
            MouseButton::Left,
            editor.origin + point(px(22.), px(22.)),
        );
        press(window, cx, "home");
        press(window, cx, "a");
        draw_hud(window.into(), cx);
        let save = draw_hud(window.into(), cx)["instructions-save"];
        click(window.into(), cx, MouseButton::Left, save.center());
        for _ in 0..100 {
            if !draw_hud(window.into(), cx).contains_key("instructions-dialog") {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
        assert_eq!(
            std::fs::read_to_string(&instruction_file).unwrap(),
            "a# Project rules\n"
        );
        window
            .update(cx, |view, window, _| {
                assert!(view.control_focus["project-instructions"].is_focused(window))
            })
            .unwrap();

        click(
            window.into(),
            cx,
            MouseButton::Left,
            bounds["permission"].center(),
        );
        let permission = draw_hud(window.into(), cx);
        assert!(permission.contains_key("ask"));
        click(
            window.into(),
            cx,
            MouseButton::Left,
            permission["bypassPermissions"].center(),
        );
        window.update(cx, |view, window, cx| {
            assert!(matches!(&view.dialog, Some(TabDialog::PermissionMode(state))
                if state.provider == "Antigravity" && state.from == PermissionMode::Ask && state.target == PermissionMode::Bypass));
            view.confirm_dialog(window, cx);
        }).unwrap();
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(command,
            Command::SetProviderPermissionMode { session_id, mode } if session_id == A && mode == "bypassPermissions")));
        window
            .update(cx, |view, _, cx| {
                view.graph["nodes"][0]["permissionMode"] = json!("bypassPermissions");
                view.reconcile_permission_modes(
                    &[json!({
                        "sessionId": A, "permissionMode": "bypassPermissions"
                    })],
                    cx,
                );
                assert_eq!(view.permission_chip().unwrap().mode, PermissionMode::Bypass);
                cx.notify();
            })
            .unwrap();
        let bypass = draw_hud(window.into(), cx)["permission"];
        click(window.into(), cx, MouseButton::Left, bypass.center());
        let ask = draw_hud(window.into(), cx)["ask"];
        click(window.into(), cx, MouseButton::Left, ask.center());
        window
            .update(cx, |view, window, cx| {
                assert!(
                    matches!(&view.dialog, Some(TabDialog::PermissionMode(state))
                if state.from == PermissionMode::Bypass && state.target == PermissionMode::Ask)
                );
                view.close_dialog(window, cx);
            })
            .unwrap();
        card_action(window, cx, A, CardMenuAction::OpenDetached);
        window
            .update(cx, |view, _, cx| {
                view.focused = Some(B.into());
                view.refresh_level(cx);
            })
            .unwrap();
        let child = window
            .update(cx, |view, _, _| view.wiring_child_for_probe(A).unwrap())
            .unwrap();
        assert!(draw_hud(window.into(), cx).contains_key("account"));
        wait_for_hud_element(child, cx, "permission").await;
        let detached = draw_hud(child, cx);
        assert!(detached.contains_key("name"));
        assert!(detached.contains_key("tools"));
        assert!(detached.contains_key("git"));
        assert!(detached.contains_key("environment"));
        assert!(detached.contains_key("egress"));
        assert!(!detached.contains_key("account"));
        click(
            child,
            cx,
            MouseButton::Left,
            detached["permission"].center(),
        );
        assert!(draw_hud(child, cx).contains_key("permission-menu"));
        dispatch_key(child, cx, "escape");
        assert!(!draw_hud(child, cx).contains_key("permission-menu"));
        let _ = child.update(cx, |_, window, _| window.remove_window());
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "HUD: live projection, Antigravity quota/permission, Git/environment popovers, edit/save and detached footer"
        );
    }

    pub(super) fn run() {
        let fixture_root = live_wiring::prepare();
        Application::new().with_assets(NativeAssets).run(move |cx| {
            cx.spawn(async move |cx| {
                verify(cx, &fixture_root).await;
                verify_card_headers(cx);
                verify_footer_hud(cx, &fixture_root).await;
                verify_horizontal_divider_drag(cx);
                verify_splits(cx);
                verify_carousel_focus(cx).await;
                verify_carousel_and_zoom(cx, &fixture_root.join("font-zoom")).await;
                verify_links(cx).await;
                live_wiring::verify(cx, fixture_root.clone()).await;
                live_wiring::verify_board(cx, fixture_root.join("board")).await;
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
    }
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn main() {
    workspace::run();
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn main() {}
