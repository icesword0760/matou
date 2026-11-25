"""Run the real debug desktop and Runtime with hidden macOS windows and an isolated fixture."""

import argparse
import os
from pathlib import Path
import signal
import subprocess
import tempfile

from support.instance_channel import instance_channel


def run(binary, log_path, root, stage=None, capture_dir=None):
    data, home, scratch, workspace = (root / name for name in ('data', 'home', 'tmp', 'workspace'))
    for directory in (data, home, scratch, workspace):
        directory.mkdir(exist_ok=True)
    (data / 'dag-window-probe.fixture').touch()
    env = {
        'HOME': str(home), 'ZDOTDIR': str(home), 'XDG_CONFIG_HOME': str(home),
        'TMPDIR': str(scratch), 'PATH': '/usr/bin:/bin:/usr/sbin:/sbin',
        'GIT_CONFIG_NOSYSTEM': '1', 'MATOU_LOCALE': 'zh-CN',
        'MATOU_DAG_WINDOW_PROBE': 'hidden', 'MATOU_DISABLE_LIVE_EGRESS': '1',
        'HTTP_PROXY': 'http://127.0.0.1:9', 'HTTPS_PROXY': 'http://127.0.0.1:9',
        'ALL_PROXY': 'http://127.0.0.1:9',
    }
    if stage:
        env['MATOU_SPLIT_DAG_I18N_PROBE'] = stage
        env['MATOU_LOCALE'] = 'en'
        env['RUST_BACKTRACE'] = '1'
    if capture_dir:
        env['MATOU_SPLIT_DAG_I18N_CAPTURE'] = str(capture_dir)
    args = [
        str(binary), '--data-root', str(data), '--workspace', str(workspace),
        '--shell-home', str(home), '--shell', '/bin/zsh',
    ]
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open('wb') as log, instance_channel(data) as (endpoint, _):
        log.write(f'argv={args!r}\n'.encode())
        log.flush()
        process = subprocess.Popen(
            args, cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )
        try:
            assert process.wait(timeout=115) == 0
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
    if stage:
        expected = {
            'record': ['split-drag-saved', 'split-restored-on-canvas-return'],
            'restore': [
                'split-restored-on-process-restart',
                'split-viewport width=720', 'split-viewport width=900', 'split-viewport width=1200',
                'dag-viewport width=720', 'dag-viewport width=900', 'dag-viewport width=1200',
                'dag-search-width=720', 'dag-repeat-dismiss-retry', 'dag-reconnecting', 'dag-runtime-recovered',
                'english-start-failure width=720', 'english-start-failure width=900',
                'english-start-failure width=1200', 'english-empty-canvas',
                'split-pruned-missing',
            ],
        }
        for case in expected[stage]:
            assert f'[split-dag-i18n-probe] {case}' in output, case
        return
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
    parser.add_argument('--split-dag-i18n', action='store_true')
    parser.add_argument('--capture-dir', type=Path)
    args = parser.parse_args()
    tmp = Path.cwd() / '.tmp'
    tmp.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='dw', dir=tmp) as root:
        binary, log = args.binary.resolve(strict=True), args.log.resolve()
        if args.split_dag_i18n:
            capture = args.capture_dir.resolve(strict=True) if args.capture_dir else None
            run(binary, log.with_stem(f'{log.stem}-record'), Path(root), 'record')
            run(binary, log.with_stem(f'{log.stem}-restore'), Path(root), 'restore', capture)
        else:
            run(binary, log, Path(root))


if __name__ == '__main__':
    main()
