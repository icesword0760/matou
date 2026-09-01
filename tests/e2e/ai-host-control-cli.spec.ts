import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test, type Locator } from '@playwright/test'

import { launchMatou } from './matou-fixture'
import { terminalCommand, visibleSurfaces, waitForShell } from './fixtures/session-canvas-fixture'

test('identifies the caller and sends to a sibling without moving the Matou UI', async () => {
  const fixture = await launchMatou()
  try {
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
    const first = visibleSurfaces(fixture.page).first()
    await waitForShell(first)
    const firstSessionId = await first.getAttribute('data-session-id')
    expect(firstSessionId).toBeTruthy()

    const identifyFile = join(fixture.rootDirectory, 'mt-identify.json')
    await terminalCommand(first, `mt identify --json > ${identifyFile}`)
    const identity = await waitForJson(identifyFile) as {
      target: { session: { id: string }; canvas: { name: string }; dag: { depth: number } }
    }
    expect(identity.target.session.id).toBe(firstSessionId)
    expect(identity.target.canvas.name).toBeTruthy()
    expect(identity.target.dag.depth).toBe(0)

    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
    const stableFirst = fixture.page.locator(`.terminal-surface[data-session-id="${firstSessionId}"]`)
    const second = fixture.page.locator(
      `.scene-stage:not([hidden]) .terminal-surface:not([data-session-id="${firstSessionId}"])`
    ).first()
    await waitForShell(second)

    await focusTerminal(stableFirst)
    const uiBefore = await uiState(fixture.page.locator('body'))
    const resultFile = join(fixture.rootDirectory, 'mt-send.json')
    await typeCommand(
      stableFirst,
      `mt send right "printf __MT_REMOTE_OK__" --enter --json > ${resultFile}`
    )
    await expect(second.locator('.xterm-rows')).toContainText('__MT_REMOTE_OK__')
    const result = await waitForJson(resultFile) as { sent: boolean }
    expect(result.sent).toBe(true)
    expect(await uiState(fixture.page.locator('body'))).toEqual(uiBefore)

    const readFilePath = join(fixture.rootDirectory, 'mt-read.json')
    await typeCommand(stableFirst, `mt read right --json > ${readFilePath}`)
    const current = await waitForJson(readFilePath) as { source: string; text: string }
    expect(current.source).toBe('screen')
    expect(current.text).toContain('__MT_REMOTE_OK__')
  } finally {
    await fixture.close()
  }
})

async function focusTerminal(surface: Locator): Promise<void> {
  const pane = surface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
  await surface.click({ position: { x: 12, y: 12 } })
  await expect(pane).toHaveAttribute('data-active', 'true')
  await surface.locator('.xterm-helper-textarea').focus()
}

async function typeCommand(surface: Locator, command: string): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(command, { delay: 1 })
  await textarea.press('Enter')
}

async function uiState(root: Locator): Promise<Record<string, unknown>> {
  return root.evaluate((element) => {
    const focused = element.querySelector<HTMLElement>('.session-card.is-focused')
    const carousel = element.querySelector<HTMLElement>('[aria-label="同级会话列表"]')
    return {
      focusedSessionId: focused?.dataset.sessionCard ?? null,
      scrollLeft: carousel?.scrollLeft ?? 0,
      notifyingCards: element.querySelectorAll('.session-card.is-notifying').length,
      notificationBadges: element.querySelectorAll('[data-testid="notification-badge"]').length
    }
  })
}

async function waitForJson(path: string): Promise<unknown> {
  const deadline = Date.now() + 5_000
  while (Date.now() < deadline) {
    try {
      return JSON.parse(await readFile(path, 'utf8'))
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 40))
    }
  }
  throw new Error(`JSON evidence was not written: ${path}`)
}
