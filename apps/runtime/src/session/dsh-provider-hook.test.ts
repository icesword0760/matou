import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { SessionRepository } from '../domain/session-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ProviderHookServer } from './provider-hook-server'

const command = (id: string) => ({ commandId: id, commandType: 'test', requestHash: id })
let db: RuntimeDatabase, sessions: SessionRepository, hooks: ProviderHookServer
let identities: unknown[], notifications: unknown[], statuses: unknown[], mismatches: unknown[]
let currentRun: string | undefined
beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-dsh-hook-'))
  db = RuntimeDatabase.open(join(root, 'test.sqlite'))
  await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate()
  const tx = new DomainTransactionManager(db), workspaces = new WorkspaceTaskRepository(db, tx)
  workspaces.createWorkspace(command('workspace'), { id: 'w', name: 'DSH', rootDirectory: root, now: 1 })
  workspaces.createPlainExecutionContext(command('context'), { id: 'c', workspaceId: 'w', cwd: root, now: 1 })
  workspaces.createTask(command('task'), { id: 't', workspaceId: 'w', executionContextId: 'c', title: 'DSH', status: 'active', sortKey: 'a', now: 1 })
  sessions = new SessionRepository(db, tx)
  for (const id of ['a', 'b']) sessions.createSession(command(id), { id, taskId: 't', executionContextId: 'c', kind: 'dsh', title: 'DSH', now: 2 })
  identities = []; notifications = []; statuses = []; mismatches = []; currentRun = undefined
  hooks = new ProviderHookServer(root, sessions, { currentRunId: () => currentRun,
    onIdentityRecorded: e => { identities.push(e) }, onNotification: e => { notifications.push(e) },
    onWorkStatus: e => { statuses.push(e) }, onIdentityMismatch: e => { mismatches.push(e) } })
  await hooks.start()
})
afterEach(async () => { await hooks.stop(); db.close() })
function payload(type: string, sequence: number, extra: Record<string, unknown> = {}) {
  return { dsh_observation: 1, run_id: 'r', sequence, generation: 1, session_id: 'native-a', foreground: true,
    event_type: type, event_data: {}, ...(type === 'ready' ? { ready: true, persisted: true, cwd: '/same/cwd', agent_status: 'idle' } : {}), ...extra }
}
async function send(url: string, body: Record<string, unknown>) {
  const response = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
  expect(response.status).toBe(200)
}
describe('DSH authenticated provider pipeline', () => {
  it('confirms an empty session without making it resumable, then promotes only on persisted native evidence', async () => {
    const run = await hooks.registerSession('dsh', { sessionId: 'a', runId: 'r' })
    await send(run.hookUrl, payload('ready', 1, { persisted: false }))
    expect(identities).toMatchObject([{ provider: 'dsh', providerSessionId: 'native-a', resumable: false }])
    expect(sessions.getResumeBinding('a', 'dsh')).toBeUndefined()
    expect(sessions.getProviderSettingsBinding('a', 'dsh')?.resumeState).toBe('unknown')
    await send(run.hookUrl, payload('ready', 2))
    expect(sessions.getResumeBinding('a', 'dsh')?.resumeState).toBe('available')
    await send(run.hookUrl, payload('ready', 3))
    expect(identities).toHaveLength(2)
    expect(notifications).toEqual([])
  })
  it('does not confirm restore from screen-like activity, mismatched ID or a non-persisted ready', async () => {
    const run = await hooks.registerSession('dsh', { sessionId: 'a', runId: 'r', expectedProviderSessionId: 'native-a' })
    await send(run.hookUrl, payload('turn/start', 1, { event_data: { turn: 1 } }))
    await send(run.hookUrl, payload('ready', 2, { persisted: false }))
    expect(identities).toEqual([])
    expect(statuses).toEqual([])
    await send(run.hookUrl, payload('ready', 3, { session_id: 'wrong-id' }))
    expect(mismatches).toHaveLength(1)
    await send(run.hookUrl, payload('ready', 4))
    expect(identities).toEqual([])
  })
  it('isolates same-cwd cards, rejects background/replayed/old-run messages, and sends DSH notifications once', async () => {
    const a = await hooks.registerSession('dsh', { sessionId: 'a', runId: 'r' })
    const b = await hooks.registerSession('dsh', { sessionId: 'b', runId: 'r' })
    await send(a.hookUrl, payload('ready', 1))
    await send(b.hookUrl, payload('ready', 1, { session_id: 'native-b' }))
    await send(a.hookUrl, payload('turn/start', 2, { foreground: false, event_data: { turn: 1 } }))
    await send(a.hookUrl, payload('turn/start', 3, { event_data: { turn: 1 } }))
    const end = payload('turn/end', 4, { event_data: { turn: 1, reason: { kind: 'completed' } } })
    await send(a.hookUrl, end); await send(a.hookUrl, end)
    expect(notifications).toMatchObject([{ sessionId: 'a', provider: 'dsh', event: { title: 'DSH', eventType: 'completed' } }])
    expect(sessions.getResumeBinding('b', 'dsh')?.providerSessionId).toBe('native-b')
    currentRun = 'replacement-run'
    const count = statuses.length
    await send(a.hookUrl, payload('ready', 5, { session_id: 'stale-id', generation: 2 }))
    expect(statuses).toHaveLength(count)
    expect(sessions.getResumeBinding('a', 'dsh')?.providerSessionId).toBe('native-a')
  })
  it('rebinds an intentional foreground switch without importing detached child events', async () => {
    const run = await hooks.registerSession('dsh', { sessionId: 'a', runId: 'r', expectedProviderSessionId: 'native-a' })
    await send(run.hookUrl, payload('ready', 1))
    await send(run.hookUrl, payload('turn/start', 2, { session_id: 'detached-child', event_data: { turn: 1 } }))
    await send(run.hookUrl, payload('ready', 3, { session_id: 'native-new', generation: 2, persisted: false }))
    expect(identities).toMatchObject([{ providerSessionId: 'native-a' }, { providerSessionId: 'native-new', resumable: false }])
    expect(sessions.getResumeBinding('a', 'dsh')).toBeUndefined()
    expect(sessions.getProviderSettingsBinding('a', 'dsh')?.providerSessionId).toBe('native-new')
    expect(notifications).toEqual([])
  })
})
