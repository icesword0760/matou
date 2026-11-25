//! Hidden settings integration probe. --visible-state displays a real GPUI
//! window with a scripted transport for the transient loading and empty states.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

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
    use matou_native_runtime::protocol::ClaudeRoleAssignmentsReply;

    include!("../src/workspace.rs");

    impl Workspace {
        pub(super) fn expose_settings_fixture(&mut self) {
            self.runtime_generation = 1;
            self.transport_connected = true;
            self.domain_ready = true;
            self.hierarchy = Some(serde_json::from_value(serde_json::json!({
                "navigation": { "windowId": "settings-probe", "taskByWorkspace": {}, "sceneByTask": {}, "sessionByScene": {} }
            })).unwrap());
        }

        pub(super) fn set_locale_data_root(&mut self, path: PathBuf) {
            self.data_root = Some(path);
        }

        pub(super) fn probe_locale(&self) -> InterfaceLocale {
            self.locale
        }

        pub(super) fn load_theme_fixture(&mut self, root: PathBuf, window: &Window) {
            self.theme =
                ThemeState::new(read_theme_preference(&root).unwrap(), window.appearance());
            self.data_root = Some(root);
        }

        pub(super) fn load_capture_fixture(&mut self, root: PathBuf, window: &Window) {
            self.load_theme_fixture(root, window);
            self.locale = InterfaceLocale::Chinese;
            self.locale_preference = LocalePreference::Chinese;
        }

        pub(super) fn claude_preview_card_header() -> Div {
            Self::session_card_header_themed(
                ui_text("Claude Code"),
                div(),
                &matou_native_desktop::theme_tokens::LIGHT,
                Self::session_provider_mark(
                    "preview",
                    Some("claude-code"),
                    InterfaceLocale::Chinese,
                    false,
                ),
                None,
                None,
                None,
            )
        }

        pub(super) fn arm_theme_terminal(&mut self, cx: &mut Context<Self>) {
            self.terminals.insert(
                "settings-theme-terminal".into(),
                cx.new(TerminalSurface::new),
            );
        }

        pub(super) fn time_zone_reply(
            &mut self,
            reply: matou_native_runtime::protocol::TimeZoneSettingsReply,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::TimeZoneSettings(reply),
                },
                window,
                cx,
            );
        }

        pub(super) fn time_zone_read_only(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            self.apply(
                BridgeEvent::Connection {
                    generation: 1,
                    phase: ConnectionPhase::ReadOnly { pending: None },
                },
                window,
                cx,
            );
        }

        pub(super) fn diagnostic_reply_for_probe(
            &mut self,
            reply: Value,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Graph(reply),
                },
                window,
                cx,
            );
        }

        pub(super) fn model_catalog_reply(
            &mut self,
            reply: Value,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Graph(serde_json::json!({ "claudeModelCatalogResult": reply })),
                },
                window,
                cx,
            );
        }

        pub(super) fn assignment_reply(
            &mut self,
            reply: ClaudeRoleAssignmentsReply,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Graph(
                        serde_json::json!({ "claudeRoleAssignmentsResult": reply }),
                    ),
                },
                window,
                cx,
            );
        }

        pub(super) fn compaction_reply(
            &mut self,
            reply: ClaudeModelPreferencesReply,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Graph(
                        serde_json::json!({ "claudeRoleAssignmentsResult": reply }),
                    ),
                },
                window,
                cx,
            );
        }

        pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            self.toggle_settings(window, cx);
        }

        pub(super) fn arm_settings_entry(&self, window: &mut Window) {
            window.focus(&self.control_focus["settings"]);
        }

        pub(super) fn settings_entity(&self) -> Option<Entity<SettingsShell>> {
            self.settings.as_ref().map(|(settings, _)| settings.clone())
        }

        pub(super) fn provider_reply(
            &mut self,
            reply: ProviderSettingsReply,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Graph(serde_json::json!({ "providerSettingsResult": reply })),
                },
                window,
                cx,
            );
        }

        pub(super) fn arm_terminal_focus(&mut self, cx: &mut Context<Self>) -> FocusHandle {
            let terminal = cx.new(TerminalSurface::new);
            let focus = terminal.read(cx).focus.clone();
            self.order.push("settings-probe-terminal".into());
            self.terminals
                .insert("settings-probe-terminal".into(), terminal);
            focus
        }

        pub(super) fn clear_terminal_focus_fixture(&mut self) {
            self.order.clear();
            self.terminals.clear();
        }
    }
}

#[cfg(all(target_os = "macos", debug_assertions))]
mod probe {
    use gpui::*;
    use matou_native_core::dispatch_model_catalog::{CATALOG_FILENAME, DispatchModelCatalog};
    use matou_native_core::dispatch_roles::DispatchRolesStore;
    use matou_native_core::provider_config::{ProviderConfigLocale, ProviderConfigStore};
    use matou_native_core::sessions::{InterfaceLocale, LocalePreference, read_locale_preference};
    use matou_native_desktop::theme_tokens::{
        AppTheme, ThemePreference, read_theme_preference, write_theme_preference,
    };
    use matou_native_runtime::protocol::{
        ClaudeModelPreferencesReply, ClaudeRoleAssignmentsReply, ProviderSettingsReply,
        ProviderSettingsResult, SubscriptionDetails, SubscriptionLoginStatus,
        SubscriptionLoginView, SubscriptionModelsView, SubscriptionQuota, SubscriptionSettingsView,
        TimeZoneSettingsError, TimeZoneSettingsReply, TimeZoneSettingsSnapshot,
    };
    use matou_native_runtime::subscription_launch::SubscriptionProvider;
    use serde_json::{Value, json};
    use std::{path::PathBuf, time::Duration};

    use crate::{
        assets::NativeAssets,
        provider_settings::{ProviderDraft, ProviderSettings, ProviderUpdate},
        runtime_bridge::{self, Command, ProbeReceiver},
        settings_shell::claude_role_assignments::{AssignmentAction, AssignmentStatus},
        settings_shell::{
            EditorStep, ProviderEditorProbe, SettingsPage, SettingsShell, probe_language_zone,
            probe_provider_editor,
        },
        workspace::Workspace,
    };

    fn business(sink: &ProbeReceiver) -> Result<Command, std::sync::mpsc::TryRecvError> {
        loop {
            match sink.try_recv()? {
                Command::OpenSubscriptionSettings { .. }
                | Command::CloseSubscriptionSettings { .. }
                | Command::GetClaudeModelPreferences { .. } => {}
                command => return Ok(command),
            }
        }
    }

    fn request_id(sink: &ProbeReceiver) -> u64 {
        match business(sink).expect("settings should send a read") {
            Command::GetProviderSettings { request_id } => request_id,
            _ => panic!("unexpected command while waiting for provider settings"),
        }
    }

    fn assignment_request_id(sink: &ProbeReceiver) -> u64 {
        match business(sink).expect("settings should read assignments") {
            Command::GetClaudeRoleAssignments { request_id } => request_id,
            _ => panic!("unexpected command while waiting for role assignments"),
        }
    }

    fn result(request_id: u64, result: ProviderSettingsResult) -> ProviderSettingsReply {
        ProviderSettingsReply { request_id, result }
    }

    fn snapshot(rows: Vec<Value>, active_id: &str) -> ProviderSettingsResult {
        ProviderSettingsResult::Snapshot(json!({
            "providers": { "claude-code": rows },
            "activeProviderIds": { "claude-code": active_id }
        }))
    }

    fn row(id: &str, name: &str) -> Value {
        json!({
            "id": id, "name": name, "cli": "claude-code",
            "endpoint": "https://api.example.invalid/v1",
            "model": "probe-model", "hasApiKey": true
        })
    }

    #[::core::prelude::v1::test]
    fn inactive_official_provider_requests_catalog_before_role_assignments_load() {
        use crate::settings_shell::claude_role_assignments::ClaudeAssignments;

        let (commands, sink) = runtime_bridge::probe_channel();
        let mut assignments = ClaudeAssignments::new(commands.clone());
        let mut providers = ProviderSettings::new(commands);
        providers.load();
        let request = request_id(&sink);
        providers.receive(result(
            request,
            snapshot(
                vec![
                    json!({
                        "id": "anthropic-official", "builtIn": true,
                        "endpoint": "https://api.anthropic.com",
                    }),
                    row("deepseek", "DeepSeek"),
                ],
                "deepseek",
            ),
        ));
        assert_eq!(providers.active_id(), Some("deepseek"));
        let request = assignments
            .load_catalog(&providers, false)
            .expect("an inactive official provider still needs its catalog");
        assert!(
            matches!(business(&sink).unwrap(), Command::GetClaudeModelCatalog { request_id } if request_id == request)
        );
        assert_eq!(assignments.load_catalog(&providers, false), None);
    }

    #[::core::prelude::v1::test]
    fn model_catalog_is_not_requested_without_an_eligible_official_provider() {
        use crate::settings_shell::claude_role_assignments::ClaudeAssignments;

        let (commands, sink) = runtime_bridge::probe_channel();
        let mut assignments = ClaudeAssignments::new(commands.clone());
        let mut providers = ProviderSettings::new(commands);
        providers.load();
        let request = request_id(&sink);
        providers.receive(result(
            request,
            snapshot(
                vec![
                    row("deepseek", "DeepSeek"),
                    json!({
                        "id": "redirected", "builtIn": true,
                        "endpoint": "https://proxy.example.invalid",
                    }),
                    json!({
                        "id": "explicit", "builtIn": true,
                        "endpoint": "https://api.anthropic.com", "modelMetadata": [],
                    }),
                ],
                "deepseek",
            ),
        ));
        assert_eq!(assignments.load_catalog(&providers, false), None);
        assert!(business(&sink).is_err());
    }

    fn verify_commands() {
        let (commands, sink) = runtime_bridge::probe_channel();
        let mut settings = ProviderSettings::new(commands);
        settings.load();
        let load_id = request_id(&sink);
        assert!(settings.loading());
        settings.receive(result(load_id + 1, snapshot(vec![], "")));
        assert!(settings.loading(), "stale reply ended the current request");
        settings.receive(result(
            load_id,
            ProviderSettingsResult::Error("隔离读取失败".into()),
        ));
        assert!(!settings.loading());
        assert_eq!(settings.load_error(), Some("隔离读取失败"));
        settings.load();
        let load_id = request_id(&sink);
        let a = row("a", "配置甲");
        let b = row("b", "配置乙");
        let c = row("c", "配置丙");
        settings.receive(result(
            load_id,
            snapshot(vec![a.clone(), b.clone(), c.clone()], "b"),
        ));
        assert_eq!(settings.providers(), &[b.clone(), a.clone(), c.clone()]);
        assert!(settings.load_error().is_none());

        settings.switch("a");
        assert_eq!(settings.switching(), Some("a"));
        let Command::SetActiveProvider {
            request_id: switch_id,
            cli,
            id,
        } = business(&sink).expect("switch should send an activation")
        else {
            panic!("unexpected command while checking activation");
        };
        assert_eq!(cli, "claude-code");
        assert_eq!(id, "a");
        settings.receive(result(
            switch_id,
            ProviderSettingsResult::Error("隔离切换失败".into()),
        ));
        assert_eq!(settings.active_id(), Some("b"));
        assert_eq!(settings.notice(), Some(("需要检查", "隔离切换失败", true)));
        assert_eq!(settings.retry_switch_id(), Some("a"));
        settings.switch("a");
        let Command::SetActiveProvider {
            request_id: switch_id,
            ..
        } = business(&sink).unwrap()
        else {
            panic!("retry did not send an activation");
        };
        settings.receive(result(switch_id, snapshot(vec![a.clone(), b, c], "a")));
        assert_eq!(settings.active_id(), Some("a"));
        assert_eq!(settings.providers()[0], a);

        let mut draft = ProviderDraft::from_provider(&a);
        assert!(draft.api_key.is_empty());
        draft.name = "确认保留草稿".into();
        draft.model_mapping[0] = "probe-opus".into();
        draft.effort_mapping[0] = "medium".into();
        settings.save(&draft, false).unwrap();
        assert!(settings.saving());
        let Command::SaveProvider {
            request_id: save_id,
            input,
        } = business(&sink).unwrap()
        else {
            panic!("save did not send a provider write");
        };
        assert!(input.get("apiKey").is_none(), "blank edit must omit key");
        assert_eq!(input["id"], "a");
        assert_eq!(input["name"], "确认保留草稿");
        assert_eq!(input["modelMapping"]["opus"], "probe-opus");
        assert_eq!(input["effortMapping"]["opus"], "medium");
        let error = settings.receive(result(
            save_id,
            ProviderSettingsResult::Error("隔离写入失败原文".into()),
        ));
        assert!(
            matches!(error, ProviderUpdate::SaveFailed(message) if message == "隔离写入失败原文")
        );
        assert!(!settings.saving());
        assert_eq!(draft.name, "确认保留草稿");
        assert_eq!(settings.providers()[0]["name"], "配置甲");

        settings.save(&draft, false).unwrap();
        let Command::SaveProvider {
            request_id: save_id,
            ..
        } = business(&sink).unwrap()
        else {
            panic!("retry did not send a provider write");
        };
        assert!(matches!(
            settings.receive(result(
                save_id,
                ProviderSettingsResult::Provider(row("a", &draft.name))
            )),
            ProviderUpdate::Saved
        ));
        assert_eq!(settings.providers()[0]["name"], draft.name);

        let mut addition = draft.clone();
        addition.id = None;
        addition.has_api_key = false;
        addition.api_key = "isolated-probe-key".into();
        settings.save(&addition, true).unwrap();
        let Command::SaveProvider {
            request_id: add_id,
            input,
        } = business(&sink).unwrap()
        else {
            panic!("add did not send a provider write");
        };
        assert!(input.get("id").is_none());
        assert!(matches!(
            settings.receive(result(
                add_id,
                ProviderSettingsResult::Provider(row("new", &draft.name))
            )),
            ProviderUpdate::None
        ));
        let Command::SetActiveProvider {
            request_id: switch_id,
            id,
            ..
        } = business(&sink).unwrap()
        else {
            panic!("add-and-use should send activation after save");
        };
        assert_eq!(id, "new");
        let failure = settings.receive(result(
            switch_id,
            ProviderSettingsResult::Error("切换失败原文".into()),
        ));
        assert!(
            matches!(failure, ProviderUpdate::SaveFailed(message) if message == "切换失败原文")
        );
        assert_eq!(settings.take_committed_provider().unwrap()["id"], "new");
        assert_eq!(settings.active_id(), Some("a"));

        // A read after reopening or retrying may return an empty snapshot.
        settings.load();
        let load_id = request_id(&sink);
        settings.receive(result(load_id, snapshot(vec![], "")));
        assert!(settings.providers().is_empty());
        println!(
            "Provider commands: loading, outdated reply, read error, stable order, switch and retry, save and retry, empty snapshot"
        );
    }

    fn draw(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
    }

    fn press(window: WindowHandle<Workspace>, key: &str, cx: &mut AsyncApp) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                assert!(
                    window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx),
                    "hidden settings window did not handle {key}"
                );
            })
            .unwrap();
    }

    fn click_hidden_view(
        window: WindowHandle<Workspace>,
        position: Point<Pixels>,
        cx: &mut AsyncApp,
    ) {
        use cocoa::{
            appkit::{NSEvent, NSEventModifierFlags, NSEventType},
            base::{id, nil},
            foundation::{NSInteger, NSPoint},
        };
        use objc::{msg_send, sel, sel_impl};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let (view, location) = window
            .update(cx, |_, window, _| {
                let RawWindowHandle::AppKit(handle) =
                    HasWindowHandle::window_handle(window).unwrap().as_raw()
                else {
                    unreachable!("hidden macOS window")
                };
                let location = NSPoint::new(
                    f64::from(position.x),
                    f64::from(window.viewport_size().height - position.y),
                );
                (handle.ns_view.as_ptr() as id, location)
            })
            .unwrap();
        // Send to the hidden view directly, bypassing the desktop event queue.
        unsafe {
            let native: id = msg_send![view, window];
            let number: NSInteger = msg_send![native, windowNumber];
            let event = |kind| {
                NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
                    nil, kind, location, NSEventModifierFlags::empty(), 0., number, nil, 0, 1, 1.,
                )
            };
            let _: () = msg_send![view, mouseDown: event(NSEventType::NSLeftMouseDown)];
            let _: () = msg_send![view, mouseUp: event(NSEventType::NSLeftMouseUp)];
        }
    }

    fn scroll_hidden_view(
        window: WindowHandle<Workspace>,
        position: Point<Pixels>,
        cx: &mut AsyncApp,
    ) {
        use cocoa::{
            appkit::NSEvent,
            base::{id, nil},
        };
        use core_foundation::base::CFRelease;
        use core_graphics::{
            display::CGDisplay,
            event::{CGScrollEventUnit, ScrollEventUnit},
            geometry::CGPoint,
        };
        use objc::{msg_send, sel, sel_impl};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
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
        }

        let (view, height) = window
            .update(cx, |_, window, _| {
                let RawWindowHandle::AppKit(handle) =
                    HasWindowHandle::window_handle(window).unwrap().as_raw()
                else {
                    unreachable!("hidden macOS window")
                };
                (handle.ns_view.as_ptr() as id, window.viewport_size().height)
            })
            .unwrap();
        // Deliver the wheel event straight to the probe view, without posting
        // anything to the desktop event queue.
        unsafe {
            let event = CGEventCreateScrollWheelEvent2(
                std::ptr::null(),
                ScrollEventUnit::PIXEL,
                1,
                -292,
                0,
                0,
            );
            assert!(!event.is_null());
            CGEventSetLocation(
                event,
                CGPoint::new(
                    f64::from(position.x),
                    CGDisplay::main().bounds().size.height - f64::from(height - position.y),
                ),
            );
            let native = NSEvent::eventWithCGEvent_(nil, event);
            CFRelease(event);
            let _: () = msg_send![view, scrollWheel: native];
        }
    }

    fn open_window(
        visible_state: Option<&str>,
        cx: &mut App,
    ) -> (WindowHandle<Workspace>, ProbeReceiver) {
        open_window_at(visible_state, 1200., cx)
    }

    fn open_window_at(
        visible_state: Option<&str>,
        width: f32,
        cx: &mut App,
    ) -> (WindowHandle<Workspace>, ProbeReceiver) {
        open_window_sized(visible_state, size(px(width), px(780.)), cx)
    }

    fn open_window_sized(
        visible_state: Option<&str>,
        dimensions: Size<Pixels>,
        cx: &mut App,
    ) -> (WindowHandle<Workspace>, ProbeReceiver) {
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_events, incoming) = async_channel::unbounded();
        let bounds = Bounds::centered(None, dimensions, cx);
        let window = cx
            .open_window(
                WindowOptions {
                    show: visible_state.is_some(),
                    focus: visible_state.is_some(),
                    titlebar: Some(TitlebarOptions {
                        title: Some(
                            format!("Matou Settings Probe {}", visible_state.unwrap_or("hidden"))
                                .into(),
                        ),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.), px(16.))),
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let mut workspace = Workspace::new(commands, incoming, window, cx);
                        workspace.expose_settings_fixture();
                        workspace
                    })
                },
            )
            .unwrap();
        if visible_state.is_none() {
            window
                .update(cx, |_, window, _| {
                    assert!(!crate::macos::is_window_visible(window).unwrap());
                })
                .unwrap();
        }
        (window, sink)
    }

    fn check_shell(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        check: impl FnOnce(&SettingsShell, &App),
    ) {
        window
            .update(cx, |view, _, cx| {
                check(view.settings_entity().unwrap().read(cx), cx)
            })
            .unwrap();
    }

    fn update_shell(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        update: impl FnOnce(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>),
    ) {
        window
            .update(cx, |view, window, cx| {
                view.settings_entity()
                    .unwrap()
                    .update(cx, |shell, cx| update(shell, window, cx));
            })
            .unwrap();
    }

    fn probe_root(test: &str) -> std::path::PathBuf {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target")
            .join(format!("settings-{test}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn seed_dispatch_catalog(root: &std::path::Path) {
        // Keep the GUI probe offline while exercising the real cache reader.
        let mut catalog = serde_json::to_value(DispatchModelCatalog::default()).unwrap();
        catalog["refreshedAt"] = serde_json::json!(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        std::fs::write(
            root.join(CATALOG_FILENAME),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
    }

    fn verify_dispatch_window(cx: &mut AsyncApp) {
        let root = probe_root("dispatch");
        let config = DispatchRolesStore::for_data_root(&root);
        seed_dispatch_catalog(&root);
        // A directory at the file path gives a deterministic read failure.
        std::fs::create_dir_all(config.path()).unwrap();
        let (window, _sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        window
            .update(cx, |view, window, cx| view.open_settings(window, cx))
            .unwrap();
        update_shell(window, cx, |shell, _, _| {
            shell.configure_data_root(Some(root.clone()));
        });
        let select_dispatch = |cx: &mut AsyncApp| {
            draw(window, cx);
            update_shell(window, cx, |shell, window, cx| {
                shell.select_dispatch_for_probe(window, cx)
            });
            draw(window, cx);
        };
        select_dispatch(cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.probe_dispatch_load_failed())
        });
        std::fs::remove_dir(config.path()).unwrap();
        update_shell(window, cx, |shell, window, _| {
            shell.probe_dispatch_retry_focus(window)
        });
        press(window, "enter", cx);
        draw(window, cx);
        check_shell(window, cx, |shell, _| {
            assert!(!shell.probe_dispatch_load_failed())
        });

        // Navigate the actual dispatch panel, including its number-input and
        // Save handlers, without touching the native desktop or reading HOME.
        let focus_number = |index, cx: &mut AsyncApp| {
            update_shell(window, cx, |shell, window, cx| {
                shell.probe_dispatch_focus(Some(index), window, cx)
            });
        };
        focus_number(0, cx);
        draw(window, cx);
        press(window, "tab", cx);
        update_shell(window, cx, |shell, window, cx| {
            assert!(shell.probe_dispatch_number_focused(1, window, cx));
        });
        press(window, "shift-tab", cx);
        update_shell(window, cx, |shell, window, cx| {
            assert!(shell.probe_dispatch_number_focused(0, window, cx));
        });
        let edit_number = |cx: &mut AsyncApp, digits: &[&str]| {
            draw(window, cx);
            press(window, "cmd-a", cx);
            for digit in digits {
                press(window, digit, cx);
            }
            draw(window, cx);
        };
        let save = |cx: &mut AsyncApp| {
            update_shell(window, cx, |shell, window, cx| {
                shell.probe_dispatch_focus(None, window, cx)
            });
            draw(window, cx);
            AnyWindowHandle::from(window)
                .update(cx, |_, window, cx| {
                    window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
                })
                .unwrap();
            draw(window, cx);
        };
        edit_number(cx, &["0"]);
        save(cx);
        assert!(
            !config.path().exists(),
            "invalid input wrote the configuration"
        );
        focus_number(0, cx);
        edit_number(cx, &["1", "3"]);
        std::fs::create_dir(config.path()).unwrap();
        save(cx);
        assert!(config.path().is_dir(), "failed save replaced the fixture");
        std::fs::remove_dir(config.path()).unwrap();
        press(window, "enter", cx);
        draw(window, cx);
        assert_eq!(config.snapshot().unwrap().config.idle_min, 13.);

        update_shell(window, cx, |shell, window, _| {
            shell.probe_settings_nav_focus(SettingsPage::Language, window);
        });
        press(window, "enter", cx);
        select_dispatch(cx);
        // The freshly mounted form should still show 13. Changing a different
        // number and saving verifies that 13 was restored into the draft.
        focus_number(1, cx);
        edit_number(cx, &["4", "9"]);
        save(cx);
        let saved = config.snapshot().unwrap().config;
        assert_eq!((saved.idle_min, saved.max_min), (13., 49.));
        update_shell(window, cx, |shell, window, cx| {
            shell.probe_dispatch_model_focus(2, window, cx)
        });
        draw(window, cx);
        press(window, "enter", cx);
        draw(window, cx);
        press(window, "escape", cx);
        draw(window, cx);
        press(window, "enter", cx);
        draw(window, cx);
        for key in ["o", "s", "s"] {
            press(window, key, cx);
        }
        draw(window, cx);
        press(window, "enter", cx);
        draw(window, cx);
        save(cx);
        let saved = config.snapshot().unwrap().config;
        assert_eq!(saved.agents[2].model, "gpt-oss-120b-medium");
        assert_eq!(saved.agents[2].effort, "medium");
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        println!(
            "Hidden GPUI dispatch: open, retry read and write failures, Tab focus, invalid number blocked, save, switch pages, model search with effort linkage and re-read"
        );
    }

    fn verify_narrow_dispatch_scroll(cx: &mut AsyncApp) {
        let root = probe_root("dispatch-narrow");
        seed_dispatch_catalog(&root);
        let (window, _sink) = cx.update(|cx| open_window_at(None, 720., cx)).unwrap();
        window
            .update(cx, |view, window, cx| view.open_settings(window, cx))
            .unwrap();
        update_shell(window, cx, |shell, window, cx| {
            shell.configure_data_root(Some(root.clone()));
            shell.select_dispatch_for_probe(window, cx);
        });
        draw(window, cx);
        draw(window, cx);
        let bounds = window
            .update(cx, |view, _, cx| {
                view.settings_entity()
                    .unwrap()
                    .read(cx)
                    .probe_dispatch_agent_scroll(cx)
                    .0
            })
            .unwrap();
        assert!(
            bounds.size.height > px(200.),
            "narrow agent scroll viewport is {bounds:?}"
        );
        scroll_hidden_view(
            window,
            point(bounds.left() + px(32.), bounds.top() + px(50.)),
            cx,
        );
        draw(window, cx);
        check_shell(window, cx, |shell, cx| {
            assert!(
                shell.probe_dispatch_agent_scroll(cx).1 < px(0.),
                "narrow cards did not scroll"
            );
        });
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        println!(
            "Hidden GPUI dispatch 720pt: agent cards scroll beneath the policy above the saved actions"
        );
    }

    fn verify_theme_window(cx: &mut AsyncApp) {
        let root = probe_root("theme");
        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        write_theme_preference(&root, ThemePreference::Light).unwrap();
        window
            .update(cx, |view, window, cx| {
                view.load_theme_fixture(root.clone(), window);
                view.arm_theme_terminal(cx);
                view.open_settings(window, cx);
            })
            .unwrap();
        request_id(&sink);
        assignment_request_id(&sink);
        update_shell(window, cx, |shell, window, cx| {
            shell.probe_language_page(window, cx)
        });
        assert!(matches!(
            business(&sink).unwrap(),
            Command::GetTimeZoneSettings { .. }
        ));
        draw(window, cx);

        let verify = |preference, expected: AppTheme, cx: &mut AsyncApp| {
            assert_eq!(read_theme_preference(&root).unwrap(), preference);
            let frame = probe_language_zone(|| draw(window, cx));
            let canvas = Some(rgba(expected.palette().workspace.canvas));
            assert_eq!(frame.settings_canvas, canvas);
            assert_eq!(frame.workspace_canvas, canvas);
            let surface = frame
                .language_zone_surface
                .expect("language/time-zone surface");
            let card = frame.language_zone_card.expect("language options card");
            match expected {
                AppTheme::Light => {
                    assert_eq!(surface, rgb(0xf7f8fa));
                    assert_eq!(card, rgb(0xfbfcfd));
                }
                AppTheme::Dark => {
                    assert_eq!(surface, rgba(expected.palette().workspace.canvas));
                    assert_eq!(card, rgba(expected.palette().settings.language_list));
                    for color in [surface, card] {
                        assert_ne!(color, rgb(0xffffff));
                        assert_ne!(color, rgb(0xeae9e4));
                    }
                }
            }
        };
        let choose = |option: usize, cx: &mut AsyncApp| {
            // The time-zone read is still pending, so the theme radio list
            // immediately follows the English radio in the tab order.
            update_shell(window, cx, |shell, window, _| {
                shell.probe_language_focus("english", window);
            });
            for _ in 0..=option {
                press(window, "tab", cx);
            }
            press(window, "enter", cx);
        };
        verify(ThemePreference::Light, AppTheme::Light, cx);
        choose(2, cx);
        verify(ThemePreference::Dark, AppTheme::Dark, cx);
        assert!(matches!(
            business(&sink).unwrap(),
            Command::Theme { session_id, theme }
                if session_id == "settings-theme-terminal" && theme == AppTheme::Dark.palette().terminal
        ));

        let appearance = window
            .update(cx, |_, window, _| window.appearance())
            .unwrap();
        choose(0, cx);
        verify(
            ThemePreference::System,
            ThemePreference::System.resolve(appearance),
            cx,
        );
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        println!(
            "Hidden GPUI theme: Light → Dark → System, language/time-zone surfaces, theme.json and terminal theme command"
        );
    }

    fn verify_provider_editors(cx: &mut AsyncApp) {
        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        let api = json!({
            "id": "api", "name": "隔离 API", "cli": "claude-code", "hasApiKey": true,
            "endpoint": "https://api.deepseek.com/anthropic", "model": "saved-model",
            "modelMapping": {"opus": "saved-model", "sonnet": "", "haiku": "unlisted-model"},
            "modelMetadata": [{"id": "saved-model", "displayName": "Saved name", "contextWindow": 200000}],
        });
        let subscription = json!({
            "id": "subscription", "name": "隔离 ChatGPT", "cli": "claude-code", "kind": "subscription",
            "model": "chat-a",
            "modelMapping": {"opus": "chat-a", "sonnet": "chat-b", "haiku": "chat-a"},
            "effortMapping": {"opus": "max", "sonnet": "low", "haiku": "follow"},
            "subscription": {
                "provider": "codex",
                "account": {"status": "ready"},
                "modelMetadata": [
                    {"id": "chat-a", "displayName": "Chat A", "contextWindow": 272000,
                     "maxContextWindow": 512000, "supportedEfforts": ["high"]},
                    {"id": "chat-b", "displayName": "Chat B", "contextWindow": 400000,
                     "supportedEfforts": ["low", "high", "max"]},
                    {"id": "image-only"}, {"id": "codex-auto-review"},
                ],
            },
        });
        let reply = |cx: &mut AsyncApp, request, response| {
            window
                .update(cx, |view, window, cx| {
                    view.provider_reply(result(request, response), window, cx);
                })
                .unwrap();
        };
        window
            .update(cx, |view, window, cx| view.open_settings(window, cx))
            .unwrap();
        let request = request_id(&sink);
        assignment_request_id(&sink);
        reply(
            cx,
            request,
            snapshot(vec![api.clone(), subscription.clone()], "api"),
        );

        let frame = |cx: &mut AsyncApp| probe_provider_editor(|| draw(window, cx));
        let open = |cx: &mut AsyncApp, id: &str| {
            draw(window, cx);
            update_shell(window, cx, |shell, window, _| {
                shell.diagnostics_focus_provider_edit(id, window)
            });
            press(window, "enter", cx);
            frame(cx)
        };
        let focus = |cx: &mut AsyncApp, control| {
            draw(window, cx);
            update_shell(window, cx, |shell, window, _| {
                shell.probe_provider_editor_focus(control, window)
            });
        };
        let activate = |cx: &mut AsyncApp, control| {
            focus(cx, control);
            press(window, "enter", cx);
            frame(cx)
        };
        let click = |cx: &mut AsyncApp, view: &ProviderEditorProbe, control: &str| {
            click_hidden_view(window, view.bounds[control].center(), cx);
            frame(cx)
        };
        let text = |view: &ProviderEditorProbe, control: &str, expected: &str| {
            assert!(
                view.labels
                    .get(control)
                    .is_some_and(|label| label.contains(expected)),
                "expected {control} to include {expected:?}, got {:?}",
                view.labels.get(control)
            );
        };
        let mapping_options = |view: &ProviderEditorProbe, expected: &[&str]| {
            let mut options: Vec<_> = view
                .labels
                .keys()
                .filter_map(|key| key.strip_prefix("mapping-option-"))
                .collect();
            options.sort_unstable();
            assert_eq!(options, expected);
        };
        let save_command = || match business(&sink).unwrap() {
            Command::SaveProvider { request_id, input } => (request_id, input),
            _ => panic!("expected provider write"),
        };

        let view = open(cx, "api");
        text(&view, "subtitle", "用于 Claude Code");
        text(&view, "protocol", "Anthropic Messages");
        text(&view, "key-label", "API Key");
        text(&view, "key-link", "获取密钥");
        assert!(
            (f32::from(view.bounds["key-label"].center().y - view.bounds["key-link"].center().y))
                .abs()
                <= 1.0,
            "API Key and its link must share one heading row"
        );
        assert!(view.bounds["key-link"].left() > view.bounds["key-label"].left());
        text(&view, "model-detail", "Saved name");
        text(&view, "model-detail", "200,000 tokens");
        assert!(view.bounds["context"].top() > view.bounds["mapping-3"].bottom());
        text(&view, "context-unknown", "沿用 CLI 默认设置");

        let view = activate(cx, "fetch");
        text(&view, "fetch", "正在获取");
        let Command::FetchProviderModels { request_id, input } = business(&sink).unwrap() else {
            panic!("expected model discovery request");
        };
        assert_eq!(
            input,
            json!({
                "cli": "claude-code", "providerId": "api", "endpoint": "https://api.deepseek.com/anthropic"
            })
        );
        reply(
            cx,
            request_id,
            ProviderSettingsResult::Models(json!({
                "models": ["saved-model", "other-model"],
                "modelMetadata": [
                    {"id": "saved-model", "displayName": "Saved name", "contextWindow": 200000},
                    {"id": "other-model", "displayName": "Unique display", "contextWindow": 512000,
                     "supportedEfforts": ["high"]},
                ]
            })),
        );
        let view = frame(cx);
        text(&view, "option-saved-model", "✓");
        let view = activate(cx, "close");
        text(&view, "confirmation", "有尚未保存的修改");
        activate(cx, "keep-editing");
        focus(cx, "search");
        for key in ["u", "n", "i", "q"] {
            press(window, key, cx);
        }
        let view = frame(cx);
        assert!(!view.bounds.contains_key("option-saved-model"));
        text(&view, "option-other-model", "Unique display");
        press(window, "enter", cx);
        let view = frame(cx);
        assert!(!view.bounds.contains_key("option-other-model"));
        text(&view, "model-detail", "other-model");
        text(&view, "model-detail", "512,000 tokens");
        text(&view, "model-source", "本次获取");

        assert_eq!(view.labels["mapping-1"], "请选择模型");
        assert_eq!(view.labels["mapping-2"], "请选择模型");
        let view = activate(cx, "mapping-2");
        mapping_options(&view, &["manual", "other-model", "saved-model"]);
        press(window, "escape", cx);
        let view = activate(cx, "mapping-1");
        mapping_options(&view, &["manual", "other-model", "saved-model"]);
        text(
            &view,
            "mapping-option-other-model",
            "other-model · 512,000 tokens",
        );
        text(&view, "mapping-option-manual", "手动填写模型 ID…");
        let view = click(cx, &view, "mapping-option-manual");
        text(&view, "mapping-list-1", "从列表选择");
        focus(cx, "manual-1");
        press(window, "cmd-a", cx);
        for key in ["u", "n", "k"] {
            press(window, key, cx);
        }
        let view = activate(cx, "mapping-list-1");
        text(&view, "mapping-1", "unk");
        text(&view, "mapping-1", "未在列表中");
        let view = activate(cx, "mapping-1");
        mapping_options(&view, &["manual", "other-model", "saved-model", "unk"]);
        text(&view, "mapping-option-unk", "✓");
        press(window, "escape", cx);
        let view = activate(cx, "effort-1");
        click(cx, &view, "effort-option-high");
        activate(cx, "context-512k");
        let view = activate(cx, "close");
        text(&view, "confirmation", "有尚未保存的修改");
        activate(cx, "keep-editing");
        activate(cx, "save");
        let (request, input) = save_command();
        assert_eq!(input["model"], "other-model");
        assert_eq!(input["modelMapping"]["fable"], "unk");
        assert_eq!(input["effortMapping"]["fable"], "high");
        assert_eq!(input["modelMapping"]["opus"], "saved-model");
        assert_eq!(input["modelMapping"]["sonnet"], "");
        assert_eq!(input["contextWindowMode"], "512k");
        assert_eq!(input["modelMetadata"].as_array().unwrap().len(), 2);
        let mut saved_api = api;
        saved_api["model"] = input["model"].clone();
        saved_api["contextWindowMode"] = input["contextWindowMode"].clone();
        reply(
            cx,
            request,
            ProviderSettingsResult::Provider(saved_api.clone()),
        );

        let view = open(cx, "subscription");
        text(&view, "title", "编辑供应商");
        text(&view, "subtitle", "用于 Claude Code");
        text(&view, "provider-name", "供应商名称");
        assert_eq!(view.labels["mapping-1"], "请选择模型");
        for role in 0..4 {
            assert!(view.bounds.contains_key(format!("mapping-{role}").as_str()));
            assert!(view.bounds.contains_key(format!("effort-{role}").as_str()));
        }
        assert!(!view.bounds.contains_key("key-label"));
        assert!(
            !view
                .labels
                .values()
                .any(|label| label.contains("暂不支持修改订阅"))
        );
        text(&view, "effort-invalid-0", "该模型不支持当前推理强度");
        assert!(!view.enabled["save"] && !view.enabled["save-switch"]);
        assert_eq!(view.context_mode.as_deref(), Some("512k"));
        assert!(view.bounds.contains_key("delete"));
        let view = activate(cx, "delete");
        text(&view, "confirmation", "删除");
        activate(cx, "keep-editing");
        let view = activate(cx, "mapping-0");
        text(&view, "mapping-option-chat-a", "Chat A");
        text(&view, "mapping-option-chat-a", "chat-a");
        text(&view, "mapping-option-chat-a", "272,000 tokens");
        assert!(!view.bounds.contains_key("mapping-option-image-only"));
        assert!(!view.bounds.contains_key("mapping-option-codex-auto-review"));
        assert!(!view.bounds.contains_key("mapping-option-manual"));
        click(cx, &view, "mapping-option-chat-b");
        let view = activate(cx, "effort-0");
        let view = click(cx, &view, "effort-option-high");
        assert!(view.enabled["save"] && view.enabled["save-switch"]);
        let view = activate(cx, "mapping-2");
        mapping_options(&view, &["chat-a", "chat-b"]);
        press(window, "escape", cx);
        let view = activate(cx, "mapping-1");
        mapping_options(&view, &["chat-a", "chat-b", "manual"]);
        text(&view, "mapping-option-manual", "手动填写模型 ID…");
        click(cx, &view, "mapping-option-chat-b");
        let view = activate(cx, "effort-1");
        click(cx, &view, "effort-option-low");
        focus(cx, "name");
        press(window, "end", cx);
        press(window, "x", cx);
        let view = activate(cx, "context-maximum");
        assert!(view.enabled["save"] && view.enabled["save-switch"]);
        let view = activate(cx, "close");
        text(&view, "confirmation", "有尚未保存的修改");
        activate(cx, "keep-editing");
        activate(cx, "save-switch");
        let (request, input) = save_command();
        assert_eq!(input["name"], "隔离 ChatGPTx");
        assert_eq!(input["kind"], "subscription");
        assert_eq!(input["modelMapping"]["opus"], "chat-b");
        assert_eq!(input["effortMapping"]["opus"], "high");
        assert_eq!(input["modelMapping"]["fable"], "chat-b");
        assert_eq!(input["effortMapping"]["fable"], "low");
        assert_eq!(input["contextWindowMode"], "maximum");
        assert!(input.get("subscription").is_none() && input.get("apiKey").is_none());
        let mut saved_subscription = subscription;
        for key in ["name", "modelMapping", "effortMapping", "contextWindowMode"] {
            saved_subscription[key] = input[key].clone();
        }
        reply(
            cx,
            request,
            ProviderSettingsResult::Provider(saved_subscription.clone()),
        );
        let Command::SetActiveProvider {
            request_id,
            cli,
            id,
        } = business(&sink).unwrap()
        else {
            panic!("expected subscription switch after saving");
        };
        assert_eq!((cli.as_str(), id.as_str()), ("claude-code", "subscription"));
        reply(
            cx,
            request_id,
            snapshot(vec![saved_api, saved_subscription], "subscription"),
        );
        assert!(!open(cx, "subscription").bounds.contains_key("delete"));
        activate(cx, "close");

        activate(cx, "new");
        activate(cx, "custom");
        let view = activate(cx, "next");
        assert!(view.bounds["context"].top() > view.bounds["mapping-3"].bottom());
        assert_eq!(view.context_mode.as_deref(), Some("default"));
        for role in 0..4 {
            assert_eq!(view.labels[&format!("mapping-{role}")], "请选择模型");
        }
        activate(cx, "context-maximum");
        let view = activate(cx, "back");
        text(&view, "confirmation", "有尚未保存的修改");
        activate(cx, "discard");
        assert!(frame(cx).bounds.get("context").is_none());
        activate(cx, "close");

        let codex = json!({
            "id": "codex-api", "cli": "codex", "name": "Codex API", "hasApiKey": true,
            "endpoint": "https://api.example.invalid/v1", "model": "codex-example",
            "reasoningEffort": "high",
        });
        update_shell(window, cx, |shell, window, cx| {
            shell.probe_open_provider_value(codex, window, cx)
        });
        let view = frame(cx);
        text(&view, "subtitle", "用于 Codex");
        text(&view, "protocol", "OpenAI Responses");
        text(&view, "codex-effort", "推理强度");
        assert!(!view.bounds.contains_key("mapping-0") && !view.bounds.contains_key("context"));
        let view = activate(cx, "fetch");
        text(&view, "fetch", "正在获取");
        let Command::FetchProviderModels { request_id, input } = business(&sink).unwrap() else {
            panic!("expected Codex model discovery request");
        };
        assert_eq!(
            input,
            json!({"cli": "codex", "providerId": "codex-api", "endpoint": "https://api.example.invalid/v1"})
        );
        reply(
            cx,
            request_id,
            ProviderSettingsResult::Models(json!({
                "models": ["codex-example"],
                "modelMetadata": [{"id": "codex-example", "displayName": "Codex Example", "contextWindow": 272000,
                    "supportedEfforts": ["low", "high"]}],
            })),
        );
        text(&frame(cx), "model-detail", "272,000 tokens");
        press(window, "escape", cx);
        let view = activate(cx, "effort-0");
        click(cx, &view, "effort-option-low");
        activate(cx, "save");
        let (_, input) = save_command();
        assert_eq!(input["cli"], "codex");
        assert_eq!(input["reasoningEffort"], "low");
        assert!(input.get("modelMapping").is_none() && input.get("contextWindowMode").is_none());
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Hidden GPUI provider editors: API headings, discovery, search, model details, manual mapping, dirty window choice, subscription validation and save/switch, Codex fields"
        );
    }

    fn verify_subscription_window(cx: &mut AsyncApp) {
        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        window
            .update(cx, |view, window, cx| view.open_settings(window, cx))
            .unwrap();
        let (mut read, mut settings, mut assignments, mut compaction) = (None, None, None, None);
        for command in std::iter::from_fn(|| sink.try_recv().ok()) {
            match command {
                Command::GetProviderSettings { request_id } => read = Some(request_id),
                Command::OpenSubscriptionSettings {
                    request_id,
                    settings_id,
                } => {
                    assert_eq!(request_id, settings_id);
                    settings = Some(settings_id);
                }
                Command::GetClaudeRoleAssignments { request_id } => assignments = Some(request_id),
                Command::GetClaudeModelPreferences { request_id } => compaction = Some(request_id),
                _ => panic!("unexpected settings start"),
            }
        }
        let settings = settings.unwrap();
        let reply = |cx: &mut AsyncApp, request, response| {
            window
                .update(cx, |view, window, cx| {
                    view.provider_reply(result(request, response), window, cx);
                })
                .unwrap();
        };
        let model_preferences = |cx: &mut AsyncApp, request_id, compaction| {
            window
                .update(cx, |view, window, cx| {
                    view.compaction_reply(
                        ClaudeModelPreferencesReply {
                            roles: ClaudeRoleAssignmentsReply {
                                request_id,
                                assignments: Some(Default::default()),
                                web_search: None,
                                error: None,
                            },
                            compaction: Some(compaction),
                        },
                        window,
                        cx,
                    )
                })
                .unwrap();
        };
        model_preferences(
            cx,
            compaction.unwrap(),
            json!({"mode": "follow", "effort": "follow"}),
        );
        window
            .update(cx, |view, window, cx| {
                view.assignment_reply(
                    ClaudeRoleAssignmentsReply {
                        request_id: assignments.unwrap(),
                        assignments: Some(Default::default()),
                        web_search: None,
                        error: None,
                    },
                    window,
                    cx,
                )
            })
            .unwrap();
        let mut api = row("api", "隔离 API");
        api["modelMapping"] = json!({"opus": "probe-model", "fable": "probe-model", "sonnet": "probe-model", "haiku": "probe-model"});
        reply(cx, read.unwrap(), snapshot(vec![api.clone()], "api"));
        let ProviderSettingsResult::Snapshot(base) = snapshot(vec![api.clone()], "api") else {
            unreachable!()
        };
        reply(
            cx,
            settings,
            ProviderSettingsResult::SubscriptionSettings(SubscriptionSettingsView {
                snapshot: base,
                accounts: Default::default(),
                antigravity_risk_accepted: false,
            }),
        );
        let frame = |cx: &mut AsyncApp| probe_provider_editor(|| draw(window, cx));
        let activate = |cx: &mut AsyncApp, control| {
            draw(window, cx);
            update_shell(window, cx, |shell, window, _| {
                shell.probe_provider_editor_focus(control, window)
            });
            press(window, "enter", cx);
            frame(cx)
        };
        let pick = |cx: &mut AsyncApp, provider: &str| {
            let view = activate(cx, "new");
            assert!(view.labels["subscription-codex"].contains("ChatGPT"));
            assert!(view.labels["subscription-antigravity"].contains("Antigravity"));
            click_hidden_view(window, view.bounds[provider].center(), cx);
            activate(cx, "next")
        };
        let view = pick(cx, "subscription-antigravity");
        assert!(view.labels["subscription-risk-body"].contains("申诉机会有限"));
        assert!(!view.enabled["subscription-continue"]);
        click_hidden_view(window, view.bounds["subscription-continue"].center(), cx);
        assert!(business(&sink).is_err());
        let view = activate(cx, "subscription-risk");
        assert!(view.enabled["subscription-continue"]);
        activate(cx, "subscription-continue");
        let Command::ConfirmAntigravityRisk { request_id } = business(&sink).unwrap() else {
            panic!("risk acknowledgement must be sent before sign-in");
        };
        reply(cx, request_id, ProviderSettingsResult::Accepted(true));
        activate(cx, "subscription-login");
        let Command::StartSubscriptionLogin {
            request_id,
            settings_id,
            provider,
            ..
        } = business(&sink).unwrap()
        else {
            panic!("expected Antigravity login");
        };
        assert_eq!(
            (settings_id, provider),
            (settings, SubscriptionProvider::Antigravity)
        );
        let login = |provider, attempt_id: &str, status, can_reopen, reason, draft| {
            ProviderSettingsResult::SubscriptionLogin(SubscriptionLoginView {
                attempt_id: attempt_id.into(),
                provider,
                status,
                can_reopen,
                system_browser_available: can_reopen,
                reason,
                draft,
            })
        };
        reply(
            cx,
            request_id,
            login(
                provider,
                "agy-attempt",
                SubscriptionLoginStatus::Waiting,
                true,
                Some(
                    "Could not open the sign-in browser. Try again or use the system browser."
                        .into(),
                ),
                None,
            ),
        );
        let view = frame(cx);
        assert!(view.labels["subscription-waiting"].contains("等待浏览器完成登录"));
        assert!(view.enabled["subscription-reopen"]);
        activate(cx, "subscription-reopen");
        let Command::ReopenSubscriptionLogin {
            request_id,
            settings_id,
            attempt_id,
            system_browser,
        } = business(&sink).unwrap()
        else {
            panic!("expected controlled browser retry");
        };
        assert_eq!(
            (settings_id, attempt_id.as_str(), system_browser),
            (settings, "agy-attempt", false)
        );
        reply(
            cx,
            request_id,
            login(
                provider,
                "agy-attempt",
                SubscriptionLoginStatus::Waiting,
                true,
                None,
                None,
            ),
        );
        activate(cx, "subscription-system-browser");
        let Command::ReopenSubscriptionLogin {
            request_id,
            system_browser: true,
            ..
        } = business(&sink).unwrap()
        else {
            panic!("expected explicit system browser fallback");
        };
        reply(
            cx,
            request_id,
            login(
                provider,
                "agy-attempt",
                SubscriptionLoginStatus::Waiting,
                false,
                None,
                None,
            ),
        );
        activate(cx, "subscription-cancel");
        let Command::CancelSubscriptionLogin {
            settings_id,
            attempt_id,
            ..
        } = business(&sink).unwrap()
        else {
            panic!("expected cancellation");
        };
        assert_eq!(
            (settings_id, attempt_id.as_deref()),
            (settings, Some("agy-attempt"))
        );
        activate(cx, "subscription-cancel");

        pick(cx, "subscription-codex");
        activate(cx, "subscription-login");
        let Command::StartSubscriptionLogin {
            request_id,
            provider: SubscriptionProvider::ChatGpt,
            ..
        } = business(&sink).unwrap()
        else {
            panic!("expected Codex login");
        };
        let account = json!({"status": "ready", "email": "test@example.invalid", "plan": "Plus"});
        let details = SubscriptionDetails {
            account: account.clone(),
            models: vec![
                json!({"id": "probe-opus", "supportedEfforts": ["high"], "contextWindow": 512000}),
                json!({"id": "probe-fable", "supportedEfforts": ["low"], "contextWindow": 512000}),
            ],
            quota: vec![SubscriptionQuota {
                name: "five_hour".into(),
                group: None,
                used_percent: 20.0,
                window_seconds: Some(18000),
                resets_at: None,
            }],
        };
        let mut codex = json!({
            "name": "隔离订阅", "kind": "subscription", "cli": "claude-code",
            "modelMapping": {"opus": "probe-opus", "fable": "probe-fable",
                             "sonnet": "probe-opus", "haiku": "probe-opus"},
            "effortMapping": {"opus": "high", "fable": "low", "sonnet": "follow", "haiku": "follow"},
            "subscription": {"provider": "codex", "account": account, "modelMetadata": details.models,
                             "quota": details.quota},
        });
        reply(
            cx,
            request_id,
            login(
                SubscriptionProvider::ChatGpt,
                "codex-attempt",
                SubscriptionLoginStatus::Ready,
                false,
                None,
                Some(codex.clone()),
            ),
        );
        let view = frame(cx);
        for role in 0..4 {
            assert!(view.bounds.contains_key(format!("mapping-{role}").as_str()));
            assert!(view.bounds.contains_key(format!("effort-{role}").as_str()));
        }
        activate(cx, "context-maximum");
        activate(cx, "save");
        let Command::CommitSubscriptionDraft {
            request_id,
            settings_id,
            attempt_id,
            input,
            activate: switch_after_save,
        } = business(&sink).unwrap()
        else {
            panic!("expected atomic subscription draft commit");
        };
        assert_eq!(
            (settings_id, attempt_id.as_str(), switch_after_save),
            (settings, "codex-attempt", false)
        );
        assert_eq!(input["modelMapping"]["fable"], "probe-fable");
        assert_eq!(input["effortMapping"]["fable"], "low");
        assert_eq!(input["contextWindowMode"], "maximum");
        assert!(input.get("subscription").is_none());
        codex["id"] = json!("codex");
        codex["contextWindowMode"] = input["contextWindowMode"].clone();
        reply(
            cx,
            request_id,
            ProviderSettingsResult::Provider(codex.clone()),
        );

        draw(window, cx);
        let card_action = |cx: &mut AsyncApp, action| {
            draw(window, cx);
            update_shell(window, cx, |shell, window, _| {
                shell.probe_subscription_card_focus("codex", action, window)
            });
            press(window, "enter", cx);
        };
        check_shell(window, cx, |shell, _| {
            assert!(shell.probe_subscription_quota("codex").contains("20%"))
        });
        update_shell(window, cx, |shell, _, cx| {
            shell.probe_compaction_mode("custom", cx);
            shell.probe_compaction_set_provider("codex", cx);
            shell.probe_compaction_set_model("probe-fable", cx);
            shell.probe_compaction_set_effort("low", cx);
            assert!(shell.probe_compaction_validation().is_none());
        });
        let save_before = frame(cx).bounds["compaction-save"];
        update_shell(window, cx, |shell, _, cx| shell.probe_compaction_save(cx));
        let Command::SetClaudeModelPreferences {
            request_id,
            assignments,
            compaction: Some(compaction),
            web_search: None,
        } = business(&sink).unwrap()
        else {
            panic!("expected independent compaction save");
        };
        assert!(assignments.is_null());
        assert_eq!(
            compaction,
            json!({"mode": "custom", "effort": "low", "providerId": "codex", "model": "probe-fable"})
        );
        model_preferences(cx, request_id, compaction);
        check_shell(window, cx, |shell, _| {
            assert!(shell.probe_compaction_state().unwrap().5)
        });
        let save_after = frame(cx).bounds["compaction-save"];
        assert_eq!(
            save_after.right(),
            save_before.right(),
            "the save button must keep its right edge once the saved confirmation appears"
        );
        update_shell(window, cx, |shell, _, cx| {
            let actions = [
                AssignmentAction::Provider(1, "codex".into()),
                AssignmentAction::Model(1, "probe-fable".into()),
                AssignmentAction::Provider(4, "codex".into()),
                AssignmentAction::Model(4, "probe-opus".into()),
                AssignmentAction::Save,
            ];
            for action in actions {
                shell.diagnostics_assignment_action(&action, cx);
            }
        });
        let Command::SetClaudeRoleAssignments {
            request_id,
            assignments,
            web_search: Some(search),
        } = business(&sink).unwrap()
        else {
            panic!("expected subscription role and independent search save");
        };
        assert_eq!(assignments["fable"]["providerId"], "codex");
        assert_eq!(assignments["fable"]["model"], "probe-fable");
        assert_eq!(search["providerId"], "codex");
        assert_eq!(search["model"], "probe-opus");
        window
            .update(cx, |view, window, cx| {
                view.assignment_reply(
                    ClaudeRoleAssignmentsReply {
                        request_id,
                        assignments: Some(serde_json::from_value(assignments).unwrap()),
                        web_search: Some(Some(serde_json::from_value(search).unwrap())),
                        error: None,
                    },
                    window,
                    cx,
                )
            })
            .unwrap();
        card_action(cx, "refresh");
        let Command::RefreshSubscriptionModels {
            request_id,
            provider_id,
            ..
        } = business(&sink).unwrap()
        else {
            panic!("expected explicit catalog refresh");
        };
        assert_eq!(provider_id, "codex");
        let ProviderSettingsResult::Snapshot(mut refreshed) =
            snapshot(vec![api, codex.clone()], "api")
        else {
            unreachable!()
        };
        let mut next_details = details;
        next_details.account["plan"] = json!("Pro 200");
        next_details.models.push(json!({"id": "probe-new"}));
        refreshed["providers"]["claude-code"][1]["subscription"]["modelMetadata"] =
            json!(next_details.models);
        reply(
            cx,
            request_id,
            ProviderSettingsResult::SubscriptionModels(SubscriptionModelsView {
                snapshot: refreshed,
                provider_id,
                details: next_details,
            }),
        );
        check_shell(window, cx, |shell, _| {
            let (rows, _) = shell.diagnostics_providers();
            assert_eq!(rows[1]["subscription"]["account"]["plan"], "Pro 200");
            assert!(
                rows[1]["subscription"]["modelMetadata"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|model| model["id"] == "probe-new")
            );
        });
        card_action(cx, "refresh");
        let Command::RefreshSubscriptionModels { request_id, .. } = business(&sink).unwrap() else {
            panic!("expected second explicit refresh");
        };
        reply(
            cx,
            request_id,
            ProviderSettingsResult::Error("Subscription sign-in is required".into()),
        );
        card_action(cx, "relogin");
        check_shell(window, cx, |shell, _| {
            assert_eq!(shell.probe_subscription_state().unwrap().0, "login")
        });
        activate(cx, "subscription-cancel");
        check_shell(window, cx, |shell, _| {
            let (panel, _) = shell.diagnostics_assignments();
            assert_eq!(panel.rows[1].relogin.as_deref(), Some("codex"));
            assert_eq!(panel.rows[4].relogin.as_deref(), Some("codex"));
        });
        update_shell(window, cx, |shell, window, cx| {
            shell.diagnostics_assignment_action_with_window(
                &AssignmentAction::Relogin("codex".into()),
                window,
                cx,
            );
        });
        check_shell(window, cx, |shell, _| {
            assert_eq!(shell.probe_subscription_state().unwrap().0, "login");
        });
        activate(cx, "subscription-cancel");
        window
            .update(cx, |view, window, cx| view.open_settings(window, cx))
            .unwrap();
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(
            command, Command::CloseSubscriptionSettings { settings_id, .. } if settings_id == settings
        )));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Hidden GPUI subscription: provider chooser, risk gate, browser reopen and cancel, Fable draft commit, quota, catalog refresh, re-login, and close"
        );
    }

    fn verify_window(cx: &mut AsyncApp) {
        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        draw(window, cx);
        window
            .update(cx, |view, window, _| view.arm_settings_entry(window))
            .unwrap();
        press(window, "enter", cx);
        let first = request_id(&sink);
        assignment_request_id(&sink);
        draw(window, cx);
        let first_entity = window
            .update(cx, |view, _, _| view.settings_entity().unwrap().entity_id())
            .unwrap();
        window
            .update(cx, |view, window, cx| {
                view.provider_reply(
                    result(
                        first,
                        ProviderSettingsResult::Error("窗口读取失败原文".into()),
                    ),
                    window,
                    cx,
                );
            })
            .unwrap();

        // The same production keyboard button toggles the live page off.
        window
            .update(cx, |view, window, _| view.arm_settings_entry(window))
            .unwrap();
        press(window, "enter", cx);
        assert!(
            window
                .update(cx, |view, _, _| view.settings_entity().is_none())
                .unwrap()
        );
        draw(window, cx);
        press(window, "enter", cx);
        let second = request_id(&sink);
        assignment_request_id(&sink);
        assert_ne!(first, second);
        assert_ne!(
            first_entity,
            window
                .update(cx, |view, _, _| view.settings_entity().unwrap().entity_id())
                .unwrap()
        );
        window
            .update(cx, |view, window, cx| {
                view.provider_reply(
                    result(second, snapshot(vec![row("a", "隔离供应商")], "a")),
                    window,
                    cx,
                );
            })
            .unwrap();
        draw(window, cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.page() == SettingsPage::ClaudeCode);
        });

        window
            .update(cx, |view, window, _| view.arm_settings_entry(window))
            .unwrap();
        // Tab order: Claude Code, Dispatch, Context optimizer, Language,
        // Diagnostics. Group labels never enter the tab order.
        press(window, "tab", cx);
        press(window, "enter", cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.page() == SettingsPage::ClaudeCode);
        });
        assert!(matches!(
            business(&sink),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        for _ in 0..3 {
            press(window, "tab", cx);
        }
        press(window, "enter", cx);
        draw(window, cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.page() == SettingsPage::Language);
        });
        assert!(matches!(
            business(&sink),
            Ok(Command::GetTimeZoneSettings { .. })
        ));
        for _ in 0..3 {
            press(window, "shift-tab", cx);
        }
        press(window, "enter", cx);
        let reentry = request_id(&sink);
        assignment_request_id(&sink);
        assert_ne!(second, reentry);
        window
            .update(cx, |view, window, cx| {
                view.provider_reply(
                    result(reentry, snapshot(vec![row("a", "隔离供应商")], "a")),
                    window,
                    cx,
                );
            })
            .unwrap();
        draw(window, cx);

        // From the sidebar button, tab through all five navigation entries,
        // Close and Add. The next button is the first card's Edit.
        window
            .update(cx, |view, window, _| view.arm_settings_entry(window))
            .unwrap();
        for _ in 0..8 {
            press(window, "tab", cx);
        }
        press(window, "enter", cx);
        draw(window, cx);
        check_shell(window, cx, |shell, cx| {
            assert!(shell.diagnostics_dialog() == Some(EditorStep::Form));
            assert!(shell.diagnostics_key_empty(cx));
        });

        // The Key field has initial focus. Reverse tab walks Close,
        // Save and Switch, then Save when there are no validation errors.
        for _ in 0..3 {
            press(window, "shift-tab", cx);
        }
        press(window, "enter", cx);
        let Command::SaveProvider {
            request_id: write_id,
            input,
        } = business(&sink).unwrap()
        else {
            panic!("editor did not send a provider save");
        };
        assert_eq!(input["name"], "隔离供应商");
        assert!(input.get("apiKey").is_none());
        window
            .update(cx, |view, window, cx| {
                let failure = ProviderSettingsResult::Error("窗口写入失败原文".into());
                view.provider_reply(result(write_id, failure), window, cx);
            })
            .unwrap();
        check_shell(window, cx, |shell, _| {
            assert!(shell.diagnostics_dialog() == Some(EditorStep::Form));
            assert_eq!(shell.diagnostics_dialog_error(), Some("窗口写入失败原文"));
        });
        draw(window, cx);
        press(window, "escape", cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.diagnostics_dialog().is_none())
        });
        draw(window, cx);

        // Inject a current terminal entity after draw so the probe needs no child process.
        let terminal = window
            .update(cx, |view, _, cx| view.arm_terminal_focus(cx))
            .unwrap();
        press(window, "escape", cx);
        window
            .update(cx, |view, window, _| {
                assert!(view.settings_entity().is_none());
                assert!(terminal.is_focused(window));
                view.clear_terminal_focus_fixture();
                window.remove_window();
            })
            .unwrap();
        println!(
            "Hidden GPUI: settings button keyboard activation and toggle, remount, Graph response, editor Save and original error, modal Escape, terminal focus restoration"
        );
    }

    fn verify_delete_window(cx: &mut AsyncApp) {
        const CLI: &str = "claude-code";
        const BUILT_IN: &str = "anthropic-official";
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let fixture = Fixture(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../target")
                .join(format!("settings-delete-probe-{}", uuid::Uuid::new_v4())),
        );
        let open_store = || ProviderConfigStore::new(&fixture.0, ProviderConfigLocale::ZhCn);
        let store = open_store();
        let add = |name: &str| {
            store
                .upsert(&json!({
                    "cli": CLI, "name": name, "endpoint": "https://api.example.invalid/v1",
                    "model": "probe-model", "apiKey": "probe-fixture-key"
                }))
                .unwrap()
        };
        let active = add("隔离活动供应商");
        let target = add("隔离待删除供应商");
        let active_id = active["id"].as_str().unwrap();
        let target_id = target["id"].as_str().unwrap();
        store.set_active(CLI, active_id).unwrap();

        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        let reply = |cx: &mut AsyncApp, request, response| {
            window
                .update(cx, |view, window, cx| {
                    view.provider_reply(result(request, response), window, cx);
                })
                .unwrap();
        };
        let open_settings = |cx: &mut AsyncApp, snapshot| {
            window
                .update(cx, |view, window, cx| view.open_settings(window, cx))
                .unwrap();
            let request = request_id(&sink);
            assignment_request_id(&sink);
            reply(cx, request, ProviderSettingsResult::Snapshot(snapshot));
            let Command::GetClaudeModelCatalog { request_id } = business(&sink).unwrap() else {
                panic!("the built-in provider should request its model catalog");
            };
            window
                .update(cx, |view, window, cx| {
                    view.model_catalog_reply(
                        json!({"requestId": request_id, "models": []}),
                        window,
                        cx,
                    );
                })
                .unwrap();
            draw(window, cx);
        };
        open_settings(cx, store.snapshot().unwrap());

        let editor_state = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, window, cx| {
                    let shell = view.settings_entity().unwrap();
                    shell.read(cx).diagnostics_delete_editor(window, cx)
                })
                .unwrap()
        };
        for (id, has_delete) in [(BUILT_IN, false), (active_id, false), (target_id, true)] {
            window
                .update(cx, |view, window, cx| {
                    view.settings_entity()
                        .unwrap()
                        .read(cx)
                        .diagnostics_focus_provider_edit(id, window);
                })
                .unwrap();
            press(window, "enter", cx);
            draw(window, cx);
            if has_delete {
                press(window, "x", cx);
            }
            // Reverse from the Key field through Close and the two save controls.
            for _ in 0..4 {
                press(window, "shift-tab", cx);
            }
            assert_eq!(editor_state(cx).0, has_delete, "delete control for {id}");
            if !has_delete {
                press(window, "escape", cx);
                draw(window, cx);
                assert!(matches!(
                    business(&sink),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                ));
            }
        }

        press(window, "enter", cx);
        draw(window, cx);
        assert_eq!(editor_state(cx), (false, true, "x".to_owned()));
        // The confirmation initially focuses Keep editing.
        press(window, "enter", cx);
        draw(window, cx);
        assert_eq!(editor_state(cx), (true, false, "x".to_owned()));
        assert!(matches!(
            business(&sink),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        assert_eq!(
            store.snapshot().unwrap()["providers"][CLI]
                .as_array()
                .unwrap()
                .len(),
            3
        );

        let confirm_delete = |cx: &mut AsyncApp| {
            press(window, "enter", cx);
            draw(window, cx);
            press(window, "tab", cx);
            press(window, "enter", cx);
            let Command::DeleteProvider {
                request_id,
                cli,
                id,
            } = business(&sink).unwrap()
            else {
                panic!("Confirm delete did not send the runtime command");
            };
            assert_eq!(cli, CLI);
            assert_eq!(id, target_id);
            request_id
        };
        let rejected = confirm_delete(cx);
        draw(window, cx);
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                for key in ["enter", "tab", "escape"] {
                    window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
                }
            })
            .unwrap();
        assert!(editor_state(cx).1);
        assert!(matches!(
            business(&sink),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        // Change the active provider on disk before executing the pending delete.
        // The store's guard rejects the now-active ID even though the UI was eligible.
        store.set_active(CLI, target_id).unwrap();
        let error = store.delete(CLI, target_id).unwrap_err();
        reply(
            cx,
            rejected,
            ProviderSettingsResult::Error(error.to_string()),
        );
        check_shell(window, cx, |shell, _| {
            assert!(shell.diagnostics_dialog() == Some(EditorStep::Form));
            assert_eq!(shell.diagnostics_dialog_error(), Some("删除失败"));
            assert_eq!(shell.diagnostics_providers().0.len(), 3);
        });
        assert_eq!(editor_state(cx), (false, true, "x".to_owned()));

        store.set_active(CLI, active_id).unwrap();
        draw(window, cx);
        press(window, "escape", cx);
        draw(window, cx);
        assert_eq!(editor_state(cx), (true, false, "x".to_owned()));
        let deleted = confirm_delete(cx);
        let snapshot = store.delete(CLI, target_id).unwrap();
        reply(cx, deleted, ProviderSettingsResult::Snapshot(snapshot));
        let check_list = |shell: &SettingsShell, notice| {
            assert!(shell.diagnostics_dialog().is_none());
            let (rows, actual_notice) = shell.diagnostics_providers();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0]["id"], active_id);
            assert_eq!(rows[1]["id"], BUILT_IN);
            assert_eq!(actual_notice, notice);
        };
        check_shell(window, cx, |shell, _| {
            check_list(shell, Some(("供应商已删除", "", false)));
        });
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.open_settings(window, cx);
                assert!(view.settings_entity().is_none());
            })
            .unwrap();
        open_settings(cx, open_store().snapshot().unwrap());
        check_shell(window, cx, |shell, _| check_list(shell, None));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Hidden GPUI delete: built-in/active hidden, ordinary shown, cancel preserves draft, pending blocks duplicate input, store rejection says 删除失败 and preserves draft, success says 供应商已删除 and count 3 → 2, remount reads persisted deletion"
        );
    }

    fn verify_assignments_window(cx: &mut AsyncApp) {
        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        window
            .update(cx, |view, window, cx| view.open_settings(window, cx))
            .unwrap();
        let provider_request = request_id(&sink);
        let request = assignment_request_id(&sink);
        let mut base = row("base", "隔离默认");
        base["modelMapping"] =
            json!({"opus": "base-opus", "sonnet": "base-sonnet", "haiku": "base-haiku"});
        let mut target = row("target", "隔离备用");
        target["modelMetadata"] = json!([
            {"id": "target-opus", "displayName": "备用 Opus", "supportedEfforts": ["high"]},
            {"id": "target-sonnet", "displayName": "备用 Sonnet"},
        ]);
        let mut no_key = row("no-key", "未配置 Key");
        no_key["hasApiKey"] = json!(false);
        let mut subscription = row("subscription", "隔离订阅");
        subscription["kind"] = json!("subscription");
        subscription["subscription"] = json!({
            "provider": "codex", "account": {"status": "ready"}, "modelMetadata": [{"id": "subscriber-model"}],
        });
        window
            .update(cx, |view, window, cx| {
                view.provider_reply(
                    result(
                        provider_request,
                        snapshot(vec![base, target, no_key, subscription], "base"),
                    ),
                    window,
                    cx,
                );
            })
            .unwrap();
        let reply = |cx: &mut AsyncApp, request_id, assignments: Option<Value>| {
            window
                .update(cx, |view, window, cx| {
                    view.assignment_reply(
                        ClaudeRoleAssignmentsReply {
                            request_id,
                            web_search: None,
                            error: assignments.is_none().then(|| "isolated failure".into()),
                            assignments: assignments
                                .map(|value| serde_json::from_value(value).unwrap()),
                        },
                        window,
                        cx,
                    );
                })
                .unwrap();
        };
        let action = |cx: &mut AsyncApp, action| {
            window
                .update(cx, |view, _, cx| {
                    view.settings_entity().unwrap().update(cx, |shell, cx| {
                        shell.diagnostics_assignment_action(&action, cx)
                    });
                })
                .unwrap();
        };
        let data = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, cx| {
                    view.settings_entity()
                        .unwrap()
                        .read(cx)
                        .diagnostics_assignments()
                })
                .unwrap()
        };

        let saved = json!({
            "sonnet": {"providerId": "target", "model": "target-sonnet"},
            "haiku": {"providerId": "subscription", "model": "subscriber-model"},
        });
        reply(cx, request + 1000, Some(saved.clone()));
        assert!(matches!(data(cx).0.status, AssignmentStatus::Loading));
        reply(cx, request, Some(saved));
        draw(window, cx);
        let (panel, flags) = data(cx);
        assert!(!flags.0);
        assert_eq!(panel.rows[0].target, "最终：隔离默认 · base-opus");
        assert_eq!(panel.rows[1].tier, "Fable");
        assert_eq!(panel.rows[1].target, "最终：隔离默认 · base-opus");
        assert!(
            panel.rows[0]
                .provider
                .items
                .iter()
                .any(|item| item.value == "subscription")
        );
        assert_eq!(panel.rows[3].provider.label, "隔离订阅");
        assert!(panel.rows[3].problem.is_none());
        assert!(
            panel.rows[4]
                .provider
                .items
                .iter()
                .any(|item| item.value == "subscription")
        );

        action(cx, AssignmentAction::Provider(3, "".into()));
        action(cx, AssignmentAction::Provider(2, "".into()));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.settings_entity()
                    .unwrap()
                    .read(cx)
                    .diagnostics_focus_assignment_provider(window, cx);
            })
            .unwrap();
        press(window, "enter", cx);
        draw(window, cx);
        press(window, "down", cx);
        press(window, "down", cx);
        press(window, "enter", cx);
        assert_eq!(data(cx).0.rows[0].provider.label, "隔离备用");
        assert!(!data(cx).0.can_save);
        action(cx, AssignmentAction::Model(0, "target-opus".into()));
        action(cx, AssignmentAction::Effort(0, "high".into()));
        assert_eq!(data(cx).0.rows[1].target, "最终：隔离备用 · 备用 Opus · 高");
        action(cx, AssignmentAction::Provider(1, "target".into()));
        action(cx, AssignmentAction::Model(1, "target-opus".into()));
        action(cx, AssignmentAction::Effort(1, "low".into()));
        assert!(!data(cx).0.can_save);
        assert!(data(cx).0.rows[1].problem.is_some());
        action(cx, AssignmentAction::Model(1, "target-sonnet".into()));
        action(cx, AssignmentAction::Effort(1, "low".into()));
        let (panel, flags) = data(cx);
        assert_eq!(
            panel.rows[1].target,
            "当前目标：隔离备用 · 备用 Sonnet · 低"
        );
        assert_eq!(flags, (true, false, false, false));
        assert_eq!(panel.rows[0].target, "当前目标：隔离备用 · 备用 Opus · 高");
        assert!(panel.can_save);
        let expected = json!({
            "opus": {"providerId": "target", "model": "target-opus", "effort": "high"},
            "fable": {"providerId": "target", "model": "target-sonnet", "effort": "low"}
        });
        let save = |cx: &mut AsyncApp, expected| {
            action(cx, AssignmentAction::Save);
            let Command::SetClaudeRoleAssignments {
                request_id,
                assignments,
                ..
            } = business(&sink).unwrap()
            else {
                panic!("allocation did not issue a save");
            };
            assert_eq!(assignments, expected);
            assert_eq!(data(cx).1, (true, true, false, false));
            request_id
        };
        let failed = save(cx, expected.clone());
        reply(cx, failed, None);
        let (panel, flags) = data(cx);
        assert_eq!(flags, (true, false, false, true));
        assert_eq!(panel.rows[0].target, "当前目标：隔离备用 · 备用 Opus · 高");
        assert!(panel.can_save);
        let succeeded = save(cx, expected.clone());
        // A normalized reply owns the new baseline, even when it differs from the request.
        let mut canonical = expected;
        canonical["opus"]["effort"] = json!("follow");
        reply(cx, succeeded, Some(canonical));
        let (panel, flags) = data(cx);
        assert_eq!(flags, (false, false, true, false));
        assert_eq!(panel.rows[0].target, "当前目标：隔离备用 · 备用 Opus");
        assert!(!panel.can_save);

        action(cx, AssignmentAction::Provider(0, "no-key".into()));
        let (panel, _) = data(cx);
        assert_eq!(
            panel.rows[0].problem.as_deref(),
            Some("API Key 未配置，请先编辑供应商。")
        );
        assert!(!panel.can_save);
        action(cx, AssignmentAction::Save);
        assert!(business(&sink).is_err());
        action(cx, AssignmentAction::Restore);
        let (panel, _) = data(cx);
        assert!(
            panel.rows[..4]
                .iter()
                .all(|row| row.target.starts_with("最终："))
        );
        assert_eq!(panel.rows[4].target, "未指定：搜索请求使用当前档位供应商");
        let restored = save(cx, json!({}));
        reply(cx, restored, Some(json!({})));
        assert_eq!(data(cx).1, (false, false, true, false));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Hidden GPUI: assignment Graph routing, stale reply, subscription retention, exact save payload, failure retention, canonical baseline, no-Key guard and restore deletion"
        );
    }

    fn verify_model_catalog_window(cx: &mut AsyncApp) {
        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        let official = json!({
            "id": "anthropic-official", "name": "隔离官方", "cli": "claude-code",
            "builtIn": true, "endpoint": "https://api.anthropic.com/", "hasApiKey": false,
        });
        let mut custom = row("deepseek", "DeepSeek");
        custom["modelMetadata"] = json!([{"id": "custom-only", "displayName": "自定义模型"}]);
        let providers = |cx: &mut AsyncApp, request, active| {
            window
                .update(cx, |view, window, cx| {
                    view.provider_reply(
                        result(
                            request,
                            snapshot(vec![official.clone(), custom.clone()], active),
                        ),
                        window,
                        cx,
                    );
                })
                .unwrap();
        };
        let assignments = |cx: &mut AsyncApp, request_id, data| {
            window
                .update(cx, |view, window, cx| {
                    view.assignment_reply(
                        ClaudeRoleAssignmentsReply {
                            request_id,
                            web_search: None,
                            assignments: Some(serde_json::from_value(data).unwrap()),
                            error: None,
                        },
                        window,
                        cx,
                    );
                })
                .unwrap();
        };
        let reply = |cx: &mut AsyncApp, request_id, mut payload: Value| {
            payload["requestId"] = json!(request_id);
            window
                .update(cx, |view, window, cx| {
                    view.model_catalog_reply(payload, window, cx)
                })
                .unwrap();
        };
        let action = |cx: &mut AsyncApp, action| {
            window
                .update(cx, |view, _, cx| {
                    view.settings_entity().unwrap().update(cx, |shell, cx| {
                        shell.diagnostics_assignment_action(&action, cx);
                    });
                })
                .unwrap();
        };
        let panel = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, cx| {
                    view.settings_entity()
                        .unwrap()
                        .read(cx)
                        .diagnostics_assignments()
                        .0
                })
                .unwrap()
        };
        let toggle = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, window, cx| view.open_settings(window, cx))
                .unwrap();
        };
        let catalog_request = || {
            let Command::GetClaudeModelCatalog { request_id } = business(&sink).unwrap() else {
                panic!("settings did not request the native model catalog");
            };
            request_id
        };
        let badge = |cx: &mut AsyncApp| {
            probe_provider_editor(|| draw(window, cx)).labels["provider-window-badge-anthropic-official"]
                .clone()
        };
        let context_labels = |cx: &mut AsyncApp, provider: Value| {
            update_shell(window, cx, |shell, window, cx| {
                shell.probe_open_provider_value(provider, window, cx);
            });
            let frame = probe_provider_editor(|| draw(window, cx));
            update_shell(window, cx, |shell, window, _| {
                shell.probe_provider_editor_focus("close", window);
            });
            press(window, "enter", cx);
            (
                frame.labels["context-default-tokens"].clone(),
                frame.labels["context-maximum-tokens"].clone(),
            )
        };

        toggle(cx);
        let provider_read = request_id(&sink);
        let assignment_read = assignment_request_id(&sink);
        providers(cx, provider_read, "anthropic-official");
        let first = catalog_request();
        assignments(cx, assignment_read, json!({}));
        action(
            cx,
            AssignmentAction::Provider(0, "anthropic-official".into()),
        );
        action(cx, AssignmentAction::Model(0, "opus".into()));
        assert!(
            business(&sink).is_err(),
            "same settings mount repeated the request"
        );

        let rows = json!({"models": [
            {"id": "claude-opus-fixture", "displayName": "Opus concrete", "contextWindow": 200000},
            {"id": "default", "resolvedModel": "claude-opus-fixture", "displayName": "Default (recommended)",
                "contextWindow": 1000000, "maxContextWindow": 1000000},
            {"id": "opus", "resolvedModel": "claude-opus-fixture", "displayName": "Opus alias",
                "contextWindow": 200000, "maxContextWindow": 1000000, "supportedEfforts": ["high"]},
            {"id": "sonnet", "resolvedModel": "claude-sonnet-fixture", "displayName": "Sonnet alias"},
            {"id": "claude-haiku-fixture", "displayName": "Haiku concrete"},
            {"id": "fable", "resolvedModel": "claude-fable-fixture"},
        ]});
        reply(cx, first + 1000, rows.clone());
        for result in [
            json!({"models": []}),
            json!({"error": "isolated catalog failure"}),
        ] {
            let active = if result.get("error").is_some() {
                action(cx, AssignmentAction::RetryCatalog(0));
                catalog_request()
            } else {
                first
            };
            reply(cx, active, result);
            assert_eq!(badge(cx), "默认窗口");
            assert_eq!(
                context_labels(cx, official.clone()),
                ("由 CLI 决定".into(), "未提供（沿用默认）".into())
            );
            let current = panel(cx);
            assert_eq!(
                current.rows[0].problem.as_deref(),
                Some("模型目录暂不可用，请重试。")
            );
            assert!(current.rows[0].retry_catalog);
            assert!(!current.can_save);
            action(cx, AssignmentAction::Save);
            assert!(business(&sink).is_err());
        }
        action(cx, AssignmentAction::RetryCatalog(0));
        let successful = catalog_request();
        update_shell(window, cx, |shell, window, cx| {
            shell.probe_open_provider_value(official.clone(), window, cx);
        });
        draw(window, cx);
        update_shell(window, cx, |shell, window, _| {
            shell.probe_provider_editor_focus("mapping-0", window);
        });
        press(window, "enter", cx);
        let pending = probe_provider_editor(|| draw(window, cx));
        assert!(pending.enabled["mapping-option-manual"]);
        assert!(!pending.labels.contains_key("mapping-option-opus"));
        reply(cx, successful, rows.clone());
        let received = probe_provider_editor(|| draw(window, cx));
        assert!(received.enabled["mapping-option-opus"]);
        assert!(received.labels["mapping-option-opus"].contains("Opus alias"));
        assert!(received.labels["model-source"].contains("官方模型目录"));
        press(window, "escape", cx);
        press(window, "escape", cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.diagnostics_dialog().is_none())
        });
        assert_eq!(
            (badge(cx), context_labels(cx, official.clone())),
            (
                "默认 1M 窗口".into(),
                ("1,000,000 tokens".into(), "1,000,000 tokens".into())
            )
        );
        let mut custom_endpoint = official.clone();
        custom_endpoint["endpoint"] = json!("https://proxy.example.invalid");
        assert_eq!(
            context_labels(cx, custom_endpoint),
            ("由 CLI 决定".into(), "未提供（沿用默认）".into())
        );
        let current = panel(cx);
        assert!(current.can_save);
        assert_eq!(current.rows[2].target, "最终：隔离官方 · Sonnet alias");
        assert_eq!(current.rows[3].target, "最终：隔离官方 · Haiku concrete");

        for (index, name) in ["Opus alias", "Opus alias", "Sonnet alias", "Haiku concrete"]
            .into_iter()
            .enumerate()
        {
            action(cx, AssignmentAction::Provider(index, "".into()));
            assert_eq!(panel(cx).rows[index].model.label, name);
            action(
                cx,
                AssignmentAction::Provider(index, "anthropic-official".into()),
            );
            let current = panel(cx);
            let options = &current.rows[index].model.items;
            assert_eq!(
                options
                    .iter()
                    .map(|option| option.value.as_str())
                    .collect::<Vec<_>>(),
                [
                    "",
                    "claude-opus-fixture",
                    "default",
                    "opus",
                    "sonnet",
                    "claude-haiku-fixture",
                    "fable"
                ]
            );
            assert_eq!(options[2].label, "Default (recommended)");
            assert_eq!(options[6].label, "fable");
            action(cx, AssignmentAction::Provider(index, "".into()));
        }
        action(cx, AssignmentAction::Provider(2, "deepseek".into()));
        let current = panel(cx);
        assert_eq!(
            current.rows[2]
                .model
                .items
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["", "custom-only"]
        );
        action(cx, AssignmentAction::Provider(2, "".into()));
        action(
            cx,
            AssignmentAction::Provider(0, "anthropic-official".into()),
        );
        action(cx, AssignmentAction::Model(0, "opus".into()));
        assert_eq!(
            panel(cx).rows[0]
                .effort
                .items
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["follow", "high"]
        );
        action(cx, AssignmentAction::Save);
        let Command::SetClaudeRoleAssignments {
            assignments: saved, ..
        } = business(&sink).unwrap()
        else {
            panic!("catalog alias should be savable");
        };
        assert_eq!(
            saved,
            json!({"opus": {"providerId": "anthropic-official", "model": "opus"}})
        );
        assert!(business(&sink).is_err());

        toggle(cx);
        reply(cx, successful, rows.clone());
        toggle(cx);
        let provider_read = request_id(&sink);
        let assignment_read = assignment_request_id(&sink);
        providers(cx, provider_read, "deepseek");
        let inactive = catalog_request();
        assignments(cx, assignment_read, saved);
        assert!(
            business(&sink).is_err(),
            "the assignment reply repeated the catalog request"
        );
        reply(cx, inactive, rows);
        assert_eq!(badge(cx), "默认 1M 窗口");
        assert_eq!(panel(cx).rows[0].model.label, "Opus alias");
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Hidden GPUI: inactive official catalog lookup, 1M badge and picker, empty fallback, endpoint isolation, alias and concrete choices, retries and alias save"
        );
    }

    async fn wait_for_language_frame(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        expected: &str,
    ) {
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let frame = probe_language_zone(|| draw(window, cx));
            if frame.labels.iter().any(|label| label == expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "missing {expected:?}: {:?}",
                frame.labels
            );
            cx.background_executor()
                .timer(Duration::from_millis(5))
                .await;
        }
    }

    async fn verify_language_zone_window(cx: &mut AsyncApp, width: f32) {
        let root = std::env::temp_dir().join(format!("timezone-probe-{}", uuid::Uuid::new_v4()));
        let (window, sink) = cx.update(|cx| open_window_at(None, width, cx)).unwrap();
        window
            .update(cx, |view, window, cx| {
                view.set_locale_data_root(root.clone());
                view.open_settings(window, cx);
            })
            .unwrap();
        request_id(&sink);
        assignment_request_id(&sink);
        let update =
            |cx: &mut AsyncApp,
             f: fn(&mut SettingsShell, &mut Window, &mut Context<SettingsShell>)| {
                window
                    .update(cx, |view, window, cx| {
                        view.settings_entity()
                            .unwrap()
                            .update(cx, |settings, cx| f(settings, window, cx));
                    })
                    .unwrap();
            };
        update(cx, SettingsShell::probe_language_page);
        let get = || match business(&sink).unwrap() {
            Command::GetTimeZoneSettings { request_id } => request_id,
            _ => panic!("time-zone read expected"),
        };
        let set = |expected: &str| match business(&sink).unwrap() {
            Command::SetTimeZoneSettings {
                request_id,
                time_zone,
            } => {
                assert_eq!(time_zone, expected);
                request_id
            }
            _ => panic!("time-zone write expected"),
        };
        let snapshot = |zone: &str| TimeZoneSettingsSnapshot {
            time_zone: zone.into(),
            system_time_zone: "Asia/Shanghai".into(),
            auto_time_zone: Some("Asia/Singapore".into()),
            zones: vec!["UTC".into(), "Europe/London".into()],
        };
        let reply = |cx: &mut AsyncApp, request_id, result| {
            window
                .update(cx, |view, window, cx| {
                    view.time_zone_reply(TimeZoneSettingsReply { request_id, result }, window, cx);
                })
                .unwrap();
        };
        let frame = |cx: &mut AsyncApp| probe_language_zone(|| draw(window, cx));
        let activate = |cx: &mut AsyncApp, control| {
            window
                .update(cx, |view, window, cx| {
                    view.settings_entity()
                        .unwrap()
                        .read(cx)
                        .probe_language_focus(control, window);
                })
                .unwrap();
            press(window, "enter", cx);
        };
        let has = |labels: &[String], wanted: &str| {
            assert!(
                labels.iter().any(|label| label == wanted),
                "{width}pt missing {wanted:?}: {labels:?}"
            );
        };

        let read = get();
        let loading = frame(cx);
        assert!(!loading.enabled["settings-time-zone-trigger"]);
        assert!(!loading.enabled["settings-time-zone-save"]);
        has(&loading.labels, "跟随系统");
        has(&loading.labels, "中文");
        has(&loading.labels, "English");
        reply(cx, read, Err(TimeZoneSettingsError::Storage));
        let failure = frame(cx);
        has(&failure.labels, "无法读取时区设置，请重试。");
        assert!(failure.enabled["settings-time-zone-retry"]);
        activate(cx, "zone-retry");
        let retry = get();
        reply(cx, read, Ok(snapshot("Europe/London")));
        assert!(!frame(cx).enabled["settings-time-zone-trigger"]);
        reply(cx, retry, Ok(snapshot("auto")));

        let ready = frame(cx);
        assert!(ready.enabled["settings-time-zone-trigger"]);
        assert!(!ready.enabled["settings-time-zone-save"]);
        has(&ready.labels, "自动（隐藏大陆时区，推荐）");
        assert!(
            ready
                .labels
                .iter()
                .any(|label| label.starts_with("当前时间：") && label.ends_with("Asia/Singapore"))
        );
        let zone_bounds = ready.bounds[2];
        assert!(zone_bounds.size.width > px(185.) && zone_bounds.size.width <= px(520.));
        assert!(
            zone_bounds.right() <= px(width + 2.),
            "{width}: {zone_bounds:?}"
        );
        if width == 720. {
            let title_y = f32::from(zone_bounds.top()) + 18.;
            assert!(
                (title_y - 307.).abs() <= 8.,
                "720pt time zone title y={title_y}"
            );
        }
        for label in ["保存", "取消", "恢复默认"] {
            assert_eq!(
                ready
                    .labels
                    .iter()
                    .filter(|copy| copy.as_str() == label)
                    .count(),
                1
            );
        }
        activate(cx, "zone");
        let menu = frame(cx);
        for label in ["跟随系统 (Asia/Shanghai)", "UTC", "Europe/London"] {
            has(&menu.labels, label);
        }
        press(window, "down", cx);
        press(window, "down", cx);
        press(window, "enter", cx);
        let draft = frame(cx);
        has(&draft.labels, "UTC");
        assert!(draft.enabled["settings-time-zone-save"]);
        activate(cx, "zone-save");
        let save = set("UTC");
        assert!(!frame(cx).enabled["settings-time-zone-trigger"]);
        reply(cx, save, Ok(snapshot("UTC")));
        has(
            &frame(cx).labels,
            "已保存，下次启动 Claude Code 时自动生效。",
        );

        update(cx, SettingsShell::probe_invalid_zone);
        frame(cx);
        activate(cx, "zone-save");
        let invalid = set("Mars/Olympus");
        reply(cx, invalid, Err(TimeZoneSettingsError::InvalidRequest));
        let rejected = frame(cx);
        has(&rejected.labels, "Mars/Olympus");
        has(&rejected.labels, "保存未确认，请重试。");
        assert!(rejected.enabled["settings-time-zone-save"]);
        activate(cx, "zone-cancel");
        assert!(!frame(cx).enabled["settings-time-zone-save"]);
        assert!(business(&sink).is_err());
        activate(cx, "zone-reset");
        frame(cx);
        activate(cx, "zone-save");
        let reset = set("auto");
        reply(cx, reset, Ok(snapshot("auto")));

        frame(cx);
        activate(cx, "zone");
        press(window, "down", cx);
        press(window, "down", cx);
        press(window, "enter", cx);
        has(&frame(cx).labels, "UTC");
        activate(cx, "english");
        wait_for_language_frame(window, cx, "Language & time zone").await;
        assert_eq!(read_locale_preference(&root), LocalePreference::English);
        assert!(
            business(&sink).is_err(),
            "language save must send no runtime command"
        );
        let english = frame(cx);
        assert_eq!(english.selected_language, "en");
        has(&english.labels, "UTC");
        assert!(english.enabled["settings-time-zone-save"]);
        activate(cx, "zone-cancel");
        let english = frame(cx);
        for label in [
            "Language & time zone",
            "Follow system",
            "The interface switches immediately; runtime messages switch after a restart",
            "Time zone",
            "Automatic (hide mainland China time zone, recommended)",
            "Save",
            "Cancel",
            "Restore defaults",
        ] {
            has(&english.labels, label);
        }
        assert!(
            english
                .labels
                .iter()
                .any(|label| label.starts_with("Current time：")
                    && label.ends_with("Asia/Singapore"))
        );
        let locale_file = root.join("locale.json");
        let saved_bytes = std::fs::read(&locale_file).unwrap();
        let preserved = root.join("locale-preserved.json");
        std::fs::rename(&locale_file, &preserved).unwrap();
        std::fs::create_dir(&locale_file).unwrap();
        activate(cx, "chinese");
        wait_for_language_frame(window, cx, "Save was not confirmed. Please retry.").await;
        assert!(locale_file.is_dir());
        assert_eq!(std::fs::read(&preserved).unwrap(), saved_bytes);
        let failed = frame(cx);
        has(&failed.labels, "Language & time zone");
        assert_eq!(failed.selected_language, "en");
        assert_eq!(
            window.update(cx, |view, _, _| view.probe_locale()).unwrap(),
            InterfaceLocale::English
        );
        std::fs::remove_dir(&locale_file).unwrap();
        std::fs::rename(&preserved, &locale_file).unwrap();
        assert_eq!(read_locale_preference(&root), LocalePreference::English);
        activate(cx, "chinese");
        wait_for_language_frame(window, cx, "语言与时区").await;
        assert_eq!(read_locale_preference(&root), LocalePreference::Chinese);

        if width == 720. {
            sink.read_only_mode(1);
            window
                .update(cx, |view, window, cx| view.time_zone_read_only(window, cx))
                .unwrap();
            reply(cx, get(), Ok(snapshot("Asia/Tokyo")));
            let read_only = frame(cx);
            has(&read_only.labels, "Asia/Tokyo");
            has(&read_only.labels, "数据库只读");
            for control in [
                "settings-time-zone-trigger",
                "settings-time-zone-save",
                "settings-time-zone-cancel",
                "settings-time-zone-reset",
            ] {
                assert!(!read_only.enabled[control]);
            }
            activate(cx, "english");
            wait_for_language_frame(window, cx, "The database is read-only.").await;
            let read_only = frame(cx);
            has(&read_only.labels, "Asia/Tokyo");
            assert!(business(&sink).is_err());
            println!(
                "Hidden GPUI 720pt: read-only time zone value and disabled controls, bilingual reason, locale still saves"
            );
        }

        println!(
            "Hidden GPUI SET-08 {width}pt: immediate English and Chinese, failed language write keeps English, time zone draft survives language switch, and time zone width {}pt",
            f32::from(zone_bounds.size.width)
        );
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        std::fs::remove_file(locale_file).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    fn verify_retention_receipt(cx: &mut AsyncApp) {
        let (window, sink) = cx.update(|cx| open_window_at(None, 720., cx)).unwrap();
        window
            .update(cx, |view, window, cx| {
                assert!(!crate::macos::is_window_visible(window).unwrap());
                view.open_settings(window, cx);
                view.settings_entity().unwrap().update(cx, |settings, cx| {
                    settings.select_diagnostics_for_probe(window, cx);
                    settings.cleanup_for_probe(cx);
                });
            })
            .unwrap();
        let request_id = std::iter::from_fn(|| business(&sink).ok())
            .find_map(|command| {
                if let Command::RunRetentionCleanup { request_id } = command {
                    Some(request_id)
                } else {
                    None
                }
            })
            .expect("cleanup dispatched");
        let receipt = |request_id, reclaimed| {
            json!({
                "type": "retentionCleanupResult", "requestId": request_id,
                "result": {"reclaimedBytes": reclaimed, "actions": [{"generation":1}]}
            })
        };
        let update = |cx: &mut AsyncApp, request, reclaimed| {
            window
                .update(cx, |view, window, cx| {
                    view.diagnostic_reply_for_probe(receipt(request, reclaimed), window, cx);
                    view.settings_entity()
                        .unwrap()
                        .read(cx)
                        .diagnostics_text_for_probe("retentionCleanupResult")
                })
                .unwrap()
        };
        let loading = update(cx, request_id + 1, 999);
        assert!(!loading.contains("999"));
        let success = update(cx, request_id, 128);
        assert!(
            success.contains("128")
                && (success.contains("1 removed") || success.contains("移除 1 项"))
        );
        draw(window, cx);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Hidden GPUI diagnostics: stale cleanup reply ignored and successful receipt shows one removal and 128 bytes freed"
        );
    }

    fn verify_late_subscription_metadata(cx: &mut AsyncApp) {
        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        window
            .update(cx, |view, window, cx| view.open_settings(window, cx))
            .unwrap();
        let mut read = None;
        let mut subscription_settings = None;
        for command in std::iter::from_fn(|| sink.try_recv().ok()) {
            match command {
                Command::GetProviderSettings { request_id } => read = Some(request_id),
                Command::OpenSubscriptionSettings { request_id, .. } => {
                    subscription_settings = Some(request_id)
                }
                _ => {}
            }
        }
        let mut provider = json!({
            "id": "subscription", "name": "Probe subscription", "cli": "claude-code",
            "kind": "subscription", "modelMapping": {"opus": "late-model"},
            "subscription": {"provider": "codex"},
        });
        let reply = |cx: &mut AsyncApp, request, response| {
            window
                .update(cx, |view, window, cx| {
                    view.provider_reply(result(request, response), window, cx)
                })
                .unwrap();
        };
        reply(
            cx,
            read.unwrap(),
            snapshot(vec![provider.clone()], "subscription"),
        );
        draw(window, cx);
        update_shell(window, cx, |shell, window, _| {
            shell.diagnostics_focus_provider_edit("subscription", window);
        });
        press(window, "enter", cx);
        let pending = probe_provider_editor(|| draw(window, cx));
        assert!(!pending.labels.contains_key("mapping-0"));
        provider["subscription"]["modelMetadata"] =
            json!([{"id": "late-model", "displayName": "Late model", "contextWindow": 200_000}]);
        let ProviderSettingsResult::Snapshot(snapshot) = snapshot(vec![provider], "subscription")
        else {
            unreachable!();
        };
        reply(
            cx,
            subscription_settings.unwrap(),
            ProviderSettingsResult::SubscriptionSettings(SubscriptionSettingsView {
                snapshot,
                accounts: Default::default(),
                antigravity_risk_accepted: false,
            }),
        );
        let received = probe_provider_editor(|| draw(window, cx));
        assert_eq!(received.labels["mapping-0"], "Late model");
        press(window, "escape", cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.diagnostics_dialog().is_none())
        });
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Hidden GPUI: subscription metadata refreshed the open editor without marking it dirty"
        );
    }

    async fn capture_visible_window(window: AnyWindowHandle, name: &str, cx: &mut AsyncApp) {
        use cocoa::{base::id, foundation::NSInteger};
        use objc::{msg_send, sel, sel_impl};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Some(destination) = std::env::var_os("MATOU_SETTINGS_CAPTURE_DIR").map(PathBuf::from)
        else {
            return;
        };
        let window_number: NSInteger = window
            .update(cx, |_, window, _| {
                let RawWindowHandle::AppKit(handle) =
                    HasWindowHandle::window_handle(window).unwrap().as_raw()
                else {
                    unreachable!("macOS window");
                };
                unsafe {
                    let view = handle.ns_view.as_ptr() as id;
                    let native: id = msg_send![view, window];
                    msg_send![native, windowNumber]
                }
            })
            .unwrap();
        cx.background_executor()
            .timer(Duration::from_millis(350))
            .await;
        let output = destination.join(format!("{name}.png"));
        let status = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-o", "-l"])
            .arg(window_number.to_string())
            .arg(&output)
            .status()
            .unwrap();
        assert!(
            status.success() && output.metadata().unwrap().len() > 0,
            "{output:?}"
        );
        println!(
            "Captured GPUI window {window_number} at {}",
            output.display()
        );
        cx.update(|cx| cx.quit()).unwrap();
    }

    fn open_focused_context_optimizer(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        use cocoa::{
            appkit::{NSEvent, NSEventModifierFlags, NSEventType},
            base::{id, nil},
            foundation::{NSInteger, NSPoint, NSString},
        };
        use objc::{msg_send, sel, sel_impl};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let focus_and_press = |cx: &mut AsyncApp, agent| {
            let view = window
                .update(cx, |view, window, cx| {
                    view.settings_entity()
                        .unwrap()
                        .read(cx)
                        .probe_context_optimizer(cx)
                        .probe_focus(agent, window);
                    let RawWindowHandle::AppKit(handle) =
                        HasWindowHandle::window_handle(window).unwrap().as_raw()
                    else {
                        unreachable!("macOS window");
                    };
                    handle.ns_view.as_ptr() as id
                })
                .unwrap();
            draw(window, cx);
            // Route the key through the probe's own AppKit view.
            unsafe {
                let native: id = msg_send![view, window];
                let number: NSInteger = msg_send![native, windowNumber];
                let enter = NSString::alloc(nil).init_str("\r");
                let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode_(
                    nil, NSEventType::NSKeyUp, NSPoint::new(0., 0.), NSEventModifierFlags::empty(),
                    0., number, nil, enter, enter, false, 36,
                );
                let _: () = msg_send![view, keyUp: event];
                let _: () = msg_send![enter, release];
            }
            draw(window, cx);
        };
        focus_and_press(cx, None);
        focus_and_press(cx, Some("antigravity"));
        window
            .update(cx, |view, window, cx| {
                assert!(
                    view.settings_entity()
                        .unwrap()
                        .read(cx)
                        .probe_context_optimizer(cx)
                        .probe_agent_focused_with_advanced_open("antigravity", window)
                );
            })
            .unwrap();
    }

    fn open_official_editor(
        window: WindowHandle<Workspace>,
        sink: &ProbeReceiver,
        cx: &mut AsyncApp,
    ) {
        let read = request_id(sink);
        assignment_request_id(sink);
        let official = json!({
            "id": "anthropic-official", "name": "Anthropic 官方", "cli": "claude-code",
            "builtIn": true, "endpoint": "https://api.anthropic.com",
            "modelMapping": {"opus": "opus", "fable": "fable", "sonnet": "sonnet", "haiku": "haiku"},
        });
        window
            .update(cx, |view, window, cx| {
                view.provider_reply(
                    result(read, snapshot(vec![official], "anthropic-official")),
                    window,
                    cx,
                );
            })
            .unwrap();
        let Command::GetClaudeModelCatalog { request_id } = business(sink).unwrap() else {
            panic!("expected a native model catalog request");
        };
        draw(window, cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.diagnostics_dialog().is_none())
        });
        update_shell(window, cx, |shell, window, _| {
            shell.diagnostics_focus_provider_edit("anthropic-official", window);
        });
        press(window, "enter", cx);
        window
            .update(cx, |view, window, cx| {
                view.model_catalog_reply(
                    json!({"requestId": request_id, "models": [
                        {"id": "opus", "displayName": "Claude Opus", "contextWindow": 1_000_000},
                        {"id": "fable", "displayName": "Claude Fable", "contextWindow": 1_000_000},
                        {"id": "sonnet", "displayName": "Claude Sonnet", "contextWindow": 1_000_000},
                        {"id": "haiku", "displayName": "Claude Haiku", "contextWindow": 200_000},
                    ]}),
                    window,
                    cx,
                );
            })
            .unwrap();
        let frame = probe_provider_editor(|| draw(window, cx));
        assert!(frame.labels["model-source"].contains("官方模型目录"));
        update_shell(window, cx, |shell, window, _| {
            shell.probe_provider_editor_focus("mapping-0", window);
        });
        press(window, "enter", cx);
        let frame = probe_provider_editor(|| draw(window, cx));
        for id in ["opus", "fable", "sonnet", "haiku"] {
            assert!(frame.enabled[&format!("mapping-option-{id}")], "{id}");
        }
        println!(
            "First official editor: catalog arrived while open, all four models in the first Opus menu"
        );
    }

    struct ClaudeIconPreview;

    impl Render for ClaudeIconPreview {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(20.))
                .bg(rgb(0xf4f6fa))
                .text_color(rgb(0x20242a))
                .child(
                    svg()
                        .path("icons/provider-claude.svg")
                        .size(px(88.))
                        .text_color(rgb(0xd97757)),
                )
                .child(
                    div()
                        .w(px(208.))
                        .child(Workspace::claude_preview_card_header()),
                )
        }
    }

    pub(super) fn run() {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let state = match args.as_slice() {
            [] => None,
            [flag, state]
                if flag == "--visible-state"
                    && matches!(
                        state.as_str(),
                        "loading"
                            | "empty"
                            | "dispatch-light"
                            | "dispatch-dark"
                            | "dispatch-narrow"
                            | "context-optimizer"
                            | "ctx-opt-focused-light"
                            | "ctx-opt-focused-dark"
                            | "provider-editor-official-light"
                            | "claude-icon-preview"
                    ) =>
            {
                Some(state.clone())
            }
            _ => panic!(
                "usage: settings_shell_probe [--visible-state loading|empty|dispatch-light|dispatch-dark|dispatch-narrow|context-optimizer|ctx-opt-focused-light|ctx-opt-focused-dark|provider-editor-official-light|claude-icon-preview]"
            ),
        };
        Application::new().with_assets(NativeAssets).run(move |cx| {
            if let Some(state @ ("context-optimizer" | "ctx-opt-focused-light" | "ctx-opt-focused-dark")) = state.as_deref() {
                let focused = state != "context-optimizer";
                let root = probe_root(state);
                if focused {
                    let theme = if state == "ctx-opt-focused-dark" { ThemePreference::Dark } else { ThemePreference::Light };
                    write_theme_preference(&root, theme).unwrap();
                }
                let (window, sink) = open_window_sized(
                    Some(state),
                    if focused { size(px(1240.), px(1000.)) } else { size(px(1000.), px(780.)) },
                    cx,
                );
                window
                    .update(cx, |view, window, cx| {
                        if focused {
                            view.load_capture_fixture(root, window);
                        } else {
                            view.set_locale_data_root(root);
                        }
                        view.open_settings(window, cx);
                    })
                    .unwrap();
                cx.activate(true);
                let capture_name = state.to_owned();
                cx.spawn(async move |cx| {
                    request_id(&sink);
                    assignment_request_id(&sink);
                    window
                        .update(cx, |view, window, cx| {
                            view.settings_entity().unwrap().read(cx).probe_settings_nav_focus(
                                SettingsPage::ContextOptimizer,
                                window,
                            );
                        })
                        .unwrap();
                    draw(window, cx);
                    press(window, "enter", cx);
                    let Command::GetContextOptimizerStats { request_id } = business(&sink).unwrap()
                    else {
                        panic!("opening the optimizer must request statistics");
                    };
                    assert!(business(&sink).is_err());
                    window
                        .update(cx, |view, window, cx| {
                            view.diagnostic_reply_for_probe(
                                json!({
                                    "type": "contextOptimizerStatsResult",
                                    "requestId": request_id,
                                    "result": {
                                        "agents": {
                                            "claude-code": {"bytes7d": 131072, "events7d": 5},
                                            "antigravity": {"bytes7d": 0, "events7d": 0},
                                            "codex": {"bytes7d": 0, "events7d": 0},
                                            "dsh": {"bytes7d": 0, "events7d": 0}
                                        }
                                    }
                                }),
                                window,
                                cx,
                            );
                        })
                        .unwrap();
                    println!("Context optimizer probe: one request, injected 131072 bytes for Claude Code");
                    if focused {
                        open_focused_context_optimizer(window, cx);
                        capture_visible_window(window.into(), &capture_name, cx).await;
                    }
                })
                .detach();
            } else if state.as_deref() == Some("provider-editor-official-light") {
                let root = probe_root("provider-editor-official-light");
                write_theme_preference(&root, ThemePreference::Light).unwrap();
                let (window, sink) = open_window_sized(
                    state.as_deref(),
                    size(px(1180.), px(940.)),
                    cx,
                );
                window.update(cx, |view, window, cx| {
                    view.load_capture_fixture(root, window);
                    view.open_settings(window, cx);
                }).unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    open_official_editor(window, &sink, cx);
                    capture_visible_window(window.into(), "provider-editor-official-light", cx).await;
                }).detach();
            } else if state.as_deref() == Some("claude-icon-preview") {
                let window = cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(300.), px(224.)),
                            cx,
                        ))),
                        titlebar: None,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| ClaudeIconPreview),
                ).unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    capture_visible_window(window.into(), "claude-icon-preview", cx).await;
                }).detach();
            } else if let Some(state @ ("dispatch-light" | "dispatch-dark" | "dispatch-narrow")) =
                state.as_deref()
            {
                let root = std::env::temp_dir()
                    .join(format!("settings-dispatch-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&root).unwrap();
                seed_dispatch_catalog(&root);
                let theme = if state == "dispatch-light" {
                    ThemePreference::Light
                } else {
                    ThemePreference::Dark
                };
                write_theme_preference(&root, theme).unwrap();
                let width = if state == "dispatch-narrow" {
                    720.
                } else {
                    1340.
                };
                let (window, _sink) = open_window_at(Some(state), width, cx);
                window
                    .update(cx, |view, window, cx| {
                        view.load_theme_fixture(root, window);
                        view.open_settings(window, cx);
                        view.settings_entity()
                            .unwrap()
                            .update(cx, |shell, cx| shell.select_dispatch_for_probe(window, cx));
                    })
                    .unwrap();
                cx.activate(true);
            } else if let Some(state) = state.as_deref() {
                let (window, sink) = open_window(Some(state), cx);
                window
                    .update(cx, |view, window, cx| view.open_settings(window, cx))
                    .unwrap();
                let request = request_id(&sink);
                let allocation = assignment_request_id(&sink);
                if state == "empty" {
                    window
                        .update(cx, |view, window, cx| {
                            view.provider_reply(result(request, snapshot(vec![], "")), window, cx);
                            view.assignment_reply(
                                ClaudeRoleAssignmentsReply {
                                    request_id: allocation,
                                    web_search: None,
                                    assignments: Some(Default::default()),
                                    error: None,
                                },
                                window,
                                cx,
                            );
                        })
                        .unwrap();
                }
                cx.activate(true);
            } else {
                verify_commands();
                cx.spawn(async |cx| {
                    verify_dispatch_window(cx);
                    verify_narrow_dispatch_scroll(cx);
                    verify_theme_window(cx);
                    verify_provider_editors(cx);
                    verify_subscription_window(cx);
                    verify_window(cx);
                    verify_delete_window(cx);
                    verify_assignments_window(cx);
                    verify_model_catalog_window(cx);
                    verify_late_subscription_metadata(cx);
                    verify_retention_receipt(cx);
                    for width in [720., 900., 1200.] {
                        verify_language_zone_window(cx, width).await;
                    }
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
            }
        });
    }
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn main() {
    probe::run();
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn main() {}
