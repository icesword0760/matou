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

#[test]
#[ignore = "writes deterministic gate evidence to explicit input/output paths"]
fn capture_canvas_motion_gate() {
    let input = std::env::var_os("MATOU_CANVAS_INPUT").unwrap();
    let output = std::env::var_os("MATOU_CANVAS_OUTPUT").unwrap();
    let plan: Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let cases: Vec<_> = plan["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(capture_case)
        .collect();
    let curves: serde_json::Map<_, _> = plan["curves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| {
            let name = name.as_str().unwrap();
            let curve = match name {
                "width" => Curve::Width,
                "lift" => Curve::Lift,
                "edge" => Curve::Edge,
                "ease" => Curve::Ease,
                _ => panic!("unknown curve {name}"),
            };
            let mut transition = Transition::new(0.);
            transition.retarget(1., 0., 1000., curve);
            let mut overshoot = false;
            let samples: Vec<_> = plan["fractions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|fraction| {
                    let fraction = fraction.as_f64().unwrap();
                    let value = transition.value(fraction * 1000.);
                    overshoot |= !(0. ..=1.).contains(&value);
                    serde_json::json!({"fraction": fraction, "value": value})
                })
                .collect();
            (
                name.to_owned(),
                serde_json::json!({"samples": samples, "overshoot": overshoot}),
            )
        })
        .collect();
    let result = serde_json::json!({"cases": cases, "curves": curves});
    std::fs::write(
        output,
        serde_json::to_string_pretty(&result).unwrap() + "\n",
    )
    .unwrap();
}

fn capture_case(case: &Value) -> Value {
    use serde_json::json;

    let mut viewport = case["viewport"].as_f64().unwrap();
    let mut focus = case["focus"].as_i64().unwrap();
    let mut hover = -1;
    let mut edge = "idle";
    let mut edge_deadline = None;
    let mut edge_target = -1;
    let mut geometry = Vec::<(f64, f64)>::new();
    let count = case["count"].as_u64().unwrap() as i64;
    let mut transitions: Vec<_> = (0..count)
        .map(|id| {
            let selected = if id == focus { 1. } else { 0. };
            (Transition::new(selected), Transition::new(selected))
        })
        .collect();
    let mut steps = case["steps"].as_array().unwrap().iter().peekable();
    let frames: Vec<_> = case["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|ms| {
            let ms = ms.as_f64().unwrap();
            while let Some(step) = steps.next_if(|step| step["ms"].as_f64().unwrap() == ms) {
                match step["event"].as_str().unwrap() {
                    "baseline" => {}
                    "focus" => focus = step["value"].as_i64().unwrap(),
                    "hover" => hover = step["value"].as_i64().unwrap(),
                    "leave" => {
                        hover = -1;
                        edge = "idle";
                        edge_deadline = None;
                    }
                    "resize" => viewport = step["value"].as_f64().unwrap(),
                    "edge-right" => {
                        // Workspace::nearest_hidden_card with the fixture initially
                        // at scrollLeft 0. It stays on the first cruise interval.
                        if let Some(index) = geometry
                            .iter()
                            .position(|(left, width)| left + width > viewport - 10. + 0.5)
                        {
                            edge_target = index as i64;
                            edge = "confirming";
                            edge_deadline = Some(ms + 180.);
                        }
                    }
                    other => panic!("unknown input event {other}"),
                }
            }

            // Workspace::render receives carousel viewport width after the
            // 260px sidebar. Only the private target/layout input is projected.
            let content_width = viewport - 20.;
            let columns = (((viewport + 12.) / 292.).floor()).clamp(1., 4.);
            let narrow = viewport < 720.;
            let base = if narrow {
                (content_width * 0.28).clamp(112., 176.)
            } else {
                (content_width - (columns - 1.) * 12.) / columns
            };
            let expanded = if narrow {
                (content_width * 0.68).clamp(260., 560.)
            } else {
                (content_width * 0.48).min(620.)
            };

            let mut left = 10.;
            geometry.clear();
            let cards: Vec<_> = (0..count)
                .zip(&mut transitions)
                .map(|(id, (width, lift))| {
                    let selected = if id == focus { 1. } else { 0. };
                    let target = if id == focus || (!narrow && id == hover) {
                        1.
                    } else {
                        0.
                    };
                    width.retarget(target, ms, 380., Curve::Width);
                    lift.retarget(selected, ms, 180., Curve::Lift);
                    let rendered_width = base + (expanded - base) * width.value(ms);
                    geometry.push((left, rendered_width));
                    left += rendered_width + 12.;
                    let mut card = json!({
                        "width": f64::from(rendered_width as f32),
                        "lift": f64::from(-4. * lift.value(ms) as f32),
                        "targetWidth": base + (expanded - base) * target,
                        "targetLift": -4. * selected,
                    });
                    if id + 1 < count {
                        card["gapAfter"] = json!(12);
                    }
                    card
                })
                .collect();
            // The Workspace render checks its edge deadline after computing
            // this frame's card widths. The hover will retarget on the next frame.
            if edge_deadline.is_some_and(|deadline| ms >= deadline) {
                hover = edge_target;
                edge = "cruising";
                edge_deadline = None;
            }
            json!({
                "ms": ms, "viewport": viewport, "columns": columns, "narrow": narrow, "baseWidth": base,
                "focus": focus, "hover": hover, "edge": edge, "cards": cards,
            })
        })
        .collect();
    assert!(steps.next().is_none(), "input event missing a sample time");
    json!({"name": case["name"], "frames": frames})
}
