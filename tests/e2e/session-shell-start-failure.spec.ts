import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'
import { activeSurface, visibleSurfaces, waitForShell } from './fixtures/session-canvas-fixture'

test.describe('real Shell startup failure recovery', () => {
  test.setTimeout(60_000)

  test('keeps a failed first canvas actionable and starts the same Session after retry', async () => {
    const shellRoot = await mkdtemp('/tmp/matou-real-dead-shell-')
    const shell = join(shellRoot, 'qa-shell')
    await writeFile(shell, '#!/bin/sh\nexec /bin/zsh "$@"\n', { mode: 0o755 })
    await chmod(shell, 0o000)
    const fixture = await launchMatou({ env: { SHELL: shell } })
    try {
      const pane = fixture.page.locator('[data-testid="terminal-pane"]:visible')
      await expect(pane.getByText('会话启动失败')).toBeVisible()
      await expect(pane.locator('.session-start-failure-reason')).toContainText(shell)
      await expect(pane.getByRole('button', { name: '重试启动' })).toBeVisible()

      const sessionId = await pane.locator('.terminal-surface').getAttribute('data-session-id')
      expect(sessionId).toBeTruthy()
      await chmod(shell, 0o755)
      await pane.getByRole('button', { name: '重试启动' }).click()
      await waitForShell(fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"]`))
      await expect(fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"] .xterm-helper-textarea`))
        .toBeFocused()
    } finally {
      await fixture.close()
      await rm(shellRoot, { recursive: true, force: true })
    }
  })

  test('keeps a failed horizontal sibling at the queue end and removes only that card', async () => {
    const shellRoot = await mkdtemp('/tmp/matou-real-dead-shell-')
    const shell = join(shellRoot, 'qa-shell')
    await writeFile(shell, '#!/bin/sh\nexec /bin/zsh "$@"\n', { mode: 0o755 })
    const fixture = await launchMatou({ env: { SHELL: shell } })
    try {
      await waitForShell(activeSurface(fixture.page))
      const originalSessionId = await activeSurface(fixture.page).getAttribute('data-session-id')
      await chmod(shell, 0o000)
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)

      const panes = fixture.page.locator('[data-testid="terminal-pane"]:visible')
      const failedPane = panes.last()
      await expect(failedPane.getByText('会话启动失败')).toBeVisible()
      await expect(failedPane.locator('.session-start-failure-reason')).toContainText(shell)
      await expect(activeSurface(fixture.page)).toHaveAttribute('data-session-id', await failedPane.locator('.terminal-surface').getAttribute('data-session-id') ?? '')
      await expect(fixture.page.locator(`.terminal-surface[data-session-id="${originalSessionId}"]`)).toHaveCount(1)

      await failedPane.getByRole('button', { name: '移除失败会话' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
      await expect(activeSurface(fixture.page)).toHaveAttribute('data-session-id', originalSessionId ?? '')
    } finally {
      await fixture.close()
      await rm(shellRoot, { recursive: true, force: true })
    }
  })
})
