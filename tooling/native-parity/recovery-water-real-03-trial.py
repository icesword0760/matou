"""Take window evidence and signal only the verified disposable runtime."""
import json
import os
from pathlib import Path
import re
import signal
import sqlite3
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2] / "artifacts/native-parity/native/evidence/recovery-water-real-03"
run = ROOT / sys.argv[1]
window = int(sys.argv[2])
launch = json.loads((run / "launch.json").read_text())
old_owner = launch["pid"]
capture = ROOT.with_name("recovery-water-real-02") / "capture-window"
ui = ROOT / "ui"


def json_out(name, value):
    path = run / (name + ".json")
    scratch = path.with_suffix(".tmp")
    scratch.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    scratch.replace(path)


def processes():
    output = subprocess.check_output(["ps", "-axo", "pid=,ppid=,command=", "-ww"], text=True)
    for row in output.splitlines():
        pid, parent, command = row.split(maxsplit=2)
        yield int(pid), int(parent), command


def runtimes():
    rows = list(processes())
    owner = [command for pid, _, command in rows if pid == old_owner]
    bundle = Path(launch["app"])
    if not bundle.is_relative_to(run) or owner != [" ".join(launch["command"])]:
        raise RuntimeError("GUI exact process and isolated path mismatch")
    same_gate = [(pid, command) for pid, _, command in rows
                 if str(ROOT) + "/" in command.split(" ", 1)[0] and "/Contents/MacOS/" in command.split(" ", 1)[0]
                 and "/Contents/Frameworks/" not in command.split(" ", 1)[0]
                 and " --data-root " in command and " --shell " not in command]
    if launch["side"] == "native" and [pid for pid, _ in same_gate] != [old_owner]:
        raise RuntimeError(f"Unexpected isolated native GUI processes: {same_gate}")

    def matches(cmd):
        if launch["side"] == "native":
            return (cmd.startswith(launch["runtime_executable"] + " ")
                    and f"--data-root {launch['data_alias']} " in cmd
                    and f"--shell-home {launch['paths']['home']} " in cmd)
        return (cmd.startswith(str(bundle / "Contents/Frameworks") + "/")
                and "--utility-sub-type=node.mojom.NodeService " in cmd
                and f"--user-data-dir={launch['paths']['user-data']} " in cmd)

    return {pid: cmd for pid, parent, cmd in rows if parent == old_owner and matches(cmd)}


def snapshot(stage):
    database = Path(launch["paths"]["data"]) / "matou.sqlite"
    sql = {
        "sessions": "SELECT id,title,kind,status,work_status FROM sessions ORDER BY created_at,id",
        "runs": "SELECT id,session_id,ordinal,runtime_generation,pid,status,started_at,ended_at FROM session_runs ORDER BY session_id, ordinal",
        "focus": "SELECT * FROM window_scene_focus ORDER BY scene_id",
        "geometry": "SELECT * FROM scene_geometry ORDER BY scene_id,owner_key",
        "history": "SELECT id,session_id,command_text,output,exit_code FROM shell_history_blocks ORDER BY id",
    }
    def query(db, querystring):
        try:
            return [dict(row) for row in db.execute(querystring)]
        except sqlite3.OperationalError as exc:
            return {"read_error": str(exc)}
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as db:
        db.row_factory = sqlite3.Row
        json_out(stage, {"unix_ns": time.time_ns(), **{name: query(db, querystring) for name, querystring in sql.items()}})


keylog = run / "during-ui.jsonl"
if not re.search(r'"event":"watch_armed"', keylog.read_text()):
    raise RuntimeError("Terra has not armed guarded system input")
trace = [json.loads(line) for line in (run / "pty.jsonl").read_text().splitlines()]
if not any(row["event"] == "input" and row["hex"] == "61" for row in trace):
    raise RuntimeError("Smoke test did not receive a as a raw PTY byte")
before = runtimes()
if len(before) != 1:
    raise RuntimeError(f"Expected exactly one identifiable runtime; saw {before}")
old = next(iter(before))
snapshot("before")
frames = run / "frames"
frames.mkdir()
with (run / "frames.jsonl").open("x") as manifest, (run / "capture.stderr").open("x") as err:
    recorder = subprocess.Popen([str(capture), str(old_owner), str(window), "1.85", str(frames)],
                                stdout=subprocess.PIPE, stderr=err, text=True)
    crash = None
    try:
        for line in recorder.stdout:
            manifest.write(line)
            manifest.flush()
            row = json.loads(line)
            if row["event"] != "frame":
                continue
            if crash is None:
                if row["index"] < 10:
                    continue
                identity = json.loads(subprocess.check_output(
                    [str(ui), str(run / "launch.json"), "guard", str(window)], text=True))
                if runtimes() != before:
                    raise RuntimeError("Runtime identity changed before kill")
                crash = {"verified_window": identity, "runtime_pid": old,
                         "runtime_command": before[old], "runtime_parent_pid": old_owner,
                         "before_signal_unix_ns": time.time_ns()}
                os.kill(old, signal.SIGKILL)
                crash["after_signal_unix_ns"] = time.time_ns()
                crash["after_signal_uptime_ns"] = time.monotonic_ns()
                json_out("crash", crash)
                print(f"SIGKILL sent to isolated runtime {old}, owner {old_owner}, window {window}", flush=True)
            elif "replacement_pid" not in crash:
                after = runtimes()
                new = {pid: command for pid, command in after.items() if pid != old}
                if len(new) == 1:
                    pid, command = next(iter(new.items()))
                    crash.update(replacement_pid=pid, replacement_command=command,
                                 replacement_seen_unix_ns=time.time_ns())
                    json_out("replacement", crash)
    finally:
        if recorder.poll() is None and crash is None:
            recorder.terminate()
        status = recorder.wait(timeout=15)
    if status != 0 or crash is None:
        raise RuntimeError(f"Capture or crash failed: {status}")
snapshot("after-before-key")
if "replacement_pid" not in crash:
    json_out("replacement-not-seen", {"after_capture": runtimes()})
print(f"Finished raw frames and readback in {run}", flush=True)
