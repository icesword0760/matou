#!/usr/bin/env python3
"""Verify native work status with an isolated Claude CLI round trip over DeepSeek."""

import argparse
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, HTTPServer
import importlib.util
import json
from pathlib import Path
from queue import Empty, Queue
import re
import shutil
import subprocess
import sys
import tempfile
from threading import Thread
from urllib.parse import urlparse

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "native_resume_check",
    Path(__file__).resolve().parents[4] / "tooling/native/verify-claude-resume.py",
)
resume = importlib.util.module_from_spec(spec)
spec.loader.exec_module(resume)
identity, launch, require = resume.identity, resume.launch, resume.require


@contextmanager
def observe_notifications():
    samples = Queue()

    class Observer(BaseHTTPRequestHandler):
        def do_POST(self):
            payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            samples.put(payload)
            self.send_response(200)
            self.end_headers()

        def log_message(self, *_):
            pass

    with HTTPServer(("127.0.0.1", 0), Observer) as server:
        worker = Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}/notification", samples
        finally:
            server.shutdown()
            worker.join()


def notification_sample(payload, key):
    candidates = {name: payload.get(name) for name in
                  ("message", "body", "text", "prompt", "error", "description")}
    for name in ("notification", "data"):
        nested = payload.get(name)
        candidates[f"{name}.message"] = nested.get("message") if isinstance(nested, dict) else None
    field, text = next(((name, value.strip()) for name, value in candidates.items()
                        if isinstance(value, str) and value.strip()), (None, ""))
    return {"notification_type": payload.get("notification_type"),
            "payload_fields": sorted(payload), "text_field": field,
            "text": identity.AUTH_HEADER.sub(
                b"<redacted>", text.encode().replace(key, b"<redacted>")).decode()}


def observe_graph(host, session, key, wanted, proof):
    def observe(_, event):
        identity.inspect_message(event, session, [], key)
        return next((node["workStatus"] for node in event.get("Graph", {}).get("nodes", [])
                     if node["sessionId"] == session and node["workStatus"] == wanted), None)

    proof["graph_events"].append(host.wait(f"Graph workStatus={wanted}", observe, timeout=65))


def state(root, session):
    with launch.database(root) as db:
        return db.execute("SELECT work_status, version FROM sessions WHERE id=?", (session,)).fetchone()


def transitions(root, session):
    with launch.database(root) as db:
        return [dict(seq=sequence, previous=previous, status=status) for sequence, previous, status in
                db.execute("""SELECT command_deduplication.first_event_seq,
                    json_extract(payload_json, '$.previousStatus'),
                    json_extract(payload_json, '$.workStatus')
                    FROM domain_events JOIN command_deduplication USING(command_id)
                    WHERE session_id=? AND command_type='session.work-status'
                    ORDER BY command_deduplication.first_event_seq""", (session,))]


def verify(root, env, guard, binary, cli, config, key, notify_url, samples, notification_timeout, proof):
    cwd = root / "workspace/session-cwd"
    cwd.mkdir(parents=True)
    (root / "matou").mkdir()
    onboarding = {"hasCompletedOnboarding": True, "theme": "dark",
                  "projects": {str(cwd): {"hasTrustDialogAccepted": True}}}
    for path in (root / "home/.claude.json", root / "claude/.claude.json"):
        launch.write_json(path, onboarding)
    launch.write_json(root / "claude/settings.json", {
        "hooks": {"Notification": [{"hooks": [{"type": "http", "url": notify_url, "timeout": 5}]}]},
    })
    session = launch.seed(root, env, guard, binary, cwd)
    config_path = identity.config_file(root, config)
    proof.update(session_id=session, graph_events=[])
    with launch.runtime(root, env, guard, binary) as host:
        require(host.ready() == session, "Runtime opened a different card")
        card = resume.Card(host, session, key)
        card.interactive()
        run = resume.running(root, card, 1)
        settings = json.loads(resume.assert_argv(run, cli, None).read_bytes())
        endpoint = urlparse(settings["hooks"]["PermissionRequest"][0]["hooks"][0]["url"])
        proof["run_id"] = run["id"]

        card.input(identity.PROMPT.encode())
        card.wait("prompt in the CLI input field",
                  lambda: True if identity.PROMPT in card.screen else None)
        proof["startup_work_status"] = state(root, session)[0]
        if proof["startup_work_status"] == "running":
            # Onboarding prompts may submit Enter before the first question.
            with launch.database(root) as db:
                db.execute("UPDATE sessions SET work_status='idle' WHERE id=?", (session,))
            proof["reset_onboarding_work_status_in_fixture"] = True
        before_turn = state(root, session)[0]
        earlier_writes = len(transitions(root, session))
        card.input(b"\r")
        observe_graph(host, session, key, "running", proof)
        observe_graph(host, session, key, "idle", proof)
        provider_id = card.wait("real CLI transcript reply", lambda: identity.cli_exchange(root))
        binding = resume.binding(root, session, provider_id)
        require(binding and json.loads(binding["metadata_json"]).get("observedNormalStop") is True,
                "The CLI transcript and real Stop were not attributed to the same binding")
        proof.update(reply="OK", cli_session_id=provider_id, real_stop=True)

        resume.post_hook(endpoint, "PermissionRequest", provider_id, cwd)
        observe_graph(host, session, key, "needs-input", proof)
        require(state(root, session)[0] == "needs-input", "PermissionRequest was not persisted")
        with launch.database(root) as db:
            db.execute("UPDATE sessions SET work_status='error' WHERE id=?", (session,))
        before = state(root, session)
        resume.post_hook(endpoint, "Stop", provider_id, cwd)
        observe_graph(host, session, key, "error", proof)
        require(state(root, session) == before, "Stop mutated the error state")

        proof["work_status_events"] = transitions(root, session)[earlier_writes:]
        require([(row["previous"], row["status"]) for row in proof["work_status_events"]]
                == [(before_turn, "running"), ("running", "idle"), ("idle", "needs-input")],
                "Unexpected session.work-status transitions")
        try:
            payload = samples.get(timeout=notification_timeout)
        except Empty:
            proof["real_notification"] = "not observed"
        else:
            sample = proof["real_notification"] = notification_sample(payload, key)
            require(sample["text_field"], "Real CLI Notification had no supported text field")
        host.shutdown()

    require(state(root, session)[0] == "exited", "Confirmed CLI exit was not persisted")
    proof["confirmed_process_exit"] = "exited"
    identity.check_secret(root, config_path, key, proof)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--evidence", type=Path, default=launch.CHECKS / "claude-work-status.json")
    parser.add_argument("--notification-timeout", type=float, default=70)
    args = parser.parse_args()
    require(sys.platform == "darwin", "This real CLI test requires macOS sandbox-exec")
    cli, binary, evidence = args.claude.resolve(strict=True), args.runtime.resolve(strict=True), args.evidence.resolve()
    require(evidence.is_relative_to(launch.REPO), "Evidence must stay in the workspace")
    launch.CHECKS.mkdir(parents=True, exist_ok=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve()
    config, key = identity.provider_config(home)
    proof = {}
    with tempfile.TemporaryDirectory(prefix="claude-work-status-", dir=launch.CHECKS) as directory:
        root = Path(directory)
        try:
            guard = launch.sandbox(root, home)
            with resume.endpoint_proxy() as port, observe_notifications() as (notify_url, samples):
                env = identity.environment(root, cli)
                proxy = f"http://127.0.0.1:{port}"
                for name in ("HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"):
                    env[name] = proxy
                env.update(NO_PROXY="127.0.0.1,localhost", no_proxy="127.0.0.1,localhost")
                resume.probe_network(root, env, guard, port)
                version = subprocess.check_output([*guard, str(cli), "--version"], env=env, cwd=root,
                                                  text=True, timeout=10, stderr=subprocess.DEVNULL).strip()
                require(re.fullmatch(r"[\d.]+ \(Claude Code\)", version), "Unexpected CLI version")
                proof["cli_version"] = version
                verify(root, env, guard, binary, cli, config, key, notify_url,
                       samples, args.notification_timeout, proof)
            proof["passed"] = True
        except Exception as error:
            message = str(error) if isinstance(error, RuntimeError) else type(error).__name__
            proof["error"] = identity.AUTH_HEADER.sub(
                b"<redacted>", message.encode().replace(key, b"<redacted>")).decode()
            proof["passed"] = False
    proof["isolated_root_removed"] = not root.exists()
    launch.write_json(evidence, proof)
    print(f"Work status evidence: {evidence}", flush=True)
    if not proof["passed"]:
        print(proof["error"], file=sys.stderr, flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
