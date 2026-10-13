# Matou ↔ claude-swap adapter

Uses **unmodified upstream code**, pinned by Git revision in `pyproject.toml` and `uv.lock`.
See `UPSTREAM-LICENSE` for MIT attribution. Never implement credential switching, refresh,
Keychain access or rollback here: call upstream methods instead.

- `pnpm accounts:setup`: prepare the project-local development environment (uv + Python 3.12+).
- `pnpm test:accounts`: main-process orchestration tests and real-library fixture tests.
- `pnpm accounts:build`: build a self-contained native sidecar for the current OS/architecture.
- One request on stdin, one redacted JSON response on stdout. No secret arguments.
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
