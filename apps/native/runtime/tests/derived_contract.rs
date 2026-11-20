use std::{env, fs};

use anyhow::{Context, Result, bail};
use matou_native_runtime::derived::{
    CwdTracker, WorkStatusTracker, terminal_summary_lines, utf16_tail,
};
use serde_json::{Value, json};

const GROUPS: [&str; 5] = ["work", "cwd", "summary", "tail", "bufferedSummary"];

fn string(field: &Value) -> Result<&str> {
    field.as_str().context("fixture field must be a string")
}

fn array(field: &Value) -> Result<&[Value]> {
    field
        .as_array()
        .map(Vec::as_slice)
        .context("fixture field must be an array")
}

fn capture_chunks<T: serde::Serialize>(
    case: &Value,
    mut ingest: impl FnMut(&str) -> T,
) -> Result<Value> {
    let results = array(&case["chunks"])?
        .iter()
        .map(|chunk| Ok(ingest(string(chunk)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!(results))
}

fn derive(group: &str, case: &Value) -> Result<Value> {
    match group {
        "work" => {
            let mut tracker = WorkStatusTracker::default();
            capture_chunks(case, |chunk| tracker.ingest(chunk))
        }
        "cwd" => {
            let mut tracker = CwdTracker::default();
            capture_chunks(case, |chunk| tracker.ingest(chunk))
        }
        "summary" => Ok(json!(terminal_summary_lines(string(&case["raw"])?))),
        "tail" => {
            let limit = case["limit"].as_u64().context("fixture suffix limit")?;
            Ok(json!(utf16_tail(
                string(&case["raw"])?,
                usize::try_from(limit)?
            )))
        }
        "bufferedSummary" => {
            // The owner keeps this bound private. Read the production call's
            // literal so a changed bound reaches this probe as well.
            let (_, source) = include_str!("../src/owner.rs")
                .split_once("summary.raw = utf16_tail(&summary.raw, ")
                .context("owner summary window")?;
            let (limit, _) = source.split_once(");").context("owner summary limit")?;
            let limit = limit.replace('_', "").parse::<usize>()?;
            let mut raw = String::new();
            capture_chunks(case, |chunk| {
                raw.push_str(chunk);
                raw = utf16_tail(&raw, limit);
                terminal_summary_lines(&raw)
            })
        }
        _ => bail!("unknown fixture group: {group}"),
    }
}

#[test]
fn shell_trackers_and_summary_match_frozen_reference() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/derived-reference.json"))?;
    for group in &GROUPS[..4] {
        for case in array(&fixture[group])? {
            assert_eq!(
                derive(group, case)?,
                case["expected"],
                "{group}: {}",
                case["name"]
            );
        }
    }
    Ok(())
}

// The gate compares fresh native and frozen captures outside this test so it
// can record every difference, including lone UTF-16 surrogates on the JS side.
#[test]
#[ignore = "run by the K-pty-derived gate with a fixed input file"]
fn capture_native_gate_cases() -> Result<()> {
    let input = env::var_os("MATOU_DERIVED_INPUT").context("MATOU_DERIVED_INPUT")?;
    let output = env::var_os("MATOU_DERIVED_OUTPUT").context("MATOU_DERIVED_OUTPUT")?;
    let fixture: Value = serde_json::from_slice(&fs::read(input)?)?;
    let mut capture = serde_json::Map::new();
    for group in GROUPS {
        let mut records = Vec::new();
        for case in array(&fixture[group])? {
            let name = string(&case["name"])?;
            let value = derive(group, case).with_context(|| format!("{group}/{name}"))?;
            records.push(json!({ "name": name, "value": value, "error": null }));
        }
        capture.insert(group.to_owned(), json!(records));
    }
    fs::write(output, serde_json::to_string_pretty(&capture)? + "\n")?;
    Ok(())
}
