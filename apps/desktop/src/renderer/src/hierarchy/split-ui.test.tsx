// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { SceneTabBar, type SceneCommands } from './SceneTabBar'
import { SplitDivider } from './SplitDivider'
import type { HierarchyProjection } from './hierarchy-types'

let resizeObserverCallback: ResizeObserverCallback | undefined

afterEach(() => {
  cleanup()
  resizeObserverCallback = undefined
  vi.unstubAllGlobals()
})
beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn()
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: ResizeObserverCallback) { resizeObserverCallback = callback }
    observe() {}
    disconnect() {}
  })
})

describe('Scene tabs and split actions', () => {
  it('creates a right-hand Shell sibling for the active terminal', async () => {
    const user = userEvent.setup()
    const commands = sceneCommands()
    render(<SceneTabBar projection={fixture(2)} commands={commands} />)

    await user.click(screen.getByRole('button', { name: '横向新增 Shell' }))
    expect(commands.createShellSibling).toHaveBeenCalledWith('scene-1', 'session-1')
  })

  it('creates a Shell inside the currently open child level after its last card was removed', async () => {
    const user = userEvent.setup()
    const commands = sceneCommands()
    render(<SceneTabBar projection={fixture(2)} commands={commands}
      levelParentSessionId="parent-session" />)

    await user.click(screen.getByRole('button', { name: '横向新增 Shell' }))
    expect(commands.createShellSibling).toHaveBeenCalledWith(
      'scene-1', 'session-1', 'parent-session'
    )
  })

  it('removes the downward split entry from the session-canvas toolbar', () => {
    render(<SceneTabBar projection={fixture(2)} commands={sceneCommands()} />)
    expect(screen.queryByRole('button', { name: '垂直分屏' })).toBeNull()
  })

  it('keeps the add-tab control beside the last visible tab like reference product', () => {
    render(<SceneTabBar projection={fixture(2)} commands={sceneCommands()} />)

    const add = screen.getByRole('button', { name: '新建页签' })
    expect(add.parentElement?.classList.contains('tab-bar-left')).toBe(true)
  })

  it('uses the approved graph and panel icons while removing the unused file toolbar action', () => {
    render(<SceneTabBar projection={fixture(2)} commands={sceneCommands()} onOpenDag={vi.fn()} />)

    expect(screen.getByRole('button', { name: '打开会话 DAG' })
      .querySelector('svg')?.dataset.icon).toBe('graph-ring')
    expect(screen.getByRole('button', { name: '横向新增 Shell' })
      .querySelector('svg')?.dataset.icon).toBe('panel-right-open')
    expect(screen.getByRole('button', { name: '新建页签' })
      .querySelector('svg')?.dataset.icon).toBe('plus')
    expect(screen.getByRole('button', { name: '关闭页签：页签 1' })
      .querySelector('svg')?.dataset.icon).toBe('x')
    expect(screen.queryByRole('button', { name: '文件' })).toBeNull()
  })

  it('pins the overflow and add controls when canvases exceed the visible tab strip like reference product', () => {
    const commands = sceneCommands()
    const { container } = render(<SceneTabBar projection={fixture(8)} commands={commands} />)
    const strip = container.querySelector<HTMLElement>('.scene-tabs')!
    setHorizontalGeometry(strip, 0, 400)
    Object.defineProperties(strip, {
      clientWidth: { configurable: true, value: 400 },
      scrollWidth: { configurable: true, value: 824 }
    })
    container.querySelectorAll<HTMLElement>('[data-scene-id]').forEach((tab, index) => {
      setHorizontalGeometry(tab, index * 100, index * 100 + 100)
    })

    act(() => resizeObserverCallback?.([], {} as ResizeObserver))

    expect(screen.getAllByRole('tab')).toHaveLength(8)
    expect(screen.getByRole('button', { name: '更多页签' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '新建页签' }).parentElement?.classList
      .contains('tab-bar-overflow-actions')).toBe(true)

    fireEvent.click(screen.getByRole('button', { name: '更多页签' }))
    expect(screen.getAllByRole('menuitem').map((item) => item.textContent)).toEqual([
      '页签 5', '页签 6', '页签 7', '页签 8'
    ])

    fireEvent.click(screen.getByRole('menuitem', { name: '页签 6' }))
    expect(commands.activateScene).toHaveBeenCalledWith('scene-6')
    expect(Element.prototype.scrollIntoView).toHaveBeenLastCalledWith({
      inline: 'center', block: 'nearest', behavior: 'smooth'
    })
  })

  it('reveals the active canvas when switching reaches a tab beyond the viewport', () => {
    const projection = fixture(20)
    projection.navigation.sceneByTask['task-1'] = 'scene-20'
    const scrollIntoView = vi.mocked(Element.prototype.scrollIntoView)

    render(<SceneTabBar projection={projection} commands={sceneCommands()} />)

    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('页签 20')
    expect(scrollIntoView).toHaveBeenCalledWith({ inline: 'nearest', block: 'nearest' })
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

    expect(screen.getByText('当前事项下已存在名为"页签 2"的标签页')).toBeTruthy()
    expect(screen.getByRole('button', { name: '确定' })).toHaveProperty('disabled', true)
  })

  it('opens the compact canvas menu at the right-click position', () => {
    render(<SceneTabBar projection={fixture(2)} commands={sceneCommands()} />)
    const tab = screen.getByRole('tab', { name: '页签 1' })

    fireEvent.contextMenu(tab, { clientX: 236, clientY: 74 })

    const menu = screen.getByRole('menu')
    expect(menu.classList.contains('scene-tab-menu')).toBe(true)
    expect(menu.getAttribute('style')).toContain('left: 236px')
    expect(menu.getAttribute('style')).toContain('top: 74px')
    expect(menu.parentElement).toBe(document.body)
  })

  it('opens canvas rename on a discoverable tab double-click and pins the chosen name', async () => {
    const user = userEvent.setup()
    const commands = sceneCommands()
    render(<SceneTabBar projection={fixture(2)} commands={commands} />)

    await user.dblClick(screen.getByRole('tab', { name: '页签 1' }))
    const input = screen.getByRole('textbox', { name: '页签名称' })
    await user.clear(input)
    await user.type(input, '登录方案')
    await user.click(screen.getByRole('button', { name: '确定' }))

    expect(commands.renameScene).toHaveBeenCalledWith('scene-1', '登录方案')
  })

  it('refreshes an unpinned generated tab from the focused Session path and keeps the full path on hover', () => {
    const projection = fixture(1)
    const longPath = `/repo/${'nested/'.repeat(28)}project`
    projection.scenes[0] = {
      ...projection.scenes[0]!, name: 'Shell · /old', titlePinned: false
    }
    projection.sessionGraphs = {
      'scene-1': {
        sceneId: 'scene-1', focusedSessionId: 'session-1',
        nodes: [{ ...graphNode('session-1', 'idle'), cwd: longPath }], edges: []
      }
    }
    render(<SceneTabBar projection={projection} commands={sceneCommands()} />)

    const tab = screen.getByRole('tab', { name: `Shell · ${longPath}` })
    expect(tab.getAttribute('title')).toContain(longPath)
  })

  it('shows affected running and waiting counts before closing a busy canvas', async () => {
    const user = userEvent.setup()
    const commands = sceneCommands()
    const projection = fixture(2)
    projection.sessionGraphs = {
      'scene-1': {
        sceneId: 'scene-1', nodes: [
          graphNode('running', 'running'), graphNode('waiting', 'needs-input'), graphNode('idle', 'idle')
        ], edges: []
      }
    }
    render(<SceneTabBar projection={projection} commands={commands} />)

    await user.click(screen.getByRole('button', { name: '关闭页签：页签 1' }))
    expect(screen.getByRole('alertdialog', { name: '关闭画布' }).textContent)
      .toContain('1 个运行中会话和 1 个待输入会话')
    await user.click(screen.getByRole('button', { name: '取消' }))
    expect(commands.closeScene).not.toHaveBeenCalled()
  })

  it('asks for confirmation before closing an idle canvas with sessions', async () => {
    const user = userEvent.setup()
    const commands = sceneCommands()
    const projection = fixture(2)
    projection.sessionGraphs = {
      'scene-1': {
        sceneId: 'scene-1', nodes: [graphNode('idle', 'idle')], edges: []
      }
    }
    render(<SceneTabBar projection={projection} commands={commands} />)

    await user.click(screen.getByRole('button', { name: '关闭页签：页签 1' }))
    const dialog = screen.getByRole('alertdialog', { name: '关闭画布' })
    expect(dialog.textContent).toContain('“页签 1”下的 1 个会话会全部从界面移除')
    expect(commands.closeScene).not.toHaveBeenCalled()

    const confirm = screen.getByRole('button', { name: '关闭画布' })
    expect(confirm.classList.contains('is-danger')).toBe(true)
    await user.click(confirm)
    expect(commands.closeScene).toHaveBeenCalledWith('scene-1', true)
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
    createCanvas: vi.fn(), createShellSibling: vi.fn(),
    reorderScene: vi.fn(), closeScene: vi.fn(), splitSession: vi.fn()
  }
}

function graphNode(sessionId: string, workStatus: 'running' | 'needs-input' | 'idle') {
  return {
    sessionId, sceneId: 'scene-1', currentMode: 'shell' as const, workStatus,
    providerRestoreState: 'none' as const, canFork: false, title: sessionId,
    cwd: '/tmp', activeChildCount: 0, stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}

function setHorizontalGeometry(element: HTMLElement, left: number, right: number) {
  element.getBoundingClientRect = () => ({
    left, right, top: 0, bottom: 40, width: right - left, height: 40, x: left, y: 0,
    toJSON: () => ({})
  })
}
