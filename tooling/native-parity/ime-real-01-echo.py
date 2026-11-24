"""Echo input from a real terminal PTY and log its bytes for IME evidence."""

import json
import os
from pathlib import Path
import sys
import termios
import time
import tty


def main():
    fd = sys.stdin.fileno()
    if not os.isatty(fd) or not os.isatty(sys.stdout.fileno()):
        raise SystemExit("This fixture requires a terminal PTY")
    output = sys.stdout.buffer
    original = termios.tcgetattr(fd)
    with Path(sys.argv[1]).open("a", encoding="utf-8", buffering=1) as trace:
        def record(event, **fields):
            row = {"event": event, "time_ns": time.time_ns(), "pid": os.getpid(), **fields}
            trace.write(json.dumps(row, ensure_ascii=False) + "\n")

        try:
            tty.setraw(fd)
            history = "".join(f"IME-HISTORY-{index:04d}\r\n" for index in range(1, 301))
            output.write(b"\x1b[?2004h" + history.encode() + b"IME-ECHO-READY> ")
            output.flush()
            record("ready", history_lines=300)
            while chunk := os.read(fd, 4096):
                record("input", hex=chunk.hex(), text=chunk.decode("utf-8", errors="replace"))
                output.write(chunk)
                output.flush()
        finally:
            output.write(b"\x1b[?2004l")
            output.flush()
            termios.tcsetattr(fd, termios.TCSANOW, original)


if __name__ == "__main__":
    main()
