//! Geometry of the frozen terminal's DOM history scrollbar (CSS pixels).
//!
//! The original scrollbar state rounds its inputs independently of the device
//! scale. Do not substitute the platform scrollbar used by the workspace list.

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollbarMetrics {
    pub viewport: f64,
    pub extent: f64,
    pub position: f64,
    pub thumb_size: f64,
    pub thumb_top: f64,
    ratio: f64,
}

fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

impl ScrollbarMetrics {
    pub fn new(viewport: f64, extent: f64, position: f64) -> Option<Self> {
        let viewport = js_round(viewport);
        let extent = js_round(extent);
        if extent <= viewport || viewport <= 0. {
            return None;
        }
        let position = js_round(position);
        let thumb_size = (viewport * viewport / extent).floor().max(20.);
        let ratio = (viewport - thumb_size) / (extent - viewport);
        Some(Self {
            viewport,
            extent,
            position,
            thumb_size,
            thumb_top: js_round(position * ratio),
            ratio,
        })
    }

    pub fn clamp(&self, position: f64) -> f64 {
        position.clamp(0., self.extent - self.viewport)
    }

    pub fn track_position(&self, pointer: f64) -> f64 {
        self.clamp(js_round((pointer - self.thumb_size / 2.) / self.ratio))
    }

    pub fn drag_position(&self, delta: f64) -> f64 {
        self.clamp(js_round((self.thumb_top + delta) / self.ratio))
    }
}

#[cfg(test)]
mod tests {
    use super::ScrollbarMetrics;

    #[test]
    fn matches_frozen_scrollbar_source() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/terminal-scrollbar.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let number = |key: &str| case[key].as_f64().unwrap();
            let metrics =
                ScrollbarMetrics::new(number("viewport"), number("extent"), number("position"));
            assert_eq!(
                metrics.is_some(),
                case["needed"].as_bool().unwrap(),
                "{case}"
            );
            let Some(metrics) = metrics else { continue };
            assert_eq!(metrics.thumb_size, number("thumbSize"), "{case}");
            assert_eq!(metrics.thumb_top, number("thumbTop"), "{case}");
            for probe in case["probes"].as_array().unwrap() {
                let value = probe["value"].as_f64().unwrap();
                let actual = if probe["op"] == "track" {
                    metrics.track_position(value)
                } else {
                    metrics.drag_position(value)
                };
                assert_eq!(actual, probe["result"].as_f64().unwrap(), "{case}: {probe}");
            }
        }
    }
}
