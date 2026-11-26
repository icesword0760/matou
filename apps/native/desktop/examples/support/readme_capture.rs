// Offline README fixture. All pixels are painted by the desktop views.
use super::*;
use crate::runtime_bridge::ProbeReceiver;
use crate::settings_shell::{
    SettingsPage, claude_role_assignments::AssignmentAction, probe_provider_editor,
};
use matou_native_core::dispatch_model_catalog::{CATALOG_FILENAME, DispatchModelCatalog};
use matou_native_core::dispatch_roles::{DispatchRolesConfig, DispatchRolesStore};
use matou_native_desktop::dag_canvas::{DagGraph, DagLayout};
use matou_native_runtime::host::LaunchOptions;
use matou_native_runtime::protocol::ProviderSettingsResult;
use matou_native_terminal::{Terminal, TerminalOptions};
use std::path::{Path, PathBuf};

const CLAUDE: &str = "payment-review";
const CODEX: &str = "payment-tests";
const GOOGLE: &str = "payment-flow";
const DSH: &str = "payment-audit";
const PROJECT: &str = "~/projects/acme-payments";
const REPO: &str = "~/projects/acme-payments/pay-service";

struct Demo {
    locale: InterfaceLocale,
    output: PathBuf,
}

impl Demo {
    fn text(&self, zh: &'static str, en: &'static str) -> &'static str {
        self.locale.text(zh, en)
    }

    fn open(&self, cx: &mut AsyncApp) -> (Handle, ProbeReceiver) {
        let root = self.output.join("profile");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("notification-system"), b"0").unwrap();
        std::fs::write(root.join("notification-sound"), b"0").unwrap();
        write_theme_preference(&root, ThemePreference::Light).unwrap();
        let mut dispatch = DispatchRolesConfig::default();
        let roles = [
            self.text(
                "实现支付功能、补齐回归测试并检查错误处理。",
                "Implement payment features, add regression tests, and check error handling.",
            ),
            self.text(
                "审查支付日志、核对重试链路并整理调查结果。",
                "Audit payment logs, verify retry behavior, and summarize findings.",
            ),
            self.text(
                "验证结账流程、演练回调重放并记录异常。",
                "Verify checkout, exercise webhook replay, and document failures.",
            ),
        ];
        for (agent, role) in dispatch.agents.iter_mut().zip(roles) {
            agent.role = role.into();
        }
        dispatch.agents[1].model = "deepseek-v4-pro".into();
        dispatch.agents[1].effort = "max".into();
        dispatch.agents[2].model = "carbon".into();
        DispatchRolesStore::for_data_root(&root)
            .save(dispatch)
            .unwrap();
        let mut catalog = serde_json::to_value(DispatchModelCatalog::default()).unwrap();
        catalog["refreshedAt"] = json!(notification_now() / 1000);
        std::fs::write(
            root.join(CATALOG_FILENAME),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        cx.update(|cx| {
            let (commands, sink) = crate::runtime_bridge::probe_channel();
            let (_, events) = async_channel::unbounded();
            let bounds = Bounds::centered(None, size(px(1460.), px(898.)), cx);
            let handle = cx
                .open_window(
                    WindowOptions {
                        show: false,
                        focus: false,
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some(self.text("码头", "Matou").into()),
                            appears_transparent: true,
                            traffic_light_position: Some(point(px(16.), px(17.))),
                        }),
                        ..Default::default()
                    },
                    |window, cx| {
                        cx.new(|cx| {
                            Workspace::with_launch_options(
                                commands,
                                events,
                                window,
                                cx,
                                Some(LaunchOptions {
                                    data_root: root.clone(),
                                    workspace: root.clone(),
                                    shell_home: root,
                                    shell: "/bin/sh".into(),
                                }),
                            )
                        })
                    },
                )
                .unwrap();
            handle
                .update(cx, |view, window, cx| self.install(view, window, cx))
                .unwrap();
            (handle, sink)
        })
        .unwrap()
    }

    fn install(&self, view: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        view.locale = self.locale;
        view.locale_preference = match self.locale {
            InterfaceLocale::Chinese => LocalePreference::Chinese,
            InterfaceLocale::English => LocalePreference::English,
        };
        view.notifications.system_enabled = false;
        view.runtime_generation = 1;
        view.transport_connected = true;
        view.domain_ready = true;
        let names = [
            self.text("支付重试", "Payment retries"),
            self.text("对账报告", "Reconciliation report"),
            self.text("回调重放", "Webhook replay"),
            self.text("超时修复", "Timeout fix"),
            self.text("告警规则", "Alert rules"),
            self.text("退款审计", "Refund audit"),
            self.text("测试基线", "Test baseline"),
        ];
        let ids = [
            "retries", "report", "replay", "timeout", "alerts", "refund", "baseline",
        ];
        let tasks: Vec<Task> = ids
            .into_iter()
            .zip(names)
            .zip([
                "active",
                "planned",
                "blocked",
                "completed",
                "planned",
                "active",
                "completed",
            ])
            .enumerate()
            .map(|(i, ((id, title), status))| {
                serde_json::from_value(json!({
                    "id":id, "workspaceId":"acme", "title":title, "status":status,
                    "executionContextId":"local", "sortKey":i.to_string(),
                    "isPinned":false, "pinSortKey":"", "lastOpenedAt":0,
                    "createdAt":0, "updatedAt":0, "version":1
                }))
                .unwrap()
            })
            .collect();
        let workspace: HierarchyWorkspace = serde_json::from_value(json!({
            "id":"acme", "name":"acme-payments", "rootDirectory":PROJECT, "taskOrder":ids,
            "isDefault":true, "isPinned":false, "pinSortKey":"",
            "lastOpenedAt":0, "createdAt":0, "updatedAt":0, "version":1
        }))
        .unwrap();
        let scenes: Vec<Scene> = [
            ("payments", self.text("支付服务", "Payment service")),
            ("integration", self.text("集成验证", "Integration checks")),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (id, name))| {
            serde_json::from_value(json!({
                "id":id, "taskId":"retries", "name":name, "mode":"terminal",
                "titlePinned":false, "sortKey":i.to_string(), "layoutRevision":0,
                "createdAt":0, "updatedAt":0
            }))
            .unwrap()
        })
        .collect();
        let roots = self.nodes();
        let graph = json!({
            "sceneId":"payments", "focusedSessionId":CLAUDE, "workspaceSessionCount":12,
            "workspaceSessionCountByTask":{"retries":5,"replay":2,"timeout":1,"refund":2,"baseline":2},
            "nodes":roots, "edges":[]
        });
        let hierarchy = serde_json::from_value(json!({
            "workspace":workspace, "task":tasks[0], "scene":scenes[0],
            "session":session(CLAUDE, self.text("支付审查", "Review")),
            "navigation":{"windowId":"readme-capture", "activeWorkspaceId":"acme",
                "taskByWorkspace":{}, "sceneByTask":{}, "sessionByScene":{"payments":CLAUDE}}
        }))
        .unwrap();
        view.apply_workspaces(vec![workspace], cx);
        view.apply_tasks(tasks, cx);
        runtime_event(
            view,
            Event::Navigation {
                hierarchy,
                scenes,
                graph,
                geometry: vec![],
            },
            window,
            cx,
        );
        for (index, id) in [CLAUDE, CODEX, GOOGLE].into_iter().enumerate() {
            view.session_frames.insert(id.into());
            view.first_frame_content.insert(id.into());
            view.recovery.insert(id.into(), RecoveryState::Ready);
            self.feed(view, id, index, cx);
        }
        view.focus_follow_until = None;
        view.restore_geometry = false;
        view.sync_terminal_input(window, cx);

        let mut status = frozen_status();
        status.state = EgressState::Ok;
        status.families = [
            EgressFamily::Anthropic,
            EgressFamily::OpenAi,
            EgressFamily::Google,
        ]
        .into_iter()
        .map(|family| EgressFamilyStatus {
            family,
            state: EgressState::Ok,
            exit: Some(EgressExit {
                ip: "203.0.113.24".into(),
                country: "US".into(),
            }),
            baseline: None,
            violation: None,
            overridden: false,
            frozen_session_ids: vec![],
        })
        .collect();
        status.session_families = HashMap::from([
            (CLAUDE.into(), EgressFamily::Anthropic),
            (CODEX.into(), EgressFamily::OpenAi),
            (GOOGLE.into(), EgressFamily::Google),
        ]);
        status.last_transition = None;
        view.set_egress_status(status, window, cx);
    }

    fn nodes(&self) -> Vec<Value> {
        [
            (
                CLAUDE,
                "claude-code",
                self.text("支付审查", "Review"),
                "running",
            ),
            (CODEX, "codex", self.text("回归测试", "Tests"), "running"),
            (
                GOOGLE,
                "antigravity",
                self.text("支付流程", "Flow"),
                "needs-input",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (id, mode, title, status))| {
            json!({
                "sessionId":id, "currentMode":mode, "title":title, "titleSource":"manual",
                "cwd":REPO, "workStatus":status, "siblingCreatedSeq":index + 1,
                "permissionMode":if mode == "claude-code" { "acceptEdits" } else { "default" },
                "git":{"state":"ready","branch":"feat/payment-retries","dirty":false},
                "environment":{"kind":"local","state":"ready","path":REPO},
                "terminalActive":true, "historyProviderSessionId":"demo-payment-transcript",
                "latestLines":[match id {
                    CODEX => self.text("幂等性回归测试通过", "Idempotency tests passed"),
                    CLAUDE => self.text("正在核对重试路径", "Reviewing the retry path"),
                    _ => self.text("等待确认支付回调", "Awaiting checkout approval"),
                }]
            })
        })
        .collect()
    }

    fn feed(&self, view: &mut Workspace, id: &str, index: usize, cx: &mut Context<Workspace>) {
        let providers = ["Claude Code", "Codex", "Antigravity", "DSH"];
        let accent = [35, 34, 36, 32][index];
        let lines = match index {
            0 => [
                self.text(
                    "检查支付失败后的重试逻辑",
                    "Review retries after a failed payment",
                ),
                "src/payments/retry.rs",
                self.text("已核对超时与退避策略", "Timeout and backoff verified"),
                self.text("已验证请求幂等性", "Idempotency checks passed"),
                self.text("正在检查重复回调", "Checking duplicate webhooks"),
                self.text("继续审查退款边界", "Continue with refund edge cases"),
            ],
            1 => [
                self.text("补齐支付回归测试", "Add payment regression tests"),
                "tests/payment_retry.rs",
                self.text("超时后可安全重试", "Safe retry after timeout"),
                self.text("重复请求只扣款一次", "Duplicate requests charge once"),
                self.text("运行测试套件 · 12 项通过", "Running checks · 12 passed"),
                self.text("正在整理测试报告", "Preparing the test report"),
            ],
            2 => [
                self.text("验证结账与支付回调", "Verify checkout and payment webhooks"),
                "checkout / webhook",
                self.text("结账状态同步正常", "Checkout state sync verified"),
                self.text("回调签名已校验", "Webhook signature verified"),
                self.text("等待确认 · 回调重放", "Approval needed · Replay webhook"),
                self.text("确认后继续", "Continue after approval"),
            ],
            _ => [
                self.text("审查退款与对账日志", "Audit refund and reconciliation logs"),
                "src/payments/refund.rs",
                self.text("已核对重复退款", "Duplicate refunds checked"),
                self.text("已确认对账一致", "Ledger entries agree"),
                self.text("正在检查部分退款", "Checking partial refunds"),
                self.text("整理审计结果", "Summarize audit findings"),
            ],
        };
        let mut terminal = Terminal::new(TerminalOptions {
            theme: view.theme_palette().terminal,
            ..TerminalOptions::new(72, 25, 9, 18)
        })
        .unwrap();
        let banner = format!(
            "\x1b[1m{}\x1b[0m\r\n\x1b[90mpay-service\x1b[0m\r\n\r\n\x1b[1m{}\x1b[0m\r\n\r\n\x1b[90m  {}\x1b[0m\r\n\r\n\x1b[32m  ✓\x1b[0m {}\r\n\x1b[32m  ✓\x1b[0m {}\r\n\r\n\x1b[{accent}m  ●\x1b[0m {}\r\n\r\n\x1b[30m❯\x1b[0m {}",
            providers[index], lines[0], lines[1], lines[2], lines[3], lines[4], lines[5],
        );
        terminal.feed(banner.as_bytes());
        let screen = terminal.screen().unwrap();
        view.terminals[id].update(cx, |surface, cx| {
            surface.set_screen(screen, terminal.input_modes().unwrap(), None, 0, cx)
        });
    }

    fn accounts(&self, personal: bool) -> Value {
        let account = |id, name, used, weekly| {
            json!({
                "id":id, "name":name, "email":format!("{id}•••@example.invalid"), "needsLogin":false,
                "usage":{"checkedAt":notification_now(), "stale":false,
                    "fiveHour":{"used":used}, "weekly":{"used":weekly}}
            })
        };
        json!({
            "currentId":if personal { "personal" } else { "work" }, "switched":personal,
            "rememberScope":true,"renewedIds":[], "loginCodeSubmitted":false,
            "accounts":[
                account("work", self.text("工作账户", "Work account"), 94, 81),
                account("personal", self.text("个人账户", "Personal account"), 6, 22),
            ]
        })
    }

    fn hud(&self, id: &str, model: &str, pressure: bool) -> Value {
        let agents = [
            (
                "Codex",
                self.text("补齐回归用例", "Cover retry regressions"),
                "GPT-5.4 · High",
                "running",
            ),
            (
                "DSH",
                self.text("审查支付日志", "Audit payment logs"),
                "DeepSeek V4 Pro · Max",
                "running",
            ),
            (
                "Antigravity",
                self.text("复核结账流程", "Verify checkout flow"),
                "Gemini 3.5 Flash · High",
                "completed",
            ),
        ]
        .into_iter()
        .map(|(agent, task, model, state)| {
            json!({
                "label":format!("{agent} · {task}"), "state":state, "source":"external",
                "detail":model, "durationMs":32_000
            })
        })
        .collect::<Vec<_>>();
        match id {
            CLAUDE => json!({
                "model":model, "permissionMode":"acceptEdits", "contextPercent":if pressure { 87 } else { 42 },
                "contextWindowSize":200000, "contextUsedTokens":if pressure { 174000 } else { 84000 },
                "defaultAccountUsage":true, "resumable":true, "agents":agents, "toolCount":24,
                "configCounts":{"hooks":3,"mcps":0},
                "hookNames":["SessionStart","PreToolUse","PostToolUse"], "gitBranch":"feat/payment-retries",
                "todos":[{"content":self.text("检查幂等性","Check idempotency"),"status":"completed"},
                    {"content":self.text("复核退款","Review refunds"),"status":"in_progress"}],
                "contextGuard":{"contextPressure":if pressure { "critical" } else { "normal" },
                    "savedNoiseBytes":96_000,"cache":{"hitRatioPercent":93,"warm":true}}
            }),
            DSH => json!({
                "model":"DeepSeek V4 Pro", "effort":"max",
                "providerPermissionLabel":self.text("只读","Read only"),
                "contextPercent":23, "resumable":true,
            }),
            _ => {
                let (model, permission, context, five_hour, weekly) = if id == CODEX {
                    ("GPT-5.4", "workspace-write", 31, 21, 34)
                } else {
                    ("Gemini 3.5 Flash", "auto", 26, 38, 18)
                };
                json!({
                    "model":model, "permissionMode":permission, "contextPercent":context, "contextWindowSize":200000,
                    "usageWindows":[{"label":"5h","usedPercent":five_hour},{"label":"Weekly","usedPercent":weekly}],
                    "gitBranch":"feat/payment-retries", "resumable":true,
                })
            }
        }
    }

    fn respond(
        &self,
        window: Handle,
        sink: &ProbeReceiver,
        cx: &mut AsyncApp,
        personal: bool,
        model: &str,
        pressure: bool,
    ) {
        while let Ok(command) = sink.try_recv() {
            let (kind, request, result) = match command {
                Command::GetAgentHud {
                    request_id,
                    session_id,
                } => (
                    "agentHudResult",
                    request_id,
                    json!({"hud":self.hud(&session_id, model, pressure)}),
                ),
                Command::GetClaudeAccounts { request_id }
                | Command::ClaudeAccountCommand { request_id, .. } => {
                    ("claudeAccountsResult", request_id, self.accounts(personal))
                }
                _ => continue,
            };
            window
                .update(cx, |view, window, cx| {
                    view.receive_wiring_reply(kind, request, &json!({"result":result}), window, cx)
                })
                .unwrap();
        }
    }
}

fn session(id: &str, title: &str) -> Value {
    json!({"id":id,"taskId":"retries","executionContextId":"local","kind":"agent",
        "status":"active","title":title,"cwd":REPO,"createdAt":0,"updatedAt":0,
        "lastActivityAt":0,"version":1})
}

fn runtime_event(
    view: &mut Workspace,
    event: Event,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    view.apply(
        BridgeEvent::Runtime {
            generation: 1,
            event,
        },
        window,
        cx,
    );
}

fn paint(handle: AnyWindowHandle, cx: &mut AsyncApp) {
    handle
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
        })
        .unwrap();
}

fn wiring_bounds(handle: Handle, cx: &mut AsyncApp) -> HashMap<String, Bounds<Pixels>> {
    capture_wiring_bounds(|| paint(handle.into(), cx))
}

fn hud_bounds(handle: Handle, cx: &mut AsyncApp) -> HashMap<&'static str, Bounds<Pixels>> {
    capture_hud(|| paint(handle.into(), cx))
}

fn context_card_alert(window: Handle, cx: &mut AsyncApp, critical: bool) {
    window
        .update(cx, |view, window, cx| {
            let mut graph = view.graph.clone();
            let node = graph["nodes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|node| node["sessionId"] == CLAUDE)
                .unwrap();
            if critical {
                node["contextCardAlert"] = json!("critical");
            } else {
                node.as_object_mut().unwrap().remove("contextCardAlert");
            }
            runtime_event(view, Event::Graph(graph), window, cx);
        })
        .unwrap();
}

fn tap(window: Handle, cx: &mut AsyncApp, position: Point<Pixels>) {
    click(window.into(), cx, MouseButton::Left, position);
}

async fn settle(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

async fn capture(handle: AnyWindowHandle, cx: &mut AsyncApp, path: &Path) {
    use cocoa::base::{id, nil};
    use objc::{msg_send, sel, sel_impl};
    let (view, _) = native_view(handle, cx);
    let number: isize = unsafe {
        let native: id = msg_send![view, window];
        // Keep the fixture below existing windows and leave the active app alone.
        let _: () = msg_send![native, orderBack: nil];
        msg_send![native, windowNumber]
    };
    assert!(number > 0, "the compositor did not assign a window number");
    settle(cx, 220).await;
    paint(handle, cx);
    unsafe {
        let _: () = msg_send![view, displayLayer: nil];
    }
    settle(cx, 140).await;
    crate::window_capture::capture_png(number, path).unwrap();
    println!("Captured window {number}: {}", path.display());
}

fn settings(window: Handle, cx: &mut AsyncApp, page: SettingsPage) {
    window
        .update(cx, |view, window, cx| {
            view.toggle_settings(window, cx);
            view.settings
                .as_ref()
                .unwrap()
                .0
                .update(cx, |shell, cx| shell.select_page(page, window, cx));
        })
        .unwrap();
}

fn provider_reply(
    window: Handle,
    cx: &mut AsyncApp,
    request_id: u64,
    result: ProviderSettingsResult,
) {
    window
        .update(cx, |view, window, cx| {
            let mut graph = view.graph.clone();
            graph["providerSettingsResult"] = json!(ProviderSettingsReply { request_id, result });
            runtime_event(view, Event::Graph(graph), window, cx);
        })
        .unwrap();
}

fn provider_control(window: Handle, cx: &mut AsyncApp, control: &str) {
    paint(window.into(), cx);
    window
        .update(cx, |view, window, cx| {
            view.settings
                .as_ref()
                .unwrap()
                .0
                .read(cx)
                .probe_provider_editor_focus(control, window);
        })
        .unwrap();
    press(window, cx, "enter");
}

fn assignment_reply(window: Handle, cx: &mut AsyncApp, request_id: u64, assignments: &Value) {
    window
        .update(cx, |view, window, cx| {
            let mut graph = view.graph.clone();
            graph["claudeRoleAssignmentsResult"] =
                json!({"requestId":request_id, "assignments":assignments});
            runtime_event(view, Event::Graph(graph), window, cx);
        })
        .unwrap();
}

async fn provider(demo: &Demo, window: Handle, sink: &ProbeReceiver, cx: &mut AsyncApp) {
    let provider_data = |id, name, models: &[&str]| {
        json!({
            "id":id, "name":name, "cli":"claude-code", "endpoint":format!("https://{id}.example.invalid/anthropic"),
            "hasApiKey":true, "model":models[0], "models":models, "modelMapping":{
                "opus":models[0],"fable":models[0],"sonnet":models[0],"haiku":models[1]
            }, "effortMapping":{"opus":"high","sonnet":"high"},
            "modelMetadata":models.iter().map(|id| json!({
                "id":id, "displayName":id, "contextWindow":200000, "supportedEfforts":["low","medium","high"]
            })).collect::<Vec<_>>()
        })
    };
    let active = provider_data(
        "deepseek",
        "DeepSeek",
        &["deepseek-v4-flash", "deepseek-v4-pro"],
    );
    let snapshot = json!({
        "providers":{"claude-code":[
            {"id":"anthropic-official","name":demo.text("Claude 官方登录","Official Claude Login"),
                "cli":"claude-code","builtIn":true,"model":demo.text("CLI 默认","CLI default")},
            active,
            provider_data("glm","GLM",&["glm-5","glm-4.7"]),
            provider_data("kimi","Kimi",&["kimi-k2.5","kimi-k2"]),
            provider_data("minimax","MiniMax",&["MiniMax-M2.7","MiniMax-M2.5"])
        ]},
        "activeProviderIds":{"claude-code":"deepseek"}
    });
    // The previous Settings page also requested a provider snapshot on entry.
    demo.respond(window, sink, cx, false, "Opus/deepseek-v4-flash", false);
    settings(window, cx, SettingsPage::ClaudeCode);
    let commands: Vec<_> = std::iter::from_fn(|| sink.try_recv().ok()).collect();
    let provider_request = commands
        .iter()
        .find_map(|command| match command {
            Command::GetProviderSettings { request_id } => Some(*request_id),
            _ => None,
        })
        .unwrap();
    let assignment_request = commands
        .iter()
        .find_map(|command| match command {
            Command::GetClaudeRoleAssignments { request_id } => Some(*request_id),
            _ => None,
        })
        .unwrap();
    provider_reply(
        window,
        cx,
        provider_request,
        ProviderSettingsResult::Snapshot(snapshot),
    );
    assignment_reply(
        window,
        cx,
        assignment_request,
        &json!({
            "opus":{"providerId":"deepseek","model":"deepseek-v4-flash","effort":"high"},
        "fable":{"providerId":"deepseek","model":"deepseek-v4-flash","effort":"medium"},
        "sonnet":{"providerId":"glm","model":"glm-5","effort":"high"},
        "haiku":{"providerId":"kimi","model":"kimi-k2","effort":"low"},
        }),
    );
    demo.save(window, cx, "provider-01").await;

    window
        .update(cx, |view, window, cx| {
            view.settings
                .as_ref()
                .unwrap()
                .0
                .read(cx)
                .diagnostics_focus_provider_edit("deepseek", window);
        })
        .unwrap();
    press(window, cx, "enter");
    paint(window.into(), cx);
    scroll_at(
        window.into(),
        cx,
        point(px(850.), px(590.)),
        core_graphics::event::ScrollEventUnit::PIXEL,
        -550,
        core_graphics::event::CGEventFlags::empty(),
    );
    provider_control(window, cx, "effort-0");
    assert!(
        probe_provider_editor(|| paint(window.into(), cx))
            .bounds
            .contains_key("effort-option-high")
    );
    demo.save(window, cx, "provider-02").await;
    press(window, cx, "escape");
    provider_control(window, cx, "close");
    window
        .update(cx, |view, _, cx| {
            view.settings.as_ref().unwrap().0.update(cx, |shell, cx| {
                shell.diagnostics_assignment_action(
                    &AssignmentAction::Model(0, "deepseek-v4-pro".into()),
                    cx,
                );
                shell.diagnostics_assignment_action(&AssignmentAction::Save, cx);
            });
        })
        .unwrap();
    let (request, assignments) = std::iter::from_fn(|| sink.try_recv().ok())
        .find_map(|command| match command {
            Command::SetClaudeRoleAssignments {
                request_id,
                assignments,
                ..
            } => Some((request_id, assignments)),
            _ => None,
        })
        .expect("role allocation sent its normal hot-switch command");
    assert_eq!(assignments["opus"]["model"], "deepseek-v4-pro");
    assignment_reply(window, cx, request, &assignments);
    paint(window.into(), cx);
    scroll_at(
        window.into(),
        cx,
        point(px(950.), px(550.)),
        core_graphics::event::ScrollEventUnit::PIXEL,
        -390,
        core_graphics::event::CGEventFlags::empty(),
    );
    demo.save(window, cx, "provider-03").await;
    window
        .update(cx, |view, window, cx| view.toggle_settings(window, cx))
        .unwrap();
    // The sandbox replies to the regular HUD poll for the same live card.
    settle(cx, 2200).await;
    demo.respond(window, sink, cx, false, "Opus/deepseek-v4-pro", false);
    demo.save(window, cx, "provider-04").await;
}

impl Demo {
    async fn save(&self, handle: impl Into<AnyWindowHandle>, cx: &mut AsyncApp, name: &str) {
        capture(handle.into(), cx, &self.output.join(format!("{name}.png"))).await;
    }

    async fn account(&self, window: Handle, sink: &ProbeReceiver, cx: &mut AsyncApp) {
        self.save(window, cx, "account-01").await;
        let target = wiring_bounds(window, cx)["footer-account"].center();
        tap(window, cx, target);
        self.respond(window, sink, cx, false, "Opus/deepseek-v4-flash", false);
        self.save(window, cx, "account-02").await;
        let target = wiring_bounds(window, cx)["claude-account-switch-personal"].center();
        tap(window, cx, target);
        self.respond(window, sink, cx, true, "Opus/deepseek-v4-flash", false);
        press(window, cx, "escape");
        self.save(window, cx, "account-03").await;
    }

    async fn dispatch(&self, window: Handle, cx: &mut AsyncApp) {
        settings(window, cx, SettingsPage::Dispatch);
        settle(cx, 400).await;
        self.save(window, cx, "dispatch-01").await;
        window
            .update(cx, |view, window, cx| view.toggle_settings(window, cx))
            .unwrap();
        self.save(window, cx, "dispatch-02").await;
        let agent = hud_bounds(window, cx)["agents"].center();
        dispatch_mouse(
            window,
            cx,
            MouseMoveEvent {
                position: agent,
                ..Default::default()
            },
        );
        paint(window.into(), cx);
        settle(cx, 900).await;
        self.save(window, cx, "dispatch-03").await;
        dispatch_mouse(
            window,
            cx,
            MouseMoveEvent {
                position: point(px(1.), px(1.)),
                ..Default::default()
            },
        );
        paint(window.into(), cx);
        settle(cx, 620).await;
    }

    async fn context(&self, window: Handle, sink: &ProbeReceiver, cx: &mut AsyncApp) {
        settings(window, cx, SettingsPage::ContextOptimizer);
        for command in std::iter::from_fn(|| sink.try_recv().ok()) {
            if let Command::GetContextOptimizerStats { request_id } = command {
                window
                    .update(cx, |view, window, cx| {
                        runtime_event(
                            view,
                            Event::Graph(json!({
                                "type":"contextOptimizerStatsResult", "requestId":request_id,
                                "result":{"agents":{"claude-code":{"bytes7d":131072,"events7d":15},
                                    "antigravity":{"bytes7d":52428,"events7d":8}}}
                            })),
                            window,
                            cx,
                        );
                    })
                    .unwrap();
            }
        }
        paint(window.into(), cx);
        tap(window, cx, point(px(502.), px(320.)));
        self.save(window, cx, "context-01").await;
        tap(window, cx, point(px(513.), px(100.)));
        self.save(window, cx, "context-02").await;
        window
            .update(cx, |view, window, cx| view.toggle_settings(window, cx))
            .unwrap();
        settle(cx, 2200).await;
        self.respond(window, sink, cx, false, "Opus/deepseek-v4-pro", true);
        context_card_alert(window, cx, true);
        let target = hud_bounds(window, cx)["stats"].center();
        tap(window, cx, target);
        self.save(window, cx, "context-03").await;
        press(window, cx, "escape");
        let target = hud_bounds(window, cx)["handoff"].center();
        dispatch_mouse(
            window,
            cx,
            MouseMoveEvent {
                position: target,
                ..Default::default()
            },
        );
        paint(window.into(), cx);
        settle(cx, 850).await;
        self.save(window, cx, "context-04").await;
        tap(window, cx, target);
        assert!(
            std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(command,
                Command::GetSessionHandoffBrief { session_id, .. } if session_id == CLAUDE
            ))
        );
        context_card_alert(window, cx, false);
        settle(cx, 1400).await;
        self.respond(window, sink, cx, false, "Opus/deepseek-v4-pro", false);
        dispatch_mouse(
            window,
            cx,
            MouseMoveEvent {
                position: point(px(1.), px(1.)),
                ..Default::default()
            },
        );
    }

    fn navigate(
        &self,
        view: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
        mut graph: Value,
        id: &str,
    ) {
        let mut hierarchy = view.hierarchy.clone().unwrap();
        let scene_id = graph["sceneId"].as_str().unwrap();
        hierarchy.scene = view
            .scenes
            .iter()
            .find(|scene| scene.id == scene_id)
            .cloned();
        let node = session_levels::node(session_levels::nodes(&graph), id).unwrap();
        hierarchy.session =
            Some(serde_json::from_value(session(id, node["title"].as_str().unwrap())).unwrap());
        graph["focusedSessionId"] = json!(id);
        runtime_event(
            view,
            Event::Navigation {
                hierarchy,
                scenes: view.scenes.clone(),
                graph,
                geometry: vec![],
            },
            window,
            cx,
        );
    }

    fn confirm_focus(&self, window: Handle, sink: &ProbeReceiver, cx: &mut AsyncApp, id: &str) {
        let focused = std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(command,
            Command::FocusSession { scene_id, session_id } if scene_id == "payments" && session_id == id
        ));
        assert!(focused, "focus routed to {id}");
        window
            .update(cx, |view, window, cx| {
                self.navigate(view, window, cx, view.graph.clone(), id);
            })
            .unwrap();
        self.respond(window, sink, cx, false, "Opus/deepseek-v4-pro", false);
    }

    async fn dsh(&self, window: Handle, sink: &ProbeReceiver, cx: &mut AsyncApp) {
        let original = window.update(cx, |view, window, cx| {
            let original = view.graph.clone();
            let mut graph = original.clone();
            graph["sceneId"] = json!("integration");
            graph["nodes"] = json!([{
                "sessionId":DSH,"currentMode":"dsh","title":self.text("退款审计","Refund audit"),
                "titleSource":"manual","cwd":REPO,"workStatus":"running","terminalActive":true,
                "git":{"state":"ready","branch":"feat/refund-audit","dirty":false},
                "environment":{"kind":"local","state":"ready","path":REPO},
            }]);
            self.navigate(view, window, cx, graph, DSH);
            view.session_frames.insert(DSH.into());
            view.first_frame_content.insert(DSH.into());
            view.recovery.insert(DSH.into(), RecoveryState::Ready);
            self.feed(view, DSH, 3, cx);
            original
        }).unwrap();
        self.respond(window, sink, cx, false, "Opus/deepseek-v4-pro", false);
        self.save(window, cx, "workspace-06").await;
        window
            .update(cx, |view, window, cx| {
                self.navigate(view, window, cx, original, GOOGLE);
                for (index, id) in [CLAUDE, CODEX, GOOGLE].into_iter().enumerate() {
                    view.session_frames.insert(id.into());
                    view.first_frame_content.insert(id.into());
                    view.recovery.insert(id.into(), RecoveryState::Ready);
                    self.feed(view, id, index, cx);
                }
            })
            .unwrap();
    }

    async fn workspace(&self, window: Handle, sink: &ProbeReceiver, cx: &mut AsyncApp) {
        self.save(window, cx, "workspace-01").await;
        window
            .update(cx, |view, window, cx| {
                view.activate_session_card(CODEX, window, cx)
            })
            .unwrap();
        self.confirm_focus(window, sink, cx, CODEX);
        settle(cx, 420).await;
        self.save(window, cx, "workspace-02").await;
        window
            .update(cx, |view, window, cx| {
                view.activate_session_card(CLAUDE, window, cx)
            })
            .unwrap();
        self.confirm_focus(window, sink, cx, CLAUDE);
        let notice = |session_id, event_id, event_type, title, subtitle, body, time| {
            json!({
                "sessionId":session_id, "eventId":event_id,"eventType":event_type,
                "title":title,"subtitle":subtitle,"body":body,"occurredAt":time,
                "sceneId":"payments","taskId":"retries","workspaceId":"acme"
            })
        };
        window
            .update(cx, |view, window, cx| {
                view.notifications.sound_enabled = false;
                let now = notification_now();
                let mut graph = view.graph.clone();
                graph["agentNotifications"] = json!([
                    notice(
                        CODEX,
                        "tests-complete",
                        "stop",
                        "Codex",
                        self.text("回归验证已完成", "Regression checks complete"),
                        self.text(
                            "12 项通过，测试报告已准备好。",
                            "All 12 checks passed. The report is ready."
                        ),
                        now - 60_000
                    ),
                    notice(
                        GOOGLE,
                        "webhook-approval",
                        "permission",
                        "Antigravity",
                        self.text("回调重放需要确认", "Webhook replay needs approval"),
                        self.text(
                            "结账流程已验证，等待确认后重放回调。",
                            "Checkout verified. Approve to replay the webhook."
                        ),
                        now
                    ),
                ]);
                runtime_event(view, Event::Graph(graph), window, cx);
            })
            .unwrap();
        self.save(window, cx, "workspace-03").await;
        window
            .update(cx, |view, _, cx| view.toggle_notifications(cx))
            .unwrap();
        self.save(window, cx, "workspace-04").await;
        tap(window, cx, point(px(150.), px(157.)));
        self.confirm_focus(window, sink, cx, GOOGLE);
        settle(cx, 420).await;
        self.save(window, cx, "workspace-05").await;
        self.dsh(window, sink, cx).await;

        let dag = window.update(cx, |view, window, cx| {
            let mut graph = view.graph.clone();
            let nodes = graph["nodes"].as_array_mut().unwrap();
            let parent = nodes.iter_mut().find(|node| node["sessionId"] == CLAUDE).unwrap();
            parent["activeChildCount"] = json!(1);
            parent["stoppedChildCount"] = json!(1);
            nodes.extend([
                json!({"sessionId":"refund-fork","parentSessionId":CLAUDE,"siblingCreatedSeq":4,
                    "title":self.text("退款边界","Refund edge cases"),"cwd":REPO,"currentMode":"claude-code",
                    "workStatus":"running","latestLines":[self.text("核对部分退款","Checking partial refunds")]}),
                json!({"sessionId":"retry-failure","parentSessionId":CLAUDE,"siblingCreatedSeq":5,
                    "title":self.text("重放失败","Replay failure"),"cwd":REPO,"currentMode":"codex",
                    "workStatus":"error","latestLines":[self.text("回调返回超时","Webhook timed out")]}),
            ]);
            graph["edges"] = json!([
                {"parentSessionId":CLAUDE,"childSessionId":"refund-fork","relationKind":"forked-from"},
                {"parentSessionId":CLAUDE,"childSessionId":"retry-failure","relationKind":"derived-from"},
            ]);
            let layout = DagLayout::new(DagGraph::from_snapshot(&graph).unwrap()).unwrap();
            let scale = 1.0_f64.min(1100. / layout.width).min(620. / layout.height);
            let viewport = GeometryUpdate {
                scene_id:"payments".into(), owner_key:dag_window::viewport_owner_key("payments"),
                layout_revision:0., now:0,
                geometry:json!({"panX":(1200. - layout.width * scale) / 2.,
                    "panY":45. + (692. - layout.height * scale) / 2., "zoom":scale}),
            };
            let dag = DagWindow::open(
                &graph, CLAUDE, Some(&viewport), view.locale,
                view.theme.effective() == AppTheme::Dark, false, window, cx,
            ).unwrap();
            dag.update(cx, |_, window, _| window.resize(size(px(1200.),px(737.)))).unwrap();
            dag
        }).unwrap();
        self.save(AnyWindowHandle::from(dag), cx, "workspace-07")
            .await;
        dag.update(cx, |_, window, _| window.remove_window())
            .unwrap();

        window
            .update(cx, |view, window, cx| view.toggle_board(window, cx))
            .unwrap();
        self.save(window, cx, "workspace-08").await;
        let board = matou_native_desktop::workspace_board::capture_board_bounds(|| {
            paint(window.into(), cx)
        });
        let from = board["board-card:acme:replay"].center();
        let to = board["board-column-tail:acme:active"].center();
        dispatch_mouse(
            window,
            cx,
            MouseMoveEvent {
                position: from,
                ..Default::default()
            },
        );
        paint(window.into(), cx);
        dispatch_mouse(window, cx, mouse_down(from, Modifiers::default()));
        for step in [0.25, 0.5, 0.75, 1.0] {
            dispatch_mouse(
                window,
                cx,
                MouseMoveEvent {
                    position: from + (to - from) * step,
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                },
            );
            paint(window.into(), cx);
        }
        dispatch_mouse(window, cx, mouse_up(to));
        let (request_id, status) = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::SetTaskStatus {
                    request_id,
                    task_id,
                    status,
                    ..
                } if task_id == "replay" => Some((request_id, status)),
                _ => None,
            })
            .expect("drag updates the task status");
        window
            .update(cx, |view, window, cx| {
                let mut tasks = view.tasks.clone();
                tasks
                    .iter_mut()
                    .find(|task| task.id == "replay")
                    .unwrap()
                    .status = status;
                view.apply_tasks(tasks.clone(), cx);
                view.receive_wiring_reply(
                    "taskStatusResult",
                    request_id,
                    &json!({"result":tasks}),
                    window,
                    cx,
                );
            })
            .unwrap();
        self.save(window, cx, "workspace-09").await;
    }

    async fn history(&self, window: Handle, sink: &ProbeReceiver, cx: &mut AsyncApp) {
        window
            .update(cx, |view, window, cx| {
                view.close_board_to_canvas(false, window, cx);
                view.clear_notifications(cx);
                view.activate_session_card(CLAUDE, window, cx);
            })
            .unwrap();
        self.confirm_focus(window, sink, cx, CLAUDE);
        window
            .update(cx, |view, _, cx| {
                let mut terminal = Terminal::new(TerminalOptions {
                    theme: view.theme_palette().terminal,
                    ..TerminalOptions::new(72, 50, 9, 18)
                })
                .unwrap();
                terminal.feed(b"\x1b[?1049h");
                terminal.feed(
                    format!(
                        "\x1b[30m❯\x1b[0m {}\x1b[48;1H\x1b[30m❯\x1b[0m {}",
                        self.text("打开预发布请求日志", "Open the staging request log"),
                        self.text("继续检查退款边界", "Continue with refund edge cases"),
                    )
                    .as_bytes(),
                );
                view.terminals[CLAUDE].update(cx, |surface, cx| {
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
        settle(cx, 420).await;
        for _ in 0..2 {
            paint(window.into(), cx);
            scroll_at(
                window.into(),
                cx,
                point(px(500.), px(390.)),
                core_graphics::event::ScrollEventUnit::LINE,
                3,
                core_graphics::event::CGEventFlags::empty(),
            );
            settle(cx, 180).await;
        }
        let (cwd, provider_session_id) = std::iter::from_fn(|| sink.try_recv().ok())
            .find_map(|command| match command {
                Command::GetProviderSession {
                    cwd,
                    provider_session_id,
                    ..
                } => Some((cwd, provider_session_id)),
                _ => None,
            })
            .expect("terminal scroll requests the normal transcript projection");
        let code = "fn retry_delay(attempt: u32) -> Duration {\n    Duration::from_millis(250 << attempt.min(4))\n}";
        let markdown = format!("{}\n\n{}\n\n\x60\x60\x60rust\n{code}\n\x60\x60\x60\n\n{}",
            self.text("## 支付重试审查", "## Payment retry review"),
            self.text("网络超时会退避重试，重复请求复用同一个幂等键。", "Timeouts retry with backoff. Duplicate requests reuse the same idempotency key."),
            self.text("### 验证结果\n\n- 超时后安全重试\n- 重复请求只扣款一次\n- 签名无效的回调不会重放",
                "### Verification\n\n- Retry safely after a timeout\n- Charge only once for duplicate requests\n- Reject replay when the signature is invalid")
        );
        window.update(cx, |view, window, cx| {
            let mut graph = view.graph.clone();
            graph["providerSessionDetailResults"] = json!([{
                "cwd":cwd,"providerSessionId":provider_session_id,
                "page":{"total":4,"startEventIndex":1,"endEventIndex":5,"hasEarlier":false},
                "events":[
                    {"index":1,"kind":"user","text":self.text("检查支付重试逻辑，并列出回归验证结果。","Review payment retries and summarize the regression checks.")},
                    {"index":2,"kind":"assistant","text":markdown},
                    {"index":3,"kind":"user","text":self.text("发布前还要验证什么？","What should we verify before release?")},
                    {"index":4,"kind":"assistant","text":self.text("### 发布检查\n\n- 在预发布环境重放一条有效的支付回调。\n- 确认重试只生成一条账本记录。\n- 核对部分退款和全额退款的状态。","### Release checks\n\n- Replay a signed webhook in staging.\n- Verify that retries produce just one ledger entry.\n- Check both partial and full refund states.")},
                ]
            }]);
            runtime_event(view, Event::Graph(graph), window, cx);
        }).unwrap();
        window
            .update(cx, |view, _, cx| {
                view.terminals[CLAUDE].read(cx).reveal_readme_transcript();
                cx.notify();
            })
            .unwrap();
        self.save(window, cx, "history").await;
    }
}

pub(super) fn run(output: PathBuf) {
    let locale = match std::env::var("MATOU_README_LOCALE").as_deref() {
        Ok("zh") => InterfaceLocale::Chinese,
        Ok("en") => InterfaceLocale::English,
        _ => panic!("MATOU_README_LOCALE must be zh or en"),
    };
    assert!(
        output.is_dir(),
        "create the isolated capture directory first"
    );
    Application::new().with_assets(NativeAssets).run(move |cx| {
        cx.spawn(async move |cx| {
            let demo = Demo { locale, output };
            let (window, sink) = demo.open(cx);
            demo.respond(window, &sink, cx, false, "Opus/deepseek-v4-flash", false);
            demo.account(window, &sink, cx).await;
            demo.dispatch(window, cx).await;
            provider(&demo, window, &sink, cx).await;
            demo.context(window, &sink, cx).await;
            demo.workspace(window, &sink, cx).await;
            demo.history(window, &sink, cx).await;
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
}
