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

  it('keeps an active hierarchy projection and removes archived entities from it', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [{ id: 'workspace-1' }],
      tasks: [{ id: 'task-1', workspaceId: 'workspace-1', status: 'active' }],
      sessions: [{ id: 'session-1', taskId: 'task-1', status: 'running' }],
      relations: [], scenes: [{ id: 'scene-1', taskId: 'task-1' }],
      hierarchy: {
        windowId: 'window-1', workspaces: [{ id: 'workspace-1' }],
        tasks: [{ id: 'task-1' }], sessions: [{ id: 'session-1' }],
        scenes: [{ id: 'scene-1' }], navigation: { windowId: 'window-1' }
      }
    })
    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'archive-session', eventType: 'session.archived',
      aggregateType: 'session', aggregateId: 'session-1', payload: { archivedAt: 2 },
      schemaVersion: 1, commandId: 'cmd', occurredAt: 2
    }])

    expect(store.view().hierarchy.sessions).toEqual([])
  })

  it('keeps archived Scenes discoverable as closed canvases without rendering them as active tabs', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [{ id: 'workspace-1' }],
      tasks: [{ id: 'task-1', workspaceId: 'workspace-1', status: 'active' }],
      sessions: [], relations: [],
      scenes: [
        { id: 'scene-active', taskId: 'task-1', name: '开发' },
        { id: 'scene-closed', taskId: 'task-1', name: '历史排查', archivedAt: 9 }
      ]
    })

    expect(store.view().hierarchy.scenes.map(({ id }) => id)).toEqual(['scene-active'])
    expect(store.view().hierarchy.closedScenes).toEqual([
      expect.objectContaining({ id: 'scene-closed', name: '历史排查', archivedAt: 9 })
    ])
  })

  it('moves a reopened canvas out of closed canvases and back into active tabs', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [], sessions: [], relations: [],
      scenes: [{ id: 'scene-closed', taskId: 'task-1', name: '排查', archivedAt: 8 }]
    })
    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'reopen-scene', eventType: 'scene.reopened',
      aggregateType: 'scene', aggregateId: 'scene-closed', payload: { reopenedAt: 9 },
      schemaVersion: 1, commandId: 'reopen', occurredAt: 9
    }])

    expect(store.view().hierarchy.scenes.map(({ id }) => id)).toEqual(['scene-closed'])
    expect(store.view().hierarchy.closedScenes).toEqual([])
  })

  it('replaces and incrementally updates Runtime-owned Scene session graphs', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [], sessions: [], relations: [], scenes: [],
      sessionGraphs: {
        'scene-1': { sceneId: 'scene-1', nodes: [], edges: [] }
      }
    })
    store.applyBatch('generation-1', [{
      sequence: 2,
      eventId: 'graph-2',
      eventType: 'session.graph-summary-changed',
      aggregateType: 'scene',
      aggregateId: 'scene-1',
      payload: {
        graph: {
          sceneId: 'scene-1',
          focusedSessionId: 'session-1',
          nodes: [{ sessionId: 'session-1', currentMode: 'shell' }],
          edges: []
        }
      },
      schemaVersion: 1,
      commandId: 'graph-command',
      occurredAt: 2
    }])

    expect(store.view().sessionGraphs['scene-1']).toMatchObject({
      focusedSessionId: 'session-1',
      nodes: [expect.objectContaining({ sessionId: 'session-1' })]
    })
    expect(store.view().hierarchy.sessionGraphs?.['scene-1']).toMatchObject({
      focusedSessionId: 'session-1'
    })
  })

  it('updates the visible terminal path when a live Shell changes directory', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [], relations: [], scenes: [],
      sessions: [{ id: 'session-1', cwd: '/old' }],
      sessionGraphs: {
        'scene-1': {
          sceneId: 'scene-1', nodes: [{ sessionId: 'session-1', cwd: '/old' }], edges: []
        }
      }
    })
    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'cwd-2', eventType: 'session.cwd-updated',
      aggregateType: 'session', aggregateId: 'session-1', payload: { cwd: '/deep/new/path' },
      schemaVersion: 1, commandId: 'cwd-command', occurredAt: 2
    }])

    expect(store.view().sessions).toEqual([expect.objectContaining({ cwd: '/deep/new/path' })])
    expect(store.view().sessionGraphs['scene-1']?.nodes).toEqual([
      expect.objectContaining({ sessionId: 'session-1', cwd: '/deep/new/path' })
    ])
  })
})
