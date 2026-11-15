#!/usr/bin/env python3
"""Analyze input-feedback.py's recorded color responses; needs Pillow (tooling only)."""
import base64, io, json, math, statistics, sys
from pathlib import Path
from PIL import Image

directory = Path(sys.argv[1])
sample_count = int(sys.argv[2])
events = json.loads((directory / 'trace.json').read_text())['traceEvents']
pastes = [e for e in events if e.get('name') == 'EventDispatch'
          and e.get('args', {}).get('data', {}).get('type') == 'paste']
if len(pastes) != sample_count:
    raise SystemExit(f'Expected exactly {sample_count} digit pastes, found {len(pastes)}; exclude setup/exit from the recording')
screenshots = sorted((e for e in events if e.get('name') == 'Screenshot'), key=lambda e: e['ts'])
frames = []
for i, frame in enumerate(screenshots):
    image = Image.open(io.BytesIO(base64.b64decode(frame['args']['snapshot']))).convert('RGB')
    pixels = list(image.get_flattened_data())
    red = sum(r > 150 and r > g * 1.6 and r > b * 1.6 for r, g, b in pixels)
    green = sum(g > 70 and g > r * 1.6 and g > b * 1.5 for r, g, b in pixels)
    frames.append({'i': i, 'ts': frame['ts'], 'color': 'red' if red > 100 else 'green' if green > 100 else None,
                   'red_pixels': red, 'green_pixels': green, 'size': image.size})
if not frames or any(f['color'] for f in frames if f['ts'] < pastes[0]['ts']):
    raise SystemExit('Missing neutral pre-input baseline; do not infer response from a preexisting patch')
samples = []
for i, event in enumerate(pastes):
    expected = 'red' if i % 2 == 0 else 'green'
    deadline = pastes[i + 1]['ts'] if i + 1 < len(pastes) else float('inf')
    visible = next((f for f in frames if event['ts'] <= f['ts'] < deadline and f['color'] == expected), None)
    samples.append({'sample': i + 1, 'input_ts': event['ts'], 'expected': expected,
                    'first_visible': visible, 'delay_ms': (visible['ts'] - event['ts']) / 1000 if visible else None})
values = sorted(s['delay_ms'] for s in samples if s['delay_ms'] is not None)
report = {
    'measurement': 'Renderer paste dispatch to first compositor trace screenshot containing the corresponding PTY-generated color patch. Includes frame sampling overhead; not OS-keydown-to-photon latency.',
    'samples': samples, 'frames': frames, 'missing_samples': sample_count - len(values),
    'median_ms': statistics.median(values) if values else None,
    'p95_ms': values[math.ceil(len(values) * .95) - 1] if values else None,
    'max_ms': max(values) if values else None,
}
(directory / 'input-feedback.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({k: v for k, v in report.items() if k not in ('samples', 'frames')}, indent=2))
if len(values) != sample_count:
    raise SystemExit('Incomplete sample set; do not treat this run as a performance baseline')
