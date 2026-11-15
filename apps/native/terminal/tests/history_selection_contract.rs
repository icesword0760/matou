use anyhow::{Result, ensure};
use matou_native_terminal::{
    GridPoint, GridSelection, SelectionDrag, SelectionMode, Terminal, TerminalOptions,
};
use serde_json::Value;

#[test]
fn selection_matches_reference_probes_on_terminal_output() -> Result<()> {
    let data: Value = serde_json::from_str(include_str!(
        "../../desktop/tests/fixtures/terminal-selection-reference.json"
    ))?;
    let point = |value: &Value| GridPoint {
        col: value[0].as_u64().unwrap() as usize,
        row: value[1].as_u64().unwrap() as usize,
    };
    let mut differences = Vec::new();
    for case in data["cases"].as_array().unwrap() {
        let cols = case["cols"].as_u64().unwrap() as u16;
        let rows = case["rows"].as_u64().unwrap() as u16;
        let mut options = TerminalOptions::new(cols, rows, 8, 16);
        options.history_lines = 0;
        let mut terminal = Terminal::new(options)?;
        terminal.feed(case["text"].as_str().unwrap().as_bytes());
        for probe in case["probes"].as_array().unwrap() {
            let mode = match probe["mode"].as_str().unwrap() {
                "word" => SelectionMode::Word,
                "line" => SelectionMode::Line,
                unexpected => panic!("Unexpected selection mode: {unexpected}"),
            };
            let start = point(&probe["point"]);
            let drag = SelectionDrag::begin(&mut terminal, mode, start.row, start.col as f32)?;
            let selection = if let Some(target) = probe.get("target").map(point) {
                drag.extend(&mut terminal, target.row, target.col as f32)?
            } else {
                drag.initial
            };
            let text = selection.text(&mut terminal)?;
            let expected = &probe["expected"];
            let expected_range = GridSelection {
                anchor: point(&expected["start"]),
                end: point(&expected["end"]),
                column: false,
            };
            if selection != expected_range || text != expected["text"].as_str().unwrap() {
                differences.push(format!(
                    "{}: {probe}\n  native: {selection:?}, {text:?}",
                    case["name"]
                ));
            }
        }
    }
    ensure!(differences.is_empty(), "{}", differences.join("\n"));
    Ok(())
}

#[test]
fn selection_crosses_soft_wrapped_and_offscreen_rows_without_moving_the_viewport() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(10, 2, 8, 16))?;
    terminal.feed(b"ab LONGPATH/abcdefghij END\r\n");
    for row in 0..10 {
        terminal.feed(format!("HIST{row:02}\r\n").as_bytes());
    }
    terminal.scroll(i32::MAX)?;
    let top = terminal.screen()?;
    assert_eq!(top.scrollbar.offset, 0);

    let drag = SelectionDrag::begin(&mut terminal, SelectionMode::Word, 0, 7.)?;
    assert_eq!(
        drag.initial,
        GridSelection {
            anchor: GridPoint { row: 0, col: 3 },
            end: GridPoint { row: 2, col: 2 },
            column: false,
        }
    );
    assert!(drag.initial.end.row >= top.rows);
    assert_eq!(drag.initial.text(&mut terminal)?, "LONGPATH/abcdefghij");
    let selection = drag.extend(&mut terminal, 7, 4.)?;
    assert_eq!(selection.end, GridPoint { row: 7, col: 6 });
    let selected = "LONGPATH/abcdefghij END\nHIST00\nHIST01\nHIST02\nHIST03\nHIST04";
    assert_eq!(selection.text(&mut terminal)?, selected);
    assert_eq!(terminal.screen()?, top);

    terminal.scroll(-i32::MAX)?;
    let bottom = terminal.screen()?;
    assert!(bottom.scrollbar.offset > selection.end.row as u64);
    assert_eq!(selection.text(&mut terminal)?, selected);
    assert_eq!(terminal.screen()?, bottom);
    Ok(())
}
