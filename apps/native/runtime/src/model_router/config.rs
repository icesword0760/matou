use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt::Write as _,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::VERSION;
use crate::{egress_probe, subscription_launch::ModelAlias};

#[derive(Deserialize, Serialize)]
pub(super) struct Keys {
    pub client: String,
    pub management: String,
}

pub(super) struct Directory {
    pub path: PathBuf,
    pub keys: Keys,
}

pub(super) fn prepare(root: &Path) -> Result<Directory> {
    let root = root
        .canonicalize()
        .context("Resolve runtime data directory")?;
    let path = private_dir(&root, "model-router")?;
    let credentials = private_dir(&path, "credentials")?;
    for entry in fs::read_dir(&credentials)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            private_file(&credentials, &entry.path())?;
        }
    }
    private_dir(&path, "logs")?;
    let file = path.join("keys.json");
    let keys = match fs::symlink_metadata(&file) {
        Ok(_) => {
            let file = private_file(&path, &file)?;
            let bytes = fs::read(file).context("Read model router keys")?;
            let keys: Keys =
                serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid model router keys"))?;
            ensure!(
                valid_key(&keys.client)
                    && valid_key(&keys.management)
                    && keys.client != keys.management,
                "Invalid model router keys"
            );
            keys
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let keys = Keys {
                client: random_key()?,
                management: random_key()?,
            };
            atomic_json(&path, &file, &keys)?;
            keys
        }
        Err(_) => return Err(anyhow!("Read model router keys")),
    };
    Ok(Directory { path, keys })
}

fn valid_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn random_key() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| anyhow!("Generate model router key"))?;
    let mut key = String::with_capacity(64);
    for byte in bytes {
        write!(key, "{byte:02x}").expect("writing to String");
    }
    Ok(key)
}

fn private_dir(root: &Path, name: &str) -> Result<PathBuf> {
    let path = root.join(name);
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    if let Err(error) = builder.create(&path) {
        ensure!(
            error.kind() == io::ErrorKind::AlreadyExists,
            "Create private model router directory"
        );
    }
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_dir() && !is_link(&metadata),
        "Invalid model router directory"
    );
    let canonical = path.canonicalize()?;
    ensure!(
        canonical.parent() == Some(root),
        "Invalid model router directory"
    );
    set_mode(&canonical, 0o700)?;
    Ok(canonical)
}

pub(crate) fn credential_path(root: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        name.strip_suffix(".json")
            .is_some_and(|stem| !stem.is_empty())
            && !name.contains(['/', '\\', ':'])
            && !name.chars().any(char::is_control),
        "Invalid subscription credential reference"
    );
    let root = root.canonicalize()?;
    let directory = private_dir(&root, "model-router")?;
    let credentials = private_dir(&directory, "credentials")?;
    private_file(&credentials, &credentials.join(name))
}

pub(super) fn private_file(root: &Path, path: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !is_link(&metadata),
        "Invalid private model router file"
    );
    let canonical = path.canonicalize()?;
    ensure!(
        canonical.parent() == Some(root),
        "Invalid private model router file"
    );
    set_mode(&canonical, 0o600)?;
    Ok(canonical)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT includes directory junctions.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn atomic_json(directory: &Path, path: &Path, value: &impl Serialize) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            private_file(directory, path)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    set_mode(temporary.path(), 0o600)?;
    serde_json::to_writer(&mut temporary, value)
        .map_err(|_| anyhow!("Write model router configuration"))?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

impl Directory {
    pub fn write_config(
        &self,
        port: u16,
        proxy: Option<&str>,
        aliases: &BTreeMap<&str, Vec<ModelAlias>>,
    ) -> Result<()> {
        let mut value = json!({
            "config-version": 8,
            // Request logging in v8 can otherwise force an error log even
            // when request-log is false. Never record local credentials.
            "server": { "host": "127.0.0.1", "port": port, "shutdown-timeout": 1, "commercial-mode": true, "discovery": { "enabled": false } },
            "management": { "allow-remote": false, "secret-key": self.keys.management, "disable-control-panel": true, "disable-auto-update-panel": true },
            "access": { "api-keys": [self.keys.client] },
            "oauth": {
                "auth-dir": self.path.join("credentials"),
                "providers": {
                    "antigravity": { "antigravity-credits": false, "disable-cooling": true },
                    "codex": { "header-defaults": { "user-agent": codex_user_agent() } }
                }
            },
            "antigravity": { "connection-pool": { "enabled": true, "idle-conn-timeout": "30s", "max-idle-conns-per-host": 2 } },
            "client": { "claude": { "show-codex-commentary": true } },
            "multimedia": { "disable-image-generation": "passthrough" },
            "observability": { "logs": { "debug": false, "logging-to-file": false, "request-log": false, "error-logs-max-files": 10 }, "usage": { "usage-statistics-enabled": false }, "pprof": { "enable": false } },
            "plugins": { "enabled": false },
            "routing": {
                "force-model-prefix": true,
                "retry": { "request-retry": 0, "max-retry-credentials": 1, "max-retry-interval": 0 },
                "cooldown": { "disable-cooling": false, "save-cooldown-status": false, "transient-error-cooldown-seconds": -1 }
            }
        });
        if let Some(proxy) = proxy {
            value["requests"] = json!({"proxy-url": proxy});
        }
        if !aliases.is_empty() {
            value["oauth"]["model-alias"] = json!(aliases);
        }
        atomic_json(&self.path, &self.path.join("config.json"), &value)
    }
}

pub(super) fn codex_user_agent() -> &'static str {
    use std::sync::LazyLock;
    static USER_AGENT: LazyLock<String> = LazyLock::new(|| {
        let platform = if cfg!(target_os = "macos") {
            let version = macos_version().unwrap_or("15.0.0");
            format!("Mac OS {version}")
        } else if cfg!(windows) {
            "Windows NT 10.0".into()
        } else {
            "Linux".into()
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            std::env::consts::ARCH
        };
        format!("codex-tui/0.162.1 ({platform}; {arch}) iTerm.app/3.6.11 (codex-tui; 0.162.1)")
    });
    &USER_AGENT
}

pub(crate) fn macos_version() -> Option<&'static str> {
    use std::sync::LazyLock;
    static VERSION: LazyLock<Option<String>> = LazyLock::new(|| {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let output = std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        let value = std::str::from_utf8(&output.stdout).ok()?.trim();
        (output.status.success()
            && !value.is_empty()
            && value
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
        .then(|| value.to_owned())
    });
    VERSION.as_deref()
}

pub(super) fn runtime_binary() -> Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let name = if cfg!(windows) {
        "matou-native-runtime.exe"
    } else {
        "matou-native-runtime"
    };
    if executable.file_name().is_some_and(|file| file == name) {
        return Ok(executable);
    }
    if cfg!(debug_assertions) {
        let executable = executable.parent().context("Resolve runtime executable")?;
        for sibling in [executable.join(name), executable.join("..").join(name)] {
            if sibling.is_file() {
                return Ok(sibling.canonicalize()?);
            }
        }
    }
    Err(anyhow!("Model router watchdog executable is unavailable"))
}

pub(super) fn router_binary(override_path: Option<OsString>) -> Result<PathBuf> {
    if cfg!(debug_assertions)
        && let Some(path) = override_path
    {
        let path = PathBuf::from(path);
        ensure!(
            path.is_absolute() && path.is_file(),
            "Model router development binary is unavailable"
        );
        return path
            .canonicalize()
            .context("Resolve model router development binary");
    }
    let executable = runtime_binary()?;
    let directory = executable
        .parent()
        .context("Resolve model router resources")?;
    let name = if cfg!(windows) {
        "model-router.exe"
    } else {
        "model-router"
    };
    let resources = if cfg!(target_os = "macos") {
        "../Resources"
    } else {
        "resources"
    };
    let bundled = directory.join(resources).join("model-router").join(name);
    if bundled.is_file() {
        let binary = bundled.canonicalize()?;
        let root = directory.join(resources).canonicalize()?;
        ensure!(
            binary.starts_with(root),
            "Invalid model router resource path"
        );
        return Ok(binary);
    }
    if cfg!(debug_assertions) {
        let platform = if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(windows) {
            "win32"
        } else {
            "linux"
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let cached = repo
            .join(".cache/model-router")
            .join(VERSION)
            .join(format!("{platform}-{arch}"))
            .join(name);
        if cached.is_file() {
            return cached
                .canonicalize()
                .context("Resolve cached model router binary");
        }
    }
    Err(anyhow!("Model router binary is unavailable"))
}

pub(super) fn proxy(environment: &BTreeMap<OsString, OsString>) -> Result<Option<String>> {
    let get = |key: &str| {
        matou_native_core::pty::get_var(environment, key)
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    if get("NO_PROXY")
        .or_else(|| get("no_proxy"))
        .is_some_and(|no_proxy| no_proxy.split(',').any(bypasses_provider))
    {
        return Ok(None);
    }
    if let Some(raw) = [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .into_iter()
    .find_map(get)
    {
        return validate_proxy(raw).map(Some);
    }
    egress_probe::read_system_proxy(Instant::now() + Duration::from_secs(2))
        .as_deref()
        .map(validate_proxy)
        .transpose()
}

fn bypasses_provider(rule: &str) -> bool {
    let rule = rule.trim().to_ascii_lowercase();
    if rule == "*" {
        return true;
    }
    let domain = match rule.split_once(':') {
        Some((name, "443")) => name,
        Some(_) => return false,
        None => &rule,
    };
    let suffix = domain
        .strip_prefix("*.")
        .or_else(|| domain.strip_prefix('.'));
    let domain = suffix.unwrap_or(domain);
    ["chatgpt.com", "auth.openai.com", "accounts.google.com"]
        .into_iter()
        .any(|host| {
            (suffix.is_none() && host == domain)
                || host
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
        || domain == "googleapis.com"
        || domain.ends_with(".googleapis.com")
}

fn validate_proxy(raw: &str) -> Result<String> {
    let value = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    ensure!(
        url::Url::parse(&value).is_ok_and(|url| matches!(
            url.scheme(),
            "http" | "https" | "socks5" | "socks5h"
        ) && url
            .host_str()
            .is_some_and(|host| !host.is_empty())
            && url.port() != Some(0)),
        "Network proxy configuration is invalid"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_proxy_preserves_priority_and_single_network_bypass() -> Result<()> {
        let mut environment = BTreeMap::from([
            ("HTTP_PROXY".into(), "http://http-proxy.invalid:80".into()),
            (
                "ALL_PROXY".into(),
                "socks5h://socks-proxy.invalid:1080".into(),
            ),
            ("https_proxy".into(), "http://lower.invalid:80".into()),
            (
                "HTTPS_PROXY".into(),
                "https://user:fixture-secret@upper.invalid:8443".into(),
            ),
            (
                "NO_PROXY".into(),
                "chatgpt.com:80,*.chatgpt.com,example.invalid".into(),
            ),
        ]);
        assert_eq!(
            proxy(&environment)?.as_deref(),
            Some("https://user:fixture-secret@upper.invalid:8443")
        );
        environment.remove(std::ffi::OsStr::new("HTTPS_PROXY"));
        assert_eq!(
            proxy(&environment)?.as_deref(),
            Some("http://lower.invalid:80")
        );
        environment.remove(std::ffi::OsStr::new("https_proxy"));
        assert_eq!(
            proxy(&environment)?.as_deref(),
            Some("socks5h://socks-proxy.invalid:1080")
        );
        environment.insert("NO_PROXY".into(), "*.googleapis.com:443".into());
        assert_eq!(proxy(&environment)?, None);
        let error = validate_proxy("ftp://user:fixture-secret@bad.invalid").unwrap_err();
        assert!(!error.to_string().contains("fixture-secret"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn router_rejects_linked_directories_keys_and_credentials() -> Result<()> {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir()?;
        let external = tempfile::tempdir()?;
        let directory = root.path().join("model-router");
        symlink(external.path(), &directory)?;
        assert!(prepare(root.path()).is_err());
        fs::remove_file(&directory)?;
        fs::create_dir(&directory)?;
        let outside_key = external.path().join("keys.json");
        fs::write(&outside_key, "outside")?;
        symlink(&outside_key, directory.join("keys.json"))?;
        assert!(prepare(root.path()).is_err());
        assert_eq!(fs::read_to_string(&outside_key)?, "outside");
        fs::remove_file(directory.join("keys.json"))?;
        symlink(&outside_key, directory.join("credentials/outside.json"))?;
        assert!(prepare(root.path()).is_err());
        assert!(credential_path(root.path(), "outside.json").is_err());
        Ok(())
    }
}
