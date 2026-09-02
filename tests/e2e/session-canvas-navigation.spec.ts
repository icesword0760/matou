import { expect, test } from '@playwright/test'

import {
  activeSurface, launchSessionCanvas, readText, terminalCommand, visibleSurfaces
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

  test('browses the nearest hidden card from the right edge and stops when intent leaves', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1200, 820))
      for (let index = 0; index < 5; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await fixture.page.mouse.move(2, 2)
      await fixture.page.waitForTimeout(600)
      await expect.poll(() => carousel.evaluate((viewport) =>
        viewport.scrollWidth - viewport.clientWidth
      )).toBeGreaterThan(200)
      await carousel.evaluate((viewport) => { viewport.scrollLeft = 0 })
      await fixture.page.waitForTimeout(80)
      const right = await carousel.evaluate((viewport) => {
        const viewportRect = viewport.getBoundingClientRect()
        const hiddenCard = [...viewport.querySelectorAll<HTMLElement>('[data-session-card]')]
          .filter((card) => card.getBoundingClientRect().right > viewportRect.right - 10)
          .sort((leftCard, rightCard) =>
            leftCard.getBoundingClientRect().left - rightCard.getBoundingClientRect().left
          )[0]!
        return {
          sessionId: hiddenCard.dataset.sessionCard!,
          before: viewport.scrollLeft,
          edgePoint: { x: viewportRect.right - 20, y: viewportRect.top + 100 }
        }
      })
      expect(right.sessionId).toBeTruthy()
      const hiddenCard = fixture.page.locator(`[data-session-card="${right.sessionId}"]`)

      await fixture.page.mouse.move(right.edgePoint.x, right.edgePoint.y)

      await expect(carousel).toHaveAttribute('data-edge-browse-direction', 'right')
      await expect.poll(() => carousel.evaluate((viewport) => viewport.scrollLeft))
        .toBeGreaterThan(right.before)
      await expect.poll(() => hiddenCard.evaluate((card) => {
        const viewport = card.closest<HTMLElement>('[aria-label="同级会话列表"]')!
        const viewportRect = viewport.getBoundingClientRect()
        const cardRect = card.getBoundingClientRect()
        return Math.min(viewportRect.right, cardRect.right) - Math.max(viewportRect.left, cardRect.left)
      })).toBeGreaterThan(300)

      await fixture.page.mouse.move(right.edgePoint.x - 180, right.edgePoint.y)
      await expect(carousel).toHaveAttribute('data-edge-browse-phase', 'idle')
      await fixture.page.waitForTimeout(1_050)
      await expect(carousel).toHaveAttribute('data-edge-browse-phase', 'idle')
      await expect(carousel).toHaveAttribute('data-edge-browse-direction', 'none')
    } finally {
      await fixture.close()
    }
  })

  test('reveals the nearest hidden card when the pointer dwells at the left edge', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1200, 820))
      for (let index = 0; index < 5; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await fixture.page.mouse.move(2, 2)
      await fixture.page.waitForTimeout(600)
      await expect.poll(() => carousel.evaluate((viewport) =>
        viewport.scrollWidth - viewport.clientWidth
      )).toBeGreaterThan(200)
      await carousel.evaluate((viewport) => {
        viewport.scrollLeft = viewport.scrollWidth - viewport.clientWidth
      })
      await fixture.page.waitForTimeout(80)
      const left = await carousel.evaluate((viewport) => {
        const viewportRect = viewport.getBoundingClientRect()
        const hiddenCard = [...viewport.querySelectorAll<HTMLElement>('[data-session-card]')]
          .filter((card) => card.getBoundingClientRect().left < viewportRect.left + 10)
          .sort((leftCard, rightCard) =>
            rightCard.getBoundingClientRect().left - leftCard.getBoundingClientRect().left
          )[0]!
        return {
          sessionId: hiddenCard.dataset.sessionCard!,
          before: viewport.scrollLeft,
          edgePoint: { x: viewportRect.left + 20, y: viewportRect.top + 100 }
        }
      })
      expect(left.sessionId).toBeTruthy()
      const hiddenCard = fixture.page.locator(`[data-session-card="${left.sessionId}"]`)

      await fixture.page.mouse.move(left.edgePoint.x, left.edgePoint.y)

      await expect(carousel).toHaveAttribute('data-edge-browse-direction', 'left')
      await expect(hiddenCard).toHaveClass(/is-expanded/, { timeout: 2_000 })
      await expect.poll(() => carousel.evaluate((viewport) => viewport.scrollLeft))
        .toBeLessThan(left.before)
      await expect.poll(() => hiddenCard.evaluate((card) => {
        const viewport = card.closest<HTMLElement>('[aria-label="同级会话列表"]')!
        const viewportRect = viewport.getBoundingClientRect()
        const cardRect = card.getBoundingClientRect()
        return Math.min(viewportRect.right, cardRect.right) - Math.max(viewportRect.left, cardRect.left)
      })).toBeGreaterThan(300)
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
      await expect.poll(() => carousel.evaluate((element) => element.scrollLeft)).toBe(0)
      // Session creation has a short focus-follow animation. Select a card by
      // its real clipped rectangle after that motion settles; data-in-viewport
      // is an index hint for virtualization, not a pointer hit-test oracle.
      await fixture.page.waitForTimeout(500)
      const startingSessionId = await carousel.evaluate((viewport) => {
        const viewportRect = viewport.getBoundingClientRect()
        return [...viewport.querySelectorAll<HTMLElement>('[data-session-card]')]
          .filter((card) => {
            const rect = card.getBoundingClientRect()
            return Math.min(viewportRect.right, rect.right) - Math.max(viewportRect.left, rect.left) > 200
          })[1]?.dataset.sessionCard
      })
      expect(startingSessionId).toBeTruthy()
      const startingCard = fixture.page.locator(`[data-session-card="${startingSessionId}"]`)
      await startingCard.hover()
      await expect(startingCard).toHaveClass(/is-expanded/)
      await fixture.page.waitForTimeout(250)
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
      // Let the last newly focused Session finish its own visibility handoff;
      // the assertion below is about hover previews, not creation navigation.
      await fixture.page.waitForTimeout(500)
      const baseline = await carousel.evaluate((element) => element.scrollLeft)
      const carouselBox = await carousel.boundingBox()
      expect(carouselBox).not.toBeNull()
      const previewCards = fixture.page.locator('.session-card:not(.is-focused)')
      let previewIndex = -1
      for (let index = 0; index < await previewCards.count(); index += 1) {
        const box = await previewCards.nth(index).boundingBox()
        if (box && box.x + box.width > carouselBox!.x + 80
          && box.x < carouselBox!.x + carouselBox!.width - 80) {
          previewIndex = index
          break
        }
      }
      expect(previewIndex).toBeGreaterThanOrEqual(0)

      for (let attempt = 0; attempt < 3; attempt += 1) {
        const card = previewCards.nth(previewIndex)
        const box = await card.boundingBox()
        expect(box).not.toBeNull()
        const hoverX = Math.min(
          carouselBox!.x + carouselBox!.width - 90,
          Math.max(carouselBox!.x + 90, box!.x + box!.width / 2)
        )
        await fixture.page.mouse.move(hoverX, box!.y + 90)
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
      const nonFocused = fixture.page.locator(
        '.session-card[data-in-viewport="true"]:not(.is-focused)'
      )
      await expect(nonFocused).toHaveCount(3)
      await fixture.page.mouse.move(2, 2)
      const carouselBox = await carousel.boundingBox()
      expect(carouselBox).not.toBeNull()
      const candidates = await nonFocused.evaluateAll((elements) => elements.map((element) => {
        const rect = element.getBoundingClientRect()
        return { id: element.getAttribute('data-session-card'), left: rect.left, right: rect.right }
      }))
      const fullyVisible = candidates.filter(({ left, right }) =>
        left >= carouselBox!.x && right <= carouselBox!.x + carouselBox!.width)
      expect(fullyVisible.length).toBeGreaterThanOrEqual(2)
      const target = fixture.page.locator(
        `.session-card[data-session-card="${fullyVisible.at(-1)!.id}"]`
      )
      const sibling = fixture.page.locator(
        `.session-card[data-session-card="${fullyVisible.at(-2)!.id}"]`
      )
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

      const siblingId = await sibling.getAttribute('data-session-card')
      const samplesPromise = target.evaluate(async (element, id) => {
        const sibling = element.closest('[aria-label="同级会话列表"]')!
          .querySelector<HTMLElement>(`[data-session-card="${id}"]`)!
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
      }, siblingId)
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
      const targetId = await cards.nth(1).getAttribute('data-session-card')
      const target = fixture.page.locator(`.session-card[data-session-card="${targetId}"]`)
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
      // If this focused card is also the visible tail, the strip now advances
      // once to expose the next pointer target. That motion must remain a
      // single trajectory rather than jittering back and forth.
      expect(directionReversals(samples.map(({ scrollLeft }) => scrollLeft), 1)).toBe(0)

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
      const targetId = await cards.nth(1).getAttribute('data-session-card')
      const target = fixture.page.locator(`.session-card[data-session-card="${targetId}"]`)
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
      expect(directionReversals(samples.map(({ scrollLeft }) => scrollLeft), 1)).toBe(0)
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
      const firstSessionId = await cards.first().getAttribute('data-session-card')
      const targetSessionId = await cards.last().getAttribute('data-session-card')
      expect(firstSessionId).toBeTruthy()
      expect(targetSessionId).toBeTruthy()
      const first = fixture.page.locator(`[data-session-card="${firstSessionId}"]`)
      const target = fixture.page.locator(`[data-session-card="${targetSessionId}"]`)
      await first.scrollIntoViewIfNeeded()
      await terminalCommand(first.locator('.terminal-surface'), 'true')
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

  test('coalesces real window dragging to 60 terminal resizes per second', async () => {
    test.setTimeout(120_000)
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      for (let index = 1; index < 16; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      const foregroundSurfaces = fixture.page.locator(
        '.scene-stage:not([hidden]) [data-testid="terminal-pane"] .terminal-surface'
      )
      await expect(foregroundSurfaces).toHaveCount(16)
      await expect.poll(async () => foregroundSurfaces.evaluateAll((surfaces) =>
        surfaces.every((surface) => /^[1-9][0-9]*$/.test(surface.getAttribute('data-pid') ?? ''))
      )).toBe(true)
      const identities = await foregroundSurfaces.evaluateAll((surfaces) => surfaces.map((surface) => ({
        sessionId: surface.getAttribute('data-session-id'),
        pid: surface.getAttribute('data-pid')
      })))
      expect(new Set(identities.map(({ sessionId }) => sessionId)).size).toBe(16)
      expect(new Set(identities.map(({ pid }) => pid)).size).toBe(16)

      await fixture.page.evaluate(() => {
        type ResizeProbe = {
          entries: Array<{ sessionId: string; resizeId: number; cols: number; rows: number; at: number }>
          applied: Array<{ sessionId: string; resizeId: number; cols: number; rows: number; at: number }>
        }
        const scope = window as typeof window & { __matouResizeProbe?: ResizeProbe }
        scope.__matouResizeProbe = { entries: [], applied: [] }
        const observed = new WeakSet<MessagePort>()
        const original = MessagePort.prototype.postMessage
        MessagePort.prototype.postMessage = function(message: unknown, transferOrOptions?: unknown) {
          if (!observed.has(this)) {
            observed.add(this)
            this.addEventListener('message', (event: MessageEvent<unknown>) => {
              const value = event.data
              if (!value || typeof value !== 'object' || !('type' in value) ||
                value.type !== 'terminal.resized' || !('sessionId' in value) ||
                !('resizeId' in value) || !('cols' in value) || !('rows' in value)) return
              scope.__matouResizeProbe?.applied.push({
                sessionId: String(value.sessionId), resizeId: Number(value.resizeId),
                cols: Number(value.cols), rows: Number(value.rows), at: performance.now()
              })
            })
          }
          if (message && typeof message === 'object' && 'type' in message &&
            message.type === 'terminal.resize' && 'sessionId' in message &&
            'resizeId' in message && 'cols' in message && 'rows' in message) {
            scope.__matouResizeProbe?.entries.push({
              sessionId: String(message.sessionId), resizeId: Number(message.resizeId),
              cols: Number(message.cols), rows: Number(message.rows), at: performance.now()
            })
          }
          if (transferOrOptions === undefined) return original.call(this, message)
          return original.call(this, message, transferOrOptions as StructuredSerializeOptions)
        }
      })

      const active = activeSurface(fixture.page)
      const activeSessionId = await active.getAttribute('data-session-id')
      expect(activeSessionId).toBeTruthy()
      const stableActive = fixture.page.locator(
        `.terminal-surface[data-session-id="${activeSessionId}"]`
      )
      const textarea = stableActive.locator('.xterm-helper-textarea')
      await textarea.focus()
      const dragWindow = fixture.app.evaluate(async ({ BrowserWindow }) => {
        const window = BrowserWindow.getAllWindows()[0]
        if (!window) throw new Error('Matou window is missing')
        const startedAt = Date.now()
        let step = 0
        await new Promise<void>((resolve) => {
          const timer = setInterval(() => {
            if (Date.now() - startedAt >= 2_100) {
              clearInterval(timer)
              window.setSize(1500, 820)
              resolve()
              return
            }
            window.setSize(1200 + (step % 30) * 10, 820)
            step += 1
          }, 8)
        })
      })
      await textarea.pressSequentially("printf '__RESIZE_INPUT_OK__\\n'", { delay: 8 })
      await textarea.press('Enter')
      await expect(stableActive.locator('.xterm-rows')).toContainText('__RESIZE_INPUT_OK__')
      await dragWindow
      await fixture.page.waitForTimeout(250)

      const entries = await fixture.page.evaluate(() => (
        window as typeof window & {
          __matouResizeProbe?: {
            entries: Array<{ sessionId: string; cols: number; rows: number; at: number }>
          }
        }
      ).__matouResizeProbe?.entries ?? [])
      expect(entries.length).toBeGreaterThan(0)
      const bySession = new Map<string, typeof entries>()
      for (const entry of entries) {
        const sessionEntries = bySession.get(entry.sessionId) ?? []
        sessionEntries.push(entry)
        bySession.set(entry.sessionId, sessionEntries)
      }
      // Fixed-width off-screen cards may keep their exact grid dimensions and
      // therefore correctly emit zero resize messages during a window drag.
      expect(bySession.size).toBeLessThanOrEqual(16)
      for (const sessionEntries of bySession.values()) {
        expect(maximumMessagesInOneSecond(sessionEntries.map(({ at }) => at))).toBeLessThanOrEqual(60)
      }

      // Pick a card that is actually mounted in the horizontal viewport. The
      // domain-focused Session can legitimately be outside the viewport while
      // still counting as foreground, in which case it has no view resize to
      // offer for this acceptance barrier.
      const finalActive = visibleSurfaces(fixture.page).last()
      await finalActive.click({ position: { x: 12, y: 12 } })
      await expect(finalActive.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]'))
        .toHaveAttribute('data-active', 'true')
      const finalActiveSessionId = await finalActive.getAttribute('data-session-id')
      expect(finalActiveSessionId).toBeTruthy()
      // The Runtime may publish activity ordering while the test is typing.
      // Bind every subsequent action to the captured Session identity instead
      // of re-resolving the dynamic [data-active] selector.
      const stableFinalActive = fixture.page.locator(
        `.terminal-surface[data-session-id="${finalActiveSessionId}"]`
      )
      const finalTextarea = stableFinalActive.locator('.xterm-helper-textarea')
      const evidenceId = Date.now().toString(36)
      const marker = `R${evidenceId}`
      const sttyResultPath = `/tmp/m-${evidenceId}`
      await fixture.page.evaluate(() => {
        const scope = window as typeof window & {
          __matouResizeProbe?: { entries: unknown[]; applied: unknown[] }
        }
        if (scope.__matouResizeProbe) {
          scope.__matouResizeProbe.entries = []
          scope.__matouResizeProbe.applied = []
        }
      })
      await fixture.app.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()[0]?.setSize(1200, 650)
      )
      await waitForCurrentXtermResizeApplied(
        fixture.page, stableFinalActive, finalActiveSessionId!
      )
      // The first real command can publish activity ordering and move the
      // selected card. Let that product behavior finish before measuring the
      // terminal dimensions so the acceptance check observes the stable card.
      const settleMarker = `S${evidenceId}`
      await finalTextarea.focus()
      await finalTextarea.pressSequentially(`printf '${settleMarker}\\n'`, { delay: 2 })
      await finalTextarea.press('Enter')
      await expect(stableFinalActive.locator('.xterm-rows')).toContainText(settleMarker)
      await waitForCurrentXtermResizeApplied(
        fixture.page, stableFinalActive, finalActiveSessionId!
      )
      await finalTextarea.focus()
      await finalTextarea.pressSequentially(
        `stty size > '${sttyResultPath}'; printf '${marker}\\n'`,
        { delay: 2 }
      )
      // Focusing and laying out a long input line can change xterm by one or
      // two columns. Settle that final offer too, then require Runtime's exact
      // application ACK before Enter reaches the PTY.
      const submitResize = await waitForCurrentXtermResizeApplied(
        fixture.page, stableFinalActive, finalActiveSessionId!
      )
      await finalTextarea.press('Enter')
      await expect.poll(async () => readText(sttyResultPath).catch(() => '')).toMatch(/^\d+ \d+\n$/)
      await expect(stableFinalActive.locator('.xterm-rows')).toContainText(marker)
      const appliedAfterSubmit = await waitForCurrentXtermResizeApplied(
        fixture.page, stableFinalActive, finalActiveSessionId!
      )
      const size = (await readText(sttyResultPath)).trim().match(/^(\d+) (\d+)$/)
      expect(size).not.toBeNull()
      expect({ rows: submitResize.rows, cols: submitResize.cols }).toEqual({
        rows: appliedAfterSubmit.rows, cols: appliedAfterSubmit.cols
      })
      expect({ rows: Number(size![1]), cols: Number(size![2]) }).toEqual({
        rows: appliedAfterSubmit.rows, cols: appliedAfterSubmit.cols
      })
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

function maximumMessagesInOneSecond(timestamps: number[]): number {
  let left = 0
  let maximum = 0
  for (let right = 0; right < timestamps.length; right += 1) {
    while (timestamps[left] !== undefined && timestamps[left]! <= timestamps[right]! - 1_000) {
      left += 1
    }
    maximum = Math.max(maximum, right - left + 1)
  }
  return maximum
}

type ResizeProbeEntry = {
  sessionId: string
  resizeId: number
  cols: number
  rows: number
  at: number
}

async function readResizeEntries(page: import('@playwright/test').Page): Promise<ResizeProbeEntry[]> {
  return page.evaluate(() => (
    window as typeof window & {
      __matouResizeProbe?: { entries: ResizeProbeEntry[] }
    }
  ).__matouResizeProbe?.entries ?? [])
}

async function readAppliedResizeEntries(
  page: import('@playwright/test').Page
): Promise<ResizeProbeEntry[]> {
  return page.evaluate(() => (
    window as typeof window & {
      __matouResizeProbe?: { applied: ResizeProbeEntry[] }
    }
  ).__matouResizeProbe?.applied ?? [])
}

async function waitForCurrentXtermResizeApplied(
  page: import('@playwright/test').Page,
  surface: import('@playwright/test').Locator,
  sessionId: string
): Promise<ResizeProbeEntry> {
  const deadline = Date.now() + 15_000
  let previous = ''
  let stableFrames = 0
  while (Date.now() < deadline) {
    const last = (await readResizeEntries(page))
      .filter((entry) => entry.sessionId === sessionId).at(-1)
    const current = await surface.locator('.terminal-surface__viewport').evaluate((element) => ({
      cols: Number((element as HTMLElement).dataset.terminalCols),
      rows: Number((element as HTMLElement).dataset.terminalRows)
    }))
    const applied = last !== undefined && (await readAppliedResizeEntries(page)).some((entry) =>
      entry.sessionId === last.sessionId && entry.resizeId === last.resizeId &&
      entry.cols === last.cols && entry.rows === last.rows
    )
    const signature = last && applied && current.cols === last.cols && current.rows === last.rows
      ? `${last.resizeId}:${last.cols}x${last.rows}`
      : ''
    stableFrames = signature !== '' && signature === previous ? stableFrames + 1 : 0
    previous = signature
    if (stableFrames >= 4 && last) return last
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
  }
  throw new Error(`xterm and Runtime did not settle on one applied resize for ${sessionId}`)
}
