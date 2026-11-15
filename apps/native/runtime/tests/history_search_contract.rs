use std::fs;

use matou_native_runtime::history::{HistoryPageInput, HistorySearchInput, JournalHistoryReader};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    variant: String,
    session_id: String,
    segments: Vec<Segment>,
    probes: Vec<Probe>,
}

#[derive(Deserialize)]
struct Segment {
    name: String,
    base64: String,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Probe {
    Page {
        input: HistoryPageInput,
        expected: Value,
    },
    Search {
        input: HistorySearchInput,
        expected: Value,
    },
}

#[test]
fn matches_frozen_journal_history_reader() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/history-search-reference.json")).unwrap();
    let root = tempfile::tempdir().unwrap();
    let reader = JournalHistoryReader::new(root.path());

    for case in fixture.cases {
        let directory = root.path().join("journal").join(&case.session_id);
        fs::create_dir_all(&directory).unwrap();
        for segment in case.segments {
            fs::write(directory.join(segment.name), decode_base64(&segment.base64)).unwrap();
        }

        for (index, probe) in case.probes.into_iter().enumerate() {
            let (actual, expected) = match probe {
                Probe::Page { input, expected } => {
                    let page = reader.page(&case.session_id, input).unwrap();
                    (json!(page), expected)
                }
                Probe::Search { input, expected } => {
                    let result = reader.search(&case.session_id, input);
                    if let Some(message) = expected.get("error").and_then(Value::as_str) {
                        // V8 and regress describe invalid pattern syntax differently.
                        let category = "invalid terminal history search pattern";
                        assert!(message.starts_with(category));
                        assert!(result.unwrap_err().to_string().starts_with(category));
                        continue;
                    }
                    (json!(result.unwrap()), expected)
                }
            };
            assert_eq!(actual, expected, "{} probe {index}", case.variant);
        }
    }
}

fn decode_base64(encoded: &str) -> Vec<u8> {
    fn sextet(byte: u8) -> u8 {
        match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("invalid fixture encoding"),
        }
    }

    let mut decoded = Vec::with_capacity(encoded.len() / 4 * 3);
    let chunks = encoded.as_bytes().chunks_exact(4);
    assert!(chunks.remainder().is_empty());
    for chunk in chunks {
        let first = sextet(chunk[0]);
        let second = sextet(chunk[1]);
        decoded.push(first << 2 | second >> 4);
        if chunk[2] != b'=' {
            let third = sextet(chunk[2]);
            decoded.push(second << 4 | third >> 2);
            if chunk[3] != b'=' {
                decoded.push(third << 6 | sextet(chunk[3]));
            }
        }
    }
    decoded
}
