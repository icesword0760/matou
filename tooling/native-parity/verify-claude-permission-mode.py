#!/usr/bin/env python3
"""Verify permission changes in one real Claude Code conversation using DeepSeek."""

import argparse
import importlib.util
import json
from pathlib import Path
import re
import secrets
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "native_resume_check", Path(__file__).with_name("verify-claude-resume.py")
)
resume = importlib.util.module_from_spec(spec)
spec.loader.exec_module(resume)
identity = resume.identity
launch = resume.launch
require = launch.require


def switch(root, host, cli, session_id, source, prior_run, mode, flags, key):
    host.command("SetProviderPermissionMode", {"session_id": session_id, "mode": mode})
    card = resume.Card(host, session_id, key)
    confirmations = {}
    consent = {"ready": False, "selected": False, "submitted": False}

    def observe(_, event):
        encoded = json.dumps(event, ensure_ascii=False).encode()
        require(key not in encoded and not identity.AUTH_HEADER.search(encoded),
                "Sensitive bytes appeared in runtime output")
        frame = event.get("Frame", {})
        if frame.get("session_id") == session_id:
            card.screen = " ".join(" ".join(launch.terminal_lines(frame)).split())
        recovery = event.get("Recovery", {})
        if recovery.get("session_id") == session_id and recovery.get("state") == "Ready":
            consent["ready"] = True
        if mode == "bypassPermissions" and consent["ready"]:
            if "❯ No, exit" in card.screen and "Yes, I accept" in card.screen and not consent["selected"]:
                # This disposable profile has no prior consent. Leave the native
                # dialog open past the normal 10-second identity timeout.
                time.sleep(11)
                card.input(b"\x1b[B")
                consent["selected"] = True
            elif "❯ Yes, I accept" in card.screen and consent["selected"] and not consent["submitted"]:
                card.input(b"\r")
                consent["submitted"] = True
        graph = event.get("Graph", {})
        for identity_event in graph.get("providerIdentityEvents", []):
            if identity_event["sessionId"] == session_id and identity_event["result"] == "confirmed":
                confirmations[identity_event["runId"]] = identity_event
        for result in graph.get("providerPermissionModeResults", []):
            if result["sessionId"] == session_id and result["permissionMode"] == mode:
                require("error" not in result, result.get("error"))
                return next(node for node in graph["nodes"] if node["sessionId"] == session_id)

    node = host.wait(f"confirmed {mode} switch", observe, timeout=40)
    runs = resume.rows(root, "session_runs", session_id)
    current = runs[-1]
    require(len(runs) == prior_run["ordinal"] + 1 and runs[-2]["id"] == prior_run["id"]
            and runs[-2]["status"] == "exited" and runs[-2]["ended_at"] <= current["started_at"]
            and current["status"] == "running", "Exactly one new run must follow the persisted exit")
    provider_id = source["provider_session_id"]
    confirmed = confirmations.get(current["id"], {})
    require(confirmed.get("providerSessionId") == provider_id
            and confirmed.get("bindingId") == source["id"]
            and confirmed.get("restoreState") == "none"
            and confirmed["metadata"]["lastHookEvent"] == "SessionStart",
            "The replacement did not confirm the original conversation and binding")
    resume.assert_argv(current, cli, provider_id)
    argv = shlex.split(subprocess.check_output(
        ["/bin/ps", "-ww", "-p", str(current["pid"]), "-o", "args="],
        text=True, timeout=5,
    ))
    actual_flags = []
    for index, argument in enumerate(argv):
        if argument == "--permission-mode":
            actual_flags.extend(argv[index:index + 2])
        elif argument in ("--dangerously-skip-permissions", "--allow-dangerously-skip-permissions"):
            actual_flags.append(argument)
    require(actual_flags == flags, f"Unexpected permission arguments for {mode}: {actual_flags}")
    bindings = resume.rows(root, "provider_bindings", session_id)
    require(len(bindings) == 1 and bindings[0]["id"] == source["id"]
            and bindings[0]["provider_session_id"] == provider_id,
            "Permission switch changed the number or identity of the bindings")
    metadata = json.loads(bindings[0]["metadata_json"])
    require(metadata["permissionMode"] == node["permissionMode"] == mode,
            f"Binding and graph did not retain requested {mode} permission")
    require(node.get("model") == metadata["model"], "Projected model did not match the binding")
    proof = {
        "mode": mode, "permission_argv": actual_flags, "resume_argv": ["--resume", provider_id],
        "run_id": current["id"], "previous_run_exited": True,
        "binding_id": source["id"], "identity_confirmed": True, "model": node["model"],
    }
    if mode == "bypassPermissions":
        require(consent["submitted"], "First bypass switch did not exercise the native CLI consent")
        proof["cli_consent"] = {"user_keys": ["Down", "Enter"], "waited_seconds": 11}
    return card, current, proof


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
    token = secrets.token_hex(8).upper()
    proof.update(session_id=session_id, switches=[])
    with launch.runtime(root, env, guard, binary) as host:
        require(host.ready() == session_id, "Runtime opened a different card")
        card = resume.Card(host, session_id, key)
        card.interactive()
        first = resume.running(root, card, 1)
        resume.assert_argv(first, cli, None)
        provider_id, _, transcript = card.ask(root, f"记住代号 {token}，只回答 OK。不要调用工具。", "OK")
        source = resume.binding(root, session_id, provider_id)
        require(source and source["resume_state"] == "available", "First reply produced no binding")
        proof.update(provider_session_id=provider_id, initial_run_id=first["id"], initial_reply="OK")
        for mode, flags in (
            ("plan", ["--permission-mode", "plan"]),
            ("bypassPermissions", ["--dangerously-skip-permissions"]),
            ("default", []),
        ):
            card, first, result = switch(root, host, cli, session_id, source, first, mode, flags, key)
            proof["switches"].append(result)
            print(f"{mode}: argv, exit order and identity verified", flush=True)
            if mode == "plan":
                card.interactive(allow_scrolled_banner=True)
                reply_id, reply, same_file = card.ask(root, resume.READBACK, token)
                require(reply_id == provider_id and same_file == transcript,
                        "Readback did not come from the original transcript")
                result["readback"] = {"expected": token, "reply": reply, "same_transcript": True}
        host.shutdown()
    identity.check_secret(root, config_path, key, [proof])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--evidence", type=Path, default=launch.CHECKS / "claude-permission-mode.json")
    args = parser.parse_args()
    require(sys.platform == "darwin", "Real permission verification requires macOS sandbox-exec")
    cli = args.claude.resolve(strict=True)
    binary = args.runtime.resolve(strict=True)
    evidence = args.evidence.resolve()
    require(evidence.is_relative_to(launch.REPO), "Evidence must stay inside the workspace")
    launch.CHECKS.mkdir(parents=True, exist_ok=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve()
    config, key = identity.provider_config(home)
    proof = {"sandbox_probe": {}}
    with tempfile.TemporaryDirectory(prefix="permission-mode-", dir=launch.CHECKS) as temporary:
        root = Path(temporary)
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
                proof["network_policy"] = "Loopback-only sandbox with a DeepSeek-only TLS tunnel"
                version = subprocess.check_output([*guard, str(cli), "--version"], env=env, cwd=root,
                                                  text=True, timeout=10, stderr=subprocess.DEVNULL).strip()
                require(re.fullmatch(r"[\d.]+ \(Claude Code\)", version), "Unexpected CLI version")
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
    print(f"Permission switch evidence: {evidence}", flush=True)
    if not proof["passed"]:
        sys.exit(proof["error"])


if __name__ == "__main__":
    main()
