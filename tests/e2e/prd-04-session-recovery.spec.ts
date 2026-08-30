import { execFile } from 'node:child_process'
import { chmod, mkdir, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'

import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

const execFileAsync = promisify(execFile)

test('restores the work structure, cwd, and existing Shell history', async () => {
  let fixture: MatouFixture = await launchMatou()
  const sessionDirectory = join(fixture.workspaceDirectory, 'session-directory')
  await mkdir(sessionDirectory)
  try {
    await fixture.page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
    await fixture.page.getByRole('button', { name: '新建页签' }).click()
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    const activeSurface = activeSurfaceFor(fixture)
    await positivePid(activeSurface)
    await typeTerminalCommand(
      activeSurface,
      `cd '${sessionDirectory}' && printf '__PRD04_OLD_SHELL_OUTPUT__\\n' && pwd`
    )
    await expect(activeSurface.locator('.xterm-rows')).toContainText('__PRD04_OLD_SHELL_OUTPUT__')
    await expect(activeSurface.locator('.xterm-rows')).toContainText(sessionDirectory)
    expect(await activeSurface.locator('.xterm-viewport').evaluate((element) => {
      const style = getComputedStyle(element)
      return { backgroundColor: style.backgroundColor, overflowY: style.overflowY }
    })).toEqual({ backgroundColor: 'rgba(0, 0, 0, 0)', overflowY: 'auto' })
    const originalPid = await positivePid(activeSurface)
    await expect.poll(async () => realpath(await processCwd(originalPid)))
      .toBe(await realpath(sessionDirectory))

    fixture = await restartMatou(fixture)
    await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
    await expect(fixture.page.getByRole('tab')).toHaveCount(2)
    await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2)
    const restoredSurface = activeSurfaceFor(fixture)
    const restoredPid = await positivePid(restoredSurface)
    expect(restoredPid).not.toBe(originalPid)
    await expect(restoredSurface.locator('.xterm-rows'))
      .toContainText('__PRD04_OLD_SHELL_OUTPUT__')
    await expect.poll(async () => realpath(await processCwd(Number(restoredPid))))
      .toBe(await realpath(sessionDirectory))
  } finally {
    await fixture.close()
  }
})

test('hides and shows the main window without restarting its live terminal', async () => {
  const fixture = await launchMatou({ preserveMainWindowCloseBehavior: true })
  try {
    const surface = visibleSurfaces(fixture).first()
    const originalPid = await positivePid(surface)
    const windowId = new URL(fixture.page.url()).searchParams.get('windowId')!
    await typeTerminalCommand(surface, "printf '%s\\n' \"$((321 + 654))\"")
    await expect(surface.locator('.xterm-rows')).toContainText('975')

    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.close())
    await expect.poll(() => fixture.app.evaluate(({ BrowserWindow }) =>
      BrowserWindow.getAllWindows()[0]?.isVisible()
    )).toBe(false)
    await fixture.page.evaluate((id) => window.matouDesktop.showWindow(id), windowId)
    await expect.poll(() => fixture.app.evaluate(({ BrowserWindow }) =>
      BrowserWindow.getAllWindows()[0]?.isVisible()
    )).toBe(true)
    await expect(surface).toHaveAttribute('data-pid', String(originalPid))
    await expect(surface.locator('.xterm-rows')).toContainText('975')
    await expect(surface.locator('.xterm-helper-textarea')).toBeFocused()
    await typeTerminalCommand(surface, "printf '%s\\n' \"$((111 + 222))\"")
    await expect(surface.locator('.xterm-rows')).toContainText('333')
  } finally {
    await fixture.close()
  }
})

test('falls back from a deleted Session directory to the surviving Workspace root', async () => {
  let fixture: MatouFixture = await launchMatou()
  const removedDirectory = join(fixture.workspaceDirectory, 'removed-session-directory')
  await mkdir(removedDirectory)
  try {
    const surface = visibleSurfaces(fixture).first()
    await positivePid(surface)
    await typeTerminalCommand(surface, `cd '${removedDirectory}' && pwd`)
    await expect(surface.locator('.xterm-rows')).toContainText(removedDirectory)
    await expect.poll(async () => realpath(await processCwd(await positivePid(surface))))
      .toBe(await realpath(removedDirectory))

    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close().catch(() => undefined)
    await rm(removedDirectory, { recursive: true, force: true })
    fixture = await restartMatou(fixture)

    const restored = visibleSurfaces(fixture).first()
    const restoredPid = await positivePid(restored)
    await expect.poll(async () => realpath(await processCwd(restoredPid)))
      .toBe(await realpath(fixture.workspaceDirectory))
    await typeTerminalCommand(restored, "printf '%s\\n' \"$((700 + 7))\"")
    await expect(restored.locator('.xterm-rows')).toContainText('707')
  } finally {
    await fixture.close()
  }
})

test('restores a valid AI conversation identity and keeps its resumed terminal interactive', async () => {
  let fixture: MatouFixture = await launchMatou()
  const providerExecutable = join(fixture.rootDirectory, 'resumable-provider.sh')
  const invocations = join(fixture.rootDirectory, 'resumable-provider-invocations.txt')
  try {
    await positivePid(visibleSurfaces(fixture).first())
    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close()
    seedProviderResume(join(fixture.dataDirectory, 'matou.sqlite'), {
      bindingId: 'e2e-valid-provider-binding',
      providerSessionId: 'remembered-provider-session',
      permissionMode: 'bypassPermissions'
    })
    await writeFile(providerExecutable, [
      '#!/bin/sh',
      'printf "%s\\n" "$*" >> "$MATOU_TEST_PROVIDER_INVOCATIONS"',
      "printf '%02050d' 0",
      'printf "\\nRESTORED_CONTEXT: yesterday we agreed on the blue deployment plan\\n"',
      'while IFS= read -r line; do',
      '  case "$line" in',
      '    *continue*) printf "CONTEXT_CONFIRMED: blue deployment plan\\n" ;;',
      '    *) printf "PROVIDER_INPUT: %s\\n" "$line" ;;',
      '  esac',
      'done',
      ''
    ].join('\n'))
    await chmod(providerExecutable, 0o755)

    fixture = await restartMatou(fixture, { env: {
      MATOU_CLAUDE_COMMAND: providerExecutable,
      MATOU_TEST_PROVIDER_INVOCATIONS: invocations
    } })
    const resumed = visibleSurfaces(fixture).first()
    await positivePid(resumed)
    await expect(resumed.locator('.xterm-rows')).toContainText(
      'RESTORED_CONTEXT: yesterday we agreed on the blue deployment plan'
    )
    await typeTerminalCommand(resumed, 'continue')
    await expect(resumed.locator('.xterm-rows')).toContainText(
      'CONTEXT_CONFIRMED: blue deployment plan'
    )
    const invocation = await readFile(invocations, 'utf8')
    expect(invocation).toContain('--resume remembered-provider-session')
    expect(invocation).toContain('--dangerously-skip-permissions')
  } finally {
    await fixture.close()
  }
})

test('starts silently with a clean work scene when the entire durable database is corrupt', async () => {
  let fixture: MatouFixture = await launchMatou()
  try {
    await fixture.page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
    await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close().catch(() => undefined)
    const databasePath = join(fixture.dataDirectory, 'matou.sqlite')
    await writeFile(databasePath, 'not a sqlite database')
    await rm(`${databasePath}-wal`, { force: true })
    await rm(`${databasePath}-shm`, { force: true })

    fixture = await restartMatou(fixture)

    await expect(fixture.page.getByTestId('active-task')).toHaveText('默认')
    await expect(fixture.page.getByText(/正在恢复|是否恢复|恢复失败/)).toHaveCount(0)
    expect((await readdir(fixture.dataDirectory)).some((name) =>
      name.startsWith('matou.sqlite.corrupt-')
    )).toBe(true)
  } finally {
    await fixture.close()
  }
})

test('keeps one corrupt terminal journal isolated while the remaining work scene stays usable', async () => {
  let fixture: MatouFixture = await launchMatou()
  try {
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    const before = visibleSurfaces(fixture)
    await expect(before).toHaveCount(2)
    const corruptSessionId = await before.first().getAttribute('data-session-id')
    if (!corruptSessionId) throw new Error('Expected the first Session identity')
    const healthySessionId = await before.last().getAttribute('data-session-id')
    if (!healthySessionId) throw new Error('Expected the second Session identity')
    const corruptSurface = fixture.page.locator(`.terminal-surface[data-session-id="${corruptSessionId}"]`)
    const healthySurface = fixture.page.locator(`.terminal-surface[data-session-id="${healthySessionId}"]`)
    await typeTerminalCommand(corruptSurface, "printf '%s\\n' \"$((1200 + 34))\"")
    await expect(corruptSurface.locator('.xterm-rows')).toContainText('1234')
    await typeTerminalCommand(healthySurface, "printf '%s\\n' \"$((5600 + 78))\"")
    await expect(healthySurface.locator('.xterm-rows')).toContainText('5678')

    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close().catch(() => undefined)
    const journalDirectory = join(fixture.dataDirectory, 'journal', corruptSessionId)
    const segmentName = (await readdir(journalDirectory)).find((name) => name.endsWith('.bin'))
    if (!segmentName) throw new Error('Expected an active terminal journal segment')
    const segmentPath = join(journalDirectory, segmentName)
    const bytes = await readFile(segmentPath)
    if (bytes.length < 21) throw new Error('Journal fixture is too short to corrupt')
    bytes[20] = bytes[20]! ^ 0xff
    await writeFile(segmentPath, bytes)

    fixture = await restartMatou(fixture)
    const restored = visibleSurfaces(fixture)
    await expect(restored).toHaveCount(2)
    const recoveredCorrupt = fixture.page.locator(`.terminal-surface[data-session-id="${corruptSessionId}"]`)
    const recoveredHealthy = fixture.page.locator(`.terminal-surface[data-session-id="${healthySessionId}"]`)
    await positivePid(recoveredCorrupt)
    await positivePid(recoveredHealthy)
    await typeTerminalCommand(recoveredCorrupt, "printf '%s\\n' \"$((40 + 2))\"")
    await expect(recoveredCorrupt.locator('.xterm-rows')).toContainText('42')
    await typeTerminalCommand(recoveredHealthy, "printf '%s\\n' \"$((80 + 4))\"")
    await expect(recoveredHealthy.locator('.xterm-rows')).toContainText('84')
    expect((await readdir(journalDirectory)).some((name) => name.includes('.corrupt-'))).toBe(true)
  } finally {
    await fixture.close()
  }
})

test('does not resurrect an explicitly removed Task, Scene, or terminal panel', async () => {
  let fixture: MatouFixture = await launchMatou()
  try {
    const { page } = fixture
    await page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项')
    await page.getByRole('button', { name: '新建页签' }).click()
    await page.getByRole('button', { name: '横向新增 Shell' }).click()
    const visiblePanels = page.locator('[data-testid="terminal-pane"]:visible')
    await expect(visiblePanels).toHaveCount(2)
    const removedSessionId = await visiblePanels.last().locator('.terminal-surface')
      .getAttribute('data-session-id')
    if (!removedSessionId) throw new Error('Expected the new Session identity')
    const removedPanel = page.locator('[data-testid="terminal-pane"]:visible').filter({
      has: page.locator(`.terminal-surface[data-session-id="${removedSessionId}"]`)
    })
    await removedPanel.getByRole('button', { name: /^删除终端：/ }).click()
    await expect(visiblePanels).toHaveCount(1)
    await page.getByRole('button', { name: /^关闭页签：/ }).last().click()
    await expect(page.getByRole('tab')).toHaveCount(1)

    await page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
    await expect(page.getByTestId('active-task')).toHaveText('新事项 2')
    await page.getByRole('button', { name: '事项菜单：新事项 2' }).click()
    await page.getByRole('menuitem', { name: '删除' }).click()
    await page.getByRole('button', { name: '确定' }).click()
    await expect(page.getByText('新事项 2', { exact: true })).toHaveCount(0)

    fixture = await restartMatou(fixture)
    await expect(fixture.page.getByText('新事项 2', { exact: true })).toHaveCount(0)
    await fixture.page.getByText('新事项', { exact: true }).click()
    await expect(fixture.page.getByRole('tab')).toHaveCount(1)
    await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(1)
    if (removedSessionId) {
      await expect(fixture.page.locator(`[data-session-id="${removedSessionId}"]`)).toHaveCount(0)
    }
  } finally {
    await fixture.close()
  }
})

test('opens a restored work scene from an ephemeral copy when durable storage is read-only', async () => {
  test.skip(process.platform === 'win32', 'POSIX permissions fixture')
  let fixture: MatouFixture = await launchMatou()
  let permissionsRestricted = false
  try {
    await fixture.page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
    await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close().catch(() => undefined)
    await chmod(fixture.dataDirectory, 0o500)
    permissionsRestricted = true

    fixture = await restartMatou(fixture)
    await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
    const restored = visibleSurfaces(fixture).first()
    await positivePid(restored)
    await typeTerminalCommand(restored, "printf '%s\\n' \"$((900 + 9))\"")
    await expect(restored.locator('.xterm-rows')).toContainText('909')
    await expect(fixture.page.getByText(/持久化|只读|恢复失败/)).toHaveCount(0)
  } finally {
    if (permissionsRestricted) await chmod(fixture.dataDirectory, 0o700).catch(() => undefined)
    await fixture.close()
  }
})

test('restores committed structure after a forced stop without restarting the foreground command', async () => {
  let fixture: MatouFixture = await launchMatou()
  try {
    await fixture.page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
    // This case verifies crash durability after the create transaction has
    // committed. Wait for the authoritative projection before typing so the
    // foreground command cannot race onto the previously active Task.
    await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
    const surface = visibleSurfaces(fixture).first()
    const originalPid = await positivePid(surface)
    await typeTerminalCommand(
      surface,
      "printf '__PRD04_FOREGROUND_STARTED__\\n'; sleep 60; printf '__PRD04_FOREGROUND_FINISHED__\\n'"
    )
    await expect(surface.locator('.xterm-rows')).toContainText('__PRD04_FOREGROUND_STARTED__')

    fixture.app.process().kill('SIGKILL')
    await fixture.app.close().catch(() => undefined)
    fixture = await restartMatou(fixture)

    await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
    const restored = visibleSurfaces(fixture).first()
    expect(await positivePid(restored)).not.toBe(originalPid)
    await expect(restored.locator('.xterm-rows')).toContainText('__PRD04_FOREGROUND_STARTED__')
    await expect.poll(async () => {
      const text = await restored.locator('.xterm-rows').textContent() ?? ''
      return text.split('__PRD04_FOREGROUND_FINISHED__').length - 1
    }).toBe(1)
    await expect(restored.locator('.xterm-rows')).toContainText('上次命令已中断，未自动重新执行')
  } finally {
    await fixture.close()
  }
})

test('degrades one invalid AI resume to a usable Shell and does not retry it next launch', async () => {
  let fixture: MatouFixture = await launchMatou()
  const providerExecutable = join(fixture.rootDirectory, 'missing-provider-session.sh')
  const invocations = join(fixture.rootDirectory, 'provider-invocations.txt')
  try {
    await positivePid(visibleSurfaces(fixture).first())
    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close()
    seedInvalidProviderResume(join(fixture.dataDirectory, 'matou.sqlite'))
    await writeFile(providerExecutable, [
      '#!/bin/sh',
      'printf "invoked\\n" >> "$MATOU_TEST_PROVIDER_INVOCATIONS"',
      'printf "No session found for requested id\\n"',
      'sleep 30',
      ''
    ].join('\n'))
    await chmod(providerExecutable, 0o755)

    fixture = await restartMatou(fixture, { env: {
      MATOU_CLAUDE_COMMAND: providerExecutable,
      MATOU_TEST_PROVIDER_INVOCATIONS: invocations
    } })
    const fallback = visibleSurfaces(fixture).first()
    await expect(fallback.locator('.xterm-rows')).toContainText(
      '[上次会话无法续接，已回到普通终端]'
    )
    await expect.poll(() => readSessionKind(join(fixture.dataDirectory, 'matou.sqlite')))
      .toBe('shell')
    await positivePid(fallback)
    await fixture.page.waitForTimeout(250)
    await typeTerminalCommand(
      visibleSurfaces(fixture).first(),
      "printf '%s\\n' \"$((31415 + 27182))\""
    )
    await expect(fallback.locator('.xterm-rows')).toContainText('58597')
    expect(readSessionKind(join(fixture.dataDirectory, 'matou.sqlite'))).toBe('shell')

    fixture = await restartMatou(fixture)
    const nextLaunch = visibleSurfaces(fixture).first()
    await positivePid(nextLaunch)
    // The earlier recovery result remains in terminal history; the durable
    // Shell identity and invocation count prove no second provider retry.
    expect((await readFile(invocations, 'utf8')).trim().split('\n')).toHaveLength(1)
  } finally {
    await fixture.close()
  }
})

test('degrades a resumed provider that exits cleanly before becoming interactive', async () => {
  let fixture: MatouFixture = await launchMatou()
  const providerExecutable = join(fixture.rootDirectory, 'early-clean-exit-provider.sh')
  try {
    await positivePid(visibleSurfaces(fixture).first())
    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close()
    seedProviderResume(join(fixture.dataDirectory, 'matou.sqlite'), {
      bindingId: 'e2e-clean-exit-provider-binding',
      providerSessionId: 'early-clean-exit-provider-session',
      permissionMode: 'default'
    })
    await writeFile(providerExecutable, '#!/bin/sh\nexit 0\n')
    await chmod(providerExecutable, 0o755)

    fixture = await restartMatou(fixture, { env: {
      MATOU_CLAUDE_COMMAND: providerExecutable
    } })
    const fallback = visibleSurfaces(fixture).first()
    await expect(fallback.locator('.xterm-rows')).toContainText(
      '[上次会话无法续接，已回到普通终端]'
    )
    await expect.poll(() => readSessionKind(join(fixture.dataDirectory, 'matou.sqlite')))
      .toBe('shell')
    await typeTerminalCommand(fallback, "printf '%s\\n' \"$((600 + 6))\"")
    await expect(fallback.locator('.xterm-rows')).toContainText('606')
  } finally {
    await fixture.close()
  }
})

test('returns an unresponsive AI resume to a usable Shell after the ten-second deadline', async () => {
  test.setTimeout(40_000)
  let fixture: MatouFixture = await launchMatou()
  const providerExecutable = join(fixture.rootDirectory, 'unresponsive-provider.sh')
  try {
    await positivePid(visibleSurfaces(fixture).first())
    await fixture.app.evaluate(({ app }) => app.quit())
    await fixture.app.close()
    seedProviderResume(join(fixture.dataDirectory, 'matou.sqlite'), {
      bindingId: 'e2e-timeout-provider-binding',
      providerSessionId: 'unresponsive-provider-session',
      permissionMode: 'default'
    })
    await writeFile(providerExecutable, '#!/bin/sh\nsleep 30\n')
    await chmod(providerExecutable, 0o755)

    fixture = await restartMatou(fixture, { env: {
      MATOU_CLAUDE_COMMAND: providerExecutable
    } })
    const fallback = visibleSurfaces(fixture).first()
    const providerPid = await positivePid(fallback)
    await fixture.page.waitForTimeout(8_000)
    await expect(fallback.locator('.xterm-rows')).not.toContainText(
      '[上次会话无法续接，已回到普通终端]'
    )
    await expect(fallback.locator('.xterm-rows')).toContainText(
      '[上次会话无法续接，已回到普通终端]',
      { timeout: 5_000 }
    )
    await expect.poll(() => readSessionKind(join(fixture.dataDirectory, 'matou.sqlite')))
      .toBe('shell')
    await expect(fallback).toHaveAttribute('data-profile', 'shell')
    await expect.poll(async () => {
      const pid = Number(await fallback.getAttribute('data-pid'))
      return pid > 0 && pid !== providerPid ? pid : 0
    }).toBeGreaterThan(0)
    await typeTerminalCommand(fallback, "printf '%s\\n' \"$((800 + 8))\"")
    await expect(fallback.locator('.xterm-rows')).toContainText('808')
  } finally {
    await fixture.close()
  }
})

function visibleSurfaces(fixture: MatouFixture) {
  return fixture.page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"] .terminal-surface')
}

function activeSurfaceFor(fixture: MatouFixture) {
  return fixture.page.locator(
    '.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"] .terminal-surface'
  )
}

async function positivePid(surface: ReturnType<typeof visibleSurfaces>): Promise<number> {
  let pid = 0
  await expect.poll(async () => {
    pid = Number(await surface.getAttribute('data-pid'))
    return pid
  }).toBeGreaterThan(0)
  return pid
}

async function typeTerminalCommand(
  surface: ReturnType<typeof visibleSurfaces>,
  command: string
): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  const pane = surface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
  if (await pane.getAttribute('data-active') !== 'true') {
    await surface.click({ position: { x: 12, y: 12 } })
  }
  await textarea.focus()
  await expect(pane).toHaveAttribute('data-active', 'true')
  await expect(textarea).toBeFocused()
  await surface.page().waitForTimeout(50)
  await textarea.focus()
  await textarea.pressSequentially(command, { delay: 2 })
  await textarea.press('Enter')
}

async function processCwd(pid: number): Promise<string> {
  if (process.platform === 'linux') {
    const { stdout } = await execFileAsync('readlink', [`/proc/${pid}/cwd`])
    return stdout.trim()
  }
  const { stdout } = await execFileAsync('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'])
  return stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1) ?? ''
}

function seedInvalidProviderResume(databasePath: string): void {
  seedProviderResume(databasePath, {
    bindingId: 'e2e-invalid-provider-binding',
    providerSessionId: 'missing-provider-session',
    permissionMode: 'default'
  })
}

function seedProviderResume(
  databasePath: string,
  input: { bindingId: string; providerSessionId: string; permissionMode: string }
): void {
  const database = new DatabaseSync(databasePath)
  try {
    const session = database.prepare(
      'SELECT id FROM sessions WHERE archived_at IS NULL ORDER BY created_at LIMIT 1'
    ).get() as { id: string } | undefined
    if (!session) throw new Error('Expected the default Session to exist')
    database.prepare("UPDATE sessions SET kind = 'claude-code' WHERE id = ?").run(session.id)
    database.prepare(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, metadata_json,
         created_at, updated_at, validated_at, invalidated_at
       ) VALUES (?, ?, 'claude-code', ?, 'available', ?, ?, ?, ?, NULL)`
    ).run(
      input.bindingId, session.id, input.providerSessionId,
      JSON.stringify({ permissionMode: input.permissionMode }), 1, 1, 1
    )
  } finally {
    database.close()
  }
}

function readSessionKind(databasePath: string): string | undefined {
  const database = new DatabaseSync(databasePath, { readOnly: true })
  try {
    return (database.prepare(
      'SELECT kind FROM sessions WHERE archived_at IS NULL ORDER BY created_at LIMIT 1'
    ).get() as { kind: string } | undefined)?.kind
  } finally {
    database.close()
  }
}
