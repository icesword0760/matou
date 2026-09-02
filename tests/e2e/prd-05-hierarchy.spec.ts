import { mkdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'

import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

const evidenceDirectory = resolve(import.meta.dirname, '../../docs/acceptance/evidence/prd-05/matou')

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
      expect(await app.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()[0]!.getBackgroundColor()
      // Electron's getter normalizes the fully transparent constructor color
      // to black while the window's `transparent` option retains the alpha.
      )).toBe('#000000')
    }
  } finally { await fixture.close() }
})

test('creates and renames a Task, adds a Scene, appends a sibling, and deletes one terminal', async () => {
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
    await page.getByRole('button', { name: '横向新增 Shell' }).click()
    await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2)

    const visiblePane = page.locator('[data-testid="terminal-pane"]:visible').last()
    await visiblePane.locator('.pane-title').click({ button: 'right' })
    await page.getByRole('menuitem', { name: '移除节点…' }).click()
    await page.getByRole('button', { name: '移除' }).click()
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

test('uses one frosted sidebar material with flat Workspace groups and a single Task selection layer', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    const workspace = page.getByRole('group', { name: 'matou_workspace 工作空间' })
    const header = workspace.locator('.workspace-group__header')
    const task = page.getByTestId('active-task').locator('..').locator('..')
    await expect(header).toHaveAttribute('aria-current', 'location')
    await expect(task).toHaveAttribute('aria-current', 'true')
    const colors = await Promise.all([page.locator('.flat-sidebar'), workspace, task].map((locator) => locator.evaluate((node) =>
      getComputedStyle(node).backgroundColor
    )))
    expect(colors).toEqual([
      'rgba(0, 0, 0, 0)',
      'rgba(0, 0, 0, 0)',
      'rgba(87, 101, 127, 0.094)'
    ])
    const glassMaterial = page.locator('.flat-sidebar__glass-material')
    await expect(glassMaterial).toBeVisible()
    expect(await glassMaterial.evaluate((node) => {
      const style = getComputedStyle(node)
      return [style.position, style.backdropFilter, style.backgroundImage]
    })).toEqual(expect.arrayContaining([
      'absolute',
      expect.stringContaining('blur(18px)'),
      expect.stringContaining('linear-gradient')
    ]))
    expect(await page.locator('.flat-sidebar__topbar').evaluate((node) =>
      getComputedStyle(node).borderBottomWidth
    )).toBe('0px')
    expect(await page.locator('.hierarchy-shell').evaluate((node) =>
      getComputedStyle(node).backgroundColor
    )).toBe('rgba(0, 0, 0, 0)')
    expect(await page.locator('body').evaluate((node) =>
      getComputedStyle(node).backgroundColor
    )).toBe('rgba(0, 0, 0, 0)')
    expect(await glassMaterial.evaluate((node) =>
      getComputedStyle(node).backgroundImage
    )).toContain('rgba(251, 253, 255, 0.09)')
    const badge = workspace.locator('.workspace-group__badge')
    expect(await badge.evaluate((node) => {
      const style = getComputedStyle(node)
      return [style.color, style.backgroundColor]
    })).toEqual(['rgb(145, 148, 158)', 'rgb(236, 238, 243)'])
    const taskMore = page.getByRole('button', { name: '事项菜单：默认' })
    expect(await taskMore.evaluate((node) => getComputedStyle(node).color)).toBe('rgb(101, 109, 118)')
    await taskMore.hover()
    expect(await taskMore.evaluate((node) => getComputedStyle(node).backgroundColor)).toBe('rgba(0, 0, 0, 0.06)')

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

test('extends the main workspace visual system into the Workspace board', async () => {
  const fixture = await launchMatou()
  try {
    const { app, page } = fixture
    await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1400, 820))
    await page.getByRole('button', { name: '看板' }).click()
    const board = page.getByRole('region', { name: 'matou_workspace 看板' })
    await expect(board).toBeVisible()
    const appearance = await board.evaluate((element) => {
      const columns = [...element.querySelectorAll<HTMLElement>('.board-column')]
      const dots = [...element.querySelectorAll<HTMLElement>('.board-column__header i')]
      return {
        canvas: getComputedStyle(element).backgroundColor,
        columnSurfaces: columns.map((column) => getComputedStyle(column).backgroundColor),
        columnRadii: columns.map((column) => getComputedStyle(column).borderRadius),
        statusDots: dots.map((dot) => getComputedStyle(dot).backgroundColor)
      }
    })
    expect(appearance.canvas).toBe('rgb(247, 248, 250)')
    expect(new Set(appearance.columnSurfaces)).toEqual(new Set(['rgb(251, 252, 253)']))
    expect(new Set(appearance.columnRadii)).toEqual(new Set(['11px']))
    expect(new Set(appearance.statusDots).size).toBe(4)
    await mkdir(evidenceDirectory, { recursive: true })
    await page.waitForTimeout(250)
    await page.locator('.hierarchy-shell').screenshot({
      path: join(evidenceDirectory, 'workspace-board-unified.png')
    })
  } finally { await fixture.close() }
})

test('separates Session cards without outlines and lifts the active card under vertical light', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    const focusedCard = page.locator('.session-card.is-focused').first()
    await expect(focusedCard).toBeVisible()
    const styles = await focusedCard.evaluate((node) => {
      const style = getComputedStyle(node)
      const light = getComputedStyle(node, '::before')
      return {
        borderWidth: style.borderTopWidth,
        transform: style.transform,
        shadow: style.boxShadow,
        lightImage: light.backgroundImage
      }
    })
    expect(styles.borderWidth).toBe('0px')
    expect(styles.transform).not.toBe('none')
    expect(styles.shadow).not.toBe('none')
    expect(styles.lightImage).toContain('linear-gradient')
  } finally { await fixture.close() }
})

test('matches reference product when deleting a Task from its sidebar menu', async () => {
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

test('matches reference product terminal shortcuts, real search, focus, zoom, and white skin end to end', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    const mod = process.platform === 'darwin' ? 'Meta' : 'Control'
    const shell = page.locator('.hierarchy-shell')
    const activeStage = page.locator('.scene-stage:not([hidden])')
    await expect(shell).toHaveAttribute('data-theme', 'light')
    expect(await shell.evaluate((node) => {
      const style = getComputedStyle(node)
      return [style.backgroundColor, style.backgroundImage]
    })).toEqual(['rgba(0, 0, 0, 0)', 'none'])

    await page.keyboard.press(`${mod}+/`)
    await expect(page.getByRole('dialog', { name: '快捷键列表' })).toBeVisible()
    await expect(page.getByRole('img', { name: '快捷键说明' })).toHaveAttribute('src', /white_mac/)
    await page.keyboard.press(`${mod}+i`)
    await expect(shell).toHaveAttribute('data-theme', 'dark')
    await expect(page.getByRole('img', { name: '快捷键说明' })).toHaveAttribute('src', /dark_mac/)
    await page.keyboard.press(`${mod}+/`)
    await expect(page.getByRole('dialog', { name: '快捷键列表' })).toHaveCount(0)

    await page.keyboard.down('Alt')
    await page.keyboard.up('Alt')
    await page.keyboard.down('Alt')
    await page.keyboard.up('Alt')
    await expect(page.getByRole('dialog', { name: '快捷键列表' })).toBeVisible()
    await page.getByRole('button', { name: '关闭快捷键列表' }).click()
    await expect(activeStage.locator('[data-testid="terminal-pane"][data-active="true"] .xterm-helper-textarea')).toBeFocused()

    await page.keyboard.press(`${mod}+i`)
    await expect(shell).toHaveAttribute('data-theme', 'light')
    await page.keyboard.press(`${mod}+t`)
    await expect(page.getByRole('tab')).toHaveCount(2)
    await page.keyboard.press(`${mod}+Shift+ArrowLeft`)
    await expect(page.getByRole('tab').first()).toHaveAttribute('aria-selected', 'true')

    await page.keyboard.press(`${mod}+d`)
    await expect(page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]')).toHaveCount(2)
    const activeBefore = await activeStage.locator('[data-testid="terminal-pane"][data-active="true"] .terminal-surface')
      .getAttribute('data-session-id')
    await page.keyboard.press(`${mod}+[`)
    await expect(activeStage.locator('[data-testid="terminal-pane"][data-active="true"] .terminal-surface'))
      .not.toHaveAttribute('data-session-id', activeBefore!)

    const terminal = activeStage.locator('.terminal-surface[data-session-id]').first()
    await terminal.hover()
    await page.keyboard.down(mod)
    await page.mouse.wheel(0, -120)
    await page.keyboard.up(mod)
    await expect(terminal).toHaveAttribute('data-font-size', '12')
    await page.keyboard.press(`${mod}+0`)
    await expect(terminal).toHaveAttribute('data-font-size', '11')

    await expect(activeStage.locator('[data-testid="terminal-pane"][data-active="true"] .xterm-helper-textarea')).toBeFocused()
    // Keep the literal search token out of the echoed command so the terminal
    // contains exactly one real result: the command output under test.
    await page.keyboard.type("printf 'MATOU_%s_TOKEN\\n' SEARCH")
    await page.keyboard.press('Enter')
    await expect(activeStage.locator('[data-testid="terminal-pane"][data-active="true"] .xterm-rows'))
      .toContainText('MATOU_SEARCH_TOKEN')
    await page.keyboard.press(`${mod}+f`)
    const search = page.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' })
    await search.fill('MATOU_SEARCH_TOKEN')
    await expect(page.locator('.terminal-search-bar__count')).toHaveText('1/1')
    await search.press(`${mod}+c`)
    await expect(page.getByRole('button', { name: '大小写敏感' })).toHaveClass(/is-active/)
    await search.press(`${mod}+r`)
    await expect(page.getByRole('button', { name: '正则表达式' })).toHaveClass(/is-active/)
    await search.press('Escape')
    await expect(search).toHaveCount(0)
    await expect(activeStage.locator('[data-testid="terminal-pane"][data-active="true"] .xterm-helper-textarea')).toBeFocused()

    await page.keyboard.press(`${mod}+w`)
    await expect(page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]')).toHaveCount(1)
  } finally { await fixture.close() }
})
