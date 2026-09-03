import { createHash } from 'node:crypto'

import type { DomainCommandMetadata } from '@matou/domain'

import type {
  ForkBatchResult,
  ForkEnvironmentChoice,
  ForkItemInput
} from './host-action-types'
import type {
  ResolvedForkEnvironment,
  ResolvedHostEntity
} from './host-action-target-resolver'
import type { HostCallerIdentity } from './host-control-types'
import type {
  CreateForkInput,
  ForkWorkflowResult
} from '../session-canvas/fork-workflow-service'

export type ResolvedForkItemInput = Omit<ForkItemInput, 'environment'> & {
  environment: ResolvedForkEnvironment
}

export interface CreateForkBatchInput {
  caller: HostCallerIdentity
  source: ResolvedHostEntity & { kind: 'session' }
  batchKey: string
  items: ResolvedForkItemInput[]
}

export interface RetryForkBatchInput extends CreateForkBatchInput {
  retryItemKeys: string[]
}

export interface ForkBatchCoordinatorDependencies {
  createChild(command: DomainCommandMetadata, input: CreateForkInput): Promise<ForkWorkflowResult>
  startSession(sessionId: string): Promise<void>
  waitUntilReady(sessionId: string): Promise<unknown>
  sendPrompt(sessionId: string, prompt: string): Promise<void>
  now?: () => number
}

type BatchItemResult = ForkBatchResult['items'][number]

interface BatchRecord {
  fingerprint: string
  items: ResolvedForkItemInput[]
  results: Map<string, BatchItemResult>
  operation?: Promise<ForkBatchResult>
}

/** Coordinates deterministic, item-idempotent child Forks for one Runtime generation. */
export class ForkBatchCoordinator {
  readonly #dependencies: ForkBatchCoordinatorDependencies
  readonly #now: () => number
  readonly #batches = new Map<string, BatchRecord>()

  constructor(dependencies: ForkBatchCoordinatorDependencies) {
    this.#dependencies = dependencies
    this.#now = dependencies.now ?? Date.now
  }

  async createChildren(input: CreateForkBatchInput): Promise<ForkBatchResult> {
    validateBatch(input)
    const fingerprint = batchFingerprint(input)
    const existing = this.#batches.get(input.batchKey)
    if (existing) {
      assertSameBatch(input.batchKey, existing.fingerprint, fingerprint)
      return existing.operation ?? batchResult(input.batchKey, existing)
    }

    const record: BatchRecord = {
      fingerprint,
      items: input.items.map(cloneItem),
      results: new Map()
    }
    this.#batches.set(input.batchKey, record)
    const operation = this.#execute(input, record, input.items.map(({ itemKey }) => itemKey))
    record.operation = operation
    void operation.finally(() => {
      if (record.operation === operation) delete record.operation
    }).catch(() => undefined)
    return operation
  }

  async retryFailures(input: RetryForkBatchInput): Promise<ForkBatchResult> {
    validateBatch(input)
    validateRetryKeys(input)
    const fingerprint = batchFingerprint(input)
    const record = this.#batches.get(input.batchKey)
    if (!record) throw new Error(`批次 ${input.batchKey} 没有可重试的上一轮结果`)
    assertSameBatch(input.batchKey, record.fingerprint, fingerprint)
    if (record.operation) await record.operation

    const invalid = input.retryItemKeys.filter((itemKey) => record.results.get(itemKey)?.state !== 'failed')
    if (invalid.length > 0) {
      throw new Error(`仅可重试上一轮失败的项目：${invalid.join(', ')}`)
    }
    if (input.retryItemKeys.length === 0) return batchResult(input.batchKey, record)

    const operation = this.#execute(input, record, input.retryItemKeys)
    record.operation = operation
    try {
      return await operation
    } finally {
      if (record.operation === operation) delete record.operation
    }
  }

  async #execute(
    input: CreateForkBatchInput,
    record: BatchRecord,
    itemKeys: string[]
  ): Promise<ForkBatchResult> {
    const selected = new Set(itemKeys)
    for (const item of record.items) {
      if (!selected.has(item.itemKey)) continue
      record.results.set(item.itemKey, await this.#executeItem(input, item))
    }
    return batchResult(input.batchKey, record)
  }

  async #executeItem(
    input: CreateForkBatchInput,
    item: ResolvedForkItemInput
  ): Promise<BatchItemResult> {
    const submissionKey = itemSubmissionKey(input.batchKey, item.itemKey)
    const environment = publicEnvironment(item.environment)
    let accepted: ForkWorkflowResult
    try {
      accepted = await this.#dependencies.createChild({
        commandId: `fork-batch-item:${submissionKey}`,
        commandType: 'structure.fork.children.item',
        requestHash: itemRequestHash(input, item),
        causationId: input.caller.runId,
        correlationId: `fork-batch:${input.batchKey}`
      }, {
        windowId: input.source.windowId,
        sceneId: input.source.sceneId,
        sourceSessionId: input.source.sessionId,
        name: item.title,
        environment: item.environment,
        submissionKey,
        now: this.#now()
      })
    } catch (error) {
      return {
        itemKey: item.itemKey,
        title: item.title,
        state: 'failed',
        environment,
        error: errorMessage(error)
      }
    }

    const sessionId = accepted.session?.id
    if (!sessionId) {
      return {
        itemKey: item.itemKey,
        title: item.title,
        state: 'failed',
        environment,
        error: accepted.error ?? 'Fork 未返回已创建的会话'
      }
    }
    const base: Omit<BatchItemResult, 'state'> = {
      itemKey: item.itemKey,
      title: item.title,
      sessionRef: `session:${sessionId}`,
      environment
    }
    if (accepted.forkState === 'failed') {
      return { ...base, state: 'failed', error: accepted.error ?? 'Fork 创建失败' }
    }
    if (item.start !== true) {
      return {
        ...base,
        state: accepted.forkState === 'succeeded' ? 'ready' : 'created',
        ...(accepted.error === undefined ? {} : { error: accepted.error })
      }
    }

    let readiness: Promise<unknown> | undefined
    try {
      // Register first so a synchronous identity hook during startup cannot be lost.
      readiness = this.#dependencies.waitUntilReady(sessionId)
      await this.#dependencies.startSession(sessionId)
      await readiness
      if (item.prompt === undefined) return { ...base, state: 'ready' }
      await this.#dependencies.sendPrompt(sessionId, item.prompt)
      return { ...base, state: 'started' }
    } catch (error) {
      void readiness?.catch(() => undefined)
      return {
        ...base,
        state: 'created',
        error: `节点已创建，任务仍待启动：${errorMessage(error)}`
      }
    }
  }
}

export function itemSubmissionKey(batchKey: string, itemKey: string): string {
  return hash(`${batchKey}:${itemKey}`)
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
    caller: input.caller,
    source: input.source,
    batchKey: input.batchKey,
    items: input.items
  }))
}

function itemRequestHash(input: CreateForkBatchInput, item: ResolvedForkItemInput): string {
  return hash(canonicalJson({
    caller: input.caller,
    source: input.source,
    batchKey: input.batchKey,
    item
  }))
}

function assertSameBatch(batchKey: string, prior: string, next: string): void {
  if (prior !== next) throw new Error(`批次 ${batchKey} 与上一轮输入不一致`)
}

function cloneItem(item: ResolvedForkItemInput): ResolvedForkItemInput {
  return {
    ...item,
    environment: { ...item.environment }
  }
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

function batchResult(batchKey: string, record: BatchRecord): ForkBatchResult {
  const items = record.items.flatMap((item) => {
    const result = record.results.get(item.itemKey)
    return result ? [{ ...result, environment: { ...result.environment } }] : []
  })
  const failedItems = items.filter(({ state }) => state === 'failed')
  return {
    kind: 'fork-batch',
    batchKey,
    succeeded: items.length - failedItems.length,
    failed: failedItems.length,
    items,
    ...(failedItems.length === 0 ? {} : {
      retry: { batchKey, itemKeys: failedItems.map(({ itemKey }) => itemKey) }
    })
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
