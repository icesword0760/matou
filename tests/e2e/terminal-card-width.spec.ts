import { chmod, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test, type Locator } from '@playwright/test'

import { activeSurface, terminalCommand, visibleSurfaces, waitForShell } from './fixtures/session-canvas-fixture'
import { launchMatou } from './matou-fixture'

async function terminalWidth(surface: Locator) {
  return surface.evaluate((element) => {
    const viewport = element.querySelector<HTMLElement>('.terminal-surface__viewport')!
    const screen = element.querySelector<HTMLElement>('.xterm-screen')!
    return {
      viewport: viewport.getBoundingClientRect().width,
      screen: screen.getBoundingClientRect().width,
      cols: Number(viewport.dataset.terminalCols),
      rows: Number(viewport.dataset.terminalRows)
    }
  })
}

test('fills a hover-expanded terminal and keeps its width when clicked without another layout change', async ({}, testInfo) => {
  test.setTimeout(60_000)
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await waitForShell(activeSurface(page))
    await page.getByRole('button', { name: '横向新增 Shell' }).click()
    await page.getByRole('button', { name: '横向新增 Shell' }).click()
    await expect(visibleSurfaces(page)).toHaveCount(3)
    const firstId = await visibleSurfaces(page).first().getAttribute('data-session-id')
    const lastId = await visibleSurfaces(page).last().getAttribute('data-session-id')
    const first = page.locator(`.terminal-surface[data-session-id="${firstId}"]`)
    const last = page.locator(`.terminal-surface[data-session-id="${lastId}"]`)
    // Returning to a project mounts the inactive cards at their compact grid.
    const taskId = await page.locator('[data-testid^="task-"]').first().getAttribute('data-testid')
    await page.getByRole('button', { name: '在 matou_workspace 中新增事项' }).click()
    await waitForShell(activeSurface(page))
    await page.getByTestId(taskId!).click()
    await expect(visibleSurfaces(page)).toHaveCount(3)
    await waitForShell(first)
    const card = first.locator('xpath=ancestor::*[@data-session-card][1]')
    const pane = first.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
    await last.locator('xpath=ancestor::*[@data-session-card][1]').click({ position: { x: 120, y: 25 } })
    await expect(activeSurface(page)).toHaveAttribute('data-session-id', lastId!)
    await page.mouse.move(10, 10)
    await expect(card).not.toHaveClass(/is-expanded/)
    await page.waitForTimeout(900)
    await expect(pane).toHaveAttribute('data-active', 'false')
    const compact = await terminalWidth(first)

    await card.hover({ position: { x: 120, y: 25 } })
    await expect(card).toHaveClass(/is-expanded/)
    await page.waitForTimeout(900)
    await expect(card).toHaveClass(/is-expanded/)
    const hovered = await terminalWidth(first)
    await expect(pane).toHaveAttribute('data-active', 'false')
    await page.screenshot({ path: testInfo.outputPath('hover-expanded.png') })

    // The preview is already wide. Clicking it changes focus, not its width,
    // so this also covers activation without a new ResizeObserver event.
    await card.click({ position: { x: 120, y: 25 } })
    await expect(pane).toHaveAttribute('data-active', 'true')
    await page.waitForTimeout(900)
    const focused = await terminalWidth(first)
    console.log(JSON.stringify({ compact, hovered, focused }))
    await page.screenshot({ path: testInfo.outputPath('focused.png') })

    expect(hovered.viewport).toBeGreaterThan(compact.viewport + 50)
    expect(hovered.cols).toBeGreaterThan(compact.cols)
    // Padding, scrollbar, and one fractional character cell are legitimate.
    expect(hovered.viewport - hovered.screen).toBeLessThan(50)
    expect(focused.viewport - focused.screen).toBeLessThan(50)
  } finally {
    await fixture.close()
  }
})

test('resizes a continuously redrawing provider once per preview transition without stealing focus', async ({}, testInfo) => {
  test.setTimeout(60_000)
  const root = await mkdtemp('/tmp/matou-e2e-preview-grid-')
  const provider = join(root, 'provider.sh')
  const script = join(root, 'provider.mjs')
  const resizeLog = join(root, 'resize.log')
  await writeFile(resizeLog, '')
  await writeFile(script, `
import { appendFileSync } from 'node:fs';
process.stdin.setRawMode(true);
process.stdin.resume();
const draw = () => {
  const cols = process.stdout.columns;
  const rows = process.stdout.rows;
  const border = '+' + '-'.repeat(cols - 2) + '+';
  process.stdout.write('\\x1b[2J\\x1b[H' + border + '\\r\\n' +
    'GRID ' + cols + 'x' + rows + '\\r\\n' +
    '预览展开后，中文内容与右侧边框应完整显示。\\r\\n' + border);
};
process.stdout.on('resize', () => {
  appendFileSync(${JSON.stringify(resizeLog)}, process.stdout.columns + 'x' + process.stdout.rows + '\\n');
  draw();
});
setInterval(draw, 50);
draw();
`)
  await writeFile(provider, `#!/bin/sh\nexec '${process.execPath}' '${script}'\n`)
  await chmod(provider, 0o755)
  await writeFile(join(root, '.zshrc'), "alias cc='claude'\n")
  const fixture = await launchMatou({ root, env: {
    SHELL: '/bin/zsh', ZDOTDIR: root, MATOU_CLAUDE_COMMAND: provider
  } })
  try {
    const { page } = fixture
    const id = await activeSurface(page).getAttribute('data-session-id')
    const surface = page.locator(`.terminal-surface[data-session-id="${id}"]`)
    await terminalCommand(surface, 'cc')
    await expect(surface).toHaveAttribute('data-profile', 'claude-code')
    await expect(surface.locator('.xterm-rows')).toContainText('GRID')
    const pid = await surface.getAttribute('data-pid')
    await page.getByRole('button', { name: '横向新增 Shell' }).click()
    const card = surface.locator('xpath=ancestor::*[@data-session-card][1]')
    const pane = surface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
    await page.mouse.move(10, 10)
    await page.waitForTimeout(900)

    for (let cycle = 0; cycle < 3; cycle++) {
      const compact = await terminalWidth(surface)
      await writeFile(resizeLog, '')
      await card.hover({ position: { x: 120, y: 25 } })
      await expect(card).toHaveClass(/is-expanded/)
      await page.waitForTimeout(900)
      const expanded = await terminalWidth(surface)
      await expect(pane).toHaveAttribute('data-active', 'false')
      await expect(surface.locator('.xterm-helper-textarea')).not.toBeFocused()
      await expect(surface).toHaveAttribute('data-pid', pid!)
      expect(expanded.cols).toBeGreaterThan(compact.cols)
      expect(expanded.viewport - expanded.screen).toBeLessThan(50)
      const grid = `${expanded.cols}x${expanded.rows}`
      await expect(surface.locator('.xterm-rows')).toContainText(`GRID ${grid}`)
      await expect(surface.locator('.xterm-rows > div').first()).toHaveText(
        '+' + '-'.repeat(expanded.cols - 2) + '+'
      )
      expect((await readFile(resizeLog, 'utf8')).trim().split('\n')).toEqual([grid])
      if (cycle === 0) await page.screenshot({ path: testInfo.outputPath('provider-expanded.png') })
      await page.mouse.move(10, 10)
      await page.waitForTimeout(900)
      const collapsed = await terminalWidth(surface)
      expect(collapsed.cols).toBe(compact.cols)
      await expect(surface.locator('.xterm-rows')).toContainText(`GRID ${collapsed.cols}x${collapsed.rows}`)
    }

    // In a narrow window this inactive card is a summary, not a visible
    // terminal. Hiding it must not shrink the still-running provider to 2 cols.
    const beforeHidden = await terminalWidth(surface)
    await writeFile(resizeLog, '')
    const bounds = await fixture.app.evaluate(({ BrowserWindow }) => {
      const window = BrowserWindow.getAllWindows()[0]!
      const previous = window.getBounds()
      window.setSize(850, 800)
      return previous
    })
    await expect(page.locator('.session-carousel')).toHaveClass(/is-narrow/)
    await expect(surface).not.toBeVisible()
    await page.waitForTimeout(900)
    expect((await terminalWidth(surface)).cols).toBe(beforeHidden.cols)
    expect(await readFile(resizeLog, 'utf8')).toBe('')

    await fixture.app.evaluate(({ BrowserWindow }, previous) => {
      BrowserWindow.getAllWindows()[0]!.setBounds(previous)
    }, bounds)
    await expect(surface).toBeVisible()
    await page.waitForTimeout(900)
    const revealed = await terminalWidth(surface)
    expect(revealed.viewport - revealed.screen).toBeLessThan(50)
    await expect(surface.locator('.xterm-rows')).toContainText(`GRID ${revealed.cols}x${revealed.rows}`)
  } finally { await fixture.close() }
})
