import { expect, test } from '@playwright/test'
import { DatabaseSync } from 'node:sqlite'
import { join } from 'node:path'

import {
  launchSessionCanvas,
  terminalCommand,
  visibleSurfaces
} from './fixtures/session-canvas-fixture'

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

  test('mirrors a live Session notification into the DAG until selecting that node clears it', async () => {
    const fixture = await launchSessionCanvas()
    try {
      const surface = visibleSurfaces(fixture.page).first()
      const sessionId = await surface.getAttribute('data-session-id')
      expect(sessionId).toBeTruthy()

      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      let dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      const node = dag.locator(`.dag-node-card[data-session-id="${sessionId}"]`)
      await expect(node).not.toHaveClass(/has-notification/)

      await fixture.page.evaluate((targetSessionId) => {
        if (!window.matouE2e) throw new Error('Matou E2E bridge is missing')
        const workspaceId = document.querySelector<HTMLElement>('.workspace-group.is-active')?.dataset.workspaceId
        const taskId = document.querySelector<HTMLElement>('.workbench-item.is-active')?.dataset.testid?.replace(/^task-/, '')
        const sceneId = document.querySelector<HTMLElement>('.tab-item.active')?.dataset.sceneId
        if (!workspaceId || !taskId || !sceneId) throw new Error('Hierarchy identity is missing')
        window.matouE2e.pushNotification({
          eventId: 'dag-notification-e2e', eventType: 'attention',
          title: 'Shell', body: 'DAG node attention', sound: false,
          workspaceId, taskId, sceneId, sessionId: targetSessionId,
          isFocusedSession: true
        })
      }, sessionId)
      await expect(node).toHaveClass(/has-notification/)

      await node.click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await expect(dag.locator(`.dag-node-card[data-session-id="${sessionId}"]`))
        .not.toHaveClass(/has-notification/)
    } finally {
      await fixture.close()
    }
  })

  test('returns from DAG with only the selected Session expanded', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      const dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await dag.getByRole('button', { name: /^打开会话：/ }).first().click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
      await fixture.page.mouse.move(2, 2)
      await fixture.page.waitForTimeout(500)

      const slots = fixture.page.locator('.session-card-slot')
      await expect(slots).toHaveCount(2)
      await expect(slots.filter({ has: fixture.page.locator('.session-card.is-focused') })).toHaveCount(1)
      const widths = await slots.evaluateAll((elements) => elements.map((element) => ({
        focused: element.classList.contains('is-focused'),
        expanded: element.classList.contains('is-expanded'),
        width: element.getBoundingClientRect().width
      })))
      const selected = widths.find(({ focused }) => focused)!
      const inactive = widths.find(({ focused }) => !focused)!
      expect(widths.filter(({ expanded }) => expanded)).toHaveLength(1)
      expect(inactive.width).toBeLessThan(selected.width * 0.75)
    } finally {
      await fixture.close()
    }
  })

  test('opens immediately on Option Tab keydown', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await expect(fixture.page.getByRole('button', { name: '打开会话 DAG' })).toBeVisible()
      await fixture.app.evaluate(({ BrowserWindow }) => {
        BrowserWindow.getAllWindows()[0]!.webContents.sendInputEvent({
          type: 'keyDown', keyCode: 'Tab', modifiers: ['alt']
        })
      })
      await expect.poll(async () => (await fixture.app.windows()).length, {
        timeout: 1_000,
        intervals: [20, 40, 80]
      }).toBe(2)
      const resized = await fixture.app.evaluate(({ BrowserWindow }) => {
        const dag = BrowserWindow.getAllWindows().find((window) => window.webContents.getURL().includes('kind=dag'))!
        const before = dag.getBounds()
        dag.setSize(Math.max(680, before.width - 60), before.height)
        return { before, after: dag.getBounds(), resizable: dag.isResizable() }
      })
      expect(resized.resizable).toBe(true)
      expect(resized.after.width).toBeLessThan(resized.before.width)
      await fixture.app.evaluate(({ BrowserWindow }) => {
        BrowserWindow.getAllWindows().find((window) => !window.webContents.getURL().includes('kind=dag'))
          ?.webContents.sendInputEvent({
          type: 'keyUp', keyCode: 'Tab', modifiers: ['alt']
        })
      })
    } finally {
      await fixture.close()
    }
  })

  test('restores the last zoom and pan after the short-lived native DAG window closes', async () => {
    const fixture = await launchSessionCanvas()
    try {
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      let dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      const canvas = dag.getByRole('application', { name: '会话 DAG 画布' })
      await dag.getByRole('button', { name: '放大' }).click()
      await dag.getByRole('button', { name: '放大' }).click()
      await canvas.dispatchEvent('wheel', { deltaX: 140, deltaY: 45 })
      await expect(canvas).toHaveAttribute('data-scale', '1.2')
      const persistedPan = await canvas.getAttribute('data-pan')
      // The native window closes on keyDown, before Playwright can send keyUp.
      // Swallow only that expected target-destroyed completion and prove closure below.
      await dag.keyboard.press('Escape').catch(() => {})
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)

      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      const restored = dag.getByRole('application', { name: '会话 DAG 画布' })
      await expect(restored).toHaveAttribute('data-scale', '1.2')
      await expect(restored).toHaveAttribute('data-pan', persistedPan!)
    } finally {
      await fixture.close()
    }
  })

  test('searches recent output and returns a hidden sibling to the viewport with terminal focus', async () => {
    const fixture = await launchSessionCanvas()
    try {
      for (let index = 0; index < 5; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      }
      await expect(visibleSurfaces(fixture.page)).toHaveCount(6)
      const target = visibleSurfaces(fixture.page).last()
      const targetSessionId = await target.getAttribute('data-session-id')
      expect(targetSessionId).toBeTruthy()
      await terminalCommand(target, "printf 'DAG_FAR_NODE_UNIQUE_830\\n'")
      const stableTarget = fixture.page.locator(
        `.terminal-surface[data-session-id="${targetSessionId}"]`
      )
      await expect(stableTarget.locator('.xterm-rows')).toContainText('DAG_FAR_NODE_UNIQUE_830')

      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await carousel.evaluate((element, sessionId) => {
        const slot = element.querySelector<HTMLElement>(`[data-session-id="${sessionId}"]`)!
        const maxScrollLeft = Math.max(0, element.scrollWidth - element.clientWidth)
        const targetCenter = slot.offsetLeft + slot.offsetWidth / 2
        element.scrollLeft = targetCenter < element.scrollWidth / 2 ? maxScrollLeft : 0
      }, targetSessionId)
      await expect(stableTarget).not.toBeInViewport()
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      const dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await dag.getByRole('searchbox', { name: '搜索会话' }).fill('DAG_FAR_NODE_UNIQUE_830')
      const result = dag.getByRole('option').filter({ hasText: 'Shell' })
      await expect(result).toHaveCount(1)
      await result.click()

      await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
      const restoredTarget = fixture.page.locator(`.terminal-surface[data-session-id="${targetSessionId}"]`)
      await expect(restoredTarget).toBeInViewport()
      await expect(restoredTarget.locator('.xterm-helper-textarea')).toBeFocused()
    } finally {
      await fixture.close()
    }
  })

  test('shows an explicit Shell read prompt as waiting for input and clears it after submission', async () => {
    const fixture = await launchSessionCanvas()
    try {
      const surface = visibleSurfaces(fixture.page).first()
      const sessionId = await surface.getAttribute('data-session-id')
      expect(sessionId).toBeTruthy()
      await terminalCommand(surface, "read \"value?STA008_WAIT> \"; printf 'STA008_GOT:%s\\n' \"$value\"")
      await expect(surface.locator('.xterm-rows')).toContainText('STA008_WAIT>')
      await expect.poll(() => {
        const database = new DatabaseSync(join(fixture.dataDirectory, 'matou.sqlite'), { readOnly: true })
        try {
          return database.prepare('SELECT work_status FROM sessions WHERE id = ?').get(sessionId!)
        } finally {
          database.close()
        }
      }).toEqual({ work_status: 'needs-input' })
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      let dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await expect(dag.locator('.dag-node-card.status-needs-input')).toContainText('等待输入')
      await dag.keyboard.press('Escape').catch(() => {})
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)

      const textarea = surface.locator('.xterm-helper-textarea')
      await textarea.focus()
      await textarea.pressSequentially('confirmed')
      await textarea.press('Enter')
      await expect(surface.locator('.xterm-rows')).toContainText('STA008_GOT:confirmed')
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await expect(dag.locator('.dag-node-card.status-idle')).toContainText('空闲')
    } finally {
      await fixture.close()
    }
  })
})
