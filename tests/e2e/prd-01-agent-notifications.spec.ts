import { expect, test, type Page } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('shows the reference product unread trail and navigates back to the originating terminal', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: '横向新增 Shell' }).click()
    const panes = page.locator('[data-testid="terminal-pane"]:visible')
    await expect(panes).toHaveCount(2)
    const origin = panes.first()
    const other = panes.last()
    await other.dispatchEvent('pointerdown')
    await expect(other).toHaveAttribute('data-active', 'true')

    const ids = await hierarchyIds(page, origin)
    await pushNotification(page, ids, {
      eventId: 'permission-1', eventType: 'permission', title: 'Claude Code',
      subtitle: 'Permission', body: 'Permission required to run the release verification command'
    })

    await expect(origin).toHaveClass(/has-notification/)
    await expect(origin.locator('.pane-notification-badge')).toHaveText('新通知')
    await expect(page.locator('.workbench-item__badge')).toHaveText('1')
    await expect(page.getByTestId(`scene-unread-${ids.sceneId}`)).toBeVisible()
    await expect(page.locator('.flat-sidebar__notify img')).toHaveAttribute('src', /rongzhi_ani-.*\.gif/)

    await page.getByRole('button', { name: '通知中心' }).click()
    const center = page.getByRole('region', { name: '通知中心' })
    await expect(center).toBeVisible()
    await expect(center).toContainText('通知 (1)')
    await expect(center).toContainText('Permission required to run the release verification command')
    await expect.poll(async () => center.evaluate((element) => {
      const rect = element.getBoundingClientRect()
      return { left: rect.left, top: rect.top, width: rect.width, bottomGap: innerHeight - rect.bottom }
    })).toEqual({ left: 0, top: 49, width: 382, bottomGap: 5 })

    await center.getByRole('button', { name: /打开通知/ }).click()
    await expect(center).toHaveCount(0)
    await expect(origin).toHaveAttribute('data-active', 'true')
    await expect(origin).not.toHaveClass(/has-notification/)
    await expect(page.locator('.workbench-item__badge')).toHaveCount(0)
    await expect(page.getByTestId(`scene-unread-${ids.sceneId}`)).toHaveCount(0)
  } finally {
    await fixture.close()
  }
})

test('keeps focused read events out of every unread indicator and highlights only the unread Session', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: '横向新增 Shell' }).click()
    const panes = page.locator('[data-testid="terminal-pane"]:visible')
    await expect(panes).toHaveCount(2)
    const unreadPane = panes.first()
    const focusedPane = panes.last()
    await focusedPane.dispatchEvent('pointerdown')

    await pushNotification(page, await hierarchyIds(page, focusedPane), {
      eventId: 'completed-1', eventType: 'completed', title: 'Claude Code',
      subtitle: 'Completed', body: 'The task is complete', isFocusedSession: true
    })

    await expect(focusedPane).not.toHaveClass(/has-notification/)
    await expect(focusedPane.locator('.pane-notification-badge')).toHaveCount(0)
    await expect(page.locator('.workbench-item__badge')).toHaveCount(0)
    await expect(page.locator('.tab-status-dot')).toHaveCount(0)

    await pushNotification(page, await hierarchyIds(page, unreadPane), {
      eventId: 'permission-2', eventType: 'permission', title: 'Claude Code',
      subtitle: 'Permission', body: 'Permission required'
    })

    await expect(page.locator('.workbench-item__badge')).toHaveText('1')
    await expect(panes.filter({ has: page.locator('.pane-notification-badge') })).toHaveCount(1)
    await expect(page.locator('.terminal-pane.has-notification')).toHaveCount(1)
    await expect(unreadPane).toHaveClass(/has-notification/)
    await expect(unreadPane.locator('.pane-notification-badge')).toHaveText('新通知')
    await expect(focusedPane).not.toHaveClass(/has-notification/)
  } finally {
    await fixture.close()
  }
})

test('closes outside or with Escape and remembers the sound preference', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    const pane = page.locator('[data-testid="terminal-pane"]:visible').first()
    await pushNotification(page, await hierarchyIds(page, pane), {
      eventId: 'waiting-1', eventType: 'waiting', title: 'Claude Code',
      subtitle: 'Waiting', body: 'Waiting for your input'
    })

    await page.getByRole('button', { name: '通知中心' }).click()
    const center = page.getByRole('region', { name: '通知中心' })
    await expect(center).toBeVisible()
    const checkbox = center.getByRole('checkbox', { name: '通知声音' })
    const switchTrack = center.locator('.notification-center__switch-track')
    if (!await checkbox.isChecked()) {
      await switchTrack.click()
      await expect(checkbox).toBeChecked()
    }
    await switchTrack.click()
    await expect(checkbox).not.toBeChecked()
    await expect.poll(() => page.evaluate(() => localStorage.getItem('kc-notification-sound-enabled'))).toBe('false')

    await page.keyboard.press('Escape')
    await expect(center).toHaveCount(0)
    await page.getByRole('button', { name: '通知中心' }).click()
    await expect(page.getByRole('checkbox', { name: '通知声音' })).not.toBeChecked()
    await page.locator('.workspace-stage').dispatchEvent('pointerdown')
    await expect(center).toHaveCount(0)
  } finally {
    await fixture.close()
  }
})

test('bounds two Workspace notification queues, navigates, and clears the retained center', async () => {
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    const pane = page.locator('[data-testid="terminal-pane"]:visible').first()
    const ids = await hierarchyIds(page, pane)

    await page.evaluate(({ ids }) => {
      if (!window.matouE2e) throw new Error('Matou E2E bridge is missing')
      for (let index = 0; index < 1_000; index += 1) {
        const eventId = `active-capacity-${index}`
        window.matouE2e.pushNotification({
          eventId, eventType: 'completed', title: '容量验收', body: eventId,
          workspaceId: ids.workspaceId, taskId: ids.taskId, sceneId: ids.sceneId,
          sessionId: `active-capacity-session-${index}`, cooldownKey: eventId, sound: false
        })
      }
      window.matouE2e.pushNotification({
        eventId: 'active-capacity-target', eventType: 'completed', title: '容量验收',
        body: 'active-capacity-target', ...ids, cooldownKey: 'active-capacity-target', sound: false
      })
      for (let index = 0; index < 1_001; index += 1) {
        const eventId = `other-capacity-${index}`
        window.matouE2e.pushNotification({
          eventId, eventType: 'completed', title: '另一工作空间', body: eventId,
          workspaceId: 'workspace-capacity-other', taskId: 'task-capacity-other',
          sceneId: 'scene-capacity-other', sessionId: `other-capacity-session-${index}`,
          cooldownKey: eventId, sound: false
        })
      }
    }, { ids })

    await expect(page.locator('.workbench-item__badge')).toHaveText('99+')
    await page.getByRole('button', { name: '通知中心' }).click()
    const center = page.getByRole('region', { name: '通知中心' })
    await expect(center).toContainText('通知 (2000)')
    await expect(center.getByRole('button', { name: '打开通知：active-capacity-0' })).toHaveCount(0)
    await expect(center.getByRole('button', { name: '打开通知：other-capacity-0' })).toHaveCount(0)

    await center.getByRole('button', { name: '打开通知：active-capacity-target' }).click()
    await expect(center).toHaveCount(0)
    await expect(pane).toHaveAttribute('data-active', 'true')

    await page.getByRole('button', { name: '通知中心' }).click()
    const reopened = page.getByRole('region', { name: '通知中心' })
    await expect(reopened).toContainText('通知 (1999)')
    await reopened.getByRole('button', { name: '清空通知' }).click()
    await expect(reopened).toContainText('通知 (0)')
    await expect(reopened).toContainText('暂无通知')
    await expect(page.locator('.workbench-item__badge')).toHaveCount(0)
  } finally {
    await fixture.close()
  }
})

async function hierarchyIds(page: Page, pane: ReturnType<Page['locator']>) {
  const workspaceId = await page.locator('.workspace-group.is-active').getAttribute('data-workspace-id')
  const taskTestId = await page.locator('.workbench-item.is-active').getAttribute('data-testid')
  const sceneId = await page.locator('.scene-stage:not([hidden])').evaluate((element) => {
    const label = element.getAttribute('aria-label') ?? ''
    const activeTab = document.querySelector<HTMLElement>('.tab-item.active')
    if (!activeTab?.dataset.sceneId) throw new Error(`Active Scene missing for ${label}`)
    return activeTab.dataset.sceneId
  })
  const sessionId = await pane.locator('.terminal-surface').getAttribute('data-session-id')
  if (!workspaceId || !taskTestId || !sessionId) throw new Error('Hierarchy identity is missing')
  return { workspaceId, taskId: taskTestId.replace(/^task-/, ''), sceneId, sessionId }
}

async function pushNotification(
  page: Page,
  ids: { workspaceId: string; taskId: string; sceneId: string; sessionId: string },
  input: {
    eventId: string; eventType: string; title: string; subtitle: string; body: string
    isFocusedSession?: boolean
  }
) {
  await page.evaluate(({ ids, input }) => {
    if (!window.matouE2e) throw new Error('Matou E2E bridge is missing')
    window.matouE2e.pushNotification({ ...input, ...ids, sound: false })
  }, { ids, input })
}
