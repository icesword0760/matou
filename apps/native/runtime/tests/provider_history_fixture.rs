use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;

const FIXTURE: &str =
    include_str!("../../../../tooling/native-parity/fixtures/provider-history-reference.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    source: FrozenSource,
    datasets: BTreeMap<String, Vec<TranscriptFile>>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenSource {
    commit: String,
    inventory: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TranscriptFile {
    provider: String,
    id: String,
    path: Option<String>,
    bucket: Option<String>,
    rows: Option<Vec<Value>>,
    json: Option<Value>,
    mtime_ms: Option<u64>,
    line_ending: Option<String>,
    no_final_newline: Option<bool>,
    zstd: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    description: String,
    input: Input,
    expected: Expected,
    source: Vec<String>,
}

#[derive(Deserialize)]
struct Input {
    category: String,
    datasets: Vec<String>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    operation: Operation,
    projection: Option<String>,
    #[serde(default)]
    fail: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Operation {
    List,
    Detail,
    Search,
    Load,
    ExtractTitle,
    ObserveTitle,
    DeferredPreview,
    ScanJsonl,
    EventWindow,
}

#[derive(Deserialize)]
struct Expected {
    steps: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    start_event_index: usize,
    end_event_index: usize,
    total: usize,
    has_earlier: bool,
    has_later: bool,
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("valid provider-history fixture")
}

fn assert_source(id: &str, source: &str) {
    let (file, range) = source.rsplit_once(':').expect("source must be file:line");
    assert!(
        (file.starts_with("apps/") || file.starts_with("packages/contracts/"))
            && (file.ends_with(".ts") || file.ends_with(".tsx"))
            && file.split('/').all(|part| !matches!(part, "" | "." | "..")),
        "{id}: invalid source path {file}"
    );
    let (start, end) = range.split_once('-').unwrap_or((range, range));
    let start: usize = start.parse().expect("one-based start line");
    let end: usize = end.parse().expect("one-based end line");
    assert!(start > 0 && start <= end, "{id}: invalid range {source}");
}

fn assert_summary(id: &str, summary: &Value) {
    for field in ["providerSessionId", "title", "cwd", "permissionMode"] {
        assert!(summary[field].is_string(), "{id}: missing {field}");
    }
    assert!(
        summary["cwd"]
            .as_str()
            .unwrap()
            .starts_with("/__fixture__/"),
        "{id}: cwd outside synthetic workspace"
    );
    assert!(summary["updatedAt"].is_number(), "{id}: missing time");
    assert!(summary["eventCount"].is_u64(), "{id}: missing event count");
    match summary["availability"].as_str() {
        Some("available") => {
            assert!(summary.get("loadedSessionId").is_none(), "{id}");
            assert!(summary.get("loadedSessionTitle").is_none(), "{id}");
        }
        Some("loaded-here" | "loaded-elsewhere") => {
            assert!(summary["loadedSessionId"].is_string(), "{id}");
            assert!(summary["loadedSessionTitle"].is_string(), "{id}");
        }
        _ => panic!("{id}: invalid availability"),
    }
}

fn assert_event(id: &str, event: &Value, index: usize) {
    assert_eq!(event["index"].as_u64(), Some(index as u64), "{id}");
    assert!(
        matches!(
            event["kind"].as_str(),
            Some("user" | "assistant" | "tool" | "system")
        ),
        "{id}: invalid event kind"
    );
    assert!(
        event["text"].is_string() && event["matched"].is_boolean(),
        "{id}"
    );
    if let Some(name) = event.get("toolName") {
        assert!(name.is_string(), "{id}: invalid tool name");
    }
}

fn assert_detail(id: &str, step: &Step, expected: &Value) {
    let page: Page = serde_json::from_value(expected["page"].clone()).expect("valid event page");
    assert_eq!(
        expected["eventCount"].as_u64(),
        Some(page.total as u64),
        "{id}"
    );
    let empty = page.start_event_index == 0;
    if empty {
        assert_eq!(page.end_event_index, 0, "{id}");
    } else {
        assert!(
            page.start_event_index <= page.end_event_index && page.end_event_index <= page.total,
            "{id}: event indexes outside the page"
        );
    }
    assert_eq!(page.has_earlier, page.start_event_index > 1, "{id}");
    assert_eq!(
        page.has_later,
        !empty && page.end_event_index < page.total,
        "{id}"
    );
    let count = if empty {
        0
    } else {
        page.end_event_index - page.start_event_index + 1
    };
    assert!(count <= 500, "{id}: exceeded the event page cap");

    if step.projection.as_deref() == Some("page") {
        assert_eq!(
            expected["returnedEventCount"].as_u64(),
            Some(count as u64),
            "{id}"
        );
        if empty {
            assert!(
                expected.get("firstEvent").is_none() && expected.get("lastEvent").is_none(),
                "{id}"
            );
        } else {
            assert_event(id, &expected["firstEvent"], page.start_event_index);
            assert_event(id, &expected["lastEvent"], page.end_event_index);
        }
    } else {
        assert_summary(id, expected);
        assert!(expected["transcriptTitle"].is_string(), "{id}");
        let events = expected["events"].as_array().expect("detail events");
        assert_eq!(events.len(), count, "{id}");
        for (i, event) in events.iter().enumerate() {
            assert_event(id, event, page.start_event_index + i);
        }
    }
}

#[test]
fn frozen_provider_history_cases_can_be_loaded_by_native_tests() {
    let fixture = fixture();
    assert_eq!(
        fixture.source.commit,
        "924de28da27999543aeef7a7c9797334fca45d97"
    );
    assert_eq!(
        fixture.source.inventory,
        ["AGT-06", "AGT-25", "AGT-27", "REC-15", "CDX-07", "DSH-05"]
    );

    let mut ids = BTreeSet::new();
    let mut used_datasets = BTreeSet::new();
    let mut categories = [0; 3];
    for case in &fixture.cases {
        let id = &case.id;
        assert!(
            !id.is_empty() && ids.insert(id),
            "invalid or duplicate id: {id}"
        );
        assert!(
            !case.description.trim().is_empty() && !case.source.is_empty(),
            "{id}"
        );
        for source in &case.source {
            assert_source(id, source);
        }
        let category = match case.input.category.as_str() {
            "normal" => 0,
            "boundary" => 1,
            "failure" => 2,
            _ => panic!("{id}: unknown category"),
        };
        categories[category] += 1;

        let mut file_ids = BTreeSet::new();
        for dataset in &case.input.datasets {
            let files = fixture
                .datasets
                .get(dataset)
                .unwrap_or_else(|| panic!("{id}: missing dataset {dataset}"));
            used_datasets.insert(dataset);
            assert!(!files.is_empty(), "{id}: empty dataset {dataset}");
            for file in files {
                assert!(
                    file_ids.insert((&file.provider, &file.id)),
                    "{id}: duplicate file"
                );
                assert!(
                    matches!(
                        file.provider.as_str(),
                        "claude-code" | "codex" | "dsh" | "antigravity"
                    ),
                    "{id}"
                );
                assert_ne!(
                    file.rows.is_some(),
                    file.json.is_some(),
                    "{id}: ambiguous encoding"
                );
                for path in [file.path.as_deref(), file.bucket.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    assert!(
                        path.split('/').all(|part| !matches!(part, "" | "." | "..")),
                        "{id}: invalid relative path"
                    );
                }
                assert!(
                    matches!(file.line_ending.as_deref(), None | Some("\n" | "\r\n")),
                    "{id}"
                );
                if file.no_final_newline == Some(true) {
                    assert!(file.rows.is_some(), "{id}");
                }
                if file.zstd == Some(true) {
                    assert_eq!(file.provider, "dsh", "{id}");
                }
                if let Some(mtime_ms) = file.mtime_ms {
                    assert!(mtime_ms > 0, "{id}: invalid fixture mtime");
                }
            }
        }

        assert!(!case.input.steps.is_empty(), "{id}: no steps");
        assert_eq!(case.input.steps.len(), case.expected.steps.len(), "{id}");
        for (step, expected) in case.input.steps.iter().zip(&case.expected.steps) {
            assert!(expected.is_object(), "{id}: expected an object");
            assert_eq!(step.fail, expected.get("error").is_some(), "{id}");
            if step.fail {
                assert!(expected["error"]["name"].is_string(), "{id}");
                assert!(expected["error"]["message"].is_string(), "{id}");
                continue;
            }
            match step.operation {
                Operation::List => {
                    let sessions = expected["sessions"].as_array().expect("list sessions");
                    for session in sessions {
                        assert_summary(id, session);
                    }
                }
                Operation::Detail => assert_detail(id, step, expected),
                Operation::Search => assert!(expected["hits"].is_array(), "{id}"),
                Operation::Load => {
                    let binding = &expected["binding"];
                    let load = &expected["load"];
                    assert!(binding["providerSessionId"].is_string(), "{id}");
                    assert_eq!(
                        binding["providerSessionId"], load["providerSessionId"],
                        "{id}"
                    );
                    assert_eq!(binding["provider"], load["provider"], "{id}");
                }
                Operation::ExtractTitle => {
                    for field in ["aiTitle", "fallbackTitle"] {
                        assert!(expected.get(field).is_none_or(Value::is_string), "{id}");
                    }
                }
                Operation::ObserveTitle => assert!(expected["providerTitle"].is_string(), "{id}"),
                Operation::DeferredPreview => {
                    assert!(
                        expected["items"].as_array().expect("preview items").len() <= 4,
                        "{id}"
                    );
                    assert!(
                        expected["latestLines"]
                            .as_array()
                            .expect("graph lines")
                            .len()
                            <= 4,
                        "{id}"
                    );
                }
                Operation::ScanJsonl => assert!(expected["lines"].is_array(), "{id}"),
                Operation::EventWindow => assert!(expected["events"].is_array(), "{id}"),
            }
        }
    }
    assert!(categories.iter().all(|&count| count >= 4), "{categories:?}");
    assert_eq!(used_datasets.len(), fixture.datasets.len());
}

#[test]
fn codex_byte_boundary_survives_json_exchange_without_object_key_order() {
    let fixture = fixture();
    let files = &fixture.datasets["codex-chain"];
    let child = &files[0];
    let parent = &files[1];
    let base = &child.rows.as_ref().unwrap()[0]["payload"]["history_base"];
    assert_eq!(base["thread_id"].as_str(), Some(parent.id.as_str()));

    let rows = parent.rows.as_ref().unwrap();
    let line_at_boundary = rows[1].as_str().expect("raw UTF-8 JSONL row");
    let end = rows[0].as_str().unwrap().len() + 1 + line_at_boundary.len();
    assert_eq!(base["end_byte_offset"].as_u64(), Some(end as u64));
    assert!(line_at_boundary.contains("祖先线索 🧩"));
    assert!(line_at_boundary.len() > line_at_boundary.chars().count());
}
