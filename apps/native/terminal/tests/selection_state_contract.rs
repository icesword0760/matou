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
