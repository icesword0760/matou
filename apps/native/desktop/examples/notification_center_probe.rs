//! Hidden GPUI notification probe using an isolated runtime graph and fake notifications.
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
#[path = "../src/provider_settings.rs"]
mod provider_settings;
#[cfg(target_os = "macos")]
#[path = "../src/settings_shell.rs"]
mod settings_shell;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    impl Workspace {
        pub(super) fn deliver(
            &mut self,
            event: Event,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event,
                },
                window,
                cx,
            );
        }

        pub(super) fn use_preferences(&mut self, path: PathBuf) {
            self.notifications = Notifications::new(Some(path));
        }

        pub(super) fn assert_center(&self, open: bool, unread: usize, titles: &[(&str, bool)]) {
            assert_eq!(self.notification_open, open);
            assert_eq!(
                self.notifications
                    .entries
                    .iter()
                    .filter(|entry| !entry.read)
                    .count(),
                unread
            );
            assert_eq!(
                self.notifications
                    .entries
                    .iter()
                    .map(|row| (row.title.as_str(), row.read))
                    .collect::<Vec<_>>(),
                titles
            );
        }

        pub(super) fn assert_details(
            &self,
            workspace_name: &str,
            task_title: &str,
            timestamp: u64,
        ) {
            let row = &self.notifications.entries[0];
            let workspace = self
                .workspaces
                .iter()
                .find(|workspace| Some(workspace.id.as_str()) == row.workspace_id.as_deref())
                .unwrap();
            let task = self
                .tasks
                .iter()
                .find(|task| Some(task.id.as_str()) == row.task_id.as_deref())
                .unwrap();
            assert_eq!(workspace.name, workspace_name);
            assert_eq!(task.title, task_title);
            assert_eq!(row.subtitle, "Completed in isolated project");
            assert_eq!(row.body, "第一行摘要\nSecond line");
            assert_eq!(row.occurred_at, timestamp);
            let time = notification_time(row.occurred_at);
            assert_eq!(time.len(), 5);
            assert_eq!(&time[2..3], ":");
            assert!(time[..2].parse::<u8>().unwrap() < 24);
            assert!(time[3..].parse::<u8>().unwrap() < 60);
            assert!(self.graph.get("agentNotifications").is_none());

            let dark = NotificationUiPalette::for_appearance(WindowAppearance::Dark);
            let light = NotificationUiPalette::for_appearance(WindowAppearance::Light);
            assert_eq!(dark.panel_bg, 0x212121);
            assert_eq!(light.panel_bg, 0xffffff);
            assert_eq!(dark.item_bg, rgba(0xffffff0a));
            assert_eq!(dark.item_hover_bg, rgba(0xffffff0f));
            assert_eq!(light.item_bg, rgba(0xf7f8faff));
            let title_weight =
                |mut title: Stateful<Div>| title.style().text.as_ref().unwrap().font_weight;
            assert_eq!(
                title_weight(notification_item_title(row, dark)),
                Some(FontWeight::BOLD)
            );
            assert_eq!(
                title_weight(notification_item_title(
                    &self.notifications.entries[1],
                    dark
                )),
                Some(FontWeight::MEDIUM)
            );
            assert_eq!(
                (
                    NOTIFICATION_UI_PANEL_WIDTH,
                    NOTIFICATION_UI_PANEL_TOP,
                    NOTIFICATION_UI_PANEL_BOTTOM_GAP,
                    NOTIFICATION_UI_PANEL_RADIUS,
                    NOTIFICATION_UI_ANIMATION_MS
                ),
                (382., 49., 5., 12., 220),
            );
        }

        pub(super) fn assert_navigation(&self, scene: &str, session: &str) {
            assert_eq!(self.active_scene_id(), Some(scene));
            assert_eq!(self.level_focus(), Some(session));
        }

        pub(super) fn layout(&self) -> (Bounds<Pixels>, Bounds<Pixels>) {
            (self.sidebar_scroll.bounds(), self.carousel_scroll.bounds())
        }

        pub(super) fn assert_sound_pref(&self, enabled: bool) {
            assert_eq!(self.notifications.sound_enabled, enabled);
            let reopened = Notifications::new(self.notifications.sound_path.clone());
            assert_eq!(reopened.sound_enabled, enabled);
        }
    }

    pub(super) fn verify_merge() {
        use serde_json::json;

        let duration = NOTIFICATION_READ_RETENTION_MS;
        let at = duration + 10_000;
        let mut store = Notifications::new(None);
        let make = |id: &str, workspace: &str, session: &str, time| {
            json!({
                "eventId": id, "eventType": "completed", "title": id, "workspaceId": workspace,
                "sessionId": session, "cooldownKey": "done", "occurredAt": time,
            })
        };
        assert!(store.push(&make("first", "a", "session", at), None, at));
        assert!(!store.push(
            &make("cooled", "a", "session", at + 4_999),
            None,
            at + 4_999
        ));
        assert_eq!(store.entries.len(), 1);
        assert!(store.push(&make("next", "a", "session", at + 5_000), None, at + 5_000));
        assert!(store.entries[1].read);
        assert!(store.push(&make("other", "a", "other", at + 5_000), None, at + 5_000));
        let mut replacement = make("pending", "a", "replacement", at + 6_000);
        replacement["replacementKey"] = json!("permission-request");
        assert!(store.push(&replacement, None, at + 6_000));
        let stable_id = store.entries[0].id;
        replacement["eventId"] = json!("replacement");
        replacement["title"] = json!("replaced");
        assert!(!store.push(&replacement, Some("replacement"), at + 6_001));
        assert_eq!(store.entries[0].id, stable_id);
        assert_eq!(store.entries[0].title, "replaced");
        assert!(store.entries[0].read);
        assert_eq!(store.entries.len(), 4);

        let mut retention = Notifications::new(None);
        assert!(!retention.push(
            &make("old-read", "a", "read", at - duration),
            Some("read"),
            at
        ));
        assert!(retention.push(&make("old-unread", "a", "unread", at - duration), None, at));
        retention.prune(at + 1);
        assert_eq!(retention.entries.len(), 1);
        assert_eq!(retention.entries[0].title, "old-unread");
        let mut silent = make("silent", "b", "silent", at);
        silent["sound"] = json!(false);
        assert!(!retention.push(&silent, None, at));
        retention.set_sound(false);
        assert!(!retention.push(&make("muted", "b", "muted", at), None, at));

        let mut bounded = Notifications::new(None);
        for index in 0..1_003 {
            let id = index.to_string();
            assert!(bounded.push(&make(&id, "a", &id, at + index), None, at));
        }
        assert!(bounded.push(&make("other", "b", "other", at - 1), None, at));
        assert_eq!(bounded.entries.len(), 1_001);
        assert_eq!(bounded.entries[0].title, "1002");
        assert_eq!(bounded.entries[999].title, "3");
        assert_eq!(bounded.entries[1_000].title, "other");
        assert_eq!(bounded.cooldowns.len(), 1_001);
        assert!(bounded.push(&make("old-key-reused", "a", "0", at + 1_004), None, at));
        println!(
            "merge: owner-scoped cooldown, 5s boundary, replacement, focus, 30d retention, cap and sound gating"
        );
    }
}

#[cfg(target_os = "macos")]
mod probe {
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use cocoa::{
        appkit::{NSEvent, NSEventModifierFlags, NSEventType},
        base::{id, nil},
        foundation::{NSInteger, NSPoint},
    };
    use gpui::{
        AnyWindowHandle, AppContext, Application, AsyncApp, Bounds, Keystroke, WindowBounds,
        WindowHandle, WindowOptions, point, px, size,
    };
    use matou_native_core::hierarchy::HierarchyResult;
    use matou_native_runtime::host::{Command, Event, LaunchOptions, RuntimeHost};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use serde_json::{Value, json};

    use crate::{assets::NativeAssets, runtime_bridge, workspace::Workspace};

    fn draw(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
    }

    fn mouse(window: WindowHandle<Workspace>, cx: &mut AsyncApp, x: f32, y: f32, click: bool) {
        let (view, location, number) = window
            .update(cx, |_, window, _| {
                let RawWindowHandle::AppKit(appkit) =
                    HasWindowHandle::window_handle(window).unwrap().as_raw()
                else {
                    unreachable!()
                };
                let view = appkit.ns_view.as_ptr() as id;
                let number: NSInteger = unsafe {
                    let native: id = msg_send![view, window];
                    msg_send![native, windowNumber]
                };
                let location = NSPoint::new(
                    f64::from(x),
                    f64::from(f32::from(window.viewport_size().height) - y),
                );
                (view, location, number)
            })
            .unwrap();
        let event = |kind| unsafe {
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
                nil, kind, location, NSEventModifierFlags::empty(), 0., number, nil, 0, 1, 1.,
            )
        };
        // Send input directly to the hidden probe view, outside the OS event queue.
        unsafe {
            let _: () = msg_send![view, mouseMoved: event(NSEventType::NSMouseMoved)];
            if click {
                let _: () = msg_send![view, mouseDown: event(NSEventType::NSLeftMouseDown)];
                let _: () = msg_send![view, mouseUp: event(NSEventType::NSLeftMouseUp)];
            }
        }
    }

    fn click(window: WindowHandle<Workspace>, cx: &mut AsyncApp, x: f32, y: f32) {
        mouse(window, cx, x, y, true);
    }

    fn deliver(window: WindowHandle<Workspace>, cx: &mut AsyncApp, event: Event) {
        window
            .update(cx, |view, window, cx| view.deliver(event, window, cx))
            .unwrap();
    }

    fn target(hierarchy: &HierarchyResult) -> (&str, &str) {
        (
            &hierarchy.scene.as_ref().unwrap().id,
            &hierarchy.session.as_ref().unwrap().id,
        )
    }

    fn read_focus_command(
        sink: &runtime_bridge::ProbeReceiver,
        (scene, session): (&str, &str),
    ) -> Command {
        let navigation: Vec<_> = std::iter::from_fn(|| sink.try_recv().ok())
            .filter(|command| {
                matches!(
                    command,
                    Command::SelectWorkspace { .. }
                        | Command::SelectTask { .. }
                        | Command::SelectScene { .. }
                        | Command::FocusSession { .. }
                )
            })
            .collect();
        assert!(
            matches!(navigation.as_slice(), [Command::FocusSession { scene_id, session_id }]
            if scene_id == scene && session_id == session)
        );
        navigation.into_iter().next().unwrap()
    }

    async fn runtime_graph(
        host: &RuntimeHost,
        cx: &mut AsyncApp,
        window: WindowHandle<Workspace>,
        deliver_to_workspace: bool,
    ) -> (HierarchyResult, Value) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let event = match host.events.try_recv() {
                Ok(event) => event,
                Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                    cx.background_executor()
                        .timer(Duration::from_millis(10))
                        .await;
                    continue;
                }
                Err(error) => panic!("no real runtime graph: {error}"),
            };
            let graph = match &event {
                Event::Ready {
                    hierarchy, graph, ..
                }
                | Event::Navigation {
                    hierarchy, graph, ..
                } => Some((hierarchy.clone(), graph.clone())),
                Event::Diagnostic(message) | Event::NavigationFailed { message, .. } => {
                    panic!("{message}")
                }
                _ => None,
            };
            if deliver_to_workspace {
                deliver(window, cx, event);
            }
            if let Some(graph) = graph {
                return graph;
            }
        }
    }

    fn payload(
        id: &str,
        title: &str,
        owner: &HierarchyResult,
        timestamp: u64,
        scene: bool,
    ) -> Value {
        let (scene_id, session_id) = target(owner);
        let mut event = json!({
            "eventId": id, "eventType": "completed", "title": title,
            "subtitle": "Completed in isolated project", "body": "第一行摘要\nSecond line",
            "workspaceId": owner.workspace.as_ref().unwrap().id,
            "taskId": owner.task.as_ref().unwrap().id,
            "sessionId": session_id, "occurredAt": timestamp, "sound": false,
        });
        if scene {
            event["sceneId"] = json!(scene_id);
        }
        event
    }

    async fn verify(cx: &mut AsyncApp) {
        super::workspace::verify_merge();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target")
            .join(format!("notification-probe-{}", uuid::Uuid::new_v4()));
        let active_path = root.join("project-active");
        let inactive_path = root.join("project-background");
        std::fs::create_dir_all(&active_path).unwrap();
        std::fs::create_dir_all(&inactive_path).unwrap();
        let data = root.join("data");
        let options = LaunchOptions {
            data_root: data.clone(),
            workspace: inactive_path,
            shell_home: root.join("shell-home"),
            shell: "/bin/zsh".into(),
        };
        let mut host = RuntimeHost::start(options.clone()).unwrap();
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_events, incoming) = async_channel::unbounded();
        let window = cx
            .open_window(
                WindowOptions {
                    show: false,
                    focus: false,
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1000.), px(650.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
            )
            .unwrap();
        let pref = data.join("notification-sound");
        window
            .update(cx, |view, _, _| view.use_preferences(pref))
            .unwrap();
        let (background, _) = runtime_graph(&host, cx, window, false).await;
        let (background_scene, background_session) = target(&background);
        host.commands
            .send(Command::CreateShellSibling {
                scene_id: background_scene.into(),
                source_session_id: background_session.into(),
                parent_session_id: None,
            })
            .unwrap();
        let (sibling, _) = runtime_graph(&host, cx, window, false).await;
        assert_ne!(target(&sibling).1, background_session);
        host.commands
            .send(Command::CreateWorkspace {
                root_directory: active_path.to_string_lossy().into_owned(),
            })
            .unwrap();
        runtime_graph(&host, cx, window, false).await;
        host.shutdown().unwrap();
        let mut host = RuntimeHost::start(options).unwrap();
        let (active, mut graph) = runtime_graph(&host, cx, window, true).await;
        assert_ne!(target(&active), target(&background));
        assert!(
            !active
                .navigation
                .session_by_scene
                .values()
                .any(|session| session == background_session)
        );
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        graph["agentNotifications"] = json!([
            payload("new", "Claude Code", &background, timestamp, true),
            payload("old", "Focused", &active, timestamp - 1_000, false),
            payload(
                "cooled",
                "Cooldown skipped",
                &background,
                timestamp + 1,
                true
            ),
        ]);
        deliver(window, cx, Event::Graph(graph.clone()));
        graph.as_object_mut().unwrap().remove("agentNotifications");
        deliver(window, cx, Event::Graph(graph.clone()));

        let assert_center = |cx: &mut AsyncApp, open, unread, titles: &[(&str, bool)]| {
            window
                .update(cx, |view, _, _| view.assert_center(open, unread, titles))
                .unwrap();
        };
        let initial = [("Claude Code", false), ("Focused", true)];
        assert_center(cx, false, 1, &initial);
        window
            .update(cx, |view, _, _| {
                view.assert_details(
                    &background.workspace.as_ref().unwrap().name,
                    &background.task.as_ref().unwrap().title,
                    timestamp,
                );
                view.assert_sound_pref(true);
            })
            .unwrap();

        draw(window, cx);
        let layout = |cx: &mut AsyncApp| window.update(cx, |view, _, _| view.layout()).unwrap();
        let closed_layout = layout(cx);
        let bell = |cx: &mut AsyncApp| {
            click(window, cx, 232., 24.);
            draw(window, cx);
        };
        bell(cx);
        assert_center(cx, true, 1, &initial);
        assert_eq!(
            layout(cx),
            closed_layout,
            "overlay must preserve both viewports"
        );
        bell(cx);
        assert_center(cx, false, 1, &initial);
        bell(cx);
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                assert!(window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx));
            })
            .unwrap();
        draw(window, cx);
        assert_center(cx, false, 1, &initial);
        bell(cx);
        click(window, cx, 410., 300.);
        draw(window, cx);
        assert_center(cx, false, 1, &initial);
        bell(cx);

        let footer_y = window
            .update(cx, |_, window, _| {
                f32::from(window.viewport_size().height) - 28.
            })
            .unwrap();
        click(window, cx, 35., footer_y);
        window
            .update(cx, |view, _, _| view.assert_sound_pref(false))
            .unwrap();
        draw(window, cx);
        while sink.try_recv().is_ok() {}
        click(window, cx, 90., 125.);
        let (scene, session) = target(&background);
        host.commands
            .send(read_focus_command(&sink, (scene, session)))
            .unwrap();
        let (selected, mut graph) = runtime_graph(&host, cx, window, true).await;
        assert_eq!(target(&selected), (scene, session));
        assert_eq!(
            selected.workspace.as_ref().unwrap().id,
            background.workspace.as_ref().unwrap().id
        );
        assert_eq!(
            selected.task.as_ref().unwrap().id,
            background.task.as_ref().unwrap().id
        );
        draw(window, cx);
        assert_center(cx, false, 0, &[("Claude Code", true), ("Focused", true)]);
        window
            .update(cx, |view, _, _| view.assert_navigation(scene, session))
            .unwrap();
        println!("navigation: FocusSession({scene}, {session}) switched workspace, task and scene");

        let mut missing = payload("gone", "Unknown target", &background, timestamp + 2, false);
        missing["sessionId"] = json!("unknown-session");
        graph["agentNotifications"] = json!([missing]);
        deliver(window, cx, Event::Graph(graph.clone()));
        bell(cx);
        while sink.try_recv().is_ok() {}
        click(window, cx, 60., 120.);
        draw(window, cx);
        assert_center(
            cx,
            true,
            1,
            &[
                ("Unknown target", false),
                ("Claude Code", true),
                ("Focused", true),
            ],
        );
        assert!(
            !std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(
                command,
                Command::FocusSession { .. } | Command::SelectScene { .. }
            ))
        );

        // Move over the first row, then hit its X once the hover style is rendered.
        mouse(window, cx, 345., 118., false);
        draw(window, cx);
        click(window, cx, 345., 120.);
        draw(window, cx);
        assert_center(cx, true, 0, &[("Claude Code", true), ("Focused", true)]);
        click(window, cx, 329., 71.);
        draw(window, cx);
        assert_center(cx, true, 0, &[]);
        click(window, cx, 358., 71.);
        draw(window, cx);
        assert_center(cx, false, 0, &[]);
        assert!(
            !std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(
                command,
                Command::FocusSession { .. } | Command::SelectScene { .. }
            ))
        );
        graph["agentNotifications"] =
            json!([payload("legacy", "Legacy", &active, timestamp + 3, false)]);
        deliver(window, cx, Event::Graph(graph));
        bell(cx);
        click(window, cx, 90., 125.);
        host.commands
            .send(read_focus_command(&sink, target(&active)))
            .unwrap();
        let (selected, _) = runtime_graph(&host, cx, window, true).await;
        assert_eq!(target(&selected), target(&active));
        draw(window, cx);
        assert_center(cx, false, 0, &[("Legacy", true)]);
        println!("navigation: older payload resolved by the existing projection");
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        host.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
        println!(
            "GPUI: bell, count and order, close and reopen, Esc, outside click, details, read, dismiss, clear, sound preference"
        );
    }

    pub(super) fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
            cx.spawn(async |cx| {
                verify(cx).await;
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
