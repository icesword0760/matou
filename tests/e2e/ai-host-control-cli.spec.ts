import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test, type Locator } from '@playwright/test'

import { launchMatou } from './matou-fixture'
import { terminalCommand, visibleSurfaces, waitForShell } from './fixtures/session-canvas-fixture'

test('identifies the caller and sends to a sibling without moving the Matou UI', async () => {
  test.setTimeout(60_000)
  const fixture = await launchMatou()
  try {
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 820))
    const first = visibleSurfaces(fixture.page).first()
    await waitForShell(first)
    const shellReadyFile = join(fixture.rootDirectory, 'shell-ready')
    await terminalCommand(first, `printf ready > ${shellQuote(shellReadyFile)}`)
    await expect.poll(async () => {
      try {
        return await readFile(shellReadyFile, 'utf8')
      } catch {
        return undefined
      }
    }, {
      message: '等待真实 Shell 执行首条命令',
      timeout: 30_000
    }).toBe('ready')
    const firstSessionId = await first.getAttribute('data-session-id')
    expect(firstSessionId).toBeTruthy()

    const identifyFile = join(fixture.rootDirectory, 'mt-identify.json')
    const identity = await runMtJson(first, 'mt identify --json', identifyFile) as {
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
    const result = await runMtJson(
      stableFirst,
      'mt send right "printf __MT_REMOTE_OK__" --enter --json',
      resultFile
    ) as { sent: boolean }
    await expect(second.locator('.xterm-rows')).toContainText('__MT_REMOTE_OK__')
    expect(result.sent).toBe(true)
    expect(await uiState(fixture.page.locator('body'))).toEqual(uiBefore)

    const readFilePath = join(fixture.rootDirectory, 'mt-read.json')
    const current = await runMtJson(stableFirst, 'mt read right --json', readFilePath) as {
      source: string
      text: string
    }
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

async function runMtJson(surface: Locator, command: string, outputPath: string): Promise<unknown> {
  const statusPath = `${outputPath}.status`
  const stderrPath = `${outputPath}.stderr`
  await typeCommand(
    surface,
    `${command} > ${shellQuote(outputPath)} 2> ${shellQuote(stderrPath)}; ` +
      `printf '%s' $? > ${shellQuote(statusPath)}`
  )

  await expect.poll(async () => {
    try {
      return (await readFile(statusPath, 'utf8')).trim()
    } catch {
      return undefined
    }
  }, {
    message: `等待真实 mt 命令结束：${command}`,
    timeout: 12_000
  }).not.toBeUndefined()
  const status = (await readFile(statusPath, 'utf8')).trim()

  if (status !== '0') {
    const stderr = await readFile(stderrPath, 'utf8').catch(() => '')
    const terminal = await surface.locator('.xterm-rows').innerText().catch(() => '')
    throw new Error(
      `mt command exited with ${status}: ${command}\nstderr: ${stderr.trim()}\nterminal:\n${terminal}`
    )
  }

  const json = await readFile(outputPath, 'utf8')
  try {
    return JSON.parse(json)
  } catch {
    throw new Error(`mt command returned invalid JSON: ${command}\nstdout: ${json}`)
  }
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'\\''`)}'`
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
