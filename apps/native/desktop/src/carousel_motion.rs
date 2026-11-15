//! Canvas card transitions and focus-follow scrolling.

#[derive(Clone, Copy, Debug)]
pub enum Curve {
    Ease,
    Width,
    Lift,
    Edge,
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
            let factor = (progress * self.shortening_factor + (1.0 - self.shortening_factor))
                .abs()
                .clamp(0.0, 1.0);
            (self.target, factor)
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
