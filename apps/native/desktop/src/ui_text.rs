use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::{prelude::*, *};
#[cfg(target_os = "macos")]
use unicode_segmentation::UnicodeSegmentation;

#[derive(IntoElement)]
/// Match the frozen Mac UI font cascade without CoreText's automatic CJK scaling.
pub struct UiText {
    text: SharedString,
    #[cfg(target_os = "macos")]
    normal_line_height: bool,
    #[cfg(target_os = "macos")]
    pre_line: bool,
    geometry: Option<TextGeometryHandle>,
}

pub fn ui_text(text: impl Into<SharedString>) -> UiText {
    UiText {
        text: text.into(),
        #[cfg(target_os = "macos")]
        normal_line_height: false,
        #[cfg(target_os = "macos")]
        pre_line: false,
        geometry: None,
    }
}

impl UiText {
    pub(crate) fn track_geometry(mut self, geometry: TextGeometryHandle) -> Self {
        self.geometry = Some(geometry);
        self
    }

    /// Use the font-dependent line height for a Mac single-line label.
    pub fn normal_line_height(self) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                normal_line_height: true,
                ..self
            }
        }
        #[cfg(not(target_os = "macos"))]
        self
    }

    /// Apply the confirmation dialog's pre-line whitespace and baseline rules on Mac.
    pub fn pre_line(self) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                pre_line: true,
                ..self
            }
        }
        #[cfg(not(target_os = "macos"))]
        self
    }
}

impl RenderOnce for UiText {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            let fonts = UiFonts::new(window);
            if self.pre_line {
                let text = normalize_pre_line(&self.text);
                let runs = fonts.runs(&text, window);
                return PreLineText(Rc::new(Paragraph { text, fonts, runs })).into_any();
            }
            let runs = fonts.runs(&self.text, window);
            if (fonts.style.white_space == WhiteSpace::Nowrap || fonts.style.line_clamp == Some(1))
                && !self.text.contains('\n')
            {
                return SingleLineText {
                    label: Rc::new(SingleLineLabel {
                        text: self.text,
                        fonts,
                        runs,
                        normal_line_height: self.normal_line_height,
                        shaped_full: RefCell::new(None),
                    }),
                    geometry: self.geometry,
                }
                .into_any();
            }
            into_text_element(StyledText::new(self.text).with_runs(runs), self.geometry)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = window;
            into_text_element(StyledText::new(self.text), self.geometry)
        }
    }
}

pub(crate) type TextGeometryHandle = Rc<RefCell<Option<TextGeometry>>>;

pub(crate) struct TextGeometry {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) layout: Arc<LineLayout>,
    pub(crate) wrapped: Option<Arc<WrappedLineLayout>>,
    pub(crate) line_height: Pixels,
}

pub(crate) fn into_text_element(
    text: StyledText,
    geometry: Option<TextGeometryHandle>,
) -> AnyElement {
    match geometry {
        Some(geometry) => TrackedStyledText { text, geometry }.into_any_element(),
        None => text.into_any_element(),
    }
}

struct TrackedStyledText {
    text: StyledText,
    geometry: TextGeometryHandle,
}

impl IntoElement for TrackedStyledText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TrackedStyledText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.text.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .prepaint(id, inspector_id, bounds, state, window, cx);
        let geometry = self
            .text
            .layout()
            .line_layout_for_index(0)
            .map(|line| TextGeometry {
                bounds,
                layout: line.unwrapped_layout.clone(),
                wrapped: (!line.wrap_boundaries().is_empty()).then_some(line),
                line_height: self.text.layout().line_height(),
            });
        self.geometry.replace(geometry);
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .paint(id, inspector_id, bounds, state, prepaint, window, cx);
    }
}

#[cfg(target_os = "macos")]
struct UiFonts {
    style: TextStyle,
    size: Pixels,
    primary: Font,
    fallback: Font,
    primary_id: FontId,
    fallback_id: FontId,
}

#[cfg(target_os = "macos")]
impl UiFonts {
    fn new(window: &Window) -> Self {
        let style = window.text_style();
        let mut primary = style.font();
        // font-kit maps Apple's Medium trait (0.23) to 530, so requesting CSS
        // 500 otherwise selects Regular. Match the frozen browser's Medium face.
        if primary.weight == FontWeight::MEDIUM {
            primary.weight = FontWeight(530.);
        }
        let fallback = Font {
            family: "PingFang SC".into(),
            ..primary.clone()
        };
        let system = window.text_system();
        Self {
            size: style.font_size.to_pixels(window.rem_size()),
            style,
            primary_id: system.resolve_font(&primary),
            fallback_id: system.resolve_font(&fallback),
            primary,
            fallback,
        }
    }

    fn runs(&self, text: &str, window: &Window) -> Vec<TextRun> {
        let system = window.text_system();
        let mut runs: Vec<TextRun> = Vec::new();
        for grapheme in text.graphemes(true) {
            let missing = grapheme.chars().any(|ch| {
                !ch.is_control() && system.advance(self.primary_id, self.size, ch).is_err()
            });
            let covered = missing
                && grapheme.chars().all(|ch| {
                    ch.is_control() || system.advance(self.fallback_id, self.size, ch).is_ok()
                });
            let face = if covered {
                &self.fallback
            } else {
                &self.primary
            };
            if let Some(last) = runs.last_mut().filter(|run| &run.font == face) {
                last.len += grapheme.len();
            } else {
                let mut run = self.style.to_run(grapheme.len());
                run.font = face.clone();
                runs.push(run);
            }
        }
        runs
    }

    fn line_height(&self, window: &Window) -> Pixels {
        self.style
            .line_height
            .to_pixels(self.size.into(), window.rem_size())
    }

    fn rounded_metrics(&self, id: FontId, window: &Window) -> (Pixels, Pixels) {
        let scale = window.scale_factor();
        let system = window.text_system();
        let ascent = (system.ascent(id, self.size) * scale).round();
        let descent = px((f32::from(system.descent(id, self.size)).abs() * scale).round());
        (ascent, descent)
    }
}

#[cfg(target_os = "macos")]
struct SingleLineLabel {
    text: SharedString,
    fonts: UiFonts,
    runs: Vec<TextRun>,
    normal_line_height: bool,
    shaped_full: RefCell<Option<(f32, Option<WrappedLine>, LineMetrics)>>,
}

#[cfg(target_os = "macos")]
impl SingleLineLabel {
    fn shape(&self, text: SharedString, runs: &[TextRun], window: &Window) -> Option<WrappedLine> {
        window
            .text_system()
            .shape_text(text, self.fonts.size, runs, None, None)
            .inspect_err(|error| eprintln!("Shape UI title: {error:#}"))
            .ok()?
            .into_iter()
            .next()
    }

    fn fit(
        &self,
        full: WrappedLine,
        width: Option<Pixels>,
        mut shape_candidate: impl FnMut(SharedString) -> Option<WrappedLine>,
    ) -> Option<WrappedLine> {
        let (Some(width), Some(TextOverflow::Truncate(suffix))) = (
            width.filter(|width| full.unwrapped_layout.width > *width),
            &self.fonts.style.text_overflow,
        ) else {
            return Some(full);
        };

        let mut best = None;
        let boundaries: Vec<_> = self
            .text
            .grapheme_indices(true)
            .map(|(start, _)| start)
            .collect();
        let mut start = 0;
        let mut end = boundaries.len();
        // GPUI's stock truncation measures each scalar in the primary font. Measure
        // whole candidates with the actual font runs so the suffix also fits.
        while start < end {
            let mid = start + (end - start) / 2;
            let text: SharedString = format!("{}{suffix}", &self.text[..boundaries[mid]]).into();
            let candidate = shape_candidate(text)?;
            if candidate.unwrapped_layout.width <= width {
                best = Some(candidate);
                start = mid + 1;
            } else {
                end = mid;
            }
        }
        best
    }

    fn measure(&self, limit: Option<Pixels>, window: &Window) -> MeasuredLine {
        self.measure_shaped(
            limit,
            window.scale_factor(),
            || {
                let full = self.shape(self.text.clone(), &self.runs, window);
                // Compute normal line height before truncation so every font still contributes.
                let metrics = self.metrics(full.as_ref(), window);
                (full, metrics)
            },
            |text| {
                let runs = self.fonts.runs(&text, window);
                self.shape(text, &runs, window)
            },
        )
    }

    fn measure_shaped(
        &self,
        limit: Option<Pixels>,
        scale: f32,
        shape_full: impl FnOnce() -> (Option<WrappedLine>, LineMetrics),
        shape_candidate: impl FnMut(SharedString) -> Option<WrappedLine>,
    ) -> MeasuredLine {
        let cached = self
            .shaped_full
            .borrow()
            .as_ref()
            .filter(|(cached_scale, _, _)| *cached_scale == scale)
            .cloned();
        let (_, full, metrics) = cached.unwrap_or_else(|| {
            let (full, metrics) = shape_full();
            let shaped = (scale, full, metrics);
            self.shaped_full.replace(Some(shaped.clone()));
            shaped
        });
        let line = full.and_then(|line| self.fit(line, limit, shape_candidate));
        MeasuredLine {
            limit,
            line,
            metrics,
        }
    }

    fn metrics(&self, line: Option<&WrappedLine>, window: &Window) -> LineMetrics {
        let scale = window.scale_factor();
        let (mut ascent, mut descent) = self.fonts.rounded_metrics(self.fonts.primary_id, window);
        let height = if self.normal_line_height {
            if let Some(line) = line {
                for run in &line.unwrapped_layout.runs {
                    let (run_ascent, run_descent) = self.fonts.rounded_metrics(run.font_id, window);
                    ascent = ascent.max(run_ascent);
                    descent = descent.max(run_descent);
                }
            }
            // The fixed Mac UI cascade (SF and PingFang SC) has zero font line gap.
            (ascent + descent) / scale
        } else {
            self.fonts.line_height(window)
        };
        LineMetrics::new(height, ascent, descent, scale)
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct LineMetrics {
    height: Pixels,
    baseline_from_top: Pixels,
    scale: f32,
}

#[cfg(target_os = "macos")]
impl LineMetrics {
    fn new(height: Pixels, ascent: Pixels, descent: Pixels, scale: f32) -> Self {
        // The reference lays out text at device scale and floors the upper half-leading.
        let upper_leading = ((height * scale - ascent - descent) / 2.).floor();
        Self {
            height,
            baseline_from_top: ascent + upper_leading,
            scale,
        }
    }

    fn baseline_at(&self, y: Pixels) -> Pixels {
        ((y * self.scale).round() + self.baseline_from_top) / self.scale
    }

    fn text_origin(&self, origin: Point<Pixels>, line: &WrappedLine) -> Point<Pixels> {
        let layout = &line.unwrapped_layout;
        // Compensate for the baseline added by WrappedLine::paint.
        let paint_baseline = (self.height - layout.ascent - layout.descent) / 2. + layout.ascent;
        point(origin.x, self.baseline_at(origin.y) - paint_baseline)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{LineMetrics, SingleLineLabel, UiFonts};
    use gpui::{
        FontId, LineLayout, SharedString, TextOverflow, TextStyle, WrappedLine, WrappedLineLayout,
        font, px,
    };
    use std::{cell::RefCell, sync::Arc};
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn multi_limit_probes_reuse_full_shape_and_keep_ellipsis_and_metrics() {
        fn shape(text: SharedString) -> WrappedLine {
            let mut line = WrappedLine::default();
            *line = Arc::new(WrappedLineLayout {
                unwrapped_layout: Arc::new(LineLayout {
                    width: px(10. * text.graphemes(true).count() as f32),
                    len: text.len(),
                    ..Default::default()
                }),
                ..Default::default()
            });
            line.text = text;
            line
        }

        let text: SharedString = "Ae\u{301}界B".into();
        let label = SingleLineLabel {
            text: text.clone(),
            fonts: UiFonts {
                style: TextStyle {
                    text_overflow: Some(TextOverflow::Truncate("…".into())),
                    ..Default::default()
                },
                size: px(12.),
                primary: font(".SystemUIFont"),
                fallback: font("PingFang SC"),
                primary_id: FontId(0),
                fallback_id: FontId(1),
            },
            runs: Vec::new(),
            normal_line_height: true,
            shaped_full: RefCell::new(None),
        };
        let mut full_shapes = 0;
        let mut candidate_shapes = 0;
        for (limit, expected, needs_candidates) in [
            (None, Some(text.as_ref()), false),
            (Some(px(50.)), Some(text.as_ref()), false),
            (Some(px(40.)), Some(text.as_ref()), false),
            (Some(px(30.)), Some("Ae\u{301}…"), true),
            (Some(px(10.)), Some("…"), true),
            (Some(px(9.)), None, true),
            (None, Some(text.as_ref()), false),
        ] {
            let previous_candidates = candidate_shapes;
            let measured = label.measure_shaped(
                limit,
                2.,
                || {
                    full_shapes += 1;
                    (
                        Some(shape(text.clone())),
                        LineMetrics::new(px(16.), px(24.), px(6.), 2.),
                    )
                },
                |candidate| {
                    candidate_shapes += 1;
                    Some(shape(candidate))
                },
            );
            assert_eq!(
                measured.line.as_ref().map(|line| line.text.as_ref()),
                expected
            );
            assert_eq!(measured.metrics.height, px(16.));
            assert_eq!(measured.metrics.baseline_at(px(0.)), px(12.5));
            assert_eq!(full_shapes, 1);
            assert_eq!(candidate_shapes > previous_candidates, needs_candidates);
        }

        let measured = label.measure_shaped(
            None,
            1.,
            || {
                (
                    Some(shape(text.clone())),
                    LineMetrics::new(px(15.), px(12.), px(3.), 1.),
                )
            },
            |_| unreachable!("unconstrained text should not shape a truncation candidate"),
        );
        assert_eq!(measured.metrics.height, px(15.));
        assert_eq!(measured.metrics.baseline_at(px(0.)), px(12.));
    }
}

#[cfg(target_os = "macos")]
struct MeasuredLine {
    limit: Option<Pixels>,
    line: Option<WrappedLine>,
    metrics: LineMetrics,
}

#[cfg(target_os = "macos")]
struct SingleLineText {
    label: Rc<SingleLineLabel>,
    geometry: Option<TextGeometryHandle>,
}

#[cfg(target_os = "macos")]
impl IntoElement for SingleLineText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

#[cfg(target_os = "macos")]
impl Element for SingleLineText {
    type RequestLayoutState = Rc<RefCell<Option<MeasuredLine>>>;
    type PrepaintState = MeasuredLine;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let measured = Rc::new(RefCell::new(None));
        let layout_id = window.request_measured_layout(Style::default(), {
            let measured = measured.clone();
            let label = self.label.clone();
            move |known, available, window, _| {
                let limit = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let result = label.measure(limit, window);
                let width = result
                    .line
                    .as_ref()
                    .map_or(px(0.), |line| line.unwrapped_layout.width.ceil());
                let height = result.metrics.height;
                measured.replace(Some(result));
                size(limit.map_or(width, |limit| width.min(limit)), height)
            }
        });
        (layout_id, measured)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        measured: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        let width = bounds.size.width;
        let result = measured
            .take()
            .filter(|result| result.limit == Some(width))
            .unwrap_or_else(|| self.label.measure(Some(width), window));
        if let Some(geometry) = &self.geometry {
            geometry.replace(result.line.as_ref().map(|line| TextGeometry {
                bounds,
                layout: line.unwrapped_layout.clone(),
                wrapped: None,
                line_height: result.metrics.height,
            }));
        }
        result
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        measured: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(line) = &measured.line {
            let align = self.label.fonts.style.text_align;
            let metrics = &measured.metrics;
            if let Err(error) = line.paint_background(
                bounds.origin,
                metrics.height,
                align,
                Some(bounds),
                window,
                cx,
            ) {
                eprintln!("Paint UI title background: {error:#}");
            }
            let origin = metrics.text_origin(bounds.origin, line);
            if let Err(error) = line.paint(origin, metrics.height, align, Some(bounds), window, cx)
            {
                eprintln!("Paint UI title: {error:#}");
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn normalize_pre_line(text: &str) -> SharedString {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut has_content = false;
    let mut pending_space = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\r' | '\n' => {
                if ch == '\r' {
                    chars.next_if_eq(&'\n');
                }
                result.push('\n');
                has_content = false;
                pending_space = false;
            }
            ' ' | '\t' | '\u{000c}' => pending_space = has_content,
            _ => {
                if pending_space {
                    result.push(' ');
                    pending_space = false;
                }
                result.push(ch);
                has_content = true;
            }
        }
    }
    if result.ends_with('\n') {
        result.pop();
    }
    result.into()
}

#[cfg(target_os = "macos")]
struct Paragraph {
    text: SharedString,
    fonts: UiFonts,
    runs: Vec<TextRun>,
}

#[cfg(target_os = "macos")]
impl Paragraph {
    fn layout(&self, width: Option<Pixels>, window: &Window) -> ParagraphLayout {
        let scale = window.scale_factor();
        // Blink stores line heights in 1/64 device-pixel layout units.
        let units_per_pixel = scale * 64.;
        let height = (self.fonts.line_height(window) * units_per_pixel).floor() / units_per_pixel;
        let (ascent, descent) = self.fonts.rounded_metrics(self.fonts.primary_id, window);
        let metrics = LineMetrics::new(height, ascent, descent, scale);
        let lines = window
            .text_system()
            .shape_text(self.text.clone(), self.fonts.size, &self.runs, width, None)
            .inspect_err(|error| eprintln!("Shape confirmation text: {error:#}"))
            .map(|lines| lines.into_vec())
            .unwrap_or_default();
        ParagraphLayout {
            width,
            metrics,
            lines,
        }
    }
}

#[cfg(target_os = "macos")]
struct ParagraphLayout {
    width: Option<Pixels>,
    metrics: LineMetrics,
    lines: Vec<WrappedLine>,
}

#[cfg(target_os = "macos")]
impl ParagraphLayout {
    fn size(&self) -> Size<Pixels> {
        let mut result = Size::<Pixels>::default();
        for line in &self.lines {
            let line_size = line.size(self.metrics.height);
            result.width = result.width.max(line_size.width).ceil();
            result.height += line_size.height;
        }
        result
    }

    fn visual_lines(
        &self,
        bounds: Bounds<Pixels>,
        align: TextAlign,
    ) -> impl Iterator<Item = ParagraphLine<'_>> {
        self.lines
            .iter()
            .flat_map(|source| {
                let edges = source.wrap_boundaries.iter().copied().map(Some);
                std::iter::once(None)
                    .chain(edges.clone())
                    .zip(edges.chain(std::iter::once(None)))
                    .map(move |(start, end)| (source, start, end))
            })
            .enumerate()
            .map(move |(ix, (source, start, end))| {
                let layout = &source.unwrapped_layout;
                let boundary_x =
                    |edge: WrapBoundary| layout.runs[edge.run_ix].glyphs[edge.glyph_ix].position.x;
                let start_x = start.map_or(Pixels::ZERO, boundary_x);
                let width = end.map_or(layout.width, boundary_x) - start_x;
                let extra_width = bounds.size.width - width;
                let align_offset = match align {
                    TextAlign::Left => Pixels::ZERO,
                    TextAlign::Center => extra_width / 2.,
                    TextAlign::Right => extra_width,
                };
                let top = bounds.origin.y + self.metrics.height * ix;
                let baseline = self.metrics.baseline_at(top);
                ParagraphLine {
                    source,
                    start,
                    end,
                    start_x,
                    origin: point(bounds.origin.x + align_offset, baseline),
                }
            })
    }
}

#[cfg(target_os = "macos")]
struct ParagraphLine<'a> {
    source: &'a WrappedLine,
    start: Option<WrapBoundary>,
    end: Option<WrapBoundary>,
    start_x: Pixels,
    origin: Point<Pixels>,
}

#[cfg(target_os = "macos")]
impl ParagraphLine<'_> {
    fn paint(&self, color: Hsla, window: &mut Window) -> anyhow::Result<()> {
        let layout = &self.source.unwrapped_layout;
        let first_run = self.start.map_or(0, |edge| edge.run_ix);
        let end_run = self.end.map_or(layout.runs.len(), |edge| edge.run_ix + 1);
        for (run_ix, run) in layout.runs.iter().enumerate().take(end_run).skip(first_run) {
            let first_glyph = self
                .start
                .filter(|edge| edge.run_ix == run_ix)
                .map_or(0, |edge| edge.glyph_ix);
            let end_glyph = self
                .end
                .filter(|edge| edge.run_ix == run_ix)
                .map_or(run.glyphs.len(), |edge| edge.glyph_ix);
            for glyph in &run.glyphs[first_glyph..end_glyph] {
                let origin = point(
                    self.origin.x + glyph.position.x - self.start_x,
                    self.origin.y,
                );
                if glyph.is_emoji {
                    window.paint_emoji(origin, run.font_id, glyph.id, layout.font_size)?;
                } else {
                    window.paint_glyph(origin, run.font_id, glyph.id, layout.font_size, color)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
struct PreLineText(Rc<Paragraph>);

#[cfg(target_os = "macos")]
impl IntoElement for PreLineText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

#[cfg(target_os = "macos")]
impl Element for PreLineText {
    type RequestLayoutState = Rc<RefCell<Option<ParagraphLayout>>>;
    type PrepaintState = ParagraphLayout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let measured = Rc::new(RefCell::new(None));
        let id = window.request_measured_layout(Style::default(), {
            let measured = measured.clone();
            let paragraph = self.0.clone();
            move |known, available, window, _| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let layout = paragraph.layout(width, window);
                let size = layout.size();
                measured.replace(Some(layout));
                size
            }
        });
        (id, measured)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        measured: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        let width = Some(bounds.size.width);
        if let Some(layout) = measured.take()
            && layout.width == width
        {
            return layout;
        }
        self.0.layout(width, window)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        paragraph: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        let style = &self.0.fonts.style;
        let result = window.paint_layer(bounds, |window| {
            for line in paragraph.visual_lines(bounds, style.text_align) {
                line.paint(style.color, window)?;
            }
            Ok::<_, anyhow::Error>(())
        });
        if let Err(error) = result {
            eprintln!("Paint confirmation text: {error:#}");
        }
    }
}
