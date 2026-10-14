# Matou ↔ claude-swap adapter

Uses the pinned upstream **local credential transaction engine**, with a narrow
`LocalAccountSwitcher` policy adapter. MIT attribution is in `UPSTREAM-LICENSE`.
Upstream still owns Keychain/file storage, compatible locks, shared MCP fields,
backup and rollback. The official CLI alone owns login, refresh and remote requests.

The adapter skips profile probes, usage reservation/fetch and token renewal.
A Python audit hook rejects outbound sockets in the sidecar as a second boundary;
exec'd official CLI and browser children are not subject to that Python hook.
No User-Agent impersonation or official CLI binary modification is used.

- `pnpm accounts:setup`: prepare the project-local development environment (uv + Python 3.12+).
- `pnpm test:accounts`: main-process orchestration tests and real-library fixture tests.
- `pnpm accounts:build`: build a self-contained native sidecar for the current OS/architecture.
- One request on stdin, one redacted JSON response on stdout. Login keeps stdin open for a single attempt-bound authorization-code message. No secret arguments.
- On startup only version detection is harmless; `snapshot` can access the system Keychain.
  Tests must use the fixture bridge or the synthetic HOME + platform guards in `test_bridge.py`.
- No automatic upstream upgrades. Update the commit, expected version, lock, license,
  contract tests, packaged smoke check and manual OAuth acceptance together.

The bridge adds isolated **official CLI login** orchestration because upstream's public add
command captures an already signed-in profile. Both profile variables point at the same
private temporary directory; upstream captures, validates, stores, and deduplicates the login.
Chinese display names are presentation-only preferences; the shared upstream roster stays
compatible with cswap. The local policy overrides pinned private extension points, covered by contract
tests. A full official rotation is backed up on the next switch, under the
upstream CLI-compatible refresh/config locks. Known foreign token families are
never assigned to another account; offline identity relies on official local
metadata rather than a remote identity oracle.

Production packages carry the interpreter; users do not install Python or uv. Developers and
release builders install uv. Build natively for each release architecture; do not reuse an
arm64 sidecar in an x64 or universal app.

## Isolated browser sign-in

The official CLI performs authorization with a per-attempt config directory. Its normal browser opener is suppressed for that child only. `isolated_browser.py` accepts only official authorization endpoints and opens an installed Chrome, Edge or Brave executable with a unique temporary user-data directory and incognito mode. It does not implement OAuth. No ordinary-browser fallback is used.

Run `pnpm test:accounts` for upstream and isolated-browser contracts. Run `uv run --frozen --project tooling/claude-accounts python tooling/claude-accounts/browser_smoke.py` for two real concurrent Chromium windows against synthetic localhost cookies; no accounts are used.

## Manual authorization code (0.2.6)

Both the automatic localhost callback and the official CLI's code#state stdin flow are supported. The renderer offers a masked input while login is pending. It forwards a bounded code only to the matching active attempt; repeated submissions are ignored and cancellation revokes the pipe immediately. The official CLI, not Matou, validates OAuth state and exchanges the code. Codes never enter disk storage, command arguments, logs, preferences, or published account state.

Login success, failure, timeout, cancellation and app shutdown leave the browser window open for the user to close. The private Chromium profile is in its own OS temporary directory, outside the credential draft directory, so saving or cancelling a draft never removes a live browser profile. An already-closed window's temporary profile is reclaimed at flow completion; a window left open retains its temporary directory for system/user cleanup. The launch redirect file is removed at flow completion.

## Reauthentication (0.2.7)

Reauthentication targets an existing identity. Saving renews that slot only. Applying a renewed default is an explicit separate command: the adapter verifies that the target is still the current default, then calls upstream `switch_to(..., force=True)` for its documented stale-live recovery path. This avoids overwriting the newly saved slot with stale live credentials. Ordinary account changes still use normal `switch_to`. Main-process state requires a saved renewal before allowing recovery, holds the runtime provider-mutation gate without waiting for working tasks, and never invokes session resume automatically. Pending renewals (IDs only) persist in presentation preferences. Windows remain user-owned.

## Official quota cache

The runtime reads the official statusLine rate_limits already emitted after model
responses. No quota endpoint is called by account management. Non-secret quota
observations are stored by account ID in desktop preferences; legacy cached values
remain readable. The original reset timestamp is authoritative: expired windows
retain their last measurement and are labelled pending, never reset to a fabricated
zero or moved by five hours/seven days. App restart preserves observations.

To avoid cross-account attribution, every observation is bound to a local official
identity and process/turn. Old processes must begin a new prompt after a 35-second
Keychain-cache margin and report a new API-duration increment before their quota
is recorded. Timer rerenders and an old task finishing do not establish ownership.
This delays quota recording, **not switching or task execution**. A newly started
process can report its first response immediately. A changed external CLI login
also invalidates attribution. A cache is not server-side account-health proof.

## Native isolated validation

`MATOU_TEST_CLAUDE=/absolute/path/to/official/claude uv run --frozen --project tooling/claude-accounts python tooling/claude-accounts/validation/real_keychain.py`

Uses two unmodified CLI processes, synthetic credentials, a localhost-only TLS
server, and a disposable real macOS Keychain. A test-only security command router
adds that keychain's explicit path. It does not read real account credentials or
replace the user's default Keychain. The report checks switch-during-refresh
serialization, one refresh grant across processes, preserved successor tokens,
same-process history, shared MCP state, cleanup and Keychain settings readback.
