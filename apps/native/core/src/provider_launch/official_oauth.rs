use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::pty::get_var;

#[cfg(target_os = "macos")]
const KEYCHAIN_OUTPUT_LIMIT: u64 = 1024 * 1024;
const IDENTITY_CHANGED: &str = "Official Claude account identity changed";
const SIGN_IN_REQUIRED: &str = "Sign in to the official Claude account before using this target";

/// Private launch configuration. Only the account identity is pinned; tokens
/// stored on disk can refresh during a run.
#[derive(Clone)]
pub struct OfficialClaudeOAuth {
    identity: String,
    environment: OfficialClaudeEnvironment,
    pub provider_name: String,
}

#[derive(Clone)]
pub(super) struct OfficialClaudeEnvironment {
    config_dir: Option<PathBuf>,
    claude_json_path: Option<PathBuf>,
    captured_token: Option<String>,
    #[cfg(target_os = "macos")]
    keychain: Option<(Option<OsString>, String)>,
}

impl OfficialClaudeEnvironment {
    pub(super) fn new(env: &BTreeMap<OsString, OsString>) -> Self {
        let get = |key| get_var(env, key).filter(|value| !value.is_empty());
        let explicit_dir = get("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let home = get("HOME")
            .or_else(|| {
                if cfg!(windows) {
                    get("USERPROFILE")
                } else {
                    None
                }
            })
            .map(PathBuf::from);
        Self {
            claude_json_path: explicit_dir
                .as_ref()
                .or(home.as_ref())
                .map(|dir| dir.join(".claude.json")),
            #[cfg(target_os = "macos")]
            keychain: get("MATOU_CLAUDE_DISABLE_KEYCHAIN")
                .is_none_or(|value| value != "1")
                .then(|| {
                    use sha2::{Digest, Sha256};
                    let user = get("USER")
                        .or_else(|| get("LOGNAME"))
                        .map(ToOwned::to_owned);
                    let mut service = "Claude Code-credentials".to_owned();
                    if let Some(dir) =
                        get("CLAUDE_SECURESTORAGE_CONFIG_DIR").or_else(|| get("CLAUDE_CONFIG_DIR"))
                    {
                        let hash = format!("{:x}", Sha256::digest(dir.as_encoded_bytes()));
                        service.push('-');
                        service.push_str(&hash[..8]);
                    }
                    (user, service)
                }),
            config_dir: explicit_dir.or_else(|| home.map(|dir| dir.join(".claude"))),
            captured_token: get("CLAUDE_CODE_OAUTH_TOKEN")
                .and_then(|token| token.to_str())
                .map(str::to_owned),
        }
    }

    pub(super) fn capture(&self, provider_name: &str) -> Result<OfficialClaudeOAuth> {
        let identity = self.read_identity().context(
            "Official Claude account identity is unavailable in local account configuration",
        )?;
        Ok(OfficialClaudeOAuth {
            identity,
            environment: self.clone(),
            provider_name: provider_name.to_owned(),
        })
    }

    fn read_identity(&self) -> Option<String> {
        let local = read_json(self.claude_json_path.as_deref()?)?;
        let account = &local["oauthAccount"];
        let id = account["accountUuid"]
            .as_str()
            .filter(|id| !id.is_empty())
            .or_else(|| account["emailAddress"].as_str().filter(|id| !id.is_empty()))?;
        let organization = account
            .get("organizationUuid")
            .filter(|value| !value.is_null());
        let empty = json!("");
        Some(json!([id, organization.unwrap_or(&empty)]).to_string())
    }

    fn read_token(&self) -> Option<String> {
        if let Some(token) = &self.captured_token {
            return Some(token.clone());
        }
        #[cfg(target_os = "macos")]
        if let Some((user, service)) = &self.keychain
            && let Some(token) =
                keychain_credentials(user.as_deref(), service).and_then(oauth_token)
        {
            return Some(token);
        }
        let config_dir = self.config_dir.as_ref()?;
        read_json(&config_dir.join(".credentials.json")).and_then(oauth_token)
    }
}

impl OfficialClaudeOAuth {
    fn check_identity(&self) -> Result<()> {
        ensure!(
            self.environment.read_identity().as_deref() == Some(&self.identity),
            IDENTITY_CHANGED
        );
        Ok(())
    }

    /// Re-read the stable identity on both sides of the credential lookup.
    pub fn read_token(&self) -> Result<String> {
        self.check_identity()?;
        let token = self.environment.read_token();
        self.check_identity()?;
        token.context(SIGN_IN_REQUIRED)
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_reader(BufReader::new(File::open(path).ok()?)).ok()
}

#[cfg(target_os = "macos")]
fn bounded_keychain_json(reader: impl std::io::Read) -> Option<Value> {
    use std::io::Read;

    let mut raw = Vec::new();
    reader
        .take(KEYCHAIN_OUTPUT_LIMIT + 1)
        .read_to_end(&mut raw)
        .ok()?;
    if raw.len() as u64 > KEYCHAIN_OUTPUT_LIMIT {
        return None;
    }
    serde_json::from_slice(&raw).ok()
}

fn oauth_token(credentials: Value) -> Option<String> {
    let oauth = &credentials["claudeAiOauth"];
    if !matches!(oauth["expiresAt"], Value::Null | Value::Bool(false)) {
        let expiry = oauth["expiresAt"].as_f64()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis() as f64;
        if expiry != 0.0 && expiry <= now {
            return None;
        }
    }
    oauth["accessToken"]
        .as_str()
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

#[cfg(target_os = "macos")]
fn keychain_credentials(user: Option<&std::ffi::OsStr>, service: &str) -> Option<Value> {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    let user = user.map(ToOwned::to_owned).or_else(current_mac_user)?;
    let mut child = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-a"])
        .arg(user)
        .args(["-s", service, "-w"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take().expect("piped Keychain stdout");
    let reader = thread::spawn(|| bounded_keychain_json(stdout));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                return status
                    .success()
                    .then(|| reader.join().ok().flatten())
                    .flatten();
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[cfg(target_os = "macos")]
fn current_mac_user() -> Option<OsString> {
    use std::{ffi::CStr, mem::MaybeUninit, os::unix::ffi::OsStrExt, ptr};

    let mut record = MaybeUninit::<libc::passwd>::uninit();
    let mut result = ptr::null_mut();
    let mut buffer = [0u8; 16 * 1024];
    // getpwuid_r fills both record and its backing buffer on success.
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return None;
    }
    let name = unsafe { (*result).pw_name };
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    Some(std::ffi::OsStr::from_bytes(name).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_account_uses_home_identity_and_pins_environment_token() -> Result<()> {
        let root = tempfile::tempdir()?;
        let home = root.path();
        std::fs::create_dir(home.join(".claude"))?;
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"fixture@local","organizationUuid":null}}"#,
        )?;
        std::fs::write(
            home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"fixture-file-token"}}"#,
        )?;
        let env = BTreeMap::from([
            ("HOME".into(), home.as_os_str().into()),
            ("CLAUDE_CODE_OAUTH_TOKEN".into(), "fixture-env-token".into()),
            ("MATOU_CLAUDE_DISABLE_KEYCHAIN".into(), "1".into()),
        ]);
        let target = OfficialClaudeEnvironment::new(&env).capture("Fixture")?;
        assert_eq!(target.identity, r#"["fixture@local",""]"#);
        assert_eq!(target.read_token()?, "fixture-env-token");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"fixture@local","organizationUuid":"other"}}"#,
        )?;
        assert_eq!(
            target.read_token().unwrap_err().to_string(),
            IDENTITY_CHANGED
        );
        Ok(())
    }
}
