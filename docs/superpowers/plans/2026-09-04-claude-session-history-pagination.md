# Claude Session History Pagination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Replace the 240-event and 100-session visibility caps with bounded pagination, complete-history search, stable match navigation, and virtualized rendering.

**Architecture:** Runtime builds and caches a lightweight byte-range index per Claude JSONL transcript, serves bounded pages, and searches complete transcripts in bounded batches. Renderer keeps only requested pages in state and virtualizes DOM rows while preserving scroll anchors.

**Tech Stack:** TypeScript, Node.js streams/file handles, React 19, TanStack React Virtual, Vitest, Testing Library, Playwright Electron.

**Spec:** `docs/superpowers/specs/2026-09-04-claude-session-history-pagination-design.md`

## Global Constraints

- Keep left metadata filtering separate from right transcript-content search.
- Preserve existing load confirmations and provider/model/permission restoration.
- Batch sizes must never become total visibility limits.
- Use stable one-based event indexes across page, search, and navigation APIs.
- Repository identifiers and user-facing naming follow `AGENTS.md`.

---

### Task 1: Paginated contracts

**Files:**
- Modify: `packages/contracts/src/claude-sessions.ts`
- Modify: `packages/contracts/src/protocol.ts`
- Test: `packages/contracts/src/protocol.test.ts`

**Interfaces:**
- Produces `ClaudeSessionEventPage`, `ClaudeSessionEventPageInfo`, and `ClaudeSessionSearchResult`.
- Adds `claude-sessions.search` to the RPC allowlist.

- [x] Write failing contract and allowlist tests for page metadata and search RPC.
- [x] Run the focused contracts tests and verify the new expectations fail.
- [x] Add the minimal types and RPC method.
- [x] Run the focused contracts tests and verify they pass.

### Task 2: Runtime transcript index and event paging

**Files:**
- Modify: `apps/runtime/src/session/claude-session-catalog.ts`
- Test: `apps/runtime/src/session/claude-session-catalog.test.ts`

**Interfaces:**
- Produces `list({cwd,query,offset,limit})` with full pagination metadata.
- Produces `detail({cwd,providerSessionId,beforeEventIndex?,aroundEventIndex?,limit})`.

- [x] Write failing tests for complete left pagination, latest event page, earlier page, around-event
      context, index reuse, and append invalidation.
- [x] Run the catalog tests and verify each new behavior fails for the missing API.
- [x] Replace full-event cache entries with lightweight indexed transcripts and bounded event reads.
- [x] Run catalog tests until all page and invalidation cases pass.

### Task 3: Complete-history search

**Files:**
- Modify: `apps/runtime/src/session/claude-session-catalog.ts`
- Test: `apps/runtime/src/session/claude-session-catalog.test.ts`

**Interfaces:**
- Produces `search({cwd,providerSessionId,query,offset,limit})` returning exact total and paged hits.

- [x] Write failing tests for a hit outside the latest page, hit pagination, Unicode/tool-result search,
      query normalization, and cache invalidation after append.
- [x] Run focused tests and verify expected failures.
- [x] Implement bounded-batch transcript search with signature-keyed lightweight hit caching.
- [x] Run focused tests and verify all search cases pass.

### Task 4: RPC and hierarchy command wiring

**Files:**
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.test.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts`

**Interfaces:**
- Routes paginated list/detail inputs and `claude-sessions.search`.
- Exposes typed renderer commands for list page, event page, and hit page.

- [x] Write failing RPC tests proving the 240 cap is gone and a distant hit opens by event index.
- [x] Run focused router tests and verify expected failures.
- [x] Implement minimal RPC validation/routing and typed renderer command adapters.
- [x] Run focused router tests and verify they pass.

### Task 5: Paginated and virtualized loader UI

**Files:**
- Modify: `apps/desktop/package.json`
- Modify: `pnpm-lock.yaml`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionLoaderDialog.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/session-canvas.css`
- Test: `apps/desktop/src/renderer/src/session-canvas/SessionLoaderDialog.test.tsx`

**Interfaces:**
- Consumes the typed list/event/search page commands from Task 4.
- Keeps `onLoad(providerSessionId)` unchanged.

- [x] Write failing UI tests for loaded/total status, left load-more, right prepend with stable anchor,
      exact complete match counter, distant-match navigation, and bounded DOM rows.
- [x] Run the focused dialog tests and verify expected failures.
- [x] Add TanStack React Virtual and implement the paginated virtual lists and navigation state machine.
- [x] Run focused dialog tests and verify they pass.

### Task 6: Product regression and packaged acceptance

**Files:**
- Modify: `tests/e2e/session-load-existing-claude.spec.ts`
- Create: `docs/acceptance/claude-session-history-pagination.md`

**Interfaces:**
- Verifies the complete user-visible flow in a production Electron build.

- [x] Add an Electron E2E fixture with more than 240 events and a search hit outside the initial page.
- [x] Run it before final UI completion and verify it catches the old cap.
- [x] Complete the interaction behavior, then run focused E2E, typecheck, identifier gate, and full tests.
- [x] Build `package:dir`, run the packaged scenario, and record evidence boundaries in the acceptance
      document.
- [x] Commit the completed branch with a concise feature message.
