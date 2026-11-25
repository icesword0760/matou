// Independent session graph window over the workspace's Runtime projection.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{prelude::*, *};
use matou_native_core::{geometry::GeometryUpdate, sessions::InterfaceLocale};
use matou_native_desktop::dag_canvas::{
    DagCanvas, DagCanvasEvent, DagCanvasView, DagGraph, DagLayout, DagSelection, DagTransform,
};
use matou_native_desktop::windows_titlebar;
use serde_json::Value;

use super::{
    Command, Workspace, card_tooltip_box, is_child_window_close_shortcut, workspace_theme_dark,
};
use crate::ui_text::ui_text;

const DAG_FAILURE_TEXT: u32 = 0x9b342f;
const DAG_FAILURE_BORDER: u32 = 0xefcaca;
const DAG_FAILURE_BACKGROUND: u32 = 0xfff2f1;
const VIEWPORT_SAVE_DELAY: Duration = Duration::from_millis(250);
#[cfg(not(target_os = "windows"))]
const CLOSE_WIDTH: f32 = 28.;
#[cfg(not(target_os = "windows"))]
const CLOSE_INSET: f32 = 16.;

pub(super) struct DagWindow {
    graph: Value,
    owner: WindowHandle<Workspace>,
    visible: bool,
    canvas: Entity<DagCanvasView>,
    canvas_events: Subscription,
    viewport: DagViewport,
    viewport_save: Option<Task<()>>,
    target: String,
    locale: InterfaceLocale,
    dark: bool,
    content_focus: FocusHandle,
    #[cfg(not(target_os = "windows"))]
    close_focus: FocusHandle,
}

impl DagWindow {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open(
        graph: &Value,
        target: &str,
        saved_viewport: Option<&GeometryUpdate>,
        locale: InterfaceLocale,
        dark: bool,
        visible: bool,
        owner: &Window,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        let owner_handle = owner
            .window_handle()
            .downcast::<Workspace>()
            .ok_or_else(|| anyhow::anyhow!("The session graph requires a workspace window"))?;
        let (state, viewport) = DagViewport::canvas(
            DagLayout::new(DagGraph::from_snapshot(graph)?)?,
            target,
            saved_viewport,
        );
        let display = owner.display(cx).or_else(|| cx.primary_display());
        let area = display.as_ref().map_or_else(
            || Bounds::new(point(px(0.), px(0.)), owner.viewport_size()),
            |display| display.bounds(),
        );
        #[cfg(target_os = "macos")]
        let area = macos_work_area(owner).unwrap_or(area);
        let min_size = size(px(680.), px(480.));
        let length = |span: Pixels, min: Pixels| {
            px(((f32::from(span) - 24.) * std::f32::consts::FRAC_1_SQRT_2).round()).max(min)
        };
        let dimensions = size(
            length(area.size.width, min_size.width),
            length(area.size.height, min_size.height),
        );
        let origin = point(
            area.origin.x + px((f32::from(area.size.width - dimensions.width) / 2.).round()),
            area.origin.y + px((f32::from(area.size.height - dimensions.height) / 2.).round()),
        );
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, dimensions))),
                display_id: display.map(|display| display.id()),
                window_min_size: Some(min_size),
                show: visible,
                focus: visible,
                titlebar: Some(TitlebarOptions {
                    title: Some(Self::title(locale).into()),
                    appears_transparent: windows_titlebar::appears_transparent(),
                    traffic_light_position: cfg!(target_os = "macos")
                        .then_some(point(px(14.), px(16.))),
                }),
                app_id: Some("com.matou.native-parity".into()),
                ..Default::default()
            },
            |window, cx| {
                #[cfg(target_os = "windows")]
                matou_native_desktop::windows_theme::set_dark(window, dark);
                cx.new(|cx| {
                    let content_focus = cx.focus_handle();
                    let child = cx.weak_entity();
                    window.on_window_should_close(cx, move |_, cx| {
                        let _ = child.update(cx, |view: &mut Self, cx| view.flush_viewport(cx));
                        true
                    });
                    let canvas = cx.new(|cx| Self::canvas_view(state, locale, dark, cx));
                    window.focus(&canvas.read(cx).canvas_focus());
                    let canvas_events = cx.subscribe_in(&canvas, window, Self::canvas_event);
                    Self {
                        graph: graph.clone(),
                        owner: owner_handle,
                        visible,
                        canvas,
                        canvas_events,
                        viewport,
                        viewport_save: None,
                        target: target.to_owned(),
                        locale,
                        dark,
                        content_focus,
                        #[cfg(not(target_os = "windows"))]
                        close_focus: cx.focus_handle().tab_stop(true),
                    }
                })
            },
        )
    }

    fn title(locale: InterfaceLocale) -> &'static str {
        locale.text("码头 · 会话 DAG", "Matou · Session DAG")
    }

    fn canvas_view(
        state: DagCanvas,
        locale: InterfaceLocale,
        dark: bool,
        cx: &mut Context<DagCanvasView>,
    ) -> DagCanvasView {
        DagCanvasView::new_with_dark(state, locale, dark, cx)
            .with_toolbar_right_inset(px(dag_toolbar_right_inset()))
    }

    pub(super) fn set_dark(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        if self.dark != dark {
            matou_native_desktop::windows_theme::set_dark(window, dark);
        }
        self.dark = dark;
        self.canvas.update(cx, |view, cx| {
            let search_focused = view.search_focused(window, cx);
            view.set_dark(dark, cx);
            if search_focused {
                view.focus_search(window, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn retarget(
        &mut self,
        graph: &Value,
        target: &str,
        saved_viewport: Option<&GeometryUpdate>,
        read_only: bool,
        locale: InterfaceLocale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<GeometryUpdate> {
        let mut previous_viewport = None;
        if graph["sceneId"] != self.graph["sceneId"] {
            let layout = DagGraph::from_snapshot(graph).and_then(DagLayout::new);
            match layout {
                Ok(layout) => {
                    previous_viewport = self.take_viewport_update(read_only, cx);
                    let (state, viewport) = DagViewport::canvas(layout, target, saved_viewport);
                    let canvas = cx.new(|cx| Self::canvas_view(state, locale, self.dark, cx));
                    window.focus(&canvas.read(cx).canvas_focus());
                    self.canvas_events = cx.subscribe_in(&canvas, window, Self::canvas_event);
                    self.canvas = canvas;
                    self.viewport = viewport;
                    self.graph = graph.clone();
                }
                Err(error) => {
                    eprintln!("Retarget native session graph: {error}");
                    return None;
                }
            }
        } else {
            self.refresh(graph, cx);
            if self.target != target {
                self.canvas
                    .update(cx, |canvas, cx| canvas.center_node(target, window, cx));
            }
        }
        self.target = target.to_owned();
        if self.locale != locale {
            self.locale = locale;
            window.set_window_title(Self::title(locale));
            self.canvas
                .update(cx, |canvas, cx| canvas.set_locale(locale, cx));
        }
        cx.notify();
        previous_viewport
    }

    pub(super) fn refresh(&mut self, graph: &Value, cx: &mut Context<Self>) {
        // Another main-window scene takes over only on an explicit open.
        if graph["sceneId"] != self.graph["sceneId"]
            || ![
                "nodes",
                "edges",
                "focusedSessionId",
                "layoutRevision",
                "runtimeGeneration",
                "eventSequence",
            ]
            .iter()
            .any(|field| graph[*field] != self.graph[*field])
        {
            return;
        }
        let updated = DagGraph::from_snapshot(graph).and_then(|graph| {
            self.canvas.update(cx, |view, cx| {
                let updated = view.canvas_mut().apply_snapshot(graph)?;
                if updated {
                    cx.notify();
                }
                Ok(updated)
            })
        });
        match updated {
            Ok(true) => self.graph = graph.clone(),
            Ok(false) => {}
            Err(error) => eprintln!("Refresh native session graph: {error}"),
        }
    }

    pub(super) fn set_notifications(&mut self, sessions: &[String], cx: &mut Context<Self>) {
        self.canvas
            .update(cx, |view, cx| view.set_notifications(sessions, cx));
    }

    #[cfg(debug_assertions)]
    pub(super) fn has_notification_for_probe(&self, session: &str, cx: &App) -> bool {
        self.canvas.read(cx).has_notification_for_probe(session)
    }

    fn canvas_event(
        &mut self,
        canvas: &Entity<DagCanvasView>,
        event: &DagCanvasEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DagCanvasEvent::CloseRequested => self.close(window, cx),
            DagCanvasEvent::Select(DagSelection { session_id, .. }) => {
                let layout = &canvas.read(cx).canvas().layout;
                let Some(node) = layout.node(session_id).map(|node| &node.node) else {
                    return;
                };
                let scene = layout.scene_id().to_owned();
                let parent = node.parent_session_id.clone();
                let archived = node.archived;
                let session_id = session_id.clone();
                let owner = self.owner;
                let visible = self.visible;
                self.close(window, cx);
                // Restore the destination after the child has been removed.
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |workspace, window, cx| {
                        workspace
                            .level_parent_by_scene
                            .insert(scene.clone(), parent);
                        let active = workspace.active_scene_id() == Some(scene.as_str());
                        if archived {
                            if active {
                                workspace.refresh_level(cx);
                                workspace.hover_card(Some(session_id.clone()), cx);
                            } else {
                                workspace.send(
                                    Command::SelectScene {
                                        scene_id: scene.clone(),
                                    },
                                    cx,
                                );
                            }
                        } else {
                            workspace.dismiss_session_notification(&session_id, cx);
                            workspace.send_card_focus(&scene, &session_id, cx);
                            if active {
                                workspace.refresh_level(cx);
                            }
                        }
                        if visible {
                            window.activate_window();
                        }
                        if !archived {
                            workspace.return_terminal_focus(scene, session_id, window, cx);
                        }
                        cx.notify();
                    });
                });
            }
            DagCanvasEvent::TransformChanged(transform) => {
                if !self.owner.read(cx).is_ok_and(|owner| !owner.is_read_only())
                    || *transform == self.viewport.last_transform
                {
                    self.viewport_save.take();
                    return;
                }
                let timer = cx.background_executor().timer(VIEWPORT_SAVE_DELAY);
                self.viewport_save = Some(cx.spawn(async move |this, cx| {
                    timer.await;
                    let _ = this.update(cx, |view, cx| view.flush_viewport(cx));
                }));
            }
        }
    }

    pub(super) fn take_viewport_update(
        &mut self,
        read_only: bool,
        cx: &App,
    ) -> Option<GeometryUpdate> {
        self.viewport_save.take();
        let canvas = self.canvas.read(cx).canvas();
        self.viewport.update(
            canvas.layout.scene_id(),
            canvas.layout.layout_revision,
            canvas.transform,
            read_only,
        )
    }

    fn flush_viewport(&mut self, cx: &mut Context<Self>) {
        let read_only = self.owner.read(cx).is_ok_and(Workspace::is_read_only);
        if let Some(update) = self.take_viewport_update(read_only, cx) {
            let _ = self.owner.update(cx, |workspace, _, cx| {
                workspace.save_dag_viewport(update, cx);
            });
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_viewport(cx);
        window.remove_window();
    }

    #[cfg(all(target_os = "macos", debug_assertions))]
    pub(super) fn probe_canvas(&self) -> (&Value, Entity<DagCanvasView>) {
        (&self.graph, self.canvas.clone())
    }
}

impl Render for DagWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner;
        #[cfg(not(target_os = "windows"))]
        let _ = window;
        let root = div()
            .size_full()
            .relative()
            .track_focus(&self.content_focus)
            .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if is_child_window_close_shortcut(&event.keystroke) {
                    cx.stop_propagation();
                    this.close(window, cx);
                    #[cfg(target_os = "windows")]
                    cx.defer(move |cx| {
                        let _ = owner.update(cx, |workspace, window, cx| {
                            window.activate_window();
                            workspace.restore_dag_terminal_focus(window, cx);
                        });
                    });
                } else if Workspace::is_theme_shortcut(&event.keystroke) {
                    cx.stop_propagation();
                    cx.defer(move |cx| {
                        let _ = owner.update(cx, |workspace, _, cx| {
                            workspace.toggle_theme_shortcut(cx);
                        });
                    });
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        this.close(window, cx);
                    }
                    "tab" if !(modifiers.control || modifiers.alt || modifiers.platform) => {
                        cx.stop_propagation();
                        if modifiers.shift {
                            window.focus_prev();
                        } else {
                            window.focus_next();
                        }
                    }
                    _ => {}
                }
            }))
            .child(self.canvas.clone());
        #[cfg(target_os = "windows")]
        let root = root
            .child(
                div()
                    .id("dag-window-drag-band")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right(px(windows_titlebar::WINDOWS_CAPTION_CONTROLS_WIDTH))
                    .h(px(14.))
                    .window_control_area(WindowControlArea::Drag)
                    .occlude(),
            )
            .child(
                windows_titlebar::windows_caption_controls(40., self.dark, window.is_maximized())
                    .absolute()
                    .top_0()
                    .right_0(),
            );
        #[cfg(not(target_os = "windows"))]
        let root = root.child(
            close_button(self.locale, !self.dark)
                .track_focus(&self.close_focus)
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.close(window, cx);
                })),
        );
        root
    }
}

pub(super) fn viewport_owner_key(scene_id: &str) -> String {
    format!("dag-viewport:{scene_id}")
}

struct DagViewport {
    last_transform: DagTransform,
    saved_revision: f64,
}

impl DagViewport {
    fn canvas(
        layout: DagLayout,
        target: &str,
        saved: Option<&GeometryUpdate>,
    ) -> (DagCanvas, Self) {
        let saved = saved.filter(|saved| {
            saved.scene_id == layout.scene_id()
                && saved.owner_key == viewport_owner_key(layout.scene_id())
        });
        let initial_transform = saved.and_then(|saved| {
            Some(DagTransform {
                x: saved.geometry["panX"].as_f64()?,
                y: saved.geometry["panY"].as_f64()?,
                scale: saved.geometry["zoom"].as_f64()?,
            })
        });
        let canvas = DagCanvas::new(layout, target, initial_transform);
        let viewport = Self {
            last_transform: canvas.transform,
            saved_revision: saved.map_or(0., |value| value.layout_revision),
        };
        (canvas, viewport)
    }

    fn update(
        &mut self,
        scene_id: &str,
        layout_revision: Option<u64>,
        transform: DagTransform,
        read_only: bool,
    ) -> Option<GeometryUpdate> {
        if self.last_transform == transform
            || !transform.x.is_finite()
            || !transform.y.is_finite()
            || !transform.scale.is_finite()
            || transform.scale <= 0.
        {
            return None;
        }
        self.last_transform = transform;
        if read_only {
            return None;
        }
        self.saved_revision = self
            .saved_revision
            .max(layout_revision.unwrap_or_default() as f64);
        Some(GeometryUpdate {
            scene_id: scene_id.to_owned(),
            owner_key: viewport_owner_key(scene_id),
            layout_revision: self.saved_revision,
            geometry: serde_json::json!({
                "panX": transform.x, "panY": transform.y, "zoom": transform.scale,
            }),
            now: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        })
    }
}

#[cfg(test)]
mod viewport_tests {
    use super::*;
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn dag_window_tooltip_uses_light_and_dark_surfaces() {
        let tooltip = DagTooltip("Close session DAG".into());
        for (dark, background, border, text) in [
            (false, 0xfffffffa, 0xd0d7de, 0x57606a),
            (true, 0x1c2128ff, 0x30363d, 0xc9d1d9),
        ] {
            let mut element = tooltip.element(dark);
            let style = element.style();
            assert_eq!(style.background, Some(rgba(background).into()));
            assert_eq!(style.border_color, Some(rgb(border).into()));
            assert_eq!(style.text.as_ref().unwrap().color, Some(rgb(text).into()));
        }
    }

    #[::core::prelude::v1::test]
    fn scene_viewport_loads_and_coalesces_writeback_with_read_only_guard() -> anyhow::Result<()> {
        let graph = json!({"sceneId": "scene", "nodes": [], "edges": [], "layoutRevision": 5});
        let layout = DagLayout::new(DagGraph::from_snapshot(&graph)?)?;
        let saved = GeometryUpdate {
            scene_id: "scene".into(),
            owner_key: viewport_owner_key("scene"),
            layout_revision: 7.,
            geometry: json!({"panX": 25.0, "panY": -42.0, "zoom": 1.25}),
            now: 1,
        };
        let (mut canvas, mut viewport) = DagViewport::canvas(layout, "", Some(&saved));
        assert_eq!(
            canvas.transform,
            DagTransform {
                x: 25.,
                y: -42.,
                scale: 1.25
            }
        );
        assert!(!canvas.focus_initial());
        let changed = DagTransform {
            x: -15.,
            y: 10.,
            scale: 0.75,
        };
        assert!(viewport.update("scene", Some(5), changed, true).is_none());
        assert!(viewport.update("scene", Some(5), changed, false).is_none());
        let latest = DagTransform { x: 71., ..changed };
        let update = viewport.update("scene", Some(6), latest, false).unwrap();
        assert_eq!(update.owner_key, "dag-viewport:scene");
        assert_eq!(update.layout_revision, 7.);
        assert_eq!(
            update.geometry,
            json!({"panX": 71.0, "panY": 10.0, "zoom": 0.75})
        );
        assert!(viewport.update("scene", Some(6), latest, false).is_none());
        Ok(())
    }
}

/// Right edge reserved for the DAG window's own top-right control.
///
/// Windows reserves the full 138px caption cluster plus 12px; macOS keeps the
/// floating close button's 16 + 28 + 12pt inset.
#[cfg(target_os = "windows")]
fn dag_toolbar_right_inset() -> f32 {
    crate::dag_canvas::DAG_WINDOWS_TOOLBAR_RIGHT_INSET
}

#[cfg(not(target_os = "windows"))]
fn dag_toolbar_right_inset() -> f32 {
    CLOSE_INSET + CLOSE_WIDTH + 12.
}

#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
fn macos_work_area(owner: &Window) -> Option<Bounds<Pixels>> {
    use cocoa::{
        appkit::{NSScreen, NSWindow},
        base::id,
    };
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(owner).ok()?.as_raw()
    else {
        return None;
    };
    // Convert the screen's work area from AppKit to GPUI's display-local top-left coordinates.
    unsafe {
        let view = handle.ns_view.as_ptr() as id;
        let native: id = msg_send![view, window];
        let screen = native.screen();
        if screen.is_null() {
            return None;
        }
        let frame = NSScreen::frame(screen);
        let visible = NSScreen::visibleFrame(screen);
        Some(Bounds::new(
            point(
                px((visible.origin.x - frame.origin.x) as f32),
                px(
                    (frame.origin.y + frame.size.height - visible.origin.y - visible.size.height)
                        as f32,
                ),
            ),
            size(
                px(visible.size.width as f32),
                px(visible.size.height as f32),
            ),
        ))
    }
}

/// The content-area close control, separate from the native system close.
/// The parent adds the focus/click wiring; this owns the visual and the tooltip.
/// It is already absolutely positioned in the content corner. If the parent
/// uses a helper that sets `relative` for a focus ring, attach focus directly
/// instead, otherwise that helper would replace this placement.
#[cfg(not(target_os = "windows"))]
pub(super) fn close_button(locale: InterfaceLocale, light: bool) -> Stateful<Div> {
    let tooltip = locale.text("关闭会话 DAG", "Close session DAG");
    div()
        .id("dag-window-close")
        .debug_selector(|| "dag-window-close".to_owned())
        .absolute()
        .top(px(14.))
        .right(px(CLOSE_INSET))
        .size(px(CLOSE_WIDTH))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(if light { 0xd5dae2 } else { 0x363c45 }))
        .bg(rgba(if light { 0xfffffff5 } else { 0x1e2126eb }))
        .text_color(rgb(if light { 0x596270 } else { 0xb8bec8 }))
        .cursor_pointer()
        .hover(move |style| style.bg(rgba(if light { 0x0000000d } else { 0xffffff14 })))
        .tooltip(move |_, cx| cx.new(|_| DagTooltip(tooltip.into())).into())
        .child(
            svg()
                .path("icons/x.svg")
                .size(px(14.))
                .flex_none()
                .text_color(rgb(if light { 0x596270 } else { 0xb8bec8 })),
        )
}

pub(super) fn open_failure_text(locale: InterfaceLocale) -> &'static str {
    locale.text(
        "会话关系视图打开失败，当前会话列表和返回入口仍可继续使用。",
        "The session graph could not be opened. The session list and the return control still work.",
    )
}

pub(super) fn open_failure_retry_button(locale: InterfaceLocale) -> Stateful<Div> {
    failure_button("dag-open-retry").child(ui_text(
        locale.text("重试打开 DAG", "Retry opening the DAG"),
    ))
}

pub(super) fn open_failure_dismiss_button(locale: InterfaceLocale) -> Stateful<Div> {
    let tooltip = locale.text("关闭 DAG 异常提示", "Dismiss the DAG error");
    failure_button("dag-open-dismiss")
        .tooltip(move |_, cx| cx.new(|_| DagTooltip(tooltip.into())).into())
        .child(ui_text("×"))
}

/// Frozen `.dag-open-error`: the light dock banner from hierarchy.css:928,
/// including the 34px minimum height, 6/10 padding, 8px gap and current-color
/// outline on the two actions.
pub(super) fn render_open_failure(
    locale: InterfaceLocale,
    retry: Stateful<Div>,
    dismiss: Stateful<Div>,
) -> Div {
    div()
        .flex()
        .flex_none()
        .w_full()
        .min_h(px(34.))
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(6.))
        .border_b_1()
        .border_color(rgb(DAG_FAILURE_BORDER))
        .bg(rgb(DAG_FAILURE_BACKGROUND))
        .text_color(rgb(DAG_FAILURE_TEXT))
        .text_size(px(11.))
        .line_height(px(15.))
        .debug_selector(|| "dag-open-error".to_owned())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .debug_selector(|| "dag-open-error-text".to_owned())
                .child(ui_text(open_failure_text(locale))),
        )
        .child(retry)
        .child(dismiss)
}

fn failure_button(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .px(px(7.))
        .py(px(3.))
        .border_1()
        .border_color(rgb(DAG_FAILURE_TEXT))
        .rounded(px(5.))
        .text_color(rgb(DAG_FAILURE_TEXT))
        .text_size(px(11.))
        .line_height(px(15.))
        .whitespace_nowrap()
        .cursor_pointer()
}

struct DagTooltip(SharedString);

impl DagTooltip {
    fn element(&self, dark: bool) -> Div {
        card_tooltip_box(dark)
            .max_w(px(320.))
            .whitespace_normal()
            .child(ui_text(self.0.clone()))
    }
}

impl Render for DagTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = window
            .root::<DagWindow>()
            .flatten()
            .map_or_else(|| workspace_theme_dark(cx), |dag| dag.read(cx).dark);
        self.element(dark)
    }
}
