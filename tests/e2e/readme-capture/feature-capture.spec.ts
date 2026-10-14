// Records the README animations for Claude account switching and service providers:
// assets/shots/account-switch-demo.gif and assets/shots/provider-setup-demo.gif (plus .mp4 copies).
// Run: pnpm build && MATOU_README_CAPTURE=1 npx playwright test tests/e2e/readme-capture/feature-capture.spec.ts --workers=1
// MATOU_LOCALE=en drives the app in English and writes assets/shots/en instead. Add
// MATOU_E2E_DISPLAY=primary on a machine with only one display.
// Accounts come from the e2e fixture engine (scenario "readme"); every session is the stub `claude`;
// the provider key is synthetic and the flow never asks a real service for anything.
import { expect, test, type Locator, type Page } from '@playwright/test'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import {
  REPO, prepareHome, prepareShopPlatform, prepareDemo, launch, placeWindow, frameRecorder, encodeAnimation,
  visibleSurfaces, stableSurface, newSurfaceAfter, waitForShell, promoteToClaude, renameTask, renameActiveTab,
  renameSession, focusCard, launches
} from './demo-scene'
import { LOCALE, demo, runtime, ui } from './locale'

const SHOTS = process.env.MATOU_SHOTS_DIR
  ?? join(REPO, 'assets', 'shots', ...(LOCALE === 'en' ? ['en'] : []))
// At 1400 CSS px the HUD still leaves the account button clear (narrower windows let the usage text
// cover it); the GIF is scaled to 1200 px so the panel and HUD text stay legible.
const SIZE = { width: 1400, height: 860 }
const GIF_WIDTH = 1200
const shell = ui.hierarchyShell
const en = LOCALE === 'en'
// The account panel and provider dialog keep their copy inline as t(zh, en), not in the catalogs.
const text = {
  accounts: en ? 'Claude accounts' : 'Claude 账户',
  work: en ? 'Work' : '工作账户',
  personal: en ? 'Personal' : '个人账户',
  switch: en ? 'Switch' : '切换',
  confirmSwitch: en ? 'Switch now' : '立即切换',
  restartNow: en ? 'Restart now' : '立即重启',
  restartTitle: en ? 'Restart all running Claude session windows?' : '确定重启所有正在运行的 Claude 会话窗口吗？',
  confirm: en ? 'Confirm' : '确定',
  restarted: en ? /Restarted and resumed \d+ running Claude sessions/ : /已重启并接续 \d+ 个正在运行的 Claude 会话/,
  closeAccounts: en ? 'Close accounts' : '关闭账户面板',
  providersHeading: en ? 'Service providers' : '服务供应商',
  addProvider: en ? 'Add provider' : '添加供应商',
  next: en ? 'Next' : '下一步',
  configure: en ? 'Configure DeepSeek' : '配置 DeepSeek',
  moreSettings: en ? 'More settings' : '更多设置',
  addAndUse: en ? 'Add and use' : '添加并使用'
}
const ROLES = ['review', 'docs', 'baseline'] as const
const CARDS = [demo.cards.review, demo.cards.docs, demo.cards.baseline]

const account = (page: Page, name: string): Locator => page.locator('.ca-account').filter({ hasText: name })
const rest = (page: Page): Promise<void> => page.mouse.move(6, SIZE.height / 2)
// MATOU_STAGE_DIR=<dir> saves a full-window screenshot at each named checkpoint, for debugging the scene.
const stage = async (page: Page, name: string): Promise<void> => {
  if (process.env.MATOU_STAGE_DIR) await page.screenshot({ path: join(process.env.MATOU_STAGE_DIR, `${name}.png`) })
}

test.setTimeout(300_000)

test('captures the account switching and service provider animations', async () => {
  test.skip(process.env.MATOU_README_CAPTURE !== '1', 'set MATOU_README_CAPTURE=1 to regenerate the feature animations')
  const root = await mkdtemp(join(tmpdir(), 'matou-readme-features-'))
  const home = join(root, 'home')
  const workspace = join(home, 'work', 'shop-api')
  const demoRoot = join(root, 'demo')
  await prepareHome(home, demoRoot)
  await prepareShopPlatform(workspace)
  await prepareDemo(demoRoot)
  // Only finished roles: an account switch waits for running Matou tasks, which would stall the demo.
  await writeFile(join(demoRoot, 'roles.queue'), ROLES.join('\n') + '\n')

  const app = await launch({
    root, home, workspace, demo: demoRoot,
    env: {
      MATOU_E2E_ACCOUNTS_BRIDGE: resolve(REPO, 'tests/e2e/fixtures/claude-accounts-engine.py'),
      MATOU_E2E_ACCOUNTS_SCENARIO: 'readme',
      MATOU_DEMO_QUIET_RESUME: '1',
      // The HUD only shows the device account's quota for sessions on the default Claude login.
      CLAUDE_CONFIG_DIR: '', CLAUDE_SECURESTORAGE_CONFIG_DIR: '', CLAUDE_CODE_OAUTH_TOKEN: '',
      ANTHROPIC_API_KEY: '', ANTHROPIC_AUTH_TOKEN: '', ANTHROPIC_BASE_URL: ''
    }
  })
  try {
    const page = await app.firstWindow()
    await placeWindow(app, 1, SIZE)

    // ----- scene: one task, three finished Claude sessions -----
    await expect(page.getByTestId('active-task')).toHaveText(runtime.hierarchy.defaultTask)
    await renameTask(page, runtime.hierarchy.defaultTask, demo.tasks.idempotency)
    await renameActiveTab(page, demo.canvases.implement)
    const surfaces: Locator[] = [await stableSurface(visibleSurfaces(page).first())]
    for (let index = 1; index < CARDS.length; index += 1) {
      surfaces.push(await newSurfaceAfter(page, () => page.getByRole('button', { name: shell.sceneTabBar.addShell }).click()))
    }
    for (const [index, title] of CARDS.entries()) {
      await waitForShell(surfaces[index]!)
      await promoteToClaude(surfaces[index]!, demoRoot)
      await renameSession(page, surfaces[index]!, title)
    }
    await focusCard(surfaces[0]!)
    // The default account's name sits on the account button; the HUD shows that account's quota.
    await expect(page.locator('.ca-trigger-label')).toHaveText(text.work)
    await expect(page.locator('.status-usage')).toContainText(['5h 94%', 'Weekly 81%'])

    await stage(page, '01-scene')
    await rest(page)
    await page.waitForTimeout(800)

    // ----- animation 1: the default account is nearly out of quota; switch and resume -----
    const accounts = frameRecorder(app, join(root, 'frames-accounts'))
    await accounts.hold(1800)
    await page.getByRole('button', { name: text.accounts, exact: true }).click()
    await expect(page.locator('.ca-account')).toHaveCount(2)
    await rest(page)
    await accounts.hold(2400)
    // The first switch explains that it changes this Mac's default login, not just one card.
    await account(page, text.personal).getByRole('button', { name: text.switch, exact: true }).click()
    await expect(page.getByRole('button', { name: text.confirmSwitch, exact: true })).toBeVisible()
    await rest(page)
    await accounts.hold(2600)
    await page.getByRole('button', { name: text.confirmSwitch, exact: true }).click()
    await expect(page.locator('.ca-switch-feedback')).toBeVisible()
    await expect(page.locator('.ca-trigger-label')).toHaveText(text.personal)
    await expect(page.locator('.status-usage')).toContainText(['5h 6%', 'Weekly 22%'])
    await rest(page)
    await accounts.hold(3000)
    await page.getByRole('button', { name: text.restartNow, exact: true }).click()
    const confirm = page.getByRole('alertdialog', { name: text.restartTitle })
    await expect(confirm).toBeVisible()
    await rest(page)
    await accounts.hold(2000)
    const before = await launches(demoRoot)
    await confirm.getByRole('button', { name: text.confirm, exact: true }).click()
    await rest(page)
    await Promise.all([
      accounts.hold(2600),
      expect.poll(() => launches(demoRoot), { timeout: 60_000 }).toBe(before + CARDS.length)
    ])
    await expect(page.locator('.ca-restart-result')).toContainText(text.restarted, { timeout: 60_000 })
    await accounts.hold(2600)
    await page.getByRole('button', { name: text.closeAccounts }).click()
    await expect(page.locator('.ca-panel')).toHaveCount(0)
    await rest(page)
    await accounts.hold(1800)
    await encodeAnimation(await accounts.writeConcatList(),
      join(SHOTS, 'account-switch-demo.gif'), join(SHOTS, 'account-switch-demo.mp4'), GIF_WIDTH)

    // ----- animation 2: add DeepSeek from a preset and switch every running session to it -----
    await focusCard(surfaces[0]!)
    await rest(page)
    const providers = frameRecorder(app, join(root, 'frames-providers'))
    await providers.hold(1400)
    await page.getByRole('button', { name: shell.taskSidebar.settings, exact: true }).click()
    await expect(page.getByRole('heading', { name: text.providersHeading })).toBeVisible()
    await rest(page)
    await providers.hold(1600)
    await page.getByRole('button', { name: shell.modelSettings.newProvider }).click()
    let dialog = page.getByRole('dialog', { name: text.addProvider })
    await expect(dialog).toBeVisible()
    await rest(page)
    await providers.hold(1600)
    await dialog.getByRole('button', { name: /DeepSeek/ }).click()
    await providers.hold(600)
    await dialog.getByRole('button', { name: text.next }).click()
    dialog = page.getByRole('dialog', { name: text.configure })
    const key = dialog.getByLabel('API Key', { exact: true })
    await expect(key).toBeFocused()
    await rest(page)
    await providers.hold(900)
    await Promise.all([key.pressSequentially('sk-demo-4f9c2a7e1b8d', { delay: 45 }), providers.hold(1300)])
    await dialog.locator('summary', { hasText: text.moreSettings }).click()
    // Bring the opus / sonnet / haiku mapping into view; the action buttons stay pinned below it.
    await dialog.locator('.provider-mapping').last().scrollIntoViewIfNeeded()
    await rest(page)
    await providers.hold(2000)
    await dialog.getByRole('button', { name: text.addAndUse }).click()
    await expect(dialog).toHaveCount(0, { timeout: 30_000 })
    await rest(page)
    await providers.hold(3000)
    await page.getByRole('button', { name: shell.modelSettings.close }).click()
    await focusCard(surfaces[0]!)
    await rest(page)
    await providers.hold(2400)
    await encodeAnimation(await providers.writeConcatList(),
      join(SHOTS, 'provider-setup-demo.gif'), join(SHOTS, 'provider-setup-demo.mp4'), GIF_WIDTH)
  } finally {
    await app.evaluate(({ app: electronApp }) => { electronApp.quit() }).catch(() => {})
    await app.close().catch(() => {})
    if (!process.env.MATOU_KEEP_DEMO_ROOT) await rm(root, { recursive: true, force: true })
    else console.log(`demo root kept at ${root}`)
  }
})
