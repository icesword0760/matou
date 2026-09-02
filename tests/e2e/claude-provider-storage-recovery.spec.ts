import { execFile } from 'node:child_process'
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'

import { expect, test, type Locator } from '@playwright/test'

import {
  expectVisibleWindowsOnPrimaryDisplay,
  launchMatou,
  primaryAcceptanceDisplayRequested,
  restartMatou,
  type MatouFixture
} from './matou-fixture'

const execFileAsync = promisify(execFile)

test.describe('Claude provider storage recovery', () => {
  test.setTimeout(90_000)

  test('isolates missing/corrupt records, locates the original node, and retries after repair', async () => {
    await assertAcceptanceDisplaysBeforeLaunch()
    let fixture = await launchMatou()
    const databasePath = join(fixture.dataDirectory, 'matou.sqlite')
    const providerExecutable = join(fixture.rootDirectory, 'storage-aware-provider.mjs')
    const projectsRoot = join(fixture.rootDirectory, 'claude-projects')
    const indexPath = join(projectsRoot, 'session-index.json')
    const invocationLog = join(fixture.rootDirectory, 'provider-invocations.jsonl')
    try {
      await assertVisibleWindowsOnlyOnColorLcd(fixture)
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(3)
      await fixture.app.evaluate(({ app }) => app.quit())
      await fixture.app.close().catch(() => undefined)

      const sessions = seedProviderSessions(databasePath)
      await mkdir(projectsRoot, { recursive: true })
      const corruptRecord = join(projectsRoot, 'corrupt-session.jsonl')
      const healthyRecord = join(projectsRoot, 'healthy-session.jsonl')
      await writeFile(corruptRecord, '{not-json\n')
      await writeFile(healthyRecord, JSON.stringify({ sessionId: 'healthy-provider' }) + '\n')
      await writeFile(indexPath, JSON.stringify({
        'corrupt-provider': corruptRecord,
        'healthy-provider': healthyRecord
      }))
      await writeFile(providerExecutable, providerFixtureSource())
      await chmod(providerExecutable, 0o755)

      const environment = {
        MATOU_CLAUDE_COMMAND: providerExecutable,
        MATOU_CLAUDE_PROJECTS_ROOT: projectsRoot,
        MATOU_TEST_PROVIDER_INDEX: indexPath,
        MATOU_TEST_PROVIDER_INVOCATIONS: invocationLog
      }
      fixture = await restartMatou(fixture, { env: environment })
      await assertVisibleWindowsOnlyOnColorLcd(fixture)

      const missingPane = paneByTitle(fixture, '缺失记录会话')
      const corruptPane = paneByTitle(fixture, '损坏记录会话')
      const healthyPane = paneByTitle(fixture, '正常会话')
      await expect(missingPane.locator('.provider-restore-banner')).toContainText('Claude Code 恢复失败')
      await expect(corruptPane.locator('.provider-restore-banner')).toContainText('Claude Code 恢复失败')
      await expect(missingPane.getByRole('button', { name: '重试恢复' })).toBeVisible()
      await expect(missingPane.getByRole('button', { name: '新开 Claude Code' })).toBeVisible()
      await expect(missingPane.locator('.terminal-surface')).toHaveCount(0)
      await expect(corruptPane.locator('.terminal-surface')).toHaveCount(0)
      await expect.poll(() => readProviderState(databasePath, sessions.missing)).toMatchObject({
        kind: 'claude-code', restoreState: 'failed'
      })
      await expect.poll(() => readProviderState(databasePath, sessions.corrupt)).toMatchObject({
        kind: 'claude-code', restoreState: 'failed'
      })

      const healthySurface = healthyPane.locator('.terminal-surface')
      await expect(healthySurface.locator('.xterm-rows')).toContainText('PROVIDER_READY:healthy-provider')
      await typeTerminalInput(healthySurface, 'healthy-input')
      await expect(healthySurface.locator('.xterm-rows'))
        .toContainText('INPUT_OK:healthy-provider')

      await fixture.page.getByRole('button', { name: '通知中心' }).click()
      const recoveryNotifications = fixture.page.getByRole('region', { name: '通知中心' })
        .getByRole('button', { name: /打开通知：provider session not found/ })
      await expect(recoveryNotifications).toHaveCount(2)
      await recoveryNotifications.first().click()
      await expect(fixture.page.locator('[data-testid="terminal-pane"][data-active="true"]'))
        .toContainText('Claude Code 恢复失败')

      await fixture.page.waitForTimeout(300)
      expect(countInvocations(await readFile(invocationLog, 'utf8'), 'missing-provider')).toBe(1)
      fixture = await restartMatou(fixture, { env: environment })
      await assertVisibleWindowsOnlyOnColorLcd(fixture)
      await fixture.page.waitForTimeout(400)
      expect(countInvocations(await readFile(invocationLog, 'utf8'), 'missing-provider')).toBe(1)
      await expect(paneByTitle(fixture, '正常会话').locator('.terminal-surface .xterm-rows'))
        .toContainText('PROVIDER_READY:healthy-provider')

      await writeFile(corruptRecord, JSON.stringify({ sessionId: 'corrupt-provider', repaired: true }) + '\n')
      const repairedPane = paneByTitle(fixture, '损坏记录会话')
      await repairedPane.getByRole('button', { name: '重试恢复' }).click()
      const repairedSurface = repairedPane.locator('.terminal-surface')
      await expect(repairedSurface.locator('.xterm-rows')).toContainText('PROVIDER_READY:corrupt-provider')
      await typeTerminalInput(repairedSurface, 'after-repair')
      await expect(repairedSurface.locator('.xterm-rows'))
        .toContainText('INPUT_OK:corrupt-provider')
      await expect.poll(() => readProviderState(databasePath, sessions.corrupt)).toMatchObject({
        kind: 'claude-code', restoreState: 'none'
      })
      expect(countInvocations(await readFile(invocationLog, 'utf8'), 'corrupt-provider')).toBe(2)
      await expect(paneByTitle(fixture, '缺失记录会话').locator('.provider-restore-banner'))
        .toContainText('Claude Code 恢复失败')

      const freshPane = paneByTitle(fixture, '缺失记录会话')
      await freshPane.getByRole('button', { name: '新开 Claude Code' }).click()
      const freshSurface = freshPane.locator('.terminal-surface')
      await expect(freshSurface.locator('.xterm-rows')).toContainText('PROVIDER_READY:fresh-provider')
      await typeTerminalInput(freshSurface, 'fresh-input')
      await expect(freshSurface.locator('.xterm-rows')).toContainText('INPUT_OK:fresh-provider')
      expect(countInvocations(await readFile(invocationLog, 'utf8'), undefined)).toBe(1)
      await fixture.page.getByRole('button', { name: '通知中心' }).click()
      await expect(fixture.page.getByRole('button', { name: /打开通知：provider session not found/ }))
        .toHaveCount(0)
      await assertVisibleWindowsOnlyOnColorLcd(fixture)
    } finally {
      await fixture.close()
    }
  })
})

function seedProviderSessions(databasePath: string): { missing: string; corrupt: string; healthy: string } {
  const database = new DatabaseSync(databasePath)
  try {
    const sessions = database.prepare(
      'SELECT id FROM sessions WHERE archived_at IS NULL ORDER BY created_at, id LIMIT 3'
    ).all() as Array<{ id: string }>
    if (sessions.length !== 3) throw new Error('Expected three durable Sessions')
    const identities = [
      { key: 'missing', title: '缺失记录会话', provider: 'missing-provider' },
      { key: 'corrupt', title: '损坏记录会话', provider: 'corrupt-provider' },
      { key: 'healthy', title: '正常会话', provider: 'healthy-provider' }
    ] as const
    identities.forEach((identity, index) => {
      const sessionId = sessions[index]!.id
      database.prepare(
        `UPDATE sessions SET kind = 'claude-code', title = ?, status = 'running',
           work_status = 'idle', updated_at = ? WHERE id = ?`
      ).run(identity.title, 100 + index, sessionId)
      database.prepare(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, restore_state,
           metadata_json, created_at, updated_at, validated_at, invalidated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', 'none', ?, ?, ?, ?, NULL)`
      ).run(
        `binding-${identity.key}`, sessionId, identity.provider,
        JSON.stringify({ permissionMode: 'default' }), 100 + index, 100 + index, 100 + index
      )
    })
    return { missing: sessions[0]!.id, corrupt: sessions[1]!.id, healthy: sessions[2]!.id }
  } finally {
    database.close()
  }
}

function readProviderState(databasePath: string, sessionId: string) {
  const database = new DatabaseSync(databasePath, { readOnly: true })
  try {
    const row = database.prepare(
      `SELECT sessions.kind, binding.restore_state AS restoreState
       FROM sessions JOIN provider_bindings AS binding ON binding.session_id = sessions.id
       WHERE sessions.id = ? ORDER BY binding.updated_at DESC LIMIT 1`
    ).get(sessionId) as { kind: string; restoreState: string } | undefined
    return row
  } finally {
    database.close()
  }
}

function paneByTitle(fixture: MatouFixture, title: string): Locator {
  return fixture.page.locator('[data-testid="terminal-pane"]:visible').filter({
    has: fixture.page.locator('.pane-title', { hasText: title })
  })
}

async function typeTerminalInput(surface: Locator, input: string): Promise<void> {
  const pane = surface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
  if (await pane.getAttribute('data-active') !== 'true') await surface.click({ position: { x: 10, y: 10 } })
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(input, { delay: 2 })
  await textarea.press('Enter')
}

function countInvocations(log: string, providerSessionId: string | undefined): number {
  return log.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line) as { resume?: string })
    .filter(({ resume }) => resume === providerSessionId).length
}

function providerFixtureSource(): string {
  return `#!/usr/bin/env node
import { readFileSync, appendFileSync } from 'node:fs'
const args = process.argv.slice(2)
const resumeIndex = args.indexOf('--resume')
const resume = resumeIndex >= 0 ? args[resumeIndex + 1] : undefined
const identity = resume ?? 'fresh-provider'
appendFileSync(process.env.MATOU_TEST_PROVIDER_INVOCATIONS, JSON.stringify({ resume, args }) + '\\n')
let index
try { index = JSON.parse(readFileSync(process.env.MATOU_TEST_PROVIDER_INDEX, 'utf8')) } catch { index = {} }
const record = resume && index[resume]
if (resume && !record) {
  process.stdout.write('No session found for requested id\\n')
  setInterval(() => {}, 1000)
} else {
  try {
    if (!resume) throw new Error('fresh provider has no stored record')
    for (const line of readFileSync(record, 'utf8').trim().split('\\n')) JSON.parse(line)
  } catch {
    if (resume) {
      process.stdout.write('Failed to resume: local session record is corrupt\\n')
      await new Promise(() => {})
    }
  }
  const settingsIndex = args.indexOf('--settings')
  if (settingsIndex >= 0) {
    try {
      const settings = JSON.parse(readFileSync(args[settingsIndex + 1], 'utf8'))
      const url = settings.hooks?.Stop?.[0]?.hooks?.[0]?.url
      if (url) await fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ hook_event_name: 'Stop', session_id: identity, cwd: process.cwd() }) })
    } catch {}
  }
  process.stdout.write('0'.repeat(2050) + '\\nPROVIDER_READY:' + identity + '\\n')
  process.stdin.setEncoding('utf8')
  process.stdin.on('data', chunk => {
    for (const line of chunk.split(/\\r?\\n/).filter(Boolean)) {
      process.stdout.write('INPUT_OK:' + identity + '\\n')
    }
  })
  setInterval(() => {}, 1000)
}
`
}

async function assertAcceptanceDisplaysBeforeLaunch(): Promise<void> {
  if (primaryAcceptanceDisplayRequested()) return
  if (process.platform !== 'darwin') return
  const { stdout } = await execFileAsync('/usr/sbin/system_profiler', ['SPDisplaysDataType', '-json'])
  const report = JSON.parse(stdout) as { SPDisplaysDataType?: Array<{ spdisplays_ndrvs?: Array<Record<string, unknown>> }> }
  const displays = report.SPDisplaysDataType?.flatMap(({ spdisplays_ndrvs }) => spdisplays_ndrvs ?? []) ?? []
  const primary = displays.find((display) => display.spdisplays_main === 'spdisplays_yes')
  const secondary = displays.find((display) => display._name === 'Color LCD' && display.spdisplays_online === 'spdisplays_yes')
  expect(primary?._name).toBe('XV272U')
  expect(secondary?._name).toBe('Color LCD')
}

async function assertVisibleWindowsOnlyOnColorLcd(fixture: MatouFixture): Promise<void> {
  if (primaryAcceptanceDisplayRequested()) {
    await expectVisibleWindowsOnPrimaryDisplay(fixture)
    return
  }
  let placement: { primaryId: number; internalId?: number; visible: Array<{ displayId: number }> } | undefined
  await expect.poll(async () => {
    placement = await fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const internal = screen.getAllDisplays().find((display) => display.internal)
    const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible()).map((window) => ({
      bounds: window.getBounds(), displayId: screen.getDisplayMatching(window.getBounds()).id
    }))
    return { primaryId: primary.id, internalId: internal?.id, visible }
    })
    return placement.visible.length
  }).toBeGreaterThan(0)
  if (!placement) throw new Error('Expected Electron display placement')
  expect(placement.internalId).toBeDefined()
  expect(placement.internalId).not.toBe(placement.primaryId)
  expect(placement.visible.length).toBeGreaterThan(0)
  expect(placement.visible.every(({ displayId }) => displayId === placement.internalId)).toBe(true)
  expect(placement.visible.filter(({ displayId }) => displayId === placement.primaryId)).toHaveLength(0)
}
