"""Bundle and launch the native Mac acceptance build with disposable data.

This does not install over the production application. Build the binary first;
the bundle has a distinct identity and its own persistent acceptance directory.
"""
from pathlib import Path
import argparse
import json
import os
import plistlib
import subprocess
import tempfile
import time

project = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
args = parser.parse_args()
binary = args.binary.resolve(strict=True)
runtime_binary = (binary.parent / 'matou-native-runtime').resolve(strict=True)
for source in (binary, runtime_binary):
    if not source.is_file():
        raise SystemExit(f"Required native executable is not a file: {source}")
destination = project / 'artifacts/native-parity/native'
destination.mkdir(parents=True, exist_ok=True)
launch = destination / 'launch.json'
if launch.exists():
    previous = json.loads(launch.read_text())
    try:
        os.kill(previous['pid'], 0)
    except ProcessLookupError:
        pass
    else:
        raise SystemExit(f"Native acceptance process {previous['pid']} is still running; quit it before rebuilding the bundle")

environment = destination / 'environment.json'
if environment.exists():
    root = Path(json.loads(environment.read_text())['root'])
else:
    root = Path(tempfile.mkdtemp(prefix='matou-parity-native-'))
    environment.write_text(json.dumps({'root': str(root)}) + '\n')
paths = {name: root / name for name in ('home', 'workspace', 'data', 'cache', 'tmp')}
for path in paths.values():
    path.mkdir(parents=True, exist_ok=True)
app = destination / 'MatouNative.app'
executable = app / 'Contents/MacOS/MatouNative'
runtime_executable = executable.parent / 'matou-native-runtime'
executable.parent.mkdir(parents=True, exist_ok=True)
resources = app / 'Contents/Resources'
resources.mkdir(parents=True, exist_ok=True)
# APFS clones keep this immutable launched copy without duplicating the build.
subprocess.run(['cp', '-c', str(binary), str(executable)], check=True)
executable.chmod(0o755)
subprocess.run(['cp', '-c', str(runtime_binary), str(runtime_executable)], check=True)
runtime_executable.chmod(0o755)
info = {
    'CFBundleIdentifier': 'com.matou.native-parity',
    'CFBundleName': 'Matou Native',
    'CFBundleDisplayName': 'Matou Native',
    'CFBundleExecutable': 'MatouNative',
    'CFBundlePackageType': 'APPL',
    'CFBundleVersion': '0.1.0',
    'CFBundleShortVersionString': '0.1.0',
    'NSHighResolutionCapable': True,
    'NSPrincipalClass': 'NSApplication',
    'LSMinimumSystemVersion': '12.0',
}
with (app / 'Contents/Info.plist').open('wb') as file:
    plistlib.dump(info, file)
subprocess.run(['codesign', '--force', '--sign', '-', str(runtime_executable)], check=True)
subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
env = dict(os.environ)
env.update(HOME=str(paths['home']), TMPDIR=str(paths['tmp']), XDG_CACHE_HOME=str(paths['cache']))
command = [str(executable), '--data-root', str(paths['data']), '--workspace', str(paths['workspace']), '--shell-home', str(paths['home'])]
with (destination / 'runtime.log').open('ab') as log:
    process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
record = {'pid': process.pid, 'app': str(app), 'binary': str(binary), 'runtime_binary': str(runtime_binary), 'runtime_executable': str(runtime_executable), 'paths': {key: str(path) for key, path in paths.items()}, 'started_unix_ns': time.time_ns()}
launch.write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record, indent=2))
