import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

import { expect, test } from '@playwright/test'

import { activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces } from './fixtures/session-canvas-fixture'

const exec = promisify(execFile)

test.describe('real Claude Fork and Git worktree', () => {
  test.setTimeout(240_000)

  test('forks a real resumable Claude conversation into a real isolated worktree', async () => {
    const fixture = await launchSessionCanvas()
    try {
      const source = activeSurface(fixture.page)
      await terminalCommand(source, 'claude --dangerously-skip-permissions')
      await expect(fixture.page.locator('.pane-title').filter({ hasText: 'Claude' })).toBeVisible({ timeout: 60_000 })
      await expect(source.locator('.xterm-rows')).toContainText('Yes, I trust this folder', { timeout: 30_000 })
      // Current Claude Code starts on the conservative "No, exit" choice.
      // Select the visible trust choice explicitly rather than relying on a
      // version-dependent default selection.
      await source.locator('.xterm-helper-textarea').press('ArrowDown')
      await expect.poll(async () => (await source.locator('.xterm-rows').textContent() ?? '')
        .includes('❯ Yes, I trust this folder')).toBe(true)
      await source.locator('.xterm-helper-textarea').press('Enter')
      await expect(source.locator('.xterm-rows')).toContainText('Claude Code v', { timeout: 60_000 })
      const marker = `MATOU_${Date.now()}`
      await terminalCommand(source, `Reply only with ${marker}`)
      await expect.poll(async () => {
        const text = await source.locator('.xterm-rows').textContent() ?? ''
        return text.split(marker).length - 1
      }, { timeout: 120_000 }).toBeGreaterThanOrEqual(2)
      const fork = fixture.page.getByRole('button', { name: /创建子分支/ })
      await expect(fork).toBeEnabled({ timeout: 60_000 })
      await fork.focus()
      await fixture.page.keyboard.press('Enter')
      await fixture.page.getByLabel('分支名称').fill('真实工作树分支')
      await fixture.page.getByText('从新工作树创建').click()
      await fixture.page.getByRole('button', { name: '创建分支', exact: true }).click()
      await expect(fixture.page.getByText('Claude 的子会话')).toBeVisible({ timeout: 90_000 })
      await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
      let worktrees = await exec('git', ['worktree', 'list', '--porcelain'], { cwd: fixture.workspaceDirectory })
      await expect.poll(async () => {
        worktrees = await exec('git', ['worktree', 'list', '--porcelain'], { cwd: fixture.workspaceDirectory })
        return worktrees.stdout.match(/^worktree /gm)?.length ?? 0
      }, { timeout: 90_000 }).toBe(2)
      expect(worktrees.stdout.match(/^worktree /gm)?.length ?? 0).toBe(2)
      expect(worktrees.stdout).toContain('真实工作树分支')
    } finally {
      await fixture.close()
    }
  })
})
