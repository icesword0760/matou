// Run frozen source behavior on disposable SQLite data; no production application state.
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'
import { HierarchyApplicationService } from '../../apps/runtime/src/hierarchy/hierarchy-application-service'

const temporary = mkdtempSync(join(tmpdir(), 'matou-bootstrap-contract-'))
const path = join(temporary, 'reference.sqlite')
let db = RuntimeDatabase.open(path)
await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate()
let service = new HierarchyApplicationService(db, new DomainTransactionManager(db))
const tables = ['workspaces', 'execution_contexts', 'tasks', 'scenes', 'scene_nodes',
  'sessions', 'session_mounts', 'app_windows', 'window_task_placements',
  'window_navigation', 'window_workspace_focus', 'window_task_focus',
  'window_scene_focus', 'bootstrap_state', 'session_canvas_memberships']
const snapshot = () => Object.fromEntries(tables.map(table => [table, db.all(`SELECT * FROM ${table} ORDER BY rowid`)]))
const steps: unknown[] = []
const originalInput = { windowId: 'main-window-1', defaultRootDirectory: join(temporary, 'workspace'), defaultName: 'workspace', now: 1000 }
for (const [name, commandId, input] of [
  ['fresh', 'initial', originalInput],
  ['replay', 'initial', originalInput],
  ['reopen', 'reopened', { ...originalInput, now: 2000 }],
  ['second-window', 'second-window', { ...originalInput, windowId: 'main-window-2', now: 3000 }],
  ['new-default-root', 'new-root', { ...originalInput, defaultRootDirectory: join(temporary, 'second'), defaultName: 'second', now: 4000 }],
  ['invalid-name', 'invalid', { ...originalInput, defaultRootDirectory: join(temporary, 'invalid'), defaultName: '  ', now: 5000 }]
] as const) {
  if (name === 'reopen') {
    db.close()
    db = RuntimeDatabase.open(path)
    await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate()
    service = new HierarchyApplicationService(db, new DomainTransactionManager(db))
  }
  const command = { commandId, commandType: 'hierarchy.bootstrap-window', requestHash: `bootstrap-${commandId}` }
  let result: unknown, error: string | undefined
  try { result = service.bootstrapWindow(command, input) } catch (e) { error = (e as Error).message }
  const workspace = db.get<{ id: string }>('SELECT id FROM workspaces WHERE root_directory = ?', input.defaultRootDirectory)
  const identity = workspace ? db.get(`SELECT w.id AS workspaceId, t.execution_context_id AS executionContextId,
    t.id AS taskId, s.id AS sceneId, s.root_node_id AS rootNodeId, m.session_id AS sessionId, m.id AS mountId
    FROM workspaces w JOIN tasks t ON t.workspace_id = w.id JOIN scenes s ON s.task_id = t.id
    JOIN session_mounts m ON m.scene_id = s.id WHERE w.id = ? ORDER BY t.created_at, s.created_at LIMIT 1`, workspace.id) : null
  steps.push({ name, input, command, ids: identity, result, error, snapshot: snapshot(),
    events: db.all('SELECT * FROM domain_events ORDER BY seq'),
    receipts: db.all('SELECT * FROM command_deduplication ORDER BY rowid') })
}
db.close()
const output = resolve('apps/native/core/tests/fixtures/bootstrap-reference.json')
mkdirSync(resolve('apps/native/core/tests/fixtures'), { recursive: true })
writeFileSync(output, JSON.stringify({ source: 'Frozen hierarchy source, real SQLite; zh-CN. UUIDs and commit times captured for native replay, no source behavior altered.', versions: process.versions, tables, steps }, null, 2) + '\n')
console.log(JSON.stringify({ output, temporary, steps: steps.length }))
