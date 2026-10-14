"""Black-box packaged sidecar smoke. Synthetic file fallback, no real Keychain."""
import json, os, pathlib, subprocess, sys, tempfile
engine = pathlib.Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix='matou-packaged-auth-') as directory:
    root=pathlib.Path(directory).resolve();home=root/'home';home.mkdir();profile=home/'.claude';profile.mkdir();tmp=root/'tmp';tmp.mkdir()
    sb=root/'isolation.sb'
    sb.write_text(f'''(version 1)
(allow default)
(deny file-write*)
(allow file-write* (subpath "{root}") (literal "/dev/null"))
(deny process-exec (literal "/usr/bin/security"))
(deny mach-lookup (global-name "com.apple.securityd") (global-name "com.apple.securityd.xpc") (global-name "com.apple.SecurityServer"))
(deny network*)
''')
    env={'HOME':str(home),'PATH':'/usr/bin:/bin','TMPDIR':str(tmp),'LANG':'en_US.UTF-8'}
    def call(action, **values):
        p=subprocess.run(['/usr/bin/sandbox-exec','-f',str(sb),str(engine),str(root/'adapter')],input=json.dumps({'action':action,**values})+'\n',text=True,env=env,cwd=root,capture_output=True,timeout=15)
        assert p.returncode == 0, (action,p.returncode,p.stderr[:120])
        output=json.loads(p.stdout);assert output['ok'],(action,output)
        assert 'fixture-access' not in p.stdout and 'fixture-refresh' not in p.stdout
        return output['value']
    def login(name):
        (home/'.claude.json').write_text(json.dumps({'oauthAccount':{'emailAddress':f'{name}@example.invalid','organizationUuid':name,'accountUuid':name},'keep':True}))
        (profile/'.credentials.json').write_text(json.dumps({'claudeAiOauth':{'accessToken':'fixture-access-'+name,'refreshToken':'fixture-refresh-'+name,'expiresAt':1},'mcpOAuth':{'fixture':True}}))
    assert call('version')['engineVersion']=='0.27.0b1'
    # Seed file-fallback backups with the source library's Linux adapter. macOS
    # capture deliberately rejects a denied Keychain rather than trusting stale
    # plaintext. The packaged switch/refresh/read paths below remain black-box.
    from unittest.mock import patch
    import contextlib, io
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
    import bridge
    from claude_swap.models import Platform
    with patch.dict(os.environ, env, clear=True), patch('claude_swap.models.Platform.detect', return_value=Platform.LINUX), patch('claude_swap.switcher.get_backup_root', return_value=home/'.claude-swap-backup'), contextlib.redirect_stdout(io.StringIO()):
        login('a'); a=bridge.dispatch({'action':'snapshot'},root/'adapter')['detected']['id']
        bridge.dispatch({'action':'save-current','id':a},root/'adapter')
        login('b'); b=bridge.dispatch({'action':'snapshot'},root/'adapter')['detected']['id']
        bridge.dispatch({'action':'save-current','id':b},root/'adapter')
    call('refresh')
    assert call('switch',id=a)['currentId']==a
    creds=json.loads((profile/'.credentials.json').read_text())
    assert creds['claudeAiOauth']['accessToken']=='fixture-access-a'
    assert creds['mcpOAuth']=={'fixture':True}
    assert json.loads((home/'.claude.json').read_text())['keep'] is True
    assert not call('snapshot')['accounts'][0]['needsLogin'] # expiry alone is not revocation
    print(json.dumps({'passed':True,'expired_target_switch':True,'network':'OS-denied','keychain':'OS-denied','mcp_preserved':True,'engine':str(engine)}))
