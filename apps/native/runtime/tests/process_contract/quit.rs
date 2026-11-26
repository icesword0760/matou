use super::*;
use std::{
    collections::HashSet,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::fs::MetadataExt,
    ptr,
    time::{SystemTime, UNIX_EPOCH},
};

fn wait_for_fixture(path: &Path, event: &str) -> Result<serde_json::Value> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(log) = fs::read_to_string(path)
            && let Some(record) = log
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|row| row["event"] == event)
        {
            return Ok(record);
        }
        ensure!(
            Instant::now() < deadline,
            "fixture didn't report {event}: {path:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn start_fixture(root: &Path, mode: &str) -> Result<(RuntimeChild, u32, Option<u32>)> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quit-real-01-child.py");
    let script = script.display().to_string().replace('\'', "'\"'\"'");
    let root_arg = root.display().to_string().replace('\'', "'\"'\"'");
    let run = format!("/usr/bin/python3 -I -B '{script}' {mode} '{root_arg}'",);
    let shell = match mode {
        // A chevron prompt is classified as a request for input if it arrives
        // in a separate PTY chunk from the preceding completion marker.
        "baseline" => "PROMPT='QUIT_BASELINE_READY$ '\n".into(),
        "nested" => format!("printf '%s' \"$$\" > '{root_arg}/shell-pid'\n{run}\n"),
        _ => format!("exec {run}\n"),
    };
    // The foreground-process fixture requires the PTY; an alias lookup
    // sources the same rc file without one.
    let shell = format!("[[ -t 0 ]] || return\n{shell}");
    let mut child = RuntimeChild::spawn_with_zshrc(root, &shell)?;
    child.ready()?;
    if mode == "baseline" {
        let pid = child.shell_pid()?;
        let db = Connection::open_with_flags(
            child.data.join("matou.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let deadline = Instant::now() + TIMEOUT;
        // The PID arrives before the Shell's command-completion marker can
        // settle its work status. This scenario starts shutdown from idle.
        loop {
            let idle: bool = db.query_row(
                "SELECT work_status='idle' FROM sessions WHERE id=?",
                [&child.session],
                |row| row.get(0),
            )?;
            if idle {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "baseline Shell did not return to idle after reporting its PID"
            );
            thread::sleep(Duration::from_millis(5));
        }
        return Ok((child, pid, None));
    }

    let record = wait_for_fixture(&root.join("child.jsonl"), "ready")?;
    let pid = u32::try_from(record["pid"].as_u64().context("fixture PID")?)?;
    let pgid = record["pgid"].as_i64().context("fixture PGID")?;
    assert_eq!(pgid, i64::from(pid));
    if mode == "nested" {
        let shell: u32 = fs::read_to_string(root.join("shell-pid"))?.parse()?;
        assert_ne!(shell, pid);
        assert_eq!(record["ppid"].as_u64(), Some(u64::from(shell)));
        assert_eq!(record["foreground_pgid"].as_i64(), Some(pgid));
        assert_eq!(
            unsafe { libc::getsid(pid as libc::pid_t) },
            shell as libc::pid_t
        );
        Ok((child, shell, Some(pid)))
    } else {
        Ok((child, pid, None))
    }
}

fn watch_exits(pids: &[u32]) -> Result<OwnedFd> {
    let queue = unsafe { libc::kqueue() };
    ensure!(queue >= 0, "kqueue: {}", std::io::Error::last_os_error());
    let queue = unsafe { OwnedFd::from_raw_fd(queue) };
    for &pid in pids {
        let change = libc::kevent {
            ident: pid as usize,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: ptr::null_mut(),
        };
        let result = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                ptr::null_mut(),
                0,
                ptr::null(),
            )
        };
        ensure!(
            result == 0,
            "watch exit for {pid}: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(queue)
}

fn drain_exits(queue: &OwnedFd, exits: &mut HashSet<u32>) -> Result<()> {
    let timeout = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
    loop {
        let count = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                ptr::null(),
                0,
                event.as_mut_ptr(),
                1,
                &timeout,
            )
        };
        match count {
            0 => return Ok(()),
            1 => {
                let event = unsafe { event.assume_init() };
                let (pid, flags) = (event.ident, event.fflags);
                ensure!(flags & libc::NOTE_EXIT != 0, "unexpected process event");
                exits.insert(u32::try_from(pid)?);
            }
            _ => return Err(std::io::Error::last_os_error().into()),
        }
    }
}

#[test]
fn clean_quit_waits_for_direct_shell_and_foreground_group_before_publishing() -> Result<()> {
    let foreign_root = tempfile::tempdir()?;
    let (mut foreign, foreign_shell, _) = start_fixture(foreign_root.path(), "ignored")?;
    let foreign_log = foreign_root.path().join("child.jsonl");
    let foreign_exits = watch_exits(&[foreign_shell])?;

    // The upper bounds include both runtime scheduling and the SQLite backup.
    // Parallel runs have published handled/ignored pairs in 2857/3267 ms.
    for (mode, grace_min, grace_max, expected_signal) in [
        ("baseline", 0, 1000, 0),
        ("handled", 900, 3250, 0),
        ("ignored", 1400, 3500, 9),
        ("nested", 1400, 2750, 0),
    ] {
        let root = tempfile::tempdir()?;
        let (mut child, shell, foreground) = start_fixture(root.path(), mode)?;
        let runtime = child.child.id();
        let pids: Vec<_> = [Some(shell), foreground, Some(runtime)]
            .into_iter()
            .flatten()
            .collect();
        let queue = watch_exits(&pids)?;
        let before = Instant::now();
        let requested_at = SystemTime::now();
        child.send(Command::Shutdown)?;
        let mut exits = HashSet::new();
        let path = loop {
            // Read directory entries before draining the already queued process events.
            let backup = fs::read_dir(child.data.join("backups"))?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .find(|path| {
                    path.to_string_lossy().contains("-clean-exit-")
                        && path.extension().is_some_and(|ext| ext == "sqlite")
                });
            drain_exits(&queue, &mut exits)?;
            if let Some(path) = backup {
                for pid in [Some(shell), foreground].into_iter().flatten() {
                    ensure!(
                        exits.contains(&pid),
                        "{mode}: backup published before PID {pid} exited"
                    );
                }
                if path.with_extension("json").is_file() {
                    break path;
                }
            }
            ensure!(
                !exits.contains(&runtime),
                "{mode}: runtime exited before the backup pair"
            );
            ensure!(
                before.elapsed() < TIMEOUT,
                "{mode}: clean-exit backup timeout"
            );
            thread::sleep(Duration::from_millis(1));
        };
        // On macOS the final manifest rename stamps ctime. Measure when the
        // pair was published even if this test was descheduled while polling.
        let manifest = fs::metadata(path.with_extension("json"))?;
        let published_at = UNIX_EPOCH
            + Duration::new(
                u64::try_from(manifest.ctime())?,
                u32::try_from(manifest.ctime_nsec())?,
            );
        let elapsed = published_at.duration_since(requested_at)?.as_millis();
        ensure!(
            (grace_min..grace_max).contains(&elapsed),
            "{mode}: backup published after {elapsed} ms"
        );
        let diagnostics = child.closed()?;
        let (status, stderr) = child.wait()?;
        ensure!(
            status.success() && diagnostics.is_empty(),
            "{mode}: {stderr}, {diagnostics:?}"
        );
        drain_exits(&queue, &mut exits)?;
        ensure!(
            exits.contains(&runtime),
            "{mode}: missing runtime exit event"
        );
        assert!(fs::read_dir(child.data.join("backups"))?.all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial")
        }));
        let snapshot = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        assert_eq!(
            snapshot.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?,
            "ok"
        );
        let (ended, signal): (bool, u32) = snapshot.query_row(
            "SELECT ended_at IS NOT NULL, signal FROM session_runs WHERE session_id = ?",
            [&child.session],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert!(ended);
        if mode != "nested" {
            assert_eq!(signal, expected_signal, "{mode}");
        }
        if mode != "baseline" {
            let log = root.path().join("child.jsonl");
            assert_eq!(wait_for_fixture(&log, "signal")?["signal"], "SIGHUP");
            assert_eq!(
                fs::read_to_string(log)?.contains("\"event\": \"self_exit\""),
                mode == "handled"
            );
        }
        let mut unrelated = HashSet::new();
        drain_exits(&foreign_exits, &mut unrelated)?;
        ensure!(
            unrelated.is_empty()
                && !fs::read_to_string(&foreign_log)?.contains("\"event\": \"signal\""),
            "{mode}: shutdown reached a different PTY"
        );
        println!(
            "{mode}: shell={shell} foreground={foreground:?} backup={elapsed} ms runtime={runtime}"
        );
    }

    foreign.send(Command::Shutdown)?;
    let _ = foreign.closed()?;
    assert!(foreign.wait()?.0.success());
    Ok(())
}
