// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { HierarchyShell } from './HierarchyShell'
import type { HierarchyProjection } from './hierarchy-types'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: ({ sessionId, inputDisabled, themeKey, fontSize, searchRequest, focusRequest }: {
    sessionId: string; inputDisabled: boolean; themeKey?: string; fontSize?: number
    searchRequest?: { query: string; direction: string; sequence: number }
    focusRequest?: number
  }) => <div data-testid={`xterm-${sessionId}`} data-input-disabled={inputDisabled}
    data-theme={themeKey} data-font-size={fontSize} data-search-query={searchRequest?.query}
    data-search-direction={searchRequest?.direction} data-focus-request={focusRequest} />
}))

beforeEach(() => {
  Object.defineProperty(navigator, 'platform', { configurable: true, value: 'MacIntel' })
})

afterEach(() => {
  cleanup()
  Reflect.deleteProperty(window, 'matouDesktop')
  Reflect.deleteProperty(window, 'matouE2e')
  window.history.replaceState({}, '', '/')
})

describe('PRD 05 hierarchy shell', () => {
  it('starts with the requested white skin and cycles the whole CLI with Kooky Cmd+I', () => {
    render(<HierarchyShell fixture={fixture()} />)

    expect(screen.getByRole('main').getAttribute('data-theme')).toBe('light')
    expect(screen.getByTestId('xterm-session-a1').dataset.theme).toBe('light')

    fireEvent.keyDown(document, { key: 'i', metaKey: true })
    expect(screen.getByRole('main').getAttribute('data-theme')).toBe('dark')
    expect(screen.getByTestId('xterm-session-a1').dataset.theme).toBe('dark')
  })

  it('opens the Kooky shortcut floating panel with Cmd+/ and double Option', () => {
    render(<HierarchyShell fixture={fixture()} />)

    fireEvent.keyDown(document, { key: '/', metaKey: true })
    expect(screen.getByRole('dialog', { name: '快捷键列表' })).toBeTruthy()
    expect(screen.getByRole('img', { name: '快捷键说明' }).getAttribute('data-theme')).toBe('light')

    fireEvent.keyDown(document, { key: '/', metaKey: true })
    expect(screen.queryByRole('dialog', { name: '快捷键列表' })).toBeNull()
    expect(screen.getByTestId('xterm-session-a1').dataset.focusRequest).toBe('1')
    fireEvent.keyDown(document, { key: 'Alt', altKey: true })
    fireEvent.keyDown(document, { key: 'Alt', altKey: true })
    expect(screen.getByRole('dialog', { name: '快捷键列表' })).toBeTruthy()
  })

  it('maps Kooky tab, split, pane, search, and font shortcuts onto Matou Scenes and Sessions', () => {
    const data = fixture()
    const first = data.sceneSnapshots![0]!
    first.scene.rootNodeId = 'split-a1'
    first.nodes = [
      { id: 'split-a1', sceneId: first.scene.id, kind: 'split', direction: 'horizontal', ordinal: 0 },
      { id: 'node-a1', sceneId: first.scene.id, parentNodeId: 'split-a1', kind: 'mount', ordinal: 0 },
      { id: 'node-a1b', sceneId: first.scene.id, parentNodeId: 'split-a1', kind: 'mount', ordinal: 1 }
    ]
    first.mounts.push({ id: 'mount-a1b', sceneId: first.scene.id, sceneNodeId: 'node-a1b', sessionId: 'session-a2' })
    render(<HierarchyShell fixture={data} />)
    const initialPaneCount = screen.getAllByTestId('terminal-pane').length

    fireEvent.keyDown(document, { key: ']', metaKey: true })
    expect(screen.getAllByTestId('terminal-pane').find(({ dataset }) => dataset.active === 'true')?.textContent)
      .toContain('终端 A2')

    fireEvent.keyDown(document, { key: ']', metaKey: true, shiftKey: true })
    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('页签 A2')
    fireEvent.keyDown(document, { key: '1', metaKey: true })
    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('页签 A1')

    fireEvent.keyDown(document, { key: 't', metaKey: true })
    expect(screen.getAllByRole('tab')).toHaveLength(3)
    expect(screen.getAllByTestId('terminal-pane')).toHaveLength(initialPaneCount + 1)
    fireEvent.keyDown(document, { key: 'd', metaKey: true })
    expect(screen.getAllByTestId('terminal-pane')).toHaveLength(initialPaneCount + 2)

    fireEvent.keyDown(document, { key: 'f', metaKey: true })
    expect(screen.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' })).toBeTruthy()

    fireEvent.keyDown(document, { key: '+', metaKey: true })
    expect(screen.getByTestId('xterm-session-a1').dataset.fontSize).toBe('12')
    fireEvent.keyDown(document, { key: '0', metaKey: true })
    expect(screen.getByTestId('xterm-session-a1').dataset.fontSize).toBe('11')
  })

  it('switches panes by their actual split direction instead of wrapping linearly', () => {
    const data = fixture()
    const first = data.sceneSnapshots![0]!
    first.scene.rootNodeId = 'split-a1'
    first.nodes = [
      { id: 'split-a1', sceneId: first.scene.id, kind: 'split', direction: 'horizontal', ordinal: 0 },
      { id: 'node-a1', sceneId: first.scene.id, parentNodeId: 'split-a1', kind: 'mount', ordinal: 0 },
      { id: 'node-a1b', sceneId: first.scene.id, parentNodeId: 'split-a1', kind: 'mount', ordinal: 1 }
    ]
    first.mounts.push({ id: 'mount-a1b', sceneId: first.scene.id, sceneNodeId: 'node-a1b', sessionId: 'session-a2' })
    render(<HierarchyShell fixture={data} />)

    fireEvent.keyDown(document, { key: 'ArrowUp', metaKey: true, altKey: true })
    expect(screen.getByTestId('xterm-session-a1').closest('[data-active="true"]')).toBeTruthy()

    fireEvent.keyDown(document, { key: 'ArrowRight', metaKey: true, altKey: true })
    expect(screen.getAllByTestId('xterm-session-a2').some((surface) =>
      surface.closest('[data-active="true"]') && !surface.closest('[hidden]')
    )).toBe(true)
  })

  it('keeps the Kooky Ctrl+Tab Scene switching behavior on Windows', () => {
    Object.defineProperty(navigator, 'platform', { configurable: true, value: 'Win32' })
    render(<HierarchyShell fixture={fixture()} />)

    fireEvent.keyDown(document, { key: 'Tab', ctrlKey: true })
    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('页签 A2')

    fireEvent.keyDown(document, { key: 'Tab', ctrlKey: true, shiftKey: true })
    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('页签 A1')
  })

  it('moves the active Scene and closes only the active split Session with Kooky shortcuts', () => {
    render(<HierarchyShell fixture={fixture()} />)

    fireEvent.keyDown(document, { key: 'ArrowRight', metaKey: true, shiftKey: true })
    expect(screen.getAllByRole('tab').map((tab) => tab.textContent)).toEqual(['页签 A2', '页签 A1'])

    fireEvent.keyDown(document, { key: 'd', metaKey: true })
    const splitSurface = screen.getAllByTestId(/xterm-/).find(({ dataset }) =>
      dataset.testid?.startsWith('xterm-fixture-split-session-')
    )
    expect(splitSurface).toBeTruthy()

    fireEvent.keyDown(document, { key: 'w', metaKey: true })
    expect(screen.queryByTestId(splitSurface!.dataset.testid!)).toBeNull()
    expect(screen.getAllByTestId(/xterm-/).map(({ dataset }) => dataset.testid)).toEqual([
      'xterm-session-a2', 'xterm-session-a1'
    ])
  })

  it('keeps Kooky font boundaries while zooming by shortcut', () => {
    render(<HierarchyShell fixture={fixture()} />)

    for (let index = 0; index < 5; index += 1) fireEvent.keyDown(document, { key: '-', metaKey: true })
    expect(screen.getByTestId('xterm-session-a1').dataset.fontSize).toBe('10')

    for (let index = 0; index < 20; index += 1) fireEvent.keyDown(document, { key: '+', metaKey: true })
    expect(screen.getByTestId('xterm-session-a1').dataset.fontSize).toBe('24')
  })

  it('routes the Kooky search bar to the focused Session only', async () => {
    render(<HierarchyShell fixture={fixture()} />)
    fireEvent.keyDown(document, { key: 'f', metaKey: true })

    await userEvent.setup().type(screen.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' }), 'MATOU_TOKEN')

    expect(screen.getByTestId('xterm-session-a1').dataset.searchQuery).toBe('MATOU_TOKEN')
    expect(screen.getByTestId('xterm-session-a1').dataset.searchDirection).toBe('next')
    expect(screen.getByTestId('xterm-session-a2').dataset.searchQuery).toBeUndefined()
  })

  it('keeps Kooky search option shortcuts while the search field is open', () => {
    render(<HierarchyShell fixture={fixture()} />)
    fireEvent.keyDown(document, { key: 'f', metaKey: true })
    const search = screen.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' })

    fireEvent.keyDown(search, { key: 'c', metaKey: true })
    expect(screen.getByRole('button', { name: '大小写敏感' }).className).toContain('is-active')

    fireEvent.keyDown(search, { key: 'r', metaKey: true })
    expect(screen.getByRole('button', { name: '正则表达式' }).className).toContain('is-active')
  })

  it('returns keyboard focus to the terminal after the search bar closes', () => {
    render(<HierarchyShell fixture={fixture()} />)
    fireEvent.keyDown(document, { key: 'f', metaKey: true })

    fireEvent.keyDown(screen.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' }), { key: 'Escape' })

    expect(screen.queryByRole('textbox', { name: '搜索当前 Tab 的终端内容' })).toBeNull()
    expect(screen.getByTestId('xterm-session-a1').dataset.focusRequest).toBe('1')
  })

  it('shows only the focused Session HUD and replaces it in one render when focus changes', async () => {
    const data = fixture()
    data.sessionHuds = [
      { sessionId: 'session-a1', mode: 'shell', shell: 'zsh', cwd: '/tmp/a', startedAt: Date.now() },
      {
        sessionId: 'session-a2', mode: 'agent', permissionMode: 'plan', modelStrategy: 'opusplan',
        cwd: '/tmp/b', startedAt: Date.now()
      }
    ]
    const first = data.sceneSnapshots![0]!
    first.scene.rootNodeId = 'split-a1'
    first.nodes = [
      { id: 'split-a1', sceneId: first.scene.id, kind: 'split', direction: 'horizontal', ordinal: 0 },
      { id: 'node-a1', sceneId: first.scene.id, parentNodeId: 'split-a1', kind: 'mount', ordinal: 0 },
      { id: 'node-a1b', sceneId: first.scene.id, parentNodeId: 'split-a1', kind: 'mount', ordinal: 1 }
    ]
    first.mounts.push({ id: 'mount-a1b', sceneId: first.scene.id, sceneNodeId: 'node-a1b', sessionId: 'session-a2' })
    render(<HierarchyShell fixture={data} />)

    expect(screen.getByLabelText('快捷指令栏').querySelector('[data-hud-mode="shell"]')).toBeTruthy()
    await userEvent.setup().click(screen.getAllByTestId('terminal-pane')[1]!)
    expect(screen.getByLabelText('快捷指令栏').querySelector('[data-hud-mode="agent"]')).toBeTruthy()
    expect(screen.queryByText('zsh')).toBeNull()
    expect(screen.getByRole('button', { name: /当前权限模式：Plan Mode/ })).toBeTruthy()
  })

  it('opens the named Fork workflow from a valid Claude title line and restores terminal focus on cancel', async () => {
    const data = fixture()
    data.sessions[0] = { ...data.sessions[0]!, kind: 'claude-code', title: 'Claude 主会话' }
    data.sessionHuds = [{
      sessionId: 'session-a1', mode: 'agent', cwd: '/tmp/a', gitBranch: 'main',
      startedAt: 1, resumable: true
    }]
    data.sessionGraphs = {
      'scene-a1': {
        sceneId: 'scene-a1', focusedSessionId: 'session-a1', edges: [],
        nodes: [{
          sessionId: 'session-a1', sceneId: 'scene-a1', currentMode: 'claude-code',
          workStatus: 'idle', providerRestoreState: 'none', canFork: true,
          title: 'Claude 主会话', cwd: '/tmp/a', activeChildCount: 0,
          historicalChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
          latestLines: [], lastUserInteractionSeq: 0
        }]
      }
    }
    render(<HierarchyShell fixture={data} />)
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '从“Claude 主会话”创建子分支' }))
    expect(screen.getByRole('dialog', { name: '创建子会话分支' })).toBeTruthy()
    expect((screen.getByRole('radio', { name: /从新工作树创建/ }) as HTMLInputElement).disabled).toBe(false)
    await user.click(screen.getByRole('button', { name: '取消' }))
    expect(screen.queryByRole('dialog', { name: '创建子会话分支' })).toBeNull()
    expect(screen.getByTestId('xterm-session-a1').dataset.focusRequest).toBe('1')
  })

  it('exposes a test-only Agent notification path through the real hierarchy UI', async () => {
    window.history.replaceState({}, '', '/?e2e=1')
    render(<HierarchyShell fixture={fixture()} />)

    window.matouE2e!.pushNotification({
      eventId: 'e2e-completed', eventType: 'completed', title: 'Claude Code', body: 'E2E 任务完成',
      workspaceId: 'workspace-a', taskId: 'task-a1', sceneId: 'scene-a1', sessionId: 'session-a1'
    })

    await userEvent.setup().click(screen.getByRole('button', { name: '通知中心' }))
    expect(screen.getByRole('button', { name: '打开通知：E2E 任务完成' })).toBeTruthy()
  })

  it('keeps startup and partial layout hydration silent for PRD 04', () => {
    const loading = render(<HierarchyShell />)
    expect(screen.queryByText(/恢复|加载/)).toBeNull()
    loading.unmount()

    const data = fixture()
    data.sceneSnapshots = []
    render(<HierarchyShell fixture={data} />)
    expect(screen.queryByText(/恢复|加载/)).toBeNull()
  })

  it('restores each Workspace navigation context after switching away', async () => {
    const user = userEvent.setup()
    render(<HierarchyShell fixture={fixture()} />)

    await user.click(screen.getByRole('button', { name: 'Workspace B' }))
    expect(screen.getByTestId('active-task').textContent).toContain('事项 B1')
    await user.click(screen.getByRole('button', { name: 'Workspace A' }))

    expect(screen.getByRole('region', { name: 'Workspace A 工作现场' })).toBeTruthy()
    expect(screen.getByTestId('active-task').textContent).toContain('事项 A1')
    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('页签 A1')
    const activePane = screen.getAllByTestId('terminal-pane').find(({ dataset }) => dataset.active === 'true')
    expect(activePane?.textContent).toContain('终端 A1')
  })

  it('keeps an inactive Scene terminal mounted and blocks input for an invalid path', () => {
    const data = fixture()
    data.pathStates = [{ workspaceId: 'workspace-a', status: 'invalid', reason: 'missing' }]
    render(<HierarchyShell fixture={data} />)

    expect(screen.getByText('工作区目录不可用，请先在本地恢复原路径，或移出该工作区')).toBeTruthy()
    expect(screen.getByTestId('xterm-session-a1').dataset.inputDisabled).toBe('true')
    expect(screen.getByTestId('xterm-session-a2')).toBeTruthy()
    expect(screen.getByRole('button', { name: '在 Workspace A 中新增事项' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '新建页签' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '水平分屏' })).toHaveProperty('disabled', true)
  })

  it('restores a persisted divider ratio for an independent Scene layout', () => {
    const data = fixture()
    const first = data.sceneSnapshots![0]!
    first.scene.rootNodeId = 'split-a1'
    first.scene.layoutRevision = 2
    first.nodes = [
      { id: 'split-a1', sceneId: first.scene.id, kind: 'split', direction: 'horizontal', ordinal: 0 },
      { id: 'node-a1', sceneId: first.scene.id, parentNodeId: 'split-a1', kind: 'mount', ordinal: 0 },
      { id: 'node-a1b', sceneId: first.scene.id, parentNodeId: 'split-a1', kind: 'mount', ordinal: 1 }
    ]
    first.mounts.push({ id: 'mount-a1b', sceneId: first.scene.id, sceneNodeId: 'node-a1b', sessionId: 'session-a2' })
    first.geometry = [{ sceneId: first.scene.id, ownerKey: 'node:split-a1', layoutRevision: 2, geometry: { ratio: 0.35 }, now: 20 }]

    render(<HierarchyShell fixture={data} />)

    expect(screen.getByTestId('split-child-split-a1-0').style.flexBasis).toBe('35%')
  })

  it('shows an ownership placeholder while the same Session lives in a detached window', () => {
    const data = fixture()
    const first = data.sceneSnapshots![0]!
    first.mounts[0]!.sceneWindowId = 'detached-1'
    first.windows.push({ id: 'detached-1', sceneId: first.scene.id, state: 'detached' })
    Object.defineProperty(window, 'matouDesktop', { configurable: true, value: {
      selectWorkspaceDirectory: vi.fn(), hideWindow: vi.fn(), showWindow: vi.fn(),
      createDetachedTerminalWindow: vi.fn(), closeDetachedTerminalWindow: vi.fn(),
      onDetachedWindowClosed: vi.fn(() => () => {})
    } })
    render(<HierarchyShell fixture={data} />)

    expect(screen.getByTestId('detached-placeholder').textContent).toContain('已脱出')
    expect(screen.queryByTestId('xterm-session-a1')).toBeNull()
  })
})

function fixture(): HierarchyProjection {
  return {
    windowId: 'window-1',
    workspaces: [
      { id: 'workspace-a', name: 'Workspace A', rootDirectory: '/tmp/a' },
      { id: 'workspace-b', name: 'Workspace B', rootDirectory: '/tmp/b' }
    ],
    tasks: [
      { id: 'task-a1', workspaceId: 'workspace-a', title: '事项 A1' },
      { id: 'task-b1', workspaceId: 'workspace-b', title: '事项 B1' }
    ],
    scenes: [
      { id: 'scene-a1', taskId: 'task-a1', name: '页签 A1', rootNodeId: 'node-a1' },
      { id: 'scene-a2', taskId: 'task-a1', name: '页签 A2', rootNodeId: 'node-a2' },
      { id: 'scene-b1', taskId: 'task-b1', name: '页签 B1', rootNodeId: 'node-b1' }
    ],
    sessions: [
      { id: 'session-a1', taskId: 'task-a1', title: '终端 A1', executionContextId: 'context-a' },
      { id: 'session-a2', taskId: 'task-a1', title: '终端 A2', executionContextId: 'context-a' },
      { id: 'session-b1', taskId: 'task-b1', title: '终端 B1', executionContextId: 'context-b' }
    ],
    sceneSnapshots: [
      snapshot('scene-a1', 'task-a1', '页签 A1', 'node-a1', 'mount-a1', 'session-a1'),
      snapshot('scene-a2', 'task-a1', '页签 A2', 'node-a2', 'mount-a2', 'session-a2'),
      snapshot('scene-b1', 'task-b1', '页签 B1', 'node-b1', 'mount-b1', 'session-b1')
    ],
    pathStates: [], taskPlacements: [],
    navigation: {
      windowId: 'window-1', activeWorkspaceId: 'workspace-a',
      taskByWorkspace: { 'workspace-a': 'task-a1', 'workspace-b': 'task-b1' },
      sceneByTask: { 'task-a1': 'scene-a1', 'task-b1': 'scene-b1' },
      sessionByScene: { 'scene-a1': 'session-a1', 'scene-a2': 'session-a2', 'scene-b1': 'session-b1' }
    }
  }
}

function snapshot(sceneId: string, taskId: string, name: string, nodeId: string, mountId: string, sessionId: string) {
  return {
    scene: { id: sceneId, taskId, name, rootNodeId: nodeId },
    nodes: [{ id: nodeId, sceneId, kind: 'mount' as const, ordinal: 0 }],
    mounts: [{ id: mountId, sceneId, sceneNodeId: nodeId, sessionId }],
    windows: []
  }
}
