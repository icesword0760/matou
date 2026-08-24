import { access, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './database'
import { FOUNDATION_MIGRATIONS } from './migrations'
import { MigrationRunner, type Migration } from './migration-runner'

const opened: RuntimeDatabase[] = []

afterEach(() => {
  for (const database of opened.splice(0)) {
    database.close()
  }
})

async function createDatabase(): Promise<{ database: RuntimeDatabase; path: string }> {
  const root = await mkdtemp(join(tmpdir(), 'matou-migration-'))
  const path = join(root, 'matou.sqlite')
  const database = RuntimeDatabase.open(path)
  opened.push(database)
  return { database, path }
}

describe('MigrationRunner', () => {
  it('creates the complete foundation schema from an empty database', async () => {
    const { database } = await createDatabase()

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([1, 2, 3, 4, 5, 6, 7, 8])
    expect(result.currentVersion).toBe(8)
    const tables = database
      .all<{ name: string }>("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
      .map(({ name }) => name)
    expect(tables).toEqual(
      expect.arrayContaining([
        'schema_migrations',
        'workspaces',
        'tasks',
        'sessions',
        'session_runs',
        'provider_bindings',
        'domain_events',
        'consumer_cursors',
        'command_deduplication',
        'session_relation_events',
        'session_relations_current',
        'scenes',
        'scene_geometry',
        'journal_checkpoints',
        'annotations',
        'artifacts',
        'validation_runs',
        'task_status_entries',
        'task_progress',
        'task_logs',
        'preferences',
        'feature_campaign_views',
        'preset_capability_state',
        'preset_capability_suppressions',
        'preset_reconcile_commands',
        'legacy_source_cursors',
        'migration_authority',
        'migration_telemetry',
        'legacy_projection_diffs',
        'legacy_import_runs',
        'shadow_repair_queue',
        'workspace_path_state',
        'app_windows',
        'window_navigation',
        'window_workspace_focus',
        'window_task_focus',
        'window_scene_focus',
        'window_task_placements',
        'bootstrap_state'
      ])
    )

    const sceneColumns = database
      .all<{ name: string }>('PRAGMA table_info(scenes)')
      .map(({ name }) => name)
    expect(sceneColumns).toEqual(expect.arrayContaining([
      'title_pinned', 'sort_key', 'layout_revision'
    ]))
  })

  it('is idempotent when every migration is already applied', async () => {
    const { database } = await createDatabase()
    const runner = new MigrationRunner(database, FOUNDATION_MIGRATIONS)
    await runner.migrate()

    await expect(runner.migrate()).resolves.toEqual({
      appliedVersions: [],
      currentVersion: 8,
      backupPath: undefined
    })
  })

  it('rejects an edited migration whose stored checksum differs', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    const edited: Migration[] = [
      { ...FOUNDATION_MIGRATIONS[0]!, sql: `${FOUNDATION_MIGRATIONS[0]!.sql}\nSELECT 1;` },
      FOUNDATION_MIGRATIONS[1]!,
      FOUNDATION_MIGRATIONS[2]!,
      FOUNDATION_MIGRATIONS[3]!,
      FOUNDATION_MIGRATIONS[4]!,
      FOUNDATION_MIGRATIONS[5]!,
      FOUNDATION_MIGRATIONS[6]!,
      FOUNDATION_MIGRATIONS[7]!
    ]

    await expect(new MigrationRunner(database, edited).migrate()).rejects.toThrow(
      'checksum mismatch for applied migration 1'
    )
  })

  it('rejects a database created by a newer application schema', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    database.run(
      'INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?, ?, ?, ?)',
      99,
      'future',
      'future-checksum',
      Date.now()
    )

    await expect(
      new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    ).rejects.toThrow('database schema version 99 is newer than supported version 8')
  })

  it('rolls back a failed migration without recording its version', async () => {
    const { database } = await createDatabase()
    const broken: Migration[] = [
      { version: 1, name: 'broken', sql: 'CREATE TABLE transient_table (id TEXT); INVALID SQL;' }
    ]

    await expect(new MigrationRunner(database, broken).migrate()).rejects.toThrow()
    expect(
      database.get<{ name: string }>(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'transient_table'"
      )
    ).toBeUndefined()
    expect(database.all('SELECT * FROM schema_migrations')).toEqual([])
  })

  it('creates a consistent backup before applying an upgrade', async () => {
    const { database, path } = await createDatabase()
    const first: Migration = {
      version: 1,
      name: 'first',
      sql: 'CREATE TABLE first_table (id TEXT PRIMARY KEY) STRICT;'
    }
    await new MigrationRunner(database, [first]).migrate()
    const second: Migration = {
      version: 2,
      name: 'second',
      sql: 'CREATE TABLE second_table (id TEXT PRIMARY KEY) STRICT;'
    }

    const result = await new MigrationRunner(database, [first, second]).migrate()

    expect(result.appliedVersions).toEqual([2])
    expect(result.backupPath).toMatch(new RegExp(`${escapeRegExp(path)}\\.pre-v2-\\d+\\.sqlite$`))
    await expect(access(result.backupPath!)).resolves.toBeUndefined()
  })
})

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}
