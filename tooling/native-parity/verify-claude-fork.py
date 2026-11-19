#!/usr/bin/env python3
"""Verify a native child fork and real context readback with Claude Code and DeepSeek."""

import argparse
import importlib.util
import json
from pathlib import Path
import re
import secrets
import shlex
import shutil
import sqlite3
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
identity, launch, require = resume.identity, resume.launch, resume.require


def fork_result(host, request, key):
    host.command("ForkSession", request)

    def observe(_, event):
        encoded = json.dumps(event, ensure_ascii=False).encode()
        require(key not in encoded and not identity.AUTH_HEADER.search(encoded),
                "Sensitive bytes appeared in runtime output")
        graph = event.get("Graph", event.get("Navigation", {}).get("graph", {}))
        result = graph.get("forkSessionResult")
        if result and result["submissionKey"] == request["submission_key"]:
            return result, graph

    return host.wait("ForkSession result", observe)


def source_snapshot(root, session_id):
    with launch.database(root) as db:
        db.row_factory = sqlite3.Row
        db.execute("BEGIN")
        def rows(table, condition):
            return [dict(row) for row in db.execute(
                f"SELECT * FROM {table} WHERE {condition}=? ORDER BY rowid", (session_id,)
            )]
        return {
            "session": rows("sessions", "id"),
            "summary": rows("session_graph_summaries", "session_id"),
            "bindings": rows("provider_bindings", "session_id"),
            "runs": rows("session_runs", "session_id"),
        }


def verify_source_unchanged(before, after, proof):
    # The source PTY can independently redraw while the child is open. The
    # regular summary writer advances updated_at even for identical lines.
    summary_before = before.pop("summary")
    summary_after = after.pop("summary")
    proof["source_summary_timestamp_advanced"] = summary_before != summary_after
    for summaries in (summary_before, summary_after):
        for summary in summaries:
            summary.pop("updated_at")
    require(summary_before == summary_after, "The source summary text changed during the fork")
    if after != before:
        proof["source_row_differences"] = {
            table: {"counts": [len(before[table]), len(current)],
                    "columns": sorted({column for previous, observed in zip(before[table], current)
                                       for column in previous if previous[column] != observed[column]})}
            for table, current in after.items() if current != before[table]
        }
        raise RuntimeError("Source session, binding, or run rows changed during the test")


def verify(root, env, guard, binary, cli, config, key, proof):
    cwd = root / "workspace/session-cwd"
    cwd.mkdir(parents=True)
    (root / "matou").mkdir()
    onboarding = {"hasCompletedOnboarding": True, "theme": "dark",
                  "projects": {str(cwd): {"hasTrustDialogAccepted": True}}}
    for path in (root / "home/.claude.json", root / "claude/.claude.json"):
        launch.write_json(path, onboarding)
    source_id = launch.seed(root, env, guard, binary, cwd)
    config_path = identity.config_file(root, config)
    try:
        with launch.database(root) as db:
            scene_id, = db.execute("SELECT scene_id FROM session_canvas_memberships WHERE session_id=?",
                                   (source_id,)).fetchone()
        token = secrets.token_hex(8).upper()
        prompt = f"记住代号 {token}，只回答 OK。不要调用工具。"
        request = {"scene_id": scene_id, "source_session_id": source_id, "placement": "child",
                   "worktree_mode": "current", "name": "  上下文分支  ", "submission_key": "real-child"}
        with launch.runtime(root, env, guard, binary) as host:
            require(host.ready() == source_id, "Runtime opened a different source card")
            source = resume.Card(host, source_id, key)
            source.interactive()
            source_run = resume.running(root, source, 1)
            resume.assert_argv(source_run, cli, None)
            blocked, graph = fork_result(host, request, key)
            source_node = next(node for node in graph["nodes"] if node["sessionId"] == source_id)
            require(blocked.get("code") == "FORK_SOURCE_NOT_READY" and not source_node["canFork"],
                    "Source without a completed turn was allowed to fork")
            proof["incomplete_turn_rejected"] = blocked["code"]

            try:
                parent, first_reply, parent_transcript = source.ask(root, prompt, "OK")
            except RuntimeError:
                exchange = resume.exchange(root, prompt)
                proof["source_exchange"] = {
                    "transcript_reply_present": exchange is not None,
                    "reply_was_ok": exchange is not None and exchange[1] == "OK",
                    "reply_was_ok_with_punctuation": exchange is not None and
                    exchange[1].rstrip(".!。！") == "OK",
                    "binding_last_hooks": [json.loads(row["metadata_json"]).get("lastHookEvent")
                                           for row in resume.rows(root, "provider_bindings", source_id)],
                }
                raise
            source.wait("source is idle after Stop", lambda: True if
                        identity.session_state(root, source_id)[1] == "idle" else None)
            # Allow the source CLI's final redraw and the batched terminal summary to settle.
            time.sleep(2)
            before = source_snapshot(root, source_id)
            require(json.loads(before["bindings"][0]["metadata_json"])["canFork"],
                    "Completed CLI turn did not enable Fork")
            proof["source"] = {"card_id": source_id, "provider_id": parent,
                               "run_id": source_run["id"], "reply": first_reply}

            accepted, graph = fork_result(host, request, key)
            child_id = accepted.get("sessionId")
            require(child_id and child_id != source_id and accepted["stage"] == "queued"
                    and not accepted["replayed"], "First fork submission was not accepted")
            node = next(node for node in graph["nodes"] if node["sessionId"] == child_id)
            require(node["parentSessionId"] == source_id and node["relationKind"] == "forked-from"
                    and node["title"] == "上下文分支" and node["cwd"] == str(cwd)
                    and node["currentMode"] == "claude-code" and node["workStatus"] == "starting",
                    "Child graph did not match the accepted fork")
            duplicate, _ = fork_result(host, request, key)
            require(duplicate["sessionId"] == child_id and duplicate["replayed"],
                    "Duplicate submission did not return the existing card")

            child = resume.Card(host, child_id, key)
            run = resume.running(root, child, 1)
            command = subprocess.check_output(
                ["/bin/ps", "-ww", "-p", str(run["pid"]), "-o", "args="],
                timeout=5, text=True, stderr=subprocess.DEVNULL,
            )
            argv = shlex.split(command)
            require(argv[:2] == [str(cli), "--settings"] and "--resume" in argv
                    and argv[argv.index("--resume"):argv.index("--resume") + 3]
                    == ["--resume", parent, "--fork-session"], "Live process used incorrect fork arguments")
            proof["fork"] = {"card_id": child_id, "run_id": run["id"],
                             "argv": ["--resume", parent, "--fork-session"]}

            def confirmation():
                failure = child.event("fork-failed", run["id"])
                if failure:
                    raise RuntimeError(failure["forkError"])
                return child.event("confirmed", run["id"])

            try:
                child.interactive()
                idle_start = time.monotonic()

                def idle_without_binding():
                    require(confirmation() is None and not resume.rows(root, "provider_bindings", child_id),
                            "Idle child already had a confirmed identity")
                    with launch.database(root) as db:
                        state = db.execute("SELECT stage FROM session_fork_intents WHERE session_id=?",
                                           (child_id,)).fetchone()
                    require(state == ("restoring-provider",), "Idle child did not stay pending")
                    return True if time.monotonic() - idle_start >= 12 else None

                child.wait("12 seconds of idle fork without binding", idle_without_binding, timeout=15)
                proof["fork"]["idle_seconds_without_binding"] = round(time.monotonic() - idle_start, 2)
                replied_id, reply, transcript = child.ask(root, resume.READBACK, token)
                event = child.wait("real child hook identity", confirmation)
            except RuntimeError:
                proof["fork"]["cli_screen_markers"] = [
                    marker for marker in ("Claude Code", "❯", "Resume this conversation?",
                                          "resume full session", "Quick safety check",
                                          "Detected a custom API key", "No conversation found",
                                          "Hooks", "Error")
                    if marker.casefold() in child.screen.casefold()
                ]
                proof["fork"]["hook_results"] = [
                    {"result": event["result"], "reason": event.get("reason"),
                     "fork_error": event.get("forkError"),
                     "hook": event.get("metadata", {}).get("lastHookEvent")}
                    for event in child.events
                ]
                confirmation()
                raise
            new_id = event["providerSessionId"]
            require(new_id != parent and resume.binding(root, child_id, new_id),
                    "Child hook did not establish a new binding")
            proof["fork"].update(provider_id=new_id,
                                 first_authoritative_hook=event["metadata"]["lastHookEvent"])
            replayed, _ = fork_result(host, request, key)
            require(replayed["sessionId"] == child_id and replayed["stage"] == "succeeded"
                    and replayed["replayed"], "Completed fork did not replay its durable result")
            require(replied_id == new_id and transcript != parent_transcript,
                    "Readback was not recorded in a new conversation transcript")
            proof["readback"] = {"requested": resume.READBACK, "source_token": token, "reply": reply,
                                 "distinct_transcript": True}
            verify_source_unchanged(before, source_snapshot(root, source_id), proof)
            require(len(resume.rows(root, "session_runs", child_id)) == 1
                    and len(resume.rows(root, "provider_bindings", child_id)) == 1,
                    "Child got multiple runs or bindings")
            with launch.database(root) as db:
                require(db.execute("SELECT COUNT(*) FROM sessions").fetchone() == (2,)
                        and db.execute("SELECT state, stage FROM session_fork_intents").fetchall()
                        == [("succeeded", "succeeded")], "Duplicate fork created extra state")
            proof["source_session_binding_run_and_summary_text_unchanged"] = True
            proof["duplicate_reused_card_and_run"] = True
            host.shutdown()
    finally:
        identity.check_secret(root, config_path, key, proof)
        proof["credential_absent_from_artifacts"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    parser.add_argument("--evidence", type=Path, default=launch.CHECKS / "claude-fork.json")
    args = parser.parse_args()
    require(sys.platform == "darwin", "Real fork verification requires macOS sandbox-exec")
    cli = args.claude.resolve(strict=True)
    binary = args.runtime.resolve(strict=True)
    evidence = args.evidence.resolve()
    require(evidence.is_relative_to(launch.REPO), "Evidence must stay inside the workspace")
    launch.CHECKS.mkdir(parents=True, exist_ok=True)
    evidence.parent.mkdir(parents=True, exist_ok=True)
    home = Path.home().resolve()
    config, key = identity.provider_config(home)
    proof = {"sandbox_probe": {}}
    with tempfile.TemporaryDirectory(prefix="claude-fork-", dir=launch.CHECKS) as temporary:
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
                proof["network"] = "Direct outbound denied. Proxy limited to api.deepseek.com:443."
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
    print(f"Fork evidence: {evidence}", flush=True)
    if not proof["passed"]:
        print(proof["error"], file=sys.stderr, flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
