//! Pure presentation rules for the Claude Code compaction-model block.
//!
//! The shell owns the editable draft and the runtime owns persistence. These
//! helpers only interpret the redacted provider rows and the persisted
//! `{mode, effort, providerId, model}` preference, mirroring the frozen
//! AgentModelSettings form.
use matou_native_core::provider_launch::EFFORTS;
use matou_native_core::sessions::InterfaceLocale;
use serde_json::{Map, Value, json};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CompactionDraft {
    pub mode: String,
    pub effort: String,
    pub provider_id: Option<String>,
    pub model: String,
}

impl CompactionDraft {
    pub(crate) fn from_value(value: &Value) -> Self {
        Self {
            mode: value["mode"].as_str().unwrap_or("follow").to_owned(),
            effort: value["effort"].as_str().unwrap_or("follow").to_owned(),
            provider_id: value["providerId"]
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .map(str::to_owned),
            model: value["model"].as_str().unwrap_or_default().to_owned(),
        }
    }

    /// The exact payload written back. Empty provider/model fields are omitted
    /// so a follow draft stays identical to the frozen persisted shape.
    pub(crate) fn input(&self) -> Value {
        let mut value = Map::new();
        value.insert("mode".into(), json!(self.mode));
        value.insert("effort".into(), json!(self.effort));
        if let Some(provider_id) = self.provider_id.as_deref().filter(|id| !id.is_empty()) {
            value.insert("providerId".into(), json!(provider_id));
        }
        if !self.model.trim().is_empty() {
            value.insert("model".into(), json!(self.model.trim()));
        }
        Value::Object(value)
    }

    pub(crate) fn is_dirty(&self, persisted: &Value) -> bool {
        self.input() != Self::from_value(persisted).input()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompactionOption {
    pub value: String,
    pub label: String,
    pub enabled: bool,
}

pub(crate) fn subscription(provider: &Value) -> bool {
    provider["subscription"].is_object()
}

pub(crate) fn subscription_status(provider: &Value) -> &str {
    provider["subscription"]["account"]["status"]
        .as_str()
        .unwrap_or_default()
}

/// A subscription has to be signed in; an API target needs a key or the
/// built-in official account. A saved target that fails this test stays visible
/// as an unavailable option so the user can see what will be replaced.
pub(crate) fn provider_available(provider: &Value) -> bool {
    if subscription(provider) {
        subscription_status(provider) == "ready"
    } else {
        provider["builtIn"] == true || provider["hasApiKey"] == true
    }
}

fn provider_name(provider: &Value) -> &str {
    provider["name"].as_str().unwrap_or_default()
}

pub(crate) fn provider_options(
    providers: &[Value],
    locale: InterfaceLocale,
) -> Vec<CompactionOption> {
    providers
        .iter()
        .filter_map(|provider| {
            let id = provider["id"].as_str()?;
            let name = provider_name(provider);
            let enabled = provider_available(provider);
            let suffix =
                if subscription(provider) && subscription_status(provider) == "needs-sign-in" {
                    Some(locale.text("需要重新登录", "Sign in again"))
                } else if !enabled {
                    Some(locale.text("不可用", "Unavailable"))
                } else {
                    None
                };
            Some(CompactionOption {
                value: id.to_owned(),
                label: suffix
                    .map(|suffix| format!("{name} · {suffix}"))
                    .unwrap_or_else(|| name.to_owned()),
                enabled,
            })
        })
        .collect()
}

fn model_rows(provider: &Value) -> Option<&[Value]> {
    provider["subscription"]
        .get("modelMetadata")
        .or_else(|| provider.get("modelMetadata"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

pub(crate) fn usable_model(id: &str) -> bool {
    !["image", "video", "codex-auto-review"]
        .iter()
        .any(|part| id.to_ascii_lowercase().contains(part))
}

/// Compaction is a chat request: generated media rows and the review helper
/// cannot be selected. Without a directory the provider's saved tier models are
/// offered as suggestions, matching the assignment rows' fallback.
pub(crate) fn model_options(provider: &Value) -> Vec<CompactionOption> {
    if let Some(rows) = model_rows(provider) {
        let mut seen = Vec::new();
        return rows
            .iter()
            .filter_map(|row| {
                let id = row["id"].as_str()?;
                if id.is_empty() || !usable_model(id) || seen.contains(&id) {
                    return None;
                }
                seen.push(id);
                Some(CompactionOption {
                    value: id.to_owned(),
                    label: row["displayName"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .unwrap_or(id)
                        .to_owned(),
                    enabled: true,
                })
            })
            .collect();
    }
    let mut seen = Vec::new();
    let mut options = Vec::new();
    for id in std::iter::once(provider["model"].as_str()).chain(
        matou_native_core::claude_role_budget::CLAUDE_ROLES
            .into_iter()
            .map(|role| provider["modelMapping"][role].as_str()),
    ) {
        let Some(id) = id
            .map(str::trim)
            .filter(|id| !id.is_empty() && *id != "CLI 默认" && *id != "__cli_default__")
        else {
            continue;
        };
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        options.push(CompactionOption {
            value: id.to_owned(),
            label: id.to_owned(),
            enabled: true,
        });
    }
    options
}

pub(crate) fn model_name(provider: &Value, id: &str) -> String {
    model_rows(provider)
        .into_iter()
        .flatten()
        .find(|row| row["id"] == id)
        .and_then(|row| row["displayName"].as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(id)
        .to_owned()
}

fn metadata_efforts(provider: &Value, model: &str) -> Option<Vec<String>> {
    model_rows(provider)?
        .iter()
        .find(|row| row["id"] == model)?
        .get("supportedEfforts")
        .and_then(Value::as_array)
        .map(|levels| {
            EFFORTS[1..]
                .iter()
                .filter(|effort| levels.iter().any(|level| level.as_str() == Some(**effort)))
                .map(|effort| (*effort).to_owned())
                .collect()
        })
}

/// Metadata wins. Before it arrives, the model family supplies the candidate
/// range exactly as the frozen `defaultEfforts`.
fn fallback_efforts(provider: &Value, model: &str) -> Vec<String> {
    let label = format!(
        "{} {} {model}",
        provider["id"].as_str().unwrap_or_default(),
        provider_name(provider)
    )
    .to_ascii_lowercase();
    let gpt_family = label
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|word| {
            matches!(word, "gpt" | "codex") || matches!(word.as_bytes(), [b'o', b'1'..=b'9'])
        });
    let claude_family = ["claude", "opus", "fable", "sonnet", "haiku"]
        .iter()
        .any(|part| label.contains(part));
    EFFORTS[1..5]
        .iter()
        .map(|effort| (*effort).to_owned())
        .chain((!gpt_family && claude_family).then(|| "max".to_owned()))
        .collect()
}

pub(crate) fn effort_values(provider: &Value, model: &str, current: &str) -> Vec<String> {
    let mut choices = vec!["follow".to_owned()];
    choices.extend(
        metadata_efforts(provider, model).unwrap_or_else(|| fallback_efforts(provider, model)),
    );
    if current != "follow" && !choices.iter().any(|effort| effort == current) {
        choices.push(current.to_owned());
    }
    choices
}

/// True when the selected model has no explicit supported-effort metadata, so
/// the picker is showing candidate levels from the model family.
pub(crate) fn effort_fallback(provider: &Value, model: &str) -> bool {
    !model.trim().is_empty() && metadata_efforts(provider, model).is_none()
}

pub(crate) fn effort_supported(provider: &Value, model: &str, effort: &str) -> bool {
    effort == "follow"
        || metadata_efforts(provider, model)
            .unwrap_or_else(|| fallback_efforts(provider, model))
            .iter()
            .any(|candidate| candidate == effort)
}

pub(crate) fn selected_provider<'a>(
    draft: &CompactionDraft,
    providers: &'a [Value],
) -> Option<&'a Value> {
    let id = draft.provider_id.as_deref()?;
    providers
        .iter()
        .find(|provider| provider["id"].as_str() == Some(id))
}

pub(crate) fn validation(
    draft: &CompactionDraft,
    providers: &[Value],
    locale: InterfaceLocale,
) -> Option<&'static str> {
    if draft.mode != "custom" {
        return None;
    }
    let Some(provider_id) = draft.provider_id.as_deref().filter(|id| !id.is_empty()) else {
        return Some(locale.text("请选择压缩供应商。", "Choose a compaction provider."));
    };
    let Some(provider) = providers
        .iter()
        .find(|provider| provider["id"].as_str() == Some(provider_id))
    else {
        return Some(locale.text(
            "已保存的压缩供应商已不存在；原选择已保留，请重新选择。",
            "The saved compaction provider no longer exists. Its selection was kept; choose another provider.",
        ));
    };
    if !provider_available(provider) {
        return Some(locale.text(
            "该供应商当前不支持独立压缩；请重新选择。",
            "This provider does not support separate compaction. Choose another provider.",
        ));
    }
    if draft.model.trim().is_empty() {
        return Some(locale.text(
            "请选择或输入压缩模型。",
            "Choose or enter a compaction model.",
        ));
    }
    if !effort_supported(provider, draft.model.trim(), &draft.effort) {
        return Some(locale.text(
            "当前推理强度不受该模型支持；原选择已保留，请重新选择。",
            "The selected reasoning effort is not supported by this model. Its value was kept; choose another effort.",
        ));
    }
    None
}

pub(crate) fn effort_label(effort: &str, locale: InterfaceLocale) -> &'static str {
    match effort {
        "follow" => locale.text("跟随会话", "Follow session"),
        "low" => locale.text("低", "Low"),
        "medium" => locale.text("中", "Medium"),
        "high" => locale.text("高", "High"),
        "xhigh" => locale.text("极高", "XHigh"),
        "max" => locale.text("最大", "Max"),
        _ => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use InterfaceLocale::{Chinese, English};

    fn codex_ready() -> Value {
        json!({
            "id": "sub", "name": "ChatGPT", "kind": "subscription",
            "subscription": {
                "provider": "codex", "account": {"status": "ready"},
                "modelMetadata": [
                    {"id": "gpt-5", "displayName": "GPT 5", "supportedEfforts": ["low", "high"]},
                    {"id": "image-only"}
                ]
            }
        })
    }

    fn api() -> Value {
        json!({
            "id": "api", "name": "API", "hasApiKey": true,
            "modelMetadata": [{"id": "claude-sonnet", "displayName": "Sonnet", "supportedEfforts": ["medium"]}]
        })
    }

    #[test]
    fn draft_round_trips_and_detects_edits() {
        let persisted =
            json!({"mode": "custom", "effort": "high", "providerId": "sub", "model": "gpt-5"});
        let mut draft = CompactionDraft::from_value(&persisted);
        assert_eq!(draft.input(), persisted);
        assert!(!draft.is_dirty(&persisted));
        draft.effort = "low".into();
        assert!(draft.is_dirty(&persisted));
        draft.effort = "high".into();
        assert!(!draft.is_dirty(&persisted));
        let follow = CompactionDraft::from_value(&json!({"mode": "follow", "effort": "follow"}));
        assert_eq!(
            follow.input(),
            json!({"mode": "follow", "effort": "follow"})
        );
    }

    #[test]
    fn providers_show_signed_out_and_keyless_targets_but_disable_them() {
        let signed_out = json!({"id": "out", "name": "Out", "kind": "subscription",
            "subscription": {"provider": "codex", "account": {"status": "needs-sign-in"}}});
        let keyless = json!({"id": "no-key", "name": "No key", "hasApiKey": false});
        let official = json!({"id": "official", "name": "Official", "builtIn": true});
        let options = provider_options(&[codex_ready(), signed_out, keyless, official], Chinese);
        assert_eq!(options.len(), 4);
        assert!(options[0].enabled);
        assert!(!options[1].enabled);
        assert_eq!(options[1].label, "Out · 需要重新登录");
        assert!(!options[2].enabled);
        assert_eq!(options[2].label, "No key · 不可用");
        assert!(options[3].enabled);
    }

    #[test]
    fn models_filter_generated_rows_and_fall_back_to_saved_tiers() {
        let options = model_options(&codex_ready());
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].value, "gpt-5");
        assert_eq!(options[0].label, "GPT 5");
        let legacy = json!({"model": "CLI 默认", "modelMapping": {"opus": "tier-opus", "sonnet": "tier-opus"}});
        assert_eq!(
            model_options(&legacy)
                .into_iter()
                .map(|option| option.value)
                .collect::<Vec<_>>(),
            ["tier-opus"]
        );
    }

    #[test]
    fn efforts_use_metadata_then_family_fallbacks() {
        assert_eq!(
            effort_values(&codex_ready(), "gpt-5", "follow"),
            ["follow", "low", "high"]
        );
        let api = api();
        assert_eq!(
            effort_values(&api, "claude-sonnet", "follow"),
            ["follow", "medium"]
        );
        let unknown = json!({"id": "api", "name": "API", "hasApiKey": true});
        assert_eq!(
            effort_values(&unknown, "claude-opus", "follow"),
            ["follow", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            effort_values(&unknown, "gpt-5", "follow"),
            ["follow", "low", "medium", "high", "xhigh"]
        );
        // A saved unsupported effort stays visible and fails validation.
        assert_eq!(
            effort_values(&api, "claude-sonnet", "max"),
            ["follow", "medium", "max"]
        );
    }

    #[test]
    fn validation_follows_the_frozen_order_and_copy() {
        let providers = vec![codex_ready(), api()];
        let mut draft = CompactionDraft {
            mode: "custom".into(),
            effort: "follow".into(),
            provider_id: None,
            model: String::new(),
        };
        assert_eq!(
            validation(&draft, &providers, Chinese),
            Some("请选择压缩供应商。")
        );
        draft.provider_id = Some("gone".into());
        assert!(
            validation(&draft, &providers, Chinese)
                .unwrap()
                .contains("已不存在")
        );
        draft.provider_id = Some("sub".into());
        draft.model = String::new();
        assert_eq!(
            validation(&draft, &providers, Chinese),
            Some("请选择或输入压缩模型。")
        );
        draft.model = "gpt-5".into();
        draft.effort = "max".into();
        assert!(
            validation(&draft, &providers, Chinese)
                .unwrap()
                .contains("推理强度")
        );
        draft.effort = "high".into();
        assert_eq!(validation(&draft, &providers, Chinese), None);
        draft.mode = "follow".into();
        assert_eq!(validation(&draft, &providers, Chinese), None);
        assert_eq!(effort_label("xhigh", English), "XHigh");
    }
}
