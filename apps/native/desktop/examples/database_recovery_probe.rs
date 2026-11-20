//! Render the real recovery page and workspace in an unshown, unfocused window.
#![cfg_attr(not(all(target_os = "macos", debug_assertions)), allow(dead_code))]

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
#[path = "../src/ui_text.rs"]
mod production_ui_text;
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
mod ui_text {
    pub(crate) use crate::production_ui_text::*;
    pub fn ui_text(text: impl Into<gpui::SharedString>) -> UiText {
        let text = text.into();
        crate::probe::LABELS.with_borrow_mut(|labels| {
            if let Some(labels) = labels {
                labels.push(text.to_string());
            }
        });
        crate::production_ui_text::ui_text(text)
    }
}

#[cfg(all(target_os = "macos", debug_assertions))]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    impl Workspace {
        pub(super) fn receive(
            &mut self,
            generation: u64,
            event: Event,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(BridgeEvent::Runtime { generation, event }, window, cx);
        }

        pub(super) fn change(
            &mut self,
            generation: u64,
            phase: ConnectionPhase,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(BridgeEvent::Connection { generation, phase }, window, cx);
        }

        pub(super) fn recovery_action(
            &mut self,
            action: &PageAction,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let id = self
                .database_recovery
                .as_ref()
                .unwrap()
                .details
                .recovery_id
                .clone();
            self.database_recovery_action(&id, action, window, cx);
        }

        pub(super) fn assert_modal_return_focus(&self, window: &Window) {
            let state = self.database_recovery.as_ref().unwrap();
            let confirmation = state.confirmation.as_ref().expect("active modal");
            assert!(confirmation.back.is_focused(window));
            assert!(!confirmation.confirm.is_focused(window));
        }

        pub(super) fn assert_trigger_focus(&self, window: &Window) {
            let state = self.database_recovery.as_ref().unwrap();
            assert!(state.confirmation.is_none());
            assert!(state.empty_trigger.is_focused(window));
        }

        pub(super) fn assert_modal_tab_order(&self, window: &mut Window) {
            let state = self.database_recovery.as_ref().unwrap();
            let confirmation = state.confirmation.as_ref().expect("active modal");
            assert!(confirmation.back.is_focused(window));
            window.focus_next();
            assert!(confirmation.confirm.is_focused(window));
            window.focus_next();
            assert!(confirmation.back.is_focused(window));
            window.focus_prev();
            assert!(confirmation.confirm.is_focused(window));
            window.focus_prev();
            assert!(confirmation.back.is_focused(window));
            let focus = ConfirmationFocus {
                back: &confirmation.back,
                confirm: &confirmation.confirm,
            };
            for (key, expected) in [
                ("tab", focus.confirm),
                ("tab", focus.back),
                ("shift-tab", focus.confirm),
                ("shift-tab", focus.back),
            ] {
                assert!(focus.handle_key(
                    &Keystroke::parse(key).unwrap(),
                    true,
                    window,
                    |_| panic!("Tab canceled confirmation")
                ));
                assert!(expected.is_focused(window));
            }
        }

        pub(super) fn escape_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            let state = self.database_recovery.as_ref().unwrap();
            let confirmation = state.confirmation.as_ref().expect("active modal");
            let focus = ConfirmationFocus {
                back: &confirmation.back,
                confirm: &confirmation.confirm,
            };
            let mut cancel = false;
            assert!(
                focus.handle_key(&Keystroke::parse("escape").unwrap(), true, window, |_| {
                    cancel = true
                })
            );
            assert!(cancel);
            self.recovery_action(&PageAction::CancelConfirmation, window, cx);
        }

        pub(super) fn recovery_selected(&self) -> bool {
            self.database_recovery.is_some() || self.invalid_recovery_marker
        }

        pub(super) fn workspaces_ready(&self) -> bool {
            self.database_recovery.is_none() && self.domain_ready && self.hierarchy.is_some()
        }

        pub(super) fn export(&mut self, cx: &mut Context<Self>) {
            self.export_read_only_database(cx);
        }

        pub(super) fn request_older(
            &mut self,
            cursor: HistoryCursor,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.request_read_only_history(Some(cursor), window, cx);
        }

        pub(super) fn read_only_page(&self) -> Option<&OfflineHistoryPage> {
            self.read_only.as_ref()?.history.page.as_ref()
        }

        pub(super) fn search_read_only(
            &mut self,
            query: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.open_search(window, cx);
            self.search.as_mut().unwrap().query = query.into();
            self.run_search(false, cx);
        }

        pub(super) fn verify_mutations_disabled(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let id = self.focused.as_ref().unwrap();
            let node = self.graph_node(id).unwrap();
            assert_eq!(
                self.card_action_blocked_reason(id, node),
                Some("数据库处于只读恢复模式")
            );
            assert_eq!(
                self.session_loader_blocked_reason(id, node),
                Some("数据库处于只读恢复模式")
            );
            let scene = self.active_scene_id().unwrap().to_owned();
            self.create_canvas(cx);
            self.split_horizontal(cx);
            self.open_rename(scene, window, cx);
            assert!(self.dialog.is_none());
        }
    }
}

#[cfg(all(target_os = "macos", debug_assertions))]
mod probe {
    use std::{cell::RefCell, path::Path, sync::Arc, time::Duration};

    use gpui::*;
    use matou_native_core::{
        backup::BackupReason,
        startup::{ReadOnlyReason, RecoveryError},
    };
    use matou_native_runtime::{
        history::{HistoryCursor, HistoryLine, HistoryPage, HistorySearchResult},
        history_worker::{HistoryReply, HistoryRequest},
        host::{DatabaseRecoveryBackup, DatabaseRecoveryReason, ReadOnlyProjection, RuntimeHost},
    };

    use crate::runtime_bridge::{
        self, Command, ConnectionPhase, DatabaseRecoveryAction, DatabaseRecoveryDetails,
        DatabaseRecoveryResult, Event, LaunchOptions, ReadOnlyNavigation,
    };
    use crate::workspace::{
        Workspace,
        database_recovery_page::{PageAction, probe_draw},
    };

    thread_local! {
        pub static LABELS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    }

    fn draw(window: WindowHandle<Workspace>, cx: &mut AsyncApp, recovery: bool) -> Vec<String> {
        window
            .update(cx, |workspace, _, _| {
                assert_eq!(workspace.recovery_selected(), recovery);
            })
            .unwrap();
        LABELS.with_borrow_mut(|labels| *labels = Some(Vec::new()));
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
        LABELS.with_borrow_mut(|labels| labels.take().unwrap())
    }

    fn has(labels: &[String], expected: &str) {
        assert!(
            labels.iter().any(|text| text == expected),
            "rendered labels missing {expected:?}: {labels:?}"
        );
    }

    fn expect_root(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        expected: &'static str,
    ) -> Vec<String> {
        let (labels, frame) = probe_draw(|| draw(window, cx, expected == "database-recovery-page"));
        assert_eq!(frame.root, Some(ElementId::from(expected)));
        labels
    }

    fn near(actual: Pixels, expected: f32) {
        assert!(
            (f32::from(actual) - expected).abs() <= 1.0,
            "expected {expected} pt, got {actual:?}",
        );
    }

    fn hidden_window(
        cx: &mut AsyncApp,
        commands: runtime_bridge::RuntimeCommands,
        incoming: async_channel::Receiver<runtime_bridge::BridgeEvent>,
        viewport: Size<Pixels>,
    ) -> WindowHandle<Workspace> {
        cx.open_window(
            WindowOptions {
                show: false,
                focus: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    viewport,
                ))),
                titlebar: Some(TitlebarOptions {
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
        )
        .unwrap()
    }

    async fn verify_invalid_marker(cx: &mut AsyncApp, scratch: &Path) {
        let root = scratch.join("m");
        let data = root.join("d");
        let home = root.join("home");
        let workspace = scratch.join("workspace");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir(&home).unwrap();
        let database = data.join("matou.sqlite");
        let good_database = std::fs::read(scratch.join("data/matou.sqlite")).unwrap();
        std::fs::write(&database, &good_database).unwrap();
        let marker = data.join("matou.sqlite.recovery.json");
        std::fs::write(&marker, b"{").unwrap();

        let executable = std::env::current_exe().unwrap();
        let binary = executable
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("matou-native-runtime");
        let mut bridge = runtime_bridge::RuntimeBridge::start_with_binary(
            LaunchOptions {
                data_root: data.clone(),
                workspace,
                shell_home: home,
                shell: "/bin/zsh".into(),
            },
            &binary,
        )
        .unwrap();
        let commands = bridge.commands.clone();
        let window = hidden_window(
            cx,
            commands.clone(),
            bridge.events.clone(),
            size(px(1200.), px(780.)),
        );
        let mut first_error_generation = None;
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| view.recovery_selected())
                .unwrap()
            {
                let generation = commands.connection().0;
                expect_root(window, cx, "database-recovery-page");
                let (labels, snapshot) = probe_draw(|| draw(window, cx, true));
                has(
                    &labels,
                    "数据库恢复记录无效。当前无法验证恢复状态，暂不能恢复备份、导出恢复资料或创建空数据库。",
                );
                has(&labels, "Matou 已保留原数据库和恢复记录，会自动重新检查。");
                has(&labels, "重新检查数据库");
                has(&labels, "导出恢复资料");
                has(&labels, "创建全新空数据库");
                has(&labels, "恢复所选备份");
                assert_eq!(snapshot.buttons.len(), 4);
                assert!(snapshot.buttons.values().all(|(enabled, _)| !enabled));
                let first = *first_error_generation.get_or_insert(generation);
                if generation > first {
                    break;
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        let first = first_error_generation.expect("invalid marker did not reach the error page");
        assert!(
            commands.connection().0 > first,
            "automatic retry did not start"
        );
        assert_eq!(std::fs::read(&marker).unwrap(), b"{");
        assert_eq!(std::fs::read(&database).unwrap(), good_database);
        assert!(!data.join("recovery-evidence").exists());
        assert!(std::fs::read_dir(&data).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".corrupt-")
        }));
        let preserved = root.join("invalid-marker.fixture");
        std::fs::rename(&marker, &preserved).unwrap();
        for _ in 0..400 {
            if window
                .update(cx, |view, _, _| view.workspaces_ready())
                .unwrap()
            {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        assert!(
            window
                .update(cx, |view, _, _| view.workspaces_ready())
                .unwrap()
        );
        expect_root(window, cx, "workspace-root");
        draw(window, cx, false);
        assert_eq!(std::fs::read(preserved).unwrap(), b"{");
        bridge.shutdown().unwrap();
        println!(
            "hidden invalid marker: error visible across runtime generations, input bytes intact, automatic retry reached workspace after fixture repair"
        );
    }

    async fn verify(cx: &mut AsyncApp) {
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_event_sender, incoming) = async_channel::unbounded();
        sink.starting();
        let window = hidden_window(cx, commands, incoming, size(px(1200.), px(780.)));
        window
            .update(cx, |_, window, _| {
                assert_eq!(window.viewport_size(), size(px(1200.), px(780.)));
            })
            .unwrap();
        assert!(expect_root(window, cx, "startup-neutral").is_empty());

        let mut details = DatabaseRecoveryDetails {
            recovery_id: String::new(),
            reason: DatabaseRecoveryReason::Integrity,
            durable_database_path: "matou.sqlite".into(),
            quarantined_path: "matou.sqlite.corrupt-123".into(),
            ownership_issue: None,
            backups: Vec::new(),
            error: None,
        };
        let synthetic_recovery =
            |cx: &mut AsyncApp, details: &DatabaseRecoveryDetails, generation| {
                let details = Arc::new(details.clone());
                sink.database_recovery(generation, details.clone());
                window
                    .update(cx, |view, window, cx| {
                        view.change(
                            generation,
                            ConnectionPhase::DatabaseRecovery {
                                details,
                                pending: None,
                            },
                            window,
                            cx,
                        );
                    })
                    .unwrap();
            };

        details.recovery_id = "probe-first-frame".into();
        synthetic_recovery(cx, &details, 1);
        expect_root(window, cx, "database-recovery-page");
        let (labels, snapshot) = probe_draw(|| draw(window, cx, true));
        has(&labels, "数据库需要恢复");
        let card = snapshot.bounds["recovery-card"][0];
        let top = card.origin.y + px(1.);
        let bottom = card.origin.y + card.size.height - px(1.);
        println!("hidden viewport 1200×780 pt, card {card:?}, white interior y={top:?}–{bottom:?}");
        near(card.origin.x, 240.);
        near(card.size.width, 720.);
        near(top, 183.);
        near(bottom, 597.);
        near(card.center().y, 390.);
        let (enabled, style) = snapshot.buttons["database-recovery-restore"];
        assert!(!enabled);
        println!("hidden disabled restore: {style:?}");
        assert_eq!(style.font_weight, FontWeight::NORMAL);
        assert_eq!(style.border_width, 1.);
        assert_eq!(style.border, Some(0x9ebefb));
        assert_eq!(style.background, style.border);
        assert_eq!(style.text, 0xffffff);
        println!("hidden recovery startup roots: startup-neutral -> database-recovery-page");

        for (index, (reason, title, description)) in [
            (DatabaseRecoveryReason::Ownership, "数据库占用状态需要处理", "数据库占用记录或接管状态异常，原数据库仍保留在原位置。处理前不会将其当作损坏文件移动。"),
            (DatabaseRecoveryReason::Wal, "数据库需要恢复", "数据库日志状态不完整。Matou 已保留原数据库和日志文件，请选择备份恢复或导出资料。"),
            (DatabaseRecoveryReason::Integrity, "数据库需要恢复", "数据库完整性检查未通过。Matou 已保留原文件，不会直接进入全新空工作区。"),
        ].into_iter().enumerate() {
            details.reason = reason;
            details.recovery_id = format!("probe-{index}");
            synthetic_recovery(cx, &details, 1);
            let labels = draw(window, cx, true);
            for expected in ["Matou 数据恢复", title, description, "可用备份", "0 份", "暂未找到通过完整性校验的备份。", "恢复所选备份", "重新检查数据库", "导出恢复资料", "创建全新空数据库"] {
                has(&labels, expected);
            }
            window.update(cx, |view, window, cx| {
                view.recovery_action(&PageAction::Restore, window, cx);
            }).unwrap();
            assert!(sink.try_recv().is_err(), "restore without a backup");
            println!("hidden rendered {reason:?}: {title} / {description}");
        }

        details.backups = (0..9)
            .map(|index| DatabaseRecoveryBackup {
                id: format!("backup-{index}"),
                created_at: 1_700_000_000_000 + 60_000 * (index / 2),
                schema_version: 6,
                size: 2 * 1024 * 1024,
                reason: BackupReason::CleanExit,
                sha256: "0".repeat(64),
            })
            .collect();
        details.error = Some(RecoveryError {
            code: "sample".into(),
            message: "备份读取警告".into(),
            retryable: true,
        });
        synthetic_recovery(cx, &details, 1);
        let (labels, snapshot) = probe_draw(|| draw(window, cx, true));
        let (enabled, style) = snapshot.buttons["database-recovery-restore"];
        assert!(enabled);
        println!("hidden enabled restore: {style:?}");
        assert_eq!(style.font_weight, FontWeight::NORMAL);
        assert_eq!(style.border_width, 1.);
        assert_eq!(style.border, Some(0x3478f6));
        assert_eq!(style.background, style.border);
        assert_eq!(style.text, 0xffffff);
        has(&labels, "7 份");
        has(&labels, "备份读取警告");
        let backups: Vec<_> = labels
            .iter()
            .filter(|text| text.starts_with("backup-"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            backups,
            [
                "backup-8", "backup-7", "backup-6", "backup-5", "backup-4", "backup-3", "backup-2"
            ]
        );
        assert!(
            labels
                .iter()
                .any(|text| text.ends_with(" · 数据版本 6 · 2.0 MB"))
        );

        window
            .update(cx, |view, window, cx| {
                view.recovery_action(&PageAction::Retry, window, cx);
                view.recovery_action(&PageAction::Retry, window, cx);
            })
            .unwrap();
        let Command::DatabaseRecovery {
            request_id,
            action:
                DatabaseRecoveryAction::RetryOpen {
                    expected_recovery_id,
                },
        } = sink.try_recv().unwrap()
        else {
            panic!("expected one retry")
        };
        assert_eq!(expected_recovery_id, details.recovery_id);
        assert!(sink.try_recv().is_err());
        let labels = draw(window, cx, true);
        has(&labels, "正在检查…");
        has(&labels, "正在处理数据库恢复，请保持 Matou 开启…");

        window
            .update(cx, |view, window, cx| {
                view.receive(
                    1,
                    Event::DatabaseRecoveryResult(DatabaseRecoveryResult::failed(
                        "late-request".into(),
                        "open",
                        "迟到错误",
                    )),
                    window,
                    cx,
                );
            })
            .unwrap();
        let labels = draw(window, cx, true);
        has(&labels, "正在检查…");
        assert!(!labels.iter().any(|label| label == "迟到错误"));
        window
            .update(cx, |view, window, cx| {
                view.receive(
                    1,
                    Event::DatabaseRecoveryResult(DatabaseRecoveryResult::failed(
                        request_id,
                        "open",
                        "本次检查失败",
                    )),
                    window,
                    cx,
                );
            })
            .unwrap();
        let labels = draw(window, cx, true);
        has(&labels, "备份读取警告");
        has(&labels, "本次检查失败");
        has(&labels, "重新检查数据库");
        assert!(!labels.iter().any(|text| text == "正在检查…"));

        sink.database_recovery(1, Arc::new(details.clone()));
        window
            .update(cx, |view, window, cx| {
                view.recovery_action(&PageAction::SelectBackup("backup-4".into()), window, cx);
                view.recovery_action(&PageAction::Restore, window, cx);
                view.recovery_action(&PageAction::Export, window, cx);
                view.recovery_action(&PageAction::OpenConfirmation, window, cx);
            })
            .unwrap();
        let Command::DatabaseRecovery {
            request_id,
            action:
                DatabaseRecoveryAction::RestoreBackup {
                    backup_id,
                    expected_recovery_id,
                },
        } = sink.try_recv().unwrap()
        else {
            panic!("expected exactly one restore");
        };
        assert_eq!(
            (backup_id.as_str(), expected_recovery_id.as_str()),
            ("backup-4", details.recovery_id.as_str())
        );
        assert!(sink.try_recv().is_err());
        let labels = draw(window, cx, true);
        has(&labels, "正在恢复…");
        has(&labels, "正在处理数据库恢复，请保持 Matou 开启…");
        window
            .update(cx, |view, window, cx| {
                view.receive(
                    1,
                    Event::DatabaseRecoveryResult(DatabaseRecoveryResult::failed(
                        request_id,
                        "restore",
                        "备份校验失败",
                    )),
                    window,
                    cx,
                );
                view.recovery_action(&PageAction::OpenConfirmation, window, cx);
                view.assert_modal_return_focus(window);
                view.recovery_action(&PageAction::Export, window, cx);
            })
            .unwrap();
        assert!(sink.try_recv().is_err(), "modal blocks underlying actions");
        let (labels, snapshot) = probe_draw(|| draw(window, cx, true));
        has(&labels, "确认创建全新空数据库？");
        has(&labels, "返回");
        has(&labels, "确认创建空数据库");
        has(&labels, "备份校验失败");
        let [back, confirm] = snapshot.bounds["confirmation-buttons"].as_slice() else {
            panic!("expected two confirmation button bounds");
        };
        println!("hidden confirmation buttons: back {back:?}, confirm {confirm:?}");
        near(back.size.width, 48.);
        near(confirm.size.width, 144.);
        near(back.size.height, 28.);
        near(confirm.size.height, 28.);
        near(confirm.origin.x - back.origin.x - back.size.width, 10.);
        for id in [
            "database-recovery-dialog-back",
            "database-recovery-dialog-confirm",
        ] {
            let (enabled, style) = snapshot.buttons[id];
            assert!(enabled);
            assert_eq!(style.radius, 0.);
            assert_eq!(style.border_width, 1.);
        }
        window
            .update(cx, |view, window, cx| {
                view.assert_modal_tab_order(window);
                view.recovery_action(&PageAction::CancelConfirmation, window, cx);
                view.assert_trigger_focus(window);
                view.recovery_action(&PageAction::OpenConfirmation, window, cx);
                view.escape_modal(window, cx);
                view.assert_trigger_focus(window);
                view.recovery_action(&PageAction::ConfirmEmpty, window, cx);
            })
            .unwrap();
        assert!(
            sink.try_recv().is_err(),
            "cancel or an out-of-dialog confirmation cannot open a database"
        );

        for (path, failure, text) in [
            (
                Some("/isolated/recovery/bundle"),
                None,
                "恢复资料已导出到 /isolated/recovery/bundle",
            ),
            (
                None,
                Some("创建导出位置失败: Not a directory"),
                "创建导出位置失败: Not a directory",
            ),
        ] {
            sink.database_recovery(1, Arc::new(details.clone()));
            window
                .update(cx, |view, window, cx| {
                    view.recovery_action(&PageAction::Export, window, cx);
                })
                .unwrap();
            let Command::DatabaseRecovery {
                request_id,
                action: DatabaseRecoveryAction::ExportRecoveryBundle,
            } = sink.try_recv().unwrap()
            else {
                panic!("expected export");
            };
            has(&draw(window, cx, true), "正在导出…");
            window
                .update(cx, |view, window, cx| {
                    view.receive(
                        1,
                        Event::DatabaseRecoveryResult(match failure {
                            Some(message) => DatabaseRecoveryResult::failed(
                                request_id,
                                "DATABASE_EXPORT_FAILED",
                                message,
                            ),
                            None => DatabaseRecoveryResult {
                                request_id,
                                exported_path: path.map(Into::into),
                                error: None,
                            },
                        }),
                        window,
                        cx,
                    );
                })
                .unwrap();
            has(&draw(window, cx, true), text);
        }

        // An old generation's confirmation and reply must never target a new recovery.
        window
            .update(cx, |view, window, cx| {
                view.recovery_action(&PageAction::OpenConfirmation, window, cx);
            })
            .unwrap();
        let mut other = details.clone();
        other.recovery_id = "probe-new-cycle".into();
        synthetic_recovery(cx, &other, 2);
        window
            .update(cx, |view, window, cx| {
                view.recovery_action(&PageAction::ConfirmEmpty, window, cx);
                view.receive(
                    1,
                    Event::DatabaseRecoveryResult(DatabaseRecoveryResult::failed(
                        "old".into(),
                        "restore",
                        "上一代错误",
                    )),
                    window,
                    cx,
                );
            })
            .unwrap();
        let labels = draw(window, cx, true);
        assert!(
            !labels
                .iter()
                .any(|text| text == "确认创建全新空数据库？" || text == "上一代错误")
        );
        assert!(sink.try_recv().is_err());
        synthetic_recovery(cx, &details, 2);
        window
            .update(cx, |view, window, cx| {
                view.recovery_action(&PageAction::OpenConfirmation, window, cx);
                view.assert_modal_return_focus(window);
                view.recovery_action(&PageAction::ConfirmEmpty, window, cx);
                view.recovery_action(&PageAction::ConfirmEmpty, window, cx);
            })
            .unwrap();
        let Command::DatabaseRecovery {
            request_id: empty_request,
            action:
                DatabaseRecoveryAction::StartEmptyDatabase {
                    expected_recovery_id,
                },
        } = sink.try_recv().unwrap()
        else {
            panic!("expected one confirmed empty action");
        };
        assert_eq!(expected_recovery_id, details.recovery_id);
        assert!(sink.try_recv().is_err());
        has(
            &draw(window, cx, true),
            "正在处理数据库恢复，请保持 Matou 开启…",
        );
        window
            .update(cx, |view, window, cx| {
                view.receive(
                    2,
                    Event::DatabaseRecoveryResult(DatabaseRecoveryResult {
                        request_id: empty_request,
                        exported_path: None,
                        error: None,
                    }),
                    window,
                    cx,
                );
            })
            .unwrap();
        has(
            &draw(window, cx, true),
            "正在处理数据库恢复，请保持 Matou 开启…",
        );
        println!(
            "hidden verified recovery requests, confirmation and return focus, progress, feedback and generation isolation"
        );

        // Use the real writable startup projection. A bridge/connection Ready
        // announcement alone must leave the current recovery page selected.
        // Keep the fixture path short enough for the runtime's Unix socket.
        let id = uuid::Uuid::new_v4().simple().to_string();
        let scratch = std::env::current_dir().unwrap().join(".tmp").join(&id[..8]);
        let mut host = RuntimeHost::start(LaunchOptions {
            data_root: scratch.join("data"),
            workspace: scratch.join("workspace"),
            shell_home: scratch.join("home"),
            shell: "/bin/zsh".into(),
        })
        .unwrap();
        let ready = loop {
            match host.events.recv().await.unwrap() {
                event @ Event::Ready { .. } => break event,
                Event::Diagnostic(error) => panic!("isolated Runtime startup: {error}"),
                Event::Closed => panic!("isolated Runtime closed before its projection"),
                _ => {}
            }
        };
        let Event::Ready {
            hierarchy,
            scenes,
            graph,
            geometry,
        } = &ready
        else {
            unreachable!()
        };
        let projection = ReadOnlyProjection {
            workspaces: hierarchy.workspace.clone().into_iter().collect(),
            tasks: hierarchy.task.clone().into_iter().collect(),
            active: ReadOnlyNavigation {
                hierarchy: hierarchy.clone(),
                scenes: scenes.clone(),
                graph: graph.clone(),
                geometry: geometry.clone(),
            },
        };
        let writable_ready = Event::Ready {
            hierarchy: hierarchy.clone(),
            scenes: scenes.clone(),
            graph: graph.clone(),
            geometry: geometry.clone(),
        };
        let (normal_commands, normal_sink) = runtime_bridge::probe_channel();
        let (_normal_events, normal_incoming) = async_channel::unbounded();
        normal_sink.starting();
        let normal = hidden_window(
            cx,
            normal_commands,
            normal_incoming,
            size(px(1200.), px(780.)),
        );
        assert!(expect_root(normal, cx, "startup-neutral").is_empty());
        normal_sink.generation(1);
        normal
            .update(cx, |view, window, cx| {
                view.change(1, ConnectionPhase::Ready, window, cx);
            })
            .unwrap();
        assert!(expect_root(normal, cx, "startup-neutral").is_empty());
        let normal_projection = Event::Ready {
            hierarchy: hierarchy.clone(),
            scenes: scenes.clone(),
            graph: graph.clone(),
            geometry: geometry.clone(),
        };
        normal
            .update(cx, |view, window, cx| {
                view.receive(1, normal_projection, window, cx);
                assert!(view.workspaces_ready());
            })
            .unwrap();
        expect_root(normal, cx, "workspace-root");
        draw(normal, cx, false);
        println!(
            "hidden normal startup roots: startup-neutral -> startup-neutral at bridge Ready -> workspace-root at projection"
        );

        sink.generation(2);
        window
            .update(cx, |view, window, cx| {
                view.change(2, ConnectionPhase::Ready, window, cx);
            })
            .unwrap();
        has(
            &draw(window, cx, true),
            "正在处理数据库恢复，请保持 Matou 开启…",
        );
        window
            .update(cx, |view, window, cx| {
                view.receive(2, ready, window, cx);
                assert!(view.workspaces_ready());
            })
            .unwrap();
        assert!(
            !draw(window, cx, false)
                .iter()
                .any(|label| label == "重新检查数据库")
        );
        println!(
            "hidden verified seven backups, progress, error correlation and one Ready projection handoff"
        );

        sink.read_only_mode(2);
        window
            .update(cx, |view, window, cx| {
                view.change(2, ConnectionPhase::ReadOnly { pending: None }, window, cx);
                view.receive(
                    2,
                    Event::ReadOnlyReady {
                        reason: ReadOnlyReason::NewerSchema,
                        projection: Ok(projection),
                    },
                    window,
                    cx,
                );
                view.verify_mutations_disabled(window, cx);
            })
            .unwrap();
        let labels = draw(window, cx, false);
        for expected in [
            "数据库处于只读恢复模式",
            "现有工作空间、事项和会话仍可浏览、搜索与复制；可能改动数据的操作已暂停。",
            "搜索当前终端",
            "导出数据库资料",
        ] {
            has(&labels, expected);
        }
        let Command::History(HistoryRequest::Page {
            session_id,
            sequence,
            before: None,
        }) = sink.try_recv().unwrap()
        else {
            panic!("expected a latest offline page")
        };
        assert!(sink.try_recv().is_err());
        let cursor = HistoryCursor {
            sequence: 4,
            line_index: 2,
        };
        window
            .update(cx, |view, window, cx| {
                view.receive(
                    2,
                    Event::History(HistoryReply::Page {
                        session_id: session_id.clone(),
                        sequence,
                        before: None,
                        result: Ok(HistoryPage {
                            lines: vec![HistoryLine {
                                sequence: 4,
                                cursor,
                                text: "新页哨兵".into(),
                            }],
                            has_more: true,
                            ..HistoryPage::default()
                        }),
                    }),
                    window,
                    cx,
                );
                assert_eq!(view.read_only_page().unwrap().lines[0].text, "新页哨兵");
                view.request_older(cursor, window, cx);
            })
            .unwrap();
        let Command::History(HistoryRequest::Page {
            session_id: older_session,
            sequence,
            before: Some(before),
        }) = sink.try_recv().unwrap()
        else {
            panic!("expected a cursor-based offline page")
        };
        assert_eq!((older_session, before), (session_id.clone(), cursor));
        window
            .update(cx, |view, window, cx| {
                view.receive(
                    2,
                    Event::History(HistoryReply::Page {
                        session_id: session_id.clone(),
                        sequence,
                        before: Some(cursor),
                        result: Ok(HistoryPage {
                            lines: vec![HistoryLine {
                                sequence: 3,
                                cursor: HistoryCursor {
                                    sequence: 3,
                                    line_index: 0,
                                },
                                text: "早页哨兵".into(),
                            }],
                            ..HistoryPage::default()
                        }),
                    }),
                    window,
                    cx,
                );
                assert_eq!(view.read_only_page().unwrap().lines[0].text, "早页哨兵");
            })
            .unwrap();

        let query = "历史搜索哨兵";
        let hit = HistoryLine {
            sequence: 7,
            cursor: HistoryCursor {
                sequence: 7,
                line_index: 0,
            },
            text: query.into(),
        };
        window
            .update(cx, |view, window, cx| {
                view.search_read_only(query, window, cx)
            })
            .unwrap();
        let Command::History(HistoryRequest::Search {
            sequence,
            query: sent_query,
            session_id: sent_id,
            ..
        }) = sink.try_recv().unwrap()
        else {
            panic!("read-only search must use the Journal, not a PTY")
        };
        assert_eq!(
            (sent_query.as_str(), sent_id.as_str()),
            (query, session_id.as_str())
        );
        window
            .update(cx, |view, window, cx| {
                view.receive(
                    2,
                    Event::History(HistoryReply::Search {
                        session_id: session_id.clone(),
                        sequence,
                        result: Ok(HistorySearchResult {
                            matches: vec![hit.clone()],
                            ..HistorySearchResult::default()
                        }),
                    }),
                    window,
                    cx,
                );
            })
            .unwrap();
        let Command::History(HistoryRequest::Context {
            sequence: selected,
            cursor: selected_cursor,
            session_id: selected_id,
        }) = sink.try_recv().unwrap()
        else {
            panic!("expected offline search context request")
        };
        assert_eq!(
            (selected, selected_cursor, selected_id.as_str()),
            (sequence, hit.cursor, session_id.as_str())
        );
        window
            .update(cx, |view, window, cx| {
                view.receive(
                    2,
                    Event::History(HistoryReply::Context {
                        session_id,
                        sequence,
                        cursor: hit.cursor,
                        result: Ok(HistoryPage {
                            lines: vec![hit],
                            anchor_index: Some(0),
                            ..HistoryPage::default()
                        }),
                    }),
                    window,
                    cx,
                );
                let page = view.read_only_page().unwrap();
                assert_eq!(page.lines[page.anchor_index.unwrap()].text, query);
            })
            .unwrap();

        for (path, error, label) in [
            (
                Some("/isolated/recovery/export"),
                None,
                "数据库资料已导出到 /isolated/recovery/export",
            ),
            (None, None, "数据库资料已导出"),
            (None, Some("测试目的地拒写"), "导出失败：测试目的地拒写"),
        ] {
            window
                .update(cx, |view, _, cx| {
                    view.export(cx);
                    view.export(cx);
                })
                .unwrap();
            let Command::DatabaseRecovery {
                request_id,
                action: DatabaseRecoveryAction::ExportRecoveryBundle,
            } = sink.try_recv().unwrap()
            else {
                panic!("expected a single export request")
            };
            assert!(sink.try_recv().is_err());
            has(&draw(window, cx, false), "正在导出…");
            // In production the transport has already cleared the pending ID before delivery.
            sink.read_only_mode(2);
            window
                .update(cx, |view, window, cx| {
                    let result = match error {
                        Some(error) => DatabaseRecoveryResult::failed(
                            request_id,
                            "DATABASE_EXPORT_FAILED",
                            error,
                        ),
                        None => DatabaseRecoveryResult {
                            request_id,
                            exported_path: path.map(Into::into),
                            error: None,
                        },
                    };
                    view.receive(2, Event::DatabaseRecoveryResult(result), window, cx);
                })
                .unwrap();
            let labels = draw(window, cx, false);
            has(&labels, label);
            has(&labels, "导出数据库资料");
            assert!(!labels.iter().any(|text| text == "正在导出…"));
        }
        sink.generation(2);
        window
            .update(cx, |view, window, cx| {
                view.change(2, ConnectionPhase::Ready, window, cx);
            })
            .unwrap();
        has(&draw(window, cx, false), "数据库处于只读恢复模式");
        window
            .update(cx, |view, window, cx| {
                view.receive(2, writable_ready, window, cx);
                assert!(view.workspaces_ready());
            })
            .unwrap();
        assert!(
            !draw(window, cx, false)
                .iter()
                .any(|text| text == "数据库处于只读恢复模式")
        );
        println!(
            "hidden verified read-only banner, disabled controls, paging, export progress/success/failure and writable handoff"
        );
        host.shutdown().unwrap();
        verify_invalid_marker(cx, &scratch).await;
        std::fs::remove_dir_all(scratch).unwrap();
    }

    pub fn run() {
        Application::new()
            .with_assets(crate::assets::NativeAssets)
            .run(|cx| {
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
    probe::run();
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn main() {}
