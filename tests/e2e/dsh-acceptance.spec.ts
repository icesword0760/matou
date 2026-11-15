import { execFile } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'
import { chmod, copyFile, mkdir, mkdtemp, readFile, readdir, rm, symlink, unlink, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'
import { expect, test, _electron as electron, type ElectronApplication, type Locator, type Page } from '@playwright/test'
import { DshSessionCatalog, encodeDshProjectPath } from '../../apps/runtime/src/session/dsh-session-catalog'
import { scanDshLog } from '../../apps/runtime/src/session/dsh-session-log-reader'
import { readSessionFrames } from '../../apps/runtime/src/journal/segment-journal'

const exec = promisify(execFile)
const ROOT = resolve(process.env.MATOU_DSH_ACCEPTANCE_ROOT ?? '.cache/dsh-stage5/acceptance')
const EVIDENCE = resolve(process.env.MATOU_DSH_ACCEPTANCE_EVIDENCE ?? '.cache/dsh-stage5/evidence')
const EXECUTABLE = resolve('apps/desktop/release/mac-arm64/码头.app/Contents/MacOS/码头')
const digest = (data: Buffer) => createHash('sha256').update(data).digest('hex')
type Event = { seq: number; event_type: string; payload_json: string; occurred_at: number }
type Dirs = { root: string; repo: string; data: string; realData: string; userData: string; zdot: string; nativeRoot: string; wrapper: string }
type Harness = Dirs & { app: ElectronApplication; page: Page; close(): Promise<void> }
type Sample = { round: number; status: string; runtimeAt: number; nativeAt?: number; uiAt: number; latencyMs: number; nativeToUiMs?: number }
const protectedFiles = new Map<string, string | null>()

async function proof(name: string, value: unknown) {
  await mkdir(EVIDENCE, { recursive: true })
  await writeFile(join(EVIDENCE, name), JSON.stringify(value, null, 2) + '\n')
}
async function screenshot(h: Harness, name: string, page = h.page) {
  await mkdir(EVIDENCE, { recursive: true })
  await page.screenshot({ path: join(EVIDENCE, name) })
}
function sql<T>(h: Dirs, statement: string, ...args: string[]): T[] {
  const db = new DatabaseSync(join(h.data, 'matou.sqlite'), { readOnly: true })
  try { return db.prepare(statement).all(...args) as T[] } finally { db.close() }
}
const events = (h: Dirs, sid: string) => sql<Event>(h, 'SELECT seq,event_type,payload_json,occurred_at FROM domain_events WHERE session_id=? ORDER BY seq', sid)
const status = (h: Dirs, sid: string) => sql<{ work_status: string }>(h, 'SELECT work_status FROM sessions WHERE id=?', sid)[0]?.work_status
const binding = (h: Dirs, sid: string) => sql<{ provider_session_id: string; resume_state: string; metadata_json: string }>(h,
  "SELECT provider_session_id,resume_state,metadata_json FROM provider_bindings WHERE session_id=? AND provider='dsh' ORDER BY updated_at DESC,rowid DESC", sid)[0]
const notices = (h: Dirs, sid: string) => events(h, sid).filter(e => e.event_type === 'agent.notification').map(e => JSON.parse(e.payload_json).event)
function eventStatus(e: Event, sid: string): string | undefined {
  const p = JSON.parse(e.payload_json)
  return p.workStatus ?? p.graph?.nodes?.find((n: any) => n.sessionId === sid)?.workStatus
}
const focused = (page: Page) => page.locator('.scene-stage:not([hidden]) .session-card.is-focused [data-testid="terminal-pane"]:visible .terminal-surface').first()
const pane = (page: Page, sid: string) => page.locator(`[data-session-card="${sid}"] [data-testid="terminal-pane"]:visible`)
const surface = (page: Page, sid: string) => pane(page, sid).locator('.terminal-surface')
const hud = (page: Page, sid: string) => page.locator(`.shortcut-bar .status-info[data-session-id="${sid}"]`)
async function line(target: Locator, text: string) {
  const input = target.locator('.xterm-helper-textarea')
  await input.focus(); await expect(input).toBeFocused()
  await input.pressSequentially(text, { delay: 2 })
  // TUI paste detection treats a trailing Enter in a burst as a literal newline.
  await target.page().waitForTimeout(300)
  await input.press('Enter')
}
async function nativeInputReady(h: Harness, sid: string) {
  await expect(hud(h.page, sid)).toHaveAttribute('data-hud-mode', 'agent')
  await expect.poll(() => status(h, sid), { timeout: 30_000 }).toBe('idle')
  await expect(pane(h.page, sid).locator('.provider-restore-banner')).toHaveCount(0)
  await expect(surface(h.page, sid).locator('.xterm-rows')).not.toContainText(/Context lo(?:ading)?/i, { timeout: 30_000 })
  // The native ready observation precedes TUI input mounting. Let its initial
  // prompt render finish before sending any user keystrokes.
  await h.page.waitForTimeout(1500)
}
async function key(target: Locator, value: string) {
  const input = target.locator('.xterm-helper-textarea'); await input.focus(); await input.press(value)
}
async function dirs(prefix: string, fixture?: 'model-error' | 'resume-gate'): Promise<Dirs> {
  await mkdir(ROOT, { recursive: true })
  const root = await mkdtemp(join(ROOT, prefix + '-'))
  const repo = join(root, 'repo'), realData = join(root, 'data'), userData = join(root, 'electron-user'), zdot = join(root, 'zdot')
  const nativeRoot = join(root, 'native-sessions'), wrapper = join(root, 'dsh-wrapper')
  for (const path of [repo, realData, userData, zdot, nativeRoot]) await mkdir(path)
  await writeFile(join(zdot, '.zshrc'), '')
  await writeFile(join(repo, 'README.md'), '# DSH isolated acceptance\nREAD_TOOL_EVIDENCE_742\n')
  await writeFile(join(repo, 'AGENTS.md'), '# Instructions\nKeep all changes inside this test workspace.\n')
  await exec('git', ['init', '-q', '-b', 'main', repo])
  await exec('git', ['-C', repo, 'add', '.'])
  await exec('git', ['-C', repo, '-c', 'user.name=Matou E2E', '-c', 'user.email=e2e@matou.invalid', 'commit', '-qm', 'Acceptance fixture'])
  const binary = (await exec('/usr/bin/which', ['dsh'])).stdout.trim()
  // Real dsh launcher in every ordinary run. Controlled variants modify only
  // this run's generated overlay or fail resume; user profiles stay untouched.
  await writeFile(wrapper, `#!/usr/bin/env python3\nimport os,sys,pathlib,re\na=sys.argv[1:]\nroot=pathlib.Path(${JSON.stringify(root)})\n` +
    (fixture === 'resume-gate' ? `if '--resume' in a and (root/'block-resume').exists():\n print('Controlled DSH resume failure',flush=True);sys.exit(1)\n` : '') +
    (fixture === 'model-error' ? `if '--patch' in a:\n p=pathlib.Path(a[a.index('--patch')+1]);s=p.read_text()\n if re.search(r'^    model:',s,re.M):s=re.sub(r'^    model:.*$', '    model: matou-intentionally-unavailable-model',s,flags=re.M)\n else:s+='    model: matou-intentionally-unavailable-model\\n'\n p.write_text(s)\n` : '') +
    `os.execv(${JSON.stringify(binary)},[${JSON.stringify(binary)},*a])\n`)
  await chmod(wrapper, 0o700)
  await mkdir('.c5', { recursive: true })
  const data = resolve('.c5', 'd-' + randomBytes(4).toString('hex'))
  await symlink(realData, data)
  return { root, repo, realData, data, userData, zdot, nativeRoot, wrapper }
}
async function launch(d: Dirs, extra: Record<string, string> = {}): Promise<Harness> {
  expect(d.root.startsWith(ROOT + '/')).toBe(true)
  const env = Object.fromEntries(Object.entries(process.env).filter(([, v]) => typeof v === 'string')) as Record<string, string>
  for (const k of Object.keys(env)) if (k.startsWith('MATOU_CONTROL_') || k === 'ELECTRON_RUN_AS_NODE') delete env[k]
  Object.assign(env, { HOME: homedir(), PATH: `/Users/icesword/.local/bin:${env.PATH}`, ZDOTDIR: d.zdot,
    DSH_TUI_SESSION_ROOT: d.nativeRoot, MATOU_DSH_COMMAND: d.wrapper, MATOU_DATA_DIR: d.data,
    MATOU_DEFAULT_WORKSPACE: d.repo, ELECTRON_USER_DATA_DIR: d.userData, MATOU_LOCALE: 'zh-CN',
    MATOU_E2E: '1', MATOU_E2E_DISPLAY: 'primary', MATOU_E2E_WINDOW_CLOSE: 'hide', ...extra })
  const app = await electron.launch({ executablePath: EXECUTABLE, args: [`--user-data-dir=${d.userData}`], env })
  const page = await app.firstWindow(); await page.waitForLoadState('domcontentloaded')
  await page.locator('[data-testid="terminal-pane"]:visible').first().waitFor({ timeout: 30_000 })
  return { ...d, app, page, async close() { await app.close() } }
}
async function dispose(h: Harness) {
  await screenshot(h, `last-${h.root.split('/').at(-1)}.png`).catch(() => {})
  await h.close().catch(() => {}); await unlink(h.data).catch(() => {})
}
async function pid(target: Locator) { await expect(target).toHaveAttribute('data-pid', /[1-9][0-9]*/, { timeout: 30_000 }) }
async function shellReady(target: Locator) { await pid(target); await expect(target.locator('.xterm-rows')).toContainText(/\brepo\s+[%#$]/, { timeout: 30_000 }) }
async function providerId(h: Dirs, sid: string): Promise<string> {
  await expect.poll(() => binding(h, sid)?.provider_session_id, { timeout: 30_000 }).toBeTruthy()
  return binding(h, sid)!.provider_session_id
}
async function rows(h: Dirs, id: string): Promise<any[]> {
  const path = join(h.nativeRoot, encodeDshProjectPath(h.repo), id, 'session.v4.jsonl.zstd')
  const result: any[] = []
  try { for await (const line of scanDshLog(path)) result.push(line.row) }
  catch (e) { if ((e as NodeJS.ErrnoException).code !== 'ENOENT') throw e }
  return result
}
function message(row: any): string {
  const data = row?.data?.message ?? row?.data
  const content = data?.content
  return typeof content === 'string' ? content : Array.isArray(content)
    ? content.filter((b: any) => b.type === 'text').map((b: any) => b.text).join('\n') : ''
}
async function turn(h: Harness, sid: string, prompt: string, answer: RegExp) {
  await nativeInputReady(h, sid)
  const id = await providerId(h, sid), before = (await rows(h, id)).filter(r => r.type === 'turn/end').length
  await line(surface(h.page, sid), prompt)
  let result: { turn?: number; reason?: string; assistant?: string; promptMatched?: boolean } = {}
  await expect.poll(async () => {
    const log = await rows(h, id), ends = log.filter(r => r.type === 'turn/end')
    if (ends.length <= before) return ''
    const end = ends.at(-1), start = log.findLast(r => r.type === 'turn/start' && r.seq < end.seq)
    const part = log.filter(r => r.seq > (start?.seq ?? -1) && r.seq < end.seq)
    const user = part.filter(r => r.type === 'user/message').map(message).join('\n')
    const assistant = part.filter(r => r.type === 'assistant/message').map(message).join('\n')
    result = { turn: end.data?.turn, reason: end.data?.reason?.kind, assistant, promptMatched: user.includes(prompt) }
    return result.reason === 'completed' && result.promptMatched ? assistant : ''
  }, { timeout: 120_000, intervals: [200, 500] }).toMatch(answer)
  await expect.poll(() => status(h, sid), { timeout: 15_000 }).toBe('idle')
  await proof(`turn-${h.root.split('/').at(-1)}-${result.turn}.json`, { sid, id, ...result })
  return result.assistant!
}
async function dag(h: Harness): Promise<Page> {
  const previous = h.app.windows().find(p => p !== h.page && !p.isClosed())
  if (previous) return previous
  const [page] = await Promise.all([h.app.waitForEvent('window'), h.page.getByRole('button', { name: '打开会话 DAG' }).click()])
  await page.waitForLoadState('domcontentloaded'); await page.locator('.dag-node-card').first().waitFor()
  return page
}
async function observe(page: Page, sid: string) {
  await page.evaluate(sid => {
    const w = window as any; w.__dshStatus = []
    const capture = () => {
      const card = document.querySelector(`.dag-node-card[data-session-id="${sid}"]`)
      const state = card && [...card.classList].find(c => c.startsWith('status-'))
      if (state && w.__dshStatus.at(-1)?.status !== state) w.__dshStatus.push({ status: state, at: Date.now() })
    }
    capture(); new MutationObserver(capture).observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ['class'] })
  }, sid)
}
async function observations(page: Page): Promise<Array<{ status: string; at: number }>> { return page.evaluate(() => (window as any).__dshStatus ?? []) }
async function sample(h: Harness, page: Page, sid: string, baseline: number, seen: number, round: number, states: string[]): Promise<Sample[]> {
  const db = events(h, sid).filter(e => e.seq > baseline), ui = (await observations(page)).slice(seen)
  const native = await rows(h, await providerId(h, sid))
  const result = states.map(state => {
    const type = state === 'running' ? 'turn/start' : state === 'needs-input' ? 'approval/asked' : 'turn/end'
    const nativeEvent = native.findLast(r => r.type === type)
    const after = typeof nativeEvent?.time === 'number' ? nativeEvent.time - 10 : 0
    const ev = db.find(e => e.occurred_at >= after && eventStatus(e, sid) === state)
    const dom = ui.find(e => e.at >= after && e.status === 'status-' + state)
    expect(ev, `${state} runtime event in native turn`).toBeTruthy(); expect(dom, `${state} actual DAG mutation in native turn`).toBeTruthy()
    expect(dom!.at).toBeGreaterThanOrEqual(ev!.occurred_at - 10)
    return { round, status: state, runtimeAt: ev!.occurred_at, uiAt: dom!.at, latencyMs: Math.max(0, dom!.at - ev!.occurred_at),
      ...(typeof nativeEvent?.time === 'number' ? { nativeAt: nativeEvent.time, nativeToUiMs: Math.max(0, dom!.at - nativeEvent.time) } : {}) }
  })
  await proof(`latency-${h.root.split('/').at(-1)}-${round}.json`, { result, ui,
    source: 'Same-host Date.now: actual Runtime domain event to actual DAG DOM mutation' })
  for (const row of result) {
    expect(row.latencyMs).toBeLessThanOrEqual(1000)
    if (row.nativeToUiMs !== undefined) expect(row.nativeToUiMs).toBeLessThanOrEqual(1000)
  }
  return result
}
async function start(h: Harness) {
  const target = focused(h.page); await shellReady(target)
  const sid = (await target.getAttribute('data-session-id'))!, d = await dag(h)
  await observe(d, sid); await h.page.bringToFront(); await line(target, 'dsh')
  await providerId(h, sid)
  await expect(hud(h.page, sid)).toHaveAttribute('data-hud-mode', 'agent')
  await expect.poll(() => status(h, sid), { timeout: 30_000 }).toBe('idle')
  await expect(pane(h.page, sid).locator('.pane-title')).toHaveText('DSH')
  await expect(d.locator(`.dag-node-card[data-session-id="${sid}"]`)).toContainText('DSH')
  await nativeInputReady(h, sid)
  return { sid, target: surface(h.page, sid), d }
}
async function sibling(h: Harness, sid: string) {
  await h.page.getByRole('button', { name: '横向新增 Shell', exact: true }).click()
  await expect.poll(async () => focused(h.page).getAttribute('data-session-id')).not.toBe(sid)
  const id = (await focused(h.page).getAttribute('data-session-id'))!; await shellReady(surface(h.page, id))
  return id
}
async function menu(h: Harness, sid: string) {
  await pane(h.page, sid).locator('.terminal-pane-header').click({ button: 'right', position: { x: 72, y: 20 }, force: true })
}
async function assertNoNativeNotifications(h: Dirs, sid: string) {
  const frames = await readSessionFrames(h.data, sid)
  const decoder = new TextDecoder(); let raw = ''
  for (const frame of frames) if (frame.kind === 'output') raw += decoder.decode(frame.data, { stream: true })
  raw += decoder.decode()
  expect(raw.length).toBeGreaterThan(0)
  const count = [...raw.matchAll(/\u001b\](?:9|99|777);/g)].length
  expect(count).toBe(0)
  return count
}

// Protect actual installed configuration before any real-HOME TUI launches.
// Runtime indices, usage and our new sessions are intentionally separate data.
test.beforeAll(async () => {
  await mkdir(EVIDENCE, { recursive: true })
  const profile = join(homedir(), '.dsh/profiles/dsh-tui')
  const paths = (await readdir(profile)).filter(n => /\.(?:ya?ml|json)$/.test(n)).map(n => join(profile, n))
  paths.push(...['settings.json', 'permission.json', 'onboarding.json', 'migrations.json'].map(n => join(homedir(), '.dsh-tui', n)))
  await mkdir(join(ROOT, 'config-backup'), { recursive: true })
  for (const [i, path] of paths.entries()) {
    try { const body = await readFile(path); protectedFiles.set(path, digest(body)); await copyFile(path, join(ROOT, 'config-backup', String(i))) }
    catch (e) { if ((e as NodeJS.ErrnoException).code !== 'ENOENT') throw e; protectedFiles.set(path, null) }
  }
})
test.afterAll(async () => {
  const changes: string[] = []
  for (const [path, before] of protectedFiles) {
    const after = await readFile(path).then(digest, e => { if (e.code === 'ENOENT') return null; throw e })
    if (before !== after) changes.push(path)
  }
  await proof('configuration-check.json', { checked: protectedFiles.size, changed: changes })
  expect(changes).toEqual([])
})

test.describe('DSH packaged acceptance — real native TUI in a PTY and disclosed failure fixtures', () => {
  test.setTimeout(600_000)

  test('PRD 1–6, 8, 11: lifecycle, labels, HUD, title, notifications, static restart and latency', async () => {
    const setup = await dirs('core'); let h = await launch(setup)
    try {
      const { sid, d } = await start(h), latency: Sample[] = [], codeword = 'harbor-' + randomBytes(6).toString('hex')
      await screenshot(h, '01-dsh-label.png'); await screenshot(h, '02-dsh-dag.png', d)
      for (let round = 1; round <= 3; round++) {
        const before = events(h, sid).at(-1)?.seq ?? 0, seen = (await observations(d)).length
        await turn(h, sid, round === 1 ? `Remember codeword ${codeword}. Reply only ACK_1. Do not use tools.` : `Reply only ACK_${round}. Do not use tools.`, new RegExp(`ACK_${round}`))
        latency.push(...await sample(h, d, sid, before, seen, round, ['running', 'idle']))
      }
      await proof('latency.json', { samples: latency, retries: test.info().retry })
      const id = await providerId(h, sid), bar = hud(h.page, sid)
      await turn(h, sid, 'Use the read tool to read README.md, then reply only TOOL_VERIFIED.', /TOOL_VERIFIED/)
      await expect(bar.locator('.status-model')).toContainText(/deepseek/i)
      const nativeChoice = (await rows(h, id)).findLast(r => r.type === 'model/selection')?.data
      if (nativeChoice?.reasoningEffort) await expect(bar.locator('.status-model')).toContainText(nativeChoice.reasoningEffort)
      await expect(bar.locator('.status-context-ring')).toBeVisible()
      await expect(bar.locator('.status-perm-badge')).toHaveText('workspace-write')
      expect(await bar.locator('.status-perm-badge').evaluate(e => e.tagName)).toBe('SPAN')
      await expect(bar.locator('button.status-perm-badge')).toHaveCount(0)
      await expect(bar.locator('.status-instructions')).toHaveCount(0)
      await expect(bar.locator('.ca-trigger, .status-usage, .team-role-badge')).toHaveCount(0)
      await expect(bar).toContainText(/Tools/)
      await expect(bar).toContainText(/repo|main/)
      await expect(pane(h.page, sid).locator('.pane-fork:not(.pane-history):not(.pane-load-session):not(.pane-remove)')).toHaveCount(0)
      await menu(h, sid)
      await expect(h.page.getByRole('menuitem', { name: /账户|供应商|Agent Team|Fork|派生/ })).toHaveCount(0)
      await h.page.keyboard.press('Escape')
      await screenshot(h, '04-dsh-hud.png')
      await expect.poll(() => pane(h.page, sid).locator('.pane-title').innerText(), { timeout: 30_000 }).not.toBe('DSH')
      await menu(h, sid); await h.page.getByRole('menuitem', { name: '重命名…', exact: true }).click()
      const name = h.page.getByRole('dialog').locator('input').first()
      await name.fill('手工锁定-DSH'); await name.press('Enter')
      await line(surface(h.page, sid), '/rename DSH_NATIVE_RENAMED')
      await expect.poll(async () => (await rows(h, id)).some(r => r.type === 'session/title' && r.data?.title === 'DSH_NATIVE_RENAMED')).toBe(true)
      await expect(pane(h.page, sid).locator('.pane-title')).toHaveText('手工锁定-DSH')
      await menu(h, sid); await h.page.getByRole('menuitem', { name: '恢复 DSH 自动标题', exact: true }).click()
      await expect(pane(h.page, sid).locator('.pane-title')).toHaveText('DSH_NATIVE_RENAMED')
      await screenshot(h, '06-title-restored.png')
      const peer = await sibling(h, sid)
      await pane(h.page, sid).click(); const noticeCount = notices(h, sid).filter(n => n.eventType === 'completed').length
      await line(surface(h.page, sid), 'Read README.md and reply only NOTIFY_DSH_DONE.')
      await pane(h.page, peer).click()
      await expect.poll(() => notices(h, sid).filter(n => n.eventType === 'completed').length, { timeout: 120_000 }).toBe(noticeCount + 1)
      await expect(pane(h.page, sid)).toHaveClass(/has-notification/)
      await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
      const center = h.page.getByRole('region', { name: '通知中心', exact: true })
      await expect(center).toContainText('DSH'); await screenshot(h, '03-completion-notification.png')
      await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
      const osc = await assertNoNativeNotifications(h, sid)
      await pane(h.page, sid).click(); await h.close(); h = await launch(setup)
      await pid(surface(h.page, sid)); const idleStarted = Date.now()
      await expect.poll(() => status(h, sid), { timeout: 15_000 }).toBe('idle')
      await h.page.waitForTimeout(1500)
      expect(await providerId(h, sid)).toBe(id)
      await expect(pane(h.page, sid).locator('.provider-restore-banner')).toHaveCount(0)
      await screenshot(h, '08-static-restored.png')
      const response = await turn(h, sid, 'What codeword did I ask you to remember? Reply only that codeword. Do not use tools.', new RegExp(codeword))
      await proof('core.json', { sid, id, codeword, response, staticRestoreMs: Date.now() - idleStarted, nativeNotificationOsc: osc,
        notificationDelta: 1, label: 'DSH', hudModel: await hud(h.page, sid).locator('.status-model').innerText(), retries: test.info().retry })
      await screenshot(h, '08-memory-restored.png')
    } finally { await dispose(h) }
  })

  test('PRD 7, 9: native /fork produces an independent child that Matou previews, searches and loads', async () => {
    const setup = await dirs('fork-loader'), h = await launch(setup)
    try {
      const { sid } = await start(h), word = 'fork-' + randomBytes(6).toString('hex')
      await turn(h, sid, `Remember parent codeword ${word}. Reply only PARENT_SAVED.`, /PARENT_SAVED/)
      const parent = await providerId(h, sid), catalog = new DshSessionCatalog(h.nativeRoot)
      const before = new Set((await catalog.list({ cwd: h.repo, query: '' })).sessions.map(s => s.providerSessionId))
      await line(surface(h.page, sid), '/fork')
      let child = ''
      await expect.poll(async () => {
        child = (await catalog.list({ cwd: h.repo, query: '' })).sessions.find(s => !before.has(s.providerSessionId))?.providerSessionId ?? ''
        return child
      }, { timeout: 30_000 }).toBeTruthy()
      expect(child).not.toBe(parent); expect(await providerId(h, sid)).toBe(parent)
      const childRows = await rows(h, child)
      expect(JSON.stringify(childRows[0])).toContain(parent)
      const fresh = await sibling(h, sid)
      await pane(h.page, fresh).locator('.pane-load-session').click()
      const loader = h.page.getByRole('dialog')
      await loader.getByRole('tab', { name: 'DSH', exact: true }).click()
      await loader.locator('[data-search-scope="sessions"] input').fill(child)
      await expect(loader.locator('.session-loader-result-main')).toHaveCount(1)
      await loader.locator('.session-loader-result-main').click()
      await expect(loader.locator('.session-loader-events')).toContainText(word)
      await loader.locator('.session-loader-content-search-group input').fill(word)
      await expect(loader.locator('.session-loader-events')).toContainText(word)
      await screenshot(h, '07-child-preview-search.png')
      await loader.getByRole('button', { name: '载入到当前卡片', exact: true }).click()
      for (const text of ['结束当前运行并载入', '仍然载入']) {
        const confirm = h.page.getByRole('button', { name: text, exact: true })
        if (await confirm.isVisible().catch(() => false)) await confirm.click()
      }
      await expect(loader).toHaveCount(0); expect(await providerId(h, fresh)).toBe(child)
      const answer = await turn(h, fresh, 'What parent codeword did I ask you to remember? Reply only that codeword.', new RegExp(word))
      await proof('fork-loader.json', { sid, parent, child, loadedCard: fresh, answer, cardForkEntry: false, retries: test.info().retry })
      await screenshot(h, '09-native-child-loaded.png')
    } finally { await dispose(h) }
  })

  test('PRD 1, 10: mt creates a DSH card and real DSH reads its sibling under the default sandbox', async () => {
    const setup = await dirs('mt'), h = await launch(setup)
    try {
      const shell = focused(h.page); await shellReady(shell)
      const original = (await shell.getAttribute('data-session-id'))!, token = 'DSH_PEER_' + randomBytes(6).toString('hex')
      await line(shell, `printf '%s\\n' '${token}'`)
      await line(shell, 'mt create session --canvas current --profile dsh --enter && mt focus right')
      await expect.poll(async () => focused(h.page).getAttribute('data-session-id'), { timeout: 30_000 }).not.toBe(original)
      const sid = (await focused(h.page).getAttribute('data-session-id'))!
      await providerId(h, sid); await expect.poll(() => status(h, sid)).toBe('idle')
      const answer = await turn(h, sid,
        'Use only these mt commands: mt identify --json, mt list --json, then mt read left --json. Reply with the DSH_PEER_ marker returned from the Shell sibling screen. Do not inspect files, change permissions, request escalation or use other tools.', new RegExp(token))
      const id = await providerId(h, sid), log = await rows(h, id)
      expect(log.some(r => r.type === 'tool/result' && JSON.stringify(r.data).includes(token))).toBe(true)
      expect(log.some(r => r.type === 'system/message' && message(r).includes('Matou-managed DSH terminal'))).toBe(true)
      await expect(hud(h.page, sid).locator('.status-perm-badge')).toHaveText('workspace-write')
      await proof('mt.json', { sid, id, original, token, answer, mode: 'workspace-write', retries: test.info().retry })
      await screenshot(h, '10-mt-screen-read.png')
    } finally { await dispose(h) }
  })

  test('PRD 2, 3: real approval request, notification and return to idle', async () => {
    const setup = await dirs('approval'), h = await launch(setup)
    try {
      const { sid, d } = await start(h), peer = await sibling(h, sid), latency: Sample[] = []
      for (let round = 1; round <= 3; round++) {
        await pane(h.page, sid).click(); const seq = events(h, sid).at(-1)?.seq ?? 0, seen = (await observations(d)).length
        await line(surface(h.page, sid), 'Use the bash tool exactly once to run printf MATOU_APPROVAL_OK, with sandbox_permissions set to danger-full-access and justification Matou approval notification probe. This is an approval UI test. Do not use another tool or command. Wait for approval.')
        await pane(h.page, peer).click()
        await expect.poll(() => status(h, sid), { timeout: 90_000 }).toBe('needs-input')
        await expect(d.locator(`.dag-node-card[data-session-id="${sid}"]`)).toHaveClass(/status-needs-input/)
        latency.push(...await sample(h, d, sid, seq, seen, round, ['needs-input']))
        await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
        await expect(h.page.getByRole('region', { name: '通知中心', exact: true })).toContainText('DSH')
        await screenshot(h, `03-approval-${round}.png`)
        await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
        await pane(h.page, sid).click(); await key(surface(h.page, sid), 'Escape')
        await expect.poll(async () => (await rows(h, await providerId(h, sid))).filter(r => r.type === 'approval/decided')).toHaveLength(round)
        // Rejection resolves the approval; interrupt any follow-up model prose
        // through the native TUI so the next sample starts from idle.
        await key(surface(h.page, sid), 'Control+c')
        await expect.poll(() => status(h, sid), { timeout: 30_000 }).toBe('idle')
      }
      await turn(h, sid, 'Reply only APPROVAL_RECOVERED. Do not use tools.', /APPROVAL_RECOVERED/)
      const permissionNotices = notices(h, sid).filter(n => n.eventType === 'permission')
      expect(permissionNotices).toHaveLength(3)
      expect(permissionNotices.every(n => n.title === 'DSH')).toBe(true)
      await proof('approval.json', { sid, latency, permissionNotices, defaultModeChanged: false, nativeNotificationOsc: await assertNoNativeNotifications(h, sid) })
    } finally { await dispose(h) }
  })

  test('PRD 2, 3: real native model error marks UI error and notifies once per turn', async () => {
    const setup = await dirs('model-error', 'model-error'), h = await launch(setup)
    try {
      const { sid, d } = await start(h), peer = await sibling(h, sid), latency: Sample[] = []
      for (let round = 1; round <= 3; round++) {
        await pane(h.page, sid).click(); const seq = events(h, sid).at(-1)?.seq ?? 0, seen = (await observations(d)).length
        await line(surface(h.page, sid), 'Reply only ERROR_PROBE.')
        await pane(h.page, peer).click()
        await expect.poll(() => status(h, sid), { timeout: 60_000 }).toBe('error')
        await expect(d.locator(`.dag-node-card[data-session-id="${sid}"]`)).toHaveClass(/status-error/)
        latency.push(...await sample(h, d, sid, seq, seen, round, ['error']))
        expect(notices(h, sid).filter(n => n.eventType === 'error')).toHaveLength(round)
      }
      const id = await providerId(h, sid), ends = (await rows(h, id)).filter(r => r.type === 'turn/end')
      expect(ends).toHaveLength(3); expect(ends.every(r => r.data?.reason?.kind === 'error')).toBe(true)
      await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
      const center = h.page.getByRole('region', { name: '通知中心', exact: true })
      await expect(center).toContainText('DSH'); await screenshot(h, '03-native-error.png')
      await proof('error.json', { fixture: 'Real DSH with per-run overlay selecting an intentionally unavailable model; no profile write', sid, latency,
        errors: notices(h, sid).filter(n => n.eventType === 'error'), nativeReasons: ends.map(e => e.data.reason.kind) })
    } finally { await dispose(h) }
  })

  test('PRD 8: controlled resume failure retries the same ID and new-start creates a fresh ID', async () => {
    const setup = await dirs('recovery', 'resume-gate'); let h = await launch(setup)
    try {
      const { sid } = await start(h), word = 'retry-' + randomBytes(6).toString('hex')
      await turn(h, sid, `Remember codeword ${word}. Reply only SAVED.`, /SAVED/)
      const id = await providerId(h, sid)
      const failRestart = async () => {
        await h.close(); await writeFile(join(setup.root, 'block-resume'), '')
        h = await launch(setup)
        const banner = pane(h.page, sid).locator('.provider-restore-banner:not(.restoring)')
        await expect(banner).toContainText('恢复失败', { timeout: 35_000 }); return banner
      }
      const first = await failRestart(); await screenshot(h, '08-restore-failed.png')
      await rm(join(setup.root, 'block-resume'))
      await first.getByRole('button', { name: '重试恢复', exact: true }).click()
      await expect.poll(() => status(h, sid), { timeout: 30_000 }).toBe('idle')
      expect(await providerId(h, sid)).toBe(id)
      await turn(h, sid, 'What codeword did I ask you to remember? Reply only that codeword.', new RegExp(word))
      const second = await failRestart()
      await second.getByRole('button', { name: '新开 DSH', exact: true }).click()
      await expect.poll(() => status(h, sid), { timeout: 30_000 }).toBe('idle')
      const fresh = await providerId(h, sid); expect(fresh).not.toBe(id)
      await turn(h, sid, 'Reply only FRESH_DSH_OK.', /FRESH_DSH_OK/)
      await proof('recovery.json', { fixture: 'Launcher exits only --resume while marker exists; all successful runs use real dsh', sid, id, fresh,
        sameIdRetry: true, newIdStart: true, retries: test.info().retry })
      await screenshot(h, '08-retry-new.png')
    } finally { await dispose(h) }
  })

  test('missing CLI: nonexistent command shows actionable installation feedback', async () => {
    const setup = await dirs('missing'), missing = join(setup.root, 'not-installed-dsh')
    const h = await launch(setup, { MATOU_DSH_COMMAND: missing })
    try {
      const target = focused(h.page); await shellReady(target)
      const sid = (await target.getAttribute('data-session-id'))!
      await line(target, 'mt create session --canvas current --profile dsh --enter && mt focus right')
      const failure = h.page.locator('.session-start-failure-card')
      await expect(failure).toBeVisible({ timeout: 30_000 })
      await expect(failure).toContainText('DSH')
      await screenshot(h, 'missing-cli.png')
      await proof('missing.json', { fixture: 'Nonexistent executable', commandExists: false, feedback: await failure.innerText() })
    } finally { await dispose(h) }
  })
})
