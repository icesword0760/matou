import { join } from 'node:path'
import { expect, test } from '@playwright/test'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { readSessionFrames } from '../../apps/runtime/src/journal/segment-journal'
import { launchMatou, restartMatou, stopMatouPreservingData, type MatouFixture } from './matou-fixture'
import { activeSurface, terminalCommand, waitForShell } from './fixtures/session-canvas-fixture'

const env = { MATOU_E2E_SCALE: '1', MATOU_E2E_TERMINAL_DIAGNOSTICS: '0' }
const metrics = (fixture: MatouFixture) => fixture.app.evaluate(async () => {
  const read = (globalThis as any).__matouE2eScaleMetrics
  const result = await read()
  return result.ptySessions as Array<{ sessionId: string; pid: number }>
})

test('starts the recent 16, preserves dormant history, and restores only the clicked card', async () => {
  test.setTimeout(120_000)
  let fixture = await launchMatou({ env })
  try {
    const initial = activeSurface(fixture.page)
    await waitForShell(initial)
    const originalId = (await initial.getAttribute('data-session-id'))!
    await terminalCommand(initial, 'printf "SAVED_BEFORE_MANUAL_RECOVERY\\n"')
    await expect(initial.locator('.xterm-rows')).toContainText('SAVED_BEFORE_MANUAL_RECOVERY')
    await stopMatouPreservingData(fixture)
    const db = RuntimeDatabase.open(join(fixture.dataDirectory, 'matou.sqlite'))
    try {
      const base = db.get<{ task_id: string; execution_context_id: string; cwd: string }>(
        'SELECT task_id, execution_context_id, cwd FROM sessions WHERE id = ?', originalId)!
      const scene = db.get<{ scene_id: string }>(
        'SELECT scene_id FROM session_canvas_memberships WHERE session_id = ?', originalId)!.scene_id
      db.run(`UPDATE sessions SET created_at = 1, last_activity_at = 999999, work_status = 'interrupted' WHERE id = ?`, originalId)
      db.run(`UPDATE session_canvas_memberships SET last_user_interaction_seq = 0,
        pending_user_interaction_seq = 0, updated_at = 1 WHERE session_id = ?`, originalId)
      for (let i = 1; i < 20; i++) {
        const id = `recent-test-${i}`
        db.run(`INSERT INTO sessions (id, task_id, execution_context_id, kind, status,
          title, cwd, created_at, updated_at, last_activity_at, work_status)
          VALUES (?, ?, ?, 'shell', 'interrupted', ?, ?, ?, ?, 999999, 'interrupted')`,
        id, base.task_id, base.execution_context_id, `会话 ${i}`, base.cwd, i + 1, i + 1)
        db.run(`INSERT INTO session_mounts (id, scene_id, session_id, created_at) VALUES (?, ?, ?, ?)`,
          `mount-${id}`, scene, id, i + 1)
        db.run(`INSERT OR REPLACE INTO session_canvas_memberships
          (session_id, scene_id, sibling_created_seq, last_user_interaction_seq, created_at, updated_at)
          VALUES (?, ?, ?, 0, ?, ?)`, id, scene, i + 1, i + 1, i + 1)
      }
    } finally { db.close() }
    fixture = await restartMatou(fixture, { env })
    await expect.poll(async () => (await metrics(fixture)).length).toBe(16)
    expect(new Set((await metrics(fixture)).map(p => p.sessionId)))
      .toEqual(new Set(Array.from({ length: 16 }, (_, i) => `recent-test-${i + 4}`)))
    const dormant = fixture.page.getByTestId('session-deferred-recovery').first()
    await expect(dormant).toBeVisible()
    await expect(dormant).toContainText('会话的记录已保留')
    await fixture.page.screenshot({ path: test.info().outputPath('manual-recovery.png') })
    expect((await metrics(fixture)).some(p => p.sessionId === originalId)).toBe(false)
    await dormant.getByRole('button', { name: /恢复会话/ }).click()
    await expect.poll(async () => (await metrics(fixture)).length).toBe(17)
    const restored = fixture.page.locator(`.terminal-surface[data-session-id="${originalId}"]`)
    await waitForShell(restored)
    const saved = await readSessionFrames(fixture.dataDirectory, originalId)
    expect(saved.filter(frame => frame.kind === 'output')
      .map(frame => new TextDecoder().decode(frame.data)).join(''))
      .toContain('SAVED_BEFORE_MANUAL_RECOVERY')
    await terminalCommand(restored, 'printf "MANUALLY_RECOVERED_INPUT_WORKS\\n"')
    await expect(restored.locator('.xterm-rows')).toContainText('MANUALLY_RECOVERED_INPUT_WORKS')
    fixture = await restartMatou(fixture, { env })
    await expect.poll(async () => (await metrics(fixture)).length).toBe(16)
    const running = (await metrics(fixture)).map(p => p.sessionId)
    expect(running).toContain(originalId)
    expect(running).not.toContain('recent-test-4')
  } catch (error) {
    console.log('RECENT_DEBUG_METRICS', JSON.stringify(await fixture.app.evaluate(async () => await (globalThis as any).__matouE2eScaleMetrics()).catch(() => null)))
    const debug = RuntimeDatabase.openReadOnly(join(fixture.dataDirectory, 'matou.sqlite'))
    try { console.log('RECENT_DEBUG_ROWS', JSON.stringify(debug.all('SELECT id, work_status, status FROM sessions'))) } finally { debug.close() }
    throw error
  } finally { await fixture.close() }
})
