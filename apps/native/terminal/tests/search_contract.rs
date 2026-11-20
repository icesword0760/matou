use std::{env, fs};

use anyhow::{Context, Result};
use matou_native_terminal::{
    GridPoint, GridSelection, SearchEngine, SearchMatch, SearchOptions, Terminal, TerminalOptions,
};
use serde_json::{Value, json};

// Exercise the live runtime state machine against deterministic terminal bytes.
#[allow(dead_code)]
#[path = "../../runtime/src/search.rs"]
mod session_search;

fn point(value: &Value) -> Result<GridPoint> {
    Ok(GridPoint {
        row: usize::try_from(value["y"].as_u64().context("row of selected point")?)?,
        col: isize::try_from(value["x"].as_i64().context("column of selected point")?)?,
    })
}

fn terminal(case: &Value) -> Result<Terminal> {
    let cols = u16::try_from(case["cols"].as_u64().unwrap())?;
    let rows = u16::try_from(case["rows"].as_u64().unwrap())?;
    let mut options = TerminalOptions::new(cols, rows, 8, 16);
    options.history_lines = usize::try_from(case["history"].as_u64().unwrap_or(0))?;
    let mut terminal = Terminal::new(options)?;
    if let Some(text) = case["text"].as_str() {
        terminal.feed(text.as_bytes());
    } else {
        let generated = &case["generated"];
        let unit = generated["unit"].as_str().unwrap();
        let count = usize::try_from(generated["count"].as_u64().unwrap())?;
        terminal.feed(unit.repeat(count).as_bytes());
    }
    Ok(terminal)
}

fn options(probe: &Value) -> Result<SearchOptions> {
    match probe.get("options") {
        Some(value) => Ok(serde_json::from_value(value.clone())?),
        None => Ok(SearchOptions::default()),
    }
}

fn buffer_probe(engine: &mut SearchEngine<'_>, probe: &Value) -> Result<Value> {
    let operation = probe["op"].as_str().unwrap();
    let term = probe["term"].as_str().unwrap();
    let options = options(probe)?;
    if operation == "all" {
        return Ok(match engine.all_matches(term, options) {
            Ok(matches) if probe["summarize"].as_bool() == Some(true) => json!({
                "count": matches.len(),
                "first": matches.first(),
                "last": matches.last(),
            }),
            Ok(matches) => json!(matches),
            Err(error) => json!({ "error": error.to_string() }),
        });
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
    let actual: Result<Option<SearchMatch>> = match operation {
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
    Ok(match actual {
        Ok(found) => json!(found),
        Err(error) => json!({ "error": error.to_string() }),
    })
}

#[test]
fn retained_buffer_search_matches_frozen_source() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/search-reference.json"))?;
    for case in fixture["cases"].as_array().unwrap() {
        let mut terminal = terminal(case)?;
        let mut engine = SearchEngine::new(&mut terminal)?;
        for probe in case["probes"].as_array().unwrap() {
            let actual = buffer_probe(&mut engine, probe)?;
            let expected = &probe["expected"];
            if expected["error"].as_bool() == Some(true) {
                assert!(actual["error"].is_string(), "{}: {probe}", case["name"]);
            } else {
                assert_eq!(&actual, expected, "{}: {probe}", case["name"]);
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "capture via K-search-policy/run.sh"]
fn capture_search_policy() -> Result<()> {
    let fixture: Value = serde_json::from_slice(&fs::read(
        env::var_os("MATOU_SEARCH_INPUT").context("MATOU_SEARCH_INPUT")?,
    )?)?;
    let mut buffer = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let mut terminal = terminal(case)?;
        let mut engine = SearchEngine::new(&mut terminal)?;
        let outcomes = case["probes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|probe| buffer_probe(&mut engine, probe))
            .collect::<Result<Vec<_>>>()?;
        buffer.push(json!({ "name": case["name"], "outcomes": outcomes }));
    }

    let mut sessions = Vec::new();
    for case in fixture["sessionCases"].as_array().unwrap() {
        let mut terminal = terminal(case)?;
        let mut state = session_search::SessionSearch::default();
        let mut outcomes = Vec::new();
        for (index, probe) in case["probes"].as_array().unwrap().iter().enumerate() {
            let request = session_search::SearchRequest {
                sequence: u64::try_from(index + 1)?,
                query: probe["term"].as_str().unwrap().to_owned(),
                options: options(probe)?,
                previous: probe["op"] == "previous",
            };
            let actual = match state.search(&mut terminal, request) {
                Ok(result) => json!({
                    "sequence": result.sequence,
                    "matches": result.matches,
                    "active": result.active,
                    "resultIndex": result.result_index,
                    "resultCount": result.matches.len(),
                }),
                Err(error) => json!({ "error": error.to_string() }),
            };
            outcomes.push(actual);
        }
        sessions.push(json!({ "name": case["name"], "outcomes": outcomes }));
    }
    let path = env::var_os("MATOU_SEARCH_OUTPUT").context("MATOU_SEARCH_OUTPUT")?;
    let output = json!({ "cases": buffer, "sessionCases": sessions });
    fs::write(path, serde_json::to_string_pretty(&output)? + "\n")?;
    Ok(())
}
