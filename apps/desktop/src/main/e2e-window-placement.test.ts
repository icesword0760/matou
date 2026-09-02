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

  it('leaves normal product windows and single-display machines unchanged', () => {
    expect(secondaryDisplayWindowBounds({
      enabled: false, width: 1200, height: 780, primaryDisplayId: 1, displays
    })).toBeUndefined()
    expect(secondaryDisplayWindowBounds({
      enabled: true, width: 1200, height: 780, primaryDisplayId: 1, displays: displays.slice(0, 1)
    })).toBeUndefined()
  })
})
