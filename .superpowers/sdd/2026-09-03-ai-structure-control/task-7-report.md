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

---

## Fix round 1 addendum (2026-09-03)

This addendum supersedes the earlier test counts and records the replay, focus, and Runtime-lifecycle hardening completed after review.

### Code changes

- **Durable public Fork identity:** migration 32 adds an opaque public-request fingerprint and a resolved replay receipt to `fork_batch_ledger`. The fingerprint binds caller Session, batch key, the original source selector, and the ordered public item fields. Resolved window, mount, current projection, and environment lookup state stay outside that identity. The resolved receipt stores the stable source plus resolved environments and omits prompt text.
- **Receipt-first replay:** batch replay checks the durable public receipt before source or environment resolution. Explicit-ref and relative selectors therefore replay after projection drift and after a database/facade reconstruction. A different selector that currently reaches the same Session conflicts with the accepted public input.
- **Single-Fork compensation:** accepted workflow results re-enter the Task 6 coordinator for readiness, start, and prompt receipts. The workflow-to-ledger interruption gap derives the stable accepted source from the durable Fork relation, then completes start and prompt delivery once.
- **Focus stability:** caller-window and mutation-target-window focus are snapshotted independently. Each child mutation restores both windows immediately; the same restoration runs before and after start/readiness/prompt waits, including cross-window single and batch Forks.
- **Unified structural disposal:** `RuntimeServer.disposeSessions` cancels queued recovery, serializes against spawn/recovery, interrupts active Run records, clears provider/CWD/summary timers and recovery waiters, detaches all Runtime views, clears replay/input/HUD state, unregisters terminal backends, revokes Run capabilities and confirmations, and then ends the PTY even while journal durability is paused.
- **Caller response ordering:** a Runtime-local, non-serialized post-response effect carries caller retirement through the Host Control result. `HostControlServer` writes the authoritative success frame before executing the effect; an elapsed post-dispatch deadline does not replace an already-committed self-removal result.
- **Path faults:** existing invalid Workspace path state now raises `WorkspacePathInvalidError` throughout create and Fork entry points and maps uniformly to `PATH_CONFLICT`.
- **Existing product rules retained:** failed-only Task 6 retries, at-most-once prompt delivery, leaf node removal, the final-Canvas rule, and project directory/Git branch/Worktree retention remain delegated to their existing services.

### TDD evidence for the review findings

- The paused-journal lifecycle regression was added first and failed with Run status `running` where `interrupted` was expected; the unified disposal transition made it pass.
- The post-dispatch deadline regression was added first and received a `TIMEOUT` response where the committed removal result was expected; post-response effect detection made it pass.
- Receipt-first explicit-ref, relative-source, changed-selector, workflow-to-ledger compensation, per-child focus, and cross-window slow-readiness cases were added before the associated facade/coordinator changes.

### Current automated verification

Focused Task 7 suite:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/runtime-host-action-facade.test.ts
```

Result: `1` file, `36/36` tests passed.

Related create/Fork/remove/close/Host Control/Runtime/migration regressions:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/fork-batch-coordinator.test.ts \
  src/control/runtime-host-action-facade.test.ts \
  src/control/host-control-server.test.ts \
  src/storage/migration-runner.test.ts \
  src/hierarchy/hierarchy-application-service.test.ts \
  src/session-canvas/session-canvas-service.test.ts \
  src/session-canvas/fork-workflow-service.test.ts \
  src/runtime-server.test.ts
```

Result: `8` files, `265/265` tests passed.

Complete Runtime regression:

```bash
pnpm --filter @matou/runtime exec vitest run
```

Result: `107` files, `932/932` tests passed. Output contained the existing Node experimental SQLite warning only.

### Task 8 handoff boundary

Task 7 continues to expose the fully constructed facade as `WritableRuntimeState.hostActions` and keeps `HostControlServer.start()` behind the completed preflight graph. Task 8 owns `RuntimeControlBackend.setHostActionExecutor(executor)`, installs that executor before `hostControl.start()`, and returns the facade result object directly so its Runtime-local post-response effect reaches `HostControlServer`. This round adds the response-ordering primitive and Runtime disposal API only; it leaves backend structure-method dispatch to Task 8.

### Code / automation / real App boundary after fix round 1

- **Code:** review findings 1–4 are implemented and self-reviewed in the Task 7 boundary plus the narrow Host Control response/disposal lifecycle seam.
- **Automated verification:** focused, related, and complete Runtime suites passed at the counts above. `pnpm --filter @matou/runtime typecheck`, `pnpm check:identifiers`, and `git diff --check` also passed after the final code and report update.
- **Real App:** 本 Task 未验收；真实 CLI 自移除与完整 UI interaction acceptance remain in the later integration and acceptance tasks.

---

## Fix round 2 addendum (2026-09-03)

This addendum records the second review closure. The scoped ruling excludes compatibility adapters for development-only migrations 29–31. Migration 32 remains the authoritative public-request receipt schema and its existing migration coverage remains unchanged.

### Code changes

1. **Operation-owned focus preservation**
   - The facade now snapshots the actual active Session in each unique caller and Fork-target window immediately before the structural mutation. A background caller is no longer treated as the visible focus.
   - `SessionCanvasService.restoreFocusedSessionIfCurrent` performs the restore transactionally only when the window still shows the expected temporary Session with the operation's focus timestamp and the snapshot still belongs to its original active Canvas.
   - Each accepted child is restored once immediately after create/retry. Later readiness, startup, and prompt boundaries carry the child identity but do not repeatedly overwrite a user's newer focus.
   - Child, sibling, same-window, cross-window, batch, background-caller, manual-switch, and concurrently removed snapshot cases are covered. A vanished snapshot leaves the accepted Fork result intact.

2. **Committed results remain authoritative**
   - Every successful create, single/batch Fork, remove commit, and Canvas-close commit carries a non-enumerable Runtime-local committed marker. It is omitted from JSON while remaining visible to `HostControlServer`.
   - The post-dispatch deadline now yields to that committed marker. A structure transaction that has completed is returned as success even when create/Fork coordination or cleanup crosses the caller deadline.
   - All domain-returned Session disposal IDs, including non-caller IDs, are now one post-response effect. Disposal begins only after the success frame is queued; cleanup exceptions are emitted as post-response diagnostics and do not replace the committed result or `activePath`.

3. **Provider-hook lifecycle follows Session/Run ownership**
   - Runtime tracks Claude hook registrations by Session and Run in a registry shared across Runtime connections that own the same PTY registry.
   - Structural disposal revokes every tracked hook before ending the PTY, awaits endpoint/settings/statusline cleanup, and then clears Runtime/HUD state. The registration's dispose Promise is idempotent, so retirement and structural disposal converge on the same cleanup.
   - Coverage proves cleanup for healthy and paused-durability Runs, including disposal from a replacement Runtime connection; the retired URL returns 404 and its old identity callback stays inactive.

4. **Active recovery removal is terminal**
   - Scheduler cancellation now removes both queued and actively restoring candidates. Coordinator tombstones prevent late ordinary completion, late external Fork tracking, or late ready/failed settlement from publishing a ghost card.
   - Both coordinator-level deferred recovery and `RuntimeServer.disposeSessions` racing an active deferred recovery are covered.

5. **Archived Session state is preserved**
   - `SessionRepository.interruptRun` always records the active Run as interrupted, while conditionally preserving an already archived Session's `status`, `work_status`, and `archived_at`.
   - A domain Session removal followed by Runtime structural disposal now retains `status = archived` and its original archive timestamp while the Run becomes interrupted.

### TDD evidence

The round-2 regressions were added before the implementation changes. The initial focused run produced:

```text
Test Files  4 failed (4)
Tests       16 failed | 148 passed (164)
```

Those failures exposed caller-derived focus snapshots, unconditional focus rewrites, accepted-result replacement by deadline/cleanup faults, retained provider-hook files and endpoints, an active recovery ghost, and archived Session status being overwritten as interrupted.

Final focused lifecycle run:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/runtime-host-action-facade.test.ts \
  src/control/host-control-server.test.ts \
  src/recovery/runtime-recovery-coordinator.test.ts \
  src/runtime-server.test.ts
```

Result: `4` files, `166/166` tests passed.

Final related create/Fork/remove/close/Host Control/recovery/provider-hook run:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/runtime-host-action-facade.test.ts \
  src/control/host-control-server.test.ts \
  src/control/fork-batch-coordinator.test.ts \
  src/recovery/runtime-recovery-coordinator.test.ts \
  src/recovery/runtime-session-recovery-scheduler.test.ts \
  src/session/provider-hook-server.test.ts \
  src/domain/session-repository.test.ts \
  src/hierarchy/hierarchy-application-service.test.ts \
  src/session-canvas/session-canvas-service.test.ts \
  src/session-canvas/fork-workflow-service.test.ts \
  src/runtime-server.test.ts
```

Result: `11` files, `315/315` tests passed.

Final complete Runtime regression:

```bash
pnpm --filter @matou/runtime exec vitest run
```

Result: `107` files, `946/946` tests passed. Output contained only the existing Node experimental SQLite warning.

### Task 6 and Task 8 boundaries

- Task 6 remains the sole owner of durable batch acceptance, failed-item retry, readiness, startup, and prompt receipts. This round only adds the temporary child identity to its focus callback; no facade-local batch/retry path was introduced.
- Task 8 still owns `RuntimeControlBackend.setHostActionExecutor(executor)` and installs it before `hostControl.start()`. Its executor must return the facade result object directly so the non-enumerable committed marker and post-response effects reach `HostControlServer`.

### Code / automation / real App boundary after fix round 2

- **Code:** all five round-2 findings are implemented and self-reviewed in the facade plus the narrow focus, Host Control response, Runtime disposal, hook, recovery, and Run-state lifecycle seams.
- **Automated verification:** focused and related suites plus the complete Runtime regression passed at the counts above. Runtime typecheck, identifier policy, and whitespace gates are recorded after the final commit preparation.
- **Real App:** this Task did not run packaged-App acceptance. Real CLI structure dispatch remains in Task 8, and end-to-end interaction acceptance remains in the later integration tasks.

### Self-review and remaining concern

- Confirmed focus restore is a compare-and-set against an operation-owned temporary identity/timestamp, not an unconditional navigation write.
- Confirmed all structural disposal is post-response and that cleanup faults stay diagnostic after the stable success result.
- Confirmed structural disposal revokes hook endpoints before PTY termination and survives a Runtime connection handoff.
- Confirmed recovery tombstones reject all late publication paths and archived domain state survives Run interruption.
- Confirmed migration 29–31 adapters were not added, migration 32 behavior was retained, Task 6 durability was not bypassed, and Task 8 backend dispatch was not implemented early.
- Remaining integration boundary: Task 8 must preserve the exact facade result object through its executor; packaged-App behavior is still pending later acceptance.

---

## Fix round 3 addendum (2026-09-03)

This addendum records the per-item focus-lease and archived work-status evidence requested in the third review round. The prior ruling still excludes adapters for development-only migrations 29–31; migration 32 and its public-request receipt semantics are unchanged.

### Code changes

1. **Each batch item owns a fresh focus lease**
   - `ForkBatchCoordinator` now asks for a focus lease immediately before each external create or failed-item retry. The lease is released immediately after that structural mutation's compare-and-set restoration.
   - The next item captures the caller and target windows again after the prior item's readiness/start/prompt stage. A focus selected by the user while the first item waits therefore becomes the second item's snapshot rather than being replaced by the batch's initial focus.
   - Startup, readiness, and prompt delivery no longer invoke an old restoration callback. They retain Task 6's existing durable receipt transitions without reapplying a stale focus snapshot.

2. **Concurrent Fork mutations do not snapshot one another's temporary child**
   - Facades sharing a Runtime database use short per-window mutation leases across the caller and target windows. Window IDs are acquired in stable order, so overlapping same-window and cross-window operations serialize without a lock-order cycle.
   - Snapshot capture occurs only after the relevant window leases are acquired. The lease covers the external structural create/retry and its immediate CAS restore, then releases before readiness/start/prompt waits.
   - A concurrent batch therefore observes the restored or newly user-selected focus, never the other batch's operation-owned temporary child.

3. **First-CAS snapshot disappearance remains non-fatal**
   - The regression removes the snapshotted Session inside the Fork workflow after the child has been temporarily activated and before the facade's first restore attempt.
   - The CAS sees that the snapshot is no longer an active member, leaves the current legal fallback/child focus in place, and preserves the already accepted Fork result.

4. **Archived work status has explicit preservation evidence**
   - The Runtime disposal regression now assigns `work_status = needs-input` before domain removal and asserts that `status = archived`, `work_status = needs-input`, and the original `archived_at` all survive disposal while the active Run becomes `interrupted`.
   - This strengthens the round-2 state-preservation test; the existing conditional `interruptRun` transition required no additional production change.

### TDD evidence

The three focus regressions were added before the focus-lease implementation. The initial facade run produced:

```text
Test Files  1 failed (1)
Tests       2 failed | 42 passed (44)
```

The failures were the intended ones: the second slow-start item restored batch-initial focus instead of the user's newer focus, and a concurrent batch entered its structural mutation while the first batch still exposed a temporary child. The first-CAS disappearance case already exercised the round-2 best-effort CAS behavior and remained green.

### Automated verification

Focused facade, coordinator, and Runtime disposal run:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/runtime-host-action-facade.test.ts \
  src/control/fork-batch-coordinator.test.ts \
  src/runtime-server.test.ts
```

Result: `3` files, `171/171` tests passed.

Related create/Fork/remove/close/Host Control/recovery/provider-hook run:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/control/runtime-host-action-facade.test.ts \
  src/control/host-control-server.test.ts \
  src/control/fork-batch-coordinator.test.ts \
  src/recovery/runtime-recovery-coordinator.test.ts \
  src/recovery/runtime-session-recovery-scheduler.test.ts \
  src/session/provider-hook-server.test.ts \
  src/domain/session-repository.test.ts \
  src/hierarchy/hierarchy-application-service.test.ts \
  src/session-canvas/session-canvas-service.test.ts \
  src/session-canvas/fork-workflow-service.test.ts \
  src/runtime-server.test.ts
```

Result: `11` files, `318/318` tests passed.

Complete Runtime regression:

```bash
pnpm --filter @matou/runtime exec vitest run
```

Result: `107` files, `949/949` tests passed. Output contained the existing Node experimental SQLite warning only.

Final gates:

```bash
pnpm --filter @matou/runtime typecheck
pnpm check:identifiers
git diff --check
```

Results: Runtime TypeScript check passed, identifier policy passed, and diff whitespace check passed.

### Task 6 and Task 8 boundaries

- Task 6 remains the sole owner of durable batch acceptance, ordered item processing, failed-item retry, readiness, startup, and prompt receipts. Round 3 replaces only its focus callback seam with a per-structural-item lease factory; no facade-local batch/retry or prompt-delivery path was introduced.
- Task 8 still owns `RuntimeControlBackend.setHostActionExecutor(executor)` and installs it before `hostControl.start()`. The facade's committed-result marker and post-response effects continue to pass through the executor unchanged.

### Code / automation / real App boundary after fix round 3

- **Code:** per-item snapshots, operation-owned CAS restoration, overlapping-window serialization, first-CAS disappearance handling, and archived work-status preservation are represented in the implementation and regression assertions.
- **Automated verification:** focused, related, and complete Runtime suites passed at the counts above; final static gates are listed separately after the last verification run.
- **Real App:** packaged-App acceptance was outside this Task. Task 8 still owns live Host Control structure dispatch, and later integration tasks own end-to-end interaction evidence.

### Self-review

- Confirmed each new/retried item acquires and releases exactly one focus lease around only its external structural mutation.
- Confirmed a slow readiness/start/prompt stage holds no window lease and performs no stale restore, so later user focus remains authoritative.
- Confirmed shared, sorted caller/target window acquisition serializes overlapping batches while allowing disjoint Runtime windows to proceed independently.
- Confirmed an archived or concurrently removed snapshot makes restoration a no-op without replacing the durable Fork outcome.
- Confirmed Task 6 ledger writes, retry generations, and delivery receipts remain in their existing coordinator paths; Task 8 backend dispatch and migration compatibility scope remain unchanged.
