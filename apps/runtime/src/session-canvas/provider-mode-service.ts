import type {
  ClaudeSessionPermissionMode,
} from '@matou/contracts'
import type {
  DomainCommandMetadata,
  ProviderBinding,
  SceneSessionGraph,
  Session,
  SessionKind,
  SessionStatus,
  SessionWorkStatus
} from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'
import { projectSceneGraphFrom } from './session-graph-repository'

interface SessionRow {
  id: string
  task_id: string
  execution_context_id: string
  kind: SessionKind
  status: SessionStatus
  work_status: SessionWorkStatus
  title: string
  cwd: string
  created_at: number
  updated_at: number
  last_activity_at: number
  archived_at: number | null
  version: number
}

interface BindingRow {
  id: string
  session_id: string
  provider: ProviderBinding['provider']
  provider_session_id: string
  resume_state: ProviderBinding['resumeState']
  restore_state: 'none' | 'restoring' | 'failed'
  restore_error: string | null
  user_exited_at: number | null
  metadata_json: string
  created_at: number
  updated_at: number
  validated_at: number | null
  invalidated_at: number | null
}

interface SessionOwner {
  task_id: string
  workspace_id: string
  scene_id: string
}

export interface ProviderModeTransitionResult {
  session: Session
  binding: ProviderBinding
  graph: SceneSessionGraph
}

export class ProviderModeService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  loadClaudeSession(
    command: DomainCommandMetadata,
    input: {
      sessionId: string
      bindingId: string
      providerSessionId: string
      title: string
      permissionMode: ClaudeSessionPermissionMode
      model?: string
      now: number
    }
  ): ProviderModeTransitionResult {
    const providerSessionId = input.providerSessionId.trim()
    const title = input.title.trim() || 'Claude'
    if (!providerSessionId) throw new Error('Provider session identity must not be empty')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const owner = requireOwner(tx, input.sessionId)
      const session = requireSession(tx, input.sessionId)
      const existing = tx.get<BindingRow & { owner_kind: SessionKind; owner_archived_at: number | null }>(
        `SELECT binding.*, owner.kind AS owner_kind, owner.archived_at AS owner_archived_at
         FROM provider_bindings AS binding
         JOIN sessions AS owner ON owner.id = binding.session_id
         WHERE binding.provider = 'claude-code' AND binding.provider_session_id = ?`,
        providerSessionId
      )
      if (
        existing && existing.session_id !== session.id && existing.owner_archived_at === null &&
        existing.owner_kind === 'claude-code' && existing.invalidated_at === null &&
        ['unknown', 'available', 'resuming', 'resumed'].includes(existing.resume_state)
      ) {
        throw new Error('该 Claude Code 会话正在另一张卡片中使用')
      }

      tx.run(
        `UPDATE provider_bindings
         SET resume_state = 'expired', restore_state = 'none', restore_error = NULL,
             invalidated_at = ?, updated_at = ?
         WHERE session_id = ? AND provider = 'claude-code' AND provider_session_id <> ?`,
        input.now, input.now, session.id, providerSessionId
      )
      const metadata = {
        ...(existing ? asMetadata(existing.metadata_json) : {}),
        permissionMode: input.permissionMode,
        ...(input.model ? { model: input.model } : {}),
        loadedFromCatalog: true,
        spawnRevision: input.now,
        canFork: false,
        observedUserPrompt: false,
        observedNormalStop: false
      }
      if (existing) {
        tx.run(
          `UPDATE provider_bindings
           SET session_id = ?, resume_state = 'available', restore_state = 'restoring',
               restore_error = NULL, user_exited_at = NULL, metadata_json = ?,
               validated_at = ?, invalidated_at = NULL, updated_at = ?
           WHERE id = ?`,
          session.id, JSON.stringify(metadata), input.now, input.now, existing.id
        )
      } else {
        tx.run(
          `INSERT INTO provider_bindings (
             id, session_id, provider, provider_session_id, resume_state, restore_state,
             restore_error, user_exited_at, metadata_json, created_at, updated_at,
             validated_at, invalidated_at
           ) VALUES (?, ?, 'claude-code', ?, 'available', 'restoring',
                     NULL, NULL, ?, ?, ?, ?, NULL)`,
          input.bindingId, session.id, providerSessionId, JSON.stringify(metadata),
          input.now, input.now, input.now
        )
      }
      tx.run(
        `UPDATE sessions SET kind = 'claude-code', title = ?, status = 'starting',
           work_status = 'starting', updated_at = ?, last_activity_at = ?, version = version + 1
         WHERE id = ?`,
        title, input.now, input.now, session.id
      )
      const binding = existing
        ? requireBinding(tx, existing.id)
        : requireBinding(tx, input.bindingId)
      const result = buildResult(tx, owner, requireSession(tx, session.id), binding)
      emitTransition(emit, command.commandId, 'session.mode-changed', owner, result, input.now)
      emitRecoveryNotification(emit, command.commandId, owner, result, input.now, 'restoring')
      return result
    }).result
  }

  markClaudeActive(
    command: DomainCommandMetadata,
    input: { sessionId: string; bindingId?: string; now: number }
  ): ProviderModeTransitionResult {
    return this.#transition(command, input.sessionId, input.now, ({ tx, session, binding }) => {
      if (input.bindingId !== undefined && binding.id !== input.bindingId) {
        throw new Error('Provider binding does not match the active Claude conversation')
      }
      tx.run(
        `UPDATE sessions SET kind = 'claude-code',
           title = CASE WHEN title = 'Shell' OR title = id THEN 'Claude' ELSE title END,
           work_status = 'idle',
           updated_at = ?, version = version + 1 WHERE id = ?`,
        input.now, session.id
      )
      tx.run(
        `UPDATE provider_bindings
         SET resume_state = 'available', restore_state = 'none', restore_error = NULL,
             user_exited_at = NULL, invalidated_at = NULL, validated_at = COALESCE(validated_at, ?),
             updated_at = ?
         WHERE id = ?`,
        input.now, input.now, binding.id
      )
    }, 'session.mode-changed', undefined, 'dismiss')
  }

  markUserExited(
    command: DomainCommandMetadata,
    input: { sessionId: string; now: number }
  ): ProviderModeTransitionResult {
    return this.#transition(command, input.sessionId, input.now, ({ tx, session, binding }) => {
      tx.run(
        `UPDATE sessions SET kind = 'shell',
           title = CASE WHEN title = 'Claude' OR title = id THEN 'Shell' ELSE title END,
           work_status = 'idle',
           updated_at = ?, last_activity_at = ?, version = version + 1 WHERE id = ?`,
        input.now, input.now, session.id
      )
      tx.run(
        `UPDATE provider_bindings
         SET resume_state = 'expired', restore_state = 'none', restore_error = NULL,
             user_exited_at = ?, invalidated_at = ?, updated_at = ?
         WHERE session_id = ? AND provider = 'claude-code'
           AND resume_state IN ('unknown', 'available', 'resuming', 'resumed')`,
        input.now, input.now, input.now, session.id
      )
      // The latest binding remains the historical conversation identity, without an error state.
      if (binding.resume_state === 'failed' || binding.resume_state === 'expired') {
        tx.run(
          `UPDATE provider_bindings
           SET restore_state = 'none', restore_error = NULL, user_exited_at = ?,
               resume_state = 'expired', invalidated_at = ?, updated_at = ?
           WHERE id = ?`,
          input.now, input.now, input.now, binding.id
        )
      }
    }, 'session.mode-changed', undefined, 'dismiss')
  }

  markRestoreFailed(
    command: DomainCommandMetadata,
    input: { sessionId: string; bindingId: string; reason: string; now: number }
  ): ProviderModeTransitionResult {
    const reason = input.reason.trim() || 'provider restore failed'
    return this.#transition(command, input.sessionId, input.now, ({ tx, session, binding }) => {
      if (binding.id !== input.bindingId) {
        const requested = tx.get<BindingRow>(
          `SELECT * FROM provider_bindings WHERE id = ? AND session_id = ?`,
          input.bindingId, input.sessionId
        )
        if (!requested) throw new Error('Provider binding does not belong to the restored Session')
        binding = requested
      }
      tx.run(
        `UPDATE sessions SET kind = 'shell',
           title = CASE WHEN title = 'Claude' OR title = id THEN 'Shell' ELSE title END,
           work_status = 'error',
           updated_at = ?, last_activity_at = ?, version = version + 1 WHERE id = ?`,
        input.now, input.now, session.id
      )
      tx.run(
        `UPDATE provider_bindings
         SET resume_state = 'failed', restore_state = 'failed', restore_error = ?,
             user_exited_at = NULL, invalidated_at = ?, updated_at = ?
         WHERE id = ?`,
        reason, input.now, input.now, binding.id
      )
    }, 'session.restore-state-changed', input.bindingId, 'failed')
  }

  retryRestore(
    command: DomainCommandMetadata,
    input: { sessionId: string; now: number }
  ): ProviderModeTransitionResult {
    return this.#transition(command, input.sessionId, input.now, ({ tx, session, binding }) => {
      if (binding.restore_state !== 'failed') {
        throw new Error('只有恢复失败的 Claude Code 会话需要重试')
      }
      tx.run(
        `UPDATE sessions SET kind = 'claude-code',
           title = CASE WHEN title = 'Shell' OR title = id THEN 'Claude' ELSE title END,
           status = 'starting',
           work_status = 'starting',
           updated_at = ?, version = version + 1 WHERE id = ?`,
        input.now, session.id
      )
      tx.run(
        `UPDATE provider_bindings
         SET resume_state = 'available', restore_state = 'restoring', restore_error = NULL,
             user_exited_at = NULL, invalidated_at = NULL, updated_at = ?
         WHERE id = ?`,
        input.now, binding.id
      )
    }, 'session.restore-state-changed', undefined, 'restoring')
  }

  observeHook(
    command: DomainCommandMetadata,
    input: {
      sessionId: string
      providerSessionId: string
      eventName: string
      now: number
    }
  ): ProviderModeTransitionResult {
    const providerSessionId = input.providerSessionId.trim()
    if (!providerSessionId) throw new Error('Provider session identity must not be empty')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const owner = requireOwner(tx, input.sessionId)
      const session = requireSession(tx, input.sessionId)
      const binding = requireRow(tx.get<BindingRow>(
        `SELECT * FROM provider_bindings
         WHERE session_id = ? AND provider = 'claude-code' AND provider_session_id = ?`,
        input.sessionId, providerSessionId
      ), 'ProviderBinding')

      // Retired hooks from a manually exited process do not reactivate a Shell node.
      if (session.kind === 'shell' && binding.user_exited_at !== null) {
        return buildResult(tx, owner, session, binding)
      }

      const wasRecovering = binding.restore_state !== 'none' || binding.restore_error !== null
      const metadata = asMetadata(binding.metadata_json)
      if (input.eventName === 'UserPromptSubmit') metadata.observedUserPrompt = true
      if (input.eventName === 'Stop') metadata.observedNormalStop = true
      // A statusline payload confirms that a resumed conversation is live, but
      // it does not represent a new prompt/Stop cycle. Preserve the previously
      // earned Fork capability until a real lifecycle hook supplies new facts.
      if (input.eventName !== 'unknown') {
        metadata.canFork = metadata.observedUserPrompt === true && metadata.observedNormalStop === true
      }
      metadata.lastHookEvent = input.eventName
      const workStatus = providerWorkStatus(input.eventName, session.work_status)
      tx.run(
        `UPDATE sessions SET kind = 'claude-code',
           title = CASE WHEN title = 'Shell' OR title = id THEN 'Claude' ELSE title END,
           work_status = ?,
           updated_at = ?, version = version + 1 WHERE id = ?`,
        workStatus, input.now, session.id
      )
      tx.run(
        `UPDATE provider_bindings
         SET resume_state = 'available', restore_state = 'none', restore_error = NULL,
             user_exited_at = NULL, metadata_json = ?, validated_at = COALESCE(validated_at, ?),
             invalidated_at = NULL, updated_at = ?
         WHERE id = ?`,
        JSON.stringify(metadata), input.now, input.now, binding.id
      )
      const result = buildResult(
        tx, owner, requireSession(tx, session.id), requireBinding(tx, binding.id)
      )
      emitTransition(emit, command.commandId, 'session.mode-changed', owner, result, input.now)
      if (wasRecovering) {
        emitRecoveryNotification(emit, command.commandId, owner, result, input.now, 'dismiss')
      }
      return result
    }).result
  }

  canFork(sessionId: string): boolean {
    const row = this.#database.get<{ kind: SessionKind; restore_state: string; metadata_json: string }>(
      `SELECT sessions.kind, binding.restore_state, binding.metadata_json
       FROM sessions
       JOIN provider_bindings AS binding ON binding.id = (
         SELECT id FROM provider_bindings
         WHERE session_id = sessions.id AND provider = 'claude-code'
         ORDER BY updated_at DESC, id DESC LIMIT 1
       )
       WHERE sessions.id = ? AND sessions.archived_at IS NULL`,
      sessionId
    )
    return row?.kind === 'claude-code' && row.restore_state === 'none' &&
      asMetadata(row.metadata_json).canFork === true
  }

  #transition(
    command: DomainCommandMetadata,
    sessionId: string,
    now: number,
    mutate: (context: {
      tx: import('../storage/database').DatabaseTransaction
      session: SessionRow
      binding: BindingRow
    }) => void,
    eventType: 'session.mode-changed' | 'session.restore-state-changed',
    preferredBindingId?: string,
    recoveryNotification?: 'failed' | 'restoring' | 'dismiss'
  ): ProviderModeTransitionResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const owner = requireOwner(tx, sessionId)
      const session = requireSession(tx, sessionId)
      let binding = preferredBindingId === undefined
        ? latestBinding(tx, sessionId)
        : requireBinding(tx, preferredBindingId)
      if (binding.session_id !== sessionId) {
        throw new Error('Provider binding does not belong to the Session')
      }
      const mutable = { tx, session, binding }
      mutate(mutable)
      binding = requireBinding(tx, binding.id)
      const result = buildResult(tx, owner, requireSession(tx, sessionId), binding)
      emitTransition(emit, command.commandId, eventType, owner, result, now)
      if (recoveryNotification) {
        emitRecoveryNotification(emit, command.commandId, owner, result, now, recoveryNotification)
      }
      return result
    }).result
  }
}

function providerWorkStatus(eventName: string, current: SessionWorkStatus): SessionWorkStatus {
  if (eventName === 'UserPromptSubmit' || eventName === 'PreToolUse') return 'running'
  if (eventName === 'PermissionRequest' || eventName === 'Notification') return 'needs-input'
  if (eventName === 'PostToolUseFailure') return 'error'
  if (eventName === 'Stop') return 'idle'
  // Statusline identity is accepted only for resume/Fork launches. Its arrival
  // means the provider has rendered an interactive conversation, so a restored
  // node must no longer remain indefinitely in the starting state.
  if (eventName === 'unknown' && current === 'starting') return 'idle'
  return current
}

function emitRecoveryNotification(
  emit: Parameters<typeof emitTransition>[0],
  commandId: string,
  owner: SessionOwner,
  result: ProviderModeTransitionResult,
  now: number,
  state: 'failed' | 'restoring' | 'dismiss'
): void {
  const replacementKey = `provider-restore:${result.session.id}`
  const event = state === 'dismiss'
    ? { operation: 'dismiss', replacementKey }
    : state === 'restoring'
      ? {
          operation: 'upsert', replacementKey,
          eventType: 'attention', title: '正在恢复 Claude Code',
          subtitle: '原会话恢复中', body: '正在尝试恢复原 Claude Code 会话',
          sound: false, cooldownKey: 'ClaudeRestore'
        }
      : {
          operation: 'upsert', replacementKey,
          eventType: 'error', title: 'Claude Code 恢复失败',
          subtitle: '已切换到 Shell',
          body: result.binding.restoreError ?? '原 Claude Code 会话恢复失败',
          sound: true, cooldownKey: 'ClaudeRestore'
        }
  emit({
    eventId: `${commandId}:agent.notification`,
    eventType: 'agent.notification',
    aggregateType: 'session',
    aggregateId: result.session.id,
    workspaceId: owner.workspace_id,
    taskId: owner.task_id,
    sessionId: result.session.id,
    payload: {
      targetSessionId: result.session.id,
      provider: 'claude-code',
      runId: `restore:${result.binding.id}`,
      event
    },
    occurredAt: now
  })
}

function buildResult(
  tx: import('../storage/database').DatabaseTransaction,
  owner: SessionOwner,
  session: SessionRow,
  binding: BindingRow
): ProviderModeTransitionResult {
  return {
    session: mapSession(session),
    binding: mapBinding(binding),
    graph: projectSceneGraphFrom(tx, owner.scene_id)
  }
}

function emitTransition(
  emit: (event: Parameters<Parameters<DomainTransactionManager['execute']>[1]>[0] extends infer C
    ? C extends { emit: infer E } ? E extends (event: infer V) => void ? V : never : never
    : never) => void,
  commandId: string,
  eventType: string,
  owner: SessionOwner,
  result: ProviderModeTransitionResult,
  now: number
): void {
  emit({
    eventId: `${commandId}:${eventType}`,
    eventType,
    aggregateType: 'session',
    aggregateId: result.session.id,
    workspaceId: owner.workspace_id,
    taskId: owner.task_id,
    sessionId: result.session.id,
    payload: result,
    occurredAt: now
  })
}

function requireOwner(
  tx: import('../storage/database').DatabaseTransaction,
  sessionId: string
): SessionOwner {
  return requireRow(tx.get<SessionOwner>(
    `SELECT sessions.task_id, tasks.workspace_id, membership.scene_id
     FROM sessions
     JOIN tasks ON tasks.id = sessions.task_id
     JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
     WHERE sessions.id = ? AND sessions.archived_at IS NULL AND tasks.archived_at IS NULL`,
    sessionId
  ), 'Session')
}

function requireSession(
  tx: import('../storage/database').DatabaseTransaction,
  sessionId: string
): SessionRow {
  return requireRow(tx.get<SessionRow>(
    'SELECT * FROM sessions WHERE id = ? AND archived_at IS NULL', sessionId
  ), 'Session')
}

function latestBinding(
  tx: import('../storage/database').DatabaseTransaction,
  sessionId: string
): BindingRow {
  return requireRow(tx.get<BindingRow>(
    `SELECT * FROM provider_bindings
     WHERE session_id = ? AND provider = 'claude-code'
     ORDER BY updated_at DESC, id DESC LIMIT 1`,
    sessionId
  ), 'ProviderBinding')
}

function requireBinding(
  tx: import('../storage/database').DatabaseTransaction,
  bindingId: string
): BindingRow {
  return requireRow(tx.get<BindingRow>(
    'SELECT * FROM provider_bindings WHERE id = ?', bindingId
  ), 'ProviderBinding')
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
    metadata: asMetadata(row.metadata_json),
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    ...(row.validated_at === null ? {} : { validatedAt: row.validated_at }),
    ...(row.invalidated_at === null ? {} : { invalidatedAt: row.invalidated_at })
  }
}

function asMetadata(value: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(value) as unknown
    return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)
      ? { ...(parsed as Record<string, unknown>) }
      : {}
  } catch {
    return {}
  }
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (row === undefined) throw new Error(`${label} does not exist`)
  return row
}
