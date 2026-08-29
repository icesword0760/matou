import { expect, test } from '@playwright/test'

import { launchSessionCanvas, visibleSurfaces } from './fixtures/session-canvas-fixture'

test.describe('horizontal sibling navigation', () => {
  test.setTimeout(60_000)

  test('shows at most four siblings, reaches the fifth horizontally, and expands only the hovered card', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      for (let index = 0; index < 4; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
        await expect(visibleSurfaces(fixture.page)).toHaveCount(index + 2)
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await expect(carousel).toHaveAttribute('data-visible-columns', '4')
      const before = await carousel.evaluate((element) => element.scrollLeft)
      await carousel.hover()
      await carousel.dispatchEvent('wheel', { deltaX: 650, deltaY: 0 })
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeGreaterThan(before)

      const last = fixture.page.getByLabel('会话：Shell').last()
      await fixture.page.waitForTimeout(150)
      await fixture.page.mouse.move(2, 2)
      await last.hover()
      await expect(last).toHaveClass(/is-expanded/)
      await carousel.dispatchEvent('wheel', { deltaX: -650, deltaY: 0 })
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeLessThan(650)
    } finally {
      await fixture.close()
    }
  })
})
