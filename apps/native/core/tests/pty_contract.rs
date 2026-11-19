use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::Path,
};

use anyhow::Result;
use matou_native_core::pty::{LaunchProfile, ShellLaunch, prepare_shell_environment};

// The real PTY test compares process-wide descriptor counts on failed launches.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn launch(dir: &Path, executable: &str) -> Result<ShellLaunch> {
    let home = dir.join("home");
    let dotfiles = dir.join("zsh-config");
    let cwd = dir.join("workspace");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&dotfiles)?;
    fs::create_dir_all(&cwd)?;
    let inherited_env = BTreeMap::from([
        (OsString::from("HOME"), home.into_os_string()),
        (OsString::from("ZDOTDIR"), dotfiles.into_os_string()),
        ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
        ("TERM".into(), "dumb".into()),
        ("LANG".into(), "C".into()),
    ]);
    Ok(ShellLaunch {
        profile: LaunchProfile::Shell,
        argv: Vec::new(),
        executable: executable.into(),
        cwd,
        data_root: dir.join("data"),
        inherited_env,
        env: BTreeMap::new(),
        cols: 80,
        rows: 24,
    })
}

#[test]
fn environment_preserves_options_and_filters_caller_state_in_source_order() -> Result<()> {
    let _guard = TEST_LOCK.lock().unwrap();
    let temp = tempfile::tempdir()?;
    let mut launch = launch(temp.path(), "/bin/zsh")?;
    let caller_markers = [
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
    for marker in caller_markers {
        launch
            .inherited_env
            .insert(marker.into(), "inherited".into());
        launch.env.insert(marker.into(), "options".into());
    }
    for marker in [
        "MATOU_AGENT_STATUS_DIR",
        "MATOU_PROVIDER_HOOK_URL",
        "MATOU_PROVIDER_HOOK_TOKEN",
        "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE",
    ] {
        launch
            .inherited_env
            .insert(marker.into(), "inherited".into());
    }
    launch
        .inherited_env
        .insert("KEEP".into(), "inherited-non-marker".into());
    launch
        .inherited_env
        .insert("PWD".into(), "previous-directory".into());
    launch
        .env
        .insert("MATOU_AGENT_STATUS_DIR".into(), "new-options".into());
    launch.env.insert("TERM".into(), "wrong-options".into());
    launch.env.insert(
        "HOME".into(),
        temp.path().join("other-home").into_os_string(),
    );
    let generated_dir = launch.data_root.join("shell-integration").join("zsh");
    let original = launch.inherited_env[OsStr::new("ZDOTDIR")].clone();
    let projected = prepare_shell_environment(&launch)?;
    for marker in caller_markers {
        assert!(projected.get(OsStr::new(marker)).is_none(), "{marker}");
    }
    assert_eq!(
        projected[OsStr::new("MATOU_AGENT_STATUS_DIR")],
        "new-options"
    );
    for marker in [
        "MATOU_PROVIDER_HOOK_URL",
        "MATOU_PROVIDER_HOOK_TOKEN",
        "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE",
    ] {
        assert!(projected.get(OsStr::new(marker)).is_none(), "{marker}");
    }
    assert_eq!(projected[OsStr::new("TERM")], "xterm-256color");
    assert_eq!(projected[OsStr::new("KEEP")], "inherited-non-marker");
    assert_eq!(
        projected[OsStr::new("HOME")],
        launch.env[OsStr::new("HOME")]
    );
    assert_eq!(projected[OsStr::new("ZDOTDIR")], generated_dir);
    assert_eq!(projected[OsStr::new("MATOU_ORIGINAL_ZDOTDIR")], original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(projected[OsStr::new("PWD")], launch.cwd.as_os_str());
        assert_eq!(
            fs::metadata(&generated_dir)?.permissions().mode() & 0o777,
            0o700
        );
        for file in [".zshenv", ".zshrc"] {
            assert_eq!(
                fs::metadata(generated_dir.join(file))?.permissions().mode() & 0o777,
                0o600
            );
        }
    }
    launch.env.insert(
        "ZDOTDIR".into(),
        temp.path().join("explicit").into_os_string(),
    );
    let projected = prepare_shell_environment(&launch)?;
    assert_eq!(
        projected[OsStr::new("ZDOTDIR")],
        launch.env[OsStr::new("ZDOTDIR")]
    );

    let plain = temp.path().join("non-zsh");
    launch.executable = "/bin/sh".into();
    launch.data_root = plain.clone();
    let projected = prepare_shell_environment(&launch)?;
    assert!(!plain.exists());
    assert!(!projected.contains_key(OsStr::new("MATOU_ORIGINAL_ZDOTDIR")));
    Ok(())
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use anyhow::{Context, ensure};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use matou_native_core::pty::{ProcessExit, PtyProcess};
    use std::{
        io::{self, Read},
        sync::mpsc::{self, Receiver},
        thread,
        time::{Duration, Instant},
    };

    fn start_reader(
        mut reader: Box<dyn Read + Send>,
    ) -> (Receiver<io::Result<Vec<u8>>>, thread::JoinHandle<()>) {
        let (send, receive) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                let next = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => Ok(buffer[..count].to_vec()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = send.send(Err(error));
                        break;
                    }
                };
                if send.send(next).is_err() {
                    break;
                }
            }
        });
        (receive, worker)
    }

    fn collect_until(receiver: &Receiver<io::Result<Vec<u8>>>, needle: &[u8]) -> Result<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut captured = Vec::new();
        while !captured.windows(needle.len()).any(|span| span == needle) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let chunk = receiver.recv_timeout(remaining).with_context(|| {
                format!(
                    "missing {:?} in {:?}",
                    String::from_utf8_lossy(needle),
                    String::from_utf8_lossy(&captured)
                )
            })??;
            captured.extend(chunk);
        }
        Ok(captured)
    }

    fn finish(process: &mut PtyProcess) -> Result<ProcessExit> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = process.try_wait()? {
                assert_eq!(process.wait()?, status);
                return Ok(status);
            }
            ensure!(Instant::now() < deadline, "real zsh did not exit");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn drain_and_join(
        receiver: Receiver<io::Result<Vec<u8>>>,
        worker: thread::JoinHandle<()>,
    ) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(result) => {
                    result?;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => return Err(error).context("Shell reader did not close after exit"),
            }
        }
        worker.join().expect("Shell reader panicked");
        Ok(())
    }

    #[test]
    fn real_isolated_zsh_input_markers_resize_exit_and_failed_launch() -> Result<()> {
        let _guard = TEST_LOCK.lock().unwrap();
        let temp = tempfile::tempdir()?;
        let missing = temp.path().join("absent-shell");
        let mut launch = launch(temp.path(), missing.to_str().unwrap())?;
        for inherited_shell in [None, Some("/bin/zsh")] {
            if let Some(shell) = inherited_shell {
                launch.inherited_env.insert("SHELL".into(), shell.into());
            }
            let before = fs::read_dir("/dev/fd")?.count();
            let Err(error) = PtyProcess::spawn(&launch) else {
                anyhow::bail!("missing executable unexpectedly launched");
            };
            assert!(error.to_string().contains("absent-shell"), "{error:#}");
            assert_eq!(
                fs::read_dir("/dev/fd")?.count(),
                before,
                "failed launch left PTY handles open"
            );
        }

        launch.executable = "/bin/zsh".into();
        assert!(launch.argv.is_empty());
        let dotfiles = std::path::PathBuf::from(&launch.inherited_env[OsStr::new("ZDOTDIR")]);
        let original_env = "export MATOU_TEST_ENV=env-loaded\n";
        let original_rc = "export MATOU_TEST_RC=rc-loaded\nPROMPT='MATOU_READY> '\n";
        fs::write(dotfiles.join(".zshenv"), original_env)?;
        fs::write(dotfiles.join(".zshrc"), original_rc)?;

        // Run the original shell route and the env exec route for an absent SHELL.
        for expected_shell in ["/bin/zsh", "unset"] {
            if expected_shell == "unset" {
                launch.inherited_env.remove(OsStr::new("SHELL"));
            }
            let (mut process, reader) = PtyProcess::spawn(&launch)?;
            assert!(process.process_id().is_some_and(|pid| pid > 0));
            let (receiver, worker) = start_reader(reader);
            collect_until(&receiver, b"MATOU_READY> ")?;
            let command = r#"printf 'MATOU_OUT_%s:%s:%s:%s:%s\n' "$MATOU_TEST_ENV" "$MATOU_TEST_RC" "$options[login]" "$TERM" "${SHELL-unset}""#;
            process.write(format!("{command}\r").as_bytes())?;
            let expected =
                format!("MATOU_OUT_env-loaded:rc-loaded:on:xterm-256color:{expected_shell}\r\n");
            let completion = b"\x1b]133;D;0\x07";
            let captured = collect_until(&receiver, completion)?;
            let token = format!("\x1b]633;E;{}\x07\x1b]133;C\x07", STANDARD.encode(command));
            let position = |needle: &[u8]| {
                captured
                    .windows(needle.len())
                    .position(|span| span == needle)
                    .with_context(|| {
                        format!(
                            "missing {:?} in {:?}",
                            String::from_utf8_lossy(needle),
                            String::from_utf8_lossy(&captured)
                        )
                    })
            };
            assert!(position(token.as_bytes())? < position(expected.as_bytes())?);
            assert!(position(expected.as_bytes())? < position(completion)?);
            process.resize(97, 33)?;
            process.write(b"stty size\r")?;
            collect_until(&receiver, b"33 97\r\n")?;
            process.write(b"exit 7\r")?;
            assert_eq!(
                finish(&mut process)?,
                ProcessExit {
                    exit_code: 7,
                    signal: Some(0)
                },
            );
            drain_and_join(receiver, worker)?;
        }

        let (mut process, reader) = PtyProcess::spawn(&launch)?;
        let (receiver, worker) = start_reader(reader);
        collect_until(&receiver, b"MATOU_READY> ")?;
        process.write(b"kill -KILL $$\r")?;
        assert_eq!(
            finish(&mut process)?,
            ProcessExit {
                exit_code: 0,
                signal: Some(9)
            },
        );
        drain_and_join(receiver, worker)?;
        assert_eq!(fs::read_to_string(dotfiles.join(".zshenv"))?, original_env);
        assert_eq!(fs::read_to_string(dotfiles.join(".zshrc"))?, original_rc);
        Ok(())
    }
}
