use anyhow::Result;
use matou_native_core::claude_role_budget::{
    CLAUDE_ROLES, ClaudeBudgetCompaction, ClaudeBudgetTarget, claude_role_budget,
};
use serde_json::{Value, json};

const FIXTURE: &str =
    include_str!("../../../../tooling/native-parity/fixtures/provider-assignment-reference.json");

#[test]
fn frozen_role_budget_fixtures() -> Result<()> {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let mut cases_checked = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        if !id.starts_with("launch-budget-") {
            continue;
        }
        let input = &case["input"];
        let preferences = &input["preferences"];
        let providers: Vec<_> = input["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|profile| {
                let id = profile
                    .as_str()
                    .or_else(|| profile["profile"].as_str())
                    .unwrap();
                let mut provider = fixture["setup"]["providers"][id].clone();
                if let Some(overrides) = profile["overrides"].as_object() {
                    provider.as_object_mut().unwrap().extend(overrides.clone());
                }
                provider
            })
            .collect();
        let provider = |id| providers.iter().find(|row| row["id"] == id).unwrap();
        let active = input["activeProvider"]
            .as_str()
            .map_or(&providers[0], provider);
        let roles = CLAUDE_ROLES.map(|role| {
            let assignment = &preferences["claudeRoleAssignments"][role];
            match assignment["providerId"].as_str() {
                Some(id) => target(provider(id), assignment["model"].as_str().unwrap()),
                None => target(active, active["modelMapping"][role].as_str().unwrap()),
            }
        });
        let compact = &preferences["compaction"];
        let compaction = match compact["mode"].as_str().unwrap() {
            "follow" => ClaudeBudgetCompaction::Follow,
            "custom" => ClaudeBudgetCompaction::Custom(target(
                provider(compact["providerId"].as_str().unwrap()),
                compact["model"].as_str().unwrap(),
            )),
            mode => panic!("{id}: unexpected compaction mode {mode}"),
        };
        let budget = claude_role_budget(
            roles,
            compaction,
            preferences["contextWindows"].as_object().unwrap(),
        );
        let expected = &case["expected"];
        if let Some(message) = expected["error"]["message"].as_str() {
            assert_eq!(budget.unwrap_err().to_string(), message, "{id}");
        } else {
            let budget = budget.unwrap_or_else(|error| panic!("{id}: {error}"));
            assert_eq!(
                json!(budget.context_window),
                expected["value"]["contextWindow"],
                "{id}"
            );
            assert_eq!(
                json!(budget.unknown_context_roles),
                expected["value"]["unknownContextRoles"],
                "{id}"
            );
            assert_eq!(
                json!(budget.unknown_compaction_context),
                expected["value"]["unknownCompactionContext"],
                "{id}"
            );
        }
        cases_checked += 1;
    }
    assert_eq!(cases_checked, 9);
    Ok(())
}

#[test]
fn maximum_falls_back_to_standard_and_checks_tiers_before_compaction() -> Result<()> {
    let target = ClaudeBudgetTarget {
        provider_id: "api",
        provider_name: "Fixture API",
        context_window: Some(123_456),
        max_context_window: None,
    };
    let modes = json!({"api": "maximum"});
    let modes = modes.as_object().unwrap();
    let budget = claude_role_budget([target; 3], ClaudeBudgetCompaction::Follow, modes)?;
    assert_eq!(budget.context_window, Some(123_456));

    let small = ClaudeBudgetTarget {
        max_context_window: Some(99_999),
        ..target
    };
    let error = claude_role_budget(
        [target, small, small],
        ClaudeBudgetCompaction::Custom(small),
        modes,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "sonnet · Fixture API: context window is under 100K"
    );
    Ok(())
}

fn target<'a>(provider: &'a Value, model: &str) -> ClaudeBudgetTarget<'a> {
    let metadata = provider["modelMetadata"]
        .as_array()
        .and_then(|models| models.iter().find(|item| item["id"] == model));
    ClaudeBudgetTarget {
        provider_id: provider["id"].as_str().unwrap(),
        provider_name: provider["name"].as_str().unwrap(),
        context_window: metadata.and_then(|item| item["contextWindow"].as_u64()),
        max_context_window: metadata.and_then(|item| item["maxContextWindow"].as_u64()),
    }
}
