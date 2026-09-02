import { describe, expect, it } from 'vitest'

import { RuntimeRecoveryCoordinator } from './runtime-recovery-coordinator'
import type { RecoveryJobSnapshot } from './runtime-session-recovery-scheduler'

async function settle(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
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
})
