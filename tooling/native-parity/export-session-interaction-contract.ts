// Run the frozen interaction service against disposable SQLite.
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { HierarchyApplicationService } from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import { runtimeMessages } from '../../apps/runtime/src/i18n/messages'
import { SessionCanvasService, type SetFocusedSessionInput } from '../../apps/runtime/src/session-canvas/session-canvas-service'
import { SessionInteractionService, type RecordSessionInteractionInput } from '../../apps/runtime/src/session-canvas/session-interaction-service'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'

const root = mkdtempSync(join(tmpdir(), 'matou-interaction-contract-'))
const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
try {
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const hierarchy = new HierarchyApplicationService(database, transactions)
  const canvas = new SessionCanvasService(database, transactions)
  const interactions = new SessionInteractionService(database, transactions)
  const command = (label: string) => ({ commandId: label, commandType: 'session.interaction', requestHash: label })
  const windowId = 'window'
  const bootstrap = hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId,
    defaultRootDirectory: '/tmp/interaction-contract-workspace',
    defaultName: 'Workspace',
    now: 1000
  })
  const sceneId = bootstrap.scene!.id
  const first = bootstrap.session!.id
  const second = canvas.createShellSibling(command('sibling'), {
    windowId,
    sceneId,
    sourceSessionId: first,
    now: 1001
  }).created.session.id

  const seedPath = join(root, 'seed.sqlite')
  await database.backupTo(seedPath)
  const seedBase64 = readFileSync(seedPath).toString('base64')
  const tables = [
    'runtime_sequences', 'session_canvas_memberships', 'window_scene_focus',
    'sessions', 'tasks', 'workspaces', 'domain_events', 'command_deduplication'
  ]
  const snapshot = () => Object.fromEntries(
    tables.map(table => [table, database.all(`SELECT * FROM ${table} ORDER BY rowid`)])
  )
  const steps: unknown[] = []
  const record = (
    label: string,
    input: RecordSessionInteractionInput,
    metadata = command(label),
    sql: string[] = []
  ) => {
    for (const statement of sql) database.exec(statement)
    let result
    let error
    try {
      result = interactions.record(metadata, input)
    } catch (failure) {
      error = (failure as Error).message
    }
    const receipt = database.get<{ committed_at: number }>(
      'SELECT committed_at FROM command_deduplication WHERE command_id = ?',
      metadata.commandId
    )
    steps.push({
      op: 'record', label, command: metadata, input, sql, result, error,
      committedAt: receipt?.committed_at ?? input.now, snapshot: snapshot()
    })
  }
  const focus = (label: string, input: SetFocusedSessionInput) => {
    const result = canvas.setFocusedSession(input)
    steps.push({ op: 'focus', label, input, sql: [], result, snapshot: snapshot() })
  }

  const submit = { sessionId: second, interactionKind: 'submit', now: 2000 } as const
  const submitCommand = command('active-submit')
  record('active-submit', submit, submitCommand)
  record('inactive-control', { sessionId: first, interactionKind: 'control', deferOrdering: false, now: 2001 })
  record('submit-replay', submit, submitCommand)
  record('explicitly-deferred', { sessionId: first, interactionKind: 'provider-action', deferOrdering: true, now: 2002 })
  focus('apply-submit-on-blur', { windowId, sceneId, sessionId: first, now: 2003 })
  focus('apply-explicit-defer-on-blur', { windowId, sceneId, sessionId: second, now: 2004 })
  record('invalid-kind', { sessionId: first, interactionKind: 'focus' as RecordSessionInteractionInput['interactionKind'], now: 2005 })
  record('missing-session', { sessionId: 'missing', interactionKind: 'submit', now: 2006 })
  record('archived-session', { sessionId: first, interactionKind: 'control', now: 2007 },
    command('archived-session'), [`UPDATE sessions SET status = 'archived', archived_at = 2007 WHERE id = '${first}'`])

  const output = process.argv[2]
  if (!output) throw new Error('Pass the output fixture path')
  writeFileSync(output, JSON.stringify({
    versions: process.versions,
    invalidKindMessage: runtimeMessages().sessionCanvas.interactionKindNotOrdering,
    seedBase64,
    steps
  }, null, 2) + '\n')
  console.log(JSON.stringify({ steps: steps.length }))
} finally {
  database.close()
}
