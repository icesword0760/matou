import { fork, spawnSync } from 'node:child_process'
import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
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

  it('publishes one canonical recoveryId when eight Runtime processes upgrade one legacy marker', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-legacy-marker-barrier-'))
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

    const results = await probeTogether(root, 8)
    expect(new Set(results.map(({ recoveryId }) => recoveryId))).toEqual(
      new Set([results[0]!.recoveryId])
    )
    expect(results.every(({ kind }) => kind === 'recovery-required')).toBe(true)
    expect(JSON.parse(await readFile(markerPath, 'utf8')))
      .toMatchObject({ recoveryId: results[0]!.recoveryId, state: 'required' })
  })

  it('publishes one durable recoveryId when eight Runtime processes upgrade legacy evidence with a damaged action fence', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-damaged-fence-legacy-barrier-'))
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
    await writeFile(`${databasePath}.recovery-action.sqlite`, 'not a sqlite database')

    const results = await probeTogether(root, 8)
    expect(results.every(({ kind }) => kind === 'recovery-required')).toBe(true)
    expect(new Set(results.map(({ recoveryId }) => recoveryId))).toEqual(
      new Set([results[0]!.recoveryId])
    )
    expect(JSON.parse(await readFile(markerPath, 'utf8')))
      .toMatchObject({ recoveryId: results[0]!.recoveryId, state: 'required' })
  })

  it('publishes one new recoveryId when eight Runtime processes re-arm a resolved generation', async () => {
    const fixture = await ownershipRecoveryFixture()
    const controller = new DatabaseRecoveryController(fixture.root, FOUNDATION_MIGRATIONS)
    const completed = await controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'resolve-before-barrier',
      action: 'retry-open', expectedRecoveryId: fixture.recovery.recoveryId
    })
    if (completed.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
    completed.bootstrap.database.close()
    await writeFile(`${fixture.databasePath}.owner`, '{"pid":')

    const results = await probeTogether(fixture.root, 8)
    expect(results.every(({ kind }) => kind === 'recovery-required')).toBe(true)
    expect(new Set(results.map(({ recoveryId }) => recoveryId))).toEqual(
      new Set([results[0]!.recoveryId])
    )
    expect(results[0]!.recoveryId).not.toBe(fixture.recovery.recoveryId)
    expect(JSON.parse(await readFile(fixture.recovery.markerPath, 'utf8')))
      .toMatchObject({ recoveryId: results[0]!.recoveryId, state: 'required' })
  })

  it('publishes one durable recoveryId when eight Runtime processes re-arm resolved evidence with a damaged action fence', async () => {
    const fixture = await ownershipRecoveryFixture()
    const controller = new DatabaseRecoveryController(fixture.root, FOUNDATION_MIGRATIONS)
    const completed = await controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'resolve-before-damaged-fence',
      action: 'retry-open', expectedRecoveryId: fixture.recovery.recoveryId
    })
    if (completed.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
    completed.bootstrap.database.close()
    await writeFile(`${fixture.databasePath}.owner`, '{"pid":')
    await writeFile(`${fixture.databasePath}.recovery-action.sqlite`, 'not a sqlite database')

    const results = await probeTogether(fixture.root, 8)
    expect(results.every(({ kind }) => kind === 'recovery-required')).toBe(true)
    expect(new Set(results.map(({ recoveryId }) => recoveryId))).toEqual(
      new Set([results[0]!.recoveryId])
    )
    expect(results[0]!.recoveryId).not.toBe(fixture.recovery.recoveryId)
    expect(JSON.parse(await readFile(fixture.recovery.markerPath, 'utf8')))
      .toMatchObject({ recoveryId: results[0]!.recoveryId, state: 'required' })
  })

  it.each([
    ['file-sync', false, 'recovery-required'],
    ['precommit-readonly', false, 'recovery-required'],
    ['postcommit-readonly', true, 'writable']
  ] as const)(
    'keeps a new Runtime consistent after a persistent %s finalization fault',
    async (fault, commandOk, nextKind) => {
      const fixture = await corruptFixture()
      const backup = fixture.recovery.backups[0]!
      await writeFile(join(fixture.root, 'matou.sqlite'), await readFile(backup.path))

      const outcome = await runFaultingRecovery(fixture.root, fault)
      await chmod(fixture.root, 0o700)

      expect(outcome).toMatchObject({
        kind: 'recovery-result', ok: commandOk, recoveryId: fixture.recovery.recoveryId
      })
      const next = await probeResult(fixture.root)
      expect(next.kind).toBe(nextKind)
      if (nextKind === 'recovery-required') {
        expect(next.recoveryId).toBe(fixture.recovery.recoveryId)
      }
    },
    20_000
  )

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

async function ownershipRecoveryFixture() {
  const root = await mkdtemp(join(tmpdir(), 'matou-recovery-rearm-barrier-'))
  const databasePath = join(root, 'matou.sqlite')
  const database = RuntimeDatabase.open(databasePath)
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  database.close()
  await writeFile(`${databasePath}.owner`, '{"pid":')
  const recovery = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
  if (recovery.kind !== 'recovery-required') throw new Error('expected ownership recovery')
  return { root, databasePath, recovery }
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

async function probeTogether(
  dataRoot: string,
  count: number
): Promise<Array<{ kind: string; recoveryId?: string }>> {
  const barrierPath = join(dataRoot, `barrier-${Date.now()}`)
  const contenders = Array.from({ length: count }, () => (
    fork(contenderEntry, [], { stdio: ['ignore', 'ignore', 'ignore', 'ipc'] })
  ))
  const results = contenders.map((contender) => new Promise<{
    kind: string
    recoveryId?: string
  }>((resolveProbe, reject) => {
    const timeout = setTimeout(() => {
      contender.kill()
      reject(new Error('timed out waiting for barrier contender'))
    }, 10_000)
    contender.once('error', reject)
    contender.on('message', (message: unknown) => {
      if (!message || typeof message !== 'object' || !('kind' in message)) return
      if (message.kind === 'barrier-ready') return
      clearTimeout(timeout)
      resolveProbe({
        kind: String(message.kind),
        ...('recoveryId' in message ? { recoveryId: String(message.recoveryId) } : {})
      })
    })
  }))
  const ready = contenders.map((contender) => new Promise<void>((resolveReady, reject) => {
    const timeout = setTimeout(() => reject(new Error('barrier contender did not become ready')), 5_000)
    const listener = (message: unknown) => {
      if (!message || typeof message !== 'object' || !('kind' in message)) return
      if (message.kind !== 'barrier-ready') return
      clearTimeout(timeout)
      contender.off('message', listener)
      resolveReady()
    }
    contender.on('message', listener)
  }))
  for (const contender of contenders) contender.send({ dataRoot, barrierPath })
  await Promise.all(ready)
  await writeFile(barrierPath, 'go')
  return Promise.all(results)
}

async function runFaultingRecovery(
  dataRoot: string,
  fault: string
): Promise<{ kind: string; ok: boolean; recoveryId?: string; error?: string }> {
  const contender = fork(contenderEntry, [], { stdio: ['ignore', 'ignore', 'ignore', 'ipc'] })
  return new Promise((resolveProbe, reject) => {
    const timeout = setTimeout(() => {
      contender.kill()
      reject(new Error('timed out waiting for faulting recovery Runtime'))
    }, 15_000)
    let result: { kind: string; ok: boolean; recoveryId?: string; error?: string } | undefined
    contender.on('message', (message: unknown) => {
      if (!message || typeof message !== 'object' || !('kind' in message) || !('ok' in message)) return
      result = {
        kind: String(message.kind),
        ok: Boolean(message.ok),
        ...('recoveryId' in message ? { recoveryId: String(message.recoveryId) } : {}),
        ...('error' in message ? { error: String(message.error) } : {})
      }
    })
    contender.once('error', reject)
    contender.once('exit', () => {
      clearTimeout(timeout)
      if (!result) reject(new Error('faulting recovery Runtime exited without a result'))
      else resolveProbe(result)
    })
    contender.send({ dataRoot, mode: 'recover-fault', fault })
  })
}
