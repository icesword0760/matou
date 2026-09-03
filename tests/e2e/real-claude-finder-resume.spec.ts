import { execFile } from 'node:child_process'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'

import { expect, test, type Locator } from '@playwright/test'

import {
  launchMatou, restartMatouGracefully, type MatouFixture
} from './matou-fixture'
import {
  terminalCommand, visibleSurfaces, waitForShell
} from './fixtures/session-canvas-fixture'

const execFileAsync = promisify(execFile)
const CLAUDE = '/Users/icesword/.nvm/versions/node/v22.16.0/bin/claude'
const FINDER_PATH = '/usr/bin:/bin:/usr/sbin:/sbin'

test.describe('real Claude Finder-launch resume', () => {
  test.setTimeout(180_000)

  test('finds the real provider and resumes its identity with the minimal macOS GUI PATH', async () => {
    const { stdout: version } = await execFileAsync(CLAUDE, ['--version'])
    expect(version).toContain('Claude Code')
    const { stdout: auth } = await execFileAsync(CLAUDE, ['auth', 'status'])
    expect(JSON.parse(auth)).toMatchObject({ loggedIn: true })

    let fixture: MatouFixture = await launchMatou({ env: { PATH: FINDER_PATH } })
    try {
      const first = visibleSurfaces(fixture.page).first()
      await waitForShell(first)
      const sessionId = await requiredAttribute(first, 'data-session-id')
      await terminalCommand(first, 'claude --dangerously-skip-permissions')
      await expect(first).toHaveAttribute('data-profile', 'claude-code', { timeout: 60_000 })
      await completeTrustPromptIfPresent(first)
      await expect(first.locator('.xterm-rows')).toContainText('Claude Code v', {
        timeout: 60_000
      })
      const marker = `MATOU_FINDER_RESUME_${Date.now()}`
      await terminalCommand(first, `Reply only with ${marker}`)
      await expect.poll(
        () => sessionWorkStatus(fixture.dataDirectory, sessionId),
        { timeout: 30_000 }
      ).toBe('running')
      await expect.poll(
        () => sessionWorkStatus(fixture.dataDirectory, sessionId),
        { timeout: 120_000 }
      ).toBe('idle')
      const binding = await waitForProviderBinding(fixture.dataDirectory, sessionId)
      const firstPid = Number(await requiredAttribute(first, 'data-pid'))

      fixture = await restartMatouGracefully(fixture, { env: { PATH: FINDER_PATH } })
      const resumed = fixture.page.locator(
        `.terminal-surface[data-session-id="${sessionId}"]`
      )
      await expect(resumed).toHaveAttribute('data-profile', 'claude-code', { timeout: 60_000 })
      await expect(resumed).toHaveAttribute('data-pid', /[1-9][0-9]*/, { timeout: 90_000 })
      await expect(
        resumed.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
          .locator('.session-recovery-overlay')
      ).toHaveCount(0, { timeout: 90_000 })
      const resumedPid = Number(await requiredAttribute(resumed, 'data-pid'))
      expect(resumedPid).not.toBe(firstPid)
      const command = await processCommand(resumedPid)
      expect(command).toContain('claude')
      expect(command).toContain(`--resume ${binding.providerSessionId}`)
    } finally {
      await fixture.close()
    }
  })
})

async function completeTrustPromptIfPresent(surface: Locator): Promise<void> {
  const rows = surface.locator('.xterm-rows')
  const prompt = await expect.poll(async () => {
    const text = await rows.textContent() ?? ''
    if (text.includes('Claude Code v')) return 'ready'
    if (text.includes('Yes, I trust this folder')) return 'trust'
    return 'waiting'
  }, { timeout: 60_000 }).not.toBe('waiting').then(async () => {
    const text = await rows.textContent() ?? ''
    return text.includes('Yes, I trust this folder') ? 'trust' : 'ready'
  })
  if (prompt !== 'trust') return

  let choice = ''
  await expect.poll(async () => {
    const text = await rows.textContent() ?? ''
    choice = text.includes('❯ Yes, I trust this folder')
      ? 'yes' : text.includes('❯ No, exit') ? 'no' : ''
    return choice
  }, { timeout: 30_000 }).not.toBe('')
  const textarea = surface.locator('.xterm-helper-textarea')
  if (choice !== 'yes') await textarea.press('ArrowDown')
  await textarea.press('Enter')
}

async function waitForProviderBinding(dataDirectory: string, sessionId: string): Promise<{
  providerSessionId: string
}> {
  let binding: { providerSessionId: string } | undefined
  await expect.poll(() => {
    const database = new DatabaseSync(join(dataDirectory, 'matou.sqlite'), { readOnly: true })
    try {
      binding = database.prepare(
        `SELECT provider_session_id AS providerSessionId
         FROM provider_bindings WHERE session_id = ? AND invalidated_at IS NULL
         ORDER BY updated_at DESC LIMIT 1`
      ).get(sessionId) as typeof binding
      return binding?.providerSessionId
    } finally {
      database.close()
    }
  }, { timeout: 60_000 }).toMatch(/^[0-9a-f-]{36}$/i)
  if (!binding) throw new Error('Claude provider binding was not recorded')
  return binding
}

async function requiredAttribute(locator: Locator, name: string): Promise<string> {
  const value = await locator.getAttribute(name)
  if (!value) throw new Error(`Expected ${name}`)
  return value
}

async function processCommand(pid: number): Promise<string> {
  const { stdout } = await execFileAsync('/bin/ps', ['-ww', '-o', 'command=', '-p', String(pid)])
  return stdout.trim()
}

function sessionWorkStatus(dataDirectory: string, sessionId: string): string | undefined {
  const database = new DatabaseSync(join(dataDirectory, 'matou.sqlite'), { readOnly: true })
  try {
    return (database.prepare(
      'SELECT work_status AS workStatus FROM sessions WHERE id = ?'
    ).get(sessionId) as { workStatus?: string } | undefined)?.workStatus
  } finally {
    database.close()
  }
}
