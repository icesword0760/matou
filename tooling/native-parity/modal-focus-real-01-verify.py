"""Read back identity, PNG geometry, fixture integrity and process cleanup."""
import hashlib
import json
from pathlib import Path
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / "artifacts/native-parity/native/evidence/modal-focus-real-01"
TRIALS = ("native", "reference", "recovery/native", "recovery/reference", "fork/native", "fork/reference")
ARCHIVE = "Contents/Resources/app.asar"


def demand(condition, detail):
    if not condition:
        raise ValueError(detail)


def read(path):
    return json.loads(path.read_text())


def sha(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def dimensions(path):
    with path.open("rb") as source:
        header = source.read(24)
    demand(header.startswith(b"\x89PNG\r\n\x1a\n") and header[12:16] == b"IHDR", path)
    return struct.unpack(">II", header[16:24])


def audit_trial(name):
    folder = EVIDENCE / name
    records = [read(record) for record in sorted(folder.glob("launch*.json"))]
    if not records:
        return {"name": name, "launched": False}
    side = name.rsplit("/", 1)[-1]
    title = "Matou Native" if side == "native" else "码头"
    fixture = records[0]["fixture"]
    workspace = Path(records[0]["paths"]["workspace"])
    demand(sha(workspace / "AGENTS.md") == fixture["workspace_agents_sha256"], f"{name} outer AGENTS")
    demand(sha(workspace / "codex-project/AGENTS.md") == fixture["agents_sha256"], f"{name} inner AGENTS")
    recovery = {}
    if name.startswith("recovery/"):
        data = Path(records[0]["paths"]["data"])
        quarantined = sorted(data.glob("matou.sqlite.corrupt-*"))
        demand(len(quarantined) == 1, f"{name}: expected one quarantined database")
        recovered_sha = sha(quarantined[0])
        demand(recovered_sha == fixture["damaged_database_sha256"], f"{name}: quarantine fingerprint")
        demand(not (data / "matou.sqlite").exists(), f"{name}: an active database was created")
        recovery = {"quarantined_fixture_sha256": recovered_sha, "empty_database_created": False}
    owners = {record["pid"]: record for record in records}
    if side == "reference":
        inputs = records[0]["inputs"]
        for key, digest in (("source_bundle", "baseline_asar_sha256"), ("home_isolated_bundle", "harness_asar_sha256")):
            demand(sha(Path(inputs[key]) / ARCHIVE) == inputs[digest], f"{name} {key} archive")
    for record in records:
        demand(record["side"] == side and record["title"] == title, f"{name} launch title")
        demand(Path(record["app"]).parent == folder, f"{name} launch path")
        app = Path(record["app"])
        if side == "reference" and app.is_dir():
            demand(sha(app / ARCHIVE) == record["inputs"]["harness_asar_sha256"], f"{name} disposable ASAR")
    windows, widths, grabs = set(), set(), set()
    event_count = 0
    for log in sorted(folder.glob("*event*.jsonl")):
        for index, line in enumerate(log.read_text().splitlines(), 1):
            if not line.strip():
                continue
            event = json.loads(line)
            if "operation" not in event or "before" not in event:
                continue
            where = f"{log.relative_to(EVIDENCE)}:{index}"
            before = event["before"]
            window = before["window"]
            pid = window["owner_pid"]
            demand(pid in owners and before["bundle_path"] == owners[pid]["app"], where)
            demand(window["title"] == title and window["layer"] == 0, where)
            if event["operation"] != "activate":
                demand(pid == before["front_pid"] and window["on_screen"], where)
            windows.add((pid, window["id"], title))
            bounds = window["bounds_pt"]
            widths.add(bounds["width"])
            if grab := event.get("screenshot"):
                path = folder / grab["file"]
                pixels = dimensions(path)
                demand(pixels == (grab["width_px"], grab["height_px"]), where)
                demand(pixels == (round(bounds["width"] * 2), round(bounds["height"] * 2)), where)
                grabs.add(grab["file"])
            event_count += 1
    return {"name": name, "launched": True, "launch_pids": sorted(owners), "window_owners": sorted(windows),
            "widths_pt": sorted(widths), "guarded_event_count": event_count, "guarded_png_count": len(grabs),
            "agents_file_fingerprints_match": True, **recovery}


trials = [audit_trial(name) for name in TRIALS]
rows = subprocess.check_output(["/bin/ps", "-axo", "pid=,ppid=,command="], text=True).splitlines()
prefix = str(EVIDENCE) + "/"
trial_processes = [row.strip() for row in rows if row.split(maxsplit=2)[-1].startswith(prefix)]
installed_path = "/Applications/码头.app/Contents/MacOS/码头"
installed_pids = [int(row.split(maxsplit=2)[0]) for row in rows if row.split(maxsplit=2)[-1] == installed_path]
demand(not trial_processes, f"Isolated trial processes still live: {trial_processes}")
print(json.dumps({"trials": trials, "trial_processes_after": trial_processes,
                  "preexisting_installed_app_pids_after": installed_pids}, ensure_ascii=False, indent=2))
