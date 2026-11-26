//! Measure process starts while RuntimeHost restores Antigravity cards.
#![cfg(unix)]

use std::{
    collections::HashSet,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use matou_native_runtime::host::{
    Command, Event, LaunchOptions, OfflineFixtures, RecoveryState, RuntimeHost,
};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(10);
const TEST_GAP_MS: u64 = 350;
const MIN_MEASURED_GAP_MS: u64 = 300;

struct Fixture {
    _root: TempDir,
    options: LaunchOptions,
    executable: PathBuf,
    traces: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
        fs::create_dir_all(&target)?;
        let root = tempfile::Builder::new().prefix("ags-").tempdir_in(target)?;
        let path = root.path().canonicalize()?;
        let options = LaunchOptions {
            data_root: path.join("data"),
            workspace: path.join("workspace"),
            shell_home: path.join("home"),
            shell: "/bin/zsh".into(),
        };
        let executable = path.join("agy");
        let traces = path.join("traces");
        for directory in [&options.workspace, &options.shell_home, &traces] {
            fs::create_dir(directory)?;
        }
        fs::write(
            &executable,
            r#"#!/usr/bin/perl
use strict;
use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);
my $started_ms = int(clock_gettime(CLOCK_MONOTONIC) * 1000);
my $trace = "$ENV{AGY_TEST_TRACES}/$ENV{MATOU_CONTROL_CALLER_SESSION}";
open(my $output, '>', "$trace.tmp") or die $!;
print $output "$started_ms\n";
close($output) or die $!;
rename("$trace.tmp", "$trace.started") or die $!;
$| = 1;
print "AGY_STAGGER_FIXTURE_READY\n";
while (<STDIN>) {
    open(my $confirmed, '>', "$trace.confirmed") or die $!;
    close($confirmed) or die $!;
    print "AGY_STAGGER_FIXTURE_CONFIRMED\n";
}
"#,
        )?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            _root: root,
            options,
            executable,
            traces,
        })
    }

    fn start(&self, gap_ms: u64) -> Result<RuntimeHost> {
        RuntimeHost::start_with_offline_fixtures(
            self.options.clone(),
            OfflineFixtures {
                provider_environment: Some(
                    [
                        ("PATH".into(), "/usr/bin:/bin".into()),
                        (
                            "MATOU_ANTIGRAVITY_COMMAND".into(),
                            self.executable.clone().into_os_string(),
                        ),
                        (
                            "MATOU_ANTIGRAVITY_STAGGER_MS".into(),
                            gap_ms.to_string().into(),
                        ),
                        (
                            "AGY_TEST_TRACES".into(),
                            self.traces.clone().into_os_string(),
                        ),
                    ]
                    .into(),
                ),
                ..Default::default()
            },
        )
    }

    fn trace(&self, id: &str, suffix: &str) -> PathBuf {
        self.traces.join(format!("{id}.{suffix}"))
    }

    fn seed_three_cards(&self) -> Result<Vec<String>> {
        let mut host = self.start(0)?;
        let (scene, first) = receive(&host, |event| match event {
            Event::Ready { graph, .. } => Some((
                graph["sceneId"].as_str()?.to_owned(),
                graph["focusedSessionId"].as_str()?.to_owned(),
            )),
            _ => None,
        })?;
        let mut cards = vec![first];
        for _ in 0..2 {
            send(
                &host,
                Command::CreateShellSibling {
                    scene_id: scene.clone(),
                    source_session_id: cards.last().unwrap().clone(),
                    parent_session_id: None,
                },
            )?;
            cards.push(receive(&host, |event| match event {
                Event::Navigation { graph, .. } => {
                    graph["focusedSessionId"].as_str().map(str::to_owned)
                }
                _ => None,
            })?);
        }
        for session_id in &cards {
            for input in [b"agy\r".as_slice(), b"confirm\r".as_slice()] {
                send(
                    &host,
                    Command::Input {
                        session_id: session_id.clone(),
                        bytes: input.to_vec(),
                        defer_ordering: false,
                    },
                )?;
            }
        }
        receive(&host, |_| {
            cards
                .iter()
                .all(|id| self.trace(id, "confirmed").is_file())
                .then_some(())
        })
        .context("promote the three fixture cards")?;
        host.shutdown_clean()?;
        for id in &cards {
            fs::remove_file(self.trace(id, "started"))?;
        }
        Ok(cards)
    }
}

fn send(host: &RuntimeHost, command: Command) -> Result<()> {
    host.commands
        .send(command)
        .context("Runtime command channel closed")
}

fn receive<T>(host: &RuntimeHost, mut accept: impl FnMut(Event) -> Option<T>) -> Result<T> {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        while let Ok(event) = host.events.try_recv() {
            match &event {
                Event::Frame {
                    session_id,
                    sequence,
                    ..
                } => send(
                    host,
                    Command::Applied {
                        session_id: session_id.clone(),
                        sequence: *sequence,
                    },
                )?,
                Event::Diagnostic(message) => eprintln!("Runtime diagnostic: {message}"),
                Event::Closed => bail!("Runtime closed during Antigravity restore"),
                _ => {}
            }
            if let Some(result) = accept(event) {
                return Ok(result);
            }
        }
        thread::sleep(Duration::from_millis(4));
    }
    bail!("Timed out waiting for a RuntimeHost event")
}

fn restore(gap_ms: u64) -> Result<Vec<u64>> {
    let fixture = Fixture::new()?;
    let cards = fixture.seed_three_cards()?;
    let mut host = fixture.start(gap_ms)?;
    let focused = receive(&host, |event| match event {
        Event::Ready { graph, .. } => graph["focusedSessionId"].as_str().map(str::to_owned),
        _ => None,
    })?;
    let start = Instant::now();
    let mut ready = HashSet::new();
    let mut first_ready = None;
    receive(&host, |event| {
        if let Event::Recovery {
            session_id,
            state: RecoveryState::Ready,
            ..
        } = event
            && cards.contains(&session_id)
        {
            first_ready.get_or_insert_with(|| (session_id.clone(), start.elapsed()));
            ready.insert(session_id);
        }
        (ready.len() == cards.len()
            && cards
                .iter()
                .all(|id| fixture.trace(id, "started").is_file()))
        .then_some(())
    })
    .with_context(|| format!("restore three fixture cards, ready: {ready:?}"))?;
    let (first, elapsed) = first_ready.context("first restored card")?;
    assert_eq!(first, focused, "the focused card should restore first");
    assert!(
        elapsed < Duration::from_millis(TEST_GAP_MS),
        "first card took {elapsed:?}"
    );

    let mut starts = cards
        .iter()
        .map(|id| {
            let trace = fixture.trace(id, "started");
            Ok(fs::read_to_string(&trace)
                .with_context(|| format!("read {}", trace.display()))?
                .trim()
                .parse::<u64>()?)
        })
        .collect::<Result<Vec<_>>>()?;
    starts.sort_unstable();
    host.shutdown_clean()?;
    let gaps = starts
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    eprintln!("gap setting {gap_ms} ms, first ready in {elapsed:?}, process gaps {gaps:?} ms");
    Ok(gaps)
}

#[test]
fn restores_three_cards_with_spaced_process_starts() -> Result<()> {
    let gaps = restore(TEST_GAP_MS)?;
    assert!(
        gaps.iter().all(|&gap| gap >= MIN_MEASURED_GAP_MS),
        "process gaps {gaps:?} ms"
    );
    Ok(())
}

#[test]
fn restores_three_cards_without_phase_delay_when_disabled() -> Result<()> {
    let gaps = restore(0)?;
    assert!(
        gaps.iter().all(|&gap| gap < MIN_MEASURED_GAP_MS),
        "process gaps {gaps:?} ms"
    );
    Ok(())
}

#[test]
fn failed_antigravity_preparation_releases_the_slot() -> Result<()> {
    let fixture = Fixture::new()?;
    let cards = fixture.seed_three_cards()?;
    let broken = cards.last().unwrap();
    // Simulate the focused card's working directory being removed while Matou was closed.
    let db = rusqlite::Connection::open(fixture.options.data_root.join("matou.sqlite"))?;
    let nonexistent = fixture.options.workspace.join("removed-directory");
    assert_eq!(
        db.execute(
            "UPDATE sessions SET cwd = ? WHERE id = ?",
            (
                nonexistent.to_str().context("fixture working directory")?,
                broken
            ),
        )?,
        1
    );
    drop(db);

    let mut host = fixture.start(TEST_GAP_MS)?;
    let mut failure = None;
    let mut ready = HashSet::new();
    receive(&host, |event| {
        if let Event::Recovery {
            session_id, state, ..
        } = event
        {
            match state {
                RecoveryState::Failed(message) | RecoveryState::StartFailed(message)
                    if &session_id == broken =>
                {
                    failure = Some(message)
                }
                RecoveryState::Ready if cards.contains(&session_id) => {
                    assert!(failure.is_some(), "the focused card should be tried first");
                    ready.insert(session_id);
                }
                _ => {}
            }
        }
        (failure.is_some() && ready.len() == 2).then_some(())
    })
    .with_context(|| {
        format!("wait for the other cards after preparation failure: {failure:?}, ready: {ready:?}")
    })?;
    assert!(
        failure
            .unwrap()
            .contains("working directory is unavailable")
    );
    assert!(!fixture.trace(broken, "started").exists());
    host.shutdown_clean()?;
    Ok(())
}
