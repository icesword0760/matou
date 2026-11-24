#!/usr/bin/env python3
"""Launch one isolated frozen reference for loopback DevTools pixel evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / 'artifacts/native-parity/native/evidence/recovery-water-pixels'
SOURCE = ROOT / 'artifacts/native-parity/reference/MatouParityReference.app'
BASELINE = ROOT / 'artifacts/native-parity/baseline/MatouReference.app'
APP = EVIDENCE / 'MatouPixelReference.app'
IDENTIFIER = 'com.matou.native-parity.pixel-reference'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=9450)
    arguments = parser.parse_args()
    port = arguments.port
    if not 1024 <= port <= 65535:
        parser.error('port must be between 1024 and 65535')
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    if APP.exists() or (EVIDENCE/'launch.json').exists():
        raise SystemExit('This single-use diagnostic already exists; preserve its bundle and evidence.')
    listener = subprocess.run(['lsof', '-nP', f'-iTCP:{port}', '-sTCP:LISTEN'], capture_output=True, text=True)
    if listener.returncode != 1 or listener.stdout:
        raise SystemExit('Selected diagnostic port is not confirmed free: '+listener.stdout+listener.stderr)
    baseline_asar = BASELINE/'Contents/Resources/app.asar'
    source_asar = SOURCE/'Contents/Resources/app.asar'
    manifest = json.loads((ROOT/'artifacts/native-parity/baseline/manifest.json').read_text())
    if sha(baseline_asar) != manifest['asar_sha256']:
        raise SystemExit('Frozen source ASAR does not match the captured manifest')
    harness = json.loads((ROOT/'artifacts/native-parity/reference/home-harness.json').read_text())
    asar_module = ROOT/'node_modules/.pnpm/@electron+asar@3.4.1/node_modules/@electron/asar/lib/asar.js'
    header_script = 'const a=require(process.argv[1]),c=require("node:crypto");process.stdout.write(c.createHash("sha256").update(a.getRawHeader(process.argv[2]).headerString).digest("hex"))'
    header = subprocess.check_output(['node', '-e', header_script, str(asar_module), str(source_asar)], text=True)
    source_info = plistlib.loads((SOURCE/'Contents/Info.plist').read_bytes())
    if header != harness['header_sha256'] or source_info['ElectronAsarIntegrity']['Resources/app.asar']['hash'] != header:
        raise SystemExit('The source home-harness header does not match its existing record/integrity metadata')
    source_asar_sha = sha(source_asar)
    subprocess.run(['cp', '-cR', str(SOURCE), str(APP)], check=True)
    plist = APP/'Contents/Info.plist'
    info = plistlib.loads(plist.read_bytes())
    info['CFBundleIdentifier'] = IDENTIFIER
    info['CFBundleDisplayName'] = 'Matou Pixel Reference'
    plist.write_bytes(plistlib.dumps(info))
    if info['CFBundleName'] != source_info['CFBundleName'] or info['CFBundleExecutable'] != source_info['CFBundleExecutable']:
        raise SystemExit('Bundle executable/name unexpectedly changed')
    if sha(APP/'Contents/Resources/app.asar') != source_asar_sha:
        raise SystemExit('Diagnostic ASAR copy differs from the known home harness')
    subprocess.run(['codesign', '--force', '--sign', '-', str(APP)], check=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(APP)], check=True)
    sandbox = Path(tempfile.mkdtemp(prefix='mtpx-'))
    paths = {name: sandbox/name for name in ['home','data','user-data','workspace','tmp']}
    for path in paths.values():
        path.mkdir()
    overrides = {
        'HOME':str(paths['home']), 'ZDOTDIR':str(paths['home']),
        'MATOU_REFERENCE_HOME':str(paths['home']), 'TMPDIR':str(paths['tmp'])+'/',
        'ELECTRON_USER_DATA_DIR':str(paths['user-data']), 'MATOU_DATA_DIR':str(paths['data']),
        'MATOU_DEFAULT_WORKSPACE':str(paths['workspace']), 'MATOU_DISABLE_AUTO_UPDATE':'1',
        'MATOU_UPDATE_BASE_URL':'http://127.0.0.1:9/native-parity-disabled', 'MATOU_LOCALE':'zh-CN',
    }
    env = dict(os.environ)
    removed = [name for name in env if name.startswith('MATOU_E2E') or name in ['ELECTRON_RUN_AS_NODE','MATOU_RUNTIME_ENTRY']]
    for name in removed:
        env.pop(name)
    env.update(overrides)
    origin = f'http://127.0.0.1:{port}'
    binary = APP/'Contents/MacOS'/info['CFBundleExecutable']
    command = [str(binary), '--user-data-dir='+str(paths['user-data']), '--remote-debugging-address=127.0.0.1', '--remote-debugging-port='+str(port), '--remote-allow-origins='+origin]
    with (EVIDENCE/'app.log').open('wb') as log:
        started = time.time_ns()
        process = subprocess.Popen(command,cwd=paths['workspace'],env=env,stdout=log,stderr=log,start_new_session=True)
    record = {
        'pid':process.pid,'app':str(APP),'bundleIdentifier':IDENTIFIER,'binary':str(binary),
        'port':port,'allowedOrigin':origin,'command':command,'startedUnixNs':started,
        'root':str(sandbox),'paths':{name:str(path) for name,path in paths.items()},
        'environmentOverrides':overrides,'removedEnvironmentKeys':removed,
        'baselineAsarSha256':sha(baseline_asar),'baselineManifestSha256':sha(ROOT/'artifacts/native-parity/baseline/manifest.json'),
        'sourceHarnessAsarSha256':source_asar_sha,'diagnosticAsarSha256':sha(APP/'Contents/Resources/app.asar'),
        'harnessHeaderSha256':header,'knownHarnessRecordSha256':sha(ROOT/'artifacts/native-parity/reference/home-harness.json'),
        'bundleExecutableSha256':sha(binary),'sourceBundleExecutableName':source_info['CFBundleExecutable'],
        'sourceBundleName':source_info['CFBundleName'],'bundleMetadataOnlyKeys':['CFBundleIdentifier','CFBundleDisplayName'],
        'bundlePlistSha256':sha(plist),'launcherSha256':sha(Path(__file__)),
        'oldInstancesOperated':False,'rendererOrProductLogicModified':False,'adhocSignatureVerified':True,
    }
    (EVIDENCE/'launch.json').write_text(json.dumps(record,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps(record,ensure_ascii=False,indent=2))


if __name__=='__main__':
    main()
