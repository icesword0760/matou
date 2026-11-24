#!/usr/bin/env python3
"""Verify native forks and real context readback with Claude Code and DeepSeek."""

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


def seed_fork_canvas(root, env, guard, binary):
    cwd = root / "workspace/session-cwd"
    cwd.mkdir(parents=True)
    (root / "matou").mkdir()
    onboarding = {"hasCompletedOnboarding": True, "theme": "dark",
                  "projects": {str(cwd): {"hasTrustDialogAccepted": True}}}
    for path in (root / "home/.claude.json", root / "claude/.claude.json"):
        launch.write_json(path, onboarding)
    return launch.seed(root, env, guard, binary, cwd), cwd


def live_fork_argv(run, cli, provider_id):
    command = subprocess.check_output(
        ["/bin/ps", "-ww", "-p", str(run["pid"]), "-o", "args="],
        timeout=5, text=True, stderr=subprocess.DEVNULL,
    )
    argv = shlex.split(command)
    require(argv[:2] == [str(cli), "--settings"] and "--resume" in argv,
            "Live process did not use the installed CLI and resume")
    index = argv.index("--resume")
    observed = argv[index:index + 3]
    require(observed == ["--resume", provider_id, "--fork-session"],
            "Live process used incorrect fork arguments")
    return observed


def verify(root, env, guard, binary, cli, config, key, proof):
    source_id, cwd = seed_fork_canvas(root, env, guard, binary)
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
            proof["fork"] = {"card_id": child_id, "run_id": run["id"],
                             "argv": live_fork_argv(run, cli, parent)}

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


def verify_sibling_and_peer(root, env, guard, binary, cli, config, key, proof, placements):
    parent_id, cwd = seed_fork_canvas(root, env, guard, binary)
    config_path = identity.config_file(root, config)
    turns = proof["original_conversations"] = {}
    forks = proof["forks"] = {}
    try:
        with launch.database(root) as db:
            scene_id, = db.execute(
                "SELECT scene_id FROM session_canvas_memberships WHERE session_id=?", (parent_id,),
            ).fetchone()

        def request(selected_id, placement):
            return {"scene_id": scene_id, "source_session_id": selected_id,
                    "placement": placement, "worktree_mode": "current",
                    "name": f"real {placement}", "submission_key": f"verify-{placement}"}

        def open_fork(host, command, parent, relation, origin_provider, evidence):
            result, graph = fork_result(host, command, key)
            card_id = result.get("sessionId")
            require(card_id and card_id != command["source_session_id"]
                    and result["stage"] == "queued" and not result["replayed"],
                    f"{command['placement']} fork did not queue a new card ({result.get('code')})")
            node = next(node for node in graph["nodes"] if node["sessionId"] == card_id)
            require(node.get("parentSessionId") == parent and node.get("relationKind") == relation
                    and node["cwd"] == str(cwd), f"Wrong {command['placement']} graph")
            evidence.update(card_id=card_id, parent_card_id=parent, relation=relation,
                            selected_card_id=command["source_session_id"])
            replay, _ = fork_result(host, command, key)
            require(replay["sessionId"] == card_id and replay["replayed"],
                    "Pending fork did not replay its existing card")
            card = resume.Card(host, card_id, key)
            run = resume.running(root, card, 1)
            evidence.update(run_id=run["id"], argv=live_fork_argv(run, cli, origin_provider))
            host.command("Refresh")
            try:
                card.interactive(allow_scrolled_banner=True)
            except RuntimeError:
                evidence["terminal_screen"] = card.screen[-1200:]
                evidence["run_status"] = [row["status"] for row in resume.rows(
                    root, "session_runs", card.id
                )]
                raise
            return card, run["id"]

        def completed_turn(card, prompt, expected, evidence):
            provider_id, reply, transcript = card.ask(root, prompt, expected)
            card.wait("idle after Stop", lambda: True if
                      identity.session_state(root, card.id)[1] == "idle" else None)
            evidence.update(card_id=card.id, provider_id=provider_id, prompt=prompt, reply=reply)
            return provider_id, transcript

        def check_settled(host, command, card, run_id, provider_id, origin_provider, evidence):
            confirmed = card.wait("fork identity", lambda: card.event("confirmed", run_id))
            require(confirmed["providerSessionId"] == provider_id and provider_id != origin_provider,
                    "Fork identity did not match its new transcript")
            replay, _ = fork_result(host, command, key)
            require(replay["sessionId"] == card.id and replay["stage"] == "succeeded"
                    and replay["replayed"], "Completed fork did not replay its durable result")
            require(len(resume.rows(root, "session_runs", card.id)) == 1
                    and len(resume.rows(root, "provider_bindings", card.id)) == 1,
                    "Replayed fork created an extra run or binding")
            evidence.update(authoritative_hook=confirmed["metadata"]["lastHookEvent"],
                            replay_reused_card_and_run=True)

        with launch.runtime(root, env, guard, binary) as host:
            require(host.ready() == parent_id, "Runtime opened a different parent card")
            parent = resume.Card(host, parent_id, key)
            parent.interactive()
            parent_run = resume.running(root, parent, 1)
            resume.assert_argv(parent_run, cli, None)
            parent_token, source_token = (secrets.token_hex(8).upper() for _ in range(2))
            require(parent_token != source_token, "Seed tokens collided")
            turns["parent"] = {"token": parent_token, "run_id": parent_run["id"]}
            parent_provider, parent_transcript = completed_turn(
                parent, f"记住代号 {parent_token}，只回答 OK。不要调用工具。",
                "OK", turns["parent"],
            )
            seed = request(parent_id, "child")
            turns["source"] = {"token": source_token}
            source, source_run = open_fork(
                host, seed, parent_id, "forked-from", parent_provider, turns["source"],
            )
            source_provider, source_transcript = completed_turn(
                source, f"将当前代号改为 {source_token}，只回答 OK。不要调用工具。",
                "OK", turns["source"],
            )
            check_settled(host, seed, source, source_run, source_provider, parent_provider,
                          turns["source"])
            require(source_transcript != parent_transcript, "Original cards share a transcript")
            time.sleep(2)
            originals = {"parent": parent_id, "source": source.id}
            baseline = {label: source_snapshot(root, card_id) for label, card_id in originals.items()}
            for rows in baseline.values():
                require(rows["runs"][0]["status"] == "running"
                        and json.loads(rows["bindings"][0]["metadata_json"])["canFork"],
                        "Original cards must be live with completed turns")

            blocked, _ = fork_result(host, {
                **request(parent_id, "sibling"), "submission_key": "verify-root-sibling",
            }, key)
            require(blocked.get("code") == "ROOT_HAS_NO_FORK_PARENT"
                    and blocked.get("error") == "根层会话可创建子分支",
                    "Root sibling did not return the frozen error")
            proof["root_sibling_rejection"] = blocked

            for placement, relation, origin, prompt in (
                ("sibling", "forked-from", "parent", resume.READBACK),
                ("peer", "derived-from", "source", "当前的代号是什么？只回答代号。不要调用工具。"),
            ):
                if placement not in placements:
                    continue
                evidence = forks[placement] = {"fork_source": origin}
                command = request(source.id, placement)
                provider_id = turns[origin]["provider_id"]
                card, run_id = open_fork(host, command, parent_id, relation, provider_id, evidence)
                new_provider, transcript = completed_turn(
                    card, prompt, turns[origin]["token"], evidence,
                )
                require(transcript not in (parent_transcript, source_transcript)
                        and new_provider not in (parent_provider, source_provider),
                        "Fork reused an original transcript or provider identity")
                check_settled(host, command, card, run_id, new_provider, provider_id, evidence)
                evidence["original_cards_unchanged"] = {}
                for label, card_id in originals.items():
                    details = evidence["original_cards_unchanged"][label] = {}
                    verify_source_unchanged(source_snapshot_copy(baseline[label]),
                                            source_snapshot(root, card_id), details)
                    details["session_binding_run_and_summary_text_unchanged"] = True

            with launch.database(root) as db:
                count = len(placements) + 1
                require(db.execute("SELECT COUNT(*) FROM sessions").fetchone() == (count + 1,)
                        and db.execute("SELECT state, stage FROM session_fork_intents").fetchall()
                        == [("succeeded", "succeeded")] * count,
                        "A rejected or repeated request created an extra card or fork intent")
            host.shutdown()
    finally:
        identity.check_secret(root, config_path, key, proof)
        proof["credential_absent_from_artifacts"] = True


def source_snapshot_copy(snapshot):
    return {table: [row.copy() for row in rows] for table, rows in snapshot.items()}


def verify_retry(root, env, guard, binary, cli, config, key, proof):
    source_id, cwd = seed_fork_canvas(root, env, guard, binary)
    config_path = identity.config_file(root, config)
    cli_link = root / "retry-cli"
    cli_link.symlink_to(cli)
    env = dict(env, MATOU_CLAUDE_COMMAND=str(cli_link))
    frozen_error = ("FORK_NOT_FAILED", "当前分支无需重试")

    def retry_result(host, card_id):
        def observe(_, event):
            encoded = json.dumps(event, ensure_ascii=False).encode()
            require(key not in encoded and not identity.AUTH_HEADER.search(encoded),
                    "Sensitive bytes appeared in runtime output")
            result = event.get("Graph", event.get("Navigation", {}).get("graph", {})).get(
                "forkSessionResult",
            )
            return result if result and result.get("sessionId") == card_id else None
        return host.wait("RetryFork result", observe)

    def check_rejected(result):
        require((result.get("code"), result.get("error")) == frozen_error,
                "Retry did not return the frozen rejection")

    def intent(card_id):
        with launch.database(root) as db:
            db.row_factory = sqlite3.Row
            row = db.execute(
                "SELECT * FROM session_fork_intents WHERE session_id=?", (card_id,),
            ).fetchone()
            require(row is not None, "Fork intent was missing")
            return dict(row)

    try:
        with launch.database(root) as db:
            scene_id, = db.execute(
                "SELECT scene_id FROM session_canvas_memberships WHERE session_id=?", (source_id,),
            ).fetchone()
        with launch.runtime(root, env, guard, binary) as host:
            require(host.ready() == source_id, "Runtime opened a different source card")
            source = resume.Card(host, source_id, key)
            source.interactive()
            resume.assert_argv(resume.running(root, source, 1), cli_link, None)
            host.command("RetryFork", {"session_id": source_id})
            check_rejected(retry_result(host, source_id))
            token = secrets.token_hex(8).upper()
            source_provider, _, source_transcript = source.ask(
                root, f"记住代号 {token}，只回答 OK。不要调用工具。", "OK",
            )
            proof["source"] = {"card_id": source_id, "provider_id": source_provider, "token": token}
            request = {"scene_id": scene_id, "source_session_id": source_id, "placement": "child",
                       "worktree_mode": "current", "name": "重试验收分支",
                       "submission_key": "real-fork-retry"}

            # The live source keeps running. Only the isolated command path is removed.
            cli_link.unlink()
            accepted, _ = fork_result(host, request, key)
            card_id = accepted.get("sessionId")
            require(card_id and card_id != source_id and accepted["stage"] == "queued",
                    "Initial submission did not create a fork card")
            child = resume.Card(host, card_id, key)
            first_failure = child.wait("initial fork failure", lambda: child.event("fork-failed"))
            original = intent(card_id)
            require(original["state"] == "failed"
                    and first_failure["forkError"] == original["error_message"]
                    and "未检测到 Claude Code" in original["error_message"]
                    and not resume.rows(root, "session_runs", card_id),
                    "Missing executable did not park the initial attempt")
            proof["initial_failure"] = {"card_id": card_id, "attempt": original["attempt"],
                                        "reason": original["error_message"]}
            cli_link.symlink_to(cli)

            def retry(duplicates, expected_attempt):
                for _ in range(duplicates):
                    host.command("RetryFork", {"session_id": card_id})
                result = retry_result(host, card_id)
                require(result.get("forkState") == "pending" and result["stage"] == "queued"
                        and result["submissionKey"] == request["submission_key"]
                        and result["error"] is None and not result["replayed"],
                        "Explicit retry did not queue the same fork")
                for _ in range(duplicates - 1):
                    check_rejected(retry_result(host, card_id))
                pending = intent(card_id)
                require(pending["attempt"] == expected_attempt and pending["error_message"] is None,
                        "Retry did not clear the error and increment the attempt once")

            # Let the installed CLI reject the same source ID by hiding only its test transcript.
            quarantined = root / "source-transcript.jsonl"
            source_transcript.rename(quarantined)
            try:
                first_retry = original["attempt"] + 1
                # This attempt can exit before the receipt is read, so check its count after parking.
                host.command("RetryFork", {"session_id": card_id})
                result = retry_result(host, card_id)
                require(result.get("forkState") == "pending" and result["error"] is None,
                        "Retry did not publish the reset state")
                failure = child.wait("real CLI retry failure", lambda: next((
                    event for event in child.events if event["result"] == "fork-failed"
                    and event.get("runId")), None), timeout=25)
                failed_run = resume.exited(root, child, 1)
                failed = intent(card_id)
                require(failed["state"] == "failed" and failed["attempt"] == first_retry
                        and failure["runId"] == failed_run["id"]
                        and failed["error_message"] == failure["forkError"]
                        and failed["error_message"] != original["error_message"]
                        and not resume.rows(root, "provider_bindings", card_id),
                        "The failed retry did not replace the old reason on the same card")
                proof["retry_failure"] = {"attempt": first_retry, "run_id": failed_run["id"],
                                          "reason": failure["forkError"]}
            finally:
                quarantined.rename(source_transcript)

            child.screen = ""
            retry(duplicates=2, expected_attempt=first_retry + 1)
            run = resume.running(root, child, 2)
            argv = live_fork_argv(run, cli_link, source_provider)
            child.interactive()
            host.command("RetryFork", {"session_id": card_id})
            check_rejected(retry_result(host, card_id))
            require(not resume.rows(root, "provider_bindings", card_id),
                    "Retry bound a provider identity before authoritative confirmation")

            replied_id, reply, transcript = child.ask(root, resume.READBACK, token)
            confirmed = child.wait("retry confirmed by current run",
                                   lambda: child.event("confirmed", run["id"]))
            require(confirmed["providerSessionId"] == replied_id and replied_id != source_provider
                    and transcript != source_transcript and resume.binding(root, card_id, replied_id),
                    "Retried fork did not bind a new identity with the source conversation")
            host.command("RetryFork", {"session_id": card_id})
            check_rejected(retry_result(host, card_id))
            replay, graph = fork_result(host, request, key)
            require(replay["sessionId"] == card_id and replay["stage"] == "succeeded"
                    and replay["replayed"], "Original submission did not resolve to the retried card")
            node = next(node for node in graph["nodes"] if node["sessionId"] == card_id)
            require(node["title"] == original["display_name"] and node["parentSessionId"] == source_id
                    and node["cwd"] == str(cwd) and node["workStatus"] == "idle",
                    "Successful retry did not restore the original card as a normal session")
            settled = intent(card_id)
            for field in ("source_session_id", "source_provider_session_id", "display_name",
                          "worktree_mode", "submission_key", "operation_id"):
                require(settled[field] == original[field], f"Retry changed {field}")
            require(settled["state"] == "succeeded" and settled["attempt"] == first_retry + 1
                    and settled["error_message"] is None
                    and len(resume.rows(root, "session_runs", card_id)) == 2
                    and len(resume.rows(root, "provider_bindings", card_id)) == 1,
                    "Retry left extra runs or bindings, or stale intent progress")
            with launch.database(root) as db:
                require(db.execute("SELECT COUNT(*) FROM sessions").fetchone() == (2,)
                        and db.execute("SELECT COUNT(*) FROM session_fork_intents").fetchone() == (1,),
                        "Retries created another card or intent")
            proof["retry_success"] = {
                "card_id": card_id, "attempt": settled["attempt"], "run_id": run["id"], "argv": argv,
                "provider_id": replied_id, "reply": reply, "submission_key": settled["submission_key"],
                "distinct_transcript": True, "duplicate_running_and_succeeded_retry_rejected": True,
            }
            host.shutdown()
    finally:
        identity.check_secret(root, config_path, key, proof)
        proof["credential_absent_from_artifacts"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, default=shutil.which("claude"))
    parser.add_argument("--runtime", type=Path, default=launch.BINARY)
    workflow = parser.add_mutually_exclusive_group()
    workflow.add_argument("--placement", choices=("child", "sibling", "peer", "both"), default="child")
    workflow.add_argument("--retry", action="store_true", help="Verify real failed-fork retries")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    require(sys.platform == "darwin", "Real fork verification requires macOS sandbox-exec")
    cli = args.claude.resolve(strict=True)
    binary = args.runtime.resolve(strict=True)
    default_name = "claude-fork.json" if args.placement == "child" else f"claude-fork-{args.placement}.json"
    if args.retry:
        default_name = "claude-fork-retry.json"
    evidence = (args.evidence or launch.CHECKS / default_name).resolve()
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
                if args.retry:
                    verify_retry(root, env, guard, binary, cli, config, key, proof)
                elif args.placement == "child":
                    verify(root, env, guard, binary, cli, config, key, proof)
                else:
                    placements = ("sibling", "peer") if args.placement == "both" else (args.placement,)
                    verify_sibling_and_peer(root, env, guard, binary, cli, config, key, proof, placements)
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
