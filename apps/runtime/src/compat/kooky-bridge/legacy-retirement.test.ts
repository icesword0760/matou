import { mkdtemp, readFile, readdir } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../../storage/database'
import { MigrationRunner } from '../../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../../storage/migrations'
import { LegacyRetirementGuard } from './legacy-retirement'

let database: RuntimeDatabase

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-retirement-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
})

afterEach(() => database.close())

describe('LegacyRetirementGuard', () => {
  it('retires legacy write/read authority while retaining a time-bounded read-only import window', () => {
    const guard = new LegacyRetirementGuard(database, { backupWindowMs: 1_000 })
    expect(guard.phase()).toBe('shadow')
    guard.retire(10)

    expect(guard.phase()).toBe('retired')
    expect(guard.readAuthority()).toBe('sqlite')
    expect(() => guard.assertShadowWriteAllowed()).toThrow('retired')
    expect(guard.canReadLegacyBackup(1_009)).toBe(true)
    expect(guard.canReadLegacyBackup(1_011)).toBe(false)
  })
})

describe('static authority boundaries', () => {
  it('keeps legacy snapshot/metadata authority confined to the compatibility package', async () => {
    const runtimeRoot = join(process.cwd(), 'src')
    const offenders: string[] = []
    for (const path of await walk(runtimeRoot)) {
      if (path.includes('/compat/kooky-bridge/') || path.endsWith('.test.ts') || path.endsWith('/storage/migrations.ts')) continue
      const contents = await readFile(path, 'utf8')
      if (/snapshot\.json|metadata\.ndjson|legacy_entity_mappings|legacy_import_runs/.test(contents)) offenders.push(path)
    }
    expect(offenders).toEqual([])
  })

  it('forbids Renderer-owned authoritative snapshot export', async () => {
    const rendererRoot = join(process.cwd(), '..', 'desktop', 'src', 'renderer')
    const offenders: string[] = []
    for (const path of await walk(rendererRoot)) {
      if (path.endsWith('.test.ts') || path.endsWith('.test.tsx')) continue
      const contents = await readFile(path, 'utf8')
      if (/export(?:Authoritative)?Snapshot|saveSnapshot|snapshot\.json/i.test(contents)) offenders.push(path)
    }
    expect(offenders).toEqual([])
  })
})

async function walk(root: string): Promise<string[]> {
  const output: string[] = []
  async function visit(directory: string): Promise<void> {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) await visit(path)
      else if (entry.isFile() && /\.(?:ts|tsx|js)$/.test(entry.name)) output.push(path)
    }
  }
  await visit(root)
  return output
}
