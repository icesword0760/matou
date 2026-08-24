// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { SceneTabBar, type SceneCommands } from './SceneTabBar'
import { SplitDivider } from './SplitDivider'
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

  it('requests a lower vertical split for the active terminal', async () => {
    const user = userEvent.setup()
    const commands = sceneCommands()
    render(<SceneTabBar projection={fixture(2)} commands={commands} />)

    await user.click(screen.getByRole('button', { name: '垂直分屏' }))
    expect(commands.splitSession).toHaveBeenCalledWith('scene-1', 'session-1', 'vertical')
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

  it('renames a Scene and blocks a duplicate pinned title', async () => {
    const user = userEvent.setup()
    const commands = sceneCommands()
    const projection = fixture(2)
    projection.scenes[0]!.titlePinned = true
    projection.scenes[1]!.titlePinned = true
    render(<SceneTabBar projection={projection} commands={commands} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('tab', { name: '页签 1' }) })
    await user.click(screen.getByRole('menuitem', { name: '重命名页签' }))
    const input = screen.getByRole('textbox', { name: '页签名称' })
    await user.clear(input)
    await user.type(input, '页签 2')

    expect(screen.getByText('当前事项下已存在名为“页签 2”的页签')).toBeTruthy()
    expect(screen.getByRole('button', { name: '确认' })).toHaveProperty('disabled', true)
  })

  it('measures divider movement against the whole split and keeps pointer capture', () => {
    const onRatio = vi.fn()
    render(<div className="split-node" data-testid="split"><div><SplitDivider direction="horizontal" onRatio={onRatio} /></div></div>)
    const split = screen.getByTestId('split')
    split.getBoundingClientRect = () => ({
      left: 10, top: 0, width: 200, height: 100, right: 210, bottom: 100, x: 10, y: 0,
      toJSON: () => ({})
    })
    const divider = screen.getByRole('separator')
    divider.setPointerCapture = vi.fn()

    fireEvent.pointerDown(divider, { pointerId: 7 })
    fireEvent.pointerMove(divider, { pointerId: 7, buttons: 1, clientX: 70 })

    expect(divider.setPointerCapture).toHaveBeenCalledWith(7)
    expect(onRatio).toHaveBeenCalledWith(0.3)
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
