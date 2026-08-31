import { createHash } from 'node:crypto'
import { chmod, lstat, mkdir, readdir, rename, rm, stat } from 'node:fs/promises'
import { basename, dirname, join, relative } from 'node:path'

import type { DomainCommandMetadata, DomainCommit } from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

export interface RetentionQuota {
  globalBytes: number
  perSessionBytes: number
  checkpointGenerations: number
}

export interface RetentionAction {
  kind: 'journal-segment' | 'checkpoint'
  sessionId: string
  path: string
  bytes: number
}

export interface QuotaRetentionPlan {
  id: string
  kind: 'quota'
  dryRun: true
  createdAt: number
  beforeBytes: number
  reclaimedBytes: number
  actions: RetentionAction[]
}

export interface SessionPurgePlan {
  id: string
  kind: 'session-purge'
  dryRun: true
  createdAt: number
  sessionId: string
  taskId: string
  paths: string[]
  metadataTables: string[]
  retainedWorktreePaths: string[]
}

interface Candidate extends RetentionAction {
  modifiedAt: number
  protected: boolean
}

export class RetentionManager {
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(dataRoot: string, database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#dataRoot = dataRoot
    this.#database = database
    this.#transactions = transactions
  }

  async planQuota(quota: RetentionQuota): Promise<QuotaRetentionPlan> {
    validateQuota(quota)
    const candidates = await this.#inventory(quota.checkpointGenerations)
    const beforeBytes = candidates.reduce((sum, item) => sum + item.bytes, 0)
    const selected = new Map<string, Candidate>()

    for (const candidate of candidates) {
      if (candidate.kind === 'checkpoint' && !candidate.protected) selected.set(candidate.path, candidate)
    }

    const sessions = new Set(candidates.map(({ sessionId }) => sessionId))
    for (const sessionId of sessions) {
      let retained = candidates
        .filter((item) => item.sessionId === sessionId)
        .reduce((sum, item) => sum + item.bytes, 0) - selectedBytes(selected, sessionId)
      const reclaimable = candidates
        .filter((item) => item.sessionId === sessionId && !item.protected && item.kind === 'journal-segment')
        .sort(oldestFirst)
      for (const candidate of reclaimable) {
        if (retained <= quota.perSessionBytes) break
        if (!selected.has(candidate.path)) {
          selected.set(candidate.path, candidate)
          retained -= candidate.bytes
        }
      }
    }

    let retainedGlobal = beforeBytes - selectedBytes(selected)
    for (const candidate of candidates.filter((item) => !item.protected).sort(oldestFirst)) {
      if (retainedGlobal <= quota.globalBytes) break
      if (!selected.has(candidate.path)) {
        selected.set(candidate.path, candidate)
        retainedGlobal -= candidate.bytes
      }
    }
    const actions = [...selected.values()].sort(oldestFirst).map(stripCandidate)
    const createdAt = Date.now()
    return {
      id: planId('quota', actions), kind: 'quota', dryRun: true, createdAt, beforeBytes,
      reclaimedBytes: actions.reduce((sum, item) => sum + item.bytes, 0), actions
    }
  }

  async executeQuota(
    command: DomainCommandMetadata,
    plan: QuotaRetentionPlan,
    now = Date.now()
  ): Promise<DomainCommit<{ planId: string; reclaimedBytes: number }>> {
    this.#assertPlan(plan.id, 'quota', plan.actions)
    const staged = await this.#stage(plan.id, plan.actions.map(({ path }) => path))
    try {
      const affectedSessions = [...new Set(plan.actions.map(({ sessionId }) => sessionId))]
      const commit = this.#transactions.execute(command, ({ tx, emit }) => {
        for (const action of plan.actions) {
          if (action.kind === 'checkpoint') tx.run('DELETE FROM journal_checkpoints WHERE file_path = ?', action.path)
        }
        for (const sessionId of affectedSessions) {
          const rows = tx.all<{ id: string; anchor_json: string }>(
            "SELECT id, anchor_json FROM annotations WHERE session_id = ? AND status = 'active'", sessionId
          )
          for (const row of rows) {
            const anchor = parseAnchor(row.anchor_json)
            if (anchor?.kind === 'command-output' || anchor?.kind === 'screen-capture') {
              tx.run("UPDATE annotations SET status = 'degraded', updated_at = ? WHERE id = ?", now, row.id)
            }
          }
        }
        const result = { planId: plan.id, reclaimedBytes: plan.reclaimedBytes }
        emit({
          eventId: command.commandId, eventType: 'retention.executed', aggregateType: 'retention',
          aggregateId: plan.id, payload: { ...result, affectedSessions }, occurredAt: now
        })
        return result
      })
      await this.#discardStaged(staged)
      return commit
    } catch (error) {
      await this.#restoreStaged(staged)
      throw error
    }
  }

  archiveSession(command: DomainCommandMetadata, sessionId: string, now = Date.now()): DomainCommit<{ sessionId: string }> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const session = tx.get<{ task_id: string }>('SELECT task_id FROM sessions WHERE id = ?', sessionId)
      if (!session) throw new Error(`Session ${sessionId} does not exist`)
      tx.run(
        "UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1 WHERE id = ?",
        now, now, sessionId
      )
      emit({
        eventId: command.commandId, eventType: 'session.archived', aggregateType: 'session',
        aggregateId: sessionId, taskId: session.task_id, sessionId,
        payload: { sessionId, archivedAt: now }, occurredAt: now
      })
      return { sessionId }
    })
  }

  async planSessionPurge(sessionId: string): Promise<SessionPurgePlan> {
    const session = this.#database.get<{ task_id: string; execution_context_id: string }>(
      'SELECT task_id, execution_context_id FROM sessions WHERE id = ?', sessionId
    )
    if (!session) throw new Error(`Session ${sessionId} does not exist`)
    const paths = [join(this.#dataRoot, 'journal', sessionId), join(this.#dataRoot, 'checkpoints', sessionId)]
      .filter((path) => pathWithin(this.#dataRoot, path))
    const worktree = this.#database.get<{ worktree_path: string }>(
      'SELECT worktree_path FROM worktrees WHERE execution_context_id = ?', session.execution_context_id
    )
    const metadataTables = [
      'annotations', 'terminal_commands', 'shell_history_blocks', 'journal_checkpoints', 'session_mounts',
      'session_relations_current', 'session_relation_events', 'provider_bindings',
      'session_runs', 'domain_events', 'sessions'
    ]
    const createdAt = Date.now()
    return {
      id: planId('session-purge', { sessionId, paths, metadataTables }), kind: 'session-purge', dryRun: true,
      createdAt, sessionId, taskId: session.task_id, paths, metadataTables,
      retainedWorktreePaths: worktree ? [worktree.worktree_path] : []
    }
  }

  async executeSessionPurge(
    command: DomainCommandMetadata,
    plan: SessionPurgePlan,
    now = Date.now()
  ): Promise<DomainCommit<{ sessionId: string; planId: string }>> {
    this.#assertPlan(plan.id, 'session-purge', {
      sessionId: plan.sessionId, paths: plan.paths, metadataTables: plan.metadataTables
    })
    const staged = await this.#stage(plan.id, plan.paths)
    try {
      const result = this.#transactions.execute(command, ({ tx, emit }) => {
        if (!tx.get('SELECT id FROM sessions WHERE id = ?', plan.sessionId)) {
          throw new Error(`Session ${plan.sessionId} does not exist`)
        }
        tx.run('DELETE FROM annotations WHERE session_id = ?', plan.sessionId)
        tx.run('DELETE FROM terminal_commands WHERE session_id = ?', plan.sessionId)
        tx.run('DELETE FROM shell_history_blocks WHERE session_id = ?', plan.sessionId)
        tx.run('DELETE FROM journal_checkpoints WHERE session_id = ?', plan.sessionId)
        tx.run('DELETE FROM session_mounts WHERE session_id = ?', plan.sessionId)
        tx.run('DELETE FROM session_relations_current WHERE from_session_id = ? OR to_session_id = ?', plan.sessionId, plan.sessionId)
        tx.run('DELETE FROM session_relation_events WHERE from_session_id = ? OR to_session_id = ?', plan.sessionId, plan.sessionId)
        tx.run('DELETE FROM provider_bindings WHERE session_id = ?', plan.sessionId)
        tx.run('DELETE FROM session_runs WHERE session_id = ?', plan.sessionId)
        tx.run('UPDATE artifacts SET producer_session_id = NULL WHERE producer_session_id = ?', plan.sessionId)
        tx.run('UPDATE validation_runs SET session_id = NULL WHERE session_id = ?', plan.sessionId)
        tx.run('DELETE FROM domain_events WHERE session_id = ?', plan.sessionId)
        tx.run('DELETE FROM sessions WHERE id = ?', plan.sessionId)
        const payload = { sessionId: plan.sessionId, planId: plan.id }
        emit({
          eventId: command.commandId, eventType: 'retention.session-purged', aggregateType: 'retention',
          aggregateId: plan.id, taskId: plan.taskId,
          payload: { ...payload, retainedWorktreePaths: plan.retainedWorktreePaths }, occurredAt: now
        })
        return payload
      })
      await this.#discardStaged(staged)
      return result
    } catch (error) {
      await this.#restoreStaged(staged)
      throw error
    }
  }

  async hardenPrivacyPermissions(): Promise<void> {
    for (const name of ['journal', 'checkpoints', 'control']) {
      const root = join(this.#dataRoot, name)
      try {
        await chmod(root, 0o700)
        await hardenTree(root)
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
      }
    }
  }

  async #inventory(checkpointGenerations: number): Promise<Candidate[]> {
    const candidates: Candidate[] = []
    const journalRoot = join(this.#dataRoot, 'journal')
    for (const sessionId of await safeReadDir(journalRoot)) {
      const sessionRoot = join(journalRoot, sessionId)
      for (const entry of await safeReadDir(sessionRoot)) {
        if (!/^segment-\d{6}\.bin(?:\.gz)?$/.test(entry)) continue
        const info = await stat(join(sessionRoot, entry))
        candidates.push({
          kind: 'journal-segment', sessionId, path: join(sessionRoot, entry), bytes: info.size,
          modifiedAt: info.mtimeMs, protected: entry.endsWith('.bin')
        })
      }
    }
    const checkpointRows = this.#database.all<{
      session_id: string; generation: number; file_path: string; created_at: number
    }>('SELECT session_id, generation, file_path, created_at FROM journal_checkpoints ORDER BY session_id, generation DESC')
    const seen = new Map<string, number>()
    for (const row of checkpointRows) {
      try {
        const info = await stat(row.file_path)
        const ordinal = seen.get(row.session_id) ?? 0
        seen.set(row.session_id, ordinal + 1)
        candidates.push({
          kind: 'checkpoint', sessionId: row.session_id, path: row.file_path, bytes: info.size,
          modifiedAt: row.created_at, protected: ordinal < checkpointGenerations
        })
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
      }
    }
    return candidates
  }

  #assertPlan(id: string, kind: string, payload: unknown): void {
    if (id !== planId(kind, payload)) throw new Error('Retention plan integrity check failed')
  }

  async #stage(plan: string, paths: string[]): Promise<Array<{ original: string; staged: string }>> {
    const trash = join(this.#dataRoot, '.trash', plan)
    await mkdir(trash, { recursive: true, mode: 0o700 })
    const staged: Array<{ original: string; staged: string }> = []
    for (const original of paths) {
      if (!pathWithin(this.#dataRoot, original)) throw new Error('Retention path escapes data root')
      try {
        await lstat(original)
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue
        throw error
      }
      const target = join(trash, `${staged.length}-${basename(original)}`)
      await rename(original, target)
      staged.push({ original, staged: target })
    }
    return staged
  }

  async #restoreStaged(staged: Array<{ original: string; staged: string }>): Promise<void> {
    for (const item of staged.reverse()) {
      await mkdir(dirname(item.original), { recursive: true })
      await rename(item.staged, item.original)
    }
  }

  async #discardStaged(staged: Array<{ staged: string }>): Promise<void> {
    for (const item of staged) await rm(item.staged, { recursive: true, force: true })
  }
}

function validateQuota(quota: RetentionQuota): void {
  if (
    !Number.isSafeInteger(quota.globalBytes) || quota.globalBytes <= 0 ||
    !Number.isSafeInteger(quota.perSessionBytes) || quota.perSessionBytes <= 0 ||
    quota.perSessionBytes > quota.globalBytes ||
    !Number.isSafeInteger(quota.checkpointGenerations) || quota.checkpointGenerations < 2
  ) throw new Error('Invalid retention quota')
}

function planId(kind: string, payload: unknown): string {
  return createHash('sha256').update(kind).update('\0').update(stableJson(payload)).digest('hex')
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`
  if (value && typeof value === 'object') {
    return `{${Object.entries(value as Record<string, unknown>).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => `${JSON.stringify(key)}:${stableJson(item)}`).join(',')}}`
  }
  return JSON.stringify(value)
}

function selectedBytes(selected: Map<string, Candidate>, sessionId?: string): number {
  return [...selected.values()].filter((item) => sessionId === undefined || item.sessionId === sessionId)
    .reduce((sum, item) => sum + item.bytes, 0)
}

function oldestFirst(a: Candidate, b: Candidate): number {
  return a.modifiedAt - b.modifiedAt || a.path.localeCompare(b.path)
}

function stripCandidate({ kind, sessionId, path, bytes }: Candidate): RetentionAction {
  return { kind, sessionId, path, bytes }
}

function parseAnchor(json: string): { kind?: string } | undefined {
  try { return JSON.parse(json) as { kind?: string } } catch { return undefined }
}

function pathWithin(root: string, path: string): boolean {
  const result = relative(root, path)
  return result !== '..' && !result.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) && !result.startsWith('/')
}

async function safeReadDir(path: string): Promise<string[]> {
  try { return await readdir(path) } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return []
    throw error
  }
}

async function hardenTree(root: string): Promise<void> {
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name)
    if (entry.isDirectory()) {
      await chmod(path, 0o700)
      await hardenTree(path)
    } else if (entry.isFile()) {
      await chmod(path, 0o600)
    }
  }
}
