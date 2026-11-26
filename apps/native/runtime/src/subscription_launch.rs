//! Deterministic subscription launch, catalog and recovery decisions for offline fixtures.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::OsStr,
    io,
    sync::LazyLock,
};

use matou_native_core::{
    claude_role_budget::{CLAUDE_ROLE_LABELS, CLAUDE_ROLES, disables_1m_context},
    journal::MAX_SAFE_INTEGER,
    provider_launch::{EFFORTS, is_claude_model_environment, role_model_env_key},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::dsh_events::js_whitespace;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubscriptionProvider {
    #[serde(rename = "codex")]
    ChatGpt,
    Antigravity,
}

impl SubscriptionProvider {
    pub(crate) fn prefix(self) -> &'static str {
        match self {
            Self::ChatGpt => "matou-chatgpt",
            Self::Antigravity => "matou-antigravity",
        }
    }

    pub(crate) fn router_provider(self) -> &'static str {
        match self {
            Self::ChatGpt => "codex",
            Self::Antigravity => "antigravity",
        }
    }
}

/// The caller supplies OS facts. The frozen check applies only on macOS.
pub fn subscription_supported(is_macos: bool, macos_major: Option<u32>) -> bool {
    !is_macos || macos_major.is_some_and(|version| version >= 15)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelRole {
    Opus,
    Fable,
    Sonnet,
    Haiku,
}

const ROLES: [ModelRole; CLAUDE_ROLES.len()] = [
    ModelRole::Opus,
    ModelRole::Fable,
    ModelRole::Sonnet,
    ModelRole::Haiku,
];
const FABLE_FALLBACK_ALIAS: &str = "matou-opus-fallback";

impl ModelRole {
    fn name(self) -> &'static str {
        CLAUDE_ROLES[self as usize]
    }

    fn label(self) -> &'static str {
        CLAUDE_ROLE_LABELS[self as usize]
    }

    fn env_key(self) -> String {
        role_model_env_key(self.name())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub supported_efforts: Vec<String>,
}

#[derive(Default)]
pub struct RoleModels<'a> {
    pub opus: Option<&'a CatalogModel>,
    pub fable: Option<&'a CatalogModel>,
    pub sonnet: Option<&'a CatalogModel>,
    pub haiku: Option<&'a CatalogModel>,
    pub default: Option<&'a CatalogModel>,
    pub small_fast: Option<&'a CatalogModel>,
}

impl<'a> RoleModels<'a> {
    fn resolve(&self, role: ModelRole) -> Option<&'a CatalogModel> {
        let (preferred, fallback, last) = match role {
            ModelRole::Opus => (self.opus, self.default, self.small_fast),
            ModelRole::Fable => (
                self.fable
                    .filter(|model| !model.id.trim_matches(js_whitespace).is_empty()),
                self.resolve(ModelRole::Opus),
                None,
            ),
            ModelRole::Sonnet => (self.sonnet, self.default, self.small_fast),
            ModelRole::Haiku => (self.haiku, self.small_fast, self.default),
        };
        [preferred, fallback, last]
            .into_iter()
            .flatten()
            .find(|model| valid_launch_text(&model.id))
    }
}

pub struct LaunchInput<'a> {
    pub provider: SubscriptionProvider,
    pub provider_name: &'a str,
    pub router_url: &'a str,
    pub client_key: &'a str,
    pub models: RoleModels<'a>,
    pub main_role: ModelRole,
    pub context_window: Option<u64>,
    pub inherited_env_names: &'a [&'a str],
}

/// Process-only environment. It contains the local router client key.
pub struct LaunchPlan {
    provider: SubscriptionProvider,
    pub env: BTreeMap<String, String>,
    pub clear_env: BTreeSet<String>,
    pub settings: Value,
    pub aliases: Vec<ModelAlias>,
    pub main_model: String,
    pub unknown: Vec<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAlias {
    pub name: String,
    pub alias: String,
    pub fork: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LaunchError {
    InvalidInput,
    MissingRoleModels(Vec<ModelRole>),
    UnsupportedContextWindow,
}

pub fn build_launch(input: LaunchInput<'_>) -> Result<LaunchPlan, LaunchError> {
    if ![input.client_key, input.provider_name, input.router_url]
        .into_iter()
        .all(valid_launch_text)
        || loopback_url(input.router_url).is_none()
    {
        return Err(LaunchError::InvalidInput);
    }
    let resolved = ROLES.map(|role| input.models.resolve(role));
    if resolved.iter().any(Option::is_none) {
        let missing = ROLES
            .into_iter()
            .zip(resolved)
            .filter_map(|(role, model)| {
                (model.is_none() && (role != ModelRole::Fable || input.models.fable.is_some()))
                    .then_some(role)
            })
            .collect();
        return Err(LaunchError::MissingRoleModels(missing));
    }
    let models = resolved.map(Option::unwrap);
    let windows = models.map(|model| {
        input
            .context_window
            .or(model.context_window)
            .filter(valid_tokens)
    });
    if windows.into_iter().flatten().any(|window| window < 100_000) {
        return Err(LaunchError::UnsupportedContextWindow);
    }

    let prefix = input.provider.prefix();
    let mut env: BTreeMap<String, String> = [
        ("ANTHROPIC_BASE_URL", input.router_url),
        ("ANTHROPIC_AUTH_TOKEN", input.client_key),
        ("CLAUDE_CODE_AUTO_MODE_SERVER", "0"),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
        ("DISABLE_TELEMETRY", "1"),
        ("DISABLE_ERROR_REPORTING", "1"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    let mut aliases: Vec<ModelAlias> = Vec::new();
    let mut picker = Vec::new();
    for ((role, model), window) in ROLES.into_iter().zip(models).zip(windows) {
        let upstream = strip_context_marker(&model.id);
        let client_id = client_model_id(input.provider, role, model);
        let name = model
            .display_name
            .as_deref()
            .filter(|name| valid_launch_text(name))
            .map_or(upstream, |name| name.trim_matches(js_whitespace));
        let mut description = format!("{} · {upstream}", input.provider_name);
        if let Some(window) = window {
            let window = if window >= 1_000_000 {
                format!("{}M", window as f64 / 1_000_000.0)
            } else {
                format!("{}K", window as f64 / 1000.0)
            };
            description.push_str(&format!(" · {window}"));
        }
        let key = role.env_key();
        env.insert(format!("{key}_NAME"), name.to_owned());
        env.insert(format!("{key}_DESCRIPTION"), description.clone());
        env.insert(key, format!("{prefix}/{client_id}"));
        picker.push(json!({"model": role.name(), "label": format!("{} · {name}", role.label()), "description": description}));
        let alias = strip_context_marker(&client_id);
        if !aliases.iter().any(|current| current.alias == alias) {
            aliases.push(ModelAlias {
                name: upstream.into(),
                alias: alias.into(),
                fork: true,
            });
        }
    }
    // Old sessions may still reference the previously generated fallback alias.
    aliases.push(ModelAlias {
        name: strip_context_marker(&models[ModelRole::Opus as usize].id).into(),
        alias: FABLE_FALLBACK_ALIAS.into(),
        fork: true,
    });
    let fable = &env[&ModelRole::Fable.env_key()];
    let mut settings = json!({
        "env": { "ANTHROPIC_API_KEY": "", "CLAUDE_CODE_OAUTH_TOKEN": "" },
        "modelPicker": { "replaceBuiltInOptions": true, "options": picker },
        "modelOverrides": {
            "claude-fable-5": fable,
            "claude-fable-5-2": fable,
            "claude-fable-5-5": fable,
        },
    });
    let mut unknown = Vec::new();
    if let [Some(opus), Some(fable), Some(sonnet), Some(haiku)] = windows {
        let window = opus.min(fable).min(sonnet).min(haiku).min(1_000_000);
        settings["autoCompactWindow"] = json!(window);
        env.insert("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(), window.to_string());
        if disables_1m_context(window) {
            env.insert("CLAUDE_CODE_DISABLE_1M_CONTEXT".into(), "1".into());
            settings["env"]["CLAUDE_CODE_DISABLE_1M_CONTEXT"] = json!("1");
        }
    } else {
        unknown.push("context-window");
    }
    if let Some(output) = models[input.main_role as usize]
        .max_output_tokens
        .filter(valid_tokens)
    {
        env.insert(
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
            output.min(128_000).to_string(),
        );
    } else {
        unknown.push("max-output-tokens");
    }
    let main_model = env[&input.main_role.env_key()].clone();
    Ok(LaunchPlan {
        provider: input.provider,
        env,
        clear_env: inherited_env_to_clear(input.inherited_env_names),
        settings,
        aliases,
        main_model,
        unknown,
    })
}

fn valid_launch_text(text: &str) -> bool {
    !text.trim_matches(js_whitespace).is_empty()
        && text.encode_utf16().count() <= 4096
        && !text.contains(['\r', '\n', '\0'])
}

fn valid_tokens(value: &u64) -> bool {
    (1..=MAX_SAFE_INTEGER).contains(value)
}

fn loopback_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn split_context_marker(id: &str) -> (&str, bool) {
    id.split_at_checked(id.len().saturating_sub(4))
        .filter(|(_, suffix)| suffix.eq_ignore_ascii_case("[1m]"))
        .map_or((id, false), |(base, _)| (base, true))
}

fn strip_context_marker(id: &str) -> &str {
    split_context_marker(id).0.trim_matches(js_whitespace)
}

fn client_model_id(
    provider: SubscriptionProvider,
    role: ModelRole,
    model: &CatalogModel,
) -> String {
    let id = model.id.trim_matches(js_whitespace).to_ascii_lowercase();
    if provider != SubscriptionProvider::Antigravity
        || !id
            .strip_prefix("anthropic/")
            .unwrap_or(&id)
            .starts_with("claude-")
    {
        return format!("matou-{}", role.name());
    }
    if role == ModelRole::Fable {
        let id = strip_context_marker(&id);
        let id = id.strip_prefix("anthropic/").unwrap_or(id);
        return match id {
            "claude-fable-5" | "claude-fable-5-2" | "claude-fable-5-5" => id.into(),
            _ => "matou-fable".into(),
        };
    }
    if role == ModelRole::Haiku {
        return "claude-haiku-5-5".into();
    }
    let point_release = ["5.5", "5-5"].into_iter().any(|version| {
        let pattern = format!("{}-{version}", role.name());
        id.match_indices(&pattern).any(|(start, matched)| {
            !id.as_bytes()
                .get(start + matched.len())
                .is_some_and(|next| next.is_ascii_digit() || *next == b'.')
        })
    });
    let context = if split_context_marker(&id).1 || model.context_window.unwrap_or(0) >= 1_000_000 {
        "[1M]"
    } else {
        ""
    };
    let version = if point_release { "5-5" } else { "5" };
    format!("claude-{}-{version}{context}", role.name())
}

fn inherited_env_to_clear(names: &[&str]) -> BTreeSet<String> {
    let fixed = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
        "CLAUDE_CODE_FORCE_SUBAGENT_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
        "CLAUDE_CODE_DISABLE_1M_CONTEXT",
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
        "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
        "CLAUDE_CODE_DISABLE_AUTO_COMPACT",
        "DISABLE_COMPACT",
        "DISABLE_AUTO_COMPACT",
        "MAX_THINKING_TOKENS",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "AWS_BEARER_TOKEN_BEDROCK",
        "CLAUDE_CODE_AUTO_MODE_SERVER",
    ];
    let mut clear: BTreeSet<_> = fixed.into_iter().map(str::to_owned).collect();
    clear.insert("CLAUDE_CODE_GATEWAY_HINT_HEADERS".into());
    for role in ROLES {
        let key = role.env_key();
        for suffix in ["", "_NAME", "_DESCRIPTION"] {
            clear.insert(format!("{key}{suffix}"));
        }
    }
    clear.extend(
        names
            .iter()
            .copied()
            .filter(|name| is_claude_model_environment(OsStr::new(name)))
            .map(str::to_owned),
    );
    clear
}

#[derive(Debug, PartialEq, Eq)]
pub struct ManagementPlan {
    pub wait_for_credential_adoption: bool,
    pub write_aliases: bool,
    pub write_prefix: bool,
    pub wait_for_aliases: bool,
}

impl LaunchPlan {
    pub fn alias_provider(&self) -> &'static str {
        self.provider.router_provider()
    }

    /// Launch uses cached provider metadata and only compares the local registry.
    /// The caller handles credential validation before applying this plan.
    pub fn management_plan(
        &self,
        credential_active: bool,
        credential_disabled: bool,
        credential_requires_sign_in: bool,
        registered_aliases: &[ModelAlias],
        registered_model_ids: &[&str],
    ) -> ManagementPlan {
        let prefix = self.provider.prefix();
        let write_aliases = self.aliases.len() != registered_aliases.len()
            || self
                .aliases
                .iter()
                .any(|alias| !registered_aliases.contains(alias));
        let write_prefix = self.aliases.iter().any(|alias| {
            !registered_model_ids.contains(&format!("{prefix}/{}", alias.alias).as_str())
        });
        ManagementPlan {
            wait_for_credential_adoption: !credential_active
                || credential_disabled
                || credential_requires_sign_in,
            write_aliases,
            write_prefix,
            wait_for_aliases: write_aliases || write_prefix,
        }
    }
}

const MAX_MODEL_BODY_BYTES: usize = 4 * 1024 * 1024;
static NON_CHAT_MODEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)image|video|codex-auto-review|embedding|rerank|moderation|(?:^|[-_/])(?:tts|whisper|transcribe)(?:$|[-_/])")
        .expect("valid frozen model filter")
});

#[derive(Debug, PartialEq, Eq)]
pub struct CatalogUnavailable;

/// `account` is the decoded upstream catalog body. Only an explicit refresh
/// requires it; ordinary reads may fall back to local router definitions.
pub fn project_catalog(
    provider: SubscriptionProvider,
    account: Option<&Value>,
    registered: &[Value],
    require_account_catalog: bool,
) -> Result<Vec<CatalogModel>, CatalogUnavailable> {
    let prefix = format!("{}/", provider.prefix());
    let definitions = parse_catalog_entries(registered, None, &prefix);
    let by_id: HashMap<_, _> = definitions
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect();
    let account_models = account.and_then(parse_account_catalog);
    if require_account_catalog && account_models.is_none() {
        return Err(CatalogUnavailable);
    }
    let models = account_models.as_deref().unwrap_or(&definitions);
    let available: Vec<_> = models
        .iter()
        .filter(|model| !NON_CHAT_MODEL.is_match(&model.id))
        .map(|model| {
            let definition = by_id.get(model.id.as_str()).copied().unwrap_or(model);
            let preferred = if provider == SubscriptionProvider::ChatGpt {
                definition
            } else {
                model
            };
            let owned_by = match preferred.owned_by.as_deref() {
                Some("Codex") if provider == SubscriptionProvider::Antigravity => {
                    Some("Antigravity".into())
                }
                _ => preferred.owned_by.clone(),
            };
            let supported_efforts = [preferred, model, definition]
                .into_iter()
                .find(|source| !source.supported_efforts.is_empty())
                .map_or_else(Vec::new, |source| source.supported_efforts.clone());
            CatalogModel {
                id: model.id.clone(),
                display_name: preferred
                    .display_name
                    .clone()
                    .or_else(|| definition.display_name.clone()),
                owned_by,
                context_window: preferred.context_window.or(definition.context_window),
                max_context_window: preferred
                    .max_context_window
                    .or(model.max_context_window)
                    .or(definition.max_context_window),
                max_output_tokens: preferred.max_output_tokens.or(definition.max_output_tokens),
                supported_efforts,
            }
        })
        .collect();
    if require_account_catalog && available.is_empty() {
        return Err(CatalogUnavailable);
    }
    Ok(available)
}

fn parse_account_catalog(body: &Value) -> Option<Vec<CatalogModel>> {
    if body.get("error").is_some()
        || body["status"] == "error"
        || serde_json::to_vec(body).ok()?.len() > MAX_MODEL_BODY_BYTES
    {
        return None;
    }
    let entries = [&body["data"], &body["models"], &body["items"], body]
        .into_iter()
        .find_map(Value::as_array);
    let map = body["models"].as_object().or_else(|| {
        (entries.is_none()
            && ["data", "items", "models"]
                .into_iter()
                .all(|key| body.get(key).is_none()))
        .then(|| body.as_object())
        .flatten()
    });
    let models = parse_catalog_entries(entries.map_or(&[], Vec::as_slice), map, "");
    (!models.is_empty()).then_some(models)
}

fn parse_catalog_entries(
    entries: &[Value],
    map: Option<&serde_json::Map<String, Value>>,
    prefix: &str,
) -> Vec<CatalogModel> {
    let mut ids = HashSet::new();
    let mut models: Vec<_> = entries
        .iter()
        .map(|entry| (entry, None))
        .chain(
            map.into_iter()
                .flatten()
                .filter(|(_, entry)| entry.is_object())
                .map(|(key, entry)| (entry, Some(key.as_str()))),
        )
        .filter_map(|(entry, fallback)| catalog_entry(entry, fallback, prefix))
        .filter(|model| ids.insert(model.id.clone()))
        .collect();
    models.sort_by(|a, b| a.id.encode_utf16().cmp(b.id.encode_utf16()));
    models
}

fn catalog_entry(entry: &Value, fallback: Option<&str>, prefix: &str) -> Option<CatalogModel> {
    if (!prefix.is_empty() && !entry.is_object())
        || entry["visibility"] == "hide"
        || entry["isInternal"] == true
        || entry["internal"] == true
    {
        return None;
    }
    let fields = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| catalog_text(entry[*key].as_str()?))
    };
    let id = entry
        .as_str()
        .and_then(catalog_text)
        .or_else(|| fields(&["slug"]))
        .or_else(|| {
            entry["id"]
                .as_str()
                .and_then(|id| catalog_text(id.strip_prefix(prefix).unwrap_or(id)))
        })
        .or_else(|| {
            if fallback.is_none() {
                fields(&["model", "name"])
            } else {
                fallback.and_then(catalog_text)
            }
        })?;
    let lower = id.to_ascii_lowercase();
    if lower.starts_with("chat_") || lower.starts_with("tab_") {
        return None;
    }
    let numbers = |keys: &[&str]| {
        let count = keys
            .iter()
            .filter_map(|key| entry.get(*key))
            .find(|value| !value.is_null())?
            .as_f64()?;
        (count > 0.0 && count <= MAX_SAFE_INTEGER as f64 && count.fract() == 0.0)
            .then_some(count as u64)
    };
    let supported_efforts = [
        &entry["supported_reasoning_levels"],
        &entry["supportedEfforts"],
        &entry["thinking"]["levels"],
    ]
    .into_iter()
    .find(|value| !value.is_null())
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(|value| value.as_str().or_else(|| value["effort"].as_str()))
    .filter(|effort| EFFORTS[1..].contains(effort))
    .map(str::to_owned)
    .collect();
    Some(CatalogModel {
        id: id.into(),
        display_name: fields(&["display_name", "displayName", "name"]).map(str::to_owned),
        owned_by: Some(
            fields(&[
                "owned_by", "ownedBy", "provider", "vendor", "category", "owner",
            ])
            .unwrap_or("Codex")
            .into(),
        ),
        context_window: numbers(&[
            "context_window",
            "contextWindow",
            "context_length",
            "maxTokens",
        ]),
        max_context_window: numbers(&["max_context_window", "maxContextWindow"]),
        max_output_tokens: numbers(&[
            "max_output_tokens",
            "maxOutputTokens",
            "max_completion_tokens",
        ]),
        supported_efforts,
    })
}

fn catalog_text(text: &str) -> Option<&str> {
    let trimmed = text.trim_matches(js_whitespace);
    (!trimmed.is_empty() && text.encode_utf16().count() <= 4096).then_some(trimmed)
}

/// One router credential/model's in-memory quota cooldown; the transport
/// supplies the rate-limit deadline. Neither the clock nor a retry is run here.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct QuotaCooldown {
    retry_at_ms: Option<u64>,
}

impl QuotaCooldown {
    pub fn observe_http(
        &mut self,
        provider: SubscriptionProvider,
        disable_cooling_override: Option<bool>,
        status: u16,
        retry_at_ms: Option<u64>,
        now_ms: u64,
    ) {
        let disabled =
            disable_cooling_override.unwrap_or(provider == SubscriptionProvider::Antigravity);
        if disabled {
            self.retry_at_ms = None;
        } else if status == 429
            && let Some(deadline) = retry_at_ms.filter(|deadline| *deadline > now_ms)
        {
            self.retry_at_ms = Some(
                self.retry_at_ms
                    .map_or(deadline, |current| current.max(deadline)),
            );
        }
    }

    pub fn retry_at(&self, now_ms: u64) -> Option<u64> {
        self.retry_at_ms.filter(|deadline| *deadline > now_ms)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RouterRequest {
    pub local_router_target: bool,
    pub body_buffered: bool,
    pub recovery_available: bool,
    pub response_started: bool,
    pub cancelled: bool,
    pub replayed: bool,
}

impl RouterRequest {
    pub fn should_reconnect(self, error: io::ErrorKind) -> bool {
        self.local_router_target
            && self.body_buffered
            && self.recovery_available
            && !self.response_started
            && !self.cancelled
            && !self.replayed
            && matches!(
                error,
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::TimedOut
            )
    }

    /// After recovery, recheck cancellation and target authorization. A retry
    /// follows the recovered host/port and preserves the request path and query.
    pub fn retry_destination(
        self,
        error: io::ErrorKind,
        original: &Url,
        recovered: Option<&str>,
        target_allowed: bool,
    ) -> Option<Url> {
        if !target_allowed || !self.should_reconnect(error) {
            return None;
        }
        if !original.username().is_empty() || original.password().is_some() {
            return None;
        }
        // Only the destination origin is copied. Keep the same URL policy as launch.
        loopback_url(original.origin().ascii_serialization().as_str())?;
        let mut destination = original.clone();
        if let Some(recovered) = recovered {
            let origin = loopback_url(recovered)?;
            destination.set_host(origin.host_str()).ok()?;
            destination.set_port(origin.port()).ok()?;
        }
        Some(destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SubscriptionProvider::*;
    use io::ErrorKind::*;

    const ROUTER: &str = "http://127.0.0.1:4800";
    const CLIENT_KEY: &str = "fixture-router-client-key-canary";

    fn model(id: &str, context_window: u64) -> CatalogModel {
        CatalogModel {
            id: id.into(),
            context_window: Some(context_window),
            ..CatalogModel::default()
        }
    }

    fn input(default: &CatalogModel) -> LaunchInput<'_> {
        LaunchInput {
            provider: ChatGpt,
            provider_name: "Fixture subscription",
            router_url: ROUTER,
            client_key: CLIENT_KEY,
            models: RoleModels {
                default: Some(default),
                ..RoleModels::default()
            },
            main_role: ModelRole::Opus,
            context_window: None,
            inherited_env_names: &[],
        }
    }

    #[test]
    fn subscription_environment_keeps_settings_and_local_client_key_separate() {
        let mut default = model("fixture-main", 272_000);
        default.display_name = Some("  Fixture Main  ".into());
        default.max_output_tokens = Some(160_000);
        let fast = model("fixture-fast", 200_000);
        let mut launch = input(&default);
        launch.models.small_fast = Some(&fast);
        launch.inherited_env_names = &[
            "ANTHROPIC_DEFAULT_FUTURE_MODEL",
            "CLAUDE_CODE_NEW_SUBAGENT_MAIN_MODEL_OVERRIDE",
            "CLAUDE_CODE_MODEL_SUBAGENT",
            "PATH",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        ];
        let plan = build_launch(launch).unwrap();
        for name in [
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            "DISABLE_TELEMETRY",
            "DISABLE_ERROR_REPORTING",
        ] {
            assert_eq!(plan.env[name], "1");
            assert!(plan.settings["env"].get(name).is_none());
        }
        assert_eq!(plan.env["CLAUDE_CODE_AUTO_MODE_SERVER"], "0");
        assert_eq!(plan.env["ANTHROPIC_BASE_URL"], ROUTER);
        assert!(plan.env["ANTHROPIC_AUTH_TOKEN"] == CLIENT_KEY);
        assert!(!plan.settings.to_string().contains(CLIENT_KEY));
        assert_eq!(plan.settings["env"]["ANTHROPIC_API_KEY"], "");
        assert_eq!(plan.settings["env"]["CLAUDE_CODE_OAUTH_TOKEN"], "");
        for name in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_DEFAULT_FUTURE_MODEL",
            "CLAUDE_CODE_NEW_SUBAGENT_MAIN_MODEL_OVERRIDE",
        ] {
            assert!(plan.clear_env.contains(name));
        }
        assert!(!plan.clear_env.contains("PATH"));
        assert!(!plan.clear_env.contains("CLAUDE_CODE_MODEL_SUBAGENT"));
        assert!(!plan.env.contains_key("CLAUDE_CODE_SUBAGENT_MODEL"));
        assert_eq!(
            plan.env["ANTHROPIC_DEFAULT_FABLE_MODEL"],
            "matou-chatgpt/matou-fable"
        );
        assert_eq!(
            plan.env["ANTHROPIC_DEFAULT_FABLE_MODEL_NAME"],
            "Fixture Main"
        );
        assert_eq!(plan.main_model, "matou-chatgpt/matou-opus");
        assert_eq!(plan.alias_provider(), "codex");
        assert_eq!(plan.aliases[1].name, "fixture-main");
        assert_eq!(plan.aliases[1].alias, "matou-fable");
        assert_eq!(plan.aliases[3].name, "fixture-fast");
        assert_eq!(
            plan.settings["modelOverrides"]["claude-fable-5"],
            "matou-chatgpt/matou-fable"
        );
        assert_eq!(
            plan.settings["modelPicker"]["options"][0]["label"],
            "Opus · Fixture Main"
        );
        assert_eq!(
            plan.settings["modelPicker"]["options"][0]["description"],
            "Fixture subscription · fixture-main · 272K"
        );
        assert_eq!(plan.settings["autoCompactWindow"], 200_000);
        assert_eq!(plan.env["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], "128000");
        assert_eq!(plan.env["CLAUDE_CODE_DISABLE_1M_CONTEXT"], "1");
        assert!(plan.unknown.is_empty());
    }

    #[test]
    fn fable_uses_its_own_subscription_alias_name_and_window() {
        let opus = model("fixture-opus", 512_000);
        let fable = model("fixture-fable", 200_000);
        for provider in [ChatGpt, Antigravity] {
            let mut launch = input(&opus);
            launch.provider = provider;
            launch.models.fable = Some(&fable);
            launch.main_role = ModelRole::Fable;
            let plan = build_launch(launch).unwrap();
            assert_eq!(
                plan.main_model,
                format!("{}/matou-fable", provider.prefix())
            );
            assert_eq!(
                plan.env["ANTHROPIC_DEFAULT_FABLE_MODEL_NAME"],
                "fixture-fable"
            );
            assert_eq!(
                plan.settings["modelPicker"]["options"][1]["label"],
                "Fable · fixture-fable"
            );
            assert_eq!(plan.settings["autoCompactWindow"], 200_000);
            for alias in ["claude-fable-5", "claude-fable-5-2", "claude-fable-5-5"] {
                assert_eq!(plan.settings["modelOverrides"][alias], plan.main_model);
            }
            assert!(
                plan.aliases
                    .iter()
                    .any(|item| item.alias == "matou-fable" && item.name == fable.id)
            );
        }
    }

    #[test]
    fn antigravity_fable_keeps_only_the_three_native_ids() {
        let opus = model("anthropic/claude-opus-5-5[1M]", 1_000_000);
        for (id, expected) in [
            ("anthropic/claude-fable-5[1M]", "claude-fable-5"),
            ("claude-fable-5-2", "claude-fable-5-2"),
            ("CLAUDE-FABLE-5-5[1m]", "claude-fable-5-5"),
            ("claude-fable-5-7", "matou-fable"),
            ("anthropic/claude-opus-5[1M]", "matou-fable"),
            ("gemini-pro", "matou-fable"),
            ("", "matou-fable"),
        ] {
            let fable = model(id, 1_000_000);
            let mut input = input(&opus);
            input.provider = SubscriptionProvider::Antigravity;
            input.models.fable = (!id.is_empty()).then_some(&fable);
            let projection = build_launch(input).unwrap();
            assert_eq!(
                projection.env["ANTHROPIC_DEFAULT_FABLE_MODEL"],
                format!("matou-antigravity/{expected}")
            );
            assert!(
                projection
                    .aliases
                    .iter()
                    .any(|alias| alias.alias == FABLE_FALLBACK_ALIAS
                        && alias.name == "anthropic/claude-opus-5-5")
            );
            assert!(
                projection
                    .aliases
                    .iter()
                    .any(|alias| alias.alias == expected
                        && alias.name
                            == if id.is_empty() {
                                strip_context_marker(&opus.id)
                            } else {
                                strip_context_marker(id)
                            })
            );
        }
    }

    #[test]
    fn antigravity_role_aliases_apply_point_release_and_context_rules() {
        let opus = model("anthropic/claude-opus-5.5-latest[1m]", 1_500_000);
        let sonnet = model("claude-sonnet-5-50", 1_500_000);
        let mut launch = input(&opus);
        launch.provider = Antigravity;
        launch.models.sonnet = Some(&sonnet);
        launch.main_role = ModelRole::Sonnet;
        let plan = build_launch(launch).unwrap();
        assert_eq!(plan.alias_provider(), "antigravity");
        assert_eq!(plan.main_model, "matou-antigravity/claude-sonnet-5[1M]");
        assert_eq!(
            plan.env["ANTHROPIC_DEFAULT_OPUS_MODEL"],
            "matou-antigravity/claude-opus-5-5[1M]"
        );
        assert_eq!(
            plan.env["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
            "matou-antigravity/claude-haiku-5-5"
        );
        assert_eq!(plan.aliases[0].name, "anthropic/claude-opus-5.5-latest");
        assert_eq!(plan.aliases[0].alias, "claude-opus-5-5");
        assert_eq!(plan.settings["autoCompactWindow"], 1_000_000);
        assert!(!plan.env.contains_key("CLAUDE_CODE_DISABLE_1M_CONTEXT"));
        assert_eq!(plan.unknown, ["max-output-tokens"]);
    }

    #[test]
    fn platform_loopback_and_context_compatibility_boundaries() {
        assert!(!subscription_supported(true, Some(14)));
        assert!(!subscription_supported(true, None));
        assert!(subscription_supported(true, Some(15)));
        assert!(subscription_supported(false, None));
        let small = model("fixture-short-context", 99_999);
        assert!(matches!(
            build_launch(input(&small)),
            Err(LaunchError::UnsupportedContextWindow)
        ));
        let mut launch = input(&small);
        launch.context_window = Some(512_000);
        let plan_512k = build_launch(launch).unwrap();
        assert_eq!(plan_512k.settings["autoCompactWindow"], 512_000);
        assert!(!plan_512k.env.contains_key("CLAUDE_CODE_DISABLE_1M_CONTEXT"));
        assert!(plan_512k.settings["env"].get("CLAUDE_CODE_DISABLE_1M_CONTEXT").is_none());
        let mut launch_872k = input(&small);
        launch_872k.context_window = Some(872_000);
        let plan_872k = build_launch(launch_872k).unwrap();
        assert_eq!(plan_872k.settings["autoCompactWindow"], 872_000);
        assert!(!plan_872k.env.contains_key("CLAUDE_CODE_DISABLE_1M_CONTEXT"));
        assert!(plan_872k.settings["env"].get("CLAUDE_CODE_DISABLE_1M_CONTEXT").is_none());
        let unknown = CatalogModel {
            id: "fixture-unknown".into(),
            ..CatalogModel::default()
        };
        let plan = build_launch(input(&unknown)).unwrap();
        assert_eq!(plan.unknown, ["context-window", "max-output-tokens"]);
        assert!(plan.settings.get("autoCompactWindow").is_none());
        let mut launch = input(&small);
        launch.models = RoleModels {
            opus: Some(&small),
            ..RoleModels::default()
        };
        assert!(
            matches!(build_launch(launch), Err(LaunchError::MissingRoleModels(roles)) if roles == [ModelRole::Sonnet, ModelRole::Haiku])
        );
        for router_url in [
            "https://localhost",
            "http://example.test",
            "http://fixture@localhost",
            "http://127.0.0.1?x=1",
            "http://localhost/#fragment",
        ] {
            let mut launch = input(&unknown);
            launch.router_url = router_url;
            assert!(matches!(
                build_launch(launch),
                Err(LaunchError::InvalidInput)
            ));
        }
        for router_url in ["http://localhost:4800", "http://[::1]:4800"] {
            let mut launch = input(&unknown);
            launch.router_url = router_url;
            assert!(build_launch(launch).is_ok());
        }
    }

    #[test]
    fn repeated_launch_suppresses_registry_writes_and_adoption_polling() {
        let model = model("fixture-main", 272_000);
        let launch = build_launch(input(&model)).unwrap();
        let mut registered_aliases = launch.aliases.clone();
        registered_aliases.reverse();
        let registered: Vec<_> = registered_aliases
            .iter()
            .map(|alias| format!("matou-chatgpt/{}", alias.alias))
            .collect();
        let registered: Vec<_> = registered.iter().map(String::as_str).collect();
        assert_eq!(
            launch.management_plan(true, false, false, &registered_aliases, &registered),
            ManagementPlan {
                wait_for_credential_adoption: false,
                write_aliases: false,
                write_prefix: false,
                wait_for_aliases: false,
            }
        );
        for (active, disabled, needs_sign_in) in [
            (false, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            assert!(
                launch
                    .management_plan(
                        active,
                        disabled,
                        needs_sign_in,
                        &registered_aliases,
                        &registered
                    )
                    .wait_for_credential_adoption
            );
        }
        let missing_prefix =
            launch.management_plan(true, false, false, &registered_aliases, &registered[1..]);
        assert!(!missing_prefix.write_aliases);
        assert!(missing_prefix.write_prefix && missing_prefix.wait_for_aliases);
        registered_aliases[0].fork = false;
        let stale_alias =
            launch.management_plan(true, false, false, &registered_aliases, &registered);
        assert!(!stale_alias.write_prefix);
        assert!(stale_alias.write_aliases && stale_alias.wait_for_aliases);
    }

    #[test]
    fn catalog_filters_non_chat_and_keeps_account_availability_with_frozen_metadata_priority() {
        let account = json!({"data": [
            {"slug":"fixture-main", "displayName":"Live name", "contextWindow":272000, "maxContextWindow":800000,
                "maxOutputTokens":128000, "supported_reasoning_levels":["invalid", {"effort":"high"}]},
            {"id":"fixture-main", "displayName":"Duplicate"}, " fixture-new ",
            {"id":"hidden-model", "visibility":"hide"}, {"id":"internal-a", "internal":true},
            {"id":"internal-b", "isInternal":true}, "CHAT_internal", "tab_seen", "fixture-image-model", "fixture-video",
            "fixture-codex-auto-review", "fixture-embedding", "fixture-rerank", "fixture-moderation", "fixture/tts",
            "fixture-whisper-v2", "fixture_transcribe",
        ]});
        let registered = [
            json!({"id":"matou-chatgpt/fixture-main", "display_name":"Router name", "context_window":512000, "max_output_tokens":64000}),
            json!({"id":"matou-chatgpt/fixture-unavailable"}),
        ];
        let catalog = project_catalog(ChatGpt, Some(&account), &registered, true).unwrap();
        assert_eq!(
            catalog
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["fixture-main", "fixture-new"]
        );
        let main = &catalog[0];
        assert_eq!(main.display_name.as_deref(), Some("Router name"));
        assert_eq!(main.context_window, Some(512_000));
        assert_eq!(main.max_context_window, Some(800_000));
        assert_eq!(main.max_output_tokens, Some(64_000));
        assert_eq!(main.supported_efforts, ["high"]);

        let registered = [
            json!({"id":"matou-antigravity/fixture-main", "contextWindow":512000, "maxOutputTokens":64000}),
        ];
        let catalog = project_catalog(Antigravity, Some(&account), &registered, false).unwrap();
        assert_eq!(catalog[0].display_name.as_deref(), Some("Live name"));
        assert_eq!(catalog[0].owned_by.as_deref(), Some("Antigravity"));
        assert_eq!(catalog[0].context_window, Some(272_000));
        assert_eq!(catalog[0].max_output_tokens, Some(128_000));
    }

    #[test]
    fn catalog_fallback_distinguishes_unavailable_and_filtered_account_responses() {
        let registered = [
            json!({"id":"matou-chatgpt/fixture-main"}),
            json!({"id":"other-tts"}),
            json!("invalid-definition"),
        ];
        for unavailable in [
            None,
            Some(json!({"error":"offline"})),
            Some(json!({"items":[]})),
        ] {
            let catalog =
                project_catalog(ChatGpt, unavailable.as_ref(), &registered, false).unwrap();
            assert_eq!(catalog.len(), 1);
            assert_eq!(catalog[0].id, "fixture-main");
            assert_eq!(
                project_catalog(ChatGpt, unavailable.as_ref(), &registered, true),
                Err(CatalogUnavailable)
            );
        }
        let filtered = json!(["fixture-embedding"]);
        assert!(
            project_catalog(ChatGpt, Some(&filtered), &registered, false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            project_catalog(ChatGpt, Some(&filtered), &registered, true),
            Err(CatalogUnavailable)
        );
        let models_map =
            json!({"models": {"fixture-key": {"name":"Display only", "context_length":512000.0}}});
        let catalog = project_catalog(ChatGpt, Some(&models_map), &[], true).unwrap();
        assert_eq!(catalog[0].id, "fixture-key");
        assert_eq!(catalog[0].context_window, Some(512_000));
    }

    #[test]
    fn quota_cooldown_is_429_only_and_ends_at_the_deadline() {
        let mut quota = QuotaCooldown::default();
        for (status, deadline) in [(503, Some(300)), (429, None), (429, Some(100))] {
            quota.observe_http(ChatGpt, None, status, deadline, 100);
            assert_eq!(quota.retry_at(100), None);
        }
        quota.observe_http(ChatGpt, None, 429, Some(200), 100);
        quota.observe_http(ChatGpt, None, 429, Some(180), 110);
        quota.observe_http(ChatGpt, None, 200, None, 120);
        assert_eq!(quota.retry_at(199), Some(200));
        assert_eq!(quota.retry_at(200), None);
        quota.observe_http(ChatGpt, None, 429, Some(350), 210);
        assert_eq!(quota.retry_at(210), Some(350));
        let mut antigravity = QuotaCooldown::default();
        antigravity.observe_http(Antigravity, None, 429, Some(200), 100);
        assert_eq!(antigravity.retry_at(100), None);
        antigravity.observe_http(Antigravity, Some(false), 429, Some(200), 100);
        assert_eq!(antigravity.retry_at(100), Some(200));
    }

    #[test]
    fn only_buffered_local_router_disconnects_can_replay_once() {
        let request = RouterRequest {
            local_router_target: true,
            body_buffered: true,
            recovery_available: true,
            ..RouterRequest::default()
        };
        for error in [ConnectionRefused, ConnectionReset, BrokenPipe, TimedOut] {
            assert!(request.should_reconnect(error));
        }
        for error in [
            ConnectionAborted,
            NotConnected,
            PermissionDenied,
            InvalidData,
            Other,
        ] {
            assert!(!request.should_reconnect(error));
        }
        for ineligible in [
            RouterRequest {
                local_router_target: false,
                ..request
            },
            RouterRequest {
                body_buffered: false,
                ..request
            },
            RouterRequest {
                recovery_available: false,
                ..request
            },
            RouterRequest {
                response_started: true,
                ..request
            },
            RouterRequest {
                cancelled: true,
                ..request
            },
            RouterRequest {
                replayed: true,
                ..request
            },
        ] {
            assert!(!ineligible.should_reconnect(ConnectionReset));
        }
        let original = Url::parse("http://127.0.0.1:4800/v1/messages?beta=true").unwrap();
        assert_eq!(
            request.retry_destination(ConnectionRefused, &original, None, true),
            Some(original.clone())
        );
        let recovered = request
            .retry_destination(ConnectionReset, &original, Some("http://[::1]:4801"), true)
            .unwrap();
        assert_eq!(
            recovered.as_str(),
            "http://[::1]:4801/v1/messages?beta=true"
        );
        assert!(
            request
                .retry_destination(
                    ConnectionReset,
                    &original,
                    Some("http://example.test"),
                    true
                )
                .is_none()
        );
        assert!(
            request
                .retry_destination(ConnectionReset, &original, None, false)
                .is_none()
        );
    }
}
