"""Launch one fresh isolated run, retain process and filesystem evidence, and reap it."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import select
import shlex
import shutil
import signal
import subprocess
import sys
import threading
import time


TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parents[1]
EVIDENCE_ROOT = ROOT / "artifacts/native-parity/native/evidence"
SOURCE = EVIDENCE_ROOT / "quit-real-01/MatouNative.app"
INSTALL = "/Applications/码头.app/Contents/MacOS/码头"


def write_json(path, value):
    with path.open("x", encoding="utf-8") as file:
        json.dump(value, file, ensure_ascii=False, indent=2)
        file.write("\n")


def load_lines(path):
    if not path.exists():
        return []
    lines = path.read_text().splitlines(keepends=True)
    return [json.loads(line) for line in lines if line.endswith("\n")]


def command(*args):
    return subprocess.check_output(args, encoding="utf-8", errors="replace").strip()


def process_table():
    out = command("/bin/ps", "-axww", "-o", "pid=,ppid=,pgid=,state=,tty=,comm=,args=")
    # comm is allowed to contain spaces. Use the shorter table for exact executable paths.
    executable_lines = command("/bin/ps", "-axww", "-o", "pid=,comm=").splitlines()
    executable = {int(line.strip().split(maxsplit=1)[0]): line.strip().split(maxsplit=1)[1]
                  for line in executable_lines if len(line.strip().split(maxsplit=1)) == 2}
    rows = {}
    for line in out.splitlines():
        columns = line.strip().split(maxsplit=5)
        if len(columns) != 6 or int(columns[0]) not in executable:
            continue
        pid, ppid, pgid = map(int, columns[:3])
        payload = columns[5]
        path = executable[pid]
        args = payload.removeprefix(path).strip() if payload.startswith(path) else payload
        rows[pid] = dict(pid=pid, ppid=ppid, pgid=pgid, state=columns[3],
                         tty=columns[4], executable=path, args=args)
    return rows


def native_rows(rows):
    return [row for row in rows.values()
            if Path(row["executable"]).name in ("MatouNative", "matou-native-desktop", "matou-native-runtime")]


def branch(rows, parent):
    ids = {parent}
    while True:
        updated = ids | {pid for pid, row in rows.items() if row["ppid"] in ids}
        if updated == ids:
            return [rows[pid] for pid in sorted(ids) if pid in rows]
        ids = updated


def prepare(binaries):
    if not SOURCE.is_dir():
        raise RuntimeError("The identified existing native build is missing")
    subprocess.run(["/bin/cp", "-cR", str(SOURCE), str(APP)], check=True)
    sources = {GUI: binaries / "matou-native-desktop", RUNTIME: binaries / RUNTIME.name}
    for target, source in sources.items():
        shutil.copy2(source, target)
    info = APP / "Contents/Info.plist"
    image = plistlib.loads(info.read_bytes())
    image["CFBundleIdentifier"] = "com.matou." + EVIDENCE.name
    info.write_bytes(plistlib.dumps(image))
    subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(APP)], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(APP)], check=True)
    parts = {}
    for target, source in sources.items():
        old_uuid = command("dwarfdump", "--uuid", str(source)).split()[1]
        new_uuid = command("dwarfdump", "--uuid", str(target)).split()[1]
        if old_uuid != new_uuid:
            raise RuntimeError(f"Payload executable UUID changed: {target}")
        parts[target.name] = dict(uuid=new_uuid, source=str(source),
                                  source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                                  run_sha256=hashlib.sha256(target.read_bytes()).hexdigest())
    write_json(EVIDENCE / "build.json", dict(source=str(SOURCE), app=str(APP), bundle=image["CFBundleIdentifier"],
                                           payload=parts, machine=command("uname", "-m"),
                                           macos=command("sw_vers", "-productVersion")))
    cache = EVIDENCE / ".cache"
    cache.mkdir(exist_ok=True)
    subprocess.run(["xcrun", "swiftc", "-module-cache-path", str(cache),
                    str(TOOLS / "quit-real-01-window.swift"), "-o", str(PROBE)], check=True)


def run(case, mode):
    directory = EVIDENCE / case
    directory.mkdir()
    paths = {name: directory / name for name in ("home", "workspace", "data", "cache", "tmp")}
    for path in paths.values():
        path.mkdir()
    inode = paths["data"].stat()
    short = Path(f"/.vol/{inode.st_dev}/{inode.st_ino}")
    if not short.samefile(paths["data"]):
        raise RuntimeError("Short socket alias does not identify the isolated data root")
    if mode == "baseline":
        rc = 'printf "QUIT-REAL-01 baseline: Shell ready\\n"\n'
    else:
        child = shlex.join((str(Path(sys.executable).resolve()), "-I", "-B",
                            str(TOOLS / "quit-real-01-child.py"), mode, str(directory)))
        rc = f"exec {child}\n" if mode != "nested" else f"{child}\n"
    (paths["home"] / ".zshrc").write_text(rc)
    initial = process_table()
    write_json(directory / "process-cleanroom.json",
               dict(native=native_rows(initial), installed=[row["pid"] for row in initial.values()
                                                            if row["executable"] == INSTALL]))
    if native_rows(initial):
        raise RuntimeError("Another native instance is present, leaving it untouched")
    env = {name: os.environ[name] for name in ("USER", "LOGNAME", "LANG", "LC_ALL") if name in os.environ}
    env.update(PATH="/usr/bin:/bin:/usr/sbin:/sbin", SHELL="/bin/zsh", HOME=str(paths["home"]),
               ZDOTDIR=str(paths["home"]), TMPDIR=str(paths["tmp"]) + "/", XDG_CACHE_HOME=str(paths["cache"]))
    argv = [str(GUI), "--data-root", str(short), "--workspace", str(paths["workspace"]),
            "--shell-home", str(paths["home"])]
    with (directory / "app.log").open("xb") as log:
        app = subprocess.Popen(argv, env=env, cwd=paths["workspace"], stdout=log,
                               stderr=subprocess.STDOUT, start_new_session=True)
    write_json(directory / "launch.json", dict(pid=app.pid, argv=argv, data=str(paths["data"]),
                                               data_alias=str(short), mode=mode))
    observed = {}
    probe = None
    registered = set()
    notifications = []
    monitor = None
    exit_queue = select.kqueue()

    def watch_exits():
        while True:
            try:
                events = exit_queue.control(None, 16, 0.1)
            except (OSError, ValueError):
                return
            for event in events:
                notifications.append(dict(event="process_exit_notification", pid=event.ident,
                                          note=event.fflags, unix_ns=time.time_ns()))

    try:
        ready_by = time.monotonic() + 12
        while time.monotonic() < ready_by:
            observed = {row["pid"]: row for row in branch(process_table(), app.pid)}
            runtime = next((row for row in observed.values() if row["executable"] == str(RUNTIME)
                            and str(short) in row["args"]), None)
            terminal = [row for row in observed.values() if runtime and row["ppid"] == runtime["pid"]
                        and row["tty"] not in ("??", "?")]
            if terminal and (mode == "baseline" or load_lines(directory / "child.jsonl")):
                break
            if app.poll() is not None:
                raise RuntimeError("Isolated GUI exited before the Shell was ready")
            time.sleep(0.035)
        else:
            raise RuntimeError("The isolated Shell did not become ready")
        write_json(directory / "process-before.json", list(observed.values()))
        for pid in observed:
            exit_queue.control([select.kevent(pid, filter=select.KQ_FILTER_PROC,
                                               flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                                               fflags=select.KQ_NOTE_EXIT)], 0, 0)
            registered.add(pid)
        monitor = threading.Thread(target=watch_exits, daemon=True)
        monitor.start()
        with (directory / "window.jsonl").open("xb") as log, (directory / "window.stderr.log").open("xb") as errors:
            probe = subprocess.Popen([str(PROBE), str(app.pid), str(APP), str(directory)],
                                     stdout=log, stderr=errors)
        files = {}
        last_scan = time.time_ns()
        post_ns = None
        probe_deadline = time.monotonic() + 25
        app_exit = None
        sampled_after_gui = False
        with (directory / "observations.jsonl").open("x", buffering=1) as log:
            def record(event, **fields):
                log.write(json.dumps({"event": event, "unix_ns": time.time_ns(), **fields}) + "\n")

            while time.monotonic() < probe_deadline:
                scan_begin = time.time_ns()
                present = {}
                for path in (paths["data"] / "backups").glob("*"):
                    try:
                        st = path.stat()
                    except FileNotFoundError:
                        continue
                    present[path.name] = st.st_size
                    if path.name not in files:
                        record("backup_first_seen", name=path.name, size=st.st_size,
                               mtime_unix_ns=st.st_mtime_ns, previous_scan_begin_unix_ns=last_scan)
                for gone in files.keys() - present.keys():
                    record("backup_no_longer_seen", name=gone)
                files = present
                last_scan = scan_begin
                if post_ns is None:
                    post_ns = next((row["keydown_unix_ns"] for row in load_lines(directory / "window.jsonl")
                                    if row["event"] == "cmd_q_posted"), None)
                status = app.poll()
                if status is not None and app_exit is None:
                    app_exit = time.time_ns()
                    record("gui_wait_reaped", status=status)
                if post_ns and app_exit is None and time.time_ns() - post_ns >= 15_000_000_000:
                    record("gui_15_second_deadline")
                    kill_owned(app, short, directory)
                if probe.poll() not in (None, 0):
                    raise RuntimeError("Window guard refused the operation. See window.jsonl.")
                if app_exit is not None and time.time_ns() - app_exit > 350_000_000:
                    if not sampled_after_gui:
                        still_running = [row for pid, row in process_table().items() if pid in observed]
                        record("processes_after_gui", rows=still_running)
                        sampled_after_gui = True
                    exited = {event["pid"] for event in notifications}
                    child_wait_over = time.time_ns() - app_exit > (4 if mode == "nested" else 1) * 1_000_000_000
                    if probe.poll() is not None and (registered <= exited or child_wait_over):
                        break
                time.sleep(0.002)
            else:
                raise RuntimeError("Observation window expired")
        if post_ns is None:
            raise RuntimeError("No validated Cmd+Q event")
    finally:
        if probe and probe.poll() is None:
            probe.terminate()
            probe.wait(timeout=3)
        if app.poll() is None:
            kill_owned(app, short, directory)
        app.wait(timeout=3)
        time.sleep(0.15)
        exit_queue.close()
        write_json(directory / "exit-notifications.json", dict(registered=sorted(registered), events=notifications))
        after = process_table()
        scope = [row for row in after.values() if row["pid"] in observed or str(directory) in row["args"]
                 or str(short) in row["args"] or row["executable"] in (str(GUI), str(RUNTIME))]
        write_json(directory / "process-after.json",
                   dict(tracked=scope, native=native_rows(after), installed=[row["pid"] for row in after.values()
                                                                            if row["executable"] == INSTALL]))
        write_json(directory / "files-after.json",
                   [str(path.relative_to(directory)) for path in sorted(directory.rglob("*")) if not path.is_dir()])
    print(f"{case}: GUI exit {app.returncode}, remaining tracked PIDs {[row['pid'] for row in scope]}", flush=True)
    if scope:
        raise RuntimeError("Observed processes survived the GUI, verify and preserve this evidence before advancing")


def kill_owned(app, short, directory):
    all_rows = process_table()
    row = all_rows.get(app.pid)
    if row is None:
        return
    if row["executable"] != str(GUI) or str(short) not in row["args"]:
        raise RuntimeError("Refusing to signal any process outside this isolated launch")
    print(f"Forcibly ending verified isolated GUI PID {app.pid}", file=sys.stderr, flush=True)
    write_json(directory / "forced-stop-processes.json", branch(all_rows, app.pid))
    os.kill(app.pid, signal.SIGKILL)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixed", action="store_true", help="Use a fresh quit-fix-01 evidence directory")
    parser.add_argument("--binaries", type=Path, help="Build directory containing the desktop and runtime binaries")
    parser.add_argument("action", choices=("prepare", "run"))
    parser.add_argument("case", nargs="?")
    parser.add_argument("mode", nargs="?", choices=("baseline", "handled", "ignored", "nested"))
    args = parser.parse_args()
    EVIDENCE = EVIDENCE_ROOT / ("quit-fix-01" if args.fixed else "quit-real-01")
    APP = EVIDENCE / "MatouNative.app"
    GUI = APP / "Contents/MacOS/MatouNative"
    RUNTIME = GUI.with_name("matou-native-runtime")
    PROBE = EVIDENCE / "window-probe"
    if args.action == "prepare":
        if not args.fixed or args.binaries is None:
            parser.error("prepare requires --fixed and --binaries so the original evidence is preserved")
        if APP.exists() or PROBE.exists() or (EVIDENCE / "build.json").exists():
            parser.error("A prepared fixed bundle already exists. Preserve it before preparing a new build.")
        binaries = args.binaries.resolve(strict=True)
        if not all((binaries / name).is_file() for name in ("matou-native-desktop", RUNTIME.name)):
            parser.error("The build directory must contain both native binaries")
        EVIDENCE.mkdir(exist_ok=True)
        prepare(binaries)
    else:
        if not args.case or not args.mode or not args.case.replace("-", "").isalnum():
            parser.error("run requires a fresh case name and a fixture mode")
        run(args.case, args.mode)
