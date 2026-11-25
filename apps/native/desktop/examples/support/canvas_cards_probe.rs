use super::*;

const MISSING: &str = "missing-terminal";
const RECOVERING: &str = "restoring-terminal";
const FAILED: &str = "failed-terminal";
const MANUAL_TITLE: &str = "Manually named fixture shell with a long title";

fn install_case(
    view: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
    case: &str,
) {
    super::install_fixture(view, window, cx);
    view.locale = InterfaceLocale::English;
    view.theme.set_preference(ThemePreference::Light);
    let mut graph = json!({"sceneId":"main", "workspaceSessionCount":0, "nodes":[]});
    if case == "workspace" {
        view.apply_tasks(Vec::new(), cx);
        let hierarchy = view.hierarchy.as_mut().unwrap();
        hierarchy.task = None;
        hierarchy.scene = None;
        view.scenes.clear();
    } else if case != "canvas" {
        let nodes = graph["nodes"].as_array_mut().unwrap();
        for (index, id) in [
            A, FAILED, MISSING, "shell-d", RECOVERING, "shell-f", "shell-g", "shell-h",
        ]
        .into_iter()
        .enumerate()
        {
            nodes.push(json!({
                "sessionId":id, "currentMode":"shell", "title": if id == A { MANUAL_TITLE } else { id },
                "titleSource": if id == A { "manual" } else { "default" },
                "cwd":"/probe/elsewhere", "workStatus":"idle",
                "git":{"state":"ready","branch":format!("branch-{index}"),"dirty":index == 0},
                "sharedWorkingDirectory":id == A || id == MISSING,
                "siblingCreatedSeq": index, "latestLines":["Fixtures only"]
            }));
        }
        if case == "missing" {
            nodes.retain(|node| node["sessionId"] == MISSING);
            view.focused = Some(MISSING.into());
        }
        graph["workspaceSessionCount"] = json!(graph["nodes"].as_array().unwrap().len());
        let scene = view.scenes.first_mut().unwrap();
        scene.name = "Shell · Fixture canvas".into();
    }
    view.apply(
        BridgeEvent::Runtime {
            generation: 1,
            event: Event::Graph(graph),
        },
        window,
        cx,
    );
    if let Some(nodes) = view.graph["nodes"].as_array() {
        for id in nodes.iter().filter_map(|node| node["sessionId"].as_str()) {
            view.session_frames.insert(id.to_owned());
            view.recovery.insert(id.to_owned(), RecoveryState::Ready);
        }
    }
    view.terminals.remove(MISSING);
    if case == "multi" {
        for (id, state) in [
            (
                FAILED,
                RecoveryState::Failed("Fixture provider exited".into()),
            ),
            (RECOVERING, RecoveryState::Restoring),
        ] {
            view.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Recovery {
                        session_id: id.into(),
                        state,
                        history_gap: None,
                    },
                },
                window,
                cx,
            );
        }
        view.focused = Some(A.into());
    }
    view.focus_follow_until = None;
    view.restore_geometry = false;
    view.sync_terminal_input(window, cx);
    if let Some(terminal) = view.focused.as_ref().and_then(|id| view.terminals.get(id)) {
        window.focus(&terminal.read(cx).focus);
    }
    cx.notify();
}

fn check_sidebar_pointer(cx: &mut AsyncApp) {
    if reduced_motion() {
        println!("Sidebar pointer probe skipped: system Reduce Motion is enabled");
        return;
    }
    let (window, _) = open_probe_at(cx, 900.);
    draw(window.into(), cx);
    for (position, expected) in [
        (point(px(120.), px(300.)), true),
        (point(px(120.), px(550.)), true),
        (point(px(400.), px(550.)), false),
    ] {
        dispatch_mouse(
            window,
            cx,
            MouseMoveEvent {
                position,
                ..Default::default()
            },
        );
        window
            .update(cx, |view, _, _| {
                assert_eq!(
                    view.sidebar_glass_pointer.get(),
                    expected.then_some(position)
                );
            })
            .unwrap();
        draw(window.into(), cx);
    }
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    println!(
        "Sidebar pointer: GPUI move dispatch followed both sidebar positions and reset on leaving"
    );
}

fn check_layout(cx: &mut AsyncApp, width: f32) {
    let (window, sink) = open_probe_at(cx, width);
    for (case, message) in [
        ("workspace", "workspace-no-tasks"),
        ("canvas", "canvas-no-sessions"),
        ("missing", "terminal-unavailable"),
    ] {
        window
            .update(cx, |view, window, cx| install_case(view, window, cx, case))
            .unwrap();
        let painted = draw(window.into(), cx);
        assert!(painted.contains_key(message), "{case}: {painted:?}");
        if case == "missing" {
            assert!(painted.contains_key(&format!("session-header-{MISSING}")));
            assert!(painted.contains_key(&format!("remove-session-{MISSING}")));
            window
                .update(cx, |view, _, _| {
                    assert!(view.wiring_card_menu(MISSING).can_open());
                    assert!(!view.recovery_water.contains_key(MISSING));
                })
                .unwrap();
        }
    }

    window
        .update(cx, |view, window, cx| {
            install_case(view, window, cx, "multi")
        })
        .unwrap();
    let painted = draw(window.into(), cx);
    window.update(cx, |view, window, cx| {
        assert_eq!(view.scene_title(&view.scenes[0]), "Shell · Fixture canvas");
        assert_eq!(view.compact_recovery_status(FAILED, view.graph_node(FAILED)),
            Some(("Recovery failed: Fixture provider exited".into(), true)));
        assert_eq!(view.compact_recovery_status(RECOVERING, view.graph_node(RECOVERING)),
            Some(("Restoring".into(), false)));
        let layout = view.carousel_viewport_width(window);
        let range = carousel_terminal_range(&view.card_geometry, 0., layout);
        let mut mounted: Vec<_> = view.carousel_mounted_terminals.iter().cloned().collect();
        mounted.sort();
        assert!(mounted.len() < view.terminals.len());
        assert!(mounted.contains(&A.to_owned()));
        assert!(mounted.iter().all(|id| {
            range.contains(&view.order.iter().position(|card| card == id).unwrap())
                || id == RECOVERING
        }));
        let menu = |id| view.wiring_card_menu(id).items.into_iter()
            .find(|item| item.action == CardMenuAction::RestoreAutomaticTitle).unwrap();
        assert!(menu(A).disabled_reason.is_none());
        assert!(menu("shell-d").disabled_reason.is_some());
        let card_width = view.card_geometry.iter().find(|(id, _, _)| id == A).unwrap().2;
        assert_eq!(painted.contains_key(&format!("card-git-{A}")), card_width > 500.);
        assert_eq!(painted.contains_key(&format!("card-shared-{A}")), card_width > 420.);
        println!("{width}pt: three empty states, card width={card_width:.1}, Git={}, shared={}, terminals={mounted:?}; {} retained surfaces",
            card_width > 500., card_width > 420., view.terminals.len());
        assert_eq!(window.viewport_size().width, px(width));
        assert!(view.terminals[A].read(cx).focus.is_focused(window));
    }).unwrap();
    if width == 1200. {
        card_action(window, cx, A, CardMenuAction::RestoreAutomaticTitle);
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command| {
            matches!(command, Command::SetSessionTitle { session_id, title: None } if session_id == A)
        }));
        println!(
            "Restore automatic title: enabled only for the manual card; menu emits the existing command"
        );
        check_team_hud(window, &sink, cx);
    }
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

fn check_team_hud(window: Handle, sink: &crate::runtime_bridge::ProbeReceiver, cx: &mut AsyncApp) {
    window
        .update(cx, |view, window, cx| {
            let mut graph = view.graph.clone();
            graph["nodes"][0]["currentMode"] = json!("claude-code");
            view.apply(
                BridgeEvent::Runtime {
                    generation: 1,
                    event: Event::Graph(graph),
                },
                window,
                cx,
            );
            view.refresh_live_wiring(cx);
            window.focus(&view.terminals[A].read(cx).focus);
        })
        .unwrap();
    let request_id = std::iter::from_fn(|| sink.try_recv().ok())
        .filter_map(|command| match command {
            Command::GetAgentHud {
                session_id,
                request_id,
            } if session_id == A => Some(request_id),
            _ => None,
        })
        .last()
        .expect("existing HUD request");
    let member = json!({"leadSessionId":A,"teammateId":"reviewer@fixture","name":"Reviewer",
        "workStatus":"running","latestLines":["Inspecting fixture output"]});
    window
        .update(cx, |view, window, cx| {
            let reply = json!({"result":{"teammates":[member.clone(), member]}});
            view.receive_wiring_reply("agentHudResult", request_id, &reply, window, cx);
            view.receive_wiring_reply("agentHudResult", request_id, &reply, window, cx);
            let cards = view.canvas_team_members();
            assert_eq!(cards.len(), 1);
            assert!(!view.terminals.contains_key(&cards[0].id));
            assert!(view.terminals[A].read(cx).focus.is_focused(window));
        })
        .unwrap();
    let painted = draw(window.into(), cx);
    assert!(painted.keys().any(|key| key.contains("reviewer@fixture")));
    println!(
        "Existing HUD reply: one read-only member card for repeated observations, no terminal or focus change"
    );
}

fn open_visible(cx: &mut App, width: f32, case: &str) {
    let (commands, _sink) = crate::runtime_bridge::probe_channel();
    let (_, events) = async_channel::unbounded();
    let bounds = Bounds::centered(None, size(px(width), px(690.)), cx);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Matou Canvas Probe — {case} {width}").into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Workspace::new(commands, events, window, cx)),
        )
        .unwrap();
    handle
        .update(cx, |view, window, cx| install_case(view, window, cx, case))
        .unwrap();
}

pub(super) fn run(mode: String) {
    let fixture_root = (mode == "test").then(live_wiring::prepare);
    Application::new().with_assets(NativeAssets).run(move |cx| {
        if let Some(root) = fixture_root {
            cx.spawn(async move |cx| {
                check_sidebar_pointer(cx);
                for width in [700., 720., 900., 1200.] {
                    check_layout(cx, width);
                }
                verify_carousel_narrow(cx);
                verify_carousel_focus(cx).await;
                live_wiring::verify_canvas_mount_output(cx, root).await;
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        } else {
            let (case, width) = mode.split_once(':').expect("expected case:width");
            open_visible(cx, width.parse().unwrap(), case);
            cx.activate(true);
        }
    });
}
