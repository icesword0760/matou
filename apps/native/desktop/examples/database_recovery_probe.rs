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

        pub(super) fn retry(&mut self, cx: &mut Context<Self>) {
            let id = self
                .database_recovery
                .as_ref()
                .unwrap()
                .details
                .recovery_id
                .clone();
            self.retry_database_recovery(&id, cx);
        }

        pub(super) fn recovery_selected(&self) -> bool {
            self.database_recovery.is_some()
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
    use std::{cell::RefCell, sync::Arc};

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
    use crate::workspace::Workspace;

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

    async fn verify(cx: &mut AsyncApp) {
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_event_sender, incoming) = async_channel::unbounded();
        let window = cx
            .open_window(
                WindowOptions {
                    show: false,
                    focus: false,
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(460.), px(430.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
            )
            .unwrap();

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

        for (index, (reason, title, description)) in [
            (DatabaseRecoveryReason::Ownership, "数据库占用状态需要处理", "数据库占用记录或接管状态异常，原数据库仍保留在原位置。处理前不会将其当作损坏文件移动。"),
            (DatabaseRecoveryReason::Wal, "数据库需要恢复", "数据库日志状态不完整。Matou 已保留原数据库和日志文件，请选择备份恢复或导出资料。"),
            (DatabaseRecoveryReason::Integrity, "数据库需要恢复", "数据库完整性检查未通过。Matou 已保留原文件，不会直接进入全新空工作区。"),
        ].into_iter().enumerate() {
            details.reason = reason;
            details.recovery_id = format!("probe-{index}");
            synthetic_recovery(cx, &details, 1);
            let labels = draw(window, cx, true);
            for expected in ["Matou 数据恢复", title, description, "可用备份", "0 份", "暂未找到通过完整性校验的备份。", "重新检查数据库"] {
                has(&labels, expected);
            }
            for hidden in ["恢复所选备份", "导出恢复资料", "创建全新空数据库"] {
                assert!(!labels.iter().any(|text| text == hidden));
            }
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
        let labels = draw(window, cx, true);
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
            .update(cx, |view, _, cx| {
                view.retry(cx);
                view.retry(cx);
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

        // Use the real writable startup projection. A bridge/connection Ready
        // announcement alone must leave the current recovery page selected.
        let scratch = std::env::current_dir()
            .unwrap()
            .join(".tmp/g-storage-recovery")
            .join(format!("gpui-{}", uuid::Uuid::new_v4()));
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
        sink.generation(1);
        window
            .update(cx, |view, window, cx| {
                view.change(1, ConnectionPhase::Ready, window, cx);
            })
            .unwrap();
        has(&draw(window, cx, true), "本次检查失败");
        window
            .update(cx, |view, window, cx| {
                view.receive(1, ready, window, cx);
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
