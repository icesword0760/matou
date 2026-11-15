// Frozen ordinary-Shell domain lifecycle on disposable SQLite.
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'
import { HierarchyApplicationService } from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import { SessionRepository } from '../../apps/runtime/src/domain/session-repository'

const root = mkdtempSync(join(tmpdir(), 'matou-session-repository-contract-'))
const path = join(root, 'matou.sqlite')
let database = RuntimeDatabase.open(path)
await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
const bootstrapInput = { windowId: 'window', defaultRootDirectory: join(root, 'workspace'), defaultName: 'workspace', now: 1000 }
const bootstrapCommand = { commandId: 'bootstrap', commandType: 'hierarchy.bootstrap-window', requestHash: 'bootstrap' }
new HierarchyApplicationService(database, new DomainTransactionManager(database)).bootstrapWindow(bootstrapCommand, bootstrapInput)
const ids = database.get<{ workspaceId: string, executionContextId: string, taskId: string }>(`SELECT
  w.id AS workspaceId,t.execution_context_id AS executionContextId,t.id AS taskId,
  s.id AS sceneId,s.root_node_id AS rootNodeId,m.session_id AS sessionId,m.id AS mountId
  FROM workspaces w JOIN tasks t ON t.workspace_id=w.id JOIN scenes s ON s.task_id=t.id
  JOIN session_mounts m ON m.scene_id=s.id LIMIT 1`)!
const bootstrapCommittedAt = database.get<{ committed_at: number }>('SELECT committed_at FROM command_deduplication WHERE command_id=?', 'bootstrap')!.committed_at
let repository = new SessionRepository(database, new DomainTransactionManager(database))
const tables = ['sessions', 'session_runs', 'domain_events', 'command_deduplication']
const snapshot = () => Object.fromEntries(tables.map(table => [table, database.all(`SELECT * FROM ${table} ORDER BY rowid`)]))
const steps: unknown[] = []
const create = { id: 'shell-2', taskId: ids.taskId, executionContextId: ids.executionContextId, title: ' Shell ', now: 1100 }
const actions: Array<{ op: string, input: any, commandId?: string, hash?: string }> = [
  { op: 'create', input: create, commandId: 'create' },
  { op: 'create', input: create, commandId: 'create' },
  { op: 'create', input: { ...create, id: 'conflict' }, commandId: 'create', hash: 'different' },
  { op: 'start', input: { id: 'run-1', sessionId: 'shell-2', runtimeGeneration: 'generation-1', pid: 123, cols: 80, rows: 24, now: 1200 } },
  { op: 'start', input: { id: 'run-2', sessionId: 'shell-2', runtimeGeneration: 'generation-1', cols: 97, rows: 33, now: 1300 } },
  { op: 'finish', input: { runId: 'run-2', exitCode: 7, signal: 0, now: 1400 } },
  { op: 'interrupt', input: { runId: 'run-1', now: 1500 } },
  { op: 'interrupt', input: { runId: 'run-1', now: 1600 } },
  { op: 'rename', input: { sessionId: 'shell-2', title: ' 新名字 ', now: 1700 } },
  { op: 'cwd', input: { sessionId: 'shell-2', cwd: ' /tmp/中文 ', now: 1800 } },
  { op: 'rename', input: { sessionId: 'shell-2', title: ' ', now: 1900 } },
  { op: 'cwd', input: { sessionId: 'shell-2', cwd: ' ', now: 2000 } },
  { op: 'start', input: { id: 'run-3', sessionId: 'shell-2', runtimeGeneration: 'generation-2', pid: 456, cols: 80, rows: 24, now: 2100 } },
  { op: 'archive', input: { sessionId: 'shell-2', now: 2200 } },
  { op: 'interrupt', input: { runId: 'run-3', now: 2300 } },
  { op: 'archive', input: { sessionId: 'shell-2', now: 2400 } },
  { op: 'start', input: { id: 'run-4', sessionId: 'shell-2', runtimeGeneration: 'generation-2', cols: 80, rows: 24, now: 2500 } },
  { op: 'rename', input: { sessionId: 'shell-2', title: '不应写入', now: 2600 } },
  { op: 'cwd', input: { sessionId: 'shell-2', cwd: '/tmp/never', now: 2700 } },
  { op: 'create', input: { ...create, id: 'unknown-task', taskId: 'missing' } },
  { op: 'create', input: { ...create, id: 'unknown-context', executionContextId: 'missing' } },
  { op: 'reopen', input: {} },
]
for (const [index, action] of actions.entries()) {
  const commandId = action.commandId ?? `command-${index}`
  const command = { commandId, commandType: `session.${action.op}`, requestHash: action.hash ?? commandId }
  const input = action.input
  let result: unknown, error: string | undefined
  try {
    switch (action.op) {
      case 'create': result = repository.createSession(command, { ...input, kind: 'shell' }); break
      case 'start': result = repository.startRun(command, { ...input, profile: 'shell' }); break
      case 'finish': result = repository.finishRun(command, input.runId, input); break
      case 'interrupt': result = repository.interruptRun(command, input.runId, input.now); break
      case 'rename': result = repository.renameSession(command, input); break
      case 'cwd': result = repository.updateCwd(command, input.sessionId, input.cwd, input.now); break
      case 'archive': result = repository.archiveSession(command, input.sessionId, input.now); break
      case 'reopen':
        database.close(); database = RuntimeDatabase.open(path)
        repository = new SessionRepository(database, new DomainTransactionManager(database))
        result = { sessions: repository.listSessions(), runs: repository.listAllRuns(), sessionRuns: repository.listRuns('shell-2'), missing: repository.getSession('missing') ?? null }
        break
    }
  } catch (e) { error = (e as Error).message }
  const receipt = database.get<{ committed_at: number }>('SELECT committed_at FROM command_deduplication WHERE command_id=?', commandId)
  steps.push({ op: action.op, input, command, result, error, committedAt: receipt?.committed_at ?? 9999, snapshot: snapshot() })
}
database.close()
const output = process.argv[2]!
writeFileSync(output, JSON.stringify({ versions: process.versions, bootstrap: { input: bootstrapInput, command: bootstrapCommand, ids, committedAt: bootstrapCommittedAt }, tables, steps }, null, 2) + '\n')
console.log(JSON.stringify({ output, temporary: root, steps: steps.length }))
