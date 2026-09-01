// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { HierarchyShell, preferredActiveChild } from './HierarchyShell'
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
  Reflect.deleteProperty(document, 'visibilityState')
  window.history.replaceState({}, '', '/')
})

describe('PRD 05 hierarchy shell', () => {
  it('opens global model switching from the new bottom-left settings entry', async () => {
    render(<HierarchyShell fixture={fixture()} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '设置' }))

    expect(screen.getByRole('region', { name: '模型切换设置' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: '点击切换模型' })).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: '关闭设置' }))
    expect(screen.queryByRole('region', { name: '模型切换设置' })).toBeNull()
  })

  it('moves the current session level into the bottom bar without the obsolete add shortcut', () => {
    const data = fixture()
    data.sessionGraphs = {
      'scene-a1': {
        sceneId: 'scene-a1', focusedSessionId: 'session-a1', edges: [],
        nodes: [graphNode('session-a1', '终端 A1')]
      }
    }

    render(<HierarchyShell fixture={data} />)

    const bottomBar = screen.getByLabelText('快捷指令栏')
    expect(within(bottomBar).getByRole('navigation', { name: '会话层级' }).textContent)
      .toContain('根会话 · 1 个会话')
    expect(screen.queryByRole('button', { name: '添加快捷指令' })).toBeNull()
    expect(document.querySelector('.session-level-header')).toBeNull()
  })

  it('opens session management from the card header centered inside the workspace stage', async () => {
    render(<HierarchyShell fixture={fixture()} />)

    await userEvent.setup().click(screen.getByRole('button', {
      name: '载入 Claude Code 会话到“终端 A1”'
    }))

    const dialog = screen.getByRole('dialog', { name: '载入 Claude Code 会话' })
    expect(dialog).toBeTruthy()
    expect(dialog.parentElement?.parentElement?.classList.contains('workspace-stage')).toBe(true)
  })

  it('returns to the parent from the bottom breadcrumb', async () => {
    const data = fixture()
    data.sessions.push({
      id: 'session-child', taskId: 'task-a1', title: '子会话', executionContextId: 'context-a'
    })
    data.sceneSnapshots![0]!.nodes.push({
      id: 'node-child', sceneId: 'scene-a1', kind: 'mount', ordinal: 1
    })
    data.sceneSnapshots![0]!.mounts.push({
      id: 'mount-child', sceneId: 'scene-a1', sceneNodeId: 'node-child', sessionId: 'session-child'
    })
    data.navigation.sessionByScene['scene-a1'] = 'session-child'
    data.sessionGraphs = {
      'scene-a1': {
        sceneId: 'scene-a1', focusedSessionId: 'session-child',
        edges: [{
          parentSessionId: 'session-a1', childSessionId: 'session-child',
          relationKind: 'derived-from', createdAt: 2
        }],
        nodes: [
          graphNode('session-a1', '父会话'),
          { ...graphNode('session-child', '子会话'), parentSessionId: 'session-a1', relationKind: 'derived-from' }
        ]
      }
    }

    render(<HierarchyShell fixture={data} />)

    const breadcrumb = screen.getByRole('navigation', { name: '会话层级' })
    expect(breadcrumb.textContent).toContain('父会话 的子会话 · 1 个会话')
    await userEvent.setup().click(within(breadcrumb).getByRole('button', { name: '返回父会话' }))
    expect(screen.getByRole('region', { name: '会话画布' }).getAttribute('data-parent-session-id'))
      .toBe('')
  })

  it('opens the highest-priority active child represented by the aggregate badge', () => {
    const idle = { ...graphNode('idle-child', '空闲子会话'), workStatus: 'idle' as const }
    const running = { ...graphNode('running-child', '运行子会话'), workStatus: 'running' as const }
    const error = { ...graphNode('error-child', '错误子会话'), workStatus: 'error' as const }
    const archivedError = { ...graphNode('archived-error', '已停止错误'), workStatus: 'error' as const, archivedAt: 1 }

    expect(preferredActiveChild([idle, running, archivedError, error])?.sessionId)
      .toBe('error-child')
  })

  it('starts with the requested white skin and cycles the whole CLI with reference product Cmd+I', () => {
    render(<HierarchyShell fixture={fixture()} />)

    expect(screen.getByRole('main').getAttribute('data-theme')).toBe('light')
    expect(screen.getByTestId('xterm-session-a1').dataset.theme).toBe('light')

    fireEvent.keyDown(document, { key: 'i', metaKey: true })
    expect(screen.getByRole('main').getAttribute('data-theme')).toBe('dark')
    expect(screen.getByTestId('xterm-session-a1').dataset.theme).toBe('dark')
  })

  it('returns keyboard focus to the active terminal when a hidden main window becomes visible', () => {
    let visibility: DocumentVisibilityState = 'hidden'
    Object.defineProperty(document, 'visibilityState', {
      configurable: true, get: () => visibility
    })
    render(<HierarchyShell fixture={fixture()} />)
    const before = Number(screen.getByTestId('xterm-session-a1').dataset.focusRequest ?? 0)

    visibility = 'visible'
    fireEvent(document, new Event('visibilitychange'))

    expect(Number(screen.getByTestId('xterm-session-a1').dataset.focusRequest)).toBeGreaterThan(before)
  })

  it('keeps ordinary navigation available when the native DAG window does not open', async () => {
    const openDagWindow = vi.fn().mockRejectedValue(new Error('native window unavailable'))
    Object.defineProperty(window, 'matouDesktop', { configurable: true, value: {
      openDagWindow,
      onDagShortcut: vi.fn(() => vi.fn()),
      onDagNodeSelected: vi.fn(() => vi.fn()),
      onDetachedWindowClosed: vi.fn(() => vi.fn())
    } })
    render(<HierarchyShell fixture={fixture()} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '打开会话 DAG' }))

    expect((await screen.findByRole('alert')).textContent).toContain('会话关系视图打开失败')
    expect(screen.getByRole('button', { name: '重试打开 DAG' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '横向新增 Shell' })).toBeTruthy()
  })

  it('opens DAG from a restored stopped-only canvas without a running navigation focus', async () => {
    const data = fixture()
    delete data.navigation.sessionByScene['scene-a1']
    data.sessionGraphs = {
      'scene-a1': {
        sceneId: 'scene-a1',
        edges: [{
          parentSessionId: 'history-parent', childSessionId: 'history-child',
          relationKind: 'derived-from', createdAt: 2
        }],
        nodes: [
          { ...graphNode('history-parent', '已停止父会话'), archivedAt: 10, workStatus: 'exited', activeChildCount: 0 },
          {
            ...graphNode('history-child', '已停止子会话'), parentSessionId: 'history-parent',
            relationKind: 'derived-from', archivedAt: 11, workStatus: 'exited'
          }
        ]
      }
    }
    const openDagWindow = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(window, 'matouDesktop', { configurable: true, value: {
      openDagWindow,
      onDagShortcut: vi.fn(() => vi.fn()),
      onDagNodeSelected: vi.fn(() => vi.fn()),
      onDetachedWindowClosed: vi.fn(() => vi.fn())
    } })
    render(<HierarchyShell fixture={data} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '打开会话 DAG' }))

    expect(openDagWindow).toHaveBeenCalledWith({
      mainWindowId: 'window-1', sceneId: 'scene-a1', sessionId: 'history-parent', theme: 'light',
      notificationSessionIds: []
    })
  })

  it('opens the reference product shortcut floating panel with Cmd+/ and double Option', () => {
    render(<HierarchyShell fixture={fixture()} />)

    fireEvent.keyDown(document, { key: '/', metaKey: true })
    expect(screen.getByRole('dialog', { name: '快捷键列表' })).toBeTruthy()
    expect(screen.getByRole('img', { name: '快捷键说明' }).getAttribute('data-theme')).toBe('light')

    fireEvent.keyDown(document, { key: '/', metaKey: true })
    expect(screen.queryByRole('dialog', { name: '快捷键列表' })).toBeNull()
    expect(Number(screen.getByTestId('xterm-session-a1').dataset.focusRequest)).toBeGreaterThanOrEqual(1)
    fireEvent.keyDown(document, { key: 'Alt', altKey: true })
    fireEvent.keyDown(document, { key: 'Alt', altKey: true })
    expect(screen.getByRole('dialog', { name: '快捷键列表' })).toBeTruthy()
  })

  it('maps reference product tab, split, pane, search, and font shortcuts onto Matou Scenes and Sessions', () => {
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

  it('keeps the reference product Ctrl+Tab Scene switching behavior on Windows', () => {
    Object.defineProperty(navigator, 'platform', { configurable: true, value: 'Win32' })
    render(<HierarchyShell fixture={fixture()} />)

    fireEvent.keyDown(document, { key: 'Tab', ctrlKey: true })
    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('页签 A2')

    fireEvent.keyDown(document, { key: 'Tab', ctrlKey: true, shiftKey: true })
    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('页签 A1')
  })

  it('moves the active Scene and closes only the active split Session with reference product shortcuts', () => {
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

  it('keeps reference product font boundaries while zooming by shortcut', () => {
    render(<HierarchyShell fixture={fixture()} />)

    for (let index = 0; index < 5; index += 1) fireEvent.keyDown(document, { key: '-', metaKey: true })
    expect(screen.getByTestId('xterm-session-a1').dataset.fontSize).toBe('10')

    for (let index = 0; index < 20; index += 1) fireEvent.keyDown(document, { key: '+', metaKey: true })
    expect(screen.getByTestId('xterm-session-a1').dataset.fontSize).toBe('24')
  })

  it('routes the reference product search bar to the focused Session only', async () => {
    render(<HierarchyShell fixture={fixture()} />)
    fireEvent.keyDown(document, { key: 'f', metaKey: true })

    await userEvent.setup().type(screen.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' }), 'MATOU_TOKEN')

    expect(screen.getByTestId('xterm-session-a1').dataset.searchQuery).toBe('MATOU_TOKEN')
    expect(screen.getByTestId('xterm-session-a1').dataset.searchDirection).toBe('next')
    expect(screen.getByTestId('xterm-session-a2').dataset.searchQuery).toBeUndefined()
  })

  it('keeps reference product search option shortcuts while the search field is open', () => {
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
    expect(Number(screen.getByTestId('xterm-session-a1').dataset.focusRequest)).toBeGreaterThanOrEqual(1)
  })

  it('returns keyboard focus to the active terminal when a hidden main window is shown again', () => {
    render(<HierarchyShell fixture={fixture()} />)
    const before = Number(screen.getByTestId('xterm-session-a1').dataset.focusRequest)

    fireEvent.focus(window)

    expect(Number(screen.getByTestId('xterm-session-a1').dataset.focusRequest)).toBeGreaterThan(before)
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
          stoppedChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
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
    expect(Number(screen.getByTestId('xterm-session-a1').dataset.focusRequest)).toBeGreaterThanOrEqual(1)
  })

  it('enters the newly forked child level when forking from a nested Claude session', async () => {
    const data = fixture()
    data.sessions[0] = { ...data.sessions[0]!, kind: 'claude-code', title: 'Depth-1' }
    data.sessions.push({
      id: 'session-depth2', taskId: 'task-a1', kind: 'claude-code',
      title: 'Depth-2', executionContextId: 'context-a'
    })
    data.sceneSnapshots![0]!.nodes.push({
      id: 'node-depth2', sceneId: 'scene-a1', kind: 'mount', ordinal: 1
    })
    data.sceneSnapshots![0]!.mounts.push({
      id: 'mount-depth2', sceneId: 'scene-a1', sceneNodeId: 'node-depth2', sessionId: 'session-depth2'
    })
    data.sessionHuds = [
      {
        sessionId: 'session-a1', mode: 'agent', cwd: '/tmp/a', gitBranch: 'main',
        startedAt: 1, resumable: true
      },
      {
        sessionId: 'session-depth2', mode: 'agent', cwd: '/tmp/a', gitBranch: 'main',
        startedAt: 2, resumable: true
      }
    ]
    data.sessionGraphs = {
      'scene-a1': {
        sceneId: 'scene-a1', focusedSessionId: 'session-a1',
        edges: [{
          parentSessionId: 'session-a1', childSessionId: 'session-depth2',
          relationKind: 'forked-from', createdAt: 2
        }],
        nodes: [
          {
            ...graphNode('session-a1', 'Depth-1'), currentMode: 'claude-code', canFork: true,
            childModeCounts: { shell: 0, claudeCode: 1 }
          },
          {
            ...graphNode('session-depth2', 'Depth-2'), currentMode: 'claude-code', canFork: true,
            parentSessionId: 'session-a1', relationKind: 'forked-from'
          }
        ]
      }
    }
    render(<HierarchyShell fixture={data} />)
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '查看 1 个子会话' }))
    expect(screen.getByRole('region', { name: '会话画布' }).getAttribute('data-parent-session-id'))
      .toBe('session-a1')
    await user.click(screen.getByRole('button', { name: '从“Depth-2”创建子分支' }))
    await user.type(screen.getByRole('textbox', { name: '分支名称' }), 'Depth-3')
    await user.click(screen.getByRole('button', { name: '创建分支' }))

    expect(screen.getByRole('region', { name: '会话画布' }).getAttribute('data-parent-session-id'))
      .toBe('session-depth2')
    expect(screen.getByTestId('xterm-fixture-fork-session-scene-a1-3')).toBeTruthy()
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

  it('opens a child notification on its sibling level and keeps the target card in view', async () => {
    window.history.replaceState({}, '', '/?e2e=1')
    const data = fixture()
    data.sessions.push({
      id: 'session-child', taskId: 'task-a1', title: '子会话', executionContextId: 'context-a'
    })
    data.sceneSnapshots![0]!.nodes.push({
      id: 'node-child', sceneId: 'scene-a1', kind: 'mount', ordinal: 1
    })
    data.sceneSnapshots![0]!.mounts.push({
      id: 'mount-child', sceneId: 'scene-a1', sceneNodeId: 'node-child', sessionId: 'session-child'
    })
    data.sessionGraphs = {
      'scene-a1': {
        sceneId: 'scene-a1', focusedSessionId: 'session-a1',
        edges: [{
          parentSessionId: 'session-a1', childSessionId: 'session-child',
          relationKind: 'derived-from', createdAt: 2
        }],
        nodes: [
          graphNode('session-a1', '父会话'),
          { ...graphNode('session-child', '子会话'), parentSessionId: 'session-a1', relationKind: 'derived-from' }
        ]
      }
    }
    render(<HierarchyShell fixture={data} />)
    window.matouE2e!.pushNotification({
      eventId: 'child-completed', eventType: 'completed', title: 'Claude Code', body: '子会话完成',
      workspaceId: 'workspace-a', taskId: 'task-a1', sceneId: 'scene-a1', sessionId: 'session-child'
    })
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '通知中心' }))
    await user.click(screen.getByRole('button', { name: '打开通知：子会话完成' }))

    expect(screen.getByRole('region', { name: '会话画布' }).getAttribute('data-parent-session-id'))
      .toBe('session-a1')
    expect(screen.getByTestId('xterm-session-child')).toBeTruthy()
  })

  it('opens a stopped root notification as the same node instead of inferring its running children', async () => {
    window.history.replaceState({}, '', '/?e2e=1')
    const data = fixture()
    data.sessions.push({
      id: 'history-parent', taskId: 'task-a1', title: '已停止父会话', executionContextId: 'context-a'
    })
    data.sceneSnapshots![0]!.nodes.push({
      id: 'node-history-parent', sceneId: 'scene-a1', kind: 'mount', ordinal: 1
    })
    data.sceneSnapshots![0]!.mounts.push({
      id: 'mount-history-parent', sceneId: 'scene-a1', sceneNodeId: 'node-history-parent', sessionId: 'history-parent'
    })
    data.sessionGraphs = {
      'scene-a1': {
        sceneId: 'scene-a1', focusedSessionId: 'session-a1',
        edges: [{
          parentSessionId: 'history-parent', childSessionId: 'session-a1',
          relationKind: 'derived-from', createdAt: 2
        }],
        nodes: [
          { ...graphNode('history-parent', '已停止父会话'), archivedAt: 10, workStatus: 'exited', activeChildCount: 1 },
          { ...graphNode('session-a1', '活动子会话'), parentSessionId: 'history-parent', relationKind: 'derived-from' }
        ]
      }
    }
    render(<HierarchyShell fixture={data} />)
    window.matouE2e!.pushNotification({
      eventId: 'history-parent-completed', eventType: 'completed', title: 'Claude Code', body: '已停止父会话已完成',
      workspaceId: 'workspace-a', taskId: 'task-a1', sceneId: 'scene-a1', sessionId: 'history-parent'
    })
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '通知中心' }))
    await user.click(screen.getByRole('button', { name: '打开通知：已停止父会话已完成' }))

    expect(await screen.findByText('已停止父会话')).toBeTruthy()
    expect(screen.queryByRole('button', { name: '重新启动' })).toBeNull()
    expect(screen.queryByTestId('xterm-session-a1')).toBeNull()
  })

  it('opens a stopped root node from the DAG with structural removal and no process controls', async () => {
    const data = fixture()
    data.sessionGraphs = {
      'scene-a1': {
        sceneId: 'scene-a1', focusedSessionId: 'session-a1',
        edges: [{
          parentSessionId: 'history-parent', childSessionId: 'session-a1',
          relationKind: 'derived-from', createdAt: 2
        }],
        nodes: [
          { ...graphNode('history-parent', '已停止父会话'), archivedAt: 10, workStatus: 'exited', activeChildCount: 1 },
          { ...graphNode('session-a1', '活动子会话'), parentSessionId: 'history-parent', relationKind: 'derived-from' }
        ]
      }
    }
    let selectDagNode: ((selection: {
      mainWindowId: string; sceneId: string; sessionId: string
    }) => void) | undefined
    Object.defineProperty(window, 'matouDesktop', { configurable: true, value: {
      onDagNodeSelected: vi.fn((listener) => { selectDagNode = listener; return vi.fn() }),
      onDetachedWindowClosed: vi.fn(() => vi.fn())
    } })
    render(<HierarchyShell fixture={data} />)
    expect(screen.getByTestId('xterm-session-a1')).toBeTruthy()

    await act(async () => selectDagNode?.({
      mainWindowId: 'window-1', sceneId: 'scene-a1', sessionId: 'history-parent'
    }))

    expect(await screen.findByText('已停止父会话')).toBeTruthy()
    expect(screen.queryByRole('button', { name: '重新启动' })).toBeNull()
    expect(screen.getByRole('button', { name: '移出节点：已停止父会话' })).toBeTruthy()
    expect(screen.queryByTestId('xterm-session-a1')).toBeNull()
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
    expect(screen.getAllByTestId('xterm-session-a2').length).toBeGreaterThan(0)
    expect(screen.getByRole('button', { name: '在 Workspace A 中新增事项' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '新建页签' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '横向新增 Shell' })).toHaveProperty('disabled', true)
    expect(screen.queryByRole('button', { name: '垂直分屏' })).toBeNull()
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

  it('returns a detached Session when its independent window closes', async () => {
    const data = fixture()
    const first = data.sceneSnapshots![0]!
    first.scene.rootNodeId = 'split-detached'
    first.nodes = [
      { id: 'split-detached', sceneId: first.scene.id, kind: 'split', direction: 'horizontal', ordinal: 0 },
      { id: 'node-a1', sceneId: first.scene.id, parentNodeId: 'split-detached', kind: 'mount', ordinal: 0 },
      { id: 'node-a2', sceneId: first.scene.id, parentNodeId: 'split-detached', kind: 'mount', ordinal: 1 }
    ]
    first.mounts[0]!.sceneWindowId = 'detached-1'
    first.mounts.push({ id: 'mount-a2', sceneId: first.scene.id, sceneNodeId: 'node-a2', sessionId: 'session-a2' })
    first.windows.push({ id: 'detached-1', sceneId: first.scene.id, state: 'detached' })
    let closeListener: ((event: { windowId: string; mainWindowId: string; sessionId: string }) => void) | undefined
    Object.defineProperty(window, 'matouDesktop', { configurable: true, value: {
      selectWorkspaceDirectory: vi.fn(), hideWindow: vi.fn(), showWindow: vi.fn(),
      createDetachedTerminalWindow: vi.fn(), closeDetachedTerminalWindow: vi.fn(),
      onDetachedWindowClosed: vi.fn((listener) => { closeListener = listener; return vi.fn() })
    } })
    render(<HierarchyShell fixture={data} />)

    expect(screen.getByTestId('detached-placeholder')).toBeTruthy()
    closeListener?.({ windowId: 'detached-1', mainWindowId: 'window-1', sessionId: 'session-a1' })

    await vi.waitFor(() => expect(screen.queryByTestId('detached-placeholder')).toBeNull())
    expect(screen.getAllByTestId('xterm-session-a1').length).toBeGreaterThan(0)
    expect(screen.getAllByTestId('xterm-session-a2').length).toBeGreaterThan(0)
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

function graphNode(sessionId: string, title: string) {
  return {
    sessionId, sceneId: 'scene-a1', currentMode: 'shell' as const,
    workStatus: 'idle' as const, providerRestoreState: 'none' as const, canFork: false,
    title, cwd: '/tmp/a', activeChildCount: sessionId === 'session-a1' ? 1 : 0,
    stoppedChildCount: 0, childModeCounts: { shell: sessionId === 'session-a1' ? 1 : 0, claudeCode: 0 },
    latestLines: [], lastUserInteractionSeq: 0
  }
}
