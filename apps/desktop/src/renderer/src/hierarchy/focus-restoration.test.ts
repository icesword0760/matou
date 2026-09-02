// @vitest-environment jsdom

import { describe, expect, it, vi } from 'vitest'

import { AppFocusRestorer, restorableFocusTarget } from './focus-restoration'

describe('AppFocusRestorer', () => {
  it('returns to the exact input the user was editing', () => {
    const frames: FrameRequestCallback[] = []
    const fallback = vi.fn()
    const restorer = new AppFocusRestorer((callback) => {
      frames.push(callback)
      return frames.length
    }, vi.fn())
    const input = document.createElement('input')
    document.body.append(input)
    input.focus()
    restorer.remember(input)
    document.body.focus()

    restorer.scheduleRestore(fallback)
    frames.shift()!(0)

    expect(document.activeElement).toBe(input)
    expect(fallback).not.toHaveBeenCalled()
  })

  it('coalesces focus and visibility events into one restoration frame', () => {
    const frames: FrameRequestCallback[] = []
    const restorer = new AppFocusRestorer((callback) => {
      frames.push(callback)
      return frames.length
    }, vi.fn())
    const input = document.createElement('input')
    document.body.append(input)
    restorer.remember(input)

    restorer.scheduleRestore(vi.fn())
    restorer.scheduleRestore(vi.fn())

    expect(frames).toHaveLength(1)
  })

  it('focuses the active terminal only after the original control has closed', () => {
    const frames: FrameRequestCallback[] = []
    const fallback = vi.fn()
    const restorer = new AppFocusRestorer((callback) => {
      frames.push(callback)
      return frames.length
    }, vi.fn())
    const forkInput = document.createElement('input')
    document.body.append(forkInput)
    restorer.remember(forkInput)
    forkInput.remove()

    restorer.scheduleRestore(fallback)
    frames.shift()!(0)

    expect(fallback).toHaveBeenCalledTimes(1)
  })

  it('does not restore disabled or hidden controls', () => {
    const disabled = document.createElement('button')
    disabled.disabled = true
    document.body.append(disabled)
    const hiddenInput = document.createElement('input')
    hiddenInput.setAttribute('aria-hidden', 'true')
    document.body.append(hiddenInput)

    expect(restorableFocusTarget(disabled)).toBe(false)
    expect(restorableFocusTarget(hiddenInput)).toBe(false)
  })
})
