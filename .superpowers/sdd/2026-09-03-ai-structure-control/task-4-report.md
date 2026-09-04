# Task 4 Report — title-aware, focus-preserving create workflows

## Status

Completed and verified. Create workflows now expose the exact created hierarchy separately from the window's current path, accept final titles, and preserve the current focus when requested. Existing Renderer calls that omit navigation continue to activate the new entity.

## RED

Command run after adding the create-workflow regression tests and before implementation:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/hierarchy/hierarchy-application-service.test.ts \
  src/session-canvas/session-canvas-service.test.ts \
  src/rpc/runtime-rpc-router.test.ts
```

Observed result:

```text
Test Files  3 failed (3)
Tests  8 failed | 61 passed (69)
```

The failures matched the missing behavior:

- hierarchy and RPC results had no `created` path;
- explicit Task/Canvas titles and `navigation: 'preserve'` were not consumed;
- `createSessionSibling` did not exist;
- the named Canvas and provider-session projections therefore remained unavailable.

## GREEN

Final focused verification command:

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/hierarchy/hierarchy-application-service.test.ts \
  src/session-canvas/session-canvas-service.test.ts \
  src/rpc/runtime-rpc-router.test.ts \
  && pnpm --filter @matou/runtime typecheck \
  && pnpm check:identifiers \
  && git diff --check
```

Result:

```text
Test Files  3 passed (3)
Tests  69 passed (69)
@matou/runtime typecheck: success
matou check:identifiers: success
git diff --check: success
```

Additional Runtime regression command:

```bash
pnpm --filter @matou/runtime test
```

Result:

```text
Main Runtime suite: 101 files passed, 739 tests passed
Runtime server suite: 1 file passed, 96 tests passed
Journal range suite: 1 file passed, 9 tests passed
```

## Delivered behavior

- Added `CreateNavigationOptions`, `CreatedHierarchyPath`, and additive `CreateHierarchyResult.created` contracts.
- `createWorkspace` and `createTask` capture the window navigation inside their existing domain transaction, create or select the requested hierarchy, optionally restore the prior navigation, and return both the created hierarchy and the current path.
- `createTask` accepts the final Task title before emitting `task.created`; omitted titles retain the existing sequential naming behavior.
- `createCanvas` accepts a final Canvas title, persists explicit titles as pinned, emits the final title in the first `scene.created` projection, and supports focus preservation.
- Added generic `createSessionSibling` for `shell`, `claude-code`, and `codex`; explicit titles are persisted as manual titles and omitted titles use the existing provider labels.
- New provider sessions do not receive a copied `provider_bindings` row, so later launch continues through the existing default provider configuration path.
- Kept `createShellSibling` as a compatibility adapter with `profile: 'shell'`.
- Runtime create RPCs accept the additive title/navigation fields while old Renderer requests that omit navigation retain activation behavior.
- Existing event names, hierarchy event payload shapes, sibling relation semantics, command deduplication, and transaction boundaries remain unchanged.

## Changed files

- `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- `apps/runtime/src/hierarchy/hierarchy-application-service.test.ts`
- `apps/runtime/src/session-canvas/session-canvas-service.ts`
- `apps/runtime/src/session-canvas/session-canvas-service.test.ts`
- `apps/runtime/src/rpc/runtime-rpc-router.ts`
- `apps/runtime/src/rpc/runtime-rpc-router.test.ts`
- `.superpowers/sdd/2026-09-03-ai-structure-control/task-4-report.md`

## Self-review

- Checked both sides of the result contract: `created` always identifies the new/selected hierarchy, while the top-level hierarchy and `navigation` represent the final current window path.
- Verified focus preservation at Workspace, Task, Canvas, and Session levels and retained old activation assertions for omitted navigation.
- Verified the first authoritative `task.created`, `scene.created`, and `session.created` payloads already contain the submitted final title.
- Verified all three ordinary session profiles and confirmed no provider history binding is inherited.
- Verified explicit Canvas titles are pinned consistently with the existing rename workflow.
- Mutation check: removing `created`, ignoring the title, activating after preservation, forcing Shell kind, or copying a provider binding each breaks a focused assertion.

## Concerns

None.
