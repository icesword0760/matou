import { createHash } from 'node:crypto'
import { mkdtemp } from 'node:fs/promises'
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
  it('installs the durable Fork batch ledger as migration 29', async () => {
    const { database } = await createDatabase()

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.currentVersion).toBe(29)
    expect(database.all<{ name: string }>(
      "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'fork_batch_%' ORDER BY name"
    )).toEqual([
      { name: 'fork_batch_items' },
      { name: 'fork_batch_ledger' }
    ])
  })

  it('creates the complete foundation schema from an empty database', async () => {
    const { database } = await createDatabase()

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29])
    expect(result.currentVersion).toBe(29)
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
        'session_environment_bindings',
        'session_environment_transitions',
        'execution_context_git_states',
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
    expect(database.get<{ name: string }>(
      "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'session_relations_structural_lookup_idx'"
    )).toEqual({ name: 'session_relations_structural_lookup_idx' })
  })

  it('is idempotent when every migration is already applied', async () => {
    const { database } = await createDatabase()
    const runner = new MigrationRunner(database, FOUNDATION_MIGRATIONS)
    await runner.migrate()

    await expect(runner.migrate()).resolves.toEqual({
      appliedVersions: [],
      currentVersion: 29,
      backupPath: undefined
    })
    expect(database.all<{ checksum: string }>(
      'SELECT checksum FROM schema_migrations ORDER BY version'
    ).every(({ checksum }) => /^v2:[a-f0-9]{64}$/.test(checksum))).toBe(true)
  })

  it('treats a migration name as editable metadata instead of executable identity', async () => {
    const { database } = await createDatabase()
    const original: Migration = {
      version: 1,
      name: 'initial-description',
      sql: 'CREATE TABLE rename_safe (id TEXT PRIMARY KEY) STRICT;'
    }
    await new MigrationRunner(database, [original]).migrate()
    const before = database.get<{ checksum: string }>(
      'SELECT checksum FROM schema_migrations WHERE version = 1'
    )!.checksum

    await expect(new MigrationRunner(database, [{
      ...original,
      name: 'updated-description'
    }]).migrate()).resolves.toMatchObject({ appliedVersions: [], currentVersion: 1 })

    expect(database.get<{ name: string; checksum: string }>(
      'SELECT name, checksum FROM schema_migrations WHERE version = 1'
    )).toEqual({ name: 'updated-description', checksum: before })
  })

  it('upgrades a v23 database that records the original migration 18 checksum', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 23)).migrate()
    for (const migration of FOUNDATION_MIGRATIONS.slice(0, 23)) {
      database.run(
        `UPDATE schema_migrations SET checksum = ? WHERE version = ?`,
        legacyChecksum(migration), migration.version
      )
    }
    database.run(`UPDATE schema_migrations SET name = ?, checksum = ? WHERE version = 18`,
      'historical-shell-command-blocks',
      'b34eff91ec349bd3472ab71c46b0bd840ab08f0cafc06957a795187d9d64b0bd')

    await expect(new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()).resolves.toMatchObject({
      appliedVersions: [24, 25, 26, 27, 28, 29],
      currentVersion: 29
    })
    expect(database.all<{ checksum: string }>(
      'SELECT checksum FROM schema_migrations ORDER BY version'
    ).every(({ checksum }) => /^v2:[a-f0-9]{64}$/.test(checksum))).toBe(true)
  })

  it('maps legacy Fork states into durable v27 operation stages', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 26)).migrate()
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
    const legacyCases = (['current', 'new'] as const).flatMap((mode) =>
      (['pending', 'starting', 'succeeded', 'failed'] as const).map((state) => ({
        sessionId: `${mode}-${state}`, mode, state
      }))
    )
    for (const id of ['parent', ...legacyCases.map(({ sessionId }) => sessionId)]) {
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at
         ) VALUES (?, 'task', 'context', 'claude-code', 'running', ?, '/tmp/workspace', 1, 1, 1)`,
        id, id
      )
    }
    for (const { sessionId, mode, state } of legacyCases) {
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, worktree_mode, created_at, attempt_count, updated_at
         ) VALUES (?, 'parent', 'claude-code', 'provider-parent', ?, ?, 1, 2, 1)`,
        sessionId, state, mode
      )
    }

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([27, 28, 29])
    expect(database.all(
      `SELECT session_id, operation_id, submission_key, stage, completed_steps,
              total_steps, attempt, lease_fence
       FROM session_fork_intents ORDER BY session_id`
    )).toEqual([
      durableLegacyRow('current-failed', 'failed', 0, 2),
      durableLegacyRow('current-pending', 'queued', 0, 2),
      durableLegacyRow('current-starting', 'queued', 0, 2),
      durableLegacyRow('current-succeeded', 'succeeded', 2, 2),
      durableLegacyRow('new-failed', 'failed', 0, 5),
      durableLegacyRow('new-pending', 'queued', 0, 5),
      durableLegacyRow('new-starting', 'queued', 0, 5),
      durableLegacyRow('new-succeeded', 'succeeded', 5, 5)
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

    expect(result.appliedVersions).toEqual([21, 22, 23, 24, 25, 26, 27, 28, 29])
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

  it('upgrades a real v21 database with independent local and owned Worktree bindings', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 21)).migrate()
    database.run(
      `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
       VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1)`
    )
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('local-context', 'workspace', 'plain-directory', '/tmp/workspace', 1),
              ('worktree-context', 'workspace', 'git-worktree', '/tmp/worktree', 2)`
    )
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         state, created_at, updated_at
       ) VALUES ('worktree', 'worktree-context', '/tmp/workspace', '/tmp/worktree',
                 'codex/task-6', 'ready', 2, 2)`
    )
    database.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, created_at, updated_at
       ) VALUES ('task', 'workspace', 'local-context', 'Task', 'active', 1, 1)`
    )
    database.run(
      `INSERT INTO scenes (id, task_id, name, mode, created_at, updated_at)
       VALUES ('scene', 'task', 'Scene', 'tile', 1, 1)`
    )
    for (const [id, context, cwd, createdAt] of [
      ['local-session', 'local-context', '/tmp/workspace', 3],
      ['worktree-session', 'worktree-context', '/tmp/worktree', 4]
    ] as const) {
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at
         ) VALUES (?, 'task', ?, 'shell', 'created', ?, ?, ?, ?, ?)`,
        id, context, id, cwd, createdAt, createdAt, createdAt
      )
      database.run(
        `INSERT INTO session_mounts (id, scene_id, session_id, created_at)
         VALUES (?, 'scene', ?, ?)`,
        `mount-${id}`, id, createdAt
      )
    }
    const relationEvent = database.run(
      `INSERT INTO session_relation_events (
         event_id, relation_id, operation, task_id, from_session_id, to_session_id,
         relation_kind, metadata_json, command_id, occurred_at
       ) VALUES ('event', 'relation', 'created', 'task', 'worktree-session',
                 'local-session', 'derived-from', '{}', 'command', 5)`
    )
    database.run(
      `INSERT INTO session_relations_current (
         relation_id, task_id, from_session_id, to_session_id, relation_kind,
         metadata_json, created_at, updated_at, source_event_sequence
       ) VALUES ('relation', 'task', 'worktree-session', 'local-session',
                 'derived-from', '{}', 5, 5, ?)`,
      Number(relationEvent.lastInsertRowid)
    )
    const before = {
      sessions: database.get<{ count: number }>('SELECT COUNT(*) AS count FROM sessions')!.count,
      relations: database.get<{ count: number }>(
        'SELECT COUNT(*) AS count FROM session_relations_current'
      )!.count
    }

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([22, 23, 24, 25, 26, 27, 28, 29])
    expect(result.currentVersion).toBe(29)
    expect(database.all(
      `SELECT session_id, local_execution_context_id, managed_worktree_id,
              active_target, state, error_message, updated_at
       FROM session_environment_bindings ORDER BY session_id`
    )).toEqual([
      {
        session_id: 'local-session', local_execution_context_id: 'local-context',
        managed_worktree_id: null, active_target: 'local', state: 'ready',
        error_message: null, updated_at: 3
      },
      {
        session_id: 'worktree-session', local_execution_context_id: 'local-context',
        managed_worktree_id: 'worktree', active_target: 'worktree', state: 'ready',
        error_message: null, updated_at: 4
      }
    ])
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM sessions')!.count)
      .toBe(before.sessions)
    expect(database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM session_relations_current'
    )!.count).toBe(before.relations)
  })

  it('enforces v24 ownership, state, foreign-key, and cascade constraints', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    database.run(
      `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
       VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1)`
    )
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('local', 'workspace', 'plain-directory', '/tmp/workspace', 1),
              ('worktree-context', 'workspace', 'git-worktree', '/tmp/worktree', 1)`
    )
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         state, created_at, updated_at
       ) VALUES ('worktree', 'worktree-context', '/tmp/workspace', '/tmp/worktree',
                 'codex/task-6', 'ready', 1, 1)`
    )
    database.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, created_at, updated_at
       ) VALUES ('task', 'workspace', 'local', 'Task', 'active', 1, 1)`
    )
    for (const id of ['first', 'second']) {
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title,
           created_at, updated_at, last_activity_at
         ) VALUES (?, 'task', 'local', 'shell', 'created', ?, 1, 1, 1)`,
        id, id
      )
    }
    database.run(
      `UPDATE session_environment_bindings
       SET managed_worktree_id = 'worktree', active_target = 'worktree'
       WHERE session_id = 'first'`
    )

    expect(() => database.run(
      `UPDATE session_environment_bindings
       SET managed_worktree_id = 'worktree', active_target = 'worktree'
       WHERE session_id = 'second'`
    )).toThrow()
    expect(() => database.run(
      `UPDATE session_environment_bindings
       SET active_target = 'invalid' WHERE session_id = 'second'`
    )).toThrow()
    expect(() => database.run(
      `UPDATE session_environment_bindings
       SET active_target = 'local', state = 'missing' WHERE session_id = 'second'`
    )).toThrow()
    expect(() => database.run("DELETE FROM worktrees WHERE id = 'worktree'")).toThrow()

    database.run("DELETE FROM sessions WHERE id = 'first'")
    expect(database.get(
      "SELECT session_id FROM session_environment_bindings WHERE session_id = 'first'"
    )).toBeUndefined()
    expect(() => database.run("DELETE FROM worktrees WHERE id = 'worktree'"))
      .not.toThrow()
  })

  it('backfills registered Worktree Git state when upgrading a real v24 database', async () => {
    const { database } = await createDatabase()
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 24)).migrate()
    database.run(
      `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
       VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1)`
    )
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('branch-context', 'workspace', 'git-worktree', '/tmp/branch', 1),
              ('detached-context', 'workspace', 'git-worktree', '/tmp/detached', 1)`
    )
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         base_revision, state, created_at, updated_at
       ) VALUES
         ('branch-worktree', 'branch-context', '/tmp/workspace', '/tmp/branch',
          'feature/shared', 'abc123', 'dirty', 1, 2),
         ('detached-worktree', 'detached-context', '/tmp/workspace', '/tmp/detached',
          '(detached)', 'def456', 'ready', 1, 3)`
    )

    const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

    expect(result.appliedVersions).toEqual([25, 26, 27, 28, 29])
    expect(database.all(
      `SELECT execution_context_id, repository_root, state, branch, detached_head,
              dirty, error_message, updated_at
       FROM execution_context_git_states ORDER BY execution_context_id`
    )).toEqual([
      {
        execution_context_id: 'branch-context', repository_root: '/tmp/workspace',
        state: 'ready', branch: 'feature/shared', detached_head: null,
        dirty: 1, error_message: null, updated_at: 2
      },
      {
        execution_context_id: 'detached-context', repository_root: '/tmp/workspace',
        state: 'ready', branch: null, detached_head: 'def456',
        dirty: 0, error_message: null, updated_at: 3
      }
    ])
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('constraint-ready', 'workspace', 'plain-directory', '/tmp/ready', 4),
              ('constraint-unavailable', 'workspace', 'plain-directory', '/tmp/unavailable', 4)`
    )
    expect(() => database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, detached_head,
         dirty, updated_at
       ) VALUES ('constraint-ready', '/tmp/workspace', 'ready', NULL, NULL, 0, 4)`
    )).toThrow()
    expect(() => database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, detached_head,
         dirty, updated_at
       ) VALUES ('constraint-unavailable', '/tmp/workspace', 'unavailable', NULL, NULL, 1, 4)`
    )).toThrow()
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('cascade-context', 'workspace', 'plain-directory', '/tmp/cascade', 4)`
    )
    database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, detached_head,
         dirty, error_message, updated_at
       ) VALUES ('cascade-context', NULL, 'unavailable', NULL, NULL, 0, 'path-missing', 4)`
    )
    database.run("DELETE FROM execution_contexts WHERE id = 'cascade-context'")
    expect(database.get<{ count: number }>(
      `SELECT COUNT(*) AS count FROM execution_context_git_states
       WHERE execution_context_id = 'cascade-context'`
    )).toEqual({ count: 0 })
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
      FOUNDATION_MIGRATIONS[22]!,
      FOUNDATION_MIGRATIONS[23]!,
      FOUNDATION_MIGRATIONS[24]!,
      FOUNDATION_MIGRATIONS[25]!,
      FOUNDATION_MIGRATIONS[26]!,
      FOUNDATION_MIGRATIONS[27]!,
      FOUNDATION_MIGRATIONS[28]!
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
    ).rejects.toThrow('database schema version 99 is newer than supported version 29')
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

    expect(result.appliedVersions).toEqual([12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29])
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

    expect(result.appliedVersions).toEqual([14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29])
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

    expect(result.appliedVersions).toEqual([19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29])
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

  it('waits for the pre-migration backup before applying the first pending migration', async () => {
    const { database } = await createDatabase()
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

    let completeBackup!: () => void
    const backupComplete = new Promise<void>((resolve) => { completeBackup = resolve })
    const events: string[] = []
    const backupService = {
      async create(_database: RuntimeDatabase, reason: 'pre-migration') {
        events.push(`backup:${reason}`)
        await backupComplete
        return {
          id: 'pre-migration', path: '/backups/pre-migration.sqlite', createdAt: 1,
          reason, schemaVersion: 1, size: 1, sha256: 'a'.repeat(64)
        }
      },
      async rotate() { events.push('rotate') }
    }
    const migrating = new MigrationRunner(database, [first, second], backupService).migrate()

    await new Promise((resolve) => setImmediate(resolve))
    expect(events).toEqual(['backup:pre-migration'])
    expect(database.get(
      "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'second_table'"
    )).toBeUndefined()
    completeBackup()
    const result = await migrating

    expect(result.appliedVersions).toEqual([2])
    expect(result.backupPath).toBe('/backups/pre-migration.sqlite')
    expect(events).toEqual(['backup:pre-migration', 'rotate'])
  })

  it('publishes deterministic interruption points around a durable migration', async () => {
    const { database } = await createDatabase()
    const first: Migration = {
      version: 1,
      name: 'observable-migration',
      sql: 'CREATE TABLE observable_table (id TEXT PRIMARY KEY) STRICT;'
    }
    const events: string[] = []
    const backups = {
      async create(_database: RuntimeDatabase, reason: 'pre-migration') {
        events.push(`backup:${reason}`)
        return {
          id: 'observable-backup', path: '/backups/observable.sqlite', createdAt: 1,
          reason, schemaVersion: 0, size: 1, sha256: 'a'.repeat(64)
        }
      },
      async rotate() { events.push('rotate') }
    }

    await new MigrationRunner(database, [first], backups, {
      onPreMigrationBackupReady: (backup) => events.push(`ready:${backup.id}`),
      onMigrationTransactionPrepared: (migration) => {
        events.push(`prepared:${migration.version}`)
        expect(database.get(
          "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'observable_table'"
        )).toEqual({ name: 'observable_table' })
        expect(database.all('SELECT * FROM schema_migrations')).toEqual([])
      },
      onMigrationCommitted: (migration) => events.push(`committed:${migration.version}`)
    }).migrate()

    expect(events).toEqual([
      'backup:pre-migration',
      'rotate',
      'ready:observable-backup',
      'prepared:1',
      'committed:1'
    ])
  })
})

function durableLegacyRow(
  sessionId: string,
  stage: string,
  completedSteps: number,
  totalSteps: number
) {
  return {
    session_id: sessionId,
    operation_id: `legacy-operation:${sessionId}`,
    submission_key: `legacy-submission:${sessionId}`,
    stage,
    completed_steps: completedSteps,
    total_steps: totalSteps,
    attempt: 2,
    lease_fence: 0
  }
}

function legacyChecksum(migration: Migration): string {
  return createHash('sha256')
    .update(`${migration.version}\0${migration.name}\0${migration.sql}`)
    .digest('hex')
}
