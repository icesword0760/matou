"""Matou IPC adapter for the pinned, unmodified claude-swap engine.

Only presentation, isolated official CLI login, and command dispatch live here.
Credential capture/storage/refresh/switch/rollback are upstream responsibilities.
No credential value or raw upstream exception crosses stdout/stderr.
"""
from __future__ import annotations

import contextlib
import hashlib
import io
import json
import logging
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

# Disable library logging before construction: upstream debug/error messages can
# contain account identifiers and OAuth responses. Matou emits only error codes.
logging.disable(logging.CRITICAL)

from claude_swap import __version__
from claude_swap.exceptions import ClaudeSwitchError
from claude_swap.session import AUTH_OVERRIDE_ENV_VARS, delete_macos_keychain_entry
from claude_swap.switcher import ClaudeAccountSwitcher

EXPECTED_VERSION = "0.27.0b1"


def identity_id(email: str, organization: str = "") -> str:
    return hashlib.sha256(json.dumps([email, organization], ensure_ascii=False).encode()).hexdigest()[:32]


def masked(email: str) -> str:
    local, _, domain = email.partition("@")
    return f"{local[:2]}•••@{domain}" if domain else "•••"


def view(row: dict) -> dict:
    usage = row.get("usage") or row.get("lastGoodUsage")
    result = {
        "id": identity_id(row["email"], row.get("organizationUuid", "")),
        "name": row.get("alias") or "Claude",
        "email": masked(row["email"]),
        "plan": row.get("organizationName") or "Claude",
        "needsLogin": row.get("usageStatus") in ("relogin_required", "no_credentials"),
    }
    if usage:
        from datetime import datetime
        stamp = row.get("usageFetchedAt") or row.get("lastGoodFetchedAt")
        if stamp:
            result["usage"] = {
                "checkedAt": int(datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000),
                "stale": row.get("usageStatus") != "ok" or (row.get("usageAgeSeconds") or 0) > 60,
            }
            for source, dest in (("fiveHour", "fiveHour"), ("sevenDay", "weekly")):
                item = usage.get(source)
                if item and isinstance(item.get("pct"), (int, float)):
                    result["usage"][dest] = {"used": max(0, min(100, item["pct"])), "resetsAt": item.get("resetsAt", "")}
    return result


def roster(engine: ClaudeAccountSwitcher, refresh: bool = False) -> tuple[dict, dict[str, str]]:
    payload = engine.list_accounts(json_output=True, fetch=None if refresh else set())
    if not isinstance(payload, dict) or payload.get("schemaVersion") != 1:
        raise ValueError("engine-version")
    rows = payload["accounts"]
    output = {"accounts": [view(row) for row in rows], "engineVersion": __version__}
    slots = {view(row)["id"]: str(row["number"]) for row in rows}
    current = next((row for row in rows if row.get("active")), None)
    if current:
        output["currentId"] = view(current)["id"]
    else:
        # Pinned private read-only adapter point: public status() fetches quota
        # even when the menu only needs a current-login onboarding candidate.
        identity = engine._get_current_account()
        if identity:
            output["detected"] = view({"email": identity[0], "organizationUuid": identity[1]})
    return output, slots


def profile_path(root: Path, nonce: str) -> Path:
    if not re.fullmatch(r"[a-f0-9-]{36}", nonce):
        raise ValueError("invalid")
    return root / "login" / nonce


@contextlib.contextmanager
def login_environment(profile: Path):
    previous = dict(os.environ)
    try:
        for name in (*AUTH_OVERRIDE_ENV_VARS, "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"):
            os.environ.pop(name, None)
        os.environ.pop("CLAUDECODE", None)
        os.environ["CLAUDE_CONFIG_DIR"] = str(profile)
        os.environ["CLAUDE_SECURESTORAGE_CONFIG_DIR"] = str(profile)
        yield
    finally:
        os.environ.clear()
        os.environ.update(previous)


def cleanup(profile: Path):
    delete_macos_keychain_entry(profile)  # Upstream owns service naming/deletion.
    shutil.rmtree(profile, ignore_errors=True)


def dispatch(request: dict, root: Path) -> dict:
    if __version__ != EXPECTED_VERSION:
        raise ValueError("engine-version")
    action = request.get("action")
    if action == "version":
        return {"engineVersion": __version__}
    if action != "cleanup-logins" and (os.environ.get("CLAUDE_CONFIG_DIR") or os.environ.get("CLAUDE_SECURESTORAGE_CONFIG_DIR")):
        # The approved entry represents the device default. An app launched
        # with a custom profile must not silently operate on a different store.
        raise ValueError("unsupported")
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    if action == "cleanup-logins":
        login_root = root / "login"
        if login_root.is_dir():
            for child in login_root.iterdir():
                if child.is_dir() and not child.is_symlink() and re.fullmatch(r"[a-f0-9-]{36}", child.name):
                    cleanup(child)
        return {}
    if action in ("login", "draft", "save-draft", "cancel-login"):
        profile = profile_path(root, request.get("nonce", ""))
        if action == "cancel-login":
            cleanup(profile)
            return {}
        if action == "login":
            claude = shutil.which("claude")
            if not claude:
                raise ValueError("cli-missing")
            profile.mkdir(parents=True, exist_ok=False, mode=0o700)
            try:
                with login_environment(profile):
                    child = subprocess.Popen([claude, "auth", "login"], cwd=profile,
                                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    try:
                        code = child.wait(timeout=180)
                    except BaseException:
                        child.terminate()
                        try:
                            child.wait(timeout=3)
                        except subprocess.TimeoutExpired:
                            child.kill(); child.wait()
                        raise
                    if code != 0:
                        raise ValueError("login-failed")
            except BaseException:
                cleanup(profile)
                raise
        with login_environment(profile):
            engine = ClaudeAccountSwitcher()
            identity = engine._get_current_account()
            if not identity:
                raise ValueError("login-failed")
            candidate = view({"email": identity[0], "organizationUuid": identity[1]})
            if request.get("replaceId") and candidate["id"] != request["replaceId"]:
                raise ValueError("identity")
            if action == "save-draft":
                if request.get("id") != candidate["id"]:
                    raise ValueError("identity")
                # Upstream add captures this explicit config profile, verifies
                # credential ownership, and updates duplicate identities in place.
                engine.add_account()
        if action == "save-draft":
            cleanup(profile)
            return roster(ClaudeAccountSwitcher())[0]
        return {"draft": candidate}
    engine = ClaudeAccountSwitcher()
    output, slots = roster(engine)
    if action == "snapshot":
        return output
    if action == "refresh":
        return roster(engine, refresh=True)[0]
    if action == "save-current":
        candidate = output.get("detected") or next((a for a in output["accounts"] if a["id"] == output.get("currentId")), None)
        if not candidate or candidate["id"] != request.get("id"):
            raise ValueError("conflict")
        engine.add_account()
    elif action == "switch":
        slot = slots.get(request.get("id"))
        if not slot:
            raise ValueError("conflict")
        # No force flag, automatic rotation, or replacement credential algorithm.
        result = engine.switch_to(slot, json_output=True)
        if not isinstance(result, dict):
            raise ValueError("engine-version")
    elif action == "remove":
        slot = slots.get(request.get("id"))
        if not slot:
            raise ValueError("conflict")
        engine.remove_account(slot, assume_yes=True)
    else:
        raise ValueError("invalid")
    result = roster(engine)[0]
    if action == "switch" and result.get("currentId") != request.get("id"):
        raise ValueError("switch-failed")
    return result


def main():
    import signal
    def interrupt(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupt)
    request = json.loads(sys.stdin.readline(8192))
    try:
        # Library output is human-oriented on add/remove. Never relay it or
        # arbitrary exception text into Electron logs or the renderer.
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            try:
                import truststore
                truststore.inject_into_ssl()
            except ImportError:
                pass
            result = dispatch(request, Path(sys.argv[1]))
        response = {"ok": True, "value": result}
    except (KeyboardInterrupt, subprocess.TimeoutExpired):
        response = {"ok": False, "error": "login-timeout"}
    except ValueError as exc:
        allowed = {"engine-version", "cli-missing", "login-failed", "identity", "conflict", "invalid", "switch-failed", "unsupported"}
        response = {"ok": False, "error": str(exc) if str(exc) in allowed else "engine-error"}
    except ClaudeSwitchError as exc:
        # Stable class-only mapping; upstream retains all transaction semantics.
        kind = type(exc).__name__
        code = {"CredentialReadError": "keychain", "ClaudeCodeLockTimeout": "conflict", "LockError": "conflict", "SessionError": "busy", "AccountNotFoundError": "conflict"}.get(kind, "engine-error")
        response = {"ok": False, "error": code}
    except Exception:
        response = {"ok": False, "error": "engine-error"}
    print(json.dumps(response, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
