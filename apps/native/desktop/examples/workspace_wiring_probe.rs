//! Hidden GPUI interaction probe for the seven Workspace entries.
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

    use crate::assets::NativeAssets;
    use matou_native_desktop::{
        card_context_menu::CardMenuAction,
        egress_panel::{
            EgressFamily, EgressFamilyStatus, EgressState, EgressStatus, EgressTransition,
            EgressViolation,
        },
        update_popover::{UpdateInstallMode, UpdateProgress, UpdateRelease, UpdateStatus},
    };
    use serde_json::json;
    use workspace_wiring::capture_wiring_bounds;

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
                view.request_offline_update_check_for_probe(window, cx);
                assert!(!view.update_view_for_probe().error);
            })
            .unwrap();
        draw(window.into(), cx);
        cx.background_executor()
            .timer(Duration::from_millis(400))
            .await;
        window
            .update(cx, |view, _, _| assert!(view.update_view_for_probe().error))
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

    pub(super) fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
            cx.spawn(async move |cx| {
                verify(cx).await;
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
