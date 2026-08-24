import { describe, expect, it } from 'vitest'

import { parseKookyMutation } from './kooky-bridge'

describe('Kooky bridge contract', () => {
  it('accepts only explicit versioned legacy mutations', () => {
    expect(parseKookyMutation({
      schemaVersion: 1, commandId: 'legacy-1', type: 'panel-updated', timestamp: 1,
      payload: { panelId: 'panel-1', cwd: '/tmp' }
    })).toMatchObject({ commandId: 'legacy-1', type: 'panel-updated' })
    expect(() => parseKookyMutation({ schemaVersion: 1, commandId: 'x', type: 'store-snapshot', timestamp: 1, payload: {} })).toThrow('Unsupported')
  })
})
