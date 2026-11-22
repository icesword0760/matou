"""Locate and clean up the channel for an isolated desktop probe fixture."""
from contextlib import contextmanager
import os
from pathlib import Path


@contextmanager
def instance_channel(data_root):
    # Match instance_open.rs's stable hash of the canonical Unix path bytes.
    key = 0xcbf29ce484222325
    for byte in os.fsencode(data_root.resolve()):
        key = ((key ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    directory = Path('/tmp') / f'matou-instance-{os.geteuid()}'
    endpoint = directory / f'{key:016x}.sock'
    lock = directory / f'{key:016x}.lock'
    try:
        yield endpoint, lock
    finally:
        endpoint.unlink(missing_ok=True)
        lock.unlink(missing_ok=True)
