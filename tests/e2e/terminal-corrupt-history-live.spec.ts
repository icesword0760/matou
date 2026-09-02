import { readdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import { JournalHistoryReader } from '../../apps/runtime/src/journal/journal-history-reader'
import {
  expectVisibleWindowsOnPrimaryDisplay,
  launchMatou,
  primaryAcceptanceDisplayRequested,
  restartMatou
} from './matou-fixture'
import { terminalCommand, visibleSurfaces, waitForShell } from './fixtures/session-canvas-fixture'

const E2E_ENV = {
  MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES: '512',
  MATOU_E2E_JOURNAL_RAW_HOT_BYTES: '1024'
}

test('keeps a real PTY usable when one cold history segment is corrupt', async () => {
  let fixture = await launchMatou({ env: E2E_ENV })
  try {
    const initial = visibleSurfaces(fixture.page).first()
    await waitForShell(initial)
    const sessionId = await initial.getAttribute('data-session-id')
    if (!sessionId) throw new Error('Expected a Session identity')

    await terminalCommand(initial,
      "i=0; while [ $i -lt 200 ]; do printf 'COLD_HISTORY_%04d_abcdefghijklmnopqrstuvwxyz\\n' $i; i=$((i+1)); done; printf 'COLD_DONE\\n'")
    await expect(initial.locator('.xterm-rows')).toContainText('COLD_DONE')

    const journalDirectory = join(fixture.dataDirectory, 'journal', sessionId)
    await expect.poll(async () =>
      (await readdir(journalDirectory)).filter((name) => name.endsWith('.gz')).sort()[0]
    ).not.toBeUndefined()
    const archiveName = (await readdir(journalDirectory)).filter((name) => name.endsWith('.gz')).sort()[0]!
    await writeFile(join(journalDirectory, archiveName), 'damaged cold archive')

    await terminalCommand(initial,
      "i=0; while [ $i -lt 80 ]; do printf 'HEALTHY_TAIL_%04d_abcdefghijklmnopqrstuvwxyz\\n' $i; i=$((i+1)); done; printf 'HEALTHY_DONE\\n'")
    await expect(initial.locator('.xterm-rows')).toContainText('HEALTHY_DONE')

    fixture = await restartMatou(fixture, { env: E2E_ENV })
    const restored = fixture.page.locator(`.terminal-surface[data-session-id=\"${sessionId}\"]`)
    await expectOnlySecondaryWindow(fixture)
    await waitForShell(restored)
    await expect(restored.locator('.xterm-rows')).toContainText('HEALTHY_DONE')
    const history = await new JournalHistoryReader(fixture.dataDirectory).search({
      sessionId,
      query: 'HEALTHY_TAIL_0079'
    })
    expect(history.matches.some(({ text }) => text.includes('HEALTHY_TAIL_0079'))).toBe(true)
    expect(history.gaps).toEqual([
      expect.objectContaining({ code: 'CORRUPT_SEGMENT' })
    ])
    await terminalCommand(restored, "printf 'LIVE_AFTER_HISTORY_GAP\\n'")
    await expect(restored.locator('.xterm-rows')).toContainText('LIVE_AFTER_HISTORY_GAP')
  } finally {
    await fixture.close()
  }
})

async function expectOnlySecondaryWindow(fixture: Awaited<ReturnType<typeof launchMatou>>): Promise<void> {
  if (primaryAcceptanceDisplayRequested()) {
    await expectVisibleWindowsOnPrimaryDisplay(fixture)
    return
  }
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
    const placements = visible.map((window) => screen.getDisplayMatching(window.getBounds()))
    return {
      visible: visible.length,
      internal: placements.filter((display) => display.internal).length,
      primary: placements.filter((display) => display.id === primary.id).length,
      xv272u: placements.filter((display) => /xv272u/i.test(display.label)).length
    }
  })).toEqual({ visible: 1, internal: 1, primary: 0, xv272u: 0 })
}
