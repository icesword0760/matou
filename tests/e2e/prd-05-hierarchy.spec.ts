import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('first launch presents the complete Workspace, Task, Scene, and terminal hierarchy', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await expect(page.getByRole('group', { name: 'matou_workspace 工作空间' })).toBeVisible()
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await expect(page.getByRole('tab')).toHaveCount(1)
    await expect(page.getByTestId('terminal-pane')).toHaveCount(1)
    await expect(page.locator('.terminal-surface[data-pid]')).toHaveCount(2)
  } finally { await fixture.close() }
})

test('creates and renames a Task, adds a Scene, splits, and deletes one terminal', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await page.getByRole('button', { name: '在 matou_workspace 中新增事项' }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')

    await page.getByRole('button', { name: '事项菜单：新事项' }).click()
    await page.getByRole('menuitem', { name: '重命名' }).click()
    await page.getByRole('textbox', { name: '事项名称' }).fill('修复登录')
    await page.getByRole('button', { name: '确定' }).click()
    await expect(page.getByTestId('active-task')).toHaveText('修复登录')

    await page.getByRole('button', { name: '新建页签' }).click()
    await expect(page.getByRole('tab')).toHaveCount(2)
    await page.getByRole('tab').first().click()
    await page.getByRole('tab').last().click()
    await expect(page.locator('.scene-stage:not([hidden]) .xterm-helper-textarea')).toBeFocused()
    await page.getByRole('button', { name: '水平分屏' }).click()
    await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2)

    const visiblePane = page.locator('[data-testid="terminal-pane"]:visible').last()
    await visiblePane.getByRole('button', { name: /^删除终端：/ }).click({ force: true })
    await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(1)
  } finally { await fixture.close() }
})

test('matches Kooky when deleting a Task from its sidebar menu', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: '事项菜单：默认' }).click()
    await page.getByRole('menuitem', { name: '删除' }).click()
    await expect(page.getByText('删除 "默认" 会丢失该事项下所有终端会话，但不会删除本地目录。 是否继续？')).toBeVisible()
    await page.getByRole('button', { name: '确定' }).click()
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await expect(page.getByTestId('terminal-pane')).toHaveCount(1)
  } finally { await fixture.close() }
})
