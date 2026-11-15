#![cfg(target_os = "macos")]

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use matou_native_core::{
    journal::{Frame, JournalOptions, for_each_frame},
    pty::ShellLaunch,
};
use matou_native_runtime::{
    SessionEvent, SessionOptions,
    worker::{SessionWorker, WorkerCommand, WorkerEvent},
};
use matou_native_terminal::TerminalOptions;

fn options(root: &Path, id: &str) -> Result<SessionOptions> {
    let home = root.join("home");
    let cwd = root.join("workspace");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&cwd)?;
    fs::write(home.join(".zshrc"), "PROMPT='WORKER_READY> '\n")?;
    Ok(SessionOptions {
        session_id: id.into(),
        launch: ShellLaunch {
            executable: "/bin/zsh".into(),
            cwd,
            data_root: root.to_owned(),
            inherited_env: BTreeMap::from([
                ("HOME".into(), home.clone().into_os_string()),
                ("ZDOTDIR".into(), home.into_os_string()),
                ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
                ("SHELL".into(), "/bin/zsh".into()),
                ("LANG".into(), "en_US.UTF-8".into()),
            ]),
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
        },
        terminal: TerminalOptions::new(80, 24, 9, 18),
        journal: JournalOptions {
            compress_sealed: false,
            ..JournalOptions::default()
        },
        restored_history: Vec::new(),
        collect_history: true,
    })
}

#[derive(Default)]
struct Seen {
    pids: BTreeMap<String, u32>,
    output: BTreeMap<String, Vec<(u64, Vec<u8>)>>,
    exited: Vec<String>,
}

impl Seen {
    fn wait(&mut self, inbox: &Receiver<WorkerEvent>, done: impl Fn(&Seen) -> bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !done(self) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let WorkerEvent {
                session_id, event, ..
            } = inbox
                .recv_timeout(remaining)
                .context("wait for real Shell worker events")?;
            match event {
                SessionEvent::Started { pid, .. } => {
                    self.pids.insert(session_id, pid);
                }
                SessionEvent::Output { sequence, data } => {
                    self.output
                        .entry(session_id)
                        .or_default()
                        .push((sequence, data));
                }
                SessionEvent::Exited { .. } => self.exited.push(session_id),
                SessionEvent::Diagnostic { message } => {
                    bail!("unexpected {session_id} diagnostic: {message}")
                }
                SessionEvent::StorageFault { message, .. } => {
                    bail!("unexpected {session_id} storage fault: {message}")
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn frame_with(&self, id: &str, marker: &[u8]) -> Option<&(u64, Vec<u8>)> {
        self.output
            .get(id)?
            .iter()
            .find(|(_, bytes)| bytes.windows(marker.len()).any(|part| part == marker))
    }
}

#[test]
fn real_workers_keep_shell_output_independent_and_return_owned_screens() -> Result<()> {
    let root = tempfile::tempdir()?;
    let root_a = root.path().join("a");
    let root_b = root.path().join("b");
    let (events, incoming) = mpsc::channel();
    let mut a = SessionWorker::spawn(options(&root_a, "a")?, events.clone(), "run-a".into())?;
    let mut b = SessionWorker::spawn(options(&root_b, "b")?, events, "run-b".into())?;
    let (pid_a, pid_b) = (a.process_id(), b.process_id());
    assert!(pid_a > 0 && pid_b > 0);
    assert_ne!(pid_a, pid_b);
    let mut seen = Seen::default();
    seen.wait(&incoming, |seen| {
        seen.frame_with("a", b"WORKER_READY> ").is_some()
            && seen.frame_with("b", b"WORKER_READY> ").is_some()
    })?;
    assert_eq!((seen.pids["a"], seen.pids["b"]), (pid_a, pid_b));

    a.set_egress_frozen(true)?;
    // The central owner can forward requests directly and continue serving
    // events before collecting the command's one-shot response.
    let (reply, written) = mpsc::channel();
    b.send(WorkerCommand::Write {
        bytes: b"printf 'WORKER_%s\\n' 'B_ONLY'\r".to_vec(),
        reply,
    })?;
    seen.wait(&incoming, |seen| {
        seen.frame_with("b", b"WORKER_B_ONLY\r\n").is_some()
    })?;
    written.recv_timeout(Duration::from_secs(2))??;
    let (b_sequence, b_data) = seen.frame_with("b", b"WORKER_B_ONLY\r\n").unwrap();
    let (b_sequence, b_data) = (*b_sequence, b_data.clone());
    let mut stored = Vec::new();
    for_each_frame(&root_b, "b", b_sequence, Some(b_sequence), |record| {
        stored.push(record.frame);
        Ok(())
    })?;
    assert_eq!(
        stored,
        [Frame::Output {
            sequence: b_sequence,
            data: b_data
        }]
    );

    let b_screen = b.screen()?;
    assert!(screen_has(&b_screen, "WORKER_B_ONLY"));
    a.set_egress_frozen(false)?;
    a.write(b"printf 'WORKER_%s\\n' 'A_BACK'\r")?;
    seen.wait(&incoming, |seen| {
        seen.frame_with("a", b"WORKER_A_BACK\r\n").is_some()
    })?;
    let a_screen = a.screen()?;
    assert!(screen_has(&a_screen, "WORKER_A_BACK"));

    b.write(b"exit\r")?;
    seen.wait(&incoming, |seen| seen.exited.iter().any(|id| id == "b"))?;
    b.acknowledge(b_sequence)?;
    assert!(screen_has(&b.screen()?, "WORKER_B_ONLY"));
    assert!(b.write(b"closed\r").is_err());
    a.shutdown()?;
    b.shutdown()?;
    wait_until_exited(seen.pids["a"])?;
    wait_until_exited(seen.pids["b"])?;
    assert!(screen_has(&a_screen, "WORKER_A_BACK"));
    assert!(screen_has(&b_screen, "WORKER_B_ONLY"));
    Ok(())
}

fn screen_has(screen: &matou_native_terminal::Screen, line: &str) -> bool {
    screen.cells.chunks(screen.cols).any(|row| {
        row.iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains(line)
    })
}

fn wait_until_exited(pid: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()?
        .status
        .success()
    {
        ensure!(Instant::now() < deadline, "Shell worker left process {pid}");
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
