use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use anyhow::Result;
use matou_native_core::provider_config::{ProviderConfigLocale, ProviderConfigStore};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

const CLAUDE: &str = "claude-code";
const CODEX: &str = "codex";
const OFFICIAL: &str = "anthropic-official";
const FILE: &str = "provider-config.json";
const TEMP: &str = "provider-config.json.tmp";
const SAMPLE: &[u8] =
    include_bytes!("fixtures/provider-config-reference.json");
const API_ID: &str = "6c3e5c40-5795-4fdf-968e-a3de83097aa6";
const SUB_ID: &str = "ee8a641e-f247-48ed-9ad1-3d175c8693d9";
const KEY: &str = "SYNTHETIC-FIXTURE-ONLY";
const OTHER_KEY: &str = "SYNTHETIC-ROTATED-ONLY";
const CREDENTIAL: &str = "fixture-only.json";

fn temp() -> TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

fn root(temp: &TempDir) -> PathBuf {
    temp.path().join("isolated").join("data")
}

fn store(root: &Path) -> ProviderConfigStore {
    ProviderConfigStore::new(root, ProviderConfigLocale::ZhCn)
}

fn file(root: &Path) -> Vec<u8> {
    fs::read(root.join(FILE)).unwrap()
}

fn state(root: &Path) -> Value {
    serde_json::from_slice(&file(root)).unwrap()
}

fn edit_disk(root: &Path, value: &Value) -> Result<()> {
    fs::create_dir_all(root)?;
    fs::write(root.join(FILE), serde_json::to_vec(value)?)?;
    Ok(())
}

fn api() -> Value {
    json!({
        "cli": CLAUDE,
        "name": "Test provider",
        "endpoint": "https://example.invalid/base",
        "model": "demo-a"
    })
}

fn subscription(provider: &str) -> Value {
    json!({
        "name": "  订阅示例  ",
        "modelMapping": {
            "opus": "demo-sub",
            "sonnet": "demo-sub",
            "haiku": "demo-sub"
        },
        "effortMapping": { "opus": "follow" },
        "subscription": {
            "provider": provider,
            "credentialRef": { "name": CREDENTIAL },
            "account": { "status": "ready", "plan": "Fixture", "reason": "discard" },
            "modelMetadata": [{
                "id": "demo-sub", "displayName": "示例订阅模型", "contextWindow": 272000,
                "maxContextWindow": 512000, "supportedEfforts": ["high"],
                "maxOutputTokens": 4096, "resolvedModel": "discard"
            }]
        }
    })
}

fn assert_redacted(text: &str) {
    for sensitive in [KEY, OTHER_KEY, CREDENTIAL] {
        assert!(!text.contains(sensitive), "a synthetic credential leaked");
    }
}

fn assert_public(value: &Value) {
    let text = value.to_string();
    assert_redacted(&text);
    assert!(!text.contains("\"apiKey\""));
    assert!(!text.contains("\"credentialRef\""));
}

fn assert_revision(store: &ProviderConfigStore, revision: u64) -> Result<()> {
    assert_eq!(store.snapshot()?["revision"].as_u64(), Some(revision));
    Ok(())
}

fn rejected_without_write(root: &Path, result: Result<Value>, expected: &str, before: &[u8]) {
    let error = result.expect_err("mutation should fail");
    assert_redacted(&format!("{error:?}"));
    assert_eq!(error.to_string(), expected);
    assert!(file(root) == before, "failed write changed the file");
    assert!(!root.join(TEMP).exists());
}

#[test]
fn replays_the_frozen_writer_sample_byte_for_byte() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let ids = Mutex::new([API_ID, SUB_ID].into_iter());
    let store = ProviderConfigStore::with_sources(
        &root,
        ProviderConfigLocale::ZhCn,
        move || Uuid::parse_str(ids.lock().unwrap().next().unwrap()).unwrap(),
        || UNIX_EPOCH + Duration::from_secs(946_684_800),
    );

    let seed = store.snapshot()?;
    assert_eq!(seed["revision"], 1);
    assert_eq!(seed["providers"][CLAUDE][0]["model"], "CLI 默认");
    assert!(!root.exists());

    let created = store.upsert(&json!({
        "cli": CLAUDE,
        "name": "  隔离示例  ",
        "endpoint": " HTTPS://EXAMPLE.INVALID:443/base/ ",
        "model": " demo-a ",
        "apiKey": format!(" {KEY} "),
        "modelMapping": { "opus": " demo-a ", "haiku": " demo-z " },
        "effortMapping": { "opus": "high", "sonnet": "follow" },
        "modelMetadata": [
            { "id": "demo-z" }, { "id": "demo-a" },
            { "id": "demo-a", "displayName": "示例模型", "contextWindow": 200000,
              "maxContextWindow": 1000000, "maxOutputTokens": 4096,
              "supportedEfforts": ["high", "follow", "high", "low"], "extra": "discard" },
            { "id": "demo-a", "displayName": "later" }
        ]
    }))?;
    assert_eq!(created["id"], API_ID);
    assert_public(&created);
    assert_revision(&store, 2)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&root)?.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(root.parent().unwrap())?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.join(FILE))?.permissions().mode() & 0o777,
            0o600
        );
    }
    let mut external = state(&root);
    external["legacyRoot"] = json!({ "untouched": true });
    external["providers"][CLAUDE][1]["legacyProvider"] = json!({ "keep": 7 });
    external["activeProviderIds"][CLAUDE] = json!("missing-active");
    external["policies"] = json!({
        "legacyPolicy": "retained",
        "antigravityRiskAcceptedAt": "2000-01-01T00:00:00.000Z"
    });
    edit_disk(&root, &external)?;
    let before_read = file(&root);
    assert_eq!(store.snapshot()?["activeProviderIds"][CLAUDE], OFFICIAL);
    assert!(file(&root) == before_read);

    let edited = store.upsert(&json!({
        "cli": CLAUDE, "id": API_ID, "name": "隔离示例（编辑）",
        "endpoint": "https://example.invalid/base", "model": "demo-a", "apiKey": "  "
    }))?;
    assert_eq!(edited["hasApiKey"], true);
    assert_revision(&store, 3)?;
    store.confirm_antigravity_risk()?;
    assert_revision(&store, 4)?;
    let adopted = store.adopt_subscription(&subscription("codex"))?;
    assert_eq!(adopted["id"], SUB_ID);
    assert_public(&adopted);
    assert_revision(&store, 5)?;
    let activated = store.set_active(CLAUDE, API_ID)?;
    assert_public(&activated);
    assert_eq!(activated["revision"], 6);

    let bytes = file(&root);
    assert_eq!(bytes.len(), 2728);
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        "8819ecaf2b472e340a058b18ac26c33fede14a218e0f17b4c0b21cc7fb2b7aa5"
    );
    assert!(
        bytes == SAMPLE,
        "native bytes differ from the frozen sample"
    );
    assert!(bytes.starts_with(b"{\n  \"version\": 1,\n") && bytes.ends_with(b"\n}\n"));
    assert!(!bytes.contains(&b'\r'));
    assert!(!root.join(TEMP).exists());
    Ok(())
}

#[test]
fn edits_the_frozen_file_in_place_and_preserves_other_cli_and_nested_legacy_data() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    fs::create_dir_all(&root)?;
    fs::write(root.join(FILE), SAMPLE)?;
    let store = store(&root);
    let read = store.snapshot()?;
    assert_public(&read);
    assert_eq!(read["revision"], 6);
    let mut before = state(&root);
    // Intentionally insert non-index keys ahead of integer-index keys at several depths.
    before["providers"]["future-cli"] = json!({ "z": [{ "b": 1, "10": 2, "2": 3 }] });
    before["activeProviderIds"]["future-cli"] = json!("future-id");
    before["providers"][CLAUDE][1]["legacyProvider"]["nested"] = json!({ "z": -0.0, "4294967295": 1e21, "01": 1e-7, "10": 1e-6, "2": 1.0, "4294967294": 9007199254740993_u64 });
    before["legacyRoot"]["precise"] = json!([-3.597847523160892e-242, 2.547114365375239e-8]);
    before["providers"][CLAUDE][2]["subscription"]["legacy"] = json!({ "keep": [true] });
    before["providers"][CLAUDE][2]["subscription"]["account"]["apiKey"] = json!(KEY);
    before["providers"][CLAUDE][2]["subscription"]["modelMetadata"][0]["credentialRef"] =
        json!(CREDENTIAL);
    edit_disk(&root, &before)?;
    let mut input = api();
    input["id"] = json!(API_ID);
    store.upsert(&input)?;

    let reread = ProviderConfigStore::new(&root, ProviderConfigLocale::En).snapshot()?;
    assert_public(&reread);
    assert_eq!(reread["revision"], 7);
    assert_eq!(reread["activeProviderIds"]["future-cli"], "future-id");
    assert_eq!(reread["providers"][CODEX][0]["name"], "OpenAI 官方");
    let after = state(&root);
    for (left, right) in [
        (&after["legacyRoot"], &before["legacyRoot"]),
        (&after["policies"], &before["policies"]),
        (&after["activeProviderIds"], &before["activeProviderIds"]),
        (
            &after["providers"]["future-cli"],
            &before["providers"]["future-cli"],
        ),
        (&after["providers"][CODEX], &before["providers"][CODEX]),
        (
            &after["providers"][CLAUDE][2],
            &before["providers"][CLAUDE][2],
        ),
    ] {
        assert!(left == right, "untouched configuration was modified");
    }
    assert_eq!(after["providers"][CLAUDE][1]["legacyProvider"]["keep"], 7);
    let written = String::from_utf8(file(&root))?;
    let numeric = "\"nested\": {\n            \"2\": 1,\n            \"10\": 0.000001,\n            \"4294967294\": 9007199254740992,\n            \"z\": 0,\n            \"4294967295\": 1e+21,\n            \"01\": 1e-7";
    assert!(
        written.contains(numeric),
        "integer index order or number format differs"
    );
    assert!(written.contains("\"2\": 3,\n          \"10\": 2,\n          \"b\": 1"));
    Ok(())
}

#[test]
fn endpoint_sentinel_and_key_rules_keep_failures_off_disk() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let store = store(&root);
    for (endpoint, expected) in [
        (
            " HTTPS://EXAMPLE.INVALID:443/base/ ",
            "https://example.invalid/base",
        ),
        (
            "http://example.invalid:080/a//",
            "http://example.invalid/a/",
        ),
        (
            "https://example.invalid:8443/?q=/",
            "https://example.invalid:8443/?q=",
        ),
        (
            "https://example.invalid/path/#fragment/",
            "https://example.invalid/path/#fragment",
        ),
        (
            "https://example.invalid/?q= 'x'#a b",
            "https://example.invalid/?q=%20%27x%27#a%20b",
        ),
        (
            "https://BÜCHER.invalid\\a\\..\\b/",
            "https://xn--bcher-kva.invalid/b",
        ),
        ("https://127.1:443/%2E/path", "https://127.0.0.1/path"),
        ("https://:@example.invalid", "https://example.invalid"),
    ] {
        let view = store.upsert(&json!({
            "kind": "api-key", "cli": CLAUDE, "name": "\u{feff} Test \u{feff}",
            "endpoint": endpoint, "model": " __cli_default__ ", "ignored": true
        }))?;
        assert_eq!(view["name"], "Test");
        assert_eq!(view["model"], "CLI 默认");
        assert_eq!(view["endpoint"], expected);
        assert_eq!(view["hasApiKey"], false);
        assert!(view.get("kind").is_none());
        let disk = state(&root);
        let last = disk["providers"][CLAUDE]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(last["model"], "");
        assert!(last.get("kind").is_none() && last.get("ignored").is_none());
    }

    let mut input = api();
    input["apiKey"] = json!(format!(" {KEY} "));
    let created = store.upsert(&input)?;
    input["id"] = created["id"].clone();
    input["apiKey"] = json!("  ");
    input["endpoint"] = json!(" HTTPS://EXAMPLE.INVALID:443/base/ ");
    assert_public(&store.upsert(&input)?);
    assert!(
        state(&root)["providers"][CLAUDE]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["apiKey"]
            == KEY
    );
    let before = file(&root);
    input["endpoint"] = json!("https://example.invalid/updated");
    rejected_without_write(
        &root,
        store.upsert(&input),
        "Endpoint changed: enter the API key for the new service",
        &before,
    );
    for key in [
        format!("{KEY} x"),
        format!("{KEY}中"),
        format!("{KEY}\u{7f}"),
        format!("{KEY}\nx"),
    ] {
        input["apiKey"] = json!(key);
        rejected_without_write(
            &root,
            store.upsert(&input),
            "API Key contains invalid characters",
            &before,
        );
    }
    input["apiKey"] = json!(OTHER_KEY);
    assert_public(&store.upsert(&input)?);
    assert!(
        state(&root)["providers"][CLAUDE]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["apiKey"]
            == OTHER_KEY
    );

    let before = file(&root);
    for endpoint in [
        "ftp://example.invalid".to_owned(),
        "/relative".into(),
        format!("https://{KEY}:secret@example.invalid"),
        format!("https://:{KEY}@example.invalid"),
    ] {
        input["endpoint"] = json!(endpoint);
        rejected_without_write(
            &root,
            store.upsert(&input),
            "API 地址需要使用有效的 HTTP 或 HTTPS 地址",
            &before,
        );
    }
    input["endpoint"] = json!("https://example.invalid");
    input["model"] = json!("   ");
    rejected_without_write(&root, store.upsert(&input), "默认模型不能为空", &before);
    input["name"] = json!("\u{feff}");
    rejected_without_write(&root, store.upsert(&input), "供应商名称不能为空", &before);
    Ok(())
}

#[test]
fn fable_mappings_round_trip_and_clear_for_api_and_subscription() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let config = store(&root);
    let mut api = api();
    api["apiKey"] = json!(KEY);
    api["modelMapping"] = json!({"opus": "old-opus", "sonnet": "old-sonnet", "haiku": "old-haiku"});
    let legacy_api = config.upsert(&api)?;
    let mut subscription_input = subscription("codex");
    subscription_input["subscription"]["modelMetadata"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "fable-special", "supportedEfforts": ["low"]}));
    let legacy_subscription = config.adopt_subscription(&subscription_input)?;
    assert!(legacy_api["modelMapping"].get("fable").is_none());
    assert!(legacy_subscription["modelMapping"].get("fable").is_none());

    api["id"] = legacy_api["id"].clone();
    api["modelMapping"]["fable"] = json!(" fable-special ");
    api["effortMapping"] = json!({"fable": "xhigh"});
    let mut edit = json!({
        "id": legacy_subscription["id"], "cli": CLAUDE, "kind": "subscription",
        "name": "Existing subscription", "modelMapping": legacy_subscription["modelMapping"],
        "effortMapping": {"fable": "high"}
    });
    edit["modelMapping"]["fable"] = json!("fable-special");
    assert!(config.upsert(&edit).is_err());
    edit["effortMapping"]["fable"] = json!("low");
    for (input, effort) in [(&mut api, "xhigh"), (&mut edit, "low")] {
        let saved = config.upsert(input)?;
        let disk = state(&root);
        let stored = disk["providers"][CLAUDE]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == saved["id"])
            .unwrap();
        for row in [&saved, stored] {
            assert_eq!(row["modelMapping"]["fable"], "fable-special");
            assert_eq!(row["effortMapping"]["fable"], effort);
        }
        input["modelMapping"]["fable"] = json!("");
        input["effortMapping"] = json!({});
        let cleared = config.upsert(input)?;
        assert!(cleared["modelMapping"].get("fable").is_none());
        assert!(cleared["effortMapping"].get("fable").is_none());
    }
    Ok(())
}

#[test]
fn model_directory_and_role_settings_follow_whitelists_and_replacement_rules() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let store = store(&root);
    let mut input = api();
    input["apiKey"] = json!(KEY);
    input["modelMapping"] = json!({ "haiku": "  z  ", "unknown": "discard", "opus": "a" });
    input["reasoningEffort"] = json!("max");
    input["effortMapping"] = json!({ "haiku": "follow", "opus": "xhigh" });
    input["modelMetadata"] = json!([
        {"id": "z"}, {"id": "a"}, {"id": "a", "displayName": " detail ",
            "contextWindow": 9007199254740991_u64, "maxContextWindow": 3.0, "maxOutputTokens": 0,
            "supportedEfforts": ["max", "follow", "low", "max", 1], "extra": KEY},
        {"id": "a", "maxOutputTokens": 10}, {"id": "invalid\u{85}"},
        {"id": " token ", "displayName": "bad\u{9f}", "contextWindow": 9007199254740992_u64,
            "maxContextWindow": -4, "maxOutputTokens": 3.4},
        {"id": "   "}, {"id": "𐀀"}, {"id": "\u{e000}"}, {"id": "😀".repeat(121)}, null
    ]);
    let view = store.upsert(&input)?;
    assert_public(&view);
    let metadata = json!([
        {"id": " token "},
        {"id": "a", "displayName": " detail ", "contextWindow": 9007199254740991_u64,
            "maxContextWindow": 3, "supportedEfforts": ["max", "low"]},
        {"id": "z"}, {"id": "𐀀"}, {"id": "\u{e000}"}
    ]);
    assert_eq!(view["modelMetadata"], metadata);
    assert_eq!(
        view["modelMapping"],
        json!({ "opus": "a", "sonnet": "", "haiku": "z" })
    );
    assert_eq!(view["reasoningEffort"], "max");
    assert_eq!(
        view["effortMapping"],
        json!({ "haiku": "follow", "opus": "xhigh" })
    );
    input = api();
    input["id"] = view["id"].clone();
    assert_eq!(store.upsert(&input)?["modelMetadata"], metadata);
    input["modelMetadata"] = json!(false);
    assert_eq!(store.upsert(&input)?["modelMetadata"], metadata);
    input["apiKey"] = json!(OTHER_KEY);
    assert!(store.upsert(&input)?.get("modelMetadata").is_none());
    input["modelMetadata"] = json!([]);
    assert_eq!(store.upsert(&input)?["modelMetadata"], json!([]));
    input["modelMetadata"] = json!([{"id":"fresh"}]);
    input["modelMapping"] = json!({});
    input["effortMapping"] = json!({});
    let view = store.upsert(&input)?;
    assert_eq!(view["modelMetadata"], json!([{"id":"fresh"}]));
    assert_eq!(
        view["modelMapping"],
        json!({"opus":"", "sonnet":"", "haiku":""})
    );
    assert_eq!(view["effortMapping"], json!({}));
    input["modelMetadata"] = Value::Array(
        (0..1510)
            .rev()
            .map(|id| json!({"id": format!("{id:04}")}))
            .collect(),
    );
    let view = store.upsert(&input)?;
    let limited = view["modelMetadata"].as_array().unwrap();
    assert_eq!(limited.len(), 1500);
    assert_eq!(limited[0]["id"], "0000");
    assert_eq!(limited[1499]["id"], "1499");
    let before = file(&root);
    for (key, bad, expected) in [
        ("reasoningEffort", json!("HIGH"), "Invalid reasoning effort"),
        (
            "effortMapping",
            json!({"wrong":"max"}),
            "Invalid role reasoning effort",
        ),
        (
            "effortMapping",
            json!({"opus":"default"}),
            "Invalid role reasoning effort",
        ),
        ("effortMapping", json!([]), "Invalid role reasoning effort"),
        (
            "effortMapping",
            json!(null),
            "Invalid role reasoning effort",
        ),
        (
            "modelMapping",
            json!({"opus": null}),
            "Invalid model mapping",
        ),
    ] {
        let mut invalid = api();
        invalid[key] = bad;
        rejected_without_write(&root, store.upsert(&invalid), expected, &before);
    }
    Ok(())
}

#[test]
fn reads_fallback_and_repairs_v1_only_in_memory_until_mutation() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let store = ProviderConfigStore::new(&root, ProviderConfigLocale::En);
    fs::create_dir_all(&root)?;
    for value in [
        json!("legacy"),
        json!({}),
        json!({"version": "1", "providers": {}, "activeProviderIds": {}}),
        json!({"version": 2, "revision": 40, "providers": {}, "activeProviderIds": {}}),
        json!({"version": 1, "providers": false, "activeProviderIds": {}}),
        json!({"version": 1, "providers": {}, "activeProviderIds": ""}),
    ] {
        edit_disk(&root, &value)?;
        let original = file(&root);
        let view = store.snapshot()?;
        assert_eq!(view["revision"], 1);
        assert_eq!(view["providers"][CODEX][0]["name"], "OpenAI (official)");
        assert!(file(&root) == original);
    }
    let from_default = store.set_active(CODEX, "openai-official")?;
    assert_eq!(from_default["revision"], 2);
    assert_eq!(state(&root)["version"], 1);

    let row = json!({ "id": "same", "cli": CLAUDE, "name": "Original", "endpoint": "https://example.invalid", "model": "", "modelMetadata": {"legacy": true} });
    let mut twin = row.clone();
    twin["name"] = json!("Second row");
    twin["modelMetadata"] = json!([{"id": "b", "discard": true}, {"id": "a"}]);
    let legacy = json!({
        "version": 1.0, "revision": 14, "legacyRoot": ["preserve"],
        "providers": { CLAUDE: [row, twin], CODEX: [] },
        "activeProviderIds": { CLAUDE: "gone", CODEX: "gone" },
        "policies": { "untouched": {"x": 1} }
    });
    edit_disk(&root, &legacy)?;
    let original = file(&root);
    let view = store.snapshot()?;
    assert_eq!(view["revision"], 14);
    assert_eq!(
        view["activeProviderIds"],
        json!({CLAUDE: "same", CODEX: "openai-official"})
    );
    assert!(view["providers"][CLAUDE][0].get("modelMetadata").is_none());
    assert_eq!(
        view["providers"][CLAUDE][1]["modelMetadata"],
        json!([{"id":"a"},{"id":"b"}])
    );
    assert!(file(&root) == original);
    let mut input = api();
    input["id"] = json!("same");
    store.upsert(&input)?;
    let disk = state(&root);
    assert_eq!(disk["revision"], 15);
    assert_eq!(disk["legacyRoot"], legacy["legacyRoot"]);
    assert_eq!(disk["policies"], legacy["policies"]);
    assert_eq!(disk["providers"][CLAUDE].as_array().unwrap().len(), 2);
    assert_eq!(disk["providers"][CLAUDE][0]["name"], "Test provider");
    assert_eq!(disk["providers"][CLAUDE][1]["name"], "Second row");
    assert!(disk["providers"][CLAUDE][0].get("modelMetadata").is_none());
    Ok(())
}

#[test]
fn increments_same_active_and_enforces_delete_order_and_subscription_boundary() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let store = store(&root);

    let first = store.set_active(CLAUDE, OFFICIAL)?;
    assert_eq!(first["revision"], 2);
    let mut official = api();
    official["id"] = json!(OFFICIAL);
    official["apiKey"] = json!(KEY);
    let edited = store.upsert(&official)?;
    assert_public(&edited);
    assert_eq!(edited["id"], OFFICIAL);
    assert_eq!(edited["name"], "Test provider");
    assert_eq!(edited["endpoint"], "https://example.invalid/base");
    assert_eq!(edited["builtIn"], true);
    assert_revision(&store, 3)?;
    let before = file(&root);
    rejected_without_write(
        &root,
        store.delete(CLAUDE, "missing"),
        "供应商配置不存在",
        &before,
    );
    rejected_without_write(
        &root,
        store.delete(CLAUDE, OFFICIAL),
        "使用中的供应商需要先切换后再删除",
        &before,
    );
    rejected_without_write(
        &root,
        store.set_active(CLAUDE, "missing"),
        "供应商配置不存在",
        &before,
    );
    rejected_without_write(
        &root,
        store.upsert(&json!({"kind":"subscription"})),
        "Subscription requires a completed sign-in",
        &before,
    );
    official["id"] = json!("ANTHROPIC-OFFICIAL");
    rejected_without_write(&root, store.upsert(&official), "供应商配置不存在", &before);
    official["id"] = json!(OFFICIAL);
    official["cli"] = json!(CODEX);
    rejected_without_write(&root, store.upsert(&official), "供应商配置不存在", &before);
    assert_revision(&store, 3)?;

    let api_view = store.upsert(&api())?;
    let api_id = api_view["id"].as_str().unwrap();
    assert_eq!(
        Uuid::parse_str(api_id)?.get_version(),
        Some(uuid::Version::Random)
    );
    let adopted = store.adopt_subscription(&subscription("codex"))?;
    let sub_id = adopted["id"].as_str().unwrap();
    assert_eq!(store.set_active(CLAUDE, api_id)?["revision"], 6);
    assert_eq!(store.set_active(CLAUDE, api_id)?["revision"], 7);
    let before = file(&root);
    rejected_without_write(
        &root,
        store.delete(CLAUDE, OFFICIAL),
        "官方供应商配置保留为默认入口",
        &before,
    );
    rejected_without_write(
        &root,
        store.delete(CLAUDE, api_id),
        "使用中的供应商需要先切换后再删除",
        &before,
    );
    let mut input = api();
    input["id"] = json!(sub_id);
    rejected_without_write(
        &root,
        store.upsert(&input),
        "Cannot change a subscription into an API key provider",
        &before,
    );
    assert_eq!(store.delete(CLAUDE, sub_id)?["revision"], 8);
    assert_eq!(store.set_active(CLAUDE, OFFICIAL)?["revision"], 9);
    assert_eq!(store.delete(CLAUDE, api_id)?["revision"], 10);
    assert_revision(&store, 10)?;
    Ok(())
}

#[test]
fn context_mode_save_keeps_other_preferences_and_rolls_back_both_writes() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let store = store(&root);
    let mut db = rusqlite::Connection::open_in_memory()?;
    db.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE preferences (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at INTEGER);
         CREATE TABLE parent (id INTEGER PRIMARY KEY);
         CREATE TABLE child (id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);",
    )?;
    let prior_preferences = json!({
        "contextWindows": {"another-provider": "default"},
        "compaction": {"mode": "follow", "effort": "follow"},
        "claudeRoleAssignments": {},
        "otherField": true,
    });
    db.execute(
        "INSERT INTO preferences VALUES ('agent.modelPreferences', ?, 1)",
        [prior_preferences.to_string()],
    )?;
    let prefs = |db: &rusqlite::Connection| -> Result<Value> {
        let raw: String = db.query_row(
            "SELECT value_json FROM preferences WHERE key = 'agent.modelPreferences'",
            [],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&raw)?)
    };
    let mut input = api();
    input["contextWindowMode"] = json!("512k");

    db.execute_batch(
        "CREATE TRIGGER reject_commit AFTER UPDATE ON preferences
         BEGIN INSERT INTO child VALUES (1); END;",
    )?;
    assert!(
        store
            .upsert_with_context_window(&mut db, &input, 2)
            .is_err()
    );
    assert!(!root.join(FILE).exists());
    assert_eq!(prefs(&db)?, prior_preferences);
    let existing = store.upsert(&api())?;
    let before = file(&root);
    input["id"] = existing["id"].clone();
    input["name"] = json!("Draft");
    assert!(
        store
            .upsert_with_context_window(&mut db, &input, 2)
            .is_err()
    );
    assert_eq!(file(&root), before);
    assert_eq!(prefs(&db)?, prior_preferences);
    db.execute_batch("DROP TRIGGER reject_commit")?;

    fs::create_dir(root.join(TEMP))?;
    assert!(
        store
            .upsert_with_context_window(&mut db, &input, 2)
            .is_err()
    );
    assert_eq!(file(&root), before);
    assert_eq!(prefs(&db)?, prior_preferences);
    fs::remove_dir(root.join(TEMP))?;

    let result = store.upsert_with_context_window(&mut db, &input, 2)?;
    assert_eq!(result["contextWindowMode"], "512k");
    assert_eq!(result["name"], "Draft");
    let mut expected = prior_preferences;
    expected["contextWindows"][existing["id"].as_str().unwrap()] = json!("512k");
    assert_eq!(prefs(&db)?, expected);
    assert!(state(&root)["providers"][CLAUDE][1]["contextWindowMode"].is_null());
    Ok(())
}

#[test]
fn old_revision_values_follow_javascript_addition_without_read_repair() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let store = store(&root);
    for (initial, next) in [
        (None, Value::Null),
        (Some(Value::Null), json!(1)),
        (Some(json!("45")), json!("451")),
        (Some(json!(true)), json!(2)),
        (Some(json!(false)), json!(1)),
        (Some(json!(2.5)), json!(3.5)),
        (
            Some(json!([2, null, {"legacy":true}])),
            json!("2,,[object Object]1"),
        ),
        (Some(json!({"legacy":true})), json!("[object Object]1")),
        (
            Some(json!(9007199254740992_u64)),
            json!(9007199254740992_u64),
        ),
    ] {
        let mut document = json!({"version": 1, "providers": {}, "activeProviderIds": {}});
        if let Some(revision) = &initial {
            document["revision"] = revision.clone();
        }
        edit_disk(&root, &document)?;
        assert_eq!(store.snapshot()?.get("revision"), initial.as_ref());
        assert_eq!(store.set_active(CLAUDE, OFFICIAL)?["revision"], next);
        assert_eq!(state(&root)["revision"], next);
    }
    Ok(())
}

#[test]
fn subscription_adoption_preserves_identity_and_policy_and_filters_new_fields() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let times = AtomicUsize::new(0);
    let store = ProviderConfigStore::with_sources(
        &root,
        ProviderConfigLocale::ZhCn,
        Uuid::new_v4,
        move || {
            assert_eq!(times.fetch_add(1, Ordering::Relaxed), 0);
            UNIX_EPOCH + Duration::from_millis(951_827_696_789)
        },
    );
    let mut input = subscription("antigravity");
    assert_eq!(
        store.adopt_subscription(&input).unwrap_err().to_string(),
        "Antigravity risk confirmation is required"
    );
    assert!(!root.exists());
    store.confirm_antigravity_risk()?;
    assert_eq!(
        state(&root)["policies"]["antigravityRiskAcceptedAt"],
        "2000-02-29T12:34:56.789Z"
    );
    store.confirm_antigravity_risk()?;
    assert_revision(&store, 3)?;
    let new = store.adopt_subscription(&input)?;
    assert_public(&new);
    assert_eq!(new["kind"], "subscription");
    assert_eq!(new["model"], "demo-sub");
    assert!(
        !new["subscription"]["account"]
            .as_object()
            .unwrap()
            .contains_key("reason")
    );
    assert!(
        !new["subscription"]["modelMetadata"][0]
            .as_object()
            .unwrap()
            .contains_key("resolvedModel")
    );
    assert_eq!(store.snapshot()?["activeProviderIds"][CLAUDE], OFFICIAL);
    let mut external = state(&root);
    external["providers"][CLAUDE][1]["legacyRow"] = json!(true);
    external["providers"][CLAUDE][1]["subscription"]["legacySibling"] = json!({"preserve":true});
    external["providers"][CLAUDE][1]["apiKey"] = json!(KEY);
    edit_disk(&root, &external)?;
    let before = file(&root);

    input["id"] = json!("different");
    rejected_without_write(
        &root,
        store.adopt_subscription(&input),
        "Subscription identity does not match",
        &before,
    );
    input["id"] = new["id"].clone();
    input["subscription"]["credentialRef"]["name"] = json!(format!("../{CREDENTIAL}"));
    rejected_without_write(
        &root,
        store.adopt_subscription(&input),
        "Invalid credential reference",
        &before,
    );
    input["subscription"]["credentialRef"]["name"] = json!(CREDENTIAL);
    input["modelMapping"]["sonnet"] = json!("unlisted");
    rejected_without_write(
        &root,
        store.adopt_subscription(&input),
        "Selected model is not available",
        &before,
    );
    input["modelMapping"]["sonnet"] = json!("demo-sub");
    input["name"] = json!(" Renewed ");
    input["subscription"]["credentialRef"]["authIndex"] = json!("mock-index");
    let changed = store.adopt_subscription(&input)?;
    assert_public(&changed);
    assert_eq!(new["id"], changed["id"]);
    assert_eq!(changed["name"], "Renewed");
    assert_eq!(changed["hasApiKey"], false);
    let disk = state(&root);
    assert_eq!(disk["revision"], 5);
    assert_eq!(disk["providers"][CLAUDE][1]["legacyRow"], true);
    assert_eq!(
        disk["providers"][CLAUDE][1]["subscription"]["legacySibling"],
        json!({"preserve": true})
    );
    assert_eq!(
        disk["providers"][CLAUDE][1]["subscription"]["credentialRef"]["authIndex"],
        "mock-index"
    );
    assert!(disk["providers"][CLAUDE][1].get("apiKey").is_none());
    Ok(())
}

#[test]
fn malformed_json_null_and_io_errors_do_not_cover_saved_data() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let store = store(&root);
    fs::create_dir_all(&root)?;
    let path = root.join(FILE);
    for (contents, expected) in [
        (
            format!("{{\"{KEY}\""),
            "Invalid provider configuration JSON",
        ),
        ("null".into(), "Provider configuration is null"),
        ("\u{feff}{}".into(), "Invalid provider configuration JSON"),
        (
            r#"{"version":1,"activeProviderIds":{},"providers":{"claude-code":[null]}}"#.into(),
            "Invalid provider configuration row",
        ),
    ] {
        fs::write(&path, &contents)?;
        assert_eq!(store.snapshot().unwrap_err().to_string(), expected);
        rejected_without_write(&root, store.upsert(&api()), expected, contents.as_bytes());
    }
    fs::remove_file(&path)?;
    fs::create_dir(&path)?;
    assert!(store.upsert(&api()).is_err());
    assert!(path.is_dir() && !root.join(TEMP).exists());
    fs::remove_dir(&path)?;

    store.upsert(&api())?;
    let before = file(&root);
    let temporary = root.join(TEMP);
    fs::create_dir(&temporary)?;
    let sentinel = temporary.join("keep");
    fs::write(&sentinel, b"untouched")?;
    let error = store.upsert(&api()).unwrap_err();
    assert_redacted(&format!("{error:?}"));
    assert_eq!(error.to_string(), "Could not write provider configuration");
    assert!(file(&root) == before);
    assert_eq!(fs::read(&sentinel)?, b"untouched");
    fs::remove_dir_all(&temporary)?;
    fs::write(&temporary, b"abandoned scratch bytes")?;
    store.upsert(&api())?;
    assert!(!temporary.exists());
    assert_revision(&store, 3)?;
    Ok(())
}

#[test]
fn same_instance_serializes_writes_and_recovers_from_rejected_calls() -> Result<()> {
    let temp = temp();
    let root = root(&temp);
    let store = store(&root);
    std::thread::scope(|scope| {
        for index in 0..8 {
            let store = &store;
            scope.spawn(move || {
                let mut input = api();
                input["name"] = json!(format!("row-{index}"));
                store.upsert(&input).unwrap();
            });
        }
        let store = &store;
        scope.spawn(move || assert!(store.upsert(&json!({"kind":"subscription"})).is_err()));
    });
    let view = store.snapshot()?;
    assert_eq!(view["revision"], 9);
    let rows = view["providers"][CLAUDE].as_array().unwrap();
    assert_eq!(rows.len(), 9);
    assert!(!root.join(TEMP).exists());
    Ok(())
}
