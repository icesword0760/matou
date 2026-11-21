"""A network-free PTY stand-in that records and retains its launch environment."""
import datetime
import json
import os
from pathlib import Path
import select
import signal
import sys


provider, run_path, *arguments = sys.argv[1:]
run = Path(run_path)
keys = ("TZ", "LANG", "LC_ALL", "LC_CTYPE", "LANGUAGE", "MATOU_LOCALE", "HOME",
        "TERM", "COLORTERM", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "DSH_HOME")


def record(event):
    row = {
        "time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "provider": provider, "event": event, "pid": os.getpid(), "ppid": os.getppid(),
        "cwd": os.getcwd(), "argv": arguments, "tty": os.isatty(0),
        "env": {key: os.environ.get(key) for key in keys},
    }
    fd = os.open(run / "provider-env.jsonl", os.O_CREAT | os.O_WRONLY | os.O_APPEND, 0o600)
    try:
        os.write(fd, (json.dumps(row, ensure_ascii=False) + "\n").encode())
    finally:
        os.close(fd)


def stop(number, _frame):
    record(signal.Signals(number).name)
    sys.exit(0)


if any(flag in arguments for flag in ("--version", "-v")):
    record("version")
    print({"claude": "2.1.0 (Claude Code)", "codex": "codex-cli 0.122.0", "dsh": "dsh 0.2.1"}[provider])
    sys.exit(0)

if provider == "dsh" and "--dump-config" in arguments:
    record("config")
    print("- id: dsh-tui\n  inject:\n    - agents\n  config:\n    model: fixture-model\n    backend: codex")
    sys.exit(0)

record("start")
for sig in (signal.SIGHUP, signal.SIGTERM, signal.SIGINT):
    signal.signal(sig, stop)
print(f"\r\n{provider} fixture · PID {os.getpid()} · TZ={os.environ.get('TZ', '<unset>')}\r", flush=True)
while True:
    readable, _, _ = select.select([0], [], [], 3)
    if readable and not os.read(0, 4096):
        record("eof")
        break
    record("alive")
