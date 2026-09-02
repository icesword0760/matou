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

  it('updates both the terminal entity and graph during restore and legacy stopped-node recovery', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [], relations: [], scenes: [],
      sessions: [{ id: 'session-1', kind: 'shell', status: 'archived', archivedAt: 1 }],
      sessionGraphs: { 'scene-1': { sceneId: 'scene-1', nodes: [], edges: [] } }
    })
    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'restart-2', eventType: 'session.stopped-state-changed',
      aggregateType: 'session', aggregateId: 'session-1',
      payload: {
        session: { id: 'session-1', kind: 'shell', status: 'created' },
        graph: { sceneId: 'scene-1', nodes: [{ sessionId: 'session-1', currentMode: 'shell' }], edges: [] }
      },
      schemaVersion: 1, commandId: 'restart', occurredAt: 2
    }])

    expect(store.view().sessions).toEqual([expect.objectContaining({ id: 'session-1', status: 'created' })])
    expect(store.view().sessionGraphs['scene-1']?.nodes).toEqual([
      expect.objectContaining({ sessionId: 'session-1', currentMode: 'shell' })
    ])
  })

  it('merges an authoritative command result without rebuilding the full projection', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [{ id: 'workspace-a' }, { id: 'workspace-b' }],
      tasks: [{ id: 'task-b', workspaceId: 'workspace-b' }],
      sessions: [{ id: 'session-b', taskId: 'task-b' }], relations: [],
      scenes: [{ id: 'scene-b', taskId: 'task-b' }],
      hierarchy: {
        windowId: 'window-1', workspaces: [], tasks: [], sessions: [], scenes: [],
        navigation: {
          windowId: 'window-1', activeWorkspaceId: 'workspace-a',
          taskByWorkspace: { 'workspace-b': 'task-b' },
          sceneByTask: { 'task-b': 'scene-b' }, sessionByScene: { 'scene-b': 'session-b' }
        }
      }
    })

    store.applyCommandResult({
      workspace: { id: 'workspace-b', name: 'Workspace B' },
      task: { id: 'task-b', workspaceId: 'workspace-b', title: 'Task B' },
      scene: { id: 'scene-b', taskId: 'task-b', name: 'Scene B' },
      session: { id: 'session-b', taskId: 'task-b', title: 'Session B' },
      navigation: {
        windowId: 'window-1', activeWorkspaceId: 'workspace-b',
        taskByWorkspace: { 'workspace-b': 'task-b' },
        sceneByTask: { 'task-b': 'scene-b' }, sessionByScene: { 'scene-b': 'session-b' }
      },
      graph: {
        sceneId: 'scene-b', focusedSessionId: 'session-b',
        nodes: [{ sessionId: 'session-b' }], edges: []
      }
    })

    const hierarchy = store.view().hierarchy
    expect(hierarchy.navigation).toMatchObject({ activeWorkspaceId: 'workspace-b' })
    expect(hierarchy.workspaces).toContainEqual(expect.objectContaining({ id: 'workspace-b', name: 'Workspace B' }))
    expect(hierarchy.sessionGraphs?.['scene-b']).toMatchObject({ focusedSessionId: 'session-b' })
    expect(store.eventSequence).toBe(1)
  })

  it('merges ordered pin results, path validation and recency without a full projection rebuild', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [
        { id: 'workspace-a', lastOpenedAt: 1 },
        { id: 'workspace-b', lastOpenedAt: 2 }
      ],
      tasks: [
        { id: 'task-a', workspaceId: 'workspace-a', lastOpenedAt: 1 },
        { id: 'task-b', workspaceId: 'workspace-a', lastOpenedAt: 2 }
      ],
      sessions: [], relations: [], scenes: [],
      hierarchy: {
        windowId: 'window-1', workspaces: [], tasks: [], sessions: [], scenes: [],
        navigation: { windowId: 'window-1' }, pathStates: []
      }
    })

    store.applyCommandResult([
      { id: 'workspace-b', isPinned: true, pinSortKey: 'a0' },
      { id: 'workspace-a', isPinned: true, pinSortKey: 'a1' }
    ], { type: 'hierarchy.reorder-pinned-workspace', input: {} })
    store.applyCommandResult([
      { id: 'task-b', workspaceId: 'workspace-a', isPinned: true, pinSortKey: 'a0' },
      { id: 'task-a', workspaceId: 'workspace-a', isPinned: true, pinSortKey: 'a1' }
    ], { type: 'hierarchy.reorder-pinned-task', input: {} })
    store.applyCommandResult({
      workspaceId: 'workspace-a', status: 'invalid', reason: 'missing',
      checkedAt: 20, validationGeneration: 3
    }, { type: 'hierarchy.validate-workspace-path', input: { workspaceId: 'workspace-a' } })
    store.applyCommandResult({
      sessionId: 'session-a', taskId: 'task-a', workspaceId: 'workspace-a', lastOpenedAt: 40
    }, { type: 'hierarchy.record-session-interaction', input: { sessionId: 'session-a' } })

    expect(store.view().workspaces).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: 'workspace-a', pinSortKey: 'a1', lastOpenedAt: 40 }),
      expect.objectContaining({ id: 'workspace-b', pinSortKey: 'a0' })
    ]))
    expect(store.view().tasks).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: 'task-a', pinSortKey: 'a1', lastOpenedAt: 40 }),
      expect.objectContaining({ id: 'task-b', pinSortKey: 'a0' })
    ]))
    expect(store.view().hierarchy.pathStates).toEqual([
      expect.objectContaining({ workspaceId: 'workspace-a', status: 'invalid' })
    ])
  })

  it('applies ordering, pin, relink, path and recency events incrementally', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [{ id: 'workspace-a', rootDirectory: '/old', lastOpenedAt: 1 }],
      tasks: [{ id: 'task-a', workspaceId: 'workspace-a', lastOpenedAt: 1 }],
      sessions: [], relations: [], scenes: [{ id: 'scene-a', taskId: 'task-a' }],
      hierarchy: {
        windowId: 'window-1', workspaces: [], tasks: [], sessions: [], scenes: [],
        navigation: { windowId: 'window-1' }, pathStates: []
      }
    })
    const base = {
      aggregateType: 'test', schemaVersion: 1, commandId: 'cmd', occurredAt: 2
    }
    store.applyBatch('generation-1', [
      { ...base, sequence: 2, eventId: '2', eventType: 'workspace.relinked', aggregateId: 'workspace-a', payload: { rootDirectory: '/new' } },
      { ...base, sequence: 3, eventId: '3', eventType: 'workspace.pin-changed', aggregateId: 'workspace-a', payload: { isPinned: true, pinSortKey: 'a0' } },
      { ...base, sequence: 4, eventId: '4', eventType: 'workspace.task-order-changed', aggregateId: 'workspace-a', payload: { taskOrder: ['task-a'] } },
      { ...base, sequence: 5, eventId: '5', eventType: 'task.pin-changed', aggregateId: 'task-a', payload: { isPinned: true, pinSortKey: 'a0' } },
      { ...base, sequence: 6, eventId: '6', eventType: 'task.scene-order-changed', aggregateId: 'task-a', payload: { sceneOrder: ['scene-a'] } },
      { ...base, sequence: 7, eventId: '7', eventType: 'navigation.recency-changed', aggregateId: 'task-a', payload: { workspaceId: 'workspace-a', taskId: 'task-a', lastOpenedAt: 50 } },
      { ...base, sequence: 8, eventId: '8', eventType: 'workspace.path-status-changed', aggregateId: 'workspace-a', payload: { workspaceId: 'workspace-a', status: 'valid', reason: '', checkedAt: 50, validationGeneration: 4 } }
    ])

    expect(store.view().workspaces[0]).toMatchObject({
      rootDirectory: '/new', isPinned: true, taskOrder: ['task-a'], lastOpenedAt: 50
    })
    expect(store.view().tasks[0]).toMatchObject({
      isPinned: true, sceneOrder: ['scene-a'], lastOpenedAt: 50
    })
    expect(store.view().hierarchy.pathStates).toEqual([
      expect.objectContaining({ workspaceId: 'workspace-a', status: 'valid' })
    ])
  })

  it('replaces one changed Scene tree without rebuilding unrelated projection state', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 7,
      workspaces: [{ id: 'workspace-a' }], tasks: [], sessions: [], relations: [],
      scenes: [{ id: 'scene-a', layoutRevision: 1 }],
      hierarchy: {
        windowId: 'window-1', workspaces: [], tasks: [], sessions: [], scenes: [],
        navigation: { windowId: 'window-1' },
        sceneSnapshots: [{
          scene: { id: 'scene-a', layoutRevision: 1 },
          nodes: [{ id: 'old-root' }], mounts: [], windows: []
        }]
      }
    })

    store.applySceneSnapshot({
      scene: { id: 'scene-a', layoutRevision: 2 },
      nodes: [{ id: 'split-root' }, { id: 'left' }, { id: 'right' }],
      mounts: [{ id: 'mount-left' }, { id: 'mount-right' }], windows: [], geometry: []
    })

    expect(store.view().hierarchy.sceneSnapshots).toEqual([
      expect.objectContaining({
        scene: expect.objectContaining({ id: 'scene-a', layoutRevision: 2 }),
        nodes: expect.arrayContaining([expect.objectContaining({ id: 'split-root' })])
      })
    ])
    expect(store.view().workspaces).toEqual([{ id: 'workspace-a' }])
    expect(store.eventSequence).toBe(7)
  })

  it('loads the graph for a Scene selected after the initial projection', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 7,
      workspaces: [], tasks: [], sessions: [], relations: [], scenes: [],
      hierarchy: {
        windowId: 'window-1', workspaces: [], tasks: [], sessions: [], scenes: [],
        navigation: { windowId: 'window-1' }
      }
    })

    store.applySceneGraph({
      sceneId: 'scene-later', focusedSessionId: 'session-later',
      nodes: [{ sessionId: 'session-later', title: 'Later' }], edges: []
    })

    expect(store.view().hierarchy.sessionGraphs).toEqual({
      'scene-later': expect.objectContaining({
        sceneId: 'scene-later', focusedSessionId: 'session-later',
        nodes: [expect.objectContaining({ sessionId: 'session-later' })]
      })
    })
    expect(store.eventSequence).toBe(7)
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
