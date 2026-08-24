# PRD 05 Four-Level Hierarchy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the complete Workspace -> Task -> Scene -> Session hierarchy and stop at a packaged, product-verifiable PRD 05 acceptance gate.

**Architecture:** Runtime owns all hierarchy records and workflow decisions in SQLite. Renderer receives a per-window projection over the existing direct MessagePort and owns only transient view state. Electron Main owns native windows, tray, and directory selection while terminal bytes continue to bypass Main.

**Tech Stack:** TypeScript 7, Electron 43, React 19, xterm.js 6, node-pty, `node:sqlite`, Zod, Vitest, Playwright.

**Spec:** `docs/superpowers/specs/2026-08-24-prd-05-four-level-hierarchy-design.md`

## Global Constraints

- UI terminology is 工作区 -> 事项 -> 页签 -> 终端.
- Runtime is the sole SQLite writer and authoritative hierarchy source.
- Electron Main never proxies terminal bytes and never opens SQLite.
- Renderer never exports an authoritative hierarchy snapshot.
- Structural mutations use transaction plus Outbox; geometry remains outside Outbox.
- Every production behavior begins with a failing test and a verified RED result.
- PRD 03 starts only after explicit user acceptance of PRD 05.
- Destructive copy and step counts match Spec section 5.2 exactly.
- Invalid Workspace paths never fall back to another cwd.
- Existing live PTYs survive hierarchy switching, window hiding, Renderer reload, and terminal detachment.

---

### Task 1: Domain types and schema migration

**Files:**
- Modify: `packages/domain/src/model.ts`
- Modify: `packages/domain/src/events.ts`
- Modify: `apps/runtime/src/storage/migrations.ts`
- Test: `apps/runtime/src/storage/migration-runner.test.ts`
- Test: `packages/domain/src/model.test.ts`

**Interfaces:**
- Produces: `WorkspacePathState`, `WindowNavigation`, `TaskPlacement`, Scene `titlePinned`, `sortKey`, and `layoutRevision`.
- Produces: schema tables and indexes from Spec section 6.

- [ ] **Step 1: Write failing domain and migration tests**

```ts
it('adds PRD 05 hierarchy state without changing existing rows', () => {
  const database = migrateFoundationFixture()
  expect(database.all('PRAGMA table_info(scenes)')).toEqual(expect.arrayContaining([
    expect.objectContaining({ name: 'title_pinned' }),
    expect.objectContaining({ name: 'sort_key' }),
    expect.objectContaining({ name: 'layout_revision' })
  ]))
  expect(tableNames(database)).toEqual(expect.arrayContaining([
    'workspace_path_state', 'app_windows', 'window_navigation',
    'window_workspace_focus', 'window_task_focus', 'window_scene_focus',
    'window_task_placements', 'bootstrap_state'
  ]))
})
```

- [ ] **Step 2: Run tests and verify RED**

Run: `pnpm --filter @matou/runtime test -- migration-runner && pnpm --filter @matou/domain test`  
Expected: FAIL because the new columns, tables, and exported types are absent.

- [ ] **Step 3: Add the migration and types**

```ts
export interface WorkspacePathState {
  workspaceId: WorkspaceId
  status: 'valid' | 'invalid'
  reason: '' | 'missing' | 'not-directory' | 'no-access' | 'unknown'
  checkedAt: number
  validationGeneration: number
}

export interface WindowNavigation {
  windowId: string
  activeWorkspaceId?: WorkspaceId
  taskByWorkspace: Record<WorkspaceId, TaskId>
  sceneByTask: Record<TaskId, SceneId>
  sessionByScene: Record<SceneId, SessionId>
}
```

- [ ] **Step 4: Run targeted tests and typecheck**

Run: `pnpm --filter @matou/domain typecheck && pnpm --filter @matou/runtime test -- migration-runner`  
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/domain apps/runtime/src/storage
git commit -m "feat: add PRD 05 hierarchy schema"
```

### Task 2: Atomic hierarchy bootstrap and Workspace lifecycle

**Files:**
- Create: `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- Create: `apps/runtime/src/hierarchy/hierarchy-application-service.test.ts`
- Create: `apps/runtime/src/hierarchy/hierarchy-ids.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `packages/contracts/src/protocol.test.ts`

**Interfaces:**
- Produces: `bootstrapWindow`, `createWorkspace`, `renameWorkspace`, `removeWorkspace`, `activateWorkspace`.
- Consumes: `RuntimeDatabase`, `DomainTransactionManager`, existing Workspace/Task/Session/Scene tables.

- [ ] **Step 1: Write a failing atomic-bootstrap test**

```ts
it('creates one complete default hierarchy in one command', async () => {
  const result = await service.bootstrapWindow(command('bootstrap-1'), {
    windowId: 'window-1', defaultRootDirectory: root, defaultName: 'matou_workspace', now: 10
  })
  expect(result.navigation.activeWorkspaceId).toBe(result.workspace.id)
  expect(result.task.title).toBe('默认')
  expect(result.scene.taskId).toBe(result.task.id)
  expect(result.session.executionContextId).toBe(result.executionContext.id)
  expect(result.mount.sessionId).toBe(result.session.id)
  expect(eventTypes(database)).toEqual([
    'workspace.created', 'task.created', 'scene.created', 'session.created', 'scene.session-mounted'
  ])
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- hierarchy-application-service`  
Expected: FAIL because the service module does not exist.

- [ ] **Step 3: Implement atomic bootstrap and idempotency**

```ts
export class HierarchyApplicationService {
  bootstrapWindow(command: DomainCommandMetadata, input: BootstrapWindowInput): Promise<BootstrapWindowResult>
  createWorkspace(command: DomainCommandMetadata, input: CreateWorkspaceInput): Promise<WorkspaceHierarchyResult>
  renameWorkspace(command: DomainCommandMetadata, input: RenameWorkspaceInput): Promise<Workspace>
  removeWorkspace(command: DomainCommandMetadata, input: RemoveWorkspaceInput): Promise<HierarchyMutationResult>
  activateWorkspace(input: ActivateWorkspaceInput): Promise<HierarchyProjection>
}
```

Create the default Workspace only when there are no active Workspace rows and
`bootstrap_state.default-workspace-removed` is not true. Reuse an existing Workspace
when its normalized path matches a create request.

- [ ] **Step 4: Add failing Workspace removal tests**

```ts
it('records explicit default removal and preserves the disk directory', async () => {
  await service.removeWorkspace(command('remove-1'), {
    windowId: 'window-1', workspaceId, confirmedIntent: intent, now: 20
  })
  expect(readBootstrapFlag(database, 'default-workspace-removed')).toBe(true)
  expect(existsSync(root)).toBe(true)
  expect(await service.bootstrapWindow(command('bootstrap-2'), bootstrapInput)).toMatchObject({ workspace: null })
})
```

- [ ] **Step 5: Implement lifecycle RPC methods and protocol allowlist**

Add the `hierarchy.*` methods from Spec section 8.1 to `RPC_METHODS` and dispatch
Workspace methods to the application service.

- [ ] **Step 6: Run targeted tests**

Run: `pnpm --filter @matou/contracts test && pnpm --filter @matou/runtime test -- hierarchy-application-service runtime-rpc-router`  
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/contracts apps/runtime/src/hierarchy apps/runtime/src/rpc
git commit -m "feat: add atomic workspace hierarchy workflows"
```

### Task 3: Workspace path validation and execution guard

**Files:**
- Create: `apps/runtime/src/hierarchy/workspace-path-service.ts`
- Create: `apps/runtime/src/hierarchy/workspace-path-service.test.ts`
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Test: `apps/runtime/src/runtime-server.test.ts`

**Interfaces:**
- Produces: `validate(workspaceId)`, `validateBeforeExecution(workspaceId)`, `startPolling()`, `stopPolling()`.
- Produces error code: `WORKSPACE_PATH_INVALID` with the fixed Chinese product message.

- [ ] **Step 1: Write failing filesystem-state tests**

```ts
it.each([
  ['missing', missingPath],
  ['not-directory', filePath],
  ['no-access', unreadableDirectory]
])('derives %s without changing Workspace ownership', async (reason, rootDirectory) => {
  const state = await service.validateWorkspace(workspaceId, rootDirectory)
  expect(state).toMatchObject({ status: 'invalid', reason })
  expect(repository.getWorkspace(workspaceId)?.rootDirectory).toBe(rootDirectory)
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- workspace-path-service`  
Expected: FAIL because the validator is absent.

- [ ] **Step 3: Implement generation-fenced validation and 30-second polling**

```ts
export const WORKSPACE_PATH_INVALID_MESSAGE =
  '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'
```

Persist only the newest validation generation and emit
`workspace.path-status-changed` when status or reason changes.

- [ ] **Step 4: Write failing Runtime input-guard test**

```ts
it('rejects input for an invalid Workspace while keeping the PTY alive', async () => {
  await markWorkspaceInvalid(database, workspaceId)
  await server.receive(terminalInput(sessionId, 'pwd\r'))
  expect(port.lastError()).toMatchObject({ code: 'WORKSPACE_PATH_INVALID' })
  expect(registry.get(sessionId)?.pid).toBe(pid)
})
```

- [ ] **Step 5: Implement Runtime enforcement for spawn, input, and hierarchy creation**

Resolve Session -> Task -> Workspace on every execution-producing request. Do not
accept a Renderer-provided fallback path.

- [ ] **Step 6: Run tests**

Run: `pnpm --filter @matou/runtime test -- workspace-path-service runtime-server hierarchy-application-service`  
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/runtime/src/hierarchy apps/runtime/src/runtime-server.ts apps/runtime/src/runtime-server.test.ts
git commit -m "feat: enforce workspace path state"
```

### Task 4: Task lifecycle, ordering, naming, and per-window navigation

**Files:**
- Create: `apps/runtime/src/hierarchy/navigation-repository.ts`
- Create: `apps/runtime/src/hierarchy/navigation-repository.test.ts`
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.test.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`

**Interfaces:**
- Produces: create/rename/reorder/delete/activate Task commands.
- Produces: canonical `WindowNavigation` and `TaskPlacement` projection.

- [ ] **Step 1: Write failing naming and reorder tests**

```ts
it('chooses the lowest available user Task name and preserves order', async () => {
  await createNamedTasks(['新事项', '新事项 3'])
  const created = await service.createTask(command('task-new'), { windowId, workspaceId, now: 30 })
  expect(created.task.title).toBe('新事项 2')
  await service.reorderTask(command('task-order'), { windowId, workspaceId, taskId: created.task.id, beforeTaskId, now: 31 })
  expect(project(windowId).tasks.map(task => task.id)).toEqual(expectedOrder)
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- hierarchy-application-service navigation-repository`  
Expected: FAIL on missing Task workflow methods.

- [ ] **Step 3: Implement Task workflows and window-scoped focus**

```ts
createTask(command: DomainCommandMetadata, input: CreateTaskWorkflowInput): Promise<TaskHierarchyResult>
renameTask(command: DomainCommandMetadata, input: RenameTaskWorkflowInput): Promise<Task>
reorderTask(command: DomainCommandMetadata, input: ReorderTaskWorkflowInput): Promise<TaskOrderResult>
deleteTask(command: DomainCommandMetadata, input: DeleteTaskWorkflowInput): Promise<HierarchyMutationResult>
activateTask(input: ActivateTaskInput): Promise<HierarchyProjection>
```

New Task creation atomically creates its plain ExecutionContext, one Scene, one
Shell Session, one SessionMount, and its window placement.

- [ ] **Step 4: Add failing cascade/default-rebuild tests**

```ts
it('deletes a confirmed final Task and atomically replaces it with 默认', async () => {
  const result = await service.deleteTask(command('delete-final'), {
    windowId, taskId, confirmedIntent: currentIntent, now: 40
  })
  expect(result.disposedSessionIds).toContain(sessionId)
  expect(result.projection.tasks).toEqual([expect.objectContaining({ title: '默认' })])
})
```

- [ ] **Step 5: Run tests and commit**

Run: `pnpm --filter @matou/runtime test -- hierarchy-application-service navigation-repository`  
Expected: PASS.

```bash
git add apps/runtime/src/hierarchy apps/runtime/src/rpc
git commit -m "feat: add Task hierarchy workflows"
```

### Task 5: Scene lifecycle and atomic split-tree replacement

**Files:**
- Create: `packages/domain/src/layout.ts`
- Create: `packages/domain/src/layout.test.ts`
- Create: `apps/runtime/src/hierarchy/scene-layout-service.ts`
- Create: `apps/runtime/src/hierarchy/scene-layout-service.test.ts`
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- Modify: `apps/runtime/src/scenes/scene-repository.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`

**Interfaces:**
- Produces: `normalizeLayout`, `splitMount`, `removeMount`, `replaceLayout`.
- Produces: create/rename/reorder/close/activate Scene workflows.

- [ ] **Step 1: Write failing pure layout tests**

```ts
it('splits the active mount to the right and collapses one-child splits', () => {
  const split = splitMount(mount('a'), 'a', mount('b'), 'horizontal')
  expect(split).toEqual({
    id: expect.any(String), kind: 'split', direction: 'horizontal',
    children: [mount('a'), mount('b')]
  })
  expect(removeMount(split, 'b')).toEqual(mount('a'))
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/domain test -- layout`  
Expected: FAIL because layout helpers are absent.

- [ ] **Step 3: Implement immutable tree normalization**

Reject duplicate mount IDs, missing mounted Sessions, cross-Scene nodes, and empty
split children.

- [ ] **Step 4: Write failing compare-and-swap repository tests**

```ts
it('replaces the complete Scene tree at one revision', async () => {
  const result = await layouts.replaceLayout(command('layout-2'), {
    sceneId, expectedRevision: 1, root, now: 50
  })
  expect(result.layoutRevision).toBe(2)
  await expect(layouts.replaceLayout(command('layout-stale'), {
    sceneId, expectedRevision: 1, root, now: 51
  })).rejects.toThrow(/revision/i)
})
```

- [ ] **Step 5: Implement Scene workflows and exact close semantics**

Pinned titles are unique within Task. Closing the last Scene of the last Task returns
`{ action: 'hide-window' }` without archiving records. Other closes archive the
correct Sessions and repair navigation deterministically.

- [ ] **Step 6: Run tests and commit**

Run: `pnpm --filter @matou/domain test -- layout && pnpm --filter @matou/runtime test -- scene-layout-service hierarchy-application-service`  
Expected: PASS.

```bash
git add packages/domain apps/runtime/src/hierarchy apps/runtime/src/scenes apps/runtime/src/rpc
git commit -m "feat: add Scene and split layout workflows"
```

### Task 6: Session deletion, PTY cleanup, and hierarchy projection

**Files:**
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/desktop/src/renderer/src/projection/RuntimeProjectionStore.ts`
- Modify: `apps/desktop/src/renderer/src/projection/RuntimeProjectionStore.test.ts`

**Interfaces:**
- Produces: split/delete/activate Session workflows and `HierarchyProjection` snapshots.
- Consumes: `disposedSessionIds` after committed lifecycle commands.

- [ ] **Step 1: Write failing Session deletion matrix tests**

```ts
it.each([
  ['sibling mount', false, 'scene-remains'],
  ['last in Scene', false, 'scene-archives'],
  ['last in Workspace', true, 'default-task-created']
])('%s applies the documented cascade', async (_name, needsIntent, expected) => {
  const result = await service.deleteSession(command(nextId()), deletionInput(needsIntent))
  expect(result.outcome).toBe(expected)
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- hierarchy-application-service runtime-server`  
Expected: FAIL on missing Session workflow behavior.

- [ ] **Step 3: Implement post-commit PTY disposal and restart cleanup**

RuntimeServer disposes only IDs returned by a committed command. Startup recovery
disposes registry Sessions already archived in SQLite.

- [ ] **Step 4: Extend projection snapshot and event application**

```ts
export interface RuntimeProjectionSnapshot {
  runtimeGeneration: string
  eventSequence: number
  hierarchy: HierarchyProjection
  // existing product projections remain additive
}
```

Add tests proving archived entities disappear from active lists and navigation stays
valid after ordered events.

- [ ] **Step 5: Run tests and commit**

Run: `pnpm --filter @matou/runtime test -- runtime-server hierarchy-application-service runtime-rpc-router && pnpm --filter @matou/desktop test -- RuntimeProjectionStore`  
Expected: PASS.

```bash
git add apps/runtime apps/desktop/src/renderer/src/projection
git commit -m "feat: connect hierarchy and terminal lifecycle"
```

### Task 7: Central Renderer RuntimeClient

**Files:**
- Create: `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`
- Create: `apps/desktop/src/renderer/src/runtime/RuntimeClient.test.ts`
- Create: `apps/desktop/src/renderer/src/runtime/RuntimeProvider.tsx`
- Create: `apps/desktop/src/renderer/src/runtime/useRuntimeProjection.ts`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`

**Interfaces:**
- Produces: `request`, `subscribeProjection`, `attachTerminal`, `detachTerminalView`, `sendTerminalInput`, `resizeTerminal`.
- Consumes: the one transferred MessagePort from Preload.

- [ ] **Step 1: Write failing client correlation/reconnect tests**

```ts
it('correlates RPC, orders events, and reattaches terminal consumers after a new port', async () => {
  const client = new RuntimeClient(fakePortFactory())
  const projection = await client.request('hierarchy.bootstrap-window', bootstrapPayload)
  expect(projection.windowId).toBe('window-1')
  await client.replacePort(secondPort)
  expect(secondPort.sentTypes()).toContain('terminal.spawn')
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- RuntimeClient`  
Expected: FAIL because RuntimeClient is absent.

- [ ] **Step 3: Implement one-port fan-out and RPC timeout/cancel**

TerminalSurface receives a Session ID and controller from context. Its cleanup
removes only the view subscription. It sends `terminal.dispose` only when the
hierarchy command has explicitly deleted the Session.

- [ ] **Step 4: Preserve the existing direct-channel E2E marker through the client**

Update the test-only smoke path without creating a special production Session.

- [ ] **Step 5: Run tests and commit**

Run: `pnpm --filter @matou/desktop test && pnpm typecheck`  
Expected: PASS.

```bash
git add apps/desktop/src/renderer/src/runtime apps/desktop/src/renderer/src/terminal
git commit -m "refactor: centralize Renderer Runtime channel"
```

### Task 8: Native directory picker, stable windows, and tray protection

**Files:**
- Create: `apps/desktop/src/shared/desktop-api.ts`
- Create: `apps/desktop/src/main/window-manager.ts`
- Create: `apps/desktop/src/main/window-manager.test.ts`
- Modify: `apps/desktop/src/main/index.ts`
- Modify: `apps/desktop/src/preload/index.ts`
- Modify: `apps/desktop/src/renderer/src/env.d.ts`

**Interfaces:**
- Produces the typed `matouDesktop` API from Spec section 11.
- Produces stable `windowId` query/bootstrap data.

- [ ] **Step 1: Write failing native lifecycle tests with Electron fakes**

```ts
it('hides only the protected main window and restores it from the tray', async () => {
  const manager = createManagerWithTwoWindows()
  manager.hideWindow('window-1')
  expect(manager.window('window-1').isVisible()).toBe(false)
  expect(manager.window('window-2').isVisible()).toBe(true)
  manager.showWindow('window-1')
  expect(manager.window('window-1').isVisible()).toBe(true)
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- window-manager`  
Expected: FAIL because the manager is absent.

- [ ] **Step 3: Implement contextBridge API, tray, and directory picker**

Use `dialog.showOpenDialog({ properties: ['openDirectory', 'createDirectory'] })`.
Tray `退出` sets an explicit quit flag before closing windows and stopping Runtime.

- [ ] **Step 4: Run tests and commit**

Run: `pnpm --filter @matou/desktop test -- window-manager runtime-host && pnpm --filter @matou/desktop typecheck`  
Expected: PASS.

```bash
git add apps/desktop/src/main apps/desktop/src/preload apps/desktop/src/shared apps/desktop/src/renderer/src/env.d.ts
git commit -m "feat: add native window and tray lifecycle"
```

### Task 9: Workspace switcher and Task sidebar

**Files:**
- Create: `apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts`
- Create: `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`
- Create: `apps/desktop/src/renderer/src/hierarchy/WorkspaceSwitcher.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/TaskSidebar.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/RenameDialog.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/ConfirmDialog.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/EmptyWorkspaceState.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/hierarchy-components.test.tsx`
- Modify: `apps/desktop/package.json`

**Interfaces:**
- Consumes: `HierarchyProjection`, `matouDesktop.selectWorkspaceDirectory`, Runtime hierarchy commands.
- Produces: Workspace and Task navigation UI.

- [ ] **Step 1: Add React DOM test support and write failing product tests**

```tsx
it('shows invalid Workspace state and preserves the path tail', async () => {
  render(<WorkspaceSwitcher projection={fixture} commands={commands} />)
  expect(screen.getByText('路径失效')).toBeVisible()
  expect(screen.getByTitle('/Users/demo/projects/frontend/app')).toHaveTextContent('frontend/app')
})

it('disables duplicate Task rename while displaying the product error', async () => {
  render(<TaskSidebar projection={fixture} commands={commands} />)
  await openRenameAndType('线上 bug')
  expect(screen.getByText('当前工作区下已存在名为“线上 bug”的事项')).toBeVisible()
  expect(screen.getByRole('button', { name: '确认' })).toBeDisabled()
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- hierarchy-components`  
Expected: FAIL because components and DOM test setup are absent.

- [ ] **Step 3: Implement Workspace and Task flows**

Include create/switch/rename/remove, `+ 新事项`, lowest-name behavior, active-item
visibility, four drag visual states, same-Workspace reorder, and cross-Workspace
rejection.

- [ ] **Step 4: Run tests and commit**

Run: `pnpm --filter @matou/desktop test -- hierarchy-components RuntimeClient`  
Expected: PASS.

```bash
git add apps/desktop/package.json pnpm-lock.yaml apps/desktop/src/renderer/src/hierarchy
git commit -m "feat: add Workspace and Task navigation"
```

### Task 10: Scene tab bar, overflow, and split UI

**Files:**
- Create: `apps/desktop/src/renderer/src/hierarchy/SceneTabBar.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/SceneOverflowMenu.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/SplitTree.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/SplitDivider.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/split-ui.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy.css`

**Interfaces:**
- Consumes: Scene snapshots and hierarchy commands.
- Produces: Scene activation, rename, close, keyboard reorder, split, focus, and debounced geometry writes.

- [ ] **Step 1: Write failing Scene/split interaction tests**

```tsx
it('creates a right-hand child for horizontal split and preserves another Scene', async () => {
  render(<HierarchyFixture activeScene="scene-a" />)
  await user.click(screen.getByRole('button', { name: '水平分屏' }))
  expect(layoutOf('scene-a')).toMatchObject({ direction: 'horizontal' })
  expect(layoutOf('scene-b')).toEqual(singleMountLayout('session-b'))
})

it('opens overflow and centers the selected Scene', async () => {
  render(<SceneTabBar {...overflowFixture} />)
  await user.click(screen.getByRole('button', { name: '更多页签' }))
  await user.click(screen.getByRole('menuitem', { name: '页签 20' }))
  expect(scrollIntoView).toHaveBeenCalledWith(expect.objectContaining({ inline: 'center' }))
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- split-ui`  
Expected: FAIL because Scene UI is absent.

- [ ] **Step 3: Implement nested split rendering and 100 ms geometry debounce**

Use Pointer Events for dividers. Enforce the 160 x 100 CSS pixel minimum. Structural
commands include expected layout revision.

- [ ] **Step 4: Run tests and commit**

Run: `pnpm --filter @matou/desktop test -- split-ui hierarchy-components`  
Expected: PASS.

```bash
git add apps/desktop/src/renderer/src/hierarchy
git commit -m "feat: add Scene tabs and split layouts"
```

### Task 11: Multi-terminal shell and exact close interactions

**Files:**
- Create: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/terminal-close-flow.ts`
- Create: `apps/desktop/src/renderer/src/hierarchy/terminal-close-flow.test.ts`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/ConfirmDialog.tsx`

**Interfaces:**
- Produces: three independent confirmation flows and mixed Session rendering.
- Consumes: RuntimeClient terminal controllers by Session ID.

- [ ] **Step 1: Write failing close-flow state-machine tests**

```ts
it('requires two Task confirmations but one final-Session confirmation', () => {
  expect(taskDeleteFlow({ sessionCount: 1 }).steps.map(step => step.title)).toEqual([
    '删除事项', '删除事项'
  ])
  expect(sessionDeleteFlow({ isWorkspaceFinal: true }).steps).toHaveLength(1)
  expect(sessionDeleteFlow({ isWorkspaceFinal: true }).steps[0]?.title).toBe('删除终端')
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- terminal-close-flow`  
Expected: FAIL because flow helpers are absent.

- [ ] **Step 3: Implement exact copy and stale-intent handling**

Dialogs request a fresh intent description from Runtime before opening. Confirmation
sends the returned token. `CONFLICT` closes the stale dialog and refreshes the view.

- [ ] **Step 4: Render one xterm per mounted Session and preserve inactive live terminals**

Inactive Scenes keep terminal controllers attached; visibility changes pause fit work
without disposing the Session.

- [ ] **Step 5: Run tests and commit**

Run: `pnpm --filter @matou/desktop test && pnpm --filter @matou/desktop typecheck`  
Expected: PASS.

```bash
git add apps/desktop/src/renderer/src/hierarchy apps/desktop/src/renderer/src/terminal
git commit -m "feat: add multi-terminal hierarchy interactions"
```

### Task 12: Assemble the product shell and invalid-path experience

**Files:**
- Create: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.test.tsx`
- Modify: `apps/desktop/src/renderer/src/App.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/terminal.css`
- Modify: `apps/desktop/src/renderer/src/main.tsx`

**Interfaces:**
- Produces: complete PRD 05 main-window product surface.
- Consumes: RuntimeProvider and all hierarchy components.

- [ ] **Step 1: Write failing whole-shell navigation tests**

```tsx
it('keeps Workspace A context after switching through Workspace B', async () => {
  render(<HierarchyShell fixture={twoWorkspaceFixture} />)
  await activate('A', '事项 A2', '页签 A2-3', '终端 A2-3-2')
  await switchWorkspace('B')
  await switchWorkspace('A')
  expect(activeHierarchy()).toEqual(['A', '事项 A2', '页签 A2-3', '终端 A2-3-2'])
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/desktop test -- HierarchyShell`  
Expected: FAIL because the assembled shell is absent.

- [ ] **Step 3: Replace the foundation smoke shell with the hierarchy product shell**

Keep E2E-only diagnostics in visually hidden outputs. Invalid Workspace state displays
the dropdown badge and terminal-area message, with execution buttons disabled and the
fixed Runtime error surfaced on races.

- [ ] **Step 4: Run desktop tests and visual build**

Run: `pnpm --filter @matou/desktop test && pnpm --filter @matou/desktop build`  
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/renderer
git commit -m "feat: assemble PRD 05 hierarchy shell"
```

### Task 13: Detached terminal windows

**Files:**
- Modify: `apps/desktop/src/main/window-manager.ts`
- Modify: `apps/desktop/src/main/window-manager.test.ts`
- Create: `apps/desktop/src/renderer/src/hierarchy/DetachedTerminalApp.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/DetachedPlaceholder.tsx`
- Create: `apps/runtime/src/hierarchy/detached-session-service.ts`
- Create: `apps/runtime/src/hierarchy/detached-session-service.test.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`

**Interfaces:**
- Produces: detach and return commands, detached BrowserWindow, and restart normalization.
- Preserves: Session ID, SessionRun ID, PID, Journal, and ProviderBinding.

- [ ] **Step 1: Write failing detach/return service tests**

```ts
it('returns the same live Session to the original Scene or current fallback Scene', async () => {
  const detached = await service.detach(command('detach-1'), detachInput)
  expect(detached.sessionId).toBe(sessionId)
  const returned = await service.returnSession(command('return-1'), { detachedWindowId, now: 70 })
  expect(returned.sessionId).toBe(sessionId)
  expect(returned.targetSceneId).toBe(originalSceneId)
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- detached-session-service && pnpm --filter @matou/desktop test -- window-manager`  
Expected: FAIL because detach workflows are absent.

- [ ] **Step 3: Implement compensated detach and native window bootstrap**

Create the BrowserWindow only after Runtime records the detached SceneWindow. If native
creation fails, issue return compensation. Closing the native window returns before its
Renderer connection is released.

- [ ] **Step 4: Add an E2E assertion for stable PID and placeholder**

```ts
expect(await mainPage.getByText('已脱出').isVisible()).toBe(true)
expect(await detachedPid(detachedPage)).toBe(originalPid)
await detachedPage.close()
expect(await attachedPid(mainPage)).toBe(originalPid)
```

- [ ] **Step 5: Run tests and commit**

Run: `pnpm --filter @matou/runtime test -- detached-session-service && pnpm --filter @matou/desktop test -- window-manager`  
Expected: PASS.

```bash
git add apps/runtime/src/hierarchy apps/runtime/src/rpc apps/desktop/src/main apps/desktop/src/renderer
git commit -m "feat: add detachable terminal windows"
```

### Task 14: Whole-Task multi-window migration

**Files:**
- Create: `apps/runtime/src/hierarchy/task-window-migration-service.ts`
- Create: `apps/runtime/src/hierarchy/task-window-migration-service.test.ts`
- Modify: `apps/runtime/src/control/host-control-server.ts`
- Modify: `apps/runtime/src/control/host-control-server.test.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.ts`
- Modify: `apps/desktop/src/main/window-manager.ts`

**Interfaces:**
- Produces Host Control scope `task.move-to-window`.
- Produces prepare/ack/commit/rollback workflow for one complete Task.

- [ ] **Step 1: Write failing migration commit/rollback tests**

```ts
it('moves the complete Task placement without changing Workspace ownership', async () => {
  const pending = await service.prepare(command('move-1'), { taskId, sourceWindowId, targetWindowId, now: 80 })
  await service.acknowledgeTarget(command('move-ack'), { migrationId: pending.id, now: 81 })
  expect(readPlacement(taskId)).toEqual({ windowId: targetWindowId })
  expect(readTask(taskId)?.workspaceId).toBe(workspaceId)
})

it('restores source placement when target closes before acknowledgement', async () => {
  const pending = await service.prepare(command('move-2'), moveInput)
  await service.fail(command('move-fail'), { migrationId: pending.id, reason: 'target-closed', now: 82 })
  expect(readPlacement(taskId)).toEqual({ windowId: sourceWindowId })
})
```

- [ ] **Step 2: Verify RED**

Run: `pnpm --filter @matou/runtime test -- task-window-migration-service host-control-server`  
Expected: FAIL because the migration scope and service are absent.

- [ ] **Step 3: Implement bounded Host Control parsing and target acknowledgement**

Require a capability token containing `task.move-to-window`. Reject stale window IDs
and migration overlap with `CONFLICT`.

- [ ] **Step 4: Run tests and commit**

Run: `pnpm --filter @matou/runtime test -- task-window-migration-service host-control-server runtime-control-backend`  
Expected: PASS.

```bash
git add apps/runtime/src/hierarchy apps/runtime/src/control apps/desktop/src/main/window-manager.ts
git commit -m "feat: add whole-Task window migration"
```

### Task 15: PRD 05 Electron acceptance suite

**Files:**
- Create: `tests/e2e/prd-05-hierarchy.spec.ts`
- Create: `tests/e2e/prd-05-detached-window.spec.ts`
- Create: `tests/e2e/prd-05-path-recovery.spec.ts`
- Create: `tests/e2e/prd-05-multi-window.spec.ts`
- Modify: `playwright.config.ts`
- Modify: `package.json`

**Interfaces:**
- Produces automated evidence for Spec section 15.3.

- [ ] **Step 1: Write the first failing first-launch E2E scenario**

```ts
test('first launch presents a complete hierarchy in the Workspace directory', async () => {
  const app = await launchMatou({ cleanDataRoot: true })
  const page = await firstWindow(app)
  await expect(page.getByTestId('workspace-name')).toHaveText('matou_workspace')
  await expect(page.getByTestId('active-task')).toHaveText('默认')
  await expect(page.getByTestId('scene-tab')).toHaveCount(1)
  await expect(page.getByTestId('terminal-pane')).toHaveCount(1)
  await expect.poll(() => terminalCwd(page)).toBe(defaultRoot)
})
```

- [ ] **Step 2: Verify RED against the current app**

Run: `pnpm build && pnpm exec playwright test tests/e2e/prd-05-hierarchy.spec.ts`  
Expected: FAIL before the assembled feature exposes the hierarchy.

- [ ] **Step 3: Add all Spec section 15.3 scenarios**

Group scenarios by lifecycle, switching/layout, destructive actions, path state,
detachment, multi-window migration, and restart. Use isolated data roots and real
node-pty processes.

- [ ] **Step 4: Run the complete development Electron suite**

Run: `pnpm test:e2e`  
Expected: PASS with zero failed scenarios.

- [ ] **Step 5: Commit**

```bash
git add tests/e2e playwright.config.ts package.json
git commit -m "test: cover PRD 05 user scenarios"
```

### Task 16: Packaged acceptance and product decision report

**Files:**
- Create: `docs/acceptance/prd-05-four-level-hierarchy.md`
- Modify: `tests/e2e/packaged-runtime.spec.ts`
- Modify: `docs/architecture/domain-model.md`
- Modify: `docs/architecture/process-model.md`
- Modify: `docs/architecture/event-and-stream-protocol.md`
- Modify: `AGENTS.md`

**Interfaces:**
- Produces: one evidence row per PRD 05 requirement and a user-runnable `Matou.app`.

- [ ] **Step 1: Add failing packaged product scenarios**

Extend packaged E2E to cover first launch, hierarchy switch, split, restart, invalid
path, detach/return, and tray hide/restore.

- [ ] **Step 2: Verify the new scenario fails before final packaging adjustments**

Run: `pnpm test:packaged`  
Expected: a product assertion fails if resources, native-window bootstrap, or packaged
Runtime wiring differs from development.

- [ ] **Step 3: Fix packaging integration and write the acceptance matrix**

The acceptance document uses this decision-ready structure for every requirement:

```md
| 用户场景 | 用户能看到/完成什么 | 产品影响 | 自动化证据 | 手工验收步骤 | 结果 |
```

List technical paths only as supporting evidence after the user-visible result.

- [ ] **Step 4: Run the full fresh completion gate**

Run:

```bash
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
pnpm test:packaged
git status --short
```

Expected: all commands exit 0; no uncommitted product changes remain after the final
commit; generated release output remains ignored.

- [ ] **Step 5: Inspect the packaged application manually**

Open `apps/desktop/release/mac-arm64/Matou.app` and verify the acceptance document's
manual rows, including visual hierarchy clarity, drag states, divider behavior,
overflow visibility, exact destructive copy, tray restore, and detached-window return.

- [ ] **Step 6: Commit the completed PRD 05 feature**

```bash
git add AGENTS.md docs apps packages tests tooling package.json pnpm-lock.yaml playwright.config.ts
git commit -m "feat: complete PRD 05 hierarchy management"
```

- [ ] **Step 7: Stop at the user acceptance gate**

Present the packaged app, the acceptance document, key user-visible changes, and the
fresh verification summary. Keep PRD 03 pending until the user explicitly approves
PRD 05.
