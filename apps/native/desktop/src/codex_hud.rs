// Shared provider HUD fields for the native footer.
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

use super::InterfaceLocale;
use super::{CardTooltip, card_tooltip_box, footer_bounds_probe, footer_text};
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
const TOOL_DETAIL_COLOR: u32 = 0x8b949e;
const TOOL_DETAIL_HOVER_COLOR: u32 = 0x57606a;
const TOOL_DETAIL_HOVER_BG: u32 = 0x1f28370f;
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
    full_model: Option<SharedString>,
    effort: Option<SharedString>,
    context_percent: Option<f64>,
    context_window: Option<f64>,
    running_tools: Vec<RunningTool>,
    tool_counts: Vec<ToolCount>,
}

impl CodexHud {
    pub(super) fn from_node(node: &Value) -> Self {
        let payload = node.get("codexHud").filter(|value| value.is_object());
        let fallback = if payload.is_some() {
            &Value::Null
        } else {
            node
        };
        Self::from_projection(fallback, payload, "codex")
    }

    pub(super) fn from_live(node: &Value, payload: &Value, mode: &str) -> Self {
        Self::from_projection(node, Some(payload), mode)
    }

    fn from_projection(node: &Value, payload: Option<&Value>, mode: &str) -> Self {
        let mut model = payload
            .and_then(|hud| clean_text(&hud["model"]))
            .or_else(|| clean_text(&node["model"]));
        let full_model = model.clone();
        if mode == "claude-code"
            && let Some(name) = &model
            && let Some((prefix, rest)) = name.split_once(char::is_whitespace)
            && prefix.eq_ignore_ascii_case("claude")
            && !rest.trim().is_empty()
        {
            model = Some(rest.trim().to_owned().into());
        }
        let effort = payload
            .filter(|_| matches!(mode, "codex" | "dsh"))
            .and_then(|payload| clean_text(&payload["effort"]));
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
            full_model,
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
        self.format_model(self.model.as_ref()?)
    }

    pub(super) fn full_model_label(&self) -> Option<SharedString> {
        self.format_model(self.full_model.as_ref()?)
    }

    fn format_model(&self, model: &str) -> Option<SharedString> {
        let mut label = model.to_owned();
        if let Some(effort) = self.effort.as_ref() {
            label.push_str(&format!(" ({effort})"));
        }
        if let Some(window) = self.context_window {
            label.push_str(&format!(" ({} context)", format_context_window(window)));
        }
        Some(label.into())
    }

    pub(super) fn has_context_window(&self) -> bool {
        self.context_window.is_some()
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

    fn tool_details(&self, locale: InterfaceLocale) -> Vec<DetailRow> {
        let running = locale.text("运行中", "Running");
        self.running_tools
            .iter()
            .map(|tool| {
                let mut item = format!("{} · {running}", tool.name);
                if let Some(target) = &tool.target {
                    item.push_str(&format!(" · {target}"));
                }
                DetailRow::plain(item)
            })
            .chain(
                self.tool_counts
                    .iter()
                    .filter(|tool| tool.count > 0)
                    .map(|tool| {
                        DetailRow::plain(match locale {
                            InterfaceLocale::Chinese => {
                                format!("{} · {} 次", tool.name, tool.count)
                            }
                            InterfaceLocale::English => format!(
                                "{} · {} {}",
                                tool.name,
                                tool.count,
                                if tool.count == 1 { "call" } else { "calls" }
                            ),
                        })
                    }),
            )
            .collect()
    }

    pub(super) fn tools_slot(&self, locale: InterfaceLocale, dark: bool) -> Option<Stateful<Div>> {
        let total = self.tool_total();
        (total > 0).then(|| {
            Detail {
                id: "tools",
                label: format!("{total} Tools"),
                progress: None,
                title: locale.text("工具调用", "Tool calls"),
                rows: self.tool_details(locale),
                color: rgb(TOOL_DETAIL_COLOR),
            }
            .slot(locale, dark)
        })
    }
}

pub(super) fn supports_details(mode: &str) -> bool {
    matches!(mode, "claude-code" | "antigravity" | "codex")
}

pub(super) fn shows_model(mode: &str, status_width: f32) -> bool {
    !supports_details(mode) || status_width > MODEL_HIDE_BELOW
}

pub(super) fn model_slot(
    label: SharedString,
    full_label: SharedString,
    locale: InterfaceLocale,
    max_width: Option<f32>,
    color: Rgba,
) -> Stateful<Div> {
    let tooltip = SharedString::from(format!(
        "{}{}",
        locale.text("当前模型：", "Model: "),
        full_label
    ));
    div()
        .children(footer_bounds_probe("name"))
        .id("footer-name")
        .flex_shrink()
        .min_w_0()
        .when_some(max_width, |view, width| view.max_w(px(width)))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(color)
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
struct DetailRow {
    text: SharedString,
    dot: Option<Rgba>,
    started_at: Option<f64>,
    duration_ms: Option<f64>,
}

impl DetailRow {
    fn plain(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            dot: None,
            started_at: None,
            duration_ms: None,
        }
    }
}

pub(super) struct Detail {
    id: &'static str,
    label: String,
    progress: Option<String>,
    title: &'static str,
    rows: Vec<DetailRow>,
    color: Rgba,
}

impl Detail {
    pub(super) fn id(&self) -> &'static str {
        self.id
    }

    pub(super) fn slot(self, locale: InterfaceLocale, dark: bool) -> Stateful<Div> {
        let id = self.id;
        let tooltip = DetailTooltip {
            title: self.title,
            rows: self.rows,
            locale,
            dark,
        };
        div()
            .id(SharedString::from(format!("footer-{id}")))
            .flex_shrink_0()
            .min_w_0()
            .max_w(px(if id == "todos" { 220. } else { 108. }))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .children(footer_bounds_probe(id))
            .px(px(3.))
            .py(px(1.))
            .rounded(px(4.))
            .text_color(self.color)
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(footer_text(id, self.label)),
            )
            .children(
                self.progress
                    .map(|progress| div().flex_none().child(ui_text(progress))),
            )
            .hover(move |style| {
                if dark {
                    style.bg(Rgba {
                        a: 0.08,
                        ..rgb(0xffffff)
                    })
                } else {
                    let style = style.bg(rgba(TOOL_DETAIL_HOVER_BG));
                    if self.color == rgb(TOOL_DETAIL_COLOR) {
                        style.text_color(rgb(TOOL_DETAIL_HOVER_COLOR))
                    } else {
                        style
                    }
                }
            })
            .hoverable_tooltip(move |_, cx| {
                cx.new(|cx: &mut Context<DetailTooltip>| {
                    if tooltip.rows.iter().any(|row| row.started_at.is_some()) {
                        cx.spawn(async move |view, cx| {
                            loop {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_secs(1))
                                    .await;
                                if view.update(cx, |_, cx| cx.notify()).is_err() {
                                    break;
                                }
                            }
                        })
                        .detach();
                    }
                    tooltip.clone()
                })
                .into()
            })
    }
}

#[derive(Clone)]
struct DetailTooltip {
    title: &'static str,
    rows: Vec<DetailRow>,
    locale: InterfaceLocale,
    dark: bool,
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.
}

fn duration(milliseconds: f64, locale: InterfaceLocale) -> String {
    let seconds = (milliseconds / 1000.).max(0.) as u64;
    match (locale, seconds) {
        (InterfaceLocale::Chinese, 0..=59) => format!("{seconds}秒"),
        (InterfaceLocale::English, 0..=59) => format!("{seconds}s"),
        (InterfaceLocale::Chinese, 60..=3599) => format!("{}分{}秒", seconds / 60, seconds % 60),
        (InterfaceLocale::English, 60..=3599) => format!("{}m {}s", seconds / 60, seconds % 60),
        (InterfaceLocale::Chinese, _) => format!("{}时{:02}分", seconds / 3600, seconds / 60 % 60),
        (InterfaceLocale::English, _) => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

impl Render for DetailTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let now = now_ms();
        let mut panel = card_tooltip_box()
            .min_w(px(DETAIL_MIN_WIDTH))
            .max_w(px(DETAIL_MAX_WIDTH));
        if self.dark {
            panel = panel
                .bg(rgb(0x1c2128))
                .border_color(rgb(0x30363d))
                .text_color(rgb(0xc9d1d9));
        }
        panel
            .child(
                div()
                    .mb(px(5.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(if self.dark { 0xe6edf3 } else { 0x24292f }))
                    .child(ui_text(self.title)),
            )
            .child(
                div()
                    .id("footer-detail-rows")
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .when(self.rows.is_empty(), |list| {
                        list.child(ui_text(self.locale.text("暂无明细", "Details pending")))
                    })
                    .children(self.rows.iter().map(|row| {
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .children(row.dot.map(|color| {
                                div().size(px(6.)).flex_none().rounded_full().bg(color)
                            }))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(ui_text(row.text.clone())),
                            )
                            .children(
                                row.started_at
                                    .map(|start| now - start)
                                    .or(row.duration_ms)
                                    .map(|ms| {
                                        div().flex_none().child(ui_text(duration(ms, self.locale)))
                                    }),
                            )
                    })),
            )
    }
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn strings(value: &Value) -> Vec<DetailRow> {
    array(value)
        .iter()
        .filter_map(clean_text)
        .map(DetailRow::plain)
        .collect()
}

fn agent_tone(status: &str, dark: bool) -> (u8, Rgba) {
    match status {
        "failed" | "error" => (3, rgb(if dark { 0xf47067 } else { 0xcf222e })),
        "warning" => (2, rgb(if dark { 0xd29922 } else { 0x9a6700 })),
        "running" => (1, rgb(if dark { 0x79c0ff } else { 0x0969da })),
        _ => (0, rgb(TOOL_DETAIL_COLOR)),
    }
}

pub(super) fn statistics(
    hud: &Value,
    node: &Value,
    locale: InterfaceLocale,
    dark: bool,
) -> Vec<Detail> {
    let counts = &hud["configCounts"];
    let legacy_counts = &node["configCounts"];
    let count = |key: &str| {
        counts[key]
            .as_u64()
            .or_else(|| legacy_counts[key].as_u64())
            .unwrap_or_default()
    };
    let neutral = rgb(TOOL_DETAIL_COLOR);
    let mut slots = Vec::new();
    let mcps = [counts, legacy_counts]
        .into_iter()
        .find_map(|counts| {
            counts["mcps"]
                .as_u64()
                .or_else(|| counts["mcpServers"].as_u64())
        })
        .unwrap_or_default();
    if mcps > 0 {
        slots.push(Detail {
            id: "mcps",
            label: format!("{mcps} MCPs"),
            progress: None,
            title: locale.text("MCP 服务器", "MCP servers"),
            rows: strings(&counts["mcpServerNames"]),
            color: neutral,
        });
    }
    let mut tone = agent_tone("", dark);
    let agents = hud["agents"].as_array();
    let rows = if let Some(agents) = agents {
        agents
            .iter()
            .filter_map(|agent| {
                let name = clean_text(&agent["name"]).or_else(|| clean_text(&agent["label"]))?;
                let status = agent["status"]
                    .as_str()
                    .or_else(|| agent["state"].as_str())
                    .unwrap_or_default();
                let state = match status {
                    "running" => locale.text("运行中", "Running"),
                    "completed" => locale.text("已完成", "Completed"),
                    "failed" | "error" => locale.text("失败", "Failed"),
                    "warning" => locale.text("告警", "Warning"),
                    "waiting" => locale.text("等待中", "Waiting"),
                    _ => "",
                };
                let agent_tone = agent_tone(status, dark);
                if agent_tone.0 > tone.0 {
                    tone = agent_tone;
                }
                let mut text = name.to_string();
                if !state.is_empty() {
                    text.push_str(&format!(" · {state}"));
                }
                if let Some(detail) = clean_text(&agent["detail"]) {
                    text.push_str(&format!(" · {detail}"));
                }
                Some(DetailRow {
                    text: text.into(),
                    dot: Some(agent_tone.1),
                    started_at: agent["startedAt"].as_f64().filter(|_| status == "running"),
                    duration_ms: agent["durationMs"].as_f64(),
                })
            })
            .collect::<Vec<_>>()
    } else {
        strings(&hud["subagents"])
            .into_iter()
            .map(|mut row| {
                tone = agent_tone("running", dark);
                row.dot = Some(tone.1);
                row
            })
            .collect()
    };
    let agent_count = if let Some(agents) = agents {
        agents.len() as u64
    } else {
        hud["subagentCount"].as_u64().unwrap_or(rows.len() as u64)
    };
    if agent_count > 0 {
        let warning = if tone.0 >= 2 { "⚠ " } else { "" };
        slots.push(Detail {
            id: "agents",
            label: format!("{warning}{agent_count} Agents"),
            progress: None,
            title: locale.text("子代理", "Agents"),
            rows,
            color: tone.1,
        });
    }
    let hooks = count("hooks");
    if hooks > 0 {
        slots.push(Detail {
            id: "hooks",
            label: format!("{hooks} hooks"),
            progress: None,
            title: "Hooks",
            rows: strings(hud.get("hookNames").unwrap_or(&counts["hookNames"])),
            color: neutral,
        });
    }
    let errors = strings(&hud["mcpErrors"]);
    if !errors.is_empty() {
        slots.push(Detail {
            id: "mcp-errors",
            label: format!("⚠ {}", errors[0].text),
            progress: None,
            title: locale.text("MCP 错误", "MCP errors"),
            rows: errors,
            color: agent_tone("failed", dark).1,
        });
    }
    if let Some(todo) = todo_detail(hud, locale, dark) {
        slots.push(todo);
    }
    slots
}

fn todo_detail(hud: &Value, locale: InterfaceLocale, dark: bool) -> Option<Detail> {
    let todos = array(&hud["todos"]);
    if todos.is_empty() {
        return None;
    }
    let done = todos
        .iter()
        .filter(|todo| todo["status"] == "completed")
        .count();
    let complete = done == todos.len();
    let current = todos
        .iter()
        .find(|todo| todo["status"] == "in_progress")
        .and_then(|todo| clean_text(&todo["content"]))
        .map(|text| text.to_string());
    let label = if complete {
        format!("✓ {}", locale.text("全部完成", "All todos complete"))
    } else if let Some(current) = current {
        format!("▸ {current}")
    } else {
        locale.text("待办事项", "Todos").to_owned()
    };
    let rows = todos
        .iter()
        .filter_map(|todo| {
            let icon = match todo["status"].as_str() {
                Some("completed") => "✓",
                Some("in_progress") => "▸",
                _ => "○",
            };
            Some(DetailRow::plain(format!(
                "{icon} {}",
                clean_text(&todo["content"])?
            )))
        })
        .collect();
    Some(Detail {
        id: "todos",
        label,
        progress: Some(format!("({done}/{})", todos.len())),
        title: locale.text("待办事项", "Todos"),
        rows,
        color: if complete {
            rgb(if dark { 0x7ee787 } else { 0x1a7f37 })
        } else {
            rgb(TOOL_DETAIL_COLOR)
        },
    })
}

#[derive(Clone)]
pub(super) struct UsageWindow {
    pub label: SharedString,
    pub percent: f64,
    pub reset: Option<SharedString>,
}

impl UsageWindow {
    pub(super) fn summary(&self) -> String {
        let mut label = format!("{} {}", self.label, format_context_percent(self.percent));
        if let Some(reset) = &self.reset {
            label.push_str(&format!(" · {reset}"));
        }
        label
    }

    pub(super) fn color(&self, dark: bool) -> Rgba {
        rgb(match (self.percent, dark) {
            (p, true) if p >= 90. => 0xf47067,
            (p, false) if p >= 90. => 0xcf222e,
            (p, true) if p >= 75. => 0xd29922,
            (p, false) if p >= 75. => 0x9a6700,
            (_, true) => 0x58a6ff,
            (_, false) => 0x0969da,
        })
    }
}

pub(super) fn usage_windows(hud: &Value, key: &str) -> Vec<UsageWindow> {
    array(&hud[key])
        .iter()
        .filter_map(|entry| {
            let percent = entry["usedPercent"].as_f64()?;
            Some(UsageWindow {
                label: clean_text(&entry["label"])?,
                percent: percent.clamp(0., 100.),
                reset: clean_text(&entry["resetText"]),
            })
        })
        .collect()
}

pub(super) fn quota_row(usage: &UsageWindow, dark: bool) -> Div {
    quota_row_with_colors(
        usage,
        usage.color(dark),
        rgb(if dark { 0x8b949e } else { 0x57606a }),
    )
}

pub(super) fn claude_quota_row(usage: &UsageWindow) -> Div {
    quota_row_with_colors(usage, rgb(0x58a6ff), rgb(0x6e7681))
}

fn quota_row_with_colors(usage: &UsageWindow, percent: Rgba, reset: Rgba) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .whitespace_nowrap()
        .child(ui_text(usage.label.clone()))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(percent)
                .child(ui_text(format_context_percent(usage.percent))),
        )
        .children(
            usage
                .reset
                .as_ref()
                .map(|text| div().text_color(reset).child(ui_text(format!("· {text}")))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn all_interactive_providers_read_the_same_optional_statistics() {
        let hud = json!({
            "model": "Claude Sonnet", "contextWindowSize": 200_000,
            "runningTools": [{"id":"r1","name":"Read","target":"notes.md"}],
            "toolCounts": [{"name":"Find","count":2}, {"name":"Zero","count":0}],
            "agents": [
                {"name":"Reader","status":"running","detail":"Indexing","startedAt":1_700_000_000_000.0},
                {"name":"Writer","status":"completed","durationMs":65_000},
                {"name":"Reviewer","status":"warning","detail":"Retrying"},
                {"name":"Runner","status":"failed","detail":"Stopped"}
            ],
            "configCounts": {"mcps":2,"hooks":1,"claudeMd":1,"rules":3},
            "hookNames": ["PostToolUse"], "mcpErrors": ["server offline","authentication failed"],
            "todos": [{"content":"Read","status":"completed"},{"content":"Check results","status":"in_progress"}]
        });
        for mode in ["claude-code", "antigravity", "codex"] {
            let projection = CodexHud::from_live(&Value::Null, &hud, mode);
            assert!(supports_details(mode));
            assert!(!shows_model(mode, 210.));
            assert!(shows_model(mode, 210.01));
            assert_eq!(projection.context_percent(), None);
            assert!(projection.has_context_window());
            assert_eq!(
                projection.full_model_label().unwrap().as_ref(),
                "Claude Sonnet (200K context)"
            );
            assert_eq!(projection.tool_total(), 3);
            for (locale, running, completed) in [
                (
                    InterfaceLocale::English,
                    "Read · Running · notes.md",
                    "Find · 2 calls",
                ),
                (
                    InterfaceLocale::Chinese,
                    "Read · 运行中 · notes.md",
                    "Find · 2 次",
                ),
            ] {
                let details = projection.tool_details(locale);
                assert_eq!(details.len(), 2);
                assert_eq!(details[0].text.as_ref(), running);
                assert_eq!(details[1].text.as_ref(), completed);
            }
            let slots = statistics(&hud, &Value::Null, InterfaceLocale::English, true);
            assert_eq!(
                slots
                    .iter()
                    .map(|slot| (slot.id, slot.label.as_str()))
                    .collect::<Vec<_>>(),
                [
                    ("mcps", "2 MCPs"),
                    ("agents", "⚠ 4 Agents"),
                    ("hooks", "1 hooks"),
                    ("mcp-errors", "⚠ server offline"),
                    ("todos", "▸ Check results")
                ]
            );
            assert_eq!(slots[1].color, rgb(0xf47067));
            assert_eq!(
                slots[1].rows[0].text.as_ref(),
                "Reader · Running · Indexing"
            );
            assert_eq!(slots[1].rows[0].started_at, Some(1_700_000_000_000.));
            assert_eq!(
                duration(
                    slots[1].rows[1].duration_ms.unwrap(),
                    InterfaceLocale::English
                ),
                "1m 5s"
            );
            assert_eq!(slots[2].rows[0].text.as_ref(), "PostToolUse");
            assert_eq!(slots[3].rows.len(), 2);
            assert_eq!(slots[4].rows[1].text.as_ref(), "▸ Check results");
            assert_eq!(slots[4].progress.as_deref(), Some("(1/2)"));
        }
        let blank = CodexHud::from_live(&Value::Null, &json!({"runningTools":[{}]}), "codex");
        assert_eq!(blank.tool_total(), 0);
        assert!(statistics(&Value::Null, &Value::Null, InterfaceLocale::English, false).is_empty());
    }

    #[::core::prelude::v1::test]
    fn quota_groups_keep_their_reset_times_and_thresholds() {
        let hud = json!({
            "usageWindows":[{"id":"current","label":"Current model","usedPercent":91,"resetText":"12:00"}],
            "allUsageWindows":[{"id":"other","label":"Other models","usedPercent":77},{"id":"invalid","label":"Malformed"}]
        });
        let current = usage_windows(&hud, "usageWindows");
        let groups = usage_windows(&hud, "allUsageWindows");
        assert_eq!(current[0].summary(), "Current model 91% · 12:00");
        assert_eq!(current[0].color(false), rgb(0xcf222e));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].summary(), "Other models 77%");
        assert_eq!(groups[0].color(true), rgb(0xd29922));
    }

    #[::core::prelude::v1::test]
    fn completed_todos_and_legacy_agent_states_use_the_same_pipeline() {
        let hud = json!({
            "agents": [{"label":"Legacy worker","state":"error","detail":"old shape"}],
            "todos": [{"content":"Test","status":"completed"}]
        });
        let stats = statistics(&hud, &Value::Null, InterfaceLocale::Chinese, false);
        assert_eq!(
            stats[0].rows[0].text.as_ref(),
            "Legacy worker · 失败 · old shape"
        );
        assert_eq!(stats[1].label, "✓ 全部完成");
        assert_eq!(stats[1].progress.as_deref(), Some("(1/1)"));
    }
}
