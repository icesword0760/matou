//! Process ownership for the fixed, private loopback subscription engine.
mod config;
mod management;

use std::{
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};

use crate::subscription_launch::ModelAlias;
pub(crate) use config::credential_path;
pub(crate) use config::macos_version;
pub(crate) use management::{AccountProbe, ManagementClient, antigravity_user_agent};

const VERSION: &str = "8.0.69";
const POLL: Duration = Duration::from_millis(25);
pub(crate) const READY_TIMEOUT: Duration = Duration::from_secs(8);
const IDLE: Duration = Duration::from_secs(1);
const RECOVERY: [Duration; 3] = [
    Duration::from_millis(300),
    Duration::from_millis(600),
    Duration::from_millis(1200),
];
pub const WATCHDOG_MODE: &str = "--model-router-watchdog";

#[derive(Clone)]
pub(crate) struct RouterConnection {
    pub port: u16,
    pub generation: u64,
    pub client_key: String,
    management_key: String,
    proxy: Option<String>,
    codex: Arc<config::CodexClientIdentity>,
}

impl RouterConnection {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

#[derive(Default)]
struct State {
    stopped: bool,
    leases: usize,
    idle_since: Option<Instant>,
    connection: Option<RouterConnection>,
    failure: Option<String>,
    aliases: std::collections::BTreeMap<&'static str, Vec<ModelAlias>>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub(crate) struct ModelRouterManager {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    environment: Arc<std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>>,
}

impl ModelRouterManager {
    pub fn new(
        root: PathBuf,
        environment: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    ) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let state = Arc::clone(&shared);
        let environment = Arc::new(environment);
        let worker_env = Arc::clone(&environment);
        let worker = thread::Builder::new()
            .name("matou-model-router".into())
            .spawn(move || supervise(&root, &worker_env, &state))?;
        Ok(Self {
            shared,
            worker: Mutex::new(Some(worker)),
            environment,
        })
    }

    pub fn acquire(&self) -> Result<(RouterConnection, RouterLease)> {
        let lease = self.reserve()?;
        Ok((self.wait_for_connection()?, lease))
    }

    pub fn reserve(&self) -> Result<RouterLease> {
        let mut state = self.shared.state.lock().unwrap();
        ensure!(!state.stopped, "Model router is shutting down");
        if state.leases == 0 {
            state.failure = None;
        }
        state.leases += 1;
        state.idle_since = None;
        self.shared.changed.notify_all();
        Ok(RouterLease(Arc::clone(&self.shared)))
    }

    pub fn wait_for_connection(&self) -> Result<RouterConnection> {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            ensure!(!state.stopped, "Model router is shutting down");
            ensure!(state.leases > 0, "Model router lease was released");
            if let Some(connection) = &state.connection {
                return Ok(connection.clone());
            }
            if let Some(error) = &state.failure {
                return Err(anyhow!("{error}"));
            }
            state = self.shared.changed.wait(state).unwrap();
        }
    }

    pub fn current_connection(&self) -> Option<RouterConnection> {
        self.shared.state.lock().unwrap().connection.clone()
    }

    pub fn egress_proxy(&self) -> Result<Option<String>> {
        if let Some(connection) = self.current_connection() {
            return Ok(connection.proxy);
        }
        config::proxy(&self.environment)
    }

    pub fn recover(&self, failed_generation: u64) -> Result<(RouterConnection, RouterLease)> {
        // The transport can observe a crash before the watchdog's next poll.
        let state = self.shared.state.lock().unwrap();
        let (state, _) = self
            .shared
            .changed
            .wait_timeout_while(state, POLL * 4, |state| {
                !state.stopped
                    && state
                        .connection
                        .as_ref()
                        .is_some_and(|connection| connection.generation == failed_generation)
            })
            .unwrap();
        drop(state);
        self.acquire()
    }

    pub fn remember_aliases(&self, provider: &'static str, aliases: Vec<ModelAlias>) {
        self.shared
            .state
            .lock()
            .unwrap()
            .aliases
            .insert(provider, aliases);
    }

    pub fn shutdown(&self) {
        self.shared.state.lock().unwrap().stopped = true;
        self.shared.changed.notify_all();
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ModelRouterManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) struct RouterLease(Arc<Shared>);

impl Drop for RouterLease {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.leases -= 1;
        if state.leases == 0 {
            state.idle_since = Some(Instant::now());
        }
        self.0.changed.notify_all();
    }
}

fn supervise(
    root: &Path,
    environment: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    shared: &Shared,
) {
    let mut process: Option<Watchdog> = None;
    let mut previous_port = None;
    let mut restart = None;
    let mut last_restart = None;
    let mut ready_since: Option<Instant> = None;
    let mut generation = 0;
    let mut codex = None;
    loop {
        let mut state = shared.state.lock().unwrap();
        if state.stopped {
            break;
        }
        if let Some(child) = &mut process {
            if !child.alive() {
                previous_port = process.as_ref().map(|child| child.port);
                process.take();
                state.connection = None;
                shared.changed.notify_all();
                if state.leases > 0 {
                    // Reset the crash budget after a stable generation.
                    if ready_since.is_some_and(|time| time.elapsed() >= READY_TIMEOUT) {
                        restart = None;
                    }
                    restart = Some(restart.map_or(0, |index: usize| index + 1));
                    last_restart = Some(Instant::now());
                }
            } else if state.idle_since.is_some_and(|idle| idle.elapsed() >= IDLE) {
                state.connection = None;
                previous_port = process.as_ref().map(|child| child.port);
                drop(state);
                process.take();
                restart = None;
                last_restart = None;
                continue;
            }
        }
        if process.is_none() && state.leases > 0 && state.failure.is_none() {
            if let Some(index) = restart {
                let Some(delay) = RECOVERY.get(index) else {
                    state.failure = Some("Model router stopped after repeated crashes".into());
                    shared.changed.notify_all();
                    continue;
                };
                let remaining = delay.saturating_sub(last_restart.unwrap().elapsed());
                if !remaining.is_zero() {
                    drop(shared.changed.wait_timeout(state, remaining).unwrap());
                    continue;
                }
            }
            let aliases = state.aliases.clone();
            drop(state);
            generation += 1;
            // Resolve only when the first subscription lease needs a router.
            let codex = codex
                .get_or_insert_with(|| Arc::new(config::CodexClientIdentity::detect(environment)));
            let result = start(
                root,
                environment,
                shared,
                &aliases,
                previous_port,
                restart.is_some(),
                generation,
                codex,
            );
            let mut state = shared.state.lock().unwrap();
            match result {
                Ok((child, connection)) => {
                    previous_port = Some(connection.port);
                    ready_since = Some(Instant::now());
                    state.connection = Some(connection);
                    process = Some(child);
                }
                Err(error) if restart.is_some() => {
                    restart = restart.map(|index| index + 1);
                    last_restart = Some(Instant::now());
                    if restart == Some(RECOVERY.len()) {
                        state.failure = Some(error.to_string());
                    }
                }
                Err(error) => state.failure = Some(error.to_string()),
            }
            shared.changed.notify_all();
        } else {
            if state.leases == 0 && process.is_none() {
                restart = None;
                last_restart = None;
                drop(shared.changed.wait(state).unwrap());
            } else {
                drop(shared.changed.wait_timeout(state, POLL).unwrap());
            }
        }
    }
    drop(process);
}

fn start(
    root: &Path,
    environment: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    shared: &Shared,
    aliases: &std::collections::BTreeMap<&'static str, Vec<ModelAlias>>,
    previous_port: Option<u16>,
    recovering: bool,
    generation: u64,
    codex: &Arc<config::CodexClientIdentity>,
) -> Result<(Watchdog, RouterConnection)> {
    let directory = config::prepare(root)?;
    let binary = config::router_binary(
        matou_native_core::pty::get_var(environment, "MATOU_MODEL_ROUTER_BIN")
            .map(std::ffi::OsStr::to_owned),
    )?;
    let watchdog = config::runtime_binary()?;
    let proxy = config::proxy(environment)?;
    let deadline = Instant::now() + READY_TIMEOUT;
    // An interrupted exchange must never reach a different process that
    // picked up the old address. Recover onto a fresh loopback endpoint.
    let mut used = previous_port
        .filter(|_| recovering)
        .into_iter()
        .collect::<Vec<_>>();
    for attempt in 0..3 {
        ensure!(
            !shared.state.lock().unwrap().stopped,
            "Model router is shutting down"
        );
        let preferred = if attempt == 0 && !recovering {
            previous_port
        } else {
            None
        };
        let socket = preferred
            .and_then(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok())
            .map_or_else(|| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)), Ok)?;
        let port = socket.local_addr()?.port();
        if used.contains(&port) {
            continue;
        }
        used.push(port);
        directory.write_config(port, proxy.as_deref(), aliases, codex)?;
        let connection = RouterConnection {
            port,
            generation,
            client_key: directory.keys.client.clone(),
            management_key: directory.keys.management.clone(),
            proxy: proxy.clone(),
            codex: Arc::clone(codex),
        };
        let client = ManagementClient::new(&connection);
        // Release the reservation only when the child is ready to bind.
        drop(socket);
        let mut child = Watchdog::spawn(&watchdog, &binary, &directory.path, port)?;
        while child.alive() && Instant::now() < deadline {
            ensure!(
                !shared.state.lock().unwrap().stopped,
                "Model router is shutting down"
            );
            let timeout = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(500));
            match client.ready(timeout) {
                Ok(true) if child.alive() => return Ok((child, connection)),
                Ok(false) if child.alive() => {
                    return Err(anyhow!("Model router version does not match {VERSION}"));
                }
                _ => {}
            }
            thread::sleep(POLL);
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    Err(anyhow!("Model router did not become ready"))
}

struct Watchdog {
    child: Child,
    port: u16,
}

impl Watchdog {
    fn spawn(watchdog: &Path, binary: &Path, directory: &Path, port: u16) -> Result<Self> {
        let mut command = Command::new(watchdog);
        command
            .args([std::ffi::OsStr::new(WATCHDOG_MODE), directory.as_os_str()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // The watchdog resolves the same trusted binary independently.
        if cfg!(debug_assertions) {
            command.env("MATOU_MODEL_ROUTER_BIN", binary);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let child = command.spawn().context("Start model router watchdog")?;
        Ok(Self { child, port })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        // Close the pipe; the watchdog owns termination and reaping.
        self.child.stdin.take();
        let _ = self.child.wait();
    }
}

#[doc(hidden)]
pub fn run_watchdog(directory: &Path) -> Result<()> {
    let directory = directory.canonicalize()?;
    ensure!(
        directory
            .file_name()
            .is_some_and(|name| name == "model-router"),
        "Invalid model router directory"
    );
    let config = config::private_file(&directory, &directory.join("config.json"))?;
    let binary = config::router_binary(std::env::var_os("MATOU_MODEL_ROUTER_BIN"))?;
    let mut command = Command::new(binary);
    command
        .arg("-config")
        .arg(config)
        .args(["-local-model", "-no-browser"])
        .current_dir(&directory)
        .env_clear()
        .env("HOME", &directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for key in [
        "PATH",
        "LANG",
        "SYSTEMROOT",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command
            .env("USERPROFILE", &directory)
            .creation_flags(0x08000000);
    }
    let (notify, closed) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("matou-model-router-parent".into())
        .spawn(move || {
            use std::io::Read;
            let _ = std::io::stdin().read(&mut [0]);
            let _ = notify.send(());
        })?;
    let mut child = command.spawn().context("Start model router engine")?;
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            return Ok(());
        }
        if !matches!(
            closed.recv_timeout(POLL),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ) {
            break;
        }
    }
    #[cfg(unix)]
    {
        // This direct child remains our PID until try_wait/wait reaps it.
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return Ok(());
            }
            thread::sleep(POLL);
        }
    }
    let _ = child.kill();
    child.wait()?;
    Ok(())
}

#[cfg(all(test, target_os = "macos", debug_assertions))]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap, ffi::OsString, fs, os::unix::fs::PermissionsExt, sync::Barrier,
    };

    fn fixture_environment(root: &Path, mode: &str) -> Result<BTreeMap<OsString, OsString>> {
        let binary = root.join("fixture-router");
        fs::write(
            &binary,
            include_str!("../tests/fixtures/subscription-router.py"),
        )?;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
        let directory = root.join("model-router");
        fs::create_dir(&directory)?;
        fs::write(directory.join("fixture-mode"), mode)?;
        Ok(BTreeMap::from([
            ("MATOU_MODEL_ROUTER_BIN".into(), binary.into()),
            ("NO_PROXY".into(), "*".into()),
        ]))
    }

    fn fixture(mode: &str) -> Result<(tempfile::TempDir, ModelRouterManager)> {
        // The workspace test command builds the runtime binary used by process contracts.
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let manager = ModelRouterManager::new(
            root.path().to_owned(),
            fixture_environment(root.path(), mode)?,
        )?;
        Ok((root, manager))
    }

    fn starts(root: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(root.join("model-router/fixture-events.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|event| event["event"] == "engine-start")
            .collect()
    }

    #[test]
    fn concurrent_leases_idle_shutdown_and_busy_previous_port() -> Result<()> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let shell = root.path().join("fixture-login-shell");
        let probe_log = root.path().join("codex-probe.log");
        fs::write(
            &shell,
            "#!/bin/sh\nprintf 'probe\\n' >> \"$PROBE_LOG\"\nprintf '__MATOU_LOGIN_PATH__=%s\\n' \"$PATH\"\n",
        )?;
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
        let mut environment = fixture_environment(root.path(), "")?;
        environment.extend([
            ("PATH".into(), root.path().into()),
            ("HOME".into(), root.path().into()),
            ("SHELL".into(), shell.into()),
            ("PROBE_LOG".into(), probe_log.clone().into()),
        ]);
        let manager = ModelRouterManager::new(root.path().to_owned(), environment)?;
        let barrier = Barrier::new(6);
        let leases = thread::scope(|scope| {
            (0..6)
                .map(|_| {
                    let manager = &manager;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        manager.acquire()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|task| task.join().unwrap())
                .collect::<Result<Vec<_>>>()
        })?;
        let port = leases[0].0.port;
        assert!(leases.iter().all(|(connection, _)| connection.port == port));
        assert_eq!(leases[0].0.codex.version, "0.162.1");
        assert_eq!(fs::read_to_string(&probe_log)?, "probe\n");
        assert_eq!(starts(root.path()).len(), 1);
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("model-router/config.json"))?)?;
        let keys: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("model-router/keys.json"))?)?;
        assert_ne!(keys["client"], keys["management"]);
        assert_eq!(keys["client"], config["access"]["api-keys"][0]);
        assert_eq!(keys["management"], config["management"]["secret-key"]);
        assert_eq!(
            fs::metadata(root.path().join("model-router/config.json"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(root.path().join("model-router/credentials"))?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        drop(leases);
        thread::sleep(Duration::from_millis(850));
        assert_eq!(manager.current_connection().unwrap().port, port);
        let deadline = Instant::now() + Duration::from_secs(2);
        let occupied = loop {
            if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
                break listener;
            }
            ensure!(
                Instant::now() < deadline,
                "Router did not stop after final lease"
            );
            thread::sleep(POLL);
        };
        let (restarted, _lease) = manager.acquire()?;
        assert_ne!(restarted.port, port);
        assert_eq!(starts(root.path()).len(), 2);
        assert_eq!(restarted.codex.version, "0.162.1");
        assert_eq!(fs::read_to_string(probe_log)?, "probe\n");
        manager.shutdown();
        TcpListener::bind((Ipv4Addr::LOCALHOST, restarted.port))?;
        drop(occupied);
        Ok(())
    }

    #[test]
    fn retries_a_child_that_cannot_bind_and_rejects_wrong_version() -> Result<()> {
        let (root, manager) = fixture("fail-first-bind")?;
        let (_, _lease) = manager.acquire()?;
        assert_eq!(starts(root.path()).len(), 2);
        manager.shutdown();

        let (root, manager) = fixture("wrong-version")?;
        let error = manager.acquire().err().context("wrong version accepted")?;
        assert!(error.to_string().contains(VERSION));
        let events = starts(root.path());
        assert_eq!(events.len(), 1);
        TcpListener::bind((
            Ipv4Addr::LOCALHOST,
            events[0]["port"].as_u64().unwrap() as u16,
        ))?;
        Ok(())
    }

    #[test]
    fn readiness_deadline_reaps_the_unready_process() -> Result<()> {
        let (root, manager) = fixture("never-ready")?;
        let at = Instant::now();
        let error = manager.acquire().err().context("unready engine accepted")?;
        assert!(error.to_string().contains("did not become ready"));
        assert!(at.elapsed() >= READY_TIMEOUT);
        assert!(at.elapsed() < READY_TIMEOUT + Duration::from_secs(3));
        let events = starts(root.path());
        assert_eq!(events.len(), 1);
        TcpListener::bind((
            Ipv4Addr::LOCALHOST,
            events[0]["port"].as_u64().unwrap() as u16,
        ))?;
        Ok(())
    }
}
