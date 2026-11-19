#!/usr/bin/env python3
"""Drive Shell promotion through the native runtime with the installed Claude CLI."""

import argparse
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "native_launch_check", Path(__file__).with_name("verify-claude-launch.py")
)
launch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launch)

PROMPT = "MATOU_PROMOTION_SHELL_READY>"


def send_input(host, session_id, data):
    host.command("Input", {"session_id": session_id, "bytes": list(data),
                           "defer_ordering": True})


@contextmanager
def loopback_provider():
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if self.path.split("?")[0] == "/v1/messages":
                events = [
                    ("message_start", {"message": {"id": "msg_fixture", "type": "message",
                                     "role": "assistant", "content": [], "model": "claude-opus-5-5",
                                     "stop_reason": None, "stop_sequence": None,
                                     "usage": {"input_tokens": 1, "output_tokens": 1}}}),
                    ("content_block_start", {"index": 0, "content_block": {"type": "text", "text": ""}}),
                    ("content_block_delta", {"index": 0, "delta": {"type": "text_delta", "text": "OK"}}),
                    ("content_block_stop", {"index": 0}),
                    ("message_delta", {"delta": {"stop_reason": "end_turn", "stop_sequence": None},
                                       "usage": {"output_tokens": 1}}),
                    ("message_stop", {}),
                ]
                body = "".join(f"event: {name}\ndata: {json.dumps(dict(payload, type=name))}\n\n"
                               for name, payload in events).encode()
                self.respond("text/event-stream", body)
            else:
                self.respond("application/json", b'{"input_tokens":1}')

        def do_GET(self):
            self.respond("application/json", b'{"data":[]}')

        def respond(self, mime, body):
            self.send_response(200)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}"
        finally:
            server.shutdown()
            worker.join(timeout=2)


def configure_fixture(root, workspace, endpoint):
    fixture_key = "sk-ant-api03-matou-loopback-fixture"
    onboarding = {"hasCompletedOnboarding": True, "theme": "dark",
                  "customApiKeyResponses": {"approved": [fixture_key[-20:]], "rejected": []},
                  "projects": {str(workspace): {"hasTrustDialogAccepted": True}}}
    for directory in ("home", "claude"):
        launch.write_json(root / directory / ".claude.json", onboarding)
    (root / "matou").mkdir()
    launch.write_json(root / "matou/provider-config.json", {
        "version": 1, "activeProviderIds": {"claude-code": "loopback"},
        "providers": {"claude-code": [{"id": "loopback", "model": "",
                                      "endpoint": endpoint, "apiKey": fixture_key}]},
    })


def verify_identity(host, root, session, run, proof):
    prompt = b"Reply with only OK."
    sent = approved = False

    def inspect(_, event):
        nonlocal sent, approved
        frame = event.get("Frame", {})
        if frame.get("session_id") == session:
            screen = " ".join(launch.terminal_lines(frame))
            if "Detected a custom API key" in screen and not approved:
                if "❯ No (recommended)" in screen:
                    send_input(host, session, b"\x1b[A")
                if "❯ Yes" in screen:
                    send_input(host, session, b"\r")
                    approved = True
            elif "❯" in screen and not sent:
                send_input(host, session, prompt)
                sent = True
            elif sent and prompt.decode() in screen and not proof.get("prompt_submitted"):
                send_input(host, session, b"\r")
                proof["prompt_submitted"] = True
        if proof.get("prompt_submitted"):
            with launch.database(root) as db:
                observed = db.execute(
                    "SELECT payload_json FROM domain_events WHERE session_id=?"
                    " AND event_type='provider-binding.recorded'"
                    " AND json_extract(payload_json, '$.runId')=?"
                    " AND json_extract(payload_json, '$.metadata.observedUserPrompt')=1"
                    " ORDER BY seq LIMIT 1", (session, run),
                ).fetchone()
            if observed:
                return json.loads(observed[0])
        return None

    host.command("Refresh")
    item = host.wait("real CLI hook identity", inspect, timeout=20)
    proof.update(hook_identity=item["providerSessionId"],
                 hook_event=item["metadata"]["lastHookEvent"])


def check_mode(root, cli, binary, guard, bypass, proof, endpoint=None):
    env = launch.isolated_environment(root, cli, cli)
    env.update(DISABLE_AUTOUPDATER="1", CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL="1")
    workspace = root / "workspace"
    workspace.mkdir()
    (root / "missing-shell").symlink_to("/bin/zsh")  # Existing probe's shell override.
    (root / "home/.zshrc").write_text(f"PROMPT='{PROMPT} '\n", encoding="utf-8")
    if endpoint:
        configure_fixture(root, workspace, endpoint)
    with launch.runtime(root, env, guard, binary) as host:
        session = host.ready()
        host.wait("Shell prompt", lambda _, event: (
            True if "Frame" in event and event["Frame"]["session_id"] == session
            and PROMPT in launch.terminal_lines(event["Frame"]) else None
        ))
        with launch.database(root) as db:
            mount, = db.execute("SELECT id FROM session_mounts WHERE session_id=?", (session,)).fetchone()
            shell_pid, = db.execute("SELECT pid FROM session_runs WHERE session_id=?", (session,)).fetchone()
        command = b"claude --dangerously-skip-permissions" if bypass else b"claude"
        send_input(host, session, command + b"\r")

        def upgraded(_, event):
            graph = event.get("Graph", {})
            nodes = graph.get("nodes", [])
            node = next((node for node in nodes if node.get("currentMode") == "claude-code"), None)
            if node is None:
                return None
            launch.require(len(nodes) == 1 and node["sessionId"] == session and node["title"] == "Claude"
                           and graph["focusedSessionId"] == session and node["workStatus"] == "starting",
                           "The upgraded graph changed card identity, focus, or initial state")
            return node

        host.wait("in-place Claude card", upgraded)
        if endpoint:
            def interactive(_, event):
                frame = event.get("Frame", {})
                if frame.get("session_id") != session:
                    return None
                lines = [line for line in launch.terminal_lines(frame) if line]
                proof["last_interactive_frame"] = lines
                return True if any(line.startswith("❯") for line in lines) else None

            host.wait("loopback Claude prompt", interactive, timeout=18)
        else:
            launch.wait_for_cli_screen(host, session, proof)
        with launch.database(root) as db:
            runs = db.execute("SELECT id, pid, profile, status FROM session_runs WHERE session_id=?"
                              " ORDER BY ordinal", (session,)).fetchall()
            launch.require(len(runs) == 2 and runs[0][1:] == (shell_pid, "shell", "exited")
                           and runs[1][2:] == ("claude-code", "running"), f"Unexpected runs: {runs}")
            run, pid = runs[1][:2]
            launch.require(db.execute("SELECT id FROM session_mounts WHERE session_id=?",
                                      (session,)).fetchone() == (mount,), "Card moved to another mount")
        launch.require(subprocess.run(
            ["/bin/ps", "-p", str(shell_pid)], stdout=subprocess.DEVNULL, check=False,
        ).returncode == 1, "Previous Shell process is still alive")
        settings, = (root / "matou/provider-hooks").glob("*.settings.json")
        argv = subprocess.check_output(["/bin/ps", "-ww", "-p", str(pid), "-o", "args="],
                                       text=True, timeout=5).strip()
        permission_args = ["--dangerously-skip-permissions"] if bypass else []
        launch.require(argv == " ".join((str(cli), "--settings", str(settings), *permission_args)),
                       "Running process omitted private hooks or had unexpected CLI arguments")
        lsof = subprocess.check_output(
            ["/usr/sbin/lsof", "-a", "-p", str(pid), "-d", "cwd,txt", "-Fn"],
            text=True, timeout=6, stderr=subprocess.DEVNULL,
        )
        launch.require(f"n{cli}\n" in lsof and f"fcwd\nn{workspace}\n" in lsof,
                       "Started executable or working directory did not match")
        with launch.database(root) as db:
            identity = db.execute("SELECT provider_session_id FROM provider_bindings WHERE session_id=?",
                                  (session,)).fetchone()
        proof.update(session_id=session, same_mount=True, same_focus=True, retired_shell_pid=shell_pid,
                     real_cli_pid=pid, run_id=run, argv=["--settings", "<run-private settings>", *permission_args],
                     hook_identity=identity[0] if identity else None)
        if endpoint:
            verify_identity(host, root, session, run, proof)
        host.shutdown()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--evidence", type=Path, default=launch.TARGET / "shell-promotion-real.json")
    parser.add_argument("--loopback-identity", action="store_true",
                        help="also exercise a real CLI hook with a fixture provider on loopback")
    args = parser.parse_args()
    launch.require(sys.platform == "darwin" and args.claude is not None,
                   "Requires macOS and an installed Claude CLI")
    cli = args.claude.resolve(strict=True)
    binary = args.runtime.resolve(strict=True)
    evidence = args.evidence.resolve()
    launch.require(evidence.is_relative_to(launch.REPO), "Evidence must stay inside the workspace")
    evidence.parent.mkdir(parents=True, exist_ok=True)
    proof = {"executable": str(cli), "cases": {},
             "isolation": "dedicated homes and config, loopback only, user credentials inaccessible"}
    with tempfile.TemporaryDirectory(prefix="shell-promotion-", dir=evidence.parent) as directory:
        root = Path(directory).resolve()
        guard = launch.sandbox(root, Path.home().resolve())
        try:
            launch.probe_sandbox(root, Path.home().resolve(), cli, guard,
                                 proof.setdefault("sandbox_probe", {}))
            env = launch.isolated_environment(root / "version", cli, cli)
            proof["version"] = subprocess.check_output(
                [*guard, str(cli), "--version"], env=env, cwd=root, text=True, timeout=12,
            ).strip()
            for mode, bypass in (("default", False), ("bypassPermissions", True)):
                check_mode(root / mode, cli, binary, guard, bypass, proof["cases"].setdefault(mode, {}))
                print(f"{mode}: same card, real CLI and argv verified", flush=True)
            if args.loopback_identity:
                with loopback_provider() as endpoint:
                    check_mode(root / "loopback-identity", cli, binary, guard, False,
                               proof["cases"].setdefault("loopback-identity", {}), endpoint)
                print("loopback-identity: real CLI hook verified", flush=True)
        except Exception as error:
            proof["error"] = str(error)
            raise
        finally:
            launch.write_json(evidence, proof)
            print(evidence, flush=True)


if __name__ == "__main__":
    main()
