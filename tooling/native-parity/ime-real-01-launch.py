"""Prepare or start only the isolated windows for the real IME acceptance run."""

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
EVIDENCE = ROOT / "artifacts/native-parity/native/evidence/ime-real-01/current"
NATIVE_BUNDLE_ROOT = Path("/tmp/MatouNativeIme.app")
NATIVE_ROOT = Path("/tmp/ime-real")
FROZEN_ROOT = ROOT / "artifacts/native-parity"
FROZEN_BASELINE = FROZEN_ROOT / "baseline/MatouReference-924de28d.app"
FROZEN_HARNESS = FROZEN_ROOT / "reference/MatouReference-924de28d/MatouParityReference.app"
REFERENCE_ROOT = EVIDENCE / ".reference-isolation"
REFERENCE_APP = REFERENCE_ROOT / "MatouImeReference.app"
ASAR_MODULE = ROOT / "node_modules/.pnpm/@electron+asar@3.4.1/node_modules/@electron/asar/lib/asar.js"
ARCHIVE = Path("Contents/Resources/app.asar")
PLIST = Path("Contents/Info.plist")


def write_json(path, obj):
    with path.open("x", encoding="utf-8") as output:
        json.dump(obj, output, indent=2, ensure_ascii=False)
        output.write("\n")


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def shell_fixture(home, name):
    home.mkdir(parents=True, exist_ok=True)
    echo = ROOT / "tooling/native-parity/ime-real-01-echo.py"
    trace = EVIDENCE / f"{name}-pty.jsonl"
    command = shlex.join([sys.executable, str(echo), str(trace)])
    with (home / ".zshrc").open("x", encoding="utf-8") as output:
        output.write(f"if [[ -t 0 && -t 1 ]]; then\n  exec {command}\nfi\n")


def minimal_environment():
    env = {name: os.environ[name] for name in ("USER", "LOGNAME", "LANG", "LC_ALL") if name in os.environ}
    env.update(PATH="/usr/bin:/bin:/usr/sbin:/sbin", SHELL="/bin/zsh")
    return env


def verify_reference():
    verify_files = """
const assert = require('node:assert/strict');
const asar = require(process.argv[1]);
const [baseline, harness] = process.argv.slice(2);
const files = asar.listPackage(baseline).sort();
assert.deepEqual(files, asar.listPackage(harness).sort());
const main = 'out/main/index.js';
const added = 'app.setPath("home", process.env.MATOU_REFERENCE_HOME);\\n';
const anchor = 'applyApplicationBrand(app, APP_DISPLAY_NAME);';
let count = 0;
for (const entry of files) {
  const path = entry.slice(1);
  const original = asar.statFile(baseline, path, false);
  const copied = asar.statFile(harness, path, false);
  if (original.files || original.link) {
    assert.equal(Boolean(copied.files), Boolean(original.files), path);
    assert.equal(copied.link, original.link, path);
    continue;
  }
  const before = asar.extractFile(baseline, path);
  const after = asar.extractFile(harness, path);
  if (path === main) {
    assert.equal(before.toString().split(anchor).length, 2);
    assert.equal(after.toString(), before.toString().replace(anchor, added + anchor));
  } else {
    assert.ok(before.equals(after), path);
  }
  count++;
}
process.stdout.write(String(count));
"""
    count = subprocess.check_output(
        ["node", "-e", verify_files, str(ASAR_MODULE), str(FROZEN_BASELINE / ARCHIVE),
         str(FROZEN_HARNESS / ARCHIVE)], text=True,
    )
    return {"source_bundle": str(FROZEN_BASELINE), "home_isolated_bundle": str(FROZEN_HARNESS),
            "compared_asar_files": int(count), "baseline_asar_sha256": sha(FROZEN_BASELINE / ARCHIVE),
            "harness_asar_sha256": sha(FROZEN_HARNESS / ARCHIVE),
            "sole_asar_change": 'app.setPath("home", process.env.MATOU_REFERENCE_HOME);'}


def prepare():
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    if any(p.exists() or p.is_symlink() for p in (NATIVE_ROOT, NATIVE_BUNDLE_ROOT, REFERENCE_ROOT)):
        raise SystemExit("Isolation path already exists. Preserve its contents and investigate before retrying.")
    NATIVE_ROOT.mkdir()
    NATIVE_BUNDLE_ROOT.mkdir()
    write_json(NATIVE_BUNDLE_ROOT / "environment.json", {"root": str(NATIVE_ROOT)})
    shell_fixture(NATIVE_ROOT / "home", "native")

    REFERENCE_ROOT.mkdir()
    shell_fixture(REFERENCE_ROOT / "home", "reference")
    subprocess.run(["cp", "-cR", str(FROZEN_HARNESS), str(REFERENCE_APP)], check=True)
    plist_path = REFERENCE_APP / PLIST
    info = plistlib.loads(plist_path.read_bytes())
    info["CFBundleIdentifier"] = "com.matou.ime-real-01.reference"
    info["CFBundleDisplayName"] = "Matou IME Reference"
    plist_path.write_bytes(plistlib.dumps(info))
    seal_reference()


def seal_reference():
    evidence = EVIDENCE / "isolation.json"
    if evidence.exists() or (EVIDENCE / "reference-launch.json").exists():
        raise SystemExit("Reference copy already verified or launched")
    info = plistlib.loads((REFERENCE_APP / PLIST).read_bytes())
    if info["CFBundleIdentifier"] != "com.matou.ime-real-01.reference":
        raise SystemExit("Refusing to sign a bundle without the acceptance identity")
    verify = verify_reference()
    subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(REFERENCE_APP)], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(REFERENCE_APP)], check=True)
    verify["run_asar_sha256"] = sha(REFERENCE_APP / ARCHIVE)
    if verify["run_asar_sha256"] != verify["harness_asar_sha256"]:
        raise SystemExit("Reference archive copy verification failed")
    write_json(evidence, verify)


def launch_native():
    run = subprocess.check_output(
        [sys.executable, str(ROOT / "tooling/native-parity/launch-native.py"), "--binary",
         str(ROOT / "apps/native/target/debug/matou-native-desktop"), "--destination",
         str(NATIVE_BUNDLE_ROOT)], env=minimal_environment(), text=True,
    )
    record = json.loads(run)
    record["bundleIdentifier"] = plistlib.loads((Path(record["app"]) / PLIST).read_bytes())["CFBundleIdentifier"]
    record["expected_window_title"] = "Matou Native"
    write_json(EVIDENCE / "native-launch.json", record)
    print(json.dumps(record, ensure_ascii=False))


def launch_reference(record_directory=EVIDENCE):
    if not (EVIDENCE / "isolation.json").is_file():
        raise SystemExit("Verify the isolated frozen copy before starting it")
    launch_record = record_directory / "reference-launch.json"
    if launch_record.exists():
        raise SystemExit("Reference window already launched during this run")
    info = plistlib.loads((REFERENCE_APP / PLIST).read_bytes())
    directories = {name: REFERENCE_ROOT / name for name in ("home", "workspace", "data", "user-data", "tmp")}
    for path in directories.values():
        path.mkdir(exist_ok=True)
    # The frozen runtime derives its Unix socket path from MATOU_DATA_DIR.
    # Address the same evidence inode under a shorter macOS path when needed.
    data = directories["data"]
    if len(os.fsencode(data / "control/runtime.sock")) >= 104:
        identity = data.stat()
        short_data = Path(f"/.vol/{identity.st_dev}/{identity.st_ino}")
        if not short_data.samefile(data):
            raise SystemExit("Could not establish a short socket path inside the isolated evidence directory")
        data = short_data
    home = str(directories["home"])
    env = minimal_environment() | {
        "HOME": home, "ZDOTDIR": home, "MATOU_REFERENCE_HOME": home,
        "TMPDIR": str(directories["tmp"]) + "/", "ELECTRON_USER_DATA_DIR": str(directories["user-data"]),
        "MATOU_DATA_DIR": str(data), "MATOU_DEFAULT_WORKSPACE": str(directories["workspace"]),
        "MATOU_DISABLE_AUTO_UPDATE": "1", "MATOU_UPDATE_BASE_URL": "http://127.0.0.1:9/native-parity-disabled",
        "MATOU_LOCALE": "zh-CN",
    }
    executable = REFERENCE_APP / "Contents/MacOS" / info["CFBundleExecutable"]
    command = [str(executable), f"--user-data-dir={directories['user-data']}"]
    with (record_directory / "reference-app.log").open("xb") as log:
        started = time.time_ns()
        process = subprocess.Popen(command, env=env, cwd=directories["workspace"], stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
    record = {"pid": process.pid, "bundleIdentifier": info["CFBundleIdentifier"], "app": str(REFERENCE_APP),
              "paths": {name: str(path) for name, path in directories.items()}, "started_unix_ns": started,
              "command": command, "runtime_data_path": str(data),
              "pty_path": str(EVIDENCE / "reference-pty.jsonl")}
    write_json(launch_record, record)
    print(json.dumps(record, ensure_ascii=False))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("step", choices=("prepare", "seal-reference", "native", "reference"))
    parser.add_argument("--reference-record-subdir", choices=("continuous", "unlocked", "cleared", "completion"))
    args = parser.parse_args()
    if args.reference_record_subdir:
        if args.step != "reference":
            parser.error("--reference-record-subdir is only valid for reference")
        launch_reference(EVIDENCE.parent / args.reference_record_subdir)
    else:
        {"prepare": prepare, "seal-reference": seal_reference,
         "native": launch_native, "reference": launch_reference}[args.step]()
