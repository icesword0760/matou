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
    expect(store.applyTerminalHud('session-a', { sessionId: 'session-a' })).toBe(false)
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

  it('does not let a delayed graph event reverse newer window-local focus', () => {
    const store = new RuntimeProjectionStore()
    const nodes = [
      { sessionId: 'session-1', currentMode: 'shell' },
      { sessionId: 'session-2', currentMode: 'shell' }
    ]
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [], sessions: [], relations: [], scenes: [],
      sessionGraphs: {
        'scene-1': { sceneId: 'scene-1', focusedSessionId: 'session-1', nodes, edges: [] }
      }
    })
    store.applyCommandResult({
      sceneId: 'scene-1', focusedSessionId: 'session-2', nodes, edges: []
    }, { type: 'hierarchy.set-focused-session', input: {
      sceneId: 'scene-1', sessionId: 'session-2'
    } })

    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'delayed-graph', eventType: 'session.graph-summary-changed',
      aggregateType: 'scene', aggregateId: 'scene-1', payload: {
        graph: { sceneId: 'scene-1', focusedSessionId: 'session-1', nodes, edges: [] }
      }, schemaVersion: 1, commandId: 'older-terminal-work', occurredAt: 2
    }])

    expect(store.view().sessionGraphs['scene-1']?.focusedSessionId).toBe('session-2')

    store.applyBatch('generation-1', [{
      sequence: 3, eventId: 'removed-focus', eventType: 'session.graph-summary-changed',
      aggregateType: 'scene', aggregateId: 'scene-1', payload: {
        graph: {
          sceneId: 'scene-1', focusedSessionId: 'session-1',
          nodes: [{ sessionId: 'session-1', currentMode: 'shell' }], edges: []
        }
      }, schemaVersion: 1, commandId: 'remove-session', occurredAt: 3
    }])
    expect(store.view().sessionGraphs['scene-1']?.focusedSessionId).toBe('session-1')
  })

  it('projects a Fork stage emitted while background setup is still running', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [], sessions: [], relations: [], scenes: [],
      sessionGraphs: {
        'scene-1': {
          sceneId: 'scene-1', nodes: [{ sessionId: 'child-1', forkProgress: {
            operationId: 'operation-1', sessionId: 'child-1', submissionKey: 'submission-1',
            stage: 'creating-worktree', completedSteps: 0, totalSteps: 5, attempt: 0
          } }], edges: []
        }
      }
    })

    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'fork-progress-2', eventType: 'session.fork-progressed',
      aggregateType: 'session', aggregateId: 'child-1', sessionId: 'child-1',
      payload: {
        graph: {
          sceneId: 'scene-1', nodes: [{ sessionId: 'child-1', forkProgress: {
            operationId: 'operation-1', sessionId: 'child-1', submissionKey: 'submission-1',
            stage: 'applying-setup', completedSteps: 1, totalSteps: 5, attempt: 0
          } }], edges: []
        }
      },
      schemaVersion: 1, commandId: 'fork-progress', occurredAt: 2
    }])

    expect(store.view().sessionGraphs['scene-1']?.nodes[0]?.forkProgress)
      .toMatchObject({ stage: 'applying-setup' })
  })

  it('keeps this window on its focused branch when a provider event carries a window-neutral graph', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [], sessions: [], relations: [], scenes: [],
      sessionGraphs: {
        'scene-1': {
          sceneId: 'scene-1', focusedSessionId: 'leaf',
          nodes: [
            { sessionId: 'root', currentMode: 'claude-code' },
            { sessionId: 'leaf', parentSessionId: 'root', currentMode: 'claude-code' }
          ], edges: [{ parentSessionId: 'root', childSessionId: 'leaf' }]
        }
      },
      hierarchy: {
        windowId: 'window-1', workspaces: [], tasks: [], sessions: [], scenes: [],
        navigation: { windowId: 'window-1', sessionByScene: { 'scene-1': 'leaf' } }
      }
    })

    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'provider-ready', eventType: 'session.mode-changed',
      aggregateType: 'session', aggregateId: 'root', sessionId: 'root',
      payload: {
        session: { id: 'root', kind: 'claude-code' },
        graph: {
          sceneId: 'scene-1',
          nodes: [
            { sessionId: 'root', currentMode: 'claude-code' },
            { sessionId: 'leaf', parentSessionId: 'root', currentMode: 'claude-code' }
          ], edges: [{ parentSessionId: 'root', childSessionId: 'leaf' }]
        }
      },
      schemaVersion: 1, commandId: 'provider-ready', occurredAt: 2
    }])

    expect(store.view().sessionGraphs['scene-1']?.focusedSessionId).toBe('leaf')
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

  it('makes a Task created in the current window visible before the next full projection', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [{ id: 'workspace-1' }],
      tasks: [{ id: 'task-1', workspaceId: 'workspace-1' }],
      sessions: [], relations: [], scenes: [],
      hierarchy: {
        windowId: 'window-1', workspaces: [], tasks: [], sessions: [], scenes: [],
        navigation: {
          windowId: 'window-1', activeWorkspaceId: 'workspace-1',
          taskByWorkspace: { 'workspace-1': 'task-1' }, sceneByTask: {}, sessionByScene: {}
        },
        taskPlacements: [{ windowId: 'window-1', taskId: 'task-1', ordinal: 0, updatedAt: 1 }]
      }
    })

    store.applyCommandResult({
      task: {
        id: 'task-2', workspaceId: 'workspace-1', title: '新事项', updatedAt: 2
      },
      navigation: {
        windowId: 'window-1', activeWorkspaceId: 'workspace-1',
        taskByWorkspace: { 'workspace-1': 'task-2' }, sceneByTask: {}, sessionByScene: {}
      }
    }, { type: 'hierarchy.create-task', input: { workspaceId: 'workspace-1' } })

    expect(store.view().hierarchy.taskPlacements).toEqual([
      expect.objectContaining({ windowId: 'window-1', taskId: 'task-1', ordinal: 0 }),
      expect.objectContaining({ windowId: 'window-1', taskId: 'task-2', ordinal: 1 })
    ])
    expect(store.view().hierarchy.navigation).toMatchObject({
      taskByWorkspace: { 'workspace-1': 'task-2' }
    })
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

  it('keeps live terminal HUD state through the next semantic event batch', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [{ id: 'workspace-a' }],
      tasks: [{ id: 'task-a', workspaceId: 'workspace-a' }],
      sessions: [], relations: [], scenes: [],
      hierarchy: {
        windowId: 'window-1', workspaces: [], tasks: [], sessions: [], scenes: [],
        navigation: { windowId: 'window-1' },
        sessionHuds: [{ sessionId: 'session-a', mode: 'shell', gitDirty: false }]
      }
    })
    store.applyTerminalHud('session-a', {
      sessionId: 'session-a', mode: 'agent', gitDirty: true, contextPercent: 72
    })
    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'interaction', eventType: 'session.user-interacted',
      aggregateType: 'session', aggregateId: 'session-a', workspaceId: 'workspace-a',
      taskId: 'task-a', sessionId: 'session-a', schemaVersion: 1,
      commandId: 'cmd', occurredAt: 50,
      payload: {
        sessionId: 'session-a', workspaceId: 'workspace-a', taskId: 'task-a',
        sceneId: 'scene-a', interactionKind: 'submit', sequence: 1,
        graph: { sceneId: 'scene-a', nodes: [], edges: [] }
      }
    }])

    expect(store.view().hierarchy.sessionHuds).toEqual([
      expect.objectContaining({
        sessionId: 'session-a', mode: 'agent', gitDirty: true, contextPercent: 72
      })
    ])
    expect(store.view().workspaces[0]).toMatchObject({ lastOpenedAt: 50 })
    expect(store.view().tasks[0]).toMatchObject({ lastOpenedAt: 50 })
  })

  it('reorders Scene entities when Runtime publishes the authoritative Scene order', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [{ id: 'task-a' }], sessions: [], relations: [],
      scenes: [
        { id: 'scene-a', taskId: 'task-a', sortKey: 'a0' },
        { id: 'scene-b', taskId: 'task-a', sortKey: 'a1' }
      ]
    })
    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'reorder', eventType: 'task.scene-order-changed',
      aggregateType: 'task', aggregateId: 'task-a', taskId: 'task-a',
      payload: { sceneOrder: ['scene-b', 'scene-a'] }, schemaVersion: 1,
      commandId: 'cmd', occurredAt: 2
    }])

    expect(store.view().hierarchy.scenes.map(({ id }) => id)).toEqual(['scene-b', 'scene-a'])
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

  it('updates the card list and DAG node when a session title changes', () => {
    const store = new RuntimeProjectionStore()
    store.replace({
      runtimeGeneration: 'generation-1', eventSequence: 1,
      workspaces: [], tasks: [], relations: [], scenes: [],
      sessions: [{ id: 'session-1', title: 'Claude', titleSource: 'default' }],
      sessionGraphs: {
        'scene-1': {
          sceneId: 'scene-1', nodes: [{ sessionId: 'session-1', title: 'Claude' }], edges: []
        }
      }
    })
    store.applyBatch('generation-1', [{
      sequence: 2, eventId: 'title-2', eventType: 'session.updated',
      aggregateType: 'session', aggregateId: 'session-1',
      payload: {
        session: { id: 'session-1', title: '修复卡片标题同步', titleSource: 'auto' }
      },
      schemaVersion: 1, commandId: 'title-command', occurredAt: 2
    }])

    expect(store.view().sessions).toEqual([
      expect.objectContaining({ title: '修复卡片标题同步', titleSource: 'auto' })
    ])
    expect(store.view().sessionGraphs['scene-1']?.nodes).toEqual([
      expect.objectContaining({ sessionId: 'session-1', title: '修复卡片标题同步' })
    ])
  })
})
