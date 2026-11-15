import os,pty,select,time,json,pathlib,subprocess,signal,fcntl,termios,struct,re,hashlib,datetime
root=pathlib.Path('.cache/c10').resolve();exe=subprocess.check_output(['which','codex'],text=True).strip()
reports=[]
for enabled in [True,False]:
 d=root/('update-'+str(enabled).lower());home=d/'codex-home';home.mkdir(parents=True,exist_ok=True)
 repo=root/'model/repo';config=(root/'model/codex-home/config.toml').read_bytes();(home/'config.toml').write_bytes(config)
 seed={'latest_version':'999.0.0','last_checked_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'dismissed_version':None}
 (home/'version.json').write_text(json.dumps(seed))
 args=[exe,'--no-daemon','--no-alt-screen','-C',str(repo),'-c','check_for_update_on_startup='+str(enabled).lower()]
 master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',40,140,0,0))
 proc=subprocess.Popen(args,stdin=slave,stdout=slave,stderr=slave,env=dict(os.environ,CODEX_HOME=str(home),TERM='xterm-256color'),start_new_session=True);os.close(slave)
 output=b'';start=time.monotonic()
 while time.monotonic()-start<5:
  ready,_,_=select.select([master],[],[],.2)
  if ready:
   try:data=os.read(master,65536)
   except OSError:break
   output+=data
   if b'\x1b[6n' in data:os.write(master,b'\x1b[1;1R')
  if proc.poll() is not None:break
 if proc.poll() is None:os.killpg(proc.pid,signal.SIGTERM)
 proc.wait(timeout=3);os.close(master)
 (d/'tui.raw').write_bytes(output)
 plain=re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',output.decode(errors='replace'))
 (d/'tui.txt').write_text(plain)
 report={'checkForUpdateOnStartup':enabled,'upgradePrompt':bool(re.search('Update available|Update now|999.0.0',plain)),'configUnchanged':(home/'config.toml').read_bytes()==config,'exitCode':proc.returncode,'versionSeed':seed}
 reports.append(report);print(json.dumps(report),flush=True)
(root/'update-summary.json').write_text(json.dumps(reports,indent=2))
