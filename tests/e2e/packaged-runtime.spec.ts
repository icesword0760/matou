import { access, chmod, mkdir, mkdtemp, readdir, readFile, rename, rm, stat, truncate } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { _electron as electron, expect, test, type ElectronApplication } from '@playwright/test'

import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'
import { terminalCommand } from './fixtures/session-canvas-fixture'

test('packaged app runs SQLite, node-pty, replay, torn-tail recovery, and schema compatibility', async () => {
  // Four independent packaged launches exercise durable replay, torn-tail
  // recovery and forward-schema compatibility in one lifecycle. Keep the
  // timeout scoped to that real packaged journey rather than the suite default.
  test.setTimeout(60_000)
  const dataDirectory = await mkdtemp(join(tmpdir(), 'matou-packaged-e2e-'))
  const executablePath = await packagedExecutable()
  const historyMarker = 'PACKAGED_HISTORY_SURVIVES_READ_ONLY'
  try {
    await expectPackagedHostControlResources(executablePath)
    const persistedSessionId = await runPackagedSmoke(
      executablePath, dataDirectory, true, 'normal', historyMarker
    )

    const databasePath = join(dataDirectory, 'matou.sqlite')
    expect(existsSync(databasePath)).toBe(true)
    const { DatabaseSync } = process.getBuiltinModule('node:sqlite') as typeof import('node:sqlite')
    const database = new DatabaseSync(databasePath, { readOnly: true })
    expect(database.prepare('SELECT MAX(version) AS version FROM schema_migrations').get()).toEqual({
      version: FOUNDATION_MIGRATIONS.at(-1)!.version
    })
    expect(database.prepare('SELECT COUNT(*) AS count FROM window_task_placements').get())
      .toEqual({ count: 1 })
    expect(database.prepare("SELECT value FROM _runtime_meta WHERE key = 'runtime_generation'").get()).toEqual({
      value: expect.stringMatching(/^[0-9a-f-]{36}$/)
    })
    database.close()

    const journalDirectory = join(dataDirectory, 'journal', persistedSessionId)
    const activeName = (await readdir(journalDirectory))
      .filter((name) => name.endsWith('.mtj') || name.endsWith('.bin'))
      .sort()
      .at(-1)!
    const activePath = join(journalDirectory, activeName)
    const before = await readFile(activePath)
    expect(before.byteLength).toBeGreaterThan(16)
    await truncate(activePath, before.byteLength - 3)

    await runPackagedSmoke(executablePath, dataDirectory, false, 'normal', historyMarker)
    const after = await readFile(activePath)
    expect(after.byteLength).toBeGreaterThan(before.byteLength - 3)

    const newer = new DatabaseSync(databasePath)
    newer.prepare(
      'INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?, ?, ?, ?)'
    ).run(999, 'future-version', 'future-checksum', Date.now())
    newer.close()
    await runPackagedSmoke(executablePath, dataDirectory, false, 'read-only', historyMarker)
    const untouched = new DatabaseSync(databasePath)
    expect(untouched.prepare('SELECT MAX(version) AS version FROM schema_migrations').get())
      .toEqual({ version: 999 })
    untouched.prepare('DELETE FROM schema_migrations WHERE version = 999').run()
    untouched.close()
    await runPackagedSmoke(executablePath, dataDirectory, false, 'normal', historyMarker)
  } finally {
    await rm(dataDirectory, { recursive: true, force: true })
  }
})

async function runPackagedSmoke(
  executablePath: string,
  dataDirectory: string,
  exerciseProduct: boolean,
  expectedMode: 'normal' | 'read-only',
  historyMarker: string
): Promise<string> {
  await chmod(executablePath, 0o755)
  // MATOU_DEFAULT_WORKSPACE models the directory a normal macOS Terminal opens
  // into. The product no longer recreates a previously moved/missing Workspace,
  // so the packaged fixture must provide the same valid starting directory as
  // the regular Electron fixture and a real user's home directory.
  const workspaceDirectory = join(dataDirectory, 'matou_workspace')
  const homeDirectory = join(dataDirectory, 'home')
  const electronUserDataDirectory = join(dataDirectory, 'electron-user-data')
  await Promise.all([
    mkdir(workspaceDirectory, { recursive: true }),
    mkdir(homeDirectory, { recursive: true }),
    mkdir(electronUserDataDirectory, { recursive: true })
  ])
  const launchEnvironment = {
    ...process.env, MATOU_E2E: '1', MATOU_E2E_TERMINAL_DIAGNOSTICS: '1',
    MATOU_DATA_DIR: dataDirectory,
    MATOU_DEFAULT_WORKSPACE: workspaceDirectory,
    HOME: homeDirectory,
    ELECTRON_USER_DATA_DIR: electronUserDataDirectory,
    MATOU_E2E_WINDOW_CLOSE: 'hide'
  }
  const app = await electron.launch({
    executablePath,
    args: [`--user-data-dir=${electronUserDataDirectory}`],
    env: launchEnvironment
  })
  try {
    const page = await app.firstWindow()
    await expectAllVisibleWindowsOnSecondaryDisplay(app)
    await expect(page.getByRole('group', { name: 'matou_workspace 工作空间' })).toBeVisible()
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await expect(page.getByTestId('terminal-pane')).toHaveCount(exerciseProduct ? 1 : 3)
    if (expectedMode === 'normal') {
      await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
      await expect(page.getByTestId('smoke-marker')).toHaveText('__MATOU_CHANNEL_READY__')
      await expect(page.getByTestId('replay-marker')).toHaveText(/^replayed-through:\d+$/)
      await page.waitForTimeout(200)
    } else {
      await expect(page.locator('.read-only-recovery-banner'))
        .toContainText('数据库处于只读恢复模式')
      const replayOnlySurface = page.getByTestId('terminal-pane').first().locator('.terminal-surface')
      await expect(replayOnlySurface.locator('.xterm-rows')).toContainText(historyMarker)
      await expect(replayOnlySurface).not.toHaveAttribute('data-pid', /\d+/)
    }
    if (exerciseProduct) {
      const packagedSurface = page.getByTestId('terminal-pane').first().locator('.terminal-surface')
      const packagedSessionId = await packagedSurface.getAttribute('data-session-id')
      const identifyPath = join(dataDirectory, 'packaged-mt-identify.json')
      await runTerminalCommand(packagedSurface, `mt identify --json > ${identifyPath}`)
      await expect.poll(async () => {
        try {
          const result = JSON.parse(await readFile(identifyPath, 'utf8')) as {
            target?: { session?: { id?: string } }
          }
          return result.target?.session?.id
        } catch {
          return undefined
        }
      }).toBe(packagedSessionId)

      await page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2)

      const embedded = page.getByTestId('terminal-pane').first().locator('.terminal-surface')
      await expect(embedded).toHaveAttribute('data-pid', /\d+/)
      await terminalCommand(embedded, `printf '${historyMarker}\\n'`)
      await expect(embedded.locator('.xterm-rows')).toContainText(historyMarker)
      const pid = await embedded.getAttribute('data-pid')
      const detachedSessionId = await embedded.getAttribute('data-session-id')
      await page.locator('.terminal-pane-header').first()
        .dispatchEvent('dragend', { screenX: -1, screenY: -1 })
      await expect(page.getByTestId('detached-placeholder')).toContainText('已脱出')
      await expect.poll(async () => (await app.windows()).length).toBe(2)
      await expectAllVisibleWindowsOnSecondaryDisplay(app)
      const detached = (await app.windows()).find((candidate) => candidate !== page)!
      await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-pid', pid!)
      await detached.close()
      await expect(page.getByTestId('detached-placeholder')).toHaveCount(0)
      const returned = page.locator(`.terminal-surface[data-session-id="${detachedSessionId}"]`)
      await expect(returned).toBeVisible()
      await expect(returned).toHaveAttribute('data-pid', pid!)
      await expect(page.locator('.stopped-session-card')).toHaveCount(0)
      await expect(page.getByRole('button', { name: '重新启动' })).toHaveCount(0)

      const sessionsBeforeAdd = await page.locator(
        '.scene-stage:not([hidden]) .terminal-surface[data-session-id]'
      ).evaluateAll((elements) => elements.map((element) =>
        (element as HTMLElement).dataset.sessionId
      ))
      await page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(3)
      const sessionsAfterAdd = await page.locator(
        '.scene-stage:not([hidden]) .terminal-surface[data-session-id]'
      ).evaluateAll((elements) => elements.map((element) =>
        (element as HTMLElement).dataset.sessionId
      ))
      expect(sessionsAfterAdd.filter((sessionId) => !sessionsBeforeAdd.includes(sessionId)))
        .toHaveLength(1)
      expect(sessionsAfterAdd).toContain(detachedSessionId)

      const workspace = join(dataDirectory, 'matou_workspace')
      const moved = `${workspace}-moved`
      await rename(workspace, moved)
      await expect(page.getByText('路径失效')).toBeVisible({ timeout: 10_000 })
      await expect(page.getByText('工作区目录不可用，请先在本地恢复原路径，或移出该工作区').first()).toBeVisible()
      await rename(moved, workspace)
      await expect(page.getByText('路径失效')).toHaveCount(0, { timeout: 10_000 })

      await page.getByRole('button', { name: /^关闭页签：/ }).click()
      await expect(page.getByRole('alertdialog', { name: '提示' })).toContainText(
        '最后一个事项下的最后一个标签'
      )
      await page.getByRole('button', { name: '我知道了' }).click()
      await expect.poll(() => app.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()[0]?.isVisible()
      )).toBe(true)

      await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.close())
      await expect.poll(() => app.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()[0]?.isVisible()
      )).toBe(false)
      const secondLaunch = spawn(executablePath, [], {
        env: launchEnvironment, stdio: 'ignore'
      })
      const [exitCode] = await once(secondLaunch, 'exit')
      expect(exitCode).toBe(0)
      await expect.poll(() => app.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()[0]?.isVisible()
      )).toBe(true)
      await expect(page.getByTestId('terminal-pane')).toHaveCount(3)
    } else if (expectedMode === 'read-only') {
      const databasePath = join(dataDirectory, 'matou.sqlite')
      const databaseBeforeInput = await readFile(databasePath)
      const replayOnlySurface = page.getByTestId('terminal-pane').first().locator('.terminal-surface')
      const textarea = replayOnlySurface.locator('.xterm-helper-textarea')
      await textarea.focus()
      await textarea.pressSequentially('READ_ONLY_INPUT_MUST_NOT_START_A_PROCESS')
      await textarea.press('Enter')
      await page.waitForTimeout(200)
      await expect(replayOnlySurface).not.toHaveAttribute('data-pid', /\d+/)
      await expect(replayOnlySurface.locator('.xterm-rows')).not
        .toContainText('READ_ONLY_INPUT_MUST_NOT_START_A_PROCESS')
      expect((await readFile(databasePath)).equals(databaseBeforeInput)).toBe(true)
    }
    const sessionId = await page.locator(
      '.scene-stage:not([hidden]) .terminal-surface[data-session-id]'
    ).first().getAttribute('data-session-id')
    if (!sessionId) throw new Error('Packaged smoke did not expose an active persisted session')
    return sessionId
  } finally {
    await app.close()
  }
}

async function runTerminalCommand(
  surface: import('@playwright/test').Locator,
  command: string
): Promise<void> {
  await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
  await surface.click({ position: { x: 12, y: 12 } })
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(command, { delay: 1 })
  await textarea.press('Enter')
}

async function expectPackagedHostControlResources(executablePath: string): Promise<void> {
  const resources = process.platform === 'darwin'
    ? resolve(executablePath, '../../Resources/runtime')
    : resolve(executablePath, '../resources/runtime')
  const required = [
    'mt-cli.cjs',
    'control-assets/bin/mt',
    'control-assets/bin/mt.cmd',
    'control-assets/providers/host-control.md',
    'control-assets/providers/claude-plugin/.claude-plugin/plugin.json',
    'control-assets/providers/claude-plugin/skills/mt-terminal/SKILL.md',
    'control-assets/providers/codex-developer-instructions.md'
  ]
  await Promise.all(required.map((path) => access(join(resources, path))))
  if (process.platform !== 'win32') {
    expect((await stat(join(resources, 'control-assets/bin/mt'))).mode & 0o111).not.toBe(0)
  }
}

async function expectAllVisibleWindowsOnSecondaryDisplay(
  app: ElectronApplication
): Promise<void> {
  if (process.platform !== 'darwin') return
  if (process.env.MATOU_E2E_DISPLAY === 'primary') {
    await expect.poll(() => app.evaluate(({ BrowserWindow, screen }) => {
      const primary = screen.getPrimaryDisplay()
      const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
      const placements = visible.map((window) => screen.getDisplayMatching(window.getBounds()))
      return visible.length > 0 && placements.every(({ id }) => id === primary.id)
    })).toBe(true)
    return
  }
  await expect.poll(() => app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const secondaryColorLcd = screen.getAllDisplays().filter(({ id, internal, label }) =>
      id !== primary.id && (internal || /color\s*lcd|内建视网膜显示器/i.test(label)))
    const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
    const placements = visible.map((window) => screen.getDisplayMatching(window.getBounds()))
    return primary.label === 'XV272U' && secondaryColorLcd.length === 1 && visible.length > 0 &&
      placements.every(({ id }) => secondaryColorLcd.some((display) => display.id === id)) &&
      placements.every(({ id }) => id !== primary.id)
  })).toBe(true)
}

async function packagedExecutable(): Promise<string> {
  const release = resolve(import.meta.dirname, '../../apps/desktop/release')
  if (process.platform === 'darwin') {
    for (const directory of await readdir(release)) {
      const candidate = join(release, directory, '码头.app', 'Contents', 'MacOS', '码头')
      if (existsSync(candidate)) return candidate
    }
  } else if (process.platform === 'win32') {
    for (const directory of await readdir(release)) {
      const candidate = join(release, directory, '码头.exe')
      if (existsSync(candidate)) return candidate
    }
  } else {
    for (const directory of await readdir(release)) {
      const candidate = join(release, directory, 'matou')
      if (existsSync(candidate)) return candidate
    }
  }
  throw new Error(`packaged 码头 executable was not found under ${release}`)
}
