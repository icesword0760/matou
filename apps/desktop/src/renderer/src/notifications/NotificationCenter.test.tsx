// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { HierarchyProjection } from '../hierarchy/hierarchy-types'
import { AgentNotificationStore, type AgentNotification } from './AgentNotificationStore'
import { NotificationCenter } from './NotificationCenter'
import { NotificationProvider } from './NotificationProvider'

describe('reference product notification center', () => {
  afterEach(cleanup)

  it('shows the exact empty state and omits the clear action', () => {
    const { container } = renderCenter(new AgentNotificationStore())
    expect(screen.getByRole('heading', { name: '通知 (0)' })).toBeTruthy()
    expect(screen.getByText('暂无通知')).toBeTruthy()
    expect(container.querySelector('.notification-center__empty-img')).toBeNull()
    expect(screen.queryByRole('button', { name: '清空通知' })).toBeNull()
  })

  it('renders newest first with hierarchy, source, role, content, and time', () => {
    let now = new Date(2026, 0, 1, 9, 5).getTime()
    const store = new AgentNotificationStore({ now: () => now })
    store.push({
      eventId: 'old', eventType: 'completed', title: 'Claude Code', body: '旧消息',
      workspaceId: 'workspace-1', taskId: 'task-a', sessionId: 'session-a', teamRole: 'Reviewer'
    })
    now += 60_000
    store.push({
      eventId: 'new', eventType: 'permission', title: 'Claude Code', subtitle: 'Completed in app',
      body: '需要允许读取文件', workspaceId: 'workspace-1', taskId: 'task-a', sessionId: 'session-b'
    })

    renderCenter(store)
    const items = screen.getAllByRole('button', { name: /打开通知：/ })
    expect(items[0]?.textContent).toContain('需要允许读取文件')
    expect(items[1]?.textContent).toContain('旧消息')
    expect(items[0]?.textContent).toContain('Frontend/事项 A')
    expect(items[0]?.textContent).toContain('Claude Code')
    expect(items[0]?.textContent).toContain('Completed in app')
    expect(items[0]?.textContent).toContain('09:06')
    expect(items[1]?.textContent).toContain('Reviewer')
  })

  it('retains malformed entries with reference product unknown hierarchy labels', () => {
    const store = new AgentNotificationStore()
    store.push({ eventId: 'orphan', eventType: 'error', title: 'Claude Code', body: '原面板已删除' })
    renderCenter(store)
    expect(screen.getByText('未知工作区')).toBeTruthy()
    expect(screen.getByText('未知工作台')).toBeTruthy()
  })

  it('clears all, dismisses one without navigating, toggles sound, and closes', async () => {
    const user = userEvent.setup()
    const persist = vi.fn()
    const store = new AgentNotificationStore({ persistSoundEnabled: persist })
    store.push({ eventId: 'a', eventType: 'completed', title: 'Claude Code', body: 'A' })
    store.push({ eventId: 'b', eventType: 'error', title: 'Claude Code', body: 'B' })
    const onClose = vi.fn()
    const onNavigate = vi.fn()
    const { rerender } = renderCenter(store, { onClose, onNavigate })

    await user.click(screen.getAllByRole('button', { name: '清除此通知' })[0]!)
    expect(store.snapshot().notifications).toHaveLength(1)
    expect(onNavigate).not.toHaveBeenCalled()

    await user.click(screen.getByRole('checkbox', { name: '通知声音' }))
    expect(store.snapshot().soundEnabled).toBe(false)
    expect(persist).toHaveBeenCalledWith(false)

    await user.click(screen.getByRole('button', { name: '关闭通知中心' }))
    expect(onClose).toHaveBeenCalledTimes(1)

    rerender(center(store, { onClose, onNavigate }))
    await user.click(screen.getByRole('button', { name: '清空通知' }))
    expect(store.snapshot().notifications).toHaveLength(0)
    expect(screen.getByText('暂无通知')).toBeTruthy()
  })

  it('delegates notification navigation to the hierarchy shell', async () => {
    const user = userEvent.setup()
    const store = new AgentNotificationStore()
    const notification = store.push({
      eventId: 'a', eventType: 'completed', title: 'Claude Code', body: '完成',
      workspaceId: 'workspace-1', taskId: 'task-a', sceneId: 'scene-a', sessionId: 'session-a'
    })!
    const onNavigate = vi.fn()
    renderCenter(store, { onNavigate })

    await user.click(screen.getByRole('button', { name: '打开通知：完成' }))
    expect(onNavigate).toHaveBeenCalledWith(notification)
  })
})

function center(store: AgentNotificationStore, handlers: {
  onClose?: () => void
  onNavigate?: (notification: AgentNotification) => void
} = {}) {
  return <NotificationProvider store={store}>
    <NotificationCenter projection={fixture()} onClose={handlers.onClose ?? vi.fn()}
      onNavigate={handlers.onNavigate ?? vi.fn()} />
  </NotificationProvider>
}

function renderCenter(store: AgentNotificationStore, handlers: {
  onClose?: () => void
  onNavigate?: (notification: AgentNotification) => void
} = {}) {
  return render(center(store, handlers))
}

function fixture(): HierarchyProjection {
  return {
    windowId: 'window-1',
    workspaces: [{ id: 'workspace-1', name: 'Frontend', rootDirectory: '/tmp/frontend' }],
    tasks: [{ id: 'task-a', workspaceId: 'workspace-1', title: '事项 A' }],
    scenes: [], sessions: [], pathStates: [], taskPlacements: [],
    navigation: { windowId: 'window-1', taskByWorkspace: {}, sceneByTask: {}, sessionByScene: {} }
  }
}
