//! The V1 provider configuration file shared with the existing runtime.
mod format;
mod models;

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{claude_role_budget::CLAUDE_ROLES as ROLES, provider_launch::EFFORTS};
use models::{normalize_metadata, subscription_model};

const CONFIG_FILE: &str = "provider-config.json";
const TEMP_FILE: &str = "provider-config.json.tmp";
const CLIS: [&str; 2] = ["claude-code", "codex"];
const RISK_POLICY: &str = "antigravityRiskAcceptedAt";
const DEFAULT_MODEL_INPUT: &str = "__cli_default__";
const DEFAULT_MODEL_VIEW: &str = "CLI 默认";
const CONTEXT_WINDOW: &str = "contextWindow";
const CONTEXT_WINDOW_MODE: &str = "contextWindowMode";
const MODEL_PREFERENCES: &str = "agent.modelPreferences";
const NOT_FOUND: [&str; 2] = [
    "供应商配置不存在",
    "That provider configuration does not exist",
];
const ENDPOINT_INVALID: [&str; 2] = [
    "API 地址需要使用有效的 HTTP 或 HTTPS 地址",
    "The API endpoint must be a valid HTTP or HTTPS address",
];

#[derive(Clone, Copy)]
pub enum ProviderConfigLocale {
    ZhCn,
    En,
}

impl ProviderConfigLocale {
    fn text(self, [zh, en]: [&'static str; 2]) -> &'static str {
        match self {
            Self::ZhCn => zh,
            Self::En => en,
        }
    }

    fn seeds(self) -> Value {
        json!({
            "version": 1,
            "revision": 1,
            "providers": {
                "claude-code": [{
                    "id": "anthropic-official",
                    "cli": "claude-code",
                    "name": self.text(["Anthropic 官方", "Anthropic (official)"]),
                    "endpoint": "https://api.anthropic.com",
                    "model": "",
                    "builtIn": true
                }],
                "codex": [{
                    "id": "openai-official",
                    "cli": "codex",
                    "name": self.text(["OpenAI 官方", "OpenAI (official)"]),
                    "endpoint": "https://api.openai.com/v1",
                    "model": "",
                    "builtIn": true
                }]
            },
            "activeProviderIds": {
                "claude-code": "anthropic-official",
                "codex": "openai-official"
            }
        })
    }
}

/// Private account authority used by the runtime, never a settings view.
#[derive(Clone, PartialEq, Eq)]
pub struct SubscriptionAccountIdentity {
    pub credential_name: String,
    pub auth_index: Option<String>,
    pub email: Option<String>,
}

pub struct StoredSubscription {
    pub name: String,
    pub provider: String,
    pub identity: SubscriptionAccountIdentity,
    pub account: Value,
    pub model_metadata: Vec<Value>,
    pub model_mapping: Value,
    pub context_window: Option<u64>,
}

fn subscription_identity(row: &Value) -> Option<SubscriptionAccountIdentity> {
    if row["kind"] != "subscription" {
        return None;
    }
    let subscription = &row["subscription"];
    let copy = |value: &Value| {
        value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Some(SubscriptionAccountIdentity {
        credential_name: copy(&subscription["credentialRef"]["name"])?,
        auth_index: copy(&subscription["credentialRef"]["authIndex"]),
        email: copy(&subscription["account"]["email"]),
    })
}

/// Keeps transactions sequential for one store. Each mutation reloads the file
/// under the lock, so an edit made between calls remains visible.
pub struct ProviderConfigStore {
    root: PathBuf,
    locale: ProviderConfigLocale,
    new_id: Box<dyn Fn() -> Uuid + Send + Sync>,
    now: Box<dyn Fn() -> SystemTime + Send + Sync>,
    writes: Mutex<()>,
    subscription_status: Mutex<BTreeMap<String, SubscriptionAccountIdentity>>,
}

impl ProviderConfigStore {
    pub fn new(root: impl Into<PathBuf>, locale: ProviderConfigLocale) -> Self {
        Self::with_sources(root, locale, Uuid::new_v4, SystemTime::now)
    }

    /// Supply deterministic ID and clock sources for byte-compatible replay.
    pub fn with_sources(
        root: impl Into<PathBuf>,
        locale: ProviderConfigLocale,
        new_id: impl Fn() -> Uuid + Send + Sync + 'static,
        now: impl Fn() -> SystemTime + Send + Sync + 'static,
    ) -> Self {
        Self {
            root: root.into(),
            locale,
            new_id: Box::new(new_id),
            now: Box::new(now),
            writes: Mutex::new(()),
            subscription_status: Mutex::default(),
        }
    }

    /// Read a redacted ProviderConfigSnapshot. Repairs stay in memory until a write.
    pub fn snapshot(&self) -> Result<Value> {
        let mut state = self.read()?;
        let statuses = self.subscription_status.lock().unwrap();
        for row in list_mut(&mut state, CLIS[0]) {
            if let Some(identity) = subscription_identity(row)
                && row["id"].as_str().and_then(|id| statuses.get(id)) == Some(&identity)
            {
                row["subscription"]["account"]["status"] = json!("needs-sign-in");
            }
        }
        snapshot(&state)
    }

    pub fn has_subscription(&self, provider: &str) -> Result<bool> {
        Ok(self.read()?["providers"][CLIS[0]]
            .as_array()
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row["kind"] == "subscription" && row["subscription"]["provider"] == provider
                })
            }))
    }

    pub fn subscription(&self, id: &str) -> Result<Option<StoredSubscription>> {
        let state = self.read()?;
        let Some(row) = state["providers"][CLIS[0]].as_array().and_then(|rows| {
            rows.iter()
                .find(|row| row["id"] == id && row["kind"] == "subscription")
        }) else {
            return Ok(None);
        };
        let identity =
            subscription_identity(row).context("Invalid subscription credential reference")?;
        let subscription = &row["subscription"];
        let mut account = subscription["account"].clone();
        if self.subscription_status.lock().unwrap().get(id) == Some(&identity) {
            account["status"] = json!("needs-sign-in");
        }
        Ok(Some(StoredSubscription {
            name: row["name"].as_str().unwrap_or(id).to_owned(),
            provider: subscription["provider"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            identity,
            account,
            model_metadata: subscription["modelMetadata"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
            model_mapping: row["modelMapping"].clone(),
            context_window: custom_context_window(&row["contextWindow"]),
        }))
    }

    pub fn mark_subscription_needs_sign_in(
        &self,
        id: &str,
        identity: &SubscriptionAccountIdentity,
    ) -> Result<()> {
        let _guard = self.writes.lock().unwrap();
        if self
            .subscription(id)?
            .is_some_and(|row| row.identity == *identity)
        {
            self.subscription_status
                .lock()
                .unwrap()
                .insert(id.to_owned(), identity.clone());
        }
        Ok(())
    }

    /// Resolve a key for a discovery request without exposing it in a settings reply.
    pub fn model_discovery_key(
        &self,
        cli: &str,
        provider_id: Option<&str>,
        endpoint: &str,
        draft_key: &str,
    ) -> Result<Option<String>> {
        let cli = self.cli(cli)?;
        let Some(id) = provider_id else {
            return Ok((!draft_key.is_empty()).then(|| draft_key.to_owned()));
        };
        let state = self.read()?;
        let saved = state["providers"][cli]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == id));
        if saved.is_some_and(|row| row["kind"] == "subscription") {
            return Ok(None);
        }
        if !draft_key.is_empty() {
            return Ok(Some(draft_key.to_owned()));
        }
        Ok(saved
            .filter(|row| {
                row["endpoint"].as_str().is_some_and(|saved_endpoint| {
                    strip_one_slash(saved_endpoint) == strip_one_slash(endpoint)
                })
            })
            .and_then(|row| row["apiKey"].as_str())
            .map(str::to_owned))
    }

    pub fn normalize_model_metadata(value: &Value) -> Option<Value> {
        normalize_metadata(value)
    }

    /// Save an API-key input, subscription edit, or numeric context-window patch.
    /// The JSON shape allows legacy optional fields to keep their V1 semantics.
    pub fn upsert(&self, input: &Value) -> Result<Value> {
        if let Some(value) = input.get(CONTEXT_WINDOW) {
            ensure!(
                input.as_object().is_some_and(|input| input.len() == 3)
                    && input["cli"] == "claude-code",
                "Invalid provider context window update"
            );
            return self.set_context_window(input["id"].as_str().unwrap_or_default(), value);
        }
        ensure!(
            input.get(CONTEXT_WINDOW_MODE).is_none(),
            "Context window mode requires model preferences storage"
        );
        self.mutate(|state| self.apply_upsert(state, input))
    }

    /// Save the draft and its selected window together. The preferences transaction
    /// rolls back if the configuration file cannot be published. If the transaction
    /// cannot commit, restore the exact previous file while still holding the lock.
    pub fn upsert_with_context_window(
        &self,
        db: &mut Connection,
        input: &Value,
        updated_at: i64,
    ) -> Result<Value> {
        let mode = input[CONTEXT_WINDOW_MODE]
            .as_str()
            .filter(|mode| valid_context_mode(mode))
            .context("Invalid provider context window mode")?;
        ensure!(
            input["cli"] == CLIS[0] && input.get(CONTEXT_WINDOW).is_none(),
            "Invalid provider context window mode"
        );
        let _guard = self
            .writes
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider configuration write lock failed"))?;
        let previous = self.read_bytes()?;
        let mut state = self.decode(previous.as_deref())?;
        let mut public = self.apply_upsert(&mut state, input)?;
        let id = public["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.encode_utf16().count() <= 256)
            .context("Invalid provider ID")?
            .to_owned();
        // An explicit preset replaces an older numeric override for this row.
        let row = list_mut(&mut state, CLIS[0])
            .iter_mut()
            .find(|row| row["id"] == id)
            .context("Missing updated provider")?;
        row.as_object_mut().unwrap().shift_remove(CONTEXT_WINDOW);
        public.as_object_mut().unwrap().shift_remove(CONTEXT_WINDOW);

        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut preferences = read_model_preferences(&tx)?;
        let windows = preferences
            .entry("contextWindows")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("Invalid context window preferences")?;
        windows.insert(id, json!(mode));
        preferences
            .entry("compaction")
            .or_insert_with(|| json!({"mode": "follow", "effort": "follow"}));
        tx.execute(
            "INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json,
                                           updated_at = excluded.updated_at",
            params![
                MODEL_PREFERENCES,
                serde_json::to_string(&preferences)?,
                updated_at
            ],
        )?;
        self.write(&state)?;
        if let Err(error) = tx.commit() {
            match previous {
                Some(bytes) => write_atomic(&self.root, &bytes),
                None => fs::remove_file(self.root.join(CONFIG_FILE)),
            }
            .context("Could not restore provider configuration after preferences save failed")?;
            return Err(error).context("Could not save provider context window preferences");
        }
        public[CONTEXT_WINDOW_MODE] = json!(mode);
        Ok(public)
    }

    /// Include persisted modes in the settings view without changing the V1 file.
    pub fn project_context_windows(db: &Connection, snapshot: &mut Value) {
        let preferences = crate::sessions::read_claude_role_launch_preferences(db);
        if let Some(providers) = snapshot["providers"][CLIS[0]].as_array_mut() {
            for provider in providers {
                if let Some(mode) = provider["id"]
                    .as_str()
                    .and_then(|id| preferences.context_windows.get(id))
                {
                    provider[CONTEXT_WINDOW_MODE] = mode.clone();
                }
            }
        }
    }

    fn apply_upsert(&self, state: &mut Value, input: &Value) -> Result<Value> {
        if input["kind"] == "subscription" {
            return self.edit_subscription(state, input);
        }
        validate_efforts(input)?;
        let cli = self.cli(input["cli"].as_str().unwrap_or_default())?;
        let name = self.name(input)?;
        let model = trim(input["model"].as_str().unwrap_or_default());
        ensure!(
            !model.is_empty(),
            self.locale
                .text(["默认模型不能为空", "The default model must not be empty"])
        );
        let model = if model == DEFAULT_MODEL_INPUT {
            ""
        } else {
            model
        };
        let endpoint = self.endpoint(input["endpoint"].as_str().unwrap_or_default())?;
        let key = match input.get("apiKey") {
            None | Some(Value::Null) => "",
            Some(value) => trim(
                value
                    .as_str()
                    .context("API Key contains invalid characters")?,
            ),
        };
        ensure!(
            key.bytes().all(|byte| (0x21..=0x7e).contains(&byte)),
            "API Key contains invalid characters"
        );
        let mapping = input
            .get("modelMapping")
            .map(normalize_mapping)
            .transpose()?;
        let list = list_mut(state, cli);
        let existing = matching_input(list, input.get("id"), self.locale)?;
        let mut row = existing
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        ensure!(
            row.get("kind").is_none_or(|kind| kind != "subscription"),
            "Cannot change a subscription into an API key provider"
        );
        let same_endpoint = row.get("endpoint").and_then(Value::as_str) == Some(&endpoint);
        ensure!(
            same_endpoint || !key.is_empty() || !row.get("apiKey").is_some_and(truthy),
            "Endpoint changed: enter the API key for the new service"
        );
        let same_key = key.is_empty() || row.get("apiKey").and_then(Value::as_str) == Some(key);
        let metadata = normalize_metadata(&input["modelMetadata"]);
        let id = self.id_for(&row);
        row.insert("id".into(), id);
        row.insert("cli".into(), json!(cli));
        row.insert("name".into(), json!(name));
        row.insert("endpoint".into(), json!(endpoint));
        row.insert("model".into(), json!(model));
        copy_present(&mut row, input, &["reasoningEffort", "effortMapping"]);
        if let Some(mapping) = mapping {
            row.insert("modelMapping".into(), mapping);
        }
        if row.get("builtIn").is_some_and(truthy) {
            row.insert("builtIn".into(), json!(true));
        }
        if !key.is_empty() {
            row.insert("apiKey".into(), json!(key));
        }
        if let Some(metadata) = metadata {
            row.insert("modelMetadata".into(), metadata);
        } else if !same_endpoint || !same_key {
            row.shift_remove("modelMetadata");
        }
        let row = Value::Object(row);
        let public = view(&row)?;
        replace_row(list, row);
        bump_revision(state);
        Ok(public)
    }

    fn edit_subscription(&self, state: &mut Value, input: &Value) -> Result<Value> {
        ensure!(
            input["id"].as_str().is_some_and(|id| !id.is_empty()),
            "Subscription requires a completed sign-in"
        );
        ensure!(
            input["cli"] == CLIS[0]
                && input
                    .as_object()
                    .is_some_and(|fields| fields.keys().all(|key| {
                        matches!(
                            key.as_str(),
                            "cli"
                                | "id"
                                | "kind"
                                | "name"
                                | "modelMapping"
                                | "effortMapping"
                                | "contextWindowMode"
                        )
                    })),
            "Invalid subscription edit"
        );
        let name = self.name(input)?;
        ensure!(name.encode_utf16().count() <= 256, "Invalid provider name");
        let mapping = normalize_mapping(&input["modelMapping"])?;
        validate_efforts(input)?;
        let list = list_mut(state, CLIS[0]);
        let existing = matching_input(list, input.get("id"), self.locale)?
            .filter(|row| row["kind"] == "subscription")
            .context("Subscription requires a completed sign-in")?;
        let subscription = &existing["subscription"];
        let models = subscription["modelMetadata"]
            .as_array()
            .context("Invalid subscription model directory")?;
        for role in ROLES {
            let id = mapping[role].as_str().unwrap_or_default();
            if role == "fable" && id.is_empty() {
                continue;
            }
            let normalized = id.to_ascii_lowercase();
            let model = models
                .iter()
                .find(|model| model["id"] == id)
                .filter(|_| {
                    !id.is_empty()
                        && !["image", "video", "codex-auto-review"]
                            .iter()
                            .any(|part| normalized.contains(part))
                })
                .context("Selected model is not available")?;
            if let Some(effort) = input["effortMapping"][role].as_str()
                && effort != "follow"
            {
                let supported = model["supportedEfforts"].as_array().map_or_else(
                    || effort != "max" || subscription_max_effort(subscription, model),
                    |values| values.iter().any(|value| value == effort),
                );
                ensure!(
                    supported && (effort != "max" || subscription["provider"] != "codex"),
                    "Selected model does not support this reasoning effort"
                );
            }
        }
        let mut row = existing.clone();
        row["name"] = json!(name);
        row["model"] = mapping["sonnet"].clone();
        row["modelMapping"] = mapping;
        if let Some(effort) = input.get("effortMapping") {
            row["effortMapping"] = effort.clone();
        }
        let public = view(&row)?;
        replace_row(list, row);
        bump_revision(state);
        Ok(public)
    }

    fn set_context_window(&self, id: &str, value: &Value) -> Result<Value> {
        let tokens = if value.is_null() {
            None
        } else {
            Some(custom_context_window(value).context(self.locale.text([
                "上下文窗口必须是至少 100,000 的有效整数。",
                "The context window must be a valid integer of at least 100,000.",
            ]))?)
        };
        self.mutate(|state| {
            let row = list_mut(state, "claude-code")
                .iter_mut()
                .find(|row| !id.is_empty() && row["id"] == id && row["kind"] != "subscription")
                .context(self.locale.text(NOT_FOUND))?;
            let object = row
                .as_object_mut()
                .context("Invalid provider configuration row")?;
            if let Some(tokens) = tokens {
                object.insert(CONTEXT_WINDOW.into(), json!(tokens));
            } else {
                object.shift_remove(CONTEXT_WINDOW);
            }
            let public = view(row)?;
            bump_revision(state);
            Ok(public)
        })
    }

    pub fn delete(&self, cli: &str, id: &str) -> Result<Value> {
        self.mutate(|state| {
            let cli = self.cli(cli)?;
            let provider = state["providers"][cli]
                .as_array()
                .unwrap()
                .iter()
                .find(|provider| provider["id"] == id)
                .context(self.locale.text(NOT_FOUND))?;
            ensure!(
                state["activeProviderIds"][cli] != id,
                self.locale.text([
                    "使用中的供应商需要先切换后再删除",
                    "Switch away from the provider in use before deleting it",
                ])
            );
            ensure!(
                !truthy(&provider["builtIn"]),
                self.locale.text([
                    "官方供应商配置保留为默认入口",
                    "The official provider configuration stays as the default entry",
                ])
            );
            list_mut(state, cli).retain(|provider| provider["id"] != id);
            bump_revision(state);
            snapshot(state)
        })
    }

    pub fn set_active(&self, cli: &str, id: &str) -> Result<Value> {
        self.set_active_checked(cli, id, None)
    }

    pub fn set_active_checked(
        &self,
        cli: &str,
        id: &str,
        expected: Option<&SubscriptionAccountIdentity>,
    ) -> Result<Value> {
        self.mutate(|state| {
            let cli = self.cli(cli)?;
            let provider = list_mut(state, cli)
                .iter()
                .find(|provider| provider["id"] == id)
                .context(self.locale.text(NOT_FOUND))?;
            if let Some(expected) = expected {
                ensure!(
                    subscription_identity(provider).as_ref() == Some(expected),
                    "Subscription account changed during activation"
                );
            }
            state["activeProviderIds"][cli] = json!(id);
            // The native contract advances the revision even when the ID matches.
            bump_revision(state);
            snapshot(state)
        })
    }

    /// Persist the existing policy date again, or capture it for the first time.
    pub fn confirm_antigravity_risk(&self) -> Result<()> {
        self.mutate(|state| {
            let current = &state["policies"][RISK_POLICY];
            let date = if current.is_null() {
                json!(iso_time((self.now)())?)
            } else {
                current.clone()
            };
            let mut policies = spread(&state["policies"]);
            policies.insert(RISK_POLICY.into(), date);
            state["policies"] = Value::Object(policies);
            bump_revision(state);
            Ok(())
        })
    }

    /// Internal store commit for an account already adopted by the caller.
    /// The credential reference stays on disk and is omitted from the returned view.
    pub fn adopt_subscription(&self, input: &Value) -> Result<Value> {
        self.mutate(|state| {
            validate_efforts(input)?;
            let incoming = &input["subscription"];
            let provider = incoming["provider"].as_str().unwrap_or_default();
            ensure!(
                matches!(provider, "codex" | "antigravity"),
                "Invalid subscription provider"
            );
            ensure!(
                provider != "antigravity" || truthy(&state["policies"][RISK_POLICY]),
                "Antigravity risk confirmation is required"
            );
            let name = input["name"].as_str().unwrap_or_default();
            ensure!(
                !trim(name).is_empty() && name.encode_utf16().count() <= 256,
                "Invalid provider name"
            );
            let credential = &incoming["credentialRef"];
            let filename = credential["name"]
                .as_str()
                .and_then(|name| name.strip_suffix(".json"))
                .unwrap_or_default();
            ensure!(
                !filename.is_empty() && !filename.contains(['/', '\\', '\0']),
                "Invalid credential reference"
            );
            let models = incoming["modelMetadata"]
                .as_array()
                .context("Selected model is not available")?;
            let mapping =
                require_object(&input["modelMapping"], "Selected model is not available")?;
            for role in ROLES {
                if role == "fable" && mapping.get(role).is_none_or(|id| id == "") {
                    continue;
                }
                ensure!(
                    mapping.get(role).is_some_and(
                        |id| id.is_string() && models.iter().any(|model| model["id"] == *id)
                    ),
                    "Selected model is not available"
                );
            }
            let list = list_mut(state, CLIS[0]);
            let existing = list.iter().find(|row| {
                row["kind"] == "subscription" && row["subscription"]["provider"] == provider
            });
            if let Some(id) = input.get("id").filter(|id| truthy(id)) {
                ensure!(
                    existing.and_then(|row| row.get("id")) == Some(id),
                    "Subscription identity does not match"
                );
            }
            let mut row = existing
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut subscription = row.get("subscription").map(spread).unwrap_or_default();
            let mut credential_ref = Map::new();
            copy_present(&mut credential_ref, credential, &["name"]);
            copy_truthy(&mut credential_ref, credential, &["authIndex"]);
            let mut account = Map::new();
            copy_present(&mut account, &incoming["account"], &["status"]);
            copy_truthy(&mut account, &incoming["account"], &["email", "plan"]);
            subscription.insert("provider".into(), json!(provider));
            subscription.insert("credentialRef".into(), Value::Object(credential_ref));
            subscription.insert("account".into(), Value::Object(account));
            subscription.insert(
                "modelMetadata".into(),
                Value::Array(models.iter().map(subscription_model).collect()),
            );
            let id = self.id_for(&row);
            row.insert("id".into(), id);
            row.insert("cli".into(), json!(CLIS[0]));
            row.insert("kind".into(), json!("subscription"));
            row.insert("name".into(), json!(trim(name)));
            row.insert("endpoint".into(), json!(""));
            row.insert("model".into(), mapping["sonnet"].clone());
            // The internal input mapping is spread in its original order.
            row.insert("modelMapping".into(), Value::Object(mapping.clone()));
            copy_present(&mut row, input, &["effortMapping"]);
            row.insert("subscription".into(), Value::Object(subscription));
            row.shift_remove("apiKey");
            let row = Value::Object(row);
            let public = view(&row)?;
            replace_row(list, row);
            bump_revision(state);
            Ok(public)
        })
    }

    fn id_for(&self, row: &Map<String, Value>) -> Value {
        row.get("id")
            .filter(|id| !id.is_null())
            .cloned()
            .unwrap_or_else(|| json!((self.new_id)().to_string()))
    }

    fn name<'a>(&self, input: &'a Value) -> Result<&'a str> {
        let name = trim(input["name"].as_str().unwrap_or_default());
        ensure!(
            !name.is_empty(),
            self.locale
                .text(["供应商名称不能为空", "The provider name must not be empty"])
        );
        Ok(name)
    }

    fn endpoint(&self, value: &str) -> Result<String> {
        let invalid = self.locale.text(ENDPOINT_INVALID);
        let url = url::Url::parse(trim(value)).map_err(|_| anyhow::anyhow!("{invalid}"))?;
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none_or(str::is_empty),
            "{invalid}"
        );
        let serialized = url.as_str();
        Ok(serialized
            .strip_suffix('/')
            .unwrap_or(serialized)
            .to_owned())
    }

    fn cli(&self, input: &str) -> Result<&'static str> {
        CLIS.into_iter().find(|cli| *cli == input).context(
            self.locale
                .text(["CLI 类型不正确", "The CLI type is invalid"]),
        )
    }

    fn mutate<T>(&self, operation: impl FnOnce(&mut Value) -> Result<T>) -> Result<T> {
        let _guard = self
            .writes
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider configuration write lock failed"))?;
        let mut state = self.read()?;
        let result = operation(&mut state)?;
        self.write(&state)?;
        Ok(result)
    }

    fn read(&self) -> Result<Value> {
        self.decode(self.read_bytes()?.as_deref())
    }

    fn read_bytes(&self) -> Result<Option<Vec<u8>>> {
        match fs::read(self.root.join(CONFIG_FILE)) {
            Ok(data) => Ok(Some(data)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("Could not read provider configuration"),
        }
    }

    fn decode(&self, data: Option<&[u8]>) -> Result<Value> {
        let Some(data) = data else {
            return Ok(self.locale.seeds());
        };
        // Hiding the parser error keeps malformed keys and credential text out of diagnostics.
        let mut state = format::parse(data)?;
        ensure!(!state.is_null(), "Provider configuration is null");
        if state["version"].as_f64() != Some(1.0)
            || !truthy(&state["providers"])
            || !truthy(&state["activeProviderIds"])
        {
            return Ok(self.locale.seeds());
        }
        require_object(&state["providers"], "Invalid provider lists")?;
        require_object(&state["activeProviderIds"], "Invalid active provider IDs")?;

        for cli in CLIS {
            let mut active = state["activeProviderIds"].get(cli).cloned();
            if state["providers"][cli].as_array().is_none_or(Vec::is_empty) {
                state["providers"][cli] = self.locale.seeds()["providers"][cli].take();
            }
            let list = list_mut(&mut state, cli);
            for provider in list.iter_mut() {
                let row = provider
                    .as_object_mut()
                    .context("Invalid provider configuration row")?;
                if let Some(raw) = row.get("modelMetadata") {
                    if let Some(metadata) = normalize_metadata(raw) {
                        row.insert("modelMetadata".into(), metadata);
                    } else {
                        row.shift_remove("modelMetadata");
                    }
                }
            }
            if !list.iter().any(|row| row.get("id") == active.as_ref()) {
                active = list[0].get("id").cloned();
                let ids = state["activeProviderIds"].as_object_mut().unwrap();
                if let Some(id) = active {
                    ids.insert(cli.into(), id);
                } else {
                    ids.shift_remove(cli);
                }
            }
        }
        Ok(state)
    }

    fn write(&self, state: &Value) -> Result<()> {
        let bytes = format::stringify(state);
        write_atomic(&self.root, bytes.as_bytes()).context("Could not write provider configuration")
    }
}

fn valid_context_mode(mode: &str) -> bool {
    matches!(mode, "default" | "maximum" | "512k")
}

fn read_model_preferences(db: &Connection) -> Result<Map<String, Value>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT value_json FROM preferences WHERE key = ?",
            [MODEL_PREFERENCES],
            |row| row.get(0),
        )
        .optional()?;
    match raw {
        Some(raw) => serde_json::from_str(&raw).context("Invalid model preferences"),
        None => Ok(Map::new()),
    }
}

fn list_mut<'a>(state: &'a mut Value, cli: &str) -> &'a mut Vec<Value> {
    state["providers"][cli].as_array_mut().unwrap()
}

fn matching_input<'a>(
    list: &'a [Value],
    id: Option<&Value>,
    locale: ProviderConfigLocale,
) -> Result<Option<&'a Value>> {
    let Some(id) = id.filter(|id| truthy(id)) else {
        return Ok(None);
    };
    Ok(Some(
        list.iter()
            .find(|row| row.get("id") == Some(id))
            .context(locale.text(NOT_FOUND))?,
    ))
}

fn replace_row(list: &mut Vec<Value>, row: Value) {
    if let Some(existing) = list.iter_mut().find(|existing| existing["id"] == row["id"]) {
        *existing = row;
    } else {
        list.push(row);
    }
}

fn validate_efforts(input: &Value) -> Result<()> {
    let valid = |value: &Value| value.as_str().is_some_and(|level| EFFORTS.contains(&level));
    if let Some(effort) = input.get("reasoningEffort") {
        ensure!(valid(effort), "Invalid reasoning effort");
    }
    if let Some(mapping) = input.get("effortMapping") {
        ensure!(
            require_object(mapping, "Invalid role reasoning effort")?
                .iter()
                .all(|(role, value)| ROLES.contains(&role.as_str()) && valid(value)),
            "Invalid role reasoning effort"
        );
    }
    Ok(())
}

fn normalize_mapping(value: &Value) -> Result<Value> {
    let mut mapping = Map::new();
    for role in ROLES {
        let model = value
            .get(role)
            .map_or(Some(""), Value::as_str)
            .context("Invalid model mapping")?;
        let model = trim(model);
        if role != "fable" || !model.is_empty() {
            mapping.insert(role.into(), json!(model));
        }
    }
    Ok(Value::Object(mapping))
}

fn require_object<'a>(value: &'a Value, message: &'static str) -> Result<&'a Map<String, Value>> {
    value.as_object().context(message)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}

fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

fn spread(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object.clone(),
        Value::Array(values) => values
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value))
            .collect(),
        Value::String(text) => text
            .chars()
            .enumerate()
            .map(|(index, value)| (index.to_string(), json!(value.to_string())))
            .collect(),
        _ => Map::new(),
    }
}

fn bump_revision(state: &mut Value) {
    // The legacy reader leaves revision alone. Match JS += 1 when rewriting it.
    let revision = match state.get("revision") {
        None => Value::Null,
        Some(Value::Null) => json!(1),
        Some(Value::Bool(value)) => json!(u8::from(*value) + 1),
        Some(Value::Number(value)) => {
            let next = value.as_f64().unwrap() + 1.0;
            // Keep ordinary integer revisions usable as integers in the public Value.
            if next.fract() == 0.0
                && (-9_007_199_254_740_992.0..=9_007_199_254_740_992.0).contains(&next)
            {
                json!(next as i64)
            } else {
                json!(next)
            }
        }
        Some(value) => json!(format!("{}1", js_string(value))),
    };
    state["revision"] = revision;
}

fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(values) => values
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
        Value::Number(number) => format::number(number),
        _ => value.to_string(),
    }
}

fn copy_present(output: &mut Map<String, Value>, input: &Value, keys: &[&str]) {
    for &key in keys {
        if let Some(value) = input.get(key) {
            output.insert(key.into(), value.clone());
        }
    }
}

fn copy_truthy(output: &mut Map<String, Value>, input: &Value, keys: &[&str]) {
    for &key in keys {
        if truthy(&input[key]) {
            output.insert(key.into(), input[key].clone());
        }
    }
}

fn copy_strings(output: &mut Map<String, Value>, input: &Value, keys: &[&str]) {
    for &key in keys {
        if let Some(text) = input[key].as_str() {
            output.insert(key.into(), json!(text));
        }
    }
}

fn strip_one_slash(value: &str) -> &str {
    value.strip_suffix('/').unwrap_or(value)
}

fn subscription_max_effort(subscription: &Value, model: &Value) -> bool {
    let label = format!(
        "{} {}",
        model["id"].as_str().unwrap_or_default(),
        model["displayName"].as_str().unwrap_or_default(),
    )
    .to_ascii_lowercase();
    subscription["provider"] != "codex"
        && !label
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .any(|word| {
                matches!(word, "gpt" | "codex") || matches!(word.as_bytes(), [b'o', b'1'..=b'9'])
            })
        && ROLES
            .into_iter()
            .chain(["claude"])
            .any(|part| label.contains(part))
}

fn snapshot(state: &Value) -> Result<Value> {
    let mut result = Map::new();
    if let Some(revision) = state.get("revision") {
        result.insert("revision".into(), redact(revision));
    }
    result.insert(
        "activeProviderIds".into(),
        redact(&state["activeProviderIds"]),
    );
    let mut providers = Map::new();
    for cli in CLIS {
        let views = state["providers"][cli]
            .as_array()
            .unwrap()
            .iter()
            .map(view)
            .collect::<Result<Vec<_>>>()?;
        providers.insert(cli.into(), Value::Array(views));
    }
    result.insert("providers".into(), Value::Object(providers));
    Ok(Value::Object(result))
}

fn redact(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "apiKey" | "credentialRef"))
                .map(|(key, value)| (key.clone(), redact(value)))
                .collect(),
        ),
        Value::Array(array) => Value::Array(array.iter().map(redact).collect()),
        _ => value.clone(),
    }
}

fn view(row: &Value) -> Result<Value> {
    let mut public = Map::new();
    copy_strings(&mut public, row, &["id", "cli", "name", "endpoint"]);
    public.insert(
        "model".into(),
        json!(
            row["model"]
                .as_str()
                .filter(|model| !model.is_empty())
                .unwrap_or(DEFAULT_MODEL_VIEW)
        ),
    );
    copy_present(&mut public, row, &["modelMetadata"]);
    if let Some(tokens) = custom_context_window(&row[CONTEXT_WINDOW]) {
        public.insert(CONTEXT_WINDOW.into(), json!(tokens));
    }
    for key in ["modelMapping", "effortMapping"] {
        if truthy(&row[key]) {
            let mut mapping = Map::new();
            copy_strings(&mut mapping, &row[key], &ROLES);
            public.insert(key.into(), Value::Object(mapping));
        }
    }
    if truthy(&row["reasoningEffort"]) {
        copy_strings(&mut public, row, &["reasoningEffort"]);
    }
    public.insert("hasApiKey".into(), json!(truthy(&row["apiKey"])));
    if truthy(&row["builtIn"]) {
        public.insert("builtIn".into(), json!(true));
    }
    let subscription = &row["subscription"];
    if row["kind"] == "subscription" && truthy(subscription) {
        let models = subscription["modelMetadata"]
            .as_array()
            .context("Invalid subscription model directory")?;
        let mut public_subscription = Map::new();
        copy_strings(&mut public_subscription, subscription, &["provider"]);
        let mut account = Map::new();
        copy_strings(
            &mut account,
            &subscription["account"],
            &["status", "email", "plan", "reason"],
        );
        public_subscription.insert("account".into(), Value::Object(account));
        public_subscription.insert(
            "modelMetadata".into(),
            Value::Array(
                models
                    .iter()
                    .map(models::public_subscription_model)
                    .collect(),
            ),
        );
        public.insert("kind".into(), json!("subscription"));
        public.insert("subscription".into(), Value::Object(public_subscription));
    }
    Ok(Value::Object(public))
}

pub(crate) fn custom_context_window(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .filter(|tokens| (100_000..=9_007_199_254_740_991).contains(tokens))
}

fn write_atomic(root: &Path, bytes: &[u8]) -> io::Result<()> {
    use fs::{DirBuilder, OpenOptions};
    use io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    let mut directory = DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    directory.mode(0o700);
    directory.create(root)?;

    let temporary = root.join(TEMP_FILE);
    // Replace crash leftovers without following a symlink at the fixed tmp path.
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    let write = (|| {
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)
    })();
    drop(file);
    // Match the frozen writer: atomic publication by rename, without fsync.
    let result = write.and_then(|()| fs::rename(&temporary, root.join(CONFIG_FILE)));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn iso_time(time: SystemTime) -> Result<String> {
    let milliseconds = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis())?,
        Err(error) => -i64::try_from(error.duration().as_millis())?,
    };
    let days = milliseconds.div_euclid(86_400_000);
    let clock = milliseconds.rem_euclid(86_400_000);
    // Convert Unix days to a Gregorian date in 400-year eras starting in March.
    let day = days + 719_468;
    let era = day.div_euclid(146_097);
    let day_of_era = day.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = (month_from_march + 2) % 12 + 1;
    let year = era * 400 + year_of_era + i64::from(month <= 2);
    ensure!(
        (0..=9999).contains(&year),
        "Policy confirmation time is out of range"
    );
    let (hours, minutes, seconds, millis) = (
        clock / 3_600_000,
        clock / 60_000 % 60,
        clock / 1000 % 60,
        clock % 1000,
    );
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_rename_removes_the_scratch_file() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join(CONFIG_FILE);
        fs::create_dir(&target)?;
        let sentinel = target.join("existing");
        fs::write(&sentinel, b"keep")?;

        assert!(write_atomic(root.path(), b"synthetic contents\n").is_err());
        assert_eq!(fs::read(sentinel)?, b"keep");
        assert!(!root.path().join(TEMP_FILE).exists());
        Ok(())
    }
}
