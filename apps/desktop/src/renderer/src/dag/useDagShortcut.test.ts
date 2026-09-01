// @vitest-environment jsdom

import { describe, expect, it, vi } from 'vitest'

import { DagShortcutController } from './useDagShortcut'

describe('DagShortcutController', () => {
  it('opens the DAG immediately on the first Option Tab keydown', () => {
    const open = vi.fn()
    const controller = new DagShortcutController({ open })

    expect(controller.keyDown(key('Tab', true))).toBe(true)

    expect(open).toHaveBeenCalledTimes(1)
  })

  it('consumes keyup and ignores automatic key repeats', () => {
    const open = vi.fn()
    const controller = new DagShortcutController({ open })

    expect(controller.keyDown(key('Tab', true))).toBe(true)
    controller.keyDown({ ...key('Tab', true), repeat: true })
    expect(controller.keyUp(key('Tab', true))).toBe(true)

    expect(open).toHaveBeenCalledTimes(1)
  })

  it('leaves ordinary Tab and unrelated shortcuts untouched', () => {
    const open = vi.fn()
    const controller = new DagShortcutController({ open })

    expect(controller.keyDown(key('Tab', false))).toBe(false)
    expect(controller.keyDown(key('Enter', true))).toBe(false)

    expect(open).not.toHaveBeenCalled()
  })
})

function key(value: string, altKey: boolean) {
  return { key: value, altKey, repeat: false, preventDefault: vi.fn(), stopPropagation: vi.fn() }
}
