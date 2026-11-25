// Shared provider HUD fields for the native footer.
//
// Render-only: the runtime already projects `node["codexHud"]`. This module
// parses that optional payload, composes the frozen model label, and draws the
// context ring and session statistics with the existing footer primitives.
//
// Frozen sources:
// - apps/desktop/src/renderer/src/hud/TerminalHud.tsx:182-191,505-517,540-591
// - apps/desktop/src/renderer/src/i18n/messages/hud.ts:24-36
// - apps/desktop/src/renderer/src/hierarchy/hierarchy.css:354-419,469-476

use gpui::{prelude::*, *};
use matou_native_desktop::theme_tokens::FooterPalette;
use serde_json::Value;

use super::InterfaceLocale;
use super::{
    CardTooltip, card_tooltip_box, footer_bounds_probe, footer_text, footer_token_count,
    popover_colors, shadow,
};
use crate::ui_text::ui_text;

/// `.status-priority-8` hides the model at a status content box of 210pt or less.
pub(super) const MODEL_HIDE_BELOW: f32 = 210.;
/// `.status-priority-7` hides the context ring at 250pt or less.
pub(super) const CONTEXT_HIDE_BELOW: f32 = 250.;

const RING_SIZE: f32 = 11.;
const RING_GAP: f32 = 4.;
const RING_RADIUS: f32 = 4.5;
const RING_STROKE: f32 = 2.;
const CONTEXT_RED: u32 = 0xf85149;
const AGENT_NEUTRAL_COLOR: u32 = 0x8b949e;
/// `.hud-agents-tooltip` clamps both bounds against `calc(100vw - 24px)`.
const AGENTS_MIN_WIDTH: f32 = 180.;
const AGENTS_MAX_WIDTH: f32 = 440.;
const VIEWPORT_GUTTER: f32 = 24.;
/// Match the HUD tooltip's 11px text, 1.45 line height, 11px inset and 1px border.
const AGENT_FONT_SIZE: f32 = 11.;
const AGENT_LINE_HEIGHT: f32 = 16.;
const AGENT_PANEL_PADDING_X: f32 = 11.;
const PANEL_HORIZONTAL_CHROME: f32 = 2. * (AGENT_PANEL_PADDING_X + 1.);
/// `.hud-agent`: 8px status column, 6px dot, 6px column gap.
const AGENT_DOT_COLUMN: f32 = 8.;
const AGENT_DOT_SIZE: f32 = 6.;
const AGENT_COLUMN_GAP: f32 = 6.;
const AGENT_DOT_TOP: f32 = (AGENT_LINE_HEIGHT - AGENT_DOT_SIZE) / 2.;
/// `.hud-agent-detail { grid-column: 2 / -1 }`: status column + column gap.
const AGENT_DETAIL_INDENT: f32 = AGENT_DOT_COLUMN + AGENT_COLUMN_GAP;
/// `.hud-agents-tooltip .hud-agent-list { gap: 6px }`; other slots keep 3pt.
const AGENT_LIST_GAP: f32 = 6.;

/// Optional `node["codexHud"]` payload. Every field stands alone; the model
/// falls back to `node["model"]` only when the payload object is absent.
pub(super) struct CodexHud {
    model: Option<SharedString>,
    full_model: Option<SharedString>,
    effort: Option<SharedString>,
    context_percent: Option<f64>,
    context_window: Option<f64>,
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
        Self {
            model,
            full_model,
            effort,
            context_percent,
            context_window,
        }
    }

    pub(super) fn model_label(&self, mode: &str) -> Option<SharedString> {
        if supports_details(mode) {
            self.model.clone()
        } else {
            self.format_model(self.model.as_ref()?)
        }
    }

    /// Keep the full model label, effort and context window in the tooltip.
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

/// Match the footer mockup's 11pt ring and 4pt gap to the percentage label.
pub(super) fn context_ring(
    percent: f64,
    tokens: Option<(u64, u64)>,
    palette: FooterPalette,
    dark: bool,
) -> Stateful<Div> {
    let label = format_context_percent(percent);
    let mut tooltip = format!("Context: {label}");
    if let Some((used, window)) = tokens {
        tooltip.push_str(&format!(
            " · {} / {}",
            footer_token_count(used),
            footer_token_count(window)
        ));
    }
    let tooltip = SharedString::from(tooltip);
    let (ring, text) = context_colors(percent, palette, dark);
    let track = rgb(if dark { 0x363636 } else { 0xdadce0 });
    div()
        .id("footer-context")
        .flex_none()
        .flex()
        .items_center()
        .gap(px(RING_GAP))
        .children(footer_bounds_probe("context"))
        .child(context_ring_canvas(percent, ring, track))
        .child(div().text_color(text).child(footer_text("context", label)))
        .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into())
}

fn context_ring_canvas(percent: f64, color: Rgba, track: Rgba) -> impl IntoElement {
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
                track,
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
    color: Rgba,
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
        window.paint_path(path, color);
    }
}

fn context_colors(percent: f64, palette: FooterPalette, dark: bool) -> (Rgba, Rgba) {
    if percent >= 68. {
        let danger = rgb(if dark { CONTEXT_RED } else { 0xcf222e });
        (danger, danger)
    } else {
        (
            rgba(palette.model),
            rgb(if dark { 0x8b949e } else { 0x57606a }),
        )
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AgentState {
    Other,
    Waiting,
    Running,
    Warning,
    Error,
}

impl AgentState {
    fn from_status(status: &str) -> Self {
        match status {
            "waiting" => Self::Waiting,
            "running" => Self::Running,
            "warning" => Self::Warning,
            "failed" | "error" => Self::Error,
            _ => Self::Other,
        }
    }

    fn badge_color(self, dark: bool) -> Rgba {
        match self {
            Self::Running => rgb(if dark { 0x79c0ff } else { 0x0969da }),
            Self::Warning => rgb(if dark { 0xd29922 } else { 0x9a6700 }),
            Self::Error => rgb(if dark { 0xf47067 } else { 0xcf222e }),
            Self::Waiting | Self::Other => rgb(AGENT_NEUTRAL_COLOR),
        }
    }

    fn dot(self, dark: bool) -> AgentDot {
        let color = match self {
            Self::Running if dark => rgb(0x4c8dff),
            Self::Waiting => Self::muted_color(dark),
            _ => self.badge_color(dark),
        };
        AgentDot {
            color,
            hollow: self == Self::Waiting,
        }
    }

    fn muted_color(dark: bool) -> Rgba {
        rgb(if dark { 0x8b949e } else { 0x57606a })
    }

    fn detail_color(self, dark: bool) -> Rgba {
        if self == Self::Error {
            self.badge_color(dark)
        } else {
            Self::muted_color(dark)
        }
    }
}

struct AgentDot {
    color: Rgba,
    hollow: bool,
}

#[derive(Clone)]
struct AgentRow {
    label: SharedString,
    detail: Option<SharedString>,
    state: AgentState,
    started_at: Option<f64>,
    duration_ms: Option<f64>,
}

impl AgentRow {
    fn from_projection(agent: &Value) -> Option<Self> {
        let label = clean_text(&agent["name"]).or_else(|| clean_text(&agent["label"]))?;
        let status = agent["status"]
            .as_str()
            .or_else(|| agent["state"].as_str())
            .unwrap_or_default();
        let state = AgentState::from_status(status);
        Some(Self {
            label,
            detail: clean_text(&agent["detail"]),
            state,
            started_at: agent["startedAt"]
                .as_f64()
                .filter(|_| state == AgentState::Running),
            duration_ms: agent["durationMs"].as_f64(),
        })
    }

    fn elapsed(&self, now: f64, locale: InterfaceLocale) -> Option<String> {
        self.started_at
            .map(|start| now - start)
            .or(self.duration_ms)
            .map(|ms| duration(ms, locale))
    }
}

#[derive(Clone)]
struct AgentsTooltip {
    title: &'static str,
    rows: Vec<AgentRow>,
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

impl Render for AgentsTooltip {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let tooltip_strong = popover_colors(self.dark).tooltip_strong;
        let title = div()
            .mb(px(5.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(tooltip_strong)
            .child(ui_text(self.title));
        let AgentTooltipLayout {
            width,
            duration_font,
            rows,
        } = agent_tooltip_layout(window, self.title, &self.rows, now_ms(), self.locale);
        let list = div()
            .id("footer-detail-rows")
            .flex()
            .flex_col()
            .gap(px(AGENT_LIST_GAP))
            .max_h(px(220.))
            .overflow_y_scroll()
            .when(self.rows.is_empty(), |list| {
                list.child(ui_text(self.locale.text("暂无明细", "Details pending")))
            })
            .children(
                rows.iter()
                    .map(|row| agent_tooltip_row(row, self.dark, tooltip_strong, &duration_font)),
            );
        card_tooltip_box(self.dark)
            .w(px(width))
            .px(px(AGENT_PANEL_PADDING_X))
            .py(px(9.))
            .rounded(px(8.))
            .text_size(px(AGENT_FONT_SIZE))
            .line_height(px(AGENT_LINE_HEIGHT))
            .child(title)
            .child(list)
    }
}

struct AgentTooltipLayout {
    width: f32,
    duration_font: Font,
    rows: Vec<AgentTooltipRow>,
}

struct AgentTooltipRow {
    first_line: SharedString,
    second_line: Option<SharedString>,
    duration: Option<SharedString>,
    detail: Option<SharedString>,
    state: AgentState,
    name_width: f32,
}

/// Measure the explicit Agents panel width from the real font, without relying
/// on the tooltip root's `AvailableSpace::min_size` pass. The chosen width is
/// then the only thing GPUI has to lay out.
fn agent_tooltip_layout(
    window: &Window,
    title: &'static str,
    agents: &[AgentRow],
    now: f64,
    locale: InterfaceLocale,
) -> AgentTooltipLayout {
    let base_style = tooltip_text_style(window);
    let title_style = TextStyle {
        font_weight: FontWeight::SEMIBOLD,
        ..base_style.clone()
    };
    let duration_style = tabular_text_style(&base_style);
    let duration_font = duration_style.font();

    let mut content_width = measure_tooltip_text(window, title, &title_style);
    let mut measured = Vec::with_capacity(agents.len());
    for row in agents {
        let label = normalized_agent_text(&row.label);
        let detail = row.detail.as_ref().map(normalized_agent_text);
        let duration = row.elapsed(now, locale).map(SharedString::from);
        let duration_width = duration.as_ref().map_or(0., |text| {
            measure_tooltip_text(window, text.as_ref(), &duration_style)
        });
        let label_width = measure_tooltip_text(window, label.as_ref(), &base_style);
        let detail_width = detail.as_ref().map_or(0., |detail| {
            measure_tooltip_text(window, detail.as_ref(), &base_style) + AGENT_DETAIL_INDENT
        });
        content_width = content_width
            .max(label_width + AGENT_DOT_COLUMN + AGENT_COLUMN_GAP * 2. + duration_width)
            .max(detail_width);
        measured.push((label, duration, duration_width, detail, row.state));
    }

    let viewport_limit = (f32::from(window.viewport_size().width) - VIEWPORT_GUTTER).max(0.);
    let min_width = AGENTS_MIN_WIDTH.min(viewport_limit);
    let max_width = AGENTS_MAX_WIDTH.min(viewport_limit).max(min_width);
    let width = (content_width + PANEL_HORIZONTAL_CHROME).clamp(min_width, max_width);
    let panel_content_width = (width - PANEL_HORIZONTAL_CHROME).max(0.);

    let rows = measured
        .into_iter()
        .map(|(label, duration, duration_width, detail, state)| {
            let name_width =
                (panel_content_width - AGENT_DOT_COLUMN - AGENT_COLUMN_GAP * 2. - duration_width)
                    .max(0.);
            let boundary =
                first_agent_wrap_boundary(window, label.as_ref(), &base_style, name_width);
            let (first_line, second_line) = split_agent_label(&label, boundary);
            AgentTooltipRow {
                first_line,
                second_line,
                duration,
                detail,
                state,
                name_width,
            }
        })
        .collect();

    AgentTooltipLayout {
        width,
        duration_font,
        rows,
    }
}

/// Shape the real inherited UI face at the Agents tooltip size. The row's
/// label/detail share the base style; the title is semibold and durations add
/// `tnum` on top of whatever feature set the theme font already carries.
fn tooltip_text_style(window: &Window) -> TextStyle {
    let mut style = window.text_style();
    style.font_size = px(AGENT_FONT_SIZE).into();
    style.line_height = px(AGENT_LINE_HEIGHT).into();
    style
}

fn tabular_text_style(base: &TextStyle) -> TextStyle {
    let mut features = base.font_features.tag_value_list().to_vec();
    features.retain(|(tag, _)| tag.as_str() != "tnum");
    features.push(("tnum".to_owned(), 1));
    TextStyle {
        font_features: FontFeatures(std::sync::Arc::new(features)),
        ..base.clone()
    }
}

fn measure_tooltip_text(window: &Window, text: &str, style: &TextStyle) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let line = window.text_system().shape_line(
        SharedString::from(text.to_owned()),
        px(AGENT_FONT_SIZE),
        &[style.to_run(text.len())],
        None,
    );
    // `UiText` rounds single-line and paragraph widths up to whole pixels.
    f32::from(line.width).ceil()
}

/// The first wrapped line ends before this glyph. `WrapBoundary` points at the
/// first overflowing glyph, whose `index` is a UTF-8 boundary in the source.
fn first_agent_wrap_boundary(
    window: &Window,
    text: &str,
    style: &TextStyle,
    name_width: f32,
) -> Option<usize> {
    if name_width <= 0. {
        return None;
    }
    let line = window
        .text_system()
        .shape_text(
            SharedString::from(text.to_owned()),
            px(AGENT_FONT_SIZE),
            &[style.to_run(text.len())],
            Some(px(name_width)),
            Some(2),
        )
        .ok()?
        .into_iter()
        .next()?;
    let boundary = *line.wrap_boundaries().first()?;
    let glyph = line
        .runs()
        .get(boundary.run_ix)?
        .glyphs
        .get(boundary.glyph_ix)?;
    Some(glyph.index)
}

/// Keep a stray `\r`/`\n`/`\t` in the projection from turning into a separate
/// visual line. This runs before measurement and wrapping, so glyph byte
/// indices remain indices into the same normalized string. The ASCII controls
/// are one byte each, so replacing them with a space keeps every offset stable.
fn normalized_agent_text(original: &SharedString) -> SharedString {
    let text = original.as_ref();
    if text
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n' | b'\t'))
    {
        SharedString::from(text.replace(&['\r', '\n', '\t'][..], " "))
    } else {
        original.clone()
    }
}

fn split_agent_label(
    label: &SharedString,
    boundary: Option<usize>,
) -> (SharedString, Option<SharedString>) {
    let text = label.as_ref();
    let Some(index) =
        boundary.filter(|index| *index > 0 && *index < text.len() && text.is_char_boundary(*index))
    else {
        return (label.clone(), None);
    };
    // GPUI keeps the separating space at the end of the first visual line.
    // It is invisible, so trim it for the nowrap UiText without shifting the
    // second line (whose start is the already-computed glyph boundary).
    let first = text[..index].trim_end_matches(char::is_whitespace);
    let second = &text[index..];
    (
        SharedString::from(first.to_owned()),
        (!second.is_empty()).then(|| SharedString::from(second.to_owned())),
    )
}

/// `.hud-agent`: the status column, the measured label width and the
/// non-wrapping duration share one aligned row; the detail hangs below
/// columns 2..-1. Both label lines are separate nowrap `UiText` runs so their
/// real widths come from the wrapped boundary instead of a 2x pre-truncation.
fn agent_tooltip_row(
    row: &AgentTooltipRow,
    dark: bool,
    label_color: Rgba,
    duration_font: &Font,
) -> impl IntoElement {
    let dot = row.state.dot(dark);
    let marker = div().size(px(AGENT_DOT_SIZE)).flex_none().rounded_full();
    let marker = if dot.hollow {
        marker.border_1().border_color(dot.color)
    } else {
        marker.bg(dot.color)
    };
    let label = div()
        .w(px(row.name_width))
        .min_w_0()
        .flex_none()
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(label_color)
                .child(ui_text(row.first_line.clone())),
        )
        .children(row.second_line.as_ref().map(|line| {
            div()
                .w_full()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(label_color)
                .child(ui_text(line.clone()))
        }));
    div()
        .flex_none()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(AGENT_COLUMN_GAP))
                .child(
                    div()
                        .w(px(AGENT_DOT_COLUMN))
                        .flex_none()
                        .pt(px(AGENT_DOT_TOP))
                        .child(marker),
                )
                .child(label)
                .child({
                    let duration = div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_right()
                        .text_color(AgentState::muted_color(dark))
                        .font(duration_font.clone());
                    match &row.duration {
                        Some(text) => duration.child(ui_text(text.clone())),
                        None => duration,
                    }
                }),
        )
        .children(row.detail.clone().map(|detail| {
            div()
                .pl(px(AGENT_DETAIL_INDENT))
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(row.state.detail_color(dark))
                .child(ui_text(detail))
        }))
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn strings(value: &Value) -> Vec<SharedString> {
    array(value).iter().filter_map(clean_text).collect()
}

pub(super) fn stats_title(locale: InterfaceLocale) -> &'static str {
    locale.text("会话统计", "Session stats")
}

#[derive(Debug, Default, PartialEq)]
pub(super) struct StatsSummary {
    pub(super) tools: Option<u64>,
    pub(super) hooks: Option<u64>,
    pub(super) mcp: Option<(u64, usize)>,
    pub(super) todos: Option<(usize, usize)>,
}

impl StatsSummary {
    pub(super) fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatsSection {
    Tools,
    Hooks,
    Mcps,
    Todos,
}

impl StatsSection {
    const fn id(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Hooks => "hooks",
            Self::Mcps => "mcps",
            Self::Todos => "todos",
        }
    }
}

fn stats_tools_hud<'a>(hud: &'a Value, node: &'a Value) -> &'a Value {
    if hud["runningTools"].is_array() || hud["toolCounts"].is_array() {
        hud
    } else {
        &node["codexHud"]
    }
}

pub(super) fn stats_summary(hud: &Value, node: &Value, _locale: InterfaceLocale) -> StatsSummary {
    let counts = &hud["configCounts"];
    let legacy_counts = &node["configCounts"];
    let tool_hud = stats_tools_hud(hud, node);
    let tool_total = || {
        let running = array(&tool_hud["runningTools"])
            .iter()
            .filter(|tool| clean_text(&tool["name"]).is_some())
            .count() as u64;
        let completed = array(&tool_hud["toolCounts"])
            .iter()
            .filter(|tool| clean_text(&tool["name"]).is_some())
            .filter_map(|tool| nonnegative_count(&tool["count"]))
            .reduce(u64::saturating_add);
        let recorded_empty = ["runningTools", "toolCounts"]
            .into_iter()
            .any(|key| tool_hud[key].as_array().is_some_and(Vec::is_empty));
        (running > 0 || completed.is_some() || recorded_empty)
            .then(|| running.saturating_add(completed.unwrap_or_default()))
    };
    let explicit_tool_count = hud["toolCount"].as_u64();
    let tools = explicit_tool_count
        .filter(|&total| total > 0)
        .or_else(tool_total)
        .or(explicit_tool_count);
    let hooks = counts["hooks"]
        .as_u64()
        .or_else(|| legacy_counts["hooks"].as_u64())
        .unwrap_or_default();
    let mcps = [counts, legacy_counts]
        .into_iter()
        .find_map(|counts| {
            counts["mcps"]
                .as_u64()
                .or_else(|| counts["mcpServers"].as_u64())
        })
        .unwrap_or_default();
    let errors = array(&hud["mcpErrors"])
        .iter()
        .filter(|value| clean_text(value).is_some())
        .count();
    let todos = array(&hud["todos"]);
    StatsSummary {
        tools,
        hooks: (hooks > 0).then_some(hooks),
        mcp: (mcps > 0 || errors > 0).then_some((mcps, errors)),
        todos: (!todos.is_empty()).then(|| {
            let done = todos
                .iter()
                .filter(|todo| todo["status"] == "completed")
                .count();
            (done, todos.len())
        }),
    }
}

fn call_count(count: u64, locale: InterfaceLocale) -> String {
    match locale {
        InterfaceLocale::Chinese => format!("{count} 次"),
        InterfaceLocale::English => {
            let unit = if count == 1 { "call" } else { "calls" };
            format!("{count} {unit}")
        }
    }
}

fn stats_detail_rows(
    section: StatsSection,
    hud: &Value,
    node: &Value,
    locale: InterfaceLocale,
) -> Vec<SharedString> {
    let config_names = |key| {
        [hud, &hud["configCounts"], &node["configCounts"]]
            .into_iter()
            .map(|source| strings(&source[key]))
            .find(|names| !names.is_empty())
            .unwrap_or_default()
    };
    match section {
        StatsSection::Tools => {
            let tools = stats_tools_hud(hud, node);
            let mut rows: Vec<_> = array(&tools["runningTools"])
                .iter()
                .filter_map(|tool| {
                    let name = clean_text(&tool["name"])?;
                    let running = locale.text("运行中", "Running");
                    let mut label = format!("{name} · {running}");
                    if let Some(target) = clean_text(&tool["target"]) {
                        label.push_str(" · ");
                        label.push_str(&target);
                    }
                    Some(SharedString::from(label))
                })
                .collect();
            let mut counts: Vec<_> = array(&tools["toolCounts"])
                .iter()
                .filter_map(|tool| {
                    let count = nonnegative_count(&tool["count"]).filter(|&count| count > 0)?;
                    Some((clean_text(&tool["name"])?, count))
                })
                .collect();
            counts.sort_by(|(a_name, a), (b_name, b)| b.cmp(a).then_with(|| a_name.cmp(b_name)));
            rows.extend(counts.into_iter().map(|(name, count)| {
                SharedString::from(format!("{name} · {}", call_count(count, locale)))
            }));
            rows
        }
        StatsSection::Hooks => config_names("hookNames"),
        StatsSection::Mcps => {
            let error_label = locale.text("错误", "Error");
            let mut rows: Vec<_> = strings(&hud["mcpErrors"])
                .into_iter()
                .map(|error| SharedString::from(format!("{error_label} · {error}")))
                .collect();
            rows.extend(config_names("mcpServerNames"));
            rows
        }
        StatsSection::Todos => array(&hud["todos"])
            .iter()
            .filter_map(|todo| {
                let marker = match todo["status"].as_str() {
                    Some("completed") => "✓",
                    Some("in_progress") => "▸",
                    _ => "○",
                };
                Some(SharedString::from(format!(
                    "{marker} {}",
                    clean_text(&todo["content"])?
                )))
            })
            .collect(),
    }
}

pub(super) fn stats_panel(
    summary: &StatsSummary,
    hud: &Value,
    node: &Value,
    expanded: Option<StatsSection>,
    locale: InterfaceLocale,
    dark: bool,
    mut button: impl FnMut(StatsSection, Stateful<Div>) -> Stateful<Div>,
) -> Div {
    let (background, border, muted, body) = if dark {
        (rgb(0x1b1d21), rgb(0x2d3035), rgb(0xa0a7b2), rgb(0xe7e9ec))
    } else {
        (rgb(0xffffff), rgba(0x0000001a), rgb(0x6b7280), rgb(0x1f2328))
    };
    let palette = popover_colors(dark);
    let numeric_features = FontFeatures(std::sync::Arc::new(vec![("tnum".to_owned(), 1)]));
    let sections = [
        (
            StatsSection::Tools,
            locale.text("工具调用", "Tool calls"),
            Some(summary.tools.map_or_else(
                || locale.text("暂无统计", "Unavailable").to_owned(),
                |count| call_count(count, locale),
            )),
            if summary.tools.is_some() { body } else { muted },
        ),
        (
            StatsSection::Hooks,
            "Hooks",
            summary.hooks.map(|count| count.to_string()),
            body,
        ),
        (
            StatsSection::Mcps,
            "MCP",
            summary.mcp.map(|(count, errors)| {
                format!("{count} · {errors} {}", locale.text("错误", "errors"))
            }),
            if summary.mcp.is_some_and(|(_, errors)| errors > 0) {
                AgentState::Error.badge_color(dark)
            } else {
                body
            },
        ),
        (
            StatsSection::Todos,
            locale.text("待办", "Todos"),
            summary
                .todos
                .map(|(done, total)| format!("{done} / {total}")),
            body,
        ),
    ];
    let rows = sections
        .into_iter()
        .filter_map(|(section, label, value, color)| {
            let value = value?;
            let id = section.id();
            let details = stats_detail_rows(section, hud, node, locale);
            let expandable = !details.is_empty();
            let is_expanded = expandable && expanded == Some(section);
            let mut number = div()
                .whitespace_nowrap()
                .text_color(color)
                .child(footer_text(id, value));
            number
                .text_style()
                .get_or_insert_with(Default::default)
                .font_features = Some(numeric_features.clone());
            let heading = div()
                .id(id)
                .min_w_0()
                .flex()
                .items_center()
                .min_h(px(28.))
                .px(px(6.))
                .rounded(px(4.))
                .gap(px(10.))
                .text_size(px(12.))
                .line_height(px(19.2))
                .children(footer_bounds_probe(id))
                .child(
                    div()
                        .size(px(12.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(muted)
                        .when(expandable, |arrow| {
                            let path = if is_expanded {
                                "icons/chevron-down.svg"
                            } else {
                                "icons/chevron-right.svg"
                            };
                            arrow.child(svg().path(path).size(px(12.)).text_color(muted))
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_color(body)
                        .child(ui_text(label)),
                )
                .child(number);
            let heading = if expandable {
                button(
                    section,
                    heading
                        .cursor_pointer()
                        .hover(move |style| style.bg(palette.hover)),
                )
            } else {
                heading
            };
            Some(div().child(heading).when(is_expanded, |row| {
                row.child(
                    div()
                        .id(("stats-details", section as usize))
                        .children(footer_bounds_probe("stats-details"))
                        .max_h(px(174.))
                        .overflow_y_scroll()
                        .pl(px(28.))
                        .pr(px(6.))
                        .py(px(3.))
                        .text_size(px(11.))
                        .line_height(px(17.))
                        .text_color(muted)
                        .children(details.into_iter().enumerate().map(|(index, detail)| {
                            div().py(px(2.)).child(if index == 0 {
                                footer_text("stats-first-detail", detail)
                            } else {
                                ui_text(detail)
                            })
                        })),
                )
            }))
        });
    div()
        .w(px(360.))
        .max_w_full()
        .px(px(14.))
        .py(px(12.))
        .rounded(px(10.))
        .border_1()
        .border_color(border)
        .bg(background)
        .shadow(vec![if dark {
            shadow(0x000000, 0.55, 18., 44.)
        } else {
            popover_colors(false).tooltip_shadow()
        }])
        .flex()
        .flex_col()
        .child(
            div()
                .mb(px(9.))
                .text_size(px(10.5))
                .line_height(px(16.8))
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(footer_text("stats-title", stats_title(locale))),
        )
        .children(rows)
}

fn agents_tooltip(
    hud: &Value,
    locale: InterfaceLocale,
    dark: bool,
) -> Option<(String, Rgba, AgentsTooltip)> {
    let agents = hud["agents"].as_array();
    let rows: Vec<_> = if let Some(agents) = agents {
        agents
            .iter()
            .filter_map(AgentRow::from_projection)
            .collect()
    } else {
        strings(&hud["subagents"])
            .into_iter()
            .map(|label| AgentRow {
                label,
                detail: None,
                state: AgentState::Running,
                started_at: None,
                duration_ms: None,
            })
            .collect()
    };
    let count = if let Some(agents) = agents {
        agents.len() as u64
    } else {
        hud["subagentCount"].as_u64().unwrap_or(rows.len() as u64)
    };
    if count == 0 {
        return None;
    }
    let state = rows
        .iter()
        .map(|row| row.state)
        .max()
        .unwrap_or(AgentState::Other);
    let warning = if state >= AgentState::Warning {
        "⚠ "
    } else {
        ""
    };
    let noun = if count == 1 { "Agent" } else { "Agents" };
    Some((
        format!("{warning}{count} {noun}"),
        state.badge_color(dark),
        AgentsTooltip {
            title: locale.text("子代理", "Agents"),
            rows,
            locale,
            dark,
        },
    ))
}

pub(super) fn agents_slot(
    hud: &Value,
    locale: InterfaceLocale,
    dark: bool,
) -> Option<Stateful<Div>> {
    let (label, color, tooltip) = agents_tooltip(hud, locale, dark)?;
    let has_live_duration = tooltip.rows.iter().any(|row| row.started_at.is_some());
    Some(
        div()
            .id("footer-agents")
            .flex_shrink_0()
            .min_w_0()
            .max_w(px(108.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .children(footer_bounds_probe("agents"))
            .px(px(3.))
            .py(px(1.))
            .rounded(px(4.))
            .text_color(color)
            .flex()
            .items_center()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(footer_text("agents", label)),
            )
            .hover(move |style| {
                if dark {
                    style.bg(rgba(0xffffff14))
                } else {
                    let style = style.bg(rgba(0x1f28370f));
                    if color == rgb(AGENT_NEUTRAL_COLOR) {
                        style.text_color(rgb(0x57606a))
                    } else {
                        style
                    }
                }
            })
            .hoverable_tooltip(move |_, cx| {
                cx.new(|cx: &mut Context<AgentsTooltip>| {
                    if has_live_duration {
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
            }),
    )
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

pub(super) fn footer_usage_windows(usages: &[&UsageWindow]) -> Vec<usize> {
    let preferred = ["5h", "Weekly"].into_iter().filter_map(|label| {
        usages
            .iter()
            .position(|usage| usage.label.eq_ignore_ascii_case(label))
    });
    let mut selected = preferred.collect::<Vec<_>>();
    for index in 0..usages.len() {
        if selected.len() == 2 {
            break;
        }
        if !selected.contains(&index) {
            selected.push(index);
        }
    }
    selected
}

pub(super) fn footer_quota_row(
    usage: &UsageWindow,
    palette: FooterPalette,
    key: &'static str,
    percent_key: &'static str,
) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(3.))
        .whitespace_nowrap()
        .children(footer_bounds_probe(key))
        .child(footer_text(key, usage.label.clone()))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgba(palette.usage_accent))
                .child(footer_text(
                    percent_key,
                    format_context_percent(usage.percent),
                )),
        )
}

pub(super) fn quota_row(usage: &UsageWindow, dark: bool) -> Div {
    quota_row_with_colors(
        usage,
        usage.color(dark),
        rgb(if dark { 0x8b949e } else { 0x57606a }),
    )
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
            "model": "Claude Sonnet", "contextUsedTokens": 136_000, "contextWindowSize": 200_000,
            "contextGuard": {"cache": {"hitRatioPercent": 91.0}},
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
            for locale in [InterfaceLocale::English, InterfaceLocale::Chinese] {
                assert_eq!(
                    stats_summary(&hud, &Value::Null, locale),
                    StatsSummary {
                        tools: Some(3),
                        hooks: Some(1),
                        mcp: Some((2, 2)),
                        todos: Some((1, 2)),
                    }
                );
            }
            let (label, color, tooltip) =
                agents_tooltip(&hud, InterfaceLocale::English, true).unwrap();
            assert_eq!(label, "⚠ 4 Agents");
            assert_eq!(color, rgb(0xf47067));
            let rows = &tooltip.rows;
            assert_eq!(rows[0].label.as_ref(), "Reader");
            assert_eq!(
                rows[0].detail.as_ref().map(|detail| detail.as_ref()),
                Some("Indexing")
            );
            assert_eq!(
                rows[0]
                    .elapsed(1_700_000_002_000., InterfaceLocale::English)
                    .as_deref(),
                Some("2s"),
            );
            assert_eq!(
                rows[1]
                    .elapsed(1_700_000_002_000., InterfaceLocale::English)
                    .as_deref(),
                Some("1m 5s"),
            );
        }
    }

    #[::core::prelude::v1::test]
    fn footer_stats_counts_legacy_tools_and_mcp_servers_without_errors() {
        let node = json!({
            "codexHud": {
                "runningTools": [{"name":"Read"}, {"name":" "}],
                "toolCounts": [{"name":"Find","count":2}, {"name":"Invalid","count":-1}]
            },
            "configCounts": {"mcpServers":4, "hooks":11}
        });
        let locale = InterfaceLocale::English;
        assert_eq!(
            stats_summary(&json!({"toolCount":0}), &node, locale),
            StatsSummary {
                tools: Some(3),
                hooks: Some(11),
                mcp: Some((4, 0)),
                ..StatsSummary::default()
            }
        );
        let hud = json!({
            "toolCount":8294,
            "toolCounts":[{"name":"Find","count":8}],
            "mcpErrors":["", "server offline", null],
            "contextWindowSize":200_000
        });
        assert_eq!(
            stats_summary(&hud, &Value::Null, locale),
            StatsSummary {
                tools: Some(8294),
                mcp: Some((0, 1)),
                ..StatsSummary::default()
            }
        );
        assert!(
            stats_summary(
                &json!({"runningTools":[{}],"agents":[{"name":"Reader"}],"contextPercent":34}),
                &Value::Null,
                locale,
            )
            .is_empty()
        );
        assert_eq!(
            stats_summary(&json!({"toolCounts": []}), &Value::Null, locale).tools,
            Some(0)
        );
        assert_eq!(
            stats_summary(&json!({"toolCount": 0}), &Value::Null, locale).tools,
            Some(0)
        );
        assert_eq!(
            stats_summary(&json!({"configCounts": {"hooks": 1}}), &Value::Null, locale).tools,
            None
        );
    }

    #[::core::prelude::v1::test]
    fn agent_rows_preserve_long_labels_and_resolve_state_colors_by_theme() {
        let name = "x".repeat(80);
        let row = AgentRow::from_projection(&json!({
            "label": name,
            "state": "waiting",
            "detail": "Waiting for a reply",
            "startedAt": 1000,
        }))
        .unwrap();
        assert_eq!(row.label.as_ref(), name);
        assert_eq!(
            row.detail.as_ref().map(|detail| detail.as_ref()),
            Some("Waiting for a reply")
        );
        assert_eq!(row.elapsed(2000., InterfaceLocale::English), None);
        for (dark, muted, blue, warning, error) in [
            (true, 0x8b949e, 0x4c8dff, 0xd29922, 0xf47067),
            (false, 0x57606a, 0x0969da, 0x9a6700, 0xcf222e),
        ] {
            let waiting = row.state.dot(dark);
            assert!(waiting.hollow);
            assert_eq!(waiting.color, rgb(muted));
            assert_eq!(row.state.detail_color(dark), rgb(muted));
            for (state, color) in [
                (AgentState::Running, blue),
                (AgentState::Warning, warning),
                (AgentState::Error, error),
            ] {
                let dot = state.dot(dark);
                assert!(!dot.hollow);
                assert_eq!(dot.color, rgb(color));
            }
            assert_eq!(AgentState::Error.detail_color(dark), rgb(error));
        }
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
    fn footer_prefers_the_subscription_windows_and_keeps_real_partial_usage() {
        for (labels, expected) in [
            (vec!["Extra", "wEeKlY", "5H", "Other"], vec!["5H", "wEeKlY"]),
            (vec!["First", "Weekly", "Other"], vec!["Weekly", "First"]),
            (vec!["First", "Second", "Third"], vec!["First", "Second"]),
            (vec!["5h"], vec!["5h"]),
            (vec![], vec![]),
        ] {
            let usages: Vec<_> = labels
                .into_iter()
                .map(|label| UsageWindow {
                    label: label.into(),
                    percent: 0.,
                    reset: None,
                })
                .collect();
            let refs: Vec<_> = usages.iter().collect();
            let selected = footer_usage_windows(&refs);
            assert_eq!(
                selected
                    .into_iter()
                    .map(|index| usages[index].label.as_ref())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[::core::prelude::v1::test]
    fn completed_todos_and_legacy_agent_states_use_the_same_pipeline() {
        let hud = json!({
            "agents": [{"label":"Legacy worker","state":"error","detail":"old shape"}],
            "todos": [{"content":"Test","status":"completed"}]
        });
        let locale = InterfaceLocale::Chinese;
        let (_, _, tooltip) = agents_tooltip(&hud, locale, false).unwrap();
        let rows = &tooltip.rows;
        assert_eq!(rows[0].label.as_ref(), "Legacy worker");
        assert_eq!(
            rows[0].detail.as_ref().map(|detail| detail.as_ref()),
            Some("old shape")
        );
        assert_eq!(rows[0].state.detail_color(false), rgb(0xcf222e));
        assert_eq!(
            stats_summary(&hud, &Value::Null, locale).todos,
            Some((1, 1))
        );
    }
}
