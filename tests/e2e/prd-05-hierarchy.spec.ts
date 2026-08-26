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

test('integrates the existing top controls into the macOS window chrome', async () => {
  const fixture = await launchMatou()
  try {
    const { app, page } = fixture
    await expect(page.getByRole('button', { name: '新增工作空间' })).toBeVisible()
    await expect(page.getByRole('button', { name: '通知中心' })).toBeVisible()
    const [sidebarBar, sceneBar, newWorkspace] = await Promise.all([
      page.locator('.flat-sidebar__topbar').boundingBox(),
      page.locator('.scene-bar.tab-bar').boundingBox(),
      page.getByRole('button', { name: '新增工作空间' }).boundingBox()
    ])
    expect(sidebarBar).not.toBeNull()
    expect(sceneBar).not.toBeNull()
    expect(newWorkspace).not.toBeNull()
    expect(sidebarBar!.height).toBe(sceneBar!.height)
    if (process.platform === 'darwin') {
      expect(newWorkspace!.x).toBeGreaterThanOrEqual(78)
      expect(await app.evaluate(({ BrowserWindow }) => {
        const window = BrowserWindow.getAllWindows()[0]!
        return window.getBounds().height - window.getContentBounds().height
      })).toBe(0)
    }
  } finally { await fixture.close() }
})

test('creates and renames a Task, adds a Scene, splits, and deletes one terminal', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await page.getByRole('button', { name: '在 matou_workspace 中新增事项' }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await expect(page.locator('.scene-stage:not([hidden]) .xterm-helper-textarea')).toBeFocused()

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

test('keeps navigation order stable on clicks, then promotes the Task after real terminal input', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: '在 matou_workspace 中新增事项' }).click()
    const taskItems = page.locator('[data-testid^="task-"]')
    await expect(taskItems.first()).toContainText('默认')
    await expect(taskItems.last()).toContainText('新事项')

    await taskItems.first().click()
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await taskItems.last().click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await expect(taskItems.first()).toContainText('默认')
    await expect(taskItems.last()).toContainText('新事项')

    await expect(page.locator('.scene-stage:not([hidden]) .xterm-helper-textarea')).toBeFocused()
    await page.keyboard.type('printf MATOU_RECENCY')
    await page.keyboard.press('Enter')
    await expect(taskItems.first()).toContainText('新事项')
    await expect(taskItems.last()).toContainText('默认')
  } finally { await fixture.close() }
})

test('shows distinct Workspace and Task selection levels and keeps pin controls separate', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    const workspace = page.getByRole('group', { name: 'matou_workspace 工作空间' })
    const header = workspace.locator('.workspace-group__header')
    const task = page.getByTestId('active-task').locator('..').locator('..')
    await expect(header).toHaveAttribute('aria-current', 'location')
    await expect(task).toHaveAttribute('aria-current', 'true')
    const colors = await Promise.all([header, task].map((locator) => locator.evaluate((node) =>
      getComputedStyle(node).backgroundColor
    )))
    expect(colors[0]).not.toBe(colors[1])

    await page.getByRole('button', { name: '工作空间菜单：matou_workspace' }).click()
    await page.getByRole('menuitem', { name: '取消置顶' }).click()
    await expect(page.getByText('已取消工作空间置顶')).toBeVisible()
    await expect(workspace.locator('.workspace-group__status .pin-icon')).toHaveCount(0)
    await page.getByRole('button', { name: '工作空间菜单：matou_workspace' }).click()
    await page.getByRole('menuitem', { name: '置顶' }).click()
    await expect(page.getByText('工作空间已置顶')).toBeVisible()
    await expect(workspace.locator('.workspace-group__status .pin-icon')).toHaveCount(1)
    const [workspaceBadgeBox, workspacePinBox] = await Promise.all([
      workspace.locator('.workspace-group__badge').boundingBox(),
      workspace.locator('.workspace-group__status .pin-icon').boundingBox()
    ])
    expect(workspaceBadgeBox).not.toBeNull()
    expect(workspacePinBox).not.toBeNull()
    expect(workspaceBadgeBox!.x + workspaceBadgeBox!.width).toBeLessThanOrEqual(workspacePinBox!.x)

    await page.getByRole('button', { name: '事项菜单：默认' }).click()
    await page.getByRole('menuitem', { name: '置顶' }).click()
    await expect(task.locator('.workbench-item__status .pin-icon')).toHaveCount(1)
    const [pinBox, actionBox] = await Promise.all([
      task.locator('.workbench-item__status .pin-icon').boundingBox(),
      task.locator('.workbench-item__actions').boundingBox()
    ])
    expect(pinBox).not.toBeNull()
    expect(actionBox).not.toBeNull()
    expect(pinBox!.x + pinBox!.width).toBeLessThanOrEqual(actionBox!.x)
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
