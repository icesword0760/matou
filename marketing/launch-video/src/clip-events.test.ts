import { describe, expect, it } from 'vitest'
import { clamp, isClick, startsZoom, type ClickEvent } from './clip-events'

const click = (label?: string): ClickEvent => ({ t: 0, type: 'click', x: 10, y: 10, ...(label ? { label } : {}) })

describe('startsZoom', () => {
  it('drops clicks that hand the frame to another window', () => {
    expect(startsZoom(click('dag'))).toBe(false)
    expect(startsZoom(click('dag-node'))).toBe(false)
  })
  it('keeps every other labelled click', () => {
    for (const label of ['card', 'tab', 'fork', 'board', 'new-shell', 'notify']) {
      expect(startsZoom(click(label))).toBe(true)
    }
  })
  it('keeps a click with no label at all', () => {
    expect(startsZoom(click())).toBe(true)
  })
})

describe('isClick', () => {
  it('narrows clicks out of a mixed event log', () => {
    const events = [
      { t: 0, type: 'move', x: 1, y: 1, ms: 0 },
      click('card'),
      { t: 5, type: 'mark', name: 'hud' },
      { t: 6, type: 'source', source: 'dag' }
    ] as const
    expect([...events].filter(isClick)).toEqual([click('card')])
  })
})

describe('clamp', () => {
  it('leaves a value inside the range alone', () => {
    expect(clamp(700, 1504)).toBe(700)
  })
  it('pulls both ends into the range', () => {
    expect(clamp(2293, 1504)).toBe(1504)
    expect(clamp(-40, 1504)).toBe(0)
  })
  it('keeps the bounds themselves', () => {
    expect(clamp(0, 1504)).toBe(0)
    expect(clamp(1504, 1504)).toBe(1504)
  })
})
