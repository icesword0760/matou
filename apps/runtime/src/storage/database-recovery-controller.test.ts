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
      action: 'restore-backup', backupId: fixture.backupId
    })

    expect(result.bootstrap?.kind).toBe('writable')
    if (result.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
    expect(result.bootstrap.database.get<{ name: string }>(
      'SELECT name FROM workspaces WHERE id = ?', 'workspace-preserved'
    )).toEqual({ name: 'Preserved Workspace' })
    result.bootstrap.database.close()
    await expect(stat(join(fixture.root, 'matou.sqlite.recovery.json'))).rejects.toMatchObject({ code: 'ENOENT' })
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
            action, backupId: fixture.backupId
          }
        : {
            type: 'runtime.recovery-command', requestId: `fence-${action}`, action
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
      action: 'restore-backup', backupId: 'missing-backup'
    })).rejects.toThrow('missing-backup')

    expect((await openRecoverableRuntimeDatabase(fixture.root, FOUNDATION_MIGRATIONS)).kind)
      .toBe('recovery-required')
    expect(await readFile(join(fixture.root, 'matou.sqlite.recovery.json'), 'utf8'))
      .toContain(fixture.recovery.quarantinedPath)
    await expect(stat(join(fixture.root, 'matou.sqlite.owner')))
      .rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('rejects a stale recovery command after another Runtime completed the recovery', async () => {
    const fixture = await corruptFixture()
    const backup = fixture.recovery.backups[0]!
    await writeFile(join(fixture.root, 'matou.sqlite'), await readFile(backup.path))
    const controller = new DatabaseRecoveryController(fixture.root, FOUNDATION_MIGRATIONS)
    const recovered = await controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'winner', action: 'retry-open'
    })
    if (recovered.bootstrap?.kind !== 'writable') throw new Error('expected writable result')
    recovered.bootstrap.database.close()

    await expect(controller.execute(fixture.recovery, {
      type: 'runtime.recovery-command', requestId: 'stale-empty', action: 'start-empty-database'
    })).rejects.toThrow('已由其他 Runtime 完成')

    const database = RuntimeDatabase.open(join(fixture.root, 'matou.sqlite'))
    expect(database.get<{ name: string }>(
      'SELECT name FROM workspaces WHERE id = ?', 'workspace-preserved'
    )).toEqual({ name: 'Preserved Workspace' })
    database.close()
  })

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
      { type: 'runtime.recovery-command', requestId: 'ownership-retry', action: 'retry-open' }
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
      type: 'runtime.recovery-command', requestId: 'retry-1', action: 'retry-open'
    })
    expect(retried.bootstrap?.kind).toBe('writable')
    if (retried.bootstrap?.kind === 'writable') retried.bootstrap.database.close()

    const empty = await corruptFixture()
    const emptied = await new DatabaseRecoveryController(empty.root, FOUNDATION_MIGRATIONS).execute(
      empty.recovery,
      { type: 'runtime.recovery-command', requestId: 'empty-1', action: 'start-empty-database' }
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
