#!/usr/bin/env python3
"""Prepare and inspect an isolated macOS recovery action trial."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import sqlite3
import stat
import subprocess
import sys
import time
from contextlib import closing

PROJECT = Path(__file__).resolve().parents[2]
EVIDENCE = PROJECT / "artifacts/native-parity/native/evidence/recovery-actions-real-01"
PRIOR = EVIDENCE.with_name("recovery-water-real-02")
BIN = EVIDENCE / "build-target/debug"
DB = "matou.sqlite"
MARKER = f"{DB}.recovery.json"
CONDITIONS = ("first", "restore", "postfail", "empty", "export", "wal", "owner", "marker")
SUPPORT = EVIDENCE / "seed"


def sha(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def read(path):
    return json.loads(path.read_text())


def save(path, value):
    with path.open("x") as output:
        output.write(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def mkdirs(run):
    paths = {key: run / key for key in ("home", "data", "cache", "workspace", "tmp", "user-data")}
    for path in paths.values():
        path.mkdir(parents=True, mode=0o700, exist_ok=True)
    return paths


def isolated_env(paths):
    env = {key: os.environ[key] for key in ("USER", "LOGNAME", "LANG", "LC_ALL") if key in os.environ}
    env.update(PATH="/usr/bin:/bin:/usr/sbin:/sbin", SHELL="/bin/zsh",
               HOME=str(paths["home"]), ZDOTDIR=str(paths["home"]),
               TMPDIR=str(paths["tmp"]) + "/", XDG_CACHE_HOME=str(paths["cache"]),
               XDG_CONFIG_HOME=str(paths["home"] / ".config"))
    return env


def seed():
    paths = mkdirs(SUPPORT)
    (paths["home"] / ".zshrc").write_text("PROMPT='RECOVERY-01> '\n")
    command = [BIN / "matou-native-runtime", "--data-root", paths["data"],
               "--workspace", paths["workspace"], "--shell-home", paths["home"], "--shell", "/bin/zsh"]
    messages = (json.dumps({"Hello": {"version": 3}}) + "\n" + json.dumps({"Command": "Shutdown"}) + "\n")
    result = subprocess.run(command, cwd=paths["workspace"], env=isolated_env(paths),
                            text=True, input=messages, capture_output=True, timeout=15)
    (SUPPORT / "runtime.jsonl").write_text(result.stdout)
    (SUPPORT / "runtime.stderr").write_text(result.stderr)
    if result.returncode or '"Ready"' not in result.stdout or '"Closed"' not in result.stdout:
        raise RuntimeError("Native schema seed did not finish cleanly")
    original = SUPPORT / "base.sqlite"
    with closing(sqlite3.connect(paths["data"] / DB)) as source, closing(sqlite3.connect(original)) as target:
        source.backup(target)
        target.execute("PRAGMA journal_mode=DELETE")
    if not original.read_bytes().startswith(b"SQLite format 3\0"):
        raise RuntimeError("Unexpected fixture header")
    (SUPPORT / "damaged.sqlite").write_bytes(b"Broken header!\0\0" + original.read_bytes()[16:])
    save(SUPPORT / "fixture.json", {"healthy": describe(original), "damaged": describe(SUPPORT / "damaged.sqlite")})


def describe(path):
    info = path.lstat()
    row = {"mode": oct(stat.S_IMODE(info.st_mode)), "uid": info.st_uid,
           "gid": info.st_gid, "size": info.st_size}
    if path.is_file() and not path.is_symlink():
        row["sha256"] = sha(path)
    return row


def database_readback(path):
    # Never read-write open the tested database. A healthy active WAL snapshot is
    # taken after genuine app shutdown when SQL readback needs WAL contents.
    try:
        with closing(sqlite3.connect(path.as_uri() + "?immutable=1&mode=ro", uri=True)) as db:
            tables = [row[0] for row in db.execute("SELECT name FROM sqlite_schema WHERE type='table'")]
            result = {"integrity": list(db.execute("PRAGMA integrity_check")), "tables": tables}
            if "workspaces" in tables:
                result["workspaces"] = list(db.execute("SELECT id, name, root_directory FROM workspaces ORDER BY id"))
            if "gate_payload" in tables:
                result["sentinel"] = list(db.execute("SELECT * FROM gate_payload"))
            if "schema_migrations" in tables:
                result["schema_migrations"] = list(db.execute("SELECT version, checksum FROM schema_migrations ORDER BY version"))
            return result
    except sqlite3.Error as exc:
        return {"error": str(exc)}


def labelled_database(target, run, name, poison=False):
    shutil.copyfile(SUPPORT / "base.sqlite", target)
    with closing(sqlite3.connect(target)) as db, db:
        db.execute("UPDATE workspaces SET name=?, root_directory=?, path_identity=?",
                   (name, str(run / "workspace"), f"path:{run / 'workspace'}"))
        db.execute("CREATE TABLE gate_payload(value TEXT NOT NULL)")
        db.execute("INSERT INTO gate_payload VALUES (?)", (name,))
        if poison:
            db.execute("UPDATE schema_migrations SET checksum='fixture-post-replace-checksum-mismatch' WHERE version=1")
        version, = db.execute("SELECT MAX(version) FROM schema_migrations").fetchone()
    return version


def backup(run, name, timestamp, poison=False):
    directory = run / "data/backups"
    directory.mkdir(mode=0o700, exist_ok=True)
    staged = directory / f"staged-{timestamp}.sqlite"
    version = labelled_database(staged, run, name, poison)
    identifier = f"matou-{timestamp}-clean-exit-v{version}"
    target = directory / f"{identifier}.sqlite"
    staged.rename(target)
    target.chmod(0o600)
    check = database_readback(target)
    assert check["integrity"] == [("ok",)] and check["sentinel"] == [(name,)]
    manifest = directory / f"{identifier}.json"
    save(manifest, {"id": identifier, "path": str(target), "createdAt": timestamp,
                    "reason": "clean-exit", "schemaVersion": version,
                    "size": target.stat().st_size, "sha256": sha(target)})
    manifest.chmod(0o600)


def prepare(side, condition, suffix):
    run = EVIDENCE / f"{side}-{condition}{suffix}"
    run.mkdir(mode=0o700)
    paths = mkdirs(run)
    (paths["home"] / ".zshrc").write_text("PROMPT='RECOVERY-01> '\n")
    root = paths["data"]
    original = root / DB
    source = SUPPORT / ("base.sqlite" if condition in ("wal", "owner", "marker", "export") else "damaged.sqlite")
    shutil.copyfile(source, original)
    original.chmod(0o600)
    if condition in ("wal", "export"):
        for extension, payload in (("-wal", b"isolated invalid WAL header\n" * 4),
                                   ("-shm", b"isolated invalid SHM header\n")):
            original.with_name(DB + extension).write_bytes(payload)
    if condition == "owner":
        original.with_name(DB + ".owner").write_text("{")
    if condition == "marker":
        original.with_name(MARKER).write_text("{")
    if condition in ("restore", "postfail"):
        backup(run, "R01 Backup Alternate", 1795190400000)
        backup(run, "R01 Backup Selected" if condition == "restore" else "R01 Backup Poisoned",
               1795190401000, condition == "postfail")
    bundle_id = f"com.matou.recovery-actions-real-01.{run.name}"
    archive = Path("Contents/Resources/app.asar")
    sources = {}
    if side == "native":
        app = run / "MatouNative.app"
        macos = app / "Contents/MacOS"
        macos.mkdir(parents=True)
        (app / "Contents/Resources").mkdir()
        name = "MatouNative"
        for src, dst in (("matou-native-desktop", name), ("matou-native-runtime", "matou-native-runtime")):
            binary = BIN / src
            subprocess.run(["/bin/cp", "-c", binary, macos / dst], check=True)
            sources[src] = {"path": str(binary), "sha256": sha(binary)}
        info = {"CFBundleName": "Matou Native", "CFBundleDisplayName": "Matou Native",
                "CFBundleExecutable": name, "CFBundlePackageType": "APPL", "CFBundleVersion": "0.1.0",
                "NSHighResolutionCapable": True, "NSPrincipalClass": "NSApplication"}
    else:
        app = run / "MatouReferenceProbe.app"
        previous = read(PRIOR / "reference-launch.json")
        audit = read(PRIOR / "reference-readback.json")
        harness, frozen = Path(previous["app"]), Path(previous["baseline"])
        assert sha(harness / archive) == audit["copy_sha256"]
        assert sha(frozen / archive) == audit["original_sha256"]
        assert not audit["differences_except_home_line"] and not audit["errors"]
        subprocess.run(["/bin/cp", "-cR", harness, app], check=True)
        info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
        name = info["CFBundleExecutable"]
        sources = {"baseline": str(frozen), "harness": str(harness), "app_asar_audit": audit}
    info["CFBundleIdentifier"] = bundle_id
    (app / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
    subprocess.run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", app], check=True)
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", app], check=True)
    if side == "reference":
        assert sha(app / archive) == sources["app_asar_audit"]["copy_sha256"]
    executable = app / "Contents/MacOS" / name
    runtime = executable.with_name("matou-native-runtime") if side == "native" else None
    save(run / "prepared.json", {"paths": {k: str(v) for k, v in paths.items()},
                                 "side": side, "condition": condition, "app": str(app),
                                 "bundle_id": bundle_id, "title": "Matou Native" if side == "native" else "码头",
                                 "sources": sources, "executable": str(executable),
                                 "executable_sha256": sha(executable),
                                 "runtime_executable": str(runtime) if runtime else None,
                                 "runtime_sha256": sha(runtime) if runtime else None,
                                 "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()})
    snapshot(run, "before-launch")
    print(run)


def launch(run):
    record = read(run / "prepared.json")
    paths = {k: Path(v) for k, v in record["paths"].items()}
    env = isolated_env(paths)
    command = [record["executable"]]
    if record["side"] == "native":
        command += ["--data-root", str(paths["data"]), "--workspace", str(paths["workspace"]),
                    "--shell-home", str(paths["home"])]
    else:
        st = paths["data"].stat()
        alias = Path(f"/.vol/{st.st_dev}/{st.st_ino}")
        assert alias.samefile(paths["data"])
        env.update(MATOU_REFERENCE_HOME=str(paths["home"]), ELECTRON_USER_DATA_DIR=str(paths["user-data"]),
                   MATOU_DATA_DIR=str(alias), MATOU_DEFAULT_WORKSPACE=str(paths["workspace"]),
                   MATOU_DISABLE_AUTO_UPDATE="1", MATOU_UPDATE_BASE_URL="http://127.0.0.1:9/native-parity-disabled",
                   MATOU_LOCALE="zh-CN")
        command += [f"--user-data-dir={paths['user-data']}"]
    with (run / "app.log").open("xb") as log:
        started_uptime, started_unix = time.monotonic_ns(), time.time_ns()
        process = subprocess.Popen(command, env=env, cwd=paths["workspace"],
                                   stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    record.update(pid=process.pid, started_uptime_ns=started_uptime, started_unix_ns=started_unix,
                  command=command, data_path_used=env.get("MATOU_DATA_DIR", str(paths["data"])))
    with (run / "launch.next").open("x") as output:
        output.write(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
    (run / "launch.next").rename(run / "launch.json")
    print(json.dumps(record, ensure_ascii=False))


def stage(run):
    condition = read(run / "prepared.json")["condition"]
    root = run / "data"
    assert (root / MARKER).is_file() and list(root.glob(f"{DB}.corrupt-*"))
    assert not (root / DB).exists()
    name = {"first": "R01 Rechecked Workspace", "restore": "R01 Original Before Restore",
            "postfail": "R01 Original Before Failure", "empty": "R01 Original To Archive"}[condition]
    target = root / DB
    labelled_database(target, run, name)
    target.chmod(0o600)
    if condition == "empty":
        for extension in ("-wal", "-shm"):
            sidecar = root / (DB + extension)
            sidecar.write_bytes(f"R01 original {extension} archived byte sentinel\n".encode())
            sidecar.chmod(0o600)
    snapshot(run, "staged-original")


def snapshot(run, label):
    root = run / "data"
    rows = {}
    for path in sorted(root.rglob("*")):
        row = describe(path)
        if path.is_file() and not path.is_symlink():
            if path.name.endswith(".json") or ".recovery.json.resolved-" in path.name:
                try:
                    row["json"] = read(path)
                except (ValueError, UnicodeDecodeError):
                    row["invalid_json"] = True
            if path.suffix == ".sqlite" or ".sqlite.replaced-" in path.name or ".sqlite.corrupt-" in path.name:
                if not path.name.endswith(("-wal", "-shm")):
                    row["sqlite_immutable"] = database_readback(path)
        rows[str(path.relative_to(root))] = row
    save(run / f"{label}.json", {"unix_ns": time.time_ns(), "files": rows})
    print(f"{run.name}/{label}.json: {len(rows)} paths")


def main():
    operation, *arguments = sys.argv[1:]
    if operation == "seed":
        seed()
    elif operation == "prepare":
        side, condition, *suffix = arguments
        assert side in ("native", "reference") and condition in CONDITIONS
        assert not suffix or (len(suffix) == 1 and suffix[0] in ("-r2", "-r3"))
        prepare(side, condition, suffix[0] if suffix else "")
    else:
        name, *extra = arguments
        run = (EVIDENCE / name).resolve()
        assert run.is_dir() and run.parent == EVIDENCE and name == run.name
        if operation == "launch":
            launch(run)
        elif operation == "stage":
            stage(run)
        elif operation == "snapshot":
            label, = extra
            assert label.replace("-", "").isalnum()
            snapshot(run, label)
        else:
            raise SystemExit("Unknown operation")


if __name__ == "__main__":
    main()
