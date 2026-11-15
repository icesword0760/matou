use matou_native_desktop::carousel_motion::{Curve, Transition};
use serde_json::Value;

#[test]
fn transitions_match_the_frozen_css_browser_samples() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/carousel-motion-reference.json")).unwrap();
    let inactive = reference["inactive"]["width"].as_f64().unwrap();
    let active = reference["active"]["width"].as_f64().unwrap();
    for case in reference["cases"].as_array().unwrap() {
        let initial = case["initial"].as_f64().unwrap();
        let mut width = Transition::new(initial);
        let mut lift = Transition::new(initial);
        let mut elapsed = 0.;
        for segment in case["segments"].as_array().unwrap() {
            let target = segment["target"].as_f64().unwrap();
            width.retarget(target, elapsed, 380., Curve::Width);
            lift.retarget(target, elapsed, 180., Curve::Lift);
            let samples = segment["samples"].as_array().unwrap();
            for sample in samples {
                let at = elapsed + sample["ms"].as_f64().unwrap();
                // An ordinary output-driven repaint must not restart a transition.
                width.retarget(target, at, 380., Curve::Width);
                lift.retarget(target, at, 180., Curve::Lift);
                let actual_width = inactive + (active - inactive) * width.value(at);
                let actual_lift = -4. * lift.value(at);
                // Chromium's layout positions are quantized (1/64 CSS pixel).
                // This numerical contract tolerance is not a visual diff mask.
                assert!(
                    (actual_width - sample["width"].as_f64().unwrap()).abs() < 0.02,
                    "{} at {at}ms: width {actual_width} vs {}",
                    case["name"],
                    sample["width"]
                );
                assert!(
                    (actual_lift - sample["lift"].as_f64().unwrap()).abs() < 0.00005,
                    "{} at {at}ms: lift {actual_lift} vs {}",
                    case["name"],
                    sample["lift"]
                );
            }
            elapsed += samples.last().unwrap()["ms"].as_f64().unwrap();
        }
    }
}
