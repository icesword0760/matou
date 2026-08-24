import { expect, test } from '@playwright/test'

import { launchMatou, windowId } from './matou-fixture'

test('moves one complete Task to another main window without restarting its terminal', async () => {
  const fixture = await launchMatou()
  try {
    const { app, page: source } = fixture
    await expect(source.getByTestId('terminal-pane').first().locator('.terminal-surface'))
      .toHaveAttribute('data-pid', /\d+/)
    const originalPid = await source.getByTestId('terminal-pane').first().locator('.terminal-surface').getAttribute('data-pid')
    const taskTestId = await source.locator('[data-testid^="task-"]').first().getAttribute('data-testid')
    const taskId = taskTestId!.slice('task-'.length)

    await source.evaluate(() => { window.open('about:blank') })
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const target = (await app.windows()).find((candidate) => candidate !== source)!
    await expect(target.getByTestId('workspace-name')).toContainText('matou_workspace')

    await source.evaluate(async ({ taskId, sourceWindowId, targetWindowId }) => {
      await window.matouE2e!.moveTaskToWindow({
        migrationId: crypto.randomUUID(), taskId, sourceWindowId, targetWindowId
      })
    }, { taskId, sourceWindowId: windowId(source), targetWindowId: windowId(target) })
    await target.reload()

    await expect(source.locator(`[data-testid="task-${taskId}"]`)).toHaveCount(0)
    await expect(target.locator(`[data-testid="task-${taskId}"]`)).toHaveCount(1)
    await expect(target.getByTestId('terminal-pane').first().locator('.terminal-surface'))
      .toHaveAttribute('data-pid', originalPid!)
  } finally { await fixture.close() }
})
