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
