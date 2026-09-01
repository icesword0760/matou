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

    expect(result.appliedVersions).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23])
    expect(result.currentVersion).toBe(23)
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
        'session_canvas_memberships',
        'session_graph_summaries',
        'shell_history_blocks',
        'runtime_sequences',
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
      .toEqual(expect.arrayContaining(['cwd', 'work_status', 'title_source', 'provider_title']))
    expect(database.all<{ name: string }>('PRAGMA table_info(provider_bindings)').map(({ name }) => name))
      .toEqual(expect.arrayContaining(['restore_state', 'restore_error', 'user_exited_at']))
    expect(database.all<{ name: string }>('PRAGMA table_info(session_canvas_memberships)').map(({ name }) => name))
      .toEqual(expect.arrayContaining(['pending_user_interaction_seq']))
  })

  it('is idempotent when every migration is already applied', async () => {
    const { database } = await createDatabase()
    const runner = new MigrationRunner(database, FOUNDATION_MIGRATIONS)
    await runner.migrate()

    await expect(runner.migrate()).resolves.toEqual({
      appliedVersions: [],
      currentVersion: 23,
      backupPath: undefined
    })
  })

  it('places pre-board active Tasks in ready exactly once during upgrade', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 22)).migrate()
    database.run(
      `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
       VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1)`
    )
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('context', 'workspace', 'plain-directory', '/tmp/workspace', 1)`
    )
    for (const [id, status] of [['ready-default', 'active'], ['already-blocked', 'blocked']] as const) {
      database.run(
        `INSERT INTO tasks (
           id, workspace_id, execution_context_id, title, status, created_at, updated_at
         ) VALUES (?, 'workspace', 'context', ?, ?, 1, 1)`,
        id, id, status
      )
    }

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([23])
    expect(database.all<{ id: string; status: string }>(
      'SELECT id, status FROM tasks ORDER BY id'
    )).toEqual([
      { id: 'already-blocked', status: 'blocked' },
      { id: 'ready-default', status: 'planned' }
    ])
  })

  it('allows one provider conversation to be associated with separate cards after upgrading', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 20)).migrate()
    database.run(
      `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
       VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1)`
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
    for (const id of ['card-a', 'card-b']) {
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at
         ) VALUES (?, 'task', 'context', 'claude-code', 'running', ?, '/tmp/workspace', 1, 1, 1)`,
        id, id
      )
    }
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state,
         metadata_json, created_at, updated_at
       ) VALUES ('binding-a', 'card-a', 'claude-code', 'provider-shared',
                 'available', '{}', 1, 1)`
    )

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([21, 22, 23])
    expect(() => database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state,
         metadata_json, created_at, updated_at
       ) VALUES ('binding-b', 'card-b', 'claude-code', 'provider-shared',
                 'available', '{}', 2, 2)`
    )).not.toThrow()
    expect(() => database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state,
         metadata_json, created_at, updated_at
       ) VALUES ('binding-a-duplicate', 'card-a', 'claude-code', 'provider-shared',
                 'available', '{}', 3, 3)`
    )).toThrow()
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
      FOUNDATION_MIGRATIONS[12]!,
      FOUNDATION_MIGRATIONS[13]!,
      FOUNDATION_MIGRATIONS[14]!,
      FOUNDATION_MIGRATIONS[15]!,
      FOUNDATION_MIGRATIONS[16]!,
      FOUNDATION_MIGRATIONS[17]!,
      FOUNDATION_MIGRATIONS[18]!,
      FOUNDATION_MIGRATIONS[19]!,
      FOUNDATION_MIGRATIONS[20]!,
      FOUNDATION_MIGRATIONS[21]!,
      FOUNDATION_MIGRATIONS[22]!
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
    ).rejects.toThrow('database schema version 99 is newer than supported version 23')
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

    expect(result.appliedVersions).toEqual([12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23])
    expect(database.all<{ id: string; title: string }>(
      'SELECT id, title FROM sessions ORDER BY id'
    )).toEqual([
      { id: 'claude', title: 'Claude' },
      { id: 'codex', title: 'Codex' },
      { id: 'shell', title: 'Shell' }
    ])
  })

  it('backfills stable canvas memberships without changing existing Fork relations', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 13)).migrate()
    database.run(
      `INSERT INTO workspaces (
         id, name, root_directory, created_at, updated_at, last_opened_at
       ) VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1, 1)`
    )
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('context', 'workspace', 'plain-directory', '/tmp/workspace', 1)`
    )
    database.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, created_at, updated_at,
         sort_key, last_opened_at
       ) VALUES ('task', 'workspace', 'context', 'Task', 'active', 1, 1, 'a', 1)`
    )
    database.run(
      `INSERT INTO scenes (
         id, task_id, name, mode, created_at, updated_at, title_pinned, sort_key, layout_revision
       ) VALUES ('scene', 'task', 'Scene', 'tile', 1, 1, 0, 'a', 1)`
    )
    for (const [id, createdAt] of [['parent', 2], ['child', 3]] as const) {
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at
         ) VALUES (?, 'task', 'context', 'claude-code', 'created', ?, '/tmp/workspace', ?, ?, ?)`,
        id, id, createdAt, createdAt, createdAt
      )
      database.run(
        `INSERT INTO session_mounts (id, scene_id, session_id, created_at)
         VALUES (?, 'scene', ?, ?)`,
        `mount-${id}`, id, createdAt
      )
    }
    const event = database.run(
      `INSERT INTO session_relation_events (
         event_id, relation_id, operation, task_id, from_session_id, to_session_id,
         relation_kind, metadata_json, command_id, occurred_at
       ) VALUES ('event-fork', 'relation-fork', 'created', 'task', 'child', 'parent',
                 'forked-from', '{}', 'command-fork', 4)`
    )
    database.run(
      `INSERT INTO session_relations_current (
         relation_id, task_id, from_session_id, to_session_id, relation_kind,
         metadata_json, created_at, updated_at, source_event_sequence
       ) VALUES ('relation-fork', 'task', 'child', 'parent', 'forked-from', '{}', 4, 4, ?)`,
      Number(event.lastInsertRowid)
    )

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([14, 15, 16, 17, 18, 19, 20, 21, 22, 23])
    expect(database.all(
      `SELECT session_id, scene_id, sibling_created_seq, last_user_interaction_seq
       FROM session_canvas_memberships ORDER BY sibling_created_seq`
    )).toEqual([
      { session_id: 'parent', scene_id: 'scene', sibling_created_seq: 1, last_user_interaction_seq: 0 },
      { session_id: 'child', scene_id: 'scene', sibling_created_seq: 2, last_user_interaction_seq: 0 }
    ])
    expect(database.get(
      `SELECT from_session_id, to_session_id, relation_kind
       FROM session_relations_current WHERE relation_id = 'relation-fork'`
    )).toEqual({ from_session_id: 'child', to_session_id: 'parent', relation_kind: 'forked-from' })
    expect(database.all('SELECT name, value FROM runtime_sequences ORDER BY name')).toEqual([
      { name: 'session-sibling-created', value: 2 },
      { name: 'session-user-interaction', value: 0 }
    ])

    database.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title, cwd,
         created_at, updated_at, last_activity_at
       ) VALUES ('future', 'task', 'context', 'shell', 'created', 'Shell',
                 '/tmp/workspace', 5, 5, 5)`
    )
    database.run(
      `INSERT INTO session_mounts (id, scene_id, session_id, created_at)
       VALUES ('mount-future', 'scene', 'future', 5)`
    )
    expect(database.get(
      `SELECT session_id, scene_id, sibling_created_seq, last_user_interaction_seq
       FROM session_canvas_memberships WHERE session_id = 'future'`
    )).toEqual({
      session_id: 'future', scene_id: 'scene', sibling_created_seq: 3,
      last_user_interaction_seq: 0
    })
  })

  it('promotes inherited Fork identities that older builds left provisional', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 18)).migrate()
    database.run(
      `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
       VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1)`
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
    for (const id of ['parent', 'child']) {
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at
         ) VALUES (?, 'task', 'context', 'claude-code', 'running', ?, '/tmp/workspace', 1, 1, 1)`,
        id, id
      )
    }
    database.run(
      `INSERT INTO session_fork_intents (
         session_id, source_session_id, source_provider, source_provider_session_id,
         state, created_at, started_at, updated_at
       ) VALUES ('child', 'parent', 'claude-code', 'provider-parent',
                 'starting', 1, 2, 2)`
    )
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state,
         metadata_json, created_at, updated_at
       ) VALUES ('binding-child', 'child', 'claude-code', 'provider-child', 'unknown',
                 '{"provisional":true,"lastHookEvent":"unknown"}', 2, 2)`
    )

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([19, 20, 21, 22, 23])
    expect(database.get<{ state: string }>(
      `SELECT state FROM session_fork_intents WHERE session_id = 'child'`
    )).toEqual({ state: 'succeeded' })
    const binding = database.get<{ resume_state: string; validated_at: number; metadata_json: string }>(
      `SELECT resume_state, validated_at, metadata_json
       FROM provider_bindings WHERE id = 'binding-child'`
    )!
    expect(binding.resume_state).toBe('available')
    expect(binding.validated_at).toBe(2)
    expect(JSON.parse(binding.metadata_json)).toMatchObject({
      inheritedConversation: true,
      canFork: true
    })
    expect(JSON.parse(binding.metadata_json)).not.toHaveProperty('provisional')
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
