//! GPUI notification probe using an isolated runtime and synthetic notifications.
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

            let dark = popover_colors(true).notification;
            let light = popover_colors(false).notification;
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
                Some(FontWeight::BOLD)
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

        #[cfg(debug_assertions)]
        pub(super) fn trail(&self, owner: &HierarchyResult, cx: &mut App) -> [usize; 7] {
            let workspace = &owner.workspace.as_ref().unwrap().id;
            let task = &owner.task.as_ref().unwrap().id;
            let scene = &owner.scene.as_ref().unwrap().id;
            let session = &owner.session.as_ref().unwrap().id;
            let dag = self.dag_window.is_some_and(|dag| {
                dag.update(cx, |view, _, cx| {
                    view.has_notification_for_probe(session, cx)
                })
                .unwrap()
            });
            [
                self.notifications.unread().count(),
                self.notifications.entries.len(),
                self.notifications.unread_for_workspace(workspace),
                self.notifications.unread_for_task(task),
                self.notifications.unread_for_scene(scene),
                usize::from(self.notifications.session_has_unread(session)),
                usize::from(dag),
            ]
        }

        #[cfg(debug_assertions)]
        pub(super) fn open_probe_dag(&mut self, window: &Window, cx: &mut Context<Self>) {
            let session = self.focused.clone().unwrap();
            self.dag_window = Some(self.create_dag_window(&session, false, window, cx).unwrap());
            self.notifications_changed(cx);
        }

        #[cfg(debug_assertions)]
        pub(super) fn collapse_probe_workspace(&mut self, id: &str, cx: &mut Context<Self>) {
            self.workspace_collapsed.insert(id.into());
            cx.notify();
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_bounds(draw: impl FnOnce()) -> HashMap<String, Bounds<Pixels>> {
            workspace_wiring::capture_wiring_bounds(draw)
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_dag(&self) -> AnyWindowHandle {
            self.dag_window.unwrap().into()
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_task_bounds(&self, task: &str) -> Bounds<Pixels> {
            self.task_bounds[task]
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_clear(&mut self, cx: &mut Context<Self>) {
            self.clear_notifications(cx);
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_detach(
            &mut self,
            session: &str,
            visible: bool,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> AnyWindowHandle {
            if !visible {
                self.hide_wiring_children_for_probe();
            }
            self.detach_card_for_probe(session, window, cx);
            self.wiring_child_for_probe(session).unwrap()
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_close_dag(&mut self, cx: &mut Context<Self>) {
            if let Some(child) = self.dag_window.take() {
                child
                    .update(cx, |_, window, _| window.remove_window())
                    .unwrap();
            }
        }

        #[cfg(debug_assertions)]
        pub(super) fn probe_open_overflow(&mut self, cx: &mut Context<Self>) {
            self.hidden_tabs = Some(
                self.scenes
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| self.scene_tab_hidden(*index))
                    .map(|(_, scene)| (scene.id.clone(), cx.focus_handle()))
                    .collect(),
            );
            cx.notify();
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
        let mut read_title =
            notification_item_title(&store.entries[1], popover_colors(true).notification);
        assert_eq!(
            read_title.style().text.as_ref().unwrap().font_weight,
            Some(FontWeight::MEDIUM)
        );
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

    #[cfg(debug_assertions)]
    fn painted(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
    ) -> std::collections::HashMap<String, Bounds<gpui::Pixels>> {
        Workspace::probe_bounds(|| draw(window, cx))
    }

    #[cfg(debug_assertions)]
    fn check_trail(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        label: &str,
        owner: &HierarchyResult,
        expected: [usize; 7],
    ) {
        let actual = window
            .update(cx, |view, _, cx| view.trail(owner, cx))
            .unwrap();
        assert_eq!(actual, expected, "{label}");
        println!("{label}: bell/center/workspace/task/tab/card/DAG = {actual:?}");
    }

    #[cfg(debug_assertions)]
    async fn capture(cx: &mut AsyncApp, stage: &str, handles: &[(&str, AnyWindowHandle)]) {
        let Some(destination) = std::env::var_os("MATOU_UNREAD_TRAIL_CAPTURE").map(PathBuf::from)
        else {
            return;
        };
        cx.update(|cx| cx.activate(true)).unwrap();
        let mut windows = serde_json::Map::new();
        for &(name, handle) in handles {
            let number: NSInteger = handle
                .update(cx, |_, window, _| {
                    window.activate_window();
                    let RawWindowHandle::AppKit(appkit) =
                        HasWindowHandle::window_handle(window).unwrap().as_raw()
                    else {
                        unreachable!()
                    };
                    unsafe {
                        let view = appkit.ns_view.as_ptr() as id;
                        let native: id = msg_send![view, window];
                        msg_send![native, windowNumber]
                    }
                })
                .unwrap();
            windows.insert(name.to_owned(), json!(number));
        }
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        std::fs::write(
            destination.join(format!("{stage}.ready.json")),
            json!({"pid": std::process::id(), "windows": windows}).to_string(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(75);
        while Instant::now() < deadline && !destination.join(format!("{stage}.continue")).exists() {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
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

    #[cfg(debug_assertions)]
    async fn verify_trail(
        host: &RuntimeHost,
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        first: &HierarchyResult,
        second: &HierarchyResult,
        foreign: &HierarchyResult,
    ) {
        let (scene, session) = target(first);
        let (_, sibling) = target(second);
        let task = &first.task.as_ref().unwrap().id;
        let workspace = &first.workspace.as_ref().unwrap().id;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let select = |owner: &HierarchyResult| Command::FocusSession {
            scene_id: target(owner).0.to_owned(),
            session_id: target(owner).1.to_owned(),
        };
        let inject = |cx: &mut AsyncApp, graph: &Value, events: Vec<Value>| {
            let mut graph = graph.clone();
            graph["agentNotifications"] = json!(events);
            deliver(window, cx, Event::Graph(graph));
        };
        let click_bounds = |cx: &mut AsyncApp, bounds: Bounds<gpui::Pixels>| {
            let center = bounds.center();
            click(window, cx, f32::from(center.x), f32::from(center.y));
        };
        let clear = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, cx| view.probe_clear(cx))
                .unwrap();
        };
        let visible = std::env::var_os("MATOU_UNREAD_TRAIL_CAPTURE").is_some();
        host.commands.send(select(first)).unwrap();
        let (_, graph) = runtime_graph(host, cx, window, true).await;
        window
            .update(cx, |view, window, cx| {
                if !visible {
                    crate::macos::hide_window(window).unwrap();
                }
                view.open_probe_dag(window, cx);
            })
            .unwrap();
        cx.background_executor()
            .timer(Duration::from_millis(80))
            .await;
        if visible {
            assert_eq!(cx.update(|cx| window.is_active(cx)).unwrap(), Some(true));
        }
        inject(
            cx,
            &graph,
            vec![payload(
                "sibling",
                "Sibling completion",
                second,
                now + 10,
                false,
            )],
        );
        check_trail(
            window,
            cx,
            "2: sibling completion reaches all layers while the other card is selected",
            second,
            [1, 1, 1, 1, 1, 1, 1],
        );
        inject(
            cx,
            &graph,
            vec![payload(
                "cooled-sibling",
                "Cooldown must suppress this",
                second,
                now + 11,
                true,
            )],
        );
        check_trail(
            window,
            cx,
            "5: same key within 5s is suppressed",
            second,
            [1, 1, 1, 1, 1, 1, 1],
        );
        capture(cx, "02-sibling", &[("main", window.into())]).await;
        if visible {
            capture(cx, "00-switch-to-other-app", &[("main", window.into())]).await;
            assert_eq!(cx.update(|cx| window.is_active(cx)).unwrap(), Some(false));
            assert!(
                window
                    .update(cx, |_, window, _| crate::macos::is_window_visible(window)
                        .unwrap())
                    .unwrap()
            );
        }

        inject(
            cx,
            &graph,
            vec![payload(
                "focus-background",
                "Focused while inactive",
                first,
                now + 12,
                true,
            )],
        );
        check_trail(
            window,
            cx,
            "2 / 3: both sessions remain unread in the same canvas",
            second,
            [2, 2, 2, 2, 2, 1, 1],
        );
        check_trail(
            window,
            cx,
            "3: focused card stays unread while its window is inactive",
            first,
            [2, 2, 2, 2, 2, 1, 1],
        );
        capture(cx, "03-background", &[("main", window.into())]).await;
        let dag = window.update(cx, |view, _, _| view.probe_dag()).unwrap();
        capture(cx, "06-dag", &[("main", window.into()), ("dag", dag)]).await;

        for width in [720., 900., 1200.] {
            window
                .update(cx, |_, window, _| window.resize(size(px(width), px(650.))))
                .unwrap();
            cx.background_executor()
                .timer(Duration::from_millis(45))
                .await;
            let actual_width = window
                .update(cx, |_, window, _| f32::from(window.viewport_size().width))
                .unwrap();
            assert!(
                (actual_width - width).abs() <= 1.,
                "requested {width}, drew {actual_width}"
            );
            painted(window, cx);
            let bounds = painted(window, cx);
            assert_eq!(
                bounds[&format!("unread-workspace-{workspace}")].size.width,
                px(8.)
            );
            assert_eq!(bounds[&format!("unread-task-{task}")].size.height, px(16.));
            assert_eq!(bounds[&format!("unread-tab-{scene}")].size.width, px(8.));
            assert!(bounds.contains_key(&format!("unread-card-{sibling}")));
            println!("2: GPUI paint bounds present at {width}pt");
        }

        // Clicking the already focused card still clears exactly this session.
        let bounds = painted(window, cx)[&format!("unread-card-{session}")];
        click_bounds(cx, bounds);
        check_trail(
            window,
            cx,
            "2: clicked card only, DAG clears the same session",
            first,
            [1, 2, 1, 1, 1, 0, 0],
        );
        check_trail(
            window,
            cx,
            "2: sibling remains unread",
            second,
            [1, 2, 1, 1, 1, 1, 1],
        );
        assert!(!painted(window, cx).contains_key(&format!("unread-card-{session}")));
        capture(
            cx,
            "05-card-cleared",
            &[("main", window.into()), ("dag", dag)],
        )
        .await;

        let mut next = payload("different-key", "Permission", second, now + 13, true);
        next["cooldownKey"] = json!("Notification");
        inject(cx, &graph, vec![next]);
        check_trail(
            window,
            cx,
            "5: different key accepted with latest unread",
            second,
            [1, 3, 1, 1, 1, 1, 1],
        );
        cx.background_executor()
            .timer(Duration::from_millis(5_010))
            .await;
        inject(
            cx,
            &graph,
            vec![payload(
                "next-stop",
                "Later completion",
                second,
                now + 14,
                true,
            )],
        );
        check_trail(
            window,
            cx,
            "6: later completion retained, older rows become read",
            second,
            [1, 4, 1, 1, 1, 1, 1],
        );

        // Open an actual detached GPUI window with the same Workspace owner.
        let detached = window
            .update(cx, |view, window, cx| {
                view.probe_detach(sibling, visible, window, cx)
            })
            .unwrap();
        if visible {
            dag.update(cx, |_, window, _| window.activate_window())
                .unwrap();
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
        }
        let mut waiting = payload(
            "detached-wait",
            "Wait in detached session",
            second,
            now + 15,
            true,
        );
        waiting["cooldownKey"] = json!("waiting");
        inject(cx, &graph, vec![waiting]);
        check_trail(
            window,
            cx,
            "9: main + detached + DAG, inactive detached event remains unread",
            second,
            [1, 5, 1, 1, 1, 1, 1],
        );
        let mut before_scope = 5;
        if visible {
            capture(
                cx,
                "09-detached",
                &[("main", window.into()), ("detached", detached)],
            )
            .await;
            check_trail(
                window,
                cx,
                "9: activating the detached window preserves the old unread",
                second,
                [1, 5, 1, 1, 1, 1, 1],
            );
            let mut event = payload(
                "detached-front",
                "Already looking at detached session",
                second,
                now + 15,
                true,
            );
            event["cooldownKey"] = json!("foreground-detached");
            inject(cx, &graph, vec![event]);
            check_trail(
                window,
                cx,
                "9: active detached target, new event read",
                second,
                [0, 6, 0, 0, 0, 0, 0],
            );
            dag.update(cx, |_, window, _| window.activate_window())
                .unwrap();
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let mut event = payload(
                "dag-key-window",
                "DAG window is active",
                second,
                now + 15,
                true,
            );
            event["cooldownKey"] = json!("dag-key");
            inject(cx, &graph, vec![event]);
            check_trail(
                window,
                cx,
                "9: active DAG, detached target remains unread",
                second,
                [1, 7, 1, 1, 1, 1, 1],
            );
            before_scope = 7;
        }
        window
            .update(cx, |view, _, cx| view.probe_close_dag(cx))
            .unwrap();

        host.commands
            .send(Command::CreateTask {
                workspace_id: workspace.clone(),
            })
            .unwrap();
        let (other_task, _) = runtime_graph(host, cx, window, true).await;
        host.commands.send(select(first)).unwrap();
        let (_, graph) = runtime_graph(host, cx, window, true).await;
        inject(
            cx,
            &graph,
            vec![
                payload("other-task", "Another task", &other_task, now + 16, true),
                payload("foreign-workspace", "Elsewhere", foreign, now + 17, true),
            ],
        );
        painted(window, cx);
        let task_row = window
            .update(cx, |view, _, _| view.probe_task_bounds(task))
            .unwrap();
        click_bounds(cx, task_row);
        check_trail(
            window,
            cx,
            "7: task row clears only its task",
            second,
            [2, before_scope + 2, 1, 0, 0, 0, 0],
        );
        check_trail(
            window,
            cx,
            "7: another task in same workspace stays unread",
            &other_task,
            [2, before_scope + 2, 1, 1, 1, 1, 0],
        );
        let bounds = painted(window, cx)[&format!("unread-workspace-{workspace}")];
        click(
            window,
            cx,
            f32::from(bounds.center().x),
            f32::from(bounds.center().y),
        );
        check_trail(
            window,
            cx,
            "7: workspace title clears only its workspace",
            &other_task,
            [1, before_scope + 2, 0, 0, 0, 0, 0],
        );
        check_trail(
            window,
            cx,
            "7: foreign workspace stays unread",
            foreign,
            [1, before_scope + 2, 1, 1, 1, 1, 0],
        );

        // Inactive canvases are real Runtime scenes, while only the last is loaded.
        let mut last_graph = graph;
        for _ in 0..10 {
            host.commands
                .send(Command::CreateCanvas {
                    task_id: task.clone(),
                })
                .unwrap();
            (_, last_graph) = runtime_graph(host, cx, window, true).await;
        }
        clear(cx);
        inject(
            cx,
            &last_graph,
            vec![payload("overflow", "Hidden canvas", first, now + 18, true)],
        );
        painted(window, cx);
        let bounds = painted(window, cx);
        assert!(bounds.contains_key("unread-tab-overflow"));
        assert!(!bounds.contains_key(&format!("unread-card-{session}")));
        check_trail(
            window,
            cx,
            "8: unopened canvas stays unread",
            first,
            [1, 1, 1, 1, 1, 1, 0],
        );
        window
            .update(cx, |view, _, cx| view.probe_open_overflow(cx))
            .unwrap();
        let bounds = painted(window, cx);
        assert!(bounds.contains_key(&format!("unread-hidden-tab-{scene}")));
        println!("8: GPUI overflow button and matching menu row painted both red dots");
        capture(cx, "04-overflow", &[("main", window.into())]).await;
        window
            .update(cx, |_, window, _| {
                crate::macos::hide_window(window).unwrap()
            })
            .unwrap();
        check_trail(
            window,
            cx,
            "8: window hidden with unread",
            first,
            [1, 1, 1, 1, 1, 1, 0],
        );
        if std::env::var_os("MATOU_UNREAD_TRAIL_CAPTURE").is_some() {
            window
                .update(cx, |_, window, _| window.activate_window())
                .unwrap();
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            assert!(
                window
                    .update(cx, |_, window, _| crate::macos::is_window_visible(window)
                        .unwrap())
                    .unwrap()
            );
            check_trail(
                window,
                cx,
                "8: same window reopened, unread retained",
                first,
                [1, 1, 1, 1, 1, 1, 0],
            );
        }

        host.commands
            .send(Command::CloseScene {
                scene_id: scene.into(),
                confirmed: true,
            })
            .unwrap();
        let (current, graph) = runtime_graph(host, cx, window, true).await;
        check_trail(
            window,
            cx,
            "10: archive clears all indicators and retains center history",
            first,
            [0, 1, 0, 0, 0, 0, 0],
        );
        let mut late = payload(
            "archived-late",
            "After canvas archival",
            first,
            now + 19,
            true,
        );
        late["cooldownKey"] = json!("late-stop");
        inject(cx, &graph, vec![late]);
        check_trail(
            window,
            cx,
            "10: already archived target only enters history",
            first,
            [0, 2, 0, 0, 0, 0, 0],
        );
        let mut unknown = payload(
            "no-location",
            "Insufficient routing",
            first,
            now + 19,
            false,
        );
        for field in ["workspaceId", "taskId", "sessionId"] {
            unknown.as_object_mut().unwrap().remove(field);
        }
        inject(cx, &graph, vec![unknown]);
        check_trail(
            window,
            cx,
            "10: missing routing remains in bell and center",
            first,
            [1, 3, 0, 0, 0, 0, 0],
        );
        if std::env::var_os("MATOU_UNREAD_TRAIL_CAPTURE").is_some() {
            assert_eq!(cx.update(|cx| window.is_active(cx)).unwrap(), Some(true));
            inject(
                cx,
                &graph,
                vec![payload(
                    "front",
                    "Already looking at this session",
                    &current,
                    now + 20,
                    true,
                )],
            );
            check_trail(
                window,
                cx,
                "4: current visible focused card, notification history only",
                &current,
                [1, 4, 0, 0, 0, 0, 0],
            );
        }
    }

    async fn verify(cx: &mut AsyncApp) {
        super::workspace::verify_merge();
        let launched = std::env::var_os("MATOU_UNREAD_TRAIL_CAPTURE").is_some();
        let launch = launched.then(|| {
            matou_native_desktop::instance_open::DesktopOptions::from_args(
                std::env::args_os().skip(1),
            )
            .unwrap()
            .runtime
        });
        let root = launch
            .as_ref()
            .map(|options| options.data_root.parent().unwrap().to_path_buf())
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../target")
                    .join(format!("notification-probe-{}", uuid::Uuid::new_v4()))
            });
        let active_path = root.join("project-active");
        let inactive_path = launch.as_ref().map_or_else(
            || root.join("project-background"),
            |options| options.workspace.clone(),
        );
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
        let mut host = RuntimeHost::start(options.clone()).unwrap();
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
        let initial = [("Claude Code", false), ("Focused", false)];
        assert_center(cx, false, 2, &initial);
        #[cfg(debug_assertions)]
        {
            let workspace_id = &background.workspace.as_ref().unwrap().id;
            let task_id = &background.task.as_ref().unwrap().id;
            window
                .update(cx, |view, _, cx| {
                    view.collapse_probe_workspace(workspace_id, cx)
                })
                .unwrap();
            let bounds = painted(window, cx);
            assert!(bounds.contains_key(&format!("unread-workspace-{workspace_id}")));
            assert!(!bounds.contains_key(&format!("unread-task-{task_id}")));
            check_trail(
                window,
                cx,
                "1: cross-workspace, collapsed workspace dot and stored task/tab/card",
                &background,
                [2, 2, 1, 1, 1, 1, 0],
            );
            capture(cx, "01-collapsed", &[("main", window.into())]).await;
        }
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
        assert_center(cx, true, 2, &initial);
        assert_eq!(
            layout(cx),
            closed_layout,
            "overlay must preserve both viewports"
        );
        bell(cx);
        assert_center(cx, false, 2, &initial);
        bell(cx);
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                assert!(window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx));
            })
            .unwrap();
        draw(window, cx);
        assert_center(cx, false, 2, &initial);
        bell(cx);
        click(window, cx, 410., 300.);
        draw(window, cx);
        assert_center(cx, false, 2, &initial);
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
        assert_center(cx, false, 1, &[("Focused", false)]);
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
            2,
            &[("Unknown target", false), ("Focused", false)],
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
        assert_center(cx, true, 1, &[("Focused", false)]);
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
        assert_center(cx, false, 0, &[]);
        println!("navigation: older payload resolved by the existing projection");
        #[cfg(debug_assertions)]
        verify_trail(&host, window, cx, &background, &sibling, &active).await;
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        host.shutdown().unwrap();
        #[cfg(debug_assertions)]
        {
            let mut restarted_host = RuntimeHost::start(options).unwrap();
            let (commands, _sink) = runtime_bridge::probe_channel();
            let (_events, incoming) = async_channel::unbounded();
            let fresh = cx
                .open_window(
                    WindowOptions {
                        show: false,
                        focus: false,
                        ..Default::default()
                    },
                    |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
                )
                .unwrap();
            let (restored, _) = runtime_graph(&restarted_host, cx, fresh, true).await;
            check_trail(
                fresh,
                cx,
                "8: fresh Workspace and restarted Runtime, same data root",
                &restored,
                [0; 7],
            );
            fresh
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
            restarted_host.shutdown().unwrap();
        }
        if !launched {
            std::fs::remove_dir_all(root).unwrap();
        }
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
