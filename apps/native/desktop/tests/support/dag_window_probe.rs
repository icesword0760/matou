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
            run(main, cx).await;
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    }
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
