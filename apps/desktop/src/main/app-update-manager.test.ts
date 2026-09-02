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
      installMode: 'automatic',
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
      status: 'error', currentVersion: '1.0.0', errorMessage: 'server unavailable',
      errorStage: 'check'
    })
  })

  it('retries a transient check twice before reporting the app as up to date', async () => {
    const updater = new FakeUpdater()
    updater.checkForUpdates
      .mockRejectedValueOnce(new Error('net::ERR_TIMED_OUT'))
      .mockRejectedValueOnce(new Error('net::ERR_CONNECTION_RESET'))
      .mockImplementationOnce(async () => { updater.emit('update-not-available') })
    const sleep = vi.fn(async () => undefined)
    const published: unknown[] = []
    const manager = new AppUpdateManager({
      updater, enabled: true, currentVersion: '1.0.0',
      checkRetryDelaysMs: [600, 1_600], sleep,
      publish: (state) => published.push(state)
    })
    manager.start()

    await manager.check()

    expect(updater.checkForUpdates).toHaveBeenCalledTimes(3)
    expect(sleep).toHaveBeenNthCalledWith(1, 600)
    expect(sleep).toHaveBeenNthCalledWith(2, 1_600)
    expect(published).toContainEqual({
      status: 'checking', currentVersion: '1.0.0', retryAttempt: 1, maxRetryAttempts: 2
    })
    expect(published).toContainEqual({
      status: 'checking', currentVersion: '1.0.0', retryAttempt: 2, maxRetryAttempts: 2
    })
    expect(manager.state()).toEqual({ status: 'not-available', currentVersion: '1.0.0' })
  })

  it('shows the concrete check failure after automatic retries are exhausted', async () => {
    const updater = new FakeUpdater()
    updater.checkForUpdates.mockRejectedValue(new Error('getaddrinfo ENOTFOUND updates.example.com'))
    const manager = new AppUpdateManager({
      updater, enabled: true, currentVersion: '1.0.0',
      checkRetryDelaysMs: [0, 0], sleep: async () => undefined
    })
    manager.start()

    await manager.check()

    expect(updater.checkForUpdates).toHaveBeenCalledTimes(3)
    expect(manager.state()).toEqual({
      status: 'error', currentVersion: '1.0.0', errorStage: 'check',
      errorMessage: 'getaddrinfo ENOTFOUND updates.example.com'
    })
  })

  it('downloads the DMG in-app with progress and opens it without using the native updater', async () => {
    const updater = new FakeUpdater()
    const downloadManualInstaller = vi.fn(async ({ onProgress }) => {
      onProgress({
        percent: 50, transferredBytes: 13_650_000, totalBytes: 27_300_000,
        bytesPerSecond: 2_000_000, remainingSeconds: 7
      })
      return '/tmp/Matou-1.2.0-mac-arm64.dmg'
    })
    const openManualInstaller = vi.fn(async () => undefined)
    const manager = new AppUpdateManager({
      updater, enabled: true, currentVersion: '1.0.0',
      installMode: 'manual', updateBaseUrl: 'https://updates.example.com/stable',
      downloadManualInstaller, openManualInstaller
    })
    manager.start()
    updater.emit('update-available', {
      version: '1.2.0',
      files: [
        { url: 'Matou-1.2.0-mac-arm64.zip', size: 24_800_000 },
        { url: 'Matou-1.2.0-mac-arm64.dmg', size: 27_300_000, sha512: 'dmg-sha512' }
      ]
    })

    expect(manager.state()).toMatchObject({
      status: 'available', installMode: 'manual',
      manualDownloadUrl: 'https://updates.example.com/stable/Matou-1.2.0-mac-arm64.dmg'
    })
    await manager.download()

    expect(downloadManualInstaller).toHaveBeenCalledWith(expect.objectContaining({
      url: 'https://updates.example.com/stable/Matou-1.2.0-mac-arm64.dmg',
      expectedSha512: 'dmg-sha512', onProgress: expect.any(Function)
    }))
    expect(manager.state()).toMatchObject({ status: 'downloaded', installMode: 'manual' })
    expect(updater.downloadUpdate).not.toHaveBeenCalled()
    await manager.install()
    expect(openManualInstaller).toHaveBeenCalledWith('/tmp/Matou-1.2.0-mac-arm64.dmg')
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
  })

  it('identifies native signature validation failures and keeps the manual DMG recovery action', () => {
    const updater = new FakeUpdater()
    const manager = new AppUpdateManager({
      updater, enabled: true, currentVersion: '1.0.0', installMode: 'automatic',
      updateBaseUrl: 'https://updates.example.com/stable'
    })
    manager.start()
    updater.emit('update-available', {
      version: '1.2.0', files: [{ url: 'Matou-1.2.0-mac-arm64.dmg' }]
    })
    updater.emit('download-progress', {
      percent: 100, transferred: 24_800_000, total: 24_800_000, bytesPerSecond: 1
    })
    updater.emit('error', new Error('Code signature did not pass validation'))

    expect(manager.state()).toEqual({
      status: 'error', currentVersion: '1.0.0', version: '1.2.0',
      errorMessage: 'Code signature did not pass validation', errorStage: 'verify',
      manualDownloadUrl: 'https://updates.example.com/stable/Matou-1.2.0-mac-arm64.dmg'
    })
  })

  it('reports install preparation failures as install errors', async () => {
    const updater = new FakeUpdater()
    const manager = new AppUpdateManager({
      updater, enabled: true, currentVersion: '1.0.0',
      prepareInstall: vi.fn(async () => { throw new Error('runtime shutdown failed') })
    })
    manager.start()
    updater.emit('update-downloaded', { version: '1.2.0' })

    await manager.install()

    expect(manager.state()).toMatchObject({
      status: 'error', errorStage: 'install', errorMessage: 'runtime shutdown failed'
    })
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
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
