// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { HierarchyShell } from './HierarchyShell'
import type { HierarchyProjection } from './hierarchy-types'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: ({ sessionId, inputDisabled }: { sessionId: string; inputDisabled: boolean }) =>
    <div data-testid={`xterm-${sessionId}`} data-input-disabled={inputDisabled} />
}))

afterEach(() => {
  cleanup()
  Reflect.deleteProperty(window, 'matouDesktop')
  Reflect.deleteProperty(window, 'matouE2e')
  window.history.replaceState({}, '', '/')
})

describe('PRD 05 hierarchy shell', () => {
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

    await user.click(screen.getByRole('button', { name: '切换工作区' }))
    await user.click(screen.getByRole('menuitem', { name: 'Workspace B' }))
    expect(screen.getByTestId('active-task').textContent).toContain('事项 B1')
    await user.click(screen.getByRole('button', { name: '切换工作区' }))
    await user.click(screen.getByRole('menuitem', { name: 'Workspace A' }))

    expect(screen.getByTestId('workspace-name').textContent).toContain('Workspace A')
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
    expect(screen.getByRole('button', { name: '事项' })).toHaveProperty('disabled', true)
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
