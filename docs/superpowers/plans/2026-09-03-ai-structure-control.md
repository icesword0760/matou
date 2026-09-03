# Matou AI Structure Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Matou 托管的 Claude Code 与 Codex 会话通过自然语言安全、可恢复地创建、Fork、移除、关闭和导航工作空间、事项、画布与会话结构。

**Architecture:** 在现有 Host Control 上增加高层产品动作协议，由 `RuntimeHostActionFacade` 统一解析稳定目标并复用现有层级、会话画布、Fork 与 Worktree 服务。移除和关闭使用 Runtime 内存中的一次性确认记录；跨窗口导航通过 Runtime 与目标 Renderer 的请求/回执桥接完成，结构写入仍以 Runtime 为唯一权威。

**Tech Stack:** TypeScript 5、Node.js、Electron、React、SQLite、Zod、Vitest、Playwright、pnpm workspace

**Spec:** `docs/superpowers/specs/2026-09-03-ai-structure-control-design.md`

## Global Constraints

- 用户可见术语统一使用“移除”；画布动作使用“关闭画布”；Worktree 清理保持为独立动作。
- 创建、Fork、聚焦和切换在目标唯一且参数完整时直接执行；移除与关闭先返回影响预览，再使用短时、一次性的确认引用提交。
- Git 分支与 Worktree 由用户明确决定；普通目录只接受 `current` 环境。
- 批量操作保留成功项、继续处理剩余项，并仅重试失败项。
- 普通创建默认保持调用方焦点；用户明确要求进入目标时才导航。
- Fork 继承 provider、模型、权限模式和对话上下文；进程重启只更新内部 Host Control 凭证。
- 自然语言结果只展示产品标题、路径、环境和状态，隐藏内部 ID、确认引用与控制凭证。
- 源码、测试、资源路径、文档和注释只使用 Matou 自有命名或 `generic`、`legacy`、`reference` 等中性术语。
- 每次提交前运行 `pnpm check:identifiers`；最终验收区分单元测试、端到端测试和真实 App 运行证据。
- 本计划不新增数据库迁移：现有 Worktree 复用通过已有 `execution_contexts`、`worktrees` 与 `session_fork_intents.target_execution_context_id` 字段表达。

## File Map

### New runtime control units

- `apps/runtime/src/control/host-action-types.ts`：高层动作输入、输出、错误与运行时解析。
- `apps/runtime/src/control/host-action-target-resolver.ts`：把会话、工作空间、事项、画布、路径和稳定 ref 解析为唯一权威实体。
- `apps/runtime/src/control/host-action-confirmation-service.ts`：影响摘要哈希、短时确认记录、调用方绑定和一次性消费。
- `apps/runtime/src/control/runtime-host-action-facade.ts`：创建、Fork、预览、提交与导航的单一业务入口。
- `apps/runtime/src/control/fork-batch-coordinator.ts`：批量 item 幂等、部分成功、失败项重试和可选任务启动。
- `apps/runtime/src/control/provider-ready-registry.ts`：在 provider 身份 hook 到达后唤醒等待发送任务的批量条目。
- `apps/runtime/src/control/host-navigation-broker.ts`：按窗口注册 Renderer 通道，发送导航请求并等待回执。

### Existing files to extend

- `apps/runtime/src/control/host-control-types.ts`、`host-control-server.ts`、`runtime-control-backend.ts`、`host-topology-projector.ts`
- `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- `apps/runtime/src/session-canvas/session-canvas-service.ts`、`fork-workflow-service.ts`
- `apps/runtime/src/runtime-server.ts`、`apps/runtime/src/index.ts`
- `apps/runtime/src/cli/mt-cli.ts`
- `packages/contracts/src/protocol.ts`
- `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`、`RuntimeProvider.tsx`
- `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/SKILL.md`
- `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/references/commands.md`
- `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/references/target-resolution.md`
- `apps/runtime/control-assets/providers/codex-developer-instructions.md`
- `package.json`

---

### Task 1: Define the high-level action contract and target metadata

**Files:**
- Create: `packages/contracts/src/host-navigation.ts`
- Modify: `packages/contracts/src/index.ts`
- Create: `apps/runtime/src/control/host-action-types.ts`
- Modify: `apps/runtime/src/control/host-control-types.ts`
- Modify: `apps/runtime/src/control/host-topology-projector.ts`
- Test: `apps/runtime/src/control/host-action-types.test.ts`
- Test: `apps/runtime/src/control/host-topology-projector.test.ts`

**Interfaces:**
- Consumes: existing `HostCallerIdentity`, `HostTargetSelector`, `HostTarget` and projection revision semantics.
- Produces: shared `HostNavigationPath`; `HostActionMethod`, `ForkEnvironmentChoice`, `HostActionRequest`, `HostActionResult`, `HostActionErrorCode`, `parseHostActionRequest(method, params)` and enriched `HostTarget.environment`.

- [ ] **Step 1: Write failing parser and topology tests**

```ts
it('parses an explicit three-item child batch without inventing Git choices', () => {
  expect(parseHostActionRequest('structure.fork.children', {
    source: { kind: 'self' },
    batchKey: 'three-options-v1',
    items: [
      { itemKey: 'light', title: '轻量适配', environment: { mode: 'current' } },
      { itemKey: 'service', title: '服务层重构', environment: {
        mode: 'new-worktree', branch: 'feature/service-refactor'
      } },
      { itemKey: 'architecture', title: '完整架构升级', environment: {
        mode: 'existing-worktree', branch: 'main', worktreeRef: 'worktree:main'
      } }
    ]
  })).toMatchObject({ method: 'structure.fork.children', batchKey: 'three-options-v1' })
})

it('includes branch and stable worktree refs in all-scope topology', () => {
  const target = projector.list(caller, 'all').find(({ sessionId }) => sessionId === 'session-2')
  expect(target?.environment).toEqual({
    executionContextRef: 'context:context-2',
    mode: 'git-worktree',
    branch: 'feature/service-refactor',
    worktreeRef: 'worktree:worktree-2'
  })
})
```

- [ ] **Step 2: Run the focused tests and confirm the missing contract fails**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/host-action-types.test.ts src/control/host-topology-projector.test.ts`

Expected: FAIL because `host-action-types.ts`, `parseHostActionRequest` and `HostTarget.environment` are absent.

- [ ] **Step 3: Add discriminated action types and strict runtime parsing**

```ts
// packages/contracts/src/host-navigation.ts
export interface HostNavigationPath {
  windowId: string
  workspaceId: string
  taskId: string
  sceneId: string
  sessionId?: string
}

// apps/runtime/src/control/host-action-types.ts
import type { HostNavigationPath } from '@matou/contracts'

export type HostActionMethod =
  | 'structure.create.workspace' | 'structure.create.task'
  | 'structure.create.canvas' | 'structure.create.session'
  | 'structure.fork.child' | 'structure.fork.sibling' | 'structure.fork.children'
  | 'structure.remove.preview' | 'structure.remove.commit'
  | 'structure.canvas-close.preview' | 'structure.canvas-close.commit'
  | 'navigation.focus.session' | 'navigation.switch.workspace'
  | 'navigation.switch.task' | 'navigation.switch.canvas'

export type ForkEnvironmentChoice =
  | { mode: 'current' }
  | { mode: 'existing-worktree'; branch: string; worktreeRef: string }
  | { mode: 'new-worktree'; branch: string }

export type HostActionErrorCode =
  | 'TARGET_NOT_FOUND' | 'AMBIGUOUS_TARGET' | 'STALE_PROJECTION'
  | 'TARGET_NOT_READY' | 'CAPABILITY_DENIED' | 'CONFIRMATION_REQUIRED'
  | 'CONFIRMATION_EXPIRED' | 'CONFIRMATION_STALE' | 'PATH_CONFLICT'
  | 'BRANCH_CONFLICT' | 'WORKTREE_CONFLICT' | 'PARTIAL_SUCCESS'
  | 'NAVIGATION_TIMEOUT' | 'STORAGE_READ_ONLY'

export type HostEntitySelector =
  | { kind: 'current'; entity: 'workspace' | 'task' | 'canvas' | 'session' }
  | { kind: 'ref'; ref: string; projectionRevision: string }
  | HostTargetSelector

export interface ForkItemInput {
  itemKey: string
  title: string
  environment: ForkEnvironmentChoice
  prompt?: string
  start?: boolean
}

export type HostActionRequest =
  | { method: 'structure.create.workspace'; path: string; title?: string;
      submissionKey: string; enter?: boolean }
  | { method: 'structure.create.task'; workspace: HostEntitySelector; title?: string;
      submissionKey: string; enter?: boolean }
  | { method: 'structure.create.canvas'; task: HostEntitySelector; title?: string;
      submissionKey: string; enter?: boolean }
  | { method: 'structure.create.session'; canvas: HostEntitySelector;
      profile: 'shell' | 'claude-code' | 'codex'; title?: string;
      submissionKey: string; enter?: boolean }
  | { method: 'structure.fork.child' | 'structure.fork.sibling';
      source: HostTargetSelector; title: string; environment: ForkEnvironmentChoice;
      prompt?: string; start?: boolean; submissionKey: string }
  | { method: 'structure.fork.children'; source: HostTargetSelector;
      batchKey: string; items: ForkItemInput[]; retryItemKeys?: string[] }
  | { method: 'structure.remove.preview'; target: HostEntitySelector;
      scope: 'node' | 'subtree' }
  | { method: 'structure.remove.commit'; confirmationRef: string }
  | { method: 'structure.canvas-close.preview'; target: HostEntitySelector }
  | { method: 'structure.canvas-close.commit'; confirmationRef: string }
  | { method: 'navigation.focus.session'; target: HostEntitySelector }
  | { method: 'navigation.switch.workspace' | 'navigation.switch.task' |
      'navigation.switch.canvas'; target: HostEntitySelector }

export interface HostResultPath {
  window: { ref: string; title: string }
  workspace: { ref: string; title: string; path: string }
  task?: { ref: string; title: string }
  canvas?: { ref: string; title: string }
  session?: { ref: string; title: string }
}

export interface HostImpactSummary {
  target: HostResultPath
  scope: 'node' | 'subtree'
  tasks: number
  canvases: number
  sessions: number
  descendants: number
  liveRuns: number
  terminalProcesses: number
  preservesProjectFiles: true
  preservesBranches: true
  preservesWorktrees: true
}

export type ForkBatchItemState = 'created' | 'ready' | 'started' | 'failed'

export interface ForkBatchResult {
  kind: 'fork-batch'
  batchKey: string
  succeeded: number
  failed: number
  items: Array<{ itemKey: string; title: string; state: ForkBatchItemState;
    sessionRef?: string; environment: ForkEnvironmentChoice; error?: string }>
  retry?: { batchKey: string; itemKeys: string[] }
}

export interface HostRemovalPreview {
  kind: 'removal-preview'
  impact: HostImpactSummary
  confirmationRef: string
}

export interface HostCanvasClosePreview {
  kind: 'canvas-close-preview'
  impact: HostImpactSummary
  confirmationRef: string
}

export type HostActionResult =
  | { kind: 'created'; entity: 'workspace' | 'task' | 'canvas' | 'session';
      createdRef: string; path: HostResultPath; focusedPath: HostResultPath }
  | { kind: 'forked'; state: 'created' | 'ready' | 'started';
      sessionRef: string; path: HostResultPath; environment: ForkEnvironmentChoice }
  | ForkBatchResult
  | HostRemovalPreview | HostCanvasClosePreview
  | { kind: 'removed'; targetRef: string; removedTasks: number;
      removedCanvases: number; removedSessions: number; activePath: HostResultPath }
  | { kind: 'canvas-closed'; targetRef: string; removedSessions: number;
      activePath: HostResultPath }
  | { kind: 'navigated'; finalPath: HostNavigationPath }

export function parseHostActionRequest(
  method: HostActionMethod,
  params: unknown
): HostActionRequest
```

Use Zod discriminated unions with these bounds: title 1–160 UTF-8 bytes, prompt at most 64 KiB, batch at most 50 items, unique `itemKey`, `batchKey` and `submissionKey` at most 160 characters, and a required explicit `environment` on every Fork item. Reject `new-worktree` for a non-Git workspace in the resolver rather than altering the submitted choice.

- [ ] **Step 4: Enrich topology targets from execution context and Worktree joins**

```ts
export interface HostTargetEnvironment {
  executionContextRef: string
  mode: 'directory' | 'git-checkout' | 'git-worktree'
  branch?: string
  worktreeRef?: string
}

export interface HostTarget {
  // existing fields stay unchanged
  environment: HostTargetEnvironment
}
```

Extend the projector query with active `execution_contexts`, `execution_context_git_states` and `worktrees`, then derive refs as `context:${id}` and `worktree:${id}`. Keep current-level ordering unchanged so relative target semantics stay stable.

- [ ] **Step 5: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/host-action-types.test.ts src/control/host-topology-projector.test.ts && pnpm --filter @matou/contracts build && pnpm --filter @matou/runtime typecheck && pnpm check:identifiers`

Expected: PASS.

```bash
git add packages/contracts/src/host-navigation.ts packages/contracts/src/index.ts apps/runtime/src/control/host-action-types.ts apps/runtime/src/control/host-action-types.test.ts apps/runtime/src/control/host-control-types.ts apps/runtime/src/control/host-topology-projector.ts apps/runtime/src/control/host-topology-projector.test.ts
git commit -m "feat: define host structure actions"
```

---

### Task 2: Add stable hierarchy target resolution and impact snapshots

**Files:**
- Create: `apps/runtime/src/control/host-action-target-resolver.ts`
- Test: `apps/runtime/src/control/host-action-target-resolver.test.ts`

**Interfaces:**
- Consumes: `HostTarget`, `HostTargetSelector`, `ForkEnvironmentChoice`, `RuntimeDatabase`.
- Produces: `ResolvedHostEntity`, `ResolvedHierarchyPath`, `RemovalImpact`, `resolveEntity(caller, selector, expectedRevision)`, `resolveForkEnvironment(source, choice)` and `previewRemoval(target, scope)`.

- [ ] **Step 1: Write failing resolution and impact tests**

```ts
it('returns one complete hierarchy path for a unique canvas ref', () => {
  expect(resolver.resolveEntity(caller, { kind: 'ref', ref: 'scene:scene-2' }, revision))
    .toMatchObject({ kind: 'canvas', windowId: 'window-1', workspaceId: 'workspace-1',
      taskId: 'task-1', sceneId: 'scene-2' })
})

it('counts descendants and live terminal runs before removal', () => {
  expect(resolver.previewRemoval({ kind: 'session', sessionId: 'parent' }, 'subtree'))
    .toMatchObject({ sessions: 3, descendants: 2, liveRuns: 2,
      preservesProjectFiles: true, preservesWorktrees: true })
})

it('resolves main only when the submitted worktree ref carries main', () => {
  expect(resolver.resolveForkEnvironment(source, {
    mode: 'existing-worktree', branch: 'main', worktreeRef: 'worktree:main'
  })).toMatchObject({ mode: 'existing-worktree', executionContextId: 'context-main' })
})
```

- [ ] **Step 2: Run the focused test and confirm the resolver is missing**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/host-action-target-resolver.test.ts`

Expected: FAIL with the missing module or export.

- [ ] **Step 3: Implement one resolver for every product level**

```ts
export interface ResolvedHierarchyPath {
  windowId: string
  workspaceId: string
  taskId: string
  sceneId: string
  sessionId?: string
}

export type ResolvedHostEntity =
  | ({ kind: 'workspace'; workspaceId: string } & ResolvedHierarchyPath)
  | ({ kind: 'task'; taskId: string } & ResolvedHierarchyPath)
  | ({ kind: 'canvas'; sceneId: string } & ResolvedHierarchyPath)
  | ({ kind: 'session'; sessionId: string; mountId?: string } & ResolvedHierarchyPath)

export interface RemovalImpact {
  target: ResolvedHostEntity
  scope: 'node' | 'subtree'
  tasks: number
  canvases: number
  sessions: number
  descendants: number
  liveRuns: number
  terminalProcesses: number
  preservesProjectFiles: true
  preservesBranches: true
  preservesWorktrees: true
}
```

Resolve stable refs directly from the database; resolve relative selectors through `HostTopologyProjector`; compare caller-supplied revision before ordinal/ref writes; sort ambiguity candidates by window/workspace/task/canvas/session ordinal and return their human-readable paths. Verify `existing-worktree.branch` against the Worktree’s current Git state and verify new branch absence with `git show-ref --verify --quiet refs/heads/<branch>`.

- [ ] **Step 4: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/host-action-target-resolver.test.ts && pnpm --filter @matou/runtime typecheck && pnpm check:identifiers`

Expected: PASS.

```bash
git add apps/runtime/src/control/host-action-target-resolver.ts apps/runtime/src/control/host-action-target-resolver.test.ts
git commit -m "feat: resolve host structure targets"
```

---

### Task 3: Implement one-time impact confirmations

**Files:**
- Create: `apps/runtime/src/control/host-action-confirmation-service.ts`
- Test: `apps/runtime/src/control/host-action-confirmation-service.test.ts`

**Interfaces:**
- Consumes: `HostCallerIdentity`, `RemovalImpact`, current projection revision.
- Produces: `HostActionConfirmationService.issue(input)`, `consume(input)` and typed confirmation faults.

- [ ] **Step 1: Write failing lifecycle tests**

```ts
it('binds a confirmation to caller, action, target, revision and impact hash', () => {
  const ref = service.issue({ caller, action: 'remove', targetRef: 'session:parent',
    scope: 'subtree', projectionRevision: 'r1', impact, now: 1_000 })
  expect(() => service.consume({ ref, caller: otherCaller, action: 'remove',
    targetRef: 'session:parent', scope: 'subtree', projectionRevision: 'r1',
    impact, now: 1_100 })).toThrowErrorMatchingObject({ code: 'CONFIRMATION_REQUIRED' })
  expect(service.consume({ ref, caller, action: 'remove', targetRef: 'session:parent',
    scope: 'subtree', projectionRevision: 'r1', impact, now: 1_100 })).toBeDefined()
  expect(() => service.consume({ ref, caller, action: 'remove', targetRef: 'session:parent',
    scope: 'subtree', projectionRevision: 'r1', impact, now: 1_200 }))
    .toThrowErrorMatchingObject({ code: 'CONFIRMATION_REQUIRED' })
})
```

Add separate cases for expiry at 120 seconds and changed revision/impact producing `CONFIRMATION_EXPIRED` and `CONFIRMATION_STALE`.

- [ ] **Step 2: Run the focused test and confirm the service is missing**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/host-action-confirmation-service.test.ts`

Expected: FAIL with the missing service.

- [ ] **Step 3: Implement hash-bound issue and consume**

```ts
export class HostActionConfirmationService {
  constructor(options: { ttlMs?: number; randomRef?: () => string } = {})
  issue(input: ConfirmationIssueInput): string
  consume(input: ConfirmationConsumeInput): ConfirmationRecord
  revokeRun(runId: string): void
}

export interface ConfirmationIssueInput {
  caller: HostCallerIdentity
  action: 'remove' | 'canvas-close'
  targetRef: string
  scope: 'node' | 'subtree'
  projectionRevision: string
  impact: HostImpactSummary
  now: number
}

export interface ConfirmationConsumeInput extends ConfirmationIssueInput {
  ref: string
}

export interface ConfirmationRecord extends ConfirmationIssueInput {
  impactHash: string
  expiresAt: number
}
```

Store records in a private `Map`; generate 24-byte base64url refs; hash canonical JSON containing action, target, scope and impact with SHA-256; delete a record before returning from successful `consume`; purge expired records during issue/consume; never persist raw refs or return them from human formatters.

- [ ] **Step 4: Run tests and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/host-action-confirmation-service.test.ts && pnpm check:identifiers`

Expected: PASS.

```bash
git add apps/runtime/src/control/host-action-confirmation-service.ts apps/runtime/src/control/host-action-confirmation-service.test.ts
git commit -m "feat: add host action confirmations"
```

---

### Task 4: Make create workflows title-aware and focus-preserving

**Files:**
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.ts`
- Modify: `apps/runtime/src/hierarchy/hierarchy-application-service.test.ts`
- Modify: `apps/runtime/src/session-canvas/session-canvas-service.ts`
- Modify: `apps/runtime/src/session-canvas/session-canvas-service.test.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.test.ts`

**Interfaces:**
- Consumes: existing hierarchy transaction helpers and `Session['kind']`.
- Produces: title-aware `createWorkspace`, `createTask`, `createCanvas`, generic `createSessionSibling`, and `navigation: 'activate' | 'preserve'` input behavior.

- [ ] **Step 1: Write failing create-workflow tests**

```ts
it('creates a named task with its default canvas and Shell while preserving focus', () => {
  const before = service.activateTask({ windowId: 'window-1', taskId: 'task-active', now: 10 })
  const created = service.createTask(command('create-task'), {
    windowId: 'window-1', workspaceId: 'workspace-1', title: '服务层重构',
    navigation: 'preserve', now: 20
  })
  expect(created.created.task.name).toBe('服务层重构')
  expect(created.navigation.activeTaskId).toBe(before.navigation.activeTaskId)
})

it.each(['shell', 'claude-code', 'codex'] as const)(
  'creates a %s sibling with an explicit title without provider history inheritance',
  (profile) => {
    const result = canvas.createSessionSibling(command('create-session'), {
      windowId: 'window-1', sceneId: 'scene-1', sourceSessionId: 'source',
      profile, title: `New ${profile}`, navigation: 'preserve', now: 30
    })
    expect(result.created.session).toMatchObject({ kind: profile, title: `New ${profile}` })
    expect(db.get('SELECT id FROM provider_bindings WHERE session_id = ?',
      result.created.session.id)).toBeUndefined()
  }
)
```

- [ ] **Step 2: Run focused tests and confirm current activation/default-title behavior fails**

Run: `pnpm --filter @matou/runtime exec vitest run src/hierarchy/hierarchy-application-service.test.ts src/session-canvas/session-canvas-service.test.ts src/rpc/runtime-rpc-router.test.ts`

Expected: FAIL on the new input fields, `created` result and `createSessionSibling`.

- [ ] **Step 3: Extend create inputs and results without changing UI defaults**

```ts
export interface CreateNavigationOptions {
  navigation?: 'activate' | 'preserve'
}

export interface CreatedHierarchyPath {
  workspace: Workspace
  task: Task
  scene: Scene
  session: Session
  mount: SessionMount
}

export interface CreateHierarchyResult extends WorkspaceHierarchyResult {
  created: CreatedHierarchyPath
}
```

Add optional `title` and `navigation` to task/canvas/session inputs, and optional `navigation` to workspace input. Existing Renderer RPC calls omit the field and retain `activate`. Host actions pass `preserve`. Capture the pre-action navigation row inside the same transaction, create entities with final titles, restore the prior path when requested, and return both `created` and current `navigation`.

- [ ] **Step 4: Generalize sibling session creation**

```ts
export interface CreateSessionSiblingInput extends CreateNavigationOptions {
  windowId: string
  sceneId: string
  sourceSessionId: string
  profile: 'shell' | 'claude-code' | 'codex'
  title?: string
  executionContextId?: string
  now: number
}

createSessionSibling(
  command: DomainCommandMetadata,
  input: CreateSessionSiblingInput
): SessionCanvasMutationResult
```

Keep `createShellSibling` as a compatibility adapter that calls `createSessionSibling` with `profile: 'shell'`. New provider sessions start from their default persisted model/permission configuration; they receive no copied provider binding.

- [ ] **Step 5: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/hierarchy/hierarchy-application-service.test.ts src/session-canvas/session-canvas-service.test.ts src/rpc/runtime-rpc-router.test.ts && pnpm --filter @matou/runtime typecheck && pnpm check:identifiers`

Expected: PASS, including old Renderer activation tests.

```bash
git add apps/runtime/src/hierarchy/hierarchy-application-service.ts apps/runtime/src/hierarchy/hierarchy-application-service.test.ts apps/runtime/src/session-canvas/session-canvas-service.ts apps/runtime/src/session-canvas/session-canvas-service.test.ts apps/runtime/src/rpc/runtime-rpc-router.ts apps/runtime/src/rpc/runtime-rpc-router.test.ts
git commit -m "feat: preserve focus for host creates"
```

---

### Task 5: Support explicit Fork environments and branch names

**Files:**
- Modify: `apps/runtime/src/session-canvas/fork-workflow-service.ts`
- Modify: `apps/runtime/src/session-canvas/fork-workflow-service.test.ts`
- Modify: `apps/runtime/src/session-canvas/fork-workflow-service.integration.test.ts`

**Interfaces:**
- Consumes: `ResolvedForkEnvironment` from Task 2 and existing Fork intent/worktree fields.
- Produces: `CreateForkInput.environment`, exact branch creation for new Worktrees, and existing execution-context reuse with no Worktree ownership transfer.

- [ ] **Step 1: Write failing environment tests**

```ts
it('uses the submitted branch when creating a new Worktree', async () => {
  const result = await workflow.createForkChild(command('fork-new'), {
    windowId: 'window-1', sceneId: 'scene-1', sourceSessionId: 'source',
    name: '服务层重构', environment: {
      mode: 'new-worktree', branch: 'feature/service-refactor'
    }, submissionKey: 'fork-new', now: 100
  })
  expect(result.worktree?.branch).toBe('feature/service-refactor')
})

it('reuses an existing Worktree context and leaves its ownership unchanged', async () => {
  const result = await workflow.createForkChild(command('fork-existing'), {
    windowId: 'window-1', sceneId: 'scene-1', sourceSessionId: 'source',
    name: 'Main 环境方案', environment: {
      mode: 'existing-worktree', branch: 'main', worktreeRef: 'worktree:main',
      executionContextId: 'context-main'
    }, submissionKey: 'fork-existing', now: 100
  })
  expect(result.created.session.executionContextId).toBe('context-main')
  expect(worktrees.get('main-worktree')?.ownerSessionId).toBeNull()
})
```

- [ ] **Step 2: Run tests and confirm only generated `current | new` modes exist**

Run: `pnpm --filter @matou/runtime exec vitest run src/session-canvas/fork-workflow-service.test.ts src/session-canvas/fork-workflow-service.integration.test.ts`

Expected: FAIL on `environment` and explicit branch behavior.

- [ ] **Step 3: Replace public mode input with an explicit environment union**

```ts
export type ResolvedForkEnvironment =
  | { mode: 'current'; executionContextId: string }
  | { mode: 'existing-worktree'; executionContextId: string; worktreeId: string;
      worktreeRef: string; branch: string }
  | { mode: 'new-worktree'; branch: string }

export interface CreateForkInput {
  windowId: string
  sceneId: string
  sourceSessionId: string
  name: string
  environment: ResolvedForkEnvironment
  submissionKey?: string
  now: number
}
```

Map `current` and `existing-worktree` to the persisted intent mode `current`; store `target_execution_context_id` for existing-context reuse; preserve the existing Worktree row and its owner fields. Map `new-worktree` to persisted mode `new`, pass its exact branch into `GitPlan`, and reject invalid refs/collisions before any scene node is inserted. Preserve current Renderer RPC by translating `worktreeMode: 'current'` to the source context and `worktreeMode: 'new'` to the existing generated branch helper.

- [ ] **Step 4: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/session-canvas/fork-workflow-service.test.ts src/session-canvas/fork-workflow-service.integration.test.ts && pnpm --filter @matou/runtime typecheck && pnpm check:identifiers`

Expected: PASS and no migration checksum changes.

```bash
git add apps/runtime/src/session-canvas/fork-workflow-service.ts apps/runtime/src/session-canvas/fork-workflow-service.test.ts apps/runtime/src/session-canvas/fork-workflow-service.integration.test.ts
git commit -m "feat: honor explicit fork environments"
```

---

### Task 6: Build provider readiness and batch Fork coordination

**Files:**
- Create: `apps/runtime/src/control/provider-ready-registry.ts`
- Create: `apps/runtime/src/control/fork-batch-coordinator.ts`
- Test: `apps/runtime/src/control/provider-ready-registry.test.ts`
- Test: `apps/runtime/src/control/fork-batch-coordinator.test.ts`
- Modify: `apps/runtime/src/index.ts`

**Interfaces:**
- Consumes: `ForkWorkflowService.createForkChild`, source-to-provider execution descriptor lookup, background `RuntimeServer.startOrResumeSession`, `RuntimeControlBackend.sendText`.
- Produces: `ProviderReadyRegistry.wait(sessionId, timeoutMs)`, `record(sessionId, runId)`, and `ForkBatchCoordinator.createChildren(input)` / `retryFailures(input)`.

- [ ] **Step 1: Write failing readiness and partial-success tests**

```ts
it('continues after one item fails and preserves successful item keys', async () => {
  createChild.mockRejectedValueOnce(new Error('branch collision'))
    .mockResolvedValueOnce(forkResult('session-2'))
    .mockResolvedValueOnce(forkResult('session-3'))
  const result = await coordinator.createChildren(batchFixture())
  expect(result.items.map(({ itemKey, state }) => [itemKey, state])).toEqual([
    ['one', 'failed'], ['two', 'ready'], ['three', 'ready']
  ])
  expect(result.retry).toEqual({ batchKey: 'batch-1', itemKeys: ['one'] })
})

it('waits for provider identity before submitting an assigned task', async () => {
  const pending = coordinator.createChildren(batchFixture({ start: true, prompt: '实现方案二' }))
  await vi.waitFor(() => expect(startSession).toHaveBeenCalledWith('session-2'))
  expect(sendText).not.toHaveBeenCalled()
  ready.record('session-2', 'run-2')
  await pending
  expect(sendText).toHaveBeenCalledWith('session-2', '实现方案二', true)
})
```

- [ ] **Step 2: Run focused tests and confirm the coordinators are missing**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/provider-ready-registry.test.ts src/control/fork-batch-coordinator.test.ts`

Expected: FAIL with missing modules.

- [ ] **Step 3: Implement readiness waiters and item-level idempotency**

```ts
export class ForkBatchCoordinator {
  createChildren(input: CreateForkBatchInput): Promise<ForkBatchResult>
  retryFailures(input: RetryForkBatchInput): Promise<ForkBatchResult>
}

export interface CreateForkBatchInput {
  caller: HostCallerIdentity
  source: ResolvedHostEntity & { kind: 'session' }
  batchKey: string
  items: Array<ForkItemInput & { environment: ResolvedForkEnvironment }>
}

export interface RetryForkBatchInput extends CreateForkBatchInput {
  retryItemKeys: string[]
}
```

Derive each individual submission key as SHA-256 of `batchKey:itemKey`; process items sequentially to keep deterministic DAG ordinals; catch and record per-item faults; use existing fork intents to return prior results on replay; retry only requested keys whose prior state is failed. For `start: true`, register the provider waiter before calling `startSession`, await the matching identity hook, then submit the item prompt with Enter. On provider timeout, keep the node and return `created` with an error describing the pending start.

- [ ] **Step 4: Wire the registry to provider hooks and background execution**

In `apps/runtime/src/index.ts`, create one `ProviderReadyRegistry`; call `record(sessionId, runId)` from `onIdentityRecorded`; inject callbacks into the batch coordinator:

```ts
{
  createChild: (command, input) => forkWorkflow.createForkChild(command, input),
  startSession: async (sessionId) => {
    const descriptor = forkExecutionDescriptor(database, sessionId)
    if (!descriptor) throw new Error(`会话 ${sessionId} 尚未准备完成`)
    await backgroundServer.startOrResumeSession(descriptor)
  },
  waitUntilReady: (sessionId) => providerReady.wait(sessionId, 60_000),
  sendPrompt: (sessionId, prompt) => controlBackend.sendText(sessionId, prompt, true)
}
```

- [ ] **Step 5: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/provider-ready-registry.test.ts src/control/fork-batch-coordinator.test.ts && pnpm --filter @matou/runtime typecheck && pnpm check:identifiers`

Expected: PASS.

```bash
git add apps/runtime/src/control/provider-ready-registry.ts apps/runtime/src/control/provider-ready-registry.test.ts apps/runtime/src/control/fork-batch-coordinator.ts apps/runtime/src/control/fork-batch-coordinator.test.ts apps/runtime/src/index.ts
git commit -m "feat: coordinate batch session forks"
```

---

### Task 7: Implement the Runtime host action facade

**Files:**
- Create: `apps/runtime/src/control/runtime-host-action-facade.ts`
- Test: `apps/runtime/src/control/runtime-host-action-facade.test.ts`
- Modify: `apps/runtime/src/index.ts`

**Interfaces:**
- Consumes: Task 2 resolver, Task 3 confirmation service, Task 4 create services, Task 5 Fork workflow, Task 6 batch coordinator.
- Produces: `RuntimeHostActionFacade.execute(method, caller, params)` as the only Host Control entry for product mutations.

- [ ] **Step 1: Write failing facade behavior tests**

```ts
it('creates three named children and keeps the caller focused', async () => {
  const result = await facade.execute('structure.fork.children', caller, batchParams)
  expect(result).toMatchObject({ kind: 'fork-batch', succeeded: 3, failed: 0 })
  expect(topology.identify(caller).target.sessionId).toBe('parent')
})

it('previews then removes a subtree with a fresh confirmation', async () => {
  const preview = await facade.execute('structure.remove.preview', caller, {
    target: { kind: 'session', sessionId: 'parent' }, scope: 'subtree'
  }) as HostRemovalPreview
  expect(preview.impact.sessions).toBe(3)
  const committed = await facade.execute('structure.remove.commit', caller, {
    confirmationRef: preview.confirmationRef
  })
  expect(committed).toMatchObject({ kind: 'removed', removedSessions: 3 })
})

it('rejects a close commit when the canvas impact changed after preview', async () => {
  const preview = await facade.execute('structure.canvas-close.preview', caller, {
    target: { kind: 'ref', ref: 'scene:scene-1', projectionRevision: 'r1' }
  }) as HostCanvasClosePreview
  seedExtraSession('scene-1')
  await expect(facade.execute('structure.canvas-close.commit', caller, {
    confirmationRef: preview.confirmationRef
  })).rejects.toMatchObject({ code: 'CONFIRMATION_STALE' })
})
```

- [ ] **Step 2: Run the focused test and confirm the facade is missing**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/runtime-host-action-facade.test.ts`

Expected: FAIL with missing facade.

- [ ] **Step 3: Implement creates, Forks and result normalization**

```ts
export class RuntimeHostActionFacade {
  async execute(
    method: HostActionMethod,
    caller: HostCallerIdentity,
    rawParams: unknown
  ): Promise<HostActionResult>
}
```

Parse once with `parseHostActionRequest`; generate domain metadata from `submissionKey` and a canonical request hash; create workspace/task/canvas/session using `navigation: 'preserve'`; resolve all Fork environments before invoking the workflow; return stable refs and human titles/paths. Map storage mode, path, branch, Worktree and target errors to the exact `HostActionErrorCode` values from Task 1.

- [ ] **Step 4: Implement preview/commit with fresh impact recomputation**

On preview, resolve the entity, compute impact and issue a ref. On commit, load the stored action metadata, re-resolve the stable target, recompute revision and impact, then consume the confirmation. Dispatch to `removeWorkspace`, `deleteTask`, `removeSessionBranch` or `closeScene` using their existing confirmed-intent inputs. Dispose returned session IDs through the injected `stopSessions(sessionIds)` callback. Query the post-action active path and include it in the result.

- [ ] **Step 5: Wire the facade in `initializeRuntime`**

Construct the resolver, confirmation service, batch coordinator and facade beside `RuntimeControlBackend`; inject the facade into the backend in Task 8. Revoke confirmations for a run whenever `CapabilityTokenService.revokeRun(runId)` is called by adding an `onRunRevoked` callback.

- [ ] **Step 6: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/runtime-host-action-facade.test.ts && pnpm --filter @matou/runtime typecheck && pnpm check:identifiers`

Expected: PASS.

```bash
git add apps/runtime/src/control/runtime-host-action-facade.ts apps/runtime/src/control/runtime-host-action-facade.test.ts apps/runtime/src/index.ts
git commit -m "feat: execute host structure actions"
```

---

### Task 8: Expose actions through Host Control and session capabilities

**Files:**
- Modify: `apps/runtime/src/control/host-control-types.ts`
- Modify: `apps/runtime/src/control/host-control-server.ts`
- Modify: `apps/runtime/src/control/host-control-server.test.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`

**Interfaces:**
- Consumes: `HostActionMethod`, `RuntimeHostActionFacade.execute`.
- Produces: all 15 structural/navigation scopes over the existing framed socket protocol and injects them into every managed session token.

- [ ] **Step 1: Write failing authorization and dispatch tests**

```ts
it.each([
  'structure.create.workspace', 'structure.create.task', 'structure.create.canvas',
  'structure.create.session', 'structure.fork.child', 'structure.fork.sibling',
  'structure.fork.children', 'structure.remove.preview', 'structure.remove.commit',
  'structure.canvas-close.preview', 'structure.canvas-close.commit',
  'navigation.focus.session', 'navigation.switch.workspace',
  'navigation.switch.task', 'navigation.switch.canvas'
] as const)('authorizes %s independently', async (scope) => {
  const token = tokens.issue(caller, [scope], Date.now() + 10_000)
  expect(await request(socket, token, scope, validParams(scope))).toMatchObject({ ok: true })
  expect(await request(socket, token, otherScope(scope), validParams(otherScope(scope))))
    .toMatchObject({ ok: false, error: { code: 'CAPABILITY_DENIED' } })
})
```

Add RuntimeServer coverage asserting a spawned Shell, Claude Code and Codex process each receives a token whose scope set contains all structural/navigation methods while model and permission settings remain unchanged.

- [ ] **Step 2: Run focused tests and confirm scopes are absent**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/host-control-server.test.ts src/control/runtime-control-backend.test.ts src/runtime-server.test.ts`

Expected: FAIL on the new scopes and backend action dispatch.

- [ ] **Step 3: Extend scope guards, errors and backend delegation**

```ts
export interface HostControlBackend {
  // existing methods stay unchanged
  executeHostAction(
    method: HostActionMethod,
    caller: HostCallerIdentity,
    params: unknown
  ): Promise<HostActionResult>
}
```

Add every action method to `HostControlScope` and `isControlScope`. In `HostControlServer.#dispatch`, delegate high-level methods before terminal target resolution. Export `ControlErrorCode`, add the spec’s error codes, and preserve them when a facade fault reaches `#process` instead of converting them to `INTERNAL_ERROR`. Map stale target revision to `STALE_PROJECTION` rather than the generic conflict result.

- [ ] **Step 4: Inject the full action capability set for each new run**

Create a frozen `MANAGED_SESSION_CONTROL_SCOPES` constant and use it in the existing RuntimeServer token issuance path. Keep tokens run-bound and continue revoking by run ID on process end. Verify that provider mode persistence reads/writes stay untouched.

- [ ] **Step 5: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/control/host-control-server.test.ts src/control/runtime-control-backend.test.ts src/runtime-server.test.ts && pnpm --filter @matou/runtime typecheck && pnpm check:identifiers`

Expected: PASS.

```bash
git add apps/runtime/src/control/host-control-types.ts apps/runtime/src/control/host-control-server.ts apps/runtime/src/control/host-control-server.test.ts apps/runtime/src/control/runtime-control-backend.ts apps/runtime/src/control/runtime-control-backend.test.ts apps/runtime/src/runtime-server.ts apps/runtime/src/runtime-server.test.ts
git commit -m "feat: expose host structure capabilities"
```

---

### Task 9: Add the `mt` create, Fork, remove, close and navigation commands

**Files:**
- Modify: `apps/runtime/src/cli/mt-cli.ts`
- Modify: `apps/runtime/src/cli/mt-cli.test.ts`

**Interfaces:**
- Consumes: all Task 8 Host Control scopes and Task 1 JSON request shapes.
- Produces: the exact CLI command families in the design spec, stdin JSON support, stable human formatters and exit-code mapping.

- [ ] **Step 1: Write failing CLI parsing tests**

```ts
it('submits a batch from stdin without shell quoting loss', async () => {
  const request = vi.fn(async () => ({ kind: 'fork-batch', succeeded: 3, failed: 0, items: [] }))
  expect(await runMt(
    ['fork', 'children', 'self', '--items-json', '-', '--batch-key', 'batch-1', '--json'],
    {}, fixture.io, request, async () => JSON.stringify(batchItems)
  )).toBe(0)
  expect(request).toHaveBeenCalledWith('structure.fork.children', expect.objectContaining({
    batchKey: 'batch-1', items: batchItems
  }))
})

it('commits removal only with the preview confirmation ref', async () => {
  await runMt(['remove', 'commit', 'confirmation-1', '--json'], {}, fixture.io, request)
  expect(request).toHaveBeenCalledWith('structure.remove.commit', {
    confirmationRef: 'confirmation-1'
  })
})
```

Cover every command in the spec, explicit environment JSON, `--start`, `--prompt`, `--submission-key`, revision-bearing refs and all new exit-code groups.

- [ ] **Step 2: Run CLI tests and confirm commands are currently unknown**

Run: `pnpm --filter @matou/runtime exec vitest run src/cli/mt-cli.test.ts`

Expected: FAIL with usage errors for the new command families.

- [ ] **Step 3: Split parsing into focused helpers and add stdin dependency**

```ts
export interface MtDependencies {
  request?: MtRequest
  readStdin?: () => Promise<string>
}

export async function runMt(
  argv: string[],
  environment: MtEnvironment,
  io: MtIo,
  dependencies: MtDependencies | MtRequest = {}
): Promise<number>
```

Retain the current injected-request call shape through a type guard. Add `parseCreateCommand`, `parseForkCommand`, `parseRemoveCommand`, `parseCloseCommand` and `parseNavigationCommand`. Parse `--items-json -` from stdin, bound stdin at 1 MiB, and preserve UTF-8 titles/prompts exactly.

- [ ] **Step 4: Add concise human output and structured JSON output**

Human batch output must list each title, environment and state, then a single success/failure summary. Preview output must state impact and that project files/branches/Worktrees remain. JSON output passes through the authoritative result unchanged. Do not display confirmation refs in human output; the provider rules use `--json` for the commit flow.

- [ ] **Step 5: Run tests, build the CLI asset and commit**

Run: `pnpm --filter @matou/runtime exec vitest run src/cli/mt-cli.test.ts && pnpm build:runtime && pnpm check:identifiers`

Expected: PASS and `apps/runtime/dist/cli/mt-cli` contains the new help text.

```bash
git add apps/runtime/src/cli/mt-cli.ts apps/runtime/src/cli/mt-cli.test.ts
git commit -m "feat: add mt structure commands"
```

---

### Task 10: Teach managed AI sessions the natural-language workflow

**Files:**
- Modify: `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/SKILL.md`
- Modify: `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/references/commands.md`
- Modify: `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/references/target-resolution.md`
- Modify: `apps/runtime/control-assets/providers/codex-developer-instructions.md`
- Modify: `tooling/prepare-runtime-control-assets.test.mjs`

**Interfaces:**
- Consumes: Task 9 CLI commands.
- Produces: matching Claude Code/Codex behavior for title summaries, environment questions, direct creates/navigation, preview/confirmation and partial retry.

- [ ] **Step 1: Write failing packaged-asset assertions**

```js
assert.match(claudeSkill, /mt fork children/)
assert.match(claudeSkill, /创建并分别实现/)
assert.match(codexInstructions, /mt remove preview/)
assert.match(codexInstructions, /只重试失败项/)
assert.doesNotMatch(codexInstructions, /MATOU_CONTROL_TOKEN/)
```

- [ ] **Step 2: Run the asset test and confirm the workflow text is absent**

Run: `node --test tooling/prepare-runtime-control-assets.test.mjs`

Expected: FAIL on the new assertions.

- [ ] **Step 3: Add the same decision sequence to both provider integrations**

Document this exact order: `identify --json` → list/resolve → summarize titles → ask once for missing branch/Worktree choices → execute with `--json` → report human titles/results. Distinguish “创建” from “创建并执行”; require preview plus explicit user confirmation for remove/close; preserve successful batch items; hide internal refs/tokens; use “移除”和“关闭画布” in user-facing wording.

- [ ] **Step 4: Add command examples for the three-option scenario**

```bash
cat <<'JSON' | mt fork children self --items-json - --batch-key three-options-v1 --json
[
  {"itemKey":"light","title":"轻量适配方案","environment":{"mode":"current"}},
  {"itemKey":"service","title":"服务层重构方案","environment":{"mode":"new-worktree","branch":"feature/service-refactor"}},
  {"itemKey":"architecture","title":"完整架构升级","environment":{"mode":"existing-worktree","branch":"main","worktreeRef":"worktree:main"}}
]
JSON
```

- [ ] **Step 5: Run packaging tests and commit**

Run: `node --test tooling/prepare-runtime-control-assets.test.mjs && pnpm build:runtime && pnpm check:identifiers`

Expected: PASS and packaged assets contain both updated integrations.

```bash
git add apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/SKILL.md apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/references/commands.md apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/references/target-resolution.md apps/runtime/control-assets/providers/codex-developer-instructions.md tooling/prepare-runtime-control-assets.test.mjs
git commit -m "feat: teach ai structure workflows"
```

---

### Task 11: Add the Runtime-to-Renderer navigation bridge

**Files:**
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `packages/contracts/src/protocol.test.ts`
- Create: `apps/runtime/src/control/host-navigation-broker.ts`
- Test: `apps/runtime/src/control/host-navigation-broker.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `apps/runtime/src/index.ts`

**Interfaces:**
- Consumes: complete `ResolvedHierarchyPath` and Runtime port lifecycle.
- Produces: `host.navigation-request`, `host.navigation-result`, `HostNavigationBroker.registerWindow`, `unregisterWindow`, `navigate`.

- [ ] **Step 1: Write failing protocol and broker tests**

```ts
it('routes one navigation request to the target main window and resolves its ack', async () => {
  const client = fakeClient('main-window-2')
  broker.registerWindow('main-window-2', client.send)
  const pending = broker.navigate({ requestId: 'nav-1', windowId: 'main-window-2',
    workspaceId: 'workspace-2', taskId: 'task-2', sceneId: 'scene-2',
    sessionId: 'session-2', focusTerminal: true, deadlineAt: 5_000 })
  expect(client.sent[0]).toMatchObject({ type: 'host.navigation-request', requestId: 'nav-1' })
  broker.acknowledge({ requestId: 'nav-1', windowId: 'main-window-2', ok: true,
    finalPath: pathFixture() })
  await expect(pending).resolves.toMatchObject({ finalPath: pathFixture() })
})
```

Add timeout, disconnected window and wrong-window acknowledgement cases.

- [ ] **Step 2: Run focused tests and confirm protocol messages are absent**

Run: `pnpm --filter @matou/contracts exec vitest run src/protocol.test.ts && pnpm --filter @matou/runtime exec vitest run src/control/host-navigation-broker.test.ts src/runtime-server.test.ts`

Expected: FAIL on missing navigation message types.

- [ ] **Step 3: Extend the handshake and navigation wire messages**

```ts
// Renderer hello
{ type: 'protocol.hello'; protocolVersion: 1; clientId: string;
  windowId?: string; windowKind?: 'main' | 'detached-terminal' | 'background' }

// Runtime to Renderer
{ type: 'host.navigation-request'; protocolVersion: 1; requestId: string;
  windowId: string; workspaceId: string; taskId: string; sceneId: string;
  sessionId?: string; focusTerminal: boolean; deadlineAt: number }

// Renderer to Runtime
{ type: 'host.navigation-result'; protocolVersion: 1; requestId: string;
  windowId: string; ok: boolean; finalPath?: HostNavigationPath; error?: string }
```

Keep window metadata optional during protocol transition so existing tests and background ports remain valid.

- [ ] **Step 4: Implement request routing and RuntimeServer registration**

Register only `windowKind: 'main'` connections after successful hello. On port close, remove that exact sender. Add a switch case for `host.navigation-result` that calls `broker.acknowledge`. Reject a pending navigation with `NAVIGATION_TIMEOUT` after its deadline and remove its timer/map entry in every settle path.

- [ ] **Step 5: Connect the facade navigation methods**

Inject the broker into `RuntimeHostActionFacade`. For switch/focus: resolve the complete target path, call `broker.navigate`, and return its acknowledged `finalPath`. `navigation.focus.session` sets `focusTerminal: true`; workspace/task/canvas switches use `false`. No structural mutation occurs before the target Renderer handles the request.

- [ ] **Step 6: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/contracts exec vitest run src/protocol.test.ts && pnpm --filter @matou/runtime exec vitest run src/control/host-navigation-broker.test.ts src/runtime-server.test.ts src/control/runtime-host-action-facade.test.ts && pnpm typecheck && pnpm check:identifiers`

Expected: PASS.

```bash
git add packages/contracts/src/protocol.ts packages/contracts/src/protocol.test.ts apps/runtime/src/control/host-navigation-broker.ts apps/runtime/src/control/host-navigation-broker.test.ts apps/runtime/src/runtime-server.ts apps/runtime/src/runtime-server.test.ts apps/runtime/src/control/runtime-host-action-facade.ts apps/runtime/src/control/runtime-host-action-facade.test.ts apps/runtime/src/index.ts
git commit -m "feat: route host navigation requests"
```

---

### Task 12: Execute navigation in the Desktop and acknowledge visible focus

**Files:**
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.test.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeProvider.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.test.tsx`

**Interfaces:**
- Consumes: Task 11 wire messages and existing `showWindow`, hierarchy commands, `revealSessionByScene`, `terminalFocusRequest`.
- Produces: `RuntimeClient.subscribeHostNavigation(listener)` and `acknowledgeHostNavigation(result)`.

- [ ] **Step 1: Write failing client and UI navigation tests**

```ts
it('publishes Runtime navigation requests and posts Renderer acknowledgements', () => {
  const listener = vi.fn()
  client.subscribeHostNavigation(listener)
  port.receive(navigationRequestFixture())
  expect(listener).toHaveBeenCalledWith(navigationRequestFixture())
  client.acknowledgeHostNavigation({ requestId: 'nav-1', windowId: 'main-window-1',
    ok: true, finalPath: pathFixture() })
  expect(port.sent.at(-1)).toMatchObject({ type: 'host.navigation-result', requestId: 'nav-1' })
})

it('brings the target window forward, activates the full path and focuses the card', async () => {
  renderShell()
  await emitHostNavigation(navigationRequestFixture({ sessionId: 'session-3' }))
  expect(window.matouDesktop.showWindow).toHaveBeenCalledWith('main-window-1')
  expect(commands.activateWorkspace).toHaveBeenCalledWith('workspace-1')
  expect(commands.activateTask).toHaveBeenCalledWith('task-1')
  expect(commands.activateScene).toHaveBeenCalledWith('scene-1')
  expect(commands.setFocusedSession).toHaveBeenCalledWith('scene-1', 'session-3')
  expect(runtime.acknowledgeHostNavigation).toHaveBeenCalledWith(
    expect.objectContaining({ requestId: 'nav-1', ok: true })
  )
})
```

- [ ] **Step 2: Run focused tests and confirm subscriptions are absent**

Run: `pnpm --filter @matou/desktop exec vitest run src/renderer/src/runtime/RuntimeClient.test.ts src/renderer/src/hierarchy/HierarchyShell.test.tsx`

Expected: FAIL on the new client methods and navigation listener.

- [ ] **Step 3: Attach the window identity during hello**

In `RuntimeProvider`, read `windowId` from `window.location.search`; main hierarchy pages send `windowKind: 'main'`; detached pages send `windowKind: 'detached-terminal'`. Pass both to `RuntimeClient`, which includes them in every hello after initial connect or port replacement.

- [ ] **Step 4: Execute a navigation request as one ordered promise chain**

`HierarchyShell` must: verify `request.windowId === projection.windowId`; call `showWindow`; activate workspace, task and scene in order; set the focused session when present; set `levelParentByScene`; set `revealSessionByScene`; increment `terminalFocusRequest`; wait one animation frame so the card is mounted; acknowledge success with the final path. Catch any step and acknowledge `{ ok: false, error }`. Dedupe completed request IDs so a repeated transport message has no second UI mutation.

- [ ] **Step 5: Run tests, typecheck and commit**

Run: `pnpm --filter @matou/desktop exec vitest run src/renderer/src/runtime/RuntimeClient.test.ts src/renderer/src/hierarchy/HierarchyShell.test.tsx && pnpm --filter @matou/desktop typecheck && pnpm check:identifiers`

Expected: PASS.

```bash
git add apps/desktop/src/renderer/src/runtime/RuntimeClient.ts apps/desktop/src/renderer/src/runtime/RuntimeClient.test.ts apps/desktop/src/renderer/src/runtime/RuntimeProvider.tsx apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx apps/desktop/src/renderer/src/hierarchy/HierarchyShell.test.tsx
git commit -m "feat: focus host navigation targets"
```

---

### Task 13: Add end-to-end scenarios and regression gates

**Files:**
- Create: `tests/e2e/ai-host-structure-control.spec.ts`
- Create: `tests/e2e/ai-host-navigation.spec.ts`
- Modify: `tests/e2e/ai-host-control-cli.spec.ts`
- Modify: `package.json`

**Interfaces:**
- Consumes: packaged `mt` CLI, real Runtime, Desktop renderer and provider launch fixtures.
- Produces: executable acceptance coverage for create/Fork/confirm/navigation/persistence and inclusion in the root E2E gate.

- [ ] **Step 1: Write failing real-App create and batch scenarios**

```ts
test('creates three named child sessions with mixed user-selected environments', async ({ page }) => {
  const parent = await seedResumableProviderSession(page)
  const result = await runMtInSession(page, parent.sessionId, [
    'fork', 'children', 'self', '--items-json', JSON.stringify(mixedEnvironmentItems),
    '--batch-key', 'e2e-three-options', '--json'
  ])
  expect(result.items.map((item: any) => item.state)).toEqual(['ready', 'ready', 'ready'])
  await expect(page.locator('[data-session-title="轻量适配方案"]')).toBeVisible()
  await expect(page.locator('[data-session-title="服务层重构方案"]')).toBeVisible()
  await expect(page.locator('[data-session-title="完整架构升级"]')).toBeVisible()
  await expect(page.locator(`[data-session-id="${parent.sessionId}"]`)).toHaveAttribute('data-focused', 'true')
})
```

Add cases for all-current environment, all-new Worktrees, one branch collision with failed-only retry, and `--start` prompt delivery after provider readiness.

- [ ] **Step 2: Write failing confirmation and cross-window scenarios**

Create a subtree, preview it, mutate it, verify stale confirmation, preview again, commit, and assert file/branch/Worktree preservation. Open two main windows, invoke `mt focus` from the first, and assert the second window is frontmost, correct hierarchy path is active, target card is visible and terminal receives keyboard input.

- [ ] **Step 3: Run only the new E2E files and observe the first failing boundary**

Run: `pnpm build && pnpm exec playwright test tests/e2e/ai-host-structure-control.spec.ts tests/e2e/ai-host-navigation.spec.ts --workers=1 --reporter=line`

Expected: FAIL until all production paths are connected; fix failures at their owning task boundary rather than weakening assertions.

- [ ] **Step 4: Add the new specs to the root E2E command**

Insert both files immediately after `tests/e2e/ai-host-control-cli.spec.ts` in `package.json#scripts.test:e2e` so future releases exercise high-level Host Control by default.

- [ ] **Step 5: Run targeted E2E, full unit tests and static gates**

Run:

```bash
pnpm build
pnpm exec playwright test tests/e2e/ai-host-control-cli.spec.ts tests/e2e/ai-host-structure-control.spec.ts tests/e2e/ai-host-navigation.spec.ts --workers=1 --reporter=line
pnpm test
pnpm typecheck
pnpm check:identifiers
```

Expected: all commands PASS.

- [ ] **Step 6: Commit E2E coverage**

```bash
git add tests/e2e/ai-host-structure-control.spec.ts tests/e2e/ai-host-navigation.spec.ts tests/e2e/ai-host-control-cli.spec.ts package.json
git commit -m "test: cover ai structure control"
```

---

### Task 14: Complete the product parity matrix and real App acceptance

**Files:**
- Create: `docs/prd/ai-structure-control-reference-matrix.md`
- Modify: `docs/superpowers/specs/2026-09-03-ai-structure-control-design.md`

**Interfaces:**
- Consumes: Task 13 test artifacts and real App screenshots/logs.
- Produces: closed interaction matrix and final implementation evidence.

- [ ] **Step 1: Create the interaction comparison matrix**

Use one row per acceptance scenario with columns: `场景`, `reference 基线`, `Matou 实际结果`, `运行证据`, `差异结论`. Include create workspace/task/canvas/three profiles; child/sibling/mixed batch; create-only versus create-and-run; partial retry; remove node/subtree/task/workspace; close canvas including the last canvas rule; current-window and cross-window navigation; restart permission/model persistence; manual Shell CLI.

- [ ] **Step 2: Run the packaged App acceptance pass**

Run: `pnpm package:dir`, launch `apps/desktop/release/mac-arm64/码头.app` (or the generated architecture-specific directory), and execute all 14 real-App scenarios from spec section 14.2. Capture screenshots after the visible result, not during loading transitions. Record actual artifact paths and observed results in the matrix.

- [ ] **Step 3: Verify restart persistence and credential rotation separately**

Set a provider session to a non-default model and elevated permission mode, restart the App, restore the session, and record that both visible settings remain. In the same run, record that the old Host Control token receives `CAPABILITY_DENIED` and the restored process receives a different working token. Keep these as two separate evidence rows.

- [ ] **Step 4: Fix the duplicate numbered line in the design spec and mark implementation state**

Change the second item `9.` in section 8.2 to `10.` and update the status line to `实现完成，待产品验收` only after Tasks 1–13 and the packaged pass have succeeded.

- [ ] **Step 5: Run final gates and commit documentation**

Run: `pnpm test && pnpm typecheck && pnpm check:identifiers`

Expected: PASS.

```bash
git add docs/prd/ai-structure-control-reference-matrix.md docs/superpowers/specs/2026-09-03-ai-structure-control-design.md
git commit -m "docs: record ai structure control acceptance"
```

## Final Verification Checklist

- [ ] `mt identify/list/read/history/commands/send/key` regressions remain green.
- [ ] Every new scope has an allow and deny authorization assertion.
- [ ] Every create path uses its final title at first projection and preserves focus by default.
- [ ] All three Fork environment modes are verified with real Git state.
- [ ] Batch retries create no duplicate successful nodes.
- [ ] Remove/close impact, expiry, caller binding, one-time use and stale checks are green.
- [ ] Cross-window focus brings the target window forward, reveals the card and focuses terminal input.
- [ ] Model/permission persistence and run-bound token rotation have distinct evidence.
- [ ] Packaged Claude Code and Codex rules contain the same workflow.
- [ ] Product copy uses “移除”和“关闭画布”.
- [ ] `pnpm test`, `pnpm typecheck`, targeted Playwright specs and `pnpm check:identifiers` all pass.
