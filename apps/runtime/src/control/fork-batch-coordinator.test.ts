import { createHash } from 'node:crypto'
import { mkdir, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import type { DomainCommandMetadata } from '@matou/domain'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ResolvedForkEnvironment, ResolvedHostEntity } from './host-action-target-resolver'
import type { HostCallerIdentity } from './host-control-types'
import { ForkBatchCoordinator, type CreateForkBatchInput } from './fork-batch-coordinator'
import { ProviderReadyRegistry } from './provider-ready-registry'
import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { SessionForkIntentRepository } from '../session/session-fork-intent-repository'
import {
  ForkWorkflowService,
  type CreateForkInput,
  type ForkWorkflowResult,
  type RetryForkInput
} from '../session-canvas/fork-workflow-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

const caller: HostCallerIdentity = { runId: 'run-parent', sessionId: 'session-parent' }
const source: ResolvedHostEntity & { kind: 'session' } = {
  kind: 'session',
  windowId: 'window-1',
  workspaceId: 'workspace-1',
  taskId: 'task-1',
  sceneId: 'scene-1',
  sessionId: 'session-parent'
}
const current: ResolvedForkEnvironment = {
  mode: 'current', executionContextId: 'execution-context-1'
}

type CreateChild = (
  command: DomainCommandMetadata,
  input: CreateForkInput
) => Promise<ForkWorkflowResult>
type RetryChild = (
  command: DomainCommandMetadata,
  input: RetryForkInput
) => Promise<ForkWorkflowResult>

let database: RuntimeDatabase
let dataRoot: string

beforeEach(async () => {
  dataRoot = await mkdtemp(join(tmpdir(), 'matou-fork-batch-'))
  database = RuntimeDatabase.open(join(dataRoot, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
})

afterEach(() => database.close())

async function restartDatabase(): Promise<void> {
  database.close()
  database = RuntimeDatabase.open(join(dataRoot, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
}

function forkResult(
  sessionId: string,
  forkState: ForkWorkflowResult['forkState'] = 'succeeded',
  error?: string
): ForkWorkflowResult {
  return {
    forkState,
    ...(error === undefined ? {} : { error }),
    session: { id: sessionId },
    workspace: null,
    executionContext: null,
    task: null,
    scene: null,
    mount: null,
    navigation: { windowId: 'window-1' },
    graph: { sceneId: 'scene-1', nodes: [], edges: [] }
  } as unknown as ForkWorkflowResult
}

function batchFixture(
  items: CreateForkBatchInput['items'] = [
    { itemKey: 'one', title: '方案一', environment: current },
    { itemKey: 'two', title: '方案二', environment: current },
    { itemKey: 'three', title: '方案三', environment: current }
  ],
  batchKey = 'batch-1'
): CreateForkBatchInput {
  return { caller, source, batchKey, items }
}

function coordinatorFixture(overrides: Partial<{
  createChild: CreateChild
  retryChild: RetryChild
  startSession: (sessionId: string) => Promise<void>
  waitUntilReady: (sessionId: string, signal?: AbortSignal) => Promise<unknown>
  sendPrompt: (sessionId: string, prompt: string) => Promise<void>
}> = {}) {
  const createChild = vi.fn<CreateChild>(overrides.createChild ?? (async (_command, input) => (
    forkResult(`created-${input.name}`)
  )))
  const retryChild = vi.fn<RetryChild>(overrides.retryChild ?? (async () => forkResult('retried-session')))
  const startSession = vi.fn(overrides.startSession ?? (async () => undefined))
  const waitUntilReady = vi.fn(overrides.waitUntilReady ?? (async () => undefined))
  const sendPrompt = vi.fn(overrides.sendPrompt ?? (async () => undefined))
  const coordinator = new ForkBatchCoordinator({
    database, createChild, retryChild, startSession, waitUntilReady, sendPrompt, now: () => 123
  })
  return { coordinator, createChild, retryChild, startSession, waitUntilReady, sendPrompt }
}

describe('ForkBatchCoordinator', () => {
  it('derives SHA-256 item submission keys and processes items serially in input order', async () => {
    let active = 0
    let maximumActive = 0
    const calls: Array<{ command: DomainCommandMetadata; input: CreateForkInput }> = []
    const { coordinator } = coordinatorFixture({
      createChild: async (command, input) => {
        active += 1
        maximumActive = Math.max(maximumActive, active)
        calls.push({ command, input })
        await Promise.resolve()
        active -= 1
        return forkResult(`session-${calls.length}`)
      }
    })

    const result = await coordinator.createChildren(batchFixture())

    expect(maximumActive).toBe(1)
    expect(calls.map(({ input }) => input.name)).toEqual(['方案一', '方案二', '方案三'])
    expect(calls.map(({ input }) => input.submissionKey)).toEqual(
      ['one', 'two', 'three'].map((itemKey) => createHash('sha256')
        .update(`batch-1:${itemKey}`).digest('hex'))
    )
    expect(calls.every(({ command, input }) => command.commandId.endsWith(input.submissionKey!)))
      .toBe(true)
    expect(result.items.map(({ state }) => state)).toEqual(['ready', 'ready', 'ready'])
  })

  it('continues after one item fails and preserves successful item keys', async () => {
    const { coordinator, createChild } = coordinatorFixture()
    createChild.mockRejectedValueOnce(new Error('branch collision'))
      .mockResolvedValueOnce(forkResult('session-2'))
      .mockResolvedValueOnce(forkResult('session-3'))

    const result = await coordinator.createChildren(batchFixture())

    expect(result.items.map(({ itemKey, state }) => [itemKey, state])).toEqual([
      ['one', 'failed'], ['two', 'ready'], ['three', 'ready']
    ])
    expect(result.retry).toEqual({ batchKey: 'batch-1', itemKeys: ['one'] })
    expect(result).toMatchObject({ succeeded: 2, failed: 1 })
  })

  it('replays a completed batch result without creating successful nodes again', async () => {
    const { coordinator, createChild } = coordinatorFixture()
    const input = batchFixture()

    const first = await coordinator.createChildren(input)
    const replay = await coordinator.createChildren(input)

    expect(replay).toEqual(first)
    expect(createChild).toHaveBeenCalledTimes(3)
  })

  it('treats reordered object fields as the same idempotent batch input', async () => {
    const { coordinator, createChild } = coordinatorFixture()
    const input = batchFixture([
      { itemKey: 'one', title: '方案一', environment: current }
    ], 'canonical-batch')
    const first = await coordinator.createChildren(input)

    const replay = await coordinator.createChildren({
      caller: { sessionId: caller.sessionId, runId: caller.runId },
      source: {
        sessionId: source.sessionId,
        sceneId: source.sceneId,
        taskId: source.taskId,
        workspaceId: source.workspaceId,
        windowId: source.windowId,
        kind: 'session'
      },
      batchKey: input.batchKey,
      items: [{
        environment: { executionContextId: current.executionContextId, mode: 'current' },
        title: '方案一',
        itemKey: 'one'
      }]
    })

    expect(replay).toEqual(first)
    expect(createChild).toHaveBeenCalledTimes(1)
  })

  it('lets a fresh coordinator reuse durable Fork results through the same submission keys', async () => {
    const durableResults = new Map<string, ForkWorkflowResult>()
    let createdNodes = 0
    const createChild: CreateChild = async (_command, input) => {
      const replay = durableResults.get(input.submissionKey!)
      if (replay) return replay
      createdNodes += 1
      const created = forkResult(`durable-${createdNodes}`)
      durableResults.set(input.submissionKey!, created)
      return created
    }
    const firstCoordinator = coordinatorFixture({ createChild }).coordinator
    const secondCoordinator = coordinatorFixture({ createChild }).coordinator

    const first = await firstCoordinator.createChildren(batchFixture())
    const replay = await secondCoordinator.createChildren(batchFixture())

    expect(replay).toEqual(first)
    expect(createdNodes).toBe(3)
  })

  it('persists canonical input and prompt delivery across a Runtime restart', async () => {
    const real = await realForkFixture()
    const createChild = vi.fn<CreateChild>((command, input) => real.workflow.createForkChild(command, input))
    const retryChild = vi.fn<RetryChild>((command, input) => real.workflow.retryFork(command, input))
    const startSession = vi.fn(async () => undefined)
    const waitUntilReady = vi.fn(async () => undefined)
    const sendPrompt = vi.fn(async () => undefined)
    const dependencies = {
      database, createChild, retryChild, startSession, waitUntilReady, sendPrompt, now: () => 123
    }
    const input = realBatchInput(real.source, {
      itemKey: 'durable', title: '持久方案', environment: real.environment,
      start: true, prompt: '实现持久方案'
    }, 'durable-batch')

    const first = await new ForkBatchCoordinator(dependencies).createChildren(input)
    const sessionsAfterFirst = database.get<{ count: number }>('SELECT COUNT(*) AS count FROM sessions')!.count
    const replay = await new ForkBatchCoordinator(dependencies).createChildren({
      ...input,
      caller: { ...input.caller, runId: 'run-after-restart' }
    })

    expect(first.items[0]).toMatchObject({ state: 'started' })
    expect(replay).toEqual(first)
    expect(createChild).toHaveBeenCalledTimes(1)
    expect(startSession).toHaveBeenCalledTimes(1)
    expect(sendPrompt).toHaveBeenCalledTimes(1)
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM sessions')!.count)
      .toBe(sessionsAfterFirst)
    expect(database.get(
      'SELECT start_state FROM fork_batch_items WHERE batch_key = ? AND item_key = ?',
      input.batchKey, 'durable'
    )).toEqual({ start_state: 'completed' })

    await expect(new ForkBatchCoordinator(dependencies).createChildren({
      ...input,
      caller: { ...input.caller, runId: 'another-run' },
      items: [{ ...input.items[0]!, title: '不同方案' }]
    })).rejects.toThrow('与已提交输入不一致')
    expect(createChild).toHaveBeenCalledTimes(1)
  })

  it('retries only failed item keys and keeps prior successful nodes', async () => {
    const { coordinator, createChild } = coordinatorFixture()
    createChild.mockRejectedValueOnce(new Error('branch collision'))
      .mockResolvedValueOnce(forkResult('session-2'))
      .mockResolvedValueOnce(forkResult('session-3'))
    const input = batchFixture()
    await coordinator.createChildren(input)
    createChild.mockResolvedValueOnce(forkResult('session-1'))

    const retried = await coordinator.retryFailures({ ...input, retryItemKeys: ['one'] })

    expect(createChild).toHaveBeenCalledTimes(4)
    expect(createChild.mock.calls[3]![1].name).toBe('方案一')
    expect(retried.items.map(({ itemKey, state }) => [itemKey, state])).toEqual([
      ['one', 'ready'], ['two', 'ready'], ['three', 'ready']
    ])
    expect(retried.retry).toBeUndefined()
  })

  it('replays only the exact ordered retry set and rejects a different set using an old receipt', async () => {
    const { coordinator, createChild } = coordinatorFixture()
    createChild.mockRejectedValueOnce(new Error('failure A'))
      .mockRejectedValueOnce(new Error('failure B'))
      .mockResolvedValueOnce(forkResult('session-retry-A'))
    const input = batchFixture([{
      itemKey: 'A', title: '方案 A', environment: current
    }, {
      itemKey: 'B', title: '方案 B', environment: current
    }], 'ordered-retry-receipt')
    await coordinator.createChildren(input)

    const first = await coordinator.retryFailures({ ...input, retryItemKeys: ['A'] })
    const replay = await new ForkBatchCoordinator({
      database,
      createChild,
      retryChild: async () => forkResult('unused'),
      startSession: async () => undefined,
      waitUntilReady: async () => undefined,
      sendPrompt: async () => undefined,
      now: () => 124
    }).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'run-exact-replay' },
      retryItemKeys: ['A']
    })

    expect(replay).toEqual(first)
    await expect(coordinator.retryFailures({
      ...input, retryItemKeys: ['A', 'B']
    })).rejects.toThrow('仅可重试上一轮失败的项目：A')
    await expect(coordinator.retryFailures({
      ...input, retryItemKeys: ['B', 'A']
    })).rejects.toThrow('仅可重试上一轮失败的项目：A')
    expect(createChild).toHaveBeenCalledTimes(3)
  })

  it('resumes a pre-create retry interrupted after durable authorization', async () => {
    const neverReturns = new Promise<ForkWorkflowResult>(() => undefined)
    const createChild = vi.fn<CreateChild>()
      .mockRejectedValueOnce(new Error('pre-create failure'))
      .mockImplementationOnce(() => neverReturns)
    const dependencies = {
      database,
      createChild,
      retryChild: vi.fn<RetryChild>(async () => forkResult('unused-retry')),
      startSession: vi.fn(async () => undefined),
      waitUntilReady: vi.fn(async () => undefined),
      sendPrompt: vi.fn(async () => undefined),
      now: () => 123
    }
    const input = batchFixture([{
      itemKey: 'pre-create', title: '创建前中断', environment: current
    }], 'pre-create-interruption')
    await new ForkBatchCoordinator(dependencies).createChildren(input)

    const interrupted = new ForkBatchCoordinator(dependencies).retryFailures({
      ...input, retryItemKeys: ['pre-create']
    })
    void interrupted.catch(() => undefined)
    await vi.waitFor(() => expect(createChild).toHaveBeenCalledTimes(2))
    expect(database.get(
      `SELECT retry.state
       FROM fork_batch_retry_items AS retry
       JOIN fork_batch_retry_attempts AS attempt ON attempt.attempt_id = retry.attempt_id
       WHERE attempt.batch_key = ? AND retry.item_key = ?`,
      input.batchKey, 'pre-create'
    )).toEqual({ state: 'executing' })

    await restartDatabase()
    const resumedCreate = vi.fn<CreateChild>(async () => forkResult('session-pre-create'))
    const replay = await new ForkBatchCoordinator({
      ...dependencies, database, createChild: resumedCreate
    }).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'run-after-pre-create-interruption' },
      retryItemKeys: ['pre-create']
    })

    expect(resumedCreate).toHaveBeenCalledTimes(1)
    expect(replay.items[0]).toMatchObject({
      state: 'ready', sessionRef: 'session:session-pre-create'
    })
  })

  it('recovers a persistently accepted create when the retry receipt was interrupted', async () => {
    const real = await realForkFixture()
    const neverReturns = new Promise<ForkWorkflowResult>(() => undefined)
    const createChild = vi.fn<CreateChild>()
      .mockRejectedValueOnce(new Error('pre-create failure'))
      .mockImplementationOnce(async (createCommand, createInput) => {
        await real.workflow.createForkChild(createCommand, createInput)
        return neverReturns
      })
    const dependencies = {
      database,
      createChild,
      retryChild: vi.fn<RetryChild>((retryCommand, retryInput) => (
        real.workflow.retryFork(retryCommand, retryInput)
      )),
      startSession: vi.fn(async () => undefined),
      waitUntilReady: vi.fn(async () => undefined),
      sendPrompt: vi.fn(async () => undefined),
      now: () => 123
    }
    const input = realBatchInput(real.source, {
      itemKey: 'accepted-create', title: '已受理创建', environment: real.environment
    }, 'accepted-create-interruption')
    await new ForkBatchCoordinator(dependencies).createChildren(input)

    const interrupted = new ForkBatchCoordinator(dependencies).retryFailures({
      ...input, retryItemKeys: ['accepted-create']
    })
    void interrupted.catch(() => undefined)
    await vi.waitFor(() => expect(createChild).toHaveBeenCalledTimes(2))
    const sessionsAfterAcceptance = database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM sessions'
    )!.count

    await restartDatabase()
    const restartedWorkflow = new ForkWorkflowService(
      dataRoot,
      database,
      new DomainTransactionManager(database),
      { stopRuns: async () => undefined }
    )
    const resumedCreate = vi.fn<CreateChild>(
      (createCommand, createInput) => restartedWorkflow.createForkChild(createCommand, createInput)
    )
    const replay = await new ForkBatchCoordinator({
      ...dependencies,
      database,
      createChild: resumedCreate,
      retryChild: (retryCommand, retryInput) => restartedWorkflow.retryFork(
        retryCommand, retryInput
      )
    }).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'run-after-accepted-create' },
      retryItemKeys: ['accepted-create']
    })

    expect(resumedCreate).not.toHaveBeenCalled()
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM sessions')!.count)
      .toBe(sessionsAfterAcceptance)
    expect(replay.items[0]).toMatchObject({ state: 'created' })
  })

  it('continues the remaining item after a multi-item retry is interrupted', async () => {
    const neverReturns = new Promise<ForkWorkflowResult>(() => undefined)
    const createChild = vi.fn<CreateChild>()
      .mockRejectedValueOnce(new Error('failure A'))
      .mockRejectedValueOnce(new Error('failure B'))
      .mockResolvedValueOnce(forkResult('session-retry-A'))
      .mockImplementationOnce(() => neverReturns)
    const startSession = vi.fn(async () => undefined)
    const sendPrompt = vi.fn(async () => undefined)
    const dependencies = {
      database,
      createChild,
      retryChild: vi.fn<RetryChild>(async () => forkResult('unused-retry')),
      startSession,
      waitUntilReady: vi.fn(async () => undefined),
      sendPrompt,
      now: () => 123
    }
    const input = batchFixture([{
      itemKey: 'A', title: '重试 A', environment: current, start: true, prompt: '执行 A'
    }, {
      itemKey: 'B', title: '重试 B', environment: current, start: true, prompt: '执行 B'
    }], 'multi-retry-interruption')
    await new ForkBatchCoordinator(dependencies).createChildren(input)

    const interrupted = new ForkBatchCoordinator(dependencies).retryFailures({
      ...input, retryItemKeys: ['A', 'B']
    })
    void interrupted.catch(() => undefined)
    await vi.waitFor(() => expect(createChild).toHaveBeenCalledTimes(4))
    expect(database.all(
      `SELECT retry.item_key, retry.state
       FROM fork_batch_retry_items AS retry
       JOIN fork_batch_retry_attempts AS attempt ON attempt.attempt_id = retry.attempt_id
       WHERE attempt.batch_key = ? ORDER BY retry.ordinal`,
      input.batchKey
    )).toEqual([
      { item_key: 'A', state: 'completed' },
      { item_key: 'B', state: 'executing' }
    ])
    expect(startSession).toHaveBeenCalledTimes(1)
    expect(sendPrompt).toHaveBeenCalledTimes(1)

    await restartDatabase()
    const resumedCreate = vi.fn<CreateChild>(async () => forkResult('session-retry-B'))
    const resumedStart = vi.fn(async () => undefined)
    const resumedSend = vi.fn(async () => undefined)
    const replay = await new ForkBatchCoordinator({
      ...dependencies,
      database,
      createChild: resumedCreate,
      startSession: resumedStart,
      sendPrompt: resumedSend
    }).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'run-after-multi-interruption' },
      retryItemKeys: ['A', 'B']
    })

    expect(resumedCreate).toHaveBeenCalledTimes(1)
    expect(resumedStart).toHaveBeenCalledTimes(1)
    expect(resumedSend).toHaveBeenCalledTimes(1)
    expect(replay.items.map(({ itemKey, state }) => [itemKey, state])).toEqual([
      ['A', 'started'], ['B', 'started']
    ])
  })

  it('settles an interrupted multi-item retry when one accepted item fails before replay', async () => {
    const real = await realForkFixture()
    const createChild = vi.fn<CreateChild>(
      (createCommand, createInput) => real.workflow.createForkChild(createCommand, createInput)
    )
    const initialRetry = vi.fn<RetryChild>(
      (retryCommand, retryInput) => real.workflow.retryFork(retryCommand, retryInput)
    )
    const dependencies = {
      database,
      createChild,
      retryChild: initialRetry,
      startSession: vi.fn(async () => undefined),
      waitUntilReady: vi.fn(async () => undefined),
      sendPrompt: vi.fn(async () => undefined),
      now: () => 123
    }
    const input: CreateForkBatchInput = {
      ...realBatchInput(real.source, {
        itemKey: 'A', title: '组合重试 A', environment: real.environment
      }, 'retry-fails-before-replay'),
      items: [{
        itemKey: 'A', title: '组合重试 A', environment: real.environment
      }, {
        itemKey: 'B', title: '组合重试 B', environment: real.environment
      }]
    }
    const initial = await new ForkBatchCoordinator(dependencies).createChildren(input)
    const sessionB = initial.items[1]!.sessionRef!.slice('session:'.length)
    const intents = new SessionForkIntentRepository(database)
    failForkIntent(intents, input.batchKey, 'A', 'initial-failure-A', 124, 'failure A')
    failForkIntent(intents, input.batchKey, 'B', 'initial-failure-B', 124, 'failure B')

    const neverReturns = new Promise<ForkWorkflowResult>(() => undefined)
    const acceptedWithoutReceipt = vi.fn<RetryChild>(async (retryCommand, retryInput) => {
      const accepted = await real.workflow.retryFork(retryCommand, retryInput)
      return retryInput.sessionId === sessionB ? neverReturns : accepted
    })
    const interrupted = new ForkBatchCoordinator({
      ...dependencies, retryChild: acceptedWithoutReceipt
    }).retryFailures({ ...input, retryItemKeys: ['A', 'B'] })
    void interrupted.catch(() => undefined)
    await vi.waitFor(() => expect(acceptedWithoutReceipt).toHaveBeenCalledTimes(2))
    expect(database.all(
      `SELECT retry.item_key, retry.state
       FROM fork_batch_retry_items AS retry
       JOIN fork_batch_retry_attempts AS attempt ON attempt.attempt_id = retry.attempt_id
       WHERE attempt.batch_key = ? ORDER BY retry.ordinal`,
      input.batchKey
    )).toEqual([
      { item_key: 'A', state: 'completed' },
      { item_key: 'B', state: 'executing' }
    ])
    failForkIntent(
      intents, input.batchKey, 'B', 'accepted-failure-B', 130, 'accepted retry B failed'
    )

    await restartDatabase()
    const restartedWorkflow = new ForkWorkflowService(
      dataRoot,
      database,
      new DomainTransactionManager(database),
      { stopRuns: async () => undefined }
    )
    const restartedCreate = vi.fn<CreateChild>(
      (createCommand, createInput) => restartedWorkflow.createForkChild(createCommand, createInput)
    )
    const restartedRetry = vi.fn<RetryChild>(
      (retryCommand, retryInput) => restartedWorkflow.retryFork(retryCommand, retryInput)
    )
    const restartedDependencies = {
      ...dependencies,
      database,
      createChild: restartedCreate,
      retryChild: restartedRetry,
      now: () => 140
    }
    const replay = await new ForkBatchCoordinator(restartedDependencies).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'replay-original-multi-retry' },
      retryItemKeys: ['A', 'B']
    })

    expect(replay.items.map(({ itemKey, state }) => [itemKey, state])).toEqual([
      ['A', 'created'], ['B', 'failed']
    ])
    expect(restartedCreate).not.toHaveBeenCalled()
    expect(restartedRetry).not.toHaveBeenCalled()
    expect(restartedDependencies.startSession).not.toHaveBeenCalled()
    expect(restartedDependencies.sendPrompt).not.toHaveBeenCalled()
    expect(database.all(
      `SELECT retry.item_key, retry.state, retry.failure_generation,
              retry.result_failure_generation
       FROM fork_batch_retry_items AS retry
       JOIN fork_batch_retry_attempts AS attempt ON attempt.attempt_id = retry.attempt_id
       WHERE attempt.batch_key = ? ORDER BY retry.ordinal`,
      input.batchKey
    )).toEqual([
      {
        item_key: 'A', state: 'completed', failure_generation: 1,
        result_failure_generation: null
      },
      {
        item_key: 'B', state: 'failed', failure_generation: 1,
        result_failure_generation: 2
      }
    ])
    expect(database.get(
      `SELECT state, replay_pending FROM fork_batch_retry_attempts
       WHERE batch_key = ? AND retry_keys_json = '["A","B"]'`,
      input.batchKey
    )).toEqual({ state: 'completed', replay_pending: 0 })

    const nextGeneration = await new ForkBatchCoordinator(restartedDependencies).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'retry-next-B-generation' },
      retryItemKeys: ['B']
    })

    expect(restartedRetry).toHaveBeenCalledTimes(1)
    expect(restartedRetry.mock.calls[0]![1].sessionId).toBe(sessionB)
    expect(nextGeneration.items[1]).toMatchObject({
      itemKey: 'B', state: 'created', sessionRef: `session:${sessionB}`
    })
  })

  it('consumes an interrupted single-item replay before authorizing its next failure generation', async () => {
    const real = await realForkFixture()
    const createChild = vi.fn<CreateChild>(
      (createCommand, createInput) => real.workflow.createForkChild(createCommand, createInput)
    )
    const dependencies = {
      database,
      createChild,
      retryChild: vi.fn<RetryChild>(
        (retryCommand, retryInput) => real.workflow.retryFork(retryCommand, retryInput)
      ),
      startSession: vi.fn(async () => undefined),
      waitUntilReady: vi.fn(async () => undefined),
      sendPrompt: vi.fn(async () => undefined),
      now: () => 123
    }
    const input = realBatchInput(real.source, {
      itemKey: 'single', title: '单项重试', environment: real.environment
    }, 'single-retry-fails-before-replay')
    const initial = await new ForkBatchCoordinator(dependencies).createChildren(input)
    const sessionId = initial.items[0]!.sessionRef!.slice('session:'.length)
    const intents = new SessionForkIntentRepository(database)
    failForkIntent(intents, input.batchKey, 'single', 'initial-single-failure', 124, 'failure one')

    const neverReturns = new Promise<ForkWorkflowResult>(() => undefined)
    const acceptedWithoutReceipt = vi.fn<RetryChild>(async (retryCommand, retryInput) => {
      await real.workflow.retryFork(retryCommand, retryInput)
      return neverReturns
    })
    const interrupted = new ForkBatchCoordinator({
      ...dependencies, retryChild: acceptedWithoutReceipt
    }).retryFailures({ ...input, retryItemKeys: ['single'] })
    void interrupted.catch(() => undefined)
    await vi.waitFor(() => expect(acceptedWithoutReceipt).toHaveBeenCalledTimes(1))
    failForkIntent(
      intents, input.batchKey, 'single', 'accepted-single-failure', 130, 'failure two'
    )

    await restartDatabase()
    const restartedWorkflow = new ForkWorkflowService(
      dataRoot,
      database,
      new DomainTransactionManager(database),
      { stopRuns: async () => undefined }
    )
    const restartedRetry = vi.fn<RetryChild>(
      (retryCommand, retryInput) => restartedWorkflow.retryFork(retryCommand, retryInput)
    )
    const restartedDependencies = {
      ...dependencies, database, retryChild: restartedRetry, now: () => 140
    }

    const replay = await new ForkBatchCoordinator(restartedDependencies).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'replay-original-single-retry' },
      retryItemKeys: ['single']
    })
    expect(replay.items[0]).toMatchObject({
      itemKey: 'single', state: 'failed', sessionRef: `session:${sessionId}`
    })
    expect(restartedRetry).not.toHaveBeenCalled()
    expect(database.get(
      `SELECT retry.state, retry.failure_generation, retry.result_failure_generation,
              attempt.state AS attempt_state, attempt.replay_pending
       FROM fork_batch_retry_items AS retry
       JOIN fork_batch_retry_attempts AS attempt ON attempt.attempt_id = retry.attempt_id
       WHERE attempt.batch_key = ? AND retry.item_key = ?`,
      input.batchKey, 'single'
    )).toEqual({
      state: 'failed', failure_generation: 1, result_failure_generation: 2,
      attempt_state: 'completed', replay_pending: 0
    })

    const nextGeneration = await new ForkBatchCoordinator(restartedDependencies).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'retry-next-single-generation' },
      retryItemKeys: ['single']
    })
    expect(restartedRetry).toHaveBeenCalledTimes(1)
    expect(nextGeneration.items[0]).toMatchObject({
      itemKey: 'single', state: 'created', sessionRef: `session:${sessionId}`
    })
    expect(database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM fork_batch_retry_attempts WHERE batch_key = ?',
      input.batchKey
    )).toEqual({ count: 2 })
  })

  it('refreshes an asynchronously failed durable Fork and retries its existing Session after restart', async () => {
    const real = await realForkFixture()
    const createChild = vi.fn<CreateChild>((command, input) => real.workflow.createForkChild(command, input))
    const retryChild = vi.fn<RetryChild>((command, input) => real.workflow.retryFork(command, input))
    const dependencies = {
      database,
      createChild,
      retryChild,
      startSession: vi.fn(async () => undefined),
      waitUntilReady: vi.fn(async () => undefined),
      sendPrompt: vi.fn(async () => undefined),
      now: () => 123
    }
    const input = realBatchInput(real.source, {
      itemKey: 'async-failure', title: '异步失败方案', environment: real.environment
    }, 'async-failure-batch')
    const initial = await new ForkBatchCoordinator(dependencies).createChildren(input)
    const sessionId = initial.items[0]!.sessionRef!.slice('session:'.length)
    const intents = new SessionForkIntentRepository(database)
    const operation = intents.findBySubmissionKey(itemKey(input.batchKey, 'async-failure'))!
    const lease = intents.acquireLease({
      operationId: operation.identity.operationId,
      owner: 'test-failure', now: 124, ttlMs: 1_000
    })
    if (lease.kind !== 'acquired') throw new Error('test Fork lease was not acquired')
    expect(intents.failOperation({
      operationId: operation.identity.operationId,
      lease: lease.lease,
      error: 'provider failed asynchronously',
      now: 125
    }).kind).toBe('applied')

    const retried = await new ForkBatchCoordinator(dependencies).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'run-after-restart' },
      retryItemKeys: ['async-failure']
    })

    expect(retryChild).toHaveBeenCalledTimes(1)
    expect(retryChild.mock.calls[0]![1].sessionId).toBe(sessionId)
    expect(createChild).toHaveBeenCalledTimes(1)
    expect(retried.items[0]).toMatchObject({
      state: 'created', sessionRef: `session:${sessionId}`
    })
    expect(database.get(
      'SELECT state, stage, attempt FROM session_fork_intents WHERE session_id = ?', sessionId
    )).toEqual({ state: 'starting', stage: 'restoring-provider', attempt: 1 })
  })

  it('creates a new retry attempt only after the authoritative failure generation advances', async () => {
    const real = await realForkFixture()
    const createChild = vi.fn<CreateChild>((command, input) => real.workflow.createForkChild(command, input))
    const retryChild = vi.fn<RetryChild>((command, input) => real.workflow.retryFork(command, input))
    const dependencies = {
      database,
      createChild,
      retryChild,
      startSession: vi.fn(async () => undefined),
      waitUntilReady: vi.fn(async () => undefined),
      sendPrompt: vi.fn(async () => undefined),
      now: () => 123
    }
    const input = realBatchInput(real.source, {
      itemKey: 'generation', title: '失败代次方案', environment: real.environment
    }, 'failure-generation-batch')
    const initial = await new ForkBatchCoordinator(dependencies).createChildren(input)
    const sessionId = initial.items[0]!.sessionRef!.slice('session:'.length)
    const intents = new SessionForkIntentRepository(database)
    const operation = intents.findBySubmissionKey(itemKey(input.batchKey, 'generation'))!
    const fail = (owner: string, now: number, error: string) => {
      const lease = intents.acquireLease({
        operationId: operation.identity.operationId, owner, now, ttlMs: 1_000
      })
      if (lease.kind !== 'acquired') throw new Error('test Fork lease was not acquired')
      expect(intents.failOperation({
        operationId: operation.identity.operationId,
        lease: lease.lease,
        error,
        now: now + 1
      }).kind).toBe('applied')
    }
    fail('generation-one', 124, 'failure generation one')

    await new ForkBatchCoordinator(dependencies).retryFailures({
      ...input, retryItemKeys: ['generation']
    })
    expect(retryChild).toHaveBeenCalledTimes(1)
    fail('generation-two', 130, 'failure generation two')

    await new ForkBatchCoordinator(dependencies).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'run-generation-two' },
      retryItemKeys: ['generation']
    })

    expect(retryChild).toHaveBeenCalledTimes(2)
    expect(database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM fork_batch_retry_attempts WHERE batch_key = ?',
      input.batchKey
    )).toEqual({ count: 2 })
    expect(database.get(
      'SELECT failure_generation FROM fork_batch_items WHERE batch_key = ? AND item_key = ?',
      input.batchKey, 'generation'
    )).toEqual({ failure_generation: 2 })
  })

  it('resumes a durably accepted retry after restart without retrying the Session twice', async () => {
    const real = await realForkFixture()
    const createChild = vi.fn<CreateChild>((command, input) => real.workflow.createForkChild(command, input))
    const retryChild = vi.fn<RetryChild>((command, input) => real.workflow.retryFork(command, input))
    const dependencies = {
      database,
      createChild,
      retryChild,
      startSession: vi.fn(async () => undefined),
      waitUntilReady: vi.fn(async () => undefined),
      sendPrompt: vi.fn(async () => undefined),
      now: () => 123
    }
    const input = realBatchInput(real.source, {
      itemKey: 'accepted-retry', title: '已受理重试', environment: real.environment
    }, 'accepted-retry-batch')
    const initial = await new ForkBatchCoordinator(dependencies).createChildren(input)
    const sessionId = initial.items[0]!.sessionRef!.slice('session:'.length)
    const intents = new SessionForkIntentRepository(database)
    const operation = intents.findBySubmissionKey(itemKey(input.batchKey, 'accepted-retry'))!
    const lease = intents.acquireLease({
      operationId: operation.identity.operationId,
      owner: 'test-accepted-retry', now: 124, ttlMs: 1_000
    })
    if (lease.kind !== 'acquired') throw new Error('test Fork lease was not acquired')
    expect(intents.failOperation({
      operationId: operation.identity.operationId,
      lease: lease.lease,
      error: 'failed before durable retry',
      now: 125
    }).kind).toBe('applied')
    const observedFailure = await new ForkBatchCoordinator(dependencies).createChildren({
      ...input, caller: { ...input.caller, runId: 'run-observing-failure' }
    })
    expect(observedFailure.items[0]).toMatchObject({ state: 'failed' })
    const neverReturns = new Promise<ForkWorkflowResult>(() => undefined)
    const acceptedWithoutReceipt = vi.fn<RetryChild>(async (retryCommand, retryInput) => {
      await real.workflow.retryFork(retryCommand, retryInput)
      return neverReturns
    })
    const interrupted = new ForkBatchCoordinator({
      ...dependencies, retryChild: acceptedWithoutReceipt
    }).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'run-accepting-retry' },
      retryItemKeys: ['accepted-retry']
    })
    void interrupted.catch(() => undefined)
    await vi.waitFor(() => expect(acceptedWithoutReceipt).toHaveBeenCalledTimes(1))
    expect(database.get(
      `SELECT retry.state
       FROM fork_batch_retry_items AS retry
       JOIN fork_batch_retry_attempts AS attempt ON attempt.attempt_id = retry.attempt_id
       WHERE attempt.batch_key = ? AND retry.item_key = ?`,
      input.batchKey, 'accepted-retry'
    )).toEqual({ state: 'executing' })

    await restartDatabase()
    const restartedWorkflow = new ForkWorkflowService(
      dataRoot,
      database,
      new DomainTransactionManager(database),
      { stopRuns: async () => undefined }
    )
    const restartedRetry = vi.fn<RetryChild>(
      (retryCommand, retryInput) => restartedWorkflow.retryFork(retryCommand, retryInput)
    )
    const replay = await new ForkBatchCoordinator({
      ...dependencies, database, retryChild: restartedRetry
    }).retryFailures({
      ...input,
      caller: { ...input.caller, runId: 'run-after-accepted-retry' },
      retryItemKeys: ['accepted-retry']
    })

    expect(retryChild).not.toHaveBeenCalled()
    expect(restartedRetry).not.toHaveBeenCalled()
    expect(createChild).toHaveBeenCalledTimes(1)
    expect(replay.items[0]).toMatchObject({
      state: 'created', sessionRef: `session:${sessionId}`
    })
    expect(database.get(
      'SELECT attempt FROM session_fork_intents WHERE session_id = ?', sessionId
    )).toEqual({ attempt: 1 })
  })

  it('coalesces concurrent retries into one create, start, and prompt delivery', async () => {
    let releaseCreate!: () => void
    const createMayFinish = new Promise<void>((resolve) => { releaseCreate = resolve })
    const ready = new ProviderReadyRegistry()
    const { coordinator, createChild, startSession, sendPrompt } = coordinatorFixture({
      startSession: async (sessionId) => {
        ready.record(sessionId, 'run-retry', 'run-retry')
      },
      waitUntilReady: (sessionId, signal) => ready.wait(sessionId, 1_000, signal)
    })
    createChild.mockRejectedValueOnce(new Error('pre-create failure'))
      .mockImplementationOnce(async () => {
        await createMayFinish
        return forkResult('session-retried')
      })
    const input = batchFixture([{
      itemKey: 'failed', title: '失败后重试', environment: current,
      start: true, prompt: '执行重试任务'
    }], 'concurrent-retry')
    await coordinator.createChildren(input)

    const first = coordinator.retryFailures({ ...input, retryItemKeys: ['failed'] })
    const second = coordinator.retryFailures({ ...input, retryItemKeys: ['failed'] })
    await vi.waitFor(() => expect(createChild).toHaveBeenCalledTimes(2))
    releaseCreate()

    await expect(Promise.all([first, second])).resolves.toEqual([
      expect.objectContaining({ failed: 0 }),
      expect.objectContaining({ failed: 0 })
    ])
    expect(createChild).toHaveBeenCalledTimes(2)
    expect(startSession).toHaveBeenCalledTimes(1)
    expect(sendPrompt).toHaveBeenCalledTimes(1)
  })

  it('shares the single retry executor across coordinator instances for one Runtime database', async () => {
    let releaseCreate!: () => void
    const createMayFinish = new Promise<void>((resolve) => { releaseCreate = resolve })
    const ready = new ProviderReadyRegistry()
    const createChild = vi.fn<CreateChild>()
      .mockRejectedValueOnce(new Error('pre-create failure'))
      .mockImplementationOnce(async () => {
        await createMayFinish
        return forkResult('session-shared-retry')
      })
    const retryChild = vi.fn<RetryChild>(async () => forkResult('unused-retry'))
    const startSession = vi.fn(async (sessionId: string) => {
      ready.record(sessionId, 'run-shared-retry', 'run-shared-retry')
    })
    const waitUntilReady = vi.fn(
      (sessionId: string, signal?: AbortSignal) => ready.wait(sessionId, 1_000, signal)
    )
    const sendPrompt = vi.fn(async () => undefined)
    const dependencies = {
      database, createChild, retryChild, startSession, waitUntilReady, sendPrompt, now: () => 123
    }
    const firstCoordinator = new ForkBatchCoordinator(dependencies)
    const secondCoordinator = new ForkBatchCoordinator(dependencies)
    const input = batchFixture([{
      itemKey: 'failed', title: '共享重试', environment: current,
      start: true, prompt: '执行共享重试任务'
    }], 'shared-concurrent-retry')
    await firstCoordinator.createChildren(input)

    const first = firstCoordinator.retryFailures({ ...input, retryItemKeys: ['failed'] })
    const second = secondCoordinator.retryFailures({ ...input, retryItemKeys: ['failed'] })
    await vi.waitFor(() => expect(createChild).toHaveBeenCalledTimes(2))
    releaseCreate()

    await expect(Promise.all([first, second])).resolves.toEqual([
      expect.objectContaining({ failed: 0 }),
      expect.objectContaining({ failed: 0 })
    ])
    expect(createChild).toHaveBeenCalledTimes(2)
    expect(retryChild).not.toHaveBeenCalled()
    expect(startSession).toHaveBeenCalledTimes(1)
    expect(sendPrompt).toHaveBeenCalledTimes(1)
  })

  it('rejects retry keys whose prior state is ready, created, or starting', async () => {
    for (const forkState of ['succeeded', 'pending', 'starting'] as const) {
      const { coordinator, createChild } = coordinatorFixture({
        createChild: async () => forkResult('session-existing', forkState)
      })
      const input = batchFixture([
        { itemKey: 'existing', title: '已存在节点', environment: current }
      ], `batch-${forkState}`)
      await coordinator.createChildren(input)

      await expect(coordinator.retryFailures({
        ...input, retryItemKeys: ['existing']
      })).rejects.toThrow('仅可重试上一轮失败的项目：existing')
      expect(createChild).toHaveBeenCalledTimes(1)
    }
  })

  it('follows parser membership rules for empty batches, duplicate items, and retry keys', async () => {
    const { coordinator, createChild } = coordinatorFixture()
    await expect(coordinator.createChildren(batchFixture([], 'empty-batch'))).resolves.toEqual({
      kind: 'fork-batch', batchKey: 'empty-batch', succeeded: 0, failed: 0, items: []
    })
    await expect(coordinator.createChildren(batchFixture([
      { itemKey: 'same', title: '方案一', environment: current },
      { itemKey: 'same', title: '方案二', environment: current }
    ], 'duplicate-items'))).rejects.toThrow('itemKey must be unique')

    const failedInput = batchFixture([
      { itemKey: 'failed', title: '失败方案', environment: current }
    ], 'retry-membership')
    createChild.mockRejectedValueOnce(new Error('branch collision'))
    await coordinator.createChildren(failedInput)
    const callsBeforeInvalidRetry = createChild.mock.calls.length
    await expect(coordinator.retryFailures({
      ...failedInput, retryItemKeys: ['failed', 'failed']
    })).rejects.toThrow('retry itemKey must be unique')
    await expect(coordinator.retryFailures({
      ...failedInput, retryItemKeys: ['unknown']
    })).rejects.toThrow('retry itemKey must belong to items')
    expect(createChild).toHaveBeenCalledTimes(callsBeforeInvalidRetry)
  })

  it.each([
    { label: 'empty retry set', itemKeys: [] },
    { label: 'nonempty retry set', itemKeys: ['failed'] }
  ])('returns a typed target fault when $label has no durable batch', async ({ itemKeys }) => {
    const { coordinator, createChild } = coordinatorFixture()
    const input = batchFixture(
      itemKeys.map((itemKey) => ({ itemKey, title: '待重试方案', environment: current })),
      `missing-${itemKeys.length}`
    )

    await expect(coordinator.retryFailures({
      ...input, retryItemKeys: itemKeys
    })).rejects.toMatchObject({
      code: 'TARGET_NOT_FOUND',
      message: `批次 ${input.batchKey} 没有可重试的上一轮结果`
    })
    expect(createChild).not.toHaveBeenCalled()
  })

  it('waits for a fresh matching provider identity before submitting an assigned task', async () => {
    const ready = new ProviderReadyRegistry()
    const waitUntilReady = vi.fn((sessionId: string) => ready.wait(sessionId, 1_000))
    const { coordinator, startSession, sendPrompt } = coordinatorFixture({ waitUntilReady })
    const input = batchFixture([{
      itemKey: 'two', title: '方案二', environment: current,
      start: true, prompt: '实现方案二'
    }])

    const pending = coordinator.createChildren(input)
    await vi.waitFor(() => expect(startSession).toHaveBeenCalledWith('created-方案二'))
    expect(waitUntilReady).toHaveBeenCalledBefore(startSession)
    expect(sendPrompt).not.toHaveBeenCalled()

    ready.record('another-session', 'run-other')
    await Promise.resolve()
    expect(sendPrompt).not.toHaveBeenCalled()
    ready.record('created-方案二', 'run-2')

    await expect(pending).resolves.toMatchObject({
      items: [{ itemKey: 'two', state: 'started', sessionRef: 'session:created-方案二' }]
    })
    expect(sendPrompt).toHaveBeenCalledWith('created-方案二', '实现方案二')
  })

  it('keeps a created node and reports a pending-start error when readiness times out', async () => {
    const { coordinator, startSession, sendPrompt } = coordinatorFixture({
      waitUntilReady: async () => { throw new Error('provider timeout') }
    })
    const result = await coordinator.createChildren(batchFixture([{
      itemKey: 'two', title: '方案二', environment: current,
      start: true, prompt: '实现方案二'
    }], 'timeout-batch'))

    expect(startSession).toHaveBeenCalledWith('created-方案二')
    expect(sendPrompt).not.toHaveBeenCalled()
    expect(result.items[0]).toMatchObject({
      itemKey: 'two', state: 'created', sessionRef: 'session:created-方案二'
    })
    expect(result.items[0]!.error).toContain('节点已创建，任务仍待启动')
  })

  it('does not erase a durable pending-start error when the Fork later succeeds', async () => {
    const real = await realForkFixture()
    const dependencies = {
      database,
      createChild: vi.fn<CreateChild>((command, input) => real.workflow.createForkChild(command, input)),
      retryChild: vi.fn<RetryChild>((command, input) => real.workflow.retryFork(command, input)),
      startSession: vi.fn(async () => undefined),
      waitUntilReady: vi.fn(async () => { throw new Error('provider timeout') }),
      sendPrompt: vi.fn(async () => undefined),
      now: () => 123
    }
    const input = realBatchInput(real.source, {
      itemKey: 'late-ready', title: '延迟就绪方案', environment: real.environment,
      start: true, prompt: '执行延迟就绪任务'
    }, 'late-ready-batch')
    const first = await new ForkBatchCoordinator(dependencies).createChildren(input)
    const sessionId = first.items[0]!.sessionRef!.slice('session:'.length)
    database.run(
      `UPDATE session_fork_intents
       SET state = 'succeeded', stage = 'succeeded', completed_steps = total_steps,
           completed_at = 130, updated_at = 130
       WHERE session_id = ?`,
      sessionId
    )

    const replay = await new ForkBatchCoordinator(dependencies).createChildren({
      ...input, caller: { ...input.caller, runId: 'run-after-late-ready' }
    })

    expect(replay.items[0]).toMatchObject({
      state: 'created', sessionRef: `session:${sessionId}`
    })
    expect(replay.items[0]!.error).toContain('节点已创建，任务仍待启动')
    expect(dependencies.startSession).toHaveBeenCalledTimes(1)
    expect(dependencies.sendPrompt).not.toHaveBeenCalled()
  })

  it('turns an interrupted delivery claim into a durable uncertain receipt without resending', async () => {
    const { coordinator, createChild, startSession, sendPrompt, waitUntilReady } = coordinatorFixture()
    const input = batchFixture([{
      itemKey: 'delivery', title: '投递方案', environment: current,
      start: true, prompt: '只投递一次'
    }], 'delivery-receipt-batch')
    await coordinator.createChildren(input)
    database.run(
      `UPDATE fork_batch_items
       SET state = 'created', start_state = 'delivering', error_message = NULL
       WHERE batch_key = ? AND item_key = ?`,
      input.batchKey, 'delivery'
    )

    const replay = await new ForkBatchCoordinator({
      database, createChild, retryChild: async () => forkResult('unused'),
      startSession, waitUntilReady, sendPrompt, now: () => 124
    }).createChildren({
      ...input, caller: { ...input.caller, runId: 'run-after-interruption' }
    })

    expect(replay.items[0]).toMatchObject({ state: 'created' })
    expect(replay.items[0]!.error).toContain('投递结果待确认')
    expect(startSession).toHaveBeenCalledTimes(1)
    expect(sendPrompt).toHaveBeenCalledTimes(1)
    expect(database.get(
      'SELECT start_state FROM fork_batch_items WHERE batch_key = ? AND item_key = ?',
      input.batchKey, 'delivery'
    )).toEqual({ start_state: 'uncertain' })
  })

  it('cancels the registered waiter when Session startup fails immediately', async () => {
    const ready = new ProviderReadyRegistry()
    const { coordinator } = coordinatorFixture({
      startSession: async () => { throw new Error('spawn failed') },
      waitUntilReady: (sessionId, signal) => ready.wait(sessionId, 60_000, signal)
    })

    const result = await coordinator.createChildren(batchFixture([{
      itemKey: 'cancelled', title: '启动失败方案', environment: current,
      start: true, prompt: '执行任务'
    }], 'cancelled-waiter'))

    expect(result.items[0]).toMatchObject({ state: 'created' })
    expect(ready.pendingWaiterCount).toBe(0)
  })
})

async function realForkFixture() {
  const workspaceRoot = join(dataRoot, 'workspace')
  await mkdir(workspaceRoot)
  const transactions = new DomainTransactionManager(database)
  const hierarchy = new HierarchyApplicationService(database, transactions)
  const initial = hierarchy.bootstrapWindow(command('bootstrap-real-fork'), {
    windowId: 'window-real', defaultRootDirectory: workspaceRoot,
    defaultName: 'workspace', now: 10
  })
  database.run(
    "UPDATE sessions SET kind = 'claude-code', title = 'Claude' WHERE id = ?",
    initial.session!.id
  )
  database.run(
    `INSERT INTO provider_bindings (
       id, session_id, provider, provider_session_id, resume_state, restore_state,
       metadata_json, created_at, updated_at, validated_at
     ) VALUES (?, ?, 'claude-code', ?, 'available', 'none', ?, 20, 20, 20)`,
    `binding-${initial.session!.id}`,
    initial.session!.id,
    `provider-${initial.session!.id}`,
    JSON.stringify({ canFork: true })
  )
  const workflow = new ForkWorkflowService(dataRoot, database, transactions, {
    stopRuns: async () => undefined
  })
  const realSource: ResolvedHostEntity & { kind: 'session' } = {
    kind: 'session',
    windowId: 'window-real',
    workspaceId: initial.workspace!.id,
    taskId: initial.task!.id,
    sceneId: initial.scene!.id,
    sessionId: initial.session!.id
  }
  const environment: ResolvedForkEnvironment = {
    mode: 'current', executionContextId: initial.executionContext!.id
  }
  return { workflow, source: realSource, environment }
}

function realBatchInput(
  realSource: ResolvedHostEntity & { kind: 'session' },
  item: CreateForkBatchInput['items'][number],
  batchKey: string
): CreateForkBatchInput {
  return {
    caller: { runId: 'run-real-parent', sessionId: realSource.sessionId },
    source: realSource,
    batchKey,
    items: [item]
  }
}

function command(commandId: string): DomainCommandMetadata {
  return { commandId, commandType: 'test', requestHash: `hash-${commandId}` }
}

function itemKey(batchKey: string, key: string): string {
  return createHash('sha256').update(`${batchKey}:${key}`).digest('hex')
}

function failForkIntent(
  intents: SessionForkIntentRepository,
  batchKey: string,
  key: string,
  owner: string,
  now: number,
  error: string
): void {
  const operation = intents.findBySubmissionKey(itemKey(batchKey, key))!
  const lease = intents.acquireLease({
    operationId: operation.identity.operationId, owner, now, ttlMs: 1_000
  })
  if (lease.kind !== 'acquired') throw new Error('test Fork lease was not acquired')
  expect(intents.failOperation({
    operationId: operation.identity.operationId,
    lease: lease.lease,
    error,
    now: now + 1
  }).kind).toBe('applied')
}
