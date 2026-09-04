import { access, mkdir, realpath } from 'node:fs/promises'
import { execFile } from 'node:child_process'
import { join } from 'node:path'
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

test('opens a newly added Workspace as a fresh interactive terminal without recovery UI', async () => {
  const fixture = await launchMatou()
  const freshWorkspace = join(fixture.rootDirectory, 'fresh-workspace')
  await mkdir(freshWorkspace)
  try {
    await fixture.app.evaluate(({ ipcMain }, selectedPath) => {
      const channel = 'matou:select-workspace-directory'
      ipcMain.removeHandler(channel)
      ipcMain.handle(channel, () => selectedPath)
    }, freshWorkspace)

    await fixture.page.getByRole('button', { name: '新增工作空间' }).click()
    await expect(fixture.page.getByRole('group', { name: 'fresh-workspace 工作空间' }))
      .toHaveClass(/is-active/)
    const card = fixture.page.getByRole('article', { name: '会话：Shell' })
    await expect(card).toBeVisible()
    const pane = card.getByTestId('terminal-pane')

    const surface = pane.locator('.terminal-surface')
    await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
    const input = surface.locator('.xterm-helper-textarea')
    await expect(input).toHaveCount(1)
    await input.focus()
    await input.pressSequentially("printf '__FRESH_WORKSPACE_READY__\\n'", { delay: 2 })
    await input.press('Enter')
    await expect(surface.locator('.xterm-rows')).toContainText('__FRESH_WORKSPACE_READY__')
    await expect(pane.getByTestId('session-recovery-water')).toHaveCount(0)
    await expect(pane.getByText('恢复中')).toHaveCount(0)
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
    await expect(page.getByRole('alertdialog', { name: '关闭画布' })).toBeVisible()
    await page.getByRole('button', { name: '关闭画布' }).click()

    await expect(tabs).toHaveCount(2)
    await expect(page.locator(`[data-scene-id="${successorId}"] [role="tab"]`))
      .toHaveAttribute('aria-selected', 'true')
    await expect(page.getByRole('alertdialog')).toHaveCount(0)
  } finally { await fixture.close() }
})

test('matches reference product by protecting the last work scene and keeping the terminal live', async () => {
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

test('persists Task order and each canvas horizontal position across restart', async () => {
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
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')

    for (let index = 0; index < 4; index += 1) {
      await page.getByRole('button', { name: '横向新增 Shell' }).click()
    }
    const carousel = page.getByRole('region', { name: '同级会话列表' })
    await carousel.hover()
    await carousel.dispatchEvent('wheel', { deltaX: 520, deltaY: 0 })
    let savedScrollLeft = 0
    await expect.poll(async () => {
      savedScrollLeft = await carousel.evaluate((element) => element.scrollLeft)
      return savedScrollLeft
    }).toBeGreaterThan(0)
    // Persist the ordinary browsing viewport, not a transient hover-expanded
    // card. The independent QA flow also verifies geometry after hover motion
    // has fully settled.
    await page.mouse.move(5, 5)
    await expect(page.locator('.session-card-slot.is-expanded')).toHaveCount(0)
    // Scroll snapping and card-width animation are real user-facing motion.
    // Capture the position only after that motion has settled, then verify the
    // same settled viewport is restored after restart.
    let previousScrollLeft = savedScrollLeft
    let stableSamples = 0
    await expect.poll(async () => {
      const current = await carousel.evaluate((element) => element.scrollLeft)
      stableSamples = Math.abs(current - previousScrollLeft) < 1 ? stableSamples + 1 : 0
      previousScrollLeft = current
      savedScrollLeft = current
      return stableSamples
    }, { intervals: [100, 100, 100, 100] }).toBeGreaterThanOrEqual(3)
    const activeCanvas = page.locator('.scene-stage:not([hidden]) .session-canvas')
    await expect.poll(async () => Math.abs(
      Number(await activeCanvas.getAttribute('data-last-saved-scroll-left')) - savedScrollLeft
    )).toBeLessThan(5)
    await expect(activeCanvas).toHaveAttribute('aria-busy', 'false')
    const focusedSessionId = await page.locator('.session-card-slot.is-focused')
      .getAttribute('data-session-id')
    expect(focusedSessionId).toBeTruthy()
    const savedFocusedViewportOffset = await focusedCardViewportOffset(page, focusedSessionId!)

    fixture = await restartMatou(fixture)
    // The new native window may open underneath the physical pointer even
    // though no hover was part of this journey. Move it back to window chrome
    // so the intentional hover-to-expand behavior is not confused with
    // persisted canvas geometry.
    await fixture.page.mouse.move(5, 5)
    await expect(fixture.page.locator('.session-card-slot.is-expanded')).toHaveCount(0)
    await expect.poll(() => taskTitles(fixture.page)).toEqual(['默认', '新事项 2', '新事项'])
    await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项 2')
    await expect.poll(async () => Math.abs(
      await fixture.page.getByRole('region', { name: '同级会话列表' })
        .evaluate((element) => element.scrollLeft) - savedScrollLeft
    )).toBeLessThan(5)
    await expect.poll(async () => Math.abs(
      await focusedCardViewportOffset(fixture.page, focusedSessionId!) - savedFocusedViewportOffset
    )).toBeLessThan(5)
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

async function focusedCardViewportOffset(
  page: MatouFixture['page'],
  sessionId: string
): Promise<number> {
  return page.locator(`.session-card-slot[data-session-id="${sessionId}"]`).evaluate((card) => {
    const viewport = card.closest('.session-carousel')
    if (!viewport) throw new Error('session carousel not found')
    return card.getBoundingClientRect().left - viewport.getBoundingClientRect().left
  })
}

async function processCwd(pid: number): Promise<string> {
  if (process.platform === 'linux') {
    const { stdout } = await execFileAsync('readlink', [`/proc/${pid}/cwd`])
    return stdout.trim()
  }
  const { stdout } = await execFileAsync('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'])
  return stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1) ?? ''
}
