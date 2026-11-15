use anyhow::{Context, Result};
use matou_native_runtime::derived::{
    CwdTracker, WorkStatusTracker, terminal_summary_lines, utf16_tail,
};
use serde_json::{Value, json};

fn string(field: &Value) -> Result<&str> {
    field.as_str().context("fixture field must be a string")
}

fn array(field: &Value) -> Result<&[Value]> {
    field
        .as_array()
        .map(Vec::as_slice)
        .context("fixture field must be an array")
}

#[test]
fn shell_trackers_and_summary_match_frozen_reference() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/derived-reference.json"))?;
    for case in array(&fixture["work"])? {
        let mut tracker = WorkStatusTracker::default();
        let statuses = array(&case["chunks"])?
            .iter()
            .map(|chunk| Ok(tracker.ingest(string(chunk)?)))
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(
            json!(statuses),
            case["expected"],
            "work status: {}",
            case["name"]
        );
    }
    for case in array(&fixture["cwd"])? {
        let mut tracker = CwdTracker::default();
        let directories = array(&case["chunks"])?
            .iter()
            .map(|chunk| Ok(tracker.ingest(string(chunk)?)))
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(
            json!(directories),
            case["expected"],
            "working directory: {}",
            case["name"]
        );
    }
    for case in array(&fixture["summary"])? {
        assert_eq!(
            json!(terminal_summary_lines(string(&case["raw"])?)),
            case["expected"],
            "terminal summary: {}",
            case["name"]
        );
    }
    for case in array(&fixture["tail"])? {
        let limit = case["limit"].as_u64().context("fixture suffix limit")?;
        assert_eq!(
            utf16_tail(string(&case["raw"])?, usize::try_from(limit)?),
            string(&case["expected"])?,
            "UTF-16 suffix: {}",
            case["name"]
        );
    }
    Ok(())
}
