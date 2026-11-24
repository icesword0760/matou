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

#[test]
#[ignore = "writes gate evidence to the explicit paths supplied by run.sh"]
fn capture_terminal_mouse_gate() -> Result<()> {
    use matou_native_terminal::{SelectionAction, SelectionMode, Terminal, TerminalOptions};
    use serde_json::json;
    use std::{env, fs};

    let input_path = env::var("MATOU_MOUSE_GATE_INPUT")?;
    let output_path = env::var("MATOU_MOUSE_GATE_OUTPUT")?;
    let fixture: Value = serde_json::from_slice(&fs::read(input_path)?)?;
    let mut output = Vec::new();

    for case in fixture["cases"].as_array().unwrap() {
        let cols = u16::try_from(case["cols"].as_u64().unwrap())?;
        let rows = u16::try_from(case["rows"].as_u64().unwrap())?;
        let mut terminal = Terminal::new(TerminalOptions::new(cols, rows, 8, 16))?;
        let mut reporter = MouseReporter::default();
        let mut previous_epoch = terminal.input_modes()?.input_epoch;
        let mut observations = Vec::new();

        for step in case["steps"].as_array().unwrap() {
            if let Some(input) = step["input"].as_str() {
                terminal.feed(input.as_bytes());
            }
            let mut report = Vec::new();
            let mut pty_input = Vec::new();
            let mut selection_swallowed = None;
            let mut cells = None;
            let mut mode_names = None;
            let observed = (|| -> Result<String> {
                let modes = terminal.input_modes()?;
                mode_names = Some((
                    protocol_name(modes.mouse_protocol),
                    encoding_name(modes.mouse_encoding),
                ));
                if modes.input_epoch != previous_epoch {
                    reporter = MouseReporter::default();
                    previous_epoch = modes.input_epoch;
                }

                if step["select"].as_bool() == Some(true) {
                    terminal.selection_action(SelectionAction::Select {
                        row: 0,
                        col: 0,
                        length: 5,
                    })?;
                } else if let Some(count) = step["cells"].as_u64() {
                    let screen = terminal.screen()?;
                    let count = usize::try_from(count)?;
                    cells = Some(
                        screen.cells[..count]
                            .iter()
                            .map(|cell| (cell.text.clone(), cell.width, cell.has_content))
                            .collect::<Vec<_>>(),
                    );
                } else if !step["event"].is_null() {
                    let event = mouse_event(&step["event"])?;
                    let click = step["click"].as_bool() == Some(true);
                    if click {
                        // Mac's surface uses no force-selection modifier with the
                        // frozen defaults. Use the real selection API after routing.
                        let captured = modes.mouse_protocol != MouseProtocol::None;
                        selection_swallowed = Some(captured);
                        if !captured {
                            terminal.selection_action(SelectionAction::Begin {
                                mode: SelectionMode::Word,
                                row: usize::try_from(event.row)?,
                                x: event.x as f32 / 8.,
                            })?;
                        }
                    }
                    for event in [
                        Some(event),
                        click.then_some(MouseEvent { action: 0, ..event }),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if let Some(bytes) = reporter.report(
                            modes.mouse_protocol,
                            modes.mouse_encoding,
                            event,
                            usize::from(cols),
                            usize::from(rows),
                        ) {
                            report.extend_from_slice(&bytes);
                            // The desktop forwards SGR only. The session clears
                            // the selection before writing forwarded bytes.
                            if modes.mouse_encoding != MouseEncoding::Default {
                                pty_input.extend_from_slice(&bytes);
                                terminal.selection_action(SelectionAction::Clear)?;
                            }
                        }
                    }
                }
                terminal.selected_text()
            })();

            let (selection, error) = match observed {
                Ok(text) => (Some(text), None),
                Err(error) => (None, Some(format!("{error:#}"))),
            };
            observations.push(json!({
                "id": step["id"],
                "protocol": mode_names.map(|(protocol, _)| protocol),
                "encoding": mode_names.map(|(_, encoding)| encoding),
                "report": report,
                "ptyInput": pty_input,
                "accepted": null,
                "selectionSwallowed": selection_swallowed,
                "selection": selection,
                "cells": cells,
                "diagnostics": null,
                "error": error,
            }));
        }
        output.push(json!({ "id": case["id"], "observations": observations }));
    }

    fs::write(
        output_path,
        format!("{}\n", serde_json::to_string_pretty(&output)?),
    )?;
    Ok(())
}

fn protocol_name(protocol: MouseProtocol) -> &'static str {
    match protocol {
        MouseProtocol::None => "NONE",
        MouseProtocol::X10 => "X10",
        MouseProtocol::Vt200 => "VT200",
        MouseProtocol::Drag => "DRAG",
        MouseProtocol::Any => "ANY",
    }
}

fn encoding_name(encoding: MouseEncoding) -> &'static str {
    match encoding {
        MouseEncoding::Default => "DEFAULT",
        MouseEncoding::Sgr => "SGR",
        MouseEncoding::SgrPixels => "SGR_PIXELS",
    }
}
