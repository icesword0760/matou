use std::{
    cmp::Ordering,
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fmt::Write as _,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{LazyLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use matou_native_core::pty::get_var;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::VERSION;
use crate::{codex_launch, egress_probe, subscription_launch::ModelAlias};

const CODEX_FALLBACK_VERSION: &str = "0.162.1";
const CODEX_VERSION_TIMEOUT: Duration = Duration::from_millis(1_400);
const CODEX_VERSION_LIMIT: u64 = 16 * 1024;

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
        codex: &CodexClientIdentity,
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
                    "codex": { "header-defaults": { "user-agent": codex.user_agent } }
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

pub(super) struct CodexClientIdentity {
    pub version: String,
    pub user_agent: String,
}

impl CodexClientIdentity {
    pub fn detect(environment: &BTreeMap<OsString, OsString>) -> Self {
        let output = detect_codex_version(environment);
        Self::from_probe(environment, output.as_deref(), macos_version())
    }

    pub(super) fn from_probe(
        environment: &BTreeMap<OsString, OsString>,
        output: Option<&str>,
        macos_product_version: Option<&str>,
    ) -> Self {
        let version = output
            .and_then(parse_semver)
            .filter(|version| compare_semver(version, CODEX_FALLBACK_VERSION).is_gt())
            .unwrap_or(CODEX_FALLBACK_VERSION)
            .to_owned();
        let platform = if cfg!(target_os = "macos") {
            let version = macos_product_version.unwrap_or("15.0.0");
            format!("Mac OS {version}")
        } else if cfg!(windows) {
            "Windows NT 10.0".into()
        } else {
            "Linux".into()
        };
        let terminal = get_var(environment, "TERM_PROGRAM")
            .filter(|name| !name.is_empty())
            .map(|name| {
                let terminal_version =
                    get_var(environment, "TERM_PROGRAM_VERSION").unwrap_or(OsStr::new("1.0"));
                format!(
                    "{}/{}",
                    name.to_string_lossy(),
                    terminal_version.to_string_lossy()
                )
            })
            .unwrap_or_else(|| "iTerm.app/3.6.11".into());
        let arch = client_arch();
        let user_agent =
            format!("codex-tui/{version} ({platform}; {arch}) {terminal} (codex-tui; {version})");
        Self {
            version,
            user_agent,
        }
    }
}

pub(super) fn client_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        arch => arch,
    }
}

pub(super) fn parse_semver(raw: &str) -> Option<&str> {
    static SEMVER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?-u:\b[0-9]+\.[0-9]+\.[0-9]+\b)").unwrap());
    SEMVER.find(raw).map(|version| version.as_str())
}

pub(super) fn compare_semver(left: &str, right: &str) -> Ordering {
    left.split('.')
        .zip(right.split('.'))
        .map(|(left, right)| {
            let left = left.trim_start_matches('0');
            let right = right.trim_start_matches('0');
            left.len().cmp(&right.len()).then_with(|| left.cmp(right))
        })
        .find(|order| !order.is_eq())
        .unwrap_or(Ordering::Equal)
}

fn detect_codex_version(environment: &BTreeMap<OsString, OsString>) -> Option<String> {
    let command = get_var(environment, "MATOU_CODEX_COMMAND")
        .filter(|value| !value.is_empty())
        .unwrap_or(OsStr::new("codex"));
    let path = Path::new(command);
    let cwd = std::env::current_dir().ok()?;
    let deadline = Instant::now() + CODEX_VERSION_TIMEOUT;
    let (executable, login_path): (_, Option<OsString>) =
        match codex_launch::resolve_executable(path, &cwd, environment) {
            Some(executable) => (executable, None),
            None => {
                #[cfg(unix)]
                {
                    if path.file_name() != Some(command) {
                        return None;
                    }
                    let login_path = codex_login_path(environment, deadline)?;
                    let mut login_environment = environment.clone();
                    login_environment.insert("PATH".into(), login_path.clone());
                    let executable =
                        codex_launch::resolve_executable(path, &cwd, &login_environment)?;
                    (executable, Some(login_path))
                }
                #[cfg(not(unix))]
                return None;
            }
        };
    let mut command = Command::new(executable);
    command.arg("--version").env_clear().envs(environment);
    if let Some(login_path) = login_path {
        command.env("PATH", login_path);
    }
    probe_output(&mut command, deadline, CODEX_VERSION_LIMIT)
}

#[cfg(unix)]
fn codex_login_path(
    environment: &BTreeMap<OsString, OsString>,
    deadline: Instant,
) -> Option<OsString> {
    if get_var(environment, "MATOU_DISABLE_LOGIN_PATH_PROBE") == Some(OsStr::new("1")) {
        return None;
    }
    let home = get_var(environment, "HOME").filter(|home| !home.is_empty())?;
    let shell = get_var(environment, "SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or(OsStr::new("/bin/zsh"));
    let mut command = Command::new(shell);
    command
        .args(["-ilc", "printf '__MATOU_LOGIN_PATH__=%s\\n' \"$PATH\""])
        .current_dir(home)
        .env_clear()
        .envs(environment);
    let output = probe_output(&mut command, deadline, 256 * 1024)?;
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("__MATOU_LOGIN_PATH__="))
        .filter(|path| !path.is_empty())
        .map(OsString::from)
}

fn probe_output(command: &mut Command, deadline: Instant, limit: u64) -> Option<String> {
    if Instant::now() >= deadline {
        return None;
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().ok()?;
    let (send, receive) = mpsc::channel();
    let stdout = child
        .stdout
        .take()
        .expect("version probe uses piped stdout");
    let reader = thread::Builder::new()
        .name("codex-probe-output".into())
        .spawn(move || {
            let mut output = Vec::new();
            let result = stdout.take(limit + 1).read_to_end(&mut output);
            let _ = send.send(
                result
                    .ok()
                    .filter(|&size| size as u64 <= limit)
                    .and_then(|_| String::from_utf8(output).ok()),
            );
        });
    if reader.is_ok() {
        let output = receive.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        if let Ok(Some(output)) = output {
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(status)) => return status.success().then_some(output),
                    Ok(None) => thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
        }
    }
    #[cfg(unix)]
    // SAFETY: the probe was placed in its own process group.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
    None
}

pub(crate) fn macos_version() -> Option<&'static str> {
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
        for directory in executable.ancestors().skip(1).take(5) {
            let sibling = directory.join(name);
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
    #[cfg(target_os = "macos")]
    if directory.file_name().is_some_and(|name| name == "MacOS") {
        let bundled = directory.join(name);
        if bundled.is_file() {
            let binary = bundled.canonicalize()?;
            ensure!(
                binary.parent() == Some(directory.canonicalize()?.as_path()),
                "Invalid model router executable path"
            );
            return Ok(binary);
        }
    }
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
    fn codex_identity_uses_first_semver_above_the_fallback() {
        assert_eq!(parse_semver("codex-cli 0.198.0 (1.2.3)"), Some("0.198.0"));
        assert_eq!(parse_semver("x0.198.0y 0.198"), None);
        assert_eq!(parse_semver("codex 0.198.0-beta.1"), Some("0.198.0"));
        assert!(compare_semver("0.198.0", "0.162.1").is_gt());
        assert!(compare_semver("0.162.0", "0.162.1").is_lt());
        assert!(compare_semver("0.0162.01", "0.162.1").is_eq());
        assert!(compare_semver("1.0.0", "0.999.999").is_gt());

        let environment = BTreeMap::from([("TERM_PROGRAM".into(), "FixtureTerminal".into())]);
        let identity =
            CodexClientIdentity::from_probe(&environment, Some("codex-cli 0.198.0"), Some("15.6"));
        assert_eq!(identity.version, "0.198.0");
        assert!(identity.user_agent.contains(" FixtureTerminal/1.0 "));
        for probe in [None, Some("unknown"), Some("0.150.0"), Some("0.162.1")] {
            let identity = CodexClientIdentity::from_probe(&BTreeMap::new(), probe, None);
            assert_eq!(identity.version, CODEX_FALLBACK_VERSION);
            assert!(identity.user_agent.contains(" iTerm.app/3.6.11 "));
        }
    }

    #[cfg(unix)]
    #[test]
    fn codex_version_probe_uses_launch_path_and_command_override() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir()?;
        for (command, version) in [("codex", "0.198.0"), ("fixture-codex", "0.201.0")] {
            let executable = directory.path().join(command);
            fs::write(
                &executable,
                format!(
                    "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\nprintf 'codex-cli {version}\\n'\n"
                ),
            )?;
            fs::set_permissions(executable, fs::Permissions::from_mode(0o700))?;
        }
        let mut environment = BTreeMap::from([("PATH".into(), directory.path().into())]);
        assert_eq!(CodexClientIdentity::detect(&environment).version, "0.198.0");
        environment.insert("MATOU_CODEX_COMMAND".into(), "fixture-codex".into());
        assert_eq!(CodexClientIdentity::detect(&environment).version, "0.201.0");

        let shell = directory.path().join("fixture-shell");
        fs::write(
            &shell,
            "#!/bin/sh\n[ \"$1\" = -ilc ] || exit 1\nprintf 'startup output\\n__MATOU_LOGIN_PATH__=%s\\n' \"$HOME\"\n",
        )?;
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
        environment.insert("PATH".into(), "/usr/bin:/bin".into());
        environment.insert("HOME".into(), directory.path().into());
        environment.insert("SHELL".into(), shell.into());
        assert_eq!(CodexClientIdentity::detect(&environment).version, "0.201.0");
        Ok(())
    }

    #[test]
    fn missing_codex_falls_back_within_the_version_probe_budget() -> Result<()> {
        let empty_path = tempfile::tempdir()?;
        let environment = BTreeMap::from([("PATH".into(), empty_path.path().into())]);
        let started = Instant::now();
        let identity = CodexClientIdentity::detect(&environment);
        assert_eq!(identity.version, CODEX_FALLBACK_VERSION);
        assert!(started.elapsed() < Duration::from_millis(1_500));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_stops_waiting_for_a_hung_child() {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let started = Instant::now();
        assert_eq!(
            probe_output(
                &mut command,
                started + Duration::from_millis(50),
                CODEX_VERSION_LIMIT
            ),
            None
        );
        assert!(started.elapsed() < Duration::from_millis(1_500));
    }

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
