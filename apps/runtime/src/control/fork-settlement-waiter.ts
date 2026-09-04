import { setTimeout as delay } from 'node:timers/promises'

import { HOST_CONTROL_FORK_SETTLEMENT_TIMEOUT_MS } from './host-control-deadlines'
import type { DatabaseTransaction } from '../storage/database'

export type ForkSettlementWaitErrorCode =
  | 'FORK_SETTLEMENT_TIMEOUT'
  | 'FORK_SETTLEMENT_MISSING'

export class ForkSettlementWaitError extends Error {
  readonly code: ForkSettlementWaitErrorCode
  readonly diagnostic: string

  constructor(code: ForkSettlementWaitErrorCode, message: string, diagnostic: string) {
    super(message)
    this.name = 'ForkSettlementWaitError'
    this.code = code
    this.diagnostic = diagnostic
  }
}

interface ForkSettlementWaitOptions {
  timeoutMs?: number
  pollIntervalMs?: number
  now?: () => number
  wait?: (milliseconds: number) => Promise<void>
}

export async function waitUntilForkSettled(
  database: Pick<DatabaseTransaction, 'get'>,
  sessionId: string,
  options: ForkSettlementWaitOptions = {}
): Promise<void> {
  const timeoutMs = options.timeoutMs ?? HOST_CONTROL_FORK_SETTLEMENT_TIMEOUT_MS
  const pollIntervalMs = options.pollIntervalMs ?? 25
  const now = options.now ?? Date.now
  const wait = options.wait ?? ((milliseconds: number) => delay(milliseconds))
  const deadline = now() + timeoutMs
  while (true) {
    const stage = database.get<{ stage: string }>(
      'SELECT stage FROM session_fork_intents WHERE session_id = ?', sessionId
    )?.stage
    if (stage === 'succeeded' || stage === 'failed') return
    if (stage === undefined) {
      throw new ForkSettlementWaitError(
        'FORK_SETTLEMENT_MISSING',
        'Fork 状态记录不可用',
        `Fork settlement intent is missing for session ${sessionId}`
      )
    }
    if (now() >= deadline) {
      throw new ForkSettlementWaitError(
        'FORK_SETTLEMENT_TIMEOUT',
        'Fork 状态确认超时',
        `Fork settlement timed out for session ${sessionId}`
      )
    }
    await wait(pollIntervalMs)
  }
}
