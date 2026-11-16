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
