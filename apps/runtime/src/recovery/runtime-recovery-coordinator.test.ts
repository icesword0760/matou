import { describe, expect, it } from 'vitest'

import { RuntimeRecoveryCoordinator } from './runtime-recovery-coordinator'
import type { RecoveryJobSnapshot } from './runtime-session-recovery-scheduler'

async function settle(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
}

function deferred(): { promise: Promise<void>; resolve(): void } {
  let resolve!: () => void
  const promise = new Promise<void>((done) => { resolve = done })
  return { promise, resolve }
}

describe('RuntimeRecoveryCoordinator', () => {
  it('publishes durable job identity and lets one failed card retry independently', async () => {
    const snapshots: Array<readonly RecoveryJobSnapshot[]> = []
    let attempts = 0
    const coordinator = new RuntimeRecoveryCoordinator({
      concurrency: 1,
      jobs: [{
        sessionId: 'session-a', sceneId: 'scene-a', priority: 'active-session', enqueueSequence: 1,
        workspaceId: 'workspace-a', taskId: 'task-a', executionContextId: 'context-a', profile: 'shell'
      }],
      start: async () => {
        attempts += 1
        if (attempts === 1) throw new Error('first start failed')
      },
      publish: (snapshot) => snapshots.push(snapshot)
    })

    coordinator.start()
    await coordinator.whenIdle()
    expect(coordinator.snapshot()).toEqual([
      expect.objectContaining({ sessionId: 'session-a', state: 'failed', error: 'first start failed' })
    ])

    coordinator.retry('session-a')
    await coordinator.whenIdle()
    expect(coordinator.snapshot()).toEqual([
      expect.objectContaining({ sessionId: 'session-a', state: 'ready' })
    ])
    expect(attempts).toBe(2)
    expect(snapshots.some((snapshot) => snapshot[0]?.state === 'restoring')).toBe(true)
  })

  it('does not restart a job after cancellation before startup', async () => {
    const started: string[] = []
    const coordinator = new RuntimeRecoveryCoordinator({
      concurrency: 1,
      jobs: [{
        sessionId: 'deleted', sceneId: 'scene-a', priority: 'active-session', enqueueSequence: 1
      }],
      start: async ({ sessionId }) => { started.push(sessionId) }
    })

    coordinator.cancel(['deleted'])
    coordinator.start()
    await settle()

    expect(started).toEqual([])
    expect(coordinator.snapshot()).toEqual([])
  })

  it('tombstones an actively restoring Session and ignores its late completion', async () => {
    const active = deferred()
    const snapshots: Array<readonly RecoveryJobSnapshot[]> = []
    const coordinator = new RuntimeRecoveryCoordinator({
      concurrency: 1,
      jobs: [{
        sessionId: 'removed-active', sceneId: 'scene-a',
        priority: 'active-session', enqueueSequence: 1
      }],
      start: () => active.promise,
      publish: (snapshot) => snapshots.push(snapshot)
    })
    coordinator.start()
    await settle()
    expect(coordinator.snapshot()).toContainEqual(expect.objectContaining({
      sessionId: 'removed-active', state: 'restoring'
    }))

    coordinator.cancel(['removed-active'])
    const cancelledAt = snapshots.length
    expect(coordinator.snapshot()).toEqual([])
    active.resolve()
    await coordinator.whenIdle()
    coordinator.trackExternal({
      sessionId: 'removed-active', sceneId: 'scene-a',
      priority: 'active-session', enqueueSequence: 2, recoveryAuthority: 'fork'
    })
    coordinator.settleExternal('removed-active', 'ready')

    expect(coordinator.snapshot()).toEqual([])
    expect(snapshots.slice(cancelledAt).every((snapshot) =>
      snapshot.every(({ sessionId }) => sessionId !== 'removed-active'))).toBe(true)
  })

  it('converges a durable Fork card from external restore to ready or failed', async () => {
    const coordinator = new RuntimeRecoveryCoordinator({
      concurrency: 4,
      jobs: [{
        sessionId: 'fork-child', sceneId: 'scene-a', priority: 'active-session',
        enqueueSequence: 1, recoveryAuthority: 'fork'
      }],
      start: async () => { throw new Error('generic recovery must not launch a durable Fork') }
    })

    coordinator.start()
    await coordinator.whenIdle()
    expect(coordinator.snapshot()).toEqual([
      expect.objectContaining({ sessionId: 'fork-child', state: 'restoring' })
    ])

    coordinator.settleExternal('fork-child', 'failed', 'fork setup failed')
    expect(coordinator.snapshot()).toEqual([
      expect.objectContaining({
        sessionId: 'fork-child', state: 'failed', error: 'fork setup failed'
      })
    ])
    coordinator.settleExternal('fork-child', 'ready')
    expect(coordinator.snapshot()).toEqual([
      expect.objectContaining({ sessionId: 'fork-child', state: 'ready' })
    ])
  })

  it('tracks a durable Fork created after startup before its headless provider launch', async () => {
    const coordinator = new RuntimeRecoveryCoordinator({
      concurrency: 4,
      jobs: [],
      start: async () => { throw new Error('external Fork must stay outside generic recovery') }
    })
    coordinator.start()

    coordinator.trackExternal({
      sessionId: 'new-fork', sceneId: 'scene-a', priority: 'active-session',
      enqueueSequence: 1, recoveryAuthority: 'fork'
    })
    await coordinator.whenIdle()

    expect(coordinator.snapshot()).toEqual([
      expect.objectContaining({ sessionId: 'new-fork', state: 'restoring' })
    ])
    coordinator.settleExternal('new-fork', 'ready')
    expect(coordinator.snapshot()).toEqual([
      expect.objectContaining({ sessionId: 'new-fork', state: 'ready' })
    ])
  })

  it('returns a failed durable Fork to restoring when its authoritative retry starts', () => {
    const coordinator = new RuntimeRecoveryCoordinator({
      concurrency: 4,
      jobs: [],
      start: async () => { throw new Error('external Fork must stay outside generic recovery') }
    })
    const external = {
      sessionId: 'retried-fork', sceneId: 'scene-a', priority: 'active-session' as const,
      enqueueSequence: 1, recoveryAuthority: 'fork' as const
    }
    coordinator.start()
    coordinator.trackExternal(external)
    coordinator.settleExternal('retried-fork', 'failed', 'first attempt failed')

    coordinator.trackExternal(external)

    expect(coordinator.snapshot()).toEqual([
      expect.objectContaining({ sessionId: 'retried-fork', state: 'restoring' })
    ])
  })
})
