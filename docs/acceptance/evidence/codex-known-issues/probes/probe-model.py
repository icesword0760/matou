import os,pty,select,time,json,pathlib,subprocess,signal,fcntl,termios,struct,re
root=pathlib.Path('.cache/c10').resolve()
exe=subprocess.check_output(['which','codex'],text=True).strip()
print(subprocess.check_output([exe,'--version'],text=True).strip(),flush=True)
capture=root/'capture.py'
capture.write_text('import sys,json,pathlib\np=json.load(sys.stdin)\nwith pathlib.Path(sys.argv[1]).open("a") as f: f.write(json.dumps(p)+"\\n")\n')
from http.server import HTTPServer,BaseHTTPRequestHandler
import threading
class Handler(BaseHTTPRequestHandler):
 def do_POST(self):
  body=b'{"error":{"message":"The model matou-probe-missing does not exist","type":"invalid_request_error","code":"model_not_found"}}'
  self.send_response(404);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 def log_message(self,*args): pass
server=HTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=server.serve_forever,daemon=True).start()
for name in ['model']:

 d=root/name; repo=d/'repo'; home=d/'codex-home'; repo.mkdir(parents=True,exist_ok=True); home.mkdir(exist_ok=True)
 subprocess.run(['git','init','-q',str(repo)],check=True)
 cmd=f'python3 {capture} {d}/hooks.jsonl'
 hooks='{ '+', '.join(f'{k} = [{{ hooks = [{{ type = "command", command = {json.dumps(cmd)}, timeout = 3 }}] }}]' for k in ['SessionStart','UserPromptSubmit','PreToolUse','PermissionRequest','PostToolUse','Stop','SessionEnd'])+' }'
 config=f'''model = "gpt-6-astra"
model_provider = "local_probe"
approval_policy = "on-request"
sandbox_mode = "read-only"
[projects.{json.dumps(str(repo))}]
trust_level = "trusted"
[model_providers.local_probe]
name = "Local failure probe"
base_url = "http://127.0.0.1:{server.server_port}/v1"
requires_openai_auth = false
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 1000
'''
 (home/'config.toml').write_text(config)
 args=[exe,'--no-daemon','--no-alt-screen','--dangerously-bypass-hook-trust','-C',str(repo),'-c','check_for_update_on_startup=false','-c','hooks='+hooks,'Reply only READY.']
 master,slave=pty.openpty(); fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',40,140,0,0))
 env=dict(os.environ,CODEX_HOME=str(home),TERM='xterm-256color',NO_COLOR='1')
 proc=subprocess.Popen(args,stdin=slave,stdout=slave,stderr=slave,env=env,start_new_session=True);os.close(slave)
 start=time.monotonic(); output=b''; exited=False; sent=False; killed=False
 while time.monotonic()-start<35:
  ready,_,_=select.select([master],[],[],.2)
  if ready:
   try: data=os.read(master,65536)
   except OSError: break
   output+=data
   if b'\x1b[6n' in data: os.write(master,b'\x1b[1;1R')
  elapsed=time.monotonic()-start
  # A deliberately killed process never runs a shell command or model tool.
  if name=='process-exit' and elapsed>7 and not killed:
   os.killpg(proc.pid,signal.SIGKILL);killed=True
  if name=='model' and elapsed>12 and not sent:
   os.write(master,b'\x03');time.sleep(.3);os.write(master,b'\x03');sent=True
  if proc.poll() is not None: break
 if proc.poll() is None:
  os.killpg(proc.pid,signal.SIGTERM)
  try:proc.wait(timeout=3)
  except:os.killpg(proc.pid,signal.SIGKILL)
 code=proc.wait();os.close(master)
 (d/'tui.raw').write_bytes(output)
 plain=re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',output.decode(errors='replace'))
 (d/'tui.txt').write_text(plain)
 rows=[]
 for f in home.glob('sessions/**/*.jsonl'):
  for line in f.read_text().splitlines():
   r=json.loads(line)
   if r.get('type')=='event_msg':rows.append(r)
 (d/'events.json').write_text(json.dumps(rows,indent=2))
 hooksrows=[json.loads(l) for l in (d/'hooks.jsonl').read_text().splitlines()] if (d/'hooks.jsonl').exists() else []
 summary={'name':name,'pid':proc.pid,'exitCode':code,'elapsed':time.monotonic()-start,'hookEvents':[r.get('hook_event_name') for r in hooksrows], 'stopPayloads':[r for r in hooksrows if r.get('hook_event_name')=='Stop'],'eventTypes':[r['payload'].get('type') for r in rows]}
 (d/'summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary),flush=True)
