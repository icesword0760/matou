import { describe, expect, it } from 'vitest'
import { DEFAULT_ZOOM, cursorAt, focusAt } from './zoom'
const id = (x: number, y: number) => ({ x, y })
describe('focusAt', () => {
  it('is identity before the first click', () => {
    expect(focusAt([{ t: 1000, x: 100, y: 100 }], 500, id)).toEqual({ scale: 1, cx: 960, cy: 540 })
  })
  it('reaches full scale after inMs and holds', () => {
    const f = focusAt([{ t: 1000, x: 100, y: 100 }], 1000 + DEFAULT_ZOOM.inMs + 500, id)
    expect(f.scale).toBeCloseTo(DEFAULT_ZOOM.scale, 5); expect(f.cx).toBe(100); expect(f.cy).toBe(100)
  })
  it('returns to 1 after hold + out', () => {
    const t = 1000 + DEFAULT_ZOOM.inMs + DEFAULT_ZOOM.holdMs + DEFAULT_ZOOM.outMs + 1
    expect(focusAt([{ t: 1000, x: 100, y: 100 }], t, id).scale).toBe(1)
  })
  it('pans instead of releasing when the next click is inside mergeGap', () => {
    const clicks = [{ t: 1000, x: 100, y: 100 }, { t: 2500, x: 900, y: 500 }]
    const f = focusAt(clicks, 2500 + DEFAULT_ZOOM.inMs, id)
    expect(f.scale).toBeCloseTo(DEFAULT_ZOOM.scale, 5); expect(f.cx).toBe(900); expect(f.cy).toBe(500)
  })
  it('is monotone during ease-in', () => {
    const a = focusAt([{ t: 0, x: 0, y: 0 }], 100, id).scale, b = focusAt([{ t: 0, x: 0, y: 0 }], 300, id).scale
    expect(b).toBeGreaterThan(a)
  })
})
describe('cursorAt', () => {
  const events = [
    { t: 0, type: 'move', x: 0, y: 0, ms: 0 },
    { t: 1000, type: 'move', x: 200, y: 100, ms: 400 },
    { t: 1400, type: 'click', x: 200, y: 100 }
  ] as const
  it('interpolates between moves', () => {
    const c = cursorAt([...events], 1200)!
    expect(c.x).toBeGreaterThan(0); expect(c.x).toBeLessThan(200); expect(c.pressed).toBe(false)
  })
  it('is pressed for 120ms after a click', () => { expect(cursorAt([...events], 1450)!.pressed).toBe(true); expect(cursorAt([...events], 1600)!.pressed).toBe(false) })
  it('is null before any event', () => { expect(cursorAt([], 10)).toBeNull() })
})
