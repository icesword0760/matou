import { createHash } from 'node:crypto'
import { copyFile, mkdtemp, readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './database'
import {
  DatabaseBackupService,
  type DatabaseBackupDescriptor
} from './database-backup-service'

const { DatabaseSync } = process.getBuiltinModule(
  'node:sqlite'
) as typeof import('node:sqlite')

const opened: RuntimeDatabase[] = []

afterEach(() => {
  for (const database of opened.splice(0)) database.close()
})

describe('DatabaseBackupService', () => {
  it('includes committed WAL rows in an independently valid online backup', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-backup-wal-'))
    const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
    opened.push(database)
    database.exec('CREATE TABLE wal_rows (value TEXT NOT NULL) STRICT;')
    database.run('INSERT INTO wal_rows (value) VALUES (?)', 'committed-in-wal')
    expect((await stat(`${database.path}-wal`)).size).toBeGreaterThan(0)

    const descriptor = await new DatabaseBackupService(root).create(database, 'clean-exit')

    const snapshot = new DatabaseSync(descriptor.path, { readOnly: true })
    try {
      expect(snapshot.prepare('SELECT value FROM wal_rows').get()).toEqual({
        value: 'committed-in-wal'
      })
      expect(snapshot.prepare('PRAGMA integrity_check').get()).toEqual({
        integrity_check: 'ok'
      })
    } finally {
      snapshot.close()
    }
  })

  it('keeps the newest seven valid backups and excludes partial or invalid artifacts', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-backup-rotation-'))
    const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
    opened.push(database)
    database.exec('CREATE TABLE values_table (value INTEGER NOT NULL) STRICT;')
    let timestamp = 1_000
    const service = new DatabaseBackupService(root, { now: () => timestamp++ })

    for (let value = 1; value <= 9; value += 1) {
      database.run('INSERT INTO values_table (value) VALUES (?)', value)
      await service.create(database, 'clean-exit')
    }
    await service.rotate()

    const rotated = await service.listValid()
    expect(rotated.map(({ createdAt }) => createdAt)).toEqual([
      1_008, 1_007, 1_006, 1_005, 1_004, 1_003, 1_002
    ])

    const backupDirectory = join(root, 'backups')
    await writeFile(join(backupDirectory, 'ignored.sqlite.partial'), 'partial')
    await addInvalidCopy(rotated[0]!, backupDirectory, 'checksum', false)
    await addInvalidCopy(rotated[1]!, backupDirectory, 'integrity', true)

    expect((await service.listValid()).map(({ createdAt }) => createdAt)).toEqual([
      1_008, 1_007, 1_006, 1_005, 1_004, 1_003, 1_002
    ])
    expect((await readdir(backupDirectory)).some((name) => name.endsWith('.partial'))).toBe(true)
  })

  it('validates a restore before replacing the target and removes stale WAL state', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-backup-restore-'))
    const source = RuntimeDatabase.open(join(root, 'source.sqlite'))
    opened.push(source)
    source.exec('CREATE TABLE restored_rows (value TEXT NOT NULL) STRICT;')
    source.run('INSERT INTO restored_rows (value) VALUES (?)', 'from-backup')
    const service = new DatabaseBackupService(root, { now: () => 2_000 })
    const backup = await service.create(source, 'clean-exit')
    const targetPath = join(root, 'target.sqlite')
    createStandaloneDatabase(targetPath, 'before-restore')
    await writeFile(`${targetPath}-wal`, 'stale wal')
    await writeFile(`${targetPath}-shm`, 'stale shm')

    await service.restore(backup.id, targetPath)

    const namesAfterRestore = await readdir(root)
    const replacedName = namesAfterRestore.find((name) =>
      /^target\.sqlite\.replaced-\d+$/.test(name)
    )
    expect(replacedName).toBeDefined()
    expect(namesAfterRestore).not.toContain('target.sqlite-wal')
    expect(namesAfterRestore).not.toContain('target.sqlite-shm')
    expect(namesAfterRestore).not.toContain(`${replacedName}-wal`)
    expect(namesAfterRestore).not.toContain(`${replacedName}-shm`)
    const rollback = new DatabaseSync(join(root, replacedName!), { readOnly: true })
    try {
      expect(rollback.prepare('SELECT value FROM original_rows').get()).toEqual({
        value: 'before-restore'
      })
      expect(rollback.prepare('PRAGMA integrity_check').get()).toEqual({ integrity_check: 'ok' })
    } finally {
      rollback.close()
    }
    const restored = new DatabaseSync(targetPath, { readOnly: true })
    try {
      expect(restored.prepare('SELECT value FROM restored_rows').get()).toEqual({
        value: 'from-backup'
      })
      expect(restored.prepare('PRAGMA integrity_check').get()).toEqual({ integrity_check: 'ok' })
    } finally {
      restored.close()
    }
  })

  it.each([
    { targetExists: true, phase: 'old-rollback' as const, expected: 'original' },
    { targetExists: true, phase: 'publish' as const, expected: 'original' },
    { targetExists: false, phase: 'publish' as const, expected: 'from-backup' }
  ])(
    'keeps a valid canonical database when $phase rename fails with targetExists=$targetExists',
    async ({ targetExists, phase, expected }) => {
    const root = await mkdtemp(join(tmpdir(), 'matou-backup-interrupt-'))
    const source = RuntimeDatabase.open(join(root, 'source.sqlite'))
    opened.push(source)
    source.exec('CREATE TABLE restored_rows (value TEXT NOT NULL) STRICT;')
    source.run('INSERT INTO restored_rows (value) VALUES (?)', 'from-backup')
    const creator = new DatabaseBackupService(root, { now: () => 3_000 })
    const backup = await creator.create(source, 'clean-exit')
    const targetPath = join(root, 'target.sqlite')
    if (targetExists) createStandaloneDatabase(targetPath, 'original')
    let injected = false
    const interrupted = new DatabaseBackupService(root, {
      now: () => 4_000,
      rename: async (from, to) => {
        const isRollbackFinalize = to.includes('.replaced-') && !to.endsWith('.partial')
        const isPublish = to === targetPath
        if (!injected && (
          (phase === 'old-rollback' && isRollbackFinalize) ||
          (phase === 'publish' && isPublish)
        )) {
          injected = true
          throw new Error(`injected ${phase} interruption`)
        }
        const { rename } = await import('node:fs/promises')
        await rename(from, to)
      }
    })

    await expect(interrupted.restore(backup.id, targetPath)).rejects.toThrow(
      `injected ${phase} interruption`
    )

    const canonical = new DatabaseSync(targetPath, { readOnly: true })
    try {
      const table = expected === 'original' ? 'original_rows' : 'restored_rows'
      expect(canonical.prepare(`SELECT value FROM ${table}`).get()).toEqual({ value: expected })
      expect(canonical.prepare('PRAGMA integrity_check').get()).toEqual({ integrity_check: 'ok' })
    } finally {
      canonical.close()
    }
    const sourceSnapshot = new DatabaseSync(backup.path, { readOnly: true })
    try {
      expect(sourceSnapshot.prepare('SELECT value FROM restored_rows').get()).toEqual({
        value: 'from-backup'
      })
      expect(sourceSnapshot.prepare('PRAGMA integrity_check').get()).toEqual({
        integrity_check: 'ok'
      })
    } finally {
      sourceSnapshot.close()
    }
  })

  it('solidifies committed target WAL data into an independently valid rollback snapshot', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-backup-old-wal-'))
    const source = RuntimeDatabase.open(join(root, 'source.sqlite'))
    opened.push(source)
    source.exec('CREATE TABLE restored_rows (value TEXT NOT NULL) STRICT;')
    source.run('INSERT INTO restored_rows (value) VALUES (?)', 'from-backup')
    const service = new DatabaseBackupService(root, { now: () => 5_000 })
    const backup = await service.create(source, 'clean-exit')
    const targetPath = join(root, 'target.sqlite')
    const old = new DatabaseSync(targetPath)
    old.exec('PRAGMA journal_mode = WAL; CREATE TABLE original_rows (value TEXT NOT NULL) STRICT;')
    old.prepare('INSERT INTO original_rows (value) VALUES (?)').run('committed-old-wal')
    expect((await stat(`${targetPath}-wal`)).size).toBeGreaterThan(0)

    await service.restore(backup.id, targetPath)
    old.close()

    const replacedName = (await readdir(root)).find((name) =>
      /^target\.sqlite\.replaced-\d+$/.test(name)
    )
    expect(replacedName).toBeDefined()
    const rollback = new DatabaseSync(join(root, replacedName!), { readOnly: true })
    try {
      expect(rollback.prepare('SELECT value FROM original_rows').get()).toEqual({
        value: 'committed-old-wal'
      })
      expect(rollback.prepare('PRAGMA integrity_check').get()).toEqual({ integrity_check: 'ok' })
    } finally {
      rollback.close()
    }
  })
})

async function addInvalidCopy(
  source: DatabaseBackupDescriptor,
  backupDirectory: string,
  suffix: string,
  corruptDatabase: boolean
): Promise<void> {
  const createdAt = suffix === 'checksum' ? 9_001 : 9_002
  const path = join(backupDirectory, `matou-${createdAt}-clean-exit-v0.sqlite`)
  if (corruptDatabase) await writeFile(path, 'not sqlite')
  else await copyFile(source.path, path)
  const bytes = await readFile(path)
  const descriptor: DatabaseBackupDescriptor = {
    ...source,
    id: basename(path, '.sqlite'),
    path,
    createdAt,
    size: bytes.byteLength,
    sha256: corruptDatabase
      ? createHash('sha256').update(bytes).digest('hex')
      : '0'.repeat(64)
  }
  await writeFile(path.replace(/\.sqlite$/, '.json'), JSON.stringify(descriptor))
}

function createStandaloneDatabase(path: string, value: string): void {
  const database = new DatabaseSync(path)
  database.exec('CREATE TABLE original_rows (value TEXT NOT NULL) STRICT;')
  database.prepare('INSERT INTO original_rows (value) VALUES (?)').run(value)
  database.close()
}
