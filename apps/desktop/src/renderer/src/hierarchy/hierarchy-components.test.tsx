// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { TaskSidebar } from './TaskSidebar'
import { WorkspaceSwitcher } from './WorkspaceSwitcher'
import type { HierarchyCommands, HierarchyProjection } from './hierarchy-types'

describe('Workspace and Task navigation', () => {
  afterEach(cleanup)
  it('shows invalid Workspace state and preserves the path tail', () => {
    render(<WorkspaceSwitcher projection={fixture()} commands={commands()} />)

    expect(screen.getByText('路径失效')).toBeTruthy()
    expect(screen.getByTitle('/Users/demo/projects/frontend/app').textContent).toContain('frontend/app')
  })

  it('disables duplicate Task rename while displaying the product error', async () => {
    const user = userEvent.setup()
    render(<TaskSidebar projection={fixture()} commands={commands()} />)
    await user.click(screen.getByRole('button', { name: '事项菜单：事项 A' }))
    await user.click(screen.getByRole('menuitem', { name: '重命名' }))
    const input = screen.getByRole('textbox', { name: '事项名称' })
    await user.clear(input)
    await user.type(input, '线上 bug')

    expect(screen.getByText('当前工作区下已存在名为“线上 bug”的事项')).toBeTruthy()
    expect(screen.getByRole('button', { name: '确认' })).toHaveProperty('disabled', true)
  })

  it('rejects a Task drop from another Workspace without issuing a reorder', () => {
    const target = commands()
    render(<TaskSidebar projection={fixture()} commands={target} />)
    const row = screen.getByTestId('task-task-a')
    fireEvent.drop(row, { dataTransfer: { getData: () => JSON.stringify({
      workspaceId: 'workspace-other', taskId: 'task-other'
    }) } })
    expect(target.reorderTask).not.toHaveBeenCalled()
  })
})

function fixture(): HierarchyProjection {
  return {
    windowId: 'window-1',
    workspaces: [{ id: 'workspace-1', name: 'Frontend', rootDirectory: '/Users/demo/projects/frontend/app' }],
    tasks: [
      { id: 'task-a', workspaceId: 'workspace-1', title: '事项 A' },
      { id: 'task-b', workspaceId: 'workspace-1', title: '线上 bug' }
    ],
    scenes: [], sessions: [],
    pathStates: [{ workspaceId: 'workspace-1', status: 'invalid', reason: 'missing' }],
    navigation: {
      windowId: 'window-1', activeWorkspaceId: 'workspace-1',
      taskByWorkspace: { 'workspace-1': 'task-a' }, sceneByTask: {}, sessionByScene: {}
    },
    taskPlacements: []
  }
}

function commands(): HierarchyCommands {
  return {
    activateWorkspace: vi.fn(), createWorkspace: vi.fn(), renameWorkspace: vi.fn(),
    removeWorkspace: vi.fn(), activateTask: vi.fn(), createTask: vi.fn(),
    renameTask: vi.fn(), reorderTask: vi.fn(), deleteTask: vi.fn()
  }
}
