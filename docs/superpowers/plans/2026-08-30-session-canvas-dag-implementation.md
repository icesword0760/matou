# Matou 会话画布与 DAG 分支交互实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 完整交付事项内多画布、通用会话关系、横向浏览、Claude Fork/worktree、恢复状态以及独立 DAG 导航，并通过 102 条隔离真实端到端用例。

**Architecture:** Runtime 继续作为 SQLite、会话关系、PTY、provider 身份和排序序号的唯一权威源；主 Renderer、DAG Renderer 和独立终端窗口都通过独立 MessagePort 消费同一投影。结构关系、画布归属和恢复状态进入事务与 Outbox，滚动、缩放和节点位置进入现有 geometry 存储，Electron Main 只管理原生窗口。

**Tech Stack:** TypeScript 7、Electron 43、React 19、xterm.js 6、node-pty、`node:sqlite`、Zod、Vitest、Testing Library、Playwright。

**Spec:** `docs/superpowers/specs/2026-08-30-session-canvas-dag-design.md`

## Global Constraints

- 每张新画布直接创建并聚焦一个普通 Shell。
- Session 是稳定关系节点，Shell/Claude Code 是当前运行形态。
- Shell 与 Claude Code 都可拥有父节点、子节点和兄弟节点。
- 用户主动退出 Claude Code 后回到 Shell，不展示退出提示。
- Claude Code 恢复失败后回到 Shell，展示失败原因和重试入口。
- 普通横向新增直接创建 Shell 兄弟；只有有效 Claude Code 提供 Fork 能力。
- Runtime 是 SQLite 唯一写入者，Renderer 只维护可重建投影。
- 结构变化使用事务加 Outbox；几何变化不进入 Outbox。
- 终端高频字节继续绕过 Electron Main。
- 每项生产行为先用失败测试证明缺口，再实现到测试通过。
- E2E 使用 `/tmp/matou-e2e-<run-id>` 隔离数据和真实 PTY/Git/provider 流程。
- 完成门槛是独立 QA Agent 逐条执行并标记 102 条用例全部通过。

---

### Task 1: 领域类型、membership 与 migration 14

**Files:**
- Modify: `packages/domain/src/model.ts`
- Modify: `packages/domain/src/events.ts`
- Modify: `packages/domain/src/index.ts`
- Modify: `apps/runtime/src/storage/migrations.ts`
- Test: `packages/domain/src/model.test.ts`
- Test: `apps/runtime/src/storage/migration-runner.test.ts`

**Interfaces:**
- Produces: `SessionCurrentMode`, `SessionWorkStatus`, `ProviderRestoreState`, `SessionCanvasMembership`, `SessionGraphNode`, `SessionGraphEdge`, `SceneSessionGraph`.
- Produces: `session_canvas_memberships`, `runtime_sequences` and provider restore columns from Spec section 7.2.

- [ ] **Step 1: Write failing domain model tests**

```ts
it('models a stable session node separately from its current mode', () => {
  const node: SessionGraphNode = graphNode({
    currentMode: 'shell',
    parentSessionId: 'session-parent',
    relationKind: 'forked-from',
    providerRestoreState: 'failed'
  })
  expect(node.currentMode).toBe('shell')
  expect(node.parentSessionId).toBe('session-parent')
  expect(node.relationKind).toBe('forked-from')
})
```

- [ ] **Step 2: Write failing migration and backfill tests**

Create a version-13 fixture with one Scene, two mounted Sessions and one existing `forked-from` relation. Migrate it and assert both Sessions receive memberships in the original Scene, creation sequences are stable, the relation remains current, and sequence rows exist.

- [ ] **Step 3: Verify RED**

Run: `pnpm --filter @matou/domain test -- model && pnpm --filter @matou/runtime test -- migration-runner`
Expected: FAIL because the types and migration 14 tables are absent.

- [ ] **Step 4: Implement exported types and migration 14**

Add the exact interfaces and SQL from Spec section 7. Backfill membership from each Session's earliest active or archived mount. Allocate `sibling_created_seq` in deterministic Scene/order/id order.

- [ ] **Step 5: Verify migration integrity**

Run: `pnpm --filter @matou/domain typecheck && pnpm --filter @matou/runtime test -- migration-runner runtime-database-bootstrap`
Expected: PASS, including checksum, idempotency and future-version tests.

- [ ] **Step 6: Commit**

```bash
git add packages/domain apps/runtime/src/storage
git commit -m "feat: add session canvas graph schema"
```

### Task 2: 统一结构父节点与关系投影

**Files:**
- Modify: `apps/runtime/src/relations/session-relation-repository.ts`
- Modify: `apps/runtime/src/relations/session-relation-repository.test.ts`
- Create: `apps/runtime/src/session-canvas/session-graph-repository.ts`
- Create: `apps/runtime/src/session-canvas/session-graph-repository.test.ts`

**Interfaces:**
- Produces: `getStructuralParent`, `listStructuralChildren`, `listSiblings`, `appendStructuralRelation`.
- Produces: `SessionGraphRepository.getMembership`, `createMembership`, `nextSequence`, `projectSceneGraph`.

- [ ] **Step 1: Write failing combined-parent tests**

```ts
it.each(['derived-from', 'forked-from'] as const)(
  'treats %s as the sole structural parent relation',
  (kind) => {
    repository.appendStructuralRelation(command(), relation(kind, 'parent', 'child'))
    expect(repository.getStructuralParent('child')).toMatchObject({ kind, sourceSessionId: 'parent' })
  }
)
```

Also assert a second structural parent, cross-Scene edge and cycle are rejected while non-structural relations remain allowed.

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- session-relation-repository`
Expected: FAIL because structural helpers do not exist and sibling calculation only understands Fork.

- [ ] **Step 3: Implement relation invariants**

Query `session_relations_current` for both structural kinds, validate Scene/Task/Workspace ownership, and perform ancestor traversal for cycle detection before appending the relation event and projection row in the caller's transaction.

- [ ] **Step 4: Write graph projection tests**

Build a Scene with root Shell/Claude siblings, one derived Shell child, one Fork child and one archived child. Assert active sibling groups, history counts, mode counts and both edge styles.

- [ ] **Step 5: Implement graph repository and projection**

Join memberships, Sessions, provider bindings, worktrees, structural relations, HUD state and latest journal summaries. Sort daily sibling groups by interaction sequence and keep graph `sibling_created_seq` stable.

- [ ] **Step 6: Run and commit**

Run: `pnpm --filter @matou/runtime test -- session-relation-repository session-graph-repository`
Expected: PASS.

```bash
git add apps/runtime/src/relations apps/runtime/src/session-canvas
git commit -m "feat: model structural session relationships"
```

### Task 3: 协议、命令与 Runtime 投影

**Files:**
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `packages/contracts/src/protocol.test.ts`
- Modify: `packages/contracts/src/domain-events.ts`
- Modify: `packages/contracts/src/domain-events.test.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.test.ts`
- Modify: `apps/desktop/src/renderer/src/projection/RuntimeProjectionStore.ts`
- Modify: `apps/desktop/src/renderer/src/projection/RuntimeProjectionStore.test.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts`

**Interfaces:**
- Produces: RPC methods and Zod schemas from Spec section 9.1.
- Produces: `sessionGraphs: Record<SceneId, SceneSessionGraph>` in snapshot and incremental events.

- [ ] **Step 1: Add failing protocol tests**

Assert every new method accepts its exact input, rejects missing `windowId/sceneId/sessionId`, validates worktree mode and branch display name length, and rejects unknown fields.

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/contracts test`
Expected: FAIL because RPC methods and graph event schemas are absent.

- [ ] **Step 3: Add contracts and router dispatch**

Add `hierarchy.create-canvas`, `create-shell-sibling`, `create-fork-child`, `create-fork-sibling`, `record-session-interaction`, `retry-provider-restore`, `reopen-historical-session`, `get-scene-session-graph`, and `set-focused-session`. Keep old `fork-session` mapped to child Fork.

- [ ] **Step 4: Add failing projection gap/rebuild tests**

Feed snapshot, ordered graph updates, a duplicate event and a sequence gap. Assert duplicates are ignored, incremental state updates, and a gap requests a fresh snapshot without mixing old nodes.

- [ ] **Step 5: Implement Renderer graph projection**

Normalize graphs by Scene, update node/edge records immutably, and reuse existing reconnect/gap recovery.

- [ ] **Step 6: Run and commit**

Run: `pnpm --filter @matou/contracts test && pnpm --filter @matou/runtime test -- runtime-rpc-router && pnpm --filter @matou/desktop test -- RuntimeProjectionStore`
Expected: PASS.

```bash
git add packages/contracts apps/runtime/src/rpc apps/desktop/src/renderer/src/projection apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts
git commit -m "feat: expose session graph runtime protocol"
```

### Task 4: SessionCanvasService 与默认 Shell 画布

**Files:**
- Create: `apps/runtime/src/session-canvas/session-canvas-service.ts`
- Create: `apps/runtime/src/session-canvas/session-canvas-service.test.ts`
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Test: `apps/runtime/src/runtime-server.test.ts`

**Interfaces:**
- Produces: `createCanvas`, `createShellSibling`, `projectSceneGraph`, `setFocusedSession`.
- Consumes: existing Session/Scene/mount repositories, PTY start workflow, `SessionGraphRepository`.

- [ ] **Step 1: Write failing atomic new-canvas test**

```ts
it('creates a scene and focused root Shell in one transaction', async () => {
  const result = await service.createCanvas(command('canvas-1'), {
    windowId, taskId, now: 100
  })
  expect(result.session).toMatchObject({ kind: 'shell', sceneId: result.scene.id })
  expect(result.graph.nodes).toHaveLength(1)
  expect(result.graph.nodes[0]?.parentSessionId).toBeUndefined()
  expect(navigation(windowId).activeSceneId).toBe(result.scene.id)
})
```

Also test unique sequential names, invalid workspace path, command replay and PTY start failure preserving the Scene error card.

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- session-canvas-service`
Expected: FAIL because the service is absent.

- [ ] **Step 3: Implement canvas and root Shell workflow**

Use one domain transaction for Scene, Session, mount, membership, navigation and Outbox. Start PTY after commit; map start failure into node error.

- [ ] **Step 4: Write and implement Shell sibling tests**

Cover root sibling and child-list sibling. The root sibling has no relation; the child-list sibling receives `derived-from` to the list parent. New sessions get the next creation sequence, interaction sequence zero, and append last.

- [ ] **Step 5: Wire Runtime server and projection events**

Runtime owns focus state; startup snapshot includes every active Scene graph. Existing split-horizontal RPC maps to `create-shell-sibling` in the new canvas UI, while split-vertical remains available only for legacy data migration tests.

- [ ] **Step 6: Run and commit**

Run: `pnpm --filter @matou/runtime test -- session-canvas-service runtime-server hierarchy-application-service`
Expected: PASS.

```bash
git add apps/runtime/src/session-canvas apps/runtime/src/hierarchy apps/runtime/src/runtime-server.ts
git commit -m "feat: create session canvases and shell siblings"
```

### Task 5: 用户交互序号与稳定动态排序

**Files:**
- Create: `apps/runtime/src/session-canvas/session-interaction-service.ts`
- Create: `apps/runtime/src/session-canvas/session-interaction-service.test.ts`
- Modify: `apps/runtime/src/session-canvas/session-canvas-service.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`

**Interfaces:**
- Produces: `record(sessionId, 'submit' | 'control' | 'provider-action')`.
- Produces: `terminal.user-interaction` direct-port message immediately before the related input bytes.

- [ ] **Step 1: Write failing Runtime ordering tests**

Create three siblings, record interactions on the second then third, and assert list order is third/second/first. Restart the database and assert order persists. Also assert click/output/draft event types are rejected by the protocol.

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- session-interaction-service`
Expected: FAIL because the sequence service is absent.

- [ ] **Step 3: Implement monotonic sequence transaction**

Increment `runtime_sequences.value`, update only the target membership and emit `session.user-interacted` in one transaction. Validate that the direct port owns the Session.

- [ ] **Step 4: Write failing TerminalSurface input classification tests**

Assert ordinary characters produce only terminal bytes; Enter produces `submit` then bytes; Ctrl+C/Ctrl+D produce `control` then bytes; mouse selection, paste draft without Enter and output produce no interaction marker.

- [ ] **Step 5: Implement input classification**

Inspect xterm `onData` payloads for newline/control bytes. Preserve exact byte order and current flow control.

- [ ] **Step 6: Run and commit**

Run: `pnpm --filter @matou/runtime test -- session-interaction-service runtime-server && pnpm --filter @matou/desktop test -- TerminalSurface`
Expected: PASS.

```bash
git add apps/runtime/src/session-canvas apps/runtime/src/runtime-server.ts apps/desktop/src/renderer/src/terminal
git commit -m "feat: order siblings by true user interaction"
```

### Task 6: Claude 当前形态与统一恢复状态机

**Files:**
- Create: `apps/runtime/src/session-canvas/provider-mode-service.ts`
- Create: `apps/runtime/src/session-canvas/provider-mode-service.test.ts`
- Modify: `apps/runtime/src/session/provider-hook-server.ts`
- Modify: `apps/runtime/src/session/provider-hook-server.test.ts`
- Modify: `apps/runtime/src/session/provider-resume-monitor.ts`
- Modify: `apps/runtime/src/session/provider-resume-monitor.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.test.tsx`

**Interfaces:**
- Produces: `markClaudeActive`, `markUserExited`, `markRestoreFailed`, `retryRestore`, `canFork`.
- Consumes: provider semantic events, hooks, durable binding and current Session process kind.

- [ ] **Step 1: Write failing state transition tests**

```ts
it('returns a manually exited Claude node to Shell without a failure badge', async () => {
  await service.markUserExited(command(), sessionId, 20)
  expect(session(sessionId).kind).toBe('shell')
  expect(binding(sessionId)).toMatchObject({ restoreState: 'none' })
  expect(graph(sessionId).parentSessionId).toBe(parentId)
})
```

Add restore-failure-to-Shell, retry success, retry failure, double-click idempotency and relation preservation cases.

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- provider-mode-service provider-hook-server provider-resume-monitor`
Expected: FAIL because the unified mode/restore state is absent.

- [ ] **Step 3: Implement valid-conversation and mode detection**

Mark `canFork=true` only after first real user message, durable provider identity and normal Stop/completion. Record user exit from provider process semantics separately from unexpected process loss.

- [ ] **Step 4: Implement restore and retry workflow**

Use the same binding and Session node. Set `restoring`, call real resume, then transition to Claude/none or Shell/failed. Update one recovery notification per Session.

- [ ] **Step 5: Implement UI states**

Shell with failed restore shows `Claude Code 恢复失败`, reason and `重试恢复`; manual exit shows ordinary Shell. Both retain child badge. Fork icon is visible only in Claude mode and enabled only after valid conversation.

- [ ] **Step 6: Run and commit**

Run: `pnpm --filter @matou/runtime test -- provider-mode-service provider-hook-server provider-resume-monitor runtime-server && pnpm --filter @matou/desktop test -- TerminalPane`
Expected: PASS.

```bash
git add apps/runtime/src/session-canvas apps/runtime/src/session apps/runtime/src/runtime-server.ts apps/desktop/src/renderer/src/hierarchy/TerminalPane*
git commit -m "feat: unify Claude mode and recovery behavior"
```

### Task 7: Fork 子会话、Fork 兄弟与 worktree 选择

**Files:**
- Create: `apps/runtime/src/session-canvas/fork-workflow-service.ts`
- Create: `apps/runtime/src/session-canvas/fork-workflow-service.test.ts`
- Create: `apps/runtime/src/session-canvas/branch-name.ts`
- Create: `apps/runtime/src/session-canvas/branch-name.test.ts`
- Modify: `apps/runtime/src/worktrees/worktree-service.ts`
- Modify: `apps/runtime/src/worktrees/worktree-service.test.ts`
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- Create: `apps/desktop/src/renderer/src/session-canvas/BranchDialog.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/BranchDialog.test.tsx`

**Interfaces:**
- Produces: `createForkChild`, `createForkSibling`, `retryFork`, `removeFailedFork`.
- Produces: `validateDisplayName`, `createGitBranchName(displayName, sessionId)`.

- [ ] **Step 1: Write failing branch-name tests**

Cover empty/65-codepoint input, trimmed Unicode, active sibling duplicate, punctuation slugging and deterministic short-id collision avoidance. Assert invalid submissions preserve input.

- [ ] **Step 2: Write failing Fork relation tests**

Assert child Fork points to source Claude. Non-root Fork sibling points to the common Claude parent and inherits that parent's provider context, not the current sibling's later output. Root Fork sibling and invalid Claude attempts return typed product errors.

- [ ] **Step 3: Verify RED**

Run: `pnpm --filter @matou/runtime test -- branch-name fork-workflow-service`
Expected: FAIL because workflow and validation modules are absent.

- [ ] **Step 4: Implement current-worktree Fork**

Create a starting node/relationship intent transaction, call existing real provider Fork using the source durable identity, then commit binding/status. Failure keeps the node with retry and remove actions.

- [ ] **Step 5: Implement new-worktree Fork**

Resolve real Git root and `HEAD`, call `WorktreeService.create()` with Matou-managed path and generated branch, then launch provider Fork in the new execution context. A non-Git source disables this choice with `需要 Git 仓库`.

- [ ] **Step 6: Implement BranchDialog**

Display name field, current/new worktree cards, uncommitted-change explanation, progress, inline validation and retry/remove states. Focus the name field on open and return focus to source on cancel.

- [ ] **Step 7: Run and commit**

Run: `pnpm --filter @matou/runtime test -- branch-name fork-workflow-service worktree-service hierarchy-application-service && pnpm --filter @matou/desktop test -- BranchDialog`
Expected: PASS with actual temporary Git repositories in worktree tests.

```bash
git add apps/runtime/src/session-canvas apps/runtime/src/worktrees apps/runtime/src/hierarchy apps/desktop/src/renderer/src/session-canvas
git commit -m "feat: create Claude forks with worktree choices"
```

### Task 8: 会话横向列表、四卡布局与焦点

**Files:**
- Create: `apps/desktop/src/renderer/src/session-canvas/SessionCanvas.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/SessionCanvas.test.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/SessionCarousel.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/SessionCarousel.test.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/SessionCard.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/SessionHeader.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/session-canvas.css`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/SceneTabBar.tsx`

**Interfaces:**
- Consumes: `SceneSessionGraph`, Runtime RPC methods, existing `TerminalPane` and xterm surface.
- Produces: focused sibling level, visible-card virtualization and `ensureSessionVisible(sessionId)`.

- [ ] **Step 1: Write failing layout tests**

Render 1–7 siblings and assert at most four are inside the visible viewport, cards use stable keys, horizontal overflow exists after four, new Shell appends last and current focus receives active styling distinct from Scene selection.

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- SessionCanvas SessionCarousel`
Expected: FAIL because components are absent.

- [ ] **Step 3: Implement canvas and carousel**

Replace the central split renderer for migrated session canvases. Preserve old mount data as ownership input. Use a horizontal CSS layout, IntersectionObserver/ResizeObserver and stable Session keys; mount xterm only for visible cards while Runtime sessions continue.

- [ ] **Step 4: Implement hover expansion and FLIP ordering**

Hover target uses `clamp(420px,44vw,760px)`, pauses while scrolling and restores after 120ms. On `session.user-interacted`, animate stable DOM nodes to new order and keep active terminal focused.

- [ ] **Step 5: Implement automatic visibility/focus**

For new canvas/session, level change and DAG navigation, await render and xterm fit, call nearest/center scroll, issue a focus token, and protect user focus moved elsewhere.

- [ ] **Step 6: Add Scene Tab graph button and horizontal add**

Scene `+` creates a new default Shell canvas. Level `+` creates a Shell sibling. Add keyboard-accessible `会话关系 (⌥Tab)` graph button. Remove the downward-create UI entry.

- [ ] **Step 7: Run and commit**

Run: `pnpm --filter @matou/desktop test -- SessionCanvas SessionCarousel HierarchyShell SceneTabBar TerminalPane`
Expected: PASS.

```bash
git add apps/desktop/src/renderer/src/session-canvas apps/desktop/src/renderer/src/hierarchy
git commit -m "feat: render horizontal session canvases"
```

### Task 9: 子会话徽章、历史列表与通知跳转

**Files:**
- Create: `apps/desktop/src/renderer/src/session-canvas/ChildSessionBadge.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/ChildSessionBadge.test.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionHeader.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionCanvas.tsx`
- Modify: `apps/desktop/src/renderer/src/notifications/NotificationCenter.tsx`
- Modify: `apps/desktop/src/renderer/src/notifications/notification-ui-integration.test.tsx`

**Interfaces:**
- Produces: status aggregation and active/history child filters.
- Consumes: PRD 01 navigation callback and graph projection counts.

- [ ] **Step 1: Write failing badge tests**

Cover active count, `Claude N · Shell M`, `+H 历史`, status priority, hover breakdown, active-list default and history toggle. Verify Shell mode with retained children still shows the badge.

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- ChildSessionBadge notification-ui-integration`
Expected: FAIL because the badge and graph-aware navigation are absent.

- [ ] **Step 3: Implement badge and level navigation**

Clicking active count enters direct children. History toggle adds archived cards as summaries without starting PTYs. Reopening history appends a new live Session according to Shell/Claude resume rules.

- [ ] **Step 4: Wire notifications**

Recovery failure, needs-input and error notifications use existing PRD 01 classes. Click activates Scene/level/session and calls `ensureSessionVisible`; recovery retries update one notification.

- [ ] **Step 5: Run and commit**

Run: `pnpm --filter @matou/desktop test -- ChildSessionBadge SessionCanvas notification-ui-integration`
Expected: PASS.

```bash
git add apps/desktop/src/renderer/src/session-canvas apps/desktop/src/renderer/src/notifications
git commit -m "feat: navigate child sessions and history"
```

### Task 10: 两段式父层返回手势

**Files:**
- Create: `apps/desktop/src/renderer/src/session-canvas/ParentPullController.ts`
- Create: `apps/desktop/src/renderer/src/session-canvas/ParentPullController.test.ts`
- Create: `apps/desktop/src/renderer/src/session-canvas/ParentProjection.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/ParentProjection.test.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionCarousel.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/session-canvas.css`

**Interfaces:**
- Produces: state machine from Spec section 11 and `onCommitParent(parentSessionId)`.

- [ ] **Step 1: Write failing gesture state tests**

Cover ordinary left/right scroll, one oversized gesture ending at left without navigation, second gesture below threshold with spring cancellation, second gesture above threshold with commit, no-parent resistance, vertical gesture, xterm selection and reduced-motion behavior.

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- ParentPullController ParentProjection`
Expected: FAIL because the controller is absent.

- [ ] **Step 3: Implement pointer/wheel normalization**

Normalize trackpad wheel and pointer drag into the same state machine. Record edge armed only after the first gesture ends at `scrollLeft=0`. Apply 22%/96–180px commit threshold.

- [ ] **Step 4: Implement projection and transition**

Render a lightweight parent card behind the child list, apply resistance transform, spring back or activate the parent's sibling group. After commit, center and focus the parent.

- [ ] **Step 5: Run and commit**

Run: `pnpm --filter @matou/desktop test -- ParentPullController ParentProjection SessionCarousel`
Expected: PASS.

```bash
git add apps/desktop/src/renderer/src/session-canvas
git commit -m "feat: add two-stage parent pull navigation"
```

### Task 11: Option+Tab 长短按与独立 DAG BrowserWindow

**Files:**
- Create: `apps/desktop/src/renderer/src/dag/useDagShortcut.ts`
- Create: `apps/desktop/src/renderer/src/dag/useDagShortcut.test.ts`
- Modify: `apps/desktop/src/main/index.ts`
- Create: `apps/desktop/src/main/dag-window-manager.ts`
- Create: `apps/desktop/src/main/dag-window-manager.test.ts`
- Modify: `apps/desktop/src/preload/index.ts`
- Modify: `apps/desktop/src/renderer/src/main.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`

**Interfaces:**
- Produces: `openDagWindow`, `selectDagNode`, `closeDagWindow` preload methods.
- Produces: long-press threshold 450ms and short Tab forwarding.

- [ ] **Step 1: Write failing shortcut timer tests**

Using fake time only at unit level, assert release at 449ms forwards one Tab, 450ms opens once and consumes Tab, repeat events are ignored, setting clamps 350–800ms, and blur/cancel clears pending state.

- [ ] **Step 2: Write failing window manager tests**

Assert one DAG window per main window, current-display centering, direct Runtime port handoff, re-open focus/update, node selection routing, detached target activation and close without PTY termination.

- [ ] **Step 3: Verify RED**

Run: `pnpm --filter @matou/desktop test -- useDagShortcut dag-window-manager`
Expected: FAIL because modules are absent.

- [ ] **Step 4: Implement shortcut and narrow preload API**

Listen only while Matou is foreground. Short press calls the active TerminalSurface Tab writer; long press invokes Main with window/Scene/session identity. Add the Scene Tab graph button to the same open command.

- [ ] **Step 5: Implement DAG window lifecycle**

Create hidden-titlebar BrowserWindow with `kind=dag` query, pass a fresh Runtime MessagePort, update context when reused, and route selections to the owning main/detached window.

- [ ] **Step 6: Run and commit**

Run: `pnpm --filter @matou/desktop test -- useDagShortcut dag-window-manager runtime-host window-manager`
Expected: PASS.

```bash
git add apps/desktop/src/main apps/desktop/src/preload apps/desktop/src/renderer/src/dag apps/desktop/src/renderer/src/main.tsx apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx
git commit -m "feat: open session DAG in a native window"
```

### Task 12: DAG 分层画布、缩放、虚影、实时摘要与搜索

**Files:**
- Create: `apps/desktop/src/renderer/src/dag/DagWindowApp.tsx`
- Create: `apps/desktop/src/renderer/src/dag/DagWindowApp.test.tsx`
- Create: `apps/desktop/src/renderer/src/dag/DagCanvas.tsx`
- Create: `apps/desktop/src/renderer/src/dag/DagCanvas.test.tsx`
- Create: `apps/desktop/src/renderer/src/dag/dag-layout.ts`
- Create: `apps/desktop/src/renderer/src/dag/dag-layout.test.ts`
- Create: `apps/desktop/src/renderer/src/dag/DagSearch.tsx`
- Create: `apps/desktop/src/renderer/src/dag/DagSearch.test.tsx`
- Create: `apps/desktop/src/renderer/src/dag/dag.css`
- Modify: `apps/desktop/src/renderer/src/main.tsx`

**Interfaces:**
- Produces: `layoutGraph`, `visibleLayers`, `searchGraph`, pan/zoom state and node navigation.
- Consumes: `SceneSessionGraph`, Main selection API and geometry persistence.

- [ ] **Step 1: Write failing layout tests**

Build branched graphs including mixed Shell/Claude, archived nodes and 100 nodes. Assert stable depth columns, stable sibling layout sequence, three full layers around focus, farther ghost groups and directed edge endpoints.

- [ ] **Step 2: Write failing pan/zoom/search tests**

Assert pointer-centered zoom clamps 0.35–1.8, smooth pan updates world transform, `⌘+/-/0`, geometry restore, search ranking and Enter selection/centering.

- [ ] **Step 3: Verify RED**

Run: `pnpm --filter @matou/desktop test -- dag-layout DagCanvas DagSearch DagWindowApp`
Expected: FAIL because DAG renderer modules are absent.

- [ ] **Step 4: Implement stable layout and virtualization**

Use structural depth on X and membership creation sequence on Y. Render visible real cards as DOM, edges as SVG and distant layers as aggregate ghosts. Replace ghosts as viewport approaches.

- [ ] **Step 5: Implement node cards and live summaries**

Show title, mode, work status, branch/cwd, latest four lines and activity time. Merge summary projection updates every 250ms and preserve current pan/selection.

- [ ] **Step 6: Implement search and navigation**

Search title/cwd/branch/latest summary; center selection before Enter. On node choose, call Main, close DAG, and let main SessionCanvas ensure visibility/focus.

- [ ] **Step 7: Run and commit**

Run: `pnpm --filter @matou/desktop test -- dag-layout DagCanvas DagSearch DagWindowApp`
Expected: PASS including the 100-node fixture and reduced-motion assertions.

```bash
git add apps/desktop/src/renderer/src/dag apps/desktop/src/renderer/src/main.tsx
git commit -m "feat: render searchable session DAG canvas"
```

### Task 13: 几何、独立终端窗口、生命周期与主题回归

**Files:**
- Modify: `apps/runtime/src/scenes/geometry-repository.ts`
- Create: `apps/runtime/src/scenes/geometry-repository.test.ts`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionCanvas.tsx`
- Modify: `apps/desktop/src/renderer/src/dag/DagCanvas.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/DetachedTerminalApp.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/DetachedTerminalApp.test.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/session-canvas.css`
- Modify: `apps/desktop/src/renderer/src/dag/dag.css`
- Modify: `apps/desktop/src/renderer/src/terminal/terminal-themes.ts`

**Interfaces:**
- Produces: geometry owner keys from Spec section 13 and debounce/flush behavior.
- Preserves: PRD 05 detached Session identity and return flow.

- [ ] **Step 1: Write failing geometry tests**

Assert per-level scroll/focus, DAG pan/zoom and stable node position are isolated by Scene, survive restart, debounce high-frequency frames and flush once on quit.

- [ ] **Step 2: Write detached relation tests**

Detach a child Session, assert original graph node remains with window id and placeholder, DAG selection raises detached window, and closing it returns the same Session/relationship to its original sibling group.

- [ ] **Step 3: Verify RED**

Run: `pnpm --filter @matou/runtime test -- geometry-repository && pnpm --filter @matou/desktop test -- DetachedTerminalApp SessionCanvas DagCanvas`
Expected: FAIL on the new owner keys and graph-aware detached behavior.

- [ ] **Step 4: Implement persistence and lifecycle normalization**

Debounce 180ms, flush on full app quit, preserve state on hide, normalize detached windows to attached on restart, and mark unfinished Shell commands interrupted without rerun.

- [ ] **Step 5: Implement theme/accessibility polish**

Use existing light/dark tokens for cards, active level, edges, historical ghosts and recovery errors. Add aria labels, keyboard focus rings, Escape handling and reduced-motion variants.

- [ ] **Step 6: Run and commit**

Run: `pnpm --filter @matou/runtime test -- geometry-repository && pnpm --filter @matou/desktop test -- DetachedTerminalApp SessionCanvas DagCanvas terminal-themes`
Expected: PASS.

```bash
git add apps/runtime/src/scenes apps/desktop/src/renderer
git commit -m "feat: persist session canvas navigation state"
```

### Task 14: 自动化 E2E 覆盖与真实隔离 App

**Files:**
- Create: `tests/e2e/session-canvas-basics.spec.ts`
- Create: `tests/e2e/session-canvas-fork-worktree.spec.ts`
- Create: `tests/e2e/session-canvas-navigation.spec.ts`
- Create: `tests/e2e/session-dag-window.spec.ts`
- Create: `tests/e2e/session-canvas-recovery.spec.ts`
- Create: `tests/e2e/session-canvas-lifecycle.spec.ts`
- Create: `tests/e2e/fixtures/session-canvas-fixture.ts`
- Modify: `playwright.config.ts`
- Modify: `package.json`

**Interfaces:**
- Produces: isolated Electron launch fixture with real `MATOU_DATA_DIR`, `ELECTRON_USER_DATA_DIR`, zsh, Git and worktree setup.
- Consumes: built desktop/runtime and real provider CLI availability for provider-labeled cases.

- [ ] **Step 1: Implement isolated launch fixture tests first**

Assert each run receives a unique `/tmp/matou-e2e-<run-id>`, user data paths are absent from process args/environment, test Workspace creation uses temporary directories, and cleanup keeps evidence until result recording completes.

- [ ] **Step 2: Add real basic/canvas/navigation journeys**

Automate new canvas default Shell, Shell siblings, input ordering, maximum four visible, bidirectional scroll, hover expansion, second right-pull, parent return, focus and restart persistence using real Electron input events and real PTYs.

- [ ] **Step 3: Add real Fork/worktree/recovery journeys**

Use temporary Git repositories and actual `git worktree list` evidence. Drive real Claude Code CLI when available in the configured acceptance environment; collect provider session identity, process output and worktree paths rather than injecting graph state.

- [ ] **Step 4: Add real DAG window journeys**

Use actual long/short key sequences, inspect the native DAG BrowserWindow, pan/zoom/search/select nodes, confirm target visibility in main or detached windows, and exercise 100 real persisted nodes created through public UI/RPC journeys.

- [ ] **Step 5: Run full automated gates**

Run:

```bash
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
pnpm test:packaged
```

Expected: every existing PRD 01–06 test and every new session-canvas suite passes.

- [ ] **Step 6: Commit**

```bash
git add tests/e2e playwright.config.ts package.json
git commit -m "test: cover session canvas and DAG journeys"
```

### Task 15: 独立 QA 逐条执行 102 条用例与缺陷闭环

**Files:**
- Modify: `docs/test/E2E-Matou-会话画布与DAG分支交互.md`
- Modify: `docs/test/TRACE-Matou-会话画布与DAG分支交互.md`
- Create: `docs/test/evidence/<run-id>/manifest.md`

**Interfaces:**
- Consumes: completed App, immutable test steps, isolated directories and real system operations.
- Produces: 102/102 case results, evidence paths, defect links and regression history.

- [ ] **Step 1: Build and launch isolated acceptance App**

Use a fresh run id and record App commit, macOS/Electron/Node/Git/Claude versions, environment paths, display setup and start time in evidence manifest.

- [ ] **Step 2: Assign the independent senior QA Agent**

Provide only the PRD, spec, test cases, built App path and isolated environment. The QA Agent executes the App as a user and records each case result without reading or changing implementation code.

- [ ] **Step 3: Execute P0 cases in document order**

For every case, record `通过/失败`, timestamp, screenshot/video/log/SQLite-readonly evidence and actual user-visible result. Stop a journey only when continuing would contaminate later evidence; start a new isolated fixture when required by the case.

- [ ] **Step 4: Main Agent fixes each product defect**

For each failed case, reproduce in a separate isolated directory, add the narrowest failing automated regression, implement the product behavior, run related tests and hand the same case plus dependency regression set back to QA.

- [ ] **Step 5: QA Agent executes P1 and full failed-case regression**

Keep original steps intact. A case text edit requires evidence that it conflicts with the original PRD and a recorded product decision; otherwise code and product behavior move to satisfy the case.

- [ ] **Step 6: Completion audit**

Verify 102 unique case IDs all have final `通过`, trace matrix remains 19/19 F, 50/50 AS, 74/74 UB, 15/15 EX and 9/9 QG, evidence paths exist, user data paths are absent, all automated gates pass on the final commit, and no open product defect remains.

- [ ] **Step 7: Commit final evidence**

```bash
git add docs/test
git commit -m "test: record session canvas acceptance"
```
