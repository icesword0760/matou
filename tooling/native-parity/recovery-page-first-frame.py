#!/usr/bin/env python3
"""Prepare or capture a native first-frame trial with isolated fixture data."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import subprocess
import sys
import time
import uuid


HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
GATE = "recovery-page-fix-01"
PREVIOUS = "recovery-actions-real-01"
EVIDENCE = PROJECT / "artifacts/native-parity/native/evidence" / GATE

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("recovery_trial", HERE / f"{PREVIOUS}.py")
trial = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trial)


def prepare(case, binaries):
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    run = EVIDENCE / f"native-{case}-{uuid.uuid4().hex[:12]}"
    run.mkdir(mode=0o700)
    paths = trial.mkdirs(run)
    (paths["home"] / ".zshrc").write_text("PROMPT='FIRST-FRAME> '\n")
    if case != "normal":
        fixture = "damaged.sqlite" if case == "recovery" else "base.sqlite"
        shutil.copyfile(trial.SUPPORT / fixture, paths["data"] / trial.DB)
        (paths["data"] / trial.DB).chmod(0o600)
    if case == "marker":
        (paths["data"] / trial.MARKER).write_bytes(b"{")

    app = run / "MatouNative.app"
    macos = app / "Contents/MacOS"
    macos.mkdir(parents=True)
    (app / "Contents/Resources").mkdir()
    executable = macos / "MatouNative"
    runtime = macos / "matou-native-runtime"
    sources = {}
    for name, destination in [
        ("matou-native-desktop", executable),
        ("matou-native-runtime", runtime),
    ]:
        source = binaries / name
        subprocess.run(["/bin/cp", "-c", source, destination], check=True)
        sources[name] = {"path": str(source), "sha256": trial.sha(source)}
    bundle_id = f"com.matou.{GATE}.{run.name}"
    info = {
        "CFBundleIdentifier": bundle_id, "CFBundleName": "Matou Native",
        "CFBundleExecutable": executable.name, "CFBundlePackageType": "APPL",
        "CFBundleVersion": "0.1.0", "NSHighResolutionCapable": True,
        "NSPrincipalClass": "NSApplication",
    }
    (app / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
    subprocess.run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", app], check=True)
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", app], check=True)
    record = {
        "app": str(app), "bundle_id": bundle_id, "title": "Matou Native",
        "side": "native", "condition": case, "sources": sources,
        "paths": {key: str(path) for key, path in paths.items()},
        "executable": str(executable), "executable_sha256": trial.sha(executable),
        "runtime_executable": str(runtime), "runtime_sha256": trial.sha(runtime),
        "git_head": subprocess.check_output(
            ["git", "-C", PROJECT, "rev-parse", "HEAD"], text=True).strip(),
    }
    trial.save(run / "prepared.json", record)
    trial.snapshot(run, "before-launch")
    return run


def build_recorder(run):
    # Use the prior sampler, changing only its run-directory and bundle guards.
    source = (HERE / f"{PREVIOUS}-window.swift").read_text()
    if source.count(PREVIOUS) != 3:
        raise RuntimeError("Recorder guards changed; review the sampler before reusing it")
    copy = run / "recorder.swift"
    copy.write_text(source.replace(PREVIOUS, GATE))
    cache = run / "cache/swift"
    cache.mkdir()
    recorder = run / "recorder"
    subprocess.run(["/usr/bin/swiftc", "-module-cache-path", cache, copy, "-o", recorder],
                   check=True)
    return recorder


def record_startup(run, recorder):
    record = trial.read(run / "prepared.json")
    paths = {key: Path(path) for key, path in record["paths"].items()}
    command = [record["executable"], "--data-root", str(paths["data"]),
               "--workspace", str(paths["workspace"]), "--shell-home", str(paths["home"])]
    frames = run / "startup.jsonl"
    with frames.open("xb") as output, (run / "app.log").open("xb") as app_log:
        sampler = subprocess.Popen([recorder, run, "startup"], stdout=output)
        app = None
        try:
            deadline = time.monotonic() + 8
            while not (run / "startup.armed").exists():
                if sampler.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError(f"Recorder did not arm. See {frames}")
                time.sleep(0.01)
            uptime, unix_time = time.monotonic_ns(), time.time_ns()
            app = subprocess.Popen(command, env=trial.isolated_env(paths),
                                   cwd=paths["workspace"], start_new_session=True,
                                   stdout=app_log, stderr=subprocess.STDOUT)
            record.update(pid=app.pid, started_uptime_ns=uptime, started_unix_ns=unix_time,
                          command=command, data_path_used=str(paths["data"]))
            trial.save(run / "launch.next", record)
            (run / "launch.next").rename(run / "launch.json")
            if sampler.wait(timeout=12) != 0:
                raise RuntimeError(f"Recorder failed. See {frames}")
        finally:
            if sampler.poll() is None:
                sampler.terminate()
                sampler.wait(timeout=5)
            if app is not None and app.poll() is None:
                samples = [json.loads(line) for line in frames.read_text().splitlines()]
                window = next((row for row in samples if row["event"] == "first_window"), None)
                try:
                    if window:
                        with (run / "termination.jsonl").open("xb") as cleanup:
                            subprocess.run(
                                [recorder, run, "terminate", str(window["window_id"])],
                                stdout=cleanup, timeout=8, check=False,
                            )
                finally:
                    if app.poll() is None:
                        # This session was created above solely for the disposable fixture.
                        os.killpg(app.pid, signal.SIGTERM)
                    app.wait(timeout=5)
            trial.snapshot(run, "after-startup")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("case", choices=("recovery", "normal", "marker"))
    parser.add_argument("--capture", action="store_true", help="Launch and sample a visible test window")
    parser.add_argument("--binary-dir", type=Path, default=PROJECT / "apps/native/target/debug")
    args = parser.parse_args()
    binaries = args.binary_dir.resolve(strict=True)
    if not binaries.is_relative_to(PROJECT):
        parser.error("--binary-dir must be a build directory in this workspace")
    run = prepare(args.case, binaries)
    print(run, flush=True)
    recorder = build_recorder(run)
    if args.capture:
        record_startup(run, recorder)


if __name__ == "__main__":
    main()
