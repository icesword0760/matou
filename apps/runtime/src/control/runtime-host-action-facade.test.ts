import { execFileSync } from 'node:child_process'
import { access, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import type { DomainCommandMetadata } from '@matou/domain'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { SessionCanvasService } from '../session-canvas/session-canvas-service'
import type {
  CreateForkInput,
  ForkWorkflowResult,
  RetryForkInput
} from '../session-canvas/fork-workflow-service'
import { ForkWorkflowError } from '../session-canvas/fork-workflow-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ForkBatchCoordinator } from './fork-batch-coordinator'
import { HostActionConfirmationService } from './host-action-confirmation-service'
import { HostActionTargetResolver } from './host-action-target-resolver'
import type {
  ForkBatchResult,
  HostActionResult,
  HostCanvasClosePreview,
  HostRemovalPreview
} from './host-action-types'
import { CapabilityTokenService } from './host-control-server'
import type { HostCallerIdentity } from './host-control-types'
import {
  RuntimeHostActionFacade,
  type RuntimeHostActionFacadeDependencies
} from './runtime-host-action-facade'

let database: RuntimeDatabase
let transactions: DomainTransactionManager
let hierarchy: HierarchyApplicationService
let sessionCanvas: SessionCanvasService
let resolver: HostActionTargetResolver
let confirmations: HostActionConfirmationService
let facade: RuntimeHostActionFacade
let caller: HostCallerIdentity
let root: string
let workspaceRoot: string
let clock: number
let stopSessions: ReturnType<typeof vi.fn<(sessionIds: string[]) => Promise<void>>>
let createForkChild: ReturnType<typeof vi.fn<(
  command: DomainCommandMetadata,
  input: CreateForkInput
) => Promise<ForkWorkflowResult>>>
let retryFork: ReturnType<typeof vi.fn<(
  command: DomainCommandMetadata,
  input: RetryForkInput
) => Promise<ForkWorkflowResult>>>

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-runtime-host-action-'))
  workspaceRoot = join(root, 'workspace')
  await mkdir(workspaceRoot)
  await writeFile(join(workspaceRoot, 'README.md'), 'keep this project\n')
  execFileSync('git', ['init', '-b', 'main'], { cwd: workspaceRoot })
  execFileSync('git', ['config', 'user.name', 'Matou Test'], { cwd: workspaceRoot })
  execFileSync('git', ['config', 'user.email', 'matou@example.test'], { cwd: workspaceRoot })
  execFileSync('git', ['add', 'README.md'], { cwd: workspaceRoot })
  execFileSync('git', ['commit', '-m', 'baseline'], { cwd: workspaceRoot })

  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  sessionCanvas = new SessionCanvasService(database, transactions)
  const initial = hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: workspaceRoot,
    defaultName: 'Workspace', now: 1
  })
  caller = { runId: 'run-parent', sessionId: initial.session!.id }
  resolver = new HostActionTargetResolver(database)
  confirmations = new HostActionConfirmationService({
    randomRef: (() => {
      let ordinal = 0
      return () => `confirmation-${++ordinal}`
    })()
  })
  clock = 100
  stopSessions = vi.fn(async () => undefined)

  createForkChild = vi.fn(async (metadata, input) => createForkResult(metadata, input))
  retryFork = vi.fn(async () => { throw new Error('unexpected Fork retry') })
  facade = createFacade()
})

afterEach(async () => {
  database.close()
  await rm(root, { recursive: true, force: true })
})

describe('RuntimeHostActionFacade create and Fork actions', () => {
  it('creates named structures and preserves caller focus unless enter is explicit', async () => {
    const secondRoot = join(root, 'second-workspace')
    await mkdir(secondRoot)

    const workspace = await facade.execute('structure.create.workspace', caller, {
      path: secondRoot, title: 'Second Workspace', submissionKey: 'create-workspace'
    })
    expect(workspace).toMatchObject({
      kind: 'created', entity: 'workspace',
      path: { workspace: { title: 'Second Workspace', path: secondRoot } },
      focusedPath: { workspace: { title: 'Workspace', path: workspaceRoot } }
    })
    expect(resolver.resolveEntity(caller, { kind: 'self' }, 'unused').sessionId)
      .toBe(caller.sessionId)

    const task = await facade.execute('structure.create.task', caller, {
      workspace: { kind: 'current', entity: 'workspace' },
      title: '服务层重构', submissionKey: 'create-task'
    }) as CreatedResult
    expect(task).toMatchObject({
      kind: 'created', entity: 'task',
      path: { task: { title: '服务层重构' } },
      focusedPath: { session: { ref: `session:${caller.sessionId}` } }
    })

    const canvas = await facade.execute('structure.create.canvas', caller, {
      task: { kind: 'ref', ref: task.path.task!.ref,
        projectionRevision: resolver.projectionRevision(caller) },
      title: '评审画布', submissionKey: 'create-canvas'
    }) as CreatedResult
    expect(canvas).toMatchObject({
      kind: 'created', entity: 'canvas', path: { canvas: { title: '评审画布' } },
      focusedPath: { session: { ref: `session:${caller.sessionId}` } }
    })

    const session = await facade.execute('structure.create.session', caller, {
      canvas: { kind: 'ref', ref: canvas.path.canvas!.ref,
        projectionRevision: resolver.projectionRevision(caller) },
      profile: 'codex', title: '验收助手', submissionKey: 'create-session', enter: true
    })
    expect(session).toMatchObject({
      kind: 'created', entity: 'session',
      path: { session: { title: '验收助手' } },
      focusedPath: { session: { title: '验收助手' } }
    })
  })

  it('replays the same create key and rejects the same key with changed input', async () => {
    const request = {
      workspace: { kind: 'current', entity: 'workspace' } as const,
      title: '幂等事项', submissionKey: 'same-create-key'
    }
    const first = await facade.execute('structure.create.task', caller, request)
    const replay = await facade.execute('structure.create.task', caller, request)

    expect(replay).toEqual(first)
    expect(count('tasks', "title = '幂等事项' AND archived_at IS NULL")).toBe(1)
    await expectFault(facade.execute('structure.create.task', caller, {
      ...request, title: '改变输入'
    }), 'PATH_CONFLICT')
    expect(count('tasks', "title = '改变输入' AND archived_at IS NULL")).toBe(0)
  })

  it('maps hierarchy name collisions and invalid Workspace paths to PATH_CONFLICT', async () => {
    await expectFault(facade.execute('structure.create.workspace', caller, {
      path: join(workspaceRoot, 'README.md'), submissionKey: 'invalid-workspace-path'
    }), 'PATH_CONFLICT')

    const taskRequest = {
      workspace: { kind: 'current', entity: 'workspace' } as const,
      title: '同名事项'
    }
    await facade.execute('structure.create.task', caller, {
      ...taskRequest, submissionKey: 'duplicate-task-first'
    })
    await expectFault(facade.execute('structure.create.task', caller, {
      ...taskRequest, submissionKey: 'duplicate-task-second'
    }), 'PATH_CONFLICT')

    const canvasRequest = {
      task: { kind: 'current', entity: 'task' } as const,
      title: '同名画布'
    }
    await facade.execute('structure.create.canvas', caller, {
      ...canvasRequest, submissionKey: 'duplicate-canvas-first'
    })
    await expectFault(facade.execute('structure.create.canvas', caller, {
      ...canvasRequest, submissionKey: 'duplicate-canvas-second'
    }), 'PATH_CONFLICT')
  })

  it('creates three named children through the durable coordinator and restores caller focus', async () => {
    const result = await facade.execute('structure.fork.children', caller, {
      source: { kind: 'self' }, batchKey: 'three-options', items: [
        { itemKey: 'one', title: '轻量适配', environment: { mode: 'current' } },
        { itemKey: 'two', title: '服务层重构', environment: { mode: 'current' } },
        { itemKey: 'three', title: '完整架构', environment: { mode: 'current' } }
      ]
    }) as ForkBatchResult

    expect(result).toMatchObject({ kind: 'fork-batch', succeeded: 3, failed: 0 })
    expect(result.items.map(({ title }) => title)).toEqual(['轻量适配', '服务层重构', '完整架构'])
    expect(resolver.resolveEntity(caller, { kind: 'self' }, 'unused').sessionId)
      .toBe(caller.sessionId)
    expect(database.get<{ active_session_id: string }>(
      `SELECT active_session_id FROM window_scene_focus
       WHERE window_id = 'window-1' AND scene_id = ?`, sourceSceneId()
    )?.active_session_id).toBe(caller.sessionId)
  })

  it('keeps Task 6 partial results and retries only its failed durable item', async () => {
    createForkChild.mockRejectedValueOnce(new Error('temporary branch reservation'))
    const request = {
      source: { kind: 'self' } as const,
      batchKey: 'durable-retry',
      items: [
        { itemKey: 'failed', title: '失败后重试', environment: { mode: 'current' } as const },
        { itemKey: 'ready', title: '已经成功', environment: { mode: 'current' } as const }
      ]
    }

    const first = await facade.execute('structure.fork.children', caller, request) as ForkBatchResult
    expect(first).toMatchObject({
      kind: 'fork-batch', succeeded: 1, failed: 1,
      retry: { batchKey: 'durable-retry', itemKeys: ['failed'] }
    })
    const successfulRef = first.items.find(({ itemKey }) => itemKey === 'ready')!.sessionRef

    const retried = await facade.execute('structure.fork.children', caller, {
      ...request, retryItemKeys: ['failed']
    }) as ForkBatchResult
    expect(retried).toMatchObject({ kind: 'fork-batch', succeeded: 2, failed: 0 })
    expect(retried.items.find(({ itemKey }) => itemKey === 'ready')!.sessionRef)
      .toBe(successfulRef)
    expect(createForkChild).toHaveBeenCalledTimes(3)
    expect(retryFork).not.toHaveBeenCalled()
  })

  it('resolves every batch environment before it invokes the coordinator', async () => {
    const createChildren = vi.fn(async () => {
      throw new Error('batch coordinator must not run')
    })
    const preflightFacade = createFacade({
      forkBatches: {
        createChildren,
        retryFailures: createChildren,
        preflightAccepted: () => false,
        coordinateAcceptedFork: createChildren
      }
    })

    await expectFault(preflightFacade.execute('structure.fork.children', caller, {
      source: { kind: 'self' }, batchKey: 'preflight-all', items: [
        { itemKey: 'valid', title: '当前环境', environment: { mode: 'current' } },
        { itemKey: 'invalid', title: '错误 Worktree', environment: {
          mode: 'new-worktree', branch: 'feature/not-a-git-workspace'
        } }
      ]
    }), 'WORKTREE_CONFLICT')
    expect(createChildren).not.toHaveBeenCalled()
  })

  it('rejects a durable batch key when any submitted input changes', async () => {
    const request = {
      source: { kind: 'self' } as const,
      batchKey: 'same-batch-key',
      items: [{ itemKey: 'one', title: '原方案', environment: { mode: 'current' } as const }]
    }
    await facade.execute('structure.fork.children', caller, request)

    await expectFault(facade.execute('structure.fork.children', caller, {
      ...request, items: [{ ...request.items[0]!, title: '已改变方案' }]
    }), 'PATH_CONFLICT')
    expect(createForkChild).toHaveBeenCalledTimes(1)
  })

  it('lets the durable coordinator replay an accepted new-Worktree batch', async () => {
    seedGitState()
    const request = {
      source: { kind: 'self' } as const,
      batchKey: 'new-worktree-replay',
      items: [{
        itemKey: 'one', title: '新 Worktree 方案',
        environment: { mode: 'new-worktree', branch: 'feature/replay-batch' } as const
      }]
    }
    const first = await facade.execute('structure.fork.children', caller, request)
    execFileSync('git', ['branch', 'feature/replay-batch'], { cwd: workspaceRoot })

    const replay = await facade.execute('structure.fork.children', caller, request)
    expect(replay).toEqual(first)
    expect(createForkChild).toHaveBeenCalledTimes(1)
  })

  it('returns PATH_CONFLICT for changed durable input before fresh environment checks', async () => {
    const request = {
      source: { kind: 'self' } as const,
      batchKey: 'changed-environment',
      items: [{
        itemKey: 'one', title: '环境冲突', environment: { mode: 'current' } as const
      }]
    }
    await facade.execute('structure.fork.children', caller, request)
    seedGitState()

    await expectFault(facade.execute('structure.fork.children', caller, {
      ...request,
      items: [{
        ...request.items[0]!,
        environment: { mode: 'new-worktree', branch: 'main' }
      }]
    }), 'PATH_CONFLICT')
    expect(createForkChild).toHaveBeenCalledTimes(1)
  })

  it('normalizes a single Fork result, keeps focus, and maps typed workflow errors', async () => {
    const forked = await facade.execute('structure.fork.child', caller, {
      source: { kind: 'self' }, title: '单个方案', environment: { mode: 'current' },
      submissionKey: 'single-fork'
    })
    expect(forked).toMatchObject({
      kind: 'forked', state: 'ready', environment: { mode: 'current' },
      path: { session: { title: '单个方案' } }
    })
    expect(database.get<{ active_session_id: string }>(
      `SELECT active_session_id FROM window_scene_focus
       WHERE window_id = 'window-1' AND scene_id = ?`, sourceSceneId()
    )?.active_session_id).toBe(caller.sessionId)

    createForkChild.mockRejectedValueOnce(new ForkWorkflowError(
      'FORK_SOURCE_NOT_READY', '来源会话尚未准备完成'
    ))
    await expectFault(facade.execute('structure.fork.child', caller, {
      source: { kind: 'self' }, title: '未就绪', environment: { mode: 'current' },
      submissionKey: 'not-ready-fork'
    }), 'TARGET_NOT_READY')

    createForkChild.mockRejectedValueOnce(new ForkWorkflowError(
      'BRANCH_CONFLICT', '目标分支已存在'
    ))
    await expectFault(facade.execute('structure.fork.child', caller, {
      source: { kind: 'self' }, title: '分支冲突', environment: { mode: 'current' },
      submissionKey: 'branch-conflict-fork'
    }), 'BRANCH_CONFLICT')
  })

  it('replays an accepted single Fork before revalidating its now-reserved branch', async () => {
    seedGitState()
    const request = {
      source: { kind: 'self' } as const,
      title: '单个 Worktree 方案',
      environment: { mode: 'new-worktree', branch: 'feature/replay-single' } as const,
      submissionKey: 'single-new-worktree'
    }
    const first = await facade.execute('structure.fork.child', caller, request)
    execFileSync('git', ['branch', 'feature/replay-single'], { cwd: workspaceRoot })

    const replay = await facade.execute('structure.fork.child', caller, request)
    expect(replay).toEqual(first)
    expect(createForkChild).toHaveBeenCalledTimes(2)
  })

  it('uses the durable coordinator for single-Fork startup and prompt delivery', async () => {
    const startSession = vi.fn(async () => undefined)
    const waitUntilReady = vi.fn(async () => undefined)
    const sendPrompt = vi.fn(async () => undefined)
    const forkBatches = new ForkBatchCoordinator({
      database,
      createChild: createForkChild,
      retryChild: retryFork,
      startSession,
      waitUntilReady,
      sendPrompt,
      now: () => ++clock
    })
    const startingFacade = createFacade({ forkBatches })
    const request = {
      source: { kind: 'self' } as const,
      title: '已分配单节点',
      environment: { mode: 'current' } as const,
      prompt: '实现单节点方案',
      start: true,
      submissionKey: 'single-start'
    }

    const first = await startingFacade.execute('structure.fork.child', caller, request)
    const restartedStart = vi.fn(async () => undefined)
    const restartedWait = vi.fn(async () => undefined)
    const restartedSend = vi.fn(async () => undefined)
    const restartedFacade = createFacade({
      forkBatches: new ForkBatchCoordinator({
        database,
        createChild: createForkChild,
        retryChild: retryFork,
        startSession: restartedStart,
        waitUntilReady: restartedWait,
        sendPrompt: restartedSend,
        now: () => ++clock
      })
    })
    const replay = await restartedFacade.execute('structure.fork.child', caller, request)

    expect(first).toMatchObject({ kind: 'forked', state: 'started' })
    expect(replay).toEqual(first)
    expect(startSession).toHaveBeenCalledTimes(1)
    expect(waitUntilReady).toHaveBeenCalledTimes(1)
    expect(sendPrompt).toHaveBeenCalledTimes(1)
    expect(sendPrompt).toHaveBeenCalledWith(
      first.kind === 'forked' ? first.sessionRef.slice('session:'.length) : '',
      '实现单节点方案'
    )
    expect(restartedStart).not.toHaveBeenCalled()
    expect(restartedWait).not.toHaveBeenCalled()
    expect(restartedSend).not.toHaveBeenCalled()
  })

  it('rejects concurrent changed input for one single-Fork key before a second workflow call', async () => {
    let release!: () => void
    const gate = new Promise<void>((resolve) => { release = resolve })
    const blockedFork = vi.fn(async (
      metadata: DomainCommandMetadata,
      input: CreateForkInput
    ) => {
      await gate
      return createForkResult(metadata, input)
    })
    const concurrentFacade = createFacade({
      forkWorkflow: { createForkChild: blockedFork, createForkSibling: blockedFork }
    })
    const first = concurrentFacade.execute('structure.fork.child', caller, {
      source: { kind: 'self' }, title: '并发原请求', environment: { mode: 'current' },
      submissionKey: 'concurrent-single'
    })
    await vi.waitFor(() => expect(blockedFork).toHaveBeenCalledTimes(1))
    const changed = concurrentFacade.execute('structure.fork.child', caller, {
      source: { kind: 'self' }, title: '并发改变请求', environment: { mode: 'current' },
      submissionKey: 'concurrent-single'
    })

    release()
    await expect(first).resolves.toMatchObject({ kind: 'forked' })
    await expectFault(changed, 'PATH_CONFLICT')
    expect(blockedFork).toHaveBeenCalledTimes(1)
  })
})

describe('RuntimeHostActionFacade destructive actions', () => {
  it('previews and removes a stable three-session subtree, stops runs, and keeps files', async () => {
    const { child, grandchild } = seedRemovableBranch()
    const preview = await facade.execute('structure.remove.preview', caller, {
      target: { kind: 'session', sessionId: caller.sessionId }, scope: 'subtree'
    }) as HostRemovalPreview
    expect(preview).toMatchObject({
      kind: 'removal-preview',
      impact: {
        scope: 'subtree', sessions: 3, descendants: 2,
        preservesProjectFiles: true, preservesBranches: true, preservesWorktrees: true
      }
    })

    const committed = await facade.execute('structure.remove.commit', caller, {
      confirmationRef: preview.confirmationRef
    })
    expect(committed).toMatchObject({
      kind: 'removed', removedTasks: 0, removedCanvases: 0, removedSessions: 3,
      activePath: { session: { title: 'Survivor' } }
    })
    expect(stopSessions).toHaveBeenCalledWith(expect.arrayContaining([
      caller.sessionId, child, grandchild
    ]))
    await expect(access(join(workspaceRoot, 'README.md'))).resolves.toBeUndefined()
    expect(execFileSync('git', ['branch', '--show-current'], { cwd: workspaceRoot, encoding: 'utf8' }).trim())
      .toBe('main')
  })

  it('removes a Workspace through the domain service without deleting its project or Worktree', async () => {
    const removableRoot = join(root, 'removable-workspace')
    const retainedWorktree = join(root, 'retained-worktree')
    await mkdir(removableRoot)
    await mkdir(retainedWorktree)
    await writeFile(join(removableRoot, 'project.txt'), 'retain project\n')
    await writeFile(join(retainedWorktree, 'worktree.txt'), 'retain worktree\n')
    const created = await facade.execute('structure.create.workspace', caller, {
      path: removableRoot, title: '可移除工作空间', submissionKey: 'workspace-to-remove'
    }) as CreatedResult
    const workspaceId = created.createdRef.slice('workspace:'.length)
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('retained-worktree-context', ?, 'git-worktree', ?, 1)`,
      workspaceId, retainedWorktree
    )
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         state, created_at, updated_at
       ) VALUES (
         'retained-worktree', 'retained-worktree-context', ?, ?, 'feature/retained',
         'ready', 1, 1
       )`,
      removableRoot, retainedWorktree
    )

    const preview = await facade.execute('structure.remove.preview', caller, {
      target: {
        kind: 'ref', ref: created.createdRef,
        projectionRevision: resolver.projectionRevision(caller)
      },
      scope: 'node'
    }) as HostRemovalPreview
    const result = await facade.execute('structure.remove.commit', caller, {
      confirmationRef: preview.confirmationRef
    })

    expect(result).toMatchObject({
      kind: 'removed', removedTasks: 1, removedCanvases: 1, removedSessions: 1,
      activePath: { workspace: { title: 'Workspace' } }
    })
    expect(database.get<{ state: string }>(
      "SELECT state FROM worktrees WHERE id = 'retained-worktree'"
    )).toEqual({ state: 'ready' })
    await expect(access(join(removableRoot, 'project.txt'))).resolves.toBeUndefined()
    await expect(access(join(retainedWorktree, 'worktree.txt'))).resolves.toBeUndefined()
  })

  it('removes a Task through the existing replacement and focus rules', async () => {
    const created = await facade.execute('structure.create.task', caller, {
      workspace: { kind: 'current', entity: 'workspace' },
      title: '待移除事项', submissionKey: 'task-to-remove'
    }) as CreatedResult
    const preview = await facade.execute('structure.remove.preview', caller, {
      target: {
        kind: 'ref', ref: created.createdRef,
        projectionRevision: resolver.projectionRevision(caller)
      },
      scope: 'node'
    }) as HostRemovalPreview
    const result = await facade.execute('structure.remove.commit', caller, {
      confirmationRef: preview.confirmationRef
    })

    expect(result).toMatchObject({
      kind: 'removed', removedTasks: 1, removedCanvases: 1, removedSessions: 1,
      activePath: { session: { ref: `session:${caller.sessionId}` } }
    })
    expect(stopSessions).toHaveBeenCalledWith([created.path.session!.ref.slice('session:'.length)])
  })

  it('uses node-only removal semantics for a leaf even when subtree was requested', async () => {
    const survivor = sessionCanvas.createShellSibling(command('leaf-survivor'), {
      windowId: 'window-1', sceneId: sourceSceneId(), sourceSessionId: caller.sessionId,
      title: 'Leaf', now: ++clock
    })
    sessionCanvas.setFocusedSession({
      windowId: 'window-1', sceneId: sourceSceneId(), sessionId: caller.sessionId, now: ++clock
    })

    const preview = await facade.execute('structure.remove.preview', caller, {
      target: { kind: 'session', sessionId: survivor.session!.id }, scope: 'subtree'
    }) as HostRemovalPreview
    expect(preview.impact).toMatchObject({ scope: 'node', descendants: 0, sessions: 1 })

    await expect(facade.execute('structure.remove.commit', caller, {
      confirmationRef: preview.confirmationRef
    })).resolves.toMatchObject({ kind: 'removed', removedSessions: 1 })
  })

  it('rejects a close commit when revision or impact changed after preview', async () => {
    const revision = resolver.projectionRevision(caller)
    const preview = await facade.execute('structure.canvas-close.preview', caller, {
      target: { kind: 'ref', ref: `scene:${sourceSceneId()}`, projectionRevision: revision }
    }) as HostCanvasClosePreview

    sessionCanvas.createShellSibling(command('impact-change'), {
      windowId: 'window-1', sceneId: sourceSceneId(), sourceSessionId: caller.sessionId,
      title: 'Extra', now: ++clock
    })

    await expectFault(facade.execute('structure.canvas-close.commit', caller, {
      confirmationRef: preview.confirmationRef
    }), 'CONFIRMATION_STALE')
    expect(database.get('SELECT id FROM scenes WHERE id = ? AND archived_at IS NULL', sourceSceneId()))
      .toBeDefined()
  })

  it('rejects a commit when impact changes without changing the topology revision', async () => {
    const survivor = sessionCanvas.createShellSibling(command('impact-only-survivor'), {
      windowId: 'window-1', sceneId: sourceSceneId(), sourceSessionId: caller.sessionId,
      title: 'Impact-only survivor', now: ++clock
    })
    const preview = await facade.execute('structure.remove.preview', caller, {
      target: { kind: 'session', sessionId: survivor.session!.id }, scope: 'node'
    }) as HostRemovalPreview
    const revision = resolver.projectionRevision(caller)
    database.run(
      `INSERT INTO session_runs (
         id, session_id, ordinal, runtime_generation, pid, status, started_at
       ) VALUES ('impact-only-run', ?, 1, 'generation-test', 9001, 'running', ?)`,
      survivor.session!.id, ++clock
    )
    expect(resolver.projectionRevision(caller)).toBe(revision)

    await expectFault(facade.execute('structure.remove.commit', caller, {
      confirmationRef: preview.confirmationRef
    }), 'CONFIRMATION_STALE')
    expect(database.get(
      'SELECT id FROM sessions WHERE id = ? AND archived_at IS NULL', survivor.session!.id
    )).toBeDefined()
  })

  it('closes a non-final canvas through the domain rule and returns the new active path', async () => {
    const target = sessionCanvas.createCanvas(command('second-canvas'), {
      windowId: 'window-1', taskId: sourceTaskId(), title: '可关闭画布',
      navigation: 'preserve', now: ++clock
    })
    const preview = await facade.execute('structure.canvas-close.preview', caller, {
      target: { kind: 'ref', ref: `scene:${target.created.scene.id}`,
        projectionRevision: resolver.projectionRevision(caller) }
    }) as HostCanvasClosePreview

    const result = await facade.execute('structure.canvas-close.commit', caller, {
      confirmationRef: preview.confirmationRef
    })
    expect(result).toMatchObject({
      kind: 'canvas-closed', targetRef: `scene:${target.created.scene.id}`,
      removedSessions: 1,
      activePath: { canvas: { ref: `scene:${sourceSceneId()}` } }
    })
    expect(stopSessions).toHaveBeenCalledWith([target.created.session.id])
  })

  it('inherits the existing last-canvas rule instead of deleting its only Session', async () => {
    const preview = await facade.execute('structure.canvas-close.preview', caller, {
      target: { kind: 'current', entity: 'canvas' }
    }) as HostCanvasClosePreview
    expect(preview.impact).toMatchObject({ canvases: 0, sessions: 0, descendants: 0 })

    const result = await facade.execute('structure.canvas-close.commit', caller, {
      confirmationRef: preview.confirmationRef
    })
    expect(result).toMatchObject({
      kind: 'canvas-closed', targetRef: `scene:${sourceSceneId()}`,
      removedSessions: 0,
      activePath: { session: { ref: `session:${caller.sessionId}` } }
    })
    expect(stopSessions).not.toHaveBeenCalled()
    expect(database.get('SELECT id FROM sessions WHERE id = ? AND archived_at IS NULL', caller.sessionId))
      .toBeDefined()
  })

  it('consumes a confirmation once and revokes it synchronously with its run token', async () => {
    seedRemovableBranch()
    const first = await facade.execute('structure.remove.preview', caller, {
      target: { kind: 'session', sessionId: caller.sessionId }, scope: 'node'
    }) as HostRemovalPreview
    await facade.execute('structure.remove.commit', caller, {
      confirmationRef: first.confirmationRef
    })
    await expectFault(facade.execute('structure.remove.commit', caller, {
      confirmationRef: first.confirmationRef
    }), 'CONFIRMATION_REQUIRED')

    const activeCaller = {
      runId: 'run-survivor',
      sessionId: database.get<{ id: string }>(
        'SELECT id FROM sessions WHERE archived_at IS NULL ORDER BY created_at DESC LIMIT 1'
      )!.id
    }
    const second = await facade.execute('structure.canvas-close.preview', activeCaller, {
      target: { kind: 'current', entity: 'canvas' }
    }) as HostCanvasClosePreview
    const tokens = new CapabilityTokenService('generation-1', {
      onRunRevoked: (runId) => confirmations.revokeRun(runId)
    })
    tokens.issue(activeCaller, ['host.identify'], Date.now() + 1_000)
    tokens.revokeRun(activeCaller.runId)

    await expectFault(facade.execute('structure.canvas-close.commit', activeCaller, {
      confirmationRef: second.confirmationRef
    }), 'CONFIRMATION_REQUIRED')
  })

  it('keeps an action-mismatched confirmation available for its original commit', async () => {
    const removable = sessionCanvas.createShellSibling(command('action-bound-confirmation'), {
      windowId: 'window-1', sceneId: sourceSceneId(), sourceSessionId: caller.sessionId,
      title: 'Action-bound leaf', now: ++clock
    })
    const preview = await facade.execute('structure.remove.preview', caller, {
      target: { kind: 'session', sessionId: removable.session!.id }, scope: 'node'
    }) as HostRemovalPreview

    await expectFault(facade.execute('structure.canvas-close.commit', caller, {
      confirmationRef: preview.confirmationRef
    }), 'CONFIRMATION_STALE')
    await expect(facade.execute('structure.remove.commit', caller, {
      confirmationRef: preview.confirmationRef
    })).resolves.toMatchObject({ kind: 'removed', removedSessions: 1 })
  })

  it('maps writable actions against a read-only Runtime to STORAGE_READ_ONLY', async () => {
    const readOnly = RuntimeDatabase.openReadOnly(database.path)
    try {
      const readOnlyFacade = createFacade({ database: readOnly })
      await expectFault(readOnlyFacade.execute('structure.create.task', caller, {
        workspace: { kind: 'current', entity: 'workspace' },
        title: '只读操作', submissionKey: 'read-only-create'
      }), 'STORAGE_READ_ONLY')
    } finally {
      readOnly.close()
    }
  })
})

function createFacade(
  overrides: Partial<RuntimeHostActionFacadeDependencies> = {}
): RuntimeHostActionFacade {
  const forkBatches = new ForkBatchCoordinator({
    database,
    createChild: createForkChild,
    retryChild: retryFork,
    startSession: async () => undefined,
    waitUntilReady: async () => undefined,
    sendPrompt: async () => undefined,
    now: () => ++clock
  })
  return new RuntimeHostActionFacade({
    database,
    resolver,
    confirmations,
    hierarchy,
    sessionCanvas,
    forkWorkflow: {
      createForkChild,
      createForkSibling: createForkChild
    },
    forkBatches,
    stopSessions,
    now: () => ++clock,
    ...overrides
  })
}

async function createForkResult(
  metadata: DomainCommandMetadata,
  input: CreateForkInput
): Promise<ForkWorkflowResult> {
  const created = sessionCanvas.createSessionSibling(metadata, {
    windowId: input.windowId,
    sceneId: input.sceneId,
    sourceSessionId: input.sourceSessionId,
    parentSessionId: input.sourceSessionId,
    profile: 'shell',
    title: input.name,
    ...(input.environment.mode === 'new-worktree'
      ? {}
      : { executionContextId: input.environment.executionContextId }),
    now: input.now
  })
  return { ...created, forkState: 'succeeded' }
}

function seedRemovableBranch(): { survivor: string; child: string; grandchild: string } {
  const sceneId = sourceSceneId()
  const survivor = sessionCanvas.createShellSibling(command('removal-survivor'), {
    windowId: 'window-1', sceneId, sourceSessionId: caller.sessionId,
    title: 'Survivor', now: ++clock
  })
  const child = sessionCanvas.createShellSibling(command('removal-child'), {
    windowId: 'window-1', sceneId, sourceSessionId: caller.sessionId,
    parentSessionId: caller.sessionId, title: 'Child', now: ++clock
  })
  const grandchild = sessionCanvas.createShellSibling(command('removal-grandchild'), {
    windowId: 'window-1', sceneId, sourceSessionId: child.session!.id,
    parentSessionId: child.session!.id, title: 'Grandchild', now: ++clock
  })
  sessionCanvas.setFocusedSession({
    windowId: 'window-1', sceneId, sessionId: caller.sessionId, now: ++clock
  })
  return {
    survivor: survivor.session!.id,
    child: child.session!.id,
    grandchild: grandchild.session!.id
  }
}

function sourceSceneId(): string {
  return database.get<{ scene_id: string }>(
    `SELECT membership.scene_id
     FROM session_canvas_memberships AS membership WHERE membership.session_id = ?`,
    caller.sessionId
  )!.scene_id
}

function sourceTaskId(): string {
  return database.get<{ task_id: string }>('SELECT task_id FROM sessions WHERE id = ?', caller.sessionId)!.task_id
}

function seedGitState(): void {
  const executionContextId = database.get<{ execution_context_id: string }>(
    'SELECT execution_context_id FROM sessions WHERE id = ?', caller.sessionId
  )!.execution_context_id
  database.run(
    `INSERT OR REPLACE INTO execution_context_git_states (
       execution_context_id, repository_root, state, branch, detached_head,
       dirty, error_message, updated_at
     ) VALUES (?, ?, 'ready', 'main', NULL, 0, NULL, ?)`,
    executionContextId, workspaceRoot, ++clock
  )
}

function count(table: string, predicate = '1 = 1'): number {
  return database.get<{ count: number }>(
    `SELECT COUNT(*) AS count FROM ${table} WHERE ${predicate}`
  )!.count
}

function command(commandId: string): DomainCommandMetadata {
  return { commandId, commandType: 'test', requestHash: `hash:${commandId}` }
}

async function expectFault(promise: Promise<unknown>, code: string): Promise<void> {
  try {
    await promise
  } catch (error) {
    expect(error).toMatchObject({ code })
    return
  }
  throw new Error(`expected Host action fault ${code}`)
}

type CreatedResult = Extract<HostActionResult, { kind: 'created' }>
