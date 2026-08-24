import { describe, expect, it } from 'vitest'

import { RuntimeProjectionStore } from './RuntimeProjectionStore'

describe('RuntimeProjectionStore', () => {
  it('rebuilds exclusively from a Runtime snapshot and ordered events', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [{ id: 'workspace-1', name: 'Old' }], tasks: [], sessions: [],
      relations: [], scenes: []
    })
    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'event-2', eventType: 'workspace.updated',
      aggregateType: 'workspace', aggregateId: 'workspace-1', workspaceId: 'workspace-1',
      payload: { id: 'workspace-1', name: 'New' }, schemaVersion: 1,
      commandId: 'cmd-2', occurredAt: 2
    }])

    expect(store.view().workspaces).toEqual([{ id: 'workspace-1', name: 'New' }])
    expect(store.eventSequence).toBe(2)
  })

  it('ignores duplicate delivery but requires a fresh snapshot on gaps or Runtime restart', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 2,
      workspaces: [], tasks: [], sessions: [], relations: [], scenes: []
    })
    const duplicate = {
      sequence: 2, eventId: 'event-2', eventType: 'test', aggregateType: 'test',
      aggregateId: 'test', payload: {}, schemaVersion: 1, commandId: 'cmd', occurredAt: 2
    }
    store.applyBatch('generation-1', [duplicate])

    expect(() => store.applyBatch('generation-1', [{ ...duplicate, sequence: 4, eventId: 'event-4' }])).toThrow(
      'projection event gap: expected 3, received 4'
    )
    expect(() => store.applyBatch('generation-2', [])).toThrow(
      'runtime generation changed; a fresh projection snapshot is required'
    )
  })

  it('does not expose an authoritative snapshot export path', () => {
    const store = new RuntimeProjectionStore()
    expect('exportAuthoritativeSnapshot' in store).toBe(false)
  })
})
