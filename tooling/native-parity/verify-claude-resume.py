#!/usr/bin/env python3
"""Verify native targeted restore using real Claude Code and the authorized DeepSeek record."""

import argparse
import contextlib
import importlib.util
from http.client import HTTPConnection
import json
import os
from pathlib import Path
import re
import secrets
import select
import shlex
import shutil
import socket
import socketserver
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlparse
from uuid import uuid4

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "native_identity_check", Path(__file__).with_name("verify-claude-identity.py")
)
identity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(identity)
launch = identity.launch
require = launch.require
MISSING = "provider session not found"
MISMATCH = ("The context returned by the AI session does not match the session being "
            "restored; try restoring again")
READBACK = "上一轮的代号是什么？只回答代号。不要调用工具。"
AUTHORITY = urlparse(identity.ENDPOINT).netloc + ":443"


class EndpointProxy(socketserver.StreamRequestHandler):
    """Forward only the authorized TLS tunnel. The test never inspects TLS payloads."""

    def handle(self):
        self.request.settimeout(10)
        request = self.rfile.readline(4096).split()
        for _ in range(32):
            if self.rfile.readline(4096) in (b"\r\n", b""):
                break
        if request[:2] != [b"CONNECT", AUTHORITY.encode()]:
            self.wfile.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            return
        try:
            with socket.create_connection((AUTHORITY.split(":")[0], 443), timeout=15) as upstream:
                self.wfile.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                peers = {self.request: upstream, upstream: self.request}
                while ready := select.select(list(peers), [], [], 120)[0]:
                    for source in ready:
                        data = source.recv(65536)
                        if not data:
                            return
                        peers[source].sendall(data)
        except OSError:
            return


@contextlib.contextmanager
def endpoint_proxy():
    with socketserver.ThreadingTCPServer(("127.0.0.1", 0), EndpointProxy) as server:
        server.daemon_threads = True
        server.handle_error = lambda *_: None
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield server.server_address[1]
        finally:
            server.shutdown()
            worker.join()


def probe_network(root, env, guard, port):
    program = r"""
import errno, socket, sys
try:
    socket.create_connection(("203.0.113.1", 443), timeout=2)
except OSError as error:
    assert error.errno in (errno.EACCES, errno.EPERM)
else:
    raise AssertionError("direct outbound unexpectedly allowed")
with socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=2) as proxy:
    proxy.sendall(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
    assert proxy.recv(4096).startswith(b"HTTP/1.1 403")
"""
    subprocess.run([*guard, sys.executable, "-I", "-B", "-c", program, str(port)],
                   env=env, cwd=root, check=True, timeout=8, capture_output=True)


def rows(root, table, session_id):
    with launch.database(root) as db:
        db.row_factory = sqlite3.Row
        order = "ordinal" if table == "session_runs" else "created_at, id"
        return [dict(row) for row in db.execute(
            f"SELECT * FROM {table} WHERE session_id=? ORDER BY {order}", (session_id,)
        )]


def binding(root, session_id, provider_id):
    return next((row for row in rows(root, "provider_bindings", session_id)
                 if row["provider_session_id"] == provider_id), None)


def revision(row):
    return json.loads(row["metadata_json"]).get("spawnRevision", 0)


def assert_argv(run, cli, expected):
    try:
        command = subprocess.check_output(
            ["/bin/ps", "-ww", "-p", str(run["pid"]), "-o", "args="],
            timeout=5, text=True, stderr=subprocess.DEVNULL,
        ).strip()
    except subprocess.CalledProcessError:
        raise RuntimeError("Could not read live Claude process arguments") from None
    argv = shlex.split(command)
    require(argv[0] == str(cli) and argv[1] == "--settings",
            "Live process did not use the installed CLI and leading private settings")
    if expected:
        require("--resume" in argv and argv[argv.index("--resume") + 1] == expected
                and "--fork-session" not in argv, "Live process did not resume the exact ID")
    else:
        require("--resume" not in argv, "Fresh CLI process unexpectedly used --resume")
    return Path(argv[2])


def exchange(root, prompt):
    for transcript in (root / "claude/projects").glob("*/*.jsonl"):
        asked = None
        observed = []
        for line in transcript.read_bytes().splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            content = event.get("message", {}).get("content", "")
            if isinstance(content, list):
                content = " ".join(block.get("text", "") for block in content
                                   if isinstance(block, dict) and block.get("type") == "text")
            if event.get("type") == "user" and isinstance(content, str):
                if content.strip() == prompt:
                    asked = event.get("sessionId")
                    observed = []
                elif asked:
                    break
            if (asked and event.get("type") == "assistant" and content.strip()
                    and event.get("sessionId") == asked):
                observed.append(content.strip())
        if observed:
            return asked, observed[-1], transcript
    return None


class Card:
    def __init__(self, host, session_id, key):
        self.host = host
        self.id = session_id
        self.key = key
        self.events = []
        self.screen = ""
        self.sent = set()
        self.diagnostics = []
        self.recovery = []

    def wait(self, label, proof, timeout=30):
        ends = time.monotonic() + timeout
        while True:
            def observe(_, event):
                encoded = json.dumps(event, ensure_ascii=False).encode()
                require(self.key not in encoded and not identity.AUTH_HEADER.search(encoded),
                        "Sensitive bytes appeared in runtime output")
                self.events.extend(item for item in event.get("Graph", {}).get(
                    "providerIdentityEvents", []) if item["sessionId"] == self.id)
                if "Diagnostic" in event:
                    self.diagnostics.append(event["Diagnostic"])
                recovery = event.get("Recovery", {})
                if recovery.get("session_id") == self.id:
                    self.recovery.append(recovery["state"])
                frame = event.get("Frame", {})
                if frame.get("session_id") == self.id:
                    self.screen = " ".join(" ".join(launch.terminal_lines(frame)).split())
                    if match := re.search(r"API Error[^\d]{0,12}(400|401|403|404|429|500|502|503)",
                                          self.screen):
                        raise RuntimeError(f"Real CLI reported provider HTTP {match[1]}")
                return proof()
            if (result := proof()) is not None:
                return result
            try:
                return self.host.wait(label, observe, timeout=min(1, max(0, ends - time.monotonic())))
            except RuntimeError as error:
                if str(error) != f"Timed out waiting for {label}" or time.monotonic() >= ends:
                    raise

    def input(self, data):
        identity.input_bytes(self.host, self.id, data)

    def interactive(self):
        def ready():
            screen = self.screen
            for marker, action in (("❯ No (recommended)", b"\x1b[A"),
                                   ("❯ Yes", b"\r"),
                                   ("❯ No, exit", b"\x1b[B"),
                                   ("❯ Yes, I trust this folder", b"\r")):
                if marker == "❯ Yes" and "Detected a custom API key" not in screen:
                    continue
                if marker in screen and marker not in self.sent:
                    self.input(action)
                    self.sent.add(marker)
                    return None
            if "Claude Code" in screen and "❯" in screen and not any(
                marker in screen for marker in ("Quick safety check", "Detected a custom API key",
                                                "Resume this conversation?")
            ):
                return True
        self.wait("interactive CLI prompt", ready, timeout=25)

    def ask(self, root, prompt, expected_reply):
        self.input(prompt.encode())
        self.wait("prompt in the CLI input field",
                  lambda: True if "".join(prompt[:15].split()) in "".join(self.screen.split()) else None,
                  timeout=8)
        self.input(b"\r")
        def answered():
            response = exchange(root, prompt)
            if response and response[1] == expected_reply:
                provider_id = response[0]
                row = binding(root, self.id, provider_id)
                if row and json.loads(row["metadata_json"]).get("lastHookEvent") == "Stop":
                    return response
        return self.wait("exact model reply and Stop hook", answered, timeout=65)

    def event(self, result, run=None):
        return next((event for event in self.events if event["result"] == result
                     and (run is None or event.get("runId") == run)), None)


def running(root, card, ordinal):
    return card.wait("live Claude process", lambda: next((
        run for run in rows(root, "session_runs", card.id)
        if run["ordinal"] == ordinal and run["status"] == "running"), None))


def parked(root, card, provider_id, expected, run=None):
    event = card.wait("parked provider restore",
                      lambda: card.event("restore-failed", run), timeout=25)
    row = binding(root, card.id, provider_id)
    require(row and row["resume_state"] == row["restore_state"] == "failed"
            and row["invalidated_at"] is not None
            and row["restore_error"] == event["restoreError"],
            "Parked binding did not preserve the failure fields")
    if expected is not None:
        require(row["restore_error"] == expected, "Restore reason differed from the frozen reason")
    return row


def exited(root, card, ordinal):
    run = card.wait("run exit persisted", lambda: next((
        run for run in rows(root, "session_runs", card.id)
        if run["ordinal"] == ordinal and run["status"] == "exited"), None))
    try:
        os.kill(run["pid"], 0)
    except ProcessLookupError:
        return run
    raise RuntimeError("Parked provider PTY process was still alive")


def post_hook(endpoint, event_name, provider_id, cwd, *, statusline=False):
    require(endpoint.hostname == "127.0.0.1", "Hook settings did not use loopback")
    endpoint_path = endpoint.path + ("?kind=statusline" if statusline else "")
    payload = dict(hook_event_name=event_name, session_id=provider_id, cwd=str(cwd))
    connection = HTTPConnection(endpoint.hostname, endpoint.port, timeout=4)
    try:
        connection.request("POST", endpoint_path, json.dumps(payload),
                           {"Content-Type": "application/json"})
        require(connection.getresponse().status == 200, "Hook did not accept the test payload")
    finally:
        connection.close()


def verify(root, env, guard, binary, cli, config, key, proof):
    cwd = root / "workspace/session-cwd"
    cwd.mkdir(parents=True)
    (root / "matou").mkdir()
    onboarding = {"hasCompletedOnboarding": True, "theme": "dark",
                  "projects": {str(cwd): {"hasTrustDialogAccepted": True}}}
    for path in (root / "home/.claude.json", root / "claude/.claude.json"):
        launch.write_json(path, onboarding)
    session_id = launch.seed(root, env, guard, binary, cwd)
    config_path = identity.config_file(root, config)
    try:
        token = secrets.token_hex(8).upper()
        first_prompt = f"记住代号 {token}，只回答 OK。不要调用工具。"
        with launch.runtime(root, env, guard, binary) as host:
            require(host.ready() == session_id, "Runtime opened a different card")
            card = Card(host, session_id, key)
            card.interactive()
            first_run = running(root, card, 1)
            assert_argv(first_run, cli, None)
            old_id, _, transcript = card.ask(root, first_prompt, "OK")
            old = binding(root, session_id, old_id)
            require(old and old["resume_state"] == "available" and old["validated_at"],
                    "First real reply did not establish an available binding")
            host.shutdown()
        require(rows(root, "session_runs", session_id)[0]["status"] == "exited",
                "Normal runtime shutdown did not finish the original run")
        proof["B3"] = {"provider_session_id": old_id, "binding_id": old["id"],
                       "run_id": first_run["id"], "reply": "OK", "resume_state": "available",
                       "runtime_shutdown": True}

        with launch.runtime(root, env, guard, binary) as host:
            require(host.ready() == session_id, "Runtime opened a different card after restart")
            card = Card(host, session_id, key)
            second = running(root, card, 2)
            assert_argv(second, cli, old_id)
            card.interactive()
            confirmed_before_input = card.event("confirmed", second["id"]) is not None
            reply_id, reply_text, same_file = card.ask(root, READBACK, token)
            event = card.wait("current-run hook confirms exact ID",
                              lambda: card.event("confirmed", second["id"]))
            require(second["runtime_generation"] != first_run["runtime_generation"]
                    and event["providerSessionId"] == reply_id == old_id
                    and event["restoreState"] == "none" and same_file == transcript
                    and binding(root, session_id, old_id)["id"] == old["id"],
                    "The readback was not attributed to the original binding and transcript")
            proof["B4"] = {"provider_session_id": old_id, "run_id": second["id"],
                           "generations_differ": True, "argv_resume_id": old_id,
                           "first_authoritative_hook": event["metadata"]["lastHookEvent"],
                           "confirmed_before_readback_input": confirmed_before_input}
            proof["B5"] = {"sent_token": token, "actual_reply": reply_text,
                           "same_transcript": True, "same_binding": True}
            host.shutdown()

        quarantine = root / "removed-from-cli-search"
        quarantine.mkdir()
        for name in ("projects", "sessions"):
            source = root / "claude" / name
            if source.exists():
                source.rename(quarantine / name)
        require(not transcript.exists(), "Original transcript was still readable under the CLI root")
        with launch.runtime(root, env, guard, binary) as host:
            host.ready()
            card = Card(host, session_id, key)
            failure = parked(root, card, old_id, None)
            proof["missing"] = {"binding_id": failure["id"], "restore_error": failure["restore_error"],
                                "invalidated": True}
            try:
                failed_run = exited(root, card, 3)
            except RuntimeError:
                proof["missing"]["runs"] = [{field: run[field] for field in
                                               ("ordinal", "status", "exit_code")}
                                              for run in rows(root, "session_runs", session_id)]
                proof["missing"]["diagnostics"] = card.diagnostics
                raise
            proof["missing"]["exit_code"] = failed_run["exit_code"]
            require(failure["restore_error"] == MISSING, "Missing transcript did not yield the frozen CLI reason")
            host.shutdown()

        with launch.runtime(root, env, guard, binary) as host:
            host.ready()
            card = Card(host, session_id, key)
            host.command("RestartSession", {"session_id": session_id})
            host.command("RetryRecovery", {"session_id": session_id})
            host.command("Refresh")
            try:
                card.wait("no auto restart from a parked binding",
                          lambda: True if len(rows(root, "session_runs", session_id)) != 3 else None,
                          timeout=2)
            except RuntimeError as error:
                require(str(error) == "Timed out waiting for no auto restart from a parked binding",
                        "Runtime failed while checking the parked binding")
            else:
                raise RuntimeError("Parked binding launched without explicit provider action")
            proof["missing"]["no_automatic_or_generic_retry"] = True
            for _ in range(2):
                host.command("RetryProviderRestore", {"session_id": session_id})
            retry = card.wait("explicit same-ID retry", lambda: card.event("retrying"))
            require(retry["providerSessionId"] == old_id
                    and retry["spawnRevision"] == revision(failure) + 1,
                    "Provider retry used a different ID or revision")
            rejected = card.wait("retry parked again", lambda: next((
                event for event in card.events if event["result"] == "restore-failed"
                and event["spawnRevision"] == retry["spawnRevision"]), None))
            require(rejected["providerSessionId"] == old_id and rejected["restoreError"] == MISSING,
                    "Provider retry did not fail on the same missing history")
            try:
                old_retry = exited(root, card, 4)
            except RuntimeError:
                proof["retry"] = {
                    "runs": [{field: run[field] for field in ("ordinal", "status", "exit_code")}
                             for run in rows(root, "session_runs", session_id)],
                    "diagnostics": card.diagnostics,
                }
                raise
            require(len(rows(root, "session_runs", session_id)) == 4
                    and rejected["runId"] == old_retry["id"], "Duplicate retry created another run")
            proof["retry"] = {"provider_session_id": old_id, "run_id": old_retry["id"],
                              "spawn_revision": retry["spawnRevision"], "runs_added": 1}
            for _ in range(2):
                host.command("StartFreshProviderSession", {"session_id": session_id})
            card.screen = ""
            card.interactive()
            fresh = running(root, card, 5)
            assert_argv(fresh, cli, None)
            fresh_prompt = "只回答 OK。不要调用工具。"
            fresh_id, _, _ = card.ask(root, fresh_prompt, "OK")
            expired = binding(root, session_id, old_id)
            require(fresh_id != old_id and expired["resume_state"] == "expired"
                    and expired["invalidated_at"] and expired["restore_state"] == "none"
                    and expired["restore_error"] is None and revision(expired) == retry["spawnRevision"] + 1,
                    "Fresh launch did not explicitly expire the old binding and use a new ID")
            proof["fresh"] = {"run_id": fresh["id"], "provider_session_id": fresh_id,
                              "old_resume_state": "expired", "old_invalidated": True,
                              "spawn_revision": revision(expired), "argv_has_resume": False}
            host.shutdown()

        launch.write_json(root / "claude/settings.json", {"disableAllHooks": True})
        pre_mismatch_state = identity.session_state(root, session_id)
        with launch.runtime(root, env, guard, binary) as host:
            host.ready()
            card = Card(host, session_id, key)
            mismatch_run = running(root, card, 6)
            settings = assert_argv(mismatch_run, cli, fresh_id)
            hook = json.loads(settings.read_bytes())["hooks"]["SessionStart"][0]["hooks"][0]["url"]
            endpoint = urlparse(hook)
            post_hook(endpoint, "SessionEnd", fresh_id, cwd)
            post_hook(endpoint, "SessionStart", fresh_id, cwd, statusline=True)
            timed_out = card.wait("10-second restore readiness release", lambda: (
                card.event("timed-out", mismatch_run["id"]) if "Ready" in card.recovery else None
            ), timeout=15)
            require(timed_out["restoreState"] == "restoring"
                    and timed_out["providerSessionId"] == fresh_id
                    and card.event("confirmed", mismatch_run["id"]) is None
                    and identity.session_state(root, session_id) == pre_mismatch_state,
                    "Timeout or non-authoritative hooks opened the identity gate")
            proof["timeout"] = {"queue_released": True, "restore_state": "restoring",
                                "statusline_and_session_end_unconfirmed": True,
                                "summary_work_and_binding_count_preserved": True}
            wrong_id = str(uuid4())
            post_hook(endpoint, "SessionStart", wrong_id, cwd)
            failed = parked(root, card, fresh_id, MISMATCH, mismatch_run["id"])
            event = card.wait("explicit mismatch result",
                              lambda: card.event("rejected", mismatch_run["id"]))
            require(event["reason"] == "identity-mismatch"
                    and binding(root, session_id, wrong_id) is None,
                    "Mismatch was accepted as another conversation")
            exited(root, card, 6)
            proof["mismatch"] = {"provider_session_id_preserved": failed["provider_session_id"],
                                 "live_pty_stopped": True, "wrong_id_written": False,
                                 "restore_error": failed["restore_error"]}
            host.shutdown()

        missing_executable = root / "missing-claude"
        broken_env = dict(env, MATOU_CLAUDE_COMMAND=str(missing_executable))
        with launch.runtime(root, broken_env, guard, binary) as host:
            host.ready()
            card = Card(host, session_id, key)
            host.command("RetryProviderRestore", {"session_id": session_id})
            failed = parked(root, card, fresh_id, None)
            problem = failed["restore_error"]
            require(problem.startswith("provider process could not start: ")
                    and str(missing_executable) in problem
                    and len(rows(root, "session_runs", session_id)) == 6,
                    "Missing executable did not preserve the real launch error")
            proof["start_failure"] = {"restore_error": problem.replace(str(root), "<isolated-root>"),
                                      "original_provider_id": fresh_id, "no_run_created": True}
            host.shutdown()
    finally:
        identity.check_secret(root, config_path, key, proof)
        proof["credential_absent_from_artifacts"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--evidence", type=Path, default=launch.CHECKS / "claude-resume.json")
    args = parser.parse_args()
    require(sys.platform == "darwin", "Real restore verification requires macOS sandbox-exec")
    cli, binary, evidence = args.claude.resolve(strict=True), args.runtime.resolve(strict=True), args.evidence.resolve()
    require(evidence.is_relative_to(launch.REPO), "Evidence must stay inside the workspace")
    launch.CHECKS.mkdir(parents=True, exist_ok=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve()
    config, key = identity.provider_config(home)
    proof = {"network_policy": "Sandbox outbound is loopback-only. The local test proxy allows "
               "only api.deepseek.com:443 via CONNECT; DNS is resolved by the host proxy. "
               "Real Claude files and Keychain access are denied.", "sandbox_probe": {}}
    with tempfile.TemporaryDirectory(prefix="claude-resume-", dir=launch.CHECKS) as temporary:
        root = Path(temporary)
        try:
            guard = launch.sandbox(root, home)
            launch.probe_sandbox(root, home, cli, guard, proof["sandbox_probe"])
            with endpoint_proxy() as port:
                env = identity.environment(root, cli)
                proxy = f"http://127.0.0.1:{port}"
                env.update({name: proxy for name in ("HTTPS_PROXY", "https_proxy", "HTTP_PROXY",
                                                     "http_proxy", "ALL_PROXY", "all_proxy")})
                env.update(NO_PROXY="127.0.0.1,localhost", no_proxy="127.0.0.1,localhost")
                probe_network(root, env, guard, port)
                proof["network_denial_probes"] = ["sandbox blocked direct external socket",
                                                   "proxy denied a non-DeepSeek CONNECT"]
                version = subprocess.check_output([*guard, str(cli), "--version"], env=env, cwd=root,
                                                  text=True, timeout=10, stderr=subprocess.DEVNULL).strip()
                require(re.fullmatch(r"[\d.]+ \(Claude Code\)", version), "Unexpected CLI version response")
                proof["cli_version"] = version
                verify(root, env, guard, binary, cli, config, key, proof)
            proof["passed"] = True
        except Exception as error:
            message = str(error) if isinstance(error, RuntimeError) else type(error).__name__
            proof["error"] = identity.AUTH_HEADER.sub(
                b"<redacted>", message.encode().replace(key, b"<redacted>")).decode()
            proof["passed"] = False
    proof["isolated_root_removed"] = not root.exists()
    launch.write_json(evidence, proof)
    print(f"Resume evidence: {evidence}", flush=True)
    if not proof["passed"]:
        print(proof["error"], file=sys.stderr, flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
