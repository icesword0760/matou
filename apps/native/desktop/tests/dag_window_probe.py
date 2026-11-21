"""Run the real debug desktop and Runtime with hidden macOS windows and an isolated fixture."""

import argparse
import os
from pathlib import Path
import signal
import subprocess
import tempfile


def run(binary, log_path, root):
    data, home, scratch, workspace = (root / name for name in ('data', 'home', 'tmp', 'workspace'))
    for directory in (data, home, scratch, workspace):
        directory.mkdir()
    (data / 'dag-window-probe.fixture').touch()
    endpoint = data / '.instance/open.sock'
    assert len(os.fsencode(endpoint)) < 104, endpoint
    env = {
        'HOME': str(home), 'ZDOTDIR': str(home), 'XDG_CONFIG_HOME': str(home),
        'TMPDIR': str(scratch), 'PATH': '/usr/bin:/bin:/usr/sbin:/sbin',
        'GIT_CONFIG_NOSYSTEM': '1', 'MATOU_LOCALE': 'zh-CN',
        'MATOU_DAG_WINDOW_PROBE': 'hidden',
        'HTTP_PROXY': 'http://127.0.0.1:9', 'HTTPS_PROXY': 'http://127.0.0.1:9',
        'ALL_PROXY': 'http://127.0.0.1:9',
    }
    args = [
        str(binary), '--data-root', str(data), '--workspace', str(workspace),
        '--shell-home', str(home), '--shell', '/bin/zsh',
    ]
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open('wb') as log:
        log.write(f'argv={args!r}\n'.encode())
        log.flush()
        process = subprocess.Popen(
            args, cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )
        try:
            assert process.wait(timeout=80) == 0
        finally:
            if process.poll() != 0:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
            print(log_path.read_text(), end='')
    assert not endpoint.exists(), 'clean exit left a socket'
    output = log_path.read_text()
    assert not any(
        'sequence' in line.lower() and any(marker in line.lower() for marker in (
            'diagnostic', 'acknowledgement exceeds', 'sequence gap',
        ))
        for line in output.splitlines()
    ), output
    prefix = '[dag-window-probe] '
    cases = [line[len(prefix):].split()[0] for line in output.splitlines()
             if line.startswith(prefix)]
    assert cases == [
        'open-icon', 'repeat', 'live-update', 'main-switch', 'main-switch-back',
        'retarget', 'close-system', 'reopen', 'close-escape', 'close-button',
        'keep-open-cmd-shift-w', 'close-cmd-w',
        'open-failure', 'dismiss-error', 'open-failure-before-retry', 'retry-success',
        'canvas-zoom', 'canvas-pan', 'canvas-search-refresh', 'canvas-search-output',
        'canvas-reconnect', 'canvas-search-escape', 'canvas-aggregate', 'canvas-search-enter',
    ], cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('apps/native/target/debug/matou-native-desktop'))
    parser.add_argument('--log', type=Path, required=True)
    args = parser.parse_args()
    tmp = Path.cwd() / '.tmp'
    tmp.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='dw', dir=tmp) as root:
        run(args.binary.resolve(strict=True), args.log.resolve(), Path(root))


if __name__ == '__main__':
    main()
