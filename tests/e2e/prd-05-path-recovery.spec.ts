import { rename } from 'node:fs/promises'

import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('preserves the work scene while the directory is missing and resumes after restoration', async () => {
  const fixture = await launchMatou()
  const moved = `${fixture.workspaceDirectory}-moved`
  try {
    const { page } = fixture
    await expect(page.getByTestId('terminal-pane')).toHaveCount(1)
    await rename(fixture.workspaceDirectory, moved)
    await expect(page.getByText('路径失效')).toBeVisible({ timeout: 10_000 })
    await expect(page.getByText('工作区目录不可用')).toBeVisible()
    await expect(page.getByTestId('terminal-pane')).toHaveCount(1)

    await rename(moved, fixture.workspaceDirectory)
    await expect(page.getByText('路径失效')).toHaveCount(0, { timeout: 10_000 })
    await expect(page.getByText('工作区目录不可用')).toHaveCount(0)
  } finally {
    await rename(moved, fixture.workspaceDirectory).catch(() => {})
    await fixture.close()
  }
})
