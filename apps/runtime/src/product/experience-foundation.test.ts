import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import {
  FeatureCampaignRegistry,
  NotificationProjection,
  PreferenceRepository
} from './experience-foundation'

let database: RuntimeDatabase

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-experience-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seed(database)
})

afterEach(() => database.close())

describe('PreferenceRepository', () => {
  it('persists only typed allow-listed preferences with safe defaults', () => {
    const preferences = new PreferenceRepository(database)

    expect(preferences.get('notification.soundEnabled')).toBe(true)
    expect(preferences.get('shell.restoreHistoryEnabled')).toBe(true)
    preferences.set('shell.restoreHistoryEnabled', false, 2)
    expect(new PreferenceRepository(database).get('shell.restoreHistoryEnabled')).toBe(false)
    preferences.set('notification.soundEnabled', false, 2)
    expect(new PreferenceRepository(database).get('notification.soundEnabled')).toBe(false)
    expect(() => preferences.set('unknown' as never, true as never, 3)).toThrow('Unsupported preference')
    expect(() => preferences.set('retention.globalBytes', -1, 3)).toThrow('Invalid preference')
  })
})

describe('NotificationProjection', () => {
  it('is session-memory only, suppresses focused panels, and keeps at most one unread per panel', () => {
    const notifications = new NotificationProjection({ cooldownMs: 5_000 })
    notifications.focus('mount-1')
    const focused = notifications.ingest(event('event-1', 'permission', 1))
    notifications.blur()
    const first = notifications.ingest(event('event-2', 'permission', 7_000))
    const second = notifications.ingest(event('event-3', 'error', 8_000))

    expect(focused?.read).toBe(true)
    expect(first?.read).toBe(false)
    expect(notifications.list().find(({ eventId }) => eventId === 'event-2')?.read).toBe(true)
    expect(second?.read).toBe(false)
    expect(notifications.unreadCount()).toBe(1)
    expect(new NotificationProjection().list()).toEqual([])
  })

  it('deduplicates provider IDs, applies a five-second type cooldown, and supports read/delete operations', () => {
    const notifications = new NotificationProjection({ cooldownMs: 5_000 })
    expect(notifications.ingest(event('same', 'error', 1))).toBeDefined()
    expect(notifications.ingest(event('same', 'error', 2))).toBeUndefined()
    expect(notifications.ingest(event('different', 'error', 3_000))).toBeUndefined()
    expect(notifications.ingest(event('later', 'error', 6_001))).toBeDefined()

    notifications.markPanelRead('mount-1')
    expect(notifications.unreadCount()).toBe(0)
    notifications.clear()
    expect(notifications.list()).toEqual([])
  })

  it('clears only the deleted Task while preserving unread feedback for other Tasks', () => {
    const notifications = new NotificationProjection({ cooldownMs: 0 })
    notifications.ingest(event('task-1-event', 'error', 1))
    notifications.ingest({ ...event('task-2-event', 'error', 2), taskId: 'task-2' })

    notifications.clearTask('task-1')

    expect(notifications.unreadCount({ taskId: 'task-1' })).toBe(0)
    expect(notifications.unreadCount({ taskId: 'task-2' })).toBe(1)
  })

  it('resolves a stable main-window mount and falls back to Task/Session history', () => {
    const notifications = new NotificationProjection()
    const item = notifications.ingest(event('event-nav', 'error', 1))!

    expect(notifications.resolveNavigation(item.id, database)).toEqual({
      kind: 'live-mount', workspaceId: 'workspace-1', taskId: 'task-1', sessionId: 'session-1',
      sceneId: 'scene-1', mountId: 'mount-1'
    })
    database.run('DELETE FROM session_mounts WHERE id = ?', 'mount-1')
    expect(notifications.resolveNavigation(item.id, database)).toEqual({
      kind: 'session-stopped', workspaceId: 'workspace-1', taskId: 'task-1', sessionId: 'session-1'
    })
  })
})

describe('FeatureCampaignRegistry', () => {
  it('shows each campaign version once only after explicit mode entry', () => {
    const campaigns = new FeatureCampaignRegistry(database, [{ id: 'cli-intro', version: 2, pages: [{ title: 'DAG' }] }])

    expect(campaigns.next({ reason: 'restore' })).toBeUndefined()
    expect(campaigns.next({ reason: 'user-enter' })?.id).toBe('cli-intro')
    campaigns.markSeen('cli-intro', 2, 10)
    campaigns.markSeen('cli-intro', 2, 11)
    expect(campaigns.next({ reason: 'user-enter' })).toBeUndefined()
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM feature_campaign_views')?.count).toBe(1)
  })

  it('keys seen state by campaign version and supports an internal force-display switch', () => {
    const v1 = new FeatureCampaignRegistry(database, [{ id: 'cli-intro', version: 1, pages: [{ title: 'One' }] }])
    v1.markSeen('cli-intro', 1, 1)
    const v2 = new FeatureCampaignRegistry(database, [{ id: 'cli-intro', version: 2, pages: [{ title: 'Two' }] }])

    expect(v2.next({ reason: 'user-enter' })?.version).toBe(2)
    v2.markSeen('cli-intro', 2, 2)
    expect(v2.next({ reason: 'user-enter', force: true })?.version).toBe(2)
  })
})

function event(eventId: string, type: string, occurredAt: number) {
  return {
    eventId, type, title: 'Claude Code', subtitle: type, body: `${type} body`,
    workspaceId: 'workspace-1', taskId: 'task-1', sessionId: 'session-1', mountId: 'mount-1',
    occurredAt
  }
}

function seed(db: RuntimeDatabase): void {
  db.transaction((tx) => {
    tx.run('INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)', 'workspace-1', 'Workspace', '/tmp/workspace', 1, 1)
    tx.run('INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)', 'context-1', 'workspace-1', 'plain-directory', '/tmp/workspace', 1)
    tx.run('INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)', 'task-1', 'workspace-1', 'context-1', 'Task', 'active', 'a', 1, 1)
    tx.run('INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)', 'session-1', 'task-1', 'context-1', 'claude-code', 'running', 'Claude', 1, 1, 1)
    tx.run('INSERT INTO scenes (id, task_id, name, mode, root_node_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)', 'scene-1', 'task-1', 'Scene', 'tile', 'root-1', 1, 1)
    tx.run("INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at) VALUES (?, ?, 'root', 0, ?)", 'root-1', 'scene-1', 1)
    tx.run('INSERT INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at) VALUES (?, ?, ?, ?, ?)', 'mount-1', 'scene-1', 'root-1', 'session-1', 1)
  })
}
