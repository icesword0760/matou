import { createHash } from 'node:crypto'

import type { DomainCommandMetadata } from '@matou/domain'
import { describe, expect, it, vi } from 'vitest'

import type { ResolvedForkEnvironment, ResolvedHostEntity } from './host-action-target-resolver'
import type { HostCallerIdentity } from './host-control-types'
import { ForkBatchCoordinator, type CreateForkBatchInput } from './fork-batch-coordinator'
import { ProviderReadyRegistry } from './provider-ready-registry'
import type { CreateForkInput, ForkWorkflowResult } from '../session-canvas/fork-workflow-service'

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
  startSession: (sessionId: string) => Promise<void>
  waitUntilReady: (sessionId: string) => Promise<unknown>
  sendPrompt: (sessionId: string, prompt: string) => Promise<void>
}> = {}) {
  const createChild = vi.fn<CreateChild>(overrides.createChild ?? (async (_command, input) => (
    forkResult(`created-${input.name}`)
  )))
  const startSession = vi.fn(overrides.startSession ?? (async () => undefined))
  const waitUntilReady = vi.fn(overrides.waitUntilReady ?? (async () => undefined))
  const sendPrompt = vi.fn(overrides.sendPrompt ?? (async () => undefined))
  const coordinator = new ForkBatchCoordinator({
    createChild, startSession, waitUntilReady, sendPrompt, now: () => 123
  })
  return { coordinator, createChild, startSession, waitUntilReady, sendPrompt }
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
})
