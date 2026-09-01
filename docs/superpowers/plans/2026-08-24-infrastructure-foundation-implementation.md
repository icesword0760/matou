# Matou INF-01～INF-25 Implementation Plan

> **Execution rule:** each task starts with a failing test, then the minimum implementation, then targeted verification. The final gate is the requirement-by-requirement audit in Task 25.

**Goal:** turn the approved infrastructure specification into a Runtime-owned, recoverable data/control plane so feature work can start without changing persistence, session identity, event, migration, or authority boundaries.

**Architecture:** Electron Main only establishes channels. The app-scoped Runtime UtilityProcess exclusively owns SQLite, PTY processes, journals, checkpoints, host-control endpoints, migrations, and authoritative projections. Renderer is rebuilt from Runtime snapshots plus replayable domain events. Terminal bytes use a direct MessagePort with credit flow control.

**Stack:** TypeScript, Electron, React, xterm.js, node-pty, `node:sqlite`, Zod, Vitest, Playwright.

---

## Task 1 — INF-01 SQLite ownership and connection

- Add `apps/runtime/src/storage/database.test.ts` proving PRAGMAs, transaction rollback, storage queue ordering, generation metadata, and close behavior.
- Add `apps/runtime/src/storage/database.ts` and `storage-queue.ts`.
- Add a dependency-boundary test proving Desktop/Renderer cannot import `node:sqlite` or Runtime storage internals.
- Verify: `pnpm --filter @matou/runtime test -- database`.

## Task 2 — INF-02 schema migrations

- Add immutable migrations under `apps/runtime/src/storage/migrations/` for all INF-08～20 tables.
- Add checksum/history/backup/failure fixtures and `migration-runner.test.ts`.
- Refuse edited applied migrations and newer on-disk schemas.
- Verify migration from empty DB and legacy fixture.

## Task 3 — INF-03 domain transaction primitive

- Add `withDomainTransaction.test.ts` first.
- Implement `withDomainTransaction(command, mutate, emit)` with `BEGIN IMMEDIATE`, command deduplication, causation/correlation metadata, and a single returned commit envelope.
- Make repositories accept `DatabaseTransaction`; disallow direct connection construction.

## Task 4 — INF-04 Domain Events / Outbox

- Add event envelope schemas in `packages/contracts` and event/cursor models in `packages/domain`.
- Implement append, paged replay, per-consumer cursor, idempotent acknowledgement, and lag metrics.
- Add replay and reconnect tests with multiple consumers.

## Task 5 — INF-05 Journal V2

- Replace the prototype journal codec with versioned frames: output, resize, reset, encoding, exit, domain-cursor.
- Add checksums, sequence monotonicity, segment rotation, sealed compression, tail truncation repair, and middle-corruption quarantine.
- Preserve single-session fault isolation.

## Task 6 — INF-06 journal/event alignment

- Add `requiredTerminalSequence` to domain commits and domain-cursor markers to the journal.
- Implement `RecoveryWatermark` and crash-window repair for journal-first and SQLite-first failures.
- Add deterministic fault-injection tests at every fsync/commit/marker boundary.

## Task 7 — INF-07 paired checkpoints

- Implement terminal snapshot files plus SQLite checkpoint indexes containing terminal and domain-event sequences.
- Write temp + fsync + rename, retain paired generations, and fall back to the previous valid pair.
- Add checkpoint + tail replay tests.

## Task 8 — INF-08 Workspace / Task

- Expand `packages/domain` with authoritative records, archive state, ordering, invariants, and codecs.
- Implement repositories and RPC commands/queries using domain transactions.
- Test cascade policy, parent-task rules, archive/restore, and ordering.

## Task 9 — INF-09 ExecutionContext / Worktree

- Implement plain-directory and git-worktree contexts, path identity, branch/base metadata, lifecycle state, and cleanup plans.
- Keep dirty worktrees on session/task closure; require explicit cleanup confirmation.
- Test real temporary git repositories and crash recovery.

## Task 10 — INF-10 Session / SessionRun / ProviderBinding

- Split logical identity, process attempts, and provider resume identities.
- Connect PTY lifecycle to SessionRun without making a pane/mount authoritative.
- Test provider resume success/failure, new run creation, exit recovery, and per-session isolation.

## Task 11 — INF-11 relation local event sourcing

- Implement append-only relation events plus same-transaction current projection.
- Enforce one active direct fork parent, endpoint/task constraints, cycle rules, and append-only history.
- Implement sibling derivation and flat team relations; test graph properties.

## Task 12 — INF-12 Scene structural state

- Implement Scene, SceneNode, SessionMount, and SceneWindow structural repositories and events.
- Test tile/card/DAG mode, split trees, attach/detach, cross-window moves, and invalid mount references.

## Task 13 — INF-13 geometry state

- Implement debounced/coalesced geometry writes outside Outbox with monotonic `layoutRevision`.
- Test stale writer rejection, invalid reference cleanup, and structural-event silence.

## Task 14 — INF-14 Agent adapters

- Define Claude Code, Codex, and generic-shell adapter contracts and normalized semantic events.
- Add fixture parsers for structured stream/hook/transcript inputs with stable provider IDs and confidence.
- Test idempotency and malformed event isolation.

## Task 15 — INF-15 anchors

- Implement Semantic, CommandOutput, and ScreenCapture anchor repositories/resolvers.
- Integrate OSC 133 command boundaries and journal retention degradation.
- Test repeated text, alternate-screen epochs, missing journal ranges, and captured-text fallback.

## Task 16 — INF-16 Runtime RPC, projection, and Host Control Plane

- Version command/query/subscription/terminal/geometry/diagnostics contracts.
- Implement request timeout/cancel/idempotency, snapshot + cursor reconnect, and stale-response protection.
- Add private Unix socket/Named Pipe control server, per-run capability tokens, stable ordinal revisions, bounded reads, key allowlist, and structured errors.
- Test Renderer reload, Runtime restart, default deny, token expiry, stale ordinals, and control-server fault isolation.

## Task 17 — INF-17 annotations, artifacts, validation, Task telemetry

- Implement repositories/events for annotations, artifacts, validation runs.
- Implement generation-scoped status/progress/log state and subscription with capacity bounds.
- Test anchor degradation and telemetry generation cleanup.

## Task 18 — INF-18 notification, preferences, feature campaigns

- Build session-memory notification projections and persistent preferences.
- Add dedup/cooldown and stable navigation target resolution.
- Implement bundled campaign manifests and idempotent local seen-state keyed by campaign version.

## Task 19 — INF-19 retention/privacy

- Add per-session/global quotas, journal/checkpoint retention, archive/purge plans, secure permissions, and anchor degradation updates.
- Test dry-run plans, archive/purge separation, audit events, and disk quota behavior.

## Task 20 — INF-20 observability and preset capability registry

- Add structured metrics/diagnostics without raw terminal payloads.
- Implement manifest/state/suppression, process locking, offline seed, checksum verification, atomic version switch, upgrade rollback, and user-uninstall suppression.
- Test drift detection and concurrent reconciliation.

## Task 21 — INF-21 reference product importer

- Add `compat/legacy-bridge` contracts and fixture copies derived from reference product snapshot/checkpoint/metadata/journal shapes.
- Implement idempotent import, source fingerprint, entity mapping, repair report, provider/team restoration, and consistency report.

## Task 22 — INF-22 shadow write

- Implement legacy mutation mapping, checkpoint bootstrap, metadata tailing, projection diff, lag metrics, and command-id repair queue.
- Test that Matou failure never blocks the legacy write path and never grants legacy snapshots authority over SQLite.

## Task 23 — INF-23 read switch

- Implement feature-flagged SQLite read authority, legacy compatibility backup writer, rollback, and telemetry.
- E2E restore, provider resume, relation correctness, and projection equality.

## Task 24 — INF-24 legacy retirement

- Remove/forbid Renderer authoritative snapshot export and legacy metadata authority outside the compatibility package.
- Keep read-only importer for the defined backup window.
- Add static authority and dependency scans.

## Task 25 — INF-25 completion gate

- Run all 37 test-matrix classes in the design specification.
- Run `pnpm test`, `pnpm typecheck`, `pnpm build`, `pnpm test:e2e`, packaged Electron smoke, corruption/fault-injection suites, and boundary scans.
- Write `docs/architecture/infrastructure-completion-audit.md` with one direct evidence row for every INF-01～25 requirement and every admission condition.
- Only declare feature-development readiness when every row has current passing evidence.
