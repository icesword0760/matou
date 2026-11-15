use gpui::{prelude::*, *};
#[cfg(target_os = "macos")]
use std::{cell::RefCell, rc::Rc};
#[cfg(target_os = "macos")]
use unicode_segmentation::UnicodeSegmentation;

#[derive(IntoElement)]
/// Match the frozen Mac UI font cascade without CoreText's automatic CJK scaling.
pub struct UiText(SharedString);

pub fn ui_text(text: impl Into<SharedString>) -> UiText {
    UiText(text.into())
}

impl RenderOnce for UiText {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            let fonts = UiFonts::new(window);
            let runs = fonts.runs(&self.0, window);
            if let Some(TextOverflow::Truncate(suffix)) = fonts.style.text_overflow.clone()
                && (fonts.style.white_space == WhiteSpace::Nowrap
                    || fonts.style.line_clamp == Some(1))
                && !self.0.is_ascii()
                && !self.0.contains('\n')
            {
                return SingleLineText(Rc::new(SingleLineLabel {
                    text: self.0,
                    suffix,
                    fonts,
                    runs,
                }))
                .into_any();
            }
            StyledText::new(self.0).with_runs(runs).into_any()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = window;
            StyledText::new(self.0)
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
        let primary = style.font();
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
    suffix: SharedString,
    fonts: UiFonts,
    runs: Vec<TextRun>,
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

    fn fit(&self, width: Option<Pixels>, window: &Window) -> Option<WrappedLine> {
        let full = self.shape(self.text.clone(), &self.runs, window)?;
        let Some(width) = width.filter(|width| full.unwrapped_layout.width > *width) else {
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
            let text: SharedString =
                format!("{}{}", &self.text[..boundaries[mid]], self.suffix).into();
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
}

#[cfg(target_os = "macos")]
struct MeasuredLine {
    limit: Option<Pixels>,
    line: Option<WrappedLine>,
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
    type RequestLayoutState = (Pixels, Rc<RefCell<Option<MeasuredLine>>>);
    type PrepaintState = Option<WrappedLine>;

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
        let line_height = self
            .0
            .fonts
            .style
            .line_height
            .to_pixels(self.0.fonts.size.into(), window.rem_size());
        let measured = Rc::new(RefCell::new(None));
        let layout_id = window.request_measured_layout(Style::default(), {
            let measured = measured.clone();
            let label = self.0.clone();
            move |known, available, window, _| {
                let limit = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let line = label.fit(limit, window);
                let width = line
                    .as_ref()
                    .map_or(px(0.), |line| line.unwrapped_layout.width.ceil());
                measured.replace(Some(MeasuredLine { limit, line }));
                size(limit.map_or(width, |limit| width.min(limit)), line_height)
            }
        });
        (layout_id, (line_height, measured))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (_, measured): &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        let width = bounds.size.width;
        if let Some(MeasuredLine {
            limit: Some(limit),
            line,
        }) = measured.take()
            && limit == width
        {
            return line;
        }
        self.0.fit(Some(width), window)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (line_height, _): &mut Self::RequestLayoutState,
        line: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(line) = line {
            let align = self.0.fonts.style.text_align;
            if let Err(error) =
                line.paint_background(bounds.origin, *line_height, align, Some(bounds), window, cx)
            {
                eprintln!("Paint UI title background: {error:#}");
            }
            if let Err(error) =
                line.paint(bounds.origin, *line_height, align, Some(bounds), window, cx)
            {
                eprintln!("Paint UI title: {error:#}");
            }
        }
    }
}
