import { expect, test } from '@playwright/test'

import { activeSurface, launchSessionCanvas } from './fixtures/session-canvas-fixture'

test('keeps the focused terminal canvas aligned to physical pixels', async () => {
  const fixture = await launchSessionCanvas()
  try {
    const surface = activeSurface(fixture.page)
    const card = surface.locator('xpath=ancestor::*[contains(@class,"session-card")][1]')
    const canvas = surface.locator('canvas').first()
    await expect(canvas).toBeVisible()

    const rendering = await canvas.evaluate((element) => {
      const rect = element.getBoundingClientRect()
      const transform = getComputedStyle(element.closest('.session-card')!).transform
      const matrix = new DOMMatrixReadOnly(transform)
      return {
        cardScaleX: matrix.a,
        cardScaleY: matrix.d,
        renderedPixelRatioX: element.width / rect.width,
        renderedPixelRatioY: element.height / rect.height,
        devicePixelRatio: window.devicePixelRatio
      }
    })

    await expect(card).toHaveAttribute('class', /is-focused/)
    expect(rendering.cardScaleX).toBe(1)
    expect(rendering.cardScaleY).toBe(1)
    expect(rendering.renderedPixelRatioX).toBeCloseTo(rendering.devicePixelRatio, 2)
    expect(rendering.renderedPixelRatioY).toBeCloseTo(rendering.devicePixelRatio, 2)
  } finally {
    await fixture.close()
  }
})
