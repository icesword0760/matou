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
    use matou_native_core::dispatch_config::DispatchConfig;
    use matou_native_core::provider_config::{ProviderConfigLocale, ProviderConfigStore};
    use matou_native_core::sessions::{InterfaceLocale, LocalePreference, read_locale_preference};
    use matou_native_desktop::theme_tokens::{
        AppTheme, ThemePreference, read_theme_preference, write_theme_preference,
    };
    use matou_native_runtime::protocol::{
        ClaudeRoleAssignmentsReply, ProviderSettingsReply, ProviderSettingsResult,
        TimeZoneSettingsError, TimeZoneSettingsReply, TimeZoneSettingsSnapshot,
    };
    use serde_json::{Value, json};

    use crate::{
        assets::NativeAssets,
        provider_settings::{ProviderDraft, ProviderSettings, ProviderUpdate},
        runtime_bridge::{self, Command, ProbeReceiver},
        settings_shell::claude_role_assignments::{AssignmentAction, AssignmentStatus},
        settings_shell::{EditorStep, SettingsPage, SettingsShell, probe_language_zone},
        workspace::Workspace,
    };

    fn request_id(sink: &ProbeReceiver) -> u64 {
        match sink.try_recv().expect("settings should send a read") {
            Command::GetProviderSettings { request_id } => request_id,
            _ => panic!("unexpected command while waiting for provider settings"),
        }
    }

    fn assignment_request_id(sink: &ProbeReceiver) -> u64 {
        match sink.try_recv().expect("settings should read assignments") {
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
        } = sink.try_recv().expect("switch should send an activation")
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
        } = sink.try_recv().unwrap()
        else {
            panic!("retry did not send an activation");
        };
        settings.receive(result(switch_id, snapshot(vec![a.clone(), b, c], "a")));
        assert_eq!(settings.active_id(), Some("a"));
        assert_eq!(settings.providers()[0], a);

        let mut draft = ProviderDraft::from_provider(&a);
        assert!(draft.api_key.is_empty());
        draft.name = "确认保留草稿".into();
        draft.model_mapping = ["probe-opus".into(), String::new(), String::new()];
        draft.effort_mapping = ["medium".into(), String::new(), String::new()];
        settings.save(&draft, false).unwrap();
        assert!(settings.saving());
        let Command::SaveProvider {
            request_id: save_id,
            input,
        } = sink.try_recv().unwrap()
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
        } = sink.try_recv().unwrap()
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
        } = sink.try_recv().unwrap()
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
        } = sink.try_recv().unwrap()
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
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_events, incoming) = async_channel::unbounded();
        let bounds = Bounds::centered(None, size(px(width), px(780.)), cx);
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

    fn verify_dispatch_window(cx: &mut AsyncApp) {
        let root = probe_root("dispatch");
        let config = DispatchConfig::for_home(&root);
        // A directory at the file path gives a deterministic read failure.
        std::fs::create_dir_all(config.path()).unwrap();
        let (window, _sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        window
            .update(cx, |view, window, cx| view.open_settings(window, cx))
            .unwrap();
        update_shell(window, cx, |shell, _, _| {
            shell.probe_dispatch_config(DispatchConfig::for_home(&root));
        });
        let select_dispatch = |cx: &mut AsyncApp| {
            draw(window, cx);
            update_shell(window, cx, |shell, window, _| {
                shell.probe_settings_nav_focus(SettingsPage::Dispatch, window);
            });
            press(window, "enter", cx);
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
        update_shell(window, cx, |shell, window, _| {
            shell.probe_settings_nav_focus(SettingsPage::Dispatch, window);
        });
        for _ in 0..7 {
            press(window, "tab", cx);
        }
        let edit_number = |cx: &mut AsyncApp, digits: &[&str]| {
            draw(window, cx);
            press(window, "cmd-a", cx);
            for digit in digits {
                press(window, digit, cx);
            }
            draw(window, cx);
        };
        let save = |cx: &mut AsyncApp| {
            for _ in 0..5 {
                press(window, "tab", cx);
            }
            press(window, "enter", cx);
            draw(window, cx);
        };
        edit_number(cx, &["0"]);
        save(cx);
        assert!(
            !config.path().exists(),
            "invalid input wrote the configuration"
        );
        edit_number(cx, &["1", "3"]);
        std::fs::create_dir(config.path()).unwrap();
        save(cx);
        assert!(config.path().is_dir(), "failed save replaced the fixture");
        std::fs::remove_dir(config.path()).unwrap();
        press(window, "enter", cx);
        draw(window, cx);
        assert_eq!(config.snapshot().unwrap().defaults.idle_min, 13.);

        update_shell(window, cx, |shell, window, _| {
            shell.probe_settings_nav_focus(SettingsPage::Language, window);
        });
        press(window, "enter", cx);
        select_dispatch(cx);
        // The freshly mounted form should still show 13. Changing a different
        // number and saving verifies that 13 was restored into the draft.
        for _ in 0..8 {
            press(window, "tab", cx);
        }
        edit_number(cx, &["4", "9"]);
        for _ in 0..4 {
            press(window, "tab", cx);
        }
        press(window, "enter", cx);
        let saved = config.snapshot().unwrap().defaults;
        assert_eq!((saved.idle_min, saved.max_min), (13., 49.));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        println!(
            "Hidden GPUI dispatch: open, retry read and write failures, invalid number blocked, save, switch pages and re-read"
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
            sink.try_recv().unwrap(),
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
            sink.try_recv().unwrap(),
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

    async fn verify_provider_context_window(cx: &mut AsyncApp) {
        let root = probe_root("provider-context");
        let store = ProviderConfigStore::new(&root, ProviderConfigLocale::ZhCn);
        let provider = store
            .upsert(&json!({
                "cli": "claude-code", "name": "隔离窗口供应商",
                "endpoint": "https://context.example.invalid", "apiKey": "fixture-key",
                "model": "probe-model"
            }))
            .unwrap();
        let id = provider["id"].as_str().unwrap();
        store.set_active("claude-code", id).unwrap();
        let (window, sink) = cx.update(|cx| open_window(None, cx)).unwrap();
        window
            .update(cx, |view, window, cx| {
                view.set_locale_data_root(root.clone());
                view.open_settings(window, cx);
            })
            .unwrap();

        let reload = |cx: &mut AsyncApp| {
            let provider_request = request_id(&sink);
            let assignments_request = assignment_request_id(&sink);
            window
                .update(cx, |view, window, cx| {
                    view.provider_reply(
                        result(
                            provider_request,
                            ProviderSettingsResult::Snapshot(store.snapshot().unwrap()),
                        ),
                        window,
                        cx,
                    );
                    view.assignment_reply(
                        ClaudeRoleAssignmentsReply {
                            request_id: assignments_request,
                            assignments: Some(Default::default()),
                            web_search: None,
                            error: None,
                        },
                        window,
                        cx,
                    );
                })
                .unwrap();
        };
        let status = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, cx| {
                    let shell = view.settings_entity().unwrap();
                    let row = shell.read(cx).probe_provider_context(id).read(cx);
                    let (badge, error, pending) = row.probe_status();
                    (badge, error.map(str::to_owned), pending)
                })
                .unwrap()
        };
        let edit = |cx: &mut AsyncApp, keys: &[&str]| {
            check_shell_for_context_input(window, id, cx);
            draw(window, cx);
            press(window, "cmd-a", cx);
            for key in keys {
                press(window, key, cx);
            }
            press(window, "enter", cx);
        };
        let write = |tokens: Value| {
            let Command::SaveProvider { request_id, input } = sink.try_recv().unwrap() else {
                panic!("expected context-only provider write");
            };
            assert_eq!(
                input,
                json!({"cli": "claude-code", "id": id, "contextWindow": tokens})
            );
            (request_id, input)
        };
        let confirm = |cx: &mut AsyncApp, request_id, reply| {
            window
                .update(cx, |view, window, cx| {
                    view.provider_reply(result(request_id, reply), window, cx);
                })
                .unwrap();
            draw(window, cx);
        };
        reload(cx);
        let chinese = probe_language_zone(|| draw(window, cx));
        assert!(
            chinese
                .labels
                .iter()
                .any(|label| label.starts_with("所有 Claude Code 会话跟随全局供应商"))
        );
        assert_eq!(status(cx).0, "默认窗口");
        edit(cx, &["9"]);
        assert!(status(cx).1.unwrap().contains("100,000"));
        assert!(sink.try_recv().is_err());

        edit(cx, &["3", "2", "0", "0", "0", "0"]);
        let (request_id, _) = write(json!(320_000));
        assert_eq!(status(cx), ("默认窗口".into(), None, true));
        confirm(
            cx,
            request_id,
            ProviderSettingsResult::Error("隔离窗口写入失败".into()),
        );
        assert_eq!(
            status(cx),
            ("默认窗口".into(), Some("隔离窗口写入失败".into()), false)
        );
        press(window, "enter", cx);
        let (request_id, input) = write(json!(320_000));
        confirm(
            cx,
            request_id,
            ProviderSettingsResult::Provider(store.upsert(&input).unwrap()),
        );
        assert_eq!(status(cx), ("320K 窗口".into(), None, false));

        edit(cx, &["backspace"]);
        let (request_id, input) = write(Value::Null);
        assert_eq!(status(cx), ("320K 窗口".into(), None, true));
        confirm(
            cx,
            request_id,
            ProviderSettingsResult::Provider(store.upsert(&input).unwrap()),
        );
        assert_eq!(status(cx), ("默认窗口".into(), None, false));

        update_shell(window, cx, |shell, window, cx| {
            shell.probe_language_page(window, cx)
        });
        assert!(matches!(
            sink.try_recv().unwrap(),
            Command::GetTimeZoneSettings { .. }
        ));
        draw(window, cx);
        update_shell(window, cx, |shell, window, _| {
            shell.probe_language_focus("english", window)
        });
        press(window, "enter", cx);
        wait_for_language_frame(window, cx, "Language & time zone").await;
        update_shell(window, cx, |shell, window, _| {
            shell.probe_settings_nav_focus(SettingsPage::ClaudeCode, window)
        });
        press(window, "enter", cx);
        reload(cx);
        let english = probe_language_zone(|| draw(window, cx));
        assert!(english.labels.iter().any(|label| {
            label.starts_with("All Claude Code sessions follow the global provider.")
        }));
        assert_eq!(status(cx), ("Default window".into(), None, false));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        println!(
            "Hidden GPUI context window: validation, confirmed save, rejected save and retry, clear to default, Chinese/English impact and re-read"
        );
    }

    fn check_shell_for_context_input(window: WindowHandle<Workspace>, id: &str, cx: &mut AsyncApp) {
        window
            .update(cx, |view, window, cx| {
                view.settings_entity()
                    .unwrap()
                    .read(cx)
                    .probe_provider_context(id)
                    .read(cx)
                    .probe_focus_input(window, cx);
            })
            .unwrap();
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
        // The navigation tabs run Claude Code, Dispatch, Language, then Diagnostics.
        // Group labels never enter the tab order.
        press(window, "tab", cx);
        press(window, "enter", cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.page() == SettingsPage::ClaudeCode);
        });
        assert!(matches!(
            sink.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        press(window, "tab", cx);
        press(window, "tab", cx);
        press(window, "enter", cx);
        draw(window, cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.page() == SettingsPage::Language);
        });
        assert!(matches!(
            sink.try_recv(),
            Ok(Command::GetTimeZoneSettings { .. })
        ));
        press(window, "shift-tab", cx);
        press(window, "shift-tab", cx);
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

        // From the sidebar button, tab through category buttons, close and add.
        // The next button is the first card's Edit.
        window
            .update(cx, |view, window, _| view.arm_settings_entry(window))
            .unwrap();
        for _ in 0..7 {
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
        } = sink.try_recv().unwrap()
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
                    sink.try_recv(),
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
            sink.try_recv(),
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
            } = sink.try_recv().unwrap()
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
            sink.try_recv(),
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
            "account": {"status": "ready"}, "modelMetadata": [{"id": "subscriber-model"}],
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
        assert!(
            panel.rows[0]
                .provider
                .items
                .iter()
                .all(|item| item.value != "subscription")
        );
        assert_eq!(panel.rows[2].provider.label, "subscription · 不可用");
        assert_eq!(
            panel.rows[2].problem.as_deref(),
            Some("账号已失效，请重新登录或选择其他供应商。")
        );
        assert!(!panel.can_save);

        action(cx, AssignmentAction::Provider(2, "".into()));
        action(cx, AssignmentAction::Provider(1, "".into()));
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
        let (panel, flags) = data(cx);
        assert_eq!(flags, (true, false, false, false));
        assert_eq!(panel.rows[0].target, "当前目标：隔离备用 · 备用 Opus · 高");
        assert!(panel.can_save);
        let expected = json!({
            "opus": {"providerId": "target", "model": "target-opus", "effort": "high"}
        });
        let save = |cx: &mut AsyncApp, expected| {
            action(cx, AssignmentAction::Save);
            let Command::SetClaudeRoleAssignments {
                request_id,
                assignments,
                ..
            } = sink.try_recv().unwrap()
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
        assert!(sink.try_recv().is_err());
        action(cx, AssignmentAction::Restore);
        let (panel, _) = data(cx);
        assert!(
            panel.rows[..3]
                .iter()
                .all(|row| row.target.starts_with("最终："))
        );
        assert_eq!(panel.rows[3].target, "未指定：搜索请求使用当前档位供应商");
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
        let mut custom = row("custom", "隔离自定义");
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
            let Command::GetClaudeModelCatalog { request_id } = sink.try_recv().unwrap() else {
                panic!("settings did not request the native model catalog");
            };
            request_id
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
            sink.try_recv().is_err(),
            "same settings mount repeated the request"
        );

        let rows = json!({"models": [
            {"id": "claude-opus-fixture", "displayName": "Opus concrete", "contextWindow": 200000},
            {"id": "default", "resolvedModel": "claude-opus-fixture", "displayName": "Default (recommended)"},
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
            let current = panel(cx);
            assert_eq!(
                current.rows[0].problem.as_deref(),
                Some("模型目录暂不可用，请重试。")
            );
            assert!(current.rows[0].retry_catalog);
            assert!(!current.can_save);
            action(cx, AssignmentAction::Save);
            assert!(sink.try_recv().is_err());
        }
        action(cx, AssignmentAction::RetryCatalog(0));
        let successful = catalog_request();
        reply(cx, successful, rows.clone());
        let current = panel(cx);
        assert!(current.can_save);
        assert_eq!(current.rows[1].target, "最终：隔离官方 · Sonnet alias");
        assert_eq!(current.rows[2].target, "最终：隔离官方 · Haiku concrete");

        for (index, name) in ["Opus alias", "Sonnet alias", "Haiku concrete"]
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
        action(cx, AssignmentAction::Provider(1, "custom".into()));
        let current = panel(cx);
        assert_eq!(
            current.rows[1]
                .model
                .items
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["", "custom-only"]
        );
        action(cx, AssignmentAction::Provider(1, "".into()));
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
        } = sink.try_recv().unwrap()
        else {
            panic!("catalog alias should be savable");
        };
        assert_eq!(
            saved,
            json!({"opus": {"providerId": "anthropic-official", "model": "opus"}})
        );
        assert!(sink.try_recv().is_err());

        toggle(cx);
        reply(cx, successful, rows.clone());
        toggle(cx);
        let provider_read = request_id(&sink);
        let assignment_read = assignment_request_id(&sink);
        providers(cx, provider_read, "custom");
        assert!(
            sink.try_recv().is_err(),
            "unused official provider should not request its catalog"
        );
        assignments(cx, assignment_read, saved);
        let explicit = catalog_request();
        reply(cx, explicit, rows);
        assert_eq!(panel(cx).rows[0].model.label, "Opus alias");
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Hidden GPUI: official catalog defaults, alias and concrete choices, explicit/non-default lookup, empty and error retries, custom isolation and alias save"
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
        let get = || match sink.try_recv().unwrap() {
            Command::GetTimeZoneSettings { request_id } => request_id,
            _ => panic!("time-zone read expected"),
        };
        let set = |expected: &str| match sink.try_recv().unwrap() {
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
        assert!(sink.try_recv().is_err());
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
            sink.try_recv().is_err(),
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
            assert!(sink.try_recv().is_err());
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
        let request_id = std::iter::from_fn(|| sink.try_recv().ok())
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

    pub(super) fn run() {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let state = match args.as_slice() {
            [] => None,
            [flag, state]
                if flag == "--visible-state" && matches!(state.as_str(), "loading" | "empty") =>
            {
                Some(state.clone())
            }
            _ => panic!("usage: settings_shell_probe [--visible-state loading|empty]"),
        };
        Application::new().with_assets(NativeAssets).run(move |cx| {
            if let Some(state) = state.as_deref() {
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
                    verify_theme_window(cx);
                    verify_provider_context_window(cx).await;
                    verify_window(cx);
                    verify_delete_window(cx);
                    verify_assignments_window(cx);
                    verify_model_catalog_window(cx);
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
