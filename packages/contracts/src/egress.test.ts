import { describe, expect, it } from 'vitest'

import { EGRESS_FAMILIES, isEgressFamily } from './egress'
import { PROTOCOL_VERSION, RPC_METHODS, type RuntimeMessage } from './protocol'

describe('egress contracts', () => {
  it('validates supported egress families with isEgressFamily', () => {
    expect(EGRESS_FAMILIES).toEqual(['anthropic', 'openai', 'chatgpt-subscription'])
    expect(isEgressFamily('anthropic')).toBe(true)
    expect(isEgressFamily('openai')).toBe(true)
    expect(isEgressFamily('chatgpt-subscription')).toBe(true)
    expect(isEgressFamily('google')).toBe(false)
    expect(isEgressFamily(undefined)).toBe(false)
  })

  it('allowlists all six egress RPC methods', () => {
    expect(RPC_METHODS).toEqual(expect.arrayContaining([
      'egress.status',
      'egress.recheck',
      'egress.set-rule',
      'egress.accept-current',
      'egress.set-protection',
      'egress.override'
    ]))
  })

  it('includes egress.status in RuntimeMessage', () => {
    const message = {
      type: 'egress.status',
      protocolVersion: PROTOCOL_VERSION,
      status: {
        state: 'checking',
        families: [],
        checks: [],
        ruleMode: 'country',
        protectionEnabled: true,
        sessionFamilies: {}
      }
    } satisfies RuntimeMessage

    expect(message.type).toBe('egress.status')
  })
})
