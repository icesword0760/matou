"""Launch an isolated native or frozen test bundle under this gate's evidence."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shlex
import subprocess
import sys
import time

PROJECT = Path(__file__).resolve().parents[2]
EVIDENCE = PROJECT / "artifacts/native-parity/native/evidence/recovery-water-real-03"
PRIOR = EVIDENCE.with_name("recovery-water-real-02")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def launch(side, label):
    run = EVIDENCE / label
    run.mkdir()
    paths = {name: run / name for name in ("home", "data", "cache", "workspace", "tmp", "user-data")}
    for path in paths.values():
        path.mkdir()
    st = paths["data"].stat()
    data_alias = paths["data"] if side == "native" and label != "native" else Path(f"/.vol/{st.st_dev}/{st.st_ino}")
    if not data_alias.samefile(paths["data"]):
        raise SystemExit("Data alias is outside this run")

    shell_command = shlex.join(("/usr/bin/python3", str(Path(__file__).with_name(
        "recovery-water-real-03-pty.py")), str(run)))
    (paths["home"] / ".zshrc").write_text(f"if [[ -t 0 && -t 1 ]]; then\n  exec {shell_command}\nfi\n")

    bundle_id = f"com.matou.recovery-water-real-03.{side}"
    app = run / ("MatouNative.app" if side == "native" else "MatouReferenceProbe.app")
    info_file = app / "Contents/Info.plist"
    source = {}
    if side == "native":
        macos = app / "Contents/MacOS"
        macos.mkdir(parents=True)
        (app / "Contents/Resources").mkdir()
        name = "MatouNative"
        for stem, target in [("matou-native-desktop", name), ("matou-native-runtime", "matou-native-runtime")]:
            binary = PROJECT / "apps/native/target/debug" / stem
            source[stem] = {"sha256": digest(binary), "mtime_ns": binary.stat().st_mtime_ns}
            subprocess.run(["/bin/cp", "-c", str(binary), str(macos / target)], check=True)
        info = {
            "CFBundleName": "Matou Native", "CFBundleDisplayName": "Matou Native",
            "CFBundleExecutable": name, "CFBundlePackageType": "APPL",
            "CFBundleVersion": "0.1.0", "NSHighResolutionCapable": True,
            "NSPrincipalClass": "NSApplication",
        }
    else:
        previous = json.loads((PRIOR / "reference-launch.json").read_text())
        audit = json.loads((PRIOR / "reference-readback.json").read_text())
        archive = Path("Contents/Resources/app.asar")
        harness = Path(previous["app"])
        baseline = Path(previous["baseline"])
        if (digest(harness / archive) != audit["copy_sha256"] or
                digest(baseline / archive) != audit["original_sha256"] or
                audit["differences_except_home_line"] or audit["errors"]):
            raise SystemExit("Frozen reference source no longer matches isolation audit")
        subprocess.run(["/bin/cp", "-cR", str(harness), str(app)], check=True)
        info = plistlib.loads(info_file.read_bytes())
        name = info["CFBundleExecutable"]
        source = {"baseline": str(baseline), "harness": str(harness), "audit": audit}
    info["CFBundleIdentifier"] = bundle_id
    info_file.write_bytes(plistlib.dumps(info))
    subprocess.run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    executable = app / "Contents/MacOS" / name
    env = {key: os.environ[key] for key in ("USER", "LOGNAME", "LANG", "LC_ALL") if key in os.environ}
    env.update(PATH="/usr/bin:/bin:/usr/sbin:/sbin", SHELL="/bin/zsh",
               HOME=str(paths["home"]), ZDOTDIR=str(paths["home"]),
               TMPDIR=str(paths["tmp"]) + "/", XDG_CACHE_HOME=str(paths["cache"]))
    command = [str(executable)]
    payload = {name: digest(executable)}
    if side == "native":
        runtime = executable.with_name("matou-native-runtime")
        payload[runtime.name] = digest(runtime)
        command += ["--data-root", str(data_alias), "--workspace", str(paths["workspace"]),
                    "--shell-home", str(paths["home"])]
        env["MATOU_NATIVE_MOTION_TRACE"] = str(run / "native-motion.jsonl")
    else:
        payload["app.asar"] = digest(app / archive)
        if payload["app.asar"] != source["audit"]["copy_sha256"]:
            raise SystemExit("Frozen harness archive changed during copy")
        runtime = None
        env.update(MATOU_REFERENCE_HOME=str(paths["home"]),
                   ELECTRON_USER_DATA_DIR=str(paths["user-data"]), MATOU_DATA_DIR=str(data_alias),
                   MATOU_DEFAULT_WORKSPACE=str(paths["workspace"]), MATOU_DISABLE_AUTO_UPDATE="1",
                   MATOU_UPDATE_BASE_URL="http://127.0.0.1:9/native-parity-disabled", MATOU_LOCALE="zh-CN")
        command += [f"--user-data-dir={paths['user-data']}"]
    with (run / "app.log").open("xb") as log:
        started = time.monotonic_ns()
        process = subprocess.Popen(command, env=env, cwd=paths["workspace"], stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
    record = {
        "side": side, "pid": process.pid, "app": str(app), "command": command,
        "runtime_executable": str(runtime) if runtime else None, "data_alias": str(data_alias),
        "title": "Matou Native" if side == "native" else "码头", "bundle_id": bundle_id,
        "paths": {name: str(path) for name, path in paths.items()}, "source": source,
        "payload_sha256": payload, "started_uptime_ns": started,
        "commit_at_launch": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    }
    (run / "launch.json").write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({k: record[k] for k in ("pid", "app", "title", "bundle_id")}, ensure_ascii=False))


side, label = sys.argv[1:]
if side not in ("native", "reference") or label not in (
    "native", "native-double", "native-confirm", "reference", "native-retry", "reference-retry",
):
    raise SystemExit("Unknown isolated run")
launch(side, label)
