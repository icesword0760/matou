import { createHash, randomUUID } from 'node:crypto'

import type { DomainCommandMetadata } from '@matou/domain'

import type {
  ForkBatchResult,
  ForkEnvironmentChoice,
  ForkItemInput,
  HostActionTargetSelector
} from './host-action-types'
import type {
  ResolvedForkEnvironment,
  ResolvedHostEntity
} from './host-action-target-resolver'
import type { HostCallerIdentity } from './host-control-types'
import type {
  CreateForkInput,
  ForkWorkflowResult,
  RetryForkInput
} from '../session-canvas/fork-workflow-service'
import type { RuntimeDatabase } from '../storage/database'

export type ResolvedForkItemInput = Omit<ForkItemInput, 'environment'> & {
  environment: ResolvedForkEnvironment
}

export interface CreateForkBatchInput {
  caller: HostCallerIdentity
  source: ResolvedHostEntity & { kind: 'session' }
  batchKey: string
  items: ResolvedForkItemInput[]
  publicRequest?: ForkBatchPublicRequest
  restoreFocus?: () => void
}

export interface RetryForkBatchInput extends CreateForkBatchInput {
  retryItemKeys: string[]
}

export interface ForkBatchPreflightInput {
  caller: HostCallerIdentity
  source: HostActionTargetSelector
  batchKey: string
  items: ForkItemInput[]
}

export interface ForkBatchPublicRequest {
  source: HostActionTargetSelector
  items: ForkItemInput[]
}

export interface AcceptedForkBatchRequest {
  source: ResolvedHostEntity & { kind: 'session' }
  items: ResolvedForkItemInput[]
}

interface StoredResolvedForkBatchRequest {
  source: ResolvedHostEntity & { kind: 'session' }
  environments: ResolvedForkEnvironment[]
}

export interface CoordinateAcceptedForkInput extends CreateForkBatchInput {
  sessionId: string
  state: 'created' | 'ready'
}

export interface ForkBatchCoordinatorDependencies {
  database: RuntimeDatabase
  createChild(command: DomainCommandMetadata, input: CreateForkInput): Promise<ForkWorkflowResult>
  retryChild(command: DomainCommandMetadata, input: RetryForkInput): Promise<ForkWorkflowResult>
  startSession(sessionId: string): Promise<void>
  waitUntilReady(sessionId: string, signal?: AbortSignal): Promise<unknown>
  sendPrompt(sessionId: string, prompt: string): Promise<void>
  now?: () => number
}

type BatchItemResult = ForkBatchResult['items'][number]
type LedgerItemState = 'unsubmitted' | 'created' | 'ready' | 'started' | 'failed'
type StartState =
  | 'not-requested' | 'pending' | 'waiting' | 'delivering'
  | 'completed' | 'failed' | 'uncertain'

interface BatchLedgerRow {
  batch_key: string
  request_fingerprint: string
  caller_session_id: string
  source_session_id: string
  source_scene_id: string
  item_count: number
  public_request_fingerprint: string | null
  resolved_request_json: string | null
}

interface BatchItemRow {
  batch_key: string
  item_key: string
  ordinal: number
  item_fingerprint: string
  submission_key: string
  title: string
  environment_json: string
  start_requested: number
  prompt_fingerprint: string | null
  session_id: string | null
  state: LedgerItemState
  start_state: StartState
  error_message: string | null
  failure_generation: number
  failure_receipt: string | null
}

interface RetryAttemptRow {
  attempt_id: string
  batch_key: string
  batch_request_fingerprint: string
  request_fingerprint: string
  retry_keys_json: string
  failure_generations_json: string
  state: 'pending' | 'completed'
  replay_pending: 0 | 1
}

interface RetryAttemptItemRow {
  attempt_id: string
  batch_key: string
  item_key: string
  ordinal: number
  failure_generation: number
  state: 'pending' | 'executing' | 'completed' | 'failed'
  session_id: string | null
  result_state: Exclude<LedgerItemState, 'unsubmitted'> | null
  result_failure_generation: number | null
  error_message: string | null
}

interface ForkIntentSnapshot {
  session_id: string
  operation_id: string
  attempt: number
  stage: 'queued' | 'creating-worktree' | 'applying-setup' | 'binding-session' |
    'restoring-provider' | 'starting-window' | 'succeeded' | 'failed'
  error_message: string | null
}

interface ActiveBatchOperation {
  fingerprint: string
  kind: 'create' | 'retry' | 'accepted'
  retryKey?: string
  promise: Promise<ForkBatchResult>
}

const DATABASE_OPERATIONS = new WeakMap<RuntimeDatabase, Map<string, ActiveBatchOperation>>()

/**
 * Durable, item-idempotent coordination for child Fork batches.
 *
 * SQLite owns completed and restart-resumable state. Memory contains only the
 * currently executing Promise for each batch, so retries have one executor and
 * completed batches do not accumulate in a Runtime-generation cache.
 */
export class ForkBatchCoordinator {
  readonly #database: RuntimeDatabase
  readonly #dependencies: ForkBatchCoordinatorDependencies
  readonly #now: () => number
  readonly #operations: Map<string, ActiveBatchOperation>

  constructor(dependencies: ForkBatchCoordinatorDependencies) {
    this.#database = dependencies.database
    this.#dependencies = dependencies
    this.#now = dependencies.now ?? Date.now
    const shared = DATABASE_OPERATIONS.get(dependencies.database) ??
      new Map<string, ActiveBatchOperation>()
    DATABASE_OPERATIONS.set(dependencies.database, shared)
    this.#operations = shared
  }

  async createChildren(input: CreateForkBatchInput): Promise<ForkBatchResult> {
    validateBatch(input)
    const fingerprint = batchFingerprint(input)
    const active = this.#operations.get(input.batchKey)
    if (active) {
      assertSameBatch(input.batchKey, active.fingerprint, fingerprint)
      return active.promise
    }

    this.#ensureLedger(input, fingerprint, true)
    const operation = this.#resumeCreate(input)
    this.#operations.set(input.batchKey, { fingerprint, kind: 'create', promise: operation })
    try {
      return await operation
    } finally {
      if (this.#operations.get(input.batchKey)?.promise === operation) {
        this.#operations.delete(input.batchKey)
      }
    }
  }

  /**
   * Validates the public request shape of an already accepted batch before the
   * facade reuses its resolved environment reservations. The full resolved
   * fingerprint is still checked by createChildren/retryFailures afterward.
   */
  preflightAccepted(input: ForkBatchPreflightInput): AcceptedForkBatchRequest | undefined {
    const accepted = this.#database.get<BatchLedgerRow>(
      'SELECT * FROM fork_batch_ledger WHERE batch_key = ?',
      input.batchKey
    )
    if (!accepted) return undefined
    const publicFingerprint = publicRequestFingerprint(input.caller, input.batchKey, {
      source: input.source,
      items: input.items
    })
    if (
      accepted.public_request_fingerprint !== publicFingerprint ||
      accepted.resolved_request_json === null
    ) throw new Error(`批次 ${input.batchKey} 与已提交输入不一致`)
    const resolved = JSON.parse(
      accepted.resolved_request_json
    ) as StoredResolvedForkBatchRequest
    if (resolved.environments.length !== input.items.length) {
      throw new Error(`批次 ${input.batchKey} 与已提交输入不一致`)
    }
    return {
      source: resolved.source,
      items: input.items.map((item, index) => ({
        ...item,
        environment: resolved.environments[index]!
      }))
    }
  }

  async retryFailures(input: RetryForkBatchInput): Promise<ForkBatchResult> {
    validateBatch(input)
    validateRetryKeys(input)
    const fingerprint = batchFingerprint(input)
    const retryKey = canonicalJson(input.retryItemKeys)
    const active = this.#operations.get(input.batchKey)
    if (active) {
      assertSameBatch(input.batchKey, active.fingerprint, fingerprint)
      if (active.kind === 'retry' && active.retryKey === retryKey) return active.promise
      await active.promise
      return this.retryFailures(input)
    }

    this.#ensureLedger(input, fingerprint, false)
    this.#refreshAll(input)
    if (input.retryItemKeys.length === 0) return this.#result(input)
    const attempt = this.#resolveRetryAttempt(input, fingerprint)

    const operation = this.#executeRetry(input, attempt)
    this.#operations.set(input.batchKey, {
      fingerprint, kind: 'retry', retryKey, promise: operation
    })
    try {
      return await operation
    } finally {
      if (this.#operations.get(input.batchKey)?.promise === operation) {
        this.#operations.delete(input.batchKey)
      }
    }
  }

  /**
   * Applies the coordinator's durable start/readiness/prompt receipt semantics
   * to a Fork node that the single-Fork workflow has already accepted.
   */
  async coordinateAcceptedFork(
    input: CoordinateAcceptedForkInput
  ): Promise<ForkBatchResult> {
    validateBatch(input)
    if (input.items.length !== 1) {
      throw new Error('已接受的单节点 Fork 必须只有一个项目')
    }
    requiredText(input.sessionId, 'sessionId')
    const ledgerFingerprint = batchFingerprint(input)
    const operationFingerprint = hash(canonicalJson({
      ledgerFingerprint,
      sessionId: input.sessionId
    }))
    const active = this.#operations.get(input.batchKey)
    if (active) {
      assertSameBatch(input.batchKey, active.fingerprint, operationFingerprint)
      return active.promise
    }

    this.#ensureLedger(input, ledgerFingerprint, true)
    const operation = this.#resumeAcceptedFork(input)
    this.#operations.set(input.batchKey, {
      fingerprint: operationFingerprint,
      kind: 'accepted',
      promise: operation
    })
    try {
      return await operation
    } finally {
      if (this.#operations.get(input.batchKey)?.promise === operation) {
        this.#operations.delete(input.batchKey)
      }
    }
  }

  async #resumeAcceptedFork(
    input: CoordinateAcceptedForkInput
  ): Promise<ForkBatchResult> {
    const item = input.items[0]!
    let row = this.#refreshItem(input.batchKey, item)
    if (row.state === 'unsubmitted') {
      this.#writeItem(input.batchKey, item.itemKey, {
        state: input.state,
        sessionId: input.sessionId,
        error: null
      })
      row = this.#itemRow(input.batchKey, item.itemKey)
    } else if (row.session_id !== input.sessionId) {
      throw new Error(`批次 ${input.batchKey} 与已提交输入不一致`)
    }
    if (shouldResumeStart(row)) {
      await this.#startItem(input.batchKey, item, row, input.restoreFocus)
    }
    return this.#result(input)
  }

  async #resumeCreate(input: CreateForkBatchInput): Promise<ForkBatchResult> {
    for (const item of input.items) {
      let row = this.#refreshItem(input.batchKey, item)
      if (row.state === 'unsubmitted') {
        row = await this.#createItem(
          input, item, `initial-create:${itemSubmissionKey(input.batchKey, item.itemKey)}`
        )
      }
      if (shouldResumeStart(row)) {
        await this.#startItem(input.batchKey, item, row, input.restoreFocus)
      }
    }
    this.#refreshAll(input)
    return this.#result(input)
  }

  async #executeRetry(
    input: RetryForkBatchInput,
    attempt: RetryAttemptRow
  ): Promise<ForkBatchResult> {
    const selected = new Set(input.retryItemKeys)
    const attemptItems = new Map(
      this.#retryItemRows(attempt.attempt_id).map((row) => [row.item_key, row])
    )
    for (const item of input.items) {
      if (!selected.has(item.itemKey)) continue
      let attemptItem = requireValue(
        attemptItems.get(item.itemKey),
        `重试 ${attempt.attempt_id} 缺少项目 ${item.itemKey}`
      )
      if (attemptItem.state === 'completed' || attemptItem.state === 'failed') continue
      let row = this.#refreshItem(input.batchKey, item)
      if (row.failure_generation !== attemptItem.failure_generation) {
        this.#recordRetryItem(attempt.attempt_id, item.itemKey, 'failed', row)
        continue
      }
      if (row.state !== 'failed') {
        if (shouldResumeStart(row)) {
          await this.#startItem(input.batchKey, item, row, input.restoreFocus)
        }
        row = this.#refreshItem(input.batchKey, item)
        this.#recordRetryItem(attempt.attempt_id, item.itemKey, 'completed', row)
        continue
      }

      if (attemptItem.state === 'pending') {
        this.#writeRetryItemState(attempt.attempt_id, item.itemKey, 'executing')
        attemptItem = this.#retryItemRow(attempt.attempt_id, item.itemKey)
      }
      const intent = this.#intent(row.submission_key)
      if (intent) {
        if (intent.stage !== 'failed') {
          row = this.#refreshItem(input.batchKey, item)
        } else {
          row = await this.#retryExistingItem(
            input, item, row, intent.session_id, attempt.attempt_id
          )
        }
      } else {
        if (row.session_id !== null) {
          throw new Error(`项目 ${item.itemKey} 的失败 Fork 记录缺失`)
        }
        row = await this.#createItem(
          input, item, `retry-create:${attempt.attempt_id}:${item.itemKey}`
        )
      }
      if (shouldResumeStart(row)) {
        await this.#startItem(input.batchKey, item, row, input.restoreFocus)
      }
      row = this.#refreshItem(input.batchKey, item)
      this.#recordRetryItem(
        attempt.attempt_id,
        item.itemKey,
        row.state === 'failed' ? 'failed' : 'completed',
        row
      )
    }
    this.#completeRetryAttempt(attempt.attempt_id)
    this.#refreshAll(input)
    return this.#result(input)
  }

  async #createItem(
    input: CreateForkBatchInput,
    item: ResolvedForkItemInput,
    failureReceipt: string
  ): Promise<BatchItemRow> {
    const submissionKey = itemSubmissionKey(input.batchKey, item.itemKey)
    try {
      const accepted = await this.#dependencies.createChild(
        createCommand(input, item, submissionKey),
        {
          windowId: input.source.windowId,
          sceneId: input.source.sceneId,
          sourceSessionId: input.source.sessionId,
          name: item.title,
          environment: item.environment,
          submissionKey,
          now: this.#now()
        }
      )
      return this.#recordWorkflowResult(
        input.batchKey, item, accepted, failureReceipt
      )
    } catch (error) {
      const intent = this.#intent(submissionKey)
      if (intent) {
        if (intent.stage === 'failed') {
          this.#recordFailure(input.batchKey, item.itemKey, {
            sessionId: intent.session_id,
            error: intent.error_message ?? errorMessage(error),
            receipt: intentFailureReceipt(intent)
          })
        } else {
          this.#writeItem(input.batchKey, item.itemKey, {
            sessionId: intent.session_id, state: 'created', error: null
          })
        }
        return this.#refreshItem(input.batchKey, item)
      }
      this.#recordFailure(input.batchKey, item.itemKey, {
        sessionId: null, error: errorMessage(error), receipt: failureReceipt
      })
      return this.#itemRow(input.batchKey, item.itemKey)
    } finally {
      input.restoreFocus?.()
    }
  }

  async #retryExistingItem(
    input: RetryForkBatchInput,
    item: ResolvedForkItemInput,
    row: BatchItemRow,
    sessionId: string,
    attemptId: string
  ): Promise<BatchItemRow> {
    this.#writeItem(input.batchKey, item.itemKey, {
      startState: item.start === true ? 'pending' : 'not-requested',
      error: null
    })
    try {
      const accepted = await this.#dependencies.retryChild({
        commandId: `fork-batch-retry:${row.submission_key}:${randomUUID()}`,
        commandType: 'structure.fork.children.retry',
        requestHash: hash(canonicalJson({
          batchKey: input.batchKey, itemKey: item.itemKey, sessionId
        })),
        causationId: input.caller.runId,
        correlationId: `fork-batch:${input.batchKey}`
      }, {
        windowId: input.source.windowId,
        sceneId: input.source.sceneId,
        sessionId,
        now: this.#now()
      })
      return this.#recordWorkflowResult(
        input.batchKey, item, accepted, `retry-workflow:${attemptId}:${item.itemKey}`
      )
    } catch (error) {
      const currentIntent = this.#intent(row.submission_key)
      if (currentIntent && currentIntent.stage !== 'failed') {
        this.#writeItem(input.batchKey, item.itemKey, {
          state: 'created', sessionId: currentIntent.session_id, error: null
        })
        return this.#refreshItem(input.batchKey, item)
      }
      this.#recordFailure(input.batchKey, item.itemKey, {
        sessionId,
        error: errorMessage(error),
        receipt: currentIntent?.stage === 'failed'
          ? retryCallFailureReceipt(currentIntent, attemptId, item.itemKey)
          : `retry-call:${attemptId}:${item.itemKey}`
      })
      return this.#itemRow(input.batchKey, item.itemKey)
    } finally {
      input.restoreFocus?.()
    }
  }

  #recordWorkflowResult(
    batchKey: string,
    item: ResolvedForkItemInput,
    accepted: ForkWorkflowResult,
    fallbackFailureReceipt: string
  ): BatchItemRow {
    const sessionId = accepted.session?.id ?? null
    if (sessionId === null) {
      this.#recordFailure(batchKey, item.itemKey, {
        sessionId: null,
        error: accepted.error ?? 'Fork 未返回已创建的会话',
        receipt: fallbackFailureReceipt
      })
    } else if (accepted.forkState === 'failed') {
      const intent = this.#intent(itemSubmissionKey(batchKey, item.itemKey))
      this.#recordFailure(batchKey, item.itemKey, {
        sessionId,
        error: accepted.error ?? intent?.error_message ?? 'Fork 创建失败',
        receipt: intent?.stage === 'failed'
          ? intentFailureReceipt(intent)
          : fallbackFailureReceipt
      })
    } else {
      this.#writeItem(batchKey, item.itemKey, {
        state: accepted.forkState === 'succeeded' ? 'ready' : 'created',
        sessionId,
        error: accepted.error ?? null
      })
    }
    return this.#itemRow(batchKey, item.itemKey)
  }

  async #startItem(
    batchKey: string,
    item: ResolvedForkItemInput,
    row: BatchItemRow,
    restoreFocus?: () => void
  ): Promise<void> {
    const sessionId = row.session_id
    if (!sessionId) return
    const abort = new AbortController()
    let readiness: Promise<unknown> | undefined
    try {
      restoreFocus?.()
      this.#writeItem(batchKey, item.itemKey, {
        startState: 'waiting', error: null
      })
      readiness = this.#dependencies.waitUntilReady(sessionId, abort.signal)
      // Startup may resolve after the readiness timeout. Attach a handler now
      // while still awaiting the original Promise below so no rejection is
      // transiently unhandled during that interval.
      void readiness.catch(() => undefined)
      await this.#dependencies.startSession(sessionId)
      restoreFocus?.()
      await readiness
      if (item.prompt === undefined) {
        this.#writeItem(batchKey, item.itemKey, {
          state: 'ready', startState: 'completed', error: null
        })
        return
      }

      // Claim delivery durably before the external terminal write. A crash after
      // this point is reported as uncertain and never replays the prompt.
      this.#writeItem(batchKey, item.itemKey, {
        state: 'created', startState: 'delivering', error: null
      })
      await this.#dependencies.sendPrompt(sessionId, item.prompt)
      this.#writeItem(batchKey, item.itemKey, {
        state: 'started', startState: 'completed', error: null
      })
    } catch (error) {
      abort.abort(error)
      await readiness?.catch(() => undefined)
      const current = this.#itemRow(batchKey, item.itemKey)
      const uncertain = current.start_state === 'delivering'
      this.#writeItem(batchKey, item.itemKey, {
        state: 'created',
        startState: uncertain ? 'uncertain' : 'failed',
        error: uncertain
          ? `节点已创建，任务投递结果待确认：${errorMessage(error)}`
          : `节点已创建，任务仍待启动：${errorMessage(error)}`
      })
    } finally {
      restoreFocus?.()
      abort.abort(new Error('provider readiness completed'))
    }
  }

  #ensureLedger(input: CreateForkBatchInput, fingerprint: string, create: boolean): void {
    this.#database.transaction((tx) => {
      const existing = tx.get<BatchLedgerRow>(
        'SELECT * FROM fork_batch_ledger WHERE batch_key = ?', input.batchKey
      )
      if (existing) {
        assertSameBatch(input.batchKey, existing.request_fingerprint, fingerprint)
        this.#assertPublicRequest(input, existing)
        if (existing.item_count !== input.items.length) {
          throw new Error(`批次 ${input.batchKey} 的持久条目数量不一致`)
        }
        const rows = tx.all<BatchItemRow>(
          'SELECT * FROM fork_batch_items WHERE batch_key = ? ORDER BY ordinal', input.batchKey
        )
        for (const [index, item] of input.items.entries()) {
          if (rows[index]?.item_fingerprint !== itemFingerprint(item)) {
            throw new Error(`批次 ${input.batchKey} 与已提交输入不一致`)
          }
        }
        return
      }
      if (!create) throw new Error(`批次 ${input.batchKey} 没有可重试的上一轮结果`)

      const now = this.#now()
      const publicFingerprint = input.publicRequest === undefined
        ? null
        : publicRequestFingerprint(input.caller, input.batchKey, input.publicRequest)
      const resolvedJson = input.publicRequest === undefined
        ? null
        : resolvedRequestJson(input)
      tx.run(
        `INSERT INTO fork_batch_ledger (
           batch_key, request_fingerprint, caller_session_id, source_session_id,
           source_scene_id, item_count, public_request_fingerprint,
           resolved_request_json, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        input.batchKey,
        fingerprint,
        input.caller.sessionId,
        input.source.sessionId,
        input.source.sceneId,
        input.items.length,
        publicFingerprint,
        resolvedJson,
        now,
        now
      )
      for (const [ordinal, item] of input.items.entries()) {
        tx.run(
          `INSERT INTO fork_batch_items (
             batch_key, item_key, ordinal, item_fingerprint, submission_key,
             title, environment_json, start_requested, prompt_fingerprint,
             session_id, state, start_state, error_message, created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 'unsubmitted', ?, NULL, ?, ?)`,
          input.batchKey,
          item.itemKey,
          ordinal,
          itemFingerprint(item),
          itemSubmissionKey(input.batchKey, item.itemKey),
          item.title,
          canonicalJson(publicEnvironment(item.environment)),
          item.start === true ? 1 : 0,
          item.prompt === undefined ? null : hash(item.prompt),
          item.start === true ? 'pending' : 'not-requested',
          now,
          now
        )
      }
    })
  }

  #assertPublicRequest(input: CreateForkBatchInput, existing: BatchLedgerRow): void {
    if (input.publicRequest === undefined) return
    if (
      existing.public_request_fingerprint !== publicRequestFingerprint(
        input.caller,
        input.batchKey,
        input.publicRequest
      ) ||
      existing.resolved_request_json !== resolvedRequestJson(input)
    ) throw new Error(`批次 ${input.batchKey} 与已提交输入不一致`)
  }

  #resolveRetryAttempt(
    input: RetryForkBatchInput,
    batchRequestFingerprint: string
  ): RetryAttemptRow {
    const byKey = new Map(
      this.#itemRows(input.batchKey).map((row) => [row.item_key, row])
    )
    const failureGenerations = input.retryItemKeys.map((itemKey) => {
      const row = requireValue(byKey.get(itemKey), `批次 ${input.batchKey} 缺少项目 ${itemKey}`)
      return { itemKey, failureGeneration: row.failure_generation }
    })
    const requestFingerprint = hash(canonicalJson({
      batchKey: input.batchKey,
      batchRequestFingerprint,
      retryItemKeys: input.retryItemKeys,
      failureGenerations
    }))
    const existing = this.#database.get<RetryAttemptRow>(
      `SELECT * FROM fork_batch_retry_attempts
       WHERE batch_key = ? AND request_fingerprint = ?`,
      input.batchKey,
      requestFingerprint
    )
    if (existing) {
      if (
        existing.batch_request_fingerprint !== batchRequestFingerprint ||
        existing.retry_keys_json !== canonicalJson(input.retryItemKeys) ||
        existing.failure_generations_json !== canonicalJson(failureGenerations)
      ) {
        throw new Error(`批次 ${input.batchKey} 的重试凭据与请求不一致`)
      }
      return existing
    }

    const interrupted = this.#resumableRetryAttempt(
      input, batchRequestFingerprint, byKey
    )
    if (interrupted) return interrupted

    const invalid = input.retryItemKeys.filter((itemKey) => byKey.get(itemKey)?.state !== 'failed')
    if (invalid.length > 0) {
      throw new Error(`仅可重试上一轮失败的项目：${invalid.join(', ')}`)
    }
    const attemptId = hash(`fork-batch-retry:${requestFingerprint}`)
    const now = this.#now()
    this.#database.transaction((tx) => {
      tx.run(
        `INSERT INTO fork_batch_retry_attempts (
           attempt_id, batch_key, batch_request_fingerprint, request_fingerprint, retry_keys_json,
           failure_generations_json, state, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, ?)`,
        attemptId,
        input.batchKey,
        batchRequestFingerprint,
        requestFingerprint,
        canonicalJson(input.retryItemKeys),
        canonicalJson(failureGenerations),
        now,
        now
      )
      for (const [ordinal, binding] of failureGenerations.entries()) {
        tx.run(
          `INSERT INTO fork_batch_retry_items (
             attempt_id, batch_key, item_key, ordinal, failure_generation,
             state, session_id, result_state, error_message, created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, 'pending', NULL, NULL, NULL, ?, ?)`,
          attemptId,
          input.batchKey,
          binding.itemKey,
          ordinal,
          binding.failureGeneration,
          now,
          now
        )
      }
    })
    return requireValue(
      this.#database.get<RetryAttemptRow>(
        'SELECT * FROM fork_batch_retry_attempts WHERE attempt_id = ?', attemptId
      ),
      `重试 ${attemptId} 未写入`
    )
  }

  #resumableRetryAttempt(
    input: RetryForkBatchInput,
    batchRequestFingerprint: string,
    currentRows: ReadonlyMap<string, BatchItemRow>
  ): RetryAttemptRow | undefined {
    const candidates = this.#database.all<RetryAttemptRow>(
      `SELECT * FROM fork_batch_retry_attempts
       WHERE batch_key = ? AND batch_request_fingerprint = ?
         AND retry_keys_json = ?
         AND (state = 'pending' OR (state = 'completed' AND replay_pending = 1))
       ORDER BY replay_pending DESC, created_at DESC, attempt_id DESC`,
      input.batchKey,
      batchRequestFingerprint,
      canonicalJson(input.retryItemKeys)
    )
    for (const candidate of candidates) {
      const receipts = new Map(
        this.#retryItemRows(candidate.attempt_id).map((row) => [row.item_key, row])
      )
      const matches = input.retryItemKeys.every((itemKey) => {
        const current = currentRows.get(itemKey)
        const receipt = receipts.get(itemKey)
        if (!current || !receipt) return false
        if (receipt.state === 'failed') {
          return current.state === 'failed' &&
            receipt.result_failure_generation === current.failure_generation
        }
        return receipt.failure_generation === current.failure_generation
      })
      if (matches) return candidate
    }
    return undefined
  }

  #retryItemRows(attemptId: string): RetryAttemptItemRow[] {
    return this.#database.all<RetryAttemptItemRow>(
      'SELECT * FROM fork_batch_retry_items WHERE attempt_id = ? ORDER BY ordinal',
      attemptId
    )
  }

  #retryItemRow(attemptId: string, itemKey: string): RetryAttemptItemRow {
    return requireValue(
      this.#database.get<RetryAttemptItemRow>(
        'SELECT * FROM fork_batch_retry_items WHERE attempt_id = ? AND item_key = ?',
        attemptId,
        itemKey
      ),
      `重试 ${attemptId} 缺少项目 ${itemKey}`
    )
  }

  #writeRetryItemState(
    attemptId: string,
    itemKey: string,
    state: RetryAttemptItemRow['state']
  ): void {
    this.#database.run(
      `UPDATE fork_batch_retry_items SET state = ?, updated_at = ?
       WHERE attempt_id = ? AND item_key = ?`,
      state,
      this.#now(),
      attemptId,
      itemKey
    )
  }

  #recordRetryItem(
    attemptId: string,
    itemKey: string,
    state: 'completed' | 'failed',
    result: BatchItemRow
  ): void {
    this.#database.run(
      `UPDATE fork_batch_retry_items
       SET state = ?, session_id = ?, result_state = ?, result_failure_generation = ?,
           error_message = ?, updated_at = ?
       WHERE attempt_id = ? AND item_key = ?`,
      state,
      result.session_id,
      result.state === 'unsubmitted' ? 'created' : result.state,
      result.state === 'failed' ? result.failure_generation : null,
      result.error_message,
      this.#now(),
      attemptId,
      itemKey
    )
  }

  #completeRetryAttempt(attemptId: string): void {
    const unfinished = this.#database.get<{ count: number }>(
      `SELECT COUNT(*) AS count FROM fork_batch_retry_items
       WHERE attempt_id = ? AND state IN ('pending', 'executing')`,
      attemptId
    )?.count ?? 0
    if (unfinished !== 0) return
    this.#database.run(
      `UPDATE fork_batch_retry_attempts
       SET state = 'completed', replay_pending = 0, updated_at = ?
       WHERE attempt_id = ?`,
      this.#now(),
      attemptId
    )
  }

  #refreshAll(input: CreateForkBatchInput): void {
    for (const item of input.items) this.#refreshItem(input.batchKey, item)
  }

  #refreshItem(
    batchKey: string,
    item: ResolvedForkItemInput
  ): BatchItemRow {
    let row = this.#itemRow(batchKey, item.itemKey)
    if (row.start_state === 'delivering') {
      this.#writeItem(batchKey, item.itemKey, {
        state: 'created', startState: 'uncertain',
        error: '节点已创建，任务投递结果待确认'
      })
      row = this.#itemRow(batchKey, item.itemKey)
    }
    const intent = this.#intent(row.submission_key)
    if (!intent) return row

    if (intent.stage === 'failed') {
      if (isRetryCallReceiptFor(row.failure_receipt, intent)) return row
      this.#recordFailure(batchKey, item.itemKey, {
        sessionId: intent.session_id,
        error: intent.error_message ?? 'Fork 创建失败',
        receipt: intentFailureReceipt(intent)
      })
    } else if (row.start_state === 'completed') {
      this.#writeItem(batchKey, item.itemKey, {
        state: item.prompt === undefined ? 'ready' : 'started',
        sessionId: intent.session_id,
        error: null
      })
    } else if (row.start_state === 'failed' || row.start_state === 'uncertain') {
      this.#writeItem(batchKey, item.itemKey, {
        state: 'created', sessionId: intent.session_id
      })
    } else if (intent.stage === 'succeeded') {
      this.#writeItem(batchKey, item.itemKey, {
        state: item.start === true ? 'created' : 'ready',
        sessionId: intent.session_id, error: null
      })
    } else {
      this.#writeItem(batchKey, item.itemKey, {
        state: 'created', sessionId: intent.session_id,
        error: null
      })
    }
    return this.#itemRow(batchKey, item.itemKey)
  }

  #intent(submissionKey: string): ForkIntentSnapshot | undefined {
    return this.#database.get<ForkIntentSnapshot>(
      `SELECT session_id, operation_id, attempt, stage, error_message
       FROM session_fork_intents WHERE submission_key = ?`,
      submissionKey
    )
  }

  #recordFailure(
    batchKey: string,
    itemKey: string,
    failure: { sessionId: string | null; error: string; receipt: string }
  ): void {
    const now = this.#now()
    this.#database.transaction((tx) => {
      const row = requireValue(tx.get<BatchItemRow>(
        'SELECT * FROM fork_batch_items WHERE batch_key = ? AND item_key = ?',
        batchKey,
        itemKey
      ), `批次 ${batchKey} 缺少项目 ${itemKey}`)
      const failureGeneration = row.failure_receipt === failure.receipt
        ? row.failure_generation
        : row.failure_generation + 1
      const advanced = failureGeneration !== row.failure_generation
      tx.run(
        `UPDATE fork_batch_items
         SET state = 'failed', session_id = ?, error_message = ?,
             failure_generation = ?, failure_receipt = ?, updated_at = ?
         WHERE batch_key = ? AND item_key = ?`,
        failure.sessionId,
        failure.error,
        failureGeneration,
        failure.receipt,
        now,
        batchKey,
        itemKey
      )
      if (advanced) {
        const relatedAttempts = tx.all<{ attempt_id: string }>(
          `SELECT DISTINCT retry.attempt_id
           FROM fork_batch_retry_items AS retry
           JOIN fork_batch_retry_attempts AS attempt
             ON attempt.attempt_id = retry.attempt_id
           WHERE retry.batch_key = ? AND retry.item_key = ?
             AND retry.failure_generation = ?
             AND retry.state IN ('pending', 'executing')
             AND attempt.state = 'pending'`,
          batchKey,
          itemKey,
          row.failure_generation
        )
        tx.run(
          `UPDATE fork_batch_retry_items
           SET state = 'failed', session_id = ?, result_state = 'failed',
               result_failure_generation = ?, error_message = ?, updated_at = ?
           WHERE batch_key = ? AND item_key = ? AND failure_generation = ?
             AND state IN ('pending', 'executing')
             AND attempt_id IN (
               SELECT attempt_id FROM fork_batch_retry_attempts WHERE state = 'pending'
             )`,
          failure.sessionId,
          failureGeneration,
          failure.error,
          now,
          batchKey,
          itemKey,
          row.failure_generation
        )
        for (const { attempt_id: attemptId } of relatedAttempts) {
          const unfinished = tx.get<{ count: number }>(
            `SELECT COUNT(*) AS count FROM fork_batch_retry_items
             WHERE attempt_id = ? AND state IN ('pending', 'executing')`,
            attemptId
          )?.count ?? 0
          if (unfinished === 0) {
            tx.run(
              `UPDATE fork_batch_retry_attempts
               SET state = 'completed', replay_pending = 1, updated_at = ?
               WHERE attempt_id = ? AND state = 'pending'`,
              now,
              attemptId
            )
          }
        }
      }
      tx.run(
        'UPDATE fork_batch_ledger SET updated_at = ? WHERE batch_key = ?',
        now,
        batchKey
      )
    })
  }

  #writeItem(
    batchKey: string,
    itemKey: string,
    update: {
      state?: LedgerItemState
      startState?: StartState
      sessionId?: string | null
      error?: string | null
    }
  ): void {
    const row = this.#itemRow(batchKey, itemKey)
    this.#database.run(
      `UPDATE fork_batch_items
       SET state = ?, start_state = ?, session_id = ?, error_message = ?,
           failure_generation = ?, failure_receipt = ?, updated_at = ?
       WHERE batch_key = ? AND item_key = ?`,
      update.state ?? row.state,
      update.startState ?? row.start_state,
      update.sessionId === undefined ? row.session_id : update.sessionId,
      update.error === undefined ? row.error_message : update.error,
      row.failure_generation,
      row.failure_receipt,
      this.#now(),
      batchKey,
      itemKey
    )
    this.#database.run(
      'UPDATE fork_batch_ledger SET updated_at = ? WHERE batch_key = ?',
      this.#now(),
      batchKey
    )
  }

  #itemRow(batchKey: string, itemKey: string): BatchItemRow {
    const row = this.#database.get<BatchItemRow>(
      'SELECT * FROM fork_batch_items WHERE batch_key = ? AND item_key = ?',
      batchKey,
      itemKey
    )
    if (!row) throw new Error(`批次 ${batchKey} 缺少项目 ${itemKey}`)
    return row
  }

  #itemRows(batchKey: string): BatchItemRow[] {
    return this.#database.all<BatchItemRow>(
      'SELECT * FROM fork_batch_items WHERE batch_key = ? ORDER BY ordinal', batchKey
    )
  }

  #result(input: CreateForkBatchInput): ForkBatchResult {
    const rows = this.#itemRows(input.batchKey)
    const items = rows.map((row, index): BatchItemResult => {
      const item = input.items[index]!
      const publicState = row.state === 'unsubmitted' ? 'created' : row.state
      return {
        itemKey: item.itemKey,
        title: item.title,
        state: publicState,
        ...(row.session_id === null ? {} : { sessionRef: `session:${row.session_id}` }),
        environment: publicEnvironment(item.environment),
        ...(row.error_message === null ? {} : { error: row.error_message })
      }
    })
    const failedItems = items.filter(({ state }) => state === 'failed')
    return {
      kind: 'fork-batch',
      batchKey: input.batchKey,
      succeeded: items.length - failedItems.length,
      failed: failedItems.length,
      items,
      ...(failedItems.length === 0 ? {} : {
        retry: {
          batchKey: input.batchKey,
          itemKeys: failedItems.map(({ itemKey }) => itemKey)
        }
      })
    }
  }
}

export function itemSubmissionKey(batchKey: string, itemKey: string): string {
  return hash(`${batchKey}:${itemKey}`)
}

function shouldResumeStart(row: BatchItemRow): boolean {
  return row.start_requested === 1 && row.session_id !== null && row.state !== 'failed' &&
    (row.start_state === 'pending' || row.start_state === 'waiting')
}

function createCommand(
  input: CreateForkBatchInput,
  item: ResolvedForkItemInput,
  submissionKey: string
): DomainCommandMetadata {
  return {
    commandId: `fork-batch-item:${submissionKey}`,
    commandType: 'structure.fork.children.item',
    requestHash: hash(canonicalJson({
      callerSessionId: input.caller.sessionId,
      source: input.source,
      batchKey: input.batchKey,
      item
    })),
    causationId: input.caller.runId,
    correlationId: `fork-batch:${input.batchKey}`
  }
}

function validateBatch(input: CreateForkBatchInput): void {
  boundedKey(input.batchKey, 'batchKey')
  if (input.items.length > 50) throw new Error('items must contain at most 50 entries')
  const itemKeys = new Set<string>()
  for (const item of input.items) {
    boundedKey(item.itemKey, 'itemKey')
    if (itemKeys.has(item.itemKey)) throw new Error('itemKey must be unique')
    itemKeys.add(item.itemKey)
    boundedUtf8(item.title, 1, 160, 'title')
    if (item.prompt !== undefined) boundedUtf8(item.prompt, 0, 64 * 1024, 'prompt')
    validateEnvironment(item.environment)
  }
}

function validateRetryKeys(input: RetryForkBatchInput): void {
  if (input.retryItemKeys.length > 50) throw new Error('retryItemKeys must contain at most 50 entries')
  const members = new Set(input.items.map(({ itemKey }) => itemKey))
  const retryKeys = new Set<string>()
  for (const itemKey of input.retryItemKeys) {
    boundedKey(itemKey, 'retry itemKey')
    if (retryKeys.has(itemKey)) throw new Error('retry itemKey must be unique')
    if (!members.has(itemKey)) throw new Error('retry itemKey must belong to items')
    retryKeys.add(itemKey)
  }
}

function validateEnvironment(environment: ResolvedForkEnvironment): void {
  if (environment.mode === 'current') {
    requiredText(environment.executionContextId, 'executionContextId')
    return
  }
  if (environment.mode === 'existing-worktree') {
    requiredText(environment.executionContextId, 'executionContextId')
    requiredText(environment.worktreeId, 'worktreeId')
    requiredText(environment.worktreeRef, 'worktreeRef')
    requiredText(environment.branch, 'branch')
    return
  }
  requiredText(environment.branch, 'branch')
}

function boundedKey(value: string, field: string): void {
  if (value.length < 1 || value.length > 160) throw new Error(`${field} must contain 1-160 characters`)
}

function boundedUtf8(value: string, minimum: number, maximum: number, field: string): void {
  const bytes = Buffer.byteLength(value, 'utf8')
  if (bytes < minimum || bytes > maximum) {
    throw new Error(`${field} must contain ${minimum}-${maximum} UTF-8 bytes`)
  }
}

function requiredText(value: string, field: string): void {
  if (!value) throw new Error(`${field} is required`)
}

function batchFingerprint(input: CreateForkBatchInput): string {
  return hash(canonicalJson({
    callerSessionId: input.caller.sessionId,
    source: input.source,
    batchKey: input.batchKey,
    items: input.items
  }))
}

function publicRequestFingerprint(
  caller: HostCallerIdentity,
  batchKey: string,
  request: ForkBatchPublicRequest
): string {
  return hash(canonicalJson({
    callerSessionId: caller.sessionId,
    batchKey,
    source: request.source,
    // Array order is part of the public identity; object keys are canonicalized.
    items: request.items
  }))
}

function resolvedRequestJson(input: CreateForkBatchInput): string {
  return canonicalJson({
    source: input.source,
    environments: input.items.map(({ environment }) => environment)
  })
}

function itemFingerprint(item: ResolvedForkItemInput): string {
  return hash(canonicalJson(item))
}

function assertSameBatch(batchKey: string, prior: string, next: string): void {
  if (prior !== next) throw new Error(`批次 ${batchKey} 与已提交输入不一致`)
}

function publicEnvironment(environment: ResolvedForkEnvironment): ForkEnvironmentChoice {
  if (environment.mode === 'current') return { mode: 'current' }
  if (environment.mode === 'new-worktree') {
    return { mode: 'new-worktree', branch: environment.branch }
  }
  return {
    mode: 'existing-worktree',
    branch: environment.branch,
    worktreeRef: environment.worktreeRef
  }
}

function hash(value: string): string {
  return createHash('sha256').update(value).digest('hex')
}

function canonicalJson(value: unknown): string {
  return JSON.stringify(canonicalValue(value))
}

function canonicalValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalValue)
  if (value === null || typeof value !== 'object') return value
  return Object.fromEntries(
    Object.entries(value)
      .filter(([, entry]) => entry !== undefined)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, entry]) => [key, canonicalValue(entry)])
  )
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function intentFailureReceipt(intent: ForkIntentSnapshot): string {
  return `fork-intent:${intent.operation_id}:${intent.attempt}`
}

function retryCallFailureReceipt(
  intent: ForkIntentSnapshot,
  attemptId: string,
  itemKey: string
): string {
  return `retry-call:${intent.operation_id}:${intent.attempt}:${attemptId}:${hash(itemKey)}`
}

function isRetryCallReceiptFor(
  receipt: string | null,
  intent: ForkIntentSnapshot
): boolean {
  return receipt?.startsWith(`retry-call:${intent.operation_id}:${intent.attempt}:`) === true
}

function requireValue<T>(value: T | undefined, message: string): T {
  if (value === undefined) throw new Error(message)
  return value
}
