import { expect, test } from '@playwright/test'

import {
  activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces
} from './fixtures/session-canvas-fixture'

test.describe('horizontal sibling navigation', () => {
  test.setTimeout(60_000)

  test('shows at most four siblings, reaches the fifth horizontally, and keeps the active card expanded', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      for (let index = 0; index < 4; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
        await expect(visibleSurfaces(fixture.page)).toHaveCount(index + 2)
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await expect(carousel).toHaveAttribute('data-visible-columns', '4')
      await expect(fixture.page.locator('.session-card.is-focused')).toHaveClass(/is-expanded/)
      // Session ordering may settle asynchronously as the Runtime publishes
      // the newly focused node. Prove overflow and both directions from an
      // explicit viewport baseline instead of assuming which edge owns focus.
      await fixture.page.mouse.move(2, 2)
      await fixture.page.waitForTimeout(500)
      expect(await carousel.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeGreaterThan(0)
      await carousel.evaluate((element) => { element.scrollLeft = 0 })
      await carousel.dispatchEvent('wheel', { deltaX: 650, deltaY: 0 })
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0)
      const right = await carousel.evaluate((element) => element.scrollLeft)
      await carousel.dispatchEvent('wheel', { deltaX: -650, deltaY: 0 })
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeLessThan(right)

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
      // Anchor near the card's leading edge. Its right edge grows during the
      // dual active + hover expansion, while this point stays on the same card.
      const point = { x: box!.x + 40, y: box!.y + 120 }
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

  test('reaches the far left while the pointer remains over an expanded right card', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1200, 820))
      for (let index = 0; index < 3; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await carousel.evaluate((element) => {
        element.scrollLeft = element.scrollWidth - element.clientWidth
      })
      await fixture.page.waitForTimeout(150)
      const viewportBox = await carousel.boundingBox()
      expect(viewportBox).not.toBeNull()
      const point = {
        x: viewportBox!.x + viewportBox!.width - 80,
        y: viewportBox!.y + 100
      }
      const rightCardId = await fixture.page.evaluate(({ x, y }) =>
        (document.elementFromPoint(x, y)?.closest('[data-session-card]') as HTMLElement | null)
          ?.dataset.sessionCard, point)
      expect(rightCardId).toBeTruthy()
      const rightCardSlot = fixture.page.locator(`.session-card-slot[data-session-id="${rightCardId}"]`)
      await fixture.page.mouse.move(point.x, point.y)
      await expect(rightCardSlot).toHaveClass(/is-expanded/)
      await expect(fixture.page.locator('.session-card-slot.is-focused')).toHaveClass(/is-expanded/)
      const rightEdge = await carousel.evaluate((element) => element.scrollLeft)
      expect(rightEdge).toBeGreaterThan(0)

      for (let index = 0; index < 6; index += 1) {
        await fixture.page.mouse.wheel(-360, 0)
        await fixture.page.waitForTimeout(40)
      }
      await fixture.page.waitForTimeout(500)

      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBeLessThanOrEqual(1)
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
      const carouselBox = await carousel.boundingBox()
      expect(carouselBox).not.toBeNull()

      for (let attempt = 0; attempt < 3; attempt += 1) {
        const card = fixture.page.locator('.session-card').nth(2)
        const box = await card.boundingBox()
        expect(box).not.toBeNull()
        await fixture.page.mouse.move(box!.x + box!.width / 2, box!.y + 90)
        await expect(card).toHaveClass(/is-expanded/)
        await fixture.page.mouse.move(carouselBox!.x + 20, Math.max(1, carouselBox!.y - 20))
        await expect(card).not.toHaveClass(/is-expanded/)
        await expect.poll(async () => Math.abs(
          (await carousel.evaluate((element) => element.scrollLeft)) - baseline
        )).toBeLessThan(1)
      }
    } finally {
      await fixture.close()
    }
  })

  test('slides an edge-hovered card fully into view while it expands', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1200, 820))
      for (let index = 0; index < 4; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await fixture.page.mouse.move(2, 2)
      await fixture.page.waitForTimeout(450)
      await carousel.evaluate((element) => {
        const slot = element.querySelectorAll<HTMLElement>('[data-session-id]')[1]!
        element.scrollLeft = slot.offsetLeft + 80
      })
      const viewportBox = await carousel.boundingBox()
      expect(viewportBox).not.toBeNull()
      const point = { x: viewportBox!.x + 28, y: viewportBox!.y + 120 }
      const targetId = await fixture.page.evaluate(({ x, y }) =>
        (document.elementFromPoint(x, y)?.closest('[data-session-card]') as HTMLElement | null)
          ?.dataset.sessionCard, point)
      expect(targetId).toBeTruthy()
      const target = fixture.page.locator(`[data-session-card="${targetId}"]`)
      const initialLeftGap = await target.evaluate((card) => {
        const viewport = card.closest<HTMLElement>('[aria-label="同级会话列表"]')!
        return card.getBoundingClientRect().left - viewport.getBoundingClientRect().left
      })
      expect(initialLeftGap).toBeLessThan(8)

      await fixture.page.mouse.move(point.x, point.y)
      await expect(target).toHaveClass(/is-expanded/)
      await fixture.page.waitForTimeout(480)

      const visibility = await target.evaluate((card) => {
        const viewport = card.closest<HTMLElement>('[aria-label="同级会话列表"]')!
        const viewportRect = viewport.getBoundingClientRect()
        const cardRect = card.getBoundingClientRect()
        return {
          leftGap: cardRect.left - viewportRect.left,
          rightGap: viewportRect.right - cardRect.right
        }
      })
      expect(visibility.leftGap).toBeGreaterThanOrEqual(8)
      expect(visibility.rightGap).toBeGreaterThanOrEqual(8)
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
      const firstId = await visibleCards.nth(0).getAttribute('data-session-card')
      const nextId = await visibleCards.nth(1).getAttribute('data-session-card')
      expect(firstId).toBeTruthy()
      expect(nextId).toBeTruthy()
      // Freeze identity before expansion updates the derived in-viewport set.
      const first = fixture.page.locator(`[data-session-card="${firstId}"]`)
      const next = fixture.page.locator(`[data-session-card="${nextId}"]`)
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

  test('keeps the active Shell expanded while a hovered sibling also expands temporarily', async () => {
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
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      const carouselBox = await carousel.boundingBox()
      expect(carouselBox).not.toBeNull()
      await fixture.page.mouse.move(carouselBox!.x + 20, Math.max(1, carouselBox!.y - 20))
      await expect(target).toHaveClass(/is-expanded/)
      await source.hover()
      await expect(source).toHaveClass(/is-expanded/)
      await expect(target).toHaveClass(/is-expanded/)
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

      await fixture.page.mouse.move(carouselBox!.x + 20, Math.max(1, carouselBox!.y - 20))
      await expect(target).toHaveClass(/is-expanded/)
    } finally {
      await fixture.close()
    }
  })

  test('keeps a real focused Shell expanded while Claude hover preview settles on one trajectory', async () => {
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

      expect(samples.every(({ expanded }) => expanded)).toBe(true)
      expect(Math.max(...samples.map(({ width }) => width)) -
        Math.min(...samples.map(({ width }) => width))).toBeLessThan(2)
      expect(samples.slice(1).filter((sample, index) =>
        sample.left > samples[index]!.left + 1)).toEqual([])
      expect(Math.max(...samples.map(({ scrollLeft }) => scrollLeft)) -
        Math.min(...samples.map(({ scrollLeft }) => scrollLeft))).toBeLessThan(1)
    } finally {
      await fixture.close()
    }
  })

  test('activates a far-right preview without width or position reversals', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      for (let index = 0; index < 2; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const cards = fixture.page.locator('.session-card')
      await expect(cards).toHaveCount(3)
      const first = cards.first()
      const target = cards.last()
      await first.locator('.terminal-surface').click({ position: { x: 30, y: 80 } })
      await expect(first).toHaveClass(/is-focused/)
      await fixture.page.mouse.move(2, 2)
      await fixture.page.waitForTimeout(520)

      const targetBox = await target.boundingBox()
      const carouselBox = await fixture.page.getByRole('region', { name: '同级会话列表' }).boundingBox()
      expect(targetBox).not.toBeNull()
      expect(carouselBox).not.toBeNull()
      const clickPoint = {
        x: Math.min(targetBox!.x + 30, carouselBox!.x + carouselBox!.width - 20),
        y: targetBox!.y + 80
      }
      const samplesPromise = target.evaluate(async (element) => {
        const viewport = element.closest<HTMLElement>('[aria-label="同级会话列表"]')!
        const source = viewport.querySelector<HTMLElement>('.session-card.is-focused')!
        const samples: Array<{
          targetWidth: number
          targetLeft: number
          sourceWidth: number
          scrollLeft: number
          targetFocused: boolean
        }> = []
        const started = performance.now()
        while (performance.now() - started < 760) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
          const targetRect = element.getBoundingClientRect()
          samples.push({
            targetWidth: targetRect.width,
            targetLeft: targetRect.left,
            sourceWidth: source.getBoundingClientRect().width,
            scrollLeft: viewport.scrollLeft,
            targetFocused: element.classList.contains('is-focused')
          })
        }
        return samples
      })
      await fixture.page.mouse.move(clickPoint.x, clickPoint.y)
      await fixture.page.mouse.down()
      await fixture.page.mouse.up()
      const samples = await samplesPromise
      await expect(target).toHaveClass(/is-focused/)

      expect(directionReversals(samples.map(({ targetWidth }) => targetWidth), 1)).toBe(0)
      expect(directionReversals(samples.map(({ targetLeft }) => targetLeft), 1)).toBe(0)
      expect(Math.max(...samples.map(({ scrollLeft }) => scrollLeft)) -
        Math.min(...samples.map(({ scrollLeft }) => scrollLeft))).toBeLessThan(12)
      const focusedAt = samples.findIndex(({ targetFocused }) => targetFocused)
      expect(focusedAt).toBeGreaterThanOrEqual(0)
      expect(directionReversals(samples.slice(focusedAt).map(({ sourceWidth }) => sourceWidth), 1))
        .toBe(0)
    } finally {
      await fixture.close()
    }
  })
})

function directionReversals(values: number[], tolerance: number): number {
  let direction = 0
  let reversals = 0
  for (let index = 1; index < values.length; index += 1) {
    const delta = values[index]! - values[index - 1]!
    const nextDirection = delta > tolerance ? 1 : delta < -tolerance ? -1 : 0
    if (nextDirection === 0) continue
    if (direction !== 0 && nextDirection !== direction) reversals += 1
    direction = nextDirection
  }
  return reversals
}
