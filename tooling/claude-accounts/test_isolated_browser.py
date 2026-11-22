"""No real accounts: process, URL-boundary and browser isolation tests."""
import os
import io
import json
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import isolated_browser as browser

URL = 'https://claude.ai/oauth/authorize?state=FIXTURE&code_challenge=FIXTURE'
COMPACT_URL = 'https://claude.ai/oauth/authorize?z-HEHOr2P7pBfhdSDfUPBteRqDDPQWyJ9aC7jG-BhXJMI'

class PrivateBrowserTests(unittest.TestCase):
    def test_only_complete_official_authorization_urls_are_accepted(self):
        for url in (URL, COMPACT_URL):
            for authority in ('claude.ai/oauth', 'claude.com/cai/oauth',
                              'console.anthropic.com/oauth', 'platform.claude.com:443/oauth'):
                official = url.replace('claude.ai/oauth', authority)
                for ending in ('\n', '\r\n', '\x1b[0m'):
                    self.assertEqual(browser.authorization_url(('Login: ' + official + ending).encode()), official)
            self.assertIsNone(browser.authorization_url(url.encode()))
        for invalid in (
            COMPACT_URL.replace('claude.ai', 'claude.ai.example.org'),
            COMPACT_URL.replace('claude.ai', 'claude.ai@evil.example'),
            COMPACT_URL.replace('claude.ai', '@claude.ai'),
            COMPACT_URL.replace('claude.ai', 'attacker:password@claude.ai'),
            COMPACT_URL.replace('claude.ai', 'claude.ai:444'),
            COMPACT_URL.replace('https:', 'http:'),
            COMPACT_URL.replace('/oauth/authorize', '/redirect'),
            COMPACT_URL + '#fragment', COMPACT_URL + '#', COMPACT_URL + '=value', COMPACT_URL + '&x',
            URL.replace('state=', 'missing='), URL.replace('state=FIXTURE', 'state='),
            *(COMPACT_URL.split('?')[0] + '?' + query for query in ('', 'a' * 15, 'a' * 513, '%41' * 16)),
        ):
            self.assertIsNone(browser.authorization_url((invalid + '\n').encode()))

    def test_split_compact_token_waits_for_line_delimiter(self):
        with tempfile.TemporaryDirectory() as temp:
            profile = Path(temp)
            prefix, suffix = COMPACT_URL[:-12], COMPACT_URL[-12:]
            cli = self.fake_cli(profile, f'''
import sys, time
sys.stdout.write({prefix!r})
sys.stdout.flush()
time.sleep(0.15)
sys.stdout.write({(suffix + chr(13) + chr(10))!r})
sys.stdout.flush()
time.sleep(0.2)
''')
            child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(5)'], start_new_session=True)
            try:
                with patch.object(browser, 'find_browser', return_value=('/fixture/chrome', [])), \
                        patch.object(browser, 'start_browser', return_value=child) as launch:
                    browser.login(cli, profile, timeout=3)
                launch.assert_called_once_with('/fixture/chrome', profile, COMPACT_URL, [])
            finally:
                browser.stop_process(child)

    def test_launch_uses_unique_profile_and_keeps_url_out_of_process_arguments(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(browser.subprocess, 'Popen') as start:
            profile = Path(temp)
            browser.start_browser('/fixture/chrome', profile, URL)
            args = start.call_args.args[0]
            self.assertIn('--incognito', args)
            directory = Path(next(arg.split('=', 1)[1] for arg in args if arg.startswith('--user-data-dir=')))
            self.assertNotEqual(directory.parent, profile)
            self.assertNotIn(URL, args)
            self.assertEqual((profile/'browser-start.html').stat().st_mode & 0o777, 0o600)
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
            shutil.rmtree(directory)

    def test_safe_kernel_takes_priority_and_returns_exact_parameters(self):
        env = {'MATOU_SAFE_BROWSER_BIN': '/fixture/safe-kernel',
               'MATOU_SAFE_BROWSER_TZ': 'Asia/Tokyo', 'MATOU_SAFE_BROWSER_SEED': '123456'}
        with patch.dict(os.environ, env, clear=True), patch.object(browser.Path, 'is_file', return_value=True), patch.object(browser.os, 'access', return_value=True), patch.object(browser.shutil, 'which') as which:
            self.assertEqual(browser.find_browser(), ('/fixture/safe-kernel', [
                '--timezone=Asia/Tokyo', '--accept-lang=zh-CN,zh',
                '--fingerprint=123456', '--fingerprint-platform=macos']))
            which.assert_not_called()

    def test_missing_or_invalid_safe_env_falls_back_without_extra_parameters(self):
        valid = {'MATOU_SAFE_BROWSER_BIN': '/fixture/safe-kernel',
                 'MATOU_SAFE_BROWSER_TZ': 'Asia/Tokyo', 'MATOU_SAFE_BROWSER_SEED': '123456'}
        cases = [{}, *[{k: v for k, v in valid.items() if k != missing} for missing in valid]]
        cases += [dict(valid, MATOU_SAFE_BROWSER_TZ=tz) for tz in ('', 'Invalid/Zone', '/Asia/Tokyo', '../Tokyo')]
        cases += [dict(valid, MATOU_SAFE_BROWSER_SEED=seed) for seed in ('', '0', '000', '-1', '1.5', 'NaN', '1e3', ' 1', '+1', '1\n', '１２')]
        cases += [dict(valid, MATOU_SAFE_BROWSER_BIN='/fixture/missing')]
        for env in cases:
            with self.subTest(env=env), patch.dict(os.environ, env, clear=True), patch.object(browser.Path, 'is_file', lambda p: str(p) in ('/fixture/chrome', '/fixture/safe-kernel')), patch.object(browser.os, 'access', return_value=True), patch.object(browser.shutil, 'which', side_effect=lambda name: '/fixture/chrome' if name == 'google-chrome' else None):
                self.assertEqual(browser.find_browser(), ('/fixture/chrome', []))

    def test_non_executable_kernel_uses_existing_browser_selection(self):
        env = {'MATOU_SAFE_BROWSER_BIN': '/fixture/safe-kernel',
               'MATOU_SAFE_BROWSER_TZ': 'Asia/Tokyo', 'MATOU_SAFE_BROWSER_SEED': '123456'}
        with patch.dict(os.environ, env, clear=True), patch.object(browser.Path, 'is_file', lambda p: str(p) in ('/fixture/chrome', '/fixture/safe-kernel')), patch.object(browser.os, 'access', side_effect=lambda p, mode: p == '/fixture/chrome'), patch.object(browser.shutil, 'which', side_effect=lambda name: '/fixture/chrome' if name == 'google-chrome' else None):
            self.assertEqual(browser.find_browser(), ('/fixture/chrome', []))

    def test_safe_launch_has_exact_arguments_unique_profiles_and_no_redirect_file(self):
        extra = ['--timezone=Asia/Tokyo', '--accept-lang=zh-CN,zh', '--fingerprint=123456', '--fingerprint-platform=macos']
        directories = []
        with tempfile.TemporaryDirectory() as temp, patch.object(browser.subprocess, 'Popen') as start:
            profile = Path(temp)
            try:
                for _ in range(2):
                    browser.start_browser('/fixture/safe-kernel', profile, URL, extra)
                    args = start.call_args.args[0]
                    directory = Path(args[1].split('=', 1)[1])
                    directories.append(directory)
                    self.assertEqual(args, ['/fixture/safe-kernel', '--user-data-dir=' + str(directory),
                        '--incognito', '--new-window', '--no-first-run', '--no-default-browser-check',
                        '--disable-sync', '--disable-background-mode', *extra, URL])
                    self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
                    self.assertFalse((profile / 'browser-start.html').exists())
                    self.assertEqual(start.call_args.kwargs['stdout'], subprocess.DEVNULL)
                    self.assertEqual(start.call_args.kwargs['stderr'], subprocess.DEVNULL)
                    self.assertTrue(start.call_args.kwargs['start_new_session'])
                self.assertNotEqual(*directories)
            finally:
                for directory in directories: shutil.rmtree(directory)

    def test_login_forwards_safe_parameters_and_removes_closed_profile(self):
        extra = ['--timezone=Asia/Tokyo', '--accept-lang=zh-CN,zh', '--fingerprint=123456', '--fingerprint-platform=macos']
        with tempfile.TemporaryDirectory() as temp:
            profile = Path(temp)
            directory = profile / 'browser-profile'
            directory.mkdir()
            cli = self.fake_cli(profile, 'import time\nprint(' + repr(URL) + ',flush=True)\ntime.sleep(30)\n')
            child = subprocess.Popen([sys.executable, '-c', 'pass'], start_new_session=True)
            child.wait(timeout=3)
            child.matou_browser_directory = directory
            with patch.object(browser, 'find_browser', return_value=('/fixture/safe-kernel', extra)), patch.object(browser, 'start_browser', return_value=child) as start:
                with self.assertRaisesRegex(ValueError, 'browser-closed'): browser.login(cli, profile, timeout=3)
                start.assert_called_once_with('/fixture/safe-kernel', profile, URL, extra)
            self.assertFalse(directory.exists())

    def fake_cli(self, root, body):
        cli = root/'claude'
        cli.write_text('#!'+sys.executable+'\n'+body)
        cli.chmod(0o700)
        return str(cli)

    def test_automatic_completion_preserves_private_browser(self):
        with tempfile.TemporaryDirectory() as temp:
            profile = Path(temp)
            cli = self.fake_cli(profile, 'import os,time\nassert os.environ["BROWSER"]=="true"\nprint('+repr(URL)+',flush=True)\ntime.sleep(.3)\n')
            children=[]
            def launch(exe, p, url, extra_args):
                self.assertEqual(url, URL)
                child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True)
                children.append(child)
                return child
            before=os.environ.get('BROWSER')
            with patch.object(browser,'find_browser',return_value=('/fixture/chrome', [])), patch.object(browser,'start_browser',side_effect=launch):
                browser.login(cli,profile,timeout=3)
            self.assertEqual(os.environ.get('BROWSER'),before)
            try:
                self.assertIsNone(children[0].poll())
            finally:
                browser.stop_process(children[0])

    def test_timeout_stops_cli_and_removes_temporary_browser_files(self):
        with tempfile.TemporaryDirectory() as temp:
            profile=Path(temp)
            cli=self.fake_cli(profile,'import time\ntime.sleep(30)\n')
            with patch.object(browser,'find_browser',return_value=('/fixture/chrome', [])):
                with self.assertRaises(subprocess.TimeoutExpired): browser.login(cli,profile,timeout=.2)
            self.assertFalse((profile/'browser').exists())

    def test_closed_browser_returns_actionable_error(self):
        with tempfile.TemporaryDirectory() as temp:
            profile=Path(temp)
            cli=self.fake_cli(profile,'import time\nprint('+repr(URL)+',flush=True)\ntime.sleep(30)\n')
            def launch(*_):
                return subprocess.Popen([sys.executable,'-c','pass'],start_new_session=True)
            with patch.object(browser,'find_browser',return_value=('/fixture/chrome', [])), patch.object(browser,'start_browser',side_effect=launch):
                with self.assertRaisesRegex(ValueError,'browser-closed'): browser.login(cli,profile,timeout=3)

    def test_missing_browser_never_starts_cli_or_default_browser(self):
        with patch.object(browser,'find_browser',side_effect=ValueError('browser-missing')), patch.object(browser.subprocess,'Popen') as start:
            with self.assertRaisesRegex(ValueError,'browser-missing'): browser.login('/fixture/claude',Path('/fixture/profile'))
            start.assert_not_called()


    def test_pasted_code_reaches_only_matching_login_and_window_remains_open(self):
        with tempfile.TemporaryDirectory() as temp:
            profile = Path(temp)
            code = 'fixture_code_123456#fixture_state_123456'
            cli = self.fake_cli(profile, 'import sys\nprint('+repr(URL)+',flush=True)\nassert sys.stdin.readline().strip()=='+repr(code)+'\n')
            messages = [
                {'action': 'submit-login-code', 'nonce': 'old', 'code': code},
                {'action': 'submit-login-code', 'nonce': 'current', 'code': 'https://wrong.example'},
                {'action': 'submit-login-code', 'nonce': 'current', 'code': code},
            ]
            child = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(30)'], start_new_session=True)
            try:
                with patch.object(browser, 'find_browser', return_value=('/fixture/chrome', [])), patch.object(browser, 'start_browser', return_value=child):
                    browser.login(cli, profile, timeout=3, code_input=io.StringIO(''.join(json.dumps(m)+'\n' for m in messages)), nonce='current')
                self.assertIsNone(child.poll())
                self.assertFalse(any(code.encode() in f.read_bytes() for f in profile.iterdir() if f.is_file() and f.name != 'claude'))
            finally:
                browser.stop_process(child)

    def test_timeout_and_cancellation_preserve_browser(self):
        for cancelled in (False, True):
            with self.subTest(cancelled=cancelled), tempfile.TemporaryDirectory() as temp:
                profile = Path(temp)
                cli = self.fake_cli(profile, 'import time\nprint('+repr(URL)+',flush=True)\ntime.sleep(30)\n')
                child = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(30)'], start_new_session=True)
                read_fd, write_fd = os.pipe()
                try:
                    with os.fdopen(read_fd) as reader, os.fdopen(write_fd, 'w') as writer:
                        def launch(*_):
                            if cancelled:
                                print(json.dumps({'action': 'cancel-login', 'nonce': 'current'}), file=writer, flush=True)
                            return child

                        with patch.object(browser, 'find_browser', return_value=('/fixture/chrome', [])), \
                                patch.object(browser, 'start_browser', side_effect=launch) as start:
                            with self.assertRaises(KeyboardInterrupt if cancelled else subprocess.TimeoutExpired):
                                browser.login(cli, profile, timeout=3 if cancelled else 1,
                                              code_input=reader if cancelled else None, nonce='current')
                        start.assert_called_once()
                        self.assertIsNone(child.poll())
                finally:
                    browser.stop_process(child)

    def test_wrong_or_missing_code_never_completes_login(self):
        with tempfile.TemporaryDirectory() as temp:
            profile = Path(temp)
            cli = self.fake_cli(profile, 'import sys\nprint('+repr(URL)+',flush=True)\nsys.stdin.readline()\n')
            child = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(30)'], start_new_session=True)
            try:
                with patch.object(browser, 'find_browser', return_value=('/fixture/chrome', [])), patch.object(browser, 'start_browser', return_value=child):
                    with self.assertRaises(subprocess.TimeoutExpired):
                        browser.login(cli, profile, timeout=.3, code_input=io.StringIO('{"nonce":"old","code":"fixture_code_123#fixture_state_123"}\n'), nonce='current')
                self.assertIsNone(child.poll())
            finally:
                browser.stop_process(child)

if __name__ == '__main__': unittest.main()
