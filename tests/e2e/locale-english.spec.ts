import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('renders the English UI when MATOU_LOCALE=en', async () => {
  const fixture = await launchMatou({ env: { MATOU_LOCALE: 'en' } })
  try {
    await expect(fixture.page.locator('html')).toHaveAttribute('lang', 'en')
    await expect(fixture.page.getByRole('button', { name: 'New task' })).toBeVisible()
    await fixture.page.screenshot({
      path: '/Users/icesword/Documents/AIProjects/matou/.superpowers/sdd/2026-09-09-english-ui-i18n/en-main-window.png',
      fullPage: false
    })
  } finally {
    await fixture.close()
  }
})
