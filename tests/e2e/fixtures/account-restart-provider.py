#!/usr/bin/env python3
"""Local-only Claude process fixture; hooks and transcripts, never real auth."""
import json, os, sys, time, urllib.request
from pathlib import Path
args = sys.argv[1:]
def arg(name):
    return args[args.index(name)+1] if name in args else None
root = Path(os.environ['MATOU_ACCOUNT_RESTART_FIXTURE'])
sid = arg('--resume') or arg('--session-id') or 'fixture-session'
with (root/'runs.jsonl').open('a') as f:
    f.write(json.dumps({'pid':os.getpid(),'session':sid,'args':args})+'\n')
if arg('--resume') and (root/'fail-restart').exists():
    print('Fixture startup failed',flush=True);sys.exit(1)
settings = json.loads(Path(arg('--settings')).read_text())
url = settings['hooks']['SessionStart'][0]['hooks'][0]['url']
project = Path(os.environ['MATOU_CLAUDE_PROJECTS_ROOT']) / ''.join(c if c.isalnum() else '-' for c in os.getcwd())
project.mkdir(parents=True,exist_ok=True)
transcript = project / (sid+'.jsonl')
if not transcript.exists():
    transcript.write_text(json.dumps({'type':'user','sessionId':sid,'cwd':os.getcwd(),'timestamp':'2026-10-14T04:00:00Z','message':{'role':'user','content':'Synthetic saved conversation'}})+'\n')
def post(event=None):
    payload={'session_id':sid,'cwd':os.getcwd(),'transcript_path':str(transcript),'permission_mode':'default','model':{'display_name':'Claude fixture'},'context_window':{'used_percentage':72},'rate_limits':{'five_hour':{'used_percentage':99}}}
    if event: payload['hook_event_name']=event
    if event=='PreToolUse': payload.update(tool_name='Read',tool_use_id='fixture-busy',tool_input={})
    req=urllib.request.Request(url,data=json.dumps(payload).encode(),headers={'content-type':'application/json'},method='POST')
    urllib.request.urlopen(req,timeout=3).read()
post('SessionStart');post();post('Stop')
print('Fixture Claude ready',flush=True)
busy=False
while True:
    next_busy=(root/'busy').exists()
    if next_busy!=busy:
        busy=next_busy;post('PreToolUse' if busy else 'Stop')
    print('Fixture terminal output',flush=True)
    time.sleep(.3)
