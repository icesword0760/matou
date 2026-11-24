"""Stage and start only disposable builds for the narrow-window comparison."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / "artifacts/native-parity/native/evidence/narrow-window-real-01"
ARCHIVE = "Contents/Resources/app.asar"
PLIST = "Contents/Info.plist"


def read(path):
    return json.loads(path.read_text())


def save(path, record):
    with path.open("x") as output:
        output.write(json.dumps(record, ensure_ascii=False, indent=2) + "\n")


def sha(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def prepare_workspace(paths):
    workspace = paths["workspace"]
    subprocess.run(["git", "-c", "init.defaultBranch=narrow-fixture", "-c", "init.templateDir=",
                    "init", "-q", workspace], check=True)
    cwd = str(workspace.resolve())
    bucket = re.sub("[^A-Za-z0-9]", "-", cwd)
    sid = "0ba60000-0000-4000-8000-000000000001"
    title = "Claude 占位会话，这是一段有意加长用于检查卡头截断行为的验收标题"
    rows = [
        {"type": "user", "sessionId": sid, "cwd": cwd, "timestamp": "2026-11-21T00:01:00Z",
         "message": {"role": "user", "content": title}, "permissionMode": "default"},
        {"type": "assistant", "sessionId": sid, "cwd": cwd, "timestamp": "2026-11-21T00:02:00Z",
         "message": {"role": "assistant", "content": [
             {"type": "text", "text": "本地合成占位内容，用来检查窗口缩放、换行、截断和焦点。"}]}},
    ]
    for config in (paths["home"] / ".claude", paths["claude"]):
        folder = config / "projects" / bucket
        folder.mkdir(parents=True)
        with (folder / f"{sid}.jsonl").open("x", encoding="utf-8") as transcript:
            transcript.writelines(json.dumps(row, ensure_ascii=False) + "\n" for row in rows)


def prepare(side):
    run = EVIDENCE / side
    run.mkdir(mode=0o700)
    paths = {key: run / key for key in
             ("home", "workspace", "cache", "tmp", "data", "user-data", "claude")}
    for path in paths.values():
        path.mkdir(mode=0o700)
    prepare_workspace(paths)
    pty = Path(__file__).with_name("scrollbar-real-02-pty.py")
    echo = shlex.join((str(Path(sys.executable).resolve()), str(pty), str(run)))
    (paths["home"] / ".zshrc").write_text(f"if [[ -t 0 && -t 1 ]]; then\n  exec {echo}\nfi\n")
    stub = run / "claude-fixture"
    stub.write_text(
        "#!/bin/sh\n"
        'case "$1" in --version|-v) echo "2.1.0 (Claude Code)"; exit 0;; esac\n'
        "printf '\\r\\nClaude fixture (local, no account)\\r\\n'\n"
        f"exec {echo}\n"
    )
    stub.chmod(0o700)
    if side == "native":
        app = run / "MatouNative.app"
        macos = app / "Contents/MacOS"
        macos.mkdir(parents=True)
        target = ROOT / "apps/native/target/debug"
        inputs = {}
        for source, destination in (("matou-native-desktop", "MatouNative"),
                                    ("matou-native-runtime", "matou-native-runtime")):
            original = target / source
            subprocess.run(["/bin/cp", "-c", original, macos / destination], check=True)
            inputs[source] = {"path": str(original), "sha256": sha(original),
                              "mtime_ns": original.stat().st_mtime_ns}
        info = {"CFBundleName": "Matou Native", "CFBundleDisplayName": "Matou Native",
                "CFBundleExecutable": "MatouNative", "CFBundlePackageType": "APPL",
                "CFBundleVersion": "0.1.0", "NSHighResolutionCapable": True,
                "NSPrincipalClass": "NSApplication"}
        title = "Matou Native"
    else:
        app = run / "MatouNarrowReference.app"
        audit = read(EVIDENCE.parent / "ime-real-01/current/isolation.json")
        for key, digest in (("source_bundle", "baseline_asar_sha256"),
                            ("home_isolated_bundle", "harness_asar_sha256")):
            if sha(Path(audit[key]) / ARCHIVE) != audit[digest]:
                raise RuntimeError(f"Frozen payload changed: {key}")
        subprocess.run(["/bin/cp", "-cR", audit["home_isolated_bundle"], app], check=True)
        info = plistlib.loads((app / PLIST).read_bytes())
        title, inputs = "码头", audit
    bundle = f"com.matou.narrow-window-real-01.{side}"
    info["CFBundleIdentifier"] = bundle
    (app / PLIST).write_bytes(plistlib.dumps(info))
    subprocess.run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", app], check=True)
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", app], check=True)
    executable = app / "Contents/MacOS" / info["CFBundleExecutable"]
    if side == "reference" and sha(app / ARCHIVE) != inputs["harness_asar_sha256"]:
        raise RuntimeError("Disposable bundle archive differs from verified HOME harness")
    save(run / "prepared.json", {
        "app": str(app), "bundle": bundle, "side": side, "title": title,
        "executable": str(executable), "sha256": sha(executable), "inputs": inputs,
        "checkout_head_at_preparation": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "paths": {key: str(value) for key, value in paths.items()},
    })


def matches_live_process(record):
    current = subprocess.run(["/bin/ps", "-p", str(record["pid"]), "-o", "comm="],
                             capture_output=True, text=True).stdout.strip()
    return current == record["executable"]


def relaunch(side):
    run = EVIDENCE / side
    first = read(run / "launch.json")
    if matches_live_process(first):
        raise RuntimeError("Quit the current isolated process before relaunching")
    suffix = "first" if not (run / "launch-first.json").exists() else str(len(list(run.glob("launch*.json"))))
    previous, app_log = run / f"launch-{suffix}.json", run / f"app-{suffix}.log"
    if previous.exists() or app_log.exists():
        raise RuntimeError("Preserving previous launch evidence")
    (run / "launch.json").rename(previous)
    (run / "app.log").rename(app_log)
    launch(side)


def launch(side):
    run = EVIDENCE / side
    prepared = read(run / "prepared.json")
    if (run / "launch.json").exists():
        raise RuntimeError("Preserving an existing launch record")
    other = EVIDENCE / ("reference" if side == "native" else "native") / "launch.json"
    if other.exists():
        if matches_live_process(read(other)):
            raise RuntimeError("Quit the other trial before launching this trial")
    paths = prepared["paths"]
    data = Path(paths["data"])
    identity = data.stat()
    alias = Path(f"/.vol/{identity.st_dev}/{identity.st_ino}")
    if not alias.samefile(data):
        raise RuntimeError("Isolated short socket alias did not resolve")
    env = {name: os.environ[name] for name in ("USER", "LOGNAME", "LANG", "LC_ALL") if name in os.environ}
    env.update(PATH="/usr/bin:/bin:/usr/sbin:/sbin", SHELL="/bin/zsh", HOME=paths["home"],
               ZDOTDIR=paths["home"], TMPDIR=paths["tmp"] + "/", XDG_CACHE_HOME=paths["cache"],
               XDG_CONFIG_HOME=paths["home"] + "/.config", CLAUDE_CONFIG_DIR=paths["claude"],
               MATOU_CLAUDE_COMMAND=str(run / "claude-fixture"), MATOU_DISABLE_LOGIN_PATH_PROBE="1")
    command = [prepared["executable"]]
    if side == "native":
        command += ["--data-root", str(alias), "--workspace", paths["workspace"], "--shell-home", paths["home"]]
    else:
        env.update(MATOU_REFERENCE_HOME=paths["home"], ELECTRON_USER_DATA_DIR=paths["user-data"],
                   MATOU_DATA_DIR=str(alias), MATOU_DEFAULT_WORKSPACE=paths["workspace"],
                   MATOU_DISABLE_AUTO_UPDATE="1", MATOU_UPDATE_BASE_URL="http://127.0.0.1:9/native-parity-disabled",
                   MATOU_LOCALE="zh-CN", MATOU_CLAUDE_PROJECTS_ROOT=paths["home"] + "/.claude/projects")
        command += [f"--user-data-dir={paths['user-data']}"]
    with (run / "app.log").open("xb") as log:
        process = subprocess.Popen(command, env=env, cwd=paths["workspace"], stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
    record = prepared | {"pid": process.pid, "command": command,
                         "runtime_data_path": str(alias), "started_unix_ns": time.time_ns()}
    save(run / "launch.json", record)
    print(json.dumps(record, ensure_ascii=False))


if __name__ == "__main__":
    action, side = sys.argv[1:]
    if side not in ("native", "reference"):
        raise SystemExit("Allowed sides: native, reference")
    {"prepare": prepare, "launch": launch, "relaunch": relaunch}[action](side)
