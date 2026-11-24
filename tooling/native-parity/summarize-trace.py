#!/usr/bin/env python3
"""Summarize real renderer trace diagnostics without treating them as display latency."""
import collections, json, math, sys
from pathlib import Path

directory = Path(sys.argv[1])
events = json.loads((directory / 'trace.json').read_text())['traceEvents']
main_threads = {(e['pid'], e['tid']) for e in events
                if e.get('name') == 'thread_name' and e.get('args', {}).get('name') == 'CrRendererMain'}

def stats(values):
    values = sorted(values)
    if not values:
        return {'count': 0}
    return {'count': len(values), 'p50_ms': values[math.ceil(len(values) * .50) - 1],
            'p95_ms': values[math.ceil(len(values) * .95) - 1], 'max_ms': values[-1]}

tasks = [e['dur'] / 1000 for e in events if e.get('name') == 'RunTask' and 'dur' in e
         and (e['pid'], e['tid']) in main_threads]
dispatch = collections.defaultdict(list)
for e in events:
    if e.get('name') == 'EventDispatch' and 'dur' in e and (e['pid'], e['tid']) in main_threads:
        dispatch[e.get('args', {}).get('data', {}).get('type', 'unknown')].append(e['dur'] / 1000)
latency = collections.defaultdict(list)
begins = collections.defaultdict(list)
for e in events:
    if e.get('name') != 'EventLatency':
        continue
    key = (e['pid'], json.dumps(e.get('id2', e.get('id')), sort_keys=True), e.get('scope'))
    if e['ph'] == 'b':
        begins[key].append(e)
    elif e['ph'] == 'e' and begins[key]:
        start = begins[key].pop()
        kind = start.get('args', {}).get('event_latency', {}).get('event_type', 'unknown')
        latency[kind].append((e['ts'] - start['ts']) / 1000)
summary = {
    'interpretation': 'Diagnostic trace of one mixed real-UI run. EventLatency is the recorded Chromium interval, not proof that shell output or a target view became visible. No physical input-to-display claim. Not a frozen performance budget.',
    'event_count': len(events), 'renderer_main_threads': sorted(main_threads),
    'renderer_tasks': {**stats(tasks), 'over_50ms': sum(v > 50 for v in tasks)},
    'event_dispatch': {k: stats(v) for k, v in sorted(dispatch.items())},
    'engine_event_latency': {k: stats(v) for k, v in sorted(latency.items())},
    'unmatched_latency_begins': sum(map(len, begins.values())),
}
(directory / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(summary, indent=2))
