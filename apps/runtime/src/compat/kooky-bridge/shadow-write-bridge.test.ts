import { appendFile, mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { RuntimeDatabase } from '../../storage/database'
import { DomainTransactionManager } from '../../storage/domain-transaction'
import { MigrationRunner } from '../../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../../storage/migrations'
import { KookyImporter, legacyIdFor } from './kooky-importer'
import { ShadowWriteBridge } from './shadow-write-bridge'

let root: string
let source: string
let database: RuntimeDatabase
let bridge: ShadowWriteBridge

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-shadow-'))
  source = join(root, 'legacy')
  await mkdir(join(source, 'journals'), { recursive: true })
  await writeFile(join(source, 'snapshot.json'), JSON.stringify(snapshot()))
  await writeFile(join(source, 'journals', 'metadata.ndjson'), '')
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  bridge = new ShadowWriteBridge(database, transactions, new KookyImporter(root, database, transactions))
  await bridge.bootstrap('default', source)
})

afterEach(() => database.close())

describe('ShadowWriteBridge', () => {
  it('writes the legacy path first and never blocks it when the Matou shadow mutation fails', async () => {
    const legacyWrite = vi.fn(async () => undefined)
    const result = await bridge.mirrorMutation({
      schemaVersion: 1, commandId: 'bad-1', type: 'workbench-created', timestamp: 2,
      payload: { workbenchId: 'orphan', projectId: 'missing', name: 'Orphan' }
    }, legacyWrite)

    expect(legacyWrite).toHaveBeenCalledOnce()
    expect(result).toMatchObject({ legacyWritten: true, shadowApplied: false, repairQueued: true })
    expect(database.get<{ command_id: string }>('SELECT command_id FROM shadow_repair_queue WHERE command_id = ?', 'bad-1')?.command_id).toBe('bad-1')
  })

  it('maps narrow legacy mutations into authoritative SQLite without accepting Store snapshots', async () => {
    const result = await bridge.mirrorMutation({
      schemaVersion: 1, commandId: 'panel-cwd-1', type: 'panel-updated', timestamp: 2,
      payload: { panelId: 'panel-1', cwd: '/tmp/project/next', title: 'Next' }
    }, async () => undefined)

    expect(result.shadowApplied).toBe(true)
    expect(database.get<{ cwd: string }>('SELECT cwd FROM execution_contexts WHERE id = ?', legacyIdFor('context-panel', 'panel-1'))?.cwd).toBe('/tmp/project/next')
    expect(database.get<{ title: string }>('SELECT title FROM sessions WHERE id = ?', legacyIdFor('session-panel', 'panel-1'))?.title).toBe('Next')
    await expect(bridge.mirrorMutation({ schemaVersion: 1, commandId: 'snapshot', type: 'store-snapshot' as never, timestamp: 3, payload: {} }, async () => undefined)).rejects.toThrow('Unsupported')
  })

  it('tails only complete metadata records, records lag, and resumes from its durable byte cursor', async () => {
    const path = join(source, 'journals', 'metadata.ndjson')
    const complete = `${JSON.stringify({ type: 'panel-updated', ts: 3, payload: { panelId: 'panel-1', title: 'From tail' } })}\n`
    const partial = JSON.stringify({ type: 'panel-updated', ts: 4, payload: { panelId: 'panel-1', cwd: '/tmp/partial' } })
    await appendFile(path, complete + partial.slice(0, 20))

    const first = await bridge.tailMetadata('default', source)
    expect(first.applied).toBe(1)
    expect(first.pendingBytes).toBeGreaterThan(0)
    expect(database.get<{ title: string }>('SELECT title FROM sessions WHERE id = ?', legacyIdFor('session-panel', 'panel-1'))?.title).toBe('From tail')

    await appendFile(path, partial.slice(20) + '\n')
    const second = await bridge.tailMetadata('default', source)
    expect(second.applied).toBe(1)
    expect(second.pendingBytes).toBe(0)
    expect(database.get<{ cwd: string }>('SELECT cwd FROM execution_contexts WHERE id = ?', legacyIdFor('context-panel', 'panel-1'))?.cwd).toBe('/tmp/partial')
  })

  it('compares normalized projections and replays the repair queue idempotently by commandId', async () => {
    const equal = await bridge.compareProjection('default', source)
    expect(equal.equal).toBe(true)

    await bridge.mirrorMutation({
      schemaVersion: 1, commandId: 'retry-task', type: 'workbench-created', timestamp: 4,
      payload: { workbenchId: 'workbench-2', projectId: 'project-2', name: 'Task 2' }
    }, async () => undefined)
    await bridge.mirrorMutation({
      schemaVersion: 1, commandId: 'create-project-2', type: 'project-created', timestamp: 5,
      payload: { projectId: 'project-2', name: 'Project 2', path: '/tmp/project-2' }
    }, async () => undefined)

    const repair = await bridge.processRepairQueue(6)
    expect(repair).toMatchObject({ completed: 1, failed: 0 })
    expect(database.get('SELECT id FROM tasks WHERE id = ?', legacyIdFor('task-workbench', 'workbench-2'))).toBeDefined()
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM tasks WHERE id = ?', legacyIdFor('task-workbench', 'workbench-2'))?.count).toBe(1)
  })
})

function snapshot() {
  return {
    version: 1, recoveryOffsets: { metadataJournalBytes: 0 },
    projects: { list: [{ id: 'project-1', name: 'Project', path: '/tmp/project' }], activeProjectId: 'project-1' },
    workbenches: { 'workbench-1': { id: 'workbench-1', projectId: 'project-1', name: 'Task', tabIds: ['tab-1'] } },
    tabs: { 'tab-1': { id: 'tab-1', workbenchId: 'workbench-1', name: 'Main', layoutRoot: { type: 'leaf', id: 'leaf-1', panelId: 'panel-1' } } },
    panels: { 'panel-1': { id: 'panel-1', projectId: 'project-1', workbenchId: 'workbench-1', tabId: 'tab-1', terminalId: 'term-1', mode: 'claude-code', title: 'Claude', cwd: '/tmp/project', claudeSessionId: 'provider-1' } }
  }
}
