import { describe, expect, it } from 'vitest'

import { toOscNotification } from './osc-notification'

describe('Kooky OSC terminal notifications', () => {
  it('maps OSC 9, 99, and 777 into the same Terminal notification category', () => {
    for (const oscId of [9, 99, 777]) {
      expect(toOscNotification(oscId, '  build finished\n successfully  ')).toEqual({
        eventType: 'osc-notification', title: 'Claude Code', subtitle: 'Terminal',
        body: 'build finished successfully', sound: true, cooldownKey: 'OSCNotification'
      })
    }
  })

  it('ignores other OSC codes and uses Kooky fallback content', () => {
    expect(toOscNotification(7, 'file:///tmp')).toBeNull()
    expect(toOscNotification(9, '')?.body).toBe('终端通知')
  })

  it('bounds raw terminal content to the Kooky 180-character body', () => {
    const body = toOscNotification(777, 'x'.repeat(300))?.body
    expect(body).toHaveLength(180)
    expect(body?.endsWith('…')).toBe(true)
  })
})
