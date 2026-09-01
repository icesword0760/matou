import { EventEmitter } from 'node:events'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { AppUpdateManager, type AppUpdaterAdapter } from './app-update-manager'

class FakeUpdater extends EventEmitter implements AppUpdaterAdapter {
  autoDownload = true
  autoInstallOnAppQuit = false
  channel = ''
  checkForUpdates = vi.fn(async () => undefined)
  downloadUpdate = vi.fn(async () => [])
  quitAndInstall = vi.fn()
  setFeedURL = vi.fn()
}

describe('AppUpdateManager', () => {
  afterEach(() => vi.useRealTimers())

  it('silently checks after startup and every four hours', async () => {
    vi.useFakeTimers()
    const updater = new FakeUpdater()
    const manager = new AppUpdateManager({
      updater, enabled: true, currentVersion: '1.0.0',
      initialDelayMs: 15_000, intervalMs: 4 * 60 * 60 * 1_000
    })

    manager.start()
    expect(updater.autoDownload).toBe(false)
    expect(updater.autoInstallOnAppQuit).toBe(true)
    await vi.advanceTimersByTimeAsync(14_999)
    expect(updater.checkForUpdates).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(4 * 60 * 60 * 1_000)
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(2)
    manager.dispose()
  })

  it('does not contact an update server when disabled', async () => {
    vi.useFakeTimers()
    const updater = new FakeUpdater()
    const manager = new AppUpdateManager({ updater, enabled: false, currentVersion: '1.0.0' })
    manager.start()
    await vi.runAllTimersAsync()
    await manager.check()
    expect(updater.checkForUpdates).not.toHaveBeenCalled()
    expect(manager.state()).toEqual({ status: 'idle', currentVersion: '1.0.0' })
  })

  it('publishes normalized available and download progress states', () => {
    const updater = new FakeUpdater()
    const published: unknown[] = []
    const manager = new AppUpdateManager({
      updater, enabled: true, currentVersion: '1.0.0', publish: (state) => published.push(state)
    })
    manager.start()

    updater.emit('checking-for-update')
    updater.emit('update-available', {
      version: '1.2.0', releaseDate: '2026-09-01T08:00:00.000Z',
      files: [{ size: 24_800_000 }],
      releaseNotes: [{ version: '1.2.0', note: '<p>云端更新与安全重启</p><p>优化会话恢复</p>' }]
    })
    updater.emit('download-progress', {
      percent: 47.2, transferred: 11_600_000, total: 24_800_000, bytesPerSecond: 733_333
    })

    expect(manager.state()).toEqual({
      status: 'downloading', currentVersion: '1.0.0', version: '1.2.0',
      releaseDate: '2026-09-01T08:00:00.000Z', sizeBytes: 24_800_000,
      releaseNotes: ['云端更新与安全重启', '优化会话恢复'],
      progress: {
        percent: 47.2, transferredBytes: 11_600_000, totalBytes: 24_800_000,
        bytesPerSecond: 733_333, remainingSeconds: 18
      }
    })
    expect(published).toHaveLength(3)
  })

  it('keeps release information when a download completes', () => {
    const updater = new FakeUpdater()
    const manager = new AppUpdateManager({ updater, enabled: true, currentVersion: '1.0.0' })
    manager.start()
    updater.emit('update-available', { version: '1.2.0', releaseNotes: '更稳定的 Claude 会话' })
    updater.emit('update-downloaded', { version: '1.2.0' })
    expect(manager.state()).toMatchObject({
      status: 'downloaded', currentVersion: '1.0.0', version: '1.2.0',
      releaseNotes: ['更稳定的 Claude 会话']
    })
  })

  it('surfaces a retryable error without replacing the running version', () => {
    const updater = new FakeUpdater()
    const manager = new AppUpdateManager({ updater, enabled: true, currentVersion: '1.0.0' })
    manager.start()
    updater.emit('error', new Error('server unavailable'))
    expect(manager.state()).toEqual({
      status: 'error', currentVersion: '1.0.0', errorMessage: 'server unavailable'
    })
  })

  it('downloads only after the user requests it', async () => {
    const updater = new FakeUpdater()
    const manager = new AppUpdateManager({ updater, enabled: true, currentVersion: '1.0.0' })
    manager.start()
    updater.emit('update-available', { version: '1.2.0' })
    expect(updater.downloadUpdate).not.toHaveBeenCalled()
    await manager.download()
    expect(updater.downloadUpdate).toHaveBeenCalledTimes(1)
  })

  it('shuts down runtime once before installing once', async () => {
    const updater = new FakeUpdater()
    const prepareInstall = vi.fn(async () => undefined)
    const manager = new AppUpdateManager({
      updater, enabled: true, currentVersion: '1.0.0', prepareInstall
    })
    manager.start()
    updater.emit('update-downloaded', { version: '1.2.0' })

    await Promise.all([manager.install(), manager.install()])

    expect(prepareInstall).toHaveBeenCalledTimes(1)
    expect(updater.quitAndInstall).toHaveBeenCalledTimes(1)
    expect(updater.quitAndInstall).toHaveBeenCalledWith(false, true)
  })
})
