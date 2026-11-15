use anyhow::{Context, Result};
use matou_native_terminal::{
    GridPoint, GridSelection, SearchEngine, SearchMatch, SearchOptions, Terminal, TerminalOptions,
};
use serde_json::Value;

fn point(value: &Value) -> Result<GridPoint> {
    Ok(GridPoint {
        row: usize::try_from(value["y"].as_u64().context("row of selected point")?)?,
        col: isize::try_from(value["x"].as_i64().context("column of selected point")?)?,
    })
}

#[test]
fn retained_buffer_search_matches_frozen_source() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/search-reference.json"))?;
    for case in fixture["cases"].as_array().unwrap() {
        let cols = u16::try_from(case["cols"].as_u64().unwrap())?;
        let rows = u16::try_from(case["rows"].as_u64().unwrap())?;
        let mut terminal_options = TerminalOptions::new(cols, rows, 8, 16);
        terminal_options.history_lines = usize::try_from(case["history"].as_u64().unwrap_or(0))?;
        let mut terminal = Terminal::new(terminal_options)?;
        if let Some(text) = case["text"].as_str() {
            terminal.feed(text.as_bytes());
        } else {
            let generated = &case["generated"];
            let unit = generated["unit"].as_str().unwrap();
            let count = usize::try_from(generated["count"].as_u64().unwrap())?;
            terminal.feed(unit.repeat(count).as_bytes());
        }
        let mut engine = SearchEngine::new(&mut terminal)?;
        for probe in case["probes"].as_array().unwrap() {
            let operation = probe["op"].as_str().unwrap();
            let term = probe["term"].as_str().unwrap();
            let mut options = SearchOptions::default();
            if let Some(value) = probe.get("options") {
                options = serde_json::from_value(value.clone())?;
            }
            let name = &case["name"];
            let expected = &probe["expected"];
            if operation == "all" {
                let actual = engine
                    .all_matches(term, options)
                    .with_context(|| format!("{name}: {probe}"))?;
                if probe["summarize"].as_bool() == Some(true) {
                    assert_eq!(
                        actual.len(),
                        expected["count"].as_u64().unwrap() as usize,
                        "{name}"
                    );
                    let first: SearchMatch = serde_json::from_value(expected["first"].clone())?;
                    let last: SearchMatch = serde_json::from_value(expected["last"].clone())?;
                    assert_eq!(actual.first(), Some(&first), "{name}");
                    assert_eq!(actual.last(), Some(&last), "{name}");
                } else {
                    let expected: Vec<SearchMatch> = serde_json::from_value(expected.clone())?;
                    assert_eq!(actual, expected, "{name}: {probe}");
                }
                continue;
            }

            let previous = &probe["previous"];
            let previous = if previous.is_null() {
                None
            } else {
                Some(GridSelection {
                    anchor: point(&previous["start"])?,
                    end: point(&previous["end"])?,
                    column: false,
                })
            };
            let cached_term = probe["cachedTerm"].as_str();
            let actual = match operation {
                "find" => engine.find(
                    term,
                    usize::try_from(probe["row"].as_u64().unwrap())?,
                    isize::try_from(probe["col"].as_i64().unwrap())?,
                    options,
                ),
                "next" => engine.find_next(term, options, previous, cached_term),
                "previous" => engine.find_previous(term, options, previous, cached_term),
                operation => panic!("unexpected search operation: {operation}"),
            };
            if expected["error"].as_bool() == Some(true) {
                assert!(actual.is_err(), "{name}: {probe}");
            } else {
                let actual = actual.with_context(|| format!("{name}: {probe}"))?;
                let expected: Option<SearchMatch> = serde_json::from_value(expected.clone())?;
                assert_eq!(actual, expected, "{name}: {probe}");
            }
        }
    }
    Ok(())
}
