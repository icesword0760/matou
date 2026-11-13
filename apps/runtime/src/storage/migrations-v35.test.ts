import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './database'
import { DatabaseBackupService } from './database-backup-service'
import { MigrationRunner, type Migration } from './migration-runner'
import { FOUNDATION_MIGRATIONS } from './migrations'

const roots: string[] = []

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

async function openTempDatabase() {
  const root = await mkdtemp(join(tmpdir(), 'matou-v35-migration-'))
  roots.push(root)
  const dbPath = join(root, 'runtime.sqlite')
  const database = RuntimeDatabase.open(dbPath)
  return { root, dbPath, database }
}

describe('migration v35 (expand-agent-provider-constraints)', () => {
  it('upgrades v34 with multi-table foreign keys, triggers, indexes, and automatic pre-migration backup intact', async () => {
    const { root, database } = await openTempDatabase()
    try {
      const backups = new DatabaseBackupService(root)
      await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 34)).migrate()

      database.run(
        `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
         VALUES ('ws-1', 'Workspace', '/tmp/ws', 100, 100)`
      )
      database.run(
        `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
         VALUES ('ec-1', 'ws-1', 'plain-directory', '/tmp/ws', 100)`
      )
      database.run(
        `INSERT INTO tasks (
           id, workspace_id, execution_context_id, title, status, created_at, updated_at
         ) VALUES ('task-1', 'ws-1', 'ec-1', 'Task 1', 'active', 100, 100)`
      )
      database.run(
        `INSERT INTO scenes (id, task_id, name, mode, created_at, updated_at)
         VALUES ('scene-1', 'task-1', 'Scene 1', 'tile', 100, 100)`
      )

      // Exercise both v10 and v24 triggers before v35 upgrade by inserting cwd = ''
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, created_at, updated_at,
           last_activity_at, title, title_source, provider_title
         ) VALUES ('sess-src', 'task-1', 'ec-1', 'claude-code', 'running', 101, 102, 103,
                   'Auto Title', 'auto', 'Auto Title')`
      )
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, created_at, updated_at,
           last_activity_at, title, cwd, title_source
         ) VALUES ('sess-child', 'task-1', 'ec-1', 'codex', 'waiting', 104, 105, 106,
                   'Manual Child', '/tmp/ws/sub', 'manual')`
      )
      database.run(
        `INSERT INTO session_runs (
           id, session_id, ordinal, runtime_generation, profile, status, started_at
         ) VALUES ('run-1', 'sess-src', 1, 'gen-1', 'claude-code', 'running', 101)`
      )
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at, restore_state
         ) VALUES ('bind-1', 'sess-src', 'claude-code', 'prov-1', 'available',
                   '{"permissionMode":"default"}', 101, 102, 102, 'none')`
      )
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at, updated_at, operation_id, submission_key, stage
         ) VALUES ('sess-child', 'sess-src', 'claude-code', 'prov-1',
                   'succeeded', 104, 105, 'op-1', 'sub-1', 'succeeded')`
      )
      database.run(
        `INSERT INTO session_mounts (id, scene_id, session_id, created_at)
         VALUES ('mount-1', 'scene-1', 'sess-src', 101),
                ('mount-2', 'scene-1', 'sess-child', 104)`
      )
      const relationEvent = database.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id, to_session_id,
           relation_kind, metadata_json, command_id, occurred_at
         ) VALUES ('ev-1', 'rel-1', 'created', 'task-1', 'sess-src', 'sess-child',
                   'derived-from', '{}', 'cmd-1', 105)`
      )
      database.run(
        `INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind,
           metadata_json, created_at, updated_at, source_event_sequence
         ) VALUES ('rel-1', 'task-1', 'sess-src', 'sess-child',
                   'derived-from', '{}', 105, 105, ?)`,
        Number(relationEvent.lastInsertRowid)
      )

      const sessionsBefore = database.all('SELECT * FROM sessions ORDER BY id')
      const bindingsBefore = database.all('SELECT * FROM provider_bindings ORDER BY id')
      const forksBefore = database.all('SELECT * FROM session_fork_intents ORDER BY session_id')
      const envBindingsBefore = database.all(
        'SELECT * FROM session_environment_bindings ORDER BY session_id'
      )

      const result = await new MigrationRunner(database, FOUNDATION_MIGRATIONS, backups).migrate()

      expect(result.appliedVersions).toEqual([35])
      expect(result.currentVersion).toBe(35)
      expect(result.backupPath).toBeTruthy()
      expect(existsSync(result.backupPath!)).toBe(true)

      expect(database.all('PRAGMA foreign_key_check')).toEqual([])
      expect(database.get<{ foreign_keys: number }>('PRAGMA foreign_keys')?.foreign_keys).toBe(1)
      expect(
        database.get<{ legacy_alter_table: number }>('PRAGMA legacy_alter_table')
          ?.legacy_alter_table
      ).toBe(0)

      const triggers = database
        .all<{ name: string }>(
          "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'sessions' ORDER BY name"
        )
        .map(({ name }) => name)
      expect(triggers).toEqual([
        'session_environment_binding_after_session_insert',
        'sessions_default_cwd_after_insert'
      ])

      const indexes = database
        .all<{ name: string }>(
          "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name IN ('sessions', 'session_fork_intents') AND sql IS NOT NULL ORDER BY name"
        )
        .map(({ name }) => name)
      expect(indexes).toEqual([
        'session_fork_intents_lease_idx',
        'session_fork_intents_operation_idx',
        'session_fork_intents_source_idx',
        'session_fork_intents_submission_idx',
        'sessions_task_idx'
      ])

      expect(database.all('SELECT * FROM sessions ORDER BY id')).toEqual(sessionsBefore)
      expect(database.all('SELECT * FROM provider_bindings ORDER BY id')).toEqual(bindingsBefore)
      expect(database.all('SELECT * FROM session_fork_intents ORDER BY session_id')).toEqual(
        forksBefore
      )
      expect(
        database.all('SELECT * FROM session_environment_bindings ORDER BY session_id')
      ).toEqual(envBindingsBefore)

      // Both triggers must still fire for newly inserted antigravity sessions
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, created_at, updated_at,
           last_activity_at, title
         ) VALUES ('sess-agy', 'task-1', 'ec-1', 'antigravity', 'created', 200, 200, 200,
                   'Antigravity')`
      )
      expect(
        database.get<{ cwd: string }>('SELECT cwd FROM sessions WHERE id = ?', 'sess-agy')
      ).toEqual({ cwd: '/tmp/ws' })
      expect(
        database.get<{ local_execution_context_id: string; state: string }>(
          'SELECT local_execution_context_id, state FROM session_environment_bindings WHERE session_id = ?',
          'sess-agy'
        )
      ).toEqual({ local_execution_context_id: 'ec-1', state: 'ready' })

      expect(() =>
        database.run(
          `INSERT INTO provider_bindings (
             id, session_id, provider, provider_session_id, resume_state, created_at, updated_at
           ) VALUES ('bind-agy', 'sess-agy', 'antigravity', 'conv-1', 'available', 200, 200)`
        )
      ).not.toThrow()

      expect(() =>
        database.run(
          `INSERT INTO session_fork_intents (
             session_id, source_session_id, source_provider, source_provider_session_id,
             state, created_at, updated_at, operation_id, submission_key
           ) VALUES ('sess-agy', 'sess-src', 'antigravity', 'conv-src', 'pending',
                     200, 200, 'op-agy', 'sub-agy')`
        )
      ).not.toThrow()

      // Reject empty/whitespace CHECK violations and broken FKs
      expect(() =>
        database.run(
          `INSERT INTO sessions (
             id, task_id, execution_context_id, kind, status, created_at, updated_at, last_activity_at
           ) VALUES ('bad-kind', 'task-1', 'ec-1', '   ', 'created', 1, 1, 1)`
        )
      ).toThrow()
      expect(() =>
        database.run(
          `INSERT INTO session_runs (
             id, session_id, ordinal, runtime_generation, profile, status, started_at
           ) VALUES ('bad-fk', 'nonexistent-session', 1, 'gen-1', 'antigravity', 'running', 1)`
        )
      ).toThrow()
    } finally {
      database.close()
    }
  })

  it('aborts and rolls back when foreignKeysOff migration leaves foreign key violations', async () => {
    const { database } = await openTempDatabase()
    try {
      const broken: Migration[] = [
        ...FOUNDATION_MIGRATIONS.slice(0, 35),
        {
          version: 36,
          name: 'broken-fk-migration',
          foreignKeysOff: true,
          sql: `
            INSERT INTO session_runs (
              id, session_id, ordinal, runtime_generation, profile, status, started_at
            ) VALUES ('orphan-run', 'missing-session', 1, 'g', 'shell', 'running', 1);
          `
        }
      ]
      await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()

      await expect(new MigrationRunner(database, broken).migrate()).rejects.toThrow(
        /foreign key violation/i
      )
      expect(
        database.get<{ version: number }>('SELECT MAX(version) AS version FROM schema_migrations')
      ).toEqual({ version: 35 })
      expect(database.get<{ foreign_keys: number }>('PRAGMA foreign_keys')?.foreign_keys).toBe(1)
      expect(database.all('PRAGMA foreign_key_check')).toEqual([])
    } finally {
      database.close()
    }
  })
})
