"""Opt-in macOS native test: real Keychain + two official CLIs, synthetic server.
Run with the project uv Python. Never uses real account credentials or installs.
"""
import os, sys, json, time, shutil, subprocess, tempfile, pathlib, threading, pty, fcntl, termios, struct, select, re, signal, secrets
import official_fixture as mock

BASE = pathlib.Path(__file__).resolve().parent
run = pathlib.Path(tempfile.mkdtemp(prefix='matou-local-auth-', dir='/private/tmp'))
mock.ROOT = run
mock.STATE.update(run_dir=str(run), refresh_delay=2)
keychain = run/'fixture.keychain-db'
original_search = subprocess.check_output(['/usr/bin/security', 'list-keychains', '-d', 'user'], text=True)
original_default = subprocess.check_output(['/usr/bin/security', 'default-keychain', '-d', 'user'], text=True)
hostenv = dict(os.environ)
processes = []
server = None
result = {'run_dir': str(run), 'backend': 'real-Apple-Keychain', 'routing': 'explicit disposable keychain; original CLI and upstream storage code', 'passed': False}
try:
    # Password is fixture-only, passed on stdin, never a real keychain password.
    import shlex
    def sec(command):
        p = subprocess.run(['/usr/bin/security', '-i'], input=command+'\n', text=True, capture_output=True)
        if p.returncode: raise RuntimeError('fixture keychain setup failed: '+p.stderr[:150])
    sec('create-keychain -p '+secrets.token_hex(24)+' '+shlex.quote(str(keychain)))
    # Creating a keychain may add it to the user's search list. Remove only this
    # fixture immediately; never replace the default keychain or unlock login.
    search = subprocess.check_output(['/usr/bin/security','list-keychains','-d','user'],text=True)
    if str(keychain) in search:
        paths = [p for p in shlex.split(search) if p != str(keychain)]
        subprocess.run(['/usr/bin/security','list-keychains','-d','user','-s',*paths],check=True,capture_output=True)
    for name in ['home','profile','work','tmp','bin']:(run/name).mkdir()
    shutil.copy(BASE/'keychain_security.py',run/'bin/security')
    (run/'bin/security').chmod(0o700)
    subprocess.run(['/usr/bin/openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(run/'fixture-ca.key'),'-out',str(run/'fixture-ca.crt'),'-days','1','-subj','/CN=Matou synthetic fixture','-addext','subjectAltName=DNS:platform.claude.com,DNS:api.anthropic.com,DNS:claude.ai'],capture_output=True,check=True)
    mock.CTX.load_cert_chain(str(run/'fixture-ca.crt'), str(run/'fixture-ca.key'))
    profile=run/'home/.claude'; profile.mkdir(); config=run/'home/.claude.json'
    mock.write_json(config, {'hasCompletedOnboarding':True,'theme':'dark','oauthAccount':mock.identity('A'), 'projects':{str(run/'work'):{'hasTrustDialogAccepted':True}},'fixturePreserved':True})
    server=mock.Server(('127.0.0.1',0),mock.Proxy)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    port=server.server_address[1]
    # All CLI outgoing traffic terminates in this synthetic server, no forwarding.
    sb=run/'isolation.sb'
    realhome=str(pathlib.Path.home())
    sb.write_text(f'''(version 1)
(allow default)
(deny file-write*)
(allow file-write* (subpath "{run}") (literal "/dev/null"))
(deny file-read* (subpath "{realhome}/.claude") (literal "{realhome}/.claude.json") (subpath "{realhome}/.claude-swap-backup"))
(deny process-exec (literal "/usr/bin/open") (literal "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"))
(deny network*)
(allow network-outbound (remote ip "localhost:{port}"))
''')
    env={'USER':'matou-fixture','LOGNAME':'matou-fixture','HOME':str(run/'home'),'TMPDIR':str(run/'tmp'),'PATH':str(run/'bin')+':/usr/bin:/bin','SHELL':'/bin/sh','LANG':'en_US.UTF-8','TERM':'xterm-256color',
      'MATOU_TEST_KEYCHAIN':str(keychain),
      'HTTPS_PROXY':f'http://127.0.0.1:{port}','HTTP_PROXY':f'http://127.0.0.1:{port}','NODE_EXTRA_CA_CERTS':str(run/'fixture-ca.crt'),
      'API_TIMEOUT_MS':'10000','CLAUDE_CODE_MAX_RETRIES':'0','DISABLE_AUTOUPDATER':'1','DISABLE_TELEMETRY':'1','DISABLE_ERROR_REPORTING':'1','CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC':'1','CLAUDE_CODE_TMPDIR':str(run/'tmp')}
    # Isolation readback before loading the adapter; real Keychain read is via
    # explicit path, not a simulated command or a default/search-list lookup.
    for cmd in [['/bin/cat',realhome+'/.claude.json'],['/usr/bin/curl','--noproxy','*','--max-time','2','https://1.1.1.1']]:
        assert subprocess.run(['/usr/bin/sandbox-exec','-f',str(sb),*cmd],env=env,capture_output=True).returncode != 0
    os.environ.clear(); os.environ.update(env)
    sys.path.insert(0,str(BASE.parent))
    from local_engine import LocalAccountSwitcher
    from claude_swap import macos_keychain
    macos_keychain._SECURITY=str(run/'bin/security')
    engine=LocalAccountSwitcher()
    def install(a, expired=False):
        c=mock.credentials(a,expired); c['mcpOAuth']={'fixture':'preserved'}
        engine._write_credentials(json.dumps(c))
        value=json.loads(config.read_text());value['oauthAccount']=mock.identity(a);mock.write_json(config,value)
    import contextlib,io
    with contextlib.redirect_stdout(io.StringIO()):
        install('A');engine.add_account();install('B',True);engine.add_account();engine.switch_to('1',json_output=True)
    assert not (profile/'.credentials.json').exists(), 'unexpected file fallback'
    probe = subprocess.run(['/usr/bin/sandbox-exec', '-f', str(sb), str(run/'bin/security'), 'find-generic-password', '-a', macos_keychain.keychain_account_name(), '-s', 'Claude Code-credentials', '-w'], env=env, capture_output=True, text=True)
    result['sandbox_keychain_probe'] = {'code': probe.returncode, 'fixture_value': 'fixture-A-valid' in probe.stdout, 'error': probe.stderr[:300]}
    assert probe.returncode == 0, 'sandbox Keychain fixture routing failed'

    for i in range(2):
        collector=run/f'collector-{i}.sh'
        collector.write_text(f'#!/bin/sh\n/bin/cat >> "{run}/status-{i}.jsonl"\n/bin/echo >> "{run}/status-{i}.jsonl"\n');collector.chmod(0o700)
        args=['/usr/bin/sandbox-exec','-f',str(sb),mock.CLI,'--model','sonnet','--tools','','--permission-mode','default','--strict-mcp-config','--mcp-config','{"mcpServers":{}}','--debug-file',str(run/f'cli-{i}.debug'),'--settings',json.dumps({'disableDeepLinkRegistration':'disable','statusLine':{'type':'command','command':str(collector)}})]
        master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',35,130,0,0))
        proc=subprocess.Popen(args,cwd=run/'work',env=env,stdin=slave,stdout=slave,stderr=slave,start_new_session=True);os.close(slave)
        processes.append({'proc':proc,'fd':master,'output':b''})
    def pump(seconds):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            fds=[p['fd'] for p in processes]
            for fd in select.select(fds,[],[],.1)[0]:
                p=next(p for p in processes if p['fd']==fd)
                try:p['output']+=os.read(fd,65536)
                except OSError:pass
    def plain(p):return re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',p['output'].decode(errors='replace'))
    def prompts(text):
        for p in processes:os.write(p['fd'],text.encode())
        pump(.3)
        for p in processes:os.write(p['fd'],b'\r')
    def until(predicate,seconds):
        end=time.monotonic()+seconds
        while time.monotonic()<end and not predicate():pump(.2)
        assert predicate(),'fixture condition timeout'
    pump(5)
    for p in processes:
        if 'Yes, I trust this folder' in plain(p):os.write(p['fd'],b'\r')
    pump(1);prompts('HISTORY_A_MARKER: remember blue triangle.')
    until(lambda:all('RESPONSE_fixture-A-valid' in plain(p) for p in processes),20)
    engine.switch_to('2',json_output=True);mock.STATE['phase']='after-swap'
    pump(36);prompts('TURN_B_MARKER: continue the same conversation.')
    until(lambda:any('/oauth/token' in e.get('path','') for e in mock.EVENTS),10)
    started=time.monotonic()
    # Switch during real CLI refresh: upstream must wait for the official lock,
    # save the renewed B pair, and then atomically activate A, not lose B's grant.
    engine.switch_to('1',json_output=True)
    result['switch_during_refresh_wait_ms']=round((time.monotonic()-started)*1000)
    b=json.loads(engine._read_account_credentials('2',mock.identity('B')['emailAddress']))['claudeAiOauth']
    result['rotated_b_preserved']=b['refreshToken']=='fixture-B-refresh-new'
    result['active_a_after_race']=json.loads(engine._read_credentials())['claudeAiOauth']['accessToken']=='fixture-A-valid'
    engine.switch_to('2',json_output=True)
    pump(37);prompts('AFTER_RENEWAL_MARKER: continue one more time.')
    until(lambda:all('RESPONSE_fixture-B-renewed' in plain(p) for p in processes),20)
    pump(3)
    result.update(
      cli_version=subprocess.check_output([mock.CLI,'--version'],env=env,text=True).strip(),
      pids=[p['proc'].pid for p in processes], same_processes_alive=all(p['proc'].poll() is None for p in processes),
      b_refresh_requests=sum('/oauth/token' in e.get('path','') and e.get('refresh')=='fixture-B-refresh' for e in mock.EVENTS),
      b_followups_keep_history=sum(e.get('credential')=='fixture-B-renewed' and e.get('history_a') and e.get('turn_c') for e in mock.EVENTS),
      plaintext_credentials_created=(profile/'.credentials.json').exists(),
      mcp_preserved=json.loads(engine._read_credentials()).get('mcpOAuth')=={'fixture':'preserved'},
      config_preserved=json.loads(config.read_text()).get('fixturePreserved') is True,
      cswap_network_requests=sum('claude-swap' in e.get('user_agent','') for e in mock.EVENTS),
      session_ids=[list({s['session_id'] for s in mock.snapshots(run/f'status-{i}.jsonl') if s.get('session_id')}) for i in range(2)])
    assert result['b_refresh_requests']==1 and result['b_followups_keep_history']>=2
    assert result['rotated_b_preserved'] and result['active_a_after_race'] and result['mcp_preserved'] and result['config_preserved']
    assert not result['plaintext_credentials_created'] and not result['cswap_network_requests']
    assert result['same_processes_alive'] and all(len(s)==1 for s in result['session_ids'])
    # Native rollback: fail the config splice after the target credential write.
    from unittest.mock import patch
    from claude_swap.exceptions import SwitchError
    before_creds = engine._read_credentials(); before_config = config.read_bytes()
    original_write = engine._write_json
    failed = [False]
    def interrupted(path, value, *args, **kwargs):
        if pathlib.Path(path) == config and not failed[0]:
            failed[0] = True
            raise OSError('fixture interrupted config write')
        return original_write(path, value, *args, **kwargs)
    with patch.object(engine, '_write_json', side_effect=interrupted):
        try:
            engine.switch_to('1', json_output=True)
            raise AssertionError('expected injected failure')
        except SwitchError:
            pass
    result['real_keychain_rollback'] = engine._read_credentials() == before_creds and config.read_bytes() == before_config
    assert result['real_keychain_rollback']
    result['passed']=True
except BaseException as e:
    result['error']=type(e).__name__+': '+str(e)
finally:
    for i,p in enumerate(processes):
        if p['proc'].poll() is None:
            os.killpg(p['proc'].pid,signal.SIGTERM)
            try:p['proc'].wait(timeout=4)
            except subprocess.TimeoutExpired:os.killpg(p['proc'].pid,signal.SIGKILL);p['proc'].wait()
        os.close(p['fd']);(run/f'terminal-{i}.txt').write_text(p['output'].decode(errors='replace'))
    if server:server.shutdown();server.server_close()
    subprocess.run(['/usr/bin/security','delete-keychain',str(keychain)],env=hostenv,capture_output=True)
    after=subprocess.check_output(['/usr/bin/security','list-keychains','-d','user'],env=hostenv,text=True)
    default=subprocess.check_output(['/usr/bin/security','default-keychain','-d','user'],env=hostenv,text=True)
    result['search_list_unchanged']=after==original_search
    result['default_keychain_unchanged']=default==original_default
    result['test_keychain_removed']=not keychain.exists()
    result['requests']=mock.EVENTS
    (run/'result.json').write_text(json.dumps(result,indent=2))
    print(json.dumps({k:v for k,v in result.items() if k!='requests'},indent=2))
    sys.exit(0 if result['passed'] and result['search_list_unchanged'] and result['default_keychain_unchanged'] else 1)
