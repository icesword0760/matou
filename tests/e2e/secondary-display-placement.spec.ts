import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('keeps the main, detached terminal, and DAG acceptance windows on the internal display', async () => {
  test.skip(process.platform !== 'darwin', 'the accepted display names are macOS-specific')
  const fixture = await launchMatou()
  try {
    const target = await fixture.app.evaluate(({ screen }) => {
      const primaryId = screen.getPrimaryDisplay().id
      const candidates = screen.getAllDisplays().filter(({ id }) => id !== primaryId)
      const display = candidates.find(({ internal }) => internal) ??
        candidates.find(({ label }) => /color\s*lcd|built[- ]?in|内建/i.test(label))
      return display ? {
        id: display.id,
        label: display.label,
        internal: display.internal,
        workArea: display.workArea
      } : undefined
    })
    expect(target, 'the internal secondary display must be connected for visible acceptance').toBeTruthy()

    await expectAllWindowsInsideTarget(fixture, target!.workArea, 1)

    await fixture.page.locator('.terminal-pane-header').first()
      .dispatchEvent('dragend', { screenX: -1, screenY: -1 })
    await expect(fixture.page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expectAllWindowsInsideTarget(fixture, target!.workArea, 2)
    const detached = (await fixture.app.windows()).find((candidate) => candidate !== fixture.page)!
    await detached.close()
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)

    await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
    await expectAllWindowsInsideTarget(fixture, target!.workArea, 2)
    const dag = (await fixture.app.windows()).find((candidate) => candidate !== fixture.page)!
    await expect(dag.getByRole('application', { name: '会话 DAG 画布' })).toBeVisible()
  } finally {
    await fixture.close()
  }
})

async function expectAllWindowsInsideTarget(
  fixture: Awaited<ReturnType<typeof launchMatou>>,
  target: { x: number; y: number; width: number; height: number },
  count: number
): Promise<void> {
  await expect.poll(async () => fixture.app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows().filter((window) => window.isVisible()).map((window) => window.getBounds())
  )).toHaveLength(count)
  const bounds = await fixture.app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows().filter((window) => window.isVisible()).map((window) => window.getBounds())
  )
  for (const window of bounds) {
    expect(window.x).toBeGreaterThanOrEqual(target.x)
    expect(window.y).toBeGreaterThanOrEqual(target.y)
    expect(window.x + window.width).toBeLessThanOrEqual(target.x + target.width)
    expect(window.y + window.height).toBeLessThanOrEqual(target.y + target.height)
  }
}
