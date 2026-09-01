import { execFile } from 'node:child_process'
import { chmod, readdir, readFile } from 'node:fs/promises'
import { join, relative } from 'node:path'
import { promisify } from 'node:util'

import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

test('detaches a live terminal, then returns the same live node when its native window closes', async () => {
  const fixture = await launchMatou()
  try {
    const { app, page } = fixture
    const embedded = page.getByTestId('terminal-pane').first().locator('.terminal-surface')
    await expect(embedded).toHaveAttribute('data-pid', /\d+/)
    const originalPid = await embedded.getAttribute('data-pid')
    const sessionId = await embedded.getAttribute('data-session-id')
    await page.locator('.terminal-pane-header').first().dispatchEvent('dragend', { screenX: -1, screenY: -1 })

    await expect(page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const detached = (await app.windows()).find((candidate) => candidate !== page)!
    await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-pid', originalPid!)

    await detached.close()
    await expect(page.getByTestId('detached-placeholder')).toHaveCount(0)
    await expect(page.locator('.stopped-session-card')).toHaveCount(0)
    const returned = page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
    await expect(returned).toHaveAttribute('data-pid', /\d+/)
    await expect(returned.locator('.xterm-helper-textarea')).toBeAttached()
  } finally { await fixture.close() }
})

test('returns a detached terminal to its Scene instead of reopening a temporary window after restart', async () => {
  let fixture: MatouFixture = await launchMatou()
  try {
    await fixture.page.locator('.terminal-pane-header').first().dispatchEvent('dragend', { screenX: -1, screenY: -1 })
    await expect(fixture.page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)

    fixture = await restartMatou(fixture)
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
    await expect(fixture.page.getByTestId('detached-placeholder')).toHaveCount(0)
    await expect(fixture.page.getByTestId('terminal-pane')).toHaveCount(1)
    await expect(fixture.page.getByTestId('terminal-pane').locator('.terminal-surface'))
      .toHaveAttribute('data-pid', /\d+/)
  } finally { await fixture.close() }
})

test('returns persisted detached history to one read-only main window for search and copy', async () => {
  test.skip(process.platform === 'win32', 'POSIX permissions fixture')
  test.setTimeout(60_000)
  let fixture: MatouFixture = await launchMatou()
  let permissionsRestricted = false
  try {
    const embedded = fixture.page.getByTestId('terminal-pane').first().locator('.terminal-surface')
    await expect(embedded).toHaveAttribute('data-pid', /\d+/)
    const textarea = embedded.locator('.xterm-helper-textarea')
    await textarea.focus()
    await expect(textarea).toBeFocused()
    await fixture.page.waitForTimeout(100)
    await textarea.press('Control+u')
    await textarea.pressSequentially("printf 'MATOU_%s_HISTORY_COPY\\n' READONLY", { delay: 2 })
    await textarea.press('Enter')
    await expect(embedded.locator('.xterm-rows')).toContainText('MATOU_READONLY_HISTORY_COPY')
    await fixture.page.locator('.terminal-pane-header').first()
      .dispatchEvent('dragend', { screenX: -1, screenY: -1 })
    await expect(fixture.page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)

    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close().catch(() => undefined)
    await setReadOnlyTree(fixture.dataDirectory)
    permissionsRestricted = true
    fixture = await restartMatou(fixture)

    await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
    await expect(fixture.page.locator('.read-only-recovery-banner'))
      .toContainText('数据库处于只读恢复模式')
    await expect(fixture.page.getByTestId('detached-placeholder')).toHaveCount(0)
    const recovered = fixture.page.locator('.scene-stage:not([hidden]) .terminal-surface').first()
    await expect(recovered).toBeVisible()
    await expect(recovered).not.toHaveAttribute('data-pid', /\d+/)
    await expect(recovered.locator('.xterm-rows')).toContainText('MATOU_READONLY_HISTORY_COPY')

    const mod = process.platform === 'darwin' ? 'Meta' : 'Control'
    await fixture.page.getByRole('button', { name: '搜索当前终端' }).click()
    const search = fixture.page.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' })
    await expect(search).toBeVisible()
    await search.fill('MATOU_READONLY_HISTORY_COPY')
    await expect(fixture.page.locator('.terminal-search-bar__count')).toHaveText('1/1')
    await search.press('Escape')

    const markerRow = recovered.locator('.xterm-rows > div').filter({
      hasText: 'MATOU_READONLY_HISTORY_COPY'
    }).last()
    const row = await markerRow.boundingBox()
    if (!row) throw new Error('Expected replayed marker row geometry')
    await fixture.page.mouse.move(row.x + 1, row.y + row.height / 2)
    await fixture.page.mouse.down()
    await fixture.page.mouse.move(row.x + row.width - 1, row.y + row.height / 2, { steps: 8 })
    await fixture.page.mouse.up()
    await fixture.page.keyboard.press(`${mod}+c`)
    await expect.poll(() => fixture.app.evaluate(({ clipboard }) => clipboard.readText()))
      .toContain('MATOU_READONLY_HISTORY_COPY')
  } finally {
    if (permissionsRestricted) await restoreWritableTree(fixture.dataDirectory).catch(() => undefined)
    await fixture.close()
  }
})

test('returns a live detached window to replay-only history after Runtime enters read-only', async () => {
  test.skip(process.platform === 'win32', 'POSIX permissions and process fixture')
  test.setTimeout(60_000)
  const fixture = await launchMatou()
  let permissionsRestricted = false
  try {
    const embedded = fixture.page.getByTestId('terminal-pane').first().locator('.terminal-surface')
    await expect(embedded).toHaveAttribute('data-pid', /\d+/)
    const sessionId = await embedded.getAttribute('data-session-id')
    const textarea = embedded.locator('.xterm-helper-textarea')
    await textarea.focus()
    await expect(textarea).toBeFocused()
    await textarea.press('Control+u')
    await textarea.pressSequentially("printf 'MATOU_LIVE_READONLY_CLOSE\\n'", { delay: 2 })
    await textarea.press('Enter')
    await expect(embedded.locator('.xterm-rows')).toContainText('MATOU_LIVE_READONLY_CLOSE')

    await fixture.page.locator('.terminal-pane-header').first()
      .dispatchEvent('dragend', { screenX: -1, screenY: -1 })
    await expect(fixture.page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
    const detached = (await fixture.app.windows()).find((candidate) => candidate !== fixture.page)!
    await expect(detached.locator('.terminal-surface')).toContainText('MATOU_LIVE_READONLY_CLOSE')

    await setReadOnlyTree(fixture.dataDirectory)
    permissionsRestricted = true
    const runtimePid = await findRuntimePid(fixture.app.process().pid)
    process.kill(runtimePid, 'SIGKILL')

    await expect(fixture.page.locator('.read-only-recovery-banner'))
      .toContainText('数据库处于只读恢复模式')
    await expect(detached.locator('.read-only-recovery-banner'))
      .toContainText('数据库处于只读恢复模式')
    await expect(fixture.page.getByTestId('detached-placeholder')).toBeVisible()
    const detachedHistory = detached.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
    await expect(detachedHistory).not.toHaveAttribute('data-pid', /\d+/)
    await expect(detachedHistory.locator('.xterm-rows')).toContainText('MATOU_LIVE_READONLY_CLOSE')
    const beforeClose = await snapshotFiles(fixture.dataDirectory)

    await detached.close()
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
    await expect(fixture.page.getByTestId('detached-placeholder')).toHaveCount(0)
    const returned = fixture.page.locator(
      `.scene-stage:not([hidden]) .terminal-surface[data-session-id="${sessionId}"]`
    )
    await expect(returned).toBeVisible()
    await expect(returned).not.toHaveAttribute('data-pid', /\d+/)
    await expect(returned.locator('.xterm-rows')).toContainText('MATOU_LIVE_READONLY_CLOSE')

    await fixture.page.getByRole('button', { name: '搜索当前终端' }).click()
    const search = fixture.page.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' })
    await search.fill('MATOU_LIVE_READONLY_CLOSE')
    await expect(fixture.page.locator('.terminal-search-bar__count')).toHaveText('1/1')
    await search.press('Escape')

    const markerRow = returned.locator('.xterm-rows > div').filter({
      hasText: 'MATOU_LIVE_READONLY_CLOSE'
    }).last()
    const row = await markerRow.boundingBox()
    if (!row) throw new Error('Expected returned marker row geometry')
    await fixture.page.mouse.move(row.x + 1, row.y + row.height / 2)
    await fixture.page.mouse.down()
    await fixture.page.mouse.move(row.x + row.width - 1, row.y + row.height / 2, { steps: 8 })
    await fixture.page.mouse.up()
    await fixture.page.keyboard.press(`${process.platform === 'darwin' ? 'Meta' : 'Control'}+c`)
    await expect.poll(() => fixture.app.evaluate(({ clipboard }) => clipboard.readText()))
      .toContain('MATOU_LIVE_READONLY_CLOSE')
    expect(await snapshotFiles(fixture.dataDirectory)).toEqual(beforeClose)
  } finally {
    if (permissionsRestricted) await restoreWritableTree(fixture.dataDirectory).catch(() => undefined)
    await fixture.close()
  }
})

const execFileAsync = promisify(execFile)

async function findRuntimePid(electronPid: number): Promise<number> {
  let runtimePid = 0
  await expect.poll(async () => {
    const { stdout } = await execFileAsync('ps', ['-axo', 'pid=,ppid=,command='])
    for (const line of stdout.split('\n')) {
      const match = line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/)
      if (!match || Number(match[2]) !== electronPid) continue
      if (match[3]!.includes('--utility-sub-type=node.mojom.NodeService')) {
        runtimePid = Number(match[1])
        break
      }
    }
    return runtimePid
  }).not.toBe(0)
  return runtimePid
}

async function snapshotFiles(root: string): Promise<Record<string, string>> {
  const snapshot: Record<string, string> = {}
  const visit = async (directory: string): Promise<void> => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) {
        await visit(path)
      } else if (entry.isFile()) {
        snapshot[relative(root, path)] = (await readFile(path)).toString('base64')
      }
    }
  }
  await visit(root)
  return snapshot
}

async function setReadOnlyTree(root: string): Promise<void> {
  const visit = async (directory: string): Promise<void> => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) {
        await visit(path)
        await chmod(path, 0o500)
      } else if (entry.isFile()) {
        await chmod(path, 0o400)
      }
    }
  }
  await visit(root)
  await chmod(root, 0o500)
}

async function restoreWritableTree(root: string): Promise<void> {
  await chmod(root, 0o700)
  const visit = async (directory: string): Promise<void> => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) {
        await chmod(path, 0o700)
        await visit(path)
      } else if (entry.isFile()) {
        await chmod(path, 0o600)
      }
    }
  }
  await visit(root)
}
