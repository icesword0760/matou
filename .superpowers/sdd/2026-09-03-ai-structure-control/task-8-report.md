# Task 8 report: Host Control structure capabilities

## Status

Task 8 is complete on `codex/ai-structure-control`. All 15 structural/navigation actions now cross the existing framed Host Control socket with independent capability checks, and every new Matou-managed Shell, Claude Code, or Codex Run receives the complete run-bound scope set.

## User-visible behavior delivered

- A managed terminal or AI session can call every Task 7 create, Fork, remove, Canvas-close, focus, and switch action through the same local `mt` control transport already used for terminal read/input.
- Each structural/navigation method is authorized independently. A token holding one method does not gain any neighboring method.
- A stale ordinal/ref projection now returns `STALE_PROJECTION`, allowing the caller to list and resolve again rather than treating the change as an opaque conflict.
- Facade error codes such as confirmation expiry/staleness, path/branch/Worktree conflicts, partial success, navigation timeout, and read-only storage reach the caller unchanged.
- Caller removal and Canvas-close responses are queued before Runtime disposes the caller process. The committed success result remains authoritative even if the deadline passes after the product mutation.
- Shell, Claude Code, and Codex keep their existing control-asset injection, provider model selection, and persisted permission metadata while gaining the 15 new scopes.

## Implementation

### Host Control scope and dispatch

- `HostControlScope` now includes `HostActionMethod` in addition to every legacy Host/terminal/Task scope.
- `HostControlBackend.executeHostAction(method, caller, params)` carries the typed Task 7 result.
- `HostControlServer` recognizes all 15 methods during request parsing, checks the exact requested capability, and delegates before listing or resolving terminal targets.
- `ControlErrorCode` is exported and includes the Task 1 facade error-code union. Recognized coded faults retain their original code and message; legacy ordinal/ref staleness maps to `STALE_PROJECTION`.

### Exact result-object handoff and startup order

- `RuntimeControlBackend.setHostActionExecutor(executor)` installs the facade only after it has been completely constructed.
- Before installation, `executeHostAction` rejects with coded `RUNTIME_NOT_READY`; no partial facade is reachable.
- After installation, `executeHostAction` directly returns the executor Promise without spreading, cloning, or serializing its result. Task 7's non-enumerable committed marker and queued post-response effects therefore remain attached to the same object.
- Writable Runtime initialization installs the executor immediately after constructing `RuntimeHostActionFacade` and before `hostControl.start()`, so the socket starts accepting requests only after the full action boundary exists.

### Managed Run capabilities

- Added frozen `MANAGED_SESSION_CONTROL_SCOPES` containing the 7 existing Host/terminal methods followed by all 15 structural/navigation methods.
- The existing single token-issuance path now uses that frozen constant for every profile and every new Run.
- Existing Run identity, 24-hour expiry, Runtime-generation binding, and `revokeRun` cleanup remain unchanged.
- Provider config and permission persistence code paths were not changed.

## TDD evidence

### Baseline

Command:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/host-control-server.test.ts \
  src/control/runtime-control-backend.test.ts \
  src/runtime-server.test.ts
```

Observed before Task 8 tests: exit code `0`; `3` files and `122/122` tests passed.

### RED

The new authorization/error/delegation, uninstalled-executor, exact-object, stale-projection, self-removal, and three-profile capability tests were written before production changes. The same focused command then produced:

```text
Test Files  3 failed (3)
Tests       34 failed | 120 passed (154)
```

Expected failures showed unsupported new methods, missing backend `setHostActionExecutor`/`executeHostAction`, legacy `CONFLICT`, and an absent managed scope constant.

### GREEN

Final focused command:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/host-control-server.test.ts \
  src/control/runtime-control-backend.test.ts \
  src/runtime-server.test.ts
```

Observed: exit code `0`; `3` files and `155/155` tests passed.

## Complete automated verification

Complete Runtime regression:

```bash
pnpm --filter @matou/runtime test
```

Observed: exit code `0` across all three configured phases:

- non-`runtime-server`/non-long-journal phase: `105` files, `871/871` tests;
- RuntimeServer phase: `1` file, `102/102` tests;
- long journal-range phase: `1` file, `9/9` tests;
- total: `107` files, `982/982` tests.

Static and repository gates:

```bash
pnpm --filter @matou/runtime typecheck
pnpm check:identifiers
git diff --check
```

Observed: all exited `0`. Database-backed tests emitted only Node's existing experimental SQLite warning.

## Coverage and self-review

- Verified the allow/deny matrix separately for every one of the 15 action scopes and asserted action dispatch does not enumerate or resolve terminal targets.
- Verified all 14 Task 1 facade fault codes retain their code/message and `RUNTIME_NOT_READY` also crosses the framed response.
- Verified the backend returns the exact facade object by identity, with both committed and post-response Symbol metadata still present and executable.
- Verified a `structure.remove.commit` caller-removal request receives a success frame before its post-response disposal stops Host Control.
- Verified stale legacy ref projection now produces `STALE_PROJECTION` while existing terminal read/input capability tests remain green.
- Verified real spawned Shell, Claude Code, and Codex fixtures receive all 22 expected scopes, retain control assets/PATH, retain configured model and permission arguments/metadata, exclude Task telemetry write scope, and lose their token after Run disposal.
- Re-read Runtime initialization to confirm executor installation occurs after facade construction and before `hostControl.start()`.
- Reviewed the final diff for accidental provider-mode, permission-mode, token lifetime, terminal dispatch, or serialization changes; none were introduced.

## Code / automation / real App boundary

- **Code:** Task 8 Host Control dispatch, executor wiring, error mapping, startup order, and managed capability injection are implemented.
- **Automated verification:** focused tests, complete Runtime regression, TypeScript, identifier policy, and whitespace gates passed at the counts above.
- **Real App:** a packaged Matou App was not launched in this Task. Real natural-language/CLI flows and visible navigation remain for the later CLI, provider-instruction, navigation-bridge, and end-to-end tasks.

## Concerns

No open code concern within Task 8 scope. The Host Control socket path is covered with real framed local sockets and real managed PTY profile launches; packaged-App interaction evidence is intentionally not claimed here.
