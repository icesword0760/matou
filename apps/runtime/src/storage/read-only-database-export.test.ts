import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { exportReadOnlyDatabaseBundle } from './read-only-database-export'

const roots: string[] = []
afterEach(async () => Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true }))))

describe('exportReadOnlyDatabaseBundle', () => {
  it('copies the immutable database bundle to an external writable destination', async () => {
    const sourceRoot = await mkdtemp(join(tmpdir(), 'matou-readonly-source-'))
    const destinationRoot = await mkdtemp(join(tmpdir(), 'matou-readonly-export-'))
    roots.push(sourceRoot, destinationRoot)
    const databasePath = join(sourceRoot, 'matou.sqlite')
    await writeFile(databasePath, 'database-bytes')
    await writeFile(`${databasePath}-wal`, 'wal-bytes')

    const exportedPath = await exportReadOnlyDatabaseBundle(databasePath, destinationRoot, 123)

    expect(exportedPath.startsWith(destinationRoot)).toBe(true)
    expect(await readFile(join(exportedPath, 'matou.sqlite'), 'utf8')).toBe('database-bytes')
    expect(await readFile(join(exportedPath, 'matou.sqlite-wal'), 'utf8')).toBe('wal-bytes')
    expect(JSON.parse(await readFile(join(exportedPath, 'manifest.json'), 'utf8'))).toMatchObject({
      mode: 'read-only', sourceDatabasePath: databasePath, exportedAt: 123,
      exportedFiles: ['matou.sqlite', 'matou.sqlite-wal']
    })
  })
})
