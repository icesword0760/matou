// Rebuilds the README demo scene with isolated fixtures and captures assets/shots/*.png.
// Run: pnpm build && MATOU_README_CAPTURE=1 npx playwright test tests/e2e/readme-capture --workers=1
// Locale: MATOU_LOCALE=en drives the app in English and writes assets/shots/en instead; the default
// (zh-CN) keeps writing assets/shots. Accessible names and default entity names are read from the
// app's own catalogs (see ./locale), so this spec cannot drift from the UI.
// Demo mode: MATOU_DEMO_HOLD=1 builds the same scene and keeps the app open (for recording); add
// MATOU_E2E_DISPLAY=primary to place it on the main display instead of the built-in one.
// The window is placed on the secondary (built-in) display like the other e2e specs.
// Every session is a stub `claude` (claude-stub.py); nothing touches the real CLI or account.
import { expect, test, type ElectronApplication, type Locator, type Page } from '@playwright/test'
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
import { LOCALE, demo, runtime, ui } from './locale'

const SHOTS = process.env.MATOU_SHOTS_DIR
  ?? join(REPO, 'assets', 'shots', ...(LOCALE === 'en' ? ['en'] : []))
const shell = ui.hierarchyShell
const board = shell.kanban
// The notification-center entry is matched on the body's leading clause, which is stable while the
// rest of the sentence is not.
const openNotification = (lead: string): RegExp =>
  new RegExp(ui.notifications.center.open(lead).replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
// Board moves are optimistic in the current build (no runtime handler persists them), so every board
// session rebuilds the same column layout before it is captured.
const spreadBoard = async (page: Page): Promise<void> => {
  await moveTask(page, demo.tasks.pagination, board.columns.active)
  await moveTask(page, demo.tasks.idempotency, board.columns.active)
  await moveTask(page, demo.tasks.checkout, board.columns.blocked)
  await moveTask(page, demo.tasks.loginAb, board.columns.completed)
  await moveTask(page, demo.tasks.ciCache, board.columns.completed)
  await expect(page.locator('.board-feedback')).toHaveCount(0, { timeout: 5_000 })
}
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
  const demoRoot = join(root, 'demo')
  await prepareHome(home, demoRoot)
  await prepareShopPlatform(workspace)
  await prepareRepo(mobile, { 'README.md': '# mobile-app\n' })
  await prepareDemo(demoRoot)

  const app = await launch({ root, home, workspace, demo: demoRoot })
  try {
    const page = await app.firstWindow()
    await placeWindow(app, ZOOM)
    console.log('viewport', await page.evaluate(() => `${window.innerWidth}x${window.innerHeight} @${window.devicePixelRatio}`))
    const stage = stageRecorder(page, root)

    // ----- shop-api: tasks -----
    const tasks = demo.tasks
    await expect(page.getByTestId('active-task')).toHaveText(runtime.hierarchy.defaultTask)
    await renameTask(page, runtime.hierarchy.defaultTask, tasks.idempotency)
    for (const title of [tasks.pagination, tasks.prisma, tasks.lcp, tasks.checkout, tasks.loginAb, tasks.ciCache]) {
      await page.getByRole('button', { name: shell.taskSidebar.newTaskIn('shop-api') }).click()
      await renameTask(page, runtime.hierarchy.newTask, title)
      await waitForShell(activeSurface(page))
      if (title === tasks.pagination) {
        await page.getByRole('button', { name: shell.sceneTabBar.addShell }).click()
        await expect(visibleSurfaces(page)).toHaveCount(2)
      }
    }
    await selectTask(page, tasks.idempotency)
    await stage('01-tasks')

    // ----- scene 1: implementation / regression / review -----
    await renameActiveTab(page, demo.canvases.implement)
    const cards = [
      demo.cards.implementation, demo.cards.regression, demo.cards.review, demo.cards.docs, demo.cards.coordinate
    ]
    const sceneOneSurfaces: Locator[] = [await stableSurface(visibleSurfaces(page).first())]
    for (let index = 1; index < cards.length; index += 1) {
      sceneOneSurfaces.push(await newSurfaceAfter(page, () => page.getByRole('button', { name: shell.sceneTabBar.addShell }).click()))
    }
    const sceneOneIds: Ids[] = []
    for (const [index, title] of cards.entries()) {
      const surface = sceneOneSurfaces[index]!
      await waitForShell(surface)
      await promoteToClaude(surface, demoRoot)
      await renameSession(page, surface, title)
      sceneOneIds.push(await hierarchyIds(page, surface))
    }
    // A subtree under the review card so this canvas's DAG has depth too: the comparison forks into
    // plan A / plan B, plus a derived shell running the regression. (Forking needs a finished turn,
    // which the review role has and the still-running implementation role does not.)
    const implPlanA = await newSurfaceAfter(page, () =>
      forkChild(page, paneOf(sceneOneSurfaces[2]!), cards[2]!, 'impl-redis'))
    await waitForRole(demoRoot, 'planA1')
    await renameSession(page, implPlanA, demo.cards.planA)
    const implPlanB = await newSurfaceAfter(page, () =>
      forkSibling(page, paneOf(implPlanA), demo.cards.planA, 'impl-unique-index'))
    await waitForRole(demoRoot, 'planB1')
    await renameSession(page, implPlanB, demo.cards.planB)
    await implPlanA.click({ position: { x: 12, y: 12 } })
    const implVitest = await newSurfaceAfter(page, () => page.getByRole('button', { name: shell.sceneTabBar.addShell }).click())
    await waitForShell(implVitest)
    await terminalCommand(implVitest, 'pnpm vitest run src/payments')
    await page.waitForTimeout(600)
    await renameSession(page, implVitest, demo.cards.vitest)
    await page.getByRole('button', { name: ui.sessionCanvas.returnToParent }).click()
    await expect(visibleSurfaces(page)).toHaveCount(cards.length)
    await stage('02-scene-one')

    // ----- scene 2: baseline with two forks and one derived shell (for the DAG) -----
    await page.getByRole('button', { name: shell.sceneTabBar.newTab }).click()
    await expect(page.getByRole('tab')).toHaveCount(2)
    await renameActiveTab(page, demo.canvases.explore)
    const baseline = await stableSurface(visibleSurfaces(page).first())
    await promoteToClaude(baseline, demoRoot)
    await renameSession(page, baseline, demo.cards.baseline)
    const planA = await newSurfaceAfter(page, () =>
      forkChild(page, paneOf(baseline), demo.cards.baseline, 'idem-redis'))
    await waitForRole(demoRoot, 'planA')
    await stage('03a-fork-child')
    await renameSession(page, planA, demo.cards.planA)
    const planB = await newSurfaceAfter(page, () =>
      forkSibling(page, paneOf(planA), demo.cards.planA, 'idem-unique-index'))
    await waitForRole(demoRoot, 'planB')
    await stage('03b-fork-sibling')
    await renameSession(page, planB, demo.cards.planB)
    await planA.click({ position: { x: 12, y: 12 } })
    const derived = await newSurfaceAfter(page, () => page.getByRole('button', { name: shell.sceneTabBar.addShell }).click())
    await waitForShell(derived)
    await terminalCommand(derived, 'pnpm vitest run src/payments')
    await page.waitForTimeout(600)
    await renameSession(page, derived, demo.cards.vitest)
    const planBIds = await hierarchyIds(page, planB)
    const vitestIds = await hierarchyIds(page, derived)
    await stage('03-scene-two')

    // ----- mobile-app workspace: one finished task for a cross-workspace notification -----
    await app.evaluate(({ ipcMain }, selectedPath) => {
      const channel = 'matou:select-workspace-directory'
      ipcMain.removeHandler(channel)
      ipcMain.handle(channel, () => selectedPath)
    }, mobile)
    await page.getByRole('button', { name: shell.taskSidebar.newWorkspace, exact: true }).click()
    await expect(page.locator('.workspace-group.is-active')).toContainText('mobile-app')
    await renameTask(page, runtime.hierarchy.defaultTask, tasks.mobileCrash)
    await waitForShell(activeSurface(page))
    const mobileIds = await hierarchyIds(page, activeSurface(page))

    // back to shop-api / the idempotency task / scene 1
    await page.locator('.workspace-group', { hasText: 'shop-api' }).locator('.workspace-group__toggle').click()
    await selectTask(page, tasks.idempotency)
    await page.getByRole('tab').first().click()
    await expect(visibleSurfaces(page)).toHaveCount(cards.length)

    // ----- notifications: drop the hook-generated ones, push curated ones -----
    await page.getByRole('button', { name: shell.notificationCenter, exact: true }).click()
    const clear = page.getByRole('button', { name: ui.notifications.center.clear })
    if (await clear.isVisible()) await clear.click()
    await page.getByRole('button', { name: ui.notifications.center.close }).click()
    const notify = async (ids: Ids, input: { eventType: string; title: string; subtitle: string; body: string }, ago: number) => {
      await page.evaluate(({ ids, input }) => {
        if (!window.matouE2e) throw new Error('Matou E2E bridge is missing')
        window.matouE2e.pushNotification({ ...input, ...ids, sound: false })
      }, { ids, input: { ...input, eventId: `readme-${input.title}` } })
      await page.waitForTimeout(ago)
    }
    const kind = demo.notificationKind
    const body = demo.notificationBody
    const regressionWaiting = body.regressionWaitingLead + body.regressionWaitingRest
    await notify(mobileIds, { eventType: 'completed', title: tasks.mobileCrash, subtitle: kind.completed,
      body: body.mobileDone }, 1200)
    await notify(sceneOneIds[2]!, { eventType: 'completed', title: demo.cards.review, subtitle: kind.completed,
      body: body.reviewDone }, 1200)
    await notify(sceneOneIds[1]!, { eventType: 'waiting', title: demo.cards.regression, subtitle: kind.waiting,
      body: regressionWaiting }, 1200)
    await notify(planBIds, { eventType: 'waiting', title: demo.cards.planB, subtitle: kind.waiting,
      body: body.planBWaiting }, 1200)
    await notify(vitestIds, { eventType: 'error', title: demo.cards.vitest, subtitle: kind.error,
      body: body.vitestError }, 300)

    // focus the implementation card so the HUD shows the running session
    await focusCard(sceneOneSurfaces[0]!)
    await page.mouse.move(5, 500)
    await page.waitForTimeout(600)

    if (HOLD) {
      // Spread the board (persisted now), come back to the hero state, then hand the app over.
      await page.getByRole('button', { name: shell.taskSidebar.board, exact: true }).click()
      await spreadBoard(page)
      await page.getByRole('button', { name: shell.taskSidebar.board, exact: true }).click()
      // A real shell as the last card: `mt` is on PATH inside managed shells, so the control plane
      // can be driven live during a recording (mt list / read / send).
      await focusCard(sceneOneSurfaces[4]!)
      const mtShell = await newSurfaceAfter(page, () => page.getByRole('button', { name: shell.sceneTabBar.addShell }).click())
      await waitForShell(mtShell)
      await terminalCommand(mtShell, 'mt list')
      await page.waitForTimeout(800)
      await renameSession(page, mtShell, demo.cards.terminal)
      await focusCard(sceneOneSurfaces[0]!)
      await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.focus())
      console.log(`DEMO READY (root ${root}) — the app stays open until this process is killed`)
      await new Promise(() => {})
    }

    // ----- shot 1: workspace -----
    await mkdir(SHOTS, { recursive: true })
    await captureWindow(app, 'main', join(SHOTS, 'workspace-demo.png'))

    // ----- shot 2: notification center + HUD -----
    await page.getByRole('button', { name: shell.notificationCenter, exact: true }).click()
    await expect(page.getByRole('region', { name: ui.notifications.center.label })).toBeVisible()
    await page.mouse.move(5, 500)
    await page.waitForTimeout(400)
    await captureWindow(app, 'main', join(SHOTS, 'agent-hud-notifications-demo.png'))
    await page.getByRole('button', { name: ui.notifications.center.close }).click()

    // ----- shot 3: kanban board -----
    await page.getByRole('button', { name: shell.taskSidebar.board, exact: true }).click()
    await expect(page.getByRole('region', { name: board.board('shop-api') })).toBeVisible()
    await spreadBoard(page)
    await resizeWindow(app, BOARD_HEIGHT)
    await page.mouse.move(5, 300)
    await page.waitForTimeout(600)
    await captureWindow(app, 'main', join(SHOTS, 'workspace-board-demo.png'))
    await resizeWindow(app, WINDOW.height)
    await page.getByRole('button', { name: shell.taskSidebar.board, exact: true }).click()

    // ----- shot 4: DAG window for scene 2 -----
    await page.getByRole('tab').last().click()
    await page.getByRole('button', { name: shell.sceneTabBar.openDag }).click()
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const dag = (await app.windows()).find((candidate) => candidate !== page)!
    await expect(dag.locator('.dag-node-card')).toHaveCount(4)
    await alignDagWindow(app, ZOOM)
    await dag.waitForTimeout(500)
    await dag.getByRole('button', { name: ui.dag.canvas.resetZoom }).click()
    await dag.getByRole('button', { name: ui.dag.canvas.focusCurrent }).click()
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
    await page.getByRole('button', { name: shell.notificationCenter, exact: true }).click()
    await page.getByRole('button', { name: ui.notifications.center.clear }).click()
    await page.getByRole('button', { name: ui.notifications.center.close }).click()
    await notify(sceneOneIds[2]!, { eventType: 'completed', title: demo.cards.review, subtitle: kind.completed,
      body: body.reviewDone }, 300)
    await page.mouse.move(5, 500)
    const frames = join(root, 'frames')
    const recorder = frameRecorder(app, frames)

    // 1. Working in the implementation card; the regression card finishes and needs a decision.
    await recorder.hold(1400)
    await notify(sceneOneIds[1]!, { eventType: 'waiting', title: demo.cards.regression, subtitle: kind.waiting,
      body: regressionWaiting }, 0)
    await recorder.hold(1600)
    // 2. Open the notification center and jump to the card that raised it.
    await page.getByRole('button', { name: shell.notificationCenter, exact: true }).click()
    await recorder.hold(1500)
    await page.getByRole('button', { name: openNotification(body.regressionWaitingLead) }).click()
    await page.mouse.move(5, 500)
    await recorder.hold(2200)
    // 3. Switch to the exploration canvas and open the DAG to see both plans at once.
    await page.getByRole('tab').last().click()
    await recorder.hold(1000)
    await page.getByRole('button', { name: shell.sceneTabBar.openDag }).click()
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const dagWindow = (await app.windows()).find((candidate) => candidate !== page)!
    await expect(dagWindow.locator('.dag-node-card')).toHaveCount(4)
    await alignDagWindow(app, ZOOM)
    await dagWindow.waitForTimeout(400)
    await dagWindow.getByRole('button', { name: ui.dag.canvas.resetZoom }).click()
    await centerDagGraph(dagWindow)
    await dagWindow.mouse.move(5, 5)
    await recorder.hold(2400, 'dag')
    // 4. Plan B is stuck: click its node to land on that card.
    await dagWindow.getByRole('button', { name: ui.dag.nodeCard.open(demo.cards.planB) }).click()
    await expect.poll(async () => (await app.windows()).length).toBe(1)
    await page.mouse.move(5, 500)
    await recorder.hold(2000)
    // 5. Open the board and park the task as blocked until the data is cleaned up.
    await page.getByRole('button', { name: shell.taskSidebar.board, exact: true }).click()
    await spreadBoard(page)
    await page.mouse.move(5, 300)
    await recorder.hold(1200)
    await moveTask(page, tasks.idempotency, board.columns.blocked)
    await recorder.hold(1800)
    await page.getByRole('button', { name: shell.taskSidebar.board, exact: true }).click()
    await encodeAnimation(await recorder.writeConcatList(), join(SHOTS, 'workspace-demo.gif'), join(SHOTS, 'workspace-demo.mp4'))
  } finally {
    await app.evaluate(({ app: electronApp }) => { electronApp.quit() }).catch(() => {})
    await app.close().catch(() => {})
    if (!process.env.MATOU_KEEP_DEMO_ROOT) await rm(root, { recursive: true, force: true })
    else console.log(`demo root kept at ${root}`)
  }
})

