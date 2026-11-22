//! Provider settings use the runtime's redacted Graph replies.
use std::sync::atomic::{AtomicU64, Ordering};

use matou_native_core::provider_launch::EFFORTS;
use matou_native_runtime::protocol::{ProviderSettingsReply, ProviderSettingsResult};
use serde_json::{Value, json};

use crate::runtime_bridge::{Command, ConnectionPhase, RuntimeCommands};

const CLI: &str = "claude-code";
const ROLES: [&str; 3] = ["opus", "sonnet", "haiku"];
const CONNECTING: &str = "供应商配置服务正在连接，请稍后重试";
const LOAD_FAILED: &str = "供应商配置载入失败";
const SAVE_FAILED: &str = "供应商配置保存失败";
pub(crate) const DELETE_FAILED: &str = "删除失败";
const SWITCH_FAILED: &str = "切换失败";
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default, PartialEq)]
pub(crate) struct ProviderDraft {
    pub(crate) id: Option<String>,
    pub(crate) cli: String,
    pub(crate) subscription_kind: Option<String>,
    pub(crate) reasoning_effort: String,
    pub(crate) context_window_mode: Option<String>,
    pub(crate) legacy_context_window: bool,
    pub(crate) fetched_metadata: Option<Value>,
    pub(crate) ignore_model_efforts: bool,
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) model_mapping: [String; 3],
    pub(crate) effort_mapping: [String; 3],
    pub(crate) built_in: bool,
    pub(crate) has_api_key: bool,
    pub(crate) original_endpoint: String,
    pub(crate) model_metadata: Value,
}

impl ProviderDraft {
    pub(crate) fn from_provider(provider: &Value) -> Self {
        let endpoint = field(provider, "endpoint");
        let model = field(provider, "model");
        Self {
            id: provider["id"].as_str().map(str::to_owned),
            cli: field(provider, "cli"),
            subscription_kind: (provider["kind"] == "subscription")
                .then(|| field(&provider["subscription"], "provider")),
            reasoning_effort: field(provider, "reasoningEffort"),
            context_window_mode: provider["contextWindowMode"].as_str().map(str::to_owned),
            legacy_context_window: provider["contextWindow"].as_u64().is_some(),
            name: field(provider, "name"),
            original_endpoint: endpoint.clone(),
            endpoint,
            model: if model == "CLI 默认" {
                String::new()
            } else {
                model
            },
            model_mapping: ROLES.map(|role| field(&provider["modelMapping"], role)),
            effort_mapping: ROLES.map(|role| field(&provider["effortMapping"], role)),
            built_in: provider["builtIn"] == true,
            has_api_key: provider["hasApiKey"] == true,
            model_metadata: if provider["kind"] == "subscription" {
                provider["subscription"]["modelMetadata"].clone()
            } else {
                provider["modelMetadata"].clone()
            },
            ..Default::default()
        }
    }

    fn cli(&self) -> &str {
        if self.cli.is_empty() { CLI } else { &self.cli }
    }

    pub(crate) fn is_codex(&self) -> bool {
        self.cli() == "codex"
    }

    pub(crate) fn context_mode(&self) -> &str {
        self.context_window_mode.as_deref().unwrap_or_else(|| {
            if self.subscription_kind.as_deref() == Some("codex") {
                "512k"
            } else {
                "default"
            }
        })
    }

    pub(crate) fn available_models(&self) -> Vec<&Value> {
        let metadata = if self.subscription_kind.is_some() {
            &self.model_metadata
        } else if let Some(metadata) = &self.fetched_metadata {
            metadata
        } else if !self.endpoint_changed() && self.api_key.trim().is_empty() {
            &self.model_metadata
        } else {
            return Vec::new();
        };
        metadata
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| {
                model["id"].as_str().is_some_and(|id| {
                    !id.is_empty()
                        && (self.subscription_kind.is_none() || {
                            let id = id.to_ascii_lowercase();
                            !["image", "video", "codex-auto-review"]
                                .iter()
                                .any(|part| id.contains(part))
                        })
                })
            })
            .collect()
    }

    pub(crate) fn subscription_models_valid(&self) -> bool {
        self.subscription_kind.is_none() || {
            let models = self.available_models();
            self.model_mapping
                .iter()
                .all(|id| models.iter().any(|model| model["id"] == id.trim()))
        }
    }

    fn endpoint_changed(&self) -> bool {
        without_trailing_slash(self.original_endpoint.trim())
            != without_trailing_slash(self.endpoint.trim())
    }

    fn validation_error(&self) -> Option<String> {
        // Frozen save reports the last failed check in this order.
        let key = self.api_key.trim();
        let mut error = None;
        if self.name.trim().is_empty() {
            error = Some("请填写显示名称。".to_owned());
        }
        if self.subscription_kind.is_some() {
            if !self.subscription_models_valid() {
                return Some("没有读到可用的模型，请重新选择。".into());
            }
            return self.invalid_effort().or(error);
        }
        if !endpoint_shape_valid(self.endpoint.trim()) {
            error = Some("请填写有效的 API 基础地址，例如 https://api.example.com/v1。".into());
        }
        if key.is_empty() && !self.has_api_key && !self.built_in {
            error = Some("请先填写 API Key。".into());
        }
        if !key.is_empty() && !key_valid(key) {
            error = Some("请仅粘贴密钥本身，不包含空格、换行或变量名。".into());
        }
        if self.model.trim().is_empty() && !self.built_in {
            error = Some("请选择或填写默认模型。".into());
        }
        if self.has_api_key && self.endpoint_changed() && key.is_empty() {
            error = Some("地址已更改，请重新填写此服务的密钥，避免把原密钥发送到其他服务。".into());
        }
        self.invalid_effort().or(error)
    }

    fn invalid_effort(&self) -> Option<String> {
        if self.is_codex() {
            return (!self.effort_supported(0, &self.reasoning_effort))
                .then(|| "当前模型不支持所选推理强度，请重新选择。".into());
        }
        self.effort_mapping
            .iter()
            .enumerate()
            .find(|(index, effort)| !self.effort_supported(*index, effort))
            .map(|(index, _)| format!("{} 的推理强度不受当前模型支持，请重新选择。", ROLES[index]))
    }

    fn base_efforts(&self, index: usize) -> Vec<String> {
        let model = if self.is_codex() {
            &self.model
        } else {
            &self.model_mapping[index]
        };
        let selected = self
            .available_models()
            .into_iter()
            .find(|row| row["id"] == model.trim());
        let mut choices = vec!["follow".into()];
        if !self.ignore_model_efforts
            && let Some(efforts) = selected.and_then(|row| row["supportedEfforts"].as_array())
        {
            choices.extend(
                efforts
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|effort| {
                        EFFORTS[1..].contains(effort)
                            && (*effort != "max"
                                || self.subscription_kind.as_deref() != Some("codex"))
                    })
                    .map(str::to_owned),
            );
            return choices;
        }
        choices.extend(["low", "medium", "high", "xhigh"].map(str::to_owned));
        let label = format!(
            "{model} {}",
            selected
                .and_then(|row| row["displayName"].as_str())
                .unwrap_or_default()
        )
        .to_ascii_lowercase();
        let gpt_family = |label: &str| {
            label
                .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .any(|word| {
                    matches!(word, "gpt" | "codex")
                        || matches!(word.as_bytes(), [b'o', b'1'..=b'9'])
                })
        };
        let max = match self.subscription_kind.as_deref() {
            Some("codex") => false,
            Some(_) => {
                !gpt_family(&label)
                    && ["claude", "opus", "sonnet", "haiku"]
                        .iter()
                        .any(|part| label.contains(part))
            }
            None => !self.is_codex() && !gpt_family(&model.to_ascii_lowercase()),
        };
        if max {
            choices.push("max".into());
        }
        choices
    }

    pub(crate) fn effort_choices(&self, index: usize) -> Vec<String> {
        let mut choices = self.base_efforts(index);
        let current = if self.is_codex() {
            &self.reasoning_effort
        } else {
            &self.effort_mapping[index]
        };
        if !current.is_empty() && !choices.contains(current) {
            choices.push(current.clone());
        }
        choices
    }

    pub(crate) fn effort_supported(&self, index: usize, effort: &str) -> bool {
        effort.is_empty()
            || self
                .base_efforts(index)
                .iter()
                .any(|choice| choice == effort)
    }

    fn fetch_input(&self) -> Value {
        let mut input = json!({"cli": self.cli(), "endpoint": self.endpoint.trim()});
        if let Some(id) = &self.id {
            input["providerId"] = json!(id);
        }
        if !self.api_key.trim().is_empty() {
            input["apiKey"] = json!(self.api_key.trim());
        }
        input
    }

    fn input(&self) -> Value {
        let model = self.model.trim();
        let mut model_mapping = serde_json::Map::new();
        let mut effort_mapping = serde_json::Map::new();
        for (index, role) in ROLES.into_iter().enumerate() {
            model_mapping.insert(role.into(), json!(self.model_mapping[index].trim()));
            let effort = self.effort_mapping[index].as_str();
            effort_mapping.insert(
                role.into(),
                json!(if effort.is_empty() { "follow" } else { effort }),
            );
        }
        let mut input = json!({"cli": self.cli(), "name": self.name.trim()});
        if let Some(id) = &self.id {
            input["id"] = json!(id);
        }
        if !self.is_codex() {
            if let Some(mode) = &self.context_window_mode {
                input["contextWindowMode"] = json!(mode);
            }
            input["modelMapping"] = json!(model_mapping);
            input["effortMapping"] = json!(effort_mapping);
        }
        if self.subscription_kind.is_some() {
            input["kind"] = json!("subscription");
            return input;
        }
        input["endpoint"] = json!(self.endpoint.trim());
        input["model"] = json!(if model.is_empty() {
            "__cli_default__"
        } else {
            model
        });
        if self.is_codex() {
            input["reasoningEffort"] = json!(if self.reasoning_effort.is_empty() {
                "follow"
            } else {
                &self.reasoning_effort
            });
        }
        if !self.api_key.trim().is_empty() {
            input["apiKey"] = json!(self.api_key.trim());
        }
        if let Some(metadata) = &self.fetched_metadata {
            input["modelMetadata"] = metadata.clone();
        }
        input
    }
}

fn field(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

fn without_trailing_slash(value: &str) -> &str {
    value.strip_suffix('/').unwrap_or(value)
}

// Core parses and normalizes the complete URL before writing. The form checks
// its scheme/authority and forbids credentials, query strings and fragments.
fn endpoint_shape_valid(value: &str) -> bool {
    let Some((scheme, remainder)) = value.split_once(':') else {
        return false;
    };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || remainder.contains(['?', '#'])
    {
        return false;
    }
    let authority = remainder
        .trim_start_matches(['/', '\\'])
        .split(['/', '\\'])
        .next()
        .unwrap_or_default();
    !authority.is_empty() && !authority.contains('@') && !authority.chars().any(char::is_whitespace)
}

fn key_valid(key: &str) -> bool {
    key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        && !key.split_once('=').is_some_and(|(prefix, _)| {
            !prefix.is_empty()
                && prefix
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
        })
}

pub(crate) enum ProviderUpdate {
    None,
    Saved,
    SaveFailed(String),
    Models(Value),
    Deleted,
    DeleteFailed,
}

enum Operation {
    Load,
    Save {
        activate: bool,
        editing: bool,
    },
    Delete,
    Switch {
        id: String,
        committed: Option<Value>,
    },
}

struct Pending {
    request_id: u64,
    generation: u64,
    operation: Operation,
}

pub(crate) struct ProviderSettings {
    commands: RuntimeCommands,
    pending: Option<Pending>,
    pending_fetch: Option<(u64, u64)>,
    providers: Vec<Value>,
    active_id: Option<String>,
    load_error: Option<String>,
    loaded: bool,
    notice: Option<(&'static str, String, bool)>,
    retry_switch_id: Option<String>,
    committed_provider: Option<Value>,
}

impl ProviderSettings {
    pub(crate) fn new(commands: RuntimeCommands) -> Self {
        Self {
            commands,
            pending: None,
            pending_fetch: None,
            providers: Vec::new(),
            active_id: None,
            load_error: None,
            loaded: false,
            notice: None,
            retry_switch_id: None,
            committed_provider: None,
        }
    }

    pub(crate) fn reset_for_page(&mut self) {
        *self = Self::new(self.commands.clone());
    }

    pub(crate) fn load(&mut self) {
        self.load_error = None;
        if self.pending.is_none()
            && !self.send(Operation::Load, |request_id| Command::GetProviderSettings {
                request_id,
            })
        {
            self.load_error = Some(CONNECTING.into());
        }
    }

    pub(crate) fn loading(&self) -> bool {
        !self.loaded && self.is_reading()
    }

    fn is_reading(&self) -> bool {
        matches!(
            self.pending.as_ref().map(|p| &p.operation),
            Some(Operation::Load)
        )
    }

    pub(crate) fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub(crate) fn active_id(&self) -> Option<&str> {
        self.active_id.as_deref()
    }

    pub(crate) fn providers(&self) -> &[Value] {
        &self.providers
    }

    pub(crate) fn deletable_provider(&self, id: Option<&str>) -> Option<&Value> {
        let id = id?;
        if self.active_id() == Some(id) {
            return None;
        }
        self.providers
            .iter()
            .find(|row| row["id"] == id && row["builtIn"] != true)
    }

    pub(crate) fn switching(&self) -> Option<&str> {
        match &self.pending.as_ref()?.operation {
            Operation::Switch { id, .. } => Some(id),
            _ => None,
        }
    }

    pub(crate) fn saving(&self) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            matches!(
                &pending.operation,
                Operation::Save { .. }
                    | Operation::Switch {
                        committed: Some(_),
                        ..
                    }
            )
        })
    }

    pub(crate) fn deleting(&self) -> bool {
        matches!(
            self.pending.as_ref().map(|pending| &pending.operation),
            Some(Operation::Delete)
        )
    }

    pub(crate) fn notice(&self) -> Option<(&str, &str, bool)> {
        self.notice
            .as_ref()
            .map(|(title, text, error)| (*title, text.as_str(), *error))
    }

    pub(crate) fn dismiss_notice(&mut self) {
        self.notice = None;
        self.retry_switch_id = None;
    }

    pub(crate) fn retry_switch_id(&self) -> Option<&str> {
        self.retry_switch_id.as_deref()
    }

    pub(crate) fn take_committed_provider(&mut self) -> Option<Value> {
        self.committed_provider.take()
    }

    pub(crate) fn switch(&mut self, id: &str) {
        if self.pending.is_some() || self.active_id() == Some(id) {
            return;
        }
        if !self.providers.iter().any(|row| row["id"] == id) {
            return;
        }
        self.dismiss_notice();
        self.request_switch(id.to_owned(), None);
    }

    pub(crate) fn save(&mut self, draft: &ProviderDraft, activate: bool) -> Result<(), String> {
        if self.pending.is_some() {
            return Err(CONNECTING.into());
        }
        if let Some(error) = draft.validation_error() {
            return Err(error);
        }
        self.dismiss_notice();
        self.committed_provider = None;
        let operation = Operation::Save {
            activate,
            editing: draft.id.is_some(),
        };
        if self.send(operation, |request_id| Command::SaveProvider {
            request_id,
            input: draft.input(),
        }) {
            Ok(())
        } else {
            Err(CONNECTING.into())
        }
    }

    pub(crate) fn fetch_models(&mut self, draft: &ProviderDraft) -> Result<(), String> {
        if self.fetching() || draft.subscription_kind.is_some() {
            return Err(CONNECTING.into());
        }
        self.pending_fetch = self.dispatch(|request_id| Command::FetchProviderModels {
            request_id,
            input: draft.fetch_input(),
        });
        self.fetching()
            .then_some(())
            .ok_or_else(|| CONNECTING.into())
    }

    pub(crate) fn fetching(&self) -> bool {
        self.pending_fetch.is_some()
    }

    pub(crate) fn discard_fetch(&mut self) {
        self.pending_fetch = None;
    }

    pub(crate) fn delete(&mut self, id: &str) -> bool {
        if self.pending.is_some() || self.deletable_provider(Some(id)).is_none() {
            return false;
        }
        self.dismiss_notice();
        self.send(Operation::Delete, |request_id| Command::DeleteProvider {
            request_id,
            cli: CLI.to_owned(),
            id: id.to_owned(),
        })
    }

    fn request_switch(&mut self, id: String, committed: Option<Value>) -> ProviderUpdate {
        let cli = committed
            .as_ref()
            .and_then(|row| row["cli"].as_str())
            .unwrap_or(CLI);
        if self.send(
            Operation::Switch {
                id: id.clone(),
                committed: committed.clone(),
            },
            |request_id| Command::SetActiveProvider {
                request_id,
                cli: cli.to_owned(),
                id: id.clone(),
            },
        ) {
            ProviderUpdate::None
        } else {
            self.switch_failed(id, committed, CONNECTING.to_owned())
        }
    }

    fn send(&mut self, operation: Operation, command: impl FnOnce(u64) -> Command) -> bool {
        let Some((request_id, generation)) = self.dispatch(command) else {
            return false;
        };
        self.pending = Some(Pending {
            request_id,
            generation,
            operation,
        });
        true
    }

    fn dispatch(&self, command: impl FnOnce(u64) -> Command) -> Option<(u64, u64)> {
        let (generation, phase) = self.commands.connection();
        if phase != ConnectionPhase::Ready {
            return None;
        }
        let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        self.commands
            .send_at(generation, command(request_id))
            .ok()?;
        Some((request_id, generation))
    }

    pub(crate) fn receive(&mut self, reply: ProviderSettingsReply) -> ProviderUpdate {
        if self
            .pending_fetch
            .is_some_and(|(request_id, _)| request_id == reply.request_id)
        {
            self.discard_fetch();
            return ProviderUpdate::Models(match reply.result {
                ProviderSettingsResult::Models(result) => result,
                _ => json!({"error": "network"}),
            });
        }
        if self
            .pending
            .as_ref()
            .is_none_or(|pending| pending.request_id != reply.request_id)
        {
            return ProviderUpdate::None;
        }
        let Pending { operation, .. } = self.pending.take().unwrap();
        match (operation, reply.result) {
            (Operation::Load, ProviderSettingsResult::Snapshot(snapshot)) => {
                if !self.accept_snapshot(snapshot) {
                    self.load_error = Some(LOAD_FAILED.into());
                }
                ProviderUpdate::None
            }
            (Operation::Save { activate, editing }, ProviderSettingsResult::Provider(provider))
                if provider["id"].as_str().is_some() =>
            {
                self.project_save(&provider);
                let id = field(&provider, "id");
                if activate && self.active_id() != Some(&id) {
                    return self.request_switch(id, Some(provider));
                }
                self.notice = Some(if editing {
                    ("配置已保存", "供应商配置已保存".into(), false)
                } else {
                    ("配置已保存", "供应商已保存，尚未切换。".into(), false)
                });
                ProviderUpdate::Saved
            }
            (Operation::Delete, ProviderSettingsResult::Snapshot(snapshot)) => {
                if !self.accept_snapshot(snapshot) {
                    return ProviderUpdate::DeleteFailed;
                }
                self.notice = Some(("供应商已删除", String::new(), false));
                ProviderUpdate::Deleted
            }
            (Operation::Switch { id, committed }, ProviderSettingsResult::Snapshot(snapshot)) => {
                let cli = committed
                    .as_ref()
                    .and_then(|row| row["cli"].as_str())
                    .unwrap_or(CLI);
                let name = snapshot["providers"][cli]
                    .as_array()
                    .and_then(|rows| rows.iter().find(|row| row["id"] == id))
                    .map_or_else(String::new, |row| field(row, "name"));
                if !self.accept_snapshot(snapshot) {
                    return self.switch_failed(id, committed, SWITCH_FAILED.into());
                }
                let app = if cli == "codex" {
                    "Codex"
                } else {
                    "Claude Code"
                };
                self.notice = Some((
                    "默认供应商已切换",
                    format!("已切换为 {name}；{app} 新会话直接生效"),
                    false,
                ));
                if committed.is_some() {
                    ProviderUpdate::Saved
                } else {
                    ProviderUpdate::None
                }
            }
            (operation, ProviderSettingsResult::Error(message)) => {
                self.fail(operation, Some(message))
            }
            (operation, _) => self.fail(operation, None),
        }
    }

    pub(crate) fn connection_changed(&mut self) -> ProviderUpdate {
        let (generation, phase) = self.commands.connection();
        let fetch_lost = self.pending_fetch.is_some_and(|(_, requested)| {
            requested != generation || phase != ConnectionPhase::Ready
        });
        if fetch_lost {
            self.discard_fetch();
        }
        let update = if self.pending.as_ref().is_some_and(|pending| {
            generation != pending.generation || phase != ConnectionPhase::Ready
        }) {
            let pending = self.pending.take().unwrap();
            self.fail(pending.operation, Some(CONNECTING.into()))
        } else {
            ProviderUpdate::None
        };
        if self.pending.is_none()
            && phase == ConnectionPhase::Ready
            && self.load_error.as_deref() == Some(CONNECTING)
        {
            self.load();
        }
        if fetch_lost && matches!(update, ProviderUpdate::None) {
            ProviderUpdate::Models(json!({"error": "network"}))
        } else {
            update
        }
    }

    fn fail(&mut self, operation: Operation, message: Option<String>) -> ProviderUpdate {
        match operation {
            Operation::Load => {
                self.load_error = Some(message.unwrap_or_else(|| LOAD_FAILED.into()));
                ProviderUpdate::None
            }
            Operation::Save { .. } => {
                ProviderUpdate::SaveFailed(message.unwrap_or_else(|| SAVE_FAILED.into()))
            }
            Operation::Delete => ProviderUpdate::DeleteFailed,
            Operation::Switch { id, committed } => self.switch_failed(
                id,
                committed,
                message.unwrap_or_else(|| SWITCH_FAILED.into()),
            ),
        }
    }

    fn switch_failed(
        &mut self,
        id: String,
        committed: Option<Value>,
        message: String,
    ) -> ProviderUpdate {
        if committed.is_some() {
            self.committed_provider = committed;
            ProviderUpdate::SaveFailed(message)
        } else {
            self.notice = Some(("需要检查", message, true));
            self.retry_switch_id = Some(id);
            ProviderUpdate::None
        }
    }

    fn accept_snapshot(&mut self, mut snapshot: Value) -> bool {
        let Some(rows) = snapshot["providers"][CLI].as_array_mut() else {
            return false;
        };
        self.providers = std::mem::take(rows);
        self.active_id = snapshot["activeProviderIds"][CLI]
            .as_str()
            .map(str::to_owned);
        self.sort_providers();
        self.loaded = true;
        self.load_error = None;
        true
    }

    fn sort_providers(&mut self) {
        if let Some(index) = self
            .active_id
            .as_deref()
            .and_then(|id| self.providers.iter().position(|row| row["id"] == id))
        {
            self.providers[..=index].rotate_right(1);
        }
    }

    fn project_save(&mut self, provider: &Value) {
        if provider["cli"] == "codex" {
            return;
        }
        if let Some(row) = self
            .providers
            .iter_mut()
            .find(|row| row["id"] == provider["id"])
        {
            let previous_mode = row.get("contextWindowMode").cloned();
            *row = provider.clone();
            if row.get("contextWindowMode").is_none()
                && let Some(mode) = previous_mode
            {
                row["contextWindowMode"] = mode;
            }
        } else {
            self.providers.push(provider.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_draft() -> ProviderDraft {
        ProviderDraft {
            name: "Provider".into(),
            endpoint: "https://api.example.invalid/v1".into(),
            api_key: "test-key".into(),
            model: "test-model".into(),
            ..Default::default()
        }
    }

    #[test]
    fn save_uses_frozen_validation_order_and_blocks_credentials_in_url() {
        let mut draft = api_draft();
        draft.name.clear();
        draft.endpoint = "https://user:pass@api.example.invalid/v1".into();
        draft.api_key.clear();
        draft.model.clear();
        assert_eq!(
            draft.validation_error().as_deref(),
            Some("请选择或填写默认模型。")
        );
        draft.model = "test-model".into();
        assert_eq!(
            draft.validation_error().as_deref(),
            Some("请先填写 API Key。")
        );
        draft.api_key = "API_KEY=value".into();
        assert_eq!(
            draft.validation_error().as_deref(),
            Some("请仅粘贴密钥本身，不包含空格、换行或变量名。")
        );
        draft.api_key = "test-key".into();
        for endpoint in [
            "https://user:pass@api.example.invalid/v1",
            "https://api.example.invalid/v1?secret=credential",
            "https://api.example.invalid/v1#fragment",
            "ftp://api.example.invalid",
        ] {
            draft.endpoint = endpoint.into();
            assert_eq!(
                draft.validation_error().as_deref(),
                Some("请填写有效的 API 基础地址，例如 https://api.example.com/v1。")
            );
        }
    }

    #[test]
    fn saved_key_is_a_presence_flag_and_is_never_reused_at_another_endpoint() {
        let mut draft = ProviderDraft::from_provider(&json!({
            "id": "existing",
            "name": "配置",
            "endpoint": "https://api.example.invalid/v1",
            "model": "CLI 默认",
            "hasApiKey": true,
            "builtIn": true,
        }));
        assert!(draft.api_key.is_empty());
        assert!(draft.model.is_empty());
        draft.endpoint.push('/');
        assert!(draft.validation_error().is_none());
        let input = draft.input();
        assert_eq!(input["model"], "__cli_default__");
        assert!(!input.as_object().unwrap().contains_key("apiKey"));
        draft.endpoint = "https://other.example.invalid/v1".into();
        assert_eq!(
            draft.validation_error().as_deref(),
            Some("地址已更改，请重新填写此服务的密钥，避免把原密钥发送到其他服务。")
        );
        draft.api_key = "replacement-key".into();
        assert!(draft.validation_error().is_none());
    }

    #[test]
    fn effort_options_respect_saved_metadata_until_connection_fields_change() {
        let mut draft = api_draft();
        draft.api_key.clear();
        draft.has_api_key = true;
        draft.original_endpoint = draft.endpoint.clone();
        draft.model_mapping[0] = "third-party-model".into();
        draft.effort_mapping[0] = "max".into();
        draft.model_metadata = json!([
            {"id": "third-party-model", "supportedEfforts": ["medium", "high"]}
        ]);
        assert_eq!(
            draft.validation_error().as_deref(),
            Some("opus 的推理强度不受当前模型支持，请重新选择。")
        );
        draft.api_key = "replacement-key".into();
        assert!(draft.validation_error().is_none());
        draft.model_mapping[0] = "gpt-5-probe".into();
        assert!(!draft.effort_supported(0, "max"));
        assert!(draft.effort_supported(0, "follow"));
    }

    #[test]
    fn codex_subscription_filters_models_and_efforts_while_codex_api_uses_one_effort() {
        let subscription = ProviderDraft::from_provider(&json!({
            "id": "subscription", "cli": "claude-code", "kind": "subscription", "name": "Account",
            "modelMapping": {"opus": "work", "sonnet": "work", "haiku": "work"},
            "subscription": {"provider": "codex", "modelMetadata": [
                {"id": "work", "supportedEfforts": ["low", "high", "max"]},
                {"id": "IMAGE"}, {"id": "video-lite"}, {"id": "codex-auto-review"}
            ]}
        }));
        assert_eq!(subscription.available_models().len(), 1);
        assert_eq!(subscription.context_mode(), "512k");
        assert_eq!(subscription.effort_choices(0), ["follow", "low", "high"]);
        assert!(subscription.validation_error().is_none());
        assert_eq!(subscription.input()["kind"], "subscription");
        assert!(subscription.input().get("subscription").is_none());

        let mut codex = api_draft();
        codex.cli = "codex".into();
        codex.reasoning_effort = "high".into();
        codex.fetched_metadata = Some(json!([
            {"id": "test-model", "supportedEfforts": ["low", "high"]}
        ]));
        assert_eq!(codex.effort_choices(0), ["follow", "low", "high"]);
        let input = codex.input();
        assert_eq!(input["reasoningEffort"], "high");
        assert!(input.get("effortMapping").is_none() && input.get("contextWindowMode").is_none());
        assert_eq!(codex.fetch_input()["cli"], "codex");
        assert!(!codex.effort_supported(0, "max"));
    }
}
