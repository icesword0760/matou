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

    expect(result.appliedVersions).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13])
    expect(result.currentVersion).toBe(13)
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
        'session_fork_intents',
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
        'task_window_migrations',
        'bootstrap_state'
      ])
    )

    const sceneColumns = database
      .all<{ name: string }>('PRAGMA table_info(scenes)')
      .map(({ name }) => name)
    expect(sceneColumns).toEqual(expect.arrayContaining([
      'title_pinned', 'sort_key', 'layout_revision'
    ]))
    expect(database.all<{ name: string }>('PRAGMA table_info(sessions)').map(({ name }) => name))
      .toContain('cwd')
  })

  it('is idempotent when every migration is already applied', async () => {
    const { database } = await createDatabase()
    const runner = new MigrationRunner(database, FOUNDATION_MIGRATIONS)
    await runner.migrate()

    await expect(runner.migrate()).resolves.toEqual({
      appliedVersions: [],
      currentVersion: 13,
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
      FOUNDATION_MIGRATIONS[7]!,
      FOUNDATION_MIGRATIONS[8]!,
      FOUNDATION_MIGRATIONS[9]!,
      FOUNDATION_MIGRATIONS[10]!,
      FOUNDATION_MIGRATIONS[11]!,
      FOUNDATION_MIGRATIONS[12]!
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
    ).rejects.toThrow('database schema version 99 is newer than supported version 13')
  })

  it('repairs stale Shell and Agent titles when upgrading an existing PRD 06 database', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 11)).migrate()
    database.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, 1, 1)',
      'workspace', 'Workspace', '/tmp/workspace'
    )
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('context', 'workspace', 'plain-directory', '/tmp/workspace', 1)`
    )
    database.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, created_at, updated_at
       ) VALUES ('task', 'workspace', 'context', 'Task', 'active', 1, 1)`
    )
    for (const [id, kind, title] of [
      ['shell', 'shell', 'Claude'],
      ['claude', 'claude-code', 'Shell'],
      ['codex', 'codex', 'Shell']
    ] as const) {
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at
         ) VALUES (?, 'task', 'context', ?, 'created', ?, '/tmp/workspace', 1, 1, 1)`,
        id, kind, title
      )
    }

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([12, 13])
    expect(database.all<{ id: string; title: string }>(
      'SELECT id, title FROM sessions ORDER BY id'
    )).toEqual([
      { id: 'claude', title: 'Claude' },
      { id: 'codex', title: 'Codex' },
      { id: 'shell', title: 'Shell' }
    ])
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
