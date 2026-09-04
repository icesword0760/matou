import { describe, expect, it, vi } from 'vitest'

import {
  ForkSettlementWaitError,
  waitUntilForkSettled
} from './fork-settlement-waiter'
import type { DatabaseTransaction } from '../storage/database'

describe('waitUntilForkSettled', () => {
  it('returns when the durable Fork intent reaches a terminal stage', async () => {
    const get = vi.fn()
      .mockReturnValueOnce({ stage: 'creating-worktree' })
      .mockReturnValueOnce({ stage: 'succeeded' })

    await expect(waitUntilForkSettled(databaseReturning(get), 'stable-session', {
      timeoutMs: 100,
      pollIntervalMs: 25,
      now: () => 0,
      wait: async () => undefined
    })).resolves.toBeUndefined()
    expect(get).toHaveBeenCalledTimes(2)
  })

  it('reports a typed timeout without putting the session identifier in its public message', async () => {
    let now = 0
    const failure = await captureFailure(waitUntilForkSettled(databaseReturning(
      vi.fn(() => ({ stage: 'creating-worktree' }))
    ), 'private-timeout-session', {
      timeoutMs: 50,
      pollIntervalMs: 25,
      now: () => now,
      wait: async (milliseconds) => { now += milliseconds }
    }))

    expect(failure).toMatchObject({
      code: 'FORK_SETTLEMENT_TIMEOUT',
      message: 'Fork 状态确认超时'
    })
    expect(failure.message).not.toContain('private-timeout-session')
    expect(failure.diagnostic).toContain('private-timeout-session')
  })

  it('reports a typed missing-intent fault without putting the session identifier in its public message', async () => {
    const failure = await captureFailure(waitUntilForkSettled(databaseReturning(
      vi.fn(() => undefined)
    ), 'private-missing-session'))

    expect(failure).toMatchObject({
      code: 'FORK_SETTLEMENT_MISSING',
      message: 'Fork 状态记录不可用'
    })
    expect(failure.message).not.toContain('private-missing-session')
    expect(failure.diagnostic).toContain('private-missing-session')
  })
})

async function captureFailure(operation: Promise<void>): Promise<ForkSettlementWaitError> {
  try {
    await operation
  } catch (error) {
    expect(error).toBeInstanceOf(ForkSettlementWaitError)
    return error as ForkSettlementWaitError
  }
  throw new Error('expected Fork settlement failure')
}

function databaseReturning(
  get: () => object | undefined
): Pick<DatabaseTransaction, 'get'> {
  return {
    get<T extends object>(): T | undefined {
      return get() as T | undefined
    }
  }
}
