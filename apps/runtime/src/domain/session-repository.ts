import type {
  DomainCommandMetadata,
  DomainCommit,
  ProviderBinding,
  Session,
  SessionKind,
  SessionRun,
  SessionRunStatus,
  SessionStatus
} from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

export interface ProviderIdentityForkAuthority {
  operationId: string
  runId: string
  lease: { token: string; fence: number }
}

export class StaleForkProviderIdentityError extends Error {
  constructor(operationId: string) {
    super(`Fork provider identity authority is stale for operation ${operationId}`)
    this.name = 'StaleForkProviderIdentityError'
  }
}

interface SessionRow {
  id: string
  task_id: string
  execution_context_id: string
  kind: SessionKind
  status: SessionStatus
  title: string
  cwd: string
  created_at: number
  updated_at: number
  last_activity_at: number
  archived_at: number | null
  version: number
}

interface RunRow {
  id: string
  session_id: string
  ordinal: number
  runtime_generation: string
  profile: 'shell' | 'claude-code' | 'codex'
  pid: number | null
  status: SessionRunStatus
  cols: number
  rows: number
  started_at: number
  ended_at: number | null
  exit_code: number | null
  signal: number | null
}

interface BindingRow {
  id: string
  session_id: string
  provider: 'claude-code' | 'codex' | 'generic'
  provider_session_id: string
  resume_state: ProviderBinding['resumeState']
  metadata_json: string
  created_at: number
  updated_at: number
  validated_at: number | null
  invalidated_at: number | null
  restore_state: 'none' | 'restoring' | 'failed'
  restore_error: string | null
  user_exited_at: number | null
}

export type ProviderPermissionMode =
  | 'default'
  | 'acceptEdits'
  | 'plan'
  | 'bypassPermissions'

export class SessionRepository {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  createSession(
    command: DomainCommandMetadata,
    input: {
      id: string
      taskId: string
      executionContextId: string
      kind: SessionKind
      title: string
      now: number
    }
  ): DomainCommit<Session> {
    const title = input.title.trim()
    if (!title) throw new Error('Session title must not be empty')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const task = tx.get<{ workspace_id: string; execution_context_id: string }>(
        'SELECT workspace_id, execution_context_id FROM tasks WHERE id = ? AND archived_at IS NULL',
        input.taskId
      )
      if (!task) throw new Error(`Task ${input.taskId} does not exist or is archived`)
      const context = tx.get<{ workspace_id: string }>(
        'SELECT workspace_id FROM execution_contexts WHERE id = ? AND archived_at IS NULL',
        input.executionContextId
      )
      if (!context || context.workspace_id !== task.workspace_id) {
        throw new Error('Session execution context must belong to the Task Workspace')
      }
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, ?, 'created', ?, ?, ?, ?, 1)`,
        input.id,
        input.taskId,
        input.executionContextId,
        input.kind,
        title,
        input.now,
        input.now,
        input.now
      )
      const session = mapSession(requireRow(tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', input.id), 'Session'))
      emit({
        eventId: `${command.commandId}:session-created`,
        eventType: 'session.created',
        aggregateType: 'session',
        aggregateId: input.id,
        workspaceId: task.workspace_id,
        taskId: input.taskId,
        sessionId: input.id,
        payload: session,
        occurredAt: input.now
      })
      return session
    })
  }

  startRun(
    command: DomainCommandMetadata,
    input: {
      id: string
      sessionId: string
      runtimeGeneration: string
      profile: 'shell' | 'claude-code' | 'codex'
      pid?: number
      cols: number
      rows: number
      now: number
    }
  ): DomainCommit<SessionRun> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const session = requireRow(tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', input.sessionId), 'Session')
      if (session.archived_at !== null) throw new Error('archived Session cannot start a new run')
      const ordinal =
        (tx.get<{ maximum: number }>(
          'SELECT COALESCE(MAX(ordinal), 0) AS maximum FROM session_runs WHERE session_id = ?',
          input.sessionId
        )?.maximum ?? 0) + 1
      tx.run(
        `INSERT INTO session_runs (
           id, session_id, ordinal, runtime_generation, profile, pid,
           status, cols, rows, started_at
         ) VALUES (?, ?, ?, ?, ?, ?, 'running', ?, ?, ?)`,
        input.id,
        input.sessionId,
        ordinal,
        input.runtimeGeneration,
        input.profile,
        input.pid ?? null,
        input.cols,
        input.rows,
        input.now
      )
      tx.run(
        `UPDATE sessions
         SET status = 'running', work_status = ?, updated_at = ?, last_activity_at = ?,
             version = version + 1
         WHERE id = ?`,
        input.profile === 'shell' ? 'idle' : 'starting',
        input.now,
        input.now,
        input.sessionId
      )
      const run = mapRun(requireRow(tx.get<RunRow>('SELECT * FROM session_runs WHERE id = ?', input.id), 'SessionRun'))
      emitSessionEvent(emit, command.commandId, 'session.run-started', session, input.now, { run })
      return run
    })
  }

  finishRun(
    command: DomainCommandMetadata,
    runId: string,
    input: { exitCode: number; signal?: number; now: number }
  ): DomainCommit<SessionRun> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<RunRow>('SELECT * FROM session_runs WHERE id = ?', runId), 'SessionRun')
      const session = requireRow(tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', before.session_id), 'Session')
      tx.run(
        `UPDATE session_runs
         SET status = 'exited', ended_at = ?, exit_code = ?, signal = ?
         WHERE id = ?`,
        input.now,
        input.exitCode,
        input.signal ?? null,
        runId
      )
      tx.run(
        `UPDATE sessions
         SET status = 'exited', work_status = 'exited', updated_at = ?,
             last_activity_at = ?, version = version + 1
         WHERE id = ?`,
        input.now,
        input.now,
        before.session_id
      )
      const run = mapRun(requireRow(tx.get<RunRow>('SELECT * FROM session_runs WHERE id = ?', runId), 'SessionRun'))
      emitSessionEvent(emit, command.commandId, 'session.run-exited', session, input.now, { run })
      return run
    })
  }

  interruptRun(
    command: DomainCommandMetadata,
    runId: string,
    now: number
  ): DomainCommit<SessionRun> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<RunRow>('SELECT * FROM session_runs WHERE id = ?', runId), 'SessionRun')
      const session = requireRow(tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', before.session_id), 'Session')
      if (before.status !== 'starting' && before.status !== 'running') {
        throw new Error('only an active SessionRun can be interrupted')
      }
      tx.run(
        `UPDATE session_runs SET status = 'interrupted', ended_at = ? WHERE id = ?`,
        now, runId
      )
      tx.run(
        `UPDATE sessions SET status = 'interrupted', work_status = 'interrupted',
         updated_at = ?, last_activity_at = ?,
         version = version + 1 WHERE id = ?`,
        now, now, before.session_id
      )
      const run = mapRun(requireRow(tx.get<RunRow>('SELECT * FROM session_runs WHERE id = ?', runId), 'SessionRun'))
      emitSessionEvent(emit, command.commandId, 'session.run-interrupted', session, now, { run })
      return run
    })
  }

  bindProvider(
    command: DomainCommandMetadata,
    input: {
      id: string
      sessionId: string
      provider: ProviderBinding['provider']
      providerSessionId: string
      metadata: unknown
      now: number
    }
  ): DomainCommit<ProviderBinding> {
    const providerSessionId = input.providerSessionId.trim()
    if (!providerSessionId) throw new Error('Provider session identity must not be empty')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const session = requireRow(tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', input.sessionId), 'Session')
      tx.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state,
           metadata_json, created_at, updated_at
         ) VALUES (?, ?, ?, ?, 'unknown', ?, ?, ?)`,
        input.id,
        input.sessionId,
        input.provider,
        providerSessionId,
        JSON.stringify(input.metadata),
        input.now,
        input.now
      )
      const binding = mapBinding(requireRow(tx.get<BindingRow>('SELECT * FROM provider_bindings WHERE id = ?', input.id), 'ProviderBinding'))
      emitSessionEvent(emit, command.commandId, 'provider-binding.created', session, input.now, { binding })
      return binding
    })
  }

  recordResumableProviderIdentity(
    command: DomainCommandMetadata,
    input: {
      id: string
      sessionId: string
      provider: ProviderBinding['provider']
      providerSessionId: string
      metadata: unknown
      provisional?: boolean
      now: number
      forkAuthority?: ProviderIdentityForkAuthority
    }
  ): DomainCommit<ProviderBinding> {
    const providerSessionId = input.providerSessionId.trim()
    if (!providerSessionId) throw new Error('Provider session identity must not be empty')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const session = requireRow(
        tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', input.sessionId),
        'Session'
      )
      if (session.archived_at !== null) {
        throw new Error('archived Session cannot record a provider identity')
      }
      if (input.forkAuthority) {
        const authoritative = tx.get<{ operation_id: string }>(
          `SELECT operation_id FROM session_fork_intents
           WHERE operation_id = ? AND session_id = ?
             AND state IN ('pending', 'starting') AND stage = 'restoring-provider'
             AND lease_token = ? AND lease_fence = ?
             AND lease_expires_at IS NOT NULL AND lease_expires_at > ?`,
          input.forkAuthority.operationId,
          input.sessionId,
          input.forkAuthority.lease.token,
          input.forkAuthority.lease.fence,
          input.now
        )
        if (!authoritative) {
          throw new StaleForkProviderIdentityError(input.forkAuthority.operationId)
        }
      }
      const authorityMetadata: Record<string, unknown> = input.forkAuthority === undefined ? {} : {
        forkOperationId: input.forkAuthority.operationId,
        forkRunId: input.forkAuthority.runId,
        forkLeaseFence: input.forkAuthority.lease.fence
      }
      const existing = tx.get<BindingRow>(
        `SELECT * FROM provider_bindings
         WHERE session_id = ? AND provider = ? AND provider_session_id = ?`,
        input.sessionId,
        input.provider,
        providerSessionId
      )
      if (existing) {
        const previousMetadata = parseMetadata(existing.metadata_json)
        const metadata: Record<string, unknown> = {
          ...(isObject(previousMetadata) ? previousMetadata : {}),
          ...(isObject(input.metadata) ? input.metadata : {}),
          ...authorityMetadata
        }
        delete metadata.invalidationReason
        if (input.provisional) metadata.provisional = true
        else delete metadata.provisional
        tx.run(
          `UPDATE provider_bindings
           SET resume_state = ?, metadata_json = ?, updated_at = ?,
               validated_at = ?, invalidated_at = NULL
           WHERE id = ?`,
          input.provisional ? 'unknown' : 'available',
          JSON.stringify(metadata),
          input.now,
          input.provisional ? null : input.now,
          existing.id
        )
      } else {
        const metadata: Record<string, unknown> = {
          ...(isObject(input.metadata) ? input.metadata : {}),
          ...authorityMetadata,
          ...(input.provisional ? { provisional: true } : {})
        }
        tx.run(
          `INSERT INTO provider_bindings (
             id, session_id, provider, provider_session_id, resume_state,
             metadata_json, created_at, updated_at, validated_at, invalidated_at
           ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)`,
          input.id,
          input.sessionId,
          input.provider,
          providerSessionId,
          input.provisional ? 'unknown' : 'available',
          JSON.stringify(metadata),
          input.now,
          input.now,
          input.provisional ? null : input.now
        )
      }
      const binding = mapBinding(requireRow(
        tx.get<BindingRow>(
          `SELECT * FROM provider_bindings
           WHERE session_id = ? AND provider = ? AND provider_session_id = ?`,
          input.sessionId,
          input.provider,
          providerSessionId
        ),
        'ProviderBinding'
      ))
      if (!input.provisional) {
        if (input.forkAuthority) {
          const startingWindow = tx.run(
            `UPDATE session_fork_intents
             SET state = 'starting', stage = 'starting-window',
                 completed_steps = MAX(completed_steps, total_steps - 1), updated_at = ?
             WHERE operation_id = ? AND session_id = ? AND stage = 'restoring-provider'
               AND lease_token = ? AND lease_fence = ?
               AND lease_expires_at IS NOT NULL AND lease_expires_at > ?`,
            input.now,
            input.forkAuthority.operationId,
            input.sessionId,
            input.forkAuthority.lease.token,
            input.forkAuthority.lease.fence,
            input.now
          )
          if (startingWindow.changes !== 1) {
            throw new StaleForkProviderIdentityError(input.forkAuthority.operationId)
          }
          const completed = tx.run(
            `UPDATE session_fork_intents
             SET state = 'succeeded', stage = 'succeeded', completed_steps = total_steps,
                 error_message = NULL, completed_at = ?, updated_at = ?
             WHERE operation_id = ? AND session_id = ? AND stage = 'starting-window'
               AND lease_token = ? AND lease_fence = ?
               AND lease_expires_at IS NOT NULL AND lease_expires_at > ?`,
            input.now,
            input.now,
            input.forkAuthority.operationId,
            input.sessionId,
            input.forkAuthority.lease.token,
            input.forkAuthority.lease.fence,
            input.now
          )
          if (completed.changes !== 1) {
            throw new StaleForkProviderIdentityError(input.forkAuthority.operationId)
          }
        } else tx.run(
          `UPDATE session_fork_intents
           SET state = 'succeeded', stage = 'succeeded',
               completed_steps = total_steps, error_message = NULL, completed_at = ?, updated_at = ?
           WHERE session_id = ? AND state IN ('pending', 'starting')
             AND (operation_id = '' OR operation_id LIKE 'legacy-operation:%')
             AND (lease_token IS NULL OR lease_expires_at IS NULL OR lease_expires_at <= ?)`,
          input.now,
          input.now,
          input.sessionId,
          input.now
        )
      }
      emitSessionEvent(
        emit,
        command.commandId,
        'provider-binding.recorded',
        session,
        input.now,
        { binding }
      )
      return binding
    })
  }

  updateProviderPermissionMode(
    command: DomainCommandMetadata,
    input: {
      sessionId: string
      provider: ProviderBinding['provider']
      permissionMode: ProviderPermissionMode
      now: number
    }
  ): DomainCommit<ProviderBinding> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const session = requireRow(
        tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', input.sessionId),
        'Session'
      )
      if (session.archived_at !== null) {
        throw new Error('archived Session permission mode cannot be modified')
      }
      const binding = requireRow(
        tx.get<BindingRow>(
          `SELECT * FROM provider_bindings
           WHERE session_id = ? AND provider = ?
             AND resume_state IN ('available', 'resumed')
             AND validated_at IS NOT NULL AND invalidated_at IS NULL
           ORDER BY validated_at DESC LIMIT 1`,
          input.sessionId,
          input.provider
        ),
        'resumable ProviderBinding'
      )
      const metadata = parseMetadata(binding.metadata_json)
      tx.run(
        `UPDATE provider_bindings
         SET metadata_json = ?, updated_at = ?
         WHERE id = ?`,
        JSON.stringify({
          ...(isObject(metadata) ? metadata : {}),
          permissionMode: input.permissionMode
        }),
        input.now,
        binding.id
      )
      const updated = mapBinding(requireRow(
        tx.get<BindingRow>('SELECT * FROM provider_bindings WHERE id = ?', binding.id),
        'ProviderBinding'
      ))
      emitSessionEvent(
        emit,
        command.commandId,
        'provider-binding.permission-mode-updated',
        session,
        input.now,
        { binding: updated, permissionMode: input.permissionMode }
      )
      return updated
    })
  }

  updateSession(
    command: DomainCommandMetadata,
    input: {
      id: string
      title?: string
      status?: Exclude<SessionStatus, 'archived'>
      now: number
    }
  ): DomainCommit<Session> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', input.id), 'Session')
      if (before.archived_at !== null) throw new Error('archived Session cannot be modified')
      const title = input.title === undefined ? before.title : input.title.trim()
      if (!title) throw new Error('Session title must not be empty')
      const status = input.status ?? before.status
      tx.run(
        `UPDATE sessions SET title = ?, status = ?, updated_at = ?,
         last_activity_at = ?, version = version + 1 WHERE id = ?`,
        title, status, input.now, input.now, input.id
      )
      const session = mapSession({
        ...before, title, status, updated_at: input.now,
        last_activity_at: input.now, version: before.version + 1
      })
      emitSessionEvent(emit, command.commandId, 'session.updated', before, input.now, { session })
      return session
    })
  }

  updateCwd(
    command: DomainCommandMetadata,
    sessionId: string,
    cwd: string,
    now: number
  ): DomainCommit<Session> {
    const normalized = cwd.trim()
    if (!normalized) throw new Error('Session cwd must not be empty')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<SessionRow>(
        'SELECT * FROM sessions WHERE id = ?', sessionId
      ), 'Session')
      if (before.archived_at !== null) throw new Error('archived Session cwd cannot be modified')
      tx.run(
        `UPDATE sessions SET cwd = ?, updated_at = ?, last_activity_at = ?,
         version = version + 1 WHERE id = ?`,
        normalized, now, now, sessionId
      )
      const session = mapSession({
        ...before, cwd: normalized, updated_at: now,
        last_activity_at: now, version: before.version + 1
      })
      emitSessionEvent(emit, command.commandId, 'session.cwd-updated', before, now, {
        cwd: normalized
      })
      return session
    })
  }

  archiveSession(
    command: DomainCommandMetadata,
    sessionId: string,
    now: number
  ): DomainCommit<Session> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', sessionId), 'Session')
      if (tx.get(
        `SELECT id FROM session_runs WHERE session_id = ? AND status IN ('starting', 'running') LIMIT 1`,
        sessionId
      )) {
        throw new Error('active SessionRuns must be stopped before archiving a Session')
      }
      tx.run(
        `UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?,
         version = version + 1 WHERE id = ?`,
        now, now, sessionId
      )
      const session = mapSession({
        ...before, status: 'archived', archived_at: now,
        updated_at: now, version: before.version + 1
      })
      emitSessionEvent(emit, command.commandId, 'session.archived', before, now, { archivedAt: now })
      return session
    })
  }

  validateProviderBinding(
    command: DomainCommandMetadata,
    bindingId: string,
    now: number
  ): DomainCommit<ProviderBinding> {
    return this.#changeBinding(command, bindingId, 'available', now, undefined)
  }

  invalidateProviderBinding(
    command: DomainCommandMetadata,
    bindingId: string,
    reason: string,
    now: number
  ): DomainCommit<ProviderBinding> {
    return this.#changeBinding(command, bindingId, 'failed', now, reason)
  }

  failResumeToShell(
    command: DomainCommandMetadata,
    sessionId: string,
    bindingId: string,
    reason: string,
    now: number
  ): DomainCommit<Session> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(
        tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', sessionId),
        'Session'
      )
      const binding = requireRow(
        tx.get<BindingRow>('SELECT * FROM provider_bindings WHERE id = ?', bindingId),
        'ProviderBinding'
      )
      if (binding.session_id !== sessionId) {
        throw new Error('ProviderBinding must belong to the failed Session')
      }
      const metadata = parseMetadata(binding.metadata_json)
      tx.run(
        `UPDATE provider_bindings
         SET resume_state = 'failed', metadata_json = ?, updated_at = ?, invalidated_at = ?
         WHERE id = ?`,
        JSON.stringify({
          ...(isObject(metadata) ? metadata : {}),
          invalidationReason: reason
        }),
        now,
        now,
        bindingId
      )
      tx.run(
        `UPDATE sessions
         SET kind = 'shell',
             title = CASE WHEN title IN ('Claude', 'Codex') OR title = id THEN 'Shell' ELSE title END,
             work_status = 'error',
             updated_at = ?, last_activity_at = ?,
             version = version + 1
         WHERE id = ?`,
        now,
        now,
        sessionId
      )
      const session = mapSession({
        ...before,
        kind: 'shell',
        title: genericShellTitle(before.title, before.id),
        updated_at: now,
        last_activity_at: now,
        version: before.version + 1
      })
      emitSessionEvent(emit, command.commandId, 'session.resume-failed', before, now, {
        session,
        bindingId,
        reason
      })
      return session
    })
  }

  returnAgentToShell(
    command: DomainCommandMetadata,
    sessionId: string,
    now: number
  ): DomainCommit<Session> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(
        tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', sessionId),
        'Session'
      )
      if (before.archived_at !== null) throw new Error('archived Session cannot return to Shell')
      tx.run(
        `UPDATE sessions
         SET kind = 'shell',
             title = CASE WHEN title IN ('Claude', 'Codex') OR title = id THEN 'Shell' ELSE title END,
             work_status = 'idle',
             updated_at = ?, last_activity_at = ?,
             version = version + 1
         WHERE id = ?`,
        now,
        now,
        sessionId
      )
      const session = mapSession({
        ...before,
        kind: 'shell',
        title: genericShellTitle(before.title, before.id),
        updated_at: now,
        last_activity_at: now,
        version: before.version + 1
      })
      emitSessionEvent(emit, command.commandId, 'session.updated', before, now, { session })
      return session
    })
  }

  promoteShellToAgent(
    command: DomainCommandMetadata,
    sessionId: string,
    kind: 'claude-code' | 'codex',
    now: number
  ): DomainCommit<Session> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(
        tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', sessionId),
        'Session'
      )
      if (before.archived_at !== null) throw new Error('archived Session cannot start an Agent')
      const title = before.title === 'Shell' || before.title === before.id
        ? (kind === 'claude-code' ? 'Claude' : 'Codex')
        : before.title
      tx.run(
        `UPDATE sessions
         SET kind = ?, title = CASE WHEN title = 'Shell' OR title = id THEN ? ELSE title END,
             work_status = 'starting', updated_at = ?,
             last_activity_at = ?, version = version + 1
         WHERE id = ?`,
        kind,
        title,
        now,
        now,
        sessionId
      )
      const session = mapSession({
        ...before,
        kind,
        title,
        updated_at: now,
        last_activity_at: now,
        version: before.version + 1
      })
      emitSessionEvent(emit, command.commandId, 'session.updated', before, now, { session })
      return session
    })
  }

  getSession(id: string): Session | undefined {
    const row = this.#database.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', id)
    return row ? mapSession(row) : undefined
  }

  listRuns(sessionId: string): SessionRun[] {
    return this.#database
      .all<RunRow>('SELECT * FROM session_runs WHERE session_id = ? ORDER BY ordinal', sessionId)
      .map(mapRun)
  }

  listProviderBindings(sessionId: string): ProviderBinding[] {
    return this.#database
      .all<BindingRow>('SELECT * FROM provider_bindings WHERE session_id = ? ORDER BY created_at', sessionId)
      .map(mapBinding)
  }

  getResumeBinding(
    sessionId: string,
    provider: ProviderBinding['provider']
  ): ProviderBinding | undefined {
    const row = this.#database.get<BindingRow>(
      `SELECT * FROM provider_bindings
       WHERE session_id = ? AND provider = ?
         AND resume_state IN ('available', 'resumed')
         AND validated_at IS NOT NULL AND invalidated_at IS NULL
       ORDER BY validated_at DESC LIMIT 1`,
      sessionId,
      provider
    )
    return row ? mapBinding(row) : undefined
  }

  #changeBinding(
    command: DomainCommandMetadata,
    bindingId: string,
    state: 'available' | 'failed',
    now: number,
    reason: string | undefined
  ): DomainCommit<ProviderBinding> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<BindingRow>('SELECT * FROM provider_bindings WHERE id = ?', bindingId), 'ProviderBinding')
      const session = requireRow(tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ?', before.session_id), 'Session')
      const metadata = parseMetadata(before.metadata_json)
      const nextMetadata = reason === undefined ? metadata : { ...(isObject(metadata) ? metadata : {}), invalidationReason: reason }
      tx.run(
        `UPDATE provider_bindings
         SET resume_state = ?, metadata_json = ?, updated_at = ?,
             validated_at = CASE WHEN ? = 'available' THEN ? ELSE validated_at END,
             invalidated_at = CASE WHEN ? = 'failed' THEN ? ELSE NULL END
         WHERE id = ?`,
        state,
        JSON.stringify(nextMetadata),
        now,
        state,
        now,
        state,
        now,
        bindingId
      )
      const binding = mapBinding(requireRow(tx.get<BindingRow>('SELECT * FROM provider_bindings WHERE id = ?', bindingId), 'ProviderBinding'))
      emitSessionEvent(
        emit,
        command.commandId,
        state === 'available' ? 'provider-binding.validated' : 'provider-binding.invalidated',
        session,
        now,
        { binding, ...(reason === undefined ? {} : { reason }) }
      )
      return binding
    })
  }
}

function emitSessionEvent(
  emit: Parameters<Parameters<DomainTransactionManager['execute']>[1]>[0]['emit'],
  eventId: string,
  eventType: string,
  session: SessionRow,
  occurredAt: number,
  payload: unknown
): void {
  emit({
    eventId,
    eventType,
    aggregateType: 'session',
    aggregateId: session.id,
    taskId: session.task_id,
    sessionId: session.id,
    payload,
    occurredAt
  })
}

function mapSession(row: SessionRow): Session {
  return {
    id: row.id,
    taskId: row.task_id,
    executionContextId: row.execution_context_id,
    kind: row.kind,
    status: row.status,
    title: row.title,
    cwd: row.cwd,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    lastActivityAt: row.last_activity_at,
    ...(row.archived_at === null ? {} : { archivedAt: row.archived_at }),
    version: row.version
  }
}

function genericShellTitle(title: string, sessionId: string): string {
  return title === 'Claude' || title === 'Codex' || title === sessionId ? 'Shell' : title
}

function mapRun(row: RunRow): SessionRun {
  return {
    id: row.id,
    sessionId: row.session_id,
    ordinal: row.ordinal,
    runtimeGeneration: row.runtime_generation,
    profile: row.profile,
    ...(row.pid === null ? {} : { pid: row.pid }),
    status: row.status,
    cols: row.cols,
    rows: row.rows,
    startedAt: row.started_at,
    ...(row.ended_at === null ? {} : { endedAt: row.ended_at }),
    ...(row.exit_code === null ? {} : { exitCode: row.exit_code }),
    ...(row.signal === null ? {} : { signal: row.signal })
  }
}

function mapBinding(row: BindingRow): ProviderBinding {
  return {
    id: row.id,
    sessionId: row.session_id,
    provider: row.provider,
    providerSessionId: row.provider_session_id,
    resumeState: row.resume_state,
    restoreState: row.restore_state,
    ...(row.restore_error === null ? {} : { restoreError: row.restore_error }),
    ...(row.user_exited_at === null ? {} : { userExitedAt: row.user_exited_at }),
    metadata: parseMetadata(row.metadata_json),
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    ...(row.validated_at === null ? {} : { validatedAt: row.validated_at }),
    ...(row.invalidated_at === null ? {} : { invalidatedAt: row.invalidated_at })
  }
}

function parseMetadata(value: string): unknown {
  try {
    return JSON.parse(value) as unknown
  } catch {
    return {}
  }
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (!row) throw new Error(`${label} does not exist`)
  return row
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
