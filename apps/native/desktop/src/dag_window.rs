// Independent session graph window over the workspace's Runtime projection.

use gpui::{prelude::*, *};
use matou_native_core::sessions::InterfaceLocale;
use matou_native_desktop::dag_canvas::{
    DagCanvas, DagCanvasEvent, DagCanvasView, DagGraph, DagLayout, DagSelection,
};
use serde_json::Value;

use super::{Command, Workspace, shadow};
use crate::ui_text::ui_text;

const DAG_FAILURE_TEXT: u32 = 0x9b342f;
const DAG_FAILURE_BORDER: u32 = 0xefcaca;
const DAG_FAILURE_BACKGROUND: u32 = 0xfff2f1;
const CLOSE_WIDTH: f32 = 28.;
const CLOSE_INSET: f32 = 16.;

pub(super) struct DagWindow {
    graph: Value,
    owner: WindowHandle<Workspace>,
    visible: bool,
    canvas: Entity<DagCanvasView>,
    canvas_events: Subscription,
    target: String,
    locale: InterfaceLocale,
    dark: bool,
    content_focus: FocusHandle,
    close_focus: FocusHandle,
}

impl DagWindow {
    pub(super) fn open(
        graph: &Value,
        target: &str,
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
        let state = DagCanvas::new(
            DagLayout::new(DagGraph::from_snapshot(graph)?)?,
            target,
            None,
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
                    appears_transparent: cfg!(target_os = "macos"),
                    traffic_light_position: cfg!(target_os = "macos")
                        .then_some(point(px(14.), px(16.))),
                }),
                app_id: Some("com.matou.native-parity".into()),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let content_focus = cx.focus_handle();
                    let canvas = cx.new(|cx| Self::canvas_view(state, locale, dark, cx));
                    window.focus(&canvas.read(cx).canvas_focus());
                    let canvas_events = cx.subscribe_in(&canvas, window, Self::canvas_event);
                    Self {
                        graph: graph.clone(),
                        owner: owner_handle,
                        visible,
                        canvas,
                        canvas_events,
                        target: target.to_owned(),
                        locale,
                        dark,
                        content_focus,
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
            .with_toolbar_right_inset(px(CLOSE_INSET + CLOSE_WIDTH + 12.))
    }

    pub(super) fn set_dark(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        locale: InterfaceLocale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if graph["sceneId"] != self.graph["sceneId"] {
            let layout = DagGraph::from_snapshot(graph).and_then(DagLayout::new);
            match layout {
                Ok(layout) => {
                    let state = DagCanvas::new(layout, target, None);
                    let canvas = cx.new(|cx| Self::canvas_view(state, locale, self.dark, cx));
                    window.focus(&canvas.read(cx).canvas_focus());
                    self.canvas_events = cx.subscribe_in(&canvas, window, Self::canvas_event);
                    self.canvas = canvas;
                    self.graph = graph.clone();
                }
                Err(error) => {
                    eprintln!("Retarget native session graph: {error}");
                    return;
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

    fn canvas_event(
        &mut self,
        canvas: &Entity<DagCanvasView>,
        event: &DagCanvasEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DagCanvasEvent::CloseRequested => window.remove_window(),
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
                window.remove_window();
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
            DagCanvasEvent::TransformChanged(_) => {}
        }
    }

    #[cfg(all(target_os = "macos", debug_assertions))]
    pub(super) fn probe_canvas(&self) -> (&Value, Entity<DagCanvasView>) {
        (&self.graph, self.canvas.clone())
    }
}

impl Render for DagWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let close = close_button(self.locale, !self.dark)
            .track_focus(&self.close_focus)
            .on_click(|_, window, cx| {
                cx.stop_propagation();
                window.remove_window();
            });
        div()
            .size_full()
            .relative()
            .track_focus(&self.content_focus)
            .on_key_down(|event: &KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        window.remove_window();
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
            })
            .child(self.canvas.clone())
            .child(close)
    }
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

impl Render for DagTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(320.))
            .px(px(9.))
            .py(px(7.))
            .border_1()
            .border_color(rgb(0xd6dae1))
            .rounded(px(7.))
            .bg(rgb(0xffffff))
            .text_color(rgb(0x333b47))
            .text_size(px(10.))
            .line_height(px(15.))
            .whitespace_normal()
            .shadow(vec![shadow(0x1f2837, 0.16, 8., 24.)])
            .child(ui_text(self.0.clone()))
    }
}
