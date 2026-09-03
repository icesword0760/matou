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

---

## Fix round 1 — framed validation faults and ambiguity choices

### Status and user impact

The two P2 Host Control response gaps are closed:

- Malformed high-level action fields now return `INVALID_REQUEST` through the framed socket. Missing fields, unsupported profiles, invalid selectors, and extra fields receive a short message naming the field that the caller can correct; Zod issue arrays, schemas, causes, and stacks stay inside Runtime.
- `AMBIGUOUS_TARGET` now returns `error.details.candidates` in authoritative resolver order. Each candidate contains only `humanPath`, so CLI/AI callers can present the choices without receiving database result paths, stable/internal refs, session IDs, or resolver objects.
- The Runtime client parses the same documented details shape, preserves order, and strips undeclared candidate/detail properties. Errors other than `AMBIGUOUS_TARGET` retain the existing `{ code, message }` payload shape.

### TDD evidence

Tests were added before production changes. The initial focused run was:

```bash
pnpm --filter @matou/runtime exec vitest run --testTimeout=30000 \
  src/control/host-control-server.test.ts \
  src/control/host-control-client.test.ts
```

Observed RED: exit code `1`; `3` expected failures and `51` passes. The real facade validation path still returned `INTERNAL_ERROR`, the real resolver ambiguity frame omitted candidates, and the client discarded candidate details.

After the implementation, the same command passed `2` files and `54/54` tests. The broader Task 8 focused run was:

```bash
pnpm --filter @matou/runtime exec vitest run --testTimeout=120000 \
  src/control/host-control-server.test.ts \
  src/control/host-control-client.test.ts \
  src/control/runtime-host-action-facade.test.ts \
  src/control/runtime-control-backend.test.ts \
  src/runtime-server.test.ts
```

Observed: exit code `0`; `5` files and `205/205` tests passed.

### Complete automated verification

```bash
pnpm --filter @matou/runtime test
pnpm --filter @matou/runtime typecheck
pnpm check:identifiers
git diff --check
```

Observed:

- complete Runtime regression: exit code `0`, with `105` files / `874` tests in the main phase, `102` RuntimeServer tests, and `9` long journal tests (`985/985` total);
- TypeScript typecheck: exit code `0`;
- identifier policy gate: exit code `0`;
- diff whitespace check: exit code `0`.

Only the repository's existing Node experimental SQLite warnings appeared.

### Coverage and self-review

- The framed validation test installs a real `RuntimeHostActionFacade` and exercises its real strict Zod parser for four field-level failure classes; it does not use the permissive action-result fixture.
- The ambiguity test uses migrated Runtime storage, two real hierarchy paths, the real `HostActionTargetResolver`, and the real facade. Its controlled projector deliberately returns the two matching surfaces in reverse order; the framed response proves resolver sorting survives while internal candidate fields are removed.
- The client test feeds extra candidate/detail fields and verifies only ordered `humanPath` values enter `HostControlClientError.details`.
- Re-read the error boundary to confirm only recognized `AMBIGUOUS_TARGET` faults can receive details. `INVALID_REQUEST` and every other fault still serialize only `code` and `message`.
- No managed scope, token lifetime/revocation, startup order, terminal dispatch, model selection, or permission persistence path changed in this fix.

### Code / automation / real App boundary

- **Code:** validation normalization, controlled ambiguity serialization, and client parsing are implemented.
- **Automated verification:** real socket frames, real facade parsing, real resolver ordering, full Runtime regression, typecheck, identifier, and diff gates passed.
- **Real App:** no packaged Matou App was launched during this fix round; visible CLI/AI choice rendering remains outside Task 8's transport boundary.

### Concerns

No open code concern in Fix round 1 scope. The packaged-App interaction boundary still needs later end-to-end product verification and is not claimed here.

---

## Fix round 2 — complete ambiguity transport and authoritative action method

### Status and user impact

Both follow-up P2 gaps are closed:

- Host Control now preserves every validated `AMBIGUOUS_TARGET` candidate in resolver order. The server and client no longer truncate after five entries; later CLI/AI formatting can decide how to ask for a narrower target when more than five choices exist.
- Candidate transport remains limited to `humanPath`. Stable refs, result-path objects, session IDs, and undeclared detail fields do not cross the protocol boundary.
- Candidate validation is all-or-nothing. A mixed malformed candidate list or a path above `4096` UTF-8 bytes yields no partial details, avoiding a misleading subset. A path exactly at the byte limit remains valid.
- The existing frame-size limit now applies to outgoing responses as well as incoming requests. When a complete ambiguity payload exceeds it, the server sends a small framed `AMBIGUOUS_TARGET` fault with an explicit refine-filter message instead of truncating candidates. Other oversized responses receive a framed `INTERNAL_ERROR` size fault.
- `params.method` is always rejected as `INVALID_REQUEST`, whether it matches or conflicts with the outer framed method. The authenticated outer request method remains the only dispatch authority, and the correction message names `method`.

### TDD evidence

The new tests were written first. Initial command:

```bash
pnpm --filter @matou/runtime exec vitest run --testTimeout=30000 \
  src/control/host-action-types.test.ts \
  src/control/host-control-server.test.ts \
  src/control/host-control-client.test.ts
```

Observed RED: exit code `1`; `9` expected failures and `59` passes. Evidence covered same/different inner `method` acceptance, sixth-candidate loss on both server and client, partial acceptance of malformed details, over-limit path leakage, and lack of an outgoing frame-size fault.

After implementation, the same three files passed `68/68` tests. The broader Task 8 focused run was:

```bash
pnpm --filter @matou/runtime exec vitest run --testTimeout=120000 \
  src/control/host-action-types.test.ts \
  src/control/host-control-server.test.ts \
  src/control/host-control-client.test.ts \
  src/control/runtime-host-action-facade.test.ts \
  src/control/runtime-control-backend.test.ts \
  src/runtime-server.test.ts
```

Observed: exit code `0`; `6` files and `219/219` tests passed.

### Complete automated verification

```bash
pnpm --filter @matou/runtime test
pnpm --filter @matou/runtime typecheck
pnpm check:identifiers
git diff --check
```

Observed:

- complete Runtime regression: exit code `0`, with `105` files / `880` tests in the main phase, `102` RuntimeServer tests, and `9` long journal tests (`991/991` total);
- TypeScript typecheck: exit code `0`;
- identifier policy gate: exit code `0`;
- diff whitespace check: exit code `0`.

Only the existing Node experimental SQLite warnings appeared.

### Coverage and self-review

- Extended the migrated-storage, real-resolver, real-facade socket fixture to six distinct hierarchy paths, deliberately reversed at the projector boundary. Both the raw framed response and `HostControlClientError` contain all six paths in authoritative sorted order.
- Added server and client boundary tests for malformed mixed candidate arrays, exactly `4096` bytes, and `4097` bytes. Invalid arrays are discarded as a whole rather than filtered into a misleading partial choice list.
- Added a small-frame socket test proving oversized complete ambiguity details produce one valid, deterministic protocol frame with no candidate truncation.
- Added parser and real-facade framed tests for matching and conflicting inner `method` fields. Both name `method` and return `INVALID_REQUEST`.
- Re-read the response-size fallback and post-response flow: the fallback is written through the same length-prefixed protocol, while queued post-response effects still run after the write.
- No scope set, token lifecycle, startup order, terminal dispatch, model selection, or permission persistence code changed.

### Code / automation / real App boundary

- **Code:** full candidate transport, strict candidate parsing, outgoing response bounds, and outer-method authority are implemented.
- **Automated verification:** real socket/client round trips, parser/facade boundary tests, full Runtime regression, typecheck, identifier, and diff gates passed.
- **Real App:** no packaged Matou App was launched in this round; presentation rules for more than five choices remain assigned to later CLI/AI formatting work.

### Concerns

No open code concern in Fix round 2 scope. Packaged-App choice presentation remains a later end-to-end verification item and is not claimed here.
