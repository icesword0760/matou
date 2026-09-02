import { execFile } from 'node:child_process'
import { chmod, mkdir, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises'
import { join, relative } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'

import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

const execFileAsync = promisify(execFile)

test('restores the work structure, cwd, and completed Shell command Blocks', async () => {
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
    await expect(restoredSurface.locator('.xterm-rows')).toContainText('会话已恢复')
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
    // The secondary acceptance display is intentionally narrow enough for
    // xterm to wrap this long path. The authoritative cwd assertion below
    // verifies the full value; this assertion only verifies visible output.
    await expect(surface.locator('.xterm-rows')).toContainText(/removed-session-directo\s*ry/)
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
    const segmentName = (await readdir(journalDirectory)).find(
      (name) => name.endsWith('.mtj') || name.endsWith('.bin')
    )
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
  let removedSessionId = ''
  try {
    await test.step('创建待删除的 Task、Scene 与 Session', async () => {
      const { page } = fixture
      await page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
      await expect(page.getByTestId('active-task')).toHaveText('新事项')
      await page.getByRole('button', { name: '新建页签' }).click()
      await page.getByRole('button', { name: '横向新增 Shell' }).click()
      const visiblePanels = page.locator('[data-testid="terminal-pane"]:visible')
      await expect(visiblePanels).toHaveCount(2)
      removedSessionId = await visiblePanels.last().locator('.terminal-surface')
        .getAttribute('data-session-id') ?? ''
      if (!removedSessionId) throw new Error('Expected the new Session identity')
    })

    await test.step('删除 Session 后立即从当前 Scene 消失', async () => {
      const { page } = fixture
      const removedPanel = page.locator('[data-testid="terminal-pane"]:visible').filter({
        has: page.locator(`.terminal-surface[data-session-id="${removedSessionId}"]`)
      })
      await removedPanel.locator('.pane-title').click({ button: 'right' })
      await page.getByRole('menuitem', { name: '移除节点…' }).click()
      await page.getByRole('button', { name: '移除' }).click()
      await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(1)
      await expect(page.locator(`[data-session-id="${removedSessionId}"]`)).toHaveCount(0)
    })

    await test.step('删除 Scene 后只保留原 Scene', async () => {
      const { page } = fixture
      await page.getByRole('button', { name: /^关闭页签：/ }).last().click()
      await expect(page.getByRole('alertdialog', { name: '关闭画布' })).toBeVisible()
      await page.getByRole('button', { name: '关闭画布' }).click()
      await expect(page.getByRole('tab')).toHaveCount(1)
    })

    await test.step('删除 Task 后立即从事项列表消失', async () => {
      const { page } = fixture
      await page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
      await expect(page.getByTestId('active-task')).toHaveText('新事项 2')
      await page.getByRole('button', { name: '事项菜单：新事项 2' }).click()
      await page.getByRole('menuitem', { name: '删除' }).click()
      await page.getByRole('button', { name: '确定' }).click()
      await expect(page.getByText('新事项 2', { exact: true })).toHaveCount(0)
    })

    await test.step('关闭应用并从同一持久化现场重启', async () => {
      fixture = await restartMatou(fixture)
    })

    await test.step('重启后 Task、Scene 与 Session 均未复活', async () => {
      const { page } = fixture
      await expect(page.getByText('新事项 2', { exact: true })).toHaveCount(0)
      await page.getByText('新事项', { exact: true }).click()
      await expect(page.getByRole('tab')).toHaveCount(1)
      await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(1)
      await expect(page.locator(`[data-session-id="${removedSessionId}"]`)).toHaveCount(0)
    })
  } finally {
    await fixture.close()
  }
})
test('opens the real database in read-only recovery mode for browse, search, copy, and export only', async () => {
  test.skip(process.platform === 'win32', 'POSIX permissions fixture')
  test.setTimeout(90_000)
  let fixture: MatouFixture = await launchMatou()
  let permissionsRestricted = false
  const exportDirectory = join(fixture.rootDirectory, 'read-only-exports')
  const historyMarker = 'MATOU_READONLY_RECOVERY_HISTORY'
  const rejectedInputMarker = 'MATOU_READONLY_REJECTED_INPUT'
  let before: Record<string, string> = {}
  let sceneNames: string[] = []
  try {
    await test.step('用真实 Shell 建立可浏览的 Task、Scene 与终端历史', async () => {
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)
      await fixture.page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
      await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
      await fixture.page.getByRole('button', { name: '新建页签' }).click()
      await expect(fixture.page.getByRole('tab')).toHaveCount(2)
      sceneNames = (await fixture.page.getByRole('tab').allTextContents())
        .map((name) => name.trim())
      expect(sceneNames).toHaveLength(2)
      const surface = activeSurfaceFor(fixture)
      await positivePid(surface)
      await typeTerminalCommand(surface, `printf '${historyMarker}\\n'`)
      await expect(surface.locator('.xterm-rows')).toContainText(historyMarker)
      await fixture.page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
      await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项 2')
      await fixture.page.getByText('新事项', { exact: true }).click()
      await expect(fixture.page.getByRole('tab', { name: sceneNames[1]! }))
        .toHaveAttribute('aria-selected', 'true')
    })

    await test.step('关闭 Runtime、记录完整数据 bytes 并 chmod 为真实只读树', async () => {
      await fixture.app.evaluate(({ app }) => app.quit())
      await fixture.app.close().catch(() => undefined)
      before = await snapshotFiles(fixture.dataDirectory)
      expect(Object.keys(before)).toContain('matou.sqlite')
      await setReadOnlyTree(fixture.dataDirectory)
      permissionsRestricted = true
    })

    await test.step('只读启动保留现场且窗口仅位于副屏 Color LCD', async () => {
      fixture = await restartMatou(fixture, {
        env: { MATOU_RECOVERY_EXPORT_DIR: exportDirectory }
      })
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)
      await expect(fixture.page.getByRole('status')).toContainText('数据库处于只读恢复模式')
      await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
      await expect(activeSurfaceFor(fixture)).not.toHaveAttribute('data-pid', /\d+/)
      await expect(activeSurfaceFor(fixture).locator('.xterm-rows')).toContainText(historyMarker)
      expect(await snapshotFiles(fixture.dataDirectory)).toEqual(before)
    })

    await test.step('只读现场仍可切换 Task 与 Scene', async () => {
      await fixture.page.getByText('新事项 2', { exact: true }).click()
      await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项 2')
      await fixture.page.getByText('新事项', { exact: true }).click()
      await fixture.page.getByRole('tab', { name: sceneNames[0]! }).click()
      await expect(fixture.page.getByRole('tab', { name: sceneNames[0]! }))
        .toHaveAttribute('aria-selected', 'true')
      await fixture.page.getByRole('tab', { name: sceneNames[1]! }).click()
      await expect(activeSurfaceFor(fixture).locator('.xterm-rows')).toContainText(historyMarker)
      expect(await snapshotFiles(fixture.dataDirectory)).toEqual(before)
    })

    await test.step('只读终端历史可搜索', async () => {
      await fixture.page.getByRole('button', { name: '搜索当前终端' }).click()
      const search = fixture.page.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' })
      await expect(search).toBeVisible()
      await search.fill(historyMarker)
      await expect(fixture.page.locator('.terminal-search-bar__count')).toHaveText('1/1')
      await search.press('Escape')
      expect(await snapshotFiles(fixture.dataDirectory)).toEqual(before)
    })

    await test.step('只读终端历史可选择并复制', async () => {
      const surface = activeSurfaceFor(fixture)
      await selectVisibleTerminal(fixture.page, surface)
      await fixture.page.keyboard.press(`${process.platform === 'darwin' ? 'Meta' : 'Control'}+c`)
      await expect.poll(() => fixture.app.evaluate(({ clipboard }) => clipboard.readText()))
        .toContain(historyMarker)
      expect(await snapshotFiles(fixture.dataDirectory)).toEqual(before)
    })

    await test.step('全部可见结构写入口逐项禁用并说明原因', async () => {
      const reason = '数据库处于只读恢复模式'
      const directWriteControls = [
        fixture.page.getByRole('button', { name: '新增工作空间' }),
        fixture.page.getByRole('button', { name: /^在 .* 中新增事项$/ }),
        fixture.page.getByRole('button', { name: '新建页签' }),
        fixture.page.getByRole('button', { name: /^关闭页签：/ }).first(),
        fixture.page.getByRole('button', { name: '横向新增 Shell' }),
        fixture.page.getByRole('button', { name: /^载入 Claude Code 会话到/ }),
        fixture.page.getByRole('button', { name: '打开 Git' })
      ]
      for (const control of directWriteControls) await expectReadOnlyDisabled(control, reason)
      const visibleReasonedControls = fixture.page.locator(
        `button[title="${reason}"]:visible`
      )
      expect(await visibleReasonedControls.count()).toBeGreaterThanOrEqual(directWriteControls.length)
      for (const control of await visibleReasonedControls.all()) await expect(control).toBeDisabled()

      await fixture.page.getByRole('button', { name: '事项菜单：新事项', exact: true }).click()
      for (const name of ['置顶', '重命名', '删除']) {
        await expectReadOnlyDisabled(fixture.page.getByRole('menuitem', { name }), reason)
      }
      await fixture.page.keyboard.press('Escape')

      await fixture.page.getByRole('button', { name: /^工作空间菜单：/ }).first().click()
      await expectReadOnlyDisabled(fixture.page.getByRole('menuitem', { name: '置顶' }), reason)
      await fixture.page.keyboard.press('Escape')

      const activePane = fixture.page.locator(
        '[data-testid="terminal-pane"]:visible[data-active="true"]'
      )
      await expect(activePane.locator('.terminal-pane-header')).toHaveAttribute('draggable', 'false')
      await activePane.locator('.pane-title').click({ button: 'right' })
      await expect(fixture.page.getByRole('menuitem', { name: '移除节点…' })).toHaveCount(0)
      await expect(fixture.page.getByRole('menuitem', { name: /独立窗口/ })).toHaveCount(0)
      expect(await snapshotFiles(fixture.dataDirectory)).toEqual(before)
    })

    await test.step('终端输入被拒绝且数据 bytes 保持不变', async () => {
      const surface = activeSurfaceFor(fixture)
      const textarea = surface.locator('.xterm-helper-textarea')
      await textarea.focus()
      await textarea.pressSequentially(`printf '${rejectedInputMarker}\\n'`, { delay: 2 })
      await textarea.press('Enter')
      await fixture.page.waitForTimeout(250)
      await expect(surface.locator('.xterm-rows')).not.toContainText(rejectedInputMarker)
      expect(await snapshotFiles(fixture.dataDirectory)).toEqual(before)
    })

    await test.step('导出只读数据库资料且原数据 bytes 保持不变', async () => {
      await fixture.page.getByRole('button', { name: '导出数据库资料' }).click()
      await expect(fixture.page.getByText(/数据库资料已导出到/)).toBeVisible()
      const bundles = await readdir(exportDirectory)
      expect(bundles).toHaveLength(1)
      expect(await readdir(join(exportDirectory, bundles[0]!))).toContain('matou.sqlite')
      expect(await snapshotFiles(fixture.dataDirectory)).toEqual(before)
    })

    await restoreWritableTree(fixture.dataDirectory)
    permissionsRestricted = false
    fixture = await restartMatou(fixture)
    await assertVisibleWindowsOnSecondaryColorLcd(fixture)
    await expect(fixture.page.getByRole('status')).toHaveCount(0)
    await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
    await positivePid(activeSurfaceFor(fixture))
  } finally {
    if (permissionsRestricted) await restoreWritableTree(fixture.dataDirectory).catch(() => undefined)
    await fixture.close()
  }
})
test('restores committed structure after a forced stop without presenting an unfinished command as history', async () => {
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
    await expect(restored.locator('.xterm-rows')).not.toContainText('__PRD04_FOREGROUND_STARTED__')
    await expect(restored.locator('.xterm-rows')).not.toContainText('__PRD04_FOREGROUND_FINISHED__')
    await expect(restored.locator('.xterm-rows')).not.toContainText('上次命令已中断')
  } finally {
    await fixture.close()
  }
})

test('keeps one invalid AI resume on its Claude card and does not retry it next launch', async () => {
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
    const failedPane = fixture.page.locator('[data-testid="terminal-pane"]:visible').first()
    await expect(failedPane.getByRole('status')).toContainText('Claude Code 恢复失败')
    await expect.poll(() => readSessionKind(join(fixture.dataDirectory, 'matou.sqlite')))
      .toBe('claude-code')
    await expect(failedPane.locator('.terminal-surface')).toHaveCount(0)
    await expect(failedPane.getByRole('button', { name: '重试恢复' })).toBeVisible()
    await expect(failedPane.getByRole('button', { name: '新开 Claude Code' })).toBeVisible()
    await fixture.page.waitForTimeout(250)
    expect(readSessionKind(join(fixture.dataDirectory, 'matou.sqlite'))).toBe('claude-code')

    fixture = await restartMatou(fixture, { env: {
      MATOU_CLAUDE_COMMAND: providerExecutable,
      MATOU_TEST_PROVIDER_INVOCATIONS: invocations
    } })
    await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible').first().getByRole('status'))
      .toContainText('Claude Code 恢复失败')
    expect((await readFile(invocations, 'utf8')).trim().split('\n')).toHaveLength(1)
  } finally {
    await fixture.close()
  }
})

test('keeps a resumed provider failed when it exits before becoming interactive', async () => {
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
    const failedPane = fixture.page.locator('[data-testid="terminal-pane"]:visible').first()
    await expect(failedPane.getByRole('status')).toContainText('Claude Code 恢复失败')
    await expect.poll(() => readSessionKind(join(fixture.dataDirectory, 'matou.sqlite')))
      .toBe('claude-code')
    await expect(failedPane.locator('.terminal-surface')).toHaveCount(0)
  } finally {
    await fixture.close()
  }
})

test('parks an unresponsive AI resume after the ten-second deadline', async () => {
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
    const pane = fixture.page.locator('[data-testid="terminal-pane"]:visible').first()
    const providerPid = await positivePid(pane.locator('.terminal-surface'))
    await fixture.page.waitForTimeout(8_000)
    await expect(pane.getByRole('status')).toContainText('Claude Code 恢复失败', { timeout: 5_000 })
    await expect.poll(() => readSessionKind(join(fixture.dataDirectory, 'matou.sqlite')))
      .toBe('claude-code')
    expect(providerPid).toBeGreaterThan(0)
    await expect(pane.locator('.terminal-surface')).toHaveCount(0)
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
  // Real interactive shells can still be painting their first prompt after
  // the PTY PID appears. Clear any partially delivered keystroke before each
  // command so the acceptance action always starts from an empty prompt.
  await textarea.press('Control+u')
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

async function expectReadOnlyDisabled(
  control: import('@playwright/test').Locator,
  reason: string
): Promise<void> {
  await expect(control).toBeVisible()
  await expect(control).toBeDisabled()
  await expect(control).toHaveAttribute('title', reason)
}

async function selectVisibleTerminal(
  page: import('@playwright/test').Page,
  surface: import('@playwright/test').Locator
): Promise<void> {
  const screen = surface.locator('.xterm-screen')
  const box = await screen.boundingBox()
  if (!box) throw new Error('Expected terminal viewport geometry')
  await page.mouse.move(box.x + 2, box.y + 2)
  await page.mouse.down()
  await page.mouse.move(box.x + box.width - 2, box.y + box.height - 2, { steps: 12 })
  await page.mouse.up()
}

async function snapshotFiles(root: string): Promise<Record<string, string>> {
  const snapshot: Record<string, string> = {}
  const visit = async (directory: string): Promise<void> => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) await visit(path)
      else if (entry.isFile()) {
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
      } else if (entry.isFile()) await chmod(path, 0o400)
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
      } else if (entry.isFile()) await chmod(path, 0o600)
    }
  }
  await visit(root)
}

async function assertVisibleWindowsOnSecondaryColorLcd(fixture: MatouFixture): Promise<void> {
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const colorLcd = screen.getAllDisplays().filter(({ id, internal, label }) =>
      id !== primary.id && (internal || /color\s*lcd|内建视网膜显示器/i.test(label)))
    const visibleWindows = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
    const displays = visibleWindows.map((window) => screen.getDisplayMatching(window.getBounds()))
    return {
      primaryLabel: primary.label,
      colorLcdCount: colorLcd.length,
      visibleWindowCount: visibleWindows.length,
      allWindowsOnColorLcd: displays.every(
        ({ id }) => colorLcd.some((display) => display.id === id)
      ),
      windowsOnPrimary: displays.filter(({ id }) => id === primary.id).length
    }
  })).toEqual({
    primaryLabel: 'XV272U',
    colorLcdCount: 1,
    visibleWindowCount: 1,
    allWindowsOnColorLcd: true,
    windowsOnPrimary: 0
  })
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
