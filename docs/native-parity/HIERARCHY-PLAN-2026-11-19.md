# Session hierarchy & lifecycle — tracked plan (2026-11-19)

> Condensed from the read-only study `hierarchy-plan-01` (evidence: `gaps.md`, `model.md`, `plan.md`, `result.json` under `artifacts/native-parity/native/evidence/hierarchy-plan-01/`, git-ignored).
> Static source evidence only; no runtime/DB/UI verification in the study. All paths below are repository-relative.

## Entry and level model (frozen)

- Current entry: `HierarchyShell → SessionCanvas → SessionCarousel → TerminalPane`; `StoppedSessionCard` for archived nodes; `SplitTree` is only the fallback when graph+snapshot are unavailable (`HierarchyShell.tsx:1113-1126`).
- Level parent: per scene override, else the focused session's structural parent (`SessionCanvas.tsx:43-44`). Siblings are **direct children** of that parent (`childrenOf(parent)`); root = children of none (`SessionCanvas.tsx:46-47`).
- Geometry owner key: `session-group:<scene>:<parentId|root>` (`SessionCanvas.tsx:48`).
- Breadcrumb count = level siblings count including archived (`HierarchyShell.tsx:841-842`).
- Back: set level to the parent's parent (or root), focus the parent, request terminal focus; also ArrowLeft/ArrowUp inside the button (`HierarchyShell.tsx:929-942`, `SessionBreadcrumb.tsx:16-23`).
- Badge entry: set level to the node and focus the preferred unarchived child — error(0) > needs-input(1) > running/starting(2) > interrupted(3) > idle(4) > exited(5), ties by graph order; no terminal-focus request (`HierarchyShell.tsx:1256-1261,1492-1502`).
- Same-level creation: `createShellSibling(sceneId, focusedSessionId, activeLevelParentId)` (`HierarchyShell.tsx:958`).

## Gates

1. **Level view + breadcrumb + child badges** (Batch A, in progress): desktop-only; uses the existing graph/memberships; adds the level map, level-aware geometry, breadcrumb/back, badge counts/summary/tooltip, and the sibling parent argument. Archived cards keep today's rendering until gate 4.
2. **Per-card close and remove scopes** (core + runtime): replicate `session-canvas-service.ts:581-794` and `hierarchy-application-service.ts:1582-1709`:
   - remove-branch: descendants via `derived-from`/`forked-from` limited to scene memberships; `node-only` reparents direct children in `sibling_created_seq` order; both scopes keep at least one membership; sessions rows and project files/worktrees are never deleted.
   - close: requires `confirmedIntent = delete-session:<id>` when the workspace has one session, a structural child, or running/needs-input work; archives the session, detaches scene windows, activates a preferred sibling, and for the last session either keeps the canvas (`preserveSceneOnLastSession`) or archives scene→task→(default task creation).
3. **Authoritative exit/removal events** (runtime): per-card command results carry graph/focus/removed IDs; normal exit, no-output early exit, spawn failure and recovery failure are distinguished by session and attempt; worker teardown reuses the existing retirement path.
4. **Canvas UI** (desktop): stopped/archived cards (title, "正在恢复会话…", last ≤8 lines, conditional remove), normal-exit and start-failure cards with reason/retry/remove, remove-node dialog with scope impact counts.
5. **Provider fork / worktree** (core+runtime+UI): three placements (child/sibling/peer), current vs new worktree (Git HEAD + setup), progress/failure/retry; must first confirm the native provider CLI/identity and Git path.

## Known ambiguities to settle with the running product

- When the stopped card refreshes after a normal close; where the frozen UI triggers "restore stopped session".
- Back from an archived parent (activation may refuse).
- Final state of detached windows and other windows after removal.
- `archived_at`/`status` races between close and natural exit.
