#!/usr/bin/env python3
"""Run the frozen product with isolated test data; never install or update it."""
import argparse, json, os, plistlib, subprocess, tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--asar-module', help='Existing @electron/asar lib/asar.js; needed on first preparation')
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
base = root / 'artifacts/native-parity'
source = base / 'baseline/MatouReference.app'
run = base / 'reference'
app = run / 'MatouParityReference.app'
run.mkdir(parents=True, exist_ok=True)
if not app.exists():
    subprocess.run(['cp', '-cR', str(source), str(app)], check=True)
    plist = app / 'Contents/Info.plist'
    info = plistlib.loads(plist.read_bytes())
    info['CFBundleIdentifier'] = 'com.matou.parity-reference'
    info['CFBundleDisplayName'] = 'Matou Parity Reference'
    plist.write_bytes(plistlib.dumps(info))
# macOS Unix socket paths must fit sockaddr_un; the repository path is too long.
environment_file = run / 'environment.json'
if environment_file.exists():
    sandbox = Path(json.loads(environment_file.read_text())['root'])
else:
    sandbox = Path(tempfile.mkdtemp(prefix='matou-parity-ref-'))
    environment_file.write_text(json.dumps({'root': str(sandbox)}) + '\n')
paths = {name: sandbox / name for name in ['home', 'data', 'user-data', 'workspace', 'tmp']}
for path in paths.values():
    path.mkdir(exist_ok=True)
env = {**os.environ, 'HOME': str(paths['home']),
       'ZDOTDIR': str(paths['home']),
       'MATOU_REFERENCE_HOME': str(paths['home']),
       'TMPDIR': str(paths['tmp']) + '/',
       'ELECTRON_USER_DATA_DIR': str(paths['user-data']),
       'MATOU_DATA_DIR': str(paths['data']),
       'MATOU_DEFAULT_WORKSPACE': str(paths['workspace']),
       'MATOU_DISABLE_AUTO_UPDATE': '1',
       'MATOU_UPDATE_BASE_URL': 'http://127.0.0.1:9/native-parity-disabled',
       'MATOU_LOCALE': 'zh-CN'}
# Ordinary product behavior, not E2E mock/diagnostic/close overrides.
for name in list(env):
    if name.startswith('MATOU_E2E') or name in ['ELECTRON_RUN_AS_NODE', 'MATOU_RUNTIME_ENTRY']:
        env.pop(name)
if not (run / 'home-harness.json').exists():
    if not args.asar_module:
        raise SystemExit('First preparation needs --asar-module <installed @electron/asar/lib/asar.js>')
    subprocess.run(['node', str(root / 'tooling/native-parity/isolate-reference-home.cjs'),
                    args.asar_module, str(base)], check=True)
    plist = app / 'Contents/Info.plist'
    info = plistlib.loads(plist.read_bytes())
    info['ElectronAsarIntegrity']['Resources/app.asar']['hash'] = json.loads(
        (run / 'home-harness.json').read_text())['header_sha256']
    plist.write_bytes(plistlib.dumps(info))
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
with (run / 'app.log').open('ab') as log:
    proc = subprocess.Popen([str(app / 'Contents/MacOS' / info['CFBundleExecutable']),
                             '--user-data-dir=' + str(paths['user-data'])],
                            cwd=paths['workspace'], env=env,
                            stdout=log, stderr=log, start_new_session=True)
record = {'pid': proc.pid, 'app': str(app), 'paths': {k: str(v) for k,v in paths.items()},
          'baseline': str(source), 'product_logic_modified': False,
          'home_path_harness': str(run / 'home-harness.json'),
          'bundle_metadata_changes': ['CFBundleIdentifier', 'CFBundleDisplayName', 'ad-hoc signing']}
(run / 'launch.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record, indent=2))
