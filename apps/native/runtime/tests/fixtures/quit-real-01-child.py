"""Bounded PTY process used only by the isolated quit observation."""

from collections import deque
import json
import os
from pathlib import Path
import signal
import sys
import time


mode, directory = sys.argv[1:]
deadline = time.monotonic() + 45
first_signal = False
pending_signals = deque()
log = (Path(directory) / "child.jsonl").open("x", buffering=1)


def record(event, **fields):
    log.write(json.dumps({"event": event, "unix_ns": time.time_ns(), "pid": os.getpid(), **fields}) + "\n")


def on_signal(number, _frame):
    # Shell exit and foreground-group cleanup can deliver overlapping HUPs.
    # Run buffered logging in the main loop so the handler cannot reenter it.
    pending_signals.append((number, time.monotonic()))


for name in (signal.SIGHUP, signal.SIGTERM):
    signal.signal(name, on_signal)
record("ready", ppid=os.getppid(), pgid=os.getpgrp(), foreground_pgid=os.tcgetpgrp(0),
       tty=os.ttyname(0), mode=mode)
print(f"QUIT-REAL-01 {mode}: PID {os.getpid()} ready", flush=True)
while time.monotonic() < deadline:
    while pending_signals:
        number, received_at = pending_signals.popleft()
        record("signal", signal=signal.Signals(number).name)
        if not first_signal:
            first_signal = True
            wait = 1 if mode == "handled" else 3
            deadline = received_at + wait
            record("self_exit_armed", delay_ms=wait * 1000)
    time.sleep(min(0.02, max(0, deadline - time.monotonic())))
record("self_exit", saw_signal=first_signal)
