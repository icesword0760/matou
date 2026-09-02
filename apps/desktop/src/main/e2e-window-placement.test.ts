import { describe, expect, it } from 'vitest'

import { secondaryDisplayWindowBounds } from './e2e-window-placement'

describe('secondaryDisplayWindowBounds', () => {
  const displays = [
    { id: 1, workArea: { x: 0, y: 0, width: 1728, height: 1080 } },
    { id: 2, workArea: { x: 1728, y: 24, width: 1440, height: 900 } }
  ]

  it('centers automated app windows on the non-primary display', () => {
    expect(secondaryDisplayWindowBounds({
      enabled: true, width: 1200, height: 780, primaryDisplayId: 1, displays
    })).toEqual({ x: 1848, y: 84 })
  })

  it('prefers the internal Color LCD when several non-primary displays exist', () => {
    expect(secondaryDisplayWindowBounds({
      enabled: true,
      width: 1200,
      height: 780,
      primaryDisplayId: 1,
      displays: [
        { id: 1, label: 'XV272U', internal: false,
          workArea: { x: 0, y: 0, width: 2560, height: 1440 } },
        { id: 2, label: 'Studio Display', internal: false,
          workArea: { x: -1920, y: 0, width: 1920, height: 1080 } },
        { id: 3, label: 'Color LCD', internal: true,
          workArea: { x: 2560, y: 0, width: 1512, height: 982 } }
      ]
    })).toEqual({ x: 2716, y: 101 })
  })

  it('leaves normal product windows and single-display machines unchanged', () => {
    expect(secondaryDisplayWindowBounds({
      enabled: false, width: 1200, height: 780, primaryDisplayId: 1, displays
    })).toBeUndefined()
    expect(secondaryDisplayWindowBounds({
      enabled: true, width: 1200, height: 780, primaryDisplayId: 1, displays: displays.slice(0, 1)
    })).toBeUndefined()
  })
})
