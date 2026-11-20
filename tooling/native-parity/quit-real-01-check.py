"""Read back the official clean-exit SQLite pair without opening it for writes."""

import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import stat


def readback(path):
    with sqlite3.connect(path.resolve().as_uri() + "?mode=ro&immutable=1", uri=True) as connection:
        connection.row_factory = sqlite3.Row

        def rows(query, *values):
            return [dict(row) for row in connection.execute(query, values)]

        return {
            "quick_check": [row[0] for row in connection.execute("PRAGMA quick_check")],
            "schema": rows("SELECT max(version) AS version, count(*) AS migrations FROM schema_migrations")[0],
            "table_count": connection.execute("SELECT count(*) FROM sqlite_schema WHERE type='table'").fetchone()[0],
            "workspaces": rows("SELECT id, name, root_directory FROM workspaces ORDER BY name, id"),
            "sessions": rows("SELECT id, title, kind, status, work_status FROM sessions ORDER BY id"),
            "runs": rows("SELECT id, session_id, pid, profile, status, ended_at, exit_code, signal FROM session_runs ORDER BY id"),
            "shutdown_events": rows("SELECT event_id, event_type, session_id FROM domain_events "
                                    "WHERE event_id LIKE 'runtime-shutdown-%' ORDER BY seq"),
            "latest_shell_summaries": rows("SELECT session_id, latest_lines_json FROM session_graph_summaries "
                                           "ORDER BY session_id"),
        }


def check(scene):
    data = scene / "data"
    backups = data / "backups"
    paths = sorted(backups.iterdir())
    files = [dict(name=path.name, size=path.stat().st_size, mtime_unix_ns=path.stat().st_mtime_ns)
             for path in paths if path.is_file()]
    official = [path for path in paths if "-clean-exit-" in path.name and path.suffix == ".sqlite"]
    manifests = [path for path in paths if "-clean-exit-" in path.name and path.suffix == ".json"]
    leftovers = [str(path.relative_to(data)) for path in data.rglob("*")
                 if path.is_socket() or path.name.endswith((".sqlite-wal", ".sqlite-shm", ".partial"))
                 or ".partial-" in path.name or stat.S_ISFIFO(path.lstat().st_mode)]
    assert len(official) == len(manifests) == 1, "Missing or ambiguous official clean-exit backup pair"
    backup = official[0]
    assert manifests[0] == backup.with_suffix(".json"), "Clean-exit filenames do not pair"
    descriptor = json.loads(manifests[0].read_text())
    actual_size = backup.stat().st_size
    actual_hash = hashlib.sha256(backup.read_bytes()).hexdigest()
    assert descriptor["reason"] == "clean-exit" and descriptor["id"] == backup.stem
    assert Path(descriptor["path"]).samefile(backup)
    assert (descriptor["size"], descriptor["sha256"]) == (actual_size, actual_hash)
    source_rows = readback(data / "matou.sqlite")
    backup_rows = readback(backup)
    assert backup_rows["quick_check"] == ["ok"] and source_rows["quick_check"] == ["ok"]
    assert backup_rows["schema"]["version"] == descriptor["schemaVersion"]
    assert backup_rows == source_rows, "Key rows differ between the official backup and isolated database"
    assert len(backup_rows["sessions"]) == len(backup_rows["runs"]) == 1
    assert backup_rows["sessions"][0]["kind"] == "shell"
    assert backup_rows["runs"][0]["session_id"] == backup_rows["sessions"][0]["id"]
    assert any(Path(row["root_directory"]).samefile(scene / "workspace") for row in backup_rows["workspaces"])
    assert not leftovers, f"Unexpected isolated socket, sidecar or partial file: {leftovers}"
    return dict(open_mode="mode=ro&immutable=1", files=files, manifest=descriptor,
                actual_sha256=actual_hash, critical_rows_match_live_database=True,
                residual_socket_fifo_sidecar_or_partial=leftovers, **backup_rows)


def timeline(scene):
    def read(name):
        return json.loads((scene / name).read_text())

    def lines(name):
        return [json.loads(line) for line in (scene / name).read_text().splitlines()]

    launch = read("launch.json")
    gui = launch["pid"]
    processes = read("process-before.json")
    runtime, = [row["pid"] for row in processes
                if row["ppid"] == gui and Path(row["executable"]).name == "matou-native-runtime"]
    shells = [row for row in processes if row["ppid"] == runtime and row["tty"] not in ("?", "??")]
    shell, = shells
    drained = [shell["pid"]]
    if launch["mode"] == "nested":
        child, = [row for row in processes if row["ppid"] == shell["pid"]]
        fixture = next(row for row in lines("child.jsonl") if row["event"] == "ready")
        assert child["tty"] == shell["tty"] and child["pgid"] != shell["pgid"]
        assert fixture["pid"] == child["pid"] and fixture["foreground_pgid"] == child["pgid"]
        drained.append(child["pid"])

    exits = {row["pid"]: row["unix_ns"] for row in read("exit-notifications.json")["events"]}
    observations = lines("observations.jsonl")
    publication = [item for item in observations if item["event"] == "backup_first_seen"
                   and "-clean-exit-" in item["name"] and item["name"].endswith((".sqlite", ".json"))]
    assert len(publication) == 2
    last_pty_exit = max(exits[pid] for pid in drained)
    earliest_backup = min(item["mtime_unix_ns"] for item in publication)
    pair_seen = max(item["unix_ns"] for item in publication)
    assert last_pty_exit <= earliest_backup <= pair_seen <= exits[runtime] <= exits[gui], \
        "Expected all PTY processes to exit before the backup, then runtime and GUI to exit"
    assert any(item["event"] == "gui_wait_reaped" and item["status"] == 0 for item in observations)
    assert not any(item["event"] == "gui_15_second_deadline" for item in observations)
    assert not read("process-after.json")["tracked"]

    window = lines("window.jsonl")
    posted, = [item for item in window if item["event"] == "cmd_q_posted"]
    left, = [item for item in window if item["event"] == "window_left_screen"]
    assert posted["window"] == left["window"]
    assert left["backup_complete_before_sample"], "Window left the screen before the clean-exit pair"
    assert not any(item["event"] == "window_back_on_screen" for item in window)
    if launch["mode"] != "baseline":
        assert left["last_visible_unix_ns"] >= posted["keydown_unix_ns"] + 350_000_000
        assert any(item["event"] == "screenshot_result" and item["name"] == "during-wait"
                   and item["status"] == 0 for item in window)

    def elapsed(timestamp):
        return round((timestamp - posted["keydown_unix_ns"]) / 1_000_000, 2)

    return {
        "pty_exits_ms": {pid: elapsed(exits[pid]) for pid in drained},
        "backup_pair_first_seen_ms": elapsed(pair_seen),
        "runtime_exit_ms": elapsed(exits[runtime]),
        "gui_exit_ms": elapsed(exits[gui]),
        "window_last_visible_ms": elapsed(left["last_visible_unix_ns"]),
        "window_first_offscreen_ms": elapsed(left["sample_unix_ns"]),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scene", type=Path)
    options = parser.parse_args()
    scene = options.scene.resolve(strict=True)
    evidence = Path(__file__).resolve().parents[2] / "artifacts/native-parity/native/evidence"
    if scene.parent not in (evidence / "quit-real-01", evidence / "quit-fix-01") or not (scene / "process-after.json").is_file():
        parser.error("Readback requires a completed isolated run in the quit evidence directory")
    result = check(scene)
    if scene.parent.name == "quit-fix-01":
        result["timeline"] = timeline(scene)
    with (scene / "backup-readback.json").open("x") as file:
        json.dump(result, file, ensure_ascii=False, indent=2)
        file.write("\n")
