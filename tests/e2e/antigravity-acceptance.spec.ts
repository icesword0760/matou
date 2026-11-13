import { execFile } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { chmod, mkdir, mkdtemp, readFile, stat, symlink, unlink, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'

import { expect, test, _electron as electron, type ElectronApplication, type Locator, type Page } from '@playwright/test'

const execFileAsync = promisify(execFile)

const SCRATCHPAD_ROOT =
  '/private/tmp/claude-501/-Users-icesword-Documents-AIProjects-matou/da0ac5f4-7b9e-4679-81d7-890907cb84cf/scratchpad/agy-e2e'
const PACKAGED_EXECUTABLE = resolve(
  import.meta.dirname,
  '../../apps/desktop/release/mac-arm64/码头.app/Contents/MacOS/码头'
)
const EVIDENCE_DIR = resolve(import.meta.dirname, '../../docs/acceptance/evidence/antigravity')
const RESULTS_DIR = join(EVIDENCE_DIR, 'results')
const GEMINI_CONVERSATIONS_ROOT = join(process.env.HOME ?? '/Users/icesword', '.gemini/antigravity-cli/conversations')
const GEMINI_BRAIN_ROOT = join(process.env.HOME ?? '/Users/icesword', '.gemini/antigravity-cli/brain')

interface PackagedHarness {
  rootDir: string
  repoDir: string
  zdotDir: string
  dataDir: string
  realDataDir: string
  userDataDir: string
  app: ElectronApplication
  page: Page
  launchEnv: Record<string, string>
  close(): Promise<void>
}

interface LatencyRecord {
  transition: string
  triggerAtMs: number
  runtimeEventAtMs?: number
  uiObservedAtMs: number
  runtimeToUiMs?: number
  endToEndMs: number
  within1000ms?: boolean
}

function assertIsolatedPath(pathValue: string): void {
  if (!pathValue.startsWith(`${SCRATCHPAD_ROOT}/`)) {
    throw new Error(`Refusing non-isolated E2E path outside scratchpad: ${pathValue}`)
  }
}

async function createIsolatedWorkspace(prefix: string, options?: { secondCommit?: boolean }): Promise<{
  rootDir: string
  repoDir: string
  zdotDir: string
  dataDir: string
  realDataDir: string
  userDataDir: string
}> {
  await mkdir(SCRATCHPAD_ROOT, { recursive: true })
  const rootDir = await mkdtemp(join(SCRATCHPAD_ROOT, `${prefix}-`))
  const repoDir = join(rootDir, 'repo')
  const zdotDir = join(rootDir, 'zdot')
  const realDataDir = join(rootDir, 'matou-data')
  const userDataDir = join(rootDir, 'electron-user')
  // macOS sockaddr_un.sun_path is capped at 104 bytes; bind control/runtime.sock
  // via a short /tmp symlink pointing into the isolated scratchpad data dir.
  const dataDir = `/tmp/m-e2e-${randomBytes(4).toString('hex')}`
  assertIsolatedPath(repoDir)
  assertIsolatedPath(zdotDir)
  assertIsolatedPath(realDataDir)
  assertIsolatedPath(userDataDir)
  await mkdir(repoDir, { recursive: true })
  await mkdir(zdotDir, { recursive: true })
  await writeFile(join(zdotDir, '.zshrc'), '')
  await mkdir(realDataDir, { recursive: true })
  await mkdir(userDataDir, { recursive: true })
  await symlink(realDataDir, dataDir)

  await writeFile(join(repoDir, 'README.md'), '# E2E Sandbox Repo\n\nLine 2 for safe read test.\nLine 3.\n')
  await writeFile(join(repoDir, 'AGENTS.md'), '# Project Instructions\nAlways keep actions strictly inside this directory.\n')
  await execFileAsync('git', ['init', '-b', 'main'], { cwd: repoDir })
  await execFileAsync('git', ['config', 'user.email', 'e2e@matou.invalid'], { cwd: repoDir })
  await execFileAsync('git', ['config', 'user.name', 'Matou E2E'], { cwd: repoDir })
  await execFileAsync('git', ['add', 'README.md', 'AGENTS.md'], { cwd: repoDir })
  await execFileAsync('git', ['commit', '-m', 'Initial commit'], { cwd: repoDir })
  if (options?.secondCommit) {
    await writeFile(join(repoDir, 'notes.txt'), 'Second commit marker\n')
    await execFileAsync('git', ['add', 'notes.txt'], { cwd: repoDir })
    await execFileAsync('git', ['commit', '-m', 'Second commit'], { cwd: repoDir })
  }
  return { rootDir, repoDir, zdotDir, dataDir, realDataDir, userDataDir }
}

async function launchPackagedApp(dirs: {
  rootDir: string
  repoDir: string
  zdotDir: string
  dataDir: string
  realDataDir: string
  userDataDir: string
}, extraEnv?: Record<string, string>): Promise<PackagedHarness> {
  assertIsolatedPath(dirs.realDataDir)
  assertIsolatedPath(dirs.userDataDir)
  assertIsolatedPath(dirs.repoDir)
  assertIsolatedPath(dirs.zdotDir)
  const basePath = process.env.PATH ?? '/usr/bin:/bin:/usr/sbin:/sbin'
  const env: Record<string, string> = {
    ...(Object.fromEntries(
      Object.entries(process.env).filter(([, value]) => typeof value === 'string')
    ) as Record<string, string>),
    PATH: `/Users/icesword/.local/bin:${basePath}`,
    ZDOTDIR: dirs.zdotDir,
    MATOU_LOCALE: 'zh-CN',
    MATOU_E2E: '1',
    MATOU_E2E_DISPLAY: process.env.MATOU_E2E_DISPLAY ?? 'primary',
    MATOU_E2E_WINDOW_CLOSE: 'hide',
    MATOU_DATA_DIR: dirs.dataDir,
    MATOU_DEFAULT_WORKSPACE: dirs.repoDir,
    ELECTRON_USER_DATA_DIR: dirs.userDataDir,
    ...extraEnv
  }
  delete env.ELECTRON_RUN_AS_NODE

  const app = await electron.launch({
    executablePath: PACKAGED_EXECUTABLE,
    args: [`--user-data-dir=${dirs.userDataDir}`],
    env
  })
  const page = await app.firstWindow()
  await page.waitForLoadState('domcontentloaded')
  await page.locator('[data-testid="terminal-pane"]:visible').first().waitFor({ timeout: 30_000 })
  return {
    ...dirs,
    app,
    page,
    launchEnv: env,
    async close() {
      await app.close()
    }
  }
}

async function cleanupWorkspaceLinks(dirs: { dataDir: string }): Promise<void> {
  await unlink(dirs.dataDir).catch(() => {})
}

async function saveProof(name: string, payload: unknown): Promise<void> {
  await mkdir(RESULTS_DIR, { recursive: true })
  await writeFile(join(RESULTS_DIR, name), `${JSON.stringify(payload, null, 2)}\n`)
}

async function saveScreenshot(
  target: Page | Locator,
  fileName: string,
  options?: { settleSelector?: string; settleMs?: number }
): Promise<string> {
  await mkdir(EVIDENCE_DIR, { recursive: true })
  const outPath = join(EVIDENCE_DIR, fileName)
  const page = 'page' in target && typeof target.page === 'function' ? target.page() : (target as Page)
  const settleMs = options?.settleMs ?? 450
  if (options?.settleSelector) {
    const settleTarget = page.locator(options.settleSelector).first()
    await expect(settleTarget).toBeVisible({ timeout: 5_000 })
    await expect.poll(
      async () => settleTarget.evaluate((el) => getComputedStyle(el).opacity),
      { timeout: 5_000 }
    ).toBe('1')
  }
  await page.evaluate(() => new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  }))
  await page.waitForTimeout(settleMs)
  await target.screenshot({ path: outPath })
  return outPath
}

function hudForSession(page: Page, sessionId: string): Locator {
  return page.locator(`.shortcut-bar .status-info[data-session-id="${sessionId}"]`)
}

function visibleFocusedSurface(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) .session-card.is-focused [data-testid="terminal-pane"]:visible .terminal-surface').first()
}

function paneForSession(page: Page, sessionId: string): Locator {
  return page.locator(`[data-session-card="${sessionId}"] [data-testid="terminal-pane"]:visible`)
}

async function positivePid(surface: Locator): Promise<number> {
  let pid = 0
  await expect.poll(async () => {
    const raw = await surface.getAttribute('data-pid')
    pid = Number(raw ?? '0')
    return pid
  }, { timeout: 25_000 }).toBeGreaterThan(0)
  return pid
}

async function sendTerminalLine(surface: Locator, line: string): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await expect(textarea).toBeFocused()
  await textarea.pressSequentially(line, { delay: 10 })
  await textarea.press('Enter')
}

async function sendTerminalRawKey(surface: Locator, key: string): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await expect(textarea).toBeFocused()
  await textarea.press(key)
}

async function openPaneMenu(pane: Locator): Promise<void> {
  await pane.locator('.terminal-pane-header').click({
    button: 'right',
    position: { x: 72, y: 20 },
    force: true
  })
}

async function confirmForkDialog(page: Page, name: string, worktreeMode: 'current' | 'new'): Promise<void> {
  await page.getByLabel('分支名称').fill(name)
  const radios = page.locator('.branch-worktree-card input[type="radio"]')
  if (worktreeMode === 'new') {
    await radios.nth(1).check()
  } else {
    await expect(radios.nth(0)).toBeChecked()
  }
  await page.getByRole('button', { name: '创建分支', exact: true }).click()
}

function readDbRows(dataDir: string, sessionId?: string): {
  bindings: Array<{
    session_id: string
    provider: string
    provider_session_id: string
    resume_state: string
    metadata_json: string
    updated_at: number
  }>
  events: Array<{
    seq: number
    event_type: string
    session_id: string | null
    payload_json: string
    occurred_at: number
  }>
  sessions: Array<{
    id: string
    title: string
    kind: string
  }>
} {
  const db = new DatabaseSync(join(dataDir, 'matou.sqlite'), { readOnly: true })
  try {
    const bindings = (sessionId
      ? db.prepare('SELECT session_id, provider, provider_session_id, resume_state, metadata_json, updated_at FROM provider_bindings WHERE session_id = ? ORDER BY updated_at DESC, rowid DESC').all(sessionId)
      : db.prepare('SELECT session_id, provider, provider_session_id, resume_state, metadata_json, updated_at FROM provider_bindings ORDER BY updated_at DESC, rowid DESC').all()) as Array<{
        session_id: string
        provider: string
        provider_session_id: string
        resume_state: string
        metadata_json: string
        updated_at: number
      }>
    const events = (sessionId
      ? db.prepare('SELECT seq, event_type, session_id, payload_json, occurred_at FROM domain_events WHERE session_id = ? ORDER BY seq ASC').all(sessionId)
      : db.prepare('SELECT seq, event_type, session_id, payload_json, occurred_at FROM domain_events ORDER BY seq DESC LIMIT 80').all()) as Array<{
        seq: number
        event_type: string
        session_id: string | null
        payload_json: string
        occurred_at: number
      }>
    const sessions = db.prepare('SELECT id, title, kind FROM sessions WHERE archived_at IS NULL').all() as Array<{
      id: string
      title: string
      kind: string
    }>
    return { bindings, events, sessions }
  } finally {
    db.close()
  }
}

function readLatestWorkStatus(dataDir: string, sessionId: string): { status: string; occurredAt: number; seq: number } | null {
  const { events } = readDbRows(dataDir, sessionId)
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const ev = events[i]!
    if (ev.event_type === 'session.work_status_changed' || ev.event_type === 'session.work-status-changed') {
      try {
        const parsed = JSON.parse(ev.payload_json) as { workStatus?: string; status?: string }
        const st = parsed.workStatus ?? parsed.status
        if (st) return { status: st, occurredAt: ev.occurred_at, seq: ev.seq }
      } catch { /* ignore */ }
    }
    try {
      const parsed = JSON.parse(ev.payload_json) as { workStatus?: string }
      if (parsed.workStatus) return { status: parsed.workStatus, occurredAt: ev.occurred_at, seq: ev.seq }
    } catch { /* ignore */ }
  }
  return null
}

async function openDagWindow(harness: PackagedHarness): Promise<Page> {
  const existing = harness.app.windows().find((win) => win !== harness.page)
  if (existing && !existing.isClosed()) return existing
  const [dagPage] = await Promise.all([
    harness.app.waitForEvent('window'),
    harness.page.getByRole('button', { name: '打开会话 DAG' }).click()
  ])
  await dagPage.waitForLoadState('domcontentloaded')
  await dagPage.locator('.dag-node-card').first().waitFor({ timeout: 20_000 })
  return dagPage
}

async function waitForDagNodeStatus(
  dagPage: Page,
  sessionId: string,
  expectedClass: 'status-starting' | 'status-idle' | 'status-running' | 'status-needs-input' | 'status-error',
  timeout = 45_000
): Promise<number> {
  const card = dagPage.locator(`.dag-node-card[data-session-id="${sessionId}"].${expectedClass}`)
  await expect(card).toHaveCount(1, { timeout })
  return Date.now()
}

async function installDagStatusObserver(dagPage: Page, sessionId: string): Promise<void> {
  await dagPage.evaluate((sid) => {
    const store = ((window as unknown as { __matouStatusLog?: Array<{ status: string; at: number }> }).__matouStatusLog = [])
    const recordCurrent = () => {
      const el = document.querySelector(`.dag-node-card[data-session-id="${sid}"]`)
      if (!el) return
      const cls = [...el.classList].find((c) => c.startsWith('status-')) ?? ''
      if (cls && store.at(-1)?.status !== cls) {
        store.push({ status: cls, at: Date.now() })
      }
    }
    recordCurrent()
    const observer = new MutationObserver(() => recordCurrent())
    observer.observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['class']
    })
  }, sessionId)
}

async function readDagStatusObserver(dagPage: Page): Promise<Array<{ status: string; at: number }>> {
  return dagPage.evaluate(() => (window as unknown as { __matouStatusLog?: Array<{ status: string; at: number }> }).__matouStatusLog ?? [])
}

async function waitForEmptyShellPrompt(surface: Locator): Promise<void> {
  await expect(surface.locator('.xterm-rows')).toContainText(/\brepo\s+[%#$]/, { timeout: 30_000 })
  const promptRows = await surface.locator('.xterm-rows').innerText()
  const afterPrompt = promptRows.split(/\brepo\s+[%#$]/).at(-1)?.trim() ?? ''
  expect(afterPrompt, `promptRows=${JSON.stringify(promptRows)}`).toBe('')
}

async function launchAgyInFreshRepo(harness: PackagedHarness): Promise<{
  sessionId: string
  surface: Locator
  dagPage: Page
}> {
  const surface = visibleFocusedSurface(harness.page)
  const sessionId = (await surface.getAttribute('data-session-id')) ?? ''
  expect(sessionId).toBeTruthy()
  await positivePid(surface)
  await waitForEmptyShellPrompt(surface)

  const dagPage = await openDagWindow(harness)
  await installDagStatusObserver(dagPage, sessionId)
  await harness.page.bringToFront()

  await sendTerminalLine(surface, 'agy')

  // Real agy prompts "Do you trust the contents of this project?" in a new directory (#3)
  await expect(surface.locator('.xterm-rows')).toContainText(/Do you trust|Trust folder|Antigravity/i, { timeout: 45_000 })
  const dagCard = dagPage.locator(`.dag-node-card[data-session-id="${sessionId}"]`)
  await expect(dagCard).toContainText('Antigravity', { timeout: 20_000 })

  // Check if workspace trust prompt appeared -> needs-input -> confirm
  const rowsText = await surface.locator('.xterm-rows').innerText()
  if (/Do you trust/i.test(rowsText)) {
    await waitForDagNodeStatus(dagPage, sessionId, 'status-needs-input', 20_000)
    await saveScreenshot(dagPage, '03-first-dir-trust-dag.png')
    await saveScreenshot(harness.page, '03-first-dir-trust.png')
    await harness.page.bringToFront()
    await harness.page.waitForTimeout(300)
    await sendTerminalRawKey(surface, 'Enter')
    await harness.page.waitForTimeout(2_000)
    const afterEnterRows = await surface.locator('.xterm-rows').innerText().catch(() => '')
    if (/Do you trust/i.test(afterEnterRows)) {
      await sendTerminalRawKey(surface, 'Enter')
    }
  }

  // Real agy sets model != null in statusLine once past the trust screen and ready for turn 1;
  // session_id is created lazily when turn 1 is submitted.
  const hud = hudForSession(harness.page, sessionId)
  await expect(hud).toHaveAttribute('data-hud-mode', 'agent', { timeout: 45_000 })
  await expect(hud.locator('.status-model')).toBeVisible({ timeout: 45_000 })
  await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 90_000)

  return { sessionId, surface, dagPage }
}

async function waitForProviderSessionId(dataDir: string, sessionId: string, timeout = 30_000): Promise<string> {
  let providerSessionId = ''
  await expect.poll(() => {
    const { bindings } = readDbRows(dataDir, sessionId)
    const binding = bindings.find((b) => b.provider === 'antigravity')
    providerSessionId = binding?.provider_session_id ?? ''
    return providerSessionId
  }, { timeout }).toBeTruthy()
  return providerSessionId
}

async function createSiblingShellAndFocus(page: Page, previousSessionId: string): Promise<{ sessionId: string; surface: Locator }> {
  await page.getByRole('button', { name: /横向新增 Shell|新增.*Shell|添加 Shell/ }).click()
  let nextId = ''
  await expect.poll(async () => {
    nextId = (await visibleFocusedSurface(page).getAttribute('data-session-id')) ?? ''
    return nextId && nextId !== previousSessionId ? nextId : ''
  }, { timeout: 15_000 }).toBeTruthy()
  const surface = visibleFocusedSurface(page)
  await positivePid(surface)
  await waitForEmptyShellPrompt(surface)
  return { sessionId: nextId, surface }
}

test.describe('Antigravity Packaged App Acceptance (Real agy + Controlled Error Fixtures)', () => {
  test.setTimeout(360_000)

  test('PRD #1 #3 #4 #5 #6 #11 #12: Real agy lifecycle, HUD, notifications, modes, and /model in packaged 码头.app', async () => {
    const dirs = await createIsolatedWorkspace('core')
    const harness = await launchPackagedApp(dirs)
    const latencies: LatencyRecord[] = []

    try {
      const { sessionId, surface, dagPage } = await launchAgyInFreshRepo(harness)
      const initialPid = await positivePid(surface)
      const hud = hudForSession(harness.page, sessionId)

      // #1 & #6: HUD verification
      await expect(hud).toHaveAttribute('data-hud-mode', 'agent')
      await expect(hud.locator('.status-perm-badge')).toBeVisible()
      await expect(hud.locator('.status-model')).toBeVisible({ timeout: 20_000 })
      await expect(hud.locator('.status-git')).toContainText('main')
      await expect(hud.locator('.status-cwd')).toContainText('repo')
      // Instruction file AGENTS.md exists in repo
      await expect(hud.locator('.status-instructions')).toBeVisible({ timeout: 15_000 })
      // No Claude Account button, no todos, no empty quota placeholder '—' when no usageWindows
      await expect(hud.locator('.claude-accounts-control, [aria-label*="Claude 账户"]')).toHaveCount(0)
      await expect(hud.locator('.status-todos')).toHaveCount(0)

      // Make repo dirty to verify git dirty '*' indicator
      await writeFile(join(dirs.repoDir, 'dirty-check.txt'), 'untracked change\n')

      // #6 HUD shows only active model's quota group ('5h'/'Weekly', no Gemini/3P prefix) and full details on hover
      await expect(hud.locator('.status-usage').first()).toBeVisible({ timeout: 20_000 })
      const usageTexts = await hud.locator('.status-usage').allInnerTexts()
      expect(usageTexts.length).toBeGreaterThanOrEqual(1)
      expect(usageTexts.some((t) => /^(?:5h|Weekly)\s/.test(t.trim()))).toBe(true)
      expect(usageTexts.join(' | ')).not.toMatch(/Gemini|3P/)

      await saveScreenshot(harness.page, '01-promote-shell.png')
      await saveScreenshot(hud, '06-hud.png')

      await hud.locator('.status-usage').first().hover()
      const quotaTooltip = harness.page.getByRole('tooltip')
      await expect(quotaTooltip).toBeVisible({ timeout: 5_000 })
      await expect(quotaTooltip).toContainText('全部用量额度')
      await expect(quotaTooltip).toContainText('Gemini 5h')
      await expect(quotaTooltip).toContainText('3P')
      await saveScreenshot(harness.page, '06-hud-tooltip.png', { settleSelector: '.hud-detail-tooltip', settleMs: 450 })
      await surface.hover()
      await expect(quotaTooltip).toHaveCount(0, { timeout: 5_000 })

      // #12: HUD model is read-only (no model switch menu)
      const modelBadge = hud.locator('.status-model')
      const modelBefore = await modelBadge.innerText()
      await modelBadge.click()
      await expect(harness.page.locator('.model-switch-menu, [role="menu"][aria-label*="模型"]')).toHaveCount(0)

      // #4 + Latency: Send a harmless prompt & measure running and idle transitions
      const turn1TriggerMs = Date.now()
      await sendTerminalLine(surface, 'Reply with ONLY the token MATOU_READY_1 and nothing else.')
      const runningUiAt = await waitForDagNodeStatus(dagPage, sessionId, 'status-running', 30_000)
      await saveScreenshot(dagPage, '04-work-status-running-dag.png')
      const idleUiAt = await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 90_000)
      await expect(surface.locator('.xterm-rows')).toContainText('MATOU_READY_1', { timeout: 30_000 })
      const providerSessionId = await waitForProviderSessionId(harness.dataDir, sessionId)
      await saveScreenshot(dagPage, '04-work-status-idle-dag.png')

      // #6 check dirty '*' updated cleanly on HUD after turn
      await expect(hud.locator('.status-git')).toContainText('*', { timeout: 15_000 })

      // Check runtime event timestamps vs DAG observer timestamps
      const observerLog = await readDagStatusObserver(dagPage)
      const dbSnapshotAfterTurn = readDbRows(harness.dataDir, sessionId)
      const runningEv = dbSnapshotAfterTurn.events.filter((e) => e.payload_json.includes('"running"')).at(-1)
      const idleEv = dbSnapshotAfterTurn.events.filter((e) => e.payload_json.includes('"idle"')).at(-1)
      const runningObs = observerLog.filter((o) => o.status === 'status-running').at(-1)
      const idleObs = observerLog.filter((o) => o.status === 'status-idle').at(-1)

      if (runningObs) {
        const rtToUi = runningEv ? Math.max(0, runningObs.at - runningEv.occurred_at) : undefined
        latencies.push({
          transition: 'idle -> running',
          triggerAtMs: turn1TriggerMs,
          ...(runningEv ? { runtimeEventAtMs: runningEv.occurred_at } : {}),
          uiObservedAtMs: runningObs.at,
          ...(rtToUi !== undefined ? { runtimeToUiMs: rtToUi, within1000ms: rtToUi <= 1000 } : {}),
          endToEndMs: runningObs.at - turn1TriggerMs
        })
      }
      if (idleObs) {
        const rtToUi = idleEv ? Math.max(0, idleObs.at - idleEv.occurred_at) : undefined
        latencies.push({
          transition: 'running -> idle (Stop)',
          triggerAtMs: turn1TriggerMs,
          ...(idleEv ? { runtimeEventAtMs: idleEv.occurred_at } : {}),
          uiObservedAtMs: idleObs.at,
          ...(rtToUi !== undefined ? { runtimeToUiMs: rtToUi, within1000ms: rtToUi <= 1000 } : {}),
          endToEndMs: idleObs.at - turn1TriggerMs
        })
      }

      // #11 Execution mode switching: 4 menu items, first 3 don't restart (data-pid unchanged)
      const permBadge = hud.locator('.status-perm-badge')
      await permBadge.click()
      const permMenu = harness.page.getByRole('menu', { name: '权限模式' })
      await expect(permMenu).toBeVisible()
      const menuItems = permMenu.locator('[role="menuitem"]')
      await expect(menuItems).toHaveCount(4)
      await expect(permMenu).toContainText('Default')
      await expect(permMenu).toContainText('Accept Edits')
      await expect(permMenu).toContainText('Plan Mode')
      await expect(permMenu).toContainText('Bypass Permissions')
      await saveScreenshot(harness.page, '11-execution-modes-menu.png', { settleSelector: '.perm-menu', settleMs: 450 })

      await permMenu.getByRole('menuitem', { name: 'Accept Edits' }).click()
      await expect(permBadge).toHaveText('AE', { timeout: 15_000 })
      await expect(surface.locator('.xterm-rows')).toContainText(/accept[- ]edits|accept edits/i, { timeout: 15_000 })
      expect(await positivePid(surface)).toBe(initialPid)

      await permBadge.click()
      await harness.page.getByRole('menu', { name: '权限模式' }).getByRole('menuitem', { name: 'Plan Mode' }).click()
      await expect(permBadge).toHaveText('PM', { timeout: 15_000 })
      await expect(surface.locator('.xterm-rows')).toContainText(/Plan mode|\bplan\s*·/i, { timeout: 15_000 })
      await harness.page.waitForTimeout(2_500)
      expect(await positivePid(surface)).toBe(initialPid)

      // Shift+Tab once from Plan Mode moves real agy into Ask Mode (cycle_mode omitted/null),
      // so HUD badge shows AK ('逐项确认') and running a shell tool triggers tool_use approval + non-focused notification (#5).
      await sendTerminalRawKey(surface, 'Shift+Tab')
      await expect(permBadge).toHaveText('AK', { timeout: 15_000 })
      await expect(permBadge).toHaveAttribute('title', /逐项确认/)
      await expect(surface.locator('.xterm-rows')).not.toContainText(/\bplan\s*·/i, { timeout: 10_000 })

      // Open a second card and focus it so the Antigravity card becomes non-focused for notification test
      const { sessionId: shell2Id } = await createSiblingShellAndFocus(harness.page, sessionId)

      // Trigger non-focused notification by sending a shell command in Ask mode to agy, then switching focus to shell2
      const agyCard = harness.page.locator(`[data-session-card="${sessionId}"]`)
      await agyCard.click()
      await expect(visibleFocusedSurface(harness.page)).toHaveAttribute('data-session-id', sessionId)
      const notifyTriggerMs = Date.now()
      await sendTerminalLine(surface, 'Run the shell command `python3 -c "print(\'TOOL_OK_42\')"` and report the output.')
      await harness.page.locator(`[data-session-card="${shell2Id}"]`).click()

      // Wait for notification badge / bell on non-focused agy card (tool approval needs-input or completion)
      await expect(harness.page.locator(`.notification-bell, [data-session-card="${sessionId}"] .pane-notification-badge`).first()).toBeVisible({ timeout: 60_000 })
      const bell = harness.page.locator('button[aria-label*="通知"]').first()
      await expect(bell).toBeVisible()
      await bell.click()
      await expect(harness.page.getByRole('region', { name: '通知中心' }).or(harness.page.locator('.notification-center')).first()).toBeVisible({ timeout: 10_000 })
      await saveScreenshot(harness.page, '05-notifications.png', {
        settleSelector: '.notification-center, [role="region"][aria-label="通知中心"]',
        settleMs: 500
      })
      await bell.click() // close notification center

      // Return focus to Antigravity card; if awaiting tool approval in Ask mode, measure & approve until idle
      await agyCard.click()
      await expect(visibleFocusedSurface(harness.page)).toHaveAttribute('data-session-id', sessionId)
      let recordedNeedLatency = false
      const askTurnDeadline = Date.now() + 90_000
      while (Date.now() < askTurnDeadline) {
        const cls = (await dagPage.locator(`.dag-node-card[data-session-id="${sessionId}"]`).getAttribute('class').catch(() => '')) ?? ''
        if (cls.includes('status-idle')) break
        if (cls.includes('status-needs-input')) {
          if (!recordedNeedLatency) {
            recordedNeedLatency = true
            const obsLog2 = await readDagStatusObserver(dagPage)
            const needObs = obsLog2.filter((o) => o.status === 'status-needs-input').at(-1)
            const needEv = readDbRows(harness.dataDir, sessionId).events.filter((e) => e.payload_json.includes('"needs-input"')).at(-1)
            if (needObs && needEv) {
              latencies.push({
                transition: 'running -> needs-input (tool_use)',
                triggerAtMs: notifyTriggerMs,
                runtimeEventAtMs: needEv.occurred_at,
                uiObservedAtMs: needObs.at,
                runtimeToUiMs: Math.max(0, needObs.at - needEv.occurred_at),
                within1000ms: Math.max(0, needObs.at - needEv.occurred_at) <= 1000,
                endToEndMs: needObs.at - notifyTriggerMs
              })
            }
          }
          await sendTerminalRawKey(surface, 'Enter')
          await harness.page.waitForTimeout(800)
          continue
        }
        await harness.page.waitForTimeout(300)
      }
      await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 30_000)

      // Test Shift+Tab round-trip out of Ask back to Accept Edits & Default, and menu switch from Ask -> Default
      await agyCard.click()
      await sendTerminalRawKey(surface, 'Shift+Tab')
      await expect(permBadge).toHaveText('AE', { timeout: 15_000 })
      await sendTerminalRawKey(surface, 'Shift+Tab')
      await expect(permBadge).toHaveText('D', { timeout: 15_000 })

      // Re-enter Ask via Plan -> Shift+Tab and verify menu selection switches out of Ask cleanly without respawn
      await permBadge.click()
      await harness.page.getByRole('menu', { name: '权限模式' }).getByRole('menuitem', { name: 'Plan Mode' }).click()
      await expect(permBadge).toHaveText('PM', { timeout: 15_000 })
      await expect(surface.locator('.xterm-rows')).toContainText(/Plan mode|\bplan\s*·/i, { timeout: 15_000 })
      await harness.page.waitForTimeout(2_500)
      await sendTerminalRawKey(surface, 'Shift+Tab')
      await expect(permBadge).toHaveText('AK', { timeout: 15_000 })
      await permBadge.click()
      await expect(harness.page.getByRole('menu', { name: '权限模式' }).locator('[role="menuitem"]')).toHaveCount(4)
      await harness.page.getByRole('menu', { name: '权限模式' }).getByRole('menuitem', { name: 'Default' }).click()
      await expect(permBadge).toHaveText('D', { timeout: 15_000 })
      expect(await positivePid(surface)).toBe(initialPid)

      // Switch INTO Bypass Permissions (requires confirmation + restarts with --dangerously-skip-permissions)
      await permBadge.click()
      await harness.page.getByRole('menu', { name: '权限模式' }).getByRole('menuitem', { name: 'Bypass Permissions' }).click()
      const bypassDialog = harness.page.getByRole('alertdialog')
      await expect(bypassDialog).toContainText('Antigravity')
      await bypassDialog.getByRole('button', { name: '确认切换' }).click()
      await expect.poll(async () => await positivePid(visibleFocusedSurface(harness.page)), { timeout: 25_000 }).not.toBe(initialPid)
      const bypassPid = await positivePid(visibleFocusedSurface(harness.page))
      await expect(hud.locator('.status-perm-badge')).toHaveText('BP', { timeout: 35_000 })
      await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 90_000)

      // Exit Bypass Permissions -> requires confirmation + restarts back
      await hud.locator('.status-perm-badge').click()
      await harness.page.getByRole('menu', { name: '权限模式' }).getByRole('menuitem', { name: 'Default' }).click()
      const exitBypassDialog = harness.page.getByRole('alertdialog')
      await expect(exitBypassDialog).toContainText('Antigravity')
      await exitBypassDialog.getByRole('button', { name: '确认退出' }).click()
      await expect.poll(async () => await positivePid(visibleFocusedSurface(harness.page)), { timeout: 30_000 }).not.toBe(bypassPid)
      await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 90_000)
      await saveScreenshot(harness.page, '11-execution-modes-bypass-done.png')

      // #12 /model inside terminal syncs HUD
      await sendTerminalLine(surface, '/model')
      await expect(surface.locator('.xterm-rows')).toContainText(/Select.*Model|Switch.*Model|Choose.*Model|Select.*Reasoning|↑\/↓/i, { timeout: 15_000 })
      await sendTerminalRawKey(surface, 'ArrowDown')
      await sendTerminalRawKey(surface, 'Enter')
      await expect.poll(async () => await modelBadge.innerText(), { timeout: 20_000 }).toBeTruthy()
      const modelAfter = await modelBadge.innerText()
      await saveScreenshot(hud, '12-model-readonly-sync.png')

      await saveProof('core-lifecycle.json', {
        sessionId,
        providerSessionId,
        modelBefore,
        modelAfter,
        latencies,
        dbBindingsAfter: readDbRows(harness.dataDir, sessionId).bindings
      })
    } finally {
      await harness.close().catch(() => {})
      await cleanupWorkspaceLinks(dirs)
    }
  })

  test('PRD #7 #9a + Latency (3x): Auto Title lock/restore, Restart recovery, and Running/Idle latency', async () => {
    const dirs = await createIsolatedWorkspace('title-restart')
    let harness = await launchPackagedApp(dirs)

    try {
      const { sessionId, surface, dagPage } = await launchAgyInFreshRepo(harness)
      const prompts = [
        { prompt: 'Remember codeword TITAN_904 and reply with only LATENCY_OK_1', token: 'LATENCY_OK_1' },
        { prompt: 'Reply with only LATENCY_OK_2', token: 'LATENCY_OK_2' },
        { prompt: 'Reply with only LATENCY_OK_3', token: 'LATENCY_OK_3' }
      ]

      const runningSamples: Array<{
        round: number
        triggerAtMs: number
        runtimeEventAtMs?: number
        uiObservedAtMs: number
        runtimeToUiMs?: number
        triggerToUiMs: number
        within1000ms?: boolean
      }> = []
      const idleSamples: Array<{
        round: number
        triggerAtMs: number
        runtimeEventAtMs?: number
        uiObservedAtMs: number
        runtimeToUiMs?: number
        within1000ms?: boolean
      }> = []

      for (let idx = 0; idx < prompts.length; idx += 1) {
        const round = idx + 1
        const item = prompts[idx]!
        await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 90_000)
        const prevMaxSeq = readDbRows(harness.dataDir, sessionId).events.at(-1)?.seq ?? 0
        const prevObsLen = (await readDagStatusObserver(dagPage)).length
        const triggerAtMs = Date.now()

        await sendTerminalLine(surface, item.prompt)
        await waitForDagNodeStatus(dagPage, sessionId, 'status-running', 30_000)
        await expect(surface.locator('.xterm-rows')).toContainText(item.token, { timeout: 90_000 })
        await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 90_000)
        await harness.page.waitForTimeout(300)

        const afterObs = (await readDagStatusObserver(dagPage)).slice(prevObsLen)
        const afterEvents = readDbRows(harness.dataDir, sessionId).events.filter((e) => e.seq > prevMaxSeq)
        const runningObs = afterObs.find((o) => o.status === 'status-running')
        const idleObs = afterObs.findLast((o) => o.status === 'status-idle')
        const runningEv = afterEvents.find((e) => e.payload_json.includes('"running"'))
        const idleEv = afterEvents.findLast((e) => e.payload_json.includes('"idle"'))

        expect(runningObs, `round ${round} missing running observer entry`).toBeTruthy()
        expect(idleObs, `round ${round} missing idle observer entry`).toBeTruthy()

        if (runningObs) {
          const rtToUi = runningEv ? Math.max(0, runningObs.at - runningEv.occurred_at) : undefined
          runningSamples.push({
            round,
            triggerAtMs,
            ...(runningEv ? { runtimeEventAtMs: runningEv.occurred_at } : {}),
            uiObservedAtMs: runningObs.at,
            ...(rtToUi !== undefined ? { runtimeToUiMs: rtToUi, within1000ms: rtToUi <= 1000 } : {}),
            triggerToUiMs: runningObs.at - triggerAtMs
          })
        }
        if (idleObs) {
          const rtToUi = idleEv ? Math.max(0, idleObs.at - idleEv.occurred_at) : undefined
          idleSamples.push({
            round,
            triggerAtMs,
            ...(idleEv ? { runtimeEventAtMs: idleEv.occurred_at } : {}),
            uiObservedAtMs: idleObs.at,
            ...(rtToUi !== undefined ? { runtimeToUiMs: rtToUi, within1000ms: rtToUi <= 1000 } : {})
          })
        }
      }

      const providerSessionId = await waitForProviderSessionId(harness.dataDir, sessionId)
      expect(runningSamples.length).toBe(3)
      expect(idleSamples.length).toBe(3)
      for (const s of [...runningSamples, ...idleSamples]) {
        if (s.within1000ms !== undefined) {
          expect(s.within1000ms).toBe(true)
        }
      }

      const runningRtMs = runningSamples.map((s) => s.runtimeToUiMs).filter((n): n is number => typeof n === 'number')
      const idleRtMs = idleSamples.map((s) => s.runtimeToUiMs).filter((n): n is number => typeof n === 'number')
      const avg = (arr: number[]) => (arr.length > 0 ? Math.round((arr.reduce((a, b) => a + b, 0) / arr.length) * 10) / 10 : null)
      const latencySummary = {
        runningCount: runningSamples.length,
        runningAvgMs: avg(runningRtMs),
        runningMaxMs: runningRtMs.length > 0 ? Math.max(...runningRtMs) : null,
        runningTriggerAvgMs: avg(runningSamples.map((s) => s.triggerToUiMs)),
        idleCount: idleSamples.length,
        idleAvgMs: avg(idleRtMs),
        idleMaxMs: idleRtMs.length > 0 ? Math.max(...idleRtMs) : null
      }

      await saveProof('core-and-latency.json', {
        sessionId,
        providerSessionId,
        runningSamples,
        idleSamples,
        summary: latencySummary
      })

      // #7 Auto Title, Manual Rename Lock, and Restore Auto Title
      const pane = paneForSession(harness.page, sessionId)
      await expect.poll(async () => {
        const title = (await pane.locator('.pane-title').innerText()).trim()
        return title && title !== 'Shell' && title !== 'Antigravity' ? title : ''
      }, { timeout: 30_000 }).toBeTruthy()
      const initialPaneTitle = await pane.locator('.pane-title').innerText()

      await openPaneMenu(pane)
      await harness.page.getByRole('menuitem', { name: '重命名…' }).click()
      const renameInput = harness.page.getByRole('dialog').locator('input').first()
      await renameInput.fill('手工锁定标题-E2E')
      await renameInput.press('Enter')
      await expect(harness.page.getByRole('dialog')).toHaveCount(0, { timeout: 10_000 })
      await expect(pane.locator('.pane-title')).toHaveText('手工锁定标题-E2E')

      // Run a short no-tool turn -> manual title must NOT be overwritten
      await sendTerminalLine(surface, 'Reply with only RENAME_LOCKED_OK')
      await expect(surface.locator('.xterm-rows')).toContainText('RENAME_LOCKED_OK', { timeout: 90_000 })
      await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 90_000)
      await expect(pane.locator('.pane-title')).toHaveText('手工锁定标题-E2E')

      // Restore auto title
      await openPaneMenu(pane)
      await harness.page.getByRole('menuitem', { name: '恢复 Antigravity 自动标题' }).click()
      await expect(pane.locator('.pane-title')).not.toHaveText('手工锁定标题-E2E', { timeout: 15_000 })
      await expect(pane.locator('.pane-title')).not.toHaveText('Shell')
      const restoredAutoTitle = await pane.locator('.pane-title').innerText()
      await saveScreenshot(harness.page, '07-auto-title.png')

      // #9a Real App Restart Recovery (--conversation resumes original conversation)
      await harness.close()
      await new Promise((r) => setTimeout(r, 500))
      harness = await launchPackagedApp(dirs)
      const restoredPane = paneForSession(harness.page, sessionId)
      const restoredSurface = restoredPane.locator('.terminal-surface')
      await positivePid(restoredSurface)
      await expect(hudForSession(harness.page, sessionId)).toHaveAttribute('data-hud-mode', 'agent', { timeout: 45_000 })
      await expect(hudForSession(harness.page, sessionId).locator('.status-model')).toBeVisible({ timeout: 45_000 })
      await expect(restoredPane.locator('.provider-restore-banner')).toHaveCount(0)
      await expect(restoredPane.locator('.pane-title')).not.toHaveText('手工锁定标题-E2E', { timeout: 15_000 })
      await expect(restoredPane.locator('.pane-title')).not.toHaveText('Shell')
      await harness.page.waitForTimeout(800)

      await sendTerminalLine(restoredSurface, 'What codeword did I ask you to remember earlier? Reply with only the codeword and RESTART_OK.')
      await expect(restoredSurface.locator('.xterm-rows')).toContainText('TITAN_904', { timeout: 90_000 })
      await expect(restoredPane.locator('.provider-restore-banner')).toHaveCount(0)
      await saveScreenshot(harness.page, '09a-restart-recovery.png')

      await saveProof('core-and-latency.json', {
        sessionId,
        providerSessionId,
        initialPaneTitle,
        restoredAutoTitle,
        runningSamples,
        idleSamples,
        summary: latencySummary,
        dbBindingsAfter: readDbRows(harness.dataDir, sessionId).bindings
      })
    } finally {
      await harness.close().catch(() => {})
      await cleanupWorkspaceLinks(dirs)
    }
  })

  test('PRD #2 #8 #10 + Running-Parent Fork: DAG/Badge/ParentProjection, Fork Current/Worktree/Running, SessionLoader', async () => {
    const dirs = await createIsolatedWorkspace('fork-loader', { secondCommit: true })
    const harness = await launchPackagedApp(dirs)

    try {
      const { sessionId: parentId, surface: parentSurface, dagPage } = await launchAgyInFreshRepo(harness)

      // Complete turn 1 with a secret token to test fork context inheritance
      await sendTerminalLine(parentSurface, 'Remember the exact codeword COBALT_719 and reply ONLY TURN1_SAVED.')
      await waitForDagNodeStatus(dagPage, parentId, 'status-idle', 90_000)
      await expect(parentSurface.locator('.xterm-rows')).toContainText('TURN1_SAVED', { timeout: 30_000 })
      const parentProviderId = await waitForProviderSessionId(harness.dataDir, parentId)

      const parentPane = paneForSession(harness.page, parentId)
      await expect(parentPane.locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)')).toHaveAttribute('aria-disabled', 'false', { timeout: 15_000 })

      // Fork Case A: Current Worktree (create child branch via pane header fork button)
      await parentPane.locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)').click()
      await confirmForkDialog(harness.page, '子分支-当前目录', 'current')

      const child1Surface = visibleFocusedSurface(harness.page)
      let child1Id = ''
      await expect.poll(async () => {
        child1Id = (await child1Surface.getAttribute('data-session-id')) ?? ''
        return child1Id && child1Id !== parentId ? child1Id : ''
      }, { timeout: 25_000 }).toBeTruthy()
      await expect(paneForSession(harness.page, child1Id).locator('.fork-progress-overlay')).toHaveCount(0, { timeout: 45_000 })
      await expect(harness.page.getByRole('button', { name: '返回父会话' })).toBeVisible({ timeout: 10_000 })

      let child1ProviderId = ''
      await expect.poll(() => {
        const b = readDbRows(harness.dataDir, child1Id).bindings.find(
          (x) => x.provider === 'antigravity' && x.resume_state === 'available' && x.provider_session_id !== parentProviderId
        )
        child1ProviderId = b?.provider_session_id ?? ''
        return child1ProviderId
      }, { timeout: 45_000 }).toBeTruthy()
      expect(child1ProviderId).not.toBe(parentProviderId)

      // Ask child1 for the inherited codeword!
      await waitForDagNodeStatus(dagPage, child1Id, 'status-idle', 90_000)
      await sendTerminalLine(child1Surface, 'What is the exact codeword I asked you to remember? Reply with ONLY that codeword.')
      await waitForDagNodeStatus(dagPage, child1Id, 'status-idle', 90_000)
      await expect(child1Surface.locator('.xterm-rows')).toContainText('COBALT_719', { timeout: 30_000 })

      // Return to parent, verify ChildSessionBadge (#2) & DAG (#2)
      await harness.page.getByRole('button', { name: '返回父会话' }).click()
      await expect(parentPane.locator('.child-session-badge')).toContainText('1', { timeout: 15_000 })
      await expect(dagPage.locator(`.dag-node-card[data-session-id="${parentId}"]`)).toContainText('Antigravity')
      await expect(dagPage.locator(`.dag-node-card[data-session-id="${child1Id}"]`)).toContainText('Antigravity')
      await saveScreenshot(dagPage, '02-identifiers-dag.png')

      // Fork Case B: New Worktree
      await parentPane.locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)').click()
      await confirmForkDialog(harness.page, '子分支-独立Worktree', 'new')

      const child2Surface = visibleFocusedSurface(harness.page)
      let child2Id = ''
      await expect.poll(async () => {
        child2Id = (await child2Surface.getAttribute('data-session-id')) ?? ''
        return child2Id && child2Id !== parentId && child2Id !== child1Id ? child2Id : ''
      }, { timeout: 30_000 }).toBeTruthy()
      await expect(paneForSession(harness.page, child2Id).locator('.fork-progress-overlay')).toHaveCount(0, { timeout: 45_000 })

      let child2ProviderId = ''
      await expect.poll(() => {
        const b = readDbRows(harness.dataDir, child2Id).bindings.find(
          (x) => x.provider === 'antigravity' && x.resume_state === 'available' && x.provider_session_id !== parentProviderId
        )
        child2ProviderId = b?.provider_session_id ?? ''
        return child2ProviderId
      }, { timeout: 45_000 }).toBeTruthy()
      expect(child2ProviderId).not.toBe(parentProviderId)
      expect(child2ProviderId).not.toBe(child1ProviderId)
      await saveScreenshot(harness.page, '10a-fork-branches.png')

      // Concurrent Check: Return to parent and Fork while Parent is ACTIVELY RUNNING ('running')
      await harness.page.getByRole('button', { name: '返回父会话' }).click()
      await expect(parentPane.locator('.child-session-badge')).toContainText('2', { timeout: 15_000 })
      await harness.page.locator(`[data-session-card="${parentId}"]`).click()
      const parentTranscriptPath = join(
        GEMINI_BRAIN_ROOT,
        parentProviderId,
        '.system_generated',
        'logs',
        'transcript.jsonl'
      )
      const parentDbPath = join(GEMINI_CONVERSATIONS_ROOT, `${parentProviderId}.db`)
      const parentTranscriptBefore = await readFile(parentTranscriptPath, 'utf8')
      const parentLineCountBefore = parentTranscriptBefore.split('\n').filter(Boolean).length

      await sendTerminalLine(
        parentSurface,
        'Write 25 numbered short sentences (each about 15 words) explaining deterministic state machines. Do not use any tools or edit any files.'
      )
      await waitForDagNodeStatus(dagPage, parentId, 'status-running', 30_000)

      // Trigger Fork immediately while parent is running!
      await parentPane.locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)').click()
      await confirmForkDialog(harness.page, '子分支-父运行中Fork', 'current')

      const child3Surface = visibleFocusedSurface(harness.page)
      let child3Id = ''
      await expect.poll(async () => {
        child3Id = (await child3Surface.getAttribute('data-session-id')) ?? ''
        return child3Id && ![parentId, child1Id, child2Id].includes(child3Id) ? child3Id : ''
      }, { timeout: 30_000 }).toBeTruthy()

      // Wait for both child3 fork & parent active turn to finish cleanly
      await expect(paneForSession(harness.page, child3Id).locator('.fork-progress-overlay')).toHaveCount(0, { timeout: 60_000 })
      await waitForDagNodeStatus(dagPage, parentId, 'status-idle', 90_000)
      await waitForDagNodeStatus(dagPage, child3Id, 'status-idle', 90_000)

      // Verify parent provider binding wasn't hijacked by child3
      const parentBindingAfter = readDbRows(harness.dataDir, parentId).bindings.find(
        (x) => x.provider === 'antigravity' && x.resume_state === 'available'
      )
      expect(parentBindingAfter?.provider_session_id).toBe(parentProviderId)
      const child3Binding = readDbRows(harness.dataDir, child3Id).bindings.find(
        (x) => x.provider === 'antigravity' && x.resume_state === 'available' && x.provider_session_id !== parentProviderId
      )
      expect(child3Binding?.provider_session_id).toBeTruthy()
      expect(child3Binding?.provider_session_id).not.toBe(parentProviderId)

      // Verify parent transcript JSONL integrity & sqlite db integrity
      const parentTranscriptAfter = await readFile(parentTranscriptPath, 'utf8')
      const parentLinesAfter = parentTranscriptAfter.split('\n').filter(Boolean)
      expect(parentLinesAfter.length).toBeGreaterThanOrEqual(parentLineCountBefore)
      for (const line of parentLinesAfter) {
        expect(() => JSON.parse(line)).not.toThrow()
      }
      let dbIntegrity = 'missing'
      const dbExists = await stat(parentDbPath).then(() => true).catch(() => false)
      if (dbExists) {
        const agyDb = new DatabaseSync(parentDbPath, { readOnly: true })
        try {
          const row = agyDb.prepare('PRAGMA integrity_check').get() as { integrity_check?: string } | undefined
          dbIntegrity = row?.integrity_check ?? 'unknown'
        } finally {
          agyDb.close()
        }
        expect(dbIntegrity).toBe('ok')
      }
      await saveScreenshot(harness.page, '10b-fork-while-running.png')

      // #8 Load Existing Session via SessionLoaderDialog -> Antigravity tab
      await harness.page.getByRole('button', { name: '返回父会话' }).click()
      await expect(parentPane.locator('.child-session-badge')).toContainText('3', { timeout: 15_000 })
      const { sessionId: freshShellId } = await createSiblingShellAndFocus(harness.page, parentId)
      await paneForSession(harness.page, freshShellId).locator('.pane-load-session').click()
      const loaderDialog = harness.page.getByRole('dialog')
      await expect(loaderDialog).toBeVisible()
      await loaderDialog.getByRole('tab', { name: 'Antigravity' }).click()
      await expect(loaderDialog).toContainText('载入 Antigravity 会话')

      const firstItem = loaderDialog.locator('.session-loader-result-main').first()
      await expect(firstItem).toBeVisible({ timeout: 20_000 })
      await firstItem.click()
      await expect(loaderDialog.locator('.session-loader-events')).toContainText(/#\d+/, { timeout: 15_000 })
      await saveScreenshot(harness.page, '08-session-loader.png')

      await loaderDialog.getByRole('button', { name: '载入到当前卡片' }).click()
      const duplicateConfirm = harness.page.getByRole('button', { name: '仍然载入' })
      if (await duplicateConfirm.isVisible({ timeout: 2_000 }).catch(() => false)) {
        await duplicateConfirm.click()
      }
      await expect(harness.page.getByRole('dialog', { name: '载入 Antigravity 会话' })).toHaveCount(0, { timeout: 20_000 })
      await expect(hudForSession(harness.page, freshShellId)).toHaveAttribute('data-hud-mode', 'agent', { timeout: 30_000 })

      await saveProof('fork-concurrent-loader.json', {
        parentId,
        parentProviderId,
        child1Id,
        child1ProviderId,
        child2Id,
        child2ProviderId,
        child3Id,
        child3ProviderId: child3Binding?.provider_session_id,
        parentLinesBefore: parentLineCountBefore,
        parentLinesAfter: parentLinesAfter.length,
        parentDbExists: dbExists,
        parentDbIntegrity: dbIntegrity
      })
    } finally {
      await harness.close().catch(() => {})
      await cleanupWorkspaceLinks(dirs)
    }
  })

  test('PRD #13: mt cross-card control inside real Antigravity card', async () => {
    const dirs = await createIsolatedWorkspace('mt-cross')
    const harness = await launchPackagedApp(dirs)

    try {
      const card1Surface = visibleFocusedSurface(harness.page)
      const card1Id = (await card1Surface.getAttribute('data-session-id')) ?? ''
      await positivePid(card1Surface)
      await waitForEmptyShellPrompt(card1Surface)

      // Create Card 2 (Shell) and print unique marker
      const { sessionId: card2Id, surface: card2Surface } = await createSiblingShellAndFocus(harness.page, card1Id)
      await sendTerminalLine(card2Surface, 'echo "MT_PEER_TOKEN_8421"')
      await expect(card2Surface.locator('.xterm-rows')).toContainText('MT_PEER_TOKEN_8421', { timeout: 10_000 })

      // Focus Card 1 & promote to agy
      await harness.page.locator(`[data-session-card="${card1Id}"]`).click()
      const dagPage = await openDagWindow(harness)
      await harness.page.bringToFront()
      await sendTerminalLine(card1Surface, 'agy')
      await expect(card1Surface.locator('.xterm-rows')).toContainText(/Do you trust/i, { timeout: 30_000 })
      await sendTerminalRawKey(card1Surface, 'Enter')
      await expect(hudForSession(harness.page, card1Id)).toHaveAttribute('data-hud-mode', 'agent', { timeout: 45_000 })
      await expect(hudForSession(harness.page, card1Id).locator('.status-model')).toBeVisible({ timeout: 45_000 })
      await waitForDagNodeStatus(dagPage, card1Id, 'status-idle', 90_000)

      await sendTerminalLine(
        card1Surface,
        'Run `mt list` to inspect the cards, then run `mt read` on the `[shell]` sibling card (use `left` or `right` depending on `mt list`) with `--lines 20`. Finally reply with ONLY the MT_PEER_TOKEN value you saw in that card.'
      )
      const card1DagNode = dagPage.locator(`.dag-node-card[data-session-id="${card1Id}"]`)
      const mtDeadline = Date.now() + 180_000
      let sawTokenInRows = false
      while (Date.now() < mtDeadline) {
        const cls = (await card1DagNode.getAttribute('class').catch(() => '')) ?? ''
        if (cls.includes('status-needs-input')) {
          await sendTerminalRawKey(card1Surface, 'Enter')
          await harness.page.waitForTimeout(800)
          continue
        }
        const rowsText = await card1Surface.locator('.xterm-rows').innerText().catch(() => '')
        if (rowsText.includes('MT_PEER_TOKEN_8421')) {
          sawTokenInRows = true
        }
        if (sawTokenInRows && cls.includes('status-idle')) {
          break
        }
        await harness.page.waitForTimeout(300)
      }
      await waitForDagNodeStatus(dagPage, card1Id, 'status-idle', 90_000)
      await expect(card1Surface.locator('.xterm-rows')).toContainText('MT_PEER_TOKEN_8421', { timeout: 30_000 })
      await saveScreenshot(harness.page, '13-mt-cross-card.png')
      await saveProof('mt-cross-card.json', { card1Id, card2Id, verifiedToken: 'MT_PEER_TOKEN_8421' })
    } finally {
      await harness.close().catch(() => {})
      await cleanupWorkspaceLinks(dirs)
    }
  })

  test('PRD #9b #14: Controlled fixtures for missing binary, unauthenticated sign-in prompt, and restore failure', async () => {
    // 14a: Missing binary via MATOU_ANTIGRAVITY_COMMAND pointing to non-existent binary
    const missingDirs = await createIsolatedWorkspace('missing-bin')
    const missingHarness = await launchPackagedApp(missingDirs, {
      MATOU_ANTIGRAVITY_COMMAND: '/private/tmp/does-not-exist/agy-missing-binary'
    })
    try {
      const surface = visibleFocusedSurface(missingHarness.page)
      const shellId = (await surface.getAttribute('data-session-id')) ?? ''
      await positivePid(surface)
      await waitForEmptyShellPrompt(surface)
      // Create an Antigravity card directly via mt CLI and focus it (since shell-promote rolls back on spawn failure)
      await sendTerminalLine(surface, 'mt create session --canvas current --profile antigravity --enter && mt focus right')
      const antigravityCard = missingHarness.page.locator('[data-session-card]').nth(1)
      await expect(antigravityCard).toBeVisible({ timeout: 15_000 })
      const failureCard = antigravityCard.locator('.session-start-failure-card')
      await expect(failureCard).toBeVisible({ timeout: 20_000 })
      await expect(failureCard).toContainText('未检测到 Antigravity')
      await expect(failureCard).toContainText('agy-missing-binary')
      await saveScreenshot(missingHarness.page, '14a-missing-binary.png')
      await saveProof('missing-binary.json', {
        fixtureMode: 'controlled-missing-binary',
        bannerText: await failureCard.innerText()
      })
    } finally {
      await missingHarness.close().catch(() => {})
      await cleanupWorkspaceLinks(missingDirs)
    }

    // 14b + 9b: Unauthenticated sign-in prompt -> needs-input -> Enter resolves -> idle; and Restore failure banner
    const stubDirs = await createIsolatedWorkspace('controlled-stub')
    const stubPath = join(stubDirs.rootDir, 'agy-stub.py')
    const stubScript = [
      '#!/usr/bin/env python3',
      'import os, sys, json, subprocess',
      'args = sys.argv[1:]',
      'if "--conversation" in args:',
      '    sys.stderr.write("Error: conversation not found\\n")',
      '    sys.stderr.flush()',
      '    sys.exit(1)',
      'settings_file = None',
      'for i, a in enumerate(args):',
      '    if a == "--settings" and i + 1 < len(args):',
      '        settings_file = args[i + 1]',
      'sys.stdout.write("You are currently not signed in. Please press Enter to sign in.\\r\\n")',
      'sys.stdout.flush()',
      'sys.stdin.readline()',
      'sys.stdout.write("Signed in! Antigravity ready.\\r\\n")',
      'sys.stdout.flush()',
      'if settings_file and os.path.exists(settings_file):',
      '    with open(settings_file) as f:',
      '        cfg = json.load(f)',
      '    cmd = cfg.get("statusLine", {}).get("command")',
      '    if cmd:',
      '        payload = json.dumps({"session_id": "stub-agy-auth-ok", "agent_state": "idle", "cycle_mode": "auto", "model": {"id": "gemini-3-pro"}})',
      '        subprocess.run(cmd, shell=True, input=payload.encode(), timeout=5)',
      'while True:',
      '    line = sys.stdin.readline()',
      '    if not line: break',
      '    sys.stdout.write(f"ECHO:{line}")',
      '    sys.stdout.flush()',
      ''
    ].join('\n')
    await writeFile(stubPath, stubScript)
    await chmod(stubPath, 0o755)

    let stubHarness = await launchPackagedApp(stubDirs, {
      MATOU_ANTIGRAVITY_COMMAND: stubPath
    })
    try {
      const surface = visibleFocusedSurface(stubHarness.page)
      const sessionId = (await surface.getAttribute('data-session-id')) ?? ''
      await positivePid(surface)
      await waitForEmptyShellPrompt(surface)
      const dagPage = await openDagWindow(stubHarness)
      await stubHarness.page.bringToFront()

      await sendTerminalLine(surface, 'agy')
      await expect(surface.locator('.xterm-rows')).toContainText('You are currently not signed in', { timeout: 20_000 })
      await waitForDagNodeStatus(dagPage, sessionId, 'status-needs-input', 20_000)
      await saveScreenshot(stubHarness.page, '14b-unauthenticated-needs-input.png')
      await saveScreenshot(dagPage, '14b-unauthenticated-dag.png')

      // Simulate user pressing Enter to complete login
      await sendTerminalRawKey(surface, 'Enter')
      await expect(surface.locator('.xterm-rows')).toContainText('Signed in! Antigravity ready.', { timeout: 15_000 })
      await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 20_000)

      // Restart with same stub (which rejects --conversation with exit 1) -> tests #9b Restore Failure banner!
      await stubHarness.close()
      stubHarness = await launchPackagedApp(stubDirs, {
        MATOU_ANTIGRAVITY_COMMAND: stubPath
      })
      const restoreBanner = paneForSession(stubHarness.page, sessionId).locator('.provider-restore-banner')
      await expect(restoreBanner).toBeVisible({ timeout: 25_000 })
      await expect(restoreBanner).toContainText('Antigravity 恢复失败')
      await expect(restoreBanner.getByRole('button', { name: '重试恢复' })).toBeVisible()
      await expect(restoreBanner.getByRole('button', { name: '新开 Antigravity' })).toBeVisible()
      await saveScreenshot(stubHarness.page, '09b-restore-failed.png')
      await saveProof('unauth-and-restore-failed.json', {
        fixtureMode: 'controlled-stub',
        sessionId,
        restoreBannerText: await restoreBanner.innerText()
      })
    } finally {
      await stubHarness.close().catch(() => {})
      await cleanupWorkspaceLinks(stubDirs)
    }
  })
})
