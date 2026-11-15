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
}

pub fn ui_text(text: impl Into<SharedString>) -> UiText {
    UiText {
        text: text.into(),
        #[cfg(target_os = "macos")]
        normal_line_height: false,
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
}

impl RenderOnce for UiText {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            let fonts = UiFonts::new(window);
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

    fn metrics(&self, line: Option<&WrappedLine>, window: &Window) -> SingleLineMetrics {
        let scale = window.scale_factor();
        let system = window.text_system();
        let size = self.fonts.size;
        let rounded_metrics = |id| {
            let ascent = (system.ascent(id, size) * scale).round();
            let descent = px((f32::from(system.descent(id, size)).abs() * scale).round());
            (ascent, descent)
        };
        let (mut ascent, mut descent) = rounded_metrics(self.fonts.primary_id);
        let height = if self.normal_line_height {
            if let Some(line) = line {
                for run in &line.unwrapped_layout.runs {
                    let (run_ascent, run_descent) = rounded_metrics(run.font_id);
                    ascent = ascent.max(run_ascent);
                    descent = descent.max(run_descent);
                }
            }
            // The fixed Mac UI cascade (SF and PingFang SC) has zero font line gap.
            (ascent + descent) / scale
        } else {
            self.fonts
                .style
                .line_height
                .to_pixels(size.into(), window.rem_size())
        };
        // The reference lays out text at device scale and floors the upper half-leading.
        let upper_leading = ((height * scale - ascent - descent) / 2.).floor();
        SingleLineMetrics {
            height,
            baseline_from_top: ascent + upper_leading,
            scale,
        }
    }
}

#[cfg(target_os = "macos")]
struct SingleLineMetrics {
    height: Pixels,
    baseline_from_top: Pixels,
    scale: f32,
}

#[cfg(target_os = "macos")]
impl SingleLineMetrics {
    fn text_origin(&self, origin: Point<Pixels>, line: &WrappedLine) -> Point<Pixels> {
        let baseline = ((origin.y * self.scale).round() + self.baseline_from_top) / self.scale;
        let layout = &line.unwrapped_layout;
        // Compensate for the baseline added by WrappedLine::paint.
        let paint_baseline = (self.height - layout.ascent - layout.descent) / 2. + layout.ascent;
        point(origin.x, baseline - paint_baseline)
    }
}

#[cfg(target_os = "macos")]
struct MeasuredLine {
    limit: Option<Pixels>,
    line: Option<WrappedLine>,
    metrics: SingleLineMetrics,
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
