// tests/e2e/launch-video/record.spec.ts
// Drives the real Matou app scene by scene and records one clip per narration section with
// ClipRecorder. Every UI action waits for the moment the voice-over reaches the phrase it
// illustrates (see cues.ts), so the clips drop straight onto the narration timeline in Remotion.
//
// Run: cd marketing/launch-video && npm run record   (needs `npm run tts` first and pnpm build at repo root)
// One section at a time: MATOU_SECTIONS=fork-dag,ai-control npm run record
//   The scene is always rebuilt in full; only the selected sections are recorded and their entries
//   are merged into the existing public/recordings/manifest.json.
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { expect, test, type ElectronApplication, type Locator, type Page } from '@playwright/test'
import * as scene from '../readme-capture/demo-scene'
import type { Ids } from '../readme-capture/demo-scene'
import { ClipRecorder } from './recorder'
import { cueTime, loadCues, phraseTime } from './cues'

const OUT = resolve(import.meta.dirname, '../../../marketing/launch-video/public/recordings')
const VIDEO_WINDOW = { width: 1504, height: 846 }
const TAIL_MS = 1500
const ZOOM = 1
// intro and outro are composed in Remotion from stills; they still get an (empty) manifest entry.
const ORDER = ['intro', 'why', 'structure', 'focus', 'persist', 'fork-dag', 'ai-control',
  'board-notify', 'model-switch', 'outro'] as const
const IMPL_CARDS = ['实现 · Redis 幂等键', '回归 · 支付模块测试', '审查 · 方案对比', '文档 · 回调约定', '协调 · 跨卡片']
// The card the AI batch-forks from, on its own canvas so that canvas's DAG is just it and its three
// children. The 方案探索 baseline tree keeps its own discussion card under a different name.
const DISCUSS = '幂等方案 · 讨论'
const PLAN_CARD = '幂等方案 · 三条路线'
// What the fork-dag section renames its new child to, so the DAG shows plan names rather than the
// branch name the fork dialog gives a fresh card.
const FORK_CHILD = '方案 C · 去重表'
const IMPL_TAB = '实现与验证'
const PLAN_TAB = '方案探索'
const FORK_TAB = 'AI 分叉'

test.setTimeout(0)

type Clip = { file: string; events: string; startAtMs: number; durationMs: number }
type Manifest = { fps: number; scale: number; viewport: typeof VIDEO_WINDOW; sections: Array<{ id: string; clips: Clip[] }> }
const recorded = new Map<string, Clip[]>()

const selected = (process.env.MATOU_SECTIONS ?? '').split(',').map((id) => id.trim()).filter(Boolean)
function wanted(id: string): boolean { return selected.length === 0 || selected.includes(id) }

interface Scene {
  app: ElectronApplication
  page: Page
  root: string; home: string; workspace: string; mobile: string; demo: string
  implIds: Ids[]
  planBIds: Ids
  vitestIds: Ids
  mobileIds: Ids
  discussIds: Ids
  forkerIds: Ids
}

// The ffmpeg child lives in the Electron main process, so the recorder must be stopped even when
// the section throws - otherwise it keeps writing frames into the next section's app evaluate calls
// and the mp4 is never finalised. A clip whose body failed is still flushed to disk, but it is not
// returned, so `writeManifest` never lists it.
async function recordClip(host: Scene, sectionId: string, part: string | undefined, startAtMs: number,
  body: (rec: ClipRecorder) => Promise<void>): Promise<Clip> {
  const name = part ? `${sectionId}-${part}` : sectionId
  const rec = new ClipRecorder(host.app, 30)
  await rec.start(join(OUT, `${name}.mp4`))
  let failure: unknown
  try {
    await body(rec)
  } catch (error) {
    failure = error
  }
  const stats = await rec.stop(join(OUT, `${name}.events.json`))
  if (failure !== undefined) {
    console.log(`clip ${name}: FAILED after ${stats.stoppedAt - stats.startedAt} ms, not written to the manifest`)
    throw failure
  }
  const clip: Clip = {
    file: `recordings/${name}.mp4`, events: `recordings/${name}.events.json`,
    startAtMs, durationMs: stats.stoppedAt - stats.startedAt
  }
  console.log(`clip ${name}: ${clip.durationMs} ms, dropped ${stats.dropped}, resized ${stats.resized}`)
  expect(stats.dropped).toBe(0)
  return clip
}

function addSection(id: string, ...clips: Clip[]): void { recorded.set(id, clips) }

// ---------- shared helpers ----------

// The stub `claude` pops one role per launch, so every section that starts a session declares the
// roles it consumes right before it runs. Trailing spares keep an unexpected launch (a provider
// switch restarting a session, say) from emptying the queue mid-recording.
async function queueRoles(demo: string, ...roles: string[]): Promise<void> {
  await writeFile(join(demo, 'roles.queue'), [...roles, ...Array(6).fill('baseline')].join('\n') + '\n')
}

function surfaceOf(page: Page, sessionId: string): Locator {
  return page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
}

const NOTIFICATIONS = (host: Scene): Array<{ ids: Ids; eventType: string; title: string; subtitle: string; body: string }> => [
  { ids: host.mobileIds, eventType: 'completed', title: '崩溃修复 · iOS 18', subtitle: '任务完成',
    body: '修复已提交到 fix/ios18-crash，12 个 XCTest 全部通过。' },
  { ids: host.implIds[2]!, eventType: 'completed', title: '审查 · 方案对比', subtitle: '任务完成',
    body: 'ADR 已写入 docs/adr/0007，建议方案 A 为主、方案 B 兜底。' },
  { ids: host.implIds[1]!, eventType: 'waiting', title: '回归 · 支付模块测试', subtitle: '等待输入',
    body: '1 个用例与新行为冲突，需要确认是否更新断言。' },
  { ids: host.planBIds, eventType: 'waiting', title: '方案 B · DB 唯一索引', subtitle: '等待输入',
    body: '迁移失败：历史数据有 37 条重复 event_id，是否先写清洗脚本？' },
  { ids: host.vitestIds, eventType: 'error', title: '回归 · vitest', subtitle: '出错',
    body: 'vitest 退出码 1：webhook.duplicate.test.ts 有 1 个用例失败。' }
]

async function notify(page: Page, ids: Ids,
  input: { eventType: string; title: string; subtitle: string; body: string }): Promise<void> {
  await page.evaluate(({ ids, input }) => {
    const bridge = (window as unknown as {
      matouE2e?: { pushNotification(payload: Record<string, unknown>): void }
    }).matouE2e
    if (!bridge) throw new Error('Matou E2E bridge is missing')
    bridge.pushNotification({ ...input, ...ids, sound: false })
  }, { ids, input: { ...input, eventId: `launch-${input.title}-${Date.now()}` } })
}

async function closeDag(host: Scene): Promise<void> {
  await host.app.evaluate(({ BrowserWindow }) => {
    BrowserWindow.getAllWindows().find((w) => w.webContents.getURL().includes('kind=dag'))?.close()
  })
  await expect.poll(async () => (await host.app.windows()).length).toBe(1)
}

// The burned-in captions sit 64px above the bottom of the 1080p frame and a single line is about
// 74px tall, so the bottom 138 output pixels are subtitle. In this window's CSS pixels that is the
// bottom ~108, and everything the DAG shot wants to show has to live above it.
const CAPTION_BAND_CSS = Math.round(((64 + 74) * VIDEO_WINDOW.height) / 1080)
const SAFE_BOTTOM_CSS = VIDEO_WINDOW.height - CAPTION_BAND_CSS
/** Keeps the framing off both edges rather than flush against them. */
const DAG_MARGIN_CSS = 8

/** Bounding box of every node card, in the DAG window's CSS pixels. */
async function dagBox(dag: Page): Promise<{ top: number; bottom: number; height: number }> {
  return dag.evaluate(() => {
    const rects = [...document.querySelectorAll('.dag-node-card')].map((node) => node.getBoundingClientRect())
    const top = Math.min(...rects.map((r) => r.top))
    const bottom = Math.max(...rects.map((r) => r.bottom))
    return { top, bottom, height: bottom - top }
  })
}

// Drags the canvas background up by `by` CSS px. The press has to land on empty canvas: pressing on
// a node card drags (or opens) the node instead of panning the graph.
async function panDagUp(dag: Page, by: number): Promise<void> {
  const from = await dag.evaluate((by) => {
    const rects = [...document.querySelectorAll('.dag-node-card')].map((node) => node.getBoundingClientRect())
    const free = (x: number, y: number) =>
      rects.every((r) => x < r.left - 12 || x > r.right + 12 || y < r.top - 12 || y > r.bottom + 12)
    for (const x of [24, window.innerWidth - 24, 64, window.innerWidth - 64]) {
      for (let y = window.innerHeight - 40; y > by + 40; y -= 20) {
        if (free(x, y)) return { x, y }
      }
    }
    return null
  }, by)
  if (!from) {
    console.log('dag pan skipped: no empty canvas to drag from')
    return
  }
  await dag.mouse.move(from.x, from.y)
  await dag.mouse.down()
  await dag.mouse.move(from.x, from.y - by, { steps: 12 })
  await dag.mouse.up()
}

/**
 * Fits the whole graph into the part of the frame the captions do not cover.
 *
 * `centerDagGraph` puts the node bounding box in the middle of the window, which leaves the lowest
 * card sitting under the subtitles. Panning up is enough only while the graph is shorter than the
 * safe area - the six-node 方案探索 canvas is taller than it, and lifting it far enough to clear
 * the captions pushed the 方案 A card (the first one the narration talks about) off the top of the
 * window. So: zoom out a step at a time until the graph fits, re-centring after each step, then
 * lift it by exactly what is left, never past the top edge.
 */
async function frameDagGraph(dag: Page): Promise<void> {
  for (let step = 0; step < 3; step += 1) {
    if ((await dagBox(dag)).height <= SAFE_BOTTOM_CSS - 2 * DAG_MARGIN_CSS) break
    await dag.getByRole('button', { name: '缩小' }).click()
    await dag.waitForTimeout(250)
    await scene.centerDagGraph(dag)
  }
  const box = await dagBox(dag)
  const lift = Math.min(
    Math.max(0, Math.round(box.bottom - SAFE_BOTTOM_CSS + DAG_MARGIN_CSS)),
    Math.max(0, Math.round(box.top - DAG_MARGIN_CSS))
  )
  console.log(`dag framing: graph ${Math.round(box.height)}px, top ${Math.round(box.top)}, ` +
    `bottom ${Math.round(box.bottom)}, safe bottom ${SAFE_BOTTOM_CSS}, lifting ${lift}`)
  if (lift > 0) await panDagUp(dag, lift)
}

async function openDag(host: Scene, rec: ClipRecorder, label: string): Promise<Page> {
  await rec.click(host.page.getByRole('button', { name: '打开会话 DAG' }), label)
  await expect.poll(async () => (await host.app.windows()).length, { timeout: 30_000 }).toBe(2)
  const dag = (await host.app.windows()).find((candidate) => candidate !== host.page)!
  await expect(dag.locator('.dag-node-card').first()).toBeVisible({ timeout: 30_000 })
  await scene.alignDagWindow(host.app, ZOOM)
  await rec.setSource('dag')
  await dag.waitForTimeout(300)
  await scene.centerDagGraph(dag)
  await frameDagGraph(dag)
  return dag
}

// Tabs are addressed by name: a restart can reorder the tab bar, so positional locators would
// silently pick the wrong canvas. A canvas that is still restoring also swallows the first click,
// so keep asking until the tab reports itself selected.
function tabOf(page: Page, name: string): Locator {
  return page.getByRole('tab', { name, exact: true })
}

async function selectTab(page: Page, name: string): Promise<void> {
  const tab = tabOf(page, name)
  await expect.poll(async () => {
    if (await tab.getAttribute('aria-selected') !== 'true') await tab.click({ timeout: 10_000 }).catch(() => {})
    return tab.getAttribute('aria-selected')
  }, { timeout: 60_000, intervals: [500] }).toBe('true')
}

// A restored canvas comes back at whatever level its remembered focus sits on, which after a
// restart can be a fork's child level rather than the canvas root a section expects. Climb until
// the named card is on the strip - the breadcrumb itself can take a beat to render, so this polls
// rather than checking once.
async function climbToCard(page: Page, title: string): Promise<void> {
  const card = page.locator('.scene-stage:not([hidden]) .pane-title', { hasText: title })
  await expect.poll(async () => {
    if (await card.count() === 0) {
      const back = page.getByRole('button', { name: '返回父会话' })
      if (await back.count() > 0) await back.click().catch(() => {})
      await page.waitForTimeout(700)
    }
    return card.count()
  }, { timeout: 90_000, intervals: [700] }).toBeGreaterThan(0)
}

// Focuses a card by its title, for the moments after a restart when a card's terminal may not be
// mounted yet and the session-id locator therefore resolves to nothing.
function paneByTitle(page: Page, title: string): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]:visible')
    .filter({ has: page.locator('.pane-title', { hasText: title }) }).first()
}

async function focusCardByTitle(page: Page, title: string): Promise<void> {
  const card = paneByTitle(page, title)
  await expect(card).toBeVisible({ timeout: 60_000 })
  await card.scrollIntoViewIfNeeded()
  await card.click({ position: { x: 12, y: 12 } })
  await expect(card).toHaveAttribute('data-active', 'true', { timeout: 30_000 })
}

async function visibleTitles(page: Page): Promise<string> {
  return (await page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]:visible .pane-title')
    .allInnerTexts()).join(' | ')
}

// Runs the clip past the end of its narration by TAIL_MS. The extra hold absorbs timer jitter, so
// a clip can never stop a few milliseconds short of the timeline it has to cover.
async function runToTail(rec: ClipRecorder, untilMs: number): Promise<void> {
  await rec.waitUntil(untilMs)
  await rec.hold(150)
}

/** Keeps a pointer target inside the recorded window, with room for the cursor sprite. */
function clampToWindow(value: number, extent: number, margin = 28): number {
  return Math.min(extent - margin, Math.max(margin, value))
}

async function centerOf(locator: Locator): Promise<{ x: number; y: number }> {
  const box = await locator.boundingBox()
  if (!box) throw new Error(`no bounding box for ${locator.toString()}`)
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 }
}

// The carousel expands and scrolls to whichever card the pointer crosses, so a click computed
// before the pointer travels can land beside a small header button. Take the trip first, let the
// hover animation settle, then let rec.click recompute the box for the final short hop.
async function hoverThenClick(rec: ClipRecorder, page: Page, target: Locator, label: string): Promise<void> {
  const approach = await centerOf(target)
  await rec.moveTo(page, approach.x, approach.y, 420)
  await page.waitForTimeout(700)
  await rec.click(target, label)
}

interface PullPlan { x: number; y: number; scrollLeft: number; width: number; right: number; on: string }

// Finds a spot the parent-pull gesture can start from. SessionCard activates itself on
// pointer-down capture and the pane header is an HTML5 drag source, so the press has to land on
// the carousel's own padding or a card slot — anything else either re-focuses a card (which
// re-lays out the strip mid-gesture) or starts a window-detach drag instead.
async function probePull(carousel: Locator): Promise<PullPlan | null> {
  return carousel.evaluate((element) => {
    const rect = element.getBoundingClientRect()
    const blocked = '.terminal-surface,button,input,textarea,select,a,[role="menuitem"],[draggable="true"]'
    const candidates: Array<{ x: number; y: number }> = []
    for (const dy of [4, 8, 12, 18, 26, 36, rect.height - 6, rect.height - 12, rect.height - 20]) {
      for (let fraction = 0.04; fraction < 0.7; fraction += 0.02) {
        candidates.push({ x: Math.round(rect.left + rect.width * fraction), y: Math.round(rect.top + dy) })
      }
    }
    const shape = { scrollLeft: element.scrollLeft, width: element.clientWidth, right: rect.right }
    let fallback: { x: number; y: number; on: string } | null = null
    for (const { x, y } of candidates) {
      const hit = document.elementFromPoint(x, y) as HTMLElement | null
      if (!hit || !(element === hit || element.contains(hit))) continue
      const on = `${hit.tagName.toLowerCase()}.${String(hit.className).split(' ')[0] ?? ''}`
      if (hit === element || hit.classList.contains('session-card-slot')) return { x, y, on, ...shape }
      if (!hit.closest(blocked) && !hit.closest('.session-card:not(.is-focused)')) {
        fallback ??= { x, y, on }
      }
    }
    return fallback === null ? null : { ...fallback, ...shape }
  })
}

// The parent-return gesture (SessionCarousel.pointerDown/Move/End): press a non-interactive spot of
// the strip while it sits at its left edge, drag right past parentPullThreshold(viewportWidth) and
// release. Falls back to the breadcrumb button when the gesture cannot be armed.
async function pullToParent(rec: ClipRecorder, page: Page): Promise<'gesture' | 'button'> {
  const carousel = page.locator('.scene-stage:not([hidden]) .session-carousel').first()
  const parentButton = page.getByRole('button', { name: '返回父会话' })
  let plan = await probePull(carousel)
  if (plan && plan.scrollLeft > 1) {
    // Only a gesture that *starts* at the left edge is read as a parent pull, so one ordinary drag
    // brings the strip there first.
    await rec.moveTo(page, plan.x, plan.y, 300)
    await page.mouse.down()
    await page.mouse.move(plan.x + Math.min(plan.scrollLeft + 60, 600), plan.y, { steps: 14 })
    await page.mouse.up()
    await page.waitForTimeout(400)
    plan = await probePull(carousel)
  }
  console.log('pull plan', JSON.stringify(plan))
  if (plan && plan.scrollLeft <= 1) {
    const travel = Math.max(284, Math.min(340, plan.width * 0.4)) + 110
    rec.mark('pull-start')
    await rec.moveTo(page, plan.x, plan.y, 260)
    await page.mouse.down()
    await rec.moveTo(page, Math.min(plan.x + travel, plan.right - 12), plan.y, 900)
    console.log('parent preview visible', await page.locator('.parent-projection').count())
    // The controller only commits 450 ms after the preview is fully exposed.
    await rec.hold(560)
    await page.mouse.up()
    rec.mark('pull-end')
    const committed = await expect.poll(() => parentButton.count(), { timeout: 4_000 }).toBe(0)
      .then(() => true).catch(() => false)
    if (committed) return 'gesture'
  }
  if (await parentButton.count()) {
    await rec.click(parentButton, 'back')
    return 'button'
  }
  return 'gesture'
}

// ---------- scene ----------

async function buildScene(root: string): Promise<Scene> {
  const home = join(root, 'home')
  const workspace = join(home, 'work', 'shop-api')
  const mobile = join(home, 'work', 'mobile-app')
  const demo = join(root, 'demo')
  await scene.prepareHome(home, demo)
  await scene.prepareShopPlatform(workspace)
  await scene.prepareRepo(mobile, { 'README.md': '# mobile-app\n' })
  await scene.prepareDemo(demo)
  await scene.seedClaudeHistory(home, workspace)
  // Scene-building launches, in the order this spec consumes them.
  await queueRoles(demo, 'implementation', 'regression', 'review', 'docs', 'coordinate',
    'planA1', 'planB1', 'baseline', 'planA', 'planB', 'baseline-three', 'ai-fork')

  const app = await scene.launch({ root, home, workspace, demo })
  const page = await app.firstWindow()
  await scene.placeWindow(app, ZOOM, VIDEO_WINDOW)
  console.log('viewport', await page.evaluate(() => `${window.innerWidth}x${window.innerHeight} @${window.devicePixelRatio}`))

  // ----- shop-api: seven tasks -----
  await expect(page.getByTestId('active-task')).toHaveText('默认')
  await scene.renameTask(page, '默认', '支付回调幂等性')
  for (const title of ['订单列表分页超时', 'Prisma 6 升级', '首页 LCP 优化', '结算页 500 热修', '登录页 A/B 实验', 'CI 缓存修复']) {
    await page.getByRole('button', { name: '在 shop-api 中新增事项' }).click()
    await scene.renameTask(page, '新事项', title)
    await scene.waitForShell(scene.activeSurface(page))
    if (title === '订单列表分页超时') {
      await page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(scene.visibleSurfaces(page)).toHaveCount(2)
    }
  }
  await scene.selectTask(page, '支付回调幂等性')

  // ----- canvas 1: 实现与验证 -----
  await scene.renameActiveTab(page, '实现与验证')
  const implSurfaces: Locator[] = [await scene.stableSurface(scene.visibleSurfaces(page).first())]
  for (let index = 1; index < IMPL_CARDS.length; index += 1) {
    implSurfaces.push(await scene.newSurfaceAfter(page, () => page.getByRole('button', { name: '横向新增 Shell' }).click()))
  }
  const implIds: Ids[] = []
  for (const [index, title] of IMPL_CARDS.entries()) {
    const surface = implSurfaces[index]!
    await scene.waitForShell(surface)
    await scene.promoteToClaude(surface, demo)
    await scene.renameSession(page, surface, title)
    implIds.push(await scene.hierarchyIds(page, surface))
  }
  // A subtree under the review card so this canvas has depth in the DAG too.
  const implPlanA = await scene.newSurfaceAfter(page, () =>
    scene.forkChild(page, scene.paneOf(implSurfaces[2]!), IMPL_CARDS[2]!, 'impl-redis'))
  await scene.waitForRole(demo, 'planA1')
  await scene.renameSession(page, implPlanA, '方案 A · Redis SETNX')
  const implPlanB = await scene.newSurfaceAfter(page, () =>
    scene.forkSibling(page, scene.paneOf(implPlanA), '方案 A · Redis SETNX', 'impl-unique-index'))
  await scene.waitForRole(demo, 'planB1')
  await scene.renameSession(page, implPlanB, '方案 B · DB 唯一索引')
  await implPlanA.click({ position: { x: 12, y: 12 } })
  const implVitest = await scene.newSurfaceAfter(page, () => page.getByRole('button', { name: '横向新增 Shell' }).click())
  await scene.waitForShell(implVitest)
  await scene.terminalCommand(implVitest, 'pnpm vitest run src/payments')
  await page.waitForTimeout(600)
  await scene.renameSession(page, implVitest, '回归 · vitest')
  await page.getByRole('button', { name: '返回父会话' }).click()
  await expect(scene.visibleSurfaces(page)).toHaveCount(IMPL_CARDS.length)

  // ----- canvas 2: 方案探索 (baseline + two forks + a derived shell + the discussion card) -----
  await page.getByRole('button', { name: '新建页签' }).click()
  await expect(page.getByRole('tab')).toHaveCount(2)
  await scene.renameActiveTab(page, '方案探索')
  const baseline = await scene.stableSurface(scene.visibleSurfaces(page).first())
  await scene.promoteToClaude(baseline, demo)
  await scene.renameSession(page, baseline, '支付回调幂等性 · 基线')
  const planA = await scene.newSurfaceAfter(page, () =>
    scene.forkChild(page, scene.paneOf(baseline), '支付回调幂等性 · 基线', 'idem-redis'))
  await scene.waitForRole(demo, 'planA')
  await scene.renameSession(page, planA, '方案 A · Redis SETNX')
  const planB = await scene.newSurfaceAfter(page, () =>
    scene.forkSibling(page, scene.paneOf(planA), '方案 A · Redis SETNX', 'idem-unique-index'))
  await scene.waitForRole(demo, 'planB')
  await scene.renameSession(page, planB, '方案 B · DB 唯一索引')
  const planBIds = await scene.hierarchyIds(page, planB)
  await planA.click({ position: { x: 12, y: 12 } })
  const derived = await scene.newSurfaceAfter(page, () => page.getByRole('button', { name: '横向新增 Shell' }).click())
  await scene.waitForShell(derived)
  await scene.terminalCommand(derived, 'pnpm vitest run src/payments')
  await page.waitForTimeout(600)
  await scene.renameSession(page, derived, '回归 · vitest')
  const vitestIds = await scene.hierarchyIds(page, derived)
  // Back to the canvas root so the discussion card is a sibling of the baseline, not of the forks.
  await page.getByRole('button', { name: '返回父会话' }).click()
  await expect(scene.visibleSurfaces(page)).toHaveCount(1)
  const discuss = await scene.newSurfaceAfter(page, () => page.getByRole('button', { name: '横向新增 Shell' }).click())
  await scene.waitForShell(discuss)
  await scene.promoteToClaude(discuss, demo)
  await scene.waitForRole(demo, 'baseline-three')
  await scene.renameSession(page, discuss, PLAN_CARD)
  const discussIds = await scene.hierarchyIds(page, discuss)

  // ----- canvas 3: AI 分叉 (one card, so its DAG is exactly the batch fork) -----
  await page.getByRole('button', { name: '新建页签' }).click()
  await expect(page.getByRole('tab')).toHaveCount(3)
  await scene.renameActiveTab(page, FORK_TAB)
  const forker = await scene.stableSurface(scene.visibleSurfaces(page).first())
  await scene.waitForShell(forker)
  await scene.renameSession(page, forker, DISCUSS)
  // `mt fork children self` is refused until the calling conversation has completed one
  // prompt→Stop cycle, and the ai-fork stub runs the command in the middle of its first cycle.
  // Priming the card here (the batch fails, the Stop still lands and makes the binding
  // fork-capable) lets the recorded run succeed. `exit` hands the card back to its Shell and the
  // next `claude` clears the screen, so the recording starts from an ordinary empty terminal.
  await scene.promoteToClaude(forker, demo)
  await scene.waitForRole(demo, 'ai-fork')
  await page.waitForTimeout(2500)
  const forkerIds = await scene.hierarchyIds(page, forker)
  await scene.terminalCommand(forker, 'exit')
  await expect(forker).not.toHaveAttribute('data-profile', 'claude-code', { timeout: 60_000 })
  // Let the Shell finish taking the terminal back: typing into it while the stub is still exiting
  // gets the first keystroke echoed twice.
  await page.waitForTimeout(2_000)
  // Leading space: the Shell echoes the first keystroke twice right after the stub exits, and
  // "  clear" survives that where "cclear" would not.
  await scene.terminalCommand(forker, ' clear')
  await page.waitForTimeout(600)

  // ----- mobile-app workspace: one finished task for a cross-workspace notification -----
  await app.evaluate(({ ipcMain }, selectedPath) => {
    const channel = 'matou:select-workspace-directory'
    ipcMain.removeHandler(channel)
    ipcMain.handle(channel, () => selectedPath)
  }, mobile)
  await page.getByRole('button', { name: '新增工作空间' }).click()
  await expect(page.locator('.workspace-group.is-active')).toContainText('mobile-app')
  await scene.renameTask(page, '默认', '崩溃修复 · iOS 18')
  await scene.waitForShell(scene.activeSurface(page))
  const mobileIds = await scene.hierarchyIds(page, scene.activeSurface(page))

  // back to shop-api / 支付回调幂等性 / 实现与验证
  await page.locator('.workspace-group', { hasText: 'shop-api' }).locator('.workspace-group__toggle').click()
  await scene.selectTask(page, '支付回调幂等性')
  await selectTab(page, IMPL_TAB)
  await expect(scene.visibleSurfaces(page)).toHaveCount(IMPL_CARDS.length)

  const host: Scene = {
    app, page, root, home, workspace, mobile, demo,
    implIds, planBIds, vitestIds, mobileIds, discussIds, forkerIds
  }
  await resetNotifications(host)
  // Hero state: the implementation card focused, the pointer parked out of the way.
  await scene.focusCard(surfaceOf(page, implIds[0]!.sessionId))
  await page.mouse.move(5, 500)
  await page.waitForTimeout(600)
  return host
}

// Notifications live in the renderer, so a restart clears them; every section that shows them
// re-pushes the same curated set first.
async function resetNotifications(host: Scene): Promise<void> {
  await host.page.getByRole('button', { name: '通知中心' }).click()
  const clear = host.page.getByRole('button', { name: '清空通知' })
  if (await clear.isVisible().catch(() => false)) await clear.click()
  await host.page.getByRole('button', { name: '关闭通知中心' }).click()
  for (const entry of NOTIFICATIONS(host)) {
    await notify(host.page, entry.ids, entry)
    await host.page.waitForTimeout(700)
  }
}

async function restartApp(host: Scene): Promise<void> {
  await host.app.evaluate(({ app: electronApp }) => { electronApp.quit() }).catch(() => {})
  await host.app.close().catch(() => {})
  host.app = await scene.launch({ root: host.root, home: host.home, workspace: host.workspace, demo: host.demo })
  host.page = await host.app.firstWindow()
  await scene.placeWindow(host.app, ZOOM, VIDEO_WINDOW)
  await expect(scene.visibleSurfaces(host.page).first()).toBeVisible({ timeout: 180_000 })
  // Only 实现与验证 is filmed after the restart. 方案探索 is deliberately left untouched: a canvas
  // relaunches its Claude cards the moment it comes on screen, and those restores are slow and
  // occasionally stall, which is why persist is recorded after every section that needs that canvas.
  // The app also comes back on whichever canvas and level it was left on, so climb to the root of
  // this one rather than assuming it.
  await selectTab(host.page, IMPL_TAB)
  await host.page.waitForTimeout(1200)
  await climbToCard(host.page, IMPL_CARDS[0]!)
  for (const title of IMPL_CARDS) {
    await expect(host.page.locator('.scene-stage:not([hidden]) .pane-title', { hasText: title }))
      .toBeVisible({ timeout: 90_000 })
  }
  await focusCardByTitle(host.page, IMPL_CARDS[0]!)
  // How many of the restored cards mount a live terminal (rather than their compact summary) is
  // the app's call, so give the restores a moment to settle and film whatever comes back.
  await host.page.waitForTimeout(8_000)
  console.log('restored 实现与验证', await scene.visibleSurfaces(host.page).count(),
    'live terminals of', await visibleTitles(host.page))
  await host.page.mouse.move(5, 500)
}

// ---------- sections ----------

// Nothing happens on screen: the hero canvas simply reads, while the pointer drifts across the five
// cards so the eye is led over the whole board.
async function recordWhy(host: Scene): Promise<Clip> {
  const { cues, durationMs } = await loadCues('why')
  const page = host.page
  return recordClip(host, 'why', undefined, 0, async (rec) => {
    rec.mark('overview')
    const start = cueTime(cues, '市面上不缺好用的终端工具')
    const step = (durationMs - start) / IMPL_CARDS.length
    for (let index = 0; index < IMPL_CARDS.length; index += 1) {
      await rec.waitUntil(start + index * step)
      // Measured here, not up front: the carousel expands and scrolls to whichever card the
      // pointer crosses, so boxes taken before the sweep starts describe a layout that no longer
      // exists - the last two cards had already slid past the right edge of the window by the time
      // the pointer was sent to them. A card that is still only half on screen when its turn comes
      // has its centre clamped into the window, so the pointer sweeps across the strip instead of
      // walking off the edge of the frame.
      const point = await centerOf(scene.visibleSurfaces(page).nth(index))
      await rec.moveTo(page, clampToWindow(point.x, VIDEO_WINDOW.width), clampToWindow(point.y, VIDEO_WINDOW.height), 1400)
    }
    await runToTail(rec, durationMs + TAIL_MS)
  })
}

// The four-layer tour. Its middle cue packs workspace / task / canvas / card into one sentence, so
// the actions are spaced by phraseTime inside that cue rather than by its start.
async function recordStructure(host: Scene): Promise<Clip> {
  const { cues, durationMs } = await loadCues('structure')
  const page = host.page
  return recordClip(host, 'structure', undefined, 0, async (rec) => {
    const toggle = page.locator('.workspace-group.is-active .workspace-group__toggle')
    await rec.waitUntil(phraseTime(cues, '工作空间对应'))
    await rec.click(toggle, 'workspace')
    await rec.hold(1100)
    await rec.click(toggle, 'workspace-expand')

    await rec.waitUntil(phraseTime(cues, '事项对应'))
    await rec.click(page.locator('.workspace-group.is-active .workbench-item', { hasText: '支付回调幂等性' }).first(), 'task')

    await rec.waitUntil(phraseTime(cues, '画布是事项里的阶段'))
    await rec.click(tabOf(page, PLAN_TAB), 'tab')
    await rec.hold(1400)
    await rec.click(tabOf(page, IMPL_TAB), 'tab')
    await expect(scene.visibleSurfaces(page)).toHaveCount(IMPL_CARDS.length)

    await rec.waitUntil(phraseTime(cues, '每张卡片是'))
    await rec.click(scene.visibleSurfaces(page).nth(1), 'card', { x: 12, y: 12 })

    await rec.waitUntil(cueTime(cues, '每张卡片下面有一条'))
    rec.mark('hud')
    await rec.moveTo(page, 300, VIDEO_WINDOW.height - 16, 900)

    await rec.waitUntil(cueTime(cues, '切换卡片'))
    await rec.click(scene.visibleSurfaces(page).nth(0), 'card', { x: 12, y: 12 })
    await rec.hold(1300)
    await rec.click(scene.visibleSurfaces(page).nth(2), 'card', { x: 12, y: 12 })
    await runToTail(rec, durationMs + TAIL_MS)
  })
}

async function recordFocus(host: Scene): Promise<Clip> {
  const { cues, durationMs } = await loadCues('focus')
  const page = host.page
  return recordClip(host, 'focus', undefined, 0, async (rec) => {
    const at = phraseTime(cues, '你聚焦哪张')
    for (const [step, index] of [1, 3, 0, 2].entries()) {
      await rec.waitUntil(at + step * 1200)
      await rec.click(scene.visibleSurfaces(page).nth(index), 'card', { x: 12, y: 12 })
    }
    await runToTail(rec, durationMs + TAIL_MS)
  })
}

// Two clips around a real app restart: the recorder's ffmpeg child lives inside the Electron main
// process, so it has to be stopped before the app quits.
async function recordPersist(host: Scene): Promise<Clip[]> {
  const { cues, durationMs } = await loadCues('persist')
  // Back to the hero state - earlier sections leave the notification centre open, another canvas
  // selected or another card focused.
  const closeNotify = host.page.getByRole('button', { name: '关闭通知中心' })
  if (await closeNotify.count() > 0) await closeNotify.click().catch(() => {})
  await selectTab(host.page, IMPL_TAB)
  await climbToCard(host.page, IMPL_CARDS[0]!)
  await focusCardByTitle(host.page, IMPL_CARDS[0]!)
  await host.page.mouse.move(5, 500)
  await host.page.waitForTimeout(800)
  const restartAt = cueTime(cues, '关掉再打开')
  const clipA = await recordClip(host, 'persist', 'a', 0, async (rec) => {
    rec.mark('before-restart')
    await rec.moveTo(host.page, 700, 420, 1200)
    await rec.waitUntil(Math.max(0, restartAt - 300))
  })
  // A restored card asks the stub for its own conversation by name, so restores no longer draw from
  // the queue. The only launch here that still does is the catalog load below.
  await queueRoles(host.demo, 'baseline')
  await restartApp(host)
  console.log('restore launches', await launchedRoles(host.demo))
  const startAtMs = clipA.durationMs
  return [clipA, await recordClip(host, 'persist', 'b', startAtMs, async (rec) => {
    const page = host.page
    rec.mark('restored')
    await rec.waitUntil(cueTime(cues, '想接着一个以前的') - startAtMs)
    const shell = await scene.newSurfaceAfter(page, () =>
      rec.click(page.getByRole('button', { name: '横向新增 Shell' }), 'new-shell'))
    await scene.waitForShell(shell)
    await hoverThenClick(rec, page, scene.paneOf(shell).getByRole('button', { name: /载入 Claude Code 会话到/ }), 'load')
    const dialog = page.getByRole('dialog', { name: '载入 Claude Code 会话' })
    await expect(dialog).toBeVisible({ timeout: 30_000 })

    await rec.waitUntil(phraseTime(cues, '左边是历史会话') - startAtMs)
    await rec.click(dialog.getByRole('button', { name: /预览会话：支付回调幂等键设计/ }), 'pick')

    await rec.waitUntil(phraseTime(cues, '点一下就载入') - startAtMs)
    await rec.click(dialog.getByRole('button', { name: '载入到当前卡片' }), 'load-confirm')
    const confirmRunning = dialog.getByRole('button', { name: '结束当前运行并载入' })
    if (await confirmRunning.isVisible().catch(() => false)) await rec.click(confirmRunning, 'load-running')
    await runToTail(rec, durationMs + TAIL_MS - startAtMs)
  })]
}

// Fork one child off the discussion card, open the DAG, read the whole canvas from it, jump into a
// node and drag back to the parent.
async function recordForkDag(host: Scene): Promise<Clip> {
  const { cues, durationMs } = await loadCues('fork-dag')
  await queueRoles(host.demo, 'planA')
  return recordClip(host, 'fork-dag', undefined, 0, async (rec) => {
    const page = host.page
    await rec.click(tabOf(page, PLAN_TAB), 'tab')
    // The card is addressed through its pane rather than its terminal: right after a restart the
    // terminal may not be mounted yet, while the pane (and its Fork button) always is.
    await climbToCard(page, PLAN_CARD)
    const discussPane = paneByTitle(page, PLAN_CARD)
    await rec.click(discussPane, 'card', { x: 12, y: 12 })

    // hoverThenClick spends ~1.5 s travelling and letting the strip settle, so start the approach
    // early enough that the click itself lands on the words "点一下 Fork".
    await rec.waitUntil(cueTime(cues, '点一下 Fork', -1600))
    const child = await scene.newSurfaceAfter(page, async () => {
      const forkButton = discussPane.getByRole('button', { name: `从“${PLAN_CARD}”创建子分支` })
      await expect(forkButton).not.toHaveAttribute('aria-disabled', 'true', { timeout: 90_000 })
      await hoverThenClick(rec, page, forkButton, 'fork')
      const branch = page.getByLabel('分支名称')
      if (await branch.count() === 0) {
        // Log the retry too: two clicks really happened, and the events file is what the cursor
        // overlay is drawn from, so it must show both rather than only the one that missed.
        console.log('fork retry: the recorded click missed the button')
        await rec.click(forkButton, 'fork')
      }
      await scene.fillForkDialog(page, 'idem/plan-a')
    })
    // A forked card is named after its branch, so the DAG opened a few seconds later would show
    // "idem/plan-a" among cards named for what they actually do. Name it like its siblings - this
    // canvas is the three-plan discussion, and this is the third plan.
    await scene.renameSession(page, child, FORK_CHILD)

    await rec.waitUntil(phraseTime(cues, '按 Option 加 Tab'))
    const dag = await openDag(host, rec, 'dag')

    const nodes = ['方案 A · Redis SETNX', '方案 B · DB 唯一索引', '回归 · vitest']
    const phrases = ['方案 A 还在跑', '方案 B 在等我拍板', '回归挂了']
    for (const [index, title] of nodes.entries()) {
      await rec.waitUntil(phraseTime(cues, phrases[index]!))
      const point = await centerOf(dag.getByRole('button', { name: `打开会话：${title}` }))
      await rec.moveTo(dag, point.x, point.y, 700)
    }

    await rec.waitUntil(cueTime(cues, '点节点直接落到'))
    await rec.click(dag.getByRole('button', { name: '打开会话：方案 B · DB 唯一索引' }), 'dag-node')
    await expect.poll(async () => (await host.app.windows()).length, { timeout: 30_000 }).toBe(1)
    await rec.setSource('main')

    await rec.waitUntil(phraseTime(cues, '往父方向一拖'))
    console.log('parent return via', await pullToParent(rec, page))

    await rec.waitUntil(phraseTime(cues, '会闪一个蓝框'))
    // The pull lands on the canvas root, where 方案 B is no longer on screen; the discussion card
    // next to us is what can actually flash.
    rec.mark('flash')
    await notify(page, host.discussIds, { eventType: 'waiting', title: PLAN_CARD, subtitle: '等待输入',
      body: '方案 1 已经跑通，要不要把方案 3 也开一条线？' })
    await runToTail(rec, durationMs + TAIL_MS)
  })
}

// Two live `mt` calls from inside a stub Claude: reading the neighbouring card, then creating three
// child cards in one shot.
async function recordAiControl(host: Scene): Promise<Clip> {
  const { cues, durationMs } = await loadCues('ai-control')
  // The reader, the forker, then the three child cards it creates if they start themselves.
  await queueRoles(host.demo, 'ai-read', 'ai-fork', 'planA', 'planB', 'baseline')
  return recordClip(host, 'ai-control', undefined, 0, async (rec) => {
    const page = host.page
    await rec.click(tabOf(page, IMPL_TAB), 'tab')

    await rec.waitUntil(phraseTime(cues, '你可以直接说'))
    await rec.click(scene.visibleSurfaces(page).last(), 'card', { x: 12, y: 12 })
    const reader = await scene.newSurfaceAfter(page, () =>
      rec.click(page.getByRole('button', { name: '横向新增 Shell' }), 'new-shell'))
    await scene.waitForShell(reader)
    await scene.promoteToClaude(reader, host.demo)
    await scene.waitForRole(host.demo, 'ai-read')
    rec.mark('mt-read')

    await rec.waitUntil(cueTime(cues, '再进一步'))
    await rec.click(tabOf(page, FORK_TAB), 'tab')
    const forker = surfaceOf(page, host.forkerIds.sessionId)
    await rec.click(forker, 'card', { x: 12, y: 12 })
    await scene.waitForShell(forker)

    await rec.waitUntil(phraseTime(cues, '为这三个方案各开一张子卡片'))
    await scene.promoteToClaude(forker, host.demo)
    rec.mark('mt-fork')
    const children = scene.paneOf(forker).getByRole('button', { name: '查看 3 个子会话' })
    await expect(children).toBeVisible({ timeout: 60_000 }).catch(async (error: Error) => {
      console.log('forker screen', await forker.locator('.xterm-rows').innerText().catch(() => '?'))
      throw error
    })
    await hoverThenClick(rec, page, children, 'children')
    await expect(page.getByLabel('会话：方案 3 · 去重表')).toBeVisible({ timeout: 30_000 })

    await rec.waitUntil(phraseTime(cues, 'DAG 上同时长出三条线'))
    const dag = await openDag(host, rec, 'dag')
    await expect(dag.locator('.dag-node-card')).toHaveCount(4, { timeout: 30_000 })
    await runToTail(rec, durationMs + TAIL_MS)
    // Hand the next section a clean slate: close the DAG inside this clip (by landing on the node,
    // which is the gesture the fork-dag section already taught) and go back to the main window, so
    // the section is self-contained rather than leaving a second window open behind it.
    await rec.click(dag.getByRole('button', { name: `打开会话：${DISCUSS}` }), 'dag-node')
    await expect.poll(async () => (await host.app.windows()).length, { timeout: 30_000 }).toBe(1)
    await rec.setSource('main')
    await rec.hold(800)
  })
}

async function recordBoardNotify(host: Scene): Promise<Clip> {
  const { cues, durationMs } = await loadCues('board-notify')
  if ((await host.app.windows()).length > 1) await closeDag(host)
  await selectTab(host.page, IMPL_TAB)
  // persist adds a sixth card to this canvas, so anchor on a card that is always there.
  await expect(surfaceOf(host.page, host.implIds[0]!.sessionId)).toBeVisible({ timeout: 30_000 })
  await resetNotifications(host)
  // Spread the columns before recording: the moves are persisted by the runtime, so the board opens
  // already arranged and the only drag on camera is the narrated one.
  await host.page.getByRole('button', { name: '看板' }).click()
  await expect(host.page.getByRole('region', { name: 'shop-api 看板' })).toBeVisible()
  for (const [title, column] of [['订单列表分页超时', '运行中'], ['结算页 500 热修', '阻塞'],
    ['登录页 A/B 实验', '完成'], ['CI 缓存修复', '完成']] as const) {
    await scene.moveTask(host.page, title, column)
  }
  await expect(host.page.locator('.board-feedback')).toHaveCount(0, { timeout: 5_000 })
  await host.page.getByRole('button', { name: '看板' }).click()
  await host.page.mouse.move(5, 500)
  return recordClip(host, 'board-notify', undefined, 0, async (rec) => {
    const page = host.page
    await rec.waitUntil(cueTime(cues, '每个工作空间都有一个看板'))
    await rec.click(page.getByRole('button', { name: '看板' }), 'board')
    await expect(page.getByRole('region', { name: 'shop-api 看板' })).toBeVisible()
    await expect(page.locator('section[aria-label="运行中列"] article.board-task-card[aria-label="订单列表分页超时"]'))
      .toBeVisible({ timeout: 15_000 })

    await rec.waitUntil(phraseTime(cues, '拖一下就行'))
    const card = await centerOf(page.locator('article.board-task-card[aria-label="支付回调幂等性"]'))
    await rec.moveTo(page, card.x, card.y, 500)
    rec.mark('drag-start')
    const column = await centerOf(page.locator('section[aria-label="阻塞列"]'))
    await rec.moveTo(page, column.x, card.y, 800)
    await scene.moveTask(page, '支付回调幂等性', '阻塞')
    rec.mark('drag-end')

    await rec.waitUntil(cueTime(cues, '通知按来源'))
    await rec.click(page.getByRole('button', { name: '看板' }), 'board-close')
    await rec.click(page.getByRole('button', { name: '通知中心' }), 'notify')
    await expect(page.getByRole('region', { name: '通知中心' })).toBeVisible()

    await rec.waitUntil(phraseTime(cues, '点一下回到事发现场'))
    await rec.click(page.getByRole('button', { name: /打开通知：1 个用例与新行为冲突/ }), 'notify-open')
    await rec.moveTo(page, 5, 500, 500)
    await runToTail(rec, durationMs + TAIL_MS)
  })
}

async function recordModelSwitch(host: Scene): Promise<Clip> {
  const { cues, durationMs } = await loadCues('model-switch')
  // Activating a provider restarts the Claude sessions it updates; each one asks for its own
  // conversation by name, except the card holding the loaded catalog session.
  await queueRoles(host.demo, 'baseline')
  return recordClip(host, 'model-switch', undefined, 0, async (rec) => {
    const page = host.page
    await rec.waitUntil(cueTime(cues, '码头内置了供应商切换'))
    await rec.click(page.getByRole('button', { name: '设置', exact: true }), 'settings')
    await expect(page.getByRole('region', { name: '模型切换设置' })).toBeVisible()

    // Open the dialog just before the narration says "fill in the address and the key".
    await rec.waitUntil(phraseTime(cues, '填上地址和密钥', -1500))
    await rec.click(page.getByRole('button', { name: '新增供应商' }), 'add-provider')
    const dialog = page.getByRole('dialog', { name: '新增供应商' })
    await expect(dialog).toBeVisible()
    await dialog.getByLabel('供应商名称').fill('DeepSeek')
    await dialog.getByLabel('默认模型').fill('deepseek-v4-pro')
    await dialog.getByLabel('API 地址').fill('https://api.deepseek.com/anthropic')
    await dialog.getByLabel('API Key').fill('sk-demo-not-a-real-key')
    await rec.click(dialog.getByRole('button', { name: '保存配置' }), 'save')
    await expect(dialog).toHaveCount(0, { timeout: 30_000 })

    await rec.waitUntil(phraseTime(cues, '随时切'))
    await rec.click(page.getByRole('button', { name: '切换到 DeepSeek' }), 'switch')
    await rec.hold(1500)
    await rec.click(page.getByRole('button', { name: '关闭设置' }), 'close-settings')
    await runToTail(rec, durationMs + TAIL_MS)
  })
}

// ---------- manifest ----------

async function writeManifest(): Promise<void> {
  await mkdir(OUT, { recursive: true })
  const previous = await readFile(join(OUT, 'manifest.json'), 'utf8')
    .then((text) => JSON.parse(text) as Manifest).catch(() => undefined)
  const manifest: Manifest = { fps: 30, scale: 2, viewport: VIDEO_WINDOW, sections: [] }
  for (const id of ORDER) {
    const kept = previous?.sections.find((section) => section.id === id)?.clips ?? []
    manifest.sections.push({ id, clips: recorded.get(id) ?? (id === 'intro' || id === 'outro' ? [] : kept) })
  }
  await writeFile(join(OUT, 'manifest.json'), JSON.stringify(manifest, null, 2))
  console.log('manifest', manifest.sections.map((s) => `${s.id}:${s.clips.length}`).join(' '))
}

async function launchedRoles(demo: string): Promise<string> {
  const log = await readFile(join(demo, 'launches.log'), 'utf8').catch(() => '')
  return log.split('\n').filter(Boolean).map((line) => JSON.parse(line).role).join(',')
}

// ---------- test ----------

test('records every narration section of the launch video', async () => {
  test.skip(process.env.MATOU_LAUNCH_VIDEO !== '1', 'set MATOU_LAUNCH_VIDEO=1')
  const root = await mkdtemp(join(tmpdir(), 'matou-launch-'))
  await mkdir(OUT, { recursive: true })
  const host = await buildScene(root)
  try {
    console.log('scene built with launches', await launchedRoles(host.demo))
    // Recording order is not timeline order (the manifest is keyed by section id). persist restarts
    // the app and model-switch swaps the provider under every running session, so both destructive
    // sections run after the ones that need the freshly built scene.
    if (wanted('why')) addSection('why', await recordWhy(host))
    if (wanted('structure')) addSection('structure', await recordStructure(host))
    if (wanted('focus')) addSection('focus', await recordFocus(host))
    if (wanted('fork-dag')) addSection('fork-dag', await recordForkDag(host))
    if (wanted('ai-control')) addSection('ai-control', await recordAiControl(host))
    if (wanted('board-notify')) addSection('board-notify', await recordBoardNotify(host))
    if (wanted('persist')) addSection('persist', ...await recordPersist(host))
    if (wanted('model-switch')) addSection('model-switch', await recordModelSwitch(host))
    console.log('all launches', await launchedRoles(host.demo))
  } finally {
    // Written even when a section throws, so the sections already in the can stay usable and the
    // run can be resumed with MATOU_SECTIONS for the rest.
    await writeManifest()
    await host.app.evaluate(({ app: electronApp }) => { electronApp.quit() }).catch(() => {})
    await host.app.close().catch(() => {})
    if (!process.env.MATOU_KEEP_DEMO_ROOT) await rm(root, { recursive: true, force: true })
    else console.log(`demo root kept at ${root}`)
  }
})
