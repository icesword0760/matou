#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use matou_native_core::hierarchy::session_environment::{self, Kind, Target};
use matou_native_core::startup::open_read_only;
use matou_native_runtime::host::{
    Command, EnvironmentChangeResult, Event, LaunchOptions, OfflineFixtures, RuntimeHost,
    STORAGE_READ_ONLY, SessionEnvironmentReply,
};
use rusqlite::{Connection, params};

struct Fixture {
    root: tempfile::TempDir,
    options: LaunchOptions,
    scene: String,
    session: String,
    other: String,
    checkout: PathBuf,
    other_checkout: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let root = tempfile::tempdir()?;
        let canonical = root.path().canonicalize()?;
        let options = LaunchOptions {
            data_root: canonical.join("data"),
            workspace: canonical.join("local"),
            shell_home: canonical.join("home"),
            shell: "/bin/zsh".into(),
        };
        fs::create_dir(&options.workspace)?;
        fs::create_dir(&options.shell_home)?;
        fs::write(options.shell_home.join(".zshrc"), "PROMPT='> '\nPS1='> '\n")?;
        git(&options.workspace, &["init", "-b", "main"])?;
        git(
            &options.workspace,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        )?;
        let checkout = canonical.join("original");
        let other_checkout = canonical.join("other");
        for (branch, path) in [("owned", &checkout), ("other-owned", &other_checkout)] {
            git(
                &options.workspace,
                &["worktree", "add", "-b", branch, path.to_str().unwrap()],
            )?;
        }
        let mut host = start(&options)?;
        let (scene, session) = wait(&host, "ready", |event| match event {
            Event::Ready { hierarchy, .. } => {
                Some((hierarchy.scene.unwrap().id, hierarchy.session.unwrap().id))
            }
            _ => None,
        })?;
        host.commands.send(Command::CreateShellSibling {
            scene_id: scene.clone(),
            source_session_id: session.clone(),
            parent_session_id: None,
        })?;
        let other = wait(&host, "second session", |event| match event {
            Event::Navigation { hierarchy, .. } => hierarchy.session.map(|session| session.id),
            _ => None,
        })?;
        host.shutdown()?;
        let db = Connection::open(options.data_root.join("matou.sqlite"))?;
        for (id, path, branch) in [
            (&session, &checkout, "owned"),
            (&other, &other_checkout, "other-owned"),
        ] {
            let context = format!("context-{branch}");
            let worktree = format!("worktree-{branch}");
            let path = path.to_str().unwrap();
            db.execute(
                "INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
                 SELECT ?, tasks.workspace_id, 'git-worktree', ?, 1
                 FROM sessions JOIN tasks ON tasks.id = sessions.task_id WHERE sessions.id = ?",
                params![context, path, id],
            )?;
            db.execute(
                "INSERT INTO worktrees (id, execution_context_id, repository_root, worktree_path, branch_name,
                    state, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, 'ready', 1, 1)",
                params![worktree, context, options.workspace.to_str(), path, branch],
            )?;
            db.execute(
                "UPDATE session_environment_bindings SET managed_worktree_id = ?, active_target = 'worktree'
                 WHERE session_id = ?", params![worktree, id],
            )?;
            db.execute(
                "UPDATE sessions SET execution_context_id = ?, cwd = ?, work_status = 'interrupted'
                 WHERE id = ?",
                params![context, path, id],
            )?;
        }
        Ok(Self {
            root,
            options,
            scene,
            session,
            other,
            checkout,
            other_checkout,
        })
    }

    fn database(&self) -> PathBuf {
        self.options.data_root.join("matou.sqlite")
    }

    fn restore(&self, request_id: u64) -> Command {
        Command::RestoreSessionEnvironment {
            request_id,
            session_id: self.session.clone(),
        }
    }

    fn locate(&self, request_id: u64, path: &Path) -> Command {
        Command::LocateSessionEnvironment {
            request_id,
            session_id: self.session.clone(),
            path: path.into(),
        }
    }

    fn handoff(&self, request_id: u64, target: Target) -> Command {
        Command::HandoffSessionEnvironment {
            request_id,
            session_id: self.session.clone(),
            target,
        }
    }

    fn assert_binding(&self, target: Target, path: &Path) -> Result<()> {
        let db = open_read_only(&self.database())?;
        let (actual_target, state, cwd, context_cwd, worktrees, transitions): (String, String, String, String, String, i64) =
            db.query_row(
                "SELECT binding.active_target, binding.state, sessions.cwd, context.cwd, worktree.worktree_path,
                    (SELECT count(*) FROM session_environment_transitions)
                 FROM session_environment_bindings binding
                 JOIN sessions ON sessions.id = binding.session_id
                 JOIN execution_contexts context ON context.id = sessions.execution_context_id
                 JOIN worktrees worktree ON worktree.id = binding.managed_worktree_id
                 WHERE binding.session_id = ?",
                 [&self.session], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )?;
        assert_eq!(actual_target, target.as_str());
        assert_eq!(state, "ready");
        assert_eq!(Path::new(&cwd), path);
        assert_eq!(Path::new(&context_cwd), path);
        if target == Target::Worktree {
            assert_eq!(Path::new(&worktrees), path);
        }
        assert_eq!(transitions, 0);
        Ok(())
    }
}

#[test]
fn running_shell_handoff_restore_and_locate_use_the_selected_checkout() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut host = start(&fixture.options)?;
    assert_pwd(&host, &fixture.session, &fixture.checkout)?;
    let original_runs = run_count(&fixture)?;
    host.commands.send(fixture.handoff(1, Target::Local))?;
    host.commands.send(fixture.restore(2))?;
    let mut replies = vec![reply(&host)?, reply(&host)?];
    replies.sort_by_key(|reply| reply.request_id);
    assert_eq!(
        replies[0].result,
        success(Target::Local, &fixture.options.workspace)
    );
    assert_eq!(
        replies[1].result,
        rejection("environment-transition-in-progress")
    );
    fixture.assert_binding(Target::Local, &fixture.options.workspace)?;
    assert_pwd(&host, &fixture.session, &fixture.options.workspace)?;

    host.commands.send(fixture.handoff(3, Target::Worktree))?;
    assert_eq!(
        reply(&host)?.result,
        success(Target::Worktree, &fixture.checkout)
    );
    fixture.assert_binding(Target::Worktree, &fixture.checkout)?;
    assert_pwd(&host, &fixture.session, &fixture.checkout)?;

    fs::remove_dir_all(&fixture.checkout)?;
    host.commands.send(fixture.restore(4))?;
    assert_eq!(
        reply(&host)?.result,
        success(Target::Worktree, &fixture.checkout)
    );
    assert!(fixture.checkout.join(".git").is_file());
    fixture.assert_binding(Target::Worktree, &fixture.checkout)?;
    assert_pwd(&host, &fixture.session, &fixture.checkout)?;

    let destination = fixture.root.path().join("elsewhere");
    fs::create_dir(&destination)?;
    let moved = destination.join("moved checkout");
    fs::rename(&fixture.checkout, &moved)?;
    let expected_cwd = moved.canonicalize()?;
    let alias = destination.join("symlink");
    std::os::unix::fs::symlink(&moved, &alias)?;
    host.commands.send(fixture.locate(5, &alias))?;
    assert_eq!(
        reply(&host)?.result,
        success(Target::Worktree, &expected_cwd)
    );
    fixture.assert_binding(Target::Worktree, &expected_cwd)?;
    assert_pwd(&host, &fixture.session, &expected_cwd)?;
    assert_eq!(run_count(&fixture)?, original_runs + 4);

    let other_moved = destination.join("other moved");
    fs::rename(&fixture.other_checkout, &other_moved)?;
    host.commands.send(fixture.locate(6, &other_moved))?;
    assert_eq!(
        reply(&host)?.result,
        EnvironmentChangeResult::SwitchSession {
            session_id: fixture.other.clone(),
            scene_id: Some(fixture.scene.clone()),
        }
    );
    // The competing checkout must keep its own original Git registration.
    let other_repo = matou_native_core::git_repo::Repository::discover(&other_moved)?.unwrap();
    assert_eq!(
        other_repo.linked_worktree_registration()?,
        fixture.other_checkout
    );
    git(
        &fixture.options.workspace,
        &["worktree", "repair", other_moved.to_str().unwrap()],
    )?;
    host.commands.send(fixture.locate(12, &other_moved))?;
    assert_eq!(
        reply(&host)?.result,
        EnvironmentChangeResult::SwitchSession {
            session_id: fixture.other.clone(),
            scene_id: Some(fixture.scene.clone()),
        }
    );
    host.commands
        .send(fixture.locate(7, &destination.join("absent")))?;
    assert_eq!(reply(&host)?.result, rejection("path-missing"));
    host.commands
        .send(fixture.locate(8, &fixture.options.workspace))?;
    assert_eq!(reply(&host)?.result, rejection("wrong-repository"));
    fixture.assert_binding(Target::Worktree, &expected_cwd)?;
    assert_eq!(run_count(&fixture)?, original_runs + 4);
    let db = open_read_only(&fixture.database())?;
    let retired: i64 = db.query_row(
        "SELECT count(*) FROM domain_events WHERE session_id = ? AND event_type = 'session.run-interrupted'
         AND json_extract(payload_json, '$.exitReason') = 'environment-transition'",
        [&fixture.session], |row| row.get(0),
    )?;
    assert_eq!(retired, 4);
    host.shutdown()
}

#[test]
fn a_target_shell_that_exits_during_startup_rolls_back_and_resumes_the_original() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut host = start(&fixture.options)?;
    assert_pwd(&host, &fixture.session, &fixture.checkout)?;
    fs::write(
        fixture.options.shell_home.join(".zshrc"),
        "if [[ ${PWD:t} = local ]]; then exit 29; fi
PROMPT='> '
",
    )?;
    host.commands.send(fixture.handoff(13, Target::Local))?;
    let result = reply(&host)?.result;
    assert!(
        matches!(result, EnvironmentChangeResult::Rejected { reason }
        if reason.starts_with("handoff-failed:"))
    );
    fixture.assert_binding(Target::Worktree, &fixture.checkout)?;
    assert_pwd(&host, &fixture.session, &fixture.checkout)?;
    host.shutdown()
}

#[test]
fn startup_reconciles_journal_and_read_only_rejects_all_three_without_writing() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut db = Connection::open(fixture.database())?;
    session_environment::begin_transition(
        &mut db,
        &fixture.other,
        "interrupted-handoff".into(),
        Kind::Handoff,
        Target::Local,
        None,
        2,
    )?;
    let moved = fixture.root.path().join("moved");
    fs::rename(&fixture.checkout, &moved)?;
    let expected_cwd = moved.canonicalize()?;
    let transition = session_environment::begin_transition(
        &mut db,
        &fixture.session,
        "interrupted-locate".into(),
        Kind::Locate,
        Target::Worktree,
        Some(moved.to_str().unwrap().into()),
        3,
    )?;
    let repo =
        matou_native_core::git_repo::Repository::discover(&fixture.options.workspace)?.unwrap();
    repo.repair_managed_worktree(
        &fixture.checkout,
        &moved,
        &matou_native_core::git_repo::Head::Branch("owned".into()),
    )?;
    session_environment::mark_transition_external_ready(&db, &transition, 3)?;
    drop(db);
    let mut host = start(&fixture.options)?;
    assert_pwd(&host, &fixture.session, &expected_cwd)?;
    fixture.assert_binding(Target::Worktree, &expected_cwd)?;
    assert_pwd(&host, &fixture.other, &fixture.other_checkout)?;
    host.shutdown()?;

    let db = Connection::open(fixture.database())?;
    db.execute_batch(
        "INSERT INTO schema_migrations (version, name, checksum, applied_at)
         VALUES (9999, 'fixture-newer', 'fixture-newer', 5);
         PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    drop(db);
    let before = fs::read(fixture.database())?;
    let mut host = start(&fixture.options)?;
    wait(&host, "read-only", |event| {
        matches!(event, Event::ReadOnlyReady { .. }).then_some(())
    })?;
    for command in [
        fixture.restore(9),
        fixture.locate(10, &moved),
        fixture.handoff(11, Target::Local),
    ] {
        host.commands.send(command)?;
        assert_eq!(reply(&host)?.result, rejection(STORAGE_READ_ONLY));
    }
    host.shutdown()?;
    assert_eq!(fs::read(fixture.database())?, before);
    fixture.assert_binding(Target::Worktree, &expected_cwd)
}

#[test]
fn provider_handoff_resumes_the_same_identity_and_recovers_it_after_a_failed_target() -> Result<()>
{
    use matou_native_runtime::dsh_catalog::encode_project_path;
    use serde_json::{Value, json};
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new()?;
    let history = fixture.root.path().join("dsh");
    let conversation = history
        .join(encode_project_path(fixture.checkout.to_str().unwrap()))
        .join("saved");
    fs::create_dir_all(&conversation)?;
    fs::write(
        conversation.join("session.v4.jsonl"),
        format!(
            "{}\n",
            json!({
                "type":"session", "version":4, "id":"saved", "cwd":fixture.checkout, "createdAt":1000
            })
        ),
    )?;
    let db = Connection::open(fixture.database())?;
    db.execute(
        "UPDATE sessions SET kind = 'dsh' WHERE id = ?",
        [&fixture.session],
    )?;
    db.execute(
        "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id, resume_state,
                                       validated_at, created_at, updated_at)
         VALUES ('fixture-binding', ?, 'dsh', 'saved', 'available', 1, 1, 1)",
        [&fixture.session],
    )?;
    drop(db);
    let stub = fixture.root.path().join("dsh-stub");
    fs::write(
        &stub,
        r#"#!/bin/sh
set -eu
if [ "$2" = --dump-config ]; then
  printf '%s\n' '- id: dsh-tui' '  config:' '    backend: dsh'
  exit 0
fi
test "$1" = dsh-tui && test "$2" = --patch && test "$4" = --resume && test "$5" = saved
if [ -f "$HOME/reject-local" ] && [ "${PWD##*/}" = local ]; then
  exit 29
fi
printf '%s\n' "$PWD" > "$HOME/provider-cwd.next"
/bin/cat "$3" >> "$HOME/provider-cwd.next"
/bin/mv "$HOME/provider-cwd.next" "$HOME/provider-cwd"
printf 'PROVIDER FIXTURE READY\n'
exec /bin/cat
"#,
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755))?;
    let mut host = RuntimeHost::start_with_offline_fixtures(
        fixture.options.clone(),
        OfflineFixtures {
            dsh_session_root: Some(history),
            provider_environment: Some(
                [
                    (
                        "HOME".into(),
                        fixture.options.shell_home.as_os_str().to_owned(),
                    ),
                    ("PATH".into(), "/usr/bin:/bin".into()),
                    ("MATOU_DSH_COMMAND".into(), stub.into_os_string()),
                ]
                .into_iter()
                .collect(),
            ),
            ..OfflineFixtures::default()
        },
    )?;
    let marker = fixture.options.shell_home.join("provider-cwd");
    let confirm = |cwd: &Path, previous: &str| -> Result<String> {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if let Ok(observed) = fs::read_to_string(&marker) {
                let field = |name: &str| -> Result<String> {
                    let value = observed
                        .lines()
                        .find_map(|line| line.trim().strip_prefix(name))
                        .context("provider hook field")?;
                    Ok(serde_json::from_str(value)?)
                };
                let run = field("runId: ")?;
                if run != previous {
                    assert_eq!(observed.lines().next(), cwd.to_str());
                    let url = url::Url::parse(&field("hookUrl: ")?)?;
                    ensure!(
                        url.host_str() == Some("127.0.0.1"),
                        "fixture hook is not loopback"
                    );
                    let payload: Value = json!({
                        "dsh_observation":1, "run_id":run, "sequence":1, "generation":1,
                        "session_id":"saved", "foreground":true, "event_type":"ready",
                        "ready":true, "persisted":true, "cwd":cwd, "agent_status":"idle"
                    });
                    let response = process::Command::new("/usr/bin/curl")
                        .args([
                            "--noproxy",
                            "*",
                            "--max-time",
                            "3",
                            "-fsS",
                            "-H",
                            "Content-Type: application/json",
                            "--data-binary",
                            &payload.to_string(),
                            url.as_str(),
                        ])
                        .output()?;
                    ensure!(
                        response.status.success(),
                        "fixture hook: {}",
                        String::from_utf8_lossy(&response.stderr)
                    );
                    return Ok(run);
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        bail!("provider did not start in {}", cwd.display())
    };
    let run = confirm(&fixture.checkout, "")?;
    wait(&host, "initial DSH recovery", |event| match event {
        Event::Recovery {
            session_id,
            state: matou_native_runtime::host::RecoveryState::Ready,
        } if session_id == fixture.session => Some(()),
        _ => None,
    })?;

    host.commands.send(fixture.handoff(20, Target::Local))?;
    let run = confirm(&fixture.options.workspace, &run)?;
    assert_eq!(
        reply(&host)?.result,
        success(Target::Local, &fixture.options.workspace)
    );
    host.commands.send(fixture.handoff(21, Target::Worktree))?;
    let run = confirm(&fixture.checkout, &run)?;
    assert_eq!(
        reply(&host)?.result,
        success(Target::Worktree, &fixture.checkout)
    );

    fs::write(fixture.options.shell_home.join("reject-local"), "")?;
    host.commands.send(fixture.handoff(22, Target::Local))?;
    confirm(&fixture.checkout, &run)?;
    assert!(
        matches!(reply(&host)?.result, EnvironmentChangeResult::Rejected { reason }
        if reason.starts_with("handoff-failed:"))
    );
    fixture.assert_binding(Target::Worktree, &fixture.checkout)?;
    let db = open_read_only(&fixture.database())?;
    let binding = matou_native_core::sessions::dsh_binding(&db, &fixture.session)?.unwrap();
    assert_eq!(binding.provider_session_id, "saved");
    assert!(binding.resumable);
    host.shutdown()
}

fn start(options: &LaunchOptions) -> Result<RuntimeHost> {
    RuntimeHost::start_with_offline_fixtures(options.clone(), OfflineFixtures::default())
}

fn git(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn success(active_target: Target, cwd: &Path) -> EnvironmentChangeResult {
    EnvironmentChangeResult::Ok {
        active_target,
        cwd: cwd.to_str().unwrap().into(),
    }
}

fn rejection(reason: &str) -> EnvironmentChangeResult {
    EnvironmentChangeResult::Rejected {
        reason: reason.into(),
    }
}

fn reply(host: &RuntimeHost) -> Result<SessionEnvironmentReply> {
    wait(host, "environment result", |event| match event {
        Event::RestoreSessionEnvironmentResult(reply)
        | Event::LocateSessionEnvironmentResult(reply)
        | Event::HandoffSessionEnvironmentResult(reply) => Some(reply),
        _ => None,
    })
}

fn run_count(fixture: &Fixture) -> Result<usize> {
    let db = open_read_only(&fixture.database())?;
    Ok(matou_native_core::sessions::list_runs(&db, &fixture.session)?.len())
}

fn assert_pwd(host: &RuntimeHost, id: &str, cwd: &Path) -> Result<()> {
    // A unique marker prevents retained scrollback from satisfying a later run.
    let marker = format!("CWD-{}|", uuid::Uuid::new_v4().simple());
    host.commands.send(Command::Input {
        session_id: id.into(),
        bytes: format!("printf '{marker}'; pwd -P\r").into_bytes(),
        defer_ordering: false,
    })?;
    let expected = format!("{marker}{}", cwd.display());
    wait(host, &expected, |event| match event {
        Event::Frame {
            session_id, screen, ..
        } if session_id == id => {
            let text: String = screen.cells.iter().map(|cell| cell.text.as_str()).collect();
            text.contains(&expected).then_some(())
        }
        _ => None,
    })
}

fn wait<T>(host: &RuntimeHost, label: &str, accept: impl Fn(Event) -> Option<T>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        match host.events.try_recv() {
            Ok(Event::Closed) => bail!("runtime closed while waiting for {label}"),
            Ok(Event::Diagnostic(message)) if message.to_lowercase().contains("sequence") => {
                bail!("sequence diagnostic: {message}")
            }
            Ok(event) => {
                if let Event::Frame {
                    session_id,
                    sequence,
                    ..
                } = &event
                {
                    host.commands.send(Command::Applied {
                        session_id: session_id.clone(),
                        sequence: *sequence,
                    })?;
                }
                if let Some(result) = accept(event) {
                    return Ok(result);
                }
            }
            Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(error).context("event channel"),
        }
    }
    bail!("timed out waiting for {label}")
}
