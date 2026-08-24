import { mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { CheckpointManager } from './checkpoint-manager'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let root: string
let database: RuntimeDatabase
let checkpoints: CheckpointManager

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-checkpoint-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seedSession(database)
  checkpoints = new CheckpointManager(root, database, { retainGenerations: 2 })
})

afterEach(() => database.close())

describe('CheckpointManager', () => {
  it('writes and restores a checkpoint carrying both stream watermarks', async () => {
    const created = await checkpoints.create({
      sessionId: 'session-1',
      terminalSequence: 12,
      domainEventSequence: 7,
      screenEpoch: 2,
      snapshot: Uint8Array.from([1, 2, 3])
    })

    expect(created.generation).toBe(1)
    await expect(
      checkpoints.loadLatest('session-1', {
        terminalSequence: 12,
        domainEventSequence: 7
      })
    ).resolves.toEqual({
      id: created.id,
      generation: 1,
      terminalSequence: 12,
      domainEventSequence: 7,
      screenEpoch: 2,
      snapshot: Uint8Array.from([1, 2, 3])
    })
  })

  it('falls back to the previous paired generation when the newest file is corrupt', async () => {
    await checkpoints.create({
      sessionId: 'session-1', terminalSequence: 2, domainEventSequence: 1,
      screenEpoch: 1, snapshot: Uint8Array.from([1])
    })
    const newest = await checkpoints.create({
      sessionId: 'session-1', terminalSequence: 4, domainEventSequence: 3,
      screenEpoch: 1, snapshot: Uint8Array.from([2])
    })
    const bytes = await readFile(newest.filePath)
    bytes[bytes.length - 1] = bytes[bytes.length - 1]! ^ 0xff
    await writeFile(newest.filePath, bytes)

    await expect(
      checkpoints.loadLatest('session-1', { terminalSequence: 4, domainEventSequence: 3 })
    ).resolves.toMatchObject({ generation: 1, snapshot: Uint8Array.from([1]) })
    expect(
      database.get<{ valid: number }>('SELECT valid FROM journal_checkpoints WHERE id = ?', newest.id)
    ).toEqual({ valid: 0 })
  })

  it('ignores a checkpoint ahead of the recovered journal/event watermark', async () => {
    await checkpoints.create({
      sessionId: 'session-1', terminalSequence: 8, domainEventSequence: 5,
      screenEpoch: 1, snapshot: Uint8Array.from([8])
    })

    await expect(
      checkpoints.loadLatest('session-1', { terminalSequence: 7, domainEventSequence: 5 })
    ).resolves.toBeUndefined()
  })

  it('does not expose a file that crashed before its SQLite index commit', async () => {
    await expect(
      checkpoints.create(
        {
          sessionId: 'session-1', terminalSequence: 1, domainEventSequence: 0,
          screenEpoch: 1, snapshot: Uint8Array.from([1])
        },
        (phase) => {
          if (phase === 'after-file-rename') throw new Error('simulated crash')
        }
      )
    ).rejects.toThrow('simulated crash')

    await expect(
      checkpoints.loadLatest('session-1', { terminalSequence: 1, domainEventSequence: 0 })
    ).resolves.toBeUndefined()
    expect(await checkpoints.removeOrphans('session-1')).toBe(1)
    expect(await readdir(join(root, 'checkpoints', 'session-1'))).toEqual([])
  })

  it('retains the newest two valid paired generations', async () => {
    for (let generation = 1; generation <= 3; generation += 1) {
      await checkpoints.create({
        sessionId: 'session-1',
        terminalSequence: generation,
        domainEventSequence: generation,
        screenEpoch: 1,
        snapshot: Uint8Array.from([generation])
      })
    }

    expect(
      database.all<{ generation: number }>(
        'SELECT generation FROM journal_checkpoints WHERE session_id = ? ORDER BY generation',
        'session-1'
      )
    ).toEqual([{ generation: 2 }, { generation: 3 }])
    expect((await readdir(join(root, 'checkpoints', 'session-1'))).sort()).toHaveLength(2)
  })
})

function seedSession(db: RuntimeDatabase): void {
  db.transaction((tx) => {
    tx.run('INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)', 'workspace-1', 'Workspace', '/tmp/workspace', 1, 1)
    tx.run('INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)', 'context-1', 'workspace-1', 'plain-directory', '/tmp/workspace', 1)
    tx.run('INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)', 'task-1', 'workspace-1', 'context-1', 'Task', 'active', 1, 1)
    tx.run('INSERT INTO sessions (id, task_id, execution_context_id, kind, status, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)', 'session-1', 'task-1', 'context-1', 'shell', 'running', 1, 1, 1)
  })
}
