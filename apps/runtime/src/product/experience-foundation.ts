import { randomUUID } from 'node:crypto'

import type { RuntimeDatabase } from '../storage/database'

export interface PreferenceValues {
  'notification.soundEnabled': boolean
  'retention.globalBytes': number
  'retention.perSessionBytes': number
  'retention.checkpointGenerations': number
  'diagnostics.enabled': boolean
  'shell.restoreHistoryEnabled': boolean
}

const PREFERENCE_DEFAULTS: PreferenceValues = {
  'notification.soundEnabled': true,
  'retention.globalBytes': 2 * 1024 * 1024 * 1024,
  'retention.perSessionBytes': 256 * 1024 * 1024,
  'retention.checkpointGenerations': 2,
  'diagnostics.enabled': true,
  'shell.restoreHistoryEnabled': true
}

export class PreferenceRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  get<K extends keyof PreferenceValues>(key: K): PreferenceValues[K] {
    this.#assertKey(key)
    const row = this.#database.get<{ value_json: string }>('SELECT value_json FROM preferences WHERE key = ?', key)
    if (!row) return PREFERENCE_DEFAULTS[key]
    try {
      const value = JSON.parse(row.value_json) as PreferenceValues[K]
      this.#validate(key, value)
      return value
    } catch {
      return PREFERENCE_DEFAULTS[key]
    }
  }

  set<K extends keyof PreferenceValues>(key: K, value: PreferenceValues[K], now = Date.now()): void {
    this.#assertKey(key)
    this.#validate(key, value)
    this.#database.run(
      `INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?)
       ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at`,
      key, JSON.stringify(value), now
    )
  }

  snapshot(): PreferenceValues {
    return Object.fromEntries(
      (Object.keys(PREFERENCE_DEFAULTS) as Array<keyof PreferenceValues>).map((key) => [key, this.get(key)])
    ) as unknown as PreferenceValues
  }

  #assertKey(key: PropertyKey): asserts key is keyof PreferenceValues {
    if (!Object.prototype.hasOwnProperty.call(PREFERENCE_DEFAULTS, key)) {
      throw new Error(`Unsupported preference: ${String(key)}`)
    }
  }

  #validate<K extends keyof PreferenceValues>(key: K, value: PreferenceValues[K]): void {
    const valid = key === 'notification.soundEnabled' || key === 'diagnostics.enabled' ||
      key === 'shell.restoreHistoryEnabled'
      ? typeof value === 'boolean'
      : typeof value === 'number' && Number.isSafeInteger(value) && value > 0
    if (!valid) throw new Error(`Invalid preference value for ${key}`)
  }
}

export interface NotificationEvent {
  eventId: string
  type: string
  title: string
  subtitle: string
  body: string
  workspaceId: string
  taskId: string
  sessionId: string
  mountId?: string
  occurredAt: number
  teamRole?: string
  teamStatus?: string
  sound?: boolean
}

export interface NotificationItem extends NotificationEvent {
  id: string
  read: boolean
  soundRequested: boolean
}

export type NotificationNavigationTarget =
  | { kind: 'live-mount'; workspaceId: string; taskId: string; sessionId: string; sceneId: string; mountId: string }
  | { kind: 'session-stopped'; workspaceId: string; taskId: string; sessionId: string }

export class NotificationProjection {
  readonly #cooldownMs: number
  readonly #items: NotificationItem[] = []
  readonly #seenEventIds = new Set<string>()
  readonly #cooldowns = new Map<string, number>()
  #focusedMountId: string | undefined

  constructor(options: { cooldownMs?: number } = {}) {
    this.#cooldownMs = options.cooldownMs ?? 5_000
  }

  focus(mountId: string): void {
    this.#focusedMountId = mountId
    this.markPanelRead(mountId)
  }

  blur(): void {
    this.#focusedMountId = undefined
  }

  ingest(event: NotificationEvent): NotificationItem | undefined {
    if (this.#seenEventIds.has(event.eventId)) return undefined
    this.#seenEventIds.add(event.eventId)
    const cooldownKey = `${event.mountId ?? event.sessionId}:${event.type}`
    const last = this.#cooldowns.get(cooldownKey)
    if (last !== undefined && event.occurredAt - last < this.#cooldownMs) return undefined
    this.#cooldowns.set(cooldownKey, event.occurredAt)

    const focused = event.mountId !== undefined && event.mountId === this.#focusedMountId
    if (!focused) {
      for (const item of this.#items) {
        if (!item.read && samePanel(item, event)) item.read = true
      }
    }
    const item: NotificationItem = {
      ...event,
      id: `notification-${randomUUID()}`,
      read: focused,
      soundRequested: !focused && event.sound !== false
    }
    this.#items.push(item)
    return { ...item }
  }

  list(): NotificationItem[] {
    return this.#items.slice().sort((a, b) => b.occurredAt - a.occurredAt).map((item) => ({ ...item }))
  }

  unreadCount(filters: Partial<Pick<NotificationItem, 'workspaceId' | 'taskId' | 'sessionId' | 'mountId'>> = {}): number {
    return this.#items.filter((item) => !item.read && matches(item, filters)).length
  }

  markPanelRead(mountId: string): void {
    for (const item of this.#items) if (item.mountId === mountId) item.read = true
  }

  markAllRead(): void {
    for (const item of this.#items) item.read = true
  }

  dismiss(id: string): void {
    const index = this.#items.findIndex((item) => item.id === id)
    if (index >= 0) this.#items.splice(index, 1)
  }

  clear(): void {
    this.#items.splice(0)
  }

  clearTask(taskId: string): void {
    for (let index = this.#items.length - 1; index >= 0; index -= 1) {
      if (this.#items[index]?.taskId === taskId) this.#items.splice(index, 1)
    }
  }

  resolveNavigation(id: string, database: RuntimeDatabase): NotificationNavigationTarget | undefined {
    const item = this.#items.find((candidate) => candidate.id === id)
    if (!item) return undefined
    const live = database.get<{ scene_id: string; id: string }>(
      `SELECT sm.scene_id, sm.id
       FROM session_mounts sm
       LEFT JOIN scene_windows sw ON sw.id = sm.scene_window_id
       JOIN scenes s ON s.id = sm.scene_id
       WHERE sm.session_id = ? AND s.archived_at IS NULL
         AND (sm.scene_window_id IS NULL OR sw.state = 'attached')
       ORDER BY CASE WHEN sm.id = ? THEN 0 ELSE 1 END, sm.created_at DESC LIMIT 1`,
      item.sessionId, item.mountId ?? ''
    )
    if (live) {
      return {
        kind: 'live-mount', workspaceId: item.workspaceId, taskId: item.taskId,
        sessionId: item.sessionId, sceneId: live.scene_id, mountId: live.id
      }
    }
    const session = database.get<{ workspace_id: string; task_id: string }>(
      `SELECT t.workspace_id, s.task_id FROM sessions s JOIN tasks t ON t.id = s.task_id WHERE s.id = ?`,
      item.sessionId
    )
    return session
      ? { kind: 'session-stopped', workspaceId: session.workspace_id, taskId: session.task_id, sessionId: item.sessionId }
      : undefined
  }
}

function samePanel(a: NotificationItem, b: NotificationEvent): boolean {
  return a.mountId !== undefined && b.mountId !== undefined
    ? a.mountId === b.mountId
    : a.sessionId === b.sessionId
}

function matches(item: NotificationItem, filters: Partial<Pick<NotificationItem, 'workspaceId' | 'taskId' | 'sessionId' | 'mountId'>>): boolean {
  return Object.entries(filters).every(([key, value]) => value === undefined || item[key as keyof NotificationItem] === value)
}

export interface FeatureCampaign {
  id: string
  version: number
  pages: Array<{ title: string; description?: string; image?: string }>
}

export class FeatureCampaignRegistry {
  readonly #database: RuntimeDatabase
  readonly #campaigns: readonly FeatureCampaign[]

  constructor(database: RuntimeDatabase, campaigns: readonly FeatureCampaign[]) {
    this.#database = database
    const identities = new Set<string>()
    for (const campaign of campaigns) {
      if (!campaign.id.trim() || !Number.isSafeInteger(campaign.version) || campaign.version < 1 || campaign.pages.length === 0) {
        throw new Error('Invalid feature campaign manifest')
      }
      const identity = `${campaign.id}:${campaign.version}`
      if (identities.has(identity)) throw new Error(`Duplicate feature campaign ${identity}`)
      identities.add(identity)
    }
    this.#campaigns = campaigns.slice()
  }

  next(trigger: { reason: 'user-enter' | 'restore'; force?: boolean }): FeatureCampaign | undefined {
    if (trigger.reason !== 'user-enter') return undefined
    return this.#campaigns.find((campaign) => trigger.force || !this.#database.get(
      'SELECT 1 FROM feature_campaign_views WHERE campaign_id = ? AND campaign_version = ?',
      campaign.id, campaign.version
    ))
  }

  markSeen(campaignId: string, campaignVersion: number, now = Date.now()): void {
    if (!this.#campaigns.some((campaign) => campaign.id === campaignId && campaign.version === campaignVersion)) {
      throw new Error('Unknown feature campaign')
    }
    this.#database.run(
      `INSERT INTO feature_campaign_views (campaign_id, campaign_version, viewed_at)
       VALUES (?, ?, ?) ON CONFLICT(campaign_id, campaign_version) DO NOTHING`,
      campaignId, campaignVersion, now
    )
  }
}
