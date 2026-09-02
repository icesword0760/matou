import { execFile } from 'node:child_process'
import { rename, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'

import { expect, test, type Locator } from '@playwright/test'

import { readSessionFrames } from '../../apps/runtime/src/journal/segment-journal'
import { launchMatou, type MatouFixture } from './matou-fixture'
import { terminalCommand, visibleSurfaces, waitForShell } from './fixtures/session-canvas-fixture'

const execFileAsync = promisify(execFile)
const CLAUDE = '/Users/icesword/.nvm/versions/node/v22.16.0/bin/claude'

test.describe('real Claude storage-fault resume', () => {
  test.setTimeout(360_000)

  test('resumes the original Claude provider identity after one card loses Journal writes', async () => {
    const startedAt = Date.now()
    const metrics: Record<string, number> = {}
    const { stdout: version } = await execFileAsync(CLAUDE, ['--version'])
    expect(version).toContain('2.1.251 (Claude Code)')
    const { stdout: auth } = await execFileAsync(CLAUDE, ['auth', 'status'])
    expect(JSON.parse(auth)).toMatchObject({ loggedIn: true, authMethod: 'claude.ai' })

    const root = `/tmp/matou-e2e-real-claude-storage-${process.pid}-${Date.now()}`
    const controlPath = join(root, 'journal-fault-control.json')
    const env = {
      MATOU_CLAUDE_COMMAND: CLAUDE,
      MATOU_E2E_JOURNAL_FAULT_CONTROL: controlPath,
      MATOU_E2E_SCALE: '1'
    }
    let fixture: MatouFixture = await launchMatou({ root, env })
    try {
      await expectOnlySecondaryColorLcd(fixture)
      const first = visibleSurfaces(fixture.page).first()
      await waitForShell(first)
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      const second = visibleSurfaces(fixture.page).last()
      await waitForShell(second)

      const claudeSessionId = await requiredAttribute(first, 'data-session-id')
      const shellSessionId = await requiredAttribute(second, 'data-session-id')
      const claudeSurface = sessionSurface(fixture, claudeSessionId)
      const shellSurface = sessionSurface(fixture, shellSessionId)

      await terminalCommand(claudeSurface, 'claude --dangerously-skip-permissions')
      await expect(claudeSurface).toHaveAttribute('data-profile', 'claude-code', { timeout: 60_000 })
      await completeTrustPromptIfPresent(claudeSurface)
      await expect(claudeSurface.locator('.xterm-rows')).toContainText('Claude Code v', {
        timeout: 60_000
      })

      const beforeMarker = `MATOU_REAL_BEFORE_${Date.now()}`
      await completeClaudeTurn(fixture, claudeSurface, claudeSessionId, beforeMarker)
      const binding = await waitForProviderBinding(fixture.dataDirectory, claudeSessionId)
      expect(binding.providerSessionId).toMatch(/^[0-9a-f-]{36}$/i)
      expect(binding.resumeState).toMatch(/^(available|resumed)$/)
      const firstPid = Number(await requiredAttribute(claudeSurface, 'data-pid'))
      const firstCommand = await processCommand(firstPid)
      expect(firstCommand).toContain('claude')
      expect(firstCommand).not.toContain('--resume')

      const beforeFault = await journalSequences(fixture.dataDirectory, claudeSessionId)
      expectContiguousSequences(beforeFault)
      const faultStartedAt = Date.now()
      await setFaultControl(controlPath, { sessionId: claudeSessionId, code: 'ENOSPC' })
      await nudgeMainWindowWidth(fixture)
      const claudePane = paneFor(claudeSurface)
      const fault = claudePane.getByRole('status', { name: /终端记录写入异常/ })
      await expect(fault).toBeVisible({ timeout: 30_000 })
      metrics.faultOverlayMs = Date.now() - faultStartedAt
      await expect(fault).toContainText('其他会话不受影响')
      await expect(paneFor(shellSurface).locator('.storage-fault-overlay')).toHaveCount(0)

      const shellInputStartedAt = Date.now()
      await terminalCommand(shellSurface, 'printf "SHELL_ALIVE_DURING_CLAUDE_FAULT\\n"')
      await expect(shellSurface.locator('.xterm-rows')).toContainText('SHELL_ALIVE_DURING_CLAUDE_FAULT')
      metrics.siblingShellInputMs = Date.now() - shellInputStartedAt
      await expect(fault).toBeVisible()

      await setFaultControl(controlPath, {})
      const retryStartedAt = Date.now()
      await fault.getByRole('button', { name: '重试写入' }).click()
      await expect(fault).toBeHidden({ timeout: 30_000 })
      metrics.durabilityRetryMs = Date.now() - retryStartedAt
      await expect(claudeSurface).toHaveAttribute('data-pid', String(firstPid))
      // The write fault can pause the real provider while the submitted text is
      // still being redrawn, so those keystrokes are intentionally not assumed
      // to have reached Claude. Prove the released original PID accepts a fresh
      // turn before crashing Runtime.
      const recoveredBeforeCrashMarker = `MATOU_RECOVERED_BEFORE_CRASH_${Date.now()}`
      await completeClaudeTurn(
        fixture, claudeSurface, claudeSessionId, recoveredBeforeCrashMarker
      )

      // Keep a real provider turn active while Runtime is killed. Active work
      // is the product's eager-recovery scope; idle cards remain durable and
      // are restored lazily when the user returns to them.
      const interruptedMarker = `MATOU_INTERRUPTED_BY_RUNTIME_${Date.now()}`
      await submitClaudePrompt(claudeSurface, `Reply only with ${interruptedMarker}`)
      await expect.poll(
        () => sessionWorkStatus(fixture.dataDirectory, claudeSessionId),
        { timeout: 30_000, message: 'the real Claude turn must be active before Runtime SIGKILL' }
      ).toBe('running')

      const firstRuntimePid = await runtimePid(fixture)
      await installRecoveryOverlayProbe(fixture, claudeSessionId)
      const runtimeKilledAt = Date.now()
      process.kill(firstRuntimePid, 'SIGKILL')
      await expect.poll(() => processExists(firstRuntimePid), {
        timeout: 10_000,
        message: 'the killed Runtime must exit before reconnect begins'
      }).toBe(false)
      await expect.poll(async () => {
        try {
          const pid = await runtimePid(fixture)
          return { changed: pid !== firstRuntimePid, pid, error: '' }
        } catch (error) {
          return {
            changed: false, pid: firstRuntimePid,
            error: error instanceof Error ? error.message : String(error)
          }
        }
      }, {
        timeout: 30_000,
        message: 'Electron must reconnect to a replacement Runtime'
      }).toMatchObject({ changed: true, error: '' })
      await expect.poll(() => recoveryOverlayWasSeen(fixture), {
        timeout: 30_000,
        message: 'the Claude card must cover itself while the replacement Runtime restores it'
      }).toBe(true)
      metrics.recoveryOverlayMs = Date.now() - runtimeKilledAt
      await expectOnlySecondaryColorLcd(fixture)
      const resumedSurface = sessionSurface(fixture, claudeSessionId)
      await expect(resumedSurface).toHaveAttribute('data-profile', 'claude-code', { timeout: 60_000 })
      await expect(resumedSurface).toHaveAttribute('data-pid', /[1-9][0-9]*/, { timeout: 90_000 })
      await expect(paneFor(resumedSurface).locator('.session-recovery-overlay')).toHaveCount(0, {
        timeout: 90_000
      })
      metrics.providerReadyAfterRuntimeKillMs = Date.now() - runtimeKilledAt
      const resumedPid = Number(await requiredAttribute(resumedSurface, 'data-pid'))
      expect(resumedPid).not.toBe(firstPid)
      const afterMarker = `MATOU_REAL_AFTER_${Date.now()}`
      await completeClaudeTurn(fixture, resumedSurface, claudeSessionId, afterMarker)
      const resumedCommand = await processCommand(resumedPid)
      expect(resumedCommand).toContain(CLAUDE)
      expect(resumedCommand).toContain(`--resume ${binding.providerSessionId}`)

      const resumedBinding = await waitForProviderBinding(
        fixture.dataDirectory, claudeSessionId, binding.updatedAt
      )
      expect(resumedBinding.providerSessionId).toBe(binding.providerSessionId)

      const finalSequences = await journalSequences(fixture.dataDirectory, claudeSessionId)
      expect(finalSequences.length).toBeGreaterThan(beforeFault.length)
      expectContiguousSequences(finalSequences)
      expect(new Set(finalSequences).size).toBe(finalSequences.length)
      await expectOnlySecondaryColorLcd(fixture)
      metrics.totalMs = Date.now() - startedAt
      console.info('[real-claude-storage-resume]', JSON.stringify({
        ...metrics,
        providerSessionId: binding.providerSessionId,
        journalSequencesBeforeFault: beforeFault.length,
        journalSequencesAfterResume: finalSequences.length,
        displays: { visible: 1, colorLcd: 1, primary: 0, xv272u: 0 }
      }))
    } finally {
      await fixture.close()
    }
  })
})

function sessionSurface(fixture: MatouFixture, sessionId: string): Locator {
  return fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
}

async function nudgeMainWindowWidth(fixture: MatouFixture): Promise<void> {
  await fixture.app.evaluate(({ BrowserWindow }) => {
    const window = BrowserWindow.getAllWindows().find((candidate) => candidate.isVisible())
    if (!window) throw new Error('visible Matou window is missing')
    const bounds = window.getBounds()
    window.setBounds({ ...bounds, width: bounds.width - 12 })
  })
}

function paneFor(surface: Locator): Locator {
  return surface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
}

async function submitClaudePrompt(surface: Locator, prompt: string): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  await surface.click({ position: { x: 12, y: 12 } })
  await textarea.focus()
  await expect(textarea).toBeFocused()
  await textarea.pressSequentially(prompt, { delay: 2 })
  await textarea.press('Enter')
}

async function completeClaudeTurn(
  fixture: MatouFixture,
  surface: Locator,
  sessionId: string,
  marker: string
): Promise<void> {
  const splitAt = Math.max(1, Math.floor(marker.length / 2))
  await submitClaudePrompt(
    surface,
    `Reply only with the concatenation of "${marker.slice(0, splitAt)}" and "${marker.slice(splitAt)}"`
  )
  await expect.poll(
    () => sessionWorkStatus(fixture.dataDirectory, sessionId),
    { timeout: 30_000, message: 'the real Claude turn must enter running state' }
  ).toBe('running')
  await expect.poll(
    () => sessionWorkStatus(fixture.dataDirectory, sessionId),
    { timeout: 120_000, message: 'the real Claude turn must return to its input prompt' }
  ).toBe('idle')
  await expect.poll(
    async () => normalizeJournalSearch(
      await journalOutputText(fixture.dataDirectory, sessionId)
    ),
    {
      timeout: 30_000,
      message: 'the real Claude answer must be durably recorded in the Session Journal'
    }
  ).toContain(marker)
}

async function completeTrustPromptIfPresent(surface: Locator): Promise<void> {
  const rows = surface.locator('.xterm-rows')
  const trustPrompt = await expect.poll(async () => {
    const text = await rows.textContent() ?? ''
    if (text.includes('Claude Code v')) return 'ready'
    if (text.includes('Yes, I trust this folder')) return 'trust'
    return 'waiting'
  }, { timeout: 60_000, intervals: [100, 250, 500] }).not.toBe('waiting').then(async () => {
    const text = await rows.textContent() ?? ''
    return text.includes('Yes, I trust this folder') ? 'trust' : 'ready'
  })
  if (trustPrompt !== 'trust') return

  let settledChoice = ''
  let stableFrames = 0
  await expect.poll(async () => {
    const text = await rows.textContent() ?? ''
    const choice = text.includes('❯ Yes, I trust this folder')
      ? 'yes' : text.includes('❯ No, exit') ? 'no' : ''
    stableFrames = choice && choice === settledChoice ? stableFrames + 1 : choice ? 1 : 0
    settledChoice = choice
    return stableFrames
  }, { timeout: 30_000, intervals: [100, 100, 100] }).toBeGreaterThanOrEqual(2)
  const textarea = surface.locator('.xterm-helper-textarea')
  if (settledChoice !== 'yes') await textarea.press('ArrowDown')
  await textarea.press('Enter')
}

async function waitForProviderBinding(
  dataDirectory: string,
  sessionId: string,
  updatedAfter?: number
): Promise<{
  providerSessionId: string
  resumeState: string
  updatedAt: number
}> {
  let value: { providerSessionId: string; resumeState: string; updatedAt: number } | undefined
  await expect.poll(() => {
    const database = new DatabaseSync(join(dataDirectory, 'matou.sqlite'), { readOnly: true })
    try {
      value = database.prepare(
        `SELECT provider_session_id AS providerSessionId, resume_state AS resumeState,
                updated_at AS updatedAt
         FROM provider_bindings WHERE session_id = ? AND invalidated_at IS NULL
         ORDER BY updated_at DESC LIMIT 1`
      ).get(sessionId) as typeof value
      if (updatedAfter !== undefined && (value?.updatedAt ?? 0) <= updatedAfter) return undefined
      return value?.providerSessionId
    } finally {
      database.close()
    }
  }, { timeout: 60_000 }).toMatch(/^[0-9a-f-]{36}$/i)
  if (!value) throw new Error('Claude provider binding was not recorded')
  return value
}

async function processCommand(pid: number): Promise<string> {
  const { stdout } = await execFileAsync('/bin/ps', ['-ww', '-o', 'command=', '-p', String(pid)])
  return stdout.trim()
}

async function runtimePid(fixture: MatouFixture): Promise<number> {
  const metrics = await fixture.app.evaluate(async () => {
    const read = (globalThis as typeof globalThis & {
      __matouE2eScaleMetrics?: () => Promise<{ runtimePid: number }>
    }).__matouE2eScaleMetrics
    if (!read) throw new Error('Runtime metrics bridge is unavailable')
    return read()
  })
  return metrics.runtimePid
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

async function installRecoveryOverlayProbe(
  fixture: MatouFixture,
  sessionId: string
): Promise<void> {
  await fixture.page.evaluate((targetSessionId) => {
    const pane = document.querySelector(
      `.terminal-surface[data-session-id="${targetSessionId}"]`
    )?.closest('[data-testid="terminal-pane"]')
    if (!pane) throw new Error('the Claude terminal pane is missing before Runtime crash')
    const state = { seen: false }
    const scan = () => {
      if (pane.querySelector('.session-recovery-overlay')) state.seen = true
    }
    new MutationObserver(scan).observe(document.body, {
      subtree: true, childList: true, attributes: true, attributeFilter: ['aria-busy']
    })
    ;(window as typeof window & { __matouRealClaudeRecovery?: typeof state })
      .__matouRealClaudeRecovery = state
    scan()
  }, sessionId)
}

async function recoveryOverlayWasSeen(fixture: MatouFixture): Promise<boolean> {
  return fixture.page.evaluate(() => (
    window as typeof window & { __matouRealClaudeRecovery?: { seen: boolean } }
  ).__matouRealClaudeRecovery?.seen === true)
}

async function journalSequences(dataDirectory: string, sessionId: string): Promise<number[]> {
  return (await readSessionFrames(dataDirectory, sessionId)).map(({ sequence }) => sequence)
}

async function journalOutputText(dataDirectory: string, sessionId: string): Promise<string> {
  const frames = await readSessionFrames(dataDirectory, sessionId)
  const decoder = new TextDecoder()
  return frames.flatMap((frame) => frame.kind === 'output' ? [decoder.decode(frame.data)] : []).join('')
}

function normalizeJournalSearch(value: string): string {
  return value
    .replace(/\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g, '')
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/[^A-Za-z0-9_]/g, '')
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

function expectContiguousSequences(sequences: number[]): void {
  expect(sequences.length).toBeGreaterThan(0)
  for (let index = 1; index < sequences.length; index += 1) {
    expect(sequences[index]).toBe(sequences[index - 1]! + 1)
  }
}

async function setFaultControl(path: string, value: unknown): Promise<void> {
  const temporary = `${path}.next`
  await writeFile(temporary, JSON.stringify(value))
  await rename(temporary, path)
}

async function requiredAttribute(locator: Locator, name: string): Promise<string> {
  const value = await locator.getAttribute(name)
  if (!value) throw new Error(`${name} is missing`)
  return value
}

async function expectOnlySecondaryColorLcd(fixture: MatouFixture): Promise<void> {
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
    const displays = visible.map((window) => screen.getDisplayMatching(window.getBounds()))
    return {
      visible: visible.length,
      colorLcd: displays.filter(({ internal }) => internal).length,
      primary: displays.filter(({ id }) => id === primary.id).length,
      xv272u: displays.filter(({ label }) => /xv272u/i.test(label)).length
    }
  })).toEqual({ visible: 1, colorLcd: 1, primary: 0, xv272u: 0 })
}
