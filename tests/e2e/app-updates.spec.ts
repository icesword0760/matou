import { mkdir, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

import { expect, test, type Locator } from '@playwright/test'

import type { AppUpdateState } from '../../apps/desktop/src/shared/desktop-api'
import { launchMatou } from './matou-fixture'

const evidenceDirectory = resolve(import.meta.dirname, '../../docs/acceptance/evidence/app-updates/matou')

test('keeps cloud update states compact, non-modal, and anchored to the app toolbar', async () => {
  const fixture = await launchMatou()
  try {
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1440, 900))
    await mkdir(evidenceDirectory, { recursive: true })
    await expect(fixture.page.getByRole('button', { name: '应用更新' })).toBeVisible()
    await fixture.page.waitForTimeout(200)

    await publishUpdateState(fixture, releaseState('available'))
    const dialog = fixture.page.getByRole('dialog', { name: 'Matou 应用更新' })
    await expect(dialog).toContainText('Matou 0.2.0 可用')
    await expect(dialog).toContainText('云端更新与安全重启')
    await expect(fixture.page.getByRole('button', { name: '后台下载' })).toBeVisible()
    await assertToolbarPlacement(dialog)
    await capture(fixture, 'available')

    await publishUpdateState(fixture, {
      ...releaseState('downloading'),
      progress: {
        percent: 47, transferredBytes: 11_600_000, totalBytes: 24_800_000,
        bytesPerSecond: 733_333, remainingSeconds: 18
      }
    })
    await expect(dialog).toContainText('47% · 约 18 秒')
    await expect(fixture.page.getByRole('button', { name: '应用更新：下载中 47%' })).toBeVisible()
    await capture(fixture, 'downloading')

    await publishUpdateState(fixture, releaseState('downloaded'))
    await expect(dialog).toContainText('更新已准备好')
    await expect(fixture.page.getByRole('button', { name: '重启并更新' })).toBeVisible()
    await capture(fixture, 'downloaded-idle')

    await publishUpdateState(fixture, {
      status: 'error', currentVersion: '0.1.0', errorMessage: 'server unavailable'
    })
    await expect(dialog).toContainText('更新检查失败')
    await expect(fixture.page.getByRole('button', { name: '重新检查' })).toBeVisible()
    await capture(fixture, 'error')

    await writeFile(join(evidenceDirectory, 'geometry.json'), JSON.stringify(
      await dialog.evaluate((element) => {
        const rect = element.getBoundingClientRect()
        const style = getComputedStyle(element)
        return {
          x: Math.round(rect.x), y: Math.round(rect.y), width: Math.round(rect.width),
          height: Math.round(rect.height), borderRadius: style.borderRadius,
          backdropFilter: style.backdropFilter
        }
      }), null, 2
    ))
  } finally {
    await fixture.close()
  }
})

async function publishUpdateState(
  fixture: Awaited<ReturnType<typeof launchMatou>>, state: AppUpdateState
): Promise<void> {
  await fixture.app.evaluate(({ BrowserWindow }, nextState) => {
    for (const window of BrowserWindow.getAllWindows()) {
      window.webContents.send('matou:app-update:state', nextState)
    }
  }, state)
}

function releaseState(status: 'available' | 'downloading' | 'downloaded') {
  return {
    status, currentVersion: '0.1.0', version: '0.2.0',
    releaseDate: '2026-09-01T08:00:00.000Z', sizeBytes: 24_800_000,
    releaseNotes: ['云端更新与安全重启', '优化会话恢复']
  } as AppUpdateState
}

async function assertToolbarPlacement(dialog: Locator) {
  await expect.poll(async () => dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect()
    return {
      top: Math.round(rect.top),
      rightGap: Math.round(window.innerWidth - rect.right),
      width: Math.round(rect.width)
    }
  })).toEqual({ top: 46, rightGap: 8, width: 350 })
}

async function capture(fixture: Awaited<ReturnType<typeof launchMatou>>, name: string): Promise<void> {
  await fixture.page.waitForTimeout(240)
  await fixture.page.locator('.hierarchy-shell').screenshot({ path: join(evidenceDirectory, `${name}.png`) })
}
