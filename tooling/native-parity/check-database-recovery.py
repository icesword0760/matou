#!/usr/bin/env python3
"""Run the existing K-storage-recovery inputs against a native protocol-3 child."""
import importlib.util
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import uuid


WORKSPACE = Path(__file__).resolve().parents[2]
GATE = WORKSPACE / "artifacts/native-parity/native/evidence/porting-map-01/gates/K-storage-recovery"
DEFAULT_BINARY = WORKSPACE / "apps/native/target/debug/matou-native-runtime"
SCRATCH = WORKSPACE / ".tmp/g-storage-recovery"
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("recovery_gate_fixtures", GATE / "replay.py")
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)


class Runtime:
    def __init__(self, binary, data):
        self.events = queue.Queue()
        home = data.parent / "shell-home"
        self.process = subprocess.Popen(
            [binary, "--data-root", data, "--workspace", data.parent / "workspace",
             "--shell-home", home, "--shell", "/bin/zsh"],
            env=os.environ | {"ZDOTDIR": str(home), "CLAUDE_CONFIG_DIR": str(home / "provider")},
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        )
        threading.Thread(target=self.read, daemon=True).start()
        self.send({"Hello": {"version": 3}})
        assert self.events.get(timeout=10) == {"Hello": {"version": 3}}

    def read(self):
        for line in self.process.stdout:
            self.events.put(json.loads(line))
        self.events.put(None)

    def send(self, message):
        self.process.stdin.write(json.dumps(message).encode() + b"\n")
        self.process.stdin.flush()

    def event(self, *accepted):
        while message := self.events.get(timeout=10):
            event = message["Event"]
            if isinstance(event, str):
                name, content = event, None
            else:
                (name, content), = event.items()
            if name in accepted:
                return name, content
            assert name != "Closed", (accepted, "premature Closed")
        raise AssertionError(f"Runtime ended before {accepted}")

    def retry(self, recovery_id):
        request = str(uuid.uuid4())
        self.send({"Command": {"DatabaseRecovery": {
            "requestId": request, "action": {"kind": "retry-open", "expectedRecoveryId": recovery_id},
        }}})
        return request

    def __enter__(self):
        return self

    def __exit__(self, error, *_):
        if self.process.poll() is None and error:
            self.process.kill()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
            raise
        finally:
            self.process.stdin.close()
            self.process.stdout.close()


def file_bytes(root):
    return {
        str(path.relative_to(root)): path.read_bytes()
        for path in root.rglob("*") if path.is_file()
    }


def verify(binary, stage):
    inputs = json.loads((GATE / "inputs.json").read_text())
    seeds = fixtures.fixture_databases(stage, inputs)
    names = {
        "marker-malformed", "marker-invalid-state", "marker-invalid-version",
        "marker-invalid-path", "quick-check-failure", "retry-action-and-reopen",
        "read-only-directory", "read-only-damaged", "owner-malformed",
    }
    for case in inputs["cases"]:
        name = case["name"]
        if name not in names:
            continue
        data = stage / name / "data"
        database = data / fixtures.DB
        marker = data / fixtures.MARKER
        fixtures.setup(data, case, inputs["marker"], seeds[case.get("db", "healthy")])
        before = file_bytes(data)
        try:
            with Runtime(binary, data) as runtime:
                event, result = runtime.event(
                    "DatabaseRecoveryRequired", "StartupBlocked", "Diagnostic", "Ready", "ReadOnlyReady",
                )
                if name.startswith("marker-"):
                    assert event == "Diagnostic" and "recovery marker is invalid" in result
                    assert runtime.event("Closed")[0] == "Closed"
                    assert runtime.process.wait(timeout=10) != 0
                    assert file_bytes(data) == before
                elif name == "read-only-directory":
                    assert event == "ReadOnlyReady" and result["reason"] == "filesystem-read-only", (event, result)
                    runtime.send({"Command": "Shutdown"})
                    assert runtime.event("Closed")[0] == "Closed"
                    assert runtime.process.wait(timeout=10) == 0
                    assert file_bytes(data) == before
                elif name == "read-only-damaged":
                    assert event == "StartupBlocked" and result["kind"] == "Configuration", (event, result)
                    assert runtime.event("Closed")[0] == "Closed"
                    assert runtime.process.wait(timeout=10) != 0
                    assert file_bytes(data) == before
                else:
                    assert event == "DatabaseRecoveryRequired", (event, result)
                    assert "markerPath" not in result
                    assert all("path" not in backup for backup in result["backups"])
                    original = file_bytes(data)
                    wrong = runtime.retry("wrong-recovery-generation")
                    _, failure = runtime.event("DatabaseRecoveryResult")
                    assert failure["requestId"] == wrong
                    assert failure["error"]["code"] == "RECOVERY_ID_CHANGED"
                    assert file_bytes(data) == original
                    for action in (
                        {"kind": "retry-open"},
                        {"kind": "restore-backup", "backupId": "unused",
                         "expectedRecoveryId": result["recoveryId"]},
                        {"kind": "start-empty-database", "expectedRecoveryId": result["recoveryId"]},
                        {"kind": "export-recovery-bundle"},
                    ):
                        request = str(uuid.uuid4())
                        runtime.send({"Command": {"DatabaseRecovery": {
                            "requestId": request, "action": action,
                        }}})
                        _, failure = runtime.event("DatabaseRecoveryResult")
                        assert failure["requestId"] == request
                        expected = "RECOVERY_ID_CHANGED" if action["kind"] == "retry-open" else "RECOVERY_NOT_IMPLEMENTED"
                        assert failure["error"]["code"] == expected
                        assert file_bytes(data) == original
                    if name == "quick-check-failure":
                        assert not database.exists()
                        assert Path(result["quarantinedPath"]).read_bytes() == seeds["damaged"]
                        database.write_bytes(seeds["healthy"])
                    runtime.send({"Command": {"SaveProvider": {"request_id": 17, "input": {}}}})
                    _, declined = runtime.event("StorageCommandRejected")
                    assert declined["request_id"] == 17 and declined["message"].startswith("STORAGE_READ_ONLY")
                    request = runtime.retry(result["recoveryId"])
                    pid = runtime.process.pid
                    assert runtime.event("Ready", "DatabaseRecoveryResult")[0] == "Ready"
                    _, completed = runtime.event("DatabaseRecoveryResult")
                    assert completed == {"requestId": request}
                    assert runtime.process.pid == pid and runtime.process.poll() is None
                    assert marker.read_bytes() == original[fixtures.MARKER]
                    tombstone = data / f'{fixtures.MARKER}.resolved-{result["recoveryId"]}'
                    assert json.loads(tombstone.read_bytes())["state"] == "resolved"
                    runtime.send({"Command": "Shutdown"})
                    assert runtime.event("Closed")[0] == "Closed"
                    assert runtime.process.wait(timeout=10) == 0
                    if name in {"quick-check-failure", "retry-action-and-reopen"}:
                        for _ in range(2):
                            with Runtime(binary, data) as reopened:
                                assert reopened.event("Ready", "DatabaseRecoveryRequired")[0] == "Ready"
                                reopened.send({"Command": "Shutdown"})
                                assert reopened.event("Closed")[0] == "Closed"
                                assert reopened.process.wait(timeout=10) == 0
                print(f"{name}: passed")
        finally:
            if case.get("readOnly"):
                data.chmod(0o755)
                database.chmod(0o644)


if __name__ == "__main__":
    SCRATCH.mkdir(parents=True, exist_ok=True)
    binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_BINARY
    with tempfile.TemporaryDirectory(prefix="k-fixtures-", dir=SCRATCH) as directory:
        verify(binary, Path(directory))
