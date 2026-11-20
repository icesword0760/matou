// Frozen Codex HUD fields for the native footer.
//
// Render-only: the runtime already projects `node["codexHud"]`. This module
// parses that optional payload, composes the frozen model label, and draws the
// context ring and tool-detail cell with the existing light footer primitives.
//
// Frozen sources:
// - apps/desktop/src/renderer/src/hud/TerminalHud.tsx:182-191,505-517,540-591
// - apps/desktop/src/renderer/src/i18n/messages/hud.ts:24-36
// - apps/desktop/src/renderer/src/hierarchy/hierarchy.css:354-419,469-476

use gpui::{prelude::*, *};
use serde_json::Value;

use super::{
    CardTooltip, FOOTER_NAME_MAX_WIDTH, card_tooltip_box, footer_bounds_probe, footer_text,
};
use crate::ui_text::ui_text;

/// `.status-priority-8` hides the model at a status content box of 210pt or less.
pub(super) const MODEL_HIDE_BELOW: f32 = 210.;
/// `.status-priority-7` hides the context ring at 250pt or less.
pub(super) const CONTEXT_HIDE_BELOW: f32 = 250.;

const RING_SIZE: f32 = 14.;
const RING_GAP: f32 = 4.;
const RING_LABEL_SIZE: f32 = 11.;
const RING_LABEL_LINE_HEIGHT: f32 = 14.;
/// SVG `circle r=8` in a 20 viewBox at 14pt, and `stroke-width: 2.5`.
const RING_RADIUS: f32 = RING_SIZE * 8. / 20.;
const RING_STROKE: f32 = 2.5 * RING_SIZE / 20.;
const RING_TRACK: u32 = 0x21262d;
const CONTEXT_GREEN: u32 = 0x3fb950;
const CONTEXT_YELLOW: u32 = 0xd29922;
const CONTEXT_RED: u32 = 0xf85149;
const MODEL_COLOR: u32 = 0x79c0ff;
const TOOL_DETAIL_COLOR: u32 = 0x8b949e;
const TOOL_DETAIL_HOVER_COLOR: u32 = 0x57606a;
const TOOL_DETAIL_HOVER_BG: u32 = 0x1f28370f;
const TOOL_CALLS_TITLE: &str = "工具调用";
const TOOL_RUNNING: &str = "运行中";
const TOOL_COUNT_SUFFIX: &str = "次";
const DETAIL_MAX_WIDTH: f32 = 320.;
const DETAIL_MIN_WIDTH: f32 = 156.;

struct RunningTool {
    name: SharedString,
    target: Option<SharedString>,
}

struct ToolCount {
    name: SharedString,
    count: u64,
}

/// Optional `node["codexHud"]` payload. Every field stands alone; the model
/// falls back to `node["model"]` only when the payload object is absent.
pub(super) struct CodexHud {
    model: Option<SharedString>,
    effort: Option<SharedString>,
    context_percent: Option<f64>,
    context_window: Option<f64>,
    running_tools: Vec<RunningTool>,
    tool_counts: Vec<ToolCount>,
}

impl CodexHud {
    pub(super) fn from_node(node: &Value) -> Self {
        let payload = node.get("codexHud").filter(|value| value.is_object());
        let model = match payload {
            Some(payload) => clean_text(&payload["model"]),
            None => clean_text(&node["model"]),
        };
        let effort = payload.and_then(|payload| clean_text(&payload["effort"]));
        let context_percent = payload
            .and_then(|payload| payload["contextPercent"].as_f64())
            .filter(|percent| percent.is_finite() && *percent >= 0.);
        let context_window = payload
            .and_then(|payload| payload["contextWindowSize"].as_f64())
            .filter(|window| window.is_finite() && *window > 0.);
        let running_tools = payload
            .and_then(|payload| payload["runningTools"].as_array())
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| {
                        Some(RunningTool {
                            name: clean_text(&tool["name"])?,
                            target: clean_text(&tool["target"]),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tool_counts = payload
            .and_then(|payload| payload["toolCounts"].as_array())
            .map(|counts| {
                counts
                    .iter()
                    .filter_map(|tool| {
                        Some(ToolCount {
                            name: clean_text(&tool["name"])?,
                            count: nonnegative_count(&tool["count"])?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            model,
            effort,
            context_percent,
            context_window,
            running_tools,
            tool_counts,
        }
    }

    /// Frozen `modelLabel`: `model (effort) (200K context)`. The context
    /// suffix only exists when the model label is non-empty and the window is
    /// a positive number.
    pub(super) fn model_label(&self) -> Option<SharedString> {
        let mut label = self.model.as_ref()?.to_string();
        if let Some(effort) = self.effort.as_ref() {
            label.push_str(&format!(" ({effort})"));
        }
        if let Some(window) = self.context_window {
            label.push_str(&format!(" ({} context)", format_context_window(window)));
        }
        Some(label.into())
    }

    pub(super) fn context_percent(&self) -> Option<f64> {
        self.context_percent
    }

    fn tool_total(&self) -> u64 {
        let running = self.running_tools.len() as u64;
        self.tool_counts
            .iter()
            .fold(running, |total, tool| total.saturating_add(tool.count))
    }

    fn tool_details(&self) -> Vec<SharedString> {
        let mut items = Vec::with_capacity(self.running_tools.len() + self.tool_counts.len());
        for tool in &self.running_tools {
            let item = match tool.target.as_ref() {
                Some(target) if !target.is_empty() => {
                    format!("{} · {TOOL_RUNNING} · {target}", tool.name)
                }
                _ => format!("{} · {TOOL_RUNNING}", tool.name),
            };
            items.push(item.into());
        }
        for tool in &self.tool_counts {
            items.push(format!("{} · {} {TOOL_COUNT_SUFFIX}", tool.name, tool.count).into());
        }
        items
    }

    /// Frozen `HudDetail` cell. Only rendered when at least one running tool
    /// or one counted call exists.
    pub(super) fn tools_slot(&self) -> Option<Stateful<Div>> {
        let total = self.tool_total();
        if total == 0 {
            return None;
        }
        let tooltip = ToolsTooltip {
            items: self.tool_details(),
        };
        Some(
            div()
                .id("footer-tools")
                .flex_none()
                .flex()
                .items_center()
                .children(footer_bounds_probe("tools"))
                .px(px(3.))
                .py(px(1.))
                .rounded(px(4.))
                .text_color(rgb(TOOL_DETAIL_COLOR))
                .child(footer_text("tools", format!("{total} Tools")))
                .hover(|style| {
                    style
                        .text_color(rgb(TOOL_DETAIL_HOVER_COLOR))
                        .bg(rgba(TOOL_DETAIL_HOVER_BG))
                })
                .tooltip(move |_, cx| cx.new(|_| tooltip.clone()).into()),
        )
    }
}

/// The existing footer model slot, at its frozen 180pt single-line budget.
pub(super) fn model_slot(label: SharedString) -> Stateful<Div> {
    let tooltip = SharedString::from(format!("当前模型：{label}"));
    div()
        .children(footer_bounds_probe("name"))
        .id("footer-name")
        .flex_shrink()
        .min_w_0()
        .max_w(px(FOOTER_NAME_MAX_WIDTH))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(rgb(MODEL_COLOR))
        .child(footer_text("name", label))
        .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into())
}

/// Frozen `ContextRing`: 14pt ring, a 4pt gap, and an 11pt percentage label.
pub(super) fn context_ring(percent: f64) -> Stateful<Div> {
    let label = format_context_percent(percent);
    let tooltip = SharedString::from(format!("Context: {label}"));
    let color = context_color(percent);
    div()
        .id("footer-context")
        .flex_none()
        .flex()
        .items_center()
        .gap(px(RING_GAP))
        .children(footer_bounds_probe("context"))
        .child(context_ring_canvas(percent, color))
        .child(
            div()
                .text_size(px(RING_LABEL_SIZE))
                .line_height(px(RING_LABEL_LINE_HEIGHT))
                .text_color(rgb(color))
                .child(footer_text("context", label)),
        )
        .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into())
}

fn context_ring_canvas(percent: f64, color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = px(RING_RADIUS);
            paint_ring(
                window,
                center,
                radius,
                -std::f32::consts::FRAC_PI_2,
                std::f32::consts::TAU,
                RING_TRACK,
            );
            let fraction = (percent / 100.).clamp(0., 1.) as f32;
            if fraction > 0. {
                paint_ring(
                    window,
                    center,
                    radius,
                    -std::f32::consts::FRAC_PI_2,
                    std::f32::consts::TAU * fraction,
                    color,
                );
            }
        },
    )
    .size(px(RING_SIZE))
    .flex_none()
}

fn paint_ring(
    window: &mut Window,
    center: Point<Pixels>,
    radius: Pixels,
    start: f32,
    sweep: f32,
    color: u32,
) {
    if !(sweep > 0.) {
        return;
    }
    let at = |angle: f32| center + point(radius * angle.cos(), radius * angle.sin());
    // Keep every `arc_to` at or below a quarter turn so half/full circles are
    // unambiguous in the path builder.
    let segments = (sweep / std::f32::consts::FRAC_PI_2).ceil().max(1.) as usize;
    let step = sweep / segments as f32;
    let mut path = PathBuilder::stroke(px(RING_STROKE));
    path.move_to(at(start));
    for index in 0..segments {
        path.arc_to(
            point(radius, radius),
            px(0.),
            false,
            true,
            at(start + step * (index + 1) as f32),
        );
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(color));
    }
}

fn context_color(percent: f64) -> u32 {
    if percent >= 85. {
        CONTEXT_RED
    } else if percent >= 70. {
        CONTEXT_YELLOW
    } else {
        CONTEXT_GREEN
    }
}

fn format_context_percent(percent: f64) -> String {
    if percent.fract() == 0. {
        format!("{}%", percent as i64)
    } else {
        format!("{percent}%")
    }
}

/// Frozen `formatContextWindow`: M with one decimal, then K, then raw size.
fn format_context_window(size: f64) -> String {
    if size >= 1_000_000. {
        let value = (size / 1_000_000. * 10.).round() / 10.;
        if value.fract() == 0. {
            format!("{}M", value as i64)
        } else {
            format!("{value}M")
        }
    } else if size >= 1_000. {
        format!("{}K", (size / 1_000.).round() as i64)
    } else {
        format!("{}", size.round() as i64)
    }
}

fn clean_text(value: &Value) -> Option<SharedString> {
    let text = value.as_str()?.replace(['\r', '\n', '\t'], " ");
    let text = text.trim();
    (!text.is_empty()).then(|| SharedString::from(text.to_owned()))
}

fn nonnegative_count(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|count| count.is_finite() && *count >= 0.)
            .map(|count| count as u64)
    })
}

#[derive(Clone)]
struct ToolsTooltip {
    items: Vec<SharedString>,
}

impl Render for ToolsTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        card_tooltip_box()
            .min_w(px(DETAIL_MIN_WIDTH))
            .max_w(px(DETAIL_MAX_WIDTH))
            .child(
                div()
                    .mb(px(5.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x24292f))
                    .child(ui_text(TOOL_CALLS_TITLE)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .children(self.items.iter().map(|item| {
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(ui_text(item.clone()))
                    })),
            )
    }
}
