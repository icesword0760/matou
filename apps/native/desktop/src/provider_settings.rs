//! Provider settings use the runtime's redacted Graph replies.
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use matou_native_core::{
    claude_role_budget::CLAUDE_ROLES as ROLES, provider_launch::EFFORTS, sessions::InterfaceLocale,
};
use matou_native_runtime::{
    protocol::{
        ClaudeModelPreferencesReply, ProviderSettingsReply, ProviderSettingsResult,
        SubscriptionLoginStatus, SubscriptionLoginView,
    },
    subscription_launch::SubscriptionProvider,
};
use serde_json::{Value, json};

use crate::runtime_bridge::{Command, ConnectionPhase, RuntimeCommands};

const CLI: &str = "claude-code";
const CONNECTING: &str = "供应商配置服务正在连接，请稍后重试";
const LOAD_FAILED: &str = "供应商配置载入失败";
const SAVE_FAILED: &str = "供应商配置保存失败";
pub(crate) const DELETE_FAILED: &str = "删除失败";
const SWITCH_FAILED: &str = "切换失败";
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

fn subscription_models_refreshed_title(locale: InterfaceLocale, count: usize) -> String {
    match locale {
        InterfaceLocale::Chinese => format!("模型列表已更新（{count} 个模型）"),
        InterfaceLocale::English => {
            let unit = if count == 1 { "model" } else { "models" };
            format!("Model list updated ({count} {unit})")
        }
    }
}

pub(crate) fn next_provider_settings_request() -> u64 {
    NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
}

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
    pub(crate) model_mapping: [String; ROLES.len()],
    pub(crate) effort_mapping: [String; ROLES.len()],
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
            self.model_mapping.iter().zip(ROLES).all(|(id, role)| {
                let id = id.trim();
                (role == "fable" && id.is_empty()) || models.iter().any(|model| model["id"] == id)
            })
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
                    && ROLES
                        .into_iter()
                        .chain(["claude"])
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
        (!self.is_codex() && ROLES[index] == "fable" && self.model_mapping[index].trim().is_empty())
            || effort.is_empty()
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
            let mapping = self.model_mapping[index].trim();
            if role == "fable" && mapping.is_empty() {
                continue;
            }
            model_mapping.insert(role.into(), json!(mapping));
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
    SubscriptionDraft(ProviderDraft),
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

#[derive(Default)]
pub(crate) struct SubscriptionLogin {
    pub provider: String,
    pub phase: &'static str,
    pub busy: bool,
    pub can_reopen: bool,
    pub system_browser_available: bool,
    pub error: Option<String>,
    attempt_id: Option<String>,
    details: Option<Value>,
    chinese: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubscriptionOperation {
    ConfirmRisk,
    Start,
    Status,
    Reopen,
    Commit { activate: bool },
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
    notice: Option<(String, String, bool)>,
    retry_switch_id: Option<String>,
    committed_provider: Option<Value>,
    revision: u64,
    subscription_round: Option<(u64, u64)>,
    subscription_open_request: Option<u64>,
    subscription_risk_accepted: bool,
    subscription_login: SubscriptionLogin,
    subscription_request: Option<(u64, SubscriptionOperation)>,
    subscription_accounts: HashMap<String, Value>,
    subscription_refreshes: HashMap<u64, (String, InterfaceLocale)>,
    compaction: Option<Value>,
    compaction_request: Option<(u64, u64, bool)>,
    compaction_error: Option<String>,
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
            revision: 0,
            subscription_round: None,
            subscription_open_request: None,
            subscription_risk_accepted: false,
            subscription_login: SubscriptionLogin::default(),
            subscription_request: None,
            subscription_accounts: HashMap::new(),
            subscription_refreshes: HashMap::new(),
            compaction: None,
            compaction_request: None,
            compaction_error: None,
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

    pub(crate) fn open_subscription_settings(&mut self) {
        if self.subscription_round.is_some() {
            return;
        }
        if let Some((id, generation)) =
            self.dispatch(|request_id| Command::OpenSubscriptionSettings {
                request_id,
                settings_id: request_id,
            })
        {
            self.subscription_round = Some((id, generation));
            self.subscription_open_request = Some(id);
        }
    }

    pub(crate) fn close_subscription_settings(&mut self) {
        if let Some((settings_id, generation)) = self.subscription_round.take() {
            let request_id = next_provider_settings_request();
            let _ = self.commands.send_at(
                generation,
                Command::CloseSubscriptionSettings {
                    request_id,
                    settings_id,
                },
            );
        }
        self.subscription_open_request = None;
        self.subscription_login = SubscriptionLogin::default();
        self.subscription_request = None;
        self.subscription_accounts.clear();
        self.subscription_refreshes.clear();
    }

    pub(crate) fn begin_subscription(&mut self, provider: &str) {
        self.subscription_login = SubscriptionLogin {
            provider: provider.into(),
            phase: if provider == "antigravity" && !self.subscription_risk_accepted {
                "risk"
            } else {
                "login"
            },
            ..SubscriptionLogin::default()
        };
    }

    pub(crate) fn subscription_login(&self) -> &SubscriptionLogin {
        &self.subscription_login
    }

    pub(crate) fn subscription_details(&self, id: &str) -> Option<&Value> {
        self.subscription_accounts.get(id)
    }

    pub(crate) fn confirm_subscription_risk(&mut self) {
        if self.subscription_login.phase == "risk" {
            self.send_subscription(SubscriptionOperation::ConfirmRisk, |request_id, _| {
                Command::ConfirmAntigravityRisk { request_id }
            });
        }
    }

    pub(crate) fn start_subscription_login_localized(&mut self, chinese: bool) {
        let provider = match self.subscription_login.provider.as_str() {
            "codex" => SubscriptionProvider::ChatGpt,
            "antigravity" if self.subscription_risk_accepted => SubscriptionProvider::Antigravity,
            _ => return,
        };
        if !matches!(self.subscription_login.phase, "login" | "error") {
            return;
        }
        self.subscription_login.chinese = chinese;
        self.subscription_login.attempt_id = None;
        self.subscription_login.can_reopen = false;
        self.subscription_login.system_browser_available = false;
        self.subscription_login.phase = "waiting";
        if !self.send_subscription(SubscriptionOperation::Start, |request_id, settings_id| {
            Command::StartSubscriptionLogin {
                request_id,
                settings_id,
                provider,
                chinese,
            }
        }) {
            self.subscription_login.phase = "error";
        }
    }

    pub(crate) fn poll_subscription_login(&mut self) -> ProviderUpdate {
        if self.subscription_login.phase == "waiting"
            && self.subscription_request.is_none()
            && let Some(attempt_id) = self.subscription_login.attempt_id.clone()
        {
            self.send_subscription(SubscriptionOperation::Status, |request_id, settings_id| {
                Command::GetSubscriptionLoginStatus {
                    request_id,
                    settings_id,
                    attempt_id,
                }
            });
        }
        ProviderUpdate::None
    }

    pub(crate) fn reopen_subscription_login(&mut self) {
        self.reopen_subscription(false);
    }

    pub(crate) fn reopen_subscription_in_system_browser(&mut self) {
        self.reopen_subscription(true);
    }

    fn reopen_subscription(&mut self, system_browser: bool) {
        if self.subscription_login.can_reopen
            && (!system_browser || self.subscription_login.system_browser_available)
            && let Some(attempt_id) = self.subscription_login.attempt_id.clone()
        {
            self.send_subscription(SubscriptionOperation::Reopen, |request_id, settings_id| {
                Command::ReopenSubscriptionLogin {
                    request_id,
                    settings_id,
                    attempt_id,
                    system_browser,
                }
            });
        }
    }

    pub(crate) fn cancel_subscription_login(&mut self) {
        let attempt_id = self.subscription_login.attempt_id.take();
        let starting = self
            .subscription_request
            .is_some_and(|(_, operation)| operation == SubscriptionOperation::Start);
        if (attempt_id.is_some() || starting)
            && let Some((settings_id, generation)) = self.subscription_round
        {
            let request_id = next_provider_settings_request();
            let _ = self.commands.send_at(
                generation,
                Command::CancelSubscriptionLogin {
                    request_id,
                    settings_id,
                    attempt_id,
                },
            );
        }
        self.subscription_request = None;
        self.subscription_login.phase = "error";
        self.subscription_login.busy = false;
        self.subscription_login.can_reopen = false;
        self.subscription_login.system_browser_available = false;
        self.subscription_login.error = Some("Sign-in was cancelled".into());
        self.subscription_login.details = None;
    }

    pub(crate) fn commit_subscription(&mut self, draft: &ProviderDraft, activate: bool) {
        let Some(attempt_id) = self.subscription_login.attempt_id.clone() else {
            return;
        };
        if self.subscription_login.phase != "models" {
            return;
        }
        if let Some(error) = draft.validation_error() {
            self.subscription_login.error = Some(error);
            return;
        }
        let mut input = draft.input();
        if input["modelMapping"]["fable"]
            .as_str()
            .is_none_or(str::is_empty)
        {
            input["modelMapping"]["fable"] = input["modelMapping"]["opus"].clone();
        }
        self.send_subscription(
            SubscriptionOperation::Commit { activate },
            |request_id, settings_id| Command::CommitSubscriptionDraft {
                request_id,
                settings_id,
                attempt_id,
                activate,
                input,
            },
        );
    }

    pub(crate) fn refresh_subscription_models(
        &mut self,
        provider_id: &str,
        locale: InterfaceLocale,
    ) {
        if self.refreshing_subscription(provider_id) {
            return;
        }
        self.dismiss_notice();
        if let Some(request_id) = self.dispatch_subscription(|request_id, settings_id| {
            Command::RefreshSubscriptionModels {
                request_id,
                settings_id,
                provider_id: provider_id.into(),
            }
        }) {
            self.subscription_refreshes
                .insert(request_id, (provider_id.into(), locale));
        } else {
            self.notice = Some(("模型刷新失败".into(), CONNECTING.into(), true));
        }
    }

    pub(crate) fn refreshing_subscription(&self, id: &str) -> bool {
        self.subscription_refreshes
            .values()
            .any(|(provider_id, _)| provider_id == id)
    }

    fn send_subscription(
        &mut self,
        operation: SubscriptionOperation,
        command: impl FnOnce(u64, u64) -> Command,
    ) -> bool {
        if self.subscription_request.is_some() {
            return false;
        }
        if let Some(request_id) = self.dispatch_subscription(command) {
            self.subscription_request = Some((request_id, operation));
            self.subscription_login.busy = operation != SubscriptionOperation::Status;
            if self.subscription_login.busy {
                self.subscription_login.error = None;
            }
            true
        } else {
            self.subscription_login.error = Some(CONNECTING.into());
            false
        }
    }

    fn dispatch_subscription(&self, command: impl FnOnce(u64, u64) -> Command) -> Option<u64> {
        let (settings_id, generation) = self.subscription_round?;
        let request_id = next_provider_settings_request();
        self.commands
            .send_at(generation, command(request_id, settings_id))
            .ok()?;
        Some(request_id)
    }

    fn apply_login_view(&mut self, view: SubscriptionLoginView) -> ProviderUpdate {
        let login = &mut self.subscription_login;
        login.attempt_id = Some(view.attempt_id);
        login.can_reopen = view.can_reopen;
        login.system_browser_available = view.system_browser_available;
        login.error = view
            .reason
            .map(|reason| subscription_login_message(reason, login.chinese));
        login.phase = match view.status {
            SubscriptionLoginStatus::Waiting | SubscriptionLoginStatus::Exchanging => "waiting",
            SubscriptionLoginStatus::Ready => "models",
            _ => "error",
        };
        if let Some(mut draft) = view.draft {
            if let Some(saved) = self.providers.iter().find(|row| row["id"] == draft["id"])
                && let Some(mode) = saved.get("contextWindowMode")
            {
                draft["contextWindowMode"] = mode.clone();
            }
            login.details = Some(
                json!({"account": draft["subscription"]["account"], "models": draft["subscription"]["modelMetadata"], "quota": draft["subscription"]["quota"]}),
            );
            return ProviderUpdate::SubscriptionDraft(ProviderDraft::from_provider(&draft));
        }
        ProviderUpdate::None
    }

    fn receive_subscription(
        &mut self,
        operation: SubscriptionOperation,
        result: ProviderSettingsResult,
    ) -> ProviderUpdate {
        self.subscription_login.busy = false;
        match (operation, result) {
            (SubscriptionOperation::ConfirmRisk, ProviderSettingsResult::Accepted(true)) => {
                self.subscription_risk_accepted = true;
                self.subscription_login.phase = "login";
                ProviderUpdate::None
            }
            (
                SubscriptionOperation::Commit { activate },
                ProviderSettingsResult::Provider(provider),
            ) if provider["id"].is_string() => {
                let id = field(&provider, "id");
                if let Some(details) = self.subscription_login.details.take() {
                    self.subscription_accounts.insert(id.clone(), details);
                }
                self.project_save(&provider);
                if activate {
                    self.active_id = Some(id);
                    self.sort_providers();
                }
                self.subscription_login = SubscriptionLogin::default();
                self.notice = Some(("配置已保存".into(), "供应商配置已保存".into(), false));
                ProviderUpdate::Saved
            }
            (_, ProviderSettingsResult::SubscriptionLogin(view)) => self.apply_login_view(view),
            (operation, result) => {
                let error = match result {
                    ProviderSettingsResult::Error(error) => error,
                    _ => CONNECTING.into(),
                };
                let error = subscription_login_message(error, self.subscription_login.chinese);
                self.subscription_login.error = Some(error.clone());
                match operation {
                    SubscriptionOperation::Commit { .. } => ProviderUpdate::SaveFailed(error),
                    SubscriptionOperation::ConfirmRisk | SubscriptionOperation::Reopen => {
                        ProviderUpdate::None
                    }
                    _ => {
                        self.subscription_login.phase = "error";
                        ProviderUpdate::None
                    }
                }
            }
        }
    }

    pub(crate) fn load_compaction(&mut self) {
        if self.compaction_request.is_none() && self.compaction.is_none() {
            self.compaction_request = self
                .dispatch(|request_id| Command::GetClaudeModelPreferences { request_id })
                .map(|(request_id, generation)| (request_id, generation, false));
            self.compaction_error = self.compaction_request.is_none().then(|| CONNECTING.into());
        }
    }

    pub(crate) fn compaction(&self) -> Option<&Value> {
        self.compaction.as_ref()
    }

    pub(crate) fn compaction_error(&self) -> Option<&str> {
        self.compaction_error.as_deref()
    }

    pub(crate) fn saving_compaction(&self) -> bool {
        self.compaction_request.is_some_and(|(_, _, saving)| saving)
    }

    pub(crate) fn save_compaction(&mut self, input: &Value) {
        if self.compaction.is_none() || self.compaction_request.is_some() {
            return;
        }
        self.compaction_request = self
            .dispatch(|request_id| Command::SetClaudeModelPreferences {
                request_id,
                assignments: Value::Null,
                web_search: None,
                compaction: Some(input.clone()),
            })
            .map(|(request_id, generation)| (request_id, generation, true));
        self.compaction_error = self.compaction_request.is_none().then(|| CONNECTING.into());
    }

    pub(crate) fn receive_compaction(&mut self, reply: &ClaudeModelPreferencesReply) -> bool {
        if !self
            .compaction_request
            .is_some_and(|(request_id, _, _)| request_id == reply.roles.request_id)
        {
            return false;
        }
        self.compaction_request = None;
        if let Some(compaction) = &reply.compaction {
            self.compaction = Some(compaction.clone());
            self.compaction_error = None;
        } else {
            self.compaction_error = Some(
                reply
                    .roles
                    .error
                    .clone()
                    .unwrap_or_else(|| LOAD_FAILED.into()),
            );
        }
        true
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
            .map(|(title, text, error)| (title.as_str(), text.as_str(), *error))
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
        let request_id = next_provider_settings_request();
        self.commands
            .send_at(generation, command(request_id))
            .ok()?;
        Some((request_id, generation))
    }

    pub(crate) fn receive(&mut self, reply: ProviderSettingsReply) -> ProviderUpdate {
        if self.subscription_open_request == Some(reply.request_id) {
            self.subscription_open_request = None;
            if let ProviderSettingsResult::SubscriptionSettings(view) = reply.result {
                self.subscription_risk_accepted |= view.antigravity_risk_accepted;
                if self.accept_subscription_snapshot(view.snapshot) {
                    for (id, details) in view.accounts {
                        self.accept_subscription_details(id, details);
                    }
                }
            }
            return ProviderUpdate::None;
        }
        if let Some((id, operation)) = self.subscription_request
            && id == reply.request_id
        {
            self.subscription_request = None;
            return self.receive_subscription(operation, reply.result);
        }
        if let Some((id, locale)) = self.subscription_refreshes.remove(&reply.request_id) {
            match reply.result {
                ProviderSettingsResult::SubscriptionModels(view) if view.provider_id == id => {
                    let title =
                        subscription_models_refreshed_title(locale, view.details.models.len());
                    if self.accept_subscription_snapshot(view.snapshot) {
                        self.accept_subscription_details(id, view.details);
                    }
                    self.notice = Some((title, String::new(), false));
                }
                result => {
                    let error = match result {
                        ProviderSettingsResult::Error(error) => error,
                        _ => "Model list could not be read".into(),
                    };
                    if error == "Subscription sign-in is required" {
                        if let Some(provider) =
                            self.providers.iter_mut().find(|row| row["id"] == id)
                        {
                            provider["subscription"]["account"]["status"] = json!("needs-sign-in");
                        }
                        self.subscription_accounts.remove(&id);
                    }
                    self.notice = Some(("模型刷新失败".into(), error, true));
                }
            }
            return ProviderUpdate::None;
        }
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
                    ("配置已保存".into(), "供应商配置已保存".into(), false)
                } else {
                    (
                        "配置已保存".into(),
                        "供应商已保存，尚未切换。".into(),
                        false,
                    )
                });
                ProviderUpdate::Saved
            }
            (Operation::Delete, ProviderSettingsResult::Snapshot(snapshot)) => {
                if !self.accept_snapshot(snapshot) {
                    return ProviderUpdate::DeleteFailed;
                }
                self.notice = Some(("供应商已删除".into(), String::new(), false));
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
                    "默认供应商已切换".into(),
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
        if self.subscription_round.is_some_and(|(_, requested)| {
            requested != generation || phase != ConnectionPhase::Ready
        }) {
            self.close_subscription_settings();
            self.subscription_risk_accepted = false;
        }
        if self.compaction_request.is_some_and(|(_, requested, _)| {
            requested != generation || phase != ConnectionPhase::Ready
        }) {
            self.compaction_request = None;
            self.compaction_error = Some(CONNECTING.into());
        }
        if phase != ConnectionPhase::Ready {
            self.compaction = None;
        }
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
        if (message.contains("Subscription sign-in is required")
            || message.contains("Subscription credential is missing or no longer valid"))
            && let Some(provider) = self
                .providers
                .iter_mut()
                .find(|row| row["id"] == id && row["kind"] == "subscription")
        {
            provider["subscription"]["account"]["status"] = json!("needs-sign-in");
            self.subscription_accounts.remove(&id);
        }
        if committed.is_some() {
            self.committed_provider = committed;
            ProviderUpdate::SaveFailed(message)
        } else {
            self.notice = Some(("需要检查".into(), message, true));
            self.retry_switch_id = Some(id);
            ProviderUpdate::None
        }
    }

    fn accept_subscription_details(
        &mut self,
        id: String,
        details: matou_native_runtime::protocol::SubscriptionDetails,
    ) {
        if let Some(provider) = self.providers.iter_mut().find(|row| row["id"] == id)
            && project_account_details(provider, &details.account)
            && let Ok(details) = serde_json::to_value(details)
        {
            self.subscription_accounts.insert(id, details);
        } else {
            self.subscription_accounts.remove(&id);
        }
    }

    fn accept_subscription_snapshot(&mut self, snapshot: Value) -> bool {
        if snapshot["revision"]
            .as_u64()
            .is_some_and(|revision| revision < self.revision)
        {
            return false;
        }
        self.accept_snapshot(snapshot)
    }

    fn accept_snapshot(&mut self, mut snapshot: Value) -> bool {
        let revision = snapshot["revision"].as_u64().unwrap_or(self.revision);
        let Some(rows) = snapshot["providers"][CLI].as_array_mut() else {
            return false;
        };
        self.revision = revision;
        self.providers = std::mem::take(rows);
        self.subscription_accounts.retain(|id, details| {
            self.providers
                .iter_mut()
                .find(|row| row["id"] == id.as_str())
                .is_some_and(|row| project_account_details(row, &details["account"]))
        });
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
        self.revision = self.revision.saturating_add(1);
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

fn subscription_login_message(reason: String, chinese: bool) -> String {
    if !chinese {
        return reason;
    }
    match reason.as_str() {
        "Sign-in port is in use. Close the other sign-in window and try again." => {
            "登录端口被占用。请关闭其他登录窗口后重试。"
        }
        "Could not listen for the browser sign-in callback" => {
            "无法接收浏览器登录回调。请关闭其他登录页面后重试。"
        }
        "Sign-in is already in progress in another window. Finish it or cancel it there." => {
            "另一个窗口正在登录。请在该窗口完成或取消登录。"
        }
        "Could not open the sign-in browser. Try again or use the system browser." => {
            "浏览器没有打开。请重试，或选择使用系统浏览器。"
        }
        "Could not open the system browser. Check your browser and try again." => {
            "系统浏览器没有打开。请检查默认浏览器设置后重试。"
        }
        "Sign-in timed out. Start again." => "登录超时，请重新登录。",
        "Could not read sign-in status. Try again." => "无法读取登录状态，请重试。",
        "Sign-in was not completed. Try again." => "登录未完成，请重试。",
        "Could not read the signed-in account or its models. Try again." => {
            "无法读取已登录账号或模型，请重试。"
        }
        "Subscription authorization address was rejected" => "无法打开授权地址，请重新开始登录。",
        "Subscription service is unavailable" => "订阅服务暂不可用，请稍后重试。",
        _ => return reason,
    }
    .into()
}

fn project_account_details(provider: &mut Value, details: &Value) -> bool {
    if provider["kind"] != "subscription" || !details.is_object() {
        return false;
    }
    let account = &mut provider["subscription"]["account"];
    if account["status"] == "needs-sign-in" {
        return false;
    }
    if let (Some(saved), Some(read)) = (account["email"].as_str(), details["email"].as_str())
        && saved != read
    {
        return false;
    }
    for key in ["status", "email", "plan"] {
        if let Some(value) = details.get(key) {
            account[key] = value.clone();
        }
    }
    true
}

impl Drop for ProviderSettings {
    fn drop(&mut self) {
        self.close_subscription_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_refresh_title_matches_the_count_and_locale() {
        assert_eq!(
            subscription_models_refreshed_title(InterfaceLocale::Chinese, 7),
            "模型列表已更新（7 个模型）"
        );
        for (count, expected) in [
            (0, "Model list updated (0 models)"),
            (1, "Model list updated (1 model)"),
            (7, "Model list updated (7 models)"),
        ] {
            assert_eq!(
                subscription_models_refreshed_title(InterfaceLocale::English, count),
                expected
            );
        }
    }

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
    fn fable_draft_saves_valid_effort_and_clears_the_optional_mapping() {
        let mut draft = ProviderDraft::from_provider(&json!({
            "id": "subscription", "kind": "subscription", "name": "Account",
            "modelMapping": {"opus": "work", "sonnet": "work", "haiku": "work"},
            "subscription": {"provider": "codex", "modelMetadata": [
                {"id": "work"}, {"id": "fable-special", "supportedEfforts": ["high"]}
            ]}
        }));
        assert!(draft.validation_error().is_none());
        draft.model_mapping[1] = "unknown".into();
        assert!(draft.validation_error().is_some());
        draft.model_mapping[1] = "fable-special".into();
        draft.effort_mapping[1] = "low".into();
        assert!(draft.validation_error().is_some());
        draft.effort_mapping[1] = "high".into();
        assert!(draft.validation_error().is_none());
        assert_eq!(draft.input()["modelMapping"]["fable"], "fable-special");
        assert_eq!(draft.input()["effortMapping"]["fable"], "high");
        draft.model_mapping[1].clear();
        assert!(draft.input()["modelMapping"].get("fable").is_none());
        assert!(draft.input()["effortMapping"].get("fable").is_none());
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
