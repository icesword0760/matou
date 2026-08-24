import { access, mkdir, realpath } from 'node:fs/promises'
import { execFile } from 'node:child_process'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

const execFileAsync = promisify(execFile)

test('confirms removing a Workspace, keeps its directory, and remembers explicit removal', async () => {
  let fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: '切换工作区' }).click()
    await page.getByRole('menuitem', { name: '删除' }).click()
    await expect(page.getByRole('alertdialog', { name: '提示' })).toContainText(
      '不会删除磁盘上的工作区目录'
    )
    await page.getByRole('button', { name: '取消' }).click()
    await expect(page.getByTestId('active-task')).toHaveText('默认')

    await page.getByRole('button', { name: '切换工作区' }).click()
    await page.getByRole('menuitem', { name: '删除' }).click()
    await page.getByRole('button', { name: '确定' }).click()
    await expect(page.getByText('还没有工作区')).toBeVisible()
    await expect(access(fixture.workspaceDirectory)).resolves.toBeUndefined()

    fixture = await restartMatou(fixture)
    await expect(fixture.page.getByText('还没有工作区')).toBeVisible()
  } finally { await fixture.close() }
})

test('creates a selected-directory Workspace and opens its complete default hierarchy immediately', async () => {
  const fixture = await launchMatou()
  const selectedDirectory = join(fixture.rootDirectory, 'selected-project')
  await mkdir(selectedDirectory)
  try {
    await fixture.app.evaluate(({ dialog }, path) => {
      dialog.showOpenDialog = async () => ({ canceled: false, filePaths: [path] })
    }, selectedDirectory)
    await fixture.page.getByRole('button', { name: '切换工作区' }).click()
    await fixture.page.getByRole('menuitem', { name: /新增工作区/ }).click()

    await expect(fixture.page.getByTestId('workspace-name')).toContainText('selected-project')
    await expect(fixture.page.getByTestId('active-task')).toHaveText('默认')
    await expect(fixture.page.getByRole('tab')).toHaveCount(1)
    await expect(fixture.page.getByTestId('terminal-pane')).toHaveCount(1)
    const surface = fixture.page.locator('.scene-stage:not([hidden]) .terminal-surface')
    await expect(surface)
      .toHaveAttribute('data-pid', /\d+/)
    if (process.platform !== 'win32') {
      expect(await realpath(await processCwd(Number(await surface.getAttribute('data-pid')))))
        .toBe(await realpath(selectedDirectory))
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
    await page.getByRole('button', { name: '事项', exact: true }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await page.getByRole('button', { name: '事项', exact: true }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')
    await dragTaskBefore(page, '新事项 2', '新事项')
    await expect.poll(() => taskTitles(page)).toEqual(['默认', '新事项 2', '新事项'])
    await dragTaskBefore(page, '新事项 2', '默认')
    await expect.poll(() => taskTitles(page)).toEqual(['新事项 2', '默认', '新事项'])

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
    await expect.poll(() => taskTitles(fixture.page)).toEqual(['新事项 2', '默认', '新事项'])
    await expect(fixture.page.locator('[data-testid^="split-child-"]').first())
      .toHaveAttribute('style', /flex-basis: 35%/)
  } finally { await fixture.close() }
})

async function taskTitles(page: MatouFixture['page']): Promise<string[]> {
  return page.locator('.workbench-item__name').allTextContents()
}

async function dragTaskBefore(page: MatouFixture['page'], sourceTitle: string, destinationTitle: string) {
  await page.evaluate(({ sourceTitle, destinationTitle }) => {
    const rows = [...document.querySelectorAll<HTMLElement>('.workbench-item')]
    const source = rows.find((row) => row.querySelector('.workbench-item__name')?.textContent === sourceTitle)
    const destination = rows.find((row) => row.querySelector('.workbench-item__name')?.textContent === destinationTitle)
    if (!source || !destination) throw new Error('task row missing')
    const dataTransfer = new DataTransfer()
    source.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer }))
    destination.dispatchEvent(new DragEvent('dragover', { bubbles: true, dataTransfer }))
    destination.dispatchEvent(new DragEvent('drop', { bubbles: true, dataTransfer }))
  }, { sourceTitle, destinationTitle })
}

async function processCwd(pid: number): Promise<string> {
  if (process.platform === 'linux') {
    const { stdout } = await execFileAsync('readlink', [`/proc/${pid}/cwd`])
    return stdout.trim()
  }
  const { stdout } = await execFileAsync('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'])
  return stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1) ?? ''
}
