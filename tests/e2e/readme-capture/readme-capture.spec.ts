// Rebuilds the README demo scene with isolated fixtures and captures assets/shots/*.png.
// Run: pnpm build && MATOU_README_CAPTURE=1 npx playwright test tests/e2e/readme-capture --workers=1
// Demo mode: MATOU_DEMO_HOLD=1 builds the same scene and keeps the app open (for recording); add
// MATOU_E2E_DISPLAY=primary to place it on the main display instead of the built-in one.
// The window is placed on the secondary (built-in) display like the other e2e specs.
// Every session is a stub `claude` (claude-stub.py); nothing touches the real CLI or account.
import { _electron as electron, expect, test, type ElectronApplication, type Locator, type Page } from '@playwright/test'
import { execFile } from 'node:child_process'
import { chmod, cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'
import { transcripts, widestLine } from './transcripts'

const run = promisify(execFile)
const REPO = resolve(import.meta.dirname, '../../..')
const SHOTS = process.env.MATOU_SHOTS_DIR ?? join(REPO, 'assets', 'shots')
// At 1400 CSS px the canvas stage stays under the four-column threshold, so the carousel shows the
// focused card plus one sibling at natural font size; capturePage() records the display's Retina pixels.
const ZOOM = 1
const WINDOW = { width: 1400, height: 880 }
const BOARD_HEIGHT = 640

const HOLD = process.env.MATOU_DEMO_HOLD === '1'
test.setTimeout(HOLD ? 0 : 300_000)

type Ids = { workspaceId: string; taskId: string; sceneId: string; sessionId: string }

test('captures README screenshots', async () => {
  test.skip(process.env.MATOU_README_CAPTURE !== '1' && !HOLD,
    'set MATOU_README_CAPTURE=1 to regenerate assets/shots, or MATOU_DEMO_HOLD=1 to keep the demo app open')
  const root = await mkdtemp(join(tmpdir(), 'matou-readme-'))
  const home = join(root, 'home')
  const workspace = join(home, 'work', 'shop-api')
  const mobile = join(home, 'work', 'mobile-app')
  const demo = join(root, 'demo')
  await prepareHome(home, demo)
  await prepareShopPlatform(workspace)
  await prepareRepo(mobile, { 'README.md': '# mobile-app\n' })
  await prepareDemo(demo)

  const app = await launch({ root, home, workspace, demo })
  try {
    const page = await app.firstWindow()
    await placeWindow(app, ZOOM)
    console.log('viewport', await page.evaluate(() => `${window.innerWidth}x${window.innerHeight} @${window.devicePixelRatio}`))
    const stage = stageRecorder(page, root)

    // ----- shop-api: tasks -----
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await renameTask(page, '默认', '支付回调幂等性')
    for (const title of ['订单列表分页超时', 'Prisma 6 升级', '首页 LCP 优化', '结算页 500 热修', '登录页 A/B 实验', 'CI 缓存修复']) {
      await page.getByRole('button', { name: '在 shop-api 中新增事项' }).click()
      await renameTask(page, '新事项', title)
      await waitForShell(activeSurface(page))
      if (title === '订单列表分页超时') {
        await page.getByRole('button', { name: '横向新增 Shell' }).click()
        await expect(visibleSurfaces(page)).toHaveCount(2)
      }
    }
    await selectTask(page, '支付回调幂等性')
    await stage('01-tasks')

    // ----- scene 1: implementation / regression / review -----
    await renameActiveTab(page, '实现与验证')
    const cards = [
      '实现 · Redis 幂等键', '回归 · 支付模块测试', '审查 · 方案对比', '文档 · 回调约定', '协调 · 跨卡片'
    ]
    const sceneOneSurfaces: Locator[] = [await stableSurface(visibleSurfaces(page).first())]
    for (let index = 1; index < cards.length; index += 1) {
      sceneOneSurfaces.push(await newSurfaceAfter(page, () => page.getByRole('button', { name: '横向新增 Shell' }).click()))
    }
    const sceneOneIds: Ids[] = []
    for (const [index, title] of cards.entries()) {
      const surface = sceneOneSurfaces[index]!
      await waitForShell(surface)
      await promoteToClaude(surface, demo)
      await renameSession(page, surface, title)
      sceneOneIds.push(await hierarchyIds(page, surface))
    }
    // A subtree under the review card so this canvas's DAG has depth too: the comparison forks into
    // plan A / plan B, plus a derived shell running the regression. (Forking needs a finished turn,
    // which the review role has and the still-running implementation role does not.)
    const implPlanA = await newSurfaceAfter(page, () =>
      forkChild(page, paneOf(sceneOneSurfaces[2]!), cards[2]!, 'impl-redis'))
    await waitForRole(demo, 'planA1')
    await renameSession(page, implPlanA, '方案 A · Redis SETNX')
    const implPlanB = await newSurfaceAfter(page, () =>
      forkSibling(page, paneOf(implPlanA), '方案 A · Redis SETNX', 'impl-unique-index'))
    await waitForRole(demo, 'planB1')
    await renameSession(page, implPlanB, '方案 B · DB 唯一索引')
    await implPlanA.click({ position: { x: 12, y: 12 } })
    const implVitest = await newSurfaceAfter(page, () => page.getByRole('button', { name: '横向新增 Shell' }).click())
    await waitForShell(implVitest)
    await terminalCommand(implVitest, 'pnpm vitest run src/payments')
    await page.waitForTimeout(600)
    await renameSession(page, implVitest, '回归 · vitest')
    await page.getByRole('button', { name: '返回父会话' }).click()
    await expect(visibleSurfaces(page)).toHaveCount(cards.length)
    await stage('02-scene-one')

    // ----- scene 2: baseline with two forks and one derived shell (for the DAG) -----
    await page.getByRole('button', { name: '新建页签' }).click()
    await expect(page.getByRole('tab')).toHaveCount(2)
    await renameActiveTab(page, '方案探索')
    const baseline = await stableSurface(visibleSurfaces(page).first())
    await promoteToClaude(baseline, demo)
    await renameSession(page, baseline, '支付回调幂等性 · 基线')
    const planA = await newSurfaceAfter(page, () =>
      forkChild(page, paneOf(baseline), '支付回调幂等性 · 基线', 'idem-redis'))
    await waitForRole(demo, 'planA')
    await stage('03a-fork-child')
    await renameSession(page, planA, '方案 A · Redis SETNX')
    const planB = await newSurfaceAfter(page, () =>
      forkSibling(page, paneOf(planA), '方案 A · Redis SETNX', 'idem-unique-index'))
    await waitForRole(demo, 'planB')
    await stage('03b-fork-sibling')
    await renameSession(page, planB, '方案 B · DB 唯一索引')
    await planA.click({ position: { x: 12, y: 12 } })
    const derived = await newSurfaceAfter(page, () => page.getByRole('button', { name: '横向新增 Shell' }).click())
    await waitForShell(derived)
    await terminalCommand(derived, 'pnpm vitest run src/payments')
    await page.waitForTimeout(600)
    await renameSession(page, derived, '回归 · vitest')
    const planBIds = await hierarchyIds(page, planB)
    const vitestIds = await hierarchyIds(page, derived)
    await stage('03-scene-two')

    // ----- mobile-app workspace: one finished task for a cross-workspace notification -----
    await app.evaluate(({ ipcMain }, selectedPath) => {
      const channel = 'matou:select-workspace-directory'
      ipcMain.removeHandler(channel)
      ipcMain.handle(channel, () => selectedPath)
    }, mobile)
    await page.getByRole('button', { name: '新增工作空间' }).click()
    await expect(page.locator('.workspace-group.is-active')).toContainText('mobile-app')
    await renameTask(page, '默认', '崩溃修复 · iOS 18')
    await waitForShell(activeSurface(page))
    const mobileIds = await hierarchyIds(page, activeSurface(page))

    // back to shop-api / 支付回调幂等性 / scene 1
    await page.locator('.workspace-group', { hasText: 'shop-api' }).locator('.workspace-group__toggle').click()
    await selectTask(page, '支付回调幂等性')
    await page.getByRole('tab').first().click()
    await expect(visibleSurfaces(page)).toHaveCount(cards.length)

    // ----- notifications: drop the hook-generated ones, push curated Chinese ones -----
    await page.getByRole('button', { name: '通知中心' }).click()
    const clear = page.getByRole('button', { name: '清空通知' })
    if (await clear.isVisible()) await clear.click()
    await page.getByRole('button', { name: '关闭通知中心' }).click()
    const notify = async (ids: Ids, input: { eventType: string; title: string; subtitle: string; body: string }, ago: number) => {
      await page.evaluate(({ ids, input }) => {
        if (!window.matouE2e) throw new Error('Matou E2E bridge is missing')
        window.matouE2e.pushNotification({ ...input, ...ids, sound: false })
      }, { ids, input: { ...input, eventId: `readme-${input.title}` } })
      await page.waitForTimeout(ago)
    }
    await notify(mobileIds, { eventType: 'completed', title: '崩溃修复 · iOS 18', subtitle: '任务完成',
      body: '修复已提交到 fix/ios18-crash，12 个 XCTest 全部通过。' }, 1200)
    await notify(sceneOneIds[2]!, { eventType: 'completed', title: '审查 · 方案对比', subtitle: '任务完成',
      body: 'ADR 已写入 docs/adr/0007，建议方案 A 为主、方案 B 兜底。' }, 1200)
    await notify(sceneOneIds[1]!, { eventType: 'waiting', title: '回归 · 支付模块测试', subtitle: '等待输入',
      body: '1 个用例与新行为冲突，需要确认是否更新断言。' }, 1200)
    await notify(planBIds, { eventType: 'waiting', title: '方案 B · DB 唯一索引', subtitle: '等待输入',
      body: '迁移失败：历史数据有 37 条重复 event_id，是否先写清洗脚本？' }, 1200)
    await notify(vitestIds, { eventType: 'error', title: '回归 · vitest', subtitle: '出错',
      body: 'vitest 退出码 1：webhook.duplicate.test.ts 有 1 个用例失败。' }, 300)

    // focus the implementation card so the HUD shows the running session
    await focusCard(sceneOneSurfaces[0]!)
    await page.mouse.move(5, 500)
    await page.waitForTimeout(600)

    if (HOLD) {
      // Spread the board (persisted now), come back to the hero state, then hand the app over.
      await page.getByRole('button', { name: '看板' }).click()
      await moveTask(page, '订单列表分页超时', '运行中')
      await moveTask(page, '支付回调幂等性', '运行中')
      await moveTask(page, '结算页 500 热修', '阻塞')
      await moveTask(page, '登录页 A/B 实验', '完成')
      await moveTask(page, 'CI 缓存修复', '完成')
      await expect(page.locator('.board-feedback')).toHaveCount(0, { timeout: 5_000 })
      await page.getByRole('button', { name: '看板' }).click()
      // A real shell as the last card: `mt` is on PATH inside managed shells, so the control plane
      // can be driven live during a recording (mt list / read / send).
      await focusCard(sceneOneSurfaces[4]!)
      const shell = await newSurfaceAfter(page, () => page.getByRole('button', { name: '横向新增 Shell' }).click())
      await waitForShell(shell)
      await terminalCommand(shell, 'mt list')
      await page.waitForTimeout(800)
      await renameSession(page, shell, '终端 · mt')
      await focusCard(sceneOneSurfaces[0]!)
      await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.focus())
      console.log(`DEMO READY (root ${root}) — the app stays open until this process is killed`)
      await new Promise(() => {})
    }

    // ----- shot 1: workspace -----
    await mkdir(SHOTS, { recursive: true })
    await captureWindow(app, 'main', join(SHOTS, 'workspace-demo.png'))

    // ----- shot 2: notification center + HUD -----
    await page.getByRole('button', { name: '通知中心' }).click()
    await expect(page.getByRole('region', { name: '通知中心' })).toBeVisible()
    await page.mouse.move(5, 500)
    await page.waitForTimeout(400)
    await captureWindow(app, 'main', join(SHOTS, 'agent-hud-notifications-demo.png'))
    await page.getByRole('button', { name: '关闭通知中心' }).click()

    // ----- shot 3: kanban board -----
    await page.getByRole('button', { name: '看板' }).click()
    await expect(page.getByRole('region', { name: 'shop-api 看板' })).toBeVisible()
    await moveTask(page, '订单列表分页超时', '运行中')
    await moveTask(page, '支付回调幂等性', '运行中')
    await moveTask(page, '结算页 500 热修', '阻塞')
    await moveTask(page, '登录页 A/B 实验', '完成')
    await moveTask(page, 'CI 缓存修复', '完成')
    await expect(page.locator('.board-feedback')).toHaveCount(0, { timeout: 5_000 })
    await resizeWindow(app, BOARD_HEIGHT)
    await page.mouse.move(5, 300)
    await page.waitForTimeout(600)
    await captureWindow(app, 'main', join(SHOTS, 'workspace-board-demo.png'))
    await resizeWindow(app, WINDOW.height)
    await page.getByRole('button', { name: '看板' }).click()

    // ----- shot 4: DAG window for scene 2 -----
    await page.getByRole('tab').last().click()
    await page.getByRole('button', { name: '打开会话 DAG' }).click()
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const dag = (await app.windows()).find((candidate) => candidate !== page)!
    await expect(dag.locator('.dag-node-card')).toHaveCount(4)
    await alignDagWindow(app, ZOOM)
    await dag.waitForTimeout(500)
    await dag.getByRole('button', { name: '恢复 100%' }).click()
    await dag.getByRole('button', { name: '聚焦当前节点' }).click()
    await dag.waitForTimeout(500)
    await centerDagGraph(dag)
    await dag.mouse.move(5, 5)
    await dag.waitForTimeout(700)
    await captureWindow(app, 'dag', join(SHOTS, 'session-dag-demo.png'))
    await stage('04-done')

    // ----- animated demo: one storyline through notification, card switch, DAG and board -----
    await app.evaluate(({ BrowserWindow }) => {
      BrowserWindow.getAllWindows().find((w) => w.webContents.getURL().includes('kind=dag'))?.close()
    })
    await expect.poll(async () => (await app.windows()).length).toBe(1)
    await page.getByRole('tab').first().click()
    await focusCard(sceneOneSurfaces[0]!)
    // Start from a quiet state with one older notification, so the new one visibly arrives.
    await page.getByRole('button', { name: '通知中心' }).click()
    await page.getByRole('button', { name: '清空通知' }).click()
    await page.getByRole('button', { name: '关闭通知中心' }).click()
    await notify(sceneOneIds[2]!, { eventType: 'completed', title: '审查 · 方案对比', subtitle: '任务完成',
      body: 'ADR 已写入 docs/adr/0007，建议方案 A 为主、方案 B 兜底。' }, 300)
    await page.mouse.move(5, 500)
    const frames = join(root, 'frames')
    const recorder = frameRecorder(app, frames)

    // 1. Working in the implementation card; the regression card finishes and needs a decision.
    await recorder.hold(1400)
    await notify(sceneOneIds[1]!, { eventType: 'waiting', title: '回归 · 支付模块测试', subtitle: '等待输入',
      body: '1 个用例与新行为冲突，需要确认是否更新断言。' }, 0)
    await recorder.hold(1600)
    // 2. Open the notification center and jump to the card that raised it.
    await page.getByRole('button', { name: '通知中心' }).click()
    await recorder.hold(1500)
    await page.getByRole('button', { name: /打开通知：1 个用例与新行为冲突/ }).click()
    await page.mouse.move(5, 500)
    await recorder.hold(2200)
    // 3. Switch to the exploration canvas and open the DAG to see both plans at once.
    await page.getByRole('tab').last().click()
    await recorder.hold(1000)
    await page.getByRole('button', { name: '打开会话 DAG' }).click()
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const dagWindow = (await app.windows()).find((candidate) => candidate !== page)!
    await expect(dagWindow.locator('.dag-node-card')).toHaveCount(4)
    await alignDagWindow(app, ZOOM)
    await dagWindow.waitForTimeout(400)
    await dagWindow.getByRole('button', { name: '恢复 100%' }).click()
    await centerDagGraph(dagWindow)
    await dagWindow.mouse.move(5, 5)
    await recorder.hold(2400, 'dag')
    // 4. Plan B is stuck: click its node to land on that card.
    await dagWindow.getByRole('button', { name: '打开会话：方案 B · DB 唯一索引' }).click()
    await expect.poll(async () => (await app.windows()).length).toBe(1)
    await page.mouse.move(5, 500)
    await recorder.hold(2000)
    // 5. Open the board and park the task as blocked until the data is cleaned up.
    // Board moves are optimistic in the current build (no runtime handler persists them), so the
    // column layout is rebuilt inside this board session before recording.
    await page.getByRole('button', { name: '看板' }).click()
    await moveTask(page, '订单列表分页超时', '运行中')
    await moveTask(page, '支付回调幂等性', '运行中')
    await moveTask(page, '结算页 500 热修', '阻塞')
    await moveTask(page, '登录页 A/B 实验', '完成')
    await moveTask(page, 'CI 缓存修复', '完成')
    await expect(page.locator('.board-feedback')).toHaveCount(0, { timeout: 5_000 })
    await page.mouse.move(5, 300)
    await recorder.hold(1200)
    await moveTask(page, '支付回调幂等性', '阻塞')
    await recorder.hold(1800)
    await page.getByRole('button', { name: '看板' }).click()
    await encodeAnimation(await recorder.writeConcatList(), join(SHOTS, 'workspace-demo.gif'), join(SHOTS, 'workspace-demo.mp4'))
  } finally {
    await app.evaluate(({ app: electronApp }) => { electronApp.quit() }).catch(() => {})
    await app.close().catch(() => {})
    if (!process.env.MATOU_KEEP_DEMO_ROOT) await rm(root, { recursive: true, force: true })
    else console.log(`demo root kept at ${root}`)
  }
})

// ---------- fixtures ----------

async function prepareHome(home: string, demo: string): Promise<void> {
  await mkdir(join(home, '.claude'), { recursive: true })
  await writeFile(join(home, '.zshrc'), `PROMPT='%F{blue}%~%f %# '\nexport PATH="${join(demo, 'bin')}:$PATH"\n`)
  await writeFile(join(home, '.claude', 'CLAUDE.md'), '# 全局约定\n\n- 回复使用中文\n')
  await writeFile(join(home, '.claude', 'settings.json'), JSON.stringify({
    hooks: { Notification: [], SessionStart: [] }
  }, null, 2))
  await writeFile(join(home, '.claude.json'), JSON.stringify({ mcpServers: { context7: {} } }, null, 2))
}

async function prepareShopPlatform(dir: string): Promise<void> {
  await prepareRepo(dir, {
    'package.json': JSON.stringify({ name: 'shop-api', private: true, version: '3.14.0' }, null, 2) + '\n',
    'CLAUDE.md': '# shop-api\n\n- 支付相关改动必须带回归测试\n- 迁移脚本先在 staging 验证\n',
    '.mcp.json': JSON.stringify({ mcpServers: { postgres: {}, browser_bridge: {} } }, null, 2) + '\n',
    '.claude/settings.local.json': JSON.stringify({ hooks: { PreToolUse: [], PostToolUse: [], Stop: [] } }, null, 2) + '\n',
    'src/payments/webhook.ts': [
      "import { parseEvent } from './parse-event'",
      "import { paymentService } from './service'",
      '',
      'export async function handleWebhook(req) {',
      '  const event = parseEvent(req.body)',
      '  await paymentService.apply(event)',
      '  return ok({})',
      '}',
      ''
    ].join('\n'),
    'src/payments/apply.ts': 'export async function apply() {}\n',
    'prisma/schema.prisma': 'model PaymentCallback {\n  id       Int    @id\n  eventId  String\n}\n',
    'docs/payments.md': '# 支付回调约定\n'
  })
  await run('git', ['checkout', '-q', '-b', 'feat/webhook-idempotency'], { cwd: dir })
  await writeFile(join(dir, 'src/payments/webhook.ts'), [
    "import { parseEvent } from './parse-event'",
    "import { paymentService } from './service'",
    "import { redis } from '../infra/redis'",
    '',
    'export async function handleWebhook(req) {',
    '  const event = parseEvent(req.body)',
    '  const key = `pay:cb:${event.id}`',
    "  const fresh = await redis.set(key, '1', 'NX', 'EX', 86_400)",
    '  if (!fresh) return ok({ duplicate: true })',
    '  await paymentService.apply(event)',
    '  return ok({})',
    '}',
    ''
  ].join('\n'))
}

async function prepareRepo(dir: string, files: Record<string, string>): Promise<void> {
  await mkdir(dir, { recursive: true })
  for (const [path, content] of Object.entries(files)) {
    await mkdir(join(dir, path, '..'), { recursive: true })
    await writeFile(join(dir, path), content)
  }
  const env = {
    ...process.env, GIT_AUTHOR_NAME: 'dev', GIT_AUTHOR_EMAIL: 'dev@example.com',
    GIT_COMMITTER_NAME: 'dev', GIT_COMMITTER_EMAIL: 'dev@example.com'
  }
  await run('git', ['init', '-q', '-b', 'main'], { cwd: dir, env })
  await run('git', ['add', '-A'], { cwd: dir, env })
  await run('git', ['commit', '-q', '-m', 'init'], { cwd: dir, env })
}

async function prepareDemo(demo: string): Promise<void> {
  await mkdir(demo, { recursive: true })
  for (const [name, content] of Object.entries(transcripts)) {
    const widest = widestLine(name)
    if (widest > 56) throw new Error(`transcript ${name} has a ${widest}-cell line`)
    await writeFile(join(demo, `${name}.ans`), content)
  }
  await cp(resolve(import.meta.dirname, 'claude-stub.py'), join(demo, 'claude'))
  await chmod(join(demo, 'claude'), 0o755)
  await mkdir(join(demo, 'bin'), { recursive: true })
  await writeFile(join(demo, 'bin', 'pnpm'), `#!/bin/sh\ncat "${join(demo, 'vitest.ans')}"\nexit 1\n`)
  await chmod(join(demo, 'bin', 'pnpm'), 0o755)
  await writeFile(join(demo, 'roles.queue'),
    ['implementation', 'regression', 'review', 'docs', 'coordinate', 'planA1', 'planB1', 'baseline', 'planA', 'planB'].join('\n') + '\n')
  const day = 86_400
  const minute = 60_000
  const base = { model: 'Claude Opus 5', weekly: 41, resets_in: 3 * day + 5 * 3600 }
  const todos = [
    { content: '梳理回调处理链路', status: 'completed' },
    { content: '选择幂等键存储：Redis SETNX + 24h 过期', status: 'completed' },
    { content: '入口加幂等校验', status: 'in_progress' },
    { content: '为重复回调补测试', status: 'pending' },
    { content: '更新 docs/payments.md 回调约定', status: 'pending' }
  ]
  const waiting = ['hook', 'Notification', { message: 'Claude is waiting for your input' }]
  const planA = {
    ...base, transcript: 'planA', permission: 'acceptEdits', context: 41, duration_ms: 16 * minute,
    events: [
      ['hook', 'UserPromptSubmit', {}],
      ['tool', 'Edit', 'a-1', { file_path: 'src/payments/webhook.ts' }, 'ok'],
      ['tool', 'Bash', 'a-2', { command: 'pnpm vitest run src/payments' }, 'running']
    ]
  }
  const planB = {
    ...base, transcript: 'planB', permission: 'acceptEdits', context: 37, duration_ms: 14 * minute,
    events: [
      ['hook', 'UserPromptSubmit', {}],
      ['tool', 'Edit', 'b-1', { file_path: 'prisma/schema.prisma' }, 'ok'],
      ['tool', 'Bash', 'b-2', { command: 'pnpm prisma migrate dev --name callback-event-unique' }, 'fail'],
      ['hook', 'Notification', { message: 'Error: migration failed with P2002 unique constraint' }]
    ]
  }
  const regression = {
    ...base, transcript: 'regression', permission: 'default', context: 34, duration_ms: 12 * minute,
    events: [
      ['hook', 'UserPromptSubmit', {}],
      ['tool', 'Bash', 'reg-1', { command: 'pnpm vitest run src/payments' }, 'ok'],
      waiting
    ]
  }
  await writeFile(join(demo, 'roles.json'), JSON.stringify({
    implementation: {
      ...base, transcript: 'implementation', permission: 'acceptEdits', context: 62, duration_ms: 47 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Read', 'impl-1', { file_path: 'src/payments/webhook.ts' }, 'ok'],
        ['tool', 'TodoWrite', 'impl-3', { todos }, 'ok'],
        ['tool', 'Edit', 'impl-4', { file_path: 'src/payments/webhook.ts' }, 'ok'],
        ['tool', 'Bash', 'impl-5', { command: 'pnpm vitest run src/payments --reporter=dot' }, 'running']
      ]
    },
    regression,
    review: {
      ...base, transcript: 'review', permission: 'acceptEdits', context: 51, duration_ms: 23 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Read', 'rev-1', { file_path: 'prisma/schema.prisma' }, 'ok'],
        ['tool', 'Write', 'rev-2', { file_path: 'docs/adr/0007-idempotency.md' }, 'ok'],
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Bash', 'rev-3', { command: 'mt read left --lines 12' }, 'ok'],
        ['hook', 'Stop', { last_assistant_message: '结论：方案 A（Redis）为主路径，方案 B 唯一索引兜底，ADR 已更新。' }]
      ]
    },
    docs: {
      ...base, transcript: 'docs', permission: 'acceptEdits', context: 22, duration_ms: 6 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Read', 'doc-1', { file_path: 'docs/payments.md' }, 'ok'],
        ['tool', 'Edit', 'doc-2', { file_path: 'docs/payments.md' }, 'ok'],
        ['hook', 'Stop', { last_assistant_message: '文档已更新，和 webhook.ts 里的实现保持一致。' }]
      ]
    },
    coordinate: {
      ...base, transcript: 'coordinate', permission: 'acceptEdits', context: 19, duration_ms: 5 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Bash', 'co-1', { command: 'mt list' }, 'ok'],
        ['tool', 'Bash', 'co-2', { command: 'mt read sibling:2 --lines 8' }, 'ok'],
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Bash', 'co-3', { command: 'mt send sibling:2 "改成新行为，并同步 docs/payments.md" --enter' }, 'ok'],
        ['hook', 'Stop', { last_assistant_message: '已经交给回归卡片了，完成后我再读一次。' }]
      ]
    },
    baseline: {
      ...base, transcript: 'baseline', permission: 'default', context: 18, duration_ms: 9 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Read', 'base-1', { file_path: 'src/payments/webhook.ts' }, 'ok'],
        ['hook', 'Stop', { last_assistant_message: '建议分两条路线并行验证：Redis SETNX / DB 唯一索引。' }]
      ]
    },
    planA1: planA,
    planB1: planB,
    planA,
    planB
  }, null, 2))
}

async function launch(input: { root: string; home: string; workspace: string; demo: string }): Promise<ElectronApplication> {
  const dataDirectory = join(input.root, 'data')
  const userData = join(input.root, 'electron-user-data')
  await mkdir(userData, { recursive: true })
  return electron.launch({
    args: [join(REPO, 'apps/desktop'), `--user-data-dir=${userData}`],
    env: {
      ...process.env,
      HOME: input.home,
      SHELL: '/bin/zsh',
      ZDOTDIR: input.home,
      CLAUDE_CONFIG_DIR: join(input.home, '.claude'),
      MATOU_E2E: '1',
      MATOU_DATA_DIR: dataDirectory,
      MATOU_DEFAULT_WORKSPACE: input.workspace,
      ELECTRON_USER_DATA_DIR: userData,
      MATOU_RUNTIME_ENTRY: join(REPO, 'apps/runtime/dist/index.cjs'),
      MATOU_CLAUDE_COMMAND: join(input.demo, 'claude'),
      MATOU_DEMO_ROOT: input.demo,
      MATOU_DISABLE_AUTO_UPDATE: '1'
    }
  })
}

async function placeWindow(app: ElectronApplication, zoom: number): Promise<void> {
  const placement = await app.evaluate(({ BrowserWindow, screen }, { zoom, size }) => {
    const window = BrowserWindow.getAllWindows()[0]!
    const area = screen.getDisplayMatching(window.getBounds()).workArea
    window.setBounds({
      x: area.x + Math.max(0, Math.floor((area.width - size.width) / 2)),
      y: area.y + Math.max(0, Math.floor((area.height - size.height) / 2)),
      width: Math.min(size.width, area.width),
      height: Math.min(size.height, area.height)
    })
    window.webContents.setZoomFactor(zoom)
    return { area, bounds: window.getBounds(), displays: screen.getAllDisplays().map((d) => ({ label: d.label, internal: d.internal, workArea: d.workArea, scale: d.scaleFactor })) }
  }, { zoom, size: WINDOW })
  console.log('window placement', JSON.stringify(placement))
}

// The DAG opens as its own window; give it the main window's frame so recorded frames share one size.
async function alignDagWindow(app: ElectronApplication, zoom: number): Promise<void> {
  await app.evaluate(({ BrowserWindow }, zoom) => {
    const windows = BrowserWindow.getAllWindows()
    const dag = windows.find((w) => w.webContents.getURL().includes('kind=dag'))!
    const main = windows.find((w) => !w.webContents.getURL().includes('kind=dag'))!
    dag.setBounds(main.getBounds())
    dag.webContents.setZoomFactor(zoom)
  }, zoom)
}

// Captures the composited window at the display's physical resolution (the zoomed CSS viewport
// confuses Playwright's own screenshot sizing).
async function captureWindow(app: ElectronApplication, which: 'main' | 'dag', path: string): Promise<void> {
  const size = await app.evaluate(async ({ BrowserWindow }, { which, path }) => {
    const isDag = (w: Electron.BrowserWindow) => w.webContents.getURL().includes('kind=dag')
    const window = BrowserWindow.getAllWindows().find((w) => (which === 'dag') === isDag(w))!
    const image = await window.webContents.capturePage()
    const fs = process.getBuiltinModule('node:fs') as typeof import('node:fs')
    fs.writeFileSync(path, image.toPNG())
    return image.getSize()
  }, { which, path })
  console.log(`captured ${path} ${size.width}x${size.height}`)
}

async function resizeWindow(app: ElectronApplication, height: number): Promise<void> {
  await app.evaluate(({ BrowserWindow }, height) => {
    const window = BrowserWindow.getAllWindows()[0]!
    window.setBounds({ ...window.getBounds(), height })
  }, height)
}

// Pans the DAG so the bounding box of all node cards sits in the middle of the window.
async function centerDagGraph(dag: Page): Promise<void> {
  const box = await dag.evaluate(() => {
    const rects = [...document.querySelectorAll('.dag-node-card')].map((node) => node.getBoundingClientRect())
    const left = Math.min(...rects.map((r) => r.left))
    const right = Math.max(...rects.map((r) => r.right))
    const top = Math.min(...rects.map((r) => r.top))
    const bottom = Math.max(...rects.map((r) => r.bottom))
    return { cx: (left + right) / 2, cy: (top + bottom) / 2, width: window.innerWidth, height: window.innerHeight }
  })
  const startX = box.width / 2
  const startY = box.height - 30
  await dag.mouse.move(startX, startY)
  await dag.mouse.down()
  await dag.mouse.move(startX + (box.width / 2 - box.cx), startY + (box.height / 2 - box.cy), { steps: 12 })
  await dag.mouse.up()
}

async function focusCard(surface: Locator): Promise<void> {
  await surface.scrollIntoViewIfNeeded()
  await surface.click({ position: { x: 12, y: 12 } })
  await expect(paneOf(surface)).toHaveAttribute('data-active', 'true')
}

// Records a window at up to `fps` while the caller drives the UI between holds. Each frame keeps its
// real duration (capturePage takes ~100 ms) so the encoded animation plays back at true speed.
function frameRecorder(app: ElectronApplication, dir: string, fps = 10) {
  const frames: Array<{ path: string; capturedAt: number }> = []
  const interval = 1000 / fps
  return {
    async hold(ms: number, which: 'main' | 'dag' = 'main') {
      await mkdir(dir, { recursive: true })
      const until = Date.now() + ms
      while (Date.now() < until) {
        const startedAt = Date.now()
        const path = join(dir, `frame-${String(frames.length).padStart(4, '0')}.png`)
        await app.evaluate(async ({ BrowserWindow }, { path, which }) => {
          const isDag = (w: Electron.BrowserWindow) => w.webContents.getURL().includes('kind=dag')
          const window = BrowserWindow.getAllWindows().find((w) => (which === 'dag') === isDag(w))!
          const image = await window.webContents.capturePage()
          const fs = process.getBuiltinModule('node:fs') as typeof import('node:fs')
          fs.writeFileSync(path, image.toPNG())
        }, { path, which })
        frames.push({ path, capturedAt: startedAt })
        const remaining = interval - (Date.now() - startedAt)
        if (remaining > 0) await new Promise((resolve) => setTimeout(resolve, remaining))
      }
    },
    async writeConcatList(): Promise<string> {
      const lines: string[] = []
      frames.forEach((frame, index) => {
        const next = frames[index + 1]
        const duration = next ? (next.capturedAt - frame.capturedAt) / 1000 : 1.5
        lines.push(`file '${frame.path}'`, `duration ${duration.toFixed(3)}`)
      })
      lines.push(`file '${frames.at(-1)!.path}'`)
      const list = join(dir, 'frames.txt')
      await writeFile(list, lines.join('\n') + '\n')
      return list
    }
  }
}

async function encodeAnimation(concatList: string, gifPath: string, mp4Path: string): Promise<void> {
  const input = ['-f', 'concat', '-safe', '0', '-i', concatList]
  const scratch = join(concatList, '..')
  const intermediate = join(scratch, 'animation.mov')
  const palette = join(scratch, 'palette.png')
  // Drop the static frames inside each hold (mpdecimate) but keep their real timing through a
  // lossless intermediate; a single filtergraph loses the variable frame durations in GIF output.
  await run('ffmpeg', ['-y', '-loglevel', 'error', ...input, '-vf', 'mpdecimate=hi=768:lo=320:frac=0.33,scale=1000:-1:flags=lanczos', '-fps_mode', 'vfr', '-c:v', 'png', intermediate])
  await run('ffmpeg', ['-y', '-loglevel', 'error', '-i', intermediate, '-vf', 'palettegen=max_colors=256:stats_mode=full', palette])
  await run('ffmpeg', ['-y', '-loglevel', 'error', '-i', intermediate, '-i', palette, '-lavfi', 'paletteuse=dither=none:diff_mode=rectangle', '-fps_mode', 'vfr', '-loop', '0', gifPath])
  await run('ffmpeg', ['-y', '-loglevel', 'error', ...input, '-vf', 'scale=1400:-2:flags=lanczos,fps=15', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-crf', '23', '-movflags', '+faststart', mp4Path])
  const sizes = await Promise.all([gifPath, mp4Path].map(async (path) => `${path} ${(await stat(path)).size} bytes`))
  console.log(sizes.join('\n'))
}

// ---------- UI helpers ----------

function visibleSurfaces(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]:visible .terminal-surface')
}

function activeSurface(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"]:visible .terminal-surface')
}

async function stableSurface(surface: Locator): Promise<Locator> {
  const sessionId = await surface.getAttribute('data-session-id')
  if (!sessionId) throw new Error('Terminal Session identity is missing')
  return surface.page().locator(`.terminal-surface[data-session-id="${sessionId}"]`)
}

async function sessionIds(page: Page): Promise<string[]> {
  return page.locator('.terminal-surface').evaluateAll((elements) =>
    elements.map((element) => element.getAttribute('data-session-id') ?? '').filter(Boolean))
}

async function newSurfaceAfter(page: Page, action: () => Promise<unknown>): Promise<Locator> {
  const before = new Set(await sessionIds(page))
  await action()
  let created = ''
  await expect.poll(async () => {
    const fresh = (await sessionIds(page)).filter((id) => !before.has(id))
    created = fresh[0] ?? ''
    return fresh.length
  }, { timeout: 30_000 }).toBe(1)
  return page.locator(`.terminal-surface[data-session-id="${created}"]`)
}

function paneOf(surface: Locator): Locator {
  return surface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
}

async function waitForShell(surface: Locator): Promise<void> {
  await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
}

async function terminalCommand(surface: Locator, command: string): Promise<void> {
  const sessionId = await surface.getAttribute('data-session-id')
  if (!sessionId) throw new Error('Terminal Session identity is missing')
  const page = surface.page()
  const stable = page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
  await waitForShell(stable)
  const pane = paneOf(stable)
  const textarea = stable.locator('.xterm-helper-textarea')
  if (await pane.getAttribute('data-active') !== 'true') {
    await stable.scrollIntoViewIfNeeded()
    await stable.click({ position: { x: 12, y: 12 } })
  }
  await textarea.focus()
  await expect(pane).toHaveAttribute('data-active', 'true')
  await expect(textarea).toBeFocused()
  await page.waitForTimeout(50)
  await textarea.pressSequentially(command, { delay: 2 })
  await textarea.press('Enter')
}

async function promoteToClaude(surface: Locator, demo: string): Promise<void> {
  const sessionId = await surface.getAttribute('data-session-id')
  const stable = surface.page().locator(`.terminal-surface[data-session-id="${sessionId}"]`)
  const before = await launches(demo)
  await terminalCommand(stable, 'claude')
  await expect(stable).toHaveAttribute('data-profile', 'claude-code')
  await expect.poll(() => launches(demo)).toBe(before + 1)
  await surface.page().waitForTimeout(900)
}

async function waitForRole(demo: string, role: string): Promise<void> {
  await expect.poll(async () => (await readFile(join(demo, 'launches.log'), 'utf8').catch(() => '')).includes(`"role": "${role}"`)).toBe(true)
}

async function launches(demo: string): Promise<number> {
  const log = await readFile(join(demo, 'launches.log'), 'utf8').catch(() => '')
  return log.split('\n').filter(Boolean).length
}

async function renameSession(page: Page, surface: Locator, title: string): Promise<void> {
  const pane = paneOf(surface)
  await pane.locator('.pane-title').scrollIntoViewIfNeeded()
  await pane.locator('.pane-title').click({ button: 'right' })
  await page.getByRole('menuitem', { name: '重命名…' }).click()
  await page.getByRole('textbox', { name: '会话名称' }).fill(title)
  await page.getByRole('button', { name: '确定' }).click()
  await expect(pane.locator('.pane-title')).toHaveText(title)
}

async function renameTask(page: Page, from: string, to: string): Promise<void> {
  await page.getByRole('button', { name: `事项菜单：${from}` }).click()
  await page.getByRole('menuitem', { name: '重命名' }).click()
  await page.getByRole('textbox', { name: '事项名称' }).fill(to)
  await page.getByRole('button', { name: '确定' }).click()
  await expect(page.getByTestId('active-task')).toHaveText(to)
}

async function selectTask(page: Page, title: string): Promise<void> {
  await page.locator('.workspace-group.is-active .workbench-item', { hasText: title }).first().click()
  await expect(page.getByTestId('active-task')).toHaveText(title)
}

async function renameActiveTab(page: Page, name: string): Promise<void> {
  await page.locator('.tab-item.active .tab-title').dblclick()
  await page.getByRole('textbox', { name: '页签名称' }).fill(name)
  await page.getByRole('button', { name: '确定' }).click()
  await expect(page.locator('.tab-item.active .tab-title')).toHaveText(name)
}

async function forkChild(page: Page, pane: Locator, title: string, branch: string): Promise<void> {
  const button = pane.getByRole('button', { name: `从“${title}”创建子分支` })
  await expect(button).not.toHaveAttribute('aria-disabled', 'true')
  await button.click()
  await fillForkDialog(page, branch)
}

async function forkSibling(page: Page, pane: Locator, title: string, branch: string): Promise<void> {
  const button = pane.getByRole('button', { name: `从共同父会话创建“${title}”的兄弟分支` })
  await expect(button).not.toHaveAttribute('aria-disabled', 'true')
  await button.click()
  await fillForkDialog(page, branch)
}

async function fillForkDialog(page: Page, branch: string): Promise<void> {
  await page.getByLabel('分支名称').fill(branch)
  await expect(page.getByRole('radio', { name: /使用当前工作树/ })).toBeChecked()
  await page.getByRole('button', { name: '创建分支', exact: true }).click()
  await expect(page.getByLabel('分支名称')).toHaveCount(0)
}

async function hierarchyIds(page: Page, surface: Locator): Promise<Ids> {
  const workspaceId = await page.locator('.workspace-group.is-active').getAttribute('data-workspace-id')
  const taskTestId = await page.locator('.workbench-item.is-active').getAttribute('data-testid')
  const sceneId = await page.locator('.tab-item.active').getAttribute('data-scene-id')
  const sessionId = await surface.getAttribute('data-session-id')
  if (!workspaceId || !taskTestId || !sceneId || !sessionId) throw new Error('Hierarchy identity is missing')
  return { workspaceId, taskId: taskTestId.replace(/^task-/, ''), sceneId, sessionId }
}

async function moveTask(page: Page, title: string, column: string): Promise<void> {
  // The board keeps the dragged id in React state, so each drag event needs its own task turn.
  const steps: Array<['card' | 'column', string]> = [
    ['card', 'dragstart'], ['column', 'dragenter'], ['column', 'dragover'], ['column', 'drop'], ['card', 'dragend']
  ]
  for (const [on, type] of steps) {
    await page.evaluate(({ title, column, on, type }) => {
      const card = document.querySelector<HTMLElement>(`article.board-task-card[aria-label="${title}"]`)
      const target = document.querySelector<HTMLElement>(`section[aria-label="${column}列"]`)
      if (!card || !target) throw new Error(`board card or column missing: ${title} → ${column}`)
      const scope = window as unknown as { __readmeDragData?: DataTransfer }
      scope.__readmeDragData ??= new DataTransfer()
      const element = on === 'card' ? card : target
      element.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: scope.__readmeDragData }))
      if (type === 'dragend') delete scope.__readmeDragData
    }, { title, column, on, type })
    await page.waitForTimeout(60)
  }
  await expect(page.locator(`section[aria-label="${column}列"] article.board-task-card[aria-label="${title}"]`)).toBeVisible()
}

function stageRecorder(page: Page, root: string) {
  const dir = process.env.MATOU_STAGE_DIR
  return async (name: string) => {
    if (!dir) return
    await mkdir(dir, { recursive: true })
    await page.locator('.hierarchy-shell').screenshot({ path: join(dir, `${name}.png`) }).catch(() => {})
    console.log(`stage ${name} captured (root ${root})`)
  }
}
