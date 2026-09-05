// Rebuilds the README demo scene with isolated fixtures and captures assets/shots/*.png.
// Run: pnpm build && MATOU_README_CAPTURE=1 npx playwright test tests/e2e/readme-capture --workers=1
// Demo mode: MATOU_DEMO_HOLD=1 builds the same scene and keeps the app open (for recording); add
// MATOU_E2E_DISPLAY=primary to place it on the main display instead of the built-in one.
// The window is placed on the secondary (built-in) display like the other e2e specs.
// Every session is a stub `claude` (claude-stub.py); nothing touches the real CLI or account.
import { expect, test, type ElectronApplication, type Locator } from '@playwright/test'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  REPO, WINDOW, type Ids, prepareHome, prepareShopPlatform, prepareRepo, prepareDemo, launch, placeWindow,
  alignDagWindow, captureWindow, resizeWindow, centerDagGraph, focusCard, frameRecorder, encodeAnimation,
  visibleSurfaces, activeSurface, stableSurface, newSurfaceAfter, paneOf, waitForShell, terminalCommand,
  promoteToClaude, renameTask, selectTask, renameActiveTab, renameSession, hierarchyIds, forkChild, forkSibling,
  waitForRole, moveTask, stageRecorder
} from './demo-scene'

const SHOTS = process.env.MATOU_SHOTS_DIR ?? join(REPO, 'assets', 'shots')
// At 1400 CSS px the canvas stage stays under the four-column threshold, so the carousel shows the
// focused card plus one sibling at natural font size; capturePage() records the display's Retina pixels.
const ZOOM = 1
const BOARD_HEIGHT = 640

const HOLD = process.env.MATOU_DEMO_HOLD === '1'
test.setTimeout(HOLD ? 0 : 300_000)

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

