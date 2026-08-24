import { expect, test, type Page } from '@playwright/test'

import { launchMatou, restartMatou } from './matou-fixture'

test('keeps Kooky Task naming, validation, drag focus, and order across restart', async () => {
  let fixture = await launchMatou()
  try {
    let { page } = fixture
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await page.getByRole('button', { name: '事项', exact: true }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await page.getByRole('button', { name: '事项', exact: true }).click()
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

    await dragTask(page, '默认', '新事项 2')
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')
    await expect(taskNames(page)).resolves.toEqual(['新事项', '新事项 2', '默认'])

    fixture = await restartMatou(fixture)
    page = fixture.page
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')
    await expect(taskNames(page)).resolves.toEqual(['新事项', '新事项 2', '默认'])
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

async function activeTaskId(page: Page): Promise<string> {
  const row = page.locator('.workbench-item.is-active')
  const testId = await row.getAttribute('data-testid')
  return testId?.replace(/^task-/, '') ?? ''
}

async function dragTask(page: Page, sourceTitle: string, targetTitle: string): Promise<void> {
  await page.evaluate(({ sourceTitle, targetTitle }) => {
    const rows = [...document.querySelectorAll<HTMLElement>('.workbench-item')]
    const source = rows.find((row) => row.querySelector('.workbench-item__name')?.textContent === sourceTitle)
    const target = rows.find((row) => row.querySelector('.workbench-item__name')?.textContent === targetTitle)
    if (!source || !target) throw new Error('Task drag fixture is missing')
    const transfer = new DataTransfer()
    source.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: transfer }))
    target.dispatchEvent(new DragEvent('dragover', { bubbles: true, dataTransfer: transfer }))
    target.dispatchEvent(new DragEvent('drop', { bubbles: true, dataTransfer: transfer }))
    source.dispatchEvent(new DragEvent('dragend', { bubbles: true, dataTransfer: transfer }))
  }, { sourceTitle, targetTitle })
}
