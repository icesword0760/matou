use anyhow::Result;
use matou_native_terminal::{MouseEncoding, MouseEvent, MouseProtocol, MouseReporter, MouseWheel};
use serde_json::Value;

fn protocol(name: &str) -> MouseProtocol {
    match name {
        "NONE" => MouseProtocol::None,
        "X10" => MouseProtocol::X10,
        "VT200" => MouseProtocol::Vt200,
        "DRAG" => MouseProtocol::Drag,
        "ANY" => MouseProtocol::Any,
        _ => panic!("Unexpected protocol: {name}"),
    }
}

fn encoding(name: &str) -> MouseEncoding {
    match name {
        "DEFAULT" => MouseEncoding::Default,
        "SGR" => MouseEncoding::Sgr,
        "SGR_PIXELS" => MouseEncoding::SgrPixels,
        _ => panic!("Unexpected mouse encoding: {name}"),
    }
}

fn mouse_event(event: &Value) -> Result<MouseEvent> {
    Ok(MouseEvent {
        col: i32::try_from(event["col"].as_i64().unwrap())?,
        row: i32::try_from(event["row"].as_i64().unwrap())?,
        x: i32::try_from(event["x"].as_i64().unwrap())?,
        y: i32::try_from(event["y"].as_i64().unwrap())?,
        button: u8::try_from(event["button"].as_u64().unwrap())?,
        action: u8::try_from(event["action"].as_u64().unwrap())?,
        ctrl: event["ctrl"].as_bool().unwrap(),
        alt: event["alt"].as_bool().unwrap(),
        shift: event["shift"].as_bool().unwrap(),
    })
}

#[test]
fn reports_match_frozen_mouse_service_byte_for_byte() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/mouse-reference.json"))?;
    for case in fixture["cases"].as_array().unwrap() {
        let mut reporter = MouseReporter::default();
        let mut active_protocol = protocol(case["protocol"].as_str().unwrap());
        let mut active_encoding = encoding(case["encoding"].as_str().unwrap());
        let cols = usize::try_from(case["cols"].as_u64().unwrap())?;
        let rows = usize::try_from(case["rows"].as_u64().unwrap())?;
        for (index, probe) in case["probes"].as_array().unwrap().iter().enumerate() {
            if let Some(name) = probe["protocol"].as_str() {
                active_protocol = protocol(name);
            }
            if let Some(name) = probe["encoding"].as_str() {
                active_encoding = encoding(name);
            }
            let event = mouse_event(&probe["event"])?;
            let expected: Option<Vec<u8>> =
                serde_json::from_value(probe["output"]["bytes"].clone())?;
            assert_eq!(
                reporter.report(active_protocol, active_encoding, event, cols, rows),
                expected,
                "{} probe {}: {probe}",
                case["name"],
                index + 1,
            );
        }
    }
    Ok(())
}

#[test]
fn wheel_amounts_match_frozen_mouse_service() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/mouse-reference.json"))?;
    for case in fixture["wheelCases"].as_array().unwrap() {
        let mut wheel = MouseWheel::default();
        let cell_height = case["cellHeight"].as_f64().unwrap() as f32;
        for (index, probe) in case["probes"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                wheel.consume(
                    probe["deltaY"].as_f64().unwrap() as f32,
                    probe["pixel"].as_bool().unwrap(),
                    cell_height,
                    probe["shift"].as_bool().unwrap(),
                    probe["alt"].as_bool().unwrap(),
                    probe["ctrl"].as_bool().unwrap(),
                ),
                probe["amount"].as_f64().unwrap() as f32,
                "{} probe {}: {probe}",
                case["name"],
                index + 1,
            );
        }
    }
    Ok(())
}
