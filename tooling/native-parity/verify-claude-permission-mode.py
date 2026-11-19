#!/usr/bin/env python3
"""Verify fresh or resumed permission changes in real Claude Code using DeepSeek."""

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
    confirmed = confirmations.get(current["id"], {})
    provider_id = confirmed.get("providerSessionId")
    require(provider_id and confirmed.get("bindingId")
            and confirmed.get("restoreState") == "none"
            and confirmed["metadata"]["lastHookEvent"] == "SessionStart",
            "The replacement did not confirm a conversation and binding at startup")
    if source:
        require(provider_id == source["provider_session_id"]
                and confirmed["bindingId"] == source["id"],
                "The replacement changed the original conversation or binding")
    resume.assert_argv(current, cli, source["provider_session_id"] if source else None)
    argv = shlex.split(subprocess.check_output(
        ["/bin/ps", "-ww", "-p", str(current["pid"]), "-o", "args="],
        text=True, timeout=5,
    ))
    actual_flags = []
    actual_resume = []
    for index, argument in enumerate(argv):
        if argument == "--permission-mode":
            actual_flags.extend(argv[index:index + 2])
        elif argument in ("--dangerously-skip-permissions", "--allow-dangerously-skip-permissions"):
            actual_flags.append(argument)
        elif argument == "--resume":
            actual_resume.extend(argv[index:index + 2])
    require(actual_flags == flags, f"Unexpected permission arguments for {mode}: {actual_flags}")
    require(actual_resume == (["--resume", provider_id] if source else []),
            f"Unexpected resume arguments for {mode}")
    bindings = resume.rows(root, "provider_bindings", session_id)
    require(len(bindings) == 1 and bindings[0]["id"] == confirmed["bindingId"]
            and bindings[0]["provider_session_id"] == provider_id
            and bindings[0]["resume_state"] == "available"
            and bindings[0]["validated_at"] is not None and bindings[0]["invalidated_at"] is None,
            "The confirmed identity did not become the sole available binding")
    metadata = json.loads(bindings[0]["metadata_json"])
    require(metadata["permissionMode"] == node["permissionMode"] == mode,
            f"Binding and graph did not retain requested {mode} permission")
    require(node.get("model") == metadata["model"], "Projected model did not match the binding")
    proof = {
        "mode": mode, "permission_argv": actual_flags, "resume_argv": actual_resume,
        "run_id": current["id"], "previous_run_exited": True,
        "binding_id": confirmed["bindingId"], "provider_session_id": provider_id,
        "identity_confirmed": True, "model": node["model"],
    }
    if mode == "bypassPermissions":
        require(consent["submitted"], "First bypass switch did not exercise the native CLI consent")
        proof["cli_consent"] = {"user_keys": ["Down", "Enter"], "waited_seconds": 11}
    return card, current, proof


def verify(root, env, guard, binary, cli, config, key, proof, fresh):
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
        if fresh:
            require(not resume.rows(root, "provider_bindings", session_id),
                    "The initial Claude card already has a binding")
            proof.update(initial_run_id=first["id"], initial_binding_count=0)
            card, plan_run, plan_proof = switch(
                root, host, cli, session_id, None, first, "plan", ["--permission-mode", "plan"], key)
            proof["switches"].append(plan_proof)
            provider_id = plan_proof["provider_session_id"]
            source = resume.binding(root, session_id, provider_id)
            require(source, "Fresh binding vanished before the default switch")
            print("plan: fresh argv, exit order and new binding verified", flush=True)
            card.interactive()
            prompt_id, reply, transcript = card.ask(
                root, f"记住代号 {token}，只回答 OK。不要调用工具。", "OK")
            require(prompt_id == provider_id, "DeepSeek reply did not use the fresh binding")
            plan_proof["deepseek_reply"] = reply
            card, _, default_proof = switch(
                root, host, cli, session_id, source, plan_run, "default", [], key)
            proof["switches"].append(default_proof)
            card.interactive(allow_scrolled_banner=True)
            reply_id, reply, same_file = card.ask(root, resume.READBACK, token)
            require(reply_id == provider_id and same_file == transcript,
                    "Readback did not come from the fresh session's transcript")
            default_proof["readback"] = {"expected": token, "reply": reply, "same_transcript": True}
            print("default: exact resume argv, binding and DeepSeek readback verified", flush=True)
        else:
            verify_bound(root, host, cli, session_id, card, first, token, key, proof)
        host.shutdown()
    identity.check_secret(root, config_path, key, [proof])


def verify_bound(root, host, cli, session_id, card, first, token, key, proof):
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--fresh", action="store_true",
                        help="Start without a binding, switch to plan, then resume with default")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    require(sys.platform == "darwin", "Real permission verification requires macOS sandbox-exec")
    cli = args.claude.resolve(strict=True)
    binary = args.runtime.resolve(strict=True)
    evidence = (args.evidence or launch.CHECKS / (
        "claude-permission-fresh.json" if args.fresh else "claude-permission-mode.json")).resolve()
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
                verify(root, env, guard, binary, cli, config, key, proof, args.fresh)
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
