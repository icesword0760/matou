#!/usr/bin/env python3
"""Exercise a real Claude CLI with two disposable Anthropic-compatible loopback servers."""
import argparse
import contextlib
import hashlib
import http.server
import json
import os
from pathlib import Path
import queue
import sqlite3
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
MODEL = "probe-shared-model"
KEYS = {"a": "synthetic-phase-one-key-a", "b": "synthetic-phase-one-key-b"}


def events(lane, payload):
    message = {
        "id": f"msg_probe_{lane}", "type": "message", "role": "assistant", "model": MODEL,
        "content": [], "stop_reason": None, "stop_sequence": None,
        "usage": {"input_tokens": 2, "output_tokens": 1},
    }
    text = f"loopback-reply-{lane}"
    if not payload.get("stream"):
        message.update(content=[{"type": "text", "text": text}], stop_reason="end_turn")
        return "application/json", json.dumps(message).encode()
    frames = [
        ("message_start", {"message": message}),
        ("content_block_start", {"index": 0, "content_block": {"type": "text", "text": ""}}),
        ("content_block_delta", {"index": 0, "delta": {"type": "text_delta", "text": text}}),
        ("content_block_stop", {"index": 0}),
        ("message_delta", {"delta": {"stop_reason": "end_turn", "stop_sequence": None},
                           "usage": {"output_tokens": 4}}),
        ("message_stop", {}),
    ]
    return "text/event-stream", "".join(
        f"event: {name}\ndata: {json.dumps({'type': name, **data})}\n\n"
        for name, data in frames
    ).encode()


@contextlib.contextmanager
def upstream(lane, observed):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def reply(self, status, mime, body):
            self.send_response(status)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            self.reply(200, "application/json", b"{}")

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))) or b"{}")
            path = self.path.split("?", 1)[0]
            if path.endswith("/count_tokens"):
                self.reply(200, "application/json", b'{"input_tokens":2}')
                return
            if not path.endswith("/v1/messages"):
                self.reply(200, "application/json", b"{}")
                return
            credential = self.headers.get("x-api-key", "")
            correct_key = credential == KEYS[lane] and not self.headers.get("authorization")
            if not correct_key or body.get("model") != MODEL:
                self.reply(400, "application/json", b'{"type":"error","error":{"type":"invalid_request_error","message":"unexpected fixture route"}}')
                observed.put({"lane": lane, "path": path, "route_valid": False})
                return
            observed.put({
                "lane": lane, "path": path, "model": MODEL, "route_valid": True,
                "key_fingerprint": hashlib.sha256(credential.encode()).hexdigest()[:12],
                "stream": body.get("stream") is True,
            })
            mime, response = events(lane, body)
            self.reply(200, mime, response)

    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}/{lane}"
        finally:
            server.shutdown()
            thread.join()


class Runtime:
    def __init__(self, runtime, cli, root):
        self.updated = queue.Queue()
        self.frame = ""
        self.home = root / "home"
        self.home.mkdir(exist_ok=True)
        self.config = self.home / "claude"
        self.config.mkdir(exist_ok=True)
        self.workspace = root / "workspace"
        self.data = root / "data"
        # No inherited CLI account, token, config, shell startup files, or proxy.
        proxy = "http://127.0.0.1:1"
        self.environment = {
            "HOME": str(self.home), "USER": "gateway-probe", "TERM": "xterm-256color",
            "TMPDIR": str(root), "CLAUDE_CONFIG_DIR": str(self.config),
            "PATH": f"{cli.parent}:/usr/bin:/bin:/usr/sbin:/sbin",
            "MATOU_CLAUDE_COMMAND": str(cli), "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "DISABLE_AUTOUPDATER": "1", "DISABLE_TELEMETRY": "1",
            "HTTP_PROXY": proxy, "HTTPS_PROXY": proxy, "ALL_PROXY": proxy,
            "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
        }
        self.child = subprocess.Popen(
            [str(runtime), "--data-root", str(self.data), "--workspace", str(self.workspace),
             "--shell-home", str(self.home), "--shell", str(root / "missing-shell")],
            env=self.environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        threading.Thread(target=self.read_messages, daemon=True).start()
        try:
            self.send({"Hello": {"version": 1}})
            self.until(lambda event: "Ready" in event, 12)
        except BaseException:
            self.close()
            raise

    def read_messages(self):
        assert self.child.stdout is not None
        for line in self.child.stdout:
            # Never persist terminal contents or raw runtime messages.
            for secret in KEYS.values():
                if secret.encode() in line:
                    self.updated.put({"probe_failure": "Synthetic upstream key leaked into runtime event"})
                    return
            self.updated.put(json.loads(line).get("Event", {}))
        self.updated.put({"probe_failure": "Native runtime pipe closed"})

    def send(self, message):
        assert self.child.stdin is not None
        self.child.stdin.write((json.dumps(message) + "\n").encode())
        self.child.stdin.flush()

    def command(self, name, **fields):
        self.send({"Command": {name: fields} if fields else name})

    def until(self, condition, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                event = self.updated.get(timeout=max(0.01, deadline - time.monotonic()))
            except queue.Empty:
                break
            if isinstance(event, dict) and "probe_failure" in event:
                raise RuntimeError(event["probe_failure"])
            if isinstance(event, dict) and "Frame" in event:
                frame = event["Frame"]
                cells, cols = frame["screen"]["cells"], frame["screen"]["cols"]
                self.frame = "\n".join(
                    "".join(cell["text"] for cell in cells[i:i + cols]).rstrip()
                    for i in range(0, len(cells), cols)
                )
                self.command("Applied", session_id=frame["session_id"], sequence=frame["sequence"])
            if condition(event):
                return event
        raise RuntimeError("timed out waiting for real CLI milestone")

    def input(self, session, data):
        self.command("Input", session_id=session, bytes=list(data.encode()), defer_ordering=False)

    def submit(self, session, text):
        self.input(session, text)
        time.sleep(0.15)
        self.input(session, "\r")

    def close(self):
        try:
            if self.child.poll() is None:
                self.command("Shutdown")
            self.child.wait(timeout=12)
        except subprocess.TimeoutExpired:
            self.child.kill()
            self.child.wait(timeout=3)
        assert self.child.stderr is not None
        stderr = self.child.stderr.read()
        if any(key.encode() in stderr for key in KEYS.values()):
            raise RuntimeError("synthetic upstream key leaked into runtime stderr")
        if self.child.returncode != 0:
            raise RuntimeError("native runtime closed unsuccessfully")


def latest_run(db, session):
    return db.execute(
        "SELECT id,pid,runtime_generation FROM session_runs WHERE session_id=? ORDER BY ordinal DESC LIMIT 1",
        (session,),
    ).fetchone()


def probe(runtime_path, cli, root):
    observed = queue.Queue()
    with upstream("a", observed) as a, upstream("b", observed) as b:
        seed = Runtime(runtime_path, cli, root)
        seed.close()
        with contextlib.closing(sqlite3.connect(seed.data / "matou.sqlite")) as db:
            session = db.execute("SELECT id FROM sessions WHERE archived_at IS NULL").fetchone()[0]
            providers = [
                {"id": lane, "endpoint": endpoint, "model": "probe-source-opus", "apiKey": KEYS[lane],
                 "modelMapping": {"opus": "probe-source-opus", "haiku": MODEL}}
                for lane, endpoint in (("a", a), ("b", b))
            ]
            (seed.data / "provider-config.json").write_text(json.dumps({
                "version": 1, "activeProviderIds": {"claude-code": "a"},
                "providers": {"claude-code": providers},
            }))
            preferences = {
                "contextWindows": {}, "compaction": {"mode": "follow", "effort": "follow"},
                "claudeRoleAssignments": {
                    "opus": {"providerId": "a", "model": MODEL},
                    "sonnet": {"providerId": "b", "model": MODEL},
                },
            }
            db.execute("UPDATE sessions SET kind='claude-code' WHERE id=?", (session,))
            db.execute("INSERT INTO preferences VALUES ('agent.modelPreferences', ?, 1)",
                       (json.dumps(preferences),))
            db.commit()
            config = {"hasCompletedOnboarding": True, "projects": {
                str(seed.workspace): {"hasTrustDialogAccepted": True, "hasCompletedProjectOnboarding": True}
            }}
            (seed.home / ".claude.json").write_text(json.dumps(config))
            (seed.config / ".claude.json").write_text(json.dumps(config))
            version = subprocess.check_output([str(cli), "--version"], env=seed.environment, text=True,
                                              timeout=15).strip()
            runtime = Runtime(runtime_path, cli, root)
            try:
                runtime.until(lambda _event: "bypass permissions" in runtime.frame.lower()
                              or "for shortcuts" in runtime.frame.lower()
                              or "ctrl+" in runtime.frame.lower()
                              or "accept" in runtime.frame.lower()
                              or "custom API key" in runtime.frame
                              or "Claude Code" in runtime.frame, 25)
                api_key_prompt = "custom API key" in runtime.frame
                if api_key_prompt:
                    runtime.input(session, "\x1b[A")
                    runtime.input(session, "\r")
                    runtime.until(lambda _event: "custom API key" not in runtime.frame, 12)
                if "trust" in runtime.frame.lower():
                    runtime.input(session, "\r")
                    runtime.until(lambda _event: "trust" not in runtime.frame.lower(), 12)
                runtime.until(lambda _event: "❯" in runtime.frame and "Claude Code" in runtime.frame, 15)
                before = latest_run(db, session)
                runtime.submit(session, "Reply with the fixture response for turn one.")
                runtime.until(lambda _event: "loopback-reply-a" in runtime.frame, 25)
                runtime.submit(session, "/model sonnet")
                def switched():
                    screen = runtime.frame.lower()
                    return "set model" in screen or "switched to" in screen

                runtime.until(lambda _event: switched() or "Switch model?" in runtime.frame, 15)
                model_switch_prompt = "Switch model?" in runtime.frame
                if model_switch_prompt:
                    runtime.input(session, "\r")
                    runtime.until(lambda _event: switched() or "Switch model?" not in runtime.frame, 15)
                runtime.submit(session, "Reply with the fixture response for turn two.")
                runtime.until(lambda _event: "loopback-reply-b" in runtime.frame, 25)
                after = latest_run(db, session)
                if before is None or before != after:
                    raise RuntimeError("CLI PID, native run ID or generation changed")
                route = list(observed.queue)
                position = next(i for i, row in enumerate(route) if row["lane"] == "a" and row.get("stream"))
                next(row for row in route[position + 1:] if row["lane"] == "b" and row.get("stream"))
                if not all(row["route_valid"] for row in route):
                    raise RuntimeError("a fake upstream received an unexpected route")
                return {"cli_version": version, "same_pid": before[1], "same_run": before[0],
                        "same_generation": before[2], "model_change": "/model sonnet",
                        "cli_api_key_confirmation": api_key_prompt,
                        "cli_model_switch_confirmation": model_switch_prompt,
                        "assistant_replies": ["loopback-reply-a", "loopback-reply-b"],
                        "upstream_requests": route}
            except Exception:
                # Report progress without printing the CLI's private URL or its frame.
                print(json.dumps({"cli_version": version, "requests": list(observed.queue),
                                  "screen_flags": {hint: hint.lower() in runtime.frame.lower()
                                                   for hint in ("Claude Code", "trust", "onboarding", "Sonnet", "loopback-reply-a",
                                                                "loopback-reply-b", "invalid", "Error")}},
                                 ensure_ascii=False))
                raise
            finally:
                runtime.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--runtime", type=Path, default=ROOT / "apps/native/target/debug/matou-native-runtime")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(dir=args.runtime.resolve().parent) as directory:
        print(json.dumps(probe(args.runtime.resolve(), args.cli.absolute(), Path(directory)),
                         ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
