import { expect, test } from '@playwright/test'

import {
  activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces
} from './fixtures/session-canvas-fixture'

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
      // Newly created sessions are required to stay in view, so the fifth
      // sibling legitimately leaves the carousel at its right edge. Navigate
      // left first, then prove an ordinary rightward gesture reaches it again.
      await fixture.page.mouse.move(2, 2)
      await fixture.page.waitForTimeout(500)
      const rightEdge = await carousel.evaluate((element) => element.scrollLeft)
      expect(rightEdge).toBeGreaterThan(0)
      await carousel.dispatchEvent('wheel', { deltaX: -650, deltaY: 0 })
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeLessThan(rightEdge)
      await fixture.page.waitForTimeout(300)
      const before = await carousel.evaluate((element) => element.scrollLeft)
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

  test('expands a non-focused card after returning from a real horizontal scroll', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      await fixture.page.getByRole('button', { name: '新建页签' }).click()
      await expect(fixture.page.getByRole('tab')).toHaveCount(2)
      for (let index = 0; index < 5; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
        await expect(visibleSurfaces(fixture.page)).toHaveCount(index + 2)
        await fixture.page.waitForTimeout(250)
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await carousel.hover()
      await fixture.page.mouse.wheel(1_400, 0)
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0)
      await carousel.evaluate((element) => { element.scrollLeft = 0 })

      const target = fixture.page.locator('.session-card:visible').nth(2)
      const before = (await target.boundingBox())!.width
      await target.hover()
      await fixture.page.waitForTimeout(500)

      await expect(target).toHaveClass(/is-expanded/)
      await expect.poll(async () => (await target.boundingBox())!.width).toBeGreaterThan(before + 10)
    } finally {
      await fixture.close()
    }
  })

  test('retargets a stationary pointer within one responsive frame during horizontal scrolling', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1200, 820))
      for (let index = 0; index < 5; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await carousel.evaluate((element) => { element.scrollLeft = 0 })
      const startingCard = fixture.page.locator('.session-card[data-in-viewport="true"]').nth(1)
      const box = await startingCard.boundingBox()
      expect(box).not.toBeNull()
      const point = { x: box!.x + box!.width / 2, y: box!.y + 120 }
      await fixture.page.mouse.move(point.x, point.y)
      const cardAtPoint = async () => fixture.page.evaluate(({ x, y }) =>
        (document.elementFromPoint(x, y)?.closest('[data-session-card]') as HTMLElement | null)
          ?.dataset.sessionCard, point)
      const before = await cardAtPoint()
      expect(before).toBeTruthy()

      const responsePromise = fixture.page.evaluate(({ point, before }) => new Promise<number>((resolve, reject) => {
        const started = performance.now()
        const probe = () => {
          const card = document.elementFromPoint(point.x, point.y)
            ?.closest<HTMLElement>('[data-session-card]')
          if (card?.dataset.sessionCard !== before && card?.classList.contains('is-expanded')) {
            resolve(performance.now() - started)
            return
          }
          if (performance.now() - started > 500) {
            reject(new Error('stationary pointer did not retarget within 500ms'))
            return
          }
          requestAnimationFrame(probe)
        }
        requestAnimationFrame(probe)
      }), { point, before })
      await fixture.page.mouse.wheel(700, 0)
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0)
      const responseMs = await responsePromise
      expect(responseMs).toBeLessThan(100)
      const settled = await cardAtPoint()
      expect(settled).toBeTruthy()
      await expect(fixture.page.locator(`[data-session-card="${settled}"]`)).toHaveClass(/is-expanded/)
    } finally {
      await fixture.close()
    }
  })

  test('moves both directions when the native horizontal gesture starts over a real terminal', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1200, 820))
      for (let index = 0; index < 3; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await carousel.evaluate((element) => { element.scrollLeft = 0 })
      const terminal = fixture.page.locator('.session-card .terminal-surface').first()
      await terminal.hover()

      await fixture.page.mouse.wheel(700, 0)
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0)
      const right = await carousel.evaluate((element) => element.scrollLeft)

      await fixture.page.mouse.wheel(-700, 0)
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeLessThan(right)
    } finally {
      await fixture.close()
    }
  })

  test('keeps the same viewport after repeatedly previewing cards without clicking', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1200, 820))
      for (let index = 0; index < 4; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await expect(carousel).toBeVisible()
      const baseline = await carousel.evaluate((element) => element.scrollLeft)

      for (let attempt = 0; attempt < 3; attempt += 1) {
        const card = fixture.page.locator('.session-card').nth(2)
        const box = await card.boundingBox()
        expect(box).not.toBeNull()
        await fixture.page.mouse.move(box!.x + box!.width / 2, box!.y + 90)
        await expect(card).toHaveClass(/is-expanded/)
        await fixture.page.mouse.move(440, 105)
        await expect(card).not.toHaveClass(/is-expanded/)
        await expect.poll(async () => Math.abs(
          (await carousel.evaluate((element) => element.scrollLeft)) - baseline
        )).toBeLessThan(1)
      }
    } finally {
      await fixture.close()
    }
  })

  test('hands preview between adjacent cards without resetting their widths in between', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1200, 820))
      for (let index = 0; index < 4; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      const visibleCards = fixture.page.locator('.session-card[data-in-viewport="true"]')
      await expect(visibleCards).toHaveCount(3)
      const first = visibleCards.nth(0)
      const next = visibleCards.nth(1)
      await first.hover()
      await expect(first).toHaveClass(/is-expanded/)
      const nextBox = await next.boundingBox()
      expect(nextBox).not.toBeNull()

      const samplesPromise = next.evaluate(async (target) => {
        const cards = [...target.closest('[aria-label="同级会话列表"]')!
          .querySelectorAll<HTMLElement>('.session-card')]
        const samples: Array<{ expandedCount: number; targetExpanded: boolean; targetWidth: number }> = []
        const started = performance.now()
        while (performance.now() - started < 520) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
          samples.push({
            expandedCount: cards.filter((card) => card.classList.contains('is-expanded')).length,
            targetExpanded: target.classList.contains('is-expanded'),
            targetWidth: target.getBoundingClientRect().width
          })
        }
        return samples
      })
      await fixture.page.mouse.move(nextBox!.x + nextBox!.width / 2, nextBox!.y + 90)
      const samples = await samplesPromise

      expect(samples.every(({ expandedCount }) => expandedCount >= 1)).toBe(true)
      const handoff = samples.findIndex(({ targetExpanded }) => targetExpanded)
      expect(handoff).toBeGreaterThanOrEqual(0)
      expect(samples.slice(handoff).every(({ targetExpanded }) => targetExpanded)).toBe(true)
      for (let index = handoff + 1; index < samples.length; index += 1) {
        expect(samples[index]!.targetWidth).toBeGreaterThanOrEqual(samples[index - 1]!.targetWidth - 1)
      }
    } finally {
      await fixture.close()
    }
  })

  test('matches the Mockup fixed-card width transition without resizing siblings', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      for (let index = 0; index < 3; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      const cards = fixture.page.locator('.session-card[data-in-viewport="true"]')
      await expect(cards).toHaveCount(4)
      const target = cards.nth(1)
      const sibling = cards.nth(2)
      await fixture.page.mouse.move(2, 2)
      await expect(target).not.toHaveClass(/is-expanded/)
      await fixture.page.waitForTimeout(450)
      const targetBox = await target.boundingBox()
      expect(targetBox).not.toBeNull()
      const siblingWidth = (await sibling.boundingBox())!.width
      const expectedExpandedWidth = await carousel.evaluate((element) => {
        const style = getComputedStyle(element)
        const innerWidth = element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)
        return Math.min(innerWidth * 0.48, 620)
      })

      const samplesPromise = target.evaluate(async (element) => {
        const sibling = element.closest('[aria-label="同级会话列表"]')!
          .querySelectorAll<HTMLElement>('.session-card')[2]!
        const samples: Array<{ target: number; sibling: number }> = []
        const started = performance.now()
        while (performance.now() - started < 520) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
          samples.push({
            target: element.getBoundingClientRect().width,
            sibling: sibling.getBoundingClientRect().width
          })
        }
        return samples
      })
      await fixture.page.mouse.move(targetBox!.x + targetBox!.width / 2, targetBox!.y + 90)
      const samples = await samplesPromise

      expect(samples.some(({ target: width }) => width > targetBox!.width + 1)).toBe(true)
      expect(Math.max(...samples.map(({ sibling: width }) => width)) -
        Math.min(...samples.map(({ sibling: width }) => width))).toBeLessThan(2)
      expect(samples.at(-1)!.sibling).toBeCloseTo(siblingWidth, 0)
      expect(samples.at(-1)!.target).toBeCloseTo(expectedExpandedWidth, 0)
    } finally {
      await fixture.close()
    }
  })

  test('keeps a focused Shell width monotonic when hover moves from its left sibling', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      for (let index = 0; index < 3; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const cards = fixture.page.locator('.session-card[data-in-viewport="true"]')
      await expect(cards).toHaveCount(4)
      const source = cards.nth(0)
      const target = cards.nth(1)
      await target.locator('.terminal-surface').click({ position: { x: 30, y: 80 } })
      await expect(target).toHaveClass(/is-focused/)
      await fixture.page.mouse.move(440, 105)
      await expect(target).not.toHaveClass(/is-expanded/)
      await source.hover()
      await expect(source).toHaveClass(/is-expanded/)
      await fixture.page.waitForTimeout(450)
      const targetBox = await target.boundingBox()
      expect(targetBox).not.toBeNull()

      const samplesPromise = target.evaluate(async (element) => {
        const viewport = element.closest<HTMLElement>('[aria-label="同级会话列表"]')!
        const samples: Array<{
          width: number; left: number; right: number; scrollLeft: number; expanded: boolean; focused: boolean
        }> = []
        const started = performance.now()
        while (performance.now() - started < 560) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
          samples.push({
            width: element.getBoundingClientRect().width,
            left: element.getBoundingClientRect().left,
            right: element.getBoundingClientRect().right,
            scrollLeft: viewport.scrollLeft,
            expanded: element.classList.contains('is-expanded'),
            focused: element.classList.contains('is-focused')
          })
        }
        return samples
      })
      await fixture.page.mouse.move(targetBox!.x + targetBox!.width / 2, targetBox!.y + 90)
      const samples = await samplesPromise

      expect(samples.every(({ focused }) => focused)).toBe(true)
      const expandedAt = samples.findIndex(({ expanded }) => expanded)
      expect(expandedAt).toBeGreaterThanOrEqual(0)
      expect(samples.slice(expandedAt).every(({ expanded }) => expanded)).toBe(true)
      const reversals = samples.slice(1).filter((sample, index) =>
        sample.width < samples[index]!.width - 1)
      expect(reversals).toEqual([])
      expect(Math.max(...samples.map(({ scrollLeft }) => scrollLeft)) -
        Math.min(...samples.map(({ scrollLeft }) => scrollLeft))).toBeLessThan(1)
    } finally {
      await fixture.close()
    }
  })

  test('keeps the real Claude Code to focused Shell handoff on one visual trajectory', async () => {
    test.setTimeout(120_000)
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      await terminalCommand(activeSurface(fixture.page), 'claude --dangerously-skip-permissions')
      await expect(fixture.page.locator('.pane-title').filter({ hasText: 'Claude' }))
        .toBeVisible({ timeout: 60_000 })
      for (let index = 0; index < 3; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const cards = fixture.page.locator('.session-card[data-in-viewport="true"]')
      await expect(cards).toHaveCount(4)
      const source = cards.nth(0)
      const target = cards.nth(1)
      await expect(source.locator('.pane-title')).toHaveText('Claude')
      await target.locator('.terminal-surface').click({ position: { x: 30, y: 80 } })
      await expect(target).toHaveClass(/is-focused/)
      await fixture.page.mouse.move(440, 105)
      await source.hover()
      await expect(source).toHaveClass(/is-expanded/)
      await fixture.page.waitForTimeout(450)
      const targetBox = await target.boundingBox()
      expect(targetBox).not.toBeNull()

      const samplesPromise = target.evaluate(async (element) => {
        const viewport = element.closest<HTMLElement>('[aria-label="同级会话列表"]')!
        const samples: Array<{
          width: number; left: number; right: number; scrollLeft: number; expanded: boolean
        }> = []
        const started = performance.now()
        while (performance.now() - started < 560) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
          samples.push({
            width: element.getBoundingClientRect().width,
            left: element.getBoundingClientRect().left,
            right: element.getBoundingClientRect().right,
            scrollLeft: viewport.scrollLeft,
            expanded: element.classList.contains('is-expanded')
          })
        }
        return samples
      })
      await fixture.page.mouse.move(targetBox!.x + targetBox!.width / 2, targetBox!.y + 90)
      const samples = await samplesPromise

      const expandedAt = samples.findIndex(({ expanded }) => expanded)
      expect(expandedAt).toBeGreaterThanOrEqual(0)
      expect(samples.slice(expandedAt).every(({ expanded }) => expanded)).toBe(true)
      expect(samples.slice(1).filter((sample, index) =>
        sample.width < samples[index]!.width - 1)).toEqual([])
      expect(samples.slice(1).filter((sample, index) =>
        sample.left > samples[index]!.left + 1)).toEqual([])
      expect(Math.max(...samples.map(({ right }) => right)) -
        Math.min(...samples.map(({ right }) => right))).toBeLessThan(2)
      expect(Math.max(...samples.map(({ scrollLeft }) => scrollLeft)) -
        Math.min(...samples.map(({ scrollLeft }) => scrollLeft))).toBeLessThan(1)
    } finally {
      await fixture.close()
    }
  })
})
