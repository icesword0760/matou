import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('detaches and returns the same live terminal process', async () => {
  const fixture = await launchMatou()
  try {
    const { app, page } = fixture
    const embedded = page.getByTestId('terminal-pane').first().locator('.terminal-surface')
    await expect(embedded).toHaveAttribute('data-pid', /\d+/)
    const originalPid = await embedded.getAttribute('data-pid')
    await page.getByRole('button', { name: /^脱出终端：/ }).click({ force: true })

    await expect(page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const detached = (await app.windows()).find((candidate) => candidate !== page)!
    await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-pid', originalPid!)

    await detached.close()
    await expect(page.getByTestId('detached-placeholder')).toHaveCount(0)
    await expect(page.getByTestId('terminal-pane').first().locator('.terminal-surface'))
      .toHaveAttribute('data-pid', originalPid!)
  } finally { await fixture.close() }
})
