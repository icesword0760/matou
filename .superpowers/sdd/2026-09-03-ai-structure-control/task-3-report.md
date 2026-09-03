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
