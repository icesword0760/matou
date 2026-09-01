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
  it.each([
    ['empty owner', async (ownerPath: string) => writeFile(ownerPath, '')],
    ['truncated owner', async (ownerPath: string) => writeFile(ownerPath, '{"pid":')],
    ['wrong-type owner', async (ownerPath: string) => mkdir(ownerPath)]
  ] as const)(
    'preserves the canonical database and durably requires recovery for %s',
    async (_scenario, breakOwner) => {
      const { root, databasePath, originalBytes } = await createOwnedFaultFixture()
      await breakOwner(`${databasePath}.owner`)

      const first = await openRecoverableRuntimeDatabase(root, [])
      const second = await openRecoverableRuntimeDatabase(root, [])

      expect(first).toMatchObject({
        kind: 'recovery-required',
        reason: 'ownership-recovery-required',
        ownershipIssue: 'owner-record-malformed',
        durableDatabasePath: databasePath,
        quarantinedPath: databasePath,
        markerPath: `${databasePath}.recovery.json`
      })
      expect(second).toMatchObject(first)
      expect(await readFile(databasePath)).toEqual(originalBytes)
      expect((await readdir(root)).filter((name) => name.includes('.corrupt-'))).toEqual([])
    }
  )

  it.each(['corrupt-bytes', 'directory'] as const)(
    'preserves the canonical database when the takeover sidecar is %s',
    async (scenario) => {
      const { root, databasePath, originalBytes } = await createOwnedFaultFixture()
      await writeFile(`${databasePath}.owner`, JSON.stringify({
        pid: 2_147_483_647,
        runtimeGeneration: 'stale-before-sidecar-fault'
      }))
      const sidecarPath = `${databasePath}.owner.takeover.sqlite`
      if (scenario === 'directory') await mkdir(sidecarPath)
      else await writeFile(sidecarPath, Buffer.from('not a sqlite database'))

      const first = await openRecoverableRuntimeDatabase(root, [])
      const second = await openRecoverableRuntimeDatabase(root, [])

      expect(first).toMatchObject({
        kind: 'recovery-required',
        reason: 'ownership-recovery-required',
        ownershipIssue: 'takeover-sidecar-unusable',
        durableDatabasePath: databasePath,
        quarantinedPath: databasePath
      })
      expect(second).toMatchObject(first)
      expect(await readFile(databasePath)).toEqual(originalBytes)
      expect((await readdir(root)).filter((name) => name.includes('.corrupt-'))).toEqual([])
    }
  )

  it('makes concurrent malformed-owner bootstraps converge on one durable marker', async () => {
    const { root, databasePath, originalBytes } = await createOwnedFaultFixture()
    await writeFile(`${databasePath}.owner`, Buffer.from([0xff, 0x00, 0x7b, 0x01]))

    const [first, second] = await Promise.all([
      openRecoverableRuntimeDatabase(root, []),
      openRecoverableRuntimeDatabase(root, [])
    ])

    expect(first).toMatchObject({
      kind: 'recovery-required',
      reason: 'ownership-recovery-required',
      ownershipIssue: 'owner-record-malformed',
      quarantinedPath: databasePath
    })
    expect(second).toMatchObject(first)
    expect(await readFile(databasePath)).toEqual(originalBytes)
    expect((await readdir(root)).filter((name) => name.includes('.corrupt-'))).toEqual([])
  })

  it.each([0o000, 0o200])(
    'repairs same-user owner mode %s without weakening the owner fence',
    async (mode) => {
      const { root, databasePath } = await createOwnedFaultFixture()
      await writeFile(`${databasePath}.owner`, JSON.stringify({
        pid: 2_147_483_647,
        runtimeGeneration: 'stale-owner-with-repairable-mode'
      }))
      await chmod(`${databasePath}.owner`, mode)

      const result = await openRecoverableRuntimeDatabase(root, [])
      expect(result.kind).toBe('writable')
      if (result.kind !== 'writable') throw new Error('expected repaired writable database')
      try {
        expect((await stat(`${databasePath}.owner`)).mode & 0o777).toBe(0o600)
        expect(result.database.get<{ value: string }>(
          'SELECT value FROM fault_sentinel'
        )).toEqual({ value: 'preserve-me' })
      } finally {
        result.database.close()
      }
    }
  )

  it.each([0o000, 0o200])(
    'repairs same-user takeover sidecar mode %s and completes stale takeover',
    async (mode) => {
      const { root, databasePath } = await createOwnedFaultFixture()
      await writeFile(`${databasePath}.owner`, JSON.stringify({
        pid: 2_147_483_647,
        runtimeGeneration: 'stale-owner-before-sidecar-mode-repair'
      }))
      const sidecarPath = `${databasePath}.owner.takeover.sqlite`
      new DatabaseSync(sidecarPath).close()
      await chmod(sidecarPath, mode)

      const result = await openRecoverableRuntimeDatabase(root, [])
      expect(result.kind).toBe('writable')
      if (result.kind !== 'writable') throw new Error('expected repaired writable database')
      try {
        expect((await stat(sidecarPath)).mode & 0o777).toBe(0o600)
        expect(result.database.get<{ value: string }>(
          'SELECT value FROM fault_sentinel'
        )).toEqual({ value: 'preserve-me' })
      } finally {
        result.database.close()
      }
    }
  )

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
      recoveryId: expect.stringMatching(/^[A-Za-z0-9._-]+$/),
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
      recoveryId: result.recoveryId,
      markerPath: join(root, 'matou.sqlite.recovery.json'),
      quarantinedPath: result.quarantinedPath
    })
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
    expect(JSON.parse(await readFile(
      join(root, 'matou.sqlite.recovery.json'), 'utf8'
    ))).toMatchObject({ quarantinedPath: result.quarantinedPath })
  })

  it('durably upgrades a valid legacy v1 recovery marker without a recoveryId', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-legacy-recovery-marker-'))
    const databasePath = join(root, 'matou.sqlite')
    const markerPath = `${databasePath}.recovery.json`
    const legacy = {
      version: 1,
      reason: 'physical-corruption',
      durableDatabasePath: databasePath,
      quarantinedPath: `${databasePath}.corrupt-1`,
      markerPath,
      createdAt: 1
    }
    await writeFile(markerPath, JSON.stringify(legacy))

    const first = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    expect(first).toMatchObject({
      kind: 'recovery-required',
      recoveryId: expect.stringMatching(/^[A-Za-z0-9._-]+$/)
    })
    if (first.kind !== 'recovery-required') throw new Error('expected upgraded recovery marker')
    const upgraded = JSON.parse(await readFile(markerPath, 'utf8')) as Record<string, unknown>
    expect(upgraded).toMatchObject({
      ...legacy,
      state: 'required',
      recoveryId: first.recoveryId
    })

    const restarted = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    expect(restarted).toMatchObject({
      kind: 'recovery-required', recoveryId: first.recoveryId
    })
  })

  it('keeps malformed legacy recovery markers fail-closed during upgrade', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-malformed-legacy-recovery-marker-'))
    const databasePath = join(root, 'matou.sqlite')
    const markerPath = `${databasePath}.recovery.json`
    const malformed = JSON.stringify({
      version: 1,
      reason: 'physical-corruption',
      durableDatabasePath: join(root, 'different.sqlite'),
      quarantinedPath: `${databasePath}.corrupt-1`,
      markerPath,
      createdAt: 1
    })
    await writeFile(markerPath, malformed)

    await expect(openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS))
      .rejects.toThrow('database recovery marker is invalid')
    expect(await readFile(markerPath, 'utf8')).toBe(malformed)
  })

  it.each([
    ['required without recoveryId', { state: 'required' }],
    ['resolved without recoveryId', { state: 'resolved' }],
    ['unknown state', { state: 'unknown', recoveryId: 'existing-recovery-id' }],
    ['wrong state type', { state: 1, recoveryId: 'existing-recovery-id' }],
    ['wrong recoveryId type', { recoveryId: 1 }]
  ] as const)('keeps an impossible legacy half-shape fail-closed: %s', async (_name, fields) => {
    const root = await mkdtemp(join(tmpdir(), 'matou-impossible-legacy-marker-'))
    const databasePath = join(root, 'matou.sqlite')
    const markerPath = `${databasePath}.recovery.json`
    const bytes = JSON.stringify({
      version: 1,
      reason: 'physical-corruption',
      durableDatabasePath: databasePath,
      quarantinedPath: `${databasePath}.corrupt-1`,
      markerPath,
      createdAt: 1,
      ...fields
    })
    await writeFile(markerPath, bytes)

    await expect(openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS))
      .rejects.toThrow('database recovery marker is invalid')
    expect(await readFile(markerPath, 'utf8')).toBe(bytes)
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
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
        expect(await readFile(`${databasePath}.owner`, 'utf8')).toContain(
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
    await expect(readFile(`${databasePath}.owner`)).rejects.toMatchObject({ code: 'ENOENT' })
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

  it('retains the owner fence when a read-only handle discovers non-header corruption', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-readonly-midpage-corruption-'))
    const databasePath = join(root, 'matou.sqlite')
    const initial = RuntimeDatabase.open(databasePath)
    initial.exec('CREATE TABLE payloads (id INTEGER PRIMARY KEY, payload TEXT NOT NULL)')
    initial.transaction((transaction) => {
      for (let id = 1; id <= 20; id += 1) {
        transaction.run(
          'INSERT INTO payloads (id, payload) VALUES (?, ?)', id, `${id}:${'x'.repeat(3000)}`
        )
      }
    })
    initial.close()

    const inspection = new DatabaseSync(databasePath, { readOnly: true })
    const pageSizeRow = inspection.prepare('PRAGMA page_size').get() as Record<string, number>
    const rootPageRow = inspection.prepare(
      "SELECT rootpage FROM sqlite_master WHERE type = 'table' AND name = 'payloads'"
    ).get() as { rootpage: number }
    inspection.close()
    const pageSize = Number(Object.values(pageSizeRow)[0])
    const handle = await open(databasePath, 'r+')
    try {
      await handle.write(Buffer.alloc(16, 0xff), 0, 16, (rootPageRow.rootpage - 1) * pageSize)
    } finally {
      await handle.close()
    }
    const corruptBytes = await readFile(databasePath)
    await chmod(databasePath, 0o444)

    const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)

    expect(result).toMatchObject({
      kind: 'recovery-required',
      reason: 'physical-corruption',
      markerPath: join(root, 'matou.sqlite.recovery.json')
    })
    if (result.kind !== 'recovery-required') throw new Error('expected database recovery')
    expect(await readFile(result.quarantinedPath)).toEqual(corruptBytes)
    await expect(readFile(databasePath)).rejects.toMatchObject({ code: 'ENOENT' })
    await expect(readFile(`${databasePath}.owner`)).rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('does not inspect or quarantine a database owned by a live Runtime', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-owned-corruption-'))
    const databasePath = join(root, 'matou.sqlite')
    const corruptBytes = Buffer.from('owned corrupt database')
    await writeFile(databasePath, corruptBytes)
    await writeFile(`${databasePath}.owner`, JSON.stringify({
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
    await writeFile(`${databasePath}.owner`, JSON.stringify({
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

async function createOwnedFaultFixture(): Promise<{
  root: string
  databasePath: string
  originalBytes: Buffer
}> {
  const root = await mkdtemp(join(tmpdir(), 'matou-ownership-fault-'))
  const databasePath = join(root, 'matou.sqlite')
  const database = RuntimeDatabase.open(databasePath)
  database.exec('CREATE TABLE fault_sentinel (value TEXT NOT NULL)')
  database.run('INSERT INTO fault_sentinel VALUES (?)', 'preserve-me')
  database.close()
  return { root, databasePath, originalBytes: await readFile(databasePath) }
}

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
