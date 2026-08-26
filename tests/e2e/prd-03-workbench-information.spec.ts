import { expect, test, type Page } from '@playwright/test'

import { launchMatou, restartMatou } from './matou-fixture'

test('keeps Kooky Task naming, validation, drag focus, and order across restart', async () => {
  let fixture = await launchMatou()
  try {
    let { page } = fixture
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await addTask(page)
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await addTask(page)
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')

    await page.getByRole('button', { name: '事项菜单：新事项 2' }).click()
    await page.getByRole('menuitem', { name: '重命名' }).click()
    await page.getByRole('textbox', { name: '事项名称' }).fill('新事项')
    await expect(page.getByText('当前工作区下已存在名为"新事项"的工作台')).toBeVisible()
    await expect(page.getByRole('button', { name: '确定' })).toBeDisabled()
    await page.getByRole('textbox', { name: '事项名称' }).fill('')
    await page.getByRole('button', { name: '确定' }).click()
    await expect(page.getByText('工作台名称不能为空')).toBeVisible()
    await page.getByRole('button', { name: '取消' }).click()

    await pinTask(page, '新事项 2')
    await pinTask(page, '默认')
    await dragTask(page, '默认', '新事项 2')
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')
    await expect(taskNames(page)).resolves.toEqual(['默认', '新事项 2', '新事项'])

    fixture = await restartMatou(fixture)
    page = fixture.page
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')
    await expect(taskNames(page)).resolves.toEqual(['默认', '新事项 2', '新事项'])
  } finally {
    await fixture.close()
  }
})

test('matches Kooky Task context-menu close behavior', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByTestId('task-' + await activeTaskId(page)).click({ button: 'right' })
    await expect(page.getByRole('menu')).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(page.getByRole('menu')).toHaveCount(0)

    await page.getByRole('button', { name: '事项菜单：默认' }).click()
    await expect(page.getByRole('menu')).toBeVisible()
    await page.evaluate(() => document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })))
    await expect(page.getByRole('menu')).toHaveCount(0)
  } finally {
    await fixture.close()
  }
})

async function taskNames(page: Page): Promise<string[]> {
  return page.locator('.workbench-item__name').allTextContents()
}

async function addTask(page: Page): Promise<void> {
  await page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
}

async function pinTask(page: Page, title: string): Promise<void> {
  await page.getByRole('button', { name: `事项菜单：${title}` }).click()
  await page.getByRole('menuitem', { name: '置顶' }).click()
}

async function activeTaskId(page: Page): Promise<string> {
  const row = page.locator('.workbench-item.is-active')
  const testId = await row.getAttribute('data-testid')
  return testId?.replace(/^task-/, '') ?? ''
}

async function dragTask(page: Page, sourceTitle: string, targetTitle: string): Promise<void> {
  const row = (title: string) => page.locator('.workbench-item').filter({
    has: page.locator('.workbench-item__name', { hasText: title })
  })
  await row(sourceTitle).dragTo(row(targetTitle))
}
