"""Run the real debug desktop and its real Runtime in isolated, hidden macOS processes."""
import argparse
import contextlib
import fcntl
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import stat
import struct
import subprocess
import tempfile
import time

from support.instance_channel import instance_channel


PROBE = '[instance-probe] '
MOUNT = '''
    SELECT w.id, s.id, m.id, w.root_directory, e.cwd
    FROM window_navigation n
    JOIN workspaces w ON w.id = n.active_workspace_id
    JOIN window_workspace_focus wf ON wf.window_id = n.window_id AND wf.workspace_id = w.id
    JOIN window_task_focus tf ON tf.window_id = n.window_id AND tf.task_id = wf.active_task_id
    JOIN window_scene_focus sf ON sf.window_id = n.window_id AND sf.scene_id = tf.active_scene_id
    JOIN sessions s ON s.id = sf.active_session_id
    JOIN session_mounts m ON m.session_id = s.id AND m.scene_id = sf.scene_id
    JOIN execution_contexts e ON e.id = s.execution_context_id
    WHERE n.window_id = 'main-window-1'
'''


def until(description, check):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            if result := check():
                return result
        except (FileNotFoundError, sqlite3.OperationalError):
            pass
        time.sleep(0.025)
    raise AssertionError(f'timed out waiting for {description}')


def children(pid):
    result = subprocess.run(['/usr/bin/pgrep', '-P', str(pid)], capture_output=True, text=True)
    assert result.returncode in (0, 1), result.stderr
    return [int(child) for child in result.stdout.split()]


class Fixture:
    def __init__(self, binary, root, logs, channels):
        self.binary = binary
        self.root = root
        self.logs = logs
        self.data = root / 'data'
        self.home = root / 'home'
        self.temp = root / 'tmp'
        self.default = root / 'default'
        self.a = root / 'A 中文 %'
        self.b = root / 'B'
        self.c = root / 'C'
        self.env = {
            'HOME': str(self.home), 'TMPDIR': str(self.temp),
            'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'LC_ALL': 'en_US.UTF-8',
            'XDG_CONFIG_HOME': str(self.home), 'ZDOTDIR': str(self.home),
            'HTTP_PROXY': 'http://127.0.0.1:9', 'HTTPS_PROXY': 'http://127.0.0.1:9',
            'ALL_PROXY': 'http://127.0.0.1:9', 'GIT_CONFIG_NOSYSTEM': '1',
        }
        for folder in (self.data, self.home, self.temp, self.default, self.a, self.b, self.c):
            folder.mkdir(parents=True)
        self.endpoint, self.lock = channels.enter_context(instance_channel(self.data))

    def argv(self, paths, data=None):
        argv = [str(self.binary), '--data-root', str(data or self.data), '--workspace',
                str(self.default), '--shell-home', str(self.home), '--shell', '/bin/zsh']
        for path in paths:
            argv.extend(['--open-workspace', str(path)])
        return argv

    def launch(self, label, *paths, data=None, success=True):
        argv = self.argv(paths, data)
        with subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              cwd=self.root, env={**self.env, 'MATOU_WINDOW_REGISTRY_PROBE': 'hidden'},
                              text=True, start_new_session=True) as process:
            try:
                output, error = process.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate(timeout=5)
                raise
        log = f'argv={argv!r}\npid={process.pid} exit={process.returncode}\n{output}{error}'
        (self.logs / f'{label}.log').write_text(log)
        assert (process.returncode == 0) == success, log
        # A second desktop must exit before creating even a hidden window or child.
        assert PROBE not in error and not children(process.pid), log
        if success:
            assert not error, log
        return error

    @contextlib.contextmanager
    def owner(self, label, *paths, mode='hidden'):
        path = self.logs / f'{label}.log'
        argv = self.argv(paths)
        with path.open('wb') as log:
            log.write(f'argv={argv!r}\nmode={mode}\n'.encode())
            log.flush()
            process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=log, stderr=log,
                                       cwd=self.root, env={**self.env, 'MATOU_WINDOW_REGISTRY_PROBE': mode},
                                       start_new_session=True)
            try:
                run = Running(process, path)
                run.wait(f'owner pid={process.pid}')
                yield run
                run.release()
                assert process.wait(timeout=12) == 0, path.read_text()
                assert not self.endpoint.exists(), 'clean exit left a socket'
                if run.runtime:
                    assert not pid_exists(run.runtime), 'Runtime survived orderly exit'
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                process.stdin.close()

    def sql(self, query, args=()):
        db = self.data / 'matou.sqlite'
        with contextlib.closing(sqlite3.connect(f'{db.as_uri()}?mode=ro', uri=True, timeout=1)) as conn:
            return conn.execute(query, args).fetchall()

    def mount(self, expected):
        def matches():
            rows = self.sql(MOUNT)
            return rows[0][:3] if len(rows) == 1 and rows[0][3:] == (str(expected), str(expected)) else None
        return until(f'active workspace and mounted cwd {expected.name}', matches)

    def count(self, path):
        return self.sql('SELECT count(*) FROM workspaces WHERE root_directory=? AND archived_at IS NULL',
                        (str(path),))[0][0]

    def verify_owner(self, owner, windows=1):
        assert owner.process.poll() is None
        assert children(owner.process.pid) == [owner.runtime]
        assert sum(line.startswith('window opened windows=1') for line in owner.lines()) == windows, owner.lines()
        assert sum(line.startswith('native visible=false') for line in owner.lines()) == windows
        assert not any(line.startswith('native visible=true') for line in owner.lines())
        assert self.sql('SELECT id FROM app_windows ORDER BY id') == [('main-window-1',)]


def pid_exists(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


class Running:
    def __init__(self, process, log):
        self.process = process
        self.log = log
        self.runtime = None

    def release(self):
        self.process.stdin.write(b'\n')
        self.process.stdin.flush()

    def lines(self):
        return self.log.read_text().split(PROBE)[1:]

    def wait(self, message, count=1):
        def matches():
            assert self.process.poll() is None, self.log.read_text()
            lines = [line.splitlines()[0] for line in self.lines() if message in line]
            return lines if len(lines) >= count else None
        return until(message, matches)

    def initial(self, windows):
        line = self.wait(f'initial windows={windows} runtime=')[0]
        self.runtime = int(line.split('runtime=')[1])
        assert self.runtime in children(self.process.pid)
        if windows:
            self.window = self.wait('window opened windows=1 id=')[0].split(' id=')[1]

    def focuses(self, count):
        lines = self.wait('focus windows=1 id=', count)
        assert len(lines) == count and all(line.endswith(self.window) for line in lines), lines

    def dispatched(self):
        return [line.splitlines()[0].split(' path=', 1)[1]
                for line in self.lines() if line.startswith('dispatch ready generation=1 path=')]


def check(label, **values):
    print(f'{label}: ' + json.dumps(values, ensure_ascii=False), flush=True)


def cases(binary, temp, logs, channels):
    same = Fixture(binary, temp / 'a', logs, channels)
    old_pid = None
    with same.owner('ready-owner') as owner:
        owner.initial(1)
        same.mount(same.default)
        old_pid = owner.process.pid
        same.launch('single', same.a)
        identity = same.mount(same.a)
        same.verify_owner(owner)
        owner.focuses(1)
        assert same.count(same.a) == 1 and owner.dispatched() == [str(same.a)]
        check('single', owner=old_pid, runtime=owner.runtime, windows=1, active=same.a.name, cwd=same.a.name, focus=owner.window)

        same.launch('duplicate', same.a, f'{same.a}/../{same.a.name}')
        owner.focuses(2)
        owner.wait('dispatch ready generation=1', 2)
        assert owner.dispatched() == [str(same.a)] * 2
        assert same.mount(same.a) == identity and same.count(same.a) == 1
        same.verify_owner(owner)
        check('duplicate', owner=old_pid, runtime=owner.runtime, windows=1, active=same.a.name, rows=1, same_mount=True, focus=owner.window)

        same.launch('multi', same.b, same.c, same.b)
        owner.focuses(3)
        assert same.mount(same.c) and owner.dispatched() == [str(same.a)] * 2 + [str(same.b), str(same.c)]
        assert same.count(same.b) == same.count(same.c) == 1
        same.verify_owner(owner)
        check('multi', owner=old_pid, runtime=owner.runtime, windows=1, order=['B', 'C'], active='C', cwd='C', focus=owner.window)

        same.launch('no-path')
        owner.focuses(4)
        assert same.mount(same.c) and len(owner.dispatched()) == 4
        same.verify_owner(owner)
        check('no-path', owner=old_pid, runtime=owner.runtime, windows=1, active='C', commands=0, focus=owner.window)

        bad = same.root / 'file'
        bad.touch()
        same.launch('invalid-directory', bad, same.root / 'missing')
        owner.focuses(5)
        assert same.count(bad) == 0 and len(owner.dispatched()) == 4
        check('invalid-directory', windows=1, active='C', commands=0, focus=owner.window)

        # Lexically different addresses of the same data directory elect one owner.
        alias = same.root / 'alias'
        alias.symlink_to(same.data, target_is_directory=True)
        same.launch('root-alias', data=alias)
        owner.focuses(6)
        same.verify_owner(owner)
        check('root-alias', owner=old_pid, runtime=owner.runtime, windows=1, focus=owner.window)

        separate = Fixture(binary, temp / 'f', logs, channels)
        with separate.owner('other-root-owner', separate.b) as other:
            other.initial(1)
            assert separate.mount(separate.b) and same.mount(same.c)
            assert other.process.pid != old_pid and other.runtime != owner.runtime
            other.focuses(1)
            separate.verify_owner(other)
            same.verify_owner(owner)
            check('separate-roots', owners=[old_pid, other.process.pid], runtimes=[owner.runtime, other.runtime],
                  windows=[1, 1], active=['C', 'B'])

    with same.owner('takeover-owner', same.b) as successor:
        successor.initial(1)
        assert successor.process.pid != old_pid and not pid_exists(old_pid)
        assert same.mount(same.b) and same.count(same.b) == 1
        successor.focuses(1)
        same.verify_owner(successor)
        check('takeover', old_pid=old_pid, owner=successor.process.pid, runtime=successor.runtime,
              windows=1, active='B', cwd='B', focus=successor.window)

    boot = Fixture(binary, temp / 'b', logs, channels)
    with boot.owner('startup-owner', mode='startup') as owner:
        boot.launch('startup-secondary', boot.a, boot.b, boot.a)
        boot.launch('startup-no-path')
        assert not children(owner.process.pid) and not (boot.data / 'matou.sqlite').exists()
        assert not owner.dispatched() and len(owner.lines()) == 1
        owner.release()
        owner.initial(1)
        owner.focuses(1)
        assert boot.mount(boot.b) and owner.dispatched() == [str(boot.a), str(boot.b)]
        assert boot.count(boot.a) == boot.count(boot.b) == 1
        boot.verify_owner(owner)
        check('startup', owner=owner.process.pid, runtime_before=0, runtime_after=owner.runtime,
              windows_before=0, windows_after=1, order=[boot.a.name, 'B'], active='B', cwd='B', focus=owner.window)

    empty = Fixture(binary, temp / 'c', logs, channels)
    with empty.owner('no-window-owner', mode='no-window') as owner:
        owner.initial(0)
        empty.mount(empty.default)
        empty.launch('no-window-no-path')
        owner.wait('no focus windows=0')
        assert not owner.dispatched()
        empty.verify_owner(owner, windows=0)
        check('no-window-no-path', owner=owner.process.pid, runtime=owner.runtime, windows=0, commands=0, focus=False)
        empty.launch('no-window-directory', empty.b)
        owner.initial(0)
        owner.window = owner.wait('window opened windows=1 id=')[0].split(' id=')[1]
        owner.focuses(1)
        assert empty.mount(empty.b) and owner.dispatched() == [str(empty.b)]
        empty.verify_owner(owner)
        check('no-window-directory', owner=owner.process.pid, runtime=owner.runtime, windows=1, active='B', cwd='B', focus=owner.window)

    broken = Fixture(binary, temp / 'd', logs, channels)
    with broken.owner('channel-owner') as owner:
        owner.initial(1)
        broken.mount(broken.default)
        channel = broken.endpoint.parent
        assert stat.S_IMODE(channel.stat().st_mode) == 0o700
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(str(broken.endpoint))
            payload = json.dumps([list(os.fsencode(broken.data / 'other')), [str(broken.a)]]).encode()
            client.sendall(struct.pack('>I', len(payload)) + payload)
            assert not client.recv(1)
        owner.wait('desktop open request data root differs')
        broken.launch('recover-after-malformed', broken.a)
        owner.focuses(1)
        assert broken.mount(broken.a)
        broken.verify_owner(owner)
        check('malformed-channel', owner=owner.process.pid, runtime=owner.runtime, windows=1, foreign_request='rejected', next_request='mounted')

        # Simulate destruction of a channel while its original process still owns the root.
        broken.endpoint.unlink()
        began = time.monotonic()
        message = broken.launch('missing-channel', broken.b, success=False)
        elapsed = time.monotonic() - began
        assert 'forward to primary desktop instance' in message and elapsed < 4, message
        assert broken.count(broken.b) == 0 and broken.mount(broken.a)
        broken.verify_owner(owner)
        check('missing-channel', owner=owner.process.pid, runtime=owner.runtime,
              windows=1, second='controlled error', active=broken.a.name, seconds=round(elapsed, 2))

    with socket.socket(socket.AF_UNIX) as stale:
        stale.bind(str(broken.endpoint))
    assert broken.endpoint.exists()
    with broken.owner('stale-socket-owner', broken.b) as owner:
        owner.initial(1)
        assert broken.mount(broken.b)
        owner.focuses(1)
        broken.verify_owner(owner)
        check('stale-socket', owner=owner.process.pid, runtime=owner.runtime, windows=1, active='B', cwd='B')

    blocked = Fixture(binary, temp / 'e', logs, channels)
    with blocked.lock.open('wb+') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with socket.socket(socket.AF_UNIX) as silent:
            silent.bind(str(blocked.endpoint))
            silent.listen()
            began = time.monotonic()
            message = blocked.launch('silent-channel', blocked.a, success=False)
            assert 'wait for desktop open acknowledgement' in message and time.monotonic() - began < 3
            assert not (blocked.data / 'matou.sqlite').exists()
    check('silent-channel', second='acknowledgement timeout', runtime=0, windows=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('apps/native/target/debug/matou-native-desktop'))
    parser.add_argument('--logs', type=Path, required=True)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    temp = Path.cwd() / '.tmp'
    temp.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='w', dir=temp) as fixtures, contextlib.ExitStack() as channels:
        cases(args.binary.resolve(strict=True), Path(fixtures).resolve(), args.logs.resolve(), channels)


if __name__ == '__main__':
    main()
