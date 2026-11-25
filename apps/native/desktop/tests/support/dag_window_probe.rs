//! Hidden macOS probe for the actual Workspace and its Runtime graph feed.

#![allow(deprecated, unexpected_cfgs)]

use super::*;
use cocoa::{
    appkit::{NSEvent, NSEventModifierFlags, NSEventType},
    base::{id, nil},
    foundation::{NSInteger, NSPoint},
};
use matou_native_desktop::dag_canvas::{DagCanvas, DagTransform};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::json;

#[derive(Default)]
pub(super) struct OpenProbe {
    pub(super) fail_next_open: bool,
}

impl Workspace {
    pub fn run_dag_window_probe(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.dag_probe = Some(OpenProbe::default());
        let main = window.window_handle().downcast::<Workspace>().unwrap();
        cx.spawn(async move |_, cx| {
            match std::env::var("MATOU_SPLIT_DAG_I18N_PROBE").as_deref() {
                Ok("record") => record_split(main, cx).await,
                Ok("restore") => restore_split_and_refresh_dag(main, cx).await,
                _ => run(main, cx).await,
            }
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    }
}

async fn wait_until(cx: &mut AsyncApp, condition: impl Fn(&mut AsyncApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition(cx) {
        assert!(Instant::now() < deadline, "Probe condition timed out");
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

fn fixture_path(main: WindowHandle<Workspace>, cx: &AsyncApp) -> PathBuf {
    main.read_with(cx, |view, _| {
        view.data_root
            .as_ref()
            .unwrap()
            .join("split-layout-expected.json")
    })
    .unwrap()
}

fn render_bounds(
    handle: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> (Size<Pixels>, HashMap<String, Bounds<Pixels>>) {
    handle
        .update(cx, |_, window, cx| {
            window.refresh();
            let bounds = workspace_wiring::capture_wiring_bounds(|| window.draw(cx).clear());
            (window.viewport_size(), bounds)
        })
        .unwrap()
}

async fn resize_probe(handle: AnyWindowHandle, width: f32, cx: &mut AsyncApp) {
    handle
        .update(cx, |_, window, _| window.resize(size(px(width), px(650.))))
        .unwrap();
    wait_until(cx, |cx| {
        handle
            .update(cx, |_, window, _| window.viewport_size().width == px(width))
            .unwrap()
    })
    .await;
}

async fn capture_probe(handle: AnyWindowHandle, name: &str, cx: &mut AsyncApp) {
    let Some(dir) = std::env::var_os("MATOU_SPLIT_DAG_I18N_CAPTURE") else {
        return;
    };
    let (view, native) = handle
        .update(cx, |_, window, _| {
            let view = appkit_view(window);
            let native: id = unsafe { msg_send![view, window] };
            (view, native)
        })
        .unwrap();
    let window_id: NSInteger = unsafe {
        let _: () = msg_send![native, orderFrontRegardless];
        let _: () = msg_send![view, displayLayer: nil];
        msg_send![native, windowNumber]
    };
    eprintln!("[split-dag-capture] {name} {window_id}");
    let png = PathBuf::from(dir).join(format!("{name}.png"));
    wait_until(cx, |_| png.is_file()).await;
    unsafe {
        let _: () = msg_send![native, orderOut: nil];
    }
}

fn probe_drag_divider(main: WindowHandle<Workspace>, index: usize, ratio: f64, cx: &mut AsyncApp) {
    let (size, bounds) = render_bounds(main.into(), cx);
    let stage = bounds["split-canvas"];
    let (direction, rect, position) = main
        .read_with(cx, |view, _| {
            let tree = view.active_split_tree().unwrap();
            let layout = tree.layout(SplitRect {
                x: f64::from(stage.left()),
                y: f64::from(stage.top() + px(1.)),
                width: f64::from(stage.size.width),
                height: f64::from(stage.size.height - px(1.)),
            });
            let divider = &layout.dividers[index];
            (divider.direction, divider.parent_bounds, divider.position)
        })
        .unwrap();
    let width = f64::from(size.width);
    let (from, to) = match direction {
        SplitDirection::Horizontal => (
            (width - position, rect.y + rect.height * 0.25),
            (
                width - (rect.x + rect.width * ratio),
                rect.y + rect.height * 0.25,
            ),
        ),
        SplitDirection::Vertical => (
            (width - (rect.x + rect.width * 0.5), position),
            (
                width - (rect.x + rect.width * 0.5),
                rect.y + rect.height * ratio,
            ),
        ),
    };
    matou_native_desktop::split_tree::with_split_drag_button_for_probe(Some(1), || {
        pointer(main.into(), cx, from, Some(to));
    });
}

async fn wait_for_shell(main: WindowHandle<Workspace>, cx: &mut AsyncApp) -> String {
    let id = wait_graph(main, cx, |view| {
        let id = view.active_level_focus()?;
        (view.domain_ready && view.terminals.contains_key(id)).then(|| id.to_owned())
    })
    .await;
    wait_until(cx, |cx| {
        let native = AnyWindowHandle::from(main)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
                appkit_view(window)
            })
            .unwrap();
        // Hidden windows do not receive a display link from AppKit.
        unsafe {
            let _: () = msg_send![native, displayLayer: nil];
        }
        main.read_with(cx, |view, _| !view.recovery_blocked(&id))
            .unwrap()
    })
    .await;
    id
}

async fn record_split(main: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    let mut source = wait_for_shell(main, cx).await;
    let scene = main
        .read_with(cx, |view, _| view.active_scene_id().unwrap().to_owned())
        .unwrap();
    for (count, direction) in [
        (2, SplitDirection::Horizontal),
        (3, SplitDirection::Vertical),
    ] {
        main.update(cx, |view, _, cx| view.split_pane(&source, direction, cx))
            .unwrap();
        wait_graph(main, cx, |view| {
            (view.active_split_tree()?.ordered_sessions().len() == count).then_some(())
        })
        .await;
        source = wait_for_shell(main, cx).await;
        eprintln!("[split-dag-i18n-probe] created splits panes={count}");
    }
    probe_drag_divider(main, 0, 0.63, cx);
    probe_drag_divider(main, 1, 0.38, cx);
    let expected = wait_graph(main, cx, |view| {
        let owner = view.carousel_owner.as_ref()?;
        let tree = view.active_split_tree()?;
        let saved = view.geometry.get(owner)?;
        let layout = tree.canvas_layout();
        let ratio = |index: usize| {
            layout
                .get(index)
                .and_then(|node: &Value| node["ratio"].as_f64())
        };
        (ratio(0).is_some_and(|r| (r - 0.63).abs() < 0.015)
            && ratio(2).is_some_and(|r| (r - 0.38).abs() < 0.015)
            && saved.geometry["splitLayout"] == layout
            && !view.geometry_dirty.contains(owner))
        .then_some(layout)
    })
    .await;
    assert_eq!(expected[0]["direction"], "horizontal");
    assert_eq!(expected[2]["direction"], "vertical");
    eprintln!("[split-dag-i18n-probe] split-drag-saved {expected}");
    std::fs::write(
        fixture_path(main, cx),
        serde_json::to_vec(&expected).unwrap(),
    )
    .unwrap();

    main.update(cx, |view, _, cx| view.create_canvas(cx))
        .unwrap();
    wait_graph(main, cx, |view| {
        (view.active_scene_id().is_some_and(|id| id != scene) && view.active_split_tree().is_none())
            .then_some(())
    })
    .await;
    navigate(main, cx, Command::SelectScene { scene_id: scene });
    wait_graph(main, cx, |view| {
        (view.active_split_tree()?.canvas_layout() == expected).then_some(())
    })
    .await;
    eprintln!("[split-dag-i18n-probe] split-restored-on-canvas-return");
}

async fn restore_split_and_refresh_dag(main: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    let expected: Value =
        serde_json::from_slice(&std::fs::read(fixture_path(main, cx)).unwrap()).unwrap();
    let (scene, panes) = wait_graph(main, cx, |view| {
        let tree = view.active_split_tree()?;
        (view.domain_ready && tree.canvas_layout() == expected).then(|| {
            (
                view.active_scene_id().unwrap().to_owned(),
                tree.ordered_sessions()
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            )
        })
    })
    .await;
    assert_eq!(panes.len(), 3);
    eprintln!("[split-dag-i18n-probe] split-restored-on-process-restart");
    for width in [720., 900., 1200.] {
        resize_probe(main.into(), width, cx).await;
        let (_, bounds) = render_bounds(main.into(), cx);
        let stage = bounds["split-canvas"];
        let [left, top, bottom] =
            std::array::from_fn(|index| bounds[&format!("split-pane-{}", panes[index])]);
        assert!(
            [left, top, bottom]
                .iter()
                .all(|pane| pane.is_contained_within(&stage))
        );
        let horizontal = f64::from(left.size.width) / f64::from(stage.size.width);
        let vertical = f64::from(top.size.height) / f64::from(top.size.height + bottom.size.height);
        assert!((horizontal - expected[0]["ratio"].as_f64().unwrap()).abs() < 0.01);
        assert!((vertical - expected[2]["ratio"].as_f64().unwrap()).abs() < 0.01);
        eprintln!(
            "[split-dag-i18n-probe] split-viewport width={width} ratios={horizontal:.3}/{vertical:.3}"
        );
        capture_probe(main.into(), &format!("split-{width}"), cx).await;
    }

    main.update(cx, |view, window, cx| view.open_dag_window(window, cx))
        .unwrap();
    let child = child_window(main, cx);
    frame(child, cx);
    let good = child
        .read_with(cx, |view, _| view.probe_canvas().0.clone())
        .unwrap();
    let transform = read_canvas(child, cx, |canvas| canvas.transform);
    // The child stays scoped to its first scene even after the main window switches.
    main.update(cx, |view, _, cx| view.create_canvas(cx))
        .unwrap();
    let other = wait_graph(main, cx, |view| {
        view.active_scene_id()
            .filter(|id| *id != scene)
            .map(str::to_owned)
    })
    .await;
    let mut invalid = good.clone();
    invalid["edges"] = json!("fixture invalid");
    let inject_failure = |cx: &mut AsyncApp| {
        child
            .update(cx, |view, _, cx| {
                view.refresh(&invalid, cx);
                view.refresh(&invalid, cx);
                assert_eq!(view.probe_canvas().0, &good);
                assert_eq!(
                    view.probe_refresh_label(),
                    Some("Couldn't refresh the graph")
                );
            })
            .unwrap();
    };
    inject_failure(cx);
    for width in [720., 900., 1200.] {
        resize_probe(child.into(), width, cx).await;
        let (size, bounds) = render_bounds(child.into(), cx);
        let banner = bounds["dag-refresh-banner"];
        let retry = bounds["dag-refresh-retry"];
        let dismiss = bounds["dag-refresh-dismiss"];
        assert!(banner.is_contained_within(&Bounds::new(point(px(0.), px(0.)), size)));
        assert!(retry.is_contained_within(&banner) && dismiss.is_contained_within(&banner));
        assert!(retry.right() < dismiss.left());
        eprintln!(
            "[split-dag-i18n-probe] dag-viewport width={width} banner={banner:?} controls={retry:?},{dismiss:?}"
        );
        capture_probe(child.into(), &format!("dag-failed-{width}"), cx).await;
        if width == 720. {
            search(child, cx, "shell");
            assert!(read_canvas(child, cx, |canvas| !canvas
                .search
                .results
                .is_empty()));
            key(child.into(), cx, "cmd-a");
            key(child.into(), cx, "backspace");
            eprintln!(
                "[split-dag-i18n-probe] dag-search-width=720 query returned results under failure banner"
            );
        }
    }
    let click_action = |id: &str, cx: &mut AsyncApp| {
        let (size, bounds) = render_bounds(child.into(), cx);
        let center = bounds[id].center();
        click(
            child.into(),
            cx,
            f64::from(size.width - center.x),
            f64::from(center.y),
        );
    };
    child
        .update(cx, |view, window, cx| {
            view.set_locale(InterfaceLocale::Chinese, window, cx);
            assert_eq!(view.probe_refresh_label(), Some("图刷新失败"));
            view.set_locale(InterfaceLocale::English, window, cx);
        })
        .unwrap();
    click_action("dag-refresh-dismiss", cx);
    assert!(
        child
            .read_with(cx, |view, _| view.probe_refresh_label().is_none()
                && view.probe_canvas().0 == &good)
            .unwrap()
    );
    child
        .update(cx, |view, _, cx| view.refresh(&good, cx))
        .unwrap();
    inject_failure(cx);

    click_action("dag-refresh-retry", cx);
    assert!(
        child
            .read_with(cx, |view, _| view.probe_retry_in_flight())
            .unwrap()
    );
    assert!(
        child
            .update(cx, |view, _, cx| view.start_refresh(cx).is_none())
            .unwrap()
    );
    wait_until(cx, |cx| {
        child
            .read_with(cx, |view, _| view.probe_refresh_label().is_none())
            .unwrap()
    })
    .await;
    assert_eq!(read_canvas(child, cx, |canvas| canvas.transform), transform);
    assert_eq!(
        main.read_with(cx, |view, _| view.active_scene_id().unwrap().to_owned())
            .unwrap(),
        other
    );
    eprintln!(
        "[split-dag-i18n-probe] dag-repeat-dismiss-retry last-graph-preserved scene-unchanged pending-duplicate-blocked"
    );

    let generation = main
        .read_with(cx, |view, _| view.runtime_generation)
        .unwrap();
    crash_fixture_runtime(main, cx);
    wait_until(cx, |cx| {
        child
            .read_with(cx, |view, _| {
                view.probe_refresh_label() == Some("Reconnecting…")
            })
            .unwrap()
    })
    .await;
    eprintln!("[split-dag-i18n-probe] dag-reconnecting observed");
    capture_probe(child.into(), "dag-reconnecting", cx).await;
    wait_graph(main, cx, |view| {
        (view.domain_ready && view.runtime_generation > generation).then_some(())
    })
    .await;
    wait_until(cx, |cx| {
        child
            .read_with(cx, |view, _| {
                view.probe_refresh_label().is_none() && view.probe_canvas().0["sceneId"] == scene
            })
            .unwrap()
    })
    .await;
    key(child.into(), cx, "escape");
    assert!(
        !cx.update(|cx| cx.windows().contains(&child.into()))
            .unwrap()
    );
    main.update(cx, |view, window, cx| view.open_dag_window(window, cx))
        .unwrap();
    let reopened = child_window(main, cx);
    assert_ne!(reopened.window_id(), child.window_id());
    assert!(
        reopened
            .read_with(cx, |view, _| view.probe_refresh_label().is_none())
            .unwrap()
    );
    key(reopened.into(), cx, "escape");
    eprintln!(
        "[split-dag-i18n-probe] dag-runtime-recovered background-scene-auto-refreshed clean-reopen"
    );

    let shell = wait_for_shell(main, cx).await;
    main.update(cx, |view, window, cx| {
        view.apply(
            BridgeEvent::Runtime {
                generation: view.runtime_generation,
                event: Event::Recovery {
                    session_id: shell.clone(),
                    state: RecoveryState::StartFailed(String::new()),
                    history_gap: None,
                },
            },
            window,
            cx,
        );
        let node = view.graph_node(&shell).unwrap();
        let card = view.start_failure_card(&shell, node).unwrap();
        assert_eq!(
            (card.title, card.reason.as_ref(), card.retry, card.remove),
            (
                "Could not start session",
                "The terminal process could not start",
                "Retry launch",
                "Remove failed session",
            )
        );
        view.locale = InterfaceLocale::Chinese;
        let card = view
            .start_failure_card(&shell, view.graph_node(&shell).unwrap())
            .unwrap();
        assert_eq!(
            (card.title, card.reason.as_ref(), card.retry, card.remove),
            (
                "会话启动失败",
                "终端进程未能启动",
                "重试启动",
                "移除失败会话",
            )
        );
        view.locale = InterfaceLocale::English;
        cx.notify();
    })
    .unwrap();
    for width in [720., 900., 1200.] {
        resize_probe(main.into(), width, cx).await;
        let (size, bounds) = render_bounds(main.into(), cx);
        let failure = bounds["session-start-failure"];
        assert!(failure.is_contained_within(&Bounds::new(point(px(0.), px(0.)), size)));
        eprintln!("[split-dag-i18n-probe] english-start-failure width={width} banner={failure:?}");
        capture_probe(main.into(), &format!("english-start-failed-{width}"), cx).await;
    }
    let order = main
        .update(cx, |view, _, cx| {
            cx.notify();
            std::mem::take(&mut view.order)
        })
        .unwrap();
    assert!(
        render_bounds(main.into(), cx)
            .1
            .contains_key("canvas-no-sessions")
    );
    capture_probe(main.into(), "english-empty-canvas", cx).await;
    main.update(cx, |view, _, cx| {
        view.order = order;
        cx.notify();
    })
    .unwrap();
    eprintln!("[split-dag-i18n-probe] english-empty-canvas rendered");

    navigate(
        main,
        cx,
        Command::SelectScene {
            scene_id: scene.clone(),
        },
    );
    wait_graph(main, cx, |view| {
        (view.active_split_tree()?.canvas_layout() == expected).then_some(())
    })
    .await;
    navigate(
        main,
        cx,
        Command::RemoveSessionBranch {
            scene_id: scene.clone(),
            session_id: panes[2].clone(),
            scope: Default::default(),
        },
    );
    wait_graph(main, cx, |view| {
        let tree = view.active_split_tree()?;
        (tree.ordered_sessions() == [panes[0].as_str(), panes[1].as_str()]
            && tree.canvas_layout()[0]["ratio"] == expected[0]["ratio"])
            .then_some(())
    })
    .await;
    navigate(
        main,
        cx,
        Command::RemoveSessionBranch {
            scene_id: scene,
            session_id: panes[1].clone(),
            scope: Default::default(),
        },
    );
    wait_graph(main, cx, |view| {
        (view.order == [panes[0].clone()] && view.active_split_tree().is_none()).then_some(())
    })
    .await;
    eprintln!(
        "[split-dag-i18n-probe] split-pruned-missing nested-branch-collapsed remaining-ratio-preserved single-card-fallback"
    );
}

fn crash_fixture_runtime(main: WindowHandle<Workspace>, cx: &AsyncApp) {
    let root = main
        .read_with(cx, |view, _| view.data_root.clone().unwrap())
        .unwrap();
    assert!(root.join("dag-window-probe.fixture").is_file());
    let output = std::process::Command::new("/bin/ps")
        .args(["-ax", "-o", "pid=,ppid=,command="])
        .output()
        .unwrap();
    assert!(output.status.success());
    let parent = std::process::id().to_string();
    let listing = String::from_utf8(output.stdout).unwrap();
    let runtimes: Vec<_> = listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?;
            let ppid = fields.next()?;
            let binary = fields.next()?;
            (ppid == parent
                && binary.ends_with("/matou-native-runtime")
                && line.contains(root.to_str().unwrap()))
            .then_some(pid)
        })
        .collect();
    assert_eq!(
        runtimes.len(),
        1,
        "expected exactly one isolated runtime child"
    );
    assert!(
        std::process::Command::new("/bin/kill")
            .args(["-KILL", runtimes[0]])
            .status()
            .unwrap()
            .success()
    );
}

async fn run(main: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    let scene_a = wait_graph(main, cx, |view| {
        (view.domain_ready && session_levels::nodes(&view.graph).len() == 1)
            .then(|| view.active_scene_id().unwrap().to_owned())
    })
    .await;
    let root = main
        .read_with(cx, |view, _| {
            view.graph["nodes"][0]["sessionId"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .unwrap();
    let branch = || Command::CreateShellSibling {
        scene_id: scene_a.clone(),
        source_session_id: root.clone(),
        parent_session_id: Some(root.clone()),
    };
    navigate(main, cx, branch());
    wait_graph(main, cx, |view| {
        (node_count(&view.graph) == 2).then_some(())
    })
    .await;

    click_graph(main, cx);
    let child = child_window(main, cx);
    assert_graph(main, child, cx, &scene_a, Some((&root, 1)));
    assert_main_focus(main, cx, "graph");
    report(
        cx,
        "open-icon",
        main,
        Some(child),
        "main-focus=graph child-focus=content nodes=2 edges=1",
    );

    click_graph(main, cx);
    assert_eq!(child_window(main, cx).window_id(), child.window_id());
    assert_graph(main, child, cx, &scene_a, Some((&root, 1)));
    assert_main_focus(main, cx, "graph");
    report(
        cx,
        "repeat",
        main,
        Some(child),
        "same-child main-focus=graph child-focus=content nodes=2 edges=1",
    );

    navigate(main, cx, branch());
    wait_graph(main, cx, |view| {
        (node_count(&view.graph) == 3).then_some(())
    })
    .await;
    const LIVE_TITLE: &str = "DAG probe live root";
    navigate(
        main,
        cx,
        Command::SetSessionTitle {
            session_id: root.clone(),
            title: Some(LIVE_TITLE.to_owned()),
        },
    );
    wait_graph(main, cx, |view| {
        session_levels::node(session_levels::nodes(&view.graph), &root)
            .is_some_and(|node| node["title"] == LIVE_TITLE)
            .then_some(())
    })
    .await;
    assert_graph(main, child, cx, &scene_a, Some((&root, 2)));
    assert_main_focus(main, cx, "graph");
    report(
        cx,
        "live-update",
        main,
        Some(child),
        "scene=A updated-root-title main-focus=graph child-focus=content nodes=3 edges=2",
    );

    main.update(cx, |view, _, cx| view.create_canvas(cx))
        .unwrap();
    let scene_b = wait_graph(main, cx, |view| {
        let scene = view.active_scene_id()?;
        (scene != scene_a && view.graph["sceneId"] == scene && node_count(&view.graph) == 1)
            .then(|| scene.to_owned())
    })
    .await;
    let saved_b = DagTransform {
        x: 31.,
        y: -27.,
        scale: 1.25,
    };
    let owner_b = dag_window::viewport_owner_key(&scene_b);
    // Bypass the view cache so the next navigation loads the saved transform
    // through the Runtime's scene geometry projection.
    navigate(
        main,
        cx,
        Command::PutGeometry {
            sequence: 0,
            update: GeometryUpdate {
                scene_id: scene_b.clone(),
                owner_key: owner_b.clone(),
                layout_revision: 1.,
                geometry: json!({"panX": saved_b.x, "panY": saved_b.y, "zoom": saved_b.scale}),
                now: 1,
            },
        },
    );
    assert_graph(main, child, cx, &scene_a, Some((&root, 2)));
    assert_main_focus(main, cx, "graph");
    report(
        cx,
        "main-switch",
        main,
        Some(child),
        "main=B child=A main-focus=graph child-focus=content nodes=3 edges=2",
    );

    const RENAMED: &str = "DAG probe updated root";
    navigate(
        main,
        cx,
        Command::SetSessionTitle {
            session_id: root.clone(),
            title: Some(RENAMED.to_owned()),
        },
    );
    navigate(
        main,
        cx,
        Command::SelectScene {
            scene_id: scene_a.clone(),
        },
    );
    wait_graph(main, cx, |view| {
        (view.active_scene_id() == Some(scene_a.as_str())
            && session_levels::node(session_levels::nodes(&view.graph), &root)
                .is_some_and(|node| node["title"] == RENAMED))
        .then_some(())
    })
    .await;
    assert_graph(main, child, cx, &scene_a, Some((&root, 2)));
    assert_main_focus(main, cx, "graph");
    report(
        cx,
        "main-switch-back",
        main,
        Some(child),
        "main=A child=A updated-root-title main-focus=graph child-focus=content nodes=3 edges=2",
    );

    navigate(
        main,
        cx,
        Command::SelectScene {
            scene_id: scene_b.clone(),
        },
    );
    wait_graph(main, cx, |view| {
        (view.active_scene_id() == Some(scene_b.as_str()) && node_count(&view.graph) == 1)
            .then_some(())
    })
    .await;
    click_graph(main, cx);
    assert_eq!(child_window(main, cx).window_id(), child.window_id());
    assert_graph(main, child, cx, &scene_b, None);
    assert_eq!(read_canvas(child, cx, |canvas| canvas.transform), saved_b);
    assert_main_focus(main, cx, "graph");
    report(
        cx,
        "retarget",
        main,
        Some(child),
        "same-child main=B child=B main-focus=graph child-focus=content nodes=1 edges=0",
    );

    key(child.into(), cx, "ctrl-=");
    let changed_b = read_canvas(child, cx, |canvas| canvas.transform);
    assert_ne!(changed_b, saved_b);
    close_system(child, cx);
    assert_closed(main, child, cx);
    wait_graph(main, cx, |view| {
        view.geometry
            .get(&owner_b)
            .filter(|row| {
                row.geometry
                    == json!({"panX": changed_b.x, "panY": changed_b.y, "zoom": changed_b.scale})
                    && !view.geometry_dirty.contains(&owner_b)
            })
            .map(|_| ())
    })
    .await;
    report(
        cx,
        "close-system",
        main,
        None,
        "AppKit performClose, main-focus=graph",
    );

    click_graph(main, cx);
    let reopened = child_window(main, cx);
    assert_ne!(reopened.window_id(), child.window_id());
    assert_graph(main, reopened, cx, &scene_b, None);
    assert_eq!(
        read_canvas(reopened, cx, |canvas| canvas.transform),
        changed_b
    );
    assert_main_focus(main, cx, "graph");
    report(
        cx,
        "reopen",
        main,
        Some(reopened),
        "new-child main-focus=graph child-focus=content nodes=1 edges=0",
    );
    key(reopened.into(), cx, "escape");
    assert_closed(main, reopened, cx);
    report(
        cx,
        "close-escape",
        main,
        None,
        "child Escape, main-focus=graph",
    );

    click_graph(main, cx);
    let child = child_window(main, cx);
    click(child.into(), cx, 30., 28.);
    assert_closed(main, child, cx);
    report(
        cx,
        "close-button",
        main,
        None,
        "child Close button click, main-focus=graph",
    );

    click_graph(main, cx);
    let child = child_window(main, cx);
    key(child.into(), cx, "ctrl-f");
    child
        .update(cx, |view, window, cx| {
            assert!(view.probe_canvas().1.read(cx).search_focused(window, cx));
        })
        .unwrap();
    assert!(!dispatch_key(child.into(), cx, "cmd-shift-w"));
    report(
        cx,
        "keep-open-cmd-shift-w",
        main,
        Some(child),
        "Cmd+Shift+W unhandled with DAG search focused",
    );
    key(child.into(), cx, "cmd-w");
    assert_closed(main, child, cx);
    report(
        cx,
        "close-cmd-w",
        main,
        None,
        "Cmd+W closed only the DAG, main-focus=graph",
    );

    fail_next_open(main, cx);
    click_graph(main, cx);
    assert_failure(main, cx, "open-failure");
    activate_control(main, cx, "dag-open-dismiss");
    assert_main_focus(main, cx, "graph");
    assert!(!main.read_with(cx, |view, _| view.dag_open_error).unwrap());
    report(
        cx,
        "dismiss-error",
        main,
        None,
        "dismissed, main-focus=graph",
    );

    fail_next_open(main, cx);
    click_graph(main, cx);
    assert_failure(main, cx, "open-failure-before-retry");
    activate_control(main, cx, "dag-open-retry");
    let child = child_window(main, cx);
    assert_graph(main, child, cx, &scene_b, None);
    assert_main_focus(main, cx, "graph");
    assert!(!main.read_with(cx, |view, _| view.dag_open_error).unwrap());
    report(
        cx,
        "retry-success",
        main,
        Some(child),
        "error-cleared main-focus=graph child-focus=content nodes=1 edges=0",
    );
    key(child.into(), cx, "escape");
    assert_closed(main, child, cx);

    exercise_canvas(main, cx, &scene_a, &scene_b, &root).await;
}

async fn exercise_canvas(
    main: WindowHandle<Workspace>,
    cx: &mut AsyncApp,
    scene: &str,
    other_scene: &str,
    root: &str,
) {
    navigate(
        main,
        cx,
        Command::FocusSession {
            scene_id: scene.to_owned(),
            session_id: root.to_owned(),
        },
    );
    let branch = wait_graph(main, cx, |view| {
        (view.graph["sceneId"] == scene && view.focused.as_deref() == Some(root)).then(|| {
            session_levels::children_of(session_levels::nodes(&view.graph), Some(root))
                .next()
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_owned()
        })
    })
    .await;
    navigate(
        main,
        cx,
        Command::SetSessionTitle {
            session_id: branch.clone(),
            title: Some("probe branch".to_owned()),
        },
    );
    wait_graph(main, cx, |view| {
        view.graph_node(&branch)
            .filter(|node| node["title"] == "probe branch")
            .map(|_| ())
    })
    .await;
    click_graph(main, cx);
    let child = child_window(main, cx);
    frame(child, cx);
    let start = read_canvas(child, cx, |canvas| canvas.transform);
    assert_ne!(
        start,
        DagTransform::default(),
        "initial target was not centered"
    );
    key(child.into(), cx, "ctrl-=");
    assert_eq!(read_canvas(child, cx, |canvas| canvas.transform.scale), 1.1);
    key(child.into(), cx, "ctrl--");
    assert_eq!(read_canvas(child, cx, |canvas| canvas.transform.scale), 1.);
    key(child.into(), cx, "ctrl-0");
    assert_eq!(
        read_canvas(child, cx, |canvas| canvas.transform),
        DagTransform::default()
    );
    report(
        cx,
        "canvas-zoom",
        main,
        Some(child),
        "zoom in/out and reset",
    );

    let width = read_canvas(child, cx, |canvas| canvas.viewport.width);
    pointer(
        child.into(),
        cx,
        (width - 10., 100.),
        Some((width - 55., 135.)),
    );
    assert_eq!(read_canvas(child, cx, |canvas| canvas.transform.x), 115.);
    assert_eq!(read_canvas(child, cx, |canvas| canvas.transform.y), 75.);
    for _ in 0..4 {
        pointer(child.into(), cx, (30., 64.), Some((600., 64.)));
    }
    read_canvas(child, cx, |canvas| {
        assert_eq!(canvas.transform.x, 115. - 4. * 570.);
        assert!(
            canvas
                .render_model()
                .real_nodes
                .iter()
                .all(|node| node.node.session_id != branch)
        );
    });
    report(
        cx,
        "canvas-pan",
        main,
        Some(child),
        "drag and offscreen culling",
    );
    let panned = read_canvas(child, cx, |canvas| canvas.transform);
    let owner = dag_window::viewport_owner_key(scene);
    wait_graph(main, cx, |view| {
        view.geometry
            .get(&owner)
            .filter(|row| {
                row.geometry == json!({"panX": panned.x, "panY": panned.y, "zoom": panned.scale})
                    && !view.geometry_dirty.contains(&owner)
            })
            .map(|_| ())
    })
    .await;

    search(child, cx, "probe");
    read_canvas(child, cx, |canvas| {
        assert_eq!(canvas.search.visible_results().len(), 2);
        assert_eq!(canvas.search.selected, 0);
    });
    key(child.into(), cx, "up");
    read_canvas(child, cx, |canvas| assert_eq!(canvas.search.selected, 1));
    key(child.into(), cx, "down");
    read_canvas(child, cx, |canvas| assert_eq!(canvas.search.selected, 0));
    key(child.into(), cx, "down");
    let (selected, transform, preview) = read_canvas(child, cx, |canvas| {
        (
            canvas.search.results[1].clone(),
            canvas.transform,
            canvas.preview_session_id.clone(),
        )
    });
    navigate(
        main,
        cx,
        Command::SetSessionTitle {
            session_id: root.to_owned(),
            title: Some("probe".to_owned()),
        },
    );
    wait_graph(main, cx, |view| {
        view.graph_node(root)
            .filter(|node| node["title"] == "probe")
            .map(|_| ())
    })
    .await;
    read_canvas(child, cx, |canvas| {
        assert_eq!(canvas.search.query, "probe");
        assert_eq!(canvas.search.results[canvas.search.selected], selected);
        assert_eq!(canvas.preview_session_id, preview);
        assert_eq!(canvas.transform, transform);
    });
    report(
        cx,
        "canvas-search-refresh",
        main,
        Some(child),
        "title matches, keyboard wrap and selection retained on Runtime update",
    );

    search(child, cx, "outputneedle");
    read_canvas(child, cx, |canvas| {
        assert!(canvas.search.results.is_empty())
    });
    navigate(
        main,
        cx,
        Command::Input {
            session_id: root.to_owned(),
            bytes: b"printf '%s%s\\n' output needle\r".to_vec(),
            defer_ordering: true,
        },
    );
    wait_graph(main, cx, |view| {
        view.graph_node(root)?["latestLines"]
            .as_array()?
            .iter()
            .any(|line| {
                line.as_str()
                    .is_some_and(|line| line.contains("outputneedle"))
            })
            .then_some(())
    })
    .await;
    let mut graph = child
        .read_with(cx, |child, _| child.probe_canvas().0.clone())
        .unwrap();
    read_canvas(child, cx, |canvas| {
        assert_eq!(canvas.search.query, "outputneedle");
        assert_eq!(canvas.search.results, [root]);
    });
    report(
        cx,
        "canvas-search-output",
        main,
        Some(child),
        "a terminal write updates the open search",
    );

    // Stress the same child with a scoped replacement snapshot while keeping
    // the real branch as the thirteenth result for the owner callback.
    let nodes = graph["nodes"].as_array_mut().unwrap();
    let chosen = nodes
        .iter_mut()
        .find(|node| node["sessionId"] == branch)
        .unwrap();
    chosen["title"] = json!("selectmatch branch");
    for index in 0..12 {
        nodes.push(json!({"sessionId": format!("match-{index}"), "title": "selectmatch", "cwd": "/fixture", "parentSessionId": root}));
    }
    let mut parent = branch.clone();
    for index in 2..=4 {
        let id = format!("depth-{index}");
        let status = ["running", "needs-input", "error"][index - 2];
        graph["nodes"].as_array_mut().unwrap().push(json!({
            "sessionId": id, "title": id, "cwd": "/fixture", "parentSessionId": parent, "workStatus": status,
        }));
        graph["edges"].as_array_mut().unwrap().push(json!({
            "parentSessionId": parent, "childSessionId": id, "relationKind": "derived-from",
        }));
        parent = id;
    }
    graph["edges"].as_array_mut().unwrap().extend((0..12).map(|index| json!({
        "parentSessionId": root, "childSessionId": format!("match-{index}"), "relationKind": "derived-from",
    })));
    graph["runtimeGeneration"] = json!("probe-reconnected");
    child
        .update(cx, |child, _, cx| child.refresh(&graph, cx))
        .unwrap();
    read_canvas(child, cx, |canvas| {
        assert_eq!(
            (canvas.search.query.as_str(), canvas.search.selected),
            ("outputneedle", 0)
        );
        assert_eq!(canvas.search.results, [root]);
        assert_eq!(canvas.preview_session_id, preview);
        assert_eq!(canvas.transform, transform);
    });
    report(
        cx,
        "canvas-reconnect",
        main,
        Some(child),
        "replacement snapshot preserves search, selection and viewport",
    );

    key(child.into(), cx, "escape");
    assert_closed(main, child, cx);
    report(
        cx,
        "canvas-search-escape",
        main,
        None,
        "Escape from the search field closes the child",
    );

    click_graph(main, cx);
    let child = child_window(main, cx);
    child
        .update(cx, |child, _, cx| child.refresh(&graph, cx))
        .unwrap();
    frame(child, cx);
    assert_eq!(read_canvas(child, cx, |canvas| canvas.transform), transform);
    // The aggregate scenario needs its root in view. Reopening now restores
    // the offscreen search viewport, so set up the aggregate fixture explicitly.
    key(child.into(), cx, "ctrl-0");
    child
        .update(cx, |view, _, cx| {
            view.probe_canvas().1.update(cx, |view, cx| {
                assert!(view.canvas_mut().focus_node(root));
                cx.notify();
            });
        })
        .unwrap();
    for _ in 0..6 {
        key(child.into(), cx, "ctrl--");
    }
    let (target, aggregate) = read_canvas(child, cx, |canvas| {
        let model = canvas.render_model();
        let entry = model
            .aggregates
            .iter()
            .find(|entry| entry.session_ids.len() == 3)
            .unwrap();
        assert_eq!(
            (
                entry.counts.running,
                entry.counts.needs_input,
                entry.counts.error
            ),
            (1, 1, 1)
        );
        let transform = canvas.transform;
        let center = (
            transform.x + (entry.rect.x + entry.rect.width * 0.5) * transform.scale,
            transform.y + (entry.rect.y + entry.rect.height * 0.5) * transform.scale,
        );
        assert!(
            center.0 > 0.
                && center.0 < canvas.viewport.width
                && center.1 > 54.
                && center.1 < canvas.viewport.height,
            "{center:?}"
        );
        (entry.target_session_id.to_owned(), center)
    });
    let right = child
        .update(cx, |_, window, _| {
            f64::from(window.viewport_size().width) - aggregate.0
        })
        .unwrap();
    click(child.into(), cx, right, aggregate.1);
    read_canvas(child, cx, |canvas| {
        assert_eq!(canvas.preview_session_id, target);
        assert!(
            canvas
                .render_model()
                .real_nodes
                .iter()
                .any(|node| node.node.session_id == target)
        );
    });
    report(
        cx,
        "canvas-aggregate",
        main,
        Some(child),
        "aggregate counts and card click center the aggregate target",
    );

    search(child, cx, "selectmatch");
    read_canvas(child, cx, |canvas| {
        assert_eq!(canvas.search.visible_results().len(), 12);
        assert_eq!(canvas.search.results.len(), 13);
        assert_eq!(canvas.search.results[12], branch);
    });
    key(child.into(), cx, "up");
    read_canvas(child, cx, |canvas| assert_eq!(canvas.search.selected, 12));
    navigate(
        main,
        cx,
        Command::SelectScene {
            scene_id: other_scene.to_owned(),
        },
    );
    wait_graph(main, cx, |view| {
        (view.active_scene_id() == Some(other_scene)).then_some(())
    })
    .await;
    assert_main_focus(main, cx, "graph");
    key(child.into(), cx, "enter");
    frame(child, cx);
    assert_child_closed(child, cx);
    wait_graph(main, cx, |view| {
        (view.active_scene_id() == Some(scene)
            && view.focused.as_deref() == Some(branch.as_str())
            && view.level_focus() == Some(branch.as_str())
            && view.level_parent() == Some(root))
        .then_some(())
    })
    .await;
    draw_main(main, cx);
    wait_graph(main, cx, |view| {
        (!view.recovery_blocked(&branch)).then_some(())
    })
    .await;
    draw_main(main, cx);
    main.update(cx, |view, window, cx| {
        assert!(view.terminals[&branch].read(cx).focus.is_focused(window));
        assert!(!view.control_focus["graph"].is_focused(window));
        assert!(!crate::macos::is_window_visible(window).unwrap());
    })
    .unwrap();
    const OUTPUT: &str = "dagreturnfocusok";
    for key_name in ["e", "c", "h", "o", "space"] {
        key(main.into(), cx, key_name);
    }
    for character in OUTPUT.chars() {
        key(main.into(), cx, &character.to_string());
    }
    key(main.into(), cx, "enter");
    wait_graph(main, cx, |view| {
        view.graph_node(&branch)?["latestLines"]
            .as_array()?
            .iter()
            .any(|line| line.as_str().is_some_and(|line| line.trim() == OUTPUT))
            .then_some(())
    })
    .await;
    report(
        cx,
        "canvas-search-enter",
        main,
        None,
        "the unrendered thirteenth match returns to its scene, gains terminal focus and runs typed input",
    );
}

fn search(child: WindowHandle<DagWindow>, cx: &mut AsyncApp, query: &str) {
    key(child.into(), cx, "ctrl-f");
    key(child.into(), cx, "cmd-a");
    for character in query.chars() {
        key(child.into(), cx, &character.to_string());
    }
}

fn read_canvas<T>(
    child: WindowHandle<DagWindow>,
    cx: &mut AsyncApp,
    read: impl FnOnce(&DagCanvas) -> T,
) -> T {
    child
        .read_with(cx, |child, cx| {
            read(child.probe_canvas().1.read(cx).canvas())
        })
        .unwrap()
}

fn frame(child: WindowHandle<DagWindow>, cx: &mut AsyncApp) {
    let view = AnyWindowHandle::from(child)
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
            appkit_view(window)
        })
        .unwrap();
    // Hidden windows have no display link. Deliver the frame through the
    // same GPUI AppKit callback that normally drains on_next_frame.
    unsafe {
        let _: () = msg_send![view, displayLayer: nil];
    }
}

fn draw_main(main: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    AnyWindowHandle::from(main)
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
        })
        .unwrap();
}

async fn wait_graph<T>(
    main: WindowHandle<Workspace>,
    cx: &mut AsyncApp,
    condition: impl Fn(&Workspace) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = main.read_with(cx, |view, _| condition(view)).unwrap() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "Runtime graph did not reach the expected state"
        );
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
}

fn navigate(main: WindowHandle<Workspace>, cx: &mut AsyncApp, command: Command) {
    main.update(cx, |view, _, cx| view.send(command, cx))
        .unwrap();
}

fn click_graph(main: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    // Right inset 8 + update 30 + split 28 + half the icon's 28 px.
    click(main.into(), cx, 80., 24.);
}

fn click(handle: AnyWindowHandle, cx: &mut AsyncApp, right: f64, top: f64) {
    pointer(handle, cx, (right, top), None);
}

fn pointer(handle: AnyWindowHandle, cx: &mut AsyncApp, from: (f64, f64), to: Option<(f64, f64)>) {
    let (view, number, size) = handle
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
            let view = appkit_view(window);
            let number: NSInteger = unsafe {
                let native: id = msg_send![view, window];
                msg_send![native, windowNumber]
            };
            (view, number, window.viewport_size())
        })
        .unwrap();
    let event = |kind, (right, top)| unsafe {
        let location = NSPoint::new(f64::from(size.width) - right, f64::from(size.height) - top);
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
            nil, kind, location, NSEventModifierFlags::empty(), 0., number, nil, 0, 1, 1.,
        )
    };
    // Deliver only to the hidden view, outside the GPUI borrow and desktop event queue.
    unsafe {
        let down = event(NSEventType::NSLeftMouseDown, from);
        let _: () = msg_send![view, mouseDown: down];
        if let Some(to) = to {
            let moved = event(NSEventType::NSLeftMouseDragged, to);
            let _: () = msg_send![view, mouseDragged: moved];
        }
        let up = event(NSEventType::NSLeftMouseUp, to.unwrap_or(from));
        let _: () = msg_send![view, mouseUp: up];
    }
}

fn key(handle: AnyWindowHandle, cx: &mut AsyncApp, key: &str) {
    assert!(dispatch_key(handle, cx, key), "{key} was not handled");
}

fn dispatch_key(handle: AnyWindowHandle, cx: &mut AsyncApp, key: &str) -> bool {
    handle
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
            window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx)
        })
        .unwrap()
}

fn activate_control(main: WindowHandle<Workspace>, cx: &mut AsyncApp, control: &str) {
    main.update(cx, |view, window, _| {
        window.focus(&view.control_focus[control])
    })
    .unwrap();
    key(main.into(), cx, "enter");
}

fn child_window(main: WindowHandle<Workspace>, cx: &mut AsyncApp) -> WindowHandle<DagWindow> {
    let child = main
        .read_with(cx, |view, _| view.dag_window)
        .unwrap()
        .unwrap();
    assert_ne!(main.window_id(), child.window_id());
    child
}

fn node_count(graph: &Value) -> usize {
    session_levels::nodes(graph).len()
}

fn assert_graph(
    main: WindowHandle<Workspace>,
    child: WindowHandle<DagWindow>,
    cx: &mut AsyncApp,
    scene: &str,
    branches: Option<(&str, usize)>,
) {
    let edge_count = branches.map_or(0, |(_, count)| count);
    AnyWindowHandle::from(child)
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
        })
        .unwrap();
    let child_graph = child
        .update(cx, |view, window, cx| {
            assert!(!crate::macos::is_window_visible(window).unwrap());
            assert!(!window.is_window_active());
            let (graph, canvas) = view.probe_canvas();
            let canvas = canvas.read(cx);
            let layout = &canvas.canvas().layout;
            assert!(canvas.canvas_focus().is_focused(window));
            assert_eq!(
                (layout.nodes.len(), layout.edges.len()),
                (edge_count + 1, edge_count)
            );
            graph.clone()
        })
        .unwrap();
    assert_eq!(child_graph["sceneId"], scene);
    assert_eq!(node_count(&child_graph), edge_count + 1);
    let edges = child_graph["edges"].as_array().unwrap();
    assert_eq!(edges.len(), edge_count);
    if let Some((parent, _)) = branches {
        let nodes = session_levels::nodes(&child_graph);
        assert!(session_levels::node(nodes, parent).is_some());
        let mut children = HashSet::new();
        for edge in edges {
            assert_eq!(edge["parentSessionId"], parent);
            assert_eq!(edge["relationKind"], "derived-from");
            let id = edge["childSessionId"].as_str().unwrap();
            assert!(id != parent && children.insert(id));
            assert!(session_levels::node(nodes, id).is_some());
        }
    }
    main.read_with(cx, |view, _| {
        if view.active_scene_id() == Some(scene) {
            assert_eq!(view.graph["nodes"], child_graph["nodes"]);
            assert_eq!(view.graph["edges"], child_graph["edges"]);
        }
    })
    .unwrap();
}

fn assert_main_focus(main: WindowHandle<Workspace>, cx: &mut AsyncApp, control: &str) {
    main.update(cx, |view, window, _| {
        assert!(view.control_focus[control].is_focused(window));
        assert!(!crate::macos::is_window_visible(window).unwrap());
        assert!(!window.is_window_active());
    })
    .unwrap();
}

fn close_system(child: WindowHandle<DagWindow>, cx: &mut AsyncApp) {
    let native: id = child
        .update(cx, |_, window, _| {
            let view = appkit_view(window);
            unsafe { msg_send![view, window] }
        })
        .unwrap();
    // Invoke AppKit outside the GPUI borrow because its close callback enters GPUI.
    unsafe {
        let _: () = msg_send![native, performClose: nil];
    }
}

fn appkit_view(window: &Window) -> id {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).unwrap().as_raw()
    else {
        panic!("expected AppKit window");
    };
    handle.ns_view.as_ptr() as id
}

fn assert_closed(main: WindowHandle<Workspace>, child: WindowHandle<DagWindow>, cx: &mut AsyncApp) {
    assert_child_closed(child, cx);
    assert_main_focus(main, cx, "graph");
}

fn assert_child_closed(child: WindowHandle<DagWindow>, cx: &mut AsyncApp) {
    assert!(
        !cx.update(|cx| cx.windows().contains(&child.into()))
            .unwrap()
    );
}

fn fail_next_open(main: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    main.update(cx, |view, _, _| {
        view.dag_probe.as_mut().unwrap().fail_next_open = true
    })
    .unwrap();
}

fn assert_failure(main: WindowHandle<Workspace>, cx: &mut AsyncApp, case: &str) {
    assert_main_focus(main, cx, "graph");
    main.read_with(cx, |view, _| {
        assert!(view.dag_open_error && view.dag_window.is_none());
        assert!(view.domain_ready && view.fault.is_none() && node_count(&view.graph) == 1);
    })
    .unwrap();
    let zh = dag_window::open_failure_text(InterfaceLocale::Chinese);
    let en = dag_window::open_failure_text(InterfaceLocale::English);
    assert_eq!(
        zh,
        "会话关系视图打开失败，当前会话列表和返回入口仍可继续使用。"
    );
    assert_eq!(
        en,
        "The session graph could not be opened. The session list and the return control still work."
    );
    report(
        cx,
        case,
        main,
        None,
        &format!("main-focus=graph nodes=1 edges=0 zh={zh:?} en={en:?}"),
    );
}

fn report(
    cx: &mut AsyncApp,
    case: &str,
    main: WindowHandle<Workspace>,
    child: Option<WindowHandle<DagWindow>>,
    details: &str,
) {
    let windows = cx.update(|cx| cx.windows()).unwrap();
    assert_eq!(windows.len(), 1 + usize::from(child.is_some()));
    assert!(windows.contains(&main.into()));
    if let Some(child) = child {
        assert!(windows.contains(&child.into()));
    }
    eprintln!(
        "[dag-window-probe] {case} windows={} main={:?} child={:?} {details}",
        windows.len(),
        main.window_id(),
        child.map(|child| child.window_id())
    );
}
