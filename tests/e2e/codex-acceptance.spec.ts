import { execFile } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { chmod, copyFile, mkdir, mkdtemp, readFile, readdir, rename, rm, symlink, unlink, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'

import { expect, test, _electron as electron, type ElectronApplication, type Locator, type Page } from '@playwright/test'

const execFileAsync = promisify(execFile)

const SCRATCHPAD_ROOT = resolve(process.env.MATOU_CODEX_ACCEPTANCE_SCRATCH_ROOT ??
  resolve(import.meta.dirname, '../../.cache/c5/codex'))
const PACKAGED_EXECUTABLE = resolve(
  import.meta.dirname,
  '../../apps/desktop/release/mac-arm64/码头.app/Contents/MacOS/码头'
)
const EVIDENCE_DIR = resolve(process.env.MATOU_CODEX_ACCEPTANCE_EVIDENCE_DIR ??
  resolve(import.meta.dirname, '../../docs/acceptance/evidence/codex-stage5'))
const RESULTS_DIR = join(EVIDENCE_DIR, 'results')

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
  turnProofs: Array<{ sessionId: string; providerSessionId: string; turnId: string; prompt: string; finalAnswer: string; completedAnswer: string }>
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
  // via a short in-project symlink pointing into the isolated data dir.
  const linkRoot = resolve(process.env.MATOU_ACCEPTANCE_LINK_ROOT ?? resolve(import.meta.dirname, '../../.c5'))
  await mkdir(linkRoot, { recursive: true })
  const dataDir = join(linkRoot, `c-${randomBytes(4).toString('hex')}`)
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
  const codexHome = join(rootDir, 'codex-home')
  await mkdir(codexHome)
  // HOME remains real; only Codex trust/config/session writes are isolated.
  await copyFile(join(process.env.HOME!, '.codex/auth.json'), join(codexHome, 'auth.json'))
  await chmod(join(codexHome, 'auth.json'), 0o600)
  const config = [
    `model = ${JSON.stringify(process.env.MATOU_CODEX_ACCEPTANCE_MODEL ?? 'gpt-6-astra')}`,
    'model_reasoning_effort = "medium"',
    'model_provider = "openai"',
    'approval_policy = "on-request"',
    'sandbox_mode = "read-only"',
    'check_for_update_on_startup = false',
    `[projects.${JSON.stringify(repoDir)}]`,
    'trust_level = "trusted"'
  ].join('\n') + '\n'
  await writeFile(join(codexHome, 'config.toml'), config)
  const binary = (await execFileAsync('/usr/bin/which', ['codex'])).stdout.trim()
  const quote = (value: string) => "'" + value.replaceAll("'", "'\\''") + "'"
  await writeFile(join(rootDir, 'codex-wrapper'), `#!/bin/sh\nexec ${quote(binary)} --no-daemon --no-alt-screen -c check_for_update_on_startup=false "$@"\n`)
  await chmod(join(rootDir, 'codex-wrapper'), 0o700)
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
    CODEX_HOME: join(dirs.rootDir, 'codex-home'),
    MATOU_CODEX_COMMAND: join(dirs.rootDir, 'codex-wrapper'),
    MATOU_LOCALE: 'zh-CN',
    MATOU_E2E: '1',
    MATOU_E2E_DISPLAY: process.env.MATOU_E2E_DISPLAY ?? 'primary',
    MATOU_E2E_WINDOW_CLOSE: 'hide',
    MATOU_DATA_DIR: dirs.dataDir,
    MATOU_DEFAULT_WORKSPACE: dirs.repoDir,
    ELECTRON_USER_DATA_DIR: dirs.userDataDir,
    ...extraEnv
  }
  for (const key of Object.keys(env)) if (key.startsWith('MATOU_CONTROL_')) delete env[key]
  delete env.ELECTRON_RUN_AS_NODE

  const app = await electron.launch({
    executablePath: PACKAGED_EXECUTABLE,
    args: [`--user-data-dir=${dirs.userDataDir}`],
    env
  })
  const page = await app.firstWindow()
  await page.waitForLoadState('domcontentloaded')
  await page.locator('[data-testid="terminal-pane"]:visible').first().waitFor({ timeout: 30_000 })
  let checkingUpgrade = false
  // An update prompt must never receive a later test Enter, including after a
  // permission respawn or app restart. Escape and close that test app at once.
  const upgradeGuard = setInterval(() => {
    if (checkingUpgrade || page.isClosed()) return
    checkingUpgrade = true
    void (async () => {
      const rows = (await page.locator('.xterm-rows:visible').allTextContents()).join('\n')
      if (/Update available|Updating Codex/.test(rows)) {
        await visibleFocusedSurface(page).locator('.xterm-helper-textarea').press('Escape').catch(() => {})
        await saveProof('upgrade-guard.json', { rootDir: dirs.rootDir, at: Date.now(), action: 'Escape then close; acceptance aborted' })
        clearInterval(upgradeGuard)
        await app.close()
      }
    })().catch(() => {}).finally(() => { checkingUpgrade = false })
  }, 200)
  return {
    ...dirs,
    app,
    page,
    launchEnv: env,
    turnProofs: [],
    async close() {
      clearInterval(upgradeGuard)
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

function eventWorkStatus(event: { payload_json: string }, sessionId: string): string | undefined {
  const parsed = JSON.parse(event.payload_json)
  return parsed.workStatus
    ?? parsed.graph?.nodes?.find((node: any) => node.sessionId === sessionId)?.workStatus
    ?? (parsed.session?.id === sessionId ? parsed.session.workStatus : undefined)
}

function readCurrentWorkStatus(dataDir: string, sessionId: string): { status: string } | null {
  const db = new DatabaseSync(join(dataDir, 'matou.sqlite'), { readOnly: true })
  try {
    return db.prepare('SELECT work_status AS status FROM sessions WHERE id = ?').get(sessionId) as { status: string } | null
  } finally { db.close() }
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

async function waitForProviderSessionId(dataDir: string, sessionId: string, timeout = 30_000): Promise<string> {
  let providerSessionId = ''
  await expect.poll(() => {
    const { bindings } = readDbRows(dataDir, sessionId)
    const binding = bindings.find((b) => b.provider === 'codex' && b.resume_state !== 'expired')
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


// These helpers inspect genuine rollout output. Prompt echo is never accepted as
// proof of an answer or remembered context.
async function rolloutFile(h: PackagedHarness, id: string): Promise<string> {
  const walk = async (dir: string): Promise<string[]> => {
    const result: string[] = []
    for (const item of await readdir(dir, { withFileTypes: true }).catch(() => [])) {
      const path = join(dir, item.name)
      if (item.isDirectory()) result.push(...await walk(path))
      else if (item.name.includes(id) && item.name.endsWith('.jsonl')) result.push(path)
    }
    return result
  }
  let paths: string[] = []
  await expect.poll(async () => {
    paths = await walk(join(h.rootDir, 'codex-home/sessions'))
    return paths.length
  }, { timeout: 30_000 }).toBe(1)
  return paths[0]!
}

async function rolloutEvents(h: PackagedHarness, id: string): Promise<any[]> {
  const contents = await readFile(await rolloutFile(h, id), 'utf8')
  return contents.split('\n').flatMap((line) => {
    try { return [JSON.parse(line)] } catch { return [] }
  })
}

async function completeTurn(h: PackagedHarness, sid: string, surface: Locator, prompt: string, answer: RegExp): Promise<string> {
  const binding = readDbRows(h.dataDir, sid).bindings.find((b) => b.provider === 'codex' && b.resume_state !== 'expired')
  const before = binding ? (await rolloutEvents(h, binding.provider_session_id))
    .filter((e) => e.payload?.type === 'task_complete').length : 0
  await sendTerminalLine(surface, prompt)
  const id = await waitForProviderSessionId(h.dataDir, sid)
  let final = ''
  let finalTurn = ''
  let completedAnswer = ''
  await expect.poll(async () => {
    const events = await rolloutEvents(h, id)
    const completed = events.filter((e) => e.payload?.type === 'task_complete')
    if (completed.length <= before) return ''
    const completion = completed.at(-1)?.payload
    finalTurn = completion?.turn_id ?? ''
    completedAnswer = completion?.last_agent_message ?? ''
    const turnOf = (e: any) => e.payload?.internal_chat_message_metadata_passthrough?.turn_id
      ?? e.metadata?.retained_source?.id?.turn_id
    const user = events.findLast((e) => e.payload?.type === 'message' && e.payload.role === 'user' && turnOf(e) === finalTurn)
    if (!user?.payload.content?.some((c: any) => c.text === prompt)) return ''
    const assistant = events.findLast((e) => e.payload?.type === 'message' && e.payload.role === 'assistant'
      && e.payload.phase === 'final_answer' && turnOf(e) === finalTurn)
    final = assistant?.payload.content?.filter((c: any) => c.type === 'output_text').map((c: any) => c.text).join('\n') ?? ''
    return final === completedAnswer ? final : '' 
  }, { timeout: 150_000, intervals: [200, 500, 1000] }).toMatch(answer)
  h.turnProofs.push({ sessionId: sid, providerSessionId: id, turnId: finalTurn, prompt, finalAnswer: final, completedAnswer })
  await saveProof(`turns-${h.rootDir.split('/').at(-1)}.json`, h.turnProofs)
  await expect.poll(() => readCurrentWorkStatus(h.dataDir, sid)?.status, { timeout: 20_000 }).toBe('idle')
  return final
}

async function launchCodex(h: PackagedHarness): Promise<{ sessionId: string; surface: Locator; dagPage: Page }> {
  const surface = visibleFocusedSurface(h.page)
  const sessionId = (await surface.getAttribute('data-session-id'))!
  await positivePid(surface)
  await waitForEmptyShellPrompt(surface)
  const dagPage = await openDagWindow(h)
  await installDagStatusObserver(dagPage, sessionId)
  await h.page.bringToFront()
  await sendTerminalLine(surface, 'codex')
  await expect(surface.locator('.xterm-rows')).toContainText(/Codex/, { timeout: 45_000 })
  // Trust prompts are a real CLI behavior; never accept an update prompt.
  const rows = await surface.locator('.xterm-rows').innerText()
  expect(rows).not.toMatch(/Update available|Updating Codex/)
  if (/trust.*(?:folder|directory)|trust the contents/i.test(rows)) await sendTerminalRawKey(surface, 'Enter')
  await expect(paneForSession(h.page, sessionId).locator('.pane-title')).toHaveText('Codex')
  await expect(dagPage.locator(`.dag-node-card[data-session-id="${sessionId}"]`)).toContainText('Codex')
  await waitForDagNodeStatus(dagPage, sessionId, 'status-starting')
  await saveScreenshot(h.page, '01-codex-starting.png')
  return { sessionId, surface, dagPage }
}

async function dispose(h: PackagedHarness): Promise<void> {
  await saveScreenshot(h.page, `last-${h.rootDir.split('/').at(-1)}.png`, { settleMs: 0 }).catch(() => {})
  await h.close().catch(() => {})
  await cleanupWorkspaceLinks(h)
  // Keep DB/rollouts and evidence, remove the copied credential immediately.
  await rm(join(h.rootDir, 'codex-home/auth.json'), { force: true })
}

async function changeMode(h: PackagedHarness, sid: string, mode: string): Promise<{ before: number; after: number }> {
  const surface = paneForSession(h.page, sid).locator('.terminal-surface')
  const before = await positivePid(surface)
  const badge = hudForSession(h.page, sid).locator('.status-perm-badge')
  const leavingBypass = (await badge.innerText()).trim() === 'BP'
  await badge.click()
  const menu = h.page.getByRole('menu', { name: '权限模式' })
  await expect(menu.getByRole('menuitem')).toHaveCount(4)
  await saveScreenshot(menu, '05-execution-modes.png')
  await menu.getByRole('menuitem', { name: mode, exact: true }).click()
  if (mode === 'Bypass Permissions' || leavingBypass) {
    const confirmation = h.page.getByRole('alertdialog', {
      name: mode === 'Bypass Permissions' ? '切换到高权限模式' : '退出高权限模式', exact: true
    })
    await expect(confirmation).toBeVisible()
    await confirmation.getByRole('button', {
      name: mode === 'Bypass Permissions' ? '确认切换' : '确认退出', exact: true
    }).click()
    await expect(confirmation).toHaveCount(0)
  }
  let after = before
  await expect.poll(async () => {
    after = Number(await paneForSession(h.page, sid).locator('.terminal-surface').getAttribute('data-pid'))
    return after > 0 && after !== before
  }, { timeout: 40_000 }).toBe(true)
  return { before, after }
}

test.describe('Codex packaged acceptance — real CLI and explicit controlled fixtures', () => {
  test.setTimeout(300_000)

  test('PRD 1–6, 8, 10: lifecycle, HUD, notifications, title, modes, restart and latency 3x', async () => {
    const dirs = await createIsolatedWorkspace('core')
    let h = await launchPackagedApp(dirs)
    try {
      const { sessionId: sid, surface, dagPage } = await launchCodex(h)
      const latency: Array<{ round: number; status: string; runtimeEventAtMs: number; uiObservedAtMs: number; runtimeToUiMs: number }> = []
      for (let round = 1; round <= 3; round++) {
        const seq = readDbRows(h.dataDir, sid).events.at(-1)?.seq ?? 0
        const obs = (await readDagStatusObserver(dagPage)).length
        const prompt = round === 1
          ? 'Remember the codeword opal-harbor-628. Reply only ACK_1.'
          : `Reply only ACK_${round}.`
        await completeTurn(h, sid, surface, prompt, new RegExp(`ACK_${round}`))
        await waitForDagNodeStatus(dagPage, sid, 'status-idle')
        const observed = (await readDagStatusObserver(dagPage)).slice(obs)
        const events = readDbRows(h.dataDir, sid).events.filter((e) => e.seq > seq)
        await saveProof(`latency-round-${round}.json`, { observed,
          events: events.filter((e) => eventWorkStatus(e, sid)).map((e) => ({ ...e, status: eventWorkStatus(e, sid) })) })
        for (const status of ['running', 'idle']) {
          const ev = events.filter((e) => eventWorkStatus(e, sid) === status).at(status === 'idle' ? -1 : 0)
          const ui = observed.filter((o) => o.status === `status-${status}`).at(status === 'idle' ? -1 : 0)
          expect(ev, `round ${round}: missing ${status} runtime event`).toBeTruthy()
          expect(ui, `round ${round}: missing ${status} DOM mutation`).toBeTruthy()
          latency.push({ round, status, runtimeEventAtMs: ev!.occurred_at, uiObservedAtMs: ui!.at,
            runtimeToUiMs: Math.max(0, ui!.at - ev!.occurred_at) })
        }
        await saveProof('latency.json', { clock: 'Date.now, Runtime domain event to DAG DOM mutation; same host', samples: latency })
      }
      await saveProof('latency.json', { clock: 'Date.now, Runtime domain event to DAG DOM mutation; same host', samples: latency })
      expect(latency).toHaveLength(6)
      for (const sample of latency) expect(sample.runtimeToUiMs).toBeLessThanOrEqual(1000)
      const providerId = await waitForProviderSessionId(h.dataDir, sid)
      const hud = hudForSession(h.page, sid)
      await expect(hud).toHaveAttribute('data-hud-mode', 'agent')
      await expect(hud.locator('.status-model')).toContainText(/gpt|GPT/)
      await expect(hud).not.toContainText(/5h|Weekly/)
      await completeTurn(h, sid, surface, 'Read README.md using a shell tool, then reply only TOOL_OK.', /TOOL_OK/)
      await expect(hud.locator('.status-context-ring')).toBeVisible()
      await saveScreenshot(hud, '04-hud.png')
      await hud.getByRole('button', { name: 'AGENTS.md', exact: true }).click()
      const editor = h.page.getByRole('dialog', { name: 'AGENTS.md', exact: true })
      await editor.getByRole('textbox').fill('# Project Instructions\nKeep all changes inside this test repo.\n<!-- Saved by packaged UI -->\n')
      await editor.getByRole('button', { name: '保存', exact: true }).click()
      await expect.poll(() => readFile(join(dirs.repoDir, 'AGENTS.md'), 'utf8')).toContain('Saved by packaged UI')
      await saveScreenshot(h.page, '10-agents-saved.png')

      const pane = paneForSession(h.page, sid)
      await expect.poll(() => pane.locator('.pane-title').innerText(), { timeout: 30_000 }).not.toMatch(/^(Shell|Codex)$/)
      const automaticTitle = await pane.locator('.pane-title').innerText()
      await openPaneMenu(pane)
      await h.page.getByRole('menuitem', { name: '重命名…' }).click()
      await h.page.getByRole('dialog').locator('input').first().fill('手工锁定-Codex')
      await h.page.getByRole('dialog').locator('input').first().press('Enter')
      await completeTurn(h, sid, surface, 'Reply only TITLE_LOCK_OK.', /TITLE_LOCK_OK/)
      await expect(pane.locator('.pane-title')).toHaveText('手工锁定-Codex')
      await openPaneMenu(pane)
      await h.page.getByRole('menuitem', { name: '恢复 Codex 自动标题' }).click()
      await expect(pane.locator('.pane-title')).toHaveText(automaticTitle)
      await saveScreenshot(h.page, '06-title.png')

      // Completion notification while focus is on another genuine card.
      await sendTerminalLine(surface, 'Read README.md with a shell tool and reply only NOTIFY_DONE.')
      const peer = await createSiblingShellAndFocus(h.page, sid)
      await expect(pane).toHaveClass(/has-notification/, { timeout: 150_000 })
      await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
      const center = h.page.getByRole('region', { name: '通知中心', exact: true })
      await expect(center).toContainText('Codex')
      await saveScreenshot(center, '03-completion-notification.png')
      await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
      await pane.click()
      await expect.poll(() => readCurrentWorkStatus(h.dataDir, sid)?.status, { timeout: 150_000 }).toBe('idle')
      await saveProof('core-front.json', { sid, providerId, automaticTitle,
        model: await hud.locator('.status-model').innerText(), peerId: peer.sessionId, turns: h.turnProofs })

      const modes = []
      for (const mode of ['Accept Edits', 'Plan Mode', 'Bypass Permissions', 'Default']) {
        const processChange = await changeMode(h, sid, mode)
        await completeTurn(h, sid, pane.locator('.terminal-surface'),
          'Which codeword did I ask you to remember? Reply only that codeword.', /opal-harbor-628/)
        expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
        const context = (await rolloutEvents(h, providerId)).findLast((e) => e.type === 'turn_context')?.payload
        const expectedSandbox = mode === 'Accept Edits' ? 'workspace-write'
          : mode === 'Bypass Permissions' ? 'danger-full-access' : 'read-only'
        expect(context?.sandbox_policy?.type).toBe(expectedSandbox)
        modes.push({ mode, ...processChange, sandbox: context.sandbox_policy.type, approval: context.approval_policy })
        await saveProof('modes-progress.json', { sid, providerId, modes, turns: h.turnProofs })
      }
      await saveProof('modes-and-core.json', { sid, providerId, peerId: peer.sessionId, modes, automaticTitle,
        actualModel: await hud.locator('.status-model').innerText(), db: readDbRows(h.dataDir, sid) })
      const turnsBeforeRestart = h.turnProofs
      await h.close()
      h = await launchPackagedApp(dirs)
      h.turnProofs = turnsBeforeRestart
      const restored = paneForSession(h.page, sid).locator('.terminal-surface')
      await positivePid(restored)
      await h.page.waitForTimeout(10_000)
      expect(readCurrentWorkStatus(h.dataDir, sid)?.status).toBe('starting')
      await saveScreenshot(h.page, '08-static-restart.png')
      await completeTurn(h, sid, restored, 'Which codeword did I ask you to remember? Reply only that codeword.', /opal-harbor-628/)
      expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
      await expect(paneForSession(h.page, sid).locator('.provider-restore-banner')).toHaveCount(0)
      await saveScreenshot(h.page, '08-restart-memory.png')
      await saveProof('restart.json', { sid, providerId, bindingAfter: readDbRows(h.dataDir, sid).bindings })
    } finally { await dispose(h) }
  })

  test('closure PRD 5: Bypass and Default confirm, respawn and retain a real new-turn memory', async () => {
    const dirs = await createIsolatedWorkspace('modes-closure')
    const h = await launchPackagedApp(dirs)
    try {
      const { sessionId: sid, surface } = await launchCodex(h)
      await completeTurn(h, sid, surface, 'Remember mode codeword river-slate-842. Reply only MODES_READY.', /MODES_READY/)
      const providerId = await waitForProviderSessionId(h.dataDir, sid)
      const modes = []
      for (const mode of ['Bypass Permissions', 'Default']) {
        const processChange = await changeMode(h, sid, mode)
        await completeTurn(h, sid, paneForSession(h.page, sid).locator('.terminal-surface'),
          'Which mode codeword did I ask you to remember? Reply only that codeword.', /river-slate-842/)
        expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
        const context = (await rolloutEvents(h, providerId)).findLast((e) => e.type === 'turn_context')?.payload
        const expectedSandbox = mode === 'Bypass Permissions' ? 'danger-full-access' : 'read-only'
        expect(context?.sandbox_policy?.type).toBe(expectedSandbox)
        expect(context?.approval_policy).toBe(mode === 'Bypass Permissions' ? 'never' : 'on-request')
        modes.push({ mode, ...processChange, sandbox: context.sandbox_policy.type, approval: context.approval_policy })
        await saveProof('closure-modes.json', { sid, providerId, modes, turns: h.turnProofs })
        await saveScreenshot(h.page, mode === 'Bypass Permissions' ? 'closure-05-bypass.png' : 'closure-05-default.png')
      }
    } finally { await dispose(h) }
  })

  test('closure PRD 8: independent restart stays starting for 10 seconds then recalls a codeword', async () => {
    const dirs = await createIsolatedWorkspace('restart-closure')
    let h = await launchPackagedApp(dirs)
    try {
      const { sessionId: sid, surface } = await launchCodex(h)
      await completeTurn(h, sid, surface, 'Remember restart codeword cedar-brook-573. Reply only RESTART_READY.', /RESTART_READY/)
      const providerId = await waitForProviderSessionId(h.dataDir, sid)
      const originalTurn = h.turnProofs.at(-1)!
      const turns = h.turnProofs
      await h.close()
      h = await launchPackagedApp(dirs)
      h.turnProofs = turns
      const pane = paneForSession(h.page, sid)
      const restored = pane.locator('.terminal-surface')
      await positivePid(restored)
      const staticStartAt = Date.now()
      await h.page.waitForTimeout(10_000)
      const staticElapsedMs = Date.now() - staticStartAt
      const staticStatus = readCurrentWorkStatus(h.dataDir, sid)?.status
      expect(staticStatus).toBe('starting')
      // Readiness is the visible DAG status and absence of a restored claim;
      // the existing UI does not promise a restoring banner for every launch.
      const banner = pane.locator('.provider-restore-banner')
      const staticBanner = await banner.count() ? await banner.innerText() : null
      await expect(pane).not.toContainText('已恢复')
      const dag = await openDagWindow(h)
      await waitForDagNodeStatus(dag, sid, 'status-starting')
      await saveScreenshot(h.page, 'closure-08-static-restart.png')
      await saveProof('closure-restart-static.json', { sid, providerId, staticElapsedMs, staticStatus,
        banner: staticBanner, bindingBeforeQuestion: readDbRows(h.dataDir, sid).bindings })
      const prompt = 'Which restart codeword did I ask you to remember? Reply only that codeword.'
      expect(prompt).not.toContain('cedar-brook-573')
      await completeTurn(h, sid, restored, prompt, /cedar-brook-573/)
      expect(h.turnProofs.at(-1)!.turnId).not.toBe(originalTurn.turnId)
      expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
      await expect(pane.locator('.provider-restore-banner')).toHaveCount(0)
      await saveScreenshot(h.page, 'closure-08-recalled-memory.png')
      await saveProof('closure-restart.json', { sid, providerId, staticElapsedMs, staticStatus,
        turns: h.turnProofs, bindingAfter: readDbRows(h.dataDir, sid).bindings })
    } finally { await dispose(h) }
  })

  test('PRD 7, 9: current/new Worktree Fork, parent-child isolation, loader search and resume', async () => {
    const dirs = await createIsolatedWorkspace('fork-load', { secondCommit: true })
    const h = await launchPackagedApp(dirs)
    try {
      const { sessionId: sid, surface, dagPage } = await launchCodex(h)
      await completeTurn(h, sid, surface, 'Remember parent codeword amber-cove-419. Reply only PARENT_OK.', /PARENT_OK/)
      const parentUuid = await waitForProviderSessionId(h.dataDir, sid)
      const children = []
      for (const mode of ['current', 'new'] as const) {
        const parent = paneForSession(h.page, sid)
        await parent.locator('.pane-fork:not(.pane-history):not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)').click()
        await confirmForkDialog(h.page, `Codex-${mode}`, mode)
        let childId = ''
        await expect.poll(async () => {
          childId = (await visibleFocusedSurface(h.page).getAttribute('data-session-id')) ?? ''
          return childId && childId !== sid ? childId : ''
        }, { timeout: 30_000 }).toBeTruthy()
        const child = paneForSession(h.page, childId)
        await expect(child.locator('.fork-progress-overlay')).toHaveCount(0, { timeout: 60_000 })
        const uuid = await waitForProviderSessionId(h.dataDir, childId)
        expect(uuid).not.toBe(parentUuid)
        const metadata = (await rolloutEvents(h, uuid)).find((e) => e.type === 'session_meta')?.payload
        expect(metadata.forked_from_id).toBe(parentUuid)
        if (mode === 'current') expect(metadata.cwd).toBe(dirs.repoDir)
        else expect(metadata.cwd).not.toBe(dirs.repoDir)
        const answer = await completeTurn(h, childId, child.locator('.terminal-surface'),
          'Which codeword did I ask you to remember? Reply only that codeword.', /amber-cove-419/)
        await completeTurn(h, childId, child.locator('.terminal-surface'),
          `Also remember child-only-${mode}-572. Reply only CHILD_OK.`, /CHILD_OK/)
        children.push({ childId, uuid, mode, cwd: metadata.cwd, answer })
        await saveScreenshot(h.page, `09-fork-${mode}.png`)
        await h.page.getByRole('button', { name: '返回父会话' }).click()
        const parentAnswer = await completeTurn(h, sid, parent.locator('.terminal-surface'),
          'List all codewords I asked you to remember. Reply only those codewords.', /amber-cove-419/)
        expect(parentAnswer).not.toContain('child-only-')
        expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(parentUuid)
      }
      await saveScreenshot(dagPage, '09-fork-dag.png')
      // Release parent CLI normally, then load it from a fresh Shell card.
      await sendTerminalLine(surface, '/exit')
      await expect(hudForSession(h.page, sid)).toHaveAttribute('data-hud-mode', 'shell', { timeout: 30_000 })
      const fresh = await createSiblingShellAndFocus(h.page, sid)
      await paneForSession(h.page, fresh.sessionId).locator('.pane-load-session').click()
      const loader = h.page.getByRole('dialog')
      await loader.getByRole('tab', { name: 'Codex', exact: true }).click()
      await loader.locator('[data-search-scope="sessions"] input').fill(parentUuid)
      const item = loader.locator('.session-loader-result-main')
      await expect(item).toHaveCount(1)
      await item.click()
      await expect(loader.locator('.session-loader-events')).toContainText('amber-cove-419')
      await loader.locator('.session-loader-content-search-group input').fill('amber-cove-419')
      await expect(loader.locator('.session-loader-events')).toContainText('amber-cove-419')
      await saveScreenshot(h.page, '07-loader.png')
      await loader.getByRole('button', { name: '载入到当前卡片', exact: true }).click()
      for (const label of ['结束当前运行并载入', '仍然载入']) {
        const button = h.page.getByRole('button', { name: label, exact: true })
        if (await button.isVisible().catch(() => false)) await button.click()
      }
      await expect(loader).toHaveCount(0)
      expect(await waitForProviderSessionId(h.dataDir, fresh.sessionId)).toBe(parentUuid)
      const loadedAnswer = await completeTurn(h, fresh.sessionId, paneForSession(h.page, fresh.sessionId).locator('.terminal-surface'),
        'Which codeword did I ask you to remember? Reply only that codeword.', /amber-cove-419/)
      await saveScreenshot(h.page, '07-loaded-memory.png')
      await saveProof('fork-loader.json', { sid, parentUuid, children, loadedId: fresh.sessionId, loadedAnswer })
    } finally { await dispose(h) }
  })

  test('PRD 1, 11: mt creates a Codex card and real Codex reads its sibling', async () => {
    const dirs = await createIsolatedWorkspace('mt')
    const configPath = join(dirs.rootDir, 'codex-home/config.toml')
    await writeFile(configPath, (await readFile(configPath, 'utf8')).replace('sandbox_mode = \"read-only\"', 'sandbox_mode = \"danger-full-access\"'))
    const h = await launchPackagedApp(dirs)
    try {
      const shell = visibleFocusedSurface(h.page)
      const shellId = (await shell.getAttribute('data-session-id'))!
      await positivePid(shell)
      await waitForEmptyShellPrompt(shell)
      const token = `PEER_${randomBytes(6).toString('hex')}`
      await sendTerminalLine(shell, `printf '%s\\n' '${token}'`)
      await sendTerminalLine(shell, 'mt create session --canvas current --profile codex --enter && mt focus right')
      let sid = ''
      await expect.poll(async () => {
        sid = (await visibleFocusedSurface(h.page).getAttribute('data-session-id')) ?? ''
        return sid && sid !== shellId ? sid : ''
      }, { timeout: 30_000 }).toBeTruthy()
      const surface = paneForSession(h.page, sid).locator('.terminal-surface')
      await expect(surface.locator('.xterm-rows')).toContainText('Codex', { timeout: 30_000 })
      const answer = await completeTurn(h, sid, surface,
        'Use the mt CLI to list cards and read the Shell sibling (mt read left --lines 30). Reply only its PEER_ token. Do not guess the token.', new RegExp(token))
      const uuid = await waitForProviderSessionId(h.dataDir, sid)
      const events = await rolloutEvents(h, uuid)
      const commands = events.filter((e) => e.payload?.type === 'item_completed' && e.payload.item?.type === 'CommandExecution')
        .map((e) => e.payload.item)
      expect(commands.some((item) => item.command?.some((part: string) => /mt read/.test(part)) && item.exit_code === 0 && item.aggregated_output?.includes(token))).toBe(true)
      await saveScreenshot(h.page, '11-mt-cross-card.png')
      await saveProof('mt.json', { shellId, sid, uuid, token, answer })
    } finally { await dispose(h) }
  })

  test('PRD 2, 3: real approval, waiting notification, continue and interrupted turn', async () => {
    const dirs = await createIsolatedWorkspace('approval')
    const h = await launchPackagedApp(dirs)
    try {
      const { sessionId: sid, surface, dagPage } = await launchCodex(h)
      await completeTurn(h, sid, surface, 'Reply only READY_APPROVAL.', /READY_APPROVAL/)
      const target = join(dirs.rootDir, 'approval-marker.txt')
      await sendTerminalLine(surface,
        `Run a shell command with sandbox_permissions=require_escalated to write APPROVED into ${target}. Explicitly request elevated permissions for this command. Do not perform any other writes.`)
      await createSiblingShellAndFocus(h.page, sid)
      await waitForDagNodeStatus(dagPage, sid, 'status-needs-input', 90_000)
      await expect(paneForSession(h.page, sid)).toHaveClass(/has-notification/, { timeout: 15_000 })
      await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
      await expect(h.page.getByRole('region', { name: '通知中心', exact: true })).toContainText('Codex')
      await saveScreenshot(h.page, '02-approval-notification.png')
      await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
      await paneForSession(h.page, sid).click()
      await saveScreenshot(h.page, '02-approval-terminal.png')
      await sendTerminalRawKey(surface, 'Enter')
      await waitForDagNodeStatus(dagPage, sid, 'status-idle', 120_000)
      await expect.poll(() => readFile(target, 'utf8')).toContain('APPROVED')
      await sendTerminalLine(surface, 'Write a very long detailed explanation of prime numbers, at least 3000 words. Do not use tools.')
      await waitForDagNodeStatus(dagPage, sid, 'status-running')
      await h.page.waitForTimeout(2500)
      await sendTerminalRawKey(surface, 'Escape')
      await expect.poll(() => readCurrentWorkStatus(h.dataDir, sid)?.status).toBe('interrupted')
      await saveScreenshot(h.page, '02-interrupted.png')
      await completeTurn(h, sid, surface, 'Reply only AFTER_INTERRUPT_OK.', /AFTER_INTERRUPT_OK/)
      await saveProof('approval-interrupt.json', { sid, events: readDbRows(h.dataDir, sid).events })
    } finally { await dispose(h) }
  })

  test('PRD 2, 8: real missing local rollout, recovery failure, retry and new conversation', async () => {
    const dirs = await createIsolatedWorkspace('recovery-failure')
    let h = await launchPackagedApp(dirs)
    let hidden: { original: string; held: string } | undefined
    try {
      const { sessionId: sid, surface } = await launchCodex(h)
      await completeTurn(h, sid, surface, 'Remember recovery codeword jade-ridge-386. Reply only SAVED.', /SAVED/)
      const uuid = await waitForProviderSessionId(h.dataDir, sid)
      const original = await rolloutFile(h, uuid)
      const held = join(dirs.rootDir, 'held-rollout.jsonl')
      const hideAndRestart = async () => {
        await h.close()
        await rename(original, held)
        hidden = { original, held }
        h = await launchPackagedApp(dirs)
        const banner = paneForSession(h.page, sid).locator('.provider-restore-banner:not(.restoring)')
        await expect(banner).toContainText('恢复失败', { timeout: 90_000 })
        expect(readCurrentWorkStatus(h.dataDir, sid)?.status).toBe('error')
        const dag = await openDagWindow(h)
        await waitForDagNodeStatus(dag, sid, 'status-error')
        await saveScreenshot(dag, '02-real-error-dag.png')
        await h.page.bringToFront()
        await saveScreenshot(h.page, '08-real-recovery-failure.png')
        return banner
      }
      const failed = await hideAndRestart()
      await rename(held, original)
      hidden = undefined
      await failed.getByRole('button', { name: '重试恢复', exact: true }).click()
      await positivePid(paneForSession(h.page, sid).locator('.terminal-surface'))
      await completeTurn(h, sid, paneForSession(h.page, sid).locator('.terminal-surface'),
        'Which recovery codeword did I ask you to remember? Reply only that codeword.', /jade-ridge-386/)
      expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(uuid)
      await saveScreenshot(h.page, '08-real-retry-memory.png')
      const failedAgain = await hideAndRestart()
      await failedAgain.getByRole('button', { name: '新开 Codex', exact: true }).click()
      await positivePid(paneForSession(h.page, sid).locator('.terminal-surface'))
      await completeTurn(h, sid, paneForSession(h.page, sid).locator('.terminal-surface'), 'Reply only FRESH_OK.', /FRESH_OK/)
      const freshUuid = await waitForProviderSessionId(h.dataDir, sid)
      expect(freshUuid).not.toBe(uuid)
      await saveScreenshot(h.page, '08-real-start-fresh.png')
      await saveProof('recovery-failure.json', { sid, originalUuid: uuid, freshUuid,
        stimulus: 'Temporarily moved only this isolated real CLI rollout; no fake hooks or database edits',
        bindingAfter: readDbRows(h.dataDir, sid).bindings })
    } finally {
      await h.close().catch(() => {})
      if (hidden) await rename(hidden.held, hidden.original).catch(() => {})
      await dispose(h)
    }
  })

  test('controlled fixtures: missing CLI and unauthenticated sign-in; no real login is revoked', async () => {
    const missing = await createIsolatedWorkspace('missing')
    let h = await launchPackagedApp(missing, { MATOU_CODEX_COMMAND: join(missing.rootDir, 'does-not-exist') })
    try {
      const surface = visibleFocusedSurface(h.page)
      await positivePid(surface)
      await waitForEmptyShellPrompt(surface)
      await sendTerminalLine(surface, 'mt create session --canvas current --profile codex --enter && mt focus right')
      await expect(h.page.locator('.session-start-failure-card')).toBeVisible({ timeout: 30_000 })
      await expect(h.page.locator('.session-start-failure-card')).toContainText('Codex')
      await saveScreenshot(h.page, 'fixture-missing.png')
      await saveProof('fixture-missing.json', { fixture: 'nonexistent executable', db: readDbRows(h.dataDir) })
    } finally { await dispose(h) }
    const auth = await createIsolatedWorkspace('unauth')
    const stub = join(auth.rootDir, 'codex-signin-fixture')
    await writeFile(stub, '#!/bin/sh\nprintf "Sign in to Codex\\n1. Sign in with ChatGPT\\n2. API key\\n"\nread choice\n')
    await chmod(stub, 0o700)
    h = await launchPackagedApp(auth, { MATOU_CODEX_COMMAND: stub })
    try {
      const surface = visibleFocusedSurface(h.page)
      await positivePid(surface)
      await waitForEmptyShellPrompt(surface)
      await sendTerminalLine(surface, 'codex')
      await expect(surface.locator('.xterm-rows')).toContainText('Sign in to Codex')
      const sid = (await surface.getAttribute('data-session-id'))!
      expect(readDbRows(h.dataDir, sid).bindings.filter((b) => b.resume_state === 'available')).toHaveLength(0)
      await saveScreenshot(h.page, 'fixture-unauthenticated.png')
      await saveProof('fixture-unauthenticated.json', { fixture: 'controlled executable prints sign-in, emits no hooks', sid, db: readDbRows(h.dataDir, sid) })
    } finally { await dispose(h) }
  })
})
