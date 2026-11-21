//! Hidden macOS probe for the actual Workspace and its Runtime graph feed.

#![allow(deprecated, unexpected_cfgs)]

use super::*;
use cocoa::{
    appkit::{NSEvent, NSEventModifierFlags, NSEventType},
    base::{id, nil},
    foundation::{NSInteger, NSPoint},
};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

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
    assert_main_focus(main, cx, "graph");
    report(
        cx,
        "retarget",
        main,
        Some(child),
        "same-child main=B child=B main-focus=graph child-focus=content nodes=1 edges=0",
    );

    close_system(child, cx);
    assert_closed(main, child, cx);
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
    let (view, number, location) = handle
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
            let view = appkit_view(window);
            let number: NSInteger = unsafe {
                let native: id = msg_send![view, window];
                msg_send![native, windowNumber]
            };
            let size = window.viewport_size();
            let location = NSPoint::new(
                f64::from(f32::from(size.width)) - right,
                f64::from(f32::from(size.height)) - top,
            );
            (view, number, location)
        })
        .unwrap();
    // Deliver only to the hidden view, outside the GPUI borrow and desktop event queue.
    unsafe {
        for kind in [NSEventType::NSLeftMouseDown, NSEventType::NSLeftMouseUp] {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
                nil, kind, location, NSEventModifierFlags::empty(), 0., number, nil, 0, 1, 1.,
            );
            if kind == NSEventType::NSLeftMouseDown {
                let _: () = msg_send![view, mouseDown: event];
            } else {
                let _: () = msg_send![view, mouseUp: event];
            }
        }
    }
}

fn key(handle: AnyWindowHandle, cx: &mut AsyncApp, key: &str) {
    handle
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
            let keystroke = Keystroke::parse(key).unwrap();
            assert!(
                window.dispatch_keystroke(keystroke, cx),
                "{key} was not handled"
            );
        })
        .unwrap();
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
        .update(cx, |view, window, _| {
            assert!(!crate::macos::is_window_visible(window).unwrap());
            assert!(!window.is_window_active());
            let (graph, focus, rendered_nodes, rendered_edges) = view.probe_state();
            assert!(focus.is_focused(window));
            assert_eq!(
                (rendered_nodes, rendered_edges),
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
    assert!(
        !cx.update(|cx| cx.windows().contains(&child.into()))
            .unwrap()
    );
    assert_main_focus(main, cx, "graph");
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
