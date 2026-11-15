#!/usr/bin/env python3
"""Real-PTY baseline workload: each input byte changes a visible color patch.

Run only in an isolated test terminal. Paste digits individually; q or Ctrl+C exits.
This never simulates host UI input and restores terminal modes on exit.
"""
import os, sys, termios, tty

fd = sys.stdin.fileno()
previous = termios.tcgetattr(fd)
try:
    tty.setraw(fd)
    sys.stdout.write('\x1b[2J\x1b[HInput feedback: paste digits; q exits\r\n')
    sys.stdout.flush()
    count = 0
    while True:
        value = os.read(fd, 1)
        if value in (b'', b'q', b'\x03'):
            break
        if value not in b'0123456789':
            continue
        count += 1
        color = '211;47;47' if count % 2 else '46;125;50'
        sys.stdout.write(f'\x1b[3;1H\x1b[48;2;{color}m  RESPONSE {count:02d} / {value.decode()}                \x1b[0m')
        sys.stdout.flush()
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, previous)
    sys.stdout.write('\x1b[0m\r\n')
    sys.stdout.flush()
