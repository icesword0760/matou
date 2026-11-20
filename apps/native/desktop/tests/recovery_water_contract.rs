use matou_native_desktop::recovery_water::Fluid;
use serde_json::Value;
fn compare(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => assert!(
            (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-8,
            "{path}: {a} != {b}"
        ),
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path} length");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                compare(a, b, &format!("{path}/{i}"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, a) in a {
                if k != "time" {
                    compare(a, &b[k], &format!("{path}/{k}"));
                }
            }
        }
        (Value::String(a), Value::String(b))
            if a.starts_with("rgba(") && b.starts_with("rgba(") =>
        {
            let channels = |s: &str| {
                s.trim_start_matches("rgba(")
                    .trim_end_matches(')')
                    .split(',')
                    .map(|s| s.parse::<f64>().unwrap())
                    .collect::<Vec<_>>()
            };
            for (a, b) in channels(a).iter().zip(channels(b)) {
                assert!((a - b).abs() < 1e-8, "{path}: color channel {a}!={b}");
            }
        }
        _ => assert_eq!(a, b, "{path}"),
    }
}
#[test]
fn original_component_math_and_draw_contract() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/recovery-water.json")).unwrap();
    let mut frames = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let mut fluid = Fluid::default();
        for (i, frame) in case["frames"].as_array().unwrap().iter().enumerate() {
            let reduced = case["reduced"].as_bool().unwrap();
            let time = frame["time"].as_f64().unwrap();
            if reduced && i > 0 {
                fluid.resume_at(time);
            }
            let actual = fluid.sample(
                time,
                frame["width"].as_f64().unwrap(),
                frame["height"].as_f64().unwrap(),
                frame["dpr"].as_f64().unwrap(),
                reduced,
            );
            compare(frame, &actual, case["name"].as_str().unwrap());
            frames += 1;
        }
    }
    assert_eq!(frames, 16);
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "offline REC-01 capture, run from the gate script"]
fn capture_recovery_water_gate() {
    use gpui::{Bounds, point, px, size};
    use matou_native_desktop::{
        recovery_water::{background_svg, bounded_fluid_svg, edge_svg, fluid_svg},
        recovery_water_macos::{draw_background, draw_edges, draw_fluid, water_base_color},
        recovery_water_view::RecoveryWater,
        workspace::session_card_hit,
    };
    use matou_native_terminal::Theme;
    use serde_json::json;
    use std::{env, fs};

    let input: Value =
        serde_json::from_slice(&fs::read(env::var_os("MATOU_WATER_CASES").unwrap()).unwrap())
            .unwrap();
    let num = |value: &Value| value.as_f64().unwrap();
    let mut cases = Vec::new();
    for case in input["clock"].as_array().unwrap() {
        let mut fluid = Fluid::default();
        let mut visible = true;
        let mut dirty = true;
        let mut reset_previous = Some(0.);
        let mut spec = None;
        let mut backing = [0, 0];
        let reduced = case["reduced"].as_bool().unwrap();
        let mut dpr = num(&case["dpr"]);
        let mut frames = Vec::new();
        for step in case["frames"].as_array().unwrap() {
            let time = num(&step["time"]);
            let event = step["event"].as_str().unwrap();
            let (width, height) = (num(&step["width"]), num(&step["height"]));
            dpr = step["dpr"].as_f64().unwrap_or(dpr);
            if spec != Some((width, height, dpr)) {
                if reduced && spec.is_some() {
                    reset_previous = Some(time);
                }
                spec = Some((width, height, dpr));
                dirty = true;
            }
            if event == "hide" {
                visible = false;
                dirty = true;
            }
            if event == "show" {
                visible = true;
                dirty = true;
                reset_previous = Some(time);
            }
            // Supply fixed worker start times and immediate job completion. The
            // real view schedules only when visible and, for reduced motion, dirty.
            let pending_before = visible && (!reduced || dirty);
            let mut sample = Value::Null;
            if pending_before && event != "hide" && event != "show" {
                if let Some(previous) = reset_previous.take() {
                    fluid.reset_previous_frame(previous);
                }
                sample = fluid.sample(time, width, height, dpr, reduced);
                let bitmap = draw_fluid(&sample).unwrap();
                backing = [bitmap.width, bitmap.height];
                dirty = false;
            }
            frames.push(json!({"time": time, "event": event, "pendingBefore": pending_before,
                "pendingAfter": visible && (!reduced || dirty), "backing": backing, "sample": sample}));
        }
        cases.push(json!({"name": case["name"], "reduced": reduced, "frames": frames}));
    }

    let mut layers = Vec::new();
    for item in input["layers"].as_array().unwrap() {
        let theme = match item["theme"].as_str().unwrap() {
            "light" => Theme::LIGHT,
            "dark" => Theme::DARK,
            other => panic!("Unexpected theme: {other}"),
        };
        let width = f64::from(num(&item["width"]) as f32);
        let height = f64::from(num(&item["height"]) as f32);
        let dpr = num(&item["dpr"]);
        let background = draw_background(width, height, dpr, theme);
        let edges = draw_edges(width, height, dpr);
        layers.push(json!({
            "name": item["name"], "water": [0., f64::from(f32::from(RecoveryWater::TOP)), width, height],
            "backgroundBacking": [background.width, background.height], "edgeBacking": [edges.width, edges.height],
            "base": format!("#{:06x}", water_base_color(theme)),
            "backgroundSvg": background_svg(width, height, theme), "edgeSvg": edge_svg(width, height)
        }));
    }

    let card_width = px(num(&input["hitCard"]["width"]) as f32);
    let card_height = px(num(&input["hitCard"]["height"]) as f32);
    let card = Bounds::new(point(px(0.), px(0.)), size(card_width, card_height));
    let water = Bounds::new(
        point(px(0.), RecoveryWater::TOP),
        size(card_width, card_height - RecoveryWater::TOP),
    );
    let hit = input["hitPoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let pos = point(px(num(&item["x"]) as f32), px(num(&item["y"]) as f32));
            let inside = water.contains(&pos);
            json!({"name": item["name"], "insideWaterBounds": inside, "insideCardHitBounds": session_card_hit(&card, pos)})
        })
        .collect::<Vec<_>>();

    let frame = &cases[0]["frames"][2]["sample"];
    let svgs = json!({"time": 144, "dpr": frame["dpr"], "full": fluid_svg(frame),
        "boundedDiagnostic": bounded_fluid_svg(frame)});
    let result = json!({"cases": cases, "layers": layers, "hit": hit, "svg": svgs});
    fs::write(
        env::var_os("MATOU_WATER_NATIVE").unwrap(),
        format!("{}\n", result),
    )
    .unwrap();
    println!(
        "Native: {} clock cases, {} static layouts, {} bounds probes",
        cases.len(),
        layers.len(),
        hit.len()
    );
}
