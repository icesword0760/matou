import { execFile } from 'node:child_process'
import { access, mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { DomainCommandMetadata } from '@matou/domain'

import { SessionRepository } from '../domain/session-repository'
import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import {
  SessionForkIntentRepository,
  type ForkLease
} from '../session/session-fork-intent-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ForkWorkflowService, type ExecuteForkInput, type ForkWorkflowResult } from './fork-workflow-service'
import {
  ForkKillPointCrash,
  ForkOperationCoordinator,
  type ForkKillPoint,
  type ForkOperationNotification
} from './fork-operation-coordinator'

const exec = promisify(execFile)

let root: string
let workspaceRoot: string
let database: RuntimeDatabase
let hierarchy: HierarchyApplicationService
let workflow: ForkWorkflowService
let intents: SessionForkIntentRepository
let sessions: SessionRepository
let source: { sceneId: string; sessionId: string; executionContextId: string }
const coordinators: ForkOperationCoordinator[] = []
const releases: Array<() => void> = []

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-fork-coordinator-'))
  workspaceRoot = join(root, 'workspace')
  await mkdir(workspaceRoot)
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  workflow = new ForkWorkflowService(root, database, transactions, { stopRuns: async () => undefined })
  intents = new SessionForkIntentRepository(database)
  sessions = new SessionRepository(database, transactions)
  const bootstrapped = hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: workspaceRoot,
    defaultName: 'Workspace', now: 1
  })
  source = {
    sceneId: bootstrapped.scene!.id,
    sessionId: bootstrapped.session!.id,
    executionContextId: bootstrapped.executionContext!.id
  }
  database.run("UPDATE sessions SET kind = 'claude-code' WHERE id = ?", source.sessionId)
  database.run(
    `INSERT INTO provider_bindings (
       id, session_id, provider, provider_session_id, resume_state, restore_state,
       metadata_json, created_at, updated_at, validated_at
     ) VALUES ('source-provider', ?, 'claude-code', 'provider-source',
               'available', 'none', '{"canFork":true}', 2, 2, 2)`,
    source.sessionId
  )
})

afterEach(async () => {
  for (const coordinator of coordinators) coordinator.stop()
  for (const release of releases) release()
  await new Promise((resolve) => setTimeout(resolve, 0))
  database.close()
  coordinators.length = 0
  releases.length = 0
})

describe('ForkOperationCoordinator', () => {
  it('runs at most two Git/setup operations and leaves the third queued until a slot opens', async () => {
    const operations = await Promise.all([
      accept('并发一', 'submission-1'),
      accept('并发二', 'submission-2'),
      accept('并发三', 'submission-3')
    ])
    const started: string[] = []
    const gates = new Map<string, Promise<void>>()
    const gateReleases = new Map<string, () => void>()
    for (const operation of operations) {
      gates.set(operation.operationId, new Promise((resolve) => {
        releases.push(resolve)
        gateReleases.set(operation.operationId, resolve)
      }))
    }
    const executeFork = vi.fn(async (_command, input: ExecuteForkInput) => {
      started.push(input.operationId)
      await gates.get(input.operationId)
      const advanced = intents.advanceStage({
        operationId: input.operationId, lease: input.lease,
        stage: 'restoring-provider', now: 10
      })
      return { forkProgress: advanced.kind === 'applied' ? advanced.progress : undefined } as ForkWorkflowResult
    })
    const coordinator = createCoordinator({ executeFork, now: () => 10 })
    coordinator.start()

    await eventually(() => expect(started).toHaveLength(2))
    const queued = operations.find(({ operationId }) => !started.includes(operationId))!
    expect(intents.progressByOperation(queued.operationId)?.stage).toBe('queued')
    gateReleases.get(started[0]!)!()
    await eventually(() => expect(started).toHaveLength(3))
  })

  it('takes over an expired startup lease with deterministic owner, token and higher fence', async () => {
    const operation = await accept('重启接管', 'takeover-submission')
    const old = intents.acquireLease({
      operationId: operation.operationId, owner: 'old-runtime', now: 0, ttlMs: 5
    })
    if (old.kind !== 'acquired') throw new Error('old lease missing')
    let observed: ExecuteForkInput | undefined
    const pending = new Promise<void>((resolve) => { releases.push(resolve) })
    const coordinator = createCoordinator({
      now: () => 10,
      executeFork: async (_command, input) => {
        observed = input
        await pending
        return {} as ForkWorkflowResult
      }
    })
    coordinator.start()

    await eventually(() => expect(observed).toBeDefined())
    expect(observed!.lease.fence).toBe(old.lease.fence + 1)
    expect(observed!.lease.owner).toBe(`runtime-new:${operation.operationId}`)
    expect(observed!.lease.token).toBe(
      `runtime-new:${operation.operationId}:${old.lease.fence + 1}`
    )
  })

  it('renews an active operation lease on the two-second coordinator heartbeat', async () => {
    const operation = await accept('续租', 'heartbeat-submission')
    let now = 10
    const pending = new Promise<void>((resolve) => { releases.push(resolve) })
    const coordinator = createCoordinator({
      now: () => now,
      executeFork: async () => {
        await pending
        return {} as ForkWorkflowResult
      }
    })
    vi.useFakeTimers()
    try {
      coordinator.start()
      await vi.advanceTimersByTimeAsync(0)
      now = 2_010
      await vi.advanceTimersByTimeAsync(2_000)
      expect(database.get(
        `SELECT last_heartbeat_at, lease_expires_at
         FROM session_fork_intents WHERE operation_id = ?`, operation.operationId
      )).toEqual({ last_heartbeat_at: 2_010, lease_expires_at: 10_010 })
      for (let index = 0; index < 5; index += 1) {
        now += 2_000
        await vi.advanceTimersByTimeAsync(2_000)
      }
      expect(database.get(
        `SELECT stage, state, last_heartbeat_at, lease_expires_at
         FROM session_fork_intents WHERE operation_id = ?`, operation.operationId
      )).toEqual({
        stage: 'queued', state: 'pending',
        last_heartbeat_at: 12_010, lease_expires_at: 20_010
      })
    } finally {
      coordinator.stop()
      vi.useRealTimers()
    }
  })

  it('waits for authoritative provider identity and emits one deterministic completion notification', async () => {
    const operation = await accept('Provider恢复', 'provider-submission')
    const notifications: ForkOperationNotification[] = []
    let authority: ForkStartAuthority | undefined
    const startOrResume = vi.fn(async (_sessionId: string, input: ForkStartAuthority) => {
      authority = input
    })
    const coordinator = createCoordinator({
      now: () => 10,
      startOrResume,
      notify: (notification) => { notifications.push(notification) },
      executeFork: async (_command, input) => {
        const advanced = intents.advanceStage({
          operationId: input.operationId, lease: input.lease,
          stage: 'restoring-provider', now: 10
        })
        return { forkProgress: advanced.kind === 'applied' ? advanced.progress : undefined } as ForkWorkflowResult
      }
    })
    coordinator.start()

    await eventually(() => expect(startOrResume).toHaveBeenCalledWith(
      operation.sessionId,
      expect.objectContaining({ operationId: operation.operationId, runId: expect.any(String) })
    ))
    await eventually(() => expect(intents.progressByOperation(operation.operationId)?.stage).toBe('restoring-provider'))
    expect(coordinator.recordOrdinaryOutput(operation.sessionId, 'x'.repeat(2001))).toBe(false)
    expect(intents.progressByOperation(operation.operationId)?.stage).toBe('restoring-provider')
    recordAuthoritativeIdentity(operation.sessionId, 'provider-authoritative', authority!)
    await expect(coordinator.confirmAuthoritativeIdentity(
      operation.sessionId, 'provider-authoritative', operation.operationId
    )).resolves.toBe(true)
    await expect(coordinator.confirmAuthoritativeIdentity(
      operation.sessionId, 'provider-authoritative'
    )).resolves.toBe(false)

    expect(intents.progressByOperation(operation.operationId)?.stage).toBe('succeeded')
    expect(notifications).toEqual([{
      eventId: `fork-operation:${operation.operationId}:succeeded`,
      replacementKey: `fork-operation:${operation.operationId}`,
      status: 'succeeded', operationId: operation.operationId, sessionId: operation.sessionId
    }])
  })

  it('ends provider identity waiting at a bounded deadline while preserving the Fork Session', async () => {
    const operation = await accept('Provider身份超时', 'provider-timeout-submission')
    const notifications: ForkOperationNotification[] = []
    let now = 10
    const coordinator = createCoordinator({
      now: () => now,
      identityTimeoutMs: 50,
      notify: (notification) => { notifications.push(notification) },
      executeFork: async (_command, input) => {
        const advanced = intents.advanceStage({
          operationId: input.operationId, lease: input.lease,
          stage: 'restoring-provider', now
        })
        return {
          forkProgress: advanced.kind === 'applied' ? advanced.progress : undefined
        } as ForkWorkflowResult
      }
    })
    coordinator.start()

    await eventually(() => expect(
      intents.progressByOperation(operation.operationId)?.stage
    ).toBe('restoring-provider'))
    now = 61
    await coordinator.reconcile()

    expect(intents.progressByOperation(operation.operationId)).toMatchObject({
      stage: 'failed', error: expect.stringContaining('身份确认超时')
    })
    expect(database.get<{ id: string }>(
      'SELECT id FROM sessions WHERE id = ?', operation.sessionId
    )).toEqual({ id: operation.sessionId })
    expect(notifications).toEqual([expect.objectContaining({
      status: 'failed', operationId: operation.operationId, sessionId: operation.sessionId
    })])
  })

  it('returns an interrupted legacy window launch to provider restore before restarting it', async () => {
    const operation = await accept('Provider重启', 'provider-restart-submission')
    const old = intents.acquireLease({
      operationId: operation.operationId, owner: 'old-runtime', now: 0, ttlMs: 5
    })
    if (old.kind !== 'acquired') throw new Error('old provider lease missing')
    applied(intents.advanceStage({
      operationId: operation.operationId, lease: old.lease,
      stage: 'restoring-provider', now: 1
    }))
    applied(intents.advanceStage({
      operationId: operation.operationId, lease: old.lease,
      stage: 'starting-window', now: 2
    }))
    const startOrResume = vi.fn(async () => undefined)
    const coordinator = createCoordinator({
      ownerId: 'runtime-provider-restart', now: () => 10, startOrResume
    })
    coordinator.start()

    await eventually(() => expect(startOrResume).toHaveBeenCalledWith(
      operation.sessionId,
      expect.objectContaining({ operationId: operation.operationId, runId: expect.any(String) })
    ))
    expect(intents.progressByOperation(operation.operationId)?.stage).toBe('restoring-provider')
  })

  it('fails closed on a real Worktree identity mismatch before execution', async () => {
    const operation = await accept('错误身份', 'mismatch-submission', 'new')
    const executeFork = vi.fn()
    const notifications: ForkOperationNotification[] = []
    const coordinator = createCoordinator({
      now: () => 10,
      executeFork,
      health: { check: async () => ({ kind: 'mismatch', reason: 'wrong-branch' }) },
      notify: (notification) => { notifications.push(notification) }
    })
    coordinator.start()

    await eventually(() => expect(intents.progressByOperation(operation.operationId)?.stage).toBe('failed'))
    expect(executeFork).not.toHaveBeenCalled()
    expect(notifications).toHaveLength(1)
    expect(notifications[0]).toMatchObject({ status: 'failed', operationId: operation.operationId })
    coordinator.stop()
    const restarted = createCoordinator({
      ownerId: 'runtime-after-failure', now: () => 20,
      executeFork,
      notify: (notification) => { notifications.push(notification) }
    })
    restarted.start()
    await new Promise((resolve) => setTimeout(resolve, 10))
    expect(notifications).toHaveLength(2)
    expect(new Set(notifications.map(({ eventId }) => eventId))).toEqual(new Set([
      `fork-operation:${operation.operationId}:failed`
    ]))
  })

  it.each([
    'intent-accepted', 'branch-created', 'path-created', 'setup-completed',
    'session-bound', 'provider-before'
  ] as const)('restarts from %s without changing operation or asset identity', async (killPoint) => {
    let now = 10
    const operation = await accept(`断点-${killPoint}`, `kill-${killPoint}`, 'new')
    const before = intents.operationById(operation.operationId)!.identity
    const crashing = createCoordinator({
      now: () => now,
      observer: { reach: (point) => { if (point === killPoint) throw new ForkKillPointCrash(point) } },
      health: { check: async () => ({ kind: 'missing', reason: 'path-missing' }) },
      executeFork: stagedExecutor()
    })
    crashing.start()
    await eventually(() => {
      const row = database.get<{ lease_fence: number }>(
        'SELECT lease_fence FROM session_fork_intents WHERE operation_id = ?', operation.operationId
      )
      expect(row!.lease_fence).toBe(1)
    })
    await new Promise((resolve) => setTimeout(resolve, 0))
    crashing.stop()
    now += 20_000

    let authority: ForkStartAuthority | undefined
    const resumed = createCoordinator({
      ownerId: 'runtime-resumed',
      now: () => now,
      health: { check: async () => ({ kind: 'missing', reason: 'path-missing' }) },
      executeFork: stagedExecutor(),
      startOrResume: async (_sessionId, input) => { authority = input }
    })
    resumed.start()
    await eventually(() => expect(
      intents.progressByOperation(operation.operationId)?.stage
    ).toBe('restoring-provider'))
    await eventually(() => expect(authority).toBeDefined())
    recordAuthoritativeIdentity(operation.sessionId, 'provider-after-restart', authority!)
    await resumed.confirmAuthoritativeIdentity(
      operation.sessionId,
      'provider-after-restart',
      operation.operationId
    )

    expect(intents.operationById(operation.operationId)!.identity).toEqual(before)
    expect(intents.progressByOperation(operation.operationId)?.stage).toBe('succeeded')
  })
})

function createCoordinator(overrides: Partial<{
  ownerId: string
  now: () => number
  identityTimeoutMs: number
  executeFork: (command: DomainCommandMetadata, input: ExecuteForkInput) => Promise<ForkWorkflowResult>
  startOrResume: (sessionId: string, authority: ForkStartAuthority) => Promise<void>
  notify: (notification: ForkOperationNotification) => void
  observer: { reach(point: ForkKillPoint): void }
  health: { check(): Promise<
    { kind: 'missing'; reason: 'path-missing' } |
    { kind: 'mismatch'; reason: 'wrong-branch' }
  > }
}> = {}): ForkOperationCoordinator {
  const coordinator = new ForkOperationCoordinator(intents, {
    ownerId: overrides.ownerId ?? 'runtime-new',
    executeFork: overrides.executeFork ?? stagedExecutor(),
    startOrResume: overrides.startOrResume ?? (async () => undefined),
    now: overrides.now ?? (() => 10),
    heartbeatMs: 2_000,
    leaseTtlMs: 8_000,
    ...(overrides.identityTimeoutMs === undefined
      ? {}
      : { identityTimeoutMs: overrides.identityTimeoutMs }),
    ...(overrides.notify ? { notify: overrides.notify } : {}),
    ...(overrides.observer ? { observer: overrides.observer } : {}),
    ...(overrides.health ? { health: overrides.health } : {})
  })
  coordinators.push(coordinator)
  return coordinator
}

type ForkStartAuthority = {
  operationId: string
  runId: string
  lease: Pick<ForkLease, 'token' | 'fence'>
}

function recordAuthoritativeIdentity(
  sessionId: string,
  providerSessionId: string,
  authority: ForkStartAuthority
): void {
  sessions.recordResumableProviderIdentity(command(`identity-${authority.operationId}`), {
    id: `binding-${authority.operationId}`,
    sessionId,
    provider: 'claude-code',
    providerSessionId,
    metadata: {},
    now: 11,
    forkAuthority: authority
  })
}

function stagedExecutor() {
  return async (_command: DomainCommandMetadata, input: ExecuteForkInput) => {
    let progress = intents.progressByOperation(input.operationId)!
    const operation = () => intents.operationById(input.operationId)!
    if (progress.stage === 'queued') {
      progress = applied(intents.advanceStage({
        operationId: input.operationId, lease: input.lease,
        stage: 'creating-worktree', now: 10
      }))
    }
    if (progress.stage === 'creating-worktree') {
      await input.observer?.reach('branch-created', operation())
      await input.observer?.reach('path-created', operation())
      progress = applied(intents.advanceStage({
        operationId: input.operationId, lease: input.lease,
        stage: 'applying-setup', now: 10
      }))
    }
    if (progress.stage === 'applying-setup') {
      await input.observer?.reach('setup-completed', operation())
      progress = applied(intents.advanceStage({
        operationId: input.operationId, lease: input.lease,
        stage: 'binding-session', now: 10
      }))
    }
    if (progress.stage === 'binding-session') {
      await input.observer?.reach('session-bound', operation())
      progress = applied(intents.advanceStage({
        operationId: input.operationId, lease: input.lease,
        stage: 'restoring-provider', now: 10
      }))
    }
    return { forkProgress: progress } as ForkWorkflowResult
  }
}

async function accept(name: string, submissionKey: string, mode: 'current' | 'new' = 'current') {
  if (mode === 'new') {
    await ensureGitRepository(workspaceRoot)
    database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, dirty, updated_at
       ) VALUES (?, ?, 'ready', 'main', 0, 3)
       ON CONFLICT(execution_context_id) DO UPDATE SET
         repository_root = excluded.repository_root, state = 'ready', branch = 'main',
         dirty = 0, updated_at = 3`,
      source.executionContextId, workspaceRoot
    )
  }
  const result = await workflow.createForkChild(command(`accept-${submissionKey}`), {
    windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
    name, worktreeMode: mode, submissionKey, now: 3
  })
  return {
    operationId: result.forkProgress!.operationId,
    sessionId: result.session!.id
  }
}

async function ensureGitRepository(path: string): Promise<void> {
  if (await access(join(path, '.git')).then(() => true, () => false)) return
  await exec('git', ['init', '-b', 'main'], { cwd: path })
  await exec('git', ['config', 'user.name', 'Matou Test'], { cwd: path })
  await exec('git', ['config', 'user.email', 'matou@example.test'], { cwd: path })
  await writeFile(join(path, 'README.md'), 'baseline\n')
  await exec('git', ['add', 'README.md'], { cwd: path })
  await exec('git', ['commit', '-m', 'baseline'], { cwd: path })
}

function applied(result: ReturnType<SessionForkIntentRepository['advanceStage']>) {
  if (result.kind !== 'applied') throw new Error('stage write was stale')
  return result.progress
}

async function eventually(assertion: () => void, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    try {
      assertion()
      return
    } catch (error) {
      if (Date.now() >= deadline) throw error
      await new Promise((resolve) => setTimeout(resolve, 5))
    }
  }
}

function command(commandId: string) {
  return { commandId, commandType: 'fork-coordinator-test', requestHash: commandId }
}
