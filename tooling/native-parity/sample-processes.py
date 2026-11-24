#!/usr/bin/env python3
"""Sample only the isolated reference process tree; RSS is not physical footprint."""
import json
import pathlib
import subprocess
import sys
import time

root = pathlib.Path(__file__).resolve().parents[2]
launch = json.loads((root / 'artifacts/native-parity/reference/launch.json').read_text())
pid = launch['pid']
out = root / 'artifacts/native-parity/reference/evidence' / f'process-samples-{time.time_ns()}.json'
duration = int(sys.argv[1])
started = time.monotonic()
samples = []
print(json.dumps({'pid': pid, 'output': str(out)}), flush=True)
while time.monotonic() - started < duration:
    raw = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,rss=,time=,comm='], text=True)
    rows = [line.split(None, 4) for line in raw.splitlines()]
    descendants = {pid}
    while True:
        found = {int(row[0]) for row in rows if int(row[1]) in descendants}
        if found <= descendants:
            break
        descendants |= found
    processes = [{'pid': int(r[0]), 'parent': int(r[1]), 'rssKiB': int(r[2]),
                  'cpuTime': r[3], 'executable': r[4]} for r in rows if int(r[0]) in descendants]
    samples.append({'seconds': time.monotonic() - started, 'processes': processes})
    time.sleep(1)
out.write_text(json.dumps({'launch': launch, 'samples': samples,
    'scope': 'Process-tree cumulative CPU time and sum RSS; includes shared-page double counting, not physical memory.'}, indent=2) + '\n')
