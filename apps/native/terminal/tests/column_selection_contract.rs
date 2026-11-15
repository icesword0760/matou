use anyhow::Result;
use matou_native_terminal::{
    GridPoint, GridSelection, SelectionAction, SelectionMode, Terminal, TerminalOptions,
};
use serde_json::Value;

fn point(value: &Value) -> GridPoint {
    GridPoint {
        col: value[0].as_u64().unwrap() as usize,
        row: value[1].as_u64().unwrap() as usize,
    }
}

#[test]
fn column_selection_matches_frozen_source_on_terminal_cells() -> Result<()> {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/column-selection-reference.json"))?;
    for case in fixture["cases"].as_array().unwrap() {
        let cols = u16::try_from(case["cols"].as_u64().unwrap())?;
        let rows = u16::try_from(case["rows"].as_u64().unwrap())?;
        let column = case["column"].as_bool().unwrap();
        for probe in case["probes"].as_array().unwrap() {
            let mut options = TerminalOptions::new(cols, rows, 8, 16);
            options.history_lines = usize::try_from(case["history"].as_u64().unwrap_or(0))?;
            let mut terminal = Terminal::new(options)?;
            terminal.feed(case["text"].as_str().unwrap().as_bytes());
            let start = point(&probe["start"]);
            terminal.selection_action(SelectionAction::Begin {
                mode: if column {
                    SelectionMode::Column
                } else {
                    SelectionMode::Character
                },
                row: start.row,
                x: start.col as f32,
            })?;
            for step in probe["steps"].as_array().unwrap() {
                let action = if let Some(target) = step.get("move") {
                    let target = point(target);
                    match step["outside"].as_i64() {
                        Some(outside) => SelectionAction::ExtendOutside {
                            row: target.row,
                            x: target.col as f32,
                            below: outside > 0,
                        },
                        None => SelectionAction::Extend {
                            row: target.row,
                            x: target.col as f32,
                        },
                    }
                } else if let Some(lines) = step["scroll"].as_i64() {
                    SelectionAction::Scroll {
                        lines: i32::try_from(lines)?,
                    }
                } else if let Some(target) = step.get("incremental") {
                    let target = point(target);
                    SelectionAction::IncrementalClick {
                        row: target.row,
                        x: target.col as f32,
                    }
                } else {
                    panic!("Unexpected column selection action: {step}");
                };

                terminal.selection_action(action)?;
                let expected = &step["expected"];
                let name = &case["name"];
                assert_eq!(
                    terminal.selection_range()?,
                    Some(GridSelection {
                        anchor: point(&expected["start"]),
                        end: point(&expected["end"]),
                        column,
                    }),
                    "{name}: {step}"
                );
                assert_eq!(
                    terminal.screen()?.scrollbar.offset,
                    expected["offset"].as_u64().unwrap(),
                    "{name}: {step}"
                );
                assert_eq!(
                    terminal.selected_text()?,
                    expected["text"].as_str().unwrap(),
                    "{name}: {step}"
                );
            }
        }
    }
    Ok(())
}
