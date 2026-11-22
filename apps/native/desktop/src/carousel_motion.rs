//! Canvas card transitions and focus-follow scrolling.

#[derive(Clone, Copy, Debug)]
pub enum Curve {
    Ease,
    Width,
    Lift,
    Edge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CardElevation {
    Resting,
    Preview,
    Focused,
}

impl CardElevation {
    pub fn for_card(selected: bool, hovered: bool) -> Self {
        if selected {
            Self::Focused
        } else if hovered {
            Self::Preview
        } else {
            Self::Resting
        }
    }

    pub fn weights(self) -> [f32; 3] {
        let mut weights = [0.; 3];
        weights[self as usize] = 1.;
        weights
    }
}

impl Curve {
    pub(crate) fn ease(self, elapsed_fraction: f64) -> f64 {
        if elapsed_fraction <= 0.0 {
            return 0.0;
        }
        if elapsed_fraction >= 1.0 {
            return 1.0;
        }

        // Both curves have the same monotonically increasing X coordinate.
        // Solve X(t) for elapsed time before evaluating the distinct Y curves.
        let x_control = if matches!(self, Self::Edge | Self::Ease) {
            0.25
        } else {
            0.2
        };
        let mut low = 0.0;
        let mut high = 1.0;
        for _ in 0..30 {
            let middle = (low + high) * 0.5;
            if cubic_coordinate(middle, x_control, x_control) < elapsed_fraction {
                low = middle;
            } else {
                high = middle;
            }
        }
        let y1 = match self {
            Self::Width | Self::Edge => 0.8,
            Self::Lift => 0.82,
            Self::Ease => 0.1,
        };
        cubic_coordinate((low + high) * 0.5, y1, 1.0)
    }
}

fn cubic_coordinate(t: f64, control1: f64, control2: f64) -> f64 {
    let remaining = 1.0 - t;
    t * (3.0 * remaining * (remaining * control1 + t * control2) + t * t)
}

#[derive(Clone, Copy, Debug)]
pub struct Transition {
    start_value: f64,
    target: f64,
    reversing_start: f64,
    shortening_factor: f64,
    start_ms: f64,
    duration_ms: f64,
    curve: Curve,
}

impl Transition {
    pub fn new(value: f64) -> Self {
        Self {
            start_value: value,
            target: value,
            reversing_start: value,
            shortening_factor: 1.0,
            start_ms: 0.0,
            duration_ms: 0.0,
            curve: Curve::Width,
        }
    }

    pub fn retarget(&mut self, target: f64, now_ms: f64, duration_ms: f64, curve: Curve) {
        if self.target == target {
            return;
        }

        let running = self.is_running(now_ms);
        let progress = if running {
            self.eased_progress(now_ms)
        } else {
            1.0
        };
        let current = self.interpolate(progress);
        if current == target || duration_ms <= 0.0 {
            self.snap(target);
            return;
        }

        // CSS Transitions §3 adjusts the duration when reversing a running
        // transition, including another reversal before the first completes.
        let (reversing_start, shortening_factor) = if running && target == self.reversing_start {
            (
                self.target,
                reversal_factor(progress, self.shortening_factor),
            )
        } else {
            (current, 1.0)
        };
        *self = Self {
            start_value: current,
            target,
            reversing_start,
            shortening_factor,
            start_ms: now_ms,
            duration_ms: duration_ms * shortening_factor,
            curve,
        };
    }

    pub fn value(&self, now_ms: f64) -> f64 {
        if self.is_running(now_ms) {
            self.interpolate(self.eased_progress(now_ms))
        } else {
            self.target
        }
    }

    pub fn is_running(&self, now_ms: f64) -> bool {
        self.duration_ms > 0.0 && now_ms - self.start_ms < self.duration_ms
    }

    pub fn snap(&mut self, value: f64) {
        *self = Self::new(value);
    }

    fn eased_progress(&self, now_ms: f64) -> f64 {
        self.curve.ease((now_ms - self.start_ms) / self.duration_ms)
    }

    fn interpolate(&self, progress: f64) -> f64 {
        if progress == 1.0 {
            return self.target;
        }
        self.start_value + (self.target - self.start_value) * progress
    }
}

fn reversal_factor(progress: f64, shortening_factor: f64) -> f64 {
    (progress * shortening_factor + (1.0 - shortening_factor))
        .abs()
        .clamp(0.0, 1.0)
}

#[derive(Clone, Copy)]
pub struct CarouselLayout {
    pub columns: u32,
    pub narrow: bool,
}

impl CarouselLayout {
    pub fn for_viewport(viewport: f64) -> Self {
        Self {
            columns: (((viewport + 12.) / 292.).floor()).clamp(1., 4.) as u32,
            narrow: viewport < 720.,
        }
    }

    pub fn width_rule(self, selected: bool, hovered: bool) -> WidthRule {
        match (self.narrow, selected, hovered) {
            (true, true, _) => WidthRule::NarrowExpanded,
            (true, false, _) => WidthRule::NarrowBase,
            (false, true, _) | (false, false, true) => WidthRule::WideExpanded,
            (false, false, false) => WidthRule::Columns(self.columns),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum WidthRule {
    Columns(u32),
    WideExpanded,
    NarrowBase,
    NarrowExpanded,
}

const WIDTH_RULES: [WidthRule; 7] = [
    WidthRule::Columns(1),
    WidthRule::Columns(2),
    WidthRule::Columns(3),
    WidthRule::Columns(4),
    WidthRule::WideExpanded,
    WidthRule::NarrowBase,
    WidthRule::NarrowExpanded,
];

impl WidthRule {
    pub fn width(self, viewport: f64) -> f64 {
        let content = viewport - 20.;
        match self {
            Self::Columns(columns) => {
                let columns = f64::from(columns);
                (content - (columns - 1.) * 12.) / columns
            }
            Self::WideExpanded => (content * 0.48).min(620.),
            Self::NarrowBase => (content * 0.28).clamp(112., 176.),
            Self::NarrowExpanded => (content * 0.68).clamp(260., 560.),
        }
    }

    fn weights(self) -> [f64; WIDTH_RULES.len()] {
        WIDTH_RULES.map(|rule| if rule == self { 1. } else { 0. })
    }
}

pub struct CardWidth {
    // A running transition can be interrupted by another rule change. Keep
    // its current blend responsive to the viewport when it becomes the start.
    start: [f64; WIDTH_RULES.len()],
    target: WidthRule,
    reversing_start: Option<WidthRule>,
    shortening_factor: f64,
    progress: Transition,
}

impl CardWidth {
    pub fn new(rule: WidthRule) -> Self {
        Self {
            start: rule.weights(),
            target: rule,
            reversing_start: Some(rule),
            shortening_factor: 1.,
            progress: Transition::new(1.),
        }
    }

    pub fn retarget(&mut self, rule: WidthRule, now: f64, duration: f64) {
        if self.target == rule {
            return;
        }
        let progress = self.progress.value(now);
        let previous_target = self.target;
        let end = previous_target.weights();
        for (start, end) in self.start.iter_mut().zip(end) {
            *start += (end - *start) * progress;
        }
        if progress < 1. && self.reversing_start == Some(rule) {
            self.shortening_factor = reversal_factor(progress, self.shortening_factor);
            self.reversing_start = Some(previous_target);
        } else {
            self.shortening_factor = 1.;
            self.reversing_start = WIDTH_RULES
                .into_iter()
                .find(|rule| rule.weights() == self.start);
        }
        self.target = rule;
        self.progress.snap(0.);
        self.progress
            .retarget(1., now, duration * self.shortening_factor, Curve::Width);
    }

    pub fn value(&self, viewport: f64, now: f64) -> f64 {
        let end = self.target.width(viewport);
        let progress = self.progress.value(now);
        if progress == 1. {
            return end;
        }
        let start: f64 = WIDTH_RULES
            .iter()
            .zip(self.start)
            .map(|(rule, weight)| rule.width(viewport) * weight)
            .sum();
        start + (end - start) * progress
    }

    pub fn is_running(&self, now: f64) -> bool {
        self.progress.is_running(now)
    }

    pub fn target_rule(&self) -> WidthRule {
        self.target
    }
}

pub fn centered_scroll(
    card_left: f64,
    card_width: f64,
    viewport_width: f64,
    max_scroll: f64,
) -> f64 {
    let padding = (viewport_width - card_width).max(0.0) / 2.0;
    (card_left - padding).min(max_scroll).max(0.0)
}

pub fn focus_scroll_step(current: f64, target: f64) -> f64 {
    let delta = target - current;
    if delta.abs() > 0.5 {
        current + delta.clamp(-28.0, 28.0)
    } else {
        current
    }
}

pub const HOVER_RESTORE_DELAY_MS: f64 = 220.;

pub fn hover_restore_step(
    now: f64,
    restore_at: f64,
    current: f64,
    baseline: f64,
    max_scroll: f64,
) -> (f64, bool) {
    if now < restore_at {
        return (current, false);
    }
    let target = baseline.clamp(0., max_scroll);
    if (target - current).abs() <= 28. {
        (target, true)
    } else {
        (focus_scroll_step(current, target), false)
    }
}

/// Source preview follows the clipped edge without recentering an already visible card.
pub fn fully_visible_scroll(
    left: f64,
    width: f64,
    current: f64,
    viewport: f64,
    max_scroll: f64,
) -> f64 {
    if left < current + 10. {
        (left - 10.).clamp(0., max_scroll)
    } else if left + width > current + viewport - 10. {
        (left + width - viewport + 10.).clamp(0., max_scroll)
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_widths_match_viewport_breakpoints_and_limits() {
        for (viewport, columns, base, expanded) in [
            (720., 2, 344., 336.),
            (863., 2, 415.5, 404.64),
            (864., 3, 820. / 3., 405.12),
            (1155., 3, 1111. / 3., 544.8),
            (1156., 4, 275., 545.28),
            (1500., 4, 361., 620.),
        ] {
            let layout = CarouselLayout::for_viewport(viewport);
            assert!(!layout.narrow);
            assert_eq!(layout.columns, columns);
            assert!((layout.width_rule(false, false).width(viewport) - base).abs() < 1e-9);
            assert!((layout.width_rule(false, true).width(viewport) - expanded).abs() < 1e-9);
            assert!((layout.width_rule(true, true).width(viewport) - expanded).abs() < 1e-9);
        }
        for (viewport, columns, base, expanded) in [
            (300., 1, 112., 260.),
            (520., 1, 140., 340.),
            (719., 2, 176., 475.32),
        ] {
            let layout = CarouselLayout::for_viewport(viewport);
            assert!(layout.narrow);
            assert_eq!(layout.columns, columns);
            assert!((layout.width_rule(false, false).width(viewport) - base).abs() < 1e-9);
            assert!((layout.width_rule(false, true).width(viewport) - base).abs() < 1e-9);
            assert!((layout.width_rule(true, false).width(viewport) - expanded).abs() < 1e-9);
        }
        assert_eq!(WidthRule::Columns(1).width(560.), 540.);
        assert_eq!(WidthRule::NarrowExpanded.width(1000.), 560.);
    }

    #[test]
    fn preview_expands_with_the_width_curve_and_promotes_without_retargeting() {
        let viewport = 1000.;
        let layout = CarouselLayout::for_viewport(viewport);
        let mut preview = CardWidth::new(layout.width_rule(false, false));
        let mut focused = CardWidth::new(layout.width_rule(true, false));
        preview.retarget(layout.width_rule(false, true), 100., 380.);
        focused.retarget(layout.width_rule(true, false), 100., 380.);
        let start = WidthRule::Columns(3).width(viewport);
        let end = WidthRule::WideExpanded.width(viewport);
        let middle = preview.value(viewport, 290.);
        assert!((middle - (start + (end - start) * Curve::Width.ease(0.5))).abs() < 1e-9);
        assert_eq!(focused.value(viewport, 290.), end);
        preview.retarget(layout.width_rule(true, false), 290., 380.);
        assert_eq!(preview.value(viewport, 290.), middle);
        assert_eq!(preview.value(viewport, 480.), end);
        assert!(!preview.is_running(480.));
    }

    #[test]
    fn hover_restore_waits_then_moves_at_most_28_pixels_per_frame() {
        let at = 100. + HOVER_RESTORE_DELAY_MS;
        assert_eq!(
            hover_restore_step(at - 1., at, 150., 100., 200.),
            (150., false)
        );
        assert_eq!(hover_restore_step(at, at, 150., 100., 200.), (122., false));
        assert_eq!(
            hover_restore_step(at + 16., at, 122., 100., 200.),
            (100., true)
        );
        assert_eq!(
            hover_restore_step(at + 16., at, 95., 150., 110.),
            (110., true)
        );
        assert_eq!(hover_restore_step(at, at, 10., 38.2, 100.), (38., false));
    }
}
