#!/usr/bin/python3
"""Pinned router contract fixture. All traffic stays on the test loopback."""
import http.server
import json
import os
from pathlib import Path
import sys
import time
import urllib.parse

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
    do_GET = do_POST = do_PATCH = do_PUT = lambda self: self.handle_request()

    def log_message(self, *_args):
        pass

    def response(self, status, body, headers=()):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("x-cpa-version", "8.0.68" if mode == "wrong-version" else "8.0.69")
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        for name, value in headers:
            self.send_header(name, value)
        self.end_headers()
        try:
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
                files = [dict(read_credential(p.name), name=p.name, id=p.name) for p in credentials.glob("*.json")]
                return self.response(200, {"files": files})
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
            return self.response(int(operation), {"type": "authentication_error"})
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
