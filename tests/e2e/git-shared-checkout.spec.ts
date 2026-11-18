import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, readFile, realpath, writeFile } from 'node:fs/promises'
import { basename, join, resolve } from 'node:path'
import { promisify } from 'node:util'

import { expect, test, type Locator, type Page } from '@playwright/test'

import { activeSurface, waitForShell } from './fixtures/session-canvas-fixture'
import { launchMatou, stopMatouPreservingData, type MatouFixture } from './matou-fixture'

const exec = promisify(execFile)
const evidenceDirectory = resolve(import.meta.dirname, '../../docs/acceptance/evidence/branch-isolation')
const featureBranch = 'feature/shared-checkout'
const checkoutFile = 'checkout.txt'
const dirtyContent = 'main with uncommitted changes\n'
const featureContent = 'clean content from the feature branch\n'

test('从共享目录切分支时只隔离当前会话，保留另一会话和原目录的修改', async ({}, testInfo) => {
  test.setTimeout(120_000)
  await mkdir(evidenceDirectory, { recursive: true })
  // The runtime creates a Unix socket under this root, so use Playwright's short output path.
  await mkdir(testInfo.project.outputDir, { recursive: true })
  const root = await mkdtemp(join(await realpath(testInfo.project.outputDir), 'r-'))
  const repo = join(root, 'matou_workspace')
  await mkdir(repo)

  let fixture: MatouFixture | undefined
  const sessions: Record<string, HudSnapshot> = {}
  let stage = '准备临时 Git 仓库'
  let windowBounds: { width: number; height: number } | undefined

  async function writeEvidence(failure?: string) {
    const evidence = {
      repo,
      retainedForDebugging: failure !== undefined,
      stage,
      windowBounds,
      mainBranch: await git(repo, 'branch', '--show-current').then((value) => value.trimEnd()).catch(() => null),
      mainStatusShort: await git(repo, 'status', '--short').then((value) => value.trimEnd()).catch(() => null),
      mainFileContent: await readFile(join(repo, checkoutFile), 'utf8').catch(() => null),
      sessions,
      failure
    }
    await writeFile(join(evidenceDirectory, 'evidence.json'), `${JSON.stringify(evidence, null, 2)}\n`)
    console.log(JSON.stringify(evidence, null, 2))
  }

  try {
    await git(repo, 'init', '-b', 'main')
    await git(repo, 'config', 'user.name', 'Matou E2E')
    await git(repo, 'config', 'user.email', 'matou@example.test')
    await git(repo, 'config', 'commit.gpgSign', 'false')
    await writeFile(join(repo, checkoutFile), 'original main content\n')
    await git(repo, 'add', checkoutFile)
    await git(repo, 'commit', '-m', 'seed main')
    await git(repo, 'switch', '-c', featureBranch)
    await writeFile(join(repo, checkoutFile), featureContent)
    await git(repo, 'commit', '-am', 'seed feature content')
    await git(repo, 'switch', 'main')
    await writeFile(join(repo, checkoutFile), dirtyContent)

    stage = '从同一工作区新增事项，确认两个会话共用脏的 main'
    fixture = await launchMatou({ root })
    const { app, page } = fixture
    windowBounds = await app.evaluate(({ BrowserWindow }) => {
      const window = BrowserWindow.getAllWindows()[0]!
      window.setContentSize(1400, 850)
      const { width, height } = window.getBounds()
      return { width, height }
    })
    expect(windowBounds.width).toBeGreaterThanOrEqual(1280)
    expect(windowBounds.height).toBeGreaterThanOrEqual(820)
    const workspace = page.getByRole('group', { name: 'matou_workspace 工作空间' })
    const task = (name: string) => workspace.getByRole('listitem').filter({ has: page.getByText(name, { exact: true }) })
    const hud = page.locator('.shortcut-bar .status-info')
    const cwd = hud.locator('.status-cwd')
    const gitTrigger = hud.getByRole('button', { name: '打开 Git', exact: true })
    const surface = activeSurface(page)
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await waitForShell(surface)
    await expect(cwd).toHaveAttribute('title', repo)
    await expect(gitTrigger).toHaveText('main*')
    sessions.firstBefore = await readHud(page)

    await workspace.getByRole('button', { name: '在 matou_workspace 中新增事项', exact: true }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await expect(workspace.getByRole('listitem')).toHaveCount(2)
    await waitForShell(surface)
    await expect(cwd).toHaveAttribute('title', repo)
    await expect(gitTrigger).toHaveText('main*')
    sessions.peerBefore = await readHud(page)
    expect(sessions.firstBefore.sessionId).toBeTruthy()
    expect(sessions.peerBefore.sessionId).toBeTruthy()
    expect(sessions.peerBefore.sessionId).not.toBe(sessions.firstBefore.sessionId)

    stage = '在第一个会话打开 Git，核对深色和浅色确认框'
    await task('默认').click()
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await expect(surface).toHaveAttribute('data-session-id', sessions.firstBefore.sessionId!)
    const shell = page.locator('.hierarchy-shell')
    const themeShortcut = process.platform === 'darwin' ? 'Meta+i' : 'Control+i'
    await expect(shell).toHaveAttribute('data-theme', /^(light|dark)$/)
    if (await shell.getAttribute('data-theme') === 'light') await page.keyboard.press(themeShortcut)
    await expect(shell).toHaveAttribute('data-theme', 'dark')
    await gitTrigger.click()
    const menu = page.getByRole('dialog', { name: 'Git 与 Worktree' })
    await menu.getByRole('button', { name: featureBranch, exact: true }).click()

    const dialog = page.getByRole('alertdialog', { name: '切换分支会影响其他会话', exact: true })
    const isolate = dialog.getByRole('button', { name: '新建独立 Worktree 并切换', exact: true })
    await expect(dialog.getByRole('heading')).toHaveText('切换分支会影响其他会话')
    await expect(dialog.getByText('这个会话和另外 1 个会话共用', { exact: false })).toBeVisible()
    await expect(dialog.getByText('直接切换会把它们一起带到「feature/shared-checkout」', { exact: false })).toBeVisible()
    await expect(dialog.getByText('当前有 1 个未提交文件，新建 Worktree 后它们会留在原目录。', { exact: true })).toBeVisible()
    await expect(dialog.getByRole('button')).toHaveText(['取消', '切换整个目录', '新建独立 Worktree 并切换'])
    await expect(dialog.locator('button.dialog-primary')).toHaveCount(1)
    await expect(isolate).toHaveClass(/\bdialog-primary\b/)
    await takeScreenshot(page, 'dialog-dark.png')

    await page.keyboard.press(themeShortcut)
    await expect(shell).toHaveAttribute('data-theme', 'light')
    await expect(dialog).toBeVisible()
    await takeScreenshot(page, 'dialog-light.png')

    stage = '选择新建独立 Worktree，核对第一个会话的 HUD 与干净文件'
    await isolate.click()
    await expect(gitTrigger).toHaveText(featureBranch, { timeout: 45_000 })
    await expect(cwd).toHaveAttribute('title', /[/\\]worktrees[/\\]/, { timeout: 45_000 })
    await expect(surface).toHaveAttribute('data-session-id', sessions.firstBefore.sessionId!)
    sessions.first = await readHud(page)
    const worktree = sessions.first.cwd!
    await expect(cwd).toHaveText(`~/${basename(worktree)}`)
    await showDirectoryAndFile(surface, featureContent)
    await takeScreenshot(page, 'isolated-hud.png')
    expect((await git(worktree, 'branch', '--show-current')).trimEnd()).toBe(featureBranch)
    expect(await git(worktree, 'status', '--short')).toBe('')
    expect(await readFile(join(worktree, checkoutFile), 'utf8')).toBe(featureContent)

    stage = '切回第二个事项，核对其 HUD、文件及主目录 Git 状态'
    await task('新事项').click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await expect(surface).toHaveAttribute('data-session-id', sessions.peerBefore.sessionId!)
    await expect(cwd).toHaveAttribute('title', repo)
    await expect(cwd).toHaveText('~/matou_workspace')
    await expect(gitTrigger).toHaveText('main*')
    sessions.peer = await readHud(page)
    await showDirectoryAndFile(surface, dirtyContent)
    await takeScreenshot(page, 'peer-unchanged.png')
    expect((await git(repo, 'branch', '--show-current')).trimEnd()).toBe('main')
    expect(await git(repo, 'status', '--short')).toBe(` M ${checkoutFile}\n`)
    expect(await readFile(join(repo, checkoutFile), 'utf8')).toBe(dirtyContent)

    stage = '真实 Electron 验收通过'
    await writeEvidence()
    await fixture.close()
  } catch (error) {
    if (fixture) {
      try {
        sessions.activeAtFailure = await readHud(fixture.page)
      } catch (captureError) {
        console.error('无法读取失败时的 HUD', captureError)
      }
      await takeScreenshot(fixture.page, 'failure.png').catch((captureError) => {
        console.error('无法截取失败画面', captureError)
      })
    }
    try {
      await writeEvidence(error instanceof Error ? error.message : String(error))
    } catch (captureError) {
      console.error('无法保存失败证据', captureError)
    } finally {
      if (fixture) await stopMatouPreservingData(fixture)
    }
    throw error
  }
})

interface HudSnapshot {
  sessionId: string | null
  cwd: string | null
  text: string | null
}

async function readHud(page: Page): Promise<HudSnapshot> {
  return page.evaluate(() => ({
    sessionId: document.querySelector('.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"] .terminal-surface')
      ?.getAttribute('data-session-id') ?? null,
    cwd: document.querySelector('.shortcut-bar .status-cwd')?.getAttribute('title') ?? null,
    text: document.querySelector<HTMLElement>('.shortcut-bar .status-info')?.innerText ?? null
  }))
}

async function showDirectoryAndFile(surface: Locator, expectedContent: string): Promise<void> {
  await waitForShell(surface)
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(`pwd; cat ${checkoutFile}`)
  await textarea.press('Enter')
  await expect(surface.locator('.xterm-rows')).toContainText(expectedContent.trimEnd())
}

async function takeScreenshot(page: Page, name: string): Promise<void> {
  await page.screenshot({ path: join(evidenceDirectory, name), scale: 'css', timeout: 5_000 })
}

async function git(repo: string, ...args: string[]): Promise<string> {
  return (await exec('git', ['-C', repo, ...args])).stdout
}
