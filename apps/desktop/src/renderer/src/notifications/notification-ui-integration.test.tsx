// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { SceneTabBar } from '../hierarchy/SceneTabBar'
import { TaskSidebar } from '../hierarchy/TaskSidebar'
import { TerminalPane } from '../hierarchy/TerminalPane'
import type { HierarchyCommands, HierarchyProjection } from '../hierarchy/hierarchy-types'
import { AgentNotificationStore } from './AgentNotificationStore'
import { NotificationProvider } from './NotificationProvider'

describe('Kooky notification hierarchy interactions', () => {
  afterEach(cleanup)

  it('opens below the Workspace header, swaps to the animated icon, and closes outside or on Escape', async () => {
    const user = userEvent.setup()
    const store = notificationStore()
    renderWithStore(<TaskSidebar projection={fixture()} commands={commands()} />, store)

    const trigger = screen.getByRole('button', { name: '通知中心' })
    expect(trigger.querySelector('img')?.getAttribute('src')).toContain('rongzhi_ani.gif')
    await user.click(trigger)
    expect(screen.getByRole('region', { name: '通知中心' })).toBeTruthy()
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole('region', { name: '通知中心' })).toBeNull()

    await user.click(trigger)
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('region', { name: '通知中心' })).toBeNull()
  })

  it('uses notification state for Task badges and Scene dots', () => {
    const store = notificationStore()
    const projection = fixture()
    const { rerender } = renderWithStore(<TaskSidebar projection={projection} commands={commands()} />, store)
    expect(screen.getByText('1').classList.contains('workbench-item__badge')).toBe(true)

    rerender(<NotificationProvider store={store}>
      <SceneTabBar projection={projection} commands={commands()} />
    </NotificationProvider>)
    expect(screen.getByTestId('scene-unread-scene-a')).toBeTruthy()
  })

  it('marks all notifications for a selected Workspace read', async () => {
    const user = userEvent.setup()
    const store = notificationStore('workspace-2')
    const target = commands()
    renderWithStore(<TaskSidebar projection={fixture()} commands={target} />, store)

    await user.click(screen.getByRole('button', { name: 'Backend' }))

    expect(store.unreadForWorkspace('workspace-2')).toBe(0)
    expect(target.activateWorkspace).toHaveBeenCalledWith('workspace-2')
  })

  it('navigates to an existing target, removes only that notification, and closes the center', async () => {
    const user = userEvent.setup()
    const store = notificationStore()
    const target = commands()
    renderWithStore(<TaskSidebar projection={fixture()} commands={target} />, store)

    await user.click(screen.getByRole('button', { name: '通知中心' }))
    await user.click(screen.getByRole('button', { name: '打开通知：需要处理权限' }))

    await waitFor(() => expect(target.activateSession).toHaveBeenCalledWith('session-a'))
    expect(target.activateWorkspace).toHaveBeenCalledWith('workspace-1')
    expect(target.activateTask).toHaveBeenCalledWith('task-a')
    expect(target.activateScene).toHaveBeenCalledWith('scene-a')
    expect(store.snapshot().notifications).toHaveLength(0)
    expect(screen.queryByRole('region', { name: '通知中心' })).toBeNull()
  })

  it('keeps a missing-target notification and shows the exact Kooky warning', async () => {
    const user = userEvent.setup()
    const store = new AgentNotificationStore()
    store.push({
      eventId: 'missing', eventType: 'error', title: 'Claude Code', body: '已删除的面板',
      workspaceId: 'workspace-1', taskId: 'task-a', sceneId: 'scene-missing', sessionId: 'session-missing'
    })
    renderWithStore(<TaskSidebar projection={fixture()} commands={commands()} />, store)

    await user.click(screen.getByRole('button', { name: '通知中心' }))
    await user.click(screen.getByRole('button', { name: '打开通知：已删除的面板' }))

    expect(await screen.findByText('原面板已不存在或不在当前窗口')).toBeTruthy()
    expect(store.snapshot().notifications).toHaveLength(1)
    expect(screen.queryByRole('region', { name: '通知中心' })).toBeNull()
  })

  it('falls back to the nearest available Task when the original upper hierarchy was deleted', async () => {
    const user = userEvent.setup()
    const store = new AgentNotificationStore()
    store.push({
      eventId: 'deleted-task', eventType: 'attention', title: 'Claude Code', body: '旧事项通知',
      workspaceId: 'workspace-1', taskId: 'task-deleted'
    })
    const target = commands()
    renderWithStore(<TaskSidebar projection={fixture()} commands={target} />, store)

    await user.click(screen.getByRole('button', { name: '通知中心' }))
    await user.click(screen.getByRole('button', { name: '打开通知：旧事项通知' }))

    await waitFor(() => expect(target.activateTask).toHaveBeenCalledWith('task-a'))
    expect(store.snapshot().notifications).toHaveLength(0)
  })

  it('resolves the live Scene from the Session when an older notification carries stale layout metadata', async () => {
    const user = userEvent.setup()
    const store = new AgentNotificationStore()
    store.push({
      eventId: 'moved-session', eventType: 'attention', title: 'Claude Code', body: '会话已移动',
      workspaceId: 'workspace-1', taskId: 'task-a', sceneId: 'scene-stale', sessionId: 'session-a'
    })
    const target = commands()
    renderWithStore(<TaskSidebar projection={fixture()} commands={target} />, store)

    await user.click(screen.getByRole('button', { name: '通知中心' }))
    await user.click(screen.getByRole('button', { name: '打开通知：会话已移动' }))

    await waitFor(() => expect(target.activateScene).toHaveBeenCalledWith('scene-a'))
    expect(target.activateSession).toHaveBeenCalledWith('session-a')
    expect(store.snapshot().notifications).toHaveLength(0)
  })

  it('shows a blue ring even for a focused read event and clears all pane notifications on focus', () => {
    const store = new AgentNotificationStore({ now: () => 10_000 })
    store.push({
      eventId: 'focused', eventType: 'completed', title: 'Claude Code', body: '当前面板完成',
      workspaceId: 'workspace-1', taskId: 'task-a', sceneId: 'scene-a', sessionId: 'session-a',
      isFocusedSession: true
    })
    const onActivate = vi.fn()
    renderWithStore(<TerminalPane session={{ id: 'session-a', taskId: 'task-a', title: 'Shell' }} active visible={false}
      workspaceSessionCount={2} taskName="事项 A" onActivate={onActivate} onDelete={vi.fn()} />, store)

    const pane = screen.getByTestId('terminal-pane')
    expect(pane.classList.contains('has-notification')).toBe(true)
    fireEvent.pointerDown(pane)
    expect(store.snapshot().notifications).toHaveLength(0)
    expect(pane.classList.contains('has-notification')).toBe(false)
    expect(onActivate).toHaveBeenCalledWith('session-a')
  })
})

function renderWithStore(node: React.ReactNode, store: AgentNotificationStore) {
  return render(<NotificationProvider store={store}>{node}</NotificationProvider>)
}

function notificationStore(workspaceId = 'workspace-1') {
  const store = new AgentNotificationStore({ now: () => 10_000 })
  store.push({
    eventId: 'permission', eventType: 'permission', title: 'Claude Code', body: '需要处理权限',
    workspaceId, taskId: workspaceId === 'workspace-1' ? 'task-a' : 'task-b',
    sceneId: workspaceId === 'workspace-1' ? 'scene-a' : 'scene-b',
    sessionId: workspaceId === 'workspace-1' ? 'session-a' : 'session-b'
  })
  return store
}

function fixture(): HierarchyProjection {
  return {
    windowId: 'window-1',
    workspaces: [
      { id: 'workspace-1', name: 'Frontend', rootDirectory: '/tmp/frontend' },
      { id: 'workspace-2', name: 'Backend', rootDirectory: '/tmp/backend' }
    ],
    tasks: [
      { id: 'task-a', workspaceId: 'workspace-1', title: '事项 A' },
      { id: 'task-b', workspaceId: 'workspace-2', title: '事项 B' }
    ],
    scenes: [
      { id: 'scene-a', taskId: 'task-a', name: 'Shell' },
      { id: 'scene-b', taskId: 'task-b', name: 'Build' }
    ],
    sessions: [
      { id: 'session-a', taskId: 'task-a', title: 'Shell' },
      { id: 'session-b', taskId: 'task-b', title: 'Build' }
    ],
    sceneSnapshots: [
      { scene: { id: 'scene-a', taskId: 'task-a', name: 'Shell' }, nodes: [], windows: [], mounts: [{ id: 'mount-a', sceneId: 'scene-a', sessionId: 'session-a' }] },
      { scene: { id: 'scene-b', taskId: 'task-b', name: 'Build' }, nodes: [], windows: [], mounts: [{ id: 'mount-b', sceneId: 'scene-b', sessionId: 'session-b' }] }
    ],
    pathStates: [], taskPlacements: [],
    navigation: {
      windowId: 'window-1', activeWorkspaceId: 'workspace-1', taskByWorkspace: { 'workspace-1': 'task-a' },
      sceneByTask: { 'task-a': 'scene-a' }, sessionByScene: { 'scene-a': 'session-a' }
    }
  }
}

function commands(): HierarchyCommands {
  return {
    activateWorkspace: vi.fn(), createWorkspace: vi.fn(), renameWorkspace: vi.fn(), relinkWorkspace: vi.fn(), removeWorkspace: vi.fn(),
    setWorkspacePinned: vi.fn(), reorderPinnedWorkspace: vi.fn(),
    activateTask: vi.fn(), createTask: vi.fn(), renameTask: vi.fn(), reorderTask: vi.fn(), deleteTask: vi.fn(),
    setTaskPinned: vi.fn(), reorderPinnedTask: vi.fn(),
    activateScene: vi.fn(), createScene: vi.fn(), renameScene: vi.fn(), reorderScene: vi.fn(), closeScene: vi.fn(),
    splitSession: vi.fn(), forkSession: vi.fn(),
    createCanvas: vi.fn(), createShellSibling: vi.fn(), createForkChild: vi.fn(), createForkSibling: vi.fn(),
    retryProviderRestore: vi.fn(), reopenHistoricalSession: vi.fn(), getSceneSessionGraph: vi.fn(),
    recordSessionInteraction: vi.fn(), setFocusedSession: vi.fn(),
    putGeometry: vi.fn(), activateSession: vi.fn(), deleteSession: vi.fn(),
    detachSession: vi.fn(), returnSession: vi.fn(), setPermissionMode: vi.fn(), setModel: vi.fn()
  }
}
