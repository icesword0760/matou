use super::*;
use matou_native_core::sessions::{claude_binding, get_session};
use matou_native_runtime::host::{ForkPlacement, ForkWorktreeMode};
use serde_json::{Value, json};
use std::collections::HashSet;

const PARENT: &str = "f1f4132a-f6f8-4b49-9f6d-4b07be37f5c2";
const CHILD: &str = "7ec4132a-f6f8-4b49-9f6d-4b07be37f5c2";
const MODEL: &str = "claude-sonnet-fixture";
const BYPASS: &str = "--dangerously-skip-permissions";
const STUB_READY: &str = "PLUGIN_CLAUDE_READY:";

struct Fixture {
    root: tempfile::TempDir,
    plugin: PathBuf,
    launches: HashSet<u32>,
    settings: HashSet<PathBuf>,
}

struct Launch {
    session: String,
    settings: PathBuf,
    cwd: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let root = tempfile::Builder::new().prefix("mp").tempdir_in("/tmp")?;
        let plugin = root.path().join("control assets/providers/claude-plugin");
        let metadata = plugin.join(".claude-plugin");
        fs::create_dir_all(&metadata)?;
        fs::write(
            metadata.join("plugin.json"),
            include_bytes!(
                "../../../../runtime/control-assets/providers/claude-plugin/.claude-plugin/plugin.json"
            ),
        )?;

        let stub = root.path().join("claude");
        fs::write(
            &stub,
            r#"#!/bin/sh
set -eu
record="$0.$$"
printf '%s\0' "$@" > "$record.argv"
printf '%s' "$PWD" > "$record.cwd"
[ "$1" = --settings ]
/bin/cat "$2" > "$record.settings"
printf 'PLUGIN_CLAUDE_READY:%s\n' "$$"
exec /bin/sleep 60
"#,
        )?;
        fs::set_permissions(stub, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            root,
            plugin,
            launches: HashSet::new(),
            settings: HashSet::new(),
        })
    }

    fn spawn(&self, zshrc: &str) -> Result<RuntimeChild> {
        let root = self.root.path();
        let home = root.join("Shell 家目录");
        RuntimeChild::spawn_configured_with(root, zshrc, Path::new("/bin/zsh"), None, |command| {
            command
                .env_clear()
                .env("HOME", &home)
                .env("ZDOTDIR", &home)
                .env("CLAUDE_CONFIG_DIR", home.join("claude"))
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("TMPDIR", root)
                .env("GIT_CEILING_DIRECTORIES", root)
                .env("MATOU_CLAUDE_COMMAND", root.join("claude"))
                .env("MATOU_CONTROL_ASSET_ROOT", root.join("control assets"))
                .env("MATOU_DISABLE_LIVE_EGRESS", "1");
        })
    }

    fn next_launch(
        &mut self,
        runtime: &mut RuntimeChild,
        entry: &str,
        expected: &[&str],
    ) -> Result<Launch> {
        let deadline = Instant::now() + TIMEOUT;
        let (session, pid) = loop {
            match runtime
                .event(deadline, true)
                .with_context(|| format!("wait for {entry} Claude launch"))?
            {
                Event::Frame {
                    session_id, screen, ..
                } => {
                    let pid = screen.cells.chunks(screen.cols).find_map(|cells| {
                        let line: String = cells.iter().map(|cell| cell.text.as_str()).collect();
                        line.trim_end().strip_prefix(STUB_READY)?.parse().ok()
                    });
                    if let Some(pid) = pid
                        && self.launches.insert(pid)
                    {
                        break (session_id, pid);
                    }
                }
                Event::Diagnostic(error) | Event::NavigationFailed { message: error, .. } => {
                    bail!("{entry}: {error}");
                }
                Event::Graph(graph) if graph["forkSessionResult"].get("error").is_some() => {
                    bail!("{entry}: {}", graph["forkSessionResult"]["error"]);
                }
                _ => {}
            }
        };

        let record = |kind| self.root.path().join(format!("claude.{pid}.{kind}"));
        let argv = fs::read_to_string(record("argv"))?;
        let args: Vec<_> = argv
            .strip_suffix('\0')
            .context("complete argv record")?
            .split('\0')
            .collect();
        let [
            "--settings",
            settings_path,
            "--plugin-dir",
            plugin,
            rest @ ..,
        ] = args.as_slice()
        else {
            bail!("{entry}: invalid settings/plugin order in {args:?}");
        };
        assert_eq!(Path::new(plugin), self.plugin, "{entry}");
        assert_eq!(rest, expected, "{entry}");
        let settings = PathBuf::from(settings_path);
        assert_eq!(
            settings.parent(),
            Some(runtime.data.join("provider-hooks").as_path()),
            "{entry}"
        );
        assert!(settings_path.ends_with(".settings.json"), "{entry}");
        assert!(
            self.settings.insert(settings.clone()),
            "{entry}: reused an earlier run's settings"
        );

        let snapshot = fs::read(record("settings"))?;
        assert_eq!(
            snapshot,
            fs::read(&settings)?,
            "{entry}: settings at launch"
        );
        let payload: Value = serde_json::from_slice(&snapshot)?;
        let prompt = &payload["hooks"]["UserPromptSubmit"][0]["hooks"][0];
        let stop = &payload["hooks"]["Stop"][0]["hooks"][0];
        let start = &payload["hooks"]["SessionStart"][0]["hooks"][0];
        let url = prompt["url"]
            .as_str()
            .context("UserPromptSubmit hook URL")?;
        let parsed = url::Url::parse(url)?;
        assert_eq!(parsed.scheme(), "http", "{entry}");
        assert_eq!(parsed.host_str(), Some("127.0.0.1"), "{entry}");
        assert!(parsed.port().is_some(), "{entry}");
        let token = parsed
            .path()
            .strip_prefix("/hooks/")
            .filter(|token| !token.is_empty())
            .context("per-run hook token")?;
        assert_eq!(prompt["type"], "http", "{entry}");
        assert_eq!(stop["type"], "http", "{entry}");
        assert_eq!(stop["url"], url, "{entry}");
        assert_eq!(start["type"], "command", "{entry}");
        let command = start["command"].as_str().context("SessionStart executable")?;
        assert!(Path::new(command).is_absolute(), "{entry}");
        assert_eq!(start["args"], json!(["--hook", "claude-session-start"]), "{entry}");
        let hud = &payload["statusLine"];
        assert_eq!(hud["type"], "command", "{entry}");
        assert_eq!(hud["padding"], 0, "{entry}");
        let hud_name = format!("{token}.statusline.sh");
        let hud_command = hud["command"].as_str().context("HUD command")?;
        assert!(hud_command.contains(&hud_name), "{entry}");
        assert!(
            fs::read_to_string(settings.with_file_name(hud_name))?
                .contains(&format!("{url}?kind=statusline")),
            "{entry}"
        );

        let cwd = PathBuf::from(fs::read_to_string(record("cwd"))?);
        println!(
            "{entry}: --settings <run-settings.json> --plugin-dir <claude-plugin> {rest:?}; cwd={cwd:?}"
        );
        Ok(Launch {
            session,
            settings,
            cwd,
        })
    }
}

fn shutdown(mut runtime: RuntimeChild) -> Result<()> {
    runtime.send(Command::Shutdown)?;
    runtime.closed()?;
    let (status, stderr) = runtime.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");
    Ok(())
}

fn configure_provider(data: &Path) -> Result<()> {
    fs::write(
        data.join("provider-config.json"),
        json!({
            "version": 1,
            "providers": {"claude-code": [{
                "id": "fixture", "model": MODEL, "reasoningEffort": "low",
                "endpoint": "http://127.0.0.1:9", "apiKey": "fixture-key"
            }]},
            "activeProviderIds": {"claude-code": "fixture"}
        })
        .to_string(),
    )?;
    Ok(())
}

fn confirm(runtime: &mut RuntimeChild, launch: &Launch, identity: &str, mode: &str) -> Result<()> {
    post_claude_identity(&launch.settings, identity, mode, MODEL)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Graph(graph) = runtime.event(deadline, true)?
            && graph["providerIdentityEvents"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|event| {
                    event["result"] == "confirmed"
                        && event["sessionId"] == launch.session
                        && event["providerSessionId"] == identity
                })
        {
            return Ok(());
        }
    }
}

#[test]
fn packaged_argv_for_create_restore_permission_restart_and_fork() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let rc = format!("PROMPT='{PROMPT} '\n");
    let mut runtime = fixture.spawn(&rc)?;
    runtime.hello()?;
    configure_provider(&runtime.data)?;
    let db = Connection::open(runtime.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    runtime.input("mt create session --canvas current --profile claude-code --enter")?;
    let fresh = fixture.next_launch(
        &mut runtime,
        "new Claude card",
        &["--model", MODEL, "--effort", "low"],
    )?;
    assert_ne!(fresh.session, runtime.session);
    confirm(&mut runtime, &fresh, PARENT, "default")?;
    shutdown(runtime)?;

    let mut runtime = fixture.spawn(&rc)?;
    runtime.connect()?;
    assert_eq!(
        runtime.session, fresh.session,
        "create --enter selected the Claude card"
    );
    let resume_args = ["--model", MODEL, "--effort", "low", "--resume", PARENT];
    let cold = fixture.next_launch(&mut runtime, "cold resume", &resume_args)?;
    assert_eq!(cold.session, fresh.session);
    confirm(&mut runtime, &cold, PARENT, "default")?;
    shutdown(runtime)?;

    // Start with a parked binding so the retry command has to start a new CLI.
    assert_eq!(
        db.execute(
            "UPDATE provider_bindings SET restore_state='failed', restore_error='fixture parked' WHERE session_id=?",
            [&fresh.session],
        )?,
        1
    );
    let mut runtime = fixture.spawn(&rc)?;
    runtime.connect()?;
    runtime.send(Command::RetryProviderRestore {
        session_id: fresh.session.clone(),
    })?;
    let manual = fixture.next_launch(&mut runtime, "manual resume", &resume_args)?;
    assert_eq!(manual.session, fresh.session);
    confirm(&mut runtime, &manual, PARENT, "default")?;

    runtime.send(Command::SetProviderPermissionMode {
        session_id: fresh.session.clone(),
        mode: "bypassPermissions".into(),
    })?;
    let restarted = fixture.next_launch(
        &mut runtime,
        "permission restart to bypass",
        &[
            "--model", MODEL, "--effort", "low", "--resume", PARENT, BYPASS,
        ],
    )?;
    assert_eq!(restarted.session, fresh.session);
    post_claude_identity(&restarted.settings, PARENT, "bypassPermissions", MODEL)?;
    let result = permission_mode_result(&mut runtime, "bypassPermissions")?;
    assert!(result.get("error").is_none(), "{result}");

    for event in ["UserPromptSubmit", "Stop"] {
        assert_eq!(
            post_claude_hook(
                &restarted.settings,
                json!({
                    "hook_event_name": event, "session_id": PARENT, "model": MODEL
                })
            )?,
            200
        );
    }
    let deadline = Instant::now() + TIMEOUT;
    while claude_binding(&db, &fresh.session)?
        .context("parent binding")?
        .metadata["canFork"]
        != true
    {
        runtime.event(deadline, true)?;
    }
    runtime.send(Command::ForkSession {
        scene_id: runtime.scene.clone(),
        source_session_id: fresh.session.clone(),
        placement: ForkPlacement::Child,
        name: "Plugin fork fixture".into(),
        worktree_mode: ForkWorktreeMode::Current,
        submission_key: "plugin-child".into(),
    })?;
    let forked = fixture.next_launch(
        &mut runtime,
        "ordinary fork",
        &[
            "--model",
            MODEL,
            "--effort",
            "low",
            "--resume",
            PARENT,
            "--fork-session",
            BYPASS,
        ],
    )?;
    assert_ne!(forked.session, fresh.session);
    confirm(&mut runtime, &forked, CHILD, "bypassPermissions")?;
    shutdown(runtime)
}

#[test]
fn packaged_argv_for_empty_fork_rebuild() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let mut seed = fixture.spawn("")?;
    seed.ready()?;
    let session = seed.session.clone();
    let workspace = seed.workspace.clone();
    let data = seed.data.clone();
    shutdown(seed)?;

    configure_provider(&data)?;
    seed_catalog(fixture.root.path(), &workspace, PARENT)?;
    let db = Connection::open(data.join("matou.sqlite"))?;
    db.execute(
        "UPDATE sessions SET kind='claude-code' WHERE id=?",
        [&session],
    )?;
    db.execute(
        "INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, cwd,
                               created_at, updated_at, last_activity_at)
         SELECT 'parent-card', task_id, execution_context_id, kind, 'created', 'Parent', cwd,
                created_at, updated_at, last_activity_at FROM sessions WHERE id=?",
        [&session],
    )?;
    db.execute(
        "INSERT INTO session_fork_intents (session_id, source_session_id, source_provider,
                                         source_provider_session_id, state, stage, created_at)
         VALUES (?, 'parent-card', 'claude-code', ?, 'succeeded', 'succeeded', 1)",
        [&session, PARENT],
    )?;
    db.execute(
        "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id,
             resume_state, restore_state, metadata_json, created_at, updated_at, validated_at)
         VALUES ('saved-child', ?, 'claude-code', ?, 'available', 'none', ?, 1, 1, 1)",
        rusqlite::params![
            session,
            CHILD,
            json!({
                "providerConfigId": "fixture", "model": MODEL, "permissionMode": "plan",
                "observedUserPrompt": false, "lastHookEvent": "SessionStart"
            })
            .to_string()
        ],
    )?;

    let mut runtime = fixture.spawn("")?;
    runtime.connect()?;
    let rebuilt = fixture.next_launch(
        &mut runtime,
        "empty fork rebuild",
        &[
            "--model",
            MODEL,
            "--effort",
            "low",
            "--resume",
            PARENT,
            "--fork-session",
            "--session-id",
            CHILD,
            "--permission-mode",
            "plan",
        ],
    )?;
    assert_eq!(rebuilt.session, session);
    confirm(&mut runtime, &rebuilt, CHILD, "plan")?;
    shutdown(runtime)
}

#[cfg(unix)]
#[test]
fn packaged_argv_for_shell_promotions() -> Result<()> {
    for (input, rc, expected) in [
        ("claude", "", &[][..]),
        ("claude --dangerously-skip-permissions", "", &[BYPASS][..]),
        ("cc", "alias cc='claude'\n", &[][..]),
        (
            "cc",
            "alias cc='claude --dangerously-skip-permissions'\n",
            &[BYPASS][..],
        ),
    ] {
        let mut fixture = Fixture::new()?;
        let mut runtime = fixture.spawn(&format!("PROMPT='{PROMPT} '\n{rc}"))?;
        runtime.hello()?;
        runtime.input(input)?;
        let launched =
            fixture.next_launch(&mut runtime, &format!("shell {input} {rc:?}"), expected)?;
        assert_eq!(
            launched.session, runtime.session,
            "promotion kept the original card"
        );
        let db = Connection::open(runtime.data.join("matou.sqlite"))?;
        assert_eq!(
            get_session(&db, &launched.session)?
                .context("promoted card")?
                .kind,
            "claude-code"
        );
        shutdown(runtime)?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn packaged_argv_for_builtin_cc_in_clean_zsh() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let mut runtime = fixture.spawn("")?;
    runtime.ready()?;
    let cwd = runtime.workspace.join("cc work directory");
    fs::create_dir(&cwd)?;
    runtime.input("if ! alias cc >/dev/null 2>&1 && [[ $(whence -w cc) == 'cc: function' ]]; then cd 'cc work directory' && printf '\\nCLEAN_ZSH_CC_READY\\n'; fi")?;
    runtime.frame(|_, screen, _| screen_has_line(screen, "CLEAN_ZSH_CC_READY"))?;
    runtime.input("cc")?;
    let launched = fixture.next_launch(&mut runtime, "clean zsh builtin cc", &[BYPASS])?;
    assert_eq!(launched.cwd.canonicalize()?, cwd.canonicalize()?);
    assert_ne!(launched.session, runtime.session, "cc opened a new card");
    let db = Connection::open(runtime.data.join("matou.sqlite"))?;
    assert_eq!(
        get_session(&db, &runtime.session)?
            .context("original card")?
            .kind,
        "shell"
    );
    let created = get_session(&db, &launched.session)?.context("cc-created card")?;
    assert_eq!(created.kind, "claude-code");
    assert_eq!(
        Path::new(&created.cwd).canonicalize()?,
        launched.cwd.canonicalize()?
    );
    let mounted: bool = db.query_row(
        "SELECT EXISTS (SELECT 1 FROM session_mounts WHERE scene_id=? AND session_id=?)",
        [&runtime.scene, &launched.session],
        |row| row.get(0),
    )?;
    assert!(
        mounted,
        "cc mounted the new Claude card in the current canvas"
    );
    shutdown(runtime)?;

    let mut runtime = fixture.spawn("")?;
    runtime.connect()?;
    assert_eq!(
        runtime.session, launched.session,
        "cc --enter selected the new card"
    );
    shutdown(runtime)
}
