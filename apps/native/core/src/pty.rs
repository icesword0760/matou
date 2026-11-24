use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub(crate) const CALLER_SESSION_MARKERS: [&str; 13] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_PID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_NAME",
    "CLAUDE_CODE_SESSION_KIND",
    "CLAUDE_BG_SOURCE",
    "CLAUDE_BG_BACKEND",
    "CLAUDE_BG_RENDEZVOUS_SOCK",
    "CLAUDE_CODE_RESUME_PROMPT",
    "CLAUDE_CODE_RESUME_INTERRUPTED_TURN",
];

const INHERITED_PROVIDER_MARKERS: [&str; 5] = [
    "MATOU_AGENT_STATUS_DIR",
    "MATOU_FOREMAN_ROLES_FILE",
    "MATOU_PROVIDER_HOOK_URL",
    "MATOU_PROVIDER_HOOK_TOKEN",
    "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE",
];

const ZSH_ENV: &str = r#"if [[ -n "$MATOU_ORIGINAL_ZDOTDIR" && "$MATOU_ORIGINAL_ZDOTDIR" != "$ZDOTDIR" && -r "$MATOU_ORIGINAL_ZDOTDIR/.zshenv" ]]; then
  source "$MATOU_ORIGINAL_ZDOTDIR/.zshenv"
fi
"#;

const ZSH_RC: &str = r#"if [[ -n "$MATOU_ORIGINAL_ZDOTDIR" && "$MATOU_ORIGINAL_ZDOTDIR" != "$ZDOTDIR" && -r "$MATOU_ORIGINAL_ZDOTDIR/.zshrc" ]]; then
  source "$MATOU_ORIGINAL_ZDOTDIR/.zshrc"
fi

_matou_preexec() {
  local _matou_command_base64
  _matou_command_base64=$(printf '%s' "$1" | base64 | tr -d '\n')
  printf '\033]633;E;%s\007' "$_matou_command_base64"
  printf '\033]133;C\007'
}

_matou_precmd() {
  local _matou_exit_code=$?
  printf '\033]133;D;%d\007\033]133;A\007' "$_matou_exit_code"
}

typeset -ga preexec_functions precmd_functions
preexec_functions=(_matou_preexec ${preexec_functions:#_matou_preexec})
precmd_functions=(_matou_precmd ${precmd_functions:#_matou_precmd})

unalias cc 2>/dev/null
cc() {
  if [[ $# -eq 0 && -n "$MATOU_CONTROL_ENDPOINT" && -n "$MATOU_CONTROL_TOKEN" ]] && command -v mt >/dev/null 2>&1; then
    mt create session --canvas current --profile claude-code --permission-mode bypassPermissions --cwd "$PWD" --enter >/dev/null
    return $?
  fi
  claude --dangerously-skip-permissions "$@"
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchProfile {
    Shell,
    ClaudeCode,
    Codex,
    Dsh,
    Antigravity,
}

impl LaunchProfile {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Dsh => "dsh",
            Self::Antigravity => "antigravity",
        }
    }
}

pub struct ShellLaunch {
    pub profile: LaunchProfile,
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub data_root: PathBuf,
    pub inherited_env: BTreeMap<OsString, OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub cols: u16,
    pub rows: u16,
}

/// Project the ordinary Shell environment and write the zsh integration when needed.
pub fn prepare_shell_environment(launch: &ShellLaunch) -> Result<BTreeMap<OsString, OsString>> {
    let mut environment = launch.inherited_env.clone();
    for key in INHERITED_PROVIDER_MARKERS {
        remove_var(&mut environment, OsStr::new(key));
    }
    if launch.executable.file_name() == Some(OsStr::new("zsh")) {
        let original = get_var(&launch.inherited_env, "ZDOTDIR")
            .or_else(|| get_var(&launch.inherited_env, "HOME"))
            .map(OsStr::to_owned)
            .or_else(|| std::env::home_dir().map(PathBuf::into_os_string))
            .context("resolve the original zsh configuration directory")?;
        let directory = launch.data_root.join("shell-integration").join("zsh");
        write_zsh_integration(&directory)?;
        set_var(
            &mut environment,
            "ZDOTDIR".into(),
            directory.into_os_string(),
        );
        set_var(&mut environment, "MATOU_ORIGINAL_ZDOTDIR".into(), original);
    }
    for (key, value) in &launch.env {
        set_var(&mut environment, key.clone(), value.clone());
    }
    for key in CALLER_SESSION_MARKERS {
        remove_var(&mut environment, OsStr::new(key));
    }
    // node-pty also projected the Unix working directory into PWD.
    #[cfg(unix)]
    set_var(
        &mut environment,
        "PWD".into(),
        launch.cwd.clone().into_os_string(),
    );
    set_var(&mut environment, "TERM".into(), "xterm-256color".into());
    Ok(environment)
}

fn write_zsh_integration(directory: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(directory)
        .with_context(|| format!("create Shell integration directory {}", directory.display()))?;

    for (name, contents) in [(".zshenv", ZSH_ENV), (".zshrc", ZSH_RC)] {
        let path = directory.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&path)
            .and_then(|mut script| script.write_all(contents.as_bytes()))
            .with_context(|| format!("write Shell integration {}", path.display()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn matches_key(left: &OsStr, right: &OsStr) -> bool {
    if let (Some(left), Some(right)) = (left.to_str(), right.to_str()) {
        return left.eq_ignore_ascii_case(right);
    }
    left == right
}

/// Look up a variable using the target platform's environment key semantics.
pub fn get_var<'a>(environment: &'a BTreeMap<OsString, OsString>, key: &str) -> Option<&'a OsStr> {
    get_var_for_platform(environment, key, cfg!(windows))
}

fn get_var_for_platform<'a>(
    environment: &'a BTreeMap<OsString, OsString>,
    key: &str,
    windows: bool,
) -> Option<&'a OsStr> {
    if windows {
        return environment
            .iter()
            .find(|(name, _)| {
                name.to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(key))
            })
            .map(|(_, value)| value.as_os_str());
    }
    environment.get(OsStr::new(key)).map(OsString::as_os_str)
}

#[test]
fn windows_environment_lookup_accepts_mixed_case_keys() {
    let environment = BTreeMap::from([
        ("Path".into(), "cli directory".into()),
        ("Codex_Home".into(), "config directory".into()),
        ("UserProfile".into(), "user directory".into()),
        ("Home".into(), "home directory".into()),
        ("Antigravity_Home".into(), "provider directory".into()),
    ]);
    for (key, value) in [
        ("PATH", "cli directory"),
        ("CODEX_HOME", "config directory"),
        ("USERPROFILE", "user directory"),
        ("HOME", "home directory"),
        ("ANTIGRAVITY_HOME", "provider directory"),
    ] {
        assert_eq!(
            get_var_for_platform(&environment, key, true),
            Some(OsStr::new(value))
        );
        assert_eq!(get_var_for_platform(&environment, key, false), None);
    }
}

#[cfg(unix)]
fn ensure_utf8_locale(environment: &mut BTreeMap<OsString, OsString>) {
    let utf8 = ["LC_ALL", "LC_CTYPE", "LANG"].into_iter().any(|name| {
        get_var(environment, name).is_some_and(|value| {
            let locale = value.to_string_lossy().to_ascii_lowercase();
            locale.contains("utf-8") || locale.contains("utf8")
        })
    });
    if !utf8 {
        set_var(environment, "LANG".into(), "en_US.UTF-8".into());
    }
}

#[cfg(all(test, unix))]
#[test]
fn pty_locale_defaults_to_utf8_and_preserves_explicit_utf8() {
    let mut env = BTreeMap::new();
    ensure_utf8_locale(&mut env);
    assert_eq!(get_var(&env, "LANG"), Some(OsStr::new("en_US.UTF-8")));
    env.insert("LANG".into(), "zh_CN.UTF-8".into());
    ensure_utf8_locale(&mut env);
    assert_eq!(get_var(&env, "LANG"), Some(OsStr::new("zh_CN.UTF-8")));
    env.remove(OsStr::new("LANG"));
    env.insert("LC_CTYPE".into(), "zh_CN.uTf8".into());
    ensure_utf8_locale(&mut env);
    assert!(get_var(&env, "LANG").is_none());
}

pub(crate) fn remove_var(environment: &mut BTreeMap<OsString, OsString>, key: &OsStr) {
    #[cfg(windows)]
    environment.retain(|name, _| !matches_key(name, key));
    #[cfg(not(windows))]
    environment.remove(key);
}

/// Set a variable, replacing case variants on Windows.
pub fn set_var(environment: &mut BTreeMap<OsString, OsString>, key: OsString, value: OsString) {
    #[cfg(windows)]
    remove_var(environment, &key);
    environment.insert(key, value);
}

struct Handles {
    child: Box<dyn Child + Send + Sync>,
    writer: Option<Box<dyn Write + Send>>,
    master: Box<dyn MasterPty + Send>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessExit {
    pub exit_code: i32,
    /// Unix reports a numeric signal (zero for a normal exit). ConPTY omits it.
    pub signal: Option<i32>,
}

#[cfg(unix)]
impl From<std::process::ExitStatus> for ProcessExit {
    fn from(status: std::process::ExitStatus) -> Self {
        use std::os::unix::process::ExitStatusExt;

        Self {
            exit_code: status.code().unwrap_or(0),
            signal: Some(status.signal().unwrap_or(0)),
        }
    }
}

#[cfg(windows)]
impl From<portable_pty::ExitStatus> for ProcessExit {
    fn from(status: portable_pty::ExitStatus) -> Self {
        Self {
            exit_code: status.exit_code() as i32,
            signal: None,
        }
    }
}

#[cfg(unix)]
fn std_child(child: &mut dyn Child) -> Result<&mut std::process::Child> {
    child
        .downcast_mut()
        .context("Unix Shell PTY backend did not return a std::process::Child")
}

pub struct PtyProcess {
    // Takes ownership of all handles until they can safely be closed and the child reaped.
    handles: Option<Handles>,
}

#[cfg(unix)]
pub struct ForegroundProcessGroup {
    pgid: libc::pid_t,
    session: libc::pid_t,
}

#[cfg(unix)]
impl ForegroundProcessGroup {
    pub fn hang_up(&self) -> io::Result<()> {
        // The direct Shell receives its own HUP through PtyProcess::terminate.
        if self.pgid == self.session {
            return Ok(());
        }
        self.signal_if_running(
            &[libc::SIGCONT, libc::SIGHUP],
            || self.is_running(),
            try_send_signal,
        )
    }

    pub fn force_kill(&self) -> io::Result<()> {
        self.signal_if_running(&[libc::SIGKILL], || self.is_running(), try_send_signal)
    }

    fn signal_if_running(
        &self,
        signals: &[libc::c_int],
        mut is_running: impl FnMut() -> io::Result<bool>,
        mut send: impl FnMut(libc::pid_t, libc::c_int) -> io::Result<()>,
    ) -> io::Result<()> {
        for &signal in signals {
            let sent = is_running().and_then(|running| {
                if running {
                    send(-self.pgid, signal)?;
                }
                Ok(running)
            });
            match sent {
                Ok(true) => {}
                Ok(false) => break,
                // Membership can change between the check and the signal.
                Err(error) if matches!(error.raw_os_error(), Some(libc::ESRCH | libc::EPERM)) => {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn is_running(&self) -> io::Result<bool> {
        #[cfg(target_os = "macos")]
        {
            // The leader can exit before other members. Confirm that any
            // surviving member is still in this PTY's original session.
            let mut members = vec![0; 16];
            loop {
                let count = unsafe {
                    libc::proc_listpgrppids(
                        self.pgid,
                        members.as_mut_ptr().cast(),
                        std::mem::size_of_val(members.as_slice()) as libc::c_int,
                    )
                };
                if count < 0 {
                    return Err(io::Error::last_os_error());
                }
                let count = count as usize;
                if members[..count]
                    .iter()
                    .any(|&pid| unsafe { libc::getsid(pid) } == self.session)
                {
                    return Ok(true);
                }
                if count < members.len() {
                    return Ok(false);
                }
                members.resize(members.len() * 2, 0);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            if unsafe { libc::kill(-self.pgid, 0) } == 0 {
                return Ok(true);
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ESRCH) => Ok(false),
                Some(libc::EPERM) => Ok(true),
                _ => Err(error),
            }
        }
    }
}

impl PtyProcess {
    pub fn spawn(launch: &ShellLaunch) -> Result<(Self, Box<dyn Read + Send>)> {
        let geometry = pty_size(launch.cols, launch.rows)?;
        let label = match launch.profile {
            LaunchProfile::Shell => "Shell",
            LaunchProfile::ClaudeCode => "Claude Code",
            LaunchProfile::Codex => "Codex",
            LaunchProfile::Dsh => "DSH",
            LaunchProfile::Antigravity => "Antigravity",
        };
        // portable-pty falls back to HOME if cwd is invalid. Check it at spawn.
        let metadata = fs::metadata(&launch.cwd)
            .with_context(|| format!("open {label} working directory {}", launch.cwd.display()))?;
        ensure!(
            metadata.is_dir(),
            "{label} working directory {} is not a directory",
            launch.cwd.display()
        );

        let mut command = CommandBuilder::new(&launch.executable);
        let environment = match launch.profile {
            LaunchProfile::Shell => {
                let environment = prepare_shell_environment(launch)?;
                #[cfg(unix)]
                {
                    if get_var(&environment, "SHELL").is_none() {
                        // Preserve an unset SHELL despite portable-pty's passwd fallback.
                        let executable = executable_for_unset_shell(launch, &environment)?;
                        command = CommandBuilder::new("/usr/bin/env");
                        command.args([
                            OsStr::new("-u"),
                            OsStr::new("SHELL"),
                            executable.as_os_str(),
                        ]);
                    }
                    if launch.argv.is_empty() {
                        command.arg("-l");
                    }
                }
                environment
            }
            LaunchProfile::ClaudeCode | LaunchProfile::Dsh | LaunchProfile::Antigravity => {
                launch.env.clone()
            }
            LaunchProfile::Codex => {
                let mut environment = launch.env.clone();
                for key in INHERITED_PROVIDER_MARKERS
                    .into_iter()
                    .chain(CALLER_SESSION_MARKERS)
                {
                    if key == "MATOU_AGENT_STATUS_DIR"
                        && get_var(&environment, key).is_some_and(|value| {
                            let path = Path::new(value);
                            path.parent().is_some_and(|parent| {
                                parent == launch.data_root.join("agent-status")
                            }) && path.is_dir()
                        })
                    {
                        continue;
                    }
                    remove_var(&mut environment, OsStr::new(key));
                }
                environment
            }
        };
        #[cfg(unix)]
        let environment = {
            let mut environment = environment;
            ensure_utf8_locale(&mut environment);
            environment
        };
        command.args(&launch.argv);
        command.cwd(&launch.cwd);
        command.env_clear();
        for (key, value) in environment {
            command.env(key, value);
        }

        let pair = native_pty_system()
            .openpty(geometry)
            .with_context(|| format!("open {label} PTY"))?;
        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("start {label} {}", launch.executable.display()))?;
        drop(pair.slave);

        let mut process = Self {
            handles: Some(Handles {
                child,
                writer: None,
                master: pair.master,
            }),
        };
        let handles = process.handles_mut();
        let reader = handles
            .master
            .try_clone_reader()
            .context("open Shell PTY output")?;
        handles.writer = Some(
            handles
                .master
                .take_writer()
                .context("open Shell PTY input")?,
        );
        Ok((process, reader))
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        let writer = self
            .handles_mut()
            .writer
            .as_mut()
            .expect("spawned Shell writer");
        writer.write_all(data).context("write Shell PTY input")?;
        writer.flush().context("flush Shell PTY input")
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.handles
            .as_ref()
            .expect("spawned Shell handles")
            .master
            .resize(pty_size(cols, rows)?)
            .context("resize Shell PTY")
    }

    pub fn process_id(&self) -> Option<u32> {
        self.handles.as_ref()?.child.process_id()
    }

    #[cfg(unix)]
    pub fn foreground_process_group(&self) -> Option<ForegroundProcessGroup> {
        let handles = self.handles.as_ref()?;
        let session = unix_pid(&*handles.child).ok()?;
        // The Unix PTY backend reads tcgetpgrp on this master and creates
        // the controlling session with the direct child as its leader.
        let pgid = handles.master.process_group_leader()?;
        (pgid > 1 && pgid != unsafe { libc::getpgrp() })
            .then_some(ForegroundProcessGroup { pgid, session })
    }

    pub fn try_wait(&mut self) -> Result<Option<ProcessExit>> {
        let child = &mut *self.handles_mut().child;
        #[cfg(unix)]
        let child = std_child(child)?;
        let status = child.try_wait().context("poll Shell process")?;
        Ok(status.map(ProcessExit::from))
    }

    pub fn wait(&mut self) -> Result<ProcessExit> {
        let child = &mut *self.handles_mut().child;
        #[cfg(unix)]
        let child = std_child(child)?;
        let status = child.wait().context("wait for Shell process")?;
        Ok(ProcessExit::from(status))
    }

    /// Request termination. The child status is determined by try_wait or wait.
    pub fn terminate(&mut self) -> Result<()> {
        if self.try_wait()?.is_none() {
            request_termination(&mut *self.handles_mut().child)?;
        }
        Ok(())
    }

    pub fn pause_execution(&self) -> Result<()> {
        #[cfg(unix)]
        {
            let child = &*self.handles.as_ref().expect("spawned Shell handles").child;
            send_signal(-unix_pid(child)?, libc::SIGSTOP)
                .context("pause the Shell process group")?;
        }
        // ConPTY output admission is paused by the session owner.
        Ok(())
    }

    pub fn resume_execution(&self) -> Result<()> {
        #[cfg(unix)]
        {
            let child = &*self.handles.as_ref().expect("spawned Shell handles").child;
            send_signal(-unix_pid(child)?, libc::SIGCONT)
                .context("resume the Shell process group")?;
        }
        Ok(())
    }

    /// Force the child to exit. The caller still observes and reaps its status.
    pub fn force_kill(&mut self) -> Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        let child = &mut *self.handles_mut().child;
        #[cfg(unix)]
        send_signal(unix_pid(child)?, libc::SIGKILL).context("force termination of the Shell")?;
        #[cfg(windows)]
        child.kill().context("force termination of the Shell")?;
        Ok(())
    }

    fn handles_mut(&mut self) -> &mut Handles {
        self.handles.as_mut().expect("spawned Shell handles")
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        let Some(mut handles) = self.handles.take() else {
            return;
        };
        if matches!(handles.child.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Err(error) = request_termination(&mut *handles.child) {
            eprintln!("could not signal the dropped Shell process: {error:#}");
        }
        // Closing a backend writer or waiting on its child may block. Move the handles
        // to a short-lived reaper so a dropped session never holds up the caller.
        let reap = thread::Builder::new()
            .name("matou-pty-reap".into())
            .spawn(move || {
                if let Err(error) = reap_child(&mut *handles.child) {
                    eprintln!("could not reap the dropped Shell process: {error:#}");
                }
                drop(handles);
            });
        if let Err(error) = reap {
            eprintln!("could not start the Shell reaper: {error}");
        }
    }
}

#[cfg(unix)]
fn executable_for_unset_shell(
    launch: &ShellLaunch,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<PathBuf> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    fn executable(path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }
        CString::new(path.as_os_str().as_bytes())
            .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 })
    }

    let file = &launch.executable;
    let cwd = std::path::absolute(&launch.cwd).context("resolve Shell working directory")?;
    let resolved = if file.is_absolute() || file.file_name() != Some(file.as_os_str()) {
        let candidate = cwd.join(file);
        executable(&candidate).then_some(candidate)
    } else {
        get_var(environment, "PATH").and_then(|path| {
            std::env::split_paths(path)
                .map(|entry| cwd.join(entry).join(file))
                .find(|candidate| executable(candidate))
        })
    };
    resolved.with_context(|| {
        format!(
            "start Shell {}: executable missing or not executable",
            file.display()
        )
    })
}

fn pty_size(cols: u16, rows: u16) -> Result<PtySize> {
    ensure!(cols > 0 && rows > 0, "Shell PTY dimensions must be nonzero");
    #[cfg(windows)]
    ensure!(
        i16::try_from(cols).is_ok() && i16::try_from(rows).is_ok(),
        "Shell PTY dimensions exceed ConPTY limits"
    );
    Ok(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}

#[cfg(unix)]
fn unix_pid(child: &dyn Child) -> Result<libc::pid_t> {
    child
        .process_id()
        .and_then(|pid| libc::pid_t::try_from(pid).ok())
        .filter(|&pid| pid > 0)
        .context("Shell process has no usable PID")
}

#[cfg(unix)]
fn send_signal(pid: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    match try_send_signal(pid, signal) {
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        result => result,
    }
}

#[cfg(unix)]
fn try_send_signal(pid: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    // portable-pty establishes the Shell as a new session and process-group leader.
    if unsafe { libc::kill(pid, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn request_termination(child: &mut dyn Child) -> Result<()> {
    #[cfg(unix)]
    {
        let pid = unix_pid(child)?;
        // Resume the process group, then send the same shell-only hangup as the reference.
        let resumed = send_signal(-pid, libc::SIGCONT);
        send_signal(pid, libc::SIGHUP).context("send SIGHUP to the Shell")?;
        resumed.context("resume the Shell process group")?;
    }
    #[cfg(windows)]
    child
        .kill()
        .context("request termination of the Shell process")?;
    Ok(())
}

fn reap_child(child: &mut dyn Child) -> Result<()> {
    if wait_bounded(child, Duration::from_millis(1_500))? {
        return Ok(());
    }
    #[cfg(unix)]
    send_signal(unix_pid(child)?, libc::SIGKILL)
        .context("force termination of the dropped Shell")?;
    #[cfg(windows)]
    child
        .kill()
        .context("force termination of the dropped Shell")?;
    ensure!(
        wait_bounded(child, Duration::from_millis(1_000))?,
        "Shell process did not exit"
    );
    Ok(())
}

fn wait_bounded(child: &mut dyn Child, allowance: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + allowance;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(true),
            Ok(None) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn foreground_group_can_stop_accepting_signals_after_the_membership_check() {
        let group = ForegroundProcessGroup {
            pgid: 42,
            session: 41,
        };
        for signals in [&[libc::SIGCONT, libc::SIGHUP][..], &[libc::SIGKILL][..]] {
            for failed_at in 0..signals.len() {
                for errno in [libc::ESRCH, libc::EPERM, libc::EINVAL] {
                    let checked = Cell::new(false);
                    let mut attempted = Vec::new();
                    let result = group.signal_if_running(
                        signals,
                        || {
                            checked.set(true);
                            Ok(true)
                        },
                        |target, signal| {
                            assert_eq!(target, -group.pgid);
                            assert!(checked.replace(false));
                            attempted.push(signal);
                            if attempted.len() == failed_at + 1 {
                                Err(io::Error::from_raw_os_error(errno))
                            } else {
                                Ok(())
                            }
                        },
                    );
                    assert_eq!(attempted, signals[..=failed_at]);
                    if errno == libc::EINVAL {
                        assert_eq!(result.unwrap_err().raw_os_error(), Some(errno));
                    } else {
                        result.unwrap();
                    }
                }
            }
        }
    }
}
