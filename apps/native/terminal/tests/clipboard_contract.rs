use std::borrow::Cow;

use anyhow::Result;
use matou_native_terminal::decode_clipboard;
use serde_json::Value;

#[test]
fn clipboard_decode_matches_frozen_osc_handler() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/clipboard-reference.json"))?;
    for case in fixture["cases"].as_array().unwrap() {
        let content = if let Some(content) = case["content"].as_str() {
            Cow::Borrowed(content)
        } else {
            let pattern = &case["generated"];
            let count = usize::try_from(pattern["count"].as_u64().unwrap())?;
            Cow::Owned(format!(
                "{}{}{}",
                pattern["prefix"].as_str().unwrap(),
                pattern["unit"].as_str().unwrap().repeat(count),
                pattern["suffix"].as_str().unwrap(),
            ))
        };
        let name = &case["name"];
        assert_eq!(
            content.encode_utf16().count(),
            usize::try_from(case["contentUnits"].as_u64().unwrap())?,
            "{name}"
        );
        assert_eq!(
            decode_clipboard(&content).as_deref(),
            case["text"].as_str(),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn clipboard_stream_matches_frozen_parser() -> Result<()> {
    use matou_native_terminal::{Terminal, TerminalOptions};
    let fixture: Value = serde_json::from_str(include_str!("fixtures/clipboard-reference.json"))?;
    for case in fixture["streams"].as_array().unwrap() {
        let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 7, 14))?;
        let mut writes = Vec::new();
        for chunk in case["chunks"].as_array().unwrap() {
            // Split at every byte, including within UTF-8 and terminators.
            for byte in chunk.as_str().unwrap().as_bytes() {
                terminal.feed(&[*byte]);
                writes.extend(terminal.drain_clipboard());
            }
        }
        assert_eq!(
            serde_json::to_value(writes)?,
            case["writes"],
            "{}",
            case["name"]
        );
        assert!(
            terminal.drain_responses().is_empty(),
            "queries must not receive clipboard data"
        );
    }
    Ok(())
}
