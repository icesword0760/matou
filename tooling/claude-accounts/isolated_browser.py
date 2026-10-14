"""Official CLI authorization in a disposable browser, never the user's profile.

Claude Code still owns OAuth and the localhost callback. Only its allowlisted
authorization URL is consumed here; CLI output and URLs never reach app logs.
"""
from __future__ import annotations

import html
import json
import os
from pathlib import Path
import queue
import re
import shutil
import signal
import subprocess
import threading
import tempfile
import time
from urllib.parse import parse_qs, urlsplit


def find_browser() -> str:
    candidates = []
    if os.name == "nt":
        for base in (os.environ.get("PROGRAMFILES"), os.environ.get("PROGRAMFILES(X86)"), os.environ.get("LOCALAPPDATA")):
            if base:
                candidates.extend(str(Path(base) / p) for p in (
                    "Google/Chrome/Application/chrome.exe", "Microsoft/Edge/Application/msedge.exe"))
    else:
        for base in (Path("/Applications"), Path.home() / "Applications"):
            candidates.extend(str(base / app / "Contents/MacOS" / binary) for app, binary in (
                ("Google Chrome.app", "Google Chrome"), ("Microsoft Edge.app", "Microsoft Edge"),
                ("Brave Browser.app", "Brave Browser")))
        candidates.extend(filter(None, (shutil.which(n) for n in ("google-chrome", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"))))
    for candidate in candidates:
        if Path(candidate).is_file() and os.access(candidate, os.X_OK):
            return candidate
    # No ordinary-browser fallback: it would silently reuse the old account.
    raise ValueError("browser-missing")


def authorization_url(output: bytes) -> str | None:
    for match in re.finditer(rb'https://[^\s\x1b<>"\x27]+(?=[\s\x1b])', output):
        try:
            url = match.group().decode("ascii")
            parsed = urlsplit(url)
            query = parse_qs(parsed.query)
            if ((parsed.hostname, parsed.path) in {
                    ("claude.com", "/cai/oauth/authorize"), ("claude.ai", "/oauth/authorize"),
                    ("console.anthropic.com", "/oauth/authorize"), ("platform.claude.com", "/oauth/authorize")}
                    and parsed.port in (None, 443)
                    and not parsed.username and not parsed.password and not parsed.fragment
                    and query.get("code_challenge") and query.get("state")):
                return url
        except (ValueError, UnicodeError):
            continue
    return None


def stop_process(child: subprocess.Popen | None):
    if child is None or child.poll() is not None:
        return
    try:
        if os.name == "posix":
            os.killpg(child.pid, signal.SIGTERM)
        else:
            child.terminate()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            if os.name == "posix":
                os.killpg(child.pid, signal.SIGKILL)
            else:
                child.kill()
            child.wait(timeout=3)
    except ProcessLookupError:
        pass


def start_browser(executable: str, profile: Path, url: str) -> subprocess.Popen:
    # An exclusive profile isolates this flow even from other incognito windows.
    # Outlive the login profile: saving/cancelling may remove credentials while
    # the user still has this browser window open. No default Chrome data is used.
    directory = Path(tempfile.mkdtemp(prefix="matou-claude-browser-"))
    start = profile / "browser-start.html"
    start.write_text('<!doctype html><meta charset="utf-8"><title>Matou · Claude</title>'
                     '<meta http-equiv="refresh" content="0; url=' + html.escape(url, quote=True) + '">'
                     '<p>Opening Claude sign-in…</p>', encoding="utf-8")
    start.chmod(0o600)
    child = subprocess.Popen([
        executable, "--user-data-dir=" + str(directory), "--incognito", "--new-window",
        "--no-first-run", "--no-default-browser-check", "--disable-sync", "--disable-background-mode",
        start.as_uri(),
    ], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        start_new_session=True)
    child.matou_browser_directory = directory
    return child


def login(claude: str, profile: Path, timeout: float = 300, *, code_input=None, nonce: str | None = None):
    executable = find_browser()
    # This override is scoped to the isolated CLI, not the shell or default app.
    env = dict(os.environ, BROWSER="true", NO_COLOR="1")
    child = None
    browser = None
    urls: queue.Queue[str] = queue.Queue(maxsize=1)
    reader = None
    codes: queue.Queue[str] = queue.Queue(maxsize=1)
    try:
        child = subprocess.Popen([claude, "auth", "login"], cwd=profile, env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.STDOUT, start_new_session=True)

        def read_codes():
            # Private parent pipe only. Never persist, print, or invoke code as a command.
            try:
                while line := code_input.readline(8193):
                    if len(line) > 8192 or not line.endswith("\n"):
                        return
                    try:
                        message = json.loads(line)
                        code = message.get("code", "")
                        if (message.get("action") == "submit-login-code" and message.get("nonce") == nonce
                                and isinstance(code, str) and len(code) <= 4096
                                and re.fullmatch(r"[A-Za-z0-9_~.\-]{10,}#[A-Za-z0-9_~.\-]{10,}", code.strip())):
                            codes.put_nowait(code.strip())
                            return  # One code per authorization attempt.
                    except (ValueError, AttributeError, queue.Full):
                        continue
            except (OSError, ValueError):
                pass

        if code_input is not None:
            threading.Thread(target=read_codes, daemon=True).start()

        def read_output():
            tail = b""
            sent = False
            try:
                while chunk := child.stdout.read1(4096):
                    if sent:
                        continue
                    tail = (tail + chunk)[-32768:]
                    url = authorization_url(tail)
                    if url:
                        try:
                            urls.put_nowait(url)
                        except queue.Full:
                            pass
                        sent = True
                        tail = b""
            except (OSError, ValueError):
                pass

        reader = threading.Thread(target=read_output, daemon=True)
        reader.start()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if browser is None:
                try:
                    browser = start_browser(executable, profile, urls.get(timeout=0.1))
                except queue.Empty:
                    pass
            try:
                submitted = codes.get_nowait()
            except queue.Empty:
                submitted = None
            if submitted is not None and child.poll() is None:
                try:
                    child.stdin.write((submitted + "\n").encode("ascii"))
                    child.stdin.flush()
                except (BrokenPipeError, OSError):
                    # Automatic callback may have won the race; let exit status decide.
                    pass
            code = child.poll()
            if code is not None:
                if code != 0 or browser is None:
                    raise ValueError("login-failed")
                return
            if browser is not None:
                if browser.poll() is not None:
                    raise ValueError("browser-closed")
                time.sleep(0.1)
        raise subprocess.TimeoutExpired("claude auth login", timeout)
    finally:
        # Stop only the CLI. All browser windows are closed by the user, even
        # after success, cancellation or timeout. Never delete a live profile.
        stop_process(child)
        directory = getattr(browser, "matou_browser_directory", None)
        if directory is not None and browser.poll() is not None:
            shutil.rmtree(directory, ignore_errors=True)
        if child and child.stdin:
            try:
                child.stdin.close()
            except OSError:
                pass
        if child and child.stdout:
            child.stdout.close()
        if reader:
            reader.join(timeout=1)
        (profile / "browser-start.html").unlink(missing_ok=True)
