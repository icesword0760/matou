import { describe, expect, it, vi } from 'vitest'

import { AgentNotificationStore } from './AgentNotificationStore'

describe('AgentNotificationStore', () => {
  it('keeps only the latest 1000 notifications per Workspace without affecting other buckets', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now, cooldownMs: 0 })

    for (let index = 0; index < 1_005; index += 1) {
      now += 1
      store.push(event({
        eventId: `workspace-a-${index}`,
        workspaceId: 'workspace-a',
        sessionId: `workspace-a-session-${index}`
      }))
    }
    for (let index = 0; index < 3; index += 1) {
      now += 1
      store.push(event({
        eventId: `workspace-b-${index}`,
        workspaceId: 'workspace-b',
        sessionId: `workspace-b-session-${index}`
      }))
    }
    for (let index = 0; index < 1_005; index += 1) {
      now += 1
      store.push(event({
        eventId: `unassigned-${index}`,
        workspaceId: null,
        sessionId: `unassigned-session-${index}`
      }))
    }

    const notifications = store.snapshot().notifications
    const workspaceA = notifications.filter(({ workspaceId }) => workspaceId === 'workspace-a')
    const workspaceB = notifications.filter(({ workspaceId }) => workspaceId === 'workspace-b')
    const unassigned = notifications.filter(({ workspaceId }) => workspaceId === null)
    expect(workspaceA).toHaveLength(1_000)
    expect(workspaceA.map(({ eventId }) => eventId)).not.toContain('workspace-a-4')
    expect(workspaceA.map(({ eventId }) => eventId)).toContain('workspace-a-5')
    expect(workspaceA.map(({ eventId }) => eventId)).toContain('workspace-a-1004')
    expect(workspaceB).toHaveLength(3)
    expect(unassigned).toHaveLength(1_000)
    expect(unassigned.map(({ eventId }) => eventId)).not.toContain('unassigned-4')
  })

  it('uses timestamp and id to evict the oldest notification deterministically', () => {
    const store = new AgentNotificationStore({ now: () => 1_000, cooldownMs: 0, maxPerWorkspace: 2 })

    store.push(event({ eventId: 'first', sessionId: 'session-first' }))
    store.push(event({ eventId: 'second', sessionId: 'session-second' }))
    store.push(event({ eventId: 'third', sessionId: 'session-third' }))

    expect(store.snapshot().notifications.map(({ eventId }) => eventId)).toEqual(['third', 'second'])
  })

  it('retains old unread notifications but prunes read notifications after 30 days', () => {
    const retention = 30 * 24 * 60 * 60 * 1_000
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now, cooldownMs: 0 })
    store.push(event({ eventId: 'old-unread', sessionId: 'old-session' }))

    now += retention + 1
    expect(store.snapshot().notifications.map(({ eventId }) => eventId)).toEqual(['old-unread'])

    store.markAllRead()
    expect(store.snapshot().notifications).toHaveLength(0)
  })

  it('keeps read notifications through the exact retention boundary', () => {
    const retention = 30 * 24 * 60 * 60 * 1_000
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now, readRetentionMs: retention })
    store.push(event({ eventId: 'focused', sessionId: 'focused-session', isFocusedSession: true }))

    now += retention
    expect(store.snapshot().notifications).toHaveLength(1)
    now += 1
    expect(store.snapshot().notifications).toHaveLength(0)
  })

  it('prunes expired read history on push while preserving equally old unread history', () => {
    const retention = 30 * 24 * 60 * 60 * 1_000
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now, cooldownMs: 0 })
    store.push(event({
      eventId: 'old-read', workspaceId: 'workspace-read', sessionId: 'session-read', isFocusedSession: true
    }))
    store.push(event({
      eventId: 'old-unread', workspaceId: 'workspace-unread', sessionId: 'session-unread'
    }))

    now += retention + 1
    store.push(event({ eventId: 'current', workspaceId: 'workspace-current', sessionId: 'session-current' }))

    expect(store.snapshot().notifications.map(({ eventId }) => eventId)).toEqual(['current', 'old-unread'])
  })

  it('updates a replacement in place without consuming another Workspace slot', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now, cooldownMs: 0, maxPerWorkspace: 1 })
    const initial = store.push(event({
      eventId: 'restore-started', sessionId: 'restore-session', replacementKey: 'restore:session'
    }))!

    now += 1
    const updated = store.push(event({
      eventId: 'restore-failed', eventType: 'error', sessionId: 'restore-session',
      replacementKey: 'restore:session'
    }))!

    expect(updated.id).toBe(initial.id)
    expect(store.snapshot().notifications).toHaveLength(1)
    expect(store.snapshot().notifications[0]?.eventId).toBe('restore-failed')
  })

  it('releases cooldown keys when their last notification is evicted or removed', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now, maxPerWorkspace: 1 })
    const evicted = store.push(event({
      eventId: 'evicted', sessionId: 'session-evicted', cooldownKey: 'capacity-key'
    }))!
    store.push(event({ eventId: 'replacement-slot', sessionId: 'session-other' }))
    expect(store.snapshot().notifications.map(({ id }) => id)).not.toContain(evicted.id)

    now += 1
    const afterEviction = store.push(event({
      eventId: 'after-eviction', sessionId: 'session-evicted', cooldownKey: 'capacity-key'
    }))
    expect(afterEviction).not.toBeNull()

    store.remove(afterEviction!.id)
    now += 1
    expect(store.push(event({
      eventId: 'after-removal', sessionId: 'session-evicted', cooldownKey: 'capacity-key'
    }))).not.toBeNull()
  })

  it('keeps ten 1000-item Workspace buckets bounded during the next prune', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now, cooldownMs: 0 })
    const populationStartedAt = performance.now()
    for (let workspaceIndex = 0; workspaceIndex < 10; workspaceIndex += 1) {
      for (let notificationIndex = 0; notificationIndex < 1_000; notificationIndex += 1) {
        now += 1
        store.push(event({
          eventId: `event-${workspaceIndex}-${notificationIndex}`,
          workspaceId: `workspace-${workspaceIndex}`,
          sessionId: `session-${workspaceIndex}-${notificationIndex}`
        }))
      }
    }
    const populationElapsedMs = performance.now() - populationStartedAt

    const startedAt = performance.now()
    now += 1
    store.push(event({
      eventId: 'workspace-0-overflow', workspaceId: 'workspace-0', sessionId: 'workspace-0-overflow'
    }))
    const elapsedMs = performance.now() - startedAt

    expect(store.snapshot().notifications).toHaveLength(10_000)
    for (let workspaceIndex = 0; workspaceIndex < 10; workspaceIndex += 1) {
      expect(store.snapshot().notifications.filter(({ workspaceId }) => workspaceId === `workspace-${workspaceIndex}`))
        .toHaveLength(1_000)
    }
    expect(Number.isFinite(elapsedMs)).toBe(true)
    expect(elapsedMs).toBeLessThan(populationElapsedMs)
  }, 20_000)

  it('keeps the latest event as the only unread notification for one Session', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now })

    const first = store.push(event({ eventId: 'event-1', eventType: 'completed' }))
    now = 7_000
    const second = store.push(event({ eventId: 'event-2', eventType: 'permission' }))

    expect(first?.read).toBe(true)
    expect(second?.read).toBe(false)
    expect(store.snapshot().notifications.map(({ eventId }) => eventId)).toEqual(['event-2', 'event-1'])
    expect(store.snapshot().unreadCount).toBe(1)
  })

  it('updates one recovery notification in place instead of stacking retry states', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now })

    const failed = store.push(event({
      eventId: 'restore-failed', eventType: 'error',
      title: 'Claude Code 恢复失败', replacementKey: 'provider-restore:session'
    }))!
    now = 1_100
    const retrying = store.push(event({
      eventId: 'restore-retrying', eventType: 'attention',
      title: '正在恢复 Claude Code', replacementKey: 'provider-restore:session', sound: false
    }))!

    expect(retrying.id).toBe(failed.id)
    expect(store.snapshot().notifications).toHaveLength(1)
    expect(store.snapshot().notifications[0]).toMatchObject({
      eventId: 'restore-retrying', title: '正在恢复 Claude Code',
      replacementKey: 'provider-restore:session'
    })

    store.removeByReplacementKey('provider-restore:session')
    expect(store.snapshot().notifications).toHaveLength(0)
  })

  it('drops a repeated event category for the same Session during the five-second cooldown', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now })

    expect(store.push(event({ eventId: 'event-1', eventType: 'waiting' }))).not.toBeNull()
    now = 5_999
    expect(store.push(event({ eventId: 'event-2', eventType: 'waiting' }))).toBeNull()
    now = 6_000
    expect(store.push(event({ eventId: 'event-3', eventType: 'waiting' }))).not.toBeNull()
  })

  it('limits cooldown independently by Session and event category', () => {
    const store = new AgentNotificationStore({ now: () => 1_000 })

    expect(store.push(event({ eventId: 'a', eventType: 'error', sessionId: 'session-a' }))).not.toBeNull()
    expect(store.push(event({ eventId: 'b', eventType: 'permission', sessionId: 'session-a' }))).not.toBeNull()
    expect(store.push(event({ eventId: 'c', eventType: 'error', sessionId: 'session-b' }))).not.toBeNull()
  })

  it('matches reference product by cooling all Notification hooks as one source category', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now })
    expect(store.push(event({ eventId: 'permission', eventType: 'permission', cooldownKey: 'Notification' }))).not.toBeNull()
    now += 1_000
    expect(store.push(event({ eventId: 'error', eventType: 'error', cooldownKey: 'Notification' }))).toBeNull()
    expect(store.snapshot().notifications.map(({ eventId }) => eventId)).toEqual(['permission'])
  })

  it('matches reference product by showing a read indicator without sound for a focused Session event', () => {
    const playSound = vi.fn()
    const store = new AgentNotificationStore({ now: () => 1_000, playSound })

    const notification = store.push(event({ eventId: 'focused', isFocusedSession: true }))

    expect(notification?.read).toBe(true)
    expect(store.snapshot().unreadCount).toBe(0)
    expect(store.sessionHasVisibleIndicator('session')).toBe(true)
    expect(playSound).not.toHaveBeenCalled()
  })

  it('matches reference product by removing the Session notification history when its indicator is dismissed', () => {
    let now = 1_000
    const store = new AgentNotificationStore({ now: () => now })
    store.push(event({ eventId: 'first', eventType: 'completed' }))
    now += 6_000
    store.push(event({ eventId: 'second', eventType: 'permission' }))

    store.dismissSessionIndicator('session')

    expect(store.snapshot().notifications).toHaveLength(0)
    expect(store.sessionHasVisibleIndicator('session')).toBe(false)
  })

  it('matches reference product by marking a whole Workspace read when the user switches to it', () => {
    const store = new AgentNotificationStore({ now: () => 1_000 })
    store.push(event({ eventId: 'target', workspaceId: 'workspace-a', sessionId: 'session-a' }))
    store.push(event({ eventId: 'other', workspaceId: 'workspace-b', sessionId: 'session-b' }))

    store.markWorkspaceRead('workspace-a')

    expect(store.unreadForWorkspace('workspace-a')).toBe(0)
    expect(store.unreadForWorkspace('workspace-b')).toBe(1)
  })

  it('aggregates unread state through Workspace, Task, Scene, and Session', () => {
    const store = new AgentNotificationStore({ now: () => 1_000 })
    store.push(event({ eventId: 'event-a', workspaceId: 'workspace-a', taskId: 'task-a', sceneId: 'scene-a', sessionId: 'session-a' }))
    store.push(event({ eventId: 'event-b', workspaceId: 'workspace-a', taskId: 'task-b', sceneId: 'scene-b', sessionId: 'session-b' }))

    expect(store.unreadForWorkspace('workspace-a')).toBe(2)
    expect(store.unreadForTask('task-a')).toBe(1)
    expect(store.unreadForScene('scene-b')).toBe(1)
    expect(store.sessionHasUnread('session-a')).toBe(true)

    store.markSessionRead('session-a')
    expect(store.unreadForWorkspace('workspace-a')).toBe(1)
    expect(store.unreadForTask('task-a')).toBe(0)
    expect(store.sessionHasUnread('session-a')).toBe(false)
  })

  it('retains an event with missing hierarchy so the user can still inspect and remove it', () => {
    const store = new AgentNotificationStore({ now: () => 1_000 })
    const notification = store.push({
      eventId: 'damaged-event', eventType: 'attention', title: 'Claude Code', body: '需要处理'
    })

    expect(notification).toMatchObject({ workspaceId: null, taskId: null, sceneId: null, sessionId: null })
    expect(store.snapshot().notifications).toHaveLength(1)
    store.remove(notification!.id)
    expect(store.snapshot().notifications).toHaveLength(0)
  })

  it('keeps notification history session-scoped while remembering the sound preference', () => {
    let persisted: boolean | undefined
    const first = new AgentNotificationStore({
      now: () => 1_000,
      loadSoundEnabled: () => persisted ?? true,
      persistSoundEnabled: (enabled) => { persisted = enabled }
    })
    first.push(event({ eventId: 'event-1' }))
    first.setSoundEnabled(false)

    const nextAppSession = new AgentNotificationStore({
      now: () => 2_000,
      loadSoundEnabled: () => persisted ?? true,
      persistSoundEnabled: (enabled) => { persisted = enabled }
    })
    expect(nextAppSession.snapshot().notifications).toHaveLength(0)
    expect(nextAppSession.snapshot().soundEnabled).toBe(false)
  })

  it('plays a sound only when the event and global preference both allow it', () => {
    let now = 1_000
    const playSound = vi.fn()
    const store = new AgentNotificationStore({ now: () => now, playSound })

    store.push(event({ eventId: 'audible', eventType: 'completed' }))
    now += 6_000
    store.push(event({ eventId: 'silent-event', eventType: 'completed', sound: false }))
    store.setSoundEnabled(false)
    now += 6_000
    store.push(event({ eventId: 'muted-globally', eventType: 'completed' }))

    expect(playSound).toHaveBeenCalledTimes(1)
  })

  it('keeps an external-store snapshot stable until visible state changes', () => {
    const store = new AgentNotificationStore({ now: () => 1_000 })
    const before = store.snapshot()
    expect(store.snapshot()).toBe(before)

    store.push(event({ eventId: 'event-1' }))

    expect(store.snapshot()).not.toBe(before)
    expect(store.snapshot()).toBe(store.snapshot())
  })

  it('notifies subscribers after every user-visible state change', () => {
    const store = new AgentNotificationStore({ now: () => 1_000 })
    const listener = vi.fn()
    const unsubscribe = store.subscribe(listener)

    const notification = store.push(event({ eventId: 'event-1' }))!
    store.markAllRead()
    store.remove(notification.id)
    store.setSoundEnabled(false)
    unsubscribe()
    store.clear()

    expect(listener).toHaveBeenCalledTimes(4)
  })
})

function event(overrides: Record<string, unknown> = {}) {
  return {
    eventId: 'event', eventType: 'completed', title: 'Claude Code', subtitle: 'Completed', body: '任务完成',
    workspaceId: 'workspace', taskId: 'task', sceneId: 'scene', sessionId: 'session', sound: true,
    ...overrides
  }
}
