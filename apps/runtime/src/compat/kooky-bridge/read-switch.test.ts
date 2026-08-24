import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { RuntimeDatabase } from '../../storage/database'
import { DomainTransactionManager } from '../../storage/domain-transaction'
import { MigrationRunner } from '../../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../../storage/migrations'
import { KookyImporter, legacyIdFor } from './kooky-importer'
import { LegacyCompatibilityBackupWriter, ReadAuthorityController } from './read-switch'

let root: string
let source: string
let database: RuntimeDatabase
let authority: ReadAuthorityController

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-read-switch-'))
  source = join(root, 'legacy')
  await mkdir(join(source, 'journals'), { recursive: true })
  await writeFile(join(source, 'snapshot.json'), JSON.stringify(snapshot()))
  await writeFile(join(source, 'journals', 'metadata.ndjson'), '')
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const tx = new DomainTransactionManager(database)
  await new KookyImporter(root, database, tx).importSource(source)
  authority = new ReadAuthorityController(database)
})

afterEach(() => database.close())

describe('ReadAuthorityController', () => {
  it('defaults to legacy, switches reads to an equal SQLite projection, and supports rollback', async () => {
    expect(authority.getReadAuthority()).toBe('legacy')
    const legacy = await authority.readProjection(source)
    authority.setReadAuthority('sqlite', 2)
    const sqlite = await authority.readProjection(source)

    expect(sqlite).toEqual(legacy)
    authority.setReadAuthority('legacy', 3)
    expect(await authority.readProjection(source)).toEqual(legacy)
  })

  it('commits SQLite first, then writes a compatibility backup, without rolling back on backup failure', async () => {
    const backup = vi.fn(async () => undefined)
    const taskId = legacyIdFor('task-workbench', 'workbench-1')
    const success = await authority.executeSqliteFirst(
      'rename-1',
      () => { database.run('UPDATE tasks SET title = ? WHERE id = ?', 'Renamed', taskId); return 'ok' },
      async () => {
        expect(database.get<{ title: string }>('SELECT title FROM tasks WHERE id = ?', taskId)?.title).toBe('Renamed')
        await backup()
      }
    )
    expect(success).toEqual({ result: 'ok', backupWritten: true })

    const failedBackup = await authority.executeSqliteFirst(
      'rename-2',
      () => { database.run('UPDATE tasks SET title = ? WHERE id = ?', 'Still committed', taskId); return 'committed' },
      async () => { throw new Error('disk full') }
    )
    expect(failedBackup).toEqual({ result: 'committed', backupWritten: false, backupError: 'disk full' })
    expect(database.get<{ title: string }>('SELECT title FROM tasks WHERE id = ?', taskId)?.title).toBe('Still committed')
  })

  it('writes Runtime-owned Kooky compatibility backups and tracks migration health telemetry', async () => {
    const backupRoot = join(root, 'compat-backup')
    const writer = new LegacyCompatibilityBackupWriter(database, backupRoot)
    const path = await writer.write(10)
    const persisted = JSON.parse(await readFile(path, 'utf8')) as { version: number; projects: { list: unknown[] }; panels: Record<string, unknown> }

    expect(persisted.version).toBe(1)
    expect(persisted.projects.list).toHaveLength(1)
    expect(Object.keys(persisted.panels).sort()).toEqual(['panel-lead', 'panel-member'])

    authority.recordHealth('restore.success', 1, 11)
    authority.recordHealth('provider-resume.success', 1, 11)
    authority.recordHealth('relation.correct', 1, 11)
    authority.recordHealth('projection.equal', 1, 11)
    expect(authority.telemetry()).toMatchObject({
      'restore.success': 1, 'provider-resume.success': 1,
      'relation.correct': 1, 'projection.equal': 1
    })
  })

  it('restores independent provider identity and team relations from SQLite after read cutover', async () => {
    authority.setReadAuthority('sqlite', 2)
    const projection = await authority.readProjection(source)

    expect(projection.sessions.map(({ providerSessionId }) => providerSessionId).sort()).toEqual(['provider-lead', 'provider-member'])
    expect(projection.relations).toHaveLength(1)
    expect(projection.relations[0]).toMatchObject({ kind: 'team-member-of' })
  })
})

function snapshot() {
  return {
    version: 1, recoveryOffsets: { metadataJournalBytes: 0 },
    projects: { list: [{ id: 'project-1', name: 'Project', path: '/tmp/project' }], activeProjectId: 'project-1' },
    workbenches: { 'workbench-1': { id: 'workbench-1', projectId: 'project-1', name: 'Task', tabIds: ['tab-1'] } },
    tabs: { 'tab-1': { id: 'tab-1', workbenchId: 'workbench-1', name: 'Main', layoutRoot: { type: 'split', id: 'split', children: [
      { type: 'leaf', id: 'leaf-1', panelId: 'panel-lead' }, { type: 'leaf', id: 'leaf-2', panelId: 'panel-member' }
    ] } } },
    panels: {
      'panel-lead': { id: 'panel-lead', projectId: 'project-1', workbenchId: 'workbench-1', tabId: 'tab-1', terminalId: 'term-lead', mode: 'claude-code', title: 'Lead', cwd: '/tmp/project', claudeSessionId: 'provider-lead', teamId: 'team', teamRole: 'leader', teamLeadSessionId: 'provider-lead' },
      'panel-member': { id: 'panel-member', projectId: 'project-1', workbenchId: 'workbench-1', tabId: 'tab-1', terminalId: 'term-member', mode: 'claude-code', title: 'Member', cwd: '/tmp/project/member', claudeSessionId: 'provider-member', teamId: 'team', teamRole: 'teammate', teamLeadSessionId: 'provider-lead' }
    }
  }
}
