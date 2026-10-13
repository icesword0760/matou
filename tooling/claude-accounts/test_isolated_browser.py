"""No real accounts: process, URL-boundary and browser isolation tests."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import isolated_browser as browser

URL = 'https://claude.ai/oauth/authorize?state=FIXTURE&code_challenge=FIXTURE'

class PrivateBrowserTests(unittest.TestCase):
    def test_only_complete_official_authorization_urls_are_accepted(self):
        self.assertEqual(browser.authorization_url(('Login: '+URL+'\n').encode()), URL)
        current = URL.replace('claude.ai/oauth', 'claude.com/cai/oauth')
        self.assertEqual(browser.authorization_url((current+'\n').encode()), current)
        for invalid in [URL, URL.replace('claude.ai', 'claude.ai.example.org')+'\n',
                        URL.replace('https:', 'http:')+'\n', URL.replace('/oauth/authorize', '/redirect')+'\n',
                        URL.replace('claude.ai', 'attacker@claude.ai')+'\n', URL.replace('state=', 'missing=')+'\n']:
            self.assertIsNone(browser.authorization_url(invalid.encode()))

    def test_launch_uses_unique_profile_and_keeps_url_out_of_process_arguments(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(browser.subprocess, 'Popen') as start:
            profile = Path(temp)
            browser.start_browser('/fixture/chrome', profile, URL)
            args = start.call_args.args[0]
            self.assertIn('--incognito', args)
            self.assertIn('--user-data-dir='+str(profile/'browser'), args)
            self.assertNotIn(URL, args)
            self.assertEqual((profile/'browser-start.html').stat().st_mode & 0o777, 0o600)
            self.assertEqual((profile/'browser').stat().st_mode & 0o777, 0o700)

    def fake_cli(self, root, body):
        cli = root/'claude'
        cli.write_text('#!'+sys.executable+'\n'+body)
        cli.chmod(0o700)
        return str(cli)

    def test_real_child_suppresses_normal_browser_and_closes_private_process(self):
        with tempfile.TemporaryDirectory() as temp:
            profile = Path(temp)
            cli = self.fake_cli(profile, 'import os,time\nassert os.environ["BROWSER"]=="true"\nprint('+repr(URL)+',flush=True)\ntime.sleep(.3)\n')
            children=[]
            def launch(exe, p, url):
                self.assertEqual(url, URL)
                child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True)
                children.append(child)
                return child
            before=os.environ.get('BROWSER')
            with patch.object(browser,'find_browser',return_value='/fixture/chrome'), patch.object(browser,'start_browser',side_effect=launch):
                browser.login(cli,profile,timeout=3)
            self.assertEqual(os.environ.get('BROWSER'),before)
            self.assertIsNotNone(children[0].poll())

    def test_timeout_stops_cli_and_removes_temporary_browser_files(self):
        with tempfile.TemporaryDirectory() as temp:
            profile=Path(temp)
            cli=self.fake_cli(profile,'import time\ntime.sleep(30)\n')
            with patch.object(browser,'find_browser',return_value='/fixture/chrome'):
                with self.assertRaises(subprocess.TimeoutExpired): browser.login(cli,profile,timeout=.2)
            self.assertFalse((profile/'browser').exists())

    def test_closed_browser_returns_actionable_error(self):
        with tempfile.TemporaryDirectory() as temp:
            profile=Path(temp)
            cli=self.fake_cli(profile,'import time\nprint('+repr(URL)+',flush=True)\ntime.sleep(30)\n')
            def launch(*_):
                return subprocess.Popen([sys.executable,'-c','pass'],start_new_session=True)
            with patch.object(browser,'find_browser',return_value='/fixture/chrome'), patch.object(browser,'start_browser',side_effect=launch):
                with self.assertRaisesRegex(ValueError,'browser-closed'): browser.login(cli,profile,timeout=3)

    def test_missing_browser_never_starts_cli_or_default_browser(self):
        with patch.object(browser,'find_browser',side_effect=ValueError('browser-missing')), patch.object(browser.subprocess,'Popen') as start:
            with self.assertRaisesRegex(ValueError,'browser-missing'): browser.login('/fixture/claude',Path('/fixture/profile'))
            start.assert_not_called()

if __name__ == '__main__': unittest.main()
