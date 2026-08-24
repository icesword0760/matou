import { chmod, mkdtemp, readdir, readFile, rm, truncate } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { _electron as electron, expect, test } from '@playwright/test'

test('packaged app runs SQLite, node-pty, replay, and torn-tail recovery', async () => {
  const dataDirectory = await mkdtemp(join(tmpdir(), 'matou-packaged-e2e-'))
  const executablePath = await packagedExecutable()
  try {
    await runPackagedSmoke(executablePath, dataDirectory)

    const databasePath = join(dataDirectory, 'matou.sqlite')
    expect(existsSync(databasePath)).toBe(true)
    const { DatabaseSync } = process.getBuiltinModule('node:sqlite') as typeof import('node:sqlite')
    const database = new DatabaseSync(databasePath, { readOnly: true })
    expect(database.prepare('SELECT MAX(version) AS version FROM schema_migrations').get()).toEqual({ version: 7 })
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

    await runPackagedSmoke(executablePath, dataDirectory)
    const after = await readFile(activePath)
    expect(after.byteLength).toBeGreaterThan(before.byteLength - 3)
  } finally {
    await rm(dataDirectory, { recursive: true, force: true })
  }
})

async function runPackagedSmoke(executablePath: string, dataDirectory: string): Promise<void> {
  await chmod(executablePath, 0o755)
  const app = await electron.launch({
    executablePath,
    args: [],
    env: { ...process.env, MATOU_E2E: '1', MATOU_DATA_DIR: dataDirectory }
  })
  try {
    const page = await app.firstWindow()
    await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
    await expect(page.getByTestId('smoke-marker')).toHaveText('__MATOU_CHANNEL_READY__')
    await expect(page.getByTestId('replay-marker')).toHaveText(/^replayed-through:\d+$/)
    await page.waitForTimeout(200)
    await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
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
