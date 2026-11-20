use matou_native_desktop::carousel_motion::{CardWidth, CarouselLayout, Curve, Transition};
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
fn edge_confirmation_retargets_width_in_the_same_frame() {
    let result = capture_case(&serde_json::json!({
        "name": "confirm-right-edge",
        "viewport": 1000,
        "focus": 0,
        "count": 3,
        "steps": [
            {"ms": 0, "event": "baseline"},
            {"ms": 16, "event": "edge-right"}
        ],
        "frames": [0, 16, 195, 196, 197, 576]
    }));
    let frames = result["frames"].as_array().unwrap();
    let before = &frames[2];
    let confirmed = &frames[3];
    let next = &frames[4];
    assert_eq!(before["edge"], "confirming");
    assert_eq!(confirmed["edge"], "cruising");
    assert_eq!(confirmed["hover"], 2);
    assert_eq!(confirmed["cards"][2]["targetWidth"], 470.4);
    assert_eq!(confirmed["cards"][2]["width"], before["cards"][2]["width"]);
    assert!(next["cards"][2]["width"].as_f64().unwrap() > 319.);
    assert_eq!(
        frames[5]["cards"][2]["width"],
        frames[0]["cards"][0]["width"]
    );
}

#[test]
fn resize_animates_rule_changes_and_resolves_unchanged_rules_immediately() {
    let wide = CarouselLayout::for_viewport(1000.);
    let fewer_columns = CarouselLayout::for_viewport(760.);
    let mut inactive = CardWidth::new(wide.width_rule(false, false));
    let mut active = CardWidth::new(wide.width_rule(true, false));

    active.retarget(fewer_columns.width_rule(true, false), 16., 380.);
    assert_eq!(active.value(760., 16.), 355.2);
    assert!(!active.is_running(16.));

    inactive.retarget(fewer_columns.width_rule(false, false), 16., 380.);
    assert!((inactive.value(760., 16.) - 238.6640625).abs() < 0.0078125);
    assert!((inactive.value(760., 32.) - 260.0078125).abs() < 0.0078125);
    assert_eq!(inactive.value(760., 396.), 364.);

    let narrow = CarouselLayout::for_viewport(680.);
    inactive.retarget(narrow.width_rule(false, false), 416., 380.);
    assert_eq!(inactive.value(680., 416.), 324.);
    assert_eq!(inactive.value(680., 796.), 176.);

    let wider = CarouselLayout::for_viewport(1500.);
    inactive.retarget(wider.width_rule(false, false), 816., 380.);
    assert_eq!(inactive.value(1500., 816.), 176.);
    assert!((inactive.value(1500., 832.) - 207.5).abs() < 0.0078125);
    inactive.retarget(wider.width_rule(false, false), 832., 380.);
    assert!(inactive.value(1540., 832.) > inactive.value(1500., 832.));
    assert_eq!(inactive.value(1500., 1196.), 361.);
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
    let initial_layout = CarouselLayout::for_viewport(viewport);
    let mut transitions: Vec<_> = (0..count)
        .map(|id| {
            (
                CardWidth::new(initial_layout.width_rule(id == focus, false)),
                Transition::new(if id == focus { 1. } else { 0. }),
            )
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

            // Workspace confirms an edge preview before rebuilding geometry.
            if edge_deadline.is_some_and(|deadline| ms >= deadline) {
                hover = edge_target;
                edge = "cruising";
                edge_deadline = None;
            }
            let layout = CarouselLayout::for_viewport(viewport);
            let base = layout.width_rule(false, false).width(viewport);

            let mut left = 10.;
            geometry.clear();
            let cards: Vec<_> = (0..count)
                .zip(&mut transitions)
                .map(|(id, (width, lift))| {
                    let selected = if id == focus { 1. } else { 0. };
                    let rule = layout.width_rule(id == focus, id == hover);
                    width.retarget(rule, ms, 380.);
                    lift.retarget(selected, ms, 180., Curve::Lift);
                    let rendered_width = width.value(viewport, ms);
                    geometry.push((left, rendered_width));
                    left += rendered_width + 12.;
                    let mut card = json!({
                        "width": f64::from(rendered_width as f32),
                        "lift": f64::from(-4. * lift.value(ms) as f32),
                        "targetWidth": width.target_rule().width(viewport),
                        "targetLift": -4. * selected,
                    });
                    if id + 1 < count {
                        card["gapAfter"] = json!(12);
                    }
                    card
                })
                .collect();
            json!({
                "ms": ms, "viewport": viewport, "columns": layout.columns, "narrow": layout.narrow, "baseWidth": base,
                "focus": focus, "hover": hover, "edge": edge, "cards": cards,
            })
        })
        .collect();
    assert!(steps.next().is_none(), "input event missing a sample time");
    json!({"name": case["name"], "frames": frames})
}
