// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { AppUpdateReleaseState, AppUpdateState, MatouDesktopApi } from '../../../shared/desktop-api'
import desktopPackage from '../../../../package.json'
import { AppUpdateControl } from './AppUpdateControl'

afterEach(() => {
  cleanup()
  localStorage.clear()
  Reflect.deleteProperty(window, 'matouDesktop')
})

describe('AppUpdateControl', () => {
  it('shows the bundled app version while the desktop bridge is still loading', async () => {
    render(<AppUpdateControl activeSessionCount={0} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '应用更新' }))

    expect(screen.getAllByText(`当前版本 ${desktopPackage.version}`)).toHaveLength(2)
    expect(screen.queryByText(/当前版本\s+—/)).toBeNull()
  })

  it('opens a discovered update and starts download only on user request', async () => {
    const api = installApi(available())
    render(<AppUpdateControl activeSessionCount={0} />)

    expect(await screen.findByText('Matou 1.2.0 可用')).toBeTruthy()
    expect(screen.getByText('云端更新与安全重启')).toBeTruthy()
    expect(api.downloadAppUpdate).not.toHaveBeenCalled()
    await userEvent.setup().click(screen.getByRole('button', { name: '后台下载' }))
    expect(api.downloadAppUpdate).toHaveBeenCalledTimes(1)
  })

  it('offers a manual DMG for locally signed experience builds', async () => {
    const api = installApi({
      ...available(), installMode: 'manual',
      manualDownloadUrl: 'https://updates.example.com/stable/Matou-1.2.0-mac-arm64.dmg'
    })
    render(<AppUpdateControl activeSessionCount={0} />)

    expect(await screen.findByText('当前体验包将通过应用内下载 DMG 更新。')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '下载更新' }))

    expect(api.downloadAppUpdate).toHaveBeenCalledTimes(1)
  })

  it('opens a downloaded manual installer without waiting for active sessions to stop', async () => {
    const api = installApi({ ...downloaded(), installMode: 'manual' })
    render(<AppUpdateControl activeSessionCount={3} />)

    expect(await screen.findByText('DMG 已下载完成')).toBeTruthy()
    expect(screen.queryByText('当前有 3 个活动会话')).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: '打开 DMG 安装' }))

    expect(api.installAppUpdate).toHaveBeenCalledTimes(1)
  })

  it('renders live progress without requiring the popover to remain open', async () => {
    const api = installApi(downloading())
    render(<AppUpdateControl activeSessionCount={0} />)

    await userEvent.setup().click(await screen.findByRole('button', { name: '应用更新：下载中 47%' }))
    expect(screen.getByText('47% · 约 18 秒')).toBeTruthy()
    expect(screen.getByText('11.6 MB / 24.8 MB')).toBeTruthy()

    api.publish({ ...downloading(), progress: { ...downloading().progress, percent: 63 } })
    await waitFor(() => expect(screen.getByRole('button', { name: '应用更新：下载中 63%' })).toBeTruthy())
  })

  it('waits for active sessions to become idle before installing once', async () => {
    const api = installApi(downloaded())
    const view = render(<AppUpdateControl activeSessionCount={3} />)
    expect(await screen.findByText('当前有 3 个活动会话')).toBeTruthy()

    await userEvent.setup().click(screen.getByRole('button', { name: '空闲后自动更新' }))
    expect(screen.getByText('已安排：空闲后自动更新')).toBeTruthy()
    expect(api.installAppUpdate).not.toHaveBeenCalled()

    view.rerender(<AppUpdateControl activeSessionCount={0} />)
    await waitFor(() => expect(api.installAppUpdate).toHaveBeenCalledTimes(1))
    view.rerender(<AppUpdateControl activeSessionCount={0} />)
    expect(api.installAppUpdate).toHaveBeenCalledTimes(1)
  })

  it('lets the user cancel an idle-install request before sessions finish', async () => {
    const api = installApi(downloaded())
    const view = render(<AppUpdateControl activeSessionCount={2} />)

    await userEvent.setup().click(await screen.findByRole('button', { name: '空闲后自动更新' }))
    await userEvent.setup().click(screen.getByRole('button', { name: '取消空闲更新' }))
    expect(screen.queryByText('已安排：空闲后自动更新')).toBeNull()

    view.rerender(<AppUpdateControl activeSessionCount={0} />)
    expect(api.installAppUpdate).not.toHaveBeenCalled()
  })

  it('allows immediate restart or install on normal exit after download', async () => {
    const api = installApi(downloaded())
    render(<AppUpdateControl activeSessionCount={0} />)

    expect(await screen.findByRole('button', { name: '退出时安装' })).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '重启并更新' }))
    expect(api.installAppUpdate).toHaveBeenCalledTimes(1)
  })

  it('closes on Escape and outside pointer input', async () => {
    installApi(available())
    render(<div><AppUpdateControl activeSessionCount={0} /><button>外部区域</button></div>)
    expect(await screen.findByRole('dialog', { name: 'Matou 应用更新' })).toBeTruthy()

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: 'Matou 应用更新' })).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: /应用更新/ }))
    expect(screen.getByRole('dialog', { name: 'Matou 应用更新' })).toBeTruthy()
    fireEvent.pointerDown(screen.getByRole('button', { name: '外部区域' }))
    expect(screen.queryByRole('dialog', { name: 'Matou 应用更新' })).toBeNull()
  })

  it('shows a retry action after an update error', async () => {
    const api = installApi({
      status: 'error', currentVersion: '1.0.0', errorMessage: 'server unavailable',
      errorStage: 'check'
    })
    render(<AppUpdateControl activeSessionCount={0} />)
    await userEvent.setup().click(await screen.findByRole('button', { name: /应用更新/ }))
    expect(screen.getByText('更新检查失败')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '重新检查' }))
    expect(api.checkForAppUpdates).toHaveBeenCalledTimes(1)
  })

  it('shows automatic retry progress without presenting a failure too early', async () => {
    installApi({
      status: 'checking', currentVersion: '1.0.0', retryAttempt: 2, maxRetryAttempts: 2
    })
    render(<AppUpdateControl activeSessionCount={0} />)

    await userEvent.setup().click(await screen.findByRole('button', { name: '应用更新：正在检查' }))

    expect(screen.getByText('连接波动，正在自动重试（2/2）…')).toBeTruthy()
  })

  it('explains DNS and timeout failures with actionable reasons', async () => {
    const api = installApi({
      status: 'error', currentVersion: '1.0.0',
      errorMessage: 'getaddrinfo ENOTFOUND updates.example.com', errorStage: 'check'
    })
    render(<AppUpdateControl activeSessionCount={0} />)
    await userEvent.setup().click(await screen.findByRole('button', { name: /应用更新/ }))
    expect(screen.getByText('更新服务器域名解析失败，请检查网络或 DNS 设置。')).toBeTruthy()

    api.publish({
      status: 'error', currentVersion: '1.0.0',
      errorMessage: 'net::ERR_TIMED_OUT', errorStage: 'check'
    })
    expect(await screen.findByText('连接更新服务器超时，请检查网络后重试。')).toBeTruthy()
  })

  it('explains signature validation failures and offers the DMG recovery action', async () => {
    const api = installApi({
      status: 'error', currentVersion: '1.0.0', version: '1.2.0',
      errorMessage: 'Code signature did not pass validation', errorStage: 'verify',
      manualDownloadUrl: 'https://updates.example.com/stable/Matou-1.2.0-mac-arm64.dmg'
    })
    render(<AppUpdateControl activeSessionCount={0} />)

    await userEvent.setup().click(await screen.findByRole('button', { name: '应用更新：安装包校验未通过' }))
    expect(screen.getByText('安装包校验未通过')).toBeTruthy()
    expect(screen.getByText('当前安装包缺少 Apple 发布签名，改用应用内 DMG 下载继续更新。')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '下载 DMG 更新' }))
    expect(api.downloadAppUpdate).toHaveBeenCalledTimes(1)
  })
})

function installApi(initial: AppUpdateState) {
  let listener: ((state: AppUpdateState) => void) | undefined
  const api = {
    getAppUpdateState: vi.fn(async () => initial),
    checkForAppUpdates: vi.fn(async () => undefined),
    downloadAppUpdate: vi.fn(async () => undefined),
    installAppUpdate: vi.fn(async () => undefined),
    onAppUpdateState: vi.fn((next: (state: AppUpdateState) => void) => {
      listener = next
      return () => { listener = undefined }
    }),
    publish: (state: AppUpdateState) => listener?.(state)
  }
  Object.defineProperty(window, 'matouDesktop', {
    configurable: true, value: api as unknown as MatouDesktopApi
  })
  return api
}

function available(): AppUpdateReleaseState & { status: 'available' } {
  return {
    status: 'available', currentVersion: '1.0.0', version: '1.2.0',
    releaseDate: '2026-09-01T08:00:00.000Z', sizeBytes: 24_800_000,
    releaseNotes: ['云端更新与安全重启', '优化会话恢复'], installMode: 'automatic'
  }
}

function downloaded(): AppUpdateReleaseState & { status: 'downloaded' } {
  return { ...available(), status: 'downloaded' }
}

function downloading(): AppUpdateReleaseState & Extract<AppUpdateState, { progress: unknown }> {
  return {
    ...available(), status: 'downloading',
    progress: {
      percent: 47, transferredBytes: 11_600_000, totalBytes: 24_800_000,
      bytesPerSecond: 733_333, remainingSeconds: 18
    }
  }
}
