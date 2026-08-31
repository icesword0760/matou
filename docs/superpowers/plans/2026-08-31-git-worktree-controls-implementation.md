# Matou Git and Worktree Controls Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Add Codex-aligned branch, commit, push, and Matou-managed Worktree controls to the bottom HUD.

**Architecture:** A focused runtime `GitWorkspaceService` owns Git subprocesses and status parsing. Typed Runtime RPC methods expose the service to a portal-based renderer menu, while existing Worktree and SessionCanvas services provide persistent execution contexts and Shell entry.

**Tech Stack:** TypeScript, Node `execFile`, SQLite, Electron Runtime RPC, React, Vitest, Testing Library.

**Spec:** `docs/superpowers/specs/2026-08-31-git-worktree-controls-design.md`

## Global Constraints

- Git commands use `execFile` argument arrays and never interpolate user input into a Shell command.
- No force checkout, force push, reset, clean, or force Worktree removal.
- Worktrees persist until an explicit safe removal.
- Git detail loads on demand; no background polling loop.
- Each user operation emits one visible result message.

---

### Task 1: Typed Git status and branch operations

**Files:**
- Create: `packages/contracts/src/git.ts`
- Modify: `packages/contracts/src/index.ts`
- Modify: `packages/contracts/src/protocol.ts`
- Create: `apps/runtime/src/git/git-workspace-service.ts`
- Test: `apps/runtime/src/git/git-workspace-service.test.ts`

**Interfaces:**
- Produces: `GitRepositoryStatus`, `GitCheckoutResult`, `GitWorkspaceService.status(cwd)`, `checkout(cwd, branch)`, `createBranch(cwd, branch)`.

- [x] Write integration tests that initialize a temporary repository and assert branch ordering, file counts, line stats, upstream state, safe checkout, and blocked checkout paths.
- [x] Run `pnpm --filter @matou/runtime test -- git/git-workspace-service.test.ts` and observe missing-module failure.
- [x] Implement porcelain parsers and branch commands with `execFile`.
- [x] Run the focused test and obtain a passing result.
- [x] Commit contracts and branch service.

### Task 2: Commit and push operations

**Files:**
- Modify: `apps/runtime/src/git/git-workspace-service.ts`
- Modify: `apps/runtime/src/git/git-workspace-service.test.ts`

**Interfaces:**
- Produces: `commit(cwd, { message, includeUnstaged })`, `push(cwd)` returning refreshed `GitRepositoryStatus`.

- [x] Add failing tests for staged-only commit, include-all commit, missing message, first push upstream creation, and subsequent push.
- [x] Run the focused tests and verify each new case fails for the missing behavior.
- [x] Implement `git add -A`, `git commit`, remote selection, and upstream push without force flags.
- [x] Run the focused tests and obtain a passing result.
- [x] Commit commit/push service behavior.

### Task 3: Worktree discovery and persistent entry

**Files:**
- Modify: `apps/runtime/src/worktrees/worktree-service.ts`
- Modify: `apps/runtime/src/worktrees/worktree-service.test.ts`
- Modify: `apps/runtime/src/session-canvas/session-canvas-service.ts`
- Modify: `apps/runtime/src/session-canvas/session-canvas-service.test.ts`
- Modify: `apps/runtime/src/git/git-workspace-service.ts`
- Modify: `apps/runtime/src/git/git-workspace-service.test.ts`

**Interfaces:**
- Produces: Worktree summaries in `status`, `WorktreeService.registerExisting`, safe managed creation/removal, and `createShellSibling` ExecutionContext/CWD override.

- [x] Add failing tests for Worktree porcelain discovery, session counts, external context registration, dirty retention, and Shell cwd override.
- [x] Run focused Runtime tests and verify missing behavior failures.
- [x] Implement Worktree metadata merge, context registration, safe creation/removal, and Shell binding override.
- [x] Run focused Runtime tests and obtain passing results.
- [x] Commit Worktree runtime behavior.

### Task 4: Runtime RPC and immediate HUD refresh

**Files:**
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts`

**Interfaces:**
- Produces RPC methods `git.status`, `git.checkout`, `git.create-branch`, `git.commit`, `git.push`, `git.worktree-create`, `git.worktree-open`, and `git.worktree-remove`.

- [x] Add failing router tests for all Git methods and RuntimeServer test proving a Git mutation republishes HUD state.
- [x] Run focused router/server tests and verify unsupported-method failures.
- [x] Wire services, validate payloads, return typed results, and refresh attached HUDs after mutations.
- [x] Run focused tests and obtain passing results.
- [x] Commit RPC wiring.

### Task 5: Bottom HUD Git and Worktree menu

**Files:**
- Create: `apps/desktop/src/renderer/src/hud/GitControlMenu.tsx`
- Create: `apps/desktop/src/renderer/src/hud/GitControlMenu.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hud/TerminalHud.tsx`
- Modify: `apps/desktop/src/renderer/src/hud/TerminalHud.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/DetachedTerminalApp.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy.css`

**Interfaces:**
- Consumes: typed Git RPC methods and `window.matouDesktop.revealDirectory`.
- Produces: branch/commit and Worktree tabs, centered conflict confirmation, serialized operations, and one result banner.

- [x] Add failing component tests for opening the menu, branch filtering, checkout, conflict-to-commit handoff, commit button states, push, Worktree reveal/open/create/remove, and single feedback.
- [x] Run focused Desktop tests and verify missing component failures.
- [x] Implement the menu as focused subcomponents with stable portal positioning and accessible labels.
- [x] Run focused Desktop tests and obtain passing results.
- [x] Commit renderer behavior.

### Task 6: Full regression and packaged-app verification

**Files:**
- Modify only files required by failures found during verification.

**Interfaces:**
- Produces a clean feature branch ready to merge.

- [x] Run `pnpm test`.
- [x] Run `pnpm typecheck`.
- [x] Run `pnpm build`.
- [x] Run `pnpm package:dir` and the Git HUD packaged smoke scenario.
- [x] Run `git diff --check` and inspect `git status --short`.
- [x] Commit verification fixes and final acceptance evidence.
