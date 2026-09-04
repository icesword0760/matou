# Task 3 Report — one-time impact confirmations

## Status

Completed. The service is implemented and committed as `feat: add host action confirmations`.

## RED

Command run before implementation:

```bash
pnpm --filter @matou/runtime exec vitest run src/control/host-action-confirmation-service.test.ts
```

Observed result:

```text
FAIL src/control/host-action-confirmation-service.test.ts [ src/control/host-action-confirmation-service.test.ts ]
Error: Cannot find module './host-action-confirmation-service'
```

## GREEN

Focused verification command:

```bash
pnpm --filter @matou/runtime exec vitest run src/control/host-action-confirmation-service.test.ts \
  && pnpm check:identifiers \
  && pnpm --filter @matou/runtime typecheck \
  && git diff --check
```

Result:

```text
Test Files  1 passed (1)
Tests  6 passed (6)
matou check:identifiers: success
@matou/runtime typecheck: success
git diff --check: success
```

## Delivered behavior

- Added `HostActionConfirmationService` with in-memory private `Map` storage and no persistence.
- Confirmation records bind caller run/session, action, stable target reference, scope, projection revision, and a SHA-256 hash of a deterministic canonical `HostImpactSummary` payload.
- Default lifetime is exactly 120,000 ms; expiry is enforced at the boundary (`now >= expiresAt`) and expired entries are purged during issue/consume.
- References use 24 random bytes encoded as base64url. Successful consumption deletes the record before returning, making each confirmation one-time.
- Caller mismatch and missing/consumed references return `CONFIRMATION_REQUIRED`; expired references return `CONFIRMATION_EXPIRED`; changed action/target/scope/revision/impact returns `CONFIRMATION_STALE`.
- Added `revokeRun(runId)` for run-end cleanup. Restart naturally clears all confirmations because they are process-memory state.
- Added lifecycle tests for reference shape, caller/action/target/revision/hash binding, one-time use, exact TTL, stale revision/impact, insertion-order-independent hashing, and run revocation.

## Self-review

The issue path takes a defensive impact snapshot, and consume returns a defensive record copy. Caller mismatch is intentionally indistinguishable from a missing reference so a caller cannot probe another run's pending confirmation. Stale records remain available for a retry with the original binding, while expired and consumed records are removed.

## Concerns

None.

## Fix round 1 — purge expired records before every consume outcome

### Finding addressed

A caller-mismatch consume returned `CONFIRMATION_REQUIRED` before running expiry cleanup, leaving unrelated expired confirmations in the private map. The consume path now snapshots the requested record and its expiry state, purges all expired entries first, then applies caller masking: a mismatched caller still receives `CONFIRMATION_REQUIRED`, while the rightful caller of an expired record receives `CONFIRMATION_EXPIRED` even though that record was purged.

### RED

Added a regression test where an expired record and a live record coexist. A mismatched caller consumes the live reference, then the rightful caller accesses the expired reference.

Command:

```bash
pnpm --filter @matou/runtime exec vitest run src/control/host-action-confirmation-service.test.ts
```

Observed result before the fix:

```text
Test Files  1 failed (1)
Tests  7 (6 passed, 1 failed)
FAIL purges expired records even when another caller submits a live reference
Expected code: CONFIRMATION_REQUIRED
Received code: CONFIRMATION_EXPIRED
```

### GREEN

Command:

```bash
pnpm --filter @matou/runtime exec vitest run src/control/host-action-confirmation-service.test.ts \
  && pnpm --filter @matou/runtime typecheck \
  && pnpm check:identifiers \
  && git diff --check
```

Result:

```text
Test Files  1 passed (1)
Tests  7 passed (7)
@matou/runtime typecheck: success
matou check:identifiers: success
git diff --check: success
```

### Self-review

`consume()` invokes `#purgeExpired()` before every success and failure path. It retains only a local reference to the requested record long enough to distinguish a rightful expiry from a missing reference; caller mismatch is checked before that distinction, so another run receives no expiry-state signal. The existing exact-boundary expiry and one-time-consume behavior remain covered.
