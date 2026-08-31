import { chmod, mkdir, mkdtemp, readdir, readFile, rename, rm, truncate } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { _electron as electron, expect, test } from '@playwright/test'

import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'

test('packaged app runs SQLite, node-pty, replay, torn-tail recovery, and schema compatibility', async () => {
  // Four independent packaged launches exercise durable replay, torn-tail
  // recovery and forward-schema compatibility in one lifecycle. Keep the
  // timeout scoped to that real packaged journey rather than the suite default.
  test.setTimeout(60_000)
  const dataDirectory = await mkdtemp(join(tmpdir(), 'matou-packaged-e2e-'))
  const executablePath = await packagedExecutable()
  try {
    await runPackagedSmoke(executablePath, dataDirectory, true)

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

    const journalDirectory = join(dataDirectory, 'journal', 'foundation-shell')
    const activeName = (await readdir(journalDirectory)).filter((name) => name.endsWith('.bin')).sort().at(-1)!
    const activePath = join(journalDirectory, activeName)
    const before = await readFile(activePath)
    expect(before.byteLength).toBeGreaterThan(16)
    await truncate(activePath, before.byteLength - 3)

    await runPackagedSmoke(executablePath, dataDirectory, false)
    const after = await readFile(activePath)
    expect(after.byteLength).toBeGreaterThan(before.byteLength - 3)

    const newer = new DatabaseSync(databasePath)
    newer.prepare(
      'INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?, ?, ?, ?)'
    ).run(999, 'future-version', 'future-checksum', Date.now())
    newer.close()
    await runPackagedSmoke(executablePath, dataDirectory, false)
    const untouched = new DatabaseSync(databasePath)
    expect(untouched.prepare('SELECT MAX(version) AS version FROM schema_migrations').get())
      .toEqual({ version: 999 })
    untouched.prepare('DELETE FROM schema_migrations WHERE version = 999').run()
    untouched.close()
    await runPackagedSmoke(executablePath, dataDirectory, false)
  } finally {
    await rm(dataDirectory, { recursive: true, force: true })
  }
})

async function runPackagedSmoke(
  executablePath: string,
  dataDirectory: string,
  exerciseProduct: boolean
): Promise<void> {
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
    ...process.env, MATOU_E2E: '1', MATOU_DATA_DIR: dataDirectory,
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
    await expect(page.getByRole('group', { name: 'matou_workspace 工作空间' })).toBeVisible()
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    await expect(page.getByTestId('terminal-pane')).toHaveCount(exerciseProduct ? 1 : 2)
    await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
    await expect(page.getByTestId('smoke-marker')).toHaveText('__MATOU_CHANNEL_READY__')
    await expect(page.getByTestId('replay-marker')).toHaveText(/^replayed-through:\d+$/)
    await page.waitForTimeout(200)
    await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
    if (exerciseProduct) {
      await page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2)

      const embedded = page.getByTestId('terminal-pane').first().locator('.terminal-surface')
      await expect(embedded).toHaveAttribute('data-pid', /\d+/)
      const pid = await embedded.getAttribute('data-pid')
      const detachedSessionId = await embedded.getAttribute('data-session-id')
      await page.locator('.terminal-pane-header').first()
        .dispatchEvent('dragend', { screenX: -1, screenY: -1 })
      await expect(page.getByTestId('detached-placeholder')).toContainText('已脱出')
      await expect.poll(async () => (await app.windows()).length).toBe(2)
      const detached = (await app.windows()).find((candidate) => candidate !== page)!
      await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-pid', pid!)
      await detached.close()
      await expect(page.getByTestId('detached-placeholder')).toHaveCount(0)
      await expect(page.locator(`.terminal-surface[data-session-id="${detachedSessionId}"]`)).toHaveCount(0)
      await expect(page.locator('.stopped-session-card')).toContainText('已停止')
      await expect(page.getByRole('button', { name: '重新启动' })).toHaveCount(0)

      await page.getByRole('button', { name: '横向新增 Shell' }).click()
      const continued = page.locator('.scene-stage:not([hidden]) .terminal-surface').first()
      await expect(continued).toHaveAttribute('data-session-id', /.+/)
      await expect(continued).not.toHaveAttribute('data-session-id', detachedSessionId!)
      await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2)

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
      await expect(page.getByTestId('terminal-pane')).toHaveCount(2)
    }
  } finally {
    await app.close()
  }
}

async function packagedExecutable(): Promise<string> {
  const release = resolve(import.meta.dirname, '../../apps/desktop/release')
  if (process.platform === 'darwin') {
    for (const directory of await readdir(release)) {
      const candidate = join(release, directory, 'Matou.app', 'Contents', 'MacOS', 'Matou')
      if (existsSync(candidate)) return candidate
    }
  } else if (process.platform === 'win32') {
    for (const directory of await readdir(release)) {
      const candidate = join(release, directory, 'Matou.exe')
      if (existsSync(candidate)) return candidate
    }
  } else {
    for (const directory of await readdir(release)) {
      const candidate = join(release, directory, 'matou')
      if (existsSync(candidate)) return candidate
    }
  }
  throw new Error(`packaged Matou executable was not found under ${release}`)
}
