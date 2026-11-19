import { describe, expect, it } from 'vitest'
import { DISPATCH_DEFAULTS, dispatchDefaultsSchema } from './dispatch-config'
import { parseRendererMessage, PROTOCOL_VERSION } from './protocol'

describe('dispatch config contract', () => {
  it.each(['dispatch-config.snapshot', 'dispatch-config.save'])('accepts %s RPC requests', method => {
    expect(parseRendererMessage({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'dispatch-1',
      method, capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: method.endsWith('.save') ? { defaults: DISPATCH_DEFAULTS } : {}
    })).toMatchObject({ method })
  })

  it('keeps the script defaults and accepts the full script range', () => {
    expect(DISPATCH_DEFAULTS).toEqual({
      agent: 'codex', model: 'sol', effort: 'high', idleMin: 10, maxMin: 45, askWaitMin: 10, retries: 3
    })
    for (const agent of ['codex', 'agy', 'dsh']) {
      for (const effort of ['low', 'medium', 'high', 'xhigh']) {
        expect(dispatchDefaultsSchema.parse({
          ...DISPATCH_DEFAULTS, agent, effort, model: 'm'.repeat(64),
          idleMin: 0.1, maxMin: 600, askWaitMin: 600, retries: 0, extra: 'discard'
        })).not.toHaveProperty('extra')
      }
    }
  })

  it.each([
    ['agent', 'other'], ['model', ''], ['model', 'a b'], ['model', 'a\nb'],
    ['model', 'a'.repeat(65)], ['effort', 'max'],
    ...['idleMin', 'maxMin', 'askWaitMin'].flatMap(field =>
      [0, -1, 601, NaN, Infinity, '10', null].map(value => [field, value])),
    ['retries', -1], ['retries', 6], ['retries', 1.5], ['retries', '3']
  ])('rejects invalid %s (%s) with its field name', (field, value) => {
    const result = dispatchDefaultsSchema.safeParse({ ...DISPATCH_DEFAULTS, [field as string]: value })
    expect(result.success).toBe(false)
    if (!result.success) expect(result.error.issues[0]?.path).toEqual([field])
  })
})
