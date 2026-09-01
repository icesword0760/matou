import { describe, expect, it } from 'vitest'

import type {
  SceneSessionGraph,
  SessionCanvasMembership,
  SessionEnvironment,
  SessionGitState,
  SessionGraphNode,
  TaskPlacement,
  WindowNavigation,
  WorkspacePathState
} from './model'

describe('PRD 05 hierarchy models', () => {
  it('expresses path state, per-window navigation, and Task placement', () => {
    const pathState = {
      workspaceId: 'workspace-1',
      status: 'invalid',
      reason: 'missing',
      checkedAt: 10,
      validationGeneration: 2
    } satisfies WorkspacePathState
    const navigation = {
      windowId: 'window-1',
      activeWorkspaceId: 'workspace-1',
      taskByWorkspace: { 'workspace-1': 'task-1' },
      sceneByTask: { 'task-1': 'scene-1' },
      sessionByScene: { 'scene-1': 'session-1' }
    } satisfies WindowNavigation
    const placement = {
      windowId: 'window-1',
      taskId: 'task-1',
      ordinal: 0,
      updatedAt: 10
    } satisfies TaskPlacement

    expect({ pathState, navigation, placement }).toMatchObject({
      pathState: { reason: 'missing' },
      navigation: { activeWorkspaceId: 'workspace-1' },
      placement: { ordinal: 0 }
    })
  })
})

describe('session canvas graph models', () => {
  it('keeps stable graph identity and relations when the current mode is Shell', () => {
    const membership = {
      sessionId: 'session-child',
      sceneId: 'scene-1',
      siblingCreatedSeq: 3,
      lastUserInteractionSeq: 9,
      createdAt: 10,
      updatedAt: 11
    } satisfies SessionCanvasMembership
    const node = {
      sessionId: membership.sessionId,
      sceneId: membership.sceneId,
      parentSessionId: 'session-parent',
      relationKind: 'forked-from',
      currentMode: 'shell',
      workStatus: 'idle',
      providerRestoreState: 'failed',
      canFork: false,
      title: '方案 A',
      cwd: '/tmp/workspace',
      activeChildCount: 2,
      stoppedChildCount: 1,
      childModeCounts: { shell: 1, claudeCode: 1 },
      latestLines: ['Claude Code 恢复失败'],
      lastUserInteractionSeq: membership.lastUserInteractionSeq
    } satisfies SessionGraphNode
    const graph = {
      sceneId: 'scene-1',
      focusedSessionId: node.sessionId,
      nodes: [node],
      edges: [{
        parentSessionId: 'session-parent',
        childSessionId: node.sessionId,
        relationKind: 'forked-from',
        createdAt: 10
      }]
    } satisfies SceneSessionGraph

    expect(graph.nodes[0]).toMatchObject({
      currentMode: 'shell',
      parentSessionId: 'session-parent',
      providerRestoreState: 'failed'
    })
    expect(graph.edges[0]).toMatchObject({ relationKind: 'forked-from' })
  })
})


describe('recovery graph models', () => {
  it('keeps a missing-worktree session asset complete when Git state is unavailable', () => {
    const environment = {
      kind: 'worktree',
      state: 'missing',
      path: '/tmp/missing-worktree',
      localExecutionContextId: 'context-local',
      worktreeId: 'worktree-1',
      worktreeExecutionContextId: 'context-worktree',
      error: 'path-missing'
    } satisfies SessionEnvironment
    const node = {
      sessionId: 'session-missing-worktree',
      sceneId: 'scene-1',
      currentMode: 'claude-code',
      workStatus: 'interrupted',
      providerRestoreState: 'none',
      canFork: false,
      title: '保留的会话资产',
      cwd: '/tmp/missing-worktree',
      environment,
      activeChildCount: 0,
      stoppedChildCount: 0,
      childModeCounts: { shell: 0, claudeCode: 0 },
      latestLines: ['last durable terminal output'],
      lastUserInteractionSeq: 0
    } satisfies SessionGraphNode

    expect(node).toMatchObject({
      sessionId: 'session-missing-worktree',
      title: '保留的会话资产',
      latestLines: ['last durable terminal output'],
      environment: { state: 'missing' }
    })
  })

  it('allows Git state to remain available without an environment projection', () => {
    const git = {
      state: 'ready',
      branch: 'feature/recovery',
      dirty: true
    } satisfies SessionGitState
    const node = {
      sessionId: 'session-git-only',
      sceneId: 'scene-1',
      currentMode: 'shell',
      workStatus: 'idle',
      providerRestoreState: 'none',
      canFork: false,
      title: 'Git 状态独立展示',
      cwd: '/tmp/local',
      git,
      activeChildCount: 0,
      stoppedChildCount: 0,
      childModeCounts: { shell: 0, claudeCode: 0 },
      latestLines: [],
      lastUserInteractionSeq: 0
    } satisfies SessionGraphNode

    expect(node).toMatchObject({ git: { branch: 'feature/recovery', dirty: true } })
  })
})
