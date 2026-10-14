"""Contract tests run against actual pinned upstream code and synthetic files only."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import bridge
from claude_swap.models import Platform
from claude_swap.switcher import ClaudeAccountSwitcher

class EngineIntegration(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="matou-account-contract-")
        self.home = Path(self.temp.name)
        self.profile = self.home / ".claude"
        self.profile.mkdir()
        self.env = patch.dict(os.environ, {"HOME": str(self.home), "XDG_DATA_HOME": str(self.home / "data")}, clear=True)
        self.env.start()
        self.platform = patch("claude_swap.models.Platform.detect", return_value=Platform.LINUX)
        self.platform.start()
        self.profile_check = patch("claude_swap.oauth.fetch_oauth_profile", return_value={"uuid": "fixture-a", "email": "work@example.com", "organizationUuid": "org-a"})
        self.profile_check.start()
        self.output = contextlib.redirect_stdout(io.StringIO()); self.output.__enter__()
        self.write_login("a")
    def tearDown(self):
        self.output.__exit__(None,None,None)
        self.profile_check.stop(); self.platform.stop(); self.env.stop(); self.temp.cleanup()
    def write_login(self, name):
        (self.home / ".claude.json").write_text(json.dumps({"oauthAccount": {"accountUuid": "fixture-"+name, "emailAddress": "work@example.com", "organizationUuid": "org-"+name}, "keepSetting": True}))
        (self.profile / ".credentials.json").write_text(json.dumps({"claudeAiOauth": {"accessToken": "FIXTURE-ACCESS-"+name, "refreshToken": "FIXTURE-REFRESH-"+name, "expiresAt": 9999999999999}, "mcpOAuth": {"fixture": True}}))
    def run_action(self, **request):
        return bridge.dispatch(request, self.home / "adapter")
    def test_detect_save_list_remove_delegate_real_engine(self):
        state = self.run_action(action="snapshot")
        self.assertEqual(state["accounts"], [])
        self.assertEqual(state["detected"]["email"], "wo•••@example.com")
        ident = state["detected"]["id"]
        with patch.object(ClaudeAccountSwitcher, "add_account", autospec=True, side_effect=ClaudeAccountSwitcher.add_account) as called:
            saved = self.run_action(action="save-current", id=ident)
            self.assertEqual(called.call_count, 1)
        self.assertEqual(saved["currentId"], ident)
        self.assertEqual(len(saved["accounts"]), 1)
        serialized = json.dumps(saved)
        self.assertNotIn("FIXTURE-", serialized)
        self.assertNotIn("work@example.com", serialized)
        before = (self.profile / ".credentials.json").read_bytes()
        removed = self.run_action(action="remove", id=ident)
        self.assertEqual(removed["accounts"], [])
        self.assertEqual((self.profile / ".credentials.json").read_bytes(), before)
    def test_repeat_add_updates_one_slot(self):
        first = self.run_action(action="snapshot")["detected"]["id"]
        self.run_action(action="save-current", id=first)
        self.run_action(action="save-current", id=first)
        self.assertEqual(len(self.run_action(action="snapshot")["accounts"]), 1)
    def test_isolated_capture_keeps_device_login(self):
        nonce = "00000000-0000-4000-8000-000000000001"
        isolated = self.home / "adapter/login" / nonce
        isolated.mkdir(parents=True)
        (isolated / ".claude.json").write_bytes((self.home / ".claude.json").read_bytes())
        (isolated / ".credentials.json").write_bytes((self.profile / ".credentials.json").read_bytes())
        before = (self.profile / ".credentials.json").read_bytes()
        draft = self.run_action(action="draft", nonce=nonce)["draft"]
        self.run_action(action="save-draft", nonce=nonce, id=draft["id"])
        self.assertEqual((self.profile / ".credentials.json").read_bytes(), before)
        self.assertFalse(isolated.exists())
    def test_real_upstream_switch_preserves_unrelated_settings_and_credentials(self):
        ident_a = self.run_action(action="snapshot")["detected"]["id"]
        self.run_action(action="save-current", id=ident_a)
        self.write_login("b")
        with patch("claude_swap.oauth.fetch_oauth_profile", side_effect=lambda token: {
            "uuid": "fixture-" + token[-1], "email": "work@example.com", "organizationUuid": "org-" + token[-1]
        }):
            ident_b = self.run_action(action="snapshot")["detected"]["id"]
            self.run_action(action="save-current", id=ident_b)
            with patch.object(ClaudeAccountSwitcher, "switch_to", autospec=True, side_effect=ClaudeAccountSwitcher.switch_to) as called:
                changed = self.run_action(action="switch", id=ident_a)
                self.assertEqual(called.call_count, 1)
            self.assertEqual(changed["currentId"], ident_a)
            credentials = json.loads((self.profile / ".credentials.json").read_text())
            self.assertEqual(credentials["claudeAiOauth"]["accessToken"], "FIXTURE-ACCESS-a")
            self.assertEqual(credentials["mcpOAuth"], {"fixture": True})
            self.assertTrue(json.loads((self.home / ".claude.json").read_text())["keepSetting"])
    def test_renewed_default_uses_upstream_recovery_without_overwriting_new_tokens(self):
        ident = self.run_action(action="snapshot")["detected"]["id"]
        self.run_action(action="save-current", id=ident)
        nonce = "00000000-0000-4000-8000-000000000002"
        isolated = self.home / "adapter/login" / nonce
        isolated.mkdir(parents=True)
        (isolated / ".claude.json").write_bytes((self.home / ".claude.json").read_bytes())
        credentials = json.loads((self.profile / ".credentials.json").read_text())
        credentials["claudeAiOauth"]["accessToken"] = "FIXTURE-RENEWED-a"
        credentials["claudeAiOauth"]["refreshToken"] = "FIXTURE-RENEWED-REFRESH-a"
        (isolated / ".credentials.json").write_text(json.dumps(credentials))
        before = (self.profile / ".credentials.json").read_bytes()
        self.run_action(action="save-draft", nonce=nonce, id=ident)
        self.assertEqual((self.profile / ".credentials.json").read_bytes(), before)
        with patch.object(ClaudeAccountSwitcher, "switch_to", autospec=True, side_effect=ClaudeAccountSwitcher.switch_to) as called:
            result = self.run_action(action="reauth", id=ident)
            self.assertTrue(called.call_args.kwargs["force"])
        self.assertEqual(result["currentId"], ident)
        live = json.loads((self.profile / ".credentials.json").read_text())
        self.assertEqual(live["claudeAiOauth"]["accessToken"], "FIXTURE-RENEWED-a")
        self.assertEqual(live["mcpOAuth"], {"fixture": True})
        self.assertEqual(len(result["accounts"]), 1)

    def test_repair_rejects_changed_default_without_writing(self):
        ident = self.run_action(action="snapshot")["detected"]["id"]
        self.run_action(action="save-current", id=ident)
        self.write_login("b")
        before = (self.profile / ".credentials.json").read_bytes()
        with self.assertRaisesRegex(ValueError, "conflict"):
            self.run_action(action="reauth", id=ident)
        self.assertEqual((self.profile / ".credentials.json").read_bytes(), before)

    def test_nonce_traversal_rejected(self):
        with self.assertRaises(ValueError):
            self.run_action(action="cancel-login", nonce="../../outside")

if __name__ == "__main__":
    unittest.main()

class UsagePresentation(unittest.TestCase):
    def test_only_explicit_login_failure_shows_reauthentication(self):
        for status in ('ok', 'unavailable', 'token_expired', 'keychain_unavailable', 'foreign_credential', 'api_key', 'relogin_required'):
            row = bridge.view({'email':'fixture@example.com','usageStatus':status})
            self.assertFalse(row['needsLogin'], status)
        for status in ('no_credentials',):
            self.assertTrue(bridge.view({'email':'fixture@example.com','usageStatus':status})['needsLogin'])
    def test_cached_timestamp_is_not_changed_and_rate_limit_keeps_last_good_usage(self):
        row = bridge.view({'email':'fixture@example.com','usageStatus':'unavailable','usageError':'http-429','usageRetryAt':'2026-10-14T04:06:00Z', 'lastGoodFetchedAt':'2026-10-14T04:00:00Z','lastGoodUsage':{'fiveHour':{'pct':38,'resetsAt':'2026-10-14T05:00:00Z'}}})
        self.assertNotIn('usageIssue', row)
        self.assertEqual(row['usage']['fiveHour']['used'],38)
        self.assertTrue(row['usage']['stale'])
        self.assertEqual(row['usage']['checkedAt'],1791950400000)
        self.assertNotIn('usageRetryAt', row)
        self.assertFalse(row['needsLogin'])
    def test_legacy_cache_is_always_presented_as_last_known(self):
        row = bridge.view({'email':'fixture@example.com','usageStatus':'ok','usageFetchedAt':'2026-10-14T04:00:00Z','usageAgeSeconds':120,'usage':{'fiveHour':{'pct':38}}})
        self.assertTrue(row['usage']['stale'])

class OfflineEngineIntegration(EngineIntegration):
    def test_every_exposed_action_skips_all_upstream_network_functions(self):
        import socket
        from contextlib import ExitStack
        with ExitStack() as stack:
            calls = [stack.enter_context(patch("claude_swap.oauth." + name, side_effect=AssertionError("unexpected HTTP")))
                     for name in ("fetch_oauth_profile", "try_refresh_oauth_credentials", "try_fetch_usage_for_account")]
            a = self.run_action(action="snapshot")["detected"]["id"]
            self.run_action(action="save-current", id=a)
            self.write_login("b")
            b = self.run_action(action="snapshot")["detected"]["id"]
            self.run_action(action="save-current", id=b)
            self.run_action(action="refresh")
            self.run_action(action="switch", id=a)
            # Official CLI full refresh-token rotation is captured on departure.
            live = json.loads((self.profile / ".credentials.json").read_text())
            live["claudeAiOauth"].update(accessToken="ROTATED-A", refreshToken="ROTATED-REFRESH-A")
            (self.profile / ".credentials.json").write_text(json.dumps(live))
            self.run_action(action="switch", id=b)
            self.run_action(action="switch", id=a)
            self.assertEqual(json.loads((self.profile / ".credentials.json").read_text())["claudeAiOauth"]["refreshToken"], "ROTATED-REFRESH-A")
            self.run_action(action="remove", id=b)
            for call in calls: call.assert_not_called()
        with socket.socket() as client:
            with self.assertRaisesRegex(RuntimeError, "network-disabled"):
                client.connect(("127.0.0.1", 9))
