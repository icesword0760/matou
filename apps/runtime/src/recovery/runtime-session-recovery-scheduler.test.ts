import { describe, expect, it } from 'vitest'

import {
  RuntimeSessionRecoveryScheduler,
  type RecoveryJob
} from './runtime-session-recovery-scheduler'

function job(
  sessionId: string,
  sceneId: string,
  priority: RecoveryJob['priority'],
  enqueueSequence: number
): RecoveryJob {
  return { sessionId, sceneId, priority, enqueueSequence }
}

function deferred(): {
  promise: Promise<void>
  resolve(): void
  reject(error: Error): void
} {
  let resolve!: () => void
  let reject!: (error: Error) => void
  const promise = new Promise<void>((onResolve, onReject) => {
    resolve = onResolve
    reject = onReject
  })
  return { promise, resolve, reject }
}

async function settle(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
}

describe('RuntimeSessionRecoveryScheduler', () => {
  it('starts the current Session first and never exceeds the concurrency limit', async () => {
    const pending = new Map<string, ReturnType<typeof deferred>>()
    const started: string[] = []
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 2,
      start: (candidate) => {
        started.push(candidate.sessionId)
        const gate = deferred()
        pending.set(candidate.sessionId, gate)
        return gate.promise
      }
    })

    scheduler.enqueue([
      job('background-1', 'scene-b', 'background', 1),
      job('foreground-2', 'scene-a', 'foreground-scene', 2),
      job('current', 'scene-a', 'active-session', 3),
      job('foreground-1', 'scene-a', 'foreground-scene', 1)
    ])
    scheduler.prioritizeScene('scene-a', 'current', [
      'current', 'foreground-1', 'foreground-2'
    ])
    await settle()

    expect(started).toEqual(['current', 'foreground-1'])
    expect(scheduler.runningCount).toBe(2)

    pending.get('foreground-1')!.resolve()
    await settle()
    expect(started).toEqual(['current', 'foreground-1', 'foreground-2'])
    expect(scheduler.runningCount).toBe(2)
  })

  it('treats only the current DAG sibling list as foreground inside one Scene', async () => {
    const first = deferred()
    const started: string[] = []
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 1,
      start: (candidate) => {
        started.push(candidate.sessionId)
        return candidate.sessionId === 'blocker' ? first.promise : Promise.resolve()
      }
    })
    scheduler.enqueue([
      job('blocker', 'scene-other', 'active-session', 0),
      job('same-scene-other-level', 'scene-a', 'foreground-scene', 1),
      job('offscreen-sibling', 'scene-a', 'active-task', 2),
      job('current', 'scene-a', 'active-task', 3)
    ])
    await settle()

    scheduler.prioritizeScene('scene-a', 'current', ['current', 'offscreen-sibling'])
    first.resolve()
    await scheduler.whenIdle()

    expect(started).toEqual([
      'blocker', 'current', 'offscreen-sibling', 'same-scene-other-level'
    ])
  })

  it('holds fork-authority recovery outside the four Runtime launch slots until it settles', async () => {
    const started: string[] = []
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 1,
      start: async (candidate) => { started.push(candidate.sessionId) }
    })
    scheduler.enqueue([
      { ...job('durable-fork', 'scene-a', 'active-session', 1), recoveryAuthority: 'fork' },
      job('ordinary', 'scene-a', 'foreground-scene', 2)
    ])

    await scheduler.whenIdle()
    expect(started).toEqual(['ordinary'])
    expect(scheduler.snapshot()).toContainEqual(expect.objectContaining({
      sessionId: 'durable-fork', state: 'restoring', recoveryAuthority: 'fork'
    }))

    scheduler.settleExternal('durable-fork', 'ready')
    expect(scheduler.snapshot()).toContainEqual(expect.objectContaining({
      sessionId: 'durable-fork', state: 'ready'
    }))
  })

  it('cancels an externally restoring Fork card after the Session is removed', async () => {
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 4,
      start: async () => undefined
    })
    scheduler.enqueue([{
      ...job('removed-fork', 'scene-a', 'active-session', 1), recoveryAuthority: 'fork'
    }])

    scheduler.cancel(['removed-fork'])

    expect(scheduler.snapshot()).toEqual([])
  })

  it('reprioritizes queued work when the user switches Scene while running work stays isolated', async () => {
    const pending = new Map<string, ReturnType<typeof deferred>>()
    const started: string[] = []
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 1,
      start: (candidate) => {
        started.push(candidate.sessionId)
        const gate = deferred()
        pending.set(candidate.sessionId, gate)
        return gate.promise
      }
    })

    scheduler.enqueue([
      job('a-1', 'scene-a', 'foreground-scene', 1),
      job('a-2', 'scene-a', 'foreground-scene', 2),
      job('b-1', 'scene-b', 'background', 3)
    ])
    scheduler.prioritizeScene('scene-a', 'a-1', ['a-1', 'a-2'])
    await settle()
    expect(started).toEqual(['a-1'])

    scheduler.prioritizeScene('scene-b', 'b-1', ['b-1'])
    pending.get('a-1')!.resolve()
    await settle()

    expect(started).toEqual(['a-1', 'b-1'])
  })

  it('downgrades the previous current Session after a foreground Scene switch', async () => {
    const first = deferred()
    const started: string[] = []
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 1,
      start: (candidate) => {
        started.push(candidate.sessionId)
        return candidate.sessionId === 'blocker' ? first.promise : Promise.resolve()
      }
    })
    scheduler.enqueue([
      job('blocker', 'scene-c', 'active-session', 0),
      job('previous-current', 'scene-a', 'active-session', 1),
      job('current-sibling', 'scene-b', 'background', 2)
    ])
    await settle()

    scheduler.prioritizeScene('scene-b', 'current-sibling', ['current-sibling'])
    first.resolve()
    await scheduler.whenIdle()

    expect(started).toEqual(['blocker', 'current-sibling', 'previous-current'])
  })

  it('runs background work after at most eight foreground starts', async () => {
    const started: string[] = []
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 1,
      start: async (candidate) => { started.push(candidate.sessionId) }
    })
    scheduler.enqueue([
      ...Array.from({ length: 10 }, (_, index) =>
        job(`foreground-${index + 1}`, 'scene-a', 'foreground-scene', index + 1)),
      job('background', 'scene-b', 'background', 20)
    ])
    scheduler.prioritizeScene('scene-a', 'foreground-1',
      Array.from({ length: 10 }, (_, index) => `foreground-${index + 1}`))

    await scheduler.whenIdle()

    expect(started.indexOf('background')).toBeGreaterThan(0)
    expect(started.indexOf('background')).toBeLessThanOrEqual(8)
  })

  it('isolates a failed Session and continues the queue', async () => {
    const started: string[] = []
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 1,
      start: async (candidate) => {
        started.push(candidate.sessionId)
        if (candidate.sessionId === 'broken') throw new Error('journal damaged')
      }
    })
    scheduler.enqueue([
      job('broken', 'scene-a', 'active-session', 1),
      job('healthy', 'scene-a', 'foreground-scene', 2)
    ])
    scheduler.prioritizeScene('scene-a', 'broken', ['broken', 'healthy'])

    await scheduler.whenIdle()

    expect(started).toEqual(['broken', 'healthy'])
    expect(scheduler.snapshot()).toEqual([
      expect.objectContaining({ sessionId: 'broken', state: 'failed', error: 'journal damaged' }),
      expect.objectContaining({ sessionId: 'healthy', state: 'ready' })
    ])
  })

  it('cancels queued deleted work without disturbing another Session', async () => {
    const first = deferred()
    const started: string[] = []
    const scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: 1,
      start: (candidate) => {
        started.push(candidate.sessionId)
        return candidate.sessionId === 'first' ? first.promise : Promise.resolve()
      }
    })
    scheduler.enqueue([
      job('first', 'scene-a', 'active-session', 1),
      job('deleted', 'scene-a', 'foreground-scene', 2),
      job('remaining', 'scene-a', 'foreground-scene', 3)
    ])
    scheduler.prioritizeScene('scene-a', 'first', ['first', 'deleted', 'remaining'])
    await settle()

    scheduler.cancel(['deleted'])
    first.resolve()
    await scheduler.whenIdle()

    expect(started).toEqual(['first', 'remaining'])
    expect(scheduler.snapshot().some(({ sessionId }) => sessionId === 'deleted')).toBe(false)
  })
})
