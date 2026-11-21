use crate::flow::Utf8Decoder;
use matou_native_core::pty::LaunchProfile;

#[cfg(unix)]
use std::{
    collections::{BTreeMap, HashMap},
    ffi::{OsStr, OsString},
    io::{ErrorKind, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, LazyLock, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InteractiveLaunch {
    pub profile: LaunchProfile,
    pub mode: &'static str,
}

#[derive(Clone, Default)]
pub(crate) struct ShellInput {
    line: Vec<u16>,
    decoder: Utf8Decoder,
    #[cfg(unix)]
    cc_alias: Option<Arc<ShellAliasEnvironment>>,
}

impl ShellInput {
    pub(crate) fn observe(&mut self, bytes: &[u8]) -> Option<InteractiveLaunch> {
        #[cfg(unix)]
        let cc_alias = self.cc_alias.clone();
        self.observe_with_cc(bytes, || {
            #[cfg(unix)]
            if let Some(environment) = cc_alias {
                return *environment.preheat_cc().wait();
            }
            None
        })
    }

    #[cfg(unix)]
    pub(crate) fn with_shell_environment(launch: &matou_native_core::pty::ShellLaunch) -> Self {
        let mut env = launch.inherited_env.clone();
        env.extend(launch.env.clone());
        Self {
            cc_alias: Some(Arc::new(ShellAliasEnvironment {
                env,
                cwd: launch.cwd.clone(),
                probe: OnceLock::new(),
            })),
            ..Self::default()
        }
    }

    #[cfg(unix)]
    pub(crate) fn preheat_cc(&self) {
        if let Some(environment) = &self.cc_alias
            && matches!(
                Path::new(environment.shell())
                    .file_name()
                    .and_then(OsStr::to_str),
                Some("zsh" | "bash" | "sh" | "dash" | "ash" | "ksh" | "mksh")
            )
        {
            // A configured PTY executable may be an application or a wrapper.
            // Launch those only if the user actually submits the alias.
            environment.preheat_cc();
        }
    }

    fn observe_with_cc(
        &mut self,
        bytes: &[u8],
        resolve_cc: impl FnOnce() -> Option<InteractiveLaunch>,
    ) -> Option<InteractiveLaunch> {
        let mut submitted = None;
        for character in self.decoder.feed(bytes).chars() {
            match character {
                '\r' | '\n' => submitted = Some(std::mem::take(&mut self.line)),
                '\x08' | '\x7f' => {
                    self.line.pop();
                }
                '\x03' | '\x15' => self.line.clear(),
                ' '.. => self
                    .line
                    .extend(character.encode_utf16(&mut [0; 2]).iter().copied()),
                _ => {}
            }
        }
        // The frozen caller forgets the buffer after any submission, including
        // the suffix after the last newline. Only the last command is matched.
        let command = submitted?;
        self.line.clear();
        String::from_utf16(&command)
            .ok()
            .as_deref()
            .and_then(|command| interactive_launch_with_cc(command, resolve_cc))
    }

    pub(crate) fn clear(&mut self) {
        self.line.clear();
    }
}

pub(crate) const CODEX_BYPASS: &str = "--dangerously-bypass-approvals-and-sandbox";

#[cfg(test)]
fn interactive_launch(command: &str) -> Option<InteractiveLaunch> {
    interactive_launch_with_cc(command, || None)
}

fn interactive_launch_with_cc(
    command: &str,
    resolve_cc: impl FnOnce() -> Option<InteractiveLaunch>,
) -> Option<InteractiveLaunch> {
    if crate::dsh_launch::matches_shell_command(command) {
        return Some(InteractiveLaunch {
            profile: LaunchProfile::Dsh,
            mode: "default",
        });
    }

    if let Some(permission) = crate::antigravity_launch::interactive_permission(command) {
        return Some(InteractiveLaunch {
            profile: LaunchProfile::Antigravity,
            mode: permission.as_str(),
        });
    }

    // Match the frozen JavaScript trim and whitespace folding.
    let mut words = command
        .split(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{0085}'))
        .filter(|word| !word.is_empty());
    let (profile, mode) = match (words.next(), words.next(), words.next()) {
        (Some("claude"), None, None) => (LaunchProfile::ClaudeCode, "default"),
        (Some("claude"), Some("--dangerously-skip-permissions"), None) => {
            (LaunchProfile::ClaudeCode, "bypassPermissions")
        }
        (Some("cc"), None, None) => return resolve_cc(),
        (Some("codex"), None, None) => (LaunchProfile::Codex, "default"),
        (Some("codex"), Some(CODEX_BYPASS), None) => (LaunchProfile::Codex, "bypassPermissions"),
        _ => return None,
    };
    Some(InteractiveLaunch { profile, mode })
}

#[cfg(unix)]
struct ShellAliasEnvironment {
    env: BTreeMap<OsString, OsString>,
    cwd: PathBuf,
    probe: OnceLock<AliasProbe>,
}

#[cfg(unix)]
type AliasKey = [Option<OsString>; 3];

#[cfg(unix)]
type AliasProbe = Arc<OnceLock<Option<InteractiveLaunch>>>;

#[cfg(unix)]
static CC_LAUNCHES: LazyLock<Mutex<HashMap<AliasKey, AliasProbe>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(unix)]
impl ShellAliasEnvironment {
    fn shell(&self) -> &OsStr {
        self.env
            .get(OsStr::new("SHELL"))
            .map_or("/bin/zsh".as_ref(), OsString::as_os_str)
    }

    fn preheat_cc(self: &Arc<Self>) -> &AliasProbe {
        self.probe.get_or_init(|| self.start_cc_probe())
    }

    fn start_cc_probe(self: &Arc<Self>) -> AliasProbe {
        let key = ["SHELL", "HOME", "ZDOTDIR"].map(|name| self.env.get(OsStr::new(name)).cloned());
        let mut cache = CC_LAUNCHES.lock().expect("configured alias cache");
        if let Some(probe) = cache.get(&key) {
            return probe.clone();
        }
        let probe = AliasProbe::default();
        let pending = probe.clone();
        let environment = self.clone();
        if thread::Builder::new()
            .name("matou-cc-alias".into())
            .spawn({
                let key = key.clone();
                move || {
                    let launch = environment
                        .probe(environment.shell())
                        .and_then(|output| parse_cc_alias(&output));
                    if launch.is_none() {
                        CC_LAUNCHES
                            .lock()
                            .expect("configured alias cache")
                            .remove(&key);
                    }
                    let _ = pending.set(launch);
                }
            })
            .is_ok()
        {
            cache.insert(key, probe.clone());
        } else {
            let _ = probe.set(None);
        }
        probe
    }

    fn probe(&self, shell: &OsStr) -> Option<String> {
        // The alias must be resolved using the session's shell configuration.
        if self.env.get(OsStr::new("HOME"))?.is_empty() {
            return None;
        }
        let mut child = Command::new(shell)
            .args(["-ic", "alias \"$1\"", "matou-alias", "cc"])
            .current_dir(&self.cwd)
            .env_clear()
            .envs(&self.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .ok()?;
        let mut reaped = false;
        let output = (|| {
            let mut stdout = child.stdout.take()?;
            // SAFETY: the child owns a valid pipe descriptor until stdout is dropped.
            if unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
                return None;
            }
            let deadline = Instant::now() + Duration::from_millis(15_000);
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            while Instant::now() < deadline && bytes.len() <= 64 * 1024 {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => bytes.extend_from_slice(&chunk[..count]),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => return None,
                }
            }
            while Instant::now() < deadline && bytes.len() <= 64 * 1024 {
                if let Some(status) = child.try_wait().ok()? {
                    reaped = true;
                    return status
                        .success()
                        .then(|| String::from_utf8_lossy(&bytes).into_owned());
                }
                thread::sleep(Duration::from_millis(5));
            }
            None
        })();
        if !reaped {
            // SAFETY: process_group(0) put the probe in its own process group.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
        }
        output
    }
}

#[cfg(any(unix, test))]
fn parse_cc_alias(output: &str) -> Option<InteractiveLaunch> {
    let value = output
        .lines()
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("cc=")
                .or_else(|| line.strip_prefix("alias cc="))
        })?
        .trim();
    let alias = value
        .strip_prefix('\'')
        .and_then(|text| text.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|text| text.strip_suffix('"'))
        })
        .map_or_else(|| value.replace("\\ ", " "), str::to_owned);
    interactive_launch_with_cc(&alias, || None)
        .filter(|launch| launch.profile == LaunchProfile::ClaudeCode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cc_only_promotes_recognized_claude_aliases() {
        for (alias, mode) in [
            ("cc=claude", Some("default")),
            (
                "alias cc='claude   --dangerously-skip-permissions'",
                Some("bypassPermissions"),
            ),
            (
                "cc=claude\\ --dangerously-skip-permissions",
                Some("bypassPermissions"),
            ),
            ("alias cc='claude -p'", None),
            ("cc=codex", None),
            ("cc=cc", None),
            ("cc=gcc", None),
        ] {
            assert_eq!(
                ShellInput::default().observe_with_cc(b"  cc  \r", || parse_cc_alias(alias)),
                mode.map(|mode| InteractiveLaunch {
                    profile: LaunchProfile::ClaudeCode,
                    mode
                })
            );
        }
        assert!(
            ShellInput::default()
                .observe_with_cc(b"cc extra\r", || panic!("not an alias invocation"))
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn configured_alias_cache_keeps_distinct_zdotdirs() -> anyhow::Result<()> {
        use std::{fs, os::unix::fs::PermissionsExt};
        let root = tempfile::tempdir()?;
        let dot_dir = root.path().join("first");
        let second_dir = root.path().join("second");
        fs::create_dir(&dot_dir)?;
        fs::create_dir(&second_dir)?;
        let shell = root.path().join("zsh");
        fs::write(
            &shell,
            "#!/bin/sh\n[ \"$#\" -eq 4 ] && [ \"$1\" = '-ic' ] && [ \"$2\" = 'alias \"$1\"' ] && [ \"$3\" = 'matou-alias' ] && [ \"$4\" = 'cc' ] || exit 1\n/bin/sleep 0.9\n/bin/cat \"$ZDOTDIR/alias.stdout\"\n",
        )?;
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
        fs::write(dot_dir.join("alias.stdout"), "cc=claude\n")?;
        fs::write(
            second_dir.join("alias.stdout"),
            "cc='claude --dangerously-skip-permissions'\n",
        )?;
        let input = |zdotdir: &std::path::Path| {
            let mut input =
                ShellInput::with_shell_environment(&matou_native_core::pty::ShellLaunch {
                    profile: LaunchProfile::Shell,
                    executable: shell.clone(),
                    argv: Vec::new(),
                    cwd: root.path().to_owned(),
                    data_root: root.path().to_owned(),
                    inherited_env: BTreeMap::from([
                        ("HOME".into(), root.path().as_os_str().into()),
                        ("SHELL".into(), shell.as_os_str().into()),
                        ("ZDOTDIR".into(), zdotdir.as_os_str().into()),
                    ]),
                    env: BTreeMap::new(),
                    cols: 80,
                    rows: 24,
                });
            input.preheat_cc();
            assert_eq!(input.observe(b"echo still interactive\r"), None);
            input.observe(b"cc\r").map(|launch| launch.mode)
        };
        assert_eq!(input(&dot_dir), Some("default"));
        fs::remove_file(dot_dir.join("alias.stdout"))?;
        assert_eq!(input(&dot_dir), Some("default"));
        assert_eq!(input(&second_dir), Some("bypassPermissions"));
        Ok(())
    }

    #[test]
    fn only_the_exact_provider_commands_are_recognized() {
        for (line, expected) in [
            ("claude", Some("default")),
            ("  claude  ", Some("default")),
            (
                "claude --dangerously-skip-permissions",
                Some("bypassPermissions"),
            ),
            (
                " \u{feff}claude   \t--dangerously-skip-permissions\u{a0}",
                Some("bypassPermissions"),
            ),
            ("claude -p", None),
            ("claude foo", None),
            ("sudo claude", None),
            ("echo claude", None),
            ("CLAUDE", None),
            ("cc", None),
        ] {
            assert_eq!(
                interactive_launch(line).map(|launch| (launch.profile, launch.mode)),
                expected.map(|mode| (LaunchProfile::ClaudeCode, mode)),
                "{line:?}"
            );
        }
    }

    #[test]
    fn codex_requires_the_exact_command_after_normalization() {
        for (command, expected) in [
            ("codex", Some("default")),
            ("  codex\u{a0}", Some("default")),
            (
                "codex --dangerously-bypass-approvals-and-sandbox",
                Some("bypassPermissions"),
            ),
            (
                " \u{feff}codex   \t--dangerously-bypass-approvals-and-sandbox ",
                Some("bypassPermissions"),
            ),
            ("codex --model fixture", None),
            (
                "codex --dangerously-bypass-approvals-and-sandbox extra",
                None,
            ),
            ("codex --dangerously-skip-permissions", None),
            ("env codex", None),
            ("echo codex", None),
            ("./codex", None),
            ("CODEX", None),
        ] {
            assert_eq!(
                interactive_launch(command),
                expected.map(|mode| InteractiveLaunch {
                    profile: LaunchProfile::Codex,
                    mode
                }),
                "{command:?}"
            );
        }
        let mut buffer = ShellInput::default();
        assert_eq!(buffer.observe(b"  co"), None);
        assert_eq!(buffer.observe(b"dex  "), None);
        assert_eq!(
            buffer.observe(b"\r"),
            Some(InteractiveLaunch {
                profile: LaunchProfile::Codex,
                mode: "default"
            })
        );
    }

    #[test]
    fn shell_input_tracks_printable_bytes_and_line_edits_across_writes() {
        for keys in [
            ["clau", "dxx\x7f\x08e", "\r"],
            ["xyz", "\x03claude", "\n"],
            ["xyz", "\x15claude", "\r"],
            ["claud你", "\x7fe", "\r"],
        ] {
            let mut buffer = ShellInput::default();
            assert!(buffer.observe(keys[0].as_bytes()).is_none());
            assert!(buffer.observe(keys[1].as_bytes()).is_none());
            assert_eq!(
                buffer.observe(keys[2].as_bytes()).map(|launch| launch.mode),
                Some("default")
            );
            assert!(buffer.observe(b"\r").is_none());
        }
        let mut buffer = ShellInput::default();
        assert!(buffer.observe(b"claude\x03\r").is_none());
        assert!(buffer.observe(b"claude\x15\n").is_none());
        assert!(buffer.observe(b"CLAUDE\r").is_none());
        assert_eq!(
            buffer
                .observe(b"  claude   --dangerously-skip-permissions  \n")
                .map(|launch| launch.mode),
            Some("bypassPermissions")
        );
    }

    #[test]
    fn only_the_last_submission_in_a_write_can_match() {
        for (bytes, expected) in [
            (&b"claude\r"[..], Some("default")),
            (&b"claude\r\n"[..], None),
            (&b"claude\rls\r"[..], None),
            (&b"touch x\rclaude\r"[..], Some("default")),
            (&b"ls\rclaude\rtail"[..], Some("default")),
        ] {
            assert_eq!(
                ShellInput::default()
                    .observe(bytes)
                    .map(|launch| launch.mode),
                expected,
                "{bytes:?}"
            );
        }
        let mut buffer = ShellInput::default();
        assert_eq!(buffer.observe(b"ls\rclaude"), None);
        assert_eq!(buffer.observe(b"\r"), None);
    }

    #[test]
    fn backspace_removes_one_utf16_code_unit() {
        for (keys, expected) in [
            ("claude😀\x7f\r", None),
            ("claude😀\x08\r", None),
            ("claude😀\x7f\x08\r", Some("default")),
            ("claude\x1b\t\x01\r", Some("default")),
        ] {
            assert_eq!(
                ShellInput::default()
                    .observe(keys.as_bytes())
                    .map(|launch| launch.mode),
                expected
            );
        }
    }

    #[test]
    fn split_chinese_character_takes_one_backspace() {
        let mut buffer = ShellInput::default();
        let (first, second) = "你".as_bytes().split_at(2);
        assert_eq!(buffer.observe(b"claude"), None);
        assert_eq!(buffer.observe(first), None);
        assert_eq!(buffer.observe(second), None);
        assert_eq!(
            buffer.observe(b"\x7f\r").map(|launch| launch.mode),
            Some("default")
        );
    }

    #[test]
    fn interrupt_clears_the_line_and_preserves_a_partial_utf8_character() {
        let mut buffer = ShellInput::default();
        let (first, second) = "你".as_bytes().split_at(2);
        assert_eq!(buffer.observe(b"previous line"), None);
        assert_eq!(buffer.observe(first), None);
        buffer.clear();
        assert_eq!(buffer.observe(second), None);
        assert_eq!(buffer.line, ['你' as u16]);
        assert_eq!(
            buffer.observe(b"\x7fclaude\r").map(|launch| launch.mode),
            Some("default")
        );
    }
}
