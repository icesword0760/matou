"""Prepare and start the two throwaway bundles used by the real-window probe."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shlex
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parents[2]
PARITY = ROOT / "artifacts/native-parity"
EVIDENCE = PARITY / "native/evidence/scrollbar-real-01/02"
IME = PARITY / "native/evidence/ime-real-01/current"
NATIVE_RECORD = IME.parent / "completion/native-recheck/native-launch.json"
ARCHIVE = Path("Contents/Resources/app.asar")
PLIST = Path("Contents/Info.plist")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, obj):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(obj, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def paths_for(run):
    return {key: run / key for key in ("home", "workspace", "data", "cache", "tmp", "user-data")}


def prepare(side, evidence):
    run = evidence / side
    run.mkdir()
    paths = paths_for(run)
    for directory in paths.values():
        directory.mkdir()
    python = str(Path(sys.executable).resolve())
    fixture = Path(__file__).with_name("scrollbar-real-02-pty.py")
    command = shlex.join((python, str(fixture), str(run)))
    (paths["home"] / ".zshrc").write_text(f"if [[ -t 0 && -t 1 ]]; then\n  exec {command}\nfi\n")

    if side == "native":
        source = Path(json.loads(NATIVE_RECORD.read_text())["app"])
        payload = ["Contents/MacOS/MatouNative", "Contents/MacOS/matou-native-runtime"]
        source_verification = {"prior_verified_launch": str(NATIVE_RECORD)}
    else:
        audit = json.loads((IME / "isolation.json").read_text())
        source = Path(audit["home_isolated_bundle"])
        baseline = Path(audit["source_bundle"])
        if digest(source / ARCHIVE) != audit["harness_asar_sha256"]:
            raise SystemExit("Frozen HOME-isolation harness hash changed")
        if digest(baseline / ARCHIVE) != audit["baseline_asar_sha256"]:
            raise SystemExit("Frozen baseline archive hash changed")
        payload = [str(ARCHIVE)]
        source_verification = audit
    app = run / ("MatouNative.app" if side == "native" else "MatouScrollReference.app")
    subprocess.run(["cp", "-cR", str(source), str(app)], check=True)
    info = plistlib.loads((app / PLIST).read_bytes())
    info["CFBundleIdentifier"] = f"com.matou.scrollbar-real-02.{side}"
    (app / PLIST).write_bytes(plistlib.dumps(info))
    subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    hashes = {item: digest(app / item) for item in payload}
    if any(hashes[item] != digest(source / item) for item in payload):
        if side == "reference":
            raise SystemExit("Frozen payload differs after isolation and re-signing")
        # Mach-O code signing may change the signature bytes. Check immutable UUIDs.
        for item in payload:
            uuid = lambda p: subprocess.check_output(["dwarfdump", "--uuid", str(p)], text=True).split()[1]
            if uuid(app / item) != uuid(source / item):
                raise SystemExit(f"Native Mach-O UUID mismatch for {item}")
    write_json(run / "prepared.json", {
        "app": str(app), "source": str(source), "bundle_id": info["CFBundleIdentifier"],
        "payload_sha256": hashes, "source_payload_sha256": {item: digest(source / item) for item in payload},
        "source_verification": source_verification,
    })


def start(side, evidence):
    run = evidence / side
    record_path = run / "launch.json"
    if record_path.exists():
        raise SystemExit(f"Preserve the existing {side} launch evidence")
    prepared = json.loads((run / "prepared.json").read_text())
    app = Path(prepared["app"])
    info = plistlib.loads((app / PLIST).read_bytes())
    paths = paths_for(run)
    identity = paths["data"].stat()
    short_data = Path(f"/.vol/{identity.st_dev}/{identity.st_ino}")
    if not short_data.samefile(paths["data"]):
        raise SystemExit("Short socket alias is not the isolated data directory")
    executable = app / "Contents/MacOS" / info["CFBundleExecutable"]
    env = {name: os.environ[name] for name in ("USER", "LOGNAME", "LANG", "LC_ALL") if name in os.environ}
    env.update(PATH="/usr/bin:/bin:/usr/sbin:/sbin", SHELL="/bin/zsh", HOME=str(paths["home"]),
               ZDOTDIR=str(paths["home"]), TMPDIR=str(paths["tmp"]) + "/", XDG_CACHE_HOME=str(paths["cache"]))
    if side == "native":
        command = [str(executable), "--data-root", str(short_data), "--workspace", str(paths["workspace"]),
                   "--shell-home", str(paths["home"])]
    else:
        env.update(MATOU_REFERENCE_HOME=str(paths["home"]), ELECTRON_USER_DATA_DIR=str(paths["user-data"]),
                   MATOU_DATA_DIR=str(short_data), MATOU_DEFAULT_WORKSPACE=str(paths["workspace"]),
                   MATOU_DISABLE_AUTO_UPDATE="1", MATOU_UPDATE_BASE_URL="http://127.0.0.1:9/native-parity-disabled",
                   MATOU_LOCALE="zh-CN")
        command = [str(executable), f"--user-data-dir={paths['user-data']}"]
    with (run / "app.log").open("xb") as log:
        timestamp = time.time_ns()
        process = subprocess.Popen(command, env=env, cwd=paths["workspace"], stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
    record = prepared | {"pid": process.pid, "command": command, "runtime_data_path": str(short_data),
                         "paths": {name: str(path) for name, path in paths.items()}, "started_unix_ns": timestamp,
                         "title": "Matou Native" if side == "native" else "码头"}
    write_json(record_path, record)
    print(json.dumps(record, ensure_ascii=False))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "start"))
    parser.add_argument("side", choices=("native", "reference"))
    parser.add_argument("--run", choices=("ime-followup",))
    args = parser.parse_args()
    evidence = EVIDENCE / args.run if args.run else EVIDENCE
    evidence.mkdir(parents=True, exist_ok=True)
    {"prepare": prepare, "start": start}[args.action](args.side, evidence)
