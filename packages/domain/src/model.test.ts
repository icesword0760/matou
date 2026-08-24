import { describe, expect, it } from 'vitest'

import type {
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
