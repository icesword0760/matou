#!/usr/bin/python3
"""Pinned router contract fixture. All traffic stays on the test loopback."""
import gzip
import http.server
import json
import os
from pathlib import Path
import sys
import time
import urllib.parse
import uuid
import zlib

config_path = Path(sys.argv[sys.argv.index("-config") + 1])
config = json.loads(config_path.read_text())
assert sys.argv[-2:] == ["-local-model", "-no-browser"]
assert config["config-version"] == 8 and config["server"]["host"] == "127.0.0.1"
assert config["management"]["allow-remote"] is False
assert config["server"]["commercial-mode"] is True
assert config["routing"]["force-model-prefix"] is True
assert config["routing"]["retry"]["request-retry"] == 0
directory = config_path.parent
assert Path(os.environ["HOME"]) == directory
credentials = Path(config["oauth"]["auth-dir"])
aliases = config["oauth"].get("model-alias", {})
management = config["management"]["secret-key"]
client = config["access"]["api-keys"][0]
assert management != client and len(management) == len(client) == 64
mode_path = directory / "fixture-mode"
mode = mode_path.read_text().strip() if mode_path.exists() else ""
port = config["server"]["port"]
oauth = {}
model_catalog = {
    "codex": ["gpt-6-astra", "gpt-6.1-sol", "gpt-6-luna", "fixture-fable"],
    "antigravity": ["claude-opus-thinking", "claude-fable-5-2", "claude-sonnet", "gemini-flash"],
}



def audit(event, **values):
    line = json.dumps(dict(event=event, pid=os.getpid(), port=port, **values)) + "\n"
    descriptor = os.open(directory / "fixture-events.jsonl", os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
    try:
        os.write(descriptor, line.encode())
    finally:
        os.close(descriptor)


def read_credential(name):
    assert name == Path(name).name
    return json.loads((credentials / name).read_text())


def write_credential(name, credential):
    temporary = credentials / (name + ".tmp")
    temporary.write_text(json.dumps(credential))
    temporary.replace(credentials / name)


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    do_GET = do_POST = do_PATCH = do_PUT = do_DELETE = lambda self: self.handle_request()

    def log_message(self, *_args):
        pass

    def response(self, status, body, headers=()):
        return self.raw_response(status, json.dumps(body).encode(), headers)

    def raw_response(self, status, data, headers=(), chunked=False):
        self.send_response(status)
        self.send_header("x-cpa-version", "8.0.68" if mode == "wrong-version" else "8.0.69")
        self.send_header("Content-Type", "application/json")
        if chunked:
            self.send_header("Transfer-Encoding", "chunked")
        else:
            self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        for name, value in headers:
            self.send_header(name, value)
        self.end_headers()
        try:
            if chunked:
                for offset in range(0, len(data), 32 * 1024):
                    chunk = data[offset:offset + 32 * 1024]
                    self.wfile.write(f"{len(chunk):x}\r\n".encode() + chunk + b"\r\n")
                self.wfile.write(b"0\r\n\r\n")
            else:
                self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def handle_request(self):
        path = urllib.parse.urlsplit(self.path)
        if path.path == "/cdn-cgi/trace":
            country = (directory / "fixture-country").read_text().strip()
            data = ("ip=203.0.113.95\nloc=" + country + "\nc colo=SIN\n").encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"]))) if int(self.headers.get("Content-Length", 0)) else {}
        namespace = "/v8/management/"
        if path.path.startswith(namespace):
            route = path.path[len(namespace):]
            if self.headers.get("Authorization") != "Bearer " + management:
                audit("wrong-management-bearer")
                return self.response(401, {})
            audit("management", route=route, method=self.command)
            if mode == "never-ready":
                return self.response(503, {})
            if route == "credentials":
                if self.command == "DELETE":
                    name = urllib.parse.parse_qs(path.query)["name"][0]
                    assert name == Path(name).name
                    (credentials / name).unlink(missing_ok=True)
                    return self.response(200, {})
                files = [dict(read_credential(p.name), name=p.name, id=p.name) for p in credentials.glob("*.json")]
                return self.response(200, {"files": files})
            if route == "oauth/auth-url":
                provider = urllib.parse.parse_qs(path.query)["provider"][0]
                callback_ports = json.loads((directory / "fixture-oauth-ports").read_text())
                callback_path = "/auth/callback" if provider == "codex" else "/oauth-callback"
                redirect = f"http://localhost:{callback_ports[provider]}{callback_path}"
                state = uuid.uuid4().hex
                oauth[state] = {"status": "wait", "provider": provider}
                base = "https://auth.openai.com/oauth/authorize" if provider == "codex" else "https://accounts.google.com/o/oauth2/v2/auth"
                if mode == "invalid-auth-url":
                    base = "https://accounts.google.com.invalid/o/oauth2/v2/auth"
                params = urllib.parse.urlencode({"redirect_uri": redirect, "state": state})
                return self.response(200, {"state": state, "url": base + "?" + params})
            if route == "oauth/status":
                state = urllib.parse.parse_qs(path.query)["state"][0]
                return self.response(200, oauth.get(state, {"status": "error"}))
            if route == "oauth/session":
                state = urllib.parse.parse_qs(path.query)["state"][0]
                if oauth.get(state, {}).get("status") != "wait":
                    return self.response(200, {"cancelled": False})
                oauth[state]["status"] = "error"
                return self.response(200, {"cancelled": True})
            if route == "oauth/callback":
                record = oauth.get(body["state"], {})
                if record.get("status") != "wait" or record["provider"] != body["provider"]:
                    return self.response(409, {})
                if "error" in body:
                    record.update(status="error", error=body["error"])
                    return self.response(200, {})
                provider = record["provider"]
                code = body["code"]
                if code not in ("first", "second", "ambiguous"):
                    return self.response(400, {})
                names = ["second", "third"] if code == "ambiguous" else [code]
                for name in names:
                    ref = f"{provider}-{name}.json"
                    write_credential(ref, dict(
                        provider=provider, status="active", disabled=False,
                        auth_index=f"{provider}-{name}-index", email=f"{name}@test.invalid",
                        models=model_catalog[provider], updated_at="first-read", project_id="test-project",
                        access_token="fixture-secret-token-never-exposed",
                    ))
                record.update(status="ok")
                if provider == "codex":
                    record["auth_id"] = ref
                return self.response(200, {})
            if route == "requests/api-call":
                account = next((read_credential(p.name) for p in credentials.glob("*.json")
                                if read_credential(p.name)["auth_index"] == body["auth_index"]), None)
                assert body["header"]["Authorization"] == "Bearer $TOKEN$"
                url = body["url"]
                audit("account-probe", route=url.split("?")[0])
                if account is None:
                    return self.response(200, {"status_code": 401, "body": "{}"})
                if url == "https://chatgpt.com/backend-api/wham/usage":
                    assert body["method"] == "GET" and account["provider"] == "codex"
                    assert body["header"]["originator"] == "codex-tui"
                    payload = {"plan_type": "pro", "rate_limit": {"primary_window": {
                        "limit_window_seconds": 18000, "used_percent": 35, "reset_at": 2000000000}}}
                elif url.endswith(":loadCodeAssist"):
                    assert body["method"] == "POST" and account["provider"] == "antigravity"
                    assert json.loads(body["data"]) == {"metadata": {"ideType": "ANTIGRAVITY"}}
                    payload = {"cloudaicompanionProject": {"id": "test-project"}, "paidTier": {"name": "Google AI Pro"}}
                elif url.endswith(":retrieveUserQuotaSummary"):
                    assert json.loads(body["data"]) == {"project": "test-project"}
                    payload = {"groups": [{"buckets": [{"bucketId": "3p-5h", "remainingFraction": 0.25}]}]}
                elif url.startswith("https://chatgpt.com/backend-api/codex/models?client_version=") or url.endswith(":fetchAvailableModels"):
                    kind = directory / "fixture-catalog-mode"
                    catalog = kind.read_text().strip() if kind.exists() else ""
                    if catalog == "failure":
                        return self.response(200, {"status_code": 503, "body": "{}"})
                    if catalog == "unauthorized":
                        return self.response(200, {"status_code": 401, "body": "{}"})
                    models = account["models"]
                    if catalog == "updated":
                        models = [*models, "fixture-updated-model"]
                    payload = {"models": [{"id": model, "context_window": 600000,
                                           "supportedEfforts": ["low", "high"]} for model in models],
                               "fixtureSecret": "fixture-secret-token-never-exposed"}
                else:
                    return self.response(400, {})
                return self.response(200, {"status_code": 200, "body": json.dumps(payload)})
            if route == "credentials/models":
                name = urllib.parse.parse_qs(path.query)["name"][0]
                credential = read_credential(name)
                audit("model-list", name=name)
                prefix = credential.get("prefix", "")
                models = set(credential.get("models", []))
                models.update(item["id"] for item in credential.get("extra_models", []))
                models.update(item["alias"] for item in aliases.get(credential["provider"], []) if item["name"] in models)
                if prefix:
                    models = {f"{prefix}/{model}" for model in models}
                return self.response(200, {"models": [{"id": model} for model in sorted(models)]})
            if route.startswith("routing/model-definitions/"):
                return self.response(200, {"models": []})
            if route in ("credentials/status", "credentials/fields"):
                if self.command != "PATCH":
                    return self.response(405, {})
                name = body.pop("name")
                credential = read_credential(name)
                credential.update(body)
                write_credential(name, credential)
                audit("patch", name=name, fields=sorted(body))
                return self.response(200, {})
            if route.startswith("config/oauth/model-alias/"):
                provider = route.rsplit("/", 1)[1]
                if self.command == "PUT":
                    aliases[provider] = body
                    audit("aliases", provider=provider, aliases=body)
                    return self.response(200, {})
                if provider not in aliases:
                    return self.response(404, {})
                return self.response(200, aliases[provider])
            return self.response(404, {})

        if self.headers.get("Authorization") != "Bearer " + client or self.headers.get("x-api-key"):
            audit("wrong-inference-bearer")
            return self.response(401, {})
        if path.path not in ("/v1/messages", "/v1/messages/count_tokens"):
            return self.response(404, {})
        operation = body.get("fixture", "")
        model = body["model"]
        audit("message", model=model, operation=operation, query=path.query, effort=body.get("output_config"))
        if operation == "retry":
            try:
                (directory / "fixture-retry").touch(exist_ok=False)
            except FileExistsError:
                pass
            else:
                os._exit(31)
        if operation == "rate":
            return self.response(429, {"model": model}, [("Retry-After", "7")])
        if operation in ("401", "403"):
            return self.response(int(operation), {"secret": "upstream-private"}, [("X-Upstream", "private")])
        if operation == "expired":
            return self.response(429, {"error": {"code": "token_expired", "message": "private"}})
        if operation == "expired-gzip":
            data = gzip.compress(b'{"error":{"type":"token_expired","message":"private"}}')
            return self.raw_response(500, data, [("Content-Encoding", "gzip")])
        if operation == "expired-deflate":
            data = zlib.compress(b'{"error":{"message":"Please sign in again"}}')
            return self.raw_response(502, data, [("Content-Encoding", "deflate")])
        if operation == "ordinary-bad-gateway":
            return self.raw_response(502, b'{"error":{"message":"upstream overloaded"}}', [("X-Upstream", "fixture")])
        if operation in ("large", "large-gzip"):
            data = b'{"error":{"code":"token_expired","message":"' + b"x" * (256 * 1024) + b'"}}'
            if operation == "large-gzip":
                encoded = gzip.compress(data)
                (directory / "fixture-large-gzip").write_bytes(encoded)
                return self.raw_response(429, encoded, [("Content-Encoding", "gzip")])
            return self.raw_response(502, data, [("X-Upstream", "fixture")], chunked=True)
        if operation in ("sse-crash", "sse-hold"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", "999")
            self.end_headers()
            self.wfile.write(b"event: message_start\ndata: {}\n\n")
            self.wfile.flush()
            time.sleep(0.15 if operation == "sse-crash" else 15)
            if operation == "sse-crash":
                os._exit(32)
            return
        return self.response(200, {"model": model, "port": port})


audit("engine-start")
if mode == "fail-first-bind" and not (directory / "fixture-bind-attempt").exists():
    (directory / "fixture-bind-attempt").touch()
    os._exit(33)

http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
