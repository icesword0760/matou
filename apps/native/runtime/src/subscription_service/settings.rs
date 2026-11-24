use super::{account_details, *};
use crate::protocol::{SubscriptionDetails, SubscriptionModelsView, SubscriptionSettingsView};

pub(super) const DETAILS_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub(super) struct SettingsRound {
    lease: Mutex<RoundLease>,
}

#[derive(Default)]
struct RoundLease {
    closed: bool,
    router: Option<RouterLease>,
}

pub(super) struct CachedDetails {
    pub(super) checked: Instant,
    identity: [Value; 6],
    details: SubscriptionDetails,
}

impl SubscriptionService {
    pub fn begin_settings(&self, id: u64) -> Result<()> {
        let mut rounds = self.settings_rounds.lock().unwrap();
        ensure!(
            !rounds.contains_key(&id),
            "Subscription settings are already open"
        );
        rounds.insert(id, Arc::new(SettingsRound::default()));
        Ok(())
    }

    pub fn read_settings(&self, id: u64) -> Result<SubscriptionSettingsView> {
        let round = self
            .settings_rounds
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .context("Subscription settings are closed")?;
        let snapshot = self.store.snapshot()?;
        let subscriptions = snapshot["providers"]["claude-code"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["kind"] == "subscription")
            .filter_map(|row| row["id"].as_str())
            .collect::<Vec<_>>();
        let supported = self.require_supported().is_ok();
        let connection = if supported {
            self.settings_connection(&round).ok()
        } else {
            None
        };
        let files = connection
            .as_ref()
            .and_then(|connection| ManagementClient::new(connection).credentials().ok());
        let mut accounts = BTreeMap::new();
        for provider_id in subscriptions {
            ensure!(
                !round.lease.lock().unwrap().closed,
                "Subscription settings are closed"
            );
            let Some(row) = self.store.subscription(provider_id)? else {
                continue;
            };
            let mut details = SubscriptionDetails {
                account: row.account.clone(),
                models: row.model_metadata.clone(),
                quota: Vec::new(),
            };
            if details.account["status"] != "needs-sign-in" && supported {
                let kind = provider(&row)?;
                let file = files
                    .as_ref()
                    .and_then(|files| credential_entry(files, kind, &row.identity));
                if let Some(file) = file.filter(|file| {
                    credential_identity_matches(
                        file,
                        &row.identity,
                        row.identity.auth_index.as_deref(),
                    ) && !needs_sign_in(file)
                        && model_router::credential_path(&self.root, &row.identity.credential_name)
                            .is_ok()
                }) {
                    let identity = fingerprint(file);
                    let cached = self
                        .account_details
                        .lock()
                        .unwrap()
                        .get(&row.identity.credential_name)
                        .filter(|cached| {
                            cached.identity == identity && cached.checked.elapsed() < DETAILS_TTL
                        })
                        .map(|cached| cached.details.clone());
                    if let Some(cached) = cached {
                        details = cached;
                    } else {
                        let connection = connection
                            .as_ref()
                            .expect("credential list requires a connection");
                        details = account_details::read_details(
                            &ManagementClient::new(connection),
                            kind,
                            file,
                            Some(&row.model_metadata),
                        );
                        // Caching is private, but must not replace fields read from a newer account.
                        let _serial = self.serial.lock().unwrap();
                        if self
                            .store
                            .subscription(provider_id)?
                            .is_some_and(|current| current.identity == row.identity)
                        {
                            self.remember_details(
                                &row.identity.credential_name,
                                identity,
                                details.clone(),
                            );
                        }
                    }
                } else if files.is_some()
                    || model_router::credential_path(&self.root, &row.identity.credential_name)
                        .is_err()
                {
                    details.account["status"] = json!("needs-sign-in");
                } else {
                    details.account["status"] = json!("router-not-running");
                }
            }
            if details.account["status"] == "needs-sign-in" {
                self.store
                    .mark_subscription_needs_sign_in(provider_id, &row.identity)?;
                details.quota.clear();
            }
            let Some(current) = self.store.subscription(provider_id)? else {
                continue;
            };
            if current.identity == row.identity {
                if current.account["status"] == "needs-sign-in" {
                    details.account["status"] = json!("needs-sign-in");
                    details.quota.clear();
                }
                details.models = current.model_metadata;
                accounts.insert(provider_id.to_owned(), details);
            }
        }
        ensure!(
            !round.lease.lock().unwrap().closed,
            "Subscription settings are closed"
        );
        Ok(SubscriptionSettingsView {
            snapshot: self.store.snapshot()?,
            accounts,
            antigravity_risk_accepted: self.store.antigravity_risk_accepted()?,
        })
    }

    fn settings_connection(&self, round: &SettingsRound) -> Result<RouterConnection> {
        {
            let mut state = round.lease.lock().unwrap();
            ensure!(!state.closed, "Subscription settings are closed");
            state.router = Some(self.manager.reserve()?);
        }
        let connection = self.manager.wait_for_connection();
        let mut state = round.lease.lock().unwrap();
        ensure!(!state.closed, "Subscription settings are closed");
        if connection.is_err() {
            state.router.take();
        }
        connection
    }

    pub fn close_settings(&self, id: u64) {
        if let Some(round) = self.settings_rounds.lock().unwrap().remove(&id) {
            let mut state = round.lease.lock().unwrap();
            state.closed = true;
            state.router.take();
        }
    }

    pub fn close_all_settings(&self) {
        let owners = self
            .settings_rounds
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for owner in owners {
            self.close_settings(owner);
            let _ = self.cancel_settings_logins(owner);
        }
    }

    pub fn refresh_models(&self, owner: u64, id: &str) -> Result<SubscriptionModelsView> {
        self.require_supported()?;
        let round = self
            .settings_rounds
            .lock()
            .unwrap()
            .get(&owner)
            .cloned()
            .context("Subscription settings are closed")?;
        let row = self
            .store
            .subscription(id)?
            .context("Subscription account is unavailable")?;
        let name = &row.identity.credential_name;
        if row.account["status"] == "needs-sign-in"
            || model_router::credential_path(&self.root, name).is_err()
        {
            self.store
                .mark_subscription_needs_sign_in(id, &row.identity)?;
            return Err(account_details::SignInRequired.into());
        }
        let kind = provider(&row)?;
        let (connection, _lease) = self.manager.acquire()?;
        let client = ManagementClient::new(&connection);
        let files = client.credentials()?;
        let file = credential_entry(&files, kind, &row.identity).filter(|file| {
            credential_identity_matches(file, &row.identity, row.identity.auth_index.as_deref())
                && !needs_sign_in(file)
        });
        let models = match file.map(|file| {
            if kind == SubscriptionProvider::Antigravity {
                let _serial = self.serial.lock().unwrap();
                ensure!(
                    self.store.subscription(id)?.is_some_and(|current| {
                        current.identity == row.identity
                            && current.account["status"] != "needs-sign-in"
                    }),
                    "Subscription account changed during refresh"
                );
                login::pin_client_headers(&client, kind, file)?;
            }
            account_details::read_models(&client, kind, file, None, true)
        }) {
            Some(Ok(models)) => models,
            Some(Err(error)) if !error.is::<account_details::SignInRequired>() => {
                return Err(error);
            }
            _ => {
                self.store
                    .mark_subscription_needs_sign_in(id, &row.identity)?;
                return Err(account_details::SignInRequired.into());
            }
        };
        let _serial = self.serial.lock().unwrap();
        ensure!(
            !round.lease.lock().unwrap().closed,
            "Subscription settings are closed"
        );
        let current = self
            .store
            .subscription(id)?
            .filter(|current| current.identity == row.identity)
            .context("Subscription account changed during refresh")?;
        ensure!(
            current.account["status"] != "needs-sign-in",
            account_details::SignInRequired
        );
        self.store
            .cache_subscription_models(id, &row.identity, &models)?;
        let mut details = SubscriptionDetails {
            account: current.account,
            models,
            quota: Vec::new(),
        };
        if let Some(cache) = self.account_details.lock().unwrap().get_mut(name) {
            cache.details.models = details.models.clone();
            details = cache.details.clone();
        }
        Ok(SubscriptionModelsView {
            snapshot: self.store.snapshot()?,
            provider_id: id.into(),
            details,
        })
    }

    pub(super) fn remember_details(
        &self,
        name: &str,
        identity: [Value; 6],
        details: SubscriptionDetails,
    ) {
        self.account_details.lock().unwrap().insert(
            name.to_owned(),
            CachedDetails {
                checked: Instant::now(),
                identity,
                details,
            },
        );
    }
}

pub(super) fn fingerprint(file: &Value) -> [Value; 6] {
    [
        "auth_index",
        "updated_at",
        "modtime",
        "adopted_at",
        "access_token_expires_at",
        "size",
    ]
    .map(|field| file[field].clone())
}
