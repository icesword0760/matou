import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('renders the English UI when MATOU_LOCALE=en', async () => {
  const fixture = await launchMatou({ env: { MATOU_LOCALE: 'en' } })
  try {
    await expect(fixture.page.locator('html')).toHaveAttribute('lang', 'en')
    await expect(fixture.page.getByRole('button', { name: 'New task' })).toBeVisible()
    const screenshotDirectory = process.env.MATOU_E2E_SCREENSHOT_DIR
    if (screenshotDirectory) {
      await fixture.page.screenshot({
        path: join(screenshotDirectory, 'en-main-window.png'),
        fullPage: false
      })
    }
  } finally {
    await fixture.close()
  }
})
