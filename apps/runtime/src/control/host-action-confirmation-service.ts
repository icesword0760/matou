import { createHash, randomBytes } from 'node:crypto'

import type { HostImpactSummary } from './host-action-types'
import type { HostCallerIdentity } from './host-control-types'

export type HostActionConfirmationCode =
  | 'CONFIRMATION_REQUIRED'
  | 'CONFIRMATION_EXPIRED'
  | 'CONFIRMATION_STALE'

export class HostActionConfirmationError extends Error {
  readonly code: HostActionConfirmationCode

  constructor(code: HostActionConfirmationCode, message: string) {
    super(message)
    this.name = 'HostActionConfirmationError'
    this.code = code
  }
}

export interface ConfirmationIssueInput {
  caller: HostCallerIdentity
  action: 'remove' | 'canvas-close'
  targetRef: string
  scope: 'node' | 'subtree'
  projectionRevision: string
  impact: HostImpactSummary
  now: number
}

export interface ConfirmationConsumeInput extends ConfirmationIssueInput {
  ref: string
}

export interface ConfirmationInspectInput {
  ref: string
  caller: HostCallerIdentity
  now: number
}

export interface ConfirmationRecord extends ConfirmationIssueInput {
  impactHash: string
  expiresAt: number
}

interface StoredConfirmation extends ConfirmationRecord {
  /** The caller identity is copied into the record to keep the binding explicit. */
  caller: HostCallerIdentity
}

const DEFAULT_TTL_MS = 120_000

/**
 * Keeps destructive-action confirmations in Runtime memory only.
 *
 * A confirmation is deliberately bound to the caller and the exact public
 * preview summary. Runtime restart therefore clears all outstanding records.
 */
export class HostActionConfirmationService {
  readonly #ttlMs: number
  readonly #randomRef: () => string
  readonly #records = new Map<string, StoredConfirmation>()

  constructor(options: { ttlMs?: number; randomRef?: () => string } = {}) {
    this.#ttlMs = options.ttlMs ?? DEFAULT_TTL_MS
    this.#randomRef = options.randomRef ?? (() => randomBytes(24).toString('base64url'))
  }

  issue(input: ConfirmationIssueInput): string {
    this.#purgeExpired(input.now)

    const ref = this.#randomRef()
    if (this.#records.has(ref)) {
      throw new Error('生成了重复的确认引用')
    }

    const impact = cloneImpact(input.impact)
    this.#records.set(ref, {
      ...input,
      caller: { ...input.caller },
      impact,
      impactHash: impactHash(input.action, input.targetRef, input.scope, impact),
      expiresAt: input.now + this.#ttlMs
    })
    return ref
  }

  inspect(input: ConfirmationInspectInput): ConfirmationRecord {
    const record = this.#records.get(input.ref)
    const requestedRecordExpired = record !== undefined && input.now >= record.expiresAt
    // Purge before any outcome, including a caller mismatch. Keep the local
    // requested record only to distinguish a rightful expired reference from
    // an absent reference after the purge; mismatched callers still receive
    // the same required fault without learning the record's state.
    this.#purgeExpired(input.now)
    if (!record) {
      throw new HostActionConfirmationError(
        'CONFIRMATION_REQUIRED',
        '确认已失效，请先重新预览'
      )
    }

    // Caller mismatch is intentionally indistinguishable from a missing or
    // already-consumed reference, so confirmation references cannot be used
    // to probe another run's pending actions.
    if (!sameCaller(record.caller, input.caller)) {
      throw new HostActionConfirmationError(
        'CONFIRMATION_REQUIRED',
        '当前运行没有可用的确认'
      )
    }

    if (requestedRecordExpired) {
      throw new HostActionConfirmationError(
        'CONFIRMATION_EXPIRED',
        '确认已过期，请重新预览'
      )
    }

    return cloneRecord(record)
  }

  consume(input: ConfirmationConsumeInput): ConfirmationRecord {
    const record = this.inspect(input)

    const submittedHash = impactHash(input.action, input.targetRef, input.scope, input.impact)
    if (
      record.action !== input.action
      || record.targetRef !== input.targetRef
      || record.scope !== input.scope
      || record.projectionRevision !== input.projectionRevision
      || record.impactHash !== submittedHash
    ) {
      throw new HostActionConfirmationError(
        'CONFIRMATION_STALE',
        '确认对应的目标或影响已变化，请重新预览'
      )
    }

    // Consume is one-time. Delete before returning so reentrant callers cannot
    // successfully consume the same reference again.
    this.#records.delete(input.ref)
    return record
  }

  revokeRun(runId: string): void {
    for (const [ref, record] of this.#records) {
      if (record.caller.runId === runId) this.#records.delete(ref)
    }
  }

  #purgeExpired(now: number): void {
    for (const [ref, record] of this.#records) {
      if (record.expiresAt <= now) this.#records.delete(ref)
    }
  }
}

function sameCaller(left: HostCallerIdentity, right: HostCallerIdentity): boolean {
  return left.runId === right.runId && left.sessionId === right.sessionId
}

function impactHash(
  action: ConfirmationIssueInput['action'],
  targetRef: string,
  scope: ConfirmationIssueInput['scope'],
  impact: HostImpactSummary
): string {
  return createHash('sha256')
    .update(canonicalJson({ action, target: targetRef, scope, impact }))
    .digest('hex')
}

/** Stable object-key ordering prevents insertion order from changing a hash. */
function canonicalJson(value: unknown): string {
  if (value === null || typeof value !== 'object') return JSON.stringify(value)
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`
  const object = value as Record<string, unknown>
  return `{${Object.keys(object).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(object[key])}`).join(',')}}`
}

function cloneImpact(impact: HostImpactSummary): HostImpactSummary {
  return JSON.parse(JSON.stringify(impact)) as HostImpactSummary
}

function cloneRecord(record: StoredConfirmation): ConfirmationRecord {
  return {
    ...record,
    caller: { ...record.caller },
    impact: cloneImpact(record.impact)
  }
}
