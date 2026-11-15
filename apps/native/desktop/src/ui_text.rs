use gpui::{prelude::*, *};
#[cfg(target_os = "macos")]
use std::{cell::RefCell, rc::Rc};
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
}

pub fn ui_text(text: impl Into<SharedString>) -> UiText {
    UiText {
        text: text.into(),
        #[cfg(target_os = "macos")]
        normal_line_height: false,
        #[cfg(target_os = "macos")]
        pre_line: false,
    }
}

impl UiText {
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
                return SingleLineText(Rc::new(SingleLineLabel {
                    text: self.text,
                    fonts,
                    runs,
                    normal_line_height: self.normal_line_height,
                }))
                .into_any();
            }
            StyledText::new(self.text).with_runs(runs).into_any()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = window;
            StyledText::new(self.text)
        }
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
        window: &Window,
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
            let runs = self.fonts.runs(&text, window);
            let candidate = self.shape(text, &runs, window)?;
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
        let full = self.shape(self.text.clone(), &self.runs, window);
        // Normal line height uses all fonts participating in layout before paint truncation.
        let metrics = self.metrics(full.as_ref(), window);
        let line = full.and_then(|line| self.fit(line, limit, window));
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

#[cfg(target_os = "macos")]
struct MeasuredLine {
    limit: Option<Pixels>,
    line: Option<WrappedLine>,
    metrics: LineMetrics,
}

#[cfg(target_os = "macos")]
struct SingleLineText(Rc<SingleLineLabel>);

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
            let label = self.0.clone();
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
        if let Some(result) = measured.take()
            && result.limit == Some(width)
        {
            return result;
        }
        self.0.measure(Some(width), window)
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
            let align = self.0.fonts.style.text_align;
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
