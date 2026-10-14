"""Local-only policy over the pinned upstream credential transaction engine.

Keep upstream storage, locks, MCP merging and rollback. No HTTP identity oracle,
usage fetch or refresh grant is used here; the official CLI owns those requests.
"""
import sys
from claude_swap import oauth
from claude_swap.exceptions import ConfigError
from claude_swap.switcher import ClaudeAccountSwitcher
from claude_swap.usage_store import with_sentinel


def _deny_network(event, _args):
    if event in ("socket.connect", "socket.getaddrinfo"):
        raise RuntimeError("account-engine-network-disabled")


# Process-wide defense against a future upstream code path. This audit hook is
# not inherited by exec'd official CLI / browser children. No credential output.
sys.addaudithook(_deny_network)


class LocalAccountSwitcher(ClaudeAccountSwitcher):
    def _prefetch_live_identity(self):
        return {"live": None, "resolved": None}

    def _known_owner(self, creds):
        fingerprint = oauth.credential_fingerprint(creds)
        if not fingerprint:
            return None
        for slot, account in (self._get_sequence_data() or {}).get("accounts", {}).items():
            stored = self._read_account_credentials(slot, account["email"])
            if stored and oauth.credential_fingerprint(stored) == fingerprint:
                return slot
        return None

    def _reject_foreign_credential_capture(self, creds, email, org_uuid="", account_uuid=""):
        # Official login produces identity + credential in the same isolated
        # profile. Upstream rechecks both for drift before writing the backup.
        # Preserve local detection of accidentally paired known-account tokens.
        slot = self._known_owner(creds)
        if slot is not None:
            identity = self.account_identity(slot)
            if identity["email"] != email or identity["organizationUuid"] != org_uuid:
                raise ConfigError("Local credential identity mismatch")
        return creds

    def _classify_outgoing_credential(self, current_account, current_email, original_creds, provenance, data):
        owner = self._known_owner(original_creds)
        if owner is not None and owner != current_account:
            return ("foreign-synced", owner)
        # A full official rotation changes both tokens. Preserve the newest live
        # pair under upstream's CLI-compatible locks; never revert to old grants.
        return super()._classify_outgoing_credential(current_account, current_email, original_creds, provenance, data)

    def _collect_usage_entries(self, accounts_info, fetch=None, *, scheduled=False):
        # Read legacy measurements for migration only. Do not reserve fetches,
        # adopt remote rejection flags, probe identity, or refresh any account.
        identities = {str(n): (email, org or "") for n, email, _, org, *_ in accounts_info}
        entries = self._usage_store.entries(identities)
        for n, _, _, _, active, creds, _ in accounts_info:
            data = oauth.extract_oauth_data(creds)
            missing = not creds or data is not None and not (data.get("accessToken") or data.get("refreshToken"))
            if active and self._active_keychain_unavailable:
                entries[str(n)] = with_sentinel(entries[str(n)], "keychain_unavailable")
            elif missing:
                entries[str(n)] = with_sentinel(entries[str(n)], "no_credentials")
        return entries
