#!/usr/bin/env python3
"""Run the existing K-storage-recovery inputs against a native protocol-3 child."""
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import queue
import sqlite3
import subprocess
import sys
import tempfile
import threading
import uuid


WORKSPACE = Path(__file__).resolve().parents[2]
GATE = WORKSPACE / "artifacts/native-parity/native/evidence/porting-map-01/gates/K-storage-recovery"
DEFAULT_BINARY = WORKSPACE / "apps/native/target/debug/matou-native-runtime"
SCRATCH = WORKSPACE / ".tmp/g-storage-recovery"
BACKUP_FIXTURE = WORKSPACE / "apps/native/core/tests/fixtures/backup-reference.json"
BACKUP_INPUTS = GATE.parent / "K-storage-backup/inputs.json"
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
            env=os.environ | {
                "ZDOTDIR": str(home),
                "CLAUDE_CONFIG_DIR": str(home / "provider"),
                "MATOU_RECOVERY_EXPORT_DIR": str(data.parent / "readonly-export-location"),
            },
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
        return self.action("retry-open", expectedRecoveryId=recovery_id)

    def action(self, kind, request=None, **arguments):
        request = request or str(uuid.uuid4())
        self.send({"Command": {"DatabaseRecovery": {
            "requestId": request, "action": {"kind": kind, **arguments},
        }}})
        return request

    def result(self, request):
        _, result = self.event("DatabaseRecoveryResult")
        assert result["requestId"] == request, result
        return result

    def close_cleanly(self):
        self.send({"Command": "Shutdown"})
        assert self.event("Closed")[0] == "Closed"
        assert self.process.wait(timeout=10) == 0

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
                    for action, arguments in (
                        ("retry-open", {}),
                        ("restore-backup", {"backupId": "unused", "expectedRecoveryId": "wrong"}),
                        ("start-empty-database", {"expectedRecoveryId": "wrong"}),
                    ):
                        request = runtime.action(action, **arguments)
                        assert runtime.result(request)["error"]["code"] == "RECOVERY_ID_CHANGED"
                        assert file_bytes(data) == original
                    if name == "owner-malformed":
                        request = runtime.action("restore-backup", backupId="missing",
                                                 expectedRecoveryId=result["recoveryId"])
                        assert runtime.result(request)["error"]["code"] == "restore"
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

    verify_actions(binary, stage, inputs, seeds)
    verify_additional_exports(binary, stage, inputs, seeds)


def verify_additional_exports(binary, stage, inputs, seeds):
    for reason in ("ownership-recovery-required", "wal-recovery-required"):
        data = stage / reason / "data"
        owner = data / f"{fixtures.DB}.owner"
        takeover = data / f"{fixtures.DB}.owner.takeover.sqlite"
        if reason.startswith("ownership"):
            case = next(case for case in inputs["cases"] if case["name"] == "owner-malformed")
            fixtures.setup(data, case, inputs["marker"], seeds["healthy"])
            quarantine = data / fixtures.DB
            takeover.mkdir()
            (takeover / "nested").mkdir()
            (takeover / "nested/state").write_bytes(b"ownership evidence")
        else:
            fixtures.setup(data, {"marker": {"reason": reason}}, inputs["marker"], seeds["healthy"])
            quarantine = data / "matou.sqlite.corrupt-123"
            quarantine.write_bytes(seeds["damaged"])
        wal, shm = (quarantine.with_name(quarantine.name + suffix) for suffix in ("-wal", "-shm"))
        wal.write_bytes(b"K WAL evidence")
        shm.write_bytes(b"K SHM evidence")
        with Runtime(binary, data) as runtime:
            _, details = runtime.event("DatabaseRecoveryRequired")
            assert Path(details["quarantinedPath"]) == quarantine
            export = runtime.result(runtime.action("export-recovery-bundle"))
            bundle = Path(export["exportedPath"])
            assert bundle.parent == data / "recovery-exports"
            manifest = json.loads((bundle / "manifest.json").read_text())
            expected = [data / fixtures.MARKER, quarantine, wal, shm]
            if reason.startswith("ownership"):
                expected += [owner, takeover]
                assert manifest["ownershipIssue"] == details["ownershipIssue"] == "takeover-sidecar-unusable"
            else:
                assert "ownershipIssue" not in manifest and not (bundle / fixtures.DB).exists()
            assert manifest["reason"] == reason
            assert manifest["exportedFiles"] == [path.name for path in expected]
            for source in expected:
                target = bundle / source.name
                if source.is_dir():
                    assert file_bytes(source) == file_bytes(target)
                else:
                    assert source.read_bytes() == target.read_bytes()
            runtime.close_cleanly()
            print(f"K recovery / export {reason}: exact evidence set")


def verify_actions(binary, stage, inputs, seeds):
    backup = json.loads(BACKUP_FIXTURE.read_text())["wal"]
    backup_inputs = json.loads(BACKUP_INPUTS.read_text())
    damaged = next(case for case in inputs["cases"] if case["name"] == "quick-check-failure")

    for kind in ("retry-open", "restore-backup", "start-empty-database", "export-recovery-bundle"):
        data = stage / f"actions-{kind}" / "data"
        fixtures.setup(data, damaged, inputs["marker"], seeds["damaged"])
        database = data / fixtures.DB
        backups = data / "backups"
        backups.mkdir()
        descriptor = dict(backup["descriptor"])
        backup_id = descriptor["id"]
        candidate = backups / f"{backup_id}.sqlite"
        manifest_path = backups / f"{backup_id}.json"
        candidate.write_bytes(base64.b64decode(backup["bytesBase64"]))
        descriptor["path"] = str(candidate)
        manifest_path.write_text(json.dumps(descriptor))
        with Runtime(binary, data) as runtime:
            _, details = runtime.event("DatabaseRecoveryRequired")
            assert [b["id"] for b in details["backups"]] == [backup_id]
            recovery_id = details["recoveryId"]
            quarantine = Path(details["quarantinedPath"])
            marker = data / fixtures.MARKER
            marker_bytes = marker.read_bytes()
            settlement = data / f"{fixtures.MARKER}.resolved-{recovery_id}"
            arguments = {"expectedRecoveryId": recovery_id}
            if kind == "restore-backup":
                arguments["backupId"] = backup_id

            if kind != "export-recovery-bundle":
                # Same recovery ID, different canonical snapshot. All three writes
                # must refuse before touching the database or old evidence.
                altered = json.loads(marker_bytes)
                altered["quarantinedPath"] += "-different"
                marker.write_text(json.dumps(altered))
                before = file_bytes(data)
                assert runtime.result(runtime.action(kind, **arguments))["error"]["code"] == "marker"
                assert file_bytes(data) == before
                marker.write_bytes(marker_bytes)

            if kind == "retry-open":
                failure = runtime.result(runtime.action(kind, **arguments))
                assert failure["error"]["code"] == "open" and not database.exists()
                database.write_bytes(candidate.read_bytes())
            elif kind == "restore-backup":
                initial = candidate.read_bytes()
                invalid = stage / "integrity-bad-candidate.sqlite"
                invalid.write_bytes(initial)
                with sqlite3.connect(invalid) as corrupted:
                    corrupted.executescript("CREATE TABLE marker(value TEXT); INSERT INTO marker VALUES ('bad');")
                    corrupted.executescript(backup_inputs["invalidSql"])
                corrupted.close()
                with sqlite3.connect(invalid) as checked:
                    assert checked.execute("PRAGMA integrity_check").fetchall() != [("ok",)]
                checked.close()
                bad_integrity = invalid.read_bytes()
                for tampered, manifest in (
                    (initial[:-1], descriptor),
                    (bytes([initial[0] ^ 1]) + initial[1:], descriptor),
                    (bad_integrity, descriptor | {
                        "size": len(bad_integrity), "sha256": hashlib.sha256(bad_integrity).hexdigest(),
                    }),
                    (initial, descriptor | {"path": str(invalid)}),
                ):
                    candidate.write_bytes(tampered)
                    manifest_path.write_text(json.dumps(manifest))
                    before = file_bytes(data)
                    request = runtime.action(kind, **arguments)
                    _, refreshed = runtime.event("DatabaseRecoveryRequired")
                    assert refreshed["recoveryId"] == recovery_id and refreshed["backups"] == []
                    assert runtime.result(request)["error"]["code"] == "restore"
                    assert file_bytes(data) == before
                candidate.write_bytes(initial)
                manifest_path.write_text(json.dumps(descriptor))
                with sqlite3.connect(database) as original:
                    original.executescript(backup_inputs["oldTargetSql"])
                original.close()
            elif kind == "start-empty-database":
                database.write_bytes(candidate.read_bytes())
                wal = data / f"{fixtures.DB}-wal"
                shm = data / f"{fixtures.DB}-shm"
                wal.write_bytes(b"K WAL evidence")
                shm.write_bytes(b"K SHM evidence")
                evidence = data / "recovery-evidence"
                evidence.write_bytes(b"blocked")
                before = file_bytes(data)
                assert runtime.result(runtime.action(kind, **arguments))["error"]["code"] == "archive-original"
                assert file_bytes(data) == before
                evidence.unlink()
            else:
                directory = data / "recovery-exports"
                directory.write_bytes(b"blocked")
                before = file_bytes(data)
                failed = runtime.result(runtime.action(kind))
                assert failed["error"]["code"] == "DATABASE_EXPORT_FAILED", failed
                assert "recovery-exports" in failed["error"]["message"]
                assert file_bytes(data) == before
                directory.unlink()
                # A failure after the first copy preserves the partial directory
                # and never publishes a successful manifest or settles the marker.
                mode = quarantine.stat().st_mode & 0o777
                quarantine.chmod(0)
                try:
                    failed = runtime.result(runtime.action(kind))
                    assert failed["error"]["code"] == "DATABASE_EXPORT_FAILED", failed
                    assert "Permission denied" in failed["error"]["message"], failed
                finally:
                    quarantine.chmod(mode)
                partial, = directory.iterdir()
                assert (partial / marker.name).read_bytes() == marker_bytes
                assert not (partial / "manifest.json").exists()
                assert not settlement.exists()
                before = {k: v for k, v in file_bytes(data).items()
                          if not k.startswith("recovery-exports/")}
                request = runtime.action(kind)
                event, exported = runtime.event("DatabaseRecoveryResult", "Ready")
                assert event == "DatabaseRecoveryResult" and exported["requestId"] == request
                bundle = Path(exported["exportedPath"])
                assert bundle.parent == directory and bundle.is_dir()
                assert not (data.parent / "readonly-export-location").exists()
                manifest = json.loads((bundle / "manifest.json").read_text())
                assert manifest["reason"] == "physical-corruption" and manifest["backupCount"] == 1
                assert manifest["backups"] == [{k: v for k, v in descriptor.items() if k != "path"}]
                assert manifest["exportedFiles"] == [marker.name, quarantine.name]
                assert (bundle / marker.name).read_bytes() == marker_bytes
                assert (bundle / quarantine.name).read_bytes() == seeds["damaged"]
                assert (bundle / "manifest.json").stat().st_mode & 0o777 == 0o600
                assert {k: v for k, v in file_bytes(data).items() if not k.startswith("recovery-exports/")} == before
                assert (partial / marker.name).read_bytes() == marker_bytes
                assert not settlement.exists()
                runtime.close_cleanly()
                print("K recovery + K backup / export-recovery-bundle: I/O failure and verified output")
                continue

            assert not settlement.exists()
            request = runtime.action(kind, **arguments)
            assert runtime.event("Ready", "DatabaseRecoveryResult")[0] == "Ready"
            assert runtime.result(request) == {"requestId": request}
            assert settlement.is_file() and marker.read_bytes() == marker_bytes
            assert quarantine.read_bytes() == seeds["damaged"]
            with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as opened:
                count = opened.execute("SELECT count(*) FROM sqlite_master WHERE name='restored_rows'").fetchone()[0]
                if kind == "start-empty-database":
                    assert count == 0
                    archived = data / "recovery-evidence" / request
                    assert (archived / database.name).read_bytes() == candidate.read_bytes()
                    assert (archived / wal.name).read_bytes() == b"K WAL evidence"
                    assert (archived / shm.name).read_bytes() == b"K SHM evidence"
                else:
                    assert opened.execute("SELECT value FROM restored_rows").fetchall() == [("from-backup",)]
            opened.close()
            if kind == "restore-backup":
                rollback, = (path for path in data.glob("matou.sqlite.replaced-*")
                             if path.name.removeprefix("matou.sqlite.replaced-").isdigit())
                with sqlite3.connect(rollback.as_uri() + "?mode=ro", uri=True) as snapshot:
                    assert snapshot.execute("SELECT value FROM marker").fetchall() == [("old-target-wal",)]
                snapshot.close()
            runtime.close_cleanly()
            print(f"K recovery + K backup / {kind}: rejection, evidence and same-process Ready")


if __name__ == "__main__":
    SCRATCH.mkdir(parents=True, exist_ok=True)
    binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_BINARY
    with tempfile.TemporaryDirectory(prefix="k-fixtures-", dir=SCRATCH) as directory:
        verify(binary, Path(directory))
