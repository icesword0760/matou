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
  replacementKey?: string
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
  replacementKey: string
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
  maxPerWorkspace?: number
  readRetentionMs?: number
}

const DEFAULT_COOLDOWN_MS = 5_000
const DEFAULT_MAX_PER_WORKSPACE = 1_000
const DEFAULT_READ_RETENTION_MS = 30 * 24 * 60 * 60 * 1_000
const UNASSIGNED_WORKSPACE_BUCKET = '__unassigned__'

export class AgentNotificationStore {
  readonly #now: () => number
  readonly #playSound: () => void
  readonly #persistSoundEnabled: (enabled: boolean) => void
  readonly #cooldownMs: number
  readonly #maxPerWorkspace: number
  readonly #readRetentionMs: number
  readonly #listeners = new Set<() => void>()
  readonly #cooldowns = new Map<string, number>()
  readonly #cooldownKeysByNotificationId = new Map<string, string>()
  readonly #cooldownKeyRefCounts = new Map<string, number>()
  readonly #notifications: AgentNotification[] = []
  #soundEnabled: boolean
  #snapshot: AgentNotificationSnapshot
  #sequence = 0

  constructor(options: AgentNotificationStoreOptions = {}) {
    this.#now = options.now ?? Date.now
    this.#playSound = options.playSound ?? (() => {})
    this.#persistSoundEnabled = options.persistSoundEnabled ?? (() => {})
    this.#cooldownMs = options.cooldownMs ?? DEFAULT_COOLDOWN_MS
    this.#maxPerWorkspace = Math.max(0, Math.floor(options.maxPerWorkspace ?? DEFAULT_MAX_PER_WORKSPACE))
    this.#readRetentionMs = Math.max(0, options.readRetentionMs ?? DEFAULT_READ_RETENTION_MS)
    this.#soundEnabled = options.loadSoundEnabled?.() ?? true
    this.#snapshot = this.#buildSnapshot()
  }

  push(input: AgentNotificationInput): AgentNotification | null {
    const now = this.#now()
    const prunedBeforePush = this.#prune(now)
    const replacementKey = input.replacementKey?.trim() ?? ''
    const replacementIndex = replacementKey
      ? this.#notifications.findIndex((notification) => notification.replacementKey === replacementKey)
      : -1
    if (replacementIndex >= 0) {
      const current = this.#notifications[replacementIndex]!
      const sessionId = input.sessionId ?? null
      Object.assign(current, {
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
        replacementKey,
        teamRole: input.teamRole ?? '',
        teamStatus: input.teamStatus ?? '',
        teamStatusTone: input.teamStatusTone ?? ''
      })
      this.#notifications.splice(replacementIndex, 1)
      this.#notifications.unshift(current)
      if (!input.isFocusedSession && current.sound && this.#soundEnabled) this.#playSound()
      this.#prune(now)
      this.#emit()
      return current
    }
    const cooldownKey = this.#cooldownKey(input)
    const lastTime = this.#cooldowns.get(cooldownKey)
    if (lastTime !== undefined && now - lastTime < this.#cooldownMs) {
      if (prunedBeforePush) this.#emit()
      return null
    }
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
      replacementKey,
      teamRole: input.teamRole ?? '',
      teamStatus: input.teamStatus ?? '',
      teamStatusTone: input.teamStatusTone ?? ''
    }
    this.#notifications.unshift(notification)
    if (!input.isFocusedSession && notification.sound && this.#soundEnabled) this.#playSound()
    this.#prune(now)
    this.#emit()
    return notification
  }

  snapshot(): AgentNotificationSnapshot {
    if (this.#prune(this.#now())) this.#snapshot = this.#buildSnapshot()
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
    return this.sessionHasUnread(sessionId)
  }

  dismissSessionIndicator(sessionId: string): void {
    const before = this.#notifications.length
    for (let index = this.#notifications.length - 1; index >= 0; index -= 1) {
      if (this.#notifications[index]?.sessionId === sessionId) this.#removeAt(index)
    }
    if (this.#notifications.length !== before) this.#emit()
  }

  markWorkspaceRead(workspaceId: string): void {
    this.#markRead(({ workspaceId: owner }) => owner === workspaceId)
  }

  markSessionRead(sessionId: string): void {
    let changed = this.#prune(this.#now())
    for (const notification of this.#notifications) {
      if (!notification.read && notification.sessionId === sessionId) {
        notification.read = true
        changed = true
      }
    }
    if (changed) this.#emit()
  }

  markAllRead(): void {
    let changed = this.#prune(this.#now())
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
    this.#removeAt(index)
    this.#cleanupFocusedReadIndicators()
    this.#emit()
  }

  removeByReplacementKey(replacementKey: string): void {
    const normalized = replacementKey.trim()
    if (!normalized) return
    const index = this.#notifications.findIndex((notification) => notification.replacementKey === normalized)
    if (index < 0) return
    this.#removeAt(index)
    this.#cleanupFocusedReadIndicators()
    this.#emit()
  }

  clear(): void {
    if (this.#notifications.length === 0) return
    this.#notifications.splice(0)
    this.#cooldowns.clear()
    this.#cooldownKeysByNotificationId.clear()
    this.#cooldownKeyRefCounts.clear()
    this.#focusedReadIndicators.clear()
    this.#emit()
  }

  setSoundEnabled(enabled: boolean): void {
    if (this.#soundEnabled === enabled) return
    this.#soundEnabled = enabled
    this.#persistSoundEnabled(enabled)
    this.#emit()
  }

  #markRead(predicate: (notification: AgentNotification) => boolean): void {
    let changed = this.#prune(this.#now())
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

  #prune(now: number): boolean {
    const removeIds = new Set<string>()
    const buckets = new Map<string, AgentNotification[]>()
    for (const notification of this.#notifications) {
      if (notification.read && now - notification.timestamp > this.#readRetentionMs) {
        removeIds.add(notification.id)
        continue
      }
      const bucket = notification.workspaceId ?? UNASSIGNED_WORKSPACE_BUCKET
      const notifications = buckets.get(bucket)
      if (notifications) notifications.push(notification)
      else buckets.set(bucket, [notification])
    }
    for (const notifications of buckets.values()) {
      const overflow = notifications.length - this.#maxPerWorkspace
      if (overflow <= 0) continue
      notifications.sort(compareOldestNotification)
      for (let index = 0; index < overflow; index += 1) {
        removeIds.add(notifications[index]!.id)
      }
    }
    if (removeIds.size === 0) return false
    for (let index = this.#notifications.length - 1; index >= 0; index -= 1) {
      if (removeIds.has(this.#notifications[index]!.id)) this.#removeAt(index)
    }
    this.#cleanupFocusedReadIndicators()
    return true
  }

  #trackCooldown(notificationId: string, cooldownKey: string): void {
    this.#cooldownKeysByNotificationId.set(notificationId, cooldownKey)
    this.#cooldownKeyRefCounts.set(cooldownKey, (this.#cooldownKeyRefCounts.get(cooldownKey) ?? 0) + 1)
  }

  #removeAt(index: number): void {
    const [removed] = this.#notifications.splice(index, 1)
    if (!removed) return
    const cooldownKey = this.#cooldownKeysByNotificationId.get(removed.id)
    if (!cooldownKey) return
    this.#cooldownKeysByNotificationId.delete(removed.id)
    const remaining = (this.#cooldownKeyRefCounts.get(cooldownKey) ?? 1) - 1
    if (remaining > 0) {
      this.#cooldownKeyRefCounts.set(cooldownKey, remaining)
      return
    }
    this.#cooldownKeyRefCounts.delete(cooldownKey)
    this.#cooldowns.delete(cooldownKey)
  }

  #cleanupFocusedReadIndicators(): void {
    if (this.#focusedReadIndicators.size === 0) return
    const remainingSessionIds = new Set(
      this.#notifications.flatMap(({ sessionId }) => sessionId ? [sessionId] : [])
    )
    for (const sessionId of this.#focusedReadIndicators) {
      if (!remainingSessionIds.has(sessionId)) this.#focusedReadIndicators.delete(sessionId)
    }
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

function compareOldestNotification(left: AgentNotification, right: AgentNotification): number {
  if (left.timestamp !== right.timestamp) return left.timestamp - right.timestamp
  const sequenceDifference = notificationSequence(left.id) - notificationSequence(right.id)
  if (sequenceDifference !== 0) return sequenceDifference
  return left.id < right.id ? -1 : left.id > right.id ? 1 : 0
}

function notificationSequence(id: string): number {
  const suffix = id.slice(id.lastIndexOf('-') + 1)
  const sequence = Number(suffix)
  return Number.isFinite(sequence) ? sequence : 0
}
