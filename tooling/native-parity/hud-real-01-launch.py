"""Stage and launch the isolated native and frozen HUD acceptance windows."""
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
BASE_EVIDENCE = ROOT / "artifacts/native-parity/native/evidence/hud-real-01"
TRIAL = os.environ.get("HUD_REAL_01_TRIAL", "")
if TRIAL not in ("", "before-fix", "after-fix", "esc-before", "esc-after"):
    raise SystemExit("Invalid HUD_REAL_01_TRIAL")
EVIDENCE = BASE_EVIDENCE / TRIAL
ARCHIVE = "Contents/Resources/app.asar"
PLIST = "Contents/Info.plist"
SESSION = "01a1ddc1-88fd-7fc1-8c66-ab8d38719da8"
LONG_WORKSPACE = "workspace-with-a-deliberately-long-name-for-real-window-hud-truncation-evidence"
MODEL = "gpt-6-hud-acceptance-model-with-a-deliberately-long-visible-name-and-distinct-suffix"
TITLE = "这是一段专门用于真实窗口验收的超长会话标题，右侧动作需要保持可见并且可以点击"
REPOSITORY_STUB = ROOT / "apps/native/runtime/tests/fixtures/codex-launch.sh"


def read(path):
    return json.loads(path.read_text())


def save(path, data):
    with path.open("x", encoding="utf-8") as output:
        output.write(json.dumps(data, ensure_ascii=False, indent=2) + "\n")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def prepare_fixture(run, paths):
    workspace, codex = paths["workspace"], paths["codex"]
    project = workspace / "codex-project"
    project.mkdir()
    subprocess.run(["git", "-c", "init.defaultBranch=hud-fixture", "-c", "init.templateDir=",
                    "init", "-q", workspace], check=True)
    initial_body = "# 隔离工作区说明\n\nHUD-REAL-01-READ-ONLY\n\n这是一份用于真实窗口验收的测试文件。\n"
    for folder, marker in ((workspace, "OUTER"), (project, "INNER")):
        body = initial_body if not TRIAL else initial_body.replace(
            "HUD-REAL-01-READ-ONLY", f"HUD-REAL-01-{marker}-READ-ONLY")
        (folder / "AGENTS.md").write_text(body, encoding="utf-8")
    (paths["home"] / ".zshrc").write_text(
        "PROMPT='hud-fixture%# '\nHISTFILE=/dev/null\n", encoding="utf-8")
    (codex / "config.toml").write_text(
        'approval_policy = "on-request"\nsandbox_mode = "workspace-write"\n', encoding="utf-8")
    sessions = codex / "sessions"
    sessions.mkdir()
    rollout = sessions / f"rollout-fixture-{SESSION}.jsonl"
    cwd = str(project)
    def row(record_type, payload):
        return {"type": record_type, "payload": payload}

    rows = [
        row("session_meta", {"id": SESSION, "cwd": cwd}),
        row("turn_context", {"model": MODEL, "effort": "high"}),
        row("event_msg", {"type": "token_count", "info": {
            "last_token_usage": {"total_tokens": 51001}, "model_context_window": 200000}}),
        row("event_msg", {"type": "item_completed", "item": {
            "id": "hud-command-1", "type": "CommandExecution", "status": "completed",
            "exit_code": 0, "command": ["printf", "local fixture"]}}),
    ]
    rollout.write_text("".join(json.dumps(item, ensure_ascii=False) + "\n" for item in rows),
                       encoding="utf-8")
    for filename, event in (("startup-hook", "SessionStart"), ("work", "UserPromptSubmit"),
                            ("done", "Stop"), ("tool", "PreToolUse")):
        fields = {"hook_event_name": event, "session_id": SESSION, "cwd": cwd,
                  "transcript_path": str(rollout), "model": MODEL, "turn_id": "hud-fixture-turn",
                  "prompt": TITLE, "tool_use_id": "hud-running-1", "tool_name": "Bash",
                  "tool_input": {"command": "printf fixture"}}
        save(codex / f"{filename}.json", fields)
    (codex / "session_index.jsonl").write_text(json.dumps({
        "id": SESSION, "thread_name": TITLE, "updated_at": "2026-11-21T00:00:00Z",
    }, ensure_ascii=False) + "\n", encoding="utf-8")
    stub = run / "codex-fixture"
    # The repository fixture handles launch and event delivery for both sides.
    # The frozen runner injects this directory into every PTY. The native
    # contract fixture asserts it is absent, so clear it only inside the clone.
    compatibility = "unset MATOU_AGENT_STATUS_DIR\n" if run.name == "reference" else ""
    stub.write_text(
        '#!/bin/sh\n'
        'case "${1:-}" in --version|-v) echo "codex-cli 0.162.1 (fixture)"; exit 0;; esac\n'
        f'{compatibility}export CODEX_FIXTURE_EVENTS=1\n'
        f'exec /bin/sh {shlex.quote(str(REPOSITORY_STUB))} "$@"\n',
        encoding="utf-8",
    )
    stub.chmod(0o700)
    return {"project": cwd, "model": MODEL, "effort": "high", "context_percent": 26,
            "rollout": str(rollout), "completed_tools": 1, "title": TITLE,
            "repository_stub": str(REPOSITORY_STUB), "stub_sha256": digest(REPOSITORY_STUB),
            "agents_sha256": digest(project / "AGENTS.md"),
            "workspace_agents_sha256": digest(workspace / "AGENTS.md")}


def prepare(side):
    run = EVIDENCE / side
    run.mkdir(mode=0o700, parents=True)
    paths = {key: run / key for key in ("home", "cache", "tmp", "data", "user-data", "codex")}
    paths["workspace"] = run / LONG_WORKSPACE
    for path in paths.values():
        path.mkdir(mode=0o700)
    fixture = prepare_fixture(run, paths)
    if side == "native":
        app = run / "MatouNative.app"
        macos = app / "Contents/MacOS"
        macos.mkdir(parents=True)
        target = BASE_EVIDENCE / ".build/debug"
        if TRIAL in ("after-fix", "esc-before", "esc-after"):
            target = ROOT / "apps/native/target/debug"
        if TRIAL == "before-fix":
            old = read(BASE_EVIDENCE / "native/prepared.json")
            app_path = BASE_EVIDENCE / "native/MatouNative.app"
            target = app_path / "Contents/MacOS"
            executable = target / "MatouNative"
            if old["app"] != str(app_path) or digest(executable) != old["sha256"]:
                raise RuntimeError("Historical acceptance bundle identity check failed")
        inputs = {}
        for name, destination in (("matou-native-desktop", "MatouNative"),
                                  ("matou-native-runtime", "matou-native-runtime")):
            source = target / (destination if TRIAL == "before-fix" else name)
            subprocess.run(["/bin/cp", "-c", source, macos / destination], check=True)
            inputs[name] = {"path": str(source), "sha256": digest(source),
                            "mtime_ns": source.stat().st_mtime_ns}
        info = {"CFBundleName": "Matou Native", "CFBundleDisplayName": "Matou Native",
                "CFBundleExecutable": "MatouNative", "CFBundlePackageType": "APPL",
                "CFBundleVersion": "0.1.0", "NSHighResolutionCapable": True,
                "NSPrincipalClass": "NSApplication"}
        title = "Matou Native"
    else:
        app = run / "MatouHudReference.app"
        inputs = read(EVIDENCE.parent / "ime-real-01/current/isolation.json")
        for key, expected in (("source_bundle", "baseline_asar_sha256"),
                              ("home_isolated_bundle", "harness_asar_sha256")):
            if digest(Path(inputs[key]) / ARCHIVE) != inputs[expected]:
                raise RuntimeError(f"Frozen archive changed: {key}")
        subprocess.run(["/bin/cp", "-cR", inputs["home_isolated_bundle"], app], check=True)
        info = plistlib.loads((app / PLIST).read_bytes())
        title = "码头"
    bundle = f"com.matou.hud-real-01.{TRIAL + '.' if TRIAL else ''}{side}"
    info["CFBundleIdentifier"] = bundle
    (app / PLIST).write_bytes(plistlib.dumps(info))
    subprocess.run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", app], check=True)
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", app], check=True)
    executable = app / "Contents/MacOS" / info["CFBundleExecutable"]
    if side == "reference" and digest(app / ARCHIVE) != inputs["harness_asar_sha256"]:
        raise RuntimeError("Disposable archive differs from verified HOME harness")
    save(run / "prepared.json", {
        "app": str(app), "bundle": bundle, "side": side, "title": title,
        "executable": str(executable), "sha256": digest(executable), "inputs": inputs,
        "checkout": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "paths": {key: str(value) for key, value in paths.items()}, "fixture": fixture,
    })


def launch(side):
    run = EVIDENCE / side
    prepared = read(run / "prepared.json")
    if (run / "launch.json").exists():
        raise RuntimeError("A launch record already exists on this side")
    other = EVIDENCE / ("reference" if side == "native" else "native") / "launch.json"
    if other.exists():
        previous = read(other)
        comm = subprocess.run(["/bin/ps", "-p", str(previous["pid"]), "-o", "comm="],
                              capture_output=True, text=True).stdout.strip()
        if comm == previous["executable"]:
            raise RuntimeError("Quit the other trial before launching")
    paths = prepared["paths"]
    data = Path(paths["data"])
    metadata = data.stat()
    alias = Path(f"/.vol/{metadata.st_dev}/{metadata.st_ino}")
    if not alias.samefile(data):
        raise RuntimeError("Isolated short socket alias failed")
    env = {name: os.environ[name] for name in ("USER", "LOGNAME", "LANG", "LC_ALL") if name in os.environ}
    env.update(PATH="/usr/bin:/bin:/usr/sbin:/sbin", SHELL="/bin/zsh", HOME=paths["home"],
               ZDOTDIR=paths["home"], TMPDIR=paths["tmp"] + "/", XDG_CACHE_HOME=paths["cache"],
               XDG_CONFIG_HOME=paths["home"] + "/.config", CODEX_HOME=paths["codex"],
               MATOU_CODEX_COMMAND=str(run / "codex-fixture"), MATOU_DISABLE_LOGIN_PATH_PROBE="1")
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


def stopped(record):
    executable = subprocess.run(["/bin/ps", "-p", str(record["pid"]), "-o", "comm="],
                                capture_output=True, text=True).stdout.strip()
    return executable != record["executable"]


def relaunch(side):
    run = EVIDENCE / side
    current, log = run / "launch.json", run / "app.log"
    previous, previous_log = run / "launch-first.json", run / "app-first.log"
    if previous.exists() or previous_log.exists():
        raise RuntimeError("Preserving previous acceptance evidence")
    if not stopped(read(current)):
        raise RuntimeError("Quit the current isolated window first")
    other = EVIDENCE / ("reference" if side == "native" else "native") / "launch.json"
    if other.exists() and not stopped(read(other)):
        raise RuntimeError("Quit the other isolated window first")
    current.rename(previous)
    log.rename(previous_log)
    launch(side)


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in ("prepare", "launch", "relaunch") or sys.argv[2] not in ("native", "reference"):
        raise SystemExit("Usage: hud-real-01-launch.py prepare|launch|relaunch native|reference")
    if TRIAL and sys.argv[2] != "native":
        raise SystemExit("Fix trials can only operate on a new native bundle")
    {"prepare": prepare, "launch": launch, "relaunch": relaunch}[sys.argv[1]](sys.argv[2])
