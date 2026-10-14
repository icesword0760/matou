#!/usr/bin/python3
"""Test-only routing of real Apple security operations into one disposable keychain."""
import os, sys, shlex, subprocess
from pathlib import Path
args = sys.argv[1:]
path = Path(os.environ['MATOU_TEST_KEYCHAIN']).resolve()
assert path.name == 'fixture.keychain-db' and 'matou-local-auth-' in str(path)
if args == ['-i']:
    args = shlex.split(sys.stdin.readline())
assert args and args[0] in ('find-generic-password', 'add-generic-password', 'delete-generic-password')
service = args[args.index('-s')+1] if '-s' in args else ''
assert service.startswith(('Claude Code', 'claude-swap', 'cswap')), 'unexpected service'
# Both CLI and adapter still use their real read/write formats, account names,
# service names and real securityd storage. Only keychain routing is changed.
if args[0] == 'add-generic-password':
    result = subprocess.run(['/usr/bin/security', '-i'], input=shlex.join([*args, str(path)])+'\n', text=True, capture_output=True)
else:
    result = subprocess.run(['/usr/bin/security', *args, str(path)], text=True, capture_output=True)
with (path.parent/'keychain-operations.jsonl').open('a') as log:
    import json
    log.write(json.dumps({'pid':os.getppid(),'op':args[0],'account':args[args.index('-a')+1] if '-a' in args else None,'service':service,'returncode':result.returncode,'error':result.stderr[:200] if result.returncode else ''})+'\n')
sys.stdout.write(result.stdout); sys.stderr.write(result.stderr)
sys.exit(result.returncode)
