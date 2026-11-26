#!/usr/bin/env python3
"""Seed a disposable native settings window and read its saved assignments."""

import argparse
import json
import os
from pathlib import Path
import plistlib
import sqlite3
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
OUTPUT = HERE / "output/assignment-ui-01"
FALLBACK = {
    "id": "assignment-default",
    "cli": "claude-code",
    "name": "隔离默认供应商",
    "endpoint": "https://assignment-default.invalid/v1",
    "apiKey": "assignment-fixture-default",
    "model": "opus",
    "modelMapping": {
        "opus": "fixture-default-opus",
        "sonnet": "fixture-default-sonnet",
        "haiku": "fixture-default-haiku",
    },
    "modelMetadata": [
        {"id": "fixture-default-opus", "displayName": "测试 Opus"},
        {"id": "fixture-default-sonnet", "displayName": "测试 Sonnet"},
        {"id": "fixture-default-haiku", "displayName": "测试 Haiku"},
    ],
}
TARGET = {
    "id": "assignment-target",
    "cli": "claude-code",
    "name": "隔离备用供应商",
    "endpoint": "https://assignment-target.invalid/v1",
    "apiKey": "assignment-fixture-target",
    "model": "fixture-target-opus",
    "modelMetadata": [
        {
            "id": "fixture-target-opus",
            "displayName": "备用 Opus",
            "supportedEfforts": ["medium", "high"],
        }
    ],
}
OFFICIAL = {
    "id": "anthropic-official",
    "cli": "claude-code",
    "name": "Anthropic 官方",
    "endpoint": "https://api.anthropic.com",
    "model": "",
    "builtIn": True,
}


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def prepare(run):
    run.mkdir(parents=True, exist_ok=False)
    root = Path(tempfile.mkdtemp(prefix="fixture-", dir=run))
    data = root / "data"
    data.mkdir()
    target = dict(TARGET)
    if run.name == "native-no-key":
        del target["apiKey"]
    providers = [OFFICIAL] if run.name == "native-official" else [FALLBACK, target]
    provider_config = data / "provider-config.json"
    write_json(provider_config, {
        "version": 1,
        "providers": {"claude-code": providers},
        "activeProviderIds": {"claude-code": providers[0]["id"]},
    })
    provider_config.chmod(0o600)
    write_json(run / "environment.json", {"root": str(root)})
    print(run)


def fixture_root(run):
    root = Path(json.loads((run / "environment.json").read_text())["root"])
    if not root.resolve().is_relative_to(run):
        raise SystemExit("Run data is outside the isolated fixture")
    return root


def launch(run, target, cli):
    root = fixture_root(run)
    paths = {name: root / name for name in ("data", "home", "workspace", "cache", "tmp")}
    for path in paths.values():
        path.mkdir(exist_ok=True)

    app_name = {"native-no-key": "AssignmentKeyState", "native-official": "AssignmentOfficial"}
    app = run / (app_name.get(run.name, "AssignmentUI") + ".app")
    executable = app / "Contents/MacOS/MatouAssignment"
    runtime = executable.parent / "matou-native-runtime"
    launch_file = run / "launch.json"
    if launch_file.exists():
        pid = json.loads(launch_file.read_text())["pid"]
        previous = subprocess.run(
            ["ps", "-p", str(pid), "-o", "comm="], capture_output=True, text=True,
        )
        if previous.stdout.strip() == str(executable):
            raise SystemExit("Quit the previous isolated window before launching again")
    executable.parent.mkdir(parents=True, exist_ok=True)
    executable.unlink(missing_ok=True)
    runtime.unlink(missing_ok=True)
    subprocess.run(["cp", "-c", str(target / "matou-native-desktop"), str(executable)], check=True)
    subprocess.run(["cp", "-c", str(target / runtime.name), str(runtime)], check=True)
    identifier = f"com.matou.native-parity.assignment-{run.name}"
    with (app / "Contents/Info.plist").open("wb") as file:
        plistlib.dump({
            "CFBundleIdentifier": identifier,
            "CFBundleName": app.stem,
            "CFBundleExecutable": executable.name,
            "CFBundlePackageType": "APPL",
            "NSHighResolutionCapable": True,
            "NSPrincipalClass": "NSApplication",
        }, file)
    for binary in (runtime, app):
        subprocess.run(["codesign", "--force", "--sign", "-", str(binary)], check=True)
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("MATOU_", "ANTHROPIC_", "CLAUDE_"))}
    env.update(HOME=str(paths["home"]), ZDOTDIR=str(paths["home"]),
               TMPDIR=str(paths["tmp"]), XDG_CACHE_HOME=str(paths["cache"]),
               MATOU_CLAUDE_COMMAND=str(cli))
    command = [
        str(executable), "--data-root", str(paths["data"]), "--workspace",
        str(paths["workspace"]), "--shell-home", str(paths["home"]),
    ]
    with (run / "runtime.log").open("w") as log:
        process = subprocess.Popen(command, env=env, cwd=paths["workspace"],
                                   stdin=subprocess.DEVNULL, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
    record = {"pid": process.pid, "app": str(app), "bundleIdentifier": identifier,
              "data": str(paths["data"]), "cli": str(cli)}
    write_json(launch_file, record)
    print(json.dumps(record))


def readback(run):
    data = fixture_root(run) / "data"
    provider_config = json.loads((data / "provider-config.json").read_text())
    with sqlite3.connect((data / "matou.sqlite").as_uri() + "?mode=ro", uri=True) as db:
        stored = db.execute(
            "SELECT value_json FROM preferences WHERE key = ?",
            ("agent.modelPreferences",),
        ).fetchone()
    preferences = json.loads(stored[0]) if stored else {}
    return {
        "activeProviderId": provider_config["activeProviderIds"]["claude-code"],
        "providers": [
            {"id": provider["id"], "hasApiKey": bool(provider.get("apiKey"))}
            for provider in provider_config["providers"]["claude-code"]
        ],
        "storedAssignments": preferences.get("claudeRoleAssignments"),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "launch", "readback"))
    parser.add_argument("run", choices=("native", "native-no-key", "native-official"))
    parser.add_argument("--target", type=Path, default=HERE.parents[1] / "apps/native/target/debug")
    parser.add_argument("--cli", type=Path, default=Path("/usr/bin/false"))
    parser.add_argument("--record", choices=("default", "saved", "restored", "no-key", "official-saved"))
    args = parser.parse_args()
    run = OUTPUT / args.run
    if args.action == "prepare":
        prepare(run)
    elif args.action == "launch":
        launch(run, args.target.resolve(strict=True), args.cli.resolve(strict=True))
    else:
        value = readback(run)
        if args.record:
            expected = {
                "default": None,
                "saved": {"opus": {
                    "providerId": TARGET["id"], "model": "fixture-target-opus", "effort": "high",
                }},
                "restored": {},
                "no-key": None,
                "official-saved": {"opus": {"providerId": OFFICIAL["id"], "model": "opus"}},
            }
            if value["storedAssignments"] != expected[args.record]:
                raise SystemExit(f"Unexpected stored assignments: {value}")
            write_json(OUTPUT / f"{args.record}-readback.json", value)
        print(json.dumps(value, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
