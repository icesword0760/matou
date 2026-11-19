#!/usr/bin/env python3
"""Verify the native runtime against an installed Claude Code CLI on macOS."""

import argparse
import contextlib
import errno
import getpass
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import threading
import time
from uuid import uuid4


REPO = Path(__file__).resolve().parents[2]
TARGET = REPO / "apps/native/target"
CHECKS = TARGET / "provider-launch-checks"
BINARY = TARGET / "debug/matou-native-runtime"
OFFICIAL = {
    "id": "anthropic-official", "cli": "claude-code", "name": "Anthropic",
    "endpoint": "https://api.anthropic.com", "model": "", "builtIn": True,
}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


KEYCHAIN_LOOKUP = ["/usr/bin/security", "find-generic-password", "-s", "Claude Code-credentials"]
DENIED = {errno.EACCES, errno.EPERM}


def write_json(path, data, *, indent=2):
    path.write_text(json.dumps(data, indent=indent) + "\n", encoding="utf-8")


def keychain_status():
    return subprocess.run(
        KEYCHAIN_LOOKUP, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL, timeout=10, check=False,
    ).returncode


def snapshot_file(path):
    def stamp(metadata):
        return metadata.st_dev, metadata.st_ino, metadata.st_mtime_ns, metadata.st_size

    for _ in range(3):
        metadata = path.lstat()
        stable = True
        if stat.S_ISLNK(metadata.st_mode):
            kind = "symlink"
            digest = hashlib.sha256(os.fsencode(os.readlink(path))).hexdigest()
        elif stat.S_ISREG(metadata.st_mode):
            kind = "file"
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            with os.fdopen(fd, "rb") as file:
                opened = os.fstat(file.fileno())
                if not stat.S_ISREG(opened.st_mode) or stamp(opened) != stamp(metadata):
                    continue
                digest = hashlib.file_digest(file, "sha256").hexdigest()
                metadata = os.fstat(file.fileno())
                stable = stamp(opened) == stamp(metadata)
        elif stat.S_ISDIR(metadata.st_mode):
            return None
        else:
            kind, digest = "special", None

        entry = dict(size=metadata.st_size, mtime_ns=metadata.st_mtime_ns, sha256=digest, kind=kind)
        if stable and stamp(metadata) == stamp(path.lstat()):
            return entry
    return dict(size=metadata.st_size, mtime_ns=metadata.st_mtime_ns,
                sha256=None, changed_while_hashing=True)


def audit(home):
    """Snapshot paths and metadata. Only hashes derived from file contents are stored."""
    files = {}

    def add(path):
        try:
            entry = snapshot_file(path)
        except FileNotFoundError:
            return
        if entry is not None:
            files[str(path.relative_to(home))] = entry

    for directory, dirs, names in os.walk(home / ".claude", followlinks=False):
        for name in sorted(dirs + names):
            add(Path(directory) / name)
    add(home / ".claude.json")
    return {"files": files, "keychain_lookup_exit_code": keychain_status()}


def process_table():
    rows = subprocess.check_output(
        ["/bin/ps", "-Aww", "-o", "pid=,ppid=,lstart=,comm="],
        text=True, timeout=6,
    ).splitlines()
    processes = {}
    for row in rows:
        pid, ppid, weekday, month, day, clock, year, command = row.split(maxsplit=7)
        processes[int(pid)] = {
            "ppid": int(ppid), "start": [weekday, month, day, clock, year], "command": command,
        }
    return processes


def is_descendant(pid, root_pid, processes):
    while pid > 1 and pid in processes:
        if pid == root_pid:
            return True
        pid = processes[pid]["ppid"]
    return False


def open_processes(path):
    result = subprocess.run(
        ["/usr/sbin/lsof", "-nP", "-w", "-Fpcfan", "--", str(path)],
        text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=6, check=False,
    )
    owners = {}
    pid, command, access = None, "", ""
    for line in result.stdout.splitlines():
        if line.startswith("p"):
            pid = int(line[1:])
            command, access = "", ""
        elif line.startswith("c"):
            command = line[1:]
        elif line.startswith("f"):
            access = ""
        elif line.startswith("a"):
            access = line[1:]
        elif line.startswith("n") and pid is not None and access in {"w", "u"}:
            owners[pid] = {"pid": pid, "name": command, "access": access}
    return list(owners.values())


def changed_paths(before, after):
    old_files, new_files = before["files"], after["files"]
    return sorted(
        path for path in old_files.keys() | new_files.keys()
        if old_files.get(path) != new_files.get(path)
        or old_files.get(path, {}).get("changed_while_hashing")
    )


def compare_audits(home, control_before, control_after, test_before, test_after, preexisting):
    control_diff = changed_paths(control_before, control_after)
    test_diff = changed_paths(test_before, test_after)
    new_in_test = sorted(set(test_diff) - set(control_diff))
    attributed = []
    unattributed = []
    for path in new_in_test:
        try:
            owners = open_processes(home / path)
        except subprocess.TimeoutExpired:
            owners = []
        current = process_table() if owners else {}
        ambient = []
        for owner in owners:
            pid = owner["pid"]
            existing, observed = preexisting.get(pid), current.get(pid)
            if (existing and observed and existing["start"] == observed["start"]
                    and existing["command"] == observed["command"]
                    and not is_descendant(pid, os.getpid(), current)):
                ambient.append(owner)
        if owners and len(ambient) == len(owners):
            attributed.append({"path": path, "kind": "ambient", "processes": ambient})
        else:
            unattributed.append({"path": path, "observed_writers": owners})
    control_keys = [view["keychain_lookup_exit_code"] for view in (control_before, control_after)]
    test_keys = [view["keychain_lookup_exit_code"] for view in (test_before, test_after)]
    return {
        "evidence_priority": {"primary": "sandbox denial probes", "supplementary": "control-window audit"},
        "control_diff": {"paths": control_diff, "count": len(control_diff)},
        "test_diff": {"paths": test_diff, "count": len(test_diff)},
        "new_in_test": {"paths": new_in_test, "count": len(new_in_test)},
        "attributed": attributed,
        "unattributed": unattributed,
        "keychain_exit_codes": {"control": control_keys, "test": test_keys},
        "passed": not unattributed and len(set(control_keys + test_keys)) == 1,
    }


def sandbox(root, real_home):
    def subpath(path):
        return f"(subpath {json.dumps(str(path))})"

    protected = " ".join(subpath(real_home / name)
                         for name in (".claude", ".claude.json", "Library/Keychains"))
    return [
        "/usr/bin/sandbox-exec", "-p", f"""
        (version 1)
        (allow default)
        (deny network*)
        (deny mach-lookup)
        (deny process-exec (literal "/usr/bin/security"))
        (deny file-read* {protected})
        (deny file-write*)
        (allow file-write* {subpath(root)} (literal "/dev/null")
            (literal "/dev/ptmx") (regex #"^/dev/ttys[0-9]+$"))
        """,
    ]


def probe_sandbox(root, home, cli, guard, proof):
    """Test the exact policy and environment used for the CLI against real paths."""
    env = isolated_environment(root / "probe", cli, cli)
    write_path = home / ".claude" / f".matou-sandbox-probe-{uuid4().hex}"
    program = """
import json
import os
from pathlib import Path
import subprocess
import sys

directory, config, target = map(Path, sys.argv[1:4])

def read_directory():
    with os.scandir(directory) as entries:
        next(entries, None)

def read_config():
    with config.open("rb"):
        pass

def query_keychain():
    subprocess.run(sys.argv[4:], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, check=True, timeout=8)

def create_file():
    fd = os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        os.close(fd)
    finally:
        target.unlink()

result = {}
for name, operation in (
        ("read_claude_directory", read_directory), ("read_claude_config", read_config),
        ("query_keychain", query_keychain), ("create_claude_file", create_file)):
    try:
        operation()
    except OSError as error:
        result[name] = {"errno": error.errno}
    except subprocess.CalledProcessError as error:
        result[name] = {"exit_code": error.returncode}
    else:
        result[name] = {"allowed": True}
print(json.dumps(result))
"""
    result = subprocess.check_output(
        [*guard, sys.executable, "-I", "-B", "-c", program, str(home / ".claude"),
         str(home / ".claude.json"), str(write_path), *KEYCHAIN_LOOKUP],
        env=env, cwd=root, stdin=subprocess.DEVNULL, text=True, timeout=12,
    )
    proof.update(json.loads(result))
    require(len(proof) == 4 and all(check.get("errno") in DENIED for check in proof.values()),
            f"Sandbox probe did not prove all four denials: {proof}")


def isolated_environment(root, cli, command):
    paths = {
        "HOME": "home",
        "CLAUDE_CONFIG_DIR": "claude",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR": "secure",
        "ZDOTDIR": "home",
        "XDG_CONFIG_HOME": "xdg/config",
        "XDG_DATA_HOME": "xdg/data",
        "XDG_CACHE_HOME": "xdg/cache",
        "XDG_STATE_HOME": "xdg/state",
        "XDG_RUNTIME_DIR": "xdg/runtime",
        "TMPDIR": "tmp",
    }
    env = {}
    for key, directory in paths.items():
        path = root / directory
        path.mkdir(parents=True, exist_ok=True, mode=0o700)
        env[key] = str(path)
    username = getpass.getuser()
    env.update({
        "PATH": f"{cli.parent}:/usr/bin:/bin", "TERM": "xterm-256color",
        "USER": username, "LOGNAME": username, "LANG": "en_US.UTF-8",
        "MATOU_CLAUDE_COMMAND": str(command), "MATOU_DISABLE_LOGIN_PATH_PROBE": "1",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        # Inherited caller markers must never prevent an independent Claude startup.
        "CLAUDECODE": "1", "CLAUDE_CODE_CHILD_SESSION": "1",
    })
    return env


class Runtime:
    def __init__(self, root, env, guard, binary):
        self.process = subprocess.Popen(
            [*guard, str(binary), "--data-root", str(root / "matou"),
             "--workspace", str(root / "workspace"), "--shell-home", str(root / "home"),
             "--shell", str(root / "missing-shell")],
            cwd=root / "workspace", env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            encoding="utf-8",
        )
        self.events = queue.Queue()
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            for line in self.process.stdout:
                self.events.put(json.loads(line))
        finally:
            self.events.put(None)

    def send(self, data):
        self.process.stdin.write(json.dumps(data) + "\n")
        self.process.stdin.flush()

    def command(self, name, args=None):
        self.send({"Command": name if args is None else {name: args}})

    def wait(self, label, accept, timeout=12):
        expires = time.monotonic() + timeout
        while True:
            try:
                message = self.events.get(timeout=max(0, expires - time.monotonic()))
            except queue.Empty:
                raise RuntimeError(f"Timed out waiting for {label}") from None
            require(message is not None, f"Native runtime closed while waiting for {label}")
            event = message.get("Event", {})
            require(event != "Closed", f"Native runtime closed while waiting for {label}")
            if isinstance(event, dict) and "StartupBlocked" in event:
                raise RuntimeError(event["StartupBlocked"]["message"])
            if isinstance(event, dict) and "Frame" in event:
                frame = event["Frame"]
                self.command("Applied", {"session_id": frame["session_id"],
                                         "sequence": frame["sequence"]})
            result = accept(message, event)
            if result is not None:
                return result

    def ready(self):
        self.send({"Hello": {"version": 1}})
        self.wait("runtime protocol Hello", lambda message, _: message.get("Hello"))
        ready = self.wait("native owner readiness", lambda _, event: event.get("Ready"))
        return ready["hierarchy"]["session"]["id"]

    def shutdown(self):
        self.command("Shutdown")
        require(self.process.wait(timeout=12) == 0, "Native runtime did not shut down cleanly")

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
        self.reader.join(timeout=2)
        self.process.stdout.close()


@contextlib.contextmanager
def runtime(root, env, guard, binary):
    process = Runtime(root, env, guard, binary)
    try:
        yield process
    finally:
        process.close()


@contextlib.contextmanager
def database(root):
    with contextlib.closing(sqlite3.connect(root / "matou/matou.sqlite")) as db, db:
        yield db


def seed(root, env, guard, binary, cwd):
    with runtime(root, env, guard, binary) as host:
        session_id = host.ready()
        host.wait("bootstrap Shell launch result",
                  lambda _, event: event.get("Recovery"))
        host.shutdown()
    with database(root) as db:
        require(db.execute("SELECT COUNT(*) FROM session_runs").fetchone() == (0,),
                "Unexpected process during bootstrap")
        db.execute("UPDATE sessions SET kind='claude-code', cwd=? WHERE id=?",
                   (str(cwd), session_id))
    return session_id


def set_config(root, requested):
    config = {
        "version": 1, "revision": 7,
        "providers": {"claude-code": [dict(OFFICIAL, id="other", model="haiku"), requested],
                      "codex": []},
        "activeProviderIds": {"claude-code": "other", "codex": "openai-official"},
    }
    path = root / "matou/provider-config.json"
    path.write_text(json.dumps(config), encoding="utf-8")
    return path, path.read_bytes()


def bind(root, session_id, metadata, *, suffix, when):
    with database(root) as db:
        db.execute(
            """INSERT INTO provider_bindings
               (id, session_id, provider, provider_session_id, resume_state,
                metadata_json, created_at, updated_at)
               VALUES (?, ?, 'claude-code', ?, 'unknown', ?, ?, ?)""",
            (f"binding-{suffix}", session_id, f"old-identity-{suffix}",
             json.dumps(metadata), when, when),
        )


def wait_failure(host, session_id, reason):
    def match(_, event):
        recovery = event.get("Recovery", {})
        if recovery.get("session_id") != session_id:
            return None
        failure = recovery["state"].get("StartFailed")
        require(failure is not None and reason in failure, "Wrong startup failure reason")
        return failure

    failure = host.wait("StartFailed event", match)
    diagnostic = host.wait("diagnostic", lambda _, event: event.get("Diagnostic"))
    require(diagnostic == failure, "Startup failure and diagnostic differ")
    return failure


def assert_no_run(root, session_id, cwd):
    with database(root) as db:
        require(db.execute("SELECT COUNT(*) FROM session_runs WHERE session_id=?",
                           (session_id,)).fetchone() == (0,), "Failed launch left a run")
        require(db.execute("SELECT cwd FROM sessions WHERE id=?",
                           (session_id,)).fetchone() == (str(cwd),), "Stored cwd changed")


def terminal_lines(frame):
    screen = frame["screen"]
    cols = screen["cols"]
    cells = screen["cells"]
    return ["".join(cell["text"] or " " for cell in cells[i:i + cols]).strip()
            for i in range(0, len(cells), cols)]


def wait_for_cli_screen(host, session_id, proof):
    def match(_, event):
        recovery = event.get("Recovery", {})
        if recovery.get("session_id") == session_id:
            state = recovery["state"]
            if isinstance(state, dict) and "StartFailed" in state:
                raise RuntimeError(state["StartFailed"])
        frame = event.get("Frame", {})
        if frame.get("session_id") != session_id:
            return None
        lines = terminal_lines(frame)
        proof["last_terminal_line"] = next((line for line in reversed(lines) if line), "")
        rendered = " ".join(" ".join(lines).split())
        for marker in ("Welcome to Claude Code", "Select login method", "Choose how to sign in",
                       "Claude Code v"):
            if marker in rendered:
                proof.update(screen=marker, first_screen_lines=[line for line in lines if line])
                return True
        return None

    host.wait("Claude Code first screen", match, timeout=18)


def verify_live_run(host, root, session_id, cwd, cli, argv, proof):
    wait_for_cli_screen(host, session_id, proof)
    with database(root) as db:
        row = db.execute("SELECT pid, profile, status FROM session_runs WHERE session_id=?",
                         (session_id,)).fetchall()
    require(len(row) == 1 and row[0][1:] == ("claude-code", "running"),
            "Expected one committed Claude Code run")
    pid = row[0][0]
    os.kill(pid, 0)
    lsof = subprocess.check_output(
        ["/usr/sbin/lsof", "-a", "-p", str(pid), "-d", "cwd,txt", "-Fn"],
        text=True, timeout=6, stderr=subprocess.DEVNULL,
    )
    require(f"fcwd\nn{cwd}\n" in lsof, "Claude Code process cwd does not match the record")
    require(f"n{cli}\n" in lsof, "PTY process did not load the real installed executable")
    args = subprocess.check_output(["/bin/ps", "-ww", "-p", str(pid), "-o", "args="],
                                   text=True, timeout=5).strip()
    require(args == " ".join((str(cli), *argv)), "Claude Code process argv differs")
    proof.update(pid=pid, argv=argv, cwd="workspace/session-cwd",
                 executable=str(cli), committed_runs=1, process_alive=True)
    proof["exit_input"] = "Ctrl+C twice"
    for _ in range(2):
        host.command("Input", {"session_id": session_id, "bytes": [3], "defer_ordering": False})
        time.sleep(0.25)

    def exited(_, event):
        frame = event.get("Frame", {})
        if frame.get("session_id") == session_id:
            proof["last_terminal_line"] = next(
                (line for line in reversed(terminal_lines(frame)) if line),
                proof["last_terminal_line"],
            )
        with database(root) as db:
            result = db.execute(
                "SELECT status, exit_code, signal FROM session_runs WHERE session_id=?",
                (session_id,),
            ).fetchall()
        if len(result) == 1 and result[0][0] == "exited":
            return result[0]
        return None

    status, code, signal = host.wait("Claude Code exit", exited, timeout=10)
    require(isinstance(code, int) and isinstance(signal, int), "Run has no actual code or signal")
    proof.update(run_status=status, exit_code=code, signal=signal)
    host.command("Refresh")
    final_frame = host.wait("final Claude Code frame", lambda _, event: (
        event["Frame"] if event.get("Frame", {}).get("session_id") == session_id else None
    ))
    proof["last_terminal_line"] = next(
        (line for line in reversed(terminal_lines(final_frame)) if line), "")

    require(subprocess.run(
        ["/bin/ps", "-p", str(pid)], stdout=subprocess.DEVNULL, check=False,
    ).returncode == 1, "Claude Code PID still exists after exit")
    proof["process_reaped"] = True
    with database(root) as db:
        require(db.execute("SELECT cwd FROM sessions WHERE id=?",
                           (session_id,)).fetchone() == (str(cwd),), "Stored cwd changed")
        require(db.execute("SELECT COUNT(*) FROM shell_history_blocks WHERE session_id=?",
                           (session_id,)).fetchone() == (0,), "Claude Code wrote Shell history")
    proof["shell_history_blocks"] = 0


def run_cases(root, cli, binary, guard, cases):
    for case in ("official", "missing-command", "non-executable",
                 "missing-cwd", "missing-binding", "config-retry"):
        case_root = root / case
        cwd = case_root / "workspace/session-cwd"
        (case_root / "matou").mkdir(parents=True)
        cwd.parent.mkdir()
        if case != "missing-cwd":
            cwd.mkdir()
        command = cli
        if case in ("missing-command", "non-executable"):
            command = case_root / "unavailable-claude"
        if case == "non-executable":
            command.touch(mode=0o600)
        env = isolated_environment(case_root, cli, command)
        if case == "official":
            version = subprocess.check_output(
                [*guard, str(cli), "--version"], text=True, env=env, cwd=cwd, timeout=10,
                stderr=subprocess.DEVNULL,
            ).strip()
            require("Claude Code" in version, "The executable did not identify as Claude Code")
        session_id = seed(case_root, env, guard, binary, cwd)
        path, contents = set_config(case_root, OFFICIAL)
        binding = {"providerConfigId": OFFICIAL["id"]}
        if case == "missing-binding":
            binding["providerConfigId"] = "removed"
        bind(case_root, session_id, {"providerConfigId": "obsolete"}, suffix="old", when=1)
        bind(case_root, session_id, binding, suffix="latest", when=2)
        if case == "config-retry":
            path.write_bytes(b'{"version":')
            contents = path.read_bytes()

        with runtime(case_root, env, guard, binary) as host:
            require(host.ready() == session_id, "Native runtime did not open the seeded card")
            live_proof = None
            if case == "official":
                live_proof = cases[case] = {"version": version}
                verify_live_run(host, case_root, session_id, cwd, cli, [], live_proof)
            else:
                expected = {
                    "missing-command": "未检测到 Claude Code",
                    "non-executable": "Permission denied",
                    "config-retry": "provider-config.json: invalid JSON",
                    "missing-cwd": str(cwd),
                    "missing-binding": "会话绑定的供应商配置不存在",
                }
                failure = wait_failure(host, session_id, expected[case])
                assert_no_run(case_root, session_id, cwd)
                processes = process_table()
                require(not any(pid != host.process.pid
                                and is_descendant(pid, host.process.pid, processes)
                                for pid in processes), "Failed launch left a child process")
                cases[case] = {"failure": failure, "committed_runs": 0,
                               "diagnostic_matches": True, "child_processes": 0}
                if case == "config-retry":
                    require(path.read_bytes() == contents, "Corrupt provider config was rewritten")
                    path, contents = set_config(
                        case_root, dict(OFFICIAL, model="sonnet", reasoningEffort="low"),
                    )
                    with database(case_root) as db:
                        db.execute("UPDATE provider_bindings SET metadata_json=? WHERE id=?",
                                   (json.dumps(dict(binding, permissionMode="plan")), "binding-latest"))
                    for _ in range(2):
                        host.command("RestartSession", {"session_id": session_id})
                    live_proof = cases[case]["retry"] = {"version": version}
                    verify_live_run(
                        host, case_root, session_id, cwd, cli,
                        ["--model", "sonnet", "--effort", "low", "--permission-mode", "plan"],
                        live_proof,
                    )
            require(path.read_bytes() == contents, "Provider configuration was rewritten")
            require(not (case_root / "matou/shell-integration").exists(),
                    "Provider launch installed Shell integration")
            host.shutdown()
        if live_proof is not None:
            # Shutdown drains and joins the worker that served the final screen.
            live_proof["worker_joined_on_runtime_shutdown"] = True
            expected_exit = (live_proof["run_status"], live_proof["exit_code"], live_proof["signal"])
            with database(case_root) as db:
                require(db.execute(
                    "SELECT status, exit_code, signal FROM session_runs WHERE session_id=?",
                    (session_id,),
                ).fetchall() == [expected_exit], "Run exit record changed during shutdown")
        print(f"{case}: passed", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, default=BINARY)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--window-seconds", type=float, default=10)
    parser.add_argument("--evidence", type=Path, default=CHECKS / "claude-acceptance.json")
    args = parser.parse_args()
    require(0 < args.window_seconds < 300, "Window duration must be between 0 and 300 seconds")
    require(sys.platform == "darwin", "This isolation harness requires macOS sandbox-exec")
    require(args.claude is not None, "Supply the installed CLI using --claude")
    cli = args.claude.resolve(strict=True)
    binary = args.runtime.resolve(strict=True)
    evidence = args.evidence.resolve()
    require(evidence.is_relative_to(REPO), "Evidence must stay in the workspace")
    require(os.access(cli, os.X_OK) and os.access(binary, os.X_OK), "CLI or runtime is not executable")
    CHECKS.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve(strict=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    snapshots = {name: evidence.with_name(f"{evidence.stem}.{name}.json")
                 for name in ("control-before", "control-after", "test-before", "test-after")}

    def save_snapshot(name):
        snapshot = audit(home)
        write_json(snapshots[name], snapshot, indent=None)
        return snapshot

    with tempfile.TemporaryDirectory(prefix="claude-isolated-", dir=CHECKS) as directory:
        root = Path(directory)
        guard = sandbox(root, home)
        proof = {"snapshots": {name: str(path) for name, path in snapshots.items()},
                 "sandbox_probe": {}, "cases": {}, "windows_seconds": {}}
        control_before = save_snapshot("control-before")
        control_start = time.monotonic()
        time.sleep(args.window_seconds)
        proof["windows_seconds"]["control"] = round(time.monotonic() - control_start, 3)
        control_after = save_snapshot("control-after")

        preexisting = process_table()
        test_before = save_snapshot("test-before")
        test_start = time.monotonic()
        try:
            probe_sandbox(root, home, cli, guard, proof["sandbox_probe"])
            print("Sandbox probe: all four operations denied", flush=True)
            run_cases(root, cli, binary, guard, proof["cases"])
        except Exception as error:
            proof["case_error"] = str(error)
            raise
        finally:
            proof["windows_seconds"]["active_test"] = round(time.monotonic() - test_start, 3)
            remaining = args.window_seconds - (time.monotonic() - test_start)
            if remaining > 0:
                time.sleep(remaining)
            proof["windows_seconds"]["test"] = round(time.monotonic() - test_start, 3)
            test_after = save_snapshot("test-after")
            proof["isolation"] = compare_audits(
                home, control_before, control_after, test_before, test_after, preexisting,
            )
            proof["isolation"]["sandbox_passed"] = (
                len(proof["sandbox_probe"]) == 4
                and all(check.get("errno") in DENIED for check in proof["sandbox_probe"].values())
            )
            proof["passed"] = (
                proof["isolation"]["passed"] and proof["isolation"]["sandbox_passed"]
                and "case_error" not in proof
            )
            write_json(evidence, proof)
            print(f"Isolation audit and case evidence: {evidence}", flush=True)
            require(proof["isolation"]["passed"], "Test-only change was unattributed or keychain status changed.")


if __name__ == "__main__":
    main()
