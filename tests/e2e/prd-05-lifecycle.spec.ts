import { access, realpath } from 'node:fs/promises'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

const execFileAsync = promisify(execFile)

test('keeps the default home Workspace protected across restart', async () => {
  let fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: '工作空间菜单：matou_workspace' }).click()
    await expect(page.getByRole('menuitem', { name: '移出码头' })).toHaveCount(0)
    await expect(page.getByRole('menuitem', { name: '重命名' })).toHaveCount(0)
    await page.keyboard.press('Escape')
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await expect(access(fixture.workspaceDirectory)).resolves.toBeUndefined()

    fixture = await restartMatou(fixture)
    await expect(fixture.page.getByRole('group', { name: 'matou_workspace 工作空间' })).toBeVisible()
  } finally { await fixture.close() }
})

test('opens the selected default directory as a complete Workspace hierarchy', async () => {
  const fixture = await launchMatou()
  try {
    await expect(fixture.page.getByRole('group', { name: 'matou_workspace 工作空间' })).toBeVisible()
    await expect(fixture.page.getByTestId('active-task')).toHaveText('默认')
    await expect(fixture.page.getByRole('tab')).toHaveCount(1)
    await expect(fixture.page.getByTestId('terminal-pane')).toHaveCount(1)
    const surface = fixture.page.locator('.scene-stage:not([hidden]) .terminal-surface')
    await expect(surface)
      .toHaveAttribute('data-pid', /\d+/)
    if (process.platform !== 'win32') {
      expect(await realpath(await processCwd(Number(await surface.getAttribute('data-pid')))))
        .toBe(await realpath(fixture.workspaceDirectory))
    }
  } finally { await fixture.close() }
})

test('closes only one non-final Scene and selects its deterministic successor', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: '新建页签' }).click()
    await page.getByRole('button', { name: '新建页签' }).click()
    const tabs = page.locator('.scene-tabs > [data-scene-id]')
    await expect(tabs).toHaveCount(3)
    const successorId = await tabs.nth(2).getAttribute('data-scene-id')
    await tabs.nth(1).getByRole('tab').click()
    await tabs.nth(1).getByRole('button', { name: /^关闭页签：/ }).click()

    await expect(tabs).toHaveCount(2)
    await expect(page.locator(`[data-scene-id="${successorId}"] [role="tab"]`))
      .toHaveAttribute('aria-selected', 'true')
    await expect(page.getByRole('alertdialog')).toHaveCount(0)
  } finally { await fixture.close() }
})

test('matches Kooky by protecting the last work scene and keeping the terminal live', async () => {
  const fixture = await launchMatou()
  try {
    const surface = fixture.page.getByTestId('terminal-pane').first().locator('.terminal-surface')
    await expect(surface).toHaveAttribute('data-pid', /\d+/)
    const pid = await surface.getAttribute('data-pid')
    await fixture.page.getByRole('button', { name: /^关闭页签：/ }).click()

    await expect(fixture.page.getByRole('alertdialog', { name: '提示' })).toContainText('最后一个事项下的最后一个标签')
    await fixture.page.getByRole('button', { name: '我知道了' }).click()
    await expect.poll(() => fixture.app.evaluate(({ BrowserWindow }) =>
      BrowserWindow.getAllWindows()[0]?.isVisible()
    )).toBe(true)
    await expect(surface).toHaveAttribute('data-pid', pid!)
    await expect(fixture.page.getByTestId('active-task')).toHaveText('默认')
  } finally { await fixture.close() }
})

test('persists Task order and each Scene divider geometry across restart', async () => {
  let fixture: MatouFixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')
    await pinTask(page, '新事项 2')
    await pinTask(page, '默认')
    await dragTaskBefore(page, '默认', '新事项 2')
    await expect.poll(() => taskTitles(page)).toEqual(['默认', '新事项 2', '新事项'])

    await page.getByRole('button', { name: '水平分屏' }).click()
    const separator = page.getByRole('separator').first()
    const box = await separator.locator('xpath=ancestor::*[contains(@class,"split-node")][1]').boundingBox()
    if (!box) throw new Error('split layout has no visible bounds')
    await separator.hover()
    await page.mouse.down()
    await page.mouse.move(box.x + box.width * 0.35, box.y + box.height / 2)
    await page.mouse.up()
    await expect.poll(async () => parseFloat(
      await page.locator('[data-testid^="split-child-"]').first().evaluate((element) =>
        (element as HTMLElement).style.flexBasis
      )
    )).toBeCloseTo(35, 0)
    await page.waitForTimeout(180)

    fixture = await restartMatou(fixture)
    await expect.poll(() => taskTitles(fixture.page)).toEqual(['默认', '新事项 2', '新事项'])
    await expect(fixture.page.locator('[data-testid^="split-child-"]').first())
      .toHaveAttribute('style', /flex-basis: 35%/)
  } finally { await fixture.close() }
})

async function taskTitles(page: MatouFixture['page']): Promise<string[]> {
  return page.locator('.workbench-item__name').allTextContents()
}

async function pinTask(page: MatouFixture['page'], title: string): Promise<void> {
  await page.getByRole('button', { name: `事项菜单：${title}` }).click()
  await page.getByRole('menuitem', { name: '置顶' }).click()
}

async function dragTaskBefore(page: MatouFixture['page'], sourceTitle: string, destinationTitle: string) {
  const row = (title: string) => page.locator('.workbench-item').filter({
    has: page.locator('.workbench-item__name', { hasText: title })
  })
  await row(sourceTitle).dragTo(row(destinationTitle))
}

async function processCwd(pid: number): Promise<string> {
  if (process.platform === 'linux') {
    const { stdout } = await execFileAsync('readlink', [`/proc/${pid}/cwd`])
    return stdout.trim()
  }
  const { stdout } = await execFileAsync('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'])
  return stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1) ?? ''
}
