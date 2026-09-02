import { describe, expect, it, vi } from 'vitest'

import { RuntimeLifecyclePublisher } from './runtime-lifecycle-publisher'

describe('runtime lifecycle publisher', () => {
  it('publishes opening, recovery-required and ready from the real child transport', () => {
    const postMessage = vi.fn()
    const publisher = new RuntimeLifecyclePublisher({ postMessage } as never, () => 'recovery-1')

    publisher.opening()
    publisher.recoveryRequired({
      kind: 'recovery-required', recoveryId: 'durable-recovery-1',
      reason: 'ownership-recovery-required',
      durableDatabasePath: '/data/matou.sqlite', quarantinedPath: '/data/matou.sqlite',
      markerPath: '/data/matou.sqlite.recovery.json', ownershipIssue: 'owner-record-malformed',
      backups: []
    })
    publisher.openingNewAttempt()
    publisher.ready('normal')

    expect(postMessage.mock.calls.map(([message]) => message.type)).toEqual([
      'runtime.lifecycle', 'runtime.recovery-details', 'runtime.lifecycle',
      'runtime.lifecycle', 'runtime.lifecycle'
    ])
    expect(postMessage.mock.calls[1]![0].recovery).toMatchObject({
      recoveryId: 'durable-recovery-1', reason: 'ownership-recovery-required',
      ownershipIssue: 'owner-record-malformed'
    })
    expect(postMessage.mock.calls[0]![0].snapshot).toMatchObject({
      recoveryId: 'recovery-1', revision: 1, stage: 'opening-database'
    })
    expect(postMessage.mock.calls[3]![0].snapshot).toMatchObject({
      recoveryId: 'recovery-1', revision: 3, stage: 'opening-database'
    })
    expect(postMessage.mock.calls[4]![0].snapshot).toMatchObject({
      revision: 4, mode: 'normal', stage: 'ready', completed: 1
    })
  })
})
