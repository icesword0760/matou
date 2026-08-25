export type AgentNotificationEventType = 'completed' | 'permission' | 'error' | 'waiting' | 'attention' | string

export interface AgentNotificationInput {
  eventId: string
  eventType: AgentNotificationEventType
  title: string
  subtitle?: string
  body?: string
  workspaceId?: string | null
  taskId?: string | null
  sceneId?: string | null
  sessionId?: string | null
  sound?: boolean
  cooldownKey?: string
  isFocusedSession?: boolean
  teamRole?: string
  teamStatus?: string
  teamStatusTone?: string
}

export interface AgentNotification {
  id: string
  eventId: string
  eventType: AgentNotificationEventType
  title: string
  subtitle: string
  body: string
  workspaceId: string | null
  taskId: string | null
  sceneId: string | null
  sessionId: string | null
  timestamp: number
  read: boolean
  sound: boolean
  teamRole: string
  teamStatus: string
  teamStatusTone: string
}

export interface AgentNotificationSnapshot {
  notifications: readonly AgentNotification[]
  unreadCount: number
  soundEnabled: boolean
}

export interface AgentNotificationStoreOptions {
  now?: () => number
  playSound?: () => void
  loadSoundEnabled?: () => boolean
  persistSoundEnabled?: (enabled: boolean) => void
  cooldownMs?: number
}

const DEFAULT_COOLDOWN_MS = 5_000

export class AgentNotificationStore {
  readonly #now: () => number
  readonly #playSound: () => void
  readonly #persistSoundEnabled: (enabled: boolean) => void
  readonly #cooldownMs: number
  readonly #listeners = new Set<() => void>()
  readonly #cooldowns = new Map<string, number>()
  readonly #notifications: AgentNotification[] = []
  readonly #focusedReadIndicators = new Set<string>()
  #soundEnabled: boolean
  #snapshot: AgentNotificationSnapshot
  #sequence = 0

  constructor(options: AgentNotificationStoreOptions = {}) {
    this.#now = options.now ?? Date.now
    this.#playSound = options.playSound ?? (() => {})
    this.#persistSoundEnabled = options.persistSoundEnabled ?? (() => {})
    this.#cooldownMs = options.cooldownMs ?? DEFAULT_COOLDOWN_MS
    this.#soundEnabled = options.loadSoundEnabled?.() ?? true
    this.#snapshot = this.#buildSnapshot()
  }

  push(input: AgentNotificationInput): AgentNotification | null {
    const now = this.#now()
    const cooldownKey = this.#cooldownKey(input)
    const lastTime = this.#cooldowns.get(cooldownKey)
    if (lastTime !== undefined && now - lastTime < this.#cooldownMs) return null
    this.#cooldowns.set(cooldownKey, now)

    const sessionId = input.sessionId ?? null
    if (sessionId) {
      for (const notification of this.#notifications) {
        if (!notification.read && notification.sessionId === sessionId) notification.read = true
      }
    }

    const notification: AgentNotification = {
      id: `notification-${now}-${++this.#sequence}`,
      eventId: input.eventId,
      eventType: input.eventType,
      title: input.title,
      subtitle: input.subtitle ?? '',
      body: input.body ?? '',
      workspaceId: input.workspaceId ?? null,
      taskId: input.taskId ?? null,
      sceneId: input.sceneId ?? null,
      sessionId,
      timestamp: now,
      read: input.isFocusedSession === true,
      sound: input.sound !== false,
      teamRole: input.teamRole ?? '',
      teamStatus: input.teamStatus ?? '',
      teamStatusTone: input.teamStatusTone ?? ''
    }
    this.#notifications.unshift(notification)
    if (input.isFocusedSession && sessionId) this.#focusedReadIndicators.add(sessionId)
    if (!input.isFocusedSession && notification.sound && this.#soundEnabled) this.#playSound()
    this.#emit()
    return notification
  }

  snapshot(): AgentNotificationSnapshot {
    return this.#snapshot
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener)
    return () => { this.#listeners.delete(listener) }
  }

  unreadForWorkspace(workspaceId: string): number {
    return this.#countUnread(({ workspaceId: owner }) => owner === workspaceId)
  }

  unreadForTask(taskId: string): number {
    return this.#countUnread(({ taskId: owner }) => owner === taskId)
  }

  unreadForScene(sceneId: string): number {
    return this.#countUnread(({ sceneId: owner }) => owner === sceneId)
  }

  sessionHasUnread(sessionId: string): boolean {
    return this.#notifications.some(({ read, sessionId: owner }) => !read && owner === sessionId)
  }

  sessionHasVisibleIndicator(sessionId: string): boolean {
    return this.sessionHasUnread(sessionId) || this.#focusedReadIndicators.has(sessionId)
  }

  dismissSessionIndicator(sessionId: string): void {
    const before = this.#notifications.length
    for (let index = this.#notifications.length - 1; index >= 0; index -= 1) {
      if (this.#notifications[index]?.sessionId === sessionId) this.#notifications.splice(index, 1)
    }
    const changedIndicator = this.#focusedReadIndicators.delete(sessionId)
    if (this.#notifications.length !== before || changedIndicator) this.#emit()
  }

  markWorkspaceRead(workspaceId: string): void {
    this.#markRead(({ workspaceId: owner }) => owner === workspaceId)
  }

  markSessionRead(sessionId: string): void {
    let changed = false
    for (const notification of this.#notifications) {
      if (!notification.read && notification.sessionId === sessionId) {
        notification.read = true
        changed = true
      }
    }
    if (changed) this.#emit()
  }

  markAllRead(): void {
    let changed = false
    for (const notification of this.#notifications) {
      if (!notification.read) {
        notification.read = true
        changed = true
      }
    }
    if (changed) this.#emit()
  }

  remove(id: string): void {
    const index = this.#notifications.findIndex((notification) => notification.id === id)
    if (index < 0) return
    this.#notifications.splice(index, 1)
    this.#emit()
  }

  clear(): void {
    if (this.#notifications.length === 0) return
    this.#notifications.splice(0)
    this.#emit()
  }

  setSoundEnabled(enabled: boolean): void {
    if (this.#soundEnabled === enabled) return
    this.#soundEnabled = enabled
    this.#persistSoundEnabled(enabled)
    this.#emit()
  }

  #markRead(predicate: (notification: AgentNotification) => boolean): void {
    let changed = false
    for (const notification of this.#notifications) {
      if (!notification.read && predicate(notification)) {
        notification.read = true
        changed = true
      }
    }
    if (changed) this.#emit()
  }

  #countUnread(predicate: (notification: AgentNotification) => boolean): number {
    return this.#notifications.filter((notification) => !notification.read && predicate(notification)).length
  }

  #cooldownKey(input: AgentNotificationInput): string {
    const owner = input.sessionId ?? input.sceneId ?? input.taskId ?? input.workspaceId ?? 'global'
    return `${owner}:${input.cooldownKey ?? input.eventType}`
  }

  #buildSnapshot(): AgentNotificationSnapshot {
    return {
      notifications: this.#notifications,
      unreadCount: this.#notifications.filter(({ read }) => !read).length,
      soundEnabled: this.#soundEnabled
    }
  }

  #emit(): void {
    this.#snapshot = this.#buildSnapshot()
    for (const listener of this.#listeners) listener()
  }
}
