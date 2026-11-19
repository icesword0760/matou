#!/usr/bin/env python3
"""Verify native catalog load with real Claude Code and the authorized DeepSeek config."""
import argparse
import hashlib
import importlib.util
import json
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from uuid import uuid4

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "native_resume_check", Path(__file__).with_name("verify-claude-resume.py")
)
resume = importlib.util.module_from_spec(spec)
spec.loader.exec_module(resume)
identity, launch, require = resume.identity, resume.launch, resume.require


def seed_history(root, env, guard, cli, config, key, cwd, token):
    provider = config["providers"]["claude-code"][0]
    prompt = f"请记住本次对话的代号 {token}。只回答 OK，不调用工具。"
    provider_id = str(uuid4())
    seed_env = dict(env, ANTHROPIC_BASE_URL=provider["endpoint"],
                    ANTHROPIC_AUTH_TOKEN=key.decode())
    for name in ("CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION"):
        seed_env.pop(name, None)
    completed = subprocess.run(
        [*guard, str(cli), "--print", "--output-format", "json", "--session-id", provider_id,
         "--model", provider["model"], "--permission-mode", "default", prompt],
        env=seed_env, cwd=cwd, capture_output=True, timeout=70, check=False,
    )
    require(key not in completed.stdout + completed.stderr
            and not identity.AUTH_HEADER.search(completed.stdout + completed.stderr),
            "Sensitive bytes appeared in the CLI seed output")
    try:
        result = json.loads(completed.stdout)
    except ValueError:
        result = {}
    require(completed.returncode == 0 and result.get("session_id") == provider_id
            and result.get("result", "").strip() == "OK",
            f"Real CLI seed did not return OK (exit {completed.returncode})")
    exchange = resume.exchange(root, prompt)
    require(exchange and exchange[:2] == (provider_id, "OK"), "CLI did not persist real history")
    return provider_id, exchange[2]


def install_gate(root, cli):
    """Pause before exec so the test can read the pending row or remove isolated history."""
    gate_dir = root / "gate"
    gate_dir.mkdir()
    shim = root / "exec-claude"
    shim.write_text(
        f"#!{sys.executable}\n"
        "import os, sys, time\n"
        "from pathlib import Path\n"
        "args = sys.argv[1:]\n"
        "target = args[args.index('--resume') + 1]\n"
        f"root = Path({str(gate_dir)!r})\n"
        "(root / (target + '.ready')).touch()\n"
        "deadline = time.monotonic() + 15\n"
        "while not (root / (target + '.release')).is_file():\n"
        "    if time.monotonic() >= deadline: sys.exit(93)\n"
        "    time.sleep(0.01)\n"
        f"os.execv({str(cli)!r}, [{str(cli)!r}, *args])\n",
        encoding="utf-8",
    )
    shim.chmod(0o700)
    return gate_dir, shim


def list_sessions(host, cwd):
    host.command("ListProviderSessions", {"cwd": str(cwd)})
    result = host.wait("provider catalog list", lambda _, event: next((
        result for result in event.get("Graph", {}).get("providerSessionCatalogResults", [])
        if result["cwd"] == str(cwd)
    ), None))
    require("error" not in result, "Runtime returned a catalog list error")
    return result["sessions"]


def load(host, session_id, provider_id):
    host.command("LoadProviderSession", {
        "session_id": session_id, "provider_session_id": provider_id,
    })


def rejected(host, session_id, provider_id):
    return host.wait("catalog load rejection", lambda _, event: next((
        error for error in event.get("Graph", {}).get("providerSessionLoadErrors", [])
        if error["sessionId"] == session_id and error["providerSessionId"] == provider_id
    ), None))


def verify(root, env, guard, binary, cli, config, key, proof):
    cwd = root / "workspace/会话 cwd"
    cwd.mkdir(parents=True)
    (root / "matou").mkdir()
    onboarding = {"hasCompletedOnboarding": True, "theme": "dark",
                  "projects": {str(cwd): {"hasTrustDialogAccepted": True}}}
    for path in (root / "home/.claude.json", root / "claude/.claude.json"):
        launch.write_json(path, onboarding)
    session_id = launch.seed(root, env, guard, binary, cwd)
    with launch.database(root) as db:
        db.execute("UPDATE sessions SET status='exited', work_status='exited' WHERE id=?",
                   (session_id,))
    config_path = identity.config_file(root, config)
    try:
        token = secrets.token_hex(8).upper()
        selected, selected_file = seed_history(root, env, guard, cli, config, key, cwd, token)
        vanishing, vanishing_file = seed_history(
            root, env, guard, cli, config, key, cwd, secrets.token_hex(8).upper())
        proof["seeded_real_conversations"] = 2
        gate, shim = install_gate(root, cli)
        env = dict(env, MATOU_CLAUDE_COMMAND=str(shim))
        fingerprints = {path: hashlib.sha256(path.read_bytes()).digest()
                        for path in (selected_file, vanishing_file)}
        with launch.runtime(root, env, guard, binary) as host:
            require(host.ready() == session_id, "Runtime opened the wrong Claude card")
            entries = list_sessions(host, cwd)
            require({entry["providerSessionId"] for entry in entries} == {selected, vanishing}
                    and all(entry["loadedBy"] is None and entry["title"] and entry["model"]
                            and entry["updatedAt"] > 0 and entry["messageCount"] >= 2
                            for entry in entries), "Catalog did not list both real CLI histories")
            require(all(hashlib.sha256(path.read_bytes()).digest() == fingerprint
                        for path, fingerprint in fingerprints.items()), "Catalog changed CLI history")
            require(not list_sessions(host, root / "workspace"), "Catalog included history from another cwd")
            proof["catalog"] = {"ids": [entry["providerSessionId"] for entry in entries],
                                "transcripts_unchanged": True, "other_cwd_empty": True}
            absent = str(uuid4())
            load(host, session_id, absent)
            error = rejected(host, session_id, absent)
            require("not found" in error["error"] and not resume.rows(root, "provider_bindings", session_id)
                    and not resume.rows(root, "session_runs", session_id),
                    "An ID absent at lookup unexpectedly wrote a binding or started a process")
            proof["missing_at_lookup"] = {"rejected": True, "binding_and_run_count": 0}

            card = resume.Card(host, session_id, key)
            load(host, session_id, vanishing)
            failed_run = resume.running(root, card, 1)
            card.wait("CLI exec gate for disappearing history",
                      lambda: (gate / f"{vanishing}.ready").is_file() or None)
            quarantined = root / "quarantined-cli-history"
            quarantined.mkdir()
            path_in_projects = vanishing_file.relative_to(root / "claude/projects")
            vanishing_file.rename(quarantined / "vanished.jsonl")
            for name in ("projects", "sessions"):
                candidate = root / "claude" / name
                if candidate.exists():
                    candidate.rename(quarantined / name)
            (gate / f"{vanishing}.release").touch()
            try:
                def failed_or_configure_key():
                    if card.event("restore-failed", failed_run["id"]):
                        return True
                    if "Detected a custom API key" in card.screen:
                        for marker, action in (("❯ No (recommended)", b"\x1b[A"), ("❯ Yes", b"\r")):
                            if marker in card.screen and marker not in card.sent:
                                card.input(action)
                                card.sent.add(marker)
                                break
                    if "❯ Yes, I trust this folder" in card.screen and "trust" not in card.sent:
                        card.input(b"\r")
                        card.sent.add("trust")
                card.wait("real CLI rejection of disappearing history", failed_or_configure_key)
                failed = resume.parked(root, card, vanishing, resume.MISSING, failed_run["id"])
                resume.exited(root, card, 1)
            except RuntimeError:
                proof["missing_probe"] = {
                    "identity_results": [event["result"] for event in card.events],
                    "run_statuses": [row["status"] for row in resume.rows(root, "session_runs", session_id)],
                    "key_prompt": "Detected a custom API key" in card.screen,
                    "trust_prompt": "Quick safety check" in card.screen,
                    "missing_prompt": "No conversation found" in card.screen,
                    "interactive_prompt": "Claude Code" in card.screen and "❯" in card.screen,
                }
                raise
            projects = root / "claude/projects"
            projects.mkdir(exist_ok=True)
            require(not (projects / path_in_projects).exists(), "CLI recreated the missing history")
            for file in (quarantined / "projects").rglob("*.jsonl"):
                target = projects / file.relative_to(quarantined / "projects")
                target.parent.mkdir(parents=True, exist_ok=True)
                file.rename(target)
            require(failed["validated_at"] is None, "Disappearing history somehow confirmed")
            proof["missing_at_resume"] = {"provider_session_id": vanishing,
                                          "reason": failed["restore_error"], "worker_exited": True}

            for _ in range(2):
                load(host, session_id, selected)
            resumed = resume.running(root, card, 2)
            card.wait("CLI exec gate for successful history",
                      lambda: (gate / f"{selected}.ready").is_file() or None)
            staged = resume.binding(root, session_id, selected)
            metadata = json.loads(staged["metadata_json"])
            catalog_entry = next(entry for entry in entries if entry["providerSessionId"] == selected)
            require(staged["resume_state"] == "available" and staged["restore_state"] == "restoring"
                    and staged["validated_at"] is None and metadata["loadedFromCatalog"] is True
                    and metadata["model"] == catalog_entry["model"]
                    and metadata["permissionMode"] == catalog_entry["permissionMode"]
                    and metadata["spawnRevision"] == resume.revision(failed) + 1,
                    "The pending binding did not preserve catalog metadata before the hook")
            expired = resume.binding(root, session_id, vanishing)
            require(expired and expired["resume_state"] == "expired",
                    "Switching history did not retain an expired prior binding")
            require(card.event("confirmed", resumed["id"]) is None,
                    "Catalog load reported success before the real CLI was started")
            (gate / f"{selected}.release").touch()
            card.interactive()
            resume.assert_argv(resumed, cli, selected)
            confirmed_before_input = card.event("confirmed", resumed["id"]) is not None
            reply_id, reply, transcript = card.ask(root, resume.READBACK, token)
            confirmed = card.wait("provider identity for the resumed run",
                                  lambda: card.event("confirmed", resumed["id"]))
            require(confirmed["providerSessionId"] == selected and confirmed["restoreState"] == "none"
                    and resume.binding(root, session_id, selected)["validated_at"],
                    "The real CLI did not confirm the requested provider ID")
            require(reply_id == selected and reply == token and transcript == selected_file,
                    "Readback did not come from the selected history")
            load(host, session_id, selected)  # A repeat while the same worker is live is a no-op.
            load(host, session_id, vanishing)
            require("running" in rejected(host, session_id, vanishing)["error"],
                    "A different history was not rejected while the card had a running worker")
            updated = list_sessions(host, cwd)
            require(len(updated) == 1 and updated[0]["loadedBy"] == {
                "sessionId": session_id, "title": catalog_entry["title"]
            }, "Available binding was not returned in loadedBy")
            require(len(resume.rows(root, "session_runs", session_id)) == 2,
                    "Duplicate load created another worker")
            proof["load_and_readback"] = {
                "session_id": session_id, "provider_session_id": selected, "argv_resume_id": selected,
                "binding_id": staged["id"], "run_id": resumed["id"],
                "validated_at_was_null_before_hook": True,
                "confirming_hook": confirmed["metadata"]["lastHookEvent"],
                "confirmed_before_readback_input": confirmed_before_input,
                "model": metadata["model"], "permission_mode": metadata["permissionMode"],
                "spawn_revision": metadata["spawnRevision"], "duplicate_runs_added": 0,
                "previous_binding_expired": True, "loaded_by": session_id,
                "expected_token": token, "actual_reply": reply, "same_transcript": True,
            }
            host.shutdown()
    finally:
        identity.check_secret(root, config_path, key, proof)
        proof["credential_absent_from_artifacts"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--evidence", type=Path, default=launch.CHECKS / "claude-catalog.json")
    args = parser.parse_args()
    require(sys.platform == "darwin", "Real catalog verification requires macOS sandbox-exec")
    cli, binary, evidence = (args.claude.resolve(strict=True), args.runtime.resolve(strict=True),
                             args.evidence.resolve())
    require(evidence.is_relative_to(launch.REPO), "Evidence must stay inside the workspace")
    launch.CHECKS.mkdir(parents=True, exist_ok=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve()
    config, key = identity.provider_config(home)
    proof = {"sandbox_probe": {}, "outbound_policy": "loopback proxy to api.deepseek.com:443 only"}
    with tempfile.TemporaryDirectory(prefix="claude-catalog-", dir=launch.CHECKS) as directory:
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
    print(f"Catalog evidence: {evidence}", flush=True)
    if not proof["passed"]:
        print(proof["error"], file=sys.stderr, flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
