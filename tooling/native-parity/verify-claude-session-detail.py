#!/usr/bin/env python3
"""Compare native history pages with a real, isolated CLI transcript."""
import argparse
from collections import Counter
from datetime import datetime
import importlib.util
import json
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import traceback
from pathlib import Path
from uuid import uuid4

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "native_resume_check", Path(__file__).with_name("verify-claude-resume.py")
)
resume = importlib.util.module_from_spec(spec)
spec.loader.exec_module(resume)
identity, launch, require = resume.identity, resume.launch, resume.require


def seed(cwd, env, guard, cli, provider, key):
    provider_id = str(uuid4())
    token = secrets.token_hex(8).upper()
    (cwd / "preview-marker.txt").write_text(f"{token}\n", encoding="utf-8")
    prompt = ("Use the Read tool to read preview-marker.txt, then reply with only the "
              "exact token from that file. Do not guess the token.")
    cli_env = dict(env, ANTHROPIC_BASE_URL=provider["endpoint"],
                   ANTHROPIC_AUTH_TOKEN=key.decode())
    for name in ("CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION"):
        cli_env.pop(name, None)
    child = subprocess.run(
        [*guard, str(cli), "--print", "--output-format", "json",
         "--session-id", provider_id, "--model", provider["model"],
         "--permission-mode", "default", "--tools=Read", "--allowedTools=Read", prompt],
        cwd=cwd, env=cli_env, capture_output=True, timeout=100, check=False,
    )
    require(key not in child.stdout + child.stderr
            and not identity.AUTH_HEADER.search(child.stdout + child.stderr),
            "Sensitive bytes appeared in CLI output")
    try:
        result = json.loads(child.stdout)
    except ValueError:
        result = {}
    require(child.returncode == 0 and result.get("session_id") == provider_id
            and result.get("result", "").strip() == token,
            f"Real CLI did not read back the token (exit {child.returncode})")
    return provider_id, prompt, token


def read_transcript(source):
    """Build expected rows from the CLI file, independent of the native response."""
    rows = [json.loads(line) for line in source.splitlines() if line.strip()]
    events = []
    message_shapes = Counter()
    cli_row_types = Counter()
    for line, row in enumerate(rows, 1):
        cli_row_types[row.get("type", "<missing>")] += 1
        message = row.get("message")
        if not isinstance(message, dict):
            continue
        content = message.get("content")
        role = message.get("role")
        parts, tools = [], []
        if isinstance(content, str):
            message_shapes["string"] += 1
            parts.append(content)
        elif isinstance(content, list):
            for block in content:
                if not isinstance(block, dict):
                    continue
                block_type = block.get("type")
                message_shapes[block_type or "<missing>"] += 1
                if block_type == "text" and isinstance(block.get("text"), str):
                    parts.append(block["text"])
                if block_type == "tool_use" and isinstance(block.get("name"), str):
                    tools.append(block["name"])
                    tool_input = compact_json(block["input"]) if "input" in block else "undefined"
                    parts.append(f"{block['name']} {tool_input}".strip())
                if block_type == "tool_result":
                    tools.append("tool_result")
                    if "content" in block:
                        result = block["content"]
                        parts.append(result if isinstance(result, str) else compact_json(result))
        text = "\n".join(filter(None, parts)).strip()
        if not text:
            continue
        role = role if role in ("user", "assistant") else None
        event = {"index": len(events) + 1, "kind": "tool" if tools else (role or "system"),
                 "text": text, "matched": False}
        if role:
            event["role"] = role
        if tools and tools[0]:
            event["toolName"] = tools[0]
        date = row.get("timestamp")
        if isinstance(date, str):
            event["timestamp"] = round(datetime.fromisoformat(date).timestamp() * 1000)
        elif isinstance(date, (int, float)):
            event["timestamp"] = date
        events.append((line, event))
    return events, cli_row_types, message_shapes


def compact_json(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def request(host, cwd, provider_id, **options):
    host.command("GetProviderSession", {"cwd": str(cwd), "provider_session_id": provider_id,
                                       **options})
    return host.wait("provider session detail", lambda _, event: next((
        detail for detail in event.get("Graph", {}).get("providerSessionDetailResults", [])
        if detail["providerSessionId"] == provider_id and detail["cwd"] == str(cwd)
    ), None))


def verify(root, env, guard, binary, cli, provider, key, proof):
    cwd = root / "workspace/会话 cwd"
    cwd.mkdir(parents=True)
    (root / "matou").mkdir()
    onboarding = {"hasCompletedOnboarding": True, "theme": "dark",
                  "projects": {str(cwd): {"hasTrustDialogAccepted": True}}}
    for path in (root / "home/.claude.json", root / "claude/.claude.json"):
        launch.write_json(path, onboarding)
    provider_id, prompt, token = seed(cwd, env, guard, cli, provider, key)
    path, = (root / "claude/projects").glob(f"*/{provider_id}.jsonl")
    before = path.stat()
    source = path.read_bytes()
    rows, cli_row_types, message_shapes = read_transcript(source)
    events = [event for _, event in rows]
    proof.update(provider_session_id=provider_id, cli_lines=len(source.splitlines()),
                 cli_row_types=cli_row_types, message_shapes=message_shapes, total=len(events))
    markers = [
        next((event["index"] for event in events if event.get("role") == "user"
              and event["text"] == prompt), 0),
        next((event["index"] for event in events if event.get("role") == "assistant"
              and event.get("toolName") == "Read" and "preview-marker.txt" in event["text"]), 0),
        next((event["index"] for event in events if event.get("role") == "user"
              and event.get("toolName") == "tool_result" and token in event["text"]), 0),
        next((event["index"] for event in events if event.get("role") == "assistant"
              and event["text"] == token), 0),
    ]
    require(all(markers) and markers == sorted(set(markers)),
            "CLI transcript did not contain the ordered Read exchange")

    def check(result, start, end):
        require(result.get("events") == events[start - 1:end - 1],
                "Runtime events differed from the CLI transcript")
        page = {"startEventIndex": start, "endEventIndex": end, "total": len(events),
                "hasEarlier": start > 1, "hasLater": end <= len(events)}
        require(result.get("page") == page, "Runtime returned unexpected page boundaries")
        return page

    with launch.runtime(root, env, guard, binary) as host:
        host.ready()
        all_rows = request(host, cwd, provider_id)
        proof["whole_page"] = check(all_rows, 1, len(events) + 1)
        proof["recent_page"] = check(request(host, cwd, provider_id, limit=2),
                                     len(events) - 1, len(events) + 1)
        proof["earlier_page"] = check(request(host, cwd, provider_id, start_event_index=1,
                                               limit=len(events) - 2), 1, len(events) - 1)
        require(request(host, cwd, provider_id) == all_rows, "Event indices changed on re-read")
        wrong_cwd = request(host, root / "workspace", provider_id)
        require("not found" in wrong_cwd.get("error", "") and "events" not in wrong_cwd,
                "A request for a different cwd returned events")
        host.shutdown()
    after = path.stat()
    require(before.st_mtime_ns == after.st_mtime_ns and source == path.read_bytes(),
            "Runtime changed the provider transcript")
    proof["verification"] = {"repeat_equal": True, "other_cwd_rejected": True,
                             "content_and_mtime_unchanged": True}
    proof["transcript_samples"] = [
        {"cli_line": line, **event} for line, event in rows if event["index"] in markers
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--evidence", type=Path, default=launch.CHECKS / "claude-session-detail.json")
    args = parser.parse_args()
    require(sys.platform == "darwin", "Real verification requires macOS sandbox-exec")
    cli, binary = args.claude.resolve(strict=True), args.runtime.resolve(strict=True)
    evidence = args.evidence.resolve()
    require(evidence.is_relative_to(launch.REPO), "Evidence must stay inside the workspace")
    launch.CHECKS.mkdir(parents=True, exist_ok=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve()
    config, key = identity.provider_config(home)
    provider, = config["providers"]["claude-code"]
    proof = {"sandbox_probe": {}, "outbound_policy": "loopback proxy to api.deepseek.com:443 only"}
    with tempfile.TemporaryDirectory(prefix="claude-detail-", dir=launch.CHECKS) as directory:
        root = Path(directory)
        try:
            guard = launch.sandbox(root, home)
            launch.probe_sandbox(root, home, cli, guard, proof["sandbox_probe"])
            with resume.endpoint_proxy() as port:
                env = identity.environment(root, cli)
                proxy = f"http://127.0.0.1:{port}"
                env.update({name: proxy for name in ("HTTPS_PROXY", "https_proxy", "HTTP_PROXY",
                                                     "http_proxy", "ALL_PROXY", "all_proxy")})
                env.update(NO_PROXY="127.0.0.1,localhost", no_proxy="127.0.0.1,localhost")
                resume.probe_network(root, env, guard, port)
                version = subprocess.check_output([*guard, str(cli), "--version"], env=env, cwd=root,
                                                  text=True, timeout=10, stderr=subprocess.DEVNULL)
                require(re.fullmatch(r"[\d.]+ \(Claude Code\)", version.strip()), "Unexpected CLI version")
                proof["cli_version"] = version.strip()
                verify(root, env, guard, binary, cli, provider, key, proof)
            proof["passed"] = True
        except Exception as error:
            frame = traceback.extract_tb(error.__traceback__)[-1]
            message = str(error) if isinstance(error, RuntimeError) else (
                f"{type(error).__name__} at {Path(frame.filename).name}:{frame.lineno}"
            )
            proof.update(passed=False, error=identity.AUTH_HEADER.sub(
                b"<redacted>", message.encode().replace(key, b"<redacted>")).decode())
        try:
            identity.check_secret(root, None, key, proof)
            require(not identity.AUTH_HEADER.search(json.dumps(proof).encode()),
                    "Authorization header appeared in evidence")
            proof["credential_absent_from_artifacts"] = True
        except RuntimeError as error:
            proof = {"passed": False, "error": str(error)}
    proof["isolated_root_removed"] = not root.exists()
    launch.write_json(evidence, proof)
    print(f"Session detail evidence: {evidence}", flush=True)
    if not proof["passed"]:
        sys.exit(proof["error"])


if __name__ == "__main__":
    main()
