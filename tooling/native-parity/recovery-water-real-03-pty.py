"""Report actual bytes received by the disposable shell's PTY."""
import base64
import fcntl
import json
import os
from pathlib import Path
import signal
import sys
import termios
import time
import tty

root = Path(sys.argv[1])
terminal = sys.stdin.fileno()
if not os.isatty(terminal) or not os.isatty(sys.stdout.fileno()):
    raise SystemExit("Expected a real PTY")
with (root / "sequence").open("a+") as counter:
    fcntl.flock(counter, fcntl.LOCK_EX)
    counter.seek(0)
    ordinal = int(counter.read() or "0") + 1
    counter.seek(0)
    counter.truncate()
    counter.write(str(ordinal))


def record(event, **fields):
    with (root / "pty.jsonl").open("a", buffering=1) as trace:
        trace.write(json.dumps({
            "event": event, "uptime_ns": time.monotonic_ns(), "unix_ns": time.time_ns(),
            "pid": os.getpid(), "ppid": os.getppid(), "tty": os.ttyname(terminal),
            "fixture_ordinal": ordinal, **fields,
        }) + "\n")


def stop(_signum, _frame):
    raise SystemExit(0)


signal.signal(signal.SIGHUP, stop)
signal.signal(signal.SIGTERM, stop)
saved = termios.tcgetattr(terminal)
try:
    tty.setraw(terminal)
    record("start")
    if ordinal == 1:
        command = base64.b64encode(b"rw03-history-fixture")
        history = b"\r\n".join(f"RW03-OLD-HISTORY-{line:02d}".encode() for line in range(1, 7))
        os.write(1, b"\x1b]133;A\x07\x1b]633;E;" + command + b"\x07\x1b]133;C\x07"
                 + history + b"\r\n\x1b]133;D;0\x07\x1b]133;A\x07")
    marker = f"RW03-LIVE-PTY-{ordinal:02d}> ".encode()
    os.write(1, marker)
    record("ready", marker=marker.decode())
    while True:
        data = os.read(terminal, 4096)
        if not data:
            break
        record("input", hex=data.hex(), byte_count=len(data))
        os.write(1, data)
except OSError:
    pass
finally:
    try:
        termios.tcsetattr(terminal, termios.TCSANOW, saved)
    except OSError:
        pass
