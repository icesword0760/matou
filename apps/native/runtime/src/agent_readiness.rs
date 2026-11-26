//! Read-only checks for the Agent statuses in Settings diagnostics.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::{self, PipeReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use matou_native_core::provider_launch::resolve_command_with_login_path;
use matou_native_core::pty::get_var;
use serde::{Deserialize, Serialize};

use crate::codex_launch::resolve_executable;

const TIMEOUT: Duration = Duration::from_secs(4);
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentReadiness {
    Ready,
    CliNotFound,
    PathUnavailable,
    NotLoggedIn,
    StartFailed,
    Installed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AgentStatus {
    pub id: &'static str,
    pub readiness: AgentReadiness,
}

impl AgentStatus {
    pub fn name(&self) -> &'static str {
        match self.id {
            "codex" => "Codex",
            "claude-code" => "Claude Code",
            "antigravity" => "Antigravity",
            _ => "DSH",
        }
    }
}

#[derive(Clone, Copy)]
struct Agent {
    id: &'static str,
    command: &'static str,
    override_key: &'static str,
    args: &'static [&'static str],
    check: Check,
}

#[derive(Clone, Copy)]
enum Check {
    CodexAuth,
    ClaudeAuth,
    Installed,
}

const AGENTS: [Agent; 4] = [
    Agent {
        id: "codex",
        command: "codex",
        override_key: "MATOU_CODEX_COMMAND",
        args: &["login", "status"],
        check: Check::CodexAuth,
    },
    Agent {
        id: "claude-code",
        command: "claude",
        override_key: "MATOU_CLAUDE_COMMAND",
        args: &["auth", "status", "--json"],
        check: Check::ClaudeAuth,
    },
    Agent {
        id: "dsh",
        command: "dsh",
        override_key: "MATOU_DSH_COMMAND",
        args: &["--version"],
        check: Check::Installed,
    },
    Agent {
        id: "antigravity",
        command: "agy",
        override_key: "MATOU_ANTIGRAVITY_COMMAND",
        args: &["--help"],
        check: Check::Installed,
    },
];

/// Call from a background worker. Returns only statuses, with no CLI output or credentials.
pub fn probe(cwd: &Path, environment: BTreeMap<OsString, OsString>) -> Vec<AgentStatus> {
    let cwd = std::path::absolute(cwd).ok();
    thread::scope(|scope| {
        let checks = AGENTS.map(|agent| {
            let environment = &environment;
            let cwd = cwd.as_deref();
            thread::Builder::new().spawn_scoped(scope, move || {
                cwd.map_or(AgentReadiness::StartFailed, |cwd| {
                    probe_agent(agent, cwd, environment)
                })
            })
        });
        AGENTS
            .into_iter()
            .zip(checks)
            .map(|(agent, check)| AgentStatus {
                id: agent.id,
                readiness: check
                    .ok()
                    .and_then(|worker| worker.join().ok())
                    .unwrap_or(AgentReadiness::StartFailed),
            })
            .collect()
    })
}

fn probe_agent(
    agent: Agent,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> AgentReadiness {
    let command = get_var(environment, agent.override_key)
        .filter(|value| !value.is_empty())
        .unwrap_or(OsStr::new(agent.command));
    let command = Path::new(command);
    let executable = resolve_executable(command, cwd, environment);
    #[cfg(unix)]
    let (executable, login_environment) = if executable.is_none() {
        let mut recovered = environment.clone();
        match resolve_command_with_login_path(command, cwd, &mut recovered) {
            Ok(executable) => (Some(executable), Some(recovered)),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return AgentReadiness::StartFailed;
            }
            Err(_) => (None, None),
        }
    } else {
        (executable, None)
    };
    let Some(executable) = executable else {
        return if command.file_name() == Some(command.as_os_str())
            && known_bin_directories(agent, environment)
                .into_iter()
                .any(|bin| resolve_executable(&bin.join(command), cwd, environment).is_some())
        {
            AgentReadiness::PathUnavailable
        } else {
            AgentReadiness::CliNotFound
        };
    };
    #[cfg(unix)]
    let environment = login_environment.as_ref().unwrap_or(environment);
    run_check(agent, &executable, cwd, environment, TIMEOUT).unwrap_or(AgentReadiness::StartFailed)
}

fn known_bin_directories(agent: Agent, environment: &BTreeMap<OsString, OsString>) -> Vec<PathBuf> {
    let path = |key| {
        get_var(environment, key)
            .map(Path::new)
            .filter(|path| path.is_absolute())
    };
    let mut bins = Vec::new();
    if let Some(home) = if cfg!(windows) {
        path("USERPROFILE").or_else(|| path("HOME"))
    } else {
        path("HOME")
    } {
        bins.push(home.join(".local/bin"));
    }
    if agent.id == "codex"
        && let Some(dir) = path("CODEX_INSTALL_DIR")
    {
        bins.push(dir.to_owned());
    }
    if let Some(pnpm) = path("PNPM_HOME") {
        bins.push(pnpm.to_owned());
        bins.push(pnpm.join("bin"));
    }
    if let Some(prefix) = path("NPM_CONFIG_PREFIX").or_else(|| path("npm_config_prefix")) {
        bins.push(if cfg!(windows) {
            prefix.to_owned()
        } else {
            prefix.join("bin")
        });
    }
    #[cfg(windows)]
    {
        if let Some(local) = path("LOCALAPPDATA") {
            match agent.id {
                "codex" => bins.push(local.join("Programs/OpenAI/Codex/bin")),
                "antigravity" => bins.push(local.join("agy/bin")),
                _ => {}
            }
            bins.push(local.join("Microsoft/WinGet/Links"));
        }
        if let Some(roaming) = path("APPDATA") {
            bins.push(roaming.join("npm"));
        }
    }
    bins
}

fn run_check(
    agent: Agent,
    executable: &Path,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
    timeout: Duration,
) -> io::Result<AgentReadiness> {
    let mut command = Command::new(executable);
    command
        .args(agent.args)
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if matches!(agent.check, Check::ClaudeAuth) {
        command.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    }
    let mut reader = if matches!(agent.check, Check::Installed) {
        None
    } else {
        // Keep potentially sensitive auth output in a bounded, anonymous pipe.
        let (reader, writer) = io::pipe()?;
        #[cfg(unix)]
        make_nonblocking(&reader)?;
        if matches!(agent.check, Check::CodexAuth) {
            command.stderr(writer.try_clone()?);
        }
        command.stdout(writer);
        Some(reader)
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(windows)]
    let job = windows::Job::create();
    let mut child = command.spawn()?;
    #[cfg(windows)]
    let job = job.filter(|job| job.attach(&child));
    drop(command); // Close the parent's copies of the pipe writers.
    let result = wait_for_check(&mut child, reader.as_mut(), agent.check, timeout);
    if result.is_err() {
        #[cfg(unix)]
        // The child still owns its process group ID until reaped.
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
        #[cfg(windows)]
        if let Some(job) = job {
            job.terminate();
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn wait_for_check(
    child: &mut Child,
    mut reader: Option<&mut PipeReader>,
    check: Check,
    timeout: Duration,
) -> io::Result<AgentReadiness> {
    let mut output = Vec::new();
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(reader) = reader.as_deref_mut() {
            drain_output(reader, &mut output)?;
        }
        if output.len() > MAX_OUTPUT_BYTES {
            return Err(io::Error::other("CLI status output exceeded its limit"));
        }
        if let Some(status) = child.try_wait()? {
            // The process may have written its final status after the last read.
            let complete = reader.is_none_or(|reader| drain_output(reader, &mut output).is_ok());
            return Ok(if complete && output.len() <= MAX_OUTPUT_BYTES {
                classify(check, status.code(), &output)
            } else {
                AgentReadiness::StartFailed
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        thread::sleep(remaining.min(Duration::from_millis(20)));
    }
}

fn classify(check: Check, code: Option<i32>, output: &[u8]) -> AgentReadiness {
    use AgentReadiness::*;
    match check {
        Check::CodexAuth => match code {
            Some(0) => Ready,
            Some(1)
                if String::from_utf8_lossy(output)
                    .lines()
                    .any(|line| line.trim() == "Not logged in") =>
            {
                NotLoggedIn
            }
            _ => StartFailed,
        },
        Check::ClaudeAuth => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct AuthStatus {
                logged_in: bool,
            }
            let auth = serde_json::from_slice::<AuthStatus>(output);
            match (code, auth.map(|status| status.logged_in)) {
                (Some(0), Ok(true)) => Ready,
                (Some(1), Ok(false)) => NotLoggedIn,
                _ => StartFailed,
            }
        }
        Check::Installed if code == Some(0) => Installed,
        Check::Installed => StartFailed,
    }
}

#[cfg(unix)]
fn make_nonblocking(reader: &PipeReader) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = reader.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn drain_output(reader: &mut PipeReader, output: &mut Vec<u8>) -> io::Result<()> {
    let remaining = (MAX_OUTPUT_BYTES + 1 - output.len()) as u64;
    match reader.take(remaining).read_to_end(output) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn drain_output(reader: &mut PipeReader, output: &mut Vec<u8>) -> io::Result<()> {
    let available = windows::available_output(reader)?;
    let remaining = (MAX_OUTPUT_BYTES + 1 - output.len()) as u64;
    reader.take(remaining.min(available)).read_to_end(output)?;
    Ok(())
}

#[cfg(windows)]
mod windows {
    use std::{
        ffi::c_void,
        io::{self, PipeReader},
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
        process::Child,
        ptr,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn PeekNamedPipe(
            pipe: RawHandle,
            buffer: *mut c_void,
            size: u32,
            bytes_read: *mut u32,
            bytes_available: *mut u32,
            bytes_left: *mut u32,
        ) -> i32;
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> RawHandle;
        fn AssignProcessToJobObject(job: RawHandle, process: RawHandle) -> i32;
        fn TerminateJobObject(job: RawHandle, exit_code: u32) -> i32;
    }

    pub fn available_output(pipe: &PipeReader) -> io::Result<u64> {
        let mut available = 0;
        let result = unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        };
        if result != 0 {
            return Ok(u64::from(available));
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(109 | 233) => Ok(0), // ERROR_BROKEN_PIPE or ERROR_PIPE_NOT_CONNECTED
            _ => Err(error),
        }
    }

    // Also stop the CLI launched by a .cmd/.bat wrapper if a probe times out.
    pub struct Job(OwnedHandle);

    impl Job {
        pub fn create() -> Option<Self> {
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return None;
            }
            Some(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
        }

        pub fn attach(&self, child: &Child) -> bool {
            unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) != 0 }
        }

        pub fn terminate(self) {
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use AgentReadiness::*;

    fn cli(directory: &Path, name: &str, unix: &str, batch: &str) -> PathBuf {
        fs::create_dir_all(directory).unwrap();
        let path = directory.join(name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = batch;
            fs::write(&path, format!("#!/bin/sh\n{unix}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
        #[cfg(windows)]
        {
            let _ = unix;
            let path = path.with_extension("cmd");
            fs::write(
                &path,
                format!("@echo off\r\n{}\r\n", batch.replace('\n', "\r\n")),
            )
            .unwrap();
            path
        }
    }

    fn environment(root: &Path, bin: &Path) -> BTreeMap<OsString, OsString> {
        let vars = [
            ("PATH".into(), std::env::join_paths([bin]).unwrap()),
            ("HOME".into(), root.as_os_str().to_owned()),
            ("USERPROFILE".into(), root.as_os_str().to_owned()),
            ("MATOU_DISABLE_LOGIN_PATH_PROBE".into(), "1".into()),
        ];
        #[cfg(windows)]
        let vars = vars
            .into_iter()
            .chain(std::env::var_os("SystemRoot").map(|root| ("SystemRoot".into(), root)));
        vars.into_iter().collect()
    }

    fn states(
        cwd: &Path,
        env: BTreeMap<OsString, OsString>,
    ) -> BTreeMap<&'static str, AgentReadiness> {
        probe(cwd, env)
            .into_iter()
            .map(|status| (status.id, status.readiness))
            .collect()
    }

    #[test]
    fn finds_the_cli_on_path_and_distinguishes_an_install_outside_path() {
        let root = tempfile::Builder::new()
            .prefix("首屏 测试 ")
            .tempdir()
            .unwrap();
        let home = root.path();
        let visible = home.join("CLI 路径");
        cli(
            &visible,
            "codex",
            r#"[ "$*" = "login status" ] || exit 9; printf 'Logged in using ChatGPT\n' >&2"#,
            "if not \"%*\"==\"login status\" exit /b 9\necho Logged in using ChatGPT 1>&2\nexit /b 0",
        );
        cli(&home.join(".local/bin"), "agy", "exit 9", "exit /b 9");
        let result = states(home, environment(home, &visible));
        assert_eq!(
            result,
            BTreeMap::from([
                ("codex", Ready),
                ("claude-code", CliNotFound),
                ("dsh", CliNotFound),
                ("antigravity", PathUnavailable),
            ]),
        );
    }

    #[test]
    fn reports_real_logout_separately_from_a_failing_executable() {
        let root = tempfile::Builder::new()
            .prefix("状态 测试 ")
            .tempdir()
            .unwrap();
        let cwd = root.path();
        let visible = cwd.join("CLI 路径");
        let claude = cli(
            &cwd.join("explicit"),
            "claude-custom",
            r#"[ "$*" = "auth status --json" ] || exit 9; printf '{"loggedIn":false,"email":"ignored-fixture"}\n'; exit 1"#,
            "if not \"%*\"==\"auth status --json\" exit /b 9\necho {\"loggedIn\":false,\"email\":\"ignored-fixture\"}\nexit /b 1",
        );
        cli(
            &visible,
            "codex",
            "echo 'Configuration is invalid' >&2; exit 1",
            "echo Configuration is invalid 1>&2\nexit /b 1",
        );
        cli(
            &visible,
            "dsh",
            "test \"$*\" = '--version'",
            "if not \"%*\"==\"--version\" exit /b 9\nexit /b 0",
        );
        cli(
            &visible,
            "agy",
            "test \"$*\" = '--help'",
            "if not \"%*\"==\"--help\" exit /b 9\nexit /b 0",
        );
        let mut env = environment(cwd, &visible);
        env.insert("MATOU_CLAUDE_COMMAND".into(), claude.into_os_string());
        let result = states(cwd, env);
        assert_eq!(
            result,
            BTreeMap::from([
                ("codex", StartFailed),
                ("claude-code", NotLoggedIn),
                ("dsh", Installed),
                ("antigravity", Installed),
            ]),
        );
    }

    #[test]
    fn validates_auth_status_without_accepting_arbitrary_cli_errors() {
        assert_eq!(
            classify(Check::CodexAuth, Some(1), b"Not logged in\r\n"),
            NotLoggedIn
        );
        assert_eq!(
            classify(Check::CodexAuth, Some(1), b"Error checking login status"),
            StartFailed
        );
        assert_eq!(
            classify(
                Check::ClaudeAuth,
                Some(0),
                br#"{"loggedIn":true,"token":"ignored-fixture"}"#
            ),
            Ready
        );
        assert_eq!(
            classify(
                Check::ClaudeAuth,
                Some(1),
                br#"{"error":"credential store unavailable"}"#
            ),
            StartFailed
        );
        assert_eq!(
            classify(Check::ClaudeAuth, Some(2), br#"{"loggedIn":false}"#),
            StartFailed
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_status_check_is_stopped_at_its_deadline() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path();
        let executable = cli(cwd, "codex", "exec /bin/sleep 10", "");
        let result = run_check(
            AGENTS[0],
            &executable,
            cwd,
            &environment(cwd, cwd),
            Duration::from_millis(50),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
}
