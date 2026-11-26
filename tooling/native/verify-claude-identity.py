#!/usr/bin/env python3
"""Verify CLI hook identity and a disabled-hook control in isolated macOS roots."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlparse
from uuid import UUID

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "native_launch_check", Path(__file__).with_name("verify-claude-launch.py")
)
launch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launch)

PROVIDER_ID = "4f828414-9c9d-4b00-9aa9-9c6a1e034e58"
ENDPOINT = "https://api.deepseek.com/anthropic"
PROMPT = "Reply with only OK. Do not call tools."
AUTH_HEADER = re.compile(rb"(?i)(?:authorization|x-api-key)[\"']?\s*:\s*(?:[\"']\s*)?(?:bearer[ \t]+)?[a-z0-9_./-]{12,}")


def provider_config(home):
    config = json.loads((home / ".matou/provider-config.json").read_bytes())
    provider, = (entry for entry in config["providers"]["claude-code"]
                 if entry.get("id") == PROVIDER_ID)
    launch.require(provider.get("name") == "DeepSeek" and provider.get("endpoint") == ENDPOINT,
                   "Authorized provider record did not match")
    key = provider.get("apiKey")
    launch.require(isinstance(key, str) and len(key) >= 12, "Authorized provider has no API key")
    result = {
        "version": 1, "revision": 1, "providers": {"claude-code": [provider]},
        "activeProviderIds": {"claude-code": PROVIDER_ID},
    }
    return result, key.encode()


def config_file(root, config):
    path = root / "matou/provider-config.json"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as file:
        json.dump(config, file)
    launch.require(stat.S_IMODE(path.stat().st_mode) == 0o600, "Provider config was not private")
    return path


def environment(root, cli):
    env = launch.isolated_environment(root, cli, cli)
    for name in ("HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"):
        proxy = os.environ.get(name)
        if proxy:
            parsed = urlparse(proxy)
            launch.require(parsed.hostname in {"127.0.0.1", "localhost"}
                           and not parsed.username and not parsed.password,
                           "Harness requires a local proxy without credentials")
            env[name] = proxy
    for name in ("NO_PROXY", "no_proxy"):
        bypass = os.environ.get(name, "")
        env[name] = ",".join(filter(None, (bypass, "127.0.0.1", "localhost")))
    env["CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL"] = "1"
    env["DISABLE_AUTOUPDATER"] = "1"
    return env


def session_state(root, session_id):
    with launch.database(root) as db:
        row = db.execute(
            """SELECT cwd, work_status, (SELECT latest_lines_json FROM session_graph_summaries
                  WHERE session_id=sessions.id) FROM sessions WHERE id=?""",
            (session_id,),
        ).fetchone()
        count = db.execute("SELECT COUNT(*) FROM provider_bindings WHERE session_id=?",
                           (session_id,)).fetchone()[0]
        return (*row, count)


def provider_events(root, session_id):
    with launch.database(root) as db:
        return [
            (event, json.loads(payload))
            for event, payload in db.execute(
                "SELECT event_type, payload_json FROM domain_events WHERE session_id=? ORDER BY seq",
                (session_id,),
            )
            if event in {"provider-binding.recorded", "session.mode-changed", "session.cwd-updated",
                         "session.work-status-changed", "agent.notification"}
        ]


def cli_exchange(root, facts=None):
    # Read only the isolated CLI's own records as an independent cross-check.
    for file in (root / "claude/projects").glob("*/*.jsonl"):
        conversation = None
        for line in file.read_bytes().splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            message = event.get("message", {})
            content = message.get("content", "")
            if isinstance(content, list):
                text = " ".join(block.get("text", "") for block in content
                                if isinstance(block, dict) and block.get("type") == "text")
            else:
                text = content
            if event.get("type") == "user" and text.strip() == PROMPT:
                conversation = event.get("sessionId")
                if facts is not None:
                    facts["prompt_in_cli_records"] = True
            if (conversation and event.get("type") == "assistant" and text.strip()
                    and event.get("sessionId") == conversation):
                if facts is not None:
                    facts["assistant_text_in_cli_records"] = True
                if text.strip().rstrip(".!。！").casefold() == "ok":
                    UUID(conversation)
                    return conversation
    return None


def inspect_message(event, session_id, observations, key):
    encoded = json.dumps(event, ensure_ascii=False).encode()
    launch.require(key not in encoded and not AUTH_HEADER.search(encoded),
                   "Sensitive bytes appeared in the runtime event stream")
    graph = event.get("Graph", {})
    observations.extend(item for item in graph.get("providerIdentityEvents", [])
                        if item.get("sessionId") == session_id)
    recovery = event.get("Recovery", {})
    if recovery.get("session_id") == session_id:
        state = recovery.get("state")
        if isinstance(state, dict):
            raise RuntimeError("Runtime reported a provider startup failure")
    frame = event.get("Frame", {})
    if frame.get("session_id") != session_id:
        return ""
    rendered = " ".join(" ".join(launch.terminal_lines(frame)).split())
    rendered_bytes = rendered.encode()
    launch.require(key not in rendered_bytes and not AUTH_HEADER.search(rendered_bytes),
                   "Sensitive bytes appeared in the rendered CLI screen")
    for pattern, failure in [
        (r"API Error[^\d]{0,12}(?:HTTP\s*)?(400|401|403|404|429|500|502|503)", "HTTP"),
        (r"(?i)(?:authentication failed|invalid api key|not logged in)", "authentication"),
        (r"(?i)(?:could not resolve host|getaddrinfo|unable to connect to API|ECONNREFUSED|ETIMEDOUT)",
         "connection"),
    ]:
        match = re.search(pattern, rendered)
        if match:
            status = f" {match.group(1)}" if match.lastindex else ""
            raise RuntimeError(f"Real CLI reported provider {failure}{status}")
    return rendered


def input_bytes(host, session_id, data):
    host.command("Input", {"session_id": session_id, "bytes": list(data),
                           "defer_ordering": False})


def check_secret(root, config_path, key, observations):
    launch.require(key not in json.dumps(observations, ensure_ascii=False).encode(),
                   "Credential appeared in the selected evidence")
    for file in root.rglob("*"):
        if file != config_path and not file.is_symlink() and file.is_file():
            data = file.read_bytes()
            header = AUTH_HEADER.search(data)
            kind = "credential" if key in data else "Authorization header" if header else None
            if kind:
                parts = file.relative_to(root).parts
                location = "/".join(parts[:2]) if len(parts) > 2 else parts[0]
                raise RuntimeError(f"Isolated artifact under {location} contained a {kind}")


def run_case(root, cli, binary, guard, config, key, disabled, proof):
    cwd = root / "workspace/session-cwd"
    cwd.mkdir(parents=True)
    (root / "matou").mkdir()
    env = environment(root, cli)
    launch.write_json(root / "claude/settings.json", {"disableAllHooks": disabled})
    onboarding = {
        "hasCompletedOnboarding": True, "theme": "dark",
        "projects": {str(cwd): {"hasTrustDialogAccepted": True}},
    }
    for path in (root / "home/.claude.json", root / "claude/.claude.json"):
        launch.write_json(path, onboarding)
    session_id = launch.seed(root, env, guard, binary, cwd)
    config_path = config_file(root, config)
    with launch.database(root) as db:
        db.execute("UPDATE sessions SET work_status='needs-input' WHERE id=?", (session_id,))
        db.execute("INSERT INTO session_graph_summaries(session_id,latest_lines_json,updated_at)"
                   " VALUES (?, ?, 1)", (session_id, '["isolated prior summary"]'))
    before = session_state(root, session_id)
    prior_events = provider_events(root, session_id)
    observations = []
    with launch.runtime(root, env, guard, binary) as host:
        launch.require(host.ready() == session_id, "Runtime opened a different card")
        trust_selection_requested = False
        trust_sent = False
        key_selection_requested = False
        key_accepted = False
        screen_state = {}

        def ready(_, event):
            nonlocal trust_selection_requested, trust_sent, key_selection_requested, key_accepted
            screen = inspect_message(event, session_id, observations, key)
            if screen:
                screen_state.update({
                    "claude_header": "Claude Code" in screen,
                    "input_indicator": "❯" in screen,
                    "shortcuts": "shortcuts" in screen,
                    "key_approval_prompt": "Detected a custom API key" in screen,
                    "yes_is_selected": "❯ Yes" in screen,
                    "workspace_trust_prompt": "Quick safety check" in screen,
                    "login_selection": "Select login method" in screen,
                    "rendered_character_count": len(screen),
                })
            if "Detected a custom API key" in screen:
                if "❯ No (recommended)" in screen and not key_selection_requested:
                    input_bytes(host, session_id, b"\x1b[A")
                    key_selection_requested = True
                if "❯ Yes" in screen and not key_accepted:
                    input_bytes(host, session_id, b"\r")
                    key_accepted = True
                return None
            if "❯ No, exit" in screen and not trust_selection_requested:
                input_bytes(host, session_id, b"\x1b[B")
                trust_selection_requested = True
                return None
            if "❯ Yes, I trust this folder" in screen and not trust_sent:
                input_bytes(host, session_id, b"\r")
                trust_sent = True
                return None
            if "Claude Code" in screen and "❯" in screen and "Quick safety check" not in screen:
                return True
            return None

        try:
            host.wait("interactive prompt", ready, timeout=20)
        except RuntimeError:
            proof["trust_selection_requested"] = trust_selection_requested
            proof["isolated_directory_trust_confirmed"] = trust_sent
            proof["provider_key_approval_requested"] = key_selection_requested
            proof["provider_key_approval_sent"] = key_accepted
            proof["screen_state"] = screen_state
            with launch.database(root) as db:
                proof["runs"] = db.execute(
                    "SELECT status, exit_code FROM session_runs WHERE session_id=?", (session_id,)
                ).fetchall()
            proof["isolated_key_approval_saved"] = any(
                json.loads(path.read_bytes()).get("customApiKeyResponses", {}).get("approved")
                for path in (root / "home/.claude.json", root / "claude/.claude.json")
                if path.is_file()
            )
            raise
        proof["interactive_prompt"] = True
        launch.require(session_state(root, session_id) == before
                       and provider_events(root, session_id) == prior_events,
                       "Provider altered derived state before identity")
        proof["pre_confirmation_state_unchanged"] = True
        hook_dir = root / "matou/provider-hooks"
        settings, = hook_dir.glob("*.settings.json")
        with launch.database(root) as db:
            run_id, pid = db.execute(
                "SELECT id, pid FROM session_runs WHERE session_id=? AND status='running'",
                (session_id,),
            ).fetchone()
        argv = subprocess.check_output(
            ["/bin/ps", "-ww", "-p", str(pid), "-o", "args="], timeout=5, text=True
        ).strip()
        launch.require(argv.startswith(f"{cli} --settings {settings}"),
                       "Running executable omitted leading private settings")
        proof["real_cli_with_private_settings"] = True
        input_bytes(host, session_id, PROMPT.encode())
        host.wait("prompt text in the input field", lambda _, event: (
            True if PROMPT in inspect_message(event, session_id, observations, key) else None
        ))
        input_bytes(host, session_id, b"\r")

        def answered(_, event):
            inspect_message(event, session_id, observations, key)
            conversation = cli_exchange(root)
            if not conversation:
                return None
            if disabled:
                return conversation if any(item["result"] == "timed-out" for item in observations) else None
            if any(event == "provider-binding.recorded" and data["metadata"]["observedNormalStop"]
                   for event, data in provider_events(root, session_id)):
                return conversation
            return None

        try:
            reply_label = "real reply and expected hook outcome"
            deadline = time.monotonic() + 65
            while True:
                try:
                    conversation = host.wait(reply_label, answered, timeout=2)
                    break
                except RuntimeError as error:
                    if str(error) != f"Timed out waiting for {reply_label}" or time.monotonic() >= deadline:
                        raise
                    host.command("Refresh")
        except RuntimeError:
            proof["cli_recorded_reply"] = cli_exchange(root, proof) is not None
            proof["identity_results"] = [
                {k: value for k, value in item.items() if k in {"result", "reason"}}
                for item in observations
            ]
            proof["lifecycle_events"] = [
                data["metadata"]["lastHookEvent"] for event, data in provider_events(root, session_id)
                if event == "provider-binding.recorded"
            ]
            check_secret(root, config_path, key, proof)
            raise
        proof["cli_recorded_reply"] = True
        proof["cli_session_id"] = conversation
        if disabled:
            launch.require(session_state(root, session_id) == before
                           and provider_events(root, session_id) == prior_events,
                           "Disabled hooks altered binding or derived state")
            launch.require(not any(item["result"] == "confirmed" for item in observations),
                           "Disabled hooks reported identity confirmation")
            proof.update(bindings=0, identity_timed_out=True, derived_state_unchanged=True)
        else:
            confirmed, = (item for item in observations if item["result"] == "confirmed")
            launch.require(confirmed["runId"] == run_id and confirmed["provider"] == "claude-code"
                           and confirmed["providerSessionId"] == conversation,
                           "Desktop event did not identify the CLI's conversation and run")
            events = [data["metadata"]["lastHookEvent"] for event, data in provider_events(root, session_id)
                      if event == "provider-binding.recorded"]
            launch.require("UserPromptSubmit" in events and "Stop" in events,
                           "Real CLI did not report both UserPromptSubmit and Stop")
            with launch.database(root) as db:
                provider_id, state, validated, invalidated, restore, error, metadata = db.execute(
                    """SELECT provider_session_id, resume_state, validated_at, invalidated_at,
                              restore_state, restore_error, metadata_json FROM provider_bindings
                       WHERE session_id=? AND provider='claude-code'""", (session_id,)
                ).fetchone()
            fields = json.loads(metadata)
            launch.require(
                provider_id == conversation and state == "available" and isinstance(validated, int)
                and invalidated is None and restore == "none" and error is None
                and fields["providerConfigId"] == PROVIDER_ID and fields["cwd"] == str(cwd)
                and fields["observedUserPrompt"] is True and fields["observedNormalStop"] is True,
                "Binding flags, metadata or CLI identity differed",
            )
            proof.update(binding_provider_session_id=provider_id, resume_state=state, validated_at=validated,
                         desktop_confirmation={
                             "run_id": run_id, "provider": confirmed["provider"],
                             "provider_session_id": confirmed["providerSessionId"],
                             "lifecycle_event": confirmed["metadata"]["lastHookEvent"],
                         },
                         prompt_observed=True, stop_observed=True, lifecycle_events=events, bindings=1)
        for _ in range(2):
            input_bytes(host, session_id, b"\x03")
            time.sleep(0.25)

        def cleaned(_, event):
            inspect_message(event, session_id, observations, key)
            return True if not any(hook_dir.iterdir()) else None

        deadline = time.monotonic() + 10
        while any(hook_dir.iterdir()) and time.monotonic() < deadline:
            host.command("Refresh")
            host.wait("render during retirement", lambda _, event: (
                cleaned(None, event) or bool(event.get("Frame")) or None), timeout=5)
            time.sleep(0.15)
        launch.require(not any(hook_dir.iterdir()), "Retired run's private files remained")
        host.shutdown()
        proof["hook_directory_empty"] = True
        check_secret(root, config_path, key, proof)
        proof["credential_absent_from_artifacts"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--evidence", type=Path, default=launch.CHECKS / "claude-identity.json")
    args = parser.parse_args()
    launch.require(sys.platform == "darwin", "The real-CLI harness requires macOS sandbox-exec")
    cli = args.claude.resolve(strict=True)
    binary = args.runtime.resolve(strict=True)
    evidence = args.evidence.resolve()
    launch.require(evidence.is_relative_to(launch.REPO), "Evidence must stay in the workspace")
    launch.CHECKS.mkdir(parents=True, exist_ok=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve()
    config, key = provider_config(home)
    proof = {
        "network_policy": "network-outbound for the authorized API, local HTTP proxy and DNS; "
                          "listener bind and inbound restricted to loopback; "
                          "official marketplace auto-install and updates disabled",
        "sandbox_probe": {},
        "cases": {},
    }
    exit_code = 0
    with tempfile.TemporaryDirectory(prefix="claude-identity-", dir=launch.CHECKS) as directory:
        root = Path(directory)
        guard = launch.sandbox(root, home, outbound=True)
        try:
            launch.probe_sandbox(root, home, cli, guard, proof["sandbox_probe"])
            env = environment(root / "version", cli)
            version = subprocess.check_output(
                [*guard, str(cli), "--version"], text=True, env=env, cwd=root, timeout=10,
                stderr=subprocess.DEVNULL,
            ).strip()
            launch.require(re.fullmatch(r"[\d.]+ \(Claude Code\)", version) is not None,
                           "Unexpected CLI version response")
            proof["cli_version"] = version
            for name, disabled in (("hooks-enabled", False), ("hooks-disabled", True)):
                case = proof["cases"][name] = {}
                run_case(root / name, cli, binary, guard, config, key, disabled, case)
                print(f"{name}: passed", flush=True)
        except Exception as error:
            if isinstance(error, RuntimeError):
                message = str(error).encode().replace(key, b"<redacted>")
                proof["error"] = AUTH_HEADER.sub(b"<redacted>", message).decode()
            else:
                proof["error"] = f"Verification could not finish ({type(error).__name__})"
            exit_code = 1
    proof["isolated_root_removed"] = not root.exists()
    proof["passed"] = exit_code == 0
    launch.write_json(evidence, proof)
    print(f"Identity evidence: {evidence}", flush=True)
    if exit_code:
        print(proof["error"], file=sys.stderr, flush=True)
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
