import { chmod, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { FOUNDATION_MIGRATIONS } from './migrations'
import { openRecoverableRuntimeDatabase } from './runtime-database-bootstrap'
import { RuntimeDatabase } from './database'
import { MigrationRunner } from './migration-runner'

describe('openRecoverableRuntimeDatabase', () => {
  it('quarantines a physically corrupt database and starts with a clean durable store', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-corrupt-database-'))
    const databasePath = join(root, 'matou.sqlite')
    await writeFile(databasePath, 'this is not a sqlite database')

    const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    try {
      expect(result.recoveredFromCorruption).toBe(true)
      expect(result.quarantinedPath).toBeDefined()
      expect(await readFile(result.quarantinedPath!, 'utf8')).toBe('this is not a sqlite database')
      expect(result.database.get<{ version: number }>(
        'SELECT MAX(version) AS version FROM schema_migrations'
      )).toEqual({ version: FOUNDATION_MIGRATIONS.at(-1)!.version })
    } finally {
      result.database.close()
    }
  })

  it('opens a writable ephemeral copy when durable storage is read-only', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-readonly-database-'))
    const databasePath = join(root, 'matou.sqlite')
    const initial = RuntimeDatabase.open(databasePath)
    await new MigrationRunner(initial, FOUNDATION_MIGRATIONS).migrate()
    initial.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'workspace-readonly', 'Readonly Workspace', root, 1, 1
    )
    initial.close()
    await chmod(databasePath, 0o444)
    await chmod(root, 0o555)

    const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    try {
      expect(result.ephemeral).toBe(true)
      expect(result.effectiveDataRoot).not.toBe(root)
      expect(result.database.get<{ name: string }>(
        'SELECT name FROM workspaces WHERE id = ?', 'workspace-readonly'
      )).toEqual({ name: 'Readonly Workspace' })
      expect(() => result.database.run(
        'UPDATE workspaces SET name = ? WHERE id = ?', 'Today Only', 'workspace-readonly'
      )).not.toThrow()
      expect(await readFile(databasePath)).toBeDefined()
    } finally {
      result.database.close()
      await chmod(root, 0o755)
      await chmod(databasePath, 0o644)
    }
  })

  it('uses a non-destructive compatibility copy for a newer database schema', async () => {
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

    const result = await openRecoverableRuntimeDatabase(root, FOUNDATION_MIGRATIONS)
    try {
      expect(result.ephemeral).toBe(true)
      expect(result.database.get<{ name: string }>(
        'SELECT name FROM workspaces WHERE id = ?', 'workspace-newer'
      )).toEqual({ name: 'Newer Workspace' })
      const original = RuntimeDatabase.open(databasePath)
      try {
        expect(original.get<{ version: number }>(
          'SELECT MAX(version) AS version FROM schema_migrations'
        )).toEqual({ version: 999 })
      } finally {
        original.close()
      }
    } finally {
      result.database.close()
    }
  })
})
