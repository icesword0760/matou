//! Local Claude account roster and presentation preferences. The bundled bridge
//! owns credential storage, sign-in and default-account transactions.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[cfg(unix)]
use std::{
    ffi::{OsStr, OsString},
    io::{Seek, SeekFrom},
    os::unix::{ffi::OsStringExt, process::CommandExt},
    sync::OnceLock,
};

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ClaudeAccountCommand {
    List,
    RefreshUsage,
    Switch { id: String, remember_scope: bool },
    Rename { id: String, name: String },
    Remove { id: String },
    Login { replace_id: Option<String> },
    SaveDraft { name: String },
    SaveDetected { name: String },
    Reauth { id: String },
    SubmitLoginCode { attempt_id: String, code: String },
    CancelLogin,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub used: f64,
    pub resets_at: Option<String>,
}

impl UsageWindow {
    pub fn reset_timestamp(&self) -> Option<u64> {
        let timestamp =
            crate::agent_hud_projection::timestamp_ms(&json!(self.resets_at.as_ref()?))?;
        (timestamp >= 0.).then_some(timestamp as u64)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    pub checked_at: u64,
    pub stale: bool,
    pub five_hour: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAccount {
    pub id: String,
    pub name: String,
    pub email: String,
    pub needs_login: bool,
    #[serde(default)]
    pub usage: Option<AccountUsage>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Roster {
    accounts: Vec<ClaudeAccount>,
    current_id: Option<String>,
    detected: Option<ClaudeAccount>,
    engine_version: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Preferences {
    names: BTreeMap<String, String>,
    remember_scope: bool,
    #[serde(default)]
    renewed_ids: Vec<String>,
    #[serde(default, deserialize_with = "read_usage")]
    usage: BTreeMap<String, AccountUsage>,
}

fn read_usage<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, AccountUsage>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, usage)| {
            let usage = serde_json::from_value::<AccountUsage>(usage.clone()).ok()?;
            let valid = [usage.five_hour.as_ref(), usage.weekly.as_ref()]
                .into_iter()
                .flatten()
                .all(|window| (0. ..=100.).contains(&window.used));
            valid.then(|| (id.clone(), usage))
        })
        .collect())
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAccountsState {
    pub accounts: Vec<ClaudeAccount>,
    pub current_id: Option<String>,
    pub detected: Option<ClaudeAccount>,
    pub engine_version: Option<String>,
    pub remember_scope: bool,
    pub renewed_ids: Vec<String>,
    pub draft: Option<ClaudeAccount>,
    pub reauth_id: Option<String>,
    pub login_attempt_id: Option<String>,
    pub login_code_submitted: bool,
    pub error: Option<String>,
    pub switched: bool,
}

type Fault = &'static str;
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_OUTPUT: usize = 2 * 1024 * 1024;

struct Engine {
    executable: PathBuf,
    script: Option<PathBuf>,
    resolve_login_path: bool,
    #[cfg(unix)]
    login_path: OnceLock<Option<OsString>>,
}

impl Engine {
    fn new(executable: PathBuf, script: Option<PathBuf>, resolve_login_path: bool) -> Self {
        Self {
            executable,
            script,
            resolve_login_path,
            #[cfg(unix)]
            login_path: OnceLock::new(),
        }
    }

    fn resolve(allow_default: bool, fixture: Option<PathBuf>) -> Option<Self> {
        if let Some(executable) = fixture {
            return Some(Self::new(executable, None, false));
        }
        if !allow_default {
            return None;
        }
        if let Some(path) = std::env::var_os("MATOU_CLAUDE_ACCOUNTS_ENGINE") {
            return Some(Self::new(
                path.into(),
                std::env::var_os("MATOU_CLAUDE_ACCOUNTS_BRIDGE").map(PathBuf::from),
                false,
            ));
        }
        if std::env::var_os("MATOU_DAG_WINDOW_PROBE").is_some()
            || std::env::var_os("MATOU_WINDOW_REGISTRY_PROBE").is_some()
        {
            return None;
        }
        let executable = std::env::current_exe().ok()?;
        let bin = executable.parent()?;
        #[cfg(windows)]
        {
            let executable = bin
                .join("resources/claude-accounts/matou-claude-accounts/matou-claude-accounts.exe");
            if executable.is_file() {
                return Some(Self::new(executable, None, true));
            }
        }
        #[cfg(target_os = "macos")]
        if bin.file_name().is_some_and(|name| name == "MacOS") {
            return Some(Self::new(
                bin.parent()?
                    .join("Resources/claude-accounts/matou-claude-accounts/matou-claude-accounts"),
                None,
                true,
            ));
        }
        let tooling =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tooling/claude-accounts");
        Some(Self::new(
            tooling.join(if cfg!(windows) {
                ".venv/Scripts/python.exe"
            } else {
                ".venv/bin/python"
            }),
            Some(tooling.join("bridge.py")),
            true,
        ))
    }

    fn spawn(&self, root: &Path, input: Value, interactive: bool) -> Result<EngineProcess, Fault> {
        if self.script.as_ref().is_some_and(|path| !path.is_file()) {
            return Err("engine-missing");
        }
        let mut command = Command::new(&self.executable);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        // Login is the bridge's only action that starts Claude Code. Roster
        // reads and local credential transactions need no shell startup.
        if self.resolve_login_path && interactive {
            let cwd = ["HOME", "USERPROFILE"]
                .into_iter()
                .filter_map(std::env::var_os)
                .map(PathBuf::from)
                .find(|path| path.is_absolute() && path.is_dir())
                .or_else(|| std::env::current_dir().ok());
            if let Some(cwd) = cwd {
                command.current_dir(&cwd);
                #[cfg(unix)]
                if let Some(path) = self.login_path.get_or_init(|| {
                    if std::env::var_os("MATOU_DISABLE_LOGIN_PATH_PROBE").as_deref()
                        == Some(OsStr::new("1"))
                    {
                        return None;
                    }
                    let shell = std::env::var_os("SHELL")
                        .filter(|shell| !shell.is_empty())
                        .unwrap_or_else(|| "/bin/zsh".into());
                    login_shell_path(&shell, &cwd)
                }) {
                    command.env("PATH", path);
                }
            }
        }
        // Inherit the current safe-browser settings on every spawn. Only PATH
        // comes from the login shell, never its credentials or other variables.
        let safe_browser = [
            "MATOU_SAFE_BROWSER_BIN",
            "MATOU_SAFE_BROWSER_TZ",
            "MATOU_SAFE_BROWSER_SEED",
        ];
        command.envs(
            safe_browser
                .into_iter()
                .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
        );
        let mut child = command
            .args(self.script.iter())
            .arg(root)
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONUNBUFFERED", "1")
            .env_remove("PYTHONPATH")
            .env_remove("PYTHONHOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "engine-missing")?;
        let stdout = child.stdout.take().expect("piped stdout");
        let output = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(MAX_OUTPUT as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "engine-error")?;
            if bytes.len() > MAX_OUTPUT {
                return Err("engine-error");
            }
            Ok(bytes)
        });
        let mut process = EngineProcess {
            child,
            output: Some(output),
        };
        let written = process.write(&input);
        if !interactive {
            written.map_err(|_| "engine-error")?;
            process.child.stdin.take();
        }
        // A fast interactive exit can close stdin before the first write.
        // Keep its stdout so the worker can report the bridge's error code.
        Ok(process)
    }
}

#[cfg(unix)]
fn login_shell_path(shell: &OsStr, cwd: &Path) -> Option<OsString> {
    const MARKER: &[u8] = b"__MATOU_LOGIN_PATH__=";
    const LIMIT: u64 = 64 * 1024;
    let mut output = tempfile::tempfile().ok()?;
    let mut child = Command::new(shell)
        .args(["-ilc", "printf '__MATOU_LOGIN_PATH__=%s\\n' \"$PATH\""])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(output.try_clone().ok()?)
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if !output
            .metadata()
            .is_ok_and(|metadata| metadata.len() <= LIMIT)
        {
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                output.seek(SeekFrom::Start(0)).ok()?;
                let mut bytes = Vec::new();
                output.take(LIMIT + 1).read_to_end(&mut bytes).ok()?;
                if bytes.len() as u64 > LIMIT {
                    return None;
                }
                return bytes
                    .split(|&byte| byte == b'\n')
                    .rev()
                    .find_map(|line| line.strip_prefix(MARKER))
                    .filter(|path| !path.is_empty())
                    .map(|path| OsString::from_vec(path.to_vec()));
            }
            Ok(Some(_)) => return None,
            Err(_) => break,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    }
    // The shell owns a separate process group until reaped.
    unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    let _ = child.wait();
    None
}

struct EngineProcess {
    child: Child,
    output: Option<thread::JoinHandle<Result<Vec<u8>, Fault>>>,
}

impl EngineProcess {
    fn write(&mut self, input: &Value) -> Result<(), Fault> {
        let stdin = self.child.stdin.as_mut().ok_or("login-ended")?;
        serde_json::to_writer(&mut *stdin, input).map_err(|_| "login-ended")?;
        stdin.write_all(b"\n").map_err(|_| "login-ended")
    }

    fn stop_login(&mut self, nonce: &str) {
        // Let the bridge terminate its own CLI before the process is reaped.
        // This also works on Windows, where killing the bridge skips Python's finally.
        if self
            .write(&json!({"action":"cancel-login", "nonce":nonce}))
            .is_ok()
        {
            self.child.stdin.take();
            let deadline = Instant::now() + Duration::from_secs(4);
            while Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn poll(&mut self) -> Option<Result<Value, Fault>> {
        // Both sides must finish. A subprocess retaining stdout must not hold
        // the account worker past its deadline.
        if !self
            .output
            .as_ref()
            .is_some_and(|output| output.is_finished())
        {
            return None;
        }
        match self.child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) if status.success() => Some(self.decode()),
            _ => Some(Err("engine-error")),
        }
    }

    fn decode(&mut self) -> Result<Value, Fault> {
        let bytes = self
            .output
            .take()
            .ok_or("engine-error")?
            .join()
            .map_err(|_| "engine-error")??;
        let result: Value = serde_json::from_slice(&bytes).map_err(|_| "engine-error")?;
        if result["ok"] == true && result["value"].is_object() {
            return Ok(result["value"].clone());
        }
        const ERRORS: &[Fault] = &[
            "browser-missing",
            "browser-closed",
            "engine-version",
            "cli-missing",
            "login-failed",
            "login-timeout",
            "identity",
            "conflict",
            "invalid",
            "switch-failed",
            "engine-error",
            "keychain",
            "busy",
            "unsupported",
            "network",
            "login-required",
            "storage",
            "recovery-required",
        ];
        Err(ERRORS
            .iter()
            .copied()
            .find(|code| result["error"] == *code)
            .unwrap_or("engine-error"))
    }
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            #[cfg(unix)]
            {
                // The bridge traps SIGTERM to close its isolated sign-in flow.
                unsafe {
                    libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
                }
                for _ in 0..10 {
                    thread::sleep(Duration::from_millis(20));
                    if self.child.try_wait().ok().flatten().is_some() {
                        return;
                    }
                }
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct ActiveLogin {
    request_id: u64,
    process: EngineProcess,
    began: Instant,
    nonce: String,
    replace_id: Option<String>,
    submitted: bool,
}

struct Draft {
    account: ClaudeAccount,
    nonce: String,
    replace_id: Option<String>,
}

struct Manager {
    root: PathBuf,
    engine: Option<Engine>,
    prefs: Preferences,
    roster: Roster,
    login: Option<ActiveLogin>,
    draft: Option<Draft>,
    cleaned: bool,
}

impl Manager {
    fn new(root: PathBuf, engine: Option<Engine>) -> Self {
        Self {
            root,
            engine,
            prefs: Preferences::default(),
            roster: Roster::default(),
            login: None,
            draft: None,
            cleaned: false,
        }
    }

    fn engine(&self) -> Result<&Engine, Fault> {
        self.engine.as_ref().ok_or("engine-missing")
    }

    fn run(&self, input: Value) -> Result<Value, Fault> {
        let mut process = self.engine()?.spawn(&self.root, input, false)?;
        let started = Instant::now();
        while started.elapsed() < TIMEOUT {
            if let Some(result) = process.poll() {
                return result;
            }
            thread::sleep(Duration::from_millis(15));
        }
        Err("engine-error")
    }

    fn read_prefs(&self) -> Result<Preferences, Fault> {
        match fs::read(self.root.join("preferences.json")) {
            Ok(bytes) => {
                let prefs: Preferences = serde_json::from_slice(&bytes).map_err(|_| "storage")?;
                if prefs.names.values().any(|name| !valid_name(name))
                    || prefs
                        .renewed_ids
                        .iter()
                        .any(|id| id.is_empty() || id.len() > 200)
                {
                    return Err("storage");
                }
                Ok(prefs)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Preferences::default())
            }
            Err(_) => Err("storage"),
        }
    }

    fn save(&mut self, edit: impl FnOnce(&mut Preferences)) -> Result<(), Fault> {
        let mut prefs = self.prefs.clone();
        edit(&mut prefs);
        let mut temp = tempfile::NamedTempFile::new_in(&self.root).map_err(|_| "storage")?;
        serde_json::to_writer(&mut temp, &prefs).map_err(|_| "storage")?;
        temp.persist(self.root.join("preferences.json"))
            .map_err(|_| "storage")?;
        self.prefs = prefs;
        Ok(())
    }

    fn set_roster(&mut self, value: Value) -> Result<(), Fault> {
        self.roster = serde_json::from_value(value).map_err(|_| "engine-error")?;
        Ok(())
    }

    fn inspect(&mut self) -> Result<(), Fault> {
        if !self.cleaned {
            self.run(json!({"action":"cleanup-logins"}))?;
            self.cleaned = true;
        }
        let value = self.run(json!({"action":"snapshot"}))?;
        self.set_roster(value)?;
        let newer: Vec<_> = self
            .roster
            .accounts
            .iter()
            .filter_map(|account| {
                let usage = account.usage.as_ref()?;
                let old = self.prefs.usage.get(&account.id);
                old.is_none_or(|old| usage.checked_at > old.checked_at)
                    .then(|| (account.id.clone(), usage.clone()))
            })
            .collect();
        if !newer.is_empty() {
            self.save(|prefs| prefs.usage.extend(newer))?;
        }
        Ok(())
    }

    fn cancel(&mut self) -> Option<u64> {
        let request = self.login.take().map(|mut login| {
            let nonce = login.nonce.clone();
            let request_id = login.request_id;
            login.process.stop_login(&nonce);
            drop(login);
            let _ = self.run(json!({"action":"cancel-login", "nonce":nonce}));
            request_id
        });
        if let Some(draft) = self.draft.take() {
            let _ = self.run(json!({"action":"cancel-login", "nonce":draft.nonce}));
        }
        request
    }

    fn poll_login(&mut self) -> Option<(u64, ClaudeAccountsState, Option<String>)> {
        let login = self.login.as_mut()?;
        let result = if login.began.elapsed() >= Duration::from_secs(360) {
            login.process.stop_login(&login.nonce);
            Err("login-timeout")
        } else {
            login.process.poll()?
        };
        let login = self.login.take().expect("active login");
        let request_id = login.request_id;
        let nonce = login.nonce.clone();
        let result = result.and_then(|value| {
            let account: ClaudeAccount =
                serde_json::from_value(value["draft"].clone()).map_err(|_| "login-failed")?;
            if login
                .replace_id
                .as_ref()
                .is_some_and(|id| *id != account.id)
            {
                return Err("identity");
            }
            self.draft = Some(Draft {
                account,
                nonce: nonce.clone(),
                replace_id: login.replace_id.clone(),
            });
            Ok(())
        });
        drop(login);
        let error = result.err();
        Some((request_id, self.state(error, false), error.map(|_| nonce)))
    }

    fn state(&self, error: Option<Fault>, switched: bool) -> ClaudeAccountsState {
        let decorate = |account: &ClaudeAccount| {
            let mut account = account.clone();
            if let Some(name) = self.prefs.names.get(&account.id) {
                account.name.clone_from(name);
            }
            if let Some(usage) = self.prefs.usage.get(&account.id) {
                account.usage = Some(usage.clone());
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if let Some(usage) = &mut account.usage {
                usage.stale |= self
                    .roster
                    .current_id
                    .as_ref()
                    .or_else(|| self.roster.detected.as_ref().map(|a| &a.id))
                    != Some(&account.id)
                    || now.saturating_sub(usage.checked_at) > 180_000
                    || [usage.five_hour.as_ref(), usage.weekly.as_ref()]
                        .into_iter()
                        .flatten()
                        .any(|window| window.reset_timestamp().is_some_and(|reset| reset <= now));
            }
            account
        };
        ClaudeAccountsState {
            accounts: self.roster.accounts.iter().map(decorate).collect(),
            current_id: self.roster.current_id.clone(),
            detected: self.roster.detected.as_ref().map(decorate),
            engine_version: self.roster.engine_version.clone(),
            remember_scope: self.prefs.remember_scope,
            renewed_ids: self.prefs.renewed_ids.clone(),
            draft: self.draft.as_ref().map(|draft| decorate(&draft.account)),
            reauth_id: self
                .login
                .as_ref()
                .and_then(|login| login.replace_id.clone())
                .or_else(|| {
                    self.draft
                        .as_ref()
                        .and_then(|draft| draft.replace_id.clone())
                }),
            login_attempt_id: self.login.as_ref().map(|login| login.nonce.clone()),
            login_code_submitted: self.login.as_ref().is_some_and(|login| login.submitted),
            error: error.map(str::to_owned),
            switched,
        }
    }

    fn account(&self, id: &str) -> Result<&ClaudeAccount, Fault> {
        self.roster
            .accounts
            .iter()
            .find(|account| account.id == id)
            .ok_or("conflict")
    }

    fn execute(&mut self, request_id: u64, command: ClaudeAccountCommand) -> Result<bool, Fault> {
        use ClaudeAccountCommand::*;
        if let SubmitLoginCode { attempt_id, code } = command {
            let login = self
                .login
                .as_mut()
                .filter(|login| login.nonce == attempt_id)
                .ok_or("login-ended")?;
            let code = code.trim();
            if !valid_code(code) {
                return Err("login-code-invalid");
            }
            if !login.submitted {
                login.process.write(
                    &json!({"action":"submit-login-code", "nonce":attempt_id, "code":code}),
                )?;
                login.submitted = true;
            }
            return Ok(false);
        }
        if self.login.is_some() {
            return Err("busy");
        }
        self.engine()?;
        self.prefs = self.read_prefs()?;
        match command {
            List | RefreshUsage => self.inspect()?,
            Login { replace_id } => {
                if let Some(id) = &replace_id {
                    self.account(id)?;
                }
                self.cancel();
                let nonce = uuid::Uuid::new_v4().to_string();
                let process = self.engine()?.spawn(
                    &self.root,
                    json!({"action":"login", "nonce":nonce, "replaceId":replace_id}),
                    true,
                )?;
                self.login = Some(ActiveLogin {
                    request_id,
                    process,
                    began: Instant::now(),
                    nonce,
                    replace_id,
                    submitted: false,
                });
            }
            Rename { id, name } => {
                self.account(&id)?;
                if !valid_name(&name) {
                    return Err("invalid");
                }
                self.save(|prefs| {
                    prefs.names.insert(id, name.trim().to_owned());
                })?;
            }
            Remove { id } => {
                self.account(&id)?;
                let value = self.run(json!({"action":"remove", "id":id}))?;
                self.set_roster(value)?;
                self.save(|prefs| {
                    prefs.names.remove(&id);
                    prefs.usage.remove(&id);
                    prefs.renewed_ids.retain(|value| *value != id);
                })?;
            }
            Switch { ref id, .. } | Reauth { ref id } => {
                let reauth = matches!(command, Reauth { .. });
                if !reauth && self.account(id)?.needs_login {
                    return Err("login-required");
                }
                if reauth && !self.prefs.renewed_ids.contains(id) {
                    return Err("conflict");
                }
                let action = if reauth { "reauth" } else { "switch" };
                let value = self.run(json!({"action":action, "id":id}))?;
                self.set_roster(value)?;
                if self.roster.current_id.as_deref() != Some(id) {
                    return Err("switch-failed");
                }
                self.save(|prefs| {
                    if let Switch { remember_scope, .. } = command {
                        prefs.remember_scope = remember_scope;
                    }
                    prefs.renewed_ids.retain(|value| value != id);
                })?;
                return Ok(true);
            }
            SaveDraft { ref name } | SaveDetected { ref name } => {
                if !valid_name(name) {
                    return Err("invalid");
                }
                let (id, nonce, renewed) = match command {
                    SaveDraft { .. } => {
                        let draft = self.draft.as_ref().ok_or("conflict")?;
                        (
                            draft.account.id.clone(),
                            Some(draft.nonce.clone()),
                            draft.replace_id.is_some(),
                        )
                    }
                    _ => (
                        self.roster.detected.as_ref().ok_or("conflict")?.id.clone(),
                        None,
                        false,
                    ),
                };
                let action = if nonce.is_some() {
                    "save-draft"
                } else {
                    "save-current"
                };
                let value = self.run(json!({"action":action, "id":id, "nonce":nonce}))?;
                if nonce.is_some() {
                    self.draft = None;
                }
                self.set_roster(value)?;
                self.save(|prefs| {
                    if renewed {
                        if !prefs.renewed_ids.contains(&id) {
                            prefs.renewed_ids.push(id);
                        }
                    } else {
                        prefs.names.insert(id, name.trim().to_owned());
                    }
                })?;
            }
            SubmitLoginCode { .. } | CancelLogin => unreachable!(),
        }
        Ok(false)
    }
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.trim().chars().count() <= 12
        && !name.chars().any(char::is_control)
}

fn valid_code(value: &str) -> bool {
    value.len() <= 4096
        && value.split_once('#').is_some_and(|(code, state)| {
            [code, state].iter().all(|part| {
                part.len() >= 10
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_~.-".contains(&byte))
            })
        })
}

pub(crate) struct AccountWorker {
    requests: Option<mpsc::Sender<(u64, ClaudeAccountCommand)>>,
    thread: Option<thread::JoinHandle<()>>,
    pub replies: mpsc::Receiver<(u64, ClaudeAccountsState)>,
}

impl AccountWorker {
    pub fn start(
        data_root: &Path,
        allow_default: bool,
        fixture: Option<PathBuf>,
    ) -> std::io::Result<Self> {
        Self::spawn(
            data_root.join("claude-accounts"),
            Engine::resolve(allow_default, fixture),
        )
    }

    pub fn send(&self, id: u64, command: ClaudeAccountCommand) -> bool {
        self.requests
            .as_ref()
            .is_some_and(|requests| requests.send((id, command)).is_ok())
    }

    fn spawn(root: PathBuf, engine: Option<Engine>) -> std::io::Result<Self> {
        let (requests, incoming) = mpsc::channel();
        let (outgoing, replies) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("claude-accounts".into())
            .spawn(move || {
                let mut manager = Manager::new(root, engine);
                loop {
                    if let Some((id, state, cleanup)) = manager.poll_login() {
                        let _ = outgoing.send((id, state));
                        if let Some(nonce) = cleanup {
                            let _ = manager.run(json!({"action":"cancel-login", "nonce":nonce}));
                        }
                    }
                    let request = if manager.login.is_some() {
                        incoming.recv_timeout(Duration::from_millis(40))
                    } else {
                        incoming
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    let (id, command) = match request {
                        Ok(request) => request,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let result = if matches!(command, ClaudeAccountCommand::CancelLogin) {
                        if let Some(login_id) = manager.cancel() {
                            let _ = outgoing.send((login_id, manager.state(None, false)));
                        }
                        Ok(false)
                    } else {
                        manager.execute(id, command)
                    };
                    let state = match result {
                        Ok(switched) => manager.state(None, switched),
                        Err(code) => manager.state(Some(code), false),
                    };
                    if outgoing.send((id, state)).is_err() {
                        break;
                    }
                }
                manager.cancel();
            })?;
        Ok(Self {
            requests: Some(requests),
            thread: Some(thread),
            replies,
        })
    }
}

impl Drop for AccountWorker {
    fn drop(&mut self) {
        drop(self.requests.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn executable(path: &Path, script: &str) {
        fs::write(path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn login_shell_path_recovers_node_for_an_env_shebang() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("nvm/bin");
        fs::create_dir_all(&bin).unwrap();
        let shell = root.path().join("login-shell");
        executable(
            &shell,
            "#!/bin/sh\n[ \"$1\" = -ilc ] || exit 1\nPATH=\"$PWD/nvm/bin:$PATH\"\nprintf 'startup banner\\n'\neval \"$2\"\n",
        );
        executable(
            &bin.join("node"),
            "#!/bin/sh\nprintf 'ready from node\\n'\n",
        );
        let claude = root.path().join("claude");
        executable(&claude, "#!/usr/bin/env node\n");
        let path = login_shell_path(shell.as_os_str(), root.path()).unwrap();
        assert_eq!(
            std::env::split_paths(&path)
                .next()
                .map(|entry| fs::canonicalize(entry).unwrap()),
            Some(fs::canonicalize(&bin).unwrap())
        );
        let result = Command::new(claude).env("PATH", path).output().unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"ready from node\n");
    }

    fn receive(worker: &AccountWorker, request: u64) -> ClaudeAccountsState {
        let (id, state) = worker.replies.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(id, request);
        state
    }

    #[test]
    fn login_publishes_attempt_and_reports_immediate_engine_errors() {
        let root = tempfile::tempdir().unwrap();
        for error in ["cli-missing", "browser-missing", "login-failed"] {
            let engine = root.path().join(error);
            executable(
                &engine,
                &format!(
                    "#!/bin/sh\nread -r request\ncase \"$request\" in\n  *cancel-login*) printf '%s\\n' '{{\"ok\":true,\"value\":{{}}}}' ;;\n  *) printf '%s\\n' '{{\"ok\":false,\"error\":\"{error}\"}}' ;;\nesac\n"
                ),
            );
            let worker = AccountWorker::start(root.path(), false, Some(engine)).unwrap();
            assert!(worker.send(1, ClaudeAccountCommand::Login { replace_id: None }));
            let waiting = receive(&worker, 1);
            assert!(waiting.login_attempt_id.is_some());
            assert!(waiting.error.is_none());
            let failed = receive(&worker, 1);
            assert!(failed.login_attempt_id.is_none());
            assert_eq!(failed.error.as_deref(), Some(error));
        }
    }

    #[test]
    fn cancel_signals_waiting_bridge_and_clears_attempt() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("claude-accounts")).unwrap();
        let engine = root.path().join("engine");
        executable(
            &engine,
            r#"#!/bin/sh
read -r request
case "$request" in
  *cancel-login*) ;;
  *)
    read -r cancel
    nonce=${cancel#*\"nonce\":\"}
    nonce=${nonce%%\"*}
    case "$cancel" in *cancel-login*) printf %s "$nonce" > "$1/stopped" ;; esac
    ;;
esac
printf '%s\n' '{"ok":true,"value":{}}'
"#,
        );
        let worker = AccountWorker::start(root.path(), false, Some(engine)).unwrap();
        assert!(worker.send(1, ClaudeAccountCommand::Login { replace_id: None }));
        let waiting = receive(&worker, 1);
        assert!(worker.send(2, ClaudeAccountCommand::CancelLogin));
        assert!(receive(&worker, 1).login_attempt_id.is_none());
        assert!(receive(&worker, 2).login_attempt_id.is_none());
        assert_eq!(
            fs::read_to_string(root.path().join("claude-accounts/stopped")).unwrap(),
            waiting.login_attempt_id.unwrap()
        );
    }
}
