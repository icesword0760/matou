#!/usr/bin/env python3
"""Stand-in for the `claude` CLI used only by readme-capture.spec.ts.

Each launch pops one role from $MATOU_DEMO_ROOT/roles.queue, prints that role's
canned transcript, then replays the role's hook events against Matou's provider
hook endpoint so the HUD, work status and DAG reflect a believable session.
"""
import fcntl
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = os.environ['MATOU_DEMO_ROOT']
args = sys.argv[1:]
settings = None
resume = None
for index, value in enumerate(args):
    if value == '--settings' and index + 1 < len(args):
        settings = args[index + 1]
    if value == '--resume' and index + 1 < len(args):
        resume = args[index + 1]
# A Fork passes the source conversation with --fork-session and the real CLI answers with a new
# identity, so only a plain resume may report the id it was handed.
if '--fork-session' in args:
    resume = None

roles = json.load(open(os.path.join(ROOT, 'roles.json')))

# Restoring a conversation asks for it by name. Answer with that conversation's own role instead of
# taking the next queue entry: the app relaunches restored terminals in whatever order it likes, so
# popping here would hand a restored card someone else's transcript. A Fork is excluded above, so it
# still draws a fresh role from the queue.
restored = resume[len('demo-'):] if resume and resume.startswith('demo-') else None
role = restored if restored in roles else None

if role is None:
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
    log.write(json.dumps(
        {'role': role, 'restored': restored is not None and role == restored,
         'args': args, 'cwd': os.getcwd()}) + '\n')

spec = roles[role]
# Report the conversation the host asked us to resume. Without this a catalog load (which resumes
# a real session id) fails the restore identity handshake and the card shows "Claude Code 恢复失败".
provider_id = resume or f'demo-{role}'
url = json.load(open(settings))['hooks']['UserPromptSubmit'][0]['hooks'][0]['url']
base = {'session_id': provider_id, 'cwd': os.getcwd()}


def post(payload):
    body = json.dumps(payload).encode()
    request = urllib.request.Request(
        url, data=body, headers={'content-type': 'application/json'}, method='POST')
    # A hook post is best-effort telemetry for the demo scene: a dead or slow host must never
    # abort the transcript. urlopen raises URLError for most failures, but a socket timeout on
    # the read surfaces as TimeoutError and a closed socket as a bare OSError.
    try:
        urllib.request.urlopen(request, timeout=3).read()
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        print(f'readme-capture: hook post failed: {error}', file=sys.stderr, flush=True)


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
ran_exec = False
for event in spec['events']:
    kind = event[0]
    if kind == 'exec':
        ran_exec = True
        _, command, label = event
        sys.stdout.write(f"\x1b[32m⏺\x1b[0m \x1b[1mBash({label})\x1b[0m\n")
        sys.stdout.flush()
        # The real `mt` command runs here, so it can hang (a runtime that never answers) or emit
        # bytes that are not valid UTF-8. Neither may wedge the recording: cap the wait and
        # replace undecodable bytes rather than raising.
        try:
            result = subprocess.run(command, shell=True, capture_output=True, text=True,
                                    errors='replace', env=os.environ, timeout=20)
            lines = (result.stdout or result.stderr).splitlines()[:12]
        except subprocess.TimeoutExpired:
            lines = ['(timed out)']
        for index, line in enumerate(lines):
            prefix = '  \x1b[90m⎿\x1b[0m  ' if index == 0 else '     '
            sys.stdout.write(prefix + line + '\n')
        sys.stdout.flush()
        time.sleep(0.4)
        continue
    if kind == 'tool':
        _, name, tool_id, tool_input, outcome = event
        hook('PreToolUse', tool_name=name, tool_use_id=tool_id, tool_input=tool_input)
        if outcome == 'ok':
            hook('PostToolUse', tool_name=name, tool_use_id=tool_id, tool_input=tool_input)
        elif outcome == 'fail':
            hook('PostToolUseFailure', tool_name=name, tool_use_id=tool_id, tool_input=tool_input)
    else:
        hook(event[1], **event[2])
        if ran_exec and event[1] == 'Stop' and 'last_assistant_message' in event[2]:
            sys.stdout.write(f"\x1b[38;5;214m⏺\x1b[0m {event[2]['last_assistant_message']}\n")
            sys.stdout.flush()
    time.sleep(0.05)

for line in sys.stdin:
    if line.strip() == 'exit':
        break
