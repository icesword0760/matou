use anyhow::Result;
use matou_native_terminal::{GridPoint, GridSelection, SelectionAction, Terminal, TerminalOptions};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    cols: u16,
    rows: u16,
    text: String,
    probes: Vec<Probe>,
}

#[derive(Deserialize)]
struct Probe {
    name: String,
    row: usize,
    col: isize,
    length: isize,
    expected: Expected,
}

#[derive(Deserialize)]
struct Expected {
    start: (isize, usize),
    end: (isize, usize),
    text: String,
}

fn point((col, row): (isize, usize)) -> GridPoint {
    GridPoint { row, col }
}

#[test]
fn programmatic_selection_matches_frozen_source() -> Result<()> {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "fixtures/programmatic-selection-reference.json"
    ))?;
    let mut options = TerminalOptions::new(fixture.cols, fixture.rows, 8, 16);
    options.history_lines = 0;
    let mut terminal = Terminal::new(options)?;
    terminal.feed(fixture.text.as_bytes());

    for probe in fixture.probes {
        terminal.selection_action(SelectionAction::Select {
            row: probe.row,
            col: probe.col,
            length: probe.length,
        })?;
        assert_eq!(
            terminal.selection_range()?,
            Some(GridSelection {
                anchor: point(probe.expected.start),
                end: point(probe.expected.end),
                column: false,
            }),
            "{}",
            probe.name
        );
        assert_eq!(
            terminal.selected_text()?,
            probe.expected.text,
            "{}",
            probe.name
        );
    }
    Ok(())
}
