import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

import { expect, test } from '@playwright/test'

import { activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces } from './fixtures/session-canvas-fixture'

const exec = promisify(execFile)

test.describe('real Claude Fork and Git worktree', () => {
  test.setTimeout(240_000)

  test('Forks a real Claude conversation beside the selected card and still creates a child in an isolated worktree', async () => {
    const fixture = await launchSessionCanvas()
    try {
      const initiallyActive = activeSurface(fixture.page)
      const sourceSessionId = await initiallyActive.getAttribute('data-session-id')
      expect(sourceSessionId).toBeTruthy()
      const source = fixture.page.locator(`.terminal-surface[data-session-id="${sourceSessionId}"]`)
      await terminalCommand(source, 'claude --dangerously-skip-permissions')
      await expect(fixture.page.locator('.pane-title').filter({ hasText: 'Claude' })).toBeVisible({ timeout: 60_000 })
      await expect(source).toHaveAttribute('data-profile', 'claude-code', { timeout: 60_000 })
      await expect(source.locator('.xterm-rows')).toContainText('Yes, I trust this folder', { timeout: 30_000 })
      // Claude may preserve either trust choice between releases. Move only
      // after the initial full-screen redraw settles, then require two stable
      // "Yes" frames so Enter cannot race stale terminal output.
      const trustRows = source.locator('.xterm-rows')
      let settledChoice = ''
      let stableChoiceFrames = 0
      await expect.poll(async () => {
        const text = await trustRows.textContent() ?? ''
        const choice = text.includes('❯ Yes, I trust this folder')
          ? 'yes' : text.includes('❯ No, exit') ? 'no' : ''
        stableChoiceFrames = choice && choice === settledChoice ? stableChoiceFrames + 1 : choice ? 1 : 0
        settledChoice = choice
        return stableChoiceFrames
      }, { intervals: [100, 100, 100] }).toBeGreaterThanOrEqual(2)
      if (settledChoice !== 'yes') {
        await source.locator('.xterm-helper-textarea').press('ArrowDown')
      }
      let stableTrustedFrames = 0
      await expect.poll(async () => {
        const selected = (await trustRows.textContent() ?? '').includes('❯ Yes, I trust this folder')
        stableTrustedFrames = selected ? stableTrustedFrames + 1 : 0
        return stableTrustedFrames
      }, { intervals: [100, 100, 100] }).toBeGreaterThanOrEqual(2)
      await source.locator('.xterm-helper-textarea').press('Enter')
      await expect(source.locator('.xterm-rows')).toContainText('Claude Code v', { timeout: 60_000 })
      const marker = `MATOU_${Date.now()}`
      await terminalCommand(source, `Reply only with ${marker}`)
      await expect.poll(async () => {
        const text = await source.locator('.xterm-rows').textContent() ?? ''
        return text.split(marker).length - 1
      }, { timeout: 120_000 }).toBeGreaterThanOrEqual(2)

      const sourcePane = source.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
      await sourcePane.locator('.terminal-pane-header').click({ button: 'right' })
      await expect(fixture.page.getByRole('menuitem', { name: '⑂ Fork 兄弟分支' })).toHaveCount(0)
      await fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' }).click()
      await expect(fixture.page.getByRole('dialog', { name: 'Fork 会话' })).toBeVisible()
      await fixture.page.getByLabel('分支名称').fill('真实横向副本')
      await fixture.page.getByRole('button', { name: '创建分支', exact: true }).click()
      const peerPane = fixture.page.getByRole('article', { name: '会话：真实横向副本' })
      await expect(peerPane.getByRole('status', { name: /正在创建分支/ }))
        .toHaveCount(0, { timeout: 90_000 })
      await expect(peerPane.locator('.terminal-surface')).toHaveAttribute('data-profile', 'claude-code')
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      await expect(fixture.page.getByRole('navigation', { name: '会话层级' }))
        .toContainText('根会话 · 2 个会话')

      await source.click({ position: { x: 12, y: 12 } })
      await expect(sourcePane).toHaveAttribute('data-active', 'true')
      const fork = sourcePane.getByRole('button', { name: /创建子分支/ })
      await expect(fork).toBeEnabled({ timeout: 60_000 })
      await fork.click()
      await expect(fixture.page.getByRole('dialog', { name: '创建子会话分支' })).toBeVisible()
      await fixture.page.getByLabel('分支名称').fill('真实工作树分支')
      await fixture.page.getByText('从新工作树创建').click()
      await fixture.page.getByRole('button', { name: '创建分支', exact: true }).click()
      const childPane = fixture.page.getByRole('article', { name: '会话：真实工作树分支' })
      await expect(childPane.getByRole('status', { name: /正在创建分支/ }))
        .toHaveCount(0, { timeout: 90_000 })
      await expect(fixture.page.getByText(`${marker} 的子会话`)).toBeVisible()
      await expect(childPane.locator('.terminal-surface')).toHaveAttribute('data-profile', 'claude-code')
      await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
      let worktrees = await exec('git', ['worktree', 'list', '--porcelain'], { cwd: fixture.workspaceDirectory })
      await expect.poll(async () => {
        worktrees = await exec('git', ['worktree', 'list', '--porcelain'], { cwd: fixture.workspaceDirectory })
        return worktrees.stdout.match(/^worktree /gm)?.length ?? 0
      }, { timeout: 90_000 }).toBe(2)
      expect(worktrees.stdout.match(/^worktree /gm)?.length ?? 0).toBe(2)
      expect(worktrees.stdout).toContain('真实工作树分支')

      const childSessionId = await childPane.locator('.terminal-surface').getAttribute('data-session-id')
      expect(childSessionId).toBeTruthy()
      const childSurface = fixture.page.locator(
        `.terminal-surface[data-session-id="${childSessionId}"]`
      )
      const stableChildPane = childSurface.locator(
        'xpath=ancestor::*[@data-testid="terminal-pane"][1]'
      )
      const childMarker = `CHILD_${Date.now()}`
      await terminalCommand(childSurface, `Reply only with ${childMarker}`)
      await expect.poll(async () => {
        const text = await childSurface.locator('.xterm-rows').textContent() ?? ''
        return text.split(childMarker).length - 1
      }, { timeout: 120_000 }).toBeGreaterThanOrEqual(2)
      await stableChildPane.locator('.terminal-pane-header').click({ button: 'right' })
      await fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' }).click()
      await fixture.page.getByLabel('分支名称').fill('真实子层副本')
      await fixture.page.getByRole('button', { name: '创建分支', exact: true }).click()
      const childPeer = fixture.page.getByRole('article', { name: '会话：真实子层副本' })
      await expect(childPeer.getByRole('status', { name: /正在创建分支/ }))
        .toHaveCount(0, { timeout: 90_000 })
      await expect(childPeer.locator('.terminal-surface')).toHaveAttribute('data-profile', 'claude-code')
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      await expect(fixture.page.getByRole('navigation', { name: '会话层级' }))
        .toContainText('2 个会话')
    } finally {
      await fixture.close()
    }
  })
})
