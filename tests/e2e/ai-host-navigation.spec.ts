import { expect, test } from '@playwright/test'

import { windowId } from './matou-fixture'
import {
  launchAiHostControl,
  runMtInSession,
  seedResumableProviderSession,
  sessionCard
} from './fixtures/ai-host-control-fixture'

interface CreatedResult {
  kind: 'created'
  createdRef: string
  path: {
    workspace: { ref: string; title: string }
    task?: { ref: string; title: string }
    canvas?: { ref: string; title: string }
    session?: { ref: string; title: string }
  }
}

interface NavigatedResult {
  kind: 'navigated'
  finalPath: {
    routeWindowId: string
    targetWindowId: string
    workspaceId: string
    taskId: string
    sceneId: string
    sessionId?: string
  }
}

interface IdentifiedResult {
  target: {
    ref: string
    workspace: { id: string }
    task: { id: string }
    canvas: { id: string }
  }
}

test('focuses and switches to a detached active Session through its owning main Renderer', async () => {
  test.setTimeout(180_000)
  const fixture = await launchAiHostControl()
  try {
    const caller = await seedResumableProviderSession(fixture)
    const identity = (await runMtInSession<IdentifiedResult>(
      fixture, caller.sessionId, ['identify', '--json']
    )).value
    const pane = fixture.page.locator(
      `[data-testid="terminal-pane"]:has(.terminal-surface[data-session-id="${caller.sessionId}"])`
    )
    await pane.locator('.terminal-pane-header').click({
      button: 'right', position: { x: 72, y: 20 }, force: true
    })
    await fixture.page.getByRole('menuitem', { name: '↗ 独立窗口' }).click()
    await expect(fixture.page.getByTestId('detached-placeholder')).toContainText('已脱出')
    await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
    const detached = (await fixture.app.windows()).find((candidate) => candidate !== fixture.page)!
    const routeWindowId = windowId(fixture.page)
    const targetWindowId = windowId(detached)

    // The command is typed through the detached terminal itself. Give that
    // native window ownership before Playwright sends keys; the packaged-App
    // acceptance exercises the stronger background-to-detached raise by
    // invoking Host Control out of process.
    await detached.evaluate((id) => window.matouDesktop!.showWindow(id), targetWindowId)
    await expectNativeFocusedWindow(fixture.app, targetWindowId)

    const focused = (await runMtInSession<NavigatedResult>(
      fixture, caller.sessionId, ['focus', identity.target.ref, '--json'], 0, detached
    )).value
    expect(focused.finalPath).toMatchObject({
      routeWindowId, targetWindowId, sessionId: caller.sessionId
    })
    await expectNativeFocusedWindow(fixture.app, targetWindowId)
    const textarea = detached.locator(
      `.terminal-surface[data-session-id="${caller.sessionId}"] .xterm-helper-textarea`
    )
    await expect(textarea).toBeFocused()

    for (const [kind, ref] of [
      ['workspace', `workspace:${identity.target.workspace.id}`],
      ['task', `task:${identity.target.task.id}`],
      ['canvas', `canvas:${identity.target.canvas.id}`]
    ] as const) {
      const switched = (await runMtInSession<NavigatedResult>(
        fixture, caller.sessionId, ['switch', kind, ref, '--json'], 0, detached
      )).value
      expect(switched.finalPath).toMatchObject({
        routeWindowId, targetWindowId, sessionId: caller.sessionId
      })
      await expectNativeFocusedWindow(fixture.app, targetWindowId)
    }
  } finally {
    await fixture.close()
  }
})

test('focuses a session in the second main window and leaves its terminal ready for keyboard input', async () => {
  test.setTimeout(180_000)
  const fixture = await launchAiHostControl()
  try {
    const { app, page: source } = fixture
    const caller = await seedResumableProviderSession(fixture)
    const task = (await runMtInSession<CreatedResult>(fixture, caller.sessionId, [
      'create', 'task', '--workspace', 'current', '--title', '跨窗口实施事项',
      '--submission-key', 'e2e-cross-window-task', '--json'
    ])).value
    const taskRef = requiredRef(task.path.task?.ref, 'task')
    const canvasRef = requiredRef(task.path.canvas?.ref, 'canvas')
    const targetSession = (await runMtInSession<CreatedResult>(fixture, caller.sessionId, [
      'create', 'session', '--canvas', canvasRef, '--profile', 'shell',
      '--title', '跨窗口焦点会话', '--submission-key', 'e2e-cross-window-session', '--json'
    ])).value
    const targetSessionRef = requiredRef(targetSession.path.session?.ref, 'session')
    const taskId = idFromRef(taskRef, 'task')
    const sceneId = idFromRef(canvasRef, 'scene')
    const targetSessionId = idFromRef(targetSessionRef, 'session')

    await source.evaluate(() => { window.open('about:blank') })
    await expect.poll(async () => (await app.windows()).length).toBe(2)
    const target = (await app.windows()).find((candidate) => candidate !== source)!
    await expect(target.getByRole('group', { name: 'matou_workspace 工作空间' })).toBeVisible()
    const sourceWindowId = windowId(source)
    const targetWindowId = windowId(target)
    expect(sourceWindowId).not.toBe(targetWindowId)

    await source.evaluate(async ({ taskId: movedTaskId, sourceId, targetId }) => {
      await window.matouE2e!.moveTaskToWindow({
        migrationId: crypto.randomUUID(),
        taskId: movedTaskId,
        sourceWindowId: sourceId,
        targetWindowId: targetId
      })
    }, { taskId, sourceId: sourceWindowId, targetId: targetWindowId })
    await target.reload()
    await expect(source.locator(`[data-testid="task-${taskId}"]`)).toHaveCount(0)
    await expect(target.locator(`[data-testid="task-${taskId}"]`)).toHaveCount(1)

    const focused = (await runMtInSession<NavigatedResult>(fixture, caller.sessionId, [
      'focus', targetSessionRef, '--json'
    ])).value
    expect(focused.finalPath).toEqual({
      routeWindowId: targetWindowId,
      targetWindowId,
      workspaceId: idFromRef(task.path.workspace.ref, 'workspace'),
      taskId,
      sceneId,
      sessionId: targetSessionId
    })

    await expect.poll(() => app.evaluate(({ BrowserWindow }, expectedWindowId) => {
      const focusedWindow = BrowserWindow.getFocusedWindow()
      const focusedId = focusedWindow === null
        ? undefined
        : new URL(focusedWindow.webContents.getURL()).searchParams.get('windowId') ?? undefined
      return { focusedId, focused: focusedWindow?.isFocused() ?? false }
    }, targetWindowId), {
      message: '等待第二个主窗口成为原生前台窗口', timeout: 30_000
    }).toEqual({ focusedId: targetWindowId, focused: true })

    await expect(target.getByRole('group', { name: 'matou_workspace 工作空间' })
      .locator('.workspace-group__header')).toHaveAttribute('aria-current', 'location')
    await expect(target.locator(`[data-testid="task-${taskId}"]`))
      .toHaveAttribute('aria-current', 'true')
    await expect(target.locator(`[data-testid="task-${taskId}"]`)).toContainText('跨窗口实施事项')
    await expect(target.locator(`[data-scene-id="${sceneId}"]`).getByRole('tab'))
      .toHaveAttribute('aria-selected', 'true')
    const card = sessionCard(target, '跨窗口焦点会话')
    await expect(card).toBeVisible()
    await expect(card).toHaveAttribute('aria-current', 'true')
    await expect(card).toHaveAttribute('data-in-viewport', 'true')

    const terminal = target.locator(`.terminal-surface[data-session-id="${targetSessionId}"]`)
    const textarea = terminal.locator('.xterm-helper-textarea')
    await expect(textarea).toBeFocused()
    await textarea.pressSequentially("printf '__CROSS_WINDOW_INPUT__\\n'", { delay: 1 })
    await textarea.press('Enter')
    await expect(terminal.locator('.xterm-rows')).toContainText('__CROSS_WINDOW_INPUT__')
  } finally {
    await fixture.close()
  }
})

function requiredRef(value: string | undefined, kind: string): string {
  if (!value) throw new Error(`Expected ${kind} ref`)
  return value
}

function idFromRef(ref: string, kind: string): string {
  const prefix = `${kind}:`
  if (!ref.startsWith(prefix)) throw new Error(`Expected ${kind} ref, received ${ref}`)
  return ref.slice(prefix.length)
}

async function expectNativeFocusedWindow(
  app: import('@playwright/test').ElectronApplication,
  expectedWindowId: string
): Promise<void> {
  await expect.poll(() => app.evaluate(({ BrowserWindow }, expectedId) => {
    const focusedWindow = BrowserWindow.getFocusedWindow()
    const focusedId = focusedWindow === null
      ? undefined
      : new URL(focusedWindow.webContents.getURL()).searchParams.get('windowId') ?? undefined
    return { focusedId, focused: focusedWindow?.isFocused() ?? false }
  }, expectedWindowId), {
    message: '等待目标窗口成为原生前台窗口', timeout: 30_000
  }).toEqual({ focusedId: expectedWindowId, focused: true })
}
