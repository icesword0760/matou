import { describe, expect, it } from 'vitest'

import { parseDomainEventEnvelope } from './domain-events'

describe('domain event wire contract', () => {
  it('accepts a complete versioned envelope', () => {
    expect(
      parseDomainEventEnvelope({
        sequence: 4,
        eventId: 'event-4',
        eventType: 'session.created',
        aggregateType: 'session',
        aggregateId: 'session-1',
        taskId: 'task-1',
        sessionId: 'session-1',
        payload: { kind: 'shell' },
        schemaVersion: 1,
        requiredTerminalSequence: 12,
        commandId: 'cmd-4',
        correlationId: 'workflow-1',
        occurredAt: 100
      })
    ).toMatchObject({ sequence: 4, requiredTerminalSequence: 12 })
  })

  it('rejects invalid sequence and schema versions', () => {
    expect(() =>
      parseDomainEventEnvelope({
        sequence: 0,
        eventId: 'event',
        eventType: 'test',
        aggregateType: 'test',
        aggregateId: 'test',
        payload: {},
        schemaVersion: 0,
        commandId: 'cmd',
        occurredAt: 1
      })
    ).toThrow()
  })
})
