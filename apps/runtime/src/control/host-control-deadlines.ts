import type { HostControlScope } from './host-control-types'

export const HOST_CONTROL_DEFAULT_TIMEOUT_MS = 5_000
export const HOST_CONTROL_FORK_PROVIDER_READY_TIMEOUT_MS = 60_000
export const HOST_CONTROL_FORK_SETTLEMENT_TIMEOUT_MS = 70_000

const HOST_CONTROL_FORK_OPERATION_GRACE_MS = 15_000
const MAX_FORK_BATCH_ITEMS = 50

/**
 * Fork batches start requested sessions serially, then settle create-only
 * sessions concurrently. Keep the client connected for that complete Runtime
 * window rather than reporting a timeout while the durable mutation continues.
 */
export function hostControlResponseTimeoutMs(
  method: HostControlScope,
  params: unknown
): number {
  if (method === 'structure.fork.child' || method === 'structure.fork.sibling') {
    return isRecord(params) && params.start === true
      ? HOST_CONTROL_FORK_OPERATION_GRACE_MS + HOST_CONTROL_FORK_PROVIDER_READY_TIMEOUT_MS
      : HOST_CONTROL_DEFAULT_TIMEOUT_MS
  }
  if (method !== 'structure.fork.children' || !isRecord(params)) {
    return HOST_CONTROL_DEFAULT_TIMEOUT_MS
  }

  const items = Array.isArray(params.items)
    ? params.items.slice(0, MAX_FORK_BATCH_ITEMS)
    : []
  if (items.length === 0) return HOST_CONTROL_DEFAULT_TIMEOUT_MS
  const startCount = items.filter((item) => isRecord(item) && item.start === true).length
  const hasCreateOnlyItem = items.some((item) => !isRecord(item) || item.start !== true)
  return HOST_CONTROL_FORK_OPERATION_GRACE_MS +
    startCount * HOST_CONTROL_FORK_PROVIDER_READY_TIMEOUT_MS +
    (hasCreateOnlyItem ? HOST_CONTROL_FORK_SETTLEMENT_TIMEOUT_MS : 0)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
