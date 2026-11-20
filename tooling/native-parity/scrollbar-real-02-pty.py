"""Feed numbered history to the isolated real-window terminal and trace raw input."""

import json
import os
from pathlib import Path
import select
import sys
import termios
import time
import tty


def main():
    run = Path(sys.argv[1])
    terminal = sys.stdin.fileno()
    if not os.isatty(terminal) or not os.isatty(sys.stdout.fileno()):
        raise SystemExit("A terminal PTY is required")
    original_mode = termios.tcgetattr(terminal)
    output = sys.stdout.buffer
    with (run / "pty.jsonl").open("a", encoding="utf-8", buffering=1) as trace:
        def record(event, **fields):
            trace.write(json.dumps({
                "event": event, "time_ns": time.time_ns(), "pid": os.getpid(), **fields,
            }) + "\n")

        def emit(first, last):
            output.write("".join(f"SCROLL-ROW-{i:04d}\r\n" for i in range(first, last + 1)).encode())
            output.flush()

        try:
            tty.setraw(terminal)
            last_row = 300
            emit(1, last_row)
            output.write(b"SCROLL-ECHO-READY> ")
            output.flush()
            record("ready", last_row=last_row)
            active = None
            while True:
                if select.select([terminal], [], [], 0.008)[0]:
                    chunk = os.read(terminal, 4096)
                    if not chunk:
                        break
                    record("input", hex=chunk.hex(), text=chunk.decode("utf-8", errors="replace"))
                    output.write(chunk)
                    output.flush()
                if active is None:
                    for request in sorted(run.glob("emit-*.request")):
                        done = request.with_suffix(".accepted")
                        if done.exists():
                            continue
                        count = int(request.read_text())
                        if not 1 <= count <= 200:
                            raise SystemExit("Invalid requested output count")
                        done.touch(exist_ok=False)
                        active = (request.stem, last_row + count)
                        output.write(b"\r\n")
                        output.flush()
                        record("output-start", request=request.stem, first=last_row + 1, count=count)
                        break
                if active is not None:
                    name, end = active
                    batch_end = min(last_row + 4, end)
                    emit(last_row + 1, batch_end)
                    record("output-batch", request=name, first=last_row + 1, last=batch_end)
                    last_row = batch_end
                    if last_row == end:
                        output.write(b"SCROLL-ECHO-READY> ")
                        output.flush()
                        record("output-end", request=name, last=last_row)
                        active = None
        finally:
            termios.tcsetattr(terminal, termios.TCSANOW, original_mode)


if __name__ == "__main__":
    main()
