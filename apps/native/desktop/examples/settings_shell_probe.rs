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
    use matou_native_runtime::protocol::{
        ClaudeRoleAssignmentsReply, ProviderSettingsReply, ProviderSettingsResult,
    };
    use serde_json::{Value, json};

    use crate::{
        assets::NativeAssets,
        provider_settings::{ProviderDraft, ProviderSettings, ProviderUpdate},
        runtime_bridge::{self, Command, ProbeReceiver},
        settings_shell::claude_role_assignments::{AssignmentAction, AssignmentStatus},
        settings_shell::{EditorStep, SettingsPage, SettingsShell},
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
                assert!(window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx));
            })
            .unwrap();
    }

    fn open_window(
        visible_state: Option<&str>,
        cx: &mut App,
    ) -> (WindowHandle<Workspace>, ProbeReceiver) {
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_events, incoming) = async_channel::unbounded();
        let bounds = Bounds::centered(None, size(px(1200.), px(780.)), cx);
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
                |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
            )
            .unwrap();
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
        // The group labels never enter the tab order. First tab is Claude
        // Code, second is Language, and a reverse tab returns to Claude Code.
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
        press(window, "enter", cx);
        draw(window, cx);
        check_shell(window, cx, |shell, _| {
            assert!(shell.page() == SettingsPage::Language);
        });
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
        for _ in 0..5 {
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
            panel
                .rows
                .iter()
                .all(|row| row.target.starts_with("最终："))
        );
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
                    verify_window(cx);
                    verify_assignments_window(cx);
                    verify_model_catalog_window(cx);
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
