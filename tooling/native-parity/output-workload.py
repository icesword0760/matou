#!/usr/bin/env python3
"""The existing long-history gate's 8 MiB workload, through a real Shell PTY."""
import sys
import time

label = sys.argv[1]
target = 8 * 1024 * 1024
duration = 30
chunk = (("H" * 127 + "\n") * 64).encode()
written = 0
started = time.monotonic()
print(f"PARITY_HISTORY_START_{label}", flush=True)
while written < target:
    sys.stdout.buffer.write(chunk)
    sys.stdout.buffer.flush()
    written += len(chunk)
    time.sleep(max(0, started + written / target * duration - time.monotonic()))
print(f"PARITY_HISTORY_DONE_{label}", flush=True)
