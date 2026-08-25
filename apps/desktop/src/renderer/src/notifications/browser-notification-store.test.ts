// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { createBrowserNotificationStore, NOTIFICATION_SOUND_STORAGE_KEY } from './browser-notification-store'
import { playNotificationSound } from './notification-sound'

describe('browser notification preferences', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('uses the Kooky sound preference key and defaults to enabled', () => {
    const store = createBrowserNotificationStore({ playSound: vi.fn() })
    expect(store.snapshot().soundEnabled).toBe(true)

    store.setSoundEnabled(false)
    expect(localStorage.getItem(NOTIFICATION_SOUND_STORAGE_KEY)).toBe('false')

    const restored = createBrowserNotificationStore({ playSound: vi.fn() })
    expect(restored.snapshot().soundEnabled).toBe(false)
  })

  it('plays the same two-tone WebAudio cue as Kooky', () => {
    const setValueAtTime = vi.fn()
    const exponentialRampToValueAtTime = vi.fn()
    const start = vi.fn()
    const stop = vi.fn()
    const oscillator = {
      connect: vi.fn(), type: '', frequency: { setValueAtTime }, start, stop
    }
    const gain = {
      connect: vi.fn(), gain: { setValueAtTime: vi.fn(), exponentialRampToValueAtTime }
    }
    const context = {
      currentTime: 12, destination: {}, createOscillator: () => oscillator, createGain: () => gain
    }

    playNotificationSound(() => context)

    expect(oscillator.type).toBe('sine')
    expect(setValueAtTime).toHaveBeenNthCalledWith(1, 880, 12)
    expect(setValueAtTime).toHaveBeenNthCalledWith(2, 1047, 12.1)
    expect(gain.gain.setValueAtTime).toHaveBeenCalledWith(0.3, 12)
    expect(exponentialRampToValueAtTime).toHaveBeenCalledWith(0.001, 12.3)
    expect(start).toHaveBeenCalledWith(12)
    expect(stop).toHaveBeenCalledWith(12.3)
  })
})
