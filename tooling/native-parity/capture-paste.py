"""Record one real bracketed paste, preserving Unicode, spaces and newlines."""
import json
import os
from pathlib import Path
import select
import sys
import termios
import time
import tty

result = Path(sys.argv[1])
fd = sys.stdin.fileno()
saved = termios.tcgetattr(fd)
data = bytearray()
end = b'\x1b[201~'
try:
    tty.setraw(fd)
    sys.stdout.write('\x1b[?2004hPASTE_CAPTURE_READY\r\n')
    sys.stdout.flush()
    deadline = time.monotonic() + 120
    while end not in data:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
            raise TimeoutError('no bracketed paste received')
        chunk = os.read(fd, 4096)
        if not chunk or chunk == b'\x03':
            raise KeyboardInterrupt
        data.extend(chunk)
finally:
    sys.stdout.write('\x1b[?2004l')
    sys.stdout.flush()
    termios.tcsetattr(fd, termios.TCSADRAIN, saved)

start = data.index(b'\x1b[200~') + 6
payload = bytes(data[start:data.index(end, start)])
record = {'text': payload.decode('utf-8'), 'utf8Hex': payload.hex()}
result.write_text(json.dumps(record, ensure_ascii=False, indent=2) + '\n')
print('\r\nPASTE_CAPTURED ' + json.dumps(record, ensure_ascii=False))
