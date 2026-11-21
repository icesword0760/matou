//! Offline account snapshots. The caller supplies the fixture path and clock.

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};
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
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Self::default(), LoadState::Missing));
            }
            Err(error) => return Err(error),
        };
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Account snapshot must be a regular file",
            ));
        }
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
        let secrets: Vec<_> = self
            .accounts
            .iter()
            .flat_map(|account| {
                [
                    account.credentials.access_token.as_deref(),
                    account.credentials.refresh_token.as_deref(),
                ]
                .into_iter()
                .flatten()
                .filter(|token| !token.is_empty())
            })
            .collect();
        let safe = |value: &str| {
            if secrets.iter().any(|token| value.contains(token)) {
                "[redacted]".to_owned()
            } else {
                value.to_owned()
            }
        };
        AccountProjection {
            current_id: self.current_id.as_deref().map(safe),
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
                        status,
                        retry_at_ms: account
                            .retry_at_ms
                            .filter(|_| status == AccountStatus::RateLimited),
                    }
                })
                .collect(),
        }
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
