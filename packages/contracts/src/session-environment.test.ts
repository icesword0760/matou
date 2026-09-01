import { describe, expect, it } from 'vitest'

import { parseRendererMessage, PROTOCOL_VERSION } from './protocol'

describe('session environment RPC contracts', () => {
  it.each([
    'session.environment-open',
    'session.environment-restore',
    'session.environment-locate',
    'session.environment-handoff'
  ] as const)('accepts %s as a renderer RPC method', (method) => {
    expect(parseRendererMessage({
      type: 'rpc.request',
      protocolVersion: PROTOCOL_VERSION,
      requestId: `request-${method}`,
      method,
      capability: 'renderer',
      deadlineAt: Date.now() + 1_000,
      payload: {}
    })).toMatchObject({ method })
  })
})
