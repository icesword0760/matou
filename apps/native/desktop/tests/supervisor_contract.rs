#![cfg(target_os = "macos")]
mod support;
use anyhow::{Context, Result, bail, ensure};
use matou_native_desktop::runtime_bridge::{
    BridgeEvent, Command, ConnectionPhase, Event, LaunchOptions, RuntimeBridge, SendError,
    StartupBlockKind,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;
const TIMEOUT: Duration = Duration::from_secs(12);
fn options() -> Result<(PathBuf, LaunchOptions)> {
    let root = std::env::temp_dir().join(format!("matou-supervisor-{}", Uuid::new_v4()));
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), "PROMPT='SUPERVISOR_READY> '\n")?;
    Ok((
        root.clone(),
        LaunchOptions {
            data_root: root.join("data"),
            workspace: root.join("workspace"),
            shell_home: home,
            shell: "/bin/zsh".into(),
        },
    ))
}
fn next(bridge: &RuntimeBridge) -> Result<BridgeEvent> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match bridge.events.try_recv() {
            Ok(event) => return Ok(event),
            Err(async_channel::TryRecvError::Empty) => {}
            Err(error) => bail!("event stream: {error}"),
        }
        ensure!(Instant::now() < deadline, "event deadline");
        thread::sleep(Duration::from_millis(4));
    }
}
fn ready(bridge: &RuntimeBridge) -> Result<(u64, String)> {
    loop {
        if let BridgeEvent::Runtime {
            generation,
            event: Event::Ready { hierarchy, .. },
        } = next(bridge)?
        {
            return Ok((generation, hierarchy.session.context("Shell session")?.id));
        }
    }
}
fn input(bridge: &RuntimeBridge, generation: u64, session: &str, text: &str) -> Result<()> {
    bridge.commands.send_at(
        generation,
        Command::Input {
            session_id: session.into(),
            bytes: format!("{text}\r").into_bytes(),
            defer_ordering: false,
        },
    )?;
    Ok(())
}
fn frame(bridge: &RuntimeBridge, generation: u64, needle: &str) -> Result<BridgeEvent> {
    let deadline = Instant::now() + TIMEOUT;
    let mut last_rows = Vec::new();
    loop {
        ensure!(
            Instant::now() < deadline,
            "frame {needle:?} deadline; rows={last_rows:?}"
        );
        let event = next(bridge)?;
        if let BridgeEvent::Runtime {
            generation: g,
            event:
                Event::Frame {
                    session_id,
                    screen,
                    sequence,
                    ..
                },
        } = &event
        {
            ensure!(
                *g == generation,
                "unexpected connection while awaiting frame"
            );
            bridge.commands.send_at(
                *g,
                Command::Applied {
                    session_id: session_id.clone(),
                    sequence: *sequence,
                },
            )?;
            let rows: Vec<String> = screen
                .cells
                .chunks(screen.cols)
                .map(|r| {
                    r.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect();
            if rows.iter().any(|r| r == needle) {
                return Ok(event);
            }
            last_rows = rows;
        }
    }
}
fn sql(path: &Path, query: &str) -> Result<String> {
    let output = ProcessCommand::new("/usr/bin/sqlite3")
        .arg(path)
        .arg(query)
        .output()?;
    ensure!(
        output.status.success(),
        "sqlite fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}
fn gone(pid: u32) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let output = ProcessCommand::new("ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()?;
        if !output.status.success()
            || String::from_utf8_lossy(&output.stdout)
                .trim()
                .starts_with('Z')
        {
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "process {pid} still alive");
        thread::sleep(Duration::from_millis(10));
    }
}
fn kill_owned(bridge: &RuntimeBridge, root: &Path) -> Result<u32> {
    let pid = bridge.runtime_pid();
    ensure!(pid > 0 && pid != std::process::id());
    let output = ProcessCommand::new("ps")
        .args(["-p", &pid.to_string(), "-o", "ppid=,args="])
        .output()?;
    let actual = String::from_utf8(output.stdout)?;
    ensure!(
        actual.contains("matou-native-runtime")
            && actual.contains(root.to_str().unwrap())
            && actual
                .split_whitespace()
                .next()
                .and_then(|pid| pid.parse::<u32>().ok())
                == Some(std::process::id()),
        "not this test's runtime: {actual}"
    );
    println!("verified owned runtime child={pid} before kill; actual PPID/args: {actual}");
    ensure!(
        ProcessCommand::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()?
            .success()
    );
    Ok(pid)
}
#[test]
fn actual_crash_reaps_owner_recovers_pty_and_rejects_old_producers_and_events() -> Result<()> {
    let (root, options) = options()?;
    let database = options.data_root.join("matou.sqlite");
    let mut bridge = RuntimeBridge::start_with_binary(options, &support::runtime_binary()?)?;
    let (g1, session) = ready(&bridge)?;
    frame(&bridge, g1, "SUPERVISOR_READY>")?;
    input(&bridge, g1, &session, "printf 'SUPERVISOR_%s\\n' BEFORE")?;
    let old_frame = frame(&bridge, g1, "SUPERVISOR_BEFORE")?;
    let old_shell = sql(
        &database,
        "SELECT pid FROM session_runs WHERE status='running' ORDER BY started_at DESC LIMIT 1",
    )?
    .trim()
    .parse::<u32>()?;
    input(
        &bridge,
        g1,
        &session,
        "printf 'SUPERVISOR_%s\\n' HOLD; read hold",
    )?;
    frame(&bridge, g1, "SUPERVISOR_HOLD")?;
    let old_pid = kill_owned(&bridge, &root)?;
    let (g2, session2) = ready(&bridge)?;
    ensure!(g2 > g1 && session2 == session && bridge.runtime_pid() != old_pid);
    gone(old_pid)?;
    gone(old_shell)?;
    ensure!(
        !bridge.commands.accepts(&old_frame),
        "old Frame passed the same apply gate as Workspace"
    );
    ensure!(matches!(
        bridge.commands.send_at(
            g1,
            Command::Input {
                session_id: session.clone(),
                bytes: b"touch OLD_GENERATION_SIDE_EFFECT\r".to_vec(),
                defer_ordering: false
            }
        ),
        Err(SendError::WrongGeneration { .. })
    ));
    if let BridgeEvent::Runtime {
        generation,
        event: Event::Frame {
            session_id,
            sequence,
            ..
        },
    } = old_frame
    {
        ensure!(
            bridge
                .commands
                .send_at(
                    generation,
                    Command::Applied {
                        session_id,
                        sequence
                    }
                )
                .is_err()
        );
    }
    frame(&bridge, g2, "SUPERVISOR_READY>")?;
    input(
        &bridge,
        g2,
        &session,
        "printf 'SUPERVISOR_%s\\n' AFTER_中文",
    )?;
    frame(&bridge, g2, "SUPERVISOR_AFTER_中文")?;
    ensure!(!root.join("workspace/OLD_GENERATION_SIDE_EFFECT").exists());
    ensure!(
        sql(
            &database,
            "SELECT status FROM session_runs ORDER BY started_at,ordinal LIMIT 1"
        )?
        .trim()
            == "interrupted"
    );
    println!(
        "recovered g{g1}/pid{old_pid}/shell{old_shell} -> g{g2}/pid{}; old input/ACK/frame rejected, new Chinese output acknowledged",
        bridge.runtime_pid()
    );
    bridge.shutdown()?;
    fs::remove_dir_all(root)?;
    Ok(())
}
#[test]
fn deterministic_startup_blocks_without_loop_and_explicit_retry_rechecks() -> Result<()> {
    for fixture in ["newer", "checksum", "path"] {
        let (root, options) = options()?;
        let database = options.data_root.join("matou.sqlite");
        if fixture != "path" {
            let mut seed =
                RuntimeBridge::start_with_binary(options.clone(), &support::runtime_binary()?)?;
            ready(&seed)?;
            seed.shutdown()?;
            if fixture == "newer" {
                sql(
                    &database,
                    "INSERT INTO schema_migrations(version,name,checksum,applied_at) VALUES(999999,'fixture','fixture',0)",
                )?;
            } else {
                sql(
                    &database,
                    "UPDATE schema_migrations SET checksum='fixture' WHERE version=(SELECT MIN(version) FROM schema_migrations)",
                )?;
            }
        } else {
            fs::write(&options.data_root, b"not a directory")?;
        }
        let mut bridge = RuntimeBridge::start_with_binary(options, &support::runtime_binary()?)?;
        let generation = loop {
            match next(&bridge)? {
                BridgeEvent::Connection {
                    generation,
                    phase: ConnectionPhase::Blocked(reason),
                } => {
                    let expected = match fixture {
                        "newer" => StartupBlockKind::ReadOnly,
                        "checksum" => StartupBlockKind::MigrationChecksum,
                        _ => StartupBlockKind::Configuration,
                    };
                    ensure!(reason.kind == expected, "fixture {fixture}: {reason:?}");
                    println!("blocked {fixture} generation={generation}: {reason:?}");
                    break generation;
                }
                BridgeEvent::Runtime {
                    event: Event::Ready { .. },
                    ..
                } => bail!("invalid fixture reported Ready"),
                _ => {}
            }
        };
        thread::sleep(Duration::from_millis(220));
        ensure!(bridge.commands.connection().0 == generation && bridge.runtime_pid() == 0);
        ensure!(bridge.commands.send_at(generation, Command::Refresh) == Err(SendError::NotReady));
        ensure!(bridge.retry_startup());
        loop {
            if let BridgeEvent::Connection {
                generation: g,
                phase: ConnectionPhase::Blocked(_),
            } = next(&bridge)?
            {
                ensure!(g == generation + 1);
                break;
            }
        }
        if fixture == "path" {
            fs::remove_file(root.join("data"))?;
            ensure!(bridge.retry_startup());
            ensure!(
                ready(&bridge)?.0 == generation + 2,
                "explicit retry did not use corrected path"
            );
        }
        bridge.shutdown()?;
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

#[test]
fn recovery_request_is_retired_on_reconnect_and_requires_another_click() -> Result<()> {
    use matou_native_desktop::runtime_bridge::DatabaseRecoveryAction;
    use matou_native_runtime::host::{DATABASE_RECOVERY_INTERRUPTED, database_recovery_request_id};

    let (root, options) = options()?;
    fs::create_dir_all(&options.data_root)?;
    let database = options.data_root.join("matou.sqlite");
    let marker = options.data_root.join("matou.sqlite.recovery.json");
    let recovery_id = "gate-generation";
    sql(
        &database,
        "CREATE TABLE payload(value TEXT); INSERT INTO payload VALUES('test');",
    )?;
    let marker_bytes = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "state": "required",
        "recoveryId": recovery_id,
        "reason": "physical-corruption",
        "durableDatabasePath": database,
        "quarantinedPath": options.data_root.join("matou.sqlite.corrupt-123"),
        "markerPath": marker,
        "createdAt": 123,
    }))?;
    fs::write(&marker, &marker_bytes)?;
    let mut bridge =
        RuntimeBridge::start_with_binary(options.clone(), &support::runtime_binary()?)?;
    let wait_recovery = |bridge: &RuntimeBridge, after| -> Result<u64> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            ensure!(Instant::now() < deadline, "missing recovery phase");
            if let BridgeEvent::Connection {
                generation,
                phase: ConnectionPhase::DatabaseRecovery { details, pending },
            } = next(bridge)?
                && generation > after
            {
                ensure!(details.recovery_id == recovery_id && pending.is_none());
                return Ok(generation);
            }
        }
    };
    let generation = wait_recovery(&bridge, 0)?;
    ensure!(
        bridge.commands.send_at(generation, Command::Refresh) == Err(SendError::StorageReadOnly)
    );
    let request = |request_id| Command::DatabaseRecovery {
        request_id,
        action: DatabaseRecoveryAction::RetryOpen {
            expected_recovery_id: recovery_id.into(),
        },
    };
    let signal = |kind: &str, pid: u32| -> Result<()> {
        ensure!(
            ProcessCommand::new("/bin/kill")
                .arg(kind)
                .arg(pid.to_string())
                .status()?
                .success()
        );
        Ok(())
    };
    let pid = bridge.runtime_pid();
    signal("-STOP", pid)?;
    let abandoned = database_recovery_request_id();
    let admitted = bridge
        .commands
        .send_at(generation, request(abandoned.clone()));
    let duplicate = bridge
        .commands
        .send_at(generation, request(database_recovery_request_id()));
    signal("-KILL", pid)?;
    admitted?;
    ensure!(duplicate == Err(SendError::RecoveryBusy));
    loop {
        if let BridgeEvent::Runtime {
            generation: ended,
            event: Event::DatabaseRecoveryResult(result),
        } = next(&bridge)?
            && result.request_id == abandoned
        {
            ensure!(ended == generation);
            ensure!(
                result.error.context("interrupted error")?.message == DATABASE_RECOVERY_INTERRUPTED
            );
            break;
        }
    }
    let next_generation = wait_recovery(&bridge, generation)?;
    let successor = bridge.runtime_pid();
    let old_operation = bridge
        .commands
        .send_at(generation, request(database_recovery_request_id()));
    ensure!(matches!(
        old_operation,
        Err(SendError::WrongGeneration { .. })
    ));
    thread::sleep(Duration::from_millis(180));
    ensure!(bridge.runtime_pid() == successor && successor != pid);
    ensure!(matches!(
        bridge.commands.connection().1,
        ConnectionPhase::DatabaseRecovery { pending: None, .. }
    ));
    ensure!(fs::read(&marker)? == marker_bytes);
    ensure!(
        !options
            .data_root
            .join(format!("matou.sqlite.recovery.json.resolved-{recovery_id}"))
            .exists()
    );
    ensure!(!options.data_root.join("matou.sqlite.owner").exists());
    let new_request = database_recovery_request_id();
    bridge
        .commands
        .send_at(next_generation, request(new_request.clone()))?;
    ensure!(ready(&bridge)?.0 == next_generation && bridge.runtime_pid() == successor);
    loop {
        if let BridgeEvent::Runtime {
            generation,
            event: Event::DatabaseRecoveryResult(result),
        } = next(&bridge)?
            && result.request_id == new_request
        {
            ensure!(generation == next_generation && result.error.is_none());
            break;
        }
    }
    bridge.shutdown()?;
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn legacy_child_rejecting_hello_blocks_with_protocol_diagnostic() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let (root, options) = options()?;
    let legacy = root.join("legacy-runtime");
    fs::write(
        &legacy,
        "#!/bin/sh\nIFS= read -r hello\ncase \"$hello\" in *'\"version\":1'*) printf '%s\\n' '{\"Hello\":{\"version\":1}}';; *) exit 1;; esac\n",
    )?;
    fs::set_permissions(&legacy, fs::Permissions::from_mode(0o700))?;
    let mut bridge = RuntimeBridge::start_with_binary(options.clone(), &legacy)?;
    loop {
        if let BridgeEvent::Connection {
            phase: ConnectionPhase::Blocked(block),
            ..
        } = next(&bridge)?
        {
            ensure!(block.kind == StartupBlockKind::Protocol);
            break;
        }
    }
    ensure!(!options.data_root.exists());
    bridge.shutdown()?;
    fs::remove_dir_all(root)?;
    Ok(())
}
