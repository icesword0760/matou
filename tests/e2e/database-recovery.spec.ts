import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

test('real Electron shows only recovery UI after SQLite header corruption and restores the latest backup', async () => {
  let fixture: MatouFixture | undefined = await launchMatou()
  try {
    await expect(fixture.page.getByRole('main')).toBeVisible()
    await expect(fixture.page.locator('.workspace-group').first()).toBeVisible()
    const workspaceName = await fixture.page.locator('.workspace-group__name').first().textContent()

    await Promise.all([
      fixture.app.waitForEvent('close'),
      fixture.app.evaluate(({ app }) => { app.quit() })
    ])
    const databasePath = join(fixture.dataDirectory, 'matou.sqlite')
    const bytes = await readFile(databasePath)
    bytes.fill(0x5a, 0, 16)
    await writeFile(databasePath, bytes)

    fixture = await restartMatou(fixture)
    await expect(fixture.page.getByRole('heading', { name: '数据库需要恢复' })).toBeVisible()
    await expect(fixture.page.locator('.hierarchy-shell')).toHaveCount(0)
    await expect(fixture.page.getByRole('radio').first()).toBeChecked()
    expect(await fixture.page.getByRole('radio').count()).toBeLessThanOrEqual(7)

    const recoveryBeforeReplay = await fixture.page.evaluate(() => (
      window.matouDesktop.getRuntimeLifecycle()
    ))
    const markerPath = `${databasePath}.recovery.json`
    const markerBeforeReplay = await readFile(markerPath)
    const quarantinedBeforeReplay = await readFile(
      recoveryBeforeReplay.recovery!.quarantinedPath
    )
    const staleReplayError = await fixture.page.evaluate(async (expectedRecoveryId) => {
      try {
        await window.matouDesktop.retryDatabaseOpen(expectedRecoveryId)
        return ''
      } catch (error) {
        return error instanceof Error ? error.message : String(error)
      }
    }, `stale-${recoveryBeforeReplay.recovery!.recoveryId}`)
    expect(staleReplayError).toContain('恢复周期已更新')
    expect(await readFile(markerPath)).toEqual(markerBeforeReplay)
    expect(await readFile(recoveryBeforeReplay.recovery!.quarantinedPath))
      .toEqual(quarantinedBeforeReplay)
    expect(await fixture.page.evaluate(() => window.matouDesktop.getRuntimeLifecycle()))
      .toEqual(recoveryBeforeReplay)

    await fixture.page.getByRole('button', { name: '恢复所选备份' }).click()
    await expect(fixture.page.locator('.workspace-group').first()).toBeVisible()
    await expect(fixture.page.locator('.workspace-group__name').first()).toHaveText(workspaceName ?? '')
  } finally {
    await fixture?.close()
  }
})
