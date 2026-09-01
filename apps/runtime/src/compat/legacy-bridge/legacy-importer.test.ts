import { mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { readSessionFrames } from '../../journal/segment-journal'
import { RuntimeDatabase } from '../../storage/database'
import { DomainTransactionManager } from '../../storage/domain-transaction'
import { MigrationRunner } from '../../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../../storage/migrations'
import { LegacyImporter } from './legacy-importer'

let root: string
let source: string
let database: RuntimeDatabase
let importer: LegacyImporter

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-legacy-import-'))
  source = join(root, 'legacy-session')
  await mkdir(join(source, 'journals', 'terminals'), { recursive: true })
  await mkdir(join(source, 'scrollback'), { recursive: true })
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  importer = new LegacyImporter(root, database, new DomainTransactionManager(database))
})

afterEach(() => database.close())

describe('LegacyImporter', () => {
  it('imports reference product Project/Workbench/Tab/Panel identities, provider bindings, teams, and terminal history', async () => {
    await writeFixture(source)

    const result = await importer.importSource(source)

    expect(result.replayed).toBe(false)
    expect(result.report.counts).toMatchObject({ workspaces: 1, tasks: 1, scenes: 1, sessions: 2, providerBindings: 2, relations: 1 })
    expect(database.get<{ root_directory: string }>('SELECT root_directory FROM workspaces')?.root_directory).toBe('/tmp/project')
    expect(database.get<{ cwd: string }>("SELECT cwd FROM execution_contexts WHERE id LIKE 'legacy-context-panel-%' ORDER BY id LIMIT 1")?.cwd).toMatch(/^\/tmp\/project/)
    expect(database.get<{ provider_session_id: string }>('SELECT provider_session_id FROM provider_bindings ORDER BY provider_session_id LIMIT 1')?.provider_session_id).toBe('claude-lead')
    expect(database.get<{ relation_kind: string }>('SELECT relation_kind FROM session_relations_current')?.relation_kind).toBe('team-member-of')
    const sessionId = database.get<{ entity_id: string }>(
      "SELECT entity_id FROM legacy_entity_mappings WHERE legacy_type = 'panel' AND legacy_id = 'panel-lead'"
    )!.entity_id
    const frames = await readSessionFrames(root, sessionId)
    expect(frames.some((frame) => frame.kind === 'output' && new TextDecoder().decode(frame.data).includes('legacy output'))).toBe(true)
  })

  it('is source-fingerprint idempotent and does not duplicate entities or journals', async () => {
    await writeFixture(source)
    const first = await importer.importSource(source)
    const second = await importer.importSource(source)

    expect(second.replayed).toBe(true)
    expect(second.importRunId).toBe(first.importRunId)
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM sessions')?.count).toBe(2)
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM legacy_import_runs')?.count).toBe(1)
  })

  it('falls back to the checkpoint, applies valid metadata tail records, and isolates malformed panels', async () => {
    const fixture = snapshot()
    ;(fixture.panels as Record<string, unknown>)['broken'] = { id: 'broken', projectId: 1 }
    await writeFile(join(source, 'snapshot.json'), '{bad json')
    await writeFile(join(source, 'checkpoint.json'), JSON.stringify(fixture))
    await writeFile(join(source, 'journals', 'metadata.ndjson'), [
      JSON.stringify({ type: 'panel-updated', ts: 2, payload: { panelId: 'panel-lead', cwd: '/tmp/project/updated' } }),
      '{torn'
    ].join('\n'))

    const result = await importer.importSource(source)

    expect(result.report.source).toBe('checkpoint.json')
    expect(result.report.repaired.some(({ code }) => code === 'snapshot-fallback')).toBe(true)
    expect(result.report.repaired.some(({ code }) => code === 'metadata-tail-truncated')).toBe(true)
    expect(result.report.ignored.some(({ legacyId }) => legacyId === 'broken')).toBe(true)
    expect(database.get<{ cwd: string }>("SELECT cwd FROM execution_contexts WHERE id LIKE 'legacy-context-panel-%' AND cwd LIKE '%updated'")?.cwd).toBe('/tmp/project/updated')
  })

  it('produces a consistency report for dangling layout and team references without failing healthy sessions', async () => {
    const fixture = snapshot()
    fixture.tabs['tab-1'].layoutRoot = {
      type: 'split', id: 'split-1', direction: 'horizontal', children: [
        { type: 'leaf', id: 'leaf-lead', panelId: 'panel-lead' },
        { type: 'leaf', id: 'leaf-missing', panelId: 'missing-panel' }
      ]
    }
    fixture.panels['panel-member'].teamLeadSessionId = 'missing-lead'
    await writeFile(join(source, 'snapshot.json'), JSON.stringify(fixture))

    const result = await importer.importSource(source)

    expect(result.report.consistency.danglingLayoutPanels).toEqual(['missing-panel'])
    expect(result.report.consistency.unresolvedTeamLeads).toEqual(['panel-member'])
    expect(result.report.counts.sessions).toBe(2)
  })
})

async function writeFixture(directory: string): Promise<void> {
  await writeFile(join(directory, 'snapshot.json'), JSON.stringify(snapshot()))
  await writeFile(join(directory, 'journals', 'metadata.ndjson'), '')
  await writeFile(join(directory, 'journals', 'terminals', 'term-lead.log'), 'legacy output\n')
  await writeFile(join(directory, 'scrollback', 'term-member.txt'), 'member scrollback\n')
}

function snapshot() {
  return {
    version: 1,
    savedAt: '2026-01-01T00:00:00.000Z',
    recoveryOffsets: { metadataJournalBytes: 0 },
    projects: { list: [{ id: 'project-1', name: 'Project', path: '/tmp/project', workbenchIds: ['workbench-1'], activeWorkbenchId: 'workbench-1' }], activeProjectId: 'project-1' },
    workbenches: { 'workbench-1': { id: 'workbench-1', projectId: 'project-1', name: 'Task', tabIds: ['tab-1'], activeTabId: 'tab-1' } },
    tabs: {
      'tab-1': {
        id: 'tab-1', workbenchId: 'workbench-1', name: 'Main', activeLeafId: 'leaf-lead',
        layoutRoot: { type: 'split', id: 'split-1', direction: 'horizontal', children: [
          { type: 'leaf', id: 'leaf-lead', panelId: 'panel-lead' },
          { type: 'leaf', id: 'leaf-member', panelId: 'panel-member' }
        ] }
      }
    },
    panels: {
      'panel-lead': { id: 'panel-lead', projectId: 'project-1', workbenchId: 'workbench-1', tabId: 'tab-1', terminalId: 'term-lead', mode: 'claude-code', title: 'Lead', cwd: '/tmp/project', claudeSessionId: 'claude-lead', aiPermissionMode: 'bypassPermissions', teamId: 'team-1', teamRole: 'leader', teamLeadSessionId: 'claude-lead' },
      'panel-member': { id: 'panel-member', projectId: 'project-1', workbenchId: 'workbench-1', tabId: 'tab-1', terminalId: 'term-member', mode: 'claude-code', title: 'Member', cwd: '/tmp/project/sub', claudeSessionId: 'claude-member', teamId: 'team-1', teamRole: 'teammate', teamLeadSessionId: 'claude-lead' }
    }
  }
}
