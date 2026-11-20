use anyhow::Result;
use matou_native_terminal::{
    GridPoint, GridSelection, SelectionMode, SelectionState, Terminal, TerminalOptions,
};
use serde_json::Value;

fn point(value: &Value) -> GridPoint {
    GridPoint {
        row: value[1].as_u64().unwrap() as usize,
        col: value[0].as_i64().unwrap() as isize,
    }
}

fn expected_range(value: &Value) -> Option<GridSelection> {
    (!value.is_null()).then(|| GridSelection {
        anchor: point(&value["start"]),
        end: point(&value["end"]),
        column: false,
    })
}

fn mode(value: &Value) -> SelectionMode {
    match value.as_str().unwrap() {
        "character" => SelectionMode::Character,
        "word" => SelectionMode::Word,
        "line" => SelectionMode::Line,
        name => panic!("Unexpected selection mode: {name}"),
    }
}

fn fixture_terminal(case: &Value) -> Result<Terminal> {
    let cols = u16::try_from(case["cols"].as_u64().unwrap())?;
    let rows = u16::try_from(case["rows"].as_u64().unwrap())?;
    let mut options = TerminalOptions::new(cols, rows, 8, 16);
    options.history_lines = 0;
    let mut terminal = Terminal::new(options)?;
    terminal.feed(case["text"].as_str().unwrap().as_bytes());
    Ok(terminal)
}

#[test]
fn selection_state_matches_frozen_model_after_each_action() -> Result<()> {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/selection-state-reference.json"))?;
    for case in data["cases"].as_array().unwrap() {
        let mut terminal = fixture_terminal(case)?;
        let cols = terminal.screen()?.cols;
        let start = point(&case["point"]);
        let mut selection = SelectionState::default();
        assert_eq!(selection.range(cols), None);
        selection.begin(
            &mut terminal,
            mode(&case["mode"]),
            start.row,
            start.col as f32,
        )?;
        assert_eq!(
            selection.range(cols),
            expected_range(&case["initial"]),
            "{} initial",
            case["name"]
        );
        for step in case["steps"].as_array().unwrap() {
            if let Some(end) = step.get("end") {
                selection.set_end(point(end));
            } else if let Some(target) = step.get("incremental") {
                let target = point(target);
                selection.incremental_click(&mut terminal, target.row, target.col as f32)?;
            } else if let Some(amount) = step["trim"].as_u64() {
                selection.trim(usize::try_from(amount)?);
            } else if step["clear"].as_bool() == Some(true) {
                selection.clear();
            } else {
                panic!("Unexpected state action: {step}");
            }
            assert_eq!(
                selection.range(cols),
                expected_range(&step["expected"]),
                "{}: {step}",
                case["name"]
            );
        }
    }
    Ok(())
}

#[test]
fn selection_state_keeps_all_pointer_probe_ranges_and_text() -> Result<()> {
    let data: Value = serde_json::from_str(include_str!(
        "../../desktop/tests/fixtures/terminal-selection-reference.json"
    ))?;
    for case in data["cases"].as_array().unwrap() {
        let mut terminal = fixture_terminal(case)?;
        let cols = terminal.screen()?.cols;
        let mut selection = SelectionState::default();
        for probe in case["probes"].as_array().unwrap() {
            let start = point(&probe["point"]);
            selection.begin(
                &mut terminal,
                mode(&probe["mode"]),
                start.row,
                start.col as f32,
            )?;
            if let Some(target) = probe.get("target").map(point) {
                selection.extend(&mut terminal, target.row, target.col as f32)?;
            }
            let range = selection.range(cols).unwrap();
            assert_eq!(
                Some(range),
                expected_range(&probe["expected"]),
                "{}: {probe}",
                case["name"]
            );
            assert_eq!(
                range.text(&mut terminal)?,
                probe["expected"]["text"].as_str().unwrap(),
                "{}: {probe}",
                case["name"]
            );
        }
    }
    Ok(())
}

#[test]
fn live_history_trim_keeps_the_surviving_word_selected() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 8, 16))?;
    const WORD_ROW: usize = 9_000;
    terminal.feed(&b"history filler\r\n".repeat(WORD_ROW));
    terminal.feed(b"prefix RETAINED suffix\r\n");
    let mut selection = SelectionState::default();
    selection.begin(&mut terminal, SelectionMode::Word, WORD_ROW, 9.)?;
    selection.extend(&mut terminal, WORD_ROW, 9.)?;
    let before = selection.range(80).unwrap();
    assert_eq!(before.text(&mut terminal)?, "RETAINED");
    let trimmed_before = terminal.history_trimmed_rows()?;

    terminal.feed(&b"more history\r\n".repeat(5_000));
    let removed = terminal.history_trimmed_rows()? - trimmed_before;
    assert!(removed > 0 && removed < WORD_ROW);
    selection.trim(removed);
    let after = selection.range(80).unwrap();
    assert_eq!(after.anchor.row, before.anchor.row - removed);
    assert_eq!(after.text(&mut terminal)?, "RETAINED");
    Ok(())
}

#[test]
fn local_scroll_does_not_move_the_selection_rows() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(10, 4, 8, 16))?;
    terminal.feed(b"A\r\nB\r\nC\r\nD");
    let mut selection = SelectionState::default();
    selection.begin(&mut terminal, SelectionMode::Character, 1, 0.)?;
    selection.extend(&mut terminal, 1, 1.)?;
    let before = selection.range(10).unwrap();
    assert_eq!(before.text(&mut terminal)?, "B");
    let trimmed_before = terminal.history_trimmed_rows()?;

    terminal.feed(b"\x1b[2;4r\x1b[4;1H\nE");
    let removed = terminal.history_trimmed_rows()? - trimmed_before;
    assert_eq!(removed, 0);
    selection.trim(removed);
    assert_eq!(selection.range(10), Some(before));
    assert_eq!(before.text(&mut terminal)?, "C");
    Ok(())
}

/// Replay the gate inputs and retain actual results even when the two engines differ.
#[test]
#[ignore = "writes the terminal selection gate evidence"]
fn capture_terminal_selection_gate() -> Result<()> {
    use matou_native_terminal::SelectionAction;
    use serde_json::json;
    use std::{env, fs};

    let inputs: Value = serde_json::from_slice(&fs::read(
        env::var_os("MATOU_SELECTION_GATE_INPUT").expect("set MATOU_SELECTION_GATE_INPUT"),
    )?)?;
    let output =
        env::var_os("MATOU_SELECTION_GATE_OUTPUT").expect("set MATOU_SELECTION_GATE_OUTPUT");
    let dimensions = |grid: &Value| -> Result<(u16, u16)> {
        Ok((
            u16::try_from(grid[0].as_u64().unwrap())?,
            u16::try_from(grid[1].as_u64().unwrap())?,
        ))
    };
    let mut observations = Vec::new();
    for case in inputs["cases"].as_array().unwrap() {
        let (cols, rows) = dimensions(&case["grid"])?;
        let mut options = TerminalOptions::new(cols, rows, 8, 16);
        options.history_lines = usize::try_from(case["history"].as_u64().unwrap_or(0))?;
        let mut terminal = Terminal::new(options)?;
        terminal.feed(case["text"].as_str().unwrap().as_bytes());

        let mut results = Vec::new();
        let initial = json!(["initial"]);
        for (index, step) in std::iter::once(&initial)
            .chain(case["steps"].as_array().unwrap())
            .enumerate()
        {
            let mut operation_error = None;
            match step[0].as_str().unwrap() {
                "initial" => {}
                "down" => {
                    let position = point(&step[2]);
                    let mode = if step[1] == "column" {
                        SelectionMode::Column
                    } else {
                        mode(&step[1])
                    };
                    terminal.selection_action(SelectionAction::Begin {
                        mode,
                        row: position.row,
                        x: position.col as f32,
                    })?;
                }
                "drag" => {
                    let position = point(&step[1]);
                    let row = position.row;
                    let x = position.col as f32;
                    let action = match step[2].as_i64().filter(|amount| *amount != 0) {
                        Some(amount) => SelectionAction::ExtendOutside {
                            row,
                            x,
                            below: amount > 0,
                        },
                        None => SelectionAction::Extend { row, x },
                    };
                    terminal.selection_action(action)?;
                }
                "shift" => {
                    let position = point(&step[1]);
                    terminal.selection_action(SelectionAction::IncrementalClick {
                        row: position.row,
                        x: position.col as f32,
                    })?;
                }
                "setSelection" => {
                    let position = point(&step[1]);
                    terminal.selection_action(SelectionAction::Select {
                        row: position.row,
                        col: position.col,
                        length: step[2].as_i64().unwrap().try_into()?,
                    })?;
                }
                "clearSelection" => terminal.selection_action(SelectionAction::Clear)?,
                "selectAll" => operation_error = Some("no native selectAll entry point"),
                "scroll" => terminal.scroll(i32::try_from(step[1].as_i64().unwrap())?)?,
                "tick" => terminal.selection_action(SelectionAction::Scroll {
                    lines: i32::try_from(step[1].as_i64().unwrap())?,
                })?,
                "resize" => {
                    let (cols, rows) = dimensions(&step[1])?;
                    terminal.resize(cols, rows, 8, 16)?;
                }
                "feed" => {
                    let text = step[1].as_str().unwrap();
                    let count = usize::try_from(step[2].as_u64().unwrap_or(1))?;
                    terminal.feed(text.repeat(count).as_bytes());
                }
                name => anyhow::bail!("Unknown gate action {name}"),
            }

            let range = terminal.selection_range()?;
            let screen = terminal.screen()?;
            results.push(json!({
                "step": index,
                "start": range.map(|range| (range.anchor.col, range.anchor.row)),
                "end": range.map(|range| (range.end.col, range.end.row)),
                "shape": range.map(|range| if range.column { "column" } else { "linear" }),
                "hasSelection": range.is_some_and(|range| range.anchor != range.end),
                "text": terminal.selected_text()?,
                "viewportCursor": screen.cursor_has_position.then_some((screen.cursor_col, screen.cursor_row)),
                "cursorVisible": screen.cursor_visible,
                "retainedRows": screen.scrollbar.total,
                "historyTrimmedRows": terminal.history_trimmed_rows()?,
                "offset": screen.scrollbar.offset,
                "buffer": if screen.alternate { "alternate" } else { "normal" },
                "grid": [screen.cols, screen.rows],
                "operationError": operation_error,
            }));
        }
        observations.push(json!({ "id": case["id"], "results": results }));
    }
    println!(
        "Native Terminal: {} fixtures, {} observations",
        observations.len(),
        observations
            .iter()
            .map(|case| case["results"].as_array().unwrap().len())
            .sum::<usize>()
    );
    fs::write(output, serde_json::to_string_pretty(&observations)? + "\n")?;
    Ok(())
}
