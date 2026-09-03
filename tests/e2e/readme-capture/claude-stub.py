#!/usr/bin/env python3
"""Stand-in for the `claude` CLI used only by readme-capture.spec.ts.

Each launch pops one role from $MATOU_DEMO_ROOT/roles.queue, prints that role's
canned transcript, then replays the role's hook events against Matou's provider
hook endpoint so the HUD, work status and DAG reflect a believable session.
"""
import fcntl
import json
import os
import sys
import time
import urllib.request

ROOT = os.environ['MATOU_DEMO_ROOT']
args = sys.argv[1:]
settings = None
for index, value in enumerate(args):
    if value == '--settings' and index + 1 < len(args):
        settings = args[index + 1]

with open(os.path.join(ROOT, 'roles.queue'), 'r+') as queue:
    fcntl.flock(queue, fcntl.LOCK_EX)
    pending = [line for line in queue.read().splitlines() if line.strip()]
    if not pending:
        print('readme-capture: roles.queue is empty', flush=True)
        sys.exit(1)
    role = pending[0]
    queue.seek(0)
    queue.truncate()
    queue.write('\n'.join(pending[1:]) + '\n')

with open(os.path.join(ROOT, 'launches.log'), 'a') as log:
    log.write(json.dumps({'role': role, 'args': args, 'cwd': os.getcwd()}) + '\n')

spec = json.load(open(os.path.join(ROOT, 'roles.json')))[role]
provider_id = f'demo-{role}'
url = json.load(open(settings))['hooks']['UserPromptSubmit'][0]['hooks'][0]['url']
base = {'session_id': provider_id, 'cwd': os.getcwd()}


def post(payload):
    body = json.dumps(payload).encode()
    request = urllib.request.Request(
        url, data=body, headers={'content-type': 'application/json'}, method='POST')
    urllib.request.urlopen(request, timeout=3).read()


def hook(name, **extra):
    post({'hook_event_name': name, **base, **extra})


with open(os.path.join(ROOT, spec['transcript'] + '.ans'), 'rb') as transcript:
    sys.stdout.buffer.write(b'\x1b[2J\x1b[H' + transcript.read())
    sys.stdout.buffer.flush()

hook('SessionStart')
post({
    **base,
    'permission_mode': spec['permission'],
    'model': {'display_name': spec['model']},
    'cost': {'total_duration_ms': spec['duration_ms']},
    'context_window': {'used_percentage': spec['context'], 'context_window_size': 200000},
    'rate_limits': {'seven_day': {
        'used_percentage': spec['weekly'], 'resets_at': time.time() + spec['resets_in']}}
})
for event in spec['events']:
    kind = event[0]
    if kind == 'tool':
        _, name, tool_id, tool_input, outcome = event
        hook('PreToolUse', tool_name=name, tool_use_id=tool_id, tool_input=tool_input)
        if outcome == 'ok':
            hook('PostToolUse', tool_name=name, tool_use_id=tool_id, tool_input=tool_input)
        elif outcome == 'fail':
            hook('PostToolUseFailure', tool_name=name, tool_use_id=tool_id, tool_input=tool_input)
    else:
        hook(event[1], **event[2])
    time.sleep(0.05)

for line in sys.stdin:
    if line.strip() == 'exit':
        break
