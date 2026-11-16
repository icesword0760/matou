use std::{
    fs, thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use matou_native_runtime::{
    history::{HistoryCursor, HistorySearchOptions},
    history_worker::{HistoryReply, HistoryRequest, HistoryWorker},
};
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
struct Probe {
    op: String,
    input: Value,
    expected: Value,
}

#[test]
fn reads_disk_history_and_reports_original_request_metadata() -> Result<()> {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/history-search-reference.json"))?;
    let root = tempfile::tempdir()?;
    let mut worker = HistoryWorker::new(root.path().to_path_buf())?;
    assert!(worker.try_recv()?.is_none());

    for case in fixture.cases {
        let directory = root.path().join("journal").join(&case.session_id);
        fs::create_dir_all(&directory)?;
        for segment in case.segments {
            fs::write(directory.join(segment.name), decode_base64(&segment.base64))?;
        }

        let recorded_search = case
            .probes
            .iter()
            .find(|probe| probe.op == "search" && probe.input["query"] == "test")
            .context("frozen whole-word search")?;
        let mut expected_context = case
            .probes
            .iter()
            .find(|probe| probe.op == "page" && probe.input == json!({}))
            .context("frozen full history page")?
            .expected
            .clone();
        let cursor: HistoryCursor = serde_json::from_value(
            case.probes
                .iter()
                .find(|probe| probe.expected.get("anchorIndex").is_some())
                .context("frozen context anchor")?
                .input["around"]
                .clone(),
        )?;
        // This fixture fits within the worker's default context limits. Its
        // full page supplies the original lines and gaps for the same cursor.
        let anchor = expected_context["lines"]
            .as_array()
            .context("frozen full history lines")?
            .iter()
            .position(|line| line["cursor"] == json!(cursor))
            .context("context cursor in full frozen history")?;
        expected_context["anchorIndex"] = json!(anchor);
        expected_context["hasMoreBefore"] = json!(false);
        expected_context["hasMoreAfter"] = json!(false);

        let search = |sequence, query: &str, options| HistoryRequest::Search {
            session_id: case.session_id.clone(),
            sequence,
            query: query.into(),
            options,
        };
        worker.send(search(
            11,
            "test",
            serde_json::from_value(recorded_search.input["options"].clone())?,
        ))?;
        worker.send(search(
            12,
            "[",
            HistorySearchOptions {
                regex: true,
                ..HistorySearchOptions::default()
            },
        ))?;
        worker.send(HistoryRequest::Context {
            session_id: case.session_id.clone(),
            sequence: 13,
            cursor,
        })?;

        let HistoryReply::Search {
            session_id,
            sequence,
            result,
        } = next_reply(&worker)?
        else {
            panic!("expected successful search reply");
        };
        assert_eq!(session_id, case.session_id);
        assert_eq!(sequence, 11);
        assert_eq!(
            json!(result.unwrap()),
            recorded_search.expected,
            "{}",
            case.variant
        );

        let HistoryReply::Search {
            session_id,
            sequence,
            result,
        } = next_reply(&worker)?
        else {
            panic!("expected invalid search reply");
        };
        assert_eq!(session_id, case.session_id);
        assert_eq!(sequence, 12);
        assert!(
            result
                .unwrap_err()
                .starts_with("invalid terminal history search pattern")
        );

        let HistoryReply::Context {
            session_id,
            sequence,
            cursor: echoed_cursor,
            result,
        } = next_reply(&worker)?
        else {
            panic!("expected context reply");
        };
        assert_eq!(session_id, case.session_id);
        assert_eq!(sequence, 13);
        assert_eq!(echoed_cursor, cursor);
        assert_eq!(json!(result.unwrap()), expected_context, "{}", case.variant);
    }

    worker.shutdown()?;
    assert!(worker.try_recv().is_err());
    Ok(())
}

fn next_reply(worker: &HistoryWorker) -> Result<HistoryReply> {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(reply) = worker.try_recv()? {
            return Ok(reply);
        }
        ensure!(
            Instant::now() < deadline,
            "journal history worker did not report a reply"
        );
        thread::sleep(Duration::from_millis(5));
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
