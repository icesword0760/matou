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
#[path = "../src/terminal_surface.rs"]
#[allow(dead_code)]
mod terminal_surface;
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
        window.focus(&view.control_focus["board"]);
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
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
            })
            .unwrap();
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

    async fn verify(cx: &mut AsyncApp) {
        let (window, sink) = open_probe_at(cx, 1260.);
        draw(window.into(), cx);
        for key in ["cmd-/", "ctrl-/"] {
            press(window, cx, key);
            overlay(window, cx, Some("shortcuts"));
            draw(window.into(), cx);
            press(window, cx, "escape");
            overlay(window, cx, None);
            window
                .update(cx, |view, window, _| {
                    assert!(view.control_focus["board"].is_focused(window))
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
        window
            .update(cx, |view, window, _| {
                window.focus(&view.control_focus["shortcut-help"])
            })
            .unwrap();
        draw(window.into(), cx);
        press(window, cx, "enter");
        overlay(window, cx, Some("shortcuts"));
        draw(window.into(), cx);
        click(
            window.into(),
            cx,
            MouseButton::Left,
            point(px(14.), px(80.)),
        );
        overlay(window, cx, None);
        draw(window.into(), cx);
        press(window, cx, "enter");
        overlay(window, cx, Some("shortcuts"));
        draw(window.into(), cx);
        cx.background_executor()
            .timer(Duration::from_millis(220))
            .await;
        draw(window.into(), cx);
        click(
            window.into(),
            cx,
            MouseButton::Left,
            point(px(998.), px(87.)),
        );
        overlay(window, cx, None);
        println!(
            "shortcuts: Cmd/Ctrl+/, explicit help, close button, Escape, backdrop, restored focus, terminal-input guard"
        );

        window
            .update(cx, |view, window, cx| {
                view.open_search(window, cx);
                view.search.as_mut().unwrap().query = "keep this query".into();
                window.focus(&view.control_focus["board"]);
            })
            .unwrap();
        draw(window.into(), cx);
        press(window, cx, "enter");
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
                window.focus(&view.control_focus["board"]);
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
                assert!(view.control_focus["board"].is_focused(window))
            })
            .unwrap();
        draw(window.into(), cx);
        let bounds = card_bounds(window, cx, A);
        click(
            window.into(),
            cx,
            MouseButton::Left,
            point(bounds.right() - px(58.), bounds.top() + px(21.)),
        );
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
            "card menu: right-click and more button, Shell actions, Escape and outside dismissal, focus, rename and close"
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
        draw(first, cx);
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
        close_native(first, cx);
        let mounts = draw(window.into(), cx);
        assert!(
            !mounts.contains_key("detached-mount-shell-a")
                && mounts.contains_key("detached-mount-shell-b")
        );
        let return_at = second
            .update(cx, |_, window, _| {
                point(window.viewport_size().width - px(70.), px(21.))
            })
            .unwrap();
        click(second, cx, MouseButton::Left, return_at);
        let mounts = draw(window.into(), cx);
        assert!(!mounts.contains_key("detached-mount-shell-b"));
        assert!(second.update(cx, |_, _, _| ()).is_err());
        println!(
            "detached: two hidden child windows, original canvas mounts, placeholder focus route, child removal and return button"
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
                assert!(
                    view.update_view_for_probe()
                        .buttons
                        .iter()
                        .any(|button| button.enabled)
                );
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
        click(window.into(), cx, MouseButton::Left, update);
        overlay(window, cx, None);
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
                            window.focus(&view.control_focus["board"]);
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
            PlatformInput::MouseMove(event) => (
                NSEventType::NSLeftMouseDragged,
                event.position,
                event.modifiers,
            ),
            _ => unreachable!(),
        };
        let (view, size) = native_view(window.into(), cx);
        let location = NSPoint::new(f64::from(position.x), f64::from(size.height - position.y));
        let flags = if modifiers.platform {
            NSEventModifierFlags::NSCommandKeyMask
        } else {
            NSEventModifierFlags::empty()
        };
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

    pub(super) fn run() {
        let fixture_root = live_wiring::prepare();
        Application::new().with_assets(NativeAssets).run(move |cx| {
            cx.spawn(async move |cx| {
                verify(cx).await;
                verify_splits(cx);
                verify_links(cx).await;
                live_wiring::verify(cx, fixture_root).await;
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
