// Session DAG window and static canvas rendered from the Runtime projection.
//
// Frozen references:
// - apps/desktop/src/renderer/src/dag/DagWindowApp.tsx:244-272 (window shell)
// - apps/desktop/src/renderer/src/dag/DagCanvas.tsx:173-256 (canvas, cards)
// - apps/desktop/src/renderer/src/dag/dag-layout.ts:31-126 (layout values)
// - apps/desktop/src/renderer/src/dag/dag.css:2-94 (visual values)
// - apps/desktop/src/renderer/src/i18n/messages/dag.ts (bilingual copy)
// - apps/desktop/src/renderer/src/hierarchy/hierarchy.css:928-931
// - apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx:1132-1136
// - apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts:124-126,398-400

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use gpui::{prelude::*, *};
use matou_native_core::sessions::InterfaceLocale;
use serde_json::Value;

use crate::ui_text::ui_text;

pub(super) struct DagWindow {
    graph: Value,
    target: String,
    locale: InterfaceLocale,
    content_focus: FocusHandle,
    close_focus: FocusHandle,
}

impl DagWindow {
    pub(super) fn open(
        graph: &Value,
        target: &str,
        locale: InterfaceLocale,
        visible: bool,
        owner: &Window,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
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
                    window.focus(&content_focus);
                    Self {
                        graph: graph.clone(),
                        target: target.to_owned(),
                        locale,
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

    pub(super) fn retarget(
        &mut self,
        graph: &Value,
        target: &str,
        locale: InterfaceLocale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.graph = graph.clone();
        self.target = target.to_owned();
        if self.locale != locale {
            self.locale = locale;
            window.set_window_title(Self::title(locale));
        }
        cx.notify();
    }

    pub(super) fn refresh(&mut self, graph: &Value, cx: &mut Context<Self>) {
        // The already-open graph stays on its requested scene. A different scene
        // from the main window only takes over on another explicit open.
        if graph["sceneId"] == self.graph["sceneId"]
            && ["nodes", "edges", "focusedSessionId"]
                .iter()
                .any(|field| graph[*field] != self.graph[*field])
        {
            self.graph = graph.clone();
            cx.notify();
        }
    }

    #[cfg(all(target_os = "macos", debug_assertions))]
    pub(super) fn probe_state(&self) -> (&Value, &FocusHandle, usize, usize) {
        let layout = layout_graph(&self.graph);
        (
            &self.graph,
            &self.content_focus,
            layout.nodes.len(),
            layout.edges.len(),
        )
    }
}

impl Render for DagWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The workspace chrome currently uses the light palette across its windows.
        let light = true;
        let close = close_button(self.locale, light)
            .track_focus(&self.close_focus)
            .on_click(|_, window, cx| {
                cx.stop_propagation();
                window.remove_window();
            });
        div()
            .size_full()
            .relative()
            .track_focus(&self.content_focus)
            .on_key_down(
                |event: &KeyDownEvent, window, cx| match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        window.remove_window();
                    }
                    "tab" if !event.keystroke.modifiers.modified() => {
                        cx.stop_propagation();
                        window.focus_next();
                    }
                    _ => {}
                },
            )
            .child(render_canvas(
                &self.graph,
                Some(&self.target),
                self.locale,
                light,
            ))
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

const NODE_WIDTH: f32 = 260.;
const NODE_BASE_HEIGHT: f32 = 174.;
const TITLE_LINE_HEIGHT: f32 = 18.;
const TITLE_UNITS_PER_LINE: f32 = 18.;
const X_GAP: f32 = 110.;
const Y_GAP: f32 = 26.;
const VIEW_X: f32 = 70.;
const VIEW_Y: f32 = 40.;

const DAG_FAILURE_TEXT: u32 = 0x9b342f;
const DAG_FAILURE_BORDER: u32 = 0xefcaca;
const DAG_FAILURE_BACKGROUND: u32 = 0xfff2f1;

const FORK_EDGE: u32 = 0x5c91cf;
const DERIVED_EDGE: u32 = 0x4d6076;
const ARROW_EDGE: u32 = 0x57708b;
const ROOT_GUIDE: u32 = 0x5b7ea452;
const LEGEND_FORK: u32 = 0x5c91cf;
const LEGEND_DERIVED: u32 = 0x708196;

const STATUS_ACTIVE: u32 = 0x4cbd78;
const STATUS_INPUT: u32 = 0xe9ae43;
const STATUS_ERROR: u32 = 0xe66b64;
const STATUS_IDLE: u32 = 0x8d95a0;

#[derive(Clone, Copy)]
struct Palette {
    canvas: u32,
    grid: u32,
    legend_border: u32,
    legend_text: u32,
    legend_background: u32,
    card_border: u32,
    card_text: u32,
    card_background: u32,
    card_shadow: u32,
    card_shadow_alpha: f32,
    focus_border: u32,
    focus_ring: u32,
    focus_ring_alpha: f32,
    focus_shadow_alpha: f32,
    title: u32,
    path: u32,
    cwd: u32,
    pre_text: u32,
    pre_background: u32,
    meta: u32,
    top: u32,
    shared_text: u32,
    shared_background: u32,
    stopped_border: u32,
    stopped_text: u32,
    stopped_background: u32,
    stopped_title: u32,
    stopped_pre_text: u32,
    stopped_pre_background: u32,
    stopped_shadow_alpha: f32,
    close_text: u32,
    close_hover_background: u32,
}

fn palette(light: bool) -> Palette {
    if light {
        Palette {
            canvas: 0xf7f8fa,
            grid: 0x2c38481c,
            legend_border: 0xd5dae2,
            legend_text: 0x596270,
            legend_background: 0xfffffff5,
            card_border: 0xd5dae2,
            card_text: 0x505966,
            card_background: 0xffffff,
            card_shadow: 0x2e3949,
            card_shadow_alpha: 0.1,
            focus_border: 0x5b9de9,
            focus_ring: 0x5b9de9,
            focus_ring_alpha: 0.2,
            focus_shadow_alpha: 0.16,
            title: 0x2d3541,
            path: 0x858e9b,
            cwd: 0x6f7885,
            pre_text: 0x586271,
            pre_background: 0xf6f7f9,
            meta: 0x717985,
            top: 0x8d95a0,
            shared_text: 0x91620e,
            shared_background: 0xfff3d5,
            stopped_border: 0xd7dbe1,
            stopped_text: 0x7e8793,
            stopped_background: 0xeef0f3,
            stopped_title: 0x6b7480,
            stopped_pre_text: 0x7c8590,
            stopped_pre_background: 0xe7e9ed,
            stopped_shadow_alpha: 0.07,
            close_text: 0x596270,
            close_hover_background: 0x0000000d,
        }
    } else {
        Palette {
            canvas: 0x17191d,
            grid: 0xffffff13,
            legend_border: 0x363c45,
            legend_text: 0x929aa6,
            legend_background: 0x1e2126eb,
            card_border: 0x3a4049,
            card_text: 0xccd1d9,
            card_background: 0x22252a,
            card_shadow: 0x000000,
            card_shadow_alpha: 0.24,
            focus_border: 0x5b9de9,
            focus_ring: 0x5b9de9,
            focus_ring_alpha: 0.2,
            focus_shadow_alpha: 0.3,
            title: 0xf0f2f5,
            path: 0x858e9b,
            cwd: 0x6f7885,
            pre_text: 0xaeb5bf,
            pre_background: 0x00000026,
            meta: 0x717985,
            top: 0x8d95a0,
            shared_text: 0xe2b35d,
            shared_background: 0xe2b35d1c,
            stopped_border: 0x343941,
            stopped_text: 0x858d98,
            stopped_background: 0x1c1f23,
            stopped_title: 0xaab0b8,
            stopped_pre_text: 0x7b838e,
            stopped_pre_background: 0x0000001a,
            stopped_shadow_alpha: 0.16,
            close_text: 0xb8bec8,
            close_hover_background: 0xffffff14,
        }
    }
}

#[derive(Clone)]
struct DagNode {
    session_id: String,
    parent_session_id: Option<String>,
    title: String,
    cwd: String,
    status: String,
    mode: String,
    branch: Option<String>,
    branch_dirty: bool,
    shared: bool,
    archived: bool,
    latest_lines: Vec<String>,
    child_count: i64,
    last_activity_at: Option<i64>,
    last_user_interaction_seq: i64,
    sibling_created_seq: Option<i64>,
}

#[derive(Clone, Copy)]
enum EdgeKind {
    Fork,
    Derived,
}

struct PositionedNode {
    node: DagNode,
    depth: i64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone)]
struct DagEdge {
    from_x: f32,
    from_y: f32,
    to_x: f32,
    to_y: f32,
    kind: EdgeKind,
}

struct DagLayout {
    nodes: Vec<PositionedNode>,
    edges: Vec<DagEdge>,
    roots: Vec<(f32, f32)>,
    width: f32,
    height: f32,
}

/// The static canvas. It keeps the frozen full world size and clips it to the
/// fixed initial viewport (`70, 40`); there is deliberately no pan or zoom.
pub(super) fn render_canvas(
    graph: &Value,
    focused: Option<&str>,
    locale: InterfaceLocale,
    light: bool,
) -> Div {
    let palette = palette(light);
    let layout = layout_graph(graph);
    let focused = resolve_focused(graph, &layout, focused);

    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .bg(rgb(palette.canvas))
        .child(dot_grid(palette.grid))
        .child(
            div()
                .absolute()
                .left(px(VIEW_X))
                .top(px(VIEW_Y))
                .w(px(layout.width))
                .h(px(layout.height))
                .debug_selector(|| "dag-world".to_owned())
                .child(edge_layer(&layout))
                .children(layout.nodes.iter().map(|node| {
                    node_card(
                        node,
                        focused.as_deref() == Some(node.node.session_id.as_str()),
                        locale,
                        &palette,
                    )
                })),
        )
        .child(legend(locale, &palette))
}

/// The content-area close control, separate from the native system close.
/// The parent adds the focus/click wiring; this owns the visual and the tooltip.
/// It is already absolutely positioned in the content corner. If the parent
/// uses a helper that sets `relative` for a focus ring, attach focus directly
/// instead, otherwise that helper would replace this placement.
pub(super) fn close_button(locale: InterfaceLocale, light: bool) -> Stateful<Div> {
    let palette = palette(light);
    let tooltip = locale.text("关闭会话 DAG", "Close session DAG");
    div()
        .id("dag-window-close")
        .debug_selector(|| "dag-window-close".to_owned())
        .absolute()
        .top(px(14.))
        .right(px(16.))
        .w(px(28.))
        .h(px(28.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(palette.legend_border))
        .bg(rgba(palette.legend_background))
        .text_color(rgb(palette.close_text))
        .cursor_pointer()
        .hover(move |style| style.bg(rgba(palette.close_hover_background)))
        .tooltip(move |_, cx| cx.new(|_| DagTooltip(tooltip.into())).into())
        .child(
            svg()
                .path("icons/x.svg")
                .size(px(14.))
                .flex_none()
                .text_color(rgb(palette.close_text)),
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

fn resolve_focused(graph: &Value, layout: &DagLayout, focused: Option<&str>) -> Option<String> {
    let exists = |id: &str| layout.nodes.iter().any(|node| node.node.session_id == id);
    focused
        .filter(|id| exists(*id))
        .map(str::to_owned)
        .or_else(|| {
            graph
                .get("focusedSessionId")
                .and_then(Value::as_str)
                .filter(|id| exists(*id))
                .map(str::to_owned)
        })
        .or_else(|| {
            layout
                .nodes
                .first()
                .map(|node| node.node.session_id.clone())
        })
}

fn dot_grid(color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let mut y = 1.;
            while y < height {
                let mut x = 1.;
                while x < width {
                    window.paint_quad(fill(
                        Bounds {
                            origin: bounds.origin + point(px(x), px(y)),
                            size: size(px(2.), px(2.)),
                        },
                        rgba(color),
                    ));
                    x += 22.;
                }
                y += 22.;
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn edge_layer(layout: &DagLayout) -> impl IntoElement {
    let edges = layout.edges.clone();
    let roots = layout.roots.clone();
    let width = layout.width;
    let height = layout.height;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            for (root_x, root_y) in &roots {
                let mut guide = PathBuilder::stroke(px(1.5)).dash_array(&[px(4.), px(4.)]);
                guide.move_to(origin + point(px(12.), px(height * 0.5)));
                guide.cubic_bezier_to(
                    origin + point(px(*root_x), px(*root_y)),
                    origin + point(px(28.), px(height * 0.5)),
                    origin + point(px(32.), px(*root_y)),
                );
                if let Ok(path) = guide.build() {
                    window.paint_path(path, rgba(ROOT_GUIDE));
                }
            }
            for edge in &edges {
                let from = origin + point(px(edge.from_x), px(edge.from_y));
                let to = origin + point(px(edge.to_x), px(edge.to_y));
                let (color, dashed) = match edge.kind {
                    EdgeKind::Fork => (FORK_EDGE, false),
                    EdgeKind::Derived => (DERIVED_EDGE, true),
                };
                let mut path = PathBuilder::stroke(px(2.));
                if dashed {
                    path = path.dash_array(&[px(6.), px(4.)]);
                }
                let mid_x = (edge.from_x + edge.to_x) / 2.;
                path.move_to(from);
                path.cubic_bezier_to(
                    to,
                    origin + point(px(mid_x), px(edge.from_y)),
                    origin + point(px(mid_x), px(edge.to_y)),
                );
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(color));
                }

                let mut arrow = PathBuilder::fill();
                arrow.move_to(to);
                arrow.line_to(to + point(px(-9.), px(-4.5)));
                arrow.line_to(to + point(px(-9.), px(4.5)));
                arrow.close();
                if let Ok(path) = arrow.build() {
                    window.paint_path(path, rgb(ARROW_EDGE));
                }
            }
        },
    )
    .absolute()
    .left(px(0.))
    .top(px(0.))
    .w(px(width))
    .h(px(height))
}

fn legend(locale: InterfaceLocale, palette: &Palette) -> Div {
    div()
        .absolute()
        .top(px(14.))
        .right(px(56.))
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(8.))
        .border_1()
        .border_color(rgb(palette.legend_border))
        .rounded(px(9.))
        .bg(rgba(palette.legend_background))
        .text_color(rgb(palette.legend_text))
        .text_size(px(9.))
        .debug_selector(|| "dag-relation-legend".to_owned())
        .child(legend_item(
            locale.text("Fork：继承对话", "Fork: inherits the conversation"),
            LEGEND_FORK,
            false,
        ))
        .child(legend_item(
            locale.text(
                "普通关联：不继承对话",
                "Link: does not inherit the conversation",
            ),
            LEGEND_DERIVED,
            true,
        ))
}

fn legend_item(label: &'static str, color: u32, dashed: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let y = bounds.origin.y + bounds.size.height * 0.5;
                    let mut path = PathBuilder::stroke(px(2.));
                    if dashed {
                        path = path.dash_array(&[px(4.), px(3.)]);
                    }
                    path.move_to(point(bounds.origin.x, y));
                    path.line_to(point(bounds.origin.x + bounds.size.width, y));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, rgb(color));
                    }
                },
            )
            .w(px(18.))
            .h(px(4.))
            .flex_none(),
        )
        .child(ui_text(label))
}

fn node_card(
    positioned: &PositionedNode,
    focused: bool,
    locale: InterfaceLocale,
    palette: &Palette,
) -> Div {
    let node = &positioned.node;
    let stopped = node.archived;
    let visual_status = if stopped {
        "exited"
    } else {
        node.status.as_str()
    };
    let border = if focused {
        palette.focus_border
    } else if stopped {
        palette.stopped_border
    } else {
        palette.card_border
    };
    let background = if stopped {
        palette.stopped_background
    } else {
        palette.card_background
    };
    let text = if stopped {
        palette.stopped_text
    } else {
        palette.card_text
    };
    let title = if stopped {
        palette.stopped_title
    } else {
        palette.title
    };
    let pre_text = if stopped {
        palette.stopped_pre_text
    } else {
        palette.pre_text
    };
    let pre_background = if stopped {
        palette.stopped_pre_background
    } else {
        palette.pre_background
    };
    let dot_color = if stopped {
        palette.top
    } else {
        status_dot_color(visual_status)
    };
    let mut shadows = Vec::new();
    if focused {
        shadows.push(shadow_with_spread(
            palette.focus_ring,
            palette.focus_ring_alpha,
            0.,
            0.,
            2.,
        ));
    }
    let (shadow_y, shadow_blur, shadow_alpha) = if focused {
        (14., 34., palette.focus_shadow_alpha)
    } else if stopped {
        (8., 22., palette.stopped_shadow_alpha)
    } else {
        (10., 28., palette.card_shadow_alpha)
    };
    shadows.push(shadow(
        palette.card_shadow,
        shadow_alpha,
        shadow_y,
        shadow_blur,
    ));

    let branch = node.branch.as_deref();
    let child_count = child_count_text(node.child_count, locale);
    let stopped_suffix = if stopped {
        locale.text(" · 已停止", " · Stopped")
    } else {
        ""
    };
    let activity = activity_label(node, locale);
    let meta = format!("{child_count}{stopped_suffix} · {activity}");
    let shared = if branch.is_some() {
        locale.text("共享工作树", "Shared worktree")
    } else {
        locale.text("共享目录", "Shared directory")
    };
    let lines = if node.latest_lines.is_empty() {
        String::new()
    } else {
        let start = node.latest_lines.len().saturating_sub(4);
        node.latest_lines[start..].join("\n")
    };
    let lines = if lines.is_empty() {
        locale
            .text("等待会话输出…", "Waiting for session output…")
            .to_owned()
    } else {
        lines
    };
    let monospace = if cfg!(target_os = "macos") {
        "SF Mono"
    } else {
        "Consolas"
    };

    let status_dot = if stopped {
        div()
            .w(px(6.))
            .h(px(6.))
            .flex_none()
            .rounded(px(3.))
            .border_1()
            .border_color(rgb(0x757e89))
    } else {
        div()
            .w(px(6.))
            .h(px(6.))
            .flex_none()
            .rounded(px(3.))
            .bg(rgb(dot_color))
    };

    let session_id = node.session_id.clone();
    let selector = format!("dag-node-{session_id}");
    div()
        .absolute()
        .left(px(positioned.x))
        .top(px(positioned.y))
        .w(px(positioned.width))
        .h(px(positioned.height))
        .rounded(px(12.))
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(background))
        .overflow_hidden()
        .shadow(shadows)
        .debug_selector(move || selector.clone())
        .child(
            div()
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .gap(px(5.))
                .p(px(12.))
                .text_color(rgb(text))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(5.))
                        .text_color(rgb(palette.top))
                        .text_size(px(9.))
                        .child(status_dot)
                        .child(ui_text(status_label(visual_status, locale)))
                        .child(
                            div()
                                .ml_auto()
                                .child(ui_text(mode_label(&node.mode, locale))),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(title))
                        .text_size(px(13.))
                        .line_height(relative(1.35))
                        .font_weight(FontWeight::BOLD)
                        .whitespace_normal()
                        .child(ui_text(node.title.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(rgb(palette.path))
                        .text_size(px(9.))
                        .child(ui_text(branch.map_or_else(
                            || node.cwd.clone(),
                            |branch| {
                                if node.branch_dirty {
                                    format!("{branch}*")
                                } else {
                                    branch.to_owned()
                                }
                            },
                        ))),
                )
                .when(branch.is_some(), |card| {
                    card.child(
                        div()
                            .flex_none()
                            .w_full()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .mt(px(-5.))
                            .text_color(rgb(palette.cwd))
                            .text_size(px(8.))
                            .child(ui_text(compact_path(&node.cwd))),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .h(px(48.))
                        .overflow_hidden()
                        .p(px(6.))
                        .rounded(px(6.))
                        .bg(rgb(pre_background))
                        .font_family(monospace)
                        .text_color(rgb(pre_text))
                        .text_size(px(9.))
                        .line_height(relative(1.5))
                        .whitespace_normal()
                        .child(lines),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(palette.meta))
                        .text_size(px(9.))
                        .child(ui_text(meta)),
                )
                .when(node.shared, |card| {
                    card.child(
                        div()
                            .absolute()
                            .right(px(11.))
                            .bottom(px(10.))
                            .px(px(5.))
                            .py(px(2.))
                            .rounded(px(7.))
                            .bg(rgb(palette.shared_background))
                            .text_color(rgb(palette.shared_text))
                            .text_size(px(8.))
                            .child(ui_text(shared)),
                    )
                }),
        )
}

fn status_dot_color(status: &str) -> u32 {
    match status {
        "running" | "starting" => STATUS_ACTIVE,
        "needs-input" => STATUS_INPUT,
        "error" => STATUS_ERROR,
        _ => STATUS_IDLE,
    }
}

fn status_label(status: &str, locale: InterfaceLocale) -> &'static str {
    match status {
        "starting" => locale.text("运行中", "Running"),
        "idle" => locale.text("空闲", "Idle"),
        "running" => locale.text("运行中", "Running"),
        "needs-input" => locale.text("等待输入", "Needs input"),
        "error" => locale.text("异常", "Error"),
        "interrupted" => locale.text("中断", "Interrupted"),
        "exited" => locale.text("已停止", "Stopped"),
        _ => "",
    }
}

fn mode_label(mode: &str, locale: InterfaceLocale) -> &'static str {
    match mode {
        "shell" => "Shell",
        "claude-code" => "Claude",
        "codex" => "Codex",
        "antigravity" => "Antigravity",
        "dsh" => "DSH",
        "agent-team-member" => locale.text("队友", "Teammate"),
        _ => "",
    }
}

fn child_count_text(count: i64, locale: InterfaceLocale) -> String {
    match locale {
        InterfaceLocale::Chinese => format!("子会话 {count}"),
        InterfaceLocale::English if count == 1 => "1 child session".to_owned(),
        InterfaceLocale::English => format!("{count} child sessions"),
    }
}

fn activity_label(node: &DagNode, locale: InterfaceLocale) -> String {
    if let Some(activity) = node.last_activity_at.filter(|value| *value > 0) {
        if let Some(clock) = clock_time(activity) {
            return match locale {
                InterfaceLocale::Chinese => format!("最近活动 {clock}"),
                InterfaceLocale::English => format!("Last activity {clock}"),
            };
        }
    }
    match locale {
        InterfaceLocale::Chinese => format!("活动记录 #{}", node.last_user_interaction_seq),
        InterfaceLocale::English => format!("Activity record #{}", node.last_user_interaction_seq),
    }
}

fn clock_time(milliseconds: i64) -> Option<String> {
    let formatted = super::database_recovery_page::format_backup_time(milliseconds);
    formatted
        .rsplit_once(' ')
        .map(|(_, clock)| clock.to_owned())
}

fn compact_path(value: &str) -> String {
    if value.chars().count() <= 42 {
        return value.to_owned();
    }
    let parts = value
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() > 1 {
        format!("…/{}", parts[parts.len() - 2..].join("/"))
    } else {
        value.to_owned()
    }
}

fn shadow(color: u32, alpha: f32, y: f32, blur: f32) -> BoxShadow {
    shadow_with_spread(color, alpha, y, blur, 0.)
}

fn shadow_with_spread(color: u32, alpha: f32, y: f32, blur: f32, spread: f32) -> BoxShadow {
    let mut color: Hsla = rgb(color).into();
    color.a = alpha;
    BoxShadow {
        color,
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
    }
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

fn layout_graph(graph: &Value) -> DagLayout {
    let nodes = parse_nodes(graph);
    let mut index_by_id = HashMap::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        index_by_id.insert(node.session_id.clone(), index);
    }
    let parents = nodes
        .iter()
        .map(|node| {
            node.parent_session_id
                .as_deref()
                .and_then(|parent| index_by_id.get(parent).copied())
        })
        .collect::<Vec<_>>();
    let mut memo = vec![None; nodes.len()];
    let mut depths = Vec::with_capacity(nodes.len());
    for index in 0..nodes.len() {
        depths.push(node_depth(index, &parents, &mut memo));
    }

    let mut groups = BTreeMap::<i64, Vec<usize>>::new();
    for (index, depth) in depths.iter().enumerate() {
        groups.entry(*depth).or_default().push(index);
    }
    for peers in groups.values_mut() {
        peers.sort_by(|left, right| stable_node_order(&nodes[*left], &nodes[*right]));
    }
    let mut columns = groups
        .into_iter()
        .map(|(depth, peers)| {
            let heights = peers
                .iter()
                .map(|index| node_height(&nodes[*index].title))
                .collect::<Vec<_>>();
            let height =
                heights.iter().sum::<f32>() + (peers.len().saturating_sub(1) as f32) * Y_GAP;
            (depth, peers, heights, height)
        })
        .collect::<Vec<_>>();
    columns.sort_by_key(|(depth, _, _, _)| *depth);
    let max_column_height = columns
        .iter()
        .map(|(_, _, _, height)| *height)
        .fold(NODE_BASE_HEIGHT, f32::max);
    let depth_count = columns
        .last()
        .map_or(1, |(depth, _, _, _)| (*depth).saturating_add(1)) as f32;
    let width = 100. + depth_count * NODE_WIDTH + (depth_count - 1.).max(0.) * X_GAP;
    let height = 100. + max_column_height;

    let mut coords = vec![(0., 0., NODE_WIDTH, NODE_BASE_HEIGHT); nodes.len()];
    let mut positioned = Vec::with_capacity(nodes.len());
    for (depth, peers, heights, column_height) in &columns {
        let mut y = 50. + (max_column_height - column_height) / 2.;
        for (peer_index, index) in peers.iter().enumerate() {
            let height = heights[peer_index];
            let x = 50. + *depth as f32 * (NODE_WIDTH + X_GAP);
            coords[*index] = (x, y, NODE_WIDTH, height);
            positioned.push(PositionedNode {
                node: nodes[*index].clone(),
                depth: *depth,
                x,
                y,
                width: NODE_WIDTH,
                height,
            });
            y += height + Y_GAP;
        }
    }

    let mut edges = Vec::new();
    if let Some(items) = graph.get("edges").and_then(Value::as_array) {
        for item in items {
            let parent = item
                .get("parentSessionId")
                .and_then(Value::as_str)
                .and_then(|id| index_by_id.get(id))
                .copied();
            let child = item
                .get("childSessionId")
                .and_then(Value::as_str)
                .and_then(|id| index_by_id.get(id))
                .copied();
            let (Some(parent), Some(child)) = (parent, child) else {
                continue;
            };
            let kind = match item.get("relationKind").and_then(Value::as_str) {
                Some("forked-from") => EdgeKind::Fork,
                Some("derived-from") => EdgeKind::Derived,
                _ => continue,
            };
            let (parent_x, parent_y, parent_width, parent_height) = coords[parent];
            let (child_x, child_y, _, child_height) = coords[child];
            edges.push(DagEdge {
                from_x: parent_x + parent_width,
                from_y: parent_y + parent_height / 2.,
                to_x: child_x,
                to_y: child_y + child_height / 2.,
                kind,
            });
        }
    }
    let roots = positioned
        .iter()
        .filter(|node| node.depth == 0)
        .map(|node| (node.x, node.y + node.height / 2.))
        .collect();

    DagLayout {
        nodes: positioned,
        edges,
        roots,
        width,
        height,
    }
}

fn parse_nodes(graph: &Value) -> Vec<DagNode> {
    let Some(items) = graph.get("nodes").and_then(Value::as_array) else {
        return Vec::new();
    };
    items.iter().filter_map(parse_node).collect()
}

fn parse_node(value: &Value) -> Option<DagNode> {
    let session_id = value.get("sessionId").and_then(Value::as_str)?.to_owned();
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let branch = value
        .pointer("/worktree/branch")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/git/branch").and_then(Value::as_str))
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned);
    let latest_lines = value
        .get("latestLines")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let active_children = value
        .get("activeChildCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let stopped_children = value
        .get("stoppedChildCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Some(DagNode {
        session_id,
        parent_session_id: value
            .get("parentSessionId")
            .and_then(Value::as_str)
            .filter(|parent| !parent.is_empty())
            .map(str::to_owned),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        cwd,
        status: value
            .get("workStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        mode: value
            .get("currentMode")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        branch,
        branch_dirty: value
            .pointer("/git/dirty")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        shared: value
            .get("sharedWorkingDirectory")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || value
                .pointer("/worktree/shared")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        archived: value
            .get("archivedAt")
            .is_some_and(|archived| !archived.is_null()),
        latest_lines,
        child_count: active_children.saturating_add(stopped_children),
        last_activity_at: value.get("lastActivityAt").and_then(Value::as_i64),
        last_user_interaction_seq: value
            .get("lastUserInteractionSeq")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        sibling_created_seq: value.get("siblingCreatedSeq").and_then(Value::as_i64),
    })
}

fn node_depth(index: usize, parents: &[Option<usize>], memo: &mut [Option<i64>]) -> i64 {
    if let Some(depth) = memo[index] {
        return depth;
    }
    let mut path = Vec::new();
    let mut visiting = HashSet::new();
    let mut cursor = Some(index);
    let mut base;
    loop {
        let Some(current) = cursor else {
            base = -1;
            break;
        };
        if let Some(depth) = memo[current] {
            base = depth;
            break;
        }
        if !visiting.insert(current) {
            base = 0;
            break;
        }
        path.push(current);
        cursor = parents[current];
    }
    for current in path.into_iter().rev() {
        base += 1;
        memo[current] = Some(base);
    }
    memo[index].unwrap_or(0)
}

fn stable_node_order(left: &DagNode, right: &DagNode) -> Ordering {
    left.sibling_created_seq
        .unwrap_or(i64::MAX)
        .cmp(&right.sibling_created_seq.unwrap_or(i64::MAX))
        .then_with(|| left.session_id.cmp(&right.session_id))
}

fn node_height(title: &str) -> f32 {
    let units = title.chars().fold(0., |sum, character| {
        sum + if is_wide_character(character) {
            1.
        } else {
            0.55
        }
    });
    let lines = (units / TITLE_UNITS_PER_LINE).ceil().max(1.);
    NODE_BASE_HEIGHT + (lines - 1.) * TITLE_LINE_HEIGHT
}

fn is_wide_character(character: char) -> bool {
    matches!(
        character,
        '\u{2e80}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{ff01}'..='\u{ff60}'
            | '\u{ffe0}'..='\u{ffe6}'
    )
}
