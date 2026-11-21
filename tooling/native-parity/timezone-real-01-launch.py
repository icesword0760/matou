"""Stage and start disposable native and frozen language/time-zone trials."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import sqlite3
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / "artifacts/native-parity/native/evidence/timezone-real-01"
ARCHIVE = "Contents/Resources/app.asar"
PLIST = "Contents/Info.plist"


def build_tools():
    folder = Path(__file__).resolve().parent
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    cache = EVIDENCE / "swift-cache"
    cache.mkdir(exist_ok=True)
    existing = (folder / "narrow-window-real-01-window.swift").read_text()
    scoped = EVIDENCE / "window-source.swift"
    scoped.write_text(existing.replace("narrow-window-real-01", "timezone-real-01"))
    for source, output in ((scoped, "window"), (folder / "timezone-real-01-startup.swift", "startup-window")):
        subprocess.run(["/usr/bin/swiftc", "-module-cache-path", cache, source,
                        "-o", EVIDENCE / output], check=True)


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
    subprocess.run(["/usr/bin/git", "-c", "init.defaultBranch=timezone-fixture",
                    "-c", "init.templateDir=", "init", "-q", workspace],
                   env={"HOME": str(paths["home"]), "PATH": "/usr/bin:/bin", "GIT_CONFIG_NOSYSTEM": "1"},
                   check=True)
    cwd = str(workspace.resolve())
    bucket = re.sub("[^A-Za-z0-9]", "-", cwd)
    sid = "0ba60000-0000-4000-8000-000000000001"
    title = "Timezone fixture transcript"
    rows = [
        {"type": "user", "sessionId": sid, "cwd": cwd, "timestamp": "2026-11-21T00:01:00Z",
         "message": {"role": "user", "content": title}, "permissionMode": "default"},
        {"type": "assistant", "sessionId": sid, "cwd": cwd, "timestamp": "2026-11-21T00:02:00Z",
         "message": {"role": "assistant", "content": [
             {"type": "text", "text": "Local transcript fixture for the resume button."}]}},
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
    (paths["home"] / ".zshrc").write_text("PROMPT='TZ-FIXTURE> '\n")
    fixture = Path(__file__).with_name("timezone-real-01-provider.py")
    for name in ("claude", "codex", "dsh"):
        stub = run / f"{name}-fixture"
        argv = (str(Path(sys.executable).resolve()), str(fixture), name, str(run))
        stub.write_text(f'#!/bin/sh\nexec {shlex.join(argv)} "$@"\n')
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
        app = run / "MatouTimezoneReference.app"
        audit = read(EVIDENCE.parent / "ime-real-01/current/isolation.json")
        for key, digest in (("source_bundle", "baseline_asar_sha256"),
                            ("home_isolated_bundle", "harness_asar_sha256")):
            if sha(Path(audit[key]) / ARCHIVE) != audit[digest]:
                raise RuntimeError(f"Frozen payload changed: {key}")
        subprocess.run(["/bin/cp", "-cR", audit["home_isolated_bundle"], app], check=True)
        info = plistlib.loads((app / PLIST).read_bytes())
        title, inputs = "码头", audit
    bundle = f"com.matou.timezone-real-01.{side}"
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


def require_other_side_closed(side):
    other = EVIDENCE / ("reference" if side == "native" else "native") / "launch.json"
    if other.exists() and matches_live_process(read(other)):
        raise RuntimeError("Quit the other trial before launching this trial")


def relaunch(side):
    run = EVIDENCE / side
    require_other_side_closed(side)
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
    require_other_side_closed(side)
    paths = prepared["paths"]
    data = Path(paths["data"])
    identity = data.stat()
    alias = Path(f"/.vol/{identity.st_dev}/{identity.st_ino}")
    if not alias.samefile(data):
        raise RuntimeError("Isolated short socket alias did not resolve")
    env = {name: os.environ[name] for name in ("USER", "LOGNAME", "LANG", "LC_ALL") if name in os.environ}
    env.update(PATH=f"{run}/bin:/usr/bin:/bin:/usr/sbin:/sbin", SHELL="/bin/zsh", HOME=paths["home"],
               ZDOTDIR=paths["home"], TMPDIR=paths["tmp"] + "/", XDG_CACHE_HOME=paths["cache"],
               XDG_CONFIG_HOME=paths["home"] + "/.config", XDG_DATA_HOME=paths["home"] + "/.local/share",
               XDG_STATE_HOME=paths["home"] + "/.local/state", CLAUDE_CONFIG_DIR=paths["claude"],
               CLAUDE_SECURESTORAGE_CONFIG_DIR=paths["claude"], CODEX_HOME=paths["home"] + "/.codex",
               DSH_HOME=paths["home"] + "/.dsh", GIT_CONFIG_NOSYSTEM="1",
               MATOU_CLAUDE_COMMAND=str(run / "claude-fixture"),
               MATOU_CODEX_COMMAND=str(run / "codex-fixture"),
               MATOU_DSH_COMMAND=str(run / "dsh-fixture"), MATOU_DISABLE_LOGIN_PATH_PROBE="1",
               MATOU_DISABLE_AUTO_UPDATE="1")
    command = [prepared["executable"]]
    if side == "native":
        command += ["--data-root", str(alias), "--workspace", paths["workspace"], "--shell-home", paths["home"]]
    else:
        fixture_path = run / "egress-fixture.json"
        if not fixture_path.exists():
            successful = {"ip": "192.0.2.1", "country": "US"}
            save(fixture_path, {"anthropic": {"api": successful, "login": successful},
                                "openai": {"api": successful, "login": successful},
                                "tunDetected": True, "browserLogin": None})
        env.update(MATOU_REFERENCE_HOME=paths["home"], ELECTRON_USER_DATA_DIR=paths["user-data"],
                   MATOU_DATA_DIR=str(alias), MATOU_DEFAULT_WORKSPACE=paths["workspace"],
                   MATOU_E2E="1", MATOU_E2E_DISPLAY="primary", MATOU_E2E_WINDOW_CLOSE="hide",
                   MATOU_E2E_EGRESS_FIXTURE=str(fixture_path),
                   MATOU_UPDATE_BASE_URL="http://127.0.0.1:9/native-parity-disabled",
                   MATOU_CLAUDE_PROJECTS_ROOT=paths["home"] + "/.claude/projects")
        command += [f"--user-data-dir={paths['user-data']}"]
    with (run / "app.log").open("xb") as log:
        started_unix_ns, started_uptime_ns = time.time_ns(), time.monotonic_ns()
        process = subprocess.Popen(command, env=env, cwd=paths["workspace"], stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
    record = prepared | {"pid": process.pid, "command": command,
                         "runtime_data_path": str(alias), "started_unix_ns": started_unix_ns,
                         "started_uptime_ns": started_uptime_ns,
                         "app_environment": {key: env.get(key) for key in (
                             "HOME", "TZ", "LANG", "LC_ALL", "MATOU_LOCALE", "MATOU_CLAUDE_COMMAND",
                             "MATOU_CODEX_COMMAND", "MATOU_DSH_COMMAND", "MATOU_E2E_EGRESS_FIXTURE")}}
    save(run / "launch.json", record)
    print(json.dumps(record, ensure_ascii=False))

def provider_path(side):
    run = EVIDENCE / side
    folder = run / "bin"
    folder.mkdir()
    for name in ("claude", "codex", "dsh"):
        (folder / name).symlink_to(run / f"{name}-fixture")


def prepare_all(side):
    prepare(side)
    provider_path(side)


def closed_database(side):
    run = EVIDENCE / side
    launched = read(run / "launch.json")
    if matches_live_process(launched):
        raise RuntimeError("Quit the isolated instance before preparing its database")
    data = Path(launched["paths"]["data"])
    markers = list(data.glob("matou.sqlite*"))
    db = data / "matou.sqlite"
    if not db.is_file() or db.is_symlink():
        raise RuntimeError(f"Expected one closed isolated database, found: {markers}")
    sidecars = {path.name: path for path in markers if path != db}
    if sidecars:
        # The frozen read-only boot can leave a zero-byte WAL and its SHM.
        # Preserve both files after confirming that the main DB contains all pages.
        allowed = {"matou.sqlite-wal", "matou.sqlite-shm"}
        wal = sidecars.get("matou.sqlite-wal")
        if set(sidecars) != allowed or wal.stat().st_size or any(p.is_symlink() for p in sidecars.values()):
            raise RuntimeError(f"Preserving unexpected or nonempty database sidecars: {markers}")
        holders = subprocess.run(["/usr/sbin/lsof", *markers], capture_output=True, text=True)
        if holders.returncode != 1 or holders.stdout or holders.stderr:
            raise RuntimeError("Could not confirm that no process holds the database bundle")
        with sqlite3.connect(db.as_uri() + "?immutable=1&mode=ro", uri=True) as connection:
            assert connection.execute("PRAGMA quick_check").fetchall() == [("ok",)]
        preserved = run / "closed-sidecars"
        if preserved.exists():
            preserved = run / "closed-sidecars-2"
        preserved.mkdir(mode=0o700)
        for path in sidecars.values():
            path.rename(preserved / path.name)
    assert db.read_bytes().startswith(b"SQLite format 3\0")
    locale = (data if side == "native" else run / "user-data") / "locale.json"
    return run, db, locale


def readonly(side):
    run, db, locale = closed_database(side)
    assert json.loads(locale.read_text())["preference"] == "en"
    record = run / "readonly.json"
    if record.exists():
        record = run / "readonly-second.json"
    if record.exists():
        raise RuntimeError("Preserving prior read-only preparation records")
    db.chmod(0o400)
    save(record, {"database_sha256": sha(db), "database_mode": "0400", "locale": read(locale),
                  "locale_sha256": sha(locale), "time_ns": time.time_ns()})


def writable(side):
    run, db, locale = closed_database(side)
    db.chmod(0o600)
    save(run / "writable-retest.json", {"database_sha256": sha(db), "database_mode": "0600",
                                       "locale": read(locale), "time_ns": time.time_ns()})


def corrupt(side):
    run, db, locale = closed_database(side)
    assert json.loads(locale.read_text())["preference"] == "en"
    original = run / "healthy-before-recovery.sqlite"
    with original.open("xb") as output, db.open("rb") as source:
        shutil.copyfileobj(source, output)
    db.chmod(0o600)
    db.write_bytes(b"Broken header!\0\0" + original.read_bytes()[16:])
    save(run / "corrupt.json", {"healthy_sha256": sha(original), "damaged_sha256": sha(db),
                               "locale": read(locale), "locale_sha256": sha(locale), "time_ns": time.time_ns()})


def stage_retest(side):
    run = EVIDENCE / side
    launched = read(run / "launch.json")
    data = run / "data"
    old = run / "data-corruption"
    if matches_live_process(launched) or old.exists() or not (data / "matou.sqlite.recovery.json").is_file():
        raise RuntimeError("Expected the closed, isolated recovery specimen")
    provenance = read(run / "corrupt.json")
    healthy = run / "healthy-before-recovery.sqlite"
    locale_root = data if side == "native" else run / "user-data"
    assert sha(healthy) == provenance["healthy_sha256"]
    assert sha(locale_root / "locale.json") == provenance["locale_sha256"]
    data.rename(old)
    data.mkdir(mode=0o700)
    db = data / "matou.sqlite"
    shutil.copyfile(healthy, db)
    db.chmod(0o400 if side == "native" else 0o600)
    if side == "native":
        shutil.copy2(old / "locale.json", data / "locale.json")
    save(run / "retest.json", {"database_sha256": sha(db), "database_mode": oct(db.stat().st_mode & 0o777),
                              "locale": read(locale_root / "locale.json"), "time_ns": time.time_ns()})


def readback(side):
    run = EVIDENCE / side
    data = run / "data"
    locale = (data if side == "native" else run / "user-data") / "locale.json"
    db = data / "matou.sqlite"
    value = None
    if db.is_file():
        try:
            with sqlite3.connect(db.as_uri() + "?mode=ro", uri=True) as connection:
                row = connection.execute(
                    "SELECT value_json, updated_at FROM preferences WHERE key = ?", ("claude.timeZone",)
                ).fetchone()
                value = {"time_zone": json.loads(row[0]), "updated_at": row[1]} if row else None
        except sqlite3.Error as error:
            value = {"error": str(error)}
    print(json.dumps({"time_ns": time.time_ns(), "preference": read(locale) if locale.exists() else None,
                      "database_preference": value, "database_mode": oct(db.stat().st_mode & 0o777) if db.exists() else None,
                      "recovery_marker": (data / "matou.sqlite.recovery.json").exists()}, ensure_ascii=False))


def write_fault(side, armed):
    run = EVIDENCE / side
    launched = read(run / "launch.json")
    db = run / "data/matou.sqlite"
    if not matches_live_process(launched) or db.is_symlink() or not db.is_file():
        raise RuntimeError("Expected the live isolated instance with its own database")
    sql = (
        "CREATE TRIGGER timezone_real_01_save_fault BEFORE INSERT ON preferences "
        "WHEN NEW.key = 'claude.timeZone' BEGIN SELECT RAISE(ABORT, 'isolated save fault'); END"
        if armed else "DROP TRIGGER timezone_real_01_save_fault"
    )
    with sqlite3.connect(db.as_uri() + "?mode=rw", uri=True) as connection:
        connection.execute(sql)
    with (run / "save-fault.jsonl").open("a") as log:
        log.write(json.dumps({"pid": launched["pid"], "armed": armed, "time_ns": time.time_ns()}) + "\n")


if __name__ == "__main__":
    if sys.argv[1:] == ["build-tools"]:
        build_tools()
        raise SystemExit(0)
    action, side = sys.argv[1:]
    if side not in ("native", "reference"):
        raise SystemExit("Allowed sides: native, reference")
    {"prepare": prepare_all, "launch": launch, "relaunch": relaunch, "corrupt": corrupt,
     "readonly": readonly, "writable": writable, "readback": readback,
     "stage-retest": stage_retest,
     "arm-error": lambda side: write_fault(side, True),
     "clear-error": lambda side: write_fault(side, False)}[action](side)
