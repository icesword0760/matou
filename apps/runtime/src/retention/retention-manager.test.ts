import { mkdir, mkdtemp, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { RetentionManager } from './retention-manager'

let root: string
let database: RuntimeDatabase
let retention: RetentionManager

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-retention-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seed(database)
  retention = new RetentionManager(root, database, new DomainTransactionManager(database))
})

afterEach(() => database.close())

describe('RetentionManager', () => {
  it('creates a dry-run quota plan that preserves all queryable journals and the latest checkpoint pairs', async () => {
    const journal = join(root, 'journal', 'session-1')
    const checkpoints = join(root, 'checkpoints', 'session-1')
    await mkdir(journal, { recursive: true })
    await mkdir(checkpoints, { recursive: true })
    await writeFile(join(journal, 'segment-000001.bin.gz'), Buffer.alloc(80))
    await writeFile(join(journal, 'segment-000002.bin.gz'), Buffer.alloc(80))
    await writeFile(join(journal, 'segment-000003.bin'), Buffer.alloc(80))
    for (let generation = 1; generation <= 3; generation += 1) {
      const path = join(checkpoints, `checkpoint-00000${generation}.bin`)
      await writeFile(path, Buffer.alloc(20))
      database.run(
        `INSERT INTO journal_checkpoints (id, session_id, generation, terminal_sequence,
         domain_event_sequence, screen_epoch, file_path, checksum, created_at)
         VALUES (?, ?, ?, ?, ?, 0, ?, 'sum', ?)`,
        `cp-${generation}`, 'session-1', generation, generation * 10, generation, path, generation
      )
    }

    const plan = await retention.planQuota({ globalBytes: 220, perSessionBytes: 180, checkpointGenerations: 2 })

    expect(plan.dryRun).toBe(true)
    expect(plan.actions.map(({ path }) => path)).not.toContain(join(journal, 'segment-000001.bin.gz'))
    expect(plan.actions.map(({ path }) => path)).not.toContain(join(journal, 'segment-000002.bin.gz'))
    expect(plan.actions.map(({ path }) => path)).toContain(join(checkpoints, 'checkpoint-000001.bin'))
    expect(plan.actions.map(({ path }) => path)).not.toContain(join(journal, 'segment-000003.bin'))
    expect(await stat(join(journal, 'segment-000001.bin.gz'))).toBeDefined()
  })

  it('executes checkpoint cleanup without deleting compressed history or degrading anchors', async () => {
    const journal = join(root, 'journal', 'session-1')
    const checkpoints = join(root, 'checkpoints', 'session-1')
    await mkdir(journal, { recursive: true })
    await mkdir(checkpoints, { recursive: true })
    const old = join(journal, 'segment-000001.bin.gz')
    const active = join(journal, 'segment-000002.bin')
    await writeFile(old, Buffer.alloc(100))
    await writeFile(active, Buffer.alloc(100))
    for (let generation = 1; generation <= 3; generation += 1) {
      const path = join(checkpoints, `checkpoint-00000${generation}.bin`)
      await writeFile(path, Buffer.alloc(20))
      database.run(
        `INSERT INTO journal_checkpoints (id, session_id, generation, terminal_sequence,
         domain_event_sequence, screen_epoch, file_path, checksum, created_at)
         VALUES (?, ?, ?, ?, ?, 0, ?, 'sum', ?)`,
        `cp-${generation}`, 'session-1', generation, generation * 10, generation, path, generation
      )
    }
    database.run(
      `INSERT INTO annotations (id, task_id, session_id, kind, text_snapshot, anchor_json,
       status, created_at, updated_at) VALUES (?, ?, ?, 'todo', 'captured', ?, 'active', 1, 1)`,
      'annotation-1', 'task-1', 'session-1', JSON.stringify({ kind: 'screen-capture', sessionId: 'session-1', sequence: 1, screenEpoch: 0, capturedText: 'captured' })
    )
    const plan = await retention.planQuota({ globalBytes: 220, perSessionBytes: 220, checkpointGenerations: 2 })
    await retention.executeQuota(command('quota'), plan, 10)

    expect(plan.actions).toEqual([
      expect.objectContaining({ kind: 'checkpoint', path: join(checkpoints, 'checkpoint-000001.bin') })
    ])
    expect(await stat(old)).toBeDefined()
    expect(await stat(active)).toBeDefined()
    expect(database.get<{ status: string }>('SELECT status FROM annotations WHERE id = ?', 'annotation-1')?.status).toBe('active')
    expect(database.get<{ event_type: string }>("SELECT event_type FROM domain_events WHERE event_type = 'retention.executed'")?.event_type).toBe('retention.executed')
  })

  it('keeps archive separate from purge and purges session metadata only after explicit execution', async () => {
    const journal = join(root, 'journal', 'session-1')
    await mkdir(journal, { recursive: true })
    await writeFile(join(journal, 'segment-000001.bin'), Buffer.from('journal'))
    database.run(
      `INSERT INTO shell_history_blocks (
         id, session_id, command_text, cwd, output, exit_code, started_at, completed_at
       ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      'block-1', 'session-1', 'printf retained', '/tmp/workspace', 'retained', 0, 1, 2
    )

    retention.archiveSession(command('archive'), 'session-1', 3)
    expect(database.get<{ status: string }>('SELECT status FROM sessions WHERE id = ?', 'session-1')?.status).toBe('archived')
    expect(database.get('SELECT id FROM shell_history_blocks WHERE id = ?', 'block-1')).toBeDefined()
    expect(await stat(journal)).toBeDefined()

    const plan = await retention.planSessionPurge('session-1')
    expect(plan.dryRun).toBe(true)
    expect(database.get('SELECT id FROM sessions WHERE id = ?', 'session-1')).toBeDefined()
    await retention.executeSessionPurge(command('purge'), plan, 4)
    expect(database.get('SELECT id FROM sessions WHERE id = ?', 'session-1')).toBeUndefined()
    expect(database.get('SELECT id FROM shell_history_blocks WHERE id = ?', 'block-1')).toBeUndefined()
    expect(database.get<{ event_type: string }>("SELECT event_type FROM domain_events WHERE event_type = 'retention.session-purged'")?.event_type).toBe('retention.session-purged')
  })

  it('hardens data directories and files to current-user access', async () => {
    const directory = join(root, 'journal', 'session-1')
    const file = join(directory, 'segment-000001.bin')
    await mkdir(directory, { recursive: true, mode: 0o755 })
    await writeFile(file, 'secret', { mode: 0o644 })

    await retention.hardenPrivacyPermissions()

    expect((await stat(join(root, 'journal'))).mode & 0o777).toBe(0o700)
    expect((await stat(directory)).mode & 0o777).toBe(0o700)
    expect((await stat(file)).mode & 0o777).toBe(0o600)
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'retention', requestHash: `hash-${commandId}` }
}

function seed(db: RuntimeDatabase): void {
  db.transaction((tx) => {
    tx.run('INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)', 'workspace-1', 'Workspace', '/tmp/workspace', 1, 1)
    tx.run('INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)', 'context-1', 'workspace-1', 'plain-directory', '/tmp/workspace', 1)
    tx.run('INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)', 'task-1', 'workspace-1', 'context-1', 'Task', 'active', 'a', 1, 1)
    tx.run('INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)', 'session-1', 'task-1', 'context-1', 'shell', 'running', 'Shell', 1, 1, 1)
  })
}
