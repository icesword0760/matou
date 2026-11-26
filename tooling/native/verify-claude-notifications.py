#!/usr/bin/env python3
"""Verify notification storage and delivery with the installed CLI over DeepSeek."""

import argparse
from contextlib import closing
from http.client import HTTPConnection
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlparse
from uuid import uuid4

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "native_resume_check", Path(__file__).with_name("verify-claude-resume.py")
)
resume = importlib.util.module_from_spec(spec)
spec.loader.exec_module(resume)
identity, launch, require = resume.identity, resume.launch, resume.require


class Client(launch.Runtime):
    def __init__(self, root, env, guard, binary):
        self.notifications = []
        super().__init__(root, env, guard, binary)

    def wait(self, label, accept, timeout=12):
        def observe(message, event):
            self.notifications.extend(event.get("Graph", {}).get("agentNotifications", []))
            return accept(message, event)
        return super().wait(label, observe, timeout)


def post(endpoint, payload):
    require(endpoint.hostname == "127.0.0.1", "Hook settings did not use loopback")
    with closing(HTTPConnection(endpoint.hostname, endpoint.port, timeout=4)) as connection:
        connection.request("POST", endpoint.path, json.dumps(payload),
                           {"Content-Type": "application/json"})
        response = connection.getresponse()
        require(response.status == 200, "Hook did not acknowledge the injected payload")
        response.read()


def notifications(root, session, run):
    with launch.database(root) as db:
        rows = db.execute(
            """SELECT event_id, aggregate_type, aggregate_id, workspace_id, task_id,
                      session_id, schema_version, occurred_at, payload_json
               FROM domain_events WHERE event_type='agent.notification' AND session_id=?
                 AND json_extract(payload_json, '$.runId')=? ORDER BY seq""",
            (session, run),
        ).fetchall()
        workspace, task = db.execute(
            "SELECT workspace_id, task_id FROM sessions JOIN tasks ON tasks.id=sessions.task_id "
            "WHERE sessions.id=?", (session,),
        ).fetchone()
    records = []
    for event_id, aggregate, aggregate_id, workspace_id, task_id, session_id, version, now, raw in rows:
        payload = json.loads(raw)
        require((aggregate, aggregate_id, workspace_id, task_id, session_id, version)
                == ("session", session, workspace, task, session, 1),
                "Notification domain envelope did not match the owning session")
        require(set(payload) == {"targetSessionId", "runId", "provider", "event"}
                and payload["targetSessionId"] == session and payload["provider"] == "claude-code"
                and payload["runId"] == run and isinstance(now, int) and now > 10**12,
                "Notification domain payload did not match the frozen contract")
        records.append(dict(payload["event"], eventId=event_id, workspaceId=workspace_id,
                            taskId=task_id, sessionId=session_id, occurredAt=now))
    return records


def verify(root, env, guard, binary, cli, config, key, proof):
    cwd = root / "workspace/session-cwd"
    cwd.mkdir(parents=True)
    (root / "matou").mkdir()
    onboarding = {"hasCompletedOnboarding": True, "theme": "dark",
                  "projects": {str(cwd): {"hasTrustDialogAccepted": True}}}
    for path in (root / "home/.claude.json", root / "claude/.claude.json"):
        launch.write_json(path, onboarding)
    session = launch.seed(root, env, guard, binary, cwd)
    config_path = identity.config_file(root, config)
    try:
        with closing(Client(root, env, guard, binary)) as host:
            require(host.ready() == session, "Runtime opened a different card")
            card = resume.Card(host, session, key)
            run = resume.running(root, card, 1)
            settings = json.loads(resume.assert_argv(run, cli, None).read_bytes())
            endpoint = urlparse(settings["hooks"]["PermissionRequest"][0]["hooks"][0]["url"])
            card.interactive()
            token = f"NOTIFY-{uuid4().hex[:8].upper()}"
            prompt = f"Reply with only {token}. Do not call tools."
            provider_id, reply, _ = card.ask(root, prompt, token)

            def delivered(event):
                matches = [row for row in notifications(root, session, run["id"])
                           if all(row.get(field) == value for field, value in event.items())]
                require(len(matches) <= 1, "A single hook published duplicate database notifications")
                if matches:
                    seen = [row for row in host.notifications if row["eventId"] == matches[0]["eventId"]]
                    require(len(seen) <= 1, "A notification was sent to the client twice")
                    if seen:
                        graph = seen[0]
                        persisted = matches[0]
                        # sceneId is a delivery-only field.
                        scene_id = graph.get("sceneId")
                        require(graph == persisted or (
                            isinstance(scene_id, str) and scene_id != ""
                            and {key: value for key, value in graph.items() if key != "sceneId"} == persisted
                        ), "Graph notification differed from the SQLite event")
                        return graph
                return None

            completed = dict(eventType="completed", title="Claude Code",
                             subtitle=f"Completed in {cwd.name}", body=reply,
                             sound=True, cooldownKey="Stop")
            proof["completed"] = card.wait("persisted and delivered real Stop reply", lambda: delivered(completed))
            proof["transcript_reply"] = reply
            proof["run_id"] = run["id"]

            hook = dict(session_id=provider_id, cwd=str(cwd))
            post(endpoint, dict(hook, hook_event_name="PermissionRequest", tool_name="Bash",
                                tool_input={"command": "printf notification-test"}))
            permission = dict(eventType="permission", title="Claude Code", subtitle="Permission",
                              body="Bash: printf notification-test", sound=True,
                              cooldownKey="Notification")
            proof["permission"] = card.wait("persisted and delivered injected permission",
                                            lambda: delivered(permission))
            for _ in range(2):
                card.input(b"\x03")
                time.sleep(0.25)
            resume.exited(root, card, 1)
            before = notifications(root, session, run["id"])
            post(endpoint, dict(hook, hook_event_name="Stop", last_assistant_message="RETIRED-STOP"))
            card.wait("retired hook rejection", lambda: next((
                event for event in card.events if event["runId"] == run["id"]
                and event["result"] == "rejected" and event["reason"] == "retired-run"
            ), None))
            require(notifications(root, session, run["id"]) == before,
                    "The retired hook wrote a notification")
            require(not any(item["body"] == "RETIRED-STOP" for item in host.notifications),
                    "The retired hook reached the client")
            require(delivered(completed) and delivered(permission),
                    "A later graph publication replayed an earlier notification")
            proof["retired_stop"] = {"hook_acknowledged": True, "rejected": True,
                                     "new_database_events": 0, "new_client_events": 0}
            host.shutdown()
    finally:
        identity.check_secret(root, config_path, key, proof)
        proof["credential_absent_from_artifacts"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--evidence", type=Path, default=launch.CHECKS / "claude-notifications.json")
    args = parser.parse_args()
    require(sys.platform == "darwin", "This real CLI test requires macOS sandbox-exec")
    require(args.claude, "Install Claude Code or supply --claude")
    cli, binary, evidence = args.claude.resolve(strict=True), args.runtime.resolve(strict=True), args.evidence.resolve()
    require(evidence.is_relative_to(launch.REPO), "Evidence must stay in the workspace")
    launch.CHECKS.mkdir(parents=True, exist_ok=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve()
    config, key = identity.provider_config(home)
    proof = {}
    with tempfile.TemporaryDirectory(prefix="claude-notifications-", dir=launch.CHECKS) as directory:
        root = Path(directory)
        try:
            guard = launch.sandbox(root, home)
            with resume.endpoint_proxy() as port:
                env = identity.environment(root, cli)
                proxy = f"http://127.0.0.1:{port}"
                for name in ("HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"):
                    env[name] = proxy
                env.update(NO_PROXY="127.0.0.1,localhost", no_proxy="127.0.0.1,localhost")
                resume.probe_network(root, env, guard, port)
                proof["cli_version"] = subprocess.check_output(
                    [*guard, str(cli), "--version"], env=env, cwd=root,
                    text=True, timeout=10, stderr=subprocess.DEVNULL,
                ).strip()
                verify(root, env, guard, binary, cli, config, key, proof)
            proof["passed"] = True
        except Exception as error:
            message = str(error) if isinstance(error, RuntimeError) else type(error).__name__
            proof["error"] = identity.AUTH_HEADER.sub(
                b"<redacted>", message.encode().replace(key, b"<redacted>")).decode()
            proof["passed"] = False
    proof["isolated_root_removed"] = not root.exists()
    launch.write_json(evidence, proof)
    print(f"Notification evidence: {evidence}", flush=True)
    if not proof["passed"]:
        print(proof["error"], file=sys.stderr, flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
