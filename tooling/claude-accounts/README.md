# Matou ↔ claude-swap adapter

Uses **unmodified upstream code**, pinned by Git revision in `pyproject.toml` and `uv.lock`.
See `UPSTREAM-LICENSE` for MIT attribution. Never implement credential switching, refresh,
Keychain access or rollback here: call upstream methods instead.

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
compatible with cswap. The sole private adapter call `_get_current_account` is read-only and
covered by pinned-version contract tests.

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

Reauthentication targets an existing identity. Saving renews that slot only. Applying a renewed default is an explicit separate command: the adapter verifies that the target is still the current default, then calls upstream `switch_to(..., force=True)` for its documented stale-live recovery path. This avoids overwriting the newly saved slot with stale live credentials. Ordinary account changes still use normal `switch_to`. Main-process state requires a saved renewal before allowing recovery, holds the existing runtime gate, rejects busy tasks without scheduling, and never invokes session resume automatically. Pending renewals (IDs only) persist in presentation preferences. Windows remain user-owned.
