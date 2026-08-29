import { expect, test } from '@playwright/test'

import { launchSessionCanvas, visibleSurfaces } from './fixtures/session-canvas-fixture'

test.describe('native session DAG window', () => {
  test.setTimeout(60_000)

  test('opens outside the main client, pans and zooms as a canvas, then returns to a visible node', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(3)
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()

      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      const dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await expect(dag.getByRole('application', { name: '会话 DAG 画布' })).toBeVisible()
      await expect(dag.getByRole('button', { name: /^打开会话：/ })).toHaveCount(3)
      const canvas = dag.getByRole('application', { name: '会话 DAG 画布' })
      const initialPan = await canvas.getAttribute('data-pan')
      await canvas.dispatchEvent('wheel', { deltaX: 180, deltaY: 70 })
      await expect.poll(() => canvas.getAttribute('data-pan')).not.toBe(initialPan)
      await dag.getByRole('button', { name: '放大' }).click()
      await expect(canvas).toHaveAttribute('data-scale', /1\.[1-9]|2/)
      await dag.getByRole('button', { name: '恢复 100%' }).click()
      await expect(canvas).toHaveAttribute('data-scale', '1')

      await dag.getByRole('button', { name: /^打开会话：/ }).last().click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
      await expect(fixture.page.locator('[aria-current="true"][aria-label^="会话："]')).toBeInViewport()
    } finally {
      await fixture.close()
    }
  })

  test('opens on a long Option Tab hold while a short hold remains terminal input', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => {
        BrowserWindow.getAllWindows()[0]!.webContents.sendInputEvent({
          type: 'keyDown', keyCode: 'Tab', modifiers: ['alt']
        })
      })
      await fixture.page.waitForTimeout(520)
      await fixture.app.evaluate(({ BrowserWindow }) => {
        BrowserWindow.getAllWindows()[0]!.webContents.sendInputEvent({
          type: 'keyUp', keyCode: 'Tab', modifiers: ['alt']
        })
      })
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
    } finally {
      await fixture.close()
    }
  })
})
