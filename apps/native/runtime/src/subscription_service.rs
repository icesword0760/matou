use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use matou_native_core::{
    claude_role_budget::CLAUDE_ROLES,
    provider_config::{ProviderConfigStore, StoredSubscription, SubscriptionAccountIdentity},
    provider_launch::{
        ClaudeLaunch, PreparedSubscriptionLaunch, PreparedSubscriptionTarget, role_model_env_key,
    },
    sessions::ClaudeRoleLaunchPreferences,
};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::{
    model_router::{
        self, ManagementClient, ModelRouterManager, READY_TIMEOUT, RouterConnection, RouterLease,
    },
    subscription_launch::{
        self, CatalogModel, LaunchInput, ModelRole, RoleModels, SubscriptionProvider,
    },
};

const POLL: Duration = Duration::from_millis(50);

pub(crate) struct SubscriptionService {
    root: PathBuf,
    pub store: Arc<ProviderConfigStore>,
    manager: ModelRouterManager,
    // Alias and credential edits affect a shared engine, so prepare against one
    // stored account at a time. The PTY owner never takes this lock.
    serial: Mutex<()>,
    bindings: Mutex<HashMap<String, Vec<Weak<TargetBinding>>>>,
}

pub(crate) struct TargetBinding {
    target: PreparedSubscriptionTarget,
    identity: SubscriptionAccountIdentity,
    auth_index: String,
    validated_generation: Mutex<u64>,
    revoked: watch::Sender<bool>,
    lease: Mutex<Option<RouterLease>>,
}

impl TargetBinding {
    pub fn is_revoked(&self) -> bool {
        *self.revoked.borrow()
    }

    pub async fn cancelled(&self) {
        let mut signal = self.revoked.subscribe();
        let _ = signal.wait_for(|value| *value).await;
    }

    fn revoke(&self) {
        self.revoked.send_replace(true);
        self.lease.lock().unwrap().take();
    }
}

pub(crate) struct SessionSubscriptions {
    pub service: Arc<SubscriptionService>,
    pub targets: HashMap<PreparedSubscriptionTarget, Arc<TargetBinding>>,
}

pub(crate) struct PreparedClaudeLaunch {
    pub launch: ClaudeLaunch,
    pub subscriptions: Option<SessionSubscriptions>,
}

impl From<ClaudeLaunch> for PreparedClaudeLaunch {
    fn from(launch: ClaudeLaunch) -> Self {
        Self {
            launch,
            subscriptions: None,
        }
    }
}

impl SubscriptionService {
    pub fn new(
        root: PathBuf,
        store: Arc<ProviderConfigStore>,
        environment: BTreeMap<OsString, OsString>,
    ) -> Result<Self> {
        Ok(Self {
            manager: ModelRouterManager::new(root.clone(), environment)?,
            root,
            store,
            serial: Mutex::new(()),
            bindings: Mutex::default(),
        })
    }

    pub fn codex_egress_proxy(&self) -> Result<Option<Option<String>>> {
        self.store
            .has_subscription("codex")?
            .then(|| self.manager.egress_proxy())
            .transpose()
    }

    pub fn shutdown(&self) {
        self.manager.shutdown();
    }

    pub fn activate(&self, id: &str) -> Result<Value> {
        let _serial = self.serial.lock().unwrap();
        let Some(row) = self.store.subscription(id)? else {
            return self.store.set_active("claude-code", id);
        };
        let target = PreparedSubscriptionTarget {
            provider_id: id.to_owned(),
            provider: provider(&row)?.router_provider(),
            model: None,
        };
        let request = self.prepare_account(&target, row)?;
        self.prepare_launch(&request, ModelRole::Opus, None, &[])?;
        self.store
            .set_active_checked("claude-code", id, Some(&request.row.identity))
    }

    pub fn delete(&self, cli: &str, id: &str) -> Result<Value> {
        let _serial = self.serial.lock().unwrap();
        let account = if cli == "claude-code" {
            self.store.subscription(id)?
        } else {
            None
        };
        let result = self.store.delete(cli, id)?;
        if let Some(account) = account {
            self.revoke_provider_targets(id);
            if let Some(connection) = self.manager.current_connection() {
                let _ = ManagementClient::new(&connection)
                    .set_disabled(&account.identity.credential_name, true);
            }
        }
        Ok(result)
    }

    /// Called within the account mutation boundary before another identity binds.
    pub fn revoke_provider_targets(&self, id: &str) {
        if let Some(targets) = self.bindings.lock().unwrap().remove(id) {
            for binding in targets.into_iter().filter_map(|target| target.upgrade()) {
                binding.revoke();
            }
        }
    }

    pub fn prepare_session(
        self: &Arc<Self>,
        mut launch: ClaudeLaunch,
        preferences: &ClaudeRoleLaunchPreferences,
    ) -> Result<PreparedClaudeLaunch> {
        let mut targets = HashMap::new();
        let required: Vec<_> = launch
            .gateway
            .iter()
            .flat_map(|gateway| gateway.connections())
            .filter_map(|connection| connection.subscription.clone())
            .collect();
        let _serial = self.serial.lock().unwrap();
        for target in required {
            if targets.contains_key(&target) {
                continue;
            }
            let row = self
                .store
                .subscription(&target.provider_id)?
                .context("Subscription provider is unavailable")?;
            let account = self.prepare_account(&target, row)?;
            if let Some(model) = target.model.as_deref() {
                self.prepare_exact_model(&account, model)?;
            } else {
                let role = match launch.subscription_role {
                    Some("fable") => ModelRole::Fable,
                    Some("sonnet") => ModelRole::Sonnet,
                    Some("haiku") => ModelRole::Haiku,
                    _ => ModelRole::Opus,
                };
                let names = launch
                    .launch
                    .env
                    .keys()
                    .filter_map(|key| key.to_str())
                    .collect::<Vec<_>>();
                let window = launch_window(
                    &account,
                    preferences.context_preference(&target.provider_id),
                );
                let projection = self.prepare_launch(&account, role, window, &names)?;
                launch.apply_subscription_launch(projection);
            }
            let binding = self.bind(target.clone(), account)?;
            targets.insert(target, binding);
        }
        Ok(PreparedClaudeLaunch {
            launch,
            subscriptions: Some(SessionSubscriptions {
                service: Arc::clone(self),
                targets,
            }),
        })
    }

    pub fn prepare_model_target(
        &self,
        target: &PreparedSubscriptionTarget,
    ) -> Result<Arc<TargetBinding>> {
        let _serial = self.serial.lock().unwrap();
        let row = self
            .store
            .subscription(&target.provider_id)?
            .context("Subscription provider is unavailable")?;
        let account = self.prepare_account(target, row)?;
        self.prepare_exact_model(
            &account,
            target
                .model
                .as_deref()
                .context("Missing subscription model")?,
        )?;
        self.bind(target.clone(), account)
    }

    fn prepare_account(
        &self,
        target: &PreparedSubscriptionTarget,
        row: StoredSubscription,
    ) -> Result<PreparedAccount> {
        let major = model_router::macos_version()
            .and_then(|version| version.split('.').next()?.parse().ok());
        ensure!(
            subscription_launch::subscription_supported(cfg!(target_os = "macos"), major),
            "Subscriptions require macOS 15 or later"
        );
        ensure!(
            row.account["status"] != "needs-sign-in",
            "Subscription sign-in is required"
        );
        let provider = provider(&row)?;
        ensure!(
            provider.router_provider() == target.provider,
            "Subscription provider changed during preparation"
        );
        if model_router::credential_path(&self.root, &row.identity.credential_name).is_err() {
            self.store
                .mark_subscription_needs_sign_in(&target.provider_id, &row.identity)?;
            return Err(anyhow!(
                "Subscription credential is missing or no longer valid"
            ));
        }
        let (connection, lease) = self.manager.acquire()?;
        let client = ManagementClient::new(&connection);
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut expected_index = row.identity.auth_index.clone();
        let auth_index = loop {
            let files = client.credentials()?;
            if let Some(file) = credential_entry(&files, provider, &row.identity) {
                if !credential_identity_matches(file, &row.identity, expected_index.as_deref())
                    || needs_sign_in(file)
                {
                    self.store
                        .mark_subscription_needs_sign_in(&target.provider_id, &row.identity)?;
                    return Err(anyhow!(
                        "Subscription credential is missing or no longer valid"
                    ));
                }
                if let Some(index) = file["auth_index"]
                    .as_str()
                    .filter(|index| !index.is_empty())
                {
                    if credential_active(file) {
                        disable_other_credentials(&client, &files, provider, &row.identity)?;
                        break index.to_owned();
                    }
                    expected_index = Some(index.to_owned());
                }
            } else if Instant::now() >= deadline {
                self.store
                    .mark_subscription_needs_sign_in(&target.provider_id, &row.identity)?;
                return Err(anyhow!(
                    "Subscription credential is missing or no longer valid"
                ));
            }
            ensure!(
                Instant::now() < deadline,
                "Subscription credential is not active"
            );
            thread::sleep(POLL);
        };
        Ok(PreparedAccount {
            row,
            provider,
            connection,
            lease,
            auth_index,
        })
    }

    fn prepare_launch(
        &self,
        account: &PreparedAccount,
        role: ModelRole,
        window: Option<u64>,
        env: &[&str],
    ) -> Result<PreparedSubscriptionLaunch> {
        let models = account
            .row
            .model_metadata
            .iter()
            .filter_map(catalog_model)
            .collect::<Vec<_>>();
        let get = |role: &str| {
            let id = account.row.model_mapping[role].as_str()?;
            models.iter().find(|model| model.id == id)
        };
        let project = |window| {
            subscription_launch::build_launch(LaunchInput {
                provider: account.provider,
                provider_name: &account.row.name,
                router_url: &account.connection.base_url(),
                client_key: &account.connection.client_key,
                models: RoleModels {
                    opus: get("opus"),
                    fable: get("fable"),
                    sonnet: get("sonnet"),
                    haiku: get("haiku"),
                    ..Default::default()
                },
                main_role: role,
                context_window: window,
                inherited_env_names: env,
            })
            .map_err(|error| {
                anyhow!("Subscription launch configuration is unavailable ({error:?})")
            })
        };
        let mut plan = project(None)?;
        if let Some(window) = window {
            plan = project(Some(window))?;
            plan.env
                .insert("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE".into(), "90".into());
            plan.env.insert(
                "CLAUDE_CODE_DISABLE_1M_CONTEXT".into(),
                u8::from(window < 1_000_000).to_string(),
            );
        }
        let client = ManagementClient::new(&account.connection);
        let name = &account.row.identity.credential_name;
        let registered_aliases = client.aliases(account.provider)?;
        let registered = client.models(name)?;
        let registered_ids = model_ids(&registered);
        let management =
            plan.management_plan(true, false, false, &registered_aliases, &registered_ids);
        if management.write_aliases {
            client.set_aliases(account.provider, &plan.aliases)?;
        }
        if management.write_prefix {
            let has_upstream = |upstream: &str| {
                registered_ids.iter().any(|id| {
                    *id == upstream
                        || id
                            .strip_prefix(account.provider.prefix())
                            .and_then(|tail| tail.strip_prefix('/'))
                            == Some(upstream)
                })
            };
            if plan.aliases.iter().any(|alias| !has_upstream(&alias.name)) {
                client.set_extra_models(
                    name,
                    account.provider.prefix(),
                    extra_models(&account.row),
                )?;
            } else {
                client.set_prefix(name, account.provider.prefix())?;
            }
        }
        if management.wait_for_aliases {
            let ids = plan
                .aliases
                .iter()
                .map(|alias| format!("{}/{}", account.provider.prefix(), alias.alias))
                .collect::<Vec<_>>();
            wait_for_models(&client, name, |registered| {
                ids.iter().all(|id| registered.contains(&id.as_str()))
            })?;
        }
        self.manager
            .remember_aliases(plan.alias_provider(), plan.aliases);
        let models = CLAUDE_ROLES.map(|role| plan.env[&role_model_env_key(role)].clone());
        let mut environment = plan.env;
        environment.remove("ANTHROPIC_BASE_URL");
        environment.remove("ANTHROPIC_AUTH_TOKEN");
        let mut settings = plan.settings;
        settings["env"]
            .as_object_mut()
            .expect("subscription settings environment")
            .extend(
                environment
                    .iter()
                    .map(|(name, value)| (name.clone(), json!(value))),
            );
        Ok(PreparedSubscriptionLaunch {
            models,
            environment,
            clear_environment: plan.clear_env,
            settings,
            main_model: plan.main_model,
        })
    }

    fn prepare_exact_model(&self, account: &PreparedAccount, model: &str) -> Result<()> {
        ensure!(
            !model.is_empty()
                && model.encode_utf16().count() <= 240
                && !model.chars().any(|c| c.is_control() || c.is_whitespace()),
            "Invalid subscription model"
        );
        ensure!(
            account
                .row
                .model_metadata
                .iter()
                .any(|item| item["id"] == model),
            "Subscription model is missing from the saved catalog"
        );
        let client = ManagementClient::new(&account.connection);
        let name = &account.row.identity.credential_name;
        let prefix = account.provider.prefix();
        let routed = format!("{prefix}/{model}");
        let registered = client.models(name)?;
        let ids = model_ids(&registered);
        if ids.contains(&routed.as_str()) {
            return Ok(());
        }
        if ids.contains(&model) {
            client.set_prefix(name, prefix)?;
        } else {
            client.set_extra_models(name, prefix, extra_models(&account.row))?;
        }
        wait_for_models(&client, name, |registered| {
            registered.contains(&routed.as_str())
        })
    }

    fn bind(
        &self,
        target: PreparedSubscriptionTarget,
        account: PreparedAccount,
    ) -> Result<Arc<TargetBinding>> {
        ensure!(
            self.store
                .subscription(&target.provider_id)?
                .is_some_and(|row| row.identity == account.row.identity),
            "Subscription account changed during preparation"
        );
        let id = target.provider_id.clone();
        let binding = Arc::new(TargetBinding {
            target,
            identity: account.row.identity,
            auth_index: account.auth_index,
            validated_generation: Mutex::new(account.connection.generation),
            revoked: watch::channel(false).0,
            lease: Mutex::new(Some(account.lease)),
        });
        let mut bindings = self.bindings.lock().unwrap();
        bindings.retain(|_, entries| {
            entries.retain(|entry| entry.strong_count() > 0);
            !entries.is_empty()
        });
        bindings
            .entry(id)
            .or_default()
            .push(Arc::downgrade(&binding));
        Ok(binding)
    }

    pub fn assert_identity(&self, binding: &TargetBinding) -> Result<()> {
        if binding.is_revoked()
            || !self
                .store
                .subscription(&binding.target.provider_id)?
                .is_some_and(|row| {
                    row.identity == binding.identity && row.provider == binding.target.provider
                })
        {
            binding.revoke();
            return Err(anyhow!(
                "Subscription account changed. Restart this Claude session."
            ));
        }
        Ok(())
    }

    pub fn connection(
        &self,
        binding: &TargetBinding,
        failed_generation: Option<u64>,
    ) -> Result<RouterConnection> {
        self.assert_identity(binding)?;
        let (connection, _lease) = match failed_generation {
            Some(generation) => self.manager.recover(generation)?,
            None => self.manager.acquire()?,
        };
        let mut validated = binding.validated_generation.lock().unwrap();
        if *validated == connection.generation {
            return Ok(connection);
        }
        let client = ManagementClient::new(&connection);
        let provider = match binding.target.provider {
            "codex" => SubscriptionProvider::ChatGpt,
            _ => SubscriptionProvider::Antigravity,
        };
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            self.assert_identity(binding)?;
            let files = client.credentials()?;
            if let Some(file) = credential_entry(&files, provider, &binding.identity) {
                if !credential_identity_matches(file, &binding.identity, Some(&binding.auth_index))
                    || needs_sign_in(file)
                {
                    return Err(anyhow!("Subscription credential did not recover"));
                }
                if credential_active(file) {
                    disable_other_credentials(&client, &files, provider, &binding.identity)?;
                    *validated = connection.generation;
                    return Ok(connection);
                }
            }
            ensure!(
                Instant::now() < deadline,
                "Subscription credential did not recover"
            );
            thread::sleep(POLL);
        }
    }

    pub fn mark_needs_sign_in(&self, binding: &TargetBinding) {
        if self.assert_identity(binding).is_ok() {
            let _ = self
                .store
                .mark_subscription_needs_sign_in(&binding.target.provider_id, &binding.identity);
        }
    }
}

struct PreparedAccount {
    row: StoredSubscription,
    provider: SubscriptionProvider,
    connection: RouterConnection,
    lease: RouterLease,
    auth_index: String,
}

fn launch_window(account: &PreparedAccount, preference: Option<&Value>) -> Option<u64> {
    if let Some(window) = preference
        .and_then(Value::as_u64)
        .or(account.row.context_window)
    {
        return Some(window);
    }
    let mode = preference.and_then(Value::as_str).unwrap_or_else(|| {
        if account.provider == SubscriptionProvider::ChatGpt {
            "512k"
        } else {
            "default"
        }
    });
    match mode {
        "512k" => Some(512_000),
        "maximum" => {
            let mapping = &account.row.model_mapping;
            let mut limit = 1_000_000;
            for role in CLAUDE_ROLES {
                let mapped = mapping[role].as_str().filter(|id| !id.is_empty());
                let id = if role == "fable" {
                    mapped.or_else(|| mapping["opus"].as_str())?
                } else {
                    mapped?
                };
                let model = account
                    .row
                    .model_metadata
                    .iter()
                    .find(|model| model["id"] == id)?;
                limit = limit.min(
                    model["maxContextWindow"]
                        .as_u64()
                        .or_else(|| model["contextWindow"].as_u64())?,
                );
            }
            Some(limit)
        }
        _ => None,
    }
}

fn provider(row: &StoredSubscription) -> Result<SubscriptionProvider> {
    match row.provider.as_str() {
        "codex" => Ok(SubscriptionProvider::ChatGpt),
        "antigravity" => Ok(SubscriptionProvider::Antigravity),
        _ => Err(anyhow!("Subscription provider is unavailable")),
    }
}

fn credential_entry<'a>(
    files: &'a [Value],
    provider: SubscriptionProvider,
    identity: &SubscriptionAccountIdentity,
) -> Option<&'a Value> {
    files.iter().find(|file| {
        file["name"] == identity.credential_name && file["provider"] == provider.router_provider()
    })
}

fn credential_identity_matches(
    file: &Value,
    identity: &SubscriptionAccountIdentity,
    auth_index: Option<&str>,
) -> bool {
    auth_index.is_none_or(|index| {
        file["auth_index"]
            .as_str()
            .is_none_or(|actual| actual.is_empty() || actual == index)
    }) && identity
        .email
        .as_deref()
        .is_none_or(|email| file["email"].as_str().is_none_or(|actual| actual == email))
}

fn disable_other_credentials(
    client: &ManagementClient<'_>,
    files: &[Value],
    provider: SubscriptionProvider,
    identity: &SubscriptionAccountIdentity,
) -> Result<()> {
    // A prefixed inference request must never select another account
    // imported into the same private engine directory.
    for file in files
        .iter()
        .filter(|file| file["provider"] == provider.router_provider() && file["disabled"] != true)
    {
        if let Some(name) = file["name"]
            .as_str()
            .filter(|name| *name != identity.credential_name)
        {
            client.set_disabled(name, true)?;
        }
    }
    Ok(())
}

fn credential_active(file: &Value) -> bool {
    file["status"] == "active"
        && file["disabled"] != true
        && file["requires_sign_in"] != true
        && file["auth_index"]
            .as_str()
            .is_some_and(|index| !index.is_empty())
}

fn needs_sign_in(file: &Value) -> bool {
    static TERMINAL_REFRESH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:invalid[_ -]grant|invalid[_ -](?:refresh[_ -])?token|refresh[_ -]token[_ -](?:revoked|expired)|sign[_ -]in[_ -]required)\b")
            .expect("subscription refresh status pattern")
    });
    let refresh = &file["refresh_error"];
    file["requires_sign_in"] == true
        || matches!(
            file["status"].as_str(),
            Some("needs-sign-in" | "expired" | "unauthorized")
        )
        || matches!(
            file["renewal_state"].as_str(),
            Some(
                "failed"
                    | "requires_sign_in"
                    | "needs-sign-in"
                    | "sign_in_required"
                    | "expired_sign_in_required"
                    | "error"
            )
        )
        || refresh["http_status"] == 401
        || [
            refresh.as_str(),
            refresh["code"].as_str(),
            refresh["message"].as_str(),
        ]
        .into_iter()
        .flatten()
        .any(|text| TERMINAL_REFRESH.is_match(text))
}

fn model_ids(registered: &[Value]) -> Vec<&str> {
    registered
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect()
}

fn wait_for_models(
    client: &ManagementClient<'_>,
    name: &str,
    matches: impl Fn(&[&str]) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if matches(&model_ids(&client.models(name)?)) {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "Subscription models were not registered before the deadline"
        );
        thread::sleep(POLL);
    }
}

fn catalog_model(model: &Value) -> Option<CatalogModel> {
    let string = |key: &str| model[key].as_str().map(str::to_owned);
    Some(CatalogModel {
        id: string("id")?,
        display_name: string("displayName"),
        owned_by: string("ownedBy"),
        context_window: model["contextWindow"].as_u64(),
        max_context_window: model["maxContextWindow"].as_u64(),
        max_output_tokens: model["maxOutputTokens"].as_u64(),
        supported_efforts: model["supportedEfforts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    })
}

fn extra_models(row: &StoredSubscription) -> Vec<Value> {
    row.model_metadata
        .iter()
        .filter_map(catalog_model)
        .map(|model| {
            let mut extra = json!({"id": model.id});
            for (key, value) in [
                ("display_name", json!(model.display_name)),
                ("owned_by", json!(model.owned_by)),
                ("context_length", json!(model.context_window)),
                ("max_completion_tokens", json!(model.max_output_tokens)),
                ("thinking_levels", json!(model.supported_efforts)),
            ] {
                if !value.is_null() {
                    extra[key] = value;
                }
            }
            extra
        })
        .collect()
}
