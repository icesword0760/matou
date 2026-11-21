use anyhow::{Context, Result, bail, ensure};
use matou_native_runtime::{
    host::{Event, LaunchOptions, OfflineFixtures, RuntimeHost},
    mt_cli::Client,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    host: RuntimeHost,
    endpoint: PathBuf,
    home: PathBuf,
    database: Connection,
    session: String,
    token: String,
    _root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Result<Self> {
        #[cfg(unix)]
        let root = {
            let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .find(|path| path.join(".git").exists())
                .context("locate short worktree socket path")?;
            tempfile::Builder::new()
                .prefix(".tmp-mtw-")
                .tempdir_in(repo)?
        };
        #[cfg(windows)]
        let root = tempfile::tempdir()?;
        let data = root.path().join("data");
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        fs::create_dir(&home)?;
        fs::create_dir(&workspace)?;
        #[cfg(unix)]
        let shell = {
            let shell = root.path().join("shell");
            fs::write(
                &shell,
                r#"#!/bin/sh
umask 077
/bin/stty -echo
identity="$HOME/identity.$MATOU_CONTROL_CALLER_SESSION"
printf '%s\n' "$MATOU_CONTROL_TOKEN" > "$identity.tmp"
/bin/mv "$identity.tmp" "$identity"
printf 'test-terminal-ready\n'
while IFS= read -r line; do
    if [ "$line" = "__exit__" ]; then exit 0; fi
    printf 'ack<%s>\n' "$line"
done
"#,
            )?;
            fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
            shell
        };
        #[cfg(windows)]
        let shell = PathBuf::from(std::env::var_os("SystemRoot").context("Windows system root")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let host = RuntimeHost::start_with_offline_fixtures(
            LaunchOptions {
                data_root: data.clone(),
                workspace,
                shell_home: home.clone(),
                shell,
            },
            OfflineFixtures::default(),
        )?;
        let session = wait(|| {
            while let Ok(event) = host.events.try_recv() {
                match event {
                    Event::Ready { hierarchy, .. } => {
                        return Ok(Some(hierarchy.session.context("initial session")?.id));
                    }
                    Event::Diagnostic(message) => bail!("{message}"),
                    Event::Closed => bail!("isolated runtime closed before readiness"),
                    _ => {}
                }
            }
            Ok(None)
        })?;
        #[cfg(unix)]
        let (endpoint, token) = (
            data.join("control/runtime.sock"),
            read_token(&home, &session)?,
        );
        #[cfg(windows)]
        let (endpoint, token) =
            capture_identity(&host, &data, &home, &session).context("initial Shell identity")?;
        let database = Connection::open_with_flags(
            data.join("matou.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        Ok(Self {
            _root: root,
            host,
            endpoint,
            home,
            database,
            session,
            token,
        })
    }

    fn invoke(&self, token: &str, args: &[&str]) -> Result<Output> {
        Ok(Command::new(env!("CARGO_BIN_EXE_matou-native-runtime"))
            .env_clear()
            .env("MATOU_CONTROL_ENDPOINT", &self.endpoint)
            .env("MATOU_CONTROL_TOKEN", token)
            .arg("--mt")
            .args(args)
            .arg("--json")
            .output()?)
    }

    fn mt(&self, args: &[&str]) -> Result<Value> {
        let out = self.invoke(&self.token, args)?;
        ensure!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stderr.is_empty());
        Ok(serde_json::from_slice(&out.stdout)?)
    }

    fn fault(&self, token: &str, args: &[&str], status: i32, code: &str) -> Result<()> {
        let out = self.invoke(token, args)?;
        assert_eq!(
            out.status.code(),
            Some(status),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stdout.is_empty());
        assert_eq!(serde_json::from_slice::<Value>(&out.stderr)?["code"], code);
        Ok(())
    }

    fn expect_echo(&self, text: &str) -> Result<()> {
        let mut last_screen = String::new();
        wait(|| {
            let screen = self.mt(&["read", "self"])?;
            last_screen = str_field(&screen, "text").to_owned();
            Ok(last_screen.lines().any(|line| line == text).then_some(()))
        })
        .with_context(|| format!("expected terminal line {text:?}, last screen {last_screen:?}"))
    }
}

fn wait<T>(mut probe: impl FnMut() -> Result<Option<T>>) -> Result<T> {
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(result) = probe()? {
            return Ok(result);
        }
        ensure!(
            Instant::now() < until,
            "timed out waiting for isolated runtime"
        );
        thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(windows)]
fn capture_identity(
    host: &RuntimeHost,
    data: &Path,
    home: &Path,
    session: &str,
) -> Result<(PathBuf, String)> {
    let identity = home.join(format!("identity.{session}"));
    let path = identity.to_string_lossy().replace('\'', "''");
    let command = format!(
        "function global:mtAck {{ param([string]$text) Write-Output ('ack<' + $text + '>') }}; [IO.File]::WriteAllLines('{path}', @((Get-Command mt -ErrorAction Stop).Source, $env:MATOU_CONTROL_ENDPOINT, $env:MATOU_CONTROL_TOKEN))"
    );
    host.commands
        .send(matou_native_runtime::host::Command::Input {
            session_id: session.to_owned(),
            bytes: format!("{command}\r").into_bytes(),
            defer_ordering: false,
        })?;
    wait(|| {
        let contents = match fs::read_to_string(&identity) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let lines = contents.lines().collect::<Vec<_>>();
        let [shim, endpoint, token] = lines.as_slice() else {
            return Ok(None);
        };
        ensure!(
            Path::new(shim) == data.join("control/bin/mt.cmd"),
            "unexpected mt shim: {shim}"
        );
        ensure!(
            endpoint.starts_with(r"\\.\pipe\matou-control-"),
            "unexpected endpoint"
        );
        ensure!(!token.is_empty(), "missing managed capabilities");
        Ok(Some((PathBuf::from(endpoint), (*token).to_owned())))
    })
}

#[cfg(unix)]
fn read_token(home: &Path, session: &str) -> Result<String> {
    wait(
        || match fs::read_to_string(home.join(format!("identity.{session}"))) {
            Ok(token) => Ok(Some(token.trim_end().to_owned())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        },
    )
}

fn str_field<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name]
        .as_str()
        .unwrap_or_else(|| panic!("missing {name}: {value}"))
}

#[test]
fn sends_text_and_keys_to_a_live_pty_and_reads_exact_echo() -> Result<()> {
    let mut fixture = Fixture::new()?;
    #[cfg(unix)]
    let (message, draft, exit) = ("输入 λ🙂", "key submit", "__exit__");
    #[cfg(windows)]
    let (message, draft, exit) = ("mtAck '输入 λ🙂'", "mtAck 'key submit'", "exit");
    assert_eq!(
        fixture.mt(&["send", "self", message, "--enter"])?,
        json!({"sent": true})
    );
    fixture.expect_echo("ack<输入 λ🙂>")?;
    assert_eq!(fixture.mt(&["send", "self", draft])?, json!({"sent": true}));
    let draft = fixture.mt(&["read", "self"])?;
    assert!(!str_field(&draft, "text").contains("ack<key submit>"));
    assert_eq!(
        fixture.mt(&["key", "self", "return"])?,
        json!({"sent": true})
    );
    fixture.expect_echo("ack<key submit>")?;
    fixture.fault(
        &fixture.token,
        &["key", "self", "F13"],
        2,
        "INVALID_REQUEST",
    )?;

    let client = Client::new(&fixture.endpoint, &fixture.token).unwrap();
    let error = client
        .request(
            "task.status.write",
            json!({"taskId": "denied", "key": "phase", "value": "running"}),
        )
        .unwrap_err();
    assert!(format!("{error:?}").contains("CAPABILITY_DENIED"));
    fixture.mt(&["send", "self", exit, "--enter"])?;
    wait(|| {
        let count: i64 = fixture.database.query_row(
            "SELECT COUNT(*) FROM session_runs WHERE session_id = ? AND ended_at IS NOT NULL",
            [&fixture.session],
            |row| row.get(0),
        )?;
        Ok((count == 1).then_some(()))
    })
    .context("Shell exited after submitting exit command")?;
    fixture.fault(
        &fixture.token,
        &["send", "self", "after exit"],
        4,
        "CAPABILITY_DENIED",
    )?;
    fixture.host.shutdown()?;
    Ok(())
}

#[test]
fn creates_lists_removes_and_closes_through_the_managed_cli() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let last = fixture.mt(&["close", "canvas-preview", "current"])?;
    assert_eq!(last["impact"]["canvases"], 0);
    assert_eq!(last["impact"]["sessions"], 0);
    let last_ref = str_field(&last, "confirmationRef");
    assert_eq!(
        fixture.mt(&["close", "canvas-commit", last_ref])?["removedSessions"],
        0
    );
    fixture.fault(
        &fixture.token,
        &["close", "canvas-commit", last_ref],
        4,
        "CONFIRMATION_REQUIRED",
    )?;

    let root = fixture.home.join("another-workspace");
    fs::create_dir(&root)?;
    let dir = root.to_str().unwrap();
    let workspace_args = [
        "create",
        "workspace",
        "--path",
        dir,
        "--title",
        "Project",
        "--submission-key",
        "workspace",
    ];
    let workspace = fixture.mt(&workspace_args)?;
    assert_eq!(workspace, fixture.mt(&workspace_args)?);
    assert_eq!(workspace["path"]["workspace"]["title"], "Project");
    let workspace = str_field(&workspace, "createdRef");
    let task_args = [
        "create",
        "task",
        "--workspace",
        workspace,
        "--title",
        "Build",
        "--submission-key",
        "task",
    ];
    let task = fixture.mt(&task_args)?;
    let task = str_field(&task, "createdRef");
    let canvas = fixture.mt(&[
        "create", "canvas", "--task", task, "--title", "Review", "--enter",
    ])?;
    let canvas_ref = str_field(&canvas, "createdRef");
    let session = fixture.mt(&[
        "create", "session", "--canvas", canvas_ref, "--title", "Leaf", "--enter",
    ])?;
    let session_ref = str_field(&session, "createdRef");
    let session_id = session_ref.strip_prefix("session:").unwrap();
    #[cfg(unix)]
    let leaf_token = read_token(&fixture.home, session_id)?;
    #[cfg(windows)]
    let (endpoint, leaf_token) = capture_identity(
        &fixture.host,
        &fixture._root.path().join("data"),
        &fixture.home,
        session_id,
    )?;
    #[cfg(windows)]
    assert_eq!(endpoint, fixture.endpoint);
    let listing = fixture.mt(&["list", "--all"])?;
    let targets = listing["targets"].as_array().unwrap();
    let row = targets
        .iter()
        .find(|row| row["ref"] == session_ref)
        .context("created shell in listing")?;
    assert_eq!(row["workspace"]["name"], "Project");
    assert_eq!(row["task"]["name"], "Build");
    assert_eq!(row["canvas"]["name"], "Review");
    assert_eq!(row["title"], "Leaf");
    assert_eq!(session["focusedPath"]["session"]["ref"], session_ref);

    let before_change = fixture.mt(&["remove", "preview", session_ref, "--scope", "node"])?;
    fixture.mt(&[
        "create",
        "task",
        "--workspace",
        workspace,
        "--title",
        "Later",
    ])?;
    fixture.fault(
        &fixture.token,
        &[
            "remove",
            "commit",
            str_field(&before_change, "confirmationRef"),
        ],
        4,
        "CONFIRMATION_STALE",
    )?;
    let preview = fixture.mt(&["remove", "preview", session_ref, "--scope", "subtree"])?;
    assert_eq!(preview["impact"]["scope"], "node");
    assert_eq!(preview["impact"]["sessions"], 1);
    assert_eq!(preview["impact"]["terminalProcesses"], 1);
    let confirmation = str_field(&preview, "confirmationRef");
    fixture.fault(
        &fixture.token,
        &["close", "canvas-commit", confirmation],
        4,
        "CONFIRMATION_STALE",
    )?;
    fixture.fault(
        &leaf_token,
        &["remove", "commit", confirmation],
        4,
        "CONFIRMATION_REQUIRED",
    )?;
    fixture.fault(
        &fixture.token,
        &["remove", "commit", "fake"],
        4,
        "CONFIRMATION_REQUIRED",
    )?;
    assert_eq!(
        fixture.mt(&["remove", "commit", confirmation])?["removedSessions"],
        1
    );
    fixture.fault(
        &fixture.token,
        &["remove", "commit", confirmation],
        4,
        "CONFIRMATION_REQUIRED",
    )?;
    fixture.fault(
        &fixture.token,
        &["send", session_ref, "archived"],
        3,
        "TARGET_NOT_FOUND",
    )?;
    fixture.fault(
        &leaf_token,
        &["send", "self", "archived"],
        4,
        "CAPABILITY_DENIED",
    )?;

    let close = fixture.mt(&["close", "canvas-preview", canvas_ref])?;
    assert_eq!(close["impact"]["canvases"], 1);
    assert_eq!(
        fixture.mt(&[
            "close",
            "canvas-commit",
            str_field(&close, "confirmationRef")
        ])?["removedSessions"],
        1
    );

    // The standard CLI exposes the same managed fork validation as its wire endpoint.
    for relation in ["child", "sibling"] {
        fixture.fault(
            &fixture.token,
            &[
                "fork",
                relation,
                "self",
                "--title",
                "From Shell",
                "--environment-json",
                r#"{"mode":"current"}"#,
            ],
            4,
            "TARGET_NOT_READY",
        )?;
    }
    let items = r#"[{"itemKey":"a","title":"Child","environment":{"mode":"current"}}]"#;
    let result = fixture.invoke(
        &fixture.token,
        &[
            "fork",
            "children",
            "self",
            "--batch-key",
            "batch",
            "--items-json",
            items,
        ],
    )?;
    assert_eq!(result.status.code(), Some(6));
    let result: Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(result["failed"], 1);
    assert_eq!(result["retry"]["itemKeys"], json!(["a"]));
    fixture.fault(
        &fixture.token,
        &[
            "fork",
            "children",
            "self",
            "--batch-key",
            "batch",
            "--items-json",
            items,
            "--retry-item-key",
            "non-member",
        ],
        2,
        "INVALID_REQUEST",
    )?;
    fixture.host.shutdown()?;
    Ok(())
}
