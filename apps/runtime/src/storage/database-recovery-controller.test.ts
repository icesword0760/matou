import { mkdtemp, readFile, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './database'
import { DatabaseBackupService } from './database-backup-service'
import { DatabaseRecoveryController } from './database-recovery-controller'
import { FOUNDATION_MIGRATIONS } from './migrations'
import { MigrationRunner } from './migration-runner'
import { openRecoverableRuntimeDatabase } from './runtime-database-bootstrap'

describe('database recovery controller', () => {
  it('restores a valid backup and reboots the canonical database', async () => {
    const fixture = await corruptFixture()
    const controller = new DatabaseRecoveryController(fixture.root, FOUNDATION_MIGRATIONS)

    const result = await controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'restore-1',
      action: 'restore-backup', backupId: fixture.backupId,
      expectedRecoveryId: fixture.recovery.recoveryId
    })

    expect(result.bootstrap?.kind).toBe('writable')
    if (result.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
    expect(result.bootstrap.database.get<{ name: string }>(
      'SELECT name FROM workspaces WHERE id = ?', 'workspace-preserved'
    )).toEqual({ name: 'Preserved Workspace' })
    result.bootstrap.database.close()
    expect(JSON.parse(await readFile(
      join(fixture.root, 'matou.sqlite.recovery.json'), 'utf8'
    ))).toMatchObject({ state: 'required', recoveryId: fixture.recovery.recoveryId })
    expect(JSON.parse(await readFile(
      `${fixture.recovery.markerPath}.resolved-${fixture.recovery.recoveryId}`, 'utf8'
    ))).toMatchObject({ recoveryId: fixture.recovery.recoveryId })
    const reopened = await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS)
    expect(reopened.kind).toBe('writable')
    if (reopened.kind === 'writable') reopened.database.close()
  })

  it.each(['restore-backup', 'retry-open', 'start-empty-database'] as const)(
    'keeps the recovery marker visible behind the owner fence during %s',
    async (action) => {
      const fixture = await corruptFixture()
      if (action === 'retry-open') {
        const backup = fixture.recovery.backups[0]!
        await writeFile(join(fixture.root, 'matou.sqlite'), await readFile(backup.path))
      }
      let fenced = false
      const controller = new DatabaseRecoveryController(
        fixture.root,
        FOUNDATION_MIGRATIONS,
        {
          async onRecoveryActionFenced() {
            fenced = true
            expect((await stat(fixture.recovery.markerPath)).isFile()).toBe(true)
            const contender = await openRecoverableRuntimeDatabase(
              fixture.root,
              FOUNDATION_MIGRATIONS
            )
            expect(contender.kind).toBe('recovery-required')
          }
        }
      )

      const result = await controller.execute(fixture.recovery, action === 'restore-backup'
        ? {
            type: 'runtime.recovery-command', requestId: `fence-${action}`,
            action, backupId: fixture.backupId,
            expectedRecoveryId: fixture.recovery.recoveryId
          }
        : {
            type: 'runtime.recovery-command', requestId: `fence-${action}`, action,
            expectedRecoveryId: fixture.recovery.recoveryId
          })

      expect(fenced).toBe(true)
      expect(result.bootstrap?.kind).toBe('writable')
      if (result.bootstrap?.kind === 'writable') result.bootstrap.database.close()
    }
  )

  it('keeps recovery-required and its original error when restore fails', async () => {
    const fixture = await corruptFixture()
    const controller = new DatabaseRecoveryController(fixture.root, FOUNDATION_MIGRATIONS)

    await expect(controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'restore-bad',
      action: 'restore-backup', backupId: 'missing-backup',
      expectedRecoveryId: fixture.recovery.recoveryId
    })).rejects.toThrow('missing-backup')

    expect((await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS)).kind)
      .toBe('recovery-required')
    expect(await readFile(join(fixture.root, 'matou.sqlite.recovery.json'), 'utf8'))
      .toContain(fixture.recovery.quarantinedPath)
    await expect(stat(join(fixture.root, 'matou.sqlite.owner')))
      .rejects.toMatchObject({ code: 'ENOENT' })
  })

  it.each(['commit', 'fence-close'] as const)(
    'compensates an action fence %s finalization failure before publishing success',
    async (failure) => {
      const fixture = await corruptFixture()
      const backup = fixture.recovery.backups[0]!
      await writeFile(join(fixture.root, 'matou.sqlite'), await readFile(backup.path))
      let openedDatabase: RuntimeDatabase | undefined
      const injected = new Error(`injected ${failure} failure`)
      const controller = new DatabaseRecoveryController(
        fixture.root,
        FOUNDATION_MIGRATIONS,
        { onDatabaseOpened: (database) => { openedDatabase = database } },
        {
          actionFenceObserver: failure === 'commit'
            ? { beforeCommit: () => { throw injected } }
            : { beforeClose: () => { throw injected } }
        }
      )

      await expect(controller.execute(fixture.recovery, {
        type: 'runtime.recovery-command', requestId: `failure-${failure}`,
        action: 'retry-open', expectedRecoveryId: fixture.recovery.recoveryId
      })).rejects.toThrow(injected.message)

      expect((await stat(fixture.recovery.markerPath)).isFile()).toBe(true)
      await expect(stat(join(fixture.root, 'matou.sqlite.owner')))
        .rejects.toMatchObject({ code: 'ENOENT' })
      expect(() => openedDatabase?.get('SELECT 1')).toThrow('database is closed')
      const next = await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS)
      expect(next).toMatchObject({
        kind: 'recovery-required', recoveryId: fixture.recovery.recoveryId
      })
    }
  )

  it('keeps a committed tombstone successful when later durability reporting fails', async () => {
    const fixture = await corruptFixture()
    const backup = fixture.recovery.backups[0]!
    await writeFile(join(fixture.root, 'matou.sqlite'), await readFile(backup.path))
    const controller = new DatabaseRecoveryController(
      fixture.root,
      FOUNDATION_MIGRATIONS,
      {},
      {
        markerFinalizationObserver: {
          afterPublish: () => { throw new Error('persistent post-publish failure') },
          beforeDirectorySync: () => { throw new Error('persistent directory fsync failure') }
        }
      }
    )

    const completed = await controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'post-publish-success',
      action: 'retry-open', expectedRecoveryId: fixture.recovery.recoveryId
    })

    expect(completed.bootstrap?.kind).toBe('writable')
    if (completed.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
    expect(JSON.parse(await readFile(fixture.recovery.markerPath, 'utf8')))
      .toMatchObject({ state: 'required', recoveryId: fixture.recovery.recoveryId })
    expect(JSON.parse(await readFile(
      `${fixture.recovery.markerPath}.resolved-${fixture.recovery.recoveryId}`,
      'utf8'
    ))).toMatchObject({ recoveryId: fixture.recovery.recoveryId })
    completed.bootstrap.database.close()
    const restarted = await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS)
    expect(restarted.kind).toBe('writable')
    if (restarted.kind === 'writable') restarted.database.close()
  })

  it.each([
    'file-sync',
    'namespace-publish',
    'database-close',
    'owner-unlink',
    'namespace-and-owner-unlink'
  ] as const)(
    'keeps the same durable recovery cycle after a %s finalization failure',
    async (failure) => {
      const fixture = await corruptFixture()
      const backup = fixture.recovery.backups[0]!
      await writeFile(join(fixture.root, 'matou.sqlite'), await readFile(backup.path))
      const injected = new Error(`injected ${failure} failure`)
      let markerFailures = 1
      let closeFailures = failure === 'database-close' ? 1 : 0
      let ownerFailures = ['owner-unlink', 'namespace-and-owner-unlink'].includes(failure) ? 1 : 0
      const openedDatabases: RuntimeDatabase[] = []
      const controller = new DatabaseRecoveryController(
        fixture.root,
        FOUNDATION_MIGRATIONS,
        { onDatabaseOpened: (database) => { openedDatabases.push(database) } },
        {
          markerFinalizationObserver: {
            beforeFileSync: () => {
              if (markerFailures > 0 && failure === 'file-sync') {
                markerFailures -= 1
                throw injected
              }
            },
            beforePublish: () => {
              if (markerFailures > 0 && (
                failure === 'namespace-publish' || failure === 'database-close' ||
                failure === 'owner-unlink' || failure === 'namespace-and-owner-unlink'
              )) {
                markerFailures -= 1
                throw injected
              }
            }
          },
          cleanupObserver: {
            beforeDatabaseClose: () => {
              if (closeFailures > 0) {
                closeFailures -= 1
                throw new Error('injected database close failure')
              }
            },
            beforeOwnerRelease: () => {
              if (ownerFailures > 0) {
                ownerFailures -= 1
                throw new Error('injected owner unlink failure')
              }
            }
          }
        }
      )

      await expect(controller.execute(fixture.recovery, {
        type: 'runtime.recovery-command', requestId: `durable-${failure}`,
        action: 'retry-open', expectedRecoveryId: fixture.recovery.recoveryId
      })).rejects.toThrow()

      expect(JSON.parse(await readFile(fixture.recovery.markerPath, 'utf8')))
        .toMatchObject({ state: 'required', recoveryId: fixture.recovery.recoveryId })
      const next = await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS)
      expect(next).toMatchObject({
        kind: 'recovery-required', recoveryId: fixture.recovery.recoveryId
      })
      if (failure === 'database-close') {
        expect(openedDatabases[0]?.get('SELECT 1')).toBeTruthy()
      } else {
        expect(() => openedDatabases[0]?.get('SELECT 1')).toThrow('database is closed')
      }
      const ownerShouldBePending = [
        'database-close', 'owner-unlink', 'namespace-and-owner-unlink'
      ].includes(failure)
      if (ownerShouldBePending) {
        expect((await stat(join(fixture.root, 'matou.sqlite.owner'))).isFile()).toBe(true)
      } else {
        await expect(stat(join(fixture.root, 'matou.sqlite.owner')))
          .rejects.toMatchObject({ code: 'ENOENT' })
      }

      const recovered = await controller.execute(fixture.recovery, {
        type: 'runtime.recovery-command', requestId: `durable-retry-${failure}`,
        action: 'retry-open', expectedRecoveryId: fixture.recovery.recoveryId
      })
      expect(recovered.bootstrap?.kind).toBe('writable')
      if (recovered.bootstrap?.kind === 'writable') recovered.bootstrap.database.close()
      await expect(stat(join(fixture.root, 'matou.sqlite.owner')))
        .rejects.toMatchObject({ code: 'ENOENT' })
    }
  )

  it('rejects a stale recovery command after another Runtime completed the recovery', async () => {
    const fixture = await corruptFixture()
    const backup = fixture.recovery.backups[0]!
    await writeFile(join(fixture.root, 'matou.sqlite'), await readFile(backup.path))
    const controller = new DatabaseRecoveryController(fixture.root, FOUNDATION_MIGRATIONS)
    const recovered = await controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'winner', action: 'retry-open',
      expectedRecoveryId: fixture.recovery.recoveryId
    })
    if (recovered.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
    recovered.bootstrap.database.close()

    await expect(controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'stale-empty', action: 'start-empty-database',
      expectedRecoveryId: fixture.recovery.recoveryId
    })).rejects.toThrow('已由其他 Runtime 完成')

    const database = RuntimeDatabase.open(join(fixture.root, 'matou.sqlite'))
    expect(database.get<{ name: string }>(
      'SELECT name FROM workspaces WHERE id = ?', 'workspace-preserved'
    )).toEqual({ name: 'Preserved Workspace' })
    database.close()
  })

  it.each(['restore-backup', 'retry-open', 'start-empty-database'] as const)(
    'rejects a %s command replayed from an earlier same-shape recovery cycle',
    async (action) => {
      const fixture = await ownershipRecoveryFixture()
      const controller = new DatabaseRecoveryController(fixture.root, FOUNDATION_MIGRATIONS)
      const firstResult = await controller.execute(fixture.first, {
        type: 'runtime.recovery-command', requestId: `complete-first-${action}`,
        action: 'retry-open', expectedRecoveryId: fixture.first.recoveryId
      })
      if (firstResult.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
      firstResult.bootstrap.database.close()

      await writeFile(`${fixture.databasePath}.owner`, '{"pid":')
      const second = await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS)
      if (second.kind !== 'recovery-required') throw new Error('expected second recovery cycle')
      expect(second.recoveryId).not.toBe(fixture.first.recoveryId)
      const markerBefore = await readFile(second.markerPath)
      const databaseBefore = await readFile(fixture.databasePath)
      const command = action === 'restore-backup'
        ? {
            type: 'runtime.recovery-command' as const,
            requestId: `replay-${action}`, action,
            backupId: fixture.backupId,
            expectedRecoveryId: fixture.first.recoveryId
          }
        : {
            type: 'runtime.recovery-command' as const,
            requestId: `replay-${action}`, action,
            expectedRecoveryId: fixture.first.recoveryId
          }
      const outcome = await controller.execute(second, command).then(
        (value) => ({ value }),
        (error: unknown) => ({ error })
      )
      if ('value' in outcome && outcome.value.bootstrap?.kind === 'writable') {
        outcome.value.bootstrap.database.close()
      }

      expect(outcome).toMatchObject({ error: expect.objectContaining({
        message: expect.stringContaining('恢复周期已更新')
      }) })
      expect(await readFile(second.markerPath)).toEqual(markerBefore)
      expect(await readFile(fixture.databasePath)).toEqual(databaseBefore)
      expect(await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS))
        .toMatchObject({ kind: 'recovery-required', recoveryId: second.recoveryId })
    }
  )

  it('exports retained recovery evidence without changing recovery state', async () => {
    const fixture = await corruptFixture()
    const controller = new DatabaseRecoveryController(fixture.root, FOUNDATION_MIGRATIONS)

    const result = await controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'export-1', action: 'export-recovery-bundle'
    })

    expect(result.value.exportedPath).toContain(join(fixture.root, 'recovery-exports'))
    expect(JSON.parse(await readFile(join(result.value.exportedPath!, 'manifest.json'), 'utf8')))
      .toMatchObject({ reason: 'physical-corruption', backupCount: 1 })
    expect((await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS)).kind)
      .toBe('recovery-required')
  })

  it('repairs ownership recovery under the action fence without moving the canonical database', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-ownership-recovery-control-'))
    const databasePath = join(root, 'matou.sqlite')
    const database = RuntimeDatabase.open(databasePath)
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    database.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'ownership-preserved', 'Ownership Preserved', root, 1, 1
    )
    database.close()
    await writeFile(`${databasePath}.owner`, '{"pid":')
    const recovery = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    if (recovery.kind !== 'recovery-required') throw new Error('expected ownership recovery')
    expect(recovery.reason).toBe('ownership-recovery-required')

    const result = await new DatabaseRecoveryController(root, FOUNDATION_MIGRATIONS).execute(
      recovery,
      {
        type: 'runtime.recovery-command', requestId: 'ownership-retry', action: 'retry-open',
        expectedRecoveryId: recovery.recoveryId
      }
    )

    expect(result.bootstrap?.kind).toBe('writable')
    if (result.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
    expect(result.bootstrap.database.get<{ name: string }>(
      'SELECT name FROM workspaces WHERE id = ?', 'ownership-preserved'
    )).toEqual({ name: 'Ownership Preserved' })
    result.bootstrap.database.close()
    expect(await readFile(join(
      root, 'recovery-evidence', 'ownership-retry', 'matou.sqlite.owner'
    ), 'utf8')).toBe('{"pid":')
  })

  it('rechecks a repaired canonical database and starts empty only after the explicit command', async () => {
    const repaired = await corruptFixture()
    const repairedBackup = repaired.recovery.backups[0]!
    await writeFile(join(repaired.root, 'matou.sqlite'), await readFile(repairedBackup.path))
    const controller = new DatabaseRecoveryController(repaired.root, FOUNDATION_MIGRATIONS)
    const retried = await controller.execute(repaired.recovery, {
      type: 'runtime.recovery-command', requestId: 'retry-1', action: 'retry-open',
      expectedRecoveryId: repaired.recovery.recoveryId
    })
    expect(retried.bootstrap?.kind).toBe('writable')
    if (retried.bootstrap?.kind === 'writable') retried.bootstrap.database.close()

    const empty = await corruptFixture()
    const emptied = await new DatabaseRecoveryController(empty.root, FOUNDATION_MIGRATIONS).execute(
      empty.recovery,
      {
        type: 'runtime.recovery-command', requestId: 'empty-1', action: 'start-empty-database',
        expectedRecoveryId: empty.recovery.recoveryId
      }
    )
    expect(emptied.bootstrap?.kind).toBe('writable')
    if (emptied.bootstrap?.kind !== 'writable') throw new Error('expected writable empty database')
    expect(emptied.bootstrap.database.get('SELECT name FROM workspaces WHERE id = ?', 'workspace-preserved'))
      .toBeUndefined()
    emptied.bootstrap.database.close()
    expect((await stat(empty.recovery.quarantinedPath)).isFile()).toBe(true)
  })
})

async function corruptFixture() {
  const root = await mkdtemp(join(tmpdir(), 'matou-recovery-control-'))
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
  const root = await mkdtemp(join(tmpdir(), 'matou-recovery-replay-'))
  const databasePath = join(root, 'matou.sqlite')
  const database = RuntimeDatabase.open(databasePath)
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  database.run(
    'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
    'replay-preserved', 'Replay Preserved', root, 1, 1
  )
  const backup = await new DatabaseBackupService(root).create(database, 'clean-exit')
  database.close()
  await writeFile(`${databasePath}.owner`, '{"pid":')
  const first = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
  if (first.kind !== 'recovery-required') throw new Error('expected first recovery cycle')
  return { root, databasePath, first, backupId: backup.id }
}
