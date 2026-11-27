//! Read-only account summaries and the legacy offline account snapshot.

use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Private-on-disk OAuth fields, using the frozen official credential field names.
/// Deliberately has no `Debug` implementation.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountCredentials {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
}

/// Raw fixture record. Use `AccountStore::project` for UI and IPC.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub id: String,
    pub name: String,
    pub email: String,
    pub plan: String,
    #[serde(default, rename = "claudeAiOauth")]
    pub credentials: AccountCredentials,
    #[serde(default)]
    pub requires_sign_in: bool,
    #[serde(default)]
    pub retry_at_ms: Option<u64>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStore {
    pub current_id: Option<String>,
    pub accounts: Vec<AccountRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadState {
    Loaded,
    Missing,
    Corrupt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountStatus {
    Active,
    Expired,
    NeedsActivation,
    RateLimited,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub name: String,
    pub email: String,
    pub plan: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<&'static str>,
    pub status: AccountStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at_ms: Option<u64>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProjection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_id: Option<String>,
    pub accounts: Vec<AccountView>,
}

impl AccountStore {
    /// Invalid JSON or roster relationships fall back to an empty view and leave
    /// the original bytes intact. I/O failures other than absence are reported.
    pub fn load(path: &Path) -> io::Result<(Self, LoadState)> {
        let mut file = match open_snapshot(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Self::default(), LoadState::Missing));
            }
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(store) if store.valid() => Ok((store, LoadState::Loaded)),
            _ => Ok((Self::default(), LoadState::Corrupt)),
        }
    }

    /// Publish one complete snapshot in the caller's existing fixture directory.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if !self.valid() || path.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid account snapshot",
            ));
        }
        let bytes =
            serde_json::to_vec(self).map_err(|_| io::Error::other("Invalid account JSON"))?;
        let temporary = path.with_file_name(format!(".account-{}.tmp", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let written = (|| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&bytes)
        })();
        drop(file);
        let result = written.and_then(|()| fs::rename(&temporary, path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn project(&self, now_ms: u64) -> AccountProjection {
        let secrets: HashSet<_> = self.secrets().collect();
        self.project_with_redaction(now_ms, |value| redact(value, &secrets))
    }

    fn secrets(&self) -> impl Iterator<Item = &str> {
        self.accounts.iter().flat_map(|account| {
            [
                account.credentials.access_token.as_deref(),
                account.credentials.refresh_token.as_deref(),
            ]
            .into_iter()
            .flatten()
            .filter(|token| !token.is_empty())
        })
    }

    fn project_with_redaction(
        &self,
        now_ms: u64,
        safe: impl Fn(&str) -> String,
    ) -> AccountProjection {
        AccountProjection {
            current_id: self.current_id.as_deref().map(&safe),
            accounts: self
                .accounts
                .iter()
                .map(|account| {
                    let status = account.status(self.current_id.as_deref(), now_ms);
                    AccountView {
                        id: safe(&account.id),
                        name: safe(&account.name),
                        email: safe(&masked_email(&safe(&account.email))),
                        plan: safe(&account.plan),
                        provider: None,
                        status,
                        retry_at_ms: account
                            .retry_at_ms
                            .filter(|_| status == AccountStatus::RateLimited),
                    }
                })
                .collect(),
        }
    }

    pub fn project_data_root(
        data_root: &Path,
        now_ms: u64,
    ) -> io::Result<(AccountProjection, LoadState)> {
        let (legacy, legacy_state) = Self::load(&data_root.join("accounts.json"))?;
        let config = read_json(&data_root.join("provider-config.json"));
        let mut corrupt = config
            .as_ref()
            .is_err_and(|error| error.kind() != io::ErrorKind::NotFound);
        let config = config.unwrap_or_default();
        let selected = config["activeProviderIds"]["claude-code"].as_str();
        let mut secrets: HashSet<_> = legacy.secrets().collect();
        collect_secrets(&config, &mut secrets);

        // Router config contains private keys and a configurable auth-dir. The
        // frozen Matou writer uses this fixed directory under the data root.
        let router = data_root.join("model-router");
        let directory = router.join("credentials");
        let mut credentials = BTreeMap::new();
        if [&router, &directory]
            .into_iter()
            .all(|path| fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        {
            for entry in fs::read_dir(&directory).into_iter().flatten().flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if !name.ends_with(".json") {
                    continue;
                }
                match read_json(&entry.path()) {
                    Ok(credential) => {
                        credentials.insert(name, credential);
                    }
                    Err(error) if error.kind() != io::ErrorKind::NotFound => corrupt = true,
                    Err(_) => {}
                }
            }
        }
        for (name, credential) in &credentials {
            secrets.insert(name.as_str());
            collect_secrets(credential, &mut secrets);
        }
        let safe = |value: &str| redact(value, &secrets);
        let mut projection = legacy.project_with_redaction(now_ms, safe);

        let mut configured = HashSet::new();
        let mut linked = HashSet::new();
        let mut current_subscription = None;
        let mut has_subscriptions = false;
        for row in config["providers"]["claude-code"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let subscription = &row["subscription"];
            let Some(provider) = subscription_provider(&subscription["provider"]) else {
                continue;
            };
            let Some(id) = row["id"].as_str().filter(|id| !id.trim().is_empty()) else {
                continue;
            };
            if row["kind"] != "subscription" || !configured.insert(id) {
                continue;
            }
            let account = &subscription["account"];
            let reference = &subscription["credentialRef"];
            let credential = reference["name"].as_str().and_then(|name| {
                let (name, credential) = credentials.get_key_value(name)?;
                if subscription_provider(&credential["type"]) != Some(provider) {
                    return None;
                }
                linked.insert(name.as_str());
                let matches = |wanted: &Value, actual: &Value| {
                    wanted
                        .as_str()
                        .zip(actual.as_str())
                        .is_none_or(|(left, right)| left == right)
                };
                (matches(&reference["authIndex"], &credential["auth_index"])
                    && matches(&account["email"], &credential["email"]))
                .then_some(credential)
            });
            let current = selected == Some(id);
            let mut view = subscription_view(
                id.to_owned(),
                provider,
                account,
                credential,
                now_ms,
            );
            safe_subscription_view(&mut view, &safe);
            if current {
                current_subscription = Some(view.id.clone());
            }
            if let Some(index) = legacy.accounts.iter().position(|record| record.id == id) {
                projection.accounts[index] = view;
                if legacy.current_id.as_deref() == Some(id) {
                    projection.current_id = None;
                }
            } else {
                projection.accounts.push(view);
            }
            has_subscriptions = true;
        }
        for (name, credential) in &credentials {
            let Some(provider) = subscription_provider(&credential["type"]) else {
                continue;
            };
            if linked.contains(name.as_str()) {
                continue;
            }
            let mut view = subscription_view(
                subscription_id(provider, name),
                provider,
                &Value::Null,
                Some(credential),
                now_ms,
            );
            safe_subscription_view(&mut view, &safe);
            projection.accounts.push(view);
            has_subscriptions = true;
        }
        if current_subscription.is_some() {
            projection.current_id = current_subscription;
        }
        let state = if has_subscriptions || legacy_state == LoadState::Loaded {
            LoadState::Loaded
        } else if corrupt || legacy_state == LoadState::Corrupt {
            LoadState::Corrupt
        } else {
            LoadState::Missing
        };
        Ok((projection, state))
    }

    fn valid(&self) -> bool {
        let mut ids = HashSet::new();
        self.accounts
            .iter()
            .all(|account| !account.id.trim().is_empty() && ids.insert(account.id.as_str()))
            && self
                .current_id
                .as_deref()
                .is_none_or(|current| ids.contains(current))
    }
}

fn open_snapshot(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Account snapshot must be a regular file",
        ));
    }
    Ok(file)
}

fn read_json(path: &Path) -> io::Result<Value> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let value: Value = serde_json::from_reader(io::BufReader::new(open_snapshot(path)?))?;
    if !value.is_object() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(value)
}

fn collect_secrets<'a>(value: &'a Value, secrets: &mut HashSet<&'a str>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_secrets(item, secrets);
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                match key.as_str() {
                    "access_token" | "accessToken" | "refresh_token" | "refreshToken"
                    | "id_token" | "idToken" | "api_key" | "apiKey" | "auth_index"
                    | "authIndex" => {
                        if let Some(secret) = value.as_str().filter(|secret| !secret.is_empty()) {
                            secrets.insert(secret);
                        }
                    }
                    "credentialRef" => {
                        // References can carry both a filename and a private index.
                        for secret in [value, &value["name"], &value["authIndex"]]
                            .into_iter()
                            .filter_map(Value::as_str)
                            .filter(|secret| !secret.is_empty())
                        {
                            secrets.insert(secret);
                        }
                        if let Some(stem) = value["name"]
                            .as_str()
                            .and_then(|name| name.strip_suffix(".json"))
                            .filter(|stem| !stem.is_empty())
                        {
                            secrets.insert(stem);
                        }
                    }
                    _ => collect_secrets(value, secrets),
                }
            }
        }
        _ => {}
    }
}

fn subscription_provider(value: &Value) -> Option<&'static str> {
    match value.as_str()? {
        "codex" => Some("codex"),
        "antigravity" => Some("antigravity"),
        _ => None,
    }
}

fn subscription_id(provider: &str, value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("subscription-{provider}-{digest:x}")
}

fn redact(value: &str, secrets: &HashSet<&str>) -> String {
    if secrets.iter().any(|secret| value.contains(secret)) {
        "[redacted]".into()
    } else {
        value.into()
    }
}

fn safe_subscription_view(view: &mut AccountView, safe: impl Fn(&str) -> String) {
    if safe(&view.id) != view.id {
        view.id = subscription_id(view.provider.unwrap_or_default(), &view.id);
    }
    view.plan = safe(&view.plan);
    view.email = safe(&masked_email(&safe(&view.email)));
}

fn subscription_view(
    id: String,
    provider: &'static str,
    account: &Value,
    credential: Option<&Value>,
    now_ms: u64,
) -> AccountView {
    use AccountStatus::*;
    let credential_present = credential.is_some();
    let credential = credential.unwrap_or(&Value::Null);
    let saved_status = account["status"].as_str().unwrap_or_default();
    let credential_status = credential["status"].as_str().unwrap_or_default();
    let retry_at_ms = [account, credential]
        .into_iter()
        .filter_map(|source| {
            source["retryAtMs"]
                .as_u64()
                .or_else(|| source["retry_at_ms"].as_u64())
                .or_else(|| {
                    source["next_retry_after"]
                        .as_str()
                        .and_then(timestamp_ms)
                        .and_then(|time| u64::try_from(time).ok())
                })
        })
        .max();
    let present = |field: &str| {
        credential[field]
            .as_str()
            .is_some_and(|token| !token.trim().is_empty())
    };
    let access_expired = credential["expired"]
        .as_str()
        .and_then(timestamp_ms)
        .is_some_and(|expires_at| i128::from(expires_at) <= i128::from(now_ms));
    let refresh_error = &credential["refresh_error"];
    let sign_in_required = credential["requires_sign_in"] == true
        || matches!(
            credential["renewal_state"].as_str(),
            Some(
                "failed"
                    | "requires_sign_in"
                    | "needs-sign-in"
                    | "sign_in_required"
                    | "expired_sign_in_required"
                    | "error"
            )
        )
        || refresh_error["http_status"] == 401
        || matches!(
            refresh_error["code"].as_str(),
            Some(
                "invalid_grant"
                    | "invalid_token"
                    | "invalid_refresh_token"
                    | "refresh_token_revoked"
                    | "refresh_token_expired"
                    | "sign_in_required"
            )
        );
    let status = if matches!(
        saved_status,
        "needs-sign-in" | "expired" | "account-unavailable"
    ) || matches!(credential_status, "needs-sign-in" | "expired")
        || sign_in_required
        || !credential_present
        || !(present("refresh_token") || present("access_token") && !access_expired)
    {
        Expired
    } else if retry_at_ms.map_or_else(
        || {
            matches!(saved_status, "rate-limited" | "quota-exhausted")
                || matches!(credential_status, "rate-limited" | "quota-exhausted")
        },
        |retry_at| retry_at > now_ms,
    ) {
        RateLimited
    } else if credential["disabled"] == true
        || matches!(saved_status, "needs-activation" | "router-not-running")
        || matches!(credential_status, "disabled" | "needs-activation")
    {
        NeedsActivation
    } else {
        Active
    };
    AccountView {
        id,
        name: match provider {
            "codex" => "ChatGPT",
            _ => "Antigravity",
        }
        .into(),
        email: account["email"]
            .as_str()
            .or_else(|| credential["email"].as_str())
            .unwrap_or_default()
            .into(),
        plan: account["plan"]
            .as_str()
            .or_else(|| credential["plan"].as_str())
            .or_else(|| credential["plan_type"].as_str())
            .unwrap_or_default()
            .into(),
        provider: Some(provider),
        status,
        retry_at_ms: retry_at_ms.filter(|_| status == RateLimited),
    }
}

// Router credential expiry is RFC 3339. Convert the Gregorian date using
// the inverse of the calendar conversion in provider_config::iso_time.
fn timestamp_ms(value: &str) -> Option<i64> {
    let (date, clock) = value.split_once('T')?;
    let (year, date) = date.split_once('-')?;
    let (month, day) = date.split_once('-')?;
    let (hours, clock) = clock.split_once(':')?;
    let (minutes, clock) = clock.split_once(':')?;
    let (clock, offset) = if let Some(clock) = clock.strip_suffix('Z') {
        (clock, 0)
    } else {
        let offset_start = clock.find(['+', '-'])?;
        let (clock, zone) = clock.split_at(offset_start);
        let (hours, minutes) = zone.get(1..)?.split_once(':')?;
        let offset = decimal(hours, 2, 23)? * 60 + decimal(minutes, 2, 59)?;
        (
            clock,
            if zone.starts_with('-') {
                -offset
            } else {
                offset
            },
        )
    };
    let (seconds, fraction) = clock.split_once('.').unwrap_or((clock, "0"));
    if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let millis = fraction
        .bytes()
        .take(3)
        .zip([100, 10, 1])
        .map(|(digit, factor)| i64::from(digit - b'0') * factor)
        .sum::<i64>();
    let year = decimal(year, 4, 9999)?;
    let month = decimal(month, 2, 12)?;
    let day = decimal(day, 2, 31)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        28 + i64::from(leap),
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if month == 0 || day == 0 || day > month_days[(month - 1) as usize] {
        return None;
    }
    let hours = decimal(hours, 2, 23)?;
    let minutes = decimal(minutes, 2, 59)?;
    let seconds = decimal(seconds, 2, 59)?;
    let year = year - i64::from(month <= 2);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let days = year.div_euclid(400) * 146_097 + year_of_era * 365 + year_of_era / 4
        - year_of_era / 100
        + day_of_year
        - 719_468;
    Some(((days * 24 + hours) * 60 + minutes - offset) * 60_000 + seconds * 1000 + millis)
}

fn decimal(part: &str, width: usize, max: i64) -> Option<i64> {
    if part.len() != width || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse::<i64>().ok().filter(|number| *number <= max)
}

impl AccountRecord {
    pub fn status(&self, current_id: Option<&str>, now_ms: u64) -> AccountStatus {
        let present = |token: &Option<String>| {
            token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty())
        };
        let valid_access = present(&self.credentials.access_token)
            && self
                .credentials
                .expires_at
                .is_none_or(|expires_at| expires_at == 0 || now_ms < expires_at);
        // Frozen official switching keeps refreshable accounts eligible for the CLI.
        if self.requires_sign_in || (!valid_access && !present(&self.credentials.refresh_token)) {
            AccountStatus::Expired
        } else if self.retry_at_ms.is_some_and(|retry_at| now_ms < retry_at) {
            AccountStatus::RateLimited
        } else if current_id == Some(self.id.as_str()) {
            AccountStatus::Active
        } else {
            AccountStatus::NeedsActivation
        }
    }
}

fn masked_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) if !domain.is_empty() => {
            format!("{}•••@{domain}", local.chars().take(2).collect::<String>())
        }
        _ => "•••".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ACCESS: &str = "fixture-access-account-canary-7";
    const REFRESH: &str = "fixture-refresh-account-canary-9";

    fn store() -> AccountStore {
        AccountStore {
            current_id: Some("fixture-account".into()),
            accounts: vec![AccountRecord {
                id: "fixture-account".into(),
                name: "Fixture".into(),
                email: "sample@example.test".into(),
                plan: "Synthetic".into(),
                credentials: AccountCredentials {
                    access_token: Some(ACCESS.into()),
                    refresh_token: Some(REFRESH.into()),
                    expires_at: Some(200),
                },
                requires_sign_in: false,
                retry_at_ms: None,
            }],
        }
    }

    #[test]
    fn account_states_follow_selection_credential_and_cooldown() {
        use AccountStatus::*;
        let mut store = store();
        assert_eq!(store.project(100).accounts[0].status, Active);
        store.current_id = None;
        assert_eq!(store.project(100).accounts[0].status, NeedsActivation);
        store.current_id = Some("fixture-account".into());
        store.accounts[0].retry_at_ms = Some(150);
        let limited = store.project(149);
        assert_eq!(limited.accounts[0].status, RateLimited);
        assert_eq!(limited.current_id.as_deref(), Some("fixture-account"));
        assert_eq!(limited.accounts[0].retry_at_ms, Some(150));
        let recovered = store.project(150);
        assert_eq!(recovered.accounts[0].status, Active);
        assert_eq!(recovered.accounts[0].retry_at_ms, None);
        assert_eq!(store.project(200).accounts[0].status, Active);
        store.accounts[0].credentials.refresh_token = None;
        assert_eq!(store.project(199).accounts[0].status, Active);
        assert_eq!(store.project(200).accounts[0].status, Expired);
        store.accounts[0].credentials.expires_at = Some(0);
        assert_eq!(store.project(200).accounts[0].status, Active);
        store.accounts[0].requires_sign_in = true;
        assert_eq!(store.project(100).accounts[0].status, Expired);
        assert_eq!(store.project(100).accounts[0].retry_at_ms, None);
        store.accounts[0].requires_sign_in = false;
        store.accounts[0].credentials = AccountCredentials::default();
        assert_eq!(store.project(100).accounts[0].status, Expired);
    }

    #[test]
    fn projection_has_only_safe_fields_even_when_metadata_echoes_a_token() {
        let mut store = store();
        assert_eq!(store.project(100).accounts[0].email, "sa•••@example.test");
        store.current_id = Some(ACCESS.into());
        let account = &mut store.accounts[0];
        account.id = ACCESS.into();
        account.name = format!("Fixture {REFRESH}");
        account.email = format!("fixture@{ACCESS}.example.test");
        account.plan = REFRESH.into();
        let projection = store.project(100);
        let serialized = serde_json::to_string(&projection).unwrap();
        let debug = format!("{projection:?}");
        for forbidden in [
            ACCESS,
            REFRESH,
            "accessToken",
            "refreshToken",
            "claudeAiOauth",
        ] {
            assert!(!serialized.contains(forbidden));
            assert!(!debug.contains(forbidden));
        }
        let view = &projection.accounts[0];
        assert_eq!(projection.current_id.as_deref(), Some(view.id.as_str()));
        assert_eq!(view.name, "[redacted]");
        assert_eq!(view.email, "•••");
        assert_eq!(serde_json::to_value(view).unwrap()["status"], "active");
    }

    #[test]
    fn missing_and_corrupt_json_fall_back_without_replacing_bytes() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("accounts.json");
        let (loaded, state) = AccountStore::load(&path)?;
        assert_eq!(state, LoadState::Missing);
        assert!(loaded.project(0).accounts.is_empty());
        for bytes in [
            b"{broken".as_slice(),
            br#"{"currentId":"missing","accounts":[]}"#,
        ] {
            fs::write(&path, bytes)?;
            let (loaded, state) = AccountStore::load(&path)?;
            assert_eq!(state, LoadState::Corrupt);
            assert!(loaded.project(0).accounts.is_empty());
            assert_eq!(fs::read(&path)?, bytes);
        }
        Ok(())
    }

    #[test]
    fn subscription_projection_merges_exact_ids_and_uses_credential_health() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let credentials = root.path().join("model-router/credentials");
        fs::create_dir_all(&credentials)?;
        let mut legacy = store();
        legacy.accounts.push(AccountRecord {
            id: "another-legacy-account".into(),
            name: ACCESS.into(),
            email: "sample@example.test".into(),
            plan: "fixture-router-id-token".into(),
            credentials: AccountCredentials::default(),
            requires_sign_in: true,
            retry_at_ms: None,
        });
        let legacy_path = root.path().join("accounts.json");
        legacy.save(&legacy_path)?;
        let before = fs::read(&legacy_path)?;
        let config = json!({
            "version": 1,
            "activeProviderIds": {"claude-code": "fixture-account"},
            "providers": {"claude-code": [{
                "id": "fixture-account", "kind": "subscription", "name": REFRESH,
                "subscription": {
                    "provider": "codex", "credentialRef": {"name": "private-reference.json"},
                    "account": {"status": "ready", "email": "sample@example.test", "plan": "Plus"}
                }
            }]}
        });
        fs::write(root.path().join("provider-config.json"), config.to_string())?;
        let credential_path = credentials.join("private-reference.json");
        let mut credential = json!({
            "type": "codex",
            "email": "sample@example.test",
            "access_token": "fixture-router-access-token",
            "id_token": "fixture-router-id-token",
            "expired": "1970-01-01T01:00:00.200+01:00",
            "next_retry_after": "1970-01-01T00:00:00.150Z",
        });
        fs::write(&credential_path, credential.to_string())?;

        let (summary, state) = AccountStore::project_data_root(root.path(), 149)?;
        assert_eq!(state, LoadState::Loaded);
        assert_eq!(summary.accounts.len(), 2);
        assert_eq!(summary.current_id.as_deref(), Some("fixture-account"));
        let subscription = &summary.accounts[0];
        assert_eq!(subscription.provider, Some("codex"));
        assert_eq!(subscription.status, AccountStatus::RateLimited);
        assert_eq!(subscription.retry_at_ms, Some(150));
        assert_eq!(subscription.email, "sa•••@example.test");
        assert_eq!(subscription.plan, "Plus");
        assert_eq!(summary.accounts[1].plan, "[redacted]");
        let wire = serde_json::to_string(&summary)?;
        for secret in [
            ACCESS,
            REFRESH,
            "fixture-router-access-token",
            "fixture-router-id-token",
            "private-reference",
        ] {
            assert!(!wire.contains(secret));
        }
        let status_at = |now| -> io::Result<AccountStatus> {
            Ok(AccountStore::project_data_root(root.path(), now)?
                .0
                .accounts[0]
                .status)
        };
        assert_eq!(status_at(150)?, AccountStatus::Active);
        assert_eq!(status_at(200)?, AccountStatus::Expired);
        credential["refresh_token"] = json!("fixture-router-refresh-token");
        fs::write(&credential_path, credential.to_string())?;
        assert_eq!(status_at(200)?, AccountStatus::Active);
        credential["refresh_error"] = json!({"code": "invalid_grant"});
        fs::write(&credential_path, credential.to_string())?;
        assert_eq!(status_at(200)?, AccountStatus::Expired);
        assert_eq!(fs::read(legacy_path)?, before);
        Ok(())
    }

    #[test]
    fn subscription_projection_falls_back_when_one_source_is_corrupt() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let legacy = root.path().join("accounts.json");
        let config = root.path().join("provider-config.json");
        fs::write(&legacy, "{bad legacy JSON")?;
        fs::write(&config, "{bad provider JSON")?;
        let (summary, state) = AccountStore::project_data_root(root.path(), 0)?;
        assert_eq!(state, LoadState::Corrupt);
        assert!(summary.accounts.is_empty());

        let directory = root.path().join("model-router/credentials");
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("unreadable.json"), "{bad credential JSON")?;
        fs::write(
            directory.join("standalone.json"),
            json!({
                "type": "antigravity", "email": "sample@example.test",
                "access_token": "fixture-standalone-access", "plan": "Paid"
            })
            .to_string(),
        )?;
        let (summary, state) = AccountStore::project_data_root(root.path(), 0)?;
        assert_eq!(state, LoadState::Loaded);
        assert_eq!(summary.current_id, None);
        assert_eq!(summary.accounts.len(), 1);
        assert_eq!(summary.accounts[0].status, AccountStatus::Active);
        assert_eq!(summary.accounts[0].provider, Some("antigravity"));
        assert_eq!(summary.accounts[0].plan, "Paid");
        assert!(!serde_json::to_string(&summary)?.contains("standalone"));
        assert_eq!(fs::read_to_string(&legacy)?, "{bad legacy JSON");
        assert_eq!(fs::read_to_string(&config)?, "{bad provider JSON");

        store().save(&legacy)?;
        let (summary, state) = AccountStore::project_data_root(root.path(), 0)?;
        assert_eq!(state, LoadState::Loaded);
        assert_eq!(summary.accounts.len(), 2);
        assert_eq!(summary.current_id.as_deref(), Some("fixture-account"));
        Ok(())
    }

    #[test]
    fn fixture_save_replaces_complete_snapshot_and_cleans_staging_on_failure() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("accounts.json");
        let mut store = store();
        store.save(&path)?;
        #[cfg(unix)]
        let mut original_file = fs::File::open(&path)?;
        let original = fs::read(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        }
        store.accounts[0].name = "New fixture name".into();
        store.save(&path)?;
        let (loaded, state) = AccountStore::load(&path)?;
        assert_eq!(state, LoadState::Loaded);
        assert_eq!(loaded.project(100), store.project(100));
        assert!(loaded.accounts[0].credentials.access_token.as_deref() == Some(ACCESS));
        assert!(loaded.accounts[0].credentials.refresh_token.as_deref() == Some(REFRESH));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
            let mut previous = Vec::new();
            original_file.read_to_end(&mut previous)?;
            assert!(previous == original);
        }
        assert!(fs::read(&path)? != original);
        let blocked = directory.path().join("blocked.json");
        fs::create_dir(&blocked)?;
        assert!(store.save(&blocked).is_err());
        assert!(blocked.is_dir());
        assert_eq!(fs::read_dir(directory.path())?.count(), 2);
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn fixture_load_does_not_follow_symlinks_and_save_replaces_the_link() -> io::Result<()> {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("target.json");
        fs::write(&target, b"untouched")?;
        let path = directory.path().join("accounts.json");
        symlink(&target, &path)?;
        assert!(AccountStore::load(&path).is_err());
        store().save(&path)?;
        assert_eq!(AccountStore::load(&path)?.1, LoadState::Loaded);
        assert_eq!(fs::read(&target)?, b"untouched");
        Ok(())
    }
}
