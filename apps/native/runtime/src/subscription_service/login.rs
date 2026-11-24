use std::sync::atomic::{AtomicU8, Ordering};

use rusqlite::Connection;
use uuid::Uuid;

use super::{account_details, callback, settings, *};
use crate::{
    browser_integration,
    model_router::antigravity_user_agent,
    protocol::{SubscriptionDetails, SubscriptionLoginStatus as Status, SubscriptionLoginView},
};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CREDENTIAL_WAIT: Duration = Duration::from_secs(2);
const CANCEL_WAIT: Duration = Duration::from_secs(8);
const BROWSER_PENDING: u8 = 0;
const BROWSER_OPENED: u8 = 1;
const BROWSER_CONTROLLED_FAILED: u8 = 2;
const BROWSER_SYSTEM_FAILED: u8 = 3;
const BROWSER_STOPPED: u8 = 4;

pub(super) struct LoginOptions {
    pub ports: Option<[u16; 2]>,
    pub timeout: Duration,
    pub open: fn(&str, bool) -> bool,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            ports: None,
            timeout: LOGIN_TIMEOUT,
            open: |url, system| {
                if system {
                    browser_integration::open_system_url(url)
                } else {
                    browser_integration::open_controlled_url(url)
                }
            },
        }
    }
}

pub(super) struct LoginAttempt {
    owner: u64,
    provider: SubscriptionProvider,
    status: Status,
    deadline: Instant,
    state: String,
    authorization: Option<String>,
    connection: RouterConnection,
    lease: Option<RouterLease>,
    callback: Option<callback::Callback>,
    browser: Arc<AtomicU8>,
    baseline: HashMap<String, [Value; 6]>,
    credential: Option<Value>,
    details: Option<SubscriptionDetails>,
    draft: Option<Value>,
    reason: Option<String>,
}

impl LoginAttempt {
    fn pending(&self) -> bool {
        matches!(
            self.status,
            Status::Waiting | Status::Exchanging | Status::Ready
        )
    }

    fn expired(&self) -> bool {
        matches!(self.status, Status::Waiting | Status::Exchanging)
            && Instant::now() >= self.deadline
    }

    fn can_open(&self) -> bool {
        self.status == Status::Waiting
            && !self.expired()
            && self.authorization.is_some()
            && self
                .callback
                .as_ref()
                .is_some_and(|callback| !callback.accepted.load(Ordering::Acquire))
    }

    fn open(&mut self, opener: fn(&str, bool) -> bool, system: bool) {
        let Some(url) = self.authorization.clone() else {
            return;
        };
        let browser = Arc::new(AtomicU8::new(BROWSER_PENDING));
        let Some(callback) = self.callback.as_ref() else {
            return;
        };
        let accepted = callback.accepted.clone();
        self.browser.store(BROWSER_STOPPED, Ordering::Release);
        self.browser = browser.clone();
        let fallback = if system {
            BROWSER_SYSTEM_FAILED
        } else {
            BROWSER_CONTROLLED_FAILED
        };
        if thread::Builder::new()
            .name("matou-subscription-browser".into())
            .spawn(move || {
                if browser.load(Ordering::Acquire) != BROWSER_PENDING
                    || accepted.load(Ordering::Acquire)
                {
                    return;
                }
                let result = if opener(&url, system) {
                    BROWSER_OPENED
                } else {
                    fallback
                };
                let _ = browser.compare_exchange(
                    BROWSER_PENDING,
                    result,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            })
            .is_err()
        {
            self.browser.store(fallback, Ordering::Release);
        }
    }

    fn view(&self, attempt_id: &str) -> SubscriptionLoginView {
        let browser = self.browser.load(Ordering::Acquire);
        let can_reopen = self.can_open() && browser != BROWSER_PENDING;
        let reason = self.reason.clone().or_else(|| {
            if !can_reopen {
                return None;
            }
            match browser {
                BROWSER_CONTROLLED_FAILED => Some(
                    "Could not open the sign-in browser. Try again or use the system browser."
                        .into(),
                ),
                BROWSER_SYSTEM_FAILED => Some(
                    "Could not open the system browser. Check your browser and try again.".into(),
                ),
                _ => None,
            }
        });
        SubscriptionLoginView {
            attempt_id: attempt_id.into(),
            provider: self.provider,
            status: self.status,
            can_reopen,
            system_browser_available: can_reopen
                && matches!(browser, BROWSER_CONTROLLED_FAILED | BROWSER_SYSTEM_FAILED),
            reason,
            draft: self.draft.clone(),
        }
    }

    fn stop_browser(&mut self) {
        self.authorization.take();
        self.browser.store(BROWSER_STOPPED, Ordering::Release);
    }
}

impl SubscriptionService {
    pub fn start_login(
        &self,
        owner: u64,
        provider: SubscriptionProvider,
        chinese: bool,
    ) -> Result<SubscriptionLoginView> {
        self.require_supported()?;
        ensure!(
            self.settings_rounds.lock().unwrap().contains_key(&owner),
            "Subscription settings are closed"
        );
        let _serial = self.serial.lock().unwrap();
        let mut attempts = self.login_attempts.lock().unwrap();
        if let Some((_, attempt)) = attempts
            .iter_mut()
            .find(|(_, attempt)| attempt.provider == provider && attempt.pending())
        {
            ensure!(
                attempt.owner == owner,
                "Sign-in is already in progress in another window. Finish it or cancel it there."
            );
            self.finish_attempt(attempt, Status::Cancelled, "Sign-in was cancelled")?;
        }
        attempts.retain(|_, attempt| attempt.owner != owner || attempt.provider != provider);
        ensure!(
            provider != SubscriptionProvider::Antigravity
                || self.store.antigravity_risk_accepted()?,
            "Antigravity risk confirmation is required"
        );
        let port = self.login_options.ports.map_or_else(
            || callback::callback_endpoint(provider).0,
            |ports| ports[usize::from(provider == SubscriptionProvider::Antigravity)],
        );
        let (port, listeners) = callback::bind(port)?;
        let (connection, lease) = self.manager.acquire()?;
        let client = ManagementClient::new(&connection);
        let baseline = client
            .credentials()?
            .iter()
            .filter_map(|file| {
                if file["provider"] != provider.router_provider() {
                    return None;
                }
                Some((
                    file["name"].as_str()?.to_owned(),
                    settings::fingerprint(file),
                ))
            })
            .collect();
        let login = client.start_oauth(provider)?;
        let state = login["state"]
            .as_str()
            .filter(|state| {
                !state.is_empty() && state.len() <= 512 && !state.chars().any(char::is_control)
            })
            .context("Invalid subscription sign-in response")?;
        let callback = (|| {
            let authorization = login["url"]
                .as_str()
                .context("Invalid subscription sign-in response")?;
            callback::validate_authorization_url(provider, authorization, state, port)?;
            let listener = callback::Callback::start(
                listeners,
                provider,
                state.into(),
                connection.clone(),
                chinese,
            )?;
            Ok((authorization.to_owned(), listener))
        })();
        let (authorization, listener) = match callback {
            Ok(value) => value,
            Err(error) => {
                let _ = client.cancel_oauth(state);
                return Err(error);
            }
        };
        let id = Uuid::new_v4().to_string();
        let mut attempt = LoginAttempt {
            owner,
            provider,
            status: Status::Waiting,
            deadline: Instant::now() + self.login_options.timeout,
            state: state.into(),
            authorization: Some(authorization),
            connection,
            lease: Some(lease),
            callback: Some(listener),
            browser: Arc::new(AtomicU8::new(BROWSER_PENDING)),
            baseline,
            credential: None,
            details: None,
            draft: None,
            reason: None,
        };
        attempt.open(self.login_options.open, false);
        let view = attempt.view(&id);
        attempts.insert(id, attempt);
        Ok(view)
    }

    pub fn login_status(&self, owner: u64, id: &str) -> Result<SubscriptionLoginView> {
        let _serial = self.serial.lock().unwrap();
        let mut attempts = self.login_attempts.lock().unwrap();
        let attempt = owned_attempt(&mut attempts, owner, id)?;
        if attempt.expired() {
            self.finish_attempt(attempt, Status::Expired, "Sign-in timed out. Start again.")?;
        }
        if !matches!(attempt.status, Status::Waiting | Status::Exchanging) {
            return Ok(attempt.view(id));
        }
        let accepted = attempt
            .callback
            .as_ref()
            .is_some_and(|callback| callback.accepted.load(Ordering::Acquire));
        if !accepted {
            return Ok(attempt.view(id));
        }
        attempt.stop_browser();
        attempt.status = Status::Exchanging;
        if attempt
            .callback
            .as_ref()
            .is_some_and(|callback| callback.failed.load(Ordering::Acquire))
        {
            self.finish_attempt(
                attempt,
                Status::Failed,
                "Sign-in was not completed. Try again.",
            )?;
            return Ok(attempt.view(id));
        }
        let client = ManagementClient::new(&attempt.connection);
        let status = match client.oauth_status(&attempt.state) {
            Ok(status) => status,
            Err(_) => {
                self.finish_attempt(
                    attempt,
                    Status::Failed,
                    "Could not read sign-in status. Try again.",
                )?;
                return Ok(attempt.view(id));
            }
        };
        match status["status"].as_str() {
            Some("wait") => {}
            Some("ok") => {
                if self.prepare_draft(attempt, &status).is_err() {
                    self.finish_attempt(
                        attempt,
                        Status::Failed,
                        "Could not read the signed-in account or its models. Try again.",
                    )?;
                }
            }
            _ => self.finish_attempt(
                attempt,
                Status::Failed,
                "Sign-in was not completed. Try again.",
            )?,
        }
        Ok(attempt.view(id))
    }

    pub fn reopen_login(
        &self,
        owner: u64,
        id: &str,
        system: bool,
    ) -> Result<SubscriptionLoginView> {
        let mut attempts = self.login_attempts.lock().unwrap();
        let attempt = owned_attempt(&mut attempts, owner, id)?;
        let view = attempt.view(id);
        if view.can_reopen && (!system || view.system_browser_available) {
            attempt.open(self.login_options.open, system);
        }
        Ok(attempt.view(id))
    }

    pub fn cancel_login(&self, owner: u64, id: &str) -> Result<SubscriptionLoginView> {
        let _serial = self.serial.lock().unwrap();
        let mut attempts = self.login_attempts.lock().unwrap();
        let attempt = owned_attempt(&mut attempts, owner, id)?;
        self.finish_attempt(attempt, Status::Cancelled, "Sign-in was cancelled")?;
        Ok(attempt.view(id))
    }

    pub fn cancel_settings_logins(&self, owner: u64) -> Result<()> {
        let _serial = self.serial.lock().unwrap();
        let mut attempts = self.login_attempts.lock().unwrap();
        let mut result = Ok(());
        attempts.retain(|_, attempt| {
            if attempt.owner != owner {
                return true;
            }
            if let Err(error) =
                self.finish_attempt(attempt, Status::Cancelled, "Sign-in was cancelled")
            {
                result = Err(error);
            }
            false
        });
        result
    }

    pub fn expire_logins(&self) {
        if !self
            .login_attempts
            .lock()
            .unwrap()
            .values()
            .any(|attempt| attempt.expired())
        {
            return;
        }
        let _serial = self.serial.lock().unwrap();
        for attempt in self.login_attempts.lock().unwrap().values_mut() {
            if attempt.expired() {
                let _ = self.finish_attempt(
                    attempt,
                    Status::Expired,
                    "Sign-in timed out. Start again.",
                );
            }
        }
    }

    fn prepare_draft(&self, attempt: &mut LoginAttempt, status: &Value) -> Result<()> {
        let client = ManagementClient::new(&attempt.connection);
        let file = self
            .resolve_credential(attempt, status, CREDENTIAL_WAIT)?
            .context("Signed-in credential has not been adopted")?;
        let name = file["name"]
            .as_str()
            .context("Missing subscription credential")?;
        let _ = model_router::credential_path(&self.root, name)?;
        // Record the credential before any fallible step so cleanup can still
        // disable a newly issued account after a partial failure.
        attempt.credential = Some(credential_reference(&file));
        let previous = self.existing_subscription(attempt.provider)?;
        let referenced = previous
            .as_ref()
            .is_some_and(|(_, row)| row.identity.credential_name == name);
        if referenced {
            let (id, row) = previous.as_ref().expect("saved subscription");
            if !credential_identity_matches(
                &file,
                &row.identity,
                row.identity.auth_index.as_deref(),
            ) {
                self.revoke_provider_targets(id);
                client.set_disabled(name, true)?;
            } else {
                client.set_prefix(name, attempt.provider.prefix())?;
                client.set_disabled(name, false)?;
            }
        } else {
            client.set_disabled(name, true)?;
            client.set_prefix(name, "")?;
        }
        pin_client_headers(&client, attempt.provider, &file)?;
        let details = account_details::read_details(&client, attempt.provider, &file, None);
        ensure!(
            details.account["status"] != "needs-sign-in" && !details.models.is_empty(),
            "Signed-in account has no available models"
        );
        let draft = make_draft(
            attempt.provider,
            &details,
            previous.as_ref().map(|(id, _)| id.as_str()),
            &self.store.snapshot()?,
        );
        attempt.details = Some(details);
        attempt.draft = Some(draft);
        attempt.callback.take();
        attempt.status = Status::Ready;
        Ok(())
    }

    fn resolve_credential(
        &self,
        attempt: &LoginAttempt,
        status: &Value,
        wait: Duration,
    ) -> Result<Option<Value>> {
        let auth_id = status["auth_id"].as_str().filter(|value| !value.is_empty());
        if auth_id.is_none() && attempt.provider != SubscriptionProvider::Antigravity {
            return Ok(None);
        }
        let client = ManagementClient::new(&attempt.connection);
        let deadline = Instant::now() + wait;
        loop {
            let files = client.credentials()?;
            let mut candidates = files.into_iter().filter(|file| {
                if file["provider"] != attempt.provider.router_provider() {
                    return false;
                }
                let Some(name) = file["name"].as_str() else {
                    return false;
                };
                if let Some(auth_id) = auth_id {
                    return name == auth_id || file["id"] == auth_id;
                }
                attempt.baseline.get(name) != Some(&settings::fingerprint(file))
            });
            let found = candidates.next();
            if let Some(another) = candidates.next() {
                // Without a unique match, keep the saved account and disarm
                // newly discovered accounts until a later, unambiguous sign-in.
                for file in found
                    .iter()
                    .chain(std::iter::once(&another))
                    .chain(candidates.collect::<Vec<_>>().iter())
                {
                    if let Some(name) = file["name"]
                        .as_str()
                        .filter(|name| !attempt.baseline.contains_key(*name))
                    {
                        let _ = client.set_disabled(name, true);
                    }
                }
                return Err(anyhow!(
                    "Sign-in matched more than one credential. Try again."
                ));
            }
            if found.is_some() || Instant::now() >= deadline {
                return Ok(found);
            }
            thread::sleep(POLL);
        }
    }

    fn finish_attempt(
        &self,
        attempt: &mut LoginAttempt,
        status: Status,
        reason: &str,
    ) -> Result<()> {
        if !attempt.pending() {
            return Ok(());
        }
        attempt.status = status;
        attempt.reason = Some(reason.to_owned());
        attempt.stop_browser();
        let accepted = attempt
            .callback
            .take()
            .is_some_and(|callback| callback.accepted.load(Ordering::Acquire));
        let client = ManagementClient::new(&attempt.connection);
        let cancelled = client.cancel_oauth(&attempt.state).unwrap_or(false);
        if accepted && attempt.credential.is_none() && !cancelled {
            let deadline = Instant::now() + CANCEL_WAIT;
            while let Ok(status) = client.oauth_status(&attempt.state) {
                match status["status"].as_str() {
                    Some("wait") if Instant::now() < deadline => thread::sleep(POLL),
                    Some("ok") => {
                        attempt.credential = self
                            .resolve_credential(attempt, &status, Duration::from_millis(500))
                            .ok()
                            .flatten()
                            .as_ref()
                            .map(credential_reference);
                        break;
                    }
                    _ => break,
                }
            }
        }
        let result = self.cleanup_draft(attempt);
        attempt.lease.take();
        attempt.details.take();
        attempt.draft.take();
        result
    }

    fn cleanup_draft(&self, attempt: &LoginAttempt) -> Result<()> {
        let Some(file) = &attempt.credential else {
            return Ok(());
        };
        let name = file["name"]
            .as_str()
            .context("Missing subscription credential")?;
        let client = ManagementClient::new(&attempt.connection);
        if let Some((id, row)) = self.existing_subscription(attempt.provider)?
            && row.identity.credential_name == name
        {
            if credential_identity_matches(file, &row.identity, row.identity.auth_index.as_deref())
            {
                client.set_prefix(name, attempt.provider.prefix())?;
                client.set_disabled(name, false)?;
            } else {
                self.store
                    .mark_subscription_needs_sign_in(&id, &row.identity)?;
                client.set_disabled(name, true)?;
            }
        } else {
            client.set_disabled(name, true)?;
            client.delete_credential(name)?;
        }
        Ok(())
    }

    pub fn commit_draft(
        &self,
        owner: u64,
        id: &str,
        input: &Value,
        activate: bool,
        db: &mut Connection,
        now: i64,
    ) -> Result<Value> {
        let _serial = self.serial.lock().unwrap();
        let mut attempts = self.login_attempts.lock().unwrap();
        let attempt = owned_attempt(&mut attempts, owner, id)?;
        ensure!(
            attempt.status == Status::Ready,
            "Sign-in is not ready to save"
        );
        let details = attempt
            .details
            .as_ref()
            .context("Sign-in is not ready to save")?;
        let credential = attempt
            .credential
            .as_ref()
            .context("Sign-in is not ready to save")?;
        let name = credential["name"]
            .as_str()
            .context("Missing subscription credential")?;
        let expected_index = credential["auth_index"]
            .as_str()
            .filter(|value| !value.is_empty())
            .context("Subscription credential has not been adopted")?;
        let previous = self.existing_subscription(attempt.provider)?;
        ensure!(
            input["id"].as_str().filter(|id| !id.is_empty())
                == previous.as_ref().map(|(id, _)| id.as_str()),
            "Subscription changed while sign-in was open"
        );
        let identity = SubscriptionAccountIdentity {
            credential_name: name.into(),
            auth_index: Some(expected_index.into()),
            email: details.account["email"].as_str().map(str::to_owned),
        };
        let mut commit = input.clone();
        ensure!(
            commit.is_object()
                && commit.get("apiKey").is_none()
                && commit.get("subscription").is_none(),
            "Invalid subscription draft"
        );
        commit["subscription"] = json!({
            "provider": attempt.provider.router_provider(),
            "credentialRef": {"name": name, "authIndex": expected_index},
            "account": details.account, "modelMetadata": details.models,
        });
        ProviderConfigStore::validate_subscription_draft(&commit)?;
        let row = StoredSubscription {
            name: commit["name"].as_str().unwrap_or_default().into(),
            provider: attempt.provider.router_provider().into(),
            identity: identity.clone(),
            account: details.account.clone(),
            model_metadata: details.models.clone(),
            model_mapping: commit["modelMapping"].clone(),
            context_window: None,
        };
        let (connection, lease) = self.manager.acquire()?;
        ensure!(
            connection.generation == attempt.connection.generation,
            "Model router restarted. Sign in again."
        );
        let client = ManagementClient::new(&connection);
        let files = client.credentials()?;
        let current = credential_entry(&files, attempt.provider, &identity)
            .context("Subscription credential changed during sign-in")?;
        ensure!(
            current["auth_index"] == expected_index
                && credential_identity_matches(current, &identity, Some(expected_index))
                && !needs_sign_in(current),
            "Subscription credential changed during sign-in"
        );
        let old_aliases = client.aliases(attempt.provider)?;
        let old_name = previous
            .as_ref()
            .map(|(_, row)| row.identity.credential_name.as_str());
        let changed = previous
            .as_ref()
            .is_some_and(|(_, row)| row.identity != identity);
        let other_enabled = files
            .iter()
            .filter(|file| {
                file["provider"] == attempt.provider.router_provider()
                    && file["disabled"] != true
                    && file["name"] != name
            })
            .filter_map(|file| file["name"].as_str())
            .collect::<Vec<_>>();
        if changed && let Some((id, _)) = &previous {
            self.revoke_provider_targets(id);
        }
        let result = (|| {
            for other in &other_enabled {
                client.set_disabled(other, true)?;
            }
            client.set_extra_models(name, attempt.provider.prefix(), extra_models(&row))?;
            client.set_disabled(name, false)?;
            let deadline = Instant::now() + READY_TIMEOUT;
            loop {
                let files = client.credentials()?;
                if credential_entry(&files, attempt.provider, &identity).is_some_and(|file| {
                    file["auth_index"] == expected_index
                        && credential_active(file)
                        && credential_identity_matches(file, &identity, Some(expected_index))
                }) {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "Subscription credential is not active"
                );
                thread::sleep(POLL);
            }
            let account = PreparedAccount {
                row,
                provider: attempt.provider,
                connection: connection.clone(),
                lease,
                auth_index: expected_index.into(),
            };
            self.prepare_launch(&account, ModelRole::Opus, None, &[])?;
            self.store
                .adopt_subscription_with_context_window(db, &commit, now, activate)
        })();
        match result {
            Ok(provider) => {
                if let Some(old) = old_name.filter(|old| *old != name) {
                    self.account_details.lock().unwrap().remove(old);
                }
                self.remember_details(name, settings::fingerprint(current), details.clone());
                attempts.remove(id);
                Ok(provider)
            }
            Err(error) => {
                // Never revive the captured bindings. A new session prepares
                // the restored account against the current router generation.
                let mut rollback = client.set_aliases(attempt.provider, &old_aliases);
                self.manager
                    .remember_aliases(attempt.provider.router_provider(), old_aliases);
                if old_name != Some(name) || changed {
                    rollback = rollback.and(client.set_disabled(name, true));
                    rollback = rollback.and(client.set_prefix(name, ""));
                } else if let Some((_, previous)) = &previous {
                    rollback = rollback.and(client.set_extra_models(
                        name,
                        attempt.provider.prefix(),
                        extra_models(previous),
                    ));
                }
                for other in other_enabled {
                    rollback = rollback.and(client.set_disabled(other, false));
                }
                rollback.context("Could not fully restore the previous subscription")?;
                Err(error)
            }
        }
    }

    pub(super) fn require_supported(&self) -> Result<()> {
        let major = model_router::macos_version()
            .and_then(|version| version.split('.').next()?.parse().ok());
        ensure!(
            subscription_launch::subscription_supported(cfg!(target_os = "macos"), major),
            "Subscriptions require macOS 15 or later"
        );
        Ok(())
    }

    fn existing_subscription(
        &self,
        provider: SubscriptionProvider,
    ) -> Result<Option<(String, StoredSubscription)>> {
        let snapshot = self.store.snapshot()?;
        let Some(id) = snapshot["providers"]["claude-code"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["subscription"]["provider"] == provider.router_provider())
            .and_then(|row| row["id"].as_str())
        else {
            return Ok(None);
        };
        Ok(self.store.subscription(id)?.map(|row| (id.to_owned(), row)))
    }
}

fn credential_reference(file: &Value) -> Value {
    json!({"name": file["name"], "auth_index": file["auth_index"], "email": file["email"]})
}

fn owned_attempt<'a>(
    attempts: &'a mut HashMap<String, LoginAttempt>,
    owner: u64,
    id: &str,
) -> Result<&'a mut LoginAttempt> {
    attempts
        .get_mut(id)
        .filter(|attempt| attempt.owner == owner)
        .context("Subscription sign-in is unavailable")
}

pub(super) fn pin_client_headers(
    client: &ManagementClient<'_>,
    provider: SubscriptionProvider,
    file: &Value,
) -> Result<()> {
    let (key, value) = match provider {
        SubscriptionProvider::ChatGpt => ("Originator", "codex-tui"),
        SubscriptionProvider::Antigravity => ("User-Agent", antigravity_user_agent()),
    };
    if file["headers"][key] != value {
        let name = file["name"]
            .as_str()
            .context("Missing subscription credential")?;
        let mut headers = file["headers"].as_object().cloned().unwrap_or_default();
        headers.retain(|name, _| !name.eq_ignore_ascii_case(key));
        headers.insert(key.into(), json!(value));
        client.set_headers(name, Value::Object(headers))?;
    }
    Ok(())
}

fn make_draft(
    provider: SubscriptionProvider,
    details: &SubscriptionDetails,
    existing: Option<&str>,
    snapshot: &Value,
) -> Value {
    let saved = snapshot["providers"]["claude-code"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| existing.is_some_and(|id| row["id"] == id))
        .unwrap_or(&Value::Null);
    let mut draft = json!({
        "cli": "claude-code", "kind": "subscription",
        "name": saved["name"].as_str().unwrap_or(match provider {
            SubscriptionProvider::ChatGpt => "ChatGPT / Codex",
            SubscriptionProvider::Antigravity => "Antigravity",
        }),
        "subscription": {"provider": provider.router_provider(), "account": details.account, "modelMetadata": details.models, "quota": details.quota},
        "modelMapping": {}, "effortMapping": {},
    });
    if let Some(id) = existing {
        draft["id"] = json!(id);
    }
    for key in ["contextWindow", "contextWindowMode"] {
        if let Some(value) = saved.get(key) {
            draft[key] = value.clone();
        }
    }
    for role in CLAUDE_ROLES {
        let preserved = details
            .models
            .iter()
            .find(|model| model["id"] == saved["modelMapping"][role]);
        let model = preserved
            .or_else(|| default_model(provider, role, &details.models))
            .or_else(|| {
                (role == "fable")
                    .then(|| {
                        details
                            .models
                            .iter()
                            .find(|model| model["id"] == draft["modelMapping"]["opus"])
                    })
                    .flatten()
            })
            .or_else(|| details.models.first());
        let Some(model) = model else { continue };
        draft["modelMapping"][role] = model["id"].clone();
        let effort = saved["effortMapping"][role].as_str().unwrap_or("follow");
        draft["effortMapping"][role] = json!(if preserved.is_some()
            && effort_supported(provider, model, effort)
        {
            effort
        } else {
            "follow"
        });
    }
    draft
}

fn effort_supported(provider: SubscriptionProvider, model: &Value, effort: &str) -> bool {
    effort == "follow"
        || (effort != "max" || provider == SubscriptionProvider::Antigravity)
            && model["supportedEfforts"].as_array().map_or_else(
                || {
                    matches!(effort, "low" | "medium" | "high" | "xhigh")
                        || (effort == "max"
                            && ["claude", "opus", "fable", "sonnet", "haiku"]
                                .iter()
                                .any(|part| model_label(model).contains(part)))
                },
                |choices| choices.iter().any(|choice| choice == effort),
            )
}

fn model_label(model: &Value) -> String {
    format!(
        "{} {}",
        model["id"].as_str().unwrap_or_default(),
        model["displayName"].as_str().unwrap_or_default()
    )
    .to_ascii_lowercase()
}

fn default_model<'a>(
    provider: SubscriptionProvider,
    role: &str,
    models: &'a [Value],
) -> Option<&'a Value> {
    if provider == SubscriptionProvider::ChatGpt {
        let preferred = match role {
            "opus" => "gpt-6-astra",
            "sonnet" => "gpt-6.1-sol",
            "haiku" => "gpt-6-luna",
            _ => return None,
        };
        return models.iter().find(|model| model["id"] == preferred);
    }
    let find = |parts: &[&str]| {
        models.iter().find(|model| {
            let label = model_label(model);
            parts.iter().all(|part| label.contains(part))
        })
    };
    match role {
        "opus" => find(&["claude", "opus", "thinking"]).or_else(|| find(&["claude", "opus"])),
        "fable" => find(&["claude", "fable"]),
        "sonnet" => find(&["claude", "sonnet"]),
        "haiku" => find(&["gemini", "flash"]),
        _ => None,
    }
}

#[cfg(all(test, target_os = "macos", debug_assertions))]
#[path = "login_tests.rs"]
mod tests;
