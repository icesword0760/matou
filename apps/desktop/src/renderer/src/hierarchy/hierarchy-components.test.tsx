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
    fireEvent.click(screen.getByRole('button', { name: '切换工作区' }))
    expect(screen.getByTitle('/Users/demo/projects/frontend/app').textContent).toContain('frontend/app')
  })

  it('shows path state inside the Workspace list and confirms removal without touching the directory', async () => {
    const user = userEvent.setup()
    const target = commands()
    render(<WorkspaceSwitcher projection={fixture()} commands={target} />)

    await user.click(screen.getByRole('button', { name: '切换工作区' }))
    expect(screen.getByRole('menuitem', { name: 'Frontend' }).textContent).toContain('路径失效')
    await user.click(screen.getByRole('menuitem', { name: '删除' }))
    expect(screen.getByRole('alertdialog', { name: '提示' }).textContent).toContain(
      '删除 "Frontend" 不会删除磁盘上的工作区目录，但该工作区下所有终端会话都会被丢弃，无法恢复。 是否继续?'
    )
    await user.click(screen.getByRole('button', { name: '取消' }))
    expect(target.removeWorkspace).not.toHaveBeenCalled()

    await user.click(screen.getByRole('button', { name: '切换工作区' }))
    await user.click(screen.getByRole('menuitem', { name: '删除' }))
    await user.click(screen.getByRole('button', { name: '确定' }))
    expect(target.removeWorkspace).toHaveBeenCalledWith('workspace-1')
  })

  it('disables duplicate Task rename while displaying the product error', async () => {
    const user = userEvent.setup()
    render(<TaskSidebar projection={fixture()} commands={commands()} />)
    await user.click(screen.getByRole('button', { name: '事项菜单：事项 A' }))
    await user.click(screen.getByRole('menuitem', { name: '重命名' }))
    const input = screen.getByRole('textbox', { name: '事项名称' })
    await user.clear(input)
    await user.type(input, '线上 bug')

    expect(screen.getByText('当前工作区下已存在名为"线上 bug"的工作台')).toBeTruthy()
    expect(screen.getByRole('button', { name: '确定' })).toHaveProperty('disabled', true)
  })

  it('keeps the Kooky rename dialog open and explains an empty Task name', async () => {
    const user = userEvent.setup()
    const target = commands()
    render(<TaskSidebar projection={fixture()} commands={target} />)
    await user.click(screen.getByRole('button', { name: '事项菜单：事项 A' }))
    await user.click(screen.getByRole('menuitem', { name: '重命名' }))
    await user.clear(screen.getByRole('textbox', { name: '事项名称' }))
    await user.click(screen.getByRole('button', { name: '确定' }))

    expect(screen.getByText('工作台名称不能为空')).toBeTruthy()
    expect(target.renameTask).not.toHaveBeenCalled()
  })

  it('keeps the Kooky rename dialog open when the authoritative rename is rejected', async () => {
    const user = userEvent.setup()
    const target = commands()
    vi.mocked(target.renameTask).mockRejectedValueOnce(new Error('conflict'))
    render(<TaskSidebar projection={fixture()} commands={target} />)
    await user.click(screen.getByRole('button', { name: '事项菜单：事项 A' }))
    await user.click(screen.getByRole('menuitem', { name: '重命名' }))
    await user.clear(screen.getByRole('textbox', { name: '事项名称' }))
    await user.type(screen.getByRole('textbox', { name: '事项名称' }), '新名字')
    await user.click(screen.getByRole('button', { name: '确定' }))

    expect(await screen.findByText('重命名失败：名称为空或已存在')).toBeTruthy()
    expect(screen.getByRole('dialog')).toBeTruthy()
    await user.type(screen.getByRole('textbox', { name: '事项名称' }), ' 2')
    expect(screen.queryByText('重命名失败：名称为空或已存在')).toBeNull()
    expect(screen.getByRole('button', { name: '确定' })).toHaveProperty('disabled', false)
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

  it('matches Kooky drag-and-drop Task ordering', async () => {
    const target = commands()
    render(<TaskSidebar projection={fixture()} commands={target} />)

    const source = screen.getByTestId('task-task-b')
    const destination = screen.getByTestId('task-task-a')
    const transfer = { setData: vi.fn(), getData: vi.fn(() => JSON.stringify({ workspaceId: 'workspace-1', taskId: 'task-b' })) }
    fireEvent.dragStart(source, { dataTransfer: transfer })
    expect(source.classList.contains('is-dragging')).toBe(true)
    fireEvent.dragOver(destination, { dataTransfer: transfer })
    expect(destination.classList.contains('drag-over')).toBe(true)
    fireEvent.drop(destination, { dataTransfer: transfer })
    expect(target.reorderTask).toHaveBeenCalledWith('workspace-1', 'task-b', 'task-a')
    expect(source.classList.contains('is-dragging')).toBe(false)
    expect(destination.classList.contains('drag-over')).toBe(false)
    expect(screen.getByTestId('active-task').textContent).toBe('事项 A')
  })

  it('matches Kooky unread badge priority, cap, and long-name truncation', () => {
    const data = fixture()
    data.tasks[0]!.title = '这是一个非常非常长的事项名称，用于验证完整名称提示'
    data.unreadByTask = { 'task-a': 120 }
    render(<TaskSidebar projection={data} commands={commands()} />)

    expect(screen.getByText('99+').classList.contains('workbench-item__badge')).toBe(true)
    expect(screen.queryByRole('button', { name: /事项菜单：这是一个/ })).toBeNull()
    expect(screen.getByText(data.tasks[0]!.title).classList.contains('workbench-item__name')).toBe(true)
    expect(screen.getByText(data.tasks[0]!.title).getAttribute('title')).toBeNull()
  })

  it('closes the Kooky Task menu on Escape and outside interaction', async () => {
    const user = userEvent.setup()
    render(<TaskSidebar projection={fixture()} commands={commands()} />)

    await user.click(screen.getByRole('button', { name: '事项菜单：事项 A' }))
    expect(screen.getByRole('menu')).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menu')).toBeNull()

    await user.click(screen.getByRole('button', { name: '事项菜单：事项 A' }))
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole('menu')).toBeNull()
  })

  it('uses the original Kooky warning before deleting a Task', async () => {
    const user = userEvent.setup()
    const target = commands()
    render(<TaskSidebar projection={fixture()} commands={target} />)

    await user.click(screen.getByRole('button', { name: '事项菜单：事项 A' }))
    await user.click(screen.getByRole('menuitem', { name: '删除' }))
    expect(screen.getByText('删除 "事项 A" 会丢失该事项下所有终端会话，但不会删除本地目录。 是否继续？')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '确定' }))

    expect(target.deleteTask).toHaveBeenCalledWith('task-a')
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
    , unreadByTask: {}
  }
}

function commands(): HierarchyCommands {
  return {
    activateWorkspace: vi.fn(), createWorkspace: vi.fn(), renameWorkspace: vi.fn(),
    removeWorkspace: vi.fn(), activateTask: vi.fn(), createTask: vi.fn(),
    renameTask: vi.fn(), reorderTask: vi.fn(), deleteTask: vi.fn(),
    activateScene: vi.fn(), createScene: vi.fn(), renameScene: vi.fn(),
    reorderScene: vi.fn(), closeScene: vi.fn(), splitSession: vi.fn(),
    putGeometry: vi.fn(),
    activateSession: vi.fn(), deleteSession: vi.fn(), detachSession: vi.fn(),
    returnSession: vi.fn(), setPermissionMode: vi.fn(), setModel: vi.fn()
  }
}
