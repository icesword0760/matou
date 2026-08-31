import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

test('detaches a live terminal, then returns the same live node when its native window closes', async () => {
  const fixture = await launchMatou()
  try {
    const { app, page } = fixture
    const embedded = page.getByTestId('terminal-pane').first().locator('.terminal-surface')
    await expect(embedded).toHaveAttribute('data-pid', /\d+/)
    const originalPid = await embedded.getAttribute('data-pid')
    const sessionId = await embedded.getAttribute('data-session-id')
    await page.locator('.terminal-pane-header').first().dispatchEvent('dragend', { screenX: -1, screenY: -1 })

    await expect(page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const detached = (await app.windows()).find((candidate) => candidate !== page)!
    await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-pid', originalPid!)

    await detached.close()
    await expect(page.getByTestId('detached-placeholder')).toHaveCount(0)
    await expect(page.locator('.stopped-session-card')).toHaveCount(0)
    const returned = page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
    await expect(returned).toHaveAttribute('data-pid', /\d+/)
    await expect(returned.locator('.xterm-helper-textarea')).toBeAttached()
  } finally { await fixture.close() }
})

test('returns a detached terminal to its Scene instead of reopening a temporary window after restart', async () => {
  let fixture: MatouFixture = await launchMatou()
  try {
    await fixture.page.locator('.terminal-pane-header').first().dispatchEvent('dragend', { screenX: -1, screenY: -1 })
    await expect(fixture.page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)

    fixture = await restartMatou(fixture)
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
    await expect(fixture.page.getByTestId('detached-placeholder')).toHaveCount(0)
    await expect(fixture.page.getByTestId('terminal-pane')).toHaveCount(1)
    await expect(fixture.page.getByTestId('terminal-pane').locator('.terminal-surface'))
      .toHaveAttribute('data-pid', /\d+/)
  } finally { await fixture.close() }
})
