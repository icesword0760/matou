import type { ForkStage } from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { RuntimeSessionRegistry } from './runtime-session-registry'
import type { ForkLease } from './session-fork-intent-repository'

export interface SessionExecutionDescriptor {
  sessionId: string
  executionContextId: string
  profile: 'shell' | 'claude-code' | 'codex'
  cols: number
  rows: number
  spawnRevision?: number | undefined
}

export interface ForkExecutionAuthorityInput {
  operationId: string
  runId: string
  lease: Pick<ForkLease, 'token' | 'fence'>
}

export interface ForkExecutionAuthority extends ForkExecutionAuthorityInput {
  sourceSessionId: string
  sourceProviderSessionId: string
}

export interface SessionExecutionBackend<T = void> {
  startOrResume(
    descriptor: SessionExecutionDescriptor,
    authority?: ForkExecutionAuthority,
    mode?: 'attach-only'
  ): Promise<T>
}

export type SessionExecutionResult<T> =
  | { kind: 'started'; value: T; authority?: ForkExecutionAuthority }
  | { kind: 'deferred'; operationId: string; stage: ForkStage }
  | { kind: 'stale-authority'; operationId: string }

interface DurableForkRow {
  operation_id: string
  session_id: string
  source_session_id: string
  source_provider_session_id: string
  stage: ForkStage
  lease_token: string | null
  lease_fence: number
  lease_expires_at: number | null
}

/**
 * Runtime-generation authority for starting or reattaching a logical Session.
 *
 * The backend owns the existing PTY/provider launch implementation. Keeping the
 * authority and per-Session serialization here lets a background coordinator
 * use that exact implementation without introducing a second spawn path.
 */
export class SessionExecutionService<T = void> {
  readonly #database: RuntimeDatabase
  readonly #sessions: RuntimeSessionRegistry
  readonly #backend: SessionExecutionBackend<T>
  readonly #now: () => number

  constructor(
    database: RuntimeDatabase,
    sessions: RuntimeSessionRegistry,
    backend: SessionExecutionBackend<T>,
    options: { now?: () => number } = {}
  ) {
    this.#database = database
    this.#sessions = sessions
    this.#backend = backend
    this.#now = options.now ?? Date.now
  }

  startOrResume(
    sessionId: string,
    descriptor: SessionExecutionDescriptor,
    authority?: ForkExecutionAuthorityInput
  ): Promise<SessionExecutionResult<T>> {
    if (descriptor.sessionId !== sessionId) {
      return Promise.reject(new Error('Session execution descriptor identity does not match'))
    }
    return this.#sessions.runExclusive(sessionId, async () => {
      const row = this.#durableFork(sessionId)
      const live = this.#sessions.has(sessionId)
      if (!row || terminal(row.stage)) {
        if (row?.stage === 'failed' && !live) {
          return { kind: 'deferred', operationId: row.operation_id, stage: row.stage }
        }
        const value = await this.#backend.startOrResume(descriptor)
        return { kind: 'started', value }
      }

      // A Renderer may only attach an existing process. It never becomes the
      // executor for an accepted durable Fork merely by mounting an xterm view.
      if (live && authority === undefined) {
        const value = await this.#backend.startOrResume(descriptor, undefined, 'attach-only')
        return { kind: 'started', value }
      }
      if (!authority) {
        return { kind: 'deferred', operationId: row.operation_id, stage: row.stage }
      }
      if (!currentAuthority(row, authority, this.#now()) || row.stage !== 'restoring-provider') {
        return { kind: 'stale-authority', operationId: authority.operationId }
      }
      const bound: ForkExecutionAuthority = {
        ...authority,
        sourceSessionId: row.source_session_id,
        sourceProviderSessionId: row.source_provider_session_id
      }
      const value = await this.#backend.startOrResume(descriptor, bound)
      return { kind: 'started', value, authority: bound }
    })
  }

  #durableFork(sessionId: string): DurableForkRow | undefined {
    const row = this.#database.get<DurableForkRow>(
      `SELECT operation_id, session_id, source_session_id, source_provider_session_id,
              stage, lease_token, lease_fence, lease_expires_at
       FROM session_fork_intents WHERE session_id = ?`,
      sessionId
    )
    if (!row || row.operation_id === '' || row.operation_id.startsWith('legacy-operation:')) {
      return undefined
    }
    return row
  }
}

function currentAuthority(
  row: DurableForkRow,
  authority: ForkExecutionAuthorityInput,
  now: number
): boolean {
  return row.operation_id === authority.operationId &&
    row.lease_token === authority.lease.token &&
    row.lease_fence === authority.lease.fence &&
    row.lease_expires_at !== null && row.lease_expires_at > now
}

function terminal(stage: ForkStage): boolean {
  return stage === 'succeeded' || stage === 'failed'
}
