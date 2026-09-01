import { spawn } from 'node:child_process'
import {
  chmod,
  mkdir,
  mkdtemp,
  open,
  readFile,
  readdir,
  rm,
  stat,
  writeFile
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { DatabaseBackupService } from './database-backup-service'
import { RuntimeDatabase } from './database'
import { MigrationRunner } from './migration-runner'
import { FOUNDATION_MIGRATIONS } from './migrations'
import { openRecoverableRuntimeDatabase } from './runtime-database-bootstrap'

const { DatabaseSync } = process.getBuiltinModule(
  'node:sqlite'
) as typeof import('node:sqlite')

describe('openRecoverableRuntimeDatabase', () => {
  it('publishes database lifecycle ownership before pre-migration work starts', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-bootstrap-ownership-'))
    const events: string[] = []

    const opening = openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS, {
      onDatabaseOpened(database, effectiveDataRoot, backups) {
        expect(database.path).toBe(join(root, 'matou.sqlite'))
        expect(effectiveDataRoot).toBe(root)
        expect(backups).toBeInstanceOf(DatabaseBackupService)
        events.push('owned')
      },
      onDatabaseClosed() { events.push('released') }
    })

    expect(events).toEqual(['owned'])
    const result = await opening
    expect(result.kind).toBe('writable')
    if (result.kind !== 'writable') throw new Error('expected writable database')
    try {
      expect(events).toEqual(['owned'])
    } finally {
      result.database.close()
    }
  })

  it('creates a retained pre-migration snapshot when initializing a durable database', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-initial-backup-'))

    const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    expect(result.kind).toBe('writable')
    if (result.kind !== 'writable') throw new Error('expected writable database')
    try {
      const backups = await new DatabaseBackupService(root).listValid()
      expect(backups).toHaveLength(1)
      expect(backups[0]).toMatchObject({ reason: 'pre-migration', schemaVersion: 0 })
      const snapshot = new DatabaseSync(backups[0]!.path, { readOnly: true })
      try {
        expect(snapshot.prepare(
          "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'"
        ).get()).toBeUndefined()
      } finally {
        snapshot.close()
      }
    } finally {
      result.database.close()
    }
  })

  it('quarantines physical corruption without creating an empty replacement and lists valid backups', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-corrupt-database-'))
    const databasePath = join(root, 'matou.sqlite')
    const initial = RuntimeDatabase.open(databasePath)
    await new MigrationRunner(initial, FOUNDATION_MIGRATIONS).migrate()
    initial.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'workspace-backup', 'Backup Workspace', root, 1, 1
    )
    const backup = await new DatabaseBackupService(root).create(initial, 'clean-exit')
    initial.close()
    await writeFile(databasePath, 'this is not a sqlite database')

    const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)

    expect(result.kind).toBe('recovery-required')
    if (result.kind !== 'recovery-required') throw new Error('expected database recovery')
    expect(result).toMatchObject({
      reason: 'physical-corruption',
      durableDatabasePath: databasePath,
      backups: [expect.objectContaining({ id: backup.id, path: backup.path })]
    })
    expect(await readFile(result.quarantinedPath, 'utf8')).toBe('this is not a sqlite database')
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })

    const snapshot = new DatabaseSync(result.backups[0]!.path, { readOnly: true })
    try {
      expect(snapshot.prepare(
        'SELECT name FROM workspaces WHERE id = ?'
      ).get('workspace-backup')).toEqual({ name: 'Backup Workspace' })
    } finally {
      snapshot.close()
    }

    const repeated = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    expect(repeated).toMatchObject({
      kind: 'recovery-required',
      markerPath: join(root, 'matou.sqlite.recovery.json'),
      quarantinedPath: result.quarantinedPath
    })
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
    expect(JSON.parse(await readFile(
      join(root, 'matou.sqlite.recovery.json'), 'utf8'
    ))).toMatchObject({ quarantinedPath: result.quarantinedPath })
  })

  it('publishes a durable marker under the owner fence before moving the corrupt bundle', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-recovery-marker-barrier-'))
    const databasePath = join(root, 'matou.sqlite')
    await writeFile(databasePath, 'corrupt database behind a barrier')
    const markerPublished = deferred<void>()
    const releaseMove = deferred<void>()

    const opening = openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS, {
      async onRecoveryMarkerPublished(marker) {
        expect(marker.markerPath).toBe(join(root, 'matou.sqlite.recovery.json'))
        expect(await readFile(marker.markerPath, 'utf8')).toContain(marker.quarantinedPath)
        expect(await readFile(`${databasePath}.owner/owner.json`, 'utf8')).toContain(
          'runtimeGeneration'
        )
        markerPublished.resolve()
        await releaseMove.promise
      }
    })

    await expect(Promise.race([
      markerPublished.promise,
      rejectingDelay(500, 'recovery marker was not published before quarantine')
    ])).resolves.toBeUndefined()
    const competing = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    expect(competing).toMatchObject({
      kind: 'recovery-required',
      markerPath: join(root, 'matou.sqlite.recovery.json')
    })
    releaseMove.resolve()
    const first = await opening
    expect(first).toMatchObject({
      kind: 'recovery-required',
      quarantinedPath: competing.kind === 'recovery-required'
        ? competing.quarantinedPath
        : 'unexpected'
    })
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
    await expect(readFile(`${databasePath}.owner/owner.json`)).rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('keeps recovery-required stable when valid backup enumeration fails', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-recovery-backup-error-'))
    const databasePath = join(root, 'matou.sqlite')
    const backupDirectory = join(root, 'backups')
    await writeFile(databasePath, 'corrupt database with unreadable backups')
    await mkdir(backupDirectory)
    await chmod(backupDirectory, 0o000)
    try {
      const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
      expect(result).toMatchObject({
        kind: 'recovery-required',
        backups: [],
        backupListError: {
          code: 'BACKUP_LIST_FAILED',
          retryable: true
        },
        markerPath: join(root, 'matou.sqlite.recovery.json')
      })
      await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
      expect(await readFile(join(root, 'matou.sqlite.recovery.json'), 'utf8')).toContain(
        result.kind === 'recovery-required' ? result.quarantinedPath : 'unexpected'
      )
    } finally {
      await chmod(backupDirectory, 0o700)
    }

    const repeated = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    expect(repeated.kind).toBe('recovery-required')
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('makes concurrent corrupt bootstraps converge on one recovery marker', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-concurrent-corruption-'))
    const databasePath = join(root, 'matou.sqlite')
    await writeFile(databasePath, 'concurrently discovered corruption')

    const [first, second] = await Promise.all([
      openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS),
      openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    ])

    expect(first.kind).toBe('recovery-required')
    expect(second.kind).toBe('recovery-required')
    if (first.kind !== 'recovery-required' || second.kind !== 'recovery-required') {
      throw new Error('expected both bootstraps to require recovery')
    }
    expect(first.markerPath).toBe(second.markerPath)
    expect(first.quarantinedPath).toBe(second.quarantinedPath)
    expect((await readdir(root)).filter((name) => name.includes('.corrupt-'))).toHaveLength(1)
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('detects corruption in a non-header SQLite page before migration and enters recovery', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-midpage-corruption-'))
    const databasePath = join(root, 'matou.sqlite')
    const initial = RuntimeDatabase.open(databasePath)
    initial.exec('CREATE TABLE payloads (id INTEGER PRIMARY KEY, payload TEXT NOT NULL)')
    initial.transaction((transaction) => {
      for (let id = 1; id <= 600; id += 1) {
        transaction.run(
          'INSERT INTO payloads (id, payload) VALUES (?, ?)', id, `${id}:${'x'.repeat(3000)}`
        )
      }
    })
    initial.close()

    const intactHeader = (await readFile(databasePath)).subarray(0, 100)
    const inspection = new DatabaseSync(databasePath, { readOnly: true })
    const pageSizeRow = inspection.prepare('PRAGMA page_size').get() as Record<string, number>
    const rootPageRow = inspection.prepare(
      "SELECT rootpage FROM sqlite_master WHERE type = 'table' AND name = 'payloads'"
    ).get() as { rootpage: number }
    inspection.close()
    const pageSize = Number(Object.values(pageSizeRow)[0])
    expect(rootPageRow.rootpage).toBeGreaterThan(1)

    const handle = await open(databasePath, 'r+')
    try {
      await handle.write(Buffer.alloc(16, 0xff), 0, 16, (rootPageRow.rootpage - 1) * pageSize)
    } finally {
      await handle.close()
    }
    expect((await readFile(databasePath)).subarray(0, 100)).toEqual(intactHeader)

    const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)

    expect(result.kind).toBe('recovery-required')
    if (result.kind !== 'recovery-required') throw new Error('expected database recovery')
    expect(result.reason).toBe('physical-corruption')
    expect(result.durableDatabasePath).toBe(databasePath)
    expect(result.quarantinedPath).not.toBe(databasePath)
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
    expect((await readFile(result.quarantinedPath)).subarray(0, 100)).toEqual(intactHeader)
  })

  it('does not inspect or quarantine a database owned by a live Runtime', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-owned-corruption-'))
    const databasePath = join(root, 'matou.sqlite')
    const corruptBytes = Buffer.from('owned corrupt database')
    await writeFile(databasePath, corruptBytes)
    await mkdir(`${databasePath}.owner`)
    await writeFile(`${databasePath}.owner/owner.json`, JSON.stringify({
      pid: process.pid,
      runtimeGeneration: 'live-owner'
    }))

    await expect(
      openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    ).rejects.toThrow('database is already owned by a live Runtime')
    expect(await readFile(databasePath)).toEqual(corruptBytes)
    expect((await readdir(root)).filter((name) => name.includes('.corrupt-'))).toEqual([])
  })

  it('checks a live owner before permission and corruption handling', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-owned-readonly-corruption-'))
    const databasePath = join(root, 'matou.sqlite')
    const corruptBytes = Buffer.from('owned readonly corrupt database')
    await writeFile(databasePath, corruptBytes)
    await mkdir(`${databasePath}.owner`)
    await writeFile(`${databasePath}.owner/owner.json`, JSON.stringify({
      pid: process.pid,
      runtimeGeneration: 'live-readonly-owner'
    }))
    await chmod(databasePath, 0o444)
    await chmod(root, 0o555)

    try {
      await expect(
        openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
      ).rejects.toThrow('database is already owned by a live Runtime')
      expect(await readFile(databasePath)).toEqual(corruptBytes)
      expect((await readdir(root)).filter((name) => name.includes('.corrupt-'))).toEqual([])
    } finally {
      await chmod(root, 0o755)
      await chmod(databasePath, 0o644)
    }
  })

  it('opens a committed WAL bundle with an existing valid SHM without hiding recent rows', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-readonly-wal-shm-'))
    const databasePath = await createCrashedWalBundle(root)
    await chmod(databasePath, 0o444)
    await chmod(`${databasePath}-wal`, 0o444)
    await chmod(`${databasePath}-shm`, 0o444)
    await chmod(root, 0o555)

    try {
      const result = await openRecoverableRuntimeDatabase(root, [])
      expect(result.kind).toBe('read-only')
      if (result.kind !== 'read-only') throw new Error('expected read-only WAL database')
      try {
        expect(result.database.all<{ value: string }>(
          'SELECT value FROM wal_values ORDER BY rowid'
        )).toEqual([{ value: 'base' }, { value: 'committed-in-wal' }])
      } finally {
        result.database.close()
      }
    } finally {
      await chmod(root, 0o755)
      await chmod(databasePath, 0o644)
      await chmod(`${databasePath}-wal`, 0o644)
      await chmod(`${databasePath}-shm`, 0o644)
    }
  })

  it.each([
    ['missing', async (shmPath: string) => rm(shmPath)],
    ['corrupt', async (shmPath: string) => writeFile(shmPath, Buffer.alloc(32_768))],
    ['unreadable', async (shmPath: string) => chmod(shmPath, 0o000)]
  ] as const)(
    'preserves a committed WAL bundle and requires recovery when SHM is %s',
    async (_condition, breakShm) => {
      const root = await mkdtemp(join(tmpdir(), 'matou-invalid-wal-shm-'))
      const databasePath = await createCrashedWalBundle(root)
      const walBytes = await readFile(`${databasePath}-wal`)
      const mainBytes = await readFile(databasePath)
      await breakShm(`${databasePath}-shm`)

      const result = await openRecoverableRuntimeDatabase(root, [])

      expect(result).toMatchObject({
        kind: 'recovery-required',
        reason: 'wal-recovery-required',
        markerPath: join(root, 'matou.sqlite.recovery.json')
      })
      if (result.kind !== 'recovery-required') throw new Error('expected WAL recovery')
      expect(await readFile(result.quarantinedPath)).toEqual(mainBytes)
      expect(await readFile(`${result.quarantinedPath}-wal`)).toEqual(walBytes)
      await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
      if (_condition === 'unreadable') {
        await chmod(`${result.quarantinedPath}-shm`, 0o600)
      }
    }
  )

  it('opens the durable database in place as read-only when its filesystem rejects writes', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-readonly-database-'))
    const databasePath = join(root, 'matou.sqlite')
    const initial = RuntimeDatabase.open(databasePath)
    await new MigrationRunner(initial, FOUNDATION_MIGRATIONS).migrate()
    initial.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'workspace-readonly', 'Readonly Workspace', root, 1, 1
    )
    initial.close()
    const originalBytes = await readFile(databasePath)
    await chmod(databasePath, 0o444)
    await chmod(root, 0o555)

    try {
      const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
      expect(result.kind).toBe('read-only')
      if (result.kind !== 'read-only') throw new Error('expected read-only database')
      try {
        expect(result).toMatchObject({ dataRoot: root, reason: 'filesystem-read-only' })
        expect(result.database.path).toBe(databasePath)
        expect(result.database.get<{ name: string }>(
          'SELECT name FROM workspaces WHERE id = ?', 'workspace-readonly'
        )).toEqual({ name: 'Readonly Workspace' })
        assertReadOnlyMutations(result.database, 'workspace-readonly')
      } finally {
        result.database.close()
      }
      expect(await readFile(databasePath)).toEqual(originalBytes)
    } finally {
      await chmod(root, 0o755)
      await chmod(databasePath, 0o644)
    }
  })

  it('opens a newer schema in place as read-only and preserves version 999', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-newer-database-'))
    const databasePath = join(root, 'matou.sqlite')
    const newer = RuntimeDatabase.open(databasePath)
    await new MigrationRunner(newer, FOUNDATION_MIGRATIONS).migrate()
    newer.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'workspace-newer', 'Newer Workspace', root, 1, 1
    )
    newer.run(
      'INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?, ?, ?, ?)',
      999, 'future-version', 'future-checksum', 2
    )
    newer.close()
    const originalBytes = await readFile(databasePath)

    const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    expect(result.kind).toBe('read-only')
    if (result.kind !== 'read-only') throw new Error('expected read-only database')
    try {
      expect(result).toMatchObject({ dataRoot: root, reason: 'newer-schema' })
      expect(result.database.path).toBe(databasePath)
      expect(result.database.get<{ name: string }>(
        'SELECT name FROM workspaces WHERE id = ?', 'workspace-newer'
      )).toEqual({ name: 'Newer Workspace' })
      expect(result.database.get<{ version: number }>(
        'SELECT MAX(version) AS version FROM schema_migrations'
      )).toEqual({ version: 999 })
      assertReadOnlyMutations(result.database, 'workspace-newer')
    } finally {
      result.database.close()
    }
    expect(await readFile(databasePath)).toEqual(originalBytes)
  })
})

function assertReadOnlyMutations(database: RuntimeDatabase, workspaceId: string): void {
  expect(() => database.run(
    'UPDATE workspaces SET name = ? WHERE id = ?', 'Changed', workspaceId
  )).toThrow('STORAGE_READ_ONLY')
  expect(() => database.exec(
    `UPDATE workspaces SET name = 'Changed' WHERE id = '${workspaceId}'`
  )).toThrow('STORAGE_READ_ONLY')

  let transactionCalled = false
  expect(() => database.transaction(() => {
    transactionCalled = true
  })).toThrow('STORAGE_READ_ONLY')
  expect(transactionCalled).toBe(false)

  let queuedWriteCalled = false
  expect(() => database.enqueueWrite(() => {
    queuedWriteCalled = true
  })).toThrow('STORAGE_READ_ONLY')
  expect(queuedWriteCalled).toBe(false)
}

async function createCrashedWalBundle(root: string): Promise<string> {
  const databasePath = join(root, 'matou.sqlite')
  const readyPath = join(root, 'writer-ready')
  const script = `
    import { writeFileSync } from 'node:fs'
    const { DatabaseSync } = process.getBuiltinModule('node:sqlite')
    const database = new DatabaseSync(${JSON.stringify(databasePath)})
    database.exec(\`
      PRAGMA journal_mode = WAL;
      PRAGMA wal_autocheckpoint = 0;
      CREATE TABLE wal_values (value TEXT NOT NULL);
      INSERT INTO wal_values VALUES ('base');
      PRAGMA wal_checkpoint(TRUNCATE);
      INSERT INTO wal_values VALUES ('committed-in-wal');
    \`)
    writeFileSync(${JSON.stringify(readyPath)}, 'ready')
    setInterval(() => {}, 1000)
  `
  const child = spawn(process.execPath, ['--experimental-sqlite', '--input-type=module', '-e', script], {
    stdio: ['ignore', 'ignore', 'pipe']
  })
  let stderr = ''
  child.stderr?.on('data', (chunk) => { stderr += String(chunk) })
  await waitUntilAsync(async () => (await stat(readyPath).catch(() => undefined))?.isFile() === true)
  const exited = new Promise<void>((resolve, reject) => {
    child.once('exit', () => resolve())
    child.once('error', reject)
  })
  child.kill('SIGKILL')
  await exited
  if (stderr && !/ExperimentalWarning/.test(stderr)) {
    throw new Error(`WAL fixture failed: ${stderr}`)
  }
  expect((await stat(`${databasePath}-wal`)).size).toBeGreaterThan(32)
  expect((await stat(`${databasePath}-shm`)).size).toBeGreaterThanOrEqual(32_768)
  return databasePath
}

function deferred<T>(): { promise: Promise<T>; resolve(value?: T): void } {
  let resolve!: (value: T | PromiseLike<T>) => void
  const promise = new Promise<T>((fulfill) => { resolve = fulfill })
  return { promise, resolve: (value?: T) => resolve(value as T) }
}

function rejectingDelay(milliseconds: number, message: string): Promise<never> {
  return new Promise((_, reject) => setTimeout(() => reject(new Error(message)), milliseconds))
}

async function waitUntilAsync(
  predicate: () => Promise<boolean>,
  timeoutMs = 3_000
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await predicate())) {
    if (Date.now() >= deadline) throw new Error('condition did not become true before timeout')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}
