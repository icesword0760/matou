use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;

const FIXTURE: &str =
    include_str!("fixtures/provider-events-reference.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    source: FrozenSource,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenSource {
    commit: String,
    inventory: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    description: String,
    input: Value,
    expected: Value,
    source: Vec<String>,
}

#[test]
fn frozen_provider_cases_can_be_loaded_by_native_tests() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("valid provider-events fixture");
    assert_eq!(
        fixture.source.commit,
        "924de28da27999543aeef7a7c9797334fca45d97"
    );
    assert_eq!(
        fixture.source.inventory,
        ["AGT-02", "AGT-04", "AGT-11", "CDX-02", "DSH-02"]
    );

    let mut ids = BTreeSet::new();
    let mut categories = [0; 3];
    for case in &fixture.cases {
        let id = &case.id;
        assert!(
            !id.is_empty() && ids.insert(id),
            "invalid or duplicate id: {id}"
        );
        assert!(
            !case.description.trim().is_empty(),
            "{id}: missing description"
        );

        let category = match case.input["category"].as_str() {
            Some("normal") => 0,
            Some("boundary") => 1,
            Some("failure") => 2,
            _ => panic!("{id}: unknown category"),
        };
        categories[category] += 1;

        let (input_field, output_field) = match case.input["operation"].as_str() {
            Some("provider-hook" | "renderer-notification") => ("steps", "steps"),
            Some("terminal-input") => ("chunks", "steps"),
            Some("terminal-output") => ("chunks", "statuses"),
            _ => panic!("{id}: unknown operation"),
        };
        let inputs = case.input[input_field]
            .as_array()
            .unwrap_or_else(|| panic!("{id}: missing {input_field} input array"));
        let outputs = case.expected[output_field]
            .as_array()
            .unwrap_or_else(|| panic!("{id}: missing {output_field} output array"));
        assert!(!inputs.is_empty(), "{id}: missing stimuli");
        assert_eq!(
            outputs.len(),
            inputs.len(),
            "{id}: expected one result per stimulus"
        );

        assert!(!case.source.is_empty(), "{id}: missing frozen provenance");
        for source in &case.source {
            let (file, lines) = source.rsplit_once(':').expect("source must be file:line");
            let (root, _) = file.split_once('/').expect("source must be repo-relative");
            assert!(
                file.ends_with(".ts")
                    && matches!(root, "apps" | "packages")
                    && !file.split('/').any(|part| part.is_empty() || part == ".."),
                "{id}: invalid source path {file}"
            );
            let (start, end) = lines.split_once('-').unwrap_or((lines, lines));
            let start: usize = start.parse().expect("one-based start line");
            let end: usize = end.parse().expect("one-based end line");
            assert!(
                start > 0 && start <= end,
                "{id}: invalid source range {source}"
            );
        }
    }
    assert!(categories.iter().all(|&count| count >= 3), "{categories:?}");
}

#[test]
fn frozen_split_surrogate_survives_the_json_exchange() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("valid provider-events fixture");
    let case = fixture
        .cases
        .iter()
        .find(|case| case.id == "permission-utf16-split-at-179")
        .expect("permission truncation fixture");
    let event = &case.expected["steps"][0]["notifications"][0]["event"];
    assert!(event.get("body").is_none());
    let units: Vec<u16> =
        serde_json::from_value(event["bodyUtf16"].clone()).expect("unsigned UTF-16 units");
    assert_eq!(units.len(), 180);
    assert_eq!(units[178..], [0xd83d, 0x2026]);
    assert!(String::from_utf16(&units).is_err());
}
