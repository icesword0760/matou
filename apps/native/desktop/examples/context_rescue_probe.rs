//! Visible offline GPUI probe for context rescue and truncated logs.
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
mod terminal_surface;
#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../src/ui_text.rs"]
mod ui_text;

#[cfg(all(target_os = "macos", debug_assertions))]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    use crate::assets::NativeAssets;
    use matou_native_terminal::{Terminal, TerminalOptions};
    use serde_json::json;

    const A: &str = "rescue-source";
    const B: &str = "rescue-destination";

    fn graph(session_id: &str) -> Value {
        json!({
            "sceneId":"main", "focusedSessionId":session_id, "workspaceSessionCount":2,
            "nodes":[
                {"sessionId":A, "currentMode":"claude-code", "title":"后台构建会话",
                    "workStatus":"running", "contextCardAlert":"critical", "permissionMode":"default"},
                {"sessionId":B, "currentMode":"shell", "title":"新会话（离线样例）", "workStatus":"idle"}
            ]
        })
    }

    fn navigation(session_id: &str) -> Event {
        let task = json!({
            "id":"task", "workspaceId":"workspace", "title":"离线验收", "status":"open",
            "executionContextId":"local", "sortKey":"a", "isPinned":false, "pinSortKey":"",
            "lastOpenedAt":0, "createdAt":0, "updatedAt":0, "version":1
        });
        let workspace = json!({
            "id":"workspace", "name":"救援验收工作区", "rootDirectory":".", "taskOrder":["task"],
            "isDefault":true, "isPinned":false, "pinSortKey":"", "lastOpenedAt":0,
            "createdAt":0, "updatedAt":0, "version":1
        });
        let scene = json!({
            "id":"main", "taskId":"task", "name":"会话救援", "mode":"terminal", "titlePinned":false,
            "sortKey":"a", "layoutRevision":0, "createdAt":0, "updatedAt":0
        });
        let (kind, title) = if session_id == A {
            ("claude-code", "后台构建会话")
        } else {
            ("shell", "新会话（离线样例）")
        };
        Event::Navigation {
            hierarchy: serde_json::from_value(json!({
                "workspace":workspace, "task":task, "scene":scene,
                "session":{"id":session_id,"taskId":"task","executionContextId":"local","kind":kind,"status":"active",
                "title":title,"cwd":".","createdAt":0,"updatedAt":0,"lastActivityAt":0,"version":1},
                "navigation":{"windowId":"context-rescue-probe","activeWorkspaceId":"workspace","taskByWorkspace":{},
                "sceneByTask":{},"sessionByScene":{"main":session_id}}
            })).unwrap(),
            scenes: vec![serde_json::from_value(scene).unwrap()],
            graph: graph(session_id),
            geometry: vec![],
        }
    }

    fn install_fixture(view: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        view.locale = InterfaceLocale::Chinese;
        view.runtime_generation = 1;
        view.transport_connected = true;
        view.domain_ready = true;
        let event = navigation(A);
        if let Event::Navigation { hierarchy, .. } = &event {
            view.apply_workspaces(hierarchy.workspace.clone().into_iter().collect(), cx);
            view.apply_tasks(hierarchy.task.clone().into_iter().collect(), cx);
        }
        view.apply(
            BridgeEvent::Runtime {
                generation: 1,
                event,
            },
            window,
            cx,
        );
        for id in [A, B] {
            view.session_frames.insert(id.into());
            view.first_frame_content.insert(id.into());
            view.restoring_sessions.remove(id);
            view.recovery.insert(id.into(), RecoveryState::Ready);
        }
        view.sync_terminal_input(window, cx);
        window.focus(&view.control_focus["sidebar-board"]);
    }

    pub(super) fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
            let (commands, sink) = crate::runtime_bridge::probe_channel();
            let (send, events) = async_channel::unbounded();
            let bounds = Bounds::centered(None, size(px(1360.), px(760.)), cx);
            let handle = cx.open_window(
                WindowOptions {
                    show: true,
                    focus: true,
                    titlebar: Some(TitlebarOptions {
                        title: Some("Matou Context Rescue Probe".into()),
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(commands, events, window, cx)),
            ).unwrap();
            handle.update(cx, install_fixture).unwrap();
            cx.activate(true);
            println!("context-rescue-probe: visible");
            cx.spawn(async move |cx| {
                let guard = json!({
                    "contextPressure":"critical", "savedNoiseBytes":282624,
                    "cache":{"timing":"precise","remainingSec":221,"warm":true,"hitRatioPercent":83},
                    "handoffReady":true
                });
                let logs = json!([
                    {"id":"log_0042","command":"cargo test --workspace","rawBytes":216541,
                     "preview":"test build_matrix ... FAILED"},
                    {"id":"log_0041","command":"pnpm check","rawBytes":65534,
                     "preview":"packages/runtime: completed"}
                ]);
                let mut sequence = 0;
                loop {
                    while let Ok(command) = sink.try_recv() {
                        let (kind, request, result) = match command {
                            Command::GetAgentHud { request_id, session_id } if session_id == A => (
                                "agentHudResult", request_id, json!({"hud":{
                                    "model":"Claude", "contextPercent":89, "contextWindowSize":200000,
                                    "taskStatus":"running", "contextGuard":guard
                                }})
                            ),
                            Command::GetContextGuardDetails { request_id, session_id } => (
                                "contextGuardDetailsResult", request_id, json!({
                                    "sessionId":session_id, "cache":guard["cache"], "logs":logs
                                })
                            ),
                            Command::GetContextGuardLog { request_id, session_id, log_id } => (
                                "contextGuardLogResult", request_id, json!({
                                    "sessionId":session_id, "id":log_id,
                                    "preview":"running 42 tests\ntest build_matrix ... FAILED\nerror: fixture configuration unavailable\nAPI_KEY=[REDACTED]\ntest result: FAILED. 41 passed; 1 failed"
                                })
                            ),
                            Command::GetSessionHandoffBrief { request_id, .. } => (
                                "sessionHandoffBriefResult", request_id, json!({
                                    "markdown":"# 会话交接摘要\n- 上一次测试：cargo test --workspace，见 log_0042"
                                })
                            ),
                            Command::RescueSessionWithHandoff { request_id, .. } => (
                                "rescueSessionWithHandoffResult", request_id, json!({
                                    "sceneId":"main", "sessionId":B, "delivery":"clipboard_fallback",
                                    "markdown":"# 会话交接摘要\n- 上一次测试：cargo test --workspace，见 log_0042"
                                })
                            ),
                            Command::FocusSession { scene_id, session_id } => {
                                println!("context-rescue-probe: focus {scene_id}/{session_id}");
                                let _ = send.send(BridgeEvent::Runtime {
                                    generation: 1,
                                    event: navigation(&session_id),
                                }).await;
                                continue;
                            }
                            Command::Resize { session_id, cols, rows, cell_width_px, cell_height_px } if cols > 0 && rows > 0 => {
                                let mut terminal = Terminal::new(TerminalOptions::new(cols, rows, cell_width_px, cell_height_px)).unwrap();
                                let message = if session_id == A {
                                    "$ cargo test --workspace\r\n    Finished test profile\r\n     Running tests\r\nerror: fixture configuration unavailable\r\n"
                                } else {
                                    "$ "
                                };
                                terminal.feed(message.as_bytes());
                                sequence += 1;
                                let _ = send.send(BridgeEvent::Runtime {
                                    generation: 1,
                                    event: Event::Frame {
                                        session_id, screen: terminal.screen().unwrap(),
                                        selection: None, modes: terminal.input_modes().unwrap(),
                                        sequence, scroll_request_id: 0,
                                    },
                                }).await;
                                continue;
                            }
                            _ => continue,
                        };
                        println!("context-rescue-probe: {kind}");
                        if send.send(BridgeEvent::Runtime {
                            generation: 1,
                            event: Event::Graph(json!({"type":kind, "requestId":request, "result":result})),
                        }).await.is_err() {
                            return;
                        }
                    }
                    cx.background_executor().timer(Duration::from_millis(25)).await;
                }
            }).detach();
        });
    }
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn main() {
    workspace::run();
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn main() {}
