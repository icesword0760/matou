// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { SceneTabBar, type SceneCommands } from './SceneTabBar'
import type { HierarchyProjection } from './hierarchy-types'

afterEach(cleanup)

describe('Scene tabs and split actions', () => {
  it('requests a right-hand horizontal split for the active terminal', async () => {
    const user = userEvent.setup()
    const commands = sceneCommands()
    render(<SceneTabBar projection={fixture(2)} commands={commands} />)

    await user.click(screen.getByRole('button', { name: '水平分屏' }))
    expect(commands.splitSession).toHaveBeenCalledWith('scene-1', 'session-1', 'horizontal')
  })

  it('opens overflow and centers the selected Scene', async () => {
    const user = userEvent.setup()
    const scrollIntoView = vi.fn()
    Element.prototype.scrollIntoView = scrollIntoView
    render(<SceneTabBar projection={fixture(20)} commands={sceneCommands()} visibleLimit={6} />)

    await user.click(screen.getByRole('button', { name: '更多页签' }))
    await user.click(screen.getByRole('menuitem', { name: '页签 20' }))
    expect(scrollIntoView).toHaveBeenCalledWith(expect.objectContaining({ inline: 'center' }))
  })
})

function fixture(count: number): HierarchyProjection {
  return {
    windowId: 'window-1', workspaces: [{ id: 'workspace-1', name: 'W', rootDirectory: '/tmp/w' }],
    tasks: [{ id: 'task-1', workspaceId: 'workspace-1', title: '事项' }],
    scenes: Array.from({ length: count }, (_, index) => ({
      id: `scene-${index + 1}`, taskId: 'task-1', name: `页签 ${index + 1}`
    })),
    sessions: [{ id: 'session-1', taskId: 'task-1', title: 'Shell' }], pathStates: [], taskPlacements: [],
    navigation: {
      windowId: 'window-1', activeWorkspaceId: 'workspace-1',
      taskByWorkspace: { 'workspace-1': 'task-1' }, sceneByTask: { 'task-1': 'scene-1' },
      sessionByScene: { 'scene-1': 'session-1' }
    }
  }
}

function sceneCommands(): SceneCommands {
  return {
    activateScene: vi.fn(), createScene: vi.fn(), renameScene: vi.fn(),
    reorderScene: vi.fn(), closeScene: vi.fn(), splitSession: vi.fn()
  }
}
