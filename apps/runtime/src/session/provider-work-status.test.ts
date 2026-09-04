import { describe, expect, it } from 'vitest'

import { nextProviderWorkStatus } from './provider-work-status'

describe('nextProviderWorkStatus', () => {
  it('keeps a terminal provider failure visible until the user starts a new attempt', () => {
    expect(nextProviderWorkStatus('error', 'completed')).toBe('error')
    expect(nextProviderWorkStatus('error', 'waiting')).toBe('error')
    expect(nextProviderWorkStatus('running', 'completed')).toBe('idle')
    expect(nextProviderWorkStatus('running', 'error')).toBe('needs-input')
    expect(nextProviderWorkStatus('running', 'permission')).toBe('needs-input')
  })
})
