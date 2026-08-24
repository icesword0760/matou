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

afterEach(() => { cleanup(); Reflect.deleteProperty(window, 'matouDesktop') })

describe('PRD 05 hierarchy shell', () => {
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

    expect(screen.getByText('工作区目录不可用')).toBeTruthy()
    expect(screen.getByTestId('xterm-session-a1').dataset.inputDisabled).toBe('true')
    expect(screen.getByTestId('xterm-session-a2')).toBeTruthy()
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
