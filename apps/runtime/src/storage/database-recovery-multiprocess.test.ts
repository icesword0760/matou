import { fork, spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './database'
import { DatabaseBackupService } from './database-backup-service'
import { DatabaseRecoveryController } from './database-recovery-controller'
import { FOUNDATION_MIGRATIONS } from './migrations'
import { MigrationRunner } from './migration-runner'
import { openRecoverableRuntimeDatabase } from './runtime-database-bootstrap'

let fixtureBundleRoot = ''
let contenderEntry = ''

beforeAll(async () => {
  fixtureBundleRoot = await mkdtemp(join(tmpdir(), 'matou-recovery-contender-'))
  const tsup = resolve(process.cwd(), '../../node_modules/.bin/tsup')
  const build = spawnSync(tsup, [
    'src/storage/fixtures/recovery-bootstrap-contender.ts',
    '--format', 'cjs', '--platform', 'node', '--out-dir', fixtureBundleRoot,
    '--silent'
  ], { cwd: process.cwd(), encoding: 'utf8' })
  if (build.status !== 0) {
    throw new Error(`failed to build recovery contender: ${build.stderr || build.stdout}`)
  }
  contenderEntry = join(fixtureBundleRoot, 'recovery-bootstrap-contender.cjs')
})

afterAll(async () => {
  await rm(fixtureBundleRoot, { recursive: true, force: true })
})

describe('database recovery multi-process handoff', () => {
  it('keeps a legacy marker recoveryId stable across real Runtime process restarts', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-legacy-marker-restart-'))
    const databasePath = join(root, 'matou.sqlite')
    const markerPath = `${databasePath}.recovery.json`
    await writeFile(markerPath, JSON.stringify({
      version: 1,
      reason: 'physical-corruption',
      durableDatabasePath: databasePath,
      quarantinedPath: `${databasePath}.corrupt-1`,
      markerPath,
      createdAt: 1
    }))

    const first = await probeResult(root)
    const second = await probeResult(root)
    expect(first).toMatchObject({
      kind: 'recovery-required', recoveryId: expect.stringMatching(/^[A-Za-z0-9._-]+$/)
    })
    expect(second).toEqual(first)
  })

  it.each(['restore-backup', 'retry-open', 'start-empty-database'] as const)(
    'never lets a competing Runtime become ready during %s',
    async (action) => {
      const fixture = await corruptFixture()
      if (action === 'retry-open') {
        const backup = fixture.recovery.backups[0]!
        await writeFile(join(fixture.root, 'matou.sqlite'), await readFile(backup.path))
      }
      const observations: string[] = []

      const controller = new DatabaseRecoveryController(
        fixture.root,
        FOUNDATION_MIGRATIONS,
        {
          async onRecoveryActionFenced() {
            observations.push(await probe(fixture.root))
          }
        }
      )
      const execution = await controller.execute(fixture.recovery, action === 'restore-backup'
        ? {
            type: 'runtime.recovery-command', requestId: `multiprocess-${action}`,
            action, backupId: fixture.backupId,
            expectedRecoveryId: fixture.recovery.recoveryId
          }
        : {
            type: 'runtime.recovery-command', requestId: `multiprocess-${action}`, action,
            expectedRecoveryId: fixture.recovery.recoveryId
          })

      expect(execution.bootstrap?.kind).toBe('writable')
      observations.push(await probe(fixture.root))
      expect(observations).toEqual(['recovery-required', 'owner-conflict'])
      expect(observations).not.toContain('writable')
      expect(observations).not.toContain('read-only')
      if (execution.bootstrap?.kind === 'writable') execution.bootstrap.database.close()
    },
    15_000
  )
})

async function corruptFixture() {
  const root = await mkdtemp(join(tmpdir(), 'matou-recovery-multiprocess-'))
  const databasePath = join(root, 'matou.sqlite')
  const database = RuntimeDatabase.open(databasePath)
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  database.run(
    'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
    'workspace-preserved', 'Preserved Workspace', root, 1, 1
  )
  const backup = await new DatabaseBackupService(root).create(database, 'clean-exit')
  database.close()
  const bytes = await readFile(databasePath)
  bytes.fill(0x7f, 0, 16)
  await writeFile(databasePath, bytes)
  const recovery = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
  if (recovery.kind !== 'recovery-required') throw new Error('expected recovery fixture')
  return { root, recovery, backupId: backup.id }
}

async function probe(dataRoot: string): Promise<string> {
  return (await probeResult(dataRoot)).kind
}

async function probeResult(dataRoot: string): Promise<{ kind: string; recoveryId?: string }> {
  const contender = fork(contenderEntry, [], { stdio: ['ignore', 'ignore', 'ignore', 'ipc'] })
  return new Promise<{ kind: string; recoveryId?: string }>((resolveProbe, reject) => {
    const timeout = setTimeout(() => {
      contender.kill()
      reject(new Error('timed out waiting for competing Runtime bootstrap'))
    }, 5_000)
    contender.once('error', reject)
    contender.once('message', (message: unknown) => {
      clearTimeout(timeout)
      if (!message || typeof message !== 'object' || !('kind' in message)) {
        reject(new Error('competing Runtime returned an invalid observation'))
        return
      }
      resolveProbe({
        kind: String(message.kind),
        ...('recoveryId' in message ? { recoveryId: String(message.recoveryId) } : {})
      })
    })
    contender.send({ dataRoot })
  })
}
