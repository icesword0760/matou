# Task 7 report: Runtime Host action facade

## Status

Task 7 is complete on `codex/ai-structure-control`. `RuntimeHostActionFacade.execute(method, caller, params)` is now the Runtime-side product mutation boundary for create, Fork, remove, and Canvas-close actions. Navigation remains connected in its planned later task.

## User-visible behavior delivered

- Workspace, Task, Canvas, and Session creation accepts final titles, returns stable refs plus human-readable hierarchy paths, and preserves the caller's current focus unless `enter: true` is explicit.
- Single child/sibling Fork and child batches resolve the submitted environment before first acceptance. Single Fork startup and prompt delivery reuse the durable Task 6 readiness/delivery receipts; batch partial success and failed-item-only retry still run through `ForkBatchCoordinator`.
- Replaying an accepted new-Worktree Fork continues to return its accepted node after its branch exists. Reusing the same key with changed title, environment, prompt, start choice, source, or item order returns `PATH_CONFLICT` before another workflow side effect.
- Remove and Canvas-close preview return the latest affected Tasks, Canvases, Sessions, descendants, live Runs, and terminal processes. Leaf subtree requests become the product's node-removal scope.
- Commit re-resolves the stored stable target, recomputes the all-scope projection revision and impact, and then consumes the caller/run-bound confirmation once. A changed target, revision, impact, or action returns `CONFIRMATION_STALE`.
- Workspace, Task, Session-branch, and Canvas operations call the existing domain services. Returned Session IDs are stopped, and the response carries the resulting active hierarchy path.
- The existing final-Canvas rule remains authoritative: closing the final Canvas of the final Task hides the window, reports zero removals, and leaves its Session running.
- Structure removal does not remove project files, Git branches, repository contents, or Worktree rows/directories.

## Implementation

### Facade and result normalization

- Added `apps/runtime/src/control/runtime-host-action-facade.ts`.
- Parses each request exactly once through `parseHostActionRequest`.
- Builds canonical SHA-256 command metadata from caller Session, method payload, and stable submission key.
- Normalizes create/Fork results into stable refs, product titles, paths, environment decisions, and `created`/`ready`/`started` states.
- Maps resolver, confirmation, storage, path, branch, Worktree, duplicate-name, command-key, and target faults to the Task 1 `HostActionErrorCode` values.
- Coalesces concurrent identical single-Fork submissions and rejects concurrent changed input for the same key.

### Task 6 durability boundary

The facade never recreates batch execution or retry logic. Every child batch still calls `ForkBatchCoordinator.createChildren` or `retryFailures`.

Two narrow coordinator APIs were added:

- `preflightAccepted` validates an accepted batch's public request before the facade reuses its already-resolved branch/Worktree reservation; the coordinator still performs its full durable resolved fingerprint check afterward.
- `coordinateAcceptedFork` applies the same durable readiness, startup, and prompt-delivery receipt states to a single Fork already accepted by the Task 5 workflow.

This preserves restart-durable replay, failed-only retry, prompt at-most-once behavior, and same-key/different-input conflicts instead of introducing a facade-local batch cache or retry path.

### Confirmation and token lifecycle

- Added a caller-bound, expiry-aware `HostActionConfirmationService.inspect` operation so commit can read stored action metadata before fresh target/revision/impact recomputation; `consume` remains the only one-shot deletion point.
- Added `CapabilityTokenService`'s synchronous `onRunRevoked` hook and wired it to `HostActionConfirmationService.revokeRun`.

### Runtime composition and Task 8 handoff

- Writable Runtime state now exposes `hostActions`.
- Runtime constructs the hierarchy service, resolver, confirmation service, Task 6 coordinator, and facade before opening Host Control for requests.
- `HostControlServer.start()` is intentionally delayed until after the complete facade/Fork graph exists.
- `RuntimeControlBackend` is unchanged in this task. Task 8 owns `setHostActionExecutor(executor)` and installs `hostActions` at the existing preflight boundary before `hostControl.start()`.

## TDD evidence

### RED 1: missing facade

Command after the first focused tests, before the production module existed:

```bash
pnpm --filter @matou/runtime exec vitest run src/control/runtime-host-action-facade.test.ts
```

Observed: exit code `1`; the suite failed to load because `runtime-host-action-facade` was missing.

### RED 2: adversarial replay and conflict cases

After adding durable replay, collision, and concurrency regressions before the corresponding hardening:

```text
Test Files  1 failed (1)
Tests       5 failed | 15 passed (20)
```

The failures showed:

- accepted single/batch new-Worktree requests were revalidated as fresh branch conflicts;
- changed batch input could surface `BRANCH_CONFLICT` before the durable key conflict;
- concurrent single-Fork changed input entered the workflow twice;
- hierarchy collision errors were not normalized.

Separate RED checks also proved the previous final-Canvas preview reported one removal despite the domain no-op rule, and an action-mismatched confirmation returned a target-type fault instead of `CONFIRMATION_STALE`.

### GREEN: focused facade

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/runtime-host-action-facade.test.ts
```

Result: `1` file passed, `24/24` tests passed.

### GREEN: related create/Fork/remove/close/Runtime regressions

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/runtime-host-action-facade.test.ts \
  src/control/host-action-confirmation-service.test.ts \
  src/control/host-action-target-resolver.test.ts \
  src/control/host-control-server.test.ts \
  src/control/fork-batch-coordinator.test.ts \
  src/hierarchy/hierarchy-application-service.test.ts \
  src/session-canvas/session-canvas-service.test.ts \
  src/session-canvas/fork-workflow-service.test.ts \
  src/session-canvas/fork-workflow-service.integration.test.ts \
  src/runtime-server.test.ts
```

Result: `10` files passed, `243/243` tests passed.

### GREEN: complete Runtime suite

```bash
pnpm --filter @matou/runtime test
```

Result:

- main Runtime suite: `105` files, `810/810` tests passed;
- Runtime server suite: `1` file, `96/96` tests passed;
- journal range suite: `1` file, `9/9` tests passed.

The database-backed suites emitted only Node's existing experimental SQLite warning.

## Verification gates

```bash
pnpm --filter @matou/runtime typecheck
pnpm check:identifiers
git diff --check
```

Results: Runtime TypeScript check passed, identifier policy passed, and diff whitespace check passed.

## Coverage highlights

- Four title-aware create paths, focus preservation, explicit enter, same-input replay, and changed-input conflict.
- Single child Fork normalization, typed branch/readiness errors, concurrent single-flight, new-branch replay, durable startup/readiness/prompt delivery, and fresh-coordinator no-resend replay.
- Three-child success, partial success, failed-only retry, no successful-node duplication, all-environment preflight, changed durable input, and accepted new-Worktree replay.
- Session node/subtree removal, leaf normalization, Task removal, Workspace removal, non-final Canvas close, and final-Canvas behavior.
- Fresh revision plus impact-only staleness, action binding, one-shot use, expiry path through the service regressions, and synchronous Run revocation.
- Session stop callbacks, post-action active paths, project file/branch retention, and Worktree row/directory retention.
- `STORAGE_READ_ONLY`, `PATH_CONFLICT`, `BRANCH_CONFLICT`, `WORKTREE_CONFLICT`, resolver faults, and confirmation faults.

## Code / automation / real App boundary

- **Code:** implemented and self-reviewed for the Task 7 scope.
- **Automated verification:** focused, related regression, full Runtime, typecheck, identifier, and whitespace gates passed as recorded above.
- **Real App:** not run in this task; packaged CLI, Host Control exposure, navigation, and full interaction acceptance belong to Tasks 8–14.

## Self-review

- Confirmed every fresh Fork environment is resolved before coordinator/workflow acceptance, while an already accepted durable reservation is replayed rather than treated as a new collision.
- Confirmed batch creation and retry always delegate to Task 6 and that single start delivery uses Task 6's persisted receipt states.
- Confirmed all destructive commits require a live caller-bound confirmation and consume it only after fresh stable-target, revision, and impact checks.
- Confirmed only domain-returned affected Session IDs are stopped.
- Confirmed the final-Canvas preview and commit both reflect the existing no-removal rule.
- Confirmed no Worktree cleanup service, Git branch deletion, repository deletion, or project-directory deletion is called.
- Confirmed Runtime construction leaves the Task 8 setter installation point before Host Control begins accepting requests.

## Concerns

No open concern within Task 7. Host Control dispatch and capability exposure are deliberately handed to Task 8; navigation execution is deliberately handed to Task 11; real-App evidence remains in the later acceptance tasks.
