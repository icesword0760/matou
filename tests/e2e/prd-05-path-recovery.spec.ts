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
    await expect(page.getByText('工作区目录不可用，请先在本地恢复原路径，或移出该工作区')).toBeVisible()
    await expect(page.getByTestId('terminal-pane')).toHaveCount(1)
    await expect(page.getByRole('button', { name: '+ 新事项' })).toBeDisabled()
    await expect(page.getByRole('button', { name: '新建页签' })).toBeDisabled()
    await expect(page.getByRole('button', { name: '水平分屏' })).toBeDisabled()

    await rename(moved, fixture.workspaceDirectory)
    await expect(page.getByText('路径失效')).toHaveCount(0, { timeout: 10_000 })
    await expect(page.getByText('工作区目录不可用，请先在本地恢复原路径，或移出该工作区')).toHaveCount(0)
  } finally {
    await rename(moved, fixture.workspaceDirectory).catch(() => {})
    await fixture.close()
  }
})
