import { describe, expect, it } from 'vitest'

import { RuntimeRecoveryE2eObserver } from './runtime-recovery-e2e-observer'

describe('RuntimeRecoveryE2eObserver', () => {
  it('records only state transitions and the peak number of restoring jobs', () => {
    const observer = new RuntimeRecoveryE2eObserver()
    observer.record([
      job('active', 'active-session', 'queued'),
      job('background', 'background', 'queued')
    ])
    observer.record([
      job('active', 'active-session', 'restoring'),
      job('background', 'background', 'queued')
    ])
    observer.record([
      job('active', 'active-session', 'ready'),
      job('background', 'background', 'restoring')
    ])
    observer.record([
      job('active', 'active-session', 'ready'),
      job('background', 'background', 'ready')
    ])

    expect(observer.snapshot()).toEqual({
      maxRestoring: 1,
      transitions: [
        transition(1, 'active', 'active-session', 'queued', 0),
        transition(2, 'background', 'background', 'queued', 0),
        transition(3, 'active', 'active-session', 'restoring', 1),
        transition(4, 'active', 'active-session', 'ready', 1),
        transition(5, 'background', 'background', 'restoring', 1),
        transition(6, 'background', 'background', 'ready', 0)
      ]
    })
  })
})

function job(
  sessionId: string,
  priority: 'active-session' | 'background',
  state: 'queued' | 'restoring' | 'ready'
) {
  return {
    sessionId, sceneId: `${sessionId}-scene`, priority,
    enqueueSequence: priority === 'active-session' ? 1 : 2, state
  }
}

function transition(
  sequence: number,
  sessionId: string,
  priority: 'active-session' | 'background',
  state: 'queued' | 'restoring' | 'ready',
  restoringCount: number
) {
  return {
    sequence, sessionId, sceneId: `${sessionId}-scene`,
    priority, state, restoringCount
  }
}
