import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { createE2eMigrationInterruptionObserver } from './e2e-migration-interruption-observer'

describe('createE2eMigrationInterruptionObserver', () => {
  it('is absent outside MATOU_E2E even when a control file is supplied', () => {
    expect(createE2eMigrationInterruptionObserver({
      MATOU_E2E: '0',
      MATOU_E2E_MIGRATION_CONTROL: '/tmp/migration-control.json'
    })).toBeUndefined()
  })

  it('records only the configured durable stage without pausing when hold is false', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-migration-observer-'))
    const controlPath = join(root, 'control.json')
    const reachedPath = join(root, 'reached.json')
    await writeFile(controlPath, JSON.stringify({
      stage: 'migration-committed',
      reachedPath,
      hold: false
    }))
    const observer = createE2eMigrationInterruptionObserver({
      MATOU_E2E: '1',
      MATOU_E2E_MIGRATION_CONTROL: controlPath
    })!

    observer.onPreMigrationBackupReady?.({
      id: 'backup', path: '/tmp/backup', createdAt: 1, reason: 'pre-migration',
      schemaVersion: 26, size: 1, sha256: 'a'.repeat(64)
    })
    await expect(readFile(reachedPath)).rejects.toMatchObject({ code: 'ENOENT' })
    observer.onMigrationCommitted?.({
      version: 27, name: 'durable-fork-operations', sql: 'SELECT 1'
    })

    expect(JSON.parse(await readFile(reachedPath, 'utf8'))).toMatchObject({
      stage: 'migration-committed',
      migrationVersion: 27,
      pid: process.pid
    })
  })
})
